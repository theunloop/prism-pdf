//! Tamper detection for the authenticated crypt filter (`AESV4`/AES-256-GCM, ISO/TS 32003).
//!
//! The point of the GCM filter is that modifying an encrypted PDF is *detectable*. A failed
//! authentication tag must therefore reach the caller as an error — never as empty content, which
//! a caller cannot tell apart from a document whose streams are legitimately empty.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use prismpdf::{Algorithm, Builder, Content, Document, PageSpec, StdFont, page_text};

const SECRET: &str = "CONFIDENTIAL SALARY DATA";

/// A one-page encrypted document containing [`SECRET`], under `algorithm`.
fn encrypted(algorithm: Algorithm) -> Vec<u8> {
    let mut content = Content::new();
    content
        .begin_text()
        .set_font("F1", 24.0)
        .text_move(72.0, 700.0)
        .show_text(SECRET.as_bytes())
        .end_text();
    let mut builder = Builder::new();
    builder.add_page(
        PageSpec::new(content.as_bytes().to_vec()).standard_font("F1", StdFont::Helvetica),
    );
    Document::open(builder.build())
        .expect("build")
        .save_encrypted(b"", b"owner", algorithm)
        .expect("encrypt")
}

/// Flip a byte inside the first stream body — i.e. inside the ciphertext.
fn tamper(mut pdf: Vec<u8>) -> Vec<u8> {
    let at = pdf
        .windows(8)
        .position(|w| w == b"stream\r\n")
        .or_else(|| pdf.windows(7).position(|w| w == b"stream\n"))
        .expect("a stream body");
    let index = at + 40;
    pdf[index] ^= 0xFF;
    pdf
}

#[test]
fn gcm_tampering_is_reported_not_swallowed() {
    let intact = encrypted(Algorithm::Aes256Gcm);

    // Baseline: the untouched document reads back its content.
    let doc = Document::open_with_password(intact.clone(), b"").expect("open");
    assert_eq!(page_text(&doc, 0).unwrap().as_deref(), Some(SECRET));

    // Tampered: the tag check fails. Opening may still succeed — parsing is lazy, and the header,
    // xref and catalog are untouched — but touching the altered object must be an error, and must
    // not yield `Ok(Some(""))`.
    let error = match Document::open_with_password(tamper(intact), b"") {
        Err(error) => error.to_string(),
        Ok(doc) => match page_text(&doc, 0) {
            Err(error) => error.to_string(),
            Ok(text) => panic!("tampering was not detected; page text came back as {text:?}"),
        },
    };
    assert!(
        error.contains("decrypt"),
        "the error should name decryption, got {error:?}"
    );
}

#[test]
fn an_intact_document_round_trips_under_every_algorithm() {
    // The tamper check must not have made ordinary decryption fragile.
    for algorithm in [
        Algorithm::Rc4,
        Algorithm::Aes128,
        Algorithm::Aes256,
        Algorithm::Aes256Gcm,
    ] {
        let doc = Document::open_with_password(encrypted(algorithm), b"").expect("open");
        assert_eq!(
            page_text(&doc, 0).unwrap().as_deref(),
            Some(SECRET),
            "round trip failed for {algorithm:?}"
        );
    }
}
