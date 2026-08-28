# spec-map — ISO section → crate

An **index**, not a status report. It answers one question: *which crate owns this clause of the
standard?* Use it to find the code from a section number, and to pick the section number to cite
when writing code, comments or tests.

It deliberately records no progress, no milestones and no roadmap. For *what shipped*, read
[`../CHANGELOG.md`](../CHANGELOG.md); for *what is next*, [`../ROADMAP.md`](../ROADMAP.md); for the
design behind the layering, [`../DESIGN.md`](../DESIGN.md).

Orient yourself by **section number** — stable across editions — never by page, which varies from
copy to copy. Numbering below follows **ISO 32000-1 (PDF 1.7)**; 2.0 renumbers some sub-sections,
so confirm against your own copy when the exact sub-section matters.

## Reference documents

The specifications themselves are not committed. [`rfc/README.md`](rfc/README.md) lists every
document, where to obtain it, and the filename to save it under.

| Short | Document | Scope |
|---|---|---|
| **1.7** | ISO 32000-1:2008 | Base reference; the section numbering used here |
| **2.0** | ISO 32000-2:2020 | Successor edition — added features, errata, clarifications |
| **TS 32001–32005** | ISO/TS extensions | Hashes, signatures, AES-GCM, integrity, structure namespaces |
| **UA-1 / UA-2** | ISO 14289-1:2014 / 14289-2:2024 | PDF/UA accessibility conformance |

Crate names are the directories under `crates/`.

## §7 — Syntax

| § | Topic | Crate |
|---|---|---|
| 7.2 | Lexical conventions (whitespace, delimiters, comments) | `pdf-cos` · `pdf-reader` |
| 7.3.2 | Boolean | `pdf-cos` |
| 7.3.3 | Numeric (Integer, Real) | `pdf-cos` |
| 7.3.4 | String (literal §7.3.4.2, hex §7.3.4.3, escapes) | `pdf-cos` (model) · `pdf-reader` (parse) |
| 7.3.5 | Name (`#xx` escapes) | `pdf-cos` · `pdf-reader` |
| 7.3.6 | Array | `pdf-cos` |
| 7.3.7 | Dictionary | `pdf-cos` |
| 7.3.8 | Stream | `pdf-cos` |
| 7.3.9 | Null | `pdf-cos` |
| 7.3.10 | Indirect objects (`n g obj` / `R`) | `pdf-cos` (`ObjectId`) · `pdf-reader` |
| 7.4.1 | Filters — general | `pdf-filters` |
| 7.4.2 | ASCIIHexDecode | `pdf-filters` |
| 7.4.3 | ASCII85Decode | `pdf-filters` |
| 7.4.4 | LZW and Flate (incl. predictors §7.4.4.4) | `pdf-filters` |
| 7.4.5 | RunLengthDecode | `pdf-filters` |
| 7.4.6 | CCITTFaxDecode | `pdf-filters` |
| 7.4.7 | JBIG2Decode | `pdf-filters` |
| 7.4.8 | DCTDecode (JPEG) | `pdf-filters` |
| 7.4.9 | JPXDecode (JPEG 2000) | `pdf-filters` |
| 7.4.10 | Crypt filter | `pdf-filters` · `pdf-crypto` |
| 7.5.2 | File header | `pdf-reader` |
| 7.5.3 | File body | `pdf-reader` |
| 7.5.4 | Cross-reference table (ASCII xref) | `pdf-reader` (read) · `pdf-writer` (write) |
| 7.5.5 | File trailer (`/Root`, `/Size`, `/Prev`) | `pdf-reader` |
| 7.5.6 | Incremental updates | `pdf-reader` (read) · `pdf-writer` (write) |
| 7.5.7 | Object streams | `pdf-reader` (read) · `pdf-writer` (write) |
| 7.5.8 | Cross-reference streams (`/W`, type 0/1/2 entries) | `pdf-reader` (read) · `pdf-writer` (write) |
| 7.6 | Encryption (§7.6.3 standard handler, §7.6.4 public-key) | `pdf-crypto` |
| 7.7.2 | Document catalog | `pdf-document` |
| 7.7.3 | Page tree (inheritable attributes §7.7.3.4) | `pdf-document` |
| 7.7.4 | Name dictionary / name trees | `pdf-document` |
| 7.8 | Content streams and resources | `pdf-content` |
| 7.9 | Common data structures (text strings, dates, …) | `pdf-cos` |
| 7.10 | Functions (types 0/2/3/4) | `pdf-graphics` |
| 7.11 | Embedded file streams / file specifications | `pdf-document` |

## §8 — Graphics

| § | Topic | Crate |
|---|---|---|
| 8.3 | Coordinate systems and matrices | `pdf-content` |
| 8.4 | Graphics state | `pdf-content` |
| 8.5 | Path construction and painting | `pdf-graphics` |
| 8.6 | Colour spaces (DeviceGray/RGB/CMYK, ICCBased, …) | `pdf-graphics` |
| 8.7 | Patterns and shadings | `pdf-graphics` |
| 8.9 / 8.10 | Images and Form XObjects | `pdf-graphics` |
| 8.11 | Optional content (OCG, `/OCProperties`, layers) | — |

## §9 — Text and fonts

| § | Topic | Crate |
|---|---|---|
| 9.3 | Text state | `pdf-content` |
| 9.4 | Text objects and operators (`BT`/`ET`, `Tj`, `TJ`, …) | `pdf-content` |
| 9.6 | Simple fonts (Type1, TrueType, Standard 14) | `pdf-fonts` |
| 9.7 | Composite fonts (Type0/CID, CMap) | `pdf-fonts` |
| 9.8 | Font descriptors | `pdf-fonts` |
| 9.9 | Embedded font programs and subsetting | `pdf-fonts` |
| 9.10 | `ToUnicode` and text extraction | `pdf-fonts` |

## §10–§11 — Rendering and transparency

| § | Topic | Crate |
|---|---|---|
| 10 | Rendering | — |
| 11 | Transparency and blend modes | — |

No crate owns these: rendering pages to pixels is outside the v1 scope
([`../DESIGN.md`](../DESIGN.md)).

## §12 — Interactive features

| § | Topic | Crate |
|---|---|---|
| 12.3 | Outlines / bookmarks | `pdf-document` |
| 12.5 | Annotations | `pdf-document` |
| 12.7 | Interactive forms (AcroForm) | `pdf-document` |
| 12.8 | Digital signatures | `pdf-crypto` · `pdf-document` |

## §14 — Document interchange

| § | Topic | Crate |
|---|---|---|
| 14.3.2 | Metadata streams / XMP | `pdf-standards` |
| 14.4 | File identifiers (`/ID`) | `pdf-writer` |
| 14.7 | Tagged PDF / logical structure | `pdf-document` · `pdf-content` · `pdf-layout` |
| 14.8 | Accessibility / PDF/UA | `pdf-standards` |
| 14.11.5 | OutputIntents (PDF/A) | `pdf-standards` |
| 14.12 | Document parts | `pdf-document` |
| 14.13 | Associated files (`/AF`, PDF/A-3) | `pdf-document` · `pdf-standards` |

## Conformance typologies — ISO family → crate

The subset standards ("typologies") each pin a base PDF edition and add a rulebook on top. Same
contract as the section tables: an index, not a status report. A `—` crate cell means no crate
owns that part yet — those gaps are roadmap material ([`../ROADMAP.md`](../ROADMAP.md)) — and the
documents themselves are listed in [`rfc/README.md`](rfc/README.md).

| Family | Part | Flavours | Base PDF | Crate |
|---|---|---|---|---|
| **PDF/A** — archival (ISO 19005) | 19005-1:2005 | 1a, 1b | PDF 1.4 | `pdf-standards` |
| | 19005-2:2011 | 2a, 2b, 2u | ISO 32000-1 | `pdf-standards` |
| | 19005-3:2012 | 3a, 3b, 3u | ISO 32000-1 | `pdf-standards` |
| | 19005-4:2020 | 4, 4e, 4f | ISO 32000-2 | `pdf-standards` |
| **PDF/UA** — accessibility (ISO 14289) | 14289-1:2014 | UA-1 | ISO 32000-1 | `pdf-standards` |
| | 14289-2:2024 | UA-2 | ISO 32000-2 | `pdf-standards` |
| **PDF/X** — print exchange (ISO 15930) | 15930-1…-6 (2001–2003) | X-1a, X-2, X-3 | PDF 1.3 / 1.4 | — |
| | 15930-7:2010 | X-4, X-4p | PDF 1.6 | — |
| | 15930-8:2010 | X-5g, X-5n, X-5pg | PDF 1.6 | — |
| | 15930-9:2020 | X-6, X-6n, X-6p | ISO 32000-2 | — |
| **PDF/VT** — variable/transactional print (ISO 16612) | 16612-2:2010 | VT-1, VT-2 | PDF 1.6, over PDF/X-4/-5 | — |
| | 16612-3:2020 | VT-3 | ISO 32000-2, over PDF/X-6 | — |
| **PDF/E** — engineering (ISO 24517) | 24517-1:2008 | E-1 | PDF 1.6 | — |
| **PDF/R** — raster scans (ISO 23504) | 23504-1:2020 | R-1 | ISO 32000-1 subset | — |

Two facts here contradict most third-party summaries, deliberately: PDF/X-4/-5 and PDF/VT-1/-2
are built on **PDF 1.6** (the Adobe reference), not on ISO 32000-1 — only PDF/X-6 and PDF/VT-3
sit on an ISO base. And ISO 24517 has no part 2: the engineering profile continued as
**PDF/A-4e** (ISO 19005-4 Annex B), which `pdf-standards` implements — so the PDF/E hole is
covered by its successor, not open.

Typologies on the same base stack: one file can conform to PDF/A-4, PDF/UA-2 and PDF/X-6
simultaneously (CI already validates PDF/A flavours together with `ua1`/`ua2`).
