//! The unified error type for the Prism PDF public API (DESIGN.md §6.1).
//!
//! [`Error`] aggregates the fallible layers behind the facade into a single type, so applications
//! program against one [`Result`] alias instead of juggling each layer's error. Every lower layer
//! keeps its own *precise* error — [`DocError`] (document/read/write/sign), [`PdfAError`] and
//! [`PdfUaError`] (standards production) — and `Error` wraps them with `#[from]` conversions, so
//! `?` composes across layers with no manual `map_err`. The variants stay public (re-exported from
//! the crate root) so callers can still match the exact cause.
//!
//! Layering note: the lower crates cannot name this facade-level type without inverting the
//! dependency graph (DESIGN.md §5), so [`Document`](crate::Document)'s own methods return
//! [`DocError`] natively; `Error: From<DocError>` makes them compose seamlessly here. The C ABI
//! (`pdf-ffi`) flattens this same set of causes into the stable `PrismPdfStatus` integer codes.

use pdf_document::DocError;
use pdf_standards::{PdfAError, PdfUaError};

/// The unified error type returned across the Prism PDF facade API.
///
/// Marked `#[non_exhaustive]`: future layers may add variants, so always include a `_` arm (or
/// match `Error` opaquely via its [`Display`](std::fmt::Display) text) when matching exhaustively.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// An error from the document layer — open/save/edit/merge/sign (§7.5–§12.8).
    #[error(transparent)]
    Document(#[from] DocError),
    /// A PDF/A conformance-production error (§14, ISO 19005): e.g. an unembedded font.
    #[error(transparent)]
    PdfA(#[from] PdfAError),
    /// A PDF/UA accessibility-production error (ISO 14289-1): e.g. an untagged document.
    #[error(transparent)]
    PdfUa(#[from] PdfUaError),
}

/// The result type for the Prism PDF facade API: shorthand for `Result<T, `[`Error`]`>`.
///
/// The `E` default lets crate-internal helpers keep returning a precise layer error (e.g.
/// `Result<T, DocError>`) while the public surface uses the unified `Result<T>`.
pub type Result<T, E = Error> = core::result::Result<T, E>;
