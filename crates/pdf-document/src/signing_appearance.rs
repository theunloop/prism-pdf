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
    if !ap.caption.draw {
        return Vec::new();
    }
    match &ap.text {
        Some(text) => text.lines().map(str::to_string).collect(),
        None => vec![
            match name {
                Some(name) => format!("Digitally signed by {name}"),
                None => "Digitally signed".to_string(),
            },
            format!("Date: {}", readable_date(date)),
        ],
    }
}

/// Render a PDF date string (§7.9.4) for someone reading the document.
///
/// The default caption used to carry the wire form — `Date: D:20260911074751Z` — into a signature
/// a person is meant to read, because the same string goes into the signature dictionary's `/M`
/// and was reused here unchanged. The offset is shown when the string declares one; a value that
/// does not parse is passed through as it stands, since an unparseable date is better shown
/// verbatim than guessed at.
fn readable_date(date: &str) -> String {
    let Some(parsed) = PdfDate::parse(date.as_bytes()) else {
        return date.to_string();
    };
    let zone = match parsed.utc_offset_minutes {
        Some(0) => " UTC".to_string(),
        Some(offset) => format!(
            " UTC{}{:02}:{:02}",
            if offset < 0 { '-' } else { '+' },
            offset.abs() / 60,
            offset.abs() % 60
        ),
        None => String::new(),
    };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}{zone}",
        parsed.year, parsed.month, parsed.day, parsed.hour, parsed.minute, parsed.second
    )
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

/// Format a coordinate for a content stream: two decimals, without the trailing zeros that make
/// an appearance stream tedious to read against a specification.
///
/// A non-finite value formats as `0` rather than as `inf` or `NaN`, neither of which is a number
/// in PDF syntax (§7.3.3). The callers resolve their geometry before they get here; this is the
/// last line of defence for a stream that, once signed, cannot be corrected in place.
fn num(value: f32) -> String {
    if !value.is_finite() {
        return "0".to_string();
    }
    let text = format!("{value:.2}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text.is_empty() || text == "-" {
        "0".to_string()
    } else {
        text.to_string()
    }
}

/// Greedy-wrap one caption line to `max_width` points, measured in the caption's own Standard-14
/// face (§9.6.2.2 metrics).
///
/// A single word wider than the box keeps its own line rather than being broken mid-word: the box
/// is a signature widget a few hundred points wide, and a fiscal code split across two lines is
/// harder to read than one that overhangs. A face with no metrics table — `Symbol`,
/// `ZapfDingbats` — measures as zero, so everything fits and nothing wraps.
fn wrap_caption_line(line: &str, style: &CaptionStyle, max_width: f64) -> Vec<String> {
    let base = style.font.base_name();
    let size = f64::from(style.resolved_size());
    let measure = |text: &str| pdf_fonts::standard_text_width(base, text, size).unwrap_or(0.0);
    if max_width <= 0.0 || measure(line) <= max_width {
        return vec![line.to_string()];
    }
    let mut wrapped = Vec::new();
    let mut current = String::new();
    for word in line.split_whitespace() {
        if current.is_empty() {
            current.push_str(word);
            continue;
        }
        let candidate = format!("{current} {word}");
        if measure(&candidate) <= max_width {
            current = candidate;
        } else {
            wrapped.push(std::mem::take(&mut current));
            current.push_str(word);
        }
    }
    if !current.is_empty() {
        wrapped.push(current);
    }
    if wrapped.is_empty() {
        wrapped.push(line.to_string());
    }
    wrapped
}

/// Build the appearance Form XObject (§12.5.5 / §8.10): a `/BBox`-bounded box drawing `lines` in
/// the caption's face and, when given, `image` — `(object id, width, height)` of an image XObject
/// — fitted into whatever the caption leaves. The widget maps this BBox onto its `/Rect`.
///
/// The three decisions run in order, because each depends on the one before:
/// [`CaptionPlacement`] decides how wide the caption is, that width decides how it wraps, and the
/// wrapped line count decides how much height the caption claims back from the graphic.
pub(super) fn appearance_xobject(
    width: f32,
    height: f32,
    font_id: ObjectId,
    lines: &[String],
    style: &CaptionStyle,
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

    let pad = 2.0f32;
    let leading = style.resolved_leading();
    let beside = image.is_some() && style.placement == CaptionPlacement::Beside;
    // The graphic keeps the left 40% when the caption sits beside it; otherwise the caption has
    // the full width, less the padding on each side.
    let image_slot = if beside { width * 0.4 } else { 0.0 };
    let caption_width = (width - image_slot - pad - pad).max(1.0);

    let drawn: Vec<String> = if lines.is_empty() {
        Vec::new()
    } else if style.wrap {
        lines
            .iter()
            .flat_map(|line| wrap_caption_line(line, style, f64::from(caption_width)))
            .collect()
    } else {
        lines.to_vec()
    };

    let mut content = Vec::new();
    // The band the caption claims off the bottom, which only `Below` takes out of the graphic.
    let caption_band = if drawn.is_empty() || beside {
        0.0
    } else {
        drawn.len() as f32 * leading + pad
    };
    let (mut text_x, mut text_top) = (pad, height);
    if let Some((_, image_width, image_height)) = image {
        let (slot_x, slot_w, slot_y, slot_h) = if drawn.is_empty() {
            (0.0, width, 0.0, height)
        } else if beside {
            (0.0, image_slot, 0.0, height)
        } else {
            (0.0, width, caption_band, height - caption_band)
        };
        let (avail_w, avail_h) = ((slot_w - 2.0 * pad).max(1.0), (slot_h - 2.0 * pad).max(1.0));
        let scale = (avail_w / image_width.max(1) as f32).min(avail_h / image_height.max(1) as f32);
        let (drawn_w, drawn_h) = (image_width as f32 * scale, image_height as f32 * scale);
        // Image space is the unit square (§8.9.4), so the `cm` carries both size and position.
        let (x, y) = (
            slot_x + pad + (avail_w - drawn_w) / 2.0,
            slot_y + pad + (avail_h - drawn_h) / 2.0,
        );
        content.extend_from_slice(
            format!(
                "q\n{} 0 0 {} {} {} cm\n/Im0 Do\nQ\n",
                num(drawn_w),
                num(drawn_h),
                num(x),
                num(y)
            )
            .as_bytes(),
        );
        if beside {
            text_x = image_slot + pad;
        } else if !drawn.is_empty() {
            text_top = caption_band;
        }
    }
    if drawn.is_empty() {
        return Stream::new(dict, content);
    }

    // Draw each line top-to-bottom from the top of the caption's own band (§9.4 text operators).
    content.extend_from_slice(
        format!(
            "BT\n/Helv {} Tf\n{} TL\n",
            num(style.resolved_size()),
            num(leading)
        )
        .as_bytes(),
    );
    content.extend_from_slice(
        format!(
            "{} {} Td\n",
            num(text_x),
            num((text_top - leading).max(0.0))
        )
        .as_bytes(),
    );
    for (i, line) in drawn.iter().enumerate() {
        if i > 0 {
            content.extend_from_slice(b"T*\n");
        }
        content.push(b'(');
        // The caption is shown with a `/WinAnsiEncoding` simple font, so the literal string holds
        // cp1252 codes rather than the caller's UTF-8 bytes (§9.6.6.1). Every other text path in
        // the engine encodes at exactly this point; this one used to hand the bytes over raw.
        escape_literal_string(&pdf_fonts::winansi_encode(line), &mut content);
        content.extend_from_slice(b") Tj\n");
    }
    content.extend_from_slice(b"ET");
    Stream::new(dict, content)
}

/// The standard-14 font object the caption is drawn with (§9.6.2.2).
///
/// `/WinAnsiEncoding` (§9.6.6.1) is what makes the caption's bytes legible: without the entry a
/// viewer falls back to the font's built-in StandardEncoding, and a caption carrying anything
/// outside ASCII is shown as the wrong glyphs. `Symbol` and `ZapfDingbats` keep their built-in
/// encodings and must not be re-encoded, so they do not get the entry.
pub(super) fn caption_font(font: StdFont) -> Dictionary {
    let mut dict = Dictionary::new();
    dict.insert(Name::from("Type"), Object::Name(Name::from("Font")));
    dict.insert(Name::from("Subtype"), Object::Name(Name::from("Type1")));
    dict.insert(
        Name::from("BaseFont"),
        Object::Name(Name::from(font.base_name())),
    );
    if !matches!(font, StdFont::Symbol | StdFont::ZapfDingbats) {
        dict.insert(
            Name::from("Encoding"),
            Object::Name(Name::from("WinAnsiEncoding")),
        );
    }
    dict
}
