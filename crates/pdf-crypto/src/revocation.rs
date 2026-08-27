//! Revocation checking for **PAdES-LT** (ISO 32000-2 §12.8.4.3): decide each chain link's status
//! from caller-supplied **OCSP responses** (RFC 6960) and **CRLs** (RFC 5280 §5) — the raw DER
//! blobs a PDF `/DSS` carries. No I/O happens here (DESIGN.md §3): in a deployment the material
//! comes from the network once and is embedded in the DSS; validation then works offline forever,
//! which is the point of the LT profile.
//!
//! Input is untrusted: everything is parsed defensively and a blob that fails to parse — or whose
//! signature does not verify against the certificate's issuer — is simply *not evidence* (the
//! status stays [`RevocationStatus::Unknown`] rather than failing the verification).

use der::asn1::ObjectIdentifier;
use der::oid::db::rfc5912;
use der::{Decode, Encode};
use sha1::Sha1;
use sha2::{Digest, Sha256};
use x509_cert::Certificate;
use x509_cert::crl::CertificateList;
use x509_ocsp::{BasicOcspResponse, CertStatus, OcspResponse, OcspResponseStatus};

use crate::sign::{certified_by, is_ocsp_signer, rsa_verifies};

/// Revocation material for validation: raw DER OCSP responses and CRLs — e.g. the streams read
/// back from a document's `/DSS` (`/OCSPs` + `/CRLs`, §12.8.4.3).
#[derive(Clone, Debug, Default)]
pub struct RevocationData {
    /// DER `OCSPResponse` blobs (RFC 6960).
    pub ocsps: Vec<Vec<u8>>,
    /// DER `CertificateList` blobs (RFC 5280 §5).
    pub crls: Vec<Vec<u8>>,
}

impl RevocationData {
    /// Whether there is no material at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.ocsps.is_empty() && self.crls.is_empty()
    }
}

/// The revocation status of **one** certificate, as witnessed by one piece of material.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RevocationStatus {
    /// A verified OCSP response or CRL covers the certificate and does not revoke it.
    Good,
    /// A verified OCSP response or CRL revokes the certificate.
    Revoked,
    /// No usable (parseable, issuer-verified, in-window) material covers the certificate.
    Unknown,
}

/// The revocation outcome for a **whole chain** (PAdES-LT): what a validator can claim about the
/// signature given the embedded material.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RevocationSummary {
    /// Every non-anchor link is covered by verified material and none is revoked.
    Good,
    /// At least one link is revoked.
    Revoked,
    /// No link is revoked, but at least one has no usable material — the LT claim is incomplete.
    Incomplete,
}

/// Summarise the revocation status of a built chain — `(certificate, issuer)` pairs from leaf to
/// anchor, the anchor itself excluded (a trust anchor is axiomatic, not checked) — against `data`
/// at time `at_secs`.
#[must_use]
pub fn chain_revocation(
    chain: &[(Certificate, Certificate)],
    data: &RevocationData,
    at_secs: u64,
) -> RevocationSummary {
    let mut incomplete = false;
    for (cert, issuer) in chain {
        match cert_revocation(cert, issuer, data, at_secs) {
            RevocationStatus::Revoked => return RevocationSummary::Revoked,
            RevocationStatus::Unknown => incomplete = true,
            RevocationStatus::Good => {}
        }
    }
    if incomplete {
        RevocationSummary::Incomplete
    } else {
        RevocationSummary::Good
    }
}

/// The status of `cert` (issued by `issuer`) at `at_secs`: the first verified OCSP response that
/// covers it wins, then the first verified CRL; with no usable evidence, `Unknown`.
#[must_use]
pub fn cert_revocation(
    cert: &Certificate,
    issuer: &Certificate,
    data: &RevocationData,
    at_secs: u64,
) -> RevocationStatus {
    for blob in &data.ocsps {
        if let Some(status) = ocsp_status(blob, cert, issuer, at_secs) {
            return status;
        }
    }
    for blob in &data.crls {
        if let Some(status) = crl_status(blob, cert, issuer, at_secs) {
            return status;
        }
    }
    RevocationStatus::Unknown
}

/// What one OCSP response (RFC 6960) says about `cert` — `None` if the blob is unusable for this
/// certificate: unparseable, unsuccessful, signed by neither the issuer nor an issuer-certified
/// responder, no matching `CertID`, or out of its validity window.
fn ocsp_status(
    blob: &[u8],
    cert: &Certificate,
    issuer: &Certificate,
    at_secs: u64,
) -> Option<RevocationStatus> {
    let response = OcspResponse::from_der(blob).ok()?;
    if response.response_status != OcspResponseStatus::Successful {
        return None;
    }
    let bytes = response.response_bytes?;
    let basic = BasicOcspResponse::from_der(bytes.response.as_bytes()).ok()?;

    // The response must be signed by the certificate's issuer, or by a *delegated responder*
    // (RFC 6960 §4.2.2.2): a certificate the issuer signed that also carries the
    // `id-kp-OCSPSigning` extended key usage. Both halves are required. Without the EKU, any
    // certificate the CA has ever issued — a routine TLS certificate, say — could mint a "good"
    // answer for any certificate under that CA, which would let an attacker-supplied `/DSS` turn a
    // `Revoked` verdict into `Good`.
    let tbs = basic.tbs_response_data.to_der().ok()?;
    let sig = basic.signature.as_bytes()?;
    let alg = basic.signature_algorithm.oid;
    let signed_by_issuer = rsa_verifies(&tbs, &alg, sig, issuer);
    let signed_by_responder = basic.certs.as_deref().unwrap_or(&[]).iter().any(|resp| {
        certified_by(resp, issuer) && is_ocsp_signer(resp) && rsa_verifies(&tbs, &alg, sig, resp)
    });
    if !signed_by_issuer && !signed_by_responder {
        return None;
    }

    // Find the SingleResponse whose CertID matches (serial + issuer name/key hashes under the
    // CertID's own hash algorithm) and whose window contains `at_secs`.
    for single in &basic.tbs_response_data.responses {
        if single.cert_id.serial_number != cert.tbs_certificate.serial_number {
            continue;
        }
        let name_der = issuer.tbs_certificate.subject.to_der().ok()?;
        let key_bytes = issuer
            .tbs_certificate
            .subject_public_key_info
            .subject_public_key
            .raw_bytes();
        let alg = single.cert_id.hash_algorithm.oid;
        let name_hash = hash_with(&alg, &name_der)?;
        let key_hash = hash_with(&alg, key_bytes)?;
        if single.cert_id.issuer_name_hash.as_bytes() != name_hash.as_slice()
            || single.cert_id.issuer_key_hash.as_bytes() != key_hash.as_slice()
        {
            continue;
        }
        let this_update = single.this_update.0.to_unix_duration().as_secs();
        if at_secs < this_update {
            continue;
        }
        if let Some(next) = &single.next_update
            && at_secs > next.0.to_unix_duration().as_secs()
        {
            continue;
        }
        return Some(match single.cert_status {
            CertStatus::Good(_) => RevocationStatus::Good,
            CertStatus::Revoked(_) => RevocationStatus::Revoked,
            CertStatus::Unknown(_) => RevocationStatus::Unknown,
        });
    }
    None
}

/// What one CRL (RFC 5280 §5) says about `cert` — `None` if the blob is unusable: unparseable,
/// from a different issuer, not verifiably signed by the issuer, or out of its window.
fn crl_status(
    blob: &[u8],
    cert: &Certificate,
    issuer: &Certificate,
    at_secs: u64,
) -> Option<RevocationStatus> {
    let crl = CertificateList::from_der(blob).ok()?;
    if crl.tbs_cert_list.issuer != issuer.tbs_certificate.subject {
        return None;
    }
    let tbs = crl.tbs_cert_list.to_der().ok()?;
    let sig = crl.signature.as_bytes()?;
    if !rsa_verifies(&tbs, &crl.signature_algorithm.oid, sig, issuer) {
        return None;
    }
    let this_update = crl.tbs_cert_list.this_update.to_unix_duration().as_secs();
    if at_secs < this_update {
        return None;
    }
    if let Some(next) = &crl.tbs_cert_list.next_update
        && at_secs > next.to_unix_duration().as_secs()
    {
        return None;
    }
    let revoked = crl
        .tbs_cert_list
        .revoked_certificates
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .any(|entry| entry.serial_number == cert.tbs_certificate.serial_number);
    Some(if revoked {
        RevocationStatus::Revoked
    } else {
        RevocationStatus::Good
    })
}

/// Hash `data` with the algorithm `oid` (the ones OCSP `CertID`s use: SHA-1 or SHA-256).
fn hash_with(oid: &ObjectIdentifier, data: &[u8]) -> Option<Vec<u8>> {
    if *oid == rfc5912::ID_SHA_1 {
        Some(Sha1::digest(data).to_vec())
    } else if *oid == rfc5912::ID_SHA_256 {
        Some(Sha256::digest(data).to_vec())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;
    use std::time::Duration;

    use der::asn1::{BitString, GeneralizedTime, Null, OctetString};
    use rsa::pkcs1v15::SigningKey;
    use rsa::pkcs8::EncodePrivateKey;
    use rsa::signature::{SignatureEncoding, Signer};
    use rsa::{RsaPrivateKey, RsaPublicKey};
    use x509_cert::builder::{Builder, CertificateBuilder, Profile};
    use x509_cert::crl::{RevokedCert, TbsCertList};
    use x509_cert::name::Name;
    use x509_cert::serial_number::SerialNumber;
    use x509_cert::spki::{AlgorithmIdentifierOwned, EncodePublicKey, SubjectPublicKeyInfoOwned};
    use x509_cert::time::{Time, Validity};
    use x509_ocsp::{CertId, OcspGeneralizedTime, ResponderId, ResponseData, SingleResponse};

    use super::*;
    use crate::{SignOptions, VerifyOptions, sign_digest_with, verify_detached_with};

    /// The shared "now" for fixtures — the real clock, since the certificates are built with
    /// `Validity::from_now` and the chain check enforces the validity window at the signing time.
    #[allow(non_snake_case)]
    fn NOW() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    fn sha256_rsa_alg() -> AlgorithmIdentifierOwned {
        AlgorithmIdentifierOwned {
            oid: rfc5912::SHA_256_WITH_RSA_ENCRYPTION,
            parameters: None,
        }
    }

    fn gtime(secs: u64) -> GeneralizedTime {
        GeneralizedTime::from_unix_duration(Duration::from_secs(secs)).unwrap()
    }

    /// A CA (self-signed root) and a CA-issued leaf: `(ca_cert, ca_key, leaf_cert, leaf_key_pkcs8)`.
    fn ca_and_leaf() -> (Certificate, RsaPrivateKey, Certificate, Vec<u8>) {
        let mut rng = rand_core::OsRng;
        let ca_key = RsaPrivateKey::new(&mut rng, 2048).expect("rsa keygen");
        let ca_spki = SubjectPublicKeyInfoOwned::try_from(
            RsaPublicKey::from(&ca_key)
                .to_public_key_der()
                .unwrap()
                .as_bytes(),
        )
        .unwrap();
        let ca_name = Name::from_str("CN=Revocation Test CA").unwrap();
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
            Name::from_str("CN=Revocation Test Leaf").unwrap(),
            leaf_spki,
            &ca_signer,
        )
        .unwrap()
        .build()
        .unwrap();
        let leaf_key_der = leaf_key.to_pkcs8_der().unwrap().as_bytes().to_vec();
        (ca, ca_key, leaf, leaf_key_der)
    }

    /// A CRL issued and signed by `signer_key` under `issuer`'s name, listing `revoked` serials,
    /// valid `[this, next]`.
    fn make_crl(
        issuer: &Certificate,
        signer_key: &RsaPrivateKey,
        revoked: &[u32],
        this: u64,
        next: u64,
    ) -> Vec<u8> {
        let entries: Vec<RevokedCert> = revoked
            .iter()
            .map(|serial| RevokedCert {
                serial_number: SerialNumber::from(*serial),
                revocation_date: Time::GeneralTime(gtime(this)),
                crl_entry_extensions: None,
            })
            .collect();
        let tbs = TbsCertList {
            version: x509_cert::Version::V2,
            signature: sha256_rsa_alg(),
            issuer: issuer.tbs_certificate.subject.clone(),
            this_update: Time::GeneralTime(gtime(this)),
            next_update: Some(Time::GeneralTime(gtime(next))),
            revoked_certificates: (!entries.is_empty()).then_some(entries),
            crl_extensions: None,
        };
        let sig = SigningKey::<Sha256>::new(signer_key.clone()).sign(&tbs.to_der().unwrap());
        CertificateList {
            tbs_cert_list: tbs,
            signature_algorithm: sha256_rsa_alg(),
            signature: BitString::from_bytes(&sig.to_vec()).unwrap(),
        }
        .to_der()
        .unwrap()
    }

    /// An OCSP response for `subject` (issued by `issuer`), signed by `signer_key` under
    /// `issuer`'s name, with the given status, valid `[this, next]`.
    fn make_ocsp(
        issuer: &Certificate,
        signer_key: &RsaPrivateKey,
        subject: &Certificate,
        status: CertStatus,
        this: u64,
        next: u64,
    ) -> Vec<u8> {
        let name_der = issuer.tbs_certificate.subject.to_der().unwrap();
        let key_bytes = issuer
            .tbs_certificate
            .subject_public_key_info
            .subject_public_key
            .raw_bytes();
        let cert_id = CertId {
            hash_algorithm: AlgorithmIdentifierOwned {
                oid: rfc5912::ID_SHA_256,
                parameters: None,
            },
            issuer_name_hash: OctetString::new(Sha256::digest(&name_der).to_vec()).unwrap(),
            issuer_key_hash: OctetString::new(Sha256::digest(key_bytes).to_vec()).unwrap(),
            serial_number: subject.tbs_certificate.serial_number.clone(),
        };
        let tbs = ResponseData {
            version: Default::default(),
            responder_id: ResponderId::ByName(issuer.tbs_certificate.subject.clone()),
            produced_at: OcspGeneralizedTime::from(gtime(this)),
            responses: vec![SingleResponse {
                cert_id,
                cert_status: status,
                this_update: OcspGeneralizedTime::from(gtime(this)),
                next_update: Some(OcspGeneralizedTime::from(gtime(next))),
                single_extensions: None,
            }],
            response_extensions: None,
        };
        let sig = SigningKey::<Sha256>::new(signer_key.clone()).sign(&tbs.to_der().unwrap());
        let basic = BasicOcspResponse {
            tbs_response_data: tbs,
            signature_algorithm: sha256_rsa_alg(),
            signature: BitString::from_bytes(&sig.to_vec()).unwrap(),
            certs: None,
        };
        OcspResponse::successful(basic).unwrap().to_der().unwrap()
    }

    /// As [`make_ocsp`], but the response carries `responder_cert` in its `certs` field — the
    /// *delegated responder* shape of RFC 6960 §4.2.2.2.
    fn make_delegated_ocsp(
        issuer: &Certificate,
        responder_cert: &Certificate,
        responder_key: &RsaPrivateKey,
        subject: &Certificate,
        status: CertStatus,
        this: u64,
        next: u64,
    ) -> Vec<u8> {
        let der = make_ocsp(issuer, responder_key, subject, status, this, next);
        let mut response = OcspResponse::from_der(&der).unwrap();
        let bytes = response.response_bytes.as_ref().unwrap();
        let mut basic = BasicOcspResponse::from_der(bytes.response.as_bytes()).unwrap();
        basic.certs = Some(vec![responder_cert.clone()]);
        response = OcspResponse::successful(basic).unwrap();
        response.to_der().unwrap()
    }

    /// An end-entity certificate issued by `ca`, optionally carrying the `id-kp-OCSPSigning`
    /// extended key usage: `(certificate, key)`.
    fn ocsp_responder(
        ca: &Certificate,
        ca_key: &RsaPrivateKey,
        ocsp_signing: bool,
    ) -> (Certificate, RsaPrivateKey) {
        use x509_cert::ext::pkix::ExtendedKeyUsage;

        let mut rng = rand_core::OsRng;
        let key = RsaPrivateKey::new(&mut rng, 2048).expect("rsa keygen");
        let spki = SubjectPublicKeyInfoOwned::try_from(
            RsaPublicKey::from(&key)
                .to_public_key_der()
                .unwrap()
                .as_bytes(),
        )
        .unwrap();
        let signer = SigningKey::<Sha256>::new(ca_key.clone());
        let mut builder = CertificateBuilder::new(
            Profile::Leaf {
                issuer: ca.tbs_certificate.subject.clone(),
                enable_key_agreement: false,
                enable_key_encipherment: false,
            },
            SerialNumber::from(31u32),
            Validity::from_now(Duration::from_secs(3600)).unwrap(),
            Name::from_str("CN=Delegated Responder").unwrap(),
            spki,
            &signer,
        )
        .unwrap();
        if ocsp_signing {
            let oid = ObjectIdentifier::new_unwrap("1.3.6.1.5.5.7.3.9");
            builder.add_extension(&ExtendedKeyUsage(vec![oid])).unwrap();
        }
        (builder.build().unwrap(), key)
    }

    #[test]
    fn delegated_ocsp_responder_needs_the_ocsp_signing_eku() {
        // RFC 6960 §4.2.2.2. Without the EKU check, *any* certificate the CA has issued could
        // answer for any certificate under that CA — so an attacker holding a routine CA-issued
        // certificate could overwrite a `Revoked` verdict with `Good` via the document's /DSS.
        let (ca, ca_key, leaf, _) = ca_and_leaf();

        let (plain, plain_key) = ocsp_responder(&ca, &ca_key, false);
        let forged = RevocationData {
            ocsps: vec![make_delegated_ocsp(
                &ca,
                &plain,
                &plain_key,
                &leaf,
                CertStatus::Good(Null),
                NOW() - 100,
                NOW() + 100,
            )],
            ..Default::default()
        };
        assert_eq!(
            cert_revocation(&leaf, &ca, &forged, NOW()),
            RevocationStatus::Unknown,
            "a responder without id-kp-OCSPSigning is not evidence"
        );

        let (authorised, authorised_key) = ocsp_responder(&ca, &ca_key, true);
        let genuine = RevocationData {
            ocsps: vec![make_delegated_ocsp(
                &ca,
                &authorised,
                &authorised_key,
                &leaf,
                CertStatus::Good(Null),
                NOW() - 100,
                NOW() + 100,
            )],
            ..Default::default()
        };
        assert_eq!(
            cert_revocation(&leaf, &ca, &genuine, NOW()),
            RevocationStatus::Good,
            "a properly authorised delegated responder still counts"
        );

        // And it must still actually be revocable through that responder.
        let revoking = RevocationData {
            ocsps: vec![make_delegated_ocsp(
                &ca,
                &authorised,
                &authorised_key,
                &leaf,
                CertStatus::Revoked(x509_ocsp::RevokedInfo {
                    revocation_time: OcspGeneralizedTime::from(gtime(NOW() - 50)),
                    revocation_reason: None,
                }),
                NOW() - 100,
                NOW() + 100,
            )],
            ..Default::default()
        };
        assert_eq!(
            cert_revocation(&leaf, &ca, &revoking, NOW()),
            RevocationStatus::Revoked
        );
    }

    #[test]
    fn crl_states_good_revoked_and_unusable() {
        let (ca, ca_key, leaf, _) = ca_and_leaf();
        let clean = RevocationData {
            crls: vec![make_crl(&ca, &ca_key, &[], NOW() - 100, NOW() + 100)],
            ..Default::default()
        };
        assert_eq!(
            cert_revocation(&leaf, &ca, &clean, NOW()),
            RevocationStatus::Good
        );

        // The leaf's serial (7) on the list → revoked.
        let revoking = RevocationData {
            crls: vec![make_crl(&ca, &ca_key, &[7], NOW() - 100, NOW() + 100)],
            ..Default::default()
        };
        assert_eq!(
            cert_revocation(&leaf, &ca, &revoking, NOW()),
            RevocationStatus::Revoked
        );

        // A CRL signed by the wrong key is not evidence; neither is one outside its window,
        // one from a different issuer, or garbage bytes.
        let mut rng = rand_core::OsRng;
        let stranger = RsaPrivateKey::new(&mut rng, 2048).unwrap();
        for bad in [
            make_crl(&ca, &stranger, &[], NOW() - 100, NOW() + 100),
            make_crl(&ca, &ca_key, &[], NOW() + 50, NOW() + 100), // not yet valid
            make_crl(&ca, &ca_key, &[], NOW() - 100, NOW() - 50), // expired
            make_crl(&leaf, &ca_key, &[], NOW() - 100, NOW() + 100), // wrong issuer name
            b"not-a-crl".to_vec(),
        ] {
            let data = RevocationData {
                crls: vec![bad],
                ..Default::default()
            };
            assert_eq!(
                cert_revocation(&leaf, &ca, &data, NOW()),
                RevocationStatus::Unknown
            );
        }
    }

    #[test]
    fn ocsp_states_good_revoked_and_unusable() {
        let (ca, ca_key, leaf, _) = ca_and_leaf();
        let good = RevocationData {
            ocsps: vec![make_ocsp(
                &ca,
                &ca_key,
                &leaf,
                CertStatus::Good(Null),
                NOW() - 100,
                NOW() + 100,
            )],
            ..Default::default()
        };
        assert_eq!(
            cert_revocation(&leaf, &ca, &good, NOW()),
            RevocationStatus::Good
        );

        let revoked = RevocationData {
            ocsps: vec![make_ocsp(
                &ca,
                &ca_key,
                &leaf,
                CertStatus::Revoked(x509_ocsp::RevokedInfo {
                    revocation_time: OcspGeneralizedTime::from(gtime(NOW() - 50)),
                    revocation_reason: None,
                }),
                NOW() - 100,
                NOW() + 100,
            )],
            ..Default::default()
        };
        assert_eq!(
            cert_revocation(&leaf, &ca, &revoked, NOW()),
            RevocationStatus::Revoked
        );

        // Response about the CA itself (serial mismatch), badly-signed, or expired → not evidence.
        let mut rng = rand_core::OsRng;
        let stranger = RsaPrivateKey::new(&mut rng, 2048).unwrap();
        for bad in [
            make_ocsp(
                &ca,
                &ca_key,
                &ca,
                CertStatus::Good(Null),
                NOW() - 100,
                NOW() + 100,
            ),
            make_ocsp(
                &ca,
                &stranger,
                &leaf,
                CertStatus::Good(Null),
                NOW() - 100,
                NOW() + 100,
            ),
            make_ocsp(
                &ca,
                &ca_key,
                &leaf,
                CertStatus::Good(Null),
                NOW() - 100,
                NOW() - 50,
            ),
            b"not-an-ocsp".to_vec(),
        ] {
            let data = RevocationData {
                ocsps: vec![bad],
                ..Default::default()
            };
            assert_eq!(
                cert_revocation(&leaf, &ca, &data, NOW()),
                RevocationStatus::Unknown
            );
        }
    }

    #[test]
    fn chain_summary_good_revoked_incomplete() {
        let (ca, ca_key, leaf, _) = ca_and_leaf();
        let chain = vec![(leaf.clone(), ca.clone())];

        let good = RevocationData {
            crls: vec![make_crl(&ca, &ca_key, &[], NOW() - 100, NOW() + 100)],
            ..Default::default()
        };
        assert_eq!(
            chain_revocation(&chain, &good, NOW()),
            RevocationSummary::Good
        );
        assert_eq!(
            chain_revocation(&chain, &RevocationData::default(), NOW()),
            RevocationSummary::Incomplete
        );
        assert!(RevocationData::default().is_empty());
        let revoking = RevocationData {
            crls: vec![make_crl(&ca, &ca_key, &[7], NOW() - 100, NOW() + 100)],
            ..Default::default()
        };
        assert_eq!(
            chain_revocation(&chain, &revoking, NOW()),
            RevocationSummary::Revoked
        );
    }

    #[test]
    fn detached_signature_reports_chain_revocation() {
        // End to end: a CA-issued leaf signs; verification with the CA as anchor plus OCSP+CRL
        // material reports the PAdES-LT revocation summary alongside trust.
        let (ca, ca_key, leaf, leaf_key) = ca_and_leaf();
        let message = b"long-term validated content";
        let opts = SignOptions {
            signing_time: Some(NOW()),
            timestamp: None,
            pades: true,
        };
        let cms =
            sign_digest_with(message, &leaf.to_der().unwrap(), &leaf_key, &opts).expect("sign");

        let with = |data: Option<RevocationData>| VerifyOptions {
            roots: vec![ca.to_der().unwrap()],
            revocation: data,
        };
        let good = RevocationData {
            ocsps: vec![make_ocsp(
                &ca,
                &ca_key,
                &leaf,
                CertStatus::Good(Null),
                NOW() - 100,
                NOW() + 100,
            )],
            ..Default::default()
        };
        let verified = verify_detached_with(&cms, message, &with(Some(good)));
        assert!(verified.valid && verified.pades);
        assert_eq!(verified.trusted, Some(true));
        assert_eq!(verified.revocation, Some(RevocationSummary::Good));

        // Without material: revocation evaluated but incomplete. Without the option: None.
        let verified = verify_detached_with(&cms, message, &with(Some(RevocationData::default())));
        assert_eq!(verified.revocation, Some(RevocationSummary::Incomplete));
        let verified = verify_detached_with(&cms, message, &with(None));
        assert_eq!(verified.revocation, None);
    }
}
