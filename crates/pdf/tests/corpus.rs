//! Corpus-driven round-trip tests (DESIGN.md §7, `corpus/README.md`).
//!
//! These walk the committed `corpus/` tree, so dropping a new `.pdf` into `valid/`, `edge/` or
//! `malformed/` automatically brings it under test. The fixtures are produced by
//! `cargo run -p prismpdf --example gen_corpus`.
//!
//! - `valid/` + `edge/`: must parse strictly, and `load → save → load` (both `save` and the
//!   compact xref-stream `save_compact`) must preserve page count and extracted text.
//! - `malformed/`: recovery must open them and report a page count without panicking.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::path::{Path, PathBuf};

use prismpdf::{Document, document_text};

fn corpus_dir() -> PathBuf {
    // CARGO_MANIFEST_DIR is crates/pdf; the corpus lives at the workspace root.
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus")
}

/// All `*.pdf` files in a corpus subdirectory, sorted for deterministic ordering.
fn pdfs_in(sub: &str) -> Vec<PathBuf> {
    let dir = corpus_dir().join(sub);
    let mut paths: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "pdf"))
        .collect();
    paths.sort();
    paths
}

fn name(path: &Path) -> String {
    path.file_name().unwrap().to_string_lossy().into_owned()
}

/// `load → save → load` preserves page count and extracted text. Runs for both serializers.
fn assert_round_trips(path: &Path) {
    let bytes = std::fs::read(path).unwrap();
    let file = name(path);

    let doc = Document::open(bytes).unwrap_or_else(|e| panic!("{file}: open failed: {e:?}"));
    let pages = doc
        .page_count()
        .unwrap_or_else(|e| panic!("{file}: page_count failed: {e:?}"));
    assert!(pages >= 1, "{file}: expected at least one page");
    let text = document_text(&doc).unwrap_or_else(|e| panic!("{file}: text failed: {e:?}"));

    for (label, saved) in [("save", doc.save()), ("save_compact", doc.save_compact())] {
        let saved = saved.unwrap_or_else(|e| panic!("{file}: {label} failed: {e:?}"));
        let again =
            Document::open(saved).unwrap_or_else(|e| panic!("{file}: reopen after {label}: {e:?}"));
        assert_eq!(
            again.page_count().unwrap(),
            pages,
            "{file}: page count changed after {label}"
        );
        assert_eq!(
            document_text(&again).unwrap(),
            text,
            "{file}: text changed after {label}"
        );
        // Round-trip fidelity (M11): every live object survives with its value intact — including
        // objects the DOM never models (annotations, outlines, …) — up to the benign indirect
        // /Length normalisation.
        common::assert_objects_preserved(&doc, &again, &format!("{file} after {label}"));
    }
}

#[test]
fn valid_corpus_round_trips() {
    let files = pdfs_in("valid");
    assert!(!files.is_empty(), "corpus/valid is empty");
    for path in files {
        assert_round_trips(&path);
    }
}

#[test]
fn edge_corpus_round_trips() {
    let files = pdfs_in("edge");
    assert!(!files.is_empty(), "corpus/edge is empty");
    for path in files {
        assert_round_trips(&path);
    }
}

#[test]
fn malformed_corpus_recovers_without_panicking() {
    let files = pdfs_in("malformed");
    assert!(!files.is_empty(), "corpus/malformed is empty");
    for path in files {
        let file = name(&path);
        let bytes = std::fs::read(&path).unwrap();
        // Recovery is first-class: a broken file must still open by rebuilding the xref / scanning.
        let doc = Document::open(bytes)
            .unwrap_or_else(|e| panic!("{file}: recovery failed to open: {e:?}"));
        let pages = doc
            .page_count()
            .unwrap_or_else(|e| panic!("{file}: page_count failed: {e:?}"));
        assert!(pages >= 1, "{file}: recovered document has no pages");
        // Extraction must not panic on a recovered document (result may be empty — that's fine).
        let _ = document_text(&doc);
    }
}

/// Spot-check that specific fixtures decode to their known text, not just *some* stable string.
#[test]
fn known_text_extracts() {
    let cases = [
        ("valid/text-classic-xref.pdf", "Hello classic xref"),
        ("valid/flate-content.pdf", "Compressed hello"),
        ("valid/xref-stream.pdf", "Xref stream text"),
        ("valid/objstm.pdf", "Object stream text"),
        ("edge/length-indirect.pdf", "Indirect length"),
        ("edge/nested-page-tree.pdf", "Nested A"),
    ];
    for (rel, expected) in cases {
        let doc = Document::open(std::fs::read(corpus_dir().join(rel)).unwrap())
            .unwrap_or_else(|e| panic!("{rel}: open failed: {e:?}"));
        let text = document_text(&doc).unwrap();
        assert!(
            text.contains(expected),
            "{rel}: expected text {expected:?}, got {text:?}"
        );
    }
}
