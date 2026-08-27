#![forbid(unsafe_code)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]
//! pdf-fonts — fonts & text (EPIC 7, ISO 32000 §9).
//!
//! First capability (Milestone M1, faithful text extraction): parsing a font's `/ToUnicode` CMap
//! (§9.10.3) so shown character codes can be mapped to Unicode. Depends on [`pdf_content`] for the
//! CMap tokenizer (architecture: `fonts → cos, content, filters`).
//!
//! Implemented (Milestone M1, faithful text extraction):
//! - **§9.10.3 — `/ToUnicode`**: [`ToUnicode`] CMap parsing.
//! - **§9.6.6 — Simple-font encoding**: [`Encoding`] (base encoding + `/Differences`), the
//!   fallback when a font has no `/ToUnicode`.
//! - **§9.7.5–6 — Composite (Type0) fonts**: [`CMap`] maps shown codes to CIDs (`Identity-H`/`-V`
//!   and embedded CMap streams), the read path for CID-keyed text (Milestone M9).
//!
//! Next: predefined CJK CMaps and CIDFontType0 (CFF) glyph recovery (§9.7).

mod cid;
mod cmap;
mod decode;
mod embed;
mod encoding;
mod program;
mod standard_metrics;
mod subset;

pub use cid::CMap;
pub use cmap::ToUnicode;
pub use decode::ResourceDecoder;
pub use embed::{FontInfo, Glyph, font_info, glyph_to_unicode, shape_text};
pub use encoding::{Encoding, winansi_encode};
pub use program::{FaceMetrics, FontProgramFormat, analyze_sfnt};
pub use standard_metrics::{standard_glyph_width, standard_text_width};
pub use subset::{glyphs_for_text, subset_sfnt, subset_with_map};
