//! Metrics and shaping for **embedding** a TrueType font as a composite (Type0) font (§9.7/§9.9).
//!
//! For authoring, embedding a font with `Identity-H` encoding means the content stream shows 2-byte
//! glyph IDs directly. This module reuses [`ttf_parser`] to provide what the PDF layer needs: the
//! descriptor metrics ([`font_info`]), and per-character glyph IDs + advances ([`shape_text`]) —
//! both scaled to the 1000-unit em PDF text space.

use std::collections::BTreeMap;

use ttf_parser::{Face, GlyphId, name_id};

/// Font-wide metrics for a `/FontDescriptor` (§9.8.1), with lengths scaled to 1000-em units.
#[derive(Clone, Debug, PartialEq)]
pub struct FontInfo {
    /// Design units per em (the native scale; advances/metrics here are already rescaled to 1000).
    pub units_per_em: u16,
    /// Ascent (top of glyphs above the baseline).
    pub ascent: i32,
    /// Descent (below the baseline; negative).
    pub descent: i32,
    /// Capital height (top of uppercase letters), falling back to the ascent.
    pub cap_height: i32,
    /// Font bounding box `[x_min, y_min, x_max, y_max]`.
    pub bbox: [i32; 4],
    /// Italic angle in degrees (0 for upright).
    pub italic_angle: f64,
    /// Whether the face is italic/oblique.
    pub italic: bool,
    /// The PostScript name (for `/BaseFont` / `/FontName`), or a generic fallback.
    pub postscript_name: String,
}

/// One shaped character: its glyph ID in the font, its advance (1000-em units), and the source char.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Glyph {
    /// Glyph ID — used directly as the CID under `Identity-H`/`Identity` CIDToGIDMap.
    pub id: u16,
    /// Horizontal advance width in 1000-em units.
    pub advance: u16,
    /// The character this glyph renders (for building `/ToUnicode`).
    pub ch: char,
}

/// Read the descriptor metrics from an sfnt `program`, or `None` if it is not a valid face.
#[must_use]
pub fn font_info(program: &[u8]) -> Option<FontInfo> {
    let face = Face::parse(program, 0).ok()?;
    let upem = face.units_per_em().max(1) as i32;
    let scale = |v: i32| v * 1000 / upem;
    let bb = face.global_bounding_box();
    // A font can carry the PostScript name in several platform records; take the first that decodes
    // (e.g. the Windows UTF-16BE one when a Macintosh record is present but undecodable).
    let postscript_name = face
        .names()
        .into_iter()
        .filter(|n| n.name_id == name_id::POST_SCRIPT_NAME)
        .filter_map(|n| n.to_string())
        .find(|s| !s.is_empty())
        .unwrap_or_else(|| "PrismEmbedded".to_string());

    Some(FontInfo {
        units_per_em: face.units_per_em(),
        ascent: scale(i32::from(face.ascender())),
        descent: scale(i32::from(face.descender())),
        cap_height: scale(i32::from(
            face.capital_height().unwrap_or_else(|| face.ascender()),
        )),
        bbox: [
            scale(i32::from(bb.x_min)),
            scale(i32::from(bb.y_min)),
            scale(i32::from(bb.x_max)),
            scale(i32::from(bb.y_max)),
        ],
        italic_angle: f64::from(face.italic_angle()),
        italic: face.is_italic(),
        postscript_name,
    })
}

/// Shape `text` against the sfnt `program`: one [`Glyph`] per character (missing characters map to
/// glyph 0, `.notdef`). `None` if the program is not a valid face.
#[must_use]
pub fn shape_text(program: &[u8], text: &str) -> Option<Vec<Glyph>> {
    let face = Face::parse(program, 0).ok()?;
    let upem = face.units_per_em().max(1) as i32;
    Some(
        text.chars()
            .map(|ch| {
                let id = face.glyph_index(ch).map_or(0, |g| g.0);
                let advance = face
                    .glyph_hor_advance(GlyphId(id))
                    .map_or(0, |a| (i32::from(a) * 1000 / upem) as u16);
                Glyph { id, advance, ch }
            })
            .collect(),
    )
}

/// Reverse an sfnt's `cmap` into a glyph-id → Unicode map (§9.7 read path): for each character a
/// Unicode `cmap` subtable maps, record `glyph → char`. This recovers text from a composite
/// (Type0) font that has **no** `/ToUnicode` but is embedded with a usable `cmap` — the common
/// `Identity-H` subset case — by going code → CID → glyph → char. `None` for a non-sfnt program.
///
/// When several characters share one glyph, the lowest code point wins (deterministic; favours the
/// base form over presentation variants). Glyph 0 (`.notdef`) is never recorded.
#[must_use]
pub fn glyph_to_unicode(program: &[u8]) -> Option<BTreeMap<u16, char>> {
    let face = Face::parse(program, 0).ok()?;
    let cmap = face.tables().cmap?;
    let mut map: BTreeMap<u16, char> = BTreeMap::new();
    for subtable in cmap.subtables {
        if !subtable.is_unicode() {
            continue;
        }
        subtable.codepoints(|cp| {
            let Some(ch) = char::from_u32(cp) else { return };
            if let Some(GlyphId(gid)) = subtable.glyph_index(cp) {
                if gid == 0 {
                    return;
                }
                // Lowest code point wins on collisions, regardless of iteration order.
                map.entry(gid)
                    .and_modify(|c| {
                        if ch < *c {
                            *c = ch;
                        }
                    })
                    .or_insert(ch);
            }
        });
    }
    Some(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dejavu() -> Option<Vec<u8>> {
        std::fs::read("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf").ok()
    }

    #[test]
    fn reads_metrics_and_name() {
        let Some(font) = dejavu() else { return };
        let info = font_info(&font).unwrap();
        assert!(info.units_per_em > 0);
        assert!(info.ascent > 0 && info.descent < 0);
        assert!(info.bbox[2] > info.bbox[0]);
        assert!(
            info.postscript_name.contains("DejaVu"),
            "{:?}",
            info.postscript_name
        );
    }

    #[test]
    fn shapes_latin_and_cyrillic() {
        let Some(font) = dejavu() else { return };
        let glyphs = shape_text(&font, "AЯ").unwrap(); // Latin A + Cyrillic YA
        assert_eq!(glyphs.len(), 2);
        assert!(
            glyphs[0].id != 0 && glyphs[1].id != 0,
            "both glyphs resolved"
        );
        assert!(glyphs[0].advance > 0 && glyphs[1].advance > 0);
        assert_eq!(glyphs[0].ch, 'A');
    }

    #[test]
    fn rejects_non_font() {
        assert!(font_info(b"not a font").is_none());
        assert!(shape_text(b"not a font", "x").is_none());
    }
}
