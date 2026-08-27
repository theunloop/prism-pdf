//! Digital signature round-trip (ISO 32000-1 §12.8) through the document layer: sign a real PDF as
//! an incremental update, then verify the detached CMS over the `/ByteRange`. The throwaway RSA
//! keypair + self-signed certificate are generated here (RustCrypto), so no key material is baked in.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::str::FromStr;
use std::time::Duration;

use der::Encode;
use pdf_document::{Document, SignSettings, SignatureAppearance, TsaCredentials};
use rsa::pkcs1v15::SigningKey;
use rsa::pkcs8::EncodePrivateKey;
use rsa::{RsaPrivateKey, RsaPublicKey};
use sha2::Sha256;
use x509_cert::builder::{Builder, CertificateBuilder, Profile};
use x509_cert::name::Name;
use x509_cert::serial_number::SerialNumber;
use x509_cert::spki::{EncodePublicKey, SubjectPublicKeyInfoOwned};
use x509_cert::time::Validity;

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

#[test]
fn sign_then_verify_round_trip() {
    let (cert, key) = self_signed("Prism PDF Signer");
    let doc = Document::open(one_page_pdf()).unwrap();

    let signed = doc.sign(&cert, &key).unwrap();
    // The signature is an append-only incremental update: the original file is a byte prefix.
    assert!(signed.starts_with(&one_page_pdf()));

    let reopened = Document::open(signed.clone()).unwrap();
    let signatures = reopened.verify_signatures().unwrap();
    assert_eq!(signatures.len(), 1, "exactly one signature");
    assert!(signatures[0].valid, "signature should verify");
    assert!(
        signatures[0]
            .signer
            .as_ref()
            .unwrap()
            .contains("Prism PDF Signer")
    );
    // The signed range covers almost the whole file (all but the /Contents hole).
    assert!(signatures[0].covered_bytes > one_page_pdf().len());

    // The form field is present and the document still opens with one page.
    assert_eq!(reopened.page_count().unwrap(), 1);
}

#[test]
fn records_signing_time_in_dict_and_cms() {
    let (cert, key) = self_signed("Timed Signer");
    let doc = Document::open(one_page_pdf()).unwrap();
    let settings = SignSettings {
        signing_time: Some(1_700_000_000),
        reason: Some("I approve this document".to_string()),
        ..SignSettings::default()
    };
    let signed = doc.sign_with(&cert, &key, &settings).unwrap();

    // The signature dictionary carries /M (a PDF date) and /Reason.
    assert!(find(&signed, b"/M (D:2023").is_some(), "/M date present");
    assert!(
        find(&signed, b"/Reason (I approve").is_some(),
        "/Reason present"
    );

    let reopened = Document::open(signed).unwrap();
    let signatures = reopened.verify_signatures().unwrap();
    assert_eq!(signatures.len(), 1);
    assert!(signatures[0].valid);
    // The CMS signingTime attribute agrees with /M.
    assert_eq!(signatures[0].signing_time, Some(1_700_000_000));
}

#[test]
fn visible_appearance_emits_form_xobject() {
    let (cert, key) = self_signed("Visible Signer");
    let doc = Document::open(one_page_pdf()).unwrap();
    let settings = SignSettings {
        name: Some("Alice".to_string()),
        signing_time: Some(1_700_000_000),
        appearance: Some(SignatureAppearance {
            page_index: 0,
            rect: [20.0, 20.0, 180.0, 70.0],
            text: None,
        }),
        ..SignSettings::default()
    };
    let signed = doc.sign_with(&cert, &key, &settings).unwrap();

    // A Form XObject appearance with a Helvetica font was emitted and bound via /AP.
    assert!(find(&signed, b"/Subtype /Form").is_some(), "form xobject");
    assert!(
        find(&signed, b"/BaseFont /Helvetica").is_some(),
        "helv font"
    );
    assert!(find(&signed, b"/AP").is_some(), "appearance dict on widget");
    assert!(
        find(&signed, b"Digitally signed by Alice").is_some(),
        "default text"
    );

    let reopened = Document::open(signed).unwrap();
    assert_eq!(reopened.page_count().unwrap(), 1);
    let signatures = reopened.verify_signatures().unwrap();
    assert_eq!(signatures.len(), 1);
    assert!(signatures[0].valid, "visible signature still verifies");
}

#[test]
fn trust_store_reports_trust() {
    let (cert, key) = self_signed("Self Root");
    let doc = Document::open(one_page_pdf()).unwrap();
    let signed = doc.sign(&cert, &key).unwrap();
    let reopened = Document::open(signed).unwrap();

    // Signer cert as its own anchor → trusted; no roots → not evaluated.
    let trusted = reopened
        .verify_signatures_with(std::slice::from_ref(&cert))
        .unwrap();
    assert_eq!(trusted[0].trusted, Some(true));
    assert!(trusted[0].valid);

    let none = reopened.verify_signatures().unwrap();
    assert_eq!(none[0].trusted, None);

    let (other, _) = self_signed("Stranger");
    let untrusted = reopened.verify_signatures_with(&[other]).unwrap();
    assert_eq!(untrusted[0].trusted, Some(false));
}

#[test]
fn embedded_timestamp_round_trips() {
    let (cert, key) = self_signed("Stamped Signer");
    let (tsa_cert, tsa_key) = self_signed("Prism PDF TSA");
    let doc = Document::open(one_page_pdf()).unwrap();
    let settings = SignSettings {
        signing_time: Some(1_700_000_000),
        timestamp: Some(TsaCredentials {
            cert_der: tsa_cert,
            key_der: tsa_key,
            gen_time: 1_700_000_500,
            serial: 7,
        }),
        ..SignSettings::default()
    };
    let signed = doc.sign_with(&cert, &key, &settings).unwrap();
    let reopened = Document::open(signed).unwrap();
    let signatures = reopened.verify_signatures().unwrap();
    assert_eq!(signatures.len(), 1);
    assert!(signatures[0].valid);
    assert_eq!(signatures[0].timestamp_time, Some(1_700_000_500));
}

/// The first occurrence of `needle` in `haystack`.
fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

#[test]
fn pades_b_signature_round_trips() {
    // A PAdES-B signature: /SubFilter /ETSI.CAdES.detached + the signing-certificate-v2 signed
    // attribute. Verify confirms the attribute binds the embedded certificate.
    let (cert, key) = self_signed("PAdES Signer");
    let doc = Document::open(one_page_pdf()).unwrap();
    let settings = SignSettings {
        signing_time: Some(1_700_000_000),
        pades: true,
        ..SignSettings::default()
    };
    let signed = doc.sign_with(&cert, &key, &settings).unwrap();
    assert!(
        find(&signed, b"/SubFilter /ETSI.CAdES.detached").is_some(),
        "PAdES subfilter"
    );

    let reopened = Document::open(signed).unwrap();
    let sigs = reopened.verify_signatures().unwrap();
    assert_eq!(sigs.len(), 1);
    assert!(sigs[0].valid, "PAdES signature verifies");
    assert!(sigs[0].pades, "signing-certificate-v2 binds the cert");

    // A plain (non-PAdES) signature does not carry the binding.
    let plain = Document::open(one_page_pdf())
        .unwrap()
        .sign(&cert, &key)
        .unwrap();
    let plain_sigs = Document::open(plain).unwrap().verify_signatures().unwrap();
    assert!(!plain_sigs[0].pades);
}

#[test]
fn document_timestamp_round_trips() {
    // A document timestamp (DTS, §12.8.5): a /DocTimeStamp signature whose /Contents is an RFC 3161
    // token over the /ByteRange bytes. Verifies via the same verify_signatures path.
    let (tsa_cert, tsa_key) = self_signed("Prism PDF TSA");
    let doc = Document::open(one_page_pdf()).unwrap();

    let stamped = doc
        .timestamp(&tsa_cert, &tsa_key, Some(1_700_000_900))
        .unwrap();
    // Append-only incremental update; the /DocTimeStamp dict and ETSI subfilter are present.
    assert!(stamped.starts_with(&one_page_pdf()));
    assert!(find(&stamped, b"/Type /DocTimeStamp").is_some());
    assert!(find(&stamped, b"/SubFilter /ETSI.RFC3161").is_some());

    let reopened = Document::open(stamped).unwrap();
    let sigs = reopened.verify_signatures().unwrap();
    assert_eq!(sigs.len(), 1, "exactly one timestamp");
    assert!(sigs[0].valid, "DTS token must verify over the ByteRange");
    assert_eq!(sigs[0].timestamp_time, Some(1_700_000_900));
    assert!(sigs[0].signer.as_ref().unwrap().contains("Prism PDF TSA"));
    assert_eq!(reopened.page_count().unwrap(), 1);
}

#[test]
fn tampering_after_timestamp_invalidates() {
    let (tsa_cert, tsa_key) = self_signed("Prism PDF TSA");
    let doc = Document::open(one_page_pdf()).unwrap();
    let mut stamped = doc
        .timestamp(&tsa_cert, &tsa_key, Some(1_700_000_900))
        .unwrap();

    stamped[1] ^= 0x01; // flip a byte inside the covered region

    let reopened = Document::open(stamped).unwrap();
    let sigs = reopened.verify_signatures().unwrap();
    assert_eq!(sigs.len(), 1);
    assert!(!sigs[0].valid, "tampered document must not verify");
}

#[test]
fn tampering_after_signing_invalidates() {
    let (cert, key) = self_signed("Prism PDF Signer");
    let doc = Document::open(one_page_pdf()).unwrap();
    let mut signed = doc.sign(&cert, &key).unwrap();

    // Flip a byte inside the signed region (the original %PDF header area).
    signed[1] ^= 0x01;

    let reopened = Document::open(signed).unwrap();
    let signatures = reopened.verify_signatures().unwrap();
    assert_eq!(signatures.len(), 1);
    assert!(!signatures[0].valid, "tampered document must not verify");
}

#[test]
fn sign_with_attached_pdf_mac_round_trips() {
    use pdf_document::Algorithm;
    let (cert, key) = self_signed("MAC Signer");

    // Base: an AES-256 (V5/R6) encrypted PDF carrying a standalone MAC and a /KDFSalt (ISO/TS 32004).
    let base = Document::open(one_page_pdf())
        .unwrap()
        .save_encrypted_with_mac(b"", b"", Algorithm::Aes256)
        .unwrap();

    // Sign it, attaching a PDF MAC token to the signature (§6.5.2).
    let signed = Document::open(base)
        .unwrap()
        .sign_with_mac(&cert, &key, &SignSettings::default(), b"")
        .unwrap();
    assert!(
        signed
            .windows("/AttachedToSig".len())
            .any(|w| w == b"/AttachedToSig"),
        "trailer /AuthCode points the MAC at the signature"
    );

    // The attached MAC authenticates the signed bytes.
    let out = Document::open(signed.clone()).unwrap();
    assert_eq!(out.verify_pdf_mac(b"").unwrap(), Some(true));

    // Tamper with a covered byte (the binary-marker comment) → the MAC fails.
    let mut bad = signed;
    bad[11] ^= 0xFF;
    assert_eq!(
        Document::open(bad).unwrap().verify_pdf_mac(b"").unwrap(),
        Some(false)
    );
}

#[test]
fn sign_with_mac_requires_kdfsalt() {
    use pdf_document::{Algorithm, DocError};
    let (cert, key) = self_signed("No Salt");
    // A plainly-encrypted doc has no /KDFSalt, so an attached MAC cannot be keyed.
    let encrypted = Document::open(one_page_pdf())
        .unwrap()
        .save_encrypted(b"", b"", Algorithm::Aes256)
        .unwrap();
    let err = Document::open(encrypted)
        .unwrap()
        .sign_with_mac(&cert, &key, &SignSettings::default(), b"")
        .unwrap_err();
    assert_eq!(err, DocError::MacFailed);
}

#[test]
fn sign_with_mac_requires_encryption() {
    use pdf_document::{Algorithm, DocError};
    let (cert, key) = self_signed("Unencrypted");
    let _ = Algorithm::Aes256;
    // A plain (unencrypted) document has no file key to derive the MAC key from.
    let err = Document::open(one_page_pdf())
        .unwrap()
        .sign_with_mac(&cert, &key, &SignSettings::default(), b"")
        .unwrap_err();
    assert_eq!(err, DocError::MacRequiresV5);
}
