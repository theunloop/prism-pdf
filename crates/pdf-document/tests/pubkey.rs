//! Public-key security handler round-trip (ISO 32000-1 §7.6.5), end to end through the document
//! layer: save under `/Adobe.PPKLite`, then reopen with the recipient's certificate + private key.
//! The throwaway RSA keypair and self-signed certificate are generated here (RustCrypto), so the
//! test needs no baked-in key material.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use der::Encode;
use pdf_document::{Algorithm, DocError, Document};
use rsa::pkcs1v15::SigningKey;
use rsa::pkcs8::EncodePrivateKey;
use rsa::{RsaPrivateKey, RsaPublicKey};
use sha2::Sha256;
use std::str::FromStr;
use std::time::Duration;
use x509_cert::builder::{Builder, CertificateBuilder, Profile};
use x509_cert::name::Name;
use x509_cert::serial_number::SerialNumber;
use x509_cert::spki::{EncodePublicKey, SubjectPublicKeyInfoOwned};
use x509_cert::time::Validity;

/// Generate an RSA-2048 keypair + self-signed cert; return (cert DER, PKCS#8 key DER).
fn self_signed_recipient(serial: u32) -> (Vec<u8>, Vec<u8>) {
    let mut rng = rand_core::OsRng;
    let private_key = RsaPrivateKey::new(&mut rng, 2048).expect("rsa keygen");
    let public_key = RsaPublicKey::from(&private_key);
    let spki =
        SubjectPublicKeyInfoOwned::try_from(public_key.to_public_key_der().unwrap().as_bytes())
            .unwrap();
    let subject = Name::from_str("CN=Prism PDF Test Recipient").unwrap();
    let signer = SigningKey::<Sha256>::new(private_key.clone());
    let cert = CertificateBuilder::new(
        Profile::Root,
        SerialNumber::from(serial),
        Validity::from_now(Duration::from_secs(3600)).unwrap(),
        subject,
        spki,
        &signer,
    )
    .unwrap()
    .build()
    .unwrap();
    (
        cert.to_der().unwrap(),
        private_key.to_pkcs8_der().unwrap().as_bytes().to_vec(),
    )
}

/// Assemble a minimal classic-xref PDF from object bodies (object `i+1` ← `objects[i]`).
fn assemble(objects: &[Vec<u8>], trailer_extra: &str) -> Vec<u8> {
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
        format!(
            "trailer\n<< /Size {} /Root 1 0 R {trailer_extra} >>\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    buf.extend_from_slice(format!("startxref\n{startxref}\n%%EOF\n").as_bytes());
    buf
}

fn sample_document() -> Document {
    let objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>".to_vec(),
        b"<< /Length 24 >>\nstream\nBT (Top secret) Tj ET\nendstream".to_vec(),
    ];
    Document::open(assemble(&objects, "")).unwrap()
}

fn round_trip(algorithm: Algorithm) {
    let (cert, key) = self_signed_recipient(7);
    let doc = sample_document();
    let encrypted = doc.save_encrypted_public_key(&[&cert], algorithm).unwrap();

    // Genuinely encrypted, and the public-key handler name is present.
    assert!(
        encrypted
            .windows(b"Top secret".len())
            .all(|w| w != b"Top secret"),
        "plaintext leaked"
    );
    assert!(encrypted.windows(13).any(|w| w == b"Adobe.PPKLite"));

    // The recipient's certificate + private key reopen it and recover the content stream.
    let reopened = Document::open_with_private_key(encrypted.clone(), &cert, &key).unwrap();
    let page = reopened.pages().unwrap().remove(0);
    assert_eq!(
        reopened.page_content_bytes(&page).unwrap(),
        b"BT (Top secret) Tj ET"
    );

    // A different recipient cannot open it.
    let (other_cert, other_key) = self_signed_recipient(8);
    assert_eq!(
        Document::open_with_private_key(encrypted, &other_cert, &other_key).unwrap_err(),
        DocError::NeedsPassword
    );
}

#[test]
fn public_key_aes128_round_trips() {
    round_trip(Algorithm::Aes128);
}

#[test]
fn public_key_aes256_round_trips() {
    round_trip(Algorithm::Aes256);
}

#[test]
fn multiple_recipients_each_open() {
    let (cert_a, key_a) = self_signed_recipient(10);
    let (cert_b, key_b) = self_signed_recipient(11);
    let doc = sample_document();
    let encrypted = doc
        .save_encrypted_public_key(&[&cert_a, &cert_b], Algorithm::Aes256)
        .unwrap();

    for (cert, key) in [(&cert_a, &key_a), (&cert_b, &key_b)] {
        let reopened = Document::open_with_private_key(encrypted.clone(), cert, key).unwrap();
        let page = reopened.pages().unwrap().remove(0);
        assert_eq!(
            reopened.page_content_bytes(&page).unwrap(),
            b"BT (Top secret) Tj ET"
        );
    }
}

#[test]
fn public_key_with_restricted_permissions_round_trips() {
    use pdf_document::Permissions;
    let (cert, key) = self_signed_recipient(20);
    let doc = sample_document();
    let perms = Permissions::RESTRICTED.allow_print().allow_copy();
    let encrypted = doc
        .save_encrypted_public_key_with(&[&cert], perms, false, Algorithm::Aes256)
        .unwrap();
    let reopened = Document::open_with_private_key(encrypted, &cert, &key).unwrap();
    let page = reopened.pages().unwrap().remove(0);
    assert_eq!(
        reopened.page_content_bytes(&page).unwrap(),
        b"BT (Top secret) Tj ET"
    );
}

#[test]
fn rc4_public_key_write_is_rejected() {
    let (cert, _key) = self_signed_recipient(1);
    let doc = sample_document();
    assert_eq!(
        doc.save_encrypted_public_key(&[&cert], Algorithm::Rc4)
            .unwrap_err(),
        DocError::BadRecipientCert
    );
}
