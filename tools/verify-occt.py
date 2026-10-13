#!/usr/bin/env -S uv run --locked --script
# /// script
# requires-python = ">=3.11,<3.14"
# dependencies = ["cadquery-ocp-novtk==8.0.1.1.0"]
# ///
"""Check fitment's test parts, and fitment itself, with Open CASCADE.

    tools/verify-occt.py manifest DIR
    tools/verify-occt.py features [--expect MANIFEST] PATH...
    tools/verify-occt.py socket DIR

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

For an assembly case (docs/research/m3-socket.md, section 13), XCAF's
leaf placements, composed from the root, one to one with the
construction's: product by name, transform within 1e-9. Leaves that
are one product's are told apart only by their transforms.

For a refused case, the facts Open CASCADE or the file text can show: an
assembly usage, two product definitions, two solids, no solid, a surface
model, no length unit; and that the stated gate stage is its codes'.

Not checked here, so fitment's own tests are the only check:

  - a vertex moved off its face, and a flipped same_sense: Open CASCADE
    repairs face orientation as it reads, so it cannot see the flip;
  - what the rules call an end they cannot read (`irregular`, `unknown`)
    and whether a hole is stated as `unknown` rather than known: those are
    the rules' answers, and the oracle checks only geometry.

`features` checks fitment against Open CASCADE instead. PATH is a STEP
file, or a directory: a generated corpus (its manifest's cases), the
fixture tree (its manifest's files), or any other tree (every .stp and
.step file outside ocp/). Each file is read by
the built fitment (target/release/fitment, or $FITMENT) and, if accepted,
by Open CASCADE, whose faces are bound to their #ids through the entity
each was transferred from:

  - every face fitment lists is bound, and every planar face Open CASCADE
    reads is in one of fitment's planes;
  - each planar face's outward normal, and its offset along it, agree;
  - each planar face's area agrees with BRepGProp within 1e-9 relative,
    where fitment's outline is known, plus its perimeter times the solid's
    max_vertex_offset_mm: fitment measures loops through the vertices as
    stated, Open CASCADE along the stated curves, and moving each vertex by
    up to d changes a loop's area by at most d times its perimeter. Faces
    that need the slack are tallied apart;
  - each cylinder's faces are cylinders in Open CASCADE with its radius,
    on its axis line, with its sense (a point just outside the wall is
    material for a hole);
  - one fitment calls full (or full with one end irregular) goes all the
    way around just inside each station and past it on an irregular side,
    by points classified against the faces (parameter bounds are a bounding
    box: a face with a cutout fills them);
  - its stations are the circles on the cylinder among the faces' edges;
    its faces run past the stations on exactly the side it calls
    irregular; a partial sweep is within the faces' parameter bounds. A cylinder whose faces go all the
    way around between its stations but fitment does not call full is a
    false negative, tallied apart;
  - each through or blind hole, and each pin or boss, passes the manifest
    mode's tests on its axis line: air along each segment and just past
    each opening, the face around each opening at its rim, material just
    past each bottom at its station, a drill point's or countersink's cone
    with its apex and angle, an annular bottom's post, a shaft's root and
    free ends;
  - the solid is valid with positive volume: the only check on a shell
    written inside out (design, R11).

With --expect, the run's tallies must equal the `oracle` entry of that
JSON file (fixtures/manifest.json pins the fixtures'): a fixture that
stops being checked, or a check that stops running, then fails.

Exits 1 on any disagreement, after listing every one.
"""

from __future__ import annotations

import json
import math
import os
import re
import subprocess
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


def on_face(face, bounds):
    """A (u, v) inside the face: the middle of its parameter bounds if that
    is on it, else the first point of a grid over them that is. A face with
    a cutout may not cover the middle of its bounds."""
    from OCP.BRepClass import BRepClass_FaceClassifier
    from OCP.gp import gp_Pnt2d

    umin, umax, vmin, vmax = bounds
    grid = [(0.5, 0.5)] + [(i / 8, j / 8) for i in range(1, 8) for j in range(1, 8)]
    for a, b in grid:
        u, v = umin + a * (umax - umin), vmin + b * (vmax - vmin)
        if BRepClass_FaceClassifier(face, gp_Pnt2d(u, v), 1e-9).State() == TopAbs_IN:
            return u, v
    return (umin + umax) / 2, (vmin + vmax) / 2


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
        mid = surface.Value(*on_face(face, (umin, umax, vmin, vmax)))
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
                "x": xyz(c.XAxis().Direction()), "y": xyz(c.YAxis().Direction()), "face": face}

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


def around(solid, centre, direction, distances, sign):
    """True if, at one of `distances` from `centre` in one of eight
    directions around the axis `direction`, material lies just before the
    point and air just past it. One direction can fall into a neighbouring
    feature, and one distance into an edge round or fillet that the rim
    lies beyond, or a bore inside a hollow end."""
    x = side(direction)
    y = cross(direction, x)
    for distance in distances:
        for k in range(8):
            angle = 2 * math.pi * k / 8
            out = add(scale3(x, math.cos(angle)), y, math.sin(angle))
            if two_sided(solid, add(centre, out, distance), direction, sign):
                return True
    return False


def beside_wall(radius):
    """Distances just outside a wall of `radius` where the face around it
    is probed: within `beside`, closer for a thin wall such as a tube's."""
    return [radius + beside(radius) * k for k in (1, 0.5, 0.25)]


def outside_wall(radius):
    """Those, and further out past a round, fillet or countersink."""
    return beside_wall(radius) + [radius + beside(radius) * k for k in (2, 4, 8)]


def scale3(v, k):
    return tuple(c * k for c in v)


def beside(radius):
    """How far outside a wall of `radius` the face around it is probed: 1 mm,
    or half the radius on a small part, where 1 mm can leave the part."""
    return min(BESIDE, radius / 2)


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
            # Further out only past a round or countersink the walk crossed
            # (features mode says so); otherwise beside the wall, so that a
            # hole through a boss cannot pass on the plate around it.
            reach = outside_wall(radius) if h.get("_far") else beside_wall(radius)
            if not around(solid, add(p, d, at), d, reach, sign):
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
            reach = outside_wall(radius) if s.get("_far") else beside_wall(radius)
            if not around(solid, p, d, reach, 1.0):
                problems.append(f"{label}: its root is not at station 0")
            ends = [(s["protrusion"], 1.0)]
        for at, sign in ends:
            if not around(solid, add(p, d, at), d, [f * radius for f in (0.75, 0.9, 0.5, 0.25)], sign):
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


# -------------------------------------------------------------- assemblies

PLACEMENT_TOL = 1e-9  # mm and rotation entries (P9): answers are rounded to 9 decimals


def xcaf_leaves(path, bind=False):
    """Every leaf occurrence Open CASCADE's XCAF reads, as (product name,
    3x4 rows root <- leaf, placed shape), composed from the free shapes
    down; with `bind`, also a function giving a face's #id (or None)."""
    from OCP.collections import Sequence_TDF_Label
    from OCP.STEPCAFControl import STEPCAFControl_Reader
    from OCP.TCollection import TCollection_ExtendedString
    from OCP.TDataStd import TDataStd_Name
    from OCP.TDF import TDF_Label
    from OCP.TDocStd import TDocStd_Document
    from OCP.TopLoc import TopLoc_Location
    from OCP.XCAFDoc import XCAFDoc_DocumentTool

    doc = TDocStd_Document(TCollection_ExtendedString("MDTV-XCAF"))
    reader = STEPCAFControl_Reader()
    reader.SetNameMode(True)
    if reader.ReadFile(str(path)) != IFSelect_RetDone or not reader.Transfer(doc):
        return None
    tool = XCAFDoc_DocumentTool.ShapeTool_s(doc.Main())

    def name_of(label):
        n = TDataStd_Name()
        return n.Get().ToExtString() if label.FindAttribute(TDataStd_Name.GetID_s(), n) else "?"

    out = []

    def walk(label, loc):
        if tool.IsAssembly_s(label):
            comps = Sequence_TDF_Label()
            tool.GetComponents_s(label, comps, False)
            for i in range(1, comps.Length() + 1):
                comp, ref = comps.Value(i), TDF_Label()
                if tool.GetReferredShape_s(comp, ref):
                    walk(ref, loc.Multiplied(tool.GetLocation_s(comp)))
        else:
            t = loc.Transformation()
            out.append((name_of(label), [[t.Value(i, j) for j in range(1, 5)] for i in range(1, 4)],
                        tool.GetShape_s(label).Moved(loc)))

    free = Sequence_TDF_Label()
    tool.GetFreeShapes(free)
    for i in range(1, free.Length() + 1):
        walk(free.Value(i), TopLoc_Location())
    if not bind:
        return out
    from OCP.TopAbs import TopAbs_FORWARD
    model = reader.Reader().StepModel()
    entities = [model.Value(rank) for rank in range(1, model.NbEntities() + 1)]
    by_rank = {id(entity): rank for rank, entity in enumerate(entities, 1)}
    ranks = file_ranks(path)
    ids = {rank: id_ for id_, rank in ranks.items()} if len(ranks) == len(entities) else {}
    transfers = reader.Reader().WS().TransferReader()

    def face_id(face):
        if face is None:
            # Asked whether faces bind at all: not when the file's
            # instances do not each start a line.
            return bool(ids)
        key = face.Located(TopLoc_Location()).Oriented(TopAbs_FORWARD)
        entity = transfers.EntityFromShapeResult(key, 1)
        return None if entity is None else ids.get(by_rank.get(id(entity)))

    # The entities stay alive with the reader the function holds.
    face_id.keep = (reader, doc, entities)
    return out, face_id


def check_assembly(directory, case):
    """An assembly case: Open CASCADE's leaf placements, one to one with the
    construction's. A case whose listing is refused (an edit Open CASCADE
    may read another way) is checked only for being readable."""
    name = case["id"]
    leaves = xcaf_leaves(directory / case["file"])
    if leaves is None:
        return [f"{name}: Open CASCADE cannot read it"]
    expect = case["expect"]
    if expect["listing"]["result"] != "accepted":
        # What Open CASCADE can show of a refusal: two roots are two free
        # shapes. A placement edit it may read another way is not judged.
        if "several_roots" in expect["listing"]["refusals"]:
            roots = {tuple(row[3] for row in m) for _, m, _ in leaves}
            if len(leaves) < 2 or len(roots) < 2:
                return [f"{name}: refused for two roots, but Open CASCADE reads {len(leaves)} leaves"]
        return []
    problems = []
    # XCAF names a leaf by its PRODUCT.name; the manifest keys it by id.
    names = {p["id"]: p["name"] for p in expect["listing"]["parts"]}
    wanted = [(names.get(product, product), m) for product, ms in expect["occurrences"].items() for m in ms]
    if len(wanted) != len(leaves):
        problems.append(f"{name}: Open CASCADE reads {len(leaves)} leaf occurrences, the construction "
                        f"{len(wanted)}")
    left = [(n, m) for n, m, _ in leaves]
    for product, m in wanted:
        # A leaf placed through a mapped item comes back named SOLID (s10).
        at = next((k for k, (n, found) in enumerate(left) if n in (product, "SOLID") and all(
            abs(found[i][j] - m[i][j]) <= PLACEMENT_TOL for i in range(3) for j in range(4))), None)
        if at is None:
            problems.append(f"{name}: no {product} where the construction places one: {m}")
        else:
            left.pop(at)
    return problems


def check_case(directory, case):
    if case.get("kind") == "assembly":
        return check_assembly(directory, case)
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


# ---------------------------------------------------------------- features

AREA_REL = 1e-9  # planar face areas, relative


def file_ranks(path):
    """Each instance's entity number (its 1-based position in the file) by
    #id, as stepq's oracle reads it."""
    text = path.read_text(encoding="latin-1")
    data = text[text.find("DATA;"):]
    ranks = {}
    for match in re.finditer(r"^\s*#(\d+)\s*=", data, re.M):
        ranks[int(match.group(1))] = len(ranks) + 1
    return ranks


def bound_faces(path):
    """Open CASCADE's solids, and each face as placed in the file's root
    shape, by the #id of the entity it was transferred from. Faces that
    bind to no entity are counted."""
    from OCP.TopAbs import TopAbs_FORWARD
    from OCP.TopLoc import TopLoc_Location

    reader = STEPControl_Reader()
    if reader.ReadFile(str(path)) != IFSelect_RetDone:
        raise OSError(f"Open CASCADE cannot read {path}")
    model = reader.StepModel()
    # Number() returns 0 through these bindings, so entities are told apart
    # by identity; the list keeps each binding alive.
    entities = [model.Value(rank) for rank in range(1, model.NbEntities() + 1)]
    by_rank = {id(entity): rank for rank, entity in enumerate(entities, 1)}
    ranks = file_ranks(path)
    if len(ranks) != len(entities):
        # Instances that do not start a line, or #n= in a comment, would
        # shift every later rank: say so rather than misbind.
        raise OSError(f"{path}: {len(ranks)} instances start a line, but Open CASCADE reads "
                      f"{len(entities)}; the oracle binds #ids only in files with one instance per line")
    ids = {rank: id_ for id_, rank in ranks.items()}
    reader.TransferRoots()
    transfers = reader.WS().TransferReader()
    solids, faces, unbound = [], {}, 0
    for i in range(1, reader.NbShapes() + 1):
        for found in explore(reader.Shape(i), TopAbs_SOLID):
            solids.append(Solid(as_solid(found)))
            for shape in explore(found, TopAbs_FACE):
                face = as_face(shape)
                key = face.Located(TopLoc_Location()).Oriented(TopAbs_FORWARD)
                entity = transfers.EntityFromShapeResult(key, 1)
                id_ = None if entity is None else ids.get(by_rank.get(id(entity)))
                if id_ is None:
                    unbound += 1
                else:
                    faces[id_] = face
    return solids, faces, unbound


def plane_facts(face):
    """A planar face's outward normal, the location its plane states, and
    its area; None for a face on another surface."""
    surface = BRepAdaptor_Surface(face)
    if surface.GetType() != Kind.GeomAbs_Plane:
        return None
    position = surface.Plane().Position()
    normal = xyz(position.Direction())
    if face.Orientation() == TopAbs_REVERSED:
        normal = tuple(-c for c in normal)
    props = GProp_GProps()
    BRepGProp.SurfaceProperties_s(face, props)
    return normal, xyz(position.Location()), props.Mass()


EXTENT_SLACK = 1e-6  # mm: Open CASCADE's box of analytic geometry is exact to rounding


def check_extent(path, extent, solid, tally):
    """fitment's extent holds Open CASCADE's box of the solid."""
    from OCP.Bnd import Bnd_Box
    from OCP.BRepBndLib import BRepBndLib

    box = Bnd_Box()
    BRepBndLib.AddOptimal_s(solid, box, False, False)
    low = box.CornerMin()
    high = box.CornerMax()
    tally["extents"] += 1
    found_low, found_high = xyz(low), xyz(high)
    if any(extent["min_mm"][i] > found_low[i] + EXTENT_SLACK or extent["max_mm"][i] < found_high[i] - EXTENT_SLACK
           for i in range(3)):
        return [f"{path}: the extent {extent} does not hold Open CASCADE's box {found_low} to {found_high}"]
    return []


def check_features(path, binary, tally):
    """fitment's planes of one file against Open CASCADE's faces."""
    run = subprocess.run([str(binary), "features", str(path)], capture_output=True)
    if run.returncode not in (0, 3):
        return [f"{path}: fitment exited {run.returncode}: {run.stderr.decode(errors='replace').strip()}"]
    document = json.loads(run.stdout)
    if document["status"] != "accepted":
        tally["refused"] += 1
        return []
    tally["accepted"] += 1
    part, problems = document["part"], []
    solids, faces, unbound = bound_faces(path)
    if len(solids) != 1 or not solids[0].valid or solids[0].volume <= 0:
        problems.append(f"{path}: Open CASCADE reads {len(solids)} solids; the first is "
                        f"{'valid' if solids and solids[0].valid else 'not valid'}, volume "
                        f"{solids[0].volume if solids else None}")
    listed = [f["face"] for p in part["planes"] for f in p["faces"]]
    listed += [face for key in ("cylinders", "cones", "tori") for g in part[key] for face in g["faces"]]
    listed += [o["face"] for o in part["other_faces"]]
    missing = sorted(set(listed) - set(faces))
    if missing or unbound:
        problems.append(f"{path}: faces fitment lists that Open CASCADE did not bind: {missing[:5]}"
                        f" ({len(missing)}); {unbound} of its faces bound to no entity")
    planar = {id_ for id_, face in faces.items() if plane_facts(face) is not None}
    stray = sorted(planar - {f["face"] for p in part["planes"] for f in p["faces"]})
    if stray:
        problems.append(f"{path}: planar faces in none of fitment's planes: {stray[:5]} ({len(stray)})")
    offset = part["solids"][0]["max_vertex_offset_mm"]
    if solids and part["extent"] is not None:
        problems += check_extent(path, part["extent"], solids[0].solid, tally)
    if solids:
        problems += check_cylinders(path, part, solids[0], faces, tally)
        # Holes and shafts, by the manifest mode's two-sided tests on the
        # axis line: air past an opening, material past a bottom, and so on.
        holes = [as_manifest_hole(h, part) for h in part["holes"]]
        shafts = [as_manifest_shaft(s) for s in part["shafts"] if s["kind"] in ("free_free", "root_free")]
        tally["holes probed"] += sum(1 for h in holes if h["kind"] in ("through", "blind"))
        tally["shafts probed"] += len(shafts)
        problems += shaft_ends(path, part["shafts"])
        problems += reached_through(path, part["holes"], solids[0])
        check_hole_ends(solids[0], holes, problems, str(path))
        check_shaft_ends(solids[0], shafts, problems, str(path))
    for plane in part["planes"]:
        n = plane["normal"]
        problems += check_plane(path, plane)
        for face in plane["faces"]:
            if face["face"] not in faces:
                continue  # reported as not bound above
            facts = plane_facts(faces[face["face"]])
            if facts is None:
                problems.append(f"{path}: face #{face['face']} is in {plane['id']}, but Open CASCADE "
                                f"reads it on a {BRepAdaptor_Surface(faces[face['face']]).GetType()}")
                continue
            normal, location, area = facts
            tally["plane faces"] += 1
            where = f"{path}: face #{face['face']} ({plane['id']})"
            if math.hypot(*cross(normal, n)) > ANGULAR or dot(normal, n) <= 0:
                problems.append(f"{where}: normal {n}, Open CASCADE {normal}")
                continue
            if abs(dot(n, location) - face["offset_mm"]) > LINEAR:
                problems.append(f"{where}: offset {face['offset_mm']}, Open CASCADE {dot(n, location)}")
            if face["area_mm2"] is None:
                tally["unknown outlines"] += 1
                continue
            error = abs(face["area_mm2"] - area)
            exact = AREA_REL * max(area, 1.0)
            slack = perimeter(face["outline"]) * offset
            if error <= exact:
                tally["areas"] += 1
            elif error <= exact + slack:
                tally["areas within the vertex offsets"] += 1
            else:
                problems.append(f"{where}: area {face['area_mm2']}, Open CASCADE {area}, "
                                f"beyond {exact + slack:.3g} mm2")
    return problems


VERTEX_SIDE = 1e-3  # mm: how far past the stations a full wall's vertices may lie (F4)
FULL_CLASSES = ("full", "full_one_end_irregular")


def perpendicular(z):
    """A unit vector perpendicular to unit z."""
    least = min(range(3), key=lambda i: abs(z[i]))
    e = tuple(1.0 if i == least else 0.0 for i in range(3))
    return norm(add(e, z, -dot(e, z)))


def cover_at(members, point, axis, radius, station, samples=360):
    """The angle, in degrees, of the circle at `station` along the axis that
    lies on the faces: points classified against each face, not the faces'
    parameter bounds, which a face with a cutout fills."""
    from OCP.BRepClass import BRepClass_FaceClassifier
    from OCP.gp import gp_Pnt2d
    from OCP.TopAbs import TopAbs_ON

    x = perpendicular(axis)
    y = cross(axis, x)
    centre = add(point, axis, station)
    covered = 0
    for k in range(samples):
        angle = 2 * math.pi * k / samples
        p = add(add(centre, x, radius * math.cos(angle)), y, radius * math.sin(angle))
        for m in members:
            off = sub(p, m["loc"])
            u = math.atan2(dot(off, m["y"]), dot(off, m["x"]))
            while u < m["u"][0] - 1e-9:
                u += 2 * math.pi
            while u > m["u"][0] + 2 * math.pi:
                u -= 2 * math.pi
            state = BRepClass_FaceClassifier(m["face"], gp_Pnt2d(u, dot(off, m["d"])), 1e-7).State()
            if state in (TopAbs_IN, TopAbs_ON):
                covered += 1
                break
    return 360.0 * covered / samples


def circle_stations(members, point, axis, radius):
    """The stations, along the axis from its point, of the faces' edges that
    are circles on the cylinder."""
    from OCP.GeomAbs import GeomAbs_CurveType

    found = set()
    for m in members:
        for e in m["edges"]:
            curve = BRepAdaptor_Curve(as_edge(e))
            if curve.GetType() != GeomAbs_CurveType.GeomAbs_Circle:
                continue
            circle = curve.Circle()
            centre, n = xyz(circle.Location()), xyz(circle.Axis().Direction())
            if (abs(circle.Radius() - radius) <= LINEAR and math.hypot(*cross(n, axis)) * radius <= LINEAR
                    and same_line(point, axis, centre, axis)):
                found.add(round(dot(sub(centre, point), axis), 9))
    return sorted(found)


def levels(cylinder):
    """Where a full cylinder's wall must go all the way around: just inside
    each station, toward each neighbouring station, and just past it on an
    irregular side."""
    kind, stations = cylinder["class"]["kind"], cylinder["stations_mm"]
    out = []
    for i, s in enumerate(stations):
        before = stations[i - 1] if i > 0 else None
        after = stations[i + 1] if i + 1 < len(stations) else None
        if before is not None:
            out.append(s - min(0.01, (s - before) / 4))
        if after is not None:
            out.append(s + min(0.01, (after - s) / 4))
    if kind == "full_one_end_irregular" and stations:
        side = cylinder["class"]["irregular"]
        out.append(stations[0] - 0.01 if side == "start" else stations[-1] + 0.01)
    return out


def check_cylinders(path, part, solid, faces, tally):
    """fitment's cylinders against Open CASCADE's cylindrical faces."""
    problems = []
    for cylinder in part["cylinders"]:
        where = f"{path}: {cylinder['id']}"
        members = []
        for id_ in cylinder["faces"]:
            if id_ not in faces:
                continue  # reported as not bound
            surface = BRepAdaptor_Surface(faces[id_], True)
            if surface.GetType() != Kind.GeomAbs_Cylinder:
                problems.append(f"{where}: face #{id_} is a {surface.GetType()} in Open CASCADE")
                continue
            members.append(solid.cylinder(faces[id_], surface))
        if not members:
            continue
        tally["cylinders"] += 1
        point, axis, radius = cylinder["axis"]["point_mm"], cylinder["axis"]["direction"], cylinder["radius_mm"]
        hole = cylinder["sense"] == "hole"
        for member in members:
            if abs(member["r"] - radius) > LINEAR or not same_line(point, axis, member["loc"], member["d"]):
                problems.append(f"{where}: radius {radius} on the axis through {point} along "
                                f"{axis}; Open CASCADE {member['r']} through {member['loc']} along {member['d']}")
            if member["concave"] != hole:
                problems.append(f"{where}: sense {cylinder['sense']}, but Open CASCADE finds material "
                                f"{'outside' if member['concave'] else 'inside'} the wall")
        problems += check_class(where, cylinder, members, tally)
    return problems


def check_class(where, cylinder, members, tally):
    """A cylinder's class, stations and extent against its faces."""
    problems = []
    point, axis, radius = cylinder["axis"]["point_mm"], cylinder["axis"]["direction"], cylinder["radius_mm"]
    kind, stations = cylinder["class"]["kind"], cylinder["stations_mm"]
    bounds = Solid.sweep(members)  # from parameter bounds: an upper bound on any cover
    if kind in FULL_CLASSES:
        tally["full cylinders"] += 1
        for level in levels(cylinder):
            cover = cover_at(members, point, axis, radius, level)
            if cover < 360:
                problems.append(f"{where}: {kind}, but at {level:.6g} mm its faces cover {cover:.1f} degrees")
    elif bounds >= 360 - SWEEP_DEG:
        middles = [(a + b) / 2 for a, b in zip(stations, stations[1:])] or [None]
        if middles[0] is not None and all(cover_at(members, point, axis, radius, t) >= 360 for t in middles):
            tally["cylinders full only in Open CASCADE"] += 1
    if kind == "partial" and cylinder["class"]["sweep_rad"] is not None:
        if math.degrees(cylinder["class"]["sweep_rad"]) > bounds + SWEEP_DEG:
            problems.append(f"{where}: sweeps {math.degrees(cylinder['class']['sweep_rad'])} degrees, but its "
                            f"faces' parameter bounds cover {bounds}")
    # Stations are the circles on the cylinder, from the edges themselves.
    circles = circle_stations(members, point, axis, radius)
    if stations and (not circles or any(min(abs(s - c) for c in circles) > LINEAR for s in stations)
                     or abs(stations[0] - circles[0]) > LINEAR or abs(stations[-1] - circles[-1]) > LINEAR):
        problems.append(f"{where}: stations {stations}, but its circles lie at {circles}")
    if kind in ("full", "full_one_end_irregular") and not stations:
        problems.append(f"{where}: {kind} with no station")
    elif kind in ("full", "full_one_end_irregular"):
        spans = []
        for member in members:
            sign = 1.0 if dot(member["d"], axis) > 0 else -1.0
            base = dot(sub(member["loc"], point), axis)
            spans += [base + sign * s for s in member["span"]]
        below = min(spans) < stations[0] - VERTEX_SIDE
        above = max(spans) > stations[-1] + VERTEX_SIDE
        irregular = cylinder["class"].get("irregular")
        if (below, above) != (irregular == "start", irregular == "finish"):
            problems.append(f"{where}: {kind} {irregular or ''}, but its faces span {min(spans):.6g} to "
                            f"{max(spans):.6g} past stations {stations[0]:.6g} to {stations[-1]:.6g}")
    return problems


def check_plane(path, plane):
    """A plane's own values against its faces': its offset within its
    spread of each face's, the spread within 1e-6 mm, and its area the sum
    of theirs, or null when any is."""
    problems, where = [], f"{path}: {plane['id']}"
    if not 0 <= plane["spread_mm"] <= LINEAR:
        problems.append(f"{where}: spread {plane['spread_mm']}")
    for face in plane["faces"]:
        if abs(plane["offset_mm"] - face["offset_mm"]) > plane["spread_mm"] + 1e-12:
            problems.append(f"{where}: offset {plane['offset_mm']}, face #{face['face']}'s {face['offset_mm']}")
    areas = [face["area_mm2"] for face in plane["faces"]]
    if None in areas:
        if plane["area_mm2"] is not None:
            problems.append(f"{where}: area {plane['area_mm2']} with an unknown face outline")
    elif plane["area_mm2"] is None or abs(plane["area_mm2"] - sum(areas)) > AREA_REL * max(sum(areas), 1.0):
        problems.append(f"{where}: area {plane['area_mm2']}, its faces' sum {sum(areas)}")
    return problems


def reached_through(path, holes, solid):
    """An opening whose rim lies past its wall's end was reached through
    something: a countersink, a round, or, if neither is reported, nothing
    that widens. Air just outside the wall halfway between them means an
    unreported widening, such as a countersink fitment dropped."""
    problems = []
    for h in holes:
        p, d = h["axis"]["point_mm"], norm(h["axis"]["direction"])
        radius = min(s["diameter_mm"] for s in h["segments"]) / 2
        tori = any(t.startswith("torus-") for t in h["transitions"])
        for end in h["ends"]:
            if end["kind"] != "opening" or end["countersink"] is not None or tori:
                continue
            if abs(end["rim_mm"] - end["at_mm"]) <= LINEAR:
                continue
            middle = add(p, d, (end["rim_mm"] + end["at_mm"]) / 2)
            out = side(d)
            y = cross(d, out)
            probes = [add(middle, add(scale3(out, math.cos(a)), y, math.sin(a)), radius + beside(radius) / 4)
                      for a in (2 * math.pi * k / 8 for k in range(8))]
            if all(solid.outside(q) for q in probes):
                problems.append(f"{path}: {h['id']}: its opening at {end['rim_mm']} widens past its wall's end "
                                f"at {end['at_mm']}, but no countersink is reported")
    return problems


def shaft_ends(path, shafts):
    """A rooted shaft starts at its root, and every known shaft's length or
    protrusion spans its rims."""
    problems = []
    for s in shafts:
        kinds = [e["kind"] for e in s["ends"]]
        rims = [e.get("rim_mm") for e in s["ends"]]
        span = s["protrusion_mm"] if s["kind"] == "root_free" else s["length_mm"]
        wanted = {"root_free": ["root", "free_end"], "free_free": ["free_end", "free_end"],
                  "root_root": ["root", "root"]}.get(s["kind"])
        if wanted is None:
            continue
        if kinds != wanted or rims[0] != 0.0 or span is None or abs(rims[1] - span) > LINEAR:
            problems.append(f"{path}: {s['id']}: {s['kind']} with ends {kinds} at {rims}, spanning {span}")
    return problems


def as_manifest_end(end):
    """A fitment hole end in the manifest's terms."""
    if end["kind"] == "opening":
        out = {"kind": "opening", "rim": end["rim_mm"]}
        if end["countersink"] is not None:
            out["countersink"] = {"outer_diameter": end["countersink"]["outer_diameter_mm"],
                                  "angle_deg": math.degrees(end["countersink"]["angle_rad"])}
        return out
    if end["kind"] != "bottom":
        return {"kind": end["kind"]}
    bottom = end["bottom"]
    out = {"kind": "bottom", "bottom": bottom["kind"], "at": end["at_mm"]}
    if bottom["kind"] in ("point", "cone_then_flat"):
        out["angle_deg"] = math.degrees(bottom["angle_rad"])
    if bottom["kind"] == "point":
        out["tip_length"] = bottom["tip_length_mm"]
    if bottom["kind"] in ("cone_then_flat", "annular"):
        out["floor_at"] = bottom["floor_at_mm"]
    if bottom["kind"] == "annular":
        out["post_diameter"] = 2 * bottom["post_radius_mm"]
    return out


def as_manifest_hole(hole, part):
    """A fitment hole in the manifest's terms, for check_hole_ends; `_far`
    when its walks crossed a torus or a countersink, past which the face
    around an opening may lie."""
    tori = any(t.startswith("torus-") for t in hole["transitions"])
    sinks = any(e.get("countersink") for e in hole["ends"])
    out = {"kind": hole["kind"], "axis_point": hole["axis"]["point_mm"], "_far": tori or sinks,
           "axis_direction": hole["axis"]["direction"],
           "segments": [[s["diameter_mm"], s["from_mm"], s["to_mm"]] for s in hole["segments"]],
           "ends": [as_manifest_end(e) for e in hole["ends"]]}
    if hole["length_mm"] is not None:
        out["length"] = hole["length_mm"]
    if hole["depth_mm"] is not None:
        out["depth"] = hole["depth_mm"]
    return out


def as_manifest_shaft(shaft):
    """A fitment shaft in the manifest's terms, for check_shaft_ends; `_far`
    when its walks crossed a transition, such as a root fillet."""
    out = {"kind": shaft["kind"], "axis_point": shaft["axis"]["point_mm"], "_far": bool(shaft["transitions"]),
           "axis_direction": shaft["axis"]["direction"],
           "segments": [[s["diameter_mm"], s["from_mm"], s["to_mm"]] for s in shaft["segments"]]}
    if shaft["length_mm"] is not None:
        out["length"] = shaft["length_mm"]
    if shaft["protrusion_mm"] is not None:
        out["protrusion"] = shaft["protrusion_mm"]
    return out


def perimeter(outline):
    """The length of a known outline's loops, in mm."""
    total = 0.0
    for loop in outline["loops"]:
        for segment in loop["segments"]:
            if segment["kind"] == "arc":
                total += segment["radius_mm"] * abs(segment["sweep_rad"])
            else:
                total += math.dist(segment["from_mm"], segment["to_mm"])
    return total


def feature_inputs(path):
    """The STEP files a features PATH names; a path that names none is an
    error, so that a mistyped path cannot pass with nothing checked."""
    if not path.exists():
        sys.exit(f"{path}: no such file or directory")
    found = step_files(path)
    if not found:
        sys.exit(f"{path}: no STEP files")
    return found


def step_files(path):
    if path.is_file():
        return [path]
    manifest = path / "manifest.json"
    if manifest.is_file():
        listed = json.loads(manifest.read_text(encoding="utf-8"))
        if "cases" in listed:
            # Assemblies are not parts: `manifest` mode checks them.
            return [path / case["file"] for case in listed["cases"] if case.get("kind") != "assembly"]
        return [path / fixture["path"] for fixture in listed["fixtures"]]
    return sorted(p for p in path.rglob("*") if p.suffix.lower() in (".stp", ".step")
                  and "ocp" not in p.relative_to(path).parts)


def features_main(argv):
    import argparse

    parser = argparse.ArgumentParser(prog="verify-occt.py features")
    parser.add_argument("--expect", metavar="MANIFEST", help="a JSON file whose `oracle` entry pins the tallies")
    parser.add_argument("paths", nargs="+", metavar="PATH")
    args = parser.parse_args(argv)
    pinned, paths = None, args.paths
    if args.expect is not None:
        pinned = json.loads(Path(args.expect).read_text(encoding="utf-8")).get("oracle")
        if not isinstance(pinned, dict):
            sys.exit(f"{args.expect} has no `oracle` entry of tallies to expect")
    binary = Path(os.environ.get("FITMENT", Path(__file__).resolve().parent.parent / "target/release/fitment"))
    if not binary.is_file():
        sys.exit(f"no fitment at {binary}: run cargo build --release -p fitment, or set FITMENT")
    tally = {"accepted": 0, "refused": 0, "plane faces": 0, "areas": 0, "areas within the vertex offsets": 0,
             "unknown outlines": 0, "cylinders": 0, "full cylinders": 0,
             "cylinders full only in Open CASCADE": 0, "holes probed": 0, "shafts probed": 0, "extents": 0}
    problems = []
    for path in [p for arg in paths for p in feature_inputs(Path(arg))]:
        try:
            found = check_features(path, binary, tally)
        except OSError as error:
            found = [str(error)]
        problems += found
        print(f"{'ok  ' if not found else 'FAIL'} {path}")
    for problem in problems:
        print(problem, file=sys.stderr)
    if pinned is not None and pinned != tally:
        changed = {k: (pinned.get(k), tally.get(k)) for k in sorted(set(pinned) | set(tally))
                   if pinned.get(k) != tally.get(k)}
        problems.append(f"the tallies differ from the pinned ones (pinned, now): {changed}")
        print(problems[-1], file=sys.stderr)
    print(", ".join(f"{v} {k}" for k, v in tally.items()) + f"; {len(problems)} disagreements with Open CASCADE")
    sys.exit(1 if problems else 0)


def quiet():
    messenger = Message.DefaultMessenger_s()
    printers = messenger.Printers()
    for i in range(printers.Length(), 0, -1):
        messenger.RemovePrinter(printers.Value(i))


# ------------------------------------------------------------------ sockets


def apply(m, p):
    """A 3x4 transform applied to point p."""
    return [sum(m[i][j] * p[j] for j in range(3)) + m[i][3] for i in range(3)]


def perpendiculars(d):
    """Two unit vectors perpendicular to unit d and to each other."""
    a = [1.0, 0.0, 0.0] if abs(d[0]) < 0.9 else [0.0, 1.0, 0.0]
    u = [d[1] * a[2] - d[2] * a[1], d[2] * a[0] - d[0] * a[2], d[0] * a[1] - d[1] * a[0]]
    n = math.sqrt(sum(x * x for x in u))
    u = [x / n for x in u]
    w = [d[1] * u[2] - d[2] * u[1], d[2] * u[0] - d[0] * u[2], d[0] * u[1] - d[1] * u[0]]
    return u, w


def ring(m, axis, station, radius):
    """Eight points at `radius` about `axis` at `station`, in the root frame."""
    p, d = axis["point_mm"], axis["direction"]
    u, w = perpendiculars(d)
    out = []
    for k in range(8):
        c, s = math.cos(k * math.pi / 4), math.sin(k * math.pi / 4)
        out.append(apply(m, [p[i] + station * d[i] + radius * (c * u[i] + s * w[i]) for i in range(3)]))
    return out


def inside_any(point, shapes):
    return any(BRepClass3d_SolidClassifier(shape, gp_Pnt(*point), 1e-7).State() == TopAbs_IN for shape in shapes)


def span_problems(where, kind, axis, span, m, target, others, tally):
    """What Open CASCADE sees that one span misstates: a clearance span's
    gap (between the clear diameter and the hole's, or the shaft's and its
    limit) is air in every other solid, and an interference span's
    overlap is material of both the target and another solid."""
    tally[f"{kind} spans"] += 1
    mid = (span["from_mm"] + span["to_mm"]) / 2
    own = span["hole_diameter_mm" if kind == "bore" else "shaft_diameter_mm"]
    clear = span["clear_diameter_mm"]
    if span["to_mm"] - span["from_mm"] <= 0.01:
        tally["touch spans"] = tally.get("touch spans", 0) + 1
        return []  # a touch (B5): nothing to probe
    if span["fit"] == "clearance" and clear is not None and abs(clear - own) > 0.05:
        radius = (clear + own) / 4
        for point in ring(m, axis, mid, radius):
            if inside_any(point, others):
                return [f"{where}: {span['fit']} with clear diameter {clear}, but Open CASCADE finds material "
                        f"at radius {radius:.4f} at {point}"]
    if span["fit"] == "interference":
        if clear is not None and abs(clear - own) > 0.002:
            # Between the target's surface and the clear diameter, where
            # the overlap is: some point there in the target and in another
            # solid (an occupant's own wall alone is no overlap).
            radii = [(own + (clear - own) * k / 8) / 2 for k in range(1, 8)]
            if not any(inside_any(point, [target]) and inside_any(point, others)
                       for radius in radii for point in ring(m, axis, mid, radius)):
                return [f"{where}: interference to clear diameter {clear}, but Open CASCADE finds no point "
                        f"between it and {own} in both the part and another solid"]
        else:
            radius = own / 2 - 0.001 if kind == "bore" else own / 2 + 0.001
            if not any(inside_any(point, others) for point in ring(m, axis, mid, radius)):
                return [f"{where}: interference, but Open CASCADE finds no other solid at radius {radius:.4f}"]
    return []


NEAR = 1.0  # mm: fitment's NEAR_MM, the near band of a contact (C5, Z6)
COS_LIMIT = math.sqrt(1 - 0.01 ** 2)  # fitment's PARALLEL_SINE_REL


def turn(m, v):
    """A 3x4 transform's rotation applied to vector v."""
    return [sum(m[i][j] * v[j] for j in range(3)) for i in range(3)]


def planar_faces(shape):
    """(face, outward normal, a point of its plane, box) for every planar
    face of `shape`."""
    from OCP.Bnd import Bnd_Box
    from OCP.BRepBndLib import BRepBndLib
    out = []
    for face in explore(shape, TopAbs_FACE):
        face = as_face(face)
        facts = plane_facts(face)
        if facts is None:
            continue
        box = Bnd_Box()
        BRepBndLib.Add_s(face, box)
        out.append((face, facts[0], facts[1], box))
    return out


def faces_on(faces, normal, offset):
    """The faces of `faces` on the plane of outward `normal` and `offset`."""
    return [f for f in faces if dot(f[1], normal) >= 1 - 1e-9 and abs(dot(f[2], normal) - offset) <= 1e-5]


def distance(a, b):
    from OCP.BRepExtrema import BRepExtrema_DistShapeShape
    measure = BRepExtrema_DistShapeShape(a, b)
    return measure.Value() if measure.IsDone() else math.inf


def common_area(face, other, normal, gap):
    """The area `other`, moved by `gap` against `normal` onto `face`'s
    plane, shares with `face`."""
    from OCP.BRepAlgoAPI import BRepAlgoAPI_Common
    from OCP.BRepBuilderAPI import BRepBuilderAPI_Transform
    from OCP.gp import gp_Trsf, gp_Vec
    t = gp_Trsf()
    t.SetTranslation(gp_Vec(*[-gap * c for c in normal]))
    moved = BRepBuilderAPI_Transform(other, t, True).Shape()
    common = BRepAlgoAPI_Common(face, moved)
    if not common.IsDone():
        return math.inf
    props = GProp_GProps()
    BRepGProp.SurfaceProperties_s(common.Shape(), props)
    return props.Mass()


def seat_problems(where, socket, m, target, others, tally):
    """C1 to C5 and L1, L2 against Open CASCADE, both ways: each seat's
    plane is a face of the target with another solid within the near band
    of it; each grip is the separation of its planes (or from its cone's
    apex); and every target face with an opposed face of another solid
    within the band, sharing area with it, is in a seat."""
    problems = []
    ours = planar_faces(target)
    theirs = [g for o in others for g in planar_faces(o)]
    planes, apexes, seat_faces = {}, {}, {}
    for requirement in socket["requirements"]:
        q = requirement["requires"]
        if q["kind"] == "seat":
            tally["seats"] += 1
            n = turn(m, q["normal"])
            offset = dot(n, apply(m, [c * q["offset_mm"] for c in q["normal"]]))
            planes[q["plane"]] = (n, offset)
            faces = faces_on(ours, n, offset)
            seat_faces[q["plane"]] = faces
            if not faces:
                problems.append(f"{where} {requirement['id']}: the part has no face on that plane")
                continue
            # An opposed face of another solid within the band, sharing area
            # with one of the seat's faces; the gaps' signs give the flags.
            gaps = [dot(n, sub(gp, f[2])) for f in faces for g, gn, gp, gb in theirs
                    if dot(gn, n) <= -COS_LIMIT and abs(dot(n, sub(gp, f[2]))) <= NEAR
                    and f[3].Distance(gb) <= NEAR + 1e-6 and common_area(f[0], g, n, dot(n, sub(gp, f[2]))) > 1e-6]
            unknown = all("unknown_outline" in b["flags"] for c in q["contacts"] for b in c["by"]) and \
                any(b for c in q["contacts"] for b in c["by"])
            if not gaps and unknown:
                # Outlines fitment could not read: an opposed face within the
                # band near the seat's faces suffices, as fitment could rule
                # out no more. Failing that, an opposed face anywhere on the
                # band's planes (fitment has no box of a face with an unread
                # edge either): counted apart, and pinned.
                near = [dot(n, sub(gp, f[2])) for f in faces for g, gn, gp, gb in theirs
                        if dot(gn, n) <= -COS_LIMIT and abs(dot(n, sub(gp, f[2]))) <= NEAR
                        and f[3].Distance(gb) <= NEAR + 1e-6]
                key = "seats of unknown outline" if near else "seats of unknown outline and no face near"
                gaps = near or [dot(n, sub(gp, f[2])) for f in faces for g, gn, gp, gb in theirs
                                if dot(gn, n) <= -COS_LIMIT and abs(dot(n, sub(gp, f[2]))) <= NEAR]
                tally[key] = tally.get(key, 0) + 1
            if not gaps:
                problems.append(f"{where} {requirement['id']}: no opposed face shares area with it within {NEAR} mm")
                continue
            tau = max(0.01, requirement["floor_mm"])
            derived = ({"near_gap"} if any(g > tau for g in gaps) else set()) | \
                ({"interference"} if any(g < -tau for g in gaps) else set())
            stated = set(requirement["flags"]) & {"near_gap", "interference"}
            if derived != stated:
                problems.append(f"{where} {requirement['id']}: flagged {sorted(stated)}, Open CASCADE measures gaps "
                                f"{[round(g, 6) for g in gaps]}")
        elif q["kind"] == "conical_seat":
            tally["conical seats"] += 1
            apexes[q["cone"]] = apply(m, q["apex_mm"])
            from OCP.BRepAdaptor import BRepAdaptor_Surface as Surface
            found = False
            for face in explore(target, TopAbs_FACE):
                surface = Surface(as_face(face))
                if surface.GetType() == Kind.GeomAbs_Cone:
                    cone = surface.Cone()
                    found = found or (math.dist(xyz(cone.Apex()), apexes[q["cone"]]) <= 1e-5
                                      and abs(2 * abs(cone.SemiAngle()) - q["included_angle_rad"]) <= 1e-9)
            if not found:
                problems.append(f"{where} {requirement['id']}: the part has no cone with that apex and angle")
    for requirement in socket["requirements"]:
        q = requirement["requires"]
        if q["kind"] != "grip":
            continue
        tally["grips"] += 1
        ends = [planes.get(x) or apexes.get(x) for x in q["seats"]]
        plane = next((e for e in ends if isinstance(e, tuple)), None)
        other = next((e for e in ends if e is not plane), None)
        if plane is None or other is None:
            problems.append(f"{where} {requirement['id']}: a seat it names is not in the socket")
            continue
        n, offset = plane
        point = other if isinstance(other, list) else [c * other[1] for c in other[0]]
        separation = abs(dot(n, point) - offset)
        if abs(separation - q["length_mm"]) > 1e-5:
            problems.append(f"{where} {requirement['id']}: {q['length_mm']} long, Open CASCADE measures {separation}")
    # L1, the other direction: two seats of opposed planes whose faces share
    # area along the normal have a grip.
    gripped = {tuple(sorted(r["requires"]["seats"])) for r in socket["requirements"]
               if r["requires"]["kind"] == "grip"}
    ids = sorted(planes)
    for i, a in enumerate(ids):
        for b in ids[i + 1:]:
            (na, oa), (nb, ob) = planes[a], planes[b]
            if dot(na, nb) > -COS_LIMIT:
                continue
            tally["seat pairs"] += 1
            overlap = any(common_area(fa[0], fb[0], na, dot(na, sub(fb[2], fa[2]))) > 1e-6
                          for fa in seat_faces.get(a, []) for fb in seat_faces.get(b, []))
            if overlap and tuple(sorted([a, b])) not in gripped:
                problems.append(f"{where}: seats {a} and {b} face each other over common area, and no grip holds them")
    # The other direction: every opposed face within the band that shares area.
    for face, n, point, box in ours:
        offset = dot(n, point)
        if any(dot(p[0], n) >= 1 - 1e-9 and abs(p[1] - offset) <= 1e-5 for p in planes.values()):
            continue
        tally["faces off every seat"] += 1
        for g, gn, gpoint, gbox in theirs:
            if dot(gn, n) > -COS_LIMIT:
                continue
            gap = dot(n, sub(gpoint, point))
            if abs(gap) > NEAR or box.Distance(gbox) > NEAR + 1e-6:
                continue
            if common_area(face, g, n, gap) > 1e-6:
                problems.append(f"{where}: a face on the plane {r_(n)} at {offset:.6g} touches another solid "
                                f"{gap:.4g} mm away, and no seat holds it")
                break
    return problems


def unstated_floor(where, q, sources, m, target, leaves, mine, placed, tally):
    """I4 the other way: beyond an end of a shaft with no `max_extra_mm`,
    past nothing but air, the part holding that end has no material (its
    hole is no blind one), on the axis or just inside the shaft's rim."""
    if not q["spans"]:
        return []
    lo = min(sp["from_mm"] for sp in q["spans"])
    hi = max(sp["to_mm"] for sp in q["spans"])
    for end, out, span in ((lo, -1, min(q["spans"], key=lambda sp: sp["from_mm"])),
                           (hi, 1, max(q["spans"], key=lambda sp: sp["to_mm"]))):
        holders = {tuple(src["neighbour"]) for src in sources
                   if src["rule"] in ("I1", "I2") and src["span_mm"] is not None
                   and min(abs(x - end) for x in src["span_mm"]) <= 1e-6}
        shapes = [shape for i, (_, lm, shape) in enumerate(leaves) if i != mine and any(
            all(abs(lm[r][c] - placed[h][r][c]) <= 1e-6 for r in range(3) for c in range(4))
            for h in holders if h in placed)]
        others = [shape for i, (_, _, shape) in enumerate(leaves) if i != mine]
        # A shaft wider than its hole (an interference) meets the hole's
        # wall beyond its end, at its rim: only the axis finds a floor.
        radii = (0.0,) if span["fit"] == "interference" else (0.0, 0.98 * span["shaft_diameter_mm"] / 2)
        for holder in shapes:
            tally["ends checked for a floor"] = tally.get("ends checked for a floor", 0) + 1
            for radius in radii:
                step, reach = 0.05, 0.0
                while reach < 200:
                    reach += step
                    points = ring(m, q["axis"], end + out * reach, radius)
                    if any(inside_any(p_, [target]) for p_ in points):
                        break  # the part's own body: no free end here
                    if any(inside_any(p_, [holder]) for p_ in points):
                        return [f"{where} shaft {q['shaft']}: no max_extra_mm, but Open CASCADE finds the part "
                                f"holding it {reach:.3g} mm beyond an end"]
                    if any(inside_any(p_, others) for p_ in points):
                        break  # another part first
                    step = min(1.0, step * 1.5)
    return []


def detail_problems(where, socket, m, target, leaves, mine, neighbours, face_id, tally, touchers):
    """I4's `max_extra_mm`, each seat's touchers, each grip's holes and
    each partial cylinder, against Open CASCADE. A merged socket lists
    the touchers of all its occurrences: each must hold at one of them,
    so `touchers` collects, per (socket, plane, neighbour, face), whether
    one occurrence found it."""
    from OCP.BRepExtrema import BRepExtrema_DistShapeShape

    problems = []
    others = [shape for i, (_, _, shape) in enumerate(leaves) if i != mine]
    requirements = [r["requires"] for r in socket["requirements"]]
    # I4: beyond one end of the shaft, air for max_extra and then material.
    placed_paths = {tuple(n["path"]): [row + [n["transform"]["translation_mm"][i]]
                                       for i, row in enumerate(n["transform"]["rotation"])] for n in neighbours}
    for requirement in socket["requirements"]:
        q = requirement["requires"]
        if q["kind"] == "shaft" and q.get("max_extra_mm") is None:
            problems += unstated_floor(where, q, requirement["sources"], m, target, leaves, mine, placed_paths, tally)
    for q in requirements:
        if q["kind"] != "shaft" or q.get("max_extra_mm") is None:
            continue
        tally["max extras"] += 1
        x = q["max_extra_mm"]
        lo = min(sp["from_mm"] for sp in q["spans"])
        hi = max(sp["to_mm"] for sp in q["spans"])
        # Just inside the shaft's end at each end, where its rim would meet
        # a floor (a ring's floor may be a step, off the axis).
        ok = False
        for end, out, span in ((lo, -1, min(q["spans"], key=lambda sp: sp["from_mm"])),
                               (hi, 1, max(q["spans"], key=lambda sp: sp["to_mm"]))):
            # On the axis (a pin in a drilled hole), or just inside its rim
            # (a ring's floor may be a step): some solid's material begins
            # there, max_extra beyond the end (another part may lie between).
            for radius in (0.0, 0.98 * span["shaft_diameter_mm"] / 2):
                after = ring(m, q["axis"], end + out * (x + 0.01), radius)
                before = ring(m, q["axis"], end + out * (x - 0.01), radius)
                ok = ok or any(inside_any(a_, [o]) and (x <= 0.02 or not inside_any(b_, [o]))
                               for o in others for a_, b_ in zip(after, before))
        if not ok:
            problems.append(f"{where} shaft {q['shaft']}: max_extra_mm {x}, but Open CASCADE finds no floor that far "
                            f"beyond either end")
    # Touchers: the face named, of the neighbour named, opposed to the seat
    # at the gap stated.
    placed = {tuple(n["path"]): [row + [n["transform"]["translation_mm"][i]]
                                 for i, row in enumerate(n["transform"]["rotation"])] for n in neighbours}
    for requirement in socket["requirements"]:
        q, requirement_floor = requirement["requires"], requirement["floor_mm"]
        if q["kind"] != "seat":
            continue
        n = turn(m, q["normal"])
        offset = dot(n, apply(m, [c * q["offset_mm"] for c in q["normal"]]))
        for contact in q["contacts"]:
            for by in contact["by"]:
                key = (where.rsplit(" ", 1)[0] + " " + socket["id"], q["plane"], tuple(by["neighbour"]), by["face"],
                       by["gap_mm"])
                if face_id(None) is False:
                    tally["touchers unbound"] = tally.get("touchers unbound", 0) + 1
                    continue
                t = placed.get(tuple(by["neighbour"]))
                leaves_there = [shape for i, (_, lm, shape) in enumerate(leaves) if i != mine and t is not None
                                and all(abs(lm[r][c] - t[r][c]) <= 1e-6 for r in range(3) for c in range(4))]
                found = False
                for shape in leaves_there:
                    for f in explore(shape, TopAbs_FACE):
                        f = as_face(f)
                        if face_id(f) != by["face"]:
                            continue
                        facts = plane_facts(f)
                        if facts is not None:
                            gap = dot(n, facts[1]) - offset
                            found = found or (dot(facts[0], n) <= -COS_LIMIT and abs(gap - by["gap_mm"]) <= 1e-5)
                            continue
                        # A ring's (H1): a cylinder or cone of a neighbour
                        # the seat has a ring of, its axis along the normal,
                        # ending on the plane.
                        surface = BRepAdaptor_Surface(f)
                        ring_neighbours = {tuple(x["neighbour"]) for x in q["rings"]}
                        if surface.GetType() == Kind.GeomAbs_Cylinder:
                            axis = xyz(surface.Cylinder().Axis().Direction())
                        elif surface.GetType() == Kind.GeomAbs_Cone:
                            axis = xyz(surface.Cone().Axis().Direction())
                        else:
                            found = found or "unknown_outline" in by["flags"]
                            continue
                        if tuple(by["neighbour"]) not in ring_neighbours or abs(dot(axis, n)) < 1 - 1e-6:
                            continue
                        u0, u1, v0, v1 = BRepTools.UVBounds_s(f)
                        ends = [xyz(surface.Value((u0 + u1) / 2, v)) for v in (v0, v1)]
                        tau = max(TOUCH, requirement_floor) + 1e-6
                        found = found or any(
                            abs(dot(n, e) - offset - by["gap_mm"]) <= tau for e in ends)
                touchers[key] = touchers.get(key, False) or bool(found)
    # Grip holes: each a bore or left-out hole of the socket, along the
    # grip's normal.
    axes = {q.get("hole"): q["axis"] for q in requirements if q["kind"] == "bore"}
    axes.update({left["feature"]: left["axis"] for left in socket["left_out"]})
    normals = {q["plane"]: q["normal"] for q in requirements if q["kind"] == "seat"}
    for q in requirements:
        if q["kind"] != "grip":
            continue
        normal = next((normals[x] for x in q["seats"] if x in normals), None)
        for hole in q["holes"]:
            tally["grip holes"] += 1
            axis = axes.get(hole)
            if axis is None or normal is None or abs(dot(axis["direction"], normal)) < 1 - 1e-9:
                problems.append(f"{where} grip {q['seats']}: hole {hole} is no bore or left-out hole along its normal")
    # Partial cylinders: a face of the target on that axis and diameter,
    # with another solid within the near band of it.
    for q in requirements:
        if q["kind"] not in ("partial_bore", "partial_shaft"):
            continue
        tally["partial cylinders"] += 1
        p, d = apply(m, q["axis"]["point_mm"]), turn(m, q["axis"]["direction"])
        faces = []
        for f in explore(target, TopAbs_FACE):
            f = as_face(f)
            surface = BRepAdaptor_Surface(f)
            if surface.GetType() == Kind.GeomAbs_Cylinder:
                c = surface.Cylinder()
                if abs(2 * c.Radius() - q["diameter_mm"]) <= 1e-4 and on_line(
                        [[1, 0, 0, 0], [0, 1, 0, 0], [0, 0, 1, 0]], {"point_mm": p, "direction": d},
                        xyz(c.Location()), xyz(c.Axis().Direction())):
                    faces.append(f)
        near = any(BRepExtrema_DistShapeShape(f, o).Value() <= NEAR for f in faces for o in others)
        if not faces or not near:
            problems.append(f"{where} {q['kind']} {q['cylinder']}: Open CASCADE finds "
                            f"{'no such face' if not faces else 'nothing near it'}")
    return problems


TOUCH = 0.01  # mm: fitment's CONTACT_MM, τ without a floor


def held_ids(socket):
    """What a socket holds by #id: its seats' contact faces; the feature
    numbers its requirements and left-out features name (their ids and
    their `hole`, `shaft`, `plane`, `cylinder`, `cone` or `feature`); and
    the neighbour faces its unread neighbours (U4) list."""
    faces, features, unread = set(), set(), set()
    number = lambda text: [int(x) for x in re.findall(r"-(\d+)", text or "")]
    for r in socket["requirements"]:
        q = r["requires"]
        features.update(number(r["id"]))
        for key in ("hole", "shaft", "plane", "cylinder", "cone", "feature"):
            if isinstance(q.get(key), str):
                features.update(number(q[key]))
        for c in q.get("contacts", []):
            faces.add(c["face"])
        for c in q.get("causes", []):
            if c["cause"] == "neighbour_unread":
                unread.update(c["faces"])
    for left in socket["left_out"]:
        features.update(number(left["feature"]))
    return faces, features, unread


def on_line(m, axis, point, direction):
    """True if the placed `axis` and the line (point, direction) agree."""
    p, d = apply(m, axis["point_mm"]), turn(m, axis["direction"])
    if abs(abs(dot(d, direction)) - 1) > 1e-6:
        return False
    off = sub(point, p)
    across = sub(off, [c * dot(off, d) for c in d])
    return math.sqrt(dot(across, across)) <= 1e-4


def held_by_geometry(face, socket, m):
    """True if a requirement or left-out feature of `socket` lies on the
    (placed) face's surface: a cylinder on a bore's, shaft's, partial's or
    left-out axis and diameter, a plane on a seat's plane, a cone on a
    conical seat's apex."""
    surface = BRepAdaptor_Surface(face)
    kind = surface.GetType()
    items = [r["requires"] for r in socket["requirements"]] + socket["left_out"]
    if kind == Kind.GeomAbs_Cylinder:
        c = surface.Cylinder()
        point, direction, d = xyz(c.Location()), xyz(c.Axis().Direction()), 2 * c.Radius()
        for q in items:
            if "axis" not in q or not on_line(m, q["axis"], point, direction):
                continue
            diameters = [q.get("diameter_mm")] + [s.get("hole_diameter_mm") or s.get("shaft_diameter_mm")
                                                 for s in q.get("spans", [])]
            if any(x is not None and abs(x - d) <= 1e-4 for x in diameters):
                return True
    elif kind == Kind.GeomAbs_Plane:
        facts = plane_facts(face)
        n, point = facts[0], facts[1]
        for q in items:
            if q.get("kind") == "seat":
                qn = turn(m, q["normal"])
                offset = dot(qn, apply(m, [c * q["offset_mm"] for c in q["normal"]]))
                if abs(dot(qn, n)) >= 1 - 1e-9 and abs(dot(qn, point) - offset) <= 1e-5:
                    return True
    elif kind == Kind.GeomAbs_Cone:
        apex = xyz(surface.Cone().Apex())
        for q in items:
            if q.get("kind") == "conical_seat" and math.dist(apex, apply(m, q["apex_mm"])) <= 1e-5:
                return True
    return False


def face_samples(face, edges, n=6):
    """Points of `face` on an n x n grid of its parameters, inside it and
    more than 2τ from its rim."""
    from OCP.BRepBuilderAPI import BRepBuilderAPI_MakeVertex
    from OCP.BRepClass import BRepClass_FaceClassifier
    from OCP.BRepExtrema import BRepExtrema_DistShapeShape
    from OCP.gp import gp_Pnt2d

    surface = BRepAdaptor_Surface(face)
    u0, u1, v0, v1 = BRepTools.UVBounds_s(face)
    out = []
    for i in range(n):
        for j in range(n):
            u, v = u0 + (i + 0.5) / n * (u1 - u0), v0 + (j + 0.5) / n * (v1 - v0)
            if BRepClass_FaceClassifier(face, gp_Pnt2d(u, v), 1e-7).State() != TopAbs_IN:
                continue
            point = surface.Value(u, v)
            to_rim = BRepExtrema_DistShapeShape(BRepBuilderAPI_MakeVertex(point).Vertex(), edges)
            if to_rim.IsDone() and to_rim.Value() <= 2 * TOUCH:
                continue
            out.append(xyz(point))
    return out


def line_touch(face, shape, edges):
    """True if `shape` comes within τ of a point inside `face`, more than
    2τ from its rim, with nothing of `shape` just beneath it (a contact,
    not a crossing)."""
    from OCP.BRepBuilderAPI import BRepBuilderAPI_MakeVertex
    from OCP.BRepExtrema import BRepExtrema_DistShapeShape, BRepExtrema_SupportType
    from OCP.BRepGProp import BRepGProp_Face
    from OCP.gp import gp_Vec

    measure = BRepExtrema_DistShapeShape(face, shape)
    if not measure.IsDone() or measure.Value() > TOUCH:
        return False
    normals = BRepGProp_Face(face)
    for k in range(1, measure.NbSolution() + 1):
        if measure.SupportTypeShape1(k) != BRepExtrema_SupportType.BRepExtrema_IsInFace:
            continue
        to_rim = BRepExtrema_DistShapeShape(BRepBuilderAPI_MakeVertex(measure.PointOnShape1(k)).Vertex(), edges)
        if to_rim.IsDone() and to_rim.Value() <= 2 * TOUCH:
            continue
        u, v = measure.ParOnFaceS1(k)
        at, normal = gp_Pnt(), gp_Vec()
        normals.Normal(u, v, at, normal)
        if normal.Magnitude() == 0:
            continue
        normal.Normalize()
        nv = (normal.X(), normal.Y(), normal.Z())
        beneath = [c - TOUCH / 2 * x for c, x in zip(xyz(at), nv)]
        t1, t2 = perpendiculars(nv)
        around = [beneath] + [[c + sign * TOUCH / 2 * x for c, x in zip(beneath, t)]
                              for t in (t1, t2) for sign in (1, -1)]
        if not any(inside_any(q, [shape]) for q in around):
            return True
    return False


def touch_problems(where, socket, m, target, named, face_id, tally, clash_ok):
    """C6's direction, and A1's: every face of the target that another
    solid comes within τ of, at points sampled inside it, is held by the
    socket (a seat's contact, a feature a requirement names by the
    smallest #id on its plane or cylinder, a requirement on its surface,
    or an unread neighbour's faces); where the two overlap there, by an
    interference requirement on that neighbour, if not so. And every
    target hole with material of another solid on its axis is held."""
    from OCP.Bnd import Bnd_Box
    from OCP.BRep import BRep_Builder
    from OCP.BRepBndLib import BRepBndLib
    from OCP.BRepBuilderAPI import BRepBuilderAPI_MakeVertex
    from OCP.BRepExtrema import BRepExtrema_DistShapeShape
    from OCP.BRepGProp import BRepGProp_Face
    from OCP.gp import gp_Vec
    from OCP.TopoDS import TopoDS_Compound

    faces, features, unread = held_ids(socket)
    interfering = set()
    for r in socket["requirements"]:
        q = r["requires"]
        tight = {"interference", "conflict"} & set(r["flags"]) or any(
            sp.get("fit") == "interference" for sp in q.get("spans", []))
        if tight:
            interfering.update(tuple(src["neighbour"]) for src in r["sources"] if src["neighbour"])

    def rim(face):
        compound, builder = TopoDS_Compound(), BRep_Builder()
        builder.MakeCompound(compound)
        for edge in explore(face, TopAbs_EDGE):
            builder.Add(compound, edge)
        return compound

    boxes = []
    for name, shape, paths in named:
        box = Bnd_Box()
        BRepBndLib.Add_s(shape, box)
        ids = {face_id(as_face(f)) for f in explore(shape, TopAbs_FACE)}
        boxes.append((name, shape, box, paths, bool(ids & unread)))
    ours = [as_face(f) for f in explore(target, TopAbs_FACE)]
    ids = [face_id(f) for f in ours]

    # The smallest #id on each face's surface: the feature's id, for a
    # plane or a lone cylinder.
    def surface_key(face):
        surface = BRepAdaptor_Surface(face)
        if surface.GetType() == Kind.GeomAbs_Plane:
            n, point, _ = plane_facts(face)
            return ("plane", tuple(round(c, 6) for c in n), round(dot(n, point), 5))
        if surface.GetType() == Kind.GeomAbs_Cylinder:
            c = surface.Cylinder()
            d = xyz(c.Axis().Direction())
            d = d if (d[2], d[1], d[0]) > (0, 0, 0) else tuple(-x for x in d)
            p = xyz(c.Location())
            foot = sub(p, [x * dot(p, d) for x in d])
            return ("cylinder", tuple(round(x, 6) for x in d), tuple(round(x, 4) for x in foot), round(c.Radius(), 6))
        return None

    keys = [surface_key(f) for f in ours]
    smallest = {}
    for key, id_ in zip(keys, ids):
        if key is not None and id_ is not None:
            smallest[key] = min(smallest.get(key, id_), id_)
    problems = []
    for face, id_, key in zip(ours, ids, keys):
        if id_ is None:
            tally["touch faces unbound"] = tally.get("touch faces unbound", 0) + 1
            continue
        held = (id_ in faces or id_ in features or smallest.get(key) in features
                or held_by_geometry(face, socket, m))
        box = Bnd_Box()
        BRepBndLib.Add_s(face, box)
        near = [b for b in boxes if box.Distance(b[2]) <= TOUCH]
        samples = face_samples(face, rim(face)) if near else []
        if near and not samples:
            tally["faces too small to sample"] = tally.get("faces too small to sample", 0) + 1
        edges = rim(face) if near else None
        for name, shape, _, paths, unread_neighbour in near:
            touch = overlap = False
            for point in samples:
                if inside_any(point, [shape]):
                    overlap = True
                else:
                    measure = BRepExtrema_DistShapeShape(BRepBuilderAPI_MakeVertex(gp_Pnt(*point)).Vertex(), shape)
                    touch = touch or (measure.IsDone() and measure.Value() <= TOUCH)
            # A line or point contact (a rod on a flat) falls between the
            # samples: Open CASCADE's nearest points inside the face, away
            # from its rim, where the target's side of the face is clear.
            if not touch:
                touch = line_touch(face, shape, edges)
            if not (touch or overlap):
                continue
            tally["touching faces"] += 1
            # Overlapping, the other solid crosses the face (an interference,
            # or a clash): points beside the crossing measure within τ too.
            if touch and not overlap and not (held or unread_neighbour):
                problems.append(f"{where}: face #{id_} touches {name} and no requirement holds it")
                return problems
            if overlap and not (held or unread_neighbour or any(p in interfering for p in paths)):
                # A clash no requirement explains: M7's to report (N3).
                tally["clashes"] = tally.get("clashes", 0) + 1
                if not clash_ok:
                    problems.append(f"{where}: face #{id_} overlaps {name} and no requirement holds it")
                    return problems
        # A1's converse: a hole with another solid's material on its axis.
        surface = BRepAdaptor_Surface(face)
        if surface.GetType() != Kind.GeomAbs_Cylinder:
            continue
        c = surface.Cylinder()
        axis_p, axis_d = xyz(c.Location()), xyz(c.Axis().Direction())
        u0, u1, v0, v1 = BRepTools.UVBounds_s(face)
        at, normal = gp_Pnt(), gp_Vec()
        BRepGProp_Face(face).Normal((u0 + u1) / 2, (v0 + v1) / 2, at, normal)
        foot = add(axis_p, [x * dot(sub(xyz(at), axis_p), axis_d) for x in axis_d])
        if dot((normal.X(), normal.Y(), normal.Z()), sub(foot, xyz(at))) <= 0:
            continue  # a shaft: material inside it is its own
        stations = [v0 + (v1 - v0) * k / 6 for k in range(1, 6)]
        on_axis = [add(axis_p, [x * s_ for x in axis_d]) for s_ in stations]
        for name, shape, other, paths, unread_neighbour in boxes:
            if all(other.IsOut(gp_Pnt(*point)) for point in on_axis):
                continue
            if any(inside_any(point, [shape]) for point in on_axis):
                tally["occupied holes"] = tally.get("occupied holes", 0) + 1
                if not (held or unread_neighbour):
                    problems.append(f"{where}: hole face #{id_} has {name} on its axis and no requirement holds it")
                    return problems
    return problems


def r_(v):
    return [round(x, 6) for x in v]


def fits_extent(shape, extent, m):
    """True if `shape`'s box is `extent`, placed by `m`, to 0.01 mm."""
    from OCP.Bnd import Bnd_Box
    from OCP.BRepBndLib import BRepBndLib
    if extent is None:
        return False
    box = Bnd_Box()
    BRepBndLib.AddOptimal_s(shape, box, False, False)
    lo = [math.inf] * 3
    hi = [-math.inf] * 3
    for k in range(8):
        corner = [extent["max_mm" if k >> i & 1 else "min_mm"][i] for i in range(3)]
        p = apply(m, corner)
        lo = [min(lo[i], p[i]) for i in range(3)]
        hi = [max(hi[i], p[i]) for i in range(3)]
    found = list(xyz(box.CornerMin())) + list(xyz(box.CornerMax()))
    return all(abs(a - b) <= 0.01 for a, b in zip(lo + hi, found))


def socket_problems(directory, case, binary, tally):
    """What Open CASCADE sees that one case's sockets miss: every point of a
    left-out hole's axis, over its wall, must be air in every other solid
    of the assembly (A1), and each bore's or shaft's spans must agree with
    the material around them (span_problems). A neighbour fitment cannot
    read, such as a B-spline bolt, is a solid like any other here, so this
    is the check that sees one."""
    problems = []
    path = directory / case["file"]
    read = xcaf_leaves(path, bind=True)
    if read is None:
        return [f"{case['id']}: Open CASCADE cannot read it"]
    leaves, face_id = read
    touchers = {}
    for target in case["expect"].get("sockets", {}):
        run = subprocess.run([str(binary), "socket", str(path), "--part", target], capture_output=True)
        if run.returncode != 0:
            problems.append(f"{case['id']}: fitment socket --part {target} exited {run.returncode}")
            continue
        document = json.loads(run.stdout)
        extracted = document["extracted"]
        for occurrence in extracted["occurrences"]:
            socket = next(s for s in extracted["sockets"] if s["id"] == occurrence["socket"])
            t = occurrence["transform"]
            m = [row + [t["translation_mm"][i]] for i, row in enumerate(t["rotation"])]
            # Every other solid: every leaf but this occurrence itself, which
            # is the target's product at this placement.
            name = occurrence["definition"]["product"]["name"]
            placed = [i for i, (_, found, _) in enumerate(leaves)
                      if all(abs(found[r][c] - m[r][c]) <= 1e-6 for r in range(3) for c in range(4))]
            mine = [i for i in placed if leaves[i][0] == name]
            if len(mine) != 1:
                # No usable name (an export without product names, or one
                # name for every part): the leaf whose box is the target's
                # extent, placed.
                mine = [i for i in placed if fits_extent(leaves[i][2], extracted["target"]["extent"], m)]
            if len(mine) != 1 and len(placed) > 1 and extracted["target"]["extent"] is None:
                # Parts placed alike, unnamed, and no extent to tell them
                # apart (the moon buggy's mapped items): counted, not probed.
                tally["unmatched occurrences"] = tally.get("unmatched occurrences", 0) + 1
                continue
            if len(mine) != 1:
                problems.append(f"{case['id']} {target}: {len(mine)} leaves at occurrence {occurrence['path']}")
                continue
            others = [shape for i, (_, _, shape) in enumerate(leaves) if i != mine[0]]
            tally["others"] += len(others)
            where = f"{case['id']} {target} {socket['id']}"
            problems += seat_problems(where, socket, m, leaves[mine[0]][2], others, tally)
            neighbour_at = [(tuple(n_["path"]), [row + [n_["transform"]["translation_mm"][r]]
                                                 for r, row in enumerate(n_["transform"]["rotation"])])
                            for n_ in extracted["neighbours"]]
            named = [(leaf[0], leaf[2], [path for path, t in neighbour_at
                                         if all(abs(leaf[1][r][c] - t[r][c]) <= 1e-6 for r in range(3) for c in range(4))])
                     for i, leaf in enumerate(leaves) if i != mine[0]]
            # A clash is a finding of its own where the case says so (N3),
            # and among the fixtures, whose tallies pin it.
            clash_ok = "N3" in case.get("rules", []) or case.get("fixture", False)
            problems += touch_problems(where, socket, m, leaves[mine[0]][2], named, face_id, tally, clash_ok)
            problems += detail_problems(where, socket, m, leaves[mine[0]][2], leaves, mine[0],
                                        extracted["neighbours"], face_id, tally, touchers)
            for requirement in socket["requirements"]:
                requires = requirement["requires"]
                if requires["kind"] in ("bore", "shaft"):
                    for span in requires["spans"]:
                        problems += span_problems(f"{where} {requirement['id']}", requires["kind"],
                                                  requires["axis"], span, m, leaves[mine[0]][2], others, tally)
            for left in socket["left_out"]:
                tally["left out holes"] += 1
                p, d = left["axis"]["point_mm"], left["axis"]["direction"]
                a, b = left["from_mm"], left["to_mm"]
                if b - a <= 0.01:
                    problems.append(f"{where}: {left['feature']} is left out over no length, {a}..{b}")
                    continue
                for k in range(1, 10):
                    s = a + (b - a) * k / 10
                    point = apply(m, [p[i] + s * d[i] for i in range(3)])
                    if inside_any(point, others):
                        problems.append(f"{where}: {left['feature']} is left out, "
                                        f"but Open CASCADE finds material on its axis at {point}")
                        break
    tally["touchers"] += len(touchers)
    for (where, plane, neighbour, face, gap), found in sorted(touchers.items()):
        if not found:
            problems.append(f"{where} seat {plane}: Open CASCADE finds no face #{face} of {list(neighbour)} "
                            f"opposed to it {gap} mm away, at any occurrence")
    return problems


def socket_main(argv):
    import argparse

    parser = argparse.ArgumentParser(prog="verify-occt.py socket")
    parser.add_argument("directory", metavar="DIR", help="a generated corpus, or the fixtures with --expect")
    parser.add_argument("--expect", metavar="MANIFEST",
                        help="the fixtures' manifest: its pinned socket targets, and its `socket_oracle` tallies")
    args = parser.parse_args(argv)
    directory = Path(args.directory)
    binary = Path(os.environ.get("FITMENT", Path(__file__).resolve().parent.parent / "target/release/fitment"))
    if not binary.is_file():
        sys.exit(f"no fitment at {binary}: run cargo build --release -p fitment, or set FITMENT")
    pinned = None
    if args.expect is not None:
        # Fixtures: each pinned target, as a case of its own file.
        expect = json.loads(Path(args.expect).read_text(encoding="utf-8"))
        pinned = expect.get("socket_oracle", {})
        cases = [{"id": f["path"], "file": f["path"], "kind": "assembly", "fixture": True,
                  "expect": {"sockets": f["sockets"]}}
                 for f in expect["fixtures"] if "sockets" in f]
    else:
        cases = json.loads((directory / "manifest.json").read_text(encoding="utf-8"))["cases"]
    tally = {"cases": 0, "others": 0, "bore spans": 0, "shaft spans": 0, "left out holes": 0, "seats": 0,
             "conical seats": 0, "grips": 0, "seat pairs": 0, "faces off every seat": 0, "touching faces": 0, "max extras": 0, "touchers": 0, "grip holes": 0,
             "partial cylinders": 0}
    problems = []
    for case in cases:
        if case.get("kind") != "assembly" or "sockets" not in case["expect"]:
            continue
        tally["cases"] += 1
        found = socket_problems(directory, case, binary, tally)
        problems += found
        print(f"{'ok  ' if not found else 'FAIL'} {case['id']}")
    for problem in problems:
        print(problem, file=sys.stderr)
    if pinned is not None:
        # Counts of sampled distances near τ can tip by one between
        # platforms' floating point: checked, printed, not pinned.
        sampled = {"touching faces"}
        firm = {k: v for k, v in tally.items() if k not in sampled}
        pinned = {k: v for k, v in pinned.items() if k not in sampled}
        if pinned != firm:
            changed = {k: (pinned.get(k), firm.get(k)) for k in sorted(set(pinned) | set(firm))
                       if pinned.get(k) != firm.get(k)}
            problems.append(f"the tallies differ from the pinned `socket_oracle` ones (pinned, now): {changed}")
            print(problems[-1], file=sys.stderr)
    elif any(v and "unbound" in k for k, v in tally.items()):
        problems.append("faces did not bind to #ids, so checks were skipped: "
                        + ", ".join(f"{v} {k}" for k, v in tally.items() if "unbound" in k))
        print(problems[-1], file=sys.stderr)
    elif any(v == 0 for v in tally.values()):
        problems.append("a socket check examined nothing: " + ", ".join(f"{v} {k}" for k, v in tally.items()))
        print(problems[-1], file=sys.stderr)
    print(", ".join(f"{v} {k}" for k, v in tally.items()) + f"; {len(problems)} disagreements with Open CASCADE")
    sys.exit(1 if problems else 0)


def main():
    if len(sys.argv) >= 3 and sys.argv[1] == "socket":
        quiet()
        socket_main(sys.argv[2:])
    if len(sys.argv) >= 3 and sys.argv[1] == "features":
        quiet()
        features_main(sys.argv[2:])
    if len(sys.argv) != 3 or sys.argv[1] != "manifest":
        sys.exit(__doc__)
    directory = Path(sys.argv[2])
    manifest = json.loads((directory / "manifest.json").read_text(encoding="utf-8"))
    quiet()
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
