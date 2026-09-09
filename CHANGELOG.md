# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project follows
[Semantic Versioning](https://semver.org/) as described in [`RELEASING.md`](./RELEASING.md).

Each released version needs a `## [x.y.z] - YYYY-MM-DD` heading before it can be tagged: the
`guard` job in `.github/workflows/release.yml` refuses a tag without one.

## [Unreleased]

### Added

- **`prismpdf_builder_embed_cid_font`** — the C ABI export the header had been citing since
  0.4.0 without ever declaring it. It embeds a **whole** sfnt program under a resource name for
  pages assembled by hand with `PageSpec` + `Content::show_glyphs`, where the glyphs a page will
  show are not known in advance: every glyph's width goes into `/W` and every `cmap`-mapped
  character into `/ToUnicode`, so the text extracts back. Behind it, `prismpdf::cid_font_from_sfnt`
  (new, in `pdf-layout`) builds the `CidFont`, and `pdf-fonts` gains `glyph_advances`. `Flow` and
  `Composition` keep subsetting to the glyphs they drew; this is for the caller who cannot.
- **`prismpdf_composition_into_builder`** — finalise a declarative composition and take its
  document over as an owned `Builder`, so `/Info` metadata, attachments, an outline and the PDF/A
  and PDF/UA passes apply to a composed document before it is serialised. The facade path already
  existed (`Composition::into_builder` → `PreparedComposition::builder_mut`); across the ABI a
  composition could set nothing but its tagged language. `PreparedComposition::into_builder` is
  new, and the type is now re-exported from `prismpdf`.
- **Form-field geometry**: `FormField` carries `rect` and `page_index` — the first widget's
  `/Rect`, normalised, and the page it sits on, from `/P` or, failing that, from the page whose
  `/Annots` lists it (§12.5.2, §12.7.3.1). The reader used to stop at name, type and value, so
  placing anything relative to a named field meant walking `/AcroForm` by hand. On the ABI:
  `prismpdf_form_field_rect` and `prismpdf_form_field_page_index`, `NotFound` for a field with no
  widget.
- **Certificate chains in signatures**: `SignSettings::extra_certificates` (and, beneath it,
  `SignOptions::extra_certificates` in `pdf-crypto`) carries additional DER certificates in the
  CMS `certificates` set alongside the signer's — the intermediates of a private issuing chain, so
  a validator that does not hold that CA can still chain the signer to a root it trusts
  (RFC 5652 §10.2.3, §12.8.3.3). Until now the CMS held the signer certificate only, and every
  signature from a private CA validated as an unknown issuer anywhere but here. On the ABI:
  `prismpdf_sign_settings_add_certificate`, once per certificate.
- **PNG as an image source**: `Image::from_png` (and `prismpdf_image_source_from_png`) accepts a
  PNG file and embeds it through the raw-sample paths the engine already had — greyscale, RGB,
  and, when the file carries alpha, the same `/SMask` soft mask `from_rgba` produces; palettes are
  expanded and 16-bit samples reduced to 8. PNG is what logo uploads, QR generators and every
  raster toolkit emit by default; until now a caller decoded it with a second library first.
  Decoded by `zune-png`, the same family as the `zune-jpeg` already used for DCTDecode, under the
  same licence policy. A new `png` fuzz target covers the decoder-plus-reshaping path.
- **Images in signature appearances**: `SignatureAppearance::image` draws an image XObject — a
  rendered signature graphic, an organisation's stamp — in the visible signature widget, fitted
  into the whole box when there is no caption and into its left part with the caption beside it
  (§12.5.5, §8.9.5); a soft or stencil mask on the image travels with it (§11.6.5). The appearance
  used to be Helvetica text and nothing else. On the ABI:
  `prismpdf_sign_settings_set_appearance_image`, taking any `PrismPdfImageSource` — JPEG, PNG,
  raw samples — which stays caller-owned.
- **Signing into an existing signature field**: `SignSettings::field_name` signs *into* a named
  `/Sig` field instead of adding a new one — the shape every template-driven workflow has
  (§12.7.4.5). The field's widget supplies the rectangle and the page, its `/V` receives the
  signature, and the appearance replaces the widget's, with the default caption drawn when none
  was configured, because an untouched empty field beside a real signature is the wrong outcome.
  `DocError::SignatureFieldNotFound` and `DocError::SignatureFieldUnusable` say why a name was
  refused (missing; not a signature field, already signed, direct object, no widget), and cross
  the ABI as `NotFound` and `InvalidUse` with the reason in `prismpdf_last_error`. The template
  side is `FormFieldSpec::Signature` on `Builder` (`prismpdf_builder_add_signature_field`), an
  empty signature field with an empty appearance for a later signer to fill. On the ABI:
  `prismpdf_sign_settings_set_field_name`.
- **Signing journeys the suite had not asked**: a second signature over a signed document, with
  both verifying and the first revision's bytes untouched; and a signature this engine did not
  produce — `corpus/valid/signed-openssl-cms.pdf`, whose detached CMS is OpenSSL's over a revision
  the engine laid out (`tools/gen_external_signature.py`). `verify_signatures` had only ever been
  run against its own output. Bindings port these as part of their conformance suites.

### Fixed

- **A CFF-flavoured OpenType program is embedded as the composite font §9.9 defines for it**:
  `/FontFile3` with `/Subtype /OpenType` under a `CIDFontType0` descendant, and no `/CIDToGIDMap`
  (a `CIDFontType2` entry only, §9.7.4.2). `cid_font_from_sfnt` accepted any face `ttf-parser`
  parsed, `.otf` files included — as `prismpdf_builder_embed_cid_font`'s own "TrueType/OpenType"
  contract invites — but the writer emitted every program as `/FontFile2` under `CIDFontType2`,
  producing a PDF whose text no conforming reader renders. `CidFont` carries the distinction in a
  new `cff` field, `pdf-fonts` reports it as `FontInfo::outlines`, and an embedded OpenType
  program floors the header at PDF 1.6. A CFF program is also embedded whole rather than
  subsetted, because only the TrueType form has a `/CIDToGIDMap` to carry the renumbering the
  subsetter applies. Two sfnt families have no §9.9 embedding form here and are now refused
  outright — `Parse` on the ABI, `false` from `Flow::embed_font`, `InvalidFont` from
  `Composition::embedded_font` — rather than embedded as something they are not: a **CID-keyed**
  CFF, whose charset maps CIDs to glyphs so glyph ids cannot double as CIDs, and CFF2.
- **A template with a signature field declares `/SigFlags`.** `FormFieldSpec::Signature`
  (`prismpdf_builder_add_signature_field`) wrote the widget and the `/AcroForm` `/Fields` entry
  but left the form dictionary without the `SignaturesExist` bit (§12.7.2, Table 218), which is
  what a viewer reads to know a document has a signature field — so Acrobat offered no signing
  workflow on a template built for exactly that. The builder now sets `/SigFlags 1`; `AppendOnly`
  stays clear until something is signed, where `Document::sign` has always set both.
- **A short `/Rect` array is no longer read as a rectangle.** `FormField::rect` (and
  `prismpdf_form_field_rect`) zero-filled a `/Rect` with fewer than four numbers, reporting a
  corner the file never gave — `[10 20 300]` came back as `[10 0 300 20]`. Worse on the new
  signing path, where `SignSettings::field_name` takes that rectangle as the signature widget's
  box and laid the appearance out over the wrong area. Such a `/Rect` is refused (`None`) now.

## [1.0.0-alpha.1] - 2026-08-31

First prerelease of the `1.0.0` stability line. The API surface is the validated `0.4.x` one;
this tag exists so bindings and downstream consumers can pin and exercise the candidate surface
while it soaks. Per SemVer, `cargo add prismpdf` and plain version requirements ignore a
prerelease — only the full `1.0.0-alpha.1` string resolves to it.

### Added

- **One-line CLI installers**: `scripts/install.sh` (macOS/Linux, `curl | sh`) and
  `scripts/install.ps1` (Windows, `irm | iex`) download the prebuilt archive matching the
  machine from GitHub Releases, verify it against the release's `SHA256SUMS-v*.txt`, and install
  the `prismpdf` binary — to `~/.local/bin` and `%LOCALAPPDATA%\Programs\prismpdf` respectively,
  no root or admin needed. The shell script picks the static `linux-musl-*` build on musl systems
  and on glibc older than the 2.17 floor; both honour `PRISMPDF_VERSION` (pin a release) and
  `PRISMPDF_INSTALL_DIR`. CI's new `install` job lints and runs both end-to-end against a real
  release on all three desktop platforms.

- **Runtime diagnostics behind a `tracing` feature** (DESIGN.md §7). `pdf-reader`, `pdf-filters`
  and `pdf-document` now emit `tracing` events on their recovery paths — the xref
  rebuild-by-scan (entry count, anti-DoS truncation), the raw-DEFLATE fallback in `FlateDecode`,
  and every `OpenReport` recovery diagnostic — and the `prismpdf` facade forwards one `tracing`
  feature into all of them. Off by default, so nothing changes for existing consumers; the
  engine only emits and never installs a subscriber, so the application chooses the destination.
  Events carry offsets, object numbers and counts, never document content.

## [0.4.1] - 2026-08-28

### Fixed

- **`Flow::embed_font` now replaces the Standard-14 font registered under the same name**, which is
  what `prismpdf_flow_embed_font`'s doc comment has always promised. It used to add the embedded
  program alongside the registration made by `Flow::new`, so a flow that named its resource up
  front — the natural way to write the call — kept a non-zero
  `BuilderFacts::standard_14_font_resources` and `make_pdfa` / `make_pdfua` refused it with
  `UnembeddedFont`, even though every glyph came from the embedded program and the page resource
  dictionary held only the Type0 font. The surviving registration also emitted one orphan
  `/Type /Font` object per page, referenced by nothing. Embedding under a name `Flow::new` never
  mentioned was the only conformant spelling; both now work. `Composition` has always replaced this
  way, because it keys its fonts by name.

### Changed

- **`verify_base` gates only with a quorum of three resolved validators.** The panel's verdict is a
  *majority* so that one member's feature gap cannot fail a valid file, but the gate asserted that
  verdict at any panel size. With a single validator resolved — the common local state, since
  `pdfcpu` is the one member not in apt and `tools/verify/` is git-ignored — "majority" meant "this
  tool is authoritative", and pdfcpu's documented refusal of Document Parts §14.12 (`"DPartRoot"
  not supported`, on a validator whose own banner says PDF 2.0 is need-basis) failed three
  should-pass files that the other four accept. Below quorum the harness now prints its report and
  names the missing tools instead of asserting. CI installs all five and is unaffected.
- **The devcontainer installs the base-PDF validator panel**, so a container gates like CI instead
  of silently standing down. It already installed veraPDF for the PDF/A oracle; this is the same
  step for the base-PDF one. Best-effort, like the veraPDF block: an offline container still
  builds and tests.
- **The C ABI names its ownership shape in each doc comment.** Calls that may claim a handle now
  carry one of three markers — `Consumes on success` (`edit_commit`, `builder_add_page_spec`,
  `builder_add_structure_node`, `struct_node_add_child`), `Consumes always` (`flow_build`,
  `flow_into_builder`) or `Finalises` (`composition_build`) — and `docs/ABI.md` tabulates them.
  `docs/BINDINGS.md` semantic contract 3 described only the first shape while listing exports from
  all three, so a binding author who applied it uniformly would free a flow that a failing
  `flow_build` had already taken. Behaviour is unchanged; only the header comments and the two
  documents moved.
- **`docs/BINDINGS.md` name rule 6** now says the signature decides placement, never the name:
  `measure_text` and `wrap_text` take a `PrismPdfTextBlock *` first, so rule 2 puts them on the
  text block rather than on the top-level class, where rule 6 used to cite them as examples. The
  rule also no longer requires that class to be spelled exactly `PrismPdf`, which C# cannot do
  without colliding with its own namespace.
- The `cbindgen` command in `crates/pdf-ffi/cbindgen.toml` and `docs/ABI.md` passes
  `--crate prismpdf-ffi`; the package was renamed in 0.4.0 and the recorded command still said
  `pdf-ffi`, which makes `cbindgen` abort.

## [0.4.0] - 2026-08-27

### Added

- **Prebuilt `prismpdf` CLI binaries on every release**, one archive per platform: Windows
  (x64/arm64/x86), Linux glibc (x64/arm64/armv7), Linux musl (x64/arm64) and macOS (x64/arm64).
  `cargo install prismpdf-cli` is no longer the only way to get the tool — it needs a Rust
  toolchain and compiles the whole engine. The musl builds are static and run anywhere; the glibc
  builds keep the same 2.17 floor as the shared libraries. These archives carry **no ABI
  guarantee** and no binding consumes them; they are listed in `docs/native-artifacts.md` only
  because they share the release and its checksum file.
- **The release workflow publishes to crates.io**, in a `crates` job that runs after every native
  leg has built. Order comes from `scripts/publish_order.py` (derived from the dependency graph);
  an already-published version is skipped, so a partial run can be re-run to completion. Needs a
  `CARGO_REGISTRY_TOKEN` repository secret.
- `scripts/workspace_version.py` — bumps `[workspace.package].version` and every internal
  requirement together, and `--check`s that they agree. Wired into CI and into the release guard.

### Changed

- **Every crate is published under a `prismpdf-*` name.** `pdf`, `pdf-cos`, `pdf-reader`,
  `pdf-writer`, `pdf-content` and `pdf-cli` were already taken on crates.io by unrelated crates.
  Directory and library names did not follow the packages: `crates/pdf-cos` still builds the
  `pdf_cos` library and `crates/pdf-ffi` still builds `libpdf_ffi`, which is the artifact filename
  bindings load. The one exception is the facade — its library is now `prismpdf`, so Rust
  consumers write `use prismpdf::Document` instead of `use pdf::Document`.
- **MSRV raised to 1.88** (from 1.85). 1.87 is where `usize::is_multiple_of` is stable, which
  removed the reason for the vendored `hayro-ccitt` fork; 1.88 is where `let` chains are stable,
  which every published `hayro-jbig2` with the `Decoder`/`Image` API needs to compile.
- Internal dependencies in `[workspace.dependencies]` now carry a `version` requirement alongside
  their `path`. `cargo publish` rejects a dependency without one.

### Removed

- **The vendored `hayro-ccitt` / `hayro-jbig2` forks and the `[patch.crates-io]` section.** The
  fork carried a one-line MSRV shim, and a patch section does not travel with a published crate —
  anyone depending on `prismpdf-filters` would have resolved the unpatched upstream anyway. Both
  now resolve from crates.io unmodified.
- **tvOS, watchOS and visionOS slices, devices and simulators alike.**
  `PrismPDF.xcframework` now carries four libraries — macOS, Mac Catalyst, iOS/iPadOS device and
  iOS/iPadOS Simulator — instead of ten. Both Intel simulator targets in the dropped set
  (`x86_64-apple-tvos`, `x86_64-apple-watchos-sim`) are tier-3 Rust targets with no `rust-std`
  distributed on stable, so `rustup target add` fails outright and no stable toolchain could
  produce them at all; the rest went with them rather than ship a framework whose coverage varies
  by architecture. The four surviving variants are unchanged and still carry both architectures
  wherever Apple has two.

### Fixed

- **Every Apple leg except macOS failed to build**, aborting with `failed to parse deployment
  target specified in MACOSX_DEPLOYMENT_TARGET: cannot parse integer from empty string`. A leg
  that declares no `macos_target` interpolated to the empty string, and GitHub exports that as a
  variable that is set-and-empty rather than absent, so rustc parsed the value instead of ignoring
  it. It reached even the iOS legs because a build script compiles for the macOS *host*. The
  deployment-target variables are now exported only for the legs that declare them.
- **Every Android leg failed to build**, aborting with `cargo-ndk panicked! … unknown package:
  21`. cargo-ndk 4.0 freed lowercase `-p` for cargo's own `--package` passthrough and moved the
  API level to `-P`/`--platform`; the install was unpinned, so that release arrived on its own and
  read `-p 21` as `--package 21`. The workflow now passes `--platform 21` and pins cargo-ndk to
  `^4.1`.

[Unreleased]: https://github.com/theunloop/prism-pdf/compare/v1.0.0-alpha.1...HEAD
[1.0.0-alpha.1]: https://github.com/theunloop/prism-pdf/compare/v0.4.1...v1.0.0-alpha.1
[0.4.1]: https://github.com/theunloop/prism-pdf/compare/v0.4.0...v0.4.1
[0.4.0]: https://github.com/theunloop/prism-pdf/releases/tag/v0.4.0
