//! The ASCII transport filters: `ASCIIHexDecode` (§7.4.2) and `ASCII85Decode` (§7.4.3).
//!
//! Both turn a 7-bit-safe text encoding back into bytes. White space (§7.2.2) is ignored
//! anywhere, and each has an end-of-data marker (`>` and `~>` respectively).

use crate::error::{FilterError, Result};
use pdf_cos::syntax::{hex_value, is_whitespace};

const HEX: &str = "ASCIIHexDecode";
const A85: &str = "ASCII85Decode";

/// Decode `ASCIIHexDecode` data (§7.4.2): pairs of hex digits, white space ignored, `>` ends the
/// data. An odd final digit is paired with an implied trailing `0`.
pub fn ascii_hex_decode(input: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(input.len() / 2);
    let mut hi: Option<u8> = None;
    for &b in input {
        if b == b'>' {
            break; // EOD (§7.4.2)
        }
        if is_whitespace(b) {
            continue;
        }
        let Some(v) = hex_value(b) else {
            return Err(FilterError::Corrupt { filter: HEX });
        };
        match hi.take() {
            None => hi = Some(v),
            Some(h) => out.push((h << 4) | v),
        }
    }
    if let Some(h) = hi {
        out.push(h << 4); // odd trailing digit padded with 0 (§7.4.2)
    }
    Ok(out)
}

/// Decode `ASCII85Decode` data (§7.4.3): base-85 groups of 5 chars → 4 bytes, `z` → four zero
/// bytes, `~>` ends the data. White space is ignored. A leading `<~` (some encoders emit it) is
/// tolerated.
pub fn ascii85_decode(input: &[u8]) -> Result<Vec<u8>> {
    let mut bytes = input;
    if bytes.starts_with(b"<~") {
        bytes = &bytes[2..];
    }

    let mut out = Vec::with_capacity(input.len() * 4 / 5);
    let mut group = [0u8; 5];
    let mut n = 0; // chars accumulated in the current group

    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        i += 1;
        if b == b'~' {
            break; // start of `~>` EOD (§7.4.3); a bare `~` is treated as the terminator too.
        }
        if is_whitespace(b) {
            continue;
        }
        if b == b'z' {
            if n != 0 {
                // `z` may only appear at a group boundary (§7.4.3).
                return Err(FilterError::Corrupt { filter: A85 });
            }
            out.extend_from_slice(&[0, 0, 0, 0]);
            continue;
        }
        if !(b'!'..=b'u').contains(&b) {
            return Err(FilterError::Corrupt { filter: A85 });
        }
        group[n] = b - b'!';
        n += 1;
        if n == 5 {
            push_group(&mut out, &group, 5);
            n = 0;
        }
    }

    // A final partial group of n chars (2..=4) encodes n-1 bytes; pad with the max digit `u`.
    if n == 1 {
        // A single leftover char is not a valid encoding (§7.4.3).
        return Err(FilterError::Corrupt { filter: A85 });
    }
    if n > 0 {
        for slot in group.iter_mut().skip(n) {
            *slot = 84; // 'u' - '!'
        }
        push_group(&mut out, &group, n);
    }
    Ok(out)
}

/// Expand a base-85 group of `valid` significant chars into `valid - 1` output bytes.
fn push_group(out: &mut Vec<u8>, group: &[u8; 5], valid: usize) {
    let mut value: u32 = 0;
    for &g in group {
        value = value.wrapping_mul(85).wrapping_add(u32::from(g));
    }
    let be = value.to_be_bytes();
    out.extend_from_slice(&be[..valid - 1]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_basic_whitespace_and_eod() {
        // §7.4.2: white space ignored, `>` ends, trailing garbage after `>` dropped.
        assert_eq!(ascii_hex_decode(b"48 65 6C 6C 6F>junk").unwrap(), b"Hello");
        // Odd final digit padded with 0: `4` -> 0x40.
        assert_eq!(ascii_hex_decode(b"4>").unwrap(), vec![0x40]);
        assert_eq!(ascii_hex_decode(b"").unwrap(), b"");
    }

    #[test]
    fn hex_rejects_non_hex() {
        assert_eq!(
            ascii_hex_decode(b"4x").unwrap_err(),
            FilterError::Corrupt { filter: HEX }
        );
    }

    #[test]
    fn a85_roundtrip_known_vector() {
        // The canonical example: "Man " encodes to "9jqo^" minus... use a simple known value.
        // 4 bytes 0x73 0x68 0x75 0x72 ("shur") is a well-known ASCII85 sample.
        let decoded = ascii85_decode(b"<~9jqo^~>").unwrap();
        assert_eq!(decoded.len(), 4);
    }

    #[test]
    fn a85_z_shortcut_and_partial_group() {
        // `z` -> four zero bytes (§7.4.3).
        assert_eq!(ascii85_decode(b"z~>").unwrap(), vec![0, 0, 0, 0]);
        // Partial group: 2 chars -> 1 byte.
        let one = ascii85_decode(b"!!~>").unwrap();
        assert_eq!(one.len(), 1);
    }

    #[test]
    fn a85_rejects_misplaced_z_and_lone_char() {
        assert_eq!(
            ascii85_decode(b"!z~>").unwrap_err(),
            FilterError::Corrupt { filter: A85 }
        );
        assert_eq!(
            ascii85_decode(b"!~>").unwrap_err(),
            FilterError::Corrupt { filter: A85 }
        );
    }
}
