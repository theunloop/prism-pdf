use super::*;

// ---------------------------------------------------------------------------------------------
// Conformance production: PDF/A (§14, ISO 19005) and PDF/UA (ISO 14289)
//
// These finalise a `Builder`, so they arrive now that authoring has a handle. A conformance
// failure is not a parse failure and does not deserve the same code: it gets
// [`PrismPdfStatus::Conformance`] plus a typed [`PrismPdfConformanceIssue`] naming the reason, so a
// caller learns *which* rule it broke rather than only that something went wrong.
// ---------------------------------------------------------------------------------------------

/// Why a conformance pass refused the document. Reported through the out-param of the
/// `prismpdf_builder_make_*` calls when they return [`PrismPdfStatus::Conformance`].
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PrismPdfConformanceIssue {
    /// A font is not embedded (PDF/A §6.3.4, PDF/UA §7.21.4.1). Standard-14 fonts are rejected:
    /// embed a real program with `prismpdf_builder_embed_cid_font`.
    UnembeddedFont = 0,
    /// The document has attachments, which only PDF/A-3 and PDF/A-4f permit (§6.8).
    AttachmentRequiresPdfA3 = 1,
    /// Level A conformance requires logical structure — the document is untagged (§6.9).
    LevelARequiresTagging = 2,
    /// The content uses transparency, which PDF/A-1 forbids (§6.4).
    TransparencyRequiresPdfA2 = 3,
    /// PDF/UA requires a tagged document (14289-1 §7.1).
    NotTagged = 4,
    /// PDF/UA requires a document title (14289-1 §7.1, with `/DisplayDocTitle`).
    MissingTitle = 5,
    /// PDF/UA requires a natural language (`/Lang`, 14289-1 §7.2).
    MissingLanguage = 6,
    /// A figure has no alternative description (14289-1 §7.3).
    FigureWithoutAlt = 7,
    /// PDF/UA-2 forbids `Note`; use `FENote` (14289-2 §8.2.5.14).
    NoteForbidden = 8,
    /// PDF/UA-2 forbids the generic `H` heading (14289-2 §8.2.5.12).
    GenericHeadingForbidden = 9,
    /// An embedded file has no description (14289-2 §8.14.1).
    AttachmentWithoutDesc = 10,
    /// A link has no structure destination (14289-2 §8.9.2).
    LinkWithoutStructureDest = 11,
    /// A structure element uses a type outside the declared namespace.
    UnknownStructureType = 12,
    /// The content references the `.notdef` glyph (14289-1 §7.21.4.2).
    NotdefGlyph = 13,
}

/// Map a facade error onto a conformance issue; `None` when it is not a conformance failure.
pub(crate) fn conformance_issue(error: &PdfError) -> Option<PrismPdfConformanceIssue> {
    match error {
        PdfError::PdfA(issue) => Some(match issue {
            PdfAError::UnembeddedFont => PrismPdfConformanceIssue::UnembeddedFont,
            PdfAError::AttachmentRequiresPdfA3 => PrismPdfConformanceIssue::AttachmentRequiresPdfA3,
            PdfAError::LevelARequiresTagging => PrismPdfConformanceIssue::LevelARequiresTagging,
            PdfAError::TransparencyRequiresPdfA2 => {
                PrismPdfConformanceIssue::TransparencyRequiresPdfA2
            }
        }),
        PdfError::PdfUa(issue) => Some(match issue {
            PdfUaError::NotTagged => PrismPdfConformanceIssue::NotTagged,
            PdfUaError::MissingTitle => PrismPdfConformanceIssue::MissingTitle,
            PdfUaError::MissingLanguage => PrismPdfConformanceIssue::MissingLanguage,
            PdfUaError::FigureWithoutAlt => PrismPdfConformanceIssue::FigureWithoutAlt,
            PdfUaError::UnembeddedFont => PrismPdfConformanceIssue::UnembeddedFont,
            PdfUaError::NoteForbidden => PrismPdfConformanceIssue::NoteForbidden,
            PdfUaError::GenericHeadingForbidden => {
                PrismPdfConformanceIssue::GenericHeadingForbidden
            }
            PdfUaError::AttachmentWithoutDesc => PrismPdfConformanceIssue::AttachmentWithoutDesc,
            PdfUaError::LinkWithoutStructureDest => {
                PrismPdfConformanceIssue::LinkWithoutStructureDest
            }
            PdfUaError::UnknownStructureType => PrismPdfConformanceIssue::UnknownStructureType,
            PdfUaError::NotdefGlyph => PrismPdfConformanceIssue::NotdefGlyph,
        }),
        _ => None,
    }
}

/// Turn a conformance result into a status, recording the reason in `out_issue` on failure.
///
/// # Safety
/// `out_issue` must be writable or null.
pub(crate) unsafe fn finish_conformance(
    result: Result<(), PdfError>,
    out_issue: *mut PrismPdfConformanceIssue,
) -> PrismPdfStatus {
    match result {
        Ok(()) => PrismPdfStatus::Ok,
        Err(error) => match conformance_issue(&error) {
            Some(issue) => {
                if !out_issue.is_null() {
                    unsafe { *out_issue = issue };
                }
                PrismPdfStatus::Conformance
            }
            None => PrismPdfStatus::Parse,
        },
    }
}

/// A PDF/A conformance level (§14, ISO 19005). Part and level together: `A2u` is part 2, level U.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PrismPdfPdfAConformance {
    /// PDF/A-1b — basic, ISO 19005-1.
    A1b = 0,
    /// PDF/A-1a — accessible (requires tagging).
    A1a = 1,
    /// PDF/A-2b — basic, ISO 19005-2.
    A2b = 2,
    /// PDF/A-2u — basic plus Unicode mapping.
    A2u = 3,
    /// PDF/A-2a — accessible.
    A2a = 4,
    /// PDF/A-3b — basic, permits attachments.
    A3b = 5,
    /// PDF/A-3u — plus Unicode mapping.
    A3u = 6,
    /// PDF/A-3a — accessible.
    A3a = 7,
    /// PDF/A-4 — ISO 19005-4, on PDF 2.0.
    A4 = 8,
    /// PDF/A-4e — engineering.
    A4e = 9,
    /// PDF/A-4f — permits attachments.
    A4f = 10,
}

impl From<PrismPdfPdfAConformance> for PdfAConformance {
    fn from(value: PrismPdfPdfAConformance) -> Self {
        match value {
            PrismPdfPdfAConformance::A1b => PdfAConformance::A1b,
            PrismPdfPdfAConformance::A1a => PdfAConformance::A1a,
            PrismPdfPdfAConformance::A2b => PdfAConformance::A2b,
            PrismPdfPdfAConformance::A2u => PdfAConformance::A2u,
            PrismPdfPdfAConformance::A2a => PdfAConformance::A2a,
            PrismPdfPdfAConformance::A3b => PdfAConformance::A3b,
            PrismPdfPdfAConformance::A3u => PdfAConformance::A3u,
            PrismPdfPdfAConformance::A3a => PdfAConformance::A3a,
            PrismPdfPdfAConformance::A4 => PdfAConformance::A4,
            PrismPdfPdfAConformance::A4e => PdfAConformance::A4e,
            PrismPdfPdfAConformance::A4f => PdfAConformance::A4f,
        }
    }
}

/// The ISO 19005 part number of a conformance level: 1, 2, 3 or 4.
#[unsafe(no_mangle)]
pub extern "C" fn prismpdf_pdfa_part(conformance: PrismPdfPdfAConformance) -> u8 {
    PdfAConformance::from(conformance).part()
}

/// Whether the level permits embedded files — only PDF/A-3 and PDF/A-4f do (§6.8). Check this
/// before calling `prismpdf_builder_attach_file` on a document destined for PDF/A.
#[unsafe(no_mangle)]
pub extern "C" fn prismpdf_pdfa_allows_attachments(conformance: PrismPdfPdfAConformance) -> bool {
    PdfAConformance::from(conformance).allows_attachments()
}

/// The conformance code as it appears in XMP, e.g. `2u` — an owned C string.
///
/// # Safety
/// `out_text` must be writable. Release the string with [`prismpdf_string_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_pdfa_code(
    conformance: PrismPdfPdfAConformance,
    out_text: *mut *mut c_char,
) -> PrismPdfStatus {
    if out_text.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    unsafe { *out_text = std::ptr::null_mut() };
    guard(|| unsafe { store_string(PdfAConformance::from(conformance).code(), out_text) })
}

// --- XMP metadata -----------------------------------------------------------------------------

/// The metadata written into the XMP packet by the conformance passes (§14.3.2). A mutable handle
/// like [`PrismPdfSignSettings`]: create it, set what applies, pass it to a `make_*` call, free it.
pub struct PrismPdfXmpMetadata(pub(crate) XmpMetadata);

/// Create an empty XMP metadata set.
///
/// # Safety
/// The returned handle must be released with [`prismpdf_xmp_metadata_free`].
#[unsafe(no_mangle)]
pub extern "C" fn prismpdf_xmp_metadata_new() -> *mut PrismPdfXmpMetadata {
    guard_ptr(|| Box::into_raw(Box::new(PrismPdfXmpMetadata(XmpMetadata::default()))))
}

/// Release an XMP metadata handle. Freeing `NULL` is a no-op.
///
/// # Safety
/// `meta` must come from [`prismpdf_xmp_metadata_new`] and must not already be freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_xmp_metadata_free(meta: *mut PrismPdfXmpMetadata) {
    unsafe { free_handle(meta) }
}

/// Borrow an XMP handle and a C string together, or `None` on a null/non-UTF-8 argument.
///
/// # Safety
/// `meta` must be a live handle or null; `value` a C string pointer or null.
pub(crate) unsafe fn xmp_and_str<'a>(
    meta: *mut PrismPdfXmpMetadata,
    value: *const c_char,
) -> Option<(&'a mut XmpMetadata, String)> {
    if meta.is_null() || value.is_null() {
        return None;
    }
    let text = unsafe { utf8(value) }?;
    Some((unsafe { &mut (*meta).0 }, text.to_string()))
}

/// Append an author (`dc:creator`). Call repeatedly for multiple authors — this is the one XMP
/// field that is a list rather than a single value.
///
/// # Safety
/// `meta` must be live and `author` a NUL-terminated UTF-8 C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_xmp_metadata_add_author(
    meta: *mut PrismPdfXmpMetadata,
    author: *const c_char,
) -> PrismPdfStatus {
    let Some((meta, value)) = (unsafe { xmp_and_str(meta, author) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| {
        meta.authors.push(value);
        PrismPdfStatus::Ok
    })
}

// --- Production passes ------------------------------------------------------------------------

/// Finalise `builder` as a conformant **PDF/A** file (§14, ISO 19005): XMP metadata, an sRGB
/// OutputIntent (§14.11.5) and a file `/ID`.
///
/// Fonts must be embedded first — Standard-14 fonts are rejected. On refusal this returns
/// [`PrismPdfStatus::Conformance`] and writes the reason to `out_issue`, which may be null if you
/// do not want it.
///
/// # Safety
/// `builder` and `meta` must be live handles; `out_issue` must be writable or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_builder_make_pdfa(
    builder: *mut PrismPdfBuilder,
    conformance: PrismPdfPdfAConformance,
    meta: *const PrismPdfXmpMetadata,
    out_issue: *mut PrismPdfConformanceIssue,
) -> PrismPdfStatus {
    let Some(handle) = (unsafe { builder_mut(builder) }) else {
        return PrismPdfStatus::NullArgument;
    };
    if meta.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    let metadata = unsafe { &(*meta).0 };
    guard(|| unsafe {
        finish_conformance(
            prismpdf::make_pdfa(handle, conformance.into(), metadata),
            out_issue,
        )
    })
}

/// As [`prismpdf_builder_make_pdfa`], but with a caller-chosen ICC output intent (§14.11.5) instead
/// of the default sRGB one — e.g. a CMYK printing condition, so `DeviceCMYK` content is conformant
/// under PDF/A §6.2.4.3.
///
/// `n` is the profile's colour-component count (1 = Gray, 3 = RGB, 4 = CMYK).
///
/// # Safety
/// As [`prismpdf_builder_make_pdfa`], plus `icc` must point to `icc_len` readable bytes and
/// `identifier` must be a NUL-terminated UTF-8 C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_builder_make_pdfa_with_output_intent(
    builder: *mut PrismPdfBuilder,
    conformance: PrismPdfPdfAConformance,
    meta: *const PrismPdfXmpMetadata,
    icc: *const u8,
    icc_len: usize,
    n: u32,
    identifier: *const c_char,
    out_issue: *mut PrismPdfConformanceIssue,
) -> PrismPdfStatus {
    let Some(handle) = (unsafe { builder_mut(builder) }) else {
        return PrismPdfStatus::NullArgument;
    };
    if meta.is_null() || icc.is_null() || identifier.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    let metadata = unsafe { &(*meta).0 };
    guard(|| {
        let Some(identifier) = (unsafe { utf8(identifier) }) else {
            return PrismPdfStatus::NullArgument;
        };
        let profile = unsafe { slice_or_empty(icc, icc_len) };
        let intent = OutputIntentProfile::new(profile.into_owned(), n, identifier);
        unsafe {
            finish_conformance(
                prismpdf::make_pdfa_with_output_intent(
                    handle,
                    conformance.into(),
                    metadata,
                    &intent,
                ),
                out_issue,
            )
        }
    })
}

/// Finalise `builder` as an accessible **PDF/UA-1** file (ISO 14289-1) in the natural language
/// `lang`. The document must be tagged and carry a title; fonts must be embedded.
///
/// # Safety
/// `builder` and `meta` must be live handles, `lang` a NUL-terminated UTF-8 C string, and
/// `out_issue` writable or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_builder_make_pdfua(
    builder: *mut PrismPdfBuilder,
    meta: *const PrismPdfXmpMetadata,
    lang: *const c_char,
    out_issue: *mut PrismPdfConformanceIssue,
) -> PrismPdfStatus {
    let Some(handle) = (unsafe { builder_mut(builder) }) else {
        return PrismPdfStatus::NullArgument;
    };
    if meta.is_null() || lang.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    let metadata = unsafe { &(*meta).0 };
    guard(|| {
        let Some(lang) = (unsafe { utf8(lang) }) else {
            return PrismPdfStatus::NullArgument;
        };
        unsafe { finish_conformance(prismpdf::make_pdfua(handle, metadata, lang), out_issue) }
    })
}

/// Finalise `builder` as an accessible **PDF/UA-2** file (ISO 14289-2:2024, on PDF 2.0): the root
/// `Document` element in the 2.0 structure namespace, `/DisplayDocTitle`, `/Lang`.
///
/// Stricter than UA-1: `Note` and the generic `H` heading are refused, and embedded files need
/// descriptions.
///
/// # Safety
/// As [`prismpdf_builder_make_pdfua`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_builder_make_pdfua2(
    builder: *mut PrismPdfBuilder,
    meta: *const PrismPdfXmpMetadata,
    lang: *const c_char,
    out_issue: *mut PrismPdfConformanceIssue,
) -> PrismPdfStatus {
    let Some(handle) = (unsafe { builder_mut(builder) }) else {
        return PrismPdfStatus::NullArgument;
    };
    if meta.is_null() || lang.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    let metadata = unsafe { &(*meta).0 };
    guard(|| {
        let Some(lang) = (unsafe { utf8(lang) }) else {
            return PrismPdfStatus::NullArgument;
        };
        unsafe { finish_conformance(prismpdf::make_pdfua2(handle, metadata, lang), out_issue) }
    })
}

/// Set the document's OutputIntent (§14.11.5) directly, without running a conformance pass: the
/// ICC profile bytes, its colour-component count `n` and the output-condition identifier.
///
/// # Safety
/// `builder` must be live, `icc` must point to `icc_len` readable bytes, and `identifier` must be
/// a NUL-terminated UTF-8 C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_builder_set_output_intent(
    builder: *mut PrismPdfBuilder,
    icc: *const u8,
    icc_len: usize,
    n: u32,
    identifier: *const c_char,
) -> PrismPdfStatus {
    let Some((handle, identifier)) = (unsafe { builder_and_str(builder, identifier) }) else {
        return PrismPdfStatus::NullArgument;
    };
    if icc.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        let profile = unsafe { slice_or_empty(icc, icc_len) };
        handle.output_intent(profile.into_owned(), n, identifier);
        PrismPdfStatus::Ok
    })
}

/// Set `dc:title`.
///
/// # Safety
/// `meta` must be live and `value` a NUL-terminated UTF-8 C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_xmp_metadata_set_title(
    meta: *mut PrismPdfXmpMetadata,
    value: *const c_char,
) -> PrismPdfStatus {
    let Some((meta, value)) = (unsafe { xmp_and_str(meta, value) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| {
        meta.title = Some(value);
        PrismPdfStatus::Ok
    })
}
/// Set `dc:description`.
///
/// # Safety
/// `meta` must be live and `value` a NUL-terminated UTF-8 C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_xmp_metadata_set_subject(
    meta: *mut PrismPdfXmpMetadata,
    value: *const c_char,
) -> PrismPdfStatus {
    let Some((meta, value)) = (unsafe { xmp_and_str(meta, value) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| {
        meta.subject = Some(value);
        PrismPdfStatus::Ok
    })
}
/// Set `pdf:Keywords`.
///
/// # Safety
/// `meta` must be live and `value` a NUL-terminated UTF-8 C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_xmp_metadata_set_keywords(
    meta: *mut PrismPdfXmpMetadata,
    value: *const c_char,
) -> PrismPdfStatus {
    let Some((meta, value)) = (unsafe { xmp_and_str(meta, value) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| {
        meta.keywords = Some(value);
        PrismPdfStatus::Ok
    })
}
/// Set `xmp:CreatorTool`.
///
/// # Safety
/// `meta` must be live and `value` a NUL-terminated UTF-8 C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_xmp_metadata_set_creator_tool(
    meta: *mut PrismPdfXmpMetadata,
    value: *const c_char,
) -> PrismPdfStatus {
    let Some((meta, value)) = (unsafe { xmp_and_str(meta, value) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| {
        meta.creator_tool = Some(value);
        PrismPdfStatus::Ok
    })
}
/// Set `pdf:Producer`.
///
/// # Safety
/// `meta` must be live and `value` a NUL-terminated UTF-8 C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_xmp_metadata_set_producer(
    meta: *mut PrismPdfXmpMetadata,
    value: *const c_char,
) -> PrismPdfStatus {
    let Some((meta, value)) = (unsafe { xmp_and_str(meta, value) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| {
        meta.producer = Some(value);
        PrismPdfStatus::Ok
    })
}
/// Set `xmp:CreateDate`.
///
/// # Safety
/// `meta` must be live and `value` a NUL-terminated UTF-8 C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_xmp_metadata_set_create_date(
    meta: *mut PrismPdfXmpMetadata,
    value: *const c_char,
) -> PrismPdfStatus {
    let Some((meta, value)) = (unsafe { xmp_and_str(meta, value) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| {
        meta.create_date = Some(value);
        PrismPdfStatus::Ok
    })
}
/// Set `xmp:ModifyDate`.
///
/// # Safety
/// `meta` must be live and `value` a NUL-terminated UTF-8 C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_xmp_metadata_set_modify_date(
    meta: *mut PrismPdfXmpMetadata,
    value: *const c_char,
) -> PrismPdfStatus {
    let Some((meta, value)) = (unsafe { xmp_and_str(meta, value) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| {
        meta.modify_date = Some(value);
        PrismPdfStatus::Ok
    })
}

// ---------------------------------------------------------------------------------------------
// Remaining `Document` entry points and reusable open options.
//
// Object lookup and mutation cross through the owned COS/edit handles above. Bulk live-object
// enumeration and page-content decoding remain expert extensions not projected here.
// ---------------------------------------------------------------------------------------------

/// Anti-DoS parsing limits (DESIGN.md §3.5). Zero in any field means "use the default" — 512 for
/// `max_depth`, 2^20 for `max_objstm_objects`, 2^21 for `max_objects`.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct PrismPdfLimits {
    /// Maximum object nesting depth before the parser refuses to descend further.
    pub max_depth: usize,
    /// Maximum objects declared by a single object stream's `/N` (§7.5.7).
    pub max_objstm_objects: usize,
    /// Maximum objects in the whole document.
    pub max_objects: usize,
}

/// Owned, reusable options for opening hostile or encrypted input.
pub struct PrismPdfOpenOptions {
    limits: Limits,
    password: Vec<u8>,
}

/// Create reusable open options populated with the engine's default anti-DoS limits.
#[unsafe(no_mangle)]
pub extern "C" fn prismpdf_open_options_new() -> *mut PrismPdfOpenOptions {
    guard_ptr(|| {
        Box::into_raw(Box::new(PrismPdfOpenOptions {
            limits: Limits::default(),
            password: Vec::new(),
        }))
    })
}

/// Set the maximum array/dictionary nesting depth.
///
/// # Safety
/// `options` must be a live options handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_open_options_set_max_depth(
    options: *mut PrismPdfOpenOptions,
    max_depth: usize,
) -> PrismPdfStatus {
    if options.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        unsafe { (*options).limits.max_depth = max_depth };
        PrismPdfStatus::Ok
    })
}

/// Set the maximum number of objects declared by one object stream (§7.5.7).
///
/// # Safety
/// `options` must be a live options handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_open_options_set_max_objstm_objects(
    options: *mut PrismPdfOpenOptions,
    max_objects: usize,
) -> PrismPdfStatus {
    if options.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        unsafe { (*options).limits.max_objstm_objects = max_objects };
        PrismPdfStatus::Ok
    })
}

/// Set the maximum number of objects in the document (§7.5).
///
/// # Safety
/// `options` must be a live options handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_open_options_set_max_objects(
    options: *mut PrismPdfOpenOptions,
    max_objects: usize,
) -> PrismPdfStatus {
    if options.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        unsafe { (*options).limits.max_objects = max_objects };
        PrismPdfStatus::Ok
    })
}

/// Copy a password tried as both user and owner password. Null with length zero clears it.
///
/// # Safety
/// `options` must be live; `password` must be readable for `password_len` bytes or null when zero.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_open_options_set_password(
    options: *mut PrismPdfOpenOptions,
    password: *const u8,
    password_len: usize,
) -> PrismPdfStatus {
    if options.is_null() || (password.is_null() && password_len != 0) {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        let value = if password.is_null() {
            Vec::new()
        } else {
            unsafe { std::slice::from_raw_parts(password, password_len) }.to_vec()
        };
        unsafe { (*options).password = value };
        PrismPdfStatus::Ok
    })
}

/// Open using a reusable options snapshot. The input and options are copied for the new document.
///
/// # Safety
/// `data` must be readable for `len` bytes, `options` live, and `out_doc` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_document_open_with_options(
    data: *const u8,
    len: usize,
    options: *const PrismPdfOpenOptions,
    out_doc: *mut *mut PrismPdfDocument,
) -> PrismPdfStatus {
    if out_doc.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    unsafe { *out_doc = std::ptr::null_mut() };
    if data.is_null() || options.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        let bytes = unsafe { std::slice::from_raw_parts(data, len) }.to_vec();
        let options = unsafe { &*options };
        store_opened(
            Document::open_with_password_and_limits(bytes, &options.password, options.limits),
            out_doc,
        )
    })
}

/// Release reusable open options. Null is ignored.
///
/// # Safety
/// `options` must be null or a live handle returned by [`prismpdf_open_options_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_open_options_free(options: *mut PrismPdfOpenOptions) {
    unsafe { free_handle(options) }
}

/// Open a PDF with explicit anti-DoS limits (DESIGN.md §3.5), instead of the defaults
/// [`prismpdf_document_open`] uses. The knob a service parsing untrusted uploads needs.
///
/// @deprecated Since 0.2.0, use [`prismpdf_document_open_with_options`]. The owned options handle
/// is ABI-extensible; this function remains available through the documented compatibility window.
///
/// # Safety
/// `data` must point to at least `len` readable bytes; `limits` must be a readable
/// [`PrismPdfLimits`] or null (null meaning defaults); `out_doc` must be writable. Release the
/// handle with [`prismpdf_document_free`].
#[deprecated(
    since = "0.2.0",
    note = "use prismpdf_document_open_with_options; the opaque options handle is ABI-extensible"
)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_document_open_with_limits(
    data: *const u8,
    len: usize,
    limits: *const PrismPdfLimits,
    out_doc: *mut *mut PrismPdfDocument,
) -> PrismPdfStatus {
    if out_doc.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    unsafe { *out_doc = std::ptr::null_mut() };
    if data.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        let mut settings = Limits::default();
        if !limits.is_null() {
            let requested = unsafe { *limits };
            if requested.max_depth != 0 {
                settings.max_depth = requested.max_depth;
            }
            if requested.max_objstm_objects != 0 {
                settings.max_objstm_objects = requested.max_objstm_objects;
            }
            if requested.max_objects != 0 {
                settings.max_objects = requested.max_objects;
            }
        }
        let bytes = unsafe { std::slice::from_raw_parts(data, len) }.to_vec();
        store_opened(Document::open_with_limits(bytes, settings), out_doc)
    })
}

/// Open a document encrypted to a certificate (§7.6.5) using the matching private key, both
/// DER-encoded — the public-key counterpart of
/// [`prismpdf_document_open_with_password`].
///
/// # Safety
/// `data`, `cert_der` and `key_der` must each point to their stated lengths of readable bytes;
/// `out_doc` must be writable. Release the handle with [`prismpdf_document_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_document_open_with_private_key(
    data: *const u8,
    len: usize,
    cert_der: *const u8,
    cert_len: usize,
    key_der: *const u8,
    key_len: usize,
    out_doc: *mut *mut PrismPdfDocument,
) -> PrismPdfStatus {
    if out_doc.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    unsafe { *out_doc = std::ptr::null_mut() };
    if data.is_null() || cert_der.is_null() || key_der.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        let bytes = unsafe { std::slice::from_raw_parts(data, len) }.to_vec();
        let cert = unsafe { slice_or_empty(cert_der, cert_len) };
        let key = unsafe { slice_or_empty(key_der, key_len) };
        store_opened(Document::open_with_private_key(bytes, &cert, &key), out_doc)
    })
}

/// The **minimum** PDF version the document's constructs actually require (§7.5.2), which can be
/// lower than the version its header declares. Useful before re-targeting with
/// [`prismpdf_document_save_as`].
///
/// # Safety
/// `doc` must be live; `out_major`/`out_minor` must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_document_min_version(
    doc: *const PrismPdfDocument,
    out_major: *mut u8,
    out_minor: *mut u8,
) -> PrismPdfStatus {
    if out_major.is_null() || out_minor.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    unsafe {
        *out_major = 0;
        *out_minor = 0;
    }
    if doc.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    let document = unsafe { &(*doc).0 };
    guard(|| match document.min_pdf_version() {
        Ok((major, minor)) => {
            unsafe {
                *out_major = major;
                *out_minor = minor;
            }
            PrismPdfStatus::Ok
        }
        Err(_) => PrismPdfStatus::Parse,
    })
}

/// Full rewrite declaring exactly version `(major, minor)` (§7.5.2), refusing constructs above the
/// target rather than emitting them — the `Document` counterpart of
/// [`prismpdf_builder_build_for`].
///
/// # Safety
/// `doc` must be live; `out_data`/`out_len` writable. Release the buffer with
/// [`prismpdf_bytes_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_document_save_as(
    doc: *const PrismPdfDocument,
    major: u8,
    minor: u8,
    out_data: *mut *mut u8,
    out_len: *mut usize,
) -> PrismPdfStatus {
    let Some(document) = (unsafe { prepare_bytes_out(doc, out_data, out_len) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| match document.save_as(major, minor) {
        Ok(bytes) => emit_bytes(bytes, out_data, out_len),
        Err(_) => PrismPdfStatus::Parse,
    })
}

/// Version-targeted full rewrite returning explicit preservation effects.
///
/// # Safety
/// `doc` must be live and `out_report` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_document_save_as_report(
    doc: *const PrismPdfDocument,
    major: u8,
    minor: u8,
    out_report: *mut *mut PrismPdfTransformReport,
) -> PrismPdfStatus {
    if doc.is_null() || out_report.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    unsafe { *out_report = std::ptr::null_mut() };
    guard(|| {
        store_transform_report(
            unsafe { &(*doc).0 }.save_as_with_report(major, minor),
            out_report,
        )
    })
}

/// Full rewrite packing objects into **object streams** (§7.5.7) as well as using a cross-reference
/// stream — the smallest output of the three save modes.
///
/// # Safety
/// `doc` must be live; `out_data`/`out_len` writable. Release the buffer with
/// [`prismpdf_bytes_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_document_save_packed(
    doc: *const PrismPdfDocument,
    out_data: *mut *mut u8,
    out_len: *mut usize,
) -> PrismPdfStatus {
    let Some(document) = (unsafe { prepare_bytes_out(doc, out_data, out_len) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| match document.save_packed() {
        Ok(bytes) => emit_bytes(bytes, out_data, out_len),
        Err(_) => PrismPdfStatus::Parse,
    })
}

/// Object-stream full rewrite returning explicit preservation effects.
///
/// # Safety
/// `doc` must be live and `out_report` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_document_save_packed_report(
    doc: *const PrismPdfDocument,
    out_report: *mut *mut PrismPdfTransformReport,
) -> PrismPdfStatus {
    if doc.is_null() || out_report.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    unsafe { *out_report = std::ptr::null_mut() };
    guard(|| store_transform_report(unsafe { &(*doc).0 }.save_packed_with_report(), out_report))
}

/// An owned list of UTF-8 strings. Released by [`prismpdf_string_list_free`]; individual entries are
/// read with [`prismpdf_string_list_get`] and are **owned** by the caller.
pub struct PrismPdfStringList(pub(crate) Vec<String>);

/// Number of strings in `list`.
///
/// # Safety
/// `list` must be a live list handle and `out_len` a writable `*mut usize`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_string_list_len(
    list: *const PrismPdfStringList,
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

/// Copy string `index` out of `list` as an owned C string.
///
/// Unlike the item accessors on other collections this **copies**, because a C string needs a NUL
/// terminator the Rust `String` does not carry.
///
/// # Safety
/// `list` must be live and `out_text` writable. Release the string with [`prismpdf_string_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_string_list_get(
    list: *const PrismPdfStringList,
    index: usize,
    out_text: *mut *mut c_char,
) -> PrismPdfStatus {
    if out_text.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    unsafe { *out_text = std::ptr::null_mut() };
    if list.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    let items = unsafe { &(*list).0 };
    guard(|| match items.get(index) {
        Some(text) => unsafe { store_string(text, out_text) },
        None => PrismPdfStatus::NotFound,
    })
}

/// Release a string list. Freeing `NULL` is a no-op.
///
/// # Safety
/// `list` must come from a `prismpdf_*` call that produced one and must not already be freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_string_list_free(list: *mut PrismPdfStringList) {
    unsafe { free_handle(list) }
}

/// The structure namespaces the document declares (§14.7.4 — PDF 2.0), as namespace URIs.
///
/// # Safety
/// `doc` must be live and `out_list` writable. Release it with [`prismpdf_string_list_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_document_structure_namespaces(
    doc: *const PrismPdfDocument,
    out_list: *mut *mut PrismPdfStringList,
) -> PrismPdfStatus {
    let Some(document) = (unsafe { prepare_list_out(doc, out_list) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| match document.structure_namespaces() {
        Ok(items) => {
            unsafe { *out_list = Box::into_raw(Box::new(PrismPdfStringList(items))) };
            PrismPdfStatus::Ok
        }
        Err(_) => PrismPdfStatus::Parse,
    })
}

/// The `/VRI` keys in the document's DSS (§12.8.4.3) — one per signature carrying long-term
/// validation material.
///
/// # Safety
/// `doc` must be live and `out_list` writable. Release it with [`prismpdf_string_list_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_document_signature_vri_keys(
    doc: *const PrismPdfDocument,
    out_list: *mut *mut PrismPdfStringList,
) -> PrismPdfStatus {
    let Some(document) = (unsafe { prepare_list_out(doc, out_list) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| match document.signature_vri_keys() {
        Ok(items) => {
            unsafe { *out_list = Box::into_raw(Box::new(PrismPdfStringList(items))) };
            PrismPdfStatus::Ok
        }
        Err(_) => PrismPdfStatus::Parse,
    })
}

// ---------------------------------------------------------------------------------------------
// High-level layout (§9.4): text flow, tables, images
//
// Three things needed adapting for C:
//
// * `TextBlock<'a>` borrows its font names, which DESIGN.md §6.4 forbids at an FFI point. The C
//   handle **owns** its strings and materialises a borrowed `TextBlock` per call.
// * `Table`'s builder methods take `self` by value. `Table` is `Clone`, so each setter clones,
//   applies and stores back — invisible to the caller, who just mutates a handle.
// * `Flow::build` and `into_builder` **consume** the flow. Their C counterparts consume the handle
