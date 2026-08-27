//! `RunLengthDecode` (§7.4.5): a simple byte-oriented run-length scheme.
//!
//! The data is a sequence of length-prefixed runs. A length byte `L`:
//! - `0..=127`: the next `L + 1` bytes are literal,
//! - `129..=255`: the next single byte is repeated `257 - L` times,
//! - `128`: end of data.

use crate::error::{FilterError, Result};

const RL: &str = "RunLengthDecode";

/// Decode `RunLengthDecode` data (§7.4.5). A missing EOD (`128`) at the end is tolerated; a run
/// that claims more literal bytes than remain is rejected.
pub fn run_length_decode(input: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(input.len());
    let mut i = 0;
    while i < input.len() {
        let length = input[i];
        i += 1;
        match length {
            128 => break, // EOD
            0..=127 => {
                let count = length as usize + 1;
                let end = i
                    .checked_add(count)
                    .filter(|&e| e <= input.len())
                    .ok_or(FilterError::Corrupt { filter: RL })?;
                out.extend_from_slice(&input[i..end]);
                i = end;
            }
            129..=255 => {
                let count = 257 - length as usize;
                let &byte = input.get(i).ok_or(FilterError::Corrupt { filter: RL })?;
                i += 1;
                out.resize(out.len() + count, byte);
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literal_and_repeat_runs() {
        // Literal run of 3 ("abc"), then 0xFD (= repeat 257-253=4) of 'x', then EOD.
        let input = [2, b'a', b'b', b'c', 253, b'x', 128];
        assert_eq!(run_length_decode(&input).unwrap(), b"abcxxxx");
    }

    #[test]
    fn missing_eod_is_tolerated() {
        assert_eq!(run_length_decode(&[2, b'a', b'b', b'c']).unwrap(), b"abc");
    }

    #[test]
    fn truncated_literal_run_is_corrupt() {
        // Claims 4 literal bytes (length 3 -> 4) but only 2 follow.
        assert_eq!(
            run_length_decode(&[3, b'a', b'b']).unwrap_err(),
            FilterError::Corrupt { filter: RL }
        );
    }
}
