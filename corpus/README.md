# corpus/ — test PDFs

The versioned corpus of real PDFs that the test suite (and fuzzer seeds) run against. Per
`DESIGN.md` §7, a corpus of valid **and** broken **and** edge-case files is how we tell a toy
library from a production one — recovery and hostile-input handling can only be proven against
real bytes.

## Layout

| Dir | Holds | Used for |
|---|---|---|
| `valid/` | Well-formed PDFs (various producers, versions 1.0–2.0). | Round-trip: `load → save → load` must be structurally equal; metadata/page-count assertions. |
| `malformed/` | Broken files: corrupt/missing xref, wrong `/Length`, truncated, garbage trailer. | Recovery mode (xref rebuild by scanning) must open them; must never panic. |
| `edge/` | Legal-but-unusual: indirect `/Length`, object streams, deep nesting, huge dicts, weird names/encodings. | Stress the COS model decisions (ADR-0001–0004, in `crates/pdf-cos/src/lib.rs`) and the anti-DoS limits. |
| `prismpdf-pdfa/` | Our own PDF/A corpus: `make_pdfa` PASS files (committed) + a FAIL backlog. | PDF/A conformance graded by the veraPDF oracle — see its `README.md`. |
| `external/` | Fetched third-party PDF/A suites (Isartor/BFO/veraPDF). **Not committed** (licences). | Conformance harness input; populate locally or via `PRISMPDF_CONFORMANCE_CORPUS`. |

## What is tracked vs. local

Only the curated fixtures (`valid/`, `malformed/`, `edge/`, `prismpdf-pdfa/`) are committed, and
they are deliberately tiny. `external/` is populated locally (gitignored), and the fuzzer's
generated state lives under `fuzz/corpus/` and `fuzz/target/` (also gitignored) — those
directories can grow to gigabytes on a working machine without adding a byte to the repository.
Deleting them only discards local downloads and fuzzing progress; see `fuzz/README.md`.

## Rules

- **Only commit redistributable files.** Prefer files you generated, public-domain samples, or
  the `pdf-association/pdf20examples` set (check their license). Do not commit copyrighted PDFs.
- **Keep them small.** A few KB that reproduces the case beats a 10 MB real-world document. Large
  or sensitive inputs belong in the fuzz corpus (gitignored), not here.
- **Name by what they exercise**, e.g. `xref-stream-objstm.pdf`, `length-indirect.pdf`,
  `truncated-trailer.pdf`. A one-line note per tricky file in this README is welcome.

A malformed file that makes Prism PDF **crash, hang, or OOM** is a security report, not just a test
case — see `SECURITY.md`.

## Generated fixtures

Most files here are produced deterministically by a byte-level generator so the corpus is
reproducible and reviewable (raw bytes, not writer output — each one targets a specific reader
path). Regenerate with:

```text
cargo run -p prismpdf --example gen_corpus
```

The directory-driven round-trip test lives in `crates/pdf/tests/corpus.rs`: it opens every file,
and for `valid/` + `edge/` asserts `load → save → load` (via both `save` and the compact
xref-stream `save_compact`) preserves page count and extracted text; for `malformed/` it asserts
recovery opens the file and reports a page count without panicking.

The same generator seeds the fuzzer: the CI `fuzz` job runs `gen_corpus -- /tmp/seeds` and hands the
result to the whole-document targets. That is why the filter coverage above matters twice over — a
decoder no corpus file reaches is a decoder the seeded `document` target never fuzzes.

| File | Exercises |
|---|---|
| `valid/minimal-2page.pdf` | Hand-written two-page classic xref (the original seed). |
| `valid/text-classic-xref.pdf` | Classic xref table + a text content stream (§7.5.4). |
| `valid/two-pages-text.pdf` | Two pages, each with its own content stream. |
| `valid/flate-content.pdf` | `/Filter /FlateDecode` content stream (§7.4.4). |
| `valid/xref-stream.pdf` | Cross-reference **stream** instead of a table (§7.5.8). |
| `valid/objstm.pdf` | Catalog/pages/page packed in an **object stream** + xref stream (§7.5.7). |
| `valid/lzw-content.pdf` | `/Filter /LZWDecode` content stream (§7.4.4.2). |
| `valid/ascii-chain-content.pdf` | A `/Filter` **array**: `ASCII85Decode` then `FlateDecode` (§7.4, §7.4.3). |
| `valid/runlength-image.pdf` | `/Filter /RunLengthDecode` image XObject, 4×2 RGB (§7.4.5). |
| `valid/ccitt-image.pdf` | `/Filter /CCITTFaxDecode` image XObject, G3 1D, two all-white rows (§7.4.6). |
| `edge/length-indirect.pdf` | Stream `/Length` given as an indirect reference (§7.3.8.2). |
| `edge/nested-page-tree.pdf` | Intermediate `/Pages` node; page-tree recursion (§7.7.3). |
| `edge/leading-comments.pdf` | Comments and binary marker interleaved with objects (§7.2.4). |
| `malformed/missing-startxref.pdf` | No xref/trailer/`startxref` — rebuild by scanning objects. |
| `malformed/bad-startxref.pdf` | `startxref` points to a bogus offset — rebuild. |
| `malformed/wrong-length.pdf` | Stream `/Length` far too small — scan to `endstream`. |
| `malformed/truncated-trailer.pdf` | File cut off inside the xref section — rebuild. |
| `malformed/garbage-prefix.pdf` | Junk bytes before `%PDF` — header isn't at offset 0. |
