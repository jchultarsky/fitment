#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11,<3.14"
# dependencies = ["cadquery-ocp-novtk==8.0.1.1.0"]
# ///
"""P4: the largest sub-shape tolerance Open CASCADE reports per solid of each assembly fixture, and
whether each solid is valid (FT6's cap of delta/10); also the kernel's version strings.

    docs/research/m7-evidence/p4_tolerance.py FILE... > p4_tolerance.txt
"""
import sys
from importlib.metadata import version

from OCP.BRepCheck import BRepCheck_Analyzer
from OCP.IFSelect import IFSelect_RetDone
from OCP.ShapeAnalysis import ShapeAnalysis_ShapeTolerance
from OCP.STEPControl import STEPControl_Reader
from OCP.TopAbs import TopAbs_SHAPE, TopAbs_SOLID
from OCP.TopExp import TopExp_Explorer

try:
    from OCP.Standard import Standard_Version  # noqa: F401
except ImportError:
    pass
import OCP
print(f"cadquery-ocp-novtk {version('cadquery-ocp-novtk')}; OCP.__version__ {getattr(OCP, '__version__', '?')}")
for path in sys.argv[1:]:
    r = STEPControl_Reader()
    if r.ReadFile(path) != IFSelect_RetDone:
        print(f"{path}: unreadable")
        continue
    r.TransferRoots()
    tols, invalid, n = [], 0, 0
    e = TopExp_Explorer(r.OneShape(), TopAbs_SOLID)
    while e.More():
        s = e.Current()
        n += 1
        tols.append(ShapeAnalysis_ShapeTolerance().Tolerance(s, 1, TopAbs_SHAPE))
        invalid += 0 if BRepCheck_Analyzer(s).IsValid() else 1
        e.Next()
    tols.sort()
    over = sum(1 for t in tols if t > 0.001)
    print(f"{path.split('/')[-1]}: {n} solids, {invalid} invalid, tolerance max {max(tols, default=0):.3g}, "
          f"median {tols[len(tols) // 2] if tols else 0:.3g}, over 0.001 mm: {over}")
