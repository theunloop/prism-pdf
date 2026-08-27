//! Object decryption on fetch (§7.6.2), including the Identity-crypt-filter exemption (§7.4.10).

use super::{crypt_stream, failing_decrypt, xor_decrypt};
use crate::xref::*;

use pdf_cos::{Dictionary, Name, Object, Stream};

#[test]
fn identity_crypt_filter_exempts_stream_data() {
    // /Crypt with /Name /Identity, and the same with no /Name (defaults to Identity): both keep
    // the data verbatim instead of running it through the decryptor.
    for stream in [crypt_stream(Some("Identity")), crypt_stream(None)] {
        let out = decrypt_object(&Object::Stream(stream), 7, 0, &xor_decrypt).unwrap();
        let Object::Stream(s) = out else {
            panic!("expected a stream")
        };
        assert_eq!(
            &s.raw()[..],
            b"plaintext",
            "identity-crypt data must not be decrypted"
        );
    }
}

#[test]
fn non_identity_streams_are_decrypted() {
    // A /Crypt naming a real crypt filter, and a plain stream with no Crypt filter, both get
    // decrypted by the handler.
    let named = crypt_stream(Some("StdCF"));
    let Object::Stream(s) = decrypt_object(&Object::Stream(named), 1, 0, &xor_decrypt).unwrap()
    else {
        panic!("expected a stream")
    };
    assert_eq!(&s.raw()[..], &xor_decrypt(1, 0, b"plaintext").unwrap()[..]);

    let plain = Stream::new(Dictionary::new(), b"plaintext".to_vec());
    let Object::Stream(s) = decrypt_object(&Object::Stream(plain), 1, 0, &xor_decrypt).unwrap()
    else {
        panic!("expected a stream")
    };
    assert_eq!(&s.raw()[..], &xor_decrypt(1, 0, b"plaintext").unwrap()[..]);
}

#[test]
fn a_failed_decrypt_propagates_instead_of_yielding_empty_bytes() {
    // An AES-GCM authentication-tag mismatch reaches the reader as `None`. It must become an
    // error: substituting empty content would make a tampered document indistinguishable from one
    // whose streams are genuinely empty, which is exactly the guarantee AESV4 exists to provide.
    let stream = Stream::new(Dictionary::new(), b"ciphertext".to_vec());
    assert!(decrypt_object(&Object::Stream(stream), 1, 0, &failing_decrypt).is_none());

    // Also through a nested container, so the failure cannot be lost in the recursion.
    let mut dict = Dictionary::new();
    dict.insert(
        Name::from("Nested"),
        Object::String(pdf_cos::PdfString::from(b"ciphertext".to_vec())),
    );
    assert!(decrypt_object(&Object::Dictionary(dict), 1, 0, &failing_decrypt).is_none());
}
