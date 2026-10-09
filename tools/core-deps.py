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
metadata = json.loads(
    subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--locked", "--all-features"],
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

extra = sorted({names[i] for i in seen} - allowed)
if extra:
    print("fitment-core's unified dependency graph has crates deny/core.toml does not allow:")
    for name in extra:
        print(f"  {name}")
    print("Either a sibling crate enabled a feature that adds them, or a new dependency")
    print("was added. Remove it, or allowlist it in deny/core.toml with a reason.")
    sys.exit(1)
print(f"fitment-core unified graph ok ({len(seen)} crates, all allowlisted)")
