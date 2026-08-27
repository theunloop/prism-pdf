//! The facade re-export surface stays usable (§6.4 stable public API).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use prismpdf::{Document, Limits, Operation, Version, cos, extract_text, parse_content_stream};

#[test]
fn open_with_limits_bounds_a_hostile_object_flood() {
    // A file padded with thousands of object headers opens, but the configured object cap keeps it
    // bounded (anti-DoS, §3.4) — and it still finds its catalog and page.
    let mut bytes = b"%PDF-1.7\n".to_vec();
    bytes.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    bytes.extend_from_slice(b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n");
    bytes.extend_from_slice(b"3 0 obj\n<< /Type /Page /Parent 2 0 R >>\nendobj\n");
    for n in 4..=4000u32 {
        bytes.extend_from_slice(format!("{n} 0 obj\n<< >>\nendobj\n").as_bytes());
    }
    // Break startxref so recovery (the bounded scan) runs.
    bytes.extend_from_slice(b"startxref\n999999999\n%%EOF\n");

    let limits = Limits {
        max_objects: 64,
        ..Limits::default()
    };
    let doc = Document::open_with_limits(bytes, limits).unwrap();
    assert_eq!(doc.page_count().unwrap(), 1);
}

#[test]
fn re_exports_are_usable() {
    // Touch the re-exported surface so the facade's public API is exercised.
    let obj = cos::Object::Integer(1);
    assert_eq!(obj.as_integer(), Some(1));
    let ops = parse_content_stream(b"(hi) Tj");
    assert_eq!(extract_text(&ops), "hi");
    assert!(matches!(
        ops.last().map(|o: &Operation| o.operator.as_str()),
        Some("Tj")
    ));
    let _v: Option<Version> = None;
}
