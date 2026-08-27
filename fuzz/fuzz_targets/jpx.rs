#![no_main]
//! Fuzz target for the JPXDecode header reader (EPIC 3, ISO 32000 §7.4.9).
//!
//! Prism PDF does not decode JPEG 2000 pixels (no safe pure-Rust decoder exists — see `ROADMAP.md`
//! M24); it *does* parse the JP2 container and the codestream's SIZ main header to learn the
//! image's geometry and component count, because §7.4.9 makes the codestream authoritative over
//! the image dictionary. That parser walks a length-prefixed box tree taken verbatim from an
//! untrusted stream, which is the classic shape for an integer overflow or an unbounded read.
//! On ANY input it must return without panicking, hanging, or allocating unboundedly
//! (DESIGN.md §3.4, §7).
//!
//! Run with: `cargo +nightly fuzz run jpx` (needs `cargo install cargo-fuzz`).

use libfuzzer_sys::fuzz_target;
use pdf_filters::jpx_info;

fuzz_target!(|data: &[u8]| {
    let _ = jpx_info(data);
});
