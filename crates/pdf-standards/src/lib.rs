#![forbid(unsafe_code)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]
//! pdf-standards — conformance & XMP (EPIC 13, ISO 32000 §14).
//!
//! Milestone M7: PDF/A production at level B. So far this provides XMP metadata generation with the
//! PDF/A identification schema ([`xmp`]); the OutputIntent/ICC machinery and the conformant-output
//! pass build on it. See `DESIGN.md` and `docs/spec-map.md` for the sections this crate owns.

pub mod output_intent;
pub mod pdfa;
pub mod pdfua;
pub mod xmp;

pub use output_intent::{OutputIntentProfile, SRGB_ICC, output_intent_dict, srgb_icc_stream};
pub use pdfa::{PdfAError, derive_file_id, make_pdfa, make_pdfa_with_output_intent};
pub use pdfua::{PdfUaError, make_pdfua, make_pdfua2};
pub use xmp::{PdfAConformance, XmpMetadata, xmp_packet, xmp_packet_ua, xmp_packet_ua2};
