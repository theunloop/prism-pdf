//! Coverage-focused tests for the facade's resolution helpers (§8.6 colour spaces, §7.10
//! functions, §9.8/§9.9 font reporting and subsetting, §8.9 image extraction). These exercise the
//! many fallback and error branches the happy-path tests in the other files don't reach, so the
//! crate clears the workspace's >90% line-coverage floor.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::{assemble, minimal_doc, stream_with, unhex};
use prismpdf::cos::{self, Dictionary, Name, Object};
use prismpdf::{
    ColorSpace, Document, document_fonts, page_annotations, page_images, page_text,
    page_text_positioned, resolve_separation, subset_fonts,
};

/// An `Object` array from a list of objects.
fn array(items: Vec<Object>) -> Object {
    Object::Array(cos::Array::from(items))
}

/// A real-number `Object` array.
fn nums(vals: &[f64]) -> Object {
    array(vals.iter().copied().map(Object::Real).collect())
}

/// A valid Type 2 tint-transform dictionary with `n_out` outputs (maps t → [t, t, …]).
fn type2_tint(n_out: usize) -> Object {
    let mut t = Dictionary::new();
    t.insert(Name::from("FunctionType"), Object::Integer(2));
    t.insert(Name::from("Domain"), nums(&[0.0, 1.0]));
    t.insert(Name::from("N"), Object::Integer(1));
    t.insert(Name::from("C0"), nums(&vec![0.0; n_out]));
    t.insert(Name::from("C1"), nums(&vec![1.0; n_out]));
    Object::Dictionary(t)
}

/// Build a one-image document (the image is object 4, `extra` are objects 5..) and return the
/// colour space the facade resolves for that image.
fn image_color_space(image_body: Vec<u8>, extra: Vec<Vec<u8>>) -> ColorSpace {
    let mut objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Resources << /XObject << /Im0 4 0 R >> >> >>".to_vec(),
        image_body,
    ];
    objects.extend(extra);
    let doc = Document::open(assemble(&objects, "")).unwrap();
    let images = page_images(&doc, 0).unwrap();
    assert_eq!(images.len(), 1, "expected exactly one image");
    images[0].info.color_space
}

/// An image XObject body with the given `/ColorSpace` entry (or none).
fn image_with(color_space: Option<&str>) -> Vec<u8> {
    let cs = match color_space {
        Some(c) => format!("/ColorSpace {c}"),
        None => String::new(),
    };
    stream_with(
        &format!("/Type /XObject /Subtype /Image /Width 1 /Height 1 /BitsPerComponent 8 {cs}"),
        &[0u8],
    )
}

#[test]
fn color_space_iccbased_variants() {
    // /N drives the component count (here 4 → CMYK).
    assert_eq!(
        image_color_space(
            image_with(Some("[/ICCBased 5 0 R]")),
            vec![stream_with("/N 4", &[0u8; 4])],
        ),
        ColorSpace::DeviceCmyk
    );
    // ICCBased with no /N defaults to 3 components (RGB).
    assert_eq!(
        image_color_space(
            image_with(Some("[/ICCBased 5 0 R]")),
            vec![stream_with("", &[0u8])],
        ),
        ColorSpace::DeviceRgb
    );
    // ICCBased whose second element is not a stream falls back to 3-component RGB.
    assert_eq!(
        image_color_space(image_with(Some("[/ICCBased /Bogus]")), vec![]),
        ColorSpace::Other(3)
    );
}

#[test]
fn color_space_array_families() {
    assert_eq!(
        image_color_space(image_with(Some("[/CalRGB << >>]")), vec![]),
        ColorSpace::DeviceRgb
    );
    assert_eq!(
        image_color_space(image_with(Some("[/CalGray << >>]")), vec![]),
        ColorSpace::DeviceGray
    );
    // DeviceN reports one component per colorant name.
    assert_eq!(
        image_color_space(
            image_with(Some("[/DeviceN [/A /B /C] /DeviceRGB 5 0 R]")),
            vec![stream_with("", b"{ }")],
        ),
        ColorSpace::Other(3)
    );
    // DeviceN whose names slot is not an array → single component.
    assert_eq!(
        image_color_space(image_with(Some("[/DeviceN /NotAnArray]")), vec![]),
        ColorSpace::Other(1)
    );
    // Indexed/Separation samples are single-component.
    assert_eq!(
        image_color_space(image_with(Some("[/Indexed /DeviceRGB 0 (x)]")), vec![]),
        ColorSpace::Other(1)
    );
    // Lab is a 3-component space (§8.6.5.4).
    assert_eq!(
        image_color_space(image_with(Some("[/Lab << >>]")), vec![]),
        ColorSpace::Other(3)
    );
    // Unknown family name → single component.
    assert_eq!(
        image_color_space(image_with(Some("[/Bogus << >>]")), vec![]),
        ColorSpace::Other(1)
    );
    // An array whose first element is not a name → single component.
    assert_eq!(
        image_color_space(image_with(Some("[123 456]")), vec![]),
        ColorSpace::Other(1)
    );
}

#[test]
fn color_space_name_and_fallbacks() {
    // An unrecognised colour-space name resolves to a single component.
    assert_eq!(
        image_color_space(image_with(Some("/Frobnicate")), vec![]),
        ColorSpace::Other(1)
    );
    // A /ColorSpace that is neither a name nor an array defaults to DeviceGray.
    assert_eq!(
        image_color_space(image_with(Some("42")), vec![]),
        ColorSpace::DeviceGray
    );
    // No /ColorSpace at all defaults to DeviceGray.
    assert_eq!(
        image_color_space(image_with(None), vec![]),
        ColorSpace::DeviceGray
    );
}

#[test]
fn resolve_separation_error_branches() {
    let doc = minimal_doc();
    let ok = |o: &Object| resolve_separation(&doc, o).unwrap();

    // First element not a name.
    assert!(ok(&array(vec![Object::Integer(1)])).is_none());
    // Unknown leading name.
    assert!(ok(&array(vec![Object::Name(Name::from("Foo"))])).is_none());
    // Separation whose colorant slot is not a name.
    assert!(
        ok(&array(vec![
            Object::Name(Name::from("Separation")),
            array(vec![]),
            Object::Name(Name::from("DeviceRGB")),
            type2_tint(3),
        ]))
        .is_none()
    );
    // Separation missing its alternate space.
    assert!(
        ok(&array(vec![
            Object::Name(Name::from("Separation")),
            Object::Name(Name::from("Cyan")),
        ]))
        .is_none()
    );
    // Separation missing its tint transform.
    assert!(
        ok(&array(vec![
            Object::Name(Name::from("Separation")),
            Object::Name(Name::from("Cyan")),
            Object::Name(Name::from("DeviceRGB")),
        ]))
        .is_none()
    );
    // Separation with an invalid tint transform.
    assert!(
        ok(&array(vec![
            Object::Name(Name::from("Separation")),
            Object::Name(Name::from("Cyan")),
            Object::Name(Name::from("DeviceRGB")),
            Object::Integer(7),
        ]))
        .is_none()
    );
    // DeviceN whose names slot is not an array.
    assert!(
        ok(&array(vec![
            Object::Name(Name::from("DeviceN")),
            Object::Name(Name::from("NotAnArray")),
            Object::Name(Name::from("DeviceRGB")),
            type2_tint(3),
        ]))
        .is_none()
    );
    // DeviceN with no usable colorant names.
    assert!(
        ok(&array(vec![
            Object::Name(Name::from("DeviceN")),
            array(vec![]),
            Object::Name(Name::from("DeviceRGB")),
            type2_tint(3),
        ]))
        .is_none()
    );
}

#[test]
fn resolve_separation_devicen_drops_non_name_colorants() {
    // A DeviceN names array with a stray non-name element keeps only the real colorant names.
    let doc = minimal_doc();
    let obj = array(vec![
        Object::Name(Name::from("DeviceN")),
        array(vec![Object::Name(Name::from("Spot")), Object::Integer(9)]),
        Object::Name(Name::from("DeviceRGB")),
        type2_tint(3),
    ]);
    let sep = resolve_separation(&doc, &obj).unwrap().unwrap();
    assert_eq!(sep.components(), 1);
    assert_eq!(sep.colorant_names(), &["Spot".to_string()]);
}

#[test]
fn page_annotations_in_range_and_out_of_range() {
    let objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Annots [4 0 R] >>".to_vec(),
        b"<< /Type /Annot /Subtype /Link /Rect [0 0 10 10] >>".to_vec(),
    ];
    let doc = Document::open(assemble(&objects, "")).unwrap();
    assert_eq!(page_annotations(&doc, 0).unwrap().len(), 1);
    // Out-of-range page index yields an empty vector, not an error.
    assert!(page_annotations(&doc, 9).unwrap().is_empty());
}

#[test]
fn page_text_positioned_out_of_range_is_none() {
    let doc = minimal_doc();
    assert_eq!(page_text_positioned(&doc, 9).unwrap(), None);
}

#[test]
fn document_fonts_covers_descriptor_and_embedded_edge_cases() {
    // A subset CFF program (FontFile3 /Type1C) so embedded_font's FontFile3-subtype refinement and
    // the non-sfnt "no metrics" branch run; metrics stay None because the bytes are not real.
    let cff = b"fake cff program";
    let objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>".to_vec(),
        // Page 1 shares /Fsh with page 2 (exercises the dedupe-by-object-number path).
        b"<< /Type /Page /Parent 2 0 R /Resources << /Font << /F1 5 0 R /Fsh 6 0 R >> >> >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Resources << /Font << /Fsh 6 0 R /F4 7 0 R /F5 8 0 R >> >> >>".to_vec(),
        // F1: Type1 with an embedded FontFile3 CFF program.
        b"<< /Type /Font /Subtype /Type1 /BaseFont /F1 /FontDescriptor 9 0 R >>".to_vec(),
        // Fsh: no /FontDescriptor at all → not embedded.
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Fsh >>".to_vec(),
        // F4: /FontDescriptor resolves to a non-dictionary object.
        b"<< /Type /Font /Subtype /TrueType /BaseFont /F4 /FontDescriptor 10 0 R >>".to_vec(),
        // F5: descriptor present but its /FontFile2 is not a stream.
        b"<< /Type /Font /Subtype /TrueType /BaseFont /F5 /FontDescriptor 11 0 R >>".to_vec(),
        b"<< /Type /FontDescriptor /FontName /F1 /FontFile3 12 0 R >>".to_vec(),
        b"999".to_vec(), // object 10: F4's descriptor is a bare integer
        b"<< /Type /FontDescriptor /FontName /F5 /FontFile2 13 0 R >>".to_vec(),
        stream_with("/Subtype /Type1C", cff), // object 12: the CFF program
        b"999".to_vec(),                        // object 13: F5's FontFile2 is a bare integer
    ];
    let doc = Document::open(assemble(&objects, "")).unwrap();
    let fonts = document_fonts(&doc).unwrap();
    // F1, Fsh, F4, F5 — Fsh reported once despite appearing on both pages.
    assert_eq!(fonts.len(), 4);

    let f1 = fonts.iter().find(|f| f.base_font == "F1").unwrap();
    let embedded = f1.embedded.as_ref().expect("F1 has an embedded program");
    assert_eq!(embedded.program, cff);
    assert!(embedded.metrics.is_none(), "fake CFF has no parsed metrics");

    for name in ["Fsh", "F4", "F5"] {
        let font = fonts.iter().find(|f| f.base_font == name).unwrap();
        assert!(font.embedded.is_none(), "{name} should not be embedded");
    }
}

#[test]
fn subset_fonts_walks_usage_and_skips_unsubsettable_fonts() {
    // Content shows several fonts (via Tj and TJ) and invokes a form (Do) whose own content shows
    // another font — exercising the usage walk and recursion — while every font hits a different
    // "cannot subset" branch, so subset_fonts returns a valid, unchanged document.
    let content = b"BT /F1 12 Tf [(A) -10 (B)] TJ \
        /F2 12 Tf (C) Tj /F3 12 Tf (D) Tj /F4 12 Tf (E) Tj /F5 12 Tf (G) Tj /F6 12 Tf (H) Tj ET \
        /Fm0 Do /FmMissing Do";
    let form = stream_with(
        "/Type /XObject /Subtype /Form /BBox [0 0 10 10] /Resources << /Font << /Ff 12 0 R >> >>",
        b"BT /Ff 10 Tf (Z) Tj ET",
    );
    let objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R \
            /Resources << /Font << /F1 5 0 R /F2 6 0 R /F3 7 0 R /F4 8 0 R /F5 9 0 R /F6 10 0 R >> \
            /XObject << /Fm0 11 0 R >> >> >>"
            .to_vec(),
        stream_with("", content),
        // F1: not a TrueType font.
        b"<< /Type /Font /Subtype /Type1 /BaseFont /F1 >>".to_vec(),
        // F2: TrueType with no /FontDescriptor.
        b"<< /Type /Font /Subtype /TrueType /BaseFont /F2 >>".to_vec(),
        // F3: TrueType descriptor with no /FontFile2.
        b"<< /Type /Font /Subtype /TrueType /BaseFont /F3 /FontDescriptor 13 0 R >>".to_vec(),
        // F4: descriptor whose /FontFile2 is a direct (non-reference) object.
        b"<< /Type /Font /Subtype /TrueType /BaseFont /F4 /FontDescriptor 14 0 R >>".to_vec(),
        // F5: descriptor whose /FontFile2 reference resolves to a non-stream.
        b"<< /Type /Font /Subtype /TrueType /BaseFont /F5 /FontDescriptor 15 0 R >>".to_vec(),
        // F6: the font object itself is not a dictionary.
        b"42".to_vec(),
        form,                                                                    // object 11
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Ff >>".to_vec(), // object 12 (used by the form)
        b"<< /Type /FontDescriptor /FontName /F3 >>".to_vec(),       // 13: no FontFile2
        b"<< /Type /FontDescriptor /FontName /F4 /FontFile2 << >> >>".to_vec(), // 14: direct, not a ref
        b"<< /Type /FontDescriptor /FontName /F5 /FontFile2 16 0 R >>".to_vec(), // 15
        b"123".to_vec(), // 16: F5's FontFile2 resolves to a non-stream
    ];
    let doc = Document::open(assemble(&objects, "")).unwrap();
    let out = subset_fonts(&doc).unwrap();
    // Nothing is subsettable, so the document round-trips and reopens cleanly.
    let reopened = Document::open(out).unwrap();
    assert_eq!(reopened.pages().unwrap().len(), 1);
}

#[test]
fn page_images_resolves_jbig2_globals_from_decodeparms_array() {
    // /DecodeParms given as an array (one entry per filter) — the facade scans it for the entry
    // carrying /JBIG2Globals. Bytes are the ISO 32000-1 §7.4.7 worked example (52×66).
    let image = unhex(
        "000000013000010000001300000034000000420000000000\
         00000040000000000002062000010000001e000000340000\
         004200000000000000000200100000000231db51ce51ffac",
    );
    let globals = unhex(
        "0000000000010000000032000003fffdff02fefefe000000\
         01000000012ae225aea9a5a538b4d9999c5c8e56ef0f872\
         7f2b53d4e37ef795cc5506dffac",
    );
    let img_obj = stream_with(
        "/Type /XObject /Subtype /Image /Width 52 /Height 66 /BitsPerComponent 1 \
         /ColorSpace /DeviceGray /Filter [/JBIG2Decode] /DecodeParms [<< /JBIG2Globals 5 0 R >>]",
        &image,
    );
    let objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Resources << /XObject << /Im0 4 0 R >> >> >>".to_vec(),
        img_obj,
        stream_with("", &globals),
    ];
    let doc = Document::open(assemble(&objects, "")).unwrap();
    let images = page_images(&doc, 0).unwrap();
    assert_eq!(images.len(), 1);
    assert_eq!(images[0].info.bits_per_component, 1);
}

#[test]
fn page_text_tolerates_non_stream_to_unicode_and_non_dict_font() {
    // F1's /ToUnicode is not a stream and F2 resolves to a non-dictionary: both are skipped
    // gracefully, the text still extracts via the Latin-1 fallback.
    let objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 5 0 R /F2 6 0 R >> >> >>".to_vec(),
        stream_with("", b"BT /F1 12 Tf (Hello) Tj ET"),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /F1 /ToUnicode 7 0 R >>".to_vec(),
        b"99".to_vec(), // object 6: F2 is not a dictionary
        b"42".to_vec(), // object 7: F1's /ToUnicode is not a stream
    ];
    let doc = Document::open(assemble(&objects, "")).unwrap();
    assert_eq!(page_text(&doc, 0).unwrap().as_deref(), Some("Hello"));
}

#[test]
fn collect_images_handles_every_xobject_shape() {
    // One page whose /XObject dictionary exercises each branch of image collection: forms with a
    // missing and a non-dictionary /Resources (both inherit the parent), a duplicate image
    // reference (deduped), a non-stream entry (skipped), a non-image/form XObject (ignored), and
    // one real image.
    let samples = [255u8, 0, 0]; // 1×1 RGB
    let objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Resources << /XObject << \
            /Fm0 4 0 R /Fm1 5 0 R /Im0 9 0 R /ImDup 9 0 R /NS 8 0 R /Ps 7 0 R >> >> >>"
            .to_vec(),
        // Fm0: form with no /Resources → inherits the parent resources.
        stream_with("/Type /XObject /Subtype /Form /BBox [0 0 10 10]", b"q Q"),
        // Fm1: form whose /Resources resolves to a non-dictionary → inherits the parent.
        stream_with(
            "/Type /XObject /Subtype /Form /BBox [0 0 10 10] /Resources 6 0 R",
            b"q Q",
        ),
        b"42".to_vec(), // object 6: Fm1's bogus /Resources target
        // Ps: an XObject that is neither Image nor Form → ignored.
        stream_with("/Type /XObject /Subtype /PS", b"x"),
        b"42".to_vec(), // object 8: a non-stream /XObject entry → skipped
        stream_with(
            "/Type /XObject /Subtype /Image /Width 1 /Height 1 /BitsPerComponent 8 /ColorSpace /DeviceRGB",
            &samples,
        ),
    ];
    let doc = Document::open(assemble(&objects, "")).unwrap();
    let images = page_images(&doc, 0).unwrap();
    assert_eq!(images.len(), 1, "only the single real image, deduped");
}

#[test]
fn page_images_with_non_dictionary_resources_is_empty() {
    // A page whose /Resources is not a dictionary is treated as empty (no images, no error).
    let objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Resources 4 0 R >>".to_vec(),
        b"42".to_vec(),
    ];
    let doc = Document::open(assemble(&objects, "")).unwrap();
    assert!(page_images(&doc, 0).unwrap().is_empty());
}
