//! PDF/UA-1 conformant-output pass (ISO 14289-1): configure a [`Builder`] so its `build()` produces
//! a Universal Accessibility document.
//!
//! PDF/UA builds on Tagged PDF (§14.7): on top of a tagged document it requires a document title
//! shown in the viewer (`/ViewerPreferences /DisplayDocTitle true` + a title in the metadata), a
//! natural language (`/Lang`), all fonts embedded, and alternate text on every figure (§14.8.5).
//! [`make_pdfua`] sets those up and rejects a document that cannot satisfy them. It does not add a
//! PDF/A OutputIntent — PDF/UA is independent of PDF/A (the two can be combined, but need not be).

use std::error::Error;
use std::fmt;

use pdf_document::{Builder, MATHML_STRUCT_NS, PDF2_STRUCT_NS, RoleMapEntry};

use crate::derive_file_id;
use crate::xmp::{XmpMetadata, xmp_packet_ua, xmp_packet_ua2};

/// The standard structure types of the **PDF 1.7** (default) namespace — ISO 32000-1 §14.8.4:
/// grouping, block-level, inline-level and illustration types. An element without `/NS` belongs
/// here.
const STD_TYPES_PDF17: &[&str] = &[
    "Document",
    "Part",
    "Art",
    "Sect",
    "Div",
    "BlockQuote",
    "Caption",
    "TOC",
    "TOCI",
    "Index",
    "NonStruct",
    "Private",
    "P",
    "H",
    "H1",
    "H2",
    "H3",
    "H4",
    "H5",
    "H6",
    "L",
    "LI",
    "Lbl",
    "LBody",
    "Table",
    "TR",
    "TH",
    "TD",
    "THead",
    "TBody",
    "TFoot",
    "Span",
    "Quote",
    "Note",
    "Reference",
    "BibEntry",
    "Code",
    "Link",
    "Annot",
    "Ruby",
    "RB",
    "RT",
    "RP",
    "Warichu",
    "WT",
    "WP",
    "Figure",
    "Formula",
    "Form",
];

/// The standard structure types of the **PDF 2.0** namespace — ISO 32000-2 §14.8.4 Tables
/// 364–375. Headings are `Hn` for any n ≥ 1 (checked separately); `H` exists in the namespace
/// but PDF/UA-2 §8.2.5.12 forbids it (a dedicated guard rejects it first).
const STD_TYPES_PDF20: &[&str] = &[
    "Document",
    "DocumentFragment",
    "Part",
    "Sect",
    "Div",
    "Aside",
    "NonStruct",
    "P",
    "H",
    "Title",
    "FENote",
    "Sub",
    "Em",
    "Strong",
    "Span",
    "Lbl",
    "Link",
    "Annot",
    "Form",
    "Ruby",
    "RB",
    "RT",
    "RP",
    "Warichu",
    "WT",
    "WP",
    "L",
    "LI",
    "LBody",
    "Table",
    "TR",
    "TH",
    "TD",
    "THead",
    "TBody",
    "TFoot",
    "Caption",
    "Figure",
    "Formula",
    "Artifact",
];

/// Whether `tag` is a numbered heading `H1`…`Hn` (any positive n — the PDF 2.0 namespace has no
/// upper bound; the PDF 1.7 set caps at `H6` and lists them explicitly instead).
fn is_numbered_heading(tag: &str) -> bool {
    tag.strip_prefix('H')
        .is_some_and(|digits| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
}

/// Whether structure type `tag`, on an element in namespace `ns` (`None` = the PDF 1.7 default),
/// belongs to that namespace — directly, or through one hop of the supplied `/RoleMapNS` entries
/// (PDF/UA-2 §8.2.4). The MathML namespace is accepted wholesale (its element set is defined by
/// W3C MathML, not ISO 32000).
fn is_known_structure_type(tag: &str, ns: Option<&str>, role_maps: &[RoleMapEntry]) -> bool {
    match ns {
        None => STD_TYPES_PDF17.contains(&tag),
        Some(PDF2_STRUCT_NS) => STD_TYPES_PDF20.contains(&tag) || is_numbered_heading(tag),
        Some(MATHML_STRUCT_NS) => true,
        Some(uri) => role_maps
            .iter()
            .filter(|e| e.ns == uri && e.custom == tag)
            .any(|e| {
                // One mapping hop must land on a standard type (no chained custom namespaces).
                match &e.target_ns {
                    None => STD_TYPES_PDF17.contains(&e.target.as_str()),
                    Some(t) if t == PDF2_STRUCT_NS => {
                        STD_TYPES_PDF20.contains(&e.target.as_str())
                            || is_numbered_heading(&e.target)
                    }
                    Some(t) if t == MATHML_STRUCT_NS => true,
                    Some(_) => false,
                }
            }),
    }
}

/// Why a document could not be made PDF/UA-1 conformant.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PdfUaError {
    /// The document has no logical structure (it is not tagged). PDF/UA is Tagged PDF — author it
    /// through a tagged flow (e.g. `Flow::tagged`) or supply `Builder::structure`.
    NotTagged,
    /// No document title was supplied. PDF/UA requires a title (shown via `/DisplayDocTitle`).
    MissingTitle,
    /// No natural language was supplied. PDF/UA requires a document `/Lang`.
    MissingLanguage,
    /// A `Figure` structure element has no alternate text (`/Alt`, §14.8.5).
    FigureWithoutAlt,
    /// A Standard-14 (non-embedded) font is in use; accessible documents embed all fonts.
    UnembeddedFont,
    /// A `Note` structure element is present — ISO 14289-2 §8.2.5.14 forbids `Note`; use `FENote`
    /// (e.g. `Flow::fenote`). PDF/UA-2 only.
    NoteForbidden,
    /// A generic `H` heading element is present — ISO 14289-2 §8.2.5.12 requires numbered
    /// `H1`…`Hn` headings only. PDF/UA-2 only.
    GenericHeadingForbidden,
    /// An embedded file lacks a description — ISO 14289-2 §8.14.1 requires `/Desc` on every
    /// filespec in `/EmbeddedFiles` (set [`pdf_document::Attachment::description`]). PDF/UA-2 only.
    AttachmentWithoutDesc,
    /// An intra-document link targets a page directly (`LinkTarget::Page`) — ISO 14289-2 §8.8
    /// requires **structure destinations** (`LinkTarget::Element`). PDF/UA-2 only.
    LinkWithoutStructureDest,
    /// A structure element's type does not belong to its namespace — ISO 14289-2 §8.2.4 requires
    /// every element to belong, directly or via `/RoleMapNS`, to the PDF 1.7, PDF 2.0 or MathML
    /// namespace. Fix the tag, set the right [`pdf_document::StructElem::namespace`], or supply a
    /// role map ([`pdf_document::Builder::role_map_ns`]). PDF/UA-2 only.
    UnknownStructureType,
    /// Shown text references the `.notdef` glyph — a character the embedded font has no glyph
    /// for (ISO 14289-1 §7.21.8 / 14289-2 §8.4.5.9). Use a font that covers the text.
    NotdefGlyph,
}

impl fmt::Display for PdfUaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            PdfUaError::NotTagged => "PDF/UA requires a tagged logical structure; none is present",
            PdfUaError::MissingTitle => "PDF/UA requires a document title (XmpMetadata::title)",
            PdfUaError::MissingLanguage => {
                "PDF/UA requires a document language (the `lang` argument)"
            }
            PdfUaError::FigureWithoutAlt => {
                "PDF/UA requires alternate text on every figure; one Figure has no /Alt"
            }
            PdfUaError::UnembeddedFont => {
                "PDF/UA requires embedded fonts; the document uses a Standard-14 font"
            }
            PdfUaError::NoteForbidden => {
                "PDF/UA-2 forbids the Note structure type (ISO 14289-2 8.2.5.14); use FENote"
            }
            PdfUaError::GenericHeadingForbidden => {
                "PDF/UA-2 allows only numbered headings H1..Hn (ISO 14289-2 8.2.5.12), not H"
            }
            PdfUaError::AttachmentWithoutDesc => {
                "PDF/UA-2 requires a description on every embedded file (ISO 14289-2 8.14.1)"
            }
            PdfUaError::LinkWithoutStructureDest => {
                "PDF/UA-2 requires structure destinations for intra-document links \
                 (ISO 14289-2 8.8); use LinkTarget::Element"
            }
            PdfUaError::UnknownStructureType => {
                "PDF/UA-2 requires every structure type to belong to the PDF 1.7, PDF 2.0 or \
                 MathML namespace, directly or via a role map (ISO 14289-2 8.2.4)"
            }
            PdfUaError::NotdefGlyph => {
                "PDF/UA forbids text that references the .notdef glyph — a character the font \
                 has no glyph for (ISO 14289-1 7.21.8 / 14289-2 8.4.5.9)"
            }
        })
    }
}

impl Error for PdfUaError {}

/// Configure `builder` so [`Builder::build`] produces a PDF/UA-1 file in language `lang` (e.g.
/// `"en-US"`):
/// - emits the XMP `/Metadata` with the `pdfuaid:part` 1 identification,
/// - sets `/Lang` and `/ViewerPreferences /DisplayDocTitle true` (§12.2),
/// - syncs `/Info` to the metadata (so the shown title is the title),
/// - sets a deterministic trailer `/ID`.
///
/// Returns a [`PdfUaError`] (changing nothing) if the document is not tagged, has no title or
/// language, still uses a non-embedded font, or has a figure without alternate text.
pub fn make_pdfua(builder: &mut Builder, meta: &XmpMetadata, lang: &str) -> Result<(), PdfUaError> {
    let facts = builder.facts();
    if facts.structure_elements.is_empty() {
        return Err(PdfUaError::NotTagged);
    }
    if facts.standard_14_font_resources > 0 {
        return Err(PdfUaError::UnembeddedFont);
    }
    if facts
        .structure_elements
        .iter()
        .any(|element| element.tag == "Figure" && !element.has_alt)
    {
        return Err(PdfUaError::FigureWithoutAlt);
    }
    if facts.notdef_glyph_referenced {
        // §7.21.8: text-showing operators must not reference .notdef.
        return Err(PdfUaError::NotdefGlyph);
    }
    let Some(title) = meta.title.as_deref().filter(|t| !t.is_empty()) else {
        return Err(PdfUaError::MissingTitle);
    };
    if lang.trim().is_empty() {
        return Err(PdfUaError::MissingLanguage);
    }

    let xmp = xmp_packet_ua(meta);
    let id = derive_file_id(xmp.as_bytes());

    builder.metadata_xmp(xmp.into_bytes());
    builder.file_id(id.to_vec());
    builder.lang(lang);
    builder.display_doc_title(true);

    // Keep /Info in sync with the XMP — the title shown by /DisplayDocTitle comes from here.
    builder.title(title);
    if !meta.authors.is_empty() {
        builder.author(&meta.authors.join(", "));
    }
    if let Some(subject) = &meta.subject {
        builder.subject(subject);
    }
    if let Some(creator) = &meta.creator_tool {
        builder.creator(creator);
    }
    if let Some(producer) = &meta.producer {
        builder.info("Producer", producer);
    }
    Ok(())
}

/// Configure `builder` so [`Builder::build`] produces a **PDF/UA-2** file (ISO 14289-2:2024, on
/// PDF 2.0) in language `lang`:
/// - emits the XMP `/Metadata` with `pdfuaid:part` 2 **and** `pdfuaid:rev` (§5, Table 1),
/// - puts the root `Document` element in the **PDF 2.0 structure namespace** (§8.2.5.2 —
///   [`PDF2_STRUCT_NS`] via `Builder::structure_namespace`), which also auto-stamps `%PDF-2.0`,
/// - sets `/Lang` and `/ViewerPreferences /DisplayDocTitle true` (§8.11.2) and syncs `/Info`,
/// - sets a deterministic trailer `/ID`.
///
/// Returns a [`PdfUaError`] (changing nothing) on the [`make_pdfua`] rejections — untagged, no
/// title/language, non-embedded font, figure without `/Alt` — plus the UA-2-specific ones: a
/// `Note` element (§8.2.5.14 forbids it; use `FENote`), a generic `H` heading (§8.2.5.12), or an
/// embedded file without a description (§8.14.1).
pub fn make_pdfua2(
    builder: &mut Builder,
    meta: &XmpMetadata,
    lang: &str,
) -> Result<(), PdfUaError> {
    let facts = builder.facts();
    if facts.structure_elements.is_empty() {
        return Err(PdfUaError::NotTagged);
    }
    if facts.standard_14_font_resources > 0 {
        return Err(PdfUaError::UnembeddedFont);
    }
    // UA-2 accepts /ActualText as the alternative to /Alt on a Figure (§8.2.5.28).
    if facts
        .structure_elements
        .iter()
        .any(|element| element.tag == "Figure" && !element.has_alt && !element.has_actual_text)
    {
        return Err(PdfUaError::FigureWithoutAlt);
    }
    if facts
        .structure_elements
        .iter()
        .any(|element| element.tag == "Note")
    {
        return Err(PdfUaError::NoteForbidden);
    }
    if facts
        .structure_elements
        .iter()
        .any(|element| element.tag == "H")
    {
        return Err(PdfUaError::GenericHeadingForbidden);
    }
    if facts.undescribed_files > 0 {
        return Err(PdfUaError::AttachmentWithoutDesc);
    }
    if facts.direct_page_links > 0 {
        return Err(PdfUaError::LinkWithoutStructureDest);
    }
    if facts.notdef_glyph_referenced {
        return Err(PdfUaError::NotdefGlyph);
    }
    // Every structure type must belong to the PDF 1.7 / PDF 2.0 / MathML namespace, directly or
    // via one role-map hop (§8.2.4).
    if facts.structure_elements.iter().any(|element| {
        !is_known_structure_type(&element.tag, element.namespace.as_deref(), &facts.role_maps)
    }) {
        return Err(PdfUaError::UnknownStructureType);
    }
    let Some(title) = meta.title.as_deref().filter(|t| !t.is_empty()) else {
        return Err(PdfUaError::MissingTitle);
    };
    if lang.trim().is_empty() {
        return Err(PdfUaError::MissingLanguage);
    }

    let xmp = xmp_packet_ua2(meta);
    let id = derive_file_id(xmp.as_bytes());

    builder.metadata_xmp(xmp.into_bytes());
    builder.file_id(id.to_vec());
    builder.lang(lang);
    builder.display_doc_title(true);
    // The root Document element must be in the PDF 2.0 structure namespace (§8.2.5.2); child
    // elements without /NS stay in the default (PDF 1.7) namespace, which §8.2.4 permits.
    builder.structure_namespace(PDF2_STRUCT_NS);

    // Keep /Info in sync with the XMP — the title shown by /DisplayDocTitle comes from here.
    builder.title(title);
    if !meta.authors.is_empty() {
        builder.author(&meta.authors.join(", "));
    }
    if let Some(subject) = &meta.subject {
        builder.subject(subject);
    }
    if let Some(creator) = &meta.creator_tool {
        builder.creator(creator);
    }
    if let Some(producer) = &meta.producer {
        builder.info("Producer", producer);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdf_cos::{Name, Object};
    use pdf_document::{Document, PageSpec, StructElem};

    fn xmp_with_title() -> XmpMetadata {
        XmpMetadata {
            title: Some("Accessible Report".into()),
            authors: vec!["Prism PDF".into()],
            ..Default::default()
        }
    }

    /// A tagged builder with one paragraph and (optionally) a figure element.
    fn tagged_builder(with_figure: Option<Option<&str>>) -> Builder {
        let mut builder = Builder::new();
        builder.add_page(PageSpec::new(b"/P <</MCID 0>> BDC\nEMC\n".to_vec()));
        let mut p = StructElem::new("P");
        p.push_content(0, 0);
        let mut structure = vec![p];
        if let Some(alt) = with_figure {
            let mut fig = StructElem::new("Figure");
            fig.push_content(0, 1);
            fig.alt = alt.map(str::to_string);
            structure.push(fig);
        }
        builder.structure(structure);
        builder
    }

    #[test]
    fn rejects_untagged() {
        let mut builder = Builder::new();
        builder.add_page(PageSpec::new(Vec::new()));
        assert_eq!(
            make_pdfua(&mut builder, &xmp_with_title(), "en-US"),
            Err(PdfUaError::NotTagged)
        );
    }

    #[test]
    fn requires_title_and_language() {
        let mut b1 = tagged_builder(None);
        assert_eq!(
            make_pdfua(&mut b1, &XmpMetadata::default(), "en-US"),
            Err(PdfUaError::MissingTitle)
        );
        let mut b2 = tagged_builder(None);
        assert_eq!(
            make_pdfua(&mut b2, &xmp_with_title(), "  "),
            Err(PdfUaError::MissingLanguage)
        );
    }

    #[test]
    fn rejects_figure_without_alt() {
        let mut builder = tagged_builder(Some(None)); // a Figure with no /Alt
        assert_eq!(
            make_pdfua(&mut builder, &xmp_with_title(), "en-US"),
            Err(PdfUaError::FigureWithoutAlt)
        );
    }

    #[test]
    fn ua2_rejects_note_generic_heading_and_undescribed_attachment() {
        // A Note element (UA-1 style) is forbidden in UA-2 (§8.2.5.14).
        let mut b = tagged_builder(None);
        let mut note = StructElem::new("Note").id("n1");
        note.push_content(0, 1);
        b.add_structure_element(note);
        assert_eq!(
            make_pdfua2(&mut b, &xmp_with_title(), "en-US"),
            Err(PdfUaError::NoteForbidden)
        );

        // A generic H heading is forbidden (§8.2.5.12) — even nested.
        let mut b = tagged_builder(None);
        let mut sect = StructElem::new("Sect");
        sect.push_child(StructElem::new("H"));
        b.add_structure_element(sect);
        assert_eq!(
            make_pdfua2(&mut b, &xmp_with_title(), "en-US"),
            Err(PdfUaError::GenericHeadingForbidden)
        );

        // An embedded file without /Desc is rejected (§8.14.1); adding the description fixes it.
        let attach = |desc: Option<&str>| pdf_document::Attachment {
            name: "data.xml".into(),
            mime: "text/xml".into(),
            relationship: "Data".into(),
            description: desc.map(str::to_string),
            mod_date: None,
            data: b"<x/>".to_vec(),
        };
        let mut b = tagged_builder(None);
        b.attach_file(attach(None));
        assert_eq!(
            make_pdfua2(&mut b, &xmp_with_title(), "en-US"),
            Err(PdfUaError::AttachmentWithoutDesc)
        );
        let mut b = tagged_builder(None);
        b.attach_file(attach(Some("machine-readable data")));
        assert_eq!(make_pdfua2(&mut b, &xmp_with_title(), "en-US"), Ok(()));

        // The inventory covers associated files below the catalog too. Form fields were one of
        // the attachment-bearing surfaces omitted by the former Builder predicate.
        use pdf_document::FormFieldSpec;
        let mut b = tagged_builder(None);
        b.add_form_field(
            0,
            FormFieldSpec::Checkbox {
                rect: [0.0, 0.0, 10.0, 10.0],
                name: "accept".into(),
                checked: false,
                tooltip: Some("Accept".into()),
            },
            vec![attach(None)],
        );
        assert_eq!(
            make_pdfua2(&mut b, &xmp_with_title(), "en-US"),
            Err(PdfUaError::AttachmentWithoutDesc)
        );

        // An intra-document link with an explicit page destination is rejected (§8.8); the
        // structure-destination form passes.
        use pdf_document::{AnnotationSpec, LinkTarget};
        let mut b = tagged_builder(None);
        b.add_annotation(
            0,
            AnnotationSpec::Link {
                rect: [0.0, 0.0, 10.0, 10.0],
                target: LinkTarget::Page(0),
                contents: Some("next page".into()),
            },
            Vec::new(),
        );
        assert_eq!(
            make_pdfua2(&mut b, &xmp_with_title(), "en-US"),
            Err(PdfUaError::LinkWithoutStructureDest)
        );
        let mut b = tagged_builder(None);
        b.add_annotation(
            0,
            AnnotationSpec::Link {
                rect: [0.0, 0.0, 10.0, 10.0],
                target: LinkTarget::Element("some-id".into()),
                contents: Some("to the section".into()),
            },
            Vec::new(),
        );
        assert_eq!(make_pdfua2(&mut b, &xmp_with_title(), "en-US"), Ok(()));

        // A Figure with /ActualText instead of /Alt passes UA-2 (§8.2.5.28) but not UA-1 (§7.3).
        let mut b = tagged_builder(None);
        let mut fig = StructElem::new("Figure").actual_text("a bar chart");
        fig.push_content(0, 1);
        b.add_structure_element(fig);
        assert_eq!(make_pdfua2(&mut b, &xmp_with_title(), "en-US"), Ok(()));
        let mut b = tagged_builder(None);
        let mut fig = StructElem::new("Figure").actual_text("a bar chart");
        fig.push_content(0, 1);
        b.add_structure_element(fig);
        assert_eq!(
            make_pdfua(&mut b, &xmp_with_title(), "en-US"),
            Err(PdfUaError::FigureWithoutAlt)
        );

        // Every UA-2 error has a distinct Display message.
        for e in [
            PdfUaError::NoteForbidden,
            PdfUaError::GenericHeadingForbidden,
            PdfUaError::AttachmentWithoutDesc,
            PdfUaError::LinkWithoutStructureDest,
        ] {
            assert!(!e.to_string().is_empty());
        }
    }

    #[test]
    fn ua2_namespace_membership_and_notdef_guards() {
        use pdf_document::RoleMapEntry;
        // An unknown type in the default (1.7) namespace is rejected…
        let mut b = tagged_builder(None);
        b.add_structure_element(StructElem::new("Chapter"));
        assert_eq!(
            make_pdfua2(&mut b, &xmp_with_title(), "en-US"),
            Err(PdfUaError::UnknownStructureType)
        );
        // …as is a 2.0-only type left in the default namespace…
        let mut b = tagged_builder(None);
        b.add_structure_element(StructElem::new("FENote").id("n1"));
        assert_eq!(
            make_pdfua2(&mut b, &xmp_with_title(), "en-US"),
            Err(PdfUaError::UnknownStructureType)
        );
        // …and a custom-namespace type without a role map.
        let mut b = tagged_builder(None);
        b.add_structure_element(StructElem::new("Callout").namespace("https://example.org/ns"));
        assert_eq!(
            make_pdfua2(&mut b, &xmp_with_title(), "en-US"),
            Err(PdfUaError::UnknownStructureType)
        );
        // A role map to a standard type fixes it; H9 is a valid 2.0 heading; MathML is open.
        let mut b = tagged_builder(None);
        b.add_structure_element(StructElem::new("Callout").namespace("https://example.org/ns"));
        b.add_structure_element(StructElem::new("H9").namespace(pdf_document::PDF2_STRUCT_NS));
        let mut formula = StructElem::new("Formula");
        formula.push_child(StructElem::new("math").namespace(pdf_document::MATHML_STRUCT_NS));
        b.add_structure_element(formula);
        b.role_map_ns(vec![RoleMapEntry {
            ns: "https://example.org/ns".to_string(),
            custom: "Callout".to_string(),
            target: "Aside".to_string(),
            target_ns: Some(pdf_document::PDF2_STRUCT_NS.to_string()),
        }]);
        assert_eq!(make_pdfua2(&mut b, &xmp_with_title(), "en-US"), Ok(()));
        // A role map hopping to another custom namespace is not a standard resolution.
        let mut b = tagged_builder(None);
        b.add_structure_element(StructElem::new("Callout").namespace("https://example.org/ns"));
        b.role_map_ns(vec![RoleMapEntry {
            ns: "https://example.org/ns".to_string(),
            custom: "Callout".to_string(),
            target: "Other".to_string(),
            target_ns: Some("https://example.org/other".to_string()),
        }]);
        assert_eq!(
            make_pdfua2(&mut b, &xmp_with_title(), "en-US"),
            Err(PdfUaError::UnknownStructureType)
        );

        // A flagged .notdef reference is rejected by both production passes.
        let mut b = tagged_builder(None);
        b.flag_notdef_reference();
        assert_eq!(
            make_pdfua2(&mut b, &xmp_with_title(), "en-US"),
            Err(PdfUaError::NotdefGlyph)
        );
        let mut b = tagged_builder(None);
        b.flag_notdef_reference();
        assert_eq!(
            make_pdfua(&mut b, &xmp_with_title(), "en-US"),
            Err(PdfUaError::NotdefGlyph)
        );
        for e in [PdfUaError::UnknownStructureType, PdfUaError::NotdefGlyph] {
            assert!(!e.to_string().is_empty());
        }
    }

    #[test]
    fn ua2_marks_a_conformant_document() {
        let mut builder = tagged_builder(Some(Some("a chart")));
        make_pdfua2(&mut builder, &xmp_with_title(), "en-US").unwrap();
        let bytes = builder.build();
        assert!(bytes.starts_with(b"%PDF-2.0"), "namespace forces PDF 2.0");
        let doc = Document::open(bytes).unwrap();
        let catalog = doc.catalog().unwrap();

        // The XMP declares PDF/UA-2 with the required revision year.
        let Some(Object::Reference(meta_ref)) = catalog.get(&Name::from("Metadata")) else {
            panic!("no /Metadata");
        };
        let Object::Stream(stream) = doc.get(*meta_ref).unwrap() else {
            panic!("metadata not a stream");
        };
        let xmp = String::from_utf8_lossy(stream.raw().as_ref()).into_owned();
        assert!(xmp.contains("<pdfuaid:part>2</pdfuaid:part>"));
        assert!(xmp.contains("<pdfuaid:rev>2024</pdfuaid:rev>"));

        // The root Document element carries the PDF 2.0 structure namespace (§8.2.5.2).
        assert_eq!(
            doc.structure_namespaces().unwrap(),
            vec![pdf_document::PDF2_STRUCT_NS.to_string()]
        );
    }

    #[test]
    fn marks_a_conformant_document() {
        let mut builder = tagged_builder(Some(Some("a chart")));
        make_pdfua(&mut builder, &xmp_with_title(), "en-US").unwrap();
        let doc = Document::open(builder.build()).unwrap();
        let catalog = doc.catalog().unwrap();

        // /Lang, /ViewerPreferences /DisplayDocTitle, and the structure tree are present.
        assert_eq!(
            catalog.get(&Name::from("Lang")),
            Some(&Object::String(pdf_cos::PdfString::from(b"en-US".to_vec())))
        );
        let Some(Object::Dictionary(prefs)) = catalog.get(&Name::from("ViewerPreferences")) else {
            panic!("no /ViewerPreferences");
        };
        assert_eq!(
            prefs.get(&Name::from("DisplayDocTitle")),
            Some(&Object::Boolean(true))
        );
        assert!(catalog.get(&Name::from("StructTreeRoot")).is_some());

        // The XMP declares PDF/UA-1.
        let Some(Object::Reference(meta_ref)) = catalog.get(&Name::from("Metadata")) else {
            panic!("no /Metadata");
        };
        let Object::Stream(stream) = doc.get(*meta_ref).unwrap() else {
            panic!("metadata not a stream");
        };
        let xmp = String::from_utf8_lossy(stream.raw().as_ref()).into_owned();
        assert!(xmp.contains("<pdfuaid:part>1</pdfuaid:part>"));
        assert!(xmp.contains("xmlns:pdfuaid="));
    }
}
