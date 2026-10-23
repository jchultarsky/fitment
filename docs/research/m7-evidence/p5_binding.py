#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11,<3.14"
# dependencies = ["cadquery-ocp-novtk==8.0.1.1.0"]
# ///
"""P5: how the helper can find the solid fitment names (FB2).

    docs/research/m7-evidence/p5_binding.py FILE... > p5_binding.txt

1. By its own #id: after TransferRoots, the transfer reader's ShapeResult of each MANIFOLD_SOLID_BREP or
   BREP_WITH_VOIDS entity (found by its rank, as tools/verify-occt.py's file_ranks).
2. By its faces: every XCAF leaf's faces bound to their #ids through EntityFromShapeResult (the oracle's
   face_id); a leaf is identified by (its faces' #ids, its transform). Counts the leaves whose every face
   binds, and whether the keys are unique.
"""
import importlib.util
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
spec = importlib.util.spec_from_file_location("oracle", ROOT / "tools" / "verify-occt.py")
oracle = importlib.util.module_from_spec(spec)
spec.loader.exec_module(oracle)

from OCP.IFSelect import IFSelect_RetDone  # noqa: E402
from OCP.STEPControl import STEPControl_Reader  # noqa: E402
from OCP.TopAbs import TopAbs_FACE  # noqa: E402

oracle.quiet()
for path in map(Path, sys.argv[1:]):
    ranks = oracle.file_ranks(path)
    text = path.read_text(encoding="latin-1")
    ids = [int(m.group(1)) for m in re.finditer(r"^#(\d+)\s*=\s*(MANIFOLD_SOLID_BREP|BREP_WITH_VOIDS)\b", text, re.M)]
    r = STEPControl_Reader()
    r.ReadFile(str(path))
    r.TransferRoots()
    model, tr = r.StepModel(), r.WS().TransferReader()
    by_id = sum(1 for n in ids if ranks.get(n) and tr.ShapeResult(model.Value(ranks[n])) is not None
                and not tr.ShapeResult(model.Value(ranks[n])).IsNull())
    leaves, face_id = oracle.xcaf_leaves(path, bind=True)
    keys, unbound = [], 0
    for _, m, shape in leaves:
        faces = [face_id(oracle.as_face(f)) for f in oracle.explore(shape, TopAbs_FACE)]
        if not faces or any(f is None for f in faces):
            unbound += 1
        keys.append((tuple(sorted(f for f in faces if f is not None)),
                     tuple(round(v, 9) for row in m for v in row)))
    print(f"{path.name}: {len(ids)} solid entities, {by_id} found by their own #id; {len(leaves)} XCAF leaves, "
          f"{unbound} with a face that does not bind, {len(set(keys))} distinct (faces, transform) keys")
