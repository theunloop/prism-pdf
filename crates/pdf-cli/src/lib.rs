#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

//! `prismpdf` — the Prism PDF command-line tool (EPIC 15): inspect, manipulate, generate and sign
//! PDFs.
//!
//! The crate is a **library** with a thin binary on top. [`Cli`] is the whole command line, declared
//! once with clap's derive API — parsing, validation and `--help` all come from that one
//! declaration — and [`Cli::run`] executes it, writing its report to a caller-supplied
//! [`Write`] sink.
//!
//! Passing the sink in is what makes the tool testable: an integration test calls [`Cli::run`] with
//! a `Vec<u8>` and asserts on the bytes, instead of spawning the binary and scraping its stdout.
//! `src/main.rs` is then only argv → [`Cli`] → [`Cli::run`] → exit code.
//!
//! Every subcommand is a plain function in [`commands`], returning `Result<(), String>` where the
//! error is the message the binary prints after `prismpdf: `.

use std::io::Write;
use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

pub mod commands;

/// The `prismpdf` command line: either a bare PDF path to inspect, or one of the subcommands.
#[derive(Debug, Parser)]
#[command(
    name = "prismpdf",
    version,
    about = "Prism PDF — inspect, manipulate, generate and sign PDFs",
    args_conflicts_with_subcommands = true
)]
pub struct Cli {
    /// A PDF to inspect: prints its version, page count and basic metadata.
    #[arg(value_name = "FILE")]
    pub path: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Option<Command>,
}

impl Cli {
    /// Execute the parsed command line, writing its report to `out`.
    ///
    /// With neither a path nor a subcommand there is nothing to do, and the error points at
    /// `--help` — the binary turns that into a message on stderr and a failing exit code.
    pub fn run(&self, out: &mut dyn Write) -> Result<(), String> {
        match (&self.command, &self.path) {
            (Some(command), _) => command.run(out),
            (None, Some(path)) => commands::inspect(path, out),
            (None, None) => Err(
                "no input: pass a PDF to inspect, or a subcommand — run `prismpdf --help` for the list"
                    .to_string(),
            ),
        }
    }
}

/// What `prismpdf` was asked to do. Each variant's doc comment is the help text clap renders.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Extract the document's text (§7.8.2, §9.4).
    Text {
        /// The PDF to read.
        input: PathBuf,
    },

    /// Rewrite a PDF: repair a broken file and rebuild its cross-reference table (§7.5).
    Save {
        /// The PDF to rewrite.
        input: PathBuf,
        /// Where to write the rewritten PDF.
        output: PathBuf,
        /// `compact` (cross-reference stream, §7.5.8), `packed` (object streams too, §7.5.7), or a
        /// target version such as `1.4` or `2.0` (§7.5.2). Default: a classic cross-reference table.
        mode: Option<SaveMode>,
    },

    /// Concatenate PDFs into one (§7.7.3).
    Merge {
        /// Where to write the merged PDF.
        output: PathBuf,
        /// The PDFs to concatenate, in order.
        #[arg(required = true, num_args = 1..)]
        inputs: Vec<PathBuf>,
    },

    /// Extract every page's images (§8.9) as JPEG/JPEG 2000, NetPBM or raw samples.
    Images {
        /// The PDF to read.
        input: PathBuf,
        /// Directory to write the images into (created if absent).
        outdir: PathBuf,
    },

    /// List the document's fonts (§9.6/§9.8/§9.9) — or dump the embedded programs.
    Fonts {
        /// The PDF to read.
        input: PathBuf,
        /// Directory to dump embedded font programs into. Omit to list them instead.
        outdir: Option<PathBuf>,
    },

    /// List the document's embedded file attachments (§7.11) — or extract them.
    Attachments {
        /// The PDF to read.
        input: PathBuf,
        /// Directory to extract the attachments into. Omit to list them instead.
        outdir: Option<PathBuf>,
    },

    /// List every page's annotations (§12.5).
    Annotations {
        /// The PDF to read.
        input: PathBuf,
    },

    /// List the interactive form fields and their values (§12.7).
    Fields {
        /// The PDF to read.
        input: PathBuf,
    },

    /// Print the document outline / bookmark tree (§12.3.3).
    Outline {
        /// The PDF to read.
        input: PathBuf,
    },

    /// Print the XMP metadata packet (§14.3.2).
    Xmp {
        /// The PDF to read.
        input: PathBuf,
    },

    /// Set form field values and write the result as an incremental update (§12.7).
    Fill {
        /// The PDF to fill.
        input: PathBuf,
        /// Where to write the filled PDF.
        output: PathBuf,
        /// Field assignments, as `name=value`.
        #[arg(required = true, num_args = 1.., value_name = "NAME=VALUE")]
        values: Vec<FieldValue>,
    },

    /// Bake form widgets into the page content, removing the fields (§12.7.4).
    Flatten {
        /// The PDF to flatten.
        input: PathBuf,
        /// Where to write the flattened PDF.
        output: PathBuf,
    },

    /// Digitally sign a PDF (§12.8).
    Sign {
        /// The PDF to sign.
        input: PathBuf,
        /// Where to write the signed PDF.
        output: PathBuf,
        /// The signer's X.509 certificate, DER-encoded.
        cert: PathBuf,
        /// The signer's private key, PKCS#8 DER-encoded.
        key: PathBuf,
    },

    /// Verify the document's digital signatures (§12.8.1).
    Verify {
        /// The PDF to verify.
        input: PathBuf,
        /// Trust anchors (DER X.509). Given any, each signer's chain is validated against them.
        roots: Vec<PathBuf>,
    },

    /// Subset an sfnt font to the glyphs a string needs (§9.9).
    SubsetFont {
        /// The TrueType/OpenType font to subset.
        font: PathBuf,
        /// Where to write the subset font.
        output: PathBuf,
        /// The text whose glyphs must survive.
        text: String,
    },

    /// Subset a PDF's embedded fonts to the glyphs it actually uses (§9.9).
    Subset {
        /// The PDF to shrink.
        input: PathBuf,
        /// Where to write the shrunk PDF.
        output: PathBuf,
    },

    /// Encrypt a PDF with the standard security handler and an empty user password (§7.6).
    Encrypt {
        /// The PDF to encrypt.
        input: PathBuf,
        /// Where to write the encrypted PDF.
        output: PathBuf,
        /// The algorithm to encrypt with. Default: AES-128.
        #[arg(value_enum)]
        algorithm: Option<Cipher>,
    },
}

impl Command {
    /// Run this subcommand, writing its report to `out`.
    pub fn run(&self, out: &mut dyn Write) -> Result<(), String> {
        match self {
            Self::Text { input } => commands::extract_text(input, out),
            Self::Save {
                input,
                output,
                mode,
            } => commands::save(input, output, mode.clone().unwrap_or_default(), out),
            Self::Merge { output, inputs } => commands::merge(output, inputs, out),
            Self::Images { input, outdir } => commands::extract_images(input, outdir, out),
            Self::Fonts { input, outdir } => match outdir {
                Some(dir) => commands::dump_fonts(input, dir, out),
                None => commands::list_fonts(input, out),
            },
            Self::Attachments { input, outdir } => match outdir {
                Some(dir) => commands::dump_attachments(input, dir, out),
                None => commands::list_attachments(input, out),
            },
            Self::Annotations { input } => commands::list_annotations(input, out),
            Self::Fields { input } => commands::list_fields(input, out),
            Self::Outline { input } => commands::list_outline(input, out),
            Self::Xmp { input } => commands::show_xmp(input, out),
            Self::Fill {
                input,
                output,
                values,
            } => commands::fill_fields(input, output, values, out),
            Self::Flatten { input, output } => commands::flatten_form(input, output, out),
            Self::Sign {
                input,
                output,
                cert,
                key,
            } => commands::sign(input, output, cert, key, out),
            Self::Verify { input, roots } => commands::verify(input, roots, out),
            Self::SubsetFont { font, output, text } => {
                commands::subset_font(font, output, text, out)
            }
            Self::Subset { input, output } => commands::subset_pdf(input, output, out),
            Self::Encrypt {
                input,
                output,
                algorithm,
            } => commands::encrypt(input, output, algorithm.unwrap_or_default().into(), out),
        }
    }
}

/// How `prismpdf save` should serialize the rewritten file.
///
/// The three forms share one positional argument because they are alternatives, not flags: a file
/// is written either as a classic table, or as a stream, or for a declared version.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum SaveMode {
    /// A classic cross-reference table (§7.5.4) — the widest-compatibility form.
    #[default]
    Classic,
    /// A cross-reference stream (§7.5.8), PDF 1.5+.
    Compact,
    /// A cross-reference stream with non-stream objects packed into object streams (§7.5.7) — the
    /// most compact form, PDF 1.5+.
    Packed,
    /// Declare exactly this `(major, minor)` version, refusing the write when the content needs a
    /// higher one (§7.5.2, the M17 construct gate).
    Version(u8, u8),
}

impl std::str::FromStr for SaveMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "compact" => Ok(Self::Compact),
            "packed" => Ok(Self::Packed),
            version => version
                .split_once('.')
                .and_then(|(major, minor)| {
                    Some(Self::Version(major.parse().ok()?, minor.parse().ok()?))
                })
                .ok_or_else(|| {
                    format!("expected `compact`, `packed` or a version like `1.4`, got {version:?}")
                }),
        }
    }
}

/// The encryption algorithm `prismpdf encrypt` should use (§7.6).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub enum Cipher {
    /// RC4-128 (§7.6.3), for readers older than PDF 1.6.
    Rc4,
    /// AES-128 (§7.6.3), PDF 1.6+ — the default.
    #[default]
    Aes128,
    /// AES-256 (§7.6.4), PDF 2.0.
    Aes256,
}

impl From<Cipher> for prismpdf::Algorithm {
    fn from(cipher: Cipher) -> Self {
        match cipher {
            Cipher::Rc4 => Self::Rc4,
            Cipher::Aes128 => Self::Aes128,
            Cipher::Aes256 => Self::Aes256,
        }
    }
}

/// One `name=value` assignment for `prismpdf fill` (§12.7.3.2: `name` is the field's fully-qualified
/// name). Parsing it here rather than inside the command means a malformed pair is a clap usage
/// error, reported with the rest of the command line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldValue {
    /// The fully-qualified field name (`/T`).
    pub name: String,
    /// The value to set (`/V`).
    pub value: String,
}

impl std::str::FromStr for FieldValue {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (name, value) = s
            .split_once('=')
            .ok_or_else(|| format!("expected name=value, got {s:?}"))?;
        if name.is_empty() {
            return Err(format!("empty field name in {s:?}"));
        }
        Ok(Self {
            name: name.to_string(),
            value: value.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn the_command_line_is_internally_consistent() {
        // clap's own audit: duplicate names, conflicting shorts, unreachable args.
        Cli::command().debug_assert();
    }

    #[test]
    fn save_mode_parses_its_three_forms() {
        use std::str::FromStr;
        assert_eq!(SaveMode::from_str("compact"), Ok(SaveMode::Compact));
        assert_eq!(SaveMode::from_str("packed"), Ok(SaveMode::Packed));
        assert_eq!(SaveMode::from_str("1.4"), Ok(SaveMode::Version(1, 4)));
        assert_eq!(SaveMode::from_str("2.0"), Ok(SaveMode::Version(2, 0)));
        assert_eq!(SaveMode::default(), SaveMode::Classic);

        for bad in ["", "1", "x.y", "1.4.2", "999.0", "-1.0"] {
            let error = SaveMode::from_str(bad).expect_err("must reject");
            assert!(error.contains("expected"), "{bad:?}: {error}");
        }
    }

    #[test]
    fn field_values_split_on_the_first_equals() {
        use std::str::FromStr;
        let parsed = FieldValue::from_str("name=a=b").expect("parses");
        assert_eq!(parsed.name, "name");
        assert_eq!(parsed.value, "a=b");
        // An empty value is legitimate — it clears the field.
        assert_eq!(FieldValue::from_str("name=").expect("parses").value, "");
        assert!(FieldValue::from_str("no-equals").is_err());
        assert!(FieldValue::from_str("=orphan").is_err());
    }

    #[test]
    fn ciphers_map_onto_the_engine_algorithms() {
        assert_eq!(prismpdf::Algorithm::from(Cipher::Rc4), prismpdf::Algorithm::Rc4);
        assert_eq!(prismpdf::Algorithm::from(Cipher::Aes128), prismpdf::Algorithm::Aes128);
        assert_eq!(prismpdf::Algorithm::from(Cipher::Aes256), prismpdf::Algorithm::Aes256);
        assert_eq!(Cipher::default(), Cipher::Aes128);
    }

    #[test]
    fn a_bare_path_inspects_and_a_subcommand_wins_over_it() {
        let cli = Cli::try_parse_from(["prismpdf", "file.pdf"]).expect("parses");
        assert_eq!(cli.path.as_deref(), Some(std::path::Path::new("file.pdf")));
        assert!(cli.command.is_none());

        let cli = Cli::try_parse_from(["prismpdf", "text", "file.pdf"]).expect("parses");
        assert!(cli.path.is_none());
        assert!(matches!(cli.command, Some(Command::Text { .. })));
    }

    #[test]
    fn an_empty_command_line_explains_itself() {
        let cli = Cli::try_parse_from(["prismpdf"]).expect("parses");
        let error = cli.run(&mut Vec::new()).expect_err("nothing to do");
        assert!(error.contains("--help"), "{error}");
    }

    #[test]
    fn usage_errors_are_rejected_at_parse_time() {
        // Too many positionals, a missing required argument, an unknown mode, a malformed pair.
        for argv in [
            vec!["prismpdf", "a.pdf", "b.pdf"],
            vec!["prismpdf", "merge", "out.pdf"],
            vec!["prismpdf", "save", "in.pdf", "out.pdf", "sideways"],
            vec!["prismpdf", "fill", "in.pdf", "out.pdf", "novalue"],
            vec!["prismpdf", "encrypt", "in.pdf", "out.pdf", "rot13"],
            vec!["prismpdf", "sign", "in.pdf", "out.pdf", "cert.der"],
        ] {
            assert!(
                Cli::try_parse_from(&argv).is_err(),
                "{argv:?} should not parse"
            );
        }
    }

    #[test]
    fn optional_trailing_arguments_stay_optional() {
        let cli = Cli::try_parse_from(["prismpdf", "fonts", "in.pdf"]).expect("parses");
        assert!(matches!(
            cli.command,
            Some(Command::Fonts { outdir: None, .. })
        ));

        let cli = Cli::try_parse_from(["prismpdf", "verify", "in.pdf"]).expect("parses");
        assert!(matches!(
            cli.command,
            Some(Command::Verify { ref roots, .. }) if roots.is_empty()
        ));
    }
}
