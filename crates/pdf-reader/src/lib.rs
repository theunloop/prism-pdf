#![forbid(unsafe_code)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]
//! pdf-reader — parser/lexer/xref/recovery (EPIC 2, ISO 32000 §7.2–§7.5).
//!
//! Turns untrusted PDF bytes into [`pdf_cos`] objects. Input is treated as hostile (DESIGN.md
//! §3.4): the public surface is fallible (see [`ReaderError`]), never panics, and recovery is a
//! first-class path rather than a fallback.
//!
//! Implemented so far (Milestone M1, see `docs/spec-map.md`):
//! - **§7.2 — Lexer**: [`Lexer`] / [`Token`].
//! - **§7.3 — Object parser**: [`Parser`] — direct objects, references, indirect objects, streams.
//! - **§7.5 — File structure**: [`XRef`] — header, classic + stream xref, trailer, `/Prev` chain,
//!   object streams, and scan-based recovery ([`XRef::rebuild`]).
//!
//! The filter layer (`pdf-filters`) now covers Flate, LZW, the ASCII filters, RunLength and DCT.

mod error;
mod lexer;
mod parser;
mod trace;
mod xref;

pub use error::{ErrorKind, ReaderError, Result};
pub use lexer::{Lexer, Token};
pub use parser::{Limits, Parser};
pub use xref::{Version, XRef, XRefEntry};
