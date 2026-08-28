//! M18 `prismpdf-verify` — cross-validation oracle for **base PDF** (1.4–2.0).
//!
//! veraPDF (the PDF/A·UA·X oracle in `conformance_corpus.rs`) does **not** validate plain PDF, so
//! "is this a valid PDF of version N?" needs a different oracle. This harness runs a **panel of
//! independent, third-party PDF validators** over a corpus and aggregates their verdicts: a file is
//! "accepted" when the panel agrees. No single tool is authoritative — agreement is the signal.
//!
//! Panel (each resolved from `$PATH` or the repo's `tools/verify/`, skipped cleanly if absent):
//!
//! - **qpdf `--check`** — structural validity (xref/streams/objects). Exit 0 = ok, 3 = warnings, 2 = errors.
//! - **mutool info** — acceptance by MuPDF's parser.
//! - **gs** (nullpage device) — acceptance by Ghostscript's interpreter.
//! - **pdfinfo** (poppler) — acceptance by poppler's parser.
//! - **pdfcpu validate** — a Go validator that asserts spec conformance (relaxed mode).
//!
//! None of these is a *strict version-conformance* checker ("forbid a 1.5 feature in a 1.4 file") —
//! that is the internal version-boundary check (M18 Phase 2). The panel answers "is it a valid PDF
//! real consumers accept", grouped by the file's declared header version.
//!
//! It walks the corpus + an authored 1.4–2.0 matrix, runs the available panel, and writes a grouped
//! report to `target/verify-baseline.json` (view with `cargo test -- --nocapture`). It **gates**
//! (M18 Phase 4): every should-pass file must be accepted by the panel *majority* and no file may
//! declare a header below its content's minimum (`malformed` stays informational — see
//! `docs/baselines/verify.md`). The gate needs at least [`GATE_QUORUM`] validators, since a
//! majority cannot survive one member's feature gap in a smaller panel; below that it reports
//! without asserting. Set `PRISMPDF_VERIFY_REPORT_ONLY=1` to do the same deliberately. When no
//! validator resolves, or no corpus exists, it skips.
//! Run: `cargo test -p prismpdf --test verify_base -- --nocapture`.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use prismpdf::{
    Algorithm, AnnotationSpec, Attachment, Builder, Content, Document, DocumentPart,
    ImageColorSpace, LinkTarget, PageSpec, SeparationSpec, StdFont, StructElem,
};
use serde::Serialize;
use walkdir::WalkDir;

/// Workspace root (this crate is `crates/pdf`).
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .unwrap()
        .to_path_buf()
}

/// Resolve a tool: prefer the repo-local `tools/verify/<name>`, else `<name>` on `$PATH`.
fn resolve(name: &str) -> Option<PathBuf> {
    let local = workspace_root().join("tools/verify").join(name);
    if local.is_file() {
        return Some(local);
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join(name))
        .find(|p| p.is_file())
}

/// A validator's verdict on one file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
enum Verdict {
    /// Accepted with no diagnostics.
    Ok,
    /// Accepted, but the tool emitted warnings.
    Warn,
    /// Rejected (parse/structural error).
    Reject,
    /// The tool could not be run (should not happen once resolved).
    Error,
}

impl Verdict {
    fn accepts(self) -> bool {
        matches!(self, Verdict::Ok | Verdict::Warn)
    }
}

/// One panel member: a resolved binary plus how to invoke it and read its verdict.
struct Validator {
    name: &'static str,
    bin: PathBuf,
}

impl Validator {
    /// The whole panel that resolved in this environment.
    /// Every panel member, resolved or not — the denominator the quorum is reported against.
    const NAMES: [&'static str; 5] = ["qpdf", "mutool", "gs", "pdfinfo", "pdfcpu"];
    const PANEL_SIZE: usize = Self::NAMES.len();

    fn panel() -> Vec<Validator> {
        Self::NAMES
            .into_iter()
            .filter_map(|n| resolve(n).map(|bin| Validator { name: n, bin }))
            .collect()
    }

    fn run(&self, file: &Path) -> Verdict {
        let f = file.as_os_str();
        let out = match self.name {
            "qpdf" => Command::new(&self.bin).arg("--check").arg(f).output(),
            "mutool" => Command::new(&self.bin).arg("info").arg(f).output(),
            "gs" => Command::new(&self.bin)
                .args([
                    "-dBATCH",
                    "-dNOPAUSE",
                    "-dNOPROMPT",
                    "-sDEVICE=nullpage",
                    "-q",
                ])
                .arg(f)
                .output(),
            "pdfinfo" => Command::new(&self.bin).arg(f).output(),
            "pdfcpu" => Command::new(&self.bin).arg("validate").arg(f).output(),
            _ => return Verdict::Error,
        };
        let Ok(out) = out else { return Verdict::Error };
        let code = out.status.code();
        match self.name {
            // qpdf: 0 = clean, 3 = warnings (still readable), 2 = errors.
            "qpdf" => match code {
                Some(0) => Verdict::Ok,
                Some(3) => Verdict::Warn,
                _ => Verdict::Reject,
            },
            // The rest signal acceptance purely by exit status.
            _ => {
                if out.status.success() {
                    Verdict::Ok
                } else {
                    Verdict::Reject
                }
            }
        }
    }
}

/// Read the declared header version (`%PDF-x.y`) from the first 1 KiB, like the reader's
/// `parse_header` (§7.5.2). Returns e.g. `"1.7"`, or `"?"` if absent.
fn header_version(file: &Path) -> String {
    let Ok(bytes) = std::fs::read(file) else {
        return "?".into();
    };
    let scan = &bytes[..bytes.len().min(1024)];
    scan.windows(5)
        .position(|w| w == b"%PDF-")
        .and_then(|i| {
            let tail = &scan[i + 5..];
            let end = tail
                .iter()
                .position(|b| !(b.is_ascii_digit() || *b == b'.'))
                .unwrap_or(tail.len());
            std::str::from_utf8(&tail[..end])
                .ok()
                .map(|s| s.to_string())
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "?".into())
}

/// Per-(version) aggregate counts within one corpus set.
#[derive(Debug, Default, Serialize)]
struct VersionStats {
    files: usize,
    /// Files the *entire* panel accepted.
    panel_unanimous_accept: usize,
    /// Per-tool acceptance count.
    per_tool_accept: BTreeMap<String, usize>,
}

/// One corpus set (`valid`, `edge`, …) and whether the panel is *expected* to accept it.
struct CorpusSet {
    /// Display name + relative dir under the workspace.
    name: &'static str,
    dir: &'static str,
    expect_accept: bool,
}

#[derive(Debug, Serialize)]
struct SetReport {
    expect_accept: bool,
    by_version: BTreeMap<String, VersionStats>,
    /// Files whose verdict contradicts the expectation (the interesting signal).
    anomalies: Vec<Anomaly>,
}

#[derive(Debug, Serialize)]
struct Anomaly {
    file: String,
    version: String,
    /// Per-tool verdicts for this file.
    verdicts: BTreeMap<String, Verdict>,
}

#[derive(Debug, Serialize)]
struct Report {
    tools: Vec<String>,
    sets: BTreeMap<String, SetReport>,
    /// Files whose declared header version is below what their content requires (M18 Phase 2).
    boundary_violations: Vec<BoundaryViolation>,
}

/// Author the version matrix the producer can now emit (M17 Phase 1 + cipher floor), one file per
/// declared version, into `out`. Proves the *Validato* column spans 1.4–2.0, and gives the panel +
/// boundary check real version-targeted output to grade. Returns the files written.
///
/// - **1.4** — a plain `Builder` page (auto-stamped minimum: only ≤1.4 constructs).
/// - **1.5** — the same document re-saved with a cross-reference *stream* (`save_compact`).
/// - **1.6** — AES-128 encrypted (empty user password, so consumers open it).
/// - **2.0** — AES-256 encrypted (V5/AESV3).
fn author_version_matrix(out: &Path) -> Vec<PathBuf> {
    let _ = std::fs::create_dir_all(out);
    let mut b = Builder::new();
    b.add_page(
        PageSpec::new(b"BT /F1 24 Tf 72 700 Td (Prism PDF verify) Tj ET".to_vec())
            .standard_font("F1", StdFont::Helvetica),
    );
    let v14 = b.build();
    let Ok(doc) = Document::open(v14.clone()) else {
        return Vec::new();
    };
    let mut written = Vec::new();
    let mut put = |name: &str, bytes: &[u8]| {
        let p = out.join(name);
        if std::fs::write(&p, bytes).is_ok() {
            written.push(p);
        }
    };
    put("authored-1.4.pdf", &v14);

    // A spot-colour (Separation §8.6.6) document — a base-PDF feature the panel validates (M19
    // Phase A). Header is 1.4 (Separation is ≤1.4), so it groups under the 1.4 bucket.
    let mut sep = Builder::new();
    sep.add_page(PageSpec::new(
        b"/Spot cs 1 scn 72 700 200 100 re f".to_vec(),
    ))
    .add_separation(
        "Spot",
        SeparationSpec {
            colorant: "PANTONE 185 C".into(),
            alternate: ImageColorSpace::Cmyk,
            full: vec![0.0, 0.91, 0.76, 0.0],
        },
    );
    put("authored-separation-1.4.pdf", &sep.build());

    // An ICCBased (sRGB) colour space — base-PDF feature (≤1.3), validated by the panel (M19
    // Phase A). Uses the bundled CC0 sRGB profile.
    let mut icc = Builder::new();
    icc.add_page(PageSpec::new(
        b"/ICC cs 0.2 0.4 0.6 scn 72 400 200 100 re f".to_vec(),
    ))
    .add_icc_based(
        "ICC",
        prismpdf::OutputIntentProfile::srgb().icc().to_vec(),
        3,
    );
    put("authored-iccbased-1.4.pdf", &icc.build());

    // An Indexed (palette) colour space — base-PDF feature, validated by the panel (M19 Phase A).
    let mut idx = Builder::new();
    idx.add_page(PageSpec::new(b"/Pal cs 1 scn 72 300 200 100 re f".to_vec()))
        .add_indexed(
            "Pal",
            ImageColorSpace::Rgb,
            vec![255, 0, 0, 0, 255, 0, 0, 0, 255],
        );
    put("authored-indexed-1.4.pdf", &idx.build());

    // A reusable content Form XObject (§8.10), painted twice — base-PDF feature, panel-validated.
    let mut form = Builder::new();
    form.add_page(PageSpec::new(
        b"q 1 0 0 1 72 600 cm /Stamp Do Q q 1 0 0 1 300 600 cm /Stamp Do Q".to_vec(),
    ))
    .add_form_xobject(
        "Stamp",
        [0.0, 0.0, 40.0, 40.0],
        b"0.2 g 0 0 40 40 re f".to_vec(),
        Vec::new(),
    );
    put("authored-formxobject-1.4.pdf", &form.build());

    // An inline image (§8.9.7) — a 2×2 RGB pixel block scaled into a 40pt square.
    let mut ic = Content::new();
    ic.save()
        .transform(40.0, 0.0, 0.0, 40.0, 72.0, 200.0)
        .inline_image(
            2,
            2,
            "RGB",
            8,
            &[255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 0],
        )
        .restore();
    let mut inl = Builder::new();
    inl.add_page(PageSpec::new(ic.into_bytes()));
    put("authored-inline-1.4.pdf", &inl.build());

    // A CIE L*a*b* colour space (§8.6.5.4) — base-PDF feature, panel-validated.
    let mut lab = Builder::new();
    lab.add_page(PageSpec::new(
        b"/Lab cs 50 20 -30 scn 72 100 200 60 re f".to_vec(),
    ))
    .add_lab("Lab", [0.9505, 1.0, 1.089], [-128.0, 127.0, -128.0, 127.0]);
    put("authored-lab-1.4.pdf", &lab.build());

    if let Ok(b) = doc.save_compact() {
        put("authored-1.5.pdf", &b);
    }
    if let Ok(b) = doc.save_encrypted(b"", b"", Algorithm::Aes128) {
        put("authored-1.6.pdf", &b);
    }
    if let Ok(b) = doc.save_encrypted(b"", b"", Algorithm::Aes256) {
        put("authored-2.0.pdf", &b);
    }

    // A genuinely-structural PDF 2.0 file: Document Parts (§14.12). The catalog /DPartRoot
    // auto-stamps %PDF-2.0 — this is the 2.0 sample that exercises a 2.0 *structure*, not just the
    // AES-256 cipher (M19 Phase C).
    let mut dp = Builder::new();
    dp.add_page(
        PageSpec::new(b"BT /F1 12 Tf 72 700 Td (part one) Tj ET".to_vec())
            .standard_font("F1", StdFont::Helvetica),
    )
    .add_page(
        PageSpec::new(b"BT /F1 12 Tf 72 700 Td (part two) Tj ET".to_vec())
            .standard_font("F1", StdFont::Helvetica),
    )
    .document_parts(&[
        DocumentPart {
            first_page: 0,
            last_page: 0,
            dpm: vec![("Title".into(), "Part one".into())],
        },
        DocumentPart {
            first_page: 1,
            last_page: 1,
            dpm: vec![("Title".into(), "Part two".into())],
        },
    ]);
    put("authored-dparts-2.0.pdf", &dp.build());

    // A genuinely-structural PDF 2.0 file: a Tagged document with structure namespaces (§14.7.4).
    // The /StructTreeRoot /Namespaces array + per-element /NS auto-stamp %PDF-2.0 — a 2.0 *structure*
    // feature distinct from DParts and the AES-256 cipher (M19 Phase C). Also the PDF/UA-2 substrate.
    let mut ns = Builder::new();
    ns.add_page(
        PageSpec::new(b"/P <</MCID 0>> BDC BT /F1 12 Tf 72 700 Td (namespaced) Tj ET EMC".to_vec())
            .standard_font("F1", StdFont::Helvetica),
    )
    .lang("en-US")
    .structure_namespace("http://iso.org/pdf2/ssn")
    .structure(vec![{
        let mut p = StructElem::new("P");
        p.push_content(0, 0);
        p
    }]);
    put("authored-namespaces-2.0.pdf", &ns.build());

    // A PDF 2.0 file whose document metadata uses UTF-8 text strings (§7.9.2.2): the non-ASCII
    // Title carries a EF BB BF BOM, which auto-stamps %PDF-2.0 (M19 Phase C).
    let mut u8doc = Builder::new();
    u8doc
        .add_page(
            PageSpec::new(b"BT /F1 12 Tf 72 700 Td (utf-8 metadata) Tj ET".to_vec())
                .standard_font("F1", StdFont::Helvetica),
        )
        .title("Rapport trimestriel — Café & résumé")
        .author("José Müller")
        .utf8_text_strings();
    put("authored-utf8-2.0.pdf", &u8doc.build());

    // A PDF 2.0 file with a **page-level** associated file (§14.13.4): the filespec hangs off the
    // page's /AF, not the catalog — a 2.0 structural placement that auto-stamps %PDF-2.0.
    let mut af = Builder::new();
    af.add_page(
        PageSpec::new(b"BT /F1 12 Tf 72 700 Td (see attached data) Tj ET".to_vec())
            .standard_font("F1", StdFont::Helvetica),
    )
    .attach_file_to_page(
        0,
        Attachment {
            name: "data.csv".into(),
            mime: "text/csv".into(),
            relationship: "Data".into(),
            description: Some("page-associated dataset".into()),
            mod_date: None,
            data: b"a,b,c\n1,2,3\n".to_vec(),
        },
    );
    put("authored-pageaf-2.0.pdf", &af.build());

    // A PDF 2.0 file with an associated file on a **structure element** (§14.13.6, the 2.0-preferred
    // placement): a tagged Figure carrying a data file via its `/AF`. Auto-stamps %PDF-2.0.
    let mut saf = Builder::new();
    saf.add_page(
        PageSpec::new(
            b"/Figure <</MCID 0>> BDC BT /F1 12 Tf 72 700 Td (figure) Tj ET EMC".to_vec(),
        )
        .standard_font("F1", StdFont::Helvetica),
    )
    .lang("en-US")
    .structure(vec![{
        let mut fig = StructElem::new("Figure")
            .alt("a chart")
            .associate_file(Attachment {
                name: "chart.csv".into(),
                mime: "text/csv".into(),
                relationship: "Supplement".into(),
                description: Some("the figure's source data".into()),
                mod_date: None,
                data: b"x,y\n1,2\n3,4\n".to_vec(),
            });
        fig.push_content(0, 0);
        fig
    }]);
    put("authored-structaf-2.0.pdf", &saf.build());

    // A PDF 2.0 file with an associated file on an **annotation** (§14.13.9): a link annotation
    // carrying a data file via its `/AF`. A non-empty annotation /AF auto-stamps %PDF-2.0.
    let mut aaf = Builder::new();
    aaf.add_page(
        PageSpec::new(b"BT /F1 12 Tf 72 700 Td (linked) Tj ET".to_vec())
            .standard_font("F1", StdFont::Helvetica),
    )
    .add_annotation(
        0,
        AnnotationSpec::Link {
            rect: [72.0, 695.0, 140.0, 710.0],
            target: LinkTarget::Uri("https://example.org/".into()),
            contents: None,
        },
        vec![Attachment {
            name: "link-data.csv".into(),
            mime: "text/csv".into(),
            relationship: "Data".into(),
            description: Some("data behind the link".into()),
            mod_date: None,
            data: b"k,v\n1,2\n".to_vec(),
        }],
    );
    put("authored-annotaf-2.0.pdf", &aaf.build());

    // A PDF 2.0 file with an associated file on a **form XObject** (§14.13.7): a reusable content
    // form carrying its source data via the stream dict's `/AF`. Auto-stamps %PDF-2.0.
    let mut xaf = Builder::new();
    xaf.add_page(PageSpec::new(b"q 1 0 0 1 72 700 cm /Fx Do Q".to_vec()))
        .add_form_xobject(
            "Fx",
            [0.0, 0.0, 40.0, 12.0],
            b"0 0 40 12 re f".to_vec(),
            vec![Attachment {
                name: "form-data.csv".into(),
                mime: "text/csv".into(),
                relationship: "Source".into(),
                description: Some("the form's source data".into()),
                mod_date: None,
                data: b"p,q\n3,4\n".to_vec(),
            }],
        );
    put("authored-xobjectaf-2.0.pdf", &xaf.build());
    written
}

/// Version-boundary verdict (M18 Phase 2): does the declared header version cover what the content
/// requires? Computed via the engine's own [`Document::min_pdf_version`] (shares the
/// feature→version table with the producer). Best-effort: files the engine can't open are skipped.
#[derive(Debug, Serialize)]
struct BoundaryViolation {
    file: String,
    declared: String,
    min_required: String,
}

fn boundary_check(file: &Path) -> Option<BoundaryViolation> {
    let bytes = std::fs::read(file).ok()?;
    let doc = Document::open(bytes).ok()?;
    let declared = doc.version().map(|v| (v.major, v.minor))?;
    let min = doc.min_pdf_version().ok()?;
    (declared < min).then(|| BoundaryViolation {
        file: file.file_name().unwrap().to_string_lossy().into_owned(),
        declared: format!("{}.{}", declared.0, declared.1),
        min_required: format!("{}.{}", min.0, min.1),
    })
}

fn pdfs_in(dir: &Path) -> Vec<PathBuf> {
    if !dir.is_dir() {
        return Vec::new();
    }
    WalkDir::new(dir)
        .into_iter()
        .filter_map(Result::ok)
        .map(|e| e.into_path())
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("pdf")))
        .collect()
}

/// Smallest panel a *majority* verdict is meaningful for: three, so one member's feature gap
/// leaves a 2-of-3 majority intact. See the gate at the end of the harness.
const GATE_QUORUM: usize = 3;

#[test]
fn base_pdf_cross_validation_baseline() {
    let panel = Validator::panel();
    if panel.is_empty() {
        eprintln!(
            "verify_base: SKIP — no base-PDF validator resolved \
             (install qpdf/mupdf-tools/ghostscript/poppler-utils, or drop pdfcpu in tools/verify/)"
        );
        return;
    }
    let tool_names: Vec<String> = panel.iter().map(|v| v.name.to_string()).collect();

    let root = workspace_root();
    // Author the version matrix (1.4/1.5/1.6/2.0) the producer can now emit, then validate it
    // alongside the committed corpus — this is what makes the baseline span versions.
    author_version_matrix(&root.join("target/verify-samples"));
    let sets = [
        CorpusSet {
            name: "authored-matrix",
            dir: "target/verify-samples",
            expect_accept: true,
        },
        CorpusSet {
            name: "valid",
            dir: "corpus/valid",
            expect_accept: true,
        },
        CorpusSet {
            name: "edge",
            dir: "corpus/edge",
            expect_accept: true,
        },
        CorpusSet {
            name: "prismpdf-pdfa",
            dir: "corpus/prismpdf-pdfa",
            expect_accept: true,
        },
        CorpusSet {
            name: "malformed",
            dir: "corpus/malformed",
            expect_accept: false,
        },
    ];

    let mut report = Report {
        tools: tool_names.clone(),
        sets: BTreeMap::new(),
        boundary_violations: Vec::new(),
    };
    let mut total = 0usize;

    for set in &sets {
        let files = pdfs_in(&root.join(set.dir));
        if files.is_empty() {
            continue;
        }
        let mut sr = SetReport {
            expect_accept: set.expect_accept,
            by_version: BTreeMap::new(),
            anomalies: Vec::new(),
        };
        for file in &files {
            total += 1;
            let version = header_version(file);
            let verdicts: BTreeMap<String, Verdict> = panel
                .iter()
                .map(|v| (v.name.to_string(), v.run(file)))
                .collect();
            let accepts: Vec<bool> = verdicts.values().map(|v| v.accepts()).collect();
            let unanimous_accept = accepts.iter().all(|&a| a);

            let vs = sr.by_version.entry(version.clone()).or_default();
            vs.files += 1;
            if unanimous_accept {
                vs.panel_unanimous_accept += 1;
            }
            for (name, verdict) in &verdicts {
                if verdict.accepts() {
                    *vs.per_tool_accept.entry(name.clone()).or_default() += 1;
                }
            }

            // Anomaly = expectation contradicted by the panel majority.
            let n_accept = accepts.iter().filter(|&&a| a).count();
            let majority_accept = n_accept * 2 > accepts.len();
            if majority_accept != set.expect_accept {
                sr.anomalies.push(Anomaly {
                    file: file.file_name().unwrap().to_string_lossy().into_owned(),
                    version,
                    verdicts,
                });
            }

            // Version-boundary check (M18 Phase 2): declared header vs content's minimum.
            if let Some(v) = boundary_check(file) {
                report.boundary_violations.push(v);
            }
        }
        report.sets.insert(set.name.to_string(), sr);
    }

    // --- write JSON report ---
    let out_path = root.join("target/verify-baseline.json");
    let json = serde_json::to_string_pretty(&report).unwrap();
    let _ = std::fs::write(&out_path, &json);

    // --- human summary ---
    eprintln!(
        "\n=== M18 verify_base — panel: {} ===",
        tool_names.join(", ")
    );
    for (set, sr) in &report.sets {
        eprintln!(
            "\n[{set}]  (expect {})",
            if sr.expect_accept { "ACCEPT" } else { "REJECT" }
        );
        for (ver, vs) in &sr.by_version {
            let per_tool: Vec<String> = tool_names
                .iter()
                .map(|t| {
                    format!(
                        "{t}={}/{}",
                        vs.per_tool_accept.get(t).copied().unwrap_or(0),
                        vs.files
                    )
                })
                .collect();
            eprintln!(
                "  PDF {ver:<4} files={:<3} panel-unanimous-accept={:<3} | {}",
                vs.files,
                vs.panel_unanimous_accept,
                per_tool.join(" ")
            );
        }
        if !sr.anomalies.is_empty() {
            eprintln!("  anomalies ({}):", sr.anomalies.len());
            for a in &sr.anomalies {
                eprintln!("    {} (v{}): {:?}", a.file, a.version, a.verdicts);
            }
        }
    }
    eprintln!(
        "\n[version-boundary] {} violation(s) (declared header < content minimum)",
        report.boundary_violations.len()
    );
    for v in &report.boundary_violations {
        eprintln!(
            "    {} declares {} but needs {}",
            v.file, v.declared, v.min_required
        );
    }
    eprintln!("\n{} files graded → {}", total, out_path.display());

    // --- gate (M18 Phase 4) ---
    // Two assertions, scoped to what the panel is actually good at (confirming validity, not
    // detecting malformation — the `malformed` set stays informational, see docs/baselines/verify.md):
    //   1. every file we expect to be valid is accepted by the panel majority, and
    //   2. the producer never stamps a header below the content's minimum (M18 Phase 2).
    // Opt out for exploratory runs with PRISMPDF_VERIFY_REPORT_ONLY=1.
    if std::env::var_os("PRISMPDF_VERIFY_REPORT_ONLY").is_some() {
        return;
    }
    // Assertion 1 is a *majority* verdict precisely so one member's feature gap cannot fail a valid
    // file (pdfcpu rejects Document Parts §14.12 as "DPartRoot not supported" — its own banner says
    // PDF 2.0 is supported on a need basis). That reasoning needs a panel big enough for a majority
    // to survive one dissenter, which takes three: with two, a single gap is already half the vote,
    // and with one "majority" just means "this tool is authoritative" — the opposite of the oracle
    // this harness is built on. Below quorum the report still prints; only the gate stands down, so
    // a partial local install reports honestly instead of failing valid output.
    if tool_names.len() < GATE_QUORUM {
        eprintln!(
            "\nverify_base: REPORT ONLY — {} of {} validators resolved ({}), below the quorum of \
             {GATE_QUORUM} a majority verdict needs. Install the rest to gate locally: \
             apt-get install qpdf mupdf-tools ghostscript poppler-utils (see tools/verify/README.md).",
            tool_names.len(),
            Validator::PANEL_SIZE,
            tool_names.join(", "),
        );
        return;
    }
    let should_pass_failures: usize = report
        .sets
        .values()
        .filter(|sr| sr.expect_accept)
        .map(|sr| sr.anomalies.len())
        .sum();
    assert_eq!(
        should_pass_failures, 0,
        "a should-pass file was rejected by the validator panel (see anomalies above)"
    );
    assert!(
        report.boundary_violations.is_empty(),
        "producer stamped a header version below the content's minimum (see violations above)"
    );
}
