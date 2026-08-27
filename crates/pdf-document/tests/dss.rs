//! Document Security Store (DSS) / VRI round-trip — ISO 32000-2 §12.8.4.3 / §12.8.4.4 (LTV).
//! Sign a PDF, append a `/DSS` holding the validation material as an incremental update, and read
//! it back; check VRI keys bind to the signature and that signing material survives untouched.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::str::FromStr;
use std::time::Duration;

use der::Encode;
use der::asn1::{BitString, GeneralizedTime};
use pdf_document::{
    Document, RevocationSummary, SignSettings, SignatureValidation, ValidationData,
};
use rsa::pkcs1v15::SigningKey;
use rsa::pkcs8::EncodePrivateKey;
use rsa::signature::{SignatureEncoding, Signer};
use rsa::{RsaPrivateKey, RsaPublicKey};
use sha2::Sha256;
use x509_cert::Certificate;
use x509_cert::builder::{Builder, CertificateBuilder, Profile};
use x509_cert::crl::{CertificateList, RevokedCert, TbsCertList};
use x509_cert::name::Name;
use x509_cert::serial_number::SerialNumber;
use x509_cert::spki::{AlgorithmIdentifierOwned, EncodePublicKey, SubjectPublicKeyInfoOwned};
use x509_cert::time::{Time, Validity};

/// Generate an RSA-2048 keypair + self-signed cert; return (cert DER, PKCS#8 key DER).
fn self_signed(cn: &str) -> (Vec<u8>, Vec<u8>) {
    let mut rng = rand_core::OsRng;
    let key = RsaPrivateKey::new(&mut rng, 2048).expect("rsa keygen");
    let spki = SubjectPublicKeyInfoOwned::try_from(
        RsaPublicKey::from(&key)
            .to_public_key_der()
            .unwrap()
            .as_bytes(),
    )
    .unwrap();
    let signer = SigningKey::<Sha256>::new(key.clone());
    let cert = CertificateBuilder::new(
        Profile::Root,
        SerialNumber::from(1u32),
        Validity::from_now(Duration::from_secs(3600)).unwrap(),
        Name::from_str(&format!("CN={cn}")).unwrap(),
        spki,
        &signer,
    )
    .unwrap()
    .build()
    .unwrap();
    (
        cert.to_der().unwrap(),
        key.to_pkcs8_der().unwrap().as_bytes().to_vec(),
    )
}

/// A minimal one-page PDF (classic xref).
fn one_page_pdf() -> Vec<u8> {
    let objects: [&[u8]; 3] = [
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >>",
    ];
    let mut buf = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in objects.iter().enumerate() {
        offsets.push(buf.len());
        buf.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        buf.extend_from_slice(body);
        buf.extend_from_slice(b"\nendobj\n");
    }
    let startxref = buf.len();
    buf.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
    );
    for off in &offsets {
        buf.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
    }
    buf.extend_from_slice(
        format!("trailer\n<< /Size {} /Root 1 0 R >>\n", objects.len() + 1).as_bytes(),
    );
    buf.extend_from_slice(format!("startxref\n{startxref}\n%%EOF\n").as_bytes());
    buf
}

/// Sign `one_page_pdf` and return the signed bytes.
fn signed_pdf(cert: &[u8], key: &[u8]) -> Vec<u8> {
    Document::open(one_page_pdf())
        .unwrap()
        .sign(cert, key)
        .unwrap()
}

/// A CA (self-signed root) and a CA-issued leaf: `(ca_der, ca_key, leaf_der, leaf_key_pkcs8)`.
fn ca_and_leaf() -> (Vec<u8>, RsaPrivateKey, Vec<u8>, Vec<u8>) {
    let mut rng = rand_core::OsRng;
    let ca_key = RsaPrivateKey::new(&mut rng, 2048).expect("rsa keygen");
    let ca_spki = SubjectPublicKeyInfoOwned::try_from(
        RsaPublicKey::from(&ca_key)
            .to_public_key_der()
            .unwrap()
            .as_bytes(),
    )
    .unwrap();
    let ca_name = Name::from_str("CN=LTV Test CA").unwrap();
    let ca_signer = SigningKey::<Sha256>::new(ca_key.clone());
    let ca = CertificateBuilder::new(
        Profile::Root,
        SerialNumber::from(1u32),
        Validity::from_now(Duration::from_secs(7200)).unwrap(),
        ca_name.clone(),
        ca_spki,
        &ca_signer,
    )
    .unwrap()
    .build()
    .unwrap();

    let leaf_key = RsaPrivateKey::new(&mut rng, 2048).expect("rsa keygen");
    let leaf_spki = SubjectPublicKeyInfoOwned::try_from(
        RsaPublicKey::from(&leaf_key)
            .to_public_key_der()
            .unwrap()
            .as_bytes(),
    )
    .unwrap();
    let leaf = CertificateBuilder::new(
        Profile::Leaf {
            issuer: ca_name,
            enable_key_agreement: false,
            enable_key_encipherment: false,
        },
        SerialNumber::from(7u32),
        Validity::from_now(Duration::from_secs(3600)).unwrap(),
        Name::from_str("CN=LTV Test Signer").unwrap(),
        leaf_spki,
        &ca_signer,
    )
    .unwrap()
    .build()
    .unwrap();
    (
        ca.to_der().unwrap(),
        ca_key,
        leaf.to_der().unwrap(),
        leaf_key.to_pkcs8_der().unwrap().as_bytes().to_vec(),
    )
}

/// A CRL issued and signed by the CA, listing `revoked` serials, valid ±1 h around `now`.
fn make_crl(ca_der: &[u8], ca_key: &RsaPrivateKey, revoked: &[u32], now: u64) -> Vec<u8> {
    use der::Decode;
    let ca = Certificate::from_der(ca_der).unwrap();
    let alg = AlgorithmIdentifierOwned {
        oid: der::oid::db::rfc5912::SHA_256_WITH_RSA_ENCRYPTION,
        parameters: None,
    };
    let gtime = |secs: u64| {
        Time::GeneralTime(GeneralizedTime::from_unix_duration(Duration::from_secs(secs)).unwrap())
    };
    let entries: Vec<RevokedCert> = revoked
        .iter()
        .map(|serial| RevokedCert {
            serial_number: SerialNumber::from(*serial),
            revocation_date: gtime(now - 60),
            crl_entry_extensions: None,
        })
        .collect();
    let tbs = TbsCertList {
        version: x509_cert::Version::V2,
        signature: alg.clone(),
        issuer: ca.tbs_certificate.subject.clone(),
        this_update: gtime(now - 3600),
        next_update: Some(gtime(now + 3600)),
        revoked_certificates: (!entries.is_empty()).then_some(entries),
        crl_extensions: None,
    };
    let sig = SigningKey::<Sha256>::new(ca_key.clone()).sign(&tbs.to_der().unwrap());
    CertificateList {
        tbs_cert_list: tbs,
        signature_algorithm: alg,
        signature: BitString::from_bytes(&sig.to_vec()).unwrap(),
    }
    .to_der()
    .unwrap()
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

#[test]
fn no_dss_on_unsigned_document() {
    let doc = Document::open(one_page_pdf()).unwrap();
    assert_eq!(doc.validation_info().unwrap(), None);
    assert!(doc.signature_vri_keys().unwrap().is_empty());
}

#[test]
fn add_dss_round_trip_preserves_signature() {
    let (cert, key) = self_signed("LTV Signer");
    let signed = signed_pdf(&cert, &key);

    // Append a DSS with the signer cert plus a stand-in OCSP and CRL (stored verbatim as DER).
    let data = ValidationData {
        certs: vec![cert.clone()],
        ocsps: vec![b"fake-ocsp-response-der".to_vec()],
        crls: vec![b"fake-crl-der".to_vec()],
    };
    let with_dss = Document::open(signed.clone())
        .unwrap()
        .add_validation_info(&data, &[])
        .unwrap();

    // Append-only: the signed revision is a verbatim prefix, so the signature still verifies.
    assert!(with_dss.starts_with(&signed));

    let reopened = Document::open(with_dss).unwrap();
    let sigs = reopened.verify_signatures().unwrap();
    assert_eq!(sigs.len(), 1);
    assert!(sigs[0].valid, "signature survives the DSS increment");

    let info = reopened.validation_info().unwrap().expect("a DSS");
    assert_eq!(info.certs, vec![cert]);
    assert_eq!(info.ocsps, vec![b"fake-ocsp-response-der".to_vec()]);
    assert_eq!(info.crls, vec![b"fake-crl-der".to_vec()]);
    assert!(info.vri_keys.is_empty(), "no VRI requested");
}

#[test]
fn vri_keyed_to_signature() {
    let (cert, key) = self_signed("LTV Signer");
    let signed = signed_pdf(&cert, &key);
    let doc = Document::open(signed).unwrap();

    // The VRI key for the one signature in the document.
    let keys = doc.signature_vri_keys().unwrap();
    assert_eq!(keys.len(), 1, "one signature → one VRI key");
    let key0 = keys[0].clone();
    assert_eq!(key0.len(), 40, "SHA-1 hex is 40 chars");
    assert!(key0.bytes().all(|b| b.is_ascii_hexdigit()));

    // The same cert appears both document-wide and in the VRI: it must be stored once and shared.
    let data = ValidationData {
        certs: vec![cert.clone()],
        ..ValidationData::default()
    };
    let vri = SignatureValidation {
        key: key0.clone(),
        data: ValidationData {
            certs: vec![cert.clone()],
            ocsps: vec![b"ocsp-for-sig".to_vec()],
            ..ValidationData::default()
        },
        created: Some(1_700_000_000),
        timestamp_token: Some(b"rfc3161-token-der".to_vec()),
    };
    let with_dss = doc.add_validation_info(&data, &[vri]).unwrap();
    // The VRI carries its /TS timestamp-token stream (§12.8.4.4, Table 262).
    assert!(
        with_dss.windows(3).any(|w| w == b"/TS"),
        "VRI entry has /TS"
    );
    assert!(
        with_dss
            .windows(b"rfc3161-token-der".len())
            .any(|w| w == b"rfc3161-token-der"),
        "token bytes embedded verbatim"
    );

    let reopened = Document::open(with_dss).unwrap();
    let info = reopened.validation_info().unwrap().expect("a DSS");
    // Deduplicated: one cert stream despite appearing in both the DSS array and the VRI.
    assert_eq!(info.certs, vec![cert]);
    assert_eq!(info.ocsps, vec![b"ocsp-for-sig".to_vec()]);
    assert_eq!(info.vri_keys, vec![key0]);
}

#[test]
fn pades_lt_round_trip_reports_revocation() {
    // PAdES-LT (§12.8.4.3): a CA-issued leaf signs PAdES-B; the DSS embeds the chain and a clean
    // CRL; LTV verification against the CA reports Good — entirely offline.
    let (ca, ca_key, leaf, leaf_key) = ca_and_leaf();
    let now = now_secs();
    let signed = Document::open(one_page_pdf())
        .unwrap()
        .sign_with(
            &leaf,
            &leaf_key,
            &SignSettings {
                signing_time: Some(now),
                pades: true,
                ..Default::default()
            },
        )
        .unwrap();
    let doc = Document::open(signed).unwrap();

    let vri_key = doc.signature_vri_keys().unwrap()[0].clone();
    let crl = make_crl(&ca, &ca_key, &[], now);
    let data = ValidationData {
        certs: vec![ca.clone(), leaf.clone()],
        crls: vec![crl.clone()],
        ..Default::default()
    };
    let vri = SignatureValidation {
        key: vri_key,
        data: ValidationData {
            crls: vec![crl],
            ..Default::default()
        },
        created: Some(now),
        timestamp_token: None,
    };
    let lt = doc.add_validation_info(&data, &[vri]).unwrap();

    let reopened = Document::open(lt).unwrap();
    let sigs = reopened
        .verify_signatures_ltv(std::slice::from_ref(&ca))
        .unwrap();
    assert_eq!(sigs.len(), 1);
    assert!(sigs[0].valid && sigs[0].pades);
    assert_eq!(sigs[0].trusted, Some(true));
    assert_eq!(sigs[0].revocation, Some(RevocationSummary::Good));

    // The plain trust-store verification does not evaluate revocation.
    let sigs = reopened.verify_signatures_with(&[ca]).unwrap();
    assert_eq!(sigs[0].revocation, None);
}

#[test]
fn pades_lt_reports_revoked_and_incomplete() {
    let (ca, ca_key, leaf, leaf_key) = ca_and_leaf();
    let now = now_secs();
    let signed = Document::open(one_page_pdf())
        .unwrap()
        .sign_with(
            &leaf,
            &leaf_key,
            &SignSettings {
                signing_time: Some(now),
                pades: true,
                ..Default::default()
            },
        )
        .unwrap();
    let doc = Document::open(signed).unwrap();

    // No DSS at all → the chain has no evidence: Incomplete.
    let sigs = doc
        .verify_signatures_ltv(std::slice::from_ref(&ca))
        .unwrap();
    assert_eq!(sigs[0].revocation, Some(RevocationSummary::Incomplete));

    // A CRL revoking the leaf's serial (7) → Revoked.
    let data = ValidationData {
        certs: vec![ca.clone(), leaf.clone()],
        crls: vec![make_crl(&ca, &ca_key, &[7], now)],
        ..Default::default()
    };
    let revoked = doc.add_validation_info(&data, &[]).unwrap();
    let sigs = Document::open(revoked)
        .unwrap()
        .verify_signatures_ltv(&[ca])
        .unwrap();
    assert_eq!(sigs[0].revocation, Some(RevocationSummary::Revoked));
    assert!(sigs[0].valid, "revocation does not undo signature validity");
}

#[test]
fn pades_lta_timestamp_over_dss() {
    // PAdES-LTA (§12.8.4.4 context): after the DSS increment, a document timestamp (§12.8.5)
    // covers signature + validation material; everything keeps verifying and the DSS survives.
    let (ca, ca_key, leaf, leaf_key) = ca_and_leaf();
    let (tsa_cert, tsa_key) = self_signed("LTV TSA");
    let now = now_secs();
    let signed = Document::open(one_page_pdf())
        .unwrap()
        .sign_with(
            &leaf,
            &leaf_key,
            &SignSettings {
                signing_time: Some(now),
                pades: true,
                ..Default::default()
            },
        )
        .unwrap();
    let data = ValidationData {
        certs: vec![ca.clone(), leaf.clone()],
        crls: vec![make_crl(&ca, &ca_key, &[], now)],
        ..Default::default()
    };
    let lt = Document::open(signed)
        .unwrap()
        .add_validation_info(&data, &[])
        .unwrap();
    let lta = Document::open(lt.clone())
        .unwrap()
        .timestamp(&tsa_cert, &tsa_key, Some(now))
        .unwrap();
    assert!(lta.starts_with(&lt), "the DTS is append-only over the DSS");

    let reopened = Document::open(lta).unwrap();
    // The DSS is still readable through the timestamp increment.
    let info = reopened.validation_info().unwrap().expect("a DSS");
    assert_eq!(info.crls.len(), 1);

    // Both the original signature (with LT revocation) and the DTS verify. The TSA is its own
    // anchor here, so trust it too.
    let sigs = reopened
        .verify_signatures_ltv(&[ca, tsa_cert.clone()])
        .unwrap();
    assert_eq!(sigs.len(), 2, "the signature and the document timestamp");
    let sig = sigs.iter().find(|s| s.pades).expect("the PAdES signature");
    assert!(sig.valid);
    assert_eq!(sig.trusted, Some(true));
    assert_eq!(sig.revocation, Some(RevocationSummary::Good));
    let dts = sigs
        .iter()
        .find(|s| s.timestamp_time.is_some() && !s.pades)
        .expect("the document timestamp");
    assert!(dts.valid);
    assert_eq!(dts.trusted, Some(true));
    // The TSA chain is the self-signed anchor itself: nothing to check → no links → Good.
    assert_eq!(dts.revocation, Some(RevocationSummary::Good));
}
