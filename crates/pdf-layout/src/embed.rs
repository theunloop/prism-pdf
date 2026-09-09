//! Embedding a whole font program as a composite font (ISO 32000-1 §9.7, §9.9).
//!
//! [`Flow`](crate::Flow) and [`Composition`](crate::Composition) subset the fonts they embed to the
//! glyphs they drew, because they know which those are. A caller assembling pages by hand through
//! `Builder` + `PageSpec` + `Content` shows glyph IDs directly and can reference any glyph in the
//! program, so the honest embedding is the *whole* program: every glyph's width in `/W`, every
//! `cmap`-mapped character in `/ToUnicode`, and no `CIDToGIDMap` renumbering.

use pdf_document::CidFont;
use pdf_fonts::GlyphOutlines;

/// Whether a face this crate parsed can be embedded as a composite font at all. Both §9.9 forms
/// show glyph indices as CIDs — `/FontFile2` under `CIDFontType2` for `glyf` outlines,
/// `/FontFile3 /Subtype /OpenType` under `CIDFontType0` for CFF ones (§9.7.4.2). A **CID-keyed**
/// CFF is refused instead of mis-embedded: its own charset maps CIDs to glyphs, so the glyph ids
/// this crate shapes and shows would select other glyphs entirely. Anything with neither outline
/// table (CFF2, bitmap-only) has no embedding form here either.
pub(crate) fn embeddable(outlines: GlyphOutlines) -> bool {
    matches!(outlines, GlyphOutlines::TrueType | GlyphOutlines::Cff)
}

/// Subset `program` down to `used_gids` for embedding, returning the program to embed and the
/// `CIDToGIDMap` bytes its descendant font needs, if any. The content stream has already shown the
/// original glyph ids as codes, so a subset that renumbers them is only safe when a `/CIDToGIDMap`
/// can carry the renumbering — which is a `CIDFontType2` entry only (§9.7.4.2). A CFF program is
/// therefore embedded whole rather than subsetted; its codes stay glyph indices into the program
/// it shipped. Subsetting failure falls back to the whole program the same way.
pub(crate) fn subset_for_embedding(
    outlines: GlyphOutlines,
    program: &[u8],
    used_gids: &[u16],
) -> (Vec<u8>, Option<Vec<u8>>) {
    if outlines != GlyphOutlines::TrueType {
        return (program.to_vec(), None);
    }
    match pdf_fonts::subset_with_map(program, used_gids) {
        Some((subset, map)) => (subset, Some(cid_to_gid_map(&map))),
        None => (program.to_vec(), None),
    }
}

/// Build a `CIDToGIDMap` byte array (§9.7.4.3) from an old→new glyph-ID remapping: indexed by CID
/// (= original glyph ID), two big-endian bytes giving the glyph's ID in the subsetted program.
fn cid_to_gid_map(map: &[(u16, u16)]) -> Vec<u8> {
    let max_cid = map.iter().map(|(old, _)| *old).max().unwrap_or(0) as usize;
    let mut bytes = vec![0u8; (max_cid + 1) * 2];
    for &(old, new) in map {
        let offset = old as usize * 2;
        bytes[offset..offset + 2].copy_from_slice(&new.to_be_bytes());
    }
    bytes
}

/// Build a [`CidFont`] that embeds the complete sfnt `program` (TrueType/OpenType), for use with
/// `Builder::embed_cid_font` when the glyphs a page will show are not known in advance. Widths cover
/// every glyph with a horizontal advance and `/ToUnicode` covers every character the font's Unicode
/// `cmap` maps, so text drawn with `Content::show_glyphs` extracts back as text. `None` when the
/// bytes are not a parseable sfnt face, or carry outlines with no §9.9 embedding form here: a
/// CID-keyed CFF (its own charset maps CIDs to glyphs, so the glyph ids shown as codes would
/// select other glyphs) or a CFF2 face.
#[must_use]
pub fn cid_font_from_sfnt(program: &[u8]) -> Option<CidFont> {
    let info = pdf_fonts::font_info(program)?;
    if !embeddable(info.outlines) {
        return None;
    }
    let widths = pdf_fonts::glyph_advances(program)?;
    let to_unicode = pdf_fonts::glyph_to_unicode(program)
        .map(|map| map.into_iter().collect())
        .unwrap_or_default();
    Some(CidFont {
        program: program.to_vec(),
        postscript_name: info.postscript_name,
        ascent: info.ascent,
        descent: info.descent,
        cap_height: info.cap_height,
        bbox: info.bbox,
        italic_angle: info.italic_angle,
        // Symbolic (bit 3), plus Italic (bit 7) when the face says so — the flags `Flow` uses.
        flags: if info.italic { 4 | 64 } else { 4 },
        default_width: 1000,
        widths,
        to_unicode,
        cid_to_gid: None,
        cff: info.outlines == GlyphOutlines::Cff,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dejavu() -> Option<Vec<u8>> {
        std::fs::read("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf").ok()
    }

    /// A CFF-flavoured (`OTTO`) OpenType face, if the system has one.
    fn opentype_cff() -> Option<Vec<u8>> {
        [
            "/usr/share/fonts/opentype/urw-base35/NimbusSans-Regular.otf",
            "/usr/share/fonts/opentype/urw-base35/NimbusRoman-Regular.otf",
        ]
        .iter()
        .find_map(|path| std::fs::read(path).ok())
    }

    #[test]
    fn whole_program_carries_every_glyph_width_and_the_cmap() {
        let Some(program) = dejavu() else { return };
        let font = cid_font_from_sfnt(&program).expect("DejaVu Sans is an sfnt");
        let metrics = pdf_fonts::analyze_sfnt(&program).unwrap();
        assert_eq!(font.program, program, "embedded unsubsetted");
        assert_eq!(font.widths.len(), usize::from(metrics.glyph_count));
        assert!(font.cid_to_gid.is_none(), "codes are glyph ids");
        // 'A' is mapped by the cmap, so it round-trips through /ToUnicode.
        let a = pdf_fonts::shape_text(&program, "A").unwrap()[0].id;
        assert!(font.to_unicode.contains(&(a, 'A')));
        assert!(font.widths.iter().any(|&(gid, w)| gid == a && w > 0));
    }

    #[test]
    fn a_non_font_is_refused() {
        assert!(cid_font_from_sfnt(b"not a font").is_none());
    }

    #[test]
    fn a_truetype_program_is_not_flagged_as_cff() {
        let Some(program) = dejavu() else { return };
        let font = cid_font_from_sfnt(&program).expect("DejaVu Sans is an sfnt");
        assert!(!font.cff, "glyf outlines embed as /FontFile2");
    }

    #[test]
    fn a_cff_program_is_flagged_for_the_fontfile3_form() {
        // §9.9 embeds a CFF-flavoured OpenType program as /FontFile3 /Subtype /OpenType under a
        // CIDFontType0 descendant. The flag is what carries that decision to the writer; without
        // it the bytes went out as /FontFile2 under CIDFontType2 and no reader rendered them.
        let Some(program) = opentype_cff() else {
            return;
        };
        assert_eq!(program[..4], *b"OTTO", "the fixture is CFF-flavoured");
        let info = pdf_fonts::font_info(&program).expect("an sfnt");
        assert_eq!(info.outlines, GlyphOutlines::Cff);
        let font = cid_font_from_sfnt(&program).expect("embeddable");
        assert!(font.cff);
        assert_eq!(font.program, program, "embedded unsubsetted");
        assert!(font.cid_to_gid.is_none(), "codes are glyph ids");
    }

    #[test]
    fn only_the_two_forms_with_an_embedding_are_accepted() {
        // A CID-keyed CFF maps CIDs to glyphs through its own charset, so the glyph ids this crate
        // shapes and shows would select other glyphs; CFF2 has no §9.9 form at all. Both are
        // refused rather than embedded as something they are not.
        assert!(embeddable(GlyphOutlines::TrueType));
        assert!(embeddable(GlyphOutlines::Cff));
        assert!(!embeddable(GlyphOutlines::CidKeyedCff));
        assert!(!embeddable(GlyphOutlines::Other));
    }

    #[test]
    fn a_cff_program_is_embedded_whole_rather_than_subsetted() {
        // Subsetting renumbers glyphs, and only a CIDFontType2 descendant has a /CIDToGIDMap to
        // carry that renumbering (§9.7.4.2) — so the CFF path keeps the program it was given.
        let Some(program) = opentype_cff() else {
            return;
        };
        let (embedded, map) = subset_for_embedding(GlyphOutlines::Cff, &program, &[3, 4]);
        assert_eq!(embedded, program);
        assert!(map.is_none());

        let Some(truetype) = dejavu() else { return };
        let (subset, map) = subset_for_embedding(GlyphOutlines::TrueType, &truetype, &[3, 4]);
        assert!(subset.len() < truetype.len(), "subsetted");
        let map = map.expect("a renumbering to carry");
        assert_eq!(map.len(), 5 * 2, "indexed by CID, up to glyph 4");
    }
}
