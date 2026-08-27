//! Cross-reference streams (§7.5.8): binary entry decoding and `/W` validation.

use super::{build_pdf_with_streams, xref_stream};
use crate::error::ErrorKind;
use crate::xref::*;

use pdf_cos::{Name, Object, ObjectId};

#[test]
fn reads_cross_reference_stream() {
    // §7.5.8: header, /Root and entries come from the xref stream's dictionary and binary body.
    let pdf = build_pdf_with_streams();
    let xref = XRef::parse(&pdf).unwrap();
    assert_eq!(xref.version, Some(Version { major: 1, minor: 5 }));
    assert_eq!(xref.root(), Some(ObjectId::new(1, 0)));
    assert!(matches!(xref.entry(5), Some(XRefEntry::InUse { .. })));
    assert_eq!(
        xref.entry(1),
        Some(XRefEntry::Compressed {
            container: 5,
            index: 0
        })
    );
}

#[test]
fn read_be_widths() {
    let mut pos = 0;
    assert_eq!(read_be(&[0x12, 0x34, 0x56], &mut pos, 2), Some(0x1234));
    assert_eq!(pos, 2);
    // Zero width consumes nothing.
    assert_eq!(read_be(&[0xFF], &mut pos, 0), Some(0));
    assert_eq!(pos, 2);
    // Past the end.
    assert_eq!(read_be(&[0x01], &mut 0, 4), None);
}

#[test]
fn xref_stream_entries_cover_all_types() {
    // W = [1 2 1], default Index [0 4]: free, uncompressed, compressed, uncompressed.
    let (dict, stream) = xref_stream(
        &[1, 2, 1],
        &[(0, 0, 0), (1, 1234, 0), (2, 9, 3), (1, 5678, 0)],
    );
    let entries = parse_xref_stream_entries(&dict, &stream, 0, Limits::default()).unwrap();
    assert_eq!(entries.len(), 4);
    assert_eq!(
        entries[0],
        (
            0,
            XRefEntry::Free {
                next_free: 0,
                generation: 0
            }
        )
    );
    assert_eq!(
        entries[1],
        (
            1,
            XRefEntry::InUse {
                offset: 1234,
                generation: 0
            }
        )
    );
    assert_eq!(
        entries[2],
        (
            2,
            XRefEntry::Compressed {
                container: 9,
                index: 3
            }
        )
    );
}

#[test]
fn xref_stream_rejects_bad_w() {
    // /W must have exactly three widths, each <= 8.
    let (mut dict, stream) = xref_stream(&[1, 2, 1], &[(1, 1, 0)]);
    dict.insert(
        Name::from("W"),
        Object::Array(vec![Object::Integer(1)].into()),
    );
    assert_eq!(
        parse_xref_stream_entries(&dict, &stream, 0, Limits::default())
            .unwrap_err()
            .kind(),
        ErrorKind::InvalidXref
    );
    // Zero total width.
    let (mut dict, stream) = xref_stream(&[1, 2, 1], &[(1, 1, 0)]);
    dict.insert(
        Name::from("W"),
        Object::Array(vec![Object::Integer(0), Object::Integer(0), Object::Integer(0)].into()),
    );
    assert_eq!(
        parse_xref_stream_entries(&dict, &stream, 0, Limits::default())
            .unwrap_err()
            .kind(),
        ErrorKind::InvalidXref
    );
}
