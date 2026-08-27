use super::*;

/// Emit one annotation object (§12.5) and return its id. Built PDF/A-clean (§6.3 of ISO 19005): the
/// `/F` flag has only the Print bit set; a link gets a permitted `URI`/`GoTo` action (§6.5.1) and no
/// visible border; a note gets a normal appearance stream (a Form XObject), required for non-link
/// subtypes (§6.3.3).
#[allow(clippy::too_many_arguments)]
pub(super) fn emit_annotation(
    spec: &AnnotationSpec,
    files: &[Attachment],
    page_id: ObjectId,
    page_ids: &[ObjectId],
    alloc: &mut dyn FnMut() -> ObjectId,
    objects: &mut Vec<(ObjectId, Object)>,
    af_sink: &mut Vec<(String, ObjectId)>,
    sd_requests: &mut Vec<(ObjectId, Vec<u8>)>,
    dp_requests: &mut Vec<(ObjectId, usize)>,
) -> ObjectId {
    const PRINT_FLAG: i64 = 4; // §12.5.3 bit 3 (Print); Hidden/Invisible/ToggleNoView/NoView clear.
    // Associated files (`/AF`, §14.13.9, PDF 2.0): emit the filespecs once and reference them from
    // whichever annotation subtype this is.
    let af = emit_af_array(files, alloc, objects, af_sink);
    match spec {
        AnnotationSpec::Link {
            rect,
            target,
            contents,
        } => {
            let mut action = Dictionary::new();
            action.insert(Name::from("Type"), Object::Name(Name::from("Action")));
            let mut sd_target: Option<Vec<u8>> = None;
            let mut dp_target: Option<usize> = None;
            match target {
                LinkTarget::Uri(uri) => {
                    action.insert(Name::from("S"), Object::Name(Name::from("URI")));
                    action.insert(
                        Name::from("URI"),
                        Object::String(PdfString::from(uri.as_bytes().to_vec())),
                    );
                }
                LinkTarget::Page(idx) => {
                    let target_id = page_ids[(*idx).min(page_ids.len() - 1)];
                    let dest = Array::from(vec![
                        Object::Reference(target_id),
                        Object::Name(Name::from("Fit")),
                    ]);
                    action.insert(Name::from("S"), Object::Name(Name::from("GoTo")));
                    action.insert(Name::from("D"), Object::Array(dest));
                }
                LinkTarget::Element(target) => {
                    // A structure destination (§12.3.2.3, PDF 2.0): the /SD (and the /D fallback
                    // retargeted to the element's page) are patched in once the structure tree is
                    // emitted; until then /D safely points at the link's own page.
                    action.insert(Name::from("S"), Object::Name(Name::from("GoTo")));
                    action.insert(
                        Name::from("D"),
                        Object::Array(Array::from(vec![
                            Object::Reference(page_id),
                            Object::Name(Name::from("Fit")),
                        ])),
                    );
                    sd_target = Some(target.as_bytes().to_vec());
                }
                LinkTarget::DocumentPart(part_index) => {
                    // A GoToDp action (§12.6.4.5, PDF 2.0): the required /Dp reference is patched
                    // in once the DPart leaves are emitted (they come after the annotations).
                    action.insert(Name::from("S"), Object::Name(Name::from("GoToDp")));
                    dp_target = Some(*part_index);
                }
            }
            let id = alloc();
            if let Some(target) = sd_target {
                sd_requests.push((id, target));
            }
            if let Some(part_index) = dp_target {
                dp_requests.push((id, part_index));
            }
            let mut d = Dictionary::new();
            d.insert(Name::from("Type"), Object::Name(Name::from("Annot")));
            d.insert(Name::from("Subtype"), Object::Name(Name::from("Link")));
            d.insert(Name::from("Rect"), rect_array(rect));
            d.insert(Name::from("F"), Object::Integer(PRINT_FLAG));
            if let Some(contents) = contents {
                // The link's alternate description (§12.5.2 — PDF/UA 14289-1 §7.18.5).
                d.insert(
                    Name::from("Contents"),
                    Object::String(PdfString::from(text_string(contents))),
                );
            }
            // Suppress the default 1-pt visible border (§12.5.4): [hradius vradius width].
            d.insert(
                Name::from("Border"),
                Object::Array(Array::from(vec![
                    Object::Integer(0),
                    Object::Integer(0),
                    Object::Integer(0),
                ])),
            );
            d.insert(Name::from("P"), Object::Reference(page_id));
            d.insert(Name::from("A"), Object::Dictionary(action));
            if let Some(af) = af {
                d.insert(Name::from("AF"), af);
            }
            objects.push((id, Object::Dictionary(d)));
            id
        }
        AnnotationSpec::Note { rect, contents } => {
            let w = (rect[2] - rect[0]).max(1.0);
            let h = (rect[3] - rect[1]).max(1.0);
            // Appearance Form XObject (§12.5.5): a small yellow note marker drawn in DeviceRGB
            // (admissible via the sRGB OutputIntent). No /Group, no /Subtype2 → PDF/A-clean (§6.2.9).
            let content = format!(
                "0.98 0.86 0.30 rg\n0 0 {w:.2} {h:.2} re\nf\n0 0 0 RG\n0.5 w\n0 0 {w:.2} {h:.2} re\nS\n"
            );
            let ap_id = alloc();
            objects.push((
                ap_id,
                Object::Stream(form_xobject_stream([0.0, 0.0, w, h], content.into_bytes())),
            ));
            let id = alloc();
            let mut d = Dictionary::new();
            d.insert(Name::from("Type"), Object::Name(Name::from("Annot")));
            d.insert(Name::from("Subtype"), Object::Name(Name::from("Text")));
            d.insert(Name::from("Rect"), rect_array(rect));
            d.insert(Name::from("F"), Object::Integer(PRINT_FLAG));
            d.insert(
                Name::from("Contents"),
                Object::String(PdfString::from(text_string(contents))),
            );
            d.insert(Name::from("P"), Object::Reference(page_id));
            // The appearance dictionary must contain only the /N key (§6.3.3 t2).
            let mut ap = Dictionary::new();
            ap.insert(Name::from("N"), Object::Reference(ap_id));
            d.insert(Name::from("AP"), Object::Dictionary(ap));
            if let Some(af) = af {
                d.insert(Name::from("AF"), af);
            }
            objects.push((id, Object::Dictionary(d)));
            id
        }
    }
}

/// Emit one form-field widget object (§12.7.4) and return its id. The dict is a merged field+widget
/// (§12.7.3.1). PDF/A-clean: Print flag set, no `/A`/`/AA` (§6.4.1 t1/t2), and a normal appearance —
/// for a checkbox a `/AP /N` *subdictionary* keyed by appearance state (§6.3.3 t3), drawn as vector
/// graphics so no font is needed.
pub(super) fn emit_form_field(
    spec: &FormFieldSpec,
    files: &[Attachment],
    page_id: ObjectId,
    alloc: &mut dyn FnMut() -> ObjectId,
    objects: &mut Vec<(ObjectId, Object)>,
    af_sink: &mut Vec<(String, ObjectId)>,
) -> ObjectId {
    const PRINT_FLAG: i64 = 4; // §12.5.3 bit 3 (Print).
    // Associated files on a form field (AN002 /AF-anywhere, PDF 2.0): filespecs emitted once,
    // referenced from the merged field/widget dictionary.
    let af = emit_af_array(files, alloc, objects, af_sink);
    match spec {
        FormFieldSpec::Checkbox {
            rect,
            name,
            checked,
            tooltip,
        } => {
            let w = (rect[2] - rect[0]).max(2.0);
            let h = (rect[3] - rect[1]).max(2.0);
            // Both states draw the box border (vector); the On state adds a check mark. Drawn in the
            // BBox [0 0 w h], in DeviceGray — no font, so PDF/A-safe regardless of embedding.
            let border = format!(
                "q\n0.6 w\n0 G\n0.5 0.5 {:.2} {:.2} re\nS\n",
                w - 1.0,
                h - 1.0
            );
            let check = format!(
                "1.2 w\n{:.2} {:.2} m\n{:.2} {:.2} l\n{:.2} {:.2} l\nS\n",
                0.22 * w,
                0.52 * h,
                0.42 * w,
                0.28 * h,
                0.78 * w,
                0.74 * h,
            );
            let on_id = alloc();
            objects.push((
                on_id,
                Object::Stream(form_xobject_stream(
                    [0.0, 0.0, w, h],
                    format!("{border}{check}Q\n").into_bytes(),
                )),
            ));
            let off_id = alloc();
            objects.push((
                off_id,
                Object::Stream(form_xobject_stream(
                    [0.0, 0.0, w, h],
                    format!("{border}Q\n").into_bytes(),
                )),
            ));

            let state = if *checked { "On" } else { "Off" };
            let id = alloc();
            let mut d = Dictionary::new();
            d.insert(Name::from("Type"), Object::Name(Name::from("Annot")));
            d.insert(Name::from("Subtype"), Object::Name(Name::from("Widget")));
            d.insert(Name::from("FT"), Object::Name(Name::from("Btn")));
            d.insert(
                Name::from("T"),
                Object::String(PdfString::from(text_string(name))),
            );
            if let Some(tooltip) = tooltip {
                // The alternate field name (§12.7.3.1), read by assistive technology (PDF/UA-1),
                // doubled as the annotation's /Contents (§12.5.2) — PDF/UA-2 §8.10.2.3 requires a
                // Contents description when the widget has no Lbl label in the structure tree.
                d.insert(
                    Name::from("TU"),
                    Object::String(PdfString::from(text_string(tooltip))),
                );
                d.insert(
                    Name::from("Contents"),
                    Object::String(PdfString::from(text_string(tooltip))),
                );
            }
            d.insert(Name::from("Rect"), rect_array(rect));
            d.insert(Name::from("F"), Object::Integer(PRINT_FLAG));
            d.insert(Name::from("P"), Object::Reference(page_id));
            d.insert(Name::from("V"), Object::Name(Name::from(state)));
            d.insert(Name::from("AS"), Object::Name(Name::from(state)));
            // /AP /N is a subdictionary keyed by appearance state (§6.3.3 t3 requires this for Btn).
            let mut n = Dictionary::new();
            n.insert(Name::from("On"), Object::Reference(on_id));
            n.insert(Name::from("Off"), Object::Reference(off_id));
            let mut ap = Dictionary::new();
            ap.insert(Name::from("N"), Object::Dictionary(n));
            d.insert(Name::from("AP"), Object::Dictionary(ap));
            if let Some(af) = af.clone() {
                d.insert(Name::from("AF"), af);
            }
            objects.push((id, Object::Dictionary(d)));
            id
        }
    }
}

/// A `[llx lly urx ury]` rectangle array (§7.9.5).
pub(super) fn rect_array(rect: &[f64; 4]) -> Object {
    Object::Array(Array::from(
        rect.iter().map(|&v| Object::Real(v)).collect::<Vec<_>>(),
    ))
}

/// A Form XObject stream (§8.10) — used here for annotation appearance streams (§12.5.5). `FormType 1`
/// with a bounding box, no transparency group and no PostScript `/Subtype2` (PDF/A §6.2.9).
/// FlateDecode-compressed; empty `/Resources` (the marker uses only device colour).
pub(super) fn form_xobject_stream(bbox: [f64; 4], content: Vec<u8>) -> Stream {
    let mut dict = Dictionary::new();
    dict.insert(Name::from("Type"), Object::Name(Name::from("XObject")));
    dict.insert(Name::from("Subtype"), Object::Name(Name::from("Form")));
    dict.insert(Name::from("FormType"), Object::Integer(1));
    dict.insert(
        Name::from("BBox"),
        Object::Array(Array::from(
            bbox.iter().map(|&v| Object::Real(v)).collect::<Vec<_>>(),
        )),
    );
    dict.insert(
        Name::from("Resources"),
        Object::Dictionary(Dictionary::new()),
    );
    dict.insert(
        Name::from("Filter"),
        Object::Name(Name::from("FlateDecode")),
    );
    Stream::new(dict, flate_encode(&content))
}
