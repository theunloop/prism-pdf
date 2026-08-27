#!/usr/bin/env python3
"""Generate `crates/pdf-fonts/src/standard_metrics.rs` — Standard-14 glyph widths (§9.6.2.2).

The 14 standard fonts are never embedded, so an engine that lays out text in them needs their
advance widths from the Adobe Core14 AFM metrics. This script joins those metrics with the Adobe
Glyph List (glyph name → Unicode) to emit per-font `(codepoint, width)` tables in 1000-em units.

Data sources (fetched at generation time; the resulting Rust tables are committed, so the build
itself needs no network):
  - Core14 AFM metrics, via Apache PDFBox's bundled copies (Adobe, redistributable).
  - Adobe Glyph List (adobe-type-tools/agl-aglfn).

Widths are factual font metrics. The Courier family is monospaced (every glyph 600), so it is
emitted as a constant rather than a table. Symbol/ZapfDingbats use their own encodings and are
intentionally omitted (callers get `None`).

Usage:  python3 tools/gen_standard_metrics.py   (run from the repo root)
"""

import re
import urllib.request

PDFBOX_AFM = (
    "https://raw.githubusercontent.com/apache/pdfbox/trunk/pdfbox/"
    "src/main/resources/org/apache/pdfbox/resources/afm"
)
AGL = "https://raw.githubusercontent.com/adobe-type-tools/agl-aglfn/master/glyphlist.txt"

FONTS = [
    ("HELVETICA", "Helvetica"),
    ("HELVETICA_BOLD", "Helvetica-Bold"),
    ("TIMES_ROMAN", "Times-Roman"),
    ("TIMES_BOLD", "Times-Bold"),
    ("TIMES_ITALIC", "Times-Italic"),
    ("TIMES_BOLD_ITALIC", "Times-BoldItalic"),
]


def fetch(url: str) -> str:
    with urllib.request.urlopen(url, timeout=30) as r:
        return r.read().decode("latin-1")


def load_agl(text: str) -> dict:
    agl = {}
    for line in text.splitlines():
        line = line.strip()
        if not line or line.startswith("#") or ";" not in line:
            continue
        name, codes = line.split(";", 1)
        agl[name] = int(codes.split()[0], 16)
    return agl


def afm_name_widths(text: str) -> dict:
    nw = {}
    for line in text.splitlines():
        if not line.startswith("C "):
            continue
        wx = re.search(r"WX\s+(-?\d+)", line)
        n = re.search(r"\sN\s+(\S+)\s*;", line)
        if wx and n:
            nw[n.group(1)] = int(wx.group(1))
    return nw


def unicode_widths(afm_text: str, agl: dict) -> dict:
    uw = {}
    for name, w in afm_name_widths(afm_text).items():
        cp = agl.get(name)
        if cp is not None and cp <= 0xFFFF and 0 <= w <= 0xFFFF:
            uw.setdefault(cp, w)
    return dict(sorted(uw.items()))


def emit_table(const: str, uw: dict) -> str:
    lines = [f"const {const}: &[(u32, u16)] = &["]
    row = "    "
    for cp, w in uw.items():
        item = f"({cp}, {w}), "
        if len(row) + len(item) > 96:
            lines.append(row.rstrip())
            row = "    "
        row += item
    if row.strip():
        lines.append(row.rstrip())
    lines.append("];")
    return "\n".join(lines)


def main() -> None:
    agl = load_agl(fetch(AGL))
    assert set(afm_name_widths(fetch(f"{PDFBOX_AFM}/Courier.afm")).values()) == {600}

    out = [
        "//! Standard-14 font metrics (ISO 32000-1 §9.6.2.2): glyph advance widths for measuring",
        "//! text set in the built-in fonts, in 1000-unit em space (AFM units).",
        "//!",
        "//! Generated from the Adobe Core14 AFM metrics + the Adobe Glyph List by",
        "//! `tools/gen_standard_metrics.py`. The Courier family is monospaced (every glyph 600).",
        "//! Symbol/ZapfDingbats use their own encodings and are not covered here (callers get `None`).",
        "//! Tables are keyed by Unicode scalar and binary-searched.",
        "",
        "/// Per-font width data: a monospaced advance, or a sorted `(codepoint, width)` table.",
        "enum Table {",
        "    Monospace(u16),",
        "    Variable(&'static [(u32, u16)]),",
        "}",
        "",
        "/// Map a Standard-14 `/BaseFont` name to its width table (oblique variants share metrics).",
        "fn table(base_font: &str) -> Option<Table> {",
        "    Some(match base_font {",
        '        "Courier" | "Courier-Bold" | "Courier-Oblique" | "Courier-BoldOblique" => {',
        "            Table::Monospace(600)",
        "        }",
        '        "Helvetica" | "Helvetica-Oblique" => Table::Variable(HELVETICA),',
        '        "Helvetica-Bold" | "Helvetica-BoldOblique" => Table::Variable(HELVETICA_BOLD),',
        '        "Times-Roman" => Table::Variable(TIMES_ROMAN),',
        '        "Times-Bold" => Table::Variable(TIMES_BOLD),',
        '        "Times-Italic" => Table::Variable(TIMES_ITALIC),',
        '        "Times-BoldItalic" => Table::Variable(TIMES_BOLD_ITALIC),',
        "        _ => return None,",
        "    })",
        "}",
        "",
        "/// Advance width of `ch` in `base_font` (a Standard-14 name), in 1000-em units. `None` if the",
        "/// font is not a supported Standard-14 font; missing glyphs in a covered font give `None`.",
        "#[must_use]",
        "pub fn standard_glyph_width(base_font: &str, ch: char) -> Option<u16> {",
        "    match table(base_font)? {",
        "        Table::Monospace(w) => Some(w),",
        "        Table::Variable(t) => {",
        "            let cp = ch as u32;",
        "            t.binary_search_by(|&(c, _)| c.cmp(&cp)).ok().map(|i| t[i].1)",
        "        }",
        "    }",
        "}",
        "",
        "/// Advance width of `text` set in `base_font` at `size` points: the summed glyph widths scaled",
        "/// by `size / 1000`. Glyphs absent from the font count as its space width. `None` if the font",
        "/// is not a supported Standard-14 font.",
        "#[must_use]",
        "pub fn standard_text_width(base_font: &str, text: &str, size: f64) -> Option<f64> {",
        "    table(base_font)?;",
        "    let fallback = standard_glyph_width(base_font, ' ').unwrap_or(500);",
        "    let units: u64 = text",
        "        .chars()",
        "        .map(|c| u64::from(standard_glyph_width(base_font, c).unwrap_or(fallback)))",
        "        .sum();",
        "    Some(units as f64 * size / 1000.0)",
        "}",
        "",
    ]

    for const, fname in FONTS:
        out.append(emit_table(const, unicode_widths(fetch(f"{PDFBOX_AFM}/{fname}.afm"), agl)))
        out.append("")

    out += [
        "#[cfg(test)]",
        "mod tests {",
        "    use super::*;",
        "",
        "    #[test]",
        "    fn known_glyph_widths() {",
        '        assert_eq!(standard_glyph_width("Helvetica", \' \'), Some(278));',
        '        assert_eq!(standard_glyph_width("Helvetica", \'W\'), Some(944));',
        '        assert_eq!(standard_glyph_width("Helvetica-Oblique", \'W\'), Some(944));',
        '        assert_eq!(standard_glyph_width("Times-Roman", \' \'), Some(250));',
        '        assert_eq!(standard_glyph_width("Courier", \'i\'), Some(600));',
        '        assert_eq!(standard_glyph_width("Courier-Bold", \'W\'), Some(600));',
        '        assert_eq!(standard_glyph_width("Symbol", \'a\'), None);',
        "    }",
        "",
        "    #[test]",
        "    fn text_width_is_additive_and_scaled() {",
        '        // "WW" at 1000pt in Helvetica = 2 * 944 units * 1 = 1888.',
        '        let w = standard_text_width("Helvetica", "WW", 1000.0).unwrap();',
        '        assert!((w - 1888.0).abs() < 1e-6, "{w}");',
        "        // Courier monospace: 5 chars * 600 * 12/1000 = 36.0.",
        '        let c = standard_text_width("Courier", "hello", 12.0).unwrap();',
        '        assert!((c - 36.0).abs() < 1e-6, "{c}");',
        '        assert_eq!(standard_text_width("NotAFont", "x", 12.0), None);',
        "    }",
        "}",
        "",
    ]

    with open("crates/pdf-fonts/src/standard_metrics.rs", "w") as f:
        f.write("\n".join(out))
    print("wrote crates/pdf-fonts/src/standard_metrics.rs")


if __name__ == "__main__":
    main()
