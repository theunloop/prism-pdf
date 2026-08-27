//! Object streams (§7.5.7): pulling compressed objects out by `(container, index)`.

use super::build_pdf_with_streams;
use crate::error::ErrorKind;
use crate::parser::Limits;
use crate::xref::*;

use pdf_cos::{Dictionary, Name, Object, Stream};

#[test]
fn fetches_objects_from_object_stream() {
    // §7.5.7: pull compressed objects out of their object stream by (container, index).
    let pdf = build_pdf_with_streams();
    let xref = XRef::parse(&pdf).unwrap();

    let Some(Object::Dictionary(catalog)) = xref.fetch(&pdf, 1).unwrap() else {
        panic!("catalog should be a dictionary");
    };
    assert_eq!(
        catalog.get_name(&Name::from("Type")),
        Some(&Name::from("Catalog"))
    );

    let Some(Object::Dictionary(pages)) = xref.fetch(&pdf, 2).unwrap() else {
        panic!("pages should be a dictionary");
    };
    assert_eq!(pages.get_integer(&Name::from("Count")), Some(1));

    let Some(Object::Dictionary(page)) = xref.fetch(&pdf, 3).unwrap() else {
        panic!("page should be a dictionary");
    };
    assert_eq!(
        page.get_name(&Name::from("Type")),
        Some(&Name::from("Page"))
    );
}

#[test]
fn rejects_object_stream_with_implausible_n() {
    // Anti-DoS (DESIGN.md §3.4): an object stream whose `/N` dwarfs its data must be refused before
    // it can drive a huge allocation/loop — not parsed up to a multi-gigabyte `Vec::with_capacity`.
    let mut dict = Dictionary::new();
    dict.insert(Name::from("Type"), Object::Name(Name::from("ObjStm")));
    dict.insert(Name::from("N"), Object::Integer(4_000_000_000));
    dict.insert(Name::from("First"), Object::Integer(4));
    let stream = Stream::new(dict, b"tiny".to_vec());

    let err = crate::xref::objstm_members(&stream, Limits::default()).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::LimitExceeded);

    // A `/N` within the limit is accepted by the count check (it then fails later on the data, not
    // on the bound) — proving the guard rejects only the implausible case.
    let small = Limits {
        max_objstm_objects: 8,
        ..Limits::default()
    };
    let mut dict2 = Dictionary::new();
    dict2.insert(Name::from("N"), Object::Integer(2));
    let ok_stream = Stream::new(dict2, b"".to_vec());
    assert_eq!(crate::xref::objstm_count(&ok_stream, small, 0).unwrap(), 2);
}
