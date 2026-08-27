//! The `prismpdf` subcommands (EPIC 15), one function each.
//!
//! Every command takes the files it works on plus the `out` sink its report goes to, and returns
//! `Result<(), String>` — the error being the message the binary prints after `prismpdf: `. Nothing
//! here reads argv or writes to `std::io::stdout` directly, so the whole surface is callable from a
//! test.

use std::io::Write;
use std::path::Path;

use prismpdf::Document;
use prismpdf::cos::{Dictionary, Name, Object};

use crate::{FieldValue, SaveMode};

/// `writeln!` into a command's sink, mapping the I/O error into the CLI's error type.
macro_rules! outln {
    ($out:expr, $($arg:tt)*) => {
        writeln!($out, $($arg)*).map_err(write_error)?
    };
}

/// `write!` into a command's sink, mapping the I/O error into the CLI's error type.
macro_rules! outp {
    ($out:expr, $($arg:tt)*) => {
        write!($out, $($arg)*).map_err(write_error)?
    };
}

/// Describe a failure to write the report itself — a closed pipe, a full disk.
fn write_error(e: std::io::Error) -> String {
    format!("cannot write output: {e}")
}

/// Open already-read PDF `bytes`, supplying the `PRISMPDF_PASSWORD` environment variable (if set)
/// as the password for encrypted files (§7.6) — tried as both the user and the owner password.
fn open_input(bytes: Vec<u8>) -> Result<Document, String> {
    let password = std::env::var_os("PRISMPDF_PASSWORD").unwrap_or_default();
    Document::open_with_password(bytes, password.as_encoded_bytes()).map_err(|e| match e {
        prismpdf::DocError::NeedsPassword => {
            "encrypted PDF: set PRISMPDF_PASSWORD to the user or owner password".to_string()
        }
        other => format!("cannot open PDF: {other}"),
    })
}

/// Read and open a PDF, mapping I/O and parse errors to messages.
fn open(path: &Path) -> Result<Document, String> {
    let bytes = read(path)?;
    open_input(bytes)
}

/// Read a file, naming it in the error.
fn read(path: &Path) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))
}

/// Write a file, naming it in the error. For a caller-chosen output path, where overwriting is the
/// intent.
fn write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    std::fs::write(path, bytes).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// Write a file **extracted from a PDF**, refusing to write to a path that already exists.
///
/// These filenames are derived from untrusted document content — an attachment name, a font name.
/// [`safe_filename`] already strips path separators and traversal, so the write cannot leave the
/// output directory; `create_new` closes the remaining gap, which is that `fs::write` follows a
/// symlink already sitting in that directory and would write through it to wherever it points. It
/// also stops one extracted file from silently overwriting another.
fn write_extracted(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write as _;
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(mut file) => file
            .write_all(bytes)
            .map_err(|e| format!("cannot write {}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Err(format!(
            "refusing to overwrite {}: extracted names come from the document, \
             so write to an empty directory",
            path.display()
        )),
        Err(e) => Err(format!("cannot write {}: {e}", path.display())),
    }
}

/// Create a directory (and its parents), naming it in the error.
fn create_dir(path: &Path) -> Result<(), String> {
    std::fs::create_dir_all(path).map_err(|e| format!("cannot create {}: {e}", path.display()))
}

/// Open `path` and print a short summary. Errors are returned as human-readable strings.
pub fn inspect(path: &Path, out: &mut dyn Write) -> Result<(), String> {
    let doc = open(path)?;

    if let Some(v) = doc.version() {
        outln!(out, "Version:  {}.{}", v.major, v.minor);
    }
    let pages = doc
        .page_count()
        .map_err(|e| format!("cannot read page tree: {e}"))?;
    outln!(out, "Pages:    {pages}");

    if let Some(info) = doc.info().map_err(|e| format!("cannot read /Info: {e}"))? {
        if let Some(title) = text_entry(&info, "Title") {
            outln!(out, "Title:    {title}");
        }
        if let Some(author) = text_entry(&info, "Author") {
            outln!(out, "Author:   {author}");
        }
    }
    Ok(())
}

/// Read a text-string entry from the info dictionary as lossy UTF-8 (full §7.9.2 text-string
/// decoding — PDFDocEncoding / UTF-16BE — comes later).
fn text_entry(info: &Dictionary, key: &str) -> Option<String> {
    match info.get(&Name::from(key))? {
        Object::String(s) => Some(String::from_utf8_lossy(s.as_bytes()).into_owned()),
        _ => None,
    }
}

/// Open `path` and print its extracted text (§7.8.2 + §9.4).
pub fn extract_text(path: &Path, out: &mut dyn Write) -> Result<(), String> {
    let doc = open(path)?;
    let text = prismpdf::document_text(&doc).map_err(|e| format!("cannot extract text: {e}"))?;
    outln!(out, "{text}");
    Ok(())
}

/// Open `input` and write a normalized full-rewrite copy to `output` (§7.5). Repairs a broken file
/// (recovery on open) and rebuilds its cross-reference data in the form `mode` names — a classic
/// table, a cross-reference stream, object streams, or whatever a target version allows.
pub fn save(
    input: &Path,
    output: &Path,
    mode: SaveMode,
    out: &mut dyn Write,
) -> Result<(), String> {
    let doc = open(input)?;
    let bytes = match mode {
        SaveMode::Classic => doc.save(),
        SaveMode::Compact => doc.save_compact(),
        SaveMode::Packed => doc.save_packed(),
        SaveMode::Version(major, minor) => doc.save_as(major, minor),
    }
    .map_err(|e| format!("cannot serialize PDF: {e}"))?;
    write(output, &bytes)?;
    outln!(out, "wrote {} ({} bytes)", output.display(), bytes.len());
    Ok(())
}

/// Encrypt `input` with the standard security handler (empty user password) and write `output`
/// (§7.6).
pub fn encrypt(
    input: &Path,
    output: &Path,
    algorithm: prismpdf::Algorithm,
    out: &mut dyn Write,
) -> Result<(), String> {
    let doc = open(input)?;
    let bytes = doc
        .save_encrypted(b"", b"", algorithm)
        .map_err(|e| format!("cannot encrypt PDF: {e}"))?;
    write(output, &bytes)?;
    outln!(
        out,
        "encrypted {} → {} ({algorithm:?})",
        input.display(),
        output.display(),
    );
    Ok(())
}

/// Merge `inputs` (in order) into a single PDF written to `output` (§7.7.3).
pub fn merge(
    output: &Path,
    inputs: &[std::path::PathBuf],
    out: &mut dyn Write,
) -> Result<(), String> {
    let mut docs = Vec::with_capacity(inputs.len());
    for input in inputs {
        docs.push(open(input)?);
    }
    let refs: Vec<&Document> = docs.iter().collect();
    let bytes = prismpdf::merge(&refs).map_err(|e| format!("cannot merge: {e}"))?;
    write(output, &bytes)?;
    outln!(
        out,
        "merged {} file(s) → {}",
        inputs.len(),
        output.display()
    );
    Ok(())
}

/// Extract every page's images from `input` into `outdir` (§8.9). JPEG/JPEG 2000 streams are
/// written verbatim; raw 8-bit Gray/RGB images become NetPBM (`.pgm`/`.ppm`); anything else is
/// written as a `.bin` of the decoded samples.
pub fn extract_images(input: &Path, outdir: &Path, out: &mut dyn Write) -> Result<(), String> {
    let doc = open(input)?;
    create_dir(outdir)?;

    let pages = doc
        .page_count()
        .map_err(|e| format!("cannot read pages: {e}"))?;
    let mut written = 0usize;
    for page in 0..pages {
        let images =
            prismpdf::page_images(&doc, page).map_err(|e| format!("cannot read images: {e}"))?;
        for (i, image) in images.iter().enumerate() {
            let (ext, payload) = encode_image(image);
            write_extracted(&outdir.join(format!("page{page}_img{i}.{ext}")), &payload)?;
            written += 1;
        }
    }
    outln!(out, "extracted {written} image(s) to {}", outdir.display());
    Ok(())
}

/// Choose a file extension and byte payload for an extracted image.
pub(crate) fn encode_image(image: &prismpdf::ExtractedImage) -> (&'static str, Vec<u8>) {
    use prismpdf::{ColorSpace, ImageData};
    match &image.data {
        ImageData::Jpeg(bytes) => ("jpg", bytes.clone()),
        ImageData::Jpeg2000(bytes) => ("jp2", bytes.clone()),
        ImageData::Jbig2(bytes) => ("jbig2", bytes.clone()),
        ImageData::Raw(samples) => {
            let info = image.info;
            if info.bits_per_component == 8 {
                // NetPBM: a tiny, dependency-free way to write viewable raster images.
                let header = match info.color_space {
                    ColorSpace::DeviceGray => {
                        Some(("pgm", format!("P5\n{} {}\n255\n", info.width, info.height)))
                    }
                    ColorSpace::DeviceRgb => {
                        Some(("ppm", format!("P6\n{} {}\n255\n", info.width, info.height)))
                    }
                    _ => None,
                };
                if let Some((ext, head)) = header {
                    let mut out = head.into_bytes();
                    out.extend_from_slice(samples);
                    return (ext, out);
                }
            }
            ("bin", samples.clone())
        }
    }
}

/// List the document's fonts (§9.6/§9.8/§9.9): base name, subtype, and embedded-program info.
pub fn list_fonts(path: &Path, out: &mut dyn Write) -> Result<(), String> {
    let doc = open(path)?;
    let fonts = prismpdf::document_fonts(&doc).map_err(|e| format!("cannot read fonts: {e}"))?;
    if fonts.is_empty() {
        outln!(out, "(no fonts)");
    }
    for font in &fonts {
        let base = if font.base_font.is_empty() {
            "(unnamed)"
        } else {
            &font.base_font
        };
        let embedded = match &font.embedded {
            None => "not embedded".to_string(),
            Some(p) => match &p.metrics {
                Some(m) => format!(
                    "embedded {:?}, {} bytes, {} glyphs, {} upm, family {}",
                    p.format,
                    p.program.len(),
                    m.glyph_count,
                    m.units_per_em,
                    m.family_name.as_deref().unwrap_or("?"),
                ),
                None => format!("embedded {:?}, {} bytes", p.format, p.program.len()),
            },
        };
        outln!(out, "{base} [{}] — {embedded}", font.subtype);
    }
    Ok(())
}

/// Dump each embedded font program into `outdir` (§9.9).
pub fn dump_fonts(input: &Path, outdir: &Path, out: &mut dyn Write) -> Result<(), String> {
    let doc = open(input)?;
    let fonts = prismpdf::document_fonts(&doc).map_err(|e| format!("cannot read fonts: {e}"))?;
    create_dir(outdir)?;
    let mut written = 0usize;
    for (i, font) in fonts.iter().enumerate() {
        if let Some(program) = &font.embedded {
            let stem: String = font
                .base_font
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
                .collect();
            let name = outdir.join(format!("{i}_{stem}.{}", program.format.extension()));
            write_extracted(&name, &program.program)?;
            written += 1;
        }
    }
    outln!(
        out,
        "dumped {written} embedded font program(s) to {}",
        outdir.display()
    );
    Ok(())
}

/// List the document's embedded file attachments (§7.11): name, size, MIME, relationship.
pub fn list_attachments(path: &Path, out: &mut dyn Write) -> Result<(), String> {
    let doc = open(path)?;
    let attachments = doc
        .attachments()
        .map_err(|e| format!("cannot read attachments: {e}"))?;
    if attachments.is_empty() {
        outln!(out, "no attachments");
        return Ok(());
    }
    for a in &attachments {
        outln!(
            out,
            "{}\t{} bytes\t{}\t{}",
            a.name,
            a.data.len(),
            a.mime.as_deref().unwrap_or("-"),
            a.relationship.as_deref().unwrap_or("-"),
        );
    }
    Ok(())
}

/// Extract every embedded file attachment (§7.11) to `outdir`, decoded.
pub fn dump_attachments(input: &Path, outdir: &Path, out: &mut dyn Write) -> Result<(), String> {
    let doc = open(input)?;
    let attachments = doc
        .attachments()
        .map_err(|e| format!("cannot read attachments: {e}"))?;
    create_dir(outdir)?;
    for (i, a) in attachments.iter().enumerate() {
        write_extracted(&outdir.join(safe_filename(&a.name, i)), &a.data)?;
    }
    outln!(
        out,
        "extracted {} attachment(s) to {}",
        attachments.len(),
        outdir.display()
    );
    Ok(())
}

/// Reduce an attachment name to a safe local filename: its basename with non-alphanumeric
/// characters (besides `.`/`-`/`_`) replaced, defeating path traversal from a hostile `/F` entry.
pub fn safe_filename(name: &str, index: usize) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or("");
    let cleaned: String = base
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = cleaned.trim_matches('.');
    if trimmed.is_empty() {
        format!("attachment_{index}")
    } else {
        trimmed.to_string()
    }
}

/// List every page's annotations (§12.5): page index, subtype, contents text and any link URI.
pub fn list_annotations(path: &Path, out: &mut dyn Write) -> Result<(), String> {
    let doc = open(path)?;
    let pages = doc
        .page_count()
        .map_err(|e| format!("cannot read pages: {e}"))?;
    let mut total = 0usize;
    for index in 0..pages {
        let annots = prismpdf::page_annotations(&doc, index)
            .map_err(|e| format!("cannot read annotations: {e}"))?;
        for a in &annots {
            total += 1;
            let detail = match (&a.uri, a.dest_page, &a.contents) {
                (Some(uri), _, _) => uri.clone(),
                (_, Some(page), _) => format!("→ page {page}"),
                (_, _, Some(contents)) => contents.clone(),
                _ => "-".to_string(),
            };
            outln!(out, "page {index}\t{}\t{detail}", a.subtype);
        }
    }
    if total == 0 {
        outln!(out, "no annotations");
    }
    Ok(())
}

/// List the document's interactive form fields (§12.7): fully-qualified name, type and value.
pub fn list_fields(path: &Path, out: &mut dyn Write) -> Result<(), String> {
    let doc = open(path)?;
    let fields = doc
        .form_fields()
        .map_err(|e| format!("cannot read form fields: {e}"))?;
    if fields.is_empty() {
        outln!(out, "no form fields");
        return Ok(());
    }
    for f in &fields {
        outln!(
            out,
            "{}\t{}\t{}",
            f.name,
            f.field_type,
            f.value.as_deref().unwrap_or("-")
        );
    }
    Ok(())
}

/// Fill AcroForm fields (§12.7) from `values` and write the updated PDF to `output` as an
/// incremental update.
pub fn fill_fields(
    input: &Path,
    output: &Path,
    values: &[FieldValue],
    out: &mut dyn Write,
) -> Result<(), String> {
    let pairs: Vec<(&str, &str)> = values
        .iter()
        .map(|v| (v.name.as_str(), v.value.as_str()))
        .collect();

    let doc = open(input)?;
    let filled = doc
        .fill_form(&pairs)
        .map_err(|e| format!("cannot fill form: {e}"))?;
    write(output, &filled)?;
    outln!(
        out,
        "filled {} field(s) → {}",
        pairs.len(),
        output.display()
    );
    Ok(())
}

/// Flatten the document's form fields (§12.7.4) and write the result to `output`.
pub fn flatten_form(input: &Path, output: &Path, out: &mut dyn Write) -> Result<(), String> {
    let doc = open(input)?;
    let flattened = doc
        .flatten_form()
        .map_err(|e| format!("cannot flatten form: {e}"))?;
    write(output, &flattened)?;
    outln!(out, "flattened → {}", output.display());
    Ok(())
}

/// Print the document outline / bookmark tree (§12.3.3), indented by depth.
pub fn list_outline(path: &Path, out: &mut dyn Write) -> Result<(), String> {
    let doc = open(path)?;
    let outline = doc
        .outline()
        .map_err(|e| format!("cannot read outline: {e}"))?;
    if outline.is_empty() {
        outln!(out, "no outline");
        return Ok(());
    }
    print_outline(&outline, 0, out)
}

/// Print `items` at `depth`, then their children one level deeper.
fn print_outline(
    items: &[prismpdf::OutlineItem],
    depth: usize,
    out: &mut dyn Write,
) -> Result<(), String> {
    for item in items {
        let target = item
            .dest_page
            .map(|p| format!(" → page {p}"))
            .unwrap_or_default();
        outln!(out, "{}{}{target}", "  ".repeat(depth), item.title);
        print_outline(&item.children, depth + 1, out)?;
    }
    Ok(())
}

/// Print the document's XMP metadata packet (§14.3.2), or note its absence.
pub fn show_xmp(path: &Path, out: &mut dyn Write) -> Result<(), String> {
    let doc = open(path)?;
    let xmp = doc
        .xmp_metadata()
        .map_err(|e| format!("cannot read XMP: {e}"))?;
    match xmp {
        Some(packet) => outp!(out, "{packet}"),
        None => outln!(out, "no XMP metadata"),
    }
    Ok(())
}

/// Digitally sign `input` (§12.8) with a DER X.509 `cert` and PKCS#8 DER `key`, writing the signed
/// PDF to `output`.
pub fn sign(
    input: &Path,
    output: &Path,
    cert: &Path,
    key: &Path,
    out: &mut dyn Write,
) -> Result<(), String> {
    let cert = read(cert)?;
    let key = read(key)?;
    let doc = open(input)?;
    let signed = doc
        .sign(&cert, &key)
        .map_err(|e| format!("cannot sign: {e}"))?;
    write(output, &signed)?;
    outln!(out, "signed → {}", output.display());
    Ok(())
}

/// Verify the document's digital signatures (§12.8.1) and report each. Any `roots` (DER X.509) are
/// trust anchors: when given, each signer's certificate chain is validated against them (PAdES-B).
pub fn verify(
    input: &Path,
    roots: &[std::path::PathBuf],
    out: &mut dyn Write,
) -> Result<(), String> {
    let doc = open(input)?;
    let anchors: Vec<Vec<u8>> = roots
        .iter()
        .map(|p| read(p))
        .collect::<Result<_, String>>()?;
    let signatures = doc
        .verify_signatures_with(&anchors)
        .map_err(|e| format!("cannot verify: {e}"))?;
    if signatures.is_empty() {
        outln!(out, "no signatures");
        return Ok(());
    }
    for s in &signatures {
        let trust = match s.trusted {
            Some(true) => "trusted",
            Some(false) => "untrusted",
            None => "trust-unchecked",
        };
        outp!(
            out,
            "{}\t{}\t{} bytes covered\t{trust}",
            if s.valid { "VALID" } else { "INVALID" },
            s.signer.as_deref().unwrap_or("(unknown signer)"),
            s.covered_bytes,
        );
        if let Some(t) = s.signing_time {
            outp!(out, "\tsigned@{t}");
        }
        if let Some(t) = s.timestamp_time {
            outp!(out, "\ttimestamped@{t}");
        }
        outln!(out, "");
    }
    Ok(())
}

/// Subset the sfnt font at `font` to the glyphs needed for `text`, writing it to `output` (§9.9).
pub fn subset_font(
    font: &Path,
    output: &Path,
    text: &str,
    out: &mut dyn Write,
) -> Result<(), String> {
    let program = read(font)?;
    let glyphs = prismpdf::glyphs_for_text(&program, text)
        .ok_or_else(|| "not a valid TrueType/OpenType font".to_string())?;
    let subset = prismpdf::subset_sfnt(&program, &glyphs)
        .ok_or_else(|| "subsetting failed (only sfnt fonts are supported)".to_string())?;
    write(output, &subset)?;
    outln!(
        out,
        "subset {} → {} bytes ({} glyphs)",
        program.len(),
        subset.len(),
        glyphs.len()
    );
    Ok(())
}

/// Subset a PDF's embedded simple-TrueType fonts to the glyphs it uses, writing a smaller PDF.
pub fn subset_pdf(input: &Path, output: &Path, out: &mut dyn Write) -> Result<(), String> {
    let bytes = read(input)?;
    let original = bytes.len();
    let doc = open_input(bytes)?;
    let subset = prismpdf::subset_fonts(&doc).map_err(|e| format!("cannot subset fonts: {e}"))?;
    outln!(out, "subset fonts: {original} → {} bytes", subset.len());
    write(output, &subset)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attachment_names_cannot_escape_the_output_directory() {
        // A hostile `/F` entry (§7.11.3) is reduced to a plain basename.
        assert_eq!(safe_filename("../../etc/passwd", 0), "passwd");
        assert_eq!(safe_filename(r"..\..\windows\system32", 0), "system32");
        assert_eq!(safe_filename("/absolute/path.xml", 0), "path.xml");
        assert_eq!(
            safe_filename("in voice;rm -rf.xml", 0),
            "in_voice_rm_-rf.xml"
        );
        // Names that reduce to nothing fall back to their index.
        assert_eq!(safe_filename("..", 3), "attachment_3");
        assert_eq!(safe_filename("", 7), "attachment_7");
        assert_eq!(safe_filename("dir/", 1), "attachment_1");
    }

    /// An extracted image with the given payload and colour space, 1×1 at 8 bits.
    fn image(
        data: prismpdf::ImageData,
        color_space: prismpdf::ColorSpace,
    ) -> prismpdf::ExtractedImage {
        prismpdf::ExtractedImage {
            info: prismpdf::ImageInfo {
                width: 1,
                height: 1,
                bits_per_component: 8,
                color_space,
            },
            data,
        }
    }

    #[test]
    fn extracted_images_pick_the_right_container() {
        use prismpdf::{ColorSpace, ImageData};

        // Codec payloads are written verbatim under their own extension.
        for (data, ext, payload) in [
            (ImageData::Jpeg(vec![0xFF, 0xD8]), "jpg", vec![0xFF, 0xD8]),
            (
                ImageData::Jpeg2000(vec![0x6A, 0x50]),
                "jp2",
                vec![0x6A, 0x50],
            ),
            (
                ImageData::Jbig2(vec![0x97, 0x4A]),
                "jbig2",
                vec![0x97, 0x4A],
            ),
        ] {
            let (got_ext, got) = encode_image(&image(data, ColorSpace::DeviceRgb));
            assert_eq!(got_ext, ext);
            assert_eq!(got, payload);
        }

        // 8-bit Gray and RGB samples become NetPBM, header first.
        let (ext, out) = encode_image(&image(ImageData::Raw(vec![128]), ColorSpace::DeviceGray));
        assert_eq!(ext, "pgm");
        assert_eq!(out, b"P5\n1 1\n255\n\x80");

        let (ext, out) = encode_image(&image(ImageData::Raw(vec![1, 2, 3]), ColorSpace::DeviceRgb));
        assert_eq!(ext, "ppm");
        assert_eq!(out, b"P6\n1 1\n255\n\x01\x02\x03");

        // A space NetPBM cannot express falls back to the raw samples.
        let (ext, out) = encode_image(&image(
            ImageData::Raw(vec![9, 9, 9, 9]),
            ColorSpace::DeviceCmyk,
        ));
        assert_eq!(ext, "bin");
        assert_eq!(out, vec![9, 9, 9, 9]);

        // So does a depth NetPBM cannot express.
        let mut deep = image(ImageData::Raw(vec![0, 255]), ColorSpace::DeviceGray);
        deep.info.bits_per_component = 16;
        assert_eq!(encode_image(&deep).0, "bin");
    }

    #[test]
    fn a_closed_sink_is_an_error_not_a_panic() {
        // `inspect` writes before it can know the sink is dead; the I/O error must surface as the
        // CLI's error type rather than a panic (DESIGN.md §3.4).
        struct Closed;
        impl Write for Closed {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::from(std::io::ErrorKind::BrokenPipe))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let pdf = prismpdf::Builder::new().add_page(prismpdf::PageSpec::new("")).build();
        let path = std::env::temp_dir().join(format!("prismpdf-closed-{}.pdf", std::process::id()));
        std::fs::write(&path, &pdf).expect("write fixture");
        let mut sink = Closed;
        // The binary flushes after the command returns; only the writes fail here.
        assert!(sink.flush().is_ok());
        let error = inspect(&path, &mut sink).expect_err("broken pipe");
        let _ = std::fs::remove_file(&path);
        assert!(error.contains("cannot write output"), "{error}");
    }
}
