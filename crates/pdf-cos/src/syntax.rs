//! Lexical conventions (ISO 32000-1 §7.2) — the character classes every PDF tokenizer shares.
//!
//! These are normative tables from the spec, not helpers: §7.2.2 partitions every byte into
//! *white space*, *delimiter*, or *regular*. §7.3.4 also defines the shared string encodings: the
//! literal-string escape rules and ASCII-hex digit values. They live here, in the crate everything
//! else depends on, so there is exactly one place to be right.
//!
//! Four consumers rely on them, and they must agree byte-for-byte or the same file tokenizes two
//! different ways: `pdf-reader` (file syntax, §7.2), `pdf-content` (content-stream syntax, §8.2 —
//! the same lexical rules), `pdf-filters` (the ASCII transport filters §7.4.2/§7.4.3 skip white
//! space anywhere in their input), and `pdf-writer` (a name must be `#`-escaped exactly where a
//! reader would otherwise stop, §7.3.5).

/// White-space characters (§7.2.2, Table 1): NUL, HT, LF, FF, CR, SP.
#[must_use]
pub fn is_whitespace(b: u8) -> bool {
    matches!(b, 0x00 | 0x09 | 0x0A | 0x0C | 0x0D | 0x20)
}

/// Delimiter characters (§7.2.2, Table 2): `( ) < > [ ] { } / %`.
#[must_use]
pub fn is_delimiter(b: u8) -> bool {
    matches!(
        b,
        b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
    )
}

/// Regular characters (§7.2.2): anything that is neither white space nor a delimiter. These form
/// the bodies of names, numbers and keywords.
#[must_use]
pub fn is_regular(b: u8) -> bool {
    !is_whitespace(b) && !is_delimiter(b)
}

/// The value of an ASCII hex digit, or `None` if `b` is not one (§7.3.4.3).
#[must_use]
pub fn hex_value(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Append a PDF literal-string body with the escapes defined by §7.3.4.2.
///
/// Parentheses and backslashes are protected, LF/CR/HT use their named escapes, printable ASCII
/// is kept readable, and every other byte is represented by an unambiguous three-digit octal
/// escape. The surrounding `(` and `)` delimiters are the caller's responsibility.
pub fn escape_literal_string(bytes: &[u8], out: &mut Vec<u8>) {
    for &b in bytes {
        match b {
            b'\\' | b'(' | b')' => {
                out.push(b'\\');
                out.push(b);
            }
            b'\n' => out.extend_from_slice(b"\\n"),
            b'\r' => out.extend_from_slice(b"\\r"),
            b'\t' => out.extend_from_slice(b"\\t"),
            0x20..=0x7e => out.push(b),
            _ => {
                out.push(b'\\');
                out.push(b'0' + ((b >> 6) & 0x07));
                out.push(b'0' + ((b >> 3) & 0x07));
                out.push(b'0' + (b & 0x07));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_three_classes_partition_every_byte() {
        // §7.2.2: every one of the 256 bytes is white space, a delimiter, or regular — exactly one.
        for b in 0..=u8::MAX {
            let classes =
                u8::from(is_whitespace(b)) + u8::from(is_delimiter(b)) + u8::from(is_regular(b));
            assert_eq!(classes, 1, "byte {b:#04x} landed in {classes} classes");
        }
    }

    #[test]
    fn white_space_is_exactly_table_1() {
        let ws: Vec<u8> = (0..=u8::MAX).filter(|&b| is_whitespace(b)).collect();
        assert_eq!(ws, vec![0x00, 0x09, 0x0A, 0x0C, 0x0D, 0x20]);
    }

    #[test]
    fn delimiters_are_exactly_table_2() {
        let d: Vec<u8> = (0..=u8::MAX).filter(|&b| is_delimiter(b)).collect();
        assert_eq!(d, b"%()/<>[]{}".to_vec());
    }

    #[test]
    fn hex_digits_decode_in_both_cases_and_nothing_else() {
        assert_eq!(hex_value(b'0'), Some(0));
        assert_eq!(hex_value(b'9'), Some(9));
        assert_eq!(hex_value(b'a'), Some(10));
        assert_eq!(hex_value(b'f'), Some(15));
        assert_eq!(hex_value(b'A'), Some(10));
        assert_eq!(hex_value(b'F'), Some(15));
        // The neighbours of each range, and a non-ASCII byte.
        for b in [b'/', b':', b'`', b'g', b'@', b'G', 0x80, 0xFF] {
            assert_eq!(hex_value(b), None, "byte {b:#04x} is not a hex digit");
        }
        assert_eq!(
            (0..=u8::MAX).filter(|&b| hex_value(b).is_some()).count(),
            22
        );
    }

    #[test]
    fn literal_strings_escape_delimiters_controls_and_arbitrary_bytes() {
        // §7.3.4.2: delimiters cannot alter nesting, named controls remain readable, and octal
        // escapes make every remaining byte representable without embedding raw control data.
        let mut escaped = Vec::new();
        escape_literal_string(b"a(b)\\c\n\r\t\x08\x0c\x00\x7f\xff", &mut escaped);
        assert_eq!(escaped, b"a\\(b\\)\\\\c\\n\\r\\t\\010\\014\\000\\177\\377");
    }

    #[test]
    fn literal_string_escape_covers_every_byte_without_raw_controls() {
        let mut escaped = Vec::new();
        escape_literal_string(&(0..=u8::MAX).collect::<Vec<_>>(), &mut escaped);

        assert!(!escaped.contains(&0));
        assert!(!escaped.contains(&0x7f));
        assert!(
            escaped
                .iter()
                .all(|&b| b == b'\n' || (0x20..=0x7e).contains(&b))
        );
    }
}
