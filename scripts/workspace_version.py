#!/usr/bin/env python3
"""Keep the workspace version and the internal dependency requirements in lockstep.

`cargo publish` refuses a dependency without a version requirement, so every internal crate in
`[workspace.dependencies]` carries one alongside its `path`. Cargo does not derive that number
from `[workspace.package].version` — nothing fails locally when the two drift, because a path
dependency resolves by path. The mismatch only surfaces at publish time, one crate at a time,
against an index that has already accepted the crates published before it.

A prerelease makes the trap sharper: `0.5.0` does not match `0.5.0-alpha.1`, so a requirement left
at the release version silently excludes the prerelease it is supposed to point at.

    scripts/workspace_version.py --check     # CI: assert every requirement matches
    scripts/workspace_version.py 0.5.0-alpha.1   # bump all of them together
"""

from pathlib import Path
import re
import sys


CARGO_TOML = Path(__file__).resolve().parent.parent / "Cargo.toml"

# `<key> = { package = "...", version = "X", path = "crates/..." }` — the internal crates. The
# facade has no `package` key (its library name already matches), hence the optional group.
INTERNAL = re.compile(
    r'^(?P<head>[\w-]+ = \{ (?:package = "[\w-]+", )?version = ")(?P<version>[^"]+)(?P<tail>", path = "crates/[\w-]+" \})$',
    re.M,
)
WORKSPACE_VERSION = re.compile(r'^(?P<head>version = ")(?P<version>[^"]+)(?P<tail>")$', re.M)
SEMVER = re.compile(r"^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$")


def workspace_version(text: str) -> str:
    """The `[workspace.package]` version — the first bare `version = ` in the file."""
    match = WORKSPACE_VERSION.search(text)
    if match is None:
        sys.exit(f"{CARGO_TOML}: no [workspace.package] version found")
    return match.group("version")


def check(text: str) -> int:
    expected = workspace_version(text)
    wrong = [m for m in INTERNAL.finditer(text) if m.group("version") != expected]
    if not wrong:
        print(f"workspace version {expected}: all internal dependency requirements match")
        return 0
    print(f"[workspace.package].version is {expected}, but these requirements disagree:")
    for match in wrong:
        print(f"  {match.group(0).strip()}")
    print(f"\nFix with: scripts/workspace_version.py {expected}")
    return 1


def bump(text: str, version: str) -> str:
    if not SEMVER.match(version):
        sys.exit(f"{version!r} is not a semantic version (e.g. 0.5.0 or 0.5.0-alpha.1)")
    text = WORKSPACE_VERSION.sub(rf'\g<head>{version}\g<tail>', text, count=1)
    text, count = INTERNAL.subn(rf'\g<head>{version}\g<tail>', text)
    print(f"set [workspace.package].version and {count} internal requirements to {version}")
    return text


def main() -> int:
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    text = CARGO_TOML.read_text()
    if sys.argv[1] == "--check":
        return check(text)
    CARGO_TOML.write_text(bump(text, sys.argv[1]))
    print("Now update CHANGELOG.md with a matching released heading before tagging (RELEASING.md).")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
