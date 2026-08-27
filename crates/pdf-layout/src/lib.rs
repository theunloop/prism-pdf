#![forbid(unsafe_code)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]
//! pdf-layout — high-level authoring/layout (EPIC 12, ISO 32000 §9.4 / §7.7).
//!
//! Builds on the operator builder ([`pdf_content::Content`]) and the document writer
//! ([`pdf_document::Builder`]): measure and wrap text in the Standard-14 fonts, lay out an aligned
//! block ([`draw_text_block`]), pour text across pages with automatic page breaks ([`Flow`]), and
//! lay out [`Table`]s. This is the start of the iText-like authoring surface.

mod compose;
mod flow;
mod image;
mod metrics;
mod table;
mod text;

pub use compose::{
    Color, Column, ComposeError, ComposeTable, ComposeTableRow, ComposedDocument, Composition,
    Container, GeometryEvent, GeometryTrace, HorizontalAlign, ImageSizing, Page, Plan, Point, Rect,
    Row, Semantic, Size, TextStyle, VerticalAlign,
};
pub use flow::{Flow, ListStyle, PageStyle};
pub use image::Image;
pub use metrics::FontMetrics;
pub use table::Table;
pub use text::{Align, TextBlock, draw_text_block, measure_text, wrap_text};
