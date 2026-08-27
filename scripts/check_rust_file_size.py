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
        1_483,
        "public builder model; implementation is already split into sibling modules",
    ),
    "crates/pdf-document/src/lib.rs": (
        1_197,
        "document facade and cohesive public API",
    ),
    "crates/pdf-ffi/src/api/authoring.rs": (
        2_090,
        "C ABI authoring surface; frozen capability-module budget, +10 on "
        "the 2026-08-27 rename reflow: longer identifiers wrap at the 100-col limit. "
        "No logic added.",
    ),
    "crates/pdf-ffi/src/api/collections.rs": (
        1_572,
        "C ABI inspection and collection handles; frozen capability-module budget",
    ),
    "crates/pdf-ffi/src/api/composition.rs": (
        1_365,
        "C ABI composition arena and operations; frozen capability-module budget, +2 on "
        "2026-08-25 for the catch_unwind wrapper that prismpdf_composition_new was missing "
        "(pdf-ffi's no-unwind contract, DESIGN.md §6.1), +2 on 2026-08-27 for the rustfmt "
        "reflow of build_draft's return chain past the 60-col chain width. No logic added.",
    ),
    "crates/pdf-ffi/src/api/core.rs": (
        1_887,
        "shared C ABI types, error boundary, document, object, and buffer operations, "
        "+8 on the 2026-08-27 rename reflow: longer identifiers wrap at the 100-col limit. "
        "No logic added.",
    ),
    "crates/pdf-ffi/src/api/layout.rs": (
        1_267,
        "C ABI legacy flow and layout operations; frozen capability-module budget, +3 on "
        "the 2026-08-27 rename reflow: longer identifiers wrap at the 100-col limit. "
        "No logic added.",
    ),
    "crates/pdf-ffi/src/api/security.rs": (
        1_035,
        "C ABI encryption, signing, and verification surface",
    ),
    "crates/pdf-ffi/src/api/tests.rs": (
        4_896,
        "cross-capability ABI and standalone C acceptance tests, +39 on 2026-08-27: 4 lines "
        "predate the rename (this budget was already stale at 4_861); 35 are reflow "
        "at the 100-col limit. No logic added.",
    ),
    "crates/pdf-fonts/src/standard_metrics.rs": (
        1_993,
        "mostly static Standard-14 font metric tables",
    ),
    "crates/pdf-layout/src/compose.rs": (
        1_032,
        "public composition model and page orchestration",
    ),
    "crates/pdf-layout/src/compose/engine.rs": (
        1_460,
        "private measurement and rendering engine; frozen post-extraction budget",
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
