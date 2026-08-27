# PDF/A coverage gaps — why the 45 `VAC` rules have no PASS file

This corpus is **PASS-only**: it proves what `make_pdfa`/`make_pdfua` can *produce*. A rule is
`VAC` (see [`checklist.md`](checklist.md)) when our producer never emits the object it governs, so
veraPDF never triggers it. This file explains *why* each `VAC` area is absent and cites the ISO
section that governs it; `docs/spec-map.md` maps those sections to the crate that owns them.

> Verified against the code on **2026-08-18**. Rows here go stale when authoring lands for an area
> without a sample being added — re-check the cited API before trusting a `read-only` marker.

Two patterns recur. Either Prism PDF **reads** a construct but cannot **author** it (a PASS file needs
the authoring side), or the authoring API exists and is tested but `gen_pdfa` builds no sample that
exercises it — those rows are marked `producible`, and closing them is corpus work, not engine work.

## Classification

| Status | Meaning |
|---|---|
| `read-only` | Reading/extraction is implemented; **authoring is not** → can't emit a PASS file. |
| `out-of-scope-v1` | Deliberately deferred for v1 (rendering/transparency), per spec-map. |
| `blocked` | The authoring API exists but a dependency is missing (e.g. a CMYK OutputIntent). |
| `n/a-by-design` | A *conditional* rule that only applies if we use a feature we deliberately don't. |
| `producible` | The library **can** author it, just not from the self-contained example yet. |

## VAC areas

| VAC object(s) | ISO 19005 clause | spec-map (§ISO 32000) | Status | Note |
|---|---|---|---|---|
| `PDAnnot` | 6.3.1–6.3.3 | §12.5 — read ✅, **authoring ✅ (M16 Fase 2)** | **DONE (link/note)** | `Builder::add_annotation` authors link + text-note annotations PDF/A-clean (Print flag §6.3.2; appearance Form XObject for notes §6.3.3). Covered by the `link`/`note` samples. Still `VAC`: 6.3.3 t3 (Widget+Btn appearance subdictionary — needs AcroForm authoring, M16 Fase 4). |
| `PDXForm` | 6.2.9 | §8.10 — read ✅, **authoring ✅ (M16 Fase 2)** | **DONE** | The note annotation's normal appearance is a Form XObject (`form_xobject_stream`: `FormType 1`, BBox, no `/Group`/`/Subtype2` → §6.2.9-clean). |
| `PDAcroForm`, `PDFormField`, `PDWidgetAnnot` | 6.4.1–6.4.2 | §12.7 — read+fill+flatten ✅, **authoring ✅ (M16 Fase 4)** | **DONE (checkbox)** | `Builder::add_form_field` authors an `/AcroForm` with checkbox (`/FT /Btn`) fields: merged field+widget, no `/A`/`/AA` (§6.4.1 t1/t2), no `/NeedAppearances`/`/XFA` (§6.4.1 t3/§6.4.2 t1), `/AP /N` On/Off appearance subdictionary (§6.3.3 t3) drawn as vector graphics (no font). Covered by the `form` sample. ⬜ text/choice/radio fields (a text field's value appearance needs an **embedded** font in `/DR`/`/DA` — a follow-up). |
| `PDSignature`, `PKCSDataObject` | 6.4.3 | §12.8 — sign+verify ✅ | **DONE** | Covered by the `signed` sample: `Document::sign` over a `make_pdfa` base, with a committed throwaway test cert (`examples/test-signer/`) and a fixed signing time → reproducible. Invisible signature (a visible one would draw non-embedded Helvetica, which PDF/A forbids). Fixing this surfaced a writer bug — see below. |
| `PDSigRef`, `PDPerms` | 6.1.12 | §12.8 | read-only | The DocMDP / `/Perms` *certification* variant of a signature; `Document::sign` produces an ordinary approval signature, not a DocMDP transform, so these stay untriggered. |
| `PDAction` | 6.5.1 t1 | §12.6 — **authoring ✅ (M16 Fase 2)** | **DONE** | Link annotations author `URI` and `GoTo` actions (the only ones PDF/A permits, §6.5.1). |
| `PDNamedAction` | 6.5.1 t2 | §12.6 | read-only | Named actions (NextPage/…) are not authored — `make_pdfa` emits no named action, so the rule never triggers. |
| `PDSeparation`, `PDDeviceN` | 6.2.4.4, 6.1.13 | §8.6.6 — `resolve_separation` ✅ (read/tint-transform), **authoring ✅** | **producible** | `Builder::add_separation` attaches a Separation colour space (array + tint transform) to a page's `/Resources /ColorSpace`. No PASS file is committed because `gen_pdfa` does not build a separation sample yet. |
| `ICCInputProfile`, `PDICCBasedCMYK` | 6.2.4.2 | §8.6 — ICCBased read via `/N`, **authoring ✅** | **producible** | `Builder::add_icc_based(name, icc, n)` authors an `[/ICCBased <profile stream>]` colour space (§8.6.5.5). No PASS file is committed because `gen_pdfa` does not build an ICCBased sample yet. |
| `PDDeviceCMYK`, `ICCInputProfile`, `PDICCBasedCMYK` | 6.2.4.3, 6.2.4.2 | `Content::set_fill_cmyk` ✅; §14.11.5 OutputIntent now **selectable** | **producible (bring-your-own profile)** | **M16 Fase 1**: CMYK fill + a caller-chosen CMYK OutputIntent (`make_pdfa_with_output_intent` / `OutputIntentProfile`) make DeviceCMYK conformant — the *code* is done and tested. No PASS file is committed because no CMYK ICC is **bundled**: real CMYK profiles carry vendor copyrights (the free eciCMYK is Heidelberg "all rights reserved", 1.8 MB) and miss this repo's CC0/permissive asset bar (cf. the CC0 sRGB in `THIRD-PARTY-NOTICES.md` §2.1). Supply one via `PRISMPDF_CMYK_ICC=<path>` to `gen_pdfa` to emit + validate a conformant `cmyk-pass.pdf` locally. `PRISMPDF_CMYK_PROBE=1` still writes the non-conformant sRGB-only probe. |
| `PDMaskImage` | 6.2.8 t5 | §8.9 — **authoring ✅ (M16 Fase 3)** | **DONE** | `Image::with_stencil_mask` authors a 1-bit `/ImageMask` stencil (`/Mask`); `Image::from_rgba` authors an 8-bit `/SMask` soft mask (alpha). Covered by the `imagestencil`/`imagealpha` samples. |
| `JPEG2000` | 6.2.8.3 | §7.4.7 / §8.9 — **JPX passthrough only** | read-only | No JPX codec; `from_jpeg` is DCT, not JPX. |
| `CosIIFilter` | 6.1.10 | §7.8 — inline images **read ✅ + authored ✅** | **producible** | The content parser surfaces `BI` as an `Operation` carrying dict + raw data, and `Content::inline_image` emits `BI … ID <data> EI` (§8.9.7). No PASS file is committed because `gen_pdfa` does not build an inline-image sample yet. |
| `PDExtGState`, `PDHalftone`, `CosBM` | 6.2.5, 6.2.10 | §11 *(transparency)* / §10 *(rendering)* — **— out-of-scope v1** | out-of-scope-v1 | Alpha/blend/soft-mask/halftone are rendering-side; `Content` exposes none. Tracked. |
| `PDOCConfig` | 6.9 | §8.11 — **no spec-map row** | read-only | Optional content (layers) is neither read nor authored. **Untracked → added to spec-map.** |
| `ExtensionSchema*` | 6.6.2.3 | §14.3.2 — XMP standard schemas only | n/a-by-design | Extension-schema descriptions are *required only if* the XMP uses non-standard properties. `make_pdfa` emits only standard schemas, so the rule never applies. |

## Bug found while adding the signed sample

The signed sample initially failed veraPDF on **clause 6.1.3 t1 (missing trailer `/ID`)**. Root
cause: `pdf_writer::append_xref_and_trailer` (the incremental-update writer behind `Document::sign`
and `fill_form`) wrote `/Size`/`/Root`/`/Prev`/`/Info` but **never carried the original trailer's
`/ID` forward** — so every incremental update silently stripped `/ID`, which PDF/A requires in every
trailer. Fixed by reusing the original's `/ID` (new `find_trailer_id`), with regression tests. This
is a genuine producer bug the corpus caught, not just a sample tweak.

## Spec-map changes made for this analysis

Three items updated in the spec-map at the time. It has since been reduced to a section →
crate index, so the detail below survives here and in Git history:

1. **§14.11.5** — noted that only an **sRGB** OutputIntent is produced, so `DeviceCMYK`/`DeviceGray`
   content needing a CMYK destination profile cannot be made conformant (the `blocked` row above).
2. **§8.11 Optional content (OCG/OCProperties)** — added a row; it was entirely absent. Neither
   read nor authoring is implemented (out-of-scope v1).
3. **§12.8** — noted that signed output is now PDF/A-validated (the `signed` corpus sample) and that
   the incremental-update `/ID` carry-forward was fixed.

Everything else above is already represented in the spec-map (as read-only / deferred / out-of-scope),
so no further rows were needed — those `VAC` rules are correctly absent, not oversights.

## What would move the needle (for a future producer, not this corpus)

- ~~**CMYK OutputIntent support**~~ → **done (M16 Fase 1)**: `make_pdfa_with_output_intent` +
  `OutputIntentProfile` let a caller supply a CMYK destination profile, making `PDDeviceCMYK`/
  ICC-CMYK content conformant. A committed PASS file still awaits a **redistributable** CMYK ICC
  (the free ones are vendor-copyrighted; see the `PDDeviceCMYK` row) — or a synthetic CC0 profile.
- ~~Authoring of annotations~~ → **done (M16 Fase 2)**: links + text notes with appearance streams.
- ~~Image soft-masks / stencil masks~~ → **done (M16 Fase 3)**: `from_rgba` (alpha `/SMask`) +
  `with_stencil_mask` (`/Mask`).
- ~~Authoring of form fields (AcroForm)~~ → **done (M16 Fase 4)**: checkbox fields (also flips 6.3.3
  t3, Widget+Btn appearance). Remaining: text/choice/radio fields (need embedded-font `/DR`/`/DA`).
- Optional content (layers) → out-of-scope v1 (rendering-side); the only larger read-only area left.
