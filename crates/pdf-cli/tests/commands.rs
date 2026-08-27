//! The `prismpdf` subcommands, driven in-process (EPIC 15).
//!
//! Each test parses a real command line and runs it through [`Cli::run`] with a `Vec<u8>` sink, so
//! it exercises exactly what the binary does — argument parsing included — while still being able
//! to assert on the report and on the files written.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;

use clap::Parser;
use pdf_cli::Cli;

mod support;
use support::{Scratch, TEST_CERT, TEST_KEY};

/// Parse `argv` (without the program name) and run it, returning everything written to the report.
fn run(argv: &[&str]) -> Result<String, String> {
    let cli = Cli::try_parse_from(["prismpdf"].iter().chain(argv).copied())
        .map_err(|e| format!("usage error: {e}"))?;
    let mut out = Vec::new();
    cli.run(&mut out)?;
    Ok(String::from_utf8(out).expect("the report is UTF-8"))
}

/// A path as a `&str` argument.
fn arg(path: &Path) -> &str {
    path.to_str().expect("scratch paths are UTF-8")
}

// --- inspect (the bare-path form) -------------------------------------------------------------

#[test]
fn inspect_reports_version_pages_and_metadata() {
    let scratch = Scratch::new("inspect");
    let input = scratch.write("in.pdf", &support::text_pdf());

    let report = run(&[arg(&input)]).expect("inspect");
    assert!(report.contains("Version:  1.7"), "{report}");
    assert!(report.contains("Pages:    1"), "{report}");
    assert!(report.contains("Title:    Doc Title"), "{report}");
    assert!(report.contains("Author:   Tester"), "{report}");
}

#[test]
fn inspect_omits_metadata_the_document_does_not_have() {
    let scratch = Scratch::new("inspect-bare");
    let input = scratch.write("in.pdf", &support::bare_pdf());

    let report = run(&[arg(&input)]).expect("inspect");
    assert!(report.contains("Pages:    1"), "{report}");
    assert!(!report.contains("Title:"), "{report}");
    assert!(!report.contains("Author:"), "{report}");
}

#[test]
fn a_missing_file_and_a_non_pdf_both_fail_cleanly() {
    let error = run(&["/no/such/file.pdf"]).expect_err("missing file");
    assert!(error.contains("cannot read"), "{error}");

    let scratch = Scratch::new("garbage");
    let input = scratch.write("in.pdf", b"this is not a pdf at all");
    let error = run(&[arg(&input)]).expect_err("not a pdf");
    assert!(error.contains("cannot open PDF"), "{error}");
}

#[test]
fn inspect_recovers_a_file_with_no_header() {
    // Recovery is first-class (DESIGN.md §3): the file still reports its pages, and simply has no
    // version line to report.
    let scratch = Scratch::new("inspect-headerless");
    let input = scratch.write("in.pdf", &support::headerless_pdf());

    let report = run(&[arg(&input)]).expect("inspect");
    assert!(!report.contains("Version:"), "{report}");
    assert!(report.contains("Pages:    1"), "{report}");
    assert!(report.contains("Title:    Doc Title"), "{report}");
}

// --- text -------------------------------------------------------------------------------------

#[test]
fn text_extracts_the_page_text() {
    let scratch = Scratch::new("text");
    let input = scratch.write("in.pdf", &support::text_pdf());

    let report = run(&["text", arg(&input)]).expect("text");
    assert!(report.contains("Hello CLI"), "{report}");
}

// --- save -------------------------------------------------------------------------------------

#[test]
fn save_writes_each_cross_reference_form() {
    let scratch = Scratch::new("save");
    let input = scratch.write("in.pdf", &support::text_pdf());

    for (mode, marker) in [
        (None, &b"\nxref"[..]),
        (Some("compact"), b"/Type /XRef"),
        (Some("packed"), b"/Type /ObjStm"),
    ] {
        let output = scratch.path(&format!("out-{}.pdf", mode.unwrap_or("classic")));
        let mut argv = vec!["save", arg(&input), arg(&output)];
        argv.extend(mode);
        let report = run(&argv).expect("save");
        assert!(report.contains("wrote"), "{report}");

        let bytes = std::fs::read(&output).expect("saved file");
        let found = bytes.windows(marker.len()).any(|w| w == marker);
        assert!(found, "{mode:?}: expected {marker:?} in the output");

        // Whatever the form, the result is a PDF the tool can read back.
        let report = run(&[arg(&output)]).expect("inspect the rewrite");
        assert!(report.contains("Pages:    1"), "{mode:?}: {report}");
    }
}

#[test]
fn save_declares_the_target_version_and_refuses_what_it_cannot_express() {
    let scratch = Scratch::new("save-as");
    let input = scratch.write("in.pdf", &support::text_pdf());

    let output = scratch.path("out-14.pdf");
    run(&["save", arg(&input), arg(&output), "1.4"]).expect("save as 1.4");
    let bytes = std::fs::read(&output).expect("saved file");
    assert!(bytes.starts_with(b"%PDF-1.4"), "header not rewritten");

    // A page carrying /Tabs (Table 30, PDF 1.5+) cannot be declared 1.4: the M17 construct gate
    // refuses the write and names what forced the higher version.
    let tabbed = scratch.write(
        "tabs.pdf",
        &support::assemble(
            &[
                b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
                b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
                b"<< /Type /Page /Parent 2 0 R /Tabs /S >>".to_vec(),
            ],
            "",
        ),
    );
    let error = run(&[
        "save",
        arg(&tabbed),
        arg(&scratch.path("out-14b.pdf")),
        "1.4",
    ])
    .expect_err("the gate refuses");
    assert!(error.contains("cannot serialize PDF"), "{error}");
    assert!(error.contains("Tabs"), "the culprit is not named: {error}");
}

// --- merge ------------------------------------------------------------------------------------

#[test]
fn merge_concatenates_pages_in_order() {
    let scratch = Scratch::new("merge");
    let one = scratch.write("one.pdf", &support::text_pdf());
    let two = scratch.write("two.pdf", &support::rich_pdf());
    let output = scratch.path("merged.pdf");

    let report = run(&["merge", arg(&output), arg(&one), arg(&two)]).expect("merge");
    assert!(report.contains("merged 2 file(s)"), "{report}");

    // One page plus two pages.
    let report = run(&[arg(&output)]).expect("inspect the merge");
    assert!(report.contains("Pages:    3"), "{report}");
}

#[test]
fn merge_names_the_input_it_could_not_read() {
    let scratch = Scratch::new("merge-missing");
    let one = scratch.write("one.pdf", &support::text_pdf());
    let error = run(&[
        "merge",
        arg(&scratch.path("out.pdf")),
        arg(&one),
        "/no/such/file.pdf",
    ])
    .expect_err("missing input");
    assert!(error.contains("/no/such/file.pdf"), "{error}");
}

// --- images -----------------------------------------------------------------------------------

#[test]
fn images_writes_an_rgb_image_as_netpbm() {
    let scratch = Scratch::new("images");
    let input = scratch.write("in.pdf", &support::image_pdf());
    let outdir = scratch.path("images");

    let report = run(&["images", arg(&input), arg(&outdir)]).expect("images");
    assert!(report.contains("extracted 1 image(s)"), "{report}");

    let ppm = std::fs::read(outdir.join("page0_img0.ppm")).expect("ppm written");
    assert!(ppm.starts_with(b"P6\n2 1\n255\n"), "wrong PPM header");
    assert!(ppm.ends_with(&[255, 0, 0, 0, 255, 0]), "wrong samples");
}

#[test]
fn images_reports_a_document_with_none() {
    let scratch = Scratch::new("images-none");
    let input = scratch.write("in.pdf", &support::bare_pdf());
    let report = run(&["images", arg(&input), arg(&scratch.path("out"))]).expect("images");
    assert!(report.contains("extracted 0 image(s)"), "{report}");
}

// --- fonts ------------------------------------------------------------------------------------

#[test]
fn fonts_lists_a_document_without_any() {
    let scratch = Scratch::new("fonts-none");
    let input = scratch.write("in.pdf", &support::bare_pdf());
    let report = run(&["fonts", arg(&input)]).expect("fonts");
    assert!(report.contains("(no fonts)"), "{report}");
}

#[test]
fn fonts_lists_fonts_that_are_unnamed_unembedded_or_unparseable() {
    let scratch = Scratch::new("fonts-odd");
    let input = scratch.write("in.pdf", &support::odd_fonts_pdf());

    let report = run(&["fonts", arg(&input)]).expect("fonts");
    // No /BaseFont and no descriptor.
    assert!(
        report.contains("(unnamed) [Type1] — not embedded"),
        "{report}"
    );
    // An embedded program whose format is recognised but whose metrics will not parse.
    assert!(
        report.contains("Broken [TrueType] — embedded TrueType, 9 bytes"),
        "{report}"
    );
    assert!(!report.contains("glyphs"), "{report}");

    // A /Title that is not a text string is not reported as one either (§7.9.2).
    let report = run(&[arg(&input)]).expect("inspect");
    assert!(!report.contains("Title:"), "{report}");
}

#[test]
fn fonts_lists_the_standard_14_as_not_embedded() {
    let scratch = Scratch::new("fonts-std14");
    let input = scratch.write("in.pdf", &support::rich_pdf());
    let report = run(&["fonts", arg(&input)]).expect("fonts");
    assert!(report.contains("Helvetica"), "{report}");
    assert!(report.contains("not embedded"), "{report}");
}

#[test]
fn fonts_lists_and_dumps_an_embedded_truetype() {
    let Some(pdf) = support::embedded_font_pdf() else {
        return; // hermetic when the host has no TrueType font to embed
    };
    let scratch = Scratch::new("fonts");
    let input = scratch.write("in.pdf", &pdf);

    let report = run(&["fonts", arg(&input)]).expect("fonts");
    assert!(report.contains("[TrueType]"), "{report}");
    assert!(report.contains("embedded TrueType"), "{report}");
    assert!(report.contains("glyphs"), "{report}");

    let outdir = scratch.path("fonts");
    let report = run(&["fonts", arg(&input), arg(&outdir)]).expect("dump fonts");
    assert!(
        report.contains("dumped 1 embedded font program(s)"),
        "{report}"
    );
    let dumped: Vec<_> = std::fs::read_dir(&outdir)
        .expect("dump dir")
        .map(|e| e.expect("entry").path())
        .collect();
    assert_eq!(dumped.len(), 1, "{dumped:?}");
    let program = std::fs::read(&dumped[0]).expect("font program");
    assert!(
        matches!(&program[0..4], b"\x00\x01\x00\x00" | b"true" | b"OTTO"),
        "not an sfnt: {:?}",
        &program[0..4]
    );
}

#[test]
fn fonts_dumps_nothing_for_a_document_without_embedded_programs() {
    let scratch = Scratch::new("fonts-dump-none");
    let input = scratch.write("in.pdf", &support::rich_pdf());
    let outdir = scratch.path("fonts");
    let report = run(&["fonts", arg(&input), arg(&outdir)]).expect("dump fonts");
    // The Standard-14 fonts the fixture uses carry no font file (§9.6.2.2).
    assert!(
        report.contains("dumped 0 embedded font program(s)"),
        "{report}"
    );
}

// --- attachments ------------------------------------------------------------------------------

#[test]
fn attachments_lists_and_extracts_embedded_files() {
    let scratch = Scratch::new("attachments");
    let input = scratch.write("in.pdf", &support::rich_pdf());

    let report = run(&["attachments", arg(&input)]).expect("attachments");
    assert!(report.contains("invoice.xml"), "{report}");
    assert!(report.contains("text/xml"), "{report}");
    assert!(report.contains("Data"), "{report}");

    let outdir = scratch.path("files");
    let report = run(&["attachments", arg(&input), arg(&outdir)]).expect("extract attachments");
    assert!(report.contains("extracted 1 attachment(s)"), "{report}");
    // `data/invoice.xml` is reduced to a basename: nothing is written outside `outdir` (§7.11.3).
    let extracted = std::fs::read(outdir.join("invoice.xml")).expect("attachment written");
    assert_eq!(extracted, b"<invoice n=\"1\"/>");
}

#[test]
fn attachments_reports_a_document_with_none() {
    let scratch = Scratch::new("attachments-none");
    let input = scratch.write("in.pdf", &support::bare_pdf());
    let report = run(&["attachments", arg(&input)]).expect("attachments");
    assert!(report.contains("no attachments"), "{report}");
}

// --- annotations ------------------------------------------------------------------------------

#[test]
fn annotations_lists_links_and_notes_per_page() {
    let scratch = Scratch::new("annotations");
    let input = scratch.write("in.pdf", &support::rich_pdf());

    let report = run(&["annotations", arg(&input)]).expect("annotations");
    assert!(
        report.contains("page 0\tLink\thttps://example.invalid/"),
        "{report}"
    );
    // A link with no URI reports its destination page instead.
    assert!(report.contains("page 0\tLink\t→ page 1"), "{report}");
    assert!(report.contains("page 1\tText\tA note to self"), "{report}");

    // A widget annotation carries neither a target nor contents, and reports a placeholder.
    let widget = scratch.write("widget.pdf", &support::forms_and_outline_pdf());
    let report = run(&["annotations", arg(&widget)]).expect("annotations");
    assert!(report.contains("page 0\tWidget\t-"), "{report}");
}

#[test]
fn annotations_reports_a_document_with_none() {
    let scratch = Scratch::new("annotations-none");
    let input = scratch.write("in.pdf", &support::bare_pdf());
    let report = run(&["annotations", arg(&input)]).expect("annotations");
    assert!(report.contains("no annotations"), "{report}");
}

// --- fields / fill / flatten ------------------------------------------------------------------

#[test]
fn fields_lists_names_types_and_values() {
    let scratch = Scratch::new("fields");
    let input = scratch.write("in.pdf", &support::rich_pdf());

    let report = run(&["fields", arg(&input)]).expect("fields");
    assert!(report.contains("agree\tBtn"), "{report}");
}

#[test]
fn fields_reports_a_document_without_a_form() {
    let scratch = Scratch::new("fields-none");
    let input = scratch.write("in.pdf", &support::bare_pdf());
    let report = run(&["fields", arg(&input)]).expect("fields");
    assert!(report.contains("no form fields"), "{report}");
}

#[test]
fn fill_sets_a_field_value_as_an_incremental_update() {
    let scratch = Scratch::new("fill");
    let input = scratch.write("in.pdf", &support::forms_and_outline_pdf());
    let output = scratch.path("filled.pdf");

    let before = run(&["fields", arg(&input)]).expect("fields");
    assert!(before.contains("subject\tTx\tbefore"), "{before}");

    let report = run(&["fill", arg(&input), arg(&output), "subject=after"]).expect("fill");
    assert!(report.contains("filled 1 field(s)"), "{report}");

    let after = run(&["fields", arg(&output)]).expect("fields");
    assert!(after.contains("subject\tTx\tafter"), "{after}");

    // An incremental update keeps the original bytes as its prefix (§7.5.6).
    let filled = std::fs::read(&output).expect("filled file");
    let original = std::fs::read(&input).expect("original");
    assert!(filled.starts_with(&original), "not an incremental update");
}

#[test]
fn fill_ignores_a_field_the_document_does_not_have() {
    let scratch = Scratch::new("fill-unknown");
    let input = scratch.write("in.pdf", &support::forms_and_outline_pdf());
    let output = scratch.path("filled.pdf");
    let report = run(&["fill", arg(&input), arg(&output), "nosuchfield=x"]).expect("fill");
    assert!(report.contains("filled 1 field(s)"), "{report}");
    let after = run(&["fields", arg(&output)]).expect("fields");
    assert!(after.contains("subject\tTx\tbefore"), "{after}");
}

#[test]
fn flatten_bakes_the_widgets_into_the_page() {
    let scratch = Scratch::new("flatten");
    let input = scratch.write("in.pdf", &support::rich_pdf());
    let output = scratch.path("flat.pdf");

    let report = run(&["flatten", arg(&input), arg(&output)]).expect("flatten");
    assert!(report.contains("flattened →"), "{report}");

    let after = run(&["fields", arg(&output)]).expect("fields");
    assert!(after.contains("no form fields"), "{after}");
}

// --- outline ----------------------------------------------------------------------------------

#[test]
fn outline_prints_the_bookmark_tree_indented_by_depth() {
    let scratch = Scratch::new("outline");
    let flat = scratch.write("flat.pdf", &support::rich_pdf());
    let report = run(&["outline", arg(&flat)]).expect("outline");
    assert!(report.contains("First page → page 0"), "{report}");
    assert!(report.contains("Second page → page 1"), "{report}");

    let nested = scratch.write("nested.pdf", &support::forms_and_outline_pdf());
    let report = run(&["outline", arg(&nested)]).expect("outline");
    assert!(report.contains("Chapter → page 0"), "{report}");
    // The child is indented one level below its parent.
    assert!(report.contains("\n  Section → page 0"), "{report:?}");
}

#[test]
fn outline_reports_a_document_without_one() {
    let scratch = Scratch::new("outline-none");
    let input = scratch.write("in.pdf", &support::bare_pdf());
    let report = run(&["outline", arg(&input)]).expect("outline");
    assert!(report.contains("no outline"), "{report}");
}

// --- xmp --------------------------------------------------------------------------------------

#[test]
fn xmp_prints_the_metadata_packet() {
    let scratch = Scratch::new("xmp");
    let input = scratch.write("in.pdf", &support::rich_pdf());
    let report = run(&["xmp", arg(&input)]).expect("xmp");
    assert!(report.contains("<x:xmpmeta"), "{report}");
    assert!(report.contains("Rich Fixture"), "{report}");
}

#[test]
fn xmp_reports_a_document_without_a_packet() {
    let scratch = Scratch::new("xmp-none");
    let input = scratch.write("in.pdf", &support::bare_pdf());
    let report = run(&["xmp", arg(&input)]).expect("xmp");
    assert!(report.contains("no XMP metadata"), "{report}");
}

// --- sign / verify ----------------------------------------------------------------------------

#[test]
fn a_signed_document_verifies_against_its_own_certificate() {
    let scratch = Scratch::new("sign");
    let input = scratch.write("in.pdf", &support::text_pdf());
    let cert = scratch.write("cert.der", TEST_CERT);
    let key = scratch.write("key.der", TEST_KEY);
    let signed = scratch.path("signed.pdf");

    let report = run(&["sign", arg(&input), arg(&signed), arg(&cert), arg(&key)]).expect("sign");
    assert!(report.contains("signed →"), "{report}");

    // Without trust anchors the signature is checked cryptographically but not for trust.
    let report = run(&["verify", arg(&signed)]).expect("verify");
    assert!(report.contains("VALID"), "{report}");
    assert!(report.contains("trust-unchecked"), "{report}");
    assert!(report.contains("Prism PDF Test Signer"), "{report}");

    // The self-signed certificate is its own trust anchor, so the chain validates against it.
    let report = run(&["verify", arg(&signed), arg(&cert)]).expect("verify with a root");
    assert!(report.contains("\ttrusted"), "{report}");

    // An anchor that is not this signer's issuer leaves the chain untrusted.
    let stranger = scratch.write("stranger.der", TEST_KEY);
    let report = run(&["verify", arg(&signed), arg(&stranger)]).expect("verify with a stranger");
    assert!(report.contains("untrusted"), "{report}");
}

#[test]
fn verify_reports_an_unsigned_document_and_an_unreadable_anchor() {
    let scratch = Scratch::new("verify");
    let input = scratch.write("in.pdf", &support::text_pdf());

    let report = run(&["verify", arg(&input)]).expect("verify");
    assert!(report.contains("no signatures"), "{report}");

    let error = run(&["verify", arg(&input), "/no/such/root.der"]).expect_err("missing anchor");
    assert!(error.contains("cannot read"), "{error}");
}

#[test]
fn sign_names_the_credential_it_could_not_read() {
    let scratch = Scratch::new("sign-missing");
    let input = scratch.write("in.pdf", &support::text_pdf());
    let error = run(&[
        "sign",
        arg(&input),
        arg(&scratch.path("out.pdf")),
        "/no/such/cert.der",
        "/no/such/key.der",
    ])
    .expect_err("missing cert");
    assert!(error.contains("cannot read /no/such/cert.der"), "{error}");
}

// --- subsetting -------------------------------------------------------------------------------

#[test]
fn subset_font_shrinks_a_system_font() {
    let Some(ttf) = support::system_font() else {
        return; // hermetic when the host has no font
    };
    let scratch = Scratch::new("subset-font");
    let input = scratch.write("font.ttf", &ttf);
    let output = scratch.path("subset.ttf");

    let report = run(&["subset-font", arg(&input), arg(&output), "Hi"]).expect("subset-font");
    assert!(report.contains("subset"), "{report}");

    let subset = std::fs::read(&output).expect("subset written");
    assert!(subset.len() < ttf.len() / 2, "not much of a subset");
    assert!(matches!(
        &subset[0..4],
        b"\x00\x01\x00\x00" | b"true" | b"OTTO"
    ));
}

#[test]
fn subset_font_rejects_something_that_is_not_a_font() {
    let scratch = Scratch::new("subset-font-bad");
    let input = scratch.write("font.ttf", b"not a font");
    let error = run(&[
        "subset-font",
        arg(&input),
        arg(&scratch.path("out.ttf")),
        "Hi",
    ])
    .expect_err("not a font");
    assert!(
        error.contains("not a valid TrueType/OpenType font"),
        "{error}"
    );
}

#[test]
fn subset_shrinks_a_documents_embedded_fonts() {
    let Some(pdf) = support::embedded_font_pdf() else {
        return; // hermetic when the host has no font
    };
    let scratch = Scratch::new("subset");
    let input = scratch.write("in.pdf", &pdf);
    let output = scratch.path("subset.pdf");

    let report = run(&["subset", arg(&input), arg(&output)]).expect("subset");
    assert!(report.contains("subset fonts:"), "{report}");
    assert!(std::fs::read(&output).is_ok(), "no output written");
    // Still readable afterwards.
    run(&[arg(&output)]).expect("inspect the subset");
}

// --- encrypt ----------------------------------------------------------------------------------

#[test]
fn encrypt_writes_a_file_that_opens_with_the_empty_password() {
    let scratch = Scratch::new("encrypt");
    let input = scratch.write("in.pdf", &support::text_pdf());

    for algorithm in [None, Some("rc4"), Some("aes128"), Some("aes256")] {
        let output = scratch.path(&format!("enc-{}.pdf", algorithm.unwrap_or("default")));
        let mut argv = vec!["encrypt", arg(&input), arg(&output)];
        argv.extend(algorithm);
        let report = run(&argv).expect("encrypt");
        assert!(report.contains("encrypted"), "{report}");

        let bytes = std::fs::read(&output).expect("encrypted file");
        let encrypted = bytes.windows(8).any(|w| w == b"/Encrypt");
        assert!(encrypted, "{algorithm:?}: no /Encrypt in the trailer");

        // The user password is empty, so the tool reads its own output back.
        let report = run(&["text", arg(&output)]).expect("read the encrypted file");
        assert!(report.contains("Hello CLI"), "{algorithm:?}: {report}");
    }
}
