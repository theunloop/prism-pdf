//! pdf-ffi — stable C ABI for Prism PDF (EPIC 10, DESIGN.md §6.1).
//!
//! This is the **only** crate permitted to use `unsafe`, and it is confined and audited here
//! (DESIGN.md §3.7). The surface is **handle-based**: callers open a document into an opaque
//! [`PrismPdfDocument`] handle and pass it back to every other call.
//!
//! The goal is **capability parity with the `prismpdf` facade** (DESIGN.md §6.1) — anything the
//! Rust API can do, C can do — not signature parity, which is impossible: C has no `Result`,
//! `Option`, `String`, `Vec` or generics. The conventions that bridge that gap are documented under
//! "Collection conventions" below and in `docs/ABI.md`.
//!
//! Contract (DESIGN.md §6.1):
//! - Every `extern "C"` function whose body can run Rust code that might panic wraps that body in
//!   [`catch_unwind`](std::panic::catch_unwind) — directly, or through the `guard` / `guard_ptr`
//!   helpers, or by delegating to a shared helper that does. A caught panic becomes
//!   [`PrismPdfStatus::Internal`], or a null pointer for the constructors.
//!
//!   The exemption is narrow and exhaustive: a function that only reads a `Copy` value out of a
//!   `#[repr(C)]` enum, performs integer or bitwise arithmetic on it, and returns a scalar cannot
//!   panic, allocate, or touch caller memory, so there is nothing for a guard to catch and — since
//!   these return `i32`/`u8`/`bool` rather than [`PrismPdfStatus`] — nowhere to report it. That
//!   covers exactly the `prismpdf_permissions_*` builders, `prismpdf_pdfa_part`,
//!   `prismpdf_pdfa_allows_attachments`, and `prismpdf_version` (which returns a pointer to a
//!   `'static` NUL-terminated literal). Anything that allocates, dereferences a caller pointer, or
//!   calls into the engine is guarded, with no exceptions.
//! - Errors are reported as a stable integer [`PrismPdfStatus`]; results travel through out-params.
//! - Memory the library allocates (handles, strings) is freed only by the matching `*_free`
//!   function — never by the C runtime's `free`.
//!
//! The C header `prismpdf.h` is generated from this file by `cbindgen` (see `cbindgen.toml`); the
//! ABI/versioning contract lives in `docs/ABI.md`.
//!
//! Exposed operations: the **read** path (open, page count, version, per-page and whole-document
//! text), a **write/transform** path returning fresh PDF byte buffers (save, save-compact,
//! save-encrypted, extract-pages, rotate-page, merge, fill-form and flatten-form), each released
//! with `prismpdf_bytes_free`, and the **collection** path (annotations §12.5, form fields §12.7,
//! the outline tree §12.3.3) returning owned list handles released with the matching `*_list_free`.
#![cfg_attr(test, allow(deprecated, clippy::unwrap_used, clippy::expect_used))]

use std::cell::RefCell;
use std::ffi::{CString, c_char};
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use prismpdf::cos::{Array, Dictionary, Name, Object, ObjectId, PdfDate, PdfString, Stream};
use prismpdf::{
    Algorithm, Align, Annotation, AnnotationSpec, Attachment, AttrValue, Builder,
    Color as LayoutColor, ColorSpace, Composition, Container as ComposeContainer, Content,
    Document, Error as PdfError, ExtractedAttachment, ExtractedImage, Flow, FontProgramFormat,
    FontReport, FormField, FormFieldSpec, HorizontalAlign as LayoutHorizontalAlign, Image,
    ImageData, ImageSizing, Limits, LinkTarget, ListStyle, OpenMode, OpenReport, OutlineItem,
    OutputIntentProfile, PageSpec, PageStyle, PdfAConformance, PdfAError, PdfUaError, Permissions,
    RecoveryReason, RevocationSummary, RewriteMode, Semantic, SignSettings, SignatureAppearance,
    SignatureEffect, SignatureStatus, Size as LayoutSize, StdFont, StructElem, StructureEffect,
    Table, TextBlock, TextStyle, TransformReport, TsaCredentials,
    VerticalAlign as LayoutVerticalAlign, XmpMetadata, measure_text, wrap_text,
};

mod api;
pub use api::*;

#[cfg(test)]
#[path = "api/tests.rs"]
mod tests;
