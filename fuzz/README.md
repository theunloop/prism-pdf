# fuzz/ — cargo-fuzz harness

Continuous parser fuzzing is a first-class requirement (`DESIGN.md` §3.4, §7), not an afterthought.

This is a **standalone workspace**, deliberately excluded from the root workspace (see the root
`Cargo.toml` `exclude`), so the nightly-only `libfuzzer-sys` toolchain never leaks into a normal
`cargo build`/`cargo test`.

## Run

```bash
cargo install cargo-fuzz          # once
cargo +nightly fuzz run lexer     # fuzz the lexer/parser target
cargo +nightly fuzz list          # list targets
```

Seed the fuzzer from the checked-in corpus once there is something to parse:

```bash
cargo +nightly fuzz run lexer ../corpus/valid ../corpus/malformed ../corpus/edge
```

## Targets

One target per untrusted-input surface (DESIGN.md §3.4). Currently:

- **`lexer`** / **`parser`** — the §7.2 lexer and §7.3 object parser (`pdf-reader`).
- **`jbig2`** — the §7.4.7 `JBIG2Decode` filter (`pdf-filters::jbig2_decode`), with and without a
  separate globals segment stream.
- **`cmap`** — the §9.7 composite-font `/Encoding` CMap and §9.10 `/ToUnicode` CMap parsers
  (`pdf-fonts::{CMap, ToUnicode}`), parse + decode.
- **`document`** — the whole read path from `Document::open` (§7.5 xref + recovery) through text,
  image, font and attachment extraction; the top-level untrusted entry point. In CI it is seeded
  from the generated corpus so the run reaches the deep parsers.
- **`ccitt`** — the §7.4.6 `CCITTFaxDecode` filter (`pdf-filters::ccitt_fax_decode`). `/K`,
  `/Columns`, `/Rows` and the flags are fuzzed alongside the data: they select between three
  decoders and size the buffers, so they are untrusted input too.
- **`lzw`** — the §7.4.4 `LZWDecode` filter plus the shared `/Predictor` post-pass
  (`pdf-filters::lzw_decode`), with `/EarlyChange` and the predictor geometry fuzzed.
- **`jpx`** — the §7.4.9 JP2 container / SIZ header reader (`pdf-filters::jpx_info`), a
  length-prefixed box walk over untrusted bytes.
- **`cms`** — signature verification over untrusted DER (§12.8): `verify_detached`,
  the PAdES-B chain path, and `verify_timestamp_token`. The largest DER surface in the engine, and
  all of it parsed before anything is authenticated.
- **`revocation`** — OCSP (RFC 6960) and CRL (RFC 5280 §5) blobs as they arrive from a document's
  own `/DSS` (§12.8.4.3), matched against the committed test certificate.

Still to add as their surfaces stabilise: the DCT decoder (it is `zune-jpeg`'s surface more than
ours), and the crypt/decrypt path.

Two of the targets are **not** reachable from the whole-document seeds, by design: `cms` and
`revocation` need DER, not a PDF. The filter targets are reachable in principle — and `gen_corpus`
now emits an LZW, an ASCII85-over-Flate, a RunLength and a CCITT document precisely so the seeded
`document` runs reach those decoders — but a dedicated target still gets orders of magnitude more
executions per second on the decoder itself.

The CI `fuzz` job smoke-runs every target listed by `cargo +nightly fuzz list` for a bounded time
on each PR — enough to catch a regression, not a full campaign. New targets are picked up
automatically once registered in `Cargo.toml`.

Crashing inputs and the generated corpus are gitignored; a minimized reproducer for a real bug
belongs in `../corpus/malformed/` with a note.

## Disk footprint

Everything the fuzzer generates is **local, not tracked**: `fuzz/corpus/` (interesting inputs
found so far) and `fuzz/target/` (nightly build artifacts) can each grow to gigabytes and are
gitignored, like `../corpus/external/`. They are working state, not repository bloat — deleting
them is safe but throws away accumulated fuzzing progress, so it is a trade-off, not cleanup. The
committed, curated fixtures live in `../corpus/` (see its README).
