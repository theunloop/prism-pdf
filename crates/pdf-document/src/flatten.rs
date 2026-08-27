//! Flattening interactive forms (ISO 32000-1 §12.7.4 / §12.5.5): bake each widget's current
//! appearance into the page content as static graphics, then drop the widgets and `/AcroForm` so
//! the result is a plain, non-interactive PDF that looks identical.
//!
//! Each flattened widget is painted by invoking its normal appearance stream (`/AP /N`, a Form
//! XObject) with the appearance-mapping transform of §12.5.5 (its `/BBox`×`/Matrix` mapped onto the
//! annotation `/Rect`). Widgets without a usable appearance reference are left interactive (this
//! does not *generate* appearances — that pairs with `/AP` synthesis, a follow-up). Output is a
//! full rewrite (§7.5).

use std::collections::HashMap;

use pdf_cos::{Array, Dictionary, Name, Object, ObjectId, Stream};
use pdf_writer::write_document;

use crate::{
    DocError, Document, Result, RewriteMode, SignatureEffect, StructureEffect, TransformReport,
};

impl Document {
    /// Flatten the document's form fields (§12.7.4): paint every widget that has a normal
    /// appearance into its page's content, remove those widgets from `/Annots`, drop the catalog
    /// `/AcroForm`, and return the rewritten PDF. A document with no forms is returned unchanged in
    /// structure (a clean rewrite).
    pub fn flatten_form(&self) -> Result<Vec<u8>> {
        let root = self.xref.root().ok_or(DocError::MissingCatalog)?;
        let info = self
            .xref
            .trailer
            .get(&Name::from("Info"))
            .and_then(Object::as_reference);
        let version = self.version().map_or((1, 7), |v| (v.major, v.minor));

        let mut objects = self.collect_objects()?;
        let by_number: HashMap<u32, usize> = objects
            .iter()
            .enumerate()
            .map(|(i, (id, _))| (id.number, i))
            .collect();
        let mut next_number = objects.iter().map(|(id, _)| id.number).max().unwrap_or(0) + 1;
        let mut new_objects: Vec<(ObjectId, Object)> = Vec::new();

        for (page_id, page) in self.page_entries()? {
            let Some(page_id) = page_id else {
                continue;
            };
            let Some(&index) = by_number.get(&page_id.number) else {
                continue;
            };
            self.flatten_page(
                index,
                &page,
                &mut objects,
                &mut new_objects,
                &mut next_number,
            );
        }

        // Drop the catalog's /AcroForm so the document is no longer a form (§12.7.2).
        if let Some(&index) = by_number.get(&root.number)
            && let Object::Dictionary(catalog) = &mut objects[index].1
        {
            catalog.remove(&Name::from("AcroForm"));
        }

        objects.extend(new_objects);
        Ok(write_document(
            &objects,
            root,
            info,
            version,
            self.preserved_file_id().as_deref(),
        ))
    }

    /// Flatten forms with explicit rewrite effects. Structure is retained, but widget object
    /// references in it may no longer identify live annotations (§12.7.4, §14.7).
    pub fn flatten_form_with_report(&self) -> Result<TransformReport> {
        Ok(TransformReport::new(
            self.flatten_form()?,
            RewriteMode::FullRewrite,
            SignatureEffect::Invalidated,
            StructureEffect::Invalidated,
        ))
    }

    /// Flatten one page: paint its widgets' appearances into a new content stream, append it to the
    /// page, fold the appearances into the page resources, and prune the painted widgets.
    fn flatten_page(
        &self,
        index: usize,
        page: &Dictionary,
        objects: &mut [(ObjectId, Object)],
        new_objects: &mut Vec<(ObjectId, Object)>,
        next_number: &mut u32,
    ) {
        let Some(annots) = page
            .get(&Name::from("Annots"))
            .and_then(|a| match self.resolve(a) {
                Ok(Object::Array(array)) => Some(array),
                _ => None,
            })
        else {
            return;
        };

        let mut content = Vec::new();
        let mut xobjects: Vec<(String, ObjectId)> = Vec::new();
        let mut kept: Vec<Object> = Vec::new();

        for entry in annots.iter() {
            let Ok(Object::Dictionary(annot)) = self.resolve(entry) else {
                continue;
            };
            match self.widget_paint(&annot, xobjects.len()) {
                Some((name, id, ops)) => {
                    content.extend_from_slice(&ops);
                    xobjects.push((name, id));
                }
                None => kept.push(entry.clone()),
            }
        }

        if xobjects.is_empty() {
            return; // nothing on this page to flatten
        }

        // A new content stream holding the painting, appended after the page's existing content.
        let stream_id = ObjectId::new(*next_number, 0);
        *next_number += 1;
        let mut sdict = Dictionary::new();
        sdict.insert(Name::from("Length"), Object::Integer(content.len() as i64));
        new_objects.push((stream_id, Object::Stream(Stream::new(sdict, content))));

        let Object::Dictionary(page_obj) = &mut objects[index].1 else {
            return;
        };
        append_content(page_obj, stream_id);
        merge_xobject_resources(page_obj, page, self, &xobjects);
        if kept.is_empty() {
            page_obj.remove(&Name::from("Annots"));
        } else {
            page_obj.insert(Name::from("Annots"), Object::Array(Array::from_vec(kept)));
        }
    }

    /// If `annot` is a widget with a usable normal appearance, return its resource name, the
    /// appearance Form XObject's id, and the content operators that paint it onto the page (§12.5.5).
    fn widget_paint(
        &self,
        annot: &Dictionary,
        ordinal: usize,
    ) -> Option<(String, ObjectId, Vec<u8>)> {
        if annot.get_name(&Name::from("Subtype")).map(Name::as_bytes) != Some(b"Widget") {
            return None;
        }
        let appearance_id = self.normal_appearance(annot)?;
        let Ok(Object::Stream(appearance)) = self.resolve(&Object::Reference(appearance_id)) else {
            return None;
        };
        let bbox = self.float_array(appearance.dict().get(&Name::from("BBox"))?)?;
        if bbox.len() < 4 {
            return None;
        }
        let matrix = self
            .float_array_opt(appearance.dict().get(&Name::from("Matrix")))
            .filter(|m| m.len() >= 6)
            .unwrap_or_else(|| vec![1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
        let rect = self.float_array(annot.get(&Name::from("Rect"))?)?;
        if rect.len() < 4 {
            return None;
        }
        let m = appearance_matrix(&bbox, &matrix, &rect)?;

        let name = format!("PrismFlat{ordinal}");
        let ops = format!(
            "q {} {} {} {} {} {} cm /{name} Do Q\n",
            fmt(m[0]),
            fmt(m[1]),
            fmt(m[2]),
            fmt(m[3]),
            fmt(m[4]),
            fmt(m[5]),
        )
        .into_bytes();
        Some((name, appearance_id, ops))
    }

    /// The object id of a widget's normal appearance Form XObject (`/AP /N`, §12.5.5): the stream
    /// directly, or — for a button — the entry of the appearance subdictionary named by `/AS`. Only
    /// indirect (referenced) appearances are returned, the standard form.
    fn normal_appearance(&self, annot: &Dictionary) -> Option<ObjectId> {
        let Ok(Object::Dictionary(ap)) = self.resolve(annot.get(&Name::from("AP"))?) else {
            return None;
        };
        let n = ap.get(&Name::from("N"))?;
        match self.resolve(n).ok()? {
            Object::Stream(_) => n.as_reference(),
            // A button's /N is keyed by appearance state (/AS).
            Object::Dictionary(states) => {
                let state = annot.get_name(&Name::from("AS"))?;
                let entry = states.get(state)?;
                matches!(self.resolve(entry), Ok(Object::Stream(_)))
                    .then(|| entry.as_reference())
                    .flatten()
            }
            _ => None,
        }
    }

    /// Resolve `value` to a vector of numbers (§7.3.6), or `None` if it isn't a numeric array.
    fn float_array(&self, value: &Object) -> Option<Vec<f64>> {
        match self.resolve(value).ok()? {
            Object::Array(array) => Some(
                array
                    .iter()
                    .filter_map(|x| self.resolve(x).ok()?.as_f64())
                    .collect(),
            ),
            _ => None,
        }
    }

    /// As [`Self::float_array`] but for an optional entry.
    fn float_array_opt(&self, value: Option<&Object>) -> Option<Vec<f64>> {
        self.float_array(value?)
    }
}

/// Append the content stream `stream_id` after a page's existing `/Contents` (§7.8.2), turning a
/// lone stream into an array so the painting runs last.
fn append_content(page: &mut Dictionary, stream_id: ObjectId) {
    let mut contents = match page.get(&Name::from("Contents")) {
        Some(Object::Array(array)) => array.iter().cloned().collect(),
        Some(other) => vec![other.clone()],
        None => Vec::new(),
    };
    contents.push(Object::Reference(stream_id));
    page.insert(
        Name::from("Contents"),
        Object::Array(Array::from_vec(contents)),
    );
}

/// Add the flattened appearances to the page's `/Resources /XObject`, starting from the page's
/// (possibly inherited) resources so nothing already there is lost.
fn merge_xobject_resources(
    page: &mut Dictionary,
    resolved_page: &Dictionary,
    doc: &Document,
    xobjects: &[(String, ObjectId)],
) {
    // Start from the resolved (inheritance-folded) resources, so setting them on the page does not
    // shadow inherited resources the content still needs.
    let mut resources = match resolved_page
        .get(&Name::from("Resources"))
        .map(|r| doc.resolve(r))
    {
        Some(Ok(Object::Dictionary(d))) => d,
        _ => Dictionary::new(),
    };
    let mut xobject = match resources
        .get(&Name::from("XObject"))
        .map(|x| doc.resolve(x))
    {
        Some(Ok(Object::Dictionary(d))) => d,
        _ => Dictionary::new(),
    };
    for (name, id) in xobjects {
        xobject.insert(Name::from(name.as_str()), Object::Reference(*id));
    }
    resources.insert(Name::from("XObject"), Object::Dictionary(xobject));
    page.insert(Name::from("Resources"), Object::Dictionary(resources));
}

/// The §12.5.5 appearance transform: map the appearance `bbox` (after its `matrix`) onto the
/// annotation `rect`. `None` if the transformed box is degenerate (zero width/height).
fn appearance_matrix(bbox: &[f64], matrix: &[f64], rect: &[f64]) -> Option<[f64; 6]> {
    let apply = |x: f64, y: f64| {
        (
            matrix[0] * x + matrix[2] * y + matrix[4],
            matrix[1] * x + matrix[3] * y + matrix[5],
        )
    };
    let corners = [
        apply(bbox[0], bbox[1]),
        apply(bbox[2], bbox[1]),
        apply(bbox[2], bbox[3]),
        apply(bbox[0], bbox[3]),
    ];
    let (mut tx0, mut ty0, mut tx1, mut ty1) = (
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    );
    for (x, y) in corners {
        tx0 = tx0.min(x);
        ty0 = ty0.min(y);
        tx1 = tx1.max(x);
        ty1 = ty1.max(y);
    }
    let (rx0, rx1) = (rect[0].min(rect[2]), rect[0].max(rect[2]));
    let (ry0, ry1) = (rect[1].min(rect[3]), rect[1].max(rect[3]));
    let (tw, th) = (tx1 - tx0, ty1 - ty0);
    if tw.abs() < 1e-6 || th.abs() < 1e-6 {
        return None;
    }
    let sx = (rx1 - rx0) / tw;
    let sy = (ry1 - ry0) / th;
    Some([sx, 0.0, 0.0, sy, rx0 - sx * tx0, ry0 - sy * ty0])
}

/// Format a coordinate for a content stream: trim a trailing `.0` so integers stay compact.
fn fmt(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{}", value as i64)
    } else {
        format!("{value:.4}")
    }
}
