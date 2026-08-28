//! PDF/UA-1 production through the facade (EPIC 14, ISO 14289-1).
//!
//! Covers the facade `make_pdfua` wrapper: that it finalises a tagged document and that an
//! unmet requirement surfaces as the unified [`prismpdf::Error`] (the `PdfUa` variant), not the raw
//! `PdfUaError`. The exhaustive per-requirement matrix lives in `pdf-standards`; this checks the
//! facade seam.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use prismpdf::cos::{Name, Object};
use prismpdf::{
    Align, Builder, Document, Flow, PageSpec, PageStyle, PdfUaError, StdFont, StructElem,
    TextBlock, XmpMetadata, make_pdfua,
};

/// A minimal tagged single-page document: one `/P` marked-content sequence with a matching
/// structure element (mirrors `pdf-standards`' own `tagged_builder`).
fn tagged_builder() -> Builder {
    let mut builder = Builder::new();
    builder.add_page(PageSpec::new(b"/P <</MCID 0>> BDC\nEMC\n".to_vec()));
    let mut p = StructElem::new("P");
    p.push_content(0, 0);
    builder.structure(vec![p]);
    builder
}

fn xmp_with_title() -> XmpMetadata {
    XmpMetadata {
        title: Some("Accessible document".to_string()),
        ..Default::default()
    }
}

#[test]
fn make_pdfua_through_the_facade() {
    let mut builder = tagged_builder();
    make_pdfua(&mut builder, &xmp_with_title(), "en-US").expect("document is PDF/UA-ready");

    let doc = Document::open(builder.build()).unwrap();
    let catalog = doc.catalog().unwrap();
    // The accessibility passes ran: document language and the tag tree are present.
    assert_eq!(
        catalog.get(&Name::from("Lang")),
        Some(&Object::String(prismpdf::cos::PdfString::from(
            b"en-US".to_vec()
        )))
    );
    assert!(catalog.get(&Name::from("StructTreeRoot")).is_some());
}

#[test]
fn make_pdfua_untagged_surfaces_unified_error() {
    let mut builder = Builder::new();
    builder.add_page(PageSpec::new(Vec::new()));
    // An untagged document fails PDF/UA; the facade reports it as the unified `prismpdf::Error`,
    // with the precise cause still matchable.
    let err = make_pdfua(&mut builder, &xmp_with_title(), "en-US");
    assert!(matches!(
        err,
        Err(prismpdf::Error::PdfUa(PdfUaError::NotTagged))
    ));
}

#[test]
fn flow_declaring_a_font_name_up_front_is_still_pdfua_conformant() {
    // The journey a binding writes first: name "F1" in `Flow::new`, then embed a real program
    // under the same name. The embed replaces the Standard-14 registration, so the document has
    // no unembedded font left and `make_pdfua` accepts it. Before that replacement the only
    // conformant spelling was to embed under a name the constructor never mentioned.
    let Ok(font) = std::fs::read("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf") else {
        return; // hermetic when no system font is present
    };
    let mut flow = Flow::new(PageStyle::default(), &[("F1", StdFont::Helvetica)]);
    flow.tagged("en-GB");
    assert!(flow.embed_font("F1", &font));
    flow.text(
        &TextBlock {
            font_resource: "F1",
            base_font: "",
            size: 14.0,
            leading: 18.0,
            align: Align::Left,
        },
        "An accessible paragraph.",
    );

    let mut builder = flow.into_builder();
    make_pdfua(&mut builder, &xmp_with_title(), "en-GB").expect("document is PDF/UA-ready");
    assert!(Document::open(builder.build()).is_ok());
}
