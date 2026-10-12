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
construction says, or an answer that is not what the part is. It shares no
code with fitment, nor a kernel version with the generator (7.9.3).

For an accepted case, every face must be accounted for:

  - exactly one valid solid with positive volume;
  - cylinder faces grouped as fitment's rule G2 groups them (one cylinder,
    one sense, sharing an edge): each full concave group is an expected
    hole segment and each full convex group a shaft segment, with its
    diameter, axis line and, for a known hole, its extent along the axis;
    each partial group an expected partial cylinder, with its exact sweep;
  - the planes: outward normal, offset, number of faces and shell;
  - the number of distinct cones and tori, and of other faces;
  - each end of each known hole, tested on both sides by classifying
    points against the solid: air just past an opening and material
    beside its rim; material just past a bottom and air just inside it;
    for a drill point, its apex and angle as a cone of the solid; for a
    countersink, its cone; for an annular bottom, the post on the axis;
  - each shaft's root (material beside it, air past it) and free end.

For a refused case, the facts Open CASCADE or the file text can show: an
assembly usage, two product definitions, two solids, no solid, a surface
model, no length unit; and that the stated gate stage is its codes'.

Not checked here, so fitment's own tests are the only check:

  - a vertex moved off its face, and a flipped same_sense: Open CASCADE
    repairs face orientation as it reads, so it cannot see the flip;
  - what the rules call an end they cannot read (`irregular`, `unknown`)
    and whether a hole is stated as `unknown` rather than known: those are
    the rules' answers, and the oracle checks only geometry.

Exits 1 on any disagreement, after listing every one.
"""

from __future__ import annotations

import json
import math
import re
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
SWEEP_DEG = 1e-6  # partial sweeps are exact from the faces' parameter ranges
PROBE = 1e-3  # mm: how far past a face a point is classified
BESIDE = 1.0  # mm: how far outside a rim the material beside it is tested


def downcast(kind):
    """TopoDS's cast to `kind`: `Face` in OCP 8, `Face_s` before."""
    return getattr(TopoDS, f"{kind}_s", None) or getattr(TopoDS, kind)


as_face, as_solid, as_edge = downcast("Face"), downcast("Solid"), downcast("Edge")


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


def xyz(p):
    return (p.X(), p.Y(), p.Z())


def explore(shape, kind):
    e = TopExp_Explorer(shape, kind)
    while e.More():
        yield e.Current()
        e.Next()


def parallel(d, e):
    return math.sqrt(dot(cross(d, e), cross(d, e))) <= ANGULAR


def same_line(p, d, q, e):
    """True if the lines (p, d) and (q, e) are one line."""
    if not parallel(d, e):
        return False
    off = sub(q, p)
    perp = sub(off, tuple(dot(off, d) * x for x in d))
    return math.sqrt(dot(perp, perp)) <= LINEAR


def canonical(d):
    """The sign the design gives an undirected axis: the largest absolute
    component positive, ties broken in the order x, y, z."""
    i = max(range(3), key=lambda k: (abs(d[k]), -k))
    return d[i] > 0


# The gate stage each refusal code belongs to (design, section 4.2).
STAGES = {"unparseable": 1, "dangling_reference": 1, "several_data_sections": 1, "schema_unrecognized": 1,
          "assembly": 2, "no_definition": 2, "several_definitions": 2, "shape_incomplete": 3, "no_solid": 3,
          "several_solids": 3, "unread_shape_item": 3, "solid_placement_unreadable": 3,
          "topology_unsupported": 4, "topology_malformed": 4, "solid_not_closed": 4, "shell_disconnected": 4,
          "units_unreadable": 5, "length_unit_unreadable": 5, "angle_unit_unreadable": 5,
          "uncertainty_unreadable": 5, "uncertainty_missing": 5, "geometry_unreadable": 6,
          "vertex_off_surface": 6, "vertex_off_curve": 6, "face_orientation": 6}


def side(d):
    """A unit vector perpendicular to `d`."""
    a = min(range(3), key=lambda i: abs(d[i]))
    return norm(cross(d, tuple(1.0 if i == a else 0.0 for i in range(3))))


def union_degrees(intervals):
    """The total angle the (start, width) intervals cover on the circle."""
    pieces = []
    for start, width in intervals:
        if width >= 360.0:
            return 360.0
        start %= 360.0
        end = start + width
        if end > 360.0:
            pieces += [(start, 360.0), (0.0, end - 360.0)]
        else:
            pieces.append((start, end))
    pieces.sort()
    total, lo, hi = 0.0, None, None
    for a, b in pieces:
        if hi is None or a > hi:
            if hi is not None:
                total += hi - lo
            lo, hi = a, b
        else:
            hi = max(hi, b)
    return total + (hi - lo if hi is not None else 0.0)


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
        voids = [s for s in explore(solid, TopAbs_SHELL) if not s.IsSame(outer)]
        self.cylinders, self.planes, self.cones, self.tori, self.others = [], [], [], [], 0
        for f in explore(solid, TopAbs_FACE):
            face = as_face(f)
            shell = next((k for k, void in enumerate(voids, 1)
                          if any(face.IsSame(g) for g in explore(void, TopAbs_FACE))), 0)
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
            elif kind == Kind.GeomAbs_Cone:
                c = surface.Cone()
                self.cones.append((xyz(c.Apex()), norm(xyz(c.Axis().Direction())), abs(c.SemiAngle())))
            elif kind == Kind.GeomAbs_Torus:
                t = surface.Torus()
                self.tori.append((xyz(t.Location()), norm(xyz(t.Axis().Direction())), t.MajorRadius(),
                                  t.MinorRadius()))
            else:
                self.others += 1

    def state(self, point):
        self.classifier.Perform(gp_Pnt(*point), 1e-7)
        return self.classifier.State()

    def inside(self, point):
        return self.state(point) == TopAbs_IN

    def outside(self, point):
        return self.state(point) == TopAbs_OUT

    def cylinder(self, face, surface):
        c = surface.Cylinder()
        loc, d, radius = xyz(c.Location()), norm(xyz(c.Axis().Direction())), c.Radius()
        umin, umax, vmin, vmax = BRepTools.UVBounds_s(face)
        mid = surface.Value((umin + umax) / 2, (vmin + vmax) / 2)
        foot = add(loc, d, dot(sub(xyz(mid), loc), d))
        radial = norm(sub(xyz(mid), foot))
        # Material just outside the wall means the wall bounds a hole.
        concave = self.inside(add(xyz(mid), radial, PROBE))
        # The face's extent along its axis, from points on its edges: the
        # parameter bounds are loose where an edge is an intersection curve.
        edges, stations = [], []
        for e in explore(face, TopAbs_EDGE):
            edges.append(e)
            curve = BRepAdaptor_Curve(as_edge(e))
            a, b = curve.FirstParameter(), curve.LastParameter()
            stations += [dot(sub(xyz(curve.Value(a + (b - a) * k / 64)), loc), d) for k in range(65)]
        return {"loc": loc, "d": d, "r": radius, "concave": concave, "u": (umin, umax),
                "span": (min(stations), max(stations)), "edges": edges,
                "x": xyz(c.XAxis().Direction()), "y": xyz(c.YAxis().Direction())}

    def groups(self):
        """Cylinder faces grouped as fitment's rule G2 groups them: on one
        cylinder, with one sense, joined by shared edges. Union-find, so the
        result does not depend on the order of the faces."""
        faces = self.cylinders
        root = list(range(len(faces)))

        def find(i):
            while root[i] != i:
                root[i] = root[root[i]]
                i = root[i]
            return i

        for i, a in enumerate(faces):
            for j in range(i + 1, len(faces)):
                b = faces[j]
                if (abs(a["r"] - b["r"]) <= LINEAR and a["concave"] == b["concave"]
                        and same_line(a["loc"], a["d"], b["loc"], b["d"])
                        and any(e.IsSame(g) for e in a["edges"] for g in b["edges"])):
                    root[find(j)] = find(i)
        groups = {}
        for i, c in enumerate(faces):
            groups.setdefault(find(i), []).append(c)
        out = []
        for members in groups.values():
            ref = members[0]
            lo, hi = math.inf, -math.inf
            for c in members:
                sign = 1.0 if dot(c["d"], ref["d"]) > 0 else -1.0
                base = dot(sub(c["loc"], ref["loc"]), ref["d"])
                ends = sorted(base + sign * s for s in c["span"])
                lo, hi = min(lo, ends[0]), max(hi, ends[1])
            out.append({"faces": members, "lo": lo, "hi": hi, "sweep": self.sweep(members)})
        return out

    @staticmethod
    def sweep(members):
        """The angle the faces cover about the axis, in degrees, exactly
        from their parameter ranges."""
        ref = members[0]
        d0 = ref["d"]
        x0, y0 = ref["x"], cross(d0, ref["x"])
        intervals = []
        for c in members:
            u0, u1 = c["u"]
            start = tuple(math.cos(u0) * a + math.sin(u0) * b for a, b in zip(c["x"], c["y"]))
            t0 = math.degrees(math.atan2(dot(start, y0), dot(start, x0)))
            width = math.degrees(u1 - u0)
            turns = dot(cross(c["x"], c["y"]), d0) > 0
            intervals.append((t0 if turns else t0 - width, width))
        return union_degrees(intervals)


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


def match_segments(groups, expected, concave, problems, case):
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
                        f"sweep {g['sweep']:.6f} deg")


def check_planes(solid, expected, problems, case):
    def same(k, n, offset, shell):
        return k[2] == shell and parallel(k[0], n) and dot(k[0], n) > 0 and abs(k[1] - offset) <= LINEAR

    found = {}
    for n, offset, shell in solid.planes:
        key = next((k for k in found if same(k, n, offset, shell)), (n, offset, shell))
        found[key] = found.get(key, 0) + 1
    for p in expected:
        n, offset, shell = norm(p["normal"]), p["offset"], p["shell"]
        key = next((k for k in found if same(k, n, offset, shell)), None)
        if key is None:
            problems.append(f"{case}: no plane with normal {p['normal']}, offset {offset}, shell {shell}")
            continue
        faces = found.pop(key)
        if faces != p["faces"]:
            problems.append(f"{case}: the plane with normal {p['normal']}, offset {offset} has {faces} faces, "
                            f"not {p['faces']}")
    for (n, offset, shell), faces in found.items():
        problems.append(f"{case}: an unexpected plane with normal {tuple(round(x, 9) for x in n)}, "
                        f"offset {offset:.9g}, shell {shell}, {faces} faces")


def distinct(items, same):
    out = []
    for item in items:
        if not any(same(item, other) for other in out):
            out.append(item)
    return out


def check_counts(solid, features, problems, case):
    cones = distinct(solid.cones, lambda a, b: math.dist(a[0], b[0]) <= LINEAR and parallel(a[1], b[1])
                     and abs(a[2] - b[2]) <= ANGULAR)
    tori = distinct(solid.tori, lambda a, b: math.dist(a[0], b[0]) <= LINEAR and parallel(a[1], b[1])
                    and abs(a[2] - b[2]) <= LINEAR and abs(a[3] - b[3]) <= LINEAR)
    for name, found, expected in (("cones", len(cones), features["cones"]), ("tori", len(tori), features["tori"]),
                                  ("other faces", solid.others, features["other_faces"])):
        if found != expected:
            problems.append(f"{case}: {found} {name}, not {expected}")


def has_cone(solid, apex, direction, half_angle):
    return any(math.dist(apex, a) <= LINEAR and parallel(direction, d) and abs(half_angle - s) <= ANGULAR
               for a, d, s in solid.cones)


def two_sided(solid, point, direction, sign):
    """True if material lies just before `point` along `direction` and air
    just past it, `sign` saying which way is out."""
    return solid.inside(add(point, direction, -sign * PROBE)) and solid.outside(add(point, direction, sign * PROBE))


def check_hole_ends(solid, holes, problems, case):
    for i, h in enumerate(holes):
        if h["kind"] == "unknown" and not canonical(h["axis_direction"]):
            problems.append(f"{case}: hole {i}: an unknown hole's axis {h['axis_direction']} is not in the canonical sign")
        if h["kind"] not in ("through", "blind"):
            continue
        label = f"{case}: hole {i}"
        p, d = h["axis_point"], norm(h["axis_direction"])
        if h["kind"] == "through" and not canonical(d):
            problems.append(f"{label}: a through hole's axis {h['axis_direction']} is not in the canonical sign")
        out = side(d)
        segments = h["segments"]
        length = h.get("length", h.get("depth"))
        start, far = h["ends"]
        if far.get("rim") != length or (h["kind"] == "through" and start.get("rim") != 0.0):
            problems.append(f"{label}: its rims {start.get('rim')}, {far.get('rim')} do not span its length {length}")
        if start.get("bottom") != "annular":
            for a, b in ((s[1], s[2]) for s in segments):
                if not solid.outside(add(p, d, (a + b) / 2)):
                    problems.append(f"{label}: its axis at {(a + b) / 2} is not air")
        openings = [(far, length, 1.0, segments[-1][0])]
        if h["kind"] == "through":
            openings.append((start, 0.0, -1.0, segments[0][0]))
        for end, at, sign, diameter in openings:
            radius = diameter / 2
            if "countersink" in end:
                cs = end["countersink"]
                radius = cs["outer_diameter"] / 2
                half = math.radians(cs["angle_deg"] / 2)
                apex = add(p, d, at - sign * radius / math.tan(half))
                if not has_cone(solid, apex, d, half):
                    problems.append(f"{label}: no countersink cone with apex at {apex}")
            if not solid.outside(add(p, d, at + sign * PROBE)):
                problems.append(f"{label}: just past its opening at {at} is not air")
            if not two_sided(solid, add(add(p, d, at), out, radius + BESIDE), d, sign):
                problems.append(f"{label}: the face around its opening is not at {at}")
        if h["kind"] != "blind":
            continue
        radius = segments[0][0] / 2
        bottom = start["bottom"]
        floor = 0.0
        if bottom == "point":
            half = math.radians(start["angle_deg"] / 2)
            floor = -start["tip_length"]
            if abs(start["tip_length"] - radius / math.tan(half)) > LINEAR:
                problems.append(f"{label}: tip length {start['tip_length']} is not r / tan(angle / 2)")
            if not has_cone(solid, add(p, d, floor), d, half):
                problems.append(f"{label}: no drill point cone with apex at station {floor}")
        elif bottom == "cone_then_flat":
            floor = start["floor_at"]
            half = math.radians(start["angle_deg"] / 2)
            if not any(abs(half - s) <= ANGULAR and same_line(p, d, a, cd) for a, cd, s in solid.cones):
                problems.append(f"{label}: no {start['angle_deg']} deg cone on its axis")
        if bottom == "annular":
            floor = start["floor_at"]
        probe = add(p, d, floor)
        if bottom == "annular":
            post = start["post_diameter"] / 2
            # Halfway between the post and the wall, where the floor is.
            probe = add(probe, out, (radius + post) / 2)
            above = add(p, d, floor + 2 * PROBE)
            if not (solid.inside(add(above, out, post - PROBE)) and solid.outside(add(above, out, post + PROBE))):
                problems.append(f"{label}: no post of dia {start['post_diameter']} on its axis")
        if not two_sided(solid, probe, d, 1.0):
            problems.append(f"{label}: its {bottom} bottom is not at station {floor}")


def check_shaft_ends(solid, shafts, problems, case):
    for i, s in enumerate(shafts):
        label = f"{case}: shaft {i}"
        p, d = s["axis_point"], norm(s["axis_direction"])
        out = side(d)
        radius = s["segments"][0][0] / 2
        if s["kind"] == "free_free":
            ends = [(0.0, -1.0), (s["length"], 1.0)]
        else:
            # Material just below the root beside the shaft, air just above.
            if not two_sided(solid, add(p, out, radius + BESIDE), d, 1.0):
                problems.append(f"{label}: its root is not at station 0")
            ends = [(s["protrusion"], 1.0)]
        for at, sign in ends:
            if not two_sided(solid, add(add(p, d, at), out, 0.75 * radius), d, sign):
                problems.append(f"{label}: its free end is not at station {at}")


def file_facts(path):
    """What the file text states: counts of a few entities, and whether the
    unit context assigns a length unit."""
    text = path.read_text(encoding="latin-1")

    def count(name):
        return len(re.findall(rf"\b{name}\s*\(", text))

    instances = dict(re.findall(r"#(\d+)\s*=\s*([^;]*);", text, re.S))
    has_length = False
    for refs in re.findall(r"GLOBAL_UNIT_ASSIGNED_CONTEXT\s*\(\s*\(([^)]*)\)", text):
        has_length |= any("LENGTH_UNIT" in instances.get(r.strip().lstrip("#"), "") for r in refs.split(","))
    return {"usages": count("NEXT_ASSEMBLY_USAGE_OCCURRENCE"), "definitions": count("PRODUCT_DEFINITION"),
            "surface_models": count("SHELL_BASED_SURFACE_MODEL"), "length_unit": has_length}


def check_refused(path, solids, codes, problems, case):
    facts = file_facts(path)
    shows = {
        "assembly": facts["usages"] > 0,
        "several_definitions": facts["definitions"] >= 2 and facts["usages"] == 0,
        "several_solids": len(solids) >= 2 and facts["definitions"] == 1 and facts["usages"] == 0,
        "no_solid": not solids,
        "unread_shape_item": facts["surface_models"] > 0,
        "length_unit_unreadable": not facts["length_unit"],
    }
    for code in codes:
        if code in shows and not shows[code]:
            problems.append(f"{case}: refused as {code}, but the file does not show it "
                            f"({facts}, {len(solids)} solids)")
    for code, shown in shows.items():
        if shown and code not in codes:
            problems.append(f"{case}: the file shows {code}, but the case is not refused for it")


def check_case(directory, case):
    problems = []
    name = case["id"]
    path = directory / case["file"]
    reader = STEPControl_Reader()
    if reader.ReadFile(str(path)) != IFSelect_RetDone:
        return [f"{name}: Open CASCADE cannot read it"]
    reader.TransferRoots()
    solids = [Solid(as_solid(s)) for i in range(1, reader.NbShapes() + 1)
              for s in explore(reader.Shape(i), TopAbs_SOLID)]
    expect = case["expect"]
    if expect["result"] == "refused":
        stage = max(STAGES.get(code, 0) for code in expect["refusals"])
        if expect["stage"] != stage:
            problems.append(f"{name}: refused at stage {expect['stage']}, but its codes are stage {stage}'s")
        check_refused(path, solids, expect["refusals"], problems, name)
        return problems
    facts = file_facts(path)
    if facts["usages"] or facts["definitions"] != 1 or facts["surface_models"] or not facts["length_unit"]:
        problems.append(f"{name}: accepted, but the file shows {facts}")
    if len(solids) != 1:
        return problems + [f"{name}: {len(solids)} solids, not 1"]
    solid = solids[0]
    if not solid.valid or solid.volume <= 0:
        return problems + [f"{name}: the solid is not valid, or its volume {solid.volume} is not positive"]
    features = expect["features"]
    groups = solid.groups()
    match_segments(groups, expected_segments(features, "holes"), True, problems, name)
    match_segments(groups, expected_segments(features, "shafts"), False, problems, name)
    check_partials(groups, features["partial_cylinders"], problems, name)
    check_planes(solid, features["planes"], problems, name)
    check_counts(solid, features, problems, name)
    check_hole_ends(solid, features["holes"], problems, name)
    check_shaft_ends(solid, features["shafts"], problems, name)
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
