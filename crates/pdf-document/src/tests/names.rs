//! Name-tree (§7.7.4) and embedded-file attachment (§7.11) reading tests.

use super::super::*;
use super::assemble;

#[test]
fn attachments_round_trip_through_the_builder() {
    // Write two attachments (one with a description), then read them back.
    let mut builder = Builder::new();
    builder.add_page(PageSpec::new(Vec::new()));
    builder.attach_file(Attachment {
        name: "invoice.xml".into(),
        mime: "text/xml".into(),
        relationship: "Data".into(),
        description: Some("the invoice".into()),
        mod_date: None,
        data: b"<invoice>1</invoice>".to_vec(),
    });
    builder.attach_file(Attachment {
        name: "notes.txt".into(),
        mime: "text/plain".into(),
        relationship: "Supplement".into(),
        description: None,
        mod_date: None,
        data: b"hello".to_vec(),
    });

    let doc = Document::open(builder.build()).unwrap();
    let mut attachments = doc.attachments().unwrap();
    attachments.sort_by(|a, b| a.name.cmp(&b.name));
    assert_eq!(attachments.len(), 2);

    let invoice = &attachments[0];
    assert_eq!(invoice.name, "invoice.xml");
    assert_eq!(invoice.data, b"<invoice>1</invoice>");
    assert_eq!(invoice.mime.as_deref(), Some("text/xml"));
    assert_eq!(invoice.relationship.as_deref(), Some("Data"));
    assert_eq!(invoice.description.as_deref(), Some("the invoice"));

    let notes = &attachments[1];
    assert_eq!(notes.name, "notes.txt");
    assert_eq!(notes.data, b"hello");
    assert_eq!(notes.description, None);
}

#[test]
fn page_level_associated_file_is_pdf_2_0() {
    // attach_file_to_page places the filespec in the page's /AF (§14.13.4, PDF 2.0) instead of the
    // catalog — which auto-stamps the header 2.0 — while still listing it in /EmbeddedFiles.
    let mut builder = Builder::new();
    builder
        .add_page(PageSpec::new(Vec::new()))
        .attach_file_to_page(
            0,
            Attachment {
                name: "data.csv".into(),
                mime: "text/csv".into(),
                relationship: "Data".into(),
                description: None,
                mod_date: None,
                data: b"a,b,c\n1,2,3\n".to_vec(),
            },
        );
    let pdf = builder.build();
    assert!(pdf.starts_with(b"%PDF-2.0"), "page /AF → %PDF-2.0");

    let doc = Document::open(pdf).unwrap();
    // The catalog has NO /AF — the association lives on the page.
    let catalog = doc.catalog().unwrap();
    assert!(catalog.get(&Name::from("AF")).is_none(), "no catalog /AF");

    // The page carries an /AF array whose filespec is the embedded file.
    let page = &doc.pages().unwrap()[0];
    let af = page
        .get(&Name::from("AF"))
        .and_then(Object::as_array)
        .expect("page /AF array");
    assert_eq!(af.iter().count(), 1);
    let fs = doc.resolve(af.iter().next().unwrap()).unwrap();
    let fs = fs.as_dict().unwrap();
    assert_eq!(
        fs.get_name(&Name::from("Type")).map(Name::as_bytes),
        Some(&b"Filespec"[..])
    );
    assert_eq!(
        fs.get_name(&Name::from("AFRelationship"))
            .map(Name::as_bytes),
        Some(&b"Data"[..])
    );

    // It is still discoverable as an attachment (in the /EmbeddedFiles name tree).
    let attachments = doc.attachments().unwrap();
    assert_eq!(attachments.len(), 1);
    assert_eq!(attachments[0].name, "data.csv");
    assert_eq!(attachments[0].data, b"a,b,c\n1,2,3\n");
}

#[test]
fn no_names_dictionary_yields_no_attachments() {
    // A plain document (no /Names) returns an empty list rather than erroring.
    let objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R >>".to_vec(),
    ];
    let doc = Document::open(assemble(&objects, "")).unwrap();
    assert!(doc.attachments().unwrap().is_empty());
    assert!(doc.names("EmbeddedFiles").unwrap().is_empty());
}

#[test]
fn names_walks_a_nested_kids_tree() {
    // A two-level /Dests name tree: a root /Kids pointing at two leaves, each with /Names.
    // Keys must come back in tree order across both leaves.
    let objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R /Names << /Dests 4 0 R >> >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R >>".to_vec(),
        b"<< /Kids [5 0 R 6 0 R] >>".to_vec(),
        b"<< /Limits [(a) (b)] /Names [(a) 100 (b) 200] >>".to_vec(),
        b"<< /Limits [(c) (d)] /Names [(c) 300 (d) 400] >>".to_vec(),
    ];
    let doc = Document::open(assemble(&objects, "")).unwrap();
    let entries = doc.names("Dests").unwrap();
    let keys: Vec<&[u8]> = entries.iter().map(|(k, _)| k.as_slice()).collect();
    assert_eq!(keys, [b"a".as_slice(), b"b", b"c", b"d"]);
    // Values are returned unresolved (here direct integers).
    assert_eq!(entries[2].1, Object::Integer(300));
}
