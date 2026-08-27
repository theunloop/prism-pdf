//! Page content-stream decoding tests (§7.8.2).

use super::super::*;
use super::{assemble, classic_three_page_pdf};

#[test]
fn decodes_page_content_streams() {
    // §7.8.2: a page's /Contents (here an unfiltered stream) is returned decoded.
    let mut buf = Vec::new();
    let mut off = [0usize; 5];
    buf.extend_from_slice(b"%PDF-1.7\n");
    let content = b"BT (hi) Tj ET";
    let obj4 = format!(
        "4 0 obj\n<< /Length {} >>\nstream\n{}\nendstream\nendobj\n",
        content.len(),
        std::str::from_utf8(content).unwrap()
    );
    let objects: [Vec<u8>; 4] = [
        b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n".to_vec(),
        b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n".to_vec(),
        b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>\nendobj\n".to_vec(),
        obj4.into_bytes(),
    ];
    for (i, body) in objects.iter().enumerate() {
        off[i + 1] = buf.len();
        buf.extend_from_slice(body);
    }
    let startxref = buf.len();
    buf.extend_from_slice(b"xref\n0 5\n0000000000 65535 f \n");
    for entry in &off[1..] {
        buf.extend_from_slice(format!("{entry:010} 00000 n \n").as_bytes());
    }
    buf.extend_from_slice(b"trailer\n<< /Size 5 /Root 1 0 R >>\n");
    buf.extend_from_slice(format!("startxref\n{startxref}\n%%EOF\n").as_bytes());

    let doc = Document::open(buf).unwrap();
    let page = doc.pages().unwrap().remove(0);
    assert_eq!(doc.page_content_bytes(&page).unwrap(), content);
}

#[test]
fn content_streams_concatenate_array_form() {
    // /Contents as an array of two streams is concatenated with a separating newline (§7.8.2).
    let objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Contents [4 0 R 5 0 R] >>".to_vec(),
        b"<< /Length 2 >>\nstream\nAB\nendstream".to_vec(),
        b"<< /Length 2 >>\nstream\nCD\nendstream".to_vec(),
    ];
    let doc = Document::open(assemble(&objects, "")).unwrap();
    let page = doc.pages().unwrap().remove(0);
    assert_eq!(doc.page_content_bytes(&page).unwrap(), b"AB\nCD");
}

#[test]
fn page_with_no_contents_is_empty() {
    let doc = Document::open(classic_three_page_pdf()).unwrap();
    let page = doc.pages().unwrap().remove(0);
    assert!(doc.page_content_bytes(&page).unwrap().is_empty());
}
