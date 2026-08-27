#![no_main]
//! Fuzz target for the JBIG2Decode filter (EPIC 3, ISO 32000 §7.4.7).
//!
//! JBIG2 is a new, complex parser surface (arithmetic decoder, symbol dictionaries, text regions),
//! so continuous hostile-input fuzzing is mandatory (DESIGN.md §3.4, §7): on ANY input — and any
//! split into image/globals segments — the decoder must return without panicking, hanging, or
//! allocating unboundedly. The pixel-count guard caps memory regardless of the header's claims.
//!
//! Run with: `cargo +nightly fuzz run jbig2` (needs `cargo install cargo-fuzz`).

use libfuzzer_sys::fuzz_target;
use pdf_filters::jbig2_decode;

fuzz_target!(|data: &[u8]| {
    // A 1 MiB pixel cap keeps each iteration cheap while still exercising the decode paths.
    const CAP: usize = 1 << 20;

    // Self-contained stream (no globals) — the filter-chain entry point's behaviour.
    let _ = jbig2_decode(data, None, CAP);

    // Embedded stream + a separate globals stream: split the input so both segment readers run.
    if data.len() >= 2 {
        let (globals, image) = data.split_at(data.len() / 2);
        let _ = jbig2_decode(image, Some(globals), CAP);
    }
});
