//! Unit tests for the document model (EPIC 4, ISO 32000 §7.7), split by theme.
//!
//! These are unit tests (a submodule of the crate) rather than integration tests because they
//! reach crate-private internals — e.g. the private [`Document::xref`] field (the encryption tests
//! inspect the trailer's `/Encrypt` directly). Descendant modules can see private items, so each
//! themed submodule pulls them in via `use super::super::*`.

mod annotations;
mod content;
mod encryption;
mod flatten;
mod forms;
mod metadata;
mod names;
mod open;
mod outlines;
mod save;
mod version;

/// A minimal classic-xref PDF with a 2-level page tree (root + 3 leaf pages).
fn classic_three_page_pdf() -> Vec<u8> {
    let mut buf = Vec::new();
    let mut off = [0usize; 6];
    buf.extend_from_slice(b"%PDF-1.7\n");
    let objects: [&[u8]; 5] = [
        b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n",
        b"2 0 obj\n<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 >>\nendobj\n",
        b"3 0 obj\n<< /Type /Page /Parent 2 0 R >>\nendobj\n",
        b"4 0 obj\n<< /Type /Page /Parent 2 0 R >>\nendobj\n",
        b"5 0 obj\n<< /Type /Page /Parent 2 0 R >>\nendobj\n",
    ];
    for (i, body) in objects.iter().enumerate() {
        off[i + 1] = buf.len();
        buf.extend_from_slice(body);
    }
    let startxref = buf.len();
    buf.extend_from_slice(b"xref\n0 6\n0000000000 65535 f \n");
    for entry in &off[1..] {
        buf.extend_from_slice(format!("{entry:010} 00000 n \n").as_bytes());
    }
    buf.extend_from_slice(b"trailer\n<< /Size 6 /Root 1 0 R >>\n");
    buf.extend_from_slice(format!("startxref\n{startxref}\n%%EOF\n").as_bytes());
    buf
}

/// Assemble a classic PDF from object bodies with optional extra trailer entries.
fn assemble(objects: &[Vec<u8>], trailer_extra: &str) -> Vec<u8> {
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

fn from_base64(s: &str) -> Vec<u8> {
    let val = |c: u8| match c {
        b'A'..=b'Z' => (c - b'A') as i32,
        b'a'..=b'z' => (c - b'a' + 26) as i32,
        b'0'..=b'9' => (c - b'0' + 52) as i32,
        b'+' => 62,
        b'/' => 63,
        _ => -1,
    };
    let (mut acc, mut bits, mut out) = (0i32, 0, Vec::new());
    for &c in s.as_bytes() {
        let v = val(c);
        if v < 0 {
            continue;
        }
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    out
}
