# AGENTS.md

This file provides guidance to Codex (Codex.ai/code) when working with code in this repository.

## Project state — read this first

This repo is a **mature, working PDF engine** (~39k lines of Rust across 14 crates), not a
skeleton. The first public cut is **0.1.0**; the `1.0` tag is gated on soak (`RELEASING.md`).
Rendering (§10–§11) is deliberately out of scope for v1.

**This file holds rules, commands and pointers — never status.** For *what shipped*, read
`CHANGELOG.md`; for *what is next*, `ROADMAP.md`.
Do not restate any of those here: a status claim in this file rots on the next merge.

- **`DESIGN.md`** is the authoritative technical design for the project: a PDF engine called
  **Prism PDF** (CLI binary `prismpdf`), written in Rust, with an FFI-first C ABI and bindings for
  many languages. Read it before doing architectural work. It carries no status and no roadmap —
  those live in `CHANGELOG.md` and `ROADMAP.md`.
- The workspace from `DESIGN.md` §5 is a virtual workspace (`Cargo.toml`) of
  single-responsibility crates under `crates/`: `pdf-cos`, `pdf-filters`, `pdf-reader`,
  `pdf-writer`, `pdf-document` (the biggest, ~12k lines), `pdf-content`, `pdf-fonts`,
  `pdf-graphics`, `pdf-crypto`, `pdf-layout`, `pdf-standards`, the `prismpdf` facade, `pdf-ffi`, and
  the `pdf-cli` binary. Edition 2024, MSRV 1.88, MIT licensed. External crate versions are
  centralized in `[workspace.dependencies]`; internal crates use `<crate>.workspace = true` path
  deps. **Declare only dependencies you actually use** — the `doc` CI job fails on an intra-doc
  link into a crate you do not depend on, which is what keeps the graph below honest.
- CI (`.github/workflows/ci.yml`) runs nine jobs on every PR — `fmt`, `clippy`, `doc`, `test`,
  `msrv` (1.88), `deny`, `fuzz` (smoke run of every target), `pdfa` (veraPDF at each sample's own
  flavour + `ua1`/`ua2`), and `verify` (the M18 five-validator panel for base PDF) — eleven check
  runs, since `test` is a Linux/macOS/Windows matrix. Warnings are errors everywhere
  (`RUSTFLAGS: -D warnings`, and `RUSTDOCFLAGS: -D warnings` in `doc`).
- A `v*` tag additionally runs `.github/workflows/release.yml`: it guards that the tag, the
  workspace version and the changelog agree, cross-compiles `pdf-ffi` for sixteen targets with
  `--profile dist`, and publishes the bundle bindings consume. The contract for those artifacts is
  `docs/native-artifacts.md`; change it and the workflow together. The same tag builds the
  `prismpdf` CLI for ten desktop targets in the `cli` job and attaches one archive per platform —
  those carry no ABI guarantee and no binding consumes them.
- **Language bindings are out of scope for this repo**: each binding (Python, WASM, Node, .NET, …)
  lives in its own separate repo and consumes the public API — the facade crate `prismpdf` or the C
  ABI `pdf-ffi`. This repo's job is to keep that surface stable and FFI-clean (§6.4) and to document
  it in `docs/ABI.md`; it does not ship the bindings themselves (there is no `bindings/` dir).
  Authors starting a binding follow `docs/BINDINGS.md` — the cross-language object model, naming
  rules, and conformance suite that keep every binding's API shape the same.
- The COS object model's design decisions (ADR-0001–0004) are recorded in the **module docs of
  `crates/pdf-cos/src/lib.rs`**, with the vocabulary in `GLOSSARY.md`. Read both before changing
  `Object`/`Dictionary` semantics — notably: `Integer(1) != Real(1.0)`, order-independent dict
  equality, `Arc`-backed O(1) clones, streams inert with `raw().len()` (not `/Length`) as the
  length authority.

**English is the repository language** (`CONTRIBUTING.md` rule 6): code, comments, commits, tests
and documentation, internal design notes included. The whole doc set is English as of 2026-08-18.
In conversation, mirror the user.

## The spec-map indexes ISO sections to crates

`docs/spec-map.md` indexes every ISO 32000 section (§7 syntax, §8 graphics, §9 text/fonts,
§12 interactive, §14 interchange) to the crate that owns it. It is an index into the code, **not**
a status report: it carries no progress markers and no roadmap. `CHANGELOG.md` remains
authoritative for what shipped, `ROADMAP.md` for what is next.

- When implementing a feature, **cite its ISO section number** in code comments and tests, and add
  a row to `spec-map.md` when you take on a section it does not index yet. Orient by section
  number (stable across editions), not page number.
- Numbering follows ISO 32000-1 (1.7); 2.0 renumbers some sub-sections, so confirm the exact
  sub-section against your own copy when it matters.
- Full ISO specs live in `docs/rfc/` (PDF32000_2008 = 1.7, ISO_32000-2 = 2.0, plus TS 32001–32005
  and the PDF/UA texts) — `docs/rfc/README.md` lists every document and its filename.
- **Milestone M1 (Read MVP)** uses only these sections:
  §7.2 (lexer) → §7.3 (objects) → §7.5.2–7.5.5 (header/body/xref table/trailer) →
  §7.4.4 + §7.4.2/3/5 (Flate + ASCII filters) → §7.5.7–7.5.8 (object/xref streams) →
  §7.7.2–7.7.3 (catalog/page tree). Don't pull in later sections early.

## Architecture

Strictly **layered with one-way dependencies** — lower layers never know about higher ones.
A Cargo monorepo of small single-responsibility crates so bindings link only what they need.
This is the **actual** graph (verified against the manifests; keep it that way):

```
cos          →  (nothing)              # pdf-cos: COS object model + §7.2.2 lexical classes, no I/O
filters      →  cos                    # pdf-filters: Flate, LZW, ASCIIHex/85, RunLength, DCT, CCITT, JBIG2
reader       →  cos, filters           # pdf-reader: lexer/parser/xref/recovery
writer       →  cos, filters           # pdf-writer: serializer/incremental/object streams
content      →  cos                    # pdf-content: content-stream operators, graphics state machine
crypto       →  cos                    # pdf-crypto: encryption (RC4/AES) + signatures
graphics     →  cos, filters           # pdf-graphics: colour spaces, image XObjects, §7.10 functions
fonts        →  cos, content           # pdf-fonts: metrics, encodings, CMaps, subsetting
document     →  cos, filters, reader, writer, content, crypto   # DOM: catalog, page tree, merge/split
layout       →  filters, document, content, fonts
standards    →  cos, document          # pdf-standards: PDF/A, PDF/UA, XMP, output intents
pdf (facade) →  all of the above except writer   # idiomatic Rust public API
pdf-ffi      →  pdf                    # C ABI (cdylib+staticlib), handle-based, cbindgen header
pdf-cli      →  pdf                    # the `prismpdf` binary
```

Language bindings (python/PyO3, wasm, node, java, dotnet, go, swift) consume `pdf-ffi` or `prismpdf`
from their own repositories — see "Project state" above.

Non-negotiable design rules from `DESIGN.md` §3, §6, §7:

- **Recovery is first-class**, not a fallback: the reader must rebuild the xref by scanning
  when a file is broken. Real PDFs are frequently malformed.
- **Hostile input**: the parser treats input as untrusted. No `panic!`/`unwrap()` on untrusted
  input; enforce configurable anti-DoS limits (max nesting, object count, decompressed size,
  reference-cycle detection). Continuous fuzzing on the parser is required.
- **FFI-first**: public API types must map cleanly onto a C ABI — no heavy generics, complex
  lifetimes, or trait objects at points destined for FFI. Errors are a serializable enum →
  stable integer codes. Every FFI fn wraps its body in `catch_unwind`; nothing unwinds across
  the boundary.
- **Zero `unsafe` in the core**; `unsafe` is confined and audited only in `pdf-ffi`.
- **Reuse over reimplementation** for codecs/crypto/fonts: `flate2`/`miniz_oxide`,
  `zune-jpeg`/`image`, `aes`/`rsa`/`sha2`, `ttf-parser`/`rustybuzz`.
- **Lazy & streaming**: parse objects on demand via the xref; never load the whole file.

Roadmap milestones M1→M8 are in `DESIGN.md` §9. M3 (prove the FFI/portability thesis) is
deliberately scheduled **right after** the write MVP, not at the end — but the proof is the *public
API surface itself* (the FFI-clean C ABI + cbindgen header + `docs/ABI.md`), since the language
bindings that would consume it are out of scope here (separate repos).

## Commit messages

The house style is what `git log` already shows; match it rather than inventing a new shape.

- **Subject**: one imperative sentence, 72 characters or fewer, capitalized, no trailing period —
  *"Carry the MIT notice by symlink instead of `license-file`"*. Backtick identifiers, paths and
  flags. No `type(scope):` prefixes; this repo does not use Conventional Commits.
- **Body**: blank line, then prose paragraphs hard-wrapped at 80 columns. **No bullet lists** — a
  body that wants bullets is a body that wants paragraphs, one per idea.
- **Say why** (`CONTRIBUTING.md`): what was wrong before, what the change does about it, which
  alternatives were rejected and on what grounds, and any consequence a reader would otherwise
  have to rediscover — a platform caveat, a contract touched, a verification you ran. The subject
  covers *what*; the body earns its length only by covering the rest.
- **Name the documents that moved with the change** when the change obliges one (`CHANGELOG.md`,
  `docs/native-artifacts.md`, `THIRD-PARTY-NOTICES.md`, a spec-map row).
- **No trailers.** No `Co-Authored-By`, no `Claude-Session`, no "Generated with" footer: a commit
  ends with its own last paragraph. `.claude/settings.json` sets `attribution.commit` and
  `attribution.pr` to `""` and `attribution.sessionUrl` to `false`, which is what suppresses them
  for Claude Code; the tool has appended them anyway through past bugs, so if one turns up in a
  message you are about to write, delete it.
- English, like everything else in the repository (rule 6).

## Commands

The devcontainer (`mcr.microsoft.com/devcontainers/rust:1-bookworm`) provides the Rust toolchain;
`.devcontainer/post-install.sh` adds `clippy` and `rustfmt`. It also bind-mounts your host
`~/.ssh` **read-only** at `~/.ssh-host`; `.devcontainer/ssh-setup.sh` (postStart, so it re-arms on
every container start) copies the identity into `~/.ssh` at `0600` and loads it into an ssh-agent,
which is what lets `git push` work from inside the container. A passphrase-locked key is reported
rather than loaded — run `ssh-add ~/.ssh/<key>` yourself in that case. The workspace is scaffolded,
so:

```bash
cargo build --workspace
cargo test --workspace --all-features   # all tests (CI's form; --all-features is what compiles
                                        # and links pdf-ffi's C acceptance tests)
cargo test -p prismpdf-cos                   # one crate
cargo test -p prismpdf-reader xref::trailer  # one test (substring filter)
cargo clippy --workspace --all-targets  # lint — this is the configured check command
cargo fmt --all                         # format (editor formats on save)
```

### Coverage (enterprise standard — hard rule)

**Every crate must keep line coverage above 90%.** No crate ships below it; when adding code,
add the tests to stay over the bar. Measure **workspace-wide** with `cargo-llvm-cov`:

```bash
cargo llvm-cov --workspace --summary-only   # authoritative per-file numbers
cargo llvm-cov --workspace --html           # per-line HTML report
cargo llvm-cov --workspace --show-missing-lines   # exact uncovered lines
```

Always judge coverage from the `--workspace` run: it attributes *every* test to the code it
exercises. A single-crate `cargo llvm-cov -p <crate>` run undercounts any code reached only by
another crate's tests (e.g. `pdf-graphics`'s `Separation` is driven by the `prismpdf` facade's
tests, so in isolation `color.rs` looks far worse than its real 100%).

Every crate clears the bar. `pdf-content`, `prismpdf` and `pdf-reader` sit closest to the floor;
don't
let new code push them under. Re-measure rather than trusting a number written down here.

`pdf-cli` is a **library** (`pdf_cli`) with a thin `main.rs`: the command line is one clap-derive
declaration in `src/lib.rs`, and each subcommand is a function in `src/commands.rs` that writes its
report to a caller-supplied `&mut dyn Write`. Keep it that way — it is what lets `tests/commands.rs`
drive every subcommand in-process instead of spawning the binary, which is how the crate got over
the bar. Only what genuinely needs a process (exit codes, the stdout/stderr split, `--help`,
`PRISMPDF_PASSWORD`) belongs in `tests/cli.rs`.

### Test infrastructure (per `DESIGN.md` §7, EPIC 0/15)

In place: `cargo fuzz` (`fuzz/`, 11 targets — `lexer parser jbig2 cmap function document ccitt lzw
jpx cms revocation` — every one smoke-run in CI; new targets are picked up automatically once
registered in `fuzz/Cargo.toml`),
`cargo-deny` for licenses/vulns, **veraPDF** in CI for PDF/A + PDF/UA conformance
(`corpus/prismpdf-pdfa/`, PASS-only, graded by `crates/pdf/tests/conformance_corpus.rs`), the M18
five-validator panel for base PDF (`crates/pdf/tests/verify_base.rs` → `docs/baselines/verify.md`),
and a versioned `corpus/{valid,malformed,edge}` driving round-trip (load→save→load) tests.

Performance baselines for parse, serialize, merge, and composition live in
`docs/baselines/benchmark.md`; the Criterion targets are attached to their owning crates.

When you add a parser that reads untrusted bytes, add its fuzz target in the same change — that is
`DESIGN.md` §3.4, not a nice-to-have. If the surface is reachable from a whole document, also add a
`gen_corpus` fixture that reaches it: the CI fuzz job seeds the whole-document targets from
`gen_corpus`, so a decoder no corpus file reaches is a decoder those runs never touch.

License: **MIT** (`LICENSE.md`) — permissive; the only condition is that the copyright and
permission notice travels with every copy. Manifests carry `license = "MIT"`, and every crate
directory holds a `LICENSE.md` symlink to the workspace-root file so `cargo package` writes the
notice into each published `.crate` — keep the symlink when you add a crate. Third-party
components keep their own licenses, recorded in `THIRD-PARTY-NOTICES.md` — add an entry there in
the same change that brings third-party code or data in, and state any modification you made to
it.
