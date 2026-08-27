//! Authoring documents from scratch and via the high-level flow layout, then reading them back
//! (§7 write path / §9 text). Each test round-trips: build → open → extract.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use prismpdf::cos::{self, Name, Object};
use prismpdf::{
    Align, Builder, Content, Document, Flow, ListStyle, PageSpec, PageStyle, StdFont, Table,
    TextBlock, document_text, draw_text_block, page_text, wrap_text,
};

#[test]
fn author_a_page_from_scratch_round_trips() {
    // Draw a page with Content, assemble it with Builder, then reopen and read the text back.
    let mut content = Content::new();
    content
        .begin_text()
        .set_font("F1", 24.0)
        .text_move(72.0, 700.0)
        .show_str("Hello from Prism PDF")
        .end_text();
    let pdf = Builder::new()
        .add_page(PageSpec::new(content.into_bytes()).standard_font("F1", StdFont::Helvetica))
        .build();

    let doc = Document::open(pdf).unwrap();
    assert_eq!(doc.page_count().unwrap(), 1);
    assert_eq!(
        page_text(&doc, 0).unwrap().as_deref(),
        Some("Hello from Prism PDF")
    );
}

#[test]
fn author_a_wrapped_paragraph_round_trips() {
    let font = "Helvetica";
    let text = "The quick brown fox jumps over the lazy dog several times in a row";
    let lines = wrap_text(font, text, 12.0, 140.0);
    assert!(lines.len() > 1);

    let mut content = Content::new();
    content
        .begin_text()
        .set_font("F1", 12.0)
        .set_leading(14.0)
        .text_move(72.0, 700.0);
    for line in &lines {
        content.show_str(line).next_line();
    }
    content.end_text();
    let pdf = Builder::new()
        .add_page(PageSpec::new(content.into_bytes()).standard_font("F1", StdFont::Helvetica))
        .build();

    let doc = Document::open(pdf).unwrap();
    let extracted = page_text(&doc, 0).unwrap().unwrap();
    for word in text.split_whitespace() {
        assert!(
            extracted.contains(word),
            "missing {word:?} in {extracted:?}"
        );
    }
}

#[test]
fn justify_widens_spaces_and_flows_and_round_trips() {
    let block = TextBlock {
        font_resource: "F1",
        base_font: "Helvetica",
        size: 12.0,
        leading: 14.0,
        align: Align::Justify,
    };
    let text = "alpha beta gamma delta epsilon zeta eta theta iota kappa";
    let mut content = Content::new();
    let end_y = draw_text_block(&mut content, &block, 72.0, 700.0, 120.0, text);
    let bytes = content.into_bytes();
    let dump = String::from_utf8(bytes.clone()).unwrap();

    // A justified (non-last) line emitted a positive word spacing.
    assert!(
        dump.lines().any(|l| l.ends_with(" Tw") && l != "0 Tw"),
        "no positive Tw emitted"
    );
    // The block flowed downward.
    assert!(end_y < 700.0);

    // Round-trips: build a page and recover every word.
    let pdf = Builder::new()
        .add_page(PageSpec::new(bytes).standard_font("F1", StdFont::Helvetica))
        .build();
    let doc = Document::open(pdf).unwrap();
    let extracted = page_text(&doc, 0).unwrap().unwrap();
    for word in text.split_whitespace() {
        assert!(extracted.contains(word), "missing {word:?}");
    }
}

#[test]
fn flow_spans_multiple_pages_and_round_trips() {
    // A small page so the text overflows onto several pages via automatic page breaks.
    let style = PageStyle {
        size: [300.0, 200.0],
        margins: [28.0; 4],
    };
    let block = TextBlock {
        font_resource: "F1",
        base_font: "Helvetica",
        size: 12.0,
        leading: 14.0,
        align: Align::Left,
    };
    let text = (0..40)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n");

    let mut flow = Flow::new(style, &[("F1", StdFont::Helvetica)]);
    flow.text(&block, &text);
    let pdf = flow.build();

    let doc = Document::open(pdf).unwrap();
    assert!(
        doc.page_count().unwrap() >= 3,
        "pages: {}",
        doc.page_count().unwrap()
    );
    let all = document_text(&doc).unwrap();
    assert!(all.contains("line 0") && all.contains("line 39"), "{all:?}");
}

#[test]
fn flow_bookmarks_point_at_their_pages() {
    let block = TextBlock {
        font_resource: "F1",
        base_font: "Helvetica",
        size: 12.0,
        leading: 14.0,
        align: Align::Left,
    };
    let mut flow = Flow::new(PageStyle::default(), &[("F1", StdFont::Helvetica)]);
    flow.bookmark("Intro").text(&block, "intro text");
    flow.page_break();
    flow.bookmark("Details").text(&block, "details text");
    let pdf = flow.build();

    let doc = Document::open(pdf).unwrap();
    let catalog = doc.catalog().unwrap();
    let Some(cos::Object::Reference(root_ref)) = catalog.get(&cos::Name::from("Outlines")) else {
        panic!("no /Outlines: {catalog:?}");
    };
    let cos::Object::Dictionary(root) = doc.get(*root_ref).unwrap() else {
        panic!("root not a dict");
    };
    assert_eq!(
        root.get(&cos::Name::from("Count")),
        Some(&cos::Object::Integer(2))
    );
    // Walk First → Next, collecting titles in order.
    let mut titles = Vec::new();
    let mut cur = root.get(&cos::Name::from("First")).cloned();
    while let Some(cos::Object::Reference(id)) = cur {
        let cos::Object::Dictionary(item) = doc.get(id).unwrap() else {
            break;
        };
        if let Some(cos::Object::String(s)) = item.get(&cos::Name::from("Title")) {
            titles.push(String::from_utf8_lossy(s.as_bytes()).into_owned());
        }
        cur = item.get(&cos::Name::from("Next")).cloned();
    }
    assert_eq!(titles, vec!["Intro".to_string(), "Details".to_string()]);
}

#[test]
fn header_and_footer_show_page_numbers() {
    let block = TextBlock {
        font_resource: "F1",
        base_font: "Helvetica",
        size: 10.0,
        leading: 12.0,
        align: Align::Center,
    };
    // A short page so the body overflows onto a second page.
    let style = PageStyle {
        size: [300.0, 200.0],
        margins: [28.0; 4],
    };
    let mut flow = Flow::new(style, &[("F1", StdFont::Helvetica)]);
    flow.header(&block, "My Report")
        .footer(&block, "Page {page} of {pages}");
    let body = (0..40)
        .map(|i| format!("body line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    flow.text(&block, &body);
    let pdf = flow.build();

    let doc = Document::open(pdf).unwrap();
    let total = doc.page_count().unwrap();
    assert!(total >= 2);
    let p0 = page_text(&doc, 0).unwrap().unwrap();
    let last = page_text(&doc, total - 1).unwrap().unwrap();
    assert!(p0.contains("My Report"), "header missing: {p0:?}");
    assert!(p0.contains(&format!("Page 1 of {total}")), "{p0:?}");
    assert!(
        last.contains(&format!("Page {total} of {total}")),
        "{last:?}"
    );
}

#[test]
fn author_list_and_non_ascii_round_trip() {
    let block = TextBlock {
        font_resource: "F1",
        base_font: "Helvetica",
        size: 12.0,
        leading: 16.0,
        align: Align::Left,
    };
    let mut flow = Flow::new(PageStyle::default(), &[("F1", StdFont::Helvetica)]);
    flow.text(&block, "Café résumé — €5") // non-ASCII via WinAnsi
        .list(&block, &["Apples", "Oranges"], ListStyle::Bullet);
    let pdf = flow.build();

    let doc = Document::open(pdf).unwrap();
    let text = page_text(&doc, 0).unwrap().unwrap();
    // Non-ASCII Latin survives WinAnsi encode → decode.
    assert!(text.contains("Café résumé"), "{text:?}");
    assert!(text.contains("€5"));
    // List items and bullets are present.
    assert!(text.contains("Apples") && text.contains("Oranges"));
    assert!(text.contains('•'), "bullet missing: {text:?}");
}

#[test]
fn flow_sets_document_metadata() {
    let block = TextBlock {
        font_resource: "F1",
        base_font: "Helvetica",
        size: 12.0,
        leading: 14.0,
        align: Align::Left,
    };
    let mut flow = Flow::new(PageStyle::default(), &[("F1", StdFont::Helvetica)]);
    flow.title("Hello Doc")
        .author("Prism PDF")
        .text(&block, "body");
    let pdf = flow.build();

    let doc = Document::open(pdf).unwrap();
    let info = doc.info().unwrap().unwrap();
    let entry = |key: &str| match info.get(&Name::from(key)) {
        Some(Object::String(s)) => s.as_bytes().to_vec(),
        _ => Vec::new(),
    };
    assert_eq!(entry("Title"), b"Hello Doc");
    assert_eq!(entry("Author"), b"Prism PDF");
}

#[test]
fn author_a_table_round_trips() {
    let mut flow = Flow::new(PageStyle::default(), &[("F1", StdFont::Helvetica)]);
    let table = Table::new(vec![1.0, 2.0])
        .font("F1", "Helvetica")
        .header_row(true)
        .row(["Name", "Description"])
        .row(["Widget", "a small gadget"])
        .row(["Gizmo", "a clever device"]);
    flow.table(&table);
    let pdf = flow.build();

    let doc = Document::open(pdf).unwrap();
    let text = page_text(&doc, 0).unwrap().unwrap();
    for cell in ["Name", "Description", "Widget", "gadget", "Gizmo", "clever"] {
        assert!(text.contains(cell), "missing {cell:?} in {text:?}");
    }
}

#[test]
fn tagged_flow_round_trips_with_structure_and_text() {
    // A tagged document (§14.7/§14.8): the structure tree is emitted AND the text still extracts
    // through the marked content (BDC/EMC must not disturb text extraction).
    let block = TextBlock {
        font_resource: "F1",
        base_font: "Helvetica",
        size: 12.0,
        leading: 16.0,
        align: Align::Left,
    };
    let mut flow = Flow::new(PageStyle::letter(72.0), &[("F1", StdFont::Helvetica)]);
    flow.tagged("en-US");
    flow.text(&block, "First paragraph here.\nSecond paragraph here.");
    flow.list(&block, &["alpha item", "beta item"], ListStyle::Bullet);
    let pdf = flow.build();

    let doc = Document::open(pdf).unwrap();

    // Catalog: tagged + a structure tree.
    let catalog = doc.catalog().unwrap();
    assert!(
        matches!(
            catalog.get(&Name::from("MarkInfo")),
            Some(Object::Dictionary(_))
        ),
        "no /MarkInfo"
    );
    let Some(Object::Reference(root_ref)) = catalog.get(&Name::from("StructTreeRoot")) else {
        panic!("no /StructTreeRoot");
    };
    let Object::Dictionary(root) = doc.get(*root_ref).unwrap() else {
        panic!("struct root not a dict");
    };
    assert_eq!(
        root.get(&Name::from("Type")),
        Some(&Object::Name(Name::from("StructTreeRoot")))
    );

    // Text still extracts (the marked content wrapping is transparent to extraction).
    let text = document_text(&doc).unwrap();
    for fragment in [
        "First paragraph",
        "Second paragraph",
        "alpha item",
        "beta item",
    ] {
        assert!(text.contains(fragment), "missing {fragment:?} in {text:?}");
    }
}
