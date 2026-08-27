use super::*;
use crate::Document;

#[test]
fn authors_structure_namespaces_as_pdf_2_0() {
    // A tagged document whose root carries the standard PDF 2.0 structure namespace and whose
    // one element carries a different (PDF/UA-2) namespace — both must surface (§14.7.4).
    const SSN: &str = "http://iso.org/pdf2/ssn";
    const UA2: &str = "http://www.iso.org/pdf2/ssn";
    let content = b"/P <</MCID 0>> BDC BT /F1 12 Tf (Hi) Tj ET EMC".to_vec();
    let mut builder = Builder::new();
    builder
        .add_page(PageSpec::new(content).standard_font("F1", StdFont::Helvetica))
        .lang("en-US")
        .structure_namespace(SSN)
        .structure(vec![{
            let mut p = StructElem::new("P").namespace(UA2);
            p.push_content(0, 0);
            p
        }]);
    let pdf = builder.build();
    // A structure namespace makes the document PDF 2.0 (auto-stamped).
    assert!(pdf.starts_with(b"%PDF-2.0"), "expected %PDF-2.0 header");

    let doc = Document::open(pdf).unwrap();
    let catalog = doc.catalog().unwrap();
    let str_root = doc
        .resolve(catalog.get(&Name::from("StructTreeRoot")).unwrap())
        .unwrap();
    let str_root = str_root.as_dict().unwrap();

    // /StructTreeRoot /Namespaces lists both namespace dictionaries, each /Type /Namespace.
    let namespaces = str_root
        .get(&Name::from("Namespaces"))
        .and_then(Object::as_array)
        .expect("/Namespaces array");
    assert_eq!(namespaces.iter().count(), 2, "two distinct namespaces");
    let uris: Vec<Vec<u8>> = namespaces
        .iter()
        .map(|r| {
            let ns = doc.resolve(r).unwrap();
            let ns = ns.as_dict().unwrap();
            assert_eq!(
                ns.get_name(&Name::from("Type")).map(Name::as_bytes),
                Some(&b"Namespace"[..])
            );
            ns.get(&Name::from("NS"))
                .and_then(Object::as_string)
                .unwrap()
                .as_bytes()
                .to_vec()
        })
        .collect();
    assert!(uris.contains(&SSN.as_bytes().to_vec()));
    assert!(uris.contains(&UA2.as_bytes().to_vec()));

    // The Document root references the SSN namespace via /NS.
    let doc_elem = doc
        .resolve(str_root.get(&Name::from("K")).unwrap())
        .unwrap();
    let doc_elem = doc_elem.as_dict().unwrap();
    let root_ns = doc
        .resolve(doc_elem.get(&Name::from("NS")).unwrap())
        .unwrap();
    assert_eq!(
        root_ns
            .as_dict()
            .unwrap()
            .get(&Name::from("NS"))
            .and_then(Object::as_string)
            .unwrap()
            .as_bytes(),
        SSN.as_bytes()
    );

    // The P element references the UA-2 namespace via /NS.
    let p = doc
        .resolve(doc_elem.get(&Name::from("K")).unwrap())
        .unwrap();
    let p = p.as_dict().unwrap();
    let p_ns = doc.resolve(p.get(&Name::from("NS")).unwrap()).unwrap();
    assert_eq!(
        p_ns.as_dict()
            .unwrap()
            .get(&Name::from("NS"))
            .and_then(Object::as_string)
            .unwrap()
            .as_bytes(),
        UA2.as_bytes()
    );

    // Read-side: Document::structure_namespaces() surfaces both URIs.
    let namespaces = doc.structure_namespaces().unwrap();
    assert!(namespaces.contains(&SSN.to_string()));
    assert!(namespaces.contains(&UA2.to_string()));
    assert_eq!(namespaces.len(), 2);
}

#[test]
fn structure_namespaces_empty_when_untagged() {
    let mut builder = Builder::new();
    builder.add_page(PageSpec::new(Vec::new()));
    let doc = Document::open(builder.build()).unwrap();
    assert!(doc.structure_namespaces().unwrap().is_empty());
}

#[test]
fn authors_associated_file_on_struct_element_as_pdf_2_0() {
    // A tagged Figure carrying an associated file (§14.13.6): the element gets /AF, the file is
    // listed in /EmbeddedFiles, and the header auto-stamps 2.0.
    let content = b"/Figure <</MCID 0>> BDC EMC".to_vec();
    let mut builder = Builder::new();
    builder.add_page(PageSpec::new(content)).structure(vec![{
        let mut fig = StructElem::new("Figure").alt("a chart");
        fig = fig.associate_file(Attachment {
            name: "chart.csv".into(),
            mime: "text/csv".into(),
            relationship: "Supplement".into(),
            description: None,
            mod_date: None,
            data: b"x,y\n1,2\n".to_vec(),
        });
        fig.push_content(0, 0);
        fig
    }]);
    let pdf = builder.build();
    assert!(pdf.starts_with(b"%PDF-2.0"), "struct /AF → %PDF-2.0");

    let doc = Document::open(pdf).unwrap();
    // Find the Figure element under the Document root and check its /AF.
    let catalog = doc.catalog().unwrap();
    let root = doc
        .resolve(catalog.get(&Name::from("StructTreeRoot")).unwrap())
        .unwrap();
    let doc_elem = doc
        .resolve(root.as_dict().unwrap().get(&Name::from("K")).unwrap())
        .unwrap();
    let fig = doc
        .resolve(doc_elem.as_dict().unwrap().get(&Name::from("K")).unwrap())
        .unwrap();
    let fig = fig.as_dict().unwrap();
    assert_eq!(
        fig.get_name(&Name::from("S")).map(Name::as_bytes),
        Some(&b"Figure"[..])
    );
    let af = fig
        .get(&Name::from("AF"))
        .and_then(Object::as_array)
        .expect("Figure /AF");
    assert_eq!(af.iter().count(), 1);

    // The file is discoverable as an attachment (in /EmbeddedFiles).
    let attachments = doc.attachments().unwrap();
    assert_eq!(attachments.len(), 1);
    assert_eq!(attachments[0].name, "chart.csv");
}

#[test]
fn authors_associated_file_on_annotation_as_pdf_2_0() {
    // A link annotation carrying an associated file (§14.13.9): the annotation gets /AF, the file
    // is listed in /EmbeddedFiles, and the header auto-stamps 2.0. An empty file list would not.
    let mut builder = Builder::new();
    builder.add_page(PageSpec::new(Vec::new())).add_annotation(
        0,
        AnnotationSpec::Link {
            rect: [0.0, 0.0, 10.0, 10.0],
            target: LinkTarget::Uri("https://example.org/".into()),
            contents: None,
        },
        vec![Attachment {
            name: "link.csv".into(),
            mime: "text/csv".into(),
            relationship: "Data".into(),
            description: None,
            mod_date: None,
            data: b"a,b\n1,2\n".to_vec(),
        }],
    );
    let pdf = builder.build();
    assert!(pdf.starts_with(b"%PDF-2.0"), "annotation /AF → %PDF-2.0");

    let doc = Document::open(pdf).unwrap();
    // The page's single /Annots entry is a Link carrying /AF.
    let pages = doc.pages().unwrap();
    let annots = pages[0]
        .get(&Name::from("Annots"))
        .and_then(Object::as_array)
        .expect("page /Annots");
    let annot = doc.resolve(annots.iter().next().unwrap()).unwrap();
    let annot = annot.as_dict().unwrap();
    assert_eq!(
        annot.get_name(&Name::from("Subtype")).map(Name::as_bytes),
        Some(&b"Link"[..])
    );
    let af = annot
        .get(&Name::from("AF"))
        .and_then(Object::as_array)
        .expect("annotation /AF");
    assert_eq!(af.iter().count(), 1);

    // Discoverable via the /EmbeddedFiles name tree.
    let attachments = doc.attachments().unwrap();
    assert_eq!(attachments.len(), 1);
    assert_eq!(attachments[0].name, "link.csv");
}

#[test]
fn authors_associated_file_on_form_field_as_pdf_2_0() {
    // A form field carrying an associated file (AN002 /AF-anywhere): the merged field/widget
    // dict gets /AF, the file is in /EmbeddedFiles, and the header auto-stamps 2.0.
    let mut builder = Builder::new();
    builder
        .add_page(PageSpec::new(b"q Q".to_vec()))
        .add_form_field(
            0,
            FormFieldSpec::Checkbox {
                rect: [72.0, 700.0, 90.0, 718.0],
                name: "signed-data".into(),
                checked: false,
                tooltip: None,
            },
            vec![Attachment {
                name: "field.xml".into(),
                mime: "application/xml".into(),
                relationship: "Data".into(),
                description: None,
                mod_date: None,
                data: b"<x/>".to_vec(),
            }],
        );
    let pdf = builder.build();
    assert!(pdf.starts_with(b"%PDF-2.0"), "field /AF → %PDF-2.0");

    let doc = Document::open(pdf).unwrap();
    let catalog = doc.catalog().unwrap();
    let acro = doc
        .resolve(catalog.get(&Name::from("AcroForm")).expect("acroform"))
        .unwrap();
    let flds = acro
        .as_dict()
        .and_then(|a| a.get_array(&Name::from("Fields")).cloned())
        .expect("/Fields");
    let field = doc.resolve(flds.iter().next().unwrap()).unwrap();
    let af = field
        .as_dict()
        .and_then(|f| f.get_array(&Name::from("AF")).cloned())
        .expect("field /AF");
    assert_eq!(af.iter().count(), 1);
    let attachments = doc.attachments().unwrap();
    assert_eq!(attachments.len(), 1);
    assert_eq!(attachments[0].name, "field.xml");
}

#[test]
fn authors_associated_file_on_form_xobject_as_pdf_2_0() {
    // A reusable content form XObject carrying an associated file (§14.13.7): the form's stream
    // dict gets /AF, the file is in /EmbeddedFiles, and the header auto-stamps 2.0.
    let mut builder = Builder::new();
    builder
        .add_page(PageSpec::new(b"/Fx Do".to_vec()))
        .add_form_xobject(
            "Fx",
            [0.0, 0.0, 10.0, 10.0],
            b"0 0 10 10 re f".to_vec(),
            vec![Attachment {
                name: "form.csv".into(),
                mime: "text/csv".into(),
                relationship: "Source".into(),
                description: None,
                mod_date: None,
                data: b"p,q\n3,4\n".to_vec(),
            }],
        );
    let pdf = builder.build();
    assert!(pdf.starts_with(b"%PDF-2.0"), "form /AF → %PDF-2.0");

    let doc = Document::open(pdf).unwrap();
    // Resolve the page → /Resources /XObject /Fx → its stream dict carries /AF.
    let pages = doc.pages().unwrap();
    let resources = doc
        .resolve(pages[0].get(&Name::from("Resources")).unwrap())
        .unwrap();
    let xobjects = doc
        .resolve(
            resources
                .as_dict()
                .unwrap()
                .get(&Name::from("XObject"))
                .unwrap(),
        )
        .unwrap();
    let form = doc
        .resolve(xobjects.as_dict().unwrap().get(&Name::from("Fx")).unwrap())
        .unwrap();
    let af = form
        .as_stream()
        .unwrap()
        .dict()
        .get(&Name::from("AF"))
        .and_then(Object::as_array)
        .expect("form XObject /AF");
    assert_eq!(af.iter().count(), 1);

    let attachments = doc.attachments().unwrap();
    assert_eq!(attachments.len(), 1);
    assert_eq!(attachments[0].name, "form.csv");
}

#[test]
fn authors_utf8_text_strings_as_pdf_2_0() {
    // With utf8_text_strings(), a non-ASCII Info value is emitted as UTF-8 (EF BB BF BOM),
    // a PDF 2.0 text string (§7.9.2.2) — which auto-stamps the header 2.0. Call order is
    // irrelevant: title() is set *before* the flag here.
    let mut builder = Builder::new();
    builder
        .add_page(PageSpec::new(Vec::new()))
        .title("Café — résumé")
        .utf8_text_strings();
    let pdf = builder.build();
    assert!(pdf.starts_with(b"%PDF-2.0"), "UTF-8 strings → %PDF-2.0");

    let doc = Document::open(pdf).unwrap();
    let info = doc.info().unwrap().unwrap();
    let title = info
        .get(&Name::from("Title"))
        .and_then(Object::as_string)
        .unwrap();
    assert_eq!(
        &title.as_bytes()[..3],
        &[0xEF, 0xBB, 0xBF],
        "UTF-8 byte-order mark"
    );
    assert_eq!(&title.as_bytes()[3..], "Café — résumé".as_bytes());
}

#[test]
fn ascii_strings_stay_1_4_even_in_utf8_mode() {
    // An ASCII title carries no BOM and is byte-identical under PDFDocEncoding, so the document
    // stays at its minimum version (1.4) even with the UTF-8 flag set.
    let mut builder = Builder::new();
    builder
        .add_page(PageSpec::new(Vec::new()))
        .utf8_text_strings()
        .title("Plain ASCII");
    let pdf = builder.build();
    assert!(pdf.starts_with(b"%PDF-1.4"), "ASCII stays 1.4");
}

#[test]
fn build_for_stamps_exactly_the_target() {
    // M17 Phase 2: a plain (min 1.4) document targeted at each supported version stamps
    // exactly that version — over-declaring is harmless and the file stays openable.
    for target in [(1u8, 4u8), (1, 5), (1, 6), (1, 7), (2, 0)] {
        let mut builder = Builder::new();
        builder
            .add_page(PageSpec::new(Vec::new()))
            .title("Targeted");
        let pdf = builder.build_for(target.0, target.1).unwrap();
        let header = format!("%PDF-{}.{}", target.0, target.1);
        assert!(
            pdf.starts_with(header.as_bytes()),
            "expected {header} header"
        );
        let doc = Document::open(pdf).unwrap();
        assert_eq!(doc.page_count().unwrap(), 1);
    }
}

#[test]
fn build_for_refuses_a_construct_above_the_target() {
    // M17 Phase 3: document parts (§14.12) are PDF 2.0 — a 1.7 target must be refused with
    // a diagnostic naming the construct, and a 2.0 target must pass.
    let mut builder = Builder::new();
    builder
        .add_page(PageSpec::new(Vec::new()))
        .document_parts(&[DocumentPart {
            first_page: 0,
            last_page: 0,
            dpm: Vec::new(),
        }]);
    let err = builder.build_for(1, 7).unwrap_err();
    let crate::DocError::TargetVersionExceeded {
        construct,
        required,
        target,
    } = &err
    else {
        panic!("expected TargetVersionExceeded, got {err:?}");
    };
    assert_eq!(*required, (2, 0));
    assert_eq!(*target, (1, 7));
    assert!(
        construct.contains("14.12") || construct.to_lowercase().contains("document part"),
        "diagnostic names the construct: {construct}"
    );
    // The Display form carries the whole story for CLI/FFI surfaces.
    let msg = err.to_string();
    assert!(msg.contains("2.0") && msg.contains("1.7"), "message: {msg}");

    let pdf = builder.build_for(2, 0).unwrap();
    assert!(pdf.starts_with(b"%PDF-2.0"));
}

#[test]
fn build_for_downgrades_utf8_strings_below_2_0() {
    // M17 Phase 3 downgrade discipline: with the UTF-8 flag set but a pre-2.0 target, text
    // strings fall back to their compatible UTF-16BE form instead of being refused …
    let mut builder = Builder::new();
    builder
        .add_page(PageSpec::new(Vec::new()))
        .title("Café — résumé")
        .utf8_text_strings();
    let pdf = builder.build_for(1, 7).unwrap();
    assert!(pdf.starts_with(b"%PDF-1.7"));
    let doc = Document::open(pdf).unwrap();
    let info = doc.info().unwrap().unwrap();
    let title = info
        .get(&Name::from("Title"))
        .and_then(Object::as_string)
        .unwrap();
    assert_eq!(&title.as_bytes()[..2], &[0xFE, 0xFF], "UTF-16BE BOM");
    let decoded: Vec<u16> = title.as_bytes()[2..]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&c| u16::from_be_bytes(c))
        .collect();
    assert_eq!(String::from_utf16(&decoded).unwrap(), "Café — résumé");

    // … while a 2.0 target keeps the UTF-8 form (EF BB BF BOM).
    let pdf = builder.build_for(2, 0).unwrap();
    assert!(pdf.starts_with(b"%PDF-2.0"));
    let doc = Document::open(pdf).unwrap();
    let info = doc.info().unwrap().unwrap();
    let title = info
        .get(&Name::from("Title"))
        .and_then(Object::as_string)
        .unwrap();
    assert_eq!(&title.as_bytes()[..3], &[0xEF, 0xBB, 0xBF], "UTF-8 BOM");
}

#[test]
fn writes_outline_tree() {
    let pdf = Builder::new()
        .add_page(PageSpec::new(b"q Q".to_vec()))
        .add_page(PageSpec::new(b"q Q".to_vec()))
        .outline("Chapter 1", 0)
        .outline("Chapter 2", 1)
        .build();
    let doc = Document::open(pdf).unwrap();
    let catalog = doc.catalog().unwrap();
    // The catalog references an /Outlines root and opens in outline view.
    let Some(Object::Reference(root_ref)) = catalog.get(&Name::from("Outlines")) else {
        panic!("catalog has no /Outlines: {catalog:?}");
    };
    assert_eq!(
        catalog.get(&Name::from("PageMode")),
        Some(&Object::Name(Name::from("UseOutlines")))
    );
    let Object::Dictionary(root) = doc.get(*root_ref).unwrap() else {
        panic!("outline root is not a dict");
    };
    assert_eq!(root.get(&Name::from("Count")), Some(&Object::Integer(2)));
    // First item is "Chapter 1" with a /Dest and a /Next to the second item.
    let Some(Object::Reference(first_ref)) = root.get(&Name::from("First")) else {
        panic!("no /First");
    };
    let Object::Dictionary(first) = doc.get(*first_ref).unwrap() else {
        panic!("item not a dict");
    };
    assert_eq!(
        first.get(&Name::from("Title")),
        Some(&Object::String(PdfString::from(b"Chapter 1".to_vec())))
    );
    assert!(first.get(&Name::from("Dest")).is_some());
    assert!(first.get(&Name::from("Next")).is_some());
    assert!(first.get(&Name::from("Prev")).is_none());
}

#[test]
fn authors_link_and_note_annotations() {
    let mut builder = Builder::new();
    builder
        .add_page(PageSpec::new(b"q Q".to_vec()))
        .add_page(PageSpec::new(b"q Q".to_vec()))
        .add_annotation(
            0,
            AnnotationSpec::Link {
                rect: [72.0, 700.0, 300.0, 720.0],
                target: LinkTarget::Uri("https://example.com".into()),
                contents: None,
            },
            Vec::new(),
        )
        .add_annotation(
            0,
            AnnotationSpec::Link {
                rect: [72.0, 670.0, 300.0, 690.0],
                target: LinkTarget::Page(1),
                contents: None,
            },
            Vec::new(),
        )
        .add_annotation(
            0,
            AnnotationSpec::Note {
                rect: [72.0, 600.0, 92.0, 620.0],
                contents: "A note".into(),
            },
            Vec::new(),
        );
    let doc = Document::open(builder.build()).unwrap();
    let pages = doc.pages().unwrap();
    let annots = doc.annotations(&pages[0]).unwrap();
    assert_eq!(annots.len(), 3, "three annotations on page 0");

    // External hyperlink (URI action).
    assert_eq!(annots[0].subtype, "Link");
    assert_eq!(annots[0].uri.as_deref(), Some("https://example.com"));

    // Internal hyperlink (GoTo) → page index 1.
    assert_eq!(annots[1].subtype, "Link");
    assert_eq!(annots[1].dest_page, Some(1));

    // Text note carrying its body, round-tripped through the reader.
    assert_eq!(annots[2].subtype, "Text");
    assert_eq!(annots[2].contents.as_deref(), Some("A note"));

    // Page 1 has no annotations.
    assert!(doc.annotations(&pages[1]).unwrap().is_empty());
}

#[test]
fn authored_annotations_are_pdfa_clean() {
    let mut builder = Builder::new();
    builder
        .add_page(PageSpec::new(b"q Q".to_vec()))
        .add_annotation(
            0,
            AnnotationSpec::Note {
                rect: [72.0, 600.0, 92.0, 620.0],
                contents: "Note".into(),
            },
            Vec::new(),
        );
    let doc = Document::open(builder.build()).unwrap();
    let pages = doc.pages().unwrap();
    let Some(Object::Array(annots)) = pages[0].get(&Name::from("Annots")) else {
        panic!("no /Annots on page");
    };
    let Some(Object::Reference(aref)) = annots.iter().next() else {
        panic!("empty /Annots");
    };
    let Object::Dictionary(annot) = doc.get(*aref).unwrap() else {
        panic!("annotation not a dict");
    };
    // The /F flag has the Print bit set (= 4) and nothing else (§6.3.2).
    assert_eq!(annot.get(&Name::from("F")), Some(&Object::Integer(4)));
    // The appearance dictionary contains only /N (§6.3.3 t2), and /N is a stream (§6.3.3 t4).
    let Some(Object::Dictionary(ap)) = annot.get(&Name::from("AP")) else {
        panic!("no /AP");
    };
    assert_eq!(ap.len(), 1, "/AP has only /N");
    let Some(Object::Reference(nref)) = ap.get(&Name::from("N")) else {
        panic!("no /N");
    };
    assert!(
        matches!(doc.get(*nref).unwrap(), Object::Stream(_)),
        "normal appearance is a Form XObject stream"
    );
}

#[test]
fn authors_acroform_checkbox() {
    let mut builder = Builder::new();
    builder
        .add_page(PageSpec::new(b"q Q".to_vec()))
        .add_form_field(
            0,
            FormFieldSpec::Checkbox {
                rect: [72.0, 700.0, 90.0, 718.0],
                name: "agree".into(),
                checked: true,
                tooltip: Some("I agree to the terms".into()),
            },
            Vec::new(),
        );
    let doc = Document::open(builder.build()).unwrap();

    // The field reads back through the forms reader: name, Btn type, On value.
    let fields = doc.form_fields().unwrap();
    assert_eq!(fields.len(), 1);
    assert_eq!(fields[0].name, "agree");
    assert_eq!(fields[0].field_type, "Btn");
    assert_eq!(fields[0].value.as_deref(), Some("On"));

    // Catalog → /AcroForm with one /Fields entry and no /NeedAppearances (PDF/A §6.4.1 t3).
    let catalog = doc.catalog().unwrap();
    let Some(Object::Reference(acro_ref)) = catalog.get(&Name::from("AcroForm")) else {
        panic!("no /AcroForm");
    };
    let Object::Dictionary(acro) = doc.get(*acro_ref).unwrap() else {
        panic!("acroform not a dict");
    };
    assert!(acro.get(&Name::from("NeedAppearances")).is_none());
    assert!(acro.get(&Name::from("XFA")).is_none());
    let Some(Object::Array(flds)) = acro.get(&Name::from("Fields")) else {
        panic!("no /Fields");
    };
    assert_eq!(flds.len(), 1);

    // The widget: FT Btn, no /A or /AA (§6.4.1 t1), /AP /N an On/Off appearance subdictionary
    // (§6.3.3 t3).
    let Some(Object::Reference(wref)) = flds.iter().next() else {
        panic!("empty /Fields");
    };
    let Object::Dictionary(w) = doc.get(*wref).unwrap() else {
        panic!("widget not a dict");
    };
    assert_eq!(
        w.get(&Name::from("Subtype")),
        Some(&Object::Name(Name::from("Widget")))
    );
    assert!(w.get(&Name::from("A")).is_none() && w.get(&Name::from("AA")).is_none());
    let Some(Object::Dictionary(ap)) = w.get(&Name::from("AP")) else {
        panic!("no /AP");
    };
    let Some(Object::Dictionary(n)) = ap.get(&Name::from("N")) else {
        panic!("/N not a subdictionary");
    };
    assert!(n.get(&Name::from("On")).is_some() && n.get(&Name::from("Off")).is_some());

    // The widget is also listed in the page's /Annots.
    let pages = doc.pages().unwrap();
    let Some(Object::Array(annots)) = pages[0].get(&Name::from("Annots")) else {
        panic!("widget not in page /Annots");
    };
    assert_eq!(annots.len(), 1);

    // The widget carries the alternate field name (/TU, §12.7.3.1)…
    assert_eq!(
        w.get(&Name::from("TU")),
        Some(&Object::String(PdfString::from(
            b"I agree to the terms".to_vec()
        )))
    );
    // …and an untagged document adds no /Tabs to the page (PDF/UA-1 §7.18.3 is about tagged
    // documents; a plain form stays 1.4-clean).
    assert!(pages[0].get(&Name::from("Tabs")).is_none());
}

#[test]
fn authors_structure_attributes_id_and_idtree() {
    // One page whose marked content backs a TH, an L, a PrintField'd P and a Note (§14.7.6,
    // §14.7.4.2/.5 — the PDF/UA-1 §7.5/§7.6/§7.14/§7.9 authoring surface).
    let content = b"/TH <</MCID 0>> BDC EMC /L <</MCID 1>> BDC EMC\n\
                    /P <</MCID 2>> BDC EMC /Note <</MCID 3>> BDC EMC\n"
        .to_vec();
    let mut builder = Builder::new();
    let mut th = StructElem::new("TH").th_scope(ThScope::Column);
    th.push_content(0, 0);
    // A second owner on the same element → /A becomes an array of two attribute dicts.
    th = th.attr("Layout", "Placement", AttrValue::Name("Block".into()));
    let mut list = StructElem::new("L").list_numbering(ListNumbering::Decimal);
    list.push_content(0, 1);
    let mut pf =
        StructElem::new("P").print_field(PrintFieldRole::Checkbox, Some(true), Some("Agree"));
    pf.push_content(0, 2);
    let mut note = StructElem::new("Note").id("note-1");
    note.push_content(0, 3);
    builder
        .add_page(PageSpec::new(content))
        .structure(vec![th, list, pf, note]);
    let doc = Document::open(builder.build()).unwrap();

    // Walk StructTreeRoot → Document → the four elements.
    let catalog = doc.catalog().unwrap();
    let root = doc
        .resolve(catalog.get(&Name::from("StructTreeRoot")).unwrap())
        .unwrap();
    let Object::Dictionary(root) = root else {
        panic!("no struct tree root");
    };
    let Object::Dictionary(doc_elem) = doc.resolve(root.get(&Name::from("K")).unwrap()).unwrap()
    else {
        panic!("no document element");
    };
    let Some(Object::Array(kids)) = doc_elem.get(&Name::from("K")) else {
        panic!("document /K not an array");
    };
    let elem = |i: usize| -> Dictionary {
        let Object::Dictionary(d) = doc.resolve(&kids[i]).unwrap() else {
            panic!("kid {i} not a dict");
        };
        d
    };

    // TH: /A is an array of two owner dicts — /Table /Scope /Column plus the Layout one.
    let th = elem(0);
    let Some(Object::Array(attrs)) = th.get(&Name::from("A")) else {
        panic!("TH /A not an array: {th:?}");
    };
    assert_eq!(attrs.len(), 2);
    let Some(Object::Dictionary(table_attr)) = attrs.iter().next() else {
        panic!("first attr not a dict");
    };
    assert_eq!(
        table_attr.get(&Name::from("O")),
        Some(&Object::Name(Name::from("Table")))
    );
    assert_eq!(
        table_attr.get(&Name::from("Scope")),
        Some(&Object::Name(Name::from("Column")))
    );

    // L: a single attribute dict is written directly — /List /ListNumbering /Decimal.
    let list = elem(1);
    let Some(Object::Dictionary(a)) = list.get(&Name::from("A")) else {
        panic!("L /A not a dict: {list:?}");
    };
    assert_eq!(
        a.get(&Name::from("O")),
        Some(&Object::Name(Name::from("List")))
    );
    assert_eq!(
        a.get(&Name::from("ListNumbering")),
        Some(&Object::Name(Name::from("Decimal")))
    );

    // PrintField: Role/checked names (spec-cased) and the Desc text string in one dict.
    let pf = elem(2);
    let Some(Object::Dictionary(a)) = pf.get(&Name::from("A")) else {
        panic!("P /A not a dict: {pf:?}");
    };
    assert_eq!(
        a.get(&Name::from("O")),
        Some(&Object::Name(Name::from("PrintField")))
    );
    assert_eq!(
        a.get(&Name::from("Role")),
        Some(&Object::Name(Name::from("cb")))
    );
    assert_eq!(
        a.get(&Name::from("checked")),
        Some(&Object::Name(Name::from("on")))
    );
    assert_eq!(
        a.get(&Name::from("Desc")),
        Some(&Object::String(PdfString::from(b"Agree".to_vec())))
    );

    // Note: /ID on the element, and the /IDTree maps it back to the element.
    let note = elem(3);
    assert_eq!(
        note.get(&Name::from("ID")),
        Some(&Object::String(PdfString::from(b"note-1".to_vec())))
    );
    let Object::Dictionary(idtree) = doc
        .resolve(root.get(&Name::from("IDTree")).unwrap())
        .unwrap()
    else {
        panic!("no /IDTree");
    };
    let Some(Object::Array(names)) = idtree.get(&Name::from("Names")) else {
        panic!("IDTree /Names missing");
    };
    assert_eq!(names.len(), 2, "one (key, element) pair");
    assert_eq!(
        names.iter().next(),
        Some(&Object::String(PdfString::from(b"note-1".to_vec())))
    );
    let Some(Object::Reference(elem_ref)) = names.get(1) else {
        panic!("IDTree value not a reference");
    };
    let Object::Dictionary(back) = doc.get(*elem_ref).unwrap() else {
        panic!("IDTree target not a dict");
    };
    assert_eq!(
        back.get(&Name::from("S")),
        Some(&Object::Name(Name::from("Note")))
    );
}
