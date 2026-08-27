# Releasing & versioning policy

This document is the contract for how Prism PDF is versioned, what stability you can rely on, and how a
release is cut. It complements [`CHANGELOG.md`](./CHANGELOG.md) (what changed) and
[`SECURITY.md`](./SECURITY.md) (how vulnerabilities are handled).

## Semantic Versioning

Prism PDF follows [Semantic Versioning 2.0.0](https://semver.org/). The **public API** under SemVer is:

- the **`prismpdf`** facade crate (the idiomatic Rust surface), and
- the **`pdf-ffi`** C ABI (functions, status codes, and the generated `prismpdf.h`), whose contract is
  in [`docs/ABI.md`](./docs/ABI.md).

The inner crates (`pdf-cos`, `pdf-reader`, `pdf-writer`, `pdf-document`, `pdf-content`, `pdf-fonts`,
`pdf-graphics`, `pdf-crypto`, `pdf-layout`, `pdf-standards`, `pdf-filters`) are **implementation
detail**: the facade depends on them as workspace path dependencies, and their APIs are not covered
by the stability guarantee — depend on `prismpdf`, not on them directly. All crates share one
workspace version and are released together.

Every crate is published under a `prismpdf-*` name — `pdf`, `pdf-cos`, `pdf-reader`, `pdf-writer`,
`pdf-content` and `pdf-cli` were already taken on crates.io by unrelated crates. Directory and
library names did not follow: `crates/pdf-cos` still builds the `pdf_cos` library, and
`crates/pdf-ffi` still builds `libpdf_ffi`, which is the filename bindings load. The one exception
is the facade, whose library is `prismpdf` because that is what a consumer types after
`cargo add prismpdf`.

### While `0.x`

Per SemVer, `0.x` makes no API-stability promise. In practice we treat the **minor** version as the
breaking axis during `0.x`:

- `0.MINOR.0` — may contain breaking changes to the public API (documented in the changelog under a
  **Changed**/**Removed** heading).
- `0.MINOR.PATCH` — bug fixes and additive, backward-compatible changes only.

### After 1.0

Once the API has soaked and we commit to stability:

- **MAJOR** — breaking changes to the public API.
- **MINOR** — backward-compatible additions.
- **PATCH** — backward-compatible bug fixes.

## Road to 1.0

The `0.3.x` line contains the planned non-rendering v1 capabilities, including tagged PDF, PDF/A
and PDF/UA production. Its `prismpdf` and `pdf-ffi` surfaces have passed the complete
release-candidate panel and independent Rust/C consumer exercise across parse, manipulate, low-level
create, and compose. The surface is accepted for the 1.0 stability guarantee; no planned capability
blocks the tag. A newly discovered correctness or safety issue in the read, sign, encrypt, compose,
or FFI ownership paths still blocks release until resolved.

`1.0.0` is the point at which the API stops moving without a major bump. Long-tail format breadth
and rendering remain explicitly non-blocking and stay in [`ROADMAP.md`](./ROADMAP.md).

## MSRV (minimum supported Rust version)

The MSRV is pinned in the workspace (`rust-version`) and verified in CI. It is currently **1.88**
(edition 2024). Raising the MSRV is a **minor**-version change (a `0.MINOR.0` bump while `0.x`); it is
called out in the changelog and never done in a patch release.

## Deprecation policy

We prefer deprecation over abrupt removal.

- A public item being retired is marked `#[deprecated(since = "x.y.z", note = "…")]` with a pointer to
  its replacement, and the deprecation is listed in the changelog under **Deprecated**.
- A deprecated item is kept for at least one minor release before removal, and is removed only in a
  version that is allowed to break the API (a minor bump while `0.x`, a major bump after 1.0).
- Removals are listed under **Removed** with the migration path.

## Cutting a release

1. Ensure `main` is green: `cargo test --workspace`, `cargo clippy --workspace --all-targets`,
   `cargo fmt --all --check`, `cargo doc --workspace --no-deps`, and `cargo deny check`.
2. Regenerate the C header if the ABI changed (`docs/ABI.md` describes the cbindgen invocation) and
   confirm `docs/spec-map.md` still points each ISO section at the crate that owns it.
3. Update [`CHANGELOG.md`](./CHANGELOG.md): move the `[Unreleased]` entries under a new
   `[x.y.z] - YYYY-MM-DD` heading and refresh the comparison links at the bottom.
4. Bump the version with `scripts/workspace_version.py X.Y.Z`. It sets `[workspace.package]`
   (which every crate inherits) and the `version` requirement on each internal dependency in
   `[workspace.dependencies]` — `cargo publish` rejects a dependency without one, and a
   requirement left behind points at the previous release.
5. Commit (`release: vX.Y.Z`), **then** tag the release commit
   (`git tag -a vX.Y.Z -m "vX.Y.Z"`) and push both.

Steps 3 and 4 are not optional bookkeeping, and the order matters: `prismpdf_version()` is
`prismpdf-ffi`'s package version, so a tag placed on a commit that was not bumped ships an ABI that
reports the *previous* version — every binding asserts on that string. The `guard` job in
`.github/workflows/release.yml` fails a tag whose version or changelog entry is missing, before
anything is built. Tags are cheap; a wrong one that bindings have already pinned is not, so verify
before pushing rather than retagging after.

## Published artifacts

Pushing a `v*` tag builds prebuilt shared `pdf-ffi` libraries for thirteen non-Apple targets and a
multi-platform Apple XCFramework (macOS, Mac Catalyst, iOS and iPadOS), then
publishes them with the C header, the conformance corpus and checksums. The same tag also builds
the `prismpdf` CLI for ten desktop targets (Windows, Linux glibc and musl, macOS; x64 and arm64
throughout) and attaches one archive per platform, so shell users are not required to have a Rust
toolchain and `cargo install prismpdf-cli`. Language bindings consume
them by version instead of by source checkout. What is published, the target matrix, the build
settings and the guarantees bindings pin against are in
[`docs/native-artifacts.md`](./docs/native-artifacts.md); that file and
`.github/workflows/release.yml` change together.

Two of those guarantees constrain how a release is cut: the ABI is **append-only**, and published
artifacts are **immutable** — a file is never re-uploaded with different bytes. A release that
turns out to be wrong is superseded by a new version, never replaced in place.

**Dry-run the matrix before tagging.** `workflow_dispatch` on the release workflow builds and
assembles everything and publishes nothing. A build failure after the tag is pushed does not burn
the tag — nothing is published unless every leg succeeds, so the fix is to correct the workflow and
re-run the same tag — but a dry run finds it before the tag exists at all.

**If the upload itself fails partway**, some assets for that version are attached to the GitHub
release and the rest are not. The immutability check will then refuse the re-run, correctly: it
cannot tell a partial release from a replacement attempt. Recover by deleting that release's
assets by hand (`gh release delete-asset v<X.Y.Z> <name>`, or delete the whole release with
`gh release delete v<X.Y.Z>` — the tag survives) and re-running the workflow. Do this only for a
version whose upload never completed — never to replace a release bindings have had the chance to
pin.

## crates.io

The same `v*` tag publishes the workspace to crates.io, in the `crates` job, after every native leg
has built. Order is derived from the dependency graph by `scripts/publish_order.py` — a registry
only accepts a crate whose dependencies it already has — and each upload is skipped if that exact
version is already on the registry, so a run that fails partway can be re-run to finish.

The registry is stricter than GitHub Releases in one direction and looser in another. **An upload
cannot be undone**: `cargo yank` hides a version from new resolution but never deletes it, and the
version number is spent forever. But crates.io has no equivalent of the immutability check on the
native bundle, because it does not need one — it refuses a second upload of a version it already
has. There is deliberately no dry run: `cargo publish --dry-run` verifies against the registry, so
every crate above the leaves would fail on internal dependencies the run has not published yet.

Publishing needs a `CARGO_REGISTRY_TOKEN` repository secret (a crates.io API token scoped to
publish-new and publish-update). Without it the job fails before uploading anything.

### Prereleases

`0.5.0-alpha.1` is a normal tag: `scripts/workspace_version.py 0.5.0-alpha.1`, a matching
`## [0.5.0-alpha.1] - YYYY-MM-DD` changelog heading, tag `v0.5.0-alpha.1`. Two things behave
differently from a release, both in your favour:

- `cargo add prismpdf` and a plain `prismpdf = "0.5"` requirement **ignore** prereleases. Only
  someone who writes the prerelease out in full gets it, which is what makes it safe to publish
  one from a branch of work that is not finished.
- For the same reason the internal requirements have to carry the full string —
  `version = "0.5.0"` does not match `0.5.0-alpha.1`. That is what `scripts/workspace_version.py`
  exists to keep true, and what the `guard` job re-checks on the tag.

The native artifacts are built and published for a prerelease tag exactly as for a release, so a
binding can pin one.

## Security releases

Vulnerabilities are handled per [`SECURITY.md`](./SECURITY.md): a private advisory, an agreed embargo,
then a fix. Security fixes are released as a patch on the latest line (and the advisory references the
fixed version). Because the threat model is hostile input crossing a trust boundary, a crash, hang,
OOM, or a bypass of the anti-DoS limits is treated as a vulnerability, not a routine bug.
