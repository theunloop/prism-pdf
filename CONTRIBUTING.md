# Contributing to Prism PDF

Thanks for helping build Prism PDF. This document covers the rules that are specific to this
project; for the big picture read [`DESIGN.md`](./DESIGN.md), and for the canonical vocabulary
read [`GLOSSARY.md`](./GLOSSARY.md).

## Ground rules that are not negotiable

These come straight from `DESIGN.md` §3/§6/§7 and are enforced in review (and partly in CI):

1. **One-way dependency graph.** Crates depend only on lower layers
   (`cos → nothing`, `filters → cos`, `reader/writer → cos,filters`, …). A PR that makes a low
   layer depend on a higher one will be rejected. The graph is documented in `AGENTS.md` and in
   each crate's `Cargo.toml`.
2. **No panics on untrusted input.** The parser treats every byte as hostile. No `unwrap()`,
   `expect()`, `panic!`, indexing that can panic, or arithmetic that can overflow on a path that
   can be reached from input. `unwrap_used`/`expect_used` are denied in CI. In tests, opt out
   locally with `#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]`.
3. **Zero `unsafe` in the core.** Every crate except `pdf-ffi` carries `#![forbid(unsafe_code)]`.
   `unsafe` lives only in `pdf-ffi`, where it is confined and commented.
4. **Cite the spec.** When you implement or change behaviour tied to ISO 32000, put the section
   number in a comment and the test name (e.g. `// §7.5.5 trailer`).
   [`docs/spec-map.md`](./docs/spec-map.md) indexes each section to the crate that owns it — add a
   row when you take on a section it does not list yet.
5. **Reuse over reimplementation** for codecs/crypto/fonts. Add the dependency version to
   `[workspace.dependencies]` in the root `Cargo.toml` and opt in per crate with
   `<crate>.workspace = true`.
6. **English is the repository language.** All code, comments, commit messages, tests, and
   documentation are written in English — including internal design notes. A reader following
   `README.md` → `DESIGN.md` → `docs/spec-map.md` should never change language.

## Before you push

```bash
cargo fmt --all
cargo clippy --workspace --all-targets --all-features   # CI denies warnings
python3 scripts/check_rust_file_size.py                  # 1,000-line source budget
cargo test --workspace --all-features                    # --all-features runs the C ABI's C tests
cargo deny check                                         # licenses + advisories
```

CI additionally runs the test suite on Linux/macOS/Windows and a `cargo check` against the
MSRV (Rust 1.88, edition 2024). Don't raise the MSRV casually — it's a published guarantee.

## Architecture decisions

Significant, hard-to-reverse decisions are recorded as **ADRs**, referenced by number from the
code that implements them. The four that govern the COS object model — **ADR-0001** (objects
never resolve references), **ADR-0002** (O(1) clones), **ADR-0003** (canonical, not byte-faithful
leaves), **ADR-0004** (inert streams) — are stated in the module docs of
[`crates/pdf-cos/src/lib.rs`](./crates/pdf-cos/src/lib.rs), next to the types they constrain.
Before proposing a change that contradicts one, read it.

If your change *is* a new such decision and it does not belong to one crate, add the
next-numbered ADR under `docs/adr/` in the same one- to three-sentence style and link it from the
relevant spec-map row. Only record decisions that are hard to reverse, surprising without
context, and the result of a real trade-off.

## Commits & licensing

Prism PDF is released under the **MIT license** ([`LICENSE.md`](./LICENSE.md)).

By submitting a contribution you:

1. **License it under the same terms** as the rest of the Software — MIT, to every downstream
   recipient, with no additional restrictions.
2. **Confirm you may grant those rights**: the work is yours, or you have your employer's
   permission to submit it under these terms.

Third-party code brought into the tree is subject to the licensing rules in
[`THIRD-PARTY-NOTICES.md`](./THIRD-PARTY-NOTICES.md) — record its provenance and license there in
the same change, and note any modification you made to it.

Keep commits focused and write messages that explain *why*, not just *what*. The message style
the repository uses — imperative 72-column subject, prose body wrapped at 80, no bullet lists,
no trailers — is spelled out under "Commit messages" in [`AGENTS.md`](./AGENTS.md).
