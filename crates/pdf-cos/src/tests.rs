//! Cross-cutting unit tests for the COS public API (EPIC 1, ISO 32000 §7.3).
//!
//! Exercises every accessor, `From` conversion, and `Debug`/`Display`/`PartialEq` impl, plus the
//! ADR-mandated equality rules (no numeric coercion, no reference resolution, order-independent
//! dictionaries — ADR-0003).

use bytes::Bytes;

use crate::{Array, Dictionary, Name, Object, ObjectId, PdfString, Stream};

// ---- Object accessors (§7.3.2–§7.3.10) -------------------------------------------------------

#[test]
fn object_default_is_null() {
    assert_eq!(Object::default(), Object::Null);
    assert!(Object::Null.is_null());
    assert!(!Object::Boolean(true).is_null());
}

#[test]
fn object_scalar_accessors_match_only_their_arm() {
    assert_eq!(Object::Boolean(true).as_bool(), Some(true));
    assert_eq!(Object::Integer(7).as_bool(), None);

    assert_eq!(Object::Integer(7).as_integer(), Some(7));
    assert_eq!(Object::Real(7.0).as_integer(), None);

    assert_eq!(Object::Real(2.5).as_real(), Some(2.5));
    assert_eq!(Object::Integer(2).as_real(), None);
}

#[test]
fn object_as_f64_is_the_only_numeric_coercion() {
    // ADR-0003: as_f64 crosses the integer/real divide; nothing else does.
    assert_eq!(Object::Integer(3).as_f64(), Some(3.0));
    assert_eq!(Object::Real(3.5).as_f64(), Some(3.5));
    assert_eq!(Object::Boolean(true).as_f64(), None);
}

#[test]
fn object_composite_accessors() {
    assert_eq!(
        Object::String(PdfString::from(b"x".to_vec())).as_string(),
        Some(&PdfString::from(b"x".to_vec()))
    );
    assert_eq!(Object::Null.as_string(), None);

    assert_eq!(
        Object::Name(Name::from("N")).as_name(),
        Some(&Name::from("N"))
    );
    assert_eq!(Object::Null.as_name(), None);

    assert!(Object::Array(Array::new()).as_array().is_some());
    assert!(Object::Null.as_array().is_none());

    assert!(Object::Dictionary(Dictionary::new()).as_dict().is_some());
    assert!(Object::Null.as_dict().is_none());

    let stream = Stream::new(Dictionary::new(), b"d".to_vec());
    assert!(Object::Stream(stream).as_stream().is_some());
    assert!(Object::Null.as_stream().is_none());

    assert_eq!(
        Object::Reference(ObjectId::new(4, 1)).as_reference(),
        Some(ObjectId::new(4, 1))
    );
    assert_eq!(Object::Null.as_reference(), None);
}

#[test]
fn object_from_conversions() {
    assert_eq!(Object::from(true), Object::Boolean(true));
    assert_eq!(Object::from(5i64), Object::Integer(5));
    assert_eq!(Object::from(5i32), Object::Integer(5));
    assert_eq!(Object::from(1.5f64), Object::Real(1.5));
    assert_eq!(
        Object::from(ObjectId::new(1, 0)),
        Object::Reference(ObjectId::new(1, 0))
    );
    assert_eq!(
        Object::from(PdfString::from(b"s".to_vec())),
        Object::String(PdfString::from(b"s".to_vec()))
    );
    assert_eq!(Object::from(Name::from("N")), Object::Name(Name::from("N")));
    assert!(matches!(Object::from(Array::new()), Object::Array(_)));
    assert!(matches!(
        Object::from(Dictionary::new()),
        Object::Dictionary(_)
    ));
    assert!(matches!(
        Object::from(Stream::new(Dictionary::new(), Bytes::new())),
        Object::Stream(_)
    ));
    assert!(matches!(
        Object::from(vec![Object::Integer(1)]),
        Object::Array(_)
    ));
}

#[test]
fn object_equality_rules_per_adr_0003() {
    // No numeric coercion.
    assert_ne!(Object::Integer(1), Object::Real(1.0));
    // References are never equal to a value, and are compared structurally.
    assert_eq!(
        Object::Reference(ObjectId::new(1, 0)),
        Object::Reference(ObjectId::new(1, 0))
    );
    assert_ne!(
        Object::Reference(ObjectId::new(1, 0)),
        Object::Reference(ObjectId::new(1, 1))
    );
}

#[test]
fn object_debug_is_available() {
    assert!(!format!("{:?}", Object::Integer(1)).is_empty());
}

// ---- Name (§7.3.5) ---------------------------------------------------------------------------

#[test]
fn name_construction_and_views() {
    assert_eq!(Name::new(b"Type".to_vec()).as_bytes(), b"Type");
    assert_eq!(Name::from_static("Type").as_bytes(), b"Type");
    assert_eq!(Name::from("Type").as_str(), Some("Type"));
    assert_eq!(Name::from(String::from("Type")), Name::from("Type"));
    assert_eq!(Name::from(b"Type".to_vec()), Name::from("Type"));
    assert_eq!(Name::from(Bytes::from_static(b"Type")), Name::from("Type"));
    assert_eq!(Name::default().as_bytes(), b"");
}

#[test]
fn name_invalid_utf8_has_no_str_view() {
    assert_eq!(Name::from(vec![0xFF, 0xFE]).as_str(), None);
}

#[test]
fn name_partial_eq_with_str_and_display_debug() {
    let n = Name::from("Type");
    assert_eq!(n, *"Type");
    assert_eq!(n, "Type");
    assert_ne!(n, "Other");
    assert_eq!(format!("{n}"), "/Type");
    assert!(format!("{n:?}").contains("Type"));
    // Names hash by bytes (usable as map/set keys).
    let mut set = std::collections::HashSet::new();
    set.insert(Name::from("A"));
    assert!(set.contains(&Name::from("A")));
}

// ---- PdfString (§7.3.4) ----------------------------------------------------------------------

#[test]
fn pdf_string_construction_and_views() {
    let s = PdfString::new(Bytes::from_static(b"hi"));
    assert_eq!(s.as_bytes(), b"hi");
    assert_eq!(s.len(), 2);
    assert!(!s.is_empty());
    assert!(PdfString::default().is_empty());
    assert_eq!(PdfString::from(&b"x"[..]), PdfString::from(b"x".to_vec()));
    assert_eq!(
        PdfString::from(Bytes::from_static(b"x")),
        PdfString::from(b"x".to_vec())
    );
    assert_eq!(
        PdfString::from(b"abc".to_vec()).into_bytes(),
        Bytes::from_static(b"abc")
    );
    assert!(!format!("{:?}", PdfString::from(b"x".to_vec())).is_empty());
}

// ---- Array (§7.3.6) --------------------------------------------------------------------------

#[test]
fn array_build_deref_and_collect() {
    let mut a = Array::new();
    assert!(a.is_empty());
    a.push(Object::Integer(1));
    a.push(Object::Integer(2));
    assert_eq!(a.len(), 2);
    assert_eq!(a[0], Object::Integer(1)); // Deref to slice
    assert_eq!(a.iter().count(), 2);

    let from_vec = Array::from_vec(vec![Object::Null]);
    assert_eq!(from_vec.len(), 1);
    let collected: Array = [Object::Integer(9)].into_iter().collect();
    assert_eq!(collected[0], Object::Integer(9));
    let from: Array = vec![Object::Boolean(true)].into();
    assert_eq!(from.len(), 1);
    assert_eq!(Array::default().len(), 0);
    assert!(format!("{a:?}").starts_with('['));
}

#[test]
fn array_clone_is_structural_equal() {
    let a = Array::from_vec(vec![Object::Integer(1)]);
    let b = a.clone();
    assert_eq!(a, b); // O(1) Arc clone, structurally equal (ADR-0002)
}

// ---- Dictionary (§7.3.7) ---------------------------------------------------------------------

#[test]
fn dictionary_crud_and_iteration() {
    let mut d = Dictionary::new();
    assert!(d.is_empty());
    assert_eq!(d.insert(Name::from("A"), Object::Integer(1)), None);
    // insert returns the previous value
    assert_eq!(
        d.insert(Name::from("A"), Object::Integer(2)),
        Some(Object::Integer(1))
    );
    assert_eq!(d.len(), 1);
    assert!(d.contains_key(&Name::from("A")));
    assert_eq!(d.get(&Name::from("A")), Some(&Object::Integer(2)));
    assert_eq!(d.keys().count(), 1);
    assert_eq!(d.iter().count(), 1);
    assert!(!format!("{d:?}").is_empty());
    assert_eq!(d.remove(&Name::from("A")), Some(Object::Integer(2)));
    assert!(d.is_empty());
    assert_eq!(Dictionary::default().len(), 0);
}

#[test]
fn dictionary_typed_getters() {
    let mut d = Dictionary::new();
    d.insert(Name::from("Int"), Object::Integer(3));
    d.insert(Name::from("Nm"), Object::Name(Name::from("V")));
    d.insert(Name::from("Arr"), Object::Array(Array::new()));
    d.insert(Name::from("Dct"), Object::Dictionary(Dictionary::new()));
    d.insert(
        Name::from("Stm"),
        Object::Stream(Stream::new(Dictionary::new(), Bytes::new())),
    );
    d.insert(Name::from("Ref"), Object::Reference(ObjectId::new(2, 0)));

    assert_eq!(d.get_integer(&Name::from("Int")), Some(3));
    assert_eq!(d.get_name(&Name::from("Nm")), Some(&Name::from("V")));
    assert!(d.get_array(&Name::from("Arr")).is_some());
    assert!(d.get_dict(&Name::from("Dct")).is_some());
    assert!(d.get_stream(&Name::from("Stm")).is_some());
    assert_eq!(
        d.get_reference(&Name::from("Ref")),
        Some(ObjectId::new(2, 0))
    );

    // Wrong type / missing → None.
    assert_eq!(d.get_integer(&Name::from("Nm")), None);
    assert_eq!(d.get_name(&Name::from("missing")), None);
}

#[test]
fn dictionary_equality_is_order_independent() {
    // ADR-0003.
    let mut a = Dictionary::new();
    a.insert(Name::from("X"), Object::Integer(1));
    a.insert(Name::from("Y"), Object::Integer(2));
    let mut b = Dictionary::new();
    b.insert(Name::from("Y"), Object::Integer(2));
    b.insert(Name::from("X"), Object::Integer(1));
    assert_eq!(a, b);

    let collected: Dictionary = [
        (Name::from("X"), Object::Integer(1)),
        (Name::from("Y"), Object::Integer(2)),
    ]
    .into_iter()
    .collect();
    assert_eq!(collected, a);
}

// ---- Stream (§7.3.8) -------------------------------------------------------------------------

#[test]
fn stream_accessors_and_length_authority() {
    let mut dict = Dictionary::new();
    dict.insert(Name::from("Length"), Object::Integer(999)); // a lie
    let mut s = Stream::new(dict, b"abcd".to_vec());
    assert_eq!(s.raw().as_ref(), b"abcd");
    assert_eq!(s.raw_len(), 4); // ADR-0004: raw length, not /Length
    assert!(s.dict().contains_key(&Name::from("Length")));
    s.dict_mut().insert(Name::from("Extra"), Object::Null);
    assert!(s.dict().contains_key(&Name::from("Extra")));
    assert!(format!("{s:?}").contains("Stream"));
    assert_eq!(Stream::default().raw_len(), 0);
    assert_eq!(s.into_raw(), Bytes::from_static(b"abcd"));
}

// ---- ObjectId (§7.3.10) ----------------------------------------------------------------------

#[test]
fn object_id_fields_display_and_ord() {
    let id = ObjectId::new(12, 3);
    assert_eq!(id.number, 12);
    assert_eq!(id.generation, 3);
    assert_eq!(format!("{id}"), "12 3 R");
    assert!(ObjectId::new(1, 0) < ObjectId::new(2, 0));
    assert!(format!("{id:?}").contains("12"));
}
