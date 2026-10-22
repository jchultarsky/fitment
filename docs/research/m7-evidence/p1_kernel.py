#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11,<3.14"
# dependencies = ["cadquery-ocp-novtk==8.0.1.1.0"]
# ///
"""P1: what Open CASCADE 8.0.1 does with the cases a fit check meets.

    docs/research/m7-evidence/p1_kernel.py [CANDIDATE.stp] > p1_kernel.txt

1. Two boxes overlapping by e, touching, or apart by g: Common's volume,
   its done/warning/error flags, and BRepExtrema's distance; in the
   identity frame and in a skew frame.
2. Containment: a box inside another; Common's volume and the distance.
3. Coaxial cylinders: clearance, line to line, interference.
4. Determinism: one Common volume computed 20 times, serial and parallel,
   compared bit for bit (float.hex).
5. A thickened face (the zone of a contact): a plane and a cylinder face.
6. With a candidate file: its solid offset by +0.01 mm, and the time.
"""

import math
import sys
import time

from OCP.BRepAlgoAPI import BRepAlgoAPI_Common, BRepAlgoAPI_Cut
from OCP.BRepBuilderAPI import BRepBuilderAPI_Transform
from OCP.BRepCheck import BRepCheck_Analyzer
from OCP.BRepExtrema import BRepExtrema_DistShapeShape
from OCP.BRepGProp import BRepGProp
from OCP.BRepOffsetAPI import BRepOffsetAPI_MakeOffsetShape, BRepOffsetAPI_MakeThickSolid
from OCP.BRepPrimAPI import BRepPrimAPI_MakeBox, BRepPrimAPI_MakeCylinder
from OCP.GProp import GProp_GProps
from OCP.IFSelect import IFSelect_RetDone
from OCP.STEPControl import STEPControl_Reader
from OCP.TopAbs import TopAbs_FACE, TopAbs_SOLID
from OCP.TopExp import TopExp_Explorer
from OCP.TopoDS import TopoDS
from OCP.gp import gp_Ax1, gp_Ax2, gp_Dir, gp_Pnt, gp_Trsf, gp_Vec


def vol(shape):
    p = GProp_GProps()
    BRepGProp.VolumeProperties_s(shape, p)
    return p.Mass()


def vol_eps(shape):
    """The Eps overload: adaptive integration to a relative 1e-9."""
    p = GProp_GProps()
    BRepGProp.VolumeProperties_s(shape, p, 1e-9, True)
    return p.Mass()


def common(a, b, parallel=False):
    """Common through BOPAlgo_BOP, which OCP 8.0.1 binds with HasWarnings and
    HasErrors (BRepAlgoAPI_Common's are not bound)."""
    from OCP.BOPAlgo import BOPAlgo_BOP, BOPAlgo_COMMON
    op = BOPAlgo_BOP()
    op.AddArgument(a)
    op.AddTool(b)
    op.SetOperation(BOPAlgo_COMMON)
    op.SetRunParallel(parallel)
    op.SetNonDestructive(True)  # as the helper does (the reviews): arguments never modified
    op.Perform()
    return op


def dist(a, b):
    d = BRepExtrema_DistShapeShape(a, b)
    return d.Value() if d.IsDone() else None, d.NbSolution() if d.IsDone() else None, d.InnerSolution() if d.IsDone() else None


def skew():
    t = gp_Trsf()
    t.SetRotation(gp_Ax1(gp_Pnt(1, 2, 3), gp_Dir(2, -1, 1)), math.radians(140.0))
    s = gp_Trsf()
    s.SetTranslation(gp_Vec(-35.0, 12.0, 60.0))
    return s.Multiplied(t)


def moved(shape, t):
    return BRepBuilderAPI_Transform(shape, t, True).Shape()


def box(x, y, z, dx, dy, dz):
    return BRepPrimAPI_MakeBox(gp_Pnt(x, y, z), dx, dy, dz).Shape()


def flags(op):
    return f"warn={op.HasWarnings()} err={op.HasErrors()}"


def section1():
    print("== 1. two 10 mm boxes, the second at x = 10 - e (overlap e) or 10 + g (gap g)")
    print("frame | case | Common volume | expected | flags | distance")
    for frame, t in (("identity", gp_Trsf()), ("skew", skew())):
        for label, x in [("overlap 1e-3", 10 - 1e-3), ("overlap 1e-4", 10 - 1e-4), ("overlap 1e-5", 10 - 1e-5),
                         ("overlap 1e-6", 10 - 1e-6), ("overlap 1e-7", 10 - 1e-7), ("overlap 1e-8", 10 - 1e-8),
                         ("touch", 10.0), ("gap 1e-7", 10 + 1e-7), ("gap 1e-5", 10 + 1e-5),
                         ("gap 0.005", 10.005), ("gap 0.02", 10.02)]:
            a = moved(box(0, 0, 0, 10, 10, 10), t)
            b = moved(box(x, 0, 0, 10, 10, 10), t)
            op = common(a, b)
            v = vol(op.Shape()) if not op.HasErrors() else None
            expected = max(10 - x, 0) * 100
            d, n, inner = dist(a, b)
            print(f"{frame} | {label} | {v:.6g} | {expected:.6g} | {flags(op)} | {d:.3g} (inner {inner})")


def section2():
    print("== 2. containment: a 2 mm box inside a 10 mm box, 4 mm from every face")
    a = box(0, 0, 0, 10, 10, 10)
    b = box(4, 4, 4, 2, 2, 2)
    op = common(a, b)
    d, n, inner = dist(a, b)
    print(f"Common volume {vol(op.Shape()):.6g} (expected 8); {flags(op)}; distance {d:.6g}, inner solution {inner}")


def section3():
    print("== 3. a shaft in a hole (a 20 mm block with a bore of radius R, a cylinder of radius r on its axis)")
    for label, R, r in (("clearance 0.3", 3.3, 3.0), ("clearance 0.005", 3.005, 3.0), ("line to line", 3.0, 3.0),
                        ("interference 0.5", 2.5, 3.0), ("interference 1e-6", 3.0 - 1e-6, 3.0)):
        block = box(-10, -10, 0, 20, 20, 10)
        bore = BRepPrimAPI_MakeCylinder(gp_Ax2(gp_Pnt(0, 0, -1), gp_Dir(0, 0, 1)), R, 12).Shape()
        holed = BRepAlgoAPI_Cut(block, bore).Shape()
        shaft = BRepPrimAPI_MakeCylinder(gp_Ax2(gp_Pnt(0, 0, -5), gp_Dir(0, 0, 1)), r, 20).Shape()
        op = common(holed, shaft)
        v = vol(op.Shape())
        expected = max(r * r - R * R, 0) * math.pi * 10
        d, n, inner = dist(holed, shaft)
        print(f"{label}: Common {v:.6g} (expected {expected:.6g}); {flags(op)}; distance {d:.6g}")


def section4(a=None, b=None):
    print("== 4. determinism: one Common, 20 times, serial and parallel; volumes as float.hex")
    if a is None:
        a = moved(box(0, 0, 0, 10, 10, 10), skew())
        b = moved(BRepPrimAPI_MakeCylinder(gp_Ax2(gp_Pnt(9.5, 5, -1), gp_Dir(0.1, 0.2, 1)), 2.0, 12).Shape(), skew())
    for parallel in (False, True):
        seen, seen_eps = set(), set()
        for _ in range(20):
            s = common(a, b, parallel).Shape()
            seen.add(vol(s).hex())
            seen_eps.add(vol_eps(s).hex())
        print(f"parallel={parallel}: {len(seen)} distinct default volume(s) {sorted(seen)}; "
              f"{len(seen_eps)} distinct Eps volume(s) {sorted(seen_eps)}")


def section5():
    print("== 5. a face thickened into a zone solid (BRepOffsetAPI_MakeThickSolid / MakeOffsetShape thickening)")
    block = box(0, 0, 0, 10, 10, 10)
    cyl = BRepPrimAPI_MakeCylinder(gp_Ax2(gp_Pnt(0, 0, 0), gp_Dir(0, 0, 1)), 3.0, 10).Shape()
    for name, shape in (("box", block), ("cylinder", cyl)):
        e = TopExp_Explorer(shape, TopAbs_FACE)
        k = 0
        while e.More():
            face = TopoDS.Face(e.Current())
            op = BRepOffsetAPI_MakeOffsetShape()
            op.PerformByJoin(face, 0.01, 1e-7, __import__("OCP.BRepOffset", fromlist=["BRepOffset_Skin"]).BRepOffset_Skin,
                             False, False, __import__("OCP.GeomAbs", fromlist=["GeomAbs_Arc"]).GeomAbs_Arc, True)
            ok = op.IsDone()
            v = vol(op.Shape()) if ok else None
            area = GProp_GProps()
            BRepGProp.SurfaceProperties_s(face, area)
            print(f"{name} face {k}: thickened by 0.01: done={ok} volume={v} (area x 0.01 = {area.Mass() * 0.01:.6g})")
            k += 1
            e.Next()


def section6(path):
    print(f"== 6. {path}: its solid offset outward by 0.01 mm")
    r = STEPControl_Reader()
    assert r.ReadFile(path) == IFSelect_RetDone
    r.TransferRoots()
    solid = TopoDS.Solid(TopExp_Explorer(r.OneShape(), TopAbs_SOLID).Current())
    faces = 0
    e = TopExp_Explorer(solid, TopAbs_FACE)
    while e.More():
        faces += 1
        e.Next()
    from OCP.BRepOffset import BRepOffset_Skin
    from OCP.GeomAbs import GeomAbs_Arc, GeomAbs_Intersection
    for join in (GeomAbs_Arc, GeomAbs_Intersection):
        t0 = time.perf_counter()
        op = BRepOffsetAPI_MakeOffsetShape()
        try:
            op.PerformByJoin(solid, 0.01, 1e-7, BRepOffset_Skin, False, False, join, False)
            ok = op.IsDone()
        except Exception as exc:  # noqa: BLE001
            ok = f"raised {type(exc).__name__}: {exc}"
        dt = time.perf_counter() - t0
        extra = ""
        if ok is True:
            out = op.Shape()
            extra = f" valid={BRepCheck_Analyzer(out).IsValid()} volume {vol(solid):.6g} -> {vol(out):.6g}"
        print(f"{faces} faces, join {join}: done={ok} in {dt:.3f} s{extra}")
    t0 = time.perf_counter()
    d = BRepExtrema_DistShapeShape(solid, moved(solid, (lambda t: (t.SetTranslation(gp_Vec(0, 0, 100)), t)[1])(gp_Trsf())))
    print(f"distance to its copy 100 mm away: {d.Value():.6g} in {time.perf_counter() - t0:.3f} s")
    section4(solid, moved(solid, (lambda t: (t.SetTranslation(gp_Vec(0.5, 0.25, 0.125)), t)[1])(gp_Trsf())))


if __name__ == "__main__":
    section1()
    section2()
    section3()
    section4()
    section5()
    for p in sys.argv[1:]:
        section6(p)
