//! AES-256 (`AESV3`, V5/R6, §7.6.4.3.3–.4): key unwrap, write round-trips, and the `/Perms` seal.

use super::*;

/// AES-256 V5/R6 with distinct user/owner passwords — reference values from the generator.
fn aes256_two_password_dict() -> Dictionary {
    let mut d = aes256_encrypt_dict();
    for (key, value) in [
        (
            "U",
            "0396260d544d947616a79d025b32f1365507bdba71ce0350ef96f8f55af4564a\
             11111111111111112222222222222222",
        ),
        (
            "UE",
            "f56815ba98d2731e41fc45077591152568022df17d1f14b7d4409a7e44380d09",
        ),
        (
            "O",
            "53d99d8c5b202732cf0c0b01ce3eecdeed87709c7b55a26b5b206cc9b318682f\
             33333333333333334444444444444444",
        ),
        (
            "OE",
            "6fe861021db087d7897284a921cbfbbfbd4d9ec49adbf563346d522ba97fd4b1",
        ),
    ] {
        d.insert(Name::from(key), Object::String(PdfString::from(hex(value))));
    }
    d
}

#[test]
fn derives_the_reference_aes256_key() {
    // The empty user password unwraps the documented 32-byte file key (00..1f). /ID is
    // unused for V5 key derivation, hence the empty slice.
    let handler = StandardSecurityHandler::open(&aes256_encrypt_dict(), &[], b"").unwrap();
    assert_eq!(
        handler.key,
        hex("000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f")
    );
    assert_eq!(handler.method, Method::AesV3);
}

#[test]
fn aes256_rejects_a_wrong_password() {
    assert!(StandardSecurityHandler::open(&aes256_encrypt_dict(), &[], b"wrong").is_none());
}

#[test]
fn aes256_opens_with_user_or_owner_password() {
    let key = hex("000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f");
    assert_eq!(
        StandardSecurityHandler::open(&aes256_two_password_dict(), &[], b"user")
            .unwrap()
            .key,
        key
    );
    assert_eq!(
        StandardSecurityHandler::open(&aes256_two_password_dict(), &[], b"owner")
            .unwrap()
            .key,
        key
    );
    assert!(StandardSecurityHandler::open(&aes256_two_password_dict(), &[], b"nope").is_none());
}

#[test]
fn aes256_write_round_trips() {
    // Encrypt under a freshly generated V5/R6 handler, then reopen the produced /Encrypt dict
    // with the user and owner passwords and confirm the file key and object data come back.
    let (writer, dict, _id0) =
        StandardSecurityHandler::new_encrypter(b"user", b"owner", -1, true, Algorithm::Aes256)
            .expect("rng available");
    assert_eq!(dict.get_integer(&Name::from("V")), Some(5));
    assert_eq!(dict.get_integer(&Name::from("R")), Some(6));

    let plaintext = b"BT (V5 secret) Tj ET";
    let ciphertext = writer.encrypt(7, 0, plaintext).expect("rng available");
    assert_ne!(ciphertext, plaintext);

    // /ID is unused for V5 key derivation (empty slice).
    let user = StandardSecurityHandler::open(&dict, &[], b"user").unwrap();
    assert_eq!(user.key, writer.key);
    assert_eq!(user.decrypt(7, 0, &ciphertext).unwrap(), plaintext);

    let owner = StandardSecurityHandler::open(&dict, &[], b"owner").unwrap();
    assert_eq!(owner.key, writer.key);

    assert!(StandardSecurityHandler::open(&dict, &[], b"wrong").is_none());
}

#[test]
fn aes256_write_empty_password_round_trips() {
    // The common case: no owner password (defaults to the user password), empty user password.
    let (writer, dict, _id0) =
        StandardSecurityHandler::new_encrypter(b"", b"", -1, true, Algorithm::Aes256)
            .expect("rng available");
    let ct = writer.encrypt(1, 0, b"hello").expect("rng available");
    let reader = StandardSecurityHandler::open(&dict, &[], b"").unwrap();
    assert_eq!(reader.decrypt(1, 0, &ct).unwrap(), b"hello");
}

#[test]
fn aes256_perms_block_seals_permissions() {
    // /Perms decrypts (AES-256 ECB, file key) to the permission block: P, 0xFF×4, 'T', "adb".
    let (writer, dict, _id0) =
        StandardSecurityHandler::new_encrypter(b"", b"", -44, true, Algorithm::Aes256)
            .expect("rng available");
    let perms = string_bytes(&dict, "Perms").unwrap();
    let block = aes256_cbc_decrypt_nopad(&writer.key, &[0u8; 16], &perms).unwrap();
    assert_eq!(block.len(), 16);
    assert_eq!(
        i32::from_le_bytes([block[0], block[1], block[2], block[3]]),
        -44
    );
    assert_eq!(&block[4..8], &[0xFF; 4]);
    assert_eq!(block[8], b'T'); // EncryptMetadata = true
    assert_eq!(&block[9..12], b"adb");
}

#[test]
fn aes256_encrypt_metadata_false_seals_perms_and_round_trips() {
    let (writer, dict, _id0) =
        StandardSecurityHandler::new_encrypter(b"", b"", -1, false, Algorithm::Aes256)
            .expect("rng available");
    assert_eq!(
        dict.get(&Name::from("EncryptMetadata"))
            .and_then(Object::as_bool),
        Some(false)
    );
    // The /Perms byte 8 records EncryptMetadata as 'F'.
    let perms = string_bytes(&dict, "Perms").unwrap();
    let block = aes256_cbc_decrypt_nopad(&writer.key, &[0u8; 16], &perms).unwrap();
    assert_eq!(block[8], b'F');
    let ct = writer.encrypt(3, 0, b"data").expect("rng available");
    let reader = StandardSecurityHandler::open(&dict, &[], b"").unwrap();
    assert_eq!(reader.decrypt(3, 0, &ct).unwrap(), b"data");
}

#[test]
fn a_tampered_perms_seal_is_rejected() {
    // Algorithm 13 (§7.6.4.3.4): /Perms seals /P under the file key. Editing /P without being able
    // to re-seal it must not go unnoticed.
    let (_writer, dict, _id0) =
        StandardSecurityHandler::new_encrypter(b"", b"", -1, true, Algorithm::Aes256)
            .expect("rng available");
    assert!(StandardSecurityHandler::open(&dict, &[], b"").is_some());

    // Flip /P to "nothing allowed" while leaving the sealed copy alone.
    let mut tampered = dict.clone();
    tampered.insert(
        Name::from("P"),
        Object::Integer(i64::from(Permissions::RESTRICTED.bits())),
    );
    assert!(
        StandardSecurityHandler::open(&tampered, &[], b"").is_none(),
        "a /P that disagrees with its /Perms seal must not open"
    );

    // Same for the EncryptMetadata flag, which byte 8 of the seal covers.
    let mut flag = dict.clone();
    flag.insert(Name::from("EncryptMetadata"), Object::Boolean(false));
    assert!(StandardSecurityHandler::open(&flag, &[], b"").is_none());
}

#[test]
fn a_missing_perms_entry_is_tolerated() {
    // Real V5 files in the wild omit /Perms; refusing them would break documents every other
    // engine opens. Only a present-and-wrong seal is a rejection.
    let (_writer, dict, _id0) =
        StandardSecurityHandler::new_encrypter(b"", b"", -1, true, Algorithm::Aes256)
            .expect("rng available");
    let mut without = Dictionary::new();
    for (key, value) in dict.iter() {
        if key.as_bytes() != b"Perms" {
            without.insert(key.clone(), value.clone());
        }
    }
    assert!(StandardSecurityHandler::open(&without, &[], b"").is_some());
}
