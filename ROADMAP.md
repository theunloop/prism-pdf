# Prism PDF — Roadmap

This file answers one question: **what should happen next?**

- ISO section → crate index: [`docs/spec-map.md`](docs/spec-map.md)
- Released work: [`CHANGELOG.md`](CHANGELOG.md)
- Architecture: [`DESIGN.md`](DESIGN.md)
- The 1.0 gate: [`RELEASING.md`](RELEASING.md)

Delivered work is removed from this file; Git and the changelog keep the history.

## Current focus

The current release is **0.4.0**. The planned non-rendering v1 capability line is implemented and
its Rust facade and C ABI have passed release-candidate validation plus independent downstream
exercise across parse, manipulate, low-level create, and compose. No capability work remains before
the `1.0.0` stability tag.
Rendering remains outside the v1.x scope.

### Before 1.0

**Cut the stability release.** Publish the validated `0.4.0` line, observe its CI, its native
artifacts (`docs/native-artifacts.md`) and documentation deployment, then follow `RELEASING.md` to
tag `1.0.0`. New correctness or safety findings remain
release blockers; long-tail format breadth below does not.

## Format breadth after 1.0

These are useful extensions, not claims that the current non-rendering engine is incomplete:

| Area | Remaining work |
|---|---|
| PDF/A production (M16) | Redistributable bundled CMYK profile; text/choice/radio field appearances; broader per-rule coverage for PDF/A parts 1 and 4 |
| Advanced crypto (M20) | Ed448 and brainpool curves (blocked on compatible upstream crates); SHA-3/SHAKE256 encryption KDF; signature policies; per-name crypt-filter selection; external CMS/PAdES oracle |
| PDF 2.0 structures (M21/M23) | DPart associated files; `/DestOutputProfileRef`; per-object metadata; RichMedia-specific handling |
| Read and font breadth (M24) | Safe JPEG 2000 pixel decoding; OTF/CFF authoring; CIDFontType0/CFF reading; predefined CJK CMaps |
| Tagged editing (M14 follow-up) | Preserve and remap structure when extracting pages instead of deliberately stripping it |
| Robustness | Document-wide decompression budget; OSS-Fuzz; more round-trip stress on complex tagged and AcroForm documents |
| Conformance | PDF Declarations XMP; Info/XMP precedence; ICC 2.0 fields; geospatial `/Measure` and `/PtData`; an internal validator |

Exact section-level gaps and upstream blockers belong in the spec overlays, not here.

## Out of scope

- **M8 rendering** (§10–§11 and rendering-only graphics interpretation) is outside v1.x. There is no
  `pdf-render` crate; create it only when rendering work begins.

## Maintenance rule

Update this file only when priorities or milestone ownership change. Update the spec map when an ISO
capability changes, and the changelog when user-visible work ships. Granular tasks belong in the
issue tracker.
