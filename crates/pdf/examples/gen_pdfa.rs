//! Generate the Prism PDF PDF/A PASS corpus for conformance validation (Milestone M7.2).
//!
//! Writes one file for every (feature, flavour) `make_pdfa`/`make_pdfua` can produce — blank,
//! vector, raster images (RGB/gray/JPEG), tagged figure, text, tagged, attachment, PDF/UA — across
//! PDF/A-2 and -3 at levels B/U/A. CI (and the conformance harness) then runs veraPDF over them to
//! prove the producer is conformant on every accept path. Run:
//!
//! ```text
//! cargo run -p prismpdf --example gen_pdfa -- <out_dir>
//! ```
//!
//! The embedded-font samples (text/tagged/accessible) are added when a TrueType font is available —
//! pass its path as the second argument or via `PRISMPDF_FONT` (CI installs `fonts-dejavu-core`).
//! Each file is built through the normal authoring API and finalised with `make_pdfa`. Filenames
//! and the rule coverage they prove are documented in `corpus/prismpdf-pdfa/`.
#![allow(clippy::expect_used, clippy::unwrap_used)] // a corpus-generation example may panic on error

use std::path::{Path, PathBuf};

use prismpdf::{
    Align, AnnotationSpec, Attachment, Builder, Content, Flow, FormFieldSpec, Image, LinkTarget,
    OutputIntentProfile, PDF2_STRUCT_NS, PageLabelRange, PageLabelStyle, PageSpec, PageStyle,
    PdfAConformance, RoleMapEntry, StructElem, TextBlock, XmpMetadata, make_pdfa,
    make_pdfa_with_output_intent, make_pdfua, make_pdfua2,
};

fn main() -> std::io::Result<()> {
    let mut args = std::env::args().skip(1);
    let out_dir = PathBuf::from(args.next().unwrap_or_else(|| "pdfa-corpus".to_string()));
    let font_path = args
        .next()
        .or_else(|| std::env::var("PRISMPDF_FONT").ok())
        .unwrap_or_else(|| "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf".to_string());

    std::fs::create_dir_all(&out_dir)?;

    // The full matrix of every (feature, flavour) `make_pdfa`/`make_pdfua` can produce, so each
    // accept path is validated by veraPDF. Filenames follow the corpus convention
    // `prismpdf-<feature>-<flavour>-pass.pdf` (see corpus/prismpdf-pdfa/README.md).
    //
    // Untagged content (blank/vector/text) is valid at the B and U levels and at part-3 B, but
    // never at level A (which requires tagging). Tagged documents are valid at every level
    // (tagging is permitted at B/U, required at A). Attachments require part 3.
    use PdfAConformance::{A1a, A1b, A2a, A2b, A2u, A3a, A3b, A3u, A4, A4e, A4f};
    let untagged = [A1b, A2b, A2u, A3b, A4];
    let every = [A1b, A1a, A2b, A2u, A2a, A3b, A3a, A4];

    for c in untagged {
        write(&out_dir, &name("blank", c), blank(c))?;
        write(&out_dir, &name("vector", c), vector(c))?;
        write(&out_dir, &name("image", c), image_rgb(c))?;
        write(&out_dir, &name("imagegray", c), image_gray(c))?;
        write(&out_dir, &name("imagejpeg", c), image_jpeg(c))?;
        write(&out_dir, &name("signed", c), signed(c))?;
        write(&out_dir, &name("link", c), link(c))?;
        write(&out_dir, &name("note", c), note(c))?;
        write(&out_dir, &name("imagestencil", c), image_stencil(c))?;
        write(&out_dir, &name("form", c), form(c))?;
    }
    // A soft-masked (alpha) image is transparency, which PDF/A-1 forbids (ISO 19005-1 §6.4) —
    // the alpha samples cover every flavour from part 2 on.
    for c in [A2b, A2u, A3b, A4] {
        write(&out_dir, &name("imagealpha", c), image_alpha(c))?;
    }
    // A tagged image needs a structure tree, so it is a level-A document (1A / 2A / 3A).
    for c in [A1a, A2a, A3a] {
        write(&out_dir, &name("figure", c), figure(c))?;
    }
    // attachment() is untagged and needs a flavour that permits embedded files: part 3 (3B/3U)
    // or the part-4 F/E extensions (ISO 19005-4 Annex A/B).
    write(&out_dir, "prismpdf-attach-3b-pass.pdf", attachment(A3b))?;
    write(&out_dir, "prismpdf-attach-3u-pass.pdf", attachment(A3u))?;
    write(&out_dir, "prismpdf-attach-4f-pass.pdf", attachment(A4f))?;
    write(&out_dir, "prismpdf-attach-4e-pass.pdf", attachment(A4e))?;
    // PDF/A-3U (part 3, level U): U requires all text Unicode-mapped. A blank page is trivially U
    // (no text); the text proof below carries an embedded font + Unicode. Both permit attachments.
    write(&out_dir, &name("blank", A3u), blank(A3u))?;

    // DeviceCMYK needs a CMYK OutputIntent. The producer can author one via
    // `make_pdfa_with_output_intent`, but no CMYK ICC profile is *bundled*: real CMYK profiles carry
    // vendor copyrights (the free eciCMYK is Heidelberg "all rights reserved", 1.8 MB) and don't meet
    // this repo's CC0/permissive asset bar — so the CMYK PASS file is "bring your own profile" and is
    // never committed. Point `PRISMPDF_CMYK_ICC` at a CMYK `.icc` to emit a *conformant* CMYK file
    // (validate it with veraPDF); without it, `PRISMPDF_CMYK_PROBE=1` writes the old non-conformant
    // probe (sRGB OutputIntent) to confirm the colour-space rule still bites.
    if let Some(icc_path) = std::env::var_os("PRISMPDF_CMYK_ICC") {
        let icc = std::fs::read(&icc_path).expect("read CMYK ICC profile");
        write(&out_dir, "cmyk-pass.pdf", cmyk_conformant(A2b, &icc))?;
    } else if std::env::var_os("PRISMPDF_CMYK_PROBE").is_some() {
        write(&out_dir, "cmyk-probe.pdf", cmyk(A2b))?;
    }

    match std::fs::read(&font_path) {
        Ok(font) => {
            for c in untagged {
                write(&out_dir, &name("text", c), text(c, &font))?;
            }
            // PDF/A-3U text proof: embedded font + Unicode text at part 3, level U.
            write(&out_dir, &name("text", A3u), text(A3u, &font))?;
            for c in every {
                write(&out_dir, &name("tagged", c), tagged(c, &font))?;
            }
            write(&out_dir, "prismpdf-accessible-ua1-pass.pdf", ua1(&font))?;
            write(&out_dir, "prismpdf-accessible-ua2-pass.pdf", ua2(&font))?;
        }
        Err(_) => eprintln!(
            "note: no font at {font_path}; skipping the embedded-text samples (set PRISMPDF_FONT)"
        ),
    }

    Ok(())
}

/// Corpus filename for a `(feature, flavour)` PASS case, e.g. `prismpdf-tagged-3a-pass.pdf`. The
/// flavour tag is the conformance code (`2b`, `4f`, …) so it can't drift.
fn name(feature: &str, c: PdfAConformance) -> String {
    format!("prismpdf-{feature}-{}-pass.pdf", c.code())
}

/// A blank page — the minimal conformant document.
fn blank(conformance: PdfAConformance) -> Vec<u8> {
    let mut builder = Builder::new();
    builder.add_page(PageSpec::new(Vec::new()));
    finalize(builder, conformance, "Blank PDF/A sample")
}

/// A page of vector graphics (no fonts) exercising DeviceRGB fills — which is exactly why the sRGB
/// OutputIntent must be present for conformance.
fn vector(conformance: PdfAConformance) -> Vec<u8> {
    let mut c = Content::new();
    c.set_fill_rgb(0.20, 0.40, 0.80);
    c.rect(72.0, 600.0, 200.0, 120.0);
    c.fill();
    c.set_stroke_rgb(0.80, 0.10, 0.10);
    c.set_line_width(3.0);
    c.move_to(72.0, 560.0);
    c.line_to(472.0, 560.0);
    c.stroke();

    let mut builder = Builder::new();
    builder.add_page(PageSpec::new(c.into_bytes()));
    finalize(builder, conformance, "Vector PDF/A sample")
}

/// A page of text set in an embedded font (the realistic case), via the flow layer.
fn text(conformance: PdfAConformance, font: &[u8]) -> Vec<u8> {
    let mut flow = Flow::new(PageStyle::default(), &[]);
    assert!(flow.embed_font("F1", font), "invalid font program");
    let block = TextBlock {
        font_resource: "F1",
        base_font: "",
        size: 14.0,
        leading: 18.0,
        align: Align::Left,
    };
    flow.text(
        &block,
        "Prism PDF PDF/A-2U sample.\nEmbedded font, Unicode text: café résumé — €5.",
    );
    finalize(flow.into_builder(), conformance, "Text PDF/A sample")
}

/// A **tagged** page (PDF/A level A) set in an embedded font: a heading, a paragraph and a list,
/// carrying a logical structure tree (§14.7) so assistive technology can navigate it.
fn tagged(conformance: PdfAConformance, font: &[u8]) -> Vec<u8> {
    let mut flow = Flow::new(PageStyle::default(), &[]);
    assert!(flow.embed_font("F1", font), "invalid font program");
    flow.tagged("en-US");
    let heading = TextBlock {
        font_resource: "F1",
        base_font: "",
        size: 20.0,
        leading: 26.0,
        align: Align::Left,
    };
    let body = TextBlock {
        size: 12.0,
        leading: 16.0,
        ..heading
    };
    flow.heading(1, &heading, "Prism PDF PDF/A-2A sample");
    flow.space(8.0);
    flow.text(
        &body,
        "A tagged document carries its logical structure separately from its visual \
         rendering, so it can be read in order by assistive technology.",
    );
    flow.space(8.0);
    flow.list(
        &body,
        &["First accessible item", "Second accessible item"],
        prismpdf::ListStyle::Bullet,
    );
    finalize(flow.into_builder(), conformance, "Tagged PDF/A sample")
}

/// A **PDF/UA-1** (ISO 14289-1) accessible document: a tagged page in an embedded font with a title
/// shown in the viewer (`/DisplayDocTitle`) and a document language, exercising the UA-1
/// accessibility surface — a numbered list with `/ListNumbering` (§7.6), a `Note` with `/ID`
/// (§7.9), a figure with a nested `Caption` (§7.3), and a checkbox widget nested in a `Form`
/// structure element (§7.18.4) with `/TU` and page `/Tabs /S` (§7.18.3).
fn ua1(font: &[u8]) -> Vec<u8> {
    let mut flow = Flow::new(PageStyle::default(), &[]);
    assert!(flow.embed_font("F1", font), "invalid font program");
    flow.tagged("en-US");
    let heading = TextBlock {
        font_resource: "F1",
        base_font: "",
        size: 20.0,
        leading: 26.0,
        align: Align::Left,
    };
    let body = TextBlock {
        size: 12.0,
        leading: 16.0,
        ..heading
    };
    flow.heading(1, &heading, "Accessible Prism PDF document");
    flow.space(8.0);
    flow.text(
        &body,
        "This document is tagged for accessibility and identifies as PDF/UA-1, with a \
         document language and a title shown in the reader.",
    );
    flow.space(8.0);
    flow.list(
        &body,
        &["Perceivable structure", "Operable navigation"],
        prismpdf::ListStyle::Numbered,
    );
    flow.space(8.0);
    flow.figure_with_caption(
        &Image::from_rgb(2, 2, checker_rgb()).expect("rgb image"),
        72.0,
        72.0,
        "a two-by-two checkerboard",
        &body,
        "Figure 1 — the sample checkerboard.",
    );
    flow.space(8.0);
    flow.note(
        &body,
        "Conformance claims follow ISO 14289-1 clause 5.",
        "note-1",
    );

    let mut builder = flow.into_builder();
    // An interactive checkbox nested in a Form structure element (§7.18.4): the widget takes /TU
    // and a /StructParent, and the page /Tabs /S.
    builder.add_form_field(
        0,
        FormFieldSpec::Checkbox {
            rect: [72.0, 96.0, 88.0, 112.0],
            name: "confirm".to_string(),
            checked: false,
            tooltip: Some("Confirm you have read the document".to_string()),
        },
        Vec::new(),
    );
    let mut form_elem = StructElem::new("Form");
    form_elem.push_widget(0);
    builder.add_structure_element(form_elem);
    let meta = XmpMetadata {
        title: Some("Accessible Prism PDF document".to_string()),
        authors: vec!["Prism PDF".to_string()],
        producer: Some("Prism PDF".to_string()),
        creator_tool: Some("Prism PDF gen_pdfa".to_string()),
        ..Default::default()
    };
    make_pdfua(&mut builder, &meta, "en-US").expect("document must be PDF/UA-ready");
    builder.build()
}

/// A **PDF/UA-2** (ISO 14289-2:2024, PDF 2.0) accessible document: the root `Document` element in
/// the PDF 2.0 structure namespace, XMP `pdfuaid:part` 2 + `rev`, a `Title` element (§8.2.5.13), a
/// `FENote` bidirectionally `/Ref`-linked with its citing paragraph (§8.2.5.14), plus the UA-1
/// surface (numbered list, captioned figure, checkbox-in-`Form`, `/Tabs /S`).
fn ua2(font: &[u8]) -> Vec<u8> {
    let mut flow = Flow::new(PageStyle::default(), &[]);
    assert!(flow.embed_font("F1", font), "invalid font program");
    flow.tagged("en-US");
    let heading = TextBlock {
        font_resource: "F1",
        base_font: "",
        size: 20.0,
        leading: 26.0,
        align: Align::Left,
    };
    let body = TextBlock {
        size: 12.0,
        leading: 16.0,
        ..heading
    };
    flow.title_element(&heading, "Accessible Prism PDF 2.0 document");
    flow.space(8.0);
    flow.heading(1, &heading, "Introduction");
    flow.space(8.0);
    flow.text(
        &body,
        "This document is tagged for accessibility on a PDF 2.0 base and identifies as \
         PDF/UA-2 [1].",
    );
    if let Some(p) = flow.last_element_mut() {
        p.id = Some("cite-1".to_string());
        p.refs.push("fn-1".to_string());
    }
    flow.space(8.0);
    flow.list(
        &body,
        &["Perceivable structure", "Operable navigation"],
        prismpdf::ListStyle::Numbered,
    );
    flow.space(8.0);
    flow.figure_with_caption(
        &Image::from_rgb(2, 2, checker_rgb()).expect("rgb image"),
        72.0,
        72.0,
        "a two-by-two checkerboard",
        &body,
        "Figure 1 — the sample checkerboard.",
    );
    flow.space(8.0);
    // A formula (§8.2.5.29): rendered text + /ActualText, with presentation MathML attached as
    // an associated file (AFRelationship Supplement) on the Formula element.
    flow.formula(&body, "E = mc2", "E equals m times c squared");
    if let Some(f) = flow.last_element_mut() {
        f.af.push(Attachment {
            name: "formula.mml".to_string(),
            mime: "application/mathml+xml".to_string(),
            relationship: "Supplement".to_string(),
            description: Some("Presentation MathML for the formula".to_string()),
            mod_date: None,
            data: b"<math xmlns=\"http://www.w3.org/1998/Math/MathML\"><mrow><mi>E</mi>\
                    <mo>=</mo><mi>m</mi><msup><mi>c</mi><mn>2</mn></msup></mrow></math>"
                .to_vec(),
        });
    }
    flow.space(8.0);
    // A code fragment tagged `Code` (§8.2.5.32) — retagged from a plain paragraph.
    flow.text(&body, "let x = 42;");
    if let Some(p) = flow.last_element_mut() {
        p.tag = "Code".to_string();
    }
    flow.space(8.0);
    flow.fenote(
        &body,
        "[1] Conformance claims follow ISO 14289-2 clause 5.",
        "fn-1",
        &["cite-1"],
    );
    flow.space(8.0);
    // A language change declared on the element (§8.2.7): an Italian quotation.
    flow.text(&body, "«La semplicità è la sofisticazione suprema.»");
    if let Some(p) = flow.last_element_mut() {
        p.lang = Some("it-IT".to_string());
    }
    flow.space(8.0);
    // A custom structure type in its own namespace, role-mapped (§8.2.4 / /RoleMapNS §14.7.4)
    // to the standard 2.0 type Aside below.
    flow.text(
        &body,
        "Tip: role-mapped custom structure types stay accessible.",
    );
    if let Some(p) = flow.last_element_mut() {
        p.tag = "Callout".to_string();
        p.ns = Some("https://prismpdf.dev/ns/sample".to_string());
    }
    // Second page: the target section of the intra-document link below.
    flow.page_break();
    flow.heading(2, &heading, "Details");
    if let Some(h) = flow.last_element_mut() {
        h.id = Some("sec-details".to_string());
    }
    flow.text(&body, "Further details live on this page.");

    let mut builder = flow.into_builder();
    builder.add_form_field(
        0,
        FormFieldSpec::Checkbox {
            rect: [72.0, 96.0, 88.0, 112.0],
            name: "confirm".to_string(),
            checked: false,
            tooltip: Some("Confirm you have read the document".to_string()),
        },
        Vec::new(),
    );
    let mut form_elem = StructElem::new("Form");
    form_elem.push_widget(0);
    builder.add_structure_element(form_elem);
    // An intra-document link with a structure destination (§8.8) to the Details heading, woven
    // into the structure tree as a Link element (§8.2.5.20).
    builder.add_annotation(
        0,
        AnnotationSpec::Link {
            rect: [72.0, 120.0, 220.0, 136.0],
            target: LinkTarget::Element("sec-details".to_string()),
            contents: Some("Go to the Details section".to_string()),
        },
        Vec::new(),
    );
    let mut link_elem = StructElem::new("Link");
    link_elem.push_annotation(0);
    builder.add_structure_element(link_elem);
    // The role map for the custom Callout type: → Aside in the PDF 2.0 namespace.
    builder.role_map_ns(vec![RoleMapEntry {
        ns: "https://prismpdf.dev/ns/sample".to_string(),
        custom: "Callout".to_string(),
        target: "Aside".to_string(),
        target_ns: Some(PDF2_STRUCT_NS.to_string()),
    }]);
    // Page labels (§8.12.3): a roman front page then decimal content pages.
    builder.page_labels(vec![
        PageLabelRange {
            first_page: 0,
            style: Some(PageLabelStyle::RomanLower),
            prefix: None,
            start: None,
        },
        PageLabelRange {
            first_page: 1,
            style: Some(PageLabelStyle::Decimal),
            prefix: None,
            start: Some(1),
        },
    ]);

    let meta = XmpMetadata {
        title: Some("Accessible Prism PDF 2.0 document".to_string()),
        authors: vec!["Prism PDF".to_string()],
        producer: Some("Prism PDF".to_string()),
        creator_tool: Some("Prism PDF gen_pdfa".to_string()),
        ..Default::default()
    };
    make_pdfua2(&mut builder, &meta, "en-US").expect("document must be PDF/UA-2-ready");
    builder.build()
}

/// A PDF/A-3 page with an embedded XML file associated to the document — the e-invoicing case
/// (FatturaPA/ZUGFeRD: the machine-readable invoice rides inside the archival PDF). Valid at part-3
/// levels B and U (the page carries no text, so it is trivially Unicode-conformant for level U).
fn attachment(conformance: PdfAConformance) -> Vec<u8> {
    let mut builder = Builder::new();
    builder.add_page(PageSpec::new(Vec::new()));
    builder.attach_file(Attachment {
        name: "invoice.xml".to_string(),
        mime: "text/xml".to_string(),
        relationship: "Data".to_string(),
        description: Some("Machine-readable invoice".to_string()),
        mod_date: None,
        data: b"<?xml version=\"1.0\"?>\n<invoice><total>5.00</total></invoice>\n".to_vec(),
    });
    finalize(builder, conformance, "PDF/A-3 with attachment")
}

/// A page with hyperlinks (§12.5.6.5): an external `URI` link and an internal `GoTo` link to a
/// second page. Link annotations need no appearance stream under PDF/A (§6.3.3) and use only
/// permitted actions (§6.5.1) — the realistic "clickable document" case.
fn link(conformance: PdfAConformance) -> Vec<u8> {
    let mut builder = Builder::new();
    builder.add_page(PageSpec::new(Vec::new()));
    builder.add_page(PageSpec::new(Vec::new()));
    builder.add_annotation(
        0,
        AnnotationSpec::Link {
            rect: [72.0, 700.0, 320.0, 720.0],
            target: LinkTarget::Uri("https://example.org/".to_string()),
            contents: Some("Prism PDF example website".to_string()),
        },
        Vec::new(),
    );
    builder.add_annotation(
        0,
        AnnotationSpec::Link {
            rect: [72.0, 670.0, 320.0, 690.0],
            target: LinkTarget::Page(1),
            contents: Some("Go to the second page".to_string()),
        },
        Vec::new(),
    );
    finalize(builder, conformance, "Hyperlink PDF/A sample")
}

/// A page with a text-note annotation (§12.5.6.4) carrying a normal appearance stream (a Form
/// XObject) — the non-link case that PDF/A requires an appearance for (§6.3.3 t1/t2/t4).
fn note(conformance: PdfAConformance) -> Vec<u8> {
    let mut builder = Builder::new();
    builder.add_page(PageSpec::new(Vec::new()));
    builder.add_annotation(
        0,
        AnnotationSpec::Note {
            rect: [72.0, 690.0, 92.0, 710.0],
            contents: "A reviewer note embedded as a PDF/A-clean annotation.".to_string(),
        },
        Vec::new(),
    );
    finalize(builder, conformance, "Text-note PDF/A sample")
}

/// A page with an interactive form (§12.7): two checkbox fields with vector `/On`+`/Off` appearance
/// subdictionaries. PDF/A-clean — no `/NeedAppearances`, no `/XFA`, no widget actions, each widget
/// carries a normal appearance (§6.4.1/§6.4.2/§6.3.3 t3); font-free, so no embedding concern.
fn form(conformance: PdfAConformance) -> Vec<u8> {
    let mut builder = Builder::new();
    builder.add_page(PageSpec::new(Vec::new()));
    builder.add_form_field(
        0,
        FormFieldSpec::Checkbox {
            rect: [72.0, 700.0, 90.0, 718.0],
            name: "subscribe".to_string(),
            checked: true,
            tooltip: Some("Subscribe to the newsletter".to_string()),
        },
        Vec::new(),
    );
    builder.add_form_field(
        0,
        FormFieldSpec::Checkbox {
            rect: [72.0, 670.0, 90.0, 688.0],
            name: "terms".to_string(),
            checked: false,
            tooltip: Some("Accept the terms of service".to_string()),
        },
        Vec::new(),
    );
    finalize(builder, conformance, "Interactive form PDF/A sample")
}

/// A small RGB checkerboard, raw 8-bit samples (FlateDecode), placed as an image XObject (§8.9).
fn image_rgb(conformance: PdfAConformance) -> Vec<u8> {
    let img = Image::from_rgb(2, 2, checker_rgb()).expect("rgb image");
    let mut flow = Flow::new(PageStyle::default(), &[]);
    flow.image(&img, 144.0, 144.0);
    finalize(flow.into_builder(), conformance, "RGB image PDF/A sample")
}

/// An RGBA image whose alpha channel becomes a `DeviceGray` **soft mask** (`/SMask`, §11.6.5.2) —
/// per-pixel transparency, the PNG-with-alpha case (PDF/A-2 permits image transparency).
fn image_alpha(conformance: PdfAConformance) -> Vec<u8> {
    // 2×2 RGBA: opaque red, half-alpha green, quarter-alpha blue, three-quarter-alpha yellow.
    let rgba = vec![
        0xFF, 0x00, 0x00, 0xFF, 0x00, 0xFF, 0x00, 0x80, // red (opaque), green (50%)
        0x00, 0x00, 0xFF, 0x40, 0xFF, 0xFF, 0x00, 0xC0, // blue (25%), yellow (75%)
    ];
    let img = Image::from_rgba(2, 2, rgba).expect("rgba image");
    let mut flow = Flow::new(PageStyle::default(), &[]);
    flow.image(&img, 144.0, 144.0);
    finalize(
        flow.into_builder(),
        conformance,
        "RGBA soft-mask PDF/A sample",
    )
}

/// An image carrying a 1-bit **stencil mask** (`/Mask`, §8.9.6.3): exercises the PDMaskImage rule
/// (PDF/A §6.2.8 t5 — a mask image's BitsPerComponent must be 1).
fn image_stencil(conformance: PdfAConformance) -> Vec<u8> {
    // 2×2 stencil, 1 byte/row (top 2 bits = the 2 pixels): mask out one pixel per row (1 = masked).
    let img = Image::from_rgb(2, 2, checker_rgb())
        .expect("rgb image")
        .with_stencil_mask(2, 2, vec![0b0100_0000, 0b1000_0000])
        .expect("stencil mask");
    let mut flow = Flow::new(PageStyle::default(), &[]);
    flow.image(&img, 144.0, 144.0);
    finalize(
        flow.into_builder(),
        conformance,
        "Stencil-masked image PDF/A sample",
    )
}

/// A DeviceGray raster image (§8.6 device colour: gray is admissible with an OutputIntent present).
fn image_gray(conformance: PdfAConformance) -> Vec<u8> {
    let img = Image::from_gray(2, 2, vec![0x00, 0x55, 0xAA, 0xFF]).expect("gray image");
    let mut flow = Flow::new(PageStyle::default(), &[]);
    flow.image(&img, 144.0, 144.0);
    finalize(flow.into_builder(), conformance, "Gray image PDF/A sample")
}

/// A JPEG image embedded as-is via `DCTDecode` (§7.4.8) — the realistic photographic case.
fn image_jpeg(conformance: PdfAConformance) -> Vec<u8> {
    let img = Image::from_jpeg(jpeg_2x2()).expect("jpeg image");
    let mut flow = Flow::new(PageStyle::default(), &[]);
    flow.image(&img, 144.0, 144.0);
    finalize(flow.into_builder(), conformance, "JPEG image PDF/A sample")
}

/// A **tagged** image (PDF/A level A): the image carries a `Figure` structure element with `/Alt`
/// text, so an image is accessible (§14.7 / §14.8).
fn figure(conformance: PdfAConformance) -> Vec<u8> {
    let img = Image::from_rgb(2, 2, checker_rgb()).expect("rgb image");
    let mut flow = Flow::new(PageStyle::default(), &[]);
    flow.tagged("en-US");
    flow.figure(&img, 144.0, 144.0, "A two-by-two colour checkerboard");
    finalize(
        flow.into_builder(),
        conformance,
        "Tagged figure PDF/A sample",
    )
}

/// A digitally **signed** PDF/A (§12.8): a conformant base document, then `Document::sign` appends
/// an invisible signature field + detached CMS as an incremental update. The signature is invisible
/// on purpose — a *visible* appearance would draw with non-embedded Helvetica, which PDF/A forbids.
///
/// Uses a committed throwaway test keypair and a fixed signing time, so the output is byte-for-byte
/// reproducible (RSA PKCS#1 v1.5 is deterministic).
fn signed(conformance: PdfAConformance) -> Vec<u8> {
    // Throwaway self-signed RSA-2048 test signer (DER), generated once; see test-signer/README.
    const CERT: &[u8] = include_bytes!("test-signer/cert.der");
    const KEY: &[u8] = include_bytes!("test-signer/key.der");
    // A fixed instant within the cert's validity, so /M and the CMS signingTime are reproducible.
    const SIGNING_TIME: u64 = 1_790_000_000; // 2026-09-21 UTC

    let base = blank(conformance);
    let doc = prismpdf::Document::open(base).expect("reopen base PDF/A");
    let settings = prismpdf::SignSettings {
        name: Some("Prism PDF Test Signer".to_string()),
        reason: Some("Archival integrity".to_string()),
        signing_time: Some(SIGNING_TIME),
        ..Default::default()
    };
    doc.sign_with(CERT, KEY, &settings).expect("sign PDF/A")
}

/// A DeviceCMYK vector fill. PDF/A admits DeviceCMYK only with a CMYK OutputIntent; `make_pdfa`
/// ships an sRGB one, so this is expected to be **rejected** — used only to confirm the limitation
/// (see the corpus README's colour-gap note), never committed as a PASS file.
fn cmyk(conformance: PdfAConformance) -> Vec<u8> {
    let mut c = Content::new();
    c.set_fill_cmyk(0.1, 0.2, 0.3, 0.0);
    c.rect(72.0, 600.0, 200.0, 120.0);
    c.fill();
    let mut builder = Builder::new();
    builder.add_page(PageSpec::new(c.into_bytes()));
    finalize(builder, conformance, "CMYK probe (expected non-conformant)")
}

/// The same DeviceCMYK page, but finalised with a caller-supplied **CMYK** OutputIntent
/// (`make_pdfa_with_output_intent`) so it *is* conformant — the demonstration that the producer can
/// author CMYK PDF/A once given a profile. `icc` is the user's CMYK ICC bytes (4 components); the
/// profile is not bundled (see the call site for why).
fn cmyk_conformant(conformance: PdfAConformance, icc: &[u8]) -> Vec<u8> {
    let mut c = Content::new();
    c.set_fill_cmyk(0.1, 0.2, 0.3, 0.0);
    c.rect(72.0, 600.0, 200.0, 120.0);
    c.fill();
    let mut builder = Builder::new();
    builder.add_page(PageSpec::new(c.into_bytes()));
    let meta = XmpMetadata {
        title: Some("CMYK PDF/A sample".to_string()),
        authors: vec!["Prism PDF".to_string()],
        producer: Some("Prism PDF".to_string()),
        creator_tool: Some("Prism PDF gen_pdfa".to_string()),
        ..Default::default()
    };
    let intent = OutputIntentProfile::new(icc.to_vec(), 4, "Custom CMYK");
    make_pdfa_with_output_intent(&mut builder, conformance, &meta, &intent)
        .expect("document must be PDF/A-ready");
    builder.build()
}

/// A 2×2 RGB checkerboard as raw interleaved 8-bit samples (12 bytes).
fn checker_rgb() -> Vec<u8> {
    vec![
        0xFF, 0x00, 0x00, 0x00, 0xFF, 0x00, // red,   green
        0x00, 0x00, 0xFF, 0xFF, 0xFF, 0x00, // blue,  yellow
    ]
}

/// A 2×2 baseline JPEG (ImageMagick), inline so the example needs no external asset.
fn jpeg_2x2() -> Vec<u8> {
    const B64: &str = "/9j/4AAQSkZJRgABAQAAAQABAAD/2wBDAAMCAgICAgMCAgIDAwMDBAYEBAQEBAgGBgUGCQgKCgkICQkKDA8MCgsOCwkJDRENDg8QEBEQCgwSExIQEw8QEBD/2wBDAQMDAwQDBAgEBAgQCwkLEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBD/wAARCAACAAIDAREAAhEBAxEB/8QAFAABAAAAAAAAAAAAAAAAAAAACP/EABQQAQAAAAAAAAAAAAAAAAAAAAD/xAAVAQEBAAAAAAAAAAAAAAAAAAAHCf/EABQRAQAAAAAAAAAAAAAAAAAAAAD/2gAMAwEAAhEDEQA/ADoDFU3/2Q==";
    base64_decode(B64)
}

/// Minimal standard-base64 decoder (the example carries one tiny asset; avoids a dependency).
fn base64_decode(s: &str) -> Vec<u8> {
    let val = |c: u8| match c {
        b'A'..=b'Z' => (c - b'A') as i32,
        b'a'..=b'z' => (c - b'a' + 26) as i32,
        b'0'..=b'9' => (c - b'0' + 52) as i32,
        b'+' => 62,
        b'/' => 63,
        _ => -1,
    };
    let (mut acc, mut bits, mut out) = (0i32, 0, Vec::new());
    for &c in s.as_bytes() {
        let v = val(c);
        if v < 0 {
            continue;
        }
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    out
}

/// Attach the PDF/A structures and serialise.
fn finalize(mut builder: Builder, conformance: PdfAConformance, title: &str) -> Vec<u8> {
    let meta = XmpMetadata {
        title: Some(title.to_string()),
        authors: vec!["Prism PDF".to_string()],
        producer: Some("Prism PDF".to_string()),
        creator_tool: Some("Prism PDF gen_pdfa".to_string()),
        ..Default::default()
    };
    make_pdfa(&mut builder, conformance, &meta).expect("document must be PDF/A-ready");
    builder.build()
}

fn write(dir: &Path, name: &str, bytes: Vec<u8>) -> std::io::Result<()> {
    let path = dir.join(name);
    std::fs::write(&path, &bytes)?;
    println!("wrote {} ({} bytes)", path.display(), bytes.len());
    Ok(())
}
