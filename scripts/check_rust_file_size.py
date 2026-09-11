#!/usr/bin/env python3
"""Reject oversized first-party Rust source files.

Line count is a readability signal, not a design verdict. Existing large files are
therefore recorded with a frozen budget and a short rationale. New files must stay
at or below MAX_LINES; an exception should only be added after review.
"""

from pathlib import Path
import sys


MAX_LINES = 1_000

# These budgets capture the 2026-08-24 baseline. Do not raise them to make CI pass:
# split the module, move its tests, or document why a reviewed exception is better.
EXCEPTIONS = {
    "crates/pdf-document/src/builder.rs": (
        1_503,
        "public builder model; implementation is already split into sibling modules, "
        "+12 on 2026-09-09 for `FormFieldSpec::Signature`, the empty signature field a "
        "template offers a later signer, and +8 for `CidFont::cff`, which tells the writer "
        "whether a program is CFF- or glyf-flavoured (§9.9). Both are model, not logic: the "
        "code that acts on them lives in the sibling modules.",
    ),
    "crates/pdf-document/src/lib.rs": (
        1_197,
        "document facade and cohesive public API",
    ),
    "crates/pdf-ffi/src/api/authoring.rs": (
        2_172,
        "C ABI authoring surface; frozen capability-module budget, +10 on "
        "the 2026-08-27 rename reflow: longer identifiers wrap at the 100-col limit, "
        "+2 on 2026-08-28 for the `Consumes on success` ownership markers on "
        "builder_add_page_spec and builder_add_structure_node, +80 on 2026-09-09 for two "
        "new exports and their contracts: builder_embed_cid_font (the export the header had "
        "cited since 0.4.0 without declaring) and builder_add_signature_field. Splitting the "
        "module would fragment the authoring surface a binding reads top to bottom.",
    ),
    "crates/pdf-ffi/src/api/collections.rs": (
        1_621,
        "C ABI inspection and collection handles; frozen capability-module budget, +49 on "
        "2026-09-09 for form_field_rect and form_field_page_index, the widget geometry a "
        "caller previously had to walk /AcroForm by hand to find (§12.5.2, §12.7.3.1).",
    ),
    "crates/pdf-ffi/src/api/composition.rs": (
        1_423,
        "C ABI composition arena and operations; frozen capability-module budget, +2 on "
        "2026-08-25 for the catch_unwind wrapper that prismpdf_composition_new was missing "
        "(pdf-ffi's no-unwind contract, DESIGN.md §6.1), +2 on 2026-08-27 for the rustfmt "
        "reflow of build_draft's return chain past the 60-col chain width, +3 on 2026-08-28 "
        "for the `Finalises` ownership marker on composition_build, +49 on 2026-09-09 for "
        "composition_into_builder, which hands a finalised composition over as an owned "
        "Builder so /Info, attachments, outlines and the conformance passes can reach it, "
        "+6 on 2026-09-10 for the doc comment on composition_status explaining why it records "
        "the ComposeError message: one Layout status covers six causes, so dropping the "
        "message left a caller nothing to debug an element tree from.",
    ),
    "crates/pdf-ffi/src/api/core.rs": (
        1_889,
        "shared C ABI types, error boundary, document, object, and buffer operations, "
        "+8 on the 2026-08-27 rename reflow: longer identifiers wrap at the 100-col limit, "
        "+2 on 2026-08-28 for the `Consumes on success` ownership marker on edit_commit. "
        "No logic added.",
    ),
    "crates/pdf-ffi/src/api/layout.rs": (
        1_300,
        "C ABI legacy flow and layout operations; frozen capability-module budget, +3 on "
        "the 2026-08-27 rename reflow: longer identifiers wrap at the 100-col limit, "
        "+9 on 2026-08-28: the `Consumes always` markers on flow_build and "
        "flow_into_builder, whose failure path frees the handle, and the ordering note on "
        "flow_embed_font. These comments are the fix for a contract a binding could only "
        "read as a double free; splitting the module to fit them would fragment the export "
        "surface for no readability gain, +24 on 2026-09-09 for image_source_from_png and "
        "the PNG size bound its contract has to state.",
    ),
    "crates/pdf-ffi/src/api/security.rs": (
        1_357,
        "C ABI encryption, signing, and verification surface, +105 on 2026-09-09 for three "
        "signing-settings exports and their contracts: sign_settings_add_certificate (chain "
        "certificates in the CMS), sign_settings_set_field_name (sign into an existing /Sig "
        "field) and sign_settings_set_appearance_image. +217 on 2026-09-11 for the caption "
        "style: an opaque handle with one setter per aspect, matching open_options, plus the "
        "styled appearance export that applies it. The historical caption was Helvetica 8pt on "
        "one unwrapped line beside the graphic, which integrators worked around by rasterising "
        "their metadata into the graphic itself.",
    ),
    "crates/pdf-ffi/src/api/tests.rs": (
        5_578,
        "cross-capability ABI and standalone C acceptance tests, +39 on 2026-08-27: 4 lines "
        "predate the rename (this budget was already stale at 4_861); 35 are reflow "
        "at the 100-col limit. No logic added. +496 on 2026-09-09 covering the eight exports "
        "this prerelease adds — CID font embedding, composition_into_builder, form-field "
        "geometry, certificate chains, PNG image sources, appearance images and signing into "
        "a named field. This is the one exception here that a split would genuinely improve: "
        "the module is tests, so `api/tests/` per capability costs nothing but the move. "
        "+76 on 2026-09-10 for composition_layout_failures_report_their_cause, which drives "
        "both finalising calls into a failure and asserts the diagnostic carries the cause. "
        "+110 on 2026-09-11 for a_styled_caption_reaches_the_appearance_stream, which drives "
        "every caption setter through to the emitted appearance stream and checks the "
        "null-handle paths.",
    ),
    "crates/pdf-fonts/src/standard_metrics.rs": (
        1_993,
        "mostly static Standard-14 font metric tables",
    ),
    "crates/pdf-layout/src/compose.rs": (
        1_070,
        "public composition model and page orchestration, +13 on 2026-09-09: "
        "PreparedComposition::into_builder, the handover a caller takes to reach everything "
        "Builder offers, and the §9.9 outline check embedded_font now makes before it "
        "accepts a program. +25 on 2026-09-10 for the ComposeError messages, which now name "
        "what to look at and say whether the fault is the caller's input or a violation of "
        "this engine's own measure/draw protocol — the only channel that can, since every "
        "variant reaches the ABI as the single status Layout.",
    ),
    "crates/pdf-layout/src/compose/engine.rs": (
        1_478,
        "private measurement and rendering engine; frozen post-extraction budget, +18 on "
        "2026-09-10 splitting DecoratedNode::constraints into validate, fits_offered_height "
        "and the width checks that remain. A box taller than the room left on the page is a "
        "page break rather than an error, and separating the three keeps the finite/negative "
        "validation ahead of a height comparison that reads every NaN as `does not fit`.",
    ),
}


def line_count(path: Path) -> int:
    with path.open("rb") as source:
        return sum(1 for _ in source)


def main() -> int:
    root = Path(__file__).resolve().parent.parent
    failures: list[str] = []

    rust_files = sorted((root / "crates").glob("*/**/*.rs"))
    for path in rust_files:
        relative = path.relative_to(root).as_posix()
        count = line_count(path)
        exception = EXCEPTIONS.get(relative)
        budget = exception[0] if exception else MAX_LINES
        if count > budget:
            failures.append(f"{relative}: {count} lines (budget {budget})")

    stale = sorted(set(EXCEPTIONS) - {p.relative_to(root).as_posix() for p in rust_files})
    failures.extend(f"stale exception: {path}" for path in stale)

    if failures:
        print("Rust source-size check failed:", file=sys.stderr)
        for failure in failures:
            print(f"  {failure}", file=sys.stderr)
        print(
            "Split the module or update the exception only after architectural review.",
            file=sys.stderr,
        )
        return 1

    oversized = sum(line_count(root / path) > MAX_LINES for path in EXCEPTIONS)
    print(
        f"Rust source-size check passed: {len(rust_files)} files; "
        f"{oversized} reviewed exceptions above {MAX_LINES} lines."
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
