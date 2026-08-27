//! Unit tests for the cross-reference layer (§7.5), grouped by theme.
//!
//! These live in a submodule folder rather than inline in `xref.rs` to keep the module readable,
//! while still reaching `xref` internals: a descendant module can use private items of its
//! ancestors, so the themed files import them via `use crate::xref::*;`.

use pdf_cos::{Dictionary, Name, Object, Stream};

mod crypt;
mod objstm;
mod recovery;
mod stream;
mod table;

/// Build a minimal but real one-page PDF, returning the bytes and the byte offset of each
/// object so the test can assert the xref table is read correctly.
fn build_pdf() -> Vec<u8> {
    let mut buf = Vec::new();
    let mut off = [0usize; 4];
    buf.extend_from_slice(b"%PDF-1.7\n");
    off[1] = buf.len();
    buf.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    off[2] = buf.len();
    buf.extend_from_slice(b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n");
    off[3] = buf.len();
    buf.extend_from_slice(
        b"3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>\nendobj\n",
    );
    let startxref = buf.len();
    buf.extend_from_slice(b"xref\n0 4\n");
    buf.extend_from_slice(b"0000000000 65535 f \n");
    for entry in &off[1..] {
        buf.extend_from_slice(format!("{entry:010} 00000 n \n").as_bytes());
    }
    buf.extend_from_slice(b"trailer\n<< /Size 4 /Root 1 0 R >>\n");
    buf.extend_from_slice(format!("startxref\n{startxref}\n%%EOF\n").as_bytes());
    buf
}

/// Zlib-compress `data` (for building Flate-encoded stream fixtures).
fn flate(data: &[u8]) -> Vec<u8> {
    use flate2::{Compression, write::ZlibEncoder};
    use std::io::Write;
    let mut e = ZlibEncoder::new(Vec::new(), Compression::default());
    e.write_all(data).unwrap();
    e.finish().unwrap()
}

/// Build a modern PDF whose objects 1–3 live in an object stream (§7.5.7, object 5) indexed by
/// a cross-reference stream (§7.5.8, object 6) — the shape almost every recent PDF uses.
fn build_pdf_with_streams() -> Vec<u8> {
    // Object-stream payload: a header of (objnum, relative-offset) pairs, then the bodies.
    let (b1, b2, b3) = (
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>",
    );
    let (o1, o2, o3) = (0, b1.len() + 1, b1.len() + 1 + b2.len() + 1);
    let header = format!("1 {o1} 2 {o2} 3 {o3} ");
    let first = header.len();
    let objstm = flate(format!("{header}{b1} {b2} {b3}").as_bytes());

    let mut buf = Vec::new();
    buf.extend_from_slice(b"%PDF-1.5\n");

    let off5 = buf.len();
    buf.extend_from_slice(
        format!(
            "5 0 obj\n<< /Type /ObjStm /N 3 /First {first} /Length {} /Filter /FlateDecode >>\nstream\n",
            objstm.len()
        )
        .as_bytes(),
    );
    buf.extend_from_slice(&objstm);
    buf.extend_from_slice(b"\nendstream\nendobj\n");

    // The xref stream's own offset is known here (before it is written), so its self-entry is
    // not circular.
    let off6 = buf.len();
    let entries: [(u8, u64, u64); 7] = [
        (0, 0, 0),           // 0: free head
        (2, 5, 0),           // 1: compressed in obj-stream 5, index 0
        (2, 5, 1),           // 2
        (2, 5, 2),           // 3
        (0, 0, 0),           // 4: unused
        (1, off5 as u64, 0), // 5: the object stream itself
        (1, off6 as u64, 0), // 6: this xref stream
    ];
    let mut data = Vec::new();
    for (kind, f1, f2) in entries {
        data.push(kind); // W[0] = 1
        data.extend_from_slice(&(f1 as u16).to_be_bytes()); // W[1] = 2
        data.push(f2 as u8); // W[2] = 1
    }
    let xref = flate(&data);
    buf.extend_from_slice(
        format!(
            "6 0 obj\n<< /Type /XRef /Size 7 /Root 1 0 R /W [1 2 1] /Length {} /Filter /FlateDecode >>\nstream\n",
            xref.len()
        )
        .as_bytes(),
    );
    buf.extend_from_slice(&xref);
    buf.extend_from_slice(b"\nendstream\nendobj\n");
    buf.extend_from_slice(format!("startxref\n{off6}\n%%EOF\n").as_bytes());
    buf
}

/// Build an unfiltered cross-reference stream dictionary + data for direct unit testing.
fn xref_stream(w: &[i64], entries: &[(u8, u64, u64)]) -> (Dictionary, Stream) {
    let mut data = Vec::new();
    for &(t, f1, f2) in entries {
        if w[0] > 0 {
            data.push(t);
        }
        data.extend_from_slice(&(f1 as u16).to_be_bytes()[2 - w[1] as usize..]);
        if w[2] > 0 {
            data.push(f2 as u8);
        }
    }
    let mut dict = Dictionary::new();
    dict.insert(Name::from("Type"), Object::Name(Name::from("XRef")));
    dict.insert(Name::from("Size"), Object::Integer(entries.len() as i64));
    dict.insert(
        Name::from("W"),
        Object::Array(
            w.iter()
                .map(|&n| Object::Integer(n))
                .collect::<Vec<_>>()
                .into(),
        ),
    );
    let stream = Stream::new(dict.clone(), data);
    (dict, stream)
}

/// A trivial reversible "cipher" for decryption tests: XOR every byte with 0xFF. Always succeeds.
fn xor_decrypt(_n: u32, _g: u16, data: &[u8]) -> Option<Vec<u8>> {
    Some(data.iter().map(|b| b ^ 0xFF).collect())
}

/// A decryptor that always fails — the shape of an AES-GCM authentication-tag mismatch.
fn failing_decrypt(_n: u32, _g: u16, _data: &[u8]) -> Option<Vec<u8>> {
    None
}

fn crypt_stream(name: Option<&str>) -> Stream {
    let mut dict = Dictionary::new();
    dict.insert(Name::from("Filter"), Object::Name(Name::from("Crypt")));
    if let Some(n) = name {
        let mut parms = Dictionary::new();
        parms.insert(Name::from("Name"), Object::Name(Name::from(n)));
        dict.insert(Name::from("DecodeParms"), Object::Dictionary(parms));
    }
    Stream::new(dict, b"plaintext".to_vec())
}
