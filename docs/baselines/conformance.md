# PDF/A conformance-corpus baseline

Baseline numbers for the data-driven conformance harness
(`crates/pdf/tests/conformance_corpus.rs`). The harness walks the public PDF/A validation-test
suites (Isartor / BFO / veraPDF), parses the expectation encoded in each filename, runs Prism PDF
over the file, and grades the verdict against the **ground truth** — the veraPDF oracle when
present, otherwise the filename label.

This test is **run manually, occasionally**, to check for regressions or improvements — it is not
in the default CI gate (it needs the third-party corpus, which is not committed). Re-run it and
compare against the table below; update this file when the numbers move and you understand why.

## How to run

```bash
# By default the harness walks the committed corpus/prismpdf-pdfa/ (our own PASS corpus) PLUS
# corpus/external/ (the fetched third-party suites) when present. Just run:
cargo test -p prismpdf --test conformance_corpus conformance_corpus -- --exact --nocapture
# Or override the roots entirely (sole root when set; use an absolute path):
PRISMPDF_CONFORMANCE_CORPUS="$PWD/corpus/external" \
  cargo test -p prismpdf --test conformance_corpus conformance_corpus -- --exact --nocapture
```

The full grouped report (by corpus → ISO clause, with per-level tallies, plus an `oracle` health
block and per-case `oracle`/`actual` verdicts) is written to `target/conformance-report.json`.

### The veraPDF oracle

When the **veraPDF CLI** is on `PATH` (the devcontainer's `post-install.sh` installs it), the
harness grades Prism PDF against veraPDF's authoritative `isCompliant` verdict — the reference PDF/A
validator, flavour-aware per file — instead of the filename label. It runs batched (one JVM per
~500 files, ~20s for the full corpus). Controls:

- `VERAPDF_BIN=/path/to/verapdf` — use a specific binary.
- `PRISMPDF_NO_ORACLE=1` — disable the oracle and grade against filename labels (the fallback when
  veraPDF isn't installed). Numbers are ~identical since the labels and veraPDF nearly always agree.

## What the numbers mean (read before comparing)

Prism PDF has **no PDF/A conformance validator yet** — it ships PDF/A *production* (`make_pdfa`)
only. So `validate_pdf` calls the real public API (`pdf::Document::open` + `page_count`) and treats
a clean parse as "pass". This is a **parse-survival proxy, not conformance**. Therefore:

- **`errored`** — files Prism PDF cannot open/parse at all. These are genuine reader gaps (or panics).
  **This is the number to watch: it should stay 0 (or only go down).**
- **`false_negatives`** — conformant files Prism PDF fails to parse. Also a real reader bug. Should
  stay 0.
- **`false_positives`** — non-conformant files (per the oracle/label) Prism PDF nonetheless parses.
  With no validator, this is *expected* and large; it measures how much ISO 19005 rule-checking is
  still unimplemented. It will only drop once a real validator is wired into `validate_pdf` (then
  flip `ASSERT_NO_REGRESSIONS` to make the test a gate).
- **`oracle` block** — `agree_with_label` vs `disagree_with_label` gauges trust in the oracle. A
  handful of disagreements is normal (the corpus is versioned; a few expected-results drift as
  veraPDF's validation profiles evolve); a spike means a veraPDF version/flavour change, not a
  Prism PDF regression.
- **`unparsed`** — filenames matching no convention (manuals, veraPDF's `undefined` category, TWG
  id-ranges). Excluded from grading, listed in the report.

## Baseline — 2026-06-22 (oracle: veraPDF 1.30.2)

Corpus: the committed `corpus/prismpdf-pdfa/` (our 30 PASS files) + `corpus/external/{Isartor
testsuite, pdfa-testsuite, veraPDF-corpus}` (plus `pdf20examples`, whose names don't match any
convention → `unparsed`). Graded against the veraPDF oracle. Runtime ~21s, debug build.

| Metric | Value |
| --- | --- |
| Total graded cases | 3159 |
| `passed` (ok) | 1114 |
| `false_positives` | 2045 |
| `false_negatives` | **0** |
| `errored` | **0** |
| `unparsed` (not graded) | 16 |

Oracle health: **3129** verdicts, **0** undecided, **3125** agree with label, **4** disagree (all
`veraPDF=compliant / label=fail` — veraPDF-corpus files 1.30.2 now accepts: `6-7-3-t01-fail-b`,
`6-8-3-3-t01-fail-a/-b`, `6-1-3-t04-fail-b`). Those 4 are why the oracle run shows 1084/2045 vs the
label-only 1080/2049 — grading flips with the reference verdict.

Per-corpus (oracle-graded):

| Corpus | Total | passed | false_pos | false_neg | errored | clauses |
| --- | --- | --- | --- | --- | --- | --- |
| prismpdf | 30 | 30 | 0 | 0 | 0 | 6 |
| isartor | 408 | 0 | 408 | 0 | 0 | 40 |
| bfo | 33 | 9 | 24 | 0 | 0 | 12 |
| verapdf | 2688 | 1075 | 1613 | 0 | 0 | 164 |

The `prismpdf` corpus is our own committed PASS files (`corpus/prismpdf-pdfa/`): the full
feature×flavour matrix `make_pdfa`/`make_pdfua` can produce — all 30 validate as `isCompliant`
under the flavour veraPDF auto-detects (every one of 2b/2u/2a/3b/3a, plus ua1), proving the
producer end-to-end across every accept path. Its "clauses" column is the flavour grouping, not
ISO clauses. It is **PASS-only** (there is no FAIL corpus — broken files test a validator Prism PDF
lacks). The 60 `VAC` rules in `corpus/prismpdf-pdfa/checklist.md` govern objects the producer does
not author yet; `corpus/prismpdf-pdfa/coverage-gaps.md` maps each to its spec-map status.

Headline: **0 errored, 0 false-negatives** — Prism PDF's recovery opened every one of the 3129 files
(including 2000+ non-conformant ones) without a single parse failure or panic. The 2045 false
positives are the conformance-checking gap (now measured against the veraPDF reference), not a
reader defect.

The 16 `unparsed` files are all legitimately ungradeable: the Isartor manual (×2), veraPDF's
`-undefined-` expectation category (×10), and the `TWG test suite A011-A015-...` id-range variant
(×4).
