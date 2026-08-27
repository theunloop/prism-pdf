//! Authoring a PDF from scratch (ISO 32000-1 §7.7) — the Milestone M6 on-ramp.
//!
//! [`Builder`] assembles a fresh document object-graph — catalog (§7.7.2), page tree (§7.7.3), and
//! one page per [`Builder::add_page`] with a content stream and Standard-14 font resources — and
//! serialises it with the writer. It is the structural complement to `pdf_content`'s operator
//! builders: the caller draws a page's operators elsewhere and hands the bytes here. Embedded
//! fonts, images and richer layout build on top of this later.

use pdf_cos::{Array, Dictionary, Name, Object, ObjectId, PdfString, Stream};
use pdf_filters::flate_encode;
use pdf_writer::write_document;

/// US Letter, the default page size, in PDF points (§8.3.2.3): 612 × 792.
const US_LETTER: [f64; 4] = [0.0, 0.0, 612.0, 792.0];

/// One of the 14 standard fonts (§9.6.2.2) — always available, never embedded.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StdFont {
    Helvetica,
    HelveticaBold,
    HelveticaOblique,
    HelveticaBoldOblique,
    TimesRoman,
    TimesBold,
    TimesItalic,
    TimesBoldItalic,
    Courier,
    CourierBold,
    CourierOblique,
    CourierBoldOblique,
    Symbol,
    ZapfDingbats,
}

impl StdFont {
    /// The `/BaseFont` name written into the font dictionary.
    #[must_use]
    pub fn base_name(self) -> &'static str {
        match self {
            StdFont::Helvetica => "Helvetica",
            StdFont::HelveticaBold => "Helvetica-Bold",
            StdFont::HelveticaOblique => "Helvetica-Oblique",
            StdFont::HelveticaBoldOblique => "Helvetica-BoldOblique",
            StdFont::TimesRoman => "Times-Roman",
            StdFont::TimesBold => "Times-Bold",
            StdFont::TimesItalic => "Times-Italic",
            StdFont::TimesBoldItalic => "Times-BoldItalic",
            StdFont::Courier => "Courier",
            StdFont::CourierBold => "Courier-Bold",
            StdFont::CourierOblique => "Courier-Oblique",
            StdFont::CourierBoldOblique => "Courier-BoldOblique",
            StdFont::Symbol => "Symbol",
            StdFont::ZapfDingbats => "ZapfDingbats",
        }
    }

    /// Whether to tag the font with `/WinAnsiEncoding`; Symbol and ZapfDingbats use their built-in
    /// encodings and must not be re-encoded (§9.6.6.1).
    fn uses_win_ansi(self) -> bool {
        !matches!(self, StdFont::Symbol | StdFont::ZapfDingbats)
    }
}

/// The colour space of an authored image (§8.6.3): selects `/ColorSpace` and the component count.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ImageColorSpace {
    /// `DeviceGray` — 1 component.
    Gray,
    /// `DeviceRGB` — 3 components.
    Rgb,
    /// `DeviceCMYK` — 4 components.
    Cmyk,
}

impl ImageColorSpace {
    fn name(self) -> &'static str {
        match self {
            ImageColorSpace::Gray => "DeviceGray",
            ImageColorSpace::Rgb => "DeviceRGB",
            ImageColorSpace::Cmyk => "DeviceCMYK",
        }
    }
}

/// The encoding of an authored image's sample data (§7.4): a `/Filter`, or none for raw samples.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ImageFilter {
    /// `DCTDecode` — the data is a complete JPEG.
    Dct,
    /// `FlateDecode` — the data is zlib-compressed samples.
    Flate,
}

impl ImageFilter {
    fn name(self) -> &'static str {
        match self {
            ImageFilter::Dct => "DCTDecode",
            ImageFilter::Flate => "FlateDecode",
        }
    }
}

/// An image XObject to embed (§8.9.5): its geometry, colour space, sample encoding and bytes,
/// optionally with a soft mask (alpha) or stencil mask.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ImageXObject {
    /// Width in samples.
    pub width: u32,
    /// Height in samples.
    pub height: u32,
    /// Colour space of the samples. Ignored when `image_mask` is set.
    pub color_space: ImageColorSpace,
    /// Bits per component (usually 8; 1 for a stencil mask).
    pub bits_per_component: u8,
    /// The `/Filter` for `data`, or `None` for raw samples.
    pub filter: Option<ImageFilter>,
    /// The (already encoded, per `filter`) image bytes.
    pub data: Vec<u8>,
    /// An optional soft-mask image (§11.6.5.2): a `DeviceGray` image giving per-pixel alpha, emitted
    /// as a separate image XObject and referenced by `/SMask`.
    pub smask: Option<Box<ImageXObject>>,
    /// An optional stencil-mask image (§8.9.6.3): a 1-bit `/ImageMask` image emitted separately and
    /// referenced by `/Mask` (1 bits mark masked-out samples).
    pub mask: Option<Box<ImageXObject>>,
    /// Whether this image is *itself* a stencil mask (`/ImageMask true`, §8.9.6.2): 1-bit, no colour
    /// space. Set on the sub-image placed in another image's `mask`.
    pub image_mask: bool,
}

/// An embedded TrueType font to write as a composite (Type0/`Identity-H`, CIDFontType2) font
/// (§9.7): the whole program plus the descriptor metrics, per-glyph widths and a glyph→Unicode map
/// for the glyphs actually used. CID == glyph ID (CIDToGIDMap `Identity`). Built by the layout layer
/// — the fields are primitives so this crate needs no font-parsing dependency.
#[derive(Clone, Debug)]
pub struct CidFont {
    /// The full sfnt (TrueType/OpenType) program bytes.
    pub program: Vec<u8>,
    /// PostScript name for `/BaseFont` / `/FontName`.
    pub postscript_name: String,
    /// `/FontDescriptor` metrics, in 1000-em units.
    pub ascent: i32,
    pub descent: i32,
    pub cap_height: i32,
    pub bbox: [i32; 4],
    pub italic_angle: f64,
    pub flags: u32,
    /// Default glyph width (`/DW`).
    pub default_width: u16,
    /// `(glyph id, advance)` for the glyphs used, advances in 1000-em units.
    pub widths: Vec<(u16, u16)>,
    /// `(glyph id, character)` for the glyphs used, for the `/ToUnicode` CMap.
    pub to_unicode: Vec<(u16, char)>,
    /// `CIDToGIDMap` bytes (CID→GID, 2 bytes each, big-endian) when the `program` is subsetted and
    /// its glyphs renumbered; `None` keeps the codes equal to glyph IDs (`/CIDToGIDMap /Identity`).
    pub cid_to_gid: Option<Vec<u8>>,
}

/// A top-level outline entry (bookmark, §12.3.3): a title and the 0-based page it jumps to.
struct OutlineItem {
    title: String,
    page_index: usize,
}

/// A configured PDF/A OutputIntent (§14.11.5): the ICC profile bytes, its colour-component count,
/// and the output-condition identifier.
struct OutputIntentSpec {
    icc: Vec<u8>,
    n: u32,
    identifier: String,
}

/// A file to embed as an attachment (§7.11.4) and associate with the document (§14.13). Required
/// for PDF/A-3 use cases like e-invoicing (FatturaPA/ZUGFeRD: the invoice XML rides inside the PDF).
#[derive(Clone, Debug)]
pub struct Attachment {
    /// File name, e.g. `"invoice.xml"`.
    pub name: String,
    /// MIME type for the embedded file's `/Subtype`, e.g. `"text/xml"` (the `/` is name-escaped).
    pub mime: String,
    /// `/AFRelationship` (§14.13): how the file relates to the document — `"Data"`, `"Source"`,
    /// `"Alternative"`, `"Supplement"`, or `"Unspecified"`.
    pub relationship: String,
    /// Optional human-readable `/Desc`.
    pub description: Option<String>,
    /// Optional modification date as a PDF date string (`"D:YYYYMMDDHHmmSS"`), for `/Params /ModDate`.
    pub mod_date: Option<String>,
    /// The file's bytes.
    pub data: Vec<u8>,
}

/// The destination of a link annotation (§12.5.6.5): where activating the link takes the reader.
/// Both forms use only PDF/A-permitted actions (§6.5.1 of ISO 19005: `URI` and `GoTo`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkTarget {
    /// An external URI — a `/URI` action (§12.6.4.7).
    Uri(String),
    /// Another page in this document (0-based index), displayed `/Fit` to the window — a `/GoTo`
    /// action with an explicit destination (§12.3.2). **PDF/UA-2 forbids this form** for
    /// intra-document links (§8.8) — use [`LinkTarget::Element`] there.
    Page(usize),
    /// A structure element in this document, by its `/ID` (see [`StructElem::id`]) — a `/GoTo`
    /// action carrying a **structure destination** (`/SD`, §12.3.2.3 — **PDF 2.0**), which
    /// PDF/UA-2 §8.8 requires for intra-document destinations. A `/D` page fallback (the
    /// element's page) is kept alongside for pre-2.0 readers. An ID that matches no element
    /// leaves the fallback pointing at the link's own page. Promotes the document to PDF 2.0.
    Element(String),
    /// A document part declared via [`Builder::document_parts`], by 0-based index — a `/GoToDp`
    /// action (§12.6.4.5, **PDF 2.0**) jumping to that part's `/Start` page. The index is clamped
    /// to the declared parts; with **no** parts declared the link is emitted without an action
    /// (a dangling `/Dp` would be invalid). Promotes the document to PDF 2.0.
    DocumentPart(usize),
}

/// The encrypted payload of an unencrypted wrapper document (§7.6.7): a PDF encrypted with a
/// *custom* (non-standard) security handler, carried as the wrapper's single embedded file so a
/// processor with the right cryptographic filter can open it while others see the wrapper's
/// instructions. Declare it with [`Builder::encrypted_payload`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EncryptedPayloadSpec {
    /// The payload's file name (the filespec `/F`/`/UF` and the collection's initial document).
    pub file_name: String,
    /// A human-readable `/Desc` — typically "encrypted with X, install Y to open" guidance.
    pub description: Option<String>,
    /// The still-encrypted payload PDF bytes, embedded verbatim.
    pub data: Vec<u8>,
    /// `/EP /Subtype` (Table 28, required): the name of the cryptographic filter that decrypts
    /// the payload.
    pub filter_subtype: String,
    /// `/EP /Version` (optional): that filter's version, emitted as a `/M.m` name.
    pub version: Option<(u8, u8)>,
}

/// An annotation to author on a page (§12.5), built PDF/A-clean: the Print flag is set and the
/// Hidden/Invisible/NoView flags cleared (§6.3.2), and the subtypes that require a normal appearance
/// stream get one (§6.3.3). Add it with [`Builder::add_annotation`].
#[derive(Clone, Debug, PartialEq)]
pub enum AnnotationSpec {
    /// A hyperlink over `rect` going to `target` (§12.5.6.5). Link annotations are exempt from the
    /// appearance-stream requirement (§6.3.3), so none is generated; the border is suppressed.
    Link {
        /// The clickable rectangle `[llx lly urx ury]` in default user space (`/Rect`).
        rect: [f64; 4],
        /// Where the link goes.
        target: LinkTarget,
        /// Optional alternate description (`/Contents`, §12.5.2) — read by assistive technology;
        /// PDF/UA (14289-1 §7.18.5, 14289-2 §8.9.2) wants links described.
        contents: Option<String>,
    },
    /// A text note (§12.5.6.4) anchored at `rect` carrying `contents` as its body (`/Contents`,
    /// §12.5.2). A normal appearance stream (a small note marker, a Form XObject) is generated, as
    /// PDF/A requires for non-link annotations (§6.3.3).
    Note {
        /// The note-icon rectangle `[llx lly urx ury]` in default user space (`/Rect`).
        rect: [f64; 4],
        /// The note body / accessibility text.
        contents: String,
    },
}

/// An interactive form field to author (§12.7.4), placed on a page as a widget annotation and listed
/// in the document's `/AcroForm`. Built PDF/A-clean: no `/A`/`/AA` actions (§6.4.1), a normal
/// appearance (§6.3.3), and the form carries no `/NeedAppearances`/`/XFA` (§6.4.1/§6.4.2). Add it
/// with [`Builder::add_form_field`].
#[derive(Clone, Debug, PartialEq)]
pub enum FormFieldSpec {
    /// A checkbox button (`/FT /Btn`, §12.7.4.2.3): on or off, with a vector `/On`+`/Off` appearance
    /// subdictionary (so it needs no font — PDF/A-safe). The on-state is named `On`.
    Checkbox {
        /// The widget rectangle `[llx lly urx ury]` in default user space (`/Rect`).
        rect: [f64; 4],
        /// The fully-qualified field name (`/T`, §12.7.3.2).
        name: String,
        /// Whether it starts checked (sets `/V` and `/AS` to `On`, else `Off`).
        checked: bool,
        /// Optional alternate field name (`/TU`, §12.7.3.1) — shown in place of `/T` by viewers
        /// and read by assistive technology; PDF/UA-1 (§7.18.4 context) recommends one.
        tooltip: Option<String>,
    },
}

/// Options for one page added with [`Builder::add_page`].
///
/// Start with [`PageSpec::new`] and add only the resources the content stream references. This one
/// extensible value replaces separate page methods for images, embedded fonts, and custom sizes.
#[derive(Clone, Debug, Default)]
pub struct PageSpec {
    /// The page's content-stream bytes (§7.8.2).
    content: Vec<u8>,
    /// Named Standard-14 font resources (§9.6.2.2).
    fonts: Vec<(String, StdFont)>,
    /// Names registered with [`Builder::embed_cid_font`] and referenced by this page.
    embedded: Vec<String>,
    /// Named image XObjects (§8.9).
    images: Vec<(String, ImageXObject)>,
    /// A page-specific media box; `None` uses the builder default (§14.11.2).
    media_box: Option<[f64; 4]>,
}

impl PageSpec {
    /// Create a page with `content` and no resources, using the builder's default media box.
    #[must_use]
    pub fn new(content: impl Into<Vec<u8>>) -> Self {
        Self {
            content: content.into(),
            ..Self::default()
        }
    }

    /// Add a named Standard-14 font resource (§9.6.2.2).
    #[must_use]
    pub fn standard_font(mut self, name: impl Into<String>, font: StdFont) -> Self {
        self.fonts.push((name.into(), font));
        self
    }

    /// Reference a composite font previously registered with [`Builder::embed_cid_font`].
    #[must_use]
    pub fn embedded_font(mut self, name: impl Into<String>) -> Self {
        self.embedded.push(name.into());
        self
    }

    /// Add a named image XObject resource (§8.9).
    #[must_use]
    pub fn image(mut self, name: impl Into<String>, image: ImageXObject) -> Self {
        self.images.push((name.into(), image));
        self
    }

    /// Override the builder's media box for this page (§14.11.2).
    #[must_use]
    pub fn media_box(mut self, media_box: [f64; 4]) -> Self {
        self.media_box = Some(media_box);
        self
    }
}

/// One page accumulated internally by the [`Builder`].
struct PageState {
    content: Vec<u8>,
    fonts: Vec<(String, StdFont)>,
    embedded: Vec<String>,
    images: Vec<(String, ImageXObject)>,
    color_spaces: Vec<(String, ColorSpaceKind)>,
    /// Reusable content Form XObjects (§8.10) authored onto this page.
    forms: Vec<FormXObjectSpec>,
    media_box: Option<[f64; 4]>,
}

/// A reusable content Form XObject (§8.10) authored onto a page via
/// [`Builder::add_form_xobject`], painted any number of times with `Do`.
struct FormXObjectSpec {
    /// The `/XObject` resource name `Do` references.
    name: String,
    /// The form's bounding box `[llx lly urx ury]` (`/BBox`).
    bbox: [f64; 4],
    /// The form's own content-stream bytes.
    content: Vec<u8>,
    /// Associated files for this form (`/AF`, §14.13.7 — **PDF 2.0**); usually empty.
    files: Vec<Attachment>,
}

/// A named colour-space resource authored into a page's `/Resources /ColorSpace` (§8.6).
enum ColorSpaceKind {
    /// A Separation (spot) colour space (§8.6.6).
    Separation(SeparationSpec),
    /// An ICCBased colour space (§8.6.5.5): the ICC profile bytes and component count `n`.
    Icc { icc: Vec<u8>, n: u32 },
    /// An Indexed (palette) colour space (§8.6.6.3): a base device space and the palette bytes
    /// (`(hival + 1) × components(base)` of them, laid out per entry).
    Indexed {
        base: ImageColorSpace,
        palette: Vec<u8>,
    },
    /// A CIE L*a*b* colour space (§8.6.5.4): a white point `[Xw Yw Zw]` and an a*/b* `range`
    /// `[amin amax bmin bmax]`.
    Lab {
        white_point: [f64; 3],
        range: [f64; 4],
    },
}

/// A Separation (spot) colour space (§8.6.6): one named `colorant` whose tint in `[0, 1]` maps —
/// via a linear tint-transform function (§7.10, type 2) — into an `alternate` device space.
/// `full` is the alternate-space colour at tint 1; tint 0 maps to the alternate's white. Author it
/// onto a page with [`Builder::add_separation`] and paint with
/// `pdf_content::Content::set_fill_color_space` + `pdf_content::Content::set_fill_color`.
#[derive(Clone, Debug)]
pub struct SeparationSpec {
    /// The colourant name (e.g. `"PANTONE 185 C"` or `"All"`).
    pub colorant: String,
    /// The alternate device colour space the tint transform targets.
    pub alternate: ImageColorSpace,
    /// The alternate-space components at full tint (tint = 1). Length must match `alternate`
    /// (Gray = 1, RGB = 3, CMYK = 4).
    pub full: Vec<f64>,
}

/// The **PDF 2.0 standard structure namespace** URI (§14.8.6, ISO 32000-2): the namespace the 2.0
/// structure types (`Document` as UA-2 root, `FENote`, `Title`, `Ruby`, `Warichu`, `Index`,
/// `BibEntry`, `Code`, …) belong to. Assign it via [`StructElem::namespace`] /
/// [`Builder::structure_namespace`]; PDF/UA-2 (ISO 14289-2 §8.2.5.2) requires the root `Document`
/// element in this namespace.
pub const PDF2_STRUCT_NS: &str = "http://iso.org/pdf2/ssn";

/// The **MathML structure namespace** URI (§14.8.6, ISO 32000-2): presentation MathML structure
/// elements (`math`, `mrow`, `mi`, …) tagged inside a `Formula` element — one of the two ways
/// PDF/UA-2 §8.2.5.29 accepts formulas (the other is a MathML **associated file** with
/// `AFRelationship` `Supplement` on the `Formula` element, via [`StructElem::associate_file`]).
pub const MATHML_STRUCT_NS: &str = "http://www.w3.org/1998/Math/MathML";

/// A node in a Tagged-PDF logical structure tree (§14.7): a structure element with a type, optional
/// alternate text, and children that are either marked content on a page or nested elements. The
/// supplied elements become the children of an implicit `Document` root, in order.
#[derive(Clone, Debug, Default)]
pub struct StructElem {
    /// The structure type (`/S`), e.g. `"P"`, `"H1"`, `"L"`, `"Table"`, `"Figure"` (§14.8.4).
    /// Standard types need no `/RoleMap`.
    pub tag: String,
    /// Optional alternate text (`/Alt`, §14.8.5) — required for figures under PDF/UA.
    pub alt: Option<String>,
    /// Optional replacement text (`/ActualText`, §14.9.4): the exact text the element's content
    /// stands for (e.g. the linear form of a formula). PDF/UA-2 §8.2.5.28 accepts it as the
    /// alternative to `/Alt` on a `Figure`.
    pub actual_text: Option<String>,
    /// Optional natural language of this element's content (`/Lang`, §14.9.2), overriding the
    /// document `/Lang` — PDF/UA wants language *changes* declared on the element where they
    /// happen (14289-1 §7.2 / 14289-2 §8.2.7).
    pub lang: Option<String>,
    /// Optional structure **namespace** URI (`/NS`, §14.7.4 — **PDF 2.0**), e.g.
    /// `"http://iso.org/pdf2/ssn"` (the standard structure namespace) or the PDF/UA-2 namespace.
    /// When set, the element references a `/Namespace` dictionary listed in the
    /// `/StructTreeRoot /Namespaces` array; any element with a namespace makes the document PDF 2.0.
    pub ns: Option<String>,
    /// Associated files for this element (`/AF`, §14.13.6 — **PDF 2.0**): the structure-element is
    /// the *preferred* placement for an associated file in 2.0. Each is embedded, listed in the
    /// `/EmbeddedFiles` name tree, and referenced from this element's `/AF`; any entry makes the
    /// document PDF 2.0.
    pub af: Vec<Attachment>,
    /// Optional element identifier (`/ID`, §14.7.4.2): a byte string unique in the document, listed
    /// in the `/StructTreeRoot /IDTree` name tree. PDF/UA-1 §7.9 requires one on every `Note`.
    pub id: Option<String>,
    /// `/Ref` targets (§14.7.4.2, **PDF 2.0**): the `/ID`s of the elements this element refers to,
    /// resolved to indirect references at build time (unknown IDs are skipped). PDF/UA-2 §8.2.5.14
    /// links a `FENote` and its citing content **bidirectionally** this way. Any entry makes the
    /// document PDF 2.0.
    pub refs: Vec<String>,
    /// Structure attributes (`/A`, §14.7.6), one dictionary per owner (`/O`) — e.g. `/Scope` under
    /// the `Table` owner, `/ListNumbering` under `List`, PrintField `/Role` under `PrintField`.
    pub attrs: Vec<StructAttr>,
    /// The element's children, in reading order: marked content and/or nested elements.
    pub kids: Vec<StructKid>,
}

/// A child of a [`StructElem`]: either a marked-content sequence on a page (a leaf, §14.7.4.2) or a
/// nested structure element.
#[derive(Clone, Debug)]
#[allow(clippy::large_enum_variant)] // Child holds the nested element by value: the tree is
// author-side and short-lived, and boxing it would put an allocation in every `push_child` and a
// `Box` in the public API for no measurable gain.
pub enum StructKid {
    /// Marked content identified by `(page_index, mcid)` — the `BDC` operand on that page. Carrying
    /// the page per child lets one element span page breaks (an `/MCR` is emitted, §14.7.4.3).
    Content { page: usize, mcid: u32 },
    /// A nested structure element.
    Child(StructElem),
    /// The widget annotation of the `field`-th form field added with [`Builder::add_form_field`]
    /// (0-based, in call order), included as an `/OBJR` object reference (§14.7.4.3). Nesting the
    /// widget in a `Form` structure element satisfies PDF/UA-1 §7.18.4; the widget dictionary gets
    /// the matching `/StructParent` parent-tree key. An out-of-range index is skipped.
    Widget { field: usize },
    /// The `index`-th annotation added with [`Builder::add_annotation`] (0-based, in call order),
    /// included as an `/OBJR` object reference (§14.7.4.3). PDF/UA requires annotations in the
    /// structure tree — a link annotation nested in a `Link` element (14289-1 §7.18.5, 14289-2
    /// §8.2.5.20). The annotation gets the matching `/StructParent` key. Out-of-range → skipped.
    Annotation { index: usize },
}

/// A structure-attribute value (§14.7.6.2) — the value forms accessibility attributes use.
#[derive(Clone, Debug, PartialEq)]
pub enum AttrValue {
    /// A PDF name, e.g. `Column` for `/Scope` or `Decimal` for `/ListNumbering`.
    Name(String),
    /// An integer, e.g. a `ColSpan`/`RowSpan` count.
    Int(i64),
    /// A text string, e.g. the PrintField `/Desc` alternate field name.
    Text(String),
}

/// A structure attribute dictionary (§14.7.6): `entries` under one `owner` (`/O`, Table 341 of
/// ISO 32000-1). Attached to a [`StructElem`], each owner becomes one dictionary in the element's
/// `/A` (a lone dictionary is written directly, several as an array).
#[derive(Clone, Debug, PartialEq)]
pub struct StructAttr {
    /// The attribute owner (`/O`), e.g. `"Table"`, `"List"`, `"PrintField"`, `"Layout"`.
    pub owner: String,
    /// The attribute entries, e.g. `("Scope", AttrValue::Name("Column"))`.
    pub entries: Vec<(String, AttrValue)>,
}

/// The `/Scope` of a `TH` table-header cell (§14.8.5.4, **PDF 1.5**): which cells the header
/// applies to. PDF/UA-1 §7.5 wants it on every `TH`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThScope {
    /// The header labels the rest of its row.
    Row,
    /// The header labels the rest of its column.
    Column,
    /// The header labels both its row and its column.
    Both,
}

impl ThScope {
    fn name(self) -> &'static str {
        match self {
            ThScope::Row => "Row",
            ThScope::Column => "Column",
            ThScope::Both => "Both",
        }
    }
}

/// The `/ListNumbering` of an `L` list element (§14.8.5.5): the numbering system of its `Lbl`
/// children. PDF/UA-1 §7.6 requires it on ordered lists (any value but `None` marks the list
/// ordered/symbol-labelled).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListNumbering {
    /// No autonumbering; `Lbl` content is arbitrary.
    None,
    /// Solid circular bullet.
    Disc,
    /// Open circular bullet.
    Circle,
    /// Solid square bullet.
    Square,
    /// Decimal arabic numerals.
    Decimal,
    /// Uppercase roman numerals.
    UpperRoman,
    /// Lowercase roman numerals.
    LowerRoman,
    /// Uppercase letters.
    UpperAlpha,
    /// Lowercase letters.
    LowerAlpha,
}

impl ListNumbering {
    fn name(self) -> &'static str {
        match self {
            ListNumbering::None => "None",
            ListNumbering::Disc => "Disc",
            ListNumbering::Circle => "Circle",
            ListNumbering::Square => "Square",
            ListNumbering::Decimal => "Decimal",
            ListNumbering::UpperRoman => "UpperRoman",
            ListNumbering::LowerRoman => "LowerRoman",
            ListNumbering::UpperAlpha => "UpperAlpha",
            ListNumbering::LowerAlpha => "LowerAlpha",
        }
    }
}

/// The PrintField `/Role` (§14.8.5.6, **PDF 1.7**): the form-field kind a *non-interactive*
/// graphic stands for (e.g. a flattened field). PDF/UA-1 §7.14 requires PrintField attributes on
/// such content.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrintFieldRole {
    /// A radio button (`/rb`).
    RadioButton,
    /// A check box (`/cb`).
    Checkbox,
    /// A push button (`/pb`).
    PushButton,
    /// A text-value field whose value was converted to text (`/tv`).
    TextValue,
}

impl PrintFieldRole {
    fn name(self) -> &'static str {
        match self {
            PrintFieldRole::RadioButton => "rb",
            PrintFieldRole::Checkbox => "cb",
            PrintFieldRole::PushButton => "pb",
            PrintFieldRole::TextValue => "tv",
        }
    }
}

impl StructElem {
    /// A new element of structure type `tag` with no children.
    #[must_use]
    pub fn new(tag: impl Into<String>) -> Self {
        StructElem {
            tag: tag.into(),
            alt: None,
            actual_text: None,
            lang: None,
            ns: None,
            af: Vec::new(),
            id: None,
            refs: Vec::new(),
            attrs: Vec::new(),
            kids: Vec::new(),
        }
    }

    /// Set the replacement text (`/ActualText`, §14.9.4).
    #[must_use]
    pub fn actual_text(mut self, text: impl Into<String>) -> Self {
        self.actual_text = Some(text.into());
        self
    }

    /// Set this element's natural language (`/Lang`, §14.9.2), e.g. `"it-IT"` — declares a
    /// language change from the document `/Lang` (PDF/UA 14289-1 §7.2 / 14289-2 §8.2.7).
    #[must_use]
    pub fn lang(mut self, code: impl Into<String>) -> Self {
        self.lang = Some(code.into());
        self
    }

    /// Add a `/Ref` target (§14.7.4.2, **PDF 2.0**): the `/ID` of an element this one refers to.
    /// Give the target an [`StructElem::id`]; at build time each ID resolves to an indirect
    /// reference in this element's `/Ref` array. PDF/UA-2 §8.2.5.14 requires a `FENote` and its
    /// citing content to reference each other. Promotes the document to PDF 2.0.
    #[must_use]
    pub fn reference(mut self, target_id: impl Into<String>) -> Self {
        self.refs.push(target_id.into());
        self
    }

    /// Set the element identifier (`/ID`, §14.7.4.2) — a byte string that must be unique in the
    /// document; every element with one is listed in the `/StructTreeRoot /IDTree`. PDF/UA-1 §7.9
    /// requires an `/ID` on every `Note` element.
    #[must_use]
    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Attach a structure attribute (`/A`, §14.7.6) `key = value` under `owner` (`/O`). Entries
    /// sharing an owner merge into one attribute dictionary. Prefer the typed helpers
    /// ([`StructElem::th_scope`], [`StructElem::list_numbering`], [`StructElem::print_field`]) for
    /// the common accessibility attributes.
    #[must_use]
    pub fn attr(mut self, owner: &str, key: &str, value: AttrValue) -> Self {
        if let Some(existing) = self.attrs.iter_mut().find(|a| a.owner == owner) {
            existing.entries.push((key.to_string(), value));
        } else {
            self.attrs.push(StructAttr {
                owner: owner.to_string(),
                entries: vec![(key.to_string(), value)],
            });
        }
        self
    }

    /// Set the table-header `/Scope` (§14.8.5.4, **PDF 1.5**) — only meaningful on a `TH` element.
    /// PDF/UA-1 §7.5 wants every `TH` to carry one.
    #[must_use]
    pub fn th_scope(self, scope: ThScope) -> Self {
        self.attr("Table", "Scope", AttrValue::Name(scope.name().to_string()))
    }

    /// Set the list `/ListNumbering` (§14.8.5.5) — only meaningful on an `L` element. PDF/UA-1
    /// §7.6 requires it on ordered lists.
    #[must_use]
    pub fn list_numbering(self, numbering: ListNumbering) -> Self {
        self.attr(
            "List",
            "ListNumbering",
            AttrValue::Name(numbering.name().to_string()),
        )
    }

    /// Mark this element as the non-interactive representation of a form field with PrintField
    /// attributes (§14.8.5.6, **PDF 1.7** — PDF/UA-1 §7.14): the field `role`, the `checked` state
    /// for radio buttons/check boxes (`None` omits it; the spec default is off), and an optional
    /// `desc` alternate field name.
    #[must_use]
    pub fn print_field(
        self,
        role: PrintFieldRole,
        checked: Option<bool>,
        desc: Option<&str>,
    ) -> Self {
        let mut elem = self.attr(
            "PrintField",
            "Role",
            AttrValue::Name(role.name().to_string()),
        );
        if let Some(on) = checked {
            // Key casing is `checked` — the spec notes it does not follow the usual conventions.
            let state = if on { "on" } else { "off" };
            elem = elem.attr("PrintField", "checked", AttrValue::Name(state.to_string()));
        }
        if let Some(desc) = desc {
            elem = elem.attr("PrintField", "Desc", AttrValue::Text(desc.to_string()));
        }
        elem
    }

    /// Append the widget annotation of the `field`-th [`Builder::add_form_field`] call (0-based) as
    /// an `/OBJR` child (§14.7.4.3) — nest it in a `Form` element to satisfy PDF/UA-1 §7.18.4.
    pub fn push_widget(&mut self, field: usize) -> &mut Self {
        self.kids.push(StructKid::Widget { field });
        self
    }

    /// Append the `index`-th [`Builder::add_annotation`] call's annotation (0-based) as an `/OBJR`
    /// child (§14.7.4.3) — nest a link annotation in a `Link` element to satisfy PDF/UA
    /// (14289-1 §7.18.5, 14289-2 §8.2.5.20).
    pub fn push_annotation(&mut self, index: usize) -> &mut Self {
        self.kids.push(StructKid::Annotation { index });
        self
    }

    /// Set the alternate text (`/Alt`).
    #[must_use]
    pub fn alt(mut self, alt: impl Into<String>) -> Self {
        self.alt = Some(alt.into());
        self
    }

    /// Associate a file with this element (`/AF`, §14.13.6 — **PDF 2.0**, the preferred placement).
    /// The file is embedded and referenced from the element's `/AF`. Promotes the document to 2.0.
    #[must_use]
    pub fn associate_file(mut self, file: Attachment) -> Self {
        self.af.push(file);
        self
    }

    /// Set the structure **namespace** URI (`/NS`, §14.7.4 — **PDF 2.0**), e.g.
    /// `"http://iso.org/pdf2/ssn"`. Promotes the document to PDF 2.0.
    #[must_use]
    pub fn namespace(mut self, uri: impl Into<String>) -> Self {
        self.ns = Some(uri.into());
        self
    }

    /// Append a marked-content child on `page` with marked-content id `mcid`.
    pub fn push_content(&mut self, page: usize, mcid: u32) -> &mut Self {
        self.kids.push(StructKid::Content { page, mcid });
        self
    }

    /// Append a nested element child.
    pub fn push_child(&mut self, child: StructElem) -> &mut Self {
        self.kids.push(StructKid::Child(child));
        self
    }
}

/// Builds a new PDF from one or more pages of content-stream bytes.
///
/// ```ignore
/// let mut content = pdf_content::Content::new();
/// content.begin_text().set_font("F1", 24.0).text_move(72.0, 700.0).show_str("Hi").end_text();
/// let bytes = pdf_document::Builder::new()
///     .add_page(
///         pdf_document::PageSpec::new(content.into_bytes())
///             .standard_font("F1", pdf_document::StdFont::Helvetica),
///     )
///     .build();
/// ```
#[derive(Default)]
pub struct Builder {
    media_box: Option<[f64; 4]>,
    pages: Vec<PageState>,
    embedded: Vec<(String, CidFont)>,
    outlines: Vec<OutlineItem>,
    /// Document-information entries (`/Info`, §14.3.3) as raw `(key, value)` pairs, encoded into PDF
    /// text strings at build time so the `utf8` flag (set later) still applies.
    info: Vec<(String, String)>,
    metadata: Option<Vec<u8>>,
    output_intent: Option<OutputIntentSpec>,
    /// Page-level OutputIntents (§14.11.5, PDF 2.0): `(page index, intent)`.
    page_output_intents: Vec<(usize, OutputIntentSpec)>,
    /// Marked-content associated-file properties (§14.13.5, PDF 2.0):
    /// `(page index, property name, files)`.
    content_af_props: Vec<(usize, String, Vec<Attachment>)>,
    /// Developer extension declarations for the catalog `/Extensions` dictionary (§7.12).
    developer_extensions: Vec<crate::DeveloperExtension>,
    /// The encrypted payload of an unencrypted wrapper document (§7.6.7), if this is one.
    encrypted_payload: Option<EncryptedPayloadSpec>,
    file_id: Option<Vec<u8>>,
    attachments: Vec<Attachment>,
    /// Page-level associated files (`/AF`, §14.13.4, PDF 2.0): `(page index, file)`.
    page_attachments: Vec<(usize, Attachment)>,
    structure: Vec<StructElem>,
    lang: Option<String>,
    display_doc_title: bool,
    /// Annotations to author: `(page index, spec, associated files)`. The `/AF` array (§14.13.9,
    /// PDF 2.0) is usually empty; a non-empty one promotes the document to 2.0.
    annotations: Vec<(usize, AnnotationSpec, Vec<Attachment>)>,
    form_fields: Vec<(usize, FormFieldSpec, Vec<Attachment>)>,
    /// Document parts (§14.12, PDF 2.0): one leaf `DPart` per entry, spanning a page range.
    document_parts: Vec<DocumentPart>,
    /// Page-label ranges (§12.4.2) for the catalog `/PageLabels` number tree.
    page_labels: Vec<PageLabelRange>,
    /// Namespace role mappings (`/RoleMapNS`, §14.7.4, PDF 2.0).
    role_maps: Vec<RoleMapEntry>,
    /// Whether authored text is known to reference the `.notdef` glyph (GID 0) — set by the
    /// layout layer when shaping met a character the font lacks; PDF/UA forbids showing it.
    notdef_reference: bool,
    /// Structure namespace URI applied to the implicit `Document` root element (`/NS`, §14.7.4,
    /// PDF 2.0). `None` leaves the root un-namespaced (1.7-flavour tagging).
    struct_namespace: Option<String>,
    /// Schema file per namespace URI (`/Schema` on the `/Namespace` dict, §14.7.4).
    ns_schemas: Vec<(String, Attachment)>,
    /// Emit user-facing text strings (Info, outline titles, `/Alt`) as **UTF-8** with a `EF BB BF`
    /// BOM (§7.9.2.2, **PDF 2.0**) instead of UTF-16BE. Opt-in; promotes the document to PDF 2.0.
    utf8: bool,
    /// Explicit header version override (§7.5.2). `None` = stamp the **minimum** version the content
    /// requires (`pdf_writer::min_version`), so a plain document declares `%PDF-1.4` rather than an
    /// inflated `1.7`. Set it to pin a version (e.g. PDF/A-2/3 pin 1.7).
    version: Option<(u8, u8)>,
}

/// One **namespace role mapping** (`/RoleMapNS` in a `/Namespace` dictionary, §14.7.4 — PDF 2.0):
/// the structure type `custom`, used by elements whose `/NS` is `ns`, maps to the standard type
/// `target` in `target_ns` (`None` = the default PDF 1.7 namespace). Author the set with
/// [`Builder::role_map_ns`]; PDF/UA-2 §8.2.4 requires every element to belong — directly or via
/// role map — to the PDF 1.7, PDF 2.0 or MathML namespace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoleMapEntry {
    /// The namespace URI whose type is being mapped (the element's `/NS`).
    pub ns: String,
    /// The custom structure type being mapped.
    pub custom: String,
    /// The standard structure type it maps to.
    pub target: String,
    /// The namespace of `target`; `None` = the default (PDF 1.7) namespace.
    pub target_ns: Option<String>,
}

/// The numbering style of a [`PageLabelRange`] (`/S`, §12.4.2 Table 159).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageLabelStyle {
    /// Decimal arabic numerals (`/D`): 1, 2, 3…
    Decimal,
    /// Uppercase roman numerals (`/R`): I, II, III…
    RomanUpper,
    /// Lowercase roman numerals (`/r`): i, ii, iii…
    RomanLower,
    /// Uppercase letters (`/A`): A, B, C… then AA, BB…
    AlphaUpper,
    /// Lowercase letters (`/a`): a, b, c…
    AlphaLower,
}

impl PageLabelStyle {
    fn name(self) -> &'static str {
        match self {
            PageLabelStyle::Decimal => "D",
            PageLabelStyle::RomanUpper => "R",
            PageLabelStyle::RomanLower => "r",
            PageLabelStyle::AlphaUpper => "A",
            PageLabelStyle::AlphaLower => "a",
        }
    }
}

/// One page-label range (§12.4.2): from `first_page` (0-based) up to the next range, pages are
/// labelled `prefix` + the `style`-formatted number starting at `start`. Author the set with
/// [`Builder::page_labels`]. PDF/UA (14289-2 §8.12.3) wants labels whenever the displayed page
/// number differs from the ordinal position.
#[derive(Clone, Debug, Default)]
pub struct PageLabelRange {
    /// First page index of the range (0-based). The set of ranges must cover page 0; a missing
    /// page-0 range is synthesised as plain decimal.
    pub first_page: usize,
    /// The numbering style (`/S`); `None` labels pages with the prefix alone.
    pub style: Option<PageLabelStyle>,
    /// Label prefix (`/P`), e.g. `"A-"`.
    pub prefix: Option<String>,
    /// The numeric value of the range's first page (`/St`, default 1; must be ≥ 1).
    pub start: Option<u32>,
}

/// A document part (§14.12, PDF 2.0) covering a contiguous, inclusive run of pages — used to author
/// a `/DPartRoot` hierarchy with [`Builder::document_parts`].
#[derive(Clone, Debug, Default)]
pub struct DocumentPart {
    /// First page index of the part (0-based, inclusive).
    pub first_page: usize,
    /// Last page index of the part (0-based, inclusive).
    pub last_page: usize,
    /// Document Part Metadata (`/DPM`, §14.12.4): arbitrary `(key, value)` text entries describing
    /// this part (e.g. `("Title", "Chapter 1")`). Emitted as a `/DPM` dictionary on the leaf
    /// `DPart`; empty leaves `/DPM` off. Values are encoded as PDF text strings.
    pub dpm: Vec<(String, String)>,
}

impl Builder {
    /// A new builder with no pages (default page size US Letter).
    #[must_use]
    pub fn new() -> Self {
        Builder::default()
    }

    /// Set the default media box (page rectangle, in points) applied to pages added afterwards
    /// without their own size.
    pub fn media_box(&mut self, media_box: [f64; 4]) -> &mut Self {
        self.media_box = Some(media_box);
        self
    }

    /// Set an arbitrary document information entry (`/Info`, §14.3.3), e.g. `("Title", "...")`. The
    /// value is stored as a PDF text string (UTF-16BE for non-ASCII).
    pub fn info(&mut self, key: &str, value: &str) -> &mut Self {
        self.info.retain(|(k, _)| k != key);
        self.info.push((key.to_string(), value.to_string()));
        self
    }

    /// Pin the PDF header version (§7.5.2), e.g. `version(1, 7)`. By default [`Builder::build`]
    /// stamps the **minimum** version the content requires (so a plain document is `%PDF-1.4`);
    /// call this to force a specific version (a floor — `build` never stamps *below* what the
    /// content needs, but an explicit value above the minimum is honoured).
    pub fn version(&mut self, major: u8, minor: u8) -> &mut Self {
        self.version = Some((major, minor));
        self
    }

    /// Set the document title (`/Title`).
    pub fn title(&mut self, value: &str) -> &mut Self {
        self.info("Title", value)
    }
    /// Set the document author (`/Author`).
    pub fn author(&mut self, value: &str) -> &mut Self {
        self.info("Author", value)
    }
    /// Set the document subject (`/Subject`).
    pub fn subject(&mut self, value: &str) -> &mut Self {
        self.info("Subject", value)
    }
    /// Set the document keywords (`/Keywords`).
    pub fn keywords(&mut self, value: &str) -> &mut Self {
        self.info("Keywords", value)
    }
    /// Set the producing application (`/Creator`).
    pub fn creator(&mut self, value: &str) -> &mut Self {
        self.info("Creator", value)
    }

    /// Add one page and its resources (§7.7.3.3), described by a single [`PageSpec`].
    pub fn add_page(&mut self, page: PageSpec) -> &mut Self {
        self.pages.push(PageState {
            content: page.content,
            fonts: page.fonts,
            embedded: page.embedded,
            images: page.images,
            color_spaces: Vec::new(),
            forms: Vec::new(),
            media_box: page.media_box,
        });
        self
    }

    /// Add a top-level outline entry (bookmark, §12.3.3) titled `title` that jumps to page
    /// `page_index` (0-based), displayed `/Fit` to the window. Entries appear in the document
    /// outline (bookmarks panel) in the order added; an out-of-range index is clamped at build time.
    pub fn outline(&mut self, title: &str, page_index: usize) -> &mut Self {
        self.outlines.push(OutlineItem {
            title: title.to_string(),
            page_index,
        });
        self
    }

    /// Add an annotation (§12.5) to page `page_index` (0-based): a hyperlink or a text note, authored
    /// PDF/A-clean (see [`AnnotationSpec`]). Annotations appear in the page's `/Annots` array in the
    /// order added; an out-of-range page index is clamped at build time. The page must exist when
    /// [`Builder::build`] runs. `files` are associated with the annotation via its `/AF`
    /// array (§14.13.9 — **PDF 2.0**): each file is embedded, listed in the `/EmbeddedFiles` name tree
    /// (§7.7.4), and referenced from the annotation's `/AF`. A non-empty `files` promotes the document
    /// to PDF 2.0 (the header auto-stamps `%PDF-2.0`). An out-of-range page index is clamped at build
    /// time.
    pub fn add_annotation(
        &mut self,
        page_index: usize,
        annotation: AnnotationSpec,
        files: Vec<Attachment>,
    ) -> &mut Self {
        self.annotations.push((page_index, annotation, files));
        self
    }

    /// Add an interactive form field (§12.7) to page `page_index` (0-based). The field's widget is
    /// added to the page's `/Annots` and listed in the document's `/AcroForm`; building it also makes
    /// the catalog reference the form. PDF/A-clean (see [`FormFieldSpec`]). An out-of-range page index
    /// is clamped at build time. `files` are associated with the field via its `/AF`
    /// array (**associated files anywhere**, AN002 / §14.13 — **PDF 2.0**): each file is embedded,
    /// listed in the `/EmbeddedFiles` name tree (§7.7.4), and referenced from the field's widget
    /// dictionary. A non-empty `files` promotes the document to PDF 2.0 (the header auto-stamps
    /// `%PDF-2.0`).
    pub fn add_form_field(
        &mut self,
        page_index: usize,
        field: FormFieldSpec,
        files: Vec<Attachment>,
    ) -> &mut Self {
        self.form_fields.push((page_index, field, files));
        self
    }

    /// Author a Document Part hierarchy (§14.12, **PDF 2.0**): a catalog `/DPartRoot` whose root node
    /// groups one leaf `DPart` per entry in `parts`, each spanning an inclusive page range via
    /// `/Start`/`/End`. Because the catalog then carries `/DPartRoot`, [`Builder::build`] auto-stamps
    /// the header as `%PDF-2.0`. Out-of-range page indices are clamped at build time.
    pub fn document_parts(&mut self, parts: &[DocumentPart]) -> &mut Self {
        self.document_parts = parts.to_vec();
        self
    }

    /// Set the catalog's XMP `/Metadata` stream (§14.3.2): the raw XMP packet bytes, emitted
    /// unfiltered (`/Type /Metadata /Subtype /XML`). Required for PDF/A.
    pub fn metadata_xmp(&mut self, xmp: impl Into<Vec<u8>>) -> &mut Self {
        self.metadata = Some(xmp.into());
        self
    }

    /// Set a PDF/A OutputIntent (§14.11.5): the ICC profile bytes, its colour-component count `n`
    /// (1 = Gray, 3 = RGB, 4 = CMYK) and the output-condition identifier (e.g. `"sRGB"`).
    pub fn output_intent(
        &mut self,
        icc: impl Into<Vec<u8>>,
        n: u32,
        identifier: &str,
    ) -> &mut Self {
        self.output_intent = Some(OutputIntentSpec {
            icc: icc.into(),
            n,
            identifier: identifier.to_string(),
        });
        self
    }

    /// Make this document an **unencrypted wrapper** (§7.6.7 — **PDF 2.0**) for `payload`, a PDF
    /// encrypted with a custom security handler. The payload becomes the document's single
    /// embedded file — listed in `/EmbeddedFiles` and the catalog `/AF` with
    /// `/AFRelationship /EncryptedPayload`, its filespec carrying the `/EP` encrypted payload
    /// dictionary (Table 28) that names the required cryptographic filter — and the catalog gains
    /// a hidden collection (`/Collection /View /H`, §12.3.5) whose initial document is the
    /// payload. The wrapper's own pages should hold the "how to open this" instructions. §7.6.7
    /// requires the wrapper's `/EmbeddedFiles` tree to hold **exactly one** entry, so do not
    /// combine this with [`Builder::attach_file`]. Read back with
    /// [`Document::encrypted_payload`](crate::Document::encrypted_payload).
    pub fn encrypted_payload(&mut self, payload: EncryptedPayloadSpec) -> &mut Self {
        self.encrypted_payload = Some(payload);
        self
    }

    /// Declare a **developer extension** (`/Extensions`, §7.12): the catalog gains a direct
    /// extensions dictionary mapping the extension's registered `prefix` (Annex E) to a developer
    /// extensions dictionary with `/BaseVersion` and `/ExtensionLevel` (Table 49). Repeatable —
    /// multiple declarations under the *same* prefix are emitted as an **array** of developer
    /// extensions dictionaries (a PDF 2.0 form, like the optional `url`/`revision` keys), and
    /// [`Builder::build`] raises the header accordingly (`min_version` also floors the header at
    /// each extension's own `/BaseVersion`, §7.12.4).
    pub fn developer_extension(&mut self, extension: crate::DeveloperExtension) -> &mut Self {
        self.developer_extensions.push(extension);
        self
    }

    /// Register a marked-content associated-file property (§14.13.5 — **PDF 2.0**) on page
    /// `page_index` (0-based): `files` are embedded, listed in the `/EmbeddedFiles` name tree, and
    /// emitted as an *array of file specification dictionaries* named `property` in the page's
    /// `/Resources /Properties`. The page's content stream links graphics objects to them with an
    /// `/AF /<property> BDC … EMC` sequence ([`Content::begin_af_marked_content`] in
    /// `pdf-content`). Because §14.13.5 is a PDF 2.0 feature, [`Builder::build`] auto-stamps the
    /// header `%PDF-2.0` when any property is registered. An out-of-range page index is clamped.
    ///
    /// [`Content::begin_af_marked_content`]: https://docs.rs/pdf-content
    pub fn add_content_af_property(
        &mut self,
        page_index: usize,
        property: &str,
        files: Vec<Attachment>,
    ) -> &mut Self {
        self.content_af_props
            .push((page_index, property.to_string(), files));
        self
    }

    /// Set a **page-level** OutputIntent (§14.11.5 — **PDF 2.0**) on page `page_index` (0-based):
    /// PDF 2.0 allows `/OutputIntents` on individual page objects, overriding the document-level
    /// intent ([`Builder::output_intent`]) for that page. Same parameters; repeatable (each call
    /// appends to that page's array). Because a page then carries `/OutputIntents`,
    /// [`Builder::build`] auto-stamps the header as `%PDF-2.0`. An out-of-range page index is
    /// clamped at build time.
    pub fn page_output_intent(
        &mut self,
        page_index: usize,
        icc: impl Into<Vec<u8>>,
        n: u32,
        identifier: &str,
    ) -> &mut Self {
        self.page_output_intents.push((
            page_index,
            OutputIntentSpec {
                icc: icc.into(),
                n,
                identifier: identifier.to_string(),
            },
        ));
        self
    }

    /// Set the trailer file `/ID` (§14.4) — required for PDF/A. When set, [`Builder::build`] emits a
    /// trailer with `/ID [<hex> <hex>]`.
    pub fn file_id(&mut self, id: impl Into<Vec<u8>>) -> &mut Self {
        self.file_id = Some(id.into());
        self
    }

    /// Embed `attachment` as a document-associated file (§7.11.4 / §14.13): it goes in the
    /// `/EmbeddedFiles` name tree and the catalog's `/AF` array. Attachments are only valid in
    /// PDF/A-3 (not 2); PDF without a PDF/A claim may use them freely.
    pub fn attach_file(&mut self, attachment: Attachment) -> &mut Self {
        self.attachments.push(attachment);
        self
    }

    /// Remove every document-information entry (`/Info`, §14.3.3) set so far. Used by conformance
    /// passes targeting PDF/A-4, where an Info dictionary may carry nothing beyond `/ModDate`
    /// (ISO 19005-4 §6.1.3) — the XMP `/Metadata` stream alone holds the document metadata.
    pub fn clear_info(&mut self) -> &mut Self {
        self.info.clear();
        self
    }

    /// Associate `attachment` with page `page_index` (0-based) via the page's `/AF` array
    /// (**associated files anywhere**, §14.13.4 — **PDF 2.0**), rather than at the catalog level.
    /// The file is embedded and listed in the `/EmbeddedFiles` name tree like any attachment, but its
    /// filespec is referenced from the page. Because a page then carries `/AF`, [`Builder::build`]
    /// auto-stamps the header as `%PDF-2.0`. An out-of-range page index is clamped at build time.
    pub fn attach_file_to_page(&mut self, page_index: usize, attachment: Attachment) -> &mut Self {
        self.page_attachments.push((page_index, attachment));
        self
    }

    /// Make the document **Tagged** (§14.7/§14.8): supply the logical structure as a flat list of
    /// [`StructElem`]s bound to page marked content by MCID. At build time this emits a
    /// `/StructTreeRoot` (with a `Document` root over the elements), a `/ParentTree`, and
    /// `/MarkInfo <</Marked true>>` in the catalog. The page content must already wrap that content in
    /// `BDC … EMC` with matching MCIDs (see `Content::begin_marked_content`). Replaces any prior call.
    pub fn structure(&mut self, elements: Vec<StructElem>) -> &mut Self {
        self.structure = elements;
        self
    }

    /// Append one structure element to the logical structure (see [`Builder::structure`]) without
    /// replacing what is already there — e.g. a `Form` element wrapping a form-field widget
    /// ([`StructElem::push_widget`], PDF/UA-1 §7.18.4) added after a tagged `Flow` supplied the
    /// main tree.
    pub fn add_structure_element(&mut self, elem: StructElem) -> &mut Self {
        self.structure.push(elem);
        self
    }

    /// Set the document's natural language (`/Lang`, §14.9.2), e.g. `"en-US"` — recommended for
    /// Tagged PDF and required by PDF/UA.
    pub fn lang(&mut self, code: &str) -> &mut Self {
        self.lang = Some(code.to_string());
        self
    }

    /// Apply a structure **namespace** (`/NS`, §14.7.4 — **PDF 2.0**) to the implicit `Document`
    /// root element of a Tagged PDF, e.g. `"http://iso.org/pdf2/ssn"` (the standard structure
    /// namespace) or the PDF/UA-2 namespace. The URI is emitted as a `/Namespace` dictionary, listed
    /// in `/StructTreeRoot /Namespaces`, and referenced by the root's `/NS`. Per-element namespaces
    /// can also be set via [`StructElem::namespace`]. Because the structure then carries a namespace,
    /// [`Builder::build`] auto-stamps the header as `%PDF-2.0`.
    pub fn structure_namespace(&mut self, uri: &str) -> &mut Self {
        self.struct_namespace = Some(uri.to_string());
        self
    }

    /// Attach a schema file to the structure namespace `uri` (§14.7.4, **PDF 2.0**): the file (e.g.
    /// an XSD or RNG describing the namespace's types) is embedded, listed in the `/EmbeddedFiles`
    /// name tree, and referenced as the `/Schema` file specification of the matching `/Namespace`
    /// dictionary. The namespace must actually occur in the structure tree (via
    /// [`Builder::structure_namespace`], [`StructElem::namespace`] or a role map) for its
    /// dictionary — and so the schema — to be emitted.
    pub fn structure_namespace_schema(&mut self, uri: &str, schema: Attachment) -> &mut Self {
        self.ns_schemas.push((uri.to_string(), schema));
        self
    }

    /// Emit the document's user-facing **text strings** — Info entries (Title/Author/…), outline
    /// titles, and structure-element `/Alt` — as **UTF-8** with a `EF BB BF` byte-order mark
    /// (§7.9.2.2, **PDF 2.0**) instead of the default UTF-16BE. Because a UTF-8 text string is a 2.0
    /// construct, [`Builder::build`] auto-stamps the header as `%PDF-2.0`. ASCII-only strings without
    /// a BOM are interpreted identically, so this is a no-op for purely-ASCII documents unless they
    /// carry non-ASCII text. Opt-in; off by default.
    pub fn utf8_text_strings(&mut self) -> &mut Self {
        self.utf8 = true;
        self
    }

    /// Encode `s` as a PDF text string honouring this builder's `utf8` flag (§7.9.2.2).
    fn encode_text(&self, s: &str) -> Vec<u8> {
        text_string_maybe_utf8(s, self.utf8)
    }

    /// Set the viewer-preferences `/DisplayDocTitle` flag (§12.2): when `true`, a reader shows the
    /// document's title (from `/Info` / XMP) in its title bar instead of the file name. Required by
    /// PDF/UA.
    pub fn display_doc_title(&mut self, on: bool) -> &mut Self {
        self.display_doc_title = on;
        self
    }

    /// Set the document's **page labels** (§12.4.2): the catalog `/PageLabels` number tree built
    /// from `ranges` (sorted by [`PageLabelRange::first_page`]; a range covering page 0 is
    /// required by the spec and synthesised as plain decimal if missing). PDF/UA (14289-2
    /// §8.12.3) wants labels whenever the shown page number is not simply `index + 1` — e.g.
    /// roman front matter followed by decimal content. Replaces any prior call.
    pub fn page_labels(&mut self, ranges: Vec<PageLabelRange>) -> &mut Self {
        self.page_labels = ranges;
        self
    }

    /// Set the **namespace role maps** (`/RoleMapNS`, §14.7.4 — PDF 2.0): each entry maps a
    /// custom structure type in its namespace to a standard type (see [`RoleMapEntry`]). The
    /// mapped namespaces (and any explicit target namespaces) are emitted as `/Namespace`
    /// dictionaries carrying `/RoleMapNS`, listed in `/StructTreeRoot /Namespaces`. Replaces any
    /// prior call. Promotes the document to PDF 2.0.
    pub fn role_map_ns(&mut self, entries: Vec<RoleMapEntry>) -> &mut Self {
        self.role_maps = entries;
        self
    }

    /// Record that authored text references the `.notdef` glyph (GID 0) — called by the layout
    /// layer when shaping met a character the embedded font lacks. PDF/UA (14289-1 §7.21.8 /
    /// 14289-2 §8.4.5.9) forbids text-showing operators from referencing `.notdef`; the
    /// production passes reject a flagged document.
    pub fn flag_notdef_reference(&mut self) -> &mut Self {
        self.notdef_reference = true;
        self
    }

    /// Register an embedded composite font (Type0/`Identity-H`), shared across pages, under
    /// `name` — the `/Fn` key pages reference via [`PageSpec::embedded_font`]. A name already
    /// registered is kept.
    pub fn embed_cid_font(&mut self, name: &str, font: CidFont) -> &mut Self {
        if !self.embedded.iter().any(|(n, _)| n == name) {
            self.embedded.push((name.to_string(), font));
        }
        self
    }

    /// Attach a reusable content Form XObject (§8.10) named `name` to the most recently added page,
    /// exposing it in that page's `/Resources /XObject`. `bbox` is its bounding box and `content` its
    /// own content stream; paint it (any number of times) with
    /// `pdf_content::Content::do_xobject(name)`, typically wrapped in `q`/`cm`/`Q` to position it.
    /// The form carries an empty `/Resources` (pure graphics; it cannot reference the page's
    /// fonts). `files` are associated with it via its
    /// `/AF` array (§14.13.7 — **PDF 2.0**): each file is embedded, listed in the `/EmbeddedFiles`
    /// name tree (§7.7.4), and referenced from the form's `/AF`. A non-empty `files` promotes the
    /// document to PDF 2.0. No-op if no page has been added yet.
    pub fn add_form_xobject(
        &mut self,
        name: &str,
        bbox: [f64; 4],
        content: impl Into<Vec<u8>>,
        files: Vec<Attachment>,
    ) -> &mut Self {
        if let Some(page) = self.pages.last_mut() {
            page.forms.push(FormXObjectSpec {
                name: name.to_string(),
                bbox,
                content: content.into(),
                files,
            });
        }
        self
    }

    /// Attach a Separation (spot) colour space (§8.6.6) named `name` to the most recently added
    /// page, exposing it in that page's `/Resources /ColorSpace`. The page's content selects it with
    /// `pdf_content::Content::set_fill_color_space(name)` and sets a tint with
    /// `pdf_content::Content::set_fill_color(&[t])`. No-op if no page has been added yet.
    pub fn add_separation(&mut self, name: &str, separation: SeparationSpec) -> &mut Self {
        if let Some(page) = self.pages.last_mut() {
            page.color_spaces
                .push((name.to_string(), ColorSpaceKind::Separation(separation)));
        }
        self
    }

    /// Attach an ICCBased colour space (§8.6.5.5) named `name` to the most recently added page,
    /// exposing it in that page's `/Resources /ColorSpace`. `icc` is the raw ICC profile and `n` its
    /// component count (1 = Gray, 3 = RGB, 4 = CMYK); the profile is embedded FlateDecode-compressed.
    /// Paint with `pdf_content::Content::set_fill_color_space(name)` then `n` components via
    /// `pdf_content::Content::set_fill_color`. No-op if no page has been added yet.
    pub fn add_icc_based(&mut self, name: &str, icc: impl Into<Vec<u8>>, n: u32) -> &mut Self {
        if let Some(page) = self.pages.last_mut() {
            page.color_spaces
                .push((name.to_string(), ColorSpaceKind::Icc { icc: icc.into(), n }));
        }
        self
    }

    /// Attach an Indexed (palette) colour space (§8.6.6.3) named `name` to the most recently added
    /// page. `base` is the device space the palette entries live in and `palette` is the lookup
    /// table laid out as consecutive entries (`components(base)` bytes each, `0..=255` per channel);
    /// the highest index is `palette.len() / components(base) - 1`. Paint with
    /// `pdf_content::Content::set_fill_color_space(name)` then a single integer index via
    /// `pdf_content::Content::set_fill_color`. No-op if no page has been added yet.
    pub fn add_indexed(
        &mut self,
        name: &str,
        base: ImageColorSpace,
        palette: impl Into<Vec<u8>>,
    ) -> &mut Self {
        if let Some(page) = self.pages.last_mut() {
            page.color_spaces.push((
                name.to_string(),
                ColorSpaceKind::Indexed {
                    base,
                    palette: palette.into(),
                },
            ));
        }
        self
    }

    /// Attach a CIE L*a*b* colour space (§8.6.5.4) named `name` to the most recently added page.
    /// `white_point` is the diffuse white `[Xw Yw Zw]` (e.g. D50 `[0.9505, 1.0, 1.089]`) and `range`
    /// the a*/b* bounds `[amin amax bmin bmax]`. Paint with
    /// `pdf_content::Content::set_fill_color_space(name)` then the three `L* a* b*` components via
    /// `pdf_content::Content::set_fill_color`. No-op if no page has been added yet.
    pub fn add_lab(&mut self, name: &str, white_point: [f64; 3], range: [f64; 4]) -> &mut Self {
        if let Some(page) = self.pages.last_mut() {
            page.color_spaces
                .push((name.to_string(), ColorSpaceKind::Lab { white_point, range }));
        }
        self
    }
}

#[path = "builder_build.rs"]
mod build;

#[path = "builder_facts.rs"]
mod facts;
pub use facts::{DocumentFacts, StructureElementFact};

/// Rebuild `obj` with every UTF-8 text string (BOM `EF BB BF`, §7.9.2.2 — a PDF 2.0 construct)
/// re-encoded as its pre-2.0 compatible form, UTF-16BE with a `FE FF` BOM. Strings that are not
/// valid UTF-8 after the BOM are left untouched (they will then trip the construct gate rather
/// than be corrupted silently).
fn downgrade_utf8_strings(obj: &Object) -> Object {
    match obj {
        Object::String(s) => {
            let bytes = s.as_bytes();
            match bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
                Some(rest) => match std::str::from_utf8(rest) {
                    Ok(text) => Object::String(PdfString::from(text_string(text))),
                    Err(_) => obj.clone(),
                },
                None => obj.clone(),
            }
        }
        Object::Array(a) => Object::Array(a.iter().map(downgrade_utf8_strings).collect()),
        Object::Dictionary(d) => {
            let mut out = Dictionary::new();
            for (key, value) in d.iter() {
                out.insert(key.clone(), downgrade_utf8_strings(value));
            }
            Object::Dictionary(out)
        }
        Object::Stream(s) => {
            let mut dict = Dictionary::new();
            for (key, value) in s.dict().iter() {
                dict.insert(key.clone(), downgrade_utf8_strings(value));
            }
            Object::Stream(Stream::new(dict, s.raw().clone()))
        }
        other => other.clone(),
    }
}

/// Encode `s` as a PDF text string (§7.9.2.2): plain bytes for ASCII, else UTF-16BE with a BOM.
pub(crate) fn text_string(s: &str) -> Vec<u8> {
    if s.is_ascii() {
        s.as_bytes().to_vec()
    } else {
        let mut bytes = vec![0xFE, 0xFF];
        for unit in s.encode_utf16() {
            bytes.extend_from_slice(&unit.to_be_bytes());
        }
        bytes
    }
}

/// Encode `s` as a PDF text string, optionally as **UTF-8** (§7.9.2.2, PDF 2.0). With `utf8` set and
/// a non-ASCII `s`, emits a `EF BB BF` BOM followed by the UTF-8 bytes; otherwise falls back to
/// [`text_string`] (ASCII bytes, or UTF-16BE). ASCII strings never get a BOM, so they stay version-1.4.
fn text_string_maybe_utf8(s: &str, utf8: bool) -> Vec<u8> {
    if utf8 && !s.is_ascii() {
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(s.as_bytes());
        bytes
    } else {
        text_string(s)
    }
}

#[path = "builder_resources.rs"]
mod resources;
use resources::*;

#[path = "builder_structure.rs"]
mod structure;
use structure::*;

#[path = "builder_interactive.rs"]
mod interactive;
use interactive::*;

#[allow(clippy::too_many_arguments)]
fn page_dict(
    parent: ObjectId,
    content: ObjectId,
    media_box: &[f64; 4],
    font_resources: Dictionary,
    xobject_resources: Dictionary,
    colorspace_resources: Dictionary,
    properties_resources: Dictionary,
    struct_parents: Option<i64>,
) -> Dictionary {
    let mut resources = Dictionary::new();
    if !font_resources.is_empty() {
        resources.insert(Name::from("Font"), Object::Dictionary(font_resources));
    }
    if !xobject_resources.is_empty() {
        resources.insert(Name::from("XObject"), Object::Dictionary(xobject_resources));
    }
    if !colorspace_resources.is_empty() {
        resources.insert(
            Name::from("ColorSpace"),
            Object::Dictionary(colorspace_resources),
        );
    }
    if !properties_resources.is_empty() {
        resources.insert(
            Name::from("Properties"),
            Object::Dictionary(properties_resources),
        );
    }
    let media = Array::from(
        media_box
            .iter()
            .map(|&v| Object::Real(v))
            .collect::<Vec<_>>(),
    );

    let mut dict = Dictionary::new();
    dict.insert(Name::from("Type"), Object::Name(Name::from("Page")));
    dict.insert(Name::from("Parent"), Object::Reference(parent));
    dict.insert(Name::from("MediaBox"), Object::Array(media));
    dict.insert(Name::from("Resources"), Object::Dictionary(resources));
    dict.insert(Name::from("Contents"), Object::Reference(content));
    if let Some(n) = struct_parents {
        dict.insert(Name::from("StructParents"), Object::Integer(n));
    }
    dict
}

#[cfg(test)]
#[path = "builder_tests_core.rs"]
mod tests_core;
#[cfg(test)]
#[path = "builder_tests_pdf20.rs"]
mod tests_pdf20;
#[cfg(test)]
#[path = "builder_tests_structure.rs"]
mod tests_structure;
