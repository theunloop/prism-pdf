#![forbid(unsafe_code)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]
//! pdf-content — content-stream operators (EPIC 6, ISO 32000 §7.8 / §8 / §9.4).
//!
//! Parses a decoded content stream into a sequence of [`Operation`]s (operator + operands) and,
//! as a first capability, extracts reading-order text from the text-showing operators (§9.4).
//! Depends only on [`pdf_cos`] (architecture: `content → cos`); decoding the stream's bytes is the
//! caller's job (via `pdf-filters`).
//!
//! Implemented so far (Milestone M1, text extraction):
//! - **§7.8.2 — Content-stream parsing**: [`parse_content`] → [`Operation`]s (inline images skipped)
//! - **§9.4 — Text extraction**: [`extract_text`] (basic; see encoding caveats in the source)
//!
//! Next: the graphics state machine (§8.4), text state (§9.3), and faithful glyph→Unicode mapping
//! via font encodings and `/ToUnicode` (§9.10, EPIC 7).

mod build;
mod layout;
mod lexer;
mod parser;
mod text;

pub use build::Content;
pub use layout::{TextFragment, extract_fragments, layout};
pub use parser::{Operation, parse_content};
pub use text::{
    GlyphDecoder, Latin1Decoder, extract_text, extract_text_with, extract_text_with_forms,
};
