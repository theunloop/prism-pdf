//! Interactive form (AcroForm) field reading (§12.7).

use super::super::*;
use super::assemble;

#[test]
fn reads_flat_text_and_button_fields() {
    let objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R /AcroForm 6 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R >>".to_vec(),
        // A text field with a value.
        b"<< /FT /Tx /T (full_name) /V (Alice) >>".to_vec(),
        // A checkbox whose value is the selected state name.
        b"<< /FT /Btn /T (subscribe) /V /Yes >>".to_vec(),
        b"<< /Fields [4 0 R 5 0 R] >>".to_vec(),
    ];
    let doc = Document::open(assemble(&objects, "")).unwrap();
    let fields = doc.form_fields().unwrap();

    assert_eq!(fields.len(), 2);
    assert_eq!(fields[0].name, "full_name");
    assert_eq!(fields[0].field_type, "Tx");
    assert_eq!(fields[0].value.as_deref(), Some("Alice"));
    assert_eq!(fields[1].name, "subscribe");
    assert_eq!(fields[1].field_type, "Btn");
    assert_eq!(fields[1].value.as_deref(), Some("Yes"));
}

#[test]
fn builds_qualified_names_and_inherits_type() {
    // A parent groups two children that carry only their partial name /T; the type /Tx and the
    // value (Anytown) are inherited from the parent (§12.7.3.2 / value inheritance).
    let objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R /AcroForm 6 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R >>".to_vec(),
        // Parent field: name "address", type Tx, a shared value, two field children.
        b"<< /T (address) /FT /Tx /V (Anytown) /Kids [5 0 R 7 0 R] >>".to_vec(),
        b"<< /T (city) >>".to_vec(),
        b"<< /Fields [4 0 R] >>".to_vec(),
        b"<< /T (zip) /V (12345) >>".to_vec(),
    ];
    let doc = Document::open(assemble(&objects, "")).unwrap();
    let fields = doc.form_fields().unwrap();

    assert_eq!(fields.len(), 2);
    // city inherits type Tx and the parent's value.
    assert_eq!(fields[0].name, "address.city");
    assert_eq!(fields[0].field_type, "Tx");
    assert_eq!(fields[0].value.as_deref(), Some("Anytown"));
    // zip overrides the value but still inherits the type and the qualified-name prefix.
    assert_eq!(fields[1].name, "address.zip");
    assert_eq!(fields[1].field_type, "Tx");
    assert_eq!(fields[1].value.as_deref(), Some("12345"));
}

#[test]
fn fill_form_sets_values_via_incremental_update() {
    let objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R /AcroForm 6 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R >>".to_vec(),
        b"<< /FT /Tx /T (full_name) /V (Alice) >>".to_vec(),
        b"<< /FT /Btn /T (subscribe) /V /Yes >>".to_vec(),
        b"<< /Fields [4 0 R 5 0 R] >>".to_vec(),
    ];
    let original = assemble(&objects, "");
    let doc = Document::open(original.clone()).unwrap();

    let report = doc
        .fill_form_with_report(&[("full_name", "Bob"), ("subscribe", "Off")])
        .unwrap();
    assert_eq!(report.rewrite_mode(), RewriteMode::Incremental);
    assert_eq!(report.signature_effect(), SignatureEffect::Preserved);
    assert_eq!(report.structure_effect(), StructureEffect::Preserved);
    let filled = report.into_bytes();
    // Append-only incremental update (§7.5.6): the original bytes are preserved as a prefix.
    assert!(filled.starts_with(&original));

    let reopened = Document::open(filled).unwrap();
    let fields = reopened.form_fields().unwrap();
    let value = |name: &str| {
        fields
            .iter()
            .find(|f| f.name == name)
            .and_then(|f| f.value.clone())
    };
    assert_eq!(value("full_name").as_deref(), Some("Bob"));
    assert_eq!(value("subscribe").as_deref(), Some("Off"));

    // NeedAppearances is set so viewers regenerate the field appearances.
    let catalog = reopened.catalog().unwrap();
    let acroform_ref = catalog.get(&Name::from("AcroForm")).unwrap();
    let Object::Dictionary(acroform) = reopened.resolve(acroform_ref).unwrap() else {
        panic!("AcroForm missing");
    };
    assert_eq!(
        acroform.get(&Name::from("NeedAppearances")),
        Some(&Object::Boolean(true))
    );
}

#[test]
fn button_only_fill_skips_need_appearances() {
    // A fill touching only a button switches its state via /AS against the existing /AP —
    // /NeedAppearances (deprecated in PDF 2.0, §12.7.3) must not be set.
    let objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R /AcroForm 5 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R >>".to_vec(),
        b"<< /FT /Btn /T (subscribe) /V /Yes >>".to_vec(),
        b"<< /Fields [4 0 R] >>".to_vec(),
    ];
    let doc = Document::open(assemble(&objects, "")).unwrap();
    let filled = doc.fill_form(&[("subscribe", "Off")]).unwrap();

    let reopened = Document::open(filled).unwrap();
    let fields = reopened.form_fields().unwrap();
    assert_eq!(fields[0].value.as_deref(), Some("Off"));
    let catalog = reopened.catalog().unwrap();
    let Object::Dictionary(acroform) = reopened
        .resolve(catalog.get(&Name::from("AcroForm")).unwrap())
        .unwrap()
    else {
        panic!("AcroForm missing");
    };
    assert!(acroform.get(&Name::from("NeedAppearances")).is_none());
}

#[test]
fn fill_form_ignores_unknown_field_names() {
    let objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R /AcroForm 6 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R >>".to_vec(),
        b"<< /FT /Tx /T (full_name) >>".to_vec(),
        b"<< /FT /Tx /T (unused) >>".to_vec(),
        b"<< /Fields [4 0 R 5 0 R] >>".to_vec(),
    ];
    let doc = Document::open(assemble(&objects, "")).unwrap();
    // A misspelled name is silently skipped; the valid one still applies.
    let filled = doc
        .fill_form(&[("full_name", "Set"), ("nope", "x")])
        .unwrap();
    let reopened = Document::open(filled).unwrap();
    let fields = reopened.form_fields().unwrap();
    let full_name = fields.iter().find(|f| f.name == "full_name").unwrap();
    assert_eq!(full_name.value.as_deref(), Some("Set"));
}

#[test]
fn no_acroform_yields_no_fields() {
    let objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R >>".to_vec(),
    ];
    let doc = Document::open(assemble(&objects, "")).unwrap();
    assert!(doc.form_fields().unwrap().is_empty());
}

#[test]
fn reports_widget_geometry_from_merged_and_kid_widgets() {
    // Three fields (§12.7.3.1): one merged with its widget and carrying /P; one whose widget is a
    // /Kids entry that names no /P, so the page has to be found from /Annots; one with no widget.
    // The merged widget's /Rect names its upper-right corner first, which /Rect allows (§12.5.2).
    let objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R /AcroForm 9 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Annots [5 0 R] >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Annots [7 0 R] >>".to_vec(),
        b"<< /FT /Tx /T (merged) /Subtype /Widget /Rect [100 50 10 20] /P 3 0 R >>".to_vec(),
        b"<< /FT /Sig /T (kids) /Kids [7 0 R] >>".to_vec(),
        b"<< /Subtype /Widget /Rect [1 2 3 4] /Parent 6 0 R >>".to_vec(),
        b"<< /FT /Tx /T (bare) >>".to_vec(),
        b"<< /Fields [5 0 R 6 0 R 8 0 R] >>".to_vec(),
    ];
    let doc = Document::open(assemble(&objects, "")).unwrap();
    let fields = doc.form_fields().unwrap();
    assert_eq!(fields.len(), 3);

    assert_eq!(fields[0].name, "merged");
    assert_eq!(
        fields[0].rect,
        Some([10.0, 20.0, 100.0, 50.0]),
        "normalised"
    );
    assert_eq!(fields[0].page_index, Some(0), "from /P");

    assert_eq!(fields[1].name, "kids");
    assert_eq!(fields[1].field_type, "Sig");
    assert_eq!(fields[1].rect, Some([1.0, 2.0, 3.0, 4.0]));
    assert_eq!(
        fields[1].page_index,
        Some(1),
        "from the page whose /Annots lists the widget"
    );

    assert_eq!(fields[2].name, "bare");
    assert_eq!(fields[2].rect, None);
    assert_eq!(fields[2].page_index, None);
}

#[test]
fn a_short_rect_array_is_no_rectangle() {
    // A /Rect needs four numbers (§12.5.2). A short array used to be zero-filled, which reported a
    // corner the file never gave — and, on the signing path, laid a signature appearance out over
    // it. It is refused instead; the field itself still lists.
    let objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R /AcroForm 5 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Annots [4 0 R] >>".to_vec(),
        b"<< /FT /Sig /T (sig) /Subtype /Widget /Rect [10 20 300] >>".to_vec(),
        b"<< /Fields [4 0 R] >>".to_vec(),
    ];
    let doc = Document::open(assemble(&objects, "")).unwrap();
    let fields = doc.form_fields().unwrap();

    assert_eq!(fields.len(), 1);
    assert_eq!(fields[0].name, "sig");
    assert_eq!(fields[0].rect, None);
}
