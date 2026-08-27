//! Shared helpers for the `prismpdf` facade integration tests.
//!
//! These build minimal, byte-exact PDFs from object bodies so the facade's public API can be
//! exercised end-to-end (open → read). Each integration test binary compiles this module in full,
//! so unused helpers are expected per-file.
#![allow(dead_code)]

use std::collections::BTreeMap;

use prismpdf::Document;
use prismpdf::cos::{Dictionary, Name, Object};

/// Assemble a classic-xref PDF from object bodies (object `i+1` ← `objects[i]`), with optional
/// extra trailer entries. Offsets are computed so the file is valid.
pub fn assemble(objects: &[Vec<u8>], trailer_extra: &str) -> Vec<u8> {
    let mut buf = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in objects.iter().enumerate() {
        offsets.push(buf.len());
        buf.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        buf.extend_from_slice(body);
        buf.extend_from_slice(b"\nendobj\n");
    }
    let startxref = buf.len();
    buf.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
    );
    for off in &offsets {
        buf.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
    }
    buf.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R {trailer_extra} >>\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    buf.extend_from_slice(format!("startxref\n{startxref}\n%%EOF\n").as_bytes());
    buf
}

/// Wrap content as an unfiltered stream object body with a correct `/Length`.
pub fn stream_obj(content: &[u8]) -> Vec<u8> {
    let mut body = format!("<< /Length {} >>\nstream\n", content.len()).into_bytes();
    body.extend_from_slice(content);
    body.extend_from_slice(b"\nendstream");
    body
}

/// Wrap `content` as an unfiltered stream object body with extra dict entries.
pub fn stream_with(extra: &str, content: &[u8]) -> Vec<u8> {
    let mut body = format!("<< {extra} /Length {} >>\nstream\n", content.len()).into_bytes();
    body.extend_from_slice(content);
    body.extend_from_slice(b"\nendstream");
    body
}

/// Assert `after` preserves every live object of `before` (M11 round-trip fidelity, §7.5), up to
/// the only normalisation a full rewrite is allowed to apply: collapsing an indirect stream
/// `/Length` to a direct integer (§7.3.8). Object numbers, stream bodies, and all other content
/// must match exactly. `ctx` labels the failure.
pub fn assert_objects_preserved(before: &Document, after: &Document, ctx: &str) {
    let a = live_objects_map(before);
    let b = live_objects_map(after);
    let a_keys: Vec<u32> = a.keys().copied().collect();
    let b_keys: Vec<u32> = b.keys().copied().collect();
    assert_eq!(a_keys, b_keys, "{ctx}: live object number set changed");
    for (number, value) in &a {
        assert!(
            objects_equivalent(value, &b[number]),
            "{ctx}: object {number} changed value across the rewrite\n  before: {value:?}\n  after:  {:?}",
            b[number],
        );
    }
}

/// A document's live objects as a `number → value` map.
fn live_objects_map(doc: &Document) -> BTreeMap<u32, Object> {
    doc.live_objects()
        .unwrap()
        .into_iter()
        .map(|(id, obj)| (id.number, obj))
        .collect()
}

/// Two objects are equal up to the stream-`/Length` normalisation: a stream matches if its body is
/// byte-identical and its dictionary agrees on every entry except `/Length`; anything else is
/// compared exactly.
fn objects_equivalent(a: &Object, b: &Object) -> bool {
    match (a, b) {
        (Object::Stream(sa), Object::Stream(sb)) => {
            sa.raw() == sb.raw() && dict_equivalent_ignoring_length(sa.dict(), sb.dict())
        }
        _ => a == b,
    }
}

/// Dictionary equality ignoring the `/Length` key.
fn dict_equivalent_ignoring_length(a: &Dictionary, b: &Dictionary) -> bool {
    let length = Name::from("Length");
    let non_length = |d: &Dictionary| d.iter().filter(|(k, _)| *k != &length).count();
    non_length(a) == non_length(b)
        && a.iter()
            .filter(|(k, _)| *k != &length)
            .all(|(k, v)| b.get(k) == Some(v))
}

/// Hex-decode ignoring whitespace (test fixtures only).
pub fn unhex(s: &str) -> Vec<u8> {
    let h: Vec<u8> = s.bytes().filter(u8::is_ascii_hexdigit).collect();
    h.as_chunks::<2>()
        .0
        .iter()
        .map(|&[hi, lo]| {
            let hi = (hi as char).to_digit(16).unwrap();
            let lo = (lo as char).to_digit(16).unwrap();
            (hi * 16 + lo) as u8
        })
        .collect()
}

/// A minimal three-object document (catalog, page tree, one empty page).
pub fn minimal_doc() -> Document {
    let objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R >>".to_vec(),
    ];
    Document::open(assemble(&objects, "")).unwrap()
}
