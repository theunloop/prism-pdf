//! The objects a signature revision writes (ISO 32000-1 §12.7.4.5, §12.5.5).
//!
//! Split out of [`signing`](super) so that module keeps to the revision itself — laying the
//! signature out, hashing the byte range, patching `/Contents` — while the dictionaries and
//! appearance streams it emits are built here. Both halves share `signing`'s imports through
//! `use super::*`, as the builder's sibling modules do.

use super::*;

/// The signature field dictionary (§12.7.4.5), merged with a widget annotation. `rect` is the widget
/// rectangle and `appearance` an optional `/AP /N` Form XObject (a visible signature, §12.5.5).
pub(super) fn signature_field(
    sig_id: ObjectId,
    page_id: ObjectId,
    rect: [f32; 4],
    appearance: Option<ObjectId>,
) -> Dictionary {
    let mut field = Dictionary::new();
    field.insert(Name::from("FT"), Object::Name(Name::from("Sig")));
    field.insert(Name::from("Type"), Object::Name(Name::from("Annot")));
    field.insert(Name::from("Subtype"), Object::Name(Name::from("Widget")));
    field.insert(
        Name::from("T"),
        Object::String(PdfString::from(b"Signature1".to_vec())),
    );
    field.insert(Name::from("V"), Object::Reference(sig_id));
    field.insert(Name::from("F"), Object::Integer(132)); // Print + Locked
    field.insert(
        Name::from("Rect"),
        Object::Array(Array::from_vec(
            rect.iter().map(|&v| Object::Real(f64::from(v))).collect(),
        )),
    );
    field.insert(Name::from("P"), Object::Reference(page_id));
    if let Some(ap_id) = appearance {
        let mut ap = Dictionary::new();
        ap.insert(Name::from("N"), Object::Reference(ap_id));
        field.insert(Name::from("AP"), Object::Dictionary(ap));
    }
    field
}

/// A fresh AcroForm holding a single signature field, with signatures-exist + append-only flags.
pub(super) fn new_acroform(field_id: ObjectId) -> Dictionary {
    let mut acroform = Dictionary::new();
    acroform.insert(
        Name::from("Fields"),
        Object::Array(Array::from_vec(vec![Object::Reference(field_id)])),
    );
    acroform.insert(Name::from("SigFlags"), Object::Integer(3));
    acroform
}

/// The text lines for a visible appearance: the caller's override, or a default derived from the
/// signer name and signing date.
pub(super) fn appearance_lines(
    ap: &SignatureAppearance,
    name: Option<&str>,
    date: &str,
) -> Vec<String> {
    match &ap.text {
        Some(text) => text.lines().map(str::to_string).collect(),
        None => vec![
            match name {
                Some(name) => format!("Digitally signed by {name}"),
                None => "Digitally signed".to_string(),
            },
            format!("Date: {date}"),
        ],
    }
}

/// An empty zero-size appearance Form XObject for an **invisible** signature widget: it draws
/// nothing, but satisfies the PDF/A requirement (ISO 19005-1 §6.9) that every form field carry an
/// appearance dictionary.
pub(super) fn empty_appearance_xobject() -> Stream {
    let mut dict = Dictionary::new();
    dict.insert(Name::from("Type"), Object::Name(Name::from("XObject")));
    dict.insert(Name::from("Subtype"), Object::Name(Name::from("Form")));
    dict.insert(Name::from("FormType"), Object::Integer(1));
    dict.insert(
        Name::from("BBox"),
        Object::Array(Array::from_vec(vec![Object::Real(0.0); 4])),
    );
    Stream::new(dict, Vec::new())
}

/// Build the appearance Form XObject (§12.5.5 / §8.10): a `/BBox`-bounded box drawing `lines` in
/// Helvetica and, when given, `image` — `(object id, width, height)` of an image XObject — fitted
/// into the box. The widget maps this BBox onto its `/Rect`.
pub(super) fn appearance_xobject(
    width: f32,
    height: f32,
    font_id: ObjectId,
    lines: &[String],
    image: Option<(ObjectId, u32, u32)>,
) -> Stream {
    let mut dict = Dictionary::new();
    dict.insert(Name::from("Type"), Object::Name(Name::from("XObject")));
    dict.insert(Name::from("Subtype"), Object::Name(Name::from("Form")));
    dict.insert(Name::from("FormType"), Object::Integer(1));
    dict.insert(
        Name::from("BBox"),
        Object::Array(Array::from_vec(vec![
            Object::Real(0.0),
            Object::Real(0.0),
            Object::Real(f64::from(width)),
            Object::Real(f64::from(height)),
        ])),
    );
    let mut fonts = Dictionary::new();
    fonts.insert(Name::from("Helv"), Object::Reference(font_id));
    let mut resources = Dictionary::new();
    resources.insert(Name::from("Font"), Object::Dictionary(fonts));
    if let Some((image_id, _, _)) = image {
        let mut xobjects = Dictionary::new();
        xobjects.insert(Name::from("Im0"), Object::Reference(image_id));
        resources.insert(Name::from("XObject"), Object::Dictionary(xobjects));
    }
    dict.insert(Name::from("Resources"), Object::Dictionary(resources));

    let mut content = Vec::new();
    // The image takes the whole box when it is alone and the left 40% when text sits beside it,
    // scaled to fit and centred in its slot. Image space is the unit square (§8.9.4), so the `cm`
    // carries both the size and the position.
    let mut text_x = 2.0f32;
    if let Some((_, image_width, image_height)) = image {
        let pad = 2.0f32;
        let slot = if lines.is_empty() { width } else { width * 0.4 };
        let (avail_w, avail_h) = ((slot - 2.0 * pad).max(1.0), (height - 2.0 * pad).max(1.0));
        let scale = (avail_w / image_width.max(1) as f32).min(avail_h / image_height.max(1) as f32);
        let (drawn_w, drawn_h) = (image_width as f32 * scale, image_height as f32 * scale);
        let (x, y) = (
            pad + (avail_w - drawn_w) / 2.0,
            pad + (avail_h - drawn_h) / 2.0,
        );
        content.extend_from_slice(
            format!("q\n{drawn_w:.2} 0 0 {drawn_h:.2} {x:.2} {y:.2} cm\n/Im0 Do\nQ\n").as_bytes(),
        );
        text_x = slot + pad;
    }
    if lines.is_empty() {
        return Stream::new(dict, content);
    }

    // Draw each line top-to-bottom with a fixed 10-unit leading (§9.4 text operators).
    content.extend_from_slice(b"BT\n/Helv 8 Tf\n10 TL\n");
    content
        .extend_from_slice(format!("{text_x:.2} {:.2} Td\n", (height - 10.0).max(0.0)).as_bytes());
    for (i, line) in lines.iter().enumerate() {
        if i > 0 {
            content.extend_from_slice(b"T*\n");
        }
        content.push(b'(');
        escape_literal_string(line.as_bytes(), &mut content);
        content.extend_from_slice(b") Tj\n");
    }
    content.extend_from_slice(b"ET");
    Stream::new(dict, content)
}

/// The standard-14 Helvetica font object (§9.6.2.2), referenced by the appearance stream.
pub(super) fn helvetica_font() -> Dictionary {
    let mut font = Dictionary::new();
    font.insert(Name::from("Type"), Object::Name(Name::from("Font")));
    font.insert(Name::from("Subtype"), Object::Name(Name::from("Type1")));
    font.insert(
        Name::from("BaseFont"),
        Object::Name(Name::from("Helvetica")),
    );
    font
}
