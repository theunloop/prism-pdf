//! Simple-font encodings (ISO 32000-1 §9.6.6): map single-byte character codes to Unicode for
//! fonts that have no `/ToUnicode` CMap.
//!
//! A simple font's `/Encoding` is either a base-encoding name (`WinAnsiEncoding`,
//! `MacRomanEncoding`, `StandardEncoding`, `PDFDocEncoding`) or a dictionary with a `/BaseEncoding`
//! and a `/Differences` array that re-maps individual codes to glyph names (§9.6.6.1). We resolve a
//! 256-entry code → `char` table from that, resolving `/Differences` glyph names via the `uniXXXX`
//! convention and a built-in subset of the Adobe Glyph List.
//!
//! Scope/approximations (faithful enough for Latin text; full coverage is a follow-up): the
//! WinAnsi (cp1252) and MacRoman tables are exact; `StandardEncoding`/`PDFDocEncoding` reuse the
//! WinAnsi table (they agree across ASCII and most Latin punctuation), and unknown `/Differences`
//! glyph names leave the code undecodable.

use pdf_cos::{Dictionary, Name, Object};

/// A resolved simple-font encoding: a code → Unicode table (§9.6.6).
#[derive(Clone, Debug)]
pub struct Encoding {
    table: [Option<char>; 256],
}

impl Encoding {
    /// Build the encoding for a simple-font dictionary by reading its `/Encoding` (§9.6.6). Falls
    /// back to WinAnsi when `/Encoding` is absent — the common case for Latin text.
    #[must_use]
    pub fn from_font_dict(font: &Dictionary) -> Self {
        let table = match font.get(&Name::from("Encoding")) {
            Some(Object::Name(name)) => base_table(name.as_bytes()),
            Some(Object::Dictionary(enc)) => {
                let mut table = match enc.get_name(&Name::from("BaseEncoding")) {
                    Some(base) => base_table(base.as_bytes()),
                    None => win_ansi_table(),
                };
                apply_differences(&mut table, enc);
                table
            }
            _ => win_ansi_table(),
        };
        Self { table }
    }

    /// Decode shown bytes (one byte per code) to Unicode (§9.6.6). Codes with no mapping
    /// contribute nothing.
    #[must_use]
    pub fn decode(&self, bytes: &[u8]) -> String {
        bytes
            .iter()
            .filter_map(|&b| self.table[b as usize])
            .collect()
    }
}

/// Select a base-encoding table by its PDF name (§9.6.6). Unknown names fall back to WinAnsi.
fn base_table(name: &[u8]) -> [Option<char>; 256] {
    match name {
        b"MacRomanEncoding" => mac_roman_table(),
        // WinAnsi is exact; Standard/PDFDoc are approximated by it (they share ASCII + most Latin).
        _ => win_ansi_table(),
    }
}

/// Apply an `/Encoding /Differences` array, re-mapping codes to glyph names (§9.6.6.1).
fn apply_differences(table: &mut [Option<char>; 256], enc: &Dictionary) {
    let Some(diffs) = enc.get_array(&Name::from("Differences")) else {
        return;
    };
    let mut code = 0usize;
    for item in diffs.iter() {
        match item {
            Object::Integer(n) => code = (*n).clamp(0, 255) as usize,
            // An unresolvable glyph name leaves the code undecodable rather than wrong.
            Object::Name(glyph) if code < 256 => {
                table[code] = glyph_to_char(glyph.as_bytes());
                code += 1;
            }
            _ => {}
        }
    }
}

/// Resolve a glyph name to a character: the `uniXXXX`/`uXXXXXX` conventions, single-character
/// names, then a built-in glyph-list subset (§9.10.2 algorithm, abbreviated).
fn glyph_to_char(name: &[u8]) -> Option<char> {
    // `uniXXXX` (one BMP scalar; we take the first of any sequence).
    if let Some(hex) = name.strip_prefix(b"uni")
        && hex.len() >= 4
        && let Some(c) = hex_scalar(&hex[..4])
    {
        return Some(c);
    }
    // `uXXXX`..`uXXXXXX`.
    if name.first() == Some(&b'u')
        && (5..=7).contains(&name.len())
        && let Some(c) = hex_scalar(&name[1..])
    {
        return Some(c);
    }
    // A single printable ASCII glyph name (`A`, `z`, …) maps to itself.
    if let [b @ 0x21..=0x7E] = name {
        return Some(char::from(*b));
    }
    AGL.iter()
        .find(|(glyph, _)| *glyph == name)
        .map(|(_, c)| *c)
}

/// Parse ASCII hex bytes into a Unicode scalar.
fn hex_scalar(hex: &[u8]) -> Option<char> {
    let text = std::str::from_utf8(hex).ok()?;
    char::from_u32(u32::from_str_radix(text, 16).ok()?)
}

/// Encode `text` to WinAnsi (cp1252) bytes, for showing it with a Standard-14 / `WinAnsiEncoding`
/// simple font (§9.6.6.1) — the inverse of the read-side table. Characters with no WinAnsi code
/// become `?`.
#[must_use]
pub fn winansi_encode(text: &str) -> Vec<u8> {
    let mut reverse = std::collections::HashMap::new();
    for (code, slot) in win_ansi_table().iter().enumerate() {
        if let Some(ch) = slot {
            reverse.entry(*ch).or_insert(code as u8);
        }
    }
    text.chars()
        .map(|ch| reverse.get(&ch).copied().unwrap_or(b'?'))
        .collect()
}

/// WinAnsiEncoding (Windows-1252), built programmatically: ASCII, Latin-1 high range, and the
/// cp1252-specific glyphs in `0x80`–`0x9F`.
fn win_ansi_table() -> [Option<char>; 256] {
    let mut table = [None; 256];
    for code in 0x20u32..=0x7E {
        table[code as usize] = char::from_u32(code);
    }
    for code in 0xA0u32..=0xFF {
        table[code as usize] = char::from_u32(code);
    }
    const HIGH: &[(usize, char)] = &[
        (0x80, '\u{20AC}'),
        (0x82, '\u{201A}'),
        (0x83, '\u{0192}'),
        (0x84, '\u{201E}'),
        (0x85, '\u{2026}'),
        (0x86, '\u{2020}'),
        (0x87, '\u{2021}'),
        (0x88, '\u{02C6}'),
        (0x89, '\u{2030}'),
        (0x8A, '\u{0160}'),
        (0x8B, '\u{2039}'),
        (0x8C, '\u{0152}'),
        (0x8E, '\u{017D}'),
        (0x91, '\u{2018}'),
        (0x92, '\u{2019}'),
        (0x93, '\u{201C}'),
        (0x94, '\u{201D}'),
        (0x95, '\u{2022}'),
        (0x96, '\u{2013}'),
        (0x97, '\u{2014}'),
        (0x98, '\u{02DC}'),
        (0x99, '\u{2122}'),
        (0x9A, '\u{0161}'),
        (0x9B, '\u{203A}'),
        (0x9C, '\u{0153}'),
        (0x9E, '\u{017E}'),
        (0x9F, '\u{0178}'),
    ];
    for &(code, ch) in HIGH {
        table[code] = Some(ch);
    }
    table
}

/// MacRomanEncoding: ASCII plus the Mac OS Roman high range `0x80`–`0xFF`.
fn mac_roman_table() -> [Option<char>; 256] {
    let mut table = [None; 256];
    for code in 0x20u32..=0x7E {
        table[code as usize] = char::from_u32(code);
    }
    const HIGH: [char; 128] = [
        'Ä', 'Å', 'Ç', 'É', 'Ñ', 'Ö', 'Ü', 'á', 'à', 'â', 'ä', 'ã', 'å', 'ç', 'é', 'è', // 80
        'ê', 'ë', 'í', 'ì', 'î', 'ï', 'ñ', 'ó', 'ò', 'ô', 'ö', 'õ', 'ú', 'ù', 'û', 'ü', // 90
        '†', '°', '¢', '£', '§', '•', '¶', 'ß', '®', '©', '™', '´', '¨', '≠', 'Æ', 'Ø', // A0
        '∞', '±', '≤', '≥', '¥', 'µ', '∂', '∑', '∏', 'π', '∫', 'ª', 'º', 'Ω', 'æ', 'ø', // B0
        '¿', '¡', '¬', '√', 'ƒ', '≈', 'Δ', '«', '»', '…', '\u{00A0}', 'À', 'Ã', 'Õ', 'Œ',
        'œ', // C0
        '–', '—', '“', '”', '‘', '’', '÷', '◊', 'ÿ', 'Ÿ', '⁄', '€', '‹', '›', 'ﬁ', 'ﬂ', // D0
        '‡', '·', '‚', '„', '‰', 'Â', 'Ê', 'Á', 'Ë', 'È', 'Í', 'Î', 'Ï', 'Ì', 'Ó', 'Ô', // E0
        '\u{F8FF}', 'Ò', 'Ú', 'Û', 'Ù', 'ı', 'ˆ', '˜', '¯', '˘', '˙', '˚', '¸', '˝', '˛',
        'ˇ', // F0
    ];
    for (i, ch) in HIGH.into_iter().enumerate() {
        table[0x80 + i] = Some(ch);
    }
    table
}

/// A small Adobe-Glyph-List subset: the named glyphs that appear most in `/Differences` arrays
/// beyond plain letters/digits. Single-character ASCII names are handled before this table.
const AGL: &[(&[u8], char)] = &[
    (b"space", ' '),
    (b"exclam", '!'),
    (b"quotedbl", '"'),
    (b"numbersign", '#'),
    (b"dollar", '$'),
    (b"percent", '%'),
    (b"ampersand", '&'),
    (b"quotesingle", '\''),
    (b"parenleft", '('),
    (b"parenright", ')'),
    (b"asterisk", '*'),
    (b"plus", '+'),
    (b"comma", ','),
    (b"hyphen", '-'),
    (b"period", '.'),
    (b"slash", '/'),
    (b"zero", '0'),
    (b"one", '1'),
    (b"two", '2'),
    (b"three", '3'),
    (b"four", '4'),
    (b"five", '5'),
    (b"six", '6'),
    (b"seven", '7'),
    (b"eight", '8'),
    (b"nine", '9'),
    (b"colon", ':'),
    (b"semicolon", ';'),
    (b"less", '<'),
    (b"equal", '='),
    (b"greater", '>'),
    (b"question", '?'),
    (b"at", '@'),
    (b"bracketleft", '['),
    (b"backslash", '\\'),
    (b"bracketright", ']'),
    (b"asciicircum", '^'),
    (b"underscore", '_'),
    (b"grave", '`'),
    (b"braceleft", '{'),
    (b"bar", '|'),
    (b"braceright", '}'),
    (b"asciitilde", '~'),
    // Common typographic and Latin glyphs.
    (b"bullet", '\u{2022}'),
    (b"endash", '\u{2013}'),
    (b"emdash", '\u{2014}'),
    (b"quoteleft", '\u{2018}'),
    (b"quoteright", '\u{2019}'),
    (b"quotedblleft", '\u{201C}'),
    (b"quotedblright", '\u{201D}'),
    (b"quotesinglbase", '\u{201A}'),
    (b"quotedblbase", '\u{201E}'),
    (b"ellipsis", '\u{2026}'),
    (b"dagger", '\u{2020}'),
    (b"daggerdbl", '\u{2021}'),
    (b"trademark", '\u{2122}'),
    (b"fi", '\u{FB01}'),
    (b"fl", '\u{FB02}'),
    (b"degree", '\u{00B0}'),
    (b"copyright", '\u{00A9}'),
    (b"registered", '\u{00AE}'),
    (b"germandbls", '\u{00DF}'),
    (b"eacute", '\u{00E9}'),
    (b"egrave", '\u{00E8}'),
    (b"agrave", '\u{00E0}'),
    (b"ccedilla", '\u{00E7}'),
    (b"ntilde", '\u{00F1}'),
    (b"adieresis", '\u{00E4}'),
    (b"odieresis", '\u{00F6}'),
    (b"udieresis", '\u{00FC}'),
];

#[cfg(test)]
mod tests {
    use super::*;

    fn enc_with(encoding: Object) -> Encoding {
        let mut font = Dictionary::new();
        font.insert(Name::from("Encoding"), encoding);
        Encoding::from_font_dict(&font)
    }

    #[test]
    fn winansi_encode_maps_ascii_latin1_and_specials() {
        assert_eq!(winansi_encode("AZ09"), b"AZ09");
        assert_eq!(winansi_encode("é"), vec![0xE9]); // Latin-1 range
        assert_eq!(winansi_encode("•"), vec![0x95]); // cp1252 bullet
        assert_eq!(winansi_encode("€"), vec![0x80]); // cp1252 euro
        assert_eq!(winansi_encode("☃"), vec![b'?']); // no WinAnsi code → '?'
        // Round-trips against the read-side table.
        let enc = enc_with(Object::Name(Name::from("WinAnsiEncoding")));
        assert_eq!(enc.decode(&winansi_encode("café — €")), "café — €");
    }

    #[test]
    fn default_is_winansi_ascii_and_high() {
        let enc = Encoding::from_font_dict(&Dictionary::new());
        assert_eq!(enc.decode(b"Hi!"), "Hi!");
        // 0x92 is a curly apostrophe in WinAnsi (Latin-1 would give a control char).
        assert_eq!(enc.decode(&[0x92]), "\u{2019}");
        assert_eq!(enc.decode(&[0x80]), "\u{20AC}"); // euro
    }

    #[test]
    fn winansi_named_base() {
        let enc = enc_with(Object::Name(Name::from("WinAnsiEncoding")));
        assert_eq!(enc.decode(&[0xE9]), "é"); // Latin-1 / WinAnsi high
    }

    #[test]
    fn mac_roman_high_bytes() {
        let enc = enc_with(Object::Name(Name::from("MacRomanEncoding")));
        // 0x80 is 'Ä' in MacRoman (vs euro in WinAnsi) — proves the distinct table.
        assert_eq!(enc.decode(&[0x80]), "Ä");
        assert_eq!(enc.decode(&[0xA5]), "•");
    }

    #[test]
    fn differences_remap_by_glyph_name() {
        let mut enc_dict = Dictionary::new();
        enc_dict.insert(
            Name::from("BaseEncoding"),
            Object::Name(Name::from("WinAnsiEncoding")),
        );
        // Re-map code 0x41 ('A') to glyph "bullet", and 0x42 to "uni00E9" (é).
        enc_dict.insert(
            Name::from("Differences"),
            Object::Array(
                vec![
                    Object::Integer(0x41),
                    Object::Name(Name::from("bullet")),
                    Object::Name(Name::from("uni00E9")),
                ]
                .into(),
            ),
        );
        let enc = enc_with(Object::Dictionary(enc_dict));
        assert_eq!(enc.decode(&[0x41, 0x42]), "\u{2022}\u{00E9}");
    }

    #[test]
    fn unknown_glyph_name_is_dropped() {
        let mut enc_dict = Dictionary::new();
        enc_dict.insert(
            Name::from("Differences"),
            Object::Array(vec![Object::Integer(0x41), Object::Name(Name::from("g42"))].into()),
        );
        let enc = enc_with(Object::Dictionary(enc_dict));
        // 0x41 was re-mapped to an unknown glyph → produces nothing; 0x42 still 'B'.
        assert_eq!(enc.decode(&[0x41, 0x42]), "B");
    }

    #[test]
    fn glyph_helpers() {
        assert_eq!(glyph_to_char(b"space"), Some(' '));
        assert_eq!(glyph_to_char(b"A"), Some('A'));
        assert_eq!(glyph_to_char(b"uni0041"), Some('A'));
        assert_eq!(glyph_to_char(b"u00E9"), Some('é'));
        assert_eq!(glyph_to_char(b"unknownglyph"), None);
    }
}
