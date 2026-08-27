//! COS object serialization (the inverse of `pdf-reader`'s §7.3 parser): turn an [`Object`] back
//! into PDF syntax bytes.
//!
//! Output is canonical, not byte-faithful (ADR-0003): a string is written as an escaped literal,
//! a name with `#XX` escapes where required (§7.3.5), and a stream's `/Length` is rewritten to its
//! actual raw length (ADR-0004) so the result always parses back.

use pdf_cos::syntax::is_delimiter;
use pdf_cos::{Name, Object};

/// Append the PDF syntax for `object` to `out` (§7.3).
pub fn serialize_object(out: &mut Vec<u8>, object: &Object) {
    match object {
        Object::Null => out.extend_from_slice(b"null"),
        Object::Boolean(true) => out.extend_from_slice(b"true"),
        Object::Boolean(false) => out.extend_from_slice(b"false"),
        Object::Integer(n) => out.extend_from_slice(n.to_string().as_bytes()),
        Object::Real(r) => write_real(out, *r),
        Object::String(s) => write_literal_string(out, s.as_bytes()),
        Object::Name(name) => write_name(out, name.as_bytes()),
        Object::Array(array) => {
            out.push(b'[');
            for (i, item) in array.iter().enumerate() {
                if i > 0 {
                    out.push(b' ');
                }
                serialize_object(out, item);
            }
            out.push(b']');
        }
        Object::Dictionary(dict) => write_dict(out, dict),
        Object::Stream(stream) => {
            // §7.3.8 / ADR-0004: the written /Length must equal the raw byte count.
            let mut dict = stream.dict().clone();
            dict.insert(
                Name::from("Length"),
                Object::Integer(stream.raw_len() as i64),
            );
            write_dict(out, &dict);
            out.extend_from_slice(b"\nstream\n");
            out.extend_from_slice(stream.raw());
            out.extend_from_slice(b"\nendstream");
        }
        Object::Reference(id) => {
            out.extend_from_slice(format!("{} {} R", id.number, id.generation).as_bytes());
        }
    }
}

/// Write a dictionary `<< /Key value … >>` (§7.3.7).
fn write_dict(out: &mut Vec<u8>, dict: &pdf_cos::Dictionary) {
    out.extend_from_slice(b"<<");
    for (key, value) in dict.iter() {
        out.push(b' ');
        write_name(out, key.as_bytes());
        out.push(b' ');
        serialize_object(out, value);
    }
    out.extend_from_slice(b" >>");
}

/// Write a real number, keeping a decimal point so it re-parses as a real, not an integer
/// (§7.3.3 / ADR-0003).
fn write_real(out: &mut Vec<u8>, r: f64) {
    let text = format!("{r}");
    out.extend_from_slice(text.as_bytes());
    if !text.contains('.') {
        out.extend_from_slice(b".0");
    }
}

/// Write a literal string `(...)`, escaping the bytes that need it and emitting non-printable
/// bytes as octal so any byte string round-trips (§7.3.4.2).
fn write_literal_string(out: &mut Vec<u8>, bytes: &[u8]) {
    out.push(b'(');
    for &b in bytes {
        match b {
            b'(' => out.extend_from_slice(b"\\("),
            b')' => out.extend_from_slice(b"\\)"),
            b'\\' => out.extend_from_slice(b"\\\\"),
            0x20..=0x7E => out.push(b),
            _ => out.extend_from_slice(format!("\\{b:03o}").as_bytes()),
        }
    }
    out.push(b')');
}

/// Write a name `/Name`, `#XX`-escaping any byte that is not a printable non-delimiter (§7.3.5).
fn write_name(out: &mut Vec<u8>, bytes: &[u8]) {
    out.push(b'/');
    for &b in bytes {
        if (0x21..=0x7E).contains(&b) && !is_delimiter(b) && b != b'#' {
            out.push(b);
        } else {
            out.extend_from_slice(format!("#{b:02X}").as_bytes());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdf_cos::{Array, Dictionary, ObjectId, PdfString, Stream};

    fn ser(object: &Object) -> Vec<u8> {
        let mut out = Vec::new();
        serialize_object(&mut out, object);
        out
    }

    #[test]
    fn scalars() {
        assert_eq!(ser(&Object::Null), b"null");
        assert_eq!(ser(&Object::Boolean(true)), b"true");
        assert_eq!(ser(&Object::Integer(-42)), b"-42");
        assert_eq!(ser(&Object::Real(2.5)), b"2.5");
        // An integral real keeps its decimal point.
        assert_eq!(ser(&Object::Real(100.0)), b"100.0");
        assert_eq!(ser(&Object::Reference(ObjectId::new(7, 0))), b"7 0 R");
    }

    #[test]
    fn strings_escape_specials_and_binary() {
        assert_eq!(
            ser(&Object::String(PdfString::from(b"a(b)\\c".to_vec()))),
            b"(a\\(b\\)\\\\c)"
        );
        // Non-printable bytes become octal.
        assert_eq!(
            ser(&Object::String(PdfString::from(vec![0x00, 0x0A]))),
            b"(\\000\\012)"
        );
    }

    #[test]
    fn names_escape_where_required() {
        assert_eq!(ser(&Object::Name(Name::from("Type"))), b"/Type");
        assert_eq!(ser(&Object::Name(Name::from("A B"))), b"/A#20B");
        assert_eq!(ser(&Object::Name(Name::from("a#b"))), b"/a#23b");
    }

    #[test]
    fn array_and_dictionary() {
        let arr = Array::from_vec(vec![Object::Integer(1), Object::Name(Name::from("X"))]);
        assert_eq!(ser(&Object::Array(arr)), b"[1 /X]");

        let mut d = Dictionary::new();
        d.insert(Name::from("K"), Object::Integer(5));
        assert_eq!(ser(&Object::Dictionary(d)), b"<< /K 5 >>");
    }

    #[test]
    fn stream_rewrites_length() {
        let mut dict = Dictionary::new();
        dict.insert(Name::from("Length"), Object::Integer(999)); // a lie, must be corrected
        let stream = Stream::new(dict, b"hello".to_vec());
        let bytes = ser(&Object::Stream(stream));
        assert!(bytes.windows(11).any(|w| w == b"/Length 5 >"));
        assert!(bytes.windows(7).any(|w| w == b"stream\n"));
        assert!(bytes.ends_with(b"\nendstream"));
    }
}
