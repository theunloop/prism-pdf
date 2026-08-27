//! `LZWDecode` (§7.4.4.2) plus the shared predictor post-processing of §7.4.4.4.
//!
//! PDF's LZW is the variable-width variant: codes start at 9 bits and grow to 12 as the table
//! fills, with the **EarlyChange** quirk (default 1) that bumps the width one code sooner than a
//! strict reading would. Codes are packed MSB-first. Two codes are reserved: 256 clears the table
//! (`ClearTable`) and 257 ends the data (`EOD`). Output is bounded to defeat decompression bombs
//! (DESIGN.md §3.4).

use pdf_cos::{Dictionary, Name};

use crate::error::{FilterError, Result};
use crate::flate::apply_predictor;

const LZW: &str = "LZWDecode";
const CLEAR_TABLE: u32 = 256;
const EOD: u32 = 257;
/// First dynamic code: 0..=255 are literals, 256 `ClearTable`, 257 `EOD`.
const FIRST_FREE: usize = 258;
const MAX_WIDTH: u32 = 12;

/// Decode `LZWDecode` data and apply any predictor from `params`, refusing to produce more than
/// `max_output` bytes (anti-DoS, DESIGN.md §3.4).
pub fn lzw_decode(input: &[u8], params: Option<&Dictionary>, max_output: usize) -> Result<Vec<u8>> {
    // /EarlyChange is 0 or 1 (default 1); anything else falls back to the default.
    let early_change = match params.and_then(|p| p.get_integer(&Name::from("EarlyChange"))) {
        Some(0) => 0,
        _ => 1,
    };
    let decoded = decode(input, early_change, max_output)?;
    match params {
        Some(p) => apply_predictor(decoded, p),
        None => Ok(decoded),
    }
}

/// The LZW state machine. `table[code]` is the byte string for that code; `prev` is the previous
/// output string, used to grow the table by one byte each step (§7.4.4.2).
fn decode(input: &[u8], early_change: usize, max: usize) -> Result<Vec<u8>> {
    let mut reader = BitReader::new(input);
    let mut table = fresh_table();
    let mut width = 9u32;
    let mut out: Vec<u8> = Vec::new();
    let mut prev: Option<Vec<u8>> = None;

    while let Some(code) = reader.next(width) {
        if code == CLEAR_TABLE {
            table.truncate(FIRST_FREE);
            width = 9;
            prev = None;
            continue;
        }
        if code == EOD {
            break;
        }

        let code = code as usize;
        // Resolve the code: a defined entry, or the classic "KwKwK" case where the code is the one
        // about to be added (its string is prev ++ prev[0]).
        let entry: Vec<u8> = if code < table.len() {
            table[code].clone()
        } else if code == table.len() {
            let p = prev.as_ref().ok_or_else(corrupt)?;
            let mut s = p.clone();
            s.push(s[0]);
            s
        } else {
            return Err(corrupt()); // a code beyond the next free slot is corrupt
        };

        out.extend_from_slice(&entry);
        if out.len() > max {
            return Err(FilterError::TooLarge { limit: max });
        }

        // Add prev ++ entry[0] as the next table entry, widening the code size as it fills.
        if let Some(p) = prev {
            let mut new_entry = p;
            new_entry.push(entry[0]);
            table.push(new_entry);
            if table.len() + early_change == (1usize << width) && width < MAX_WIDTH {
                width += 1;
            }
        }
        prev = Some(entry);
    }
    Ok(out)
}

/// A table seeded with the 256 single-byte literals and two empty slots for the control codes
/// (256/257), so dynamic entries begin at index 258.
fn fresh_table() -> Vec<Vec<u8>> {
    let mut table: Vec<Vec<u8>> = (0..=255u8).map(|b| vec![b]).collect();
    table.push(Vec::new()); // 256 ClearTable
    table.push(Vec::new()); // 257 EOD
    table
}

fn corrupt() -> FilterError {
    FilterError::Corrupt { filter: LZW }
}

/// An MSB-first bit reader over the input bytes.
struct BitReader<'a> {
    data: &'a [u8],
    bit_pos: usize,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        BitReader { data, bit_pos: 0 }
    }

    /// Read the next `width` bits (≤ 32) as a big-endian code, or `None` at end of input.
    fn next(&mut self, width: u32) -> Option<u32> {
        let width = width as usize;
        if self.bit_pos + width > self.data.len() * 8 {
            return None;
        }
        let mut code = 0u32;
        for _ in 0..width {
            let byte = self.data[self.bit_pos / 8];
            let bit = (byte >> (7 - (self.bit_pos % 8))) & 1;
            code = (code << 1) | u32::from(bit);
            self.bit_pos += 1;
        }
        Some(code)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A reference LZW encoder (variable width, EarlyChange=1, MSB-first) to round-trip against.
    /// Its width growth is kept in lock-step with the decoder by simulating the decoder's table
    /// length (`dsize`), which lags the encoder's own dictionary by one entry.
    fn encode(data: &[u8]) -> Vec<u8> {
        let mut writer = BitWriter::new();
        let mut table: std::collections::HashMap<Vec<u8>, u32> =
            (0..=255u8).map(|b| (vec![b], u32::from(b))).collect();
        let mut next_code = FIRST_FREE as u32;
        let mut width = 9u32;
        let mut dsize = FIRST_FREE; // simulated decoder table length
        let mut emitted = 0usize; // data codes emitted so far
        writer.put(CLEAR_TABLE, width);

        // Emit a data code at the width the decoder will read it with, then advance the decoder
        // simulation exactly as `decode` does (add an entry from the 2nd code on, then widen).
        let mut emit = |writer: &mut BitWriter, code: u32| {
            writer.put(code, width);
            if emitted > 0 {
                dsize += 1;
                if dsize + 1 == (1usize << width) && width < MAX_WIDTH {
                    width += 1;
                }
            }
            emitted += 1;
        };

        let mut current: Vec<u8> = Vec::new();
        for &byte in data {
            let mut extended = current.clone();
            extended.push(byte);
            if table.contains_key(&extended) {
                current = extended;
            } else {
                emit(&mut writer, table[&current]);
                table.insert(extended, next_code);
                next_code += 1;
                current = vec![byte];
            }
        }
        if !current.is_empty() {
            emit(&mut writer, table[&current]);
        }
        writer.put(EOD, width);
        writer.finish()
    }

    struct BitWriter {
        out: Vec<u8>,
        acc: u32,
        bits: u32,
    }
    impl BitWriter {
        fn new() -> Self {
            BitWriter {
                out: Vec::new(),
                acc: 0,
                bits: 0,
            }
        }
        fn put(&mut self, code: u32, width: u32) {
            self.acc = (self.acc << width) | code;
            self.bits += width;
            while self.bits >= 8 {
                self.bits -= 8;
                self.out.push((self.acc >> self.bits) as u8);
            }
        }
        fn finish(mut self) -> Vec<u8> {
            if self.bits > 0 {
                self.out.push((self.acc << (8 - self.bits)) as u8);
            }
            self.out
        }
    }

    #[test]
    fn round_trips_various_inputs() {
        for input in [
            b"".to_vec(),
            b"A".to_vec(),
            b"TOBEORNOTTOBEORTOBEORNOT".to_vec(),
            b"aaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_vec(),
            (0..=255u8).collect::<Vec<_>>().repeat(40), // forces width growth past 9 bits
        ] {
            let encoded = encode(&input);
            assert_eq!(lzw_decode(&encoded, None, 1 << 20).unwrap(), input);
        }
    }

    #[test]
    fn matches_the_canonical_spec_example() {
        // The "-----A---B" example whose compressed output is documented as
        // 80 0B 60 50 22 0C 0C 85 01 (TIFF 6.0 §13 / the same LZW PDF uses). Validates both the
        // decoder (against authoritative bytes) and the reference encoder (it reproduces them).
        let original = b"-----A---B";
        let spec = [0x80, 0x0B, 0x60, 0x50, 0x22, 0x0C, 0x0C, 0x85, 0x01];
        assert_eq!(encode(original), spec);
        assert_eq!(lzw_decode(&spec, None, 1 << 20).unwrap(), original);
    }

    #[test]
    fn output_is_bounded() {
        let encoded = encode(&vec![b'z'; 10_000]);
        assert_eq!(
            lzw_decode(&encoded, None, 100).unwrap_err(),
            FilterError::TooLarge { limit: 100 }
        );
    }

    #[test]
    fn a_code_beyond_the_table_is_corrupt() {
        // ClearTable then an immediate high code (300) that cannot exist yet.
        let mut w = BitWriter::new();
        w.put(CLEAR_TABLE, 9);
        w.put(300, 9);
        assert_eq!(
            lzw_decode(&w.finish(), None, 1 << 20).unwrap_err(),
            FilterError::Corrupt { filter: LZW }
        );
    }
}
