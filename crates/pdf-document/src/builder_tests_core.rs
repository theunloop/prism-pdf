use super::*;
use crate::Document;

#[test]
fn sets_document_metadata() {
    let pdf = Builder::new()
        .title("My Title")
        .author("Jane Doe")
        .info("Subject", "Tëst") // non-ASCII → UTF-16BE
        .add_page(PageSpec::new(Vec::new()))
        .build();
    let doc = Document::open(pdf).unwrap();
    let info = doc.info().unwrap().unwrap();
    assert_eq!(
        info.get(&Name::from("Title")),
        Some(&Object::String(PdfString::from(b"My Title".to_vec())))
    );
    assert_eq!(
        info.get(&Name::from("Author")),
        Some(&Object::String(PdfString::from(b"Jane Doe".to_vec())))
    );
    // The non-ASCII subject is stored as UTF-16BE with a byte-order mark.
    let Some(Object::String(subject)) = info.get(&Name::from("Subject")) else {
        panic!("missing subject")
    };
    assert_eq!(&subject.as_bytes()[..2], &[0xFE, 0xFF]);
}

#[test]
fn builds_an_openable_empty_page() {
    let pdf = Builder::new().add_page(PageSpec::new(Vec::new())).build();
    let doc = Document::open(pdf).unwrap();
    assert_eq!(doc.page_count().unwrap(), 1);
}

#[test]
fn authors_a_separation_colour_space() {
    let content = b"/Spot cs 1 scn 0 0 100 100 re f".to_vec();
    let mut builder = Builder::new();
    builder.add_page(PageSpec::new(content)).add_separation(
        "Spot",
        SeparationSpec {
            colorant: "PANTONE 185 C".into(),
            alternate: ImageColorSpace::Cmyk,
            full: vec![0.0, 0.91, 0.76, 0.0],
        },
    );
    let pdf = builder.build();
    let doc = Document::open(pdf).unwrap();

    // The page's /Resources /ColorSpace /Spot is a Separation array referencing a tint function.
    let page = doc.pages().unwrap().remove(0);
    let resources = page.get_dict(&Name::from("Resources")).unwrap();
    let cs = resources.get_dict(&Name::from("ColorSpace")).unwrap();
    let sep = doc.resolve(cs.get(&Name::from("Spot")).unwrap()).unwrap();
    let arr: Vec<Object> = sep.as_array().unwrap().iter().cloned().collect();
    assert_eq!(
        arr[0].as_name().map(Name::as_bytes),
        Some(&b"Separation"[..])
    );
    assert_eq!(
        arr[1].as_name().map(Name::as_bytes),
        Some(&b"PANTONE 185 C"[..])
    );
    assert_eq!(
        arr[2].as_name().map(Name::as_bytes),
        Some(&b"DeviceCMYK"[..])
    );

    // The tint transform is a type-2 function from white to the full colourant.
    let func = doc.resolve(&arr[3]).unwrap();
    assert_eq!(
        func.as_dict()
            .unwrap()
            .get_integer(&Name::from("FunctionType")),
        Some(2)
    );
}

#[test]
fn authors_an_icc_based_colour_space() {
    let mut builder = Builder::new();
    builder
        .add_page(PageSpec::new(
            b"/ICC cs 0.1 0.2 0.3 scn 0 0 50 50 re f".to_vec(),
        ))
        .add_icc_based("ICC", vec![0u8; 64], 3);
    let doc = Document::open(builder.build()).unwrap();

    let page = doc.pages().unwrap().remove(0);
    let cs = page
        .get_dict(&Name::from("Resources"))
        .unwrap()
        .get_dict(&Name::from("ColorSpace"))
        .unwrap();
    let arr: Vec<Object> = doc
        .resolve(cs.get(&Name::from("ICC")).unwrap())
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .cloned()
        .collect();
    assert_eq!(arr[0].as_name().map(Name::as_bytes), Some(&b"ICCBased"[..]));

    // The profile stream carries /N and is FlateDecode-compressed.
    let stream = doc.resolve(&arr[1]).unwrap();
    let sdict = stream.as_stream().unwrap().dict();
    assert_eq!(sdict.get_integer(&Name::from("N")), Some(3));
    assert_eq!(
        sdict.get_name(&Name::from("Filter")).map(Name::as_bytes),
        Some(&b"FlateDecode"[..])
    );
}

#[test]
fn authors_an_indexed_colour_space() {
    // A 3-entry RGB palette (red, green, blue) ⇒ hival = 2.
    let palette = vec![255, 0, 0, 0, 255, 0, 0, 0, 255];
    let mut builder = Builder::new();
    builder
        .add_page(PageSpec::new(b"/Pal cs 1 scn 0 0 50 50 re f".to_vec()))
        .add_indexed("Pal", ImageColorSpace::Rgb, palette);
    let doc = Document::open(builder.build()).unwrap();

    let page = doc.pages().unwrap().remove(0);
    let cs = page
        .get_dict(&Name::from("Resources"))
        .unwrap()
        .get_dict(&Name::from("ColorSpace"))
        .unwrap();
    let arr: Vec<Object> = doc
        .resolve(cs.get(&Name::from("Pal")).unwrap())
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .cloned()
        .collect();
    assert_eq!(arr[0].as_name().map(Name::as_bytes), Some(&b"Indexed"[..]));
    assert_eq!(
        arr[1].as_name().map(Name::as_bytes),
        Some(&b"DeviceRGB"[..])
    );
    assert_eq!(arr[2].as_integer(), Some(2)); // hival = 3 entries - 1
}

#[test]
fn authors_a_reusable_form_xobject() {
    let mut builder = Builder::new();
    builder
        .add_page(PageSpec::new(b"q 1 0 0 1 100 100 cm /Stamp Do Q".to_vec()))
        .add_form_xobject(
            "Stamp",
            [0.0, 0.0, 20.0, 20.0],
            b"0 0 20 20 re f".to_vec(),
            Vec::new(),
        );
    let doc = Document::open(builder.build()).unwrap();

    let page = doc.pages().unwrap().remove(0);
    let xobj = page
        .get_dict(&Name::from("Resources"))
        .unwrap()
        .get_dict(&Name::from("XObject"))
        .unwrap();
    let form = doc
        .resolve(xobj.get(&Name::from("Stamp")).unwrap())
        .unwrap();
    let dict = form.as_stream().unwrap().dict();
    assert_eq!(
        dict.get_name(&Name::from("Subtype")).map(Name::as_bytes),
        Some(&b"Form"[..])
    );
    assert!(dict.get(&Name::from("BBox")).is_some());
}

#[test]
fn authors_a_lab_colour_space() {
    let mut builder = Builder::new();
    builder
        .add_page(PageSpec::new(b"/Lab cs 50 0 0 scn 0 0 50 50 re f".to_vec()))
        .add_lab("Lab", [0.9505, 1.0, 1.089], [-100.0, 100.0, -100.0, 100.0]);
    let doc = Document::open(builder.build()).unwrap();

    let page = doc.pages().unwrap().remove(0);
    let cs = page
        .get_dict(&Name::from("Resources"))
        .unwrap()
        .get_dict(&Name::from("ColorSpace"))
        .unwrap();
    let arr: Vec<Object> = doc
        .resolve(cs.get(&Name::from("Lab")).unwrap())
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .cloned()
        .collect();
    assert_eq!(arr[0].as_name().map(Name::as_bytes), Some(&b"Lab"[..]));
    assert!(
        arr[1]
            .as_dict()
            .unwrap()
            .get(&Name::from("WhitePoint"))
            .is_some()
    );
}

#[test]
fn authors_document_parts_as_pdf_2_0() {
    let mut builder = Builder::new();
    builder
        .add_page(PageSpec::new(Vec::new()))
        .add_page(PageSpec::new(Vec::new()))
        .document_parts(&[
            DocumentPart {
                first_page: 0,
                last_page: 0,
                dpm: vec![("Title".into(), "Chapter 1".into())],
            },
            DocumentPart {
                first_page: 1,
                last_page: 1,
                dpm: Vec::new(),
            },
        ]);
    let pdf = builder.build();
    // The catalog /DPartRoot makes the header auto-stamp PDF 2.0.
    assert!(pdf.starts_with(b"%PDF-2.0"), "expected %PDF-2.0 header");

    let doc = Document::open(pdf).unwrap();
    let catalog = doc.catalog().unwrap();
    let root = doc
        .resolve(catalog.get(&Name::from("DPartRoot")).unwrap())
        .unwrap();
    let root = root.as_dict().unwrap();
    assert_eq!(
        root.get_name(&Name::from("Type")).map(Name::as_bytes),
        Some(&b"DPartRoot"[..])
    );
    // The root node lists one leaf DPart per part.
    let node = doc
        .resolve(root.get(&Name::from("DPartRootNode")).unwrap())
        .unwrap();
    let parts: Vec<Object> = node
        .as_dict()
        .unwrap()
        .get(&Name::from("DParts"))
        .and_then(Object::as_array)
        .unwrap()
        .iter()
        .cloned()
        .collect();
    assert_eq!(parts.len(), 2);

    // The first leaf carries /DPM with the supplied metadata.
    let leaf0 = doc.resolve(&parts[0]).unwrap();
    let dpm = leaf0
        .as_dict()
        .unwrap()
        .get_dict(&Name::from("DPM"))
        .unwrap();
    assert_eq!(
        dpm.get(&Name::from("Title")).and_then(Object::as_string),
        Some(&PdfString::from(b"Chapter 1".to_vec()))
    );

    // Each page carries a /DPart back-reference to its owning leaf (§14.12.3).
    let pages = doc.pages().unwrap();
    let page0_dpart = doc
        .resolve(pages[0].get(&Name::from("DPart")).unwrap())
        .unwrap();
    assert_eq!(
        page0_dpart
            .as_dict()
            .unwrap()
            .get_name(&Name::from("Type"))
            .map(Name::as_bytes),
        Some(&b"DPart"[..])
    );
    assert!(pages[1].get(&Name::from("DPart")).is_some());
}

#[test]
fn reads_back_document_parts() {
    // Round-trip the DPart tree through the read-side Document::document_parts().
    let mut builder = Builder::new();
    builder
        .add_page(PageSpec::new(Vec::new()))
        .add_page(PageSpec::new(Vec::new()))
        .add_page(PageSpec::new(Vec::new()))
        .document_parts(&[
            DocumentPart {
                first_page: 0,
                last_page: 1,
                dpm: vec![
                    ("Title".into(), "Front matter".into()),
                    ("Author".into(), "Prism".into()),
                ],
            },
            DocumentPart {
                first_page: 2,
                last_page: 2,
                dpm: Vec::new(),
            },
        ]);
    let doc = Document::open(builder.build()).unwrap();
    let parts = doc.document_parts().unwrap();
    assert_eq!(parts.len(), 2);
    assert_eq!(parts[0].start_page, 0);
    assert_eq!(parts[0].end_page, 1);
    assert_eq!(
        parts[0].metadata,
        vec![
            ("Title".to_string(), "Front matter".to_string()),
            ("Author".to_string(), "Prism".to_string()),
        ]
    );
    assert_eq!(parts[1].start_page, 2);
    assert_eq!(parts[1].end_page, 2);
    assert!(parts[1].metadata.is_empty());
}

#[test]
fn document_parts_empty_without_dpart_root() {
    let mut builder = Builder::new();
    builder.add_page(PageSpec::new(Vec::new()));
    let doc = Document::open(builder.build()).unwrap();
    assert!(doc.document_parts().unwrap().is_empty());
}

#[test]
fn emits_a_structure_tree_when_tagged() {
    // A page with two paragraphs of marked content (MCIDs 0 and 1).
    let content = b"/P <</MCID 0>> BDC\nBT /F1 12 Tf (First) Tj ET\nEMC\n\
                    /P <</MCID 1>> BDC\nBT (Second) Tj ET\nEMC\n"
        .to_vec();

    let mut builder = Builder::new();
    builder
        .add_page(PageSpec::new(content).standard_font("F1", StdFont::Helvetica))
        .lang("en-US")
        .structure(vec![
            {
                let mut p = StructElem::new("P");
                p.push_content(0, 0);
                p
            },
            {
                let mut p = StructElem::new("P");
                p.push_content(0, 1);
                p
            },
        ]);
    assert!(!builder.facts().structure_elements.is_empty());
    let doc = Document::open(builder.build()).unwrap();
    let catalog = doc.catalog().unwrap();

    // Catalog: /MarkInfo<</Marked true>>, /Lang, and a /StructTreeRoot.
    let Some(Object::Dictionary(mark_info)) = catalog.get(&Name::from("MarkInfo")) else {
        panic!("no /MarkInfo: {catalog:?}");
    };
    assert_eq!(
        mark_info.get(&Name::from("Marked")),
        Some(&Object::Boolean(true))
    );
    assert_eq!(
        catalog.get(&Name::from("Lang")),
        Some(&Object::String(PdfString::from(b"en-US".to_vec())))
    );
    let Some(Object::Reference(str_root_ref)) = catalog.get(&Name::from("StructTreeRoot")) else {
        panic!("no /StructTreeRoot");
    };

    // StructTreeRoot → Document element → two P elements; a /ParentTree is present.
    let Object::Dictionary(str_root) = doc.get(*str_root_ref).unwrap() else {
        panic!("struct tree root not a dict");
    };
    assert_eq!(
        str_root.get(&Name::from("Type")),
        Some(&Object::Name(Name::from("StructTreeRoot")))
    );
    assert!(str_root.get(&Name::from("ParentTree")).is_some());
    let Some(Object::Reference(doc_ref)) = str_root.get(&Name::from("K")) else {
        panic!("root /K not a single ref");
    };
    let Object::Dictionary(doc_elem) = doc.get(*doc_ref).unwrap() else {
        panic!("document element not a dict");
    };
    assert_eq!(
        doc_elem.get(&Name::from("S")),
        Some(&Object::Name(Name::from("Document")))
    );
    let Some(Object::Array(kids)) = doc_elem.get(&Name::from("K")) else {
        panic!("document /K not an array");
    };
    assert_eq!(kids.len(), 2, "two paragraph structure elements");

    // The first P element references MCID 0 on the page, with /S /P.
    let Object::Reference(p0_ref) = kids.iter().next().unwrap() else {
        panic!("kid not a ref");
    };
    let Object::Dictionary(p0) = doc.get(*p0_ref).unwrap() else {
        panic!("P not a dict");
    };
    assert_eq!(
        p0.get(&Name::from("S")),
        Some(&Object::Name(Name::from("P")))
    );
    // Its single child is a marked-content reference (/MCR) naming the page and MCID 0.
    let Some(Object::Dictionary(mcr)) = p0.get(&Name::from("K")) else {
        panic!("P /K not an MCR dict: {p0:?}");
    };
    assert_eq!(
        mcr.get(&Name::from("Type")),
        Some(&Object::Name(Name::from("MCR")))
    );
    assert_eq!(mcr.get(&Name::from("MCID")), Some(&Object::Integer(0)));
    assert!(mcr.get(&Name::from("Pg")).is_some(), "MCR names its page");

    // The page carries a /StructParents key (into the parent tree).
    let pages = doc.pages().unwrap();
    assert!(pages[0].get(&Name::from("StructParents")).is_some());
}

#[test]
fn authors_an_unencrypted_wrapper_with_encrypted_payload() {
    // §7.6.7: the wrapper embeds the payload as its single file — /EP names the crypto
    // filter, /AFRelationship is /EncryptedPayload, and the catalog declares a hidden
    // collection whose initial document is the payload. Header stamps 2.0.
    let payload_bytes = b"%custom-crypto-blob%".to_vec();
    let mut builder = Builder::new();
    builder
        .add_page(PageSpec::new(Vec::new())) // the visible "install Acme to open this" page
        .encrypted_payload(EncryptedPayloadSpec {
            file_name: "protected.pdf".into(),
            description: Some("Encrypted with AcmeCustomCrypto".into()),
            data: payload_bytes.clone(),
            filter_subtype: "AcmeCustomCrypto".into(),
            version: Some((1, 0)),
        });
    let pdf = builder.build();
    assert!(pdf.starts_with(b"%PDF-2.0"), "wrapper → 2.0");
    let err = builder.build_for(1, 7).unwrap_err();
    assert!(err.to_string().contains("7.6.7"), "diagnostic: {err}");

    let doc = Document::open(pdf).unwrap();
    // Read back through the dedicated API.
    let payload = doc.encrypted_payload().unwrap().expect("has a payload");
    assert_eq!(payload.file_name, "protected.pdf");
    assert_eq!(payload.filter_subtype, "AcmeCustomCrypto");
    assert_eq!(payload.version, Some((1, 0)));
    assert_eq!(payload.data, payload_bytes);

    // Structure: hidden collection targeting the payload; /AFRelationship EncryptedPayload.
    let catalog = doc.catalog().unwrap();
    let collection = catalog
        .get_dict(&Name::from("Collection"))
        .expect("/Collection");
    assert_eq!(
        collection.get_name(&Name::from("View")).map(Name::as_bytes),
        Some(&b"H"[..])
    );
    let attachments = doc.attachments().unwrap();
    assert_eq!(attachments.len(), 1, "exactly one embedded file (§7.6.7)");
    assert_eq!(
        attachments[0].relationship.as_deref(),
        Some("EncryptedPayload")
    );

    // A plain document has no payload.
    let plain = Document::open(Builder::new().add_page(PageSpec::new(Vec::new())).build()).unwrap();
    assert_eq!(plain.encrypted_payload().unwrap(), None);
}

#[test]
fn authors_gotodp_link_to_a_document_part() {
    // §12.6.4.5: a link whose target is a document part becomes /A << /S /GoToDp /Dp … >>
    // referencing that part's leaf DPart; the reader resolves it to the part's /Start page.
    let mut builder = Builder::new();
    builder
        .add_page(PageSpec::new(Vec::new()))
        .add_page(PageSpec::new(Vec::new()))
        .add_page(PageSpec::new(Vec::new()))
        .document_parts(&[
            DocumentPart {
                first_page: 0,
                last_page: 0,
                dpm: Vec::new(),
            },
            DocumentPart {
                first_page: 1,
                last_page: 2,
                dpm: Vec::new(),
            },
        ])
        .add_annotation(
            0,
            AnnotationSpec::Link {
                rect: [0.0, 0.0, 50.0, 20.0],
                target: LinkTarget::DocumentPart(1),
                contents: None,
            },
            Vec::new(),
        );
    let pdf = builder.build();
    assert!(pdf.starts_with(b"%PDF-2.0"), "GoToDp/DParts → 2.0");

    let doc = Document::open(pdf).unwrap();
    let pages = doc.pages().unwrap();
    let annots = doc.annotations(&pages[0]).unwrap();
    assert_eq!(annots.len(), 1);
    // The reader follows /Dp → /Start: part 1 starts on page index 1.
    assert_eq!(annots[0].dest_page, Some(1));
    // Structurally: the action is a GoToDp with an indirect /Dp to a /Type /DPart dict.
    let raw = doc
        .resolve(pages[0].get(&Name::from("Annots")).unwrap())
        .unwrap();
    let annot = doc
        .resolve(raw.as_array().unwrap().iter().next().unwrap())
        .unwrap();
    let action = annot
        .as_dict()
        .and_then(|a| a.get_dict(&Name::from("A")).cloned())
        .expect("/A action");
    assert_eq!(
        action.get_name(&Name::from("S")).map(Name::as_bytes),
        Some(&b"GoToDp"[..])
    );
    let dpart = doc.resolve(action.get(&Name::from("Dp")).unwrap()).unwrap();
    assert_eq!(
        dpart
            .as_dict()
            .unwrap()
            .get_name(&Name::from("Type"))
            .map(Name::as_bytes),
        Some(&b"DPart"[..])
    );
}

#[test]
fn gotodp_link_without_parts_drops_the_action() {
    // With no document_parts declared, a dangling GoToDp (missing its required /Dp) would be
    // invalid — the builder drops the action and keeps the (inert) link annotation.
    let mut builder = Builder::new();
    builder.add_page(PageSpec::new(Vec::new())).add_annotation(
        0,
        AnnotationSpec::Link {
            rect: [0.0, 0.0, 10.0, 10.0],
            target: LinkTarget::DocumentPart(0),
            contents: None,
        },
        Vec::new(),
    );
    let doc = Document::open(builder.build()).unwrap();
    let pages = doc.pages().unwrap();
    let raw = doc
        .resolve(pages[0].get(&Name::from("Annots")).unwrap())
        .unwrap();
    let annot = doc
        .resolve(raw.as_array().unwrap().iter().next().unwrap())
        .unwrap();
    assert!(annot.as_dict().unwrap().get(&Name::from("A")).is_none());
}

#[test]
fn authors_and_reads_developer_extensions() {
    // §7.12: one declaration per prefix = the 1.7 dictionary form; two under the same prefix
    // = the 2.0 array form. Both read back via Document::developer_extensions().
    use crate::DeveloperExtension;
    let ext = |prefix: &str, level: i64, url: Option<&str>| DeveloperExtension {
        prefix: prefix.into(),
        base_version: (1, 7),
        extension_level: level,
        url: url.map(str::to_string),
        revision: None,
    };

    // Single-dictionary form: stays PDF 1.7, and a 1.6 target is refused naming §7.12.
    let mut builder = Builder::new();
    builder
        .add_page(PageSpec::new(Vec::new()))
        .developer_extension(ext("GLGR", 1002, None));
    let pdf = builder.build();
    assert!(pdf.starts_with(b"%PDF-1.7"), "Extensions dict → 1.7");
    let err = builder.build_for(1, 6).unwrap_err();
    assert!(err.to_string().contains("7.12"), "diagnostic: {err}");
    let doc = Document::open(pdf).unwrap();
    let exts = doc.developer_extensions().unwrap();
    assert_eq!(exts.len(), 1);
    assert_eq!(exts[0].prefix, "GLGR");
    assert_eq!(exts[0].base_version, (1, 7));
    assert_eq!(exts[0].extension_level, 1002);

    // Array form + URL: PDF 2.0; all three declarations read back.
    let mut builder = Builder::new();
    builder
        .add_page(PageSpec::new(Vec::new()))
        .developer_extension(ext("ISO_", 24064, None))
        .developer_extension(ext("ISO_", 24065, None))
        .developer_extension(ext("ADBE", 3, Some("https://example.org/ext3.pdf")));
    let pdf = builder.build();
    assert!(pdf.starts_with(b"%PDF-2.0"), "array/URL form → 2.0");
    let doc = Document::open(pdf).unwrap();
    let exts = doc.developer_extensions().unwrap();
    assert_eq!(exts.len(), 3);
    let levels: Vec<i64> = exts
        .iter()
        .filter(|e| e.prefix == "ISO_")
        .map(|e| e.extension_level)
        .collect();
    assert_eq!(levels, vec![24064, 24065]);
    let adbe = exts.iter().find(|e| e.prefix == "ADBE").unwrap();
    assert_eq!(adbe.url.as_deref(), Some("https://example.org/ext3.pdf"));
}

#[test]
fn authors_marked_content_af_property_as_pdf_2_0() {
    // §14.13.5: graphics wrapped in `/AF /F0 BDC … EMC` link to files via the named property
    // resource — an array of filespec dicts in /Resources /Properties. Header stamps 2.0, and
    // a pre-2.0 target refuses with the construct named.
    let content = b"/AF /F0 BDC\n0 0 10 10 re f\nEMC".to_vec();
    let mut builder = Builder::new();
    builder
        .add_page(PageSpec::new(content))
        .add_content_af_property(
            0,
            "F0",
            vec![Attachment {
                name: "chart-data.csv".into(),
                mime: "text/csv".into(),
                relationship: "Data".into(),
                description: None,
                mod_date: None,
                data: b"x,y\n1,2\n".to_vec(),
            }],
        );
    let pdf = builder.build();
    assert!(pdf.starts_with(b"%PDF-2.0"), "marked-content /AF → 2.0");

    let doc = Document::open(pdf).unwrap();
    let pages = doc.pages().unwrap();
    let resources = pages[0].get_dict(&Name::from("Resources")).unwrap();
    let props = resources
        .get_dict(&Name::from("Properties"))
        .expect("/Properties");
    let af = doc.resolve(props.get(&Name::from("F0")).unwrap()).unwrap();
    let af = af.as_array().expect("property is an array of filespecs");
    let fs = doc.resolve(af.iter().next().unwrap()).unwrap();
    assert_eq!(
        fs.as_dict()
            .unwrap()
            .get_name(&Name::from("Type"))
            .map(Name::as_bytes),
        Some(&b"Filespec"[..])
    );
    // In /EmbeddedFiles too, and refused below PDF 2.0 with a §14.13.5 diagnostic.
    assert_eq!(doc.attachments().unwrap().len(), 1);
    let err = builder.build_for(1, 7).unwrap_err();
    assert!(err.to_string().contains("14.13.5"), "diagnostic: {err}");
    assert!(builder.build_for(2, 0).is_ok());
}

#[test]
fn authors_page_level_output_intent_as_pdf_2_0() {
    // A page-level OutputIntent (§14.11.5, PDF 2.0): the page carries /OutputIntents with a
    // GTS_PDFA1 intent whose /DestOutputProfile is the ICC stream, and the header stamps 2.0.
    let icc = vec![0u8; 16]; // opaque profile bytes: the builder embeds them verbatim
    let mut builder = Builder::new();
    builder
        .add_page(PageSpec::new(Vec::new()))
        .add_page(PageSpec::new(Vec::new()))
        .page_output_intent(1, icc.clone(), 3, "sRGB-page");
    let pdf = builder.build();
    assert!(pdf.starts_with(b"%PDF-2.0"), "page OutputIntent → 2.0");

    let doc = Document::open(pdf).unwrap();
    let pages = doc.pages().unwrap();
    assert!(
        pages[0].get(&Name::from("OutputIntents")).is_none(),
        "only the named page carries the intent"
    );
    let intents = pages[1]
        .get(&Name::from("OutputIntents"))
        .and_then(Object::as_array)
        .expect("page /OutputIntents");
    let intent = doc.resolve(intents.iter().next().unwrap()).unwrap();
    let intent = intent.as_dict().unwrap();
    assert_eq!(
        intent.get_name(&Name::from("S")).map(Name::as_bytes),
        Some(&b"GTS_PDFA1"[..])
    );
    let profile = doc
        .resolve(intent.get(&Name::from("DestOutputProfile")).unwrap())
        .unwrap();
    let Object::Stream(profile) = profile else {
        panic!("profile is a stream");
    };
    assert_eq!(profile.dict().get_integer(&Name::from("N")), Some(3));
    assert_eq!(profile.raw().as_ref(), icc.as_slice());
}

#[test]
fn authors_namespace_schema_filespec() {
    // A namespace with an attached schema (§14.7.4): the /Namespace dict carries /Schema — a
    // filespec of the embedded schema file, also listed in /EmbeddedFiles.
    const NS: &str = "https://example.org/ns/custom";
    let content = b"/P <</MCID 0>> BDC BT (x) Tj ET EMC".to_vec();
    let mut builder = Builder::new();
    builder
        .add_page(PageSpec::new(content))
        .structure_namespace(NS)
        .structure_namespace_schema(
            NS,
            Attachment {
                name: "custom.xsd".into(),
                mime: "application/xml".into(),
                relationship: "Schema".into(),
                description: Some("Namespace schema".into()),
                mod_date: None,
                data: b"<xs:schema/>".to_vec(),
            },
        )
        .structure(vec![{
            let mut p = StructElem::new("P");
            p.push_content(0, 0);
            p
        }]);
    let pdf = builder.build();
    assert!(pdf.starts_with(b"%PDF-2.0"));

    let doc = Document::open(pdf).unwrap();
    let catalog = doc.catalog().unwrap();
    let str_root = doc
        .resolve(catalog.get(&Name::from("StructTreeRoot")).unwrap())
        .unwrap();
    let namespaces = str_root
        .as_dict()
        .and_then(|r| r.get_array(&Name::from("Namespaces")).cloned())
        .expect("/Namespaces");
    let ns = doc.resolve(namespaces.iter().next().unwrap()).unwrap();
    let schema = doc
        .resolve(ns.as_dict().unwrap().get(&Name::from("Schema")).unwrap())
        .unwrap();
    let schema = schema.as_dict().expect("schema filespec");
    assert_eq!(
        schema.get_name(&Name::from("Type")).map(Name::as_bytes),
        Some(&b"Filespec"[..])
    );
    // The schema file is also discoverable as an embedded file.
    let attachments = doc.attachments().unwrap();
    assert_eq!(attachments.len(), 1);
    assert_eq!(attachments[0].name, "custom.xsd");
    assert_eq!(attachments[0].data, b"<xs:schema/>");
}
