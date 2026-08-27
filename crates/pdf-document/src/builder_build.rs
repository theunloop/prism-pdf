use super::*;

impl Builder {
    /// Serialise the document to PDF bytes (§7.5): catalog → page tree → pages, with a classic
    /// cross-reference table. The result reopens with [`Document::open`](crate::Document).
    #[must_use]
    pub fn build(&self) -> Vec<u8> {
        // Infallible: build_inner errors only on the build_for (target-version) path.
        self.build_inner(None).unwrap_or_default()
    }

    /// Serialise the document declaring exactly the **target version** `(major, minor)` (§7.5.2,
    /// M17 Phases 2–3), with the *guarantee* that the output contains only constructs valid at
    /// that version.
    ///
    /// Where a version-compatible form exists, the builder downgrades automatically: with
    /// [`Builder::utf8_text_strings`] set and a target below 2.0, non-ASCII text strings fall
    /// back to UTF-16BE (§7.9.2.2). Any remaining construct above the target — document parts,
    /// structure namespaces, structure destinations, page-level `/AF`, … — is refused with
    /// [`DocError`](crate::DocError)`::TargetVersionExceeded` naming the offending construct.
    /// A target *above* the content's minimum is always fine (over-declaring is harmless) and is
    /// stamped as-is; a pin set via [`Builder::version`] is ignored on this path.
    pub fn build_for(&self, major: u8, minor: u8) -> crate::Result<Vec<u8>> {
        self.build_inner(Some((major, minor)))
    }

    /// Shared assembly for [`Builder::build`] (auto-stamp / pin-as-floor) and
    /// [`Builder::build_for`] (target gate). Errors only on the target path.
    fn build_inner(&self, target: Option<(u8, u8)>) -> crate::Result<Vec<u8>> {
        let catalog_id = ObjectId::new(1, 0);
        let pages_id = ObjectId::new(2, 0);
        let default_box = self.media_box.unwrap_or(US_LETTER);

        let mut objects: Vec<(ObjectId, Object)> = Vec::new();
        let mut next = 3u32;
        let mut alloc = || {
            let id = ObjectId::new(next, 0);
            next += 1;
            id
        };

        // Embedded composite fonts are shared across pages: emit each one's object set once and
        // remember its Type0 font object by resource name.
        let mut embedded_ids: Vec<(String, ObjectId)> = Vec::new();
        for (name, font) in &self.embedded {
            let fontfile_id = alloc();
            objects.push((fontfile_id, Object::Stream(fontfile2_stream(&font.program))));
            let descriptor_id = alloc();
            objects.push((
                descriptor_id,
                Object::Dictionary(font_descriptor_dict(font, fontfile_id)),
            ));
            let cid_to_gid_id = font.cid_to_gid.as_ref().map(|map| {
                let id = alloc();
                let mut dict = Dictionary::new();
                dict.insert(
                    Name::from("Filter"),
                    Object::Name(Name::from("FlateDecode")),
                );
                objects.push((id, Object::Stream(Stream::new(dict, flate_encode(map)))));
                id
            });
            let cid_id = alloc();
            objects.push((
                cid_id,
                Object::Dictionary(cid_font_dict(font, descriptor_id, cid_to_gid_id)),
            ));
            let tounicode_id = alloc();
            objects.push((
                tounicode_id,
                Object::Stream(tounicode_stream(&font.to_unicode)),
            ));
            let type0_id = alloc();
            objects.push((
                type0_id,
                Object::Dictionary(type0_dict(font, cid_id, tounicode_id)),
            ));
            embedded_ids.push((name.clone(), type0_id));
        }

        let tagged = !self.structure.is_empty();
        // Associated files (`/AF`, §14.13) attached to form XObjects (§14.13.7) and annotations
        // (§14.13.9) are emitted as the pages/annotations are built; their filespecs accumulate here
        // so the `/EmbeddedFiles` name tree (§7.7.4) can list them alongside catalog/page/struct AF.
        let mut af_tree_refs: Vec<(String, ObjectId)> = Vec::new();
        let mut kids = Vec::with_capacity(self.pages.len());
        let mut page_ids = Vec::with_capacity(self.pages.len());
        for (page_index, page) in self.pages.iter().enumerate() {
            let content_id = alloc();
            objects.push((
                content_id,
                Object::Stream(Stream::new(Dictionary::new(), page.content.clone())),
            ));

            // One font dictionary object per named Standard-14 resource, plus references to the
            // shared embedded fonts this page uses.
            let mut font_resources = Dictionary::new();
            for (name, font) in &page.fonts {
                let font_id = alloc();
                objects.push((font_id, Object::Dictionary(font_dict(*font))));
                font_resources.insert(Name::from(name.as_str()), Object::Reference(font_id));
            }
            for name in &page.embedded {
                if let Some((_, id)) = embedded_ids.iter().find(|(n, _)| n == name) {
                    font_resources.insert(Name::from(name.as_str()), Object::Reference(*id));
                }
            }

            // One image XObject stream per named image resource (plus any soft-/stencil-mask
            // sub-images, emitted as their own objects and referenced by /SMask or /Mask).
            let mut xobject_resources = Dictionary::new();
            for (name, image) in &page.images {
                let image_id = emit_image(image, &mut alloc, &mut objects);
                xobject_resources.insert(Name::from(name.as_str()), Object::Reference(image_id));
            }
            // Reusable content Form XObjects (§8.10) share the page's /XObject resource dict. A form
            // may carry associated files (`/AF`, §14.13.7, PDF 2.0) referenced from its stream dict.
            for form in &page.forms {
                let form_id = alloc();
                let mut stream = form_xobject_stream(form.bbox, form.content.clone());
                if let Some(af) =
                    emit_af_array(&form.files, &mut alloc, &mut objects, &mut af_tree_refs)
                {
                    stream.dict_mut().insert(Name::from("AF"), af);
                }
                objects.push((form_id, Object::Stream(stream)));
                xobject_resources
                    .insert(Name::from(form.name.as_str()), Object::Reference(form_id));
            }

            // One colour-space object (Separation array + tint fn, or ICCBased array + profile
            // stream) per named resource.
            let mut colorspace_resources = Dictionary::new();
            for (name, kind) in &page.color_spaces {
                let cs_id = emit_color_space(kind, &mut alloc, &mut objects);
                colorspace_resources.insert(Name::from(name.as_str()), Object::Reference(cs_id));
            }

            // Marked-content associated files (§14.13.5, PDF 2.0): each registered property is an
            // indirect *array of file specification dictionaries* named in /Resources /Properties,
            // referenced from an `/AF <name> BDC` sequence in the content stream
            // (`Content::begin_af_marked_content`). The files also join /EmbeddedFiles.
            let mut properties_resources = Dictionary::new();
            for (_, name, files) in self
                .content_af_props
                .iter()
                .filter(|(pi, _, _)| (*pi).min(self.pages.len() - 1) == page_index)
            {
                if let Some(af) = emit_af_array(files, &mut alloc, &mut objects, &mut af_tree_refs)
                {
                    let array_id = alloc();
                    objects.push((array_id, af));
                    properties_resources
                        .insert(Name::from(name.as_str()), Object::Reference(array_id));
                }
            }

            let page_id = alloc();
            let media = page.media_box.unwrap_or(default_box);
            // A tagged page needs a /StructParents key into the parent tree (§14.7.4.4); we key each
            // page by its index.
            let struct_parents = tagged.then_some(page_index as i64);
            objects.push((
                page_id,
                Object::Dictionary(page_dict(
                    pages_id,
                    content_id,
                    &media,
                    font_resources,
                    xobject_resources,
                    colorspace_resources,
                    properties_resources,
                    struct_parents,
                )),
            ));
            kids.push(Object::Reference(page_id));
            page_ids.push(page_id);
        }

        let mut pages_dict = Dictionary::new();
        pages_dict.insert(Name::from("Type"), Object::Name(Name::from("Pages")));
        pages_dict.insert(Name::from("Kids"), Object::Array(Array::from(kids)));
        pages_dict.insert(
            Name::from("Count"),
            Object::Integer(self.pages.len() as i64),
        );
        objects.push((pages_id, Object::Dictionary(pages_dict)));

        // Annotations (§12.5) and form-field widgets (§12.7) both live in a page's /Annots; widgets
        // are additionally listed in the document /AcroForm. Emitted after the page loop so links and
        // fields can reference any page. PDF/A-clean: the Print flag is set, notes/widgets carry a
        // normal appearance (§6.3), and the form has no /NeedAppearances or /XFA (§6.4.1/§6.4.2).
        // Each form field's widget / each annotation and its page, in call order — the structure
        // tree resolves `StructKid::Widget` / `StructKid::Annotation` indices (§14.7.4.3 OBJR)
        // against these. `sd_requests` queues GoTo actions whose structure destination (`/SD`,
        // §12.3.2.3) can only resolve once the structure tree is emitted.
        let mut widget_refs: Vec<(ObjectId, ObjectId)> = Vec::new();
        let mut annot_refs: Vec<(ObjectId, ObjectId)> = Vec::new();
        let mut sd_requests: Vec<(ObjectId, Vec<u8>)> = Vec::new();
        let mut dp_requests: Vec<(ObjectId, usize)> = Vec::new();
        let acroform_id = if (!self.annotations.is_empty() || !self.form_fields.is_empty())
            && !page_ids.is_empty()
        {
            let mut per_page: Vec<Vec<ObjectId>> = vec![Vec::new(); page_ids.len()];
            for (page_index, spec, files) in &self.annotations {
                let pi = (*page_index).min(page_ids.len() - 1);
                let annot_id = emit_annotation(
                    spec,
                    files,
                    page_ids[pi],
                    &page_ids,
                    &mut alloc,
                    &mut objects,
                    &mut af_tree_refs,
                    &mut sd_requests,
                    &mut dp_requests,
                );
                per_page[pi].push(annot_id);
                annot_refs.push((annot_id, page_ids[pi]));
            }
            let mut field_ids: Vec<ObjectId> = Vec::new();
            for (page_index, field, files) in &self.form_fields {
                let pi = (*page_index).min(page_ids.len() - 1);
                let id = emit_form_field(
                    field,
                    files,
                    page_ids[pi],
                    &mut alloc,
                    &mut objects,
                    &mut af_tree_refs,
                );
                per_page[pi].push(id);
                field_ids.push(id);
                widget_refs.push((id, page_ids[pi]));
            }
            for (pi, ids) in per_page.iter().enumerate() {
                if ids.is_empty() {
                    continue;
                }
                let annots = Object::Array(Array::from(
                    ids.iter()
                        .map(|id| Object::Reference(*id))
                        .collect::<Vec<_>>(),
                ));
                if let Some((_, Object::Dictionary(page))) =
                    objects.iter_mut().find(|(id, _)| *id == page_ids[pi])
                {
                    page.insert(Name::from("Annots"), annots);
                    if tagged {
                        // A tagged page with annotations takes them in structure order
                        // (`/Tabs /S`, Table 30 — PDF 1.5; required by PDF/UA-1 §7.18.3).
                        page.insert(Name::from("Tabs"), Object::Name(Name::from("S")));
                    }
                }
            }
            // The interactive form dictionary (§12.7.2): /Fields only — no /NeedAppearances (every
            // field carries its own appearance) and no /XFA (PDF/A §6.4.1 t3 / §6.4.2 t1).
            if field_ids.is_empty() {
                None
            } else {
                let id = alloc();
                let mut acro = Dictionary::new();
                acro.insert(
                    Name::from("Fields"),
                    Object::Array(Array::from(
                        field_ids
                            .iter()
                            .map(|i| Object::Reference(*i))
                            .collect::<Vec<_>>(),
                    )),
                );
                acro.insert(Name::from("DR"), Object::Dictionary(Dictionary::new()));
                objects.push((id, Object::Dictionary(acro)));
                Some(id)
            }
        } else {
            None
        };

        // The document outline (bookmarks, §12.3.3): a root /Outlines object whose items form a
        // doubly-linked list (Prev/Next), each with a /Dest pointing to its page.
        let outlines_id = if self.outlines.is_empty() || page_ids.is_empty() {
            None
        } else {
            let root_id = alloc();
            let item_ids: Vec<ObjectId> = self.outlines.iter().map(|_| alloc()).collect();
            for (i, item) in self.outlines.iter().enumerate() {
                let page_idx = item.page_index.min(page_ids.len() - 1);
                let dest = Array::from(vec![
                    Object::Reference(page_ids[page_idx]),
                    Object::Name(Name::from("Fit")),
                ]);
                let mut d = Dictionary::new();
                d.insert(
                    Name::from("Title"),
                    Object::String(PdfString::from(self.encode_text(&item.title))),
                );
                d.insert(Name::from("Parent"), Object::Reference(root_id));
                d.insert(Name::from("Dest"), Object::Array(dest));
                if i > 0 {
                    d.insert(Name::from("Prev"), Object::Reference(item_ids[i - 1]));
                }
                if i + 1 < item_ids.len() {
                    d.insert(Name::from("Next"), Object::Reference(item_ids[i + 1]));
                }
                objects.push((item_ids[i], Object::Dictionary(d)));
            }
            let mut root = Dictionary::new();
            root.insert(Name::from("Type"), Object::Name(Name::from("Outlines")));
            root.insert(Name::from("First"), Object::Reference(item_ids[0]));
            root.insert(
                Name::from("Last"),
                Object::Reference(item_ids[item_ids.len() - 1]),
            );
            root.insert(Name::from("Count"), Object::Integer(item_ids.len() as i64));
            objects.push((root_id, Object::Dictionary(root)));
            Some(root_id)
        };

        // PDF/A structures (M7): the XMP /Metadata stream (§14.3.2) and the OutputIntent + ICC
        // profile (§14.11.5), each referenced from the catalog.
        let metadata_id = self.metadata.as_ref().map(|xmp| {
            let id = alloc();
            let mut dict = Dictionary::new();
            dict.insert(Name::from("Type"), Object::Name(Name::from("Metadata")));
            dict.insert(Name::from("Subtype"), Object::Name(Name::from("XML")));
            objects.push((id, Object::Stream(Stream::new(dict, xmp.clone()))));
            id
        });
        let output_intent_id = self
            .output_intent
            .as_ref()
            .map(|oi| emit_output_intent(oi, &mut alloc, &mut objects));

        // Page-level OutputIntents (§14.11.5, PDF 2.0): each named page gets its own
        // /OutputIntents array, overriding the document-level intent for that page.
        for (page_index, oi) in &self.page_output_intents {
            if page_ids.is_empty() {
                break;
            }
            let pi = (*page_index).min(page_ids.len() - 1);
            let oi_id = emit_output_intent(oi, &mut alloc, &mut objects);
            if let Some((_, Object::Dictionary(page))) =
                objects.iter_mut().find(|(id, _)| *id == page_ids[pi])
            {
                let mut refs = match page.get(&Name::from("OutputIntents")) {
                    Some(Object::Array(existing)) => existing.iter().cloned().collect::<Vec<_>>(),
                    _ => Vec::new(),
                };
                refs.push(Object::Reference(oi_id));
                page.insert(
                    Name::from("OutputIntents"),
                    Object::Array(Array::from(refs)),
                );
            }
        }

        // Embedded files (§7.11.4) and their file specifications (§7.11.3). Catalog-level attachments
        // (PDF/A-3) go in the catalog's /AF; page-level associations (§14.13.4, PDF 2.0) go in the
        // owning page's /AF. Both kinds share the /EmbeddedFiles name tree (§7.7.4).
        let mut filespec_refs: Vec<(String, ObjectId)> = Vec::new();
        // The encrypted payload of an unencrypted wrapper (§7.6.7): a normal embedded-file
        // filespec (related as /EncryptedPayload) that additionally carries the /EP encrypted
        // payload dictionary (Table 28) naming the cryptographic filter.
        if let Some(payload) = &self.encrypted_payload {
            let attachment = Attachment {
                name: payload.file_name.clone(),
                mime: "application/pdf".into(),
                relationship: "EncryptedPayload".into(),
                description: payload.description.clone(),
                mod_date: None,
                data: payload.data.clone(),
            };
            let fs_id = emit_filespec(&attachment, &mut alloc, &mut objects);
            if let Some((_, Object::Dictionary(fs))) =
                objects.iter_mut().find(|(id, _)| *id == fs_id)
            {
                let mut ep = Dictionary::new();
                ep.insert(
                    Name::from("Type"),
                    Object::Name(Name::from("EncryptedPayload")),
                );
                ep.insert(
                    Name::from("Subtype"),
                    Object::Name(Name::from(payload.filter_subtype.as_str())),
                );
                if let Some((major, minor)) = payload.version {
                    ep.insert(
                        Name::from("Version"),
                        Object::Name(Name::from(format!("{major}.{minor}").as_str())),
                    );
                }
                fs.insert(Name::from("EP"), Object::Dictionary(ep));
            }
            filespec_refs.push((payload.file_name.clone(), fs_id));
        }
        for attachment in &self.attachments {
            let fs_id = emit_filespec(attachment, &mut alloc, &mut objects);
            filespec_refs.push((attachment.name.clone(), fs_id));
        }
        // Page-level associated files (§14.13.4, PDF 2.0): emit each filespec and remember which page
        // it attaches to; the page dicts gain an /AF array after this, and the header auto-stamps 2.0.
        let mut page_af: Vec<(usize, ObjectId)> = Vec::new();
        let mut page_af_named: Vec<(String, ObjectId)> = Vec::new();
        for (page_index, attachment) in &self.page_attachments {
            if page_ids.is_empty() {
                break;
            }
            let fs_id = emit_filespec(attachment, &mut alloc, &mut objects);
            let pi = (*page_index).min(page_ids.len() - 1);
            page_af.push((pi, fs_id));
            page_af_named.push((attachment.name.clone(), fs_id));
        }
        // Insert /AF into each page that has associated files (§14.13.4), grouping by page.
        for (pi, page_id) in page_ids.iter().enumerate() {
            let refs: Vec<Object> = page_af
                .iter()
                .filter(|(p, _)| *p == pi)
                .map(|(_, id)| Object::Reference(*id))
                .collect();
            if refs.is_empty() {
                continue;
            }
            if let Some((_, Object::Dictionary(page))) =
                objects.iter_mut().find(|(id, _)| id == page_id)
            {
                page.insert(Name::from("AF"), Object::Array(Array::from(refs)));
            }
        }
        // Associated files (§14.13) accumulate here for the /EmbeddedFiles name tree (§7.7.4), built
        // *after* the structure tree so struct-element /AF files (§14.13.6) are listed too.
        let mut tree_refs = filespec_refs.clone();
        tree_refs.extend(page_af_named.iter().cloned());
        // Form-XObject (§14.13.7) and annotation (§14.13.9) /AF files, emitted during the page and
        // annotation passes above, must also appear in the /EmbeddedFiles name tree.
        tree_refs.extend(af_tree_refs.iter().cloned());

        // Tagged-PDF logical structure (§14.7): a /StructTreeRoot whose single child is a `Document`
        // element grouping the supplied tree; each element's /K holds nested element refs and/or
        // marked-content references (`/MCR`, so an element can span page breaks). A /ParentTree
        // (§14.7.4.4) maps each page's /StructParents key to an array, indexed by MCID, of the owning
        // element. Built after pages so an /MCR's /Pg can reference them.
        let struct_tree_root_id = if tagged {
            let root_id = alloc();
            let document = StructElem {
                tag: "Document".to_string(),
                alt: None,
                actual_text: None,
                lang: None,
                ns: self.struct_namespace.clone(),
                af: Vec::new(),
                id: None,
                refs: Vec::new(),
                attrs: Vec::new(),
                kids: self
                    .structure
                    .iter()
                    .cloned()
                    .map(StructKid::Child)
                    .collect(),
            };

            // Structure namespaces (§14.7.4, PDF 2.0): gather every distinct /NS URI in the tree —
            // plus the namespaces named by the /RoleMapNS entries — and emit one /Namespace
            // dictionary each, keyed by URI so each element can reference its own. Ids are
            // allocated first so a role map can reference its target's namespace dictionary. The
            // list goes in /StructTreeRoot /Namespaces; any namespace makes this 2.0.
            let mut ns_uris: Vec<String> = Vec::new();
            collect_namespaces(&document, &mut ns_uris);
            for entry in &self.role_maps {
                if !ns_uris.contains(&entry.ns) {
                    ns_uris.push(entry.ns.clone());
                }
                if let Some(target_ns) = &entry.target_ns {
                    if !ns_uris.contains(target_ns) {
                        ns_uris.push(target_ns.clone());
                    }
                }
            }
            let ns_ids: Vec<(String, ObjectId)> =
                ns_uris.iter().map(|uri| (uri.clone(), alloc())).collect();
            for (uri, id) in &ns_ids {
                let mut d = Dictionary::new();
                d.insert(Name::from("Type"), Object::Name(Name::from("Namespace")));
                d.insert(
                    Name::from("NS"),
                    Object::String(PdfString::from(text_string(uri))),
                );
                // The namespace's role map (`/RoleMapNS`, §14.7.4): custom type → a standard type
                // name (default namespace) or [name, target-namespace] pair.
                let mut role_map = Dictionary::new();
                for entry in self.role_maps.iter().filter(|e| &e.ns == uri) {
                    let value = match &entry.target_ns {
                        None => Object::Name(Name::from(entry.target.as_str())),
                        Some(target_ns) => {
                            let target_id = ns_ids
                                .iter()
                                .find(|(u, _)| u == target_ns)
                                .map(|(_, id)| *id)
                                .unwrap_or(*id);
                            Object::Array(Array::from(vec![
                                Object::Name(Name::from(entry.target.as_str())),
                                Object::Reference(target_id),
                            ]))
                        }
                    };
                    role_map.insert(Name::from(entry.custom.as_str()), value);
                }
                if !role_map.is_empty() {
                    d.insert(Name::from("RoleMapNS"), Object::Dictionary(role_map));
                }
                // The namespace's /Schema (§14.7.4): a file specification of the schema that
                // defines it, embedded and listed in /EmbeddedFiles like any attachment.
                if let Some((_, schema)) = self.ns_schemas.iter().find(|(u, _)| u == uri) {
                    let fs_id = emit_filespec(schema, &mut alloc, &mut objects);
                    tree_refs.push((schema.name.clone(), fs_id));
                    d.insert(Name::from("Schema"), Object::Reference(fs_id));
                }
                objects.push((*id, Object::Dictionary(d)));
            }

            let mut per_page_parents: Vec<Vec<Option<ObjectId>>> =
                vec![Vec::new(); self.pages.len()];
            let mut id_entries: Vec<(Vec<u8>, ObjectId)> = Vec::new();
            let mut widget_parents: Vec<(ObjectId, ObjectId)> = Vec::new();
            let mut ref_requests: Vec<(ObjectId, Vec<Vec<u8>>)> = Vec::new();
            let doc_id = emit_struct_elem(
                &document,
                root_id,
                &mut alloc,
                &mut objects,
                &StructCtx {
                    page_ids: &page_ids,
                    ns_ids: &ns_ids,
                    widgets: &widget_refs,
                    annots: &annot_refs,
                    utf8: self.utf8,
                },
                &mut StructSinks {
                    per_page: &mut per_page_parents,
                    af: &mut tree_refs,
                    ids: &mut id_entries,
                    widget_parents: &mut widget_parents,
                    ref_requests: &mut ref_requests,
                },
            );

            // Resolve structure destinations (`/SD`, §12.3.2.3, PDF 2.0) now that every element
            // and its /ID exist: patch each queued GoTo action with /SD [elem /Fit], and retarget
            // its /D page fallback to the element's page (the first /MCR /Pg among its kids) when
            // one is found. An unknown ID leaves the action's original /D fallback untouched.
            for (annot_id, target) in &sd_requests {
                let Some((_, elem_id)) = id_entries.iter().find(|(key, _)| key == target) else {
                    continue;
                };
                let elem_page = objects
                    .iter()
                    .find(|(id, _)| id == elem_id)
                    .and_then(|(_, obj)| obj.as_dict())
                    .and_then(|d| d.get(&Name::from("K")))
                    .and_then(first_mcr_page);
                if let Some((_, Object::Dictionary(annot))) =
                    objects.iter_mut().find(|(id, _)| id == annot_id)
                {
                    let Some(Object::Dictionary(mut action)) = annot.get(&Name::from("A")).cloned()
                    else {
                        continue;
                    };
                    action.insert(
                        Name::from("SD"),
                        Object::Array(Array::from(vec![
                            Object::Reference(*elem_id),
                            Object::Name(Name::from("Fit")),
                        ])),
                    );
                    if let Some(page) = elem_page {
                        action.insert(
                            Name::from("D"),
                            Object::Array(Array::from(vec![
                                Object::Reference(page),
                                Object::Name(Name::from("Fit")),
                            ])),
                        );
                    }
                    annot.insert(Name::from("A"), Object::Dictionary(action));
                }
            }

            // Resolve /Ref targets (§14.7.4.2, PDF 2.0) now that every element and its /ID is
            // emitted: each requested target ID becomes an indirect reference; unknown IDs are
            // skipped, and an element whose targets all vanished gets no /Ref at all.
            for (elem_id, targets) in &ref_requests {
                let refs: Vec<Object> = targets
                    .iter()
                    .filter_map(|t| {
                        id_entries
                            .iter()
                            .find(|(key, _)| key == t)
                            .map(|(_, id)| Object::Reference(*id))
                    })
                    .collect();
                if refs.is_empty() {
                    continue;
                }
                if let Some((_, Object::Dictionary(d))) =
                    objects.iter_mut().find(|(id, _)| id == elem_id)
                {
                    d.insert(Name::from("Ref"), Object::Array(Array::from(refs)));
                }
            }

            // The parent tree (§14.7.4.4): /Nums = [pageKey [owning-elem per MCID] …], then one key
            // per widget annotation referenced from the tree (§14.7.4.3 OBJR) — its value is the
            // owning element and the widget dictionary gets the matching /StructParent.
            let parent_tree_id = alloc();
            let mut nums = Vec::with_capacity((self.pages.len() + widget_parents.len()) * 2);
            for (page_index, slots) in per_page_parents.iter().enumerate() {
                let arr = slots
                    .iter()
                    .map(|slot| slot.map_or(Object::Null, Object::Reference))
                    .collect::<Vec<_>>();
                nums.push(Object::Integer(page_index as i64));
                nums.push(Object::Array(Array::from(arr)));
            }
            for (i, (widget_id, elem_id)) in widget_parents.iter().enumerate() {
                let key = (self.pages.len() + i) as i64;
                nums.push(Object::Integer(key));
                nums.push(Object::Reference(*elem_id));
                if let Some((_, Object::Dictionary(widget))) =
                    objects.iter_mut().find(|(id, _)| id == widget_id)
                {
                    widget.insert(Name::from("StructParent"), Object::Integer(key));
                }
            }
            let mut parent_tree = Dictionary::new();
            parent_tree.insert(Name::from("Nums"), Object::Array(Array::from(nums)));
            objects.push((parent_tree_id, Object::Dictionary(parent_tree)));

            let mut str_root = Dictionary::new();
            str_root.insert(
                Name::from("Type"),
                Object::Name(Name::from("StructTreeRoot")),
            );
            str_root.insert(Name::from("K"), Object::Reference(doc_id));
            str_root.insert(Name::from("ParentTree"), Object::Reference(parent_tree_id));
            str_root.insert(
                Name::from("ParentTreeNextKey"),
                Object::Integer((self.pages.len() + widget_parents.len()) as i64),
            );
            // The /IDTree name tree (§14.7.4.5) maps element /IDs (byte strings, sorted) to their
            // elements — required once any element carries an /ID. IDs must be unique (§14.7.4.2);
            // a duplicate keeps its first element.
            if !id_entries.is_empty() {
                id_entries.sort_by(|(a, _), (b, _)| a.cmp(b));
                id_entries.dedup_by(|(a, _), (b, _)| a == b);
                let mut names = Vec::with_capacity(id_entries.len() * 2);
                for (key, elem_id) in &id_entries {
                    names.push(Object::String(PdfString::from(key.clone())));
                    names.push(Object::Reference(*elem_id));
                }
                let idtree_id = alloc();
                let mut idtree = Dictionary::new();
                idtree.insert(Name::from("Names"), Object::Array(Array::from(names)));
                objects.push((idtree_id, Object::Dictionary(idtree)));
                str_root.insert(Name::from("IDTree"), Object::Reference(idtree_id));
            }
            if !ns_ids.is_empty() {
                let refs = ns_ids
                    .iter()
                    .map(|(_, id)| Object::Reference(*id))
                    .collect::<Vec<_>>();
                str_root.insert(Name::from("Namespaces"), Object::Array(Array::from(refs)));
            }
            objects.push((root_id, Object::Dictionary(str_root)));
            Some(root_id)
        } else {
            None
        };

        // The /EmbeddedFiles name tree (§7.7.4) lists every embedded file — catalog-, page- and
        // struct-element-level (the struct-element /AF files were pushed during emit_struct_elem).
        let names_id = if tree_refs.is_empty() {
            None
        } else {
            tree_refs.sort_by(|a, b| a.0.cmp(&b.0)); // name-tree entries must be sorted by key
            let mut names = Vec::with_capacity(tree_refs.len() * 2);
            for (name, id) in &tree_refs {
                names.push(Object::String(PdfString::from(text_string(name))));
                names.push(Object::Reference(*id));
            }
            let mut ef_tree = Dictionary::new();
            ef_tree.insert(Name::from("Names"), Object::Array(Array::from(names)));
            let mut names_dict = Dictionary::new();
            names_dict.insert(Name::from("EmbeddedFiles"), Object::Dictionary(ef_tree));
            let id = alloc();
            objects.push((id, Object::Dictionary(names_dict)));
            Some(id)
        };

        // Document parts (§14.12, PDF 2.0): /DPartRoot → root DPart → one leaf DPart per part,
        // each spanning a page range via /Start (and /End). Each covered page also gets a /DPart
        // back-reference (§14.12.3), and a leaf may carry /DPM Document Part Metadata (§14.12.4).
        // The catalog /DPartRoot makes the header auto-stamp 2.0 (pdf_writer::min_version).
        let mut dpart_leaf_ids: Vec<ObjectId> = Vec::new();
        let dpart_root_id = (!self.document_parts.is_empty() && !page_ids.is_empty()).then(|| {
            let dpartroot_id = alloc();
            let root_node_id = alloc();
            let last_page = page_ids.len() - 1;
            // (page index → owning leaf DPart id) so each page can be given a /DPart back-reference.
            let mut page_dpart: Vec<(usize, ObjectId)> = Vec::new();
            let leaf_refs: Vec<Object> = self
                .document_parts
                .iter()
                .map(|part| {
                    let first = part.first_page.min(last_page);
                    let last = part.last_page.min(last_page).max(first);
                    let leaf_id = alloc();
                    dpart_leaf_ids.push(leaf_id);
                    let mut leaf = Dictionary::new();
                    leaf.insert(Name::from("Type"), Object::Name(Name::from("DPart")));
                    leaf.insert(Name::from("Parent"), Object::Reference(root_node_id));
                    leaf.insert(Name::from("Start"), Object::Reference(page_ids[first]));
                    if last != first {
                        leaf.insert(Name::from("End"), Object::Reference(page_ids[last]));
                    }
                    // /DPM — Document Part Metadata (§14.12.4), when supplied.
                    if !part.dpm.is_empty() {
                        let mut dpm = Dictionary::new();
                        for (key, value) in &part.dpm {
                            dpm.insert(
                                Name::from(key.as_str()),
                                Object::String(PdfString::from(self.encode_text(value))),
                            );
                        }
                        leaf.insert(Name::from("DPM"), Object::Dictionary(dpm));
                    }
                    objects.push((leaf_id, Object::Dictionary(leaf)));
                    for p in first..=last {
                        page_dpart.push((p, leaf_id));
                    }
                    Object::Reference(leaf_id)
                })
                .collect();
            // Page → leaf back-reference (§14.12.3): a page's /DPart names the leaf it belongs to.
            for (p, leaf_id) in page_dpart {
                if let Some((_, Object::Dictionary(page))) =
                    objects.iter_mut().find(|(id, _)| *id == page_ids[p])
                {
                    page.insert(Name::from("DPart"), Object::Reference(leaf_id));
                }
            }
            let mut root_node = Dictionary::new();
            root_node.insert(Name::from("Type"), Object::Name(Name::from("DPart")));
            root_node.insert(Name::from("Parent"), Object::Reference(dpartroot_id));
            root_node.insert(Name::from("DParts"), Object::Array(Array::from(leaf_refs)));
            objects.push((root_node_id, Object::Dictionary(root_node)));
            let mut dpartroot = Dictionary::new();
            dpartroot.insert(Name::from("Type"), Object::Name(Name::from("DPartRoot")));
            dpartroot.insert(Name::from("DPartRootNode"), Object::Reference(root_node_id));
            objects.push((dpartroot_id, Object::Dictionary(dpartroot)));
            dpartroot_id
        });
        // Patch queued GoToDp actions (§12.6.4.5) with their leaf /Dp reference, now that the
        // DPart leaves exist. With no parts declared the dangling action is dropped entirely
        // (a GoToDp without its required /Dp would be invalid).
        for (annot_id, part_index) in &dp_requests {
            let Some((_, Object::Dictionary(annot))) =
                objects.iter_mut().find(|(id, _)| id == annot_id)
            else {
                continue;
            };
            match dpart_leaf_ids.get((*part_index).min(dpart_leaf_ids.len().saturating_sub(1))) {
                Some(leaf_id) => {
                    if let Some(Object::Dictionary(action)) = annot.get(&Name::from("A")) {
                        let mut action = action.clone();
                        action.insert(Name::from("Dp"), Object::Reference(*leaf_id));
                        annot.insert(Name::from("A"), Object::Dictionary(action));
                    }
                }
                None => {
                    annot.remove(&Name::from("A"));
                }
            }
        }

        let mut catalog = Dictionary::new();
        catalog.insert(Name::from("Type"), Object::Name(Name::from("Catalog")));
        catalog.insert(Name::from("Pages"), Object::Reference(pages_id));
        if let Some(id) = dpart_root_id {
            catalog.insert(Name::from("DPartRoot"), Object::Reference(id));
        }
        if let Some(id) = names_id {
            catalog.insert(Name::from("Names"), Object::Reference(id));
        }
        if !filespec_refs.is_empty() {
            let af: Vec<Object> = filespec_refs
                .iter()
                .map(|(_, id)| Object::Reference(*id))
                .collect();
            catalog.insert(Name::from("AF"), Object::Array(Array::from(af)));
        }
        if let Some(id) = outlines_id {
            catalog.insert(Name::from("Outlines"), Object::Reference(id));
            catalog.insert(
                Name::from("PageMode"),
                Object::Name(Name::from("UseOutlines")),
            );
        }
        if let Some(id) = metadata_id {
            catalog.insert(Name::from("Metadata"), Object::Reference(id));
        }
        if let Some(id) = output_intent_id {
            catalog.insert(
                Name::from("OutputIntents"),
                Object::Array(Array::from(vec![Object::Reference(id)])),
            );
        }
        if let Some(id) = acroform_id {
            catalog.insert(Name::from("AcroForm"), Object::Reference(id));
        }
        // Unencrypted wrapper (§7.6.7): a hidden collection whose initial document is the
        // encrypted payload, so a processor with the right filter opens it directly (§12.3.5).
        if let Some(payload) = &self.encrypted_payload {
            let mut collection = Dictionary::new();
            collection.insert(
                Name::from("D"),
                Object::String(PdfString::from(payload.file_name.as_bytes().to_vec())),
            );
            collection.insert(Name::from("View"), Object::Name(Name::from("H")));
            catalog.insert(Name::from("Collection"), Object::Dictionary(collection));
        }
        // Developer extensions (§7.12): a direct /Extensions dictionary in the catalog (the spec
        // forbids indirect objects here). Prefixes keep first-declaration order; one declaration
        // per prefix stays the 1.7 dictionary form, several become the 2.0 array form (Table 48).
        if !self.developer_extensions.is_empty() {
            let mut extensions = Dictionary::new();
            extensions.insert(Name::from("Type"), Object::Name(Name::from("Extensions")));
            let mut prefixes: Vec<&str> = Vec::new();
            for ext in &self.developer_extensions {
                if !prefixes.contains(&ext.prefix.as_str()) {
                    prefixes.push(&ext.prefix);
                }
            }
            for prefix in prefixes {
                let entries: Vec<&crate::DeveloperExtension> = self
                    .developer_extensions
                    .iter()
                    .filter(|e| e.prefix == prefix)
                    .collect();
                let value = if let [single] = entries.as_slice() {
                    Object::Dictionary(developer_extension_dict(single, false, self.utf8))
                } else {
                    Object::Array(Array::from(
                        entries
                            .iter()
                            .map(|e| {
                                Object::Dictionary(developer_extension_dict(e, true, self.utf8))
                            })
                            .collect::<Vec<_>>(),
                    ))
                };
                extensions.insert(Name::from(prefix), value);
            }
            catalog.insert(Name::from("Extensions"), Object::Dictionary(extensions));
        }
        // Page labels (§12.4.2): a /PageLabels number tree keyed by first page index of each
        // range. The tree must have a key 0 — a plain-decimal range is synthesised if the author
        // did not cover page 0.
        if !self.page_labels.is_empty() {
            let mut ranges = self.page_labels.clone();
            ranges.sort_by_key(|r| r.first_page);
            ranges.dedup_by_key(|r| r.first_page);
            let mut nums: Vec<Object> = Vec::with_capacity(ranges.len() * 2 + 2);
            if ranges.first().is_none_or(|r| r.first_page != 0) {
                let mut d = Dictionary::new();
                d.insert(Name::from("S"), Object::Name(Name::from("D")));
                nums.push(Object::Integer(0));
                nums.push(Object::Dictionary(d));
            }
            for range in &ranges {
                let mut d = Dictionary::new();
                if let Some(style) = range.style {
                    d.insert(Name::from("S"), Object::Name(Name::from(style.name())));
                }
                if let Some(prefix) = &range.prefix {
                    d.insert(
                        Name::from("P"),
                        Object::String(PdfString::from(self.encode_text(prefix))),
                    );
                }
                if let Some(start) = range.start {
                    d.insert(Name::from("St"), Object::Integer(i64::from(start.max(1))));
                }
                nums.push(Object::Integer(range.first_page as i64));
                nums.push(Object::Dictionary(d));
            }
            let mut labels = Dictionary::new();
            labels.insert(Name::from("Nums"), Object::Array(Array::from(nums)));
            catalog.insert(Name::from("PageLabels"), Object::Dictionary(labels));
        }
        if let Some(id) = struct_tree_root_id {
            catalog.insert(Name::from("StructTreeRoot"), Object::Reference(id));
            let mut mark_info = Dictionary::new();
            mark_info.insert(Name::from("Marked"), Object::Boolean(true));
            catalog.insert(Name::from("MarkInfo"), Object::Dictionary(mark_info));
        }
        if let Some(lang) = &self.lang {
            catalog.insert(
                Name::from("Lang"),
                Object::String(PdfString::from(text_string(lang))),
            );
        }
        if self.display_doc_title {
            let mut prefs = Dictionary::new();
            prefs.insert(Name::from("DisplayDocTitle"), Object::Boolean(true));
            catalog.insert(Name::from("ViewerPreferences"), Object::Dictionary(prefs));
        }
        objects.push((catalog_id, Object::Dictionary(catalog)));

        // The document information dictionary (§14.3.3), referenced from the trailer if non-empty.
        // Values are encoded now so the `utf8` flag (§7.9.2.2) applies regardless of call order.
        let info_id = if self.info.is_empty() {
            None
        } else {
            let mut info_dict = Dictionary::new();
            for (key, value) in &self.info {
                info_dict.insert(
                    Name::from(key.as_str()),
                    Object::String(PdfString::from(self.encode_text(value))),
                );
            }
            let id = alloc();
            objects.push((id, Object::Dictionary(info_dict)));
            Some(id)
        };

        // Header version (§7.5.2). Target path (build_for): downgrade what has a compatible
        // form, then gate — any construct still above the target is a hard error naming it —
        // and stamp exactly the target. Default path (build): stamp the minimum the content
        // requires, unless pinned higher (the pin is a floor — never stamp below what the
        // constructs actually need).
        // Marked-content /AF (§14.13.5) is invisible to the object-set scan (it lives in the
        // content stream + a plain /Properties array), so its 2.0 floor is applied here — the
        // "caller-side floor" pattern the version table documents for writer-choice features.
        let content_af_floor = !self.content_af_props.is_empty();
        let version = match target {
            Some(t) => {
                if t < (2, 0) {
                    if content_af_floor {
                        return Err(crate::DocError::TargetVersionExceeded {
                            construct: "marked-content associated files (§14.13.5)".to_owned(),
                            required: (2, 0),
                            target: t,
                        });
                    }
                    // UTF-8 text strings (§7.9.2.2, PDF 2.0) have a pre-2.0 compatible form:
                    // re-encode them as UTF-16BE instead of refusing (downgrade discipline).
                    for (_, obj) in &mut objects {
                        *obj = downgrade_utf8_strings(obj);
                    }
                }
                if let Some(v) = pdf_writer::version_violation(&objects, t) {
                    return Err(crate::DocError::TargetVersionExceeded {
                        construct: v.construct.to_owned(),
                        required: v.version,
                        target: t,
                    });
                }
                t
            }
            None => {
                let mut inferred = pdf_writer::min_version(&objects);
                if content_af_floor && inferred < (2, 0) {
                    inferred = (2, 0);
                }
                match self.version {
                    Some(v) if v > inferred => v,
                    _ => inferred,
                }
            }
        };

        // An explicit `file_id` (PDF/A, signing) wins; otherwise the writer synthesizes one when the
        // inferred version requires `/ID` in the trailer (§7.5.5, PDF 2.0 on).
        Ok(write_document(
            &objects,
            catalog_id,
            info_id,
            version,
            self.file_id.as_deref(),
        ))
    }
}
