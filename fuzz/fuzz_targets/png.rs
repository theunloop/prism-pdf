#![no_main]
//! Fuzz target for the PNG image source (ISO 32000 §8.9.5 on the authoring side).
//!
//! `Image::from_png` hands untrusted bytes to the `zune-png` decoder and then re-shapes the
//! samples into the raw image paths the engine already had. The decoder is a third-party parser of
//! a length-prefixed chunk stream with an inflate step inside it — the classic shape for an
//! overflow, an unbounded allocation, or a panic on a malformed IHDR — and the re-shaping trusts
//! the geometry the decoder reports. On ANY input the whole path must return without panicking,
//! hanging, or allocating unboundedly (DESIGN.md §3.4, §7).
//!
//! Run with: `cargo +nightly fuzz run png` (needs `cargo install cargo-fuzz`).

use libfuzzer_sys::fuzz_target;
use prismpdf::Image;

fuzz_target!(|data: &[u8]| {
    let _ = Image::from_png(data);
});
