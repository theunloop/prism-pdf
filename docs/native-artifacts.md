# Published native artifacts

Every language binding lives in its own repository and links against the C ABI
([`BINDINGS.md`](BINDINGS.md)). Building that ABI from source is fine for developing a binding and
wrong for shipping one: a NuGet, wheel or npm package has to carry a prebuilt library for every
platform it claims to support, and no binding repository should be in the business of
cross-compiling Rust for sixteen targets.

So this repository publishes them. On every `v*` tag, `.github/workflows/release.yml` builds the
matrix below and uploads a versioned bundle that bindings consume **by version and checksum**
instead of by source checkout. This file is the contract for what that bundle contains and what
may be relied on; the workflow is its implementation, and the two change together.

Building from source remains supported and unchanged — `cargo build -p prismpdf-ffi --release` — and
is still the right path for anyone working on the engine itself. The artifacts are a way to not have
to.

## What is published per tag

Five files, each carrying the version in its name, plus one CLI archive per platform. GitHub
scopes assets per release, so the name is not what keeps them apart — it is what a binding pins,
and what identifies the file once it has been downloaded into a directory alongside other
releases' copies:

| File | Contains |
|---|---|
| `prism-pdf-natives-vX.Y.Z.tar.gz` | One shared library per RID (below), plus `prismpdf.h`, `VERSION`, `COMMIT`, and a `SHA256SUMS` covering every file in the bundle. |
| `prism-pdf-corpus-vX.Y.Z.tar.gz` | `corpus/{valid,malformed,edge}` — the shared conformance inputs. |
| `prism-pdf-xcframework-vX.Y.Z.zip` | `PrismPDF.xcframework` — static slices for every supported Apple platform. Only Apple bindings need it. |
| `prism-pdf-vX.Y.Z.h` | The header on its own, for vendoring without downloading the binaries. |
| `prismpdf-vX.Y.Z-<rid>.tar.gz` / `.zip` | The `prismpdf` command-line tool, prebuilt, one archive per platform. **Not a binding artifact** — nothing here is part of the ABI contract, and a binding never downloads one. See the CLI section below. |
| `SHA256SUMS-vX.Y.Z.txt` | Checksums of every file above. |

The CLI archives are listed here only because they share the release and the checksum file. They
carry no ABI guarantee, are not covered by the append-only rule, and their target list is a subset
of the one below: no Android, no Apple slices, because nothing runs a shell binary there.

`prismpdf.h` ships **inside** the bundle as well as beside it, so a binding can prove the header it
vendored and generated its P/Invoke layer from is the one these binaries were built against.

The corpus is a separate artifact rather than a directory in the bundle: it changes far less often
than the engine, so a binding can pin it independently and skip re-downloading it on an engine-only
release. It is what makes the "same journeys, same inputs, same assertions" rule in
[`BINDINGS.md`](BINDINGS.md) enforceable across languages.

## The target matrix

Thirteen non-Apple targets. The RID column is the .NET runtime identifier; other ecosystems name
these differently but the triple is the same artifact. They are shared libraries shipped per RID.
Apple platforms are static archives delivered together in the XCFramework — see "Apple platforms"
below.

| RID | Rust target | Library |
|---|---|---|
| `win-x64` | `x86_64-pc-windows-msvc` | `pdf_ffi.dll` |
| `win-x86` | `i686-pc-windows-msvc` | `pdf_ffi.dll` |
| `win-arm64` | `aarch64-pc-windows-msvc` | `pdf_ffi.dll` |
| `linux-x64` | `x86_64-unknown-linux-gnu` | `libpdf_ffi.so` |
| `linux-arm64` | `aarch64-unknown-linux-gnu` | `libpdf_ffi.so` |
| `linux-arm` | `armv7-unknown-linux-gnueabihf` | `libpdf_ffi.so` |
| `linux-musl-x64` | `x86_64-unknown-linux-musl` | `libpdf_ffi.so` |
| `linux-musl-arm64` | `aarch64-unknown-linux-musl` | `libpdf_ffi.so` |
| `osx-x64` | `x86_64-apple-darwin` | `libpdf_ffi.dylib` |
| `osx-arm64` | `aarch64-apple-darwin` | `libpdf_ffi.dylib` |
| `android-arm64` | `aarch64-linux-android` | `libpdf_ffi.so` |
| `android-arm` | `armv7-linux-androideabi` | `libpdf_ffi.so` |
| `android-x64` | `x86_64-linux-android` | `libpdf_ffi.so` |
## Apple platforms

Apple platforms do not use the downloaded-at-runtime shared-library model. The engine is linked
statically into the signed application, so every supported Apple platform is delivered as one
`PrismPDF.xcframework` inside `prism-pdf-xcframework-vX.Y.Z.zip`. The framework is a selection
container, not a new ABI: each archive exposes the same C ABI in `prismpdf.h`, and Xcode selects the
correct platform/environment/architecture slice at build time.

| Platform variant | Rust targets | Minimum deployment target |
| --- | --- | --- |
| macOS | `aarch64-apple-darwin`, `x86_64-apple-darwin` | 11.0 |
| Mac Catalyst | `aarch64-apple-ios-macabi`, `x86_64-apple-ios-macabi` | 14.0 |
| iOS / iPadOS device | `aarch64-apple-ios` | 13.0 |
| iOS / iPadOS Simulator | `aarch64-apple-ios-sim`, `x86_64-apple-ios` | 13.0 |
| tvOS device | `aarch64-apple-tvos` | 13.0 |
| tvOS Simulator | `aarch64-apple-tvos-sim`, `x86_64-apple-tvos` | 13.0 |
| visionOS device | `aarch64-apple-visionos` | 1.0 |
| visionOS Simulator | `aarch64-apple-visionos-sim` | 1.0 |
| watchOS device | `aarch64-apple-watchos` | 7.0 |
| watchOS Simulator | `aarch64-apple-watchos-sim`, `x86_64-apple-watchos-sim` | 7.0 |

The framework carries `prismpdf.h` and `module.modulemap`. Its C module is **`CPrismPDF`**, leaving
the `PrismPDF` product name available to a dedicated Swift wrapper. Swift and Objective-C therefore
use `import CPrismPDF`; a managed binding resolves the static entry points from the app binary (for
example, .NET uses `DllImport("__Internal")`).

All Apple archives disable stripping and LTO. A static archive is an input to the consuming linker:
stripping removes the symbol table it needs for `prismpdf_*`, and thin LTO leaves LLVM bitcode that
requires the app linker to match rustc's LLVM. The app linker dead-strips unused code, so this does
not inflate the shipped application merely because an archive is large.

The release pipeline assembles the XCFramework using Xcode and links a C probe against every
platform slice. That proves the archive links with its matching SDK; it does not execute PDF work
on a device or simulator. Apple binding CI must run those end-to-end journeys. Applications should
also link only one Rust static library where possible: multiple Rust static libraries can collide on
Rust runtime symbols.

## Build settings, and why they are not the binding's problem

Four settings decide whether an artifact loads on a consumer's machine. None of them can be fixed
afterwards by a binding, which is why they are pinned here and not left to whatever the CI runner
happened to have installed.

- **Windows: static CRT.** Rust's MSVC targets link the VC++ runtime dynamically, so a consumer
  without the redistributable gets a load failure. All three Windows legs build with
  `-C target-feature=+crt-static`, which links the runtime into the DLL.

- **Linux glibc: a floor of 2.17.** A build on a current Ubuntu links that runner's glibc and will
  not load on RHEL 8, let alone RHEL 7. The three `linux-gnu` legs are built with
  [`cargo-zigbuild`](https://github.com/rust-cross/cargo-zigbuild) against an explicit `.2.17`
  target suffix. **glibc 2.17 (RHEL 7 / CentOS 7 era) is the committed floor** for the ecosystem;
  raising it is a breaking change for consumers and is announced in `CHANGELOG.md`.

- **Linux musl: a separate build, dynamically linked.** A glibc `.so` cannot load on Alpine, so
  musl is its own artifact rather than a variant. It is built with
  `-C target-feature=-crt-static`: the musl target defaults to a *static* libc, which is right for
  an executable and wrong for a shared object a host process will `dlopen`. Related, for binding
  authors: musl has no `libdl` — it implements `dlopen` inside libc — which some runtime loaders
  need told.

- **macOS: deployment target and a signature.** `MACOSX_DEPLOYMENT_TARGET` is 10.12 for x64 and
  11.0 for arm64, and both dylibs are ad-hoc signed (`codesign --force --sign -`) in CI. On arm64
  macOS an unsigned dylib does not load at all; ad-hoc signing establishes a valid signature
  without asserting an identity, and turns a failure the consumer would have to `xattr` their way
  out of into one that never happens.

Artifacts are built with `[profile.dist]` (`Cargo.toml`): `release` plus `strip = "symbols"`, thin
LTO and a single codegen unit. `release` itself deliberately keeps symbols, so a local build can
still symbolicate the backtrace behind an overflow-check panic. `panic` stays `unwind` — `pdf-ffi`
contains panics with `catch_unwind` and reports them as `PrismPdfStatus_Internal` (DESIGN.md §6.1),
and `panic = "abort"` would turn that contained error into a host-process abort.

## Guarantees a binding may pin against

- **`prismpdf_version()` equals the release tag.** It is `env!("CARGO_PKG_VERSION")` of `pdf-ffi`,
  and the release workflow refuses to build a `vX.Y.Z` tag whose workspace version disagrees, or
  one with no `CHANGELOG.md` entry. A binding's vertical slice may assert on the string. (Historic
  exception: `v0.3.1` was tagged on a documentation commit before this guard existed and reports
  `0.3.0`. It carries no engine change over `0.3.0` and is superseded by `0.4.0`.)

- **The ABI is append-only.** Existing signatures and status-code values never change; new surface
  is added. This is what lets a binding built against an older header keep working against newer
  binaries, and why a managed-side fix can ship without rebuilding any Rust. The policy is
  [`ABI.md`](ABI.md), "Versioning policy"; additions are listed in `CHANGELOG.md`.

- **Artifacts are immutable.** A published file is never re-uploaded with different bytes. The
  release workflow checks the destination and fails rather than replace an existing name — a silent
  replacement becomes an unreproducible build in someone else's CI days later. If a release is
  wrong, cut a new version.

- **Nothing is fetched at runtime.** These artifacts are for a binding's *build*, to be packaged
  into whatever it ships. A binding that downloads a native library on first use breaks air-gapped
  consumers and is a supply-chain liability; do not design one.

## Consuming them

Artifacts are published to **GitHub Releases**, one release per tag. The repository is public, so
binding CI needs no credentials — a plain unauthenticated HTTPS GET serves every language equally
rather than favouring one package ecosystem:

```bash
BASE="https://github.com/theunloop/prism-pdf/releases/download/v${VERSION}"
curl -fsSLO "${BASE}/prism-pdf-natives-v${VERSION}.tar.gz"
curl -fsSLO "${BASE}/SHA256SUMS-v${VERSION}.txt"
sha256sum -c "SHA256SUMS-v${VERSION}.txt"
```

`gh release download v${VERSION} --pattern 'prism-pdf-natives-*'` does the same where the GitHub CLI
is available. Fetch by tag, never by `latest`: a binding pins a version and its checksums, and
`latest` silently changes what it resolves to.

A binding then unpacks the per-RID directories into wherever its packaging expects them
(`runtimes/<rid>/native/` for NuGet), records the tag, commit and per-RID checksum in whatever file
it uses to track vendored inputs, and verifies that its vendored `prismpdf.h` matches the bundled
one before regenerating any P/Invoke layer from it.

Requests for a target that is not in the matrix — iOS static libraries, 32-bit anything else, a
different glibc floor — belong in an issue against this repository rather than a workaround in a
binding: the matrix is a list in one workflow file, and one pipeline serves every binding.
