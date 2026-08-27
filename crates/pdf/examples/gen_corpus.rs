//! Generate the versioned test corpus under `corpus/` (see `corpus/README.md`).
//!
//! These are byte-level fixtures (not authored through the writer) so each one exercises a
//! *specific* reader path — classic xref tables, cross-reference streams, object streams, indirect
//! `/Length`, and the recovery cases the design treats as first-class (§7.5, DESIGN.md §7). The
//! `tests/corpus.rs` round-trip test runs `load → save → load` over everything here.
//!
//! Run from the workspace root:
//!
//! ```text
//! cargo run -p prismpdf --example gen_corpus
//! ```
//!
//! It writes into `corpus/{valid,edge,malformed}/` relative to the workspace, overwriting the
//! generated files. The set is deterministic, so re-running it produces byte-identical output.
#![allow(clippy::expect_used, clippy::unwrap_used)] // a corpus generator may panic on I/O error.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use flate2::Compression;
use flate2::write::ZlibEncoder;

fn main() -> std::io::Result<()> {
    // Default to the workspace `corpus/`; allow an override as argv[1] for ad-hoc runs.
    let root = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus"));

    // --- valid/ : well-formed; must parse strictly and survive a round-trip unchanged. ---
    write(&root, "valid/text-classic-xref.pdf", text_classic())?;
    write(&root, "valid/two-pages-text.pdf", two_pages())?;
    write(&root, "valid/flate-content.pdf", flate_content())?;
    write(&root, "valid/xref-stream.pdf", xref_stream_doc())?;
    write(&root, "valid/objstm.pdf", objstm_doc())?;
    write(&root, "valid/lzw-content.pdf", lzw_content())?;
    write(
        &root,
        "valid/ascii-chain-content.pdf",
        ascii_chain_content(),
    )?;
    write(&root, "valid/runlength-image.pdf", runlength_image())?;
    write(&root, "valid/ccitt-image.pdf", ccitt_image())?;

    // --- edge/ : legal but unusual; stresses model decisions and parsing corners. ---
    write(&root, "edge/length-indirect.pdf", length_indirect())?;
    write(&root, "edge/nested-page-tree.pdf", nested_page_tree())?;
    write(&root, "edge/leading-comments.pdf", leading_comments())?;

    // --- malformed/ : broken; recovery (xref rebuild / header scan) must open them, never panic. ---
    write(
        &root,
        "malformed/missing-startxref.pdf",
        missing_startxref(),
    )?;
    write(&root, "malformed/bad-startxref.pdf", bad_startxref())?;
    write(&root, "malformed/wrong-length.pdf", wrong_length())?;
    write(
        &root,
        "malformed/truncated-trailer.pdf",
        truncated_trailer(),
    )?;
    write(&root, "malformed/garbage-prefix.pdf", garbage_prefix())?;

    Ok(())
}

fn write(root: &Path, rel: &str, bytes: Vec<u8>) -> std::io::Result<()> {
    let path = root.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, &bytes)?;
    println!("wrote {} ({} bytes)", path.display(), bytes.len());
    Ok(())
}

// --- byte-level builders -------------------------------------------------------------------------

/// Wrap `content` as an unfiltered stream object body with a correct `/Length`. `extra` is injected
/// into the dictionary verbatim (e.g. extra entries or a `/Filter`).
fn stream_body(extra: &str, content: &[u8]) -> Vec<u8> {
    let mut b = format!("<< {extra} /Length {} >>\nstream\n", content.len()).into_bytes();
    b.extend_from_slice(content);
    b.extend_from_slice(b"\nendstream");
    b
}

/// Assemble a classic cross-reference *table* PDF. `objects[i]` becomes object `i+1 0`. `prefix`
/// is emitted before the header (used to test leading-garbage recovery); `trailer_extra` injects
/// extra trailer entries. Offsets are computed from the true start of the file.
fn classic(prefix: &[u8], version: &str, objects: &[Vec<u8>], trailer_extra: &str) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(prefix);
    buf.extend_from_slice(format!("%PDF-{version}\n").as_bytes());
    buf.extend_from_slice(b"%\xE2\xE3\xCF\xD3\n"); // binary marker (§7.5.2)

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

/// A standard-14 font object body (no embedding needed — present in every conformant viewer).
const HELVETICA: &[u8] = b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>";

/// A renderable text content stream: selects font `/F1`, positions the cursor near the top of a
/// US-Letter page, and shows `text`. Real viewers need the `Tf` + `Td`; without them text is blank
/// even though extraction would still recover the operand.
fn show_text(text: &str) -> String {
    format!("BT /F1 24 Tf 72 720 Td ({text}) Tj ET")
}

/// Object bodies for a one-page document that actually *renders* `text` (Helvetica, positioned).
/// `text` must be ASCII without parentheses. Returns five bodies: catalog, pages, page (with a
/// `/Font` resource), content stream, font.
fn one_page_objects(text: &str) -> Vec<Vec<u8>> {
    vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R \
          /Resources << /Font << /F1 5 0 R >> >> >>"
            .to_vec(),
        stream_body("", show_text(text).as_bytes()),
        HELVETICA.to_vec(),
    ]
}

fn text_classic() -> Vec<u8> {
    classic(b"", "1.7", &one_page_objects("Hello classic xref"), "")
}

fn two_pages() -> Vec<u8> {
    // Both pages share the one Helvetica font (object 7).
    let objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R 5 0 R] /Count 2 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R \
          /Resources << /Font << /F1 7 0 R >> >> >>"
            .to_vec(),
        stream_body("", show_text("Page one").as_bytes()),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 6 0 R \
          /Resources << /Font << /F1 7 0 R >> >> >>"
            .to_vec(),
        stream_body("", show_text("Page two").as_bytes()),
        HELVETICA.to_vec(),
    ];
    classic(b"", "1.7", &objects, "")
}

fn flate_content() -> Vec<u8> {
    let mut enc = ZlibEncoder::new(Vec::new(), Compression::default());
    enc.write_all(show_text("Compressed hello").as_bytes())
        .unwrap();
    let compressed = enc.finish().unwrap();
    let objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R \
          /Resources << /Font << /F1 5 0 R >> >> >>"
            .to_vec(),
        stream_body("/Filter /FlateDecode", &compressed),
        HELVETICA.to_vec(),
    ];
    classic(b"", "1.7", &objects, "")
}

/// LZW-encode `data` as literal codes only (§7.4.4.2): a `ClearTable` (256), one 9-bit code per
/// input byte — every byte value is its own code in the initial table — and an `EOD` (257). The
/// decoder still grows its string table entry by entry, so this exercises the real state machine;
/// keeping `data` short keeps every code 9 bits wide, before the 10-bit boundary at 511.
fn lzw_literals(data: &[u8]) -> Vec<u8> {
    assert!(
        data.len() < 200,
        "would cross the 9-bit code-width boundary"
    );
    let mut out = Vec::new();
    let (mut acc, mut bits) = (0u32, 0u32);
    let put = |code: u32, out: &mut Vec<u8>, acc: &mut u32, bits: &mut u32| {
        *acc = (*acc << 9) | code;
        *bits += 9;
        while *bits >= 8 {
            *bits -= 8;
            out.push((*acc >> *bits) as u8);
        }
    };
    put(256, &mut out, &mut acc, &mut bits);
    for &b in data {
        put(u32::from(b), &mut out, &mut acc, &mut bits);
    }
    put(257, &mut out, &mut acc, &mut bits);
    if bits > 0 {
        out.push((acc << (8 - bits)) as u8);
    }
    out
}

/// A page whose content stream is `LZWDecode`d (§7.4.4.2) — the filter no seeded corpus file
/// reached before, so the fuzzer never got past `FlateDecode`.
fn lzw_content() -> Vec<u8> {
    let content = show_text("LZW hello");
    let objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R \
          /Resources << /Font << /F1 5 0 R >> >> >>"
            .to_vec(),
        stream_body("/Filter /LZWDecode", &lzw_literals(content.as_bytes())),
        HELVETICA.to_vec(),
    ];
    classic(b"", "1.7", &objects, "")
}

/// ASCII85-encode `data` (§7.4.3): four bytes at a time as five base-85 digits from `!`, `z` for an
/// all-zero group, and the `~>` terminator.
fn ascii85(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    for chunk in data.chunks(4) {
        let mut group = [0u8; 4];
        group[..chunk.len()].copy_from_slice(chunk);
        let mut value = u32::from_be_bytes(group);
        if chunk.len() == 4 && value == 0 {
            out.push(b'z');
            continue;
        }
        let mut digits = [0u8; 5];
        for slot in digits.iter_mut().rev() {
            *slot = b'!' + (value % 85) as u8;
            value /= 85;
        }
        // A partial final group emits one digit more than it has bytes (§7.4.3).
        out.extend_from_slice(&digits[..chunk.len() + 1]);
    }
    out.extend_from_slice(b"~>");
    out
}

/// A page whose content stream runs through a *chain* of filters (§7.4, `/Filter` as an array):
/// `ASCII85Decode` then `FlateDecode`. Exercises the chain walker and the ASCII transport decoder,
/// neither of which any earlier corpus file reached.
fn ascii_chain_content() -> Vec<u8> {
    let mut enc = ZlibEncoder::new(Vec::new(), Compression::default());
    enc.write_all(show_text("ASCII85 over Flate").as_bytes())
        .unwrap();
    let compressed = enc.finish().unwrap();
    let objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R \
          /Resources << /Font << /F1 5 0 R >> >> >>"
            .to_vec(),
        stream_body(
            "/Filter [/ASCII85Decode /FlateDecode]",
            &ascii85(&compressed),
        ),
        HELVETICA.to_vec(),
    ];
    classic(b"", "1.7", &objects, "")
}

/// A one-page document with a `RunLengthDecode` image XObject (§7.4.5): a 4×2 RGB image as one
/// literal run per row plus the EOD marker (128).
fn runlength_image() -> Vec<u8> {
    let row: [u8; 12] = [255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 0];
    let mut encoded = Vec::new();
    for _ in 0..2 {
        encoded.push(row.len() as u8 - 1); // "copy the next n+1 bytes literally"
        encoded.extend_from_slice(&row);
    }
    encoded.push(128); // EOD

    let objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R \
          /Resources << /XObject << /Im0 5 0 R >> >> >>"
            .to_vec(),
        stream_body("", b"q 400 0 0 200 100 500 cm /Im0 Do Q"),
        stream_body(
            "/Type /XObject /Subtype /Image /Width 4 /Height 2 /BitsPerComponent 8 \
             /ColorSpace /DeviceRGB /Filter /RunLengthDecode",
            &encoded,
        ),
    ];
    classic(b"", "1.7", &objects, "")
}

/// A one-page document with a `CCITTFaxDecode` image XObject (§7.4.6): two all-white 64-pixel rows
/// in Group 3 one-dimensional coding (`/K 0`), each a white make-up code for 64 followed by the
/// white terminating code for 0 (ITU-T T.4 tables).
fn ccitt_image() -> Vec<u8> {
    const WHITE_MAKEUP_64: &str = "11011";
    const WHITE_TERM_0: &str = "00110101";
    let bits: String = std::iter::repeat_n(format!("{WHITE_MAKEUP_64}{WHITE_TERM_0}"), 2).collect();
    let mut encoded = Vec::new();
    let (mut cur, mut n) = (0u8, 0u8);
    for bit in bits.bytes() {
        cur = (cur << 1) | (bit - b'0');
        n += 1;
        if n == 8 {
            encoded.push(cur);
            cur = 0;
            n = 0;
        }
    }
    if n > 0 {
        encoded.push(cur << (8 - n));
    }

    let objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R \
          /Resources << /XObject << /Im0 5 0 R >> >> >>"
            .to_vec(),
        stream_body("", b"q 128 0 0 4 100 500 cm /Im0 Do Q"),
        stream_body(
            "/Type /XObject /Subtype /Image /Width 64 /Height 2 /BitsPerComponent 1 \
             /ColorSpace /DeviceGray /Filter /CCITTFaxDecode \
             /DecodeParms << /K 0 /Columns 64 /Rows 2 /EndOfBlock false >>",
            &encoded,
        ),
    ];
    classic(b"", "1.7", &objects, "")
}

/// Big-endian pack `val` into `width` bytes (for cross-reference stream entries, §7.5.8).
fn put_be(out: &mut Vec<u8>, val: u64, width: usize) {
    for i in (0..width).rev() {
        out.push((val >> (8 * i)) as u8);
    }
}

/// Build a PDF whose cross-reference is a *stream* (§7.5.8). `objects[i]` is object `i+1 0`, all
/// uncompressed; the xref stream is appended as the last object and `startxref` points at it.
fn xref_stream_doc() -> Vec<u8> {
    let objects = one_page_objects("Xref stream text");

    let mut buf = b"%PDF-1.5\n%\xE2\xE3\xCF\xD3\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in objects.iter().enumerate() {
        offsets.push(buf.len());
        buf.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        buf.extend_from_slice(body);
        buf.extend_from_slice(b"\nendobj\n");
    }

    let xref_num = objects.len() + 1; // the xref stream's own object number
    let xref_offset = buf.len();

    // Entries for objects 0..=xref_num. W = [1,4,2]: type / field2 / field3.
    let mut data = Vec::new();
    put_be(&mut data, 0, 1); // obj 0: free head
    put_be(&mut data, 0, 4);
    put_be(&mut data, 65535, 2);
    for off in &offsets {
        put_be(&mut data, 1, 1); // type 1: uncompressed
        put_be(&mut data, *off as u64, 4);
        put_be(&mut data, 0, 2);
    }
    put_be(&mut data, 1, 1); // the xref stream itself
    put_be(&mut data, xref_offset as u64, 4);
    put_be(&mut data, 0, 2);

    let size = xref_num + 1;
    let dict = format!(
        "<< /Type /XRef /Size {size} /Root 1 0 R /W [1 4 2] /Length {} >>",
        data.len()
    );
    buf.extend_from_slice(format!("{xref_num} 0 obj\n{dict}\nstream\n").as_bytes());
    buf.extend_from_slice(&data);
    buf.extend_from_slice(b"\nendstream\nendobj\n");
    buf.extend_from_slice(format!("startxref\n{xref_offset}\n%%EOF\n").as_bytes());
    buf
}

/// Build a PDF that packs the catalog/pages/page into an *object stream* (§7.5.7), referenced by a
/// cross-reference stream with type-2 entries. The content stream and containers stay uncompressed.
fn objstm_doc() -> Vec<u8> {
    // Objects 1,2,3 live inside the object stream (obj 5); the page points at the font (obj 6).
    let b1 = b"<< /Type /Catalog /Pages 2 0 R >>".to_vec();
    let b2 = b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec();
    let b3 = b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R \
               /Resources << /Font << /F1 6 0 R >> >> >>"
        .to_vec();

    // Object-stream payload: bodies joined by "\n", with byte offsets relative to /First.
    let off1 = 0;
    let off2 = b1.len() + 1;
    let off3 = off2 + b2.len() + 1;
    let header = format!("1 {off1} 2 {off2} 3 {off3} ");
    let first = header.len();
    let mut payload = header.into_bytes();
    payload.extend_from_slice(&b1);
    payload.push(b'\n');
    payload.extend_from_slice(&b2);
    payload.push(b'\n');
    payload.extend_from_slice(&b3);

    let mut buf = b"%PDF-1.5\n%\xE2\xE3\xCF\xD3\n".to_vec();

    // obj 4: the page content stream (uncompressed).
    let off4 = buf.len();
    buf.extend_from_slice(b"4 0 obj\n");
    buf.extend_from_slice(&stream_body("", show_text("Object stream text").as_bytes()));
    buf.extend_from_slice(b"\nendobj\n");

    // obj 5: the object stream container.
    let off5 = buf.len();
    buf.extend_from_slice(b"5 0 obj\n");
    buf.extend_from_slice(&stream_body(
        &format!("/Type /ObjStm /N 3 /First {first}"),
        &payload,
    ));
    buf.extend_from_slice(b"\nendobj\n");

    // obj 6: the Helvetica font (a stream object can't live in an ObjStm, so it stays here).
    let off6 = buf.len();
    buf.extend_from_slice(b"6 0 obj\n");
    buf.extend_from_slice(HELVETICA);
    buf.extend_from_slice(b"\nendobj\n");

    // obj 7: the cross-reference stream.
    let xref_offset = buf.len();
    let mut data = Vec::new();
    put_be(&mut data, 0, 1); // 0: free head
    put_be(&mut data, 0, 4);
    put_be(&mut data, 65535, 2);
    for index in 0..3u64 {
        put_be(&mut data, 2, 1); // 1,2,3: compressed, in objstm 5
        put_be(&mut data, 5, 4);
        put_be(&mut data, index, 2);
    }
    for off in [off4, off5, off6, xref_offset] {
        put_be(&mut data, 1, 1); // 4: content, 5: objstm, 6: font, 7: the xref stream itself
        put_be(&mut data, off as u64, 4);
        put_be(&mut data, 0, 2);
    }

    let dict = format!(
        "<< /Type /XRef /Size 8 /Root 1 0 R /W [1 4 2] /Length {} >>",
        data.len()
    );
    buf.extend_from_slice(format!("7 0 obj\n{dict}\nstream\n").as_bytes());
    buf.extend_from_slice(&data);
    buf.extend_from_slice(b"\nendstream\nendobj\n");
    buf.extend_from_slice(format!("startxref\n{xref_offset}\n%%EOF\n").as_bytes());
    buf
}

fn length_indirect() -> Vec<u8> {
    let content = show_text("Indirect length");
    let objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R \
          /Resources << /Font << /F1 6 0 R >> >> >>"
            .to_vec(),
        // /Length points at object 5 instead of being a direct integer (§7.3.8.2).
        {
            let mut b = b"<< /Length 5 0 R >>\nstream\n".to_vec();
            b.extend_from_slice(content.as_bytes());
            b.extend_from_slice(b"\nendstream");
            b
        },
        format!("{}", content.len()).into_bytes(),
        HELVETICA.to_vec(),
    ];
    classic(b"", "1.7", &objects, "")
}

fn nested_page_tree() -> Vec<u8> {
    // catalog → Pages(2) → Pages(3, intermediate) → [Page(4), Page(6)]. page_count must be 2.
    // Both leaf pages share the font (object 8).
    let objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 2 >>".to_vec(),
        b"<< /Type /Pages /Parent 2 0 R /Kids [4 0 R 6 0 R] /Count 2 >>".to_vec(),
        b"<< /Type /Page /Parent 3 0 R /MediaBox [0 0 612 792] /Contents 5 0 R \
          /Resources << /Font << /F1 8 0 R >> >> >>"
            .to_vec(),
        stream_body("", show_text("Nested A").as_bytes()),
        b"<< /Type /Page /Parent 3 0 R /MediaBox [0 0 612 792] /Contents 7 0 R \
          /Resources << /Font << /F1 8 0 R >> >> >>"
            .to_vec(),
        stream_body("", show_text("Nested B").as_bytes()),
        HELVETICA.to_vec(),
    ];
    classic(b"", "1.7", &objects, "")
}

fn leading_comments() -> Vec<u8> {
    // Valid file sprinkled with comments. A comment line right after the header and another between
    // objects must be ignored by the lexer (§7.2.4). Offsets still account for every byte.
    let mut buf = b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n% a friendly comment\n".to_vec();
    let objects = one_page_objects("Comments ok");
    let mut offsets = Vec::new();
    for (i, body) in objects.iter().enumerate() {
        buf.extend_from_slice(b"% between objects\n");
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
        format!("trailer\n<< /Size {} /Root 1 0 R >>\n", objects.len() + 1).as_bytes(),
    );
    buf.extend_from_slice(format!("startxref\n{startxref}\n%%EOF\n").as_bytes());
    buf
}

/// Objects with no xref/trailer/startxref at all — recovery must rebuild by scanning for `N G obj`.
fn missing_startxref() -> Vec<u8> {
    let mut buf = b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n".to_vec();
    for (i, body) in one_page_objects("Recovered no xref").iter().enumerate() {
        buf.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        buf.extend_from_slice(body);
        buf.extend_from_slice(b"\nendobj\n");
    }
    buf.extend_from_slice(b"%%EOF\n");
    buf
}

/// A well-formed body but `startxref` points to a bogus offset — recovery must rebuild.
fn bad_startxref() -> Vec<u8> {
    let mut doc = classic(b"", "1.7", &one_page_objects("Recovered bad startxref"), "");
    // Replace the real offset after the last `startxref\n` with a bogus one.
    let marker = b"startxref\n";
    let pos = doc
        .windows(marker.len())
        .rposition(|w| w == marker)
        .expect("startxref present")
        + marker.len();
    let end = pos + doc[pos..].iter().position(|&b| b == b'\n').unwrap();
    let mut patched = doc[..pos].to_vec();
    patched.extend_from_slice(b"999999"); // points into nowhere
    patched.extend_from_slice(&doc[end..]);
    doc = patched;
    doc
}

/// A content stream whose `/Length` is far too small — recovery must scan to `endstream`.
fn wrong_length() -> Vec<u8> {
    let content = show_text("Wrong length recovered");
    let objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R \
          /Resources << /Font << /F1 5 0 R >> >> >>"
            .to_vec(),
        {
            // Claim /Length 2 (way short); the real content runs to `endstream`.
            let mut b = b"<< /Length 2 >>\nstream\n".to_vec();
            b.extend_from_slice(content.as_bytes());
            b.extend_from_slice(b"\nendstream");
            b
        },
        HELVETICA.to_vec(),
    ];
    classic(b"", "1.7", &objects, "")
}

/// Objects then a half-written xref section, cut off before the trailer/`startxref`/`%%EOF`.
fn truncated_trailer() -> Vec<u8> {
    let mut buf = b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n".to_vec();
    for (i, body) in one_page_objects("Truncated trailer").iter().enumerate() {
        buf.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        buf.extend_from_slice(body);
        buf.extend_from_slice(b"\nendobj\n");
    }
    // Start an xref section then stop abruptly.
    buf.extend_from_slice(b"xref\n0 5\n0000000000 65535 f \n0000000015 00000 n ");
    buf
}

/// A perfectly valid document preceded by junk bytes; the header isn't at offset 0 (common with
/// HTTP preambles / concatenated data). Offsets include the prefix, so only header-finding is odd.
fn garbage_prefix() -> Vec<u8> {
    classic(
        b"GARBAGE not-a-pdf \x00\x01\x02 leading bytes\n",
        "1.7",
        &one_page_objects("Garbage prefix"),
        "",
    )
}
