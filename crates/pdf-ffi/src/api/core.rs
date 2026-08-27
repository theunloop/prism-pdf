use super::*;

/// An opaque, owned PDF document. Created by [`prismpdf_document_open`], released by
/// [`prismpdf_document_free`]. Never inspect or dereference this from C.
pub struct PrismPdfDocument(pub(crate) Document);

/// An independently owned, read-only COS object (§7.3).
pub struct PrismPdfObject(pub(crate) Object);

/// An owned object-edit transaction tied to one live document handle.
pub struct PrismPdfEdit {
    document_identity: usize,
    changes: Vec<(ObjectId, Object)>,
}

/// An owned raw Tagged-PDF structure node (§14.7), transferred into a parent node or builder.
pub struct PrismPdfStructNode(pub(crate) StructElem);

/// How an object-edit transaction is committed (§7.5–§7.5.6).
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PrismPdfEditCommitMode {
    /// Append a new revision while retaining every original byte.
    Incremental = 0,
    /// Re-emit the live object graph as one normalized revision.
    FullRewrite = 1,
}

/// The exact COS variant stored in a [`PrismPdfObject`] (§7.3.2–§7.3.10).
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PrismPdfObjectKind {
    Null = 0,
    Boolean = 1,
    Integer = 2,
    Real = 3,
    String = 4,
    Name = 5,
    Array = 6,
    Dictionary = 7,
    Stream = 8,
    Reference = 9,
}

/// Owned report describing whether a document opened strictly or through bounded recovery.
pub struct PrismPdfOpenReport(pub(crate) OpenReport);

/// Owned output bytes plus manipulation preservation effects.
pub struct PrismPdfTransformReport(pub(crate) TransformReport);

/// An owned snapshot of one thread's most recent structured FFI failure.
pub struct PrismPdfErrorInfo {
    status: PrismPdfStatus,
    message: String,
}

thread_local! {
    static LAST_ERROR: RefCell<Option<PrismPdfErrorInfo>> = const { RefCell::new(None) };
}

/// How a document was opened.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PrismPdfOpenMode {
    Strict = 0,
    Recovered = 1,
}

/// Why the strict open path switched to recovery.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PrismPdfRecoveryReason {
    XrefParseFailure = 0,
    UnreachableCatalog = 1,
}

/// Serialization strategy used by a manipulation.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PrismPdfRewriteMode {
    Incremental = 0,
    FullRewrite = 1,
    Reconstructed = 2,
}

/// Effect on signatures already present in the source.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PrismPdfSignatureEffect {
    Preserved = 0,
    Invalidated = 1,
    Removed = 2,
}

/// Effect on the logical structure tree (§14.7).
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PrismPdfStructureEffect {
    Preserved = 0,
    Removed = 1,
    Invalidated = 2,
}

/// Result status for every C ABI call. `Ok` is `0`; the non-zero values are **stable** integer
/// codes (DESIGN.md §6) and must not be renumbered.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PrismPdfStatus {
    /// Success.
    Ok = 0,
    /// A required pointer argument was null.
    NullArgument = 1,
    /// The document could not be parsed (even after recovery).
    Parse = 2,
    /// The requested item does not exist (e.g. page index out of range, or no header version).
    NotFound = 3,
    /// An internal error — including a caught panic — occurred.
    Internal = 4,
    /// The document is encrypted and the supplied password (none, by default) is wrong (§7.6).
    Password = 5,
    /// A conformance pass (`prismpdf_builder_make_pdfa`, `_make_pdfua`, `_make_pdfua2`) refused the
    /// document. Distinct from [`PrismPdfStatus::Parse`]: nothing is malformed, a standard's rule is
    /// unmet. The specific rule arrives in the call's `out_issue` parameter as a
    /// [`PrismPdfConformanceIssue`].
    Conformance = 6,
    /// A mutable handle is stale, belongs to a released tree, or has already been finalised.
    InvalidUse = 7,
    /// Declarative composition rejected invalid geometry or could not paginate the element tree.
    Layout = 8,
}

/// Run `body`, converting any panic into [`PrismPdfStatus::Internal`] so nothing unwinds across the
/// FFI boundary (DESIGN.md §6.1).
pub(crate) fn guard(body: impl FnOnce() -> PrismPdfStatus) -> PrismPdfStatus {
    LAST_ERROR.with(|slot| *slot.borrow_mut() = None);
    let status = match std::panic::catch_unwind(AssertUnwindSafe(body)) {
        Ok(status) => status,
        Err(_) => {
            record_failure(
                PrismPdfStatus::Internal,
                "a Rust panic was caught at the FFI boundary",
            );
            PrismPdfStatus::Internal
        }
    };
    if status != PrismPdfStatus::Ok {
        LAST_ERROR.with(|slot| {
            if slot.borrow().is_none() {
                *slot.borrow_mut() = Some(PrismPdfErrorInfo {
                    status,
                    message: default_status_message(status).to_string(),
                });
            }
        });
    }
    status
}

pub(crate) fn record_failure(status: PrismPdfStatus, message: impl Into<String>) {
    LAST_ERROR.with(|slot| {
        *slot.borrow_mut() = Some(PrismPdfErrorInfo {
            status,
            message: message.into(),
        });
    });
}

pub(crate) fn default_status_message(status: PrismPdfStatus) -> &'static str {
    match status {
        PrismPdfStatus::Ok => "success",
        PrismPdfStatus::NullArgument => "a required argument was null or invalid",
        PrismPdfStatus::Parse => "PDF parsing or serialization failed",
        PrismPdfStatus::NotFound => "the requested item was not found",
        PrismPdfStatus::Password => "a password or private key is required or incorrect",
        PrismPdfStatus::Internal => "an internal failure occurred",
        PrismPdfStatus::Conformance => "a conformance requirement was not met",
        PrismPdfStatus::InvalidUse => "a handle was stale, cross-owner, or already finalized",
        PrismPdfStatus::Layout => "layout failed",
    }
}

/// Clone this thread's most recent guarded FFI failure. Returns `NotFound` when the most recent
/// guarded call succeeded or no guarded call has run. The snapshot remains valid across later calls.
///
/// # Safety
/// `out_error` must be writable. Release the result with [`prismpdf_error_info_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_last_error(
    out_error: *mut *mut PrismPdfErrorInfo,
) -> PrismPdfStatus {
    if out_error.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    unsafe { *out_error = std::ptr::null_mut() };
    match std::panic::catch_unwind(AssertUnwindSafe(|| {
        LAST_ERROR.with(|slot| {
            slot.borrow().as_ref().map(|error| PrismPdfErrorInfo {
                status: error.status,
                message: error.message.clone(),
            })
        })
    })) {
        Ok(Some(error)) => {
            unsafe { *out_error = Box::into_raw(Box::new(error)) };
            PrismPdfStatus::Ok
        }
        Ok(None) => PrismPdfStatus::NotFound,
        Err(_) => PrismPdfStatus::Internal,
    }
}

/// Read the stable status associated with an error snapshot.
///
/// # Safety
/// `error` must be live and `out_status` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_error_info_status(
    error: *const PrismPdfErrorInfo,
    out_status: *mut PrismPdfStatus,
) -> PrismPdfStatus {
    if error.is_null() || out_status.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        unsafe { *out_status = (*error).status };
        PrismPdfStatus::Ok
    })
}

/// Copy the diagnostic message as an owned C string released by [`prismpdf_string_free`].
///
/// # Safety
/// `error` must be live and `out_message` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_error_info_message(
    error: *const PrismPdfErrorInfo,
    out_message: *mut *mut c_char,
) -> PrismPdfStatus {
    if error.is_null() || out_message.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    unsafe { *out_message = std::ptr::null_mut() };
    guard(|| {
        let Ok(value) = CString::new(unsafe { &(*error).message }.as_str()) else {
            return PrismPdfStatus::Internal;
        };
        unsafe { *out_message = value.into_raw() };
        PrismPdfStatus::Ok
    })
}

/// Release an owned error snapshot. Freeing `NULL` is a no-op.
///
/// # Safety
/// `error` must be null or a live snapshot returned by [`prismpdf_last_error`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_error_info_free(error: *mut PrismPdfErrorInfo) {
    unsafe { free_handle(error) }
}

/// Pointer-returning counterpart to [`guard`]; constructors report a caught panic as null.
pub(crate) fn guard_ptr<T>(body: impl FnOnce() -> *mut T) -> *mut T {
    std::panic::catch_unwind(AssertUnwindSafe(body)).unwrap_or(std::ptr::null_mut())
}

/// Borrow a caller-supplied NUL-terminated C string as UTF-8. `None` when the pointer is null or
/// the bytes are not valid UTF-8 — the two invalid-argument cases every string-taking FFI entry
/// point must reject the same way.
///
/// # Safety
/// A non-null `ptr` must point to a NUL-terminated allocation that stays live for `'a`.
pub(crate) unsafe fn utf8<'a>(ptr: *const c_char) -> Option<&'a str> {
    if ptr.is_null() {
        return None;
    }
    unsafe { std::ffi::CStr::from_ptr(ptr) }.to_str().ok()
}

/// Shared body of every `prismpdf_*_free` for a `Box`-allocated handle: freeing null is a no-op,
/// and the drop runs under [`guard`] so a panicking destructor cannot unwind across the boundary.
///
/// # Safety
/// A non-null `handle` must be a live, exclusively-owned pointer from `Box::into_raw`.
pub(crate) unsafe fn free_handle<T>(handle: *mut T) {
    if handle.is_null() {
        return;
    }
    let _ = guard(|| {
        drop(unsafe { Box::from_raw(handle) });
        PrismPdfStatus::Ok
    });
}

/// Open a PDF from an in-memory buffer of `len` bytes, writing an owned handle to `*out_doc`.
///
/// On any error `*out_doc` is set to null. The buffer is copied, so the caller may free `data`
/// immediately after this returns.
///
/// # Safety
/// `data` must point to at least `len` readable bytes, and `out_doc` must point to a writable
/// `*mut PrismPdfDocument`. The returned handle must be released with [`prismpdf_document_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_document_open(
    data: *const u8,
    len: usize,
    out_doc: *mut *mut PrismPdfDocument,
) -> PrismPdfStatus {
    if out_doc.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    // Establish the null result before anything that could fail.
    unsafe { *out_doc = std::ptr::null_mut() };
    if data.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    let bytes = unsafe { std::slice::from_raw_parts(data, len) }.to_vec();
    guard(|| store_opened(Document::open(bytes), out_doc))
}

/// Open an encrypted (or plain) PDF, trying `password` (`password_len` bytes, may be null/empty) as
/// both the user and the owner password (§7.6). Returns [`PrismPdfStatus::Password`] when the file
/// is encrypted with a supported handler but the password matches neither.
///
/// # Safety
/// `data`/`password` must point to at least `len`/`password_len` readable bytes (`password` may be
/// null only when `password_len` is 0), and `out_doc` must be a writable `*mut PrismPdfDocument`.
/// The returned handle must be released with [`prismpdf_document_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_document_open_with_password(
    data: *const u8,
    len: usize,
    password: *const u8,
    password_len: usize,
    out_doc: *mut *mut PrismPdfDocument,
) -> PrismPdfStatus {
    if out_doc.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    unsafe { *out_doc = std::ptr::null_mut() };
    if data.is_null() || (password.is_null() && password_len != 0) {
        return PrismPdfStatus::NullArgument;
    }
    let bytes = unsafe { std::slice::from_raw_parts(data, len) }.to_vec();
    let pw = if password.is_null() {
        Vec::new()
    } else {
        unsafe { std::slice::from_raw_parts(password, password_len) }.to_vec()
    };
    guard(|| store_opened(Document::open_with_password(bytes, &pw), out_doc))
}

/// Store a freshly opened document into `out_doc`, mapping the error to a status code.
pub(crate) fn store_opened(
    result: Result<Document, prismpdf::DocError>,
    out_doc: *mut *mut PrismPdfDocument,
) -> PrismPdfStatus {
    match result {
        Ok(doc) => {
            let handle = Box::into_raw(Box::new(PrismPdfDocument(doc)));
            unsafe { *out_doc = handle };
            PrismPdfStatus::Ok
        }
        Err(error @ prismpdf::DocError::NeedsPassword) => {
            record_failure(PrismPdfStatus::Password, error.to_string());
            PrismPdfStatus::Password
        }
        Err(error) => {
            record_failure(PrismPdfStatus::Parse, error.to_string());
            PrismPdfStatus::Parse
        }
    }
}

/// Release a document handle returned by [`prismpdf_document_open`]. Null is ignored. Must not be
/// called twice on the same handle.
///
/// # Safety
/// `doc` must be a handle from [`prismpdf_document_open`] that has not already been freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_document_free(doc: *mut PrismPdfDocument) {
    unsafe { free_handle(doc) }
}

pub(crate) unsafe fn store_object(object: Object, out: *mut *mut PrismPdfObject) -> PrismPdfStatus {
    if out.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    unsafe { *out = Box::into_raw(Box::new(PrismPdfObject(object))) };
    PrismPdfStatus::Ok
}

/// Clone the document catalog as an independently owned object (§7.7.2).
///
/// # Safety
/// `doc` must be live and `out_object` writable. Release the result with
/// [`prismpdf_object_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_document_catalog_object(
    doc: *const PrismPdfDocument,
    out_object: *mut *mut PrismPdfObject,
) -> PrismPdfStatus {
    if out_object.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    unsafe { *out_object = std::ptr::null_mut() };
    if doc.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| match unsafe { &(*doc).0 }.catalog() {
        Ok(dict) => unsafe { store_object(Object::Dictionary(dict), out_object) },
        Err(_) => PrismPdfStatus::Parse,
    })
}

/// Clone one inherited leaf page dictionary as an owned object (§7.7.3.4).
///
/// # Safety
/// As [`prismpdf_document_catalog_object`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_document_page_object(
    doc: *const PrismPdfDocument,
    page_index: usize,
    out_object: *mut *mut PrismPdfObject,
) -> PrismPdfStatus {
    if out_object.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    unsafe { *out_object = std::ptr::null_mut() };
    if doc.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| match unsafe { &(*doc).0 }.pages() {
        Ok(pages) => match pages.get(page_index) {
            Some(page) => unsafe { store_object(Object::Dictionary(page.clone()), out_object) },
            None => PrismPdfStatus::NotFound,
        },
        Err(_) => PrismPdfStatus::Parse,
    })
}

/// Fetch an indirect object by number and generation (§7.3.10). Missing/free objects are returned
/// as an owned null object, matching the Rust facade.
///
/// # Safety
/// As [`prismpdf_document_catalog_object`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_document_get_object(
    doc: *const PrismPdfDocument,
    number: u32,
    generation: u16,
    out_object: *mut *mut PrismPdfObject,
) -> PrismPdfStatus {
    if out_object.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    unsafe { *out_object = std::ptr::null_mut() };
    if doc.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(
        || match unsafe { &(*doc).0 }.get(ObjectId::new(number, generation)) {
            Ok(object) => unsafe { store_object(object, out_object) },
            Err(_) => PrismPdfStatus::Parse,
        },
    )
}

/// Resolve an object through any indirect-reference chain and return an owned direct clone
/// (§7.3.10).
///
/// # Safety
/// `doc` and `object` must be live and `out_object` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_document_resolve_object(
    doc: *const PrismPdfDocument,
    object: *const PrismPdfObject,
    out_object: *mut *mut PrismPdfObject,
) -> PrismPdfStatus {
    if out_object.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    unsafe { *out_object = std::ptr::null_mut() };
    if doc.is_null() || object.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(
        || match unsafe { &(*doc).0 }.resolve(unsafe { &(*object).0 }) {
            Ok(value) => unsafe { store_object(value, out_object) },
            Err(_) => PrismPdfStatus::Parse,
        },
    )
}

/// Release an owned COS object. Freeing `NULL` is a no-op.
///
/// # Safety
/// `object` must be null or a live owned object returned by this API.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_object_free(object: *mut PrismPdfObject) {
    unsafe { free_handle(object) }
}

/// Report the exact COS variant (§7.3).
///
/// # Safety
/// `object` must be live and `out_kind` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_object_kind(
    object: *const PrismPdfObject,
    out_kind: *mut PrismPdfObjectKind,
) -> PrismPdfStatus {
    if object.is_null() || out_kind.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        let kind = match unsafe { &(*object).0 } {
            Object::Null => PrismPdfObjectKind::Null,
            Object::Boolean(_) => PrismPdfObjectKind::Boolean,
            Object::Integer(_) => PrismPdfObjectKind::Integer,
            Object::Real(_) => PrismPdfObjectKind::Real,
            Object::String(_) => PrismPdfObjectKind::String,
            Object::Name(_) => PrismPdfObjectKind::Name,
            Object::Array(_) => PrismPdfObjectKind::Array,
            Object::Dictionary(_) => PrismPdfObjectKind::Dictionary,
            Object::Stream(_) => PrismPdfObjectKind::Stream,
            Object::Reference(_) => PrismPdfObjectKind::Reference,
        };
        unsafe { *out_kind = kind };
        PrismPdfStatus::Ok
    })
}

/// Read a COS boolean; a different object kind returns `InvalidUse` (§7.3.2).
///
/// # Safety
/// `object` must be live and `out_value` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_object_boolean(
    object: *const PrismPdfObject,
    out_value: *mut bool,
) -> PrismPdfStatus {
    if object.is_null() || out_value.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| match unsafe { &(*object).0 } {
        Object::Boolean(value) => {
            unsafe { *out_value = *value };
            PrismPdfStatus::Ok
        }
        _ => PrismPdfStatus::InvalidUse,
    })
}

/// Read a COS integer without coercing a real; a different kind returns `InvalidUse` (§7.3.3).
///
/// # Safety
/// `object` must be live and `out_value` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_object_integer(
    object: *const PrismPdfObject,
    out_value: *mut i64,
) -> PrismPdfStatus {
    if object.is_null() || out_value.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| match unsafe { &(*object).0 } {
        Object::Integer(value) => {
            unsafe { *out_value = *value };
            PrismPdfStatus::Ok
        }
        _ => PrismPdfStatus::InvalidUse,
    })
}

/// Read a COS real without coercing an integer; a different kind returns `InvalidUse` (§7.3.3).
///
/// # Safety
/// `object` must be live and `out_value` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_object_real(
    object: *const PrismPdfObject,
    out_value: *mut f64,
) -> PrismPdfStatus {
    if object.is_null() || out_value.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| match unsafe { &(*object).0 } {
        Object::Real(value) => {
            unsafe { *out_value = *value };
            PrismPdfStatus::Ok
        }
        _ => PrismPdfStatus::InvalidUse,
    })
}

/// Lend the raw bytes of a COS string or name (§7.3.4–§7.3.5). The view remains valid until the
/// object is freed. Other kinds return `InvalidUse`.
///
/// # Safety
/// `object` must be live and both out-params writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_object_bytes(
    object: *const PrismPdfObject,
    out_data: *mut *const u8,
    out_len: *mut usize,
) -> PrismPdfStatus {
    let Some(object) = (unsafe { prepare_borrowed_bytes_out(object, out_data, out_len) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| match &object.0 {
        Object::String(value) => unsafe { lend_bytes(value.as_bytes(), out_data, out_len) },
        Object::Name(value) => unsafe { lend_bytes(value.as_bytes(), out_data, out_len) },
        _ => PrismPdfStatus::InvalidUse,
    })
}

/// Return an array's length (§7.3.6).
///
/// # Safety
/// `object` must be live and `out_len` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_object_array_len(
    object: *const PrismPdfObject,
    out_len: *mut usize,
) -> PrismPdfStatus {
    if object.is_null() || out_len.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| match unsafe { &(*object).0 } {
        Object::Array(array) => {
            unsafe { *out_len = array.len() };
            PrismPdfStatus::Ok
        }
        _ => PrismPdfStatus::InvalidUse,
    })
}

/// Clone one array element as an independent object (§7.3.6).
///
/// # Safety
/// `object` must be live and `out_item` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_object_array_get(
    object: *const PrismPdfObject,
    index: usize,
    out_item: *mut *mut PrismPdfObject,
) -> PrismPdfStatus {
    if object.is_null() || out_item.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    unsafe { *out_item = std::ptr::null_mut() };
    guard(|| match unsafe { &(*object).0 } {
        Object::Array(array) => match array.get(index) {
            Some(item) => unsafe { store_object(item.clone(), out_item) },
            None => PrismPdfStatus::NotFound,
        },
        _ => PrismPdfStatus::InvalidUse,
    })
}

/// Return a dictionary's number of entries, or a stream dictionary's entry count (§7.3.7–§7.3.8).
///
/// # Safety
/// `object` must be live and `out_len` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_object_dictionary_len(
    object: *const PrismPdfObject,
    out_len: *mut usize,
) -> PrismPdfStatus {
    if object.is_null() || out_len.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        let len = match unsafe { &(*object).0 } {
            Object::Dictionary(dict) => dict.len(),
            Object::Stream(stream) => stream.dict().len(),
            _ => return PrismPdfStatus::InvalidUse,
        };
        unsafe { *out_len = len };
        PrismPdfStatus::Ok
    })
}

/// Look up a binary-safe dictionary key and clone its value. Stream objects expose their stream
/// dictionary through the same operation (§7.3.7–§7.3.8).
///
/// # Safety
/// `key` must be readable for `key_len` bytes (or null when zero); `out_value` must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_object_dictionary_get(
    object: *const PrismPdfObject,
    key: *const u8,
    key_len: usize,
    out_value: *mut *mut PrismPdfObject,
) -> PrismPdfStatus {
    if object.is_null() || out_value.is_null() || (key.is_null() && key_len != 0) {
        return PrismPdfStatus::NullArgument;
    }
    unsafe { *out_value = std::ptr::null_mut() };
    let key = if key_len == 0 {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(key, key_len) }
    };
    guard(|| {
        let dict = match unsafe { &(*object).0 } {
            Object::Dictionary(dict) => dict,
            Object::Stream(stream) => stream.dict(),
            _ => return PrismPdfStatus::InvalidUse,
        };
        match dict.get(&Name::from(key.to_vec())) {
            Some(value) => unsafe { store_object(value.clone(), out_value) },
            None => PrismPdfStatus::NotFound,
        }
    })
}

/// Lend a stream's raw, still-encoded bytes (§7.3.8). The view dies with the object handle.
///
/// # Safety
/// `object` must be live and both out-params writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_object_stream_raw(
    object: *const PrismPdfObject,
    out_data: *mut *const u8,
    out_len: *mut usize,
) -> PrismPdfStatus {
    let Some(object) = (unsafe { prepare_borrowed_bytes_out(object, out_data, out_len) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| match &object.0 {
        Object::Stream(stream) => unsafe { lend_bytes(stream.raw(), out_data, out_len) },
        _ => PrismPdfStatus::InvalidUse,
    })
}

/// Read an indirect reference's object and generation numbers (§7.3.10).
///
/// # Safety
/// `object` must be live and both out-params writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_object_reference(
    object: *const PrismPdfObject,
    out_number: *mut u32,
    out_generation: *mut u16,
) -> PrismPdfStatus {
    if object.is_null() || out_number.is_null() || out_generation.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| match unsafe { &(*object).0 } {
        Object::Reference(id) => {
            unsafe {
                *out_number = id.number;
                *out_generation = id.generation;
            }
            PrismPdfStatus::Ok
        }
        _ => PrismPdfStatus::InvalidUse,
    })
}

/// Clone an owned COS object (§7.3). Returns null for a null input.
///
/// # Safety
/// `object` must be live; release the result with [`prismpdf_object_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_object_clone(
    object: *const PrismPdfObject,
) -> *mut PrismPdfObject {
    if object.is_null() {
        return std::ptr::null_mut();
    }
    guard_ptr(|| Box::into_raw(Box::new(PrismPdfObject(unsafe { &(*object).0 }.clone()))))
}

/// Create an owned COS null value (§7.3.9).
#[unsafe(no_mangle)]
pub extern "C" fn prismpdf_object_new_null() -> *mut PrismPdfObject {
    guard_ptr(|| Box::into_raw(Box::new(PrismPdfObject(Object::Null))))
}

/// Create an owned COS boolean value (§7.3.2).
#[unsafe(no_mangle)]
pub extern "C" fn prismpdf_object_new_boolean(value: bool) -> *mut PrismPdfObject {
    guard_ptr(|| Box::into_raw(Box::new(PrismPdfObject(Object::Boolean(value)))))
}

/// Create an owned COS integer value (§7.3.3).
#[unsafe(no_mangle)]
pub extern "C" fn prismpdf_object_new_integer(value: i64) -> *mut PrismPdfObject {
    guard_ptr(|| Box::into_raw(Box::new(PrismPdfObject(Object::Integer(value)))))
}

/// Create an empty owned COS array (§7.3.6).
#[unsafe(no_mangle)]
pub extern "C" fn prismpdf_object_new_array() -> *mut PrismPdfObject {
    guard_ptr(|| Box::into_raw(Box::new(PrismPdfObject(Object::Array(Array::new())))))
}

/// Create an empty owned COS dictionary (§7.3.7).
#[unsafe(no_mangle)]
pub extern "C" fn prismpdf_object_new_dictionary() -> *mut PrismPdfObject {
    guard_ptr(|| {
        Box::into_raw(Box::new(PrismPdfObject(Object::Dictionary(
            Dictionary::new(),
        ))))
    })
}

/// Create an owned indirect reference (§7.3.10).
#[unsafe(no_mangle)]
pub extern "C" fn prismpdf_object_new_reference(
    number: u32,
    generation: u16,
) -> *mut PrismPdfObject {
    guard_ptr(|| {
        Box::into_raw(Box::new(PrismPdfObject(Object::Reference(ObjectId::new(
            number, generation,
        )))))
    })
}

/// Create an owned real-number object. Non-finite values are rejected as null (§7.3.3).
#[unsafe(no_mangle)]
pub extern "C" fn prismpdf_object_new_real(value: f64) -> *mut PrismPdfObject {
    if !value.is_finite() {
        return std::ptr::null_mut();
    }
    guard_ptr(|| Box::into_raw(Box::new(PrismPdfObject(Object::Real(value)))))
}

pub(crate) unsafe fn object_from_bytes(
    data: *const u8,
    len: usize,
    make: impl FnOnce(Vec<u8>) -> Object,
) -> *mut PrismPdfObject {
    if data.is_null() && len != 0 {
        return std::ptr::null_mut();
    }
    let bytes = if len == 0 {
        Vec::new()
    } else {
        unsafe { std::slice::from_raw_parts(data, len) }.to_vec()
    };
    guard_ptr(|| Box::into_raw(Box::new(PrismPdfObject(make(bytes)))))
}

/// Create a binary-safe owned COS string (§7.3.4).
///
/// # Safety
/// `data` must be readable for `len` bytes or null when `len` is zero.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_object_new_string(
    data: *const u8,
    len: usize,
) -> *mut PrismPdfObject {
    unsafe { object_from_bytes(data, len, |bytes| Object::String(PdfString::from(bytes))) }
}

/// Create a binary-safe owned COS name (§7.3.5).
///
/// # Safety
/// As [`prismpdf_object_new_string`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_object_new_name(
    data: *const u8,
    len: usize,
) -> *mut PrismPdfObject {
    unsafe { object_from_bytes(data, len, |bytes| Object::Name(Name::from(bytes))) }
}

/// Append a clone of `value` to an owned array (§7.3.6). Both handles remain caller-owned.
///
/// # Safety
/// `array` and `value` must be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_object_array_push(
    array: *mut PrismPdfObject,
    value: *const PrismPdfObject,
) -> PrismPdfStatus {
    if array.is_null() || value.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| match unsafe { &mut (*array).0 } {
        Object::Array(array) => {
            array.push(unsafe { &(*value).0 }.clone());
            PrismPdfStatus::Ok
        }
        _ => PrismPdfStatus::InvalidUse,
    })
}

/// Insert a cloned value under a binary-safe dictionary key (§7.3.7). Both handles remain owned
/// by the caller.
///
/// # Safety
/// `dictionary` and `value` must be live; `key` readable for `key_len` bytes or null when zero.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_object_dictionary_set(
    dictionary: *mut PrismPdfObject,
    key: *const u8,
    key_len: usize,
    value: *const PrismPdfObject,
) -> PrismPdfStatus {
    if dictionary.is_null() || value.is_null() || (key.is_null() && key_len != 0) {
        return PrismPdfStatus::NullArgument;
    }
    let key = if key_len == 0 {
        Vec::new()
    } else {
        unsafe { std::slice::from_raw_parts(key, key_len) }.to_vec()
    };
    guard(|| {
        let dict = match unsafe { &mut (*dictionary).0 } {
            Object::Dictionary(dict) => dict,
            Object::Stream(stream) => stream.dict_mut(),
            _ => return PrismPdfStatus::InvalidUse,
        };
        dict.insert(Name::from(key), unsafe { &(*value).0 }.clone());
        PrismPdfStatus::Ok
    })
}

/// Create a stream by cloning a dictionary and copying raw, still-encoded bytes (§7.3.8).
///
/// # Safety
/// `dictionary` must be a live dictionary object; `data` readable for `len` bytes or null at zero.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_object_new_stream(
    dictionary: *const PrismPdfObject,
    data: *const u8,
    len: usize,
) -> *mut PrismPdfObject {
    if dictionary.is_null() || (data.is_null() && len != 0) {
        return std::ptr::null_mut();
    }
    let Object::Dictionary(dict) = (unsafe { &(*dictionary).0 }) else {
        return std::ptr::null_mut();
    };
    let bytes = if len == 0 {
        Vec::new()
    } else {
        unsafe { std::slice::from_raw_parts(data, len) }.to_vec()
    };
    let dict = dict.clone();
    guard_ptr(|| {
        Box::into_raw(Box::new(PrismPdfObject(Object::Stream(Stream::new(
            dict, bytes,
        )))))
    })
}

/// Begin an object-edit transaction tied to `doc`. The document must remain live until the edit
/// is freed or successfully committed.
///
/// # Safety
/// `doc` must be a live document handle. Release with [`prismpdf_edit_free`] unless committed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_edit_new(doc: *const PrismPdfDocument) -> *mut PrismPdfEdit {
    if doc.is_null() {
        return std::ptr::null_mut();
    }
    guard_ptr(|| {
        Box::into_raw(Box::new(PrismPdfEdit {
            document_identity: doc as usize,
            changes: Vec::new(),
        }))
    })
}

/// Add or replace one changed indirect object (§7.3.10). The value is cloned. Setting the same
/// object identity again replaces the earlier change.
///
/// # Safety
/// `edit` and `value` must be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_edit_set_object(
    edit: *mut PrismPdfEdit,
    number: u32,
    generation: u16,
    value: *const PrismPdfObject,
) -> PrismPdfStatus {
    if edit.is_null() || value.is_null() || number == 0 {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        let edit = unsafe { &mut *edit };
        let id = ObjectId::new(number, generation);
        let replacement = unsafe { &(*value).0 }.clone();
        if let Some((_, object)) = edit
            .changes
            .iter_mut()
            .find(|(existing, _)| *existing == id)
        {
            *object = replacement;
        } else {
            edit.changes.push((id, replacement));
        }
        PrismPdfStatus::Ok
    })
}

/// Commit an edit as an incremental revision or full rewrite. Success consumes `edit`; any
/// reported failure leaves it caller-owned. The returned transform report owns the output bytes.
///
/// # Safety
/// `doc` must be the same live handle passed to [`prismpdf_edit_new`], `edit` must be live, and
/// `out_report` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_edit_commit(
    edit: *mut PrismPdfEdit,
    doc: *const PrismPdfDocument,
    mode: PrismPdfEditCommitMode,
    out_report: *mut *mut PrismPdfTransformReport,
) -> PrismPdfStatus {
    if edit.is_null() || doc.is_null() || out_report.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    unsafe { *out_report = std::ptr::null_mut() };
    if unsafe { (*edit).document_identity } != doc as usize {
        return PrismPdfStatus::InvalidUse;
    }
    guard(|| {
        let changes = unsafe { &(*edit).changes };
        let result = match mode {
            PrismPdfEditCommitMode::Incremental => {
                unsafe { &(*doc).0 }.save_incremental_with_report(changes)
            }
            PrismPdfEditCommitMode::FullRewrite => {
                let overrides = changes
                    .iter()
                    .map(|(id, object)| (id.number, object.clone()))
                    .collect();
                unsafe { &(*doc).0 }.save_with_overrides_report(&overrides)
            }
        };
        let status = store_transform_report(result, out_report);
        if status == PrismPdfStatus::Ok {
            drop(unsafe { Box::from_raw(edit) });
        }
        status
    })
}

/// Release an uncommitted edit. Freeing `NULL` is a no-op.
///
/// # Safety
/// `edit` must be null or a live, uncommitted edit handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_edit_free(edit: *mut PrismPdfEdit) {
    unsafe { free_handle(edit) }
}

/// Clone the document's open report into an independently owned handle.
///
/// # Safety
/// `doc` must be live and `out_report` writable. Release the result with
/// [`prismpdf_open_report_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_document_open_report(
    doc: *const PrismPdfDocument,
    out_report: *mut *mut PrismPdfOpenReport,
) -> PrismPdfStatus {
    if doc.is_null() || out_report.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    unsafe { *out_report = std::ptr::null_mut() };
    guard(|| {
        let report = unsafe { &(*doc).0 }.open_report().clone();
        unsafe { *out_report = Box::into_raw(Box::new(PrismPdfOpenReport(report))) };
        PrismPdfStatus::Ok
    })
}

/// Read the report's strict/recovered mode.
///
/// # Safety
/// `report` must be live and `out_mode` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_open_report_mode(
    report: *const PrismPdfOpenReport,
    out_mode: *mut PrismPdfOpenMode,
) -> PrismPdfStatus {
    if report.is_null() || out_mode.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        let mode = match unsafe { &(*report).0 }.mode() {
            OpenMode::Strict => PrismPdfOpenMode::Strict,
            OpenMode::Recovered => PrismPdfOpenMode::Recovered,
        };
        unsafe { *out_mode = mode };
        PrismPdfStatus::Ok
    })
}

/// Return the bounded recovery diagnostic count (zero for a strict open).
///
/// # Safety
/// `report` must be live and `out_count` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_open_report_diagnostic_count(
    report: *const PrismPdfOpenReport,
    out_count: *mut usize,
) -> PrismPdfStatus {
    if report.is_null() || out_count.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        unsafe { *out_count = (*report).0.diagnostics().len() };
        PrismPdfStatus::Ok
    })
}

/// Read one recovery diagnostic. `out_has_offset` distinguishes absence from byte zero.
///
/// # Safety
/// `report` must be live and all out-pointers writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_open_report_diagnostic(
    report: *const PrismPdfOpenReport,
    index: usize,
    out_reason: *mut PrismPdfRecoveryReason,
    out_has_offset: *mut bool,
    out_offset: *mut usize,
) -> PrismPdfStatus {
    if report.is_null() || out_reason.is_null() || out_has_offset.is_null() || out_offset.is_null()
    {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        let Some(diagnostic) = (unsafe { &(*report).0 }).diagnostics().get(index) else {
            return PrismPdfStatus::NotFound;
        };
        let reason = match diagnostic.reason {
            RecoveryReason::XrefParseFailure => PrismPdfRecoveryReason::XrefParseFailure,
            RecoveryReason::UnreachableCatalog => PrismPdfRecoveryReason::UnreachableCatalog,
        };
        unsafe {
            *out_reason = reason;
            *out_has_offset = diagnostic.offset.is_some();
            *out_offset = diagnostic.offset.unwrap_or(0);
        }
        PrismPdfStatus::Ok
    })
}

/// Release an owned open report. Null is ignored.
///
/// # Safety
/// `report` must be null or a live handle returned by [`prismpdf_document_open_report`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_open_report_free(report: *mut PrismPdfOpenReport) {
    unsafe { free_handle(report) }
}

pub(crate) fn store_transform_report<E: std::fmt::Display>(
    result: Result<TransformReport, E>,
    out_report: *mut *mut PrismPdfTransformReport,
) -> PrismPdfStatus {
    match result {
        Ok(report) => {
            unsafe { *out_report = Box::into_raw(Box::new(PrismPdfTransformReport(report))) };
            PrismPdfStatus::Ok
        }
        Err(error) => {
            record_failure(PrismPdfStatus::Parse, error.to_string());
            PrismPdfStatus::Parse
        }
    }
}

/// Borrow the output PDF bytes owned by a transform report.
///
/// # Safety
/// `report` must be live and both out-pointers writable. The view dies with `report`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_transform_report_bytes(
    report: *const PrismPdfTransformReport,
    out_data: *mut *const u8,
    out_len: *mut usize,
) -> PrismPdfStatus {
    if report.is_null() || out_data.is_null() || out_len.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        let bytes = unsafe { &(*report).0 }.bytes();
        unsafe {
            *out_data = bytes.as_ptr();
            *out_len = bytes.len();
        }
        PrismPdfStatus::Ok
    })
}

/// Read the transform's serialization strategy.
///
/// # Safety
/// `report` must be live and `out_mode` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_transform_report_rewrite_mode(
    report: *const PrismPdfTransformReport,
    out_mode: *mut PrismPdfRewriteMode,
) -> PrismPdfStatus {
    if report.is_null() || out_mode.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    let mode = match unsafe { &(*report).0 }.rewrite_mode() {
        RewriteMode::Incremental => PrismPdfRewriteMode::Incremental,
        RewriteMode::FullRewrite => PrismPdfRewriteMode::FullRewrite,
        RewriteMode::Reconstructed => PrismPdfRewriteMode::Reconstructed,
    };
    guard(|| {
        unsafe { *out_mode = mode };
        PrismPdfStatus::Ok
    })
}

/// Read the transform's effect on existing signatures.
///
/// # Safety
/// `report` must be live and `out_effect` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_transform_report_signature_effect(
    report: *const PrismPdfTransformReport,
    out_effect: *mut PrismPdfSignatureEffect,
) -> PrismPdfStatus {
    if report.is_null() || out_effect.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    let effect = match unsafe { &(*report).0 }.signature_effect() {
        SignatureEffect::Preserved => PrismPdfSignatureEffect::Preserved,
        SignatureEffect::Invalidated => PrismPdfSignatureEffect::Invalidated,
        SignatureEffect::Removed => PrismPdfSignatureEffect::Removed,
    };
    guard(|| {
        unsafe { *out_effect = effect };
        PrismPdfStatus::Ok
    })
}

/// Read the transform's effect on logical structure (§14.7).
///
/// # Safety
/// `report` must be live and `out_effect` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_transform_report_structure_effect(
    report: *const PrismPdfTransformReport,
    out_effect: *mut PrismPdfStructureEffect,
) -> PrismPdfStatus {
    if report.is_null() || out_effect.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    let effect = match unsafe { &(*report).0 }.structure_effect() {
        StructureEffect::Preserved => PrismPdfStructureEffect::Preserved,
        StructureEffect::Removed => PrismPdfStructureEffect::Removed,
        StructureEffect::Invalidated => PrismPdfStructureEffect::Invalidated,
    };
    guard(|| {
        unsafe { *out_effect = effect };
        PrismPdfStatus::Ok
    })
}

/// Release a transform report and its output bytes. Null is ignored.
///
/// # Safety
/// `report` must be null or a live report returned by this API.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_transform_report_free(report: *mut PrismPdfTransformReport) {
    unsafe { free_handle(report) }
}

/// Write the number of pages to `*out_count`.
///
/// # Safety
/// `doc` must be a live handle and `out_count` a writable `*mut usize`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_document_page_count(
    doc: *const PrismPdfDocument,
    out_count: *mut usize,
) -> PrismPdfStatus {
    if doc.is_null() || out_count.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    let document = unsafe { &(*doc).0 };
    guard(|| match document.page_count() {
        Ok(count) => {
            unsafe { *out_count = count };
            PrismPdfStatus::Ok
        }
        Err(_) => PrismPdfStatus::Parse,
    })
}

/// Write the header version to `*out_major`/`*out_minor`. Returns [`PrismPdfStatus::NotFound`] if
/// the file declared no version.
///
/// # Safety
/// `doc` must be a live handle and the out-pointers writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_document_version(
    doc: *const PrismPdfDocument,
    out_major: *mut u8,
    out_minor: *mut u8,
) -> PrismPdfStatus {
    if doc.is_null() || out_major.is_null() || out_minor.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    let document = unsafe { &(*doc).0 };
    guard(|| match document.version() {
        Some(version) => {
            unsafe {
                *out_major = version.major;
                *out_minor = version.minor;
            }
            PrismPdfStatus::Ok
        }
        None => PrismPdfStatus::NotFound,
    })
}

/// Extract the text of page `index` (0-based) as a newly allocated, NUL-terminated UTF-8 string,
/// written to `*out_text`. Returns [`PrismPdfStatus::NotFound`] if the index is out of range.
///
/// The returned string must be released with [`prismpdf_string_free`]. Any interior NUL bytes in
/// the extracted text are stripped so the C string is well-formed.
///
/// # Safety
/// `doc` must be a live handle and `out_text` a writable `*mut *mut c_char`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_page_text(
    doc: *const PrismPdfDocument,
    index: usize,
    out_text: *mut *mut c_char,
) -> PrismPdfStatus {
    if out_text.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    unsafe { *out_text = std::ptr::null_mut() };
    if doc.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    let document = unsafe { &(*doc).0 };
    guard(|| match prismpdf::page_text(document, index) {
        Ok(Some(text)) => {
            let cleaned: Vec<u8> = text.into_bytes().into_iter().filter(|&b| b != 0).collect();
            match CString::new(cleaned) {
                Ok(cstring) => {
                    unsafe { *out_text = cstring.into_raw() };
                    PrismPdfStatus::Ok
                }
                Err(_) => PrismPdfStatus::Internal,
            }
        }
        Ok(None) => PrismPdfStatus::NotFound,
        Err(_) => PrismPdfStatus::Parse,
    })
}

/// Release a string returned by the library (e.g. by [`prismpdf_page_text`]). Null is ignored.
///
/// # Safety
/// `text` must be a pointer returned by this library and not already freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_string_free(text: *mut c_char) {
    if text.is_null() {
        return;
    }
    let _ = guard(|| {
        drop(unsafe { CString::from_raw(text) });
        PrismPdfStatus::Ok
    });
}

/// The library version as a static, NUL-terminated string. Must **not** be freed.
#[unsafe(no_mangle)]
pub extern "C" fn prismpdf_version() -> *const c_char {
    concat!(env!("CARGO_PKG_VERSION"), "\0")
        .as_ptr()
        .cast::<c_char>()
}

// --- Write / transform: functions returning a freshly allocated PDF byte buffer ---------------

/// Hand a `Vec<u8>` to the caller as `(*out_data, *out_len)`, to be released with
/// [`prismpdf_bytes_free`]. The out-pointers must be non-null (checked by the caller).
pub(crate) fn emit_bytes(
    data: Vec<u8>,
    out_data: *mut *mut u8,
    out_len: *mut usize,
) -> PrismPdfStatus {
    let mut boxed = data.into_boxed_slice();
    let ptr = boxed.as_mut_ptr();
    let len = boxed.len();
    std::mem::forget(boxed);
    unsafe {
        *out_data = ptr;
        *out_len = len;
    }
    PrismPdfStatus::Ok
}

/// Release a byte buffer returned by a `prismpdf_*` write/transform call. Null is ignored; `len` must
/// be the length that was returned alongside `data`.
///
/// # Safety
/// `data`/`len` must be a buffer-and-length pair returned by this library, not already freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_bytes_free(data: *mut u8, len: usize) {
    if data.is_null() {
        return;
    }
    let _ = guard(|| {
        drop(unsafe { Box::from_raw(std::ptr::slice_from_raw_parts_mut(data, len)) });
        PrismPdfStatus::Ok
    });
}

/// Serialise the document to a fresh PDF (full rewrite, classic cross-reference table, §7.5.4),
/// writing the buffer to `*out_data`/`*out_len` (release with [`prismpdf_bytes_free`]).
///
/// # Safety
/// `doc` must be a live handle; `out_data`/`out_len` must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_document_save(
    doc: *const PrismPdfDocument,
    out_data: *mut *mut u8,
    out_len: *mut usize,
) -> PrismPdfStatus {
    let Some(document) = (unsafe { prepare_bytes_out(doc, out_data, out_len) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| match document.save() {
        Ok(bytes) => emit_bytes(bytes, out_data, out_len),
        Err(_) => PrismPdfStatus::Parse,
    })
}

/// Full-rewrite save returning owned bytes and explicit preservation effects.
///
/// # Safety
/// `doc` must be live and `out_report` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_document_save_report(
    doc: *const PrismPdfDocument,
    out_report: *mut *mut PrismPdfTransformReport,
) -> PrismPdfStatus {
    if doc.is_null() || out_report.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    unsafe { *out_report = std::ptr::null_mut() };
    guard(|| store_transform_report(unsafe { &(*doc).0 }.save_with_report(), out_report))
}

/// As [`prismpdf_document_save`] but with a compact cross-reference **stream** (§7.5.8).
///
/// # Safety
/// `doc` must be a live handle; `out_data`/`out_len` must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_document_save_compact(
    doc: *const PrismPdfDocument,
    out_data: *mut *mut u8,
    out_len: *mut usize,
) -> PrismPdfStatus {
    let Some(document) = (unsafe { prepare_bytes_out(doc, out_data, out_len) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| match document.save_compact() {
        Ok(bytes) => emit_bytes(bytes, out_data, out_len),
        Err(_) => PrismPdfStatus::Parse,
    })
}

/// Cross-reference-stream full rewrite returning explicit preservation effects.
///
/// # Safety
/// `doc` must be live and `out_report` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_document_save_compact_report(
    doc: *const PrismPdfDocument,
    out_report: *mut *mut PrismPdfTransformReport,
) -> PrismPdfStatus {
    if doc.is_null() || out_report.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    unsafe { *out_report = std::ptr::null_mut() };
    guard(|| store_transform_report(unsafe { &(*doc).0 }.save_compact_with_report(), out_report))
}

/// Serialise the document **encrypted** with the standard security handler (§7.6): `algorithm` is
/// `0` = RC4-128, `1` = AES-128, `2` = AES-256. Each password may be null (with length 0) and is
/// tried as both the user and owner password on reopen. Result via `*out_data`/`*out_len`.
///
/// # Safety
/// `doc` must be a live handle; the password pointers must be valid for their lengths (or null with
/// length 0); `out_data`/`out_len` must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_document_save_encrypted(
    doc: *const PrismPdfDocument,
    user_password: *const u8,
    user_len: usize,
    owner_password: *const u8,
    owner_len: usize,
    algorithm: u32,
    out_data: *mut *mut u8,
    out_len: *mut usize,
) -> PrismPdfStatus {
    let Some(document) = (unsafe { prepare_bytes_out(doc, out_data, out_len) }) else {
        return PrismPdfStatus::NullArgument;
    };
    if (user_password.is_null() && user_len != 0) || (owner_password.is_null() && owner_len != 0) {
        return PrismPdfStatus::NullArgument;
    }
    let Some(algorithm) = algorithm_from_code(algorithm) else {
        return PrismPdfStatus::NullArgument;
    };
    let user = unsafe { slice_or_empty(user_password, user_len) };
    let owner = unsafe { slice_or_empty(owner_password, owner_len) };
    guard(|| match document.save_encrypted(&user, &owner, algorithm) {
        Ok(bytes) => emit_bytes(bytes, out_data, out_len),
        Err(_) => PrismPdfStatus::Parse,
    })
}

/// Serialise a new PDF containing only the pages at the 0-based `indices` (in the given order,
/// §7.7.3 — duplicates allowed). Result via `*out_data`/`*out_len`.
///
/// # Safety
/// `doc` must be a live handle; `indices` must point to `count` readable `usize`s (or be null when
/// `count` is 0); `out_data`/`out_len` must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_document_extract_pages(
    doc: *const PrismPdfDocument,
    indices: *const usize,
    count: usize,
    out_data: *mut *mut u8,
    out_len: *mut usize,
) -> PrismPdfStatus {
    let Some(document) = (unsafe { prepare_bytes_out(doc, out_data, out_len) }) else {
        return PrismPdfStatus::NullArgument;
    };
    if indices.is_null() && count != 0 {
        return PrismPdfStatus::NullArgument;
    }
    let pages = if count == 0 {
        Vec::new()
    } else {
        unsafe { std::slice::from_raw_parts(indices, count) }.to_vec()
    };
    guard(|| match document.extract_pages(&pages) {
        Ok(bytes) => emit_bytes(bytes, out_data, out_len),
        Err(_) => PrismPdfStatus::Parse,
    })
}

/// Page extraction returning reconstruction, signature, and structure effects with its bytes.
///
/// # Safety
/// `doc`, `indices`, and `out_report` follow [`prismpdf_document_extract_pages`]'s contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_document_extract_pages_report(
    doc: *const PrismPdfDocument,
    indices: *const usize,
    count: usize,
    out_report: *mut *mut PrismPdfTransformReport,
) -> PrismPdfStatus {
    if doc.is_null() || out_report.is_null() || (indices.is_null() && count != 0) {
        return PrismPdfStatus::NullArgument;
    }
    unsafe { *out_report = std::ptr::null_mut() };
    let pages = if count == 0 {
        Vec::new()
    } else {
        unsafe { std::slice::from_raw_parts(indices, count) }.to_vec()
    };
    guard(|| {
        store_transform_report(
            unsafe { &(*doc).0 }.extract_pages_with_report(&pages),
            out_report,
        )
    })
}

/// Serialise the document with page `index` rotated by `degrees` (a multiple of 90, §7.7.3.3).
/// Result via `*out_data`/`*out_len`.
///
/// # Safety
/// `doc` must be a live handle; `out_data`/`out_len` must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_document_rotate_page(
    doc: *const PrismPdfDocument,
    index: usize,
    degrees: i64,
    out_data: *mut *mut u8,
    out_len: *mut usize,
) -> PrismPdfStatus {
    let Some(document) = (unsafe { prepare_bytes_out(doc, out_data, out_len) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| match document.rotate_page(index, degrees) {
        Ok(bytes) => emit_bytes(bytes, out_data, out_len),
        Err(_) => PrismPdfStatus::Parse,
    })
}

/// Rotation returning full-rewrite, signature, and structure effects with its bytes.
///
/// # Safety
/// `doc` must be live and `out_report` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_document_rotate_page_report(
    doc: *const PrismPdfDocument,
    index: usize,
    degrees: i64,
    out_report: *mut *mut PrismPdfTransformReport,
) -> PrismPdfStatus {
    if doc.is_null() || out_report.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    unsafe { *out_report = std::ptr::null_mut() };
    guard(|| {
        store_transform_report(
            unsafe { &(*doc).0 }.rotate_page_with_report(index, degrees),
            out_report,
        )
    })
}

/// Merge into a reconstructed graph and return explicit removal effects.
///
/// # Safety
/// `docs` must contain `count` live handles and `out_report` must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_merge_report(
    docs: *const *const PrismPdfDocument,
    count: usize,
    out_report: *mut *mut PrismPdfTransformReport,
) -> PrismPdfStatus {
    if docs.is_null() || out_report.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    unsafe { *out_report = std::ptr::null_mut() };
    guard(|| {
        let handles = unsafe { std::slice::from_raw_parts(docs, count) };
        if handles.iter().any(|handle| handle.is_null()) {
            return PrismPdfStatus::NullArgument;
        }
        let documents: Vec<&Document> = handles
            .iter()
            .map(|handle| unsafe { &(**handle).0 })
            .collect();
        store_transform_report(prismpdf::merge_with_report(&documents), out_report)
    })
}

/// Concatenate `count` documents (in order) into one new PDF (§7.7.3). Result via
/// `*out_data`/`*out_len`.
///
/// # Safety
/// `docs` must point to `count` live, non-null document handles; `out_data`/`out_len` must be
/// writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_merge(
    docs: *const *const PrismPdfDocument,
    count: usize,
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
    if docs.is_null() || count == 0 {
        return PrismPdfStatus::NullArgument;
    }
    let handles = unsafe { std::slice::from_raw_parts(docs, count) };
    if handles.iter().any(|h| h.is_null()) {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        let documents: Vec<&Document> = handles.iter().map(|&h| unsafe { &(*h).0 }).collect();
        match prismpdf::merge(&documents) {
            Ok(bytes) => emit_bytes(bytes, out_data, out_len),
            Err(_) => PrismPdfStatus::Parse,
        }
    })
}

/// Extract the reading-order text of the whole document (all pages joined by form feeds, §9.4) as a
/// NUL-terminated UTF-8 string written to `*out_text` (release with [`prismpdf_string_free`]).
///
/// # Safety
/// `doc` must be a live handle; `out_text` must be a writable `*mut *mut c_char`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_document_text(
    doc: *const PrismPdfDocument,
    out_text: *mut *mut c_char,
) -> PrismPdfStatus {
    if out_text.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    unsafe { *out_text = std::ptr::null_mut() };
    if doc.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    let document = unsafe { &(*doc).0 };
    guard(|| match prismpdf::document_text(document) {
        Ok(text) => {
            let cleaned: Vec<u8> = text.into_bytes().into_iter().filter(|&b| b != 0).collect();
            match CString::new(cleaned) {
                Ok(cstring) => {
                    unsafe { *out_text = cstring.into_raw() };
                    PrismPdfStatus::Ok
                }
                Err(_) => PrismPdfStatus::Internal,
            }
        }
        Err(_) => PrismPdfStatus::Parse,
    })
}

/// Null-check a document handle and its byte-output pointers, zeroing the outputs and returning the
/// borrowed [`Document`] on success (`None` means a null argument).
///
/// # Safety
/// `doc` is a document handle (or null); `out_data`/`out_len` are out-pointers (or null).
pub(crate) unsafe fn prepare_bytes_out<'a>(
    doc: *const PrismPdfDocument,
    out_data: *mut *mut u8,
    out_len: *mut usize,
) -> Option<&'a Document> {
    if out_data.is_null() || out_len.is_null() {
        return None;
    }
    unsafe {
        *out_data = std::ptr::null_mut();
        *out_len = 0;
    }
    if doc.is_null() {
        return None;
    }
    Some(unsafe { &(*doc).0 })
}

/// Borrow `ptr`/`len` as a byte slice, or an empty slice when `ptr` is null.
///
/// # Safety
/// `ptr` must point to at least `len` readable bytes unless it is null.
pub(crate) unsafe fn slice_or_empty<'a>(ptr: *const u8, len: usize) -> std::borrow::Cow<'a, [u8]> {
    if ptr.is_null() {
        std::borrow::Cow::Borrowed(&[])
    } else {
        std::borrow::Cow::Borrowed(unsafe { std::slice::from_raw_parts(ptr, len) })
    }
}

/// Null-check a document handle and a list out-pointer, zeroing the output and returning the
/// borrowed [`Document`]. Shared by every collection producer.
///
/// # Safety
/// `doc` is a document handle (or null); `out_list` is an out-pointer (or null).
pub(crate) unsafe fn prepare_list_out<'a, L>(
    doc: *const PrismPdfDocument,
    out_list: *mut *mut L,
) -> Option<&'a Document> {
    if out_list.is_null() {
        return None;
    }
    unsafe { *out_list = std::ptr::null_mut() };
    if doc.is_null() {
        return None;
    }
    Some(unsafe { &(*doc).0 })
}

/// Write an optional string: `Some` stores it, `None` reports [`PrismPdfStatus::NotFound`] and
/// leaves the out-param null. Absence is not an error — it is the ABI's `Option::None`.
///
/// # Safety
/// `out` must be a writable, non-null `*mut c_char`.
pub(crate) unsafe fn store_opt_string(
    value: Option<&str>,
    out: *mut *mut c_char,
) -> PrismPdfStatus {
    match value {
        Some(text) => unsafe { store_string(text, out) },
        None => PrismPdfStatus::NotFound,
    }
}

/// Null-check a borrowed item and a `(ptr, len)` out-pair, zeroing both outputs.
///
/// # Safety
/// `item` is a borrowed item pointer (or null); the out-params are pointers (or null).
pub(crate) unsafe fn prepare_borrowed_bytes_out<'a, T>(
    item: *const T,
    out_data: *mut *const u8,
    out_len: *mut usize,
) -> Option<&'a T> {
    if out_data.is_null() || out_len.is_null() {
        return None;
    }
    unsafe {
        *out_data = std::ptr::null();
        *out_len = 0;
    }
    if item.is_null() {
        return None;
    }
    Some(unsafe { &*item })
}

/// Lend `bytes` as a borrowed `(ptr, len)` view into the owning list's allocation — no copy, and
/// nothing for the caller to free. An empty slice lends a null pointer with length 0 rather than a
/// dangling one.
///
/// # Safety
/// Both out-params must be writable, and the caller must guarantee the list outlives the view.
pub(crate) unsafe fn lend_bytes(
    bytes: &[u8],
    out_data: *mut *const u8,
    out_len: *mut usize,
) -> PrismPdfStatus {
    if bytes.is_empty() {
        return PrismPdfStatus::Ok;
    }
    unsafe {
        *out_data = bytes.as_ptr();
        *out_len = bytes.len();
    }
    PrismPdfStatus::Ok
}

// ---------------------------------------------------------------------------------------------
// Collection conventions (EPIC 10, docs/ABI.md "Collections")
//
// The facade returns owned Rust collections (`Vec<Annotation>`, `Vec<FormField>`, …) whose items
// carry `String` and `Option<T>` fields. C has no vector and no `Option`, so every collection
// crosses the boundary the same way:
//
//   1. A producer writes an owned **list handle** to an out-param (`*_list_free` releases it).
//   2. `*_list_len` reports the item count.
//   3. `*_list_get` lends a **borrowed** item pointer, valid only while the list handle lives.
//      Borrowed items are never freed by the caller.
//   4. Per-field getters read one field off a borrowed item. A field that is `Option::None`
//      returns [`PrismPdfStatus::NotFound`] with the out-param left null/zero.
//
// Strings always come back owned and NUL-terminated, released with [`prismpdf_string_free`] — the
// same rule the read path already uses for `prismpdf_page_text`.
// ---------------------------------------------------------------------------------------------

/// Write `value` to `*out` as an owned C string, or report [`PrismPdfStatus::Internal`] if it
/// contains an interior NUL (which a C string cannot represent).
///
/// # Safety
/// `out` must be a writable, non-null `*mut c_char`.
pub(crate) unsafe fn store_string(value: &str, out: *mut *mut c_char) -> PrismPdfStatus {
    match CString::new(value) {
        Ok(cstring) => {
            unsafe { *out = cstring.into_raw() };
            PrismPdfStatus::Ok
        }
        Err(_) => PrismPdfStatus::Internal,
    }
}

/// Null-check `item` and `out`, zeroing `*out`. `None` means a null argument was supplied.
///
/// # Safety
/// `item` is a borrowed item pointer (or null); `out` is an out-pointer (or null).
pub(crate) unsafe fn prepare_string_out<'a, T>(
    item: *const T,
    out: *mut *mut c_char,
) -> Option<&'a T> {
    if out.is_null() {
        return None;
    }
    unsafe { *out = std::ptr::null_mut() };
    if item.is_null() {
        return None;
    }
    Some(unsafe { &*item })
}
