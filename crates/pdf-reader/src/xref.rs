//! File structure: header, cross-reference table, and trailer (ISO 32000-1 §7.5).
//!
//! This is the map that turns a PDF file into a navigable object store. [`XRef::parse`] reads the
//! header version (§7.5.2), follows `startxref` (§7.5.5) to the cross-reference table (§7.5.4),
//! parses the trailer dictionary (§7.5.5), and walks the `/Prev` chain back through any
//! incremental updates (§7.5.6), merging the sections so the most recent definition of each
//! object wins. [`XRef::fetch`] then locates and parses any indirect object by number.
//!
//! Both the **classic** cross-reference table (§7.5.4) and cross-reference **streams** (§7.5.8)
//! are read, including the compressed objects that streams index (object streams, §7.5.7) and the
//! hybrid `/XRefStm` link (§7.5.8.4); Flate- and LZW-encoded streams are both supported via the
//! filter layer. Rebuilding a broken table by scanning (recovery, DESIGN.md §3) is a later slice.
//!
//! Hostile input is assumed (DESIGN.md §3.4): the `/Prev` walk is cycle-guarded and section/entry
//! counts are bounded by the input length, so no malformed file can loop forever or allocate
//! unboundedly.

use std::collections::BTreeMap;
use std::collections::BTreeSet;

use std::sync::Arc;

use pdf_cos::syntax::{is_delimiter, is_regular, is_whitespace};
use pdf_cos::{Dictionary, Name, Object, ObjectId, PdfString, Stream};

use crate::error::{ErrorKind, ReaderError, Result};
use crate::lexer::{Lexer, Token};
use crate::parser::{Limits, Parser};

/// A parsed PDF version from the file header (§7.5.2), e.g. `1.7` or `2.0`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Version {
    /// Major version (the `1` in `%PDF-1.7`).
    pub major: u8,
    /// Minor version (the `7` in `%PDF-1.7`).
    pub minor: u8,
}

/// A single cross-reference table entry (§7.5.4, Table 18).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum XRefEntry {
    /// A free entry (`f`, type 0): the object is not in use. Carries the next free object number
    /// and the generation to use if this slot is reused.
    Free { next_free: u32, generation: u16 },
    /// An in-use entry (`n`, type 1): the object's body begins at byte `offset`.
    InUse { offset: u64, generation: u16 },
    /// A compressed entry (type 2, §7.5.8): the object lives inside the object stream (§7.5.7)
    /// `container` at position `index`. Only cross-reference *streams* produce these; the object's
    /// generation is always 0.
    Compressed { container: u32, index: u32 },
}

/// Decrypts an object's string/stream bytes given its number and generation (§7.6). Supplied by
/// the document layer once the `/Encrypt` dictionary is understood; the reader stays
/// crypto-agnostic.
///
/// `None` means the bytes could not be decrypted. For an authenticated crypt filter
/// (`AESV4`/AES-256-GCM) that is a failed authentication tag, i.e. the document was altered — the
/// reader turns it into [`ErrorKind::DecryptionFailed`] rather than substituting empty content,
/// because a caller must be able to distinguish a tampered file from one with empty streams.
pub type DecryptFn = dyn Fn(u32, u16, &[u8]) -> Option<Vec<u8>> + Send + Sync;

/// A cloneable, `Debug`-able wrapper around a [`DecryptFn`].
#[derive(Clone)]
struct Decryptor(Arc<DecryptFn>);

impl std::fmt::Debug for Decryptor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Decryptor")
    }
}

/// The resolved cross-reference information for a document (§7.5).
#[derive(Clone, Debug)]
pub struct XRef {
    /// Header version (§7.5.2), if one was found.
    pub version: Option<Version>,
    /// Object number → entry, merged across all `/Prev` sections (most recent wins).
    pub entries: BTreeMap<u32, XRefEntry>,
    /// The trailer dictionary of the most recent section (§7.5.5).
    pub trailer: Dictionary,
    /// The limits applied when fetching objects.
    limits: Limits,
    /// Decryptor for fetched object content, if the document is encrypted (§7.6).
    decryptor: Option<Decryptor>,
    /// Object number whose content is never decrypted (the `/Encrypt` dictionary itself).
    exempt: Option<u32>,
}

impl XRef {
    /// Install a decryptor (§7.6): fetched objects' strings/streams are decrypted, except the
    /// `exempt` object (the `/Encrypt` dictionary, whose strings are stored unencrypted).
    pub fn set_decryptor(&mut self, exempt: Option<u32>, decrypt: Arc<DecryptFn>) {
        self.exempt = exempt;
        self.decryptor = Some(Decryptor(decrypt));
    }

    /// Decrypt a freshly-parsed object's content, unless decryption is off or it is exempt.
    fn decrypt(&self, object: Object, number: u32, generation: u16) -> Result<Object> {
        match &self.decryptor {
            Some(d) if self.exempt != Some(number) => {
                decrypt_object(&object, number, generation, &*d.0)
                    .ok_or_else(|| ReaderError::new(ErrorKind::DecryptionFailed, 0))
            }
            _ => Ok(object),
        }
    }
}

/// Recursively decrypt the strings and stream bytes of an object (§7.6.2). `None` if any piece
/// fails to decrypt (see [`DecryptFn`]).
fn decrypt_object(
    object: &Object,
    number: u32,
    generation: u16,
    decrypt: &DecryptFn,
) -> Option<Object> {
    Some(match object {
        Object::String(s) => {
            Object::String(PdfString::from(decrypt(number, generation, s.as_bytes())?))
        }
        Object::Array(array) => Object::Array(
            array
                .iter()
                .map(|item| decrypt_object(item, number, generation, decrypt))
                .collect::<Option<_>>()?,
        ),
        Object::Dictionary(dict) => {
            Object::Dictionary(decrypt_dict(dict, number, generation, decrypt)?)
        }
        Object::Stream(stream) => {
            let dict = decrypt_dict(stream.dict(), number, generation, decrypt)?;
            // §7.4.10/§7.6.2: a stream whose first filter is /Crypt with the Identity crypt filter
            // is not encrypted — decrypting it would corrupt it (common for Metadata when
            // /EncryptMetadata is false). Leave its data untouched; dict strings still decrypt.
            let raw = if stream_data_is_unencrypted(stream.dict()) {
                stream.raw().to_vec()
            } else {
                decrypt(number, generation, stream.raw())?
            };
            Object::Stream(Stream::new(dict, raw))
        }
        other => other.clone(),
    })
}

/// Whether a stream's data is exempt from the security handler because it carries an explicit
/// `/Crypt` filter naming the **Identity** crypt filter (§7.4.10). The Crypt filter, when present,
/// must be first in the `/Filter` array; its crypt-filter name comes from the matching
/// `/DecodeParms`, defaulting to `Identity` when absent (Table 14).
fn stream_data_is_unencrypted(dict: &Dictionary) -> bool {
    let crypt_is_first = match dict.get(&Name::from("Filter")) {
        Some(Object::Name(n)) => n.as_bytes() == b"Crypt",
        Some(Object::Array(arr)) => {
            matches!(arr.first(), Some(Object::Name(n)) if n.as_bytes() == b"Crypt")
        }
        _ => false,
    };
    if !crypt_is_first {
        return false;
    }
    // The DecodeParms entry for the (first) Crypt filter: a lone dict, or the first array element.
    let parms = match dict.get(&Name::from("DecodeParms")) {
        Some(Object::Dictionary(d)) => Some(d),
        Some(Object::Array(arr)) => arr.first().and_then(|o| match o {
            Object::Dictionary(d) => Some(d),
            _ => None,
        }),
        _ => None,
    };
    match parms.and_then(|p| p.get_name(&Name::from("Name"))) {
        Some(name) => name.as_bytes() == b"Identity",
        None => true, // default crypt-filter name is Identity
    }
}

/// Decrypt every value of a dictionary, honouring the §7.6.2 exemption for a signature
/// dictionary's `/Contents`.
fn decrypt_dict(
    dict: &Dictionary,
    number: u32,
    generation: u16,
    decrypt: &DecryptFn,
) -> Option<Dictionary> {
    let signature = is_signature_dict(dict);
    dict.iter()
        .map(|(key, value)| {
            // §7.6.2: "strings in the signature dictionary's Contents entry shall not be
            // encrypted" — the signature is written into a reserved placeholder *after* the
            // document is encrypted, so its bytes are already plaintext. Running the security
            // handler over them corrupts the CMS, and under an authenticated or padded cipher the
            // attempt fails outright.
            if signature && key.as_bytes() == b"Contents" {
                return Some((key.clone(), value.clone()));
            }
            Some((
                key.clone(),
                decrypt_object(value, number, generation, decrypt)?,
            ))
        })
        .collect()
}

/// Whether `dict` is a signature dictionary (§12.8.1) whose `/Contents` is exempt from encryption.
///
/// Recognised by an explicit `/Type` of `/Sig` or `/DocTimeStamp`, or — since `/Type` is optional
/// there — by the `/ByteRange` + `/Contents` pair that every signature dictionary carries.
fn is_signature_dict(dict: &Dictionary) -> bool {
    if matches!(
        dict.get_name(&Name::from("Type")).map(Name::as_bytes),
        Some(b"Sig") | Some(b"DocTimeStamp")
    ) {
        return true;
    }
    matches!(dict.get(&Name::from("ByteRange")), Some(Object::Array(_)))
        && matches!(dict.get(&Name::from("Contents")), Some(Object::String(_)))
}

/// Upper bound on the number of `/Prev` sections followed, beyond the cycle guard — a malformed
/// chain of distinct offsets cannot be longer than the file has bytes, but cap it well below that.
const MAX_XREF_SECTIONS: usize = 8192;

impl XRef {
    /// Parse the header, cross-reference table and trailer of `input` (§7.5).
    pub fn parse(input: &[u8]) -> Result<Self> {
        Self::parse_with_limits(input, Limits::default())
    }

    /// As [`parse`](Self::parse) with explicit anti-DoS [`Limits`].
    pub fn parse_with_limits(input: &[u8], limits: Limits) -> Result<Self> {
        let version = parse_header(input);

        let mut offset = find_startxref(input)?;
        let mut entries: BTreeMap<u32, XRefEntry> = BTreeMap::new();
        let mut trailer: Option<Dictionary> = None;
        let mut seen = BTreeSet::new();

        for _ in 0..MAX_XREF_SECTIONS {
            if !seen.insert(offset) {
                break; // cycle in the /Prev chain (§7.5.6) — stop rather than loop.
            }
            let section = read_section(input, offset, limits)?;

            // Sections are visited newest-first, so an earlier (newer) entry wins.
            for (number, entry) in section.entries {
                entries.entry(number).or_insert(entry);
            }

            // Hybrid-reference files (§7.5.8.4): a classic trailer may point via /XRefStm to a
            // supplementary cross-reference stream that holds the compressed-object entries. It is
            // best-effort — a broken /XRefStm must not sink an otherwise-readable file.
            if let Some(xrefstm) = section.trailer.get_integer(&Name::from("XRefStm")) {
                if xrefstm >= 0 && seen.insert(xrefstm as usize) {
                    if let Ok(extra) = read_xref_stream_section(input, xrefstm as usize, limits) {
                        for (number, entry) in extra.entries {
                            entries.entry(number).or_insert(entry);
                        }
                    }
                }
            }

            let prev = section.trailer.get_integer(&Name::from("Prev"));
            if trailer.is_none() {
                trailer = Some(section.trailer);
            }
            match prev {
                Some(p) if p >= 0 => offset = p as usize,
                _ => break,
            }
        }

        // Anti-DoS (DESIGN.md §3.4): refuse a table that declares implausibly many objects. Open
        // falls back to recovery, which instead *truncates* the scan to the same bound.
        if entries.len() > limits.max_objects {
            return Err(ReaderError::new(ErrorKind::LimitExceeded, 0));
        }

        Ok(Self {
            version,
            entries,
            trailer: trailer.ok_or_else(|| ReaderError::new(ErrorKind::InvalidXref, 0))?,
            limits,
            decryptor: None,
            exempt: None,
        })
    }

    /// Rebuild the cross-reference table by scanning the whole file (DESIGN.md §3: recovery is
    /// first-class, not a fallback). Used when the `startxref`/xref/trailer cannot be trusted —
    /// real PDFs are frequently broken.
    pub fn rebuild(input: &[u8]) -> Result<Self> {
        Self::rebuild_with_limits(input, Limits::default())
    }

    /// As [`rebuild`](Self::rebuild) with explicit anti-DoS [`Limits`].
    ///
    /// Scans for every `n g obj` header (recording the *actual* offsets, latest definition
    /// winning), expands object streams so compressed objects — including a catalog stored in one
    /// — are found, then reconstructs a trailer: an explicit `trailer`/xref-stream `/Root` if one
    /// is sound, otherwise the last scanned `/Type /Catalog` object.
    pub fn rebuild_with_limits(input: &[u8], limits: Limits) -> Result<Self> {
        let version = parse_header(input);

        // 1. Locate every indirect object by scanning; a later definition supersedes an earlier
        //    one (incremental updates append, §7.5.6). Bound the scan against a hostile file padded
        //    with millions of fabricated `n g obj` headers (anti-DoS, DESIGN.md §3.4).
        let mut headers = scan_object_headers(input);
        headers.truncate(limits.max_objects);
        let mut entries: BTreeMap<u32, XRefEntry> = BTreeMap::new();
        for &(number, generation, offset) in &headers {
            entries.insert(
                number,
                XRefEntry::InUse {
                    offset: offset as u64,
                    generation,
                },
            );
        }

        // 2. Visit each object once: expand object streams (§7.5.7) into compressed entries, and
        //    note the most recent catalog and any xref-stream dictionary (for /Root, /Info).
        let mut catalog: Option<ObjectId> = None;
        let mut xref_dict: Option<Dictionary> = None;
        let mut compressed: Vec<(u32, XRefEntry)> = Vec::new();
        for &(number, _generation, offset) in &headers {
            let mut parser = Parser::with_limits(input, limits);
            parser.seek(offset);
            let Ok((_id, object)) = parser.parse_indirect_object() else {
                continue;
            };
            match object {
                Object::Stream(stream) => match type_of(stream.dict()) {
                    Some(b"ObjStm") => {
                        if let Ok(members) = objstm_members(&stream, limits) {
                            for (member, index) in members {
                                if compressed.len() >= limits.max_objects {
                                    break; // bound the compressed-object accumulation (anti-DoS)
                                }
                                compressed.push((
                                    member,
                                    XRefEntry::Compressed {
                                        container: number,
                                        index,
                                    },
                                ));
                            }
                        }
                    }
                    Some(b"XRef") if xref_dict.is_none() => {
                        xref_dict = Some(stream.dict().clone());
                    }
                    _ => {}
                },
                Object::Dictionary(dict) if type_of(&dict) == Some(b"Catalog") => {
                    catalog = Some(ObjectId::new(number, 0)); // latest wins (file order)
                }
                _ => {}
            }
        }
        // Compressed entries fill gaps but never override a directly-scanned object, up to the cap.
        for (number, entry) in compressed {
            if entries.len() >= limits.max_objects {
                break;
            }
            entries.entry(number).or_insert(entry);
        }

        let trailer = recover_trailer(input, limits, catalog, xref_dict, &entries)?;
        Ok(Self {
            version,
            entries,
            trailer,
            limits,
            decryptor: None,
            exempt: None,
        })
    }

    /// The `/Root` catalog reference from the trailer (§7.7.2), if present.
    #[must_use]
    pub fn root(&self) -> Option<ObjectId> {
        self.trailer.get_reference(&Name::from("Root"))
    }

    /// The `/Size` (one past the highest object number) from the trailer (§7.5.5), if present.
    #[must_use]
    pub fn size(&self) -> Option<i64> {
        self.trailer.get_integer(&Name::from("Size"))
    }

    /// The entry for object `number`, if the table has one.
    #[must_use]
    pub fn entry(&self, number: u32) -> Option<XRefEntry> {
        self.entries.get(&number).copied()
    }

    /// Locate and parse the indirect object with the given `number` (§7.5.4/§7.5.7 + §7.3.10).
    ///
    /// Handles both uncompressed objects (parsed at their byte offset) and compressed ones
    /// (extracted from their object stream, §7.5.7). Returns `Ok(None)` for a free or absent
    /// object. References inside the returned object are **not** resolved — that is the document
    /// layer's job (ADR-0001).
    pub fn fetch(&self, input: &[u8], number: u32) -> Result<Option<Object>> {
        match self.entry(number) {
            Some(XRefEntry::InUse { offset, .. }) => {
                let mut parser = Parser::with_limits(input, self.limits);
                parser.seek(offset as usize);
                let (id, object) = parser.parse_indirect_object()?;
                // A mismatched object number means the table points at the wrong place; surface it
                // rather than returning a silently-wrong object.
                if id.number != number {
                    return Err(ReaderError::new(ErrorKind::InvalidXref, offset as usize));
                }
                self.decrypt(object, number, id.generation).map(Some)
            }
            Some(XRefEntry::Compressed { container, index }) => self
                .fetch_compressed(input, container, index, number)
                .map(Some),
            None | Some(XRefEntry::Free { .. }) => Ok(None),
        }
    }

    /// Extract object `number` from object stream `container` at position `index` (§7.5.7).
    fn fetch_compressed(
        &self,
        input: &[u8],
        container: u32,
        index: u32,
        number: u32,
    ) -> Result<Object> {
        // The container must itself be an uncompressed stream — an object stream is never nested
        // inside another (§7.5.7), so this cannot recurse and cannot cycle.
        let Some(XRefEntry::InUse { offset, .. }) = self.entry(container) else {
            return Err(ReaderError::new(ErrorKind::InvalidXref, 0));
        };
        let mut parser = Parser::with_limits(input, self.limits);
        parser.seek(offset as usize);
        let (id, object) = parser.parse_indirect_object()?;
        // Decrypt the container stream (§7.6) before decoding; the objects it holds are then plain.
        let object = self.decrypt(object, container, id.generation)?;
        let stream = match object {
            Object::Stream(s) if id.number == container => s,
            _ => return Err(ReaderError::new(ErrorKind::InvalidXref, offset as usize)),
        };

        let n = objstm_count(&stream, self.limits, offset as usize)?;
        let first = stream
            .dict()
            .get_integer(&Name::from("First"))
            .and_then(|f| usize::try_from(f).ok())
            .ok_or_else(|| ReaderError::new(ErrorKind::InvalidXref, offset as usize))?;
        let decoded = self
            .limits
            .decode(&stream)
            .map_err(|_| ReaderError::new(ErrorKind::StreamDecodeFailed, offset as usize))?;

        // The object stream begins with N pairs of integers: (object number, relative offset).
        let mut header = Parser::with_limits(&decoded, self.limits);
        let mut target: Option<usize> = None;
        let mut target_number: Option<u32> = None;
        for i in 0..n {
            let obj_number = expect_int(&mut header)?;
            let rel_offset = expect_int(&mut header)?;
            if i == index as usize {
                target = usize::try_from(rel_offset).ok();
                target_number = u32::try_from(obj_number).ok();
            }
        }
        let rel = target.ok_or_else(|| ReaderError::new(ErrorKind::InvalidXref, 0))?;
        if target_number != Some(number) {
            return Err(ReaderError::new(ErrorKind::InvalidXref, 0));
        }

        let start = first
            .checked_add(rel)
            .ok_or_else(|| ReaderError::new(ErrorKind::InvalidXref, 0))?;
        let mut body = Parser::with_limits(&decoded, self.limits);
        body.seek(start);
        body.parse_object()?
            .ok_or_else(|| ReaderError::new(ErrorKind::UnexpectedEof, start))
    }
}

/// Read the next object as a non-negative integer (used for object-stream headers, §7.5.7).
fn expect_int(parser: &mut Parser<'_>) -> Result<i64> {
    match parser.parse_object()? {
        Some(Object::Integer(n)) => Ok(n),
        _ => Err(ReaderError::new(ErrorKind::InvalidXref, parser.offset())),
    }
}

/// One cross-reference section: its entries plus its trailer dictionary.
struct Section {
    entries: Vec<(u32, XRefEntry)>,
    trailer: Dictionary,
}

/// Parse the `%PDF-M.m` header (§7.5.2). The marker is searched for in the first 1 KiB so a few
/// junk bytes before it (common in the wild) do not defeat detection. Returns `None` if absent or
/// unparseable — a missing header does not by itself prevent reading the file.
fn parse_header(input: &[u8]) -> Option<Version> {
    const MARKER: &[u8] = b"%PDF-";
    let window = &input[..input.len().min(1024)];
    let at = find(window, 0, MARKER)? + MARKER.len();
    let major = digit(input.get(at).copied()?)?;
    if input.get(at + 1).copied()? != b'.' {
        return None;
    }
    let minor = digit(input.get(at + 2).copied()?)?;
    Some(Version { major, minor })
}

/// Find the byte offset named by the last `startxref` keyword (§7.5.5). The last one is
/// authoritative for incremental updates.
fn find_startxref(input: &[u8]) -> Result<usize> {
    let kw = rfind(input, b"startxref")
        .ok_or_else(|| ReaderError::new(ErrorKind::MissingStartxref, input.len()))?;
    let mut lexer = Lexer::new(input);
    lexer.set_offset(kw + b"startxref".len());
    match lexer.next_token()? {
        Some(Token::Integer(n)) if n >= 0 => Ok(n as usize),
        _ => Err(ReaderError::new(ErrorKind::MissingStartxref, kw)),
    }
}

/// Read one cross-reference section starting at `offset` (§7.5.4–§7.5.5/§7.5.8).
///
/// Dispatches on the first token: the `xref` keyword opens a classic table; an integer opens an
/// `n g obj` whose value is a cross-reference stream.
fn read_section(input: &[u8], offset: usize, limits: Limits) -> Result<Section> {
    let mut lexer = Lexer::new(input);
    lexer.set_offset(offset);
    match next_token(&mut lexer)? {
        Token::Keyword(kw) if kw == b"xref" => read_classic_section(input, lexer, limits),
        Token::Integer(_) => read_xref_stream_section(input, offset, limits),
        _ => Err(ReaderError::new(ErrorKind::InvalidXref, offset)),
    }
}

/// Read a classic cross-reference table and its trailer (§7.5.4–§7.5.5), continuing from a lexer
/// positioned just after the `xref` keyword.
fn read_classic_section<'a>(
    input: &'a [u8],
    mut lexer: Lexer<'a>,
    limits: Limits,
) -> Result<Section> {
    let mut entries = Vec::new();
    loop {
        // Each subsection starts with `first count`; the run of subsections ends at `trailer`.
        match next_token(&mut lexer)? {
            Token::Keyword(kw) if kw == b"trailer" => break,
            Token::Integer(first) if first >= 0 => {
                let first = u32::try_from(first)
                    .map_err(|_| ReaderError::new(ErrorKind::InvalidXref, lexer.offset()))?;
                let count = next_uint(&mut lexer)?;
                // An honest table cannot have more entries than the file has bytes.
                if count > input.len() as u64 {
                    return Err(ReaderError::new(ErrorKind::InvalidXref, lexer.offset()));
                }
                for i in 0..count {
                    let number = first
                        .checked_add(u32::try_from(i).unwrap_or(u32::MAX))
                        .ok_or_else(|| ReaderError::new(ErrorKind::InvalidXref, lexer.offset()))?;
                    entries.push((number, read_entry(&mut lexer)?));
                }
            }
            _ => return Err(ReaderError::new(ErrorKind::InvalidXref, lexer.offset())),
        }
    }

    // The `trailer` keyword was just consumed; parse the dictionary that follows.
    let mut parser = Parser::with_limits(input, limits);
    parser.seek(lexer.offset());
    let trailer = match parser.parse_object()? {
        Some(Object::Dictionary(dict)) => dict,
        _ => return Err(ReaderError::new(ErrorKind::InvalidXref, lexer.offset())),
    };

    Ok(Section { entries, trailer })
}

/// Read a cross-reference *stream* section (§7.5.8): an `n g obj` whose stream value encodes the
/// entries in binary, and whose dictionary doubles as the trailer.
fn read_xref_stream_section(input: &[u8], offset: usize, limits: Limits) -> Result<Section> {
    let mut parser = Parser::with_limits(input, limits);
    parser.seek(offset);
    let (_, object) = parser.parse_indirect_object()?;
    let Object::Stream(stream) = object else {
        return Err(ReaderError::new(ErrorKind::InvalidXref, offset));
    };
    let dict = stream.dict().clone();
    let entries = parse_xref_stream_entries(&dict, &stream, offset, limits)?;
    Ok(Section {
        entries,
        trailer: dict,
    })
}

/// Decode a cross-reference stream and parse its binary entries per `/W` and `/Index` (§7.5.8).
fn parse_xref_stream_entries(
    dict: &Dictionary,
    stream: &Stream,
    offset: usize,
    limits: Limits,
) -> Result<Vec<(u32, XRefEntry)>> {
    let err = || ReaderError::new(ErrorKind::InvalidXref, offset);

    // /W [w0 w1 w2]: the byte width of each entry's type, field-2 and field-3 (§7.5.8.2).
    let w = dict.get_array(&Name::from("W")).ok_or_else(err)?;
    if w.len() != 3 {
        return Err(err());
    }
    let widths: Vec<usize> = w
        .iter()
        .map(|o| {
            o.as_integer()
                .and_then(|n| usize::try_from(n).ok())
                .filter(|&n| n <= 8)
                .ok_or_else(err)
        })
        .collect::<Result<_>>()?;
    let (w0, w1, w2) = (widths[0], widths[1], widths[2]);
    let entry_width = w0 + w1 + w2;
    if entry_width == 0 {
        return Err(err());
    }

    let size = dict.get_integer(&Name::from("Size"));
    // /Index [start count ...] defaults to [0 Size] (§7.5.8.2).
    let index: Vec<i64> = match dict.get_array(&Name::from("Index")) {
        Some(arr) => arr.iter().filter_map(Object::as_integer).collect(),
        None => vec![0, size.ok_or_else(err)?],
    };

    let data = limits
        .decode(stream)
        .map_err(|_| ReaderError::new(ErrorKind::StreamDecodeFailed, offset))?;

    let mut entries = Vec::new();
    let mut pos = 0usize;
    for pair in index.chunks(2) {
        let [start, count] = pair else { break };
        let start = u32::try_from(*start).map_err(|_| err())?;
        let count = u64::try_from(*count).map_err(|_| err())?;
        if count > data.len() as u64 {
            return Err(err());
        }
        for i in 0..count {
            let number = start
                .checked_add(u32::try_from(i).unwrap_or(u32::MAX))
                .ok_or_else(err)?;
            // §7.5.8.3: a zero-width type field defaults to type 1.
            let kind = if w0 == 0 {
                1
            } else {
                read_be(&data, &mut pos, w0).ok_or_else(err)?
            };
            let f1 = read_be(&data, &mut pos, w1).ok_or_else(err)?;
            let f2 = read_be(&data, &mut pos, w2).ok_or_else(err)?;
            match kind {
                0 => entries.push((
                    number,
                    XRefEntry::Free {
                        next_free: u32::try_from(f1).unwrap_or(u32::MAX),
                        generation: u16::try_from(f2).unwrap_or(0),
                    },
                )),
                1 => entries.push((
                    number,
                    XRefEntry::InUse {
                        offset: f1,
                        generation: u16::try_from(f2).unwrap_or(0),
                    },
                )),
                2 => entries.push((
                    number,
                    XRefEntry::Compressed {
                        container: u32::try_from(f1).unwrap_or(u32::MAX),
                        index: u32::try_from(f2).unwrap_or(u32::MAX),
                    },
                )),
                // Unknown entry types are reserved (§7.5.8.3); skip rather than fail.
                _ => {}
            }
        }
    }
    Ok(entries)
}

/// Read a big-endian unsigned integer of `width` bytes (0 ≤ width ≤ 8), advancing `pos`. A zero
/// width yields 0 without consuming input (§7.5.8.2).
fn read_be(data: &[u8], pos: &mut usize, width: usize) -> Option<u64> {
    if width == 0 {
        return Some(0);
    }
    let end = pos.checked_add(width)?;
    let bytes = data.get(*pos..end)?;
    let mut value = 0u64;
    for &b in bytes {
        value = (value << 8) | u64::from(b);
    }
    *pos = end;
    Some(value)
}

/// The bytes of a dictionary's `/Type` name, if it has one.
fn type_of(dict: &Dictionary) -> Option<&[u8]> {
    dict.get_name(&Name::from("Type")).map(Name::as_bytes)
}

/// Read and validate an object stream's `/N` count (§7.5.7) against [`Limits::max_objstm_objects`]
/// (anti-DoS, DESIGN.md §3.4): an attacker-chosen `/N` far larger than the stream data must not
/// drive a huge allocation or loop. `offset` locates the error.
fn objstm_count(stream: &Stream, limits: Limits, offset: usize) -> Result<usize> {
    let n = stream
        .dict()
        .get_integer(&Name::from("N"))
        .and_then(|n| usize::try_from(n).ok())
        .ok_or_else(|| ReaderError::new(ErrorKind::InvalidXref, offset))?;
    if n > limits.max_objstm_objects {
        return Err(ReaderError::new(ErrorKind::LimitExceeded, offset));
    }
    Ok(n)
}

/// The `(object number, relative offset)` members of an object stream (§7.5.7), used by recovery
/// to register each compressed object's index.
fn objstm_members(stream: &Stream, limits: Limits) -> Result<Vec<(u32, u32)>> {
    let n = objstm_count(stream, limits, 0)?;
    let decoded = limits
        .decode(stream)
        .map_err(|_| ReaderError::new(ErrorKind::StreamDecodeFailed, 0))?;
    let mut header = Parser::with_limits(&decoded, limits);
    // Pre-allocate against the data size, never the unvalidated `/N`: the header holds two integers
    // (≥1 byte each) per member, so `decoded.len()` caps how many can really be present.
    let mut members = Vec::with_capacity(n.min(decoded.len()));
    for index in 0..n {
        let number = u32::try_from(expect_int(&mut header)?)
            .map_err(|_| ReaderError::new(ErrorKind::InvalidXref, 0))?;
        let _relative_offset = expect_int(&mut header)?;
        members.push((number, u32::try_from(index).unwrap_or(u32::MAX)));
    }
    Ok(members)
}

/// Reconstruct a usable trailer during recovery. Prefers a scanned `/Type /Catalog` object as
/// `/Root` (ground truth in a corrupt file), else an explicit `trailer`/xref-stream `/Root`. Fills
/// in `/Size` from the highest object number when missing. Errors only if no catalog can be found.
fn recover_trailer(
    input: &[u8],
    limits: Limits,
    catalog: Option<ObjectId>,
    xref_dict: Option<Dictionary>,
    entries: &BTreeMap<u32, XRefEntry>,
) -> Result<Dictionary> {
    // The last parseable `trailer` dictionary in the file (incremental updates append).
    let mut explicit: Option<Dictionary> = None;
    let mut pos = 0;
    while let Some(at) = find(input, pos, b"trailer") {
        pos = at + b"trailer".len();
        let mut parser = Parser::with_limits(input, limits);
        parser.seek(pos);
        if let Ok(Some(Object::Dictionary(dict))) = parser.parse_object() {
            explicit = Some(dict);
        }
    }

    let mut trailer = explicit.or(xref_dict).unwrap_or_default();
    // A scanned catalog object is the most reliable /Root in a damaged file.
    if let Some(root) = catalog {
        trailer.insert(Name::from("Root"), Object::Reference(root));
    }
    if trailer.get_reference(&Name::from("Root")).is_none() {
        return Err(ReaderError::new(ErrorKind::InvalidXref, 0));
    }
    if trailer.get_integer(&Name::from("Size")).is_none() {
        let max = entries.keys().max().copied().unwrap_or(0);
        trailer.insert(Name::from("Size"), Object::Integer(i64::from(max) + 1));
    }
    Ok(trailer)
}

/// Scan the whole file for `n g obj` headers, returning `(object number, generation, offset)` in
/// file order. Robust against binary stream content: it works at the byte level and validates the
/// token boundaries around each `obj`, so `endobj` and substrings are not mistaken for headers.
fn scan_object_headers(input: &[u8]) -> Vec<(u32, u16, usize)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 3 <= input.len() {
        if &input[i..i + 3] == b"obj" {
            let boundary_ok = match input.get(i + 3) {
                None => true,
                Some(&b) => is_whitespace(b) || is_delimiter(b),
            };
            if boundary_ok {
                if let Some(header) = parse_obj_header_before(input, i) {
                    out.push(header);
                }
            }
            i += 3;
        } else {
            i += 1;
        }
    }
    out
}

/// Parse the `n g` of an `n g obj` header by walking backwards from the `obj` at `obj_pos`,
/// returning `(object number, generation, start offset)` or `None` if the bytes before `obj` are
/// not a well-formed header.
fn parse_obj_header_before(input: &[u8], obj_pos: usize) -> Option<(u32, u16, usize)> {
    let mut i = obj_pos;
    i = skip_back_whitespace(input, i)?;
    let (generation, gen_start) = take_back_digits(input, i)?;
    i = skip_back_whitespace(input, gen_start)?;
    let (number, num_start) = take_back_digits(input, i)?;
    // The object number must not be glued onto a regular character (e.g. inside a word).
    if num_start > 0 && is_regular(input[num_start - 1]) {
        return None;
    }
    Some((number, u16::try_from(generation).ok()?, num_start))
}

/// Skip whitespace backwards from `end`, requiring at least one byte; returns the new index.
fn skip_back_whitespace(input: &[u8], end: usize) -> Option<usize> {
    let mut i = end;
    while i > 0 && is_whitespace(input[i - 1]) {
        i -= 1;
    }
    (i < end).then_some(i)
}

/// Read ASCII digits backwards from `end`, requiring at least one; returns `(value, start index)`.
fn take_back_digits(input: &[u8], end: usize) -> Option<(u32, usize)> {
    let mut i = end;
    while i > 0 && input[i - 1].is_ascii_digit() {
        i -= 1;
    }
    if i == end {
        return None;
    }
    let value: u32 = std::str::from_utf8(&input[i..end]).ok()?.parse().ok()?;
    Some((value, i))
}

/// Read one 3-field cross-reference entry: `offset generation (n|f)` (§7.5.4, Table 18).
fn read_entry(lexer: &mut Lexer<'_>) -> Result<XRefEntry> {
    let field = next_uint(lexer)?;
    let generation = u16::try_from(next_uint(lexer)?)
        .map_err(|_| ReaderError::new(ErrorKind::InvalidXref, lexer.offset()))?;
    match next_token(lexer)? {
        Token::Keyword(kw) if kw == b"n" => Ok(XRefEntry::InUse {
            offset: field,
            generation,
        }),
        Token::Keyword(kw) if kw == b"f" => Ok(XRefEntry::Free {
            next_free: u32::try_from(field).unwrap_or(u32::MAX),
            generation,
        }),
        _ => Err(ReaderError::new(ErrorKind::InvalidXref, lexer.offset())),
    }
}

/// The next token, treating end of input as an error.
fn next_token(lexer: &mut Lexer<'_>) -> Result<Token> {
    lexer
        .next_token()?
        .ok_or_else(|| ReaderError::new(ErrorKind::UnexpectedEof, lexer.offset()))
}

/// The next token, requiring a non-negative integer, returned as `u64`.
fn next_uint(lexer: &mut Lexer<'_>) -> Result<u64> {
    match next_token(lexer)? {
        Token::Integer(n) if n >= 0 => Ok(n as u64),
        _ => Err(ReaderError::new(ErrorKind::InvalidXref, lexer.offset())),
    }
}

/// The decimal value of an ASCII digit byte, if it is one.
fn digit(b: u8) -> Option<u8> {
    if b.is_ascii_digit() {
        Some(b - b'0')
    } else {
        None
    }
}

/// First occurrence of `needle` in `hay[from..]`, or `None`.
fn find(hay: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || from >= hay.len() {
        return None;
    }
    hay[from..]
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|p| from + p)
}

/// Last occurrence of `needle` in `hay`, or `None`.
fn rfind(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || needle.len() > hay.len() {
        return None;
    }
    hay.windows(needle.len()).rposition(|w| w == needle)
}

#[cfg(test)]
mod tests;
