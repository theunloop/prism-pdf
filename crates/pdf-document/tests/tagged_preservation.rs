//! Tagged-PDF structure preservation across editing (ISO 32000-1 §14.7, Milestone M14.4).
//!
//! A full rewrite (`save`) and an in-place edit (`rotate_page`) re-emit the whole object graph, so
//! the logical structure tree survives losslessly. A page subset (`extract_pages`) or a `merge`
//! cannot faithfully carry the structure (elements would dangle or renumber wrongly), so they drop
//! it cleanly — the output is valid but **untagged**, with no `/StructTreeRoot`, no `/MarkInfo`, and
//! no dangling page `/StructParents`.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use pdf_cos::{Name, Object};
use pdf_document::{Builder, Document, PageSpec, StructElem, merge};

/// A tagged document with `pages` pages, each carrying one `P` paragraph (MCID 0).
fn tagged_doc(pages: usize) -> Vec<u8> {
    let mut builder = Builder::new();
    let mut structure = Vec::new();
    for i in 0..pages {
        builder.add_page(PageSpec::new(b"/P <</MCID 0>> BDC\nBT ET\nEMC\n".to_vec()));
        let mut p = StructElem::new("P");
        p.push_content(i, 0);
        structure.push(p);
    }
    builder.lang("en-US");
    builder.structure(structure);
    builder.build()
}

fn has_struct_tree(doc: &Document) -> bool {
    doc.catalog()
        .unwrap()
        .get(&Name::from("StructTreeRoot"))
        .is_some()
}

#[test]
fn full_rewrite_preserves_the_structure_tree() {
    let doc = Document::open(tagged_doc(1)).unwrap();
    assert!(has_struct_tree(&doc));
    let resaved = Document::open(doc.save().unwrap()).unwrap();
    assert!(
        has_struct_tree(&resaved),
        "save() must keep the structure tree"
    );
    assert!(
        matches!(
            resaved.catalog().unwrap().get(&Name::from("MarkInfo")),
            Some(Object::Dictionary(_))
        ),
        "save() must keep /MarkInfo"
    );
}

#[test]
fn rotate_page_preserves_the_structure_tree() {
    let doc = Document::open(tagged_doc(1)).unwrap();
    let rotated = Document::open(doc.rotate_page(0, 90).unwrap()).unwrap();
    assert!(
        has_struct_tree(&rotated),
        "rotate_page must keep the structure tree"
    );
    assert_eq!(
        rotated.pages().unwrap()[0].get(&Name::from("Rotate")),
        Some(&Object::Integer(90))
    );
    // The page is still in the parent tree (its /StructParents is intact).
    assert!(
        rotated.pages().unwrap()[0]
            .get(&Name::from("StructParents"))
            .is_some()
    );
}

#[test]
fn extract_pages_drops_structure_cleanly() {
    let doc = Document::open(tagged_doc(2)).unwrap();
    let extracted = Document::open(doc.extract_pages(&[1]).unwrap()).unwrap();
    assert_eq!(extracted.page_count().unwrap(), 1);

    // No structure tree, no /MarkInfo, and crucially no dangling /StructParents on the page.
    let catalog = extracted.catalog().unwrap();
    assert!(!has_struct_tree(&extracted), "structure must be dropped");
    assert!(catalog.get(&Name::from("MarkInfo")).is_none());
    assert!(
        extracted.pages().unwrap()[0]
            .get(&Name::from("StructParents"))
            .is_none(),
        "no dangling parent-tree key"
    );
}

#[test]
fn merge_drops_structure_cleanly() {
    let a = Document::open(tagged_doc(1)).unwrap();
    let b = Document::open(tagged_doc(1)).unwrap();
    let merged = Document::open(merge(&[&a, &b]).unwrap()).unwrap();
    assert_eq!(merged.page_count().unwrap(), 2);

    let catalog = merged.catalog().unwrap();
    assert!(
        !has_struct_tree(&merged),
        "structure must be dropped on merge"
    );
    assert!(catalog.get(&Name::from("MarkInfo")).is_none());
    for page in merged.pages().unwrap() {
        assert!(
            page.get(&Name::from("StructParents")).is_none(),
            "no dangling parent-tree key"
        );
    }
}
