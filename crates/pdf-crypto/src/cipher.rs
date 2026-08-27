//! Low-level ciphers used by the standard security handler (ISO 32000-1 §7.6).
//!
//! RC4 is implemented inline (a few lines; the RustCrypto `rc4` crate fixes the key length at the
//! type level, which does not fit PDF's runtime-variable keys). AES-128-CBC is delegated to
//! RustCrypto (`aes` + `cbc`), per DESIGN.md §6.

use aes::{Aes128, Aes256};
use aes_gcm::aead::Aead;
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use cbc::cipher::{
    BlockDecryptMut, BlockEncryptMut, KeyIvInit,
    block_padding::{NoPadding, Pkcs7},
};
use cbc::{Decryptor, Encryptor};

/// AES-256-GCM nonce length (96 bits — the recommended size, ISO/TS 32003).
pub const GCM_NONCE_LEN: usize = 12;

/// RC4 keystream cipher (symmetric: the same call encrypts and decrypts), §7.6.2.
#[must_use]
pub fn rc4(key: &[u8], data: &[u8]) -> Vec<u8> {
    debug_assert!(!key.is_empty());
    let mut s: [u8; 256] = std::array::from_fn(|i| i as u8);
    let mut j = 0u8;
    for i in 0..256 {
        j = j.wrapping_add(s[i]).wrapping_add(key[i % key.len()]);
        s.swap(i, j as usize);
    }
    let mut out = Vec::with_capacity(data.len());
    let (mut a, mut b) = (0u8, 0u8);
    for &byte in data {
        a = a.wrapping_add(1);
        b = b.wrapping_add(s[a as usize]);
        s.swap(a as usize, b as usize);
        let k = s[(s[a as usize].wrapping_add(s[b as usize])) as usize];
        out.push(byte ^ k);
    }
    out
}

/// Decrypt AES-128-CBC data whose first 16 bytes are the IV, removing PKCS#7 padding (§7.6.2).
/// `None` if the input is too short, the key length is wrong, or the padding is invalid.
#[must_use]
pub fn aes128_cbc_decrypt(key: &[u8], data: &[u8]) -> Option<Vec<u8>> {
    if key.len() != 16 || data.len() < 16 {
        return None;
    }
    let (iv, ciphertext) = data.split_at(16);
    let decryptor = Decryptor::<Aes128>::new_from_slices(key, iv).ok()?;
    decryptor.decrypt_padded_vec_mut::<Pkcs7>(ciphertext).ok()
}

/// Decrypt AES-256-CBC data whose first 16 bytes are the IV, removing PKCS#7 padding (§7.6.2,
/// `AESV3`). `None` if the input is too short, the key length is wrong, or the padding is invalid.
///
/// CBC is unauthenticated, so a padding failure is evidence of corruption rather than proof of
/// tampering — but it is still evidence, and the caller is told rather than handed empty bytes.
#[must_use]
pub fn aes256_cbc_decrypt(key: &[u8], data: &[u8]) -> Option<Vec<u8>> {
    if key.len() != 32 || data.len() < 16 {
        return None;
    }
    let (iv, ciphertext) = data.split_at(16);
    let decryptor = Decryptor::<Aes256>::new_from_slices(key, iv).ok()?;
    decryptor.decrypt_padded_vec_mut::<Pkcs7>(ciphertext).ok()
}

/// Decrypt AES-256-CBC with an explicit IV and **no** padding (§7.6.4.3.4, Algorithm 2.A: used to
/// unwrap the file key from `/UE`/`/OE`). `data` must be a whole number of 16-byte blocks.
#[must_use]
pub fn aes256_cbc_decrypt_nopad(key: &[u8], iv: &[u8], data: &[u8]) -> Option<Vec<u8>> {
    if key.len() != 32 || iv.len() != 16 || data.is_empty() || data.len() % 16 != 0 {
        return None;
    }
    let decryptor = Decryptor::<Aes256>::new_from_slices(key, iv).ok()?;
    decryptor.decrypt_padded_vec_mut::<NoPadding>(data).ok()
}

/// Encrypt AES-128-CBC and return `iv || ciphertext` (§7.6.2, `AESV2` write path), PKCS#7-padded.
/// Returns the empty vector if the key or IV length is wrong.
#[must_use]
pub fn aes128_cbc_encrypt(key: &[u8], iv: &[u8], data: &[u8]) -> Vec<u8> {
    if key.len() != 16 || iv.len() != 16 {
        return Vec::new();
    }
    let Ok(encryptor) = Encryptor::<Aes128>::new_from_slices(key, iv) else {
        return Vec::new();
    };
    let mut out = iv.to_vec();
    out.extend_from_slice(&encryptor.encrypt_padded_vec_mut::<Pkcs7>(data));
    out
}

/// Encrypt AES-128-CBC with an explicit IV and **no** padding (§7.6.4.3.4, Algorithm 2.B: the inner
/// step of the hardened password hash). `data` must be a whole number of 16-byte blocks.
#[must_use]
pub fn aes128_cbc_encrypt_nopad(key: &[u8], iv: &[u8], data: &[u8]) -> Vec<u8> {
    if key.len() != 16 || iv.len() != 16 || data.len() % 16 != 0 {
        return Vec::new();
    }
    let Ok(encryptor) = Encryptor::<Aes128>::new_from_slices(key, iv) else {
        return Vec::new();
    };
    encryptor.encrypt_padded_vec_mut::<NoPadding>(data)
}

/// Encrypt AES-256-CBC and return `iv || ciphertext` (§7.6.2, `AESV3` write path), PKCS#7-padded.
/// Returns the empty vector if the key or IV length is wrong.
#[must_use]
pub fn aes256_cbc_encrypt(key: &[u8], iv: &[u8], data: &[u8]) -> Vec<u8> {
    if key.len() != 32 || iv.len() != 16 {
        return Vec::new();
    }
    let Ok(encryptor) = Encryptor::<Aes256>::new_from_slices(key, iv) else {
        return Vec::new();
    };
    let mut out = iv.to_vec();
    out.extend_from_slice(&encryptor.encrypt_padded_vec_mut::<Pkcs7>(data));
    out
}

/// Encrypt AES-256-CBC with an explicit IV and **no** padding (§7.6.4.3.3–.4, Algorithms 8–10: wrap
/// the file key into `/UE`/`/OE`, and produce `/Perms`). `data` must be a whole number of 16-byte
/// blocks. Returns the empty vector on a length mismatch.
#[must_use]
pub fn aes256_cbc_encrypt_nopad(key: &[u8], iv: &[u8], data: &[u8]) -> Vec<u8> {
    if key.len() != 32 || iv.len() != 16 || data.is_empty() || data.len() % 16 != 0 {
        return Vec::new();
    }
    let Ok(encryptor) = Encryptor::<Aes256>::new_from_slices(key, iv) else {
        return Vec::new();
    };
    encryptor.encrypt_padded_vec_mut::<NoPadding>(data)
}

/// Encrypt AES-256-GCM and return `nonce ‖ ciphertext ‖ tag` (ISO/TS 32003, `AESV4`): the 12-byte
/// `nonce` is prepended and the 16-byte authentication tag is appended by the AEAD. Returns the
/// empty vector if the key or nonce length is wrong.
#[must_use]
pub fn aes256_gcm_encrypt(key: &[u8], nonce: &[u8], data: &[u8]) -> Vec<u8> {
    if key.len() != 32 || nonce.len() != GCM_NONCE_LEN {
        return Vec::new();
    }
    let Ok(cipher) = Aes256Gcm::new_from_slice(key) else {
        return Vec::new();
    };
    let Ok(ciphertext) = cipher.encrypt(Nonce::from_slice(nonce), data) else {
        return Vec::new();
    };
    let mut out = nonce.to_vec();
    out.extend_from_slice(&ciphertext);
    out
}

/// Decrypt AES-256-GCM data laid out as `nonce ‖ ciphertext ‖ tag` (ISO/TS 32003, `AESV4`),
/// verifying the authentication tag. `None` if the input is too short, the key is wrong, or
/// **authentication fails** — a tampered or truncated payload.
///
/// The distinction between `None` and `Some(vec![])` is the whole value of this filter. ISO/TS
/// 32003 added `AESV4` so that modifying an encrypted PDF is *detectable*; mapping a failed tag
/// check onto empty output would hand the caller a document whose streams silently read as empty
/// and no way to tell that from a document that legitimately has empty streams.
#[must_use]
pub fn aes256_gcm_decrypt(key: &[u8], data: &[u8]) -> Option<Vec<u8>> {
    if key.len() != 32 || data.len() < GCM_NONCE_LEN {
        return None;
    }
    let (nonce, ciphertext) = data.split_at(GCM_NONCE_LEN);
    let cipher = Aes256Gcm::new_from_slice(key).ok()?;
    cipher.decrypt(Nonce::from_slice(nonce), ciphertext).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rc4_known_answer() {
        // Classic vector: RC4("Key", "Plaintext") = BBF316E8D940AF0AD3.
        let out = rc4(b"Key", b"Plaintext");
        assert_eq!(out, hex(b"BBF316E8D940AF0AD3"));
        // Symmetric: applying again with the same key restores the plaintext.
        assert_eq!(rc4(b"Key", &out), b"Plaintext");
    }

    #[test]
    fn aes128_round_trips_via_cbc() {
        use cbc::Encryptor;
        use cbc::cipher::BlockEncryptMut;
        let key = [0x11u8; 16];
        let iv = [0x22u8; 16];
        let plaintext = b"the quick brown fox";
        let mut buf = iv.to_vec();
        let ct = Encryptor::<Aes128>::new(&key.into(), &iv.into())
            .encrypt_padded_vec_mut::<Pkcs7>(plaintext);
        buf.extend_from_slice(&ct);
        assert_eq!(aes128_cbc_decrypt(&key, &buf).unwrap(), plaintext);
    }

    #[test]
    fn aes128_rejects_short_or_wrong_key() {
        assert!(aes128_cbc_decrypt(&[0; 16], &[0; 4]).is_none());
        assert!(aes128_cbc_decrypt(&[0; 8], &[0; 32]).is_none());
    }

    #[test]
    fn aes256_gcm_round_trips_and_authenticates() {
        let key = [0x33u8; 32];
        let nonce = [0x44u8; GCM_NONCE_LEN];
        let plaintext = b"authenticated content";
        let blob = aes256_gcm_encrypt(&key, &nonce, plaintext);
        // Layout: nonce ‖ ciphertext ‖ 16-byte tag.
        assert_eq!(&blob[..GCM_NONCE_LEN], &nonce);
        assert_eq!(blob.len(), GCM_NONCE_LEN + plaintext.len() + 16);
        assert_eq!(aes256_gcm_decrypt(&key, &blob).unwrap(), plaintext);

        // A flipped ciphertext byte fails authentication → None, so the caller can tell a
        // tampered document from one whose streams are genuinely empty.
        let mut tampered = blob.clone();
        let last = tampered.len() - 1;
        tampered[last] ^= 0x01;
        assert!(aes256_gcm_decrypt(&key, &tampered).is_none());
        // Wrong key also fails.
        assert!(aes256_gcm_decrypt(&[0x99; 32], &blob).is_none());
    }

    /// Decode an ASCII-hex string (uppercase) to bytes — test helper.
    fn hex(s: &[u8]) -> Vec<u8> {
        s.chunks(2)
            .map(|c| {
                let v = |b: u8| (b as char).to_digit(16).unwrap() as u8;
                (v(c[0]) << 4) | v(c[1])
            })
            .collect()
    }
}
