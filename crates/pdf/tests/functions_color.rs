//! PDF functions (§7.10) and color-space resolution (§8.6) through the facade.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::{assemble, minimal_doc};
use prismpdf::cos::{self, Dictionary, Name, Object, ObjectId, PdfString};
use prismpdf::{ColorSpace, Document, parse_function, resolve_indexed, resolve_separation};

#[test]
fn parse_function_resolves_indirect_subfunctions() {
    // A Type 2 function as an indirect object (as a Separation /TintTransform would be),
    // referenced from the catalog so it survives loading; parse_function must follow the ref.
    let objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R /T 4 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R >>".to_vec(),
        b"<< /FunctionType 2 /Domain [0 1] /N 1 /C0 [0 0 0] /C1 [0.2 0.4 0.6] >>".to_vec(),
    ];
    let doc = Document::open(assemble(&objects, "")).unwrap();
    let f = parse_function(&doc, &Object::Reference(ObjectId::new(4, 0))).unwrap();
    let out = f.eval(&[0.5]);
    assert_eq!(out.len(), 3);
    assert!((out[0] - 0.1).abs() < 1e-6);
    assert!((out[1] - 0.2).abs() < 1e-6);
    assert!((out[2] - 0.3).abs() < 1e-6);
}

#[test]
fn resolve_separation_with_exponential_tint() {
    // [/Separation /Cyan /DeviceCMYK <Type 2 t -> [t,0,0,0]>]
    let mut tint = Dictionary::new();
    tint.insert(Name::from("FunctionType"), Object::Integer(2));
    tint.insert(
        Name::from("Domain"),
        Object::Array(cos::Array::from(vec![
            Object::Integer(0),
            Object::Integer(1),
        ])),
    );
    tint.insert(Name::from("N"), Object::Integer(1));
    tint.insert(
        Name::from("C0"),
        Object::Array(cos::Array::from(vec![Object::Real(0.0); 4])),
    );
    tint.insert(
        Name::from("C1"),
        Object::Array(cos::Array::from(
            [1.0, 0.0, 0.0, 0.0].map(Object::Real).to_vec(),
        )),
    );
    let sep_obj = Object::Array(cos::Array::from(vec![
        Object::Name(Name::from("Separation")),
        Object::Name(Name::from("Cyan")),
        Object::Name(Name::from("DeviceCMYK")),
        Object::Dictionary(tint),
    ]));

    let doc = minimal_doc();
    let sep = resolve_separation(&doc, &sep_obj).unwrap().unwrap();
    assert_eq!(sep.components(), 1);
    assert_eq!(sep.colorant_names(), &["Cyan".to_string()]);
    assert_eq!(sep.alternate(), ColorSpace::DeviceCmyk);
    let cmyk = sep.to_alternate(&[0.5]);
    assert_eq!(cmyk.len(), 4);
    assert!((cmyk[0] - 0.5).abs() < 1e-6 && cmyk[1].abs() < 1e-6);
}

#[test]
fn resolve_separation_rejects_non_separation() {
    let doc = minimal_doc();
    assert!(
        resolve_separation(&doc, &Object::Name(Name::from("DeviceRGB")))
            .unwrap()
            .is_none()
    );
}

#[test]
fn resolve_indexed_palette_and_lookup() {
    // [/Indexed /DeviceRGB 2 (red green blue)] — a 3-entry RGB palette.
    let palette = vec![255, 0, 0, 0, 255, 0, 0, 0, 255];
    let obj = Object::Array(cos::Array::from(vec![
        Object::Name(Name::from("Indexed")),
        Object::Name(Name::from("DeviceRGB")),
        Object::Integer(2),
        Object::String(PdfString::from(palette)),
    ]));
    let doc = minimal_doc();
    let idx = resolve_indexed(&doc, &obj).unwrap().unwrap();
    assert_eq!(idx.base, ColorSpace::DeviceRgb);
    assert_eq!(idx.hival, 2);
    assert_eq!(idx.entry(1), Some(&[0u8, 255, 0][..])); // entry 1 = green
    assert_eq!(idx.entry(3), None); // out of range
}

#[test]
fn resolve_indexed_rejects_non_indexed() {
    let doc = minimal_doc();
    assert!(
        resolve_indexed(&doc, &Object::Name(Name::from("DeviceRGB")))
            .unwrap()
            .is_none()
    );
}

#[test]
fn lab_colour_space_is_three_components() {
    // Lab is 3-component (§8.6.5.4); exercised here as the base of an Indexed space so the public
    // resolver path runs `color_space_object` over `[/Lab << … >>]`.
    let obj = Object::Array(cos::Array::from(vec![
        Object::Name(Name::from("Indexed")),
        Object::Array(cos::Array::from(vec![
            Object::Name(Name::from("Lab")),
            Object::Dictionary(Dictionary::new()),
        ])),
        Object::Integer(0),
        Object::String(PdfString::from(vec![50, 0, 0])),
    ]));
    let doc = minimal_doc();
    let idx = resolve_indexed(&doc, &obj).unwrap().unwrap();
    assert_eq!(idx.base.components(), 3);
}

// --- §7.10.4 stitching recursion bounds (DESIGN.md §3.4) ---------------------------------------

/// A document whose object 4 is a stitching function referencing `functions`, plus an optional
/// object 5 for mutual-recursion cases.
fn stitching_doc(functions: &str, object5: Option<&str>) -> Document {
    let mut objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R /T 4 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R >>".to_vec(),
        format!(
            "<< /FunctionType 3 /Domain [0 1] /Functions {functions} /Bounds [] /Encode [0 1] >>"
        )
        .into_bytes(),
    ];
    if let Some(five) = object5 {
        objects.push(five.as_bytes().to_vec());
    }
    Document::open(assemble(&objects, "")).unwrap()
}

#[test]
fn self_referential_stitching_function_is_rejected() {
    // /Functions points back at the function's own object. Before the depth bound this recursed
    // until the stack was exhausted, which aborts the process (SIGABRT) rather than unwinding —
    // so `catch_unwind` at the FFI boundary could not contain it either.
    let doc = stitching_doc("[4 0 R]", None);
    assert!(parse_function(&doc, &Object::Reference(ObjectId::new(4, 0))).is_none());
}

#[test]
fn mutually_referential_stitching_functions_are_rejected() {
    // The two-object cycle 4 → 5 → 4, which a single-step "is this my own id" check would miss.
    let doc = stitching_doc(
        "[5 0 R]",
        Some("<< /FunctionType 3 /Domain [0 1] /Functions [4 0 R] /Bounds [] /Encode [0 1] >>"),
    );
    assert!(parse_function(&doc, &Object::Reference(ObjectId::new(4, 0))).is_none());
}

#[test]
fn stitching_nesting_is_bounded_but_allows_real_depth() {
    // A leaf type-2 wrapped in `levels` stitching functions. Nesting to the cap still parses;
    // one level deeper is refused. Real files nest one or two levels.
    fn nested(levels: usize) -> String {
        let mut s = "<< /FunctionType 2 /Domain [0 1] /N 1 /C0 [0] /C1 [1] >>".to_string();
        for _ in 0..levels {
            s = format!(
                "<< /FunctionType 3 /Domain [0 1] /Functions [{s}] /Bounds [] /Encode [0 1] >>"
            );
        }
        s
    }

    // `nested(k)` already carries the outermost level, so the object under test is the wrapper:
    // parsing it descends k further levels.
    let deep = stitching_doc(&format!("[{}]", nested(6)), None);
    let f = parse_function(&deep, &Object::Reference(ObjectId::new(4, 0)))
        .expect("nesting within the bound parses");
    assert_eq!(f.eval(&[0.5]).len(), 1);

    let too_deep = stitching_doc(&format!("[{}]", nested(9)), None);
    assert!(parse_function(&too_deep, &Object::Reference(ObjectId::new(4, 0))).is_none());
}
