#![no_main]
//! Fuzz target for the font CMap parsers (EPIC 7, ISO 32000 §9.7 / §9.10).
//!
//! Both the composite-font `/Encoding` CMap ([`pdf_fonts::CMap`], §9.7.5–6) and the `/ToUnicode`
//! CMap ([`pdf_fonts::ToUnicode`], §9.10.3) parse a small PostScript-like program drawn from
//! untrusted PDF bytes — new attack surface from Milestone M9. On ANY input both the parse and the
//! subsequent code→CID / code→text decode of arbitrary shown bytes must return without panicking,
//! hanging, or allocating unboundedly (DESIGN.md §3.4, §7).
//!
//! Run with: `cargo +nightly fuzz run cmap` (needs `cargo install cargo-fuzz`).

use libfuzzer_sys::fuzz_target;
use pdf_fonts::{CMap, ToUnicode};

fuzz_target!(|data: &[u8]| {
    // Split the input: the first half is the CMap program, the second half is a shown string to
    // decode through it — so both the parser and the decode path are exercised.
    let (program, shown) = data.split_at(data.len() / 2);

    let cmap = CMap::parse(program);
    let _ = cmap.codes_to_cids(shown);

    let to_unicode = ToUnicode::parse(program);
    let _ = to_unicode.decode(shown);
});
