//! Save / round-trip / incremental-update tests (§7.5).

use super::super::*;
use super::{assemble, classic_three_page_pdf};

#[test]
fn save_round_trips_a_document() {
    // Load → save → reload: the page tree and catalog survive a full rewrite (§7.5).
    let original = Document::open(classic_three_page_pdf()).unwrap();
    let saved = original.save().unwrap();

    let reloaded = Document::open(saved).unwrap();
    assert_eq!(reloaded.page_count().unwrap(), 3);
    assert_eq!(
        reloaded.catalog().unwrap().get_name(&Name::from("Type")),
        Some(&Name::from("Catalog"))
    );
}

#[test]
fn save_preserves_content_streams() {
    let objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>".to_vec(),
        b"<< /Length 13 >>\nstream\nBT (hi) Tj ET\nendstream".to_vec(),
    ];
    let doc = Document::open(assemble(&objects, "")).unwrap();
    let saved = doc.save().unwrap();

    let reloaded = Document::open(saved).unwrap();
    let page = reloaded.pages().unwrap().remove(0);
    assert_eq!(
        reloaded.page_content_bytes(&page).unwrap(),
        b"BT (hi) Tj ET"
    );
}

#[test]
fn save_preserves_unmodelled_objects_losslessly() {
    // M11 round-trip fidelity (§7.5): a full rewrite must re-emit *every* live object, not just the
    // page tree — annotations, outlines and other objects the DOM never models included. The whole
    // (object number → value) set must come back byte-identical after save → reopen.
    let objects = vec![
        // 1: catalog references an outline tree the page walk never visits.
        b"<< /Type /Catalog /Pages 2 0 R /Outlines 6 0 R >>".to_vec(),
        // 2: page tree.
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        // 3: page carries an annotation via /Annots.
        b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Annots [5 0 R] >>".to_vec(),
        // 4: a content stream (raw bytes + /Length must survive).
        b"<< /Length 13 >>\nstream\nBT (hi) Tj ET\nendstream".to_vec(),
        // 5: a text annotation — unmodelled, with a string holding non-ASCII bytes.
        b"<< /Type /Annot /Subtype /Text /Rect [0 0 20 20] /Contents (caf\xe9) >>".to_vec(),
        // 6: an outline dictionary — unmodelled, reachable only from the catalog.
        b"<< /Type /Outlines /Count 0 >>".to_vec(),
    ];
    let doc = Document::open(assemble(&objects, "")).unwrap();

    let before: std::collections::BTreeMap<u32, Object> = doc
        .collect_objects()
        .unwrap()
        .into_iter()
        .map(|(id, obj)| (id.number, obj))
        .collect();

    let reloaded = Document::open(doc.save().unwrap()).unwrap();
    let after: std::collections::BTreeMap<u32, Object> = reloaded
        .collect_objects()
        .unwrap()
        .into_iter()
        .map(|(id, obj)| (id.number, obj))
        .collect();

    // Nothing dropped, added, renumbered or altered.
    assert_eq!(before, after, "object set changed across save");
    // Spot-check the two unmodelled objects specifically.
    let annotation = after[&5].as_dict().expect("annotation survives");
    assert_eq!(
        annotation.get_name(&Name::from("Subtype")),
        Some(&Name::from("Text"))
    );
    let outline = after[&6].as_dict().expect("outline survives");
    assert_eq!(
        outline.get_name(&Name::from("Type")),
        Some(&Name::from("Outlines"))
    );
}

#[test]
fn save_compact_round_trips_via_xref_stream() {
    let objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>".to_vec(),
        b"<< /Length 13 >>\nstream\nBT (hi) Tj ET\nendstream".to_vec(),
    ];
    let doc = Document::open(assemble(&objects, "")).unwrap();
    let saved = doc.save_compact().unwrap();

    // It really is a cross-reference stream: parseable strictly (no recovery) with a /Type
    // /XRef and no classic `trailer` keyword, and at least PDF 1.5.
    assert!(saved.starts_with(b"%PDF-1.5") || saved.starts_with(b"%PDF-1.7"));
    assert!(saved.windows(b"trailer".len()).all(|w| w != b"trailer"));
    let xref = XRef::parse(&saved).unwrap();
    assert!(xref.root().is_some());

    let reloaded = Document::open(saved).unwrap();
    assert_eq!(reloaded.page_count().unwrap(), 1);
    let page = reloaded.pages().unwrap().remove(0);
    assert_eq!(
        reloaded.page_content_bytes(&page).unwrap(),
        b"BT (hi) Tj ET"
    );
}

#[test]
fn save_packed_round_trips_via_object_streams() {
    // save_packed (§7.5.7) stores the non-stream objects in an ObjStm container while the
    // content stream stays a normal indirect object; everything reads back intact.
    let objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>".to_vec(),
        b"<< /Length 13 >>\nstream\nBT (hi) Tj ET\nendstream".to_vec(),
    ];
    let doc = Document::open(assemble(&objects, "")).unwrap();
    let saved = doc.save_packed().unwrap();

    // At least PDF 1.5, no classic trailer keyword, and a real /ObjStm container present.
    assert!(saved.starts_with(b"%PDF-1.5") || saved.starts_with(b"%PDF-1.7"));
    assert!(saved.windows(b"trailer".len()).all(|w| w != b"trailer"));
    assert!(
        saved.windows(b"/ObjStm".len()).any(|w| w == b"/ObjStm"),
        "output carries an object stream"
    );
    let xref = XRef::parse(&saved).unwrap();
    assert!(xref.root().is_some());

    let reloaded = Document::open(saved).unwrap();
    assert_eq!(reloaded.page_count().unwrap(), 1);
    let page = reloaded.pages().unwrap().remove(0);
    assert_eq!(
        reloaded.page_content_bytes(&page).unwrap(),
        b"BT (hi) Tj ET"
    );
    // Every original object value survives the packing (the container explodes on re-save).
    assert_eq!(reloaded.save().unwrap(), doc.save().unwrap());
}

#[test]
fn save_packed_spills_into_multiple_containers() {
    // More compressible objects than one container holds (100, §7.5.7 cap) → several /ObjStm
    // objects, all still reachable: 150 single-page-referencing outline-ish dicts.
    let mut objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Annots [".to_vec(),
    ];
    // The page's /Annots array references objects 4..154 so they stay live.
    let mut page = objects.pop().unwrap();
    for n in 4..154 {
        page.extend_from_slice(format!("{n} 0 R ").as_bytes());
    }
    page.extend_from_slice(b"] >>");
    objects.push(page);
    for n in 4..154 {
        objects.push(
            format!("<< /Type /Annot /Subtype /Text /Rect [0 0 1 1] /Contents (n{n}) >>")
                .into_bytes(),
        );
    }
    let doc = Document::open(assemble(&objects, "")).unwrap();
    let saved = doc.save_packed().unwrap();
    let containers = saved
        .windows(b"/ObjStm".len())
        .filter(|w| w == b"/ObjStm")
        .count();
    assert!(
        containers >= 2,
        "expected multiple ObjStm, got {containers}"
    );
    let reloaded = Document::open(saved).unwrap();
    let page = reloaded.pages().unwrap().remove(0);
    assert_eq!(reloaded.annotations(&page).unwrap().len(), 150);
}

#[test]
fn save_repairs_a_broken_file() {
    // A file with smashed xref offsets opens via recovery; saving emits a clean classic file
    // that reopens through the normal (non-recovery) path with the right page count.
    let mut pdf = classic_three_page_pdf();
    let xref_at = pdf.windows(4).position(|w| w == b"xref").unwrap();
    for byte in &mut pdf[xref_at..] {
        if byte.is_ascii_digit() {
            *byte = b'0';
        }
    }
    let recovered = Document::open(pdf).unwrap();
    let saved = recovered.save().unwrap();

    // The saved file must parse strictly (no recovery needed) and keep all three pages.
    let xref = XRef::parse(&saved).unwrap();
    assert!(xref.root().is_some());
    assert_eq!(Document::open(saved).unwrap().page_count().unwrap(), 3);
}

#[test]
fn incremental_update_preserves_unmodelled_objects() {
    // M11 incremental-save fidelity (§7.5.6): an append-only update that overrides one object must
    // leave every other object — including ones the DOM never models — reachable and unchanged.
    let objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Annots [4 0 R] >>".to_vec(),
        b"<< /Type /Annot /Subtype /Text /Contents (keep me) >>".to_vec(),
    ];
    let doc = Document::open(assemble(&objects, "")).unwrap();
    let before = live_object_numbers(&doc);

    // Override the page (object 3) with a rotation, via an incremental update.
    let page_id = ObjectId::new(3, 0);
    let Object::Dictionary(mut page) = doc.get(page_id).unwrap() else {
        unreachable!()
    };
    page.insert(Name::from("Rotate"), Object::Integer(90));
    let updated = doc
        .save_incremental(&[(page_id, Object::Dictionary(page))])
        .unwrap();

    let reopened = Document::open(updated).unwrap();
    // The override took effect…
    assert_eq!(
        reopened.pages().unwrap()[0].get_integer(&Name::from("Rotate")),
        Some(90)
    );
    // …the unmodelled annotation (object 4) survived…
    let Object::Dictionary(annot) = reopened.get(ObjectId::new(4, 0)).unwrap() else {
        panic!("annotation lost in incremental update");
    };
    assert_eq!(
        annot.get_name(&Name::from("Subtype")),
        Some(&Name::from("Text"))
    );
    // …and no object was dropped.
    assert_eq!(live_object_numbers(&reopened), before);
}

/// The set of a document's live (in-use) object numbers.
fn live_object_numbers(doc: &Document) -> std::collections::BTreeSet<u32> {
    doc.live_objects()
        .unwrap()
        .into_iter()
        .map(|(id, _)| id.number)
        .collect()
}

#[test]
fn incremental_update_appends_and_overrides() {
    let base = classic_three_page_pdf();
    let doc = Document::open(base.clone()).unwrap();

    // Modify the stored object for the first page: add /Rotate 90.
    let (id, _) = doc.page_entries().unwrap()[0].clone();
    let id = id.unwrap();
    let Object::Dictionary(mut page) = doc.get(id).unwrap() else {
        panic!("page should be a dictionary");
    };
    page.insert(Name::from("Rotate"), Object::Integer(90));

    let updated = doc
        .save_incremental(&[(id, Object::Dictionary(page))])
        .unwrap();

    // Append-only: the original file is preserved verbatim as a prefix.
    assert!(updated.starts_with(&base));
    assert!(updated.len() > base.len());

    // Reopening sees the override via the /Prev chain, with all pages intact.
    let reopened = Document::open(updated).unwrap();
    assert_eq!(reopened.page_count().unwrap(), 3);
    assert_eq!(
        reopened.pages().unwrap()[0].get_integer(&Name::from("Rotate")),
        Some(90)
    );
    // Untouched pages still have no rotation.
    assert_eq!(
        reopened.pages().unwrap()[1].get_integer(&Name::from("Rotate")),
        None
    );
}

/// A one-page document, optionally with extra trailer entries (e.g. an `/ID`).
fn one_page_pdf(trailer_extra: &str) -> Vec<u8> {
    let objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>".to_vec(),
        b"<< /Length 13 >>\nstream\nBT (hi) Tj ET\nendstream".to_vec(),
    ];
    assemble(&objects, trailer_extra)
}

/// The trailer `/ID` is **required** from PDF 2.0 on (ISO 32000-2 §7.5.5, Table 15). Targeting 2.0
/// with [`Document::save_as`] therefore has to produce one even though the input has none —
/// otherwise the file the version gate just approved is not a valid 2.0 file.
#[test]
fn save_as_2_0_emits_the_required_file_id() {
    let doc = Document::open(one_page_pdf("")).unwrap();
    let saved = doc.save_as(2, 0).unwrap();
    assert!(saved.starts_with(b"%PDF-2.0"));

    let reopened = Document::open(saved).unwrap();
    let id = reopened
        .xref
        .trailer
        .get(&Name::from("ID"))
        .and_then(Object::as_array)
        .expect("a 2.0 trailer must carry /ID");
    assert_eq!(id.len(), 2, "/ID is a two-element array (§14.4)");
    assert_eq!(
        id[0].as_string().unwrap().len(),
        16,
        "a synthesized identifier is 16 bytes"
    );
}

/// §14.4: the first `/ID` element "shall not change" for the life of the file. Every full-rewrite
/// path must therefore carry the input's identity forward rather than mint a new one.
#[test]
fn full_rewrites_preserve_the_permanent_file_id() {
    let doc = Document::open(one_page_pdf("/ID [<0011223344556677> <8899aabbccddeeff>]")).unwrap();
    let permanent: &[u8] = &[0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77];

    for (label, saved) in [
        ("save", doc.save().unwrap()),
        ("save_as", doc.save_as(2, 0).unwrap()),
        ("save_compact", doc.save_compact().unwrap()),
        ("save_packed", doc.save_packed().unwrap()),
    ] {
        let reopened = Document::open(saved).unwrap();
        let id = reopened
            .xref
            .trailer
            .get(&Name::from("ID"))
            .and_then(Object::as_array)
            .unwrap_or_else(|| panic!("{label} dropped /ID"));
        assert_eq!(
            id[0].as_string().unwrap().as_bytes(),
            permanent,
            "{label} must keep the permanent identifier (§14.4)"
        );
    }
}

/// Below 2.0 `/ID` is only "strongly recommended" (ISO 32000-1, Table 15), so a save that has
/// nothing to preserve leaves the trailer as it was — no gratuitous byte churn.
#[test]
fn pre_2_0_save_adds_no_file_id() {
    let doc = Document::open(one_page_pdf("")).unwrap();
    let saved = doc.save_as(1, 7).unwrap();
    assert!(!saved.windows(3).any(|w| w == b"/ID"), "unexpected /ID");
}
