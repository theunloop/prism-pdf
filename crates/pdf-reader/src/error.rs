//! Reader error type (DESIGN.md §6/§7: `thiserror` enum per crate, mapped to stable integer
//! codes only at the FFI boundary — never here).
//!
//! The reader treats every byte of input as hostile (DESIGN.md §3.4): parsing fallible input
//! returns a [`ReaderError`] rather than panicking. Each variant records the byte
//! [`offset`](ReaderError::offset) where the problem was detected so callers (and recovery, EPIC
//! 2) can locate it in the original file.

use std::fmt;

/// The result type returned throughout `pdf-reader`.
pub type Result<T> = std::result::Result<T, ReaderError>;

/// A failure encountered while reading a PDF.
///
/// Carries the absolute byte `offset` into the input where the error was detected, which is the
/// anchor recovery uses to resynchronise. Construct with [`ReaderError::new`].
#[derive(Clone, PartialEq, Eq)]
pub struct ReaderError {
    kind: ErrorKind,
    offset: usize,
}

impl ReaderError {
    /// Build an error of `kind` detected at byte `offset`.
    #[must_use]
    pub fn new(kind: ErrorKind, offset: usize) -> Self {
        Self { kind, offset }
    }

    /// The classification of this error.
    #[must_use]
    pub fn kind(&self) -> ErrorKind {
        self.kind
    }

    /// The absolute byte offset into the input at which the error was detected.
    #[must_use]
    pub fn offset(&self) -> usize {
        self.offset
    }
}

impl fmt::Display for ReaderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} at byte {}", self.kind, self.offset)
    }
}

impl fmt::Debug for ReaderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ReaderError {{ {:?} @ {} }}", self.kind, self.offset)
    }
}

impl std::error::Error for ReaderError {}

/// The classification of a [`ReaderError`].
///
/// `Copy` and field-free so it maps cleanly onto a stable integer code at the FFI boundary
/// (DESIGN.md §6). Human-readable detail lives in the [`Display`](fmt::Display) text.
#[derive(Clone, Copy, PartialEq, Eq, Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ErrorKind {
    /// A token ran past the end of the input (e.g. an unterminated string or hex string).
    #[error("unexpected end of input")]
    UnexpectedEof,
    /// A byte appeared where the lexical grammar (§7.2) does not allow it.
    #[error("unexpected byte")]
    UnexpectedByte,
    /// A numeric token (§7.3.3) could not be represented (overflow or malformed).
    #[error("invalid number")]
    InvalidNumber,
    /// A hexadecimal string (§7.3.4.3) contained a non-hex digit.
    #[error("invalid hexadecimal digit")]
    InvalidHexDigit,
    /// A token appeared where the object grammar (§7.3) does not allow it.
    #[error("unexpected token")]
    UnexpectedToken,
    /// A composite object (§7.3.6/§7.3.7) nested deeper than the configured limit (anti-DoS,
    /// DESIGN.md §3.4).
    #[error("nesting too deep")]
    NestingTooDeep,
    /// A stream (§7.3.8) was not terminated by an `endstream` keyword.
    #[error("unterminated stream")]
    UnterminatedStream,
    /// The `startxref` pointer to the cross-reference table (§7.5.5) is missing or unreadable.
    #[error("missing or invalid startxref")]
    MissingStartxref,
    /// The cross-reference table (§7.5.4) or trailer (§7.5.5) is malformed.
    #[error("malformed cross-reference table or trailer")]
    InvalidXref,
    /// A cross-reference stream (§7.5.8) or object stream (§7.5.7) could not be decoded by the
    /// filter layer (§7.4).
    #[error("failed to decode a cross-reference or object stream")]
    StreamDecodeFailed,
    /// A configured anti-DoS limit was exceeded (DESIGN.md §3.4) — e.g. an object stream whose
    /// `/N` count is implausibly large (§7.5.7).
    #[error("anti-DoS limit exceeded")]
    LimitExceeded,
    /// Decrypting an object's content failed (§7.6.2). For an authenticated crypt filter
    /// (`AESV4`/AES-256-GCM, ISO/TS 32003) this means the authentication tag did not verify — the
    /// document has been altered since it was encrypted. For the unauthenticated CBC modes it
    /// means the ciphertext was truncated or its padding was invalid.
    ///
    /// This is deliberately distinct from "the stream was empty": a tag check that fails must be
    /// visible to the caller, since detecting tampering is the entire purpose of the GCM filter.
    #[error("failed to decrypt object content")]
    DecryptionFailed,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn carries_kind_and_offset() {
        let err = ReaderError::new(ErrorKind::InvalidXref, 42);
        assert_eq!(err.kind(), ErrorKind::InvalidXref);
        assert_eq!(err.offset(), 42);
    }

    #[test]
    fn display_includes_message_and_offset() {
        let err = ReaderError::new(ErrorKind::UnexpectedEof, 7);
        assert_eq!(err.to_string(), "unexpected end of input at byte 7");
    }

    #[test]
    fn debug_is_compact() {
        let err = ReaderError::new(ErrorKind::UnexpectedByte, 3);
        let debug = format!("{err:?}");
        assert!(debug.contains("UnexpectedByte"));
        assert!(debug.contains("@ 3"));
    }

    #[test]
    fn every_kind_has_a_distinct_message() {
        let kinds = [
            ErrorKind::UnexpectedEof,
            ErrorKind::UnexpectedByte,
            ErrorKind::InvalidNumber,
            ErrorKind::InvalidHexDigit,
            ErrorKind::UnexpectedToken,
            ErrorKind::NestingTooDeep,
            ErrorKind::UnterminatedStream,
            ErrorKind::MissingStartxref,
            ErrorKind::InvalidXref,
            ErrorKind::StreamDecodeFailed,
            ErrorKind::LimitExceeded,
        ];
        let messages: std::collections::BTreeSet<String> =
            kinds.iter().map(ToString::to_string).collect();
        assert_eq!(
            messages.len(),
            kinds.len(),
            "messages must be unique and non-empty"
        );
        assert!(messages.iter().all(|m| !m.is_empty()));
        // ReaderError implements std::error::Error.
        let err = ReaderError::new(ErrorKind::InvalidNumber, 0);
        let _: &dyn std::error::Error = &err;
    }
}
