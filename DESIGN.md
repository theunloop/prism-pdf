# PDF Engine in Rust — Technical Design Document

> Project name: **Prism PDF** (crate/lib: `prismpdf`). Note: near-but-not-identical names exist
> (`limePDF`, a PHP port of TCPDF; `PrimoPDF`); no exact collision is known.
>
> Goal: a PDF engine (read, manipulate, generate) written in Rust, with a stable FFI boundary
> designed in from the start so that idiomatic bindings can be generated for many languages.
> Functional inspiration: iText, IronPDF, Aspose.PDF, PDFBox.

> **This document describes how the engine is built and why.** It carries no status. For *what
> shipped*, read [`CHANGELOG.md`](./CHANGELOG.md); for *what is next*,
> [`ROADMAP.md`](./ROADMAP.md).

---

## 1. Scope and non-goals

### In scope (full vision)
- **Read**: robust, tolerant parsing of real-world PDFs (malformed ones included); extraction of
  metadata, text, images and structure.
- **Manipulation**: merge, split, reorder, rotate, stamping/watermark, form filling, incremental
  update.
- **Generation**: creating PDFs from scratch, from the bottom (operators) to the top (layout).
- **Security**: encryption/decryption, digital signatures.
- **Conformance**: PDF/A, PDF/UA, PDF/X (later phase).
- **Portability**: bindings for Python, JS/WASM, Node, Java/Kotlin, C#, Go, Swift, C/C++.

### Non-goals (at least for v1.x)
- Full **rendering/rasterisation** (page → bitmap): large, and heavy on dependencies. Out of scope
  initially, or delegated to a separate optional crate. See §4 (EPIC 14).
- **HTML→PDF conversion** with a complete CSS layout engine: out of scope for v1.
- Reimplementing base codecs (zlib, JPEG, crypto primitives): mature crates are **reused** instead.

The engine is **clean-room**: it does not wrap or fork an existing Rust PDF library. The cost is
more code; the benefit is full control over the public API and the C ABI, which §6 makes the
central architectural bet.

---

## 2. Reference specifications (inputs to the work)

| Document | What it covers | Source |
|---|---|---|
| ISO 32000-1:2008 (PDF 1.7) | Base spec, still the most widely used reference | Free Adobe copy (`PDF32000_2008.pdf`) |
| ISO 32000-2:2020 (PDF 2.0) | Modern spec + errata + crypto | PDF Association, free (`pdfa.org/sponsored-standards`) |
| ISO/TS 32001 / 32002 | Hash algorithms, digital signatures | PDF Association bundle |
| ISO 19005 (PDF/A-1..4) | Archiving | ISO (paid) / veraPDF docs |
| ISO 14289 (PDF/UA) | Accessibility | ISO |
| Adobe normative refs | CMap, AFM, fonts | `reference.pdfa.org/iso/32000/` |
| pdf20examples | Sample files for 2.0 features | GitHub `pdf-association/pdf20examples` |

The repo keeps a [`docs/spec-map.md`](docs/spec-map.md) indexing every section of the standard
(§7 syntax, §8 graphics, §9 text, §12 interactivity, …) to the crate that owns it. It is an index
into the code, not a status report.

---

## 3. Architectural principles

1. **Layered, one-way dependencies.** Lower layers never know about higher ones.
2. **Lazy & streaming.** Do not load everything into memory: parse objects on demand, support
   large files. Lazy loading goes through the xref.
3. **Error tolerance.** Real PDFs are frequently broken. A *recovery* mode (rebuilding the xref by
   scanning) is a first-class feature, not a fallback. It is what separates a toy library from a
   production one.
4. **Robustness against hostile input.** The parser treats input as untrusted → no panic may
   propagate across the FFI, anti-DoS limits (nesting depth, decompression bombs, reference
   cycles), continuous fuzzing.
5. **FFI-first in the core's design.** The core's public APIs must map onto a C ABI boundary
   without contortions. No exotic Rust types at points destined to become FFI.
6. **Reuse over reimplementation** for codecs, crypto and font parsing.
7. **Zero `unsafe` in the core**; `unsafe` is confined to, and audited in, the FFI crate alone.
8. **Stable, versioned API (SemVer).** The C ABI has its own separate versioning policy.

---

## 4. Layered architecture

```
┌─────────────────────────────────────────────────────────────────────┐
│  BINDINGS (Python, WASM/JS, Node, Java, C#, Go, Swift, C/C++)         │  EPIC 11
├─────────────────────────────────────────────────────────────────────┤
│  FFI / C ABI  (handle-based, no-panic, cbindgen)                      │  EPIC 10
├─────────────────────────────────────────────────────────────────────┤
│  HIGH-LEVEL API / LAYOUT      │  STANDARDS / CONFORMANCE              │  EPIC 12 / 13
│  (builder, paragraphs, tables)│  (PDF/A, PDF/UA, PDF/X, XMP)          │
├───────────────────────────────┴───────────────────────────────────── ┤
│  SUBSYSTEMS                                                            │
│   Fonts & Text │ Graphics & Imaging │ Crypto & Signatures             │  EPIC 7 / 8 / 9
├─────────────────────────────────────────────────────────────────────┤
│  CONTENT STREAMS  (graphics/text operators, graphics state machine)   │  EPIC 6
├─────────────────────────────────────────────────────────────────────┤
│  DOCUMENT MODEL (DOM)  (catalog, page tree, pages, name trees)        │  EPIC 4
├─────────────────────────────────────────────────────────────────────┤
│  READER/PARSER  │  WRITER/SERIALIZER  │  FILTERS/CODECS               │  EPIC 2 / 5 / 3
├─────────────────────────────────────────────────────────────────────┤
│  COS — Core Object System  (object model: dict, array, stream, ...)   │  EPIC 1
└─────────────────────────────────────────────────────────────────────┘
        ▲ reuse: flate2, image/zune-jpeg, aes/rsa/sha2, ttf-parser, rustybuzz
```

A short description of each layer:

- **COS (Core Object System)** — the object model of PDF syntax (ISO 32000 §7): `Boolean, Integer,
  Real, Name, String (literal/hex), Array, Dictionary, Stream, Null, Reference`. Registry of
  indirect objects (object number + generation).
- **Reader/Parser** — lexer + parser, xref table and xref stream, trailer, object streams,
  incremental updates, linearisation, recovery.
- **Filters/Codecs** — stream encode/decode: Flate, LZW, ASCIIHex, ASCII85, RunLength, DCT (JPEG),
  CCITTFax, JBIG2, JPX, Crypt; PNG/TIFF predictors.
- **Writer/Serializer** — full or incremental serialisation, xref table/stream, object-stream
  compression on write.
- **DOM** — document semantics: catalog, page tree with inherited attributes, resources, boxes
  (Media/Crop/…), outlines, metadata (Info + XMP), name/number trees.
- **Content streams** — the operator model, graphics state machine, builder & parser.
- **Fonts & Text** — Standard 14, TrueType/OpenType, Type1/CFF, Type0/CID, CMap, encoding,
  ToUnicode, embedding & subsetting; text extraction with positions.
- **Graphics & Imaging** — colour spaces, image/form XObjects, paths, shadings, patterns,
  transparency.
- **Crypto & Signatures** — standard security handler (RC4, AES-128/256), permissions, public-key
  handler, PKCS#7/CMS signatures.
- **High-level / Layout** — high-level authoring API (iText-style): document, paragraphs, tables,
  lists, flow/pagination.
- **Standards** — PDF/A, PDF/UA and PDF/X production and validation.

---

## 5. Cargo workspace structure

A monorepo, one Cargo workspace. Small single-responsibility crates, so bindings link only what
they need (light builds, feature flags).

```
prismpdf/
├── Cargo.toml                  # workspace
├── crates/
│   ├── pdf-cos/                # EPIC 1  — object model (no I/O)
│   ├── pdf-filters/            # EPIC 3  — codecs/filters
│   ├── pdf-reader/             # EPIC 2  — parser/lexer/xref/recovery
│   ├── pdf-writer/             # EPIC 5  — serializer/incremental
│   ├── pdf-document/           # EPIC 4  — DOM, pages, manipulation
│   ├── pdf-content/            # EPIC 6  — content stream ops
│   ├── pdf-fonts/              # EPIC 7  — fonts & text
│   ├── pdf-graphics/           # EPIC 8  — colour, images, shading
│   ├── pdf-crypto/             # EPIC 9  — encryption & signatures
│   ├── pdf-layout/             # EPIC 12 — high-level authoring
│   ├── pdf-standards/          # EPIC 13 — PDF/A, /UA, /X, XMP
│                               # (EPIC 14 — rasterisation: out of scope for v1; the
│                               #  `pdf-render` crate is created when the work starts)
│   ├── pdf/                    # facade crate: re-exports the idiomatic Rust public API
│   ├── pdf-ffi/                # EPIC 10 — C ABI (cdylib + staticlib), cbindgen
│   └── pdf-cli/                # EPIC 15 — CLI tool (inspect/merge/split/extract)
│                               # (EPIC 11 — bindings: OUT OF SCOPE, separate repos consuming
│                               #  the `prismpdf` crate or the `pdf-ffi` C ABI; not hosted here)
├── fuzz/                       # cargo-fuzz targets
├── corpus/                     # test PDFs (valid, malformed, edge cases)
└── docs/
    ├── README.md               # index: what each file is and who it is for
    ├── ABI.md                  # C ABI contract + versioning policy
    ├── BINDINGS.md             # binding author's guide (bindings live in their own repos)
    ├── native-artifacts.md     # contract for the prebuilt bundle published per tag
    ├── spec-map.md             # ISO section → crate index
    ├── baselines/              # recorded benchmark / conformance / validator runs
    └── rfc/                    # ISO specification copies (local only, gitignored)
```

**Dependency graph.** This is the *real* graph, derived from actual `use` statements — not an
aspirational one. Keeping it accurate is the point: the `doc` CI job fails on an intra-doc link
into a crate you do not depend on, which is what keeps the manifests honest.

```
cos          →  (nothing)
filters      →  cos
reader       →  cos, filters
writer       →  cos, filters
content      →  cos
crypto       →  cos
graphics     →  cos, filters
fonts        →  cos, content
document     →  cos, filters, reader, writer, content, crypto
layout       →  filters, document, content, fonts
standards    →  cos, document
pdf (facade) →  all of the above except writer
pdf-ffi      →  pdf
pdf-cli      →  pdf
```

Bindings depend on `pdf-ffi` (or on a native framework plus `prismpdf`) from their own
repositories.

---

## 6. FFI and binding strategy (the heart of the project)

The "bindings for every language, easily portable" constraint is the main architectural risk: it
must be **validated early**, not deferred to the end of the project.

> **Scope.** This repo provides and maintains **only the public API** that bindings consume: the
> `prismpdf` facade crate (idiomatic Rust API) and the `pdf-ffi` C ABI (plus the `cbindgen` header
> and [`docs/ABI.md`](docs/ABI.md)). The **actual per-language bindings are out of scope**: each
> lives in a **separate repo** with its own release cycle, depending on the `prismpdf` crate or
> linking `libpdf_ffi`. Sections 6.1–6.4 remain the guide to *how* those repos attach and which
> rules the core must respect to keep them feasible — not a commitment to host them here.

### 6.1 The canonical boundary: a handle-based C ABI
- Expose a **stable C surface covering the whole facade** (`#[repr(C)]`, `extern "C"`). The C ABI
  is the product surface, not a demonstration: every capability reachable from the `prismpdf` facade
  gets an entry point, so a binding written against `libpdf_ffi` is never a subset of the engine.
- **Capability parity, not signature parity.** C has no `Result`, `Option`, `String`, `Vec` or
  generics, so shapes necessarily change at the boundary (see the conventions in
  [`docs/ABI.md`](docs/ABI.md)). What must not change is *what you can do*: if the facade can do
  it, C can do it.
- **Opaque handles**: `*mut PdfDocument`, `*mut PdfPage`, and so on. The caller never sees the
  layout.
- **Explicit memory management**: every `*_new`/`*_open` has its matching `*_free`. Returned
  buffers come back as `(ptr, len)` with a corresponding `*_free_buffer`.
- **No panic across the boundary**: every FFI function wraps its body in
  `std::panic::catch_unwind`; errors leave as codes/out-params (`PdfResult`), never as unwinding.
- **`cbindgen`** generates the `prismpdf.h` header; the ABI versioning policy is documented in
  [`docs/ABI.md`](docs/ABI.md), separately from the Rust crate's SemVer.

This layer covers C and C++ on its own, and via FFI any language that can call C (Go/cgo,
C#/P-Invoke, Swift, …). It is the portability safety net.

### 6.2 Idiomatic wrappers for "tier 1" languages
For the highest-demand languages it is better NOT to go through the C ABI, but to use native
frameworks, for ergonomics and zero-copy:
- **Python** → **PyO3** (native module, `maturin` for packaging/PyPI). Best UX.
- **Browser/JS** → **wasm-bindgen** (`wasm32-unknown-unknown` target, npm package).
- **Node** → **napi-rs** (native N-API addon).

### 6.3 The "many languages cheaply" option: UniFFI
**UniFFI** (Mozilla) generates bindings for Swift, Kotlin, Python and Ruby (and, via third
parties, C#/Go) automatically from proc-macro definitions. Trade-off: less control over complex
APIs, callbacks and zero-copy. Recommended strategy:

> **Hybrid**: the C ABI + `cbindgen` as the universal bedrock → covers everything.
> PyO3 + wasm-bindgen as the first two proofs (they exercise both the "native framework" path and
> the "portable WASM" one). UniFFI evaluated later to cover Swift/Kotlin quickly.

### 6.4 Design rules the core must respect from day one
- Public APIs use mappable types (no heavy generics, complex lifetimes or trait objects at points
  destined for FFI).
- Errors modelled as an enum serialisable to stable integer codes.
- Operations idempotent and stateless where possible; state lives behind a handle.
- Abstract streaming I/O (read/write traits) to support files, memory and (in JS) buffers.

---

## 7. Cross-cutting concerns

- **Error handling**: `thiserror` in each crate, a rich error enum; mapped to stable codes at the
  FFI. Never `unwrap()`/`panic!` on untrusted input.
- **Logging/diagnostics**: `tracing` behind a feature flag (switchable off for lightweight
  bindings).
- **Security**: configurable limits (max nesting, max object count, max decompressed size),
  reference-cycle detection, logical timeouts. A security policy plus an advisory process.
- **Testing** (critical for PDFs):
  - Unit tests per object/operator.
  - **Round-trip**: load → save → load, compared structurally.
  - A **corpus** of real PDFs (valid + broken + edge cases), versioned under `corpus/`.
  - Continuous **fuzzing** (`cargo-fuzz`/libFuzzer) on the parser — mandatory for hostile input.
  - **Conformance**: veraPDF integration for PDF/A; `pdf20examples` for 2.0 features.
  - **Snapshot/visual diff** once rendering arrives.
- **Performance**: `criterion` benchmarks on parsing/serialisation/merge; memory profiles.
- **CI/CD**: a Linux/macOS/Windows matrix; clippy + rustfmt + deny(warnings); a declared MSRV;
  `cargo-deny` for licences and vulnerabilities.
- **Licence**: **MIT** — permissive, OSI-approved, and the default of the Rust ecosystem the
  engine depends on. Chosen over a source-available licence so the engine can be adopted and
  embedded without a legal review, which matters most for an FFI-first library that other
  languages link into their own products.

---

## 8. Milestones and backlog

Deliberately not here. Milestones, open residue and project debt live in
[`ROADMAP.md`](./ROADMAP.md); delivered work lives in [`CHANGELOG.md`](./CHANGELOG.md). A roadmap duplicated into the design document goes stale on the
first merge, and then disagrees with the copy that is maintained.

One sequencing decision is architectural rather than planning, so it belongs here: the
**portability proof (M3) is scheduled immediately after the write MVP, not at the end**.
Validating the FFI boundary while the API is still small is cheap and protects the whole
architecture; discovering ABI or ownership problems on a mature project is very expensive. The
proof is the boundary itself — C ABI + header + a C consumer — not bindings hosted here.
