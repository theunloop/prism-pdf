//! Outline (bookmark) reading tests (§12.3.3).

use super::super::*;
use super::assemble;

#[test]
fn reads_nested_outline_with_destinations() {
    // Two top-level bookmarks; the first has one child. Destinations point at page 0 and page 1.
    let objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R /Outlines 4 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R 8 0 R] /Count 2 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R >>".to_vec(),
        b"<< /Type /Outlines /First 5 0 R /Last 7 0 R /Count 2 >>".to_vec(),
        b"<< /Title (Chapter 1) /Parent 4 0 R /Dest [3 0 R /Fit] /First 6 0 R /Next 7 0 R >>"
            .to_vec(),
        b"<< /Title (Section 1.1) /Parent 5 0 R /Dest [8 0 R /Fit] >>".to_vec(),
        b"<< /Title (Chapter 2) /Parent 4 0 R /Dest [8 0 R /Fit] /Prev 5 0 R >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R >>".to_vec(),
    ];
    let doc = Document::open(assemble(&objects, "")).unwrap();
    let outline = doc.outline().unwrap();

    assert_eq!(outline.len(), 2);
    assert_eq!(outline[0].title, "Chapter 1");
    assert_eq!(outline[0].dest_page, Some(0));
    assert_eq!(outline[0].children.len(), 1);
    assert_eq!(outline[0].children[0].title, "Section 1.1");
    assert_eq!(outline[0].children[0].dest_page, Some(1));

    assert_eq!(outline[1].title, "Chapter 2");
    assert_eq!(outline[1].dest_page, Some(1));
    assert!(outline[1].children.is_empty());
}

#[test]
fn no_outline_yields_empty() {
    let objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R >>".to_vec(),
    ];
    let doc = Document::open(assemble(&objects, "")).unwrap();
    assert!(doc.outline().unwrap().is_empty());
}

#[test]
fn outline_round_trips_through_the_builder() {
    // Authoring writes an outline; reading recovers its titles and page targets.
    let mut builder = Builder::new();
    builder.add_page(PageSpec::new(Vec::new()));
    builder.add_page(PageSpec::new(Vec::new()));
    builder.outline("Intro", 0);
    builder.outline("Body", 1);
    let doc = Document::open(builder.build()).unwrap();

    let outline = doc.outline().unwrap();
    let titles: Vec<&str> = outline.iter().map(|i| i.title.as_str()).collect();
    assert_eq!(titles, ["Intro", "Body"]);
    assert_eq!(outline[0].dest_page, Some(0));
    assert_eq!(outline[1].dest_page, Some(1));
}
