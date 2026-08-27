//! PDF string objects (ISO 32000 §7.3.4).

use core::fmt;

use bytes::Bytes;

/// A PDF string: an arbitrary **byte** string, not text.
///
/// The literal-`()` (§7.3.4.2) vs hex-`<>` (§7.3.4.3) source encoding is **not** retained — both
/// decode to the same bytes (ADR-0003). Interpreting the bytes as text (PDFDocEncoding or
/// UTF-16BE, §7.9.2) is a higher-layer concern and deliberately not done here.
#[derive(Clone, PartialEq, Eq, Hash, Default)]
pub struct PdfString(Bytes);

impl PdfString {
    /// Creates a string from raw bytes.
    pub fn new(bytes: impl Into<Bytes>) -> Self {
        Self(bytes.into())
    }

    /// The raw string bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Consumes the string, returning its bytes.
    #[must_use]
    pub fn into_bytes(self) -> Bytes {
        self.0
    }

    /// The number of bytes in the string.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether the string is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl From<&[u8]> for PdfString {
    fn from(b: &[u8]) -> Self {
        Self(Bytes::copy_from_slice(b))
    }
}

impl From<Vec<u8>> for PdfString {
    fn from(v: Vec<u8>) -> Self {
        Self(Bytes::from(v))
    }
}

impl From<Bytes> for PdfString {
    fn from(b: Bytes) -> Self {
        Self(b)
    }
}

impl fmt::Debug for PdfString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match core::str::from_utf8(&self.0) {
            Ok(s) => write!(f, "PdfString({s:?})"),
            Err(_) => write!(f, "PdfString({:?})", self.as_bytes()),
        }
    }
}
