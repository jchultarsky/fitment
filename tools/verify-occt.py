#!/usr/bin/env -S uv run --locked --script
# /// script
# requires-python = ">=3.11,<3.14"
# dependencies = ["cadquery-ocp-novtk==8.0.1.1.0"]
# ///
"""Check fitment's test parts with Open CASCADE, the oracle.

    tools/verify-occt.py manifest DIR

Reads every case of DIR/manifest.json (written by tools/gen-corpus.py) with
Open CASCADE 8.0.1, the kernel the M7 fit verifier pins (decision 8), and
checks what it reads against the answers the manifest states from the
construction. This catches generator bugs: a part that is not what its
construction says. It is independent of fitment's code, and of the
generator's kernel (Open CASCADE 7.9.3 through cadquery).

For an accepted case, where the manifest states it:

  - exactly one valid solid with positive volume;
  - the full concave cylinder groups are the expected hole segments, and
    the full convex ones the shaft segments: diameter, axis line and, for
    a known hole, the extent along the axis; a group is faces on one
    cylinder with one sense whose spans along the axis touch, as fitment
    groups them;
  - the partial cylinder groups are the expected partial cylinders, with
    their sweep within 0.5 deg;
  - the planes: outward normal, offset, number of faces and shell;
  - through or blind: just beyond each opening is outside the solid, just
    beyond a blind bottom is inside, and the hole's own axis is air;
  - the number of faces that are not planes, cylinders, cones or tori.

For a refused case it checks only what Open CASCADE can see: two solids
for `several_solids`, none for `no_solid`. Open CASCADE reads a file with no
length unit as millimetres and honours a flipped same_sense, so those
refusals are checked by fitment's tests alone.

Exits 1 on any disagreement, after listing every one.
"""

from __future__ import annotations

import json
import math
import sys
from pathlib import Path

from OCP.BRepAdaptor import BRepAdaptor_Curve, BRepAdaptor_Surface
from OCP.BRepCheck import BRepCheck_Analyzer
from OCP.BRepClass3d import BRepClass3d, BRepClass3d_SolidClassifier
from OCP.BRepGProp import BRepGProp
from OCP.BRepTools import BRepTools
from OCP.GeomAbs import GeomAbs_SurfaceType as Kind
from OCP.GProp import GProp_GProps
from OCP.gp import gp_Pnt
from OCP.IFSelect import IFSelect_RetDone
from OCP.Message import Message
from OCP.STEPControl import STEPControl_Reader
from OCP.TopAbs import TopAbs_EDGE, TopAbs_FACE, TopAbs_IN, TopAbs_OUT, TopAbs_REVERSED, TopAbs_SHELL, TopAbs_SOLID
from OCP.TopExp import TopExp_Explorer
from OCP.TopoDS import TopoDS

LINEAR = 1e-6  # mm: the manifest's tolerance
ANGULAR = 1e-9  # rad
SWEEP_DEG = 0.5  # partial sweeps are measured from parameter ranges
PROBE = 1e-3  # mm: how far beyond an end a point is classified


def dot(a, b):
    return sum(x * y for x, y in zip(a, b))


def sub(a, b):
    return tuple(x - y for x, y in zip(a, b))


def add(p, d, s=1.0):
    return tuple(a + s * b for a, b in zip(p, d))


def cross(a, b):
    return (a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0])


def norm(a):
    n = math.sqrt(dot(a, a))
    return tuple(x / n for x in a)


def downcast(kind):
    """TopoDS's cast to `kind`: `Face` in OCP 8, `Face_s` before."""
    return getattr(TopoDS, f"{kind}_s", None) or getattr(TopoDS, kind)


as_face, as_solid = downcast("Face"), downcast("Solid")


def xyz(p):
    return (p.X(), p.Y(), p.Z())


def explore(shape, kind):
    e = TopExp_Explorer(shape, kind)
    while e.More():
        yield e.Current()
        e.Next()


def same_line(p, d, q, e):
    """True if the lines (p, d) and (q, e) are one line."""
    if math.sqrt(dot(cross(d, e), cross(d, e))) > ANGULAR:
        return False
    off = sub(q, p)
    perp = sub(off, tuple(dot(off, d) * x for x in d))
    return math.sqrt(dot(perp, perp)) <= LINEAR


class Solid:
    """What Open CASCADE reads in one solid."""

    def __init__(self, solid):
        self.solid = solid
        props = GProp_GProps()
        BRepGProp.VolumeProperties_s(solid, props)
        self.volume = props.Mass()
        self.valid = BRepCheck_Analyzer(solid).IsValid()
        self.classifier = BRepClass3d_SolidClassifier(solid)
        outer = BRepClass3d.OuterShell_s(solid)
        shells = list(explore(solid, TopAbs_SHELL))
        voids = [s for s in shells if not s.IsSame(outer)]
        self.cylinders, self.planes, self.others = [], [], 0
        for f in explore(solid, TopAbs_FACE):
            face = as_face(f)
            shell = 0
            for k, void in enumerate(voids, 1):
                if any(face.IsSame(g) for g in explore(void, TopAbs_FACE)):
                    shell = k
            surface = BRepAdaptor_Surface(face, True)
            kind = surface.GetType()
            if kind == Kind.GeomAbs_Cylinder:
                self.cylinders.append(self.cylinder(face, surface))
            elif kind == Kind.GeomAbs_Plane:
                plane = surface.Plane()
                n = xyz(plane.Axis().Direction())
                if face.Orientation() == TopAbs_REVERSED:
                    n = tuple(-x for x in n)
                self.planes.append((n, dot(n, xyz(plane.Location())), shell))
            elif kind not in (Kind.GeomAbs_Cone, Kind.GeomAbs_Torus):
                self.others += 1

    def inside(self, point):
        self.classifier.Perform(gp_Pnt(*point), 1e-7)
        return self.classifier.State() == TopAbs_IN

    def outside(self, point):
        self.classifier.Perform(gp_Pnt(*point), 1e-7)
        return self.classifier.State() == TopAbs_OUT

    def cylinder(self, face, surface):
        c = surface.Cylinder()
        loc, d, radius = xyz(c.Location()), norm(xyz(c.Axis().Direction())), c.Radius()
        umin, umax, vmin, vmax = BRepTools.UVBounds_s(face)
        mid = surface.Value((umin + umax) / 2, (vmin + vmax) / 2)
        foot = add(loc, d, dot(sub(xyz(mid), loc), d))
        radial = norm(sub(xyz(mid), foot))
        # Material just outside the wall means the wall bounds a hole.
        concave = self.inside(add(xyz(mid), radial, PROBE))
        x, y = xyz(c.XAxis().Direction()), xyz(c.YAxis().Direction())
        # The face's extent along its axis, from points on its edges: the
        # parameter bounds are loose where an edge is an intersection curve.
        stations = []
        for e in explore(face, TopAbs_EDGE):
            curve = BRepAdaptor_Curve(downcast("Edge")(e))
            a, b = curve.FirstParameter(), curve.LastParameter()
            stations += [dot(sub(xyz(curve.Value(a + (b - a) * k / 64)), loc), d) for k in range(65)]
        return {"loc": loc, "d": d, "r": radius, "concave": concave, "u": (umin, umax),
                "v": (min(stations), max(stations)), "x": x, "y": y}

    def groups(self):
        """Cylinder faces grouped as fitment groups them: one cylinder, one
        sense, spans along the axis that touch."""
        groups = []
        for c in self.cylinders:
            for g in groups:
                g0 = g["faces"][0]
                if (abs(c["r"] - g0["r"]) <= LINEAR and c["concave"] == g0["concave"]
                        and same_line(g0["loc"], g0["d"], c["loc"], c["d"])):
                    lo, hi = self.span(c, g0)
                    if lo <= g["hi"] + LINEAR and hi >= g["lo"] - LINEAR:
                        g["faces"].append(c)
                        g["lo"], g["hi"] = min(g["lo"], lo), max(g["hi"], hi)
                        break
            else:
                lo, hi = self.span(c, c)
                groups.append({"faces": [c], "lo": lo, "hi": hi})
        for g in groups:
            g["sweep"] = self.sweep(g)
        return groups

    @staticmethod
    def span(c, ref):
        """The face's stations along `ref`'s axis."""
        d = ref["d"]
        sign = 1.0 if dot(c["d"], d) > 0 else -1.0
        base = dot(sub(c["loc"], ref["loc"]), d)
        a, b = (base + sign * v for v in c["v"])
        return min(a, b), max(a, b)

    @staticmethod
    def sweep(g):
        """The angle the group's faces cover about the axis, in degrees."""
        ref = g["faces"][0]
        d0 = ref["d"]
        x0, y0 = ref["x"], cross(d0, ref["x"])
        intervals = []
        for c in g["faces"]:
            u0, u1 = c["u"]
            start = tuple(math.cos(u0) * a + math.sin(u0) * b for a, b in zip(c["x"], c["y"]))
            t0 = math.degrees(math.atan2(dot(start, y0), dot(start, x0)))
            width = math.degrees(u1 - u0)
            turns = dot(cross(c["x"], c["y"]), d0) > 0
            intervals.append(((t0 if turns else t0 - width) % 360.0, width))
        covered = [False] * 3600
        for t, width in intervals:
            for k in range(3600):
                if (k / 10 - t) % 360.0 <= width + 1e-9:
                    covered[k] = True
        return sum(covered) / 10


def expected_segments(features, key):
    """(diameter, axis point, direction, extent or None, label) per expected
    cylinder of the holes or shafts."""
    out = []
    for i, f in enumerate(features[key]):
        label = f"{key[:-1]} {i}"
        if f["kind"] == "unknown":
            out.append((f["diameter"], f["axis_line_point"], norm(f["axis_direction"]), None, label))
            continue
        for d, a, b in f["segments"]:
            out.append((d, f["axis_point"], norm(f["axis_direction"]), (a, b), label))
    return out


def match_segments(solid, groups, expected, concave, problems, case):
    pool = [g for g in groups if g["faces"][0]["concave"] == concave and g["sweep"] >= 360 - SWEEP_DEG]
    for d, point, direction, extent, label in expected:
        found = None
        for g in pool:
            g0 = g["faces"][0]
            if abs(2 * g0["r"] - d) > LINEAR or not same_line(point, direction, g0["loc"], g0["d"]):
                continue
            if extent is not None:
                sign = 1.0 if dot(g0["d"], direction) > 0 else -1.0
                base = dot(sub(g0["loc"], point), direction)
                stations = sorted(base + sign * s for s in (g["lo"], g["hi"]))
                if abs(stations[0] - extent[0]) > LINEAR or abs(stations[1] - extent[1]) > LINEAR:
                    continue
            found = g
            break
        if found is None:
            problems.append(f"{case}: {label}: no full {'concave' if concave else 'convex'} group of dia {d} "
                            f"on the axis through {point} along {direction}" + (f" spanning {extent}" if extent else ""))
        else:
            pool.remove(found)
    for g in pool:
        g0 = g["faces"][0]
        problems.append(f"{case}: an unexpected full {'concave' if concave else 'convex'} group of dia "
                        f"{2 * g0['r']:.9g} on the axis through {tuple(round(x, 6) for x in g0['loc'])}")


def check_partials(groups, expected, problems, case):
    pool = [g for g in groups if g["sweep"] < 360 - SWEEP_DEG]
    for p in expected:
        point, direction = p["axis_line_point"], norm(p["axis_direction"])
        found = next((g for g in pool if abs(2 * g["faces"][0]["r"] - p["diameter"]) <= LINEAR
                      and g["faces"][0]["concave"] == p["concave"]
                      and same_line(point, direction, g["faces"][0]["loc"], g["faces"][0]["d"])
                      and abs(g["sweep"] - p["sweep_deg"]) <= SWEEP_DEG), None)
        if found is None:
            problems.append(f"{case}: no partial group of dia {p['diameter']}, sweep {p['sweep_deg']} deg, "
                            f"on the axis through {point}")
        else:
            pool.remove(found)
    for g in pool:
        problems.append(f"{case}: an unexpected partial group of dia {2 * g['faces'][0]['r']:.9g}, "
                        f"sweep {g['sweep']:.1f} deg")


def check_planes(solid, expected, problems, case):
    found = {}
    for n, offset, shell in solid.planes:
        key = next((k for k in found if k[2] == shell and math.sqrt(dot(cross(k[0], n), cross(k[0], n))) <= ANGULAR
                    and dot(k[0], n) > 0 and abs(k[1] - offset) <= LINEAR), None)
        if key is None:
            key = (n, offset, shell)
            found[key] = 0
        found[key] += 1
    for p in expected:
        n, offset, shell = norm(p["normal"]), p["offset"], p["shell"]
        key = next((k for k in found if k[2] == shell and math.sqrt(dot(cross(k[0], n), cross(k[0], n))) <= ANGULAR
                    and dot(k[0], n) > 0 and abs(k[1] - offset) <= LINEAR), None)
        if key is None:
            problems.append(f"{case}: no plane with normal {p['normal']}, offset {offset}, shell {shell}")
        elif found[key] != p["faces"]:
            problems.append(f"{case}: the plane with normal {p['normal']}, offset {offset} has {found.pop(key)} faces, "
                            f"not {p['faces']}")
        else:
            found.pop(key)
    for (n, offset, shell), faces in found.items():
        problems.append(f"{case}: an unexpected plane with normal {tuple(round(x, 9) for x in n)}, "
                        f"offset {offset:.9g}, shell {shell}, {faces} faces")


def check_ends(solid, holes, problems, case):
    for i, h in enumerate(holes):
        if h["kind"] not in ("through", "blind"):
            continue
        p, d = h["axis_point"], norm(h["axis_direction"])
        segments = h["segments"]
        end = segments[-1][2]
        start_end, far_end = h["ends"]
        if start_end.get("bottom") != "annular":
            for a, b in ((s[1], s[2]) for s in segments):
                if not solid.outside(add(p, d, (a + b) / 2)):
                    problems.append(f"{case}: hole {i}: its axis at {(a + b) / 2} is not air")
        if not solid.outside(add(p, d, end + PROBE)):
            problems.append(f"{case}: hole {i}: just beyond its opening at {end} is not outside")
        if h["kind"] == "through":
            if not solid.outside(add(p, d, -PROBE)):
                problems.append(f"{case}: hole {i}: just beyond its opening at 0 is not outside")
            continue
        beyond = {"flat": 0.0, "annular": 0.0}.get(start_end["bottom"])
        if start_end["bottom"] == "point":
            beyond = -start_end["tip_length"]
        elif start_end["bottom"] == "cone_then_flat":
            beyond = start_end["floor_at"]
        if not solid.inside(add(p, d, beyond - PROBE)):
            problems.append(f"{case}: hole {i}: just beyond its {start_end['bottom']} bottom is not material")


def check_case(directory, case):
    problems = []
    name = case["id"]
    reader = STEPControl_Reader()
    if reader.ReadFile(str(directory / case["file"])) != IFSelect_RetDone:
        return [f"{name}: Open CASCADE cannot read it"]
    reader.TransferRoots()
    solids = [Solid(as_solid(s)) for i in range(1, reader.NbShapes() + 1)
              for s in explore(reader.Shape(i), TopAbs_SOLID)]
    expect = case["expect"]
    if expect["result"] == "refused":
        codes = expect["refusals"]
        if "several_solids" in codes and len(solids) < 2:
            problems.append(f"{name}: refused for several solids, but Open CASCADE reads {len(solids)}")
        if "no_solid" in codes and solids:
            problems.append(f"{name}: refused for no solid, but Open CASCADE reads {len(solids)}")
        return problems
    if len(solids) != 1:
        return [f"{name}: {len(solids)} solids, not 1"]
    solid = solids[0]
    if not solid.valid or solid.volume <= 0:
        return [f"{name}: the solid is not valid, or its volume {solid.volume} is not positive"]
    features = expect["features"]
    groups = solid.groups()
    match_segments(solid, groups, expected_segments(features, "holes"), True, problems, name)
    match_segments(solid, groups, expected_segments(features, "shafts"), False, problems, name)
    check_partials(groups, features["partial_cylinders"], problems, name)
    if features["planes"] is not None:
        check_planes(solid, features["planes"], problems, name)
    check_ends(solid, features["holes"], problems, name)
    if "other_faces" in features and features["other_faces"] != solid.others:
        problems.append(f"{name}: {solid.others} other faces, not {features['other_faces']}")
    return problems


def main():
    if len(sys.argv) != 3 or sys.argv[1] != "manifest":
        sys.exit(__doc__)
    directory = Path(sys.argv[2])
    manifest = json.loads((directory / "manifest.json").read_text(encoding="utf-8"))
    messenger = Message.DefaultMessenger_s()
    printers = messenger.Printers()
    for i in range(printers.Length(), 0, -1):
        messenger.RemovePrinter(printers.Value(i))
    problems = []
    for case in manifest["cases"]:
        found = check_case(directory, case)
        problems += found
        print(f"{'ok  ' if not found else 'FAIL'} {case['id']}")
    for problem in problems:
        print(problem, file=sys.stderr)
    print(f"{len(manifest['cases'])} cases, {len(problems)} disagreements with Open CASCADE")
    sys.exit(1 if problems else 0)


if __name__ == "__main__":
    main()
