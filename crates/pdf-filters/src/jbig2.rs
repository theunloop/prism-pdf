//! JBIG2Decode (ISO 32000-1 §7.4.7): decode bi-level (1 bit/pixel) image data via the
//! memory-safe, pure-Rust [`hayro_jbig2`] decoder (DESIGN.md §6, reuse over reimplementation).
//!
//! **Output convention.** The decoded bytes are packed 1-bit samples, MSB-first, each row padded
//! to a byte boundary — the layout a 1-bpc image XObject expects. Polarity follows the *normal PDF
//! image convention*: sample **0 = black, 1 = white**. This is the inverse of JBIG2's own
//! convention (where a 1 bit is black), so each pixel is inverted as it is packed (§7.4.7; cf. the
//! `BlackIs1` note in §7.4.6, which calls 1=black "the reverse of the normal PDF convention").
//!
//! **Globals.** A JBIG2 stream may reference shared segments held in a separate `/JBIG2Globals`
//! stream (§7.4.7, Table 12). That parameter is an *indirect* stream reference, which this low
//! filter layer cannot resolve (architecture: `filters → cos`, no document/resolver). So the
//! filter-chain entry point decodes with no globals; a caller that has a resolver (the image layer)
//! extracts the globals bytes and calls [`jbig2_decode`] directly.
//!
//! PDF embeds JBIG2 using the *embedded* organization (Annex D.3 — segment headers only, no file
//! header), so [`hayro_jbig2::Image::new_embedded`] is always the right entry point here.

use hayro_jbig2::{Decoder, Image};

use crate::error::{FilterError, Result};

const FILTER: &str = "JBIG2Decode";

/// Decode a JBIG2 image stream into packed 1-bpp samples (see module docs for layout/polarity).
///
/// `globals` is the optional `/JBIG2Globals` segment stream. `max` bounds the work: the image's
/// pixel count is rejected when it exceeds `max`, which caps both this output and the decoder's
/// intermediate page bitmap (anti-DoS, DESIGN.md §3.4). Any malformed input yields
/// [`FilterError::Corrupt`] rather than a panic.
pub fn jbig2_decode(data: &[u8], globals: Option<&[u8]>, max: usize) -> Result<Vec<u8>> {
    let image =
        Image::new_embedded(data, globals).map_err(|_| FilterError::Corrupt { filter: FILTER })?;

    let width = image.width() as usize;
    let height = image.height() as usize;

    // Bound before decoding: reject implausible dimensions so neither our packed buffer nor the
    // decoder's intermediate bitmap can be driven to exhaust memory by a hostile header.
    let pixels = width
        .checked_mul(height)
        .filter(|&p| p <= max)
        .ok_or(FilterError::TooLarge { limit: max })?;
    if pixels == 0 {
        return Ok(Vec::new());
    }

    let row_bytes = width.div_ceil(8);
    let capacity = row_bytes.saturating_mul(height);
    let mut sink = BitPacker::new(capacity);
    image
        .decode(&mut sink)
        .map_err(|_| FilterError::Corrupt { filter: FILTER })?;

    Ok(sink.into_bytes())
}

/// A [`Decoder`] sink that packs pixels into PDF 1-bpp rows: MSB-first, byte-aligned per row, with
/// black → 0 and white → 1 (the inversion of JBIG2's convention).
struct BitPacker {
    out: Vec<u8>,
    /// Current partial byte, filled from the most significant bit downward.
    cur: u8,
    /// Number of bits currently held in `cur` (0..8).
    nbits: u8,
}

impl BitPacker {
    fn new(capacity: usize) -> Self {
        Self {
            out: Vec::with_capacity(capacity),
            cur: 0,
            nbits: 0,
        }
    }

    /// Append a single sample bit (1 = white, 0 = black), flushing a full byte when complete.
    fn push_bit(&mut self, bit: u8) {
        self.cur = (self.cur << 1) | (bit & 1);
        self.nbits += 1;
        if self.nbits == 8 {
            self.out.push(self.cur);
            self.cur = 0;
            self.nbits = 0;
        }
    }

    fn into_bytes(mut self) -> Vec<u8> {
        // Flush any trailing partial byte (defensive: a row of width not a multiple of 8 with no
        // closing `next_line` would otherwise be lost).
        if self.nbits > 0 {
            self.cur <<= 8 - self.nbits;
            self.out.push(self.cur);
        }
        self.out
    }
}

impl Decoder for BitPacker {
    fn push_pixel(&mut self, black: bool) {
        // PDF sample: 0 for black, 1 for white.
        self.push_bit(u8::from(!black));
    }

    fn push_pixel_chunk(&mut self, black: bool, chunk_count: u32) {
        // Each chunk is 8 same-coloured pixels = one packed byte; called only on a byte boundary
        // (hayro contract), so the bytes can be emitted directly. Black → 0x00, white → 0xFF.
        let byte = if black { 0x00 } else { 0xFF };
        self.out
            .extend(std::iter::repeat_n(byte, chunk_count as usize));
    }

    fn next_line(&mut self) {
        // Pad the row out to a byte boundary (padding bits lie beyond the image width).
        if self.nbits > 0 {
            self.cur <<= 8 - self.nbits;
            self.out.push(self.cur);
            self.cur = 0;
            self.nbits = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Hex-decode, ignoring whitespace (test fixtures only).
    fn unhex(s: &str) -> Vec<u8> {
        let h: Vec<u8> = s.bytes().filter(u8::is_ascii_hexdigit).collect();
        h.chunks_exact(2)
            .map(|c| {
                let hi = (c[0] as char).to_digit(16).unwrap();
                let lo = (c[1] as char).to_digit(16).unwrap();
                (hi * 16 + lo) as u8
            })
            .collect()
    }

    /// The worked JBIG2 example from ISO 32000-1 §7.4.7 (EXAMPLE 1/2): a 52×66 image that uses a
    /// symbol dictionary held in a separate globals stream.
    const IMAGE: &str = "000000013000010000001300000034000000420000000000\
        00000040000000000002062000010000001e000000340000\
        004200000000000000000200100000000231db51ce51ffac";
    const GLOBALS: &str = "0000000000010000000032000003fffdff02fefefe000000\
        01000000012ae225aea9a5a538b4d9999c5c8e56ef0f872\
        7f2b53d4e37ef795cc5506dffac";

    #[test]
    fn decodes_spec_example_with_globals() {
        let img = unhex(IMAGE);
        let globals = unhex(GLOBALS);
        let out = jbig2_decode(&img, Some(&globals), 1 << 20).expect("decodes");

        // 52 px wide → 7 bytes/row (4 padding bits); 66 rows.
        let row_bytes = 52usize.div_ceil(8);
        assert_eq!(row_bytes, 7);
        assert_eq!(out.len(), row_bytes * 66);

        // Count black samples (bit == 0) within the 52-px image area. The spec image has 234.
        let mut black = 0;
        for row in out.chunks_exact(row_bytes) {
            for x in 0..52usize {
                let bit = (row[x / 8] >> (7 - (x % 8))) & 1;
                if bit == 0 {
                    black += 1;
                }
            }
        }
        assert_eq!(black, 234, "black pixel count from §7.4.7 example");
    }

    #[test]
    fn missing_globals_fails_gracefully() {
        // The image references symbols defined only in the globals stream; without them it cannot
        // decode — but it must return an error, never panic.
        let img = unhex(IMAGE);
        assert!(matches!(
            jbig2_decode(&img, None, 1 << 20),
            Err(FilterError::Corrupt {
                filter: "JBIG2Decode"
            })
        ));
    }

    #[test]
    fn hostile_inputs_never_panic() {
        for data in [
            [].as_slice(),
            &[0u8; 64],
            &[0xFFu8; 256],
            &[0x00, 0x00, 0x00, 0x0C, 0x6A, 0x50],
        ] {
            // Either a clean decode or a clean error — never a panic.
            let _ = jbig2_decode(data, None, 1 << 20);
        }
    }

    #[test]
    fn oversized_dimensions_are_rejected() {
        // A header claiming a huge page must be refused by the pixel-count guard, not allocated.
        let globals = unhex(GLOBALS);
        let img = unhex(IMAGE);
        assert!(matches!(
            jbig2_decode(&img, Some(&globals), 16), // 52*66 = 3432 pixels ≫ 16
            Err(FilterError::TooLarge { limit: 16 })
        ));
    }

    #[test]
    fn bit_packing_polarity_and_alignment() {
        // Drive the sink directly: black,white,black across a 3-px row → bits 0,1,0 then pad.
        let mut p = BitPacker::new(0);
        p.push_pixel(true); // black → 0
        p.push_pixel(false); // white → 1
        p.push_pixel(true); // black → 0
        p.next_line();
        // 010 then 5 padding zero bits = 0b0100_0000 = 0x40.
        assert_eq!(p.into_bytes(), vec![0x40]);
    }
}
