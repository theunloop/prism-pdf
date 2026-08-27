//! Encryption save/open round-trip tests for the standard security handler (§7.6).

use super::super::*;
use super::{assemble, from_base64};
use pdf_cos::PdfString;

fn roundtrips_under(algorithm: Algorithm) {
    let objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>".to_vec(),
        b"<< /Length 24 >>\nstream\nBT (Top secret) Tj ET\nendstream".to_vec(),
        b"<< /Title (Classified) >>".to_vec(),
    ];
    let doc = Document::open(assemble(&objects, "/Info 5 0 R")).unwrap();
    let encrypted = doc.save_encrypted(b"", b"", algorithm).unwrap();

    // The saved file is genuinely encrypted: the plaintext does not appear in the raw bytes,
    // and an /Encrypt entry is present.
    assert!(
        encrypted
            .windows(b"Top secret".len())
            .all(|w| w != b"Top secret")
    );
    assert!(encrypted.windows(8).any(|w| w == b"/Encrypt"));

    // Reopening transparently decrypts (empty user password): content and /Title come back.
    let reopened = Document::open(encrypted).unwrap();
    let page = reopened.pages().unwrap().remove(0);
    assert_eq!(
        reopened.page_content_bytes(&page).unwrap(),
        b"BT (Top secret) Tj ET"
    );
    let title = reopened.info().unwrap().unwrap();
    assert_eq!(
        title.get(&Name::from("Title")),
        Some(&Object::String(PdfString::from(b"Classified".to_vec())))
    );
}

#[test]
fn save_encrypted_round_trips_rc4() {
    roundtrips_under(Algorithm::Rc4);
}

#[test]
fn save_encrypted_round_trips_aes128() {
    roundtrips_under(Algorithm::Aes128);
}

#[test]
fn save_encrypted_round_trips_aes256() {
    roundtrips_under(Algorithm::Aes256);
}

#[test]
fn save_encrypted_round_trips_aes256_gcm() {
    roundtrips_under(Algorithm::Aes256Gcm);
}

#[test]
fn aes256_gcm_emits_aesv4_crypt_filter_and_stamps_2_0() {
    let objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R >>".to_vec(),
    ];
    let doc = Document::open(assemble(&objects, "")).unwrap();
    let encrypted = doc.save_encrypted(b"", b"", Algorithm::Aes256Gcm).unwrap();
    // The crypt-filter method is AESV4 (ISO/TS 32003); AES-256 sits at the PDF 2.0 floor.
    let has_aesv4 = encrypted.windows(5).any(|w| w == b"AESV4");
    assert!(has_aesv4, "AESV4 crypt-filter method present");
    assert!(encrypted.starts_with(b"%PDF-2.0"), "AES-256 → %PDF-2.0");
}

#[test]
fn save_encrypted_with_permissions_and_metadata_flag() {
    // Restricted permissions (print only) + cleartext metadata must survive the round-trip and
    // appear in the saved /Encrypt, while content still decrypts.
    let objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>".to_vec(),
        b"<< /Length 17 >>\nstream\nBT (locked) Tj ET\nendstream".to_vec(),
    ];
    let doc = Document::open(assemble(&objects, "")).unwrap();
    let perms = Permissions::RESTRICTED.allow_print();

    for algorithm in [Algorithm::Aes128, Algorithm::Aes256] {
        let encrypted = doc
            .save_encrypted_with(b"", b"", perms, false, algorithm)
            .unwrap();
        // /P and /EncryptMetadata false are present in the output.
        assert!(
            encrypted
                .windows(b"/EncryptMetadata".len())
                .any(|w| w == b"/EncryptMetadata")
        );
        let reopened = Document::open(encrypted).unwrap();
        let p = reopened
            .resolve(reopened.xref.trailer.get(&Name::from("Encrypt")).unwrap())
            .unwrap();
        let Object::Dictionary(enc) = p else {
            panic!("no /Encrypt dict")
        };
        assert_eq!(enc.get_integer(&Name::from("P")), Some(perms.bits() as i64));
        assert_eq!(
            enc.get(&Name::from("EncryptMetadata"))
                .and_then(Object::as_bool),
            Some(false)
        );
        // Content still decrypts under the (empty) user password.
        let page = reopened.pages().unwrap().remove(0);
        assert_eq!(
            reopened.page_content_bytes(&page).unwrap(),
            b"BT (locked) Tj ET"
        );
    }
}

#[test]
fn aes256_open_tries_user_owner_and_rejects_wrong() {
    let objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>".to_vec(),
        b"<< /Length 17 >>\nstream\nBT (locked) Tj ET\nendstream".to_vec(),
    ];
    let doc = Document::open(assemble(&objects, "")).unwrap();
    let encrypted = doc
        .save_encrypted(b"user-pw", b"owner-pw", Algorithm::Aes256)
        .unwrap();

    assert_eq!(
        Document::open_with_password(encrypted.clone(), b"bad").unwrap_err(),
        DocError::NeedsPassword
    );
    for pw in [b"user-pw".as_slice(), b"owner-pw".as_slice()] {
        let doc = Document::open_with_password(encrypted.clone(), pw).unwrap();
        let page = doc.pages().unwrap().remove(0);
        assert_eq!(doc.page_content_bytes(&page).unwrap(), b"BT (locked) Tj ET");
    }
}

#[test]
fn password_protected_open_tries_user_owner_and_rejects_wrong() {
    let objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>".to_vec(),
        b"<< /Length 17 >>\nstream\nBT (locked) Tj ET\nendstream".to_vec(),
    ];
    let doc = Document::open(assemble(&objects, "")).unwrap();
    // Distinct user and owner passwords (AES-128 / V4).
    let encrypted = doc
        .save_encrypted(b"user-pw", b"owner-pw", Algorithm::Aes128)
        .unwrap();

    // No password (empty) is wrong → a clear error rather than garbage.
    assert_eq!(
        Document::open(encrypted.clone()).unwrap_err(),
        DocError::NeedsPassword
    );
    assert_eq!(
        Document::open_with_password(encrypted.clone(), b"bad").unwrap_err(),
        DocError::NeedsPassword
    );

    // Both the user and the owner password decrypt the content.
    for pw in [b"user-pw".as_slice(), b"owner-pw".as_slice()] {
        let doc = Document::open_with_password(encrypted.clone(), pw).unwrap();
        let page = doc.pages().unwrap().remove(0);
        assert_eq!(doc.page_content_bytes(&page).unwrap(), b"BT (locked) Tj ET");
    }
}

/// An RC4 (V2/R3, 128-bit) encrypted one-page PDF (empty user password) generated by the
/// reference encryptor: object 4's content shows "Secret Text", object 5's /Title is
/// "Confidential". Base64-encoded so the test is self-contained.
const ENCRYPTED_PDF_BASE64: &str = "JVBERi0xLjYKMSAwIG9iago8PCAvVHlwZSAvQ2F0YWxvZyAvUGFnZXMgMiAwIFIgPj4KZW5kb2JqCjIgMCBvYmoKPDwgL1R5cGUgL1BhZ2VzIC9LaWRzIFszIDAgUl0gL0NvdW50IDEgPj4KZW5kb2JqCjMgMCBvYmoKPDwgL1R5cGUgL1BhZ2UgL1BhcmVudCAyIDAgUiAvTWVkaWFCb3ggWzAgMCA2MTIgNzkyXSAvQ29udGVudHMgNCAwIFIgPj4KZW5kb2JqCjQgMCBvYmoKPDwgL0xlbmd0aCA0MiA+PgpzdHJlYW0KnfS8P1qpSNCWJq+nGRTfc/NRw9zcElaS7/a4YCHT5wJXHxcjShpSVxKlCmVuZHN0cmVhbQplbmRvYmoKNSAwIG9iago8PCAvVGl0bGUgKM9cKcLAekf2quymdLYpID4+CmVuZG9iago2IDAgb2JqCjw8IC9GaWx0ZXIgL1N0YW5kYXJkIC9WIDIgL1IgMyAvTGVuZ3RoIDEyOCAvTyA8MzY0NTFiZDM5ZDc1M2I3YzFkMTA5MjJjMjhlNjY2NWFhNGYzMzUzZmIwMzQ4YjUzNjg5M2UzYjFkYjVjNTc5Yj4gL1UgPDlhYzU4MDhiZDVkOTVlNmZkZDFkMmI1NWYxMDQwZDg2MDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDA+IC9QIC00NCA+PgplbmRvYmoKeHJlZgowIDcKMDAwMDAwMDAwMCA2NTUzNSBmIAowMDAwMDAwMDA5IDAwMDAwIG4gCjAwMDAwMDAwNTggMDAwMDAgbiAKMDAwMDAwMDExNSAwMDAwMCBuIAowMDAwMDAwMjAyIDAwMDAwIG4gCjAwMDAwMDAyOTQgMDAwMDAgbiAKMDAwMDAwMDMzOCAwMDAwMCBuIAp0cmFpbGVyCjw8IC9TaXplIDcgL1Jvb3QgMSAwIFIgL0luZm8gNSAwIFIgL0VuY3J5cHQgNiAwIFIgL0lEIFs8MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY+IDwwMTIzNDU2Nzg5YWJjZGVmMDEyMzQ1Njc4OWFiY2RlZj5dID4+CnN0YXJ0eHJlZgo1NDYKJSVFT0YK";

#[test]
fn opens_an_rc4_encrypted_document() {
    let doc = Document::open(from_base64(ENCRYPTED_PDF_BASE64)).unwrap();
    assert_eq!(doc.page_count().unwrap(), 1);

    // The page content stream decrypts to the original operators.
    let page = doc.pages().unwrap().remove(0);
    let content = doc.page_content_bytes(&page).unwrap();
    assert_eq!(content, b"BT /F1 24 Tf 72 700 Td (Secret Text) Tj ET");

    // The /Info /Title string decrypts too.
    let title = doc.info().unwrap().unwrap();
    assert_eq!(
        title.get(&Name::from("Title")),
        Some(&Object::String(pdf_cos::PdfString::from(
            b"Confidential".to_vec()
        )))
    );
}

/// An AES-256 (V5/R6, `AESV3`) PDF from the openssl-backed reference generator: one page
/// ("AES Secret"), /Info /Title "Locked", empty user password.
const AES256_PDF_BASE64: &str = "JVBERi0xLjcKMSAwIG9iago8PCAvVHlwZSAvQ2F0YWxvZyAvUGFnZXMgMiAwIFIgPj4KZW5kb2JqCjIgMCBvYmoKPDwgL1R5cGUgL1BhZ2VzIC9LaWRzIFszIDAgUl0gL0NvdW50IDEgPj4KZW5kb2JqCjMgMCBvYmoKPDwgL1R5cGUgL1BhZ2UgL1BhcmVudCAyIDAgUiAvTWVkaWFCb3ggWzAgMCA2MTIgNzkyXSAvQ29udGVudHMgNCAwIFIgPj4KZW5kb2JqCjQgMCBvYmoKPDwgL0xlbmd0aCA2NCA+PgpzdHJlYW0KBwcHBwcHBwcHBwcHBwcHB9sMYjNbF+s46Oj3M2K+QkJUg/6fcFKHSvzSomL9D/+iAZpUcgUguEL7s9n3Lx0QoAplbmRzdHJlYW0KZW5kb2JqCjUgMCBvYmoKPDwgL1RpdGxlICgHBwcHBwcHBwcHBwcHBwcHbb/qt3jsJ2rROtemhflazCkgPj4KZW5kb2JqCjYgMCBvYmoKPDwgL0ZpbHRlciAvU3RhbmRhcmQgL1YgNSAvUiA2IC9MZW5ndGggMjU2IC9PIDwwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDA+IC9VIDwzMmNkMTc0MGYzOThmNGI4MjBiNjNiNTNhMjFkZjE1NDBlZWQxNzMyNzA0MjI3MDc1MDYyMGViYzhhODM0NmRmMTExMTExMTExMTExMTExMTIyMjIyMjIyMjIyMjIyMjI+IC9PRSA8MDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMD4gL1VFIDw2MDA3MDIwMTRkOGI1YmNkNjlmMWE0ZjQ2NjRmMGEzYzE0ZGJlYzJiMDg3ZDM1NDQzY2RiZDYzY2E2YzYxODllPiAvUCAtNDQgL1Blcm1zIDwwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMD4gL0NGIDw8IC9TdGRDRiA8PCAvQ0ZNIC9BRVNWMyA+PiA+PiAvU3RtRiAvU3RkQ0YgL1N0ckYgL1N0ZENGID4+CmVuZG9iagp4cmVmCjAgNwowMDAwMDAwMDAwIDY1NTM1IGYgCjAwMDAwMDAwMDkgMDAwMDAgbiAKMDAwMDAwMDA1OCAwMDAwMCBuIAowMDAwMDAwMTE1IDAwMDAwIG4gCjAwMDAwMDAyMDIgMDAwMDAgbiAKMDAwMDAwMDMxNiAwMDAwMCBuIAowMDAwMDAwMzc5IDAwMDAwIG4gCnRyYWlsZXIKPDwgL1NpemUgNyAvUm9vdCAxIDAgUiAvSW5mbyA1IDAgUiAvRW5jcnlwdCA2IDAgUiAvSUQgWzxhYWJiPiA8Y2NkZD5dID4+CnN0YXJ0eHJlZgo4OTYKJSVFT0YK";

#[test]
fn opens_an_aes256_encrypted_document() {
    let doc = Document::open(from_base64(AES256_PDF_BASE64)).unwrap();
    assert_eq!(doc.page_count().unwrap(), 1);

    // The page content stream decrypts via AES-256 (file key used directly).
    let page = doc.pages().unwrap().remove(0);
    let content = doc.page_content_bytes(&page).unwrap();
    assert_eq!(content, b"BT /F1 24 Tf 72 700 Td (AES Secret) Tj ET");

    // The /Info /Title string decrypts too.
    let title = doc.info().unwrap().unwrap();
    assert_eq!(
        title.get(&Name::from("Title")),
        Some(&Object::String(pdf_cos::PdfString::from(
            b"Locked".to_vec()
        )))
    );
}

// --- PDF MAC integrity protection (ISO/TS 32004) ---

fn secret_doc() -> Document {
    let objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>".to_vec(),
        b"<< /Length 24 >>\nstream\nBT (Top secret) Tj ET\nendstream".to_vec(),
    ];
    Document::open(assemble(&objects, "")).unwrap()
}

#[test]
fn save_with_mac_round_trips_and_verifies() {
    for algorithm in [Algorithm::Aes256, Algorithm::Aes256Gcm] {
        let saved = secret_doc()
            .save_encrypted_with_mac(b"", b"", algorithm)
            .unwrap();
        // The AuthCode trailer dict, the KDFSalt, and the PDF 2.0 header are all present.
        assert!(saved.starts_with(b"%PDF-2.0"));
        assert!(saved.windows(9).any(|w| w == b"/AuthCode"));
        assert!(saved.windows(11).any(|w| w == b"/Standalone"));
        assert!(saved.windows(8).any(|w| w == b"/KDFSalt"));
        // The standalone MAC over the current bytes authenticates under the user password.
        let reopened = Document::open(saved).unwrap();
        assert_eq!(reopened.verify_pdf_mac(b"").unwrap(), Some(true));
    }
}

#[test]
fn mac_clears_permission_bit_13() {
    // §5.1.2/5.1.3: a MAC-protected file signals "MAC required" by zeroing /P bit 13.
    let saved = secret_doc()
        .save_encrypted_with_mac(b"", b"", Algorithm::Aes256)
        .unwrap();
    let reopened = Document::open(saved).unwrap();
    let Object::Dictionary(enc) = reopened
        .resolve(reopened.xref.trailer.get(&Name::from("Encrypt")).unwrap())
        .unwrap()
    else {
        panic!("no /Encrypt dict")
    };
    let p = enc.get_integer(&Name::from("P")).unwrap();
    assert_eq!(p & (1 << 12), 0, "/P bit 13 (MAC-required) must be zero");
    // Starting from all permissions (-1), only bit 13 is cleared → -4097 = !(1 << 12).
    assert_eq!(p, -4097);
}

#[test]
fn mac_detects_tampering() {
    let mut saved = secret_doc()
        .save_encrypted_with_mac(b"", b"", Algorithm::Aes256)
        .unwrap();
    // Flip a byte inside the binary-marker comment (offset 11): covered by the ByteRange but not
    // structural, so the file still parses — yet the document digest no longer matches the MAC.
    saved[11] ^= 0xFF;
    let reopened = Document::open(saved).unwrap();
    assert_eq!(reopened.verify_pdf_mac(b"").unwrap(), Some(false));
}

#[test]
fn mac_requires_aes256() {
    for algorithm in [Algorithm::Rc4, Algorithm::Aes128] {
        assert_eq!(
            secret_doc()
                .save_encrypted_with_mac(b"", b"", algorithm)
                .unwrap_err(),
            DocError::MacRequiresV5
        );
    }
}

#[test]
fn verify_pdf_mac_absent_without_authcode() {
    // A plain encrypted document (no /AuthCode) reports None, not a spurious verdict.
    let saved = secret_doc()
        .save_encrypted(b"", b"", Algorithm::Aes256)
        .unwrap();
    assert_eq!(
        Document::open(saved).unwrap().verify_pdf_mac(b"").unwrap(),
        None
    );
}

#[test]
fn verify_pdf_mac_rejects_authcode_without_usable_encrypt() {
    let objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R >>".to_vec(),
    ];
    // A standalone /AuthCode but no /Encrypt at all → not verifiable (Some(false)).
    let no_enc = Document::open(assemble(
        &objects,
        "/AuthCode << /MACLocation /Standalone >>",
    ))
    .unwrap();
    assert_eq!(no_enc.verify_pdf_mac(b"").unwrap(), Some(false));
    // /Encrypt resolves but carries no /KDFSalt → also Some(false).
    let no_salt = Document::open(assemble(
        &objects,
        "/Encrypt 1 0 R /AuthCode << /MACLocation /Standalone >>",
    ))
    .unwrap();
    assert_eq!(no_salt.verify_pdf_mac(b"").unwrap(), Some(false));
}

#[test]
fn verify_pdf_mac_unknown_location_is_none() {
    let mut bytes = secret_doc()
        .save_encrypted_with_mac(b"", b"", Algorithm::Aes256)
        .unwrap();
    // Rewrite /Standalone to an unrecognised location of the same length: not this engine's job.
    let pos = bytes
        .windows(b"/Standalone".len())
        .position(|w| w == b"/Standalone")
        .unwrap();
    bytes[pos..pos + b"/Standalone".len()].copy_from_slice(b"/Bogusxxxxx");
    let doc = Document::open(bytes).unwrap();
    assert_eq!(doc.verify_pdf_mac(b"").unwrap(), None);
}

#[test]
fn verify_pdf_mac_wrong_password_errors() {
    // Saved with an empty user password (so it reopens), but the file key cannot be recovered
    // without the right password: a wrong one is a clear error, not a false verdict.
    let saved = secret_doc()
        .save_encrypted_with_mac(b"", b"", Algorithm::Aes256)
        .unwrap();
    let reopened = Document::open(saved).unwrap();
    assert_eq!(
        reopened.verify_pdf_mac(b"wrong-password").unwrap_err(),
        DocError::NeedsPassword
    );
    assert_eq!(reopened.verify_pdf_mac(b"").unwrap(), Some(true));
}
