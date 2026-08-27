//! Tests for [`Document::min_pdf_version`] (§7.5.2) — the checker side of M18, sharing
//! [`pdf_writer::min_version`] with the producer. Covers the object-set analysis and the
//! encryption-method floor that the cipher hides from the object set.

use super::super::*;
use super::assemble;

#[test]
fn plain_document_needs_only_1_4() {
    let objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>".to_vec(),
        b"<< /Length 21 >>\nstream\nBT (hi) Tj ET\nendstream".to_vec(),
    ];
    let doc = Document::open(assemble(&objects, "")).unwrap();
    assert_eq!(doc.min_pdf_version().unwrap(), (1, 4));
}

#[test]
fn jpx_content_raises_to_1_5() {
    // The page's content stream is JPXDecode-filtered (a ≥1.5 filter). Reachable from the root,
    // so it is a live object the analysis sees.
    let objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>".to_vec(),
        b"<< /Length 1 /Filter /JPXDecode >>\nstream\nx\nendstream".to_vec(),
    ];
    let doc = Document::open(assemble(&objects, "")).unwrap();
    assert_eq!(doc.min_pdf_version().unwrap(), (1, 5));
}

#[test]
fn encryption_method_floors_the_version() {
    let objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>".to_vec(),
        b"<< /Length 21 >>\nstream\nBT (hi) Tj ET\nendstream".to_vec(),
    ];
    let doc = Document::open(assemble(&objects, "")).unwrap();

    // AES-128 (V4) → 1.6; AES-256 (V5) → 2.0. Empty user password so the harness can reopen.
    let aes128 = doc.save_encrypted(b"", b"", Algorithm::Aes128).unwrap();
    assert_eq!(
        Document::open(aes128).unwrap().min_pdf_version().unwrap(),
        (1, 6)
    );

    let aes256 = doc.save_encrypted(b"", b"", Algorithm::Aes256).unwrap();
    assert_eq!(
        Document::open(aes256).unwrap().min_pdf_version().unwrap(),
        (2, 0)
    );
}

#[test]
fn save_as_stamps_the_target_and_round_trips() {
    // M17 Phase 2 (checker side): a plain 1.4-minimum document saved at any target ≥ its
    // minimum stamps exactly that target and stays openable.
    let objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>".to_vec(),
        b"<< /Length 21 >>\nstream\nBT (hi) Tj ET\nendstream".to_vec(),
    ];
    let doc = Document::open(assemble(&objects, "")).unwrap();
    for target in [(1u8, 4u8), (1, 5), (1, 7), (2, 0)] {
        let saved = doc.save_as(target.0, target.1).unwrap();
        let header = format!("%PDF-{}.{}", target.0, target.1);
        assert!(saved.starts_with(header.as_bytes()), "expected {header}");
        assert_eq!(Document::open(saved).unwrap().page_count().unwrap(), 1);
    }
}

#[test]
fn save_as_refuses_content_above_the_target() {
    // A document carrying a PDF 2.0 construct (document parts, §14.12) cannot be saved with a
    // 1.7 target: the refusal names the construct. The 2.0 target is fine.
    let mut builder = Builder::new();
    builder
        .add_page(PageSpec::new(Vec::new()))
        .document_parts(&[DocumentPart {
            first_page: 0,
            last_page: 0,
            dpm: Vec::new(),
        }]);
    let doc = Document::open(builder.build()).unwrap();

    let err = doc.save_as(1, 7).unwrap_err();
    let DocError::TargetVersionExceeded {
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
        construct.contains("14.12") || construct.to_lowercase().contains("part"),
        "diagnostic names the construct: {construct}"
    );

    let saved = doc.save_as(2, 0).unwrap();
    assert!(saved.starts_with(b"%PDF-2.0"));
    assert_eq!(Document::open(saved).unwrap().page_count().unwrap(), 1);
}
