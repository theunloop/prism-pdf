//! PDF/A production through the facade (EPIC 13, §14): XMP metadata, OutputIntent, attachments
//! and the embedded-font precondition.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use prismpdf::cos;
use prismpdf::{
    Align, Attachment, Builder, Document, Flow, PageSpec, PageStyle, PdfAConformance, PdfAError,
    StdFont, TextBlock, XmpMetadata, make_pdfa, page_text,
};

#[test]
fn make_pdfa_through_the_facade() {
    // The PDF/A API is reachable from the facade and produces a marked, reopenable file.
    let mut builder = Builder::new();
    builder.add_page(PageSpec::new(Vec::new())); // blank page = minimal conformant document
    let meta = XmpMetadata {
        title: Some("Quarterly Report".into()),
        authors: vec!["Jane Doe".into()],
        producer: Some("Prism PDF".into()),
        ..Default::default()
    };
    make_pdfa(&mut builder, PdfAConformance::A2u, &meta).unwrap();
    let bytes = builder.build();

    let doc = Document::open(bytes).unwrap();
    assert_eq!(doc.page_count().unwrap(), 1);
    let catalog = doc.catalog().unwrap();
    assert!(catalog.get(&cos::Name::from("Metadata")).is_some());
    assert!(catalog.get(&cos::Name::from("OutputIntents")).is_some());
    // U-level identification made it into the XMP.
    let Some(cos::Object::Reference(m)) = catalog.get(&cos::Name::from("Metadata")) else {
        panic!("no /Metadata");
    };
    let cos::Object::Stream(s) = doc.get(*m).unwrap() else {
        panic!("not a stream");
    };
    let xmp = String::from_utf8_lossy(s.raw().as_ref()).into_owned();
    assert!(xmp.contains("<pdfaid:conformance>U</pdfaid:conformance>"));
}

#[test]
fn pdfa_from_flow_with_embedded_font() {
    // The realistic path: lay text with an embedded font, then make the document PDF/A.
    let Some(font) = std::fs::read("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf").ok() else {
        return; // hermetic when no system font is present
    };
    let mut flow = Flow::new(PageStyle::default(), &[]);
    assert!(flow.embed_font("F1", &font));
    let block = TextBlock {
        font_resource: "F1",
        base_font: "",
        size: 14.0,
        leading: 18.0,
        align: Align::Left,
    };
    flow.text(&block, "Archival document — café résumé");

    let mut builder = flow.into_builder();
    let meta = XmpMetadata {
        title: Some("Archive".into()),
        authors: vec!["Prism PDF".into()],
        ..Default::default()
    };
    // No Standard-14 fonts were registered, so the embedded-font check passes.
    make_pdfa(&mut builder, PdfAConformance::A2u, &meta).unwrap();
    let pdf = builder.build();

    let doc = Document::open(pdf).unwrap();
    let catalog = doc.catalog().unwrap();
    assert!(catalog.get(&cos::Name::from("Metadata")).is_some());
    assert!(catalog.get(&cos::Name::from("OutputIntents")).is_some());
    // The embedded font survived and the text still extracts.
    let text = page_text(&doc, 0).unwrap().unwrap();
    assert!(text.contains("Archival document"), "{text:?}");
    assert!(text.contains("café"));
}

#[test]
fn pdfa3_attachment_round_trips() {
    let mut builder = Builder::new();
    builder.add_page(PageSpec::new(Vec::new()));
    builder.attach_file(Attachment {
        name: "data.xml".into(),
        mime: "application/xml".into(),
        relationship: "Data".into(),
        description: None,
        mod_date: None,
        data: b"<x/>".to_vec(),
    });
    make_pdfa(&mut builder, PdfAConformance::A3b, &XmpMetadata::default()).unwrap();
    let doc = Document::open(builder.build()).unwrap();
    let catalog = doc.catalog().unwrap();
    // Document-associated file present.
    assert!(catalog.get(&cos::Name::from("AF")).is_some());
    assert!(catalog.get(&cos::Name::from("Names")).is_some());
}

#[test]
fn pdfa4_pins_pdf20_and_keeps_metadata_out_of_info() {
    // PDF/A-4 (ISO 19005-4) sits on PDF 2.0 and forbids an /Info dictionary carrying anything
    // beyond /ModDate: the header must stamp 2.0, the XMP must identify part 4 with its revision
    // year and no conformance key, and the trailer must not reference an /Info at all.
    let mut builder = Builder::new();
    builder.add_page(PageSpec::new(Vec::new()));
    builder.title("Stale user title"); // must be cleared by the part-4 pass
    let meta = XmpMetadata {
        title: Some("Archived 2.0".into()),
        producer: Some("Prism PDF".into()),
        ..Default::default()
    };
    make_pdfa(&mut builder, PdfAConformance::A4, &meta).unwrap();
    let bytes = builder.build();

    let text = String::from_utf8_lossy(&bytes).into_owned();
    assert!(text.starts_with("%PDF-2.0"), "header: {:?}", &text[..16]);
    // Part 4 forbids a document Info dictionary: the trailer must not reference one. (An
    // OutputIntent legitimately carries its own /Info entry, so scope the check to the trailer.)
    let trailer = &text[text.rfind("trailer").expect("trailer keyword")..];
    assert!(!trailer.contains("/Info"), "trailer: {trailer:?}");

    let doc = Document::open(bytes).unwrap();
    let catalog = doc.catalog().unwrap();
    let Some(cos::Object::Reference(m)) = catalog.get(&cos::Name::from("Metadata")) else {
        panic!("no /Metadata");
    };
    let cos::Object::Stream(s) = doc.get(*m).unwrap() else {
        panic!("not a stream");
    };
    let xmp = String::from_utf8_lossy(s.raw().as_ref()).into_owned();
    assert!(xmp.contains("<pdfaid:part>4</pdfaid:part>"));
    assert!(xmp.contains("<pdfaid:rev>2020</pdfaid:rev>"));
    assert!(!xmp.contains("pdfaid:conformance"));
    assert!(xmp.contains("<dc:title>")); // the metadata lives in the XMP instead
}

#[test]
fn pdfa4_attachments_need_the_f_or_e_extension() {
    // Plain PDF/A-4 forbids embedded files; the F (and E) extensions permit them.
    let attach = || Attachment {
        name: "data.xml".into(),
        mime: "application/xml".into(),
        relationship: "Data".into(),
        description: None,
        mod_date: None,
        data: b"<x/>".to_vec(),
    };
    let mut builder = Builder::new();
    builder.add_page(PageSpec::new(Vec::new()));
    builder.attach_file(attach());
    assert!(matches!(
        make_pdfa(&mut builder, PdfAConformance::A4, &XmpMetadata::default()),
        Err(prismpdf::Error::PdfA(PdfAError::AttachmentRequiresPdfA3))
    ));
    assert!(make_pdfa(&mut builder, PdfAConformance::A4f, &XmpMetadata::default()).is_ok());
    let mut engineering = Builder::new();
    engineering.add_page(PageSpec::new(Vec::new()));
    engineering.attach_file(attach());
    assert!(
        make_pdfa(
            &mut engineering,
            PdfAConformance::A4e,
            &XmpMetadata::default()
        )
        .is_ok()
    );
}

#[test]
fn pdfa1_pins_pdf14_and_rejects_transparency() {
    // PDF/A-1 sits on PDF 1.4: the header stamps 1.4 and the XMP identifies part 1 level B.
    let mut builder = Builder::new();
    builder.add_page(PageSpec::new(Vec::new()));
    make_pdfa(&mut builder, PdfAConformance::A1b, &XmpMetadata::default()).unwrap();
    let bytes = builder.build();
    let text = String::from_utf8_lossy(&bytes).into_owned();
    assert!(text.starts_with("%PDF-1.4"), "header: {:?}", &text[..16]);

    let doc = Document::open(bytes).unwrap();
    let catalog = doc.catalog().unwrap();
    let Some(cos::Object::Reference(m)) = catalog.get(&cos::Name::from("Metadata")) else {
        panic!("no /Metadata");
    };
    let cos::Object::Stream(s) = doc.get(*m).unwrap() else {
        panic!("not a stream");
    };
    let xmp = String::from_utf8_lossy(s.raw().as_ref()).into_owned();
    assert!(xmp.contains("<pdfaid:part>1</pdfaid:part>"));
    assert!(xmp.contains("<pdfaid:conformance>B</pdfaid:conformance>"));

    // A soft-masked (alpha) image is transparency — forbidden by PDF/A-1, fine at PDF/A-2.
    let img = prismpdf::Image::from_rgba(1, 1, vec![0xFF, 0x00, 0x00, 0x80]).unwrap();
    let mut flow = Flow::new(PageStyle::default(), &[]);
    flow.image(&img, 72.0, 72.0);
    let mut alpha = flow.into_builder();
    assert!(matches!(
        make_pdfa(&mut alpha, PdfAConformance::A1b, &XmpMetadata::default()),
        Err(prismpdf::Error::PdfA(PdfAError::TransparencyRequiresPdfA2))
    ));
    assert!(make_pdfa(&mut alpha, PdfAConformance::A2b, &XmpMetadata::default()).is_ok());
}

#[test]
fn make_pdfa_rejects_unembedded_fonts() {
    let mut builder = Builder::new();
    builder.add_page(
        PageSpec::new(b"BT /F1 12 Tf (hi) Tj ET".to_vec()).standard_font("F1", StdFont::Helvetica),
    );
    let err = make_pdfa(&mut builder, PdfAConformance::A2b, &XmpMetadata::default());
    // The facade now surfaces the unified `prismpdf::Error`; the precise cause is still matchable.
    assert!(matches!(
        err,
        Err(prismpdf::Error::PdfA(PdfAError::UnembeddedFont))
    ));
}
