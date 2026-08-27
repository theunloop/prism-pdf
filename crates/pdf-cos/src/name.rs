//! PDF name objects (ISO 32000 §7.3.5).

use core::fmt;

use bytes::Bytes;

/// A PDF name such as `/Type`, stored as its **decoded** bytes.
///
/// `#xx` escapes are resolved by the reader before construction and re-encoded by the writer
/// (ADR-0003), so two names that differ only in escaping — `/Foo` and `/F#6Fo` — compare equal.
/// Equality is plain byte equality. Names are byte strings (usually, but not necessarily,
/// ASCII).
#[derive(Clone, PartialEq, Eq, Hash, Default)]
pub struct Name(Bytes);

impl Name {
    /// Creates a name from already-decoded bytes.
    pub fn new(bytes: impl Into<Bytes>) -> Self {
        Self(bytes.into())
    }

    /// Creates a name from a `&'static str` without allocating — ideal for literals, e.g.
    /// `Name::from_static("Type")`.
    #[must_use]
    pub fn from_static(s: &'static str) -> Self {
        Self(Bytes::from_static(s.as_bytes()))
    }

    /// The decoded name bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// The name as UTF-8, or `None` if it is not valid UTF-8.
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        core::str::from_utf8(&self.0).ok()
    }
}

impl From<&str> for Name {
    fn from(s: &str) -> Self {
        Self(Bytes::copy_from_slice(s.as_bytes()))
    }
}

impl From<String> for Name {
    fn from(s: String) -> Self {
        Self(Bytes::from(s.into_bytes()))
    }
}

impl From<Vec<u8>> for Name {
    fn from(v: Vec<u8>) -> Self {
        Self(Bytes::from(v))
    }
}

impl From<Bytes> for Name {
    fn from(b: Bytes) -> Self {
        Self(b)
    }
}

impl PartialEq<str> for Name {
    fn eq(&self, other: &str) -> bool {
        self.as_bytes() == other.as_bytes()
    }
}

impl PartialEq<&str> for Name {
    fn eq(&self, other: &&str) -> bool {
        self.as_bytes() == other.as_bytes()
    }
}

impl fmt::Debug for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.as_str() {
            Some(s) => write!(f, "Name({s:?})"),
            None => write!(f, "Name({:?})", self.as_bytes()),
        }
    }
}

impl fmt::Display for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Render in PDF source form (`/Name`), escaping non-graphic bytes and `#` as `#xx`.
        f.write_str("/")?;
        for &b in self.as_bytes() {
            if b.is_ascii_graphic() && b != b'#' {
                write!(f, "{}", b as char)?;
            } else {
                write!(f, "#{b:02X}")?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoded_names_are_byte_equal() {
        // §7.3.5: `/Foo` and `/F#6Fo` are the same name once decoded.
        assert_eq!(Name::from_static("Foo"), Name::from("Foo"));
        assert_eq!(Name::from_static("Foo"), "Foo");
    }

    #[test]
    fn display_re_encodes_specials() {
        assert_eq!(Name::from("A B").to_string(), "/A#20B");
        assert_eq!(Name::from("Type").to_string(), "/Type");
    }
}
