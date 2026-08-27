//! `/ToUnicode` CMap parsing (ISO 32000-1 §9.10.3): map character codes shown by a font to
//! Unicode text, the reliable basis for text extraction.
//!
//! A ToUnicode CMap is a small PostScript-like program. Rather than write a CMap interpreter, we
//! reuse the content-stream tokenizer ([`pdf_content::parse_content`], architecture:
//! `fonts → content`): its operators are exactly the CMap section keywords
//! (`begincodespacerange`/`beginbfchar`/`beginbfrange` …) and the entries in between arrive as the
//! operands of the matching `end…` operation.
//!
//! Parsing is total and best-effort (DESIGN.md §3): malformed entries are skipped, and range
//! sizes / total entry counts are bounded against hostile input.

use std::collections::BTreeMap;

use pdf_content::parse_content;
use pdf_cos::Object;

use crate::cid::code_value;

/// A parsed `/ToUnicode` CMap: character code → Unicode string (§9.10.3).
#[derive(Clone, Debug, Default)]
pub struct ToUnicode {
    /// Number of bytes per character code (1 for simple fonts, 2 for typical Type0/CID, §9.10.3).
    width: usize,
    /// Code value → destination text.
    map: BTreeMap<u32, String>,
}

/// Cap on total mappings and on a single `bfrange` span (anti-DoS, DESIGN.md §3.4).
const MAX_ENTRIES: usize = 1 << 20;
const MAX_RANGE: u32 = 1 << 16;

impl ToUnicode {
    /// Parse a decoded ToUnicode CMap. Always returns a value; unparseable parts are skipped.
    #[must_use]
    pub fn parse(cmap: &[u8]) -> Self {
        let mut width = 0usize;
        let mut map = BTreeMap::new();

        for op in parse_content(cmap) {
            if map.len() >= MAX_ENTRIES {
                break;
            }
            match op.operator.as_str() {
                // `<lo> <hi> endcodespacerange`: the bound width is the code width (§9.10.3).
                "endcodespacerange" => {
                    if width == 0
                        && let Some(Object::String(bound)) = op.operands.first()
                    {
                        width = bound.as_bytes().len();
                    }
                }
                // `<src> <dst> …`: individual code → text mappings.
                "endbfchar" => {
                    for pair in op.operands.as_chunks::<2>().0 {
                        if let [Object::String(src), Object::String(dst)] = pair {
                            width = width.max(src.as_bytes().len());
                            map.insert(code_value(src.as_bytes()), utf16be(dst.as_bytes()));
                        }
                    }
                }
                // `<lo> <hi> <dst>` or `<lo> <hi> [<dst> …]`: contiguous ranges.
                "endbfrange" => insert_bfrange(&op.operands, &mut width, &mut map),
                _ => {}
            }
        }

        Self {
            width: width.max(1),
            map,
        }
    }

    /// Decode the bytes of a shown string into Unicode text, splitting them into fixed-width codes
    /// (§9.10.3). Codes with no mapping contribute nothing.
    #[must_use]
    pub fn decode(&self, codes: &[u8]) -> String {
        let mut out = String::new();
        let mut i = 0;
        while i < codes.len() {
            let end = (i + self.width).min(codes.len());
            if let Some(text) = self.map.get(&code_value(&codes[i..end])) {
                out.push_str(text);
            }
            i = end;
        }
        out
    }

    /// Whether the CMap produced any mappings.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

/// Insert the entries of one or more `bfrange` triples into `map`.
fn insert_bfrange(operands: &[Object], width: &mut usize, map: &mut BTreeMap<u32, String>) {
    for triple in operands.as_chunks::<3>().0 {
        let (Object::String(lo), Object::String(hi)) = (&triple[0], &triple[1]) else {
            continue;
        };
        *width = (*width).max(lo.as_bytes().len());
        let (lo_code, hi_code) = (code_value(lo.as_bytes()), code_value(hi.as_bytes()));
        if hi_code < lo_code || hi_code - lo_code >= MAX_RANGE {
            continue; // empty or implausibly large range
        }
        let span = hi_code - lo_code;
        match &triple[2] {
            // Destination base, incremented across the range (§9.10.3).
            Object::String(dst) => {
                let base = code_value(dst.as_bytes());
                for k in 0..=span {
                    if let Some(c) = char::from_u32(base + k) {
                        map.insert(lo_code + k, c.to_string());
                    }
                }
            }
            // Explicit destination per code.
            Object::Array(items) => {
                for (k, item) in items.iter().enumerate().take(span as usize + 1) {
                    if let Object::String(dst) = item {
                        map.insert(lo_code + k as u32, utf16be(dst.as_bytes()));
                    }
                }
            }
            _ => {}
        }
    }
}

/// Decode UTF-16BE bytes (a ToUnicode destination) into a string, replacing invalid units.
fn utf16be(bytes: &[u8]) -> String {
    let units = bytes
        .chunks(2)
        .map(|c| (u16::from(c[0]) << 8) | u16::from(c.get(1).copied().unwrap_or(0)));
    char::decode_utf16(units)
        .map(|r| r.unwrap_or('\u{FFFD}'))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIMPLE: &[u8] = b"/CIDInit /ProcSet findresource begin 12 dict begin begincmap\n\
        1 begincodespacerange <00> <FF> endcodespacerange\n\
        2 beginbfchar <41> <0041> <42> <0042> endbfchar\n\
        1 beginbfrange <61> <63> <0061> endbfrange\n\
        endcmap end end";

    #[test]
    fn parses_single_byte_cmap() {
        let tu = ToUnicode::parse(SIMPLE);
        assert_eq!(tu.decode(b"\x41\x42"), "AB");
        // bfrange 0x61..=0x63 -> a, b, c.
        assert_eq!(tu.decode(b"\x61\x62\x63"), "abc");
        // Unmapped code contributes nothing.
        assert_eq!(tu.decode(b"\x41\xFF\x42"), "AB");
    }

    #[test]
    fn parses_two_byte_cmap() {
        // Type0/CID style: 2-byte codes.
        let cmap = b"begincmap\n\
            1 begincodespacerange <0000> <FFFF> endcodespacerange\n\
            1 beginbfchar <0003> <0041> endbfchar\n\
            endcmap";
        let tu = ToUnicode::parse(cmap);
        assert_eq!(tu.decode(b"\x00\x03"), "A");
    }

    #[test]
    fn bfrange_array_form() {
        let cmap = b"begincmap\n\
            1 begincodespacerange <00> <FF> endcodespacerange\n\
            1 beginbfrange <01> <02> [<0058> <0059>] endbfrange\n\
            endcmap";
        let tu = ToUnicode::parse(cmap);
        assert_eq!(tu.decode(b"\x01\x02"), "XY");
    }

    #[test]
    fn empty_or_garbage_is_safe() {
        assert!(ToUnicode::parse(b"").is_empty());
        assert!(ToUnicode::parse(b")))) not a cmap (((").is_empty());
    }
}
