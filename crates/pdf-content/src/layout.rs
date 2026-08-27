//! Positioned text extraction via a minimal graphics-state machine (ISO 32000-1 §8.3/§8.4/§9.4).
//!
//! Tracks the current transformation matrix (`cm`, `q`/`Q`) and the text matrices (`BT`, `Tm`,
//! `Td`/`TD`, `T*`) to compute the device-space position of each shown string, then orders those
//! fragments geometrically (top-to-bottom, left-to-right). This recovers reading order even when a
//! document emits text out of order, and breaks lines on actual `y` changes rather than operator
//! heuristics.
//!
//! Scope: glyph *advance* within a string is not modelled (it needs per-glyph widths), so each
//! show op yields one fragment positioned at its text-origin; and form XObjects (`Do`) are not
//! followed here (use the emission-order extractor for those).

use pdf_cos::Object;

use crate::parser::Operation;
use crate::text::GlyphDecoder;

/// A run of shown text with its device-space origin (§9.4.4).
#[derive(Clone, PartialEq, Debug)]
pub struct TextFragment {
    /// The decoded text of one show operation.
    pub text: String,
    /// Device-space x of the text origin.
    pub x: f64,
    /// Device-space y of the text origin.
    pub y: f64,
}

/// A `TJ` adjustment beyond this (thousandths of text space) is rendered as a space (cf. §9.4.3).
const WORD_GAP_THRESHOLD: f64 = 100.0;

/// A 2-D affine matrix in PDF row-vector form `[a b c d e f]` (§8.3.3): a point `(x, y)` maps to
/// `(a·x + c·y + e, b·x + d·y + f)`.
type Matrix = [f64; 6];

const IDENTITY: Matrix = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

/// Compose `a` then `b` (matrix product for row vectors).
fn mul(a: Matrix, b: Matrix) -> Matrix {
    [
        a[0] * b[0] + a[1] * b[2],
        a[0] * b[1] + a[1] * b[3],
        a[2] * b[0] + a[3] * b[2],
        a[2] * b[1] + a[3] * b[3],
        a[4] * b[0] + a[5] * b[2] + b[4],
        a[4] * b[1] + a[5] * b[3] + b[5],
    ]
}

/// A pure translation matrix.
fn translate(tx: f64, ty: f64) -> Matrix {
    [1.0, 0.0, 0.0, 1.0, tx, ty]
}

/// Read the first `n` operands as numbers (`Integer`/`Real`), or `None`.
fn numbers(op: &Operation, n: usize) -> Option<Vec<f64>> {
    if op.operands.len() < n {
        return None;
    }
    op.operands.iter().take(n).map(Object::as_f64).collect()
}

/// Extract positioned text fragments from page content operations (§9.4), using `decoder` for
/// glyph→Unicode mapping. See [`layout`] to turn them into reading-order text.
#[must_use]
pub fn extract_fragments(
    operations: &[Operation],
    decoder: &dyn GlyphDecoder,
) -> Vec<TextFragment> {
    let mut fragments = Vec::new();
    let mut ctm_stack: Vec<Matrix> = Vec::new();
    let mut ctm = IDENTITY;
    let mut tm = IDENTITY;
    let mut tlm = IDENTITY;
    let mut leading = 0.0;
    let mut font: Option<String> = None;

    for op in operations {
        match op.operator.as_str() {
            "q" => ctm_stack.push(ctm),
            "Q" => {
                if let Some(saved) = ctm_stack.pop() {
                    ctm = saved;
                }
            }
            "cm" => {
                if let Some(m) = numbers(op, 6) {
                    ctm = mul([m[0], m[1], m[2], m[3], m[4], m[5]], ctm);
                }
            }
            "BT" => {
                tm = IDENTITY;
                tlm = IDENTITY;
            }
            "Tf" => {
                if let Some(Object::Name(name)) = op.operands.first() {
                    font = Some(String::from_utf8_lossy(name.as_bytes()).into_owned());
                }
            }
            "Td" => {
                if let Some(t) = numbers(op, 2) {
                    tlm = mul(translate(t[0], t[1]), tlm);
                    tm = tlm;
                }
            }
            "TD" => {
                if let Some(t) = numbers(op, 2) {
                    leading = -t[1];
                    tlm = mul(translate(t[0], t[1]), tlm);
                    tm = tlm;
                }
            }
            "Tm" => {
                if let Some(m) = numbers(op, 6) {
                    tm = [m[0], m[1], m[2], m[3], m[4], m[5]];
                    tlm = tm;
                }
            }
            "TL" => {
                if let Some(v) = numbers(op, 1) {
                    leading = v[0];
                }
            }
            "T*" => {
                tlm = mul(translate(0.0, -leading), tlm);
                tm = tlm;
            }
            "Tj" => push_fragment(
                &mut fragments,
                tm,
                ctm,
                show_text(op, decoder, font.as_deref()),
            ),
            "'" | "\"" => {
                tlm = mul(translate(0.0, -leading), tlm);
                tm = tlm;
                push_fragment(
                    &mut fragments,
                    tm,
                    ctm,
                    show_text(op, decoder, font.as_deref()),
                );
            }
            "TJ" => push_fragment(
                &mut fragments,
                tm,
                ctm,
                show_tj(op, decoder, font.as_deref()),
            ),
            _ => {}
        }
    }
    fragments
}

/// Compute a fragment's device origin from `tm`∘`ctm` and record it if it has text.
fn push_fragment(out: &mut Vec<TextFragment>, tm: Matrix, ctm: Matrix, text: String) {
    if text.is_empty() {
        return;
    }
    let m = mul(tm, ctm);
    out.push(TextFragment {
        text,
        x: m[4],
        y: m[5],
    });
}

/// Decode the (last) string operand of a `Tj`/`'`/`"`.
fn show_text(op: &Operation, decoder: &dyn GlyphDecoder, font: Option<&str>) -> String {
    match op.operands.last() {
        Some(Object::String(s)) => decoder.decode(font, s.as_bytes()),
        _ => String::new(),
    }
}

/// Decode a `TJ` array, inserting a space for each large negative adjustment.
fn show_tj(op: &Operation, decoder: &dyn GlyphDecoder, font: Option<&str>) -> String {
    let Some(Object::Array(array)) = op.operands.last() else {
        return String::new();
    };
    let mut out = String::new();
    for element in array.iter() {
        match element {
            Object::String(s) => out.push_str(&decoder.decode(font, s.as_bytes())),
            Object::Integer(n) if (*n as f64) <= -WORD_GAP_THRESHOLD => out.push(' '),
            Object::Real(r) if *r <= -WORD_GAP_THRESHOLD => out.push(' '),
            _ => {}
        }
    }
    out
}

/// Order fragments into reading-order text: rows top-to-bottom (by `y`), each row left-to-right
/// (by `x`). Fragments within ~1 unit of `y` share a row (joined by spaces); rows are separated by
/// newlines.
#[must_use]
pub fn layout(fragments: &[TextFragment]) -> String {
    let mut ordered: Vec<&TextFragment> = fragments.iter().collect();
    ordered.sort_by(|a, b| {
        let (ay, by) = (a.y.round() as i64, b.y.round() as i64);
        by.cmp(&ay).then_with(|| a.x.total_cmp(&b.x))
    });

    let mut out = String::new();
    let mut last_row: Option<i64> = None;
    for fragment in ordered {
        let row = fragment.y.round() as i64;
        match last_row {
            Some(prev) if prev == row => out.push(' '),
            Some(_) => out.push('\n'),
            None => {}
        }
        out.push_str(&fragment.text);
        last_row = Some(row);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_content;
    use crate::text::Latin1Decoder;

    fn fragments(content: &[u8]) -> Vec<TextFragment> {
        extract_fragments(&parse_content(content), &Latin1Decoder)
    }

    #[test]
    fn computes_text_origin_from_tm() {
        let frags = fragments(b"BT 1 0 0 1 50 700 Tm (Hi) Tj ET");
        assert_eq!(frags.len(), 1);
        assert_eq!((frags[0].x, frags[0].y), (50.0, 700.0));
        assert_eq!(frags[0].text, "Hi");
    }

    #[test]
    fn cm_translation_offsets_position() {
        // The CTM (cm) shifts the text origin: 10 + 50 = 60, 20 + 700 = 720.
        let frags = fragments(b"1 0 0 1 10 20 cm BT 1 0 0 1 50 700 Tm (x) Tj ET");
        assert_eq!((frags[0].x, frags[0].y), (60.0, 720.0));
    }

    #[test]
    fn q_restores_the_ctm() {
        // The cm inside q/Q must not affect text drawn after Q.
        let frags = fragments(b"q 1 0 0 1 100 0 cm Q BT 1 0 0 1 5 5 Tm (a) Tj ET");
        assert_eq!(frags[0].x, 5.0);
    }

    #[test]
    fn layout_orders_left_to_right_then_top_to_bottom() {
        // Emitted out of order: "World" (right) before "Hello" (left), both on the top row; a
        // lower row drawn first.
        let content = b"BT 1 0 0 1 50 700 Tm (low) Tj \
            1 0 0 1 200 760 Tm (World) Tj \
            1 0 0 1 50 760 Tm (Hello) Tj ET";
        assert_eq!(layout(&fragments(content)), "Hello World\nlow");
    }

    #[test]
    fn t_star_uses_leading() {
        // TL sets leading; T* moves down one line.
        let frags = fragments(b"BT 1 0 0 1 0 100 Tm 12 TL (one) Tj T* (two) Tj ET");
        assert_eq!(frags[0].y, 100.0);
        assert_eq!(frags[1].y, 88.0); // 100 - 12
    }
}
