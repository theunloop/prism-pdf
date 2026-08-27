//! Signing an **encrypted** document (§12.8 over §7.6).
//!
//! An incremental update is part of the same file, so two ISO rules bind it:
//!
//! - §7.5.6 — "The added trailer shall contain all the entries except the `Prev` entry (if
//!   present) from the previous trailer, whether modified or not." A reader treats the newest
//!   trailer as authoritative, so a revision that drops `/Encrypt` leaves a file declaring itself
//!   unencrypted while its body objects are still ciphertext.
//! - §7.6.2 — encryption "applies to all strings and streams in the document's PDF file", with
//!   four exceptions: the trailer `/ID`, strings in the `/Encrypt` dictionary, strings already
//!   inside an encrypted stream, and the signature `/Contents` hex string.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::str::FromStr;
use std::time::Duration;

use der::Encode;
use pdf_cos::{Name as CosName, Object, ObjectId};
use pdf_document::{Algorithm, Document, SignSettings, SignatureAppearance};
use rsa::pkcs1v15::SigningKey;
use rsa::pkcs8::EncodePrivateKey;
use rsa::{RsaPrivateKey, RsaPublicKey};
use sha2::Sha256;
use x509_cert::builder::{Builder, CertificateBuilder, Profile};
use x509_cert::name::Name;
use x509_cert::serial_number::SerialNumber;
use x509_cert::spki::{EncodePublicKey, SubjectPublicKeyInfoOwned};
use x509_cert::time::Validity;

const SECRET: &str = "PAYROLL TOTAL";
const REASON: &str = "QUARTERLY APPROVAL";

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

fn one_page_pdf() -> Vec<u8> {
    let objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R >>".to_vec(),
        format!("<< /Length 44 >>\nstream\nBT /F1 12 Tf 20 100 Td ({SECRET}) Tj ET\nendstream")
            .into_bytes(),
    ];
    let mut out: Vec<u8> = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    }
    let startxref = out.len();
    out.extend_from_slice(b"xref\n0 5\n0000000000 65535 f \n");
    for offset in &offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!("trailer\n<< /Size 5 /Root 1 0 R >>\nstartxref\n{startxref}\n%%EOF\n").as_bytes(),
    );
    out
}

/// The signature dictionary and the signature field of a signed document.
fn signature_parts(doc: &Document) -> (Option<pdf_cos::Dictionary>, Option<pdf_cos::Dictionary>) {
    let (mut sig, mut field) = (None, None);
    for number in 1..80u32 {
        let Ok(Object::Dictionary(dict)) = doc.get(ObjectId::new(number, 0)) else {
            continue;
        };
        if dict.get(&CosName::from("ByteRange")).is_some()
            && dict.get(&CosName::from("Contents")).is_some()
        {
            sig = Some(dict);
        } else if dict.get(&CosName::from("FT")).is_some() {
            field = Some(dict);
        }
    }
    (sig, field)
}

fn text_entry(dict: &Option<pdf_cos::Dictionary>, key: &str) -> Option<String> {
    match dict.as_ref()?.get(&CosName::from(key))? {
        Object::String(s) => Some(String::from_utf8_lossy(s.as_bytes()).into_owned()),
        _ => None,
    }
}

/// Sign an `algorithm`-encrypted document and assert every ISO rule the revision has to honour.
fn check(algorithm: Algorithm) {
    let (cert, key) = self_signed("Encrypted Signer");
    let settings = SignSettings {
        reason: Some(REASON.into()),
        name: Some("Alice Auditor".into()),
        appearance: Some(SignatureAppearance {
            page_index: 0,
            rect: [10.0, 10.0, 190.0, 60.0],
            text: None,
        }),
        ..Default::default()
    };

    let encrypted = Document::open(one_page_pdf())
        .unwrap()
        .save_encrypted(b"", b"owner", algorithm)
        .unwrap();
    let signed = Document::open_with_password(encrypted, b"")
        .unwrap()
        .sign_with(&cert, &key, &settings)
        .unwrap();

    // §7.6.2: the signature metadata is a string in an encrypted file, so it must not be legible
    // in the raw bytes. An encrypted document should not leak who signed it and why.
    assert!(
        !signed.windows(REASON.len()).any(|w| w == REASON.as_bytes()),
        "{algorithm:?}: /Reason was written in the clear"
    );

    let doc = Document::open_with_password(signed, b"").unwrap();

    // §7.5.6: /Encrypt survived into the added trailer, so the body still decrypts.
    let pages = doc.pages().unwrap();
    let content = doc.page_content_bytes(&pages[0]).unwrap();
    assert!(
        String::from_utf8_lossy(&content).contains(SECRET),
        "{algorithm:?}: page content did not survive signing"
    );

    // §7.6.2 again, from the other side: what was encrypted reads back intact.
    let (sig, field) = signature_parts(&doc);
    assert_eq!(
        text_entry(&sig, "Reason").as_deref(),
        Some(REASON),
        "{algorithm:?}: /Reason did not round-trip"
    );
    assert_eq!(
        text_entry(&sig, "Name").as_deref(),
        Some("Alice Auditor"),
        "{algorithm:?}: /Name did not round-trip"
    );
    assert!(
        text_entry(&sig, "M").is_some_and(|m| m.starts_with("D:")),
        "{algorithm:?}: /M did not round-trip"
    );
    assert_eq!(
        text_entry(&field, "T").as_deref(),
        Some("Signature1"),
        "{algorithm:?}: the field's /T did not round-trip"
    );

    // The appearance stream is a stream, so it is encrypted too — and must decode back to
    // operators, not ciphertext.
    let appearance = field
        .as_ref()
        .and_then(|f| f.get(&CosName::from("AP")).cloned())
        .and_then(|ap| doc.resolve(&ap).ok())
        .and_then(|ap| {
            ap.as_dict()
                .and_then(|d| d.get(&CosName::from("N")).cloned())
        })
        .and_then(|n| doc.resolve(&n).ok())
        .expect("appearance stream");
    let stream = appearance.as_stream().expect("N is a stream");
    let decoded = doc.decode_stream(stream).expect("appearance decodes");
    assert!(
        String::from_utf8_lossy(&decoded).contains("BT"),
        "{algorithm:?}: appearance stream did not survive encryption"
    );

    // And the signature still verifies over the bytes as written.
    let statuses = doc.verify_signatures().unwrap();
    assert_eq!(statuses.len(), 1, "{algorithm:?}: expected one signature");
    assert!(statuses[0].valid, "{algorithm:?}: signature did not verify");
}

#[test]
fn signing_preserves_an_encrypted_document() {
    for algorithm in [
        Algorithm::Rc4,
        Algorithm::Aes128,
        Algorithm::Aes256,
        Algorithm::Aes256Gcm,
    ] {
        check(algorithm);
    }
}

#[test]
fn timestamping_preserves_an_encrypted_document() {
    let (cert, key) = self_signed("Encrypted TSA");
    let encrypted = Document::open(one_page_pdf())
        .unwrap()
        .save_encrypted(b"", b"owner", Algorithm::Aes256)
        .unwrap();
    let stamped = Document::open_with_password(encrypted, b"")
        .unwrap()
        .timestamp(&cert, &key, Some(1_700_000_000))
        .unwrap();

    let doc = Document::open_with_password(stamped, b"").unwrap();
    let pages = doc.pages().unwrap();
    let content = doc.page_content_bytes(&pages[0]).unwrap();
    assert!(String::from_utf8_lossy(&content).contains(SECRET));
    assert!(doc.verify_signatures().unwrap()[0].valid);
}

#[test]
fn signing_an_unencrypted_document_is_unchanged() {
    // The encryption-aware path must not disturb the ordinary case.
    let (cert, key) = self_signed("Plain Signer");
    let settings = SignSettings {
        reason: Some(REASON.into()),
        ..Default::default()
    };
    let signed = Document::open(one_page_pdf())
        .unwrap()
        .sign_with(&cert, &key, &settings)
        .unwrap();
    let doc = Document::open(signed).unwrap();
    let (sig, _) = signature_parts(&doc);
    assert_eq!(text_entry(&sig, "Reason").as_deref(), Some(REASON));
    assert!(doc.verify_signatures().unwrap()[0].valid);
}
