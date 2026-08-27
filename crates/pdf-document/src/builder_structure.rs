use super::*;

/// Immutable context threaded through [`emit_struct_elem`].
pub(super) struct StructCtx<'a> {
    /// The emitted page objects, by page index.
    pub(super) page_ids: &'a [ObjectId],
    /// The shared `/Namespace` objects (§14.7.4), by URI.
    pub(super) ns_ids: &'a [(String, ObjectId)],
    /// Each form field's widget annotation and its page, in `add_form_field` order — resolves
    /// [`StructKid::Widget`] indices.
    pub(super) widgets: &'a [(ObjectId, ObjectId)],
    /// Each annotation and its page, in `add_annotation` order — resolves
    /// [`StructKid::Annotation`] indices.
    pub(super) annots: &'a [(ObjectId, ObjectId)],
    /// Whether text strings (e.g. `/Alt`) are written as UTF-8 (§7.9.2.2, PDF 2.0).
    pub(super) utf8: bool,
}

/// Mutable accumulators threaded through [`emit_struct_elem`].
pub(super) struct StructSinks<'a> {
    /// Per page: MCID → owning element, for the parent tree (§14.7.4.4).
    pub(super) per_page: &'a mut [Vec<Option<ObjectId>>],
    /// `(name, filespec)` entries for the `/EmbeddedFiles` name tree (element `/AF`, §14.13.6).
    pub(super) af: &'a mut Vec<(String, ObjectId)>,
    /// `/ID` → element, for the `/IDTree` (§14.7.4.5).
    pub(super) ids: &'a mut Vec<(Vec<u8>, ObjectId)>,
    /// Widget/annotation object → owning element, for `/StructParent` + the parent tree
    /// (§14.7.4.3 — one entry per `/OBJR` emitted).
    pub(super) widget_parents: &'a mut Vec<(ObjectId, ObjectId)>,
    /// Element → requested `/Ref` target IDs (§14.7.4.2, PDF 2.0), resolved once all elements
    /// (and their `/ID`s) are emitted.
    pub(super) ref_requests: &'a mut Vec<(ObjectId, Vec<Vec<u8>>)>,
}

/// A page dictionary (§7.7.3.3) referencing its content stream, media box and resources.
/// Recursively emit a structure element and its subtree (§14.7), returning the element's object id.
/// Content children become `/MCR` marked-content references (each carrying its own `/Pg`, so an
/// element may span page breaks) and are recorded in `sinks.per_page` (MCID → owning element) for
/// the parent tree; widget children become `/OBJR` object references; element children are emitted
/// depth-first and referenced.
pub(super) fn emit_struct_elem(
    elem: &StructElem,
    parent: ObjectId,
    alloc: &mut dyn FnMut() -> ObjectId,
    objects: &mut Vec<(ObjectId, Object)>,
    ctx: &StructCtx<'_>,
    sinks: &mut StructSinks<'_>,
) -> ObjectId {
    let id = alloc();
    let mut kids: Vec<Object> = Vec::with_capacity(elem.kids.len());
    for kid in &elem.kids {
        match kid {
            StructKid::Content { page, mcid } => {
                let page = (*page).min(ctx.page_ids.len().saturating_sub(1));
                let mut mcr = Dictionary::new();
                mcr.insert(Name::from("Type"), Object::Name(Name::from("MCR")));
                mcr.insert(Name::from("Pg"), Object::Reference(ctx.page_ids[page]));
                mcr.insert(Name::from("MCID"), Object::Integer(i64::from(*mcid)));
                kids.push(Object::Dictionary(mcr));

                let slots = &mut sinks.per_page[page];
                let idx = *mcid as usize;
                if slots.len() <= idx {
                    slots.resize(idx + 1, None);
                }
                slots[idx] = Some(id);
            }
            StructKid::Child(child) => {
                let child_id = emit_struct_elem(child, id, alloc, objects, ctx, sinks);
                kids.push(Object::Reference(child_id));
            }
            StructKid::Widget { field } => {
                // An /OBJR object reference (§14.7.4.3) to the widget of form field `field` — this
                // nests the widget in the structure (PDF/UA-1 §7.18.4). The widget's /StructParent
                // and its parent-tree entry are patched in once all keys are known; an out-of-range
                // index is skipped.
                if let Some((widget_id, page_id)) = ctx.widgets.get(*field) {
                    let mut objr = Dictionary::new();
                    objr.insert(Name::from("Type"), Object::Name(Name::from("OBJR")));
                    objr.insert(Name::from("Obj"), Object::Reference(*widget_id));
                    objr.insert(Name::from("Pg"), Object::Reference(*page_id));
                    kids.push(Object::Dictionary(objr));
                    sinks.widget_parents.push((*widget_id, id));
                }
            }
            StructKid::Annotation { index } => {
                // As above, for a plain annotation (`add_annotation` order) — this nests e.g. a
                // link annotation in a `Link` element (PDF/UA-1 §7.18.5 / UA-2 §8.2.5.20).
                if let Some((annot_id, page_id)) = ctx.annots.get(*index) {
                    let mut objr = Dictionary::new();
                    objr.insert(Name::from("Type"), Object::Name(Name::from("OBJR")));
                    objr.insert(Name::from("Obj"), Object::Reference(*annot_id));
                    objr.insert(Name::from("Pg"), Object::Reference(*page_id));
                    kids.push(Object::Dictionary(objr));
                    sinks.widget_parents.push((*annot_id, id));
                }
            }
        }
    }

    let mut d = Dictionary::new();
    d.insert(Name::from("Type"), Object::Name(Name::from("StructElem")));
    d.insert(Name::from("S"), Object::Name(Name::from(elem.tag.as_str())));
    d.insert(Name::from("P"), Object::Reference(parent));
    // A lone child is written directly; otherwise an array (§14.7.4.3).
    let k = if kids.len() == 1 {
        kids.into_iter().next().unwrap_or(Object::Null)
    } else {
        Object::Array(Array::from(kids))
    };
    d.insert(Name::from("K"), k);
    if let Some(alt) = &elem.alt {
        d.insert(
            Name::from("Alt"),
            Object::String(PdfString::from(text_string_maybe_utf8(alt, ctx.utf8))),
        );
    }
    // Replacement text (`/ActualText`, §14.9.4) — the UA-2 alternative to /Alt (§8.2.5.28).
    if let Some(actual) = &elem.actual_text {
        d.insert(
            Name::from("ActualText"),
            Object::String(PdfString::from(text_string_maybe_utf8(actual, ctx.utf8))),
        );
    }
    // A per-element language change (`/Lang`, §14.9.2 — PDF/UA wants these declared).
    if let Some(lang) = &elem.lang {
        d.insert(
            Name::from("Lang"),
            Object::String(PdfString::from(text_string(lang))),
        );
    }
    // The element identifier (`/ID`, §14.7.4.2) — a byte string, also recorded for the /IDTree.
    if let Some(elem_id) = &elem.id {
        let bytes = elem_id.as_bytes().to_vec();
        d.insert(
            Name::from("ID"),
            Object::String(PdfString::from(bytes.clone())),
        );
        sinks.ids.push((bytes, id));
    }
    // Structure attributes (`/A`, §14.7.6): one dictionary per owner.
    if let Some(a) = attr_object(&elem.attrs, ctx.utf8) {
        d.insert(Name::from("A"), a);
    }
    // `/Ref` targets (§14.7.4.2, PDF 2.0) resolve after the whole tree is emitted — queue them.
    if !elem.refs.is_empty() {
        sinks.ref_requests.push((
            id,
            elem.refs.iter().map(|r| r.as_bytes().to_vec()).collect(),
        ));
    }
    // The element's structure namespace (§14.7.4, PDF 2.0): reference the shared /Namespace object.
    if let Some(uri) = &elem.ns
        && let Some((_, ns_id)) = ctx.ns_ids.iter().find(|(u, _)| u == uri)
    {
        d.insert(Name::from("NS"), Object::Reference(*ns_id));
    }
    // Associated files on the element (`/AF`, §14.13.6, PDF 2.0): emit each filespec, list it in the
    // name tree (via `sinks.af`) and reference it here. This is the 2.0-preferred /AF placement.
    if let Some(af) = emit_af_array(&elem.af, alloc, objects, sinks.af) {
        d.insert(Name::from("AF"), af);
    }
    objects.push((id, Object::Dictionary(d)));
    id
}

/// The `/Pg` page reference of the first marked-content reference found in a structure element's
/// `/K` value — used to point a structure destination's `/D` page fallback at the element's page.
pub(super) fn first_mcr_page(k: &Object) -> Option<ObjectId> {
    match k {
        Object::Dictionary(d) => match d.get(&Name::from("Pg")) {
            Some(Object::Reference(id)) => Some(*id),
            _ => None,
        },
        Object::Array(a) => a.iter().find_map(first_mcr_page),
        _ => None,
    }
}

/// The `/A` value for `attrs` (§14.7.6): each [`StructAttr`] becomes an attribute dictionary with
/// its owner in `/O`; a lone dictionary is written directly, several as an array. `None` if empty.
pub(super) fn attr_object(attrs: &[StructAttr], utf8: bool) -> Option<Object> {
    if attrs.is_empty() {
        return None;
    }
    let mut dicts: Vec<Object> = Vec::with_capacity(attrs.len());
    for attr in attrs {
        let mut d = Dictionary::new();
        d.insert(
            Name::from("O"),
            Object::Name(Name::from(attr.owner.as_str())),
        );
        for (key, value) in &attr.entries {
            let obj = match value {
                AttrValue::Name(n) => Object::Name(Name::from(n.as_str())),
                AttrValue::Int(i) => Object::Integer(*i),
                AttrValue::Text(t) => {
                    Object::String(PdfString::from(text_string_maybe_utf8(t, utf8)))
                }
            };
            d.insert(Name::from(key.as_str()), obj);
        }
        dicts.push(Object::Dictionary(d));
    }
    Some(if dicts.len() == 1 {
        dicts.into_iter().next().unwrap_or(Object::Null)
    } else {
        Object::Array(Array::from(dicts))
    })
}

/// Collect the distinct structure-namespace URIs (`/NS`, §14.7.4) used anywhere in `elem`'s subtree,
/// in first-seen order, into `out`.
pub(super) fn collect_namespaces(elem: &StructElem, out: &mut Vec<String>) {
    if let Some(uri) = &elem.ns
        && !out.contains(uri)
    {
        out.push(uri.clone());
    }
    for kid in &elem.kids {
        if let StructKid::Child(child) = kid {
            collect_namespaces(child, out);
        }
    }
}
