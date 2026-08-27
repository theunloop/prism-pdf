//! Text and image extraction through the facade (§9.4 text, §8.9 images), including recursion
//! into form XObjects and geometry-ordered text.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::{assemble, stream_obj, stream_with, unhex};
use prismpdf::{
    ColorSpace, Document, Flow, Image, ImageData, PageStyle, document_text, page_images, page_text,
    page_text_positioned,
};

#[test]
fn page_text_includes_annotation_contents() {
    // Text living in an annotation (§12.5.2) is appended to the page's content-stream text, so it
    // is no longer lost. The annotation /Contents here is a PDF 2.0 UTF-8 string (EF BB BF BOM).
    let utf8_note = {
        let mut s = b"<< /Type /Annot /Subtype /Text /Rect [0 0 10 10] /Contents (".to_vec();
        s.extend_from_slice(&[0xEF, 0xBB, 0xBF]); // UTF-8 BOM
        s.extend_from_slice("café".as_bytes());
        s.extend_from_slice(b") >>");
        s
    };
    let objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Annots [5 0 R] >>".to_vec(),
        stream_obj(b"BT (Body text) Tj ET"),
        utf8_note,
    ];
    let doc = Document::open(assemble(&objects, "")).unwrap();
    let text = page_text(&doc, 0).unwrap().unwrap();
    assert!(text.contains("Body text"), "content text missing: {text:?}");
    assert!(text.contains("café"), "annotation text missing: {text:?}");
}

#[test]
fn page_text_decodes_via_to_unicode() {
    // Font F1 maps bytes 0x01/0x02/0x03 to P/D/F via /ToUnicode; Latin-1 would differ.
    let cmap = b"begincmap\n1 begincodespacerange <00> <FF> endcodespacerange\n\
        3 beginbfchar <01> <0050> <02> <0044> <03> <0046> endbfchar\nendcmap";
    let objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>"
            .to_vec(),
        stream_obj(b"BT /F1 12 Tf (\x01\x02\x03) Tj ET"),
        b"<< /Type /Font /Subtype /Type1 /ToUnicode 6 0 R >>".to_vec(),
        stream_obj(cmap),
    ];
    let doc = Document::open(assemble(&objects, "")).unwrap();
    assert_eq!(page_text(&doc, 0).unwrap().as_deref(), Some("PDF"));
    assert_eq!(document_text(&doc).unwrap(), "PDF");
    // Out-of-range page index.
    assert_eq!(page_text(&doc, 9).unwrap(), None);
}

#[test]
fn page_text_falls_back_to_latin1_without_to_unicode() {
    // No /ToUnicode and no /Resources at all → Latin-1 decoding of ASCII content.
    let objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>".to_vec(),
        stream_obj(b"BT (Plain ASCII) Tj ET"),
    ];
    let doc = Document::open(assemble(&objects, "")).unwrap();
    assert_eq!(page_text(&doc, 0).unwrap().as_deref(), Some("Plain ASCII"));
}

#[test]
fn page_text_uses_simple_font_encoding() {
    // Font has /Encoding /WinAnsiEncoding but no /ToUnicode; byte 0x92 is a curly apostrophe in
    // WinAnsi (a raw Latin-1 fallback would emit a control character instead).
    let objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>"
            .to_vec(),
        stream_obj(b"BT /F1 12 Tf (it\x92s) Tj ET"),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>"
            .to_vec(),
    ];
    let doc = Document::open(assemble(&objects, "")).unwrap();
    assert_eq!(page_text(&doc, 0).unwrap().as_deref(), Some("it\u{2019}s"));
}

#[test]
fn page_text_recurses_into_form_xobjects() {
    // The page only invokes a form (`/Fm0 Do`); the text lives in the form's content stream.
    let objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /XObject << /Fm0 5 0 R >> >> >>".to_vec(),
        stream_with("", b"/Fm0 Do"),
        stream_with(
            "/Type /XObject /Subtype /Form /BBox [0 0 100 100] /Resources << /Font << /F1 6 0 R >> >>",
            b"BT /F1 12 Tf (text inside a form) Tj ET",
        ),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
    ];
    let doc = Document::open(assemble(&objects, "")).unwrap();
    assert_eq!(
        page_text(&doc, 0).unwrap().as_deref(),
        Some("text inside a form")
    );
}

#[test]
fn page_images_extracts_an_xobject() {
    // A page whose /Resources /XObject /Im0 is a 2×1 unfiltered RGB image.
    let samples = [255u8, 0, 0, 0, 255, 0];
    let mut img = format!(
        "<< /Type /XObject /Subtype /Image /Width 2 /Height 1 /BitsPerComponent 8 \
         /ColorSpace /DeviceRGB /Length {} >>\nstream\n",
        samples.len()
    )
    .into_bytes();
    img.extend_from_slice(&samples);
    img.extend_from_slice(b"\nendstream");

    let objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Resources << /XObject << /Im0 4 0 R >> >> >>".to_vec(),
        img,
    ];
    let doc = Document::open(assemble(&objects, "")).unwrap();

    let images = page_images(&doc, 0).unwrap();
    assert_eq!(images.len(), 1);
    assert_eq!(images[0].info.width, 2);
    assert_eq!(images[0].info.height, 1);
    assert_eq!(images[0].info.color_space, ColorSpace::DeviceRgb);
    assert_eq!(images[0].data, ImageData::Raw(samples.to_vec()));
    // A page with no XObjects yields nothing.
    assert!(page_images(&doc, 9).unwrap().is_empty());
}

#[test]
fn page_images_decodes_jbig2_with_globals() {
    // End-to-end: a page whose image XObject is JBIG2-encoded (§7.4.7) with its shared symbols in a
    // separate /JBIG2Globals stream. The facade must resolve the globals, decode, and yield 1-bpp
    // samples. Bytes are the ISO 32000-1 §7.4.7 worked example (52×66).
    let image = unhex(
        "000000013000010000001300000034000000420000000000\
         00000040000000000002062000010000001e000000340000\
         004200000000000000000200100000000231db51ce51ffac",
    );
    let globals = unhex(
        "0000000000010000000032000003fffdff02fefefe000000\
         01000000012ae225aea9a5a538b4d9999c5c8e56ef0f872\
         7f2b53d4e37ef795cc5506dffac",
    );
    let img_obj = stream_with(
        "/Type /XObject /Subtype /Image /Width 52 /Height 66 /BitsPerComponent 1 \
         /ColorSpace /DeviceGray /Filter /JBIG2Decode /DecodeParms << /JBIG2Globals 5 0 R >>",
        &image,
    );
    let globals_obj = stream_with("", &globals);

    let objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Resources << /XObject << /Im0 4 0 R >> >> >>".to_vec(),
        img_obj,
        globals_obj,
    ];
    let doc = Document::open(assemble(&objects, "")).unwrap();

    let images = page_images(&doc, 0).unwrap();
    assert_eq!(images.len(), 1);
    assert_eq!(images[0].info.width, 52);
    assert_eq!(images[0].info.bits_per_component, 1);
    match &images[0].data {
        ImageData::Raw(samples) => assert_eq!(samples.len(), 7 * 66), // 52 px → 7 bytes/row
        other => panic!("expected decoded 1-bpp JBIG2 samples, got {other:?}"),
    }
}

#[test]
fn page_images_recurses_into_form_xobjects() {
    // The image lives in a form XObject's resources, not directly on the page.
    let samples = [10u8, 20, 30, 40, 50, 60];
    let objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /XObject << /Fm0 5 0 R >> >> >>".to_vec(),
        stream_with("", b"/Fm0 Do"),
        stream_with(
            "/Type /XObject /Subtype /Form /BBox [0 0 100 100] /Resources << /XObject << /Im0 6 0 R >> >>",
            b"q Q",
        ),
        stream_with(
            "/Type /XObject /Subtype /Image /Width 2 /Height 1 /BitsPerComponent 8 /ColorSpace /DeviceRGB",
            &samples,
        ),
    ];
    let doc = Document::open(assemble(&objects, "")).unwrap();
    let images = page_images(&doc, 0).unwrap();
    assert_eq!(images.len(), 1);
    assert_eq!(images[0].info.width, 2);
    assert_eq!(images[0].data, ImageData::Raw(samples.to_vec()));
}

#[test]
fn author_an_image_round_trips() {
    let rgb = vec![255u8, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 0]; // 2×2 RGB
    let image = Image::from_rgb(2, 2, rgb.clone()).unwrap();
    let mut flow = Flow::new(PageStyle::default(), &[]);
    flow.image(&image, 144.0, 144.0);
    let pdf = flow.build();

    let doc = Document::open(pdf).unwrap();
    let images = page_images(&doc, 0).unwrap();
    assert_eq!(images.len(), 1);
    assert_eq!((images[0].info.width, images[0].info.height), (2, 2));
    assert_eq!(images[0].info.color_space, ColorSpace::DeviceRgb);
    match &images[0].data {
        ImageData::Raw(samples) => assert_eq!(samples, &rgb),
        other => panic!("expected raw samples, got {other:?}"),
    }
}

#[test]
fn page_text_positioned_orders_by_geometry() {
    // Two words emitted right-then-left on one row, plus a word on a lower row first.
    let content = b"BT 1 0 0 1 50 700 Tm (lower) Tj \
        1 0 0 1 200 760 Tm (World) Tj 1 0 0 1 50 760 Tm (Hello) Tj ET";
    let objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>".to_vec(),
        stream_with("", content),
    ];
    let doc = Document::open(assemble(&objects, "")).unwrap();
    assert_eq!(
        page_text_positioned(&doc, 0).unwrap().as_deref(),
        Some("Hello World\nlower")
    );
}
