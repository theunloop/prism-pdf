//! Building a content stream by emitting operators (ISO 32000-1 §8 / §9.4).
//!
//! The inverse of [`parse_content`](crate::parse_content): [`Content`] is a small fluent builder
//! that appends graphics-state (§8.4), path (§8.5), colour (§8.6) and text-showing (§9.4) operators
//! and hands back the raw stream bytes. It is the drawing surface for authoring a page from scratch
//! (Milestone M6) — it generates operators rather than interpreting them, so no graphics-state
//! machine is involved. Coordinates are in the current user space (default: PDF points, origin at
//! the page's bottom-left).

use pdf_cos::syntax::escape_literal_string;

/// A content-stream builder. Chain operator calls, then take the bytes with [`Content::into_bytes`].
#[derive(Clone, Debug, Default)]
pub struct Content {
    out: Vec<u8>,
}

impl Content {
    /// A new, empty content stream.
    #[must_use]
    pub fn new() -> Self {
        Content::default()
    }

    /// The bytes built so far (to embed as a page's `/Contents` stream).
    #[must_use]
    pub fn into_bytes(self) -> Vec<u8> {
        self.out
    }

    /// Borrow the bytes built so far.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.out
    }

    // --- Graphics state (§8.4) ---

    /// `q` — save the graphics state.
    pub fn save(&mut self) -> &mut Self {
        self.op(&[], "q")
    }
    /// `Q` — restore the graphics state.
    pub fn restore(&mut self) -> &mut Self {
        self.op(&[], "Q")
    }
    /// `a b c d e f cm` — concatenate a matrix onto the current transform (§8.3.4).
    pub fn transform(&mut self, a: f64, b: f64, c: f64, d: f64, e: f64, f: f64) -> &mut Self {
        self.op(&[a, b, c, d, e, f], "cm")
    }
    /// `w line_width` — set the stroke line width.
    pub fn set_line_width(&mut self, width: f64) -> &mut Self {
        self.op(&[width], "w")
    }

    // --- Colour (§8.6) ---

    /// `g gray` — set the non-stroking (fill) colour to a grey level in `[0, 1]`.
    pub fn set_fill_gray(&mut self, gray: f64) -> &mut Self {
        self.op(&[gray], "g")
    }
    /// `G gray` — set the stroking colour to a grey level.
    pub fn set_stroke_gray(&mut self, gray: f64) -> &mut Self {
        self.op(&[gray], "G")
    }
    /// `r g b rg` — set the non-stroking (fill) colour in DeviceRGB.
    pub fn set_fill_rgb(&mut self, r: f64, g: f64, b: f64) -> &mut Self {
        self.op(&[r, g, b], "rg")
    }
    /// `r g b RG` — set the stroking colour in DeviceRGB.
    pub fn set_stroke_rgb(&mut self, r: f64, g: f64, b: f64) -> &mut Self {
        self.op(&[r, g, b], "RG")
    }
    /// `c m y k k` — set the non-stroking (fill) colour in DeviceCMYK.
    pub fn set_fill_cmyk(&mut self, c: f64, m: f64, y: f64, k: f64) -> &mut Self {
        self.op(&[c, m, y, k], "k")
    }
    /// `/name cs` — set the non-stroking (fill) colour space to a named resource in the page's
    /// `/Resources /ColorSpace` (§8.6.3) — e.g. a Separation, DeviceN or ICCBased space.
    pub fn set_fill_color_space(&mut self, name: &str) -> &mut Self {
        self.out.push(b'/');
        self.out.extend_from_slice(name.as_bytes());
        self.out.extend_from_slice(b" cs\n");
        self
    }
    /// `c1 … cn scn` — set the non-stroking (fill) colour components in the current colour space
    /// (§8.6.8). For a Separation space this is a single tint value in `[0, 1]`.
    pub fn set_fill_color(&mut self, components: &[f64]) -> &mut Self {
        self.op(components, "scn")
    }

    // --- Path construction & painting (§8.5) ---

    /// `x y m` — begin a new subpath at `(x, y)`.
    pub fn move_to(&mut self, x: f64, y: f64) -> &mut Self {
        self.op(&[x, y], "m")
    }
    /// `x y l` — append a straight line segment to `(x, y)`.
    pub fn line_to(&mut self, x: f64, y: f64) -> &mut Self {
        self.op(&[x, y], "l")
    }
    /// `x1 y1 x2 y2 x3 y3 c` — append a cubic Bézier curve.
    pub fn curve_to(&mut self, x1: f64, y1: f64, x2: f64, y2: f64, x3: f64, y3: f64) -> &mut Self {
        self.op(&[x1, y1, x2, y2, x3, y3], "c")
    }
    /// `x y w h re` — append a rectangle as a complete subpath.
    pub fn rect(&mut self, x: f64, y: f64, w: f64, h: f64) -> &mut Self {
        self.op(&[x, y, w, h], "re")
    }
    /// `h` — close the current subpath.
    pub fn close_path(&mut self) -> &mut Self {
        self.op(&[], "h")
    }
    /// `S` — stroke the path.
    pub fn stroke(&mut self) -> &mut Self {
        self.op(&[], "S")
    }
    /// `f` — fill the path with the non-zero winding rule.
    pub fn fill(&mut self) -> &mut Self {
        self.op(&[], "f")
    }
    /// `B` — fill then stroke the path.
    pub fn fill_and_stroke(&mut self) -> &mut Self {
        self.op(&[], "B")
    }
    /// `W` — set the clipping path to the current path using the non-zero winding rule (§8.5.4).
    pub fn clip(&mut self) -> &mut Self {
        self.op(&[], "W")
    }
    /// `n` — end the current path without filling or stroking it (§8.5.2).
    pub fn end_path(&mut self) -> &mut Self {
        self.op(&[], "n")
    }

    // --- External objects (§8.8) ---

    /// `/name Do` — paint an XObject (image or form) named in the page's `/Resources /XObject`.
    /// Scale/position it by wrapping in [`Content::save`] / [`Content::transform`] / [`Content::restore`].
    pub fn do_xobject(&mut self, name: &str) -> &mut Self {
        self.out.push(b'/');
        self.escape_name(name);
        self.out.extend_from_slice(b" Do\n");
        self
    }

    /// `BI … ID <data> EI` — an inline image (§8.9.7), positioned by the current transform (wrap in
    /// `q`/`cm`/`Q`). `cs` is the colour-space abbreviation (`"G"`, `"RGB"` or `"CMYK"`), `bpc` the
    /// bits per component, and `data` the raw (unfiltered) samples — exactly
    /// `width × height × components × bpc / 8` bytes, rows packed MSB-first. Inline images suit only
    /// small images; use [`do_xobject`](Self::do_xobject) for anything larger.
    pub fn inline_image(
        &mut self,
        width: u32,
        height: u32,
        cs: &str,
        bpc: u32,
        data: &[u8],
    ) -> &mut Self {
        self.out.extend_from_slice(
            format!("BI /W {width} /H {height} /CS /{cs} /BPC {bpc} ID ").as_bytes(),
        );
        self.out.extend_from_slice(data);
        self.out.extend_from_slice(b"\nEI\n");
        self
    }

    // --- Text objects (§9.4) ---

    /// `BT` — begin a text object.
    pub fn begin_text(&mut self) -> &mut Self {
        self.op(&[], "BT")
    }
    /// `ET` — end the text object.
    pub fn end_text(&mut self) -> &mut Self {
        self.op(&[], "ET")
    }
    /// `/name size Tf` — set the font (a resource name) and size for the current text object.
    pub fn set_font(&mut self, name: &str, size: f64) -> &mut Self {
        self.out.push(b'/');
        self.escape_name(name);
        self.out.push(b' ');
        self.out.extend_from_slice(fmt_num(size).as_bytes());
        self.out.extend_from_slice(b" Tf\n");
        self
    }
    /// `spacing Tc` — set character spacing, added after each glyph, in unscaled text units (§9.3.2).
    pub fn set_char_spacing(&mut self, spacing: f64) -> &mut Self {
        self.op(&[spacing], "Tc")
    }
    /// `spacing Tw` — set word spacing, added at each space (code 32), in unscaled text units
    /// (§9.3.3). Used to justify lines by widening their spaces.
    pub fn set_word_spacing(&mut self, spacing: f64) -> &mut Self {
        self.op(&[spacing], "Tw")
    }
    /// `leading TL` — set the text leading (line spacing) used by [`Content::next_line`].
    pub fn set_leading(&mut self, leading: f64) -> &mut Self {
        self.op(&[leading], "TL")
    }
    /// `tx ty Td` — move to the start of the next line, offset by `(tx, ty)`.
    pub fn text_move(&mut self, tx: f64, ty: f64) -> &mut Self {
        self.op(&[tx, ty], "Td")
    }
    /// `a b c d e f Tm` — set the text matrix directly (§9.4.2).
    pub fn set_text_matrix(&mut self, a: f64, b: f64, c: f64, d: f64, e: f64, f: f64) -> &mut Self {
        self.op(&[a, b, c, d, e, f], "Tm")
    }
    /// `T*` — move to the next line using the current leading.
    pub fn next_line(&mut self) -> &mut Self {
        self.op(&[], "T*")
    }
    /// `(string) Tj` — show a literal string. `bytes` are the font's encoded codes (for a WinAnsi
    /// Standard-14 font, ASCII text is identity); the literal-string delimiters are escaped.
    pub fn show_text(&mut self, bytes: &[u8]) -> &mut Self {
        self.out.push(b'(');
        escape_literal_string(bytes, &mut self.out);
        self.out.extend_from_slice(b") Tj\n");
        self
    }
    /// Convenience for [`Content::show_text`] over a `&str` (its UTF-8 bytes — fine for ASCII; encode
    /// to the font's encoding yourself for other characters).
    pub fn show_str(&mut self, text: &str) -> &mut Self {
        self.show_text(text.as_bytes())
    }

    /// `<hhhh…> Tj` — show a run of 2-byte glyph codes, as a hex string. This is how text is shown
    /// in an `Identity-H` Type0 (composite) font, where each code is the glyph/CID (§9.4.3 / §9.7.5).
    pub fn show_glyphs(&mut self, gids: &[u16]) -> &mut Self {
        self.out.push(b'<');
        for &g in gids {
            self.out.extend_from_slice(format!("{g:04X}").as_bytes());
        }
        self.out.extend_from_slice(b"> Tj\n");
        self
    }

    // --- Marked content (§14.7.4.2 / §14.8.2) ---

    /// `/Tag << /MCID n >> BDC` — begin a marked-content sequence that a structure element refers to
    /// by its marked-content identifier `mcid` (§14.7.4.2). Pair with [`Content::end_marked_content`].
    /// `tag` is the structure tag (e.g. `P`, `H1`, `Span`); it is `#`-escaped as a name.
    pub fn begin_marked_content(&mut self, tag: &str, mcid: u32) -> &mut Self {
        self.out.push(b'/');
        self.escape_name(tag);
        self.out.extend_from_slice(b" <</MCID ");
        self.out.extend_from_slice(mcid.to_string().as_bytes());
        self.out.extend_from_slice(b">> BDC\n");
        self
    }

    /// `/AF /Name BDC` — begin a marked-content sequence whose graphics objects carry associated
    /// files (§14.13.5, **PDF 2.0**). `property` names an entry in the page's
    /// `/Resources /Properties` whose value is an *array of file specification dictionaries*
    /// (the form §14.13.5 requires — filespecs are indirect, so they cannot be inlined here).
    /// Pair with [`Content::end_marked_content`]; on the document side, register the property via
    /// `Builder::add_content_af_property`.
    pub fn begin_af_marked_content(&mut self, property: &str) -> &mut Self {
        self.out.extend_from_slice(b"/AF /");
        self.escape_name(property);
        self.out.extend_from_slice(b" BDC\n");
        self
    }

    /// `/Artifact BMC` — begin an artifact: content (running heads/footers, page numbers, rules,
    /// backgrounds) excluded from the logical structure (§14.8.2.2). Pair with
    /// [`Content::end_marked_content`].
    pub fn begin_artifact(&mut self) -> &mut Self {
        self.out.extend_from_slice(b"/Artifact BMC\n");
        self
    }

    /// `EMC` — end the innermost marked-content sequence (a [`begin_marked_content`] or
    /// [`begin_artifact`]).
    ///
    /// [`begin_marked_content`]: Content::begin_marked_content
    /// [`begin_artifact`]: Content::begin_artifact
    pub fn end_marked_content(&mut self) -> &mut Self {
        self.op(&[], "EMC")
    }

    /// Append `operands` then `operator`, space-separated, terminated by a newline.
    fn op(&mut self, operands: &[f64], operator: &str) -> &mut Self {
        for n in operands {
            self.out.extend_from_slice(fmt_num(*n).as_bytes());
            self.out.push(b' ');
        }
        self.out.extend_from_slice(operator.as_bytes());
        self.out.push(b'\n');
        self
    }

    /// Write a name's characters, `#`-escaping anything outside the regular-character set (§7.3.5).
    fn escape_name(&mut self, name: &str) {
        for &b in name.as_bytes() {
            if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'+') {
                self.out.push(b);
            } else {
                self.out.extend_from_slice(format!("#{b:02X}").as_bytes());
            }
        }
    }
}

/// Format a number as a PDF real (§7.3.3): integral values without a fraction, others trimmed, and
/// never in exponential notation (which PDF does not accept).
fn fmt_num(v: f64) -> String {
    if !v.is_finite() {
        return "0".to_string();
    }
    if v == v.trunc() && v.abs() < 1e15 {
        return format!("{}", v as i64);
    }
    let s = format!("{v:.6}");
    let trimmed = s.trim_end_matches('0').trim_end_matches('.');
    trimmed.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Operation, extract_text, parse_content};

    #[test]
    fn fmt_num_is_clean_and_exponent_free() {
        assert_eq!(fmt_num(12.0), "12");
        assert_eq!(fmt_num(0.5), "0.5");
        assert_eq!(fmt_num(-3.25), "-3.25");
        assert_eq!(fmt_num(1.0e-7), "0"); // below 6dp → trimmed to 0, not "1e-7"
        assert!(!fmt_num(1.0e20).contains('e'));
    }

    #[test]
    fn builds_a_text_showing_stream_that_parses_back() {
        let mut c = Content::new();
        c.begin_text()
            .set_font("F1", 12.0)
            .text_move(72.0, 700.0)
            .show_str("Hello, PDF")
            .end_text();
        let bytes = c.into_bytes();

        // Round-trips through the parser, and the text extractor recovers the string.
        let ops = parse_content(&bytes);
        assert!(
            ops.iter()
                .any(|o| matches!(o, Operation { operator, .. } if operator == "Tj"))
        );
        assert_eq!(extract_text(&ops), "Hello, PDF");
    }

    #[test]
    fn escapes_literal_string_delimiters() {
        let mut c = Content::new();
        c.show_text(b"a(b)c\\d");
        let s = String::from_utf8(c.into_bytes()).unwrap();
        assert!(s.contains("(a\\(b\\)c\\\\d) Tj"));
    }

    #[test]
    fn emits_named_colour_space_and_tint() {
        let mut c = Content::new();
        c.set_fill_color_space("Spot0").set_fill_color(&[1.0]);
        assert_eq!(
            String::from_utf8(c.into_bytes()).unwrap(),
            "/Spot0 cs\n1 scn\n"
        );
    }

    #[test]
    fn emits_an_inline_image() {
        let mut c = Content::new();
        // 1×1 RGB pixel.
        c.inline_image(1, 1, "RGB", 8, &[10, 20, 30]);
        assert_eq!(
            c.into_bytes(),
            b"BI /W 1 /H 1 /CS /RGB /BPC 8 ID \x0a\x14\x1e\nEI\n"
        );
    }

    #[test]
    fn emits_text_spacing_operators() {
        let mut c = Content::new();
        c.set_char_spacing(0.5).set_word_spacing(2.0);
        assert_eq!(String::from_utf8(c.into_bytes()).unwrap(), "0.5 Tc\n2 Tw\n");
    }

    #[test]
    fn shows_two_byte_glyph_codes() {
        let mut c = Content::new();
        c.show_glyphs(&[0x0041, 0x012A]);
        assert_eq!(
            String::from_utf8(c.into_bytes()).unwrap(),
            "<0041012A> Tj\n"
        );
    }

    #[test]
    fn places_an_xobject() {
        let mut c = Content::new();
        c.save()
            .transform(100.0, 0.0, 0.0, 50.0, 10.0, 20.0)
            .do_xobject("Im0")
            .restore();
        assert_eq!(
            String::from_utf8(c.into_bytes()).unwrap(),
            "q\n100 0 0 50 10 20 cm\n/Im0 Do\nQ\n"
        );
    }

    #[test]
    fn clips_and_ends_a_path() {
        let mut c = Content::new();
        c.rect(1.0, 2.0, 3.0, 4.0).clip().end_path();
        assert_eq!(
            String::from_utf8(c.into_bytes()).unwrap(),
            "1 2 3 4 re\nW\nn\n"
        );
    }

    #[test]
    fn emits_marked_content_and_artifacts() {
        let mut c = Content::new();
        c.begin_marked_content("P", 0)
            .begin_text()
            .show_str("Hi")
            .end_text()
            .end_marked_content()
            .begin_artifact()
            .end_marked_content();
        let s = String::from_utf8(c.as_bytes().to_vec()).unwrap();
        assert!(s.starts_with("/P <</MCID 0>> BDC\n"));
        assert!(s.contains("EMC\n"));
        assert!(s.contains("/Artifact BMC\n"));
        // The marked content round-trips through the parser (BDC/EMC are recognized operators).
        let ops = parse_content(c.as_bytes());
        assert!(ops.iter().any(|o| o.operator == "BDC"));
        assert!(ops.iter().any(|o| o.operator == "EMC"));
    }

    #[test]
    fn emits_af_marked_content() {
        // §14.13.5: `/AF /F0 BDC … EMC` — the named property resource holds the filespec array.
        let mut c = Content::new();
        c.begin_af_marked_content("F0")
            .rect(0.0, 0.0, 10.0, 10.0)
            .fill()
            .end_marked_content();
        let s = String::from_utf8(c.as_bytes().to_vec()).unwrap();
        assert!(s.starts_with("/AF /F0 BDC\n"), "got: {s}");
        assert!(s.ends_with("EMC\n"));
        // Round-trips through the parser as a BDC with a name operand.
        let ops = parse_content(c.as_bytes());
        assert!(ops.iter().any(|o| o.operator == "BDC"));
    }

    #[test]
    fn emits_path_and_colour_operators() {
        let mut c = Content::new();
        c.set_fill_rgb(1.0, 0.0, 0.0)
            .rect(10.0, 10.0, 100.0, 50.0)
            .fill();
        let s = String::from_utf8(c.into_bytes()).unwrap();
        assert_eq!(s, "1 0 0 rg\n10 10 100 50 re\nf\n");
    }
}
