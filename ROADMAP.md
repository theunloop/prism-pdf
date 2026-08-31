# Prism PDF — Roadmap

This file answers one question: **what should happen next?**

- ISO section → crate index: [`docs/spec-map.md`](docs/spec-map.md)
- Released work: [`CHANGELOG.md`](CHANGELOG.md)
- Architecture: [`DESIGN.md`](DESIGN.md)
- The 1.0 gate: [`RELEASING.md`](RELEASING.md)

Delivered work is removed from this file; Git and the changelog keep the history.

## Current focus

The planned non-rendering v1 capability line is implemented and its Rust facade and C ABI have
passed release-candidate validation plus independent downstream exercise across parse, manipulate,
low-level create, and compose. No capability work remains before the `1.0.0` stability tag. Which
release is current is [`CHANGELOG.md`](CHANGELOG.md)'s answer, not this file's.
Rendering remains outside the v1.x scope.

### Before 1.0

**Cut the stability release.** Publish the validated line, observe its CI, its native artifacts
(`docs/native-artifacts.md`) and documentation deployment, then follow `RELEASING.md` to tag
`1.0.0`. New correctness or safety findings remain release blockers; long-tail format breadth
below does not.

## Conformance typologies after 1.0 — every remaining part

The goal is a crate cell in every row of the spec map's
[typology index](docs/spec-map.md#conformance-typologies--iso-family--crate): **all remaining PDF
subset standards get implemented**, not just the ones with demonstrated demand.

**PDF/A is work in progress, not done.** All four parts (19005-1…-4) ship and are CI-validated by
veraPDF, but the production gaps from M16 keep it open: a redistributable bundled CMYK profile,
text/choice/radio field appearances, and broader per-rule coverage for parts 1 and 4.

The unowned parts, in dependency order:

1. **PDF/X-6** (ISO 15930-9; X-6, X-6n, X-6p) — on the existing PDF 2.0 OutputIntent/XMP
   machinery.
2. **PDF/VT-3** (ISO 16612-3) — layered on X-6.
3. **PDF/R-1** (ISO 23504-1) — raster scans, ISO 32000-1 subset base.
4. **PDF/X-4/X-4p** (ISO 15930-7) and **PDF/X-5** (ISO 15930-8; X-5g, X-5n, X-5pg) — PDF 1.6
   base.
5. **Legacy PDF/X** (ISO 15930-1…-6; X-1a, X-2, X-3) — PDF 1.3/1.4 bases.
6. **PDF/VT-1/VT-2** (ISO 16612-2) — over PDF/X-4/-5, so last.

PDF/E stays out of scope (see below): ISO 24517 has no part 2 and its profile continued as
PDF/A-4e, which is already implemented — the hole is covered by its successor.

## AI-native features after 1.0

A new capability line: make Prism PDF the extraction engine LLM pipelines call directly, instead
of a text dump they post-process. All three build on machinery that already ships — the
`/ToUnicode` text decoding, the preserved structure trees, and the image extraction — and none
requires rendering (out of scope, below). New public API here follows the same FFI-first rule as
everything else, so every binding gets these, not just Rust.

- **Native semantic chunking (RAG).** Extract text as a hierarchy — headings, paragraphs, tables,
  lists — instead of one flat block, and emit JSON or Markdown already split into chunks sized
  for vector databases. Tagged PDFs use the structure tree as the source of truth; untagged ones
  fall back to layout heuristics (font size, weight, position).
- **Layout-aware Markdown export.** Read a PDF and emit clean, structured Markdown that an LLM
  can consume directly — the serialization of the same hierarchical model chunking uses, exposed
  in the facade and as a CLI subcommand.
- **Image extraction for vision models.** The safe decoded-image extraction exists
  (`prismpdf images`); the AI-native part is the in-memory API that hands a multimodal model
  (e.g. a local LLaVA) each image *with its context* — page, position, and the caption or alt
  text the structure tree carries — rather than anonymous files on disk.

## Format breadth after 1.0

These are useful extensions, not claims that the current non-rendering engine is incomplete:

| Area | Remaining work |
|---|---|
| Advanced crypto (M20) | Ed448 and brainpool curves (blocked on compatible upstream crates); SHA-3/SHAKE256 encryption KDF; signature policies; per-name crypt-filter selection; external CMS/PAdES oracle |
| PDF 2.0 structures (M21/M23) | DPart associated files; `/DestOutputProfileRef`; per-object metadata; RichMedia-specific handling |
| Read and font breadth (M24) | Safe JPEG 2000 pixel decoding; OTF/CFF authoring; CIDFontType0/CFF reading; predefined CJK CMaps |
| Tagged editing (M14 follow-up) | Preserve and remap structure when extracting pages instead of deliberately stripping it |
| Robustness | Document-wide decompression budget; OSS-Fuzz; more round-trip stress on complex tagged and AcroForm documents |
| Conformance | PDF Declarations XMP; Info/XMP precedence; ICC 2.0 fields; geospatial `/Measure` and `/PtData`; an internal validator |

Exact section-level gaps and upstream blockers belong in the spec overlays, not here.

## Out of scope

- **PDF/E** (ISO 24517-1, PDF 1.6-based, no part 2): superseded by PDF/A-4e (ISO 19005-4
  Annex B), which is already implemented. No separate PDF/E work is planned.
- **M8 rendering** (§10–§11 and rendering-only graphics interpretation) is outside v1.x. There is no
  `pdf-render` crate; create it only when rendering work begins.

## Maintenance rule

Update this file only when priorities or milestone ownership change. Never name the current
release here — that is a status claim, it rots on the next tag, and `CHANGELOG.md` owns it. Update
the spec map when an ISO capability changes, and the changelog when user-visible work ships.
Granular tasks belong in the issue tracker.
