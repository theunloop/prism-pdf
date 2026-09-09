//! Signing journeys every binding ports (`docs/BINDINGS.md`): a second signature over a signed
//! document, and a signature that another implementation produced (ISO 32000-1 §12.8).
//!
//! Both are the day-one questions of anyone migrating from another PDF library, and neither was
//! asked here before: the suite signed once, and only ever verified its own output.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;

use prismpdf::{Document, SignSettings};

const CERT: &[u8] = include_bytes!("../examples/test-signer/cert.der");
const KEY: &[u8] = include_bytes!("../examples/test-signer/key.der");

fn corpus(name: &str) -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpus/valid")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

#[test]
fn a_second_signature_leaves_the_first_intact() {
    let doc = Document::open(corpus("minimal-2page.pdf")).unwrap();
    let once = doc.sign(CERT, KEY).unwrap();

    let settings = SignSettings {
        name: Some("Second signer".to_string()),
        ..SignSettings::default()
    };
    let twice = Document::open(once.clone())
        .unwrap()
        .sign_with(CERT, KEY, &settings)
        .unwrap();

    // The second revision only appends (§7.5.6): every byte of the once-signed file is still in
    // place, so the first signature's /ByteRange still covers what it signed (§12.8.1).
    assert!(twice.starts_with(&once), "incremental update appends");
    let statuses = Document::open(twice).unwrap().verify_signatures().unwrap();
    assert_eq!(statuses.len(), 2);
    assert!(statuses.iter().all(|s| s.valid), "both signatures verify");
    assert!(statuses[0].covered_bytes < statuses[1].covered_bytes);
}

#[test]
fn a_signature_from_another_implementation_verifies() {
    // The engine laid out this signature revision; OpenSSL produced the detached CMS that sits in
    // its /Contents (tools/gen_external_signature.py). Verifying it exercises everything the
    // verifier assumes about a CMS it did not write: attribute set, encoding, signer lookup.
    let doc = Document::open(corpus("signed-openssl-cms.pdf")).unwrap();
    let statuses = doc.verify_signatures().unwrap();
    assert_eq!(statuses.len(), 1);
    assert!(
        statuses[0].valid,
        "OpenSSL's CMS over the /ByteRange verifies"
    );
    assert!(
        statuses[0]
            .signer
            .as_deref()
            .unwrap_or("")
            .contains("Prism PDF Test Signer"),
        "signer read from the embedded certificate"
    );
    let trusted = doc.verify_signatures_with(&[CERT.to_vec()]).unwrap();
    assert_eq!(trusted[0].trusted, Some(true));
}
