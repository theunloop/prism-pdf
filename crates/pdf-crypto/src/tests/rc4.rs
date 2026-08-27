//! RC4 (V1/V2, §7.6.4.3.2) read/write and owner-password recovery (Algorithm 7).

use super::*;

/// RC4 V2/R3 with distinct user ("user") and owner ("owner") passwords — reference values from
/// the independent generator. The file key is `0708c7bd…`.
fn rc4_two_password_dict() -> Dictionary {
    let mut d = rc4_encrypt_dict();
    d.insert(
        Name::from("O"),
        Object::String(PdfString::from(hex(
            "0ba3835f88f90388e74e54584125ce142be0de24c6b0d37746e075b891756671",
        ))),
    );
    d.insert(
        Name::from("U"),
        Object::String(PdfString::from(hex(
            "5b947164064e83daf1379e5be31d5c3900000000000000000000000000000000",
        ))),
    );
    d
}

#[test]
fn derives_the_reference_rc4_key() {
    let handler = StandardSecurityHandler::open(&rc4_encrypt_dict(), &reference_id(), b"").unwrap();
    assert_eq!(handler.key, hex("f5d1bcf88811c838d1f7f145904fe466"));
}

#[test]
fn decrypts_a_reference_object() {
    let handler = StandardSecurityHandler::open(&rc4_encrypt_dict(), &reference_id(), b"").unwrap();
    let content =
        hex("9df4bc3f5aa948d09626afa71914df73f351c3dcdc125692eff6b86021d3e702571f17234a1a525712a5");
    assert_eq!(
        handler.decrypt(4, 0, &content).unwrap(),
        b"BT /F1 24 Tf 72 700 Td (Secret Text) Tj ET"
    );
    // The title string of object 5 decrypts too.
    assert_eq!(
        handler
            .decrypt(5, 0, &hex("cf29c2c07a47f6aaeca674b6"))
            .unwrap(),
        b"Confidential"
    );
}

#[test]
fn rc4_opens_with_user_or_owner_password() {
    let key = hex("0708c7bdda190c3cc20a44e674bb2c0e");
    let user = StandardSecurityHandler::open(&rc4_two_password_dict(), &reference_id(), b"user");
    let owner = StandardSecurityHandler::open(&rc4_two_password_dict(), &reference_id(), b"owner");
    assert_eq!(user.unwrap().key, key);
    assert_eq!(owner.unwrap().key, key); // owner password recovers the same file key
    assert!(
        StandardSecurityHandler::open(&rc4_two_password_dict(), &reference_id(), b"nope").is_none()
    );
}

#[test]
fn rc4_ignores_encrypt_metadata_false() {
    // RC4/V2 has no EncryptMetadata switch: the entry is not written and the round-trip holds.
    let (writer, dict, id0) =
        StandardSecurityHandler::new_encrypter(b"", b"", -1, false, Algorithm::Rc4)
            .expect("rng available");
    assert!(dict.get(&Name::from("EncryptMetadata")).is_none());
    let ct = writer.encrypt(2, 0, b"data").expect("rng available");
    let reader = StandardSecurityHandler::open(&dict, &id0, b"").unwrap();
    assert_eq!(reader.decrypt(2, 0, &ct).unwrap(), b"data");
}
