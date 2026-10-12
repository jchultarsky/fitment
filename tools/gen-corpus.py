#!/usr/bin/env -S uv run --locked --script
# /// script
# requires-python = ">=3.11,<3.14"
# dependencies = ["cadquery==2.8.0"]
# ///
"""Write fitment's test parts and the answers they must read as.

    tools/gen-corpus.py OUT_DIR              every case: the generated corpus
    tools/gen-corpus.py --unit-set OUT_DIR   only the committed unit set
    tools/gen-corpus.py --assembly-set OUT_DIR   only the committed assemblies

Each part is built from explicit primitives (boxes, cylinders, cones, tori
from fillets, one prism), a few through documented text edits of a written
file, and written by Open CASCADE 7.9.3 (cadquery 2.8.0). OUT_DIR gets one
.stp per case and manifest.json, whose expected answers come only from the
construction parameters below, never from reading a written file back.
docs/research/m2-features.md, section 13, is the design; its section 9 is
the convention the answers are stated in:

  - a hole's axis points from its bottom to its opening when blind, and
    with its largest component positive (ties x, y, z) otherwise;
  - `axis_point` is the axis point at the start end, and every station is
    a distance from it along `axis_direction`;
  - a segment is one cylinder: [diameter, from, to] in mm.

Files are written without pcurves (fitment reads 3D curves; pcurves make
them 2.4 times larger) and with a fixed FILE_NAME time stamp, so the same
script always writes the same bytes on one platform. In the corpus only,
each positive part built from a shape is written again with pcurves
(`_pc`) and, where its answers follow from its construction alone, moved
into a second skew frame (`_rot`); parts whose answers are patched by
hand (p32, p33) and text edits (p20, p29) have no `_rot`, and text edits
no `_pc`. The unit set lives in
crates/fitment-core/tests/data; CI regenerates it and diffs it against that
copy.
"""

from __future__ import annotations

import argparse
import importlib.metadata
import json
import math
import re
from dataclasses import dataclass, field
from pathlib import Path

import cadquery as cq
from OCP.BRep import BRep_Builder
from OCP.BRepAdaptor import BRepAdaptor_Curve
from OCP.BRepAlgoAPI import BRepAlgoAPI_Cut, BRepAlgoAPI_Fuse
from OCP.BRepBuilderAPI import BRepBuilderAPI_NurbsConvert, BRepBuilderAPI_Transform
from OCP.BRepFilletAPI import BRepFilletAPI_MakeFillet
from OCP.BRepPrimAPI import (
    BRepPrimAPI_MakeBox,
    BRepPrimAPI_MakeCone,
    BRepPrimAPI_MakeCylinder,
    BRepPrimAPI_MakeSphere,
)
from OCP.GeomAbs import GeomAbs_Circle
from OCP.gp import gp_Ax1, gp_Ax2, gp_Dir, gp_Pnt, gp_Trsf, gp_Vec
from OCP.IFSelect import IFSelect_RetDone
from OCP.Interface import Interface_Static
from OCP.Message import Message
from OCP.ShapeUpgrade import ShapeUpgrade_ShapeDivideAngle, ShapeUpgrade_ShapeDivideClosed
from OCP.STEPControl import STEPControl_AsIs, STEPControl_Writer
from OCP.TopAbs import TopAbs_EDGE, TopAbs_SHELL
from OCP.TopExp import TopExp_Explorer
from OCP.TopoDS import TopoDS, TopoDS_Compound

TIME_STAMP = "2026-01-01T00:00:00"
DIGITS = 9  # expected values are rounded here: 5e-10 mm, far below any test tolerance

# ---------------------------------------------------------------- geometry


def add(p, d, s=1.0):
    return tuple(a + s * b for a, b in zip(p, d))


def dot(a, b):
    return sum(x * y for x, y in zip(a, b))


def cross(a, b):
    return (a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0])


def unit(v):
    n = math.sqrt(dot(v, v))
    return tuple(x / n for x in v)


def canonical(d):
    """The sign the design gives an undirected axis: the largest absolute
    component positive, ties broken in the order x, y, z."""
    i = max(range(3), key=lambda k: (abs(d[k]), -k))
    return d if d[i] > 0 else tuple(-x for x in d)


@dataclass(frozen=True)
class Motion:
    """A rigid motion p -> R p + t, with R a rotation by `angle` degrees
    about `axis`, applied to construction parameters and to shapes alike."""

    axis: tuple
    angle: float
    shift: tuple

    def vector(self, v):
        k, th = unit(self.axis), math.radians(self.angle)
        c, s = math.cos(th), math.sin(th)
        kv, kxv = dot(k, v), cross(k, v)
        return tuple(v[i] * c + kxv[i] * s + k[i] * kv * (1 - c) for i in range(3))

    def point(self, p):
        return add(self.vector(p), self.shift)

    def trsf(self):
        r = gp_Trsf()
        r.SetRotation(gp_Ax1(gp_Pnt(0, 0, 0), gp_Dir(*unit(self.axis))), math.radians(self.angle))
        t = gp_Trsf()
        t.SetTranslation(gp_Vec(*self.shift))
        return t.Multiplied(r)


IDENTITY = Motion((0, 0, 1), 0.0, (0, 0, 0))
# The frame motion of p14 and p14s: no axis of the result is axis-aligned.
TILT = Motion((1, 2, 3), 37.0, (200.0, -40.0, 17.0))
# The motion of the `_rot` variants: a second skew frame, turned far enough
# (140 degrees) that holes along y and z come out with the other canonical
# sign, so the variants test that fitment re-chooses it.
VARIANT = Motion((2, -1, 1), 140.0, (-35.0, 12.0, 60.0))
# The mapped item of p29: a quarter turn about z, then a shift.
MAPPED = Motion((0, 0, 1), 90.0, (100.0, -50.0, 25.0))

# --------------------------------------------------------------- expected


@dataclass
class Hole:
    """A hole as constructed: it enters at `entry` and goes `into` the
    material. `segments` are (diameter, depth) from the entry, outermost
    first, the last one ending at `length` for a through hole."""

    entry: tuple
    into: tuple
    segments: list
    through: float | None = None  # the material length on the axis
    point_deg: float | None = None  # a drill point's included angle
    flat_after_point: float | None = None  # the diameter of a flat below a point
    countersink: tuple | None = None  # (diameter at the entry, included angle)
    post_diameter: float | None = None  # a post rising from a blind hole's floor
    floor_below: float = 0.0  # how far below the wall's end the floor is (a floor fillet)
    rim_end: str = "opening"  # how the entry end reads: opening, unknown
    far_end: str | None = None  # how the far end reads, if not by its kind
    kind: str | None = None  # forced kind, such as "unknown"
    reason: str | None = None  # why an unknown end is unknown, as fitment names it

    def moved(self, m: Motion):
        return Hole(**{**self.__dict__, "entry": m.point(self.entry), "into": m.vector(self.into)})

    def expect(self):
        if self.kind == "unknown":
            # Only the axis line and diameter are stated; ends say why.
            reason = {"unknown_reason": self.reason} if self.reason else {}
            return {
                "kind": "unknown",
                **reason,
                "axis_line_point": r3(self.entry),
                "axis_direction": r3(canonical(self.into)),
                "diameter": r(self.segments[-1][0]),
                # In axis order, like every other hole's ends.
                "ends": [self.rim_end, self.far_end or "unknown"] if canonical(self.into) == self.into
                else [self.far_end or "unknown", self.rim_end],
            }
        cone_depth = 0.0
        if self.countersink is not None:
            d0, angle = self.countersink
            cone_depth = (d0 - self.segments[0][0]) / 2 / math.tan(math.radians(angle / 2))
        spans, prev = [], 0.0
        for i, (d, depth) in enumerate(self.segments):
            last = i == len(self.segments) - 1
            to = self.through if last and self.through is not None else depth
            spans.append((d, cone_depth if i == 0 else prev, to))
            prev = to
        if self.through is not None:
            length = self.through
            direction = canonical(self.into)
            if direction == self.into:
                start, stations = self.entry, [(d, a, b) for d, a, b in spans]
                ends = [self.end_at(0.0, "entry"), self.end_at(length, "far")]
            else:
                start = add(self.entry, self.into, length)
                stations = [(d, length - b, length - a) for d, a, b in reversed(spans)]
                ends = [self.end_at(0.0, "far"), self.end_at(length, "entry")]
            hole = {"kind": "through", "length": r(length)}
        else:
            depth = spans[-1][2]
            direction = tuple(-x for x in self.into)
            start = add(self.entry, self.into, depth)
            stations = [(d, depth - b, depth - a) for d, a, b in reversed(spans)]
            ends = [self.bottom(), self.end_at(depth, "entry")]
            hole = {"kind": "blind", "depth": r(depth)}
        return {
            **hole,
            "axis_point": r3(start),
            "axis_direction": r3(direction),
            "segments": [[r(d), r(a), r(b)] for d, a, b in stations],
            "ends": ends,
        }

    def end_at(self, at, which):
        end = {"kind": "opening", "rim": r(at)}
        if which == "entry" and self.countersink is not None:
            end["countersink"] = {"outer_diameter": r(self.countersink[0]), "angle_deg": r(self.countersink[1])}
        return end

    def bottom(self):
        d = self.segments[-1][0]
        if self.post_diameter is not None:
            return {"kind": "bottom", "bottom": "annular", "at": 0.0, "post_diameter": r(self.post_diameter),
                    "floor_at": r(-self.floor_below)}
        if self.point_deg is None:
            return {"kind": "bottom", "bottom": "flat", "at": 0.0}
        half = math.radians(self.point_deg / 2)
        if self.flat_after_point is not None:
            floor = (d - self.flat_after_point) / 2 / math.tan(half)
            return {"kind": "bottom", "bottom": "cone_then_flat", "at": 0.0,
                    "angle_deg": r(self.point_deg), "floor_at": r(-floor)}
        return {"kind": "bottom", "bottom": "point", "at": 0.0, "angle_deg": r(self.point_deg),
                "tip_length": r(d / 2 / math.tan(half))}


@dataclass
class Shaft:
    """A shaft from its `root` along `out`; `free_free` for a pin, whose
    root is just one end."""

    root: tuple
    out: tuple
    diameter: float
    length: float  # the cylinder's own length
    protrusion: float | None = None  # root rim to free rim, when it differs
    kind: str = "root_free"

    def moved(self, m: Motion):
        return Shaft(**{**self.__dict__, "root": m.point(self.root), "out": m.vector(self.out)})

    def expect(self):
        out, start = self.out, self.root
        if self.kind == "free_free" and canonical(out) != out:
            start, out = add(self.root, self.out, self.length), tuple(-x for x in out)
        e = {"kind": self.kind, "axis_point": r3(start), "axis_direction": r3(out),
             "segments": [[r(self.diameter), 0.0, r(self.length)]]}
        e["length" if self.kind == "free_free" else "protrusion"] = r(self.protrusion or self.length)
        return e


@dataclass
class Partial:
    point: tuple
    axis: tuple
    diameter: float
    sweep_deg: float
    concave: bool = True

    def moved(self, m: Motion):
        return Partial(m.point(self.point), m.vector(self.axis), self.diameter, self.sweep_deg, self.concave)

    def expect(self):
        return {"axis_line_point": r3(self.point), "axis_direction": r3(canonical(self.axis)),
                "diameter": r(self.diameter), "concave": self.concave, "sweep_deg": r(self.sweep_deg)}


@dataclass
class Plane:
    normal: tuple
    offset: float
    faces: int = 1
    shell: int = 0

    def moved(self, m: Motion):
        n = m.vector(self.normal)
        return Plane(n, self.offset + dot(n, m.shift), self.faces, self.shell)

    def expect(self):
        return {"normal": r3(self.normal), "offset": r(self.offset), "faces": self.faces, "shell": self.shell}


def box_planes(x0, y0, z0, dx, dy, dz, top_faces=1):
    return [
        Plane((0, 0, 1), z0 + dz, top_faces), Plane((0, 0, -1), -z0),
        Plane((1, 0, 0), x0 + dx), Plane((-1, 0, 0), -x0),
        Plane((0, 1, 0), y0 + dy), Plane((0, -1, 0), -y0),
    ]


def r(x):
    x = round(float(x), DIGITS)
    return 0.0 if x == 0 else x


def r3(v):
    return [r(x) for x in v]


@dataclass
class Features:
    """Everything a part's faces are: so that a stray face, such as one a
    cutting tool left behind, is always a disagreement."""

    holes: list = field(default_factory=list)
    shafts: list = field(default_factory=list)
    partials: list = field(default_factory=list)
    planes: list = field(default_factory=list)
    cones: int = 0  # distinct conical surfaces
    tori: int = 0  # distinct toroidal surfaces
    other_faces: int = 0  # faces on spheres and unread surfaces

    def moved(self, m: Motion):
        return Features([h.moved(m) for h in self.holes], [s.moved(m) for s in self.shafts],
                        [p.moved(m) for p in self.partials], [p.moved(m) for p in self.planes],
                        self.cones, self.tori, self.other_faces)

    def expect(self):
        return {"holes": [h.expect() for h in self.holes], "shafts": [s.expect() for s in self.shafts],
                "partial_cylinders": [p.expect() for p in self.partials],
                "planes": [p.expect() for p in self.planes],
                "cones": self.cones, "tori": self.tori, "other_faces": self.other_faces}


# ---------------------------------------------------------------- shapes


def box(x0, y0, z0, dx, dy, dz):
    return BRepPrimAPI_MakeBox(gp_Pnt(x0, y0, z0), dx, dy, dz).Shape()


def cyl(p, d, radius, h):
    return BRepPrimAPI_MakeCylinder(gp_Ax2(gp_Pnt(*p), gp_Dir(*d)), radius, h).Shape()


def cone(p, d, r1, r2, h):
    return BRepPrimAPI_MakeCone(gp_Ax2(gp_Pnt(*p), gp_Dir(*d)), r1, r2, h).Shape()


def cut(a, *tools):
    for b in tools:
        a = BRepAlgoAPI_Cut(a, b).Shape()
    return a


def fuse(a, *more):
    for b in more:
        a = BRepAlgoAPI_Fuse(a, b).Shape()
    return a


def compound(*shapes):
    c, b = TopoDS_Compound(), BRep_Builder()
    b.MakeCompound(c)
    for s in shapes:
        b.Add(c, s)
    return c


def tool(h: Hole, overshoot=1.0):
    """The cutter for hole `h`: it overshoots the entry by `overshoot`, and
    a through hole's last segment overshoots the far face too."""
    start = add(h.entry, h.into, -overshoot)
    cutter = None
    for i, (d, depth) in enumerate(h.segments):
        last = i == len(h.segments) - 1
        reach = (h.through + overshoot) if last and h.through is not None else depth
        piece = cyl(start, h.into, d / 2, reach + overshoot)
        cutter = piece if cutter is None else fuse(cutter, piece)
    d, depth = h.segments[-1]
    if h.point_deg is not None:
        half = math.radians(h.point_deg / 2)
        if h.flat_after_point is None:
            cutter = fuse(cutter, cone(add(h.entry, h.into, depth), h.into, d / 2, 0.0, d / 2 / math.tan(half)))
        else:
            hh = (d - h.flat_after_point) / 2 / math.tan(half)
            cutter = fuse(cutter, cone(add(h.entry, h.into, depth), h.into, d / 2, h.flat_after_point / 2, hh))
    if h.countersink is not None:
        d0, angle = h.countersink
        hh = d0 / 2 / math.tan(math.radians(angle / 2))
        cutter = fuse(cutter, cone(h.entry, h.into, d0 / 2, 0.0, hh), cyl(start, h.into, d0 / 2, overshoot))
    return cutter


def round_edges(shape, radius, pick):
    """`shape` with every circular edge `pick(circle)` accepts filleted."""
    fillet = BRepFilletAPI_MakeFillet(shape)
    explorer = TopExp_Explorer(shape, TopAbs_EDGE)
    seen = 0
    while explorer.More():
        edge = TopoDS.Edge_s(explorer.Current())
        curve = BRepAdaptor_Curve(edge)
        if curve.GetType() == GeomAbs_Circle and pick(curve.Circle()):
            fillet.Add(radius, edge)
            seen += 1
        explorer.Next()
    if seen == 0:
        raise RuntimeError("round_edges: no edge picked")
    return fillet.Shape()


def revolved_cut(shape, centre, profile):
    """`shape` minus the solid `profile` sweeps about the vertical axis
    through `centre` (x, y). `profile` is cadquery drawing steps on the XZ
    plane, in (radius, z): a cut by a revolved tool states its tori
    consistently, where Open CASCADE 7.9 writes some filleted tori inside
    out (the torus's loop, as its FACE_BOUND and same_sense state it, runs
    the same way as both its neighbours along their shared circles), which
    fitment rightly refuses as a solid that is not closed."""
    tool = profile(cq.Workplane("XZ")).close().revolve(360, (0, 0, 0), (0, 1, 0))
    return cut(shape, tool.translate((centre[0], centre[1], 0)).val().wrapped)


def arc_mid(centre, radius, degrees):
    """The point of the arc about `centre` (r, z) at `degrees`."""
    a = math.radians(degrees)
    return (centre[0] + radius * math.cos(a), centre[1] + radius * math.sin(a))


def circle_at(z, radius):
    return lambda c: abs(c.Location().Z() - z) < 1e-9 and abs(c.Radius() - radius) < 1e-9


DOWN = (0, 0, -1)
UP = (0, 0, 1)

# ------------------------------------------------------------- coverage

CORE = "crates/fitment-core/tests"
SRC = "crates/fitment-core/src/features"

# Every rule and gate check the design (docs/research/m2-features.md)
# defines, and what covers it beyond the cases that cite it in their
# `rules`: Rust tests, named as file::function, for what no generated part
# can reach, or the reason it is deferred. crates/fitment/tests/corpus.rs
# fails if an id has no case, no test and no reason, if a test it names is
# not there, or if a case cites an id missing here.
RULES = {
    "4.2#1": {"tests": [f"{CORE}/features.rs::a_file_that_is_not_step_is_unparseable"]},
    "4.2#2": {"tests": [f"{CORE}/features.rs::a_dangling_reference_is_refused_with_its_instance"]},
    "4.2#3": {"tests": [f"{CORE}/features.rs::every_check_in_stage_1_runs"]},
    "4.2#4": {"tests": [f"{CORE}/features.rs::every_check_in_stage_1_runs",
                        f"{CORE}/features.rs::only_geometric_validation_may_follow_the_schema"]},
    "4.2#5": {"tests": [f"{CORE}/features.rs::an_assembly_is_refused"]},
    "4.2#6": {"tests": [f"{CORE}/features.rs::a_file_with_no_product_definition_is_refused"]},
    "4.2#7": {"tests": [f"{CORE}/features.rs::two_product_definitions_without_an_assembly_are_refused"]},
    "4.2#8": {"tests": [f"{CORE}/features.rs::an_incomplete_shape_is_refused_and_ends_the_stage"]},
    "4.2#9": {"tests": [f"{CORE}/features.rs::a_definition_with_no_shape_has_no_solid"]},
    "4.2#10": {"tests": [f"{CORE}/features.rs::two_solids_in_one_product_are_refused"]},
    "4.2#11": {"tests": [f"{CORE}/features.rs::a_set_holding_a_surface_is_refused",
                         f"{CORE}/features.rs::a_surface_model_beside_the_solid_is_refused"]},
    "4.2#12": {"tests": [f"{CORE}/features.rs::stage_3_refuses_a_placement_that_is_not_rigid"]},
    "4.2#13": {"tests": [f"{CORE}/features.rs::stage_4_refuses_a_solid_whose_topology_cannot_be_read"]},
    "4.2#14": {"tests": [f"{CORE}/features.rs::stage_4_refuses_a_solid_whose_topology_cannot_be_read"]},
    "4.2#15": {"tests": [f"{CORE}/features.rs::stage_4_refuses_a_shell_with_a_face_missing",
                         f"{CORE}/features.rs::stage_4_refuses_a_shell_split_across_two_shells"]},
    "4.2#16": {"tests": [f"{CORE}/features.rs::stage_4_refuses_a_shell_in_two_pieces"]},
    "4.2#17": {"tests": [f"{CORE}/features.rs::stage_5_refuses_a_solid_whose_context_cannot_be_read"]},
    "4.2#18": {},
    "4.2#19": {"tests": [f"{CORE}/features.rs::stage_5_refuses_missing_units"]},
    "4.2#20": {"tests": [f"{CORE}/features.rs::stage_5_refuses_missing_units"]},
    "4.2#21": {"tests": [f"{CORE}/features.rs::stage_5_refuses_missing_units"]},
    "4.2#22": {"tests": [f"{CORE}/features.rs::stage_6_refuses_unreadable_and_out_of_range_geometry",
                         f"{CORE}/features.rs::stage_6_refuses_a_face_or_edge_whose_geometry_is_not_geometry"]},
    "4.2#23": {},
    "4.2#24": {},
    "4.2#25": {},
    "4.2#25a": {"tests": [f"{CORE}/features.rs::stage_6_refuses_a_plane_face_whose_sense_is_flipped",
                          f"{CORE}/features.rs::stage_6_refuses_a_flipped_plane_face_with_an_arc_standing_across_it"]},
    "4.2#25b": {},
    "4.2#26": {"tests": [f"{CORE}/features.rs::stage_6_refuses_unreadable_and_out_of_range_geometry",
                         f"{CORE}/planes.rs::a_mapped_item_that_carries_the_solid_out_of_range_is_refused"]},
    "4.2#27": {"tests": [f"{SRC}/primary.rs::faces_on_one_cylinder_with_opposite_senses_are_a_contradiction"]},
    "4.2#28": {"tests": [f"{SRC}/walk.rs::an_annulus_whose_side_contradicts_its_radii_is_a_contradiction",
                         f"{SRC}/walk.rs::a_shaft_widening_into_air_through_an_annulus_is_a_contradiction"]},
    "4.2#29": {"tests": [f"{SRC}/full.rs::a_chain_of_arcs_around_the_circle_twice_is_a_contradiction"]},
    "4.2#30": {"deferred": "internal is a fitment bug, not a property of input: the unit, corpus and fixture "
                           "tests assert that no file produces it"},
    "4.3": {},
    "5.1": {},
    "5.2.near_miss": {"tests": [f"{CORE}/cylinders.rs::halves_apart_by_less_than_the_uncertainty_are_a_near_miss"]},
    "5.3": {"tests": [f"{CORE}/planes.rs::a_stated_outer_bound_that_is_not_the_outer_loop_is_a_note"]},
    "5.4": {"tests": [f"{CORE}/features.rs::reading_is_deterministic"]},
    "6": {"tests": [f"{CORE}/planes.rs::every_face_is_in_exactly_one_primary_feature",
                    f"{CORE}/planes.rs::a_document_whose_features_break_the_partition_is_not_read"]},
    "7.1": {},
    "7.2": {"tests": [f"{CORE}/planes.rs::a_step_hidden_by_a_far_plane_location_is_two_planes"]},
    "7.3": {"tests": [f"{CORE}/planes.rs::a_plane_face_with_a_hole_has_its_exact_area_and_loops",
                      f"{CORE}/planes.rs::an_edge_on_an_unread_curve_leaves_the_outline_unknown",
                      f"{CORE}/planes.rs::a_circle_tilted_off_its_plane_leaves_the_outline_unknown"]},
    "G1": {}, "G2": {}, "G3": {}, "G4": {}, "G5": {},
    "S1": {}, "S3": {},
    "S2": {"tests": [f"{SRC}/primary.rs::faces_on_one_cylinder_with_opposite_senses_are_a_contradiction"]},
    "F1": {"tests": [f"{CORE}/cylinders.rs::a_through_hole_is_full_between_its_two_end_circles"]},
    "F2": {"tests": [f"{SRC}/full.rs::two_arcs_around_the_circle_once_are_a_full_station",
                     f"{SRC}/full.rs::a_chain_that_sweeps_nothing_is_not_full"]},
    "F3": {"deferred": "withdrawn in review of PR 5: a seam is used twice by one loop as a slit is, and "
                       "without the curves on the surface it proves no wrap"},
    "F4.full": {"tests": [f"{CORE}/cylinders.rs::a_through_hole_is_full_between_its_two_end_circles"]},
    "F4.undecided": {"tests": [f"{SRC}/full.rs::end_stations_facing_away_from_the_wall_are_undecided",
                               f"{CORE}/cylinders.rs::a_hole_with_no_end_circle_is_undecided_whatever_its_seam"]},
    "F4.partial": {}, "F4.one_irregular": {}, "F4.wall_openings": {},
    "F4.seam": {"tests": [f"{CORE}/cylinders.rs::a_slit_used_twice_by_one_loop_proves_no_wrap"]},
    "F4.one_station": {"tests": [
        f"{CORE}/cylinders.rs::a_wall_past_its_one_station_is_irregular_there_however_its_curve_ends"]},
    "F5": {},
    "8.6": {}, "8.7": {},
    "W1": {},
    "W2": {"tests": [f"{SRC}/walk.rs::a_hole_widening_through_an_annulus_into_air_steps_to_the_wider_bore"]},
    "W3.plane": {"tests": [f"{SRC}/walk.rs::a_hole_widening_through_an_annulus_into_air_steps_to_the_wider_bore"]},
    "W3.other": {}, "W3.cone": {}, "W3.torus": {}, "W3.plane_agreement": {},
    "W4.annulus": {"tests": [
        f"{SRC}/walk.rs::a_hole_widening_through_an_annulus_into_air_steps_to_the_wider_bore"]},
    "W4.apex": {"tests": [f"{CORE}/holes.rs::a_point_of_two_cones_measures_its_tip_to_the_apex",
                          f"{CORE}/holes.rs::a_dimple_in_a_flat_floor_is_no_drill_point"]},
    "W4.cone": {}, "W4.disk": {}, "W4.face": {}, "W4.post": {}, "W4.sphere": {}, "W4.torus": {},
    "W4.step": {"tests": [f"{SRC}/walk.rs::a_hole_widening_through_an_annulus_into_air_steps_to_the_wider_bore"]},
    "W4.transition": {},
    "W5.opening": {}, "W5.bottom.flat": {}, "W5.bottom.cone_then_flat": {}, "W5.bottom.point": {},
    "W5.bottom.annular": {}, "W5.free_end": {}, "W5.root": {}, "W5.unknown": {}, "W5.irregular": {},
    "W5.step": {"tests": [f"{SRC}/stack.rs::mutual_steps_join_two_cylinders_in_one_stack"]},
    "W6": {"tests": [f"{SRC}/stack.rs::mutual_steps_join_two_cylinders_in_one_stack",
                     f"{SRC}/stack.rs::a_step_the_other_cylinder_does_not_return_is_unknown"]},
    "W7": {},
    "W7b": {"tests": [f"{CORE}/holes.rs::a_face_running_back_along_the_axis_says_nothing_about_the_end"]},
    "W8": {},
    "K1": {}, "K2.through": {}, "K2.blind": {},
    "K2.enclosed": {},
    "K2.unknown": {"tests": [f"{CORE}/holes.rs::a_face_running_back_along_the_axis_says_nothing_about_the_end"]},
    "K3.counterbore": {}, "K3.countersink": {}, "K3.step_drill": {}, "K3.edge_round": {},
    "E1": {"tests": [f"{CORE}/holes.rs::stations_come_from_circle_centres_not_vertices"]},
    "E2": {}, "E3": {}, "E4": {},
    "9.4": {}, "9.7": {}, "13.2": {},
}

# M3 (docs/research/m3-socket.md): its gate checks 31 to 39 and its rules.
# Those a later PR builds are deferred with the PR that builds them, so the
# coverage test sees every id the design states from the start.
M3_CORE = "crates/fitment-core/tests"
RULES.update({
    "m3.4.1#31": {"tests": [f"{M3_CORE}/assemblies.rs::a_part_file_lists_one_part_and_is_not_an_assembly"]},
    "m3.4.1#32": {"tests": [f"{M3_CORE}/assemblies.rs::an_assembly_needs_exactly_one_root"]},
    "m3.4.1#33": {"tests": [f"{M3_CORE}/assemblies.rs::a_cycle_is_an_unreadable_placement"]},
    "m3.4.1#34": {"tests": [f"{M3_CORE}/assemblies.rs::a_diamond_past_the_occurrence_limit_is_refused"]},
    "m3.4.1#35": {"tests": [f"{M3_CORE}/assemblies.rs::material_in_an_assembly_s_own_shape_is_refused"]},
    "m3.4.1#36": {"deferred": "M3 PR 2 reads the components"},
    "m3.4.1#37": {"deferred": "M3 PR 2 reads the components"},
    "m3.4.1#38": {"deferred": "M3 PR 2 reads the components"},
    "m3.4.1#39": {"deferred": "M3 PR 3 builds the relation search"},
    "T1": {}, "T2": {"tests": [f"{M3_CORE}/assemblies.rs::a_definition_instance_selects_unless_an_id_reads_the_same"]},
    "T3": {}, "T4": {}, "T5": {}, "T6": {"deferred": "M3 PR 3: the socket command's exit code for a selection error"},
    "O1": {}, "O2": {}, "O3": {"tests": [f"{M3_CORE}/assemblies.rs::a_path_segment_that_names_two_usages_is_refused"]},
    "O4": {}, "O5": {}, "O6": {},
    "P1": {}, "P2": {}, "P7": {}, "P9": {},
    "P3": {"deferred": "M3 PR 2 reads the components and checks their frames"},
    "P4": {"deferred": "M3 PR 3 moves features"}, "P5": {"deferred": "M3 PR 3 moves features"},
    "P6": {"deferred": "M3 PR 3 compares placed axes"},
    "P8": {"deferred": "M3 PR 3 builds s22"},
    **{f"Z{k}": {"deferred": "M3 PR 2 (Z1) and PR 3 (the rest)"} for k in range(1, 8)},
    **{f"{letter}{k}": {"deferred": f"M3 PR {pr}"} for letter, n, pr in [
        ("N", 5, 3), ("X", 6, 3), ("B", 8, 3), ("I", 7, 3), ("J", 3, 3), ("U", 8, 3), ("A", 3, 3),
        ("H", 2, 4), ("Q", 2, 4), ("C", 6, 4), ("L", 2, 4), ("Y", 4, 3), ("V", 2, 3)]
       for k in range(1, n + 1)},
})


def coverage(cases):
    """The `rule_ids` entry of the corpus manifest: every rule in RULES with
    the cases that cite it, its tests and any reason it is deferred."""
    cited = {}
    for case in cases:
        for rule in case["rules"]:
            if rule not in RULES:
                raise RuntimeError(f"{case['id']} cites {rule}, which RULES does not list")
            cited.setdefault(rule, []).append(case["id"])
    return {rule: {"cases": sorted(cited.get(rule, [])), **entry} for rule, entry in sorted(RULES.items())}


# ----------------------------------------------------------------- output


class Writer:
    def __init__(self, out: Path):
        self.out = out
        self.cases = []
        out.mkdir(parents=True, exist_ok=True)

    def shape(self, shape, name, unit="MM", roots=None, pcurves=False):
        """Writes `shape` (or several `roots`, each its own product) without
        pcurves unless asked, and with assembly mode off, so a compound is one
        product."""
        writer = STEPControl_Writer()
        Interface_Static.SetIVal_s("write.surfacecurve.mode", 1 if pcurves else 0)
        Interface_Static.SetIVal_s("write.step.assembly", 0)
        Interface_Static.SetCVal_s("xstep.cascade.unit", "MM")
        Interface_Static.SetCVal_s("write.step.unit", unit)
        for s in roots or [shape]:
            if writer.Transfer(s, STEPControl_AsIs) != IFSelect_RetDone:
                raise RuntimeError(f"{name}: transfer failed")
        path = self.out / f"{name}.stp"
        if writer.Write(str(path)) != IFSelect_RetDone:
            raise OSError(f"{name}: write failed")
        self.fix_time(path)
        return path

    def text(self, text, name, base):
        """Writes an edited file, its products renamed from `base`'s case."""
        path = self.out / f"{name}.stp"
        renamed = text.replace(f"'{base}'", f"'{name}'")
        if renamed == text:
            raise RuntimeError(f"{name}: no product named {base}")
        path.write_text(renamed, encoding="latin-1", newline="\n")
        self.fix_time(path)
        return path

    @staticmethod
    def fix_time(path):
        """Makes a file depend only on its own construction: a fixed time
        stamp, and products named after the case rather than by Open
        CASCADE's count of products written so far in this process."""
        text = path.read_text(encoding="latin-1")
        text, n = re.subn(r"(FILE_NAME\('[^']*',)'[^']*'", rf"\1'{TIME_STAMP}'", text, count=1)
        if n != 1:
            raise RuntimeError(f"{path.name}: no FILE_NAME time stamp")
        names = []
        for name in re.findall(r"Open CASCADE STEP translator [\d.]+ [\d.]+", text):
            if name not in names:
                names.append(name)
        for k, name in enumerate(names):
            text = text.replace(f"'{name}'", f"'{path.stem}{'' if k == 0 else f' {k + 1}'}'")
        path.write_text(text, encoding="latin-1", newline="\n")

    def case(self, name, tests, rules, unit_set, construction, expect):
        self.cases.append({"id": name, "file": f"{name}.stp", "set": "unit" if unit_set else "corpus",
                           "tests": tests, "rules": rules, "construction": construction, "expect": expect})


def accepted(features: Features):
    return {"result": "accepted", "features": features.expect()}


def refused(stage, *codes):
    return {"result": "refused", "stage": stage, "refusals": list(codes)}


class Part21:
    """The instances of a written file by number, for the documented text
    edits (as in stepq's tools/gen-occt-cases.py); written back in order."""

    def __init__(self, text):
        start = text.index("DATA;") + len("DATA;")
        self.head, self.inst, self.tail = text[:start], {}, ""
        chunk, quoted, i = [], False, start
        while i < len(text):
            c = text[i]
            if c == "'":
                quoted = not quoted
            elif not quoted and c == ";":
                statement = "".join(chunk).strip()
                chunk = []
                if statement == "ENDSEC":
                    self.tail = text[i + 1:]
                    break
                name, _, value = statement.partition("=")
                self.inst[int(name.strip()[1:])] = " ".join(value.split())
                i += 1
                continue
            chunk.append(c)
            i += 1

    def text(self):
        body = "".join(f"\n#{n} = {v};" for n, v in self.inst.items())
        return f"{self.head}{body}\nENDSEC;{self.tail}"

    def add(self, value):
        n = max(self.inst) + 1
        self.inst[n] = value
        return n

    def refs(self, n):
        return [int(x) for x in re.findall(r"#(\d+)", self.inst[n])]

    def point(self, n):
        return [float(x) for x in re.findall(r"[-+]?\d+\.?\d*(?:E[-+]?\d+)?", self.inst[n].split("(", 2)[2])]

    def find(self, prefix):
        return [n for n, v in self.inst.items() if v.startswith(prefix)]

    def plane_face(self, normal, offset):
        """The `advanced_face` on the plane whose stated z and position point
        give `normal · p = offset`."""
        for n in self.find("ADVANCED_FACE"):
            surface = self.refs(n)[-1]
            if not self.inst[surface].startswith("PLANE"):
                continue
            position = self.refs(surface)[0]
            loc, axis = self.refs(position)[:2]
            z = self.point(axis)
            if abs(abs(dot(z, normal)) - 1) < 1e-12 and abs(dot(self.point(loc), normal) - offset) < 1e-9:
                return n
        raise RuntimeError("no such plane face")


def real(x):
    """`x` as a Part 21 REAL: a decimal point always, an upper-case E."""
    text = repr(float(x)).upper()
    mantissa, _, exponent = text.partition("E")
    if "." not in mantissa:
        mantissa += "."
    return mantissa + ("E" + exponent if exponent else "")


def reals(v):
    return "(" + ",".join(real(c) for c in v) + ")"


def flip_sense(face_value):
    head, _, sense = face_value.rpartition(",")
    return head + "," + (".T.)" if sense.strip() == ".F.)" else ".F.)")


# ------------------------------------------------------------------ cases


def build(w: Writer, unit_only: bool):
    """Every case. With `unit_only`, cases outside the unit set are still
    built when another case needs them, but not written."""

    def part(name, shape, features, tests, rules, unit_set, construction, unit="MM"):
        """Writes an accepted case; `features` is a Features, or the
        expected dict when a rule changes what construction alone gives."""
        if unit_set or not unit_only:
            w.shape(shape, name, unit=unit)
            expect = features if isinstance(features, dict) else accepted(features)
            w.case(name, tests, rules, unit_set, construction, expect)
        if unit_only:
            return
        # Every positive part built here again, in the corpus only: written
        # with pcurves, as Open CASCADE writes by default, and, where its
        # answers follow from its construction alone, in a second skew frame.
        w.shape(shape, f"{name}_pc", unit=unit, pcurves=True)
        w.case(f"{name}_pc", f"{name} written with pcurves", rules, False, {"variant_of": name}, expect)
        if not isinstance(features, dict):
            moved = BRepBuilderAPI_Transform(shape, VARIANT.trsf(), True).Shape()
            w.shape(moved, f"{name}_rot", unit=unit)
            w.case(f"{name}_rot", f"{name} moved into a second skew frame", rules, False,
                   {"variant_of": name, "motion": {"axis": VARIANT.axis, "angle_deg": VARIANT.angle,
                                                   "shift": VARIANT.shift}},
                   accepted(features.moved(VARIANT)))

    # p01: the simplest through hole, and the base of every text edit.
    h01 = Hole((20, 15, 8), DOWN, [(6.6, 8)], through=8)
    s01 = cut(box(0, 0, 0, 40, 30, 8), tool(h01))
    f01 = Features([h01], planes=box_planes(0, 0, 0, 40, 30, 8))
    part("p01_through", s01, f01, "one through hole: one face with a seam, same_sense .F.",
         ["G2", "S1", "F2", "W4.face", "W5.opening", "K2.through", "E2", "7.1"], True,
         {"box": [0, 0, 0, 40, 30, 8], "hole": "dia 6.6 through at (20, 15)"})
    # p01 is in the unit set, so it is always written; the text edits start
    # from it.
    p01_text = (w.out / "p01_through.stp").read_text(encoding="latin-1")

    h03 = Hole((20, 20, 20), DOWN, [(8, 12)])
    part("p03_blind_flat", cut(box(0, 0, 0, 40, 40, 20), tool(h03)),
         Features([h03], planes=box_planes(0, 0, 0, 40, 40, 20) + [Plane(UP, 8)]),
         "blind hole with a flat bottom, depth 12", ["W4.disk", "W5.bottom.flat", "K2.blind", "E3"], True,
         {"box": [0, 0, 0, 40, 40, 20], "hole": "dia 8 to depth 12 at (20, 20)"})

    h04 = Hole((20, 20, 20), DOWN, [(8, 12)], point_deg=118)
    part("p04_blind_drill", cut(box(0, 0, 0, 40, 40, 20), tool(h04)),
         Features([h04], planes=box_planes(0, 0, 0, 40, 40, 20), cones=1),
         "blind hole ending in a 118 deg drill point; depth to the shoulder 12",
         ["W4.apex", "W5.bottom.point", "E3", "8.6"], True,
         {"box": [0, 0, 0, 40, 40, 20], "hole": "dia 8 to depth 12, 118 deg point"})

    h05 = Hole((20, 20, 15), DOWN, [(11, 6.5), (6.6, 15)], through=15)
    s05 = cut(box(0, 0, 0, 40, 40, 15), tool(h05))
    f05 = Features([h05], planes=box_planes(0, 0, 0, 40, 40, 15) + [Plane(UP, 8.5)])
    c05 = {"box": [0, 0, 0, 40, 40, 15], "hole": "counterbore dia 11 x 6.5 over dia 6.6 through"}
    part("p05_cbore", s05, f05, "counterbore: two coaxial radii joined by an annulus",
         ["W4.annulus", "W6", "K1", "K3.counterbore"], True, c05)

    h06 = Hole((20, 20, 10), DOWN, [(6.6, 10)], through=10, countersink=(12.4, 90))
    part("p06_csink", cut(box(0, 0, 0, 40, 40, 10), tool(h06)),
         Features([h06], planes=box_planes(0, 0, 0, 40, 40, 10), cones=1),
         "90 deg countersink to dia 12.4 over dia 6.6 through", ["W3.cone", "W4.cone", "K3.countersink", "W8"],
         True, {"box": [0, 0, 0, 40, 40, 10], "hole": "dia 6.6 through, countersink 90 deg to 12.4"})

    h07 = Hole((20, 20, 10), DOWN, [(16, 0.5), (6.6, 10)], through=10)
    part("p07_spotface", cut(box(0, 0, 0, 40, 40, 10), tool(h07)),
         Features([h07], planes=box_planes(0, 0, 0, 40, 40, 10) + [Plane(UP, 9.5)]),
         "spotface: a counterbore 0.5 deep", ["K3.counterbore"], False,
         {"box": [0, 0, 0, 40, 40, 10], "hole": "dia 16 x 0.5 over dia 6.6 through"})

    def multi():
        hs = [Hole((15, 20, 30), DOWN, [(6.6, 30)], through=30),
              Hole((30, 20, 30), DOWN, [(11, 6), (6.6, 30)], through=30),
              Hole((45, 20, 30), DOWN, [(11, 6), (6.6, 20)], point_deg=118)]
        return cut(box(0, 0, 0, 60, 40, 30), *[tool(h) for h in hs]), hs

    s08, h08 = multi()
    f08 = Features(h08, planes=box_planes(0, 0, 0, 60, 40, 30) + [Plane(UP, 24, faces=2)], cones=1)
    part("p08_multi", s08, f08, "three stacks: plain, counterbored through, counterbored blind with a point",
         ["K1", "K2.through", "K2.blind"], False, {"box": [0, 0, 0, 60, 40, 30], "holes": "x = 15, 30, 45"})

    part("p09_pin", cyl((0, 0, 0), UP, 3, 20),
         Features(shafts=[Shaft((0, 0, 0), UP, 6, 20, kind="free_free")], planes=[Plane(UP, 20), Plane(DOWN, 0)]),
         "a whole-part shaft: same_sense .T., two free ends", ["S1", "W5.free_end", "E4"], True,
         {"cylinder": "dia 6, z 0 to 20"})

    h10 = Hole((30, 20, 16), DOWN, [(6.6, 16)], through=16)
    s10 = cut(fuse(box(0, 0, 0, 60, 40, 6), cyl((30, 20, 6), UP, 6, 10)), tool(h10))
    part("p10_boss_hole", s10,
         Features([h10], [Shaft((30, 20, 6), UP, 12, 10)],
                  planes=[Plane(UP, 6), Plane(DOWN, 0), Plane((1, 0, 0), 60), Plane((-1, 0, 0), 0),
                          Plane((0, 1, 0), 40), Plane((0, -1, 0), 0), Plane(UP, 16)]),
         "a boss with a bore through it: the bore opens on the boss top, an annulus",
         ["W4.post", "W5.root", "9.7"], True, {"plate": [0, 0, 0, 60, 40, 6], "boss": "dia 12, z 6 to 16"})

    h11 = Hole((35, 30, 10), DOWN, [(6.6, 10)], through=10)
    # One extruded L profile with its R3 fillet, so that each side is one
    # face (fused boxes would leave coplanar faces split).
    corner = 13 - 3 / math.sqrt(2)
    s11 = (cq.Workplane("XZ").polyline([(10, 30), (0, 30), (0, 0), (60, 0), (60, 10), (13, 10)])
           .threePointArc((corner, corner), (10, 13)).close().extrude(-40).val().wrapped)
    slot = fuse(cyl((25, 10, -1), UP, 3, 12), cyl((45, 10, -1), UP, 3, 12), box(25, 7, -1, 20, 6, 12))
    s11 = cut(s11, slot, tool(h11))
    part("p11_fillet_slot", s11,
         Features([h11], partials=[Partial((13, 0, 13), (0, 1, 0), 6, 90),
                                   Partial((25, 10, 0), UP, 6, 180), Partial((45, 10, 0), UP, 6, 180)],
                  planes=[Plane(DOWN, 0), Plane(UP, 10), Plane(UP, 30), Plane((1, 0, 0), 10), Plane((-1, 0, 0), 0),
                          Plane((1, 0, 0), 60), Plane((0, -1, 0), 0), Plane((0, 1, 0), 40), Plane((0, 1, 0), 7),
                          Plane((0, -1, 0), -13)]),
         "concave cylinders that are not holes: a fillet (90 deg) and two slot ends (180 deg)",
         ["F4.partial", "F5"], True, {"L block": "60 x 40 x 10 with an upright", "slot": "width 6, x 25 to 45"})

    arc = 360 - 2 * math.degrees(math.acos(2 / 4))
    part("p12_edge_notch", cut(box(0, 0, 0, 40, 30, 8), cyl((2, 15, -1), UP, 4, 10)),
         Features(partials=[Partial((2, 15, 0), UP, 8, arc)],
                  planes=[Plane(UP, 8), Plane(DOWN, 0), Plane((1, 0, 0), 40), Plane((-1, 0, 0), 0, faces=2),
                          Plane((0, 1, 0), 30), Plane((0, -1, 0), 0)]),
         "a concave cylinder of 240 deg cut through an edge: more than half, not a hole", ["F4.partial"], True,
         {"box": [0, 0, 0, 40, 30, 8], "cut": "dia 8 at (2, 15)"})

    def crossing(d2):
        s = cut(box(0, 0, 0, 30, 30, 30), cyl((-1, 15, 15), (1, 0, 0), 4, 32), cyl((15, -1, 15), (0, 1, 0), d2 / 2, 32))
        x = Hole((0, 15, 15), (1, 0, 0), [(8, 30)], through=30)
        halves = [Hole((15, 0, 15), (0, 1, 0), [(d2, 30)], kind="unknown", far_end="irregular"),
                  Hole((15, 30, 15), (0, -1, 0), [(d2, 30)], kind="unknown", far_end="irregular")]
        return s, x, halves

    s13, x13, y13 = crossing(6)
    part("p13_cross", s13, Features([x13] + y13, planes=box_planes(0, 0, 0, 30, 30, 30)),
         "dia 8 along x crossed by dia 6 along y: the dia 6 is two cylinders, each ending in the dia 8 bore",
         ["G5", "F4.wall_openings", "F4.one_irregular", "W5.irregular"], False,
         {"cube": 30, "holes": "dia 8 along x, dia 6 along y"})
    # Equal bores meet along curves that touch only at two points, so each
    # bore is two faces sharing no edge: four cylinders under G2.
    s13b = crossing(8)[0]
    part("p13b_cross_equal", s13b,
         Features([Hole((0, 15, 15), (1, 0, 0), [(8, 30)], kind="unknown", far_end="irregular"),
                   Hole((30, 15, 15), (-1, 0, 0), [(8, 30)], kind="unknown", far_end="irregular"),
                   Hole((15, 0, 15), (0, 1, 0), [(8, 30)], kind="unknown", far_end="irregular"),
                   Hole((15, 30, 15), (0, -1, 0), [(8, 30)], kind="unknown", far_end="irregular")],
                  planes=box_planes(0, 0, 0, 30, 30, 30)),
         "two equal dia 8 bores crossing: four cylinders (G2), each with one opening and one irregular end",
         ["G2", "G5", "F4.one_irregular"], False, {"cube": 30, "holes": "dia 8 along x and y"})

    def baked(shape, m):
        return BRepBuilderAPI_Transform(shape, m.trsf(), True).Shape()

    part("p14s_rotated_cbore", baked(s05, TILT), f05.moved(TILT),
         "p05 rotated 37 deg about (1, 2, 3) and moved: no axis-aligned shortcut", ["9.4", "13.2"], True,
         {**c05, "motion": {"axis": TILT.axis, "angle_deg": TILT.angle, "shift": TILT.shift}})
    part("p14_rotated", baked(s08, TILT), f08.moved(TILT), "p08 under the same motion", ["9.4"], False,
         {"motion": {"axis": TILT.axis, "angle_deg": TILT.angle, "shift": TILT.shift}})
    part("p16_inch", s05, f05, "p05 written in inches: the answers in mm", ["5.1"], True, c05, unit="INCH")

    div = ShapeUpgrade_ShapeDivideClosed(s05)
    div.SetNbSplitPoints(1)
    div.Perform()
    halves = div.Result()
    part("p17_split_halves", halves, f05, "p05 with each closed face split in two", ["G4"], False, c05)
    quarters = ShapeUpgrade_ShapeDivideAngle(math.radians(90), s05)
    quarters.Perform()
    part("p18_split_quarters", quarters.Result(), f05, "p05 with each face cut every 90 deg", ["G4"], False, c05)

    # p20: the second half of each cylinder restated by a surface whose
    # placement is moved 7 mm along the axis, reversed and turned 90 deg.
    w.shape(halves, "_p17_text")
    f = Part21((w.out / "_p17_text.stp").read_text(encoding="latin-1"))
    (w.out / "_p17_text.stp").unlink()
    seen, edits = set(), 0
    for surface in f.find("CYLINDRICAL_SURFACE"):
        position = f.refs(surface)[0]
        loc, axis, ref = f.refs(position)
        pt, ax, rf = f.point(loc), f.point(axis), f.point(ref)
        radius = f.inst[surface].rsplit(",", 1)[1].rstrip(")")
        key = (tuple(pt), tuple(ax), radius)
        if key not in seen:
            seen.add(key)
            continue
        p_ = f.add(f"CARTESIAN_POINT('',{reals(add(pt, ax, 7))})")
        a_ = f.add(f"DIRECTION('',{reals([-c for c in ax])})")
        r_ = f.add(f"DIRECTION('',{reals(cross(ax, rf))})")
        f.inst[surface] = f"CYLINDRICAL_SURFACE('',#{f.add(f'AXIS2_PLACEMENT_3D({chr(39)}{chr(39)},#{p_},#{a_},#{r_})')},{radius})"
        edits += 1
    if edits != 2:
        raise RuntimeError(f"p20: {edits} edits, expected 2")
    w.text(f.text(), "p20_split_surfaces", "_p17_text")
    w.case("p20_split_surfaces", "p17 with each cylinder's second half on a surface moved 7 mm along the axis, "
           "reversed and turned 90 deg (a text edit): grouping ignores the placement and the axis sign",
           ["G1", "G4"], True, {**c05, "edit": "2 cylindrical surfaces restated"}, accepted(f05))

    s21 = cut(box(0, 0, 0, 60, 40, 20), box(20, -1, 10, 20, 42, 11))
    chamfer = cq.Workplane("XZ").polyline([(56, 20), (61, 20), (61, 15)]).close().extrude(-42).translate((0, -1, 0))
    s21 = cut(s21, chamfer.val().wrapped)
    k = 1 / math.sqrt(2)
    part("p21_planes", s21,
         Features(planes=[Plane(UP, 20, faces=2), Plane(UP, 10), Plane((1, 0, 0), 20), Plane((-1, 0, 0), -40),
                          Plane(DOWN, 0), Plane((-1, 0, 0), 0), Plane((1, 0, 0), 60), Plane((0, -1, 0), 0),
                          Plane((0, 1, 0), 40), Plane((k, 0, k), 76 * k)]),
         "a channel and a 45 deg chamfer: 10 planes over 11 faces, two coplanar tops in one plane", ["7.1", "7.2"],
         True, {"box": [0, 0, 0, 60, 40, 20], "channel": "x 20 to 40, z 10 to 20", "chamfer": "(56, 20) to (60, 16)"})

    s22, h22 = box(0, 0, 0, 40, 20, 10), []
    for x, off in ((10, 0.0), (30, 0.02)):
        top, bottom = Hole((x, 10, 10), DOWN, [(6, 4)]), Hole((x + off, 10, 0), UP, [(6, 4)])
        s22 = cut(s22, tool(top), tool(bottom))
        h22 += [top, bottom]
    part("p22_coaxial_blind", s22,
         Features(h22, planes=box_planes(0, 0, 0, 40, 20, 10) + [Plane(UP, 6, faces=2), Plane(DOWN, -4, faces=2)]),
         "blind holes from opposite faces, a 2 mm wall between; one pair coaxial, one 0.02 mm apart: four blind holes",
         ["G5", "K2.blind"], True, {"box": [0, 0, 0, 40, 20, 10], "holes": "dia 6 x 4 from top and bottom at x 10, 30"})

    h23 = Hole((10, 10, 10), DOWN, [(6, 9.995)])
    part("p23_thin_floor", cut(box(0, 0, 0, 20, 20, 10), tool(h23)),
         Features([h23], planes=box_planes(0, 0, 0, 20, 20, 10) + [Plane(UP, 0.005)]),
         "a 0.005 mm floor: blind, never through", ["K2.blind"], False, {"box": [0, 0, 0, 20, 20, 10]})

    h24 = Hole((10, 10, 20), DOWN, [(10.02, 8), (10.0, 20)], through=20)
    part("p24_radius_step", cut(box(0, 0, 0, 20, 20, 20), tool(h24)),
         Features([h24], planes=box_planes(0, 0, 0, 20, 20, 20) + [Plane(UP, 12)]),
         "dia 10.02 over dia 10.00: two segments joined by a 0.01 mm annulus", ["G1", "K3.counterbore"], False,
         {"box": [0, 0, 0, 20, 20, 20]})

    th = math.radians(30)
    into25 = (0.0, math.sin(th), -math.cos(th))
    t25 = 10 / math.cos(th)
    h25 = Hole(add((20, 20, 10), into25, -t25), into25, [(6, 2 * t25)], through=2 * t25,
               kind="unknown", rim_end="irregular", far_end="irregular")
    part("p25_angled", cut(box(0, 0, 0, 40, 40, 20), tool(Hole(h25.entry, into25, [(6, 2 * t25 + 5)]), overshoot=5.0)),
         Features([h25], planes=box_planes(0, 0, 0, 40, 40, 20)),
         "dia 6 through, tilted 30 deg: ends are ellipses, so the cylinder is undecided and no hole is read (F3 "
         "withdrawn)", ["F4.seam"],
         True, {"box": [0, 0, 0, 40, 40, 20], "hole": "dia 6 through (20, 20, 10), tilted 30 deg about x"})

    part("p26_all_bspline", BRepBuilderAPI_NurbsConvert(s01, True).Shape(), Features(planes=[], other_faces=7),
         "p01 with every face a B-spline: accepted, every face other, no feature (R1)", ["4.3", "8.7"], True,
         {"from": "p01", "edit": "BRepBuilderAPI_NurbsConvert"})

    profile = (cq.Workplane("XZ").moveTo(0, 0).lineTo(40, 0).lineTo(40, 8)
               .spline([(20, 10), (0, 8)], includeCurrent=True).close().extrude(-30))
    h27 = Hole((20, 15, 0), UP, [(6.6, 4)])
    part("p27_freeform", cut(profile.val().wrapped, tool(h27)),
         Features([h27], planes=[Plane(DOWN, 0), Plane((-1, 0, 0), 0), Plane((1, 0, 0), 40), Plane((0, -1, 0), 0),
                                 Plane((0, 1, 0), 30), Plane(DOWN, -4)], other_faces=1),
         "a spline top face (surface of linear extrusion) away from a blind hole: the hole is read", ["8.7"], False,
         {"profile": "40 x 8 with a spline top, extruded 30 in y", "hole": "dia 6.6 x 4 from below"})

    s28 = cut(box(0, 0, 0, 20, 20, 20), BRepPrimAPI_MakeSphere(gp_Pnt(10, 10, 10), 4).Shape())
    part("p28_void", s28, Features(planes=box_planes(0, 0, 0, 20, 20, 20), other_faces=1),
         "a cube with a spherical void: accepted, the sphere is an other face in shell 1, no hole (R5)", ["S3", "8.7"],
         False, {"cube": 20, "void": "sphere dia 8 at the centre"})

    # p29: p01's solid moved into a second representation, placed by a
    # mapped item. The answers are p01's moved by the mapped item.
    f = Part21(p01_text)
    shape_rep = f.find("ADVANCED_BREP_SHAPE_REPRESENTATION")[0]
    origin, solid = f.refs(shape_rep)[:2]
    context = f.refs(shape_rep)[-1]
    held = f.add(f"ADVANCED_BREP_SHAPE_REPRESENTATION('',(#{origin},#{solid}),#{context})")
    rep_map = f.add(f"REPRESENTATION_MAP(#{origin},#{held})")
    p_ = f.add(f"CARTESIAN_POINT('',{reals(MAPPED.shift)})")
    z_ = f.add(f"DIRECTION('',{reals(MAPPED.vector(UP))})")
    x_ = f.add(f"DIRECTION('',{reals(MAPPED.vector((1, 0, 0)))})")
    target = f.add(f"AXIS2_PLACEMENT_3D('',#{p_},#{z_},#{x_})")
    mapped = f.add(f"MAPPED_ITEM('',#{rep_map},#{target})")
    f.inst[shape_rep] = f"ADVANCED_BREP_SHAPE_REPRESENTATION('',(#{origin},#{mapped}),#{context})"
    if not unit_only:
        w.text(f.text(), "p29_mapped", "p01_through")
    w.case("p29_mapped", "p01's solid placed in its definition by a mapped item (a text edit): p01's answers moved by it",
           ["4.2#12", "5.1"], False,
           {"from": "p01", "mapped_item": {"axis": MAPPED.axis, "angle_deg": MAPPED.angle, "shift": MAPPED.shift}},
           accepted(f01.moved(MAPPED)))

    # p30: a bore through a boss whose top edge is rounded; the rim is the
    # boss top, not the round.
    boss30 = round_edges(cyl((20, 20, 10), UP, 8, 6), 1.0, circle_at(16, 8))
    h30 = Hole((20, 20, 16), DOWN, [(4, 16)], through=16)
    s30 = cut(fuse(box(0, 0, 0, 40, 40, 10), boss30), tool(h30))
    part("p30_round_bore", s30,
         Features([h30], [Shaft((20, 20, 10), UP, 16, 5, protrusion=6)],
                  planes=box_planes(0, 0, 0, 40, 40, 10) + [Plane(UP, 16)], tori=1),
         "a bore through a boss with a 1 mm round on its top edge: length 16, rim on the boss top (W8)",
         ["W8", "W3.torus", "W4.torus"], True, {"plate": [0, 0, 0, 40, 40, 10], "boss": "dia 16 x 6, R1 round"})

    # A dia 16 pocket from z 12 with an R1 floor fillet, around a dia 4 post
    # to z 15, cut as one revolved tool.
    s31 = revolved_cut(box(0, 0, 0, 40, 40, 20), (20, 20), lambda w: (
        w.moveTo(0, 15).lineTo(2, 15).lineTo(2, 12).lineTo(7, 12)
        .threePointArc(arc_mid((7, 13), 1, -45), (8, 13)).lineTo(8, 21).lineTo(0, 21)))
    part("p31_fillet_post", s31,
         # The wall ends at z 13, where the floor fillet starts; the floor is at z 12.
         Features([Hole((20, 20, 20), DOWN, [(16, 7)], post_diameter=4, floor_below=1)],
                  [Shaft((20, 20, 12), UP, 4, 3)],
                  planes=box_planes(0, 0, 0, 40, 40, 20) + [Plane(UP, 12), Plane(UP, 15)], tori=1),
         "a dia 4 x 3 post on the floor of a dia 16 pocket with a 1 mm floor fillet: protrusion 3",
         ["W8", "W5.bottom.annular"], False, {"box": [0, 0, 0, 40, 40, 20], "pocket": "dia 16 x 8, R1 floor fillet"})

    h32 = Hole((20, 20, 12), DOWN, [(11, 4), (6.6, 12)], through=12)
    # The bore, the counterbore with its R0.5 floor fillet, and the R0.5 rim
    # round, cut as one revolved tool.
    s32 = revolved_cut(box(0, 0, 0, 40, 40, 12), (20, 20), lambda w: (
        w.moveTo(0, -1).lineTo(3.3, -1).lineTo(3.3, 8).lineTo(5.0, 8)
        .threePointArc(arc_mid((5.0, 8.5), 0.5, -45), (5.5, 8.5)).lineTo(5.5, 11.5)
        .threePointArc(arc_mid((6.0, 11.5), 0.5, 135), (6.0, 12)).lineTo(6.0, 13).lineTo(0, 13)))
    e32 = accepted(Features([h32], planes=box_planes(0, 0, 0, 40, 40, 12) + [Plane(UP, 8)], tori=2))
    # The dia 11 wall runs only from the floor fillet (z 8.5) to the rim
    # round (z 11.5); the rim stays on the top face (W8).
    e32["features"]["holes"][0]["segments"] = [[6.6, 0.0, 8.0], [11.0, 8.5, 11.5]]
    part("p32_torus_cbore", s32, e32,
         "a counterbore with a 0.5 mm floor fillet and a 0.5 mm rim round: the walk crosses torus, annulus, torus",
         ["W3.torus", "W4.torus", "K3.edge_round", "W8"], True,
         {"box": [0, 0, 0, 40, 40, 12], "hole": "dia 11 x 4 over dia 6.6 through, R0.5 at both dia 11 edges"})

    half59 = math.tan(math.radians(59))
    h33 = Hole((20, 20, 20), DOWN, [(8, 10), (5, 20)], through=20)
    s33 = cut(box(0, 0, 0, 40, 40, 20), tool(Hole((20, 20, 20), DOWN, [(8, 10)])),
              cone((20, 20, 10), DOWN, 4, 2.5, 1.5 / half59), cyl((20, 20, -1), UP, 2.5, 22))
    e33 = accepted(Features([h33], planes=box_planes(0, 0, 0, 40, 40, 20), cones=1))
    # The dia 5 wall ends where the 118 deg cone from dia 8 meets it.
    e33["features"]["holes"][0]["segments"] = [[5.0, 0.0, r(10 - 1.5 / half59)], [8.0, 10.0, 20.0]]
    part("p33_step_drill", s33, e33, "dia 8 to depth 10, then dia 5 through via a 118 deg cone (a step drill)",
         ["K3.step_drill", "W4.cone"], False, {"box": [0, 0, 0, 40, 40, 20]})

    h34 = Hole((20, 20, 20), DOWN, [(11, 4), (6, 12)], point_deg=118, flat_after_point=3)
    part("p34_cbore_blind", cut(box(0, 0, 0, 40, 40, 20), tool(h34)),
         Features([h34], planes=box_planes(0, 0, 0, 40, 40, 20) + [Plane(UP, 16), Plane(UP, 8 - 1.5 / half59)],
                  cones=1),
         "a counterbored blind hole whose 118 deg point ends on a dia 3 flat", ["W5.bottom.cone_then_flat"], False,
         {"box": [0, 0, 0, 40, 40, 20]})

    s35 = fuse(cut(box(0, 0, 0, 40, 40, 20), cyl((20, 20, 12), UP, 5, 9)), cyl((20, 20, 12), UP, 2, 4))
    part("p35_annular_bottom", s35,
         Features([Hole((20, 20, 20), DOWN, [(10, 8)], post_diameter=4)], [Shaft((20, 20, 12), UP, 4, 4)],
                  planes=box_planes(0, 0, 0, 40, 40, 20) + [Plane(UP, 12), Plane(UP, 16)]),
         "a dia 10 blind hole around a dia 4 post rising from its floor", ["W5.bottom.annular", "W4.post"], False,
         {"box": [0, 0, 0, 40, 40, 20]})

    t30 = math.tan(math.radians(30))
    wedge = (cq.Workplane("YZ").polyline([(-10, 5 - 30 * t30), (50, 5 + 30 * t30), (50, -50), (-10, -50)])
             .close().extrude(60).translate((-10, 0, 0)))
    s36 = cut(box(0, 0, 0, 40, 40, 20), wedge.val().wrapped, cyl((20, 20, -10), UP, 3, 40))
    part("p36_inclined_exit", s36,
         Features([Hole((20, 20, 20), DOWN, [(6, 20)], kind="unknown", far_end="irregular")],
                  planes=[Plane(UP, 20), Plane(DOWN, 0), Plane((1, 0, 0), 40), Plane((-1, 0, 0), 0),
                          Plane((0, -1, 0), 0), Plane((0, 1, 0), 40),
                          Plane((0, 0.5, -math.cos(math.radians(30))), 20 * 0.5 - 5 * math.cos(math.radians(30)))]),
         "a dia 6 hole whose far end meets a 30 deg face: one opening, one irregular end", ["F4.one_irregular"],
         False, {"box": [0, 0, 0, 40, 40, 20], "underside": "a 30 deg plane through (y 20, z 5)"})

    s37 = cut(s05, box(30, -1, 8.5, 11, 42, 7))
    f37 = Features([h05], planes=[Plane(UP, 15), Plane(DOWN, 0), Plane((1, 0, 0), 40), Plane((-1, 0, 0), 0),
                                  Plane((0, 1, 0), 40), Plane((0, -1, 0), 0), Plane(UP, 8.5, faces=2),
                                  Plane((1, 0, 0), 30)])
    part("p37_cbore_shoulder", s37, f37,
         "p05 with a milled shoulder coplanar with the counterbore floor: the walk group is local (W1)", ["W1", "7.2"],
         False, {**c05, "shoulder": "x 30 to 40 above z 8.5"})

    # One extruded profile, so that each side is one face (a fuse of two
    # boxes would leave two coplanar faces on three sides).
    step = cq.Workplane("XZ").polyline([(0, 0), (20, 0), (20, 12), (10, 12), (10, 20), (0, 20)]).close().extrude(-20)
    s38 = cut(step.val().wrapped, box(2, 5, 12, 6, 10, 4))
    part("p38_void_coplanar", s38,
         Features(planes=[Plane(UP, 20), Plane(UP, 12), Plane((1, 0, 0), 10), Plane(DOWN, 0),
                          Plane((-1, 0, 0), 0), Plane((1, 0, 0), 20), Plane((0, -1, 0), 0), Plane((0, 1, 0), 20),
                          Plane(UP, 12, shell=1), Plane(DOWN, -16, shell=1), Plane((1, 0, 0), 2, shell=1),
                          Plane((-1, 0, 0), -8, shell=1), Plane((0, 1, 0), 5, shell=1),
                          Plane((0, -1, 0), -15, shell=1)]),
         "a box void whose floor is coplanar with a step top: two planes, one per shell", ["7.2", "S3"], False,
         {"block": "20 x 20 x 12 with a 10 x 20 x 8 step", "void": "x 2 to 8, y 5 to 15, z 12 to 16"})

    # Parts whose ends must read as unknown, never as through. In u01 and
    # u02 a cone widens from the dia 10 bore straight onto a flat floor, so
    # the walk crosses a cone that says air, then a plane that says
    # material (W3, section 18 C2).
    def chamber(floor):
        top = cyl((20, 20, 10), UP, 5, 21)
        widen = cone((20, 20, 10), DOWN, 5, 8, 3)
        return floor(cut(box(0, 0, 0, 40, 40, 30), fuse(top, widen)))

    bore = Hole((20, 20, 30), DOWN, [(10, 20)], kind="unknown", far_end="unknown", reason="air_then_material")
    s_u1 = chamber(lambda s: cut(s, cyl((25, 20, 5), UP, 1, 2.5)))
    part("u01_undercut_floor", s_u1,
         Features([bore, Hole((25, 20, 7), DOWN, [(2, 2)])],
                  planes=box_planes(0, 0, 0, 40, 40, 30) + [Plane(UP, 7), Plane(UP, 5)], cones=1),
         "a dia 10 hole widening by a cone onto a flat floor that holds an off-axis dia 2 hole: never through (C2)",
         ["W3.plane_agreement"], True, {"box": [0, 0, 0, 40, 40, 30], "cone": "dia 10 at z 10 to dia 16 at z 7"})
    s_u2 = chamber(lambda s: fuse(s, cyl((20, 20, 7), UP, 2, 3)))
    part("u02_undercut_post", s_u2,
         Features([bore], [Shaft((20, 20, 7), UP, 4, 3)],
                  planes=box_planes(0, 0, 0, 40, 40, 30) + [Plane(UP, 7), Plane(UP, 10)], cones=1),
         "as u01, the floor an annulus around a dia 4 post: never through", ["W3.plane_agreement"], False,
         {"box": [0, 0, 0, 40, 40, 30], "post": "dia 4, z 7 to 10"})
    # One extruded profile, so that each side is one face.
    l_block = cq.Workplane("XZ").polyline([(0, 0), (40, 0), (40, 20), (20, 20), (20, 10), (0, 10)]).close().extrude(-30)
    s_u3 = cut(l_block.val().wrapped, cyl((20, 15, -1), UP, 3, 11))
    part("u03_half_capped", s_u3,
         Features([Hole((20, 15, 0), UP, [(6, 10)], kind="unknown", far_end="unknown", reason="groups_beyond")],
                  planes=[Plane(DOWN, 0), Plane(UP, 10), Plane(UP, 20), Plane((-1, 0, 0), -20), Plane((-1, 0, 0), 0),
                          Plane((1, 0, 0), 40), Plane((0, -1, 0), 0), Plane((0, 1, 0), 30), Plane(DOWN, -10)]),
         "a dia 6 hole half open, half capped at its top: unknown whatever the face order (H1)", ["G3", "W1"], False,
         {"block": "x 0 to 20 at z 10, x 20 to 40 at z 20", "hole": "dia 6 at (20, 15), z 0 to 10"})
    s_u4 = cut(box(0, 0, 0, 30, 30, 20), cyl((15, 15, 10), UP, 3, 11),
               BRepPrimAPI_MakeSphere(gp_Pnt(15, 15, 10), 3).Shape())
    part("u04_ball_end", s_u4,
         Features([Hole((15, 15, 20), DOWN, [(6, 10)], kind="unknown", far_end="unknown", reason="first_face")],
                  planes=box_planes(0, 0, 0, 30, 30, 20), other_faces=1),
         "a dia 6 hole ending in a hemisphere: its bottom is unknown", ["W4.sphere", "W3.other"], False,
         {"box": [0, 0, 0, 30, 30, 20]})
    # The dia 3 bore at z 6.4 spans z 4.9 to 7.9: it pierces the drill point
    # (z 5.597 to 8) but not the dia 8 wall above it, and a strip of its own
    # wall survives below the tip, so it is one through hole.
    s_u5 = cut(cut(box(0, 0, 0, 40, 40, 20), tool(h04)), cyl((-1, 20, 6.4), (1, 0, 0), 1.5, 42))
    part("u05_drill_pierced", s_u5,
         Features([Hole((20, 20, 20), DOWN, [(8, 12)], kind="unknown", far_end="unknown", reason="transition"),
                   Hole((0, 20, 6.4), (1, 0, 0), [(3, 40)], through=40)],
                  planes=box_planes(0, 0, 0, 40, 40, 20), cones=1),
         "p04 with a dia 3 cross bore through its drill point: not a closed point (L1)", ["W4.apex", "W4.transition"], False,
         {"from": "p04", "cross bore": "dia 3 along x at z 6.4"})
    s_u6 = cut(box(0, 0, 0, 40, 40, 20), cyl((20, 20, 12), UP, 5, 9), cyl((23, 20, 10), UP, 1, 2.5))
    part("u06_general_face", s_u6,
         Features([Hole((20, 20, 20), DOWN, [(10, 8)], kind="unknown", far_end="unknown", reason="combination"),
                   Hole((23, 20, 12), DOWN, [(2, 2)])],
                  planes=box_planes(0, 0, 0, 40, 40, 20) + [Plane(UP, 12), Plane(UP, 10)]),
         "a dia 10 blind hole whose floor also holds an off-axis dia 2 hole: its bottom is unknown (W5)", ["W5.unknown"],
         False, {"box": [0, 0, 0, 40, 40, 20]})

    # A dia 10 chamber z 10 to 20 inside a block, reached only by a dia 4
    # cross bore along x: a hole closed at both ends in the outer shell
    # (built by the review of PR 8).
    s39 = cut(box(0, 0, 0, 40, 40, 30), cyl((20, 20, 10), UP, 5, 10), cyl((-1, 20, 15), (1, 0, 0), 2, 42))
    e39 = accepted(Features([Hole((0, 20, 15), (1, 0, 0), [(4, 40)], kind="unknown", far_end="irregular"),
                             Hole((40, 20, 15), (-1, 0, 0), [(4, 40)], kind="unknown", far_end="irregular")],
                            planes=box_planes(0, 0, 0, 40, 40, 30) + [Plane(UP, 10), Plane(DOWN, -20)]))
    e39["features"]["holes"].insert(0, {
        "kind": "enclosed", "axis_point": [20.0, 20.0, 10.0], "axis_direction": [0.0, 0.0, 1.0],
        "segments": [[10.0, 0.0, 10.0]],
        "ends": [{"kind": "bottom", "bottom": "flat", "at": 0.0}, {"kind": "bottom", "bottom": "flat", "at": 10.0}]})
    part("p39_enclosed", s39, e39, "a chamber reached only by a cross bore: a hole closed at both ends",
         ["K2.enclosed", "W5.bottom.flat"], False, {"box": [0, 0, 0, 40, 40, 30], "chamber": "dia 10, z 10 to 20",
                                                  "cross bore": "dia 4 along x at (y 20, z 15)"})
    # A dia 4 bore under ten stacked cones that widen, by turns, 1 and 0.5
    # mm per mm: a walk crosses more than W7's eight transitions.
    t40, r40 = cyl((20, 20, -1), UP, 2, 32), 2.0
    for k in range(10):
        wider = r40 + (0.5 if k % 2 else 1.0)
        t40 = fuse(t40, BRepPrimAPI_MakeCone(gp_Ax2(gp_Pnt(20, 20, 20 + k), gp_Dir(0, 0, 1)), r40, wider, 1).Shape())
        r40 = wider
    part("p40_many_cones", cut(box(0, 0, 0, 40, 40, 30), fuse(t40, cyl((20, 20, 30), UP, r40, 1))),
         Features([Hole((20, 20, 0), UP, [(4, 20)], kind="unknown", far_end="unknown", reason="too_long")],
                  planes=box_planes(0, 0, 0, 40, 40, 30), cones=10),
         "a bore under ten stacked cones: the walk stops at its bound", ["W7"], False,
         {"box": [0, 0, 0, 40, 40, 30], "cones": "ten, from dia 4 at z 20, 1 mm each, widening 1 and 0.5 by turns"})

    # Refusals.
    w.shape(None, "r03_two_solids", roots=[compound(box(0, 0, 0, 10, 10, 10), box(20, 0, 0, 10, 10, 10))])
    w.case("r03_two_solids", "a compound of two cubes, one product (assembly mode off)", ["4.2#10"], True,
           {"cubes": "10 mm at x 0 and 20"}, refused(3, "several_solids"))
    asm = cq.Assembly(name="ASM")
    asm.add(cq.Workplane().add(cq.Shape.cast(box(0, 0, 0, 10, 10, 10))), name="A")
    asm.add(cq.Workplane().add(cq.Shape.cast(box(0, 0, 0, 10, 10, 10))), name="B", loc=cq.Location(cq.Vector(20, 0, 0)))
    asm.export(str(w.out / "r04_assembly.stp"), exportType="STEP", write_pcurves=False)
    Writer.fix_time(w.out / "r04_assembly.stp")
    w.case("r04_assembly", "a two-part assembly", ["4.2#5"], True, {"parts": "two 10 mm cubes"}, refused(2, "assembly"))

    f = Part21(p01_text)
    context = next(n for n, v in f.inst.items() if "GLOBAL_UNIT_ASSIGNED_CONTEXT" in v)
    m = re.search(r"GLOBAL_UNIT_ASSIGNED_CONTEXT\s*\(\(([^)]*)\)\)", f.inst[context])
    units = [u.strip() for u in m.group(1).split(",")]
    length = next(u for u in units if "LENGTH_UNIT" in f.inst[int(u[1:])])
    keep = ",".join(u for u in units if u != length)
    f.inst[context] = f.inst[context].replace(m.group(0), f"GLOBAL_UNIT_ASSIGNED_CONTEXT(({keep}))")
    w.text(f.text(), "r05_no_length_unit", "p01_through")
    w.case("r05_no_length_unit", f"p01 with its length unit {length} removed from the context (a text edit)",
           ["4.2#18"], True, {"from": "p01"}, refused(5, "length_unit_unreadable"))

    shell = TopExp_Explorer(s01, TopAbs_SHELL).Current()
    w.shape(shell, "r06_shell")
    w.case("r06_shell", "p01's closed shell written alone, as a surface model", ["4.2#9", "4.2#11"], True,
           {"from": "p01"}, refused(3, "no_solid", "unread_shape_item"))

    if not unit_only:
        w.shape(compound(s01, TopExp_Explorer(box(50, 0, 0, 5, 5, 5), TopAbs_SHELL).Current()), "r08_surface_model")
        w.case("r08_surface_model", "p01 with a shell-based surface model beside the solid (R2)", ["4.2#11"], False,
               {"from": "p01", "shell": "a 5 mm cube's shell at x 50"}, refused(3, "unread_shape_item"))
        w.shape(None, "r09_two_definitions", roots=[box(0, 0, 0, 10, 10, 10), box(20, 0, 0, 10, 10, 10)])
        w.case("r09_two_definitions", "two products and no assembly usage", ["4.2#7"], False,
               {"cubes": "10 mm, written as two roots"}, refused(2, "several_definitions"))

        f = Part21(p01_text)
        vertex = next(n for n in f.find("CARTESIAN_POINT") if f.point(n) == [0.0, 0.0, 8.0]
                      and any(f.inst[v] == f"VERTEX_POINT('',#{n})" for v in f.find("VERTEX_POINT")))
        f.inst[vertex] = "CARTESIAN_POINT('',(0.,0.,8.1))"
        w.text(f.text(), "r10_off_surface", "p01_through")
        w.case("r10_off_surface", "p01 with the vertex at (0, 0, 8) moved 0.1 mm off its top face (a text edit)",
               ["4.2#23", "4.2#24"], False, {"from": "p01"}, refused(6, "vertex_off_surface", "vertex_off_curve"))

        f = Part21(p01_text)
        top = f.plane_face(UP, 8)
        f.inst[top] = flip_sense(f.inst[top])
        w.text(f.text(), "r11_flipped_plane", "p01_through")
        w.case("r11_flipped_plane", "p01 with its top face's same_sense flipped (a text edit)", ["4.2#25"], False,
               {"from": "p01"}, refused(6, "face_orientation"))

    f = Part21(p01_text)
    wall = next(n for n in f.find("ADVANCED_FACE") if f.inst[f.refs(n)[-1]].startswith("CYLINDRICAL_SURFACE"))
    f.inst[wall] = flip_sense(f.inst[wall])
    w.text(f.text(), "r12_flipped_cylinder", "p01_through")
    w.case("r12_flipped_cylinder", "p01 with its hole face's same_sense flipped (a text edit): not a dia 6.6 pin (C1)",
           ["4.2#25", "4.2#25b"], True, {"from": "p01"}, refused(6, "face_orientation"))


# ------------------------------------------------------------- assemblies
#
# docs/research/m3-socket.md, section 13. Each assembly is a tree of Leaf
# parts and Node assemblies, each child placed by a 4x4 matrix; one Python
# object is one product however often it is placed. It is written through
# XCAF directly, so that product names and shared sub-assemblies are exactly
# the construction's, and the expected occurrence transforms come from the
# same tree, multiplied in pure Python.


def m_motion(m: Motion):
    """The 4x4 matrix of a Motion: rotation by `angle` about `axis`, then the shift."""
    cols = [m.vector(e) for e in ((1, 0, 0), (0, 1, 0), (0, 0, 1))]
    return [[cols[0][i], cols[1][i], cols[2][i], m.shift[i]] for i in range(3)] + [[0.0, 0.0, 0.0, 1.0]]


def m_tr(v):
    return m_motion(Motion((0, 0, 1), 0.0, tuple(float(c) for c in v)))


def m_rot(axis, deg, shift=(0, 0, 0)):
    return m_motion(Motion(axis, deg, shift))


def m_mul(*ms):
    out = [[float(i == j) for j in range(4)] for i in range(4)]
    for m in ms:
        out = [[sum(out[i][k] * m[k][j] for k in range(4)) for j in range(4)] for i in range(4)]
    return out


def m_inv(m):
    """The inverse of a rigid 4x4 matrix."""
    rt = [[m[j][i] for j in range(3)] for i in range(3)]
    t = [-sum(rt[i][k] * m[k][3] for k in range(3)) for i in range(3)]
    return [rt[i] + [t[i]] for i in range(3)] + [[0.0, 0.0, 0.0, 1.0]]


I4 = m_tr((0, 0, 0))


@dataclass(eq=False)
class Leaf:
    """A part: one product."""
    name: str
    shape: object


@dataclass(eq=False)
class Node:
    """An assembly: one product whose children are (Leaf or Node, 4x4 matrix)."""
    name: str
    children: list = field(default_factory=list)

    def add(self, child, matrix=None):
        self.children.append((child, matrix if matrix is not None else I4))
        return self


def occurrences(node, at=None, out=None):
    """{product name: [3x4 rows]} for every leaf occurrence under `node`."""
    at = I4 if at is None else at
    out = {} if out is None else out
    for child, matrix in node.children:
        placed = m_mul(at, matrix)
        if isinstance(child, Leaf):
            out.setdefault(child.name, []).append([[r(x) for x in row] for row in placed[:3]])
        else:
            occurrences(child, placed, out)
    return out


def quantities(root):
    """{(name, kind): quantity} for every definition under `root`, the root included."""
    out = {(root.name, "assembly"): 1}

    def walk(node):
        for child, _ in node.children:
            kind = "part" if isinstance(child, Leaf) else "assembly"
            out[(child.name, kind)] = out.get((child.name, kind), 0) + 1
            if isinstance(child, Node):
                walk(child)

    walk(root)
    return out


def listing(root):
    """The `parts` expected of `root`: id, name, kind and quantity, by name."""
    return [{"id": name, "name": name, "kind": kind, "quantity": n}
            for (name, kind), n in sorted(quantities(root).items())]


def write_xcaf(w: Writer, root: Node, name: str, pcurves=False):
    """Writes the tree under `root` as one STEP assembly: every Leaf and
    Node object one product, named after it, every child one usage."""
    from OCP.STEPCAFControl import STEPCAFControl_Writer
    from OCP.TCollection import TCollection_ExtendedString
    from OCP.TDataStd import TDataStd_Name
    from OCP.TDocStd import TDocStd_Document
    from OCP.TopLoc import TopLoc_Location
    from OCP.XCAFDoc import XCAFDoc_DocumentTool

    doc = TDocStd_Document(TCollection_ExtendedString("XmlOcaf"))
    tool = XCAFDoc_DocumentTool.ShapeTool_s(doc.Main())
    labels = {}

    def label(item):
        if id(item) in labels:
            return labels[id(item)]
        if isinstance(item, Leaf):
            made = tool.AddShape(item.shape, False)
        else:
            made = tool.NewShape()
            for child, matrix in item.children:
                t = gp_Trsf()
                t.SetValues(*[matrix[i][j] for i in range(3) for j in range(4)])
                tool.AddComponent(made, label(child), TopLoc_Location(t))
        TDataStd_Name.Set_s(made, TCollection_ExtendedString(item.name))
        labels[id(item)] = made
        return made

    roots = root if isinstance(root, list) else [root]
    for item in roots:
        label(item)
    tool.UpdateAssemblies()
    # The writer first: its first construction initialises the STEP
    # statics, which would undo settings made before it.
    writer = STEPCAFControl_Writer()
    Interface_Static.SetIVal_s("write.surfacecurve.mode", 1 if pcurves else 0)
    Interface_Static.SetIVal_s("write.step.assembly", 1)
    Interface_Static.SetCVal_s("xstep.cascade.unit", "MM")
    Interface_Static.SetCVal_s("write.step.unit", "MM")
    if not writer.Transfer(doc, STEPControl_AsIs):
        raise RuntimeError(f"{name}: transfer failed")
    path = w.out / f"{name}.stp"
    if writer.Write(str(path)) != IFSelect_RetDone:
        raise OSError(f"{name}: write failed")
    fix_assembly(path)
    return path


def fix_assembly(path: Path):
    """A fixed time stamp, and usage ids renumbered from 1 in file order:
    Open CASCADE numbers them with a counter that runs across the whole
    process, so a case written alone would differ."""
    Writer.fix_time(path)
    text = path.read_text(encoding="latin-1")
    k = iter(range(1, 10 ** 6))
    text = re.sub(r"(NEXT_ASSEMBLY_USAGE_OCCURRENCE\s*\(\s*')[^']*(')",
                  lambda m: f"{m.group(1)}{next(k)}{m.group(2)}", text)
    path.write_text(text, encoding="latin-1", newline="\n")


def hex_prism(af, z0, h):
    """A hexagonal prism across flats `af` (two flats normal to y), from z0 up by h."""
    from OCP.BRepBuilderAPI import BRepBuilderAPI_MakeFace, BRepBuilderAPI_MakePolygon
    from OCP.BRepPrimAPI import BRepPrimAPI_MakePrism
    rad = af / math.sqrt(3)
    poly = BRepBuilderAPI_MakePolygon()
    for k in range(6):
        a = math.radians(60 * k)
        poly.Add(gp_Pnt(rad * math.cos(a), rad * math.sin(a), z0))
    poly.Close()
    face = BRepBuilderAPI_MakeFace(poly.Wire()).Face()
    return BRepPrimAPI_MakePrism(face, gp_Vec(0, 0, h)).Shape()


def through(shape, pts, d, z0, z1):
    """`shape` with through holes of diameter d at `pts`, cut from z0 - 1 to z1 + 1."""
    return cut(shape, *[cyl((x, y, z0 - 1), UP, d / 2, z1 - z0 + 2) for x, y in pts])


PTS = [(-30, -15), (30, -15), (-30, 15), (30, 15)]


def a_plate(lx=100, ly=60, t=10, d=6.6, pts=PTS):
    """A plate centred in x and y, top face at z = 0, through holes at `pts`."""
    return through(box(-lx / 2, -ly / 2, -t, lx, ly, t), pts, d, -t, 0)


def a_bracket():
    """80 x 50 x 8, bottom face at z = 0: four dia 6.6 holes, one dia 8."""
    return through(through(box(-40, -25, 0, 80, 50, 8), PTS, 6.6, 0, 8), [(0, 0)], 8, 0, 8)


def a_hex_bolt(length=25, d=6):
    """Hex head (across flats 10) z 0 to 4, its bearing face at z = 0; shank z -length to 0."""
    return fuse(hex_prism(10, 0, 4), cyl((0, 0, -length), UP, d / 2, length))


def a_hex_nut():
    """Across flats 10, z -5 to 0 (its bearing face at z = 0); a dia 5 hole."""
    return through(hex_prism(10, -5, 5), [(0, 0)], 5, -5, 0)


def a_shcs(length=16):
    """Socket head cap screw without its socket: head dia 10, z 0 to 6; shank dia 6 down to -length."""
    return fuse(cyl((0, 0, 0), UP, 5, 6), cyl((0, 0, -length), UP, 3, length))


def bolted(name, bracket=None, bolt=None, nut=None):
    """A bracket on nothing, four bolts through it at z = 8 and four nuts at z = -10."""
    sub = Node(name)
    sub.add(bracket or Leaf("BRACKET", a_bracket()))
    bolt = bolt or Leaf("BOLT", a_hex_bolt())
    nut = nut or Leaf("NUT", a_hex_nut())
    for x, y in PTS:
        sub.add(bolt, m_tr((x, y, 8)))
        sub.add(nut, m_tr((x, y, -10)))
    return sub


def asm_expect(root, select=(), listing_result=None, gate=None):
    """The answers of an assembly case: its parts list, every leaf
    occurrence's transform, the selections to try, and the assembly gate."""
    expect = {"listing": listing_result or {"result": "accepted", "path_form": "usage_id", "parts": listing(root)},
              "assembly_gate": gate or {"result": "accepted"},
              "select": list(select)}
    if expect["listing"]["result"] == "accepted":
        expect["occurrences"] = occurrences(root)
    return expect


class Asm:
    """Writes assembly cases."""

    def __init__(self, w: Writer, which):
        self.w, self.which = w, which

    def case(self, name, root, tests, rules, unit_set, construction, expect, edit=None):
        if not self.which(unit_set):
            return None
        path = write_xcaf(self.w, root, name)
        if edit is not None:
            f = Part21(path.read_text(encoding="latin-1"))
            edit(f)
            path.write_text(f.text(), encoding="latin-1", newline="\n")
            Writer.fix_time(path)
        self.w.cases.append({"id": name, "file": f"{name}.stp", "kind": "assembly",
                             "set": "unit" if unit_set else "corpus", "tests": tests, "rules": rules,
                             "construction": construction, "expect": expect})
        return path


def usage_of(f: Part21, child_name):
    """The usage (NAUO) whose child is the product named `child_name`."""
    product = next(n for n in f.find("PRODUCT(") if f.inst[n].startswith(f"PRODUCT('{child_name}'"))
    formation = next(n for n in f.find("PRODUCT_DEFINITION_FORMATION") if product in f.refs(n))
    definition = next(n for n in f.find("PRODUCT_DEFINITION(") if formation in f.refs(n))
    return next(n for n in f.find("NEXT_ASSEMBLY_USAGE_OCCURRENCE") if f.refs(n)[1] == definition)


def placement_of(f: Part21, usage):
    """The CDSR, relationship and item_defined_transformation that place `usage`."""
    pds = next(n for n in f.find("PRODUCT_DEFINITION_SHAPE") if f.refs(n) == [usage])
    cdsr = next(n for n in f.find("CONTEXT_DEPENDENT_SHAPE_REPRESENTATION") if f.refs(n)[1] == pds)
    relationship = f.refs(cdsr)[0]
    idt = next(ref for ref in f.refs(relationship) if f.inst[ref].startswith("ITEM_DEFINED_TRANSFORMATION"))
    return cdsr, relationship, idt


def component_item(f: Part21, usage):
    """The transform item of `usage`'s placement that lies in the
    component's representation, and that representation."""
    _, relationship, idt = placement_of(f, usage)
    rep_1, rep_2 = [ref for ref in f.refs(relationship) if not f.inst[ref].startswith("ITEM_DEFINED")][:2]
    item_1, item_2 = f.refs(idt)
    child = f.refs(usage)[1]
    pds = next(n for n in f.find("PRODUCT_DEFINITION_SHAPE") if f.refs(n) == [child])
    sdr = next(n for n in f.find("SHAPE_DEFINITION_REPRESENTATION") if f.refs(n)[0] == pds)
    component_rep = f.refs(sdr)[1]
    if rep_1 == component_rep:
        return idt, item_1, rep_1
    return idt, item_2, rep_2


def replace_component_item(f: Part21, usage, value):
    """Gives `usage` a component-side transform item of its own, `value`:
    Open CASCADE shares one identity frame among every usage, so editing
    that frame would move them all."""
    idt, item, rep = component_item(f, usage)
    n = f.add(value)
    f.inst[idt] = re.sub(rf"#{item}\b", f"#{n}", f.inst[idt], count=1)
    f.inst[rep] = re.sub(rf"#{item}\b", f"#{item},#{n}", f.inst[rep], count=1)


def build_assemblies(w: Writer, which):
    """Every assembly case `which(unit_set)` asks for."""
    a = Asm(w, which)
    pts8 = PTS + [(x + 100, y) for x, y in PTS]

    # s01: the bracket bolted to a plate; its dia 8 hole has nothing on its axis.
    s01 = Node("s01_bolted").add(Leaf("PLATE", a_plate()))
    bracket, bolt, nut = Leaf("BRACKET", a_bracket()), Leaf("BOLT", a_hex_bolt()), Leaf("NUT", a_hex_nut())
    s01.add(bracket)
    for x, y in PTS:
        s01.add(bolt, m_tr((x, y, 8)))
        s01.add(nut, m_tr((x, y, -10)))
    a.case("s01_bolted", s01, "a bracket bolted to a plate with four hex bolts and nuts", ["T3", "T5", "O1", "O4", "O5", "O6",
           "P2", "m3.4.1#32", "m3.4.1#33", "m3.4.1#35"], True,
           {"plate": [100, 60, 10], "bracket": [80, 50, 8], "holes": 6.6, "pts": PTS,
            "bolt": {"af": 10, "head": 4, "d": 6, "length": 25, "at_z": 8}, "nut": {"af": 10, "h": 5, "d": 5, "at_z": -10}},
           asm_expect(s01, select=[
               {"part": "BRACKET", "result": "BRACKET", "occurrences": 1, "matched_by": "product_id"},
               {"part": "BOLT", "result": "BOLT", "occurrences": 4, "matched_by": "product_id"},
               {"part": "NOPE", "error": "part_not_found", "suggestions": []},
               {"part": "bolt", "error": "part_not_found", "suggestions": ["BOLT"]},
               {"part": "s01_bolted", "error": "part_is_assembly"},
               {"part": "BRACKET", "occurrence_of": "BOLT", "error": "occurrence_mismatch"},
               {"occurrence_of": "BRACKET", "result": "BRACKET", "occurrences": 1, "matched_by": "occurrence"},
               {"part": "BRACKET", "occurrence": "nowhere", "error": "occurrence_not_found"},
               {"error": "nothing_selected"},
           ]))

    # s02: a counterbored block on a base, a socket head cap screw into the base's dia 5 blind hole.
    s02 = Node("s02_cbore")
    base = cut(box(-30, -30, -10, 60, 60, 10), cyl((0, 0, -9), UP, 2.5, 10))
    block = cut(box(-20, -20, 0, 40, 40, 15), cyl((0, 0, -1), UP, 3.3, 17), cyl((0, 0, 8.5), UP, 5.5, 7.5))
    s02.add(Leaf("BASE", base)).add(Leaf("BLOCK", block)).add(Leaf("SCREW", a_shcs()), m_tr((0, 0, 8.5)))
    c02 = {"base": "60 x 60 x 10, top at z 0, dia 5 blind 9 deep", "block": "40 x 40 x 15, dia 11 x 6.5 over dia 6.6",
           "screw": "head dia 10 x 6, shank dia 6 x 16, at z 8.5"}
    a.case("s02_cbore", s02, "a counterbored block screwed to a base", ["T3", "P2"], True, c02,
           asm_expect(s02, select=[{"part": "BLOCK", "result": "BLOCK", "occurrences": 1, "matched_by": "product_id"}]))

    # s03: a plate on four dowels in a base: holes 5.97 (press), 6.2 (clearance), 6 and 6.015 (line to line at tau).
    xs = (-30, -10, 10, 30)
    holes03 = (5.97, 6.2, 6.0, 6.015)
    plate03 = box(-40, -20, 0, 80, 40, 10)
    for x, d in zip(xs, holes03):
        plate03 = through(plate03, [(x, 0)], d, 0, 10)
    base03 = cut(box(-50, -30, -10, 100, 60, 10), *[cyl((x, 0, -8), UP, 3.0, 9) for x in xs])
    s03 = Node("s03_dowels").add(Leaf("BASE", base03)).add(Leaf("PLATE", plate03))
    dowel = Leaf("DOWEL", cyl((0, 0, 0), UP, 3.0, 16))
    for x in xs:
        s03.add(dowel, m_tr((x, 0, -8)))
    a.case("s03_dowels", s03, "a plate on four dowels: press, clearance and two line-to-line fits", ["T3", "P2"], True,
           {"plate": [80, 40, 10], "holes": list(zip(xs, holes03)), "dowel": "dia 6 x 16 from z -8",
            "base": "100 x 60 x 10, dia 6 blind 8 deep"},
           asm_expect(s03, select=[{"part": "DOWEL", "result": "DOWEL", "occurrences": 4, "matched_by": "product_id"}]))

    # s08x: one sub-assembly product (bracket, bolts, nuts) used twice, as AS1 does.
    long_plate = Leaf("PLATE", a_plate(lx=220, pts=[(x - 50, y) for x, y in pts8]))
    turned = m_rot((0, 0, 1), 180, (100, 0, 0))
    lb = bolted("LB_ASSEM")
    s08x = Node("s08x_nested").add(long_plate, m_tr((50, 0, 0))).add(lb).add(lb, turned)
    a.case("s08x_nested", s08x, "one sub-assembly product used twice: one definition, two paths",
           ["O1", "O4", "P1", "T1"], True,
           {"plate": "220 x 60 x 10 at x 50", "LB_ASSEM": "bracket, 4 bolts, 4 nuts", "second": "turned 180 about z, x + 100"},
           asm_expect(s08x, select=[
               {"part": "BRACKET", "result": "BRACKET", "occurrences": 2, "matched_by": "product_id"},
               {"part": "LB_ASSEM", "error": "part_is_assembly"},
               {"part": "BRACKET", "occurrence_of": "BRACKET", "result": "BRACKET", "occurrences": 1,
                "matched_by": "product_id"},
           ]))

    # s07: the same without a sub-assembly. s08: two sub-assembly products sharing their leaves.
    s07 = Node("s07_twice").add(long_plate, m_tr((50, 0, 0)))
    s07.add(bracket).add(bracket, turned)
    for x, y in pts8:
        s07.add(bolt, m_tr((x, y, 8)))
        s07.add(nut, m_tr((x, y, -10)))
    a.case("s07_twice", s07, "two brackets on one plate, the second turned 180 deg", ["T3", "O6"], False,
           {"from": "s08x, flat"}, asm_expect(s07, select=[
               {"part": "BRACKET", "result": "BRACKET", "occurrences": 2, "matched_by": "product_id"}]))
    s08 = Node("s08_nested").add(long_plate, m_tr((50, 0, 0)))
    s08.add(bolted("LB_A", bracket, bolt, nut)).add(bolted("LB_B", bracket, bolt, nut), turned)
    a.case("s08_nested", s08, "two sub-assembly products sharing their leaves", ["O1", "P1"], False,
           {"from": "s08x, with two sub-assembly products"}, asm_expect(s08, select=[
               {"part": "BRACKET", "result": "BRACKET", "occurrences": 2, "matched_by": "product_id"}]))

    # s23: a joint twelve levels deep, each level turned 30 deg about a skew axis.
    inner = Node("LEVEL_12").add(bracket).add(bolt, m_tr((-30, -15, 8)))
    for k in range(11, 0, -1):
        axis = (1, (k % 3) - 1 + 0.5, 2)
        inner = Node(f"LEVEL_{k}").add(inner, m_rot(axis, 30, (k * 7.0, -k * 3.0, k * 1.5)))
    s23 = Node("s23_deep").add(inner, m_rot((0, 1, 1), 30, (5, 5, 5)))
    a.case("s23_deep", s23, "a joint nested 12 levels deep under skew turns", ["P2", "P9"], False,
           {"levels": 12, "turn": "30 deg about (1, k mod 3 - 0.5, 2), shift (7k, -3k, 1.5k)"}, asm_expect(s23))

    # s25: s01 with two sibling usages sharing an id, so paths are written by instance (O2).
    def same_id(f: Part21):
        usages = f.find("NEXT_ASSEMBLY_USAGE_OCCURRENCE")
        f.inst[usages[1]] = re.sub(r"\('[^']*'", "('1'", f.inst[usages[1]], count=1)

    exp25 = asm_expect(s01, select=[{"part": "BRACKET", "result": "BRACKET", "occurrences": 1,
                                     "matched_by": "product_id"}])
    exp25["listing"]["path_form"] = "instance"
    a.case("s25_usage_ids", s01, "s01 with two sibling usages both given the id '1' (a text edit)", ["O2", "O3"], False,
           {"from": "s01"}, exp25, edit=same_id)

    # Refusals and ambiguities.
    s21 = Node("r21_name_ambiguous").add(long_plate, m_tr((50, 0, 0)))
    s21.add(Leaf("BRACKET-A", a_bracket())).add(Leaf("BRACKET-B", a_bracket()), turned)

    def rename(f: Part21):
        for n in f.find("PRODUCT("):
            f.inst[n] = re.sub(r"^PRODUCT\('(BRACKET-[AB])','[^']*'", r"PRODUCT('\1','BRACKET'", f.inst[n])

    exp21 = asm_expect(s21, select=[{"part": "BRACKET", "error": "part_ambiguous", "candidates": 2},
                                    {"part": "BRACKET-A", "result": "BRACKET-A", "occurrences": 1,
                                     "matched_by": "product_id"}])
    for p in exp21["listing"]["parts"]:
        if p["id"].startswith("BRACKET-"):
            p["name"] = "BRACKET"
    a.case("r21_name_ambiguous", s21, "two bracket products, ids BRACKET-A and BRACKET-B, both named BRACKET",
           ["T4"], False, {"from": "s07, without bolts"}, exp21, edit=rename)

    r32 = Node("r32_id_ambiguous")
    r32.add(Node("SIDE_A").add(Leaf("BOLT", a_hex_bolt())), m_tr((-30, 0, 0)))
    r32.add(Node("SIDE_B").add(Leaf("BOLT", a_hex_bolt(length=30, d=8))), m_tr((30, 0, 0)))
    exp32 = asm_expect(r32, select=[{"part": "BOLT", "error": "part_ambiguous", "candidates": 2}])
    exp32["listing"]["parts"] = sorted([{"id": "BOLT", "name": "BOLT", "kind": "part", "quantity": 1}] * 2 + [
        p for p in exp32["listing"]["parts"] if p["id"] != "BOLT"], key=lambda p: (p["id"], p["kind"]))
    exp32["occurrences"] = {"BOLT": occurrences(r32)["BOLT"]}
    a.case("r32_id_ambiguous", r32, "two different parts, both with PRODUCT.id BOLT, in two sub-assemblies", ["T3"],
           False, {"bolts": "M6 x 25 and M8 x 30"}, exp32)

    refused33 = {"result": "refused", "check": 33, "refusals": ["placement_unreadable"]}

    def unplaced(f: Part21):
        cdsr, _, _ = placement_of(f, usage_of(f, "SCREW"))
        del f.inst[cdsr]

    a.case("r24_unplaced", s02, "s02 without the screw usage's context_dependent_shape_representation", ["m3.4.1#33"], False,
           {"from": "s02"}, asm_expect(s02, listing_result=refused33), edit=unplaced)

    def divergent(f: Part21):
        point = f.add("CARTESIAN_POINT('',(10.,0.,0.))")
        z, x = f.add("DIRECTION('',(0.,0.,1.))"), f.add("DIRECTION('',(1.,0.,0.))")
        replace_component_item(f, usage_of(f, "SCREW"), f"AXIS2_PLACEMENT_3D('',#{point},#{z},#{x})")

    a.case("r25_divergent", s02, "s02 with the screw's component-side frame moved to (10, 0, 0)", ["m3.4.1#33"], False,
           {"from": "s02"}, asm_expect(s02, listing_result=refused33), edit=divergent)

    def mirrored(f: Part21):
        o = f.add("CARTESIAN_POINT('',(0.,0.,0.))")
        x = f.add("DIRECTION('',(-1.,0.,0.))")
        y = f.add("DIRECTION('',(0.,1.,0.))")
        z = f.add("DIRECTION('',(0.,0.,1.))")
        replace_component_item(f, usage_of(f, "SCREW"),
                               f"CARTESIAN_TRANSFORMATION_OPERATOR_3D('','',#{x},#{y},#{o},1.,#{z})")

    a.case("r26_mirrored", s02, "s02 with the screw's component-side frame a mirroring cartesian_transformation_operator_3d",
           ["m3.4.1#33", "P7"], False, {"from": "s02"}, asm_expect(s02, listing_result=refused33), edit=mirrored)

    def quantified(f: Part21):
        usage = usage_of(f, "SCREW")
        ident, name_ = re.match(r"NEXT_ASSEMBLY_USAGE_OCCURRENCE\('([^']*)','([^']*)'", f.inst[usage]).groups()
        parent, child = f.refs(usage)[:2]
        unit = next(n for n in f.inst if "LENGTH_UNIT" in f.inst[n])
        m = f.add(f"MEASURE_WITH_UNIT(COUNT_MEASURE(4.),#{unit})")
        f.inst[usage] = (f"(ASSEMBLY_COMPONENT_USAGE($)NEXT_ASSEMBLY_USAGE_OCCURRENCE()PRODUCT_DEFINITION_RELATIONSHIP("
                         f"'{ident}','{name_}','',#{parent},#{child})PRODUCT_DEFINITION_USAGE()"
                         f"QUANTIFIED_ASSEMBLY_COMPONENT_USAGE(#{m}))")

    a.case("r28_quantified", s02, "s02 with the screw usage quantified, quantity 4", ["m3.4.1#33"], False, {"from": "s02"},
           asm_expect(s02, listing_result=refused33), edit=quantified)

    two = [Node("ROOT_A").add(Leaf("PLATE", a_plate())), Node("ROOT_B").add(bolt, m_tr((0, 0, 8)))]
    path = None
    if which(False):
        path = write_xcaf(w, two, "r30_two_roots")
        w.cases.append({"id": "r30_two_roots", "file": "r30_two_roots.stp", "kind": "assembly", "set": "corpus",
                        "tests": "two root assemblies in one file", "rules": ["m3.4.1#32"],
                        "construction": {"roots": ["ROOT_A: a plate", "ROOT_B: a bolt"]},
                        "expect": {"listing": {"result": "refused", "check": 32, "refusals": ["several_roots"]},
                                   "assembly_gate": {"result": "refused"}, "select": []}})
    return path


# A number in a Part 21 file, as Open CASCADE writes them.
NUMBER = re.compile(r"[-+]?(?:\d+\.\d*|\.\d+|\d+)(?:[Ee][-+]?\d+)?")
# How far a written number may differ between platforms: the last digit of
# what Open CASCADE writes, and noise near zero from its trigonometry, are
# the platform's maths library's. Far below any test tolerance (1e-6 mm).
SAME_NUMBER = 1e-9


def differences(generated: Path, committed: Path):
    """How the files of `committed` differ from those just `generated`: the
    manifests must be identical, and each .stp the same text but for
    numbers within SAME_NUMBER. Returns one line per difference."""
    found = []
    names = sorted({p.name for p in generated.iterdir()} | {p.name for p in committed.iterdir()})
    for name in names:
        a, b = generated / name, committed / name
        if not a.exists() or not b.exists():
            found.append(f"{name}: only in {'the committed set' if b.exists() else 'the generated set'}")
            continue
        x, y = a.read_text(encoding="latin-1"), b.read_text(encoding="latin-1")
        if not name.endswith(".stp"):
            if x != y:
                found.append(f"{name}: differs")
            continue
        if NUMBER.sub("#", x) != NUMBER.sub("#", y):
            found.append(f"{name}: differs in more than its numbers")
            continue
        for u, v in zip(NUMBER.findall(x), NUMBER.findall(y)):
            if abs(float(u) - float(v)) > SAME_NUMBER:
                found.append(f"{name}: {u} against {v}")
                break
    return found


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("out", type=Path, help="the directory to write into")
    parser.add_argument("--unit-set", action="store_true", help="write only the committed unit set")
    parser.add_argument("--assembly-set", action="store_true",
                        help="write only the committed assembly set (crates/fitment-core/tests/assemblies)")
    parser.add_argument("--check", type=Path, metavar="DIR",
                        help="after writing, check that DIR holds the same files (numbers within 1e-9)")
    args = parser.parse_args()
    # Open CASCADE's progress messages are noise here.
    messenger = Message.DefaultMessenger_s()
    printers = messenger.Printers()
    for i in range(printers.Length(), 0, -1):
        messenger.RemovePrinter(printers.Value(i))

    w = Writer(args.out)
    if args.assembly_set:
        build_assemblies(w, lambda unit_set: unit_set)
    else:
        build(w, args.unit_set)
        if not args.unit_set:
            build_assemblies(w, lambda unit_set: True)
    committed = args.unit_set or args.assembly_set
    if committed:
        keep = {c["file"] for c in w.cases if c["set"] == "unit"}
        w.cases = [c for c in w.cases if c["set"] == "unit"]
        for path in args.out.glob("*.stp"):
            if path.name not in keep:
                path.unlink()
    if not committed:
        rule_ids = coverage(w.cases)
    manifest = {
        "format": "fitment.test-parts",
        "format_version": 1,
        "generator": {"script": "tools/gen-corpus.py", "cadquery": importlib.metadata.version("cadquery"),
                      "cadquery-ocp": importlib.metadata.version("cadquery-ocp")},
        "tolerance": {"linear_mm": 1e-6, "angular_rad": 1e-9},
        "cases": sorted(w.cases, key=lambda c: c["id"]),
    }
    if not committed:
        # The full corpus only: the committed unit set stays within its
        # budget, and CI checks coverage on the corpus.
        manifest = {**{k: v for k, v in manifest.items() if k != "cases"}, "rule_ids": rule_ids,
                    "cases": manifest["cases"]}
    # One case per line keeps the committed manifest small and its diffs
    # readable.
    head = json.dumps({k: v for k, v in manifest.items() if k != "cases"}, indent=1)
    cases = ",\n".join("  " + json.dumps(c, separators=(",", ":")) for c in manifest["cases"])
    text = head[:-2] + ',\n "cases": [\n' + cases + "\n ]\n}\n"
    json.loads(text)
    (args.out / "manifest.json").write_text(text, encoding="utf-8", newline="\n")
    if args.check is not None:
        found = differences(args.out, args.check)
        for line in found:
            print(line)
        if found:
            raise SystemExit(f"{args.check} is not what the generator writes: regenerate it with "
                             f"tools/gen-corpus.py {'--assembly-set' if args.assembly_set else '--unit-set'} {args.check}")
        print(f"{args.check} matches the generator")


if __name__ == "__main__":
    main()
