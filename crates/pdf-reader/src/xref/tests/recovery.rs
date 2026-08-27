//! Scan-based recovery: rebuilding a broken table by scanning (DESIGN.md §3).

use super::build_pdf;
use crate::parser::Limits;
use crate::xref::*;

use pdf_cos::{Name, Object, ObjectId};

#[test]
fn rebuild_rejects_non_digits_without_panicking() {
    // ISO 32000-2 §7.5: recovery scans hostile bytes for indirect-object headers. Non-digits
    // adjacent to a PDF header must be rejected, not evaluated as decimal digits (DESIGN.md §3.4).
    let input = b"%PDF-\r(((";
    assert!(XRef::rebuild(input).is_err());
}

#[test]
fn rebuild_caps_object_count() {
    // Anti-DoS (DESIGN.md §3.4): a file padded with far more object headers than the configured
    // cap must not materialise them all — the recovery scan is truncated to the bound.
    let mut input = b"%PDF-1.7\n".to_vec();
    input.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    input.extend_from_slice(b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n");
    input.extend_from_slice(b"3 0 obj\n<< /Type /Page /Parent 2 0 R >>\nendobj\n");
    for n in 4..=500u32 {
        input.extend_from_slice(format!("{n} 0 obj\n<< >>\nendobj\n").as_bytes());
    }

    let limits = Limits {
        max_objects: 10,
        ..Limits::default()
    };
    let xref = XRef::rebuild_with_limits(&input, limits).expect("recovers within the cap");
    assert!(
        xref.entries.len() <= 10,
        "object count not capped: {}",
        xref.entries.len()
    );
    assert!(
        xref.root().is_some(),
        "catalog still recovered within the cap"
    );
}

#[test]
fn rebuild_recovers_from_corrupt_startxref() {
    // The objects and trailer are intact but startxref points at garbage: scanning must still
    // find the objects at their real offsets (DESIGN.md §3: recovery is first-class).
    let mut pdf = build_pdf();
    // Corrupt the startxref offset (last number before %%EOF) to an absurd value.
    let at = rfind(&pdf, b"startxref").unwrap() + b"startxref".len();
    // Replace the offset line with a bogus one of the same shape.
    let line_end = pdf[at..].iter().position(|&b| b == b'\n').unwrap() + at;
    pdf.splice(at..line_end, b"\n999999".iter().copied());

    // Strict parse fails, but rebuild succeeds and locates the catalog and pages.
    assert!(XRef::parse(&pdf).is_err() || XRef::parse(&pdf).unwrap().fetch(&pdf, 1).is_err());
    let xref = XRef::rebuild(&pdf).unwrap();
    assert_eq!(xref.root(), Some(ObjectId::new(1, 0)));
    let Some(Object::Dictionary(catalog)) = xref.fetch(&pdf, 1).unwrap() else {
        panic!("recovered catalog should be a dictionary");
    };
    assert_eq!(
        catalog.get_name(&Name::from("Type")),
        Some(&Name::from("Catalog"))
    );
}

#[test]
fn rebuild_finds_catalog_without_any_trailer() {
    // No xref, no trailer, no startxref — just a header and objects. Recovery must scan the
    // objects and identify /Root from the /Type /Catalog object.
    let mut pdf = Vec::new();
    pdf.extend_from_slice(b"%PDF-1.4\n");
    pdf.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    pdf.extend_from_slice(b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n");
    pdf.extend_from_slice(b"3 0 obj\n<< /Type /Page /Parent 2 0 R >>\nendobj\n");

    let xref = XRef::rebuild(&pdf).unwrap();
    assert_eq!(xref.root(), Some(ObjectId::new(1, 0)));
    assert!(matches!(xref.entry(2), Some(XRefEntry::InUse { .. })));
    let Some(Object::Dictionary(pages)) = xref.fetch(&pdf, 2).unwrap() else {
        panic!("recovered pages should be a dictionary");
    };
    assert_eq!(pages.get_integer(&Name::from("Count")), Some(1));
}

#[test]
fn scan_ignores_endobj_and_substrings() {
    // `endobj` ends in "obj" and "object" contains it; neither is a header.
    let input = b"%PDF-1.7\n1 0 obj\n<< >>\nendobj\n% the object above\n";
    let headers = scan_object_headers(input);
    assert_eq!(headers.len(), 1);
    assert_eq!(headers[0].0, 1); // object number 1
}
