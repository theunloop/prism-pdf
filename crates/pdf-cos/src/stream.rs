//! PDF stream objects (ISO 32000 §7.3.8).

use core::fmt;

use bytes::Bytes;

use crate::Dictionary;

/// A PDF stream: a [`Dictionary`] plus its **raw, still-encoded** bytes, exactly as they appeared
/// between the `stream` and `endstream` keywords.
///
/// COS streams are **inert** (ADR-0004): there is no decode method here, because `pdf-cos` must
/// not depend on `pdf-filters` (the dependency runs the other way). Decoding is a `pdf-filters`
/// operation that takes a `&Stream` and returns bytes.
///
/// [`raw_len`](Self::raw_len) is the authority for the stream's byte length. The dict's `/Length`
/// is just another value — it may even be an indirect reference (`<< /Length 12 0 R >>`) that COS
/// cannot resolve — and the writer recomputes it on save, so it is never trusted for slicing.
#[derive(Clone, PartialEq, Default)]
pub struct Stream {
    dict: Dictionary,
    raw: Bytes,
}

impl Stream {
    /// Creates a stream from its dictionary and raw (encoded) bytes.
    pub fn new(dict: Dictionary, raw: impl Into<Bytes>) -> Self {
        Self {
            dict,
            raw: raw.into(),
        }
    }

    /// The stream dictionary.
    #[must_use]
    pub fn dict(&self) -> &Dictionary {
        &self.dict
    }

    /// The stream dictionary, mutably.
    pub fn dict_mut(&mut self) -> &mut Dictionary {
        &mut self.dict
    }

    /// The raw, still-encoded stream bytes.
    #[must_use]
    pub fn raw(&self) -> &Bytes {
        &self.raw
    }

    /// The actual number of raw bytes — the authority for stream length (ADR-0004), independent
    /// of whatever the dict's `/Length` says.
    #[must_use]
    pub fn raw_len(&self) -> usize {
        self.raw.len()
    }

    /// Consumes the stream, returning its raw bytes.
    #[must_use]
    pub fn into_raw(self) -> Bytes {
        self.raw
    }
}

impl fmt::Debug for Stream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Never dump raw bytes (they can be megabytes); show the dict and a length summary.
        f.debug_struct("Stream")
            .field("dict", &self.dict)
            .field("raw_len", &self.raw.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Name, Object};

    #[test]
    fn raw_len_is_the_authority_not_dict_length() {
        // §7.3.8 + ADR-0004: a wrong/indirect `/Length` does not affect the real byte count.
        let mut dict = Dictionary::new();
        dict.insert(Name::from_static("Length"), Object::Integer(999));
        let s = Stream::new(dict, &b"abc"[..]);
        assert_eq!(s.raw_len(), 3);
    }
}
