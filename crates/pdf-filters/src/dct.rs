//! `DCTDecode` (§7.4.8): decode a baseline/progressive JPEG to interleaved image samples.
//!
//! Reuse over reimplementation (DESIGN.md §6): decoding is delegated to [`zune_jpeg`]. Output is
//! bounded to guard against decompression bombs (DESIGN.md §3.4) — a tiny JPEG can declare a huge
//! frame, so the size is checked from the headers *before* decoding.

use zune_jpeg::JpegDecoder;

use crate::error::{FilterError, Result};

const DCT: &str = "DCTDecode";

/// Decode `DCTDecode` (JPEG) data to interleaved samples (RGB, grayscale, or CMYK per the JPEG),
/// refusing to produce more than `max_output` bytes.
pub fn dct_decode(input: &[u8], max_output: usize) -> Result<Vec<u8>> {
    let mut decoder = JpegDecoder::new(input);
    decoder
        .decode_headers()
        .map_err(|_| FilterError::Corrupt { filter: DCT })?;

    // Reject an oversized frame before allocating its pixels. Use 4 (CMYK) as a conservative upper
    // bound on components; the exact size is checked again after decoding.
    if let Some((width, height)) = decoder.dimensions() {
        let estimate = width.saturating_mul(height).saturating_mul(4);
        if estimate > max_output {
            return Err(FilterError::TooLarge { limit: max_output });
        }
    }

    let pixels = decoder
        .decode()
        .map_err(|_| FilterError::Corrupt { filter: DCT })?;
    if pixels.len() > max_output {
        return Err(FilterError::TooLarge { limit: max_output });
    }
    Ok(pixels)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 2×2 RGB JPEG (created with ImageMagick), base64-encoded so the test is self-contained.
    const JPEG_2X2: &str = "/9j/4AAQSkZJRgABAQAAAQABAAD/2wBDAAMCAgICAgMCAgIDAwMDBAYEBAQEBAgGBgUGCQgKCgkICQkKDA8MCgsOCwkJDRENDg8QEBEQCgwSExIQEw8QEBD/2wBDAQMDAwQDBAgEBAgQCwkLEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBD/wAARCAACAAIDAREAAhEBAxEB/8QAFAABAAAAAAAAAAAAAAAAAAAACP/EABQQAQAAAAAAAAAAAAAAAAAAAAD/xAAVAQEBAAAAAAAAAAAAAAAAAAAHCf/EABQRAQAAAAAAAAAAAAAAAAAAAAD/2gAMAwEAAhEDEQA/ADoDFU3/2Q==";

    /// Decode the embedded base64 into bytes (tiny, ad-hoc; tests only).
    fn from_base64(s: &str) -> Vec<u8> {
        const fn val(c: u8) -> i32 {
            match c {
                b'A'..=b'Z' => (c - b'A') as i32,
                b'a'..=b'z' => (c - b'a' + 26) as i32,
                b'0'..=b'9' => (c - b'0' + 52) as i32,
                b'+' => 62,
                b'/' => 63,
                _ => -1,
            }
        }
        let mut acc = 0i32;
        let mut bits = 0;
        let mut out = Vec::new();
        for &c in s.as_bytes() {
            let v = val(c);
            if v < 0 {
                continue; // skip '=' padding and any whitespace
            }
            acc = (acc << 6) | v;
            bits += 6;
            if bits >= 8 {
                bits -= 8;
                out.push((acc >> bits) as u8);
            }
        }
        out
    }

    #[test]
    fn decodes_a_real_jpeg_to_samples() {
        let jpeg = from_base64(JPEG_2X2);
        let samples = dct_decode(&jpeg, 1 << 20).expect("decode 2x2 jpeg");
        // 2×2 pixels × 3 components (RGB).
        assert_eq!(samples.len(), 2 * 2 * 3);
    }

    #[test]
    fn corrupt_jpeg_errors() {
        assert_eq!(
            dct_decode(b"not a jpeg", 1 << 20).unwrap_err(),
            FilterError::Corrupt { filter: DCT }
        );
    }

    #[test]
    fn oversized_frame_is_rejected_before_decoding() {
        // The same JPEG, but with a 1-byte output cap: rejected from the headers.
        let jpeg = from_base64(JPEG_2X2);
        assert_eq!(
            dct_decode(&jpeg, 1).unwrap_err(),
            FilterError::TooLarge { limit: 1 }
        );
    }
}
