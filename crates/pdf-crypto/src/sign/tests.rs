use super::*;
use rsa::RsaPublicKey;
use rsa::pkcs8::EncodePrivateKey;
use std::str::FromStr;
use std::time::Duration;
use x509_cert::builder::{Builder, CertificateBuilder, Profile};
use x509_cert::name::Name as X509Name;
use x509_cert::serial_number::SerialNumber;
use x509_cert::spki::{EncodePublicKey, SubjectPublicKeyInfoOwned};
use x509_cert::time::Validity;

/// A fresh RSA-2048 private key and its public `SubjectPublicKeyInfo`.
fn rsa_key() -> (RsaPrivateKey, SubjectPublicKeyInfoOwned) {
    let mut rng = rand_core::OsRng;
    let key = RsaPrivateKey::new(&mut rng, 2048).expect("rsa keygen");
    let spki = SubjectPublicKeyInfoOwned::try_from(
        RsaPublicKey::from(&key)
            .to_public_key_der()
            .unwrap()
            .as_bytes(),
    )
    .unwrap();
    (key, spki)
}

/// A throwaway RSA-2048 keypair + self-signed cert: (cert DER, PKCS#8 key DER).
fn keypair(cn: &str) -> (Vec<u8>, Vec<u8>) {
    let (key, spki) = rsa_key();
    let signer = SigningKey::<Sha256>::new(key.clone());
    let cert = CertificateBuilder::new(
        Profile::Root,
        SerialNumber::from(1u32),
        Validity::from_now(Duration::from_secs(3600)).unwrap(),
        X509Name::from_str(&format!("CN={cn}")).unwrap(),
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

/// A throwaway **Ed25519** keypair + self-signed cert: (cert DER, PKCS#8 key DER). The cert is
/// itself Ed25519-signed (via [`EdSigner`]), so it exercises the Ed25519 path end to end.
fn ed25519_keypair(cn: &str) -> (Vec<u8>, Vec<u8>) {
    let mut seed = [0u8; 32];
    rand_core::RngCore::fill_bytes(&mut rand_core::OsRng, &mut seed);
    let key = Ed25519SigningKey::from_bytes(&seed);
    let spki = SubjectPublicKeyInfoOwned::try_from(
        key.verifying_key().to_public_key_der().unwrap().as_bytes(),
    )
    .unwrap();
    let cert = CertificateBuilder::new(
        Profile::Root,
        SerialNumber::from(1u32),
        Validity::from_now(Duration::from_secs(3600)).unwrap(),
        X509Name::from_str(&format!("CN={cn}")).unwrap(),
        spki,
        &EdSigner(key.clone()),
    )
    .unwrap()
    .build()
    .unwrap();
    (
        cert.to_der().unwrap(),
        key.to_pkcs8_der().unwrap().as_bytes().to_vec(),
    )
}

#[test]
fn ed25519_sign_and_verify_round_trip() {
    let (cert, key) = ed25519_keypair("Ed25519 Signer");
    let message = b"ed25519 signed content";
    let opts = SignOptions {
        signing_time: Some(1_700_000_000),
        timestamp: None,
        pades: true,
    };
    let cms = sign_digest_with(message, &cert, &key, &opts).expect("ed25519 sign");
    let verified = verify_detached(&cms, message);
    assert!(verified.valid, "Ed25519 signature verifies");
    assert_eq!(verified.signing_time, Some(1_700_000_000));
    assert!(verified.pades, "PAdES cert binding over Ed25519");
    assert!(verified.signer.unwrap().contains("Ed25519 Signer"));

    // Tampering with the message breaks verification.
    assert!(!verify_detached(&cms, b"tampered content").valid);
}

#[test]
fn rsa_sha3_signature_verifies() {
    // ISO/TS 32001 §5.1: the SHA-3 family is valid in PDF 2.0 signatures. The `rsa` crate has
    // no SHA-3 signature-OID impl, so this local adapter supplies
    // id-rsassa-pkcs1-v1_5-with-sha3-256; the verify path must accept both the SHA-3
    // messageDigest attribute and the RSA-SHA3 signature algorithm.
    struct Sha3RsaSigner(SigningKey<sha3::Sha3_256>);
    impl Signer<Signature> for Sha3RsaSigner {
        fn try_sign(&self, msg: &[u8]) -> Result<Signature, rsa::signature::Error> {
            self.0.try_sign(msg)
        }
    }
    impl Keypair for Sha3RsaSigner {
        type VerifyingKey = VerifyingKey<sha3::Sha3_256>;
        fn verifying_key(&self) -> Self::VerifyingKey {
            self.0.verifying_key()
        }
    }
    impl DynSignatureAlgorithmIdentifier for Sha3RsaSigner {
        fn signature_algorithm_identifier(
            &self,
        ) -> Result<AlgorithmIdentifierOwned, x509_cert::spki::Error> {
            Ok(AlgorithmIdentifierOwned {
                oid: ID_RSA_SHA3_256,
                parameters: None,
            })
        }
    }

    let (cert_der, key_der) = keypair("RSA SHA-3 Signer");
    let cert = Certificate::from_der(&cert_der).unwrap();
    let rsa = RsaPrivateKey::from_pkcs8_der(&key_der).unwrap();
    let message = b"sha3 signed content";
    let cms = assemble_signed_data::<_, Signature>(
        &Sha3RsaSigner(SigningKey::new(rsa)),
        AlgorithmIdentifierOwned {
            oid: ID_SHA3_256,
            parameters: None,
        },
        sha3::Sha3_256::digest(message).as_slice(),
        &cert,
        &cert_der,
        Some(1_700_000_000),
        None,
        false,
    )
    .expect("sha3 CMS");
    let verified = verify_detached(&cms, message);
    assert!(verified.valid, "RSA-SHA3-256 signature verifies");
    assert_eq!(verified.signing_time, Some(1_700_000_000));
    assert!(!verify_detached(&cms, b"tampered").valid, "tamper detected");
}

/// Throwaway ECDSA keypairs + self-signed certs (cert DER, PKCS#8 key DER), one per curve,
/// self-signed via the matching ECDSA signer wrapper so the ECDSA path is exercised end to end.
fn p256_keypair(cn: &str) -> (Vec<u8>, Vec<u8>) {
    let key = p256::ecdsa::SigningKey::random(&mut rand_core::OsRng);
    let spki = SubjectPublicKeyInfoOwned::try_from(
        key.verifying_key().to_public_key_der().unwrap().as_bytes(),
    )
    .unwrap();
    let cert = CertificateBuilder::new(
        Profile::Root,
        SerialNumber::from(1u32),
        Validity::from_now(Duration::from_secs(3600)).unwrap(),
        X509Name::from_str(&format!("CN={cn}")).unwrap(),
        spki,
        &P256Signer(key.clone()),
    )
    .unwrap()
    .build()
    .unwrap();
    (
        cert.to_der().unwrap(),
        key.to_pkcs8_der().unwrap().as_bytes().to_vec(),
    )
}

fn p384_keypair(cn: &str) -> (Vec<u8>, Vec<u8>) {
    let key = p384::ecdsa::SigningKey::random(&mut rand_core::OsRng);
    let spki = SubjectPublicKeyInfoOwned::try_from(
        key.verifying_key().to_public_key_der().unwrap().as_bytes(),
    )
    .unwrap();
    let cert = CertificateBuilder::new(
        Profile::Root,
        SerialNumber::from(1u32),
        Validity::from_now(Duration::from_secs(3600)).unwrap(),
        X509Name::from_str(&format!("CN={cn}")).unwrap(),
        spki,
        &P384Signer(key.clone()),
    )
    .unwrap()
    .build()
    .unwrap();
    (
        cert.to_der().unwrap(),
        key.to_pkcs8_der().unwrap().as_bytes().to_vec(),
    )
}

fn p521_keypair(cn: &str) -> (Vec<u8>, Vec<u8>) {
    // The generic `ecdsa` key type (see P521Signer): pkcs8-encodable, SPKI-encodable.
    let key = ecdsa::SigningKey::<p521::NistP521>::random(&mut rand_core::OsRng);
    let spki = SubjectPublicKeyInfoOwned::try_from(
        key.verifying_key().to_public_key_der().unwrap().as_bytes(),
    )
    .unwrap();
    let cert = CertificateBuilder::new(
        Profile::Root,
        SerialNumber::from(1u32),
        Validity::from_now(Duration::from_secs(3600)).unwrap(),
        X509Name::from_str(&format!("CN={cn}")).unwrap(),
        spki,
        &P521Signer(key.clone()),
    )
    .unwrap()
    .build()
    .unwrap();
    (
        cert.to_der().unwrap(),
        key.to_pkcs8_der().unwrap().as_bytes().to_vec(),
    )
}

#[test]
fn ecdsa_sign_and_verify_round_trip() {
    for (label, (cert, key)) in [
        ("P-256", p256_keypair("ECDSA P-256 Signer")),
        ("P-384", p384_keypair("ECDSA P-384 Signer")),
        ("P-521", p521_keypair("ECDSA P-521 Signer")),
    ] {
        let message = b"ecdsa signed content";
        let opts = SignOptions {
            signing_time: Some(1_700_000_000),
            timestamp: None,
            pades: true,
        };
        let cms =
            sign_digest_with(message, &cert, &key, &opts).unwrap_or_else(|| panic!("{label} sign"));
        let verified = verify_detached(&cms, message);
        assert!(verified.valid, "{label} signature verifies");
        assert_eq!(verified.signing_time, Some(1_700_000_000));
        assert!(verified.pades, "{label} PAdES binding");
        // Tampering breaks it.
        assert!(!verify_detached(&cms, b"tampered").valid, "{label} tamper");
    }
}

/// A root CA certificate (DER) plus a leaf it issues: (ca DER, leaf DER, leaf key DER).
fn ca_and_leaf() -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let (ca_key, ca_spki) = rsa_key();
    let ca_name = X509Name::from_str("CN=Prism PDF Root CA").unwrap();
    let ca_signer = SigningKey::<Sha256>::new(ca_key);
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

    let (leaf_key, leaf_spki) = rsa_key();
    let leaf = CertificateBuilder::new(
        Profile::Leaf {
            issuer: ca_name,
            enable_key_agreement: false,
            enable_key_encipherment: false,
        },
        SerialNumber::from(2u32),
        Validity::from_now(Duration::from_secs(3600)).unwrap(),
        X509Name::from_str("CN=Prism PDF Leaf Signer").unwrap(),
        leaf_spki,
        &ca_signer, // signed by the CA's key
    )
    .unwrap()
    .build()
    .unwrap();

    (
        ca.to_der().unwrap(),
        leaf.to_der().unwrap(),
        leaf_key.to_pkcs8_der().unwrap().as_bytes().to_vec(),
    )
}

#[test]
fn sign_and_verify_round_trip() {
    let (cert, key) = keypair("Prism PDF Signer");
    let message = b"the exact bytes covered by /ByteRange";

    let cms = sign_digest(message, &cert, &key).expect("sign");
    let verified = verify_detached(&cms, message);
    assert!(
        verified.valid,
        "signature should verify over the signed bytes"
    );
    assert!(verified.signer.unwrap().contains("Prism PDF Signer"));

    // Any change to the covered bytes invalidates the signature (§12.8.1).
    assert!(!verify_detached(&cms, b"tampered bytes").valid);
}

#[test]
fn garbage_is_not_valid_and_never_panics() {
    assert!(!verify_detached(b"", b"x").valid);
    assert!(!verify_detached(&[0x30, 0x03, 0x02, 0x01, 0x00], b"x").valid);
    assert!(!verify_detached(b"not der at all", b"x").valid);
}

#[test]
fn signing_time_is_carried_and_read_back() {
    let (cert, key) = keypair("Timed Signer");
    let message = b"timed content";
    let opts = SignOptions {
        signing_time: Some(1_700_000_000),
        timestamp: None,
        pades: false,
    };
    let cms = sign_digest_with(message, &cert, &key, &opts).expect("sign");
    let verified = verify_detached(&cms, message);
    assert!(verified.valid);
    assert_eq!(verified.signing_time, Some(1_700_000_000));
}

#[test]
fn trust_store_validates_self_signed_anchor() {
    let (cert, key) = keypair("Trusted Root");
    let message = b"trusted content";
    let opts = SignOptions {
        signing_time: Some(now_secs()),
        timestamp: None,
        pades: false,
    };
    let cms = sign_digest_with(message, &cert, &key, &opts).expect("sign");

    // The signer cert is its own anchor → trusted.
    let trusting = VerifyOptions {
        roots: vec![cert.clone()],
        ..Default::default()
    };
    assert_eq!(
        verify_detached_with(&cms, message, &trusting).trusted,
        Some(true)
    );

    // A different root → not trusted; with no roots → not evaluated.
    let (other, _) = keypair("Other Root");
    let untrusting = VerifyOptions {
        roots: vec![other],
        ..Default::default()
    };
    assert_eq!(
        verify_detached_with(&cms, message, &untrusting).trusted,
        Some(false)
    );
    assert_eq!(verify_detached(&cms, message).trusted, None);
}

#[test]
fn chain_to_ca_root_is_trusted_but_not_to_a_stranger() {
    let (ca_der, leaf_der, leaf_key) = ca_and_leaf();
    let message = b"chained content";
    let opts = SignOptions {
        signing_time: Some(now_secs()),
        timestamp: None,
        pades: false,
    };
    // The signer is the CA-issued leaf (not self-signed); the CA is the only trust anchor.
    let cms = sign_digest_with(message, &leaf_der, &leaf_key, &opts).expect("sign");

    let trusting = VerifyOptions {
        roots: vec![ca_der],
        ..Default::default()
    };
    assert_eq!(
        verify_detached_with(&cms, message, &trusting).trusted,
        Some(true),
        "leaf chains to its issuing CA"
    );

    let (stranger, _) = keypair("Unrelated Root");
    let untrusting = VerifyOptions {
        roots: vec![stranger],
        ..Default::default()
    };
    assert_eq!(
        verify_detached_with(&cms, message, &untrusting).trusted,
        Some(false),
        "leaf does not chain to an unrelated root"
    );
}

#[test]
fn embedded_timestamp_verifies() {
    let (cert, key) = keypair("Stamped Signer");
    let (tsa_cert, tsa_key) = keypair("Prism PDF TSA");
    let message = b"stamped content";
    let opts = SignOptions {
        signing_time: Some(1_700_000_000),
        timestamp: Some(TsaCredentials {
            cert_der: tsa_cert,
            key_der: tsa_key,
            gen_time: 1_700_000_123,
            serial: 42,
        }),
        pades: false,
    };
    let cms = sign_digest_with(message, &cert, &key, &opts).expect("sign");
    let verified = verify_detached(&cms, message);
    assert!(verified.valid, "signature still verifies with a timestamp");
    assert_eq!(verified.timestamp_time, Some(1_700_000_123));

    // No timestamp requested → none reported.
    let plain = sign_digest(message, &cert, &key).expect("sign");
    assert_eq!(verify_detached(&plain, message).timestamp_time, None);
}

// --- RFC 5280 §4.2.1.9 basic constraints -------------------------------------------------------

/// Issue an end-entity certificate for `cn` under `issuer_cert`/`issuer_key`: `basicConstraints`
/// says `cA=FALSE` and `keyUsage` withholds `keyCertSign`, so it must never act as an issuer.
fn issue_end_entity(
    cn: &str,
    issuer_cert: &Certificate,
    issuer_key: &RsaPrivateKey,
) -> (Vec<u8>, RsaPrivateKey) {
    let (key, spki) = rsa_key();
    let signer = SigningKey::<Sha256>::new(issuer_key.clone());
    let cert = CertificateBuilder::new(
        Profile::Leaf {
            issuer: issuer_cert.tbs_certificate.subject.clone(),
            enable_key_agreement: false,
            enable_key_encipherment: false,
        },
        SerialNumber::from(99u32),
        Validity::from_now(Duration::from_secs(3600)).unwrap(),
        X509Name::from_str(&format!("CN={cn}")).unwrap(),
        spki,
        &signer,
    )
    .unwrap()
    .build()
    .unwrap();
    (cert.to_der().unwrap(), key)
}

#[test]
fn an_end_entity_certificate_cannot_act_as_an_issuer() {
    // The basic-constraints bypass (CVE-2002-0862 class). Anyone can obtain an ordinary
    // certificate from a public CA; if path building only checked names and signatures, the holder
    // of that key could mint a subordinate for *any* subject and have it chain to the trusted root.
    let (root_der, root_key_der) = keypair("Constraint Test Root");
    let root = Certificate::from_der(&root_der).unwrap();
    let root_key = RsaPrivateKey::from_pkcs8_der(&root_key_der).unwrap();

    let (ee_der, ee_key) = issue_end_entity("Ordinary Leaf", &root, &root_key);
    let ee = Certificate::from_der(&ee_der).unwrap();
    assert!(
        !super::verification::is_ca(&ee),
        "an end-entity certificate is not a CA"
    );

    // The attacker holds the end-entity key and signs a certificate naming someone else.
    let (rogue_der, _) = issue_end_entity("CFO of ACME Corp", &ee, &ee_key);
    let rogue = Certificate::from_der(&rogue_der).unwrap();

    assert!(
        super::verification::build_chain(&rogue, &[&ee], std::slice::from_ref(&root), now_secs())
            .is_none(),
        "a certificate signed by a non-CA must not chain to the trust anchor"
    );

    // The legitimate path through the same anchor still works, so the check is not simply
    // rejecting everything.
    assert!(
        super::verification::build_chain(&ee, &[], &[root], now_secs()).is_some(),
        "the CA-issued end-entity certificate itself still chains"
    );
}

#[test]
fn a_trust_anchor_is_accepted_by_der_identity_not_by_being_a_ca() {
    // Anchors are matched byte-for-byte before any CA question is asked, so a self-signed
    // certificate a caller has explicitly trusted keeps working regardless of its extensions.
    let (root_der, root_key_der) = keypair("Self Signed Anchor");
    let root = Certificate::from_der(&root_der).unwrap();
    let root_key = RsaPrivateKey::from_pkcs8_der(&root_key_der).unwrap();
    let (leaf_der, _) = issue_end_entity("Anchored Leaf", &root, &root_key);
    let leaf = Certificate::from_der(&leaf_der).unwrap();

    assert!(super::verification::build_chain(&leaf, &[], &[root], now_secs()).is_some());
}

// --- RFC 5652 §5.3 signer identification, §5.5 algorithm agility ------------------------------

#[test]
fn the_signer_certificate_is_found_by_sid_not_by_position() {
    // `certificates` is a SET OF, so its order carries no meaning. Verification must locate the
    // signer through `SignerInfo.sid`; assuming `certificates[0]` breaks a valid CMS whose set
    // happens to be ordered differently, and can report a subject that did not sign.
    let (cert, key) = keypair("Positional Signer");
    let message = b"order-independent content";
    let cms = sign_digest(message, &cert, &key).expect("sign");

    let content_info = ContentInfo::from_der(&cms).unwrap();
    let mut signed_data = content_info.content.decode_as::<SignedData>().unwrap();

    // Prepend an unrelated certificate so the signer is no longer first.
    let (decoy_der, _) = keypair("Unrelated Decoy");
    let decoy = Certificate::from_der(&decoy_der).unwrap();
    let mut certs: Vec<CertificateChoices> = signed_data
        .certificates
        .as_ref()
        .unwrap()
        .0
        .as_slice()
        .to_vec();
    certs.insert(0, CertificateChoices::Certificate(decoy));
    signed_data.certificates = Some(cms::signed_data::CertificateSet(
        SetOfVec::from_iter(certs).unwrap(),
    ));

    let reordered = ContentInfo {
        content_type: content_info.content_type,
        content: Any::encode_from(&signed_data).unwrap(),
    }
    .to_der()
    .unwrap();

    let verified = verify_detached(&reordered, message);
    assert!(
        verified.valid,
        "the signature is still valid when reordered"
    );
    assert!(
        verified
            .signer
            .as_deref()
            .unwrap()
            .contains("Positional Signer"),
        "reported signer should be the one named by sid, got {:?}",
        verified.signer
    );
}

#[test]
fn an_unknown_signature_algorithm_is_refused() {
    // A catch-all that verified every unrecognised OID as RSA-PKCS#1-v1.5/SHA-256 meant the
    // declared `signatureAlgorithm` was ignored entirely. An algorithm we do not implement must
    // come back invalid, not silently reinterpreted.
    let (cert, key) = keypair("Relabelled Signer");
    let message = b"relabelled content";
    let cms = sign_digest(message, &cert, &key).expect("sign");
    assert!(verify_detached(&cms, message).valid);

    let content_info = ContentInfo::from_der(&cms).unwrap();
    let mut signed_data = content_info.content.decode_as::<SignedData>().unwrap();
    let mut infos = signed_data.signer_infos.0.as_slice().to_vec();
    // Relabel as RSASSA-PSS, which this crate does not verify.
    infos[0].signature_algorithm = AlgorithmIdentifierOwned {
        oid: ObjectIdentifier::new_unwrap("1.2.840.113549.1.1.10"),
        parameters: None,
    };
    signed_data.signer_infos = cms::signed_data::SignerInfos(SetOfVec::from_iter(infos).unwrap());

    let relabelled = ContentInfo {
        content_type: content_info.content_type,
        content: Any::encode_from(&signed_data).unwrap(),
    }
    .to_der()
    .unwrap();

    assert!(
        !verify_detached(&relabelled, message).valid,
        "a signature under an unimplemented algorithm must not verify"
    );
}

#[test]
fn signer_count_is_reported() {
    let (cert, key) = keypair("Sole Signer");
    let message = b"one signer";
    let cms = sign_digest(message, &cert, &key).expect("sign");
    assert_eq!(verify_detached(&cms, message).signer_count, 1);
}

#[test]
fn a_verified_timestamp_outranks_the_signers_own_claimed_time() {
    // `signingTime` is signed by the signer — i.e. by the party whose certificate is in question —
    // so it must not anchor certificate-validity checking. A verified RFC 3161 `genTime` is
    // asserted by a third party and does.
    use super::verification::validation_instant;

    assert_eq!(validation_instant(Some(1_000), Some(9_999)), 1_000);
    assert_eq!(validation_instant(None, Some(9_999)), 9_999);
    // Neither available → wall clock, which is well past these fixtures.
    assert!(validation_instant(None, None) > 9_999);
}
