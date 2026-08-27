//! The `prismpdf` binary itself (EPIC 15): the shell around the library — argv parsing, exit codes,
//! the stdout/stderr split, and the `PRISMPDF_PASSWORD` environment variable.
//!
//! The subcommands' *behaviour* is tested in-process in `commands.rs`; what needs a real process is
//! everything `main.rs` does with the outcome. Each test here spawns the built executable.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::process::Command;

mod support;
use support::Scratch;

/// Path to the binary under test, provided by Cargo for integration tests.
const BIN: &str = env!("CARGO_BIN_EXE_prismpdf");

#[test]
fn a_bare_path_inspects_the_file_on_stdout() {
    let scratch = Scratch::new("bin-inspect");
    let input = scratch.write("in.pdf", &support::text_pdf());

    let out = Command::new(BIN)
        .arg(&input)
        .output()
        .expect("run prismpdf");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("Version:  1.7"), "{stdout}");
    assert!(stdout.contains("Pages:    1"), "{stdout}");
    assert!(stdout.contains("Doc Title"), "{stdout}");
    assert!(out.stderr.is_empty(), "stderr should be quiet on success");
}

#[test]
fn a_subcommand_writes_its_report_to_stdout() {
    let scratch = Scratch::new("bin-text");
    let input = scratch.write("in.pdf", &support::text_pdf());

    let out = Command::new(BIN)
        .args(["text".as_ref(), input.as_os_str()])
        .output()
        .expect("run prismpdf text");
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("Hello CLI"));
}

#[test]
fn an_empty_command_line_fails_and_points_at_help() {
    let out = Command::new(BIN).output().expect("run prismpdf");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.starts_with("prismpdf: "), "{stderr}");
    assert!(stderr.contains("--help"), "{stderr}");
}

#[test]
fn a_usage_error_is_reported_by_clap_and_still_fails() {
    // Two positionals where one is allowed, and an unknown value for a checked argument.
    for argv in [vec!["a.pdf", "b.pdf"], vec!["encrypt", "a", "b", "rot13"]] {
        let out = Command::new(BIN)
            .args(&argv)
            .output()
            .expect("run prismpdf");
        assert!(!out.status.success(), "{argv:?} should fail");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains("error:"), "{argv:?}: {stderr}");
        assert!(stderr.contains("--help"), "{argv:?}: {stderr}");
    }
}

#[test]
fn a_failing_command_prefixes_its_message_and_exits_nonzero() {
    let out = Command::new(BIN)
        .args(["text", "/no/such/file.pdf"])
        .output()
        .expect("run prismpdf text");
    assert!(!out.status.success());
    assert!(out.stdout.is_empty(), "no report on failure");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.starts_with("prismpdf: cannot read"), "{stderr}");
}

#[test]
fn help_and_version_print_to_stdout_and_succeed() {
    for flag in ["--help", "-h"] {
        let out = Command::new(BIN).arg(flag).output().expect("run prismpdf");
        assert!(out.status.success(), "{flag} should succeed");
        let stdout = String::from_utf8_lossy(&out.stdout);
        // Every subcommand is listed, from the same declaration that parses them.
        for command in [
            "text",
            "save",
            "merge",
            "images",
            "fonts",
            "attachments",
            "annotations",
            "fields",
            "outline",
            "xmp",
            "fill",
            "flatten",
            "sign",
            "verify",
            "subset-font",
            "subset",
            "encrypt",
        ] {
            assert!(stdout.contains(command), "{flag}: {command} missing");
        }
    }

    let out = Command::new(BIN)
        .arg("--version")
        .output()
        .expect("run prismpdf");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains(env!("CARGO_PKG_VERSION")), "{stdout}");
}

#[test]
fn a_subcommands_own_help_documents_its_arguments() {
    let out = Command::new(BIN)
        .args(["help", "save"])
        .output()
        .expect("run prismpdf help save");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("compact"), "{stdout}");
    assert!(stdout.contains("packed"), "{stdout}");
}

#[test]
fn an_encrypted_file_needs_prismpdf_password() {
    let scratch = Scratch::new("bin-encrypted");
    let doc = prismpdf::Document::open(support::text_pdf()).expect("open");
    let encrypted = doc
        .save_encrypted(b"open-sesame", b"owner", prismpdf::Algorithm::Aes128)
        .expect("encrypt");
    let input = scratch.write("locked.pdf", &encrypted);

    // Without the password the file cannot be opened, and the message says what to do.
    let out = Command::new(BIN)
        .arg(&input)
        .output()
        .expect("run prismpdf");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("PRISMPDF_PASSWORD"), "{stderr}");

    // With it, the document opens like any other.
    let out = Command::new(BIN)
        .env("PRISMPDF_PASSWORD", "open-sesame")
        .arg(&input)
        .output()
        .expect("run prismpdf");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("Pages:    1"));
}

#[test]
fn a_command_that_writes_files_works_end_to_end_through_the_binary() {
    // One full path through the real process: read a PDF, write two, read one back.
    let scratch = Scratch::new("bin-merge");
    let a = scratch.write("a.pdf", &support::text_pdf());
    let b = scratch.write("b.pdf", &support::text_pdf());
    let merged = scratch.path("merged.pdf");

    let status = Command::new(BIN)
        .args([
            "merge".as_ref(),
            merged.as_os_str(),
            a.as_os_str(),
            b.as_os_str(),
        ])
        .status()
        .expect("run prismpdf merge");
    assert!(status.success());

    let out = Command::new(BIN)
        .arg(&merged)
        .output()
        .expect("inspect the merge");
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("Pages:    2"));
}
