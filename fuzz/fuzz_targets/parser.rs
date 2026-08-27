#![no_main]
//! Fuzz target for the PDF object parser (EPIC 2, ISO 32000 §7.3).
//!
//! The parser is the security-critical surface: continuous hostile-input fuzzing is mandatory
//! (DESIGN.md §3.4, §7). On ANY input it must return without panicking, looping forever, or
//! allocating unboundedly — the default [`Limits`] cap nesting so recursion cannot exhaust the
//! stack.
//!
//! Run with: `cargo +nightly fuzz run parser` (needs `cargo install cargo-fuzz`).

use libfuzzer_sys::fuzz_target;
use pdf_reader::Parser;

fuzz_target!(|data: &[u8]| {
    // Drain the object stream. Any Ok/Err outcome is fine; a panic or hang is a bug. The cursor
    // advances on every token, so this terminates.
    let mut parser = Parser::new(data);
    loop {
        match parser.parse_object() {
            Ok(Some(_)) => continue,
            Ok(None) | Err(_) => break,
        }
    }
});
