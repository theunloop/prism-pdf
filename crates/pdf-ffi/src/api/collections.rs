use super::*;

// ---------------------------------------------------------------------------------------------
// Annotations (§12.5)
// ---------------------------------------------------------------------------------------------

/// One annotation read from a page. Borrowed from a [`PrismPdfAnnotationList`]; never freed
/// directly. `repr(transparent)` so a `&Annotation` inside the list can be lent as this type.
/// cbindgen:opaque
#[repr(transparent)]
pub struct PrismPdfAnnotation(pub(crate) Annotation);

/// An owned list of annotations. Released by [`prismpdf_annotation_list_free`].
pub struct PrismPdfAnnotationList(pub(crate) Vec<PrismPdfAnnotation>);

/// Read the annotations of page `index` (0-based, §12.5), writing an owned list to `*out_list`.
///
/// A page with no `/Annots` (or an out-of-range `index`) yields an empty list, not an error.
///
/// # Safety
/// `doc` must be a live document handle and `out_list` a writable `*mut PrismPdfAnnotationList`.
/// The returned list must be released with [`prismpdf_annotation_list_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_page_annotations(
    doc: *const PrismPdfDocument,
    index: usize,
    out_list: *mut *mut PrismPdfAnnotationList,
) -> PrismPdfStatus {
    if out_list.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    unsafe { *out_list = std::ptr::null_mut() };
    if doc.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    let document = unsafe { &(*doc).0 };
    guard(|| match prismpdf::page_annotations(document, index) {
        Ok(items) => {
            let list = items.into_iter().map(PrismPdfAnnotation).collect();
            unsafe { *out_list = Box::into_raw(Box::new(PrismPdfAnnotationList(list))) };
            PrismPdfStatus::Ok
        }
        Err(_) => PrismPdfStatus::Parse,
    })
}

/// Number of annotations in `list`.
///
/// # Safety
/// `list` must be a live list handle and `out_len` a writable `*mut usize`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_annotation_list_len(
    list: *const PrismPdfAnnotationList,
    out_len: *mut usize,
) -> PrismPdfStatus {
    if out_len.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    unsafe { *out_len = 0 };
    if list.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        unsafe { *out_len = (*list).0.len() };
        PrismPdfStatus::Ok
    })
}

/// Lend annotation `index` from `list`. The returned pointer is **borrowed**: it stays valid until
/// the list is freed and must not be passed to any `*_free`.
///
/// # Safety
/// `list` must be a live list handle and `out_item` a writable `*mut *const PrismPdfAnnotation`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_annotation_list_get(
    list: *const PrismPdfAnnotationList,
    index: usize,
    out_item: *mut *const PrismPdfAnnotation,
) -> PrismPdfStatus {
    if out_item.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    unsafe { *out_item = std::ptr::null() };
    if list.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    let items = unsafe { &(*list).0 };
    guard(|| match items.get(index) {
        Some(item) => {
            unsafe { *out_item = item as *const PrismPdfAnnotation };
            PrismPdfStatus::Ok
        }
        None => PrismPdfStatus::NotFound,
    })
}

/// Release an annotation list. Freeing `NULL` is a no-op. Any borrowed item pointer obtained from
/// [`prismpdf_annotation_list_get`] is dangling afterwards.
///
/// # Safety
/// `list` must come from [`prismpdf_page_annotations`] and must not already be freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_annotation_list_free(list: *mut PrismPdfAnnotationList) {
    unsafe { free_handle(list) }
}

/// The annotation `/Subtype` (`Link`, `Text`, `Widget`, `Highlight`, …) as an owned C string.
///
/// # Safety
/// `annot` must be borrowed from a live list; `out_text` must be writable. Release the string with
/// [`prismpdf_string_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_annotation_subtype(
    annot: *const PrismPdfAnnotation,
    out_text: *mut *mut c_char,
) -> PrismPdfStatus {
    let Some(item) = (unsafe { prepare_string_out(annot, out_text) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| unsafe { store_string(&item.0.subtype, out_text) })
}

/// The annotation rectangle `[llx lly urx ury]` in default user space (`/Rect`, §12.5.2), written
/// as four `double`s.
///
/// # Safety
/// `annot` must be borrowed from a live list; `out_rect` must point to at least 4 writable
/// `double`s.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_annotation_rect(
    annot: *const PrismPdfAnnotation,
    out_rect: *mut f64,
) -> PrismPdfStatus {
    if annot.is_null() || out_rect.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        let rect = unsafe { (*annot).0.rect };
        unsafe { std::ptr::copy_nonoverlapping(rect.as_ptr(), out_rect, 4) };
        PrismPdfStatus::Ok
    })
}

/// The annotation text contents (`/Contents`, §12.5.2), or [`PrismPdfStatus::NotFound`] when absent.
///
/// # Safety
/// `annot` must be borrowed from a live list; `out_text` must be writable. Release the string with
/// [`prismpdf_string_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_annotation_contents(
    annot: *const PrismPdfAnnotation,
    out_text: *mut *mut c_char,
) -> PrismPdfStatus {
    let Some(item) = (unsafe { prepare_string_out(annot, out_text) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| match item.0.contents.as_deref() {
        Some(text) => unsafe { store_string(text, out_text) },
        None => PrismPdfStatus::NotFound,
    })
}

/// For a link annotation with a URI action (§12.6.4.7), the external URI; otherwise
/// [`PrismPdfStatus::NotFound`].
///
/// # Safety
/// `annot` must be borrowed from a live list; `out_text` must be writable. Release the string with
/// [`prismpdf_string_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_annotation_uri(
    annot: *const PrismPdfAnnotation,
    out_text: *mut *mut c_char,
) -> PrismPdfStatus {
    let Some(item) = (unsafe { prepare_string_out(annot, out_text) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| match item.0.uri.as_deref() {
        Some(text) => unsafe { store_string(text, out_text) },
        None => PrismPdfStatus::NotFound,
    })
}

/// For a link that jumps within the document (§12.3.2), the 0-based target page index; otherwise
/// [`PrismPdfStatus::NotFound`].
///
/// # Safety
/// `annot` must be borrowed from a live list; `out_index` must be a writable `*mut usize`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_annotation_dest_page(
    annot: *const PrismPdfAnnotation,
    out_index: *mut usize,
) -> PrismPdfStatus {
    if out_index.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    unsafe { *out_index = 0 };
    if annot.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| match unsafe { (*annot).0.dest_page } {
        Some(page) => {
            unsafe { *out_index = page };
            PrismPdfStatus::Ok
        }
        None => PrismPdfStatus::NotFound,
    })
}

// ---------------------------------------------------------------------------------------------
// Interactive form fields (§12.7)
// ---------------------------------------------------------------------------------------------

/// One terminal form field. Borrowed from a [`PrismPdfFormFieldList`]; never freed directly.
/// cbindgen:opaque
#[repr(transparent)]
pub struct PrismPdfFormField(pub(crate) FormField);

/// An owned list of form fields. Released by [`prismpdf_form_field_list_free`].
pub struct PrismPdfFormFieldList(pub(crate) Vec<PrismPdfFormField>);

/// Read the document's interactive form fields (§12.7), one entry per terminal field. A document
/// with no AcroForm yields an empty list.
///
/// # Safety
/// `doc` must be a live document handle and `out_list` a writable
/// `*mut PrismPdfFormFieldList`. Release it with [`prismpdf_form_field_list_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_document_form_fields(
    doc: *const PrismPdfDocument,
    out_list: *mut *mut PrismPdfFormFieldList,
) -> PrismPdfStatus {
    if out_list.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    unsafe { *out_list = std::ptr::null_mut() };
    if doc.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    let document = unsafe { &(*doc).0 };
    guard(|| match document.form_fields() {
        Ok(items) => {
            let list = items.into_iter().map(PrismPdfFormField).collect();
            unsafe { *out_list = Box::into_raw(Box::new(PrismPdfFormFieldList(list))) };
            PrismPdfStatus::Ok
        }
        Err(_) => PrismPdfStatus::Parse,
    })
}

/// Number of fields in `list`.
///
/// # Safety
/// `list` must be a live list handle and `out_len` a writable `*mut usize`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_form_field_list_len(
    list: *const PrismPdfFormFieldList,
    out_len: *mut usize,
) -> PrismPdfStatus {
    if out_len.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    unsafe { *out_len = 0 };
    if list.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        unsafe { *out_len = (*list).0.len() };
        PrismPdfStatus::Ok
    })
}

/// Lend field `index` from `list`. Borrowed — valid until the list is freed.
///
/// # Safety
/// `list` must be a live list handle and `out_item` a writable `*mut *const PrismPdfFormField`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_form_field_list_get(
    list: *const PrismPdfFormFieldList,
    index: usize,
    out_item: *mut *const PrismPdfFormField,
) -> PrismPdfStatus {
    if out_item.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    unsafe { *out_item = std::ptr::null() };
    if list.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    let items = unsafe { &(*list).0 };
    guard(|| match items.get(index) {
        Some(item) => {
            unsafe { *out_item = item as *const PrismPdfFormField };
            PrismPdfStatus::Ok
        }
        None => PrismPdfStatus::NotFound,
    })
}

/// Release a form-field list. Freeing `NULL` is a no-op.
///
/// # Safety
/// `list` must come from [`prismpdf_document_form_fields`] and must not already be freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_form_field_list_free(list: *mut PrismPdfFormFieldList) {
    unsafe { free_handle(list) }
}

/// The fully-qualified field name (§12.7.3.2), e.g. `address.city`.
///
/// # Safety
/// `field` must be borrowed from a live list; `out_text` must be writable. Release the string with
/// [`prismpdf_string_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_form_field_name(
    field: *const PrismPdfFormField,
    out_text: *mut *mut c_char,
) -> PrismPdfStatus {
    let Some(item) = (unsafe { prepare_string_out(field, out_text) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| unsafe { store_string(&item.0.name, out_text) })
}

/// The field type from `/FT` (`Tx`, `Btn`, `Ch`, `Sig`); empty when unknown.
///
/// # Safety
/// `field` must be borrowed from a live list; `out_text` must be writable. Release the string with
/// [`prismpdf_string_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_form_field_type(
    field: *const PrismPdfFormField,
    out_text: *mut *mut c_char,
) -> PrismPdfStatus {
    let Some(item) = (unsafe { prepare_string_out(field, out_text) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| unsafe { store_string(&item.0.field_type, out_text) })
}

/// The current value `/V` as text, or [`PrismPdfStatus::NotFound`] when unset or non-textual.
///
/// # Safety
/// `field` must be borrowed from a live list; `out_text` must be writable. Release the string with
/// [`prismpdf_string_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_form_field_value(
    field: *const PrismPdfFormField,
    out_text: *mut *mut c_char,
) -> PrismPdfStatus {
    let Some(item) = (unsafe { prepare_string_out(field, out_text) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| match item.0.value.as_deref() {
        Some(text) => unsafe { store_string(text, out_text) },
        None => PrismPdfStatus::NotFound,
    })
}

/// Fill form fields by fully-qualified name and re-emit the document as an incremental update
/// (§7.5.6), writing the new bytes to `*out_data`/`*out_len`.
///
/// `names` and `values` are parallel arrays of `count` NUL-terminated UTF-8 C strings. Unknown
/// names are ignored. A name or value that is not valid UTF-8 is skipped.
///
/// # Safety
/// `names` and `values` must each point to `count` readable, non-null C string pointers. `doc`
/// must be live; `out_data`/`out_len` must be writable. Release the buffer with
/// [`prismpdf_bytes_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_document_fill_form(
    doc: *const PrismPdfDocument,
    names: *const *const c_char,
    values: *const *const c_char,
    count: usize,
    out_data: *mut *mut u8,
    out_len: *mut usize,
) -> PrismPdfStatus {
    let Some(document) = (unsafe { prepare_bytes_out(doc, out_data, out_len) }) else {
        return PrismPdfStatus::NullArgument;
    };
    if count > 0 && (names.is_null() || values.is_null()) {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        // Collect owned &str pairs first: `fill_form` borrows them for the whole call.
        let mut pairs: Vec<(&str, &str)> = Vec::with_capacity(count);
        for i in 0..count {
            let (name_ptr, value_ptr) = unsafe { (*names.add(i), *values.add(i)) };
            if name_ptr.is_null() || value_ptr.is_null() {
                return PrismPdfStatus::NullArgument;
            }
            let name = unsafe { std::ffi::CStr::from_ptr(name_ptr) };
            let value = unsafe { std::ffi::CStr::from_ptr(value_ptr) };
            match (name.to_str(), value.to_str()) {
                (Ok(n), Ok(v)) => pairs.push((n, v)),
                _ => continue, // non-UTF-8 input is skipped, matching `fill_form`'s tolerance
            }
        }
        match document.fill_form(&pairs) {
            Ok(bytes) => emit_bytes(bytes, out_data, out_len),
            Err(_) => PrismPdfStatus::Parse,
        }
    })
}

/// Incremental form fill returning explicit signature and structure preservation effects.
///
/// # Safety
/// `doc`, `names`, `values`, and `out_report` follow [`prismpdf_document_fill_form`]'s contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_document_fill_form_report(
    doc: *const PrismPdfDocument,
    names: *const *const c_char,
    values: *const *const c_char,
    count: usize,
    out_report: *mut *mut PrismPdfTransformReport,
) -> PrismPdfStatus {
    if doc.is_null() || out_report.is_null() || (count > 0 && (names.is_null() || values.is_null()))
    {
        return PrismPdfStatus::NullArgument;
    }
    unsafe { *out_report = std::ptr::null_mut() };
    guard(|| {
        let mut pairs = Vec::with_capacity(count);
        for index in 0..count {
            let (name, value) = unsafe { (*names.add(index), *values.add(index)) };
            if name.is_null() || value.is_null() {
                return PrismPdfStatus::NullArgument;
            }
            let (Some(name), Some(value)) = (unsafe { utf8(name) }, unsafe { utf8(value) }) else {
                continue;
            };
            pairs.push((name, value));
        }
        store_transform_report(
            unsafe { &(*doc).0 }.fill_form_with_report(&pairs),
            out_report,
        )
    })
}

/// Flatten the interactive form (§12.7): stamp each widget's appearance into its page content,
/// drop `/AcroForm`, and write the rewritten PDF to `*out_data`/`*out_len`.
///
/// # Safety
/// `doc` must be live; `out_data`/`out_len` must be writable. Release the buffer with
/// [`prismpdf_bytes_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_document_flatten_form(
    doc: *const PrismPdfDocument,
    out_data: *mut *mut u8,
    out_len: *mut usize,
) -> PrismPdfStatus {
    let Some(document) = (unsafe { prepare_bytes_out(doc, out_data, out_len) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| match document.flatten_form() {
        Ok(bytes) => emit_bytes(bytes, out_data, out_len),
        Err(_) => PrismPdfStatus::Parse,
    })
}

/// Flatten forms and report the full rewrite plus possible logical-structure invalidation.
///
/// # Safety
/// `doc` must be live and `out_report` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_document_flatten_form_report(
    doc: *const PrismPdfDocument,
    out_report: *mut *mut PrismPdfTransformReport,
) -> PrismPdfStatus {
    if doc.is_null() || out_report.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    unsafe { *out_report = std::ptr::null_mut() };
    guard(|| store_transform_report(unsafe { &(*doc).0 }.flatten_form_with_report(), out_report))
}

// ---------------------------------------------------------------------------------------------
// Document outline / bookmarks (§12.3.3)
//
// The outline is a *tree*, so it needs no per-level list handle: the root list owns everything,
// and a child is lent straight out of its parent. This is the pattern every nested facade type
// (structure elements, name trees) should follow.
// ---------------------------------------------------------------------------------------------

/// One outline entry (bookmark). Borrowed from a [`PrismPdfOutlineList`] or from a parent entry.
/// cbindgen:opaque
#[repr(transparent)]
pub struct PrismPdfOutlineItem(pub(crate) OutlineItem);

/// An owned list of top-level outline entries. Released by [`prismpdf_outline_list_free`].
pub struct PrismPdfOutlineList(pub(crate) Vec<PrismPdfOutlineItem>);

/// Read the document outline (§12.3.3) as a tree of top-level entries. A document without
/// `/Outlines` yields an empty list.
///
/// # Safety
/// `doc` must be live and `out_list` writable. Release it with [`prismpdf_outline_list_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_document_outline(
    doc: *const PrismPdfDocument,
    out_list: *mut *mut PrismPdfOutlineList,
) -> PrismPdfStatus {
    if out_list.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    unsafe { *out_list = std::ptr::null_mut() };
    if doc.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    let document = unsafe { &(*doc).0 };
    guard(|| match document.outline() {
        Ok(items) => {
            let list = items.into_iter().map(PrismPdfOutlineItem).collect();
            unsafe { *out_list = Box::into_raw(Box::new(PrismPdfOutlineList(list))) };
            PrismPdfStatus::Ok
        }
        Err(_) => PrismPdfStatus::Parse,
    })
}

/// Number of top-level entries in `list`.
///
/// # Safety
/// `list` must be a live list handle and `out_len` a writable `*mut usize`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_outline_list_len(
    list: *const PrismPdfOutlineList,
    out_len: *mut usize,
) -> PrismPdfStatus {
    if out_len.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    unsafe { *out_len = 0 };
    if list.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        unsafe { *out_len = (*list).0.len() };
        PrismPdfStatus::Ok
    })
}

/// Lend top-level entry `index` from `list`. Borrowed — valid until the list is freed.
///
/// # Safety
/// `list` must be live and `out_item` a writable `*mut *const PrismPdfOutlineItem`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_outline_list_get(
    list: *const PrismPdfOutlineList,
    index: usize,
    out_item: *mut *const PrismPdfOutlineItem,
) -> PrismPdfStatus {
    if out_item.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    unsafe { *out_item = std::ptr::null() };
    if list.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    let items = unsafe { &(*list).0 };
    guard(|| match items.get(index) {
        Some(item) => {
            unsafe { *out_item = item as *const PrismPdfOutlineItem };
            PrismPdfStatus::Ok
        }
        None => PrismPdfStatus::NotFound,
    })
}

/// Release an outline list. Freeing `NULL` is a no-op. Every borrowed entry — including nested
/// children — is dangling afterwards.
///
/// # Safety
/// `list` must come from [`prismpdf_document_outline`] and must not already be freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_outline_list_free(list: *mut PrismPdfOutlineList) {
    unsafe { free_handle(list) }
}

/// The bookmark title (`/Title`, §7.9.2.2) as an owned C string.
///
/// # Safety
/// `item` must be borrowed from a live outline; `out_text` must be writable. Release the string
/// with [`prismpdf_string_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_outline_item_title(
    item: *const PrismPdfOutlineItem,
    out_text: *mut *mut c_char,
) -> PrismPdfStatus {
    let Some(entry) = (unsafe { prepare_string_out(item, out_text) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| unsafe { store_string(&entry.0.title, out_text) })
}

/// The 0-based page the bookmark jumps to, or [`PrismPdfStatus::NotFound`] when its destination
/// does not resolve.
///
/// # Safety
/// `item` must be borrowed from a live outline; `out_index` must be a writable `*mut usize`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_outline_item_dest_page(
    item: *const PrismPdfOutlineItem,
    out_index: *mut usize,
) -> PrismPdfStatus {
    if out_index.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    unsafe { *out_index = 0 };
    if item.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| match unsafe { (*item).0.dest_page } {
        Some(page) => {
            unsafe { *out_index = page };
            PrismPdfStatus::Ok
        }
        None => PrismPdfStatus::NotFound,
    })
}

/// Number of bookmarks nested directly under `item`.
///
/// # Safety
/// `item` must be borrowed from a live outline; `out_len` must be a writable `*mut usize`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_outline_item_child_count(
    item: *const PrismPdfOutlineItem,
    out_len: *mut usize,
) -> PrismPdfStatus {
    if out_len.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    unsafe { *out_len = 0 };
    if item.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        unsafe { *out_len = (*item).0.children.len() };
        PrismPdfStatus::Ok
    })
}

/// Lend child `index` of `item`. Borrowed from the same allocation as its parent, so it stays
/// valid until the owning list is freed — recurse to any depth without allocating.
///
/// # Safety
/// `item` must be borrowed from a live outline; `out_child` must be a writable
/// `*mut *const PrismPdfOutlineItem`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_outline_item_child(
    item: *const PrismPdfOutlineItem,
    index: usize,
    out_child: *mut *const PrismPdfOutlineItem,
) -> PrismPdfStatus {
    if out_child.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    unsafe { *out_child = std::ptr::null() };
    if item.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    let children = unsafe { &(*item).0.children };
    guard(|| match children.get(index) {
        // `PrismPdfOutlineItem` is `repr(transparent)` over `OutlineItem`, so a child reference
        // inside the parent's `Vec<OutlineItem>` can be lent directly as the wrapper type.
        Some(child) => {
            unsafe { *out_child = std::ptr::from_ref(child).cast::<PrismPdfOutlineItem>() };
            PrismPdfStatus::Ok
        }
        None => PrismPdfStatus::NotFound,
    })
}

// ---------------------------------------------------------------------------------------------
// Embedded files / attachments (§7.11)
//
// Introduces the fourth collection shape: an item that owns **bytes**. Payload getters lend a
// `(ptr, len)` view into the list's own allocation rather than copying — nothing to free, and the
// view dies with the list, exactly like a borrowed item pointer.
// ---------------------------------------------------------------------------------------------

/// One embedded file. Borrowed from a [`PrismPdfAttachmentList`]; never freed directly.
#[repr(transparent)]
pub struct PrismPdfAttachment(pub(crate) ExtractedAttachment);

/// An owned list of embedded files. Released by [`prismpdf_attachment_list_free`].
pub struct PrismPdfAttachmentList(pub(crate) Vec<PrismPdfAttachment>);

/// Read the document's embedded files (§7.11) from the `/EmbeddedFiles` name tree (§7.7.4),
/// decoding each through its filter chain. A document with none yields an empty list.
///
/// # Safety
/// `doc` must be live and `out_list` writable. Release it with [`prismpdf_attachment_list_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_document_attachments(
    doc: *const PrismPdfDocument,
    out_list: *mut *mut PrismPdfAttachmentList,
) -> PrismPdfStatus {
    let Some(document) = (unsafe { prepare_list_out(doc, out_list) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| match document.attachments() {
        Ok(items) => {
            let list = items.into_iter().map(PrismPdfAttachment).collect();
            unsafe { *out_list = Box::into_raw(Box::new(PrismPdfAttachmentList(list))) };
            PrismPdfStatus::Ok
        }
        Err(_) => PrismPdfStatus::Parse,
    })
}

/// Number of embedded files in `list`.
///
/// # Safety
/// `list` must be a live list handle and `out_len` a writable `*mut usize`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_attachment_list_len(
    list: *const PrismPdfAttachmentList,
    out_len: *mut usize,
) -> PrismPdfStatus {
    if out_len.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    unsafe { *out_len = 0 };
    if list.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        unsafe { *out_len = (*list).0.len() };
        PrismPdfStatus::Ok
    })
}

/// Lend embedded file `index` from `list`. Borrowed — valid until the list is freed.
///
/// # Safety
/// `list` must be live and `out_item` a writable `*mut *const PrismPdfAttachment`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_attachment_list_get(
    list: *const PrismPdfAttachmentList,
    index: usize,
    out_item: *mut *const PrismPdfAttachment,
) -> PrismPdfStatus {
    if out_item.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    unsafe { *out_item = std::ptr::null() };
    if list.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    let items = unsafe { &(*list).0 };
    guard(|| match items.get(index) {
        Some(item) => {
            unsafe { *out_item = item as *const PrismPdfAttachment };
            PrismPdfStatus::Ok
        }
        None => PrismPdfStatus::NotFound,
    })
}

/// Release an attachment list. Freeing `NULL` is a no-op. Every borrowed item — and every byte
/// view lent by [`prismpdf_attachment_data`] — is dangling afterwards.
///
/// # Safety
/// `list` must come from [`prismpdf_document_attachments`] and must not already be freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_attachment_list_free(list: *mut PrismPdfAttachmentList) {
    unsafe { free_handle(list) }
}

/// The file name (`/UF` preferred, else `/F`, else the name-tree key) as a decoded text string.
///
/// # Safety
/// `att` must be borrowed from a live list; `out_text` must be writable. Release the string with
/// [`prismpdf_string_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_attachment_name(
    att: *const PrismPdfAttachment,
    out_text: *mut *mut c_char,
) -> PrismPdfStatus {
    let Some(item) = (unsafe { prepare_string_out(att, out_text) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| unsafe { store_string(&item.0.name, out_text) })
}

/// Lend the decoded file bytes. **Borrowed**: the view points into the list's allocation, must not
/// be passed to [`prismpdf_bytes_free`], and dies when the list is freed. An empty file yields a
/// null pointer with length 0.
///
/// # Safety
/// `att` must be borrowed from a live list; `out_data`/`out_len` must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_attachment_data(
    att: *const PrismPdfAttachment,
    out_data: *mut *const u8,
    out_len: *mut usize,
) -> PrismPdfStatus {
    let Some(item) = (unsafe { prepare_borrowed_bytes_out(att, out_data, out_len) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| unsafe { lend_bytes(&item.0.data, out_data, out_len) })
}

/// The embedded file's MIME type (`/EmbeddedFile /Subtype`), or [`PrismPdfStatus::NotFound`].
///
/// # Safety
/// `att` must be borrowed from a live list; `out_text` must be writable. Release the string with
/// [`prismpdf_string_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_attachment_mime(
    att: *const PrismPdfAttachment,
    out_text: *mut *mut c_char,
) -> PrismPdfStatus {
    let Some(item) = (unsafe { prepare_string_out(att, out_text) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| unsafe { store_opt_string(item.0.mime.as_deref(), out_text) })
}

/// How the file relates to the document (`/AFRelationship`, §14.13), or
/// [`PrismPdfStatus::NotFound`].
///
/// # Safety
/// `att` must be borrowed from a live list; `out_text` must be writable. Release the string with
/// [`prismpdf_string_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_attachment_relationship(
    att: *const PrismPdfAttachment,
    out_text: *mut *mut c_char,
) -> PrismPdfStatus {
    let Some(item) = (unsafe { prepare_string_out(att, out_text) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| unsafe { store_opt_string(item.0.relationship.as_deref(), out_text) })
}

/// A human-readable description (`/Desc`), or [`PrismPdfStatus::NotFound`].
///
/// # Safety
/// `att` must be borrowed from a live list; `out_text` must be writable. Release the string with
/// [`prismpdf_string_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_attachment_description(
    att: *const PrismPdfAttachment,
    out_text: *mut *mut c_char,
) -> PrismPdfStatus {
    let Some(item) = (unsafe { prepare_string_out(att, out_text) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| unsafe { store_opt_string(item.0.description.as_deref(), out_text) })
}

// ---------------------------------------------------------------------------------------------
// Fonts (§9.5–§9.7, §9.9)
// ---------------------------------------------------------------------------------------------

/// The format of an embedded font program (§9.9).
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PrismPdfFontFormat {
    /// `/FontFile` — a Type 1 font program.
    Type1 = 0,
    /// `/FontFile2` — a TrueType (sfnt) font program.
    TrueType = 1,
    /// `/FontFile3` with `/Subtype /Type1C` or `/CIDFontType0C` — a bare CFF program.
    Cff = 2,
    /// `/FontFile3` with `/Subtype /OpenType` — an OpenType (sfnt) program.
    OpenType = 3,
}

/// One font used by the document. Borrowed from a [`PrismPdfFontList`]; never freed directly.
#[repr(transparent)]
pub struct PrismPdfFont(pub(crate) FontReport);

/// An owned list of fonts. Released by [`prismpdf_font_list_free`].
pub struct PrismPdfFontList(pub(crate) Vec<PrismPdfFont>);

/// Report every font the document's pages reference (§9.5), with its embedded program where one
/// is present (§9.9).
///
/// # Safety
/// `doc` must be live and `out_list` writable. Release it with [`prismpdf_font_list_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_document_fonts(
    doc: *const PrismPdfDocument,
    out_list: *mut *mut PrismPdfFontList,
) -> PrismPdfStatus {
    let Some(document) = (unsafe { prepare_list_out(doc, out_list) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| match prismpdf::document_fonts(document) {
        Ok(items) => {
            let list = items.into_iter().map(PrismPdfFont).collect();
            unsafe { *out_list = Box::into_raw(Box::new(PrismPdfFontList(list))) };
            PrismPdfStatus::Ok
        }
        Err(_) => PrismPdfStatus::Parse,
    })
}

/// Number of fonts in `list`.
///
/// # Safety
/// `list` must be a live list handle and `out_len` a writable `*mut usize`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_font_list_len(
    list: *const PrismPdfFontList,
    out_len: *mut usize,
) -> PrismPdfStatus {
    if out_len.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    unsafe { *out_len = 0 };
    if list.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        unsafe { *out_len = (*list).0.len() };
        PrismPdfStatus::Ok
    })
}

/// Lend font `index` from `list`. Borrowed — valid until the list is freed.
///
/// # Safety
/// `list` must be live and `out_item` a writable `*mut *const PrismPdfFont`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_font_list_get(
    list: *const PrismPdfFontList,
    index: usize,
    out_item: *mut *const PrismPdfFont,
) -> PrismPdfStatus {
    if out_item.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    unsafe { *out_item = std::ptr::null() };
    if list.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    let items = unsafe { &(*list).0 };
    guard(|| match items.get(index) {
        Some(item) => {
            unsafe { *out_item = item as *const PrismPdfFont };
            PrismPdfStatus::Ok
        }
        None => PrismPdfStatus::NotFound,
    })
}

/// Release a font list. Freeing `NULL` is a no-op.
///
/// # Safety
/// `list` must come from [`prismpdf_document_fonts`] and must not already be freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_font_list_free(list: *mut PrismPdfFontList) {
    unsafe { free_handle(list) }
}

/// The `/BaseFont` name, often carrying a subset tag like `ABCDEF+`.
///
/// # Safety
/// `font` must be borrowed from a live list; `out_text` must be writable. Release the string with
/// [`prismpdf_string_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_font_base_font(
    font: *const PrismPdfFont,
    out_text: *mut *mut c_char,
) -> PrismPdfStatus {
    let Some(item) = (unsafe { prepare_string_out(font, out_text) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| unsafe { store_string(&item.0.base_font, out_text) })
}

/// The font `/Subtype` (`Type1`, `TrueType`, `Type0`, …).
///
/// # Safety
/// `font` must be borrowed from a live list; `out_text` must be writable. Release the string with
/// [`prismpdf_string_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_font_subtype(
    font: *const PrismPdfFont,
    out_text: *mut *mut c_char,
) -> PrismPdfStatus {
    let Some(item) = (unsafe { prepare_string_out(font, out_text) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| unsafe { store_string(&item.0.subtype, out_text) })
}

/// The embedded program's format (§9.9), or [`PrismPdfStatus::NotFound`] when the font is not
/// embedded — the check a PDF/A or PDF/UA pre-flight needs.
///
/// # Safety
/// `font` must be borrowed from a live list; `out_format` must be a writable
/// `*mut PrismPdfFontFormat`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_font_program_format(
    font: *const PrismPdfFont,
    out_format: *mut PrismPdfFontFormat,
) -> PrismPdfStatus {
    if out_format.is_null() || font.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| match unsafe { (*font).0.embedded.as_ref() } {
        Some(embedded) => {
            let format = match embedded.format {
                FontProgramFormat::Type1 => PrismPdfFontFormat::Type1,
                FontProgramFormat::TrueType => PrismPdfFontFormat::TrueType,
                FontProgramFormat::Cff => PrismPdfFontFormat::Cff,
                FontProgramFormat::OpenType => PrismPdfFontFormat::OpenType,
            };
            unsafe { *out_format = format };
            PrismPdfStatus::Ok
        }
        None => PrismPdfStatus::NotFound,
    })
}

/// Lend the embedded program bytes, or [`PrismPdfStatus::NotFound`] when the font is not embedded.
/// **Borrowed**: the view dies with the list and must not be passed to [`prismpdf_bytes_free`].
///
/// # Safety
/// `font` must be borrowed from a live list; `out_data`/`out_len` must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_font_program(
    font: *const PrismPdfFont,
    out_data: *mut *const u8,
    out_len: *mut usize,
) -> PrismPdfStatus {
    let Some(item) = (unsafe { prepare_borrowed_bytes_out(font, out_data, out_len) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| match item.0.embedded.as_ref() {
        Some(embedded) => unsafe { lend_bytes(&embedded.program, out_data, out_len) },
        None => PrismPdfStatus::NotFound,
    })
}

/// Parsed sfnt metrics: design units per em and glyph count. [`PrismPdfStatus::NotFound`] when the
/// font is not embedded, or its program is Type1/CFF or unparseable.
///
/// # Safety
/// `font` must be borrowed from a live list; both out-params must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_font_metrics(
    font: *const PrismPdfFont,
    out_units_per_em: *mut u16,
    out_glyph_count: *mut u16,
) -> PrismPdfStatus {
    if out_units_per_em.is_null() || out_glyph_count.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    unsafe {
        *out_units_per_em = 0;
        *out_glyph_count = 0;
    }
    if font.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(
        || match unsafe { (*font).0.embedded.as_ref() }.and_then(|e| e.metrics.as_ref()) {
            Some(metrics) => {
                unsafe {
                    *out_units_per_em = metrics.units_per_em;
                    *out_glyph_count = metrics.glyph_count;
                }
                PrismPdfStatus::Ok
            }
            None => PrismPdfStatus::NotFound,
        },
    )
}

/// The family name recorded in the embedded program, or [`PrismPdfStatus::NotFound`].
///
/// # Safety
/// `font` must be borrowed from a live list; `out_text` must be writable. Release the string with
/// [`prismpdf_string_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_font_family_name(
    font: *const PrismPdfFont,
    out_text: *mut *mut c_char,
) -> PrismPdfStatus {
    let Some(item) = (unsafe { prepare_string_out(font, out_text) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| {
        let name = item
            .0
            .embedded
            .as_ref()
            .and_then(|e| e.metrics.as_ref())
            .and_then(|m| m.family_name.as_deref());
        unsafe { store_opt_string(name, out_text) }
    })
}

/// Subset every embedded font to the glyphs the document actually uses (§9.9) and return the
/// rewritten PDF in `*out_data`/`*out_len`.
///
/// # Safety
/// `doc` must be live; `out_data`/`out_len` must be writable. Release the buffer with
/// [`prismpdf_bytes_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_document_subset_fonts(
    doc: *const PrismPdfDocument,
    out_data: *mut *mut u8,
    out_len: *mut usize,
) -> PrismPdfStatus {
    let Some(document) = (unsafe { prepare_bytes_out(doc, out_data, out_len) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| match prismpdf::subset_fonts(document) {
        Ok(bytes) => emit_bytes(bytes, out_data, out_len),
        Err(_) => PrismPdfStatus::Parse,
    })
}

/// Font subsetting returning full-rewrite preservation effects with the output.
///
/// # Safety
/// `doc` must be live and `out_report` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_document_subset_fonts_report(
    doc: *const PrismPdfDocument,
    out_report: *mut *mut PrismPdfTransformReport,
) -> PrismPdfStatus {
    if doc.is_null() || out_report.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    unsafe { *out_report = std::ptr::null_mut() };
    guard(|| {
        store_transform_report(
            prismpdf::subset_fonts_with_report(unsafe { &(*doc).0 }),
            out_report,
        )
    })
}

// ---------------------------------------------------------------------------------------------
// Image XObjects (§8.6, §8.9)
// ---------------------------------------------------------------------------------------------

/// An image's colour space (§8.6).
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PrismPdfColorSpace {
    /// `/DeviceGray` — 1 component.
    DeviceGray = 0,
    /// `/DeviceRGB` — 3 components.
    DeviceRgb = 1,
    /// `/DeviceCMYK` — 4 components.
    DeviceCmyk = 2,
    /// Anything else; query the component count with [`prismpdf_image_components`].
    Other = 3,
}

/// How an image's payload is encoded.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PrismPdfImageKind {
    /// Decoded raster samples, row-major.
    Raw = 0,
    /// A complete JPEG file (`DCTDecode`), verbatim.
    Jpeg = 1,
    /// A complete JPEG 2000 file (`JPXDecode`), verbatim.
    Jpeg2000 = 2,
    /// An undecodable JBIG2 codestream (`JBIG2Decode`), verbatim.
    Jbig2 = 3,
}

/// One image XObject drawn by a page. Borrowed from a [`PrismPdfImageList`].
#[repr(transparent)]
pub struct PrismPdfImage(pub(crate) ExtractedImage);

/// An owned list of images. Released by [`prismpdf_image_list_free`].
pub struct PrismPdfImageList(pub(crate) Vec<PrismPdfImage>);

/// Collect the images page `index` draws (§8.6), recursing into form XObjects (§8.10).
///
/// # Safety
/// `doc` must be live and `out_list` writable. Release it with [`prismpdf_image_list_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_page_images(
    doc: *const PrismPdfDocument,
    index: usize,
    out_list: *mut *mut PrismPdfImageList,
) -> PrismPdfStatus {
    let Some(document) = (unsafe { prepare_list_out(doc, out_list) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| match prismpdf::page_images(document, index) {
        Ok(items) => {
            let list = items.into_iter().map(PrismPdfImage).collect();
            unsafe { *out_list = Box::into_raw(Box::new(PrismPdfImageList(list))) };
            PrismPdfStatus::Ok
        }
        Err(_) => PrismPdfStatus::Parse,
    })
}

/// Number of images in `list`.
///
/// # Safety
/// `list` must be a live list handle and `out_len` a writable `*mut usize`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_image_list_len(
    list: *const PrismPdfImageList,
    out_len: *mut usize,
) -> PrismPdfStatus {
    if out_len.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    unsafe { *out_len = 0 };
    if list.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        unsafe { *out_len = (*list).0.len() };
        PrismPdfStatus::Ok
    })
}

/// Lend image `index` from `list`. Borrowed — valid until the list is freed.
///
/// # Safety
/// `list` must be live and `out_item` a writable `*mut *const PrismPdfImage`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_image_list_get(
    list: *const PrismPdfImageList,
    index: usize,
    out_item: *mut *const PrismPdfImage,
) -> PrismPdfStatus {
    if out_item.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    unsafe { *out_item = std::ptr::null() };
    if list.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    let items = unsafe { &(*list).0 };
    guard(|| match items.get(index) {
        Some(item) => {
            unsafe { *out_item = item as *const PrismPdfImage };
            PrismPdfStatus::Ok
        }
        None => PrismPdfStatus::NotFound,
    })
}

/// Release an image list. Freeing `NULL` is a no-op. Every lent payload view dies with it.
///
/// # Safety
/// `list` must come from [`prismpdf_page_images`] and must not already be freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_image_list_free(list: *mut PrismPdfImageList) {
    unsafe { free_handle(list) }
}

/// Sample dimensions and depth: `/Width`, `/Height` and `/BitsPerComponent`.
///
/// # Safety
/// `image` must be borrowed from a live list; all three out-params must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_image_info(
    image: *const PrismPdfImage,
    out_width: *mut u32,
    out_height: *mut u32,
    out_bits_per_component: *mut u8,
) -> PrismPdfStatus {
    if out_width.is_null() || out_height.is_null() || out_bits_per_component.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    unsafe {
        *out_width = 0;
        *out_height = 0;
        *out_bits_per_component = 0;
    }
    if image.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        let info = unsafe { &(*image).0.info };
        unsafe {
            *out_width = info.width;
            *out_height = info.height;
            *out_bits_per_component = info.bits_per_component;
        }
        PrismPdfStatus::Ok
    })
}

/// The image's colour space (§8.6).
///
/// # Safety
/// `image` must be borrowed from a live list; `out_space` must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_image_color_space(
    image: *const PrismPdfImage,
    out_space: *mut PrismPdfColorSpace,
) -> PrismPdfStatus {
    if out_space.is_null() || image.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        let space = match unsafe { (*image).0.info.color_space } {
            ColorSpace::DeviceGray => PrismPdfColorSpace::DeviceGray,
            ColorSpace::DeviceRgb => PrismPdfColorSpace::DeviceRgb,
            ColorSpace::DeviceCmyk => PrismPdfColorSpace::DeviceCmyk,
            _ => PrismPdfColorSpace::Other,
        };
        unsafe { *out_space = space };
        PrismPdfStatus::Ok
    })
}

/// Number of colour components per sample — the value needed to walk `Raw` payload bytes, and the
/// only way to size a [`PrismPdfColorSpace::Other`] space.
///
/// # Safety
/// `image` must be borrowed from a live list; `out_components` must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_image_components(
    image: *const PrismPdfImage,
    out_components: *mut u8,
) -> PrismPdfStatus {
    if out_components.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    unsafe { *out_components = 0 };
    if image.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        unsafe { *out_components = (*image).0.info.color_space.components() };
        PrismPdfStatus::Ok
    })
}

/// How the payload lent by [`prismpdf_image_data`] is encoded.
///
/// # Safety
/// `image` must be borrowed from a live list; `out_kind` must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_image_kind(
    image: *const PrismPdfImage,
    out_kind: *mut PrismPdfImageKind,
) -> PrismPdfStatus {
    if out_kind.is_null() || image.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        let kind = match unsafe { &(*image).0.data } {
            ImageData::Raw(_) => PrismPdfImageKind::Raw,
            ImageData::Jpeg(_) => PrismPdfImageKind::Jpeg,
            ImageData::Jpeg2000(_) => PrismPdfImageKind::Jpeg2000,
            ImageData::Jbig2(_) => PrismPdfImageKind::Jbig2,
        };
        unsafe { *out_kind = kind };
        PrismPdfStatus::Ok
    })
}

/// Lend the image payload — decoded samples for [`PrismPdfImageKind::Raw`], a complete container
/// file otherwise. **Borrowed**: the view dies with the list and must not be freed.
///
/// # Safety
/// `image` must be borrowed from a live list; `out_data`/`out_len` must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_image_data(
    image: *const PrismPdfImage,
    out_data: *mut *const u8,
    out_len: *mut usize,
) -> PrismPdfStatus {
    let Some(item) = (unsafe { prepare_borrowed_bytes_out(image, out_data, out_len) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| {
        let bytes = match &item.0.data {
            ImageData::Raw(b)
            | ImageData::Jpeg(b)
            | ImageData::Jpeg2000(b)
            | ImageData::Jbig2(b) => b,
        };
        unsafe { lend_bytes(bytes, out_data, out_len) }
    })
}

// ---------------------------------------------------------------------------------------------
// Document metadata (§14.3) and positioned text (§9.4)
// ---------------------------------------------------------------------------------------------

/// A parsed PDF date (§7.9.4). `has_utc_offset` is false when the string declares no relationship
/// to UTC, in which case `utc_offset_minutes` is 0 and carries no meaning.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct PrismPdfDate {
    /// Four-digit year.
    pub year: u16,
    /// Month 1–12.
    pub month: u8,
    /// Day 1–31.
    pub day: u8,
    /// Hour 0–23.
    pub hour: u8,
    /// Minute 0–59.
    pub minute: u8,
    /// Second 0–59.
    pub second: u8,
    /// Whether `utc_offset_minutes` is meaningful.
    pub has_utc_offset: bool,
    /// Offset of local time from UTC in minutes (`Z` → 0).
    pub utc_offset_minutes: i16,
}

/// Flatten a parsed [`PdfDate`] into the `#[repr(C)]` shape, collapsing its `Option<i16>` offset
/// into a `has_utc_offset` flag (§7.9.4 lets a date declare no relationship to UTC).
pub(crate) fn convert_date(date: &PdfDate) -> PrismPdfDate {
    PrismPdfDate {
        year: date.year,
        month: date.month,
        day: date.day,
        hour: date.hour,
        minute: date.minute,
        second: date.second,
        has_utc_offset: date.utc_offset_minutes.is_some(),
        utc_offset_minutes: date.utc_offset_minutes.unwrap_or(0),
    }
}

/// The document's XMP metadata packet (§14.3.2) as raw XML, or [`PrismPdfStatus::NotFound`] when
/// there is no `/Metadata` stream.
///
/// # Safety
/// `doc` must be live; `out_text` must be writable. Release the string with
/// [`prismpdf_string_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_document_xmp(
    doc: *const PrismPdfDocument,
    out_text: *mut *mut c_char,
) -> PrismPdfStatus {
    let Some(handle) = (unsafe { prepare_string_out(doc, out_text) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| match handle.0.xmp_metadata() {
        Ok(Some(xml)) => unsafe { store_string(&xml, out_text) },
        Ok(None) => PrismPdfStatus::NotFound,
        Err(_) => PrismPdfStatus::Parse,
    })
}

/// Read one `/Info` entry (§14.3.3) by key — `Title`, `Author`, `Subject`, `Keywords`, `Creator`,
/// `Producer`, … — decoded as a PDF text string (§7.9.2.2, so UTF-16BE and PDF 2.0 UTF-8 packets
/// come back as UTF-8). [`PrismPdfStatus::NotFound`] when there is no `/Info`, no such key, or the
/// value is not a string.
///
/// # Safety
/// `doc` must be live, `key` a NUL-terminated UTF-8 C string, `out_text` writable. Release the
/// string with [`prismpdf_string_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_document_info(
    doc: *const PrismPdfDocument,
    key: *const c_char,
    out_text: *mut *mut c_char,
) -> PrismPdfStatus {
    let Some(handle) = (unsafe { prepare_string_out(doc, out_text) }) else {
        return PrismPdfStatus::NullArgument;
    };
    if key.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        let Some(key) = (unsafe { utf8(key) }) else {
            return PrismPdfStatus::NotFound;
        };
        let document = &handle.0;
        let info = match document.info() {
            Ok(Some(info)) => info,
            Ok(None) => return PrismPdfStatus::NotFound,
            Err(_) => return PrismPdfStatus::Parse,
        };
        let Some(value) = info.get(&Name::from(key)) else {
            return PrismPdfStatus::NotFound;
        };
        match document.resolve(value) {
            Ok(Object::String(s)) => {
                let text = prismpdf::decode_text_string(s.as_bytes());
                unsafe { store_string(&text, out_text) }
            }
            Ok(_) => PrismPdfStatus::NotFound,
            Err(_) => PrismPdfStatus::Parse,
        }
    })
}

/// The document's creation date (`/Info /CreationDate`, §14.3.3), or [`PrismPdfStatus::NotFound`]
/// when absent or unparsable.
///
/// # Safety
/// `doc` must be live and `out_date` a writable `*mut PrismPdfDate`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_document_creation_date(
    doc: *const PrismPdfDocument,
    out_date: *mut PrismPdfDate,
) -> PrismPdfStatus {
    if out_date.is_null() || doc.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    let document = unsafe { &(*doc).0 };
    guard(|| match document.creation_date() {
        Ok(Some(date)) => {
            unsafe { *out_date = convert_date(&date) };
            PrismPdfStatus::Ok
        }
        Ok(None) => PrismPdfStatus::NotFound,
        Err(_) => PrismPdfStatus::Parse,
    })
}

/// The document's modification date (`/Info /ModDate`, §14.3.3), or [`PrismPdfStatus::NotFound`].
///
/// # Safety
/// `doc` must be live and `out_date` a writable `*mut PrismPdfDate`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_document_modification_date(
    doc: *const PrismPdfDocument,
    out_date: *mut PrismPdfDate,
) -> PrismPdfStatus {
    if out_date.is_null() || doc.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    let document = unsafe { &(*doc).0 };
    guard(|| match document.modification_date() {
        Ok(Some(date)) => {
            unsafe { *out_date = convert_date(&date) };
            PrismPdfStatus::Ok
        }
        Ok(None) => PrismPdfStatus::NotFound,
        Err(_) => PrismPdfStatus::Parse,
    })
}

/// Extract page `index`'s text preserving layout — line breaks and horizontal gaps reconstructed
/// from the text matrix (§9.4), rather than the reading-order run that
/// [`prismpdf_page_text`] returns. [`PrismPdfStatus::NotFound`] for an out-of-range index.
///
/// # Safety
/// `doc` must be live; `out_text` must be writable. Release the string with
/// [`prismpdf_string_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_page_text_positioned(
    doc: *const PrismPdfDocument,
    index: usize,
    out_text: *mut *mut c_char,
) -> PrismPdfStatus {
    let Some(handle) = (unsafe { prepare_string_out(doc, out_text) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| match prismpdf::page_text_positioned(&handle.0, index) {
        Ok(Some(text)) => {
            let cleaned: String = text.chars().filter(|&c| c != '\0').collect();
            unsafe { store_string(&cleaned, out_text) }
        }
        Ok(None) => PrismPdfStatus::NotFound,
        Err(_) => PrismPdfStatus::Parse,
    })
}
