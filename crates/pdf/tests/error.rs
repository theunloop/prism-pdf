//! The unified facade error (`prismpdf::Error`) and its `Result` alias (DESIGN.md §6.1).
//!
//! These exercise the public error surface: that each layer error converts in via `From`, that
//! `?` composes a `DocError`-returning call inside a `prismpdf::Result` function, and that
//! `Display` is transparent (the wrapper adds no noise over the underlying message).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use prismpdf::{DocError, Error, PdfAError, PdfUaError};

/// Every layer error converts into the unified `Error` via `From`/`#[from]`, landing in its
/// matching variant.
#[test]
fn from_each_layer_error() {
    let doc: Error = DocError::MissingCatalog.into();
    assert!(matches!(doc, Error::Document(DocError::MissingCatalog)));

    let pdfa: Error = PdfAError::UnembeddedFont.into();
    assert!(matches!(pdfa, Error::PdfA(PdfAError::UnembeddedFont)));

    let pdfua: Error = PdfUaError::NotTagged.into();
    assert!(matches!(pdfua, Error::PdfUa(PdfUaError::NotTagged)));
}

/// `#[error(transparent)]` means the unified error's `Display` is exactly the wrapped cause's —
/// no `"Document: …"` prefix leaks through — for every variant.
#[test]
fn display_is_transparent() {
    let cases: [(Error, String); 3] = [
        (
            DocError::MissingCatalog.into(),
            DocError::MissingCatalog.to_string(),
        ),
        (
            PdfAError::UnembeddedFont.into(),
            PdfAError::UnembeddedFont.to_string(),
        ),
        (
            PdfUaError::MissingTitle.into(),
            PdfUaError::MissingTitle.to_string(),
        ),
    ];
    for (wrapped, inner_msg) in cases {
        assert_eq!(wrapped.to_string(), inner_msg);
    }

    // `Error` is a real `std::error::Error`. With `transparent`, `source()` is forwarded to the
    // inner cause: a leaf error (no nested source) reports `None`, while a `DocError` that itself
    // wraps the reader layer surfaces that nested cause through the unified error.
    let leaf: Error = PdfUaError::MissingTitle.into();
    assert!((&leaf as &dyn std::error::Error).source().is_none());
}

/// `?` lifts a `DocError`-returning call (the document layer keeps its own error) into a function
/// that returns the unified `prismpdf::Result`, with no manual `map_err`. This is the whole point
/// of the aggregate error: layers stay precise, the facade stays uniform.
#[test]
fn question_mark_composes_across_layers() {
    fn lift() -> prismpdf::Result<()> {
        // The document layer hands back its own `DocError`; `?` converts it to `Error` here.
        let from_doc_layer: Result<(), DocError> = Err(DocError::BadPageTree);
        from_doc_layer?;
        Ok(())
    }

    let err = lift().unwrap_err();
    assert!(matches!(err, Error::Document(DocError::BadPageTree)));
}
