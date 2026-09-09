//! Embedding a whole font program as a composite font (ISO 32000-1 §9.7, §9.9).
//!
//! [`Flow`](crate::Flow) and [`Composition`](crate::Composition) subset the fonts they embed to the
//! glyphs they drew, because they know which those are. A caller assembling pages by hand through
//! `Builder` + `PageSpec` + `Content` shows glyph IDs directly and can reference any glyph in the
//! program, so the honest embedding is the *whole* program: every glyph's width in `/W`, every
//! `cmap`-mapped character in `/ToUnicode`, and no `CIDToGIDMap` renumbering.

use pdf_document::CidFont;

/// Build a [`CidFont`] that embeds the complete sfnt `program` (TrueType/OpenType), for use with
/// `Builder::embed_cid_font` when the glyphs a page will show are not known in advance. Widths cover
/// every glyph with a horizontal advance and `/ToUnicode` covers every character the font's Unicode
/// `cmap` maps, so text drawn with `Content::show_glyphs` extracts back as text. `None` when the
/// bytes are not a parseable sfnt face.
#[must_use]
pub fn cid_font_from_sfnt(program: &[u8]) -> Option<CidFont> {
    let info = pdf_fonts::font_info(program)?;
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
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dejavu() -> Option<Vec<u8>> {
        std::fs::read("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf").ok()
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
}
