//! `FlateDecode` (§7.4.4.2) plus the predictor post-processing of §7.4.4.4.
//!
//! Reuse over reimplementation (DESIGN.md §6): inflation is delegated to [`flate2`]. PDF Flate
//! data is zlib (RFC 1950); some malformed producers emit raw DEFLATE (RFC 1951) instead, so we
//! fall back to that. Output is bounded to guard against decompression bombs (DESIGN.md §3.4).

use std::io::{Read, Write};

use flate2::Compression;
use flate2::read::{DeflateDecoder, ZlibDecoder};
use flate2::write::ZlibEncoder;
use pdf_cos::{Dictionary, Name};

use crate::error::{FilterError, Result};
use crate::trace::log_warn;

const FLATE: &str = "FlateDecode";

/// Compress `data` as `FlateDecode` (zlib, RFC 1950) — the inverse of [`flate_decode`] with no
/// predictor — for writing streams (§7.4.4.2). The round-trip `flate_decode(flate_encode(x)) == x`
/// holds for any input.
#[must_use]
pub fn flate_encode(data: &[u8]) -> Vec<u8> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    // Writing to an in-memory buffer cannot fail; finish() yields the compressed bytes.
    let _ = encoder.write_all(data);
    encoder.finish().unwrap_or_default()
}

/// Inflate `FlateDecode` data and apply any predictor from `params`, refusing to produce more
/// than `max_output` bytes (anti-DoS, DESIGN.md §3.4).
pub fn flate_decode(
    input: &[u8],
    params: Option<&Dictionary>,
    max_output: usize,
) -> Result<Vec<u8>> {
    let inflated = inflate_bounded(input, max_output)?;
    match params {
        Some(p) => apply_predictor(inflated, p),
        None => Ok(inflated),
    }
}

/// Inflate zlib data, falling back to raw DEFLATE, capping output at `max`.
fn inflate_bounded(input: &[u8], max: usize) -> Result<Vec<u8>> {
    match read_capped(ZlibDecoder::new(input), max) {
        Ok(out) => Ok(out),
        // A size-limit hit is a hard stop, not a reason to retry with a different codec.
        Err(e @ FilterError::TooLarge { .. }) => Err(e),
        Err(_) => {
            let out = read_capped(DeflateDecoder::new(input), max)?;
            log_warn!(
                "FlateDecode stream is raw DEFLATE (RFC 1951), not zlib (RFC 1950); \
                 decoded via fallback (§7.4.4.2)"
            );
            Ok(out)
        }
    }
}

/// Read a decoder to its end, erroring if it would exceed `max` bytes.
fn read_capped<R: Read>(reader: R, max: usize) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    // Read one byte past the limit so we can tell "exactly at the limit" from "over it".
    let cap = max.saturating_add(1) as u64;
    reader
        .take(cap)
        .read_to_end(&mut out)
        .map_err(|_| FilterError::Corrupt { filter: FLATE })?;
    if out.len() > max {
        return Err(FilterError::TooLarge { limit: max });
    }
    Ok(out)
}

/// Reverse a PNG (predictor ≥ 10) or TIFF (predictor 2) predictor (§7.4.4.4). Predictor ≤ 1 is a
/// no-op.
pub(crate) fn apply_predictor(data: Vec<u8>, params: &Dictionary) -> Result<Vec<u8>> {
    let predictor = params.get_integer(&Name::from("Predictor")).unwrap_or(1);
    if predictor <= 1 {
        return Ok(data);
    }

    let colors = positive(params, "Colors", 1)?;
    let bpc = positive(params, "BitsPerComponent", 8)?;
    let columns = positive(params, "Columns", 1)?;

    // Bytes per row and per pixel (PNG rounds the pixel up to at least one byte).
    let bits_per_pixel = colors.checked_mul(bpc).ok_or(invalid())?;
    let row_len = bits_per_pixel
        .checked_mul(columns)
        .map(|bits| bits.div_ceil(8))
        .ok_or(invalid())?;
    if row_len == 0 {
        return Err(invalid());
    }
    let bpp = bits_per_pixel.div_ceil(8).max(1);

    if predictor == 2 {
        return tiff_predictor2(data, colors, bpc, row_len);
    }
    png_predictor(&data, row_len, bpp)
}

/// Reverse PNG row filters (predictor ≥ 10). Each input row is one filter-type byte followed by
/// `row_len` bytes; trailing partial rows are ignored.
fn png_predictor(data: &[u8], row_len: usize, bpp: usize) -> Result<Vec<u8>> {
    let in_row = row_len + 1;
    let rows = data.len() / in_row;
    let mut out = Vec::with_capacity(rows * row_len);
    let mut prev = vec![0u8; row_len];

    for r in 0..rows {
        let base = r * in_row;
        let ftype = data[base];
        let mut cur = data[base + 1..base + 1 + row_len].to_vec();
        for i in 0..row_len {
            let left = if i >= bpp { cur[i - bpp] } else { 0 };
            let up = prev[i];
            let up_left = if i >= bpp { prev[i - bpp] } else { 0 };
            let add = match ftype {
                0 => 0,                                             // None
                1 => left,                                          // Sub
                2 => up,                                            // Up
                3 => ((u16::from(left) + u16::from(up)) / 2) as u8, // Average
                4 => paeth(left, up, up_left),                      // Paeth
                _ => return Err(FilterError::Corrupt { filter: FLATE }),
            };
            cur[i] = cur[i].wrapping_add(add);
        }
        out.extend_from_slice(&cur);
        prev = cur;
    }
    Ok(out)
}

/// Reverse the TIFF predictor 2 (horizontal differencing). Only 8-bit components are supported;
/// other depths are rare and rejected rather than mis-decoded.
fn tiff_predictor2(
    mut data: Vec<u8>,
    colors: usize,
    bpc: usize,
    row_len: usize,
) -> Result<Vec<u8>> {
    if bpc != 8 {
        return Err(invalid());
    }
    let rows = data.len() / row_len;
    for r in 0..rows {
        let base = r * row_len;
        for i in colors..row_len {
            data[base + i] = data[base + i].wrapping_add(data[base + i - colors]);
        }
    }
    Ok(data)
}

/// The PNG Paeth predictor function (§7.4.4.4 / PNG spec).
fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let (a, b, c) = (i16::from(a), i16::from(b), i16::from(c));
    let p = a + b - c;
    let pa = (p - a).abs();
    let pb = (p - b).abs();
    let pc = (p - c).abs();
    if pa <= pb && pa <= pc {
        a as u8
    } else if pb <= pc {
        b as u8
    } else {
        c as u8
    }
}

/// Read a strictly-positive integer parameter, falling back to `default`.
fn positive(params: &Dictionary, key: &str, default: i64) -> Result<usize> {
    let v = params.get_integer(&Name::from(key)).unwrap_or(default);
    usize::try_from(v)
        .ok()
        .filter(|&n| n > 0)
        .ok_or_else(invalid)
}

fn invalid() -> FilterError {
    FilterError::InvalidParams { filter: FLATE }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::Compression;
    use flate2::write::ZlibEncoder;
    use std::io::Write;

    fn zlib(data: &[u8]) -> Vec<u8> {
        let mut e = ZlibEncoder::new(Vec::new(), Compression::default());
        e.write_all(data).unwrap();
        e.finish().unwrap()
    }

    #[test]
    fn flate_encode_round_trips() {
        for input in [b"".to_vec(), b"hello".to_vec(), vec![0u8; 5000]] {
            let encoded = flate_encode(&input);
            assert_eq!(flate_decode(&encoded, None, 1 << 20).unwrap(), input);
        }
    }

    #[test]
    fn roundtrip_plain_flate() {
        let original = b"The quick brown fox jumps over the lazy dog.".repeat(20);
        let encoded = zlib(&original);
        assert_eq!(flate_decode(&encoded, None, 1 << 20).unwrap(), original);
    }

    #[test]
    fn decompression_bomb_is_bounded() {
        // 1 MiB of zeros compresses tiny; a small limit must reject it (DESIGN.md §3.4).
        let encoded = zlib(&vec![0u8; 1 << 20]);
        assert_eq!(
            flate_decode(&encoded, None, 4096).unwrap_err(),
            FilterError::TooLarge { limit: 4096 }
        );
    }

    #[test]
    fn corrupt_flate_errors() {
        assert_eq!(
            flate_decode(b"not zlib data at all", None, 1 << 20).unwrap_err(),
            FilterError::Corrupt { filter: FLATE }
        );
    }

    #[test]
    fn png_up_predictor_roundtrip() {
        // Two rows of 3 bytes, PNG "Up" filter (type 2): row2 stored as row2-row1.
        // Decoded rows should be [10,20,30] and [11,22,33].
        let mut params = Dictionary::new();
        params.insert(Name::from("Predictor"), pdf_cos::Object::Integer(12));
        params.insert(Name::from("Columns"), pdf_cos::Object::Integer(3));
        // filter-type 0 (None) for row1, type 2 (Up) for row2 storing deltas 1,2,3.
        let encoded = zlib(&[0, 10, 20, 30, 2, 1, 2, 3]);
        let out = flate_decode(&encoded, Some(&params), 1 << 20).unwrap();
        assert_eq!(out, vec![10, 20, 30, 11, 22, 33]);
    }

    #[test]
    fn tiff_predictor2_roundtrip() {
        // One row, 4 samples, Colors=1 BPC=8: stored as horizontal deltas of [5,7,10,10].
        let mut params = Dictionary::new();
        params.insert(Name::from("Predictor"), pdf_cos::Object::Integer(2));
        params.insert(Name::from("Columns"), pdf_cos::Object::Integer(4));
        let encoded = zlib(&[5, 2, 3, 0]); // 5, 5+2=7, 7+3=10, 10+0=10
        let out = flate_decode(&encoded, Some(&params), 1 << 20).unwrap();
        assert_eq!(out, vec![5, 7, 10, 10]);
    }

    /// Raw DEFLATE (no zlib header), which some malformed producers emit.
    fn raw_deflate(data: &[u8]) -> Vec<u8> {
        use flate2::Compression;
        use flate2::write::DeflateEncoder;
        let mut e = DeflateEncoder::new(Vec::new(), Compression::default());
        e.write_all(data).unwrap();
        e.finish().unwrap()
    }

    #[test]
    fn raw_deflate_fallback() {
        let original = b"raw deflate, no zlib header";
        assert_eq!(
            flate_decode(&raw_deflate(original), None, 1 << 20).unwrap(),
            original
        );
    }

    /// The `tracing` feature's whole wiring, end to end: a shim macro call site actually reaches
    /// an installed subscriber. One behavioural test here covers the pattern for every crate;
    /// the no-op arm is covered by every default-feature build of this same test file.
    #[cfg(feature = "tracing")]
    mod tracing_events {
        use super::*;
        use std::fmt;
        use std::sync::{Arc, Mutex};
        use tracing::field::{Field, Visit};
        use tracing::{Event, Level, Metadata, span};

        /// Minimal collector: `tracing-subscriber` is deliberately not a dependency of any
        /// engine crate (the consumer owns the subscriber), so the test hand-rolls one.
        #[derive(Clone, Default)]
        struct Collector(Arc<Mutex<Vec<(Level, String)>>>);

        struct MessageVisitor(String);
        impl Visit for MessageVisitor {
            fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
                if field.name() == "message" {
                    self.0 = format!("{value:?}");
                }
            }
        }

        impl tracing::Subscriber for Collector {
            fn enabled(&self, _: &Metadata<'_>) -> bool {
                true
            }
            fn new_span(&self, _: &span::Attributes<'_>) -> span::Id {
                span::Id::from_u64(1)
            }
            fn record(&self, _: &span::Id, _: &span::Record<'_>) {}
            fn record_follows_from(&self, _: &span::Id, _: &span::Id) {}
            fn event(&self, event: &Event<'_>) {
                let mut visitor = MessageVisitor(String::new());
                event.record(&mut visitor);
                self.0
                    .lock()
                    .unwrap()
                    .push((*event.metadata().level(), visitor.0));
            }
            fn enter(&self, _: &span::Id) {}
            fn exit(&self, _: &span::Id) {}
        }

        #[test]
        fn raw_deflate_fallback_emits_a_warning() {
            let collector = Collector::default();
            let events = Arc::clone(&collector.0);
            tracing::subscriber::with_default(collector, || {
                flate_decode(&raw_deflate(b"event under test"), None, 1 << 20).unwrap();
            });
            let events = events.lock().unwrap();
            assert!(
                events
                    .iter()
                    .any(|(level, msg)| *level == Level::WARN && msg.contains("raw DEFLATE")),
                "no raw-DEFLATE warning among: {events:?}"
            );
        }

        #[test]
        fn clean_zlib_emits_nothing() {
            let collector = Collector::default();
            let events = Arc::clone(&collector.0);
            tracing::subscriber::with_default(collector, || {
                flate_decode(&zlib(b"well-formed"), None, 1 << 20).unwrap();
            });
            assert_eq!(*events.lock().unwrap(), vec![]);
        }
    }

    #[test]
    fn predictor_one_is_noop() {
        let mut params = Dictionary::new();
        params.insert(Name::from("Predictor"), pdf_cos::Object::Integer(1));
        let encoded = zlib(&[1, 2, 3]);
        assert_eq!(
            flate_decode(&encoded, Some(&params), 1 << 20).unwrap(),
            vec![1, 2, 3]
        );
    }

    #[test]
    fn png_sub_average_paeth_predictors() {
        // Row of 4 bytes, Columns=4, with filter type covering Sub(1)/Average(3)/Paeth(4).
        for ftype in [1u8, 3, 4] {
            let mut params = Dictionary::new();
            params.insert(Name::from("Predictor"), pdf_cos::Object::Integer(10));
            params.insert(Name::from("Columns"), pdf_cos::Object::Integer(4));
            let encoded = zlib(&[ftype, 1, 2, 3, 4]);
            // Just assert it decodes to 4 bytes without panicking (values depend on filter math).
            let out = flate_decode(&encoded, Some(&params), 1 << 20).unwrap();
            assert_eq!(out.len(), 4, "filter type {ftype}");
        }
    }

    #[test]
    fn unknown_png_filter_type_is_corrupt() {
        let mut params = Dictionary::new();
        params.insert(Name::from("Predictor"), pdf_cos::Object::Integer(12));
        params.insert(Name::from("Columns"), pdf_cos::Object::Integer(2));
        let encoded = zlib(&[9, 1, 2]); // filter type 9 is invalid
        assert_eq!(
            flate_decode(&encoded, Some(&params), 1 << 20).unwrap_err(),
            FilterError::Corrupt { filter: FLATE }
        );
    }

    #[test]
    fn invalid_predictor_params_rejected() {
        // TIFF predictor 2 only supports 8-bit components.
        let mut params = Dictionary::new();
        params.insert(Name::from("Predictor"), pdf_cos::Object::Integer(2));
        params.insert(Name::from("BitsPerComponent"), pdf_cos::Object::Integer(4));
        assert_eq!(
            flate_decode(&zlib(&[1, 2]), Some(&params), 1 << 20).unwrap_err(),
            FilterError::InvalidParams { filter: FLATE }
        );

        // A zero /Columns is invalid.
        let mut params = Dictionary::new();
        params.insert(Name::from("Predictor"), pdf_cos::Object::Integer(12));
        params.insert(Name::from("Columns"), pdf_cos::Object::Integer(0));
        assert_eq!(
            flate_decode(&zlib(&[0, 1]), Some(&params), 1 << 20).unwrap_err(),
            FilterError::InvalidParams { filter: FLATE }
        );
    }

    #[test]
    fn tiff_predictor_multi_color() {
        // Colors=2, BPC=8, Columns=2 -> row of 4 bytes; horizontal diff per colour channel.
        let mut params = Dictionary::new();
        params.insert(Name::from("Predictor"), pdf_cos::Object::Integer(2));
        params.insert(Name::from("Colors"), pdf_cos::Object::Integer(2));
        params.insert(Name::from("Columns"), pdf_cos::Object::Integer(2));
        // deltas: [10, 20, 1, 2] -> [10, 20, 11, 22]
        let out = flate_decode(&zlib(&[10, 20, 1, 2]), Some(&params), 1 << 20).unwrap();
        assert_eq!(out, vec![10, 20, 11, 22]);
    }
}
