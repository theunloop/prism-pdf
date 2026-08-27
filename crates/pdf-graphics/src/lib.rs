#![forbid(unsafe_code)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]
//! pdf-graphics — color spaces and image/form XObjects (EPIC 8, ISO 32000 §8).
//!
//! First capability (Milestone M4): extract image XObjects (§8.9) — transport filters decoded to
//! raw samples, image codecs (JPEG/JPEG 2000) passed through — with their color space (§8.6)
//! reduced to a component count. Depends on [`pdf_cos`] and [`pdf_filters`]
//! (architecture: `graphics → cos, filters`).
//!
//! Also implemented: **§7.10 functions** (type 0/2/3/4) — the numeric maps behind tint transforms,
//! shadings, and transfer curves ([`Function`], [`parse_function`]).
//!
//! Next: pixel decoding of image codecs, form XObjects (§8.10), shadings/patterns (§8.7), the
//! graphics-state machine (§8.4).

mod color;
mod function;
mod image;

pub use color::{
    ColorSpace, IndexedColorSpace, Separation, resolve_color_space, resolve_image_color_space,
    resolve_indexed, resolve_separation,
};
pub use function::{Function, parse_function};
pub use image::{ExtractedImage, ImageData, ImageInfo, extract_image};
