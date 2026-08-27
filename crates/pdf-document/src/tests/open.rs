//! Opening, recovery and page-tree navigation tests (§7.5, §7.7.2–§7.7.3).

use super::super::*;
use super::{assemble, classic_three_page_pdf};

#[test]
fn opens_and_counts_pages() {
    let pdf = classic_three_page_pdf();
    let doc = Document::open(pdf).unwrap();
    assert_eq!(doc.version(), Some(Version { major: 1, minor: 7 }));
    assert_eq!(doc.page_count().unwrap(), 3);
    assert_eq!(doc.pages().unwrap().len(), 3);
    // Each leaf really is a /Page.
    for page in doc.pages().unwrap() {
        assert_eq!(
            page.get_name(&Name::from("Type")),
            Some(&Name::from("Page"))
        );
    }
}

#[test]
fn opens_a_file_with_broken_xref_via_recovery() {
    // Stale xref offsets (as if the file were edited without rewriting the table): open() must
    // recover by scanning and still count pages (DESIGN.md §3).
    let mut pdf = classic_three_page_pdf();
    // Smash every xref offset to zero so the table is useless.
    let xref_at = pdf.windows(4).position(|w| w == b"xref").expect("has xref");
    for byte in &mut pdf[xref_at..] {
        if byte.is_ascii_digit() {
            *byte = b'0';
        }
    }
    let doc = Document::open(pdf).unwrap();
    assert_eq!(doc.page_count().unwrap(), 3);
    assert_eq!(doc.open_report().mode(), OpenMode::Recovered);
    assert_eq!(doc.open_report().diagnostics().len(), 1);
    assert_eq!(
        doc.open_report().diagnostics()[0].reason,
        RecoveryReason::XrefParseFailure
    );
    assert!(doc.open_report().diagnostics()[0].offset.is_some());
}

#[test]
fn strict_and_parse_failure_open_reports_are_discoverable() {
    // ISO 32000-1 §7.5.5: a valid startxref stays on the strict path.
    let strict = Document::open(classic_three_page_pdf()).unwrap();
    assert_eq!(strict.open_report().mode(), OpenMode::Strict);
    assert!(strict.open_report().diagnostics().is_empty());

    // Removing startxref forces the bounded §7.5 recovery scan and records the reader offset.
    let mut broken = classic_three_page_pdf();
    let startxref = broken
        .windows(9)
        .position(|window| window == b"startxref")
        .expect("fixture has startxref");
    broken.truncate(startxref);
    let recovered = Document::open(broken).unwrap();
    assert_eq!(recovered.open_report().mode(), OpenMode::Recovered);
    assert_eq!(
        recovered.open_report().diagnostics()[0].reason,
        RecoveryReason::XrefParseFailure
    );
    assert!(recovered.open_report().diagnostics()[0].offset.is_some());
}

#[test]
fn catalog_is_reachable() {
    let doc = Document::open(classic_three_page_pdf()).unwrap();
    let catalog = doc.catalog().unwrap();
    assert_eq!(
        catalog.get_name(&Name::from("Type")),
        Some(&Name::from("Catalog"))
    );
}

#[test]
fn nested_intermediate_nodes_are_walked() {
    // Page tree with an intermediate /Pages node between root and a leaf (§7.7.3).
    let mut buf = Vec::new();
    let mut off = [0usize; 6];
    buf.extend_from_slice(b"%PDF-1.4\n");
    let objects: [&[u8]; 5] = [
        b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n",
        b"2 0 obj\n<< /Type /Pages /Kids [3 0 R 5 0 R] /Count 2 >>\nendobj\n",
        b"3 0 obj\n<< /Type /Pages /Kids [4 0 R] /Count 1 >>\nendobj\n",
        b"4 0 obj\n<< /Type /Page /Parent 3 0 R >>\nendobj\n",
        b"5 0 obj\n<< /Type /Page /Parent 2 0 R >>\nendobj\n",
    ];
    for (i, body) in objects.iter().enumerate() {
        off[i + 1] = buf.len();
        buf.extend_from_slice(body);
    }
    let startxref = buf.len();
    buf.extend_from_slice(b"xref\n0 6\n0000000000 65535 f \n");
    for entry in &off[1..] {
        buf.extend_from_slice(format!("{entry:010} 00000 n \n").as_bytes());
    }
    buf.extend_from_slice(b"trailer\n<< /Size 6 /Root 1 0 R >>\n");
    buf.extend_from_slice(format!("startxref\n{startxref}\n%%EOF\n").as_bytes());

    let doc = Document::open(buf).unwrap();
    assert_eq!(doc.page_count().unwrap(), 2);
}

#[test]
fn cyclic_page_tree_does_not_loop() {
    // Root /Pages whose /Kids points back at itself: must terminate (anti-DoS, DESIGN.md §3.4).
    let mut buf = Vec::new();
    let mut off = [0usize; 3];
    buf.extend_from_slice(b"%PDF-1.7\n");
    let objects: [&[u8]; 2] = [
        b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n",
        b"2 0 obj\n<< /Type /Pages /Kids [2 0 R] /Count 1 >>\nendobj\n",
    ];
    for (i, body) in objects.iter().enumerate() {
        off[i + 1] = buf.len();
        buf.extend_from_slice(body);
    }
    let startxref = buf.len();
    buf.extend_from_slice(b"xref\n0 3\n0000000000 65535 f \n");
    for entry in &off[1..] {
        buf.extend_from_slice(format!("{entry:010} 00000 n \n").as_bytes());
    }
    buf.extend_from_slice(b"trailer\n<< /Size 3 /Root 1 0 R >>\n");
    buf.extend_from_slice(format!("startxref\n{startxref}\n%%EOF\n").as_bytes());

    let doc = Document::open(buf).unwrap();
    // The self-referential kid is visited once then skipped; it has no leaf pages.
    assert_eq!(doc.page_count().unwrap(), 0);
}

#[test]
fn get_and_resolve() {
    let doc = Document::open(classic_three_page_pdf()).unwrap();
    // get on an in-use object returns it; a missing object resolves to Null (§7.3.10).
    assert!(matches!(
        doc.get(ObjectId::new(1, 0)).unwrap(),
        Object::Dictionary(_)
    ));
    assert_eq!(doc.get(ObjectId::new(99, 0)).unwrap(), Object::Null);
    // resolve follows a reference, and returns non-references unchanged.
    let resolved = doc
        .resolve(&Object::Reference(ObjectId::new(1, 0)))
        .unwrap();
    assert!(matches!(resolved, Object::Dictionary(_)));
    assert_eq!(
        doc.resolve(&Object::Integer(5)).unwrap(),
        Object::Integer(5)
    );
}

#[test]
fn info_present_and_absent() {
    // Present: /Info points to a dictionary.
    let objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R >>".to_vec(),
        b"<< /Title (Hi) >>".to_vec(),
    ];
    let doc = Document::open(assemble(&objects, "/Info 4 0 R")).unwrap();
    assert!(doc.info().unwrap().is_some());

    // Absent: no /Info entry → Ok(None).
    let doc = Document::open(classic_three_page_pdf()).unwrap();
    assert_eq!(doc.info().unwrap(), None);
    assert_eq!(doc.version(), Some(Version { major: 1, minor: 7 }));
}

#[test]
fn malformed_kids_is_bad_page_tree() {
    // /Kids is not an array → BadPageTree rather than a silent miscount.
    let objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids 3 0 R /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R >>".to_vec(),
    ];
    let doc = Document::open(assemble(&objects, "")).unwrap();
    assert_eq!(doc.pages().unwrap_err(), DocError::BadPageTree);
}

#[test]
fn error_display_is_present() {
    // Exercise the Display impls of each DocError variant.
    for e in [
        DocError::MissingCatalog,
        DocError::NotADictionary("Catalog"),
        DocError::BadPageTree,
        DocError::ContentDecode,
        DocError::NeedsPassword,
    ] {
        assert!(!e.to_string().is_empty());
    }
}
