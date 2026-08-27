//! Data-driven PDF/A conformance harness over the public validation-test suites
//! (Isartor / BFO / veraPDF), graded against the expectation encoded in each filename.
//!
//! The three suites share one idea: the filename declares whether a *conformant* validator must
//! **pass** or **fail** the file, and maps it to an ISO 19005 clause. The conventions differ only
//! in their prefix:
//!
//! - Isartor — `isartor-<clause>-t<topic>-<expected>-<instance>.pdf`
//!   e.g. `isartor-6-7-8-t02-fail-a.pdf`
//! - BFO     — `pdfa2-<clause>-<creator>-t<topic>-<expected>.pdf`
//!   e.g. `pdfa2-6-1-3-bfo-t01-fail.pdf`
//! - veraPDF — `veraPDF test suite <clause>-t<topic>-<expected>-<instance>.pdf`
//!   e.g. `veraPDF test suite 6-2-11-7-3-t01-fail-e.pdf`
//!
//! `<clause>` is a dash-separated run of numbers (the ISO sub-clause path), `<topic>` is `t<NN>`,
//! and `<expected>` is always the literal `pass` or `fail`.
//!
//! veraPDF-corpus additionally ships two variants this harness also parses: bare/dotted-clause
//! names with no prefix (`7.1-t03-pass-b.pdf`) and the TWG set keyed by id+profile
//! (`TWG test suite A007-pdfa2-fail-a.pdf`, grouped under its profile).
//!
//! - Prism PDF — our own committed corpus under `corpus/prismpdf-pdfa/` (redistributable, unlike the
//!   third-party suites). PASS-only producer proof: `prismpdf-<feature>-<flavour>-pass.pdf`, grouped
//!   by flavour. See `corpus/prismpdf-pdfa/README.md`.
//!
//! The third-party suites are NOT committed (licences). Point the harness at them with
//! `PRISMPDF_CONFORMANCE_CORPUS=/path/to/corpus` (the sole root when set), or drop them under
//! `corpus/external/`. By default the harness walks the committed `corpus/prismpdf-pdfa/` **plus**
//! `corpus/external/` when present (suite subdirectory names don't matter; discovery is by
//! filename). When no root exists the walking test **skips** rather than fails; the parser/grader
//! unit tests always run. A grouped JSON report (by corpus, then by clause) is written to
//! `target/conformance-report.json` — view with `cargo test -- --nocapture`.
//!
//! NOTE: [`validate_pdf`] drives the real public API ([`prismpdf::Document::open`] + `page_count`),
//! but that is a **parse-survival proxy, not PDF/A conformance** — the engine has no conformance
//! validator yet (it ships production, `make_pdfa`, only). So a file the engine merely *parses*
//! grades as `pass`, and every non-conformant file therefore counts as a false positive: that
//! number measures how much rule-checking is still unimplemented, not a harness bug. The walking
//! test reports grades without asserting on them; wire a real validator into `validate_pdf` and
//! flip `ASSERT_NO_REGRESSIONS` to turn this into a gate.
//!
//! ORACLE: when the veraPDF CLI is on `PATH` (the devcontainer installs it; override with
//! `$VERAPDF_BIN`, disable with `$PRISMPDF_NO_ORACLE`), the harness grades Prism PDF against
//! veraPDF's authoritative `isCompliant` verdict instead of the filename label — veraPDF is the
//! reference PDF/A validator, flavour-aware per file. It runs batched (one JVM per ~500 files,
//! ~45s for the full corpus) and verdicts align with the input by `<job>` order. The report's
//! `oracle` block records how often veraPDF agrees with the suite labels (oracle health); the
//! per-case `oracle` field carries its verdict alongside Prism PDF's `actual`.
//!
//! Run manually now and then to check for regressions/improvements; the recorded baseline
//! (and how to read the numbers) lives in `docs/baselines/conformance.md`.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use regex::Regex;
use serde::Serialize;
use walkdir::WalkDir;

// ---------------------------------------------------------------------------
// Filename parsing
// ---------------------------------------------------------------------------

/// Which suite a file came from — also the top-level grouping key in the report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
enum Corpus {
    Isartor,
    Bfo,
    VeraPdf,
    /// The committed, Prism PDF-authored corpus under `corpus/prismpdf-pdfa/` (see its README).
    PrismPdf,
}

impl fmt::Display for Corpus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Corpus::Isartor => "isartor",
            Corpus::Bfo => "bfo",
            Corpus::VeraPdf => "verapdf",
            Corpus::PrismPdf => "prismpdf",
        })
    }
}

/// The expectation the filename declares for a conformant validator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
enum Expected {
    /// The file IS conformant — a validator must accept it (`true`).
    Pass,
    /// The file is NOT conformant — a validator must reject it (`false`).
    Fail,
}

/// One parsed corpus entry.
#[derive(Debug, Clone, PartialEq, Eq)]
struct CaseSpec {
    corpus: Corpus,
    /// ISO clause path as written in the filename (e.g. `6-7-8`); for TWG files, the profile
    /// (e.g. `pdfa2`), which is the most useful grouping key there.
    clause: String,
    /// Topic number without the `t` prefix (e.g. `02`); for TWG files, the test id (e.g. `A007`).
    topic: String,
    expected: Expected,
}

/// Lazily-compiled, case-insensitive matchers. Each has a disjoint anchor — three name prefixes
/// (`isartor-`, `pdfa2-`, `veraPDF test suite `, `TWG test suite `) plus the bare digit-led form —
/// so detection is order-independent. `[-.]` covers both clause separators seen in the wild
/// (`6-3-5` and `7.1`).
struct Patterns {
    isartor: Regex,
    bfo: Regex,
    verapdf: Regex,
    /// veraPDF-corpus files with no `veraPDF test suite ` prefix, e.g. `7.1-t03-pass-b.pdf`.
    verapdf_bare: Regex,
    /// veraPDF's TWG files, e.g. `TWG test suite A007-pdfa2-fail-a.pdf` — keyed by `<id>-<profile>`
    /// instead of an ISO clause/topic.
    twg: Regex,
    /// Prism PDF PASS corpus, `prismpdf-<feature>-<flavour>-pass.pdf` (producer proof). Keyed by
    /// flavour (one conformant file satisfies many clauses), feature as the topic — like TWG.
    /// PASS-only: there is no FAIL corpus (broken files would test a validator Prism PDF lacks).
    prismpdf_pass: Regex,
}

fn patterns() -> &'static Patterns {
    static P: OnceLock<Patterns> = OnceLock::new();
    // clause = number([-.]number)*; an optional trailing `-instance` (a letter/digit run).
    const CLAUSE: &str = r"\d+(?:[-.]\d+)*";
    P.get_or_init(|| Patterns {
        isartor: Regex::new(&format!(
            r"(?i)^isartor-(?P<clause>{CLAUSE})-t(?P<topic>\d+)-(?P<expected>pass|fail)(?:-[a-z0-9]+)?\.pdf$"
        ))
        .unwrap(),
        // clause is purely numeric; the creator token starts with a letter, so the boundary is
        // unambiguous even though both sit between dashes.
        bfo: Regex::new(&format!(
            r"(?i)^pdfa2-(?P<clause>{CLAUSE})-(?P<creator>[a-z][a-z0-9]*)-t(?P<topic>\d+)-(?P<expected>pass|fail)(?:-[a-z0-9]+)?\.pdf$"
        ))
        .unwrap(),
        verapdf: Regex::new(&format!(
            r"(?i)^veraPDF test suite (?P<clause>{CLAUSE})-t(?P<topic>\d+)-(?P<expected>pass|fail)(?:-[a-z0-9]+)?\.pdf$"
        ))
        .unwrap(),
        verapdf_bare: Regex::new(&format!(
            r"(?i)^(?P<clause>{CLAUSE})-t(?P<topic>\d+)-(?P<expected>pass|fail)(?:-[a-z0-9]+)?\.pdf$"
        ))
        .unwrap(),
        twg: Regex::new(
            r"(?i)^TWG test suite (?P<id>[a-z0-9]+)-(?P<profile>[a-z0-9]+)-(?P<expected>pass|fail)(?:-[a-z0-9]+)?\.pdf$",
        )
        .unwrap(),
        // PASS form: feature is alphabetic, flavour is a part+level token (2b/2u/2a/3b/3a/ua1).
        prismpdf_pass: Regex::new(
            r"(?i)^prismpdf-(?P<feature>[a-z]+)-(?P<flavour>[a-z0-9]+)-(?P<expected>pass)\.pdf$",
        )
        .unwrap(),
    })
}

fn expected_of(caps: &regex::Captures<'_>) -> Option<Expected> {
    match &caps["expected"].to_ascii_lowercase()[..] {
        "pass" => Some(Expected::Pass),
        "fail" => Some(Expected::Fail),
        _ => None, // unreachable: the alternation only admits pass|fail
    }
}

/// Parse a leaf filename into a [`CaseSpec`], or `None` if it matches no convention.
fn parse_filename(name: &str) -> Option<CaseSpec> {
    let p = patterns();

    // The four ISO-clause/topic shapes share an extractor; only the corpus tag differs.
    for (corpus, re) in [
        (Corpus::Isartor, &p.isartor),
        (Corpus::Bfo, &p.bfo),
        (Corpus::VeraPdf, &p.verapdf),
        (Corpus::VeraPdf, &p.verapdf_bare),
    ] {
        if let Some(caps) = re.captures(name) {
            return Some(CaseSpec {
                corpus,
                clause: caps["clause"].to_string(),
                topic: caps["topic"].to_string(),
                expected: expected_of(&caps)?,
            });
        }
    }

    // TWG files carry no ISO clause/topic — group them by conformance profile, keyed by test id.
    if let Some(caps) = p.twg.captures(name) {
        return Some(CaseSpec {
            corpus: Corpus::VeraPdf,
            clause: caps["profile"].to_ascii_lowercase(),
            topic: caps["id"].to_ascii_uppercase(),
            expected: expected_of(&caps)?,
        });
    }

    // Prism PDF PASS files: a conformant file spans many clauses, so group by flavour, feature as
    // the topic (like TWG by profile/id).
    if let Some(caps) = p.prismpdf_pass.captures(name) {
        return Some(CaseSpec {
            corpus: Corpus::PrismPdf,
            clause: caps["flavour"].to_ascii_lowercase(),
            topic: caps["feature"].to_ascii_lowercase(),
            expected: expected_of(&caps)?,
        });
    }

    None
}

// ---------------------------------------------------------------------------
// Validation (real `prismpdf::Document` parse) + grading
// ---------------------------------------------------------------------------

/// Error surfaced when validation could not produce a verdict at all (vs. a clean pass/fail).
#[derive(Debug)]
enum ValidationError {
    /// The bytes could not be read off disk.
    Io(std::io::Error),
    /// The validator could not run to a verdict (parse failure, internal limit, …).
    Unverifiable(String),
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ValidationError::Io(e) => write!(f, "io error: {e}"),
            ValidationError::Unverifiable(m) => write!(f, "unverifiable: {m}"),
        }
    }
}

impl std::error::Error for ValidationError {}

/// Run Prism PDF over `path` and report whether it accepts the file.
///
/// IMPORTANT — this is a **parse-survival proxy, not PDF/A conformance**. The engine ships PDF/A
/// *production* (`make_pdfa`) only; it has no conformance validator yet, so the strongest honest
/// signal the public API can give is "does `prismpdf::Document` open and read this file". That is
/// what we call here:
///   - `Ok(true)`  — Prism PDF opened it and could count its pages (parse + recovery survived).
///   - `Err(..)`   — Prism PDF could not reach a page count (a genuine reader gap), or it panicked.
///
/// Because a clean parse maps to `Ok(true)` ("conformant"), every expected-`fail` file the engine
/// merely *parses* shows up as a false positive in the report. That is the intended reading: the
/// false-positive count measures how much PDF/A rule-checking is still unimplemented — it is not a
/// bug in the harness. When `pdf-standards` grows a real validator, swap the `Ok(true)` arm for it
/// (e.g. `prismpdf::validate_pdfa(&doc, PdfAConformance::A2u).map(|r| r.is_conformant())`).
///
/// The call is wrapped in `catch_unwind` so a single malformed file that trips a panic is recorded
/// as `Errored` and the sweep continues over the rest of the corpus instead of aborting.
fn validate_pdf(path: &Path) -> Result<bool, ValidationError> {
    let bytes = std::fs::read(path).map_err(ValidationError::Io)?;

    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let doc = prismpdf::Document::open(bytes).map_err(|e| format!("open: {e:?}"))?;
        doc.page_count().map_err(|e| format!("page_count: {e:?}"))?;
        Ok::<(), String>(())
    }));

    match outcome {
        Ok(Ok(())) => Ok(true),
        Ok(Err(msg)) => Err(ValidationError::Unverifiable(msg)),
        Err(_) => Err(ValidationError::Unverifiable("panicked".into())),
    }
}

/// The outcome of comparing the validator's verdict to the filename's expectation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Grade {
    /// Verdict matched the expectation.
    Pass,
    /// Expected fail, validator passed it — a conformance hole.
    FalsePositive,
    /// Expected pass, validator rejected it — over-strict.
    FalseNegative,
    /// Validation could not reach a verdict.
    Errored,
}

/// Does the filename's expectation mean "a validator must accept this file"?
fn conformant(expected: Expected) -> bool {
    matches!(expected, Expected::Pass)
}

/// Grade Prism PDF's `verdict` against `truth` (the conformant/non-conformant ground truth — the
/// veraPDF oracle when present, otherwise the filename label).
fn grade(truth: bool, verdict: &Result<bool, ValidationError>) -> Grade {
    match verdict {
        Err(_) => Grade::Errored,
        Ok(v) => match (truth, *v) {
            (true, true) | (false, false) => Grade::Pass,
            (false, true) => Grade::FalsePositive,
            (true, false) => Grade::FalseNegative,
        },
    }
}

// ---------------------------------------------------------------------------
// veraPDF oracle — the reference PDF/A validator (installed via the devcontainer)
// ---------------------------------------------------------------------------

/// Locate the veraPDF CLI: `$VERAPDF_BIN` if set, else `verapdf` on `PATH`. `None` disables the
/// oracle (and the harness falls back to grading against the filename label).
fn verapdf_bin() -> Option<PathBuf> {
    if std::env::var_os("PRISMPDF_NO_ORACLE").is_some() {
        return None;
    }
    if let Some(p) = std::env::var_os("VERAPDF_BIN") {
        let p = PathBuf::from(p);
        return p.is_file().then_some(p);
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join("verapdf"))
        .find(|p| p.is_file())
}

/// Run veraPDF over `files` and return its per-file `isCompliant` verdict, aligned by index.
/// `Some(true/false)` is the reference verdict; `None` means veraPDF reached no verdict (couldn't
/// parse the file, or the batch result didn't line up). veraPDF auto-detects each file's PDF/A
/// flavour from its XMP. Files are batched so the JVM starts once per ~500 files, not once per file.
fn oracle_verdicts(bin: &Path, files: &[&Path]) -> Vec<Option<bool>> {
    let mut out = Vec::with_capacity(files.len());
    for chunk in files.chunks(500) {
        // veraPDF exits non-zero when any file is non-compliant — that's expected, so we read
        // stdout regardless of status and never treat it as a run failure.
        let verdicts = Command::new(bin)
            .args(chunk)
            .output()
            .ok()
            .map(|o| parse_oracle_report(&String::from_utf8_lossy(&o.stdout)));

        match verdicts {
            // Aligned 1:1 with the chunk — trust the per-file verdicts.
            Some(v) if v.len() == chunk.len() => out.extend(v),
            // veraPDF crashed or the job count didn't match: don't risk misaligning, mark unknown.
            _ => out.extend(std::iter::repeat_n(None, chunk.len())),
        }
    }
    out
}

/// Parse a veraPDF batch report into one verdict per `<job>`, in document (= input) order.
/// `Some(true/false)` from the job's `isCompliant`; `None` when a job carries no verdict (veraPDF
/// couldn't validate that file).
fn parse_oracle_report(xml: &str) -> Vec<Option<bool>> {
    // `(?s)` lets `.` cross newlines so a whole <job> is one match; within it the first
    // `isCompliant="…"` is veraPDF's verdict for that file.
    static RE: OnceLock<(Regex, Regex)> = OnceLock::new();
    let (job, compliant) = RE.get_or_init(|| {
        (
            Regex::new(r"(?s)<job>.*?</job>").unwrap(),
            Regex::new(r#"isCompliant="(true|false)""#).unwrap(),
        )
    });
    job.find_iter(xml)
        .map(|m| compliant.captures(m.as_str()).map(|c| &c[1] == "true"))
        .collect()
}

// ---------------------------------------------------------------------------
// Report model (serde) — grouped by corpus, then by ISO clause
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Serialize)]
struct Tally {
    total: u32,
    passed: u32,
    false_positives: u32,
    false_negatives: u32,
    errored: u32,
}

impl Tally {
    fn record(&mut self, g: Grade) {
        self.total += 1;
        match g {
            Grade::Pass => self.passed += 1,
            Grade::FalsePositive => self.false_positives += 1,
            Grade::FalseNegative => self.false_negatives += 1,
            Grade::Errored => self.errored += 1,
        }
    }
}

#[derive(Debug, Serialize)]
struct CaseResult {
    file: String,
    clause: String,
    topic: String,
    /// The suite's declared expectation (from the filename).
    expected: Expected,
    /// veraPDF's reference verdict (`isCompliant`): `Some(true/false)`, or `None` when the oracle
    /// is unavailable or reached no verdict. This is the ground truth `grade` is judged against
    /// when present.
    oracle: Option<bool>,
    /// Prism PDF's verdict: `Some(true/false)`, or `None` when validation errored.
    actual: Option<bool>,
    grade: Grade,
}

/// Oracle-health summary: how veraPDF's verdicts line up with the suite's filename labels. This
/// gauges trust in the oracle — large `disagree_with_label` usually means a flavour/version nuance,
/// not a Prism PDF bug.
#[derive(Debug, Default, Serialize)]
struct OracleSummary {
    /// Was the veraPDF CLI found and used?
    available: bool,
    /// Files for which veraPDF returned a clear compliant/non-compliant verdict.
    verdicts: u32,
    /// Files veraPDF was asked about but reached no verdict (unparseable, or batch misaligned).
    unknown: u32,
    agree_with_label: u32,
    disagree_with_label: u32,
    /// A capped sample of disagreements, for eyeballing (`file: veraPDF=… label=…`).
    disagreements: Vec<String>,
}

#[derive(Debug, Default, Serialize)]
struct ClauseReport {
    tally: Tally,
    cases: Vec<CaseResult>,
}

#[derive(Debug, Default, Serialize)]
struct CorpusReport {
    tally: Tally,
    /// Keyed by ISO clause; `BTreeMap` keeps the report stable and clause-sorted.
    clauses: BTreeMap<String, ClauseReport>,
}

#[derive(Debug, Default, Serialize)]
struct Report {
    tally: Tally,
    /// How the veraPDF oracle's verdicts compare to the suite labels (oracle health).
    oracle: OracleSummary,
    /// Files found on disk whose names matched no convention (named, not silently dropped).
    unparsed: Vec<String>,
    corpora: BTreeMap<String, CorpusReport>,
}

impl Report {
    /// Record one graded file. `oracle` is veraPDF's verdict (`None` when unavailable/undecided);
    /// when present it is the ground truth `grade` is judged against, otherwise the filename label
    /// is used. Either way the oracle/label agreement is tracked in [`OracleSummary`].
    fn record(
        &mut self,
        spec: &CaseSpec,
        file: String,
        verdict: Result<bool, ValidationError>,
        oracle: Option<bool>,
    ) {
        let label = conformant(spec.expected);
        let truth = oracle.unwrap_or(label);
        let g = grade(truth, &verdict);
        self.tally.record(g);

        // Oracle health: does veraPDF agree with the suite's filename label?
        match oracle {
            Some(o) if o == label => self.oracle.agree_with_label += 1,
            Some(o) => {
                self.oracle.disagree_with_label += 1;
                if self.oracle.disagreements.len() < 50 {
                    self.oracle
                        .disagreements
                        .push(format!("{file}: veraPDF={o} label={label}"));
                }
            }
            None if self.oracle.available => self.oracle.unknown += 1,
            None => {}
        }
        if oracle.is_some() {
            self.oracle.verdicts += 1;
        }

        let corpus = self.corpora.entry(spec.corpus.to_string()).or_default();
        corpus.tally.record(g);

        let clause = corpus.clauses.entry(spec.clause.clone()).or_default();
        clause.tally.record(g);
        clause.cases.push(CaseResult {
            file,
            clause: spec.clause.clone(),
            topic: spec.topic.clone(),
            expected: spec.expected,
            oracle,
            actual: verdict.ok(),
            grade: g,
        });
    }
}

// ---------------------------------------------------------------------------
// Corpus discovery
// ---------------------------------------------------------------------------

/// Resolve the corpus roots to walk. Discovery is by *filename*, so subdirectory names don't matter.
///
/// - `$PRISMPDF_CONFORMANCE_CORPUS`, when set, is the **sole** root (full override) — point it at a
///   single suite or at `corpus/` to walk everything.
/// - Otherwise the default is the committed, Prism PDF-authored `corpus/prismpdf-pdfa/` (always
///   present) **plus** the in-tree `corpus/external/` (the fetched third-party suites) when it
///   exists. So the committed PASS corpus is graded even when the external suites aren't fetched.
///
/// Returns the roots that exist; empty when none do, so the walking test can skip.
fn corpus_roots() -> Vec<PathBuf> {
    if let Some(dir) = std::env::var_os("PRISMPDF_CONFORMANCE_CORPUS") {
        let dir = PathBuf::from(dir);
        return if dir.is_dir() { vec![dir] } else { Vec::new() };
    }
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    [
        manifest.join("../../corpus/prismpdf-pdfa"),
        manifest.join("../../corpus/external"),
    ]
    .into_iter()
    .filter(|d| d.is_dir())
    .collect()
}

// ---------------------------------------------------------------------------
// The harness test
// ---------------------------------------------------------------------------

/// Flip to `true` once `validate_pdf` is wired to the real validator to make this a hard gate.
const ASSERT_NO_REGRESSIONS: bool = false;

#[test]
fn conformance_corpus() {
    let roots = corpus_roots();
    if roots.is_empty() {
        eprintln!(
            "skip: PDF/A conformance corpus not present. Set PRISMPDF_CONFORMANCE_CORPUS or \
             populate corpus/external/ with the fetched suites (see test docs)."
        );
        return;
    }

    // Skip dot-directories (notably the suites' `.git`) — they hold no test PDFs and are huge.
    let is_hidden =
        |e: &walkdir::DirEntry| e.depth() > 0 && e.file_name().to_string_lossy().starts_with('.');

    // Pass 1: walk every root, splitting files into graded cases and unrecognised names.
    let mut report = Report::default();
    let mut graded: Vec<(PathBuf, String, CaseSpec)> = Vec::new();
    for root in &roots {
        for entry in WalkDir::new(root)
            .into_iter()
            .filter_entry(|e| !is_hidden(e))
            .filter_map(Result::ok)
        {
            let path = entry.path();
            if !path.is_file()
                || path
                    .extension()
                    .is_none_or(|e| !e.eq_ignore_ascii_case("pdf"))
            {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            match parse_filename(&name) {
                Some(spec) => graded.push((path.to_path_buf(), name, spec)),
                None => report.unparsed.push(name),
            }
        }
    }

    // Pass 2: ask the veraPDF oracle (one JVM per ~500 files) for the reference verdicts, aligned
    // by index with `graded`. Absent CLI → all `None`, and grading falls back to the filename label.
    let oracle = verapdf_bin();
    report.oracle.available = oracle.is_some();
    match &oracle {
        Some(bin) => eprintln!("conformance: oracle = veraPDF ({})", bin.display()),
        None => eprintln!(
            "conformance: oracle = none (verapdf not on PATH / PRISMPDF_NO_ORACLE set) — \
             grading against filename labels"
        ),
    }
    let oracle_verdicts = match &oracle {
        Some(bin) => {
            let paths: Vec<&Path> = graded.iter().map(|(p, ..)| p.as_path()).collect();
            oracle_verdicts(bin, &paths)
        }
        None => vec![None; graded.len()],
    };

    // Pass 3: run Prism PDF per file and grade against the oracle (or the label when absent).
    for ((path, name, spec), oracle_verdict) in graded.iter().zip(oracle_verdicts) {
        let verdict = validate_pdf(path);
        report.record(spec, name.clone(), verdict, oracle_verdict);
    }

    // Emit the grouped JSON report next to the build artefacts.
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/conformance-report.json");
    let json = serde_json::to_string_pretty(&report).expect("serialize report");
    if let Some(parent) = out.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::write(&out, &json).expect("write report");

    let t = &report.tally;
    let truth = if report.oracle.available {
        "veraPDF"
    } else {
        "labels"
    };
    eprintln!(
        "conformance: {} cases across {} corpora vs {truth} — {} ok, {} false-pos, {} false-neg, \
         {} errored ({} unparsed). Report: {}",
        t.total,
        report.corpora.len(),
        t.passed,
        t.false_positives,
        t.false_negatives,
        t.errored,
        report.unparsed.len(),
        out.display(),
    );
    if report.oracle.available {
        let o = &report.oracle;
        eprintln!(
            "conformance: oracle health — {} verdicts, {} agree with label, {} disagree, {} undecided",
            o.verdicts, o.agree_with_label, o.disagree_with_label, o.unknown,
        );
    }

    assert!(
        t.total > 0,
        "corpus present but no recognised test files found under {roots:?}"
    );

    if ASSERT_NO_REGRESSIONS {
        assert_eq!(
            t.false_positives, 0,
            "validator accepted non-conformant files (see report)"
        );
        assert_eq!(
            t.false_negatives, 0,
            "validator rejected conformant files (see report)"
        );
        assert_eq!(
            t.errored, 0,
            "validator failed to reach a verdict on some files (see report)"
        );
    }
}

// ---------------------------------------------------------------------------
// Deterministic unit tests for the parser + grader (run with no corpus on disk)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_isartor_with_instance() {
        let spec = parse_filename("isartor-6-7-8-t02-fail-a.pdf").expect("isartor");
        assert_eq!(spec.corpus, Corpus::Isartor);
        assert_eq!(spec.clause, "6-7-8");
        assert_eq!(spec.topic, "02");
        assert_eq!(spec.expected, Expected::Fail);
    }

    #[test]
    fn parses_bfo_clause_not_creator() {
        let spec = parse_filename("pdfa2-6-1-3-bfo-t01-fail.pdf").expect("bfo");
        assert_eq!(spec.corpus, Corpus::Bfo);
        // The numeric clause must stop before the `bfo` creator token.
        assert_eq!(spec.clause, "6-1-3");
        assert_eq!(spec.topic, "01");
        assert_eq!(spec.expected, Expected::Fail);
    }

    #[test]
    fn parses_verapdf_long_clause() {
        let spec = parse_filename("veraPDF test suite 6-2-11-7-3-t01-fail-e.pdf").expect("verapdf");
        assert_eq!(spec.corpus, Corpus::VeraPdf);
        assert_eq!(spec.clause, "6-2-11-7-3");
        assert_eq!(spec.topic, "01");
        assert_eq!(spec.expected, Expected::Fail);
    }

    #[test]
    fn parses_verapdf_bare_dotted_clause() {
        // veraPDF-corpus files without the `veraPDF test suite ` prefix, with dotted clauses.
        let spec = parse_filename("7.1-t03-pass-b.pdf").expect("bare verapdf");
        assert_eq!(spec.corpus, Corpus::VeraPdf);
        assert_eq!(spec.clause, "7.1");
        assert_eq!(spec.topic, "03");
        assert_eq!(spec.expected, Expected::Pass);

        let mixed = parse_filename("7-18.3-t01-pass-a.pdf").expect("mixed separators");
        assert_eq!(mixed.clause, "7-18.3");
    }

    #[test]
    fn parses_twg_by_profile_and_id() {
        let spec = parse_filename("TWG test suite A007-pdfa2-fail-a.pdf").expect("twg");
        assert_eq!(spec.corpus, Corpus::VeraPdf);
        // TWG has no ISO clause/topic: grouped by profile, keyed by test id.
        assert_eq!(spec.clause, "pdfa2");
        assert_eq!(spec.topic, "A007");
        assert_eq!(spec.expected, Expected::Fail);
    }

    #[test]
    fn parses_prismpdf_pass_by_flavour_and_feature() {
        // PASS files are keyed by flavour (clause) with the feature as the topic.
        let spec = parse_filename("prismpdf-text-2u-pass.pdf").expect("prismpdf pass");
        assert_eq!(spec.corpus, Corpus::PrismPdf);
        assert_eq!(spec.clause, "2u");
        assert_eq!(spec.topic, "text");
        assert_eq!(spec.expected, Expected::Pass);

        // PDF/UA-1 sample: flavour token carries a digit; feature stays alphabetic.
        let ua = parse_filename("prismpdf-accessible-ua1-pass.pdf").expect("prismpdf ua1");
        assert_eq!(ua.clause, "ua1");
        assert_eq!(ua.topic, "accessible");
    }

    #[test]
    fn rejects_malformed_prismpdf_names() {
        // The corpus is PASS-only with the feature/flavour shape; anything else is unrecognised.
        assert!(parse_filename("prismpdf-text-2u-fail.pdf").is_none()); // no FAIL corpus
        assert!(parse_filename("prismpdf-6-2-2-t01-pass.pdf").is_none()); // feature must be alphabetic
        assert!(parse_filename("prismpdf-blank.pdf").is_none()); // missing flavour + expectation
    }

    #[test]
    fn parses_pass_cases_and_is_case_insensitive() {
        assert_eq!(
            parse_filename("isartor-6-1-2-t01-pass-b.pdf")
                .unwrap()
                .expected,
            Expected::Pass
        );
        assert_eq!(
            parse_filename("PDFA2-6-1-3-BFO-T01-PASS.PDF")
                .unwrap()
                .expected,
            Expected::Pass
        );
    }

    #[test]
    fn rejects_unrecognised_names() {
        assert!(parse_filename("random.pdf").is_none());
        assert!(parse_filename("isartor-6-7-8-t02.pdf").is_none()); // no expected token
        assert!(parse_filename("isartor-6-7-8-t02-maybe-a.pdf").is_none()); // not pass|fail
        assert!(parse_filename("isartor-6-7-8-t02-fail-a.txt").is_none()); // wrong extension
    }

    #[test]
    fn grading_truth_table() {
        // `truth` is the ground-truth "is conformant" bool (oracle if present, else label).
        // truth conformant + verdict conformant -> Pass
        assert_eq!(grade(true, &Ok(true)), Grade::Pass);
        // truth non-conformant + verdict non-conformant -> Pass
        assert_eq!(grade(false, &Ok(false)), Grade::Pass);
        // truth non-conformant + verdict conformant -> false positive
        assert_eq!(grade(false, &Ok(true)), Grade::FalsePositive);
        // truth conformant + verdict non-conformant -> false negative
        assert_eq!(grade(true, &Ok(false)), Grade::FalseNegative);
        // any error -> Errored
        let err: Result<bool, ValidationError> = Err(ValidationError::Unverifiable("boom".into()));
        assert_eq!(grade(true, &err), Grade::Errored);

        // `conformant` maps the filename label onto that bool.
        assert!(conformant(Expected::Pass));
        assert!(!conformant(Expected::Fail));
    }

    #[test]
    fn report_groups_by_corpus_and_clause() {
        let mut report = Report::default();
        let spec = parse_filename("isartor-6-7-8-t02-fail-a.pdf").unwrap();
        // Force a false positive: label says fail, Prism PDF says "pass" (parsed), no oracle.
        report.record(&spec, "isartor-6-7-8-t02-fail-a.pdf".into(), Ok(true), None);

        assert_eq!(report.tally.total, 1);
        assert_eq!(report.tally.false_positives, 1);
        let corpus = &report.corpora["isartor"];
        let clause = &corpus.clauses["6-7-8"];
        assert_eq!(clause.tally.false_positives, 1);
        assert_eq!(clause.cases[0].grade, Grade::FalsePositive);
        assert_eq!(clause.cases[0].actual, Some(true));
        assert_eq!(clause.cases[0].oracle, None);

        // The whole report serialises cleanly.
        let json = serde_json::to_string(&report).unwrap();
        assert!(json.contains("\"false_positives\""));
    }

    #[test]
    fn oracle_is_ground_truth_over_label() {
        // Label says fail, but the veraPDF oracle says compliant=true and Prism PDF parsed it (true).
        // Graded against the oracle, that's a Pass — and the oracle/label disagreement is tracked.
        let mut report = Report::default();
        report.oracle.available = true;
        let spec = parse_filename("isartor-6-7-8-t02-fail-a.pdf").unwrap();
        report.record(&spec, "f.pdf".into(), Ok(true), Some(true));

        assert_eq!(report.tally.false_positives, 0);
        assert_eq!(report.tally.passed, 1);
        assert_eq!(report.oracle.verdicts, 1);
        assert_eq!(report.oracle.disagree_with_label, 1);
        assert_eq!(report.oracle.agree_with_label, 0);
        assert_eq!(
            report.corpora["isartor"].clauses["6-7-8"].cases[0].oracle,
            Some(true)
        );
    }

    #[test]
    fn parses_verapdf_batch_report() {
        // Two <job>s in order: one non-compliant, one compliant; plus a job with no verdict.
        let xml = r#"
            <report>
              <job><item><name>a.pdf</name></item>
                <validationReport isCompliant="false">x</validationReport></job>
              <job><item><name>b.pdf</name></item>
                <validationReport isCompliant="true">y</validationReport></job>
              <job><item><name>c.pdf</name></item><taskException>boom</taskException></job>
            </report>"#;
        assert_eq!(
            parse_oracle_report(xml),
            vec![Some(false), Some(true), None]
        );
    }
}
