use super::*;

// ---------------------------------------------------------------------------------------------

/// A content-stream builder. Created by [`prismpdf_content_new`], released by
/// [`prismpdf_content_free`].
pub struct PrismPdfContent(pub(crate) Content);

/// An owned low-level page description: content plus exactly the resources it references.
///
/// This is the precision/escape-hatch layer below composition. Create it from a content handle,
/// add resources, then transfer it to a [`PrismPdfBuilder`] with
/// [`prismpdf_builder_add_page_spec`].
pub struct PrismPdfPageSpec(pub(crate) PageSpec);

/// Create a page specification by copying the assembled content bytes (§7.8.2).
///
/// The content handle remains owned by the caller and may be reused or freed immediately.
/// Returns null when `content` is null.
///
/// # Safety
/// `content` must be a live handle. Release the result with [`prismpdf_page_spec_free`] unless it
/// is successfully transferred to [`prismpdf_builder_add_page_spec`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_page_spec_new(
    content: *const PrismPdfContent,
) -> *mut PrismPdfPageSpec {
    if content.is_null() {
        return std::ptr::null_mut();
    }
    guard_ptr(|| {
        let bytes = unsafe { &(*content).0 }.as_bytes().to_vec();
        Box::into_raw(Box::new(PrismPdfPageSpec(PageSpec::new(bytes))))
    })
}

/// Release a page specification. Freeing `NULL` is a no-op.
///
/// # Safety
/// `page` must be a live, untransferred page-spec handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_page_spec_free(page: *mut PrismPdfPageSpec) {
    unsafe { free_handle(page) }
}

pub(crate) unsafe fn page_spec_update(
    page: *mut PrismPdfPageSpec,
    update: impl FnOnce(PageSpec) -> PageSpec,
) -> PrismPdfStatus {
    if page.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        let handle = unsafe { &mut (*page).0 };
        *handle = update(std::mem::take(handle));
        PrismPdfStatus::Ok
    })
}

/// Override this page's media box (§14.11.2).
///
/// # Safety
/// `page` must be a live page-spec handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_page_spec_set_media_box(
    page: *mut PrismPdfPageSpec,
    llx: f64,
    lly: f64,
    urx: f64,
    ury: f64,
) -> PrismPdfStatus {
    unsafe { page_spec_update(page, |spec| spec.media_box([llx, lly, urx, ury])) }
}

/// Add a named Standard-14 font resource (§9.6.2.2).
///
/// # Safety
/// `page` must be live and `name` must be a NUL-terminated UTF-8 string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_page_spec_add_standard_font(
    page: *mut PrismPdfPageSpec,
    name: *const c_char,
    font: PrismPdfStdFont,
) -> PrismPdfStatus {
    let Some(name) = (unsafe { utf8(name) }) else {
        return PrismPdfStatus::NullArgument;
    };
    unsafe { page_spec_update(page, |spec| spec.standard_font(name, StdFont::from(font))) }
}

/// Reference a CID font previously registered on the builder under `name` (§9.7).
///
/// # Safety
/// `page` must be live and `name` must be a NUL-terminated UTF-8 string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_page_spec_add_embedded_font(
    page: *mut PrismPdfPageSpec,
    name: *const c_char,
) -> PrismPdfStatus {
    let Some(name) = (unsafe { utf8(name) }) else {
        return PrismPdfStatus::NullArgument;
    };
    unsafe { page_spec_update(page, |spec| spec.embedded_font(name)) }
}

/// Add a named image XObject resource (§8.9). The image is copied and remains caller-owned.
///
/// # Safety
/// All handles must be live and `name` must be a NUL-terminated UTF-8 string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_page_spec_add_image(
    page: *mut PrismPdfPageSpec,
    name: *const c_char,
    image: *const PrismPdfImageSource,
) -> PrismPdfStatus {
    if name.is_null() || image.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    let Some(name) = (unsafe { utf8(name) }) else {
        return PrismPdfStatus::NullArgument;
    };
    let xobject = unsafe { &(*image).0 }.to_xobject();
    unsafe { page_spec_update(page, |spec| spec.image(name, xobject)) }
}

/// Create an empty content stream.
///
/// # Safety
/// The returned handle must be released with [`prismpdf_content_free`].
#[unsafe(no_mangle)]
pub extern "C" fn prismpdf_content_new() -> *mut PrismPdfContent {
    guard_ptr(|| Box::into_raw(Box::new(PrismPdfContent(Content::new()))))
}

/// Release a content handle. Freeing `NULL` is a no-op, and any byte view lent by
/// [`prismpdf_content_bytes`] is dangling afterwards.
///
/// # Safety
/// `content` must come from [`prismpdf_content_new`] and must not already be freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_content_free(content: *mut PrismPdfContent) {
    unsafe { free_handle(content) }
}

/// Lend the assembled operator bytes. **Borrowed**: the view dies with the handle and must not be
/// passed to [`prismpdf_bytes_free`]. An empty stream lends a null pointer with length 0.
///
/// # Safety
/// `content` must be a live handle; `out_data`/`out_len` must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_content_bytes(
    content: *const PrismPdfContent,
    out_data: *mut *const u8,
    out_len: *mut usize,
) -> PrismPdfStatus {
    let Some(handle) = (unsafe { prepare_borrowed_bytes_out(content, out_data, out_len) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| unsafe { lend_bytes(handle.0.as_bytes(), out_data, out_len) })
}

/// Borrow a content handle for mutation, or `None` when it is null.
///
/// # Safety
/// `content` must be a live handle or null.
pub(crate) unsafe fn content_mut<'a>(content: *mut PrismPdfContent) -> Option<&'a mut Content> {
    if content.is_null() {
        return None;
    }
    Some(unsafe { &mut (*content).0 })
}

/// Push the graphics state (`q`, §8.4.4).
///
/// # Safety
/// `content` must be a live handle from [`prismpdf_content_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_content_save(content: *mut PrismPdfContent) -> PrismPdfStatus {
    let Some(stream) = (unsafe { content_mut(content) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| {
        stream.save();
        PrismPdfStatus::Ok
    })
}

/// Pop the graphics state (`Q`, §8.4.4).
///
/// # Safety
/// `content` must be a live handle from [`prismpdf_content_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_content_restore(content: *mut PrismPdfContent) -> PrismPdfStatus {
    let Some(stream) = (unsafe { content_mut(content) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| {
        stream.restore();
        PrismPdfStatus::Ok
    })
}

/// Concatenate a matrix onto the current transformation (`cm`, §8.3.3).
///
/// # Safety
/// `content` must be a live handle from [`prismpdf_content_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_content_transform(
    content: *mut PrismPdfContent,
    a: f64,
    b: f64,
    c: f64,
    d: f64,
    e: f64,
    f: f64,
) -> PrismPdfStatus {
    let Some(stream) = (unsafe { content_mut(content) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| {
        stream.transform(a, b, c, d, e, f);
        PrismPdfStatus::Ok
    })
}

/// Set the stroke width (`w`, §8.4.3.2).
///
/// # Safety
/// `content` must be a live handle from [`prismpdf_content_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_content_set_line_width(
    content: *mut PrismPdfContent,
    width: f64,
) -> PrismPdfStatus {
    let Some(stream) = (unsafe { content_mut(content) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| {
        stream.set_line_width(width);
        PrismPdfStatus::Ok
    })
}

/// Set a `DeviceGray` fill colour (`g`, §8.6.4.2).
///
/// # Safety
/// `content` must be a live handle from [`prismpdf_content_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_content_set_fill_gray(
    content: *mut PrismPdfContent,
    gray: f64,
) -> PrismPdfStatus {
    let Some(stream) = (unsafe { content_mut(content) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| {
        stream.set_fill_gray(gray);
        PrismPdfStatus::Ok
    })
}

/// Set a `DeviceGray` stroke colour (`G`, §8.6.4.2).
///
/// # Safety
/// `content` must be a live handle from [`prismpdf_content_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_content_set_stroke_gray(
    content: *mut PrismPdfContent,
    gray: f64,
) -> PrismPdfStatus {
    let Some(stream) = (unsafe { content_mut(content) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| {
        stream.set_stroke_gray(gray);
        PrismPdfStatus::Ok
    })
}

/// Set a `DeviceRGB` fill colour (`rg`, §8.6.4.3).
///
/// # Safety
/// `content` must be a live handle from [`prismpdf_content_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_content_set_fill_rgb(
    content: *mut PrismPdfContent,
    r: f64,
    g: f64,
    b: f64,
) -> PrismPdfStatus {
    let Some(stream) = (unsafe { content_mut(content) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| {
        stream.set_fill_rgb(r, g, b);
        PrismPdfStatus::Ok
    })
}

/// Set a `DeviceRGB` stroke colour (`RG`, §8.6.4.3).
///
/// # Safety
/// `content` must be a live handle from [`prismpdf_content_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_content_set_stroke_rgb(
    content: *mut PrismPdfContent,
    r: f64,
    g: f64,
    b: f64,
) -> PrismPdfStatus {
    let Some(stream) = (unsafe { content_mut(content) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| {
        stream.set_stroke_rgb(r, g, b);
        PrismPdfStatus::Ok
    })
}

/// Set a `DeviceCMYK` fill colour (`k`, §8.6.4.4).
///
/// # Safety
/// `content` must be a live handle from [`prismpdf_content_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_content_set_fill_cmyk(
    content: *mut PrismPdfContent,
    c: f64,
    m: f64,
    y: f64,
    k: f64,
) -> PrismPdfStatus {
    let Some(stream) = (unsafe { content_mut(content) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| {
        stream.set_fill_cmyk(c, m, y, k);
        PrismPdfStatus::Ok
    })
}

/// Begin a new subpath at `(x, y)` (`m`, §8.5.2.1).
///
/// # Safety
/// `content` must be a live handle from [`prismpdf_content_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_content_move_to(
    content: *mut PrismPdfContent,
    x: f64,
    y: f64,
) -> PrismPdfStatus {
    let Some(stream) = (unsafe { content_mut(content) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| {
        stream.move_to(x, y);
        PrismPdfStatus::Ok
    })
}

/// Append a straight segment to `(x, y)` (`l`, §8.5.2.1).
///
/// # Safety
/// `content` must be a live handle from [`prismpdf_content_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_content_line_to(
    content: *mut PrismPdfContent,
    x: f64,
    y: f64,
) -> PrismPdfStatus {
    let Some(stream) = (unsafe { content_mut(content) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| {
        stream.line_to(x, y);
        PrismPdfStatus::Ok
    })
}

/// Append a cubic Bezier with both control points (`c`, §8.5.2.2).
///
/// # Safety
/// `content` must be a live handle from [`prismpdf_content_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_content_curve_to(
    content: *mut PrismPdfContent,
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
    x3: f64,
    y3: f64,
) -> PrismPdfStatus {
    let Some(stream) = (unsafe { content_mut(content) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| {
        stream.curve_to(x1, y1, x2, y2, x3, y3);
        PrismPdfStatus::Ok
    })
}

/// Append a complete rectangular subpath (`re`, §8.5.2.1).
///
/// # Safety
/// `content` must be a live handle from [`prismpdf_content_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_content_rect(
    content: *mut PrismPdfContent,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
) -> PrismPdfStatus {
    let Some(stream) = (unsafe { content_mut(content) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| {
        stream.rect(x, y, w, h);
        PrismPdfStatus::Ok
    })
}

/// Close the current subpath (`h`, §8.5.2.1).
///
/// # Safety
/// `content` must be a live handle from [`prismpdf_content_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_content_close_path(
    content: *mut PrismPdfContent,
) -> PrismPdfStatus {
    let Some(stream) = (unsafe { content_mut(content) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| {
        stream.close_path();
        PrismPdfStatus::Ok
    })
}

/// Stroke the current path (`S`, §8.5.3.1).
///
/// # Safety
/// `content` must be a live handle from [`prismpdf_content_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_content_stroke(content: *mut PrismPdfContent) -> PrismPdfStatus {
    let Some(stream) = (unsafe { content_mut(content) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| {
        stream.stroke();
        PrismPdfStatus::Ok
    })
}

/// Fill the current path, non-zero winding (`f`, §8.5.3.3).
///
/// # Safety
/// `content` must be a live handle from [`prismpdf_content_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_content_fill(content: *mut PrismPdfContent) -> PrismPdfStatus {
    let Some(stream) = (unsafe { content_mut(content) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| {
        stream.fill();
        PrismPdfStatus::Ok
    })
}

/// Fill and then stroke the current path (`B`, §8.5.3.1).
///
/// # Safety
/// `content` must be a live handle from [`prismpdf_content_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_content_fill_and_stroke(
    content: *mut PrismPdfContent,
) -> PrismPdfStatus {
    let Some(stream) = (unsafe { content_mut(content) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| {
        stream.fill_and_stroke();
        PrismPdfStatus::Ok
    })
}

/// Begin a text object (`BT`, §9.4.1).
///
/// # Safety
/// `content` must be a live handle from [`prismpdf_content_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_content_begin_text(
    content: *mut PrismPdfContent,
) -> PrismPdfStatus {
    let Some(stream) = (unsafe { content_mut(content) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| {
        stream.begin_text();
        PrismPdfStatus::Ok
    })
}

/// End a text object (`ET`, §9.4.1).
///
/// # Safety
/// `content` must be a live handle from [`prismpdf_content_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_content_end_text(
    content: *mut PrismPdfContent,
) -> PrismPdfStatus {
    let Some(stream) = (unsafe { content_mut(content) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| {
        stream.end_text();
        PrismPdfStatus::Ok
    })
}

/// Set character spacing (`Tc`, §9.3.2).
///
/// # Safety
/// `content` must be a live handle from [`prismpdf_content_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_content_set_char_spacing(
    content: *mut PrismPdfContent,
    spacing: f64,
) -> PrismPdfStatus {
    let Some(stream) = (unsafe { content_mut(content) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| {
        stream.set_char_spacing(spacing);
        PrismPdfStatus::Ok
    })
}

/// Set word spacing (`Tw`, §9.3.3).
///
/// # Safety
/// `content` must be a live handle from [`prismpdf_content_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_content_set_word_spacing(
    content: *mut PrismPdfContent,
    spacing: f64,
) -> PrismPdfStatus {
    let Some(stream) = (unsafe { content_mut(content) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| {
        stream.set_word_spacing(spacing);
        PrismPdfStatus::Ok
    })
}

/// Set the leading used by `next_line` (`TL`, §9.3.5).
///
/// # Safety
/// `content` must be a live handle from [`prismpdf_content_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_content_set_leading(
    content: *mut PrismPdfContent,
    leading: f64,
) -> PrismPdfStatus {
    let Some(stream) = (unsafe { content_mut(content) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| {
        stream.set_leading(leading);
        PrismPdfStatus::Ok
    })
}

/// Move to the next line offset by `(tx, ty)` (`Td`, §9.4.2).
///
/// # Safety
/// `content` must be a live handle from [`prismpdf_content_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_content_text_move(
    content: *mut PrismPdfContent,
    tx: f64,
    ty: f64,
) -> PrismPdfStatus {
    let Some(stream) = (unsafe { content_mut(content) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| {
        stream.text_move(tx, ty);
        PrismPdfStatus::Ok
    })
}

/// Replace the text and line matrices (`Tm`, §9.4.2).
///
/// # Safety
/// `content` must be a live handle from [`prismpdf_content_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_content_set_text_matrix(
    content: *mut PrismPdfContent,
    a: f64,
    b: f64,
    c: f64,
    d: f64,
    e: f64,
    f: f64,
) -> PrismPdfStatus {
    let Some(stream) = (unsafe { content_mut(content) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| {
        stream.set_text_matrix(a, b, c, d, e, f);
        PrismPdfStatus::Ok
    })
}

/// Move to the start of the next line, using the leading (`T*`, §9.4.2).
///
/// # Safety
/// `content` must be a live handle from [`prismpdf_content_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_content_next_line(
    content: *mut PrismPdfContent,
) -> PrismPdfStatus {
    let Some(stream) = (unsafe { content_mut(content) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| {
        stream.next_line();
        PrismPdfStatus::Ok
    })
}

/// Open an artifact marked-content sequence (`BMC /Artifact`, §14.8.2.2) — content excluded from the logical structure, which PDF/UA requires for decoration.
///
/// # Safety
/// `content` must be a live handle from [`prismpdf_content_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_content_begin_artifact(
    content: *mut PrismPdfContent,
) -> PrismPdfStatus {
    let Some(stream) = (unsafe { content_mut(content) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| {
        stream.begin_artifact();
        PrismPdfStatus::Ok
    })
}

/// Close the innermost marked-content sequence (`EMC`, §14.6).
///
/// # Safety
/// `content` must be a live handle from [`prismpdf_content_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_content_end_marked_content(
    content: *mut PrismPdfContent,
) -> PrismPdfStatus {
    let Some(stream) = (unsafe { content_mut(content) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| {
        stream.end_marked_content();
        PrismPdfStatus::Ok
    })
}

/// Borrow a content handle and a C string together, or `None` on a null/non-UTF-8 argument.
///
/// # Safety
/// `content` must be a live handle or null; `text` a C string pointer or null.
pub(crate) unsafe fn content_and_str<'a>(
    content: *mut PrismPdfContent,
    text: *const c_char,
) -> Option<(&'a mut Content, &'a str)> {
    if content.is_null() || text.is_null() {
        return None;
    }
    let text = unsafe { utf8(text) }?;
    Some((unsafe { &mut (*content).0 }, text))
}

/// Select a fill colour space by resource name (`cs`, §8.6.8) — the name must be a key in the
/// page's `/Resources /ColorSpace`, e.g. one added by `prismpdf_builder_add_separation`.
///
/// # Safety
/// `content` must be live and `name` a NUL-terminated UTF-8 C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_content_set_fill_color_space(
    content: *mut PrismPdfContent,
    name: *const c_char,
) -> PrismPdfStatus {
    let Some((stream, name)) = (unsafe { content_and_str(content, name) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| {
        stream.set_fill_color_space(name);
        PrismPdfStatus::Ok
    })
}

/// Set fill-colour components in the current colour space (`sc`, §8.6.8): one value for a
/// Separation or Gray space, three for RGB, four for CMYK.
///
/// # Safety
/// `components` must point to `count` readable `double`s (or be null with `count` 0), and
/// `content` must be a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_content_set_fill_color(
    content: *mut PrismPdfContent,
    components: *const f64,
    count: usize,
) -> PrismPdfStatus {
    let Some(stream) = (unsafe { content_mut(content) }) else {
        return PrismPdfStatus::NullArgument;
    };
    if components.is_null() && count != 0 {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        let values = if count == 0 {
            &[][..]
        } else {
            unsafe { std::slice::from_raw_parts(components, count) }
        };
        stream.set_fill_color(values);
        PrismPdfStatus::Ok
    })
}

/// Draw a named XObject (`Do`, §8.8) — an image or form from the page's `/Resources /XObject`.
///
/// # Safety
/// `content` must be live and `name` a NUL-terminated UTF-8 C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_content_do_xobject(
    content: *mut PrismPdfContent,
    name: *const c_char,
) -> PrismPdfStatus {
    let Some((stream, name)) = (unsafe { content_and_str(content, name) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| {
        stream.do_xobject(name);
        PrismPdfStatus::Ok
    })
}

/// Emit an inline image (`BI … ID … EI`, §8.9.7): `cs` is an abbreviated colour-space name
/// (`G`, `RGB`, `CMYK`), `bpc` the bits per component, `data` the raw samples.
///
/// # Safety
/// `content` must be live, `cs` a NUL-terminated UTF-8 C string, and `data` must point to
/// `data_len` readable bytes (or be null with `data_len` 0).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_content_inline_image(
    content: *mut PrismPdfContent,
    width: u32,
    height: u32,
    cs: *const c_char,
    bits_per_component: u32,
    data: *const u8,
    data_len: usize,
) -> PrismPdfStatus {
    let Some((stream, cs)) = (unsafe { content_and_str(content, cs) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| {
        let samples = unsafe { slice_or_empty(data, data_len) };
        stream.inline_image(width, height, cs, bits_per_component, &samples);
        PrismPdfStatus::Ok
    })
}

/// Select a font and size (`Tf`, §9.3.1). `name` must be a key in the page's
/// `/Resources /Font` — one of the names passed to `prismpdf_builder_add_page`.
///
/// # Safety
/// `content` must be live and `name` a NUL-terminated UTF-8 C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_content_set_font(
    content: *mut PrismPdfContent,
    name: *const c_char,
    size: f64,
) -> PrismPdfStatus {
    let Some((stream, name)) = (unsafe { content_and_str(content, name) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| {
        stream.set_font(name, size);
        PrismPdfStatus::Ok
    })
}

/// Show a string of raw character codes (`Tj`, §9.4.3) — the bytes are written as given, so the
/// caller controls the encoding.
///
/// # Safety
/// `content` must be live and `bytes` must point to `len` readable bytes (or be null with `len` 0).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_content_show_text(
    content: *mut PrismPdfContent,
    bytes: *const u8,
    len: usize,
) -> PrismPdfStatus {
    let Some(stream) = (unsafe { content_mut(content) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| {
        let text = unsafe { slice_or_empty(bytes, len) };
        stream.show_text(&text);
        PrismPdfStatus::Ok
    })
}

/// Show UTF-8 text (`Tj`, §9.4.3), encoded for the current Standard-14 font.
///
/// # Safety
/// `content` must be live and `text` a NUL-terminated UTF-8 C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_content_show_str(
    content: *mut PrismPdfContent,
    text: *const c_char,
) -> PrismPdfStatus {
    let Some((stream, text)) = (unsafe { content_and_str(content, text) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| {
        stream.show_str(text);
        PrismPdfStatus::Ok
    })
}

/// Show pre-shaped glyph indices (`Tj` with two-byte codes, §9.4.3) for a composite font embedded
/// with `prismpdf_builder_embed_cid_font`.
///
/// # Safety
/// `content` must be live and `gids` must point to `count` readable `uint16_t`s (or be null with
/// `count` 0).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_content_show_glyphs(
    content: *mut PrismPdfContent,
    gids: *const u16,
    count: usize,
) -> PrismPdfStatus {
    let Some(stream) = (unsafe { content_mut(content) }) else {
        return PrismPdfStatus::NullArgument;
    };
    if gids.is_null() && count != 0 {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        let glyphs = if count == 0 {
            &[][..]
        } else {
            unsafe { std::slice::from_raw_parts(gids, count) }
        };
        stream.show_glyphs(glyphs);
        PrismPdfStatus::Ok
    })
}

/// Open a marked-content sequence tying this content to structure element `mcid`
/// (`BDC`, §14.6) — how tagged content is associated with the structure tree.
///
/// # Safety
/// `content` must be live and `tag` a NUL-terminated UTF-8 C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_content_begin_marked_content(
    content: *mut PrismPdfContent,
    tag: *const c_char,
    mcid: u32,
) -> PrismPdfStatus {
    let Some((stream, tag)) = (unsafe { content_and_str(content, tag) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| {
        stream.begin_marked_content(tag, mcid);
        PrismPdfStatus::Ok
    })
}

/// Open a marked-content sequence associating this content with an embedded file
/// (`BDC` with an `/AF` property, §14.13.9 — **PDF 2.0**). `property` names an entry added by
/// `prismpdf_builder_add_content_af_property`.
///
/// # Safety
/// `content` must be live and `property` a NUL-terminated UTF-8 C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_content_begin_af_marked_content(
    content: *mut PrismPdfContent,
    property: *const c_char,
) -> PrismPdfStatus {
    let Some((stream, property)) = (unsafe { content_and_str(content, property) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| {
        stream.begin_af_marked_content(property);
        PrismPdfStatus::Ok
    })
}

// ---------------------------------------------------------------------------------------------
// Authoring: the document builder (§7.7)
//
// `Builder` mutates in place and `build(&self)` does not consume it, so it crosses as a mutable
// handle exactly like `SignSettings` and `Content`.
//
// Payload-carrying enums (`AnnotationSpec`, `FormFieldSpec`, `LinkTarget`) have no C
// representation. Rather than invent spec handles with move semantics that C callers would have to
// track, the **shallow** ones are flattened: one entry point per variant, taking the variant's
// fields directly. That removes the ownership question entirely. Handles are reserved for the one
// place recursion makes them unavoidable — the structure tree (`StructElem`/`StructKid`).
// ---------------------------------------------------------------------------------------------

/// One of the 14 Standard-14 fonts (§9.6.2.2), which need no embedding.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PrismPdfStdFont {
    /// Helvetica.
    Helvetica = 0,
    /// Helvetica Bold.
    HelveticaBold = 1,
    /// Helvetica Oblique.
    HelveticaOblique = 2,
    /// Helvetica Bold Oblique.
    HelveticaBoldOblique = 3,
    /// Times Roman.
    TimesRoman = 4,
    /// Times Bold.
    TimesBold = 5,
    /// Times Italic.
    TimesItalic = 6,
    /// Times Bold Italic.
    TimesBoldItalic = 7,
    /// Courier.
    Courier = 8,
    /// Courier Bold.
    CourierBold = 9,
    /// Courier Oblique.
    CourierOblique = 10,
    /// Courier Bold Oblique.
    CourierBoldOblique = 11,
    /// Symbol.
    Symbol = 12,
    /// Zapf Dingbats.
    ZapfDingbats = 13,
}

impl From<PrismPdfStdFont> for StdFont {
    fn from(font: PrismPdfStdFont) -> Self {
        match font {
            PrismPdfStdFont::Helvetica => StdFont::Helvetica,
            PrismPdfStdFont::HelveticaBold => StdFont::HelveticaBold,
            PrismPdfStdFont::HelveticaOblique => StdFont::HelveticaOblique,
            PrismPdfStdFont::HelveticaBoldOblique => StdFont::HelveticaBoldOblique,
            PrismPdfStdFont::TimesRoman => StdFont::TimesRoman,
            PrismPdfStdFont::TimesBold => StdFont::TimesBold,
            PrismPdfStdFont::TimesItalic => StdFont::TimesItalic,
            PrismPdfStdFont::TimesBoldItalic => StdFont::TimesBoldItalic,
            PrismPdfStdFont::Courier => StdFont::Courier,
            PrismPdfStdFont::CourierBold => StdFont::CourierBold,
            PrismPdfStdFont::CourierOblique => StdFont::CourierOblique,
            PrismPdfStdFont::CourierBoldOblique => StdFont::CourierBoldOblique,
            PrismPdfStdFont::Symbol => StdFont::Symbol,
            PrismPdfStdFont::ZapfDingbats => StdFont::ZapfDingbats,
        }
    }
}

/// Create an empty raw structure element with `/S tag` (§14.7.4.2).
///
/// # Safety
/// `tag` must be a NUL-terminated UTF-8 string. Release with [`prismpdf_struct_node_free`] unless
/// successfully transferred to a parent or builder.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_struct_node_new(tag: *const c_char) -> *mut PrismPdfStructNode {
    let Some(tag) = (unsafe { utf8(tag) }) else {
        return std::ptr::null_mut();
    };
    let tag = tag.to_string();
    guard_ptr(|| Box::into_raw(Box::new(PrismPdfStructNode(StructElem::new(tag)))))
}

/// Release an untransferred structure node. Freeing `NULL` is a no-op.
///
/// # Safety
/// `node` must be null or a live, untransferred handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_struct_node_free(node: *mut PrismPdfStructNode) {
    unsafe { free_handle(node) }
}

pub(crate) unsafe fn struct_node_text(
    node: *mut PrismPdfStructNode,
    value: *const c_char,
    set: impl FnOnce(&mut StructElem, String),
) -> PrismPdfStatus {
    if node.is_null() || value.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    let Some(value) = (unsafe { utf8(value) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| {
        set(unsafe { &mut (*node).0 }, value.to_string());
        PrismPdfStatus::Ok
    })
}

/// Set alternate text (`/Alt`, §14.9.3).
///
/// # Safety
/// `node` must be live and `value` a NUL-terminated UTF-8 string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_struct_node_set_alt(
    node: *mut PrismPdfStructNode,
    value: *const c_char,
) -> PrismPdfStatus {
    unsafe { struct_node_text(node, value, |elem, value| elem.alt = Some(value)) }
}

/// Set replacement text (`/ActualText`, §14.9.4).
///
/// # Safety
/// `node` must be live and `value` a NUL-terminated UTF-8 string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_struct_node_set_actual_text(
    node: *mut PrismPdfStructNode,
    value: *const c_char,
) -> PrismPdfStatus {
    unsafe { struct_node_text(node, value, |elem, value| elem.actual_text = Some(value)) }
}

/// Set the element language (`/Lang`, §14.9.2).
///
/// # Safety
/// `node` must be live and `value` a NUL-terminated UTF-8 string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_struct_node_set_lang(
    node: *mut PrismPdfStructNode,
    value: *const c_char,
) -> PrismPdfStatus {
    unsafe { struct_node_text(node, value, |elem, value| elem.lang = Some(value)) }
}

/// Set the PDF 2.0 structure namespace URI (`/NS`, §14.7.4).
///
/// # Safety
/// `node` must be live and `value` a NUL-terminated UTF-8 string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_struct_node_set_namespace(
    node: *mut PrismPdfStructNode,
    value: *const c_char,
) -> PrismPdfStatus {
    unsafe { struct_node_text(node, value, |elem, value| elem.ns = Some(value)) }
}

/// Set the element identifier (`/ID`, §14.7.4.2).
///
/// # Safety
/// `node` must be live and `value` a NUL-terminated UTF-8 string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_struct_node_set_id(
    node: *mut PrismPdfStructNode,
    value: *const c_char,
) -> PrismPdfStatus {
    unsafe { struct_node_text(node, value, |elem, value| elem.id = Some(value)) }
}

/// Add an element-ID `/Ref` target (§14.7.4.2, PDF 2.0).
///
/// # Safety
/// `node` must be live and `target_id` a NUL-terminated UTF-8 string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_struct_node_add_reference(
    node: *mut PrismPdfStructNode,
    target_id: *const c_char,
) -> PrismPdfStatus {
    unsafe { struct_node_text(node, target_id, |elem, value| elem.refs.push(value)) }
}

pub(crate) unsafe fn struct_node_add_attr(
    node: *mut PrismPdfStructNode,
    owner: *const c_char,
    key: *const c_char,
    value: AttrValue,
) -> PrismPdfStatus {
    if node.is_null() || owner.is_null() || key.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    let (Some(owner), Some(key)) = (unsafe { utf8(owner) }, unsafe { utf8(key) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| {
        let elem = unsafe { &mut (*node).0 };
        if let Some(attr) = elem.attrs.iter_mut().find(|attr| attr.owner == owner) {
            attr.entries.push((key.to_string(), value));
        } else {
            elem.attrs.push(prismpdf::StructAttr {
                owner: owner.to_string(),
                entries: vec![(key.to_string(), value)],
            });
        }
        PrismPdfStatus::Ok
    })
}

/// Add a name-valued structure attribute (§14.7.6).
///
/// # Safety
/// All strings must be NUL-terminated UTF-8 and `node` live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_struct_node_add_name_attribute(
    node: *mut PrismPdfStructNode,
    owner: *const c_char,
    key: *const c_char,
    value: *const c_char,
) -> PrismPdfStatus {
    let Some(value) = (unsafe { utf8(value) }) else {
        return PrismPdfStatus::NullArgument;
    };
    unsafe { struct_node_add_attr(node, owner, key, AttrValue::Name(value.to_string())) }
}

/// Add an integer-valued structure attribute (§14.7.6).
///
/// # Safety
/// `owner` and `key` must be NUL-terminated UTF-8 and `node` live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_struct_node_add_integer_attribute(
    node: *mut PrismPdfStructNode,
    owner: *const c_char,
    key: *const c_char,
    value: i64,
) -> PrismPdfStatus {
    unsafe { struct_node_add_attr(node, owner, key, AttrValue::Int(value)) }
}

/// Add a text-valued structure attribute (§14.7.6).
///
/// # Safety
/// All strings must be NUL-terminated UTF-8 and `node` live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_struct_node_add_text_attribute(
    node: *mut PrismPdfStructNode,
    owner: *const c_char,
    key: *const c_char,
    value: *const c_char,
) -> PrismPdfStatus {
    let Some(value) = (unsafe { utf8(value) }) else {
        return PrismPdfStatus::NullArgument;
    };
    unsafe { struct_node_add_attr(node, owner, key, AttrValue::Text(value.to_string())) }
}

/// Append a marked-content reference `(page_index, mcid)` (§14.7.4.3).
///
/// # Safety
/// `node` must be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_struct_node_add_content(
    node: *mut PrismPdfStructNode,
    page_index: usize,
    mcid: u32,
) -> PrismPdfStatus {
    if node.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        unsafe { &mut (*node).0 }.push_content(page_index, mcid);
        PrismPdfStatus::Ok
    })
}

/// Append a form-widget `/OBJR` child by builder insertion index (§14.7.4.3).
///
/// # Safety
/// `node` must be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_struct_node_add_widget(
    node: *mut PrismPdfStructNode,
    field_index: usize,
) -> PrismPdfStatus {
    if node.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        unsafe { &mut (*node).0 }.push_widget(field_index);
        PrismPdfStatus::Ok
    })
}

/// Append an annotation `/OBJR` child by builder insertion index (§14.7.4.3).
///
/// # Safety
/// `node` must be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_struct_node_add_annotation(
    node: *mut PrismPdfStructNode,
    annotation_index: usize,
) -> PrismPdfStatus {
    if node.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        unsafe { &mut (*node).0 }.push_annotation(annotation_index);
        PrismPdfStatus::Ok
    })
}

/// Transfer `child` into `parent` in reading order (§14.7.4.2).
///
/// **Consumes on success.** A null rejection leaves ownership unchanged.
///
/// # Safety
/// Both nodes must be distinct live handles.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_struct_node_add_child(
    parent: *mut PrismPdfStructNode,
    child: *mut PrismPdfStructNode,
) -> PrismPdfStatus {
    if parent.is_null() || child.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    if parent == child {
        return PrismPdfStatus::InvalidUse;
    }
    guard(|| {
        let child = unsafe { Box::from_raw(child) };
        unsafe { &mut (*parent).0 }.push_child(child.0);
        PrismPdfStatus::Ok
    })
}

/// Associate an embedded file with this structure element (`/AF`, §14.13.6, PDF 2.0).
/// `description` may be null.
///
/// # Safety
/// `node` must be live; `name`, `mime`, and `relationship` must be NUL-terminated UTF-8;
/// `description` must be UTF-8 or null; `data` readable for `data_len` bytes or null at zero.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_struct_node_associate_file(
    node: *mut PrismPdfStructNode,
    name: *const c_char,
    mime: *const c_char,
    relationship: *const c_char,
    description: *const c_char,
    data: *const u8,
    data_len: usize,
) -> PrismPdfStatus {
    if node.is_null()
        || name.is_null()
        || mime.is_null()
        || relationship.is_null()
        || (data.is_null() && data_len != 0)
    {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        let (Some(name), Some(mime), Some(relationship)) =
            (unsafe { utf8(name) }, unsafe { utf8(mime) }, unsafe {
                utf8(relationship)
            })
        else {
            return PrismPdfStatus::NullArgument;
        };
        let description = if description.is_null() {
            None
        } else {
            let Some(value) = (unsafe { utf8(description) }) else {
                return PrismPdfStatus::NullArgument;
            };
            Some(value.to_string())
        };
        let bytes = unsafe { slice_or_empty(data, data_len) }.into_owned();
        unsafe { &mut (*node).0 }.af.push(Attachment {
            name: name.to_string(),
            mime: mime.to_string(),
            relationship: relationship.to_string(),
            description,
            mod_date: None,
            data: bytes,
        });
        PrismPdfStatus::Ok
    })
}

/// A low-level document under construction (§7.7). Created by [`prismpdf_builder_new`], released by
/// [`prismpdf_builder_free`].
pub struct PrismPdfBuilder(pub(crate) Builder);

/// Create an empty document builder — US Letter pages, no metadata, no pages.
///
/// # Safety
/// The returned handle must be released with [`prismpdf_builder_free`].
#[unsafe(no_mangle)]
pub extern "C" fn prismpdf_builder_new() -> *mut PrismPdfBuilder {
    guard_ptr(|| Box::into_raw(Box::new(PrismPdfBuilder(Builder::new()))))
}

/// Release a builder handle. Freeing `NULL` is a no-op.
///
/// # Safety
/// `builder` must come from [`prismpdf_builder_new`] and must not already be freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_builder_free(builder: *mut PrismPdfBuilder) {
    unsafe { free_handle(builder) }
}

/// Borrow a builder handle for mutation, or `None` when it is null.
///
/// # Safety
/// `builder` must be a live handle or null.
pub(crate) unsafe fn builder_mut<'a>(builder: *mut PrismPdfBuilder) -> Option<&'a mut Builder> {
    if builder.is_null() {
        return None;
    }
    Some(unsafe { &mut (*builder).0 })
}

/// Borrow a builder handle and a C string together, or `None` on a null/non-UTF-8 argument.
///
/// # Safety
/// `builder` must be a live handle or null; `text` a C string pointer or null.
pub(crate) unsafe fn builder_and_str<'a>(
    builder: *mut PrismPdfBuilder,
    text: *const c_char,
) -> Option<(&'a mut Builder, &'a str)> {
    if builder.is_null() || text.is_null() {
        return None;
    }
    let text = unsafe { utf8(text) }?;
    Some((unsafe { &mut (*builder).0 }, text))
}

/// Transfer one top-level raw structure element to the builder (§14.7).
///
/// **Consumes on success.** A null-argument rejection leaves `node` caller-owned.
///
/// # Safety
/// `builder` and `node` must be distinct live handles of their respective types.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_builder_add_structure_node(
    builder: *mut PrismPdfBuilder,
    node: *mut PrismPdfStructNode,
) -> PrismPdfStatus {
    let Some(builder) = (unsafe { builder_mut(builder) }) else {
        return PrismPdfStatus::NullArgument;
    };
    if node.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        let node = unsafe { Box::from_raw(node) };
        builder.add_structure_element(node.0);
        PrismPdfStatus::Ok
    })
}

/// Set the PDF 2.0 namespace URI on the implicit `Document` structure root (§14.7.4).
///
/// # Safety
/// `builder` must be live and `uri` a NUL-terminated UTF-8 string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_builder_set_structure_namespace(
    builder: *mut PrismPdfBuilder,
    uri: *const c_char,
) -> PrismPdfStatus {
    let Some((builder, uri)) = (unsafe { builder_and_str(builder, uri) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| {
        builder.structure_namespace(uri);
        PrismPdfStatus::Ok
    })
}

/// Serialise the document, stamping the **minimum** header version its content requires (§7.5.2)
/// unless one was pinned with [`prismpdf_builder_set_version`].
///
/// The builder is not consumed: keep adding pages and build again.
///
/// # Safety
/// `builder` must be live; `out_data`/`out_len` writable. Release the buffer with
/// [`prismpdf_bytes_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_builder_build(
    builder: *const PrismPdfBuilder,
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
    if builder.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    let handle = unsafe { &(*builder).0 };
    guard(|| emit_bytes(handle.build(), out_data, out_len))
}

/// Serialise declaring exactly the target version `(major, minor)` (§7.5.2), guaranteeing the
/// output contains only constructs valid at that version.
///
/// Constructs above the target are **refused** rather than silently downgraded, so a failure here
/// names a real incompatibility: [`PrismPdfStatus::Parse`].
///
/// # Safety
/// As [`prismpdf_builder_build`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_builder_build_for(
    builder: *const PrismPdfBuilder,
    major: u8,
    minor: u8,
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
    if builder.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    let handle = unsafe { &(*builder).0 };
    guard(|| match handle.build_for(major, minor) {
        Ok(bytes) => emit_bytes(bytes, out_data, out_len),
        Err(_) => PrismPdfStatus::Parse,
    })
}

/// Set the default page box `[llx lly urx ury]` (`/MediaBox`, §7.7.3.3) for pages added after
/// this call. Defaults to US Letter.
///
/// # Safety
/// `builder` must be live and `media_box` must point to 4 readable `double`s.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_builder_set_media_box(
    builder: *mut PrismPdfBuilder,
    media_box: *const f64,
) -> PrismPdfStatus {
    let Some(handle) = (unsafe { builder_mut(builder) }) else {
        return PrismPdfStatus::NullArgument;
    };
    if media_box.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        let mut bounds = [0.0f64; 4];
        unsafe { std::ptr::copy_nonoverlapping(media_box, bounds.as_mut_ptr(), 4) };
        handle.media_box(bounds);
        PrismPdfStatus::Ok
    })
}

/// Pin the header version (§7.5.2). This is a **floor**: `build` never stamps below what the
/// content requires, but an explicit value above the minimum is honoured.
///
/// # Safety
/// `builder` must be a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_builder_set_version(
    builder: *mut PrismPdfBuilder,
    major: u8,
    minor: u8,
) -> PrismPdfStatus {
    let Some(handle) = (unsafe { builder_mut(builder) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| {
        handle.version(major, minor);
        PrismPdfStatus::Ok
    })
}

/// Set an arbitrary `/Info` entry (§14.3.3) by key, replacing any previous value for that key.
///
/// # Safety
/// `builder` must be live; `key` and `value` NUL-terminated UTF-8 C strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_builder_set_info(
    builder: *mut PrismPdfBuilder,
    key: *const c_char,
    value: *const c_char,
) -> PrismPdfStatus {
    let Some((handle, key)) = (unsafe { builder_and_str(builder, key) }) else {
        return PrismPdfStatus::NullArgument;
    };
    if value.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        let Some(value) = (unsafe { utf8(value) }) else {
            return PrismPdfStatus::NullArgument;
        };
        handle.info(key, value);
        PrismPdfStatus::Ok
    })
}

/// Drop every `/Info` entry set so far — PDF/A-4 and PDF 2.0 prefer XMP as the sole metadata
/// source (§14.3).
///
/// # Safety
/// `builder` must be a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_builder_clear_info(
    builder: *mut PrismPdfBuilder,
) -> PrismPdfStatus {
    let Some(handle) = (unsafe { builder_mut(builder) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| {
        handle.clear_info();
        PrismPdfStatus::Ok
    })
}

/// Attach an XMP metadata packet (§14.3.2) as the document's `/Metadata` stream.
///
/// # Safety
/// `builder` must be live and `xmp` must point to `len` readable bytes (or be null with `len` 0).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_builder_set_metadata_xmp(
    builder: *mut PrismPdfBuilder,
    xmp: *const u8,
    len: usize,
) -> PrismPdfStatus {
    let Some(handle) = (unsafe { builder_mut(builder) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| {
        let packet = unsafe { slice_or_empty(xmp, len) };
        handle.metadata_xmp(packet.into_owned());
        PrismPdfStatus::Ok
    })
}

/// Set the document's natural language (`/Lang`, §14.9.2) — required by PDF/UA.
///
/// # Safety
/// `builder` must be live and `code` a NUL-terminated UTF-8 C string (e.g. `en-GB`).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_builder_set_lang(
    builder: *mut PrismPdfBuilder,
    code: *const c_char,
) -> PrismPdfStatus {
    let Some((handle, code)) = (unsafe { builder_and_str(builder, code) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| {
        handle.lang(code);
        PrismPdfStatus::Ok
    })
}

/// Set the permanent file identifier (`/ID` element 1, §14.4) instead of letting the writer derive
/// one from the content.
///
/// # Safety
/// `builder` must be live and `id` must point to `len` readable bytes (or be null with `len` 0).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_builder_set_file_id(
    builder: *mut PrismPdfBuilder,
    id: *const u8,
    len: usize,
) -> PrismPdfStatus {
    let Some(handle) = (unsafe { builder_mut(builder) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| {
        let value = unsafe { slice_or_empty(id, len) };
        handle.file_id(value.into_owned());
        PrismPdfStatus::Ok
    })
}

/// Write text strings as UTF-8 with a BOM (§7.9.2.2) rather than UTF-16BE — a PDF 2.0 form that
/// [`prismpdf_builder_build_for`] downgrades automatically when the target is below 2.0.
///
/// # Safety
/// `builder` must be a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_builder_set_utf8_text_strings(
    builder: *mut PrismPdfBuilder,
) -> PrismPdfStatus {
    let Some(handle) = (unsafe { builder_mut(builder) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| {
        handle.utf8_text_strings();
        PrismPdfStatus::Ok
    })
}

/// Set `/ViewerPreferences /DisplayDocTitle` (§12.2) — PDF/UA requires it so viewers show the
/// document title rather than the file name.
///
/// # Safety
/// `builder` must be a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_builder_set_display_doc_title(
    builder: *mut PrismPdfBuilder,
    on: bool,
) -> PrismPdfStatus {
    let Some(handle) = (unsafe { builder_mut(builder) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| {
        handle.display_doc_title(on);
        PrismPdfStatus::Ok
    })
}

/// Append a page whose content stream is `content`, exposing `font_count` Standard-14 fonts in its
/// `/Resources /Font` under the given names — the names `prismpdf_content_set_font` references.
///
/// `font_names` and `fonts` are parallel arrays of `font_count` entries.
///
/// # Safety
/// `content` must point to `content_len` readable bytes (or be null with `content_len` 0);
/// `font_names` must point to `font_count` non-null C strings and `fonts` to `font_count`
/// readable [`PrismPdfStdFont`] values. `builder` must be a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_builder_add_page(
    builder: *mut PrismPdfBuilder,
    content: *const u8,
    content_len: usize,
    font_names: *const *const c_char,
    fonts: *const PrismPdfStdFont,
    font_count: usize,
) -> PrismPdfStatus {
    let Some(handle) = (unsafe { builder_mut(builder) }) else {
        return PrismPdfStatus::NullArgument;
    };
    if font_count > 0 && (font_names.is_null() || fonts.is_null()) {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        let bytes = unsafe { slice_or_empty(content, content_len) };
        // The facade takes `&[(&str, StdFont)]`, so the names must outlive the call: collect the
        // borrowed strs first, then pair them.
        let mut names: Vec<&str> = Vec::with_capacity(font_count);
        for i in 0..font_count {
            let ptr = unsafe { *font_names.add(i) };
            let Some(name) = (unsafe { utf8(ptr) }) else {
                return PrismPdfStatus::NullArgument;
            };
            names.push(name)
        }
        let mut page = PageSpec::new(bytes.into_owned());
        for (i, name) in names.into_iter().enumerate() {
            page = page.standard_font(name, StdFont::from(unsafe { *fonts.add(i) }));
        }
        handle.add_page(page);
        PrismPdfStatus::Ok
    })
}

/// Transfer an assembled low-level page specification to the builder (§7.7.3.3).
///
/// **Consumes on success**: `page` must not then be used or freed. On failure ownership remains
/// with the caller. The simpler [`prismpdf_builder_add_page`] remains available for content plus
/// Standard-14 fonts.
///
/// # Safety
/// `builder` and `page` must be live handles.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_builder_add_page_spec(
    builder: *mut PrismPdfBuilder,
    page: *mut PrismPdfPageSpec,
) -> PrismPdfStatus {
    let Some(handle) = (unsafe { builder_mut(builder) }) else {
        return PrismPdfStatus::NullArgument;
    };
    if page.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        let page = unsafe { Box::from_raw(page) };
        handle.add_page(page.0);
        PrismPdfStatus::Ok
    })
}

/// Add a top-level bookmark (§12.3.3) titled `title` jumping to `page_index` (0-based).
///
/// # Safety
/// `builder` must be live and `title` a NUL-terminated UTF-8 C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_builder_add_outline(
    builder: *mut PrismPdfBuilder,
    title: *const c_char,
    page_index: usize,
) -> PrismPdfStatus {
    let Some((handle, title)) = (unsafe { builder_and_str(builder, title) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| {
        handle.outline(title, page_index);
        PrismPdfStatus::Ok
    })
}

/// Embed a file (§7.11) and list it in the `/EmbeddedFiles` name tree (§7.7.4).
///
/// `description` may be null. `mime` and `relationship` (`/AFRelationship`, §14.13) are required —
/// pass `"application/octet-stream"` and `"Unspecified"` when nothing better applies.
///
/// # Safety
/// `builder` must be live; `name`, `mime` and `relationship` NUL-terminated UTF-8 C strings;
/// `description` such a string or null; `data` must point to `data_len` readable bytes (or be null
/// with `data_len` 0).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_builder_attach_file(
    builder: *mut PrismPdfBuilder,
    name: *const c_char,
    mime: *const c_char,
    relationship: *const c_char,
    description: *const c_char,
    data: *const u8,
    data_len: usize,
) -> PrismPdfStatus {
    let Some((handle, name)) = (unsafe { builder_and_str(builder, name) }) else {
        return PrismPdfStatus::NullArgument;
    };
    if mime.is_null() || relationship.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        let (Some(mime), Some(relationship)) =
            (unsafe { utf8(mime) }, unsafe { utf8(relationship) })
        else {
            return PrismPdfStatus::NullArgument;
        };
        let Ok(desc) = (unsafe { read_opt_str(description) }) else {
            return PrismPdfStatus::NullArgument;
        };
        let bytes = unsafe { slice_or_empty(data, data_len) };
        handle.attach_file(Attachment {
            name: name.to_string(),
            mime: mime.to_string(),
            relationship: relationship.to_string(),
            description: desc,
            mod_date: None,
            data: bytes.into_owned(),
        });
        PrismPdfStatus::Ok
    })
}

/// Read a `rect` argument as `[llx lly urx ury]`, or `None` when the pointer is null.
///
/// # Safety
/// `rect` must point to 4 readable `double`s, or be null.
pub(crate) unsafe fn read_rect(rect: *const f64) -> Option<[f64; 4]> {
    if rect.is_null() {
        return None;
    }
    let mut bounds = [0.0f64; 4];
    unsafe { std::ptr::copy_nonoverlapping(rect, bounds.as_mut_ptr(), 4) };
    Some(bounds)
}

/// Read an optional C string argument: `None` for a null pointer, an error for invalid UTF-8.
///
/// # Safety
/// `text` must be a NUL-terminated C string or null.
pub(crate) unsafe fn read_opt_str(text: *const c_char) -> Result<Option<String>, ()> {
    if text.is_null() {
        return Ok(None);
    }
    match unsafe { utf8(text) } {
        Some(value) => Ok(Some(value.to_string())),
        None => Err(()),
    }
}

/// Add a hyperlink annotation (§12.5.6.5) over `rect` on page `page_index` pointing at an external
/// `uri`. `contents` is the alternate description PDF/UA wants on links (§7.18.5); it may be null.
///
/// # Safety
/// `builder` must be live, `rect` must point to 4 readable `double`s, `uri` must be a
/// NUL-terminated UTF-8 C string, and `contents` such a string or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_builder_add_link_uri(
    builder: *mut PrismPdfBuilder,
    page_index: usize,
    rect: *const f64,
    uri: *const c_char,
    contents: *const c_char,
) -> PrismPdfStatus {
    let Some((handle, uri)) = (unsafe { builder_and_str(builder, uri) }) else {
        return PrismPdfStatus::NullArgument;
    };
    let Some(rect) = (unsafe { read_rect(rect) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| {
        let Ok(contents) = (unsafe { read_opt_str(contents) }) else {
            return PrismPdfStatus::NullArgument;
        };
        handle.add_annotation(
            page_index,
            AnnotationSpec::Link {
                rect,
                target: LinkTarget::Uri(uri.to_string()),
                contents,
            },
            Vec::new(),
        );
        PrismPdfStatus::Ok
    })
}

/// Add a hyperlink annotation jumping to another page in the same document (§12.3.2).
///
/// # Safety
/// As [`prismpdf_builder_add_link_uri`], without the `uri` argument.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_builder_add_link_page(
    builder: *mut PrismPdfBuilder,
    page_index: usize,
    rect: *const f64,
    target_page: usize,
    contents: *const c_char,
) -> PrismPdfStatus {
    let Some(handle) = (unsafe { builder_mut(builder) }) else {
        return PrismPdfStatus::NullArgument;
    };
    let Some(rect) = (unsafe { read_rect(rect) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| {
        let Ok(contents) = (unsafe { read_opt_str(contents) }) else {
            return PrismPdfStatus::NullArgument;
        };
        handle.add_annotation(
            page_index,
            AnnotationSpec::Link {
                rect,
                target: LinkTarget::Page(target_page),
                contents,
            },
            Vec::new(),
        );
        PrismPdfStatus::Ok
    })
}

/// Add a hyperlink annotation jumping to a **structure element** by its `/ID` (a structure
/// destination, §12.3.2.2 — PDF 2.0), which is what PDF/UA-2 wants instead of a page destination.
///
/// # Safety
/// As [`prismpdf_builder_add_link_uri`], with `element_id` in place of `uri`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_builder_add_link_element(
    builder: *mut PrismPdfBuilder,
    page_index: usize,
    rect: *const f64,
    element_id: *const c_char,
    contents: *const c_char,
) -> PrismPdfStatus {
    let Some((handle, element_id)) = (unsafe { builder_and_str(builder, element_id) }) else {
        return PrismPdfStatus::NullArgument;
    };
    let Some(rect) = (unsafe { read_rect(rect) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| {
        let Ok(contents) = (unsafe { read_opt_str(contents) }) else {
            return PrismPdfStatus::NullArgument;
        };
        handle.add_annotation(
            page_index,
            AnnotationSpec::Link {
                rect,
                target: LinkTarget::Element(element_id.to_string()),
                contents,
            },
            Vec::new(),
        );
        PrismPdfStatus::Ok
    })
}

/// Add a hyperlink annotation jumping to a document part (§14.12 — PDF 2.0).
///
/// # Safety
/// As [`prismpdf_builder_add_link_page`], with a part index in place of a page index.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_builder_add_link_document_part(
    builder: *mut PrismPdfBuilder,
    page_index: usize,
    rect: *const f64,
    part_index: usize,
    contents: *const c_char,
) -> PrismPdfStatus {
    let Some(handle) = (unsafe { builder_mut(builder) }) else {
        return PrismPdfStatus::NullArgument;
    };
    let Some(rect) = (unsafe { read_rect(rect) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| {
        let Ok(contents) = (unsafe { read_opt_str(contents) }) else {
            return PrismPdfStatus::NullArgument;
        };
        handle.add_annotation(
            page_index,
            AnnotationSpec::Link {
                rect,
                target: LinkTarget::DocumentPart(part_index),
                contents,
            },
            Vec::new(),
        );
        PrismPdfStatus::Ok
    })
}

/// Add a text-note annotation (§12.5.6.4) anchored at `rect` carrying `contents` as its body. A
/// normal appearance stream is generated, as PDF/A requires for non-link annotations (§6.3.3).
///
/// # Safety
/// `builder` must be live, `rect` must point to 4 readable `double`s, and `contents` must be a
/// NUL-terminated UTF-8 C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_builder_add_note(
    builder: *mut PrismPdfBuilder,
    page_index: usize,
    rect: *const f64,
    contents: *const c_char,
) -> PrismPdfStatus {
    let Some((handle, contents)) = (unsafe { builder_and_str(builder, contents) }) else {
        return PrismPdfStatus::NullArgument;
    };
    let Some(rect) = (unsafe { read_rect(rect) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| {
        handle.add_annotation(
            page_index,
            AnnotationSpec::Note {
                rect,
                contents: contents.to_string(),
            },
            Vec::new(),
        );
        PrismPdfStatus::Ok
    })
}

/// Add a checkbox form field (`/FT /Btn`, §12.7.4.2.3) as a widget on `page_index`. Its on-state is
/// named `On`, and its appearance is vector-drawn so it needs no font (PDF/A-safe).
///
/// `tooltip` (`/TU`, §12.7.3.1) may be null, but PDF/UA wants one — assistive technology reads it
/// in place of the field name.
///
/// # Safety
/// `builder` must be live, `rect` must point to 4 readable `double`s, `name` must be a
/// NUL-terminated UTF-8 C string, and `tooltip` such a string or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_builder_add_checkbox(
    builder: *mut PrismPdfBuilder,
    page_index: usize,
    rect: *const f64,
    name: *const c_char,
    checked: bool,
    tooltip: *const c_char,
) -> PrismPdfStatus {
    let Some((handle, name)) = (unsafe { builder_and_str(builder, name) }) else {
        return PrismPdfStatus::NullArgument;
    };
    let Some(rect) = (unsafe { read_rect(rect) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| {
        let Ok(tooltip) = (unsafe { read_opt_str(tooltip) }) else {
            return PrismPdfStatus::NullArgument;
        };
        handle.add_form_field(
            page_index,
            FormFieldSpec::Checkbox {
                rect,
                name: name.to_string(),
                checked,
                tooltip,
            },
            Vec::new(),
        );
        PrismPdfStatus::Ok
    })
}

/// Set the document title (`/Title`, §14.3.3) — PDF/UA requires one.
///
/// # Safety
/// `builder` must be live and `value` a NUL-terminated UTF-8 C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_builder_set_title(
    builder: *mut PrismPdfBuilder,
    value: *const c_char,
) -> PrismPdfStatus {
    let Some((handle, value)) = (unsafe { builder_and_str(builder, value) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| {
        handle.title(value);
        PrismPdfStatus::Ok
    })
}

/// Set the author (`/Author`, §14.3.3).
///
/// # Safety
/// `builder` must be live and `value` a NUL-terminated UTF-8 C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_builder_set_author(
    builder: *mut PrismPdfBuilder,
    value: *const c_char,
) -> PrismPdfStatus {
    let Some((handle, value)) = (unsafe { builder_and_str(builder, value) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| {
        handle.author(value);
        PrismPdfStatus::Ok
    })
}

/// Set the subject (`/Subject`, §14.3.3).
///
/// # Safety
/// `builder` must be live and `value` a NUL-terminated UTF-8 C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_builder_set_subject(
    builder: *mut PrismPdfBuilder,
    value: *const c_char,
) -> PrismPdfStatus {
    let Some((handle, value)) = (unsafe { builder_and_str(builder, value) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| {
        handle.subject(value);
        PrismPdfStatus::Ok
    })
}

/// Set the keywords (`/Keywords`, §14.3.3).
///
/// # Safety
/// `builder` must be live and `value` a NUL-terminated UTF-8 C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_builder_set_keywords(
    builder: *mut PrismPdfBuilder,
    value: *const c_char,
) -> PrismPdfStatus {
    let Some((handle, value)) = (unsafe { builder_and_str(builder, value) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| {
        handle.keywords(value);
        PrismPdfStatus::Ok
    })
}

/// Set the creating application (`/Creator`, §14.3.3).
///
/// # Safety
/// `builder` must be live and `value` a NUL-terminated UTF-8 C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_builder_set_creator(
    builder: *mut PrismPdfBuilder,
    value: *const c_char,
) -> PrismPdfStatus {
    let Some((handle, value)) = (unsafe { builder_and_str(builder, value) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| {
        handle.creator(value);
        PrismPdfStatus::Ok
    })
}
