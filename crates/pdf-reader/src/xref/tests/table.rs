//! Classic cross-reference table, header, trailer, and `startxref` (§7.5.2/§7.5.4/§7.5.5).

use super::build_pdf;
use crate::error::ErrorKind;
use crate::xref::*;

use pdf_cos::{Name, Object, ObjectId};

#[test]
fn reads_header_table_and_trailer() {
    // §7.5.2 header, §7.5.4 table, §7.5.5 trailer.
    let pdf = build_pdf();
    let xref = XRef::parse(&pdf).unwrap();
    assert_eq!(xref.version, Some(Version { major: 1, minor: 7 }));
    assert_eq!(xref.size(), Some(4));
    assert_eq!(xref.root(), Some(ObjectId::new(1, 0)));
    assert_eq!(xref.entries.len(), 4);
    assert!(matches!(xref.entry(0), Some(XRefEntry::Free { .. })));
    assert!(matches!(xref.entry(1), Some(XRefEntry::InUse { .. })));
}

#[test]
fn fetches_objects_via_the_table() {
    // §7.5.4 + §7.3.10: locate the catalog and the page-tree root by number.
    let pdf = build_pdf();
    let xref = XRef::parse(&pdf).unwrap();

    let root = xref.root().unwrap();
    let Some(Object::Dictionary(catalog)) = xref.fetch(&pdf, root.number).unwrap() else {
        panic!("catalog should be a dictionary");
    };
    assert_eq!(
        catalog.get_name(&Name::from("Type")),
        Some(&Name::from("Catalog"))
    );

    let pages_ref = catalog.get_reference(&Name::from("Pages")).unwrap();
    let Some(Object::Dictionary(pages)) = xref.fetch(&pdf, pages_ref.number).unwrap() else {
        panic!("page tree root should be a dictionary");
    };
    assert_eq!(pages.get_integer(&Name::from("Count")), Some(1));

    // A free object resolves to nothing.
    assert_eq!(xref.fetch(&pdf, 0).unwrap(), None);
}

#[test]
fn missing_startxref_errors_without_panic() {
    // DESIGN.md §3.4: a file with no startxref is rejected cleanly.
    assert_eq!(
        XRef::parse(b"%PDF-1.7\njunk").unwrap_err().kind(),
        ErrorKind::MissingStartxref
    );
}

#[test]
fn prev_chain_cycle_terminates() {
    // A /Prev pointing back at itself must not loop (anti-DoS, DESIGN.md §3.4).
    let mut pdf = Vec::new();
    pdf.extend_from_slice(b"%PDF-1.7\n");
    let xref_at = pdf.len();
    // /Prev points at this same section's offset -> immediate cycle.
    pdf.extend_from_slice(b"xref\n0 1\n0000000000 65535 f \ntrailer\n");
    pdf.extend_from_slice(format!("<< /Size 1 /Root 1 0 R /Prev {xref_at} >>\n").as_bytes());
    pdf.extend_from_slice(format!("startxref\n{xref_at}\n%%EOF\n").as_bytes());
    // Should return (cycle broken), not hang.
    let xref = XRef::parse(&pdf).unwrap();
    assert_eq!(xref.size(), Some(1));
}

#[test]
fn parse_header_variants() {
    assert_eq!(
        parse_header(b"%PDF-1.7\n"),
        Some(Version { major: 1, minor: 7 })
    );
    assert_eq!(
        parse_header(b"%PDF-2.0 rest"),
        Some(Version { major: 2, minor: 0 })
    );
    // A few junk bytes before the marker are tolerated.
    assert_eq!(
        parse_header(b"\x00\x00%PDF-1.4"),
        Some(Version { major: 1, minor: 4 })
    );
    // Malformed: missing version, missing dot, non-digit, or no marker at all.
    assert_eq!(parse_header(b"%PDF-"), None);
    assert_eq!(parse_header(b"%PDF-1x"), None);
    assert_eq!(parse_header(b"%PDF-a.b"), None);
    assert_eq!(parse_header(b"not a pdf"), None);
}

#[test]
fn find_helpers_handle_edges() {
    assert_eq!(find(b"abcdef", 0, b"cd"), Some(2));
    assert_eq!(find(b"abc", 0, b""), None);
    assert_eq!(find(b"abc", 9, b"a"), None);
    assert_eq!(rfind(b"a.a.a", b"a"), Some(4));
    assert_eq!(rfind(b"ab", b"abc"), None);
    assert_eq!(rfind(b"ab", b""), None);
}

#[test]
fn negative_startxref_is_rejected() {
    assert_eq!(
        find_startxref(b"junk startxref\n-5\n%%EOF")
            .unwrap_err()
            .kind(),
        ErrorKind::MissingStartxref
    );
    assert_eq!(
        find_startxref(b"no marker here").unwrap_err().kind(),
        ErrorKind::MissingStartxref
    );
}

#[test]
fn classic_table_rejects_implausible_count() {
    // A subsection claiming far more entries than the file has bytes is malformed.
    let pdf = b"%PDF-1.7\nxref\n0 999999999999\ntrailer\n<< /Root 1 0 R >>\nstartxref\n9\n%%EOF";
    assert!(XRef::parse(pdf).is_err());
}

#[test]
fn free_entry_fetches_to_none() {
    let pdf = build_pdf();
    let xref = XRef::parse(&pdf).unwrap();
    // Object 0 is the free-list head.
    assert!(matches!(xref.entry(0), Some(XRefEntry::Free { .. })));
    assert_eq!(xref.fetch(&pdf, 0).unwrap(), None);
    // An object number with no entry at all also fetches to None.
    assert_eq!(xref.fetch(&pdf, 999).unwrap(), None);
    assert_eq!(xref.entry(999), None);
}
