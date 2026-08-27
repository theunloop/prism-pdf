#![no_main]
//! Fuzz target for the whole read path (EPIC 2/4/6/7, ISO 32000 §7–§9).
//!
//! [`Document::open`] is the top-level untrusted entry point: header parse (§7.5.2), classic/stream
//! xref (§7.5.4/7.5.8), the recovery rebuild when those fail (§7.5, scan), then object resolution
//! and the higher-level extractors. On ANY input the whole stack must return without panicking,
//! hanging, or allocating unboundedly (DESIGN.md §3.4, §7) — this exercises the anti-DoS limits
//! (nesting, object-stream `/N`, cycle guards) and every parser reachable from a page: content,
//! fonts/CMaps (§9.7/§9.10), images incl. JBIG2 (§7.4.7), and the name-tree/attachment reader.
//!
//! Run with: `cargo +nightly fuzz run document` (needs `cargo install cargo-fuzz`).

use libfuzzer_sys::fuzz_target;
use prismpdf::{Document, document_fonts, page_images, page_text, page_text_positioned};

fuzz_target!(|data: &[u8]| {
    let Ok(doc) = Document::open(data.to_vec()) else {
        return;
    };

    // Walk a bounded number of pages through every extractor — enough to reach the deep parsers
    // without letting one input dominate the campaign.
    if let Ok(count) = doc.page_count() {
        for index in 0..count.min(16) {
            let _ = page_text(&doc, index);
            let _ = page_text_positioned(&doc, index);
            let _ = page_images(&doc, index);
        }
    }

    let _ = document_fonts(&doc);
    let _ = doc.attachments();
});
