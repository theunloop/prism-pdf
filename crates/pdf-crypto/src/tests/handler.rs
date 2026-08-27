//! Handler selection and the V4 `/EncryptMetadata` flag (§7.6.4 / §7.6.5).

use super::*;

#[test]
fn rejects_non_standard_filter() {
    let mut d = Dictionary::new();
    d.insert(Name::from("Filter"), Object::Name(Name::from("Custom")));
    assert!(StandardSecurityHandler::open(&d, &[], b"").is_none());
    // A V5 dict lacking /U and /UE is malformed and must be rejected.
    let mut v5 = rc4_encrypt_dict();
    v5.insert(Name::from("V"), Object::Integer(5));
    assert!(StandardSecurityHandler::open(&v5, &reference_id(), b"").is_none());
}

#[test]
fn v4_encrypt_metadata_false_round_trips_and_is_recorded() {
    // /EncryptMetadata false changes the key derivation; open() must read it back and match.
    let (writer, dict, id0) =
        StandardSecurityHandler::new_encrypter(b"", b"", -44, false, Algorithm::Aes128)
            .expect("rng available");
    assert_eq!(
        dict.get(&Name::from("EncryptMetadata"))
            .and_then(Object::as_bool),
        Some(false)
    );
    assert_eq!(dict.get_integer(&Name::from("P")), Some(-44));
    let ct = writer.encrypt(3, 0, b"data").expect("rng available");
    let reader = StandardSecurityHandler::open(&dict, &id0, b"").unwrap();
    assert_eq!(reader.decrypt(3, 0, &ct).unwrap(), b"data");
}

#[test]
fn supports_reports_known_handlers() {
    assert!(supports(&rc4_encrypt_dict()));
    assert!(supports(&aes256_encrypt_dict()));
    let mut custom = rc4_encrypt_dict();
    custom.insert(
        Name::from("Filter"),
        Object::Name(Name::from("Adobe.PPKLite")),
    );
    assert!(!supports(&custom));
}
