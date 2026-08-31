#![forbid(unsafe_code)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]
//! pdf-filters — stream filters & codecs (EPIC 3, ISO 32000 §7.4).
//!
//! Decodes the (still-encoded) raw bytes of a [`pdf_cos::Stream`] (ADR-0004) into plain bytes by
//! running its `/Filter` chain. Input is untrusted (DESIGN.md §3.4): every decoder is fallible,
//! never panics, and output is bounded to defeat decompression bombs.
//!
//! Implemented:
//! - **§7.4.2 ASCIIHexDecode**, **§7.4.3 ASCII85Decode**, **§7.4.5 RunLengthDecode**
//! - **§7.4.4 FlateDecode** and **§7.4.4.2 LZWDecode**, incl. the **§7.4.4.4** PNG/TIFF predictors.
//! - **§7.4.6 CCITTFaxDecode** (ITU-T T.4 Group 3 1D/2D and T.6 Group 4).
//! - **§7.4.7 JBIG2Decode** (bi-level images, via the pure-Rust `hayro-jbig2`; see [`jbig2_decode`]).
//! - **§7.4.8 DCTDecode** (JPEG).
//!
//! Partial: **§7.4.9 JPXDecode** parses the JPEG 2000 main header for metadata ([`jpx_info`]) but
//! does not decode pixels — [`decode_stream`] still reports it as [`FilterError::Unsupported`] and
//! the image layer passes the codestream through.
//!
//! The **§7.4.10 Crypt** filter is the identity transform here: stream decryption is done by the
//! reader before the filter chain runs (see `pdf-reader`), so this layer only needs to skip it.
//!
//! Within [`decode_stream`] a `JBIG2Decode` stage decodes with **no** `/JBIG2Globals` (that
//! parameter is an indirect stream the `filters → cos` layer cannot resolve); a caller holding the
//! globals bytes uses [`jbig2_decode`] directly. `JPXDecode` pixel decoding stays
//! [`FilterError::Unsupported`].

mod ascii;
mod ccitt;
mod dct;
mod error;
mod flate;
mod jbig2;
mod jpx;
mod lzw;
mod run_length;
mod trace;

pub use ascii::{ascii_hex_decode, ascii85_decode};
pub use ccitt::ccitt_fax_decode;
pub use dct::dct_decode;
pub use error::{FilterError, Result};
pub use flate::{flate_decode, flate_encode};
pub use jbig2::jbig2_decode;
pub use jpx::{JpxInfo, jpx_info};
pub use lzw::lzw_decode;
pub use run_length::run_length_decode;

use pdf_cos::{Dictionary, Name, Object, Stream};

/// Default ceiling on a single filter's decoded output (256 MiB), used by [`decode_stream`].
pub const DEFAULT_MAX_DECODED: usize = 256 * 1024 * 1024;

/// Default ceiling on the number of stages in one stream's `/Filter` chain (§7.4), used by
/// [`decode_stream`].
///
/// Real files use one filter, occasionally two (`[/ASCII85Decode /FlateDecode]`); eight is far
/// beyond anything legitimate. The bound is load-bearing rather than cosmetic: the per-stage output
/// ceiling does **not** bound the work of a chain, because every stage re-processes the whole of the
/// previous stage's output. With the count unbounded, a stream body of *b* bytes and *n* named
/// filters costs `b × n` — two factors an attacker scales independently and cheaply, since each
/// extra stage costs about seven bytes of file. `Crypt` is the worst of them: it is the identity
/// transform at this layer (decryption already happened in the reader), so it copies the body
/// without ever shrinking it. Measured before this bound: a 21 MB file spent 174 s of CPU in
/// `Document::open` + `page_text`, scaling linearly with no ceiling.
pub const DEFAULT_MAX_FILTER_CHAIN: usize = 8;

/// A stream filter (§7.4). Both the full name and the inline-image abbreviation (§7.4, Table 6)
/// are recognised by [`Filter::from_name`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Filter {
    /// `ASCIIHexDecode` / `AHx` (§7.4.2).
    AsciiHex,
    /// `ASCII85Decode` / `A85` (§7.4.3).
    Ascii85,
    /// `LZWDecode` / `LZW` (§7.4.4.2).
    Lzw,
    /// `FlateDecode` / `Fl` (§7.4.4).
    Flate,
    /// `RunLengthDecode` / `RL` (§7.4.5).
    RunLength,
    /// `CCITTFaxDecode` / `CCF` (§7.4.6).
    CcittFax,
    /// `DCTDecode` / `DCT` (§7.4.8).
    Dct,
    /// `JBIG2Decode` (§7.4.7) — not yet implemented.
    Jbig2,
    /// `JPXDecode` (§7.4.9) — header parsed ([`jpx_info`]); pixel decoding not implemented.
    Jpx,
    /// `Crypt` (§7.4.10) — identity at the filter layer; decryption is the reader's job.
    Crypt,
}

impl Filter {
    /// Recognise a filter from its `/Filter` name (full or abbreviated), or `None` if unknown.
    #[must_use]
    pub fn from_name(name: &Name) -> Option<Self> {
        Some(match name.as_bytes() {
            b"ASCIIHexDecode" | b"AHx" => Filter::AsciiHex,
            b"ASCII85Decode" | b"A85" => Filter::Ascii85,
            b"LZWDecode" | b"LZW" => Filter::Lzw,
            b"FlateDecode" | b"Fl" => Filter::Flate,
            b"RunLengthDecode" | b"RL" => Filter::RunLength,
            b"CCITTFaxDecode" | b"CCF" => Filter::CcittFax,
            b"DCTDecode" | b"DCT" => Filter::Dct,
            b"JBIG2Decode" => Filter::Jbig2,
            b"JPXDecode" => Filter::Jpx,
            b"Crypt" => Filter::Crypt,
            _ => return None,
        })
    }

    /// The canonical (full) filter name, for diagnostics.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Filter::AsciiHex => "ASCIIHexDecode",
            Filter::Ascii85 => "ASCII85Decode",
            Filter::Lzw => "LZWDecode",
            Filter::Flate => "FlateDecode",
            Filter::RunLength => "RunLengthDecode",
            Filter::CcittFax => "CCITTFaxDecode",
            Filter::Dct => "DCTDecode",
            Filter::Jbig2 => "JBIG2Decode",
            Filter::Jpx => "JPXDecode",
            Filter::Crypt => "Crypt",
        }
    }

    /// Apply this filter to `input`, given optional `/DecodeParms` and an output ceiling.
    fn apply(self, input: &[u8], params: Option<&Dictionary>, max: usize) -> Result<Vec<u8>> {
        let out = match self {
            Filter::Flate => return flate_decode(input, params, max),
            Filter::Lzw => return lzw_decode(input, params, max),
            Filter::CcittFax => return ccitt_fax_decode(input, params, max),
            Filter::Dct => return dct_decode(input, max),
            // §7.4.7: decode without globals — the chain layer can't resolve the indirect
            // /JBIG2Globals stream (see module docs); callers with a resolver use jbig2_decode.
            Filter::Jbig2 => return jbig2_decode(input, None, max),
            // §7.4.10: the Crypt filter selects how the security handler decrypts the stream, which
            // happens in the reader *before* the filter chain runs. By the time bytes reach here
            // they are already plaintext (or were never encrypted, e.g. /Name /Identity), so at this
            // layer Crypt is the identity transform — pass through untouched.
            Filter::Crypt => return Ok(input.to_vec()),
            Filter::AsciiHex => ascii_hex_decode(input)?,
            Filter::Ascii85 => ascii85_decode(input)?,
            Filter::RunLength => run_length_decode(input)?,
            other => return Err(FilterError::Unsupported(other.name())),
        };
        // Bound the expanding filters too (RunLength can grow ~64×): defence in depth.
        if out.len() > max {
            return Err(FilterError::TooLarge { limit: max });
        }
        Ok(out)
    }
}

/// Decode a stream's raw bytes by running its full `/Filter` chain (§7.4), with the default
/// [`DEFAULT_MAX_DECODED`] output ceiling per stage and the default [`DEFAULT_MAX_FILTER_CHAIN`]
/// stage-count ceiling.
pub fn decode_stream(stream: &Stream) -> Result<Vec<u8>> {
    decode_stream_with_limits(stream, DEFAULT_MAX_DECODED, DEFAULT_MAX_FILTER_CHAIN)
}

/// As [`decode_stream`] with an explicit per-stage output ceiling (anti-DoS, DESIGN.md §3.4), and
/// the default [`DEFAULT_MAX_FILTER_CHAIN`] stage-count ceiling.
pub fn decode_stream_with_limit(stream: &Stream, max: usize) -> Result<Vec<u8>> {
    decode_stream_with_limits(stream, max, DEFAULT_MAX_FILTER_CHAIN)
}

/// As [`decode_stream`] with both anti-DoS ceilings given explicitly: `max` bounds what any single
/// stage may produce, and `max_chain` bounds how many stages the `/Filter` chain may name.
///
/// Both are needed. `max` alone is a decompression-bomb guard on one decoder; it says nothing about
/// a chain, because each stage runs over the whole of the previous stage's output — so total work
/// is the product of the two (see [`DEFAULT_MAX_FILTER_CHAIN`]).
pub fn decode_stream_with_limits(stream: &Stream, max: usize, max_chain: usize) -> Result<Vec<u8>> {
    let dict = stream.dict();
    let chain = filter_chain(dict, max_chain)?;
    let mut data = stream.raw().to_vec();
    for (filter, params) in chain {
        data = filter.apply(&data, params.as_ref(), max)?;
    }
    Ok(data)
}

/// Build the ordered list of `(filter, params)` from a stream dictionary's `/Filter` and
/// `/DecodeParms` (§7.4.2), refusing a chain longer than `max_chain`. A `/Filter` that is an
/// indirect reference cannot be resolved here and is rejected.
///
/// The length is checked against the raw array *before* any filter is looked up or any
/// `/DecodeParms` dictionary is cloned, so an over-long chain costs nothing to reject.
fn filter_chain(dict: &Dictionary, max_chain: usize) -> Result<Vec<(Filter, Option<Dictionary>)>> {
    let filters: Vec<Filter> = match dict.get(&Name::from("Filter")) {
        None | Some(Object::Null) => Vec::new(),
        Some(Object::Name(n)) => vec![lookup(n)?],
        Some(Object::Array(arr)) => {
            if arr.len() > max_chain {
                return Err(FilterError::ChainTooLong { limit: max_chain });
            }
            arr.iter()
                .map(|obj| match obj {
                    Object::Name(n) => lookup(n),
                    _ => Err(FilterError::Unsupported("non-name /Filter entry")),
                })
                .collect::<Result<_>>()?
        }
        Some(_) => return Err(FilterError::Unsupported("indirect or malformed /Filter")),
    };

    let mut params: Vec<Option<Dictionary>> = vec![None; filters.len()];
    match dict.get(&Name::from("DecodeParms")) {
        None | Some(Object::Null) => {}
        // A lone dictionary applies to the first (usually only) filter.
        Some(Object::Dictionary(d)) => {
            if let Some(slot) = params.first_mut() {
                *slot = Some(d.clone());
            }
        }
        Some(Object::Array(arr)) => {
            for (slot, obj) in params.iter_mut().zip(arr.iter()) {
                if let Object::Dictionary(d) = obj {
                    *slot = Some(d.clone());
                }
            }
        }
        Some(_) => {}
    }

    Ok(filters.into_iter().zip(params).collect())
}

/// Resolve a `/Filter` name to a [`Filter`], erroring if the name is unknown.
fn lookup(name: &Name) -> Result<Filter> {
    Filter::from_name(name).ok_or(FilterError::Unsupported("unknown /Filter"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{Compression, write::ZlibEncoder};
    use pdf_cos::Array;
    use std::io::Write;

    fn zlib(data: &[u8]) -> Vec<u8> {
        let mut e = ZlibEncoder::new(Vec::new(), Compression::default());
        e.write_all(data).unwrap();
        e.finish().unwrap()
    }

    #[test]
    fn from_name_full_and_abbreviated() {
        assert_eq!(
            Filter::from_name(&Name::from("FlateDecode")),
            Some(Filter::Flate)
        );
        assert_eq!(Filter::from_name(&Name::from("Fl")), Some(Filter::Flate));
        assert_eq!(Filter::from_name(&Name::from("Nope")), None);
    }

    #[test]
    fn single_flate_filter_stream() {
        let mut dict = Dictionary::new();
        dict.insert(
            Name::from("Filter"),
            Object::Name(Name::from("FlateDecode")),
        );
        let stream = Stream::new(dict, zlib(b"hello world"));
        assert_eq!(decode_stream(&stream).unwrap(), b"hello world");
    }

    #[test]
    fn no_filter_returns_raw() {
        let stream = Stream::new(Dictionary::new(), b"raw bytes".to_vec());
        assert_eq!(decode_stream(&stream).unwrap(), b"raw bytes");
    }

    #[test]
    fn filter_chain_ascii85_then_flate() {
        // §7.4.2: filters apply in array order. Encode as Flate, then wrap that in ASCII85, so
        // decoding runs ASCII85 first, then Flate.
        let flated = zlib(b"chained filters");
        let ascii85 = encode_ascii85(&flated);

        let mut dict = Dictionary::new();
        dict.insert(
            Name::from("Filter"),
            Object::Array(
                vec![
                    Object::Name(Name::from("ASCII85Decode")),
                    Object::Name(Name::from("FlateDecode")),
                ]
                .into(),
            ),
        );
        let stream = Stream::new(dict, ascii85);
        assert_eq!(decode_stream(&stream).unwrap(), b"chained filters");
    }

    #[test]
    fn a_long_filter_chain_is_refused() {
        // Every stage re-processes the whole of the previous stage's output, so an uncapped chain
        // multiplies the work a small file can demand. `Crypt` is the cheapest amplifier: at this
        // layer it is the identity transform, so it copies the body without shrinking it.
        let body = b"amplify me".to_vec();
        let chain = |n: usize| {
            let mut arr = Array::new();
            for _ in 0..n {
                arr.push(Object::Name(Name::from("Crypt")));
            }
            let mut dict = Dictionary::new();
            dict.insert(Name::from("Filter"), Object::Array(arr));
            Stream::new(dict, body.clone())
        };

        // At the limit the chain still runs (Crypt is the identity, so the body survives it).
        let at_limit = chain(DEFAULT_MAX_FILTER_CHAIN);
        assert_eq!(decode_stream(&at_limit).unwrap(), body);

        // One past it is refused, and refused by *count* — not by running the stages first.
        assert_eq!(
            decode_stream(&chain(DEFAULT_MAX_FILTER_CHAIN + 1)),
            Err(FilterError::ChainTooLong {
                limit: DEFAULT_MAX_FILTER_CHAIN
            })
        );
        assert_eq!(
            decode_stream(&chain(500_000)),
            Err(FilterError::ChainTooLong {
                limit: DEFAULT_MAX_FILTER_CHAIN
            })
        );

        // The ceiling is configurable in both directions.
        assert!(decode_stream_with_limits(&chain(3), DEFAULT_MAX_DECODED, 2).is_err());
        assert!(decode_stream_with_limits(&chain(50), DEFAULT_MAX_DECODED, 64).is_ok());
    }

    #[test]
    fn unsupported_filter_is_reported() {
        // JPXDecode pixel decoding is not implemented (JBIG2Decode now is, §7.4.7).
        let mut dict = Dictionary::new();
        dict.insert(Name::from("Filter"), Object::Name(Name::from("JPXDecode")));
        let stream = Stream::new(dict, b"anything".to_vec());
        assert_eq!(
            decode_stream(&stream).unwrap_err(),
            FilterError::Unsupported("JPXDecode")
        );
    }

    #[test]
    fn jbig2_filter_decodes_through_the_chain() {
        // The §7.4.7 spec example, ASCII-hex wrapped then JBIG2-decoded — but the image needs its
        // globals, which the chain can't resolve, so this self-contained case errors cleanly
        // (Corrupt), never panics. A real globals-bearing decode is covered in `jbig2.rs` tests.
        let mut dict = Dictionary::new();
        dict.insert(
            Name::from("Filter"),
            Object::Name(Name::from("JBIG2Decode")),
        );
        let stream = Stream::new(dict, b"\x00\x00\x00\x01not-real-jbig2".to_vec());
        assert!(matches!(
            decode_stream(&stream).unwrap_err(),
            FilterError::Corrupt {
                filter: "JBIG2Decode"
            }
        ));
    }

    #[test]
    fn crypt_filter_is_identity() {
        // §7.4.10: at the filter layer Crypt is a no-op (the reader already decrypted the bytes).
        let mut dict = Dictionary::new();
        dict.insert(Name::from("Filter"), Object::Name(Name::from("Crypt")));
        let stream = Stream::new(dict, b"already plaintext".to_vec());
        assert_eq!(decode_stream(&stream).unwrap(), b"already plaintext");
    }

    #[test]
    fn crypt_then_flate_chain_decodes() {
        // A real encrypted stream is often /Filter [/Crypt /FlateDecode]: Crypt passes through,
        // then Flate inflates. Previously this errored Unsupported and broke the whole stream.
        let flated = zlib(b"decrypted then deflated");
        let mut dict = Dictionary::new();
        dict.insert(
            Name::from("Filter"),
            Object::Array(
                vec![
                    Object::Name(Name::from("Crypt")),
                    Object::Name(Name::from("FlateDecode")),
                ]
                .into(),
            ),
        );
        let stream = Stream::new(dict, flated);
        assert_eq!(decode_stream(&stream).unwrap(), b"decrypted then deflated");
    }

    #[test]
    fn lzw_through_decode_stream() {
        // The canonical "-----A---B" LZW example (TIFF 6.0 §13): 80 0B 60 50 22 0C 0C 85 01.
        let encoded = [0x80, 0x0B, 0x60, 0x50, 0x22, 0x0C, 0x0C, 0x85, 0x01];
        let mut d = Dictionary::new();
        d.insert(Name::from("Filter"), Object::Name(Name::from("LZWDecode")));
        let stream = Stream::new(d, encoded.to_vec());
        assert_eq!(decode_stream(&stream).unwrap(), b"-----A---B");
    }

    /// Minimal ASCII85 encoder for the chain test (no `z` shortcut, adds the `~>` terminator).
    fn encode_ascii85(data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        for chunk in data.chunks(4) {
            let mut word = [0u8; 4];
            word[..chunk.len()].copy_from_slice(chunk);
            let value = u32::from_be_bytes(word);
            let mut group = [0u8; 5];
            let mut v = value;
            for slot in group.iter_mut().rev() {
                *slot = b'!' + (v % 85) as u8;
                v /= 85;
            }
            out.extend_from_slice(&group[..chunk.len() + 1]);
        }
        out.extend_from_slice(b"~>");
        out
    }

    #[test]
    fn filter_names_round_trip() {
        for (text, filter) in [
            ("ASCIIHexDecode", Filter::AsciiHex),
            ("ASCII85Decode", Filter::Ascii85),
            ("LZWDecode", Filter::Lzw),
            ("FlateDecode", Filter::Flate),
            ("RunLengthDecode", Filter::RunLength),
            ("CCITTFaxDecode", Filter::CcittFax),
            ("DCTDecode", Filter::Dct),
            ("JBIG2Decode", Filter::Jbig2),
            ("JPXDecode", Filter::Jpx),
            ("Crypt", Filter::Crypt),
        ] {
            assert_eq!(Filter::from_name(&Name::from(text)), Some(filter));
            assert_eq!(filter.name(), text);
        }
    }

    #[test]
    fn run_length_and_ascii_hex_through_decode_stream() {
        // RunLength: literal run "ab" then EOD.
        let mut d = Dictionary::new();
        d.insert(
            Name::from("Filter"),
            Object::Name(Name::from("RunLengthDecode")),
        );
        let stream = Stream::new(d, vec![1u8, b'a', b'b', 128]);
        assert_eq!(decode_stream(&stream).unwrap(), b"ab");

        let mut d = Dictionary::new();
        d.insert(
            Name::from("Filter"),
            Object::Name(Name::from("ASCIIHexDecode")),
        );
        let stream = Stream::new(d, b"4869>".to_vec());
        assert_eq!(decode_stream(&stream).unwrap(), b"Hi");
    }

    #[test]
    fn decode_parms_dictionary_and_array_forms() {
        // A FlateDecode with a Predictor in a lone /DecodeParms dictionary.
        let mut params = Dictionary::new();
        params.insert(Name::from("Predictor"), Object::Integer(12));
        params.insert(Name::from("Columns"), Object::Integer(3));
        let mut d = Dictionary::new();
        d.insert(
            Name::from("Filter"),
            Object::Name(Name::from("FlateDecode")),
        );
        d.insert(
            Name::from("DecodeParms"),
            Object::Dictionary(params.clone()),
        );
        let stream = Stream::new(d, zlib(&[0, 10, 20, 30, 2, 1, 2, 3]));
        assert_eq!(
            decode_stream(&stream).unwrap(),
            vec![10, 20, 30, 11, 22, 33]
        );

        // Array /DecodeParms aligned with an array /Filter (single Flate here).
        let mut d = Dictionary::new();
        d.insert(
            Name::from("Filter"),
            Object::Array(vec![Object::Name(Name::from("FlateDecode"))].into()),
        );
        d.insert(
            Name::from("DecodeParms"),
            Object::Array(vec![Object::Dictionary(params)].into()),
        );
        let stream = Stream::new(d, zlib(&[0, 10, 20, 30, 2, 1, 2, 3]));
        assert_eq!(
            decode_stream(&stream).unwrap(),
            vec![10, 20, 30, 11, 22, 33]
        );
    }

    #[test]
    fn malformed_filter_specs_are_rejected() {
        // Indirect (unresolved) /Filter.
        let mut d = Dictionary::new();
        d.insert(
            Name::from("Filter"),
            Object::Reference(pdf_cos::ObjectId::new(9, 0)),
        );
        assert!(matches!(
            decode_stream(&Stream::new(d, b"x".to_vec())).unwrap_err(),
            FilterError::Unsupported(_)
        ));

        // Non-name entry inside a /Filter array.
        let mut d = Dictionary::new();
        d.insert(
            Name::from("Filter"),
            Object::Array(vec![Object::Integer(1)].into()),
        );
        assert!(matches!(
            decode_stream(&Stream::new(d, b"x".to_vec())).unwrap_err(),
            FilterError::Unsupported(_)
        ));

        // Unknown filter name.
        let mut d = Dictionary::new();
        d.insert(Name::from("Filter"), Object::Name(Name::from("Bogus")));
        assert!(matches!(
            decode_stream(&Stream::new(d, b"x".to_vec())).unwrap_err(),
            FilterError::Unsupported(_)
        ));
    }

    #[test]
    fn expanding_filter_is_bounded() {
        // RunLength can expand ~64×; a tiny limit must reject it.
        let mut d = Dictionary::new();
        d.insert(
            Name::from("Filter"),
            Object::Name(Name::from("RunLengthDecode")),
        );
        // 0xFF repeats the next byte 2 times... use a repeat run that exceeds the limit.
        let stream = Stream::new(d, vec![129u8, b'z']); // repeat 'z' 128 times
        assert_eq!(
            decode_stream_with_limit(&stream, 10).unwrap_err(),
            FilterError::TooLarge { limit: 10 }
        );
    }
}
