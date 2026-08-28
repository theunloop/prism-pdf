<!-- Keep this short. The rules live in CONTRIBUTING.md; this is only the checklist. -->

## What this changes

## Why

## Checklist

- [ ] The dependency graph stays one-way (`CONTRIBUTING.md` rule 1).
- [ ] No panicking path reachable from untrusted input; no `unsafe` outside `pdf-ffi` (rules 2, 3).
- [ ] ISO 32000 section cited in the code and test names, and `docs/spec-map.md` updated if this
      takes on a section it does not index yet (rule 4).
- [ ] `CHANGELOG.md` updated if this changes behaviour, the public API, or the C ABI.
- [ ] Ran locally: `cargo fmt --all`, `cargo clippy --workspace --all-targets --all-features`,
      `cargo test --workspace --all-features`, `cargo deny check`.
