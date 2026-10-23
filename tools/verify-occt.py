#!/usr/bin/env -S uv run --locked --script
# /// script
# requires-python = ">=3.11,<3.14"
# dependencies = ["cadquery-ocp-novtk==8.0.1.1.0"]
# ///
"""Check fitment's test parts, and fitment itself, with Open CASCADE.

    tools/verify-occt.py manifest DIR
    tools/verify-occt.py features [--expect MANIFEST] PATH...
    tools/verify-occt.py socket DIR
    tools/verify-occt.py mutations DIR
    tools/verify-occt.py match [--expect MANIFEST] DIR
    tools/verify-occt.py fit DIR DOCS

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

For a match case (docs/research/m4-match.md, KO1), the candidate placed
by the construction (each occurrence's transform after the case's
placement) into its socket's source assembly, against every other leaf:
no common volume where the original has none, an interference changed by
no more than a radius change of tol/2 over its interface, and no contact
lost beyond tol times the original contact's boundary (planes exactly,
by face booleans within one tolerance band; cylinders and cones sampled).
Volumes gained and lost, and contact lost, are each measured as regions,
so a gain never hides a loss; the target's other occurrences are
neighbours too, as the candidate replacing them. `fits` and `fn_rule`
must fit, `does_not_fit` and `fits_functional` must not, `collides` may
depart by common volume alone, outside every region of M3's expected
sockets (each bore's core, each shaft's hole and the near band beyond,
each seat's faces swept through the near band); `edge`, `refused`,
component candidates and grip-only thickness changes (M4-8, M7's) are not
judged.

`match` runs the built fitment on every match case's file candidate,
under both policies, and checks each document (KO2 to KO6): every pass
fits as KO1 says at the placement fitment states, at every occurrence of
every socket (under functional, the ring a larger clearance bore takes
from its seats excused); every placement is rigid; every passed check's
candidate feature, bound by #id and placed, lies on the requirement's
plane or axis line within the tolerance; and each mutation of a document
(a reject turned interface_match, a placement moved 0.01 mm across an
axis, turned 0.02 degrees, or turned 180 degrees about a hole's axis)
meets a disagreement somewhere in the corpus. A `collides` pass may
depart by common volume alone, none of it inside a requirement's region.
Every failed check of every placed candidate fails something, with a code
its failed measures allow; each failed measure Open CASCADE can measure
alone agrees with fitment's within 1e-6 mm (1e-9 rad) and breaks its
bound: an axis's offset, a span's diameters, margin and wall from the
named feature's coaxial pieces as fitment's run takes them, an end's
opening, a seat's plane over its contacts (with no feature named, the
nearest facing plane), a cone's angle, a contact face left uncovered; a
core intrusion's distance is a sound bound, never past the face's true
one. Where fitment found no piece (M2's reading: an annular bottom, a
zone not taken), and zone profiles, are tallied, not re-measured. The
other mutations: a failed measure's found changed by 0.1, a passed check
marked failed, a passed measure marked failed with its code, failure
codes swapped between two checks, a check dropped, the policy's name
changed. A bore's core reaches on past each end it requires open as far
as the candidate's material encloses it, so a wall continued past the
original's rim and into a neighbour is a defect, not a collision. KO2:
every bad case but a mirror image, at its construction's placement,
departs physically only with the neighbours its defect's requirements
name as sources.

`fit` (docs/research/m7-fit.md, FO1 to FO3, VT72) checks the fitment.match
v2 documents the fit corpus suite writes (crates/fitment/tests/fit_corpus.rs,
into DOCS with an index.json) against the corpus in DIR, with zones the oracle
builds itself from fitment's socket document (a seat's cited neighbour face
swept d either side, a tight span's annulus, a conical seat's shell), never
fitment's: every `Match` has no common volume with any other leaf or copy
outside them at depth t, and none at 8 turns where a free turn was excused
(FO1); every `interferes` re-measured at the pair's stated epsilon agrees
with fitment's residual (FO2), and every `collides` case collides at its
construction's placement, at every turn of a free axisymmetric candidate and
some of another's (FO2, FK3); each placement's freedom is the one its case
states (FK3); and three mutations of the documents (an interferes turned
verified, a collides case verified, a freedom dropped) each meet a
disagreement (FO3). In manifest and match mode, a source whose original
interferes by construction (FK4's `original_interferes`) is not judged by
KO1 or KO3, and an FK2 `fits` case that states its fit has gained volume on a
seat's plane, within the tolerance, excused as FO1's zones excuse it.

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

import hashlib
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
    if case.get("kind") == "match":
        return match_physics(directory, case)
    if case.get("kind") == "hostile":
        # Bounds and time, judged by crates/fitment/tests/matching.rs; their physics would take hours here.
        return []
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


# ---------------------------------------------------------------- match physics (KO1)

TOL = 0.01  # mm: the default policy's linear tolerance, which every case's truth is stated under
VOLUME = 1e-6  # mm³: a common volume no larger is none
SAMPLES = 16  # a curved face's contact, sampled on an n x n grid of its parameters
NEAR = 1.0  # mm: fitment's NEAR_MM, the band of a seat's region about its plane

# Classes whose truth KO1 cannot judge at the construction's placement, each for its reason.
UNJUDGED = {
    # M4-8: a grip-only change of thickness fits once the neighbours reseat, which only M7's body-fit verifier
    # can check; at the construction's placement it overlaps or leaves a neighbour.
    "b_thickness": "the neighbours reseat (M4-8): M7's",
}

# How many cases KO1 judged, by truth: printed, and none at all is a disagreement.
KO1_TALLY = {}


class BooleanFailed(Exception):
    """An Open CASCADE boolean that did not finish: a disagreement of its own, never a volume of zero."""


def compose(a, b):
    """The 3x4 transform a after b."""
    return [[sum(a[i][k] * b[k][j] for k in range(3)) + (a[i][3] if j == 3 else 0) for j in range(4)]
            for i in range(3)]


def moved(shape, m):
    from OCP.BRepBuilderAPI import BRepBuilderAPI_Transform
    from OCP.gp import gp_Trsf
    t = gp_Trsf()
    t.SetValues(*[m[i][j] for i in range(3) for j in range(4)])
    return BRepBuilderAPI_Transform(shape, t, True).Shape()


def translated(shape, v):
    return moved(shape, [[1.0 * (i == j) for j in range(3)] + [v[i]] for i in range(3)])


def one_solid(path):
    """The one solid a candidate file holds, in millimetres."""
    reader = STEPControl_Reader()
    if reader.ReadFile(str(path)) != IFSelect_RetDone:
        return None
    reader.TransferRoots()
    solids = [as_solid(s) for i in range(1, reader.NbShapes() + 1) for s in explore(reader.Shape(i), TopAbs_SOLID)]
    return solids[0] if len(solids) == 1 else None


def volume_of(shape):
    if shape is None:
        return 0.0
    props = GProp_GProps()
    BRepGProp.VolumeProperties_s(shape, props)
    return abs(props.Mass())


def area_of(shape):
    props = GProp_GProps()
    BRepGProp.SurfaceProperties_s(shape, props)
    return props.Mass()


def length_of(shape):
    props = GProp_GProps()
    BRepGProp.LinearProperties_s(shape, props)
    return props.Mass()


def common(a, b):
    from OCP.BRepAlgoAPI import BRepAlgoAPI_Common
    op = BRepAlgoAPI_Common(a, b)
    if not op.IsDone():
        raise BooleanFailed("a common")
    return op.Shape()


def without(a, b):
    """a less b; a or b None is nothing."""
    from OCP.BRepAlgoAPI import BRepAlgoAPI_Cut
    if a is None or b is None:
        return a
    op = BRepAlgoAPI_Cut(a, b)
    if not op.IsDone():
        raise BooleanFailed("a cut")
    return op.Shape()


def apart(a, b, gap):
    """True if the boxes of a and b are more than `gap` apart."""
    from OCP.Bnd import Bnd_Box
    from OCP.BRepBndLib import BRepBndLib
    boxes = []
    for shape in (a, b):
        box = Bnd_Box()
        BRepBndLib.Add_s(shape, box)
        box.Enlarge(gap)
        boxes.append(box)
    return boxes[0].IsOut(boxes[1])


def planar_contact(x, n, band):
    """x's contact with n's planar faces, as pieces of n's faces by index: each face of x opposed to a face g of n
    and at most `band` from g's plane, moved onto g's plane and intersected with g exactly."""
    mine = [(f, plane_facts(f)) for f in (as_face(e) for e in explore(x, TopAbs_FACE))]
    mine = [(f, facts) for f, facts in mine if facts is not None]
    out = {}
    for k, g in enumerate(as_face(e) for e in explore(n, TopAbs_FACE)):
        theirs = plane_facts(g)
        if theirs is None or apart(g, x, band):
            continue
        normal, point, _ = theirs
        pieces = []
        for f, (fn, fp, _) in mine:
            if dot(fn, normal) > -1 + 1e-9:
                continue
            gap = dot(sub(fp, point), normal)
            if not -band <= gap <= band or apart(f, g, band):
                continue
            shared = common(g, translated(f, [-gap * c for c in normal]))
            if area_of(shared) > 1e-9:
                pieces.append(shared)
        if pieces:
            out[k] = pieces
    return out


def curved_touch(x_shapes, n, band):
    """For each cylindrical or conical face of n near some shape of `x_shapes`: (area, boundary length, and per
    shape the share of its sampled points that shape lies against: the point `band`/2 inside n's surface, on the
    shape's side, is inside the shape)."""
    from OCP.BRepClass import BRepClass_FaceClassifier
    from OCP.BRepGProp import BRepGProp_Face
    from OCP.gp import gp_Pnt2d, gp_Vec
    classifiers = [BRepClass3d_SolidClassifier(x) for x in x_shapes]
    out = []
    for g in (as_face(e) for e in explore(n, TopAbs_FACE)):
        if BRepAdaptor_Surface(g).GetType() not in (Kind.GeomAbs_Cylinder, Kind.GeomAbs_Cone):
            continue
        if all(apart(g, x, band) for x in x_shapes):
            continue
        normals = BRepGProp_Face(g)
        u0, u1, v0, v1 = BRepTools.UVBounds_s(g)
        touched = [[] for _ in x_shapes]
        for i in range(SAMPLES):
            for j in range(SAMPLES):
                u, v = u0 + (i + 0.5) / SAMPLES * (u1 - u0), v0 + (j + 0.5) / SAMPLES * (v1 - v0)
                if BRepClass_FaceClassifier(g, gp_Pnt2d(u, v), 1e-7).State() != TopAbs_IN:
                    continue
                at, normal = gp_Pnt(), gp_Vec()
                normals.Normal(u, v, at, normal)
                if normal.Magnitude() == 0:
                    continue
                normal.Normalize()
                probe = [c + band / 2 * d for c, d in zip(xyz(at), (normal.X(), normal.Y(), normal.Z()))]
                for k, classifier in enumerate(classifiers):
                    classifier.Perform(gp_Pnt(*probe), 1e-7)
                    touched[k].append(classifier.State() == TopAbs_IN)
        if touched[0]:
            out.append((area_of(g), length_of(g), touched))
    return out


def contact_lost(t, c, n_t, n_c, band):
    """The contact t makes with n_t that c no longer makes with n_c, and the boundary length of t's contact.
    One neighbour (n_t is n_c): region by region, planes exactly and curved faces sample by sample, so a contact
    gained elsewhere never hides one lost. Two (a copy of the target, replaced too): by totals."""
    pt, pc = planar_contact(t, n_t, band), planar_contact(c, n_c, band)
    boundary = sum(length_of(p) for pieces in pt.values() for p in pieces)
    if n_t is n_c:
        lost = 0.0
        for k, pieces in pt.items():
            for p in pieces:
                kept = sum(area_of(common(p, q)) for q in pc.get(k, []))
                lost += max(area_of(p) - kept, 0.0)
        for area, length, (by_t, by_c) in curved_touch([t, c], n_t, band):
            share = sum(1 for a, b in zip(by_t, by_c) if a and not b) / len(by_t)
            lost += share * area
            boundary += sum(by_t) / len(by_t) * length
        return lost, boundary
    total = lambda pieces: sum(area_of(p) for ps in pieces.values() for p in ps)  # noqa: E731
    lost = total(pt) - total(pc)
    for shape, n in ((t, n_t), (c, n_c)):
        for area, length, (by,) in curved_touch([shape], n, band):
            share = sum(by) / len(by)
            lost += share * area if shape is t else -share * area
            if shape is t:
                boundary += share * length
    return max(lost, 0.0), boundary


def physics(target, candidate, neighbours, name):
    """KO1's judgment of one placed candidate against the original it replaces, neighbour by neighbour (each
    `(label, as the original meets it, as the candidate meets it)`): no common volume the original's lacks, nor
    any of the original's lost, beyond the engagement a radius change of tol/2 allows over an interference's
    interface (half its common's surface; none without one); and no contact lost beyond tol times the original
    contact's boundary. Returns the departures as (kind, text, neighbour as the candidate meets it, gained
    common)."""
    out = []
    for label, n_t, n_c in neighbours:
        near_t, near_c = not apart(target, n_t, TOL), not apart(candidate, n_c, TOL)
        if not near_t and not near_c:
            continue
        tn = common(target, n_t) if near_t else None
        cn = common(candidate, n_c) if near_c else None
        vt = volume_of(tn)
        allowed = (TOL / 2 * area_of(tn) / 2 if vt > VOLUME else 0.0) + VOLUME
        gained_shape = without(cn, tn) if vt > VOLUME else cn
        gained, lost = volume_of(gained_shape), volume_of(without(tn, cn) if cn is not None else tn)
        if gained > allowed:
            out.append(("volume", f"{name}: {gained:.6g} mm³ more common with {label} than the original's "
                                  f"(allowed {allowed:.6g})", n_c, gained_shape))
        if lost > allowed:
            out.append(("interference", f"{name}: {lost:.6g} mm³ of the original's common with {label} lost "
                                        f"(allowed {allowed:.6g})", n_c, None))
        area, boundary = contact_lost(target, candidate, n_t, n_c, TOL)
        if area > TOL * boundary + 1e-6:
            out.append(("contact", f"{name}: {area:.6g} mm² of the original's contact with {label} lost "
                                   f"(allowed {TOL * boundary:.6g})", n_c, None))
    return out


def neighbours_at(leaves, part, k, originals, candidates):
    """The neighbours of the target's occurrence k: every other leaf as itself, and every other occurrence of the
    target as the original and as the candidate that replaces it too."""
    out = [(f"{product} #{j}", shape, shape) for j, (product, _, shape) in enumerate(leaves) if product != part]
    out += [(f"{part} copy {j}", originals[j], candidates[j]) for j in range(len(originals)) if j != k]
    return out


def region_spec_from_document(socket):
    """A socket document's socket as regions to place: each bore's core (its clear diameter, or with none its
    own, over each span, and the ends it requires open), each shaft's neighbour hole and the band beyond (its
    clear diameter, or its own, plus NEAR), each seat's plane."""
    out = []
    for requirement in socket["requirements"]:
        q = requirement["requires"]
        if q["kind"] in ("bore", "shaft"):
            own = "hole_diameter_mm" if q["kind"] == "bore" else "shaft_diameter_mm"
            spans = [(s_["from_mm"], s_["to_mm"], (s_.get("clear_diameter_mm") or s_[own]) / 2
                      + (NEAR if q["kind"] == "shaft" else 0.0)) for s_ in q.get("spans", [])]
            opened = q.get("open") or {}
            ends = (bool(opened.get("start")), bool(opened.get("finish"))) if q["kind"] == "bore" else (False, False)
            out.append({"axis": (q["axis"]["point_mm"], norm(q["axis"]["direction"])), "spans": spans,
                        "open": ends})
        elif q["kind"] == "seat":
            out.append({"plane": (q["normal"], q["offset_mm"])})
    return out


def region_spec_from_expectation(socket):
    """The same regions from M3's expected socket (gen-corpus.py's e_socket), stated from the construction."""
    out = []
    for q in socket.get("requirements", []):
        point, d = q["axis_point"], norm(q["axis_direction"])
        spans = []
        for s_ in q["spans"]:
            own = (s_["clear"] or s_["diameter"]) / 2 + (NEAR if q["kind"] == "shaft" else 0.0)
            spans.append((dot(sub(s_["from"], point), d), dot(sub(s_["to"], point), d), own))
        stations = [dot(sub(x, point), d) for x in q.get("open", [])]
        ends = (False, False)
        if spans and q["kind"] == "bore":
            low = min(min(a, b) for a, b, _ in spans)
            high = max(max(a, b) for a, b, _ in spans)
            ends = (any(abs(x - low) <= 1e-6 for x in stations), any(abs(x - high) <= 1e-6 for x in stations))
        out.append({"axis": (point, d), "spans": spans, "open": ends})
    for q in socket.get("seats", []):
        out.append({"plane": (q["normal"], dot(q["point"], q["normal"]))})
    return out


def wall_past(candidate, point, d, station, radius, outward):
    """How far the candidate's hole about a core runs on past `station` (outward +1 or -1): the last station, in
    steps of 0.05 mm up to 50 mm, at which the candidate's material encloses the core, within 2 NEAR of its radius
    at each of eight angles. A core required open reaches the candidate's own opening there (VR22, RS14); a leg
    or a bar past the opening encloses nothing, and the core stops at the first station it does not."""
    side = norm(cross(d, [1.0, 0.0, 0.0] if abs(d[0]) < 0.9 else [0.0, 1.0, 0.0]))
    other = cross(d, side)
    classifier = BRepClass3d_SolidClassifier(candidate)

    def walled(centre, a):
        for extra in (0.02, 0.25, 0.5, 1.0, 1.5, 2 * NEAR):
            probe = [centre[i] + (radius + extra) * (math.cos(a) * side[i] + math.sin(a) * other[i])
                     for i in range(3)]
            classifier.Perform(gp_Pnt(*probe), 1e-7)
            if classifier.State() == TopAbs_IN:
                return True
        return False

    # Probed inside each step, at an odd offset, so no probe lands on a face's plane at a round station.
    reach = station
    for step in range(1, 1001):
        at = station + outward * 0.05 * step
        centre = add(point, d, at - outward * 0.0137)
        if not all(walled(centre, 2 * math.pi * k / 8) for k in range(8)):
            return reach
        reach = at
    return reach


def regions(spec, at, original, candidate=None):
    """KO1's regions placed by `at`: cylinders along each axis over each span, a bore's core reaching on to each
    end it requires open as far as the placed candidate's wall about it runs, and the original's faces on each
    seat's plane swept through NEAR either side of it."""
    from OCP.BRepPrimAPI import BRepPrimAPI_MakeCylinder, BRepPrimAPI_MakePrism
    from OCP.gp import gp_Ax2, gp_Dir, gp_Vec
    out = []
    planes = [(f, plane_facts(f)) for f in (as_face(e) for e in explore(original, TopAbs_FACE))]
    planes = [(f, facts) for f, facts in planes if facts is not None]

    def cylinder(point, d, lo, hi, radius):
        base = apply_point(at, add(point, d, lo))
        return BRepPrimAPI_MakeCylinder(gp_Ax2(gp_Pnt(*base), gp_Dir(*turn(at, d))), radius, hi - lo).Shape()

    for item in spec:
        if "axis" in item:
            point, d = item["axis"]
            spans = [(min(a, b), max(a, b), radius) for a, b, radius in item["spans"]
                     if abs(b - a) > 0 and radius > 0]
            out += [cylinder(point, d, lo, hi, radius) for lo, hi, radius in spans]
            if candidate is None or not spans:
                continue
            for end, opened in zip((0, 1), item.get("open", (False, False))):
                if not opened:
                    continue
                lo, hi, radius = min(spans) if end == 0 else max(spans, key=lambda x: x[1])
                station, outward = (lo, -1) if end == 0 else (hi, 1)
                reach = wall_past(candidate, apply_point(at, point), turn(at, d), station, radius, outward)
                if abs(reach - station) > 1e-9:
                    out.append(cylinder(point, d, min(station, reach), max(station, reach), radius))
        else:
            normal, offset = item["plane"]
            n = turn(at, normal)
            o = dot(n, apply_point(at, [c * offset for c in normal]))
            for face, (fn, fp, _) in planes:
                if dot(fn, n) >= 1 - 1e-9 and abs(dot(fp, n) - o) <= 1e-6:
                    start = translated(face, [-NEAR * c for c in n])
                    out.append(BRepPrimAPI_MakePrism(start, gp_Vec(*[2 * NEAR * c for c in n])).Shape())
    return out


def inside_regions(gained, places):
    """True if any of a candidate's gained common volume lies in some region (RS9: a defect, not a collision)."""
    return any(volume_of(common(gained, region)) > VOLUME for region in places)


def collisions_problems(departures, places):
    """A `collides` candidate's departures: none but gained volume, and none of it inside a requirement's
    region."""
    if {d[0] for d in departures} != {"volume"}:
        return [d[1] for d in departures]
    return [f"{d[1]}: inside a requirement's region (RS9)" for d in departures if inside_regions(d[3], places)]


MANIFESTS = {}


def expected_sockets(directory, source, part):
    """M3's expected sockets of `part` in `source`, from the corpus manifest; [] for a source with none."""
    if directory not in MANIFESTS:
        MANIFESTS[directory] = {c["id"]: c for c in
                                json.loads((directory / "manifest.json").read_text(encoding="utf-8"))["cases"]}
    case = MANIFESTS[directory].get(source, {})
    return (case.get("expect", {}).get("sockets") or {}).get(part, [])


def fo_seat_excused(departures, spec, m):
    """KO1 for a `fits` case that states its fit (FK2): gained common volume with a neighbour that lies within
    the slab, d = TOL either side, of the neighbour's faces on a seat plane of M3's expected sockets (placed by
    m) is a contact within the tolerance, as FO1's zones (fo_zones) excuse it; any other departure stands."""
    out = []
    planes = [(turn(m, item["plane"][0]), item["plane"]) for item in spec if "plane" in item]
    for kind, text, neighbour, gained in departures:
        if kind == "volume":
            slabs = []
            for n, (normal, offset) in planes:
                o = dot(n, apply_point(m, [c * offset for c in normal]))
                for f in (as_face(e) for e in explore(neighbour, TopAbs_FACE)):
                    facts = plane_facts(f)
                    if facts and abs(abs(dot(facts[0], n)) - 1) <= 1e-9 and abs(dot(facts[1], n) - o) <= 1e-6:
                        slabs.append(fo_slab(f, TOL))
            rest = gained
            for slab in slabs:
                rest = without(rest, slab)
            if volume_of(rest) <= VOLUME:
                continue
        out.append((kind, text, neighbour, gained))
    return out


def match_physics(directory, case):
    """KO1: the candidate placed by the construction (the occurrence's transform after the case's placement)
    into its socket's source assembly, at every occurrence of the target, judged against the original; the
    physics must agree with the truth: `fits` and `fn_rule` fit, `does_not_fit` and `fits_functional` do not
    (strict), `collides` departs by common volume alone, outside every region of M3's expected sockets. `edge`
    hits a stated limit exactly, so physics cannot side with either label, `refused` inputs have no physics,
    component candidates are the original, and the classes of UNJUDGED wait for M7. `waived` (M6-38) is the
    user's acceptance of a defect a review removed, never physics' answer."""
    name, truth = case["id"], case["truth"]
    socket = case["socket"]
    if truth in ("edge", "refused", "waived") or "file" not in case["candidate"]:
        return []
    if socket.get("original_interferes"):
        # FK4: the original already interferes by construction, and KO1 judges a candidate against it.
        KO1_TALLY["not judged: an original that interferes (FK4)"] = \
            KO1_TALLY.get("not judged: an original that interferes (FK4)", 0) + 1
        return []
    if truth == "fn_rule" and case["class"] in UNJUDGED:
        print(f"skip {name}: {UNJUDGED[case['class']]}")
        return []
    leaves = xcaf_leaves(directory / f"{socket['case']}.stp")
    if leaves is None:
        return [f"{name}: Open CASCADE cannot read {socket['case']}"]
    targets = [(m, shape) for product, m, shape in leaves if product == socket["part"]]
    candidate = one_solid(directory / case["candidate"]["file"])
    if not targets or candidate is None:
        return [f"{name}: no target {socket['part']}, or no one candidate solid"]
    KO1_TALLY[truth] = KO1_TALLY.get(truth, 0) + 1
    originals = [shape for _, shape in targets]
    placed = [moved(candidate, compose(m, case["placement"])) for m, _ in targets]
    # A match-only source written from another (M7's `from`, FK2) has that one's expected sockets.
    spec = [r_ for s_ in expected_sockets(directory, socket.get("from", socket["case"]), socket["part"])
            for r_ in region_spec_from_expectation(s_)]
    stated_fit = (case.get("expect", {}).get("default") or {}).get("fit") if truth == "fits" else None
    departures, problems = [], []
    try:
        for k, (m, original) in enumerate(targets):
            found = physics(original, placed[k], neighbours_at(leaves, socket["part"], k, originals, placed),
                            f"{name} at occurrence {k}")
            if stated_fit is not None:
                found = fo_seat_excused(found, spec, m)
            if truth == "collides":
                if not spec:
                    return [f"{name}: labelled collides, and its source states no socket to bound the collision"]
                problems += collisions_problems(found, regions(spec, m, original, placed[k]))
            departures += found
    except BooleanFailed as failed:
        return [f"{name}: {failed} failed in Open CASCADE"]
    if truth in ("fits", "fn_rule") and departures:
        return [f"{name}: labelled {truth}, but {d[1]}" for d in departures]
    if truth in ("does_not_fit", "fits_functional") and not departures:
        return [f"{name}: labelled {truth}, but it fits the original's place at the construction's placement"]
    if truth == "collides":
        return [f"{name}: labelled collides, but {text}" for text in problems]
    return []


# ---------------------------------------------------------------- match mode (KO3, KO5)


def placement_rows(p):
    """A document's placement {rotation, translation_mm} as 3x4 rows."""
    return [list(p["rotation"][i]) + [p["translation_mm"][i]] for i in range(3)]


def rigid_problem(where, m):
    """KO5: a placement is a rotation and a shift: orthonormal within 1e-12, determinant +1."""
    r = [row[:3] for row in m]
    for i in range(3):
        for j in range(3):
            if abs(sum(r[i][k] * r[j][k] for k in range(3)) - (i == j)) > 1e-12:
                return f"{where}: the placement is not orthonormal"
    det = (r[0][0] * (r[1][1] * r[2][2] - r[1][2] * r[2][1]) - r[0][1] * (r[1][0] * r[2][2] - r[1][2] * r[2][0])
           + r[0][2] * (r[1][0] * r[2][1] - r[1][1] * r[2][0]))
    return None if abs(det - 1) <= 1e-12 else f"{where}: the placement's determinant is {det}"


def apply_point(m, p):
    return [sum(m[i][j] * p[j] for j in range(3)) + m[i][3] for i in range(3)]


def off_line(point, line_point, line_dir):
    off = sub(point, line_point)
    off = sub(off, [line_dir[i] * dot(off, line_dir) for i in range(3)])
    return math.sqrt(dot(off, off))


def plane_problem(where, name, face, m, normal, offset, linear, angular_rad):
    """A face's plane, placed, against a requirement's: within the angle, and within the tolerance of offset."""
    facts = plane_facts(face)
    if facts is None:
        return [f"{where}: {name} is no plane"]
    n = turn(m, facts[0])
    out = []
    if dot(n, normal) < math.cos(angular_rad) - 1e-12:
        out.append(f"{where}: {name}'s plane is turned from the requirement's")
    off = dot(apply_point(m, facts[1]), normal) - offset
    if abs(off) > linear:
        out.append(f"{where}: {name}'s plane lies {off:.6g} mm off the requirement's")
    return out


def cylinder_problem(where, name, face, m, q, linear, angular_rad):
    """A face's cylinder, placed, on a requirement's axis line: its axis within the angle, and both ends of its
    extent along the axis within the tolerance of the line."""
    surface = BRepAdaptor_Surface(face)
    if surface.GetType() != Kind.GeomAbs_Cylinder:
        return [f"{where}: {name} is no cylinder"]
    c = surface.Cylinder()
    axis = norm(xyz(c.Axis().Direction()))
    line_point, line_dir = q["axis"]["point_mm"], norm(q["axis"]["direction"])
    out = []
    if abs(dot(norm(turn(m, axis)), line_dir)) < math.cos(angular_rad) - 1e-12:
        out.append(f"{where}: {name}'s axis is turned from the requirement's")
    _, _, v0, v1 = BRepTools.UVBounds_s(face)
    for v in (v0, v1):
        at = apply_point(m, add(xyz(c.Location()), axis, v))
        if off_line(at, line_point, line_dir) > linear:
            out.append(f"{where}: {name}'s axis lies {off_line(at, line_point, line_dir):.6g} mm off the "
                       f"requirement's at one end")
    return out


def cone_problem(where, name, face, m, q, linear, angular_rad):
    """A face's cone, placed, on a conical seat: its apex within the tolerance, its axis and half angle within the
    angle."""
    surface = BRepAdaptor_Surface(face)
    if surface.GetType() != Kind.GeomAbs_Cone:
        return [f"{where}: {name} is no cone"]
    c = surface.Cone()
    out = []
    apex = apply_point(m, xyz(c.Apex()))
    gap = math.sqrt(dot(sub(apex, q["apex_mm"]), sub(apex, q["apex_mm"])))
    if gap > linear:
        out.append(f"{where}: {name}'s apex lies {gap:.6g} mm off the seat's")
    if abs(dot(norm(turn(m, xyz(c.Axis().Direction()))), norm(q["axis"]["direction"]))) < math.cos(angular_rad) - 1e-12:
        out.append(f"{where}: {name}'s axis is turned from the seat's")
    if abs(abs(c.SemiAngle()) - q["included_angle_rad"] / 2) > angular_rad:
        out.append(f"{where}: {name}'s half angle is not the seat's")
    return out


def feature_problems(where, requirement, feature, faces, m, linear, angular_rad, requirements, tally):
    """KO5: a passed check's candidate features, placed, on the requirement: a seat's plane, a bore's or shaft's
    cylinder on its axis line, a conical seat's cone, and a grip's two features each on its seat's plane or
    cone. A feature of another surface than the requirement's is a disagreement."""
    out = []
    q = requirement["requires"]
    names = feature.split("+")
    if q["kind"] == "grip":
        seats = {}
        for r_ in requirements.values():
            key = r_["requires"].get("plane") or r_["requires"].get("cone")
            if r_["requires"]["kind"] in ("seat", "conical_seat") and key:
                seats[key] = r_["requires"]
        # The features in the order the grip's id names its seats.
        order = requirement["id"].split(":", 1)[1].split("+")
        targets = [seats.get(s_) for s_ in order]
        if len(targets) != len(names) or None in targets:
            return [f"{where}: the grip's features do not pair with its seats"]
    else:
        targets = [q] * len(names)
    for name, target in zip(names, targets):
        face = faces.get(int(name.rsplit("-", 1)[1]))
        if face is None:
            out.append(f"{where}: {name} names no face of the candidate")
            continue
        tally["features"] += 1
        if target["kind"] == "seat":
            out += plane_problem(where, name, face, m, target["normal"], target["offset_mm"], linear, angular_rad)
        elif target["kind"] in ("bore", "shaft"):
            out += cylinder_problem(where, name, face, m, target, linear, angular_rad)
        elif target["kind"] == "conical_seat":
            out += cone_problem(where, name, face, m, target, linear, angular_rad)
        else:
            out.append(f"{where}: a {target['kind']} feature the oracle cannot place")
    return out


def holes_on(candidate, axis_point, axis_dir):
    """The placed candidate's holes on an axis line: (station from, station to, radius) of each concave cylinder
    whose axis lies within TOL of it, from Open CASCADE alone."""
    out = []
    for face in (as_face(e) for e in explore(candidate, TopAbs_FACE)):
        r = revolution_of(face)
        if r is None or r[2] is None or not r[3]:
            continue
        if abs(dot(r[1], axis_dir)) < 1 - 1e-9 or off_line(r[0], axis_point, axis_dir) > TOL:
            continue
        a, b = extent_on(face, axis_point, axis_dir)
        out.append((a, b, r[2]))
    return out


def ring_excused(original, candidate, socket, at):
    """D6 functional: the original with each clearance span of each bore widened to the candidate's own hole
    over that span (the least radius of its coaxial holes overlapping the span by more than τ, as Open CASCADE
    measures them), so the ring a larger clearance hole takes from its seats is excused (KO3). Pinned and
    interference spans are never widened: functional excuses no lost engagement; nor is a span widened to a
    counterbore beside it."""
    from OCP.BRepPrimAPI import BRepPrimAPI_MakeCylinder
    from OCP.gp import gp_Ax2, gp_Dir
    shape = original
    for requirement in socket["requirements"]:
        q = requirement["requires"]
        if q["kind"] != "bore":
            continue
        d = norm(turn(at, q["axis"]["direction"]))
        point = apply_point(at, q["axis"]["point_mm"])
        holes = holes_on(candidate, point, d)
        for span in q.get("spans", []):
            if span["fit"] != "clearance" or span["pinned"]:
                continue
            lo, hi = min(span["from_mm"], span["to_mm"]), max(span["from_mm"], span["to_mm"])
            over = [r for a, b, r in holes if min(b, hi) - max(a, lo) > RUN_TOUCH]
            if not over or min(over) <= span["hole_diameter_mm"] / 2:
                continue
            base = add(point, d, lo)
            tool = BRepPrimAPI_MakeCylinder(gp_Ax2(gp_Pnt(*base), gp_Dir(*d)), min(over), hi - lo).Shape()
            shape = without(shape, tool)
    return shape


def rotation_about(point, axis, degrees):
    """The 3x4 turn by `degrees` about the line through `point` along `axis`."""
    k, th = norm(axis), math.radians(degrees)
    c, s_ = math.cos(th), math.sin(th)
    skew = [[0.0, -k[2], k[1]], [k[2], 0.0, -k[0]], [-k[1], k[0], 0.0]]
    r_ = [[c * (i == j) + s_ * skew[i][j] + (1 - c) * k[i] * k[j] for j in range(3)] for i in range(3)]
    shift = sub(point, [sum(r_[i][j] * point[j] for j in range(3)) for i in range(3)])
    return [r_[i] + [shift[i]] for i in range(3)]


def first_axis(socket, result):
    """The axis line of the first bore or shaft requirement of a socket result's socket, in the target's frame."""
    sockets = {s_["id"]: s_ for s_ in socket["extracted"]["sockets"]}
    for r_ in sockets[result["socket"]]["requirements"]:
        if r_["requires"]["kind"] in ("bore", "shaft"):
            return r_["requires"]["axis"]["point_mm"], r_["requires"]["axis"]["direction"]
    return None


def with_placements(doc, socket, change, rejected=False):
    """A copy of `doc` whose passes' (or, with `rejected`, placed rejects') placements are changed by `change`,
    those rejects turned into interface matches with every check passed; None if nothing changed (KO6's placement
    mutations)."""
    import copy
    out = copy.deepcopy(doc)
    changed = False
    for entry in out["candidates"]:
        if (entry["verdict"] == "reject") != rejected:
            continue
        if rejected and (not entry["sockets"] or any(r_.get("placement") is None for r_ in entry["sockets"])):
            continue
        for result in entry["sockets"]:
            m = change(placement_rows(result["placement"]), first_axis(socket, result))
            if m is None:
                continue
            result["placement"]["rotation"] = [row[:3] for row in m]
            result["placement"]["translation_mm"] = [row[3] for row in m]
            changed = True
        if rejected:
            entry["verdict"] = "interface_match"
            for result in entry["sockets"]:
                for check in result["checks"]:
                    check["result"] = "passed"
            changed = True
    return out if changed else None


def across(m, axis):
    """Moved 0.0101 mm square to the first axis (or along x, with none)."""
    d = norm(axis[1]) if axis else [0.0, 0.0, 1.0]
    side = norm(cross(d, [1.0, 0.0, 0.0] if abs(d[0]) < 0.9 else [0.0, 1.0, 0.0]))
    return [row[:3] + [row[3] + 0.0101 * side[i]] for i, row in enumerate(m)]


def turned(m, axis, degrees=0.02):
    """Turned 0.02 degrees about the first axis's point, square to it."""
    if axis is None:
        return None
    d = norm(axis[1])
    side = norm(cross(d, [1.0, 0.0, 0.0] if abs(d[0]) < 0.9 else [0.0, 1.0, 0.0]))
    return compose(rotation_about(axis[0], side, degrees), m)


def half_turn(m, axis):
    """A half turn about the first hole's own axis."""
    return None if axis is None else compose(rotation_about(axis[0], axis[1], 180.0), m)


# KO6: each mutation of fitment's match documents must meet a disagreement somewhere in the corpus.
MATCH_MUTATIONS = [
    ("a reject turned interface_match", lambda doc, socket: with_placements(doc, socket, lambda m, a: m, True)),
    ("a placement moved 0.01 mm across an axis", lambda doc, socket: with_placements(doc, socket, across)),
    ("a placement turned 0.02 degrees", lambda doc, socket: with_placements(doc, socket, turned)),
    ("a placement turned 180 degrees about a hole's axis", lambda doc, socket: with_placements(doc, socket, half_turn)),
    ("a failed check's found changed by 0.1", lambda doc, socket: found_changed(doc)),
    ("a passed check marked failed", lambda doc, socket: passed_failed(doc)),
    ("a passed measure marked failed, with its code", lambda doc, socket: measure_failed(doc)),
    ("failure codes swapped between two checks", lambda doc, socket: codes_swapped(doc)),
    ("a check dropped", lambda doc, socket: check_dropped(doc)),
    ("a policy name changed", lambda doc, socket: dict(doc, policy=dict(doc["policy"], name="cli"))),
]


def each_check(doc):
    for entry in doc["candidates"]:
        for result in entry["sockets"]:
            if result.get("placement") is None:
                continue
            for check in result["checks"]:
                yield entry, result, check


def found_changed(doc):
    import copy
    out = copy.deepcopy(doc)
    for _, _, check in each_check(out):
        for measure in check.get("measures") or []:
            if (measure.get("passed") is False and measure["quantity"] in ("axis_offset_mm", "diameter_mm",
                                                                            "plane_offset_mm", "cone_angle_rad")
                    and number(measure.get("found")) is not None):
                measure["found"] += 0.1
                return out
    return None


def passed_failed(doc):
    import copy
    out = copy.deepcopy(doc)
    for _, _, check in each_check(out):
        if check["result"] == "passed" and check.get("measures"):
            check["result"] = "failed"
            return out
    return None


def measure_failed(doc):
    """A passed check's re-measurable measure (a pinned diameter, an axis offset) marked failed, the check failed
    with that measure's code: a lie only re-measurement can catch."""
    import copy
    out = copy.deepcopy(doc)
    for _, _, check in each_check(out):
        if check["result"] != "passed":
            continue
        for measure in check.get("measures") or []:
            if measure["quantity"] in ("axis_offset_mm", "diameter_mm") and measure.get("passed") is True:
                measure["passed"] = False
                check["result"] = "failed"
                code = "axis_offset" if measure["quantity"] == "axis_offset_mm" else "diameter_out_of_limits:max"
                check["failures"] = [{"code": code, "at": measure.get("at", {}), "detail": "mutated"}]
                return out
    return None


def codes_swapped(doc):
    import copy
    out = copy.deepcopy(doc)
    failed = [check for _, _, check in each_check(out) if check["result"] == "failed" and check.get("failures")
              and check.get("measures")]
    for a in failed:
        for b in failed:
            codes_a = {f["code"] for f in a["failures"]}
            codes_b = {f["code"] for f in b["failures"]}
            if not codes_a & codes_b:
                a["failures"], b["failures"] = b["failures"], a["failures"]
                return out
    return None


def check_dropped(doc):
    import copy
    out = copy.deepcopy(doc)
    for entry in out["candidates"]:
        for result in entry["sockets"]:
            if result["checks"]:
                result["checks"].pop()
                return out
    return None


def run_fitment(binary, args, codes, cwd=None):
    """fitment's JSON on stdout, or the problem: an exit code outside `codes`, with its stderr."""
    run = subprocess.run([str(binary), *args], capture_output=True, cwd=cwd)
    if run.returncode not in codes:
        return None, f"fitment {' '.join(args[:2])}: exit {run.returncode}: {run.stderr.decode(errors='replace')}"
    try:
        return json.loads(run.stdout), None
    except json.JSONDecodeError as error:
        return None, f"fitment {' '.join(args[:2])}: no JSON ({error}): {run.stderr.decode(errors='replace')}"


def write_part(shape, path):
    """A component's solid, in its own frame, written alone (Open CASCADE's writer, no pcurves, millimetres)."""
    from OCP.Interface import Interface_Static
    from OCP.STEPControl import STEPControl_AsIs, STEPControl_Writer
    writer = STEPControl_Writer()
    Interface_Static.SetIVal_s("write.surfacecurve.mode", 0)
    Interface_Static.SetIVal_s("write.step.assembly", 0)
    Interface_Static.SetCVal_s("write.step.unit", "MM")
    if writer.Transfer(shape, STEPControl_AsIs) != IFSelect_RetDone or writer.Write(str(path)) != IFSelect_RetDone:
        raise OSError(f"Open CASCADE cannot write {path}")


def inverse_of(o):
    r_ = [row[:3] for row in o]
    return [[r_[j][i] for j in range(3)] + [-sum(r_[j][i] * o[j][3] for j in range(3))] for i in range(3)]


def fixture_groups(directory, expect, scratch):
    """KO8: AS1's cross-export runs (KC12) as match groups: for each part and each export whose socket of it the
    fixtures' manifest pins, every export's component of that part written alone into `scratch`, as candidates.
    Yields (export path, part name, candidate files, cases)."""
    for k, _ in enumerate(AS1_EXPORTS[0][2]):
        files = []
        for name, path, parts in AS1_EXPORTS:
            leaves = xcaf_leaves(directory / path)
            mine = [(m, shape) for product, m, shape in leaves if product == parts[k]]
            if not mine:
                continue
            out = scratch / f"as1-{name}-{k}.stp"
            write_part(moved(mine[0][1], inverse_of(mine[0][0])), out)
            files.append(out)
        cases = [{"candidate": {"file": str(f)}, "truth": "fits"} for f in files]
        for _, path, parts in AS1_EXPORTS:
            yield path, parts[k], files, cases


AS1_EXPORTS = [
    ("ac", "steptools/as1-ac-214.stp", ["PLATE", "L-BRACKET", "BOLT", "NUT", "ROD"]),
    ("md", "steptools/as1-md-214.stp", ["PLATE", "LBRACKET", "BOLT", "NUT", "ROD"]),
    ("ug", "steptools/as1-ug-214.stp", ["plate", "l_bracket", "bolt", "nut", "rod"]),
    ("pe", "nist-edm/as1_pe.stp", ["PLATE", "L-BRACKET", "BOLT", "NUT", "ROD"]),
    ("ec", "steptools/as1-ec-214.stp", ["as1 1.2", "as1 1.1.1", "as1 1.1.2.2", "as1 1.1.2.1", "as1 1.4.1"]),
    ("tc", "steptools/as1-tc-214.stp", ["plate", "l-bracket", "bolt", "nut", "rod"]),
]


def match_main(argv):
    """`match [--expect MANIFEST] DIR`: every match case's file candidate judged by the built fitment against its
    source's socket, under both policies M4 offers, each document then checked by Open CASCADE: every pass fits
    physically at the placement fitment states, at every occurrence (KO3); every failure is re-measured (KO4); its
    placement is rigid with each passed check's feature on the requirement (KO5); and each mutation of a document
    meets a disagreement somewhere (KO6). A reviewed case's group (M6-38) is judged against its socket reviewed by
    `fitment review`, and a waived pass gets every check but KO3's physics. With --expect, the fixtures: AS1's
    parts across its exports (KC12), each export's component written alone, with the tallies pinned under
    `match_oracle` (KO8).

    What it cannot check (KO7): threads and press fits beyond their geometry; materials and other properties; what
    a review means; the shortlist (the Rust harness cross-checks it, KH9); whether any placement exists where
    fitment found none (it knows the construction's placement, KO1, and checks the one fitment chose); policies
    beyond D6's ring; what an unverifiable requirement means; and what both kernels would misread alike, both
    being Open CASCADE. Body fit is judged as physics here, not reported as a verdict, until M7."""
    import argparse
    import tempfile
    parser = argparse.ArgumentParser(prog="verify-occt.py match")
    parser.add_argument("directory", metavar="DIR")
    parser.add_argument("--expect", metavar="MANIFEST")
    args = parser.parse_args(argv)
    directory = Path(args.directory)
    binary = Path(os.environ.get("FITMENT", Path(__file__).resolve().parent.parent / "target/release/fitment"))
    if not binary.is_file():
        sys.exit(f"no fitment at {binary}: run cargo build --release -p fitment, or set FITMENT")
    tally = {"documents": 0, "passes": 0, "occurrences": 0, "features": 0, "collisions excused": 0,
             "re-measured": 0}
    problems, met = [], set()
    with tempfile.TemporaryDirectory(prefix="fitment-match-oracle-") as scratch:
        if args.expect is None:
            cases = json.loads((directory / "manifest.json").read_text(encoding="utf-8"))["cases"]
            groups = {}
            for case in cases:
                if case.get("kind") != "match" or "file" not in case["candidate"] or case["truth"] == "refused":
                    continue
                # By socket as judged: a reviewed case's against its review (M6-38).
                review = json.dumps(case["socket"].get("review") or [], sort_keys=True)
                groups.setdefault((case["socket"]["case"], case["socket"]["part"], review), []).append(case)
            runs = [(f"{source}.stp", part, sorted({c["candidate"]["file"] for c in group}), group)
                    for (source, part, _), group in sorted(groups.items())]
        else:
            runs = list(fixture_groups(directory, None, Path(scratch)))
        for source, part, files, group in runs:
            before = len(problems)
            match_group(binary, directory, source, part, [str(f) for f in files], group, Path(scratch), tally,
                        problems, met)
            reviewed = " reviewed" if group and group[0].get("socket", {}).get("review") else ""
            print(f"{'ok  ' if len(problems) == before else 'FAIL'} {source} {part}{reviewed}")
    if args.expect is None:
        unmet = [name for name, _ in MATCH_MUTATIONS if name not in met]
        if unmet:
            problems.append(f"mutations no check disagreed with anywhere (KO6): {unmet}")
        if any(v == 0 for k, v in tally.items() if k != "collisions excused" and not k.startswith("not ")):
            problems.append("a match check examined nothing: " + ", ".join(f"{v} {k}" for k, v in tally.items()))
    else:
        pinned = json.loads(Path(args.expect).read_text(encoding="utf-8")).get("match_oracle")
        if pinned != tally:
            changed = {k: (pinned.get(k) if pinned else None, tally.get(k))
                       for k in sorted(set(pinned or {}) | set(tally)) if (pinned or {}).get(k) != tally.get(k)}
            problems.append(f"the tallies differ from the pinned `match_oracle` ones (pinned, now): {changed}")
    for problem in problems:
        print(problem, file=sys.stderr)
    print(", ".join(f"{v} {k}" for k, v in tally.items()) + f"; {len(problems)} disagreements with Open CASCADE")
    sys.exit(1 if problems else 0)


def match_group(binary, directory, source, part, files, group, scratch, tally, problems, met):
    """One socket: written by fitment, its candidates judged under both policies, each document checked, and
    each mutation of it tried (KO6), each bad case's defect isolated (KO2)."""
    review = (group[0].get("socket", {}).get("review") or []) if group else []
    tag = hashlib.sha256(json.dumps(review, sort_keys=True).encode()).hexdigest()[:12]
    socket_path = scratch / f"{Path(source).stem}--{part}--{tag}.json"
    socket, problem = run_fitment(binary, ["socket", str(directory / source), "--part", part, "--format", "json"],
                                  (0,))
    if problem:
        problems.append(f"{source} {part}: {problem}")
        return
    socket_path.write_text(json.dumps(socket), encoding="utf-8")
    if review:
        socket, problem = reviewed_socket(binary, socket, socket_path, review)
        if problem:
            problems.append(f"{source} {part}: {problem}")
            return
    leaves, face_id = xcaf_leaves(directory / source, bind=True)
    for _, _, shape in leaves:
        LEAF_FACE_IDS[id(shape)] = face_id
    for case in group:
        # A mirror image differs everywhere: it is no one defect, and KO2 does not judge it.
        if (case.get("truth") == "does_not_fit" and case.get("defect") and "placement" in case
                and case.get("class") != "b_mirror"):
            tally["defects isolated"] = tally.get("defects isolated", 0) + 1
            try:
                problems += one_thing_broken(directory, case, socket, leaves)
            except BooleanFailed as failed:
                problems.append(f"{case['id']}: {failed} failed in Open CASCADE")
    for clearance in ("strict", "functional"):
        doc, problem = run_fitment(binary, ["match", str(socket_path), *files, "--format", "json", "--clearance",
                                            clearance], (0, 4, 5), cwd=directory)
        if problem:
            problems.append(f"{source} {part} {clearance}: {problem}")
            continue
        tally["documents"] += 1
        problems += document_problems(directory, doc, socket, leaves, group, clearance, tally)
        for name, mutate in MATCH_MUTATIONS:
            mutated = mutate(doc, socket)
            if mutated is not None and document_problems(directory, mutated, socket, leaves, group, clearance,
                                                         dict(tally)):
                met.add(name)


def reviewed_socket(binary, socket, path, review):
    """`socket`, written at `path`, with a case's review applied by `fitment review` (M6-38): each entry's
    requirement located (KC5) in every socket that has it. The document read back, or the problem."""
    for entry in review:
        found = locate(socket, entry)
        if not found:
            return None, f"the review's {entry} names no requirement"
        flag = {"removed": "--remove", "kept": "--keep", "unreviewed": "--unreview"}.get(entry.get("status"))
        if flag is None:
            return None, f"the review's {entry} states no status fitment review knows"
        for socket_id, requirement in found:
            args = ["review", str(path), flag, requirement["id"], "--socket", socket_id]
            run = subprocess.run([str(binary), *args, *(["--note", entry["note"]] if entry.get("note") else [])],
                                 capture_output=True)
            if run.returncode != 0:
                return None, f"fitment review: exit {run.returncode}: {run.stderr.decode(errors='replace')}"
    return json.loads(path.read_text(encoding="utf-8")), None


def locate(socket, locator):
    """The requirement ids of a socket document that a case's locator (KC5) names: a bore or shaft by its axis
    line, a seat by its outward normal and offset, a grip by its length, a conical seat by its apex."""
    out = []
    for s_ in socket["extracted"]["sockets"]:
        for requirement in s_["requirements"]:
            q = requirement["requires"]
            if q["kind"] != locator["kind"]:
                continue
            if q["kind"] in ("bore", "shaft"):
                d, e = norm(q["axis"]["direction"]), norm(locator["axis_direction"])
                named = (abs(abs(dot(d, e)) - 1) <= 1e-9
                         and off_line(locator["axis_point"], q["axis"]["point_mm"], d) <= 1e-6)
            elif q["kind"] == "seat":
                named = (dot(q["normal"], locator["normal"]) >= 1 - 1e-9
                         and abs(q["offset_mm"] - locator["offset"]) <= 1e-6)
            elif q["kind"] == "grip":
                named = abs(q["length_mm"] - locator["length"]) <= 1e-6
            else:
                named = all(abs(a - b) <= 1e-6 for a, b in zip(q["apex_mm"], locator["apex"]))
            if named:
                out.append((s_["id"], requirement))
    return out


def one_thing_broken(directory, case, socket, leaves):
    """KO2: a bad case, at its construction's placement, departs physically at each occurrence only with the
    neighbours its defect's requirements concern there (their sources at that occurrence); a defect that disturbs
    another is not one defect."""
    # Per occurrence: a neighbour concerns the defect at the occurrences whose sources name it there.
    concerned = set()
    for locator in case["defect"]:
        for _, requirement in locate(socket, locator):
            concerned |= {(tuple(src["occurrence"]), tuple(src["neighbour"]))
                          for src in requirement.get("sources", [])}
    part = socket["selection"]["part"]
    paths = {}
    for neighbour in socket["extracted"]["neighbours"] + socket["extracted"]["occurrences"]:
        o = placement_rows(neighbour["transform"])
        product = neighbour["definition"]["product"]["id"]
        for k, (name, t, shape) in enumerate(leaves):
            if name == product and all(abs(t[i][j] - o[i][j]) <= 1e-9 for i in range(3) for j in range(4)):
                paths[id(shape)] = tuple(neighbour["path"])
    targets = [(m, shape) for name, m, shape in leaves if name == part]
    candidate = one_solid(directory / case["candidate"]["file"])
    originals = [shape for _, shape in targets]
    placed = [moved(candidate, compose(m, case["placement"])) for m, _ in targets]
    out = []
    for k, (m, original) in enumerate(targets):
        neighbours = neighbours_at(leaves, part, k, originals, placed)
        here = paths.get(id(original))
        for label, n_t, n_c in neighbours:
            found = physics(original, placed[k], [(label, n_t, n_c)], f"{case['id']} at occurrence {k}")
            if found and (here, paths.get(id(n_t))) not in concerned:
                out.append(f"{case['id']}: departs with {label}, which no requirement of its defect concerns "
                           f"(KO2): {found[0][1]}")
    return out


def target_faces(original, o, face_id):
    """The original's faces by #id, in the target's frame (`o` placed it)."""
    if face_id is None:
        return {}
    r_ = [row[:3] for row in o]
    inverse = [[r_[j][i] for j in range(3)] + [-sum(r_[j][i] * o[j][3] for j in range(3))] for i in range(3)]
    out = {}
    for face in (as_face(e) for e in explore(original, TopAbs_FACE)):
        id_ = face_id(face)
        if id_ is not None:
            out[id_] = as_face(moved(face, inverse))
    return out


def ko4_problems(directory, doc, socket, by_socket, occurrences, originals, clearance, tally):
    """KO4 on every placed socket result of every candidate, and the envelope's checks against the socket."""
    problems = []
    if not occurrences:
        return problems
    occurrence, o, k = occurrences[0]
    original_faces = target_faces(originals[k], o, LEAF_FACE_IDS.get(id(originals[k])))
    for entry in doc["candidates"]:
        label = entry["labels"][0]
        where = f"{label} ({clearance})"
        if entry["status"] == "refused":
            continue
        faces = None
        for result in entry["sockets"]:
            socket_doc = by_socket.get(result["socket"])
            if socket_doc is None:
                continue
            judged = [c for c in result["checks"] if c["result"] in ("passed", "failed", "not_checked")]
            if len(judged) != len(socket_doc["requirements"]):
                problems.append(f"{where} {result['socket']}: {len(judged)} checks for "
                                f"{len(socket_doc['requirements'])} requirements")
            if result.get("placement") is None:
                continue
            if faces is None:
                try:
                    _, faces, _ = bound_faces(directory / label)
                except OSError as error:
                    problems.append(f"{where}: {error}")
                    break
                candidate = one_solid(directory / label)
            m = placement_rows(result["placement"])
            candidate_placed = moved(candidate, m)
            requirements = {r_["id"]: r_ for r_ in socket_doc["requirements"]}
            for check in result["checks"]:
                if check["result"] != "failed" or check["requirement"] not in requirements:
                    continue
                q = requirements[check["requirement"]]["requires"]
                try:
                    problems += failure_problems(f"{where} {result['socket']}/{check['requirement']}", check, q,
                                                 faces, m, candidate_placed, original_faces,
                                                 clearance == "strict", tally)
                except BooleanFailed as failed:
                    problems.append(f"{where}: {failed} failed in Open CASCADE")
    return problems


# The #id function of each leaf shape xcaf_leaves bound, by the shape's identity.
LEAF_FACE_IDS = {}


def occurrence_leaves(where, socket, leaves):
    """Each of fitment's occurrences with its XCAF leaf (the one whose transform agrees within 1e-9), and the
    problems: an occurrence with no leaf, or a leaf of the part fitment's extraction left out."""
    part = socket["selection"]["part"]
    mine = [(t, shape) for name, t, shape in leaves if name == part]
    out, problems, used = [], [], set()
    for occurrence in socket["extracted"]["occurrences"]:
        o = placement_rows(occurrence["transform"])
        k = next((k for k, (t, _) in enumerate(mine)
                  if all(abs(t[i][j] - o[i][j]) <= 1e-9 for i in range(3) for j in range(4))), None)
        if k is None:
            problems.append(f"{where}: no XCAF leaf at occurrence {occurrence['path']}")
            continue
        used.add(k)
        out.append((occurrence, o, k))
    problems += [f"{where}: an XCAF leaf of {part} no occurrence of fitment's names" for k in range(len(mine))
                 if k not in used]
    return out, [shape for _, shape in mine], problems


def document_problems(directory, doc, socket, leaves, group, clearance, tally):
    """KO3, KO4 and KO5 on one fitment.match document, and what its envelope must state: the run's policy
    (KH2.6) and a check or a removal for every requirement of every socket."""
    linear = doc["policy"]["linear_mm"]
    angular = math.radians(doc["policy"]["angular_deg"])
    by_socket = {s_["id"]: s_ for s_ in socket["extracted"]["sockets"]}
    occurrences, originals, problems = occurrence_leaves(socket["selection"]["part"], socket, leaves)
    part = socket["selection"]["part"]
    if doc["policy"]["name"] != "default" or doc["policy"]["clearance"] != clearance:
        problems.append(f"{part} ({clearance}): the document states the policy {doc['policy']}")
    problems += ko4_problems(directory, doc, socket, by_socket, occurrences, originals, clearance, tally)
    for entry in doc["candidates"]:
        if entry["verdict"] == "reject":
            continue
        tally["passes"] += 1
        label = entry["labels"][0]
        truths = {c["truth"] for c in group if c["candidate"]["file"] in entry["labels"]}
        # FK2's `fits` cases that state their fit: a contact within the tolerance on a seat's plane is theirs.
        stated_fits = truths == {"fits"} and all((c.get("expect", {}).get("default") or {}).get("fit")
                                                 for c in group if c["candidate"]["file"] in entry["labels"])
        where = f"{label} ({clearance})"
        if {r_["socket"] for r_ in entry["sockets"]} != set(by_socket):
            problems.append(f"{where}: a pass whose socket results are not the document's sockets")
            continue
        try:
            _, faces, _ = bound_faces(directory / label)
        except OSError as error:
            problems.append(f"{where}: {error}")
            continue
        candidate = one_solid(directory / label)
        placements = {}
        for result in entry["sockets"]:
            m = placement_rows(result["placement"])
            problem = rigid_problem(f"{where} {result['socket']}", m)
            if problem:
                problems.append(problem)
            requirements = {r_["id"]: r_ for r_ in by_socket[result["socket"]]["requirements"]}
            for check in result["checks"]:
                if check["result"] == "passed" and check.get("feature") and check["requirement"] in requirements:
                    problems += feature_problems(f"{where} {result['socket']}/{check['requirement']}",
                                                 requirements[check["requirement"]], check["feature"], faces, m,
                                                 linear, angular, requirements, tally)
            placements[result["socket"]] = m
        # The candidate at every occurrence, each by its socket's placement.
        placed = list(originals)
        for occurrence, o, k in occurrences:
            placed[k] = moved(candidate, compose(o, placements[occurrence["socket"]]))
        try:
            for occurrence, o, k in occurrences:
                tally["occurrences"] += 1
                result_socket = by_socket[occurrence["socket"]]
                original = originals[k]
                if clearance == "functional":
                    original = ring_excused(original, placed[k], result_socket, o)
                if "waived" in truths:
                    # A waived pass is the user's acceptance of what its review removed: its features and
                    # placement are checked above (KO5), its physics is not KO3's to judge.
                    tally["not judged: waived passes"] = tally.get("not judged: waived passes", 0) + 1
                    continue
                if any(c.get("socket", {}).get("original_interferes") for c in group):
                    # FK4: the original already interferes by construction; KO3 judges against it.
                    key = "not judged: an original that interferes (FK4)"
                    tally[key] = tally.get(key, 0) + 1
                    continue
                found = physics(original, placed[k], neighbours_at(leaves, part, k, originals, placed),
                                f"{where} at {occurrence['path']}")
                if stated_fits:
                    found = fo_seat_excused(found, region_spec_from_document(result_socket), o)
                if "collides" in truths and found:
                    inside = collisions_problems(found, regions(region_spec_from_document(result_socket), o,
                                                                originals[k], placed[k]))
                    if not inside:
                        tally["collisions excused"] += 1
                    problems += [f"{text}: a collides pass that does not merely collide" for text in inside]
                    continue
                problems += [f"{d[1]}: a pass that does not fit" for d in found]
        except BooleanFailed as failed:
            problems.append(f"{where}: {failed} failed in Open CASCADE")
    return problems


# ---------------------------------------------------------------- match mode: failures re-measured (KO4)

SAME = 1e-6  # mm: a re-measured length agrees with fitment's within this
SAME_RAD = 1e-9  # rad
JOIN = 1e-9  # relative: pieces of one wall join end to end within this
RUN_TOUCH = 1e-9  # mm: axial.rs's TOUCH_MM, the least overlap with a span that counts
PROBE_PAST = 0.05  # mm: how far past a wall's end its opening is probed

# The codes a failed measure of each quantity may give (KO4: codes swapped between checks are caught here).
CODES = {
    "diameter_mm": {"diameter_out_of_limits:min", "diameter_out_of_limits:max"},
    "clear_margin_mm": {"diameter_out_of_limits:min", "diameter_out_of_limits:max"},
    "core_distance_mm": {"core_intruded"},
    "wall_from_mm": {"wall_missing"},
    "wall_to_mm": {"wall_missing"},
    "open_start": {"end_not_open"},
    "open_finish": {"end_not_open"},
    "covers": {"contact_not_covered", "unknown_outline", "coverage_exhausted", "floor_above_tolerance"},
    "plane_offset_mm": {"seat_missing", "no_feature"},
    "plane_sine": {"seat_missing", "no_feature"},
    # One code for both axis measures: axis_angle whenever the tilt fails too (axial.rs's `missing`).
    "axis_offset_mm": {"axis_offset", "axis_angle"},
    "axis_sine": {"axis_angle"},
    "cone_angle_rad": {"cone_mismatch:angle"},
    "apex_offset_mm": {"cone_mismatch:apex"},
    "cone_range_mm": {"cone_mismatch:range"},
    "grip_length_mm": {"grip_length"},
    "profile_offset_mm": {"diameter_out_of_limits:profile"},
}


def placed_face(face, m):
    """A face of the candidate, in its own frame, placed by `m` (a copy: its #id is known already)."""
    return as_face(moved(face, m))


class Revolution:
    """A coaxial piece of a candidate's stack, in the requirement's stations: its extent, its radius (None for a
    zone, a cone or torus between cylinders), and whether it is a hole's (concave) wall."""

    def __init__(self, lo, hi, radius, hole):
        self.lo, self.hi, self.radius, self.hole = lo, hi, radius, hole


def revolution_of(face):
    """(axis point, axis direction, radius or None, concave) of a cylindrical, conical or toroidal face."""
    from OCP.BRepGProp import BRepGProp_Face
    from OCP.gp import gp_Vec
    surface = BRepAdaptor_Surface(face)
    kind = surface.GetType()
    if kind == Kind.GeomAbs_Cylinder:
        c = surface.Cylinder()
        point, d, radius = xyz(c.Location()), norm(xyz(c.Axis().Direction())), c.Radius()
    elif kind == Kind.GeomAbs_Cone:
        c = surface.Cone()
        point, d, radius = xyz(c.Apex()), norm(xyz(c.Axis().Direction())), None
    elif kind == Kind.GeomAbs_Torus:
        t = surface.Torus()
        point, d, radius = xyz(t.Location()), norm(xyz(t.Axis().Direction())), None
    else:
        return None
    u0, u1, v0, v1 = BRepTools.UVBounds_s(face)
    at, normal = gp_Pnt(), gp_Vec()
    BRepGProp_Face(face).Normal((u0 + u1) / 2, (v0 + v1) / 2, at, normal)
    foot = add(point, d, dot(sub(xyz(at), point), d))
    concave = dot((normal.X(), normal.Y(), normal.Z()), sub(xyz(at), foot)) < 0
    return point, d, radius, concave


def extent_on(face, point, d):
    """The stations along the line (point, d) that a face's vertices span."""
    from OCP.BRep import BRep_Tool
    from OCP.TopAbs import TopAbs_VERTEX
    stations = [dot(sub(xyz(BRep_Tool.Pnt_s(downcast("Vertex")(v))), point), d) for v in explore(face, TopAbs_VERTEX)]
    if not stations:
        # A face with no vertex (a whole cylinder with seam-less circles): its parameter range.
        _, _, v0, v1 = BRepTools.UVBounds_s(face)
        return v0, v1
    return min(stations), max(stations)


def stack_of(faces, seed, q, m):
    """The candidate's stack about a requirement's axis: every placed cylinder, cone or torus coaxial with the
    seed's axis (the named feature's, or with none the nearest coaxial cylinder within 1 mm), of the seed's
    sense, as pieces in the requirement's stations; with the stack's axis line. None with no seed."""
    line_point, line_dir = q["axis"]["point_mm"], norm(q["axis"]["direction"])
    placed = {k: placed_face(f, m) for k, f in faces.items()}
    revolutions = {k: revolution_of(f) for k, f in placed.items()}
    if seed is None:
        near = [(off_line(r[0], line_point, line_dir), k) for k, r in revolutions.items()
                if r is not None and r[2] is not None and abs(dot(r[1], line_dir)) > 1 - 1e-6
                and off_line(r[0], line_point, line_dir) <= 1.0]
        if not near:
            return None
        seed = min(near)[1]
    if revolutions.get(seed) is None:
        return None
    sp, sd, _, concave = revolutions[seed]
    pieces = []
    for k, r in revolutions.items():
        if r is None or r[3] != concave or abs(dot(r[1], sd)) < 1 - 1e-9 or off_line(r[0], sp, sd) > SAME:
            continue
        a, b = extent_on(placed[k], line_point, line_dir)
        pieces.append(Revolution(a, b, r[2], concave))
    pieces.sort(key=lambda p: (p.lo, p.hi))
    return pieces, (sp, sd)


def offset_at(station, q, axis):
    """The distance of the requirement's axis at `station` from the stack's axis line."""
    line_point, line_dir = q["axis"]["point_mm"], norm(q["axis"]["direction"])
    return off_line(add(line_point, line_dir, station), axis[0], axis[1])


def overlapping(pieces, lo, hi):
    """The pieces that overlap a span by more than axial.rs's TOUCH_MM (or, a span no longer, hold its middle),
    as fitment's run takes them."""
    touch = RUN_TOUCH
    if hi - lo > touch:
        return [p for p in pieces if min(p.hi, hi) - max(p.lo, lo) > touch]
    return [p for p in pieces if p.lo <= (lo + hi) / 2 <= p.hi]


def wall_run(pieces, lo, hi):
    """The run of pieces joined end to end that holds the span's middle: its first and last stations."""
    mid = (lo + hi) / 2
    hit = next((k for k, p in enumerate(pieces) if p.lo <= mid <= p.hi), None)
    if hit is None:
        return None
    start = end = hit
    joined = lambda a, b: abs(b.lo - a.hi) <= JOIN * (1 + abs(a.hi))  # noqa: E731
    while start > 0 and joined(pieces[start - 1], pieces[start]):
        start -= 1
    while end + 1 < len(pieces) and joined(pieces[end], pieces[end + 1]):
        end += 1
    return pieces[start].lo, max(p.hi for p in pieces[start:end + 1])


def span_of(q, at):
    """The span of a requirement a measure's `span_mm` names."""
    lo, hi = at["span_mm"]
    return next((s_ for s_ in q.get("spans", []) if abs(s_["from_mm"] - lo) <= 1e-9 and abs(s_["to_mm"] - hi) <= 1e-9),
                None)


def remeasure_axial(quantity, measure, q, stack):
    """An axial measure re-measured from the stack's pieces (the coaxial cylinders, cones and tori of the named
    feature's sense) as fitment's run takes them: an axis's offset at the requirement's ends; a pinned span's
    covering cylinder; a clearance span's greatest (bore) or least (shaft) radius over it, and its margin; the
    run of joined pieces holding the span's middle. None where the oracle cannot (a span it cannot name)."""
    pieces, axis = stack
    at = measure.get("at", {})
    if quantity == "axis_offset_mm":
        spans = q.get("spans", [])
        if not spans:
            return None
        low = min(min(s_["from_mm"], s_["to_mm"]) for s_ in spans)
        high = max(max(s_["from_mm"], s_["to_mm"]) for s_ in spans)
        return max(offset_at(low, q, axis), offset_at(high, q, axis))
    span = span_of(q, at) if "span_mm" in at else None
    if span is None:
        return None
    lo, hi = min(span["from_mm"], span["to_mm"]), max(span["from_mm"], span["to_mm"])
    hole = q["kind"] == "bore"
    if quantity in ("wall_from_mm", "wall_to_mm"):
        run = wall_run(pieces, lo, hi)
        return None if run is None else run[0] if quantity == "wall_from_mm" else run[1]
    over = [p for p in overlapping(pieces, lo, hi) if p.radius is not None]
    if quantity == "diameter_mm":
        if span["fit"] == "clearance" and not span["pinned"]:
            if not over:
                return None
            return 2 * (max(p.radius for p in over) if hole else min(p.radius for p in over))
        covering = [p for p in over if p.lo - lo < TOL and hi - p.hi < TOL]
        return 2 * covering[0].radius if covering else None
    if quantity == "clear_margin_mm":
        clear = span.get("clear_diameter_mm")
        if not over or clear is None:
            return None
        delta = max(offset_at(span["from_mm"], q, axis), offset_at(span["to_mm"], q, axis))
        if hole:
            return min(p.radius for p in over) - delta - clear / 2
        return clear / 2 - max(p.radius for p in over) - delta
    return None


def open_end(quantity, q, stack, candidate_placed):
    """Whether the candidate's stack, at the end of the requirement's spans the measure names, is open within the
    tolerance of that end: its wall's last station there within TOL of the spans' end, with air on the axis just
    past it."""
    pieces, axis = stack
    spans = q.get("spans", [])
    if not spans or not pieces:
        return None
    start = quantity == "open_start"
    edge = (min(min(s_["from_mm"], s_["to_mm"]) for s_ in spans) if start
            else max(max(s_["from_mm"], s_["to_mm"]) for s_ in spans))
    end = min(p.lo for p in pieces) if start else max(p.hi for p in pieces)
    line_point, line_dir = q["axis"]["point_mm"], norm(q["axis"]["direction"])
    probe = add(line_point, line_dir, end + (-PROBE_PAST if start else PROBE_PAST))
    foot = add(axis[0], axis[1], dot(sub(probe, axis[0]), axis[1]))
    classifier = BRepClass3d_SolidClassifier(candidate_placed, gp_Pnt(*foot), 1e-7)
    air = classifier.State() == TopAbs_OUT
    return air and abs(end - edge) < TOL


def plane_deviation(face, q, m):
    """VR3's deviation of a placed plane face from a seat over its contacts' outlines: each vertex's distance, and
    each arc's centre's plus its radius times the tilt."""
    facts = plane_facts(face)
    if facts is None:
        return None
    normal, point = turn(m, facts[0]), apply_point(m, facts[1])
    offset = dot(normal, point)
    sine = math.sqrt(dot(cross(normal, q["normal"]), cross(normal, q["normal"])))
    pts = []
    for contact in q.get("contacts", []):
        for loop in (contact.get("outline") or {}).get("loops", []):
            for s_ in loop.get("segments", []):
                if s_["kind"] == "line":
                    pts += [(s_["from_mm"], 0.0), (s_["to_mm"], 0.0)]
                else:
                    pts.append((s_["center_mm"], s_["radius_mm"]))
    if not pts:
        pts = [([c * q["offset_mm"] for c in q["normal"]], 0.0)]
    return max(abs(dot(normal, p) - offset) + r * sine for p, r in pts)


def uncovered_area(contact_face, candidate_placed, normal, band):
    """The area of a target contact face no face of the candidate covers: the candidate's faces on the contact's
    plane, facing its way within `band`, moved onto it and intersected."""
    facts = plane_facts(contact_face)
    if facts is None:
        return None
    _, point, area = facts
    covered = 0.0
    for f in (as_face(e) for e in explore(candidate_placed, TopAbs_FACE)):
        mine = plane_facts(f)
        if mine is None or dot(mine[0], normal) < 1 - 1e-9:
            continue
        gap = dot(sub(mine[1], point), normal)
        if abs(gap) > band or apart(f, contact_face, band):
            continue
        covered += area_of(common(contact_face, translated(f, [-gap * c for c in normal])))
    return area - covered


def violates(value, measure):
    """True if `value` breaks the measure's stated bound (a NaN, no value, always does)."""
    if value is None or (isinstance(value, float) and math.isnan(value)):
        return True
    lo, hi = measure.get("min"), measure.get("max")
    return (lo is not None and value <= lo) or (hi is not None and value >= hi)


def number(found):
    """A measure's found value; None for none, which fitment writes as ±f64::MAX (EV2: JSON has no NaN)."""
    return None if found is None or abs(found) >= 1e300 else found


def agrees(value, found, quantity):
    found = number(found)
    if value is None:
        return True
    if isinstance(value, float) and math.isnan(value):
        return found is None
    if found is None:
        return False
    return abs(value - found) <= (SAME_RAD if quantity.endswith("_rad") or quantity.endswith("sine") else SAME)


def failure_problems(where, check, q, faces, m, candidate_placed, original_faces, strict, tally):
    """KO4 on one failed check: a failed check fails something; each failed measure re-measured by Open CASCADE
    agrees with fitment's found and breaks its bound; and the check's codes include one its failed measures
    allow."""
    measures = check.get("measures") or []
    failures = check.get("failures") or []
    failed = [m_ for m_ in measures if m_.get("passed") is False]
    if not failed and not failures:
        return [f"{where}: failed with no failure and no failing measure"]
    out = []
    codes = {f["code"] for f in failures}
    feature = check.get("feature")
    seed = int(feature.split("+")[0].rsplit("-", 1)[1]) if feature else None
    stack = None
    if q["kind"] in ("bore", "shaft"):
        stack = stack_of(faces, seed if seed in faces else None, q, m)
    for measure in failed:
        quantity = measure["quantity"]
        allowed = CODES.get(quantity)
        if allowed is not None and not (codes & allowed):
            out.append(f"{where}: {quantity} failed, but the check's codes {sorted(codes)} name none of "
                       f"{sorted(allowed)}")
        value = None
        if quantity in ("open_start", "open_finish"):
            is_open = open_end(quantity, q, stack, candidate_placed) if stack else None
            if is_open is None:
                count(tally, f"not re-measured: {quantity}")
            else:
                tally["re-measured"] += 1
                if is_open:
                    out.append(f"{where}: {quantity} failed, but the candidate's wall ends open within the "
                               f"tolerance of the span's end")
            continue
        if quantity in ("axis_offset_mm", "diameter_mm", "clear_margin_mm", "wall_from_mm", "wall_to_mm"):
            # No piece found (±f64::MAX) is the stack's own composition, M2's reading: an annular bottom, a
            # zone not taken.
            if number(measure.get("found")) is None:
                count(tally, "not re-measured: no piece fitment found")
                continue
            value = remeasure_axial(quantity, measure, q, stack) if stack else None
        elif quantity == "plane_offset_mm":
            # With no feature named (no plane within the band), fitment's nearest facing plane.
            if seed in faces:
                value = plane_deviation(faces[seed], q, m)
            else:
                value = min((dev for dev in (plane_deviation(f, q, m) for f in faces.values()
                                              if facing(f, q, m)) if dev is not None), default=None)
        elif quantity == "plane_sine" and seed in faces:
            facts = plane_facts(faces[seed])
            if facts is not None:
                n = turn(m, facts[0])
                value = math.sqrt(dot(cross(n, q["normal"]), cross(n, q["normal"])))
        elif quantity == "cone_angle_rad" and seed in faces:
            surface = BRepAdaptor_Surface(faces[seed])
            value = 2 * abs(surface.Cone().SemiAngle()) if surface.GetType() == Kind.GeomAbs_Cone else None
        elif quantity == "covers":
            face = original_faces.get(measure.get("at", {}).get("contact_face"))
            uncovered = uncovered_area(face, candidate_placed, q["normal"], TOL) if face is not None else None
            if uncovered is not None:
                tally["re-measured"] += 1
                if uncovered <= SAME:
                    out.append(f"{where}: covers failed, but the candidate covers the contact face to "
                               f"{uncovered:.3g} mm²")
            continue
        elif quantity == "core_distance_mm":
            # A sound bound from boxes: a hole's face may lie no nearer than stated, a shaft's no farther. The
            # true distance may keep the bound (the box was loose: a false negative by the rule, never a pass).
            value = core_distance(check, q, measure, faces, m)
            found = number(measure.get("found"))
            if value is None or found is None:
                count(tally, "not re-measured: core_distance_mm, " + ("no distance stated" if found is None
                                                                      else "no span or face to measure"))
                continue
            if True:
                tally["re-measured"] += 1
                hole = q["kind"] == "bore"
                if (found > value + SAME) if hole else (found < value - SAME):
                    out.append(f"{where}: core_distance_mm states {found}, but the face lies {value} from the axis")
                if not violates(value, measure):
                    tally["core bounds loose"] = tally.get("core bounds loose", 0) + 1
            continue
        if value is None:
            count(tally, f"not re-measured: {quantity}")
            continue
        tally["re-measured"] += 1
        if not agrees(value, measure.get("found"), quantity):
            out.append(f"{where}: {quantity} found {measure.get('found')}, Open CASCADE measures {value}")
        if not violates(value, measure):
            out.append(f"{where}: {quantity} failed, but {value} keeps its bound")
    return out


def count(tally, key):
    tally[key] = tally.get(key, 0) + 1


def facing(face, q, m):
    """True if a candidate face, placed, is a plane facing the seat's way (fitment's candidates for a seat)."""
    facts = plane_facts(face)
    return facts is not None and dot(turn(m, facts[0]), q["normal"]) > 0


def core_distance(check, q, measure, faces, m):
    """The face a core failure names, placed, against the requirement's axis over the span: for a bore, its least
    distance from the axis segment; for a shaft, its greatest radial distance within the span, from its vertices
    and a grid of its points."""
    from OCP.BRepBuilderAPI import BRepBuilderAPI_MakeEdge
    from OCP.BRepExtrema import BRepExtrema_DistShapeShape
    from OCP.BRepClass import BRepClass_FaceClassifier
    from OCP.BRep import BRep_Tool
    from OCP.TopAbs import TopAbs_VERTEX
    from OCP.gp import gp_Pnt2d
    named = [int(x) for f in check.get("failures") or [] if f["code"] == "core_intruded"
             for x in re.findall(r"face #(\d+)", f.get("detail", ""))]
    span = span_of(q, measure.get("at", {}))
    if not named or named[0] not in faces or span is None:
        return None
    face = placed_face(faces[named[0]], m)
    line_point, line_dir = q["axis"]["point_mm"], norm(q["axis"]["direction"])
    lo, hi = min(span["from_mm"], span["to_mm"]), max(span["from_mm"], span["to_mm"])
    if q["kind"] == "bore":
        a, b = add(line_point, line_dir, lo), add(line_point, line_dir, hi)
        found = BRepExtrema_DistShapeShape(BRepBuilderAPI_MakeEdge(gp_Pnt(*a), gp_Pnt(*b)).Edge(), face)
        return found.Value() if found.IsDone() else None
    points = [xyz(BRep_Tool.Pnt_s(downcast("Vertex")(v))) for v in explore(face, TopAbs_VERTEX)]
    surface = BRepAdaptor_Surface(face)
    u0, u1, v0, v1 = BRepTools.UVBounds_s(face)
    n = 24
    for i in range(n + 1):
        for j in range(n + 1):
            u, v = u0 + i / n * (u1 - u0), v0 + j / n * (v1 - v0)
            if BRepClass_FaceClassifier(face, gp_Pnt2d(u, v), 1e-7).State() != TopAbs_OUT:
                points.append(xyz(surface.Value(u, v)))
    within = [p for p in points if lo - 1e-9 <= dot(sub(p, line_point), line_dir) <= hi + 1e-9]
    return max((off_line(p, line_point, line_dir) for p in within), default=None)


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
            return [path / case["file"] for case in listed["cases"] if case.get("kind", "part") == "part"]
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


def ring(m, axis, station, radius, count=8):
    """`count` points at `radius` about `axis` at `station`, in the root
    frame."""
    p, d = axis["point_mm"], axis["direction"]
    u, w = perpendiculars(d)
    out = []
    for k in range(count):
        c, s = math.cos(2 * k * math.pi / count), math.sin(2 * k * math.pi / count)
        out.append(apply(m, [p[i] + station * d[i] + radius * (c * u[i] + s * w[i]) for i in range(3)]))
    return out


def core_problems(where, requirement, m, target, guard, tally):
    """U11 and X8 against Open CASCADE: a bore's or shaft's core that
    fitment leaves unflagged holds none of the target's own material at
    points sampled well inside it (a bore's cylinder of radius c/2 + tau +
    g or d/2 - tau; a shaft's outside of c/2 - tau - g or d/2 + tau), and a
    zone piece's profile is the target's own radius halfway along it: air
    just inside it for a hole, material just inside it for a shaft."""
    problems = []
    q = requirement["requires"]
    hole = q["kind"] == "bore"
    tau = max(0.01, requirement["floor_mm"], guard)
    flagged = "core_not_clear" in requirement["flags"]
    for span in q["spans"]:
        own = span["hole_diameter_mm" if hole else "shaft_diameter_mm"]
        clear = span["clear_diameter_mm"]
        a, b = span["from_mm"], span["to_mm"]
        if span.get("profile_mm") is not None:
            # A quarter and three quarters along: the profile's radius there,
            # so a profile reversed or moved is met.
            tally["zone profiles"] = tally.get("zone profiles", 0) + 1
            p0, p1 = span["profile_mm"]
            for k in (0.25, 0.75):
                r = (p0 + (p1 - p0) * k) / 2
                airy, solid = (r - 0.02, r + 0.02) if hole else (r + 0.02, r - 0.02)
                at = a + (b - a) * k
                if any(inside_any(pt, [target]) for pt in ring(m, q["axis"], at, airy)) or \
                        not all(inside_any(pt, [target]) for pt in ring(m, q["axis"], at, solid)):
                    problems.append(f"{where}: zone profile {span['profile_mm']} is not the part's radius at {at:.4f}")
                    break
        if flagged or b - a <= 0.05 or span.get("basis") == "past_known_end":
            continue
        if span["pinned"] or span.get("basis") == "zone":
            rho = own / 2 - 0.01 if hole else own / 2 + 0.01
        elif clear is not None:
            rho = clear / 2 + tau + guard if hole else clear / 2 - tau - guard
        else:
            continue
        tally["cores sampled"] = tally.get("cores sampled", 0) + 1
        radii = [rho * k / 4 for k in range(4)] + [rho - 0.02] if hole else \
            [rho + x for x in (0.02, 0.1, 0.3, 0.6, 1, 2, 3, 5, 8, 12, 18, 25, 35, 50, 75, 100)]
        for station in (a + 0.02, (a + b) / 2, b - 0.02):
            for radius in (x for x in radii if x >= 0):
                hit = next((pt for pt in ring(m, q["axis"], station, radius, 16) if inside_any(pt, [target])), None)
                if hit is not None:
                    return problems + [f"{where}: the part's own material at radius {radius:.4f}, station "
                                       f"{station:.4f}, inside the core of radius {rho:.4f}, and no core_not_clear"]
    return problems


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
    plane, shares with `face`. A face tilted within the parallel window is
    first turned about the point its plane states, square to `normal`."""
    from OCP.BRepAlgoAPI import BRepAlgoAPI_Common
    from OCP.BRepBuilderAPI import BRepBuilderAPI_Transform
    from OCP.gp import gp_Ax1, gp_Dir, gp_Trsf, gp_Vec
    facts = plane_facts(other)
    if facts is not None and dot(facts[0], normal) > -1 + 1e-12:
        towards = [-c for c in normal]
        axis = cross(facts[0], towards)
        size = math.sqrt(dot(axis, axis))
        if size > 1e-15:
            turn_ = gp_Trsf()
            turn_.SetRotation(gp_Ax1(gp_Pnt(*facts[1]), gp_Dir(*[c / size for c in axis])),
                              math.atan2(size, dot(facts[0], towards)))
            other = BRepBuilderAPI_Transform(other, turn_, True).Shape()
    t = gp_Trsf()
    t.SetTranslation(gp_Vec(*[-gap * c for c in normal]))
    moved = BRepBuilderAPI_Transform(other, t, True).Shape()
    common = BRepAlgoAPI_Common(face, moved)
    if not common.IsDone():
        return math.inf
    props = GProp_GProps()
    BRepGProp.SurfaceProperties_s(common.Shape(), props)
    return props.Mass()


def cone_faces(shape):
    """Each conical face of `shape`: its apex, axis (the way it widens),
    semi-angle, the least and greatest distance of its edges from the axis,
    and its sense: material just outside the surface, away from the axis,
    makes it a countersink (`hole`), else a cone on a shaft (`shaft`)."""
    out = []
    for shape_face in explore(shape, TopAbs_FACE):
        face = as_face(shape_face)
        surface = BRepAdaptor_Surface(face)
        if surface.GetType() != Kind.GeomAbs_Cone:
            continue
        cone = surface.Cone()
        apex = xyz(cone.Apex())
        d = norm(xyz(cone.Axis().Direction()))
        radii = []
        for e in explore(face, TopAbs_EDGE):
            curve = BRepAdaptor_Curve(as_edge(e))
            a, b = curve.FirstParameter(), curve.LastParameter()
            for k in range(65):
                p = xyz(curve.Value(a + (b - a) * k / 64))
                along = dot(sub(p, apex), d)
                radii.append(math.sqrt(max(dot(sub(p, apex), sub(p, apex)) - along * along, 0.0)))
        mid = xyz(surface.Value(*on_face(face, BRepTools.UVBounds_s(face))))
        along = dot(sub(mid, apex), d)
        foot = add(apex, d, along)
        out.append({"apex": apex, "d": d if along >= 0 else tuple(-x for x in d), "semi": abs(cone.SemiAngle()),
                    "radii": (min(radii), max(radii)), "mid": mid, "radial": norm(sub(mid, foot))})
    for f in out:
        f["sense"] = "hole" if inside_any(add(f["mid"], f["radial"], PROBE), [shape]) else "shaft"
    return out


def cone_problems(where, q, faces, target, others, tally):
    """Q1 and Q3 against Open CASCADE: the target cone's sense, and its
    range: from the least to the greatest of the ranges it shares with each
    other solid's cone on its axis, apex within τ along it."""
    problems = []
    if any(f["sense"] != q["sense"] for f in faces):
        problems.append(f"{where}: sense {q['sense']}, Open CASCADE finds {[f['sense'] for f in faces]}")
    if q["radii_mm"] is None:
        return problems
    own = (min(f["radii"][0] for f in faces), max(f["radii"][1] for f in faces))
    d, apex = faces[0]["d"], faces[0]["apex"]
    shares = []
    for other in others:
        for g in cone_faces(other):
            if abs(abs(dot(g["d"], d)) - 1) > 1e-9 or math.dist(g["apex"], apex) > 0.02 \
                    or abs(g["semi"] - faces[0]["semi"]) > 1e-6:
                continue
            lo, hi = max(own[0], g["radii"][0]), min(own[1], g["radii"][1])
            if lo <= hi + 1e-9:
                shares.append((lo, hi))
    tally["cone ranges"] = tally.get("cone ranges", 0) + 1
    if not shares:
        problems.append(f"{where}: no other solid's cone shares its range")
    elif abs(min(s[0] for s in shares) - q["radii_mm"][0]) > 1e-4 or \
            abs(max(s[1] for s in shares) - q["radii_mm"][1]) > 1e-4:
        problems.append(f"{where}: radii {q['radii_mm']}, Open CASCADE finds shares {shares}")
    return problems


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
            faces = [f for f in cone_faces(target)
                     if math.dist(f["apex"], apexes[q["cone"]]) <= 1e-5
                     and abs(2 * f["semi"] - q["included_angle_rad"]) <= 1e-9]
            if not faces:
                problems.append(f"{where} {requirement['id']}: the part has no cone with that apex and angle")
                continue
            problems += cone_problems(f"{where} {requirement['id']}", q, faces, target, others, tally)
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
    # Every bore and shaft lies on a cylinder of the target, of one of its
    # spans' diameters.
    cylinders = []
    for f in explore(target, TopAbs_FACE):
        surface = BRepAdaptor_Surface(as_face(f))
        if surface.GetType() == Kind.GeomAbs_Cylinder:
            c = surface.Cylinder()
            cylinders.append((xyz(c.Location()), xyz(c.Axis().Direction()), 2 * c.Radius()))
    for q in requirements:
        if q["kind"] not in ("bore", "shaft") or not q["spans"]:
            continue
        tally["axes"] = tally.get("axes", 0) + 1
        diameters = [sp.get("hole_diameter_mm") or sp.get("shaft_diameter_mm") for sp in q["spans"]]
        if not any(on_line(m, q["axis"], p, d) and any(abs(x - dia) <= 1e-4 for x in diameters)
                   for p, d, dia in cylinders):
            problems.append(f"{where} {q['kind']} {q.get('hole') or q.get('shaft')}: no cylinder of the part "
                            f"lies on its axis at {diameters}")
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
    contact_faces = {face_id(as_face(f)): as_face(f) for f in explore(target, TopAbs_FACE)}
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
                            # Opposed at the gap, and within it of this
                            # contact's face, not another's on the plane.
                            mine_face = contact_faces.get(contact["face"])
                            # (An outline fitment could not read places the
                            # toucher nowhere: counted apart, above.)
                            near_it = mine_face is None or "unknown_outline" in by["flags"] or \
                                BRepExtrema_DistShapeShape(mine_face, f).Value() <= \
                                abs(gap) + max(TOUCH, requirement_floor) + 1e-6
                            if abs(dot(facts[0], n) + 1) <= 1e-9:
                                at_gap = abs(gap - by["gap_mm"]) <= 1e-5
                            else:
                                # Tilted within the window: fitment states the
                                # least gap over both outlines' vertices, which
                                # may lie past the contact, so no more than the
                                # faces' true distance, in the band.
                                d = BRepExtrema_DistShapeShape(mine_face, f).Value() if mine_face is not None else 0.0
                                at_gap = by["gap_mm"] <= d + 1e-6 and d <= NEAR + requirement_floor
                            found = found or (dot(facts[0], n) <= -COS_LIMIT and at_gap and near_it)
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
    # And the other way: every plane face of another solid opposed to a
    # contact face within the band, sharing area with it, is listed among
    # that contact's touchers (the merged socket's, of every occurrence).
    if face_id(None):
        from OCP.Bnd import Bnd_Box
        from OCP.BRepBndLib import BRepBndLib
        ours = {face_id(as_face(f)): as_face(f) for f in explore(target, TopAbs_FACE)}
        target_box = Bnd_Box()
        BRepBndLib.Add_s(target, target_box)
        near = []
        for i, (_, lm, shape) in enumerate(leaves):
            if i == mine:
                continue
            box = Bnd_Box()
            BRepBndLib.Add_s(shape, box)
            if box.Distance(target_box) > NEAR:
                continue
            paths = [p for p, t in placed.items()
                     if all(abs(lm[r][c] - t[r][c]) <= 1e-6 for r in range(3) for c in range(4))]
            near.append((paths, planar_faces(shape)))
        for requirement in socket["requirements"]:
            q = requirement["requires"]
            if q["kind"] != "seat":
                continue
            for contact in q["contacts"]:
                face = ours.get(contact["face"])
                facts = plane_facts(face) if face is not None else None
                if facts is None:
                    continue
                n, point = facts[0], facts[1]
                listed = {(tuple(b_["neighbour"]), b_["face"]) for b_ in contact["by"]}
                fbox = Bnd_Box()
                BRepBndLib.Add_s(face, fbox)
                for paths, theirs in near:
                    for g, gn, gp, gb in theirs:
                        gap = dot(n, sub(gp, point))
                        if dot(gn, n) > -COS_LIMIT or abs(gap) > NEAR or fbox.Distance(gb) > NEAR + 1e-6:
                            continue
                        if common_area(face, g, n, gap) <= 1e-6:
                            continue
                        tally["touchers found"] = tally.get("touchers found", 0) + 1
                        if not any((p, face_id(g)) in listed for p in paths):
                            problems.append(f"{where} seat {q['plane']}: face #{face_id(g)} of {paths} lies on "
                                            f"contact #{contact['face']}, {gap:.3g} mm away, and is not listed")
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
    numbers its other requirements and left-out features name (their ids
    and their `hole`, `shaft`, `plane`, `cylinder`, `cone` or `feature`;
    a seat's or grip's plane holds no face by itself); and the neighbour
    faces its unread neighbours (U4) list."""
    faces, features, unread = set(), set(), set()
    number = lambda text: [int(x) for x in re.findall(r"-(\d+)", text or "")]
    for r in socket["requirements"]:
        q = r["requires"]
        if q["kind"] in ("seat", "grip"):
            # A seat holds its plane's faces as contacts only (below).
            for c in q.get("contacts", []):
                faces.add(c["face"])
            continue
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
    left-out axis and diameter, a cone on a conical seat's apex. A plane on
    a seat's plane is held only as one of its contacts."""
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
        # A face on a seat's plane is held as one of its contacts, and so
        # only; any other by the ids and geometry the socket states.
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
        if MUTATE is not None:
            MUTATE(document)
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
            stated = extracted["target"]
            guard = max(document["extraction"]["socket"].get("guard_mm", 1e-6), stated["part_uncertainty_mm"],
                        stated["vertex_offset_mm"])
            for requirement in socket["requirements"]:
                requires = requirement["requires"]
                if requires["kind"] in ("bore", "shaft"):
                    for span in requires["spans"]:
                        problems += span_problems(f"{where} {requirement['id']}", requires["kind"],
                                                  requires["axis"], span, m, leaves[mine[0]][2], others, tally)
                    problems += core_problems(f"{where} {requirement['id']}", requirement, m, leaves[mine[0]][2],
                                              guard, tally)
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


# A mutation `mutations` mode applies to every document fitment gives.
MUTATE = None

# The cases the socket oracle may skip, each saying why in its construction.
SKIPPED = {"s55_tiles"}


def each_requirement(document):
    for socket in document["extracted"]["sockets"]:
        for r in socket["requirements"]:
            yield socket, r


def drop_core_flags(document):
    """U11: every core_not_clear flag dropped."""
    for _, r in each_requirement(document):
        r["flags"] = [f for f in r["flags"] if f != "core_not_clear"]


def widen_profiles(document):
    """X8: every zone profile 0.5 mm wider."""
    for _, r in each_requirement(document):
        for sp in r["requires"].get("spans", []):
            if sp.get("profile_mm") is not None:
                sp["profile_mm"] = [x + 0.5 for x in sp["profile_mm"]]


def reverse_profiles(document):
    """X8: every zone profile end for end."""
    for _, r in each_requirement(document):
        for sp in r["requires"].get("spans", []):
            if sp.get("profile_mm") is not None:
                sp["profile_mm"] = sp["profile_mm"][::-1]


def flip_senses(document):
    """Q3: every conical seat's sense the other way."""
    for _, r in each_requirement(document):
        q = r["requires"]
        if q["kind"] == "conical_seat":
            q["sense"] = "shaft" if q["sense"] == "hole" else "hole"


def narrow_ranges(document):
    """Q1: every known conical range's greatest radius 0.5 mm less, as one
    neighbour's range alone states it where another reaches further."""
    for _, r in each_requirement(document):
        q = r["requires"]
        if q["kind"] == "conical_seat" and q["radii_mm"] is not None:
            q["radii_mm"] = [q["radii_mm"][0], max(q["radii_mm"][0], q["radii_mm"][1] - 0.5)]


def drop(kind):
    def mutate(document):
        for socket in document["extracted"]["sockets"]:
            socket["requirements"] = [r for r in socket["requirements"] if r["requires"]["kind"] != kind]
    return mutate


def move_axes(document):
    for _, r in each_requirement(document):
        q = r["requires"]
        if q["kind"] in ("bore", "shaft"):
            u, _ = perpendiculars(q["axis"]["direction"])
            q["axis"]["point_mm"] = [p + 0.01 * x for p, x in zip(q["axis"]["point_mm"], u)]


def swap_fits(document):
    for _, r in each_requirement(document):
        for sp in r["requires"].get("spans", []):
            if sp["fit"] == "clearance":
                sp["fit"], sp["clear_diameter_mm"] = "interference", None
            elif sp["fit"] == "interference":
                own = sp.get("hole_diameter_mm") or sp.get("shaft_diameter_mm")
                sp["fit"], sp["clear_diameter_mm"] = "clearance", own


def shrink_contacts(document):
    for _, r in each_requirement(document):
        contacts = r["requires"].get("contacts", [])
        for c in contacts:
            if len(c["by"]) > 1:
                c["by"].pop()
        if len(contacts) > 1:
            contacts.pop()


def merge_sockets(document):
    sockets = document["extracted"]["sockets"]
    if len(sockets) > 1:
        for o in document["extracted"]["occurrences"]:
            o["socket"] = sockets[0]["id"]


def null_extras(document):
    for _, r in each_requirement(document):
        if r["requires"]["kind"] == "shaft":
            r["requires"]["max_extra_mm"] = None


MUTATIONS = {"drop bores": drop("bore"), "drop shafts": drop("shaft"), "drop seats": drop("seat"),
             "drop grips": drop("grip"), "move axes 0.01 mm": move_axes, "swap fits": swap_fits,
             "shrink contacts": shrink_contacts, "merge sockets": merge_sockets, "null max_extra": null_extras,
             "flip cone senses": flip_senses, "narrow cone ranges": narrow_ranges,
             "drop core flags": drop_core_flags, "widen zone profiles": widen_profiles,
             "reverse zone profiles": reverse_profiles}


def mutations_main(argv):
    """Every mutation of fitment's documents must meet a disagreement."""
    global MUTATE
    directory = Path(argv[0])
    binary = Path(os.environ.get("FITMENT", Path(__file__).resolve().parent.parent / "target/release/fitment"))
    cases = [c for c in json.loads((directory / "manifest.json").read_text(encoding="utf-8"))["cases"]
             if c.get("kind") == "assembly" and "sockets" in c["expect"]
             and not c.get("construction", {}).get("socket_oracle")]
    missed = []
    fresh = lambda: {"cases": 0, "others": 0, "bore spans": 0, "shaft spans": 0, "left out holes": 0, "seats": 0,
                     "conical seats": 0, "grips": 0, "seat pairs": 0, "faces off every seat": 0, "touching faces": 0,
                     "max extras": 0, "touchers": 0, "grip holes": 0, "partial cylinders": 0}
    # What the documents as given already meet is not the mutation's.
    MUTATE = None
    baseline = {case["id"]: set(socket_problems(directory, case, binary, fresh())) for case in cases}
    for name, mutate in MUTATIONS.items():
        MUTATE = mutate
        failing = sum(1 for case in cases
                      if set(socket_problems(directory, case, binary, fresh())) - baseline[case["id"]])
        print(f"{name}: {failing} cases disagree")
        if not failing:
            missed.append(name)
    MUTATE = None
    if missed:
        print("mutations no check catches: " + ", ".join(missed), file=sys.stderr)
    sys.exit(1 if missed else 0)


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
        if case.get("construction", {}).get("socket_oracle"):
            if case["id"] not in SKIPPED:
                problems.append(f"{case['id']} asks to skip the socket oracle, and is not among {sorted(SKIPPED)}")
                continue
            print(f"skip {case['id']}: {case['construction']['socket_oracle']}")
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


# ---------------------------------------------------------------- fit mode (FO1 to FO3, VT72)

FO_TIGHT = {"B2", "B3", "B4", "B8", "I2", "I7"}  # sources whose relation declares a zone (FZ3)
FO_AGREE_REL, FO_AGREE_ABS = 1e-6, 1e-7  # FO2: a re-measured residual agrees with fitment's within these
FO_TURNS = 8  # FO1, FO2: an excused free turn is checked at this many turns about its axis


def parts_paths(binary, path):
    """`fitment parts`' leaf occurrences of an assembly: path (usage #ids) -> (product name, 3x4 rows)."""
    doc, problem = run_fitment(binary, ["parts", str(path), "--format", "json"], (0,))
    if problem:
        return None, problem
    out = {}
    for p in doc["parts"]:
        if p["kind"] != "part":
            continue
        for o in p["occurrences"]:
            out[tuple(o["path"])] = (p["definition"]["product"]["name"], placement_rows(o["transform"]))
    return out, None


class FitScene:
    """One socket of a source, as the fit mode reads it: Open CASCADE's leaves bound by face #id, fitment's
    socket document, each of fitment's paths bound to its leaves (by product and transform), and the target's
    occurrences with their transforms."""

    def __init__(self, socket, leaves, face_id, paths):
        self.socket, self.leaves, self.face_id = socket, leaves, face_id
        self.part = socket["selection"]["part"]
        self.occurrences, self.originals, self.problems = occurrence_leaves(self.part, socket, leaves)
        self.targets = [i for i, (n, _, _) in enumerate(leaves) if n == self.part]
        self.by_path = {}
        for path, (name, m) in paths.items():
            self.by_path[path] = [i for i, (n, t, _) in enumerate(leaves) if n == name and all(
                abs(t[i_][j] - m[i_][j]) <= 1e-9 for i_ in range(3) for j in range(4))]
        self.at = {tuple(occ["path"]): (occ, o, k) for occ, o, k in self.occurrences}

    def neighbours(self):
        """Every leaf that is no occurrence of the target, by index."""
        return [i for i in range(len(self.leaves)) if i not in self.targets]

    def body(self, path):
        """What a path names: ("copy", path) for an occurrence of the target, else ("leaf", i) for each leaf."""
        path = tuple(path)
        if path in self.at:
            return [("copy", path)]
        return [("leaf", i) for i in self.by_path.get(path, [])]

    def face(self, body, face):
        """The face #id `face` of a body, placed: a leaf's, or the original's at a copy's occurrence (FZ5)."""
        shape = self.leaves[body[1]][2] if body[0] == "leaf" else self.originals[self.at[body[1]][2]]
        return next((as_face(f) for f in explore(shape, TopAbs_FACE) if self.face_id(as_face(f)) == face), None)

    def sources(self, occurrence):
        """Every requirement of every socket with its sources at `occurrence`."""
        for s_ in self.socket["extracted"]["sockets"]:
            for requirement in s_["requirements"]:
                yield requirement, [s for s in requirement.get("sources", []) if tuple(s["occurrence"]) == occurrence]


def fo_slab(face, d):
    """A planar face swept d either side of its plane; None for a face on another surface."""
    from OCP.BRepPrimAPI import BRepPrimAPI_MakePrism
    from OCP.gp import gp_Vec
    facts = plane_facts(face)
    if facts is None:
        return None
    n = facts[0]
    return BRepPrimAPI_MakePrism(translated(face, [-d * c for c in n]), gp_Vec(*[2 * d * c for c in n])).Shape()


def fo_annulus(o, axis, lo, hi, r0, r1):
    """The ring about `axis` (the target's frame, placed by o) from station lo to hi, radii r0 to r1."""
    from OCP.BRepPrimAPI import BRepPrimAPI_MakeCylinder
    from OCP.gp import gp_Ax2, gp_Dir
    point, d = apply_point(o, axis["point_mm"]), norm(turn(o, axis["direction"]))
    base = gp_Ax2(gp_Pnt(*add(point, d, lo)), gp_Dir(*d))
    outer = BRepPrimAPI_MakeCylinder(base, r1, hi - lo).Shape()
    return outer if r0 <= 1e-9 else without(outer, BRepPrimAPI_MakeCylinder(base, r0, hi - lo).Shape())


def fo_shell(o, q, d):
    """A conical seat's shell: the cone over its radii widened by d, swept d either side of the surface along its
    normal, revolved from a quadrilateral in a plane through the axis (the target's frame, placed by o)."""
    from OCP.BRepBuilderAPI import BRepBuilderAPI_MakeFace, BRepBuilderAPI_MakePolygon
    from OCP.BRepPrimAPI import BRepPrimAPI_MakeRevol
    from OCP.gp import gp_Ax1, gp_Dir
    apex, w = q["apex_mm"], norm(q["axis"]["direction"])
    if dot(sub(q["axis"]["point_mm"], apex), w) < 0:
        w = tuple(-c for c in w)
    a = q["included_angle_rad"] / 2
    e = norm(cross(w, [1.0, 0.0, 0.0] if abs(w[0]) < 0.9 else [0.0, 1.0, 0.0]))
    n = [math.cos(a) * e[i] - math.sin(a) * w[i] for i in range(3)]
    corners = []
    for r, side in [(min(q["radii_mm"]) - d, -1), (max(q["radii_mm"]) + d, -1), (max(q["radii_mm"]) + d, 1),
                    (min(q["radii_mm"]) - d, 1)]:
        on = [apex[i] + w[i] * r / math.tan(a) + e[i] * r for i in range(3)]
        corners.append(apply_point(o, [on[i] + side * d * n[i] for i in range(3)]))
    poly = BRepBuilderAPI_MakePolygon()
    for c in corners:
        poly.Add(gp_Pnt(*c))
    poly.Close()
    face = BRepBuilderAPI_MakeFace(poly.Wire()).Face()
    return BRepPrimAPI_MakeRevol(face, gp_Ax1(gp_Pnt(*apply_point(o, apex)), gp_Dir(*turn(o, w)))).Shape()


def fo_span_zones(scene, o, q, sources, body, d):
    """FZ3's annuli as the oracle reads them: each tight source naming `body` over a zoned span."""
    out = []
    own_key = "hole_diameter_mm" if q["kind"] == "bore" else "shaft_diameter_mm"
    sense = -1.0 if q["kind"] == "bore" else 1.0
    for span in q.get("spans", []):
        if not (span["fit"] in ("interference", "line_to_line") or (span["fit"] == "not_read" and span["pinned"])):
            continue
        own = span[own_key] / 2
        for s in sources:
            if s["rule"] not in FO_TIGHT or not s.get("span_mm") or body not in scene.body(s["neighbour"]):
                continue
            lo, hi = max(span["from_mm"], min(s["span_mm"])), min(span["to_mm"], max(s["span_mm"]))
            if hi - lo <= 1e-9:
                continue
            theirs = own + sense * (s.get("margin_mm") or 0.0)
            out.append(fo_annulus(o, q["axis"], lo - d, hi + d, min(own, theirs) - d, max(own, theirs) + d))
    return out


def fo_zones(scene, occurrence, body, d):
    """FO1: the zones the oracle builds itself from fitment's socket document (never from fitment's zones) for
    the candidate at `occurrence` against `body` (a leaf, or a copy at another occurrence), at depth d: each seat
    face a plane-contact source (C1, C5) names on that body swept d either side; each tight source's annulus over
    a zoned span (interference, line to line, a pinned unread occupant), radii widened by d; each conical seat's
    shell, d either side of its cone. At a depth of 0 or less, none (FZ6: a pair whose uncertainty leaves no
    tolerance has no zone)."""
    out = []
    if d <= 0:
        return out
    o = scene.at[occurrence][1]
    for requirement, sources in scene.sources(occurrence):
        q = requirement["requires"]
        if q["kind"] == "seat":
            for s in sources:
                if s["rule"].startswith("C") and body in scene.body(s["neighbour"]):
                    for face in s["faces"]:
                        f = scene.face(body, face)
                        slab = fo_slab(f, d) if f is not None else None
                        if slab is not None:
                            out.append(slab)
        elif q["kind"] in ("bore", "shaft"):
            out += fo_span_zones(scene, o, q, sources, body, d)
        elif q["kind"] == "conical_seat" and q.get("radii_mm"):
            if any(body in scene.body(s["neighbour"]) for s in sources if s["neighbour"]):
                out.append(fo_shell(o, q, d))
    return out


# How many pairs had a Common the oracle's zones excused: shown, so a run whose zones excuse nothing is seen.
FO_EXCUSED = {"pairs": 0}


def fo_residual(candidate, other, zones):
    """The Common of two placed shapes less `zones`: its volume (0 for boxes apart)."""
    if apart(candidate, other, 1e-6):
        return 0.0
    v = common(candidate, other)
    if volume_of(v) <= VOLUME:
        return volume_of(v)
    for z in zones:
        v = without(v, z)
    residual = volume_of(v)
    if residual <= VOLUME:
        FO_EXCUSED["pairs"] += 1
    return residual


def fo_placed(scene, candidate, placements, turns=None):
    """The candidate at every occurrence: o · R · A, A its socket's placement, R a turn (or none) in the
    target's frame."""
    out = {}
    for occ, o, _ in scene.occurrences:
        a = placements[occ["socket"]]
        if turns and occ["socket"] in turns:
            a = compose(turns[occ["socket"]], a)
        out[tuple(occ["path"])] = moved(candidate, compose(o, a))
    return out


def fo_pairs(scene, placed):
    """Every pair FT3 checks: (occurrence, body key, the body's placed shape)."""
    for occurrence, shape in placed.items():
        for i in scene.neighbours():
            yield occurrence, ("leaf", i), shape, scene.leaves[i][2]
        for other, copy in placed.items():
            if other != occurrence:
                yield occurrence, ("copy", other), shape, copy


def excused_turns(entry):
    """Per socket, the axes of the turns FT10 excused (`axisymmetric`), in the target's frame."""
    out = {}
    for result in entry["sockets"]:
        for motion in (result.get("placement") or {}).get("freedom") or []:
            if motion["kind"] == "turn" and motion["axisymmetric"]:
                out.setdefault(result["socket"], []).append(motion["about"])
    return out


def fo_matches(scene, entry, candidate, d, tally):
    """FO1: every `Match`, physically: the candidate at every occurrence by its socket's placement, against every
    other leaf and its own copies, has no common volume outside the oracle's own zones (fo_zones) at depth t;
    where FT10 excused a free turn, the same at FO_TURNS turns about its axis."""
    placements = {r["socket"]: placement_rows(r["placement"]) for r in entry["sockets"]}
    axes = excused_turns(entry)
    variants = [None] + [{s: rotation_about(a[0][0], a[0][1], 360.0 * k / FO_TURNS) for s, a in axes.items()}
                         for k in range(1, FO_TURNS)] if axes else [None]
    problems = []
    for turns in variants:
        tally["fo1 placements"] += 1
        for occurrence, body, shape, other in fo_pairs(scene, fo_placed(scene, candidate, placements, turns)):
            tally["fo1 pairs"] += 1
            residual = fo_residual(shape, other, fo_zones(scene, occurrence, body, d))
            if residual > VOLUME:
                problems.append(f"{entry['labels'][0]}: a Match with {residual:.6g} mm³ in {body} at "
                                f"{list(occurrence)} outside every zone{' turned' if turns else ''} (FO1)")
                return problems
    return problems


def fo_interferences(scene, entry, candidate, tally):
    """FO2: each interference a document states, re-measured: the candidate placed as the document says, the
    named body, the Common less the oracle's zones at the pair's own epsilon exceeds 1e-6 mm³ and agrees with
    fitment's residual within FO_AGREE_REL relative and FO_AGREE_ABS."""
    placements = {r["socket"]: placement_rows(r["placement"]) for r in entry["sockets"]}
    placed = fo_placed(scene, candidate, placements)
    problems = []
    for pair in entry["fit"]["interferences"]:
        occurrence = tuple(pair["occurrence"])
        bodies = scene.body(pair["body"]["path"])
        if pair["body"]["kind"] == "copy":
            bodies = [("copy", tuple(pair["body"]["path"]))]
        if occurrence not in placed or not bodies:
            problems.append(f"{entry['labels'][0]}: an interference names {pair} the oracle cannot bind (FO2)")
            continue
        body = bodies[0]
        other = placed[body[1]] if body[0] == "copy" else scene.leaves[body[1]][2]
        residual = fo_residual(placed[occurrence], other, fo_zones(scene, occurrence, body, pair["epsilon_mm"]))
        stated = pair["measured"]["residual_mm3"]
        tally["fo2 re-measured"] += 1
        diff = abs(residual - stated)
        tally["fo2 largest difference mm3"] = max(tally["fo2 largest difference mm3"], diff)
        if residual <= VOLUME or diff > FO_AGREE_REL * stated + FO_AGREE_ABS:
            problems.append(f"{entry['labels'][0]}: an interference with {pair['body']} at {list(occurrence)} "
                            f"re-measured {residual:.9g} mm³, fitment {stated:.9g} (FO2)")
    return problems


def epsilons(value):
    """Every `epsilon_mm` a document's fit states, anywhere in it."""
    if isinstance(value, dict):
        for k, v in value.items():
            if k == "epsilon_mm" and isinstance(v, (int, float)):
                yield v
            else:
                yield from epsilons(v)
    elif isinstance(value, list):
        for v in value:
            yield from epsilons(v)


def fo_collisions(scene, case, entry, candidate, t, tally):
    """FO2's construction half: a `collides` case collides at its construction's placement (each occurrence's
    transform after the case's placement), some pair's Common less the oracle's zones above 1e-6 mm³ at the
    least epsilon its document states; where FK3 states a free turn, at FO_TURNS turns about the free axis:
    at every one for an axisymmetric candidate, at some for another."""
    d = min(epsilons(entry["fit"]), default=t - 1e-6)
    a = case["placement"]
    freedom = case.get("freedom", "unique")
    axis = next((m["about"] for r in entry["sockets"] for m in (r.get("placement") or {}).get("freedom") or []
                 if m["kind"] == "turn"), None)
    if axis is None and freedom.startswith("turn"):
        axis = first_axis(scene.socket, entry["sockets"][0])
    turns = [0.0] + ([360.0 * k / FO_TURNS for k in range(1, FO_TURNS)] if freedom.startswith("turn") else [])
    found = []
    for degrees in turns:
        r = rotation_about(axis[0], axis[1], degrees) if degrees else None
        placed = {tuple(occ["path"]): moved(candidate, compose(o, compose(r, a) if r else a))
                  for occ, o, _ in scene.occurrences}
        tally["fo2 construction placements"] += 1
        found.append(any(fo_residual(shape, other, fo_zones(scene, occ, body, d)) > VOLUME
                         for occ, body, shape, other in fo_pairs(scene, placed)))
    want = all(found) if freedom == "turn_axisymmetric" else any(found)
    if not want:
        return [f"{case['id']}: labelled collides, and it collides at {sum(found)} of {len(found)} placements "
                f"(FK3: {freedom}) (FO2)"]
    return []


def freedom_statement(freedom):
    """FK3's statement of a placement's freedom (FC4's motions)."""
    if not freedom:
        return "unique"
    if all(m["kind"] == "turn" and m["axisymmetric"] for m in freedom):
        return "turn_axisymmetric"
    if any(m["kind"] == "slide" for m in freedom):
        return "slide"
    return "turn"


def fo_freedom(entry, case):
    """FK3 on a default-policy document: each socket's stated freedom is the one the manifest states for the
    case (a freedom not worked out, null, is not judged)."""
    stated = case.get("freedom")
    out = []
    for result in entry["sockets"]:
        freedom = (result.get("placement") or {}).get("freedom")
        if stated is None or freedom is None:
            continue
        if freedom_statement(freedom) != stated:
            out.append(f"{case['id']} {result['socket']}: the document's freedom is "
                       f"{freedom_statement(freedom)}, its case states {stated} (FK3)")
    return out


def fo_turned_match(entry):
    """`entry` as a `Match` with a verified fit, its placements as stated."""
    import copy
    out = copy.deepcopy(entry)
    out["verdict"] = "match"
    out["fit"] = {"status": "verified", "kernel": "open_cascade", "version": "8.0.1", "pairs": [],
                  "scope_not_checked": []}
    return out


def mutate_interferes(entry, case):
    return fo_turned_match(entry) if entry["fit"]["status"] == "interferes" else None


def mutate_collides(entry, case):
    if case is None or case["truth"] != "collides":
        return None
    return entry if entry["verdict"] == "match" else fo_turned_match(entry)


def mutate_freedom(entry, case):
    import copy
    for k, result in enumerate(entry["sockets"]):
        if (result.get("placement") or {}).get("freedom"):
            out = copy.deepcopy(entry)
            out["sockets"][k]["placement"]["freedom"].pop()
            return out
    return None


# FO3: each mutation of a v2 document must meet a disagreement somewhere in the corpus: FO1 meets the first two,
# FK3's freedom check the third.
FIT_MUTATIONS = [
    ("an interferes turned verified", mutate_interferes, "match"),
    ("a verified on a collides case", mutate_collides, "match"),
    ("a freedom entry dropped from a placement", mutate_freedom, "freedom"),
]


def fo_mutations(scene, doc, cases, solid, met, tally):
    """FO3: KO6's mutations for v2 documents (FIT_MUTATIONS), each tried on this document's entries until one
    meets a disagreement; `met` collects the names met."""
    default = doc["policy"]["clearance"] == "strict"
    for name, mutate, check in FIT_MUTATIONS:
        if name in met:
            continue
        for entry in doc["candidates"]:
            if entry["source"]["kind"] != "file":
                continue
            case = cases.get(entry["labels"][0])
            mutated = mutate(entry, case)
            if mutated is None:
                continue
            tally["fo3 mutations tried"] += 1
            if check == "match":
                found = fo_matches(scene, mutated, solid(entry), doc["policy"]["linear_mm"], dict(tally))
            else:
                found = fo_freedom(mutated, case) if default and case else []
            if found:
                met.add(name)
                break


def fo_document(directory, scene, doc, cases, solid, tally):
    """FO1, FO2 and FK3 on one v2 document: every file entry's Match physically, each interference
    re-measured, and under the default policy each placement's freedom as its case states."""
    problems = []
    t = doc["policy"]["linear_mm"]
    for entry in doc["candidates"]:
        if entry["source"]["kind"] != "file":
            continue
        if entry["verdict"] == "match":
            tally["fo1 matches"] += 1
            problems += fo_matches(scene, entry, solid(entry), t, tally)
        if entry["fit"]["status"] == "interferes":
            problems += fo_interferences(scene, entry, solid(entry), tally)
        case = cases.get(entry["labels"][0])
        if doc["policy"]["clearance"] == "strict" and case is not None:
            tally["fk3 freedoms"] += 1
            problems += fo_freedom(entry, case)
    return problems


def fo_scene(binary, directory, key, scratch):
    """The socket of one index key (source, part, review), as fitment writes it and its review leaves it, with
    Open CASCADE's leaves and fitment's paths: a FitScene, or the problem."""
    source, part, review = key
    path = scratch / f"{source}--{part}--{hashlib.sha256(review.encode()).hexdigest()[:12]}.json"
    socket, problem = run_fitment(binary, ["socket", str(directory / f"{source}.stp"), "--part", part,
                                           "--format", "json"], (0,))
    if problem:
        return None, problem
    path.write_text(json.dumps(socket), encoding="utf-8")
    if json.loads(review):
        socket, problem = reviewed_socket(binary, socket, path, json.loads(review))
        if problem:
            return None, problem
    read = xcaf_leaves(directory / f"{source}.stp", bind=True)
    paths, problem = parts_paths(binary, directory / f"{source}.stp")
    if read is None or problem:
        return None, problem or "Open CASCADE cannot read it"
    scene = FitScene(socket, read[0], read[1], paths)
    return scene, "; ".join(scene.problems) or None


def fo_group(binary, directory, key, docs, cases, met, tally, scratch):
    """One socket's v2 documents (each policy): FO1, FO2, FK3 and FO3's mutations on each, and FO2's
    construction half on each of its `collides` cases."""
    scene, problem = fo_scene(binary, directory, key, scratch)
    if scene is None or problem:
        return [f"{key[0]} {key[1]}: {problem}"]
    solids = {}

    def solid(entry):
        label = entry["labels"][0]
        if label not in solids:
            solids[label] = one_solid(directory / label)
        return solids[label]

    problems = []
    try:
        for doc in docs:
            tally["documents"] += 1
            problems += fo_document(directory, scene, doc, cases, solid, tally)
            fo_mutations(scene, doc, cases, solid, met, tally)
        default = next((d for d in docs if d["policy"]["clearance"] == "strict"), None)
        for label, case in sorted(cases.items()):
            entry = next((e for e in default["candidates"] if label in e["labels"]), None) if default else None
            if case["truth"] == "collides" and entry is not None:
                tally["fo2 collides cases"] += 1
                problems += fo_collisions(scene, case, entry, solid(entry), default["policy"]["linear_mm"], tally)
    except BooleanFailed as failed:
        problems.append(f"{key[0]} {key[1]}: {failed} failed in Open CASCADE")
    return problems


def fit_main(argv):
    """`fit DIR DOCS` (VT72): FO1 to FO3 on every fitment.match v2 document the fit corpus suite wrote into DOCS
    (`crates/fitment/tests/fit_corpus.rs`: one per socket and policy, and `index.json` naming each one's source,
    part, review and policy), against the corpus in DIR: every `Match` fits physically outside the oracle's own
    zones at depth t, and at FO_TURNS turns where a free turn was excused (FO1); every `interferes` re-measured
    at the pair's own epsilon (FO2), and every `collides` case colliding at its construction's placement (and its
    free turns, FK3); each placement's freedom as its case states (FK3); and each of FIT_MUTATIONS meets a
    disagreement somewhere (FO3)."""
    import tempfile
    directory, docs_dir = Path(argv[0]), Path(argv[1])
    binary = Path(os.environ.get("FITMENT", Path(__file__).resolve().parent.parent / "target/release/fitment"))
    if not binary.is_file():
        sys.exit(f"no fitment at {binary}: run cargo build --release -p fitment, or set FITMENT")
    cases = [c for c in json.loads((directory / "manifest.json").read_text(encoding="utf-8"))["cases"]
             if c.get("kind") == "match" and "file" in c["candidate"]]
    groups = {}
    for item in json.loads((docs_dir / "index.json").read_text(encoding="utf-8")):
        s_ = item["socket"]
        key = (s_["source"], s_["part"], json.dumps(s_["review"], sort_keys=True))
        groups.setdefault(key, []).append(json.loads((docs_dir / item["file"]).read_text(encoding="utf-8")))
    tally = {"documents": 0, "fo1 matches": 0, "fo1 placements": 0, "fo1 pairs": 0, "fo2 re-measured": 0,
             "fo2 largest difference mm3": 0.0, "fo2 collides cases": 0, "fo2 construction placements": 0,
             "fk3 freedoms": 0, "fo3 mutations tried": 0}
    problems, met = [], set()
    with tempfile.TemporaryDirectory(prefix="fitment-fit-oracle-") as scratch:
        for key, docs in sorted(groups.items()):
            mine = {c["candidate"]["file"]: c for c in cases if c["socket"]["case"] == key[0]
                    and c["socket"]["part"] == key[1]
                    and json.dumps(c["socket"].get("review") or [], sort_keys=True) == key[2]}
            before = len(problems)
            problems += fo_group(binary, directory, key, docs, mine, met, tally, Path(scratch))
            print(f"{'ok  ' if len(problems) == before else 'FAIL'} {key[0]} {key[1]}"
                  f"{' reviewed' if json.loads(key[2]) else ''}")
    tally["pairs a zone excused"] = FO_EXCUSED["pairs"]
    unmet = [name for name, _, _ in FIT_MUTATIONS if name not in met]
    if unmet:
        problems.append(f"mutations no check disagreed with anywhere (FO3): {unmet}")
    if any(v == 0 for v in tally.values() if isinstance(v, int)):
        problems.append("a fit check examined nothing: " + ", ".join(f"{v} {k}" for k, v in tally.items()))
    for problem in problems:
        print(problem, file=sys.stderr)
    print(", ".join(f"{v:.3g} {k}" if isinstance(v, float) else f"{v} {k}" for k, v in tally.items())
          + f"; {len(problems)} disagreements with Open CASCADE")
    sys.exit(1 if problems else 0)


def main():
    if len(sys.argv) == 3 and sys.argv[1] == "mutations":
        quiet()
        mutations_main(sys.argv[2:])
    if len(sys.argv) >= 3 and sys.argv[1] == "socket":
        quiet()
        socket_main(sys.argv[2:])
    if len(sys.argv) >= 3 and sys.argv[1] == "match":
        quiet()
        match_main(sys.argv[2:])
    if len(sys.argv) == 4 and sys.argv[1] == "fit":
        quiet()
        fit_main(sys.argv[2:])
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
    if any(c.get("kind") == "match" for c in manifest["cases"]) and not KO1_TALLY:
        problems.append("KO1 judged no match case")
    for problem in problems:
        print(problem, file=sys.stderr)
    if KO1_TALLY:
        print("KO1 judged " + ", ".join(f"{v} {k}" for k, v in sorted(KO1_TALLY.items())))
    print(f"{len(manifest['cases'])} cases, {len(problems)} disagreements with Open CASCADE")
    sys.exit(1 if problems else 0)


if __name__ == "__main__":
    main()
