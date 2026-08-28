use super::*;

//   too: after either call the handle is dead, exactly like `fclose`. Do not free it again.
// ---------------------------------------------------------------------------------------------

/// Horizontal alignment of a text block (§9.4.3).
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PrismPdfAlign {
    /// Ragged right.
    Left = 0,
    /// Centred.
    Center = 1,
    /// Ragged left.
    Right = 2,
    /// Flush both margins.
    Justify = 3,
}

impl From<PrismPdfAlign> for Align {
    fn from(value: PrismPdfAlign) -> Self {
        match value {
            PrismPdfAlign::Left => Align::Left,
            PrismPdfAlign::Center => Align::Center,
            PrismPdfAlign::Right => Align::Right,
            PrismPdfAlign::Justify => Align::Justify,
        }
    }
}

/// How list items are marked.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PrismPdfListStyle {
    /// A bullet before each item.
    Bullet = 0,
    /// An incrementing number before each item.
    Numbered = 1,
}

impl From<PrismPdfListStyle> for ListStyle {
    fn from(value: PrismPdfListStyle) -> Self {
        match value {
            PrismPdfListStyle::Bullet => ListStyle::Bullet,
            PrismPdfListStyle::Numbered => ListStyle::Numbered,
        }
    }
}

/// Text style: which font resource to use, at what size, leading and alignment. Owns its font
/// names, unlike the borrowing Rust `TextBlock`.
pub struct PrismPdfTextBlock {
    font_resource: String,
    base_font: String,
    size: f64,
    leading: f64,
    align: Align,
}

impl PrismPdfTextBlock {
    /// Materialise the borrowing view the layout functions take.
    fn view(&self) -> TextBlock<'_> {
        TextBlock {
            font_resource: &self.font_resource,
            base_font: &self.base_font,
            size: self.size,
            leading: self.leading,
            align: self.align,
        }
    }
}

/// Create a text style. `font_resource` is the name in the page's `/Resources /Font`;
/// `base_font` is the font's PostScript name, used for metrics.
///
/// # Safety
/// Both names must be NUL-terminated UTF-8 C strings. Release the handle with
/// [`prismpdf_text_block_free`]. Returns null on a null or non-UTF-8 argument.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_text_block_new(
    font_resource: *const c_char,
    base_font: *const c_char,
    size: f64,
    leading: f64,
    align: PrismPdfAlign,
) -> *mut PrismPdfTextBlock {
    // `utf8` already yields `None` for a null pointer, so it covers the null case too.
    let (Some(resource), Some(base)) = (unsafe { utf8(font_resource) }, unsafe { utf8(base_font) })
    else {
        return std::ptr::null_mut();
    };
    guard_ptr(|| {
        Box::into_raw(Box::new(PrismPdfTextBlock {
            font_resource: resource.to_string(),
            base_font: base.to_string(),
            size,
            leading,
            align: align.into(),
        }))
    })
}

/// Release a text style. Freeing `NULL` is a no-op.
///
/// # Safety
/// `block` must come from [`prismpdf_text_block_new`] and must not already be freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_text_block_free(block: *mut PrismPdfTextBlock) {
    unsafe { free_handle(block) }
}

/// Measure how wide `text` renders in the style's font at its size (§9.4), in points.
///
/// # Safety
/// `block` must be live, `text` a NUL-terminated UTF-8 C string, `out_width` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_measure_text(
    block: *const PrismPdfTextBlock,
    text: *const c_char,
    out_width: *mut f64,
) -> PrismPdfStatus {
    if out_width.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    unsafe { *out_width = 0.0 };
    if block.is_null() || text.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        let Some(text) = (unsafe { utf8(text) }) else {
            return PrismPdfStatus::NullArgument;
        };
        let style = unsafe { &*block };
        // `None` means the base font is not one of the Standard-14, so there are no built-in
        // metrics to measure with — a missing answer, not a failure.
        match measure_text(&style.base_font, text, style.size) {
            Some(width) => {
                unsafe { *out_width = width };
                PrismPdfStatus::Ok
            }
            None => PrismPdfStatus::NotFound,
        }
    })
}

/// Wrap `text` to `width` points, returning one string per line.
///
/// # Safety
/// `block` must be live, `text` a NUL-terminated UTF-8 C string, `out_list` writable. Release the
/// list with [`prismpdf_string_list_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_wrap_text(
    block: *const PrismPdfTextBlock,
    text: *const c_char,
    width: f64,
    out_list: *mut *mut PrismPdfStringList,
) -> PrismPdfStatus {
    if out_list.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    unsafe { *out_list = std::ptr::null_mut() };
    if block.is_null() || text.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        let Some(text) = (unsafe { utf8(text) }) else {
            return PrismPdfStatus::NullArgument;
        };
        let style = unsafe { &*block };
        let lines = wrap_text(&style.base_font, text, style.size, width);
        unsafe { *out_list = Box::into_raw(Box::new(PrismPdfStringList(lines))) };
        PrismPdfStatus::Ok
    })
}

// --- Images to place --------------------------------------------------------------------------

/// An image to be **placed** on a page — distinct from [`PrismPdfImage`], which is one *extracted*
/// from an existing document.
pub struct PrismPdfImageSource(pub(crate) Image);

/// Wrap a complete JPEG file, embedded verbatim as `DCTDecode` (§7.4.8). Returns null if the data
/// is not a usable JPEG.
///
/// # Safety
/// `data` must point to `len` readable bytes. Release the handle with
/// [`prismpdf_image_source_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_image_source_from_jpeg(
    data: *const u8,
    len: usize,
) -> *mut PrismPdfImageSource {
    if data.is_null() {
        return std::ptr::null_mut();
    }
    let bytes = unsafe { std::slice::from_raw_parts(data, len) }.to_vec();
    guard_ptr(|| match Image::from_jpeg(bytes) {
        Some(image) => Box::into_raw(Box::new(PrismPdfImageSource(image))),
        None => std::ptr::null_mut(),
    })
}

/// Wrap raw 8-bit interleaved RGB samples (`width * height * 3` bytes). Returns null on a length
/// mismatch.
///
/// # Safety
/// `data` must point to `len` readable bytes. Release the handle with
/// [`prismpdf_image_source_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_image_source_from_rgb(
    width: u32,
    height: u32,
    data: *const u8,
    len: usize,
) -> *mut PrismPdfImageSource {
    if data.is_null() {
        return std::ptr::null_mut();
    }
    let bytes = unsafe { std::slice::from_raw_parts(data, len) }.to_vec();
    guard_ptr(|| match Image::from_rgb(width, height, bytes) {
        Some(image) => Box::into_raw(Box::new(PrismPdfImageSource(image))),
        None => std::ptr::null_mut(),
    })
}

/// Wrap raw 8-bit grayscale samples (`width * height` bytes). Returns null on a length mismatch.
///
/// # Safety
/// As [`prismpdf_image_source_from_rgb`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_image_source_from_gray(
    width: u32,
    height: u32,
    data: *const u8,
    len: usize,
) -> *mut PrismPdfImageSource {
    if data.is_null() {
        return std::ptr::null_mut();
    }
    let bytes = unsafe { std::slice::from_raw_parts(data, len) }.to_vec();
    guard_ptr(|| match Image::from_gray(width, height, bytes) {
        Some(image) => Box::into_raw(Box::new(PrismPdfImageSource(image))),
        None => std::ptr::null_mut(),
    })
}

/// Wrap raw 8-bit RGBA samples (`width * height * 4` bytes): the alpha channel becomes a
/// `DeviceGray` soft mask (`/SMask`, §11.6.5.2), so the image carries per-pixel transparency.
/// Returns null on a length mismatch.
///
/// # Safety
/// As [`prismpdf_image_source_from_rgb`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_image_source_from_rgba(
    width: u32,
    height: u32,
    data: *const u8,
    len: usize,
) -> *mut PrismPdfImageSource {
    if data.is_null() {
        return std::ptr::null_mut();
    }
    let bytes = unsafe { std::slice::from_raw_parts(data, len) }.to_vec();
    guard_ptr(|| match Image::from_rgba(width, height, bytes) {
        Some(image) => Box::into_raw(Box::new(PrismPdfImageSource(image))),
        None => std::ptr::null_mut(),
    })
}

/// The image's pixel dimensions.
///
/// # Safety
/// `image` must be live; both out-params writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_image_source_size(
    image: *const PrismPdfImageSource,
    out_width: *mut u32,
    out_height: *mut u32,
) -> PrismPdfStatus {
    if out_width.is_null() || out_height.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    unsafe {
        *out_width = 0;
        *out_height = 0;
    }
    if image.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        let source = unsafe { &(*image).0 };
        unsafe {
            *out_width = source.width();
            *out_height = source.height();
        }
        PrismPdfStatus::Ok
    })
}

/// Release a placeable image. Freeing `NULL` is a no-op.
///
/// # Safety
/// `image` must come from a `prismpdf_image_source_from_*` call and must not already be freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_image_source_free(image: *mut PrismPdfImageSource) {
    unsafe { free_handle(image) }
}

// --- Tables -----------------------------------------------------------------------------------

/// A table laid out across fixed column widths. Rust's builder takes `self` by value; the handle
/// clones-and-stores so C sees plain in-place mutation.
pub struct PrismPdfTable(pub(crate) Table);

/// Create a table with `count` column widths in points.
///
/// # Safety
/// `columns` must point to `count` readable `double`s. Release the handle with
/// [`prismpdf_table_free`]. Returns null on a null argument.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_table_new(
    columns: *const f64,
    count: usize,
) -> *mut PrismPdfTable {
    if columns.is_null() || count == 0 {
        return std::ptr::null_mut();
    }
    let widths = unsafe { std::slice::from_raw_parts(columns, count) }.to_vec();
    guard_ptr(|| Box::into_raw(Box::new(PrismPdfTable(Table::new(widths)))))
}

/// Release a table. Freeing `NULL` is a no-op.
///
/// # Safety
/// `table` must come from [`prismpdf_table_new`] and must not already be freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_table_free(table: *mut PrismPdfTable) {
    unsafe { free_handle(table) }
}

/// Set the table's font: the page resource name and the PostScript base font used for metrics.
///
/// # Safety
/// `table` must be live; both names NUL-terminated UTF-8 C strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_table_set_font(
    table: *mut PrismPdfTable,
    resource: *const c_char,
    base_font: *const c_char,
) -> PrismPdfStatus {
    if table.is_null() || resource.is_null() || base_font.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        let (Some(resource), Some(base)) = (unsafe { utf8(resource) }, unsafe { utf8(base_font) })
        else {
            return PrismPdfStatus::NullArgument;
        };
        let handle = unsafe { &mut (*table).0 };
        *handle = handle.clone().font(resource, base);
        PrismPdfStatus::Ok
    })
}

/// Append a row of `count` cells, in column order. Call after
/// [`prismpdf_table_set_header_row`] to make the first row a repeating header.
///
/// # Safety
/// `table` must be live and `cells` must point to `count` non-null NUL-terminated UTF-8 C strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_table_add_row(
    table: *mut PrismPdfTable,
    cells: *const *const c_char,
    count: usize,
) -> PrismPdfStatus {
    if table.is_null() || (count > 0 && cells.is_null()) {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        let mut values: Vec<String> = Vec::with_capacity(count);
        for i in 0..count {
            let ptr = unsafe { *cells.add(i) };
            let Some(text) = (unsafe { utf8(ptr) }) else {
                return PrismPdfStatus::NullArgument;
            };
            values.push(text.to_string())
        }
        let handle = unsafe { &mut (*table).0 };
        *handle = handle.clone().row(values);
        PrismPdfStatus::Ok
    })
}

/// Font size in points.
///
/// # Safety
/// `table` must be a live handle from [`prismpdf_table_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_table_set_size(
    table: *mut PrismPdfTable,
    size: f64,
) -> PrismPdfStatus {
    if table.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        let handle = unsafe { &mut (*table).0 };
        *handle = handle.clone().size(size);
        PrismPdfStatus::Ok
    })
}
/// Baseline-to-baseline distance in points.
///
/// # Safety
/// `table` must be a live handle from [`prismpdf_table_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_table_set_leading(
    table: *mut PrismPdfTable,
    leading: f64,
) -> PrismPdfStatus {
    if table.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        let handle = unsafe { &mut (*table).0 };
        *handle = handle.clone().leading(leading);
        PrismPdfStatus::Ok
    })
}
/// Cell padding in points.
///
/// # Safety
/// `table` must be a live handle from [`prismpdf_table_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_table_set_padding(
    table: *mut PrismPdfTable,
    padding: f64,
) -> PrismPdfStatus {
    if table.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        let handle = unsafe { &mut (*table).0 };
        *handle = handle.clone().padding(padding);
        PrismPdfStatus::Ok
    })
}
/// Border stroke width in points; 0 draws none.
///
/// # Safety
/// `table` must be a live handle from [`prismpdf_table_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_table_set_border(
    table: *mut PrismPdfTable,
    width: f64,
) -> PrismPdfStatus {
    if table.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        let handle = unsafe { &mut (*table).0 };
        *handle = handle.clone().border(width);
        PrismPdfStatus::Ok
    })
}
/// Whether the first row is a header, repeated on each page.
///
/// # Safety
/// `table` must be a live handle from [`prismpdf_table_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_table_set_header_row(
    table: *mut PrismPdfTable,
    on: bool,
) -> PrismPdfStatus {
    if table.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        let handle = unsafe { &mut (*table).0 };
        *handle = handle.clone().header_row(on);
        PrismPdfStatus::Ok
    })
}
/// Horizontal alignment of every cell.
///
/// # Safety
/// `table` must be a live handle from [`prismpdf_table_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_table_set_align(
    table: *mut PrismPdfTable,
    align: PrismPdfAlign,
) -> PrismPdfStatus {
    if table.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        let handle = unsafe { &mut (*table).0 };
        *handle = handle.clone().align(align.into());
        PrismPdfStatus::Ok
    })
}
// --- Flow -------------------------------------------------------------------------------------

/// A document being poured page by page, breaking automatically when content runs past the bottom
/// margin. The highest-level authoring API in the engine.
pub struct PrismPdfFlow(pub(crate) Flow);

/// Create a flow with a page size and margins in points, exposing `font_count` Standard-14 fonts
/// under the given resource names.
///
/// `size` is `[width height]`; `margins` is `[top right bottom left]`.
///
/// # Safety
/// `size` must point to 2 readable `double`s and `margins` to 4; `font_names` must point to
/// `font_count` non-null C strings and `fonts` to `font_count` [`PrismPdfStdFont`] values. Release
/// the handle with [`prismpdf_flow_free`], or consume it with [`prismpdf_flow_build`]. Returns null
/// on a null or non-UTF-8 argument.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_flow_new(
    size: *const f64,
    margins: *const f64,
    font_names: *const *const c_char,
    fonts: *const PrismPdfStdFont,
    font_count: usize,
) -> *mut PrismPdfFlow {
    if size.is_null() || margins.is_null() {
        return std::ptr::null_mut();
    }
    if font_count > 0 && (font_names.is_null() || fonts.is_null()) {
        return std::ptr::null_mut();
    }
    let mut page_size = [0.0f64; 2];
    let mut page_margins = [0.0f64; 4];
    unsafe {
        std::ptr::copy_nonoverlapping(size, page_size.as_mut_ptr(), 2);
        std::ptr::copy_nonoverlapping(margins, page_margins.as_mut_ptr(), 4);
    }
    let mut names: Vec<&str> = Vec::with_capacity(font_count);
    for i in 0..font_count {
        let ptr = unsafe { *font_names.add(i) };
        let Some(name) = (unsafe { utf8(ptr) }) else {
            return std::ptr::null_mut();
        };
        names.push(name)
    }
    let pairs: Vec<(&str, StdFont)> = names
        .iter()
        .enumerate()
        .map(|(i, name)| (*name, StdFont::from(unsafe { *fonts.add(i) })))
        .collect();
    let style = PageStyle {
        size: page_size,
        margins: page_margins,
    };
    guard_ptr(|| Box::into_raw(Box::new(PrismPdfFlow(Flow::new(style, &pairs)))))
}

/// Release a flow **without** building it. Freeing `NULL` is a no-op.
///
/// Do not call this after [`prismpdf_flow_build`] or [`prismpdf_flow_into_builder`]: those consume
/// the handle.
///
/// # Safety
/// `flow` must come from [`prismpdf_flow_new`] and must not already be freed or consumed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_flow_free(flow: *mut PrismPdfFlow) {
    unsafe { free_handle(flow) }
}

/// Borrow a flow handle for mutation, or `None` when it is null.
///
/// # Safety
/// `flow` must be a live handle or null.
pub(crate) unsafe fn flow_mut<'a>(flow: *mut PrismPdfFlow) -> Option<&'a mut Flow> {
    if flow.is_null() {
        return None;
    }
    Some(unsafe { &mut (*flow).0 })
}

/// Borrow a flow, a text style and a C string together — the argument shape most flow calls take.
///
/// # Safety
/// All three must be live handles/strings or null.
pub(crate) unsafe fn flow_block_text<'a>(
    flow: *mut PrismPdfFlow,
    block: *const PrismPdfTextBlock,
    text: *const c_char,
) -> Option<(&'a mut Flow, &'a PrismPdfTextBlock, &'a str)> {
    if flow.is_null() || block.is_null() || text.is_null() {
        return None;
    }
    let text = unsafe { utf8(text) }?;
    Some((unsafe { &mut (*flow).0 }, unsafe { &*block }, text))
}

/// **Consume** the flow and serialise it. The handle is dead afterwards — do not free it.
///
/// # Safety
/// `flow` must come from [`prismpdf_flow_new`] and must not already be consumed;
/// `out_data`/`out_len` must be writable. Release the buffer with [`prismpdf_bytes_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_flow_build(
    flow: *mut PrismPdfFlow,
    out_data: *mut *mut u8,
    out_len: *mut usize,
) -> PrismPdfStatus {
    if out_data.is_null() || out_len.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    unsafe {
        *out_data = std::ptr::null_mut();
        *out_len = 0;
    }
    if flow.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        let owned = unsafe { Box::from_raw(flow) };
        emit_bytes(owned.0.build(), out_data, out_len)
    })
}

/// **Consume** the flow into a [`PrismPdfBuilder`] without serialising, so the document can be
/// post-processed — running a conformance pass, attaching files, adding annotations. This is the
/// composition point between the layout API and everything else.
///
/// The flow handle is dead afterwards; the returned builder must be freed.
///
/// # Safety
/// `flow` must come from [`prismpdf_flow_new`] and must not already be consumed; `out_builder`
/// must be writable. Release the builder with [`prismpdf_builder_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_flow_into_builder(
    flow: *mut PrismPdfFlow,
    out_builder: *mut *mut PrismPdfBuilder,
) -> PrismPdfStatus {
    if out_builder.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    unsafe { *out_builder = std::ptr::null_mut() };
    if flow.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        let owned = unsafe { Box::from_raw(flow) };
        let builder = owned.0.into_builder();
        unsafe { *out_builder = Box::into_raw(Box::new(PrismPdfBuilder(builder))) };
        PrismPdfStatus::Ok
    })
}

/// Turn on logical structure (tagging) in `lang`, so the flow emits a structure tree — the
/// prerequisite for PDF/UA and PDF/A level A.
///
/// # Safety
/// `flow` must be live and `lang` a NUL-terminated UTF-8 C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_flow_set_tagged(
    flow: *mut PrismPdfFlow,
    lang: *const c_char,
) -> PrismPdfStatus {
    let Some(handle) = (unsafe { flow_mut(flow) }) else {
        return PrismPdfStatus::NullArgument;
    };
    if lang.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        let Some(lang) = (unsafe { utf8(lang) }) else {
            return PrismPdfStatus::NullArgument;
        };
        handle.tagged(lang);
        PrismPdfStatus::Ok
    })
}

/// Embed a real font program under `resource`, replacing the Standard-14 font of that name — the
/// call that makes a flowed document PDF/A-conformant.
///
/// Call it before pouring any text that uses `resource`: a page carries one resource entry per
/// name, so text already drawn in the Standard-14 font of that name would be shown by the embedded
/// one instead.
///
/// Returns [`PrismPdfStatus::Parse`] when the program cannot be parsed as an sfnt.
///
/// # Safety
/// `flow` must be live, `resource` a NUL-terminated UTF-8 C string, and `program` must point to
/// `len` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_flow_embed_font(
    flow: *mut PrismPdfFlow,
    resource: *const c_char,
    program: *const u8,
    len: usize,
) -> PrismPdfStatus {
    let Some(handle) = (unsafe { flow_mut(flow) }) else {
        return PrismPdfStatus::NullArgument;
    };
    if resource.is_null() || program.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        let Some(resource) = (unsafe { utf8(resource) }) else {
            return PrismPdfStatus::NullArgument;
        };
        let bytes = unsafe { slice_or_empty(program, len) };
        if handle.embed_font(resource, &bytes) {
            PrismPdfStatus::Ok
        } else {
            PrismPdfStatus::Parse
        }
    })
}

/// Add an `/Info` entry (§14.3.3).
///
/// # Safety
/// `flow` must be live; `key` and `value` NUL-terminated UTF-8 C strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_flow_set_info(
    flow: *mut PrismPdfFlow,
    key: *const c_char,
    value: *const c_char,
) -> PrismPdfStatus {
    let Some(handle) = (unsafe { flow_mut(flow) }) else {
        return PrismPdfStatus::NullArgument;
    };
    if key.is_null() || value.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        let (Some(key), Some(value)) = (unsafe { utf8(key) }, unsafe { utf8(value) }) else {
            return PrismPdfStatus::NullArgument;
        };
        handle.info(key, value);
        PrismPdfStatus::Ok
    })
}

/// Add a bookmark (§12.3.3) pointing at the current position in the flow.
///
/// # Safety
/// `flow` must be live and `title` a NUL-terminated UTF-8 C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_flow_add_bookmark(
    flow: *mut PrismPdfFlow,
    title: *const c_char,
) -> PrismPdfStatus {
    let Some(handle) = (unsafe { flow_mut(flow) }) else {
        return PrismPdfStatus::NullArgument;
    };
    if title.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        let Some(title) = (unsafe { utf8(title) }) else {
            return PrismPdfStatus::NullArgument;
        };
        handle.bookmark(title);
        PrismPdfStatus::Ok
    })
}

/// Pour a paragraph in the given style, wrapping to the text column and breaking pages as needed.
///
/// # Safety
/// `flow` and `block` must be live handles and `text` a NUL-terminated UTF-8 C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_flow_text(
    flow: *mut PrismPdfFlow,
    block: *const PrismPdfTextBlock,
    text: *const c_char,
) -> PrismPdfStatus {
    let Some((handle, style, text)) = (unsafe { flow_block_text(flow, block, text) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| {
        handle.text(&style.view(), text);
        PrismPdfStatus::Ok
    })
}

/// Pour a heading at `level` (1–6), tagged `H1`…`H6` when the flow is tagged.
///
/// # Safety
/// As [`prismpdf_flow_text`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_flow_heading(
    flow: *mut PrismPdfFlow,
    level: u8,
    block: *const PrismPdfTextBlock,
    text: *const c_char,
) -> PrismPdfStatus {
    let Some((handle, style, text)) = (unsafe { flow_block_text(flow, block, text) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| {
        handle.heading(level, &style.view(), text);
        PrismPdfStatus::Ok
    })
}

/// Pour a bulleted or numbered list of `count` items.
///
/// # Safety
/// `flow` and `block` must be live and `items` must point to `count` non-null NUL-terminated
/// UTF-8 C strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_flow_list(
    flow: *mut PrismPdfFlow,
    block: *const PrismPdfTextBlock,
    items: *const *const c_char,
    count: usize,
    style: PrismPdfListStyle,
) -> PrismPdfStatus {
    let Some(handle) = (unsafe { flow_mut(flow) }) else {
        return PrismPdfStatus::NullArgument;
    };
    if block.is_null() || (count > 0 && items.is_null()) {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        let mut entries: Vec<&str> = Vec::with_capacity(count);
        for i in 0..count {
            let ptr = unsafe { *items.add(i) };
            let Some(text) = (unsafe { utf8(ptr) }) else {
                return PrismPdfStatus::NullArgument;
            };
            entries.push(text)
        }
        let text_style = unsafe { &*block };
        handle.list(&text_style.view(), &entries, style.into());
        PrismPdfStatus::Ok
    })
}

/// Place a table, breaking across pages and repeating the header row where one is set.
///
/// # Safety
/// `flow` and `table` must be live handles.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_flow_table(
    flow: *mut PrismPdfFlow,
    table: *const PrismPdfTable,
) -> PrismPdfStatus {
    let Some(handle) = (unsafe { flow_mut(flow) }) else {
        return PrismPdfStatus::NullArgument;
    };
    if table.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        handle.table(unsafe { &(*table).0 });
        PrismPdfStatus::Ok
    })
}

/// Place an image at an explicit size in points, as an artifact (untagged decoration).
///
/// # Safety
/// `flow` and `image` must be live handles.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_flow_image(
    flow: *mut PrismPdfFlow,
    image: *const PrismPdfImageSource,
    width: f64,
    height: f64,
) -> PrismPdfStatus {
    let Some(handle) = (unsafe { flow_mut(flow) }) else {
        return PrismPdfStatus::NullArgument;
    };
    if image.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        handle.image(unsafe { &(*image).0 }, width, height);
        PrismPdfStatus::Ok
    })
}

/// Place an image scaled to fit `max_width` points, preserving aspect ratio.
///
/// # Safety
/// As [`prismpdf_flow_image`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_flow_image_fit(
    flow: *mut PrismPdfFlow,
    image: *const PrismPdfImageSource,
    max_width: f64,
) -> PrismPdfStatus {
    let Some(handle) = (unsafe { flow_mut(flow) }) else {
        return PrismPdfStatus::NullArgument;
    };
    if image.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        handle.image_fit(unsafe { &(*image).0 }, max_width);
        PrismPdfStatus::Ok
    })
}

/// Place an image as a **tagged `Figure`** carrying `alt` text — what PDF/UA requires (§7.3), and
/// the difference between [`prismpdf_flow_image`] and an accessible document.
///
/// # Safety
/// `flow` and `image` must be live and `alt` a NUL-terminated UTF-8 C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_flow_figure(
    flow: *mut PrismPdfFlow,
    image: *const PrismPdfImageSource,
    width: f64,
    height: f64,
    alt: *const c_char,
) -> PrismPdfStatus {
    let Some(handle) = (unsafe { flow_mut(flow) }) else {
        return PrismPdfStatus::NullArgument;
    };
    if image.is_null() || alt.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        let Some(alt) = (unsafe { utf8(alt) }) else {
            return PrismPdfStatus::NullArgument;
        };
        handle.figure(unsafe { &(*image).0 }, width, height, alt);
        PrismPdfStatus::Ok
    })
}

/// A tagged `Figure` scaled to fit `max_width`, carrying `alt` text.
///
/// # Safety
/// As [`prismpdf_flow_figure`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_flow_figure_fit(
    flow: *mut PrismPdfFlow,
    image: *const PrismPdfImageSource,
    max_width: f64,
    alt: *const c_char,
) -> PrismPdfStatus {
    let Some(handle) = (unsafe { flow_mut(flow) }) else {
        return PrismPdfStatus::NullArgument;
    };
    if image.is_null() || alt.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        let Some(alt) = (unsafe { utf8(alt) }) else {
            return PrismPdfStatus::NullArgument;
        };
        handle.figure_fit(unsafe { &(*image).0 }, max_width, alt);
        PrismPdfStatus::Ok
    })
}

/// A tagged `Figure` with a `Caption` beneath it, kept together on one page.
///
/// # Safety
/// `flow`, `image` and `block` must be live; `alt` and `caption` NUL-terminated UTF-8 C strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_flow_figure_with_caption(
    flow: *mut PrismPdfFlow,
    image: *const PrismPdfImageSource,
    width: f64,
    height: f64,
    alt: *const c_char,
    block: *const PrismPdfTextBlock,
    caption: *const c_char,
) -> PrismPdfStatus {
    let Some(handle) = (unsafe { flow_mut(flow) }) else {
        return PrismPdfStatus::NullArgument;
    };
    if image.is_null() || alt.is_null() || block.is_null() || caption.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        let (Some(alt), Some(caption)) = (unsafe { utf8(alt) }, unsafe { utf8(caption) }) else {
            return PrismPdfStatus::NullArgument;
        };
        let style = unsafe { &*block };
        handle.figure_with_caption(
            unsafe { &(*image).0 },
            width,
            height,
            alt,
            &style.view(),
            caption,
        );
        PrismPdfStatus::Ok
    })
}

/// A footnote tagged `Note` with the given `id` (PDF/UA-1). PDF/UA-2 forbids `Note` — use
/// [`prismpdf_flow_fenote`] there.
///
/// # Safety
/// `flow` and `block` must be live; `text` and `id` NUL-terminated UTF-8 C strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_flow_note(
    flow: *mut PrismPdfFlow,
    block: *const PrismPdfTextBlock,
    text: *const c_char,
    id: *const c_char,
) -> PrismPdfStatus {
    let Some((handle, style, text)) = (unsafe { flow_block_text(flow, block, text) }) else {
        return PrismPdfStatus::NullArgument;
    };
    if id.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        let Some(id) = (unsafe { utf8(id) }) else {
            return PrismPdfStatus::NullArgument;
        };
        handle.note(&style.view(), text, id);
        PrismPdfStatus::Ok
    })
}

/// A footnote tagged `FENote` (ISO 14289-2 §8.2.5.14) with `citation_count` citation references —
/// the PDF/UA-2 replacement for `Note`.
///
/// # Safety
/// `flow` and `block` must be live; `text` and `id` NUL-terminated UTF-8 C strings; `citations`
/// must point to `citation_count` non-null C strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_flow_fenote(
    flow: *mut PrismPdfFlow,
    block: *const PrismPdfTextBlock,
    text: *const c_char,
    id: *const c_char,
    citations: *const *const c_char,
    citation_count: usize,
) -> PrismPdfStatus {
    let Some((handle, style, text)) = (unsafe { flow_block_text(flow, block, text) }) else {
        return PrismPdfStatus::NullArgument;
    };
    if id.is_null() || (citation_count > 0 && citations.is_null()) {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        let Some(id) = (unsafe { utf8(id) }) else {
            return PrismPdfStatus::NullArgument;
        };
        let mut refs: Vec<&str> = Vec::with_capacity(citation_count);
        for i in 0..citation_count {
            let ptr = unsafe { *citations.add(i) };
            let Some(value) = (unsafe { utf8(ptr) }) else {
                return PrismPdfStatus::NullArgument;
            };
            refs.push(value)
        }
        handle.fenote(&style.view(), text, id, &refs);
        PrismPdfStatus::Ok
    })
}

/// The document title as a tagged `Title` element.
///
/// # Safety
/// As [`prismpdf_flow_text`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_flow_title_element(
    flow: *mut PrismPdfFlow,
    block: *const PrismPdfTextBlock,
    text: *const c_char,
) -> PrismPdfStatus {
    let Some((handle, style, text)) = (unsafe { flow_block_text(flow, block, text) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| {
        handle.title_element(&style.view(), text);
        PrismPdfStatus::Ok
    })
}

/// A formula tagged `Formula` with `actual_text` as its `/ActualText` — how a mathematical
/// expression is made readable to assistive technology.
///
/// # Safety
/// `flow` and `block` must be live; `text` and `actual_text` NUL-terminated UTF-8 C strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_flow_formula(
    flow: *mut PrismPdfFlow,
    block: *const PrismPdfTextBlock,
    text: *const c_char,
    actual_text: *const c_char,
) -> PrismPdfStatus {
    let Some((handle, style, text)) = (unsafe { flow_block_text(flow, block, text) }) else {
        return PrismPdfStatus::NullArgument;
    };
    if actual_text.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        let Some(actual) = (unsafe { utf8(actual_text) }) else {
            return PrismPdfStatus::NullArgument;
        };
        handle.formula(&style.view(), text, actual);
        PrismPdfStatus::Ok
    })
}

/// A running header drawn at the top of every page, as an artifact.
///
/// # Safety
/// As [`prismpdf_flow_text`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_flow_set_header(
    flow: *mut PrismPdfFlow,
    block: *const PrismPdfTextBlock,
    text: *const c_char,
) -> PrismPdfStatus {
    let Some((handle, style, text)) = (unsafe { flow_block_text(flow, block, text) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| {
        handle.header(&style.view(), text);
        PrismPdfStatus::Ok
    })
}

/// A running footer drawn at the bottom of every page, as an artifact.
///
/// # Safety
/// As [`prismpdf_flow_text`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_flow_set_footer(
    flow: *mut PrismPdfFlow,
    block: *const PrismPdfTextBlock,
    text: *const c_char,
) -> PrismPdfStatus {
    let Some((handle, style, text)) = (unsafe { flow_block_text(flow, block, text) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| {
        handle.footer(&style.view(), text);
        PrismPdfStatus::Ok
    })
}

/// Advance the cursor by `dy` points without drawing.
///
/// # Safety
/// `flow` must be a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_flow_space(flow: *mut PrismPdfFlow, dy: f64) -> PrismPdfStatus {
    let Some(handle) = (unsafe { flow_mut(flow) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| {
        handle.space(dy);
        PrismPdfStatus::Ok
    })
}

/// Finish the current page and start a new one.
///
/// # Safety
/// `flow` must be a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_flow_page_break(flow: *mut PrismPdfFlow) -> PrismPdfStatus {
    let Some(handle) = (unsafe { flow_mut(flow) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| {
        handle.page_break();
        PrismPdfStatus::Ok
    })
}

/// How many pages the flow has produced so far.
///
/// # Safety
/// `flow` must be live and `out_count` a writable `*mut usize`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_flow_page_count(
    flow: *const PrismPdfFlow,
    out_count: *mut usize,
) -> PrismPdfStatus {
    if out_count.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    unsafe { *out_count = 0 };
    if flow.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        unsafe { *out_count = (*flow).0.page_count() };
        PrismPdfStatus::Ok
    })
}

/// The current vertical cursor position in points from the page bottom — for deciding whether the
/// next block fits before it breaks.
///
/// # Safety
/// `flow` must be live and `out_y` a writable `*mut f64`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_flow_cursor_y(
    flow: *const PrismPdfFlow,
    out_y: *mut f64,
) -> PrismPdfStatus {
    if out_y.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    unsafe { *out_y = 0.0 };
    if flow.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        unsafe { *out_y = (*flow).0.cursor_y() };
        PrismPdfStatus::Ok
    })
}

/// The document title (`/Title`, §14.3.3).
///
/// # Safety
/// `flow` must be live and `value` a NUL-terminated UTF-8 C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_flow_set_title(
    flow: *mut PrismPdfFlow,
    value: *const c_char,
) -> PrismPdfStatus {
    let Some(handle) = (unsafe { flow_mut(flow) }) else {
        return PrismPdfStatus::NullArgument;
    };
    if value.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        let Some(value) = (unsafe { utf8(value) }) else {
            return PrismPdfStatus::NullArgument;
        };
        handle.title(value);
        PrismPdfStatus::Ok
    })
}
/// The document author (`/Author`, §14.3.3).
///
/// # Safety
/// `flow` must be live and `value` a NUL-terminated UTF-8 C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_flow_set_author(
    flow: *mut PrismPdfFlow,
    value: *const c_char,
) -> PrismPdfStatus {
    let Some(handle) = (unsafe { flow_mut(flow) }) else {
        return PrismPdfStatus::NullArgument;
    };
    if value.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        let Some(value) = (unsafe { utf8(value) }) else {
            return PrismPdfStatus::NullArgument;
        };
        handle.author(value);
        PrismPdfStatus::Ok
    })
}

// ---------------------------------------------------------------------------------------------
// Declarative composition (M25 / pre-1.0 Phase 5).
//
// Container handles never borrow Rust nodes. Both the composition and every scoped container hold
// an `Arc` to an arena; a stable tree id plus per-slot generation detects cross-tree/stale use.
// Releasing the composition flips `alive`, so surviving container handles fail safely.
