//! Page annotation reading (§12.5): subtype, rectangle, contents, and link URIs.

use super::super::*;
use super::assemble;

#[test]
fn reads_text_note_and_link_annotations() {
    let objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Annots [4 0 R 5 0 R] >>".to_vec(),
        // A text note with non-ASCII contents.
        b"<< /Type /Annot /Subtype /Text /Rect [10 20 30 40] /Contents (caf\xe9 note) >>".to_vec(),
        // A link whose action opens an external URI.
        b"<< /Type /Annot /Subtype /Link /Rect [0 0 100 12] \
            /A << /S /URI /URI (https://example.com/) >> >>"
            .to_vec(),
    ];
    let doc = Document::open(assemble(&objects, "")).unwrap();
    let page = doc.pages().unwrap().remove(0);
    let annots = doc.annotations(&page).unwrap();

    assert_eq!(annots.len(), 2);

    let note = &annots[0];
    assert_eq!(note.subtype, "Text");
    assert_eq!(note.rect, [10.0, 20.0, 30.0, 40.0]);
    assert_eq!(note.contents.as_deref(), Some("café note"));
    assert_eq!(note.uri, None);

    let link = &annots[1];
    assert_eq!(link.subtype, "Link");
    assert_eq!(link.uri.as_deref(), Some("https://example.com/"));
    assert_eq!(link.contents, None);
}

#[test]
fn resolves_explicit_and_goto_link_destinations() {
    // Two pages; page 0 has two links targeting page 1 — one via /Dest, one via a GoTo action.
    let objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Annots [5 0 R 6 0 R] >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R >>".to_vec(),
        b"<< /Type /Annot /Subtype /Link /Rect [0 0 1 1] /Dest [4 0 R /Fit] >>".to_vec(),
        b"<< /Type /Annot /Subtype /Link /Rect [0 0 1 1] \
            /A << /S /GoTo /D [4 0 R /Fit] >> >>"
            .to_vec(),
    ];
    let doc = Document::open(assemble(&objects, "")).unwrap();
    let page0 = doc.pages().unwrap().remove(0);
    let annots = doc.annotations(&page0).unwrap();
    assert_eq!(annots[0].dest_page, Some(1)); // explicit /Dest
    assert_eq!(annots[1].dest_page, Some(1)); // GoTo action
}

#[test]
fn resolves_named_destination_via_name_tree() {
    // A string-named destination resolved through the catalog /Names /Dests name tree (§12.3.2.3).
    let objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R /Names << /Dests 7 0 R >> >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Annots [5 0 R] >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R >>".to_vec(),
        b"<< /Type /Annot /Subtype /Link /Rect [0 0 1 1] /Dest (chapter2) >>".to_vec(),
        b"<< >>".to_vec(), // filler so the dest array's page ref (4 0 R) is page 1
        // The Dests name tree: key "chapter2" → a /D-holder pointing at page 1.
        b"<< /Names [(chapter2) << /D [4 0 R /Fit] >>] >>".to_vec(),
    ];
    let doc = Document::open(assemble(&objects, "")).unwrap();
    let page0 = doc.pages().unwrap().remove(0);
    let annots = doc.annotations(&page0).unwrap();
    assert_eq!(annots[0].dest_page, Some(1));
}

#[test]
fn page_without_annots_yields_none() {
    let objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R >>".to_vec(),
    ];
    let doc = Document::open(assemble(&objects, "")).unwrap();
    let page = doc.pages().unwrap().remove(0);
    assert!(doc.annotations(&page).unwrap().is_empty());
}

#[test]
fn malformed_annotation_entries_are_skipped() {
    // /Annots holds a non-dictionary and a dangling reference among a valid annotation; only the
    // valid one is reported, and nothing panics (DESIGN.md §3.4).
    let objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Annots [99 0 R (junk) 4 0 R] >>".to_vec(),
        b"<< /Type /Annot /Subtype /Highlight /Rect [1 2 3 4] >>".to_vec(),
    ];
    let doc = Document::open(assemble(&objects, "")).unwrap();
    let page = doc.pages().unwrap().remove(0);
    let annots = doc.annotations(&page).unwrap();
    assert_eq!(annots.len(), 1);
    assert_eq!(annots[0].subtype, "Highlight");
}
