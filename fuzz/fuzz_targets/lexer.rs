#![no_main]
//! Fuzz target for the PDF lexer (EPIC 2, ISO 32000 §7.2/§7.3).
//!
//! Continuous hostile-input fuzzing of the parser is mandatory (DESIGN.md §3.4, §7): on ANY
//! input the lexer must return without panicking, looping forever, or allocating unboundedly.
//!
//! Run with: `cargo +nightly fuzz run lexer` (needs `cargo install cargo-fuzz`).

use libfuzzer_sys::fuzz_target;
use pdf_reader::Lexer;

fuzz_target!(|data: &[u8]| {
    // Drain the whole token stream. Every outcome is acceptable except a panic or a hang: the
    // cursor advances on every branch, so the loop is guaranteed to terminate.
    let mut lexer = Lexer::new(data);
    loop {
        match lexer.next_token() {
            Ok(Some(_)) => continue,
            Ok(None) | Err(_) => break,
        }
    }
});
