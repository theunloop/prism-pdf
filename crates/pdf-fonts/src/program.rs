//! Embedded font programs (ISO 32000-1 §9.8 font descriptors / §9.9 embedded fonts).
//!
//! A font descriptor embeds its program via `/FontFile` (Type 1), `/FontFile2` (TrueType), or
//! `/FontFile3` (CFF / OpenType, distinguished by the stream's `/Subtype`). This module classifies
//! the program format and, for TrueType/OpenType (sfnt) programs, reports basic face metrics by
//! reusing [`ttf_parser`] (DESIGN.md §6, reuse over reimplementation).

/// The format of an embedded font program (§9.9, Table 126).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FontProgramFormat {
    /// `/FontFile` — a Type 1 font program.
    Type1,
    /// `/FontFile2` — a TrueType (sfnt) font program.
    TrueType,
    /// `/FontFile3` with `/Subtype /Type1C` or `/CIDFontType0C` — a bare CFF program.
    Cff,
    /// `/FontFile3` with `/Subtype /OpenType` — an OpenType (sfnt) program.
    OpenType,
}

impl FontProgramFormat {
    /// The descriptor key that carries a program of this format.
    #[must_use]
    pub fn descriptor_key(self) -> &'static str {
        match self {
            FontProgramFormat::Type1 => "FontFile",
            FontProgramFormat::TrueType => "FontFile2",
            FontProgramFormat::Cff | FontProgramFormat::OpenType => "FontFile3",
        }
    }

    /// Map a `/FontFile3` stream `/Subtype` to its format (§9.9). Unknown subtypes default to CFF,
    /// the common `/FontFile3` payload.
    #[must_use]
    pub fn from_fontfile3_subtype(subtype: Option<&[u8]>) -> Self {
        match subtype {
            Some(b"OpenType") => FontProgramFormat::OpenType,
            _ => FontProgramFormat::Cff,
        }
    }

    /// A conventional file extension for the program (for dumping).
    #[must_use]
    pub fn extension(self) -> &'static str {
        match self {
            FontProgramFormat::Type1 => "pfb",
            FontProgramFormat::TrueType => "ttf",
            FontProgramFormat::Cff => "cff",
            FontProgramFormat::OpenType => "otf",
        }
    }

    /// Whether this format is an sfnt (TrueType/OpenType) parseable by [`analyze_sfnt`].
    #[must_use]
    pub fn is_sfnt(self) -> bool {
        matches!(
            self,
            FontProgramFormat::TrueType | FontProgramFormat::OpenType
        )
    }
}

/// Basic metrics read from an sfnt (TrueType/OpenType) font program (§9.9).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct FaceMetrics {
    /// Font design units per em (the glyph coordinate grid).
    pub units_per_em: u16,
    /// Number of glyphs in the program.
    pub glyph_count: u16,
    /// The font's family name, if the program records one.
    pub family_name: Option<String>,
}

/// Parse an embedded sfnt (TrueType/OpenType) program and report basic metrics, or `None` if the
/// bytes are not a valid font face.
#[must_use]
pub fn analyze_sfnt(data: &[u8]) -> Option<FaceMetrics> {
    let face = ttf_parser::Face::parse(data, 0).ok()?;
    // A font records the family name under several platforms; take the first that decodes (the
    // Macintosh-platform records often do not, while the Windows/Unicode ones do).
    let family_name = face
        .names()
        .into_iter()
        .filter(|name| name.name_id == ttf_parser::name_id::FAMILY)
        .find_map(|name| name.to_string());
    Some(FaceMetrics {
        units_per_em: face.units_per_em(),
        glyph_count: face.number_of_glyphs(),
        family_name,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_classification() {
        assert_eq!(FontProgramFormat::Type1.descriptor_key(), "FontFile");
        assert_eq!(FontProgramFormat::TrueType.descriptor_key(), "FontFile2");
        assert_eq!(FontProgramFormat::Cff.descriptor_key(), "FontFile3");
        assert_eq!(
            FontProgramFormat::from_fontfile3_subtype(Some(b"OpenType")),
            FontProgramFormat::OpenType
        );
        assert_eq!(
            FontProgramFormat::from_fontfile3_subtype(Some(b"Type1C")),
            FontProgramFormat::Cff
        );
        assert_eq!(
            FontProgramFormat::from_fontfile3_subtype(None),
            FontProgramFormat::Cff
        );
        assert!(FontProgramFormat::TrueType.is_sfnt());
        assert!(!FontProgramFormat::Type1.is_sfnt());
        assert_eq!(FontProgramFormat::OpenType.extension(), "otf");
    }

    #[test]
    fn analyze_rejects_non_font_bytes() {
        assert_eq!(analyze_sfnt(b"not a font"), None);
        assert_eq!(analyze_sfnt(&[]), None);
    }

    #[test]
    fn analyze_reads_a_real_system_font_when_available() {
        // Exercise the success path against an installed font if one is present (hermetic when
        // not: the test just no-ops). Real sfnt fixtures are too large/fragile to hand-build.
        let candidates = [
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
            "/usr/share/fonts/truetype/freefont/FreeSans.ttf",
        ];
        for path in candidates {
            if let Ok(bytes) = std::fs::read(path) {
                let metrics = analyze_sfnt(&bytes).expect("a system TTF should parse");
                assert!(metrics.units_per_em > 0);
                assert!(metrics.glyph_count > 0);
                assert!(metrics.family_name.is_some());
                return;
            }
        }
    }
}
