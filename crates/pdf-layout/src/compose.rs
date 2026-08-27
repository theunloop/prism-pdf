//! Declarative, constraint-based document composition (§7.7/§9.4).
//!
//! This first vertical slice proves the measurement/drawing protocol with a paginating [`Column`]
//! and wrapping [`Text`]. Coordinates use a top-left origin with y descending; conversion to PDF
//! user space happens only while drawing.

use std::cell::RefCell;
use std::collections::BTreeMap;

use pdf_content::Content;
use pdf_document::{
    AnnotationSpec, Builder, CidFont, ImageXObject, LinkTarget, ListNumbering, PageSpec, StdFont,
    StructElem, ThScope,
};

use crate::PageStyle;
use crate::metrics::{EmbeddedMetrics, FontMetrics, StandardMetrics, wrap_paragraph_with};

const EPSILON: f64 = 1.0e-9;

/// A width and height in composition points.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Size {
    /// Horizontal extent.
    pub width: f64,
    /// Vertical extent.
    pub height: f64,
}

impl Size {
    /// Construct a size.
    #[must_use]
    pub const fn new(width: f64, height: f64) -> Self {
        Self { width, height }
    }

    fn is_valid(self) -> bool {
        self.width.is_finite() && self.height.is_finite() && self.width >= 0.0 && self.height >= 0.0
    }
}

/// A position in top-left-origin composition coordinates.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point {
    /// Distance from the page's left edge.
    pub x: f64,
    /// Distance from the page's top edge.
    pub y: f64,
}

/// A placed rectangle in top-left-origin composition coordinates.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    /// Top-left position.
    pub origin: Point,
    /// Measured extent.
    pub size: Size,
}

/// An RGB colour with components in the inclusive range 0–1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color {
    /// Red component.
    pub red: f64,
    /// Green component.
    pub green: f64,
    /// Blue component.
    pub blue: f64,
}

impl Color {
    /// Construct an RGB colour.
    #[must_use]
    pub const fn rgb(red: f64, green: f64, blue: f64) -> Self {
        Self { red, green, blue }
    }

    fn is_valid(self) -> bool {
        [self.red, self.green, self.blue]
            .into_iter()
            .all(|component| component.is_finite() && (0.0..=1.0).contains(&component))
    }
}

/// Horizontal placement inside a constrained box.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HorizontalAlign {
    /// Place at the left edge.
    #[default]
    Left,
    /// Centre horizontally.
    Center,
    /// Place at the right edge.
    Right,
}

/// Vertical placement inside a constrained box.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum VerticalAlign {
    /// Place at the top edge.
    #[default]
    Top,
    /// Centre vertically.
    Center,
    /// Place at the bottom edge.
    Bottom,
}

/// Scaling policy for a composition image inside a requested box (§8.9).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ImageSizing {
    /// Preserve aspect ratio and fit entirely inside the box.
    Fit(Size),
    /// Preserve aspect ratio, fill the box, and clip overflow.
    Fill(Size),
    /// Scale directly to the box, even when that changes aspect ratio.
    Exact(Size),
}

/// Logical-structure annotation for tagged composition (§14.7–§14.8).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Semantic {
    /// Paragraph (`P`).
    Paragraph,
    /// Heading level 1–6 (`H1`–`H6`).
    Heading(u8),
    /// List container (`L`).
    List,
    /// List item (`LI`).
    ListItem,
    /// List label (`Lbl`).
    ListLabel,
    /// List body (`LBody`).
    ListBody,
    /// Table container (`Table`).
    Table,
    /// Table row (`TR`).
    TableRow,
    /// Column header cell (`TH` with `/Scope /Column`).
    TableHeaderCell,
    /// Data cell (`TD`).
    TableCell,
    /// URI link with an accessible description.
    Link { uri: String, description: String },
    /// Figure carrying required alternate text.
    Figure { alt: String },
}

/// What an element reports when measured against a constraint.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Plan {
    /// The element has no remaining content.
    Empty,
    /// All remaining content fits in this size.
    Full(Size),
    /// This size contains a prefix; the element must be offered space again.
    Partial(Size),
    /// Nothing fits in the offered remainder; retry in fresh space.
    Wrap,
}

/// One drawn element in a PDF-independent geometry trace.
#[derive(Clone, Debug, PartialEq)]
pub struct GeometryEvent {
    /// Zero-based physical page index.
    pub page: usize,
    /// Stable element kind such as `"Column"`, `"Row"`, `"Decorated"`, or `"Text"`.
    pub kind: &'static str,
    /// Placed bounds in top-left-origin page coordinates.
    pub bounds: Rect,
    /// Text drawn by this event, when it is a text element.
    pub text: Option<String>,
}

/// PDF-independent placement log produced alongside composed bytes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GeometryTrace {
    events: Vec<GeometryEvent>,
}

impl GeometryTrace {
    /// Events in draw order. This is also semantic reading order for this slice.
    #[must_use]
    pub fn events(&self) -> &[GeometryEvent] {
        &self.events
    }
}

/// A successfully composed PDF and its deterministic geometry trace.
#[derive(Clone, Debug)]
pub struct ComposedDocument {
    pdf: Vec<u8>,
    trace: GeometryTrace,
}

/// A measured and drawn composition awaiting optional document-level post-processing.
///
/// This keeps conformance policy in the facade/standards layer while allowing a composition to be
/// passed through `prismpdf::make_pdfua` or `prismpdf::make_pdfa` before serialisation.
pub struct PreparedComposition {
    builder: Builder,
    trace: GeometryTrace,
}

impl PreparedComposition {
    /// Mutably borrow the low-level builder for metadata or conformance passes.
    pub fn builder_mut(&mut self) -> &mut Builder {
        &mut self.builder
    }

    /// Serialise the prepared document and retain its geometry trace.
    #[must_use]
    pub fn build(self) -> ComposedDocument {
        ComposedDocument {
            pdf: self.builder.build(),
            trace: self.trace,
        }
    }
}

impl ComposedDocument {
    /// Borrow the generated PDF bytes.
    #[must_use]
    pub fn pdf(&self) -> &[u8] {
        &self.pdf
    }

    /// Borrow the geometry trace.
    #[must_use]
    pub fn trace(&self) -> &GeometryTrace {
        &self.trace
    }

    /// Consume the result and return the PDF bytes.
    #[must_use]
    pub fn into_pdf(self) -> Vec<u8> {
        self.pdf
    }
}

/// Composition failures detected before an invalid PDF or an infinite loop can be produced.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum ComposeError {
    /// Page geometry, a constraint, or a text size/leading was non-finite or non-positive.
    #[error("invalid composition geometry")]
    InvalidGeometry,
    /// A text element references a font resource that was not registered.
    #[error("font resource {0} is not registered")]
    MissingFont(String),
    /// An element cannot fit even in a fresh page content region.
    #[error("element cannot fit in a fresh page content region")]
    OverTallElement,
    /// Measurement returned a drawable plan that consumed no space.
    #[error("composition made no observable progress")]
    NoProgress,
    /// Drawing did not receive the size from its immediately preceding measurement.
    #[error("drawing diverged from its preceding measurement")]
    MeasurementMismatch,
    /// Embedded-font bytes are not a valid TrueType/OpenType font.
    #[error("invalid embedded font program")]
    InvalidFont,
}

/// Text styling for composition.
#[derive(Clone, Debug)]
pub struct TextStyle {
    font_resource: String,
    size: f64,
    leading: f64,
}

impl TextStyle {
    /// Default style: resource `F1`, 12-point type and 14-point leading.
    #[must_use]
    pub fn new() -> Self {
        Self {
            font_resource: "F1".to_string(),
            size: 12.0,
            leading: 14.0,
        }
    }

    /// Select a registered font resource.
    #[must_use]
    pub fn font(mut self, resource: &str) -> Self {
        self.font_resource = resource.to_string();
        self
    }

    /// Set the font size. Leading follows at 1.18× unless set afterwards.
    #[must_use]
    pub fn size(mut self, size: f64) -> Self {
        self.size = size;
        self.leading = size * 1.18;
        self
    }

    /// Set baseline-to-baseline line spacing.
    #[must_use]
    pub fn leading(mut self, leading: f64) -> Self {
        self.leading = leading;
        self
    }
}

impl Default for TextStyle {
    fn default() -> Self {
        Self::new()
    }
}

/// An owned declarative document description.
pub struct Composition {
    fonts: BTreeMap<String, FontSlot>,
    pages: Vec<PageDefinition>,
    lang: Option<String>,
}

impl Composition {
    /// Start a composition with Helvetica registered as `F1`.
    #[must_use]
    pub fn new() -> Self {
        let mut fonts = BTreeMap::new();
        fonts.insert("F1".to_string(), FontSlot::Standard(StdFont::Helvetica));
        Self {
            fonts,
            pages: Vec::new(),
            lang: None,
        }
    }

    /// Register a Standard-14 font resource available to every composed page.
    #[must_use]
    pub fn standard_font(mut self, resource: &str, font: StdFont) -> Self {
        self.fonts
            .insert(resource.to_string(), FontSlot::Standard(font));
        self
    }

    /// Register a TrueType/OpenType program as a composite embedded font (§9.7/§9.9).
    ///
    /// # Errors
    /// Returns [`ComposeError::InvalidFont`] when `program` is not a supported sfnt font.
    pub fn embedded_font(mut self, resource: &str, program: &[u8]) -> Result<Self, ComposeError> {
        let info = pdf_fonts::font_info(program).ok_or(ComposeError::InvalidFont)?;
        self.fonts.insert(
            resource.to_string(),
            FontSlot::Embedded(EmbeddedSlot {
                program: program.to_vec(),
                info,
                used: RefCell::new(BTreeMap::new()),
            }),
        );
        Ok(self)
    }

    /// Enable tagged-PDF output and set the document's natural language (§14.7/§14.9.2).
    #[must_use]
    pub fn tagged(mut self, lang: &str) -> Self {
        self.lang = Some(lang.to_string());
        self
    }

    /// Add one page design. Its content tree may produce multiple physical pages.
    #[must_use]
    pub fn page(mut self, style: PageStyle, configure: impl FnOnce(&mut Page)) -> Self {
        let mut page = Page::new();
        configure(&mut page);
        self.pages.push(PageDefinition {
            style,
            root: page.root.unwrap_or_else(Node::empty_column),
            header: page.header,
            footer: page.footer,
        });
        self
    }

    /// Measure, paginate and draw the composition into PDF bytes plus a geometry trace.
    ///
    /// # Errors
    /// Returns [`ComposeError`] for invalid geometry, missing fonts, over-tall elements,
    /// measurement/draw divergence, or zero-progress pagination.
    pub fn build(self) -> Result<ComposedDocument, ComposeError> {
        Ok(self.into_builder()?.build())
    }

    /// Measure and draw into a builder so higher layers can apply conformance policy.
    ///
    /// # Errors
    /// Returns the same layout failures as [`Composition::build`].
    pub fn into_builder(self) -> Result<PreparedComposition, ComposeError> {
        let metrics = Metrics::new(&self.fonts);
        let mut builder = Builder::new();
        let mut trace = GeometryTrace::default();
        let mut physical_page = 0usize;
        let tagged = self.lang.is_some();
        let mut annotations = Vec::new();
        let mut structure = Vec::new();

        let mut pages = if self.pages.is_empty() {
            vec![PageDefinition {
                style: PageStyle::default(),
                root: Node::empty_column(),
                header: None,
                footer: None,
            }]
        } else {
            self.pages
        };

        let mut page_counts = Vec::with_capacity(pages.len());
        for definition in &mut pages {
            page_counts.push(preflight_pages(definition, &metrics)?);
        }
        let total_pages = page_counts.iter().sum::<usize>();

        for (mut definition, expected_pages) in pages.into_iter().zip(page_counts) {
            let first_physical_page = physical_page;
            let content_area = content_size(definition.style)?;
            definition.root.reset();
            let mut produced_for_design = false;
            loop {
                let page_number = physical_page + 1;
                definition.root.set_page_numbers(page_number, total_pages);
                let header_size = measure_repeating(
                    definition.header.as_mut(),
                    content_area,
                    &metrics,
                    page_number,
                    total_pages,
                )?;
                let footer_size = measure_repeating(
                    definition.footer.as_mut(),
                    content_area,
                    &metrics,
                    page_number,
                    total_pages,
                )?;
                let available = body_size(content_area, header_size, footer_size)?;
                let plan = definition.root.measure(available, &metrics)?;
                let measured = match checked_size(plan, definition.root.has_remaining())? {
                    None => {
                        if produced_for_design {
                            break;
                        }
                        Size::default()
                    }
                    Some(size) => size,
                };
                let mut content = Content::new();
                let mut images = Vec::new();
                let mut mcid_next = 0u32;
                let mut marked_depth = 0usize;
                let origin = Point {
                    x: definition.style.margins[0],
                    y: definition.style.margins[2],
                };
                let mut context = DrawCtx {
                    content: &mut content,
                    metrics: &metrics,
                    trace: &mut trace,
                    page: physical_page,
                    page_height: definition.style.size[1],
                    origin,
                    images: &mut images,
                    tagged,
                    mcid_next: &mut mcid_next,
                    annotations: &mut annotations,
                    marked_depth: &mut marked_depth,
                };
                if tagged && header_size != Size::default() {
                    context.content.begin_artifact();
                }
                context.tagged = false;
                draw_repeating(definition.header.as_mut(), &mut context, header_size)?;
                context.tagged = tagged;
                if tagged && header_size != Size::default() {
                    context.content.end_marked_content();
                }
                if !matches!(plan, Plan::Empty) {
                    let mut body_context = context.translated(0.0, header_size.height);
                    definition.root.draw(&mut body_context, measured)?;
                }
                let footer_y = content_area.height - footer_size.height;
                let mut footer_context = context.translated(0.0, footer_y);
                footer_context.tagged = false;
                if tagged && footer_size != Size::default() {
                    footer_context.content.begin_artifact();
                }
                draw_repeating(definition.footer.as_mut(), &mut footer_context, footer_size)?;
                if tagged && footer_size != Size::default() {
                    footer_context.content.end_marked_content();
                }

                let mut page = PageSpec::new(content.into_bytes()).media_box([
                    0.0,
                    0.0,
                    definition.style.size[0],
                    definition.style.size[1],
                ]);
                for (resource, font) in &self.fonts {
                    page = match font {
                        FontSlot::Standard(font) => page.standard_font(resource, *font),
                        FontSlot::Embedded(_) => page.embedded_font(resource),
                    };
                }
                for (name, image) in images {
                    page = page.image(name, image);
                }
                builder.add_page(page);
                physical_page += 1;
                produced_for_design = true;

                if matches!(plan, Plan::Full(_) | Plan::Empty) {
                    break;
                }
            }
            if produced_for_design && physical_page - first_physical_page != expected_pages {
                return Err(ComposeError::MeasurementMismatch);
            }
            if tagged {
                definition.root.collect_structure(&mut structure)?;
            }
        }

        for (page, annotation) in annotations {
            builder.add_annotation(page, annotation, Vec::new());
        }
        if let Some(lang) = &self.lang {
            builder.lang(lang);
            builder.structure(structure);
        }

        for (resource, slot) in &self.fonts {
            if let FontSlot::Embedded(slot) = slot {
                builder.embed_cid_font(resource, slot.cid_font());
            }
        }

        Ok(PreparedComposition { builder, trace })
    }
}

impl Default for Composition {
    fn default() -> Self {
        Self::new()
    }
}

/// Scoped editor for a composition page.
pub struct Page {
    root: Option<Node>,
    header: Option<Node>,
    footer: Option<Node>,
}

impl Page {
    fn new() -> Self {
        Self {
            root: None,
            header: None,
            footer: None,
        }
    }

    /// Edit the page's content region.
    pub fn content(&mut self) -> Container<'_> {
        Container {
            slot: &mut self.root,
        }
    }

    /// Edit the element tree repeated above the content on every physical page.
    pub fn header(&mut self) -> Container<'_> {
        Container {
            slot: &mut self.header,
        }
    }

    /// Edit the element tree repeated below the content on every physical page.
    pub fn footer(&mut self) -> Container<'_> {
        Container {
            slot: &mut self.footer,
        }
    }
}

/// Scoped editor for one layout-element slot.
pub struct Container<'a> {
    slot: &'a mut Option<Node>,
}

impl Container<'_> {
    /// Fill this slot with a vertical column.
    pub fn column(&mut self, configure: impl FnOnce(&mut Column<'_>)) {
        let mut children = Vec::new();
        let mut spacing = 0.0;
        {
            let mut column = Column {
                children: &mut children,
                spacing: &mut spacing,
            };
            configure(&mut column);
        }
        let children = children
            .into_iter()
            .map(|child| child.unwrap_or_else(Node::empty_column))
            .collect();
        *self.slot = Some(Node::Column(ColumnNode::new(children, spacing)));
    }

    /// Fill this slot with wrapping text.
    pub fn text(&mut self, text: &str, style: TextStyle) {
        *self.slot = Some(Node::Text(TextNode::new(text, style)));
    }

    /// Fill this slot with an indivisible horizontal row.
    pub fn row(&mut self, configure: impl FnOnce(&mut Row<'_>)) {
        let mut children = Vec::new();
        {
            let mut row = Row {
                children: &mut children,
            };
            configure(&mut row);
        }
        let children = children
            .into_iter()
            .map(|(width, child)| (width, child.unwrap_or_else(Node::empty_column)))
            .collect();
        *self.slot = Some(Node::Row(RowNode::new(children)));
    }

    /// Fill this slot with a paginating table whose cells contain layout-element trees.
    pub fn table(&mut self, configure: impl FnOnce(&mut ComposeTable<'_>)) {
        let mut columns = Vec::new();
        let mut header = None;
        let mut rows = Vec::new();
        {
            let mut table = ComposeTable {
                columns: &mut columns,
                header: &mut header,
                rows: &mut rows,
            };
            configure(&mut table);
        }
        *self.slot = Some(Node::Table(TableNode::new(columns, header, rows)));
    }

    /// Fill this slot with an indivisible image using the requested scaling policy.
    pub fn image(&mut self, image: &crate::Image, sizing: ImageSizing) {
        *self.slot = Some(Node::Image(ImageNode::new(image.clone(), sizing)));
    }

    /// Annotate a child with logical structure for tagged-PDF output.
    pub fn semantic(&mut self, semantic: Semantic, configure: impl FnOnce(&mut Container<'_>)) {
        let mut child = None;
        configure(&mut Container { slot: &mut child });
        *self.slot = Some(Node::Semantic(SemanticNode::new(
            semantic,
            child.unwrap_or_else(Node::empty_column),
        )));
    }

    /// Force following column content onto a fresh physical page.
    pub fn page_break(&mut self) {
        *self.slot = Some(Node::PageBreak(PageBreakNode { complete: false }));
    }

    /// Wrap a child in uniform padding.
    pub fn padding(&mut self, points: f64, configure: impl FnOnce(&mut Container<'_>)) {
        self.decorate(
            Decoration {
                padding: [points; 4],
                ..Decoration::default()
            },
            configure,
        );
    }

    /// Wrap a child and align it within the offered box.
    pub fn align(
        &mut self,
        horizontal: HorizontalAlign,
        vertical: VerticalAlign,
        configure: impl FnOnce(&mut Container<'_>),
    ) {
        self.decorate(
            Decoration {
                horizontal,
                vertical,
                extend_width: true,
                extend_height: vertical != VerticalAlign::Top,
                ..Decoration::default()
            },
            configure,
        );
    }

    /// Constrain a child to an exact outer width.
    pub fn width(&mut self, points: f64, configure: impl FnOnce(&mut Container<'_>)) {
        self.decorate(
            Decoration {
                width: Some(points),
                ..Decoration::default()
            },
            configure,
        );
    }

    /// Constrain a child to an exact outer height.
    pub fn height(&mut self, points: f64, configure: impl FnOnce(&mut Container<'_>)) {
        self.decorate(
            Decoration {
                height: Some(points),
                ..Decoration::default()
            },
            configure,
        );
    }

    /// Extend a child to consume all offered width and height.
    pub fn extend(&mut self, configure: impl FnOnce(&mut Container<'_>)) {
        self.decorate(
            Decoration {
                extend_width: true,
                extend_height: true,
                ..Decoration::default()
            },
            configure,
        );
    }

    /// Paint a border around a child.
    pub fn border(&mut self, width: f64, color: Color, configure: impl FnOnce(&mut Container<'_>)) {
        self.decorate(
            Decoration {
                border: Some((width, color)),
                ..Decoration::default()
            },
            configure,
        );
    }

    /// Paint a background behind a child.
    pub fn background(&mut self, color: Color, configure: impl FnOnce(&mut Container<'_>)) {
        self.decorate(
            Decoration {
                background: Some(color),
                ..Decoration::default()
            },
            configure,
        );
    }

    fn decorate(&mut self, decoration: Decoration, configure: impl FnOnce(&mut Container<'_>)) {
        let mut child = None;
        configure(&mut Container { slot: &mut child });
        *self.slot = Some(Node::Decorated(DecoratedNode::new(
            decoration,
            child.unwrap_or_else(Node::empty_column),
        )));
    }
}

/// Scoped editor for a vertical container.
pub struct Column<'a> {
    children: &'a mut Vec<Option<Node>>,
    spacing: &'a mut f64,
}

impl Column<'_> {
    /// Set space inserted between non-empty children.
    pub fn spacing(&mut self, spacing: f64) {
        *self.spacing = spacing;
    }

    /// Append and edit one child slot.
    #[allow(clippy::expect_used)] // `push` guarantees `last_mut`; no input-dependent invariant.
    pub fn item(&mut self) -> Container<'_> {
        self.children.push(None);
        let slot = self.children.last_mut().expect("a child was just pushed");
        Container { slot }
    }
}

/// Scoped editor for an indivisible horizontal container.
pub struct Row<'a> {
    children: &'a mut Vec<(RowWidth, Option<Node>)>,
}

/// Scoped editor for a composition table.
pub struct ComposeTable<'a> {
    columns: &'a mut Vec<RowWidth>,
    header: &'a mut Option<Vec<Option<Node>>>,
    rows: &'a mut Vec<Vec<Option<Node>>>,
}

impl ComposeTable<'_> {
    /// Add an exact-width column in points.
    pub fn fixed_column(&mut self, width: f64) {
        self.columns.push(RowWidth::Fixed(width));
    }

    /// Add a column receiving a weighted share of remaining width.
    pub fn relative_column(&mut self, factor: f64) {
        self.columns.push(RowWidth::Relative(factor));
    }

    /// Add a column sized to its widest measured cell on the current fragment.
    pub fn automatic_column(&mut self) {
        self.columns.push(RowWidth::Auto);
    }

    /// Define the header row repeated on every table fragment.
    pub fn header(&mut self, configure: impl FnOnce(&mut ComposeTableRow<'_>)) {
        let mut cells = Vec::new();
        configure(&mut ComposeTableRow { cells: &mut cells });
        *self.header = Some(cells);
    }

    /// Append one body row.
    pub fn row(&mut self, configure: impl FnOnce(&mut ComposeTableRow<'_>)) {
        let mut cells = Vec::new();
        configure(&mut ComposeTableRow { cells: &mut cells });
        self.rows.push(cells);
    }
}

/// Scoped editor for one table row.
pub struct ComposeTableRow<'a> {
    cells: &'a mut Vec<Option<Node>>,
}

impl ComposeTableRow<'_> {
    /// Append and edit one cell.
    #[allow(clippy::expect_used)] // `push` guarantees `last_mut`; no input-dependent invariant.
    pub fn cell(&mut self) -> Container<'_> {
        self.cells.push(None);
        let slot = self.cells.last_mut().expect("a cell was just pushed");
        Container { slot }
    }
}

impl Row<'_> {
    /// Append a child with an exact width in points.
    pub fn fixed(&mut self, width: f64) -> Container<'_> {
        self.push(RowWidth::Fixed(width))
    }

    /// Append a child receiving a weighted share of width left after fixed and automatic children.
    pub fn relative(&mut self, factor: f64) -> Container<'_> {
        self.push(RowWidth::Relative(factor))
    }

    /// Append a child using its measured natural width.
    pub fn auto(&mut self) -> Container<'_> {
        self.push(RowWidth::Auto)
    }

    #[allow(clippy::expect_used)] // `push` guarantees `last_mut`; no input-dependent invariant.
    fn push(&mut self, width: RowWidth) -> Container<'_> {
        self.children.push((width, None));
        let (_, slot) = self.children.last_mut().expect("a child was just pushed");
        Container { slot }
    }
}

#[derive(Clone, Copy)]
enum RowWidth {
    Fixed(f64),
    Relative(f64),
    Auto,
}

#[derive(Clone, Copy)]
struct Decoration {
    padding: [f64; 4],
    horizontal: HorizontalAlign,
    vertical: VerticalAlign,
    width: Option<f64>,
    height: Option<f64>,
    extend_width: bool,
    extend_height: bool,
    border: Option<(f64, Color)>,
    background: Option<Color>,
}

impl Default for Decoration {
    fn default() -> Self {
        Self {
            padding: [0.0; 4],
            horizontal: HorizontalAlign::Left,
            vertical: VerticalAlign::Top,
            width: None,
            height: None,
            extend_width: false,
            extend_height: false,
            border: None,
            background: None,
        }
    }
}

struct PageDefinition {
    style: PageStyle,
    root: Node,
    header: Option<Node>,
    footer: Option<Node>,
}

fn content_size(style: PageStyle) -> Result<Size, ComposeError> {
    let size = Size::new(
        style.size[0] - style.margins[0] - style.margins[1],
        style.size[1] - style.margins[2] - style.margins[3],
    );
    if !size.is_valid() || size.width <= 0.0 || size.height <= 0.0 {
        return Err(ComposeError::InvalidGeometry);
    }
    Ok(size)
}

fn measure_repeating(
    node: Option<&mut Node>,
    available: Size,
    metrics: &Metrics,
    page: usize,
    pages: usize,
) -> Result<Size, ComposeError> {
    let Some(node) = node else {
        return Ok(Size::default());
    };
    node.reset();
    node.set_page_numbers(page, pages);
    match node.measure(available, metrics)? {
        Plan::Empty => Ok(Size::default()),
        Plan::Full(size) if size.is_valid() && size.width <= available.width + EPSILON => Ok(size),
        Plan::Full(_) => Err(ComposeError::MeasurementMismatch),
        Plan::Partial(_) | Plan::Wrap => Err(ComposeError::OverTallElement),
    }
}

fn body_size(content: Size, header: Size, footer: Size) -> Result<Size, ComposeError> {
    let height = content.height - header.height - footer.height;
    if !height.is_finite() || height <= 0.0 {
        return Err(ComposeError::OverTallElement);
    }
    Ok(Size::new(content.width, height))
}

fn draw_repeating(
    node: Option<&mut Node>,
    context: &mut DrawCtx<'_>,
    size: Size,
) -> Result<(), ComposeError> {
    if let Some(node) = node {
        if size != Size::default() {
            node.draw(context, size)?;
        }
    }
    Ok(())
}

fn preflight_pages(
    definition: &mut PageDefinition,
    metrics: &Metrics,
) -> Result<usize, ComposeError> {
    let content_area = content_size(definition.style)?;
    let header = measure_repeating(definition.header.as_mut(), content_area, metrics, 1, 1)?;
    let footer = measure_repeating(definition.footer.as_mut(), content_area, metrics, 1, 1)?;
    let available = body_size(content_area, header, footer)?;
    definition.root.reset();
    let mut pages = 0usize;
    loop {
        let plan = definition.root.measure(available, metrics)?;
        let measured = match checked_size(plan, definition.root.has_remaining())? {
            None if pages > 0 => break,
            None => Size::default(),
            Some(size) => size,
        };
        let mut content = Content::new();
        let mut images = Vec::new();
        let mut mcid_next = 0;
        let mut marked_depth = 0;
        let mut annotations = Vec::new();
        let mut trace = GeometryTrace::default();
        let mut context = DrawCtx {
            content: &mut content,
            metrics,
            trace: &mut trace,
            page: pages,
            page_height: definition.style.size[1],
            origin: Point::default(),
            images: &mut images,
            tagged: false,
            mcid_next: &mut mcid_next,
            annotations: &mut annotations,
            marked_depth: &mut marked_depth,
        };
        if !matches!(plan, Plan::Empty) {
            definition.root.draw(&mut context, measured)?;
        }
        pages += 1;
        if matches!(plan, Plan::Full(_) | Plan::Empty) {
            break;
        }
    }
    definition.root.reset();
    Ok(pages)
}

fn checked_size(plan: Plan, has_remaining: bool) -> Result<Option<Size>, ComposeError> {
    match plan {
        Plan::Empty => Ok(None),
        Plan::Wrap => Err(ComposeError::OverTallElement),
        Plan::Full(size) | Plan::Partial(size) => {
            if !size.is_valid() {
                return Err(ComposeError::InvalidGeometry);
            }
            if size.height <= EPSILON && has_remaining {
                return Err(ComposeError::NoProgress);
            }
            Ok(Some(size))
        }
    }
}

mod engine;
use engine::*;

#[cfg(test)]
#[path = "compose/tests.rs"]
mod tests;
