//! Font embedding, reporting and subsetting through the facade (§9.6–§9.9).

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::{assemble, stream_with};
use prismpdf::{
    Align, Document, Flow, FontProgramFormat, PageStyle, TextBlock, document_fonts, page_text,
    shape_text, subset_fonts,
};

/// A Type0 font with `Identity-H` encoding and an embedded TrueType program, but **no**
/// `/ToUnicode` — the long-tail case M9 (§9.7) recovers by reversing the font's own `cmap`.
/// The content shows the glyph ids for `text` directly (Identity-H = 2-byte CID = GID).
#[test]
fn type0_identity_h_without_tounicode_extracts_via_cmap() {
    let Some(font) = std::fs::read("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf").ok() else {
        return; // hermetic when no system font is present
    };
    let text = "Hi";
    // Shape to glyph ids and emit them as a 2-byte-per-code hex string for Tj.
    let glyphs = shape_text(&font, text).unwrap();
    assert!(glyphs.iter().all(|g| g.id != 0), "all glyphs resolved");
    let codes: String = glyphs.iter().map(|g| format!("{:04X}", g.id)).collect();
    let content = format!("BT /F0 24 Tf <{codes}> Tj ET").into_bytes();

    let objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] \
            /Resources << /Font << /F0 5 0 R >> >> /Contents 4 0 R >>"
            .to_vec(),
        stream_with("", &content),
        // Type0 parent: Identity-H, no /ToUnicode.
        b"<< /Type /Font /Subtype /Type0 /BaseFont /DejaVuSans /Encoding /Identity-H \
            /DescendantFonts [6 0 R] >>"
            .to_vec(),
        // CIDFontType2 descendant with Identity CIDToGIDMap.
        b"<< /Type /Font /Subtype /CIDFontType2 /BaseFont /DejaVuSans \
            /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> \
            /FontDescriptor 7 0 R /CIDToGIDMap /Identity >>"
            .to_vec(),
        b"<< /Type /FontDescriptor /FontName /DejaVuSans /Flags 4 \
            /FontBBox [0 0 1000 1000] /ItalicAngle 0 /Ascent 800 /Descent -200 \
            /CapHeight 700 /StemV 80 /FontFile2 8 0 R >>"
            .to_vec(),
        stream_with(&format!("/Length1 {}", font.len()), &font),
    ];

    let doc = Document::open(assemble(&objects, "")).unwrap();
    let extracted = page_text(&doc, 0).unwrap().unwrap();
    assert_eq!(extracted, text, "Type0 text recovered via embedded cmap");
}

#[test]
fn embed_font_round_trips_non_latin_text() {
    // Embed a real TrueType font and write Cyrillic; it must extract back via /ToUnicode.
    let Some(font) = std::fs::read("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf").ok() else {
        return; // hermetic when no system font is present
    };
    let mut flow = Flow::new(PageStyle::default(), &[]);
    assert!(flow.embed_font("F1", &font));
    let block = TextBlock {
        font_resource: "F1",
        base_font: "",
        size: 16.0,
        leading: 20.0,
        align: Align::Left,
    };
    let message = "Привет мир"; // "Hello world" in Russian
    flow.text(&block, message);
    let pdf = flow.build();

    let doc = Document::open(pdf).unwrap();
    let text = page_text(&doc, 0).unwrap().unwrap();
    assert!(text.contains("Привет"), "extracted {text:?}");
}

#[test]
fn document_fonts_reports_simple_and_embedded() {
    let program = b"fake truetype program bytes";
    let mut ff2 = format!("<< /Length {} >>\nstream\n", program.len()).into_bytes();
    ff2.extend_from_slice(program);
    ff2.extend_from_slice(b"\nendstream");

    let objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Resources << /Font << /F1 4 0 R /F2 5 0 R >> >> >>"
            .to_vec(),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
        b"<< /Type /Font /Subtype /TrueType /BaseFont /ABCDEF+MyFont /FontDescriptor 6 0 R >>"
            .to_vec(),
        b"<< /Type /FontDescriptor /FontName /ABCDEF+MyFont /FontFile2 7 0 R >>".to_vec(),
        ff2,
    ];
    let doc = Document::open(assemble(&objects, "")).unwrap();
    let fonts = document_fonts(&doc).unwrap();
    assert_eq!(fonts.len(), 2);

    let helvetica = fonts.iter().find(|f| f.base_font == "Helvetica").unwrap();
    assert_eq!(helvetica.subtype, "Type1");
    assert!(helvetica.embedded.is_none());

    let embedded = fonts
        .iter()
        .find(|f| f.base_font == "ABCDEF+MyFont")
        .unwrap();
    assert_eq!(embedded.subtype, "TrueType");
    let program_info = embedded.embedded.as_ref().unwrap();
    assert_eq!(program_info.format, FontProgramFormat::TrueType);
    assert_eq!(program_info.program, program);
    assert!(program_info.metrics.is_none()); // not a real sfnt
}

#[test]
fn subset_fonts_shrinks_an_embedded_opentype_fontfile3() {
    // §9.9 Table 127: an OpenType (CFF-outline) program embedded as /FontFile3 /Subtype /OpenType
    // on a simple TrueType font subsets through the same path as /FontFile2.
    let Some(otf) = [
        "/usr/share/fonts/opentype/urw-base35/NimbusRoman-Regular.otf",
        "/usr/share/fonts/opentype/urw-base35/C059-Roman.otf",
    ]
    .iter()
    .find_map(|p| std::fs::read(p).ok()) else {
        return; // hermetic when no OTF is present
    };
    let objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".to_vec(),
        stream_with("", b"BT /F1 12 Tf (Hi) Tj ET"),
        b"<< /Type /Font /Subtype /TrueType /BaseFont /Nimbus /Encoding /WinAnsiEncoding /FontDescriptor 6 0 R >>".to_vec(),
        b"<< /Type /FontDescriptor /FontName /Nimbus /FontFile3 7 0 R >>".to_vec(),
        stream_with("/Subtype /OpenType", &otf),
    ];
    let doc = Document::open(assemble(&objects, "")).unwrap();
    let out = subset_fonts(&doc).unwrap();

    let reopened = Document::open(out).unwrap();
    let font = document_fonts(&reopened)
        .unwrap()
        .into_iter()
        .find(|f| f.subtype == "TrueType")
        .unwrap();
    let embedded = font.embedded.unwrap();
    assert_eq!(
        embedded.format,
        FontProgramFormat::OpenType,
        "stays /OpenType"
    );
    assert!(
        embedded.program.len() < otf.len(),
        "subset should be smaller ({} vs {})",
        embedded.program.len(),
        otf.len()
    );
    let metrics = embedded.metrics.unwrap();
    assert!(
        metrics.glyph_count <= 5,
        "only the used glyphs survive (got {})",
        metrics.glyph_count
    );
    assert!(metrics.glyph_count >= 3); // .notdef + H + i
}

#[test]
fn subset_fonts_shrinks_an_embedded_truetype() {
    let Some(ttf) = std::fs::read("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf").ok() else {
        return; // hermetic when no system font
    };
    // Embed DejaVuSans as a simple TrueType font; the page shows only "Hi".
    let objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".to_vec(),
        stream_with("", b"BT /F1 12 Tf (Hi) Tj ET"),
        b"<< /Type /Font /Subtype /TrueType /BaseFont /DejaVuSans /Encoding /WinAnsiEncoding /FontDescriptor 6 0 R >>".to_vec(),
        b"<< /Type /FontDescriptor /FontName /DejaVuSans /FontFile2 7 0 R >>".to_vec(),
        stream_with(&format!("/Length1 {}", ttf.len()), &ttf),
    ];
    let doc = Document::open(assemble(&objects, "")).unwrap();
    let out = subset_fonts(&doc).unwrap();

    // The re-embedded program is dramatically smaller but still a valid font with few glyphs.
    let reopened = Document::open(out).unwrap();
    let font = document_fonts(&reopened)
        .unwrap()
        .into_iter()
        .find(|f| f.subtype == "TrueType")
        .unwrap();
    let embedded = font.embedded.unwrap();
    assert!(
        embedded.program.len() < ttf.len() / 10,
        "subset should be far smaller"
    );
    let metrics = embedded.metrics.unwrap();
    assert!(
        metrics.glyph_count <= 5,
        "only the used glyphs survive (got {})",
        metrics.glyph_count
    );
    assert!(metrics.glyph_count >= 3); // .notdef + H + i
}
