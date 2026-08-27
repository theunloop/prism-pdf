use super::*;
use crate::Document;

#[test]
fn emits_structure_destination_and_link_objr() {
    // An intra-document link targeting a structure element by /ID (§12.3.2.3): the GoTo action
    // gains /SD [elem /Fit], its /D fallback is retargeted to the element's page, and the
    // annotation is woven into a Link element via /OBJR (§8.2.5.20 of ISO 14289-2).
    let mut builder = Builder::new();
    builder
        .add_page(PageSpec::new(b"/P <</MCID 0>> BDC EMC\n".to_vec()))
        .add_page(PageSpec::new(b"/H2 <</MCID 0>> BDC EMC\n".to_vec()))
        .add_annotation(
            0,
            AnnotationSpec::Link {
                rect: [72.0, 120.0, 220.0, 136.0],
                target: LinkTarget::Element("sec".into()),
                contents: Some("Go to the section".into()),
            },
            Vec::new(),
        );
    let mut p = StructElem::new("P");
    p.push_content(0, 0);
    let mut h = StructElem::new("H2").id("sec");
    h.push_content(1, 0);
    builder.structure(vec![p, h]);
    let mut link = StructElem::new("Link");
    link.push_annotation(0);
    link.push_annotation(9); // out of range — skipped
    builder.add_structure_element(link);
    let bytes = builder.build();
    assert!(bytes.starts_with(b"%PDF-2.0"), "/SD promotes to 2.0");
    let doc = Document::open(bytes).unwrap();

    // The annotation: /Contents, /StructParent, and a GoTo whose /SD names the H2 element
    // and whose /D fallback points at page 1 (the element's page).
    let pages = doc.pages().unwrap();
    let Some(Object::Array(annots)) = pages[0].get(&Name::from("Annots")) else {
        panic!("no /Annots");
    };
    let Object::Dictionary(annot) = doc.resolve(annots.iter().next().unwrap()).unwrap() else {
        panic!("annot not a dict");
    };
    assert_eq!(
        annot.get(&Name::from("Contents")),
        Some(&Object::String(PdfString::from(
            b"Go to the section".to_vec()
        )))
    );
    assert!(annot.get(&Name::from("StructParent")).is_some());
    let Some(Object::Dictionary(action)) = annot.get(&Name::from("A")) else {
        panic!("no /A");
    };
    let Some(Object::Array(sd)) = action.get(&Name::from("SD")) else {
        panic!("no /SD: {action:?}");
    };
    let Some(Object::Reference(elem_ref)) = sd.iter().next() else {
        panic!("/SD[0] not a reference");
    };
    let Object::Dictionary(target) = doc.get(*elem_ref).unwrap() else {
        panic!("target not a dict");
    };
    assert_eq!(
        target.get(&Name::from("S")),
        Some(&Object::Name(Name::from("H2")))
    );
    let Some(Object::Array(d_dest)) = action.get(&Name::from("D")) else {
        panic!("no /D fallback");
    };
    let Some(Object::Reference(dpage)) = d_dest.iter().next() else {
        panic!("/D[0] not a reference");
    };
    let Object::Dictionary(dpage) = doc.get(*dpage).unwrap() else {
        panic!("page not a dict");
    };
    assert!(
        matches!(dpage.get(&Name::from("Type")), Some(Object::Name(n)) if n.as_bytes() == b"Page"),
        "/D fallback targets a page"
    );

    // The Link element's lone kid is the /OBJR (the out-of-range index dropped out).
    let catalog = doc.catalog().unwrap();
    let Object::Dictionary(root) = doc
        .resolve(catalog.get(&Name::from("StructTreeRoot")).unwrap())
        .unwrap()
    else {
        panic!("no struct tree root");
    };
    let Object::Dictionary(doc_elem) = doc.resolve(root.get(&Name::from("K")).unwrap()).unwrap()
    else {
        panic!("no document element");
    };
    let Some(Object::Array(kids)) = doc_elem.get(&Name::from("K")) else {
        panic!("document /K not an array");
    };
    let Object::Dictionary(link) = doc.resolve(&kids[2]).unwrap() else {
        panic!("no Link element");
    };
    let Some(Object::Dictionary(objr)) = link.get(&Name::from("K")) else {
        panic!("Link /K not the lone OBJR: {link:?}");
    };
    assert_eq!(
        objr.get(&Name::from("Type")),
        Some(&Object::Name(Name::from("OBJR")))
    );
}

#[test]
fn emits_page_labels_number_tree() {
    // Ranges out of order and missing page 0: sorted, page-0 range synthesised as decimal.
    let mut builder = Builder::new();
    builder
        .add_page(PageSpec::new(Vec::new()))
        .add_page(PageSpec::new(Vec::new()))
        .add_page(PageSpec::new(Vec::new()))
        .page_labels(vec![
            PageLabelRange {
                first_page: 2,
                style: Some(PageLabelStyle::Decimal),
                prefix: None,
                start: Some(1),
            },
            PageLabelRange {
                first_page: 1,
                style: Some(PageLabelStyle::RomanLower),
                prefix: Some("A-".into()),
                start: None,
            },
        ]);
    let doc = Document::open(builder.build()).unwrap();
    let catalog = doc.catalog().unwrap();
    let Some(Object::Dictionary(labels)) = catalog.get(&Name::from("PageLabels")) else {
        panic!("no /PageLabels");
    };
    let Some(Object::Array(nums)) = labels.get(&Name::from("Nums")) else {
        panic!("no /Nums");
    };
    // [0 <</S /D>> 1 <</S /r /P (A-)>> 2 <</S /D /St 1>>]
    assert_eq!(nums.len(), 6);
    assert_eq!(nums.iter().next(), Some(&Object::Integer(0)));
    let Some(Object::Dictionary(synth)) = nums.get(1) else {
        panic!("no synthesised page-0 range");
    };
    assert_eq!(
        synth.get(&Name::from("S")),
        Some(&Object::Name(Name::from("D")))
    );
    assert_eq!(nums.get(2), Some(&Object::Integer(1)));
    let Some(Object::Dictionary(roman)) = nums.get(3) else {
        panic!("no roman range");
    };
    assert_eq!(
        roman.get(&Name::from("S")),
        Some(&Object::Name(Name::from("r")))
    );
    assert_eq!(
        roman.get(&Name::from("P")),
        Some(&Object::String(PdfString::from(b"A-".to_vec())))
    );
    let Some(Object::Dictionary(dec)) = nums.get(5) else {
        panic!("no decimal range");
    };
    assert_eq!(dec.get(&Name::from("St")), Some(&Object::Integer(1)));

    // All page-label styles map to their spec names.
    for (style, name) in [
        (PageLabelStyle::Decimal, "D"),
        (PageLabelStyle::RomanUpper, "R"),
        (PageLabelStyle::RomanLower, "r"),
        (PageLabelStyle::AlphaUpper, "A"),
        (PageLabelStyle::AlphaLower, "a"),
    ] {
        assert_eq!(style.name(), name);
    }
}

#[test]
fn authors_role_map_ns_and_element_lang() {
    // A custom "Callout" type in its own namespace, role-mapped to Aside in the PDF 2.0
    // namespace (§14.7.4 /RoleMapNS) — plus a second mapping into the default namespace and
    // a per-element /Lang (§14.9.2).
    const SAMPLE_NS: &str = "https://example.org/ns";
    let mut builder = Builder::new();
    let mut callout = StructElem::new("Callout")
        .namespace(SAMPLE_NS)
        .lang("it-IT");
    callout.push_content(0, 0);
    builder
        .add_page(PageSpec::new(b"/Callout <</MCID 0>> BDC EMC\n".to_vec()))
        .structure(vec![callout])
        .role_map_ns(vec![
            RoleMapEntry {
                ns: SAMPLE_NS.to_string(),
                custom: "Callout".to_string(),
                target: "Aside".to_string(),
                target_ns: Some(PDF2_STRUCT_NS.to_string()),
            },
            RoleMapEntry {
                ns: SAMPLE_NS.to_string(),
                custom: "Hint".to_string(),
                target: "P".to_string(),
                target_ns: None,
            },
        ]);
    let facts = builder.facts();
    assert_eq!(facts.role_maps.len(), 2);
    assert_eq!(
        facts
            .structure_elements
            .iter()
            .map(|element| (element.tag.clone(), element.namespace.clone()))
            .collect::<Vec<_>>(),
        vec![("Callout".to_string(), Some(SAMPLE_NS.to_string()))]
    );
    let doc = Document::open(builder.build()).unwrap();

    // Both namespaces are listed; the sample one carries the /RoleMapNS with both forms.
    let namespaces = doc.structure_namespaces().unwrap();
    assert!(namespaces.contains(&SAMPLE_NS.to_string()));
    assert!(namespaces.contains(&PDF2_STRUCT_NS.to_string()));
    let catalog = doc.catalog().unwrap();
    let Object::Dictionary(root) = doc
        .resolve(catalog.get(&Name::from("StructTreeRoot")).unwrap())
        .unwrap()
    else {
        panic!("no struct tree root");
    };
    let Some(Object::Array(ns_refs)) = root.get(&Name::from("Namespaces")) else {
        panic!("no /Namespaces");
    };
    let sample_ns = ns_refs
        .iter()
        .filter_map(|r| match doc.resolve(r).unwrap() {
            Object::Dictionary(d) => Some(d),
            _ => None,
        })
        .find(|d| {
            matches!(d.get(&Name::from("NS")), Some(Object::String(s))
                if s.as_bytes() == SAMPLE_NS.as_bytes())
        })
        .expect("sample namespace dict");
    let Some(Object::Dictionary(role_map)) = sample_ns.get(&Name::from("RoleMapNS")) else {
        panic!("no /RoleMapNS: {sample_ns:?}");
    };
    // Callout → [Aside <pdf2-ns-ref>]; Hint → /P (default-namespace name form).
    let Some(Object::Array(pair)) = role_map.get(&Name::from("Callout")) else {
        panic!("Callout mapping not an array");
    };
    assert_eq!(pair.iter().next(), Some(&Object::Name(Name::from("Aside"))));
    let Some(Object::Reference(pdf2_ref)) = pair.get(1) else {
        panic!("no target namespace ref");
    };
    let Object::Dictionary(pdf2_ns) = doc.get(*pdf2_ref).unwrap() else {
        panic!("target ns not a dict");
    };
    assert!(
        matches!(pdf2_ns.get(&Name::from("NS")), Some(Object::String(s))
            if s.as_bytes() == PDF2_STRUCT_NS.as_bytes())
    );
    assert_eq!(
        role_map.get(&Name::from("Hint")),
        Some(&Object::Name(Name::from("P")))
    );

    // The element carries its /Lang.
    let Object::Dictionary(doc_elem) = doc.resolve(root.get(&Name::from("K")).unwrap()).unwrap()
    else {
        panic!("no document element");
    };
    let Object::Dictionary(elem) = doc
        .resolve(doc_elem.get(&Name::from("K")).unwrap())
        .unwrap()
    else {
        panic!("no Callout element");
    };
    assert_eq!(
        elem.get(&Name::from("Lang")),
        Some(&Object::String(PdfString::from(b"it-IT".to_vec())))
    );
}

#[test]
fn authors_actual_text_ruby_warichu_and_aria() {
    // /ActualText (§14.9.4), Ruby (RB/RT) and Warichu (WP/WT/WP) assemblies in the PDF 2.0
    // namespace, and an ARIA-1.1 attribute owner (ISO 14289-2 §8.2.6.4) — all through the
    // generic structure machinery.
    let content = b"/Formula <</MCID 0>> BDC EMC\n".to_vec();
    let mut builder = Builder::new();
    let mut formula = StructElem::new("Formula").actual_text("E equals m c squared");
    formula.push_content(0, 0);
    let mut ruby = StructElem::new("Ruby").namespace(PDF2_STRUCT_NS);
    ruby.push_child(StructElem::new("RB").namespace(PDF2_STRUCT_NS));
    ruby.push_child(StructElem::new("RT").namespace(PDF2_STRUCT_NS));
    let mut warichu = StructElem::new("Warichu").namespace(PDF2_STRUCT_NS);
    for tag in ["WP", "WT", "WP"] {
        warichu.push_child(StructElem::new(tag).namespace(PDF2_STRUCT_NS));
    }
    let aria =
        StructElem::new("P").attr("ARIA-1.1", "role", AttrValue::Name("doc-abstract".into()));
    builder
        .add_page(PageSpec::new(content))
        .structure(vec![formula, ruby, warichu, aria]);
    let doc = Document::open(builder.build()).unwrap();

    let catalog = doc.catalog().unwrap();
    let Object::Dictionary(root) = doc
        .resolve(catalog.get(&Name::from("StructTreeRoot")).unwrap())
        .unwrap()
    else {
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
    assert_eq!(
        elem(0).get(&Name::from("ActualText")),
        Some(&Object::String(PdfString::from(
            b"E equals m c squared".to_vec()
        )))
    );
    // Ruby: RB + RT children, all namespaced.
    let ruby = elem(1);
    assert_eq!(
        ruby.get(&Name::from("S")),
        Some(&Object::Name(Name::from("Ruby")))
    );
    assert!(ruby.get(&Name::from("NS")).is_some());
    let Some(Object::Array(rkids)) = ruby.get(&Name::from("K")) else {
        panic!("Ruby /K not an array");
    };
    assert_eq!(rkids.len(), 2);
    // Warichu: WP/WT/WP.
    let warichu = elem(2);
    let Some(Object::Array(wkids)) = warichu.get(&Name::from("K")) else {
        panic!("Warichu /K not an array");
    };
    assert_eq!(wkids.len(), 3);
    // ARIA-1.1 attribute owner.
    let aria = elem(3);
    let Some(Object::Dictionary(a)) = aria.get(&Name::from("A")) else {
        panic!("no ARIA /A");
    };
    assert_eq!(
        a.get(&Name::from("O")),
        Some(&Object::Name(Name::from("ARIA-1.1")))
    );
    assert_eq!(
        a.get(&Name::from("role")),
        Some(&Object::Name(Name::from("doc-abstract")))
    );
}

#[test]
fn resolves_struct_elem_refs_by_id() {
    // A citing paragraph and a FENote referencing each other by /ID (§14.7.4.2 — the PDF/UA-2
    // §8.2.5.14 bidirectional link); a dangling target is skipped without error.
    let content = b"/P <</MCID 0>> BDC EMC /FENote <</MCID 1>> BDC EMC\n".to_vec();
    let mut builder = Builder::new();
    let mut cite = StructElem::new("P").id("cite-1").reference("fn-1");
    cite.push_content(0, 0);
    let mut note = StructElem::new("FENote")
        .id("fn-1")
        .reference("cite-1")
        .reference("no-such-id");
    note.push_content(0, 1);
    builder
        .add_page(PageSpec::new(content))
        .structure(vec![cite, note]);
    let doc = Document::open(builder.build()).unwrap();

    let catalog = doc.catalog().unwrap();
    let Object::Dictionary(root) = doc
        .resolve(catalog.get(&Name::from("StructTreeRoot")).unwrap())
        .unwrap()
    else {
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
    let (cite, note) = (elem(0), elem(1));

    // Each /Ref is an array holding the indirect reference of the other element; the dangling
    // "no-such-id" target simply drops out.
    let refs_of = |d: &Dictionary| -> Vec<Object> {
        let Some(Object::Array(a)) = d.get(&Name::from("Ref")).cloned() else {
            panic!("no /Ref: {d:?}");
        };
        a.iter().cloned().collect()
    };
    assert_eq!(refs_of(&note).len(), 1, "dangling target skipped");
    let Object::Reference(back) = refs_of(&note)[0] else {
        panic!("not a reference");
    };
    let Object::Dictionary(target) = doc.get(back).unwrap() else {
        panic!("target not a dict");
    };
    assert_eq!(
        target.get(&Name::from("ID")),
        Some(&Object::String(PdfString::from(b"cite-1".to_vec())))
    );
    assert_eq!(refs_of(&cite).len(), 1);

    // Both IDs live in the /IDTree.
    let Object::Dictionary(idtree) = doc
        .resolve(root.get(&Name::from("IDTree")).unwrap())
        .unwrap()
    else {
        panic!("no /IDTree");
    };
    let Some(Object::Array(names)) = idtree.get(&Name::from("Names")) else {
        panic!("no /Names");
    };
    assert_eq!(names.len(), 4, "two (key, ref) pairs");
}

#[test]
fn document_facts_walk_structure_and_inventory_files() {
    // The snapshot flattens nested tags and inventories every attachment-capable surface.
    let mut builder = Builder::new();
    builder.add_page(PageSpec::new(Vec::new()));
    let mut sect = StructElem::new("Sect");
    sect.push_child(StructElem::new("H"));
    builder.structure(vec![sect]);
    let facts = builder.facts();
    assert!(facts.structure_elements.iter().any(|e| e.tag == "H"));
    assert!(facts.structure_elements.iter().any(|e| e.tag == "Sect"));
    assert!(!facts.structure_elements.iter().any(|e| e.tag == "Note"));

    let undescribed = Attachment {
        name: "a.bin".into(),
        mime: "application/octet-stream".into(),
        relationship: "Data".into(),
        description: None,
        mod_date: None,
        data: vec![1],
    };
    assert_eq!(builder.facts().undescribed_files, 0);
    // On a structure element (§14.13.6)…
    let mut b2 = Builder::new();
    b2.add_page(PageSpec::new(Vec::new()));
    b2.structure(vec![
        StructElem::new("P").associate_file(undescribed.clone()),
    ]);
    assert_eq!(b2.facts().undescribed_files, 1);
    // …and empty-string descriptions count as missing too.
    let mut b3 = Builder::new();
    b3.add_page(PageSpec::new(Vec::new()));
    let mut described = undescribed;
    described.description = Some(String::new());
    b3.attach_file(described);
    assert_eq!(b3.facts().undescribed_files, 1);
}

#[test]
fn attribute_enums_map_to_spec_names() {
    // §14.8.5.4 Scope values.
    for (scope, name) in [
        (ThScope::Row, "Row"),
        (ThScope::Column, "Column"),
        (ThScope::Both, "Both"),
    ] {
        assert_eq!(scope.name(), name);
    }
    // §14.8.5.5 ListNumbering values.
    for (numbering, name) in [
        (ListNumbering::None, "None"),
        (ListNumbering::Disc, "Disc"),
        (ListNumbering::Circle, "Circle"),
        (ListNumbering::Square, "Square"),
        (ListNumbering::Decimal, "Decimal"),
        (ListNumbering::UpperRoman, "UpperRoman"),
        (ListNumbering::LowerRoman, "LowerRoman"),
        (ListNumbering::UpperAlpha, "UpperAlpha"),
        (ListNumbering::LowerAlpha, "LowerAlpha"),
    ] {
        assert_eq!(numbering.name(), name);
    }
    // §14.8.5.6 PrintField roles (spec-cased short names).
    for (role, name) in [
        (PrintFieldRole::RadioButton, "rb"),
        (PrintFieldRole::Checkbox, "cb"),
        (PrintFieldRole::PushButton, "pb"),
        (PrintFieldRole::TextValue, "tv"),
    ] {
        assert_eq!(role.name(), name);
    }
    // print_field with checked = Some(false) → /checked /off; no Desc entry.
    let elem = StructElem::new("P").print_field(PrintFieldRole::RadioButton, Some(false), None);
    assert_eq!(elem.attrs.len(), 1);
    assert_eq!(
        elem.attrs[0].entries,
        vec![
            ("Role".to_string(), AttrValue::Name("rb".to_string())),
            ("checked".to_string(), AttrValue::Name("off".to_string())),
        ]
    );
    // An integer attribute value serialises as an integer.
    let one = attr_object(
        &[StructAttr {
            owner: "Table".to_string(),
            entries: vec![("ColSpan".to_string(), AttrValue::Int(2))],
        }],
        false,
    )
    .unwrap();
    let Object::Dictionary(d) = one else {
        panic!("single attr not a dict");
    };
    assert_eq!(d.get(&Name::from("ColSpan")), Some(&Object::Integer(2)));
    assert!(attr_object(&[], false).is_none());
}

#[test]
fn nests_widget_in_form_structure_element() {
    // A tagged page with one paragraph and a checkbox; the widget is nested in a Form element
    // via /OBJR (§14.7.4.3 — PDF/UA-1 §7.18.4) and the page takes /Tabs /S (§7.18.3).
    let mut builder = Builder::new();
    builder
        .add_page(PageSpec::new(b"/P <</MCID 0>> BDC EMC\n".to_vec()))
        .add_form_field(
            0,
            FormFieldSpec::Checkbox {
                rect: [72.0, 700.0, 90.0, 718.0],
                name: "agree".into(),
                checked: false,
                tooltip: Some("Agree?".into()),
            },
            Vec::new(),
        );
    let mut p = StructElem::new("P");
    p.push_content(0, 0);
    builder.structure(vec![p]);
    let mut form = StructElem::new("Form");
    form.push_widget(0);
    form.push_widget(7); // out of range — skipped, not an error
    builder.add_structure_element(form);
    let doc = Document::open(builder.build()).unwrap();

    // The page: /Annots with the widget, and /Tabs /S because the document is tagged.
    let pages = doc.pages().unwrap();
    let Some(Object::Array(annots)) = pages[0].get(&Name::from("Annots")) else {
        panic!("no /Annots");
    };
    let Some(Object::Reference(widget_ref)) = annots.iter().next() else {
        panic!("no widget");
    };
    assert_eq!(
        pages[0].get(&Name::from("Tabs")),
        Some(&Object::Name(Name::from("S")))
    );

    // The Form element's single kid is the /OBJR naming the widget and its page.
    let catalog = doc.catalog().unwrap();
    let Object::Dictionary(root) = doc
        .resolve(catalog.get(&Name::from("StructTreeRoot")).unwrap())
        .unwrap()
    else {
        panic!("no struct tree root");
    };
    let Object::Dictionary(doc_elem) = doc.resolve(root.get(&Name::from("K")).unwrap()).unwrap()
    else {
        panic!("no document element");
    };
    let Some(Object::Array(kids)) = doc_elem.get(&Name::from("K")) else {
        panic!("document /K not an array");
    };
    let Object::Dictionary(form) = doc.resolve(&kids[1]).unwrap() else {
        panic!("no Form element");
    };
    assert_eq!(
        form.get(&Name::from("S")),
        Some(&Object::Name(Name::from("Form")))
    );
    let Some(Object::Dictionary(objr)) = form.get(&Name::from("K")) else {
        panic!("Form /K not the lone OBJR dict: {form:?}");
    };
    assert_eq!(
        objr.get(&Name::from("Type")),
        Some(&Object::Name(Name::from("OBJR")))
    );
    assert_eq!(
        objr.get(&Name::from("Obj")),
        Some(&Object::Reference(*widget_ref))
    );
    assert!(objr.get(&Name::from("Pg")).is_some());

    // The widget's /StructParent key follows the per-page keys (1 page → key 1), the parent
    // tree maps it back to the Form element, and /ParentTreeNextKey accounts for it.
    let Object::Dictionary(widget) = doc.get(*widget_ref).unwrap() else {
        panic!("widget not a dict");
    };
    assert_eq!(
        widget.get(&Name::from("StructParent")),
        Some(&Object::Integer(1))
    );
    let Object::Dictionary(parent_tree) = doc
        .resolve(root.get(&Name::from("ParentTree")).unwrap())
        .unwrap()
    else {
        panic!("no parent tree");
    };
    let Some(Object::Array(nums)) = parent_tree.get(&Name::from("Nums")) else {
        panic!("no /Nums");
    };
    // [0 [per-MCID refs] 1 form-elem-ref] — the widget key's value is the element itself.
    assert_eq!(nums.len(), 4);
    assert_eq!(nums.get(2), Some(&Object::Integer(1)));
    let Some(Object::Reference(owner)) = nums.get(3) else {
        panic!("widget parent not a reference");
    };
    let Object::Dictionary(owner) = doc.get(*owner).unwrap() else {
        panic!("owner not a dict");
    };
    assert_eq!(
        owner.get(&Name::from("S")),
        Some(&Object::Name(Name::from("Form")))
    );
    assert_eq!(
        root.get(&Name::from("ParentTreeNextKey")),
        Some(&Object::Integer(2))
    );
    // No element carries an /ID here, so no /IDTree is emitted.
    assert!(root.get(&Name::from("IDTree")).is_none());
}

#[test]
fn emits_image_with_soft_and_stencil_masks() {
    let plain = |w, h, cs, bpc, n, mask| ImageXObject {
        width: w,
        height: h,
        color_space: cs,
        bits_per_component: bpc,
        filter: None,
        data: vec![0u8; n],
        smask: None,
        mask: None,
        image_mask: mask,
    };
    let mut img = plain(2, 2, ImageColorSpace::Rgb, 8, 12, false);
    img.smask = Some(Box::new(plain(2, 2, ImageColorSpace::Gray, 8, 4, false)));
    img.mask = Some(Box::new(plain(2, 2, ImageColorSpace::Gray, 1, 2, true)));

    let mut builder = Builder::new();
    builder.add_page(PageSpec::new(b"q Q".to_vec()).image("Im0", img));
    let doc = Document::open(builder.build()).unwrap();
    let pages = doc.pages().unwrap();

    let Some(Object::Dictionary(res)) = pages[0].get(&Name::from("Resources")) else {
        panic!("no /Resources");
    };
    let Some(Object::Dictionary(xobj)) = res.get(&Name::from("XObject")) else {
        panic!("no /XObject");
    };
    let Some(Object::Reference(im_ref)) = xobj.get(&Name::from("Im0")) else {
        panic!("no Im0");
    };
    let Object::Stream(im) = doc.get(*im_ref).unwrap() else {
        panic!("image not a stream");
    };
    // The base image references a soft mask and a stencil mask.
    let Some(Object::Reference(smask_ref)) = im.dict().get(&Name::from("SMask")) else {
        panic!("no /SMask: {:?}", im.dict());
    };
    let Some(Object::Reference(mask_ref)) = im.dict().get(&Name::from("Mask")) else {
        panic!("no /Mask");
    };
    // The soft mask is a DeviceGray image; the stencil mask is a 1-bit /ImageMask.
    let Object::Stream(smask) = doc.get(*smask_ref).unwrap() else {
        panic!("smask not a stream");
    };
    assert_eq!(
        smask.dict().get(&Name::from("ColorSpace")),
        Some(&Object::Name(Name::from("DeviceGray")))
    );
    let Object::Stream(mask) = doc.get(*mask_ref).unwrap() else {
        panic!("mask not a stream");
    };
    assert_eq!(
        mask.dict().get(&Name::from("ImageMask")),
        Some(&Object::Boolean(true))
    );
    assert_eq!(
        mask.dict().get(&Name::from("BitsPerComponent")),
        Some(&Object::Integer(1))
    );
    // A stencil mask carries no colour space (§8.9.6.2).
    assert!(mask.dict().get(&Name::from("ColorSpace")).is_none());
}

#[test]
fn builds_multi_page_with_default_and_custom_size() {
    let pdf = Builder::new()
        .media_box([0.0, 0.0, 200.0, 300.0])
        .add_page(PageSpec::new(b"q Q".to_vec()))
        .add_page(PageSpec::new(Vec::new()).media_box([0.0, 0.0, 100.0, 100.0]))
        .build();
    let doc = Document::open(pdf).unwrap();
    assert_eq!(doc.page_count().unwrap(), 2);
    let pages = doc.pages().unwrap();
    // First page inherits the builder default; second uses its own box.
    let media = |p: &Dictionary| match p.get(&Name::from("MediaBox")) {
        Some(Object::Array(a)) => a
            .iter()
            .filter_map(|o| match o {
                Object::Real(r) => Some(*r),
                Object::Integer(i) => Some(*i as f64),
                _ => None,
            })
            .collect::<Vec<f64>>(),
        _ => Vec::new(),
    };
    assert_eq!(media(&pages[0]), vec![0.0, 0.0, 200.0, 300.0]);
    assert_eq!(media(&pages[1]), vec![0.0, 0.0, 100.0, 100.0]);
}
