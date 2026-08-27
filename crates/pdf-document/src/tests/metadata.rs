//! XMP metadata reading tests (§14.3.2).

use super::super::*;
use super::assemble;

#[test]
fn reads_xmp_metadata_stream() {
    let xmp = b"<?xpacket begin=\"\xEF\xBB\xBF\"?><x:xmpmeta><rdf:RDF>\
        <dc:title><rdf:Alt><rdf:li>Hello</rdf:li></rdf:Alt></dc:title>\
        </rdf:RDF></x:xmpmeta><?xpacket end=\"w\"?>";
    let mut meta = format!(
        "<< /Type /Metadata /Subtype /XML /Length {} >>\nstream\n",
        xmp.len()
    )
    .into_bytes();
    meta.extend_from_slice(xmp);
    meta.extend_from_slice(b"\nendstream");

    let objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R /Metadata 4 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R >>".to_vec(),
        meta,
    ];
    let doc = Document::open(assemble(&objects, "")).unwrap();
    let packet = doc.xmp_metadata().unwrap().expect("has XMP");
    assert!(packet.contains("<dc:title>"));
    assert!(packet.contains("Hello"));
}

#[test]
fn no_metadata_yields_none() {
    let objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R >>".to_vec(),
    ];
    let doc = Document::open(assemble(&objects, "")).unwrap();
    assert!(doc.xmp_metadata().unwrap().is_none());
}

#[test]
fn reads_info_dates() {
    // /Info CreationDate/ModDate are parsed as §7.9.4 date strings; a malformed or missing
    // date is best-effort None.
    let objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R >>".to_vec(),
        b"<< /CreationDate (D:20260817143005+02'00') /ModDate (garbage) >>".to_vec(),
    ];
    let doc = Document::open(assemble(&objects, "/Info 4 0 R")).unwrap();
    let created = doc.creation_date().unwrap().expect("parsable date");
    assert_eq!((created.year, created.month, created.day), (2026, 8, 17));
    assert_eq!((created.hour, created.minute, created.second), (14, 30, 5));
    assert_eq!(created.utc_offset_minutes, Some(120));
    assert_eq!(doc.modification_date().unwrap(), None);
}

#[test]
fn missing_info_yields_no_dates() {
    let objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R >>".to_vec(),
    ];
    let doc = Document::open(assemble(&objects, "")).unwrap();
    assert_eq!(doc.creation_date().unwrap(), None);
    assert_eq!(doc.modification_date().unwrap(), None);
}
