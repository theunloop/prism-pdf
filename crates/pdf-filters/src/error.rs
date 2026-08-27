//! Filter error type (DESIGN.md §6/§7: `thiserror` enum per crate, mapped to stable integer
//! codes only at the FFI boundary).
//!
//! Decoders treat their input as hostile (DESIGN.md §3.4): malformed data returns a
//! [`FilterError`] rather than panicking, and a decompression bomb is bounded by an explicit
//! output limit ([`FilterError::TooLarge`]).

/// The result type returned throughout `pdf-filters`.
pub type Result<T> = std::result::Result<T, FilterError>;

/// A failure while decoding a stream filter (§7.4).
///
/// Each variant names the filter (a stable `&'static str`) so the message is actionable and the
/// FFI layer can map it to a code without parsing text.
#[derive(Clone, PartialEq, Eq, Debug, thiserror::Error)]
#[non_exhaustive]
pub enum FilterError {
    /// The encoded data was malformed for the named filter.
    #[error("corrupt {filter} data")]
    Corrupt {
        /// The filter that rejected the data (e.g. `"FlateDecode"`).
        filter: &'static str,
    },
    /// The filter is recognised but not yet implemented (e.g. `LZWDecode`, `DCTDecode`).
    #[error("unsupported filter: {0}")]
    Unsupported(&'static str),
    /// Decoding would produce more than the configured limit — a decompression-bomb guard
    /// (DESIGN.md §3.4).
    #[error("decoded output exceeded the {limit}-byte limit")]
    TooLarge {
        /// The byte limit that was exceeded.
        limit: usize,
    },
    /// The `/DecodeParms` for the named filter were invalid or unsupported.
    #[error("invalid decode parameters for {filter}")]
    InvalidParams {
        /// The filter whose parameters were rejected.
        filter: &'static str,
    },
    /// The stream's `/Filter` chain names more filters than will be run — an anti-DoS guard, since
    /// every stage re-processes the whole of the previous stage's output (DESIGN.md §3.4).
    #[error("filter chain has more than {limit} stages")]
    ChainTooLong {
        /// The stage-count limit that was exceeded.
        limit: usize,
    },
}
