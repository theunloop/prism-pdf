# Prism PDF PDF/A corpus — testable-points checklist

**Source:** veraPDF 1.30.2 validation profiles (`PDFA-2B/2U/2A/3B/3A.xml`, from `cli-1.30.2.jar`) — the same checks the conformance oracle applies, and the source of truth for *what* to test (ISO 19005 is not in `docs/rfc/`).

Facts only — clause, test number, target object, flavour membership, coverage tag. The normative requirement *wording* is not reproduced (it is veraPDF's GPLv3 / ISO 19005's; this repo is MIT licensed). To read a rule's exact text, open it in the profile XML by `clause`+`testNumber`:

```text
unzip -p ~/.local/opt/verapdf/bin/cli-1.30.2.jar org/verapdf/pdfa/validation/PDFA-2B.xml \
  | grep -A3 'clause="6.2.2" testNumber="1"'
```

**156 distinct rules** across the five part-2/3 profiles (PDF/A-2 and -3, levels B/U/A). The
per-rule table below is that part-2/3 extraction.

## Parts 1 and 4 (added 2026-08)

`make_pdfa` now also produces **PDF/A-1a/1b** (ISO 19005-1, PDF 1.4) and **PDF/A-4/4e/4f**
(ISO 19005-4, PDF 2.0). Their rules come from the same source of truth — the veraPDF profiles
`PDFA-1A/1B.xml` (**135** distinct clause+testNumber rules) and `PDFA-4/4E/4F.xml` (**111**
distinct) — and are enforced by the oracle on every `1a`/`1b`/`4`/`4e`/`4f` PASS file, but at
**flavour granularity**: there is no per-rule proven/`VAC` table for them yet (the part-2/3
table below remains the only rule-level inventory). The part-specific accept paths the corpus
proves: the 1.4 pin + classic xref, no transparency and no attachments at part 1 (plus the
empty `/AP` on invisible signature widgets, 19005-1 §6.9); the 2.0 pin, `pdfaid:rev`, the
`/Info`-free trailer and E/F-only attachments at part 4.

## Coverage legend

The corpus is **PASS-only**: every file is conformant output from `make_pdfa`/`make_pdfua`, validated by veraPDF. **111/156 rules are actively proven** by emitting the object they govern; the rest are **`VAC`** — our producer never emits that object, so the rule is never triggered. See [`coverage-gaps.md`](coverage-gaps.md) for why each `VAC` area is absent (read-only, out-of-scope-v1, or a tracked authoring gap) and which map to spec-map entries.

| Tag | Proven by | Count |
|---|---|---|
| `A-base` | every PASS file (structure, XMP, OutputIntent, DeviceRGB, content) | 53 |
| `A-text` | embedded-font samples (text/tagged/accessible) | 26 |
| `A-image`| raster-image samples (image/imagegray/imagejpeg/figure/imagealpha/imagestencil): incl. soft-mask alpha + 1-bit stencil mask | 7 |
| `A-attach`| PDF/A-3 attachment sample | 5 |
| `A-annot`| annotation samples (link/note): annotations, actions, appearance Form XObjects | 9 |
| `A-form`| interactive-form sample (form): AcroForm, form fields, widget annotations, Btn appearance subdictionary | 5 |
| `A-sign` | digitally-signed sample (signed) | 3 |
| `A-tag` | tagged level-A samples | 3 |
| `VAC` | not emitted by the producer (see coverage-gaps.md) | 45 |

## Clause families

| Family | What it governs |
|---|---|
| 6.1 | File structure: header, xref, trailer/ID, streams, filters, numeric/string limits. |
| 6.2 | Graphics: colour spaces & OutputIntent, images, extended graphics state, rendering intents, content operators, fonts & embedding. |
| 6.3 | Annotations: permitted subtypes, appearance streams, flags. |
| 6.4 | Interactive forms & digital signatures. |
| 6.5 | Actions: permitted/forbidden action types, named actions. |
| 6.6 | Metadata: XMP packet & schema rules (incl. PDF/A extension schemas). |
| 6.7 | Metadata: XMP packet form, pdfaid identification, history. |
| 6.8 | Logical structure / tagging (level A) and associated files (PDF/A-3). |
| 6.9 | Optional content (layers): configuration constraints. |
| 6.10 | Use of embedded files / extension schemas. |
| 6.11 | Use of embedded files / extension schemas. |

## Rules

`Flavours` = profiles including the rule (2b 2u 2a 3b 3a).

| Clause | T | Object | Flavours | Cov |
|---|---|---|---|---|
| 6.1.2 | 1 | CosDocument | 2b 2u 2a 3b 3a | A-base |
| 6.1.2 | 2 | CosDocument | 2b 2u 2a 3b 3a | A-base |
| 6.1.3 | 1 | CosDocument | 2b 2u 2a 3b 3a | A-base |
| 6.1.3 | 2 | CosTrailer | 2b 2u 2a 3b 3a | A-base |
| 6.1.3 | 3 | CosDocument | 2b 2u 2a 3b 3a | A-base |
| 6.1.4 | 2 | CosXRef | 2b 2u 2a 3b 3a | A-base |
| 6.1.6 | 1 | CosString | 2b 2u 2a 3b 3a | A-base |
| 6.1.6 | 2 | CosString | 2b 2u 2a 3b 3a | A-base |
| 6.1.7.1 | 1 | CosStream | 2b 2u 2a 3b 3a | A-base |
| 6.1.7.1 | 2 | CosStream | 2b 2u 2a 3b 3a | A-base |
| 6.1.7.1 | 3 | CosStream | 2b 2u 2a 3b 3a | A-base |
| 6.1.7.2 | 1 | CosFilter | 2b 2u 2a 3b 3a | A-base |
| 6.1.8 | 1 | CosUnicodeName | 2b 2u 2a 3b 3a | A-base |
| 6.1.9 | 1 | CosIndirect | 2b 2u 2a 3b 3a | A-base |
| 6.1.10 | 1 | CosIIFilter | 2b 2u 2a 3b 3a | VAC |
| 6.1.12 | 1 | PDPerms | 2b 2u 2a 3b 3a | VAC |
| 6.1.12 | 2 | PDSigRef | 2b 2u 2a 3b 3a | VAC |
| 6.1.13 | 1 | CosInteger | 2b 2u 2a 3b 3a | A-base |
| 6.1.13 | 2 | CosReal | 2b 2u 2a 3b 3a | A-base |
| 6.1.13 | 3 | CosString | 2b 2u 2a 3b 3a | A-base |
| 6.1.13 | 4 | CosName | 2b 2u 2a 3b 3a | A-base |
| 6.1.13 | 5 | CosReal | 2b 2u 2a 3b 3a | A-base |
| 6.1.13 | 7 | CosDocument | 2b 2u 2a 3b 3a | A-base |
| 6.1.13 | 8 | Op_q_gsave | 2b 2u 2a 3b 3a | A-base |
| 6.1.13 | 9 | PDDeviceN | 2b 2u 2a 3b 3a | VAC |
| 6.1.13 | 10 | CMapFile | 2b 2u 2a 3b 3a | A-text |
| 6.1.13 | 11 | CosBBox | 2b 2u 2a 3b 3a | A-base |
| 6.2.2 | 1 | Op_Undefined | 2b 2u 2a 3b 3a | A-base |
| 6.2.2 | 2 | PDContentStream | 2b 2u 2a 3b 3a | A-base |
| 6.2.3 | 1 | ICCOutputProfile | 2b 2u 2a 3b 3a | A-base |
| 6.2.3 | 2 | OutputIntents | 2b 2u 2a 3b 3a | A-base |
| 6.2.3 | 3 | PDOutputIntent | 2b 2u 2a 3b 3a | A-base |
| 6.2.4.2 | 1 | ICCInputProfile | 2b 2u 2a 3b 3a | VAC |
| 6.2.4.2 | 2 | PDICCBasedCMYK | 2b 2u 2a 3b 3a | VAC |
| 6.2.4.3 | 2 | PDDeviceRGB | 2b 2u 2a 3b 3a | A-base |
| 6.2.4.3 | 3 | PDDeviceCMYK | 2b 2u 2a 3b 3a | VAC |
| 6.2.4.3 | 4 | PDDeviceGray | 2b 2u 2a 3b 3a | A-image |
| 6.2.4.4 | 1 | PDDeviceN | 2b 2u 2a 3b 3a | VAC |
| 6.2.4.4 | 2 | PDSeparation | 2b 2u 2a 3b 3a | VAC |
| 6.2.5 | 1 | PDExtGState | 2b 2u 2a 3b 3a | VAC |
| 6.2.5 | 2 | PDExtGState | 2b 2u 2a 3b 3a | VAC |
| 6.2.5 | 3 | PDExtGState | 2b 2u 2a 3b 3a | VAC |
| 6.2.5 | 4 | PDHalftone | 2b 2u 2a 3b 3a | VAC |
| 6.2.5 | 5 | PDHalftone | 2b 2u 2a 3b 3a | VAC |
| 6.2.5 | 6 | PDHalftone | 2b 2u 2a 3b 3a | VAC |
| 6.2.6 | 1 | CosRenderingIntent | 2b 2u 2a 3b 3a | A-base |
| 6.2.8 | 1 | PDXImage | 2b 2u 2a 3b 3a | A-image |
| 6.2.8 | 2 | PDXImage | 2b 2u 2a 3b 3a | A-image |
| 6.2.8 | 3 | PDXImage | 2b 2u 2a 3b 3a | A-image |
| 6.2.8 | 4 | PDXImage | 2b 2u 2a 3b 3a | A-image |
| 6.2.8 | 5 | PDMaskImage | 2b 2u 2a 3b 3a | A-image |
| 6.2.8.3 | 1 | JPEG2000 | 2b 2u 2a 3b 3a | VAC |
| 6.2.8.3 | 2 | JPEG2000 | 2b 2u 2a 3b 3a | VAC |
| 6.2.8.3 | 3 | JPEG2000 | 2b 2u 2a 3b 3a | VAC |
| 6.2.8.3 | 4 | JPEG2000 | 2b 2u 2a 3b 3a | VAC |
| 6.2.8.3 | 5 | JPEG2000 | 2b 2u 2a 3b 3a | VAC |
| 6.2.9 | 1 | PDXForm | 2b 2u 2a 3b 3a | A-annot |
| 6.2.9 | 2 | PDXForm | 2b 2u 2a 3b 3a | A-annot |
| 6.2.9 | 3 | PDXObject | 2b 2u 2a 3b 3a | A-image |
| 6.2.10 | 1 | CosBM | 2b 2u 2a 3b 3a | VAC |
| 6.2.10 | 2 | PDPage | 2b 2u 2a 3b 3a | A-base |
| 6.2.11.2 | 1 | PDFont | 2b 2u 2a 3b 3a | A-text |
| 6.2.11.2 | 2 | PDFont | 2b 2u 2a 3b 3a | A-text |
| 6.2.11.2 | 3 | PDFont | 2b 2u 2a 3b 3a | A-text |
| 6.2.11.2 | 4 | PDSimpleFont | 2b 2u 2a 3b 3a | A-text |
| 6.2.11.2 | 5 | PDSimpleFont | 2b 2u 2a 3b 3a | A-text |
| 6.2.11.2 | 6 | PDSimpleFont | 2b 2u 2a 3b 3a | A-text |
| 6.2.11.2 | 7 | PDFont | 2b 2u 2a 3b 3a | A-text |
| 6.2.11.3.1 | 1 | PDType0Font | 2b 2u 2a 3b 3a | A-text |
| 6.2.11.3.2 | 1 | PDCIDFont | 2b 2u 2a 3b 3a | A-text |
| 6.2.11.3.3 | 1 | PDCMap | 2b 2u 2a 3b 3a | A-text |
| 6.2.11.3.3 | 2 | CMapFile | 2b 2u 2a 3b 3a | A-text |
| 6.2.11.3.3 | 3 | PDReferencedCMap | 2b 2u 2a 3b 3a | A-text |
| 6.2.11.4.1 | 1 | PDFont | 2b 2u 2a 3b 3a | A-text |
| 6.2.11.4.1 | 2 | Glyph | 2b 2u 2a 3b 3a | A-text |
| 6.2.11.4.2 | 1 | PDType1Font | 2b 2u 2a 3b 3a | A-text |
| 6.2.11.4.2 | 2 | PDCIDFont | 2b 2u 2a 3b 3a | A-text |
| 6.2.11.5 | 1 | Glyph | 2b 2u 2a 3b 3a | A-text |
| 6.2.11.6 | 1 | TrueTypeFontProgram | 2b 2u 2a 3b 3a | A-text |
| 6.2.11.6 | 2 | PDTrueTypeFont | 2b 2u 2a 3b 3a | A-text |
| 6.2.11.6 | 3 | PDTrueTypeFont | 2b 2u 2a 3b 3a | A-text |
| 6.2.11.6 | 4 | TrueTypeFontProgram | 2b 2u 2a 3b 3a | A-text |
| 6.2.11.7.2 | 1 | Glyph | 2u 2a 3a | A-text |
| 6.2.11.7.2 | 2 | Glyph | 2u 2a 3a | A-text |
| 6.2.11.7.3 | 1 | Glyph | 2a 3a | A-text |
| 6.2.11.8 | 1 | Glyph | 2b 2u 2a 3b 3a | A-text |
| 6.3.1 | 1 | PDAnnot | 2b 2u 2a 3b 3a | A-annot |
| 6.3.2 | 1 | PDAnnot | 2b 2u 2a 3b 3a | A-annot |
| 6.3.2 | 2 | PDAnnot | 2b 2u 2a 3b 3a | A-annot |
| 6.3.3 | 1 | PDAnnot | 2b 2u 2a 3b 3a | A-annot |
| 6.3.3 | 2 | PDAnnot | 2b 2u 2a 3b 3a | A-annot |
| 6.3.3 | 3 | PDAnnot | 2b 2u 2a 3b 3a | A-form |
| 6.3.3 | 4 | PDAnnot | 2b 2u 2a 3b 3a | A-annot |
| 6.4.1 | 1 | PDWidgetAnnot | 2b 2u 2a 3b 3a | A-form |
| 6.4.1 | 2 | PDFormField | 2b 2u 2a 3b 3a | A-form |
| 6.4.1 | 3 | PDAcroForm | 2b 2u 2a 3b 3a | A-form |
| 6.4.2 | 1 | PDAcroForm | 2b 2u 2a 3b 3a | A-form |
| 6.4.2 | 2 | CosDocument | 2b 2u 2a 3b 3a | A-base |
| 6.4.3 | 1 | PDSignature | 2b 2u 2a 3b 3a | A-sign |
| 6.4.3 | 2 | PKCSDataObject | 2b 2u 2a 3b 3a | A-sign |
| 6.4.3 | 3 | PKCSDataObject | 2b 2u 2a 3b 3a | A-sign |
| 6.5.1 | 1 | PDAction | 2b 2u 2a 3b 3a | A-annot |
| 6.5.1 | 2 | PDNamedAction | 2b 2u 2a 3b 3a | VAC |
| 6.5.2 | 1 | PDDocument | 2b 2u 2a 3b 3a | A-base |
| 6.5.2 | 2 | PDPage | 2b 2u 2a 3b 3a | A-base |
| 6.6.2.1 | 1 | PDDocument | 2b 2u 2a 3b 3a | A-base |
| 6.6.2.1 | 2 | XMPPackage | 2b 2u 2a 3b 3a | A-base |
| 6.6.2.1 | 3 | XMPPackage | 2b 2u 2a 3b 3a | A-base |
| 6.6.2.1 | 4 | XMPPackage | 2b 2u 2a 3b 3a | A-base |
| 6.6.2.1 | 5 | XMPPackage | 2b 2u 2a 3b 3a | A-base |
| 6.6.2.3.1 | 1 | XMPProperty | 2b 2u 2a 3b 3a | A-base |
| 6.6.2.3.1 | 2 | XMPProperty | 2b 2u 2a 3b 3a | A-base |
| 6.6.2.3.2 | 1 | ExtensionSchemaObject | 2b 2u 2a 3b 3a | VAC |
| 6.6.2.3.3 | 1 | ExtensionSchemasContainer | 2b 2u 2a 3b 3a | VAC |
| 6.6.2.3.3 | 2 | ExtensionSchemaDefinition | 2b 2u 2a 3b 3a | VAC |
| 6.6.2.3.3 | 3 | ExtensionSchemaDefinition | 2b 2u 2a 3b 3a | VAC |
| 6.6.2.3.3 | 4 | ExtensionSchemaDefinition | 2b 2u 2a 3b 3a | VAC |
| 6.6.2.3.3 | 5 | ExtensionSchemaDefinition | 2b 2u 2a 3b 3a | VAC |
| 6.6.2.3.3 | 6 | ExtensionSchemaDefinition | 2b 2u 2a 3b 3a | VAC |
| 6.6.2.3.3 | 7 | ExtensionSchemaProperty | 2b 2u 2a 3b 3a | VAC |
| 6.6.2.3.3 | 8 | ExtensionSchemaProperty | 2b 2u 2a 3b 3a | VAC |
| 6.6.2.3.3 | 9 | ExtensionSchemaProperty | 2b 2u 2a 3b 3a | VAC |
| 6.6.2.3.3 | 10 | ExtensionSchemaProperty | 2b 2u 2a 3b 3a | VAC |
| 6.6.2.3.3 | 11 | ExtensionSchemaValueType | 2b 2u 2a 3b 3a | VAC |
| 6.6.2.3.3 | 12 | ExtensionSchemaValueType | 2b 2u 2a 3b 3a | VAC |
| 6.6.2.3.3 | 13 | ExtensionSchemaValueType | 2b 2u 2a 3b 3a | VAC |
| 6.6.2.3.3 | 14 | ExtensionSchemaValueType | 2b 2u 2a 3b 3a | VAC |
| 6.6.2.3.3 | 15 | ExtensionSchemaValueType | 2b 2u 2a 3b 3a | VAC |
| 6.6.2.3.3 | 16 | ExtensionSchemaField | 2b 2u 2a 3b 3a | VAC |
| 6.6.2.3.3 | 17 | ExtensionSchemaField | 2b 2u 2a 3b 3a | VAC |
| 6.6.2.3.3 | 18 | ExtensionSchemaField | 2b 2u 2a 3b 3a | VAC |
| 6.6.4 | 1 | MainXMPPackage | 2b 2u 2a 3b 3a | A-base |
| 6.6.4 | 2 | PDFAIdentification | 2b 2u 2a 3b 3a | A-base |
| 6.6.4 | 3 | PDFAIdentification | 2b 2u 2a 3b 3a | A-base |
| 6.6.4 | 4 | PDFAIdentification | 2b 2u 2a 3b 3a | A-base |
| 6.6.4 | 5 | PDFAIdentification | 2b 2u 2a 3b 3a | A-base |
| 6.6.4 | 6 | PDFAIdentification | 2b 2u 2a 3b 3a | A-base |
| 6.6.4 | 7 | PDFAIdentification | 2b 2u 2a 3b 3a | A-base |
| 6.7.2.2 | 1 | CosDocument | 2a 3a | A-base |
| 6.7.3.3 | 1 | PDDocument | 2a 3a | A-base |
| 6.7.3.4 | 1 | SENonStandard | 2a 3a | A-tag |
| 6.7.3.4 | 2 | SENonStandard | 2a 3a | A-tag |
| 6.7.3.4 | 3 | SENonStandard | 2a 3a | A-tag |
| 6.7.4 | 1 | CosLang | 2a 3a | A-base |
| 6.8 | 1 | EmbeddedFile | 3b 3a | A-attach |
| 6.8 | 2 | CosFileSpecification | 2b 2u 2a 3b 3a | A-attach |
| 6.8 | 3 | CosFileSpecification | 3b 3a | A-attach |
| 6.8 | 4 | CosFileSpecification | 3b 3a | A-attach |
| 6.8 | 5 | EmbeddedFile | 2b 2u 2a | A-attach |
| 6.9 | 1 | PDOCConfig | 2b 2u 2a 3b 3a | VAC |
| 6.9 | 2 | PDOCConfig | 2b 2u 2a 3b 3a | VAC |
| 6.9 | 3 | PDOCConfig | 2b 2u 2a 3b 3a | VAC |
| 6.9 | 4 | PDOCConfig | 2b 2u 2a 3b 3a | VAC |
| 6.10 | 1 | PDDocument | 2b 2u 2a 3b 3a | A-base |
| 6.10 | 2 | PDPage | 2b 2u 2a 3b 3a | A-base |
| 6.11 | 1 | CosDocument | 2b 2u 2a 3b 3a | A-base |
