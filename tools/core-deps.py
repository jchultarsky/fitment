#!/usr/bin/env python3
"""Fail if fitment-core's dependency graph, as the workspace builds it, holds a
crate that deny/core.toml does not allow.

cargo-deny checks fitment-core on its own. But Cargo unifies features across
a build, so a sibling crate that turns on, say, serde_json's preserve_order
pulls indexmap and hashbrown into fitment-core's build without fitment-core
asking for them. This walks the workspace-wide resolve with every feature on,
which is a superset of any real build, and fails closed.
"""

import json
import pathlib
import re
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
ALLOWLIST = ROOT / "deny" / "core.toml"

allowed = set(re.findall(r'crate\s*=\s*"([^"]+)"', ALLOWLIST.read_text()))
# CO4 (docs/research/m6-api.md): crates allowed only under fitment-core's
# `schema` feature. They must not be in the graph a default build resolves.
schema_only = set(
    re.findall(r'crate\s*=\s*"([^"]+)",\s*reason\s*=\s*"schema feature only', ALLOWLIST.read_text())
)


def core_graph(*features):
    """The names of the crates in fitment-core's normal and build graph, as
    the workspace resolves it with `features` (cargo metadata's flags)."""
    metadata = json.loads(
        subprocess.run(
            ["cargo", "metadata", "--format-version", "1", "--locked", *features],
            cwd=ROOT,
            check=True,
            capture_output=True,
            text=True,
        ).stdout
    )
    names = {p["id"]: p["name"] for p in metadata["packages"]}
    nodes = {n["id"]: n for n in metadata["resolve"]["nodes"]}
    roots = [i for i, n in names.items() if n == "fitment-core" and i in metadata["workspace_members"]]
    if len(roots) != 1:
        sys.exit(f"expected one fitment-core workspace member, found {len(roots)}")
    seen, stack = set(), list(roots)
    while stack:
        node = stack.pop()
        if node in seen:
            continue
        seen.add(node)
        for dep in nodes[node]["deps"]:
            # Normal and build edges only; dev-dependencies never reach the library.
            if any(k["kind"] in (None, "build") for k in dep["dep_kinds"]):
                stack.append(dep["pkg"])
    return {names[i] for i in seen}


everything = core_graph("--all-features")
extra = sorted(everything - allowed)
if extra:
    print("fitment-core's unified dependency graph has crates deny/core.toml does not allow:")
    for name in extra:
        print(f"  {name}")
    print("Either a sibling crate enabled a feature that adds them, or a new dependency")
    print("was added. Remove it, or allowlist it in deny/core.toml with a reason.")
    sys.exit(1)
if not schema_only:
    sys.exit("deny/core.toml names no schema-only crate: the CO4 check would be vacuous")
default = core_graph()
leaked = sorted(default & schema_only)
if leaked:
    print("fitment-core's default graph holds crates allowed for the `schema` feature only:")
    for name in leaked:
        print(f"  {name}")
    print("Something turned on fitment-core's (or fitment-catalog's) `schema` feature by default (CO4, SV9).")
    sys.exit(1)
print(f"fitment-core unified graph ok ({len(everything)} crates, all allowlisted; "
      f"the default graph's {len(default)} hold none of the {len(schema_only)} schema-only ones)")
