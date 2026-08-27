//! XMP metadata production (ISO 32000-1 §14.3.2) with the PDF/A identification schema.
//!
//! Builds the XMP packet that PDF/A requires as the document catalog's `/Metadata` stream: a
//! standard `x:xmpmeta` / `rdf:RDF` document carrying Dublin Core (`dc:`), XMP Basic (`xmp:`),
//! the PDF schema (`pdf:`) and the PDF/A identification schema (`pdfaid:part`/`pdfaid:conformance`).
//! The packet is plain UTF-8 text; the conformant-production pass wraps it in an unfiltered
//! `/Type /Metadata /Subtype /XML` stream.

/// The PDF/A conformance Prism PDF can declare. Parts 1–3 carry a conformance level: B = visual
/// fidelity, U = B + Unicode text, A = accessible (Tagged PDF, the most demanding). Part 4
/// (ISO 19005-4, on PDF 2.0) has no levels; its `E`/`F` variants extend the base permissions
/// (engineering / embedded files). The variant selects `pdfaid:part` and `pdfaid:conformance`
/// (and, for part 4, `pdfaid:rev`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PdfAConformance {
    /// PDF/A-1B (ISO 19005-1, level B — visual fidelity, on PDF 1.4).
    A1b,
    /// PDF/A-1A (ISO 19005-1, level A — tagged logical structure, on PDF 1.4).
    A1a,
    /// PDF/A-2B (ISO 19005-2, level B — visual fidelity).
    A2b,
    /// PDF/A-2U (ISO 19005-2, level U — B plus all text mapped to Unicode).
    A2u,
    /// PDF/A-2A (ISO 19005-2, level A — U plus a tagged logical structure).
    A2a,
    /// PDF/A-3B (ISO 19005-3, level B — like 2B, allows embedded files).
    A3b,
    /// PDF/A-3U (ISO 19005-3, level U — 3B plus all text mapped to Unicode).
    A3u,
    /// PDF/A-3A (ISO 19005-3, level A — tagged, allows embedded files).
    A3a,
    /// PDF/A-4 (ISO 19005-4, on PDF 2.0 — no conformance levels).
    A4,
    /// PDF/A-4E (ISO 19005-4 Annex B, engineering — permits 3D/RichMedia and embedded files).
    A4e,
    /// PDF/A-4F (ISO 19005-4 Annex A — permits embedded files of any type).
    A4f,
}

impl PdfAConformance {
    /// The ISO 19005 part number (`pdfaid:part`): 1, 2, 3 or 4.
    #[must_use]
    pub fn part(self) -> u8 {
        match self {
            PdfAConformance::A1b | PdfAConformance::A1a => 1,
            PdfAConformance::A2b | PdfAConformance::A2u | PdfAConformance::A2a => 2,
            PdfAConformance::A3b | PdfAConformance::A3u | PdfAConformance::A3a => 3,
            PdfAConformance::A4 | PdfAConformance::A4e | PdfAConformance::A4f => 4,
        }
    }

    /// The conformance identifier letter (`pdfaid:conformance`): `A`, `B`, or `U` for parts 1–3;
    /// `E` or `F` for the part-4 extensions. `None` for plain PDF/A-4, which declares no
    /// conformance key in its identification schema (ISO 19005-4).
    #[must_use]
    pub fn level(self) -> Option<char> {
        match self {
            PdfAConformance::A1b | PdfAConformance::A2b | PdfAConformance::A3b => Some('B'),
            PdfAConformance::A2u | PdfAConformance::A3u => Some('U'),
            PdfAConformance::A1a | PdfAConformance::A2a | PdfAConformance::A3a => Some('A'),
            PdfAConformance::A4 => None,
            PdfAConformance::A4e => Some('E'),
            PdfAConformance::A4f => Some('F'),
        }
    }

    /// The flavour code as veraPDF and the corpus name it: `"2b"`, `"3u"`, `"4"`, `"4e"`, …
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            PdfAConformance::A1b => "1b",
            PdfAConformance::A1a => "1a",
            PdfAConformance::A2b => "2b",
            PdfAConformance::A2u => "2u",
            PdfAConformance::A2a => "2a",
            PdfAConformance::A3b => "3b",
            PdfAConformance::A3u => "3u",
            PdfAConformance::A3a => "3a",
            PdfAConformance::A4 => "4",
            PdfAConformance::A4e => "4e",
            PdfAConformance::A4f => "4f",
        }
    }

    /// Whether this is conformance level A — which requires a tagged logical structure (§14.7).
    #[must_use]
    pub fn is_level_a(self) -> bool {
        matches!(
            self,
            PdfAConformance::A1a | PdfAConformance::A2a | PdfAConformance::A3a
        )
    }

    /// Whether this conformance permits embedded file attachments: part 3 (any level) and the
    /// part-4 `E`/`F` extensions. Part 1, part 2 and plain PDF/A-4 forbid them.
    #[must_use]
    pub fn allows_attachments(self) -> bool {
        self.part() == 3 || matches!(self, PdfAConformance::A4e | PdfAConformance::A4f)
    }
}

/// Document metadata to render into the XMP packet. All fields are optional; only those present are
/// emitted. Mirrors the `/Info` keys (§14.3.3) so the two stay in sync.
#[derive(Clone, Debug, Default)]
pub struct XmpMetadata {
    /// `dc:title`.
    pub title: Option<String>,
    /// `dc:creator` (one entry per author).
    pub authors: Vec<String>,
    /// `dc:description` (the document's subject).
    pub subject: Option<String>,
    /// `pdf:Keywords`.
    pub keywords: Option<String>,
    /// `xmp:CreatorTool` (the authoring application).
    pub creator_tool: Option<String>,
    /// `pdf:Producer`.
    pub producer: Option<String>,
    /// `xmp:CreateDate`, as a W3C-datetime / ISO 8601 string (e.g. `2026-06-20T10:00:00Z`).
    pub create_date: Option<String>,
    /// `xmp:ModifyDate`, as a W3C-datetime / ISO 8601 string.
    pub modify_date: Option<String>,
}

/// Render the complete XMP packet (including the `xpacket` processing instructions) for `meta` at
/// the given PDF/A `conformance`. The result is valid UTF-8 XML ready to wrap in a `/Metadata`
/// stream.
#[must_use]
pub fn xmp_packet(meta: &XmpMetadata, conformance: PdfAConformance) -> String {
    render(meta, Some(conformance), None)
}

/// The PDF/UA part a document declares in its XMP identification (ISO 14289).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum PdfUaId {
    /// PDF/UA-1 (ISO 14289-1:2014, PDF 1.7 base): `pdfuaid:part` 1.
    Part1,
    /// PDF/UA-2 (ISO 14289-2:2024, PDF 2.0 base): `pdfuaid:part` 2 **plus** `pdfuaid:rev`, the
    /// four-digit year of the standard's revision (Table 1 of ISO 14289-2 requires both).
    Part2,
}

/// As [`xmp_packet`], but for a **PDF/UA-1** document (ISO 14289-1): emits the `pdfuaid:part` 1
/// identification instead of the PDF/A schema. Used by the PDF/UA production pass.
#[must_use]
pub fn xmp_packet_ua(meta: &XmpMetadata) -> String {
    render(meta, None, Some(PdfUaId::Part1))
}

/// As [`xmp_packet`], but for a **PDF/UA-2** document (ISO 14289-2:2024): emits `pdfuaid:part` 2
/// and the required `pdfuaid:rev` revision year (§5, Table 1).
#[must_use]
pub fn xmp_packet_ua2(meta: &XmpMetadata) -> String {
    render(meta, None, Some(PdfUaId::Part2))
}

/// Render the XMP packet, optionally carrying the PDF/A identification (`pdfa`) and/or a PDF/UA
/// identification (`pdfua`). With `Some(conformance), None` the output is the PDF/A packet.
fn render(meta: &XmpMetadata, pdfa: Option<PdfAConformance>, pdfua: Option<PdfUaId>) -> String {
    let mut s = String::new();
    // The xpacket header begins with a UTF-8 BOM (U+FEFF) per the XMP spec.
    s.push_str("<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n");
    s.push_str("<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n");
    s.push_str(" <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n");
    s.push_str("  <rdf:Description rdf:about=\"\"\n");
    s.push_str("      xmlns:dc=\"http://purl.org/dc/elements/1.1/\"\n");
    s.push_str("      xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\"\n");
    s.push_str("      xmlns:pdf=\"http://ns.adobe.com/pdf/1.3/\"");
    if pdfa.is_some() {
        s.push_str("\n      xmlns:pdfaid=\"http://www.aiim.org/pdfa/ns/id/\"");
    }
    if pdfua.is_some() {
        s.push_str("\n      xmlns:pdfuaid=\"http://www.aiim.org/pdfua/ns/id/\"");
    }
    s.push_str(">\n");

    if let Some(title) = &meta.title {
        s.push_str("   <dc:title><rdf:Alt><rdf:li xml:lang=\"x-default\">");
        s.push_str(&escape(title));
        s.push_str("</rdf:li></rdf:Alt></dc:title>\n");
    }
    if !meta.authors.is_empty() {
        s.push_str("   <dc:creator><rdf:Seq>");
        for author in &meta.authors {
            s.push_str("<rdf:li>");
            s.push_str(&escape(author));
            s.push_str("</rdf:li>");
        }
        s.push_str("</rdf:Seq></dc:creator>\n");
    }
    if let Some(subject) = &meta.subject {
        s.push_str("   <dc:description><rdf:Alt><rdf:li xml:lang=\"x-default\">");
        s.push_str(&escape(subject));
        s.push_str("</rdf:li></rdf:Alt></dc:description>\n");
    }
    if let Some(tool) = &meta.creator_tool {
        push_simple(&mut s, "xmp:CreatorTool", tool);
    }
    if let Some(date) = &meta.create_date {
        push_simple(&mut s, "xmp:CreateDate", date);
    }
    if let Some(date) = &meta.modify_date {
        push_simple(&mut s, "xmp:ModifyDate", date);
    }
    if let Some(producer) = &meta.producer {
        push_simple(&mut s, "pdf:Producer", producer);
    }
    if let Some(keywords) = &meta.keywords {
        push_simple(&mut s, "pdf:Keywords", keywords);
    }

    // PDF/A identification schema — the entries that mark the file as PDF/A. Part 4 (ISO
    // 19005-4:2020) additionally requires `pdfaid:rev`, the four-digit year of the standard's
    // revision, and declares `pdfaid:conformance` only for its E/F extensions.
    if let Some(conformance) = pdfa {
        s.push_str(&format!(
            "   <pdfaid:part>{}</pdfaid:part>\n",
            conformance.part()
        ));
        if let Some(level) = conformance.level() {
            s.push_str(&format!(
                "   <pdfaid:conformance>{level}</pdfaid:conformance>\n"
            ));
        }
        if conformance.part() == 4 {
            s.push_str("   <pdfaid:rev>2020</pdfaid:rev>\n");
        }
    }
    // PDF/UA identification (ISO 14289-1/-2). Part 2 also requires `pdfuaid:rev`, the four-digit
    // year of the standard's revision (ISO 14289-2 §5, Table 1) — 2024 for ISO 14289-2:2024.
    match pdfua {
        Some(PdfUaId::Part1) => s.push_str("   <pdfuaid:part>1</pdfuaid:part>\n"),
        Some(PdfUaId::Part2) => {
            s.push_str("   <pdfuaid:part>2</pdfuaid:part>\n");
            s.push_str("   <pdfuaid:rev>2024</pdfuaid:rev>\n");
        }
        None => {}
    }

    s.push_str("  </rdf:Description>\n");
    s.push_str(" </rdf:RDF>\n");
    s.push_str("</x:xmpmeta>\n");
    // Trailing padding is conventional (lets editors grow the packet in place); `end="w"` =
    // writable. We keep it minimal.
    s.push_str("<?xpacket end=\"w\"?>");
    s
}

/// Emit a simple `<tag>value</tag>` element with escaped text.
fn push_simple(s: &mut String, tag: &str, value: &str) {
    s.push_str(&format!("   <{tag}>{}</{tag}>\n", escape(value)));
}

/// XML-escape text content, dropping control characters that XML 1.0 forbids (keeping tab/CR/LF).
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            '\t' | '\n' | '\r' => out.push(ch),
            c if (c as u32) < 0x20 => {} // disallowed control char
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conformance_maps_to_part_and_level() {
        use PdfAConformance::{A1a, A1b, A2b, A2u, A3b, A3u, A4, A4e, A4f};
        for (c, part, level, code) in [
            (A1b, 1, Some('B'), "1b"),
            (A1a, 1, Some('A'), "1a"),
            (A2b, 2, Some('B'), "2b"),
            (A2u, 2, Some('U'), "2u"),
            (A3b, 3, Some('B'), "3b"),
            (A3u, 3, Some('U'), "3u"),
            (A4, 4, None, "4"),
            (A4e, 4, Some('E'), "4e"),
            (A4f, 4, Some('F'), "4f"),
        ] {
            assert_eq!((c.part(), c.level(), c.code()), (part, level, code));
        }
        // Level U is not level A — it needs no tagging; A1a is level A like A2a/A3a.
        assert!(!PdfAConformance::A3u.is_level_a());
        assert!(PdfAConformance::A1a.is_level_a());
        // Attachments: part 3 and the part-4 E/F extensions only.
        assert!(A3b.allows_attachments());
        assert!(A4f.allows_attachments());
        assert!(A4e.allows_attachments());
        assert!(!A4.allows_attachments());
        assert!(!A2b.allows_attachments());
        assert!(!A1b.allows_attachments());
    }

    #[test]
    fn part4_packet_declares_rev_and_optional_conformance() {
        // Plain PDF/A-4: part + rev, and no pdfaid:conformance key at all (ISO 19005-4).
        let xmp = xmp_packet(&XmpMetadata::default(), PdfAConformance::A4);
        assert!(xmp.contains("<pdfaid:part>4</pdfaid:part>"));
        assert!(xmp.contains("<pdfaid:rev>2020</pdfaid:rev>"));
        assert!(!xmp.contains("pdfaid:conformance"));
        // The F extension declares conformance F; E declares E.
        let xmp_f = xmp_packet(&XmpMetadata::default(), PdfAConformance::A4f);
        assert!(xmp_f.contains("<pdfaid:conformance>F</pdfaid:conformance>"));
        assert!(xmp_f.contains("<pdfaid:rev>2020</pdfaid:rev>"));
        let xmp_e = xmp_packet(&XmpMetadata::default(), PdfAConformance::A4e);
        assert!(xmp_e.contains("<pdfaid:conformance>E</pdfaid:conformance>"));
        // Parts 1–3 declare no rev.
        let xmp_2 = xmp_packet(&XmpMetadata::default(), PdfAConformance::A2b);
        assert!(!xmp_2.contains("pdfaid:rev"));
        let xmp_1 = xmp_packet(&XmpMetadata::default(), PdfAConformance::A1b);
        assert!(xmp_1.contains("<pdfaid:part>1</pdfaid:part>"));
        assert!(xmp_1.contains("<pdfaid:conformance>B</pdfaid:conformance>"));
    }

    #[test]
    fn packet_has_pdfa_identification_and_metadata() {
        let meta = XmpMetadata {
            title: Some("My Report".into()),
            authors: vec!["Jane Doe".into(), "John Roe".into()],
            subject: Some("Quarterly results".into()),
            producer: Some("Prism PDF".into()),
            ..Default::default()
        };
        let xmp = xmp_packet(&meta, PdfAConformance::A2b);
        assert!(xmp.starts_with("<?xpacket begin="));
        assert!(xmp.trim_end().ends_with("<?xpacket end=\"w\"?>"));
        assert!(xmp.contains("<pdfaid:part>2</pdfaid:part>"));
        assert!(xmp.contains("<pdfaid:conformance>B</pdfaid:conformance>"));
        assert!(xmp.contains("xml:lang=\"x-default\">My Report</rdf:li>"));
        assert!(xmp.contains("<rdf:li>Jane Doe</rdf:li><rdf:li>John Roe</rdf:li>"));
        assert!(xmp.contains("<pdf:Producer>Prism PDF</pdf:Producer>"));
    }

    #[test]
    fn empty_metadata_still_marks_pdfa() {
        let xmp = xmp_packet(&XmpMetadata::default(), PdfAConformance::A3b);
        assert!(xmp.contains("<pdfaid:part>3</pdfaid:part>"));
        assert!(!xmp.contains("<dc:title>"));
    }

    #[test]
    fn ua_packets_declare_their_part() {
        let ua1 = xmp_packet_ua(&XmpMetadata::default());
        assert!(ua1.contains("<pdfuaid:part>1</pdfuaid:part>"));
        assert!(!ua1.contains("pdfuaid:rev"), "rev is UA-2 only");
        // UA-2 (ISO 14289-2:2024 §5, Table 1) requires part 2 plus the 4-digit revision year.
        let ua2 = xmp_packet_ua2(&XmpMetadata::default());
        assert!(ua2.contains("<pdfuaid:part>2</pdfuaid:part>"));
        assert!(ua2.contains("<pdfuaid:rev>2024</pdfuaid:rev>"));
        assert!(ua2.contains("xmlns:pdfuaid="));
        assert!(!ua2.contains("pdfaid:part"), "no PDF/A identification");
    }

    #[test]
    fn special_characters_are_escaped() {
        let meta = XmpMetadata {
            title: Some("A & B <tag> \"q\"".into()),
            ..Default::default()
        };
        let xmp = xmp_packet(&meta, PdfAConformance::A2b);
        assert!(xmp.contains("A &amp; B &lt;tag&gt; &quot;q&quot;"));
        assert!(!xmp.contains("<tag>"));
    }
}
