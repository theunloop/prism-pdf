//! Document editing operations (EPIC 4/Milestone M2): merge, split, and rotate.
//!
//! These build a new document by **importing** the object graph reachable from a set of pages into
//! a fresh, sequentially-numbered object set, remapping every indirect reference (§7.3.10) and
//! deduplicating shared objects per source document. The result is serialized by [`pdf_writer`].
//!
//! Pages are taken from [`Document::page_entries`], so inherited attributes (§7.7.3.4) are already
//! folded in and each imported page stands alone; its `/Parent` is dropped and rewritten to the
//! new page-tree node.

use std::collections::HashMap;

use pdf_cos::{Array, Dictionary, Name, Object, ObjectId, Stream};
use pdf_writer::write_document;

use crate::{
    DocError, Document, Result, RewriteMode, SignatureEffect, StructureEffect, TransformReport,
};

/// Bound on reference-chain / nesting depth while importing, against hostile input (DESIGN.md §3.4).
const MAX_IMPORT_DEPTH: usize = 1024;

/// Accumulates imported objects under fresh, ascending object numbers.
struct Builder {
    objects: Vec<(ObjectId, Object)>,
    next: u32,
}

impl Builder {
    fn new() -> Self {
        Self {
            objects: Vec::new(),
            next: 1,
        }
    }

    /// Allocate the next object id.
    fn reserve(&mut self) -> ObjectId {
        let id = ObjectId::new(self.next, 0);
        self.next += 1;
        id
    }

    fn put(&mut self, id: ObjectId, object: Object) {
        self.objects.push((id, object));
    }

    /// Finish into a complete PDF: build the `/Pages` node from `kids` and the `/Catalog`.
    fn finish(mut self, kids: Vec<Object>, catalog_id: ObjectId, pages_id: ObjectId) -> Vec<u8> {
        let mut pages = Dictionary::new();
        pages.insert(Name::from("Type"), Object::Name(Name::from("Pages")));
        pages.insert(Name::from("Count"), Object::Integer(kids.len() as i64));
        pages.insert(Name::from("Kids"), Object::Array(Array::from_vec(kids)));
        self.put(pages_id, Object::Dictionary(pages));

        let mut catalog = Dictionary::new();
        catalog.insert(Name::from("Type"), Object::Name(Name::from("Catalog")));
        catalog.insert(Name::from("Pages"), Object::Reference(pages_id));
        self.put(catalog_id, Object::Dictionary(catalog));

        // A merge builds a genuinely new document, so it takes a fresh identity (§14.4) rather than
        // any input's: `None` lets the writer synthesize one where the version requires it.
        write_document(&self.objects, catalog_id, None, (1, 7), None)
    }
}

/// A per-source-document map from old object number to its freshly-assigned id (dedup + cycles).
type Remap = HashMap<u32, ObjectId>;

/// Import one page (an effective page dict from [`Document::page_entries`]) into `builder`,
/// dropping `/Parent` and re-pointing it at `parent`. Returns the new page id.
fn import_page(
    builder: &mut Builder,
    doc: &Document,
    page: &Dictionary,
    map: &mut Remap,
    parent: ObjectId,
) -> Result<ObjectId> {
    let page_id = builder.reserve();
    let mut imported = Dictionary::new();
    let parent_key = Name::from("Parent");
    for (key, value) in page.iter() {
        if *key == parent_key {
            continue; // re-pointed below; importing it would drag in the whole original tree
        }
        // A page-subset / merge cannot faithfully carry the source's logical structure tree
        // (§14.7): the elements would dangle or renumber wrongly. We drop the structure (the new
        // document is valid but untagged), so we must also drop the page's parent-tree key and
        // structure tab order, which would otherwise be left pointing at a tree that no longer
        // exists.
        if matches!(key.as_bytes(), b"StructParents" | b"Tabs") {
            continue;
        }
        imported.insert(key.clone(), import_value(builder, doc, value, map, 0)?);
    }
    imported.insert(parent_key, Object::Reference(parent));
    builder.put(page_id, Object::Dictionary(imported));
    Ok(page_id)
}

/// Deep-copy `value` from `doc` into `builder`, remapping references.
fn import_value(
    builder: &mut Builder,
    doc: &Document,
    value: &Object,
    map: &mut Remap,
    depth: usize,
) -> Result<Object> {
    if depth > MAX_IMPORT_DEPTH {
        return Err(DocError::BadPageTree);
    }
    match value {
        Object::Reference(old) => Ok(Object::Reference(import_ref(
            builder, doc, *old, map, depth,
        )?)),
        Object::Array(array) => {
            let mut items = Vec::with_capacity(array.len());
            for item in array.iter() {
                items.push(import_value(builder, doc, item, map, depth + 1)?);
            }
            Ok(Object::Array(Array::from_vec(items)))
        }
        Object::Dictionary(dict) => Ok(Object::Dictionary(import_dict(
            builder, doc, dict, map, depth,
        )?)),
        Object::Stream(stream) => {
            let dict = import_dict(builder, doc, stream.dict(), map, depth)?;
            Ok(Object::Stream(Stream::new(dict, stream.raw().clone())))
        }
        scalar => Ok(scalar.clone()),
    }
}

/// Import every entry of a dictionary.
fn import_dict(
    builder: &mut Builder,
    doc: &Document,
    dict: &Dictionary,
    map: &mut Remap,
    depth: usize,
) -> Result<Dictionary> {
    let mut imported = Dictionary::new();
    for (key, value) in dict.iter() {
        imported.insert(
            key.clone(),
            import_value(builder, doc, value, map, depth + 1)?,
        );
    }
    Ok(imported)
}

/// Import the object that `old` points to (or reuse its already-assigned id), returning the new id.
fn import_ref(
    builder: &mut Builder,
    doc: &Document,
    old: ObjectId,
    map: &mut Remap,
    depth: usize,
) -> Result<ObjectId> {
    if let Some(new) = map.get(&old.number) {
        return Ok(*new);
    }
    // Reserve and record the mapping *before* recursing so reference cycles terminate.
    let new = builder.reserve();
    map.insert(old.number, new);
    let source = doc.get(old)?;
    let imported = import_value(builder, doc, &source, map, depth + 1)?;
    builder.put(new, imported);
    Ok(new)
}

/// Combine the pages of several documents into one, in order (§7.7.3). Each source's object graph
/// is imported with independent reference remapping, so object-number collisions cannot occur.
pub fn merge(docs: &[&Document]) -> Result<Vec<u8>> {
    let mut builder = Builder::new();
    let catalog_id = builder.reserve();
    let pages_id = builder.reserve();

    let mut kids = Vec::new();
    for doc in docs {
        let mut map = Remap::new();
        for (_, page) in doc.page_entries()? {
            let page_id = import_page(&mut builder, doc, &page, &mut map, pages_id)?;
            kids.push(Object::Reference(page_id));
        }
    }
    Ok(builder.finish(kids, catalog_id, pages_id))
}

/// Merge with an explicit report: the fresh graph omits source signatures and structure trees
/// because their object references cannot remain valid (§12.8, §14.7).
pub fn merge_with_report(docs: &[&Document]) -> Result<TransformReport> {
    Ok(TransformReport::new(
        merge(docs)?,
        RewriteMode::Reconstructed,
        SignatureEffect::Removed,
        StructureEffect::Removed,
    ))
}

impl Document {
    /// Produce a new PDF containing only the pages at `indices` (split / page subset), in the
    /// given order. Out-of-range indices are skipped.
    pub fn extract_pages(&self, indices: &[usize]) -> Result<Vec<u8>> {
        let entries = self.page_entries()?;
        let mut builder = Builder::new();
        let catalog_id = builder.reserve();
        let pages_id = builder.reserve();

        let mut map = Remap::new();
        let mut kids = Vec::new();
        for &i in indices {
            if let Some((_, page)) = entries.get(i) {
                let page_id = import_page(&mut builder, self, page, &mut map, pages_id)?;
                kids.push(Object::Reference(page_id));
            }
        }
        Ok(builder.finish(kids, catalog_id, pages_id))
    }

    /// Page reconstruction with explicit signature and logical-structure effects (§12.8, §14.7).
    pub fn extract_pages_with_report(&self, indices: &[usize]) -> Result<TransformReport> {
        Ok(TransformReport::new(
            self.extract_pages(indices)?,
            RewriteMode::Reconstructed,
            SignatureEffect::Removed,
            StructureEffect::Removed,
        ))
    }

    /// Produce a new PDF with the page at `index` rotated by `degrees` (normalised to `0..360`),
    /// via a full rewrite that sets the page's `/Rotate` (§7.7.3.3). Other pages are unchanged.
    pub fn rotate_page(&self, index: usize, degrees: i64) -> Result<Vec<u8>> {
        let entries = self.page_entries()?;
        let target = entries
            .get(index)
            .and_then(|(id, _)| *id)
            .ok_or(DocError::BadPageTree)?;
        let rotation = degrees.rem_euclid(360);

        let mut objects = self.collect_objects()?;
        for (id, object) in &mut objects {
            if id.number == target.number
                && let Object::Dictionary(dict) = object
            {
                dict.insert(Name::from("Rotate"), Object::Integer(rotation));
            }
        }

        let root = self.xref.root().ok_or(DocError::MissingCatalog)?;
        let info = self
            .xref
            .trailer
            .get(&Name::from("Info"))
            .and_then(Object::as_reference);
        let version = self.version().map_or((1, 7), |v| (v.major, v.minor));
        Ok(write_document(
            &objects,
            root,
            info,
            version,
            self.preserved_file_id().as_deref(),
        ))
    }

    /// Rotation with an explicit full-rewrite preservation report.
    pub fn rotate_page_with_report(&self, index: usize, degrees: i64) -> Result<TransformReport> {
        Ok(TransformReport::new(
            self.rotate_page(index, degrees)?,
            RewriteMode::FullRewrite,
            SignatureEffect::Invalidated,
            StructureEffect::Preserved,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Assemble a classic PDF whose pages (objects 3..) each carry a `/Contents` stream showing a
    /// unique label, so merged/split output can be checked by extracting text.
    fn labelled_pdf(labels: &[&str]) -> Vec<u8> {
        let mut objects: Vec<Vec<u8>> = vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            Vec::new(), // placeholder for the /Pages node, filled below
        ];
        let mut kids = String::new();
        for (i, label) in labels.iter().enumerate() {
            let page_obj = 3 + i * 2;
            let content_obj = page_obj + 1;
            kids.push_str(&format!("{page_obj} 0 R "));
            objects.push(
                format!(
                    "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents {content_obj} 0 R >>"
                )
                .into_bytes(),
            );
            let content = format!("BT /F1 12 Tf ({label}) Tj ET");
            objects.push(
                format!(
                    "<< /Length {} >>\nstream\n{content}\nendstream",
                    content.len()
                )
                .into_bytes(),
            );
        }
        objects[1] = format!(
            "<< /Type /Pages /Kids [{}] /Count {} >>",
            kids.trim(),
            labels.len()
        )
        .into_bytes();

        let mut buf = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, body) in objects.iter().enumerate() {
            offsets.push(buf.len());
            buf.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
            buf.extend_from_slice(body);
            buf.extend_from_slice(b"\nendobj\n");
        }
        let startxref = buf.len();
        buf.extend_from_slice(
            format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
        );
        for off in &offsets {
            buf.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
        }
        buf.extend_from_slice(
            format!("trailer\n<< /Size {} /Root 1 0 R >>\n", objects.len() + 1).as_bytes(),
        );
        buf.extend_from_slice(format!("startxref\n{startxref}\n%%EOF\n").as_bytes());
        buf
    }

    fn text(bytes: Vec<u8>) -> Vec<String> {
        let doc = Document::open(bytes).unwrap();
        doc.pages()
            .unwrap()
            .iter()
            .enumerate()
            .map(|(i, _)| {
                let content = doc.page_content_bytes(&doc.pages().unwrap()[i]).unwrap();
                String::from_utf8_lossy(&content).into_owned()
            })
            .collect()
    }

    #[test]
    fn merge_concatenates_pages() {
        let a = Document::open(labelled_pdf(&["A1", "A2"])).unwrap();
        let b = Document::open(labelled_pdf(&["B1"])).unwrap();
        let merged = merge(&[&a, &b]).unwrap();

        let doc = Document::open(merged).unwrap();
        assert_eq!(doc.page_count().unwrap(), 3);
        // Each page's content survived with its label intact.
        let labels: Vec<String> = (0..3)
            .map(|i| {
                let page = doc.pages().unwrap().remove(i);
                String::from_utf8_lossy(&doc.page_content_bytes(&page).unwrap()).into_owned()
            })
            .collect();
        assert!(labels[0].contains("A1"));
        assert!(labels[1].contains("A2"));
        assert!(labels[2].contains("B1"));
    }

    #[test]
    fn extract_pages_subsets_and_reorders() {
        let doc = Document::open(labelled_pdf(&["P0", "P1", "P2"])).unwrap();
        // Take pages 2 and 0, in that order.
        let out = doc.extract_pages(&[2, 0]).unwrap();
        let result = Document::open(out).unwrap();
        assert_eq!(result.page_count().unwrap(), 2);

        let first = result.pages().unwrap().remove(0);
        let second = result.pages().unwrap().remove(1);
        assert!(
            String::from_utf8_lossy(&result.page_content_bytes(&first).unwrap()).contains("P2")
        );
        assert!(
            String::from_utf8_lossy(&result.page_content_bytes(&second).unwrap()).contains("P0")
        );
    }

    #[test]
    fn extract_skips_out_of_range_indices() {
        let doc = Document::open(labelled_pdf(&["only"])).unwrap();
        let out = doc.extract_pages(&[5, 0, 9]).unwrap();
        assert_eq!(Document::open(out).unwrap().page_count().unwrap(), 1);
    }

    #[test]
    fn transform_reports_make_rewrite_and_preservation_effects_explicit() {
        let doc = Document::open(labelled_pdf(&["one", "two"])).unwrap();
        let saved = doc.save_with_report().unwrap();
        assert_eq!(saved.rewrite_mode(), RewriteMode::FullRewrite);
        assert_eq!(saved.signature_effect(), SignatureEffect::Invalidated);
        assert_eq!(saved.structure_effect(), StructureEffect::Preserved);
        assert_eq!(
            Document::open(saved.bytes().to_vec())
                .unwrap()
                .page_count()
                .unwrap(),
            2
        );

        let rotated = doc.rotate_page_with_report(0, 90).unwrap();
        assert_eq!(rotated.rewrite_mode(), RewriteMode::FullRewrite);
        assert_eq!(rotated.signature_effect(), SignatureEffect::Invalidated);
        assert_eq!(rotated.structure_effect(), StructureEffect::Preserved);

        let extracted = doc.extract_pages_with_report(&[1]).unwrap();
        assert_eq!(extracted.rewrite_mode(), RewriteMode::Reconstructed);
        assert_eq!(extracted.signature_effect(), SignatureEffect::Removed);
        assert_eq!(extracted.structure_effect(), StructureEffect::Removed);
        assert_eq!(
            Document::open(extracted.into_bytes())
                .unwrap()
                .page_count()
                .unwrap(),
            1
        );

        for report in [
            doc.save_as_with_report(1, 7).unwrap(),
            doc.save_compact_with_report().unwrap(),
            doc.save_packed_with_report().unwrap(),
        ] {
            assert_eq!(report.rewrite_mode(), RewriteMode::FullRewrite);
            assert_eq!(report.signature_effect(), SignatureEffect::Invalidated);
            assert_eq!(report.structure_effect(), StructureEffect::Preserved);
        }
        let flattened = doc.flatten_form_with_report().unwrap();
        assert_eq!(flattened.rewrite_mode(), RewriteMode::FullRewrite);
        assert_eq!(flattened.structure_effect(), StructureEffect::Invalidated);
        let merged = merge_with_report(&[&doc]).unwrap();
        assert_eq!(merged.rewrite_mode(), RewriteMode::Reconstructed);
        assert_eq!(merged.signature_effect(), SignatureEffect::Removed);
        assert_eq!(merged.structure_effect(), StructureEffect::Removed);
    }

    #[test]
    fn rotate_sets_page_rotation() {
        let doc = Document::open(labelled_pdf(&["x", "y"])).unwrap();
        let out = doc.rotate_page(1, 90).unwrap();

        let rotated = Document::open(out).unwrap();
        assert_eq!(rotated.page_count().unwrap(), 2);
        let page = rotated.pages().unwrap().remove(1);
        assert_eq!(page.get_integer(&Name::from("Rotate")), Some(90));
        // Negative input is normalised into 0..360.
        let back = Document::open(doc.rotate_page(0, -90).unwrap()).unwrap();
        assert_eq!(
            back.pages().unwrap()[0].get_integer(&Name::from("Rotate")),
            Some(270)
        );
    }

    #[test]
    fn rotate_out_of_range_errors() {
        let doc = Document::open(labelled_pdf(&["x"])).unwrap();
        assert_eq!(doc.rotate_page(9, 90).unwrap_err(), DocError::BadPageTree);
    }

    #[test]
    fn merged_text_round_trips() {
        let a = Document::open(labelled_pdf(&["hello"])).unwrap();
        let merged = merge(&[&a, &a]).unwrap();
        let all = text(merged);
        assert_eq!(all.len(), 2);
        assert!(all.iter().all(|t| t.contains("hello")));
    }
}
