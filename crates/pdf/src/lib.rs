#![forbid(unsafe_code)]
#![warn(missing_docs)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]
//! pdf — the idiomatic Rust public API for Prism PDF (the facade crate).
//!
//! Re-exports the engine's layers behind one import surface so applications depend on `prismpdf`
//! alone. As of Milestone M1 the read path is wired end to end: open a PDF, count its pages, and
//! extract text (faithfully decoded via each font's `/ToUnicode` CMap where present).
//!
//! Choose one of four public journeys:
//! - parse and inspect with [`Document`] plus typed extraction functions;
//! - manipulate an opened document with report-returning transforms such as
//!   [`Document::save_with_report`] and [`merge_with_report`];
//! - author exact operators and resources with [`Content`], [`PageSpec`], and [`Builder`];
//! - measure and paginate a declarative tree with [`Composition`].
//!
//! Inner workspace crates are implementation layers. This facade and the C ABI are the supported
//! application contracts.

use std::collections::HashMap;

mod error;
/// The unified [`Error`] and [`Result`] for the whole facade.
pub use error::{Error, Result};

use pdf_content::{extract_fragments, extract_text_with_forms, layout, parse_content};
use pdf_cos::{Dictionary, Name, Object, ObjectId, Stream};
use pdf_fonts::{Encoding, ResourceDecoder, analyze_sfnt};
use pdf_graphics::{extract_image, resolve_image_color_space};
use std::collections::HashSet;

/// The core object model (COS, §7.3): [`cos::Object`] and its value types.
pub mod cos {
    pub use pdf_cos::{Array, Dictionary, Name, Object, ObjectId, PdfDate, PdfString, Stream};
}

/// The document model (§7.7): open a file, navigate its catalog and pages, and edit (merge via
/// [`merge`]; split/rotate via [`Document`] methods). [`Document::names`] walks a name tree
/// (§7.7.4) and [`Document::attachments`] reads embedded files (§7.11) as [`ExtractedAttachment`]s.
pub use pdf_document::{
    Annotation, DeveloperExtension, DocError, Document, DocumentPartInfo, EncryptedPayload,
    ExtractedAttachment, FormField, Limits, OpenDiagnostic, OpenMode, OpenReport, OutlineItem,
    RecoveryReason, RewriteMode, SignSettings, SignatureAppearance, SignatureEffect,
    SignatureStatus, StructureEffect, TransformReport, TsaCredentials, decode_text_string, merge,
    merge_with_report,
};

/// Precision authoring from scratch (§7.7, Milestone M6): draw an exact operator stream with
/// [`Content`], describe its fonts, images, and page box with [`PageSpec`], then add it to a
/// [`Builder`]. Prefer [`Composition`] or [`Flow`] when automatic measurement and pagination are
/// more useful than direct control of PDF page resources.
pub use pdf_content::Content;
pub use pdf_document::{
    AnnotationSpec, Attachment, AttrValue, Builder, DocumentPart, EncryptedPayloadSpec,
    FormFieldSpec, ImageColorSpace, LinkTarget, ListNumbering, MATHML_STRUCT_NS, PDF2_STRUCT_NS,
    PageLabelRange, PageLabelStyle, PageSpec, PrintFieldRole, RoleMapEntry, SeparationSpec,
    StdFont, StructAttr, StructElem, StructKid, ThScope,
};

/// High-level text layout (§9.4): measure/wrap text in Standard-14 fonts, draw an aligned block
/// ([`draw_text_block`]), or pour text across pages with automatic page breaks ([`Flow`]).
pub use pdf_layout::{
    Align, Color, Column, ComposeError, ComposeTable, ComposeTableRow, ComposedDocument,
    Composition, Container, Flow, GeometryEvent, GeometryTrace, HorizontalAlign, Image,
    ImageSizing, ListStyle, Page, PageStyle, Plan, Point, Rect, Row, Semantic, Size, Table,
    TextBlock, TextStyle, VerticalAlign, draw_text_block, measure_text, wrap_text,
};

/// Reader-layer types surfaced through the document API (§7.5).
pub use pdf_reader::Version;

/// PDF/A production (EPIC 13, §14): the conformance selector and XMP metadata fed to
/// [`make_pdfa`] / [`make_pdfua`]. The production passes themselves are facade wrappers (below)
/// so they return the unified [`Error`].
pub use pdf_standards::{OutputIntentProfile, PdfAConformance, PdfAError, PdfUaError, XmpMetadata};

/// Finalise `builder` as a conformant PDF/A file (§14, ISO 19005): writes XMP metadata, an sRGB
/// OutputIntent and a file `/ID`. Fonts must be embedded first (Standard-14 fonts are rejected).
///
/// Thin facade wrapper over [`pdf_standards::make_pdfa`] that surfaces the unified [`Error`]
/// (the [`Error::PdfA`] variant) instead of the layer's `PdfAError`.
///
/// # Errors
/// Returns [`PdfAError`] (as [`Error::PdfA`]) if the document is not ready for the requested
/// conformance — e.g. an unembedded font, or level A without logical structure.
pub fn make_pdfa(
    builder: &mut Builder,
    conformance: PdfAConformance,
    meta: &XmpMetadata,
) -> Result<()> {
    pdf_standards::make_pdfa(builder, conformance, meta)?;
    Ok(())
}

/// Finalise `builder` as a conformant PDF/A file with a caller-chosen [`OutputIntentProfile`]
/// (§14.11.5) instead of the default sRGB one — e.g. a CMYK printing condition so `DeviceCMYK`
/// content ([`Content::set_fill_cmyk`]) is conformant under PDF/A §6.2.4.3.
///
/// Thin facade wrapper over [`pdf_standards::make_pdfa_with_output_intent`] that surfaces the unified
/// [`Error`].
///
/// # Errors
/// Same as [`make_pdfa`]: an unembedded font, attachments without PDF/A-3, or level A without a
/// logical structure.
pub fn make_pdfa_with_output_intent(
    builder: &mut Builder,
    conformance: PdfAConformance,
    meta: &XmpMetadata,
    output_intent: &OutputIntentProfile,
) -> Result<()> {
    pdf_standards::make_pdfa_with_output_intent(builder, conformance, meta, output_intent)?;
    Ok(())
}

/// Finalise `builder` as an accessible **PDF/UA-1** file (ISO 14289-1) in the natural language
/// `lang`. The document must be tagged and carry a title; fonts must be embedded.
///
/// Thin facade wrapper over [`pdf_standards::make_pdfua`] that surfaces the unified [`Error`]
/// (the [`Error::PdfUa`] variant) instead of the layer's `PdfUaError`.
///
/// # Errors
/// Returns [`PdfUaError`] (as [`Error::PdfUa`]) if a PDF/UA requirement is unmet — e.g. the
/// document is untagged, has no title or language, a figure lacks alt text, or a font is
/// unembedded.
pub fn make_pdfua(builder: &mut Builder, meta: &XmpMetadata, lang: &str) -> Result<()> {
    pdf_standards::make_pdfua(builder, meta, lang)?;
    Ok(())
}

/// Finalise `builder` as an accessible **PDF/UA-2** file (ISO 14289-2:2024, on PDF 2.0) in the
/// natural language `lang`: XMP `pdfuaid:part` 2 + `rev`, the root `Document` element in the
/// PDF 2.0 structure namespace (auto-stamping `%PDF-2.0`), `/DisplayDocTitle`, `/Lang`.
///
/// Thin facade wrapper over [`pdf_standards::make_pdfua2`] surfacing the unified [`Error`].
///
/// # Errors
/// Returns [`PdfUaError`] (as [`Error::PdfUa`]) on the [`make_pdfua`] rejections, plus the
/// UA-2-specific ones: a `Note` element (use `FENote`, §8.2.5.14), a generic `H` heading
/// (§8.2.5.12), or an embedded file without a description (§8.14.1).
pub fn make_pdfua2(builder: &mut Builder, meta: &XmpMetadata, lang: &str) -> Result<()> {
    pdf_standards::make_pdfua2(builder, meta, lang)?;
    Ok(())
}

/// Encryption algorithm selector and access [`Permissions`] for the `save_encrypted*` methods (§7.6).
pub use pdf_crypto::{Algorithm, Permissions};

/// PAdES-LT revocation outcomes (§12.8.4.3): surfaced per signature by
/// [`Document::verify_signatures_ltv`], which checks each chain link against the OCSP responses
/// and CRLs embedded in the document's `/DSS`.
pub use pdf_crypto::{RevocationData, RevocationStatus, RevocationSummary};

/// Content-stream parsing and text extraction (§7.8 / §9.4).
pub use pdf_content::{Operation, extract_text, parse_content as parse_content_stream};

/// Image extraction (§8.9): an image XObject and its color space (§8.6).
pub use pdf_graphics::{
    ColorSpace, ExtractedImage, ImageData, ImageInfo, IndexedColorSpace, Separation,
};

/// PDF functions (§7.10): the numeric maps behind tint transforms, shadings, and transfer curves.
pub use pdf_graphics::Function;

/// Parse a PDF function (§7.10) from `obj`, resolving any indirect references through `doc`.
///
/// `obj` is the function dictionary/stream (or a reference to one), e.g. a `/TintTransform` of a
/// `Separation` colour space or a shading's `/Function`. Returns `None` if it is not a valid
/// function. Evaluate the result with [`Function::eval`].
#[must_use]
pub fn parse_function(doc: &Document, obj: &Object) -> Option<Function> {
    pdf_graphics::parse_function(obj, &|o| doc.resolve(o).ok())
}

/// Font program types surfaced in [`FontReport`] (§9.8/§9.9).
pub use pdf_fonts::{FaceMetrics as FontFaceMetrics, FontProgramFormat};

/// Font subsetting (§9.9): shrink a TrueType/CFF program to the glyphs needed.
pub use pdf_fonts::{glyphs_for_text, subset_sfnt};

/// Shaping a string against an sfnt program (§9.7): one [`Glyph`] (glyph id + advance) per char,
/// the basis for `Identity-H` embedding and for building Type0 test fixtures.
pub use pdf_fonts::{Glyph, shape_text};

/// What is known about one font used in a document (§9.6/§9.8/§9.9).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct FontReport {
    /// The font's `/BaseFont` name (often with a subset tag like `ABCDEF+`).
    pub base_font: String,
    /// The font `/Subtype` (`Type1`, `TrueType`, `Type0`, …).
    pub subtype: String,
    /// The embedded font program, if the font is embedded (§9.9).
    pub embedded: Option<EmbeddedFont>,
}

/// An embedded font program and (for sfnt programs) its parsed metrics.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct EmbeddedFont {
    /// The program format (§9.9).
    pub format: FontProgramFormat,
    /// The decoded program bytes.
    pub program: Vec<u8>,
    /// Parsed metrics for TrueType/OpenType programs; `None` for Type1/CFF or unparseable data.
    pub metrics: Option<FontFaceMetrics>,
}

/// Report on every distinct font used across the document's pages (§9.6/§9.8/§9.9): its base name,
/// subtype, and embedded program (with parsed metrics for TrueType/OpenType).
pub fn document_fonts(doc: &Document) -> Result<Vec<FontReport>> {
    let mut seen = HashSet::new();
    let mut reports = Vec::new();
    for page in doc.pages()? {
        for (_, font_ref) in font_dict(doc, &page)?.iter() {
            // Dedupe by font object number so a font shared across pages is reported once.
            if let Object::Reference(id) = font_ref
                && !seen.insert(id.number)
            {
                continue;
            }
            if let Object::Dictionary(font) = doc.resolve(font_ref)? {
                reports.push(font_report(doc, &font)?);
            }
        }
    }
    Ok(reports)
}

/// Build a [`FontReport`] for one resolved font dictionary.
fn font_report(doc: &Document, font: &Dictionary) -> Result<FontReport, DocError> {
    let subtype = name_string(font, "Subtype");
    let base_font = name_string(font, "BaseFont");
    let embedded = match font_descriptor(doc, font)? {
        Some(descriptor) => embedded_font(doc, &descriptor)?,
        None => None,
    };
    Ok(FontReport {
        base_font,
        subtype,
        embedded,
    })
}

/// Resolve a font's `/FontDescriptor` (§9.8). For Type0 fonts it lives on the descendant CIDFont.
fn font_descriptor(doc: &Document, font: &Dictionary) -> Result<Option<Dictionary>, DocError> {
    if let Some(fd) = font.get(&Name::from("FontDescriptor")) {
        return match doc.resolve(fd)? {
            Object::Dictionary(d) => Ok(Some(d)),
            _ => Ok(None),
        };
    }
    // Type0 → /DescendantFonts [<CIDFont>] → its /FontDescriptor.
    if let Some(descendants) = font.get(&Name::from("DescendantFonts"))
        && let Object::Array(array) = doc.resolve(descendants)?
        && let Some(first) = array.first()
        && let Object::Dictionary(cid_font) = doc.resolve(first)?
    {
        return font_descriptor(doc, &cid_font);
    }
    Ok(None)
}

/// Extract the embedded font program from a descriptor (§9.9), decoding the stream and parsing
/// sfnt metrics where applicable.
fn embedded_font(
    doc: &Document,
    descriptor: &Dictionary,
) -> Result<Option<EmbeddedFont>, DocError> {
    for format in [
        FontProgramFormat::Type1,
        FontProgramFormat::TrueType,
        FontProgramFormat::Cff, // /FontFile3 — refined by /Subtype below
    ] {
        let Some(entry) = descriptor.get(&Name::from(format.descriptor_key())) else {
            continue;
        };
        let Object::Stream(stream) = doc.resolve(entry)? else {
            continue;
        };
        let format = if format == FontProgramFormat::Cff {
            let subtype = stream
                .dict()
                .get_name(&Name::from("Subtype"))
                .map(Name::as_bytes);
            FontProgramFormat::from_fontfile3_subtype(subtype)
        } else {
            format
        };
        let program = doc.decode_stream(&stream)?;
        let metrics = if format.is_sfnt() {
            analyze_sfnt(&program)
        } else {
            None
        };
        return Ok(Some(EmbeddedFont {
            format,
            program,
            metrics,
        }));
    }
    Ok(None)
}

/// Subset every embedded **simple TrueType** font to only the glyphs the document actually uses,
/// returning a smaller PDF (§9.9). Fonts whose glyph mapping is uncertain — Type0/CFF/Type1 — are
/// left unchanged, so the result always renders the same.
pub fn subset_fonts(doc: &Document) -> Result<Vec<u8>> {
    // Which byte codes each font object actually shows, across every page (and nested forms).
    let mut usage: HashMap<u32, HashSet<u8>> = HashMap::new();
    for page in doc.pages()? {
        let resources = page_resources(doc, &page)?;
        let content = doc.page_content_bytes(&page)?;
        collect_font_usage(doc, &content, &resources, &mut usage, 0)?;
    }

    let mut overrides: HashMap<u32, Object> = HashMap::new();
    for (&font_number, codes) in &usage {
        if let Some((program_number, subset)) = subset_one_font(doc, font_number, codes)? {
            overrides.insert(program_number, Object::Stream(subset));
        }
    }
    Ok(doc.save_with_overrides(&overrides)?)
}

/// Subset embedded fonts and report the full-rewrite preservation effects (§9.6.4).
pub fn subset_fonts_with_report(doc: &Document) -> Result<TransformReport> {
    Ok(TransformReport::preserving_full_rewrite(subset_fonts(doc)?))
}

/// Walk a content stream (recursing into form XObjects) recording, per font object number, the
/// byte codes it shows.
fn collect_font_usage(
    doc: &Document,
    content: &[u8],
    resources: &Dictionary,
    usage: &mut HashMap<u32, HashSet<u8>>,
    depth: usize,
) -> Result<(), DocError> {
    if depth > MAX_FORM_DEPTH {
        return Ok(());
    }
    let fonts = font_name_to_number(doc, resources)?;
    let mut current: Option<u32> = None;
    for op in parse_content_stream(content) {
        match op.operator.as_str() {
            "Tf" => {
                if let Some(Object::Name(name)) = op.operands.first() {
                    current = fonts
                        .get(&String::from_utf8_lossy(name.as_bytes()).into_owned())
                        .copied();
                }
            }
            "Tj" | "'" | "\"" => {
                if let (Some(id), Some(Object::String(s))) = (current, op.operands.last()) {
                    usage
                        .entry(id)
                        .or_default()
                        .extend(s.as_bytes().iter().copied());
                }
            }
            "TJ" => {
                if let (Some(id), Some(Object::Array(array))) = (current, op.operands.last()) {
                    for element in array.iter() {
                        if let Object::String(s) = element {
                            usage
                                .entry(id)
                                .or_default()
                                .extend(s.as_bytes().iter().copied());
                        }
                    }
                }
            }
            "Do" => {
                if let Some(Object::Name(name)) = op.operands.first() {
                    let name = String::from_utf8_lossy(name.as_bytes());
                    if let Some(form) = lookup_xobject(doc, resources, &name, b"Form")? {
                        let inner = doc.decode_stream(&form)?;
                        let inner_resources = form_resources(doc, &form, resources)?;
                        collect_font_usage(doc, &inner, &inner_resources, usage, depth + 1)?;
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// Resource font name → font object number, for a resources dictionary.
fn font_name_to_number(
    doc: &Document,
    resources: &Dictionary,
) -> Result<HashMap<String, u32>, DocError> {
    let mut map = HashMap::new();
    for (name, font_ref) in subdict(doc, resources, "Font")?.iter() {
        if let Object::Reference(id) = font_ref {
            map.insert(
                String::from_utf8_lossy(name.as_bytes()).into_owned(),
                id.number,
            );
        }
    }
    Ok(map)
}

/// Subset one font's embedded program if it is a simple TrueType with an sfnt program — a
/// `/FontFile2` (TrueType) or a `/FontFile3 /Subtype /OpenType` (§9.9, Table 127; the OpenType
/// wrapper may carry CFF outlines) — and the subset is smaller. Returns
/// `(font-program object number, new uncompressed stream)`.
fn subset_one_font(
    doc: &Document,
    font_number: u32,
    codes: &HashSet<u8>,
) -> Result<Option<(u32, Stream)>, DocError> {
    let Object::Dictionary(font) = doc.get(ObjectId::new(font_number, 0))? else {
        return Ok(None);
    };
    if font.get_name(&Name::from("Subtype")).map(Name::as_bytes) != Some(b"TrueType") {
        return Ok(None); // only simple TrueType is mapped safely here
    }
    let Some(descriptor) = font_descriptor(doc, &font)? else {
        return Ok(None);
    };
    let (program_ref, is_fontfile3) = match descriptor.get(&Name::from("FontFile2")) {
        Some(program_ref) => (program_ref, false),
        None => match descriptor.get(&Name::from("FontFile3")) {
            Some(program_ref) => (program_ref, true),
            None => return Ok(None),
        },
    };
    let Some(program_number) = program_ref.as_reference().map(|id| id.number) else {
        return Ok(None);
    };
    let Object::Stream(program_stream) = doc.resolve(program_ref)? else {
        return Ok(None);
    };
    if is_fontfile3
        && program_stream
            .dict()
            .get_name(&Name::from("Subtype"))
            .map(Name::as_bytes)
            != Some(b"OpenType")
    {
        // A bare CFF (/Type1C) has no cmap to map characters through — leave it embedded as-is.
        return Ok(None);
    }
    let program = doc.decode_stream(&program_stream)?;

    // Map used byte codes → characters (via the PDF /Encoding) → glyph ids (via the font's cmap).
    let encoding = Encoding::from_font_dict(&font);
    let text: String = codes
        .iter()
        .flat_map(|&code| encoding.decode(&[code]).chars().collect::<Vec<_>>())
        .collect();
    let Some(glyphs) = glyphs_for_text(&program, &text) else {
        return Ok(None);
    };
    let Some(subset) = subset_sfnt(&program, &glyphs) else {
        return Ok(None);
    };
    if subset.len() >= program.len() {
        return Ok(None); // no benefit
    }

    // Re-embed uncompressed; the writer fills in /Length. A FontFile3 keeps its required
    // /Subtype /OpenType (Table 127); a FontFile2 carries /Length1 instead (Table 125).
    let mut dict = Dictionary::new();
    if is_fontfile3 {
        dict.insert(Name::from("Subtype"), Object::Name(Name::from("OpenType")));
    } else {
        dict.insert(Name::from("Length1"), Object::Integer(subset.len() as i64));
    }
    Ok(Some((program_number, Stream::new(dict, subset))))
}

/// Read a `/Key` name from a dictionary as a string (empty if absent or not a name).
fn name_string(dict: &Dictionary, key: &str) -> String {
    match dict.get(&Name::from(key)) {
        Some(Object::Name(n)) => String::from_utf8_lossy(n.as_bytes()).into_owned(),
        _ => String::new(),
    }
}

/// Maximum form-XObject nesting traversed when extracting content (anti-DoS, DESIGN.md §3.4).
const MAX_FORM_DEPTH: usize = 16;

/// Extract every image XObject a page can draw (§8.9), including those nested inside form XObjects
/// (§8.10). Returns an empty vector for a page with no images or an out-of-range `index`.
/// Transport filters and JPEG are decoded; JPEG 2000 is returned encoded (see [`ImageData`]).
pub fn page_images(doc: &Document, index: usize) -> Result<Vec<ExtractedImage>> {
    let pages = doc.pages()?;
    let Some(page) = pages.get(index) else {
        return Ok(Vec::new());
    };
    let resources = page_resources(doc, page)?;
    let mut images = Vec::new();
    let mut visited = HashSet::new();
    collect_images(doc, &resources, &mut visited, &mut images, 0)?;
    Ok(images)
}

/// Read a page's annotations (§12.5): links, notes, form widgets and other interactive overlays it
/// lists in `/Annots`, with their subtype, rectangle, text contents and any link URI. Returns an
/// empty vector for a page with no annotations or an out-of-range `index`.
pub fn page_annotations(doc: &Document, index: usize) -> Result<Vec<Annotation>> {
    let pages = doc.pages()?;
    match pages.get(index) {
        Some(page) => Ok(doc.annotations(page)?),
        None => Ok(Vec::new()),
    }
}

/// Collect images from a `/Resources /XObject` dictionary, recursing into form XObjects (§8.10).
/// `visited` (keyed by object number) dedupes shared XObjects and guards against cycles.
fn collect_images(
    doc: &Document,
    resources: &Dictionary,
    visited: &mut HashSet<u32>,
    out: &mut Vec<ExtractedImage>,
    depth: usize,
) -> Result<(), DocError> {
    if depth > MAX_FORM_DEPTH {
        return Ok(());
    }
    for (_, entry) in subdict(doc, resources, "XObject")?.iter() {
        if let Object::Reference(id) = entry
            && !visited.insert(id.number)
        {
            continue;
        }
        let Object::Stream(stream) = doc.resolve(entry)? else {
            continue;
        };
        match stream
            .dict()
            .get_name(&Name::from("Subtype"))
            .map(Name::as_bytes)
        {
            Some(b"Image") => {
                let color_space =
                    resolve_image_color_space(stream.dict(), &|object| doc.resolve(object))?;
                let globals = jbig2_globals(doc, stream.dict())?;
                out.push(extract_image(&stream, color_space, globals.as_deref()));
            }
            Some(b"Form") => {
                let form_resources = form_resources(doc, &stream, resources)?;
                collect_images(doc, &form_resources, visited, out, depth + 1)?;
            }
            _ => {}
        }
    }
    Ok(())
}

/// Resolve and decode an image's `/JBIG2Globals` stream (§7.4.7), if any — the shared JBIG2
/// segments needed to decode the image. `/JBIG2Globals` lives in the JBIG2 stage's `/DecodeParms`
/// (a lone dictionary, or the array entry aligned with the `JBIG2Decode` filter) and is an indirect
/// stream the filter layer cannot resolve, so the document layer does it here.
fn jbig2_globals(doc: &Document, dict: &Dictionary) -> Result<Option<Vec<u8>>, DocError> {
    let Some(parms) = dict.get(&Name::from("DecodeParms")) else {
        return Ok(None);
    };
    // The parms may be a single dictionary or an array (one entry per filter); scan for the one
    // carrying /JBIG2Globals either way.
    let globals_ref = match doc.resolve(parms)? {
        Object::Dictionary(d) => d.get(&Name::from("JBIG2Globals")).cloned(),
        Object::Array(arr) => arr.iter().find_map(|entry| match doc.resolve(entry) {
            Ok(Object::Dictionary(d)) => d.get(&Name::from("JBIG2Globals")).cloned(),
            _ => None,
        }),
        _ => None,
    };
    let Some(globals_ref) = globals_ref else {
        return Ok(None);
    };
    match doc.resolve(&globals_ref)? {
        // The globals stream may itself be transport-filtered (e.g. ASCIIHexDecode), so decode it.
        Object::Stream(stream) => Ok(doc.decode_stream(&stream).ok()),
        _ => Ok(None),
    }
}

/// Resolve `dict[key]` to a dictionary, or an empty one if absent or not a dictionary.
fn subdict(doc: &Document, dict: &Dictionary, key: &str) -> Result<Dictionary, DocError> {
    match dict.get(&Name::from(key)) {
        Some(object) => match doc.resolve(object)? {
            Object::Dictionary(d) => Ok(d),
            _ => Ok(Dictionary::new()),
        },
        None => Ok(Dictionary::new()),
    }
}

/// A page's resolved `/Resources` dictionary (§7.8.3), or empty.
fn page_resources(doc: &Document, page: &Dictionary) -> Result<Dictionary, DocError> {
    subdict(doc, page, "Resources")
}

/// A form XObject's own `/Resources`, inheriting the parent's when absent (§8.10.1).
fn form_resources(
    doc: &Document,
    form: &Stream,
    parent: &Dictionary,
) -> Result<Dictionary, DocError> {
    match form.dict().get(&Name::from("Resources")) {
        Some(object) => match doc.resolve(object)? {
            Object::Dictionary(d) => Ok(d),
            _ => Ok(parent.clone()),
        },
        None => Ok(parent.clone()),
    }
}

/// Look up a named XObject of the given `/Subtype` (`Form`/`Image`) in a resources dictionary.
fn lookup_xobject(
    doc: &Document,
    resources: &Dictionary,
    name: &str,
    subtype: &[u8],
) -> Result<Option<Stream>, DocError> {
    let Some(entry) = subdict(doc, resources, "XObject")?
        .get(&Name::from(name))
        .cloned()
    else {
        return Ok(None);
    };
    match doc.resolve(&entry)? {
        Object::Stream(stream)
            if stream
                .dict()
                .get_name(&Name::from("Subtype"))
                .map(Name::as_bytes)
                == Some(subtype) =>
        {
            Ok(Some(stream))
        }
        _ => Ok(None),
    }
}

/// Resolve an **Indexed** colour space (§8.6.6.3) into its base space, highest index, and palette
/// bytes. Returns `None` if `obj` is not a 4-element `[/Indexed base hival lookup]` array. The lookup
/// table is taken from a byte string directly, or decoded from a stream.
pub fn resolve_indexed(doc: &Document, obj: &Object) -> Result<Option<IndexedColorSpace>> {
    Ok(pdf_graphics::resolve_indexed(
        obj,
        &|object| doc.resolve(object),
        &|stream| doc.decode_stream(stream),
    )?)
}

/// Resolve a **Separation** or **DeviceN** color space (§8.6.6) into a [`Separation`] carrying its
/// colorant names, alternate device space, and tint-transform [`Function`] — so a caller can turn
/// tint values into the alternate space's components via [`Separation::to_alternate`]. Returns `None`
/// if `obj` is not a Separation/DeviceN array or its tint transform is missing/invalid.
pub fn resolve_separation(doc: &Document, obj: &Object) -> Result<Option<Separation>> {
    Ok(pdf_graphics::resolve_separation(obj, &|object| {
        doc.resolve(object)
    })?)
}

/// Extract reading-order text from a single page (§7.8.2 + §9.4), or `None` if `index` is out of
/// range. Spans the document layer (locating and decoding the page's content streams and fonts)
/// and the content layer (parsing operators and showing text). Shown bytes are decoded through
/// each font's `/ToUnicode` CMap (§9.10) when present, falling back to Latin-1.
pub fn page_text(doc: &Document, index: usize) -> Result<Option<String>> {
    let pages = doc.pages()?;
    let Some(page) = pages.get(index) else {
        return Ok(None);
    };
    Ok(Some(extract_page_text(doc, page)?))
}

/// Extract a page's text in **geometric reading order** (§8.3/§9.4), or `None` if `index` is out
/// of range. Unlike [`page_text`] (which follows emission order and recurses into form XObjects),
/// this runs the graphics-state machine to position each shown string and orders them
/// top-to-bottom, left-to-right — better for documents that emit text out of order. Form XObjects
/// are not followed here.
pub fn page_text_positioned(doc: &Document, index: usize) -> Result<Option<String>> {
    let pages = doc.pages()?;
    let Some(page) = pages.get(index) else {
        return Ok(None);
    };
    let resources = page_resources(doc, page)?;
    let decoder = resource_decoder(doc, &resources)?;
    let content = doc.page_content_bytes(page)?;
    let fragments = extract_fragments(&parse_content(&content), &decoder);
    Ok(Some(layout(&fragments)))
}

/// Extract text from every page, joined by form feeds (`\f`, the conventional page separator).
pub fn document_text(doc: &Document) -> Result<String> {
    let pages = doc.pages()?;
    let mut chunks = Vec::with_capacity(pages.len());
    for page in &pages {
        chunks.push(extract_page_text(doc, page)?);
    }
    Ok(chunks.join("\u{000C}"))
}

/// Decode and extract one page's text, recursing into the form XObjects it invokes (§8.10) and
/// appending the text its annotations carry (notes, free-text, markup `/Contents`, §12.5.2) — text
/// that lives outside the content stream and would otherwise be lost.
fn extract_page_text(doc: &Document, page: &Dictionary) -> Result<String, DocError> {
    let resources = page_resources(doc, page)?;
    let content = doc.page_content_bytes(page)?;
    let mut text = extract_content_text(doc, &content, &resources, 0)?;
    for annotation in doc.annotations(page)? {
        if let Some(contents) = annotation.contents.filter(|c| !c.is_empty()) {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(&contents);
        }
    }
    Ok(text)
}

/// Extract text from a content stream under `resources`, inlining the text of any form XObject it
/// invokes via `Do` (§8.10). Depth-bounded against pathological nesting.
fn extract_content_text(
    doc: &Document,
    content: &[u8],
    resources: &Dictionary,
    depth: usize,
) -> Result<String, DocError> {
    if depth > MAX_FORM_DEPTH {
        return Ok(String::new());
    }
    let decoder = resource_decoder(doc, resources)?;
    let operations = parse_content(content);
    let text = extract_text_with_forms(&operations, &decoder, &|name| {
        // Best-effort: a form that fails to resolve/decode contributes no text.
        form_text(doc, resources, name, depth).ok().flatten()
    });
    Ok(text)
}

/// The text of the form XObject `name` (§8.10), resolved within `resources` and extracted with the
/// form's own resources (inheriting the parent's when absent).
fn form_text(
    doc: &Document,
    resources: &Dictionary,
    name: &str,
    depth: usize,
) -> Result<Option<String>, DocError> {
    let Some(form) = lookup_xobject(doc, resources, name, b"Form")? else {
        return Ok(None);
    };
    let content = doc.decode_stream(&form)?;
    let form_resources = form_resources(doc, &form, resources)?;
    Ok(Some(extract_content_text(
        doc,
        &content,
        &form_resources,
        depth + 1,
    )?))
}

fn resource_decoder(doc: &Document, resources: &Dictionary) -> Result<ResourceDecoder, DocError> {
    ResourceDecoder::from_resources(resources, &|object| doc.resolve(object), &|stream| {
        doc.decode_stream(stream)
    })
}

/// Resolve a page's `/Resources /Font` dictionary, or an empty one if it has none.
fn font_dict(doc: &Document, page: &Dictionary) -> Result<Dictionary, DocError> {
    subdict(doc, &page_resources(doc, page)?, "Font")
}
