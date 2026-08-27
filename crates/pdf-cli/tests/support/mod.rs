//! Shared fixtures for the `prismpdf` tests: scratch directories, and the sample PDFs the
//! subcommands are pointed at.
//!
//! Every integration-test binary compiles this module in full, so unused helpers are expected
//! per-file.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use prismpdf::{AnnotationSpec, Attachment, Builder, FormFieldSpec, LinkTarget, PageSpec};

/// The throwaway self-signed test signer (see `crates/pdf/examples/test-signer/README.md`).
/// Committed in the clear on purpose; it signs nothing of value.
pub const TEST_CERT: &[u8] = include_bytes!("../../../pdf/examples/test-signer/cert.der");
/// The matching PKCS#8 RSA-2048 private key.
pub const TEST_KEY: &[u8] = include_bytes!("../../../pdf/examples/test-signer/key.der");

/// A temporary directory that removes itself when the test ends — including on panic, so a failing
/// assertion never leaves files behind.
pub struct Scratch {
    dir: PathBuf,
}

impl Scratch {
    /// Create a scratch directory unique to this process and call site.
    pub fn new(tag: &str) -> Self {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("prismpdf-test-{tag}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create scratch dir");
        Self { dir }
    }

    /// The directory itself.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// A path inside the directory. The file need not exist.
    pub fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    /// Write `bytes` to `name` inside the directory and return its path.
    pub fn write(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.path(name);
        std::fs::write(&path, bytes).expect("write scratch file");
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Assemble a classic-xref PDF from object bodies (object `i+1` ← `objects[i]`), with optional
/// extra trailer entries. Offsets are computed so the file is valid (§7.5.4).
pub fn assemble(objects: &[Vec<u8>], trailer_extra: &str) -> Vec<u8> {
    let mut buf = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in objects.iter().enumerate() {
        offsets.push(buf.len());
        buf.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        buf.extend_from_slice(body);
        buf.extend_from_slice(b"\nendobj\n");
    }
    let startxref = buf.len();
    buf.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
    );
    for off in &offsets {
        buf.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
    }
    buf.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R {trailer_extra} >>\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    buf.extend_from_slice(format!("startxref\n{startxref}\n%%EOF\n").as_bytes());
    buf
}

/// Wrap `content` as an unfiltered stream object body with a correct `/Length` and extra entries.
pub fn stream_obj(extra: &str, content: &[u8]) -> Vec<u8> {
    let mut body = format!("<< {extra} /Length {} >>\nstream\n", content.len()).into_bytes();
    body.extend_from_slice(content);
    body.extend_from_slice(b"\nendstream");
    body
}

/// A one-page PDF with an `/Info` title and a content stream that shows text.
pub fn text_pdf() -> Vec<u8> {
    assemble(
        &[
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>".to_vec(),
            stream_obj("", b"BT /F1 12 Tf (Hello CLI) Tj ET"),
            b"<< /Title (Doc Title) /Author (Tester) >>".to_vec(),
        ],
        "/Info 5 0 R",
    )
}

/// A one-page PDF whose page references a 2×1 unfiltered RGB image XObject (§8.9).
pub fn image_pdf() -> Vec<u8> {
    assemble(
        &[
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /Resources << /XObject << /Im0 4 0 R >> >> >>".to_vec(),
            stream_obj(
                "/Type /XObject /Subtype /Image /Width 2 /Height 1 /BitsPerComponent 8 \
                 /ColorSpace /DeviceRGB",
                &[255, 0, 0, 0, 255, 0],
            ),
        ],
        "",
    )
}

/// A one-page PDF carrying a nested outline (§12.3.3) and a text form field with a value (§12.7):
/// the two shapes the `Builder` cannot author, and the ones `outline` and `fill` need.
pub fn forms_and_outline_pdf() -> Vec<u8> {
    assemble(
        &[
            b"<< /Type /Catalog /Pages 2 0 R /Outlines 4 0 R /AcroForm << /Fields [7 0 R] >> >>"
                .to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Annots [7 0 R] >>".to_vec(),
            // 4: the outline root, 5: a top-level item owning 6: a child.
            b"<< /Type /Outlines /First 5 0 R /Last 5 0 R /Count 2 >>".to_vec(),
            b"<< /Title (Chapter) /Parent 4 0 R /Dest [3 0 R /Fit] /First 6 0 R /Last 6 0 R >>"
                .to_vec(),
            b"<< /Title (Section) /Parent 5 0 R /Dest [3 0 R /Fit] >>".to_vec(),
            // 7: a text field that is also its own widget annotation (§12.7.3.3).
            b"<< /Type /Annot /Subtype /Widget /FT /Tx /T (subject) /V (before) /Rect [10 10 190 40] /F 4 >>"
                .to_vec(),
        ],
        "",
    )
}

/// A PDF authored through the facade: two pages, an outline, a link and a note annotation, a
/// checkbox field, an attachment and an XMP packet — everything the listing subcommands report.
pub fn rich_pdf() -> Vec<u8> {
    let mut builder = Builder::new();
    builder
        .title("Rich Fixture")
        .author("Tester")
        .add_page(
            PageSpec::new("BT /F1 12 Tf (page one) Tj ET")
                .standard_font("F1", prismpdf::StdFont::Helvetica),
        )
        .add_page(
            PageSpec::new("BT /F1 12 Tf (page two) Tj ET")
                .standard_font("F1", prismpdf::StdFont::Helvetica),
        )
        .outline("First page", 0)
        .outline("Second page", 1)
        .add_annotation(
            0,
            AnnotationSpec::Link {
                rect: [10.0, 10.0, 100.0, 30.0],
                target: LinkTarget::Uri("https://example.invalid/".to_string()),
                contents: Some("Example link".to_string()),
            },
            Vec::new(),
        )
        .add_annotation(
            0,
            AnnotationSpec::Link {
                rect: [10.0, 110.0, 100.0, 130.0],
                target: LinkTarget::Page(1),
                contents: None,
            },
            Vec::new(),
        )
        .add_annotation(
            1,
            AnnotationSpec::Note {
                rect: [10.0, 40.0, 30.0, 60.0],
                contents: "A note to self".to_string(),
            },
            Vec::new(),
        )
        .add_form_field(
            0,
            FormFieldSpec::Checkbox {
                rect: [10.0, 70.0, 30.0, 90.0],
                name: "agree".to_string(),
                checked: false,
                tooltip: Some("Agree to the terms".to_string()),
            },
            Vec::new(),
        )
        .attach_file(Attachment {
            name: "data/invoice.xml".to_string(),
            mime: "text/xml".to_string(),
            relationship: "Data".to_string(),
            description: Some("The source invoice".to_string()),
            mod_date: None,
            data: b"<invoice n=\"1\"/>".to_vec(),
        })
        .metadata_xmp(
            b"<?xpacket begin=\"\xef\xbb\xbf\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\
              <x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF \
              xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\
              <rdf:Description rdf:about=\"\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\">\
              <dc:title><rdf:Alt><rdf:li xml:lang=\"x-default\">Rich Fixture</rdf:li>\
              </rdf:Alt></dc:title></rdf:Description></rdf:RDF></x:xmpmeta><?xpacket end=\"w\"?>"
                .to_vec(),
        );
    builder.build()
}

/// A minimal one-page PDF with nothing else in it: no fonts, annotations, fields, outline,
/// attachments or metadata — the "empty" branch of every listing subcommand.
pub fn bare_pdf() -> Vec<u8> {
    Builder::new().add_page(PageSpec::new("")).build()
}

/// A one-page PDF embedding a system TrueType font uncompressed, or `None` when the host has no
/// font to embed (the font-dependent tests then no-op, staying hermetic).
pub fn embedded_font_pdf() -> Option<Vec<u8>> {
    let ttf = system_font()?;
    Some(assemble(
        &[
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /Resources << /Font << /F1 4 0 R >> >> >>".to_vec(),
            b"<< /Type /Font /Subtype /TrueType /BaseFont /Embedded /FontDescriptor 5 0 R >>"
                .to_vec(),
            b"<< /Type /FontDescriptor /FontName /Embedded /FontFile2 6 0 R >>".to_vec(),
            stream_obj(&format!("/Length1 {}", ttf.len()), &ttf),
        ],
        "",
    ))
}

/// A TrueType font from the host, if one of the usual suspects is installed.
pub fn system_font() -> Option<Vec<u8>> {
    [
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
    ]
    .iter()
    .find_map(|p| std::fs::read(p).ok())
}

/// A one-page PDF whose fonts and `/Info` sit in the corners of what the listing reports: a font
/// with no `/BaseFont` and no descriptor, a font whose embedded program is unparseable, and a
/// `/Title` that is not a text string (§7.9.2).
pub fn odd_fonts_pdf() -> Vec<u8> {
    assemble(
        &[
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /Resources << /Font << /F1 4 0 R /F2 5 0 R >> >> >>"
                .to_vec(),
            b"<< /Type /Font /Subtype /Type1 >>".to_vec(),
            b"<< /Type /Font /Subtype /TrueType /BaseFont /Broken /FontDescriptor 6 0 R >>"
                .to_vec(),
            b"<< /Type /FontDescriptor /FontName /Broken /FontFile2 7 0 R >>".to_vec(),
            // An sfnt magic number followed by nothing usable: the format is recognised, the
            // metrics are not.
            stream_obj("/Length1 8", b"\x00\x01\x00\x00trunc"),
            b"<< /Title 42 >>".to_vec(),
        ],
        "/Info 8 0 R",
    )
}

/// A PDF whose `%PDF-M.m` header line has been cut off (§7.5.2). The reader recovers it by scanning
/// for objects (DESIGN.md §3: recovery is first-class), but the file declares no version.
pub fn headerless_pdf() -> Vec<u8> {
    let mut bytes = text_pdf();
    let first_line = bytes
        .iter()
        .position(|&b| b == b'\n')
        .expect("the header ends in a newline");
    bytes.drain(0..=first_line);
    bytes
}
