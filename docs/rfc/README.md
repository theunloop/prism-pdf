# docs/rfc — specification copies (local only, never committed)

The ISO and PDF Association documents Prism PDF is built against are **not committed**. The
"sponsored" ISO copies are free to download for your own use, but they carry no redistribution
grant — republishing them from this repository would redistribute ISO copyrighted material. The
tooling and the spec-map therefore reference files that you place here yourself; everything in
this directory except this README is gitignored.

Nothing in the build or the test suite needs these files. They are reading material, plus the
occasional `pdftotext docs/rfc/<file>.pdf` when checking a clause quotation.

## What to download, and where

Free sponsored copies of the ISO standards, the ISO/TS extensions and the application notes are
listed on the PDF Association's **[sponsored standards](https://pdfa.org/sponsored-standards/)**
page. Save each one under the filename below — that is the name `docs/spec-map.md`,
`AGENTS.md` and the PDF/A notes cite.

| Filename | Document |
|---|---|
| `PDF32000_2008.pdf` | ISO 32000-1:2008 — PDF 1.7 ([free from Adobe](https://opensource.adobe.com/dc-acrobat-sdk-docs/pdfstandards/PDF32000_2008.pdf)) |
| `ISO_32000-2_sponsored_EC3.pdf` | ISO 32000-2 — PDF 2.0 |
| `ISO_TS_32001-2022_sponsored_EC3.pdf` | ISO/TS 32001:2022 — SHA-2 / extended digital signatures |
| `ISO_TS_32002-2022_sponsored_EC3.pdf` | ISO/TS 32002:2022 — EdDSA signatures |
| `ISO_TS_32003-2023_sponsored.pdf` | ISO/TS 32003:2023 — AES-GCM encryption |
| `ISO-TS-32004-2024_sponsored.pdf` | ISO/TS 32004:2024 — integrity protection |
| `ISO-TS-32005-2023-sponsored.pdf` | ISO/TS 32005:2023 — structure namespace inclusion |
| `ISO-14289-1-2014-sponsored.pdf` | ISO 14289-1 — PDF/UA-1 |
| `ISO-14289-2-2024-sponsored.pdf` | ISO 14289-2 — PDF/UA-2 |
| `Well-Tagged-PDF-WTPDF-1.0.pdf` | WTPDF 1.0 — Well-Tagged PDF |
| `PDF-Declarations.pdf` | PDF Declarations |
| `PDF20_AN001-BPC.pdf` | PDF 2.0 application note — black point compensation |
| `PDF20_AN002-AF.pdf` | PDF 2.0 application note — associated files |
| `PDF20_AN003-ObjectMetadataLocations.pdf` | PDF 2.0 application note — object metadata locations |

Do not commit them, and do not paste normative clause text into this repository: cite the section
number instead, as `CONTRIBUTING.md` rule 4 requires.
