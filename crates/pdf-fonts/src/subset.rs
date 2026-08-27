//! Font subsetting (ISO 32000-1 §9.9): rewrite an embedded TrueType/CFF (sfnt) program so it
//! contains only the glyphs actually used, shrinking embedded fonts dramatically.
//!
//! Reuse over reimplementation (DESIGN.md §6): the actual table rewriting (glyf/loca/CFF charstring
//! pruning, directory rebuild) is delegated to the [`subsetter`] crate. This module wraps it with
//! the glyph-selection helpers the PDF layer needs.

use ttf_parser::Face;

/// Subset an sfnt (TrueType/OpenType/CFF) `program` down to `glyphs`, returning the new program.
///
/// `.notdef` (glyph 0) is always retained. Returns `None` if the program is not a subsettable
/// sfnt or subsetting fails.
#[must_use]
pub fn subset_sfnt(program: &[u8], glyphs: &[u16]) -> Option<Vec<u8>> {
    let mut gids: Vec<u16> = glyphs.to_vec();
    if !gids.contains(&0) {
        gids.push(0); // the missing-glyph entry must survive
    }
    let remapper = subsetter::GlyphRemapper::new_from_glyphs(&gids);
    subsetter::subset(program, 0, &remapper).ok()
}

/// A subsetted program plus the `(old glyph id, new glyph id)` remapping it applied.
pub type SubsetWithMap = (Vec<u8>, Vec<(u16, u16)>);

/// Subset `program` to `glyphs` and also return the old→new glyph-ID remapping, so a caller that
/// must keep the original glyph IDs as codes (e.g. a Type0 font whose content already shows them)
/// can build a `CIDToGIDMap`. `.notdef` (0) is always included. `None` on a non-sfnt / failure.
#[must_use]
pub fn subset_with_map(program: &[u8], glyphs: &[u16]) -> Option<SubsetWithMap> {
    let mut gids: Vec<u16> = glyphs.to_vec();
    if !gids.contains(&0) {
        gids.push(0);
    }
    let remapper = subsetter::GlyphRemapper::new_from_glyphs(&gids);
    let subset = subsetter::subset(program, 0, &remapper).ok()?;
    let map: Vec<(u16, u16)> = gids
        .iter()
        .filter_map(|&old| remapper.get(old).map(|new| (old, new)))
        .collect();
    Some((subset, map))
}

/// The glyph IDs (including `.notdef`) needed to render `text` with the sfnt `program`, mapped via
/// the font's character map (§9.6.6 / cmap). Returns `None` if the program is not a valid face.
#[must_use]
pub fn glyphs_for_text(program: &[u8], text: &str) -> Option<Vec<u16>> {
    let face = Face::parse(program, 0).ok()?;
    let mut gids = vec![0u16];
    for ch in text.chars() {
        if let Some(glyph) = face.glyph_index(ch) {
            if !gids.contains(&glyph.0) {
                gids.push(glyph.0);
            }
        }
    }
    Some(gids)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn system_font() -> Option<Vec<u8>> {
        [
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
        ]
        .iter()
        .find_map(|p| std::fs::read(p).ok())
    }

    #[test]
    fn rejects_non_font_bytes() {
        assert_eq!(subset_sfnt(b"not a font", &[0, 1]), None);
        assert_eq!(glyphs_for_text(b"not a font", "hi"), None);
    }

    #[test]
    fn subsets_a_cff_opentype_font_when_available() {
        // An OpenType font with CFF outlines (URW base35 .otf): the same sfnt path must subset
        // it — the `subsetter` crate prunes the CFF charstrings instead of glyf/loca (§9.9).
        let Some(font) = [
            "/usr/share/fonts/opentype/urw-base35/NimbusSans-Regular.otf",
            "/usr/share/fonts/opentype/urw-base35/NimbusRoman-Regular.otf",
            "/usr/share/fonts/opentype/urw-base35/C059-Roman.otf",
        ]
        .iter()
        .find_map(|p| std::fs::read(p).ok()) else {
            return; // hermetic when no OTF is present
        };
        let face = Face::parse(&font, 0).expect("otf parses");
        assert!(face.tables().cff.is_some(), "fixture has CFF outlines");

        let glyphs = glyphs_for_text(&font, "Hi").expect("map text to glyphs");
        let subset = subset_sfnt(&font, &glyphs).expect("subset CFF OTF");
        assert!(subset.len() < font.len(), "subset should be smaller");
        let face = Face::parse(&subset, 0).expect("subset parses");
        assert!(face.number_of_glyphs() as usize <= glyphs.len());
        assert!(face.tables().cff.is_some(), "outlines stay CFF");
    }

    #[test]
    fn subsets_a_system_font_when_available() {
        let Some(font) = system_font() else {
            return; // hermetic when no system font is present
        };

        let glyphs = glyphs_for_text(&font, "Hi").expect("map text to glyphs");
        assert!(glyphs.contains(&0)); // .notdef
        assert!(glyphs.len() >= 3); // notdef + H + i

        let subset = subset_sfnt(&font, &glyphs).expect("subset");
        assert!(subset.len() < font.len(), "subset should be smaller");

        // The subset is still a valid font with far fewer glyphs.
        let face = Face::parse(&subset, 0).expect("subset parses");
        assert!(face.number_of_glyphs() as usize <= glyphs.len());
        assert!(face.number_of_glyphs() >= 1);
    }
}
