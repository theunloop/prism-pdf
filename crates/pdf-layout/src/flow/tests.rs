use super::*;
use crate::table::Table;
use crate::text::Align;
use pdf_document::StructKid;

fn block() -> TextBlock<'static> {
    TextBlock {
        font_resource: "F1",
        base_font: "Helvetica",
        size: 12.0,
        leading: 14.0,
        align: Align::Left,
    }
}

#[test]
fn short_text_is_one_page() {
    let mut flow = Flow::new(PageStyle::letter(72.0), &[("F1", StdFont::Helvetica)]);
    flow.text(&block(), "just a line");
    assert_eq!(flow.page_count(), 1);
}

#[test]
fn long_text_breaks_onto_multiple_pages() {
    // A short page so a modest amount of text overflows: 200pt tall, 144pt of usable height,
    // 14pt leading ⇒ ~10 lines/page.
    let style = PageStyle {
        size: [300.0, 200.0],
        margins: [28.0; 4],
    };
    let mut flow = Flow::new(style, &[("F1", StdFont::Helvetica)]);
    let para = (0..60)
        .map(|i| format!("line number {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    flow.text(&block(), &para);
    assert!(flow.page_count() >= 4, "pages: {}", flow.page_count());
}

#[test]
fn explicit_page_break_and_space() {
    let mut flow = Flow::new(PageStyle::default(), &[("F1", StdFont::Helvetica)]);
    flow.text(&block(), "page one");
    let before = flow.cursor_y();
    flow.space(20.0);
    assert!((flow.cursor_y() - (before - 20.0)).abs() < 1e-6);
    flow.page_break();
    flow.text(&block(), "page two");
    assert_eq!(flow.page_count(), 2);
}

#[test]
fn empty_flow_builds_one_page() {
    let pdf = Flow::new(PageStyle::default(), &[]).build();
    assert!(pdf.starts_with(b"%PDF-"));
}

#[test]
fn tagging_is_opt_in() {
    // Without `tagged`, no marked content or structure leaks into the output.
    let mut flow = Flow::new(PageStyle::default(), &[("F1", StdFont::Helvetica)]);
    flow.text(&block(), "plain text");
    let pdf = flow.build();
    let s = String::from_utf8_lossy(&pdf);
    assert!(!s.contains("BDC"));
    assert!(!s.contains("/StructTreeRoot"));
    assert!(!s.contains("/MarkInfo"));
}

#[test]
fn tagged_flow_emits_a_structure_tree() {
    let mut flow = Flow::new(PageStyle::default(), &[("F1", StdFont::Helvetica)]);
    flow.tagged("en-US");
    flow.text(&block(), "First paragraph.\nSecond paragraph.");
    flow.list(&block(), &["alpha", "beta"], ListStyle::Bullet);
    let pdf = flow.build();
    let s = String::from_utf8_lossy(&pdf);

    // Catalog marks the document tagged and points to a structure tree with a Document root and
    // P elements; the page carries a parent-tree key; content is wrapped in marked content with
    // per-page MCIDs starting at 0.
    assert!(s.contains("/MarkInfo"));
    assert!(s.contains("/Marked true"));
    assert!(s.contains("/StructTreeRoot"));
    assert!(s.contains("/S /Document"));
    assert!(s.contains("/S /P"));
    assert!(s.contains("/Lang (en-US)"));
    assert!(s.contains("/StructParents"));
    assert!(s.contains("/P <</MCID 0>> BDC"));
    // The document still reopens as a valid PDF.
    assert_eq!(
        pdf_document::Document::open(pdf.clone())
            .unwrap()
            .page_count()
            .unwrap(),
        1
    );
}

#[test]
fn heading_is_tagged_as_h_level() {
    let mut flow = Flow::new(PageStyle::default(), &[("F1", StdFont::Helvetica)]);
    flow.tagged("en");
    flow.heading(1, &block(), "Title");
    flow.text(&block(), "Body text.");
    let s = String::from_utf8_lossy(&flow.build()).into_owned();
    assert!(s.contains("/S /H1"), "heading element");
    assert!(
        s.contains("/H1 <</MCID"),
        "heading marked content uses the H1 tag"
    );
    assert!(s.contains("/S /P"), "paragraph element");
}

#[test]
fn tagged_list_emits_l_li_lbl_lbody() {
    let mut flow = Flow::new(PageStyle::default(), &[("F1", StdFont::Helvetica)]);
    flow.tagged("en");
    flow.list(&block(), &["first", "second"], ListStyle::Numbered);
    let s = String::from_utf8_lossy(&flow.build()).into_owned();
    for tag in ["/S /L", "/S /LI", "/S /Lbl", "/S /LBody"] {
        assert!(s.contains(tag), "missing {tag}");
    }
    // Two items → two LI elements.
    assert_eq!(s.matches("/S /LI").count(), 2);
    // The numbered list carries /ListNumbering /Decimal under the List owner (PDF/UA-1 §7.6).
    let l = flow_list_elem(ListStyle::Numbered);
    assert_eq!(l.attrs.len(), 1);
    assert_eq!(l.attrs[0].owner, "List");
    assert_eq!(
        l.attrs[0].entries,
        vec![(
            "ListNumbering".to_string(),
            pdf_document::AttrValue::Name("Decimal".to_string())
        )]
    );
    // A bulleted list is /Disc.
    let l = flow_list_elem(ListStyle::Bullet);
    assert_eq!(
        l.attrs[0].entries[0].1,
        pdf_document::AttrValue::Name("Disc".to_string())
    );
}

/// The `L` element a one-item tagged list of `style` produces.
fn flow_list_elem(style: ListStyle) -> StructElem {
    let mut flow = Flow::new(PageStyle::default(), &[("F1", StdFont::Helvetica)]);
    flow.tagged("en");
    flow.list(&block(), &["only"], style);
    flow.structure
        .iter()
        .find(|e| e.tag == "L")
        .expect("an L element")
        .clone()
}

#[test]
fn tagged_table_emits_table_tr_th_td() {
    let mut flow = Flow::new(PageStyle::default(), &[("F1", StdFont::Helvetica)]);
    flow.tagged("en");
    let table = Table::new(vec![1.0, 1.0])
        .font("F1", "Helvetica")
        .header_row(true)
        .row(["Name", "Qty"])
        .row(["Foo", "3"]);
    flow.table(&table);
    let s = String::from_utf8_lossy(&flow.build()).into_owned();
    for tag in ["/S /Table", "/S /TR", "/S /TH", "/S /TD"] {
        assert!(s.contains(tag), "missing {tag}");
    }
    // Header row → 2 TH; body row → 2 TD.
    assert_eq!(s.matches("/S /TH").count(), 2);
    assert_eq!(s.matches("/S /TD").count(), 2);
    // Each TH carries /Scope /Column (§14.8.5.4 — PDF/UA-1 §7.5); TDs carry no attributes.
    assert_eq!(s.matches("/Scope /Column").count(), 2);
}

#[test]
fn tagged_figure_carries_alt_text() {
    use crate::image::Image;
    let mut flow = Flow::new(PageStyle::default(), &[]);
    flow.tagged("en");
    let image = Image::from_rgb(2, 2, vec![0u8; 12]).unwrap();
    flow.figure(&image, 100.0, 100.0, "a red square");
    let s = String::from_utf8_lossy(&flow.build()).into_owned();
    assert!(s.contains("/S /Figure"), "figure element");
    assert!(s.contains("(a red square)"), "alt text present");
    // A plain image is an artifact, not a figure.
    let mut flow2 = Flow::new(PageStyle::default(), &[]);
    flow2.tagged("en");
    flow2.image(&image, 100.0, 100.0);
    let s2 = String::from_utf8_lossy(&flow2.build()).into_owned();
    assert!(!s2.contains("/S /Figure"));
    assert!(s2.contains("/Artifact BMC"));
}

#[test]
fn figure_caption_nests_in_the_figure_element() {
    use crate::image::Image;
    let mut flow = Flow::new(PageStyle::default(), &[("F1", StdFont::Helvetica)]);
    flow.tagged("en");
    let image = Image::from_rgb(2, 2, vec![0u8; 12]).unwrap();
    flow.figure_with_caption(
        &image,
        100.0,
        100.0,
        "a chart",
        &block(),
        "Figure 1 — sales",
    );
    // The Figure element holds the image content plus a nested Caption element (UA-1 §7.3).
    let fig = flow
        .structure
        .iter()
        .find(|e| e.tag == "Figure")
        .expect("a Figure element");
    assert_eq!(fig.alt.as_deref(), Some("a chart"));
    let caption = fig
        .kids
        .iter()
        .find_map(|k| match k {
            StructKid::Child(c) if c.tag == "Caption" => Some(c),
            _ => None,
        })
        .expect("a nested Caption");
    assert!(!caption.kids.is_empty(), "caption references its content");
    let s = String::from_utf8_lossy(&flow.build()).into_owned();
    assert!(s.contains("/S /Caption"), "Caption element serialised");
}

#[test]
fn fenote_and_title_are_pdf2_namespaced() {
    let mut flow = Flow::new(PageStyle::default(), &[("F1", StdFont::Helvetica)]);
    flow.tagged("en");
    flow.title_element(&block(), "The Document Title");
    flow.text(&block(), "Cited claim [1].");
    if let Some(p) = flow.last_element_mut() {
        p.id = Some("cite-1".into());
        p.refs.push("fn-1".into());
    }
    flow.fenote(&block(), "[1] Footnote body.", "fn-1", &["cite-1"]);

    let title = flow
        .structure
        .iter()
        .find(|e| e.tag == "Title")
        .expect("a Title element");
    assert_eq!(title.ns.as_deref(), Some(PDF2_STRUCT_NS));

    let note = flow
        .structure
        .iter()
        .find(|e| e.tag == "FENote")
        .expect("a FENote element");
    assert_eq!(note.ns.as_deref(), Some(PDF2_STRUCT_NS));
    assert_eq!(note.id.as_deref(), Some("fn-1"));
    assert_eq!(note.refs, vec!["cite-1".to_string()]);

    let cite = flow
        .structure
        .iter()
        .find(|e| e.id.as_deref() == Some("cite-1"))
        .expect("the citing paragraph");
    assert_eq!(cite.refs, vec!["fn-1".to_string()]);

    // Serialised: both 2.0 types present, /Ref emitted, header promoted to 2.0.
    let bytes = flow.build();
    assert!(
        bytes.starts_with(b"%PDF-2.0"),
        "namespace/Ref promote to 2.0"
    );
    let s = String::from_utf8_lossy(&bytes).into_owned();
    assert!(s.contains("/S /FENote"));
    assert!(s.contains("/S /Title"));
    assert!(s.contains("/Ref"));
}

#[test]
fn formula_carries_actual_text() {
    let mut flow = Flow::new(PageStyle::default(), &[("F1", StdFont::Helvetica)]);
    flow.tagged("en");
    flow.formula(&block(), "E = mc2", "E equals m times c squared");
    let formula = flow
        .structure
        .iter()
        .find(|e| e.tag == "Formula")
        .expect("a Formula element");
    assert_eq!(
        formula.actual_text.as_deref(),
        Some("E equals m times c squared")
    );
    let s = String::from_utf8_lossy(&flow.build()).into_owned();
    assert!(s.contains("/S /Formula"));
    assert!(s.contains("/ActualText"));
}

#[test]
fn note_carries_its_id() {
    let mut flow = Flow::new(PageStyle::default(), &[("F1", StdFont::Helvetica)]);
    flow.tagged("en");
    flow.note(&block(), "See ISO 32000-2 for details.", "note-1");
    let note = flow
        .structure
        .iter()
        .find(|e| e.tag == "Note")
        .expect("a Note element");
    assert_eq!(note.id.as_deref(), Some("note-1"), "UA-1 §7.9 /ID");
    let s = String::from_utf8_lossy(&flow.build()).into_owned();
    assert!(s.contains("/S /Note"), "Note element serialised");
    assert!(s.contains("(note-1)"), "/ID string serialised");
}

#[test]
fn paragraph_spanning_pages_is_one_element() {
    // A tiny page so a single (newline-free) paragraph wraps past the bottom margin.
    let style = PageStyle {
        size: [300.0, 120.0],
        margins: [20.0; 4],
    };
    let mut flow = Flow::new(style, &[("F1", StdFont::Helvetica)]);
    flow.tagged("en");
    let para = (0..40)
        .map(|i| format!("word{i}"))
        .collect::<Vec<_>>()
        .join(" ");
    flow.text(&block(), &para);

    // One paragraph → exactly one P element, with content children on two or more pages.
    let ps: Vec<&StructElem> = flow.structure.iter().filter(|e| e.tag == "P").collect();
    assert_eq!(ps.len(), 1, "one paragraph should be one P element");
    let pages: std::collections::BTreeSet<usize> = ps[0]
        .kids
        .iter()
        .filter_map(|k| match k {
            StructKid::Content { page, .. } => Some(*page),
            _ => None,
        })
        .collect();
    assert!(
        pages.len() >= 2,
        "paragraph should span >= 2 pages, got {pages:?}"
    );
    assert!(flow.page_count() >= 2);
}

#[test]
fn table_emits_borders_and_cell_text() {
    let mut flow = Flow::new(PageStyle::default(), &[("F1", StdFont::Helvetica)]);
    let table = Table::new(vec![1.0, 2.0])
        .font("F1", "Helvetica")
        .row(["Name", "Description"])
        .row(["Foo", "the foo widget"]);
    let before_y = flow.cursor_y();
    flow.table(&table);
    assert!(
        flow.cursor_y() < before_y,
        "table did not advance the cursor"
    );
    let dump = String::from_utf8(std::mem::take(&mut flow.current).into_bytes()).unwrap();
    assert!(
        dump.contains(" re\n") && dump.contains("S\n"),
        "no grid drawn"
    );
    assert!(dump.contains("(Name) Tj") && dump.contains("(Foo) Tj"));
}

fn dejavu() -> Option<Vec<u8>> {
    std::fs::read("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf").ok()
}

#[test]
fn flags_notdef_when_a_glyph_is_missing() {
    let Some(font) = dejavu() else { return };
    // DejaVu Sans has no CJK glyphs: shaping "漢" falls back to .notdef (GID 0) and the flag
    // travels to the Builder (PDF/UA 14289-1 §7.21.8 / 14289-2 §8.4.5.9).
    let mut flow = Flow::new(PageStyle::default(), &[]);
    assert!(flow.embed_font("F1", &font));
    let block = TextBlock {
        font_resource: "F1",
        base_font: "",
        size: 12.0,
        leading: 16.0,
        align: Align::Left,
    };
    flow.text(&block, "notdef ahead: 漢");
    assert!(flow.into_builder().facts().notdef_glyph_referenced);

    // Fully covered text does not raise the flag.
    let mut flow = Flow::new(PageStyle::default(), &[]);
    assert!(flow.embed_font("F1", &font));
    flow.text(&block, "plain latin text");
    assert!(!flow.into_builder().facts().notdef_glyph_referenced);
}

#[test]
fn embedding_replaces_the_standard_14_registration_of_that_name() {
    let Some(font) = dejavu() else { return };
    // Declaring "F1" as Helvetica up front and then embedding under the same name is the natural
    // way to write the call. The embed must drop the Standard-14 registration, or the document
    // keeps a `standard_14_font_resources` count that `make_pdfa`/`make_pdfua` refuse on even
    // though every glyph is drawn from the embedded program.
    let mut flow = Flow::new(PageStyle::default(), &[("F1", StdFont::Helvetica)]);
    assert!(flow.embed_font("F1", &font));
    let block = TextBlock {
        font_resource: "F1",
        base_font: "",
        size: 12.0,
        leading: 14.0,
        align: Align::Left,
    };
    flow.text(&block, "Привет мир");
    let facts = flow.into_builder().facts();
    assert_eq!(facts.standard_14_font_resources, 0);

    // A name the embed never mentions keeps its Standard-14 registration.
    let mut flow = Flow::new(
        PageStyle::default(),
        &[("F1", StdFont::Helvetica), ("F2", StdFont::Courier)],
    );
    assert!(flow.embed_font("F1", &font));
    flow.text(&block, "Привет мир");
    assert_eq!(flow.into_builder().facts().standard_14_font_resources, 1);
}

#[test]
fn embeds_font_and_emits_type0() {
    let Some(font) = dejavu() else { return };
    let mut flow = Flow::new(PageStyle::default(), &[]);
    assert!(flow.embed_font("F1", &font));
    assert!(!flow.embed_font("F1", b"not a font")); // invalid program is rejected
    let block = TextBlock {
        font_resource: "F1",
        base_font: "",
        size: 14.0,
        leading: 18.0,
        align: Align::Left,
    };
    flow.text(&block, "Привет мир"); // Cyrillic
    let pdf = String::from_utf8_lossy(&flow.build()).into_owned();
    assert!(pdf.contains("/Type0") && pdf.contains("/Identity-H"));
    assert!(pdf.contains("/CIDFontType2") && pdf.contains("/ToUnicode"));
}

#[test]
fn embedded_font_is_subsetted() {
    let Some(font) = dejavu() else { return };
    let mut flow = Flow::new(PageStyle::default(), &[]);
    flow.embed_font("F1", &font);
    let block = TextBlock {
        font_resource: "F1",
        base_font: "",
        size: 12.0,
        leading: 14.0,
        align: Align::Left,
    };
    flow.text(&block, "Hi");
    let pdf = flow.build();
    // A two-glyph subset must be far smaller than the ~742 KiB full font, and use a CIDToGIDMap.
    assert!(
        pdf.len() < font.len() / 5,
        "pdf {} vs font {}",
        pdf.len(),
        font.len()
    );
    assert!(String::from_utf8_lossy(&pdf).contains("/CIDToGIDMap"));
}

#[test]
fn list_emits_markers_and_items() {
    let mut flow = Flow::new(PageStyle::default(), &[("F1", StdFont::Helvetica)]);
    flow.list(
        &block(),
        &["first item", "second item"],
        ListStyle::Numbered,
    );
    let dump = String::from_utf8(std::mem::take(&mut flow.current).into_bytes()).unwrap();
    // Numbered markers "1." and "2." are shown, plus the item text.
    assert!(
        dump.contains("(1.) Tj") && dump.contains("(2.) Tj"),
        "{dump}"
    );
    assert!(dump.contains("(first item) Tj"));
}

#[test]
fn image_is_placed_with_transform() {
    use crate::image::Image;
    let mut flow = Flow::new(PageStyle::default(), &[]);
    let img = Image::from_rgb(2, 1, vec![255, 0, 0, 0, 255, 0]).unwrap();
    let before = flow.cursor_y();
    flow.image(&img, 100.0, 50.0);
    assert!((flow.cursor_y() - (before - 50.0)).abs() < 1e-6);
    let dump = String::from_utf8(std::mem::take(&mut flow.current).into_bytes()).unwrap();
    assert!(
        dump.contains("100 0 0 50 ") && dump.contains("/Im0 Do"),
        "{dump}"
    );
}

#[test]
fn tall_table_spans_pages_and_repeats_header() {
    let style = PageStyle {
        size: [300.0, 160.0],
        margins: [24.0; 4],
    };
    let mut flow = Flow::new(style, &[("F1", StdFont::Helvetica)]);
    let mut table = Table::new(vec![1.0, 1.0])
        .header_row(true)
        .row(["H1", "H2"]);
    for i in 0..40 {
        table = table.row([format!("a{i}"), format!("b{i}")]);
    }
    flow.table(&table);
    assert!(flow.page_count() >= 3, "pages: {}", flow.page_count());
}
