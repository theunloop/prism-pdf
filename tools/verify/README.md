# `tools/verify/` — M18 base-PDF validator panel

The `verify_base` harness (`crates/pdf/tests/verify_base.rs`) cross-validates Prism PDF's authored
output against a panel of **independent, third-party PDF validators**, because veraPDF only judges
PDF/A·UA·X, not plain PDF 1.4–2.0. A file is "accepted" when the panel agrees; no single tool is
authoritative.

Each tool is resolved from `$PATH` first, then from this directory; missing tools are skipped
cleanly (the harness still runs with whatever resolved, and skips entirely if none do).

## Panel

| Tool | Provides | Install |
|---|---|---|
| `qpdf --check` | structural validity (xref/streams/objects) | `apt-get install qpdf` |
| `mutool info` | MuPDF parser acceptance | `apt-get install mupdf-tools` |
| `gs` | Ghostscript interpreter acceptance | `apt-get install ghostscript` |
| `pdfinfo` | poppler parser acceptance | `apt-get install poppler-utils` |
| `pdfcpu validate` | Go spec-conformance validator | prebuilt binary (below) |

Binaries placed here are **git-ignored** (arch-specific, not committed); only this README is tracked.

## Fetching `pdfcpu` (not in apt)

```sh
# pick the asset for your arch from https://github.com/pdfcpu/pdfcpu/releases/latest
V=0.13.0; ARCH=arm64   # or x86_64
curl -sSL -o /tmp/pdfcpu.txz \
  "https://github.com/pdfcpu/pdfcpu/releases/download/v$V/pdfcpu_${V}_Linux_${ARCH}.tar.xz"
tar xJf /tmp/pdfcpu.txz -C /tmp
cp "$(find /tmp -name pdfcpu -type f | head -1)" tools/verify/pdfcpu && chmod +x tools/verify/pdfcpu
```

## Run

```sh
cargo test -p prismpdf --test verify_base -- --nocapture   # writes target/verify-baseline.json
```
