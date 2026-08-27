//! CMaps for composite (Type0) fonts: mapping shown character codes to CIDs (ISO 32000-1 §9.7.5–6).
//!
//! A Type0 font's `/Encoding` is either a **predefined** CMap name (`Identity-H`/`Identity-V`, where
//! the 2-byte code *is* the CID, plus the CJK Adobe collections we do not bundle) or an **embedded**
//! CMap stream — a small PostScript-like program listing a codespace and `cidrange`/`cidchar`
//! mappings. This module parses both into a [`CMap`]: it tokenizes a shown byte string into codes
//! using the codespace (codes may be variable width, §9.7.6.2) and resolves each code to a CID.
//!
//! Like the `/ToUnicode` parser ([`crate::ToUnicode`]) it reuses the content-stream tokenizer
//! ([`pdf_content::parse_content`]): the CMap section keywords (`begincidrange`, `begincidchar`,
//! `begincodespacerange`, …) are exactly its operators, and the entries in between arrive as the
//! operands of the matching `end…` operation. Parsing is total and best-effort (DESIGN.md §3):
//! malformed entries are skipped and counts are bounded against hostile input.

use pdf_content::parse_content;
use pdf_cos::Object;

/// Cap on the number of stored single/range mappings and codespace ranges (anti-DoS, §3.4).
const MAX_ENTRIES: usize = 1 << 20;
/// Cap on a single `cidrange` span; larger spans are clamped (they stay cheap — ranges are stored,
/// not expanded — but the cap guards CID resolution arithmetic against absurd inputs).
const MAX_SPAN: u32 = 1 << 24;
/// Longest code length we tokenize (PDF codes are at most 4 bytes, §9.7.6.2).
pub(crate) const MAX_CODE_LEN: usize = 4;

/// One codespace range (§9.7.6.2): a fixed byte length and per-byte `[low, high]` bounds.
#[derive(Clone, Debug)]
struct Codespace {
    /// Code length in bytes (1..=4); equal for `low` and `high`.
    len: usize,
    low: Vec<u8>,
    high: Vec<u8>,
}

impl Codespace {
    /// Whether `bytes` (already `self.len` long) lies within this range, byte by byte.
    fn contains(&self, bytes: &[u8]) -> bool {
        bytes.len() == self.len
            && bytes
                .iter()
                .zip(&self.low)
                .zip(&self.high)
                .all(|((&b, &lo), &hi)| lo <= b && b <= hi)
    }
}

/// A contiguous `cidrange` mapping: codes `lo..=hi` map to `base_cid + (code - lo)`.
#[derive(Clone, Debug)]
struct CidRange {
    lo: u32,
    hi: u32,
    base_cid: u32,
}

/// A parsed Type0 `/Encoding` CMap: tokenizes shown bytes into codes and resolves codes to CIDs
/// (§9.7.5–6).
#[derive(Clone, Debug, Default)]
pub struct CMap {
    /// `Identity-H`/`Identity-V`: every 2-byte code maps to itself as a CID (§9.7.4.3).
    identity: bool,
    codespaces: Vec<Codespace>,
    singles: Vec<(u32, u32)>,
    ranges: Vec<CidRange>,
}

impl CMap {
    /// The predefined `Identity-H`/`Identity-V` CMap: 2-byte codes, CID = code (§9.7.4.3).
    #[must_use]
    pub fn identity() -> Self {
        Self {
            identity: true,
            codespaces: vec![Codespace {
                len: 2,
                low: vec![0x00, 0x00],
                high: vec![0xFF, 0xFF],
            }],
            ..Self::default()
        }
    }

    /// Resolve a predefined CMap `/Encoding` name (§9.7.4.3). Only the `Identity` CMaps are built
    /// in; the named Adobe CJK collections are not bundled, so they yield `None`.
    #[must_use]
    pub fn from_predefined(name: &[u8]) -> Option<Self> {
        matches!(name, b"Identity" | b"Identity-H" | b"Identity-V").then(Self::identity)
    }

    /// Parse a decoded embedded CMap stream (§9.7.5.3). Always returns a value; unparseable parts
    /// are skipped. If no codespace is declared, a 2-byte one is assumed (the Type0 default).
    #[must_use]
    pub fn parse(bytes: &[u8]) -> Self {
        let mut cmap = Self::default();
        for op in parse_content(bytes) {
            if cmap.len() >= MAX_ENTRIES {
                break;
            }
            match op.operator.as_str() {
                "endcodespacerange" => cmap.add_codespaces(&op.operands),
                "endcidchar" => cmap.add_cidchars(&op.operands),
                "endcidrange" => cmap.add_cidranges(&op.operands),
                _ => {}
            }
        }
        if cmap.codespaces.is_empty() {
            cmap.codespaces.push(Codespace {
                len: 2,
                low: vec![0x00, 0x00],
                high: vec![0xFF, 0xFF],
            });
        }
        cmap
    }

    /// Total number of stored entries (for the anti-DoS cap).
    fn len(&self) -> usize {
        self.singles.len() + self.ranges.len() + self.codespaces.len()
    }

    /// `<lo> <hi> …` pairs from `endcodespacerange`.
    fn add_codespaces(&mut self, operands: &[Object]) {
        for pair in operands.as_chunks::<2>().0 {
            if let [Object::String(lo), Object::String(hi)] = pair {
                let (low, high) = (lo.as_bytes(), hi.as_bytes());
                let len = low.len();
                if (1..=MAX_CODE_LEN).contains(&len) && high.len() == len {
                    self.codespaces.push(Codespace {
                        len,
                        low: low.to_vec(),
                        high: high.to_vec(),
                    });
                }
            }
        }
    }

    /// `<code> cid …` pairs from `endcidchar`.
    fn add_cidchars(&mut self, operands: &[Object]) {
        for pair in operands.as_chunks::<2>().0 {
            if let [Object::String(code), cid] = pair
                && let Some(cid) = as_cid(cid)
            {
                self.singles.push((code_value(code.as_bytes()), cid));
            }
        }
    }

    /// `<lo> <hi> cid …` triples from `endcidrange`.
    fn add_cidranges(&mut self, operands: &[Object]) {
        for triple in operands.as_chunks::<3>().0 {
            if let [Object::String(lo), Object::String(hi), cid] = triple
                && let Some(base_cid) = as_cid(cid)
            {
                let (lo, hi) = (code_value(lo.as_bytes()), code_value(hi.as_bytes()));
                if lo <= hi && hi - lo <= MAX_SPAN {
                    self.ranges.push(CidRange { lo, hi, base_cid });
                }
            }
        }
    }

    /// Tokenize `bytes` into character codes using the codespace (§9.7.6.2), resolving each to a
    /// CID. Codes that match no mapping resolve to CID 0 (`.notdef`).
    #[must_use]
    pub fn codes_to_cids(&self, bytes: &[u8]) -> Vec<u32> {
        let mut out = Vec::new();
        let mut i = 0;
        while i < bytes.len() {
            let len = self.code_len(&bytes[i..]);
            let code = code_value(&bytes[i..i + len]);
            out.push(self.cid(code));
            i += len;
        }
        out
    }

    /// Length in bytes of the next code starting at `bytes[0]` (§9.7.6.2). Prefers the shortest
    /// codespace that fully contains the candidate; otherwise falls back to a codespace whose first
    /// byte matches, then to a single byte — always consuming at least one byte so iteration ends.
    fn code_len(&self, bytes: &[u8]) -> usize {
        let mut first_byte_len: Option<usize> = None;
        for cs in &self.codespaces {
            if cs.len <= bytes.len() && cs.contains(&bytes[..cs.len]) {
                return cs.len; // full match wins
            }
            if cs.low.first() <= Some(&bytes[0]) && Some(&bytes[0]) <= cs.high.first() {
                // Remember a partial (first-byte) match as a fallback width.
                first_byte_len = Some(first_byte_len.map_or(cs.len, |l| l.min(cs.len)));
            }
        }
        first_byte_len.unwrap_or(1).min(bytes.len()).max(1)
    }

    /// Resolve a code to its CID: identity, then explicit chars, then ranges; else 0.
    fn cid(&self, code: u32) -> u32 {
        if self.identity {
            return code;
        }
        if let Some(&(_, cid)) = self.singles.iter().find(|&&(c, _)| c == code) {
            return cid;
        }
        for r in &self.ranges {
            if (r.lo..=r.hi).contains(&code) {
                return r.base_cid.saturating_add(code - r.lo);
            }
        }
        0
    }
}

/// A `cidrange`/`cidchar` CID operand is a non-negative integer (§9.7.5.3).
fn as_cid(obj: &Object) -> Option<u32> {
    match obj {
        Object::Integer(n) if *n >= 0 => u32::try_from(*n).ok(),
        _ => None,
    }
}

/// Interpret up to [`MAX_CODE_LEN`] bytes as a big-endian code value. Shared with the ToUnicode
/// CMap parser in `cmap`, which reads the same §9.7.5.3 codespace syntax.
pub(crate) fn code_value(bytes: &[u8]) -> u32 {
    bytes
        .iter()
        .take(MAX_CODE_LEN)
        .fold(0u32, |acc, &b| (acc << 8) | u32::from(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_maps_two_byte_code_to_itself() {
        let cmap = CMap::identity();
        // Two 2-byte codes -> the same CIDs.
        assert_eq!(
            cmap.codes_to_cids(&[0x00, 0x03, 0x12, 0x34]),
            [0x0003, 0x1234]
        );
    }

    #[test]
    fn predefined_names() {
        assert!(CMap::from_predefined(b"Identity-H").is_some());
        assert!(CMap::from_predefined(b"Identity-V").is_some());
        assert!(CMap::from_predefined(b"UniGB-UCS2-H").is_none());
    }

    #[test]
    fn parses_embedded_cidrange_and_cidchar() {
        let src = b"/CIDInit /ProcSet findresource begin 12 dict begin begincmap\n\
            1 begincodespacerange <0000> <FFFF> endcodespacerange\n\
            1 begincidchar <0003> 10 endcidchar\n\
            1 begincidrange <0010> <0012> 20 endcidrange\n\
            endcmap end end";
        let cmap = CMap::parse(src);
        // cidchar: 0x0003 -> 10.
        assert_eq!(cmap.codes_to_cids(&[0x00, 0x03]), [10]);
        // cidrange: 0x10->20, 0x11->21, 0x12->22.
        assert_eq!(cmap.codes_to_cids(&[0x00, 0x10, 0x00, 0x12]), [20, 22]);
        // Unmapped code -> CID 0.
        assert_eq!(cmap.codes_to_cids(&[0xAB, 0xCD]), [0]);
    }

    #[test]
    fn variable_width_codespace_tokenizes_by_first_byte() {
        // A 1-byte range for 0x00..0x7F and a 2-byte range for 0x80xx..0xFFxx.
        let src = b"begincmap\n\
            2 begincodespacerange <00> <7F> <8000> <FFFF> endcodespacerange\n\
            1 begincidchar <41> 5 endcidchar\n\
            1 begincidchar <8001> 9 endcidchar\n\
            endcmap";
        let cmap = CMap::parse(src);
        // 0x41 is a 1-byte code (5); 0x80 0x01 is a 2-byte code (9).
        assert_eq!(cmap.codes_to_cids(&[0x41, 0x80, 0x01]), [5, 9]);
    }

    #[test]
    fn missing_codespace_defaults_to_two_bytes() {
        let cmap = CMap::parse(b"begincmap\n1 begincidchar <0001> 7 endcidchar\nendcmap");
        assert_eq!(cmap.codes_to_cids(&[0x00, 0x01]), [7]);
    }

    #[test]
    fn empty_or_garbage_is_safe() {
        assert!(CMap::parse(b"").codes_to_cids(&[0x00, 0x00]) == [0]);
        // A stray odd byte still terminates (consumes the final byte).
        assert_eq!(CMap::parse(b")) garbage ((").codes_to_cids(&[0x05]), [0]);
    }
}
