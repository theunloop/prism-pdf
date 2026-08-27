//! Multi-page text flow (ISO 32000-1 §7.7 + §9.4): pour wrapped, aligned text down a page and
//! start a new one automatically when it runs past the bottom margin.
//!
//! [`Flow`] is the document-level companion to [`draw_text_block`](crate::draw_text_block): where
//! that lays out one block on one page, `Flow` tracks a cursor across pages, breaking paragraphs
//! across page boundaries, and finally assembles the whole document via the writer.

use std::collections::BTreeMap;

use pdf_content::Content;
use pdf_document::{
    Builder, CidFont, ImageXObject, ListNumbering, PDF2_STRUCT_NS, PageSpec, StdFont, StructElem,
    ThScope,
};
use pdf_fonts::{FontInfo, shape_text};

use crate::image::Image;
use crate::metrics::{EmbeddedMetrics, FontMetrics, wrap_paragraph_with};
use crate::table::Table;
use crate::text::{TextBlock, line_layout, measure_text, wrap_paragraph, wrap_text};

/// A page's named image XObject resources.
type PageImages = Vec<(String, ImageXObject)>;
/// An accumulated page: content-stream bytes, image resources, and embedded-font names referenced.
type FinishedPage = (Vec<u8>, PageImages, Vec<String>);

/// A registered embedded font and the glyphs used so far (glyph id → advance + source character).
struct EmbeddedSlot {
    resource: String,
    program: Vec<u8>,
    info: FontInfo,
    used: BTreeMap<u16, (u16, char)>,
}

/// Page geometry for a [`Flow`]: the media box size and its four margins, in points.
#[derive(Clone, Copy, Debug)]
pub struct PageStyle {
    /// `[width, height]` of the page (the media box), in points.
    pub size: [f64; 2],
    /// `[left, right, top, bottom]` margins, in points.
    pub margins: [f64; 4],
}

impl PageStyle {
    /// US Letter (612 × 792 pt) with the given uniform margin.
    #[must_use]
    pub fn letter(margin: f64) -> Self {
        PageStyle {
            size: [612.0, 792.0],
            margins: [margin; 4],
        }
    }

    /// ISO A4 (595 × 842 pt) with the given uniform margin.
    #[must_use]
    pub fn a4(margin: f64) -> Self {
        PageStyle {
            size: [595.276, 841.89],
            margins: [margin; 4],
        }
    }

    fn left(&self) -> f64 {
        self.margins[0]
    }
    fn width(&self) -> f64 {
        self.size[0] - self.margins[0] - self.margins[1]
    }
    fn top(&self) -> f64 {
        self.size[1] - self.margins[2]
    }
    fn bottom(&self) -> f64 {
        self.margins[3]
    }
}

impl Default for PageStyle {
    fn default() -> Self {
        PageStyle::letter(72.0)
    }
}

/// The marker style for [`Flow::list`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ListStyle {
    /// A bullet (`•`) before each item.
    Bullet,
    /// A `1.`, `2.`, … decimal number before each item.
    Numbered,
}

/// A running header or footer drawn on every page in the margin (Standard-14 fonts only). The text
/// may contain `{page}` (current 1-based page number) and `{pages}` (total page count) placeholders,
/// substituted per page at [`Flow::build`] time.
struct RunningText {
    font_resource: String,
    base_font: String,
    size: f64,
    align: crate::text::Align,
    text: String,
}

impl RunningText {
    fn new(block: &TextBlock, text: &str) -> Self {
        RunningText {
            font_resource: block.font_resource.to_string(),
            base_font: block.base_font.to_string(),
            size: block.size,
            align: block.align,
            text: text.to_string(),
        }
    }

    /// Render this running text at baseline `y` for `page` of `pages` within `style`'s content
    /// width, returning content-stream bytes (Standard-14, WinAnsi-encoded).
    fn render(&self, style: &PageStyle, y: f64, page: usize, pages: usize) -> Vec<u8> {
        let text = self
            .text
            .replace("{page}", &page.to_string())
            .replace("{pages}", &pages.to_string());
        let width = style.width();
        let line_w = measure_text(&self.base_font, &text, self.size).unwrap_or(0.0);
        let (dx, _) = line_layout(self.align, &text, line_w, width, true);
        let x = style.left() + dx;
        let mut c = Content::new();
        c.begin_text();
        c.set_font(&self.font_resource, self.size);
        c.set_text_matrix(1.0, 0.0, 0.0, 1.0, x, y);
        c.show_text(&pdf_fonts::winansi_encode(&text));
        c.end_text();
        c.into_bytes()
    }
}

/// A multi-page text flow. Register the fonts available on every page, pour [`Flow::text`] (and
/// [`Flow::space`] / [`Flow::page_break`]) into it, then [`Flow::build`] the PDF bytes.
pub struct Flow {
    style: PageStyle,
    fonts: Vec<(String, StdFont)>,
    info: Vec<(String, String)>,
    embedded: Vec<EmbeddedSlot>,
    finished: Vec<FinishedPage>,
    bookmarks: Vec<(String, usize)>,
    header: Option<RunningText>,
    footer: Option<RunningText>,
    current: Content,
    current_images: PageImages,
    current_embedded: Vec<String>,
    cursor_y: f64,
    open: bool,
    /// Tagged-PDF state (§14.7/§14.8): when `tagged`, drawn content is wrapped in marked content and
    /// a flat structure tree is accumulated. `mcid_next` is the per-page MCID counter (reset on page
    /// break); `structure` collects the elements; `lang` is the document language.
    tagged: bool,
    lang: Option<String>,
    mcid_next: u32,
    structure: Vec<StructElem>,
    /// Whether shaping ever fell back to `.notdef` (GID 0) — a character the embedded font
    /// lacks. Reported to the [`Builder`] so the PDF/UA passes can reject the document
    /// (14289-1 §7.21.8 / 14289-2 §8.4.5.9).
    notdef_used: bool,
}

impl Flow {
    /// Start a flow with the given page geometry and the Standard-14 fonts to expose on every page
    /// (each `(resource name, font)` — the resource name is what a [`TextBlock::font_resource`]
    /// refers to).
    #[must_use]
    pub fn new(style: PageStyle, fonts: &[(&str, StdFont)]) -> Self {
        Flow {
            style,
            fonts: fonts.iter().map(|(n, f)| ((*n).to_string(), *f)).collect(),
            info: Vec::new(),
            embedded: Vec::new(),
            finished: Vec::new(),
            bookmarks: Vec::new(),
            header: None,
            footer: None,
            current: Content::new(),
            current_images: Vec::new(),
            current_embedded: Vec::new(),
            cursor_y: style.top(),
            open: false,
            tagged: false,
            lang: None,
            mcid_next: 0,
            structure: Vec::new(),
            notdef_used: false,
        }
    }

    /// Produce a **Tagged PDF** (§14.7/§14.8): wrap drawn content in marked content and emit a logical
    /// structure tree, setting the document language to `lang` (e.g. `"en-US"`). Call this before
    /// drawing. For M14.1 the structure is flat — each paragraph (and list item) becomes a `P`
    /// element; running headers/footers, table rules and images are marked as artifacts (images are
    /// promoted to `Figure` with alternate text in a later phase).
    pub fn tagged(&mut self, lang: &str) -> &mut Self {
        self.tagged = true;
        self.lang = Some(lang.to_string());
        self
    }

    /// Register a TrueType/OpenType font (the program bytes) for embedding under `resource`, the
    /// `/Fn` name a [`TextBlock::font_resource`] then refers to. The font is embedded as a composite
    /// (Type0/`Identity-H`) font, so [`Flow::text`] with it can show any glyph the font contains
    /// (e.g. non-Latin scripts), and the text still extracts via a generated `/ToUnicode`. Returns
    /// `false` (and registers nothing) if the bytes are not a valid font. Justified alignment falls
    /// back to left for embedded fonts.
    pub fn embed_font(&mut self, resource: &str, program: &[u8]) -> bool {
        let Some(info) = pdf_fonts::font_info(program) else {
            return false;
        };
        if !self.embedded.iter().any(|s| s.resource == resource) {
            self.embedded.push(EmbeddedSlot {
                resource: resource.to_string(),
                program: program.to_vec(),
                info,
                used: BTreeMap::new(),
            });
        }
        true
    }

    fn embedded_index(&self, resource: &str) -> Option<usize> {
        self.embedded.iter().position(|s| s.resource == resource)
    }

    /// Set a document information entry (`/Info`, §14.3.3), e.g. `("Title", "...")`.
    pub fn info(&mut self, key: &str, value: &str) -> &mut Self {
        self.info.push((key.to_string(), value.to_string()));
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

    /// Draw `text` as a running header (in the top margin) on every page, set in `block`'s
    /// Standard-14 font/size and aligned by `block.align` across the content width. `{page}` and
    /// `{pages}` in `text` are replaced per page with the page number and total page count. The
    /// font resource must be one registered with [`Flow::new`]. Embedded fonts are not supported
    /// for running text.
    pub fn header(&mut self, block: &TextBlock, text: &str) -> &mut Self {
        self.header = Some(RunningText::new(block, text));
        self
    }

    /// Draw `text` as a running footer (in the bottom margin) on every page. See [`Flow::header`]
    /// for the placeholder and font rules; a `"Page {page} of {pages}"` footer is the common case.
    pub fn footer(&mut self, block: &TextBlock, text: &str) -> &mut Self {
        self.footer = Some(RunningText::new(block, text));
        self
    }

    /// Add a document outline entry (bookmark, §12.3.3) titled `title` pointing at the page being
    /// flowed right now — call it just before the heading it marks. The bookmarks appear in the
    /// reader's outline panel in call order.
    pub fn bookmark(&mut self, title: &str) -> &mut Self {
        self.ensure_open();
        let page_index = self.finished.len(); // the in-progress page's eventual index
        self.bookmarks.push((title.to_string(), page_index));
        self
    }

    /// Number of completed pages plus the one in progress (always ≥ 1 once anything is drawn).
    #[must_use]
    pub fn page_count(&self) -> usize {
        self.finished.len() + usize::from(self.open)
    }

    /// The baseline `y` where the next line will be drawn on the current page.
    #[must_use]
    pub fn cursor_y(&self) -> f64 {
        self.cursor_y
    }

    /// Pour `text` into the flow with `block`'s font/size/leading/alignment, wrapping to the content
    /// width and breaking onto new pages as needed. Existing newlines are paragraph breaks. The font
    /// is `block.font_resource`: an embedded font (registered via [`Flow::embed_font`]) if one matches
    /// that name, otherwise the Standard-14 font named by `block.base_font`.
    pub fn text(&mut self, block: &TextBlock, text: &str) -> &mut Self {
        let width = self.style.width();
        let emb = self.embedded_index(block.font_resource);
        for paragraph in text.split('\n') {
            let lines = match emb {
                Some(idx) => self.wrap_embedded(idx, paragraph, block.size, width),
                None => wrap_paragraph(block.base_font, paragraph, block.size, width),
            };
            let last = lines.len().saturating_sub(1);
            let mut marks = Vec::new();
            for (i, line) in lines.iter().enumerate() {
                if let Some(mark) = self.place_line(block, line, width, i == last, emb, "P") {
                    marks.push(mark);
                }
            }
            self.record_block("P", marks); // one paragraph → a P element (spanning any pages)
        }
        self
    }

    /// Pour `text` as a heading of `level` 1–6 (clamped) with `block`'s font/size — like
    /// [`Flow::text`] but tagged `H1`–`H6` (§14.8.4.2) when the flow is tagged. Use a larger/bolder
    /// `block` to make it look like a heading; the structure level is independent of the visual size.
    pub fn heading(&mut self, level: u8, block: &TextBlock, text: &str) -> &mut Self {
        let width = self.style.width();
        let emb = self.embedded_index(block.font_resource);
        let tag = format!("H{}", level.clamp(1, 6));
        let mut marks = Vec::new();
        for paragraph in text.split('\n') {
            let lines = match emb {
                Some(idx) => self.wrap_embedded(idx, paragraph, block.size, width),
                None => wrap_paragraph(block.base_font, paragraph, block.size, width),
            };
            let last = lines.len().saturating_sub(1);
            for (i, line) in lines.iter().enumerate() {
                if let Some(mark) = self.place_line(block, line, width, i == last, emb, &tag) {
                    marks.push(mark);
                }
            }
        }
        self.record_block(&tag, marks); // the whole heading → one H{level} element
        self
    }

    /// Render `items` as a bulleted or numbered list with `block`'s font/size/leading: each item is
    /// wrapped to the content width minus a hanging indent, its marker drawn at the left margin and
    /// its text at the indent; long items and the list as a whole break across pages.
    pub fn list(&mut self, block: &TextBlock, items: &[&str], style: ListStyle) -> &mut Self {
        let emb = self.embedded_index(block.font_resource);
        let indent = block.size * 1.6;
        let text_width = (self.style.width() - indent).max(1.0);
        // /ListNumbering describes the marker scheme (§14.8.5.5); PDF/UA-1 §7.6 requires it on
        // ordered lists — Disc matches the bullet marker, Decimal the "1." numbering below.
        let mut list_elem = StructElem::new("L").list_numbering(match style {
            ListStyle::Bullet => ListNumbering::Disc,
            ListStyle::Numbered => ListNumbering::Decimal,
        });
        for (i, item) in items.iter().enumerate() {
            let marker = match style {
                ListStyle::Bullet => "\u{2022}".to_string(),
                ListStyle::Numbered => format!("{}.", i + 1),
            };
            let lines = match emb {
                Some(idx) => self.wrap_embedded(idx, item, block.size, text_width),
                None => wrap_paragraph(block.base_font, item, block.size, text_width),
            };
            // A list item is LI = Lbl (marker) + LBody (text), §14.8.4.3.
            let mut label_marks = Vec::new();
            let mut body_marks = Vec::new();
            for (j, line) in lines.iter().enumerate() {
                self.ensure_open();
                if self.cursor_y < self.style.bottom() {
                    self.page_break();
                }
                let y = self.cursor_y;
                let left = self.style.left();
                if j == 0 {
                    if let Some(mark) = self.draw_run(block, &marker, left, y, emb, "Lbl") {
                        label_marks.push(mark);
                    }
                }
                if let Some(mark) = self.draw_run(block, line, left + indent, y, emb, "LBody") {
                    body_marks.push(mark);
                }
                self.cursor_y -= block.leading;
            }
            if self.tagged {
                let mut li = StructElem::new("LI");
                if let Some(label) = Self::element_from("Lbl", label_marks) {
                    li.push_child(label);
                }
                if let Some(body) = Self::element_from("LBody", body_marks) {
                    li.push_child(body);
                }
                if !li.kids.is_empty() {
                    list_elem.push_child(li);
                }
            }
        }
        if self.tagged && !list_elem.kids.is_empty() {
            self.structure.push(list_elem); // the whole list → one L element
        }
        self
    }

    /// Render `table` into the flow at the current cursor, breaking rows onto new pages as needed
    /// (repeating the header row when [`Table::header_row`] is set). Column widths are scaled to the
    /// content width. The cursor is left just below the table.
    pub fn table(&mut self, table: &Table) -> &mut Self {
        self.ensure_open();
        let width = self.style.width();
        let total: f64 = table.columns.iter().sum();
        if total <= 0.0 || table.rows.is_empty() {
            return self;
        }
        let cols: Vec<f64> = table.columns.iter().map(|w| w * width / total).collect();
        let laid: Vec<(Vec<Vec<String>>, f64)> = table
            .rows
            .iter()
            .map(|r| layout_row(r, &cols, table))
            .collect();

        let mut table_elem = StructElem::new("Table");
        for (ri, (cells, row_h)) in laid.iter().enumerate() {
            // Break before a row that would cross the bottom margin — unless we are already at the
            // top of a fresh page (a row taller than the page simply overflows rather than looping).
            if self.cursor_y - row_h < self.style.bottom() && self.cursor_y < self.style.top() {
                self.page_break();
                if table.header && ri > 0 {
                    // The repeated header is a pagination artifact, not a second logical header row.
                    let (h_cells, h_h) = &laid[0];
                    self.draw_table_row(&cols, h_cells, *h_h, table, RowKind::RepeatedHeader);
                }
            }
            let kind = if table.header && ri == 0 {
                RowKind::Header
            } else {
                RowKind::Body
            };
            if let Some(tr) = self.draw_table_row(&cols, cells, *row_h, table, kind) {
                table_elem.push_child(tr);
            }
        }
        if self.tagged && !table_elem.kids.is_empty() {
            self.structure.push(table_elem); // the whole table → one Table element
        }
        self
    }

    /// Advance the cursor down by `dy` points (a vertical gap between blocks).
    pub fn space(&mut self, dy: f64) -> &mut Self {
        self.ensure_open();
        self.cursor_y -= dy;
        self
    }

    /// Finish the current page and start a fresh one.
    pub fn page_break(&mut self) -> &mut Self {
        self.ensure_open();
        let bytes = std::mem::take(&mut self.current).into_bytes();
        let images = std::mem::take(&mut self.current_images);
        let embedded = std::mem::take(&mut self.current_embedded);
        self.finished.push((bytes, images, embedded));
        self.cursor_y = self.style.top();
        self.mcid_next = 0; // MCIDs are per-page (§14.7.4.2)
        self
    }

    /// Place `image` at the cursor scaled to `width` × `height` points, breaking to a new page first
    /// if it would cross the bottom margin. The cursor drops to the image's bottom edge. In a tagged
    /// document this image is an **artifact** (decorative) — use [`Flow::figure`] for a meaningful
    /// image that needs alternate text.
    pub fn image(&mut self, image: &Image, width: f64, height: f64) -> &mut Self {
        self.emit_image(image, width, height, None);
        self
    }

    /// Place `image` scaled to fit `max_width` points wide (preserving aspect ratio), as an artifact.
    pub fn image_fit(&mut self, image: &Image, max_width: f64) -> &mut Self {
        let (width, height) = self.fit(image, max_width);
        self.image(image, width, height)
    }

    /// Place `image` at `width` × `height` as a tagged **`Figure`** (§14.8.4.4) carrying `alt` as its
    /// alternate text (`/Alt`, §14.8.5) — the accessible form for a meaningful image. Without
    /// [`Flow::tagged`] this behaves like [`Flow::image`].
    pub fn figure(&mut self, image: &Image, width: f64, height: f64, alt: &str) -> &mut Self {
        self.emit_image(image, width, height, Some(alt));
        self
    }

    /// [`Flow::figure`] scaled to fit `max_width` points wide (preserving aspect ratio).
    pub fn figure_fit(&mut self, image: &Image, max_width: f64, alt: &str) -> &mut Self {
        let (width, height) = self.fit(image, max_width);
        self.figure(image, width, height, alt)
    }

    /// [`Flow::figure`] followed by a caption paragraph in `block`'s font/size. In a tagged
    /// document the caption becomes a **`Caption`** element (§14.8.4.4) nested in the `Figure` —
    /// PDF/UA-1 §7.3 requires a caption accompanying a figure to be tagged as such.
    pub fn figure_with_caption(
        &mut self,
        image: &Image,
        width: f64,
        height: f64,
        alt: &str,
        block: &TextBlock,
        caption: &str,
    ) -> &mut Self {
        self.emit_image(image, width, height, Some(alt));
        let marks = self.pour_block(block, caption, "Caption");
        if let Some(caption_elem) = Self::element_from("Caption", marks) {
            if let Some(figure) = self.structure.last_mut().filter(|e| e.tag == "Figure") {
                figure.push_child(caption_elem);
            }
        }
        self
    }

    /// Pour `text` as a **`Note`** element (§14.8.4.4 — a footnote/endnote/citation) carrying the
    /// element identifier `id` (`/ID`, listed in the `/StructTreeRoot /IDTree`) — PDF/UA-1 §7.9
    /// requires a unique `/ID` on every `Note`. Renders like [`Flow::text`], one element for the
    /// whole text. **PDF/UA-2 forbids `Note`** — author a [`Flow::fenote`] instead there.
    pub fn note(&mut self, block: &TextBlock, text: &str, id: &str) -> &mut Self {
        let marks = self.pour_block(block, text, "Note");
        if let Some(elem) = Self::element_from("Note", marks) {
            self.structure.push(elem.id(id));
        }
        self
    }

    /// Pour `text` as a **`FENote`** element (ISO 32000-2 §14.8.4.7.2 — the PDF 2.0 footnote/
    /// endnote type, in the PDF 2.0 structure namespace) with element identifier `id` and `/Ref`
    /// links back to the `citations` element IDs. PDF/UA-2 §8.2.5.14 replaces `Note` with
    /// `FENote` and wants note and citing content linked **bidirectionally**: give the citing
    /// element an `/ID` plus a reference to `id` (see [`Flow::last_element_mut`] /
    /// [`StructElem::reference`]), and list its ID here. Promotes the document to PDF 2.0.
    pub fn fenote(
        &mut self,
        block: &TextBlock,
        text: &str,
        id: &str,
        citations: &[&str],
    ) -> &mut Self {
        let marks = self.pour_block(block, text, "FENote");
        if let Some(elem) = Self::element_from("FENote", marks) {
            let mut elem = elem.id(id).namespace(PDF2_STRUCT_NS);
            for citation in citations {
                elem = elem.reference(*citation);
            }
            self.structure.push(elem);
        }
        self
    }

    /// Pour `text` as a **`Title`** element (ISO 32000-2 §14.8.4.4 — the PDF 2.0 document-title
    /// type, distinct from a heading, in the PDF 2.0 structure namespace). PDF/UA-2 §8.2.5.13
    /// wants the content rendering a document's title tagged `Title`, not `H1`. Promotes the
    /// document to PDF 2.0.
    pub fn title_element(&mut self, block: &TextBlock, text: &str) -> &mut Self {
        let marks = self.pour_block(block, text, "Title");
        if let Some(elem) = Self::element_from("Title", marks) {
            self.structure.push(elem.namespace(PDF2_STRUCT_NS));
        }
        self
    }

    /// Pour `text` (the formula rendered as plain text, e.g. `"E = mc2"`) as a **`Formula`**
    /// element (§14.8.4.4) with `actual_text` as its `/ActualText` replacement (§14.9.4). For
    /// PDF/UA-2 §8.2.5.29 attach presentation MathML as an associated file with `AFRelationship`
    /// `Supplement` on the returned element ([`Flow::last_element_mut`] → push into `af`), or tag
    /// MathML sub-elements in the MathML namespace.
    pub fn formula(&mut self, block: &TextBlock, text: &str, actual_text: &str) -> &mut Self {
        let marks = self.pour_block(block, text, "Formula");
        if let Some(elem) = Self::element_from("Formula", marks) {
            self.structure.push(elem.actual_text(actual_text));
        }
        self
    }

    /// The most recently pushed top-level structure element, for post-hoc customisation — e.g.
    /// giving the paragraph that cites a [`Flow::fenote`] its `/ID` and `/Ref`
    /// ([`StructElem::id`] fields are public):
    ///
    /// ```ignore
    /// flow.text(&body, "As shown by the data [1].");
    /// if let Some(p) = flow.last_element_mut() {
    ///     p.id = Some("cite-1".into());
    ///     p.refs.push("fn-1".into());
    /// }
    /// flow.fenote(&small, "[1] The measurement details.", "fn-1", &["cite-1"]);
    /// ```
    pub fn last_element_mut(&mut self) -> Option<&mut StructElem> {
        self.structure.last_mut()
    }

    /// Wrap and place `text` line by line tagged `tag`, returning the `(page, mcid)` marks — the
    /// shared machinery of [`Flow::figure_with_caption`] and [`Flow::note`].
    fn pour_block(&mut self, block: &TextBlock, text: &str, tag: &str) -> Vec<(usize, u32)> {
        let width = self.style.width();
        let emb = self.embedded_index(block.font_resource);
        let mut marks = Vec::new();
        for paragraph in text.split('\n') {
            let lines = match emb {
                Some(idx) => self.wrap_embedded(idx, paragraph, block.size, width),
                None => wrap_paragraph(block.base_font, paragraph, block.size, width),
            };
            let last = lines.len().saturating_sub(1);
            for (i, line) in lines.iter().enumerate() {
                if let Some(mark) = self.place_line(block, line, width, i == last, emb, tag) {
                    marks.push(mark);
                }
            }
        }
        marks
    }

    /// `width` × `height` for `image` fit to `max_width` (clamped to the content width), aspect kept.
    fn fit(&self, image: &Image, max_width: f64) -> (f64, f64) {
        let (iw, ih) = (f64::from(image.width()), f64::from(image.height()));
        let width = max_width.min(self.style.width());
        let height = if iw > 0.0 { width * ih / iw } else { 0.0 };
        (width, height)
    }

    /// Emit an image XObject at the cursor. When tagged: `alt = Some` makes it a `Figure` structure
    /// element with `/Alt`; `alt = None` makes it an artifact (§14.8.2.2).
    fn emit_image(&mut self, image: &Image, width: f64, height: f64, alt: Option<&str>) {
        self.ensure_open();
        if self.cursor_y - height < self.style.bottom() && self.cursor_y < self.style.top() {
            self.page_break();
        }
        let name = format!("Im{}", self.current_images.len());
        self.current_images
            .push((name.clone(), image.xobject.clone()));
        let x = self.style.left();
        let bottom = self.cursor_y - height;

        let mark = match alt {
            Some(_) => self.begin_struct("Figure"),
            None => {
                self.begin_artifact();
                None
            }
        };
        {
            let c = &mut self.current;
            c.save();
            c.transform(width, 0.0, 0.0, height, x, bottom);
            c.do_xobject(&name);
            c.restore();
        }
        self.end_marked();

        if let (Some(alt), Some(mark)) = (alt, mark) {
            let mut figure = StructElem::new("Figure").alt(alt);
            figure.push_content(mark.0, mark.1);
            self.structure.push(figure);
        }

        self.cursor_y = bottom;
    }

    fn ensure_open(&mut self) {
        if !self.open {
            self.cursor_y = self.style.top();
            self.open = true;
        }
    }

    // --- Tagged-PDF helpers (§14.7/§14.8); all no-ops when `tagged` is false ---

    /// If tagging is on, open a structure marked-content sequence (`/<tag> <</MCID n>> BDC`) on the
    /// current page and return its `(page_index, mcid)`; pair with [`Flow::end_marked`].
    fn begin_struct(&mut self, tag: &str) -> Option<(usize, u32)> {
        if !self.tagged {
            return None;
        }
        let mcid = self.mcid_next;
        self.mcid_next += 1;
        let page_index = self.finished.len(); // the in-progress page's eventual index
        self.current.begin_marked_content(tag, mcid);
        Some((page_index, mcid))
    }

    /// If tagging is on, open an artifact sequence (`/Artifact BMC`); pair with [`Flow::end_marked`].
    fn begin_artifact(&mut self) {
        if self.tagged {
            self.current.begin_artifact();
        }
    }

    /// If tagging is on, close the innermost marked-content sequence (`EMC`).
    fn end_marked(&mut self) {
        if self.tagged {
            self.current.end_marked_content();
        }
    }

    /// Build a structure element of type `tag` from the marked content of one logical block. Each
    /// `(page, mcid)` becomes a content child, so a block that spilled across a page break is one
    /// element spanning pages (§14.7.4.3). Returns `None` if there was no marked content.
    fn element_from(tag: &str, marks: Vec<(usize, u32)>) -> Option<StructElem> {
        if marks.is_empty() {
            return None;
        }
        let mut elem = StructElem::new(tag);
        for (page, mcid) in marks {
            elem.push_content(page, mcid);
        }
        Some(elem)
    }

    /// Record a top-level block (paragraph, heading) as one structure element under the `Document`
    /// root.
    fn record_block(&mut self, tag: &str, marks: Vec<(usize, u32)>) {
        if let Some(elem) = Self::element_from(tag, marks) {
            self.structure.push(elem);
        }
    }

    /// Wrap a paragraph for an embedded font (measuring via the font's own advances).
    fn wrap_embedded(&self, idx: usize, paragraph: &str, size: f64, max_width: f64) -> Vec<String> {
        let slot = &self.embedded[idx];
        wrap_paragraph_with(
            &EmbeddedMetrics::new(&slot.program, &slot.info),
            paragraph,
            size,
            max_width,
        )
    }

    /// Draw one already-wrapped line, breaking to a new page first if it would fall below the
    /// bottom margin. `emb` selects an embedded font (2-byte glyph codes) over the Standard-14 path.
    fn place_line(
        &mut self,
        block: &TextBlock,
        line: &str,
        width: f64,
        is_last: bool,
        emb: Option<usize>,
        tag: &str,
    ) -> Option<(usize, u32)> {
        self.ensure_open();
        if self.cursor_y < self.style.bottom() {
            self.page_break();
        }
        let mark = self.begin_struct(tag);
        let y = self.cursor_y;
        match emb {
            Some(idx) => {
                let glyphs = shape_text(&self.embedded[idx].program, line).unwrap_or_default();
                let line_w =
                    EmbeddedMetrics::new(&self.embedded[idx].program, &self.embedded[idx].info)
                        .width(line, block.size)
                        .unwrap_or(0.0);
                // Justification widens byte-32 spaces (Tw), which an Identity-H string has none of —
                // so treat embedded text as non-justified (left) by passing is_last = true.
                let (dx, _) = line_layout(block.align, line, line_w, width, true);
                for g in &glyphs {
                    if g.id == 0 {
                        // The font has no glyph for this character: the shown GID would be
                        // `.notdef` — remembered so the PDF/UA passes can reject the document.
                        self.notdef_used = true;
                    }
                    self.embedded[idx]
                        .used
                        .entry(g.id)
                        .or_insert((g.advance, g.ch));
                }
                if !self
                    .current_embedded
                    .iter()
                    .any(|r| r == block.font_resource)
                {
                    self.current_embedded.push(block.font_resource.to_string());
                }
                let gids: Vec<u16> = glyphs.iter().map(|g| g.id).collect();
                let x = self.style.left() + dx;
                let c = &mut self.current;
                c.begin_text();
                c.set_font(block.font_resource, block.size);
                c.set_text_matrix(1.0, 0.0, 0.0, 1.0, x, y);
                c.show_glyphs(&gids);
                c.end_text();
            }
            None => {
                let line_w = measure_text(block.base_font, line, block.size).unwrap_or(0.0);
                let (dx, word_space) = line_layout(block.align, line, line_w, width, is_last);
                let x = self.style.left() + dx;
                let bytes = pdf_fonts::winansi_encode(line);
                let c = &mut self.current;
                c.begin_text();
                c.set_font(block.font_resource, block.size);
                if word_space != 0.0 {
                    c.set_word_spacing(word_space);
                }
                c.set_text_matrix(1.0, 0.0, 0.0, 1.0, x, y);
                c.show_text(&bytes);
                c.end_text();
            }
        }
        self.end_marked();
        self.cursor_y -= block.leading;
        mark
    }

    /// Draw a single run of text at an explicit `(x, y)` baseline, without touching the cursor —
    /// used for list markers and items. Font-kind aware (Standard-14 WinAnsi vs embedded glyphs).
    fn draw_run(
        &mut self,
        block: &TextBlock,
        text: &str,
        x: f64,
        y: f64,
        emb: Option<usize>,
        tag: &str,
    ) -> Option<(usize, u32)> {
        let mark = self.begin_struct(tag);
        match emb {
            Some(idx) => {
                let glyphs = shape_text(&self.embedded[idx].program, text).unwrap_or_default();
                for g in &glyphs {
                    if g.id == 0 {
                        // The font has no glyph for this character: the shown GID would be
                        // `.notdef` — remembered so the PDF/UA passes can reject the document.
                        self.notdef_used = true;
                    }
                    self.embedded[idx]
                        .used
                        .entry(g.id)
                        .or_insert((g.advance, g.ch));
                }
                if !self
                    .current_embedded
                    .iter()
                    .any(|r| r == block.font_resource)
                {
                    self.current_embedded.push(block.font_resource.to_string());
                }
                let gids: Vec<u16> = glyphs.iter().map(|g| g.id).collect();
                let c = &mut self.current;
                c.begin_text();
                c.set_font(block.font_resource, block.size);
                c.set_text_matrix(1.0, 0.0, 0.0, 1.0, x, y);
                c.show_glyphs(&gids);
                c.end_text();
            }
            None => {
                let bytes = pdf_fonts::winansi_encode(text);
                let c = &mut self.current;
                c.begin_text();
                c.set_font(block.font_resource, block.size);
                c.set_text_matrix(1.0, 0.0, 0.0, 1.0, x, y);
                c.show_text(&bytes);
                c.end_text();
            }
        }
        self.end_marked();
        mark
    }

    /// Draw one table row (its cells already wrapped) of height `row_h` at the cursor, with grid
    /// borders, then lower the cursor to the row's bottom edge. When tagged, returns the row's `TR`
    /// structure element (its cells as `TH`/`TD` per `kind`); a [`RowKind::RepeatedHeader`] is drawn
    /// as an artifact and returns `None`.
    fn draw_table_row(
        &mut self,
        cols: &[f64],
        cells: &[Vec<String>],
        row_h: f64,
        t: &Table,
        kind: RowKind,
    ) -> Option<StructElem> {
        let left = self.style.left();
        let top = self.cursor_y;
        let bottom = top - row_h;

        // The grid borders are a layout artifact (§14.8.2.2), excluded from the structure.
        if t.border > 0.0 {
            self.begin_artifact();
            let c = &mut self.current;
            c.set_line_width(t.border);
            let mut x = left;
            for &cw in cols {
                c.rect(x, bottom, cw, row_h);
                x += cw;
            }
            c.stroke();
            self.end_marked();
        }

        let cell_tag = match kind {
            RowKind::Header => "TH",
            RowKind::Body => "TD",
            RowKind::RepeatedHeader => "",
        };
        let mut row = (kind != RowKind::RepeatedHeader).then(|| StructElem::new("TR"));

        let mut x = left;
        for (ci, lines) in cells.iter().enumerate() {
            let cw = cols[ci];
            let text_w = (cw - 2.0 * t.padding).max(1.0);
            // A repeated header is an artifact; a real cell opens a TH/TD marked-content sequence.
            let mark = match kind {
                RowKind::RepeatedHeader => {
                    self.begin_artifact();
                    None
                }
                _ => self.begin_struct(cell_tag),
            };
            {
                let c = &mut self.current;
                let mut baseline = top - t.padding - t.size; // first line's baseline inside the cell
                for line in lines {
                    let line_w = measure_text(&t.base_font, line, t.size).unwrap_or(0.0);
                    let (dx, _) = line_layout(t.align, line, line_w, text_w, true);
                    c.begin_text();
                    c.set_font(&t.font_resource, t.size);
                    c.set_text_matrix(1.0, 0.0, 0.0, 1.0, x + t.padding + dx, baseline);
                    c.show_text(&pdf_fonts::winansi_encode(line));
                    c.end_text();
                    baseline -= t.leading;
                }
            }
            self.end_marked();
            if let (Some(row), Some(mark)) = (row.as_mut(), mark) {
                let mut cell = StructElem::new(cell_tag);
                if kind == RowKind::Header {
                    // A header-row cell labels its column (/Scope /Column, §14.8.5.4 — PDF 1.5);
                    // PDF/UA-1 §7.5 wants a /Scope on every TH.
                    cell = cell.th_scope(ThScope::Column);
                }
                cell.push_content(mark.0, mark.1);
                row.push_child(cell);
            }
            x += cw;
        }

        self.cursor_y = bottom;
        row
    }
}

/// Which logical row a table row is, for tagging (§14.8.4.3).
#[derive(Clone, Copy, PartialEq, Eq)]
enum RowKind {
    /// The (first) header row — its cells become `TH`.
    Header,
    /// A body row — its cells become `TD`.
    Body,
    /// A header row repeated at the top of a continued page — a pagination artifact, not structure.
    RepeatedHeader,
}

/// Build a `CIDToGIDMap` byte array (§9.7.4.3) from an old→new glyph-ID remapping: indexed by CID
/// (= original glyph ID), two big-endian bytes giving the glyph's ID in the subsetted program.
fn cid_to_gid_map(map: &[(u16, u16)]) -> Vec<u8> {
    let max_cid = map.iter().map(|(old, _)| *old).max().unwrap_or(0);
    let mut bytes = vec![0u8; (max_cid as usize + 1) * 2];
    for &(old, new) in map {
        let i = old as usize * 2;
        bytes[i] = (new >> 8) as u8;
        bytes[i + 1] = new as u8;
    }
    bytes
}

/// Wrap each cell of `row` to its column's inner width and return the wrapped cells plus the row
/// height (the tallest cell drives the row, plus top and bottom padding).
fn layout_row(row: &[String], cols: &[f64], t: &Table) -> (Vec<Vec<String>>, f64) {
    let mut cells = Vec::with_capacity(cols.len());
    let mut max_lines = 1usize;
    for (ci, &cw) in cols.iter().enumerate() {
        let text = row.get(ci).map(String::as_str).unwrap_or("");
        let text_w = (cw - 2.0 * t.padding).max(1.0);
        let lines = wrap_text(&t.base_font, text, t.size, text_w);
        max_lines = max_lines.max(lines.len());
        cells.push(lines);
    }
    let row_h = max_lines as f64 * t.leading + 2.0 * t.padding;
    (cells, row_h)
}

mod build;

#[cfg(test)]
#[path = "flow/tests.rs"]
mod tests;
