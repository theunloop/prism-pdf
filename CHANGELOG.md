# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project follows
[Semantic Versioning](https://semver.org/) as described in [`RELEASING.md`](./RELEASING.md).

Each released version needs a `## [x.y.z] - YYYY-MM-DD` heading before it can be tagged: the
`guard` job in `.github/workflows/release.yml` refuses a tag without one.

## [Unreleased]

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

### Fixed

- **Every Apple leg except macOS failed to build**, aborting with `failed to parse deployment
  target specified in MACOSX_DEPLOYMENT_TARGET: cannot parse integer from empty string`. A leg
  that declares no `macos_target` interpolated to the empty string, and GitHub exports that as a
  variable that is set-and-empty rather than absent, so rustc parsed the value instead of ignoring
  it. It reached even the iOS and tvOS legs because a build script compiles for the macOS *host*.
  The deployment-target variables are now exported only for the legs that declare them.
- **Every Android leg failed to build**, aborting with `cargo-ndk panicked! … unknown package:
  21`. cargo-ndk 4.0 freed lowercase `-p` for cargo's own `--package` passthrough and moved the
  API level to `-P`/`--platform`; the install was unpinned, so that release arrived on its own and
  read `-p 21` as `--package 21`. The workflow now passes `--platform 21` and pins cargo-ndk to
  `^4.1`.

[Unreleased]: https://github.com/theunloop/prism-pdf/compare/v0.4.0...HEAD
[0.4.0]: https://github.com/theunloop/prism-pdf/releases/tag/v0.4.0
