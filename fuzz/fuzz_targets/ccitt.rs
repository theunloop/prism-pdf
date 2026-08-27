#![no_main]
//! Fuzz target for the CCITTFaxDecode filter (EPIC 3, ISO 32000 §7.4.6).
//!
//! The Group 3/4 decoder is a hand-written MSB-first bit reader over hand-transcribed T.4/T.6
//! Huffman tables, driving a changing-element state machine — the kind of code where a malformed
//! run length or a mode code at the end of a row turns into an index out of bounds. On ANY input,
//! under ANY `/DecodeParms`, it must return without panicking, hanging, or allocating unboundedly
//! (DESIGN.md §3.4, §7).
//!
//! The parameters are fuzzed alongside the data because they *are* untrusted input: `/K` selects
//! between three different decoders (pure 1D, mixed, pure 2D), and `/Columns` and `/Rows` size the
//! buffers the coded data is decoded into.
//!
//! Run with: `cargo +nightly fuzz run ccitt` (needs `cargo install cargo-fuzz`).

use libfuzzer_sys::fuzz_target;
use pdf_cos::{Dictionary, Name, Object};
use pdf_filters::ccitt_fax_decode;

fuzz_target!(|input: (i8, u16, u16, u8, &[u8])| {
    let (k, columns, rows, flags, data) = input;
    // A 1 MiB output cap keeps each iteration cheap while still reaching the decode paths, and the
    // row geometry is bounded well below it: a fuzzer that spends its budget zeroing one 1728×64k
    // image explores far fewer Huffman paths than one that runs many small decodes. The common
    // real-world geometry (1728 columns, unbounded rows) is still covered — it is the default the
    // no-parameters call below takes.
    const CAP: usize = 1 << 20;

    let mut params = Dictionary::new();
    params.insert(Name::from("K"), Object::Integer(i64::from(k)));
    params.insert(
        Name::from("Columns"),
        Object::Integer(1 + i64::from(columns % 512)),
    );
    params.insert(Name::from("Rows"), Object::Integer(i64::from(rows % 128)));
    params.insert(Name::from("BlackIs1"), Object::Boolean(flags & 1 != 0));
    params.insert(
        Name::from("EncodedByteAlign"),
        Object::Boolean(flags & 2 != 0),
    );
    params.insert(Name::from("EndOfBlock"), Object::Boolean(flags & 4 != 0));
    let _ = ccitt_fax_decode(data, Some(&params), CAP);

    // No `/DecodeParms` at all: the defaults path (§7.4.6 Table 11 — G3 1D, 1728 columns,
    // unbounded rows), which is what a stream with a bare `/Filter` gets.
    let _ = ccitt_fax_decode(data, None, CAP);
});
