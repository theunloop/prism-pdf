#![forbid(unsafe_code)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]
//! pdf-writer — serializer & file assembly (EPIC 5, ISO 32000 §7.3/§7.5).
//!
//! Turns COS objects back into PDF bytes: [`serialize_object`] writes one value, and
//! [`write_document`] assembles a complete single-revision file — header, body, a classic
//! cross-reference table (§7.5.4), and trailer (§7.5.5). Depends only on [`pdf_cos`] and
//! [`pdf_filters`] (architecture: `writer → cos, filters`).
//!
//! Implemented (Milestone M2): full-rewrite serialization with a classic xref table. Incremental
//! update (§7.5.6) and cross-reference *stream* output are follow-ups.

mod serialize;
mod version;

pub use serialize::serialize_object;
pub use version::{VersionRequirement, min_version, version_violation};

use std::collections::HashMap;

use pdf_cos::{Dictionary, Name, Object, ObjectId, Stream};
use pdf_filters::flate_encode;
use sha2::{Digest, Sha256};

/// A 20-byte free cross-reference entry (§7.5.4): used for object 0 and any gap.
const FREE_ENTRY: &[u8] = b"0000000000 65535 f \n";

/// Maximum number of compressed objects per object stream (§7.5.7): a modest cap keeps each
/// container's header small and limits how much must be decoded to reach one object.
const MAX_OBJSTM_OBJECTS: usize = 100;

/// First version whose trailer **requires** a file identifier `/ID` (ISO 32000-2 §7.5.5, Table 15).
///
/// Up to PDF 1.7 (ISO 32000-1, Table 15) `/ID` is "required if an `Encrypt` entry is present …
/// strongly recommended otherwise"; PDF 2.0 promotes it to unconditionally required. A writer that
/// stamps `%PDF-2.0` therefore has to emit one, so from this version on it is synthesized when the
/// caller has none to preserve.
const ID_REQUIRED_FROM: (u8, u8) = (2, 0);

/// The length in bytes of a synthesized file identifier (§14.4): 16, the size §14.4's own worked
/// example produces and what every consumer expects.
const FILE_ID_LEN: usize = 16;

/// Derive a file identifier (§14.4) from the bytes assembled so far.
///
/// §14.4 asks for a value "computed by means of a message digest algorithm" over data that makes a
/// collision between two different files unlikely. Hashing the assembled body folds in every
/// object's content, which satisfies that and — unlike a clock or a random source — keeps
/// serialization **deterministic**: the same document always writes the same bytes, which is what
/// the round-trip tests assert.
fn synthesize_file_id(body: &[u8]) -> [u8; FILE_ID_LEN] {
    let digest = Sha256::digest(body);
    let mut id = [0u8; FILE_ID_LEN];
    id.copy_from_slice(&digest[..FILE_ID_LEN]);
    id
}

/// Assemble a complete PDF file (single revision) from a set of indirect objects.
///
/// `objects` are written body-first in ascending object-number order; `root` is the document
/// catalog (`/Root`), `info` the optional information dictionary, and `version` the header
/// version. The result is a valid file with a classic cross-reference table (§7.5.4) — gaps in the
/// object-number range are written as free entries.
///
/// `id` is the trailer file identifier (§14.4). Pass `Some(id)` to carry an existing document's
/// identity forward — the permanent element must stay constant for a file's lifetime, so a full
/// rewrite of an opened document should preserve it. `None` synthesizes one from the body when the
/// declared `version` requires it (`ID_REQUIRED_FROM`, PDF 2.0 on) and omits it otherwise. Both
/// array elements are written equal: this is a fresh original revision, not an update.
#[must_use]
pub fn write_document(
    objects: &[(ObjectId, Object)],
    root: ObjectId,
    info: Option<ObjectId>,
    version: (u8, u8),
    id: Option<&[u8]>,
) -> Vec<u8> {
    write_document_inner(objects, root, info, version, "", id)
}

/// Assemble a complete encrypted PDF (§7.6): like [`write_document`], but the trailer also carries
/// `/Encrypt` (pointing at the standard security handler's dictionary, itself one of `objects`) and
/// a `/ID` whose two elements are `id`. Object strings/streams must already be encrypted by the
/// caller; the `/Encrypt` object and the trailer `/ID` are written in the clear (§7.6.1).
#[must_use]
pub fn write_document_encrypted(
    objects: &[(ObjectId, Object)],
    root: ObjectId,
    info: Option<ObjectId>,
    version: (u8, u8),
    encrypt: ObjectId,
    id: &[u8],
) -> Vec<u8> {
    write_document_inner(
        objects,
        root,
        info,
        version,
        &id_trailer(id, Some(encrypt)),
        None,
    )
}

/// As [`write_document_encrypted`], but the trailer additionally carries a direct `/AuthCode`
/// dictionary (ISO/TS 32004 §5.2.1, Table 5) — the standalone PDF MAC token's home. `authcode` is
/// the verbatim ` /AuthCode << … >>` fragment (with leading space); the caller patches its
/// `/ByteRange` and `/MAC` placeholders after layout. The `/AuthCode` strings stay in the clear
/// (§5.2.2: the MAC byte string is exempt from encryption).
#[must_use]
pub fn write_document_encrypted_with_authcode(
    objects: &[(ObjectId, Object)],
    root: ObjectId,
    info: Option<ObjectId>,
    version: (u8, u8),
    encrypt: ObjectId,
    id: &[u8],
    authcode: &str,
) -> Vec<u8> {
    let trailer = format!("{}{authcode}", id_trailer(id, Some(encrypt)));
    write_document_inner(objects, root, info, version, &trailer, None)
}

/// Format the trailer fragment carrying a file `/ID` (§14.4) and, when encrypting, the `/Encrypt`
/// reference (§7.6.1). Both `/ID` array elements are the same hex string (original revision).
fn id_trailer(id: &[u8], encrypt: Option<ObjectId>) -> String {
    let hex: String = id.iter().map(|b| format!("{b:02x}")).collect();
    match encrypt {
        Some(e) => format!(
            " /Encrypt {} {} R /ID [<{hex}> <{hex}>]",
            e.number, e.generation
        ),
        None => format!(" /ID [<{hex}> <{hex}>]"),
    }
}

/// Shared body of the full-rewrite writers: `trailer_extra` is appended verbatim inside the
/// trailer dictionary (before `>>`), letting the encrypted variant add `/Encrypt` and `/ID`.
/// `id` is the file identifier for the unencrypted paths (the encrypted ones always carry their
/// own inside `trailer_extra`, §7.6.1).
fn write_document_inner(
    objects: &[(ObjectId, Object)],
    root: ObjectId,
    info: Option<ObjectId>,
    version: (u8, u8),
    trailer_extra: &str,
    id: Option<&[u8]>,
) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(format!("%PDF-{}.{}\n", version.0, version.1).as_bytes());
    // Binary marker (§7.5.2): high bytes in a comment mark the file as binary for transfer tools.
    out.extend_from_slice(b"%\xE2\xE3\xCF\xD3\n");

    // Write each object body in ascending number order, recording its offset.
    let mut sorted: Vec<&(ObjectId, Object)> = objects.iter().collect();
    sorted.sort_by_key(|(id, _)| id.number);
    let mut located: HashMap<u32, (usize, u16)> = HashMap::with_capacity(sorted.len());
    for (id, object) in &sorted {
        let offset = out.len();
        out.extend_from_slice(format!("{} {} obj\n", id.number, id.generation).as_bytes());
        serialize_object(&mut out, object);
        out.extend_from_slice(b"\nendobj\n");
        located.insert(id.number, (offset, id.generation));
    }

    let max_number = sorted.last().map_or(0, |(id, _)| id.number);
    let size = u64::from(max_number) + 1;

    // Classic cross-reference table (§7.5.4): one subsection covering 0..size.
    let startxref = out.len();
    out.extend_from_slice(format!("xref\n0 {size}\n").as_bytes());
    out.extend_from_slice(FREE_ENTRY); // object 0 is the head of the free list
    for number in 1..size {
        let entry = u32::try_from(number).ok().and_then(|n| located.get(&n));
        match entry {
            // Each entry is exactly 20 bytes: 10-digit offset, gen, type, EOL.
            Some(&(offset, generation)) => {
                out.extend_from_slice(format!("{offset:010} {generation:05} n \n").as_bytes());
            }
            None => out.extend_from_slice(FREE_ENTRY),
        }
    }

    // Trailer (§7.5.5).
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {size} /Root {} {} R",
            root.number, root.generation
        )
        .as_bytes(),
    );
    if let Some(info) = info {
        out.extend_from_slice(format!(" /Info {} {} R", info.number, info.generation).as_bytes());
    }
    // File identifier (§14.4): the caller's if it has one to preserve, otherwise synthesized from
    // the body when the declared version requires `/ID` (§7.5.5). The encrypted writers put theirs
    // in `trailer_extra`, so never add a second one here.
    let synthesized =
        (id.is_none() && version >= ID_REQUIRED_FROM && !trailer_extra.contains("/ID"))
            .then(|| synthesize_file_id(&out));
    if let Some(id) = id.or(synthesized.as_ref().map(|s| &s[..])) {
        out.extend_from_slice(id_trailer(id, None).as_bytes());
    }
    out.extend_from_slice(trailer_extra.as_bytes());
    out.extend_from_slice(b" >>\n");
    out.extend_from_slice(format!("startxref\n{startxref}\n%%EOF\n").as_bytes());
    out
}

/// Assemble a complete PDF whose cross-reference is a **cross-reference stream** (§7.5.8) rather
/// than a classic table — the compact modern form, readable by PDF 1.5+ consumers.
///
/// The objects are written body-first as in [`write_document`]; the cross-reference itself is then
/// emitted as one more indirect object — a `/Type /XRef` stream of fixed-width binary entries
/// (`FlateDecode`-compressed), with the trailer keys (`/Size`, `/Root`, `/Info`, `/ID`) in its
/// dictionary. There is no `trailer` keyword and no classic table; `startxref` points at the stream
/// object. `id` behaves as in [`write_document`].
#[must_use]
pub fn write_document_xref_stream(
    objects: &[(ObjectId, Object)],
    root: ObjectId,
    info: Option<ObjectId>,
    version: (u8, u8),
    id: Option<&[u8]>,
) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(format!("%PDF-{}.{}\n", version.0, version.1).as_bytes());
    out.extend_from_slice(b"%\xE2\xE3\xCF\xD3\n");

    let mut sorted: Vec<&(ObjectId, Object)> = objects.iter().collect();
    sorted.sort_by_key(|(id, _)| id.number);
    let mut located: HashMap<u32, (usize, u16)> = HashMap::with_capacity(sorted.len());
    for (id, object) in &sorted {
        let offset = out.len();
        out.extend_from_slice(format!("{} {} obj\n", id.number, id.generation).as_bytes());
        serialize_object(&mut out, object);
        out.extend_from_slice(b"\nendobj\n");
        located.insert(id.number, (offset, id.generation));
    }

    // The cross-reference stream is itself an object, taking the next free number; its own entry
    // points at the offset where it begins (known now, before it is written).
    let max_number = sorted.last().map_or(0, |(id, _)| id.number);
    let xref_number = max_number + 1;
    let xref_offset = out.len() as u64;
    let size = u64::from(xref_number) + 1;

    // Build one entry per object number 0..size (§7.5.8.3): type 0 = free, 1 = in-use (offset,
    // generation), 2 = compressed — we never emit type 2 here (no object streams in our output).
    let entry = |number: u64| -> (u64, u64, u64) {
        if number == 0 {
            (0, 0, 65535) // head of the free list
        } else if number == u64::from(xref_number) {
            (1, xref_offset, 0)
        } else if let Some(&(offset, generation)) = located.get(&(number as u32)) {
            (1, offset as u64, u64::from(generation))
        } else {
            (0, 0, 0) // a gap in the number range
        }
    };
    let entries: Vec<(u64, u64, u64)> = (0..size).map(entry).collect();
    emit_xref_stream_tail(
        &mut out,
        xref_number,
        xref_offset,
        &entries,
        root,
        info,
        version,
        id,
    );
    out
}

/// Assemble a complete PDF that stores its non-stream objects inside **object streams** (§7.5.7),
/// cross-referenced by a **cross-reference stream** (§7.5.8) — the most compact form, readable by
/// PDF 1.5+ consumers (the header version is floored at 1.5 accordingly).
///
/// Only generation-0 non-stream objects are compressible (§7.5.7 forbids streams inside object
/// streams); stream objects and non-zero generations are written as normal indirect objects.
/// Compressed objects are packed `MAX_OBJSTM_OBJECTS` per container and located by type-2
/// cross-reference entries. Not for encrypted output: an object stream's contents would need the
/// container's keys, and `/Encrypt` itself may not be compressed — use the encrypted writers.
/// `id` behaves as in [`write_document`].
#[must_use]
pub fn write_document_object_streams(
    objects: &[(ObjectId, Object)],
    root: ObjectId,
    info: Option<ObjectId>,
    version: (u8, u8),
    id: Option<&[u8]>,
) -> Vec<u8> {
    let version = version.max((1, 5)); // object streams require PDF 1.5 (§7.5.7)
    let mut out = Vec::new();
    out.extend_from_slice(format!("%PDF-{}.{}\n", version.0, version.1).as_bytes());
    out.extend_from_slice(b"%\xE2\xE3\xCF\xD3\n");

    type ObjRefs<'a> = Vec<&'a (ObjectId, Object)>;
    let mut sorted: ObjRefs<'_> = objects.iter().collect();
    sorted.sort_by_key(|(id, _)| id.number);
    let (compressible, direct): (ObjRefs<'_>, ObjRefs<'_>) = sorted
        .iter()
        .partition(|(id, obj)| id.generation == 0 && !matches!(obj, Object::Stream(_)));

    // Direct (uncompressible) objects are written as usual, recording their offsets.
    let mut located: HashMap<u32, (usize, u16)> = HashMap::with_capacity(direct.len());
    for (id, object) in direct {
        let offset = out.len();
        out.extend_from_slice(format!("{} {} obj\n", id.number, id.generation).as_bytes());
        serialize_object(&mut out, object);
        out.extend_from_slice(b"\nendobj\n");
        located.insert(id.number, (offset, id.generation));
    }

    // Pack the compressible objects into ObjStm containers, numbered after the existing set.
    let max_number = sorted.last().map_or(0, |(id, _)| id.number);
    let mut next_number = max_number + 1;
    // Object number → (container number, index within it), for the type-2 xref entries.
    let mut packed: HashMap<u32, (u32, u16)> = HashMap::with_capacity(compressible.len());
    for chunk in compressible.chunks(MAX_OBJSTM_OBJECTS) {
        let container = next_number;
        next_number += 1;
        // §7.5.7: the decoded stream opens with N pairs of "objnum offset" integers (the offsets
        // relative to /First, the position of the first object body), then the bodies themselves.
        let mut header = Vec::new();
        let mut bodies = Vec::new();
        for (index, (id, object)) in chunk.iter().enumerate() {
            header.extend_from_slice(format!("{} {} ", id.number, bodies.len()).as_bytes());
            serialize_object(&mut bodies, object);
            bodies.push(b'\n');
            packed.insert(id.number, (container, index as u16));
        }
        let first = header.len();
        let mut payload = header;
        payload.extend_from_slice(&bodies);
        let compressed = flate_encode(&payload);

        let mut dict = Dictionary::new();
        dict.insert(Name::from("Type"), Object::Name(Name::from("ObjStm")));
        dict.insert(Name::from("N"), Object::Integer(chunk.len() as i64));
        dict.insert(Name::from("First"), Object::Integer(first as i64));
        dict.insert(
            Name::from("Filter"),
            Object::Name(Name::from("FlateDecode")),
        );
        located.insert(container, (out.len(), 0));
        out.extend_from_slice(format!("{container} 0 obj\n").as_bytes());
        serialize_object(&mut out, &Object::Stream(Stream::new(dict, compressed)));
        out.extend_from_slice(b"\nendobj\n");
    }

    // The cross-reference stream itself takes the next free number.
    let xref_number = next_number;
    let xref_offset = out.len() as u64;
    let size = u64::from(xref_number) + 1;
    let entry = |number: u64| -> (u64, u64, u64) {
        if number == 0 {
            (0, 0, 65535) // head of the free list
        } else if number == u64::from(xref_number) {
            (1, xref_offset, 0)
        } else if let Some(&(container, index)) = packed.get(&(number as u32)) {
            (2, u64::from(container), u64::from(index)) // compressed (§7.5.8.3 type 2)
        } else if let Some(&(offset, generation)) = located.get(&(number as u32)) {
            (1, offset as u64, u64::from(generation))
        } else {
            (0, 0, 0) // a gap in the number range
        }
    };
    let entries: Vec<(u64, u64, u64)> = (0..size).map(entry).collect();
    emit_xref_stream_tail(
        &mut out,
        xref_number,
        xref_offset,
        &entries,
        root,
        info,
        version,
        id,
    );
    out
}

/// Emit the cross-reference stream object (§7.5.8) and the `startxref` epilogue shared by the
/// xref-stream writers: binary fixed-width entries per `/W`, FlateDecode-compressed, with the
/// trailer keys in the stream's dictionary. `id`/`version` drive `/ID` exactly as in the classic
/// trailer (§14.4, §7.5.5) — an xref stream's dictionary *is* the trailer dictionary (§7.5.8.2).
#[expect(
    clippy::too_many_arguments,
    reason = "one call site per xref-stream writer; the trailer keys have no natural grouping"
)]
fn emit_xref_stream_tail(
    out: &mut Vec<u8>,
    xref_number: u32,
    xref_offset: u64,
    entries: &[(u64, u64, u64)],
    root: ObjectId,
    info: Option<ObjectId>,
    version: (u8, u8),
    id: Option<&[u8]>,
) {
    // Field widths /W: type always fits in 1 byte; the second field must hold the largest value
    // (an offset or a container number); the third (generation or in-stream index) fits 2 bytes.
    let max_offset = entries.iter().map(|&(_, f2, _)| f2).max().unwrap_or(0);
    let (w_type, w_offset, w_gen) = (1usize, bytes_needed(max_offset), 2usize);

    let mut data = Vec::with_capacity(entries.len() * (w_type + w_offset + w_gen));
    for &(t, f2, f3) in entries {
        push_be(&mut data, t, w_type);
        push_be(&mut data, f2, w_offset);
        push_be(&mut data, f3, w_gen);
    }
    let compressed = flate_encode(&data);

    let mut dict = Dictionary::new();
    dict.insert(Name::from("Type"), Object::Name(Name::from("XRef")));
    dict.insert(Name::from("Size"), Object::Integer(entries.len() as i64));
    dict.insert(
        Name::from("Root"),
        Object::Reference(ObjectId::new(root.number, root.generation)),
    );
    if let Some(info) = info {
        dict.insert(Name::from("Info"), Object::Reference(info));
    }
    // An xref stream's dictionary is the trailer dictionary (§7.5.8.2), so `/ID` belongs here on
    // exactly the same terms as in a classic trailer: preserved when given, synthesized from the
    // body once the declared version requires it (§7.5.5, §14.4).
    let synthesized =
        (id.is_none() && version >= ID_REQUIRED_FROM).then(|| synthesize_file_id(out));
    if let Some(id) = id.or(synthesized.as_ref().map(|s| &s[..])) {
        let element = Object::String(pdf_cos::PdfString::from(id.to_vec()));
        dict.insert(
            Name::from("ID"),
            Object::Array(pdf_cos::Array::from_vec(vec![element.clone(), element])),
        );
    }
    dict.insert(
        Name::from("W"),
        Object::Array(pdf_cos::Array::from_vec(vec![
            Object::Integer(w_type as i64),
            Object::Integer(w_offset as i64),
            Object::Integer(w_gen as i64),
        ])),
    );
    dict.insert(
        Name::from("Filter"),
        Object::Name(Name::from("FlateDecode")),
    );

    out.extend_from_slice(format!("{xref_number} 0 obj\n").as_bytes());
    // serialize_object rewrites /Length to the raw (compressed) byte count (ADR-0004).
    serialize_object(out, &Object::Stream(Stream::new(dict, compressed)));
    out.extend_from_slice(b"\nendobj\n");
    out.extend_from_slice(format!("startxref\n{xref_offset}\n%%EOF\n").as_bytes());
}

/// The minimum number of bytes (1–8) needed to hold `value` big-endian.
fn bytes_needed(value: u64) -> usize {
    let bits = 64 - value.leading_zeros() as usize;
    bits.div_ceil(8).max(1)
}

/// Append `value` as `width` big-endian bytes.
fn push_be(out: &mut Vec<u8>, value: u64, width: usize) {
    out.extend_from_slice(&value.to_be_bytes()[8 - width..]);
}

/// Append an incremental update to an existing PDF (§7.5.6): keep `original` byte-for-byte, then
/// append only the `changed`/new objects, a cross-reference section listing just them, and a
/// trailer whose `/Prev` chains back to the previous section.
///
/// `size` is the new `/Size` (one past the highest object number in the whole document). A reader
/// that follows `/Prev` (newest-wins) sees the appended objects override the originals.
#[must_use]
pub fn write_incremental(
    original: &[u8],
    changed: &[(ObjectId, Object)],
    root: ObjectId,
    info: Option<ObjectId>,
    size: u64,
) -> Vec<u8> {
    let mut out = original.to_vec();
    if !out.ends_with(b"\n") {
        out.push(b'\n'); // keep the appended revision on its own line
    }

    // Append each changed object body, recording (number, offset, generation).
    let mut sorted: Vec<&(ObjectId, Object)> = changed.iter().collect();
    sorted.sort_by_key(|(id, _)| id.number);
    let mut located: Vec<(u32, usize, u16)> = Vec::with_capacity(sorted.len());
    for (id, object) in &sorted {
        let offset = out.len();
        out.extend_from_slice(format!("{} {} obj\n", id.number, id.generation).as_bytes());
        serialize_object(&mut out, object);
        out.extend_from_slice(b"\nendobj\n");
        located.push((id.number, offset, id.generation));
    }

    append_xref_and_trailer(&mut out, &located, original, root, info, size, "");
    out
}

/// Append an updated revision that carries a **digital signature** (§12.8): like
/// [`write_incremental`], but the signature value object is appended from `signature.1` verbatim —
/// preserving its hex `/Contents` placeholder and fixed-width `/ByteRange` so the caller can patch
/// them after layout. `signature.0` is that object's id (it joins `changed` in the cross-reference).
#[must_use]
pub fn write_incremental_signed(
    original: &[u8],
    changed: &[(ObjectId, Object)],
    signature: (ObjectId, &[u8]),
    root: ObjectId,
    info: Option<ObjectId>,
    size: u64,
) -> Vec<u8> {
    write_incremental_signed_inner(original, changed, signature, root, info, size, "")
}

/// As [`write_incremental_signed`], but the new revision's trailer additionally carries the
/// verbatim `trailer_extra` fragment — used to attach an `/AuthCode` dictionary (ISO/TS 32004
/// §5.2.3, `/MACLocation /AttachedToSig`) alongside the signature.
#[must_use]
pub fn write_incremental_signed_with_trailer(
    original: &[u8],
    changed: &[(ObjectId, Object)],
    signature: (ObjectId, &[u8]),
    root: ObjectId,
    info: Option<ObjectId>,
    size: u64,
    trailer_extra: &str,
) -> Vec<u8> {
    write_incremental_signed_inner(
        original,
        changed,
        signature,
        root,
        info,
        size,
        trailer_extra,
    )
}

fn write_incremental_signed_inner(
    original: &[u8],
    changed: &[(ObjectId, Object)],
    signature: (ObjectId, &[u8]),
    root: ObjectId,
    info: Option<ObjectId>,
    size: u64,
    trailer_extra: &str,
) -> Vec<u8> {
    let mut out = original.to_vec();
    if !out.ends_with(b"\n") {
        out.push(b'\n');
    }

    // Pre-serialize every appended object; the signature object keeps its caller-provided body so
    // its hex /Contents and fixed-width /ByteRange survive byte-for-byte.
    let mut bodies: Vec<(ObjectId, Vec<u8>)> = changed
        .iter()
        .map(|(id, object)| {
            let mut buf = Vec::new();
            serialize_object(&mut buf, object);
            (*id, buf)
        })
        .collect();
    bodies.push((signature.0, signature.1.to_vec()));
    bodies.sort_by_key(|(id, _)| id.number);

    let mut located: Vec<(u32, usize, u16)> = Vec::with_capacity(bodies.len());
    for (id, body) in &bodies {
        let offset = out.len();
        out.extend_from_slice(format!("{} {} obj\n", id.number, id.generation).as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
        located.push((id.number, offset, id.generation));
    }

    append_xref_and_trailer(
        &mut out,
        &located,
        original,
        root,
        info,
        size,
        trailer_extra,
    );
    out
}

/// Append the cross-reference section (§7.5.4, one subsection per contiguous run of object numbers)
/// and the trailer (§7.5.5/§7.5.6, with `/Prev` to the previous revision) for an incremental update.
fn append_xref_and_trailer(
    out: &mut Vec<u8>,
    located: &[(u32, usize, u16)],
    original: &[u8],
    root: ObjectId,
    info: Option<ObjectId>,
    size: u64,
    trailer_extra: &str,
) {
    let startxref = out.len();
    out.extend_from_slice(b"xref\n");
    let mut i = 0;
    while i < located.len() {
        let mut j = i;
        while j + 1 < located.len() && located[j + 1].0 == located[j].0 + 1 {
            j += 1;
        }
        out.extend_from_slice(format!("{} {}\n", located[i].0, j - i + 1).as_bytes());
        for &(_, offset, generation) in &located[i..=j] {
            out.extend_from_slice(format!("{offset:010} {generation:05} n \n").as_bytes());
        }
        i = j + 1;
    }

    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {size} /Root {} {} R",
            root.number, root.generation
        )
        .as_bytes(),
    );
    if let Some(prev) = find_prev_startxref(original) {
        out.extend_from_slice(format!(" /Prev {prev}").as_bytes());
    }
    if let Some(info) = info {
        out.extend_from_slice(format!(" /Info {} {} R", info.number, info.generation).as_bytes());
    }
    // Carry the original trailer's /ID forward (§7.5.6): an incremental update keeps the same file
    // identifier, and PDF/A (ISO 19005, clause 6.1.3) requires every trailer to have an /ID.
    // Keeping it *unchanged* is load-bearing for encrypted files as well: Table 15 NOTE 3 records
    // that the /ID strings feed the encryption algorithm, so a regenerated /ID would change the
    // derived file key and orphan every object already written.
    if let Some(id) = find_trailer_id(original) {
        out.extend_from_slice(b" /ID ");
        out.extend_from_slice(id);
    }
    // Carry /Encrypt forward too. ISO 32000-2 §7.5.6: "The added trailer shall contain all the
    // entries except the Prev entry (if present) from the previous trailer, whether modified or
    // not." A reader takes the newest trailer as authoritative, so dropping /Encrypt leaves a file
    // that declares itself unencrypted while its body objects are still ciphertext — unreadable in
    // any conforming reader, not merely in ours. `trailer_extra` is emitted after this, and a
    // caller that supplies its own /Encrypt there would duplicate the key, so it must not.
    if !trailer_extra.contains("/Encrypt") {
        if let Some(encrypt) = find_trailer_encrypt(original) {
            out.extend_from_slice(b" /Encrypt ");
            out.extend_from_slice(encrypt);
        }
    }
    out.extend_from_slice(trailer_extra.as_bytes());
    out.extend_from_slice(b" >>\n");
    out.extend_from_slice(format!("startxref\n{startxref}\n%%EOF\n").as_bytes());
}

/// The `/ID` array of the original's most recent trailer, for an incremental update to reuse.
///
/// Finds the last `/ID` key — in a classic trailer or an xref-stream dictionary — and returns its
/// array verbatim, brackets included, so the appended revision keeps the same file identity
/// (§7.5.6/§14.4). The two identifiers may be written either way (§7.3.4): hex `<…>` as Prism PDF
/// writes them, or a literal `(…)` as other producers do. A literal identifier is arbitrary binary
/// and may well contain a `]` or an escaped `)`, so the scan honours PDF string syntax rather than
/// stopping at the first `]`. Returns `None` if no well-formed `/ID` array is found.
fn find_trailer_id(original: &[u8]) -> Option<&[u8]> {
    // Walk the `/ID` occurrences from the end: the newest revision's trailer wins (§7.5.6). Not
    // every hit is the trailer key — `/IDTree` (§14.7.4.5) starts the same way, and a struct
    // element's own `/ID` is a string, not an array — so each candidate is checked before use.
    let mut search = original.len();
    while let Some(pos) = original[..search].windows(3).rposition(|w| w == b"/ID") {
        search = pos + 2; // next round looks strictly further left
        // `rposition` consumes from the back and stops on the first hit, so the scans cover disjoint
        // stretches and the whole walk stays linear in the file size even on hostile input.
        let rest = &original[pos + 3..];
        let window = &rest[..rest.len().min(MAX_FILE_ID_ARRAY)];
        if let Some(array) = trailer_id_array(window) {
            return Some(array);
        }
    }
    None
}

/// The `/Encrypt` value of the newest trailer that has one — the `n g R` reference tokens, ready to
/// re-emit verbatim (§7.5.6, carrying the previous trailer's entries into the added one).
///
/// Walks `/Encrypt` occurrences from the end so the newest revision wins, and checks each candidate
/// is really the key (not a longer name such as `/EncryptMetadata`) with an indirect-reference
/// value — which is what `/Encrypt` always is in a file we can incrementally update, since the
/// encryption dictionary has to be an object the xref can point at.
fn find_trailer_encrypt(original: &[u8]) -> Option<&[u8]> {
    let mut search = original.len();
    while let Some(pos) = original[..search]
        .windows(8)
        .rposition(|w| w == b"/Encrypt")
    {
        search = pos + 7; // next round looks strictly further left
        let rest = &original[pos + 8..];
        let window = &rest[..rest.len().min(MAX_ENCRYPT_REFERENCE)];
        if let Some(reference) = trailer_encrypt_reference(window) {
            return Some(reference);
        }
    }
    None
}

/// How far past an `/Encrypt` key its reference may extend. `4294967295 65535 R` is 18 bytes, so
/// this is generous; its job is to stop a malformed file full of `/Encrypt`-prefixed keys from
/// making the candidate walk quadratic (DESIGN.md §3.4).
const MAX_ENCRYPT_REFERENCE: usize = 64;

/// The `n g R` tokens starting in `rest` (the bytes right after an `/Encrypt` key), or `None` when
/// `rest` does not begin one — a longer name such as `/EncryptMetadata`, or a non-reference value.
fn trailer_encrypt_reference(rest: &[u8]) -> Option<&[u8]> {
    // A name token ends at whitespace or a delimiter (§7.2.3); anything else means the key was
    // really `/Encryptxxx`.
    if !rest.first()?.is_ascii_whitespace() {
        return None;
    }
    let start = rest.iter().position(|b| !b.is_ascii_whitespace())?;
    let mut cursor = start;
    // number, generation, then the literal `R`.
    for _ in 0..2 {
        let digits = rest[cursor..]
            .iter()
            .take_while(|b| b.is_ascii_digit())
            .count();
        if digits == 0 {
            return None;
        }
        cursor += digits;
        let spaces = rest[cursor..]
            .iter()
            .take_while(|b| b.is_ascii_whitespace())
            .count();
        if spaces == 0 {
            return None;
        }
        cursor += spaces;
    }
    if rest.get(cursor)? != &b'R' {
        return None;
    }
    Some(&rest[start..=cursor])
}

/// How far past a `/ID` key the array is allowed to extend. Two file identifiers are ~70 bytes
/// written out, so this is generous — its job is to keep a *malformed* file (many `/ID`-prefixed
/// keys, none of them a closed array) from turning the candidate walk quadratic (DESIGN.md §3.4).
const MAX_FILE_ID_ARRAY: usize = 4096;

/// The `/ID` array starting in `rest` (the bytes right after a `/ID` key), or `None` when `rest`
/// does not begin one — a longer name such as `/IDTree`, or a value that is not an array.
fn trailer_id_array(rest: &[u8]) -> Option<&[u8]> {
    // A name token ends at whitespace or a delimiter (§7.2.3); anything else means the key was
    // really `/IDxxx`, not `/ID`.
    let first = *rest.first()?;
    if !(first.is_ascii_whitespace() || matches!(first, b'[' | b'(' | b'<' | b'/' | b'%')) {
        return None;
    }
    let open = rest.iter().position(|&b| !b.is_ascii_whitespace())?;
    if rest[open] != b'[' {
        return None;
    }

    let mut i = open + 1;
    while i < rest.len() {
        match rest[i] {
            b']' => return Some(&rest[open..=i]),
            b'<' => i = skip_hex_string(rest, i)?,
            b'(' => i = skip_literal_string(rest, i)?,
            _ => i += 1,
        }
    }
    None
}

/// Index just past the hex string (§7.3.4.3) opening at `start`, or `None` if it is unterminated.
fn skip_hex_string(bytes: &[u8], start: usize) -> Option<usize> {
    let close = bytes[start + 1..].iter().position(|&b| b == b'>')?;
    Some(start + 1 + close + 1)
}

/// Index just past the literal string (§7.3.4.2) opening at `start`, or `None` if it is
/// unterminated. Honours `\` escapes and the balanced-parenthesis rule, so a `)` or `]` inside the
/// identifier's bytes does not end it early.
fn skip_literal_string(bytes: &[u8], start: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut i = start;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2, // an escape consumes the next byte, whatever it is
            b'(' => {
                depth += 1;
                i += 1;
            }
            b')' => {
                depth = depth.checked_sub(1)?;
                i += 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => i += 1,
        }
    }
    None
}

/// Read the byte offset named by the last `startxref` in `original` (§7.5.5), for the new
/// trailer's `/Prev`.
fn find_prev_startxref(original: &[u8]) -> Option<u64> {
    let keyword = b"startxref";
    let pos = original
        .windows(keyword.len())
        .rposition(|w| w == keyword)?;
    let rest = &original[pos + keyword.len()..];
    let start = rest.iter().position(|b| b.is_ascii_digit())?;
    let len = rest[start..]
        .iter()
        .take_while(|b| b.is_ascii_digit())
        .count();
    std::str::from_utf8(&rest[start..start + len])
        .ok()?
        .parse()
        .ok()
}

#[cfg(test)]
mod tests;
