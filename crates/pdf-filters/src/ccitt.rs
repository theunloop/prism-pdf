//! `CCITTFaxDecode` (§7.4.6) — ITU-T T.4 (Group 3) and T.6 (Group 4) fax decoding.
//!
//! The CCITT filters reconstruct a bi-level (1 bit/pixel) image from the run-length, Huffman-coded
//! bitstream produced by fax machines. Three flavours, selected by the `/K` parameter (Table 11):
//! - **K = 0** — Group 3 *one-dimensional* (T.4): every row is a sequence of white/black run
//!   lengths, each coded with the modified-Huffman white/black tables.
//! - **K < 0** — Group 4 / *two-dimensional* (T.6): every row is coded against the row above it
//!   using pass/horizontal/vertical mode codes. This is by far the most common form in PDFs.
//! - **K > 0** — Group 3 *mixed* (T.4 2D): each row carries a 1-bit tag choosing 1D or 2D coding.
//!
//! Input is untrusted (DESIGN.md §3.4): the decoder never panics, bounds every run, and caps the
//! produced output. Output rows are packed MSB-first and padded to a byte boundary, matching the
//! image-data layout PDF expects.

use std::collections::HashMap;

use pdf_cos::{Dictionary, Name, Object};

use crate::error::{FilterError, Result};

const CCITT: &str = "CCITTFaxDecode";
/// No code in the white/black/mode tables is longer than this; reading past it without a match
/// means the bitstream is corrupt (or we walked into padding/EOFB).
const MAX_CODE_BITS: u8 = 14;

/// The `/DecodeParms` of a CCITT stream (§7.4.6, Table 11), with PDF defaults applied.
struct Params {
    /// `/K`: <0 ⇒ pure 2D (G4), 0 ⇒ pure 1D (G3), >0 ⇒ mixed 1D/2D (G3 2D).
    k: i64,
    /// `/Columns`: pixels per row (default 1728).
    columns: usize,
    /// `/Rows`: row count, 0 ⇒ unbounded (stop at EOFB / end of data).
    rows: usize,
    /// `/BlackIs1`: when false (default) a 0 bit is black and a 1 bit is white in the output.
    black_is_1: bool,
    /// `/EncodedByteAlign`: pad each coded row out to the next byte boundary.
    byte_align: bool,
    /// `/EndOfBlock`: stop at an end-of-block pattern even before `/Rows` is reached (default true).
    end_of_block: bool,
}

impl Params {
    fn from(params: Option<&Dictionary>) -> Self {
        let int = |key: &str, dflt: i64| {
            params
                .and_then(|p| p.get_integer(&Name::from(key)))
                .unwrap_or(dflt)
        };
        let flag = |key: &str, dflt: bool| {
            params
                .and_then(|p| p.get(&Name::from(key)))
                .and_then(Object::as_bool)
                .unwrap_or(dflt)
        };
        Params {
            k: int("K", 0),
            columns: int("Columns", 1728).max(0) as usize,
            rows: int("Rows", 0).max(0) as usize,
            black_is_1: flag("BlackIs1", false),
            byte_align: flag("EncodedByteAlign", false),
            end_of_block: flag("EndOfBlock", true),
        }
    }
}

/// Decode `CCITTFaxDecode` data, refusing to produce more than `max_output` bytes (anti-DoS).
pub fn ccitt_fax_decode(
    input: &[u8],
    params: Option<&Dictionary>,
    max_output: usize,
) -> Result<Vec<u8>> {
    let p = Params::from(params);
    if p.columns == 0 {
        return Err(FilterError::InvalidParams { filter: CCITT });
    }
    let tables = Tables::build();
    let (white_bit, black_bit) = if p.black_is_1 { (0u8, 1u8) } else { (1u8, 0u8) };

    let mut reader = BitReader::new(input);
    let mut writer = RowWriter::new();
    // The reference for the first 2D row is an imaginary all-white line: no transitions.
    let mut reference: Vec<usize> = Vec::new();
    let mut rows_done = 0usize;

    loop {
        if p.rows != 0 && rows_done >= p.rows {
            break;
        }
        if p.byte_align {
            reader.align_to_byte();
        }
        // Consume any EOL markers separating rows. Two in a row is the end-of-facsimile-block
        // (EOFB, §7.4.6) / return-to-control — the data is finished.
        let mut eols = 0;
        while reader.take_eol() {
            eols += 1;
            if eols >= 2 {
                break;
            }
        }
        if (eols >= 2 && p.end_of_block) || reader.at_end() {
            break;
        }

        let two_dimensional = if p.k < 0 {
            true
        } else if p.k == 0 {
            false
        } else {
            // K > 0: a leading tag bit selects the coding for this row (1 = 1D, 0 = 2D).
            match reader.next_bit() {
                Some(1) => false,
                Some(0) => true,
                _ => break,
            }
        };

        let row = if two_dimensional {
            decode_2d_row(&mut reader, &reference, p.columns, &tables)
        } else {
            decode_1d_row(&mut reader, p.columns, &tables)
        };
        let Some(row) = row else { break };

        writer.put_row(&row, p.columns, white_bit, black_bit);
        if writer.len() > max_output {
            return Err(FilterError::TooLarge { limit: max_output });
        }
        reference = row;
        rows_done += 1;
    }

    Ok(writer.finish())
}

/// Decode one Group 3 1D row into its list of colour-change positions (transitions), the row
/// starting white. `None` on a corrupt/short run.
fn decode_1d_row(reader: &mut BitReader, columns: usize, tables: &Tables) -> Option<Vec<usize>> {
    let mut transitions = Vec::new();
    let mut pos = 0usize;
    let mut white = true;
    while pos < columns {
        let map = if white { &tables.white } else { &tables.black };
        let run = read_run(reader, map, columns)?;
        pos = (pos + run).min(columns);
        transitions.push(pos);
        white = !white;
    }
    Some(transitions)
}

/// Decode one Group 4 / Group 3-2D row against `reference` (the previous row's transitions) into
/// its own transition list. Implements the pass/horizontal/vertical modes of T.6.
fn decode_2d_row(
    reader: &mut BitReader,
    reference: &[usize],
    columns: usize,
    tables: &Tables,
) -> Option<Vec<usize>> {
    let mut cur: Vec<usize> = Vec::new();
    let mut a0: isize = -1;
    let mut white = true;
    // Bound the per-row work: each mode advances a0, so at most ~columns iterations are legitimate.
    for _ in 0..=columns + 1 {
        if a0 >= columns as isize {
            break;
        }
        let (b1, b2) = find_b1_b2(reference, a0, white, columns);
        match read_symbol(reader, &tables.modes, MAX_CODE_BITS)? {
            Mode::Pass => {
                // The run of the current colour extends to b2; no colour change is recorded.
                if (b2 as isize) <= a0 {
                    return None; // no progress ⇒ corrupt
                }
                a0 = b2 as isize;
            }
            Mode::Horizontal => {
                let start = if a0 < 0 { 0 } else { a0 as usize };
                let map1 = if white { &tables.white } else { &tables.black };
                let map2 = if white { &tables.black } else { &tables.white };
                let run1 = read_run(reader, map1, columns)?;
                let run2 = read_run(reader, map2, columns)?;
                let a1 = (start + run1).min(columns);
                let a2 = (a1 + run2).min(columns);
                cur.push(a1);
                cur.push(a2);
                a0 = a2 as isize;
            }
            Mode::Vertical(delta) => {
                let a1 = (b1 as isize + delta as isize).clamp(0, columns as isize) as usize;
                cur.push(a1);
                a0 = a1 as isize;
                white = !white;
            }
            Mode::Eol => break,
            Mode::Extension => return None, // uncompressed/extension mode: unsupported
        }
    }
    Some(cur)
}

/// Locate b1 and b2 on the reference line (§7.4.6): b1 is the first changing element to the right of
/// a0 whose colour is opposite to a0's; b2 is the next change after b1. Transitions in `reference`
/// alternate colour starting black at index 0, so parity selects the right one.
fn find_b1_b2(reference: &[usize], a0: isize, white: bool, columns: usize) -> (usize, usize) {
    let mut i = 0;
    while i < reference.len() && (reference[i] as isize) <= a0 {
        i += 1;
    }
    // A changing element at an even index turns the line black; b1 must be opposite a0's colour.
    let want_even = white;
    if i < reference.len() && ((i % 2 == 0) != want_even) {
        i += 1;
    }
    let b1 = reference.get(i).copied().unwrap_or(columns);
    let b2 = reference.get(i + 1).copied().unwrap_or(columns);
    (b1, b2)
}

/// Read one run length: zero or more make-up codes (≥ 64) followed by a terminating code (0..=63).
/// Bounded to `cap` to defeat absurd runs on hostile input.
fn read_run(reader: &mut BitReader, map: &CodeMap<i32>, cap: usize) -> Option<usize> {
    let mut total = 0usize;
    loop {
        let run = read_symbol(reader, map, MAX_CODE_BITS)?;
        if run < 0 {
            return None;
        }
        total += run as usize;
        if total > cap.saturating_mul(2) + 64 {
            return None;
        }
        if run < 64 {
            return Some(total); // a terminating code closes the run
        }
    }
}

/// Match the next prefix code against `map` (the codes are prefix-free), reading bit by bit. `None`
/// at end of input or if no code matches within `max_bits`.
fn read_symbol<T: Copy>(reader: &mut BitReader, map: &CodeMap<T>, max_bits: u8) -> Option<T> {
    let mut len = 0u8;
    let mut code = 0u16;
    loop {
        let bit = reader.next_bit()?;
        code = (code << 1) | u16::from(bit);
        len += 1;
        if let Some(v) = map.get(&(len, code)) {
            return Some(*v);
        }
        if len >= max_bits {
            return None;
        }
    }
}

/// 2D coding modes (§7.4.6 / T.6 Table 1). `Vertical(d)` is V(d) with d ∈ [-3, 3] (V0 ⇒ d = 0).
#[derive(Clone, Copy)]
enum Mode {
    Pass,
    Horizontal,
    Vertical(i32),
    Eol,
    Extension,
}

type CodeMap<T> = HashMap<(u8, u16), T>;

/// The decoding tables (built once per stream): white runs, black runs, and 2D mode codes.
struct Tables {
    white: CodeMap<i32>,
    black: CodeMap<i32>,
    modes: CodeMap<Mode>,
}

impl Tables {
    fn build() -> Self {
        let mut white = HashMap::new();
        insert_runs(&mut white, WHITE_TERM);
        insert_runs(&mut white, WHITE_MAKEUP);
        insert_runs(&mut white, EXT_MAKEUP);

        let mut black = HashMap::new();
        insert_runs(&mut black, BLACK_TERM);
        insert_runs(&mut black, BLACK_MAKEUP);
        insert_runs(&mut black, EXT_MAKEUP);

        let mut modes = HashMap::new();
        for &(bits, mode) in MODE_CODES {
            let (len, code) = parse_bits(bits);
            modes.insert((len, code), mode);
        }
        Tables {
            white,
            black,
            modes,
        }
    }
}

fn insert_runs(map: &mut CodeMap<i32>, table: &[(i32, &str)]) {
    for &(run, bits) in table {
        let (len, code) = parse_bits(bits);
        map.insert((len, code), run);
    }
}

/// Parse a binary code string like `"00110101"` into `(bit length, value)`.
fn parse_bits(bits: &str) -> (u8, u16) {
    let mut code = 0u16;
    for b in bits.bytes() {
        code = (code << 1) | u16::from(b - b'0');
    }
    (bits.len() as u8, code)
}

/// An MSB-first bit reader over the encoded bytes.
struct BitReader<'a> {
    data: &'a [u8],
    bit_pos: usize,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        BitReader { data, bit_pos: 0 }
    }

    fn next_bit(&mut self) -> Option<u8> {
        if self.bit_pos >= self.data.len() * 8 {
            return None;
        }
        let byte = self.data[self.bit_pos / 8];
        let bit = (byte >> (7 - (self.bit_pos % 8))) & 1;
        self.bit_pos += 1;
        Some(bit)
    }

    fn at_end(&self) -> bool {
        self.bit_pos >= self.data.len() * 8
    }

    fn align_to_byte(&mut self) {
        if !self.bit_pos.is_multiple_of(8) {
            self.bit_pos = (self.bit_pos / 8 + 1) * 8;
        }
    }

    /// Peek the next `n` (≤ 16) bits without consuming, or `None` if fewer remain.
    fn peek(&self, n: usize) -> Option<u16> {
        if self.bit_pos + n > self.data.len() * 8 {
            return None;
        }
        let mut v = 0u16;
        for i in 0..n {
            let pos = self.bit_pos + i;
            let bit = (self.data[pos / 8] >> (7 - (pos % 8))) & 1;
            v = (v << 1) | u16::from(bit);
        }
        Some(v)
    }

    /// Consume an EOL code (`000000000001`, §7.4.6) if it is next; report whether one was found.
    fn take_eol(&mut self) -> bool {
        if self.peek(12) == Some(0b0000_0000_0001) {
            self.bit_pos += 12;
            true
        } else {
            false
        }
    }
}

/// Packs decoded pixels MSB-first, one bit per pixel, padding every row to a byte boundary.
struct RowWriter {
    out: Vec<u8>,
    cur: u8,
    bits: u8,
}

impl RowWriter {
    fn new() -> Self {
        RowWriter {
            out: Vec::new(),
            cur: 0,
            bits: 0,
        }
    }

    fn push_bit(&mut self, bit: u8) {
        self.cur = (self.cur << 1) | (bit & 1);
        self.bits += 1;
        if self.bits == 8 {
            self.out.push(self.cur);
            self.cur = 0;
            self.bits = 0;
        }
    }

    /// Render a row from its transition list (colour changes, starting white), then byte-align.
    fn put_row(&mut self, transitions: &[usize], columns: usize, white_bit: u8, black_bit: u8) {
        let mut col = 0usize;
        let mut white = true;
        for &t in transitions {
            let end = t.min(columns);
            let bit = if white { white_bit } else { black_bit };
            for _ in col..end {
                self.push_bit(bit);
            }
            col = end;
            white = !white;
            if col >= columns {
                break;
            }
        }
        let bit = if white { white_bit } else { black_bit };
        for _ in col..columns {
            self.push_bit(bit);
        }
        // Image-data rows are byte-aligned.
        if self.bits != 0 {
            self.cur <<= 8 - self.bits;
            self.out.push(self.cur);
            self.cur = 0;
            self.bits = 0;
        }
    }

    fn len(&self) -> usize {
        self.out.len()
    }

    fn finish(self) -> Vec<u8> {
        self.out
    }
}

// --- ITU-T T.4 modified-Huffman code tables (§7.4.6). -----------------------------------------
// Run lengths 0..=63 are terminating codes; 64..=1728 are make-up codes; 1792..=2560 are the
// shared extended make-up codes used by both colours.

const WHITE_TERM: &[(i32, &str)] = &[
    (0, "00110101"),
    (1, "000111"),
    (2, "0111"),
    (3, "1000"),
    (4, "1011"),
    (5, "1100"),
    (6, "1110"),
    (7, "1111"),
    (8, "10011"),
    (9, "10100"),
    (10, "00111"),
    (11, "01000"),
    (12, "001000"),
    (13, "000011"),
    (14, "110100"),
    (15, "110101"),
    (16, "101010"),
    (17, "101011"),
    (18, "0100111"),
    (19, "0001100"),
    (20, "0001000"),
    (21, "0010111"),
    (22, "0000011"),
    (23, "0000100"),
    (24, "0101000"),
    (25, "0101011"),
    (26, "0010011"),
    (27, "0100100"),
    (28, "0011000"),
    (29, "00000010"),
    (30, "00000011"),
    (31, "00011010"),
    (32, "00011011"),
    (33, "00010010"),
    (34, "00010011"),
    (35, "00010100"),
    (36, "00010101"),
    (37, "00010110"),
    (38, "00010111"),
    (39, "00101000"),
    (40, "00101001"),
    (41, "00101010"),
    (42, "00101011"),
    (43, "00101100"),
    (44, "00101101"),
    (45, "00000100"),
    (46, "00000101"),
    (47, "00001010"),
    (48, "00001011"),
    (49, "01010010"),
    (50, "01010011"),
    (51, "01010100"),
    (52, "01010101"),
    (53, "00100100"),
    (54, "00100101"),
    (55, "01011000"),
    (56, "01011001"),
    (57, "01011010"),
    (58, "01011011"),
    (59, "01001010"),
    (60, "01001011"),
    (61, "00110010"),
    (62, "00110011"),
    (63, "00110100"),
];

const WHITE_MAKEUP: &[(i32, &str)] = &[
    (64, "11011"),
    (128, "10010"),
    (192, "010111"),
    (256, "0110111"),
    (320, "00110110"),
    (384, "00110111"),
    (448, "01100100"),
    (512, "01100101"),
    (576, "01101000"),
    (640, "01100111"),
    (704, "011001100"),
    (768, "011001101"),
    (832, "011010010"),
    (896, "011010011"),
    (960, "011010100"),
    (1024, "011010101"),
    (1088, "011010110"),
    (1152, "011010111"),
    (1216, "011011000"),
    (1280, "011011001"),
    (1344, "011011010"),
    (1408, "011011011"),
    (1472, "010011000"),
    (1536, "010011001"),
    (1600, "010011010"),
    (1664, "011000"),
    (1728, "010011011"),
];

const BLACK_TERM: &[(i32, &str)] = &[
    (0, "0000110111"),
    (1, "010"),
    (2, "11"),
    (3, "10"),
    (4, "011"),
    (5, "0011"),
    (6, "0010"),
    (7, "00011"),
    (8, "000101"),
    (9, "000100"),
    (10, "0000100"),
    (11, "0000101"),
    (12, "0000111"),
    (13, "00000100"),
    (14, "00000111"),
    (15, "000011000"),
    (16, "0000010111"),
    (17, "0000011000"),
    (18, "0000001000"),
    (19, "00001100111"),
    (20, "00001101000"),
    (21, "00001101100"),
    (22, "00000110111"),
    (23, "00000101000"),
    (24, "00000010111"),
    (25, "00000011000"),
    (26, "000011001010"),
    (27, "000011001011"),
    (28, "000011001100"),
    (29, "000011001101"),
    (30, "000001101000"),
    (31, "000001101001"),
    (32, "000001101010"),
    (33, "000001101011"),
    (34, "000011010010"),
    (35, "000011010011"),
    (36, "000011010100"),
    (37, "000011010101"),
    (38, "000011010110"),
    (39, "000011010111"),
    (40, "000001101100"),
    (41, "000001101101"),
    (42, "000011011010"),
    (43, "000011011011"),
    (44, "000001010100"),
    (45, "000001010101"),
    (46, "000001010110"),
    (47, "000001010111"),
    (48, "000001100100"),
    (49, "000001100101"),
    (50, "000001010010"),
    (51, "000001010011"),
    (52, "000000100100"),
    (53, "000000110111"),
    (54, "000000111000"),
    (55, "000000100111"),
    (56, "000000101000"),
    (57, "000001011000"),
    (58, "000001011001"),
    (59, "000000101011"),
    (60, "000000101100"),
    (61, "000001011010"),
    (62, "000001100110"),
    (63, "000001100111"),
];

const BLACK_MAKEUP: &[(i32, &str)] = &[
    (64, "0000001111"),
    (128, "000011001000"),
    (192, "000011001001"),
    (256, "000001011011"),
    (320, "000000110011"),
    (384, "000000110100"),
    (448, "000000110101"),
    (512, "0000001101100"),
    (576, "0000001101101"),
    (640, "0000001001010"),
    (704, "0000001001011"),
    (768, "0000001001100"),
    (832, "0000001001101"),
    (896, "0000001110010"),
    (960, "0000001110011"),
    (1024, "0000001110100"),
    (1088, "0000001110101"),
    (1152, "0000001110110"),
    (1216, "0000001110111"),
    (1280, "0000001010010"),
    (1344, "0000001010011"),
    (1408, "0000001010100"),
    (1472, "0000001010101"),
    (1536, "0000001011010"),
    (1600, "0000001011011"),
    (1664, "0000001100100"),
    (1728, "0000001100101"),
];

/// Extended make-up codes (1792..=2560), shared by both colours.
const EXT_MAKEUP: &[(i32, &str)] = &[
    (1792, "00000001000"),
    (1856, "00000001100"),
    (1920, "00000001101"),
    (1984, "000000010010"),
    (2048, "000000010011"),
    (2112, "000000010100"),
    (2176, "000000010101"),
    (2240, "000000010110"),
    (2304, "000000010111"),
    (2368, "000000011100"),
    (2432, "000000011101"),
    (2496, "000000011110"),
    (2560, "000000011111"),
];

/// 2D mode codes (§7.4.6 / T.6 Table 1).
const MODE_CODES: &[(&str, Mode)] = &[
    ("1", Mode::Vertical(0)),
    ("011", Mode::Vertical(1)),
    ("000011", Mode::Vertical(2)),
    ("0000011", Mode::Vertical(3)),
    ("010", Mode::Vertical(-1)),
    ("000010", Mode::Vertical(-2)),
    ("0000010", Mode::Vertical(-3)),
    ("001", Mode::Horizontal),
    ("0001", Mode::Pass),
    ("000000000001", Mode::Eol),
    ("0000001", Mode::Extension), // 2D extension (uncompressed mode), unsupported
];

#[cfg(test)]
mod tests {
    use super::*;

    /// A test-only MSB-first bit writer used to hand-build encoded streams.
    struct BitWriter {
        out: Vec<u8>,
        cur: u8,
        bits: u8,
    }
    impl BitWriter {
        fn new() -> Self {
            BitWriter {
                out: Vec::new(),
                cur: 0,
                bits: 0,
            }
        }
        fn put_str(&mut self, s: &str) {
            for b in s.bytes() {
                self.cur = (self.cur << 1) | (b - b'0');
                self.bits += 1;
                if self.bits == 8 {
                    self.out.push(self.cur);
                    self.cur = 0;
                    self.bits = 0;
                }
            }
        }
        fn finish(mut self) -> Vec<u8> {
            if self.bits != 0 {
                self.cur <<= 8 - self.bits;
                self.out.push(self.cur);
            }
            self.out
        }
    }

    /// Look up the code string for a run in a (run, bits) table.
    fn code_for(table: &'static [(i32, &'static str)], run: i32) -> &'static str {
        table
            .iter()
            .find(|&&(r, _)| r == run)
            .map(|&(_, s)| s)
            .unwrap()
    }

    /// Encode one run as make-up codes (greedy, largest first) + a terminating code (G3 1D).
    fn enc_run(w: &mut BitWriter, mut run: usize, white: bool) {
        let (term, makeup) = if white {
            (WHITE_TERM, WHITE_MAKEUP)
        } else {
            (BLACK_TERM, BLACK_MAKEUP)
        };
        while run >= 64 {
            let pick = makeup
                .iter()
                .map(|&(r, _)| r as usize)
                .filter(|&r| r <= run)
                .max()
                .unwrap();
            w.put_str(code_for(makeup, pick as i32));
            run -= pick;
        }
        w.put_str(code_for(term, run as i32));
    }

    /// Encode a full bitmap (row-major, `true` = black pixel) as Group 3 1D.
    fn encode_g3_1d(rows: &[Vec<bool>]) -> Vec<u8> {
        let mut w = BitWriter::new();
        for row in rows {
            let mut col = 0;
            let mut white = true;
            while col < row.len() {
                let start = col;
                while col < row.len() && row[col] != white {
                    col += 1;
                }
                enc_run(&mut w, col - start, white);
                white = !white;
            }
        }
        w.finish()
    }

    fn parms(pairs: &[(&str, Object)]) -> Dictionary {
        let mut d = Dictionary::new();
        for (k, v) in pairs {
            d.insert(Name::from(*k), v.clone());
        }
        d
    }

    /// Decode `out` (default convention: 1 = white, 0 = black) back into a bitmap of `true` = black.
    fn to_bitmap(out: &[u8], columns: usize, rows: usize) -> Vec<Vec<bool>> {
        let row_bytes = columns.div_ceil(8);
        (0..rows)
            .map(|r| {
                (0..columns)
                    .map(|c| {
                        let byte = out[r * row_bytes + c / 8];
                        let bit = (byte >> (7 - (c % 8))) & 1;
                        bit == 0 // 0 = black under the default (BlackIs1 = false) convention
                    })
                    .collect()
            })
            .collect()
    }

    #[test]
    fn group3_1d_round_trips() {
        // A 16×3 bitmap with mixed runs, including a run > 63 (needs a make-up code on row 2).
        let rows = vec![
            vec![
                false, false, false, true, true, false, false, false, true, false, false, false,
                false, false, false, false,
            ],
            vec![true; 16],
            vec![false; 16],
        ];
        let encoded = encode_g3_1d(&rows);
        let d = parms(&[("K", Object::Integer(0)), ("Columns", Object::Integer(16))]);
        let out = ccitt_fax_decode(&encoded, Some(&d), 1 << 20).unwrap();
        assert_eq!(to_bitmap(&out, 16, 3), rows);
    }

    #[test]
    fn group3_1d_wide_run_uses_makeup() {
        // 200 pixels: 100 white, then 100 black — both runs exceed the terminating range.
        let mut row = vec![false; 100];
        row.extend(std::iter::repeat_n(true, 100));
        let encoded = encode_g3_1d(&[row.clone()]);
        let d = parms(&[("K", Object::Integer(0)), ("Columns", Object::Integer(200))]);
        let out = ccitt_fax_decode(&encoded, Some(&d), 1 << 20).unwrap();
        assert_eq!(to_bitmap(&out, 200, 1), vec![row]);
    }

    #[test]
    fn group4_all_white_via_v0() {
        // In 2D, an all-white row is a single V0 code: a1 = b1 = columns.
        let mut w = BitWriter::new();
        for _ in 0..3 {
            w.put_str("1"); // V0
        }
        let d = parms(&[
            ("K", Object::Integer(-1)),
            ("Columns", Object::Integer(8)),
            ("Rows", Object::Integer(3)),
        ]);
        let out = ccitt_fax_decode(&w.finish(), Some(&d), 1 << 20).unwrap();
        // 1 byte/row, all white ⇒ all 1 bits.
        assert_eq!(out, vec![0xFF, 0xFF, 0xFF]);
    }

    #[test]
    fn group4_all_black_via_horizontal() {
        // All-black row: Horizontal mode, white run 0 then black run 8 (= columns).
        let mut w = BitWriter::new();
        w.put_str("001"); // Horizontal
        enc_run_2d(&mut w, 0, true);
        enc_run_2d(&mut w, 8, false);
        let d = parms(&[
            ("K", Object::Integer(-1)),
            ("Columns", Object::Integer(8)),
            ("Rows", Object::Integer(1)),
        ]);
        let out = ccitt_fax_decode(&w.finish(), Some(&d), 1 << 20).unwrap();
        assert_eq!(out, vec![0x00]); // all black ⇒ all 0 bits
    }

    /// Run encoder reused by the 2D tests (same code tables, just clearer name).
    fn enc_run_2d(w: &mut BitWriter, run: usize, white: bool) {
        enc_run(w, run, white);
    }

    #[test]
    fn group4_vertical_copies_reference() {
        // Row 1: black 0..4, white 4..8, coded with Horizontal then a V0 to close the white run.
        // Row 2: identical to row 1, coded as three V0 codes copying each change straight down.
        let columns = 8;
        let mut w = BitWriter::new();
        // Row 1.
        w.put_str("001"); // Horizontal
        enc_run_2d(&mut w, 0, true); // white run 0
        enc_run_2d(&mut w, 4, false); // black run 4
        w.put_str("1"); // V0 → close the trailing white run at columns (8)
        // Row 2 (== row 1): V0 (a1=0, →black), V0 (a1=4, →white), V0 (a1=8).
        w.put_str("1");
        w.put_str("1");
        w.put_str("1");
        let d = parms(&[
            ("K", Object::Integer(-1)),
            ("Columns", Object::Integer(columns as i64)),
            ("Rows", Object::Integer(2)),
        ]);
        let out = ccitt_fax_decode(&w.finish(), Some(&d), 1 << 20).unwrap();
        // 0xF0 = 11110000 but black=0 ⇒ first 4 black, last 4 white ⇒ bits 0000_1111 = 0x0F.
        assert_eq!(out, vec![0x0F, 0x0F]);
    }

    #[test]
    fn black_is_1_inverts_output() {
        let mut w = BitWriter::new();
        w.put_str("1"); // V0 all-white row
        let d = parms(&[
            ("K", Object::Integer(-1)),
            ("Columns", Object::Integer(8)),
            ("Rows", Object::Integer(1)),
            ("BlackIs1", Object::Boolean(true)),
        ]);
        let out = ccitt_fax_decode(&w.finish(), Some(&d), 1 << 20).unwrap();
        // All white, but BlackIs1 ⇒ white = 0 bit.
        assert_eq!(out, vec![0x00]);
    }

    #[test]
    fn rows_limit_stops_decoding() {
        // Stream encodes 4 all-white rows, but /Rows says 2.
        let mut w = BitWriter::new();
        for _ in 0..4 {
            w.put_str("1");
        }
        let d = parms(&[
            ("K", Object::Integer(-1)),
            ("Columns", Object::Integer(8)),
            ("Rows", Object::Integer(2)),
        ]);
        let out = ccitt_fax_decode(&w.finish(), Some(&d), 1 << 20).unwrap();
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn encoded_byte_align_pads_rows() {
        // Two all-white G4 rows, each followed by byte padding so the next row starts byte-aligned.
        let mut w = BitWriter::new();
        w.put_str("1"); // row 1: V0
        let mut bytes = w.finish(); // already byte-padded by finish()
        // Row 2 in its own byte.
        let mut w2 = BitWriter::new();
        w2.put_str("1");
        bytes.extend(w2.finish());
        let d = parms(&[
            ("K", Object::Integer(-1)),
            ("Columns", Object::Integer(8)),
            ("Rows", Object::Integer(2)),
            ("EncodedByteAlign", Object::Boolean(true)),
        ]);
        let out = ccitt_fax_decode(&bytes, Some(&d), 1 << 20).unwrap();
        assert_eq!(out, vec![0xFF, 0xFF]);
    }

    #[test]
    fn output_is_bounded() {
        let mut w = BitWriter::new();
        for _ in 0..100 {
            w.put_str("1");
        }
        let d = parms(&[
            ("K", Object::Integer(-1)),
            ("Columns", Object::Integer(64)),
            ("Rows", Object::Integer(100)),
        ]);
        // Each row is 8 bytes; a 10-byte cap must trip after the first row or two.
        assert_eq!(
            ccitt_fax_decode(&w.finish(), Some(&d), 10).unwrap_err(),
            FilterError::TooLarge { limit: 10 }
        );
    }

    #[test]
    fn corrupt_2d_data_does_not_panic() {
        // Random bytes as a 2D stream: must return cleanly (Ok with short output or an error),
        // never panic or loop.
        let garbage = vec![0xAB, 0xCD, 0xEF, 0x12, 0x34];
        let d = parms(&[("K", Object::Integer(-1)), ("Columns", Object::Integer(32))]);
        let _ = ccitt_fax_decode(&garbage, Some(&d), 1 << 20);
    }
}
