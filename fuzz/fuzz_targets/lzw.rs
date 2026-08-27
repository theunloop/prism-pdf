#![no_main]
//! Fuzz target for the LZWDecode filter and the shared predictors (EPIC 3, ISO 32000 §7.4.4).
//!
//! LZW is a hand-written variable-width (9–12 bit) code reader feeding a string table that grows
//! by one entry per code, with `/EarlyChange` shifting *when* the width grows — a state machine
//! where a code referring to an entry that does not exist yet, or a table that never resets, is
//! exactly what a hostile stream will contain. The `/Predictor` post-pass (PNG and TIFF, §7.4.4.4)
//! then reinterprets the decoded bytes as rows, so `/Columns`, `/Colors` and `/BitsPerComponent`
//! are untrusted geometry over an untrusted buffer. On ANY combination the filter must return
//! without panicking, hanging, or allocating unboundedly (DESIGN.md §3.4, §7).
//!
//! Run with: `cargo +nightly fuzz run lzw` (needs `cargo install cargo-fuzz`).

use libfuzzer_sys::fuzz_target;
use pdf_cos::{Dictionary, Name, Object};
use pdf_filters::lzw_decode;

fuzz_target!(|input: (u8, u8, u16, u8, &[u8])| {
    let (early_change, predictor, columns, depth, data) = input;
    const CAP: usize = 1 << 20;

    let mut params = Dictionary::new();
    params.insert(
        Name::from("EarlyChange"),
        Object::Integer(i64::from(early_change % 3)),
    );
    // 1 = none, 2 = TIFF, 10–15 = the PNG filters (§7.4.4.4 Table 10); anything else must be
    // rejected rather than trusted.
    params.insert(
        Name::from("Predictor"),
        Object::Integer(i64::from(predictor)),
    );
    params.insert(
        Name::from("Columns"),
        Object::Integer(1 + i64::from(columns % 4096)),
    );
    params.insert(
        Name::from("Colors"),
        Object::Integer(1 + i64::from(depth % 8)),
    );
    params.insert(
        Name::from("BitsPerComponent"),
        Object::Integer(i64::from(depth % 17)),
    );
    let _ = lzw_decode(data, Some(&params), CAP);

    // The unparameterised path: `/EarlyChange` 1, no predictor.
    let _ = lzw_decode(data, None, CAP);
});
