//! Basic text extraction from content-stream operations (ISO 32000-1 §9.4).
//!
//! Walks the text-showing operators — `Tj`, `TJ`, `'`, `"` (§9.4.3) — and concatenates the bytes
//! they paint, using the text-positioning operators (`Td`, `TD`, `T*`, `Tm`, §9.4.2) as rough line
//! breaks. The result is *layout-free* reading-order text.
//!
//! **Limitations (follow-ups in EPIC 7).** Shown bytes are mapped to characters as Latin-1, which
//! is correct for ASCII content under the standard encodings but not for custom encodings or
//! composite (Type0/CID) fonts; faithful Unicode needs the font's encoding and `/ToUnicode` CMap
//! (§9.10). Inter-glyph spacing in `TJ` is approximated by a fixed threshold rather than computed
//! from the text state.

use pdf_cos::Object;

use crate::parser::Operation;

/// A `TJ` position adjustment more negative than this (thousandths of text space) is treated as a
/// word gap and rendered as a space (§9.4.3). Heuristic, pending real text-state metrics.
const WORD_GAP_THRESHOLD: f64 = 100.0;

/// Maps the bytes shown by a text operator to Unicode, given the current font (§9.10).
///
/// The content layer knows nothing about fonts (architecture: `content → cos`); a caller that has
/// the page's fonts implements this to supply faithful decoding (e.g. via `/ToUnicode`). The
/// `font` is the current resource name set by the most recent `Tf` (§9.4.2), or `None`.
pub trait GlyphDecoder {
    /// Decode `bytes` (a shown string) under `font` into Unicode text.
    fn decode(&self, font: Option<&str>, bytes: &[u8]) -> String;
}

/// The default decoder: bytes → Latin-1 characters. Correct for ASCII under the standard
/// encodings; not for custom encodings or composite fonts (those need a real [`GlyphDecoder`]).
pub struct Latin1Decoder;

impl GlyphDecoder for Latin1Decoder {
    fn decode(&self, _font: Option<&str>, bytes: &[u8]) -> String {
        bytes.iter().map(|&b| char::from(b)).collect()
    }
}

/// Extract reading-order text from parsed content operations (§9.4), decoding shown bytes as
/// Latin-1. See [`extract_text_with`] to supply font-aware decoding.
#[must_use]
pub fn extract_text(operations: &[Operation]) -> String {
    extract_text_with(operations, &Latin1Decoder)
}

/// Extract reading-order text (§9.4) using `decoder` to map shown bytes to Unicode. Tracks the
/// current font set by `Tf` (§9.4.2) and passes it to the decoder. Form XObjects (`Do`, §8.10) are
/// ignored; see [`extract_text_with_forms`].
#[must_use]
pub fn extract_text_with(operations: &[Operation], decoder: &dyn GlyphDecoder) -> String {
    extract_text_with_forms(operations, decoder, &|_| None)
}

/// As [`extract_text_with`], but on a `Do` operator (§8.10) that invokes a form XObject the
/// `on_form` callback supplies that form's already-extracted text, which is inlined in place. The
/// callback returns `None` for non-form XObjects (e.g. images) or unresolvable names.
#[must_use]
pub fn extract_text_with_forms(
    operations: &[Operation],
    decoder: &dyn GlyphDecoder,
    on_form: &dyn Fn(&str) -> Option<String>,
) -> String {
    let mut out = String::new();
    let mut font: Option<String> = None;
    for op in operations {
        match op.operator.as_str() {
            // Select font (§9.4.2): first operand is the resource name.
            "Tf" => {
                if let Some(Object::Name(name)) = op.operands.first() {
                    font = Some(String::from_utf8_lossy(name.as_bytes()).into_owned());
                }
            }
            // Show a string (§9.4.3): the (single) string operand.
            "Tj" => show_last_string(&mut out, op, decoder, font.as_deref()),
            // Move to next line and show; `"` also sets word/char spacing (§9.4.3).
            "'" | "\"" => {
                push_newline(&mut out);
                show_last_string(&mut out, op, decoder, font.as_deref());
            }
            // Show with individual glyph positioning (§9.4.3): array of strings and numbers.
            "TJ" => show_tj_array(&mut out, op, decoder, font.as_deref()),
            // Text positioning that begins a new line (§9.4.2): approximate as a line break.
            "Td" | "TD" | "T*" | "Tm" => push_newline(&mut out),
            // End of a text object (§9.4.1).
            "ET" => push_newline(&mut out),
            // Invoke an XObject (§8.10): inline a form XObject's text.
            "Do" => {
                if let Some(Object::Name(name)) = op.operands.first() {
                    if let Some(text) = on_form(&String::from_utf8_lossy(name.as_bytes())) {
                        if !text.is_empty() {
                            push_newline(&mut out);
                            out.push_str(&text);
                            push_newline(&mut out);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    out.trim().to_string()
}

/// Append the operation's last string operand (the argument to `Tj`/`'`/`"`), decoded.
fn show_last_string(
    out: &mut String,
    op: &Operation,
    decoder: &dyn GlyphDecoder,
    font: Option<&str>,
) {
    if let Some(Object::String(s)) = op.operands.last() {
        out.push_str(&decoder.decode(font, s.as_bytes()));
    }
}

/// Append the strings of a `TJ` array (decoded), inserting a space for each large negative
/// adjustment.
fn show_tj_array(out: &mut String, op: &Operation, decoder: &dyn GlyphDecoder, font: Option<&str>) {
    let Some(Object::Array(array)) = op.operands.last() else {
        return;
    };
    for element in array.iter() {
        match element {
            Object::String(s) => out.push_str(&decoder.decode(font, s.as_bytes())),
            Object::Integer(n) if (*n as f64) <= -WORD_GAP_THRESHOLD => out.push(' '),
            Object::Real(r) if *r <= -WORD_GAP_THRESHOLD => out.push(' '),
            _ => {}
        }
    }
}

/// Append a newline unless the output already ends with one (avoids runs of blank lines).
fn push_newline(out: &mut String) {
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_content;

    fn text_of(content: &[u8]) -> String {
        extract_text(&parse_content(content))
    }

    #[test]
    fn extracts_simple_show() {
        assert_eq!(text_of(b"BT /F1 12 Tf (Hello World) Tj ET"), "Hello World");
    }

    #[test]
    fn extracts_tj_array_with_word_gap() {
        // A large negative adjustment between the two strings becomes a space.
        assert_eq!(text_of(b"BT [(Hello) -250 (World)] TJ ET"), "Hello World");
    }

    #[test]
    fn line_breaks_on_positioning() {
        let text = text_of(b"BT (line one) Tj 0 -14 Td (line two) Tj ET");
        assert_eq!(text, "line one\nline two");
    }

    #[test]
    fn quote_operator_starts_new_line() {
        let text = text_of(b"BT (first) Tj (second) ' ET");
        assert_eq!(text, "first\nsecond");
    }

    #[test]
    fn ignores_non_text_operators() {
        assert_eq!(text_of(b"1 0 0 1 0 0 cm 0 0 100 100 re f"), "");
    }

    /// A decoder that upper-cases bytes for font `F1` to prove font-aware decoding is wired.
    struct UpperFor;
    impl GlyphDecoder for UpperFor {
        fn decode(&self, font: Option<&str>, bytes: &[u8]) -> String {
            let text: String = bytes.iter().map(|&b| char::from(b)).collect();
            if font == Some("F1") {
                text.to_uppercase()
            } else {
                text
            }
        }
    }

    #[test]
    fn do_operator_inlines_form_text() {
        // `/Fm0 Do` invokes a form XObject; its text is supplied by the callback and inlined.
        let ops = parse_content(b"BT (page) Tj ET /Fm0 Do BT (after) Tj ET");
        let text = extract_text_with_forms(&ops, &Latin1Decoder, &|name| {
            (name == "Fm0").then(|| "form-text".to_string())
        });
        assert_eq!(text, "page\nform-text\nafter");
        // The default extractor ignores forms (the `Do` produces nothing).
        assert_eq!(extract_text_with(&ops, &Latin1Decoder), "page\nafter");
    }

    #[test]
    fn uses_decoder_with_current_font() {
        // Tf selects /F1, so the decoder sees font "F1" and upper-cases.
        let ops = parse_content(b"BT /F1 12 Tf (hi) Tj ET");
        assert_eq!(extract_text_with(&ops, &UpperFor), "HI");
        // Without a Tf the font is None and the decoder leaves text unchanged.
        let ops = parse_content(b"BT (hi) Tj ET");
        assert_eq!(extract_text_with(&ops, &UpperFor), "hi");
    }
}
