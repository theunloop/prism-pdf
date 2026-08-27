//! Compile-checked sketches for the four pre-1.0 facade journeys.
//!
//! The first three functions deliberately use the real `prismpdf` facade. `compose_document` uses
//! the smallest local model of the proposed surface because Phase 3 has not implemented it yet.
//! When `prismpdf::compose` exists, that model is deleted and the function is compiled against the
//! facade.

#![allow(dead_code)]

use prismpdf::{
    Algorithm, Builder, Composition, Content, Document, Limits, PageSpec, SignSettings, StdFont,
    TextStyle, document_text,
};

fn parse_and_inspect(bytes: Vec<u8>) -> prismpdf::Result<(usize, String)> {
    let document = Document::open_with_limits(bytes, Limits::default())?;
    let page_count = document.page_count()?;
    let text = document_text(&document)?;
    Ok((page_count, text))
}

fn manipulate_loss_preservingly(bytes: Vec<u8>) -> prismpdf::Result<Vec<u8>> {
    let document = Document::open(bytes)?;
    // Form filling is an incremental update (§7.5.6): unrelated original bytes remain intact.
    Ok(document.fill_form(&[("customer.name", "Acme Corp")])?)
}

fn create_encrypt_and_sign(
    certificate_der: &[u8],
    private_key_der: &[u8],
) -> prismpdf::Result<Vec<u8>> {
    let mut content = Content::new();
    content
        .set_line_width(1.0)
        .rect(72.0, 700.0, 180.0, 36.0)
        .stroke()
        .begin_text()
        .set_font("F1", 12.0)
        .text_move(84.0, 714.0)
        .show_str("Signed and encrypted")
        .end_text();

    let mut builder = Builder::new();
    builder.add_page(PageSpec::new(content.into_bytes()).standard_font("F1", StdFont::Helvetica));
    let plain = builder.build();

    let document = Document::open(plain)?;
    let encrypted = document.save_encrypted(b"reader", b"owner", Algorithm::Aes256)?;
    let encrypted_document = Document::open_with_password(encrypted, b"owner")?;
    Ok(encrypted_document.sign_with(certificate_der, private_key_der, &SignSettings::default())?)
}

fn compose_document() -> Result<prismpdf::ComposedDocument, prismpdf::ComposeError> {
    Composition::new()
        .page(prismpdf::PageStyle::a4(56.0), |page| {
            page.content().column(|column| {
                column.spacing(20.0);
                column.item().text("Invoice", TextStyle::new().size(20.0));
                column.item().text(
                    "Bill to:\nAcme Corp\n1 Main St",
                    TextStyle::new().size(10.0),
                );
            });
        })
        .build()
}

#[test]
fn journey_sketches_compile_independently() {
    let _: fn(Vec<u8>) -> prismpdf::Result<(usize, String)> = parse_and_inspect;
    let _: fn(Vec<u8>) -> prismpdf::Result<Vec<u8>> = manipulate_loss_preservingly;
    let _: fn(&[u8], &[u8]) -> prismpdf::Result<Vec<u8>> = create_encrypt_and_sign;
    let _: fn() -> Result<prismpdf::ComposedDocument, prismpdf::ComposeError> = compose_document;
}
