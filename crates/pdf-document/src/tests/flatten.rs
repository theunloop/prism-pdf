//! Form flattening (§12.7.4 / §12.5.5): painting widget appearances into page content.

use super::super::*;
use super::assemble;

/// A stream object body with a correct `/Length`.
fn stream(extra: &str, content: &[u8]) -> Vec<u8> {
    let mut body = format!("<< {extra} /Length {} >>\nstream\n", content.len()).into_bytes();
    body.extend_from_slice(content);
    body.extend_from_slice(b"\nendstream");
    body
}

#[test]
fn flatten_paints_appearance_and_removes_widget_and_acroform() {
    let appearance = b"BT /Helv 12 Tf (Bob) Tj ET";
    let objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R /AcroForm 7 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R \
            /Resources << >> /Annots [5 0 R] >>"
            .to_vec(),
        stream("", b"q Q"),
        // A text widget with a normal appearance stream.
        b"<< /Type /Annot /Subtype /Widget /FT /Tx /T (full_name) /V (Bob) \
            /Rect [10 10 110 30] /AP << /N 6 0 R >> >>"
            .to_vec(),
        stream(
            "/Type /XObject /Subtype /Form /BBox [0 0 100 20]",
            appearance,
        ),
        b"<< /Fields [5 0 R] >>".to_vec(),
    ];
    let doc = Document::open(assemble(&objects, "")).unwrap();

    let flattened = doc.flatten_form().unwrap();
    let out = Document::open(flattened).unwrap();

    // The form is gone: no /AcroForm, no fields.
    assert!(
        out.catalog()
            .unwrap()
            .get(&Name::from("AcroForm"))
            .is_none()
    );
    assert!(out.form_fields().unwrap().is_empty());

    let page = out.pages().unwrap().remove(0);
    // The widget was removed from /Annots (it was the only one).
    assert!(page.get(&Name::from("Annots")).is_none());

    // The appearance is now painted by the page content: a `Do` of the flattened XObject.
    let content = out.page_content_bytes(&page).unwrap();
    let text = String::from_utf8_lossy(&content);
    assert!(text.contains("/PrismFlat0 Do"), "content was: {text}");
    assert!(text.contains("cm"), "expected a placement matrix: {text}");

    // …and the appearance form is wired into the page resources.
    let Object::Dictionary(resources) = out
        .resolve(page.get(&Name::from("Resources")).unwrap())
        .unwrap()
    else {
        panic!("no resources");
    };
    let Object::Dictionary(xobject) = out
        .resolve(resources.get(&Name::from("XObject")).unwrap())
        .unwrap()
    else {
        panic!("no /XObject");
    };
    assert!(xobject.get(&Name::from("PrismFlat0")).is_some());
}

#[test]
fn flatten_keeps_non_widget_annotations() {
    // A link annotation (not a form widget) must survive flattening.
    let objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Annots [4 0 R] >>".to_vec(),
        b"<< /Type /Annot /Subtype /Link /Rect [0 0 10 10] >>".to_vec(),
    ];
    let doc = Document::open(assemble(&objects, "")).unwrap();
    let out = Document::open(doc.flatten_form().unwrap()).unwrap();
    let page = out.pages().unwrap().remove(0);
    let annots = out.annotations(&page).unwrap();
    assert_eq!(annots.len(), 1);
    assert_eq!(annots[0].subtype, "Link");
}
