//! PDF/A conformant-output pass (ISO 19005, level B): configure a [`Builder`] so its `build()`
//! emits a PDF/A file.
//!
//! [`make_pdfa`] attaches the three things PDF/A requires on top of a normal document: the XMP
//! `/Metadata` packet with the identification schema (§14.3.2), an sRGB OutputIntent (§14.11.5),
//! and a trailer file `/ID` (§14.4); it also syncs the `/Info` dictionary to the same values (so
//! the two don't disagree) and rejects documents that still use non-embedded Standard-14 fonts,
//! which PDF/A forbids. Fonts must therefore be embedded by the authoring layer beforehand.

use std::error::Error;
use std::fmt;
use std::hash::Hasher;

use pdf_document::Builder;

use crate::output_intent::OutputIntentProfile;
use crate::xmp::{PdfAConformance, XmpMetadata, xmp_packet};

/// Why a document could not be made PDF/A-conformant.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PdfAError {
    /// The builder still exposes a Standard-14 (non-embedded) font; PDF/A requires all fonts
    /// embedded. Re-author the text with an embedded font.
    UnembeddedFont,
    /// The builder has embedded file attachments, which only PDF/A-3 and PDF/A-4E/4F permit — use
    /// [`PdfAConformance::A3b`] (or `A4f`/`A4e` on the PDF 2.0 line).
    AttachmentRequiresPdfA3,
    /// Conformance level A was requested but the document has no logical structure (it is not
    /// tagged). Author it through a tagged flow (e.g. `Flow::tagged`) or supply
    /// `Builder::structure`, or target a level-B/U conformance instead.
    LevelARequiresTagging,
    /// A PDF/A-1 conformance was requested but the document uses image transparency (a soft
    /// mask), which PDF 1.4-based PDF/A-1 forbids (ISO 19005-1 §6.4) — target PDF/A-2 or later,
    /// or drop the alpha channel.
    TransparencyRequiresPdfA2,
}

impl fmt::Display for PdfAError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PdfAError::UnembeddedFont => f.write_str(
                "PDF/A forbids non-embedded fonts; the document uses a Standard-14 font",
            ),
            PdfAError::AttachmentRequiresPdfA3 => f.write_str(
                "embedded file attachments require PDF/A-3 or PDF/A-4E/4F \
                 (use PdfAConformance::A3b or A4f)",
            ),
            PdfAError::LevelARequiresTagging => f.write_str(
                "PDF/A level A requires a tagged logical structure; the document is not tagged",
            ),
            PdfAError::TransparencyRequiresPdfA2 => f.write_str(
                "PDF/A-1 forbids image transparency (soft masks); target PDF/A-2 or later",
            ),
        }
    }
}

impl Error for PdfAError {}

/// Configure `builder` so [`Builder::build`] produces a PDF/A file at `conformance`:
/// - attaches the XMP `/Metadata` packet built from `meta` (with the `pdfaid` schema),
/// - embeds the bundled sRGB OutputIntent,
/// - sets a deterministic trailer `/ID` (derived from the metadata),
/// - syncs the `/Info` dictionary to match the XMP.
///
/// Returns [`PdfAError::UnembeddedFont`] (changing nothing) if the builder still uses a Standard-14
/// font — embed fonts first.
pub fn make_pdfa(
    builder: &mut Builder,
    conformance: PdfAConformance,
    meta: &XmpMetadata,
) -> Result<(), PdfAError> {
    make_pdfa_with_output_intent(builder, conformance, meta, &OutputIntentProfile::srgb())
}

/// Like [`make_pdfa`], but with a caller-chosen OutputIntent destination profile (§14.11.5) instead
/// of the bundled sRGB one.
///
/// The OutputIntent's colour space governs which device colour spaces the file may use under PDF/A
/// §6.2.4.3: pass [`OutputIntentProfile::srgb`] for DeviceRGB/Gray content, or a CMYK printing
/// condition (`OutputIntentProfile::new(cmyk_icc, 4, "…")`) to make `DeviceCMYK` content
/// (`Content::set_fill_cmyk`) conformant. Mixing colour families in one file needs the matching
/// `/Default*` colour spaces, which this pass does not add — choose the profile for the colour the
/// document actually uses.
///
/// # Errors
/// Same as [`make_pdfa`]: an unembedded Standard-14 font, attachments without PDF/A-3, or level A
/// without a logical structure.
pub fn make_pdfa_with_output_intent(
    builder: &mut Builder,
    conformance: PdfAConformance,
    meta: &XmpMetadata,
    output_intent: &OutputIntentProfile,
) -> Result<(), PdfAError> {
    let facts = builder.facts();
    if facts.standard_14_font_resources > 0 {
        return Err(PdfAError::UnembeddedFont);
    }
    if facts.embedded_files > 0 && !conformance.allows_attachments() {
        return Err(PdfAError::AttachmentRequiresPdfA3);
    }
    // Level A is Tagged PDF: the document must carry a logical structure (§14.7).
    if conformance.is_level_a() && facts.structure_elements.is_empty() {
        return Err(PdfAError::LevelARequiresTagging);
    }
    // PDF/A-1 (on PDF 1.4) predates the transparency model: soft-masked images are forbidden
    // (ISO 19005-1 §6.4); PDF/A-2 onwards permit them.
    if conformance.part() == 1 && facts.soft_mask_images > 0 {
        return Err(PdfAError::TransparencyRequiresPdfA2);
    }

    let xmp = xmp_packet(meta, conformance);
    let id = derive_file_id(xmp.as_bytes());

    builder.metadata_xmp(xmp.into_bytes());
    builder.output_intent(
        output_intent.icc().to_vec(),
        output_intent.n(),
        output_intent.identifier(),
    );
    builder.file_id(id.to_vec());
    // Each PDF/A part is defined against a specific PDF version: part 1 → PDF 1.4, parts 2/3 →
    // PDF 1.7 (ISO 32000-1), part 4 → PDF 2.0. Pin the header so the auto-minimum (which would
    // see only ≤1.4 constructs) doesn't stamp a version the validators tie to a different part.
    match conformance.part() {
        1 => builder.version(1, 4),
        4 => builder.version(2, 0),
        _ => builder.version(1, 7),
    };

    // Keep /Info (§14.3.3) consistent with the XMP — PDF/A requires the two not to disagree.
    // PDF/A-4 (on PDF 2.0, where /Info is deprecated) instead *forbids* an Info dictionary with
    // anything beyond /ModDate (ISO 19005-4 §6.1.3): the XMP alone carries the metadata there.
    if conformance.part() != 4 {
        if let Some(title) = &meta.title {
            builder.title(title);
        }
        if !meta.authors.is_empty() {
            builder.author(&meta.authors.join(", "));
        }
        if let Some(subject) = &meta.subject {
            builder.subject(subject);
        }
        if let Some(keywords) = &meta.keywords {
            builder.keywords(keywords);
        }
        if let Some(creator) = &meta.creator_tool {
            builder.creator(creator);
        }
        if let Some(producer) = &meta.producer {
            builder.info("Producer", producer);
        }
    } else {
        builder.clear_info();
    }
    Ok(())
}

/// Derive a deterministic 16-byte file identifier from `seed` (§14.4). Two hashes of the seed give
/// a stable, content-dependent `/ID` without needing a clock or RNG (the `/ID` need only exist and
/// be reproducible, not be cryptographic).
#[must_use]
pub fn derive_file_id(seed: &[u8]) -> [u8; 16] {
    let mut id = [0u8; 16];
    let mut h1 = std::collections::hash_map::DefaultHasher::new();
    h1.write(seed);
    id[..8].copy_from_slice(&h1.finish().to_be_bytes());
    let mut h2 = std::collections::hash_map::DefaultHasher::new();
    h2.write(seed);
    h2.write_u8(0x9e); // perturb so the second half differs from the first
    id[8..].copy_from_slice(&h2.finish().to_be_bytes());
    id
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdf_cos::{Name, Object};
    use pdf_document::{Attachment, Document, PageSpec, StdFont, StructElem};

    fn xml_attachment() -> Attachment {
        Attachment {
            name: "invoice.xml".into(),
            mime: "text/xml".into(),
            relationship: "Data".into(),
            description: Some("e-invoice".into()),
            mod_date: None,
            data: b"<invoice/>".to_vec(),
        }
    }

    #[test]
    fn attachments_require_pdfa3() {
        let mut builder = Builder::new();
        builder.add_page(PageSpec::new(Vec::new()));
        builder.attach_file(xml_attachment());
        // 2B rejects attachments; 3B accepts them.
        assert_eq!(
            make_pdfa(&mut builder, PdfAConformance::A2b, &XmpMetadata::default()),
            Err(PdfAError::AttachmentRequiresPdfA3)
        );
        assert!(make_pdfa(&mut builder, PdfAConformance::A3b, &XmpMetadata::default()).is_ok());
    }

    #[test]
    fn pdfa3_embeds_the_attachment() {
        let mut builder = Builder::new();
        builder.add_page(PageSpec::new(Vec::new()));
        builder.attach_file(xml_attachment());
        make_pdfa(&mut builder, PdfAConformance::A3b, &XmpMetadata::default()).unwrap();
        let doc = Document::open(builder.build()).unwrap();
        let catalog = doc.catalog().unwrap();

        // /AF associates the file with the document, and /Names/EmbeddedFiles holds the file spec.
        let Some(Object::Array(af)) = catalog.get(&Name::from("AF")) else {
            panic!("no /AF: {catalog:?}");
        };
        let Some(Object::Reference(fs_ref)) = af.iter().next() else {
            panic!("empty /AF");
        };
        let Object::Dictionary(fs) = doc.get(*fs_ref).unwrap() else {
            panic!("filespec not a dict");
        };
        assert_eq!(
            fs.get(&Name::from("AFRelationship")),
            Some(&Object::Name(Name::from("Data")))
        );
        // The embedded file stream carries the bytes and a MIME /Subtype.
        let Some(Object::Dictionary(ef)) = fs.get(&Name::from("EF")) else {
            panic!("no /EF");
        };
        let Some(Object::Reference(ef_ref)) = ef.get(&Name::from("F")) else {
            panic!("no /EF /F");
        };
        let Object::Stream(stream) = doc.get(*ef_ref).unwrap() else {
            panic!("embedded file not a stream");
        };
        assert_eq!(stream.raw().as_ref(), b"<invoice/>");
        assert_eq!(
            stream.dict().get(&Name::from("Subtype")),
            Some(&Object::Name(Name::from("text/xml")))
        );
    }

    #[test]
    fn level_a_requires_a_tagged_document() {
        // An untagged document cannot be PDF/A level A.
        let mut untagged = Builder::new();
        untagged.add_page(PageSpec::new(Vec::new()));
        assert_eq!(
            make_pdfa(&mut untagged, PdfAConformance::A2a, &XmpMetadata::default()),
            Err(PdfAError::LevelARequiresTagging)
        );
    }

    #[test]
    fn level_a_tagged_document_declares_conformance_a() {
        let mut builder = Builder::new();
        builder.add_page(PageSpec::new(b"/P <</MCID 0>> BDC\nEMC\n".to_vec()));
        builder.lang("en-US");
        let mut p = StructElem::new("P");
        p.push_content(0, 0);
        builder.structure(vec![p]);

        make_pdfa(&mut builder, PdfAConformance::A2a, &XmpMetadata::default()).unwrap();
        let doc = Document::open(builder.build()).unwrap();
        let catalog = doc.catalog().unwrap();

        // The structure tree survives the PDF/A pass.
        assert!(catalog.get(&Name::from("StructTreeRoot")).is_some());

        // The XMP identification declares level A, part 2.
        let Some(Object::Reference(meta_ref)) = catalog.get(&Name::from("Metadata")) else {
            panic!("no /Metadata");
        };
        let Object::Stream(meta_stream) = doc.get(*meta_ref).unwrap() else {
            panic!("metadata not a stream");
        };
        let xmp = String::from_utf8_lossy(meta_stream.raw().as_ref()).into_owned();
        assert!(xmp.contains("<pdfaid:part>2</pdfaid:part>"));
        assert!(xmp.contains("<pdfaid:conformance>A</pdfaid:conformance>"));
    }

    #[test]
    fn rejects_standard_14_fonts() {
        let mut builder = Builder::new();
        builder.add_page(
            PageSpec::new(b"BT /F1 12 Tf ET".to_vec()).standard_font("F1", StdFont::Helvetica),
        );
        let err = make_pdfa(&mut builder, PdfAConformance::A2b, &XmpMetadata::default());
        assert_eq!(err, Err(PdfAError::UnembeddedFont));
    }

    #[test]
    fn derive_file_id_is_deterministic_and_split() {
        let a = derive_file_id(b"hello");
        assert_eq!(a, derive_file_id(b"hello"));
        assert_ne!(a, derive_file_id(b"world"));
        // The two 8-byte halves differ (the perturbation worked).
        assert_ne!(a[..8], a[8..]);
    }

    #[test]
    fn produces_a_pdfa_marked_file() {
        // A blank page (no fonts) is the minimal conformant document — enough to prove the wiring.
        let mut builder = Builder::new();
        builder.add_page(PageSpec::new(Vec::new()));
        let meta = XmpMetadata {
            title: Some("Archived".into()),
            authors: vec!["Jane Doe".into()],
            producer: Some("Prism PDF".into()),
            ..Default::default()
        };
        make_pdfa(&mut builder, PdfAConformance::A2b, &meta).unwrap();
        let bytes = builder.build();

        // Trailer /ID present (required by PDF/A); header is PDF 1.7 (PDF/A-2 base).
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.starts_with("%PDF-1.7"));
        assert!(text.contains("/ID [<"));

        let doc = Document::open(bytes).unwrap();
        let catalog = doc.catalog().unwrap();

        // Catalog references the XMP /Metadata stream, and it carries the PDF/A identification.
        let Some(Object::Reference(meta_ref)) = catalog.get(&Name::from("Metadata")) else {
            panic!("no /Metadata: {catalog:?}");
        };
        let Object::Stream(meta_stream) = doc.get(*meta_ref).unwrap() else {
            panic!("metadata not a stream");
        };
        let xmp = String::from_utf8_lossy(meta_stream.raw().as_ref()).into_owned();
        assert!(xmp.contains("<pdfaid:part>2</pdfaid:part>"));
        assert!(xmp.contains("<pdfaid:conformance>B</pdfaid:conformance>"));
        assert!(xmp.contains("<dc:title>"));

        // Catalog references an OutputIntent whose DestOutputProfile is the bundled sRGB ICC.
        let Some(Object::Array(intents)) = catalog.get(&Name::from("OutputIntents")) else {
            panic!("no /OutputIntents");
        };
        let Some(Object::Reference(oi_ref)) = intents.iter().next() else {
            panic!("empty /OutputIntents");
        };
        let Object::Dictionary(oi) = doc.get(*oi_ref).unwrap() else {
            panic!("output intent not a dict");
        };
        assert_eq!(
            oi.get(&Name::from("S")),
            Some(&Object::Name(Name::from("GTS_PDFA1")))
        );
        let Some(Object::Reference(prof_ref)) = oi.get(&Name::from("DestOutputProfile")) else {
            panic!("no DestOutputProfile");
        };
        let Object::Stream(prof) = doc.get(*prof_ref).unwrap() else {
            panic!("profile not a stream");
        };
        assert_eq!(&prof.raw().as_ref()[36..40], b"acsp"); // valid ICC signature
    }

    #[test]
    fn custom_output_intent_is_threaded_through() {
        // Prove the chosen profile (identifier + /N) reaches the emitted OutputIntent. Uses the sRGB
        // bytes through the custom path so the test needs no extra asset — the point is the wiring,
        // not the colour space (a CMYK profile would slot in identically once bundled).
        use crate::output_intent::{SRGB_ICC, SRGB_ICC_N};
        let mut builder = Builder::new();
        builder.add_page(PageSpec::new(Vec::new()));
        let profile = OutputIntentProfile::new(SRGB_ICC.to_vec(), SRGB_ICC_N, "Custom-RGB");
        make_pdfa_with_output_intent(
            &mut builder,
            PdfAConformance::A2b,
            &XmpMetadata::default(),
            &profile,
        )
        .unwrap();
        let bytes = builder.build();

        let doc = Document::open(bytes).unwrap();
        let catalog = doc.catalog().unwrap();
        let Some(Object::Array(intents)) = catalog.get(&Name::from("OutputIntents")) else {
            panic!("no /OutputIntents");
        };
        let Some(Object::Reference(oi_ref)) = intents.iter().next() else {
            panic!("empty /OutputIntents");
        };
        let Object::Dictionary(oi) = doc.get(*oi_ref).unwrap() else {
            panic!("output intent not a dict");
        };
        let Some(Object::String(id)) = oi.get(&Name::from("OutputConditionIdentifier")) else {
            panic!("no /OutputConditionIdentifier");
        };
        assert_eq!(id.as_bytes(), b"Custom-RGB");
        let Some(Object::Reference(prof_ref)) = oi.get(&Name::from("DestOutputProfile")) else {
            panic!("no DestOutputProfile");
        };
        let Object::Stream(prof) = doc.get(*prof_ref).unwrap() else {
            panic!("profile not a stream");
        };
        assert_eq!(prof.dict().get(&Name::from("N")), Some(&Object::Integer(3)));
    }
}
