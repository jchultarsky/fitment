#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11,<3.14"
# dependencies = ["cadquery-ocp-novtk==8.0.1.1.0"]
# ///
"""P7: the soundness review's probe R1 (C1), kept and extended: what an uncertain neighbour does to a zone.

    docs/research/m7-evidence/p7_uncertainty.py > p7_uncertainty.txt

(a) Overlaps against a body's shape tolerance (FT6's cap of t/10).
(b) A plate whose top face a seat cites, a candidate seated on it with a 1 x 1 mm boss 0.04 mm into it,
    inside the face's footprint. As first designed, delta = max(t, the pair's uncertainty) set the zone's
    depth, so an uncertainty of 0.058 mm (the weldment's) deepened the slab past the boss: verified.
    As revised (C1): eps = t - u_pair sets the interference zones, t + u_pair the proximity threshold,
    eps + t + u_pair the proximity zones, and eps <= 0 is not_verified (uncertainty_above_tolerance).
(c) FZ7's volume check alone cannot see a zone placed at the wrong depth (hence M1's position checks).
"""
from OCP.BOPAlgo import BOPAlgo_BOP, BOPAlgo_COMMON, BOPAlgo_CUT
from OCP.BRepAlgoAPI import BRepAlgoAPI_Fuse
from OCP.BRepExtrema import BRepExtrema_DistShapeShape
from OCP.BRepGProp import BRepGProp
from OCP.BRepPrimAPI import BRepPrimAPI_MakeBox
from OCP.GProp import GProp_GProps
from OCP.ShapeFix import ShapeFix_ShapeTolerance
from OCP.TopAbs import TopAbs_SHAPE
from OCP.gp import gp_Pnt

T, GUARD = 0.01, 1e-6


def bop(a, b, op):
    o = BOPAlgo_BOP()
    o.AddArgument(a)
    o.AddTool(b)
    o.SetOperation(op)
    o.SetRunParallel(False)
    o.SetFuzzyValue(0.0)
    o.SetNonDestructive(True)
    o.Perform()
    return o.Shape(), o.HasWarnings() or o.HasErrors()


def vol(s):
    g = GProp_GProps()
    BRepGProp.VolumeProperties_s(s, g)
    return g.Mass()


def dist(a, b):
    d = BRepExtrema_DistShapeShape(a, b)
    return 0.0 if d.InnerSolution() else d.Value()


def box(x0, y0, z0, x1, y1, z1):
    return BRepPrimAPI_MakeBox(gp_Pnt(x0, y0, z0), gp_Pnt(x1, y1, z1)).Shape()


def slab(depth):
    return box(0, 0, 10 - depth, 100, 100, 10 + depth)


print("== (a) a 10 mm box B with every sub-shape tolerance set to tol; C overlaps it by e from above")
for tol in (1e-7, 1e-4, 1e-3):
    for e in (5e-5, 5e-4, 2e-3):
        B = box(0, 0, 0, 10, 10, 10)
        if tol > 1e-7:
            ShapeFix_ShapeTolerance().SetTolerance(B, tol, TopAbs_SHAPE)
        C = box(2, 2, 10 - e, 8, 8, 12)
        s, alert = bop(C, B, BOPAlgo_COMMON)
        print(f"tol {tol:g} overlap {e:g}: Common {vol(s):.6g} (exact {36 * e:.6g}) alert={alert}; "
              f"distance {dist(C, B):.3g}")

print("== (b) a boss 0.04 mm into a cited plate face, by the pair's uncertainty u")
B = box(0, 0, 0, 100, 100, 10)
C = BRepAlgoAPI_Fuse(box(10, 10, 10, 30, 30, 15), box(50, 50, 10 - 0.04, 51, 51, 10.0)).Shape()
C = BRepAlgoAPI_Fuse(C, box(30, 20, 10, 51, 21, 11)).Shape()
V, _ = bop(C, B, BOPAlgo_COMMON)
for u in (1e-7, 0.004, 0.0099, 0.01, 0.058):
    up = max(GUARD, u)
    # as first designed
    d = max(T, up)
    R, _ = bop(V, slab(d), BOPAlgo_CUT)
    gap = dist(C, bop(B, slab(2 * d), BOPAlgo_CUT)[0])
    first = "interferes" if vol(R) >= 1e-6 else ("not_verified (proximity)" if gap <= d + GUARD else "verified")
    # as revised (C1)
    eps, thr = T - up, T + up
    if eps <= 0:
        revised = "not_verified (uncertainty_above_tolerance)"
    else:
        R2, _ = bop(V, slab(eps), BOPAlgo_CUT)
        gap2 = dist(C, bop(B, slab(eps + thr), BOPAlgo_CUT)[0])
        revised = ("interferes" if vol(R2) >= 1e-6 else
                   ("not_verified (proximity)" if gap2 <= thr else "verified")) + f" (residual {vol(R2):.4g} mm3)"
    print(f"u {u:g}: as first designed (delta {d:g}): {first}; as revised (eps {eps:.6g}): {revised}")

print("== (c) a zone's volume is the same wherever it is placed")
print(f"slab at the face {vol(box(0, 0, 9.99, 100, 100, 10.01)):.6g}, "
      f"the same slab 5 mm deep {vol(box(0, 0, 4.99, 100, 100, 5.01)):.6g}")
