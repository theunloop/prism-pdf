//! Digital signature round-trip (ISO 32000-1 §12.8) through the document layer: sign a real PDF as
//! an incremental update, then verify the detached CMS over the `/ByteRange`. The throwaway RSA
//! keypair + self-signed certificate are generated here (RustCrypto), so no key material is baked in.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::str::FromStr;
use std::time::Duration;

use der::Encode;
// The document builder is aliased: `x509_cert::builder::Builder` is the trait behind the
// certificate builders' `.build()` and keeps its name.
use pdf_document::Builder as PdfBuilder;
use pdf_document::{
    DocError, Document, FormFieldSpec, ImageColorSpace, ImageXObject, PageSpec, SignSettings,
    SignatureAppearance, TsaCredentials,
};
use rsa::pkcs1v15::SigningKey;
use rsa::pkcs8::EncodePrivateKey;
use rsa::{RsaPrivateKey, RsaPublicKey};
use sha2::Sha256;
use x509_cert::builder::{Builder, CertificateBuilder, Profile};
use x509_cert::name::Name;
use x509_cert::serial_number::SerialNumber;
use x509_cert::spki::{EncodePublicKey, SubjectPublicKeyInfoOwned};
use x509_cert::time::Validity;

/// Generate an RSA-2048 keypair + self-signed cert; return (cert DER, PKCS#8 key DER).
fn self_signed(cn: &str) -> (Vec<u8>, Vec<u8>) {
    let mut rng = rand_core::OsRng;
    let key = RsaPrivateKey::new(&mut rng, 2048).expect("rsa keygen");
    let spki = SubjectPublicKeyInfoOwned::try_from(
        RsaPublicKey::from(&key)
            .to_public_key_der()
            .unwrap()
            .as_bytes(),
    )
    .unwrap();
    let signer = SigningKey::<Sha256>::new(key.clone());
    let cert = CertificateBuilder::new(
        Profile::Root,
        SerialNumber::from(1u32),
        Validity::from_now(Duration::from_secs(3600)).unwrap(),
        Name::from_str(&format!("CN={cn}")).unwrap(),
        spki,
        &signer,
    )
    .unwrap()
    .build()
    .unwrap();
    (
        cert.to_der().unwrap(),
        key.to_pkcs8_der().unwrap().as_bytes().to_vec(),
    )
}

/// A minimal one-page PDF (classic xref).
fn one_page_pdf() -> Vec<u8> {
    let objects: [&[u8]; 3] = [
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >>",
    ];
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

#[test]
fn sign_then_verify_round_trip() {
    let (cert, key) = self_signed("Prism PDF Signer");
    let doc = Document::open(one_page_pdf()).unwrap();

    let signed = doc.sign(&cert, &key).unwrap();
    // The signature is an append-only incremental update: the original file is a byte prefix.
    assert!(signed.starts_with(&one_page_pdf()));

    let reopened = Document::open(signed.clone()).unwrap();
    let signatures = reopened.verify_signatures().unwrap();
    assert_eq!(signatures.len(), 1, "exactly one signature");
    assert!(signatures[0].valid, "signature should verify");
    assert!(
        signatures[0]
            .signer
            .as_ref()
            .unwrap()
            .contains("Prism PDF Signer")
    );
    // The signed range covers almost the whole file (all but the /Contents hole).
    assert!(signatures[0].covered_bytes > one_page_pdf().len());

    // The form field is present and the document still opens with one page.
    assert_eq!(reopened.page_count().unwrap(), 1);
}

#[test]
fn records_signing_time_in_dict_and_cms() {
    let (cert, key) = self_signed("Timed Signer");
    let doc = Document::open(one_page_pdf()).unwrap();
    let settings = SignSettings {
        signing_time: Some(1_700_000_000),
        reason: Some("I approve this document".to_string()),
        ..SignSettings::default()
    };
    let signed = doc.sign_with(&cert, &key, &settings).unwrap();

    // The signature dictionary carries /M (a PDF date) and /Reason.
    assert!(find(&signed, b"/M (D:2023").is_some(), "/M date present");
    assert!(
        find(&signed, b"/Reason (I approve").is_some(),
        "/Reason present"
    );

    let reopened = Document::open(signed).unwrap();
    let signatures = reopened.verify_signatures().unwrap();
    assert_eq!(signatures.len(), 1);
    assert!(signatures[0].valid);
    // The CMS signingTime attribute agrees with /M.
    assert_eq!(signatures[0].signing_time, Some(1_700_000_000));
}

#[test]
fn visible_appearance_emits_form_xobject() {
    let (cert, key) = self_signed("Visible Signer");
    let doc = Document::open(one_page_pdf()).unwrap();
    let settings = SignSettings {
        name: Some("Alice".to_string()),
        signing_time: Some(1_700_000_000),
        appearance: Some(SignatureAppearance {
            page_index: 0,
            rect: [20.0, 20.0, 180.0, 70.0],
            text: None,
            image: None,
        }),
        ..SignSettings::default()
    };
    let signed = doc.sign_with(&cert, &key, &settings).unwrap();

    // A Form XObject appearance with a Helvetica font was emitted and bound via /AP.
    assert!(find(&signed, b"/Subtype /Form").is_some(), "form xobject");
    assert!(
        find(&signed, b"/BaseFont /Helvetica").is_some(),
        "helv font"
    );
    assert!(find(&signed, b"/AP").is_some(), "appearance dict on widget");
    assert!(
        find(&signed, b"Digitally signed by Alice").is_some(),
        "default text"
    );

    let reopened = Document::open(signed).unwrap();
    assert_eq!(reopened.page_count().unwrap(), 1);
    let signatures = reopened.verify_signatures().unwrap();
    assert_eq!(signatures.len(), 1);
    assert!(signatures[0].valid, "visible signature still verifies");
}

#[test]
fn caption_is_encoded_to_winansi() {
    // The caption is drawn with a Standard-14 simple font, so the literal string in the appearance
    // stream carries cp1252 codes and the font says so (§9.6.6.1). Handing the caller's UTF-8 bytes
    // over raw — which is what this path used to do — shows every accented character as two wrong
    // glyphs, and an Italian signature caption is exactly where that bites.
    let (cert, key) = self_signed("Accented Signer");
    let doc = Document::open(one_page_pdf()).unwrap();
    let settings = SignSettings {
        name: Some("Rossi".to_string()),
        signing_time: Some(1_700_000_000),
        appearance: Some(SignatureAppearance {
            page_index: 0,
            rect: [20.0, 20.0, 220.0, 80.0],
            text: Some("Firmato da Società Rossi".to_string()),
            image: None,
        }),
        ..SignSettings::default()
    };
    let signed = doc.sign_with(&cert, &key, &settings).unwrap();

    assert!(
        find(&signed, b"/Encoding /WinAnsiEncoding").is_some(),
        "the caption font names the encoding its codes are written in"
    );
    // U+00E0 is 0xE0 in cp1252, which `escape_literal_string` writes as the octal escape \340.
    assert!(
        find(&signed, br"Societ\340").is_some(),
        "the caption holds cp1252 codes"
    );
    // The same character as raw UTF-8 would be 0xC3 0xA0 — the bytes this path used to emit.
    assert!(
        find(&signed, br"Societ\303\240").is_none(),
        "the caption no longer holds undecoded UTF-8"
    );

    let reopened = Document::open(signed).unwrap();
    let signatures = reopened.verify_signatures().unwrap();
    assert!(signatures[0].valid, "encoded caption still verifies");
}

#[test]
fn visible_appearance_can_carry_an_image() {
    let (cert, key) = self_signed("Stamping Signer");
    let doc = Document::open(one_page_pdf()).unwrap();
    // A 2×2 RGB image with a soft mask (§11.6.5.2): the mask must travel too.
    let stamp = ImageXObject {
        width: 2,
        height: 2,
        color_space: ImageColorSpace::Rgb,
        bits_per_component: 8,
        filter: None,
        data: vec![255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255],
        smask: Some(Box::new(ImageXObject {
            width: 2,
            height: 2,
            color_space: ImageColorSpace::Gray,
            bits_per_component: 8,
            filter: None,
            data: vec![255, 128, 0, 255],
            smask: None,
            mask: None,
            image_mask: false,
        })),
        mask: None,
        image_mask: false,
    };
    let settings = SignSettings {
        name: Some("Bob".to_string()),
        appearance: Some(SignatureAppearance {
            page_index: 0,
            rect: [20.0, 20.0, 220.0, 80.0],
            text: None,
            image: Some(stamp),
        }),
        ..SignSettings::default()
    };
    let signed = doc.sign_with(&cert, &key, &settings).unwrap();

    // The appearance form references an image XObject, paints it, and the soft mask is wired.
    assert!(find(&signed, b"/Subtype /Image").is_some(), "image xobject");
    assert!(find(&signed, b"/Im0 Do").is_some(), "painted");
    assert!(find(&signed, b"/SMask").is_some(), "soft mask carried");
    assert!(
        find(&signed, b"Digitally signed by Bob").is_some(),
        "caption beside it"
    );
    let reopened = Document::open(signed).unwrap();
    let signatures = reopened.verify_signatures().unwrap();
    assert_eq!(signatures.len(), 1);
    assert!(signatures[0].valid, "still verifies");
}

/// A one-page template with two empty signature fields, `worker` and `company` (§12.7.4.5).
fn two_field_template() -> Vec<u8> {
    let mut builder = PdfBuilder::new();
    builder.add_page(PageSpec::new(Vec::new()));
    for (name, y) in [("worker", 50.0), ("company", 150.0)] {
        builder.add_form_field(
            0,
            FormFieldSpec::Signature {
                rect: [50.0, y, 250.0, y + 50.0],
                name: name.to_string(),
                tooltip: Some(format!("{name} signature")),
            },
            Vec::new(),
        );
    }
    builder.build()
}

#[test]
fn signing_into_named_fields_fills_the_template_twice() {
    let (worker_cert, worker_key) = self_signed("Worker");
    let (company_cert, company_key) = self_signed("Company");
    let template = Document::open(two_field_template()).unwrap();
    let fields = template.form_fields().unwrap();
    assert_eq!(fields.len(), 2);
    assert!(
        fields
            .iter()
            .all(|f| f.field_type == "Sig" && f.value.is_none())
    );
    assert_eq!(fields[0].rect, Some([50.0, 50.0, 250.0, 100.0]));

    // First party signs into its own field: no field is added, the widget shows the signature.
    let settings = SignSettings {
        field_name: Some("worker".to_string()),
        name: Some("Alice".to_string()),
        signing_time: Some(1_700_000_000),
        ..SignSettings::default()
    };
    let once = template
        .sign_with(&worker_cert, &worker_key, &settings)
        .unwrap();
    assert!(
        find(&once, b"Digitally signed by Alice").is_some(),
        "default caption in the widget"
    );
    let once_doc = Document::open(once).unwrap();
    assert_eq!(once_doc.form_fields().unwrap().len(), 2, "no field added");
    let statuses = once_doc.verify_signatures().unwrap();
    assert_eq!(statuses.len(), 1);
    assert!(statuses[0].valid);

    // The same field again is refused; a field that is not there is refused differently.
    assert!(matches!(
        once_doc.sign_with(&worker_cert, &worker_key, &settings),
        Err(DocError::SignatureFieldUnusable(name, "already signed")) if name == "worker"
    ));
    let nobody = SignSettings {
        field_name: Some("nobody".to_string()),
        ..SignSettings::default()
    };
    assert!(matches!(
        once_doc.sign_with(&worker_cert, &worker_key, &nobody),
        Err(DocError::SignatureFieldNotFound(name)) if name == "nobody"
    ));

    // Second party signs into the other field: both signatures stay intact (§12.8.1 — the second
    // revision appends and the first one's bytes are untouched).
    let company = SignSettings {
        field_name: Some("company".to_string()),
        name: Some("Bob".to_string()),
        ..SignSettings::default()
    };
    let twice = once_doc
        .sign_with(&company_cert, &company_key, &company)
        .unwrap();
    let twice_doc = Document::open(twice).unwrap();
    assert_eq!(twice_doc.form_fields().unwrap().len(), 2);
    let statuses = twice_doc.verify_signatures().unwrap();
    assert_eq!(statuses.len(), 2);
    assert!(statuses.iter().all(|s| s.valid), "both signatures verify");
    assert!(
        statuses[0]
            .signer
            .as_deref()
            .unwrap_or("")
            .contains("Worker")
    );
    assert!(
        statuses[1]
            .signer
            .as_deref()
            .unwrap_or("")
            .contains("Company")
    );
}

#[test]
fn signing_into_a_field_that_is_not_a_signature_field_is_refused() {
    let (cert, key) = self_signed("Signer");
    let mut builder = PdfBuilder::new();
    builder.add_page(PageSpec::new(Vec::new()));
    builder.add_form_field(
        0,
        FormFieldSpec::Checkbox {
            rect: [10.0, 10.0, 30.0, 30.0],
            name: "agree".to_string(),
            checked: false,
            tooltip: None,
        },
        Vec::new(),
    );
    let doc = Document::open(builder.build()).unwrap();
    let settings = SignSettings {
        field_name: Some("agree".to_string()),
        ..SignSettings::default()
    };
    assert!(matches!(
        doc.sign_with(&cert, &key, &settings),
        Err(DocError::SignatureFieldUnusable(_, "not a signature field"))
    ));
}

#[test]
fn trust_store_reports_trust() {
    let (cert, key) = self_signed("Self Root");
    let doc = Document::open(one_page_pdf()).unwrap();
    let signed = doc.sign(&cert, &key).unwrap();
    let reopened = Document::open(signed).unwrap();

    // Signer cert as its own anchor → trusted; no roots → not evaluated.
    let trusted = reopened
        .verify_signatures_with(std::slice::from_ref(&cert))
        .unwrap();
    assert_eq!(trusted[0].trusted, Some(true));
    assert!(trusted[0].valid);

    let none = reopened.verify_signatures().unwrap();
    assert_eq!(none[0].trusted, None);

    let (other, _) = self_signed("Stranger");
    let untrusted = reopened.verify_signatures_with(&[other]).unwrap();
    assert_eq!(untrusted[0].trusted, Some(false));
}

/// A private issuing chain — root, an intermediate it issues, a leaf the intermediate issues — as
/// (root DER, intermediate DER, leaf DER, leaf key DER). RSA-2048 throughout.
fn private_chain() -> (Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>) {
    let mut rng = rand_core::OsRng;
    let keypair = |rng: &mut rand_core::OsRng| {
        let key = RsaPrivateKey::new(rng, 2048).expect("rsa keygen");
        let spki = SubjectPublicKeyInfoOwned::try_from(
            RsaPublicKey::from(&key)
                .to_public_key_der()
                .unwrap()
                .as_bytes(),
        )
        .unwrap();
        (key, spki)
    };
    let validity = || Validity::from_now(Duration::from_secs(7200)).unwrap();

    let (root_key, root_spki) = keypair(&mut rng);
    let root_name = Name::from_str("CN=Private Root").unwrap();
    let root_signer = SigningKey::<Sha256>::new(root_key);
    let root = CertificateBuilder::new(
        Profile::Root,
        SerialNumber::from(1u32),
        validity(),
        root_name.clone(),
        root_spki,
        &root_signer,
    )
    .unwrap()
    .build()
    .unwrap();

    let (intermediate_key, intermediate_spki) = keypair(&mut rng);
    let intermediate_name = Name::from_str("CN=Private Issuing CA").unwrap();
    let intermediate = CertificateBuilder::new(
        Profile::SubCA {
            issuer: root_name,
            path_len_constraint: None,
        },
        SerialNumber::from(2u32),
        validity(),
        intermediate_name.clone(),
        intermediate_spki,
        &root_signer,
    )
    .unwrap()
    .build()
    .unwrap();

    let intermediate_signer = SigningKey::<Sha256>::new(intermediate_key);
    let (leaf_key, leaf_spki) = keypair(&mut rng);
    let leaf = CertificateBuilder::new(
        Profile::Leaf {
            issuer: intermediate_name,
            enable_key_agreement: false,
            enable_key_encipherment: false,
        },
        SerialNumber::from(3u32),
        validity(),
        Name::from_str("CN=Chained Signer").unwrap(),
        leaf_spki,
        &intermediate_signer,
    )
    .unwrap()
    .build()
    .unwrap();

    (
        root.to_der().unwrap(),
        intermediate.to_der().unwrap(),
        leaf.to_der().unwrap(),
        leaf_key.to_pkcs8_der().unwrap().as_bytes().to_vec(),
    )
}

#[test]
fn embedded_intermediates_let_a_private_chain_verify() {
    let (root, intermediate, leaf, leaf_key) = private_chain();
    let doc = Document::open(one_page_pdf()).unwrap();
    let roots = vec![root];

    // The leaf alone: intact, but a verifier holding only the root cannot reach it.
    let bare = doc.sign(&leaf, &leaf_key).unwrap();
    let status = Document::open(bare)
        .unwrap()
        .verify_signatures_with(&roots)
        .unwrap();
    assert!(status[0].valid);
    assert_eq!(
        status[0].trusted,
        Some(false),
        "no path without the intermediate"
    );

    // With the intermediate embedded (§12.8.3.3), the same root now anchors the signer.
    let settings = SignSettings {
        extra_certificates: vec![intermediate],
        ..SignSettings::default()
    };
    let chained = doc.sign_with(&leaf, &leaf_key, &settings).unwrap();
    let status = Document::open(chained)
        .unwrap()
        .verify_signatures_with(&roots)
        .unwrap();
    assert!(status[0].valid);
    assert_eq!(
        status[0].trusted,
        Some(true),
        "intermediate completes the chain"
    );

    // Something that is not a certificate is refused, not dropped.
    let junk = SignSettings {
        extra_certificates: vec![b"junk".to_vec()],
        ..SignSettings::default()
    };
    assert!(doc.sign_with(&leaf, &leaf_key, &junk).is_err());
}

#[test]
fn embedded_timestamp_round_trips() {
    let (cert, key) = self_signed("Stamped Signer");
    let (tsa_cert, tsa_key) = self_signed("Prism PDF TSA");
    let doc = Document::open(one_page_pdf()).unwrap();
    let settings = SignSettings {
        signing_time: Some(1_700_000_000),
        timestamp: Some(TsaCredentials {
            cert_der: tsa_cert,
            key_der: tsa_key,
            gen_time: 1_700_000_500,
            serial: 7,
        }),
        ..SignSettings::default()
    };
    let signed = doc.sign_with(&cert, &key, &settings).unwrap();
    let reopened = Document::open(signed).unwrap();
    let signatures = reopened.verify_signatures().unwrap();
    assert_eq!(signatures.len(), 1);
    assert!(signatures[0].valid);
    assert_eq!(signatures[0].timestamp_time, Some(1_700_000_500));
}

/// The first occurrence of `needle` in `haystack`.
fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

#[test]
fn pades_b_signature_round_trips() {
    // A PAdES-B signature: /SubFilter /ETSI.CAdES.detached + the signing-certificate-v2 signed
    // attribute. Verify confirms the attribute binds the embedded certificate.
    let (cert, key) = self_signed("PAdES Signer");
    let doc = Document::open(one_page_pdf()).unwrap();
    let settings = SignSettings {
        signing_time: Some(1_700_000_000),
        pades: true,
        ..SignSettings::default()
    };
    let signed = doc.sign_with(&cert, &key, &settings).unwrap();
    assert!(
        find(&signed, b"/SubFilter /ETSI.CAdES.detached").is_some(),
        "PAdES subfilter"
    );

    let reopened = Document::open(signed).unwrap();
    let sigs = reopened.verify_signatures().unwrap();
    assert_eq!(sigs.len(), 1);
    assert!(sigs[0].valid, "PAdES signature verifies");
    assert!(sigs[0].pades, "signing-certificate-v2 binds the cert");

    // A plain (non-PAdES) signature does not carry the binding.
    let plain = Document::open(one_page_pdf())
        .unwrap()
        .sign(&cert, &key)
        .unwrap();
    let plain_sigs = Document::open(plain).unwrap().verify_signatures().unwrap();
    assert!(!plain_sigs[0].pades);
}

#[test]
fn document_timestamp_round_trips() {
    // A document timestamp (DTS, §12.8.5): a /DocTimeStamp signature whose /Contents is an RFC 3161
    // token over the /ByteRange bytes. Verifies via the same verify_signatures path.
    let (tsa_cert, tsa_key) = self_signed("Prism PDF TSA");
    let doc = Document::open(one_page_pdf()).unwrap();

    let stamped = doc
        .timestamp(&tsa_cert, &tsa_key, Some(1_700_000_900))
        .unwrap();
    // Append-only incremental update; the /DocTimeStamp dict and ETSI subfilter are present.
    assert!(stamped.starts_with(&one_page_pdf()));
    assert!(find(&stamped, b"/Type /DocTimeStamp").is_some());
    assert!(find(&stamped, b"/SubFilter /ETSI.RFC3161").is_some());

    let reopened = Document::open(stamped).unwrap();
    let sigs = reopened.verify_signatures().unwrap();
    assert_eq!(sigs.len(), 1, "exactly one timestamp");
    assert!(sigs[0].valid, "DTS token must verify over the ByteRange");
    assert_eq!(sigs[0].timestamp_time, Some(1_700_000_900));
    assert!(sigs[0].signer.as_ref().unwrap().contains("Prism PDF TSA"));
    assert_eq!(reopened.page_count().unwrap(), 1);
}

#[test]
fn tampering_after_timestamp_invalidates() {
    let (tsa_cert, tsa_key) = self_signed("Prism PDF TSA");
    let doc = Document::open(one_page_pdf()).unwrap();
    let mut stamped = doc
        .timestamp(&tsa_cert, &tsa_key, Some(1_700_000_900))
        .unwrap();

    stamped[1] ^= 0x01; // flip a byte inside the covered region

    let reopened = Document::open(stamped).unwrap();
    let sigs = reopened.verify_signatures().unwrap();
    assert_eq!(sigs.len(), 1);
    assert!(!sigs[0].valid, "tampered document must not verify");
}

#[test]
fn tampering_after_signing_invalidates() {
    let (cert, key) = self_signed("Prism PDF Signer");
    let doc = Document::open(one_page_pdf()).unwrap();
    let mut signed = doc.sign(&cert, &key).unwrap();

    // Flip a byte inside the signed region (the original %PDF header area).
    signed[1] ^= 0x01;

    let reopened = Document::open(signed).unwrap();
    let signatures = reopened.verify_signatures().unwrap();
    assert_eq!(signatures.len(), 1);
    assert!(!signatures[0].valid, "tampered document must not verify");
}

#[test]
fn sign_with_attached_pdf_mac_round_trips() {
    use pdf_document::Algorithm;
    let (cert, key) = self_signed("MAC Signer");

    // Base: an AES-256 (V5/R6) encrypted PDF carrying a standalone MAC and a /KDFSalt (ISO/TS 32004).
    let base = Document::open(one_page_pdf())
        .unwrap()
        .save_encrypted_with_mac(b"", b"", Algorithm::Aes256)
        .unwrap();

    // Sign it, attaching a PDF MAC token to the signature (§6.5.2).
    let signed = Document::open(base)
        .unwrap()
        .sign_with_mac(&cert, &key, &SignSettings::default(), b"")
        .unwrap();
    assert!(
        signed
            .windows("/AttachedToSig".len())
            .any(|w| w == b"/AttachedToSig"),
        "trailer /AuthCode points the MAC at the signature"
    );

    // The attached MAC authenticates the signed bytes.
    let out = Document::open(signed.clone()).unwrap();
    assert_eq!(out.verify_pdf_mac(b"").unwrap(), Some(true));

    // Tamper with a covered byte (the binary-marker comment) → the MAC fails.
    let mut bad = signed;
    bad[11] ^= 0xFF;
    assert_eq!(
        Document::open(bad).unwrap().verify_pdf_mac(b"").unwrap(),
        Some(false)
    );
}

#[test]
fn sign_with_mac_requires_kdfsalt() {
    use pdf_document::{Algorithm, DocError};
    let (cert, key) = self_signed("No Salt");
    // A plainly-encrypted doc has no /KDFSalt, so an attached MAC cannot be keyed.
    let encrypted = Document::open(one_page_pdf())
        .unwrap()
        .save_encrypted(b"", b"", Algorithm::Aes256)
        .unwrap();
    let err = Document::open(encrypted)
        .unwrap()
        .sign_with_mac(&cert, &key, &SignSettings::default(), b"")
        .unwrap_err();
    assert_eq!(err, DocError::MacFailed);
}

#[test]
fn sign_with_mac_requires_encryption() {
    use pdf_document::{Algorithm, DocError};
    let (cert, key) = self_signed("Unencrypted");
    let _ = Algorithm::Aes256;
    // A plain (unencrypted) document has no file key to derive the MAC key from.
    let err = Document::open(one_page_pdf())
        .unwrap()
        .sign_with_mac(&cert, &key, &SignSettings::default(), b"")
        .unwrap_err();
    assert_eq!(err, DocError::MacRequiresV5);
}
