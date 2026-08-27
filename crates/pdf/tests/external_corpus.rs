//! Round-trip tests against real third-party PDFs — the PDF Association's `pdf20examples` set
//! (CC BY-SA 4.0). These files are NOT committed (see `.gitignore`: `corpus/external/`); fetch
//! them with:
//!
//! ```text
//! git clone --depth 1 https://github.com/pdf-association/pdf20examples.git \
//!     corpus/external/pdf20examples
//! ```
//!
//! When the directory is absent (e.g. CI without the clone) every test here **skips** rather than
//! fails, printing a note (`cargo test -- --nocapture`). When present, they prove Prism PDF opens
//! genuine PDF 2.0 producer output and survives `load → save → load` with a stable page count.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::path::{Path, PathBuf};

use prismpdf::{Document, document_text};

/// The external corpus directory, or `None` if it hasn't been fetched.
fn external_dir() -> Option<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/external/pdf20examples");
    dir.is_dir().then_some(dir)
}

fn pdfs(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x == "pdf") {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

#[test]
fn pdf20examples_round_trip() {
    let Some(dir) = external_dir() else {
        eprintln!("skip: corpus/external/pdf20examples not present (see test docs to fetch)");
        return;
    };
    let files = pdfs(&dir);
    assert!(!files.is_empty(), "external corpus directory has no PDFs");

    for path in &files {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let bytes = std::fs::read(path).unwrap();

        // Real producer output must open (recovery is first-class) and report a page count.
        let doc = Document::open(bytes).unwrap_or_else(|e| panic!("{name}: open failed: {e:?}"));
        let pages = doc
            .page_count()
            .unwrap_or_else(|e| panic!("{name}: page_count failed: {e:?}"));
        assert!(pages >= 1, "{name}: no pages");

        // Extraction must not error (text in the content stream and in annotations, §12.5.2).
        let _ = document_text(&doc);

        // load → save → load preserves the page count and the full live-object set (M11): genuine
        // producer output carries objects the DOM never models (annotations, metadata, outlines,
        // structure tree) — a full rewrite must drop none of them.
        for (label, saved) in [("save", doc.save()), ("save_compact", doc.save_compact())] {
            let saved = saved.unwrap_or_else(|e| panic!("{name}: {label} failed: {e:?}"));
            let re = Document::open(saved)
                .unwrap_or_else(|e| panic!("{name}: reopen after {label}: {e:?}"));
            assert_eq!(
                re.page_count().unwrap(),
                pages,
                "{name}: page count changed after {label}"
            );
            common::assert_objects_preserved(&doc, &re, &format!("{name} after {label}"));
        }
    }
}

#[test]
fn pdf20examples_known_text_extracts() {
    let Some(dir) = external_dir() else {
        eprintln!("skip: corpus/external/pdf20examples not present (see test docs to fetch)");
        return;
    };
    // Files whose page-content text Prism PDF is expected to recover verbatim.
    let cases = [
        ("Simple PDF 2.0 file.pdf", "Hello World"),
        (
            "PDF 2.0 via incremental save.pdf",
            "PDF 2.0 Words Have Spacing",
        ),
        (
            "PDF 2.0 with offset start.pdf",
            "This is a PDF 2.0 document",
        ),
    ];
    for (file, expected) in cases {
        let path = dir.join(file);
        if !path.exists() {
            eprintln!("skip: {file} not in corpus");
            continue;
        }
        let doc = Document::open(std::fs::read(&path).unwrap())
            .unwrap_or_else(|e| panic!("{file}: open failed: {e:?}"));
        let text = document_text(&doc).unwrap();
        assert!(
            text.contains(expected),
            "{file}: expected {expected:?}, got {text:?}"
        );
    }
}
