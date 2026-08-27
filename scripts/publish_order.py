#!/usr/bin/env python3
"""Print the workspace crates in the order crates.io can accept them.

A registry only accepts a crate whose dependencies it already has, so the workspace has to go out
leaf-first. The order is a property of the dependency graph, not a list worth maintaining by hand:
this derives it, and fails loudly on a cycle rather than emitting an order that cannot work.

Reads the manifests directly (no `cargo metadata`), so it runs anywhere python does.

    scripts/publish_order.py            # one package name per line, leaf first
"""

from pathlib import Path
import re
import sys


ROOT = Path(__file__).resolve().parent.parent

# In `[workspace.dependencies]`, the key is the extern crate name and `package` (when present) is
# the crates.io name. A member's `[dependencies]` refers to the key, so resolving one to the other
# is what turns `pdf-cos.workspace = true` into "depends on the package prismpdf-cos".
WORKSPACE_DEP = re.compile(
    r'^(?P<key>[\w-]+) = \{ (?:package = "(?P<package>[\w-]+)", )?version = "[^"]+", path = "crates/[\w-]+" \}$',
    re.M,
)


def key_to_package() -> dict[str, str]:
    text = (ROOT / "Cargo.toml").read_text()
    return {m.group("key"): m.group("package") or m.group("key") for m in WORKSPACE_DEP.finditer(text)}


def member_dependencies(manifest: Path, keys: dict[str, str]) -> tuple[str, set[str]]:
    """The member's package name and the internal packages it needs published before it.

    `[dev-dependencies]` are deliberately excluded: cargo strips a dev dependency that carries no
    version requirement, and this workspace has no internal ones. Build dependencies would count,
    but no member has any.
    """
    text = manifest.read_text()
    name = re.search(r'^\[package\]\nname = "(?P<name>[\w-]+)"$', text, re.M).group("name")
    body = text.split("\n[dependencies]\n", 1)
    needs: set[str] = set()
    if len(body) == 2:
        section = re.split(r"^\[", body[1], maxsplit=1, flags=re.M)[0]
        for line in section.splitlines():
            key = line.split(".", 1)[0].split(" ", 1)[0].strip()
            if key in keys:
                needs.add(keys[key])
    return name, needs


def main() -> int:
    keys = key_to_package()
    graph = dict(member_dependencies(m, keys) for m in sorted((ROOT / "crates").glob("*/Cargo.toml")))

    ordered: list[str] = []
    done: set[str] = set()
    while len(ordered) < len(graph):
        # Alphabetical among the crates that are ready, so the order is stable run to run.
        ready = sorted(n for n, needs in graph.items() if n not in done and needs <= done)
        if not ready:
            remaining = sorted(set(graph) - done)
            sys.exit(f"dependency cycle or missing member among: {', '.join(remaining)}")
        ordered.extend(ready)
        done.update(ready)

    print("\n".join(ordered))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
