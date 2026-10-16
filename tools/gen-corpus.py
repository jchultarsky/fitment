#!/usr/bin/env -S uv run --locked --script
# /// script
# requires-python = ">=3.11,<3.14"
# dependencies = ["cadquery==2.8.0"]
# ///
"""Write fitment's test parts and the answers they must read as.

    tools/gen-corpus.py OUT_DIR              every case: the generated corpus
    tools/gen-corpus.py --unit-set OUT_DIR   only the committed unit set
    tools/gen-corpus.py --assembly-set OUT_DIR   only the committed assemblies
    tools/gen-corpus.py --candidate-set OUT_DIR  only the committed match candidates

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
from OCP.TopTools import TopTools_ListOfShape
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
    "m3.4.1#36": {"tests": [f"{M3_CORE}/assemblies.rs::a_snippet_assembly_reads",
                            f"{M3_CORE}/assemblies.rs::a_solid_two_definitions_share_is_refused_unread"]},
    "m3.4.1#37": {},
    "m3.4.1#38": {"tests": [f"{M3_CORE}/assemblies.rs::a_placement_far_out_of_range_is_refused",
                            f"{M3_CORE}/assemblies.rs::check_38_counts_the_part_s_reach_beside_the_translation",
                            f"{M3_CORE}/assemblies.rs::a_translation_of_1e17_mm_is_refused_without_panic"]},
    "m3.4.1#39": {"tests": [f"{M3_CORE}/hostile.rs::ten_thousand_coincident_joints_are_refused_by_check_39"]},
    "T1": {}, "T2": {"tests": [f"{M3_CORE}/assemblies.rs::a_definition_instance_selects_unless_an_id_reads_the_same"]},
    "T3": {}, "T4": {}, "T5": {}, "T6": {"tests": ["crates/fitment/tests/cli.rs::a_selection_error_names_the_candidates"]},
    "O1": {}, "O2": {}, "O3": {"tests": [f"{M3_CORE}/assemblies.rs::a_path_segment_that_names_two_usages_is_refused"]},
    "O4": {}, "O5": {}, "O6": {},
    "P2": {}, "P9": {},
    "P1": {},
    "P7": {"tests": ["crates/fitment-core/src/geom.rs::a_reflection_or_shear_is_not_a_rigid_transform"]},
    "P3": {},
    "P4": {}, "P5": {}, "P6": {},
    "P8": {},
    "Z1": {"tests": [f"{M3_CORE}/assemblies.rs::a_placement_floor_is_the_largest_its_path_states"]},
    "Z2": {"tests": [f"crates/fitment-core/src/socket/mod.rs::each_structural_rule_refuses_an_edit_with_its_digest_recomputed"]},
    "Z3": {}, "Z4": {}, "Z5": {},
    "Z6": {},
    "Z7": {"deferred": "holds by construction: the socket rules read M2's features and never change its constants"},
    "N1": {}, "N2": {}, "N3": {}, "N5": {},
    "N4": {"tests": [f"{M3_CORE}/hostile.rs::ten_thousand_bolts_are_read_in_reach_of_their_own_joints",
                     f"{M3_CORE}/hostile.rs::a_hole_with_a_thousand_coaxial_neighbours_is_read"]},
    "X1": {}, "X2": {}, "X3": {}, "X4": {}, "X5": {}, "X6": {}, "X7": {}, "X8": {},
    "B1": {}, "B2": {}, "B3": {}, "B5": {}, "B6": {}, "B7": {}, "B8": {},
    "B4": {},
    "I1": {}, "I2": {}, "I3": {},
    "I4": {},
    "I5": {}, "I6": {}, "I7": {},
    "J1": {}, "J2": {}, "J3": {},
    "U1": {},
    "U2": {}, "U3": {}, "U4": {}, "U8": {},
    "U5": {},
    "U6": {},
    "U7": {},
    "U9": {},
    "U10": {},
    "U11": {},
    "A1": {},
    "A2": {},
    "A3": {"tests": [f"crates/fitment-core/src/socket/mod.rs::each_structural_rule_refuses_an_edit_with_its_digest_recomputed"]},
    "Y1": {}, "Y3": {},
    "Y2": {"tests": [f"crates/fitment-core/src/socket/mod.rs::each_structural_rule_refuses_an_edit_with_its_digest_recomputed"]},
    # Y4's two halves: the original witnesses every verifiable requirement (the read-back's clearance bounds,
    # the corpus's sockets from the construction, the oracle's seats and grips on the part's own faces) and fails
    # exactly the unverifiable ones, which M4 shows by running it as a candidate.
    "Y4.witness": {"tests": ["crates/fitment-core/src/socket/mod.rs::a_clearance_looser_than_its_sources_margins_is_refused",
                             "crates/fitment/tests/corpus.rs::every_corpus_assembly_reads_as_its_construction_says"]},
    "Y4.original_run": {"deferred": "M4 runs the original as a candidate and expects exactly its unverifiable "
                                    "requirements to fail"},
    "V1": {"deferred": "M7 checks the candidate's copies against each other"},
    "V2": {},
    "H1": {}, "H2": {},
    "Q1": {}, "Q2": {}, "Q3": {}, "L1": {}, "L2": {},
    "C1": {}, "C2": {}, "C3": {}, "C4": {}, "C5": {}, "C6": {},
})


# M4's rules (docs/research/m4-match.md), each deferred to the PR of section 22 that builds it until then; the
# coverage test allows M4's deferrals until M4 closes (PR 7).
M4_PR = {"CF": 2, "SG": 2, "SL": 2, "AL": 3, "VR": 3, "VD": 4, "EV": 4, "MF": 4, "KC": 5, "KH": 5, "KO": 6}
M4_IN_PR3 = {"KC2", "KC5", "KC15", "KH1", "KH2", "KH3", "KH10"}
M4_COUNTS = {"CF": 10, "SG": 11, "SL": 10, "AL": 14, "VR": 22, "VD": 15, "EV": 9, "MF": 11, "KC": 16, "KH": 14,
             "KO": 8}
for prefix, count in M4_COUNTS.items():
    for n in range(1, count + 1):
        rule = f"{prefix}{n}"
        RULES[rule] = {"deferred": f"M4 PR {3 if rule in M4_IN_PR3 else M4_PR[prefix]}"}
# The socket PR (M4 PR 1) builds MF11: a document of older socket rules read for its review alone.
RULES["MF11"] = {"tests": ["crates/fitment/tests/cli.rs::an_older_document_s_decisions_carry_over"]}
# PR 2 builds the candidate's signature, the socket's, the shortlist and its index; CF1, CF2 and CF8 are read by
# the matcher and verification (PR 3).
M4_CORE_SIG = "crates/fitment-core/src/matching/signature.rs"
M4_CORE_SL = "crates/fitment-core/src/matching/shortlist.rs"
M4_CORE_TESTS = "crates/fitment-core/tests/shortlist.rs"
M4_CLI_TESTS = "crates/fitment/tests/shortlist.rs"
M4_TESTS = {
    "CF3": [f"{M4_CLI_TESTS}::a_part_s_signature_does_not_depend_on_its_frame"],
    "CF4": [f"{M4_CORE_SIG}::unread_features_are_counted_and_unknown_ends_unbounded",
            f"{M4_CLI_TESTS}::a_signature_states_exactly_what_its_part_reads"],
    "CF5": [f"{M4_CORE_SIG}::unread_features_are_counted_and_unknown_ends_unbounded",
            f"{M4_CLI_TESTS}::a_signature_states_exactly_what_its_part_reads"],
    "CF6": [f"{M4_CLI_TESTS}::a_part_s_signature_does_not_depend_on_its_frame",
            f"{M4_CLI_TESTS}::a_signature_states_exactly_what_its_part_reads"],
    "CF7": [f"{M4_CLI_TESTS}::a_part_s_signature_does_not_depend_on_its_frame",
            f"{M4_CLI_TESTS}::a_signature_states_exactly_what_its_part_reads"],
    "CF9": [f"{M4_CLI_TESTS}::a_part_s_signature_does_not_depend_on_its_frame"],
    "CF10": [f"{M4_CORE_SIG}::unread_features_are_counted_and_unknown_ends_unbounded"],
    "SG1": [f"{M4_CLI_TESTS}::a_part_s_signature_does_not_depend_on_its_frame",
            f"{M4_CORE_SIG}::mutations_within_the_tolerance_are_kept_and_beyond_dropped"],
    "SG2": [f"{M4_CLI_TESTS}::a_part_s_signature_does_not_depend_on_its_frame"],
    "SG3": [f"{M4_CORE_SIG}::windows_follow_the_pin_and_the_clear_diameter"],
    "SG4": [f"{M4_CORE_SIG}::mutations_within_the_tolerance_are_kept_and_beyond_dropped",
            f"{M4_CORE_SIG}::a_tilted_seat_far_from_the_origin"],
    "SG5": [f"{M4_CORE_SIG}::near_requirements_group",
            f"{M4_CORE_SIG}::near_bores_served_by_two_stacks",
            f"{M4_CORE_SIG}::a_stepped_seat_under_a_coarse_policy"],
    "SG6": [f"{M4_CORE_SIG}::a_window_holds_its_ends",
            f"{M4_CORE_SIG}::mutations_within_the_tolerance_are_kept_and_beyond_dropped"],
    "SG7": [f"{M4_CORE_TESTS}::blocked_and_removed_requirements_demand_nothing"],
    "SG8": [f"{M4_CLI_TESTS}::every_original_is_kept_by_its_own_sockets"],
    "SG9": [f"{M4_CORE_SIG}::a_signature_is_deterministic"],
    "SG10": ["crates/fitment-core/src/matching/index.rs::bins_are_monotone_on_their_edges"],
    "SG11": [f"{M4_CORE_TESTS}::level_one_returns_whatever_level_two_keeps"],
    "SL1": [f"{M4_CORE_TESTS}::a_part_lacking_the_socket_s_elements_is_dropped"],
    "SL2": [f"{M4_CORE_TESTS}::a_part_lacking_the_socket_s_elements_is_dropped",
            f"{M4_CLI_TESTS}::every_original_is_kept_by_its_own_sockets"],
    "SL3": [f"{M4_CORE_SIG}::mutations_within_the_tolerance_are_kept_and_beyond_dropped"],
    "SL4": [f"{M4_CORE_SIG}::a_hostile_candidate_is_judged_within_the_budget"],
    "SL5": [f"{M4_CORE_SL}::distinct_requirements_need_distinct_witnesses",
            f"{M4_CORE_SIG}::one_plane_for_two_slightly_tilted_seats"],
    "SL6": [f"{M4_CORE_TESTS}::the_original_is_kept_by_its_own_sockets"],
    "SL7": [f"{M4_CLI_TESTS}::level_one_returns_whatever_level_two_keeps",
            f"{M4_CORE_SIG}::a_part_with_too_many_pairs_is_marked_incomplete"],
    "SL8": [f"{M4_CORE_TESTS}::level_one_returns_whatever_level_two_keeps",
            f"{M4_CORE_TESTS}::blocked_and_removed_requirements_demand_nothing"],
    "SL9": [f"{M4_CLI_TESTS}::level_one_returns_whatever_level_two_keeps",
            f"{M4_CORE_SIG}::level_one_holds_near_lines",
            f"{M4_CORE_TESTS}::level_one_returns_whatever_level_two_keeps"],
    "SL10": [f"{M4_CORE_SIG}::a_hostile_candidate_is_judged_within_the_budget"],
}
# PR 3a builds verification at a given placement: the candidate's stacks and profiles, VR1 to VR22 but the matcher's
# own (VR16, VR17, VR19), and Y4 at the identity. Alignment, the matcher and the committed candidates follow (3b, 3c).
M4_VERIFY = "crates/fitment-core/tests/verify.rs"
M4_ORIGINAL = "crates/fitment/tests/original.rs"
M4_CAND = "crates/fitment-core/src/matching/candidate.rs"
M4_Y4 = [f"{M4_ORIGINAL}::every_original_passes_what_m4_can_verify_at_the_identity",
         f"{M4_ORIGINAL}::every_pinned_fixture_original_passes_what_m4_can_verify_at_the_identity"]
M4_EDGE = [f"{M4_VERIFY}::a_placement_off_by_the_tolerance_rejects"]
M4_TESTS.update({
    "AL14": M4_EDGE + [f"{M4_VERIFY}::every_original_passes_its_own_sockets_at_the_identity"],
    "VR1": M4_Y4 + [f"{M4_VERIFY}::every_original_passes_its_own_sockets_at_the_identity"],
    "VR2": M4_EDGE + M4_Y4[1:],
    "VR3": M4_EDGE + [f"{M4_VERIFY}::a_turn_is_judged_by_where_it_moves_the_features"],
    "VR4": M4_Y4,
    "VR5": M4_Y4[1:],
    "VR6": M4_Y4,
    "VR7": M4_Y4 + M4_EDGE,
    "VR8": [f"{M4_CAND}::a_profile_takes_the_zones_whose_faces_stay_outside"] + M4_Y4,
    "VR9": M4_Y4,
    "VR10": M4_EDGE + M4_Y4,
    "VR11": ["crates/fitment-core/src/matching/region.rs::a_region_is_covered_to_within_the_tolerance",
             "crates/fitment-core/src/matching/region.rs::a_whole_circle_winds_once_from_inside",
             "crates/fitment-core/src/matching/region.rs::coverage_stops_at_its_bound"],
    "VR12": M4_Y4,
    "VR13": M4_Y4,
    "VR14": M4_Y4,
    "VR15": M4_Y4,
    "VR18": [f"{M4_CAND}::a_profile_takes_the_zones_whose_faces_stay_outside"],
    "VR20": ["crates/fitment-core/src/matching/evidence.rs::a_measure_passes_strictly_inside_its_limits"] + M4_EDGE,
    "VR21": M4_Y4,
    "VR22": [f"{M4_CAND}::a_core_meets_every_face_inside_it_but_the_stack_s_own",
             f"{M4_CAND}::a_cylinder_s_surface_is_boxed_within_the_part"] + M4_Y4,
})
# PR 3b builds alignment and the matcher: every original, aligned from scratch, fails exactly what no candidate
# can pass (KH10, Y4's original run).
M4_MATCHER = "crates/fitment-core/tests/matcher.rs"
M4_KH10 = [f"{M4_ORIGINAL}::every_original_fails_exactly_what_no_candidate_can_pass",
           f"{M4_ORIGINAL}::every_pinned_fixture_original_fails_exactly_what_no_candidate_can_pass",
           f"{M4_MATCHER}::each_original_fails_exactly_what_no_candidate_can_pass"]
M4_TESTS.update({
    "CF1": [f"{M4_MATCHER}::a_part_lacking_the_socket_s_elements_is_rejected_at_the_shortlist"] + M4_KH10[:1],
    "CF2": [f"{M4_MATCHER}::a_refused_file_is_a_reject_with_the_gate_s_codes"],
    **{f"AL{n}": M4_KH10 for n in range(1, 14)},
    "VR17": M4_KH10,
    "VR21": M4_Y4 + M4_KH10,
    "KH10": M4_KH10,
    "Y4.original_run": M4_KH10,
})
# PR 3c builds the committed candidates, the generator's match cases and the strict gate (KH2, KH3) through the
# library (KH1); the cases cite the rules they show.
M4_GATE = ["crates/fitment-core/tests/candidates.rs::every_committed_candidate_meets_its_construction",
           "crates/fitment/tests/matching.rs::every_match_case_meets_its_construction"]
M4_TESTS.update({
    "KC2": M4_GATE, "KC5": M4_GATE, "KH1": M4_GATE, "KH2": M4_GATE, "KH3": M4_GATE, "KH8": M4_GATE,
    "KH13": ["crates/fitment/tests/corpus.rs::the_candidate_set_fits_its_budget_and_its_manifest"],
    "VR8": M4_TESTS["VR8"] + M4_GATE,
    "VR22": M4_TESTS["VR22"] + M4_GATE,
})
# PR 4 builds the verdicts, the fitment.match document and the CLI.
M4_CLI = "crates/fitment/tests/match_cli.rs"
M4_VERDICT = "crates/fitment-core/tests/verdict.rs"
M4_GOLDEN = ["crates/fitment/tests/golden.rs::match_output_matches_the_goldens"]
M4_CHECKER = [f"{M4_CLI}::the_checker_finds_a_tampered_document", f"{M4_CLI}::the_original_and_directories_are_judged"]
M4_CODES = [f"{M4_CLI}::the_exit_code_is_the_best_verdict", f"{M4_CLI}::bad_input_is_refused_before_any_candidate"]
M4_TESTS.update({
    "VD1": [f"{M4_VERDICT}::a_candidate_is_judged_by_its_worst_socket",
            f"{M4_MATCHER}::a_refused_file_is_a_reject_with_the_gate_s_codes"],
    "VD2": [f"{M4_VERDICT}::a_candidate_is_judged_by_its_worst_socket"],
    "VD3": [f"{M4_VERDICT}::passing_interface_is_judged_by_fit_alone",
            f"{M4_VERDICT}::failing_interface_rejects_under_every_fit"],
    "VD4": [f"{M4_VERDICT}::a_candidate_is_judged_by_its_worst_socket"] + M4_GOLDEN,
    "VD5": M4_KH10, "VD6": M4_KH10,
    "VD7": [f"{M4_CLI}::a_removed_requirement_is_not_checked"],
    "VD8": [f"{M4_CLI}::bad_input_is_refused_before_any_candidate"] + M4_GOLDEN,
    "VD9": [f"{M4_CLI}::bad_input_is_refused_before_any_candidate"],
    "VD10": [f"{M4_MATCHER}::a_refused_file_is_a_reject_with_the_gate_s_codes"],
    "VD11": [f"{M4_MATCHER}::an_incomplete_candidate_is_flagged"],
    "VD12": [f"{M4_MATCHER}::an_incomplete_candidate_is_flagged", f"{M4_CLI}::a_removed_requirement_is_not_checked"],
    "VD13": M4_GOLDEN,
    "VD14": M4_CODES,
    "VD15": [f"{M4_CLI}::the_original_and_directories_are_judged"],
    **{f"EV{n}": M4_GOLDEN + M4_CHECKER for n in range(1, 10)},
    "EV8": M4_GOLDEN + [f"{M4_CLI}::the_document_does_not_depend_on_argument_order"],
    "MF1": [f"{M4_CLI}::the_original_and_directories_are_judged"],
    "MF2": [f"{M4_CLI}::bad_input_is_refused_before_any_candidate"],
    "MF3": [f"{M4_CLI}::the_original_and_directories_are_judged", f"{M4_CLI}::the_exit_code_is_the_best_verdict"],
    "MF4": [f"{M4_CLI}::labels_are_escaped", f"{M4_CLI}::the_original_and_directories_are_judged"],
    "MF5": [f"{M4_CLI}::the_document_does_not_depend_on_argument_order"],
    "MF6": M4_GOLDEN, "MF7": M4_CHECKER,
    "MF8": M4_CODES, "MF9": [f"{M4_CLI}::labels_are_escaped"], "MF10": M4_CODES,
    "KH11": [f"{M4_CLI}::the_document_does_not_depend_on_argument_order"] + M4_GOLDEN,
    "KH12": M4_CHECKER,
})
# PR 5 builds the corpus's cases and its harness: the binary path beside the library's, the baseline, the report,
# the shortlist cross-check, determinism, the runs in place (KC11 to KC13) and KH14's lists.
M4_MATCHING = "crates/fitment/tests/matching.rs"
M4_BINARY = [f"{M4_MATCHING}::the_binary_agrees_with_the_library"]
M4_SOCKET_GATE = [f"{M4_MATCHING}::the_socket_gate_refuses_before_any_candidate"]
M4_TESTS.update({
    "KC1": M4_GATE, "KC4": M4_GATE + M4_KH10, "KC11": M4_KH10,
    "KC12": [f"{M4_ORIGINAL}::as1_parts_match_across_exports"],
    "KC13": [f"{M4_ORIGINAL}::review_waives_only_what_it_removes"],
    "KC14": M4_GATE + [f"{M4_MATCHING}::the_report_states_the_rates"],
    "KC16": ["crates/fitment/tests/corpus.rs::the_candidate_set_fits_its_budget_and_its_manifest"],
    "KH1": M4_GATE + M4_BINARY,
    "KH4": [f"{M4_MATCHING}::the_false_negatives_are_the_baseline"],
    "KH5": [f"{M4_MATCHING}::the_false_negatives_are_the_baseline"],
    "KH6": [f"{M4_MATCHING}::the_report_states_the_rates"],
    "KH7": [f"{M4_MATCHING}::the_report_states_the_rates"],
    "KH9": [f"{M4_MATCHING}::the_shortlist_never_drops_what_verification_passes"],
    "KH11": M4_TESTS["KH11"] + [f"{M4_MATCHING}::matching_is_deterministic"],
    "KH12": M4_CHECKER + M4_BINARY,
    "KH14": ["crates/fitment/tests/corpus.rs::every_rule_has_a_case_a_test_or_a_reason"],
    "AL11": M4_TESTS["AL11"] + [f"{M4_MATCHING}::hostile_inputs_stay_within_their_bounds"],
    "VR4": M4_TESTS["VR4"] + ["crates/fitment-core/src/matching/axial.rs::a_clearance_with_no_room_fails_every_candidate"],
    "VD9": M4_TESTS["VD9"] + M4_SOCKET_GATE,
    "MF2": M4_TESTS["MF2"] + M4_SOCKET_GATE,
})
for rule, tests in M4_TESTS.items():
    RULES[rule] = {"tests": tests}
# PR 6 builds the match oracle: KO1 to KO8 are tools/verify-occt.py's, which CI runs (`manifest` and `match` on the
# corpus, `match --expect` on the fixtures), each named by a function that checks it, or for KO7 states it; the
# coverage test checks that each exists and names its rule.
M4_ORACLE = "tools/verify-occt.py"
RULES.update({
    "KO1": {"oracle": [f"{M4_ORACLE}::match_physics", f"{M4_ORACLE}::physics"]},
    "KO2": {"oracle": [f"{M4_ORACLE}::one_thing_broken"]},
    "KO3": {"oracle": [f"{M4_ORACLE}::document_problems", f"{M4_ORACLE}::ring_excused"]},
    "KO4": {"oracle": [f"{M4_ORACLE}::failure_problems"]},
    "KO5": {"oracle": [f"{M4_ORACLE}::feature_problems", f"{M4_ORACLE}::rigid_problem"]},
    "KO6": {"oracle": [f"{M4_ORACLE}::match_group", f"{M4_ORACLE}::with_placements"]},
    # What the oracle cannot check: match mode's documentation states it.
    "KO7": {"oracle": [f"{M4_ORACLE}::match_main"]},
    "KO8": {"oracle": [f"{M4_ORACLE}::fixture_groups"]},
})
# Shown by the match cases that cite them.
for rule in ("CF8", "KC3", "KC6", "KC7", "KC8", "KC9", "KC10", "KC15", "VR19"):
    RULES[rule] = {}
RULES["VR16"] = {"tests": ["crates/fitment/tests/match_cli.rs::a_removed_requirement_is_not_checked"]}

# M5's rules (docs/research/m5-catalog.md), each deferred to the PR of its section 10 that builds it until then
# (CT19); the coverage test allows M5's deferrals until M5 closes (PR 7).
M5_COUNTS = {"ST": 11, "DB": 9, "GT": 9, "ID": 9, "CR": 11, "RX": 11, "IX": 10, "QR": 13, "CC": 6, "PH": 8,
             "SZ": 5, "VF": 5, "PR": 7, "CL": 10, "CM": 11, "IN": 9}
M5_PR = {"ST": 2, "DB": 2, "GT": 2, "ID": 2, "PH": 2, "SZ": 2, "CL": 2, "CR": 3, "IN": 3, "VF": 3, "QR": 4,
         "CM": 4, "PR": 5, "RX": 6, "IX": 4, "CC": 2}
M5_OTHER_PR = {"ST5": 3, "CR8": 4, "CR9": 4, "CC3": 3, "CC4": 3, "RX1": 4, "RX2": 4, "RX3": 4}
for prefix, count in M5_COUNTS.items():
    for n in range(1, count + 1):
        rule = f"{prefix}{n}"
        RULES[rule] = {"deferred": f"M5 PR {M5_OTHER_PR.get(rule, M5_PR[prefix])}"}
# PR 1 builds core's part: level 1 over any source (IX3 to IX8), a stored document judged as its file (QR7), and
# the atomic write (CL7).
M5_STORED = "crates/fitment-core/tests/stored.rs"
M5_INDEX = "crates/fitment-core/src/matching/index.rs"
M5_SIGNATURE = "crates/fitment-core/src/matching/signature.rs"
M5_SHORTLIST = "crates/fitment-core/tests/shortlist.rs"
RULES.update({
    "IX3": {"tests": [f"{M5_INDEX}::a_window_finds_its_diameters_on_the_bin_edges"]},
    "IX4": {"tests": [f"{M5_SHORTLIST}::level_one_returns_whatever_level_two_keeps"]},
    "IX5": {"tests": [f"{M5_INDEX}::a_seat_bucket_is_never_below_the_lever",
                      f"{M5_SIGNATURE}::a_seat_bucket_never_splits_a_seat_group"]},
    "IX6": {"tests": [f"{M5_STORED}::admitted_holds_what_level_two_keeps"]},
    "IX7": {"tests": [f"{M5_SIGNATURE}::level_one_keeps_a_part_one_stack_serves_a_group_with",
                      "crates/fitment/tests/matching.rs::level_one_admits_every_pass"]},
    "IX8": {"tests": [f"{M5_SHORTLIST}::blocked_and_removed_requirements_demand_nothing"]},
    "QR7": {"tests": [f"{M5_STORED}::a_stored_document_is_judged_as_its_file",
                      f"{M5_STORED}::a_document_of_another_extractor_is_not_judged",
                      f"{M5_STORED}::judge_step_hands_its_bytes_to_the_fit_request",
                      "crates/fitment/tests/matching.rs::stored_documents_are_judged_as_their_files"]},
    "CL7": {"tests": ["crates/fitment/src/cmd/mod.rs::a_write_lands_whole_or_leaves_nothing",
                      "crates/fitment/src/cmd/mod.rs::a_write_only_directory_takes_the_write"]},
})
# PR 2 builds the store, ingest, and init, list and get. What it cannot show yet waits for the PR that can:
# concurrency for PR 3's races (CT14), the rest for M5's exit, where each gets a test or a stated reason.
T = "crates/fitment-catalog/tests"
C = "crates/fitment/tests/catalog_cli.rs"
S = "crates/fitment-catalog/src"
M5_PR2 = {
    "ST1": [f"{T}/store.rs::every_committed_part_is_stored_as_the_gate_reads_it"],
    "ST2": [f"{T}/store.rs::the_source_is_read_once"],
    "ST3": [f"{T}/fault.rs::every_named_blob_has_its_directory_synced_before_commit",
            f"{T}/fault.rs::a_reused_orphan_has_its_directory_synced"],
    "ST4": [f"{T}/edges.rs::a_link_at_a_blobs_name_is_never_followed"],
    "ST6": [f"{T}/fault.rs::every_named_blob_has_its_directory_synced_before_commit"],
    "ST7": [f"{T}/fault.rs::an_ingest_is_all_or_nothing_under_any_fault",
            f"{T}/edges.rs::a_taken_temporary_name_is_passed_over"],
    "ST9": [f"{T}/edges.rs::the_database_is_durable"],
    "ST10": [f"{T}/store.rs::an_input_inside_the_catalog_is_refused",
             f"{T}/edges.rs::init_refuses_a_directory_whose_tmp_is_not_ours",
             f"{C}::get_shows_an_entry_its_document_and_its_bytes"],
    "ST11": [f"{C}::get_step_refuses_a_damaged_blob"],
    "DB1": [f"{T}/store.rs::init_makes_a_catalog_once_and_refuses_what_is_not_one",
            f"{T}/edges.rs::init_leaves_a_foreign_database_as_it_was"],
    "DB3": [f"{T}/edges.rs::a_batch_holds_at_most_64_files"],
    "DB5": [f"{S}/schema.rs::migration_runs_each_step_once_and_records_it",
            f"{T}/store.rs::init_makes_a_catalog_once_and_refuses_what_is_not_one"],
    "DB6": [f"{S}/schema.rs::hex_columns_refuse_what_is_not_a_sha256"],
    "DB7": [f"{S}/schema.rs::tokens_need_an_accepted_document_and_rows_never_change",
            f"{T}/snapshot.rs::each_preimage_field_is_bound"],
    "DB8": [f"{T}/store.rs::every_committed_part_is_stored_as_the_gate_reads_it"],
    "DB9": [f"{T}/snapshot.rs::no_read_depends_on_unordered_selects"],
    "GT1": [f"{T}/store.rs::every_committed_part_is_stored_as_the_gate_reads_it"],
    "GT2": [f"{T}/snapshot.rs::each_preimage_field_is_bound",
            f"{T}/snapshot.rs::a_features_row_copied_to_another_key_fails_its_hash",
            f"{C}::get_shows_an_entry_its_document_and_its_bytes"],
    "GT3": [f"{T}/store.rs::files_refused_at_the_door_store_nothing"],
    "GT4": [f"{S}/schema.rs::tokens_need_an_accepted_document_and_rows_never_change"],
    "GT5": [f"{C}::list_writes_a_catalog_document"],
    "GT6": [f"{T}/store.rs::every_committed_part_is_stored_as_the_gate_reads_it"],
    "GT7": [f"{T}/store.rs::ids_go_to_exact_stems_first"],
    "ID1": [f"{S}/ids.rs::the_mapping_gives_an_id_or_none_and_keeps_ids"],
    "ID2": [f"{S}/schema.rs::the_partial_indexes_hold_identity", f"{T}/store.rs::ids_go_to_exact_stems_first"],
    "ID3": [f"{S}/ids.rs::default_ids_follow_id3"],
    "ID4": [f"{T}/store.rs::ids_go_to_exact_stems_first",
            f"{T}/edges.rs::lines_keep_their_order_and_links_inside_are_refused"],
    "ID5": [f"{T}/store.rs::ids_go_to_exact_stems_first", f"{C}::ingest_reports_each_file_and_exits_by_the_worst"],
    "ID7": [f"{C}::references_are_strict", f"{T}/snapshot.rs::a_shared_content_prefix_is_ambiguous"],
    "ID8": [f"{C}::list_writes_a_catalog_document"],
    "PH1": [f"{S}/hex.rs::only_lower_case_hex_of_64_digits_is_a_sha256",
            f"{S}/schema.rs::hex_columns_refuse_what_is_not_a_sha256"],
    "PH2": [f"{T}/store.rs::a_planted_link_is_refused", f"{T}/edges.rs::a_link_at_a_blobs_name_is_never_followed"],
    "PH3": [f"{T}/store.rs::hostile_names_are_stored_and_mapped"],
    "PH4": [f"{T}/store.rs::files_refused_at_the_door_store_nothing"],
    "PH6": [f"{C}::hostile_names_are_escaped", f"{T}/snapshot.rs::a_hand_edited_id_or_revision_is_corrupt"],
    "PH7": [f"{C}::get_shows_an_entry_its_document_and_its_bytes"],
    "PH8": [f"{C}::references_are_strict"],
    "SZ1": [f"{T}/store.rs::files_refused_at_the_door_store_nothing"],
    "SZ5": [f"{T}/edges.rs::a_full_database_commits_nothing"],
    "CL1": [f"{C}::init_and_what_is_not_a_catalog", f"{C}::a_catalog_named_like_a_uri_is_a_directory"],
    "CL2": [f"{C}::ingest_reports_each_file_and_exits_by_the_worst"],
    "CL3": [f"{C}::list_writes_a_catalog_document"],
    "CL4": [f"{C}::ingest_reports_each_file_and_exits_by_the_worst"],
    "CL8": ["crates/fitment/tests/no_catalog.rs::a_build_without_the_catalog_says_so"],
    "CL9": [f"{C}::list_writes_a_catalog_document", f"{C}::ingest_reports_each_file_and_exits_by_the_worst"],
    "CC5": [f"{T}/edges.rs::the_database_is_durable"],
    "CC6": [f"{T}/edges.rs::a_busy_catalog_stops_the_run"],
}
for rule, tests in M5_PR2.items():
    RULES[rule] = {"tests": tests}
for rule in ("DB2", "DB4", "ID6", "ID9", "CC1", "CC2"):
    RULES[rule] = {"deferred": "M5 PR 3" if rule in ("DB2", "ID6", "ID9", "CC1", "CC2") else "M5 PR 4"}
for rule in ("ST8", "GT8", "GT9", "PH5", "SZ2", "SZ3", "CL5", "CL6", "CL10"):
    RULES[rule] = {"deferred": "M5 PR 7"}

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
        self.cases.append({"id": name, "kind": "part", "file": f"{name}.stp",
                           "set": "unit" if unit_set else "corpus",
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
        if isinstance(child, Leaf):
            LEAVES.setdefault((self.name, child.name), child.shape)
        return self


# Every leaf the assembly section builds, by (assembly, part): the shapes the candidates' builders must rebuild
# with their defaults (15.4).
LEAVES = {}


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


def read_placements(path):
    """{product name: sorted 3x4 rows} of every leaf Open CASCADE's XCAF
    reads in `path`, rounded as the answers are."""
    from OCP.STEPCAFControl import STEPCAFControl_Reader
    from OCP.TCollection import TCollection_ExtendedString
    from OCP.TDataStd import TDataStd_Name
    from OCP.TDF import TDF_Label, TDF_LabelSequence as Sequence_TDF_Label
    from OCP.TDocStd import TDocStd_Document
    from OCP.TopLoc import TopLoc_Location
    from OCP.XCAFDoc import XCAFDoc_DocumentTool

    doc = TDocStd_Document(TCollection_ExtendedString("MDTV-XCAF"))
    reader = STEPCAFControl_Reader()
    reader.SetNameMode(True)
    if reader.ReadFile(str(path)) != IFSelect_RetDone or not reader.Transfer(doc):
        raise OSError(f"Open CASCADE cannot read {path}")
    tool = XCAFDoc_DocumentTool.ShapeTool_s(doc.Main())
    out = {}

    def walk(label, loc):
        if tool.IsAssembly_s(label):
            comps = Sequence_TDF_Label()
            tool.GetComponents_s(label, comps, False)
            for i in range(1, comps.Length() + 1):
                comp, ref = comps.Value(i), TDF_Label()
                if tool.GetReferredShape_s(comp, ref):
                    walk(ref, loc.Multiplied(tool.GetLocation_s(comp)))
        else:
            name = TDataStd_Name()
            label.FindAttribute(TDataStd_Name.GetID_s(), name)
            t = loc.Transformation()
            out.setdefault(name.Get().ToExtString(), []).append(
                [[r(t.Value(i, j)) + 0.0 for j in range(1, 5)] for i in range(1, 4)])

    free = Sequence_TDF_Label()
    tool.GetFreeShapes(free)
    for i in range(1, free.Length() + 1):
        walk(free.Value(i), TopLoc_Location())
    return {k: sorted(v) for k, v in out.items()}


def write_xcaf(w: Writer, root: Node, name: str, pcurves=False):
    """Writes the tree under `root` as one STEP assembly: every Leaf and
    Node object one product, named after it, every child one usage."""
    from OCP.STEPCAFControl import STEPCAFControl_Writer
    from OCP.TCollection import TCollection_ExtendedString
    from OCP.TDataStd import TDataStd_Name
    from OCP.TDocStd import TDocStd_Document
    from OCP.TopLoc import TopLoc_Location
    from OCP.XCAFDoc import XCAFDoc_DocumentTool, XCAFDoc_ShapeTool

    # cadquery's own export turns XCAF's automatic naming off for the whole
    # process; set it here so a case does not depend on what ran before.
    XCAFDoc_ShapeTool.SetAutoNaming_s(False)
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
    """A fixed time stamp, usage ids renumbered from 1 in file order and
    their names blanked, and one instance per line: Open CASCADE numbers
    usage ids with a counter that runs across the whole process and wraps
    lines by their length, so a case written alone would otherwise differ."""
    Writer.fix_time(path)
    f = Part21(path.read_text(encoding="latin-1"))
    # No whitespace outside strings: where Open CASCADE wrapped a line
    # depends on how wide the platform wrote its numbers.
    for n, value in f.inst.items():
        f.inst[n] = "".join(part if i % 2 else re.sub(r"\s+", "", part)
                            for i, part in enumerate(re.split(r"('[^']*')", value)))
    k = iter(range(1, 10 ** 6))
    for n in f.find("NEXT_ASSEMBLY_USAGE_OCCURRENCE"):
        f.inst[n] = re.sub(r"^NEXT_ASSEMBLY_USAGE_OCCURRENCE\('[^']*','[^']*'",
                           lambda m: f"NEXT_ASSEMBLY_USAGE_OCCURRENCE('{next(k)}',''", f.inst[n])
    path.write_text(f.text(), encoding="latin-1", newline="\n")


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


# Expected sockets (docs/research/m3-socket.md, sections 7 and 8), stated
# in the target's own frame from the construction: an axis by a point and
# a direction, spans and open rims as stations along it, compared as
# points, so the sign M2 gives an axis does not matter.


def e_span(a, b, dia, clear, fit, pinned, basis="cylinder", profile=None):
    """One span's fields: stations `a` to `b`, the original's diameter, the
    clear diameter, the fit, whether pinned, what it lies on (X8: a
    segment's `cylinder`, a `zone`, or `past_known_end`), and a zone's
    original diameters at `a` and `b` where one cone joins them."""
    return a, b, dia, clear, fit, pinned, basis, profile


def e_axial(kind, point, direction, spans, open_at=(), flags=(), depth="known", max_extra=None):
    """A bore or shaft: `spans` as (from, to, diameter, clear, fit, pinned),
    with a basis and a zone's profile after them where not a cylinder's
    (`e_span`), stations along `direction` from `point`; `open_at` the
    stations of rims that must be open (none on a cylinder in no stack); a
    bore's `depth`."""
    d = unit(direction)
    out = {"kind": kind, "axis_point": r3(point), "axis_direction": r3(d),
           "spans": [{"from": r3(add(point, d, a)), "to": r3(add(point, d, b)), "diameter": r(dia),
                      "clear": None if clear is None else r(clear), "fit": fit, "pinned": pinned,
                      "basis": basis, "profile": None if profile is None else [r(profile[0]), r(profile[1])]}
                     for a, b, dia, clear, fit, pinned, basis, profile in (e_span(*x) for x in spans)],
           "open": [r3(add(point, d, x)) for x in open_at], "flags": sorted(flags)}
    if kind == "bore":
        out["depth"] = depth
    else:
        # I4: how much longer the shaft may be before it meets a blind hole's floor.
        out["max_extra"] = None if max_extra is None else r(max_extra)
    return out


def e_left(point, direction, diameter, reason="nothing_on_axis", wall=None):
    """A left-out feature; `wall` its wall's stations along `direction`
    from `point`."""
    d = unit(direction)
    out = {"axis_point": r3(point), "axis_direction": r3(d), "diameter": r(diameter), "reason": reason}
    if wall is not None:
        out["wall"] = [r3(add(point, d, x)) for x in wall]
    return out


def e_seat(point, normal, by=1, flags=(), rings=(), basis=("target_face",), touchers=None):
    """A seat on the plane through `point` with outward `normal`: `by`
    neighbour faces on it in all, the bases of its contacts, its rings as
    (inner, outer) diameters, and where given its touchers as (neighbour
    product, gap, flags)."""
    out = {"point": r3(point), "normal": r3(unit(normal)), "by": by, "flags": sorted(flags),
           "rings": sorted([r(a), r(b)] for a, b in rings), "basis": sorted(basis)}
    if touchers is not None:
        out["touchers"] = [{"product": name, "gap": r(gap), "flags": sorted(fl)} for name, gap, fl in touchers]
    return out


def e_grip(length, holes=0, flags=()):
    """A grip of `length` between two seats, through `holes` target holes,
    with these flags (`seat_unverifiable` when a seat of it is, U10)."""
    return {"length": r(length), "holes": holes, "flags": sorted(flags)}


def e_cone(apex, angle_deg, axis, radii, sense="hole"):
    """A conical seat with this apex, included angle, axis direction (either
    sign), radial range (None for an unknown one: from the least to the
    greatest of its neighbour cones', Q1) and the target cone's sense (Q3)."""
    return {"apex": r3(apex), "angle": r(math.radians(angle_deg)), "axis": r3(unit(axis)),
            "radii": None if radii is None else [r(radii[0]), r(radii[1])], "sense": sense}


def e_partial(kind, point, direction, diameter, clear, a, b, sweep, flags=()):
    """A partial bore or shaft: its axis, diameters, its span `a` to `b`
    (a <= b) along `direction` from `point`, and the largest arc its
    cylinder covers."""
    d = unit(direction)
    return {"kind": kind, "axis_point": r3(point), "axis_direction": r3(d), "diameter": r(diameter),
            "clear": None if clear is None else r(clear), "from": r3(add(point, d, a)), "to": r3(add(point, d, b)),
            "sweep": r(sweep), "flags": sorted(flags)}


def e_socket(requirements, left_out=(), unresolved=None, at=(0, 0, 0), seats=(), grips=(), cones=(), partials=()):
    """One socket: its bores and shafts, what it leaves out, how many
    `unresolved` requirements of each cause, where its occurrence's origin
    is placed, and its seats, grips, conical seats and partial cylinders,
    each kind in full."""
    unresolved = unresolved or {}
    # Y4: the kinds every candidate fails, the original included.
    # U9: a piece past a known end, or in a zone with no pin, clear diameter or profile.
    unknown = lambda sp: sp["basis"] == "past_known_end" or (  # noqa: E731
        sp["basis"] == "zone" and not sp["pinned"] and sp["clear"] is None and sp["profile"] is None)
    unverifiable = (sum(unresolved.values()) + len(partials)
                    + sum(1 for q in requirements
                          if {"target_undecided", "open_at_unknown_end", "core_not_clear"} & set(q["flags"])
                          or any(unknown(sp) for sp in q["spans"]))
                    + sum(1 for q in seats if "unknown" in q["basis"])
                    + sum(1 for q in cones if q["radii"] is None)
                    + sum(1 for q in grips if "seat_unverifiable" in q["flags"]))
    return {"requirements": list(requirements), "left_out": list(left_out), "unresolved": unresolved,
            "at": r3(at), "seats": list(seats), "grips": list(grips), "cones": list(cones),
            "partials": list(partials), "unverifiable": unverifiable}


Z = (0, 0, 1)


# The length uncertainty Open CASCADE 7.9 writes in every context it makes.
OCC_UNCERTAINTY_MM = 1e-7


def asm_expect(root, select=(), listing_result=None, gate=None, notes=(), components=None, sockets=None):
    """The answers of an assembly case: its parts list, every leaf
    occurrence's transform, the selections to try, the assembly gate, its
    notes, the floors (every context Open CASCADE writes states 1e-7 mm)
    and, where given, what some components read as."""
    expect = {"listing": listing_result or {"result": "accepted", "path_form": "usage_id", "parts": listing(root)},
              "assembly_gate": gate or {"result": "accepted"},
              "select": list(select), "notes": list(notes),
              "floors": {"part_mm": OCC_UNCERTAINTY_MM, "placement_mm": OCC_UNCERTAINTY_MM}}
    if expect["listing"]["result"] == "accepted":
        expect["occurrences"] = occurrences(root)
    if components is not None:
        expect["components"] = components
    if sockets is not None:
        # Each socket says where its occurrence is: one per occurrence, so
        # a socket handed another occurrence's requirements is caught.
        placed = occurrences(root)
        for name, wanted in sockets.items():
            ats = sorted(tuple(w["at"]) for w in wanted)
            truth = sorted(tuple(r(row[3]) for row in m) for m in placed[name])
            assert ats == truth, f"{name}: sockets at {ats}, occurrences at {truth}"
        # 9.1: occurrences whose sockets are the same share one, listing
        # where each is.
        merged = {}
        for name, wanted in sockets.items():
            groups = {}
            for w in wanted:
                key = json.dumps({k: v for k, v in w.items() if k != "at"}, sort_keys=True)
                groups.setdefault(key, {**w, "at": []})["at"].append(w["at"])
            merged[name] = [{**g, "at": sorted(g["at"])} for g in groups.values()]
        expect["sockets"] = merged
    return expect


class Asm:
    """Writes assembly cases."""

    def __init__(self, w: Writer, which):
        self.w, self.which = w, which

    def case(self, name, root, tests, rules, unit_set, construction, expect, edit=None, written=None, variants=True):
        if not self.which(unit_set):
            return None
        path = write_xcaf(self.w, written or root, name)
        if written is not None:
            # `written` states frames Open CASCADE writes otherwise: the
            # answers are `root`'s, and Open CASCADE must read them back.
            read = read_placements(path)
            wanted = {k: sorted(v) for k, v in occurrences(root).items()}
            if read != wanted:
                raise RuntimeError(f"{name}: Open CASCADE wrote {read}, not {wanted}")
        if edit is not None:
            f = Part21(path.read_text(encoding="latin-1"))
            edit(f)
            path.write_text(f.text(), encoding="latin-1", newline="\n")
            Writer.fix_time(path)
        self.w.cases.append({"id": name, "file": f"{name}.stp", "kind": "assembly",
                             "set": "unit" if unit_set else "corpus", "tests": tests, "rules": rules,
                             "construction": construction, "expect": expect})
        if "sockets" in expect and edit is None and written is None and variants and self.which(False):
            self.variants(name, root, expect)
        return path

    def variants(self, name, root, expect):
        """`_rot`: the whole assembly under VARIANT, whose sockets must be the
        base's; `_tgt`: every socket target modelled in TILT and placed by
        TILT⁻¹, whose sockets must be the base's moved by TILT (P4, P5)."""
        rot = Node(f"{name}_rot").add(root, m_motion(VARIANT))
        write_xcaf(self.w, rot, f"{name}_rot")
        self.w.cases.append({"id": f"{name}_rot", "file": f"{name}_rot.stp", "kind": "assembly", "set": "corpus",
                             "tests": f"{name} under a skew root motion", "rules": ["P2", "P4", "P5"],
                             "construction": {"variant_of": name, "root": "VARIANT"},
                             "expect": {**asm_expect(rot), "variant": {"of": name, "kind": "rot",
                                                                       "motion": m_motion(VARIANT)}}})
        # `_pc`: written with pcurves; `_perm`: every node's children in
        # reverse order, so #ids and paths move. Both have the base's sockets.
        write_xcaf(self.w, Node(f"{name}_pc", root.children), f"{name}_pc", pcurves=True)
        perm = reversed_tree(root, f"{name}_perm")
        write_xcaf(self.w, perm, f"{name}_perm")
        for kind, tree, tests, rules in (("pc", Node(f"{name}_pc", root.children), "written with pcurves", ["P1"]),
                                         ("perm", perm, "with its components in reverse order", ["O1", "P2"])):
            self.w.cases.append({"id": f"{name}_{kind}", "file": f"{name}_{kind}.stp", "kind": "assembly",
                                 "set": "corpus", "tests": f"{name} {tests}", "rules": rules,
                                 "construction": {"variant_of": name, kind: True},
                                 "expect": {**asm_expect(tree), "variant": {"of": name, "kind": kind,
                                                                            "motion": [row[:] for row in I4[:3]]}}})
        targets = set(expect["sockets"])
        tgt = tilted(root, targets, f"{name}_tgt")
        write_xcaf(self.w, tgt, f"{name}_tgt")
        self.w.cases.append({"id": f"{name}_tgt", "file": f"{name}_tgt.stp", "kind": "assembly", "set": "corpus",
                             "tests": f"{name} with its targets modelled in TILT", "rules": ["P4", "P5", "P6"],
                             "construction": {"variant_of": name, "targets": "modelled in TILT, placed by TILT^-1"},
                             "expect": {**asm_expect(tgt), "variant": {"of": name, "kind": "tgt",
                                                                       "motion": m_motion(TILT)}}})


def reversed_tree(root, name):
    """`root` again with every node's children in reverse order; leaves and
    sub-assemblies stay shared."""
    nodes = {}

    def node(n, rename=None):
        if id(n) not in nodes or rename:
            copy = Node(rename or n.name)
            for child, matrix in reversed(n.children):
                copy.add(child if isinstance(child, Leaf) else node(child), matrix)
            if rename:
                return copy
            nodes[id(n)] = copy
        return nodes[id(n)]

    return node(root, name)


def tilted(root, targets, name):
    """`root` again, each leaf named in `targets` modelled in TILT (its shape
    moved by TILT) and placed by TILT⁻¹, so that every solid stays where it
    was; shared leaves and sub-assemblies stay shared."""
    back = m_inv(m_motion(TILT))
    leaves, nodes = {}, {}

    def leaf(child):
        if id(child) not in leaves:
            leaves[id(child)] = Leaf(child.name, BRepBuilderAPI_Transform(child.shape, TILT.trsf(), True).Shape())
        return leaves[id(child)]

    def node(n, rename=None):
        if id(n) in nodes:
            return nodes[id(n)]
        out = Node(rename or n.name)
        nodes[id(n)] = out
        for child, matrix in n.children:
            if isinstance(child, Leaf) and child.name in targets:
                out.add(leaf(child), m_mul(matrix, back))
            elif isinstance(child, Leaf):
                out.add(child, matrix)
            else:
                out.add(node(child), matrix)
        return out

    return node(root, name)


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
    # The bracket: each dia 6.6 hole holds a bolt's dia 6 shank over its
    # 8 mm, which runs on into the plate below; the dia 8 has nothing.
    bracket_socket = e_socket(
        [e_axial("bore", (x, y, 0), Z, [(0, 8, 6.6, 6, "clearance", False)], open_at=[0]) for x, y in PTS],
        left_out=[e_left((0, 0, 0), Z, 8, wall=(0, 8))],
        # On the plate below, under the four heads above: one face each, gripped 8 through all five holes.
        seats=[e_seat((0, 0, 0), (0, 0, -1), touchers=[("PLATE", 0, [])]),
               e_seat((0, 0, 8), Z, by=4, touchers=[("BOLT", 0, [])] * 4)], grips=[e_grip(8, holes=5)])
    sockets01 = {
        "BRACKET": [bracket_socket],
        # The plate, z -10 to 0: each hole holds the shank, which runs past both faces.
        "PLATE": [e_socket([e_axial("bore", (x, y, -10), Z, [(0, 10, 6.6, 6, "clearance", False)], open_at=[0, 10])
                            for x, y in PTS],
                           seats=[e_seat((0, 0, 0), Z), e_seat((0, 0, -10), (0, 0, -1), by=4)],
                           grips=[e_grip(10, holes=4)])],
        # The nut, z -5 to 0 in its frame: the dia 6 shank through its dia 5 hole.
        "NUT": [e_socket([e_axial("bore", (0, 0, -5), Z, [(0, 5, 5, 6, "interference", True)], open_at=[0, 5],
                                  flags=["thread_not_confirmed"])], at=(x, y, -10),
                         seats=[e_seat((0, 0, 0), Z)]) for x, y in PTS],
        # The bolt, head bearing face at z = 0: bracket z -8 to 0, plate -18 to -8, nut -23 to -18.
        "BOLT": [e_socket([e_axial("shaft", (0, 0, 0), (0, 0, -1),
                                   # The nut's faces lie inside the shank: positions known to tau, its span
                                   # widened by it (I3, the socket PR's SR7).
                                   [(0, 17.99, 6, 6.6, "clearance", False), (17.99, 23.01, 6, None, "interference", True)],
                                   flags=["thread_not_confirmed"])], at=(x, y, 8),
                          seats=[e_seat((0, 0, 0), (0, 0, -1))]) for x, y in PTS],
    }
    a.case("s01_bolted", s01, "a bracket bolted to a plate with four hex bolts and nuts", ["T3", "T5", "O1", "O4", "O5", "O6", "Z1", "N1", "N3", "N5", "B1", "B6", "J1", "A1", "Y3", "V2", "C1", "C2", "C4", "L1",
           "P2", "m3.4.1#32", "m3.4.1#33", "m3.4.1#35"], True,
           {"plate": [100, 60, 10], "bracket": [80, 50, 8], "holes": 6.6, "pts": PTS,
            "bolt": {"af": 10, "head": 4, "d": 6, "length": 25, "at_z": 8}, "nut": {"af": 10, "h": 5, "d": 5, "at_z": -10}},
           asm_expect(s01, select=[
               {"part": "BRACKET", "result": "BRACKET", "occurrences": 1, "matched_by": "product_id"},
               {"part": "BOLT", "result": "BOLT", "occurrences": 4, "matched_by": "product_id"},
               {"part": "NOPE", "error": "part_not_found", "suggestions": []},
               {"part": "bolt", "error": "part_not_found", "suggestions": ["BOLT"]},
               {"part": "s01_bolted", "error": "part_is_assembly", "suggestions": ["s01_bolted"]},
               {"part": "BRACKET", "occurrence_of": "BOLT", "nth": 2, "error": "occurrence_mismatch",
                "suggestions": ["BRACKET"]},
               {"occurrence_of": "BRACKET", "result": "BRACKET", "occurrences": 1, "matched_by": "occurrence"},
               {"part": "BRACKET", "occurrence": "nowhere", "error": "occurrence_not_found"},
               {"error": "nothing_selected"},
           ], components={"BRACKET": {"holes": 5, "shafts": 0}, "PLATE": {"holes": 4, "shafts": 0},
                          "NUT": {"holes": 1, "shafts": 0}, "BOLT": {"holes": 0, "shafts": 1}},
           sockets=sockets01))

    # s02: a counterbored block on a base, a socket head cap screw into the base's dia 5 blind hole.
    s02 = Node("s02_cbore")
    base = cut(box(-30, -30, -10, 60, 60, 10), cyl((0, 0, -9), UP, 2.5, 10))
    block = cut(box(-20, -20, 0, 40, 40, 15), cyl((0, 0, -1), UP, 3.3, 17), cyl((0, 0, 8.5), UP, 5.5, 7.5))
    s02.add(Leaf("BASE", base)).add(Leaf("BLOCK", block)).add(Leaf("SCREW", a_shcs()), m_tr((0, 0, 8.5)))
    c02 = {"base": "60 x 60 x 10, top at z 0, dia 5 blind 9 deep", "block": "40 x 40 x 15, dia 11 x 6.5 over dia 6.6",
           "screw": "head dia 10 x 6, shank dia 6 x 16, at z 8.5"}
    sockets02 = {
        # The block: shank dia 6 in the dia 6.6 bore to z 8.5, head dia 10 in the dia 11 counterbore to 14.5;
        # the shank runs on into the base.
        "BLOCK": [e_socket([e_axial("bore", (0, 0, 0), Z, [(0, 8.5, 6.6, 6, "clearance", False),
                                                          (8.5, 14.51, 11, 10, "clearance", False)], open_at=[0])],
                           # On the base, and under the head on the counterbore's floor: the head's end face
                           # (C1) and its dia 10 wall on the dia 6.6 hole's rim (H1).
                           seats=[e_seat((0, 0, 0), (0, 0, -1)), e_seat((0, 0, 8.5), Z, by=2, rings=[(6.6, 10)])],
                           grips=[e_grip(8.5)])],
        # The base's dia 5 blind hole, 9 deep from z 0: the dia 6 shank's tip at z -7.5.
        "BASE": [e_socket([e_axial("bore", (0, 0, -9), Z, [(1.49, 9, 5, 6, "interference", True)], open_at=[9],
                                   flags=["thread_not_confirmed"])], seats=[e_seat((0, 0, 0), Z)])],
        # The screw, head z 0 to 6, shank -16 to 0: base to -8.5, block to 0, counterbore around the head.
        "SCREW": [e_socket([e_axial("shaft", (0, 0, -16), Z, [(0, 7.51, 6, None, "interference", True),
                                                             (7.51, 16, 6, 6.6, "clearance", False),
                                                             (16, 22, 10, 11, "clearance", False)],
                                    flags=["thread_not_confirmed"], max_extra=1.5)], at=(0, 0, 8.5),
                           seats=[e_seat((0, 0, 0), (0, 0, -1))])],
    }
    a.case("s02_cbore", s02, "a counterbored block screwed to a base", ["T3", "P2", "B1", "B3", "B5", "B6", "B7", "I1",
                                                                       "I2", "I3", "I4", "Y1", "H1"], True, c02,
           asm_expect(s02, select=[{"part": "BLOCK", "result": "BLOCK", "occurrences": 1, "matched_by": "product_id"}],
                      sockets=sockets02))

    # s10: s02 with the screw placed by a mapped item (a text edit): its CDSR, relationship and transformation
    # go, and a MAPPED_ITEM takes the root-side frame's place among the root representation's items, mapping
    # the screw's representation from a copy of its origin to a copy of that frame. Left as a plain item,
    # the frame would place the screw a second time for Open CASCADE. The sockets are s02's.
    def mapped(f: Part21):
        usage = usage_of(f, "SCREW")
        cdsr, relationship, idt = placement_of(f, usage)
        _, child_item, child_rep = component_item(f, usage)
        parent_item = next(i for i in f.refs(idt) if i != child_item)
        parent_rep = next(r for r in f.refs(relationship) if r not in (idt, child_rep))
        target, origin = f.add(f.inst[parent_item]), f.add(f.inst[child_item])
        rep_map = f.add(f"REPRESENTATION_MAP(#{origin},#{child_rep})")
        item = f.add(f"MAPPED_ITEM('',#{rep_map},#{target})")
        f.inst[parent_rep] = re.sub(rf"#{parent_item}\b", f"#{item}", f.inst[parent_rep], count=1)
        for n in (cdsr, relationship, idt):
            del f.inst[n]

    a.case("s10_mapped", s02, "s02 with the screw placed by a mapped item", ["P2", "P3"], False, {"from": "s02"},
           asm_expect(s02, sockets=sockets02), edit=mapped)

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
    fits03 = {5.97: ("interference", True), 6.2: ("clearance", False), 6.0: ("line_to_line", True),
              6.015: ("line_to_line", True)}
    tnc = lambda fit: ["thread_not_confirmed"] if fit != "clearance" else []  # noqa: E731
    sockets03 = {
        # The plate, z 0 to 10: each dowel fills its hole to z 8 and runs on into the base.
        "PLATE": [e_socket([e_axial("bore", (x, 0, 0), Z, [(0, 8.01, d, 6, *fits03[d])], open_at=[0], flags=tnc(fits03[d][0]))
                            for x, d in zip(xs, holes03)], seats=[e_seat((0, 0, 0), (0, 0, -1))])],
        # Each dowel, z 0 to 16: line to line in the base's dia 6 to z 8, then its plate hole's fit.
        "DOWEL": [e_socket([e_axial("shaft", (0, 0, 0), Z,
                                    # The base's top lies inside the dowel at z 8: the stricter fit there takes
                                    # tau past it (I3, SR7).
                                    [(0, 7.99, 6, None, "line_to_line", True), (7.99, 16, 6, None, "interference", True)]
                                    if d == 5.97 else
                                    [(0, 8.01, 6, None, "line_to_line", True), (8.01, 16, 6, 6.2, "clearance", False)]
                                    if d == 6.2 else [(0, 8, 6, None, "line_to_line", True)],
                                    flags=["thread_not_confirmed"], max_extra=0)], at=(x, 0, -8),
                           # Each dowel bottoms on its hole's floor (I4): a seat, and no room to spare.
                           seats=[e_seat((0, 0, 0), (0, 0, -1))]) for x, d in zip(xs, holes03)],
    }
    # Line to line on both, with one fit: one span.
    for sock, d in zip(sockets03["DOWEL"], holes03):
        if d in (6.0, 6.015):
            sock["requirements"][0]["spans"] = e_axial("shaft", (0, 0, 0), Z, [(0, 16, 6, None, "line_to_line", True)])["spans"]
    a.case("s03_dowels", s03, "a plate on four dowels: press, clearance and two line-to-line fits", ["T3", "P2", "B2",
                                                                                                    "B3", "Z3", "Z4", "X6", "C2", "I4"], True,
           {"plate": [80, 40, 10], "holes": list(zip(xs, holes03)), "dowel": "dia 6 x 16 from z -8",
            "base": "100 x 60 x 10, dia 6 blind 8 deep"},
           asm_expect(s03, select=[{"part": "DOWEL", "result": "DOWEL", "occurrences": 4, "matched_by": "product_id"}],
                      sockets=sockets03))

    # s08x: one sub-assembly product (bracket, bolts, nuts) used twice, as AS1 does.
    long_plate = Leaf("PLATE", a_plate(lx=220, pts=[(x - 50, y) for x, y in pts8]))
    # Exact, not through cos and sin: platforms round 1e-16 terms apart,
    # and the committed files must be the same text everywhere.
    turned = [[-1.0, 0.0, 0.0, 100.0], [0.0, -1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]]
    lb = bolted("LB_ASSEM")
    s08x = Node("s08x_nested").add(long_plate, m_tr((50, 0, 0))).add(lb).add(lb, turned)
    a.case("s08x_nested", s08x, "one sub-assembly product used twice: one definition, two paths",
           ["O1", "O3", "O4", "T1", "P1"], True,
           {"plate": "220 x 60 x 10 at x 50", "LB_ASSEM": "bracket, 4 bolts, 4 nuts", "second": "turned 180 about z, x + 100"},
           asm_expect(s08x, select=[
               {"part": "BRACKET", "result": "BRACKET", "occurrences": 2, "matched_by": "product_id"},
               {"part": "LB_ASSEM", "error": "part_is_assembly", "suggestions": ["LB_ASSEM"]},
               {"part": "BRACKET", "occurrence_of": "BRACKET", "nth": 0, "result": "BRACKET", "occurrences": 1,
                "matched_by": "product_id"},
               {"part": "BRACKET", "occurrence_of": "BRACKET", "nth": 1, "result": "BRACKET", "occurrences": 1,
                "matched_by": "product_id"},
           ], components={"BRACKET": {"holes": 5, "shafts": 0}, "PLATE": {"holes": 8, "shafts": 0},
                          "NUT": {"holes": 1, "shafts": 0}, "BOLT": {"holes": 0, "shafts": 1}},
           sockets={"BRACKET": [bracket_socket, {**bracket_socket, "at": r3((100, 0, 0))}]}))

    # s07: the same without a sub-assembly. s08: two sub-assembly products sharing their leaves.
    s07 = Node("s07_twice").add(long_plate, m_tr((50, 0, 0)))
    s07.add(bracket).add(bracket, turned)
    for x, y in pts8:
        s07.add(bolt, m_tr((x, y, 8)))
        s07.add(nut, m_tr((x, y, -10)))
    # Both brackets need what s01's does: one socket for both (9.1).
    a.case("s07_twice", s07, "two brackets on one plate, the second turned 180 deg", ["T3", "O6"], False,
           {"from": "s08x, flat"}, asm_expect(s07, select=[
               {"part": "BRACKET", "result": "BRACKET", "occurrences": 2, "matched_by": "product_id"}],
               sockets={"BRACKET": [bracket_socket, {**bracket_socket, "at": r3((100, 0, 0))}]}))

    # s07b: s07 without the second bracket's bolt and nut at (130, 15), its own (-30, -15) turned: that
    # bore is now continued by the plate's hole with no fastener across (J1, pinned), so the brackets need
    # different sockets.
    s07b = Node("s07b_one_missing").add(long_plate, m_tr((50, 0, 0)))
    s07b.add(bracket).add(bracket, turned)
    for x, y in pts8:
        if (x, y) != (130, 15):
            s07b.add(bolt, m_tr((x, y, 8)))
            s07b.add(nut, m_tr((x, y, -10)))
    missing = e_socket(
        [e_axial("bore", (x, y, 0), Z, [(0, 8, 6.6, None, "not_read", True)] if (x, y) == (-30, -15)
                 else [(0, 8, 6.6, 6, "clearance", False)], open_at=[0]) for x, y in PTS],
        left_out=[e_left((0, 0, 0), Z, 8, wall=(0, 8))],
        seats=[e_seat((0, 0, 0), (0, 0, -1), touchers=[("PLATE", 0, [])]),
               e_seat((0, 0, 8), Z, by=3, touchers=[("BOLT", 0, [])] * 3)],
        grips=[e_grip(8, holes=5)], at=(100, 0, 0))
    a.case("s07b_one_missing", s07b, "s07 with one bolt and its nut removed from the second bracket", ["J1", "Y1"],
           False, {"from": "s07", "removed": "the bolt and nut at (130, 15)"},
           asm_expect(s07b, select=[
               {"part": "BRACKET", "result": "BRACKET", "occurrences": 2, "matched_by": "product_id"}],
               sockets={"BRACKET": [bracket_socket, missing]}))
    s08 = Node("s08_nested").add(long_plate, m_tr((50, 0, 0)))
    s08.add(bolted("LB_A", bracket, bolt, nut)).add(bolted("LB_B", bracket, bolt, nut), turned)
    a.case("s08_nested", s08, "two sub-assembly products sharing their leaves", ["O1"], False,
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
                                     "matched_by": "product_id"},
                                    {"occurrence": "1", "error": "occurrence_ambiguous"}])
    exp25["listing"]["path_form"] = "instance"
    a.case("s25_usage_ids", s01, "s01 with two sibling usages both given the id '1' (a text edit)", ["O2", "O3"], False,
           {"from": "s01"}, exp25, edit=same_id)

    # s06: holes with nothing in them. BRACKET_ROW's five dia 6.6 holes over a plate (top z 0) that ends at
    # x -36, with a dia 6.6 hole under x 0 and a 20 mm window under x 48, and a bar 30 mm below with dia 6.6
    # holes under x 24 and 48. No bolts.
    xs06 = (-48, -24, 0, 24, 48)
    row = through(box(-60, -15, 0, 120, 30, 8), [(x, 0) for x in xs06], 6.6, 0, 8)
    plate06 = cut(box(-36, -25, -10, 106, 50, 10), cyl((0, 0, -11), UP, 3.3, 12), box(38, -10, -11, 20, 20, 12))
    bar06 = through(box(10, -15, -40, 50, 30, 10), [(24, 0), (48, 0)], 6.6, -40, -30)
    s06 = Node("s06_open_holes").add(Leaf("BRACKET_ROW", row)).add(Leaf("PLATE", plate06)).add(Leaf("BAR", bar06))
    sockets06 = {"BRACKET_ROW": [e_socket(
        [e_axial("bore", (0, 0, 0), Z, [(0, 8, 6.6, None, "not_read", True)], open_at=[0]),
         e_axial("bore", (48, 0, 0), Z, [(0, 8, 6.6, None, "not_read", True)], open_at=[0])],
        left_out=[e_left((-48, 0, 0), Z, 6.6, wall=(0, 8)), e_left((-24, 0, 0), Z, 6.6, wall=(0, 8)),
                  e_left((24, 0, 0), Z, 6.6, "blocked_gap", wall=(0, 8))],
        seats=[e_seat((0, 0, 0), (0, 0, -1))])]}
    a.case("s06_open_holes", s06, "holes with nothing in them: beyond the plate, over plain plate, over a plate hole, "
           "over a hole 30 mm below behind the plate, and the same through a window", ["J1", "J2", "J3", "A1"],
           False, {"bracket_row": "120 x 30 x 8, dia 6.6 at x -48, -24, 0, 24, 48", "plate": "x -36 to 70, top z 0, "
                   "dia 6.6 at x 0, 20 mm window at x 48", "bar": "top z -30, dia 6.6 at x 24, 48"},
           asm_expect(s06, sockets=sockets06))

    # s11: s01 with its bolts written as B-splines: nothing reads them, so every hole they might be in is
    # pinned, and none is left out.
    nurbs_bolt = Leaf("BOLT", BRepBuilderAPI_NurbsConvert(a_hex_bolt(), True).Shape())
    s11 = Node("s11_unread_neighbour").add(Leaf("PLATE", a_plate())).add(bracket)
    for x, y in PTS:
        s11.add(nurbs_bolt, m_tr((x, y, 8)))
        s11.add(nut, m_tr((x, y, -10)))
    sockets11 = {"BRACKET": [e_socket(
        [e_axial("bore", (x, y, 0), Z, [(0, 8, 6.6, None, "not_read", True)], open_at=[0], flags=["occupant_unread"])
         for x, y in PTS] + [e_axial("bore", (0, 0, 0), Z, [(0, 8, 8, None, "not_read", True)],
                                     flags=["occupant_unread"])],
        unresolved={"neighbour_unread": 1}, seats=[e_seat((0, 0, 0), (0, 0, -1))])],
        # Each B-spline bolt reads as ten other faces, each unresolved (U5), and its copies as unread
        # neighbours (U4).
        "BOLT": [e_socket([], unresolved={"target_unread_face": 10, "neighbour_unread": 1}, at=(x, y, 8))
                 for x, y in PTS]}
    a.case("s11_unread_neighbour", s11, "s01 with its bolts converted to B-splines: no hole may be left out",
           ["B8", "U4", "U5", "A1"], False, {"from": "s01", "bolts": "BRepBuilderAPI_NurbsConvert"},
           asm_expect(s11, sockets=sockets11))

    # s12: a dia 6 pin through p25's angled hole (30 deg to z in the y-z plane), the hole's ends ellipses.
    th12 = math.radians(30)
    into12 = (0.0, math.sin(th12), -math.cos(th12))
    entry12 = add((20, 20, 10), into12, -10 / math.cos(th12))
    angled = cut(box(0, 0, 0, 40, 40, 20), tool(Hole(entry12, into12, [(6, 20 / math.cos(th12) + 5)]), overshoot=5.0))
    pin12 = Leaf("PIN", cyl((0, 0, -20), UP, 3, 40))
    # Turned +30 deg about x, z goes to (0, -sin 30, cos 30): along the hole, against its drilling direction.
    tilt12 = m_mul(m_tr((20, 20, 10)), m_rot((1, 0, 0), 30))
    s12 = Node("s12_undecided_target").add(Leaf("ANGLED", angled)).add(pin12, tilt12)
    # The angled cylinder is undecided (its ends are ellipses): a bore it can only fail, its spans unchecked.
    # It is held line to line over the pin's length, and neither end is known.
    sockets12 = {"ANGLED": [e_socket([e_axial("bore", (20, 20, 10), into12, [(-20.01, 20.01, 6, 6, "line_to_line", True)],
                                              flags=["extent_unknown", "target_undecided", "thread_not_confirmed"],
                                              depth="unknown_end")],
                                     # The pin crosses the top and bottom faces through the hole's ellipses,
                                     # which leave their outlines unknown: nothing shows it touches them not (C6).
                                     unresolved={"contact_no_rule": 2})]}
    a.case("s12_undecided_target", s12, "a dia 6 pin through an angled dia 6 hole whose ends are ellipses",
           ["U2", "U3", "U8", "C6"], False, {"from": "p25", "pin": "dia 6 x 40 on the hole's axis"},
           asm_expect(s12, sockets=sockets12))

    # s16: off axis and tilted. A plate z 0 to 8 with a dia 6.6 hole at (0, 0) holding a dia 6 pin 0.2 mm
    # off its axis, and a dia 7 hole at (15, 0) holding the same pin tilted 0.5 deg about y.
    plate16 = through(through(box(-10, -20, 0, 40, 40, 8), [(0, 0)], 6.6, 0, 8), [(15, 0)], 7.0, 0, 8)
    pin16 = Leaf("PIN", cyl((0, 0, -10), UP, 3, 20))
    tilt16 = 0.5
    s16 = Node("s16_offaxis").add(Leaf("PLATE", plate16)).add(pin16, m_tr((0.2, 0, 4)))
    s16.add(pin16, m_mul(m_tr((15, 0, 4)), m_rot((0, 1, 0), tilt16)))
    t16 = math.radians(tilt16)
    e16 = 4 * math.tan(t16)  # the tilted axis is off the hole's by this much at z 0 and z 8
    clear16 = 2 * (e16 + 3 / math.cos(t16))
    held16 = 4 * math.cos(t16) - 3.5 * math.sin(t16)
    sockets16 = {"PLATE": [e_socket([
        e_axial("bore", (0, 0, 0), Z, [(0, 8, 6.6, 6.4, "clearance", False)], open_at=[0, 8], flags=["offset"]),
        e_axial("bore", (15, 0, 0), Z, [(0, 8, 7.0, clear16, "clearance", False)], open_at=[0, 8], flags=["offset"]),
    ])],
        # Each pin, z -10 to 10 about its centre at z 4: the dia 6.6 hole's wall 0.2 mm off, over the plate.
        # The tilted pin sees the dia 7 hole's ends at 4 cos t from its centre, less the hole's tilted disc
        # (X2, shrunk), and the hole's axis |s| tan t off its own at its station s (X3, X4).
        "PIN": [e_socket([e_axial("shaft", (0, 0, 0), Z, [(-4.01, 4.01, 6, 6.2, "clearance", False)], flags=["offset"])],
                         at=(0.2, 0, 4)),
                e_socket([e_axial("shaft", (0, 0, 0), Z, [(-held16 - 0.01, held16 + 0.01, 6, 2 * (3.5 - held16 * math.tan(t16)),
                                                          "clearance", False)], flags=["offset"])],
                         at=(15, 0, 4))]}
    a.case("s16_offaxis", s16, "a pin 0.2 mm off its hole's axis, and one tilted 0.5 deg in a dia 7 hole",
           ["X1", "X2", "X3", "X4", "X5", "Z5", "B1", "B6", "I1", "P4", "P5"], False,
           {"plate": "40 x 40 x 8", "holes": "dia 6.6 at (0, 0), dia 7 at (15, 0)", "pin": "dia 6 x 20",
            "offset": 0.2, "tilt_deg": tilt16}, asm_expect(s16, sockets=sockets16))

    # s19: a cover with a dia 12 boss, 5 long, in a housing's dia 12.1 blind bore 8 deep: 3 mm short of the floor.
    cover = fuse(box(-20, -20, 0, 40, 40, 5), cyl((0, 0, -5), UP, 6, 5))
    housing = cut(box(-20, -20, -20, 40, 40, 20), cyl((0, 0, -8), UP, 6.05, 9))
    s19 = Node("s19_boss").add(Leaf("COVER", cover)).add(Leaf("HOUSING", housing))
    sockets19 = {
        "COVER": [e_socket([e_axial("shaft", (0, 0, 0), (0, 0, -1), [(0, 5, 12, 12.1, "clearance", False)], max_extra=3)],
                           seats=[e_seat((0, 0, 0), (0, 0, -1))])],
        "HOUSING": [e_socket([e_axial("bore", (0, 0, -8), Z, [(2.99, 8, 12.1, 12, "clearance", False)])],
                             seats=[e_seat((0, 0, 0), Z)])],
    }
    a.case("s19_boss", s19, "a dia 12 boss in a dia 12.1 blind bore, 3 mm short of its floor",
           ["I1", "I3", "I4", "B1", "B5"], False, {"cover": "40 x 40 x 5, boss dia 12 x 5", "housing": "bore dia 12.1 x 8"},
           asm_expect(s19, sockets=sockets19))

    # s21: two blocks with dia 10 holes on one axis, 150 mm apart, the second turned 180 deg about x; nothing
    # between them. Each needs its hole open toward the other, pinned, as a copy of the target.
    block10 = Leaf("BLOCK10", through(box(-20, -20, 0, 40, 40, 10), [(0, 0)], 10, 0, 10))
    flip = [[1.0, 0.0, 0.0, 0.0], [0.0, -1.0, 0.0, 0.0], [0.0, 0.0, -1.0, 170.0], [0.0, 0.0, 0.0, 1.0]]
    s21 = Node("s21_self_rod").add(block10).add(block10, flip)
    sock21 = e_socket([e_axial("bore", (0, 0, 0), Z, [(0, 10, 10, None, "not_read", True)], open_at=[10],
                               flags=["self"])])
    a.case("s21_self_rod", s21, "two copies of a block, their dia 10 holes on one axis 150 mm apart",
           ["N2", "J2", "P6"], False, {"block10": "40 x 40 x 10, dia 10", "second": "turned 180 about x, top at z 160"},
           asm_expect(s21, sockets={"BLOCK10": [sock21, {**sock21, "at": r3((0, 0, 170))}]}))

    # The PR 3 review's cases (X7, B6, B8, J1 to J3).

    # s40: a dia 6.6 hole stepping by a cone (z 8 to 10) to dia 11, a dia 6 pin through it all. The cone's
    # zone goes to the narrower segment, so the pin's clear diameter holds there too.
    plate40 = cut(box(-20, -20, 0, 40, 40, 14), cyl((0, 0, -1), UP, 3.3, 10), cone((0, 0, 8), UP, 3.3, 5.5, 2),
                  cyl((0, 0, 10), UP, 5.5, 5))
    s40 = Node("s40_cone_step").add(Leaf("PLATE", plate40)).add(Leaf("PIN", cyl((0, 0, -5), UP, 3, 25)))
    sockets40 = {
        # The dia 6.6 hole stands for the cone to z 10 (X7), a zone whose profile runs from dia 6.6 to 11 (X8).
        "PLATE": [e_socket([e_axial("bore", (0, 0, 0), Z, [(0, 8, 6.6, 6, "clearance", False),
                                                          (8, 10, 6.6, 6, "clearance", False, "zone", (6.6, 11)),
                                                          (10, 14, 11, 6, "clearance", False)], open_at=[0, 14])])],
        # The pin, z -5 to 20: at most the hole's narrower dia 6.6 up to the cone's top.
        "PIN": [e_socket([e_axial("shaft", (0, 0, 0), Z, [(-0.01, 10.01, 6, 6.6, "clearance", False),
                                                         (10.01, 14.01, 6, 11, "clearance", False)])])],
    }
    a.case("s40_cone_step", s40, "a dia 6 pin through a hole that steps from dia 6.6 to dia 11 by a cone",
           ["X7", "X8", "B1", "B6", "I1"], False, {"plate": "40 x 40 x 14, dia 6.6 to z 8, cone to z 10, dia 11 to 14",
                                           "pin": "dia 6, z -5 to 20"}, asm_expect(s40, sockets=sockets40))

    # s41: a screw whose head (dia 10.8, z 12 to 14) meets its dia 6 shank by a cone (z 10 to 12), in a dia 11
    # counterbore from z 8. The cone's zone goes to the wider head.
    plate41 = cut(box(-20, -20, 0, 40, 40, 14), cyl((0, 0, -1), UP, 3.3, 10), cyl((0, 0, 8), UP, 5.5, 7))
    screw41 = fuse(cyl((0, 0, -5), UP, 3, 15), cone((0, 0, 10), UP, 3, 5.4, 2), cyl((0, 0, 12), UP, 5.4, 2))
    s41 = Node("s41_cone_head").add(Leaf("PLATE", plate41)).add(Leaf("SCREW", screw41))
    sockets41 = {
        "PLATE": [e_socket([e_axial("bore", (0, 0, 0), Z, [(0, 8, 6.6, 6, "clearance", False),
                                                          (8, 9.99, 11, 6, "clearance", False),
                                                          (9.99, 14, 11, 10.8, "clearance", False)], open_at=[0])])],
        "SCREW": [e_socket([e_axial("shaft", (0, 0, 0), Z, [(-0.01, 8.01, 6, 6.6, "clearance", False),
                                                           (8.01, 10, 6, 11, "clearance", False),
                                                           # The head's cone, z 10 to 12, is its zone (X8).
                                                           (10, 12, 10.8, 11, "clearance", False, "zone", (6, 10.8)),
                                                           (12, 14, 10.8, 11, "clearance", False)])])],
    }
    a.case("s41_cone_head", s41, "a screw with a cone under its head in a counterbore", ["X7", "X8", "B1", "B7", "I1"],
           False, {"plate": "40 x 40 x 14, dia 6.6 to z 8, dia 11 above",
                   "screw": "dia 6 from z -5, cone z 10 to 12, head dia 10.8 to z 14"},
           asm_expect(s41, sockets=sockets41))

    # s42: chamfered pin tips. A blind dia 6.6 hole, floor at z 8, holds a pin whose tip's cone reaches z 8.5;
    # a through hole at x 15 holds a pin whose tip pokes 1 mm out below. Each tip's zone reaches its rim.
    plate42 = cut(box(-10, -10, 0, 40, 20, 20), cyl((0, 0, 8), UP, 3.3, 13), cyl((15, 0, -1), UP, 3.3, 22))
    pin42a = fuse(cyl((0, 0, 10), UP, 3, 30), cone((0, 0, 8.5), UP, 1.5, 3, 1.5))
    pin42b = fuse(cyl((15, 0, 0.5), UP, 3, 29.5), cone((15, 0, -1), UP, 1.5, 3, 1.5))
    s42 = Node("s42_chamfer").add(Leaf("PLATE", plate42)).add(Leaf("PIN1", pin42a)).add(Leaf("PIN2", pin42b))
    sockets42 = {"PLATE": [e_socket([
        e_axial("bore", (0, 0, 8), Z, [(0.49, 12, 6.6, 6, "clearance", False)], open_at=[12]),
        e_axial("bore", (15, 0, 0), Z, [(0, 20, 6.6, 6, "clearance", False)], open_at=[0, 20])],
        # The first tip's flat end, dia 3, 0.5 mm over the blind hole's floor: in the near band (C5).
        seats=[e_seat((0, 0, 8), Z, flags=["near_gap"], touchers=[("PIN1", 0.5, ["near_gap"])])])]}
    a.case("s42_chamfer", s42, "pins with chamfered tips: one short of a blind hole's floor, one out of a through hole",
           ["X7", "B5", "B6", "C5"], False, {"plate": "40 x 20 x 20, blind dia 6.6 floor z 8, through dia 6.6 at x 15",
                                     "tips": "cone dia 3 to 6 over 1.5"}, asm_expect(s42, sockets=sockets42))

    # s43: a block's dia 10 hole, with a dia 4 disc 2 mm above it and a dia 3 tube beyond that, then a far
    # plate's dia 10 hole at z 30. Neither the disc nor the tube blocks the line to the far hole: a dia 1 rod
    # 4 mm off the axis passes both.
    block43 = cut(box(-20, -20, 0, 40, 40, 10), cyl((0, 0, -1), UP, 5, 12))
    tube43 = cut(cyl((0, 0, 15), UP, 1.5, 5), cyl((0, 0, 14), UP, 1, 7))
    far43 = cut(box(-20, -20, 30, 40, 40, 10), cyl((0, 0, 29), UP, 5, 12))
    s43 = Node("s43_nearest_blocked").add(Leaf("BLOCK", block43)).add(Leaf("DISC", cyl((0, 0, 12), UP, 2, 1)))
    s43.add(Leaf("TUBE", tube43)).add(Leaf("FAR", far43))
    sockets43 = {"BLOCK": [e_socket([e_axial("bore", (0, 0, 0), Z, [(0, 10, 10, None, "not_read", True)],
                                             open_at=[10])])],
                 # The disc, a shaft in free air, is left out (A2).
                 "DISC": [e_socket([], left_out=[e_left((0, 0, 12), Z, 4, wall=(0, 1))])]}
    a.case("s43_nearest_blocked", s43, "a hole whose nearest neighbour hole is blocked, a wider one beyond is not",
           ["J2", "J3", "A2"], False, {"block": "dia 10, z 0 to 10", "disc": "dia 4, z 12 to 13",
                                 "tube": "bore dia 2, outside dia 3, z 15 to 20", "far": "dia 10, z 30 to 40"},
           asm_expect(s43, sockets=sockets43))

    # s44: a dia 6 pin from z 5 to 12 in a block's dia 10 hole, short of a far plate's dia 10 hole at z 30:
    # it does not span the joint, so the hole is pinned where the pin is not.
    block44 = cut(box(-20, -20, 0, 40, 40, 10), cyl((0, 0, -1), UP, 5, 12))
    s44 = Node("s44_spanned").add(Leaf("BLOCK", block44)).add(Leaf("PIN", cyl((0, 0, 5), UP, 3, 7)))
    s44.add(Leaf("FAR", far43))
    sockets44 = {"BLOCK": [e_socket([e_axial("bore", (0, 0, 0), Z, [(0, 5, 10, None, "not_read", True),
                                                                   (5, 10, 10, 6, "clearance", False)],
                                             open_at=[10])])]}
    a.case("s44_spanned", s44, "a pin out of a hole that stops short of the next hole on its axis",
           ["J1", "J2", "B1", "B6"], False, {"block": "dia 10, z 0 to 10", "pin": "dia 6, z 5 to 12",
                                             "far": "dia 10, z 30 to 40"}, asm_expect(s44, sockets=sockets44))

    # s45: a hex bar, 5 across flats, through a dia 6.6 hole and out both sides: no shaft is read, B8 pins
    # the hole and opens both ends.
    plate45 = cut(box(-20, -20, 0, 40, 40, 10), cyl((0, 0, -1), UP, 3.3, 12))
    s45 = Node("s45_hexbar").add(Leaf("PLATE", plate45)).add(Leaf("BAR", hex_prism(5, -10, 30)))
    sockets45 = {"PLATE": [e_socket([e_axial("bore", (0, 0, 0), Z, [(0, 10, 6.6, None, "not_read", True)],
                                             open_at=[0, 10], flags=["occupant_unread"])])]}
    a.case("s45_hexbar", s45, "a hex bar through a round hole and out both sides", ["B8", "B6"], False,
           {"plate": "40 x 40 x 10, dia 6.6", "bar": "hex 5 across flats, z -10 to 20"},
           asm_expect(s45, sockets=sockets45))

    # s46: a dia 6 pin filling a lower plate's dia 6 hole, flush with its top, under an upper plate's dia 6.6
    # hole. The pin touches the upper hole's rim but does not enter it: J1 pins the upper hole.
    top46 = cut(box(-20, -20, 0, 40, 40, 10), cyl((0, 0, -1), UP, 3.3, 12))
    low46 = cut(box(-20, -20, -10, 40, 40, 10), cyl((0, 0, -11), UP, 3.0, 12))
    s46 = Node("s46_flush_pin").add(Leaf("TOP", top46)).add(Leaf("LOW", low46))
    s46.add(Leaf("PIN", cyl((0, 0, -10), UP, 3.0, 10)))
    sockets46 = {"TOP": [e_socket([e_axial("bore", (0, 0, 0), Z, [(0, 10, 6.6, None, "not_read", True)],
                                           open_at=[0])],
                                  # On the lower plate; the pin's end lies within the hole, touching nothing.
                                  seats=[e_seat((0, 0, 0), (0, 0, -1))])]}
    a.case("s46_flush_pin", s46, "a pin flush with the rim of the hole above it", ["J1", "C2"], False,
           {"top": "dia 6.6, z 0 to 10", "low": "dia 6, z -10 to 0", "pin": "dia 6, z -10 to 0"},
           asm_expect(s46, sockets=sockets46))

    # s18: a 90 deg countersunk screw, flush, in a dia 6.6 hole countersunk to dia 12.4, with a nut below. The
    # head's cone flares past the shank, so the shank does not stand for it (X7), and the cone is Q1's (PR 4).
    h18 = Hole((0, 0, 10), DOWN, [(6.6, 10)], through=10, countersink=(12.4, 90))
    plate18 = cut(box(-20, -20, 0, 40, 40, 10), tool(h18))
    screw18 = fuse(cone((0, 0, 6.8), UP, 3, 6.2, 3.2), cyl((0, 0, -8), UP, 3, 14.8))
    s18 = Node("s18_csink").add(Leaf("PLATE", plate18)).add(Leaf("SCREW", screw18)).add(Leaf("NUT", a_hex_nut()))
    sockets18 = {
        # The shank's end at the cone is unknown (air, then the head), so past z 6.8 nothing is read (U3):
        # pinned where the plate's dia 6.6 wall is, free in its countersink's zone.
        # The countersink is the hole's end zone, its profile dia 6.6 to 12.4 (X8): verifiable.
        "PLATE": [e_socket([e_axial("bore", (0, 0, 0), Z, [(0, 6.8, 6.6, 6, "clearance", False),
                                                          (6.8, 7.1, 6.6, None, "not_read", True),
                                                          (7.1, 10, 6.6, None, "not_read", False, "zone", (6.6, 12.4))],
                                    open_at=[0], flags=["extent_unknown"])],
                           # On the nut; the countersink is a 90 deg conical seat with its apex 6.2 below the
                           # top, which grips 3.8 to the bottom face (L2).
                           seats=[e_seat((0, 0, 0), (0, 0, -1))], cones=[e_cone((0, 0, 3.8), 90, Z, (3.3, 6.2))],
                           grips=[e_grip(3.8)])],
        "SCREW": [e_socket([e_axial("shaft", (0, 0, 0), Z, [(-5.01, 0.01, 6, None, "interference", True),
                                                           (0.01, 6.8, 6, 6.6, "clearance", False),
                                                           # Past the shank's known end (U3): unverifiable (U9).
                                                           (6.8, 10, 6, None, "not_read", False, "past_known_end")],
                                    flags=["extent_unknown", "thread_not_confirmed"])],
                           # The range both cones cover: the hole's dia 6.6 up to the head's dia 12.4.
                           cones=[e_cone((0, 0, 3.8), 90, Z, (3.3, 6.2), sense="shaft")])],
    }
    a.case("s18_csink", s18, "a 90 deg countersunk screw, flush, with a nut",
           ["X7", "X8", "U3", "U9", "B1", "B6", "I1", "I2", "Q1", "Q2", "Q3", "L2"], False,
           {"plate": "40 x 40 x 10, dia 6.6 through, countersunk 90 deg to dia 12.4 at z 10",
            "screw": "dia 6 from z -8, head cone dia 6 at z 6.8 to dia 12.4 at z 10", "nut": "z -5 to 0"},
           asm_expect(s18, sockets=sockets18))

    # s58: s18's countersunk screw in a plate whose top is inclined 2 degrees, below the countersink's rim (z 9.6 on
    # the axis): the cone breaks through the inclined face in an ellipse, no circle, so its radial range is unknown
    # and the conical seat has none: an unverifiable requirement (Y4, 8.4).
    rise58 = 20 * math.tan(math.radians(2))
    profile58 = (cq.Workplane("XZ").polyline([(-20, 0), (20, 0), (20, 9.6 + rise58), (-20, 9.6 - rise58)])
                 .close().extrude(-20)).translate((0, -10, 0))
    plate58 = cut(profile58.val().wrapped, tool(h18))
    s58 = Node("s58_csink_tilted").add(Leaf("PLATE", plate58)).add(Leaf("SCREW", screw18))
    a.case("s58_csink_tilted", s58, "a countersunk screw in a plate whose inclined top cuts the countersink",
           ["Q1", "U9", "Y4.witness"], False,
           {"plate": "40 x 20, top inclined 2 deg through z 9.6 on the axis, dia 6.6 through, countersunk 90 deg "
            "to dia 12.4 at z 10", "screw": "s18's"},
           # The head stands proud of the inclined top, which touches its cone along a curve no rule reads (C6).
           asm_expect(s58, sockets={"PLATE": [e_socket(
               [e_axial("bore", (0, 0, 0), Z, [(0, 6.8, 6.6, 6, "not_read", True), (6.8, 7.1, 6.6, None, "not_read", True),
                                               (7.1, 10, 6.6, None, "not_read", False, "past_known_end")],
                        # Its inclined top's unread edge leaves the plate unbounded: nothing shows that face
                        # clear of the core (U11).
                        open_at=[0], flags=["core_not_clear", "extent_unknown", "occupant_unread"],
                        depth="unknown_end")],
               unresolved={"contact_no_rule": 1}, cones=[e_cone((0, 0, 3.8), 90, Z, None)])]}))

    # s60: s18's countersunk plate with two neighbour cones on its one countersink, apart: a screw whose head stops
    # at dia 9.4 (z 8.5) and a conical plug from dia 10.4 (z 9) to the rim's dia 12.4. The conical seat states the
    # range from the least to the greatest of theirs (Q1, the socket PR's SR8), not the first neighbour's.
    screw60 = fuse(cone((0, 0, 6.8), UP, 3, 4.7, 1.7), cyl((0, 0, -8), UP, 3, 14.8))
    plug60 = cone((0, 0, 9), UP, 5.2, 6.2, 1)
    s60 = Node("s60_two_cones").add(Leaf("PLATE", plate18)).add(Leaf("SCREW", screw60)).add(Leaf("PLUG", plug60))
    a.case("s60_two_cones", s60, "a countersink holding a screw's head and a conical plug apart", ["Q1", "Q3", "X8", "B8"],
           False,
           {"plate": "s18's", "screw": "s18's, its head cone stopping at dia 9.4, z 8.5",
            "plug": "a cone from dia 10.4 at z 9 to dia 12.4 at z 10"},
           asm_expect(s60, sockets={"PLATE": [e_socket(
               # The plug is material in the hole that is no shaft (B8): the screw's spans are pinned, to its
               # head's top at z 8.5.
               [e_axial("bore", (0, 0, 0), Z, [(0, 6.8, 6.6, 6, "not_read", True),
                                               (6.8, 7.1, 6.6, None, "not_read", True),
                                               (7.1, 8.5, 6.6, None, "not_read", False, "zone", (6.6, 9.4))],
                        open_at=[0], flags=["extent_unknown", "occupant_unread"])],
               cones=[e_cone((0, 0, 3.8), 90, Z, (3.3, 6.2))])]}))

    # s64: a base with an annular groove, dia 12 to 6 deep, around its own dia 6 post (top 1 below the base's),
    # and a dia 11.8 / 6.6 sleeve in the groove, on its floor, standing 4 out. The groove's bore holds the base's
    # own post in its core, and the post's core the groove's wall: both unverifiable (U11).
    base64 = fuse(cut(box(-20, -20, -10, 40, 40, 10), cyl((0, 0, -6), UP, 6, 7)), cyl((0, 0, -6.5), UP, 3, 5.5))
    sleeve64 = cut(cyl((0, 0, -6), UP, 5.9, 10), cyl((0, 0, -7), UP, 3.3, 12))
    s64 = Node("s64_sleeve_post").add(Leaf("BASE", base64)).add(Leaf("SLEEVE", sleeve64))
    a.case("s64_sleeve_post", s64, "a sleeve in a groove round its base's own post", ["U11"], False,
           {"base": "40 x 40 x 10, top at z 0, groove dia 12 to z -6 round a dia 6 post to z -1",
            "sleeve": "dia 11.8 / 6.6, z -6 to 4"},
           asm_expect(s64, sockets={"BASE": [e_socket(
               [e_axial("bore", (0, 0, -6), Z, [(0, 6, 12, 11.8, "clearance", False)], open_at=[6],
                        flags=["core_not_clear"]),
                e_axial("shaft", (0, 0, -6), Z, [(0, 5, 6, 6.6, "clearance", False)], flags=["core_not_clear"])],
               seats=[e_seat((0, 0, -6), Z)])]}))

    # PR 4's cases: seats, grips, partial cylinders and the near band.

    # s05: plates B (z 0 to 10), M (10 to 16) and T (16 to 24) on one bolt with a nut below, and two washers
    # under the head (z 24 and 25.6): two occurrences of one part, each the other's neighbour.
    def plate05(z0, h):
        return through(box(-20, -20, z0, 40, 40, h), [(0, 0)], 6.6, z0, z0 + h)
    washer = Leaf("WASHER", cut(cyl((0, 0, 0), UP, 6, 1.6), cyl((0, 0, -1), UP, 3.2, 3.6)))
    s05 = Node("s05_stack").add(Leaf("B", plate05(0, 10))).add(Leaf("M", plate05(10, 6))).add(Leaf("T", plate05(16, 8)))
    s05.add(washer, m_tr((0, 0, 24))).add(washer, m_tr((0, 0, 25.6)))
    s05.add(Leaf("BOLT", a_hex_bolt(length=35)), m_tr((0, 0, 27.2))).add(Leaf("NUT", a_hex_nut()))

    def plate_socket05(z0, h, above=1, rings=()):
        return [e_socket([e_axial("bore", (0, 0, z0), Z, [(0, h, 6.6, 6, "clearance", False)], open_at=[0, h])],
                         seats=[e_seat((0, 0, z0), (0, 0, -1)), e_seat((0, 0, z0 + h), Z, by=above, rings=rings)],
                         grips=[e_grip(h, holes=1)])]
    # Each washer: the bolt in its dia 6.4 hole, and the other washer's hole continuing it (J1, spanned by
    # the bolt), so the bore and the seat between them carry `self`. Each washer's dia 12 outside ends on the
    # hole rim below it (H1: T's, and the lower washer's), and the two outsides abut (I6).
    def washer05(z):
        between = {"by": 2, "flags": ["self"], "rings": [(6.4, 12)]}
        # The bolt's shank runs on past the lower washer's top, but ends under its head on the upper one's.
        return e_socket([e_axial("bore", (0, 0, 0), Z, [(0, 1.6, 6.4, 6, "clearance", False)],
                                 open_at=[0, 1.6] if z == 24 else [0], flags=["self"]),
                         e_axial("shaft", (0, 0, 0), Z, [], flags=["abutting", "self"])],
                        seats=[e_seat((0, 0, 0), (0, 0, -1), **(between if z == 25.6 else {})),
                               e_seat((0, 0, 1.6), Z, **(between if z == 24 else {}))],
                        grips=[e_grip(1.6, holes=1)], at=(0, 0, z))
    sockets05 = {"B": plate_socket05(0, 10), "M": plate_socket05(10, 6),
                 "T": plate_socket05(16, 8, above=2, rings=[(6.6, 12)]), "WASHER": [washer05(24), washer05(25.6)],
                 # The bolt, head bearing at z 27.2: the two washers' dia 6.4, the three plates' dia 6.6 (24 mm),
                 # then the nut's dia 5. Nothing of its head touches the washers' outsides (I5, C6).
                 "BOLT": [e_socket([e_axial("shaft", (0, 0, 0), (0, 0, -1),
                                            [(0, 3.21, 6, 6.4, "clearance", False), (3.21, 27.19, 6, 6.6, "clearance", False),
                                             (27.19, 32.21, 6, None, "interference", True)],
                                            flags=["thread_not_confirmed"])],
                                   seats=[e_seat((0, 0, 0), (0, 0, -1), touchers=[("WASHER", 0, [])])],
                                   at=(0, 0, 27.2))],
                 "NUT": [e_socket([e_axial("bore", (0, 0, -5), Z, [(0, 5, 5, 6, "interference", True)], open_at=[0, 5],
                                           flags=["thread_not_confirmed"])], seats=[e_seat((0, 0, 0), Z)])]}
    a.case("s05_stack", s05, "three plates and two washers on one bolt: seats, grips and a washer against its copy",
           ["C1", "C2", "C4", "L1", "H1", "I5", "I6", "N2", "J1"], False,
           {"plates": "B z 0 to 10, M 10 to 16, T 16 to 24, 40 x 40, dia 6.6", "washers": "dia 12 / 6.4 x 1.6",
            "bolt": "hex, dia 6 x 35, head bearing at z 27.2", "nut": "z -5 to 0"},
           asm_expect(s05, sockets=sockets05))

    # s04: a 608 bearing (dia 22 / 8 x 7) in a housing's dia 22 seat over a dia 18 bore, on a dia 8 shaft. The
    # ring is line to line in both (pinned), sits on the seat's floor, and its outside ends on the dia 18
    # bore's rim there (H1).
    housing04 = cut(box(-20, -20, 0, 40, 40, 20), cyl((0, 0, -1), UP, 9, 22), cyl((0, 0, 13), UP, 11, 8))
    bearing04 = cut(cyl((0, 0, 0), UP, 11, 7), cyl((0, 0, -1), UP, 4, 9))
    s04 = Node("s04_bearing").add(Leaf("HOUSING", housing04)).add(Leaf("BEARING", bearing04), m_tr((0, 0, 13)))
    s04.add(Leaf("SHAFT", cyl((0, 0, -10), UP, 4, 40)))
    sockets04 = {
        "HOUSING": [e_socket([e_axial("bore", (0, 0, 0), Z, [(0, 13, 18, 8, "clearance", False),
                                                            (13, 20, 22, 22, "line_to_line", True)],
                                      open_at=[0, 20], flags=["thread_not_confirmed"])],
                             seats=[e_seat((0, 0, 13), Z, by=2, rings=[(18, 22)])])],
        "BEARING": [e_socket([e_axial("bore", (0, 0, 0), Z, [(0, 7, 8, 8, "line_to_line", True)], open_at=[0, 7],
                                      flags=["thread_not_confirmed"]),
                              # Its lower end bears on the seat's step to dia 18 (I4).
                              e_axial("shaft", (0, 0, 0), Z, [(0, 7, 22, None, "line_to_line", True)],
                                      flags=["thread_not_confirmed"], max_extra=0)],
                             seats=[e_seat((0, 0, 0), (0, 0, -1))], at=(0, 0, 13))],
    }
    a.case("s04_bearing", s04, "a bearing in a housing's seat, on a shaft", ["B2", "B7", "I2", "I5", "C1", "H1"], False,
           {"housing": "40 x 40 x 20, dia 18 through, dia 22 from z 13", "bearing": "dia 22 / 8 x 7 at z 13",
            "shaft": "dia 8, z -10 to 30"}, asm_expect(s04, sockets=sockets04))

    # s13: a block whose outline has a spline side sits on a plate, a lid on it: its bottom and top faces'
    # outlines are unknown, so their seats' bases are too (C4, U6), the grip between them is unverifiable (U10),
    # and the spline side is a face no rule reads (U5).
    spline13 = (cq.Workplane("XY").moveTo(0, 0).lineTo(30, 0).lineTo(30, 20)
                .spline([(15, 25), (0, 20)], includeCurrent=True).close().extrude(10)).val().wrapped
    s13 = (Node("s13_unknown_outline").add(Leaf("PLATE", box(-10, -10, -10, 60, 50, 10))).add(Leaf("BLOCK", spline13))
           .add(Leaf("LID", box(-10, -10, 10, 60, 50, 10))))
    sockets13 = {
        "BLOCK": [e_socket([], unresolved={"target_unread_face": 1},
                           seats=[e_seat((0, 0, 0), (0, 0, -1), flags=["unknown_outline"], basis=("unknown",)),
                                  e_seat((0, 0, 10), Z, flags=["unknown_outline"], basis=("unknown",))],
                           grips=[e_grip(10, flags=["seat_unverifiable"])])],
        "PLATE": [e_socket([], unresolved={"neighbour_unread": 1},
                           seats=[e_seat((0, 0, 0), Z, flags=["unknown_outline"])])],
    }
    a.case("s13_unknown_outline", s13, "a block with a spline side between a plate and a lid: seats of unknown outline",
           ["C3", "C4", "U5", "U6", "U4", "U10", "L1"], False,
           {"plate": "60 x 50 x 10, top at z 0", "block": "30 x 20 x 10 with a spline side bulging to y 25",
            "lid": "60 x 50 x 10, bottom at z 10"},
           asm_expect(s13, sockets=sockets13))

    # s14: a box overlapping the block by 10 x 10 x 5, with no face of one near a face of the other: no rule
    # relates them. M7 catches the clash.
    s14 = Node("s14_clash").add(Leaf("BLOCK", box(0, 0, 0, 20, 20, 10))).add(Leaf("CLASH", box(10, 10, 5, 20, 20, 10)))
    a.case("s14_clash", s14, "a box overlapping the block, no face near another", ["N3"], False,
           {"block": "20 x 20 x 10", "clash": "20 x 20 x 10 from (10, 10, 5)"},
           asm_expect(s14, sockets={"BLOCK": [e_socket([])]}))

    # s17: a dia 6 pin through one end of a 6.6-wide slot 20 long in a 10 mm plate. The slot's ends are half
    # holes: the occupied one is a partial bore (U1), the other is required of nothing; the pin is pinned in
    # its partial hole (I7).
    slot17 = fuse(cyl((0, 0, -1), UP, 3.3, 12), cyl((13.4, 0, -1), UP, 3.3, 12), box(0, -3.3, -1, 13.4, 6.6, 12))
    s17 = Node("s17_slot").add(Leaf("PLATE", cut(box(-15, -15, 0, 45, 30, 10), slot17)))
    s17.add(Leaf("PIN", cyl((0, 0, -5), UP, 3, 20)))
    sockets17 = {
        # The pin runs past both ends of the half hole, which are not known ends (U1, B6, U8).
        "PLATE": [e_socket([], partials=[e_partial("partial_bore", (0, 0, 0), Z, 6.6, 6, 0, 10, math.pi,
                                                   flags=["open_at_unknown_end"])])],
        "PIN": [e_socket([e_axial("shaft", (0, 0, 0), Z, [(-0.01, 10.01, 6, None, "not_read", True)],
                                  flags=["partial_neighbour"])])],
    }
    a.case("s17_slot", s17, "a dia 6 pin at one end of a 6.6 wide slot", ["U1", "I7"], False,
           {"plate": "45 x 30 x 10", "slot": "6.6 wide, centres (0, 0) and (13.4, 0)", "pin": "dia 6, z -5 to 15"},
           asm_expect(s17, sockets=sockets17))

    # s20: two blocks on a plate, one 0.05 mm above its top face and one 0.05 mm into it: one seat, both in
    # the near band (C5).
    s20 = Node("s20_near").add(Leaf("PLATE", box(-50, -20, -10, 100, 40, 10)))
    s20.add(Leaf("ABOVE", box(-40, -10, 0.05, 20, 20, 10))).add(Leaf("INTO", box(20, -10, -0.05, 20, 20, 10)))
    sockets20 = {"PLATE": [e_socket([], seats=[e_seat((0, 0, 0), Z, by=2, flags=["interference", "near_gap"],
                                                      touchers=[("ABOVE", 0.05, ["near_gap"]),
                                                                ("INTO", -0.05, ["interference"])])])]}
    a.case("s20_near", s20, "blocks 0.05 mm above and 0.05 mm into a plate", ["C5", "Z6"], False,
           {"plate": "100 x 40 x 10, top at z 0", "above": "20 x 20 x 10 at z 0.05", "into": "at z -0.05"},
           asm_expect(s20, sockets=sockets20))

    # s48: a dia 10 D-shaft line to line in a bushing's dia 10 hole along x, its flat beyond the bushing. The
    # shaft's cylinder is partial (U1): it is no round shaft, so B8 pins the hole, and the shaft gets a partial
    # shaft held line to line.
    bush48 = cut(box(-5, -15, 0, 10, 30, 30), cyl((-6, 0, 20), (1, 0, 0), 5, 12))
    dshaft48 = cut(cyl((-30, 0, 20), (1, 0, 0), 5, 60), box(15, 4, 10, 20, 10, 20))
    s48 = Node("s48_dshaft").add(Leaf("BUSHING", bush48)).add(Leaf("DSHAFT", dshaft48))
    s48.add(Leaf("BASE", box(-30, -30, -10, 60, 60, 10)))
    sockets48 = {
        "BUSHING": [e_socket([e_axial("bore", (-5, 0, 20), (1, 0, 0), [(0, 10, 10, None, "not_read", True)],
                                      open_at=[0, 10], flags=["occupant_unread"])],
                             seats=[e_seat((0, 0, 0), (0, 0, -1))])],
        "DSHAFT": [e_socket([], partials=[e_partial("partial_shaft", (-30, 0, 20), (1, 0, 0), 10, None, 24.99, 35.01,
                                                    2 * math.pi, flags=["thread_not_confirmed"])])],
    }
    a.case("s48_dshaft", s48, "a D-shaft line to line in a bushing", ["U1", "B8", "I2"], False,
           {"bushing": "10 x 30 x 30, dia 10 along x at z 20", "dshaft": "dia 10 x 60 from x -30, flat 1 mm deep from x 15",
            "base": "60 x 60 x 10, top at z 0"}, asm_expect(s48, sockets=sockets48))

    # s49: a dia 40 / 30 tube sunk 0.5 mm into a block's dia 30 hole, and a dia 30 / 10 spacer line to line in
    # the tube's bore, clear of the block. Seen from the spacer, the tube's outside is wider than the spacer's bore, but the tube's
    # bore clears the spacer's outside: no interference. The block has no round outside to clear the tube's
    # bore against, and the tube really is in its material: an interference.
    block49 = cut(box(-25, -25, 0, 50, 50, 10), cyl((0, 0, -1), UP, 15, 12))
    s49 = Node("s49_ring").add(Leaf("BLOCK", block49))
    s49.add(Leaf("TUBE", cut(cyl((0, 0, 9.5), UP, 20, 10.5), cyl((0, 0, 8), UP, 15, 13))))
    s49.add(Leaf("SPACER", cut(cyl((0, 0, 12), UP, 15, 8), cyl((0, 0, 11), UP, 5, 10))))
    sockets49 = {
        # The spacer's bore continues the block's hole across the gap (J2): pinned, nothing in it.
        "SPACER": [e_socket([e_axial("shaft", (0, 0, 12), Z, [(0, 8, 30, None, "line_to_line", True)],
                                     flags=["thread_not_confirmed"]),
                             e_axial("bore", (0, 0, 12), Z, [(0, 8, 10, None, "not_read", True)], open_at=[0])])],
        # The tube: its outside is no interference in the spacer's bore (the ring rule, from the ring's side),
        # but is in the block's hole. The block's top meets its bore's core at the rim (touching counts): B8
        # pins the bore below the spacer.
        "TUBE": [e_socket([e_axial("shaft", (0, 0, 9.5), Z, [(0, 0.51, 40, None, "interference", True)],
                                   flags=["thread_not_confirmed"]),
                           e_axial("bore", (0, 0, 9.5), Z, [(0, 2.49, 30, None, "not_read", True),
                                                             (2.49, 10.5, 30, 30, "line_to_line", True)],
                                   flags=["occupant_unread", "thread_not_confirmed"])],
                          seats=[e_seat((0, 0, 9.5), (0, 0, -1), flags=["interference"],
                                        touchers=[("BLOCK", -0.5, ["interference"])])])],
        "BLOCK": [e_socket([e_axial("bore", (0, 0, 0), Z, [(9.49, 10, 30, 40, "interference", True)], open_at=[10],
                                    flags=["thread_not_confirmed"])],
                           seats=[e_seat((0, 0, 10), Z, flags=["interference"],
                                         touchers=[("TUBE", -0.5, ["interference"])])])],
    }
    a.case("s49_ring", s49, "a spacer in a tube's bore, the tube sunk into a block", ["B3", "I2", "J2", "C5"], False,
           {"block": "50 x 50 x 10, dia 30 through", "tube": "dia 40 / 30, z 9.5 to 20",
            "spacer": "dia 30 / 10, z 12 to 20"}, asm_expect(s49, sockets=sockets49))

    # s50: a cap on a dia 40 / 16 tube: its dia 16.1 spigot pressed 0.05 mm into the tube's bore, its dia 50 / 42
    # skirt clear around the tube. The skirt's bore is wider than the tube's outside, but it is no bore of the
    # spigot's wall: the press fit stands on both sides (the ring rule's counterexample).
    tube50 = cut(cyl((0, 0, 0), UP, 20, 30), cyl((0, 0, -1), UP, 8, 32))
    cap50 = fuse(cyl((0, 0, 0), UP, 8.05, 30.5), cyl((0, 0, 30), UP, 25, 3),
                 cut(cyl((0, 0, 0), UP, 25, 30.5), cyl((0, 0, -1), UP, 21, 32)))
    s50 = Node("s50_cap").add(Leaf("TUBE", tube50)).add(Leaf("CAP", cap50))
    sockets50 = {
        # The tube's top meets the cap's underside: its outside may be no longer (I4, 0).
        "TUBE": [e_socket([e_axial("shaft", (0, 0, 0), Z, [(0, 30, 40, 42, "clearance", False)], max_extra=0),
                           e_axial("bore", (0, 0, 0), Z, [(0, 30, 16, 16.1, "interference", True)],
                                   flags=["thread_not_confirmed"])],
                          seats=[e_seat((0, 0, 30), Z)])],
        # The skirt's outside holds nothing and is held by nothing.
        # The spigot's core holds the cap's own skirt, and the skirt's bore the cap's own spigot: both
        # unverifiable (U11) until a reviewer removes them.
        "CAP": [e_socket([e_axial("shaft", (0, 0, 30), (0, 0, -1), [(0, 30, 16.1, None, "interference", True)],
                                  flags=["core_not_clear", "thread_not_confirmed"]),
                          e_axial("bore", (0, 0, 30), (0, 0, -1), [(0, 30, 42, 40, "clearance", False)],
                                  flags=["core_not_clear"])],
                         left_out=[e_left((0, 0, 0), Z, 50, wall=(0, 33))], seats=[e_seat((0, 0, 30), (0, 0, -1))])],
    }
    a.case("s50_cap", s50, "a cap pressed into a tube's bore, its skirt clear around the tube", ["B3", "I2", "U11"],
           False,
           {"tube": "dia 40 / 16, z 0 to 30", "cap": "dia 16.1 spigot from z 0, dia 50 / 42 skirt from z 0, "
            "top dia 50 from z 30 to 33"}, asm_expect(s50, sockets=sockets50))

    # s51: a dia 6 pin 0.5 mm off the axis of a 10 mm plate's dia 6.6 hole: it runs 0.2 mm into the wall (B4),
    # a conflict on both sides.
    plate51 = cut(box(-20, -20, 0, 40, 40, 10), cyl((0, 0, -1), UP, 3.3, 12))
    s51 = Node("s51_wall").add(Leaf("PLATE", plate51)).add(Leaf("PIN", cyl((0.5, 0, -5), UP, 3, 20)))
    a.case("s51_wall", s51, "a pin through a hole's wall", ["B4"], False,
           {"plate": "40 x 40 x 10, dia 6.6", "pin": "dia 6, z -5 to 15, 0.5 mm off the hole's axis"},
           asm_expect(s51, sockets={
               "PLATE": [e_socket([e_axial("bore", (0, 0, 0), Z, [(0, 10, 6.6, 7, "interference", True)], open_at=[0, 10],
                                           flags=["conflict"])])],
               "PIN": [e_socket([e_axial("shaft", (0.5, 0, -5), Z, [(4.99, 15.01, 6, None, "interference", True)],
                                         flags=["conflict"])])]}))

    # s52: a dia 4 pin at 10 degrees through a 10 mm plate's dia 10 hole, touching nothing: a skew relation,
    # unresolved on both sides (U7); the plate's bore is pinned by the pin's unread (skew) material (B8).
    t52 = math.radians(10)
    plate52 = cut(box(-20, -20, 0, 40, 40, 10), cyl((0, 0, -1), UP, 5, 12))
    pin52 = cyl((-10 * math.sin(t52), 0, 5 - 10 * math.cos(t52)), (math.sin(t52), 0, math.cos(t52)), 2, 20)
    s52 = Node("s52_skew").add(Leaf("PLATE", plate52)).add(Leaf("PIN", pin52))
    a.case("s52_skew", s52, "a pin at 10 degrees through a hole", ["U7", "B8"], False,
           {"plate": "40 x 40 x 10, dia 10", "pin": "dia 4 x 20, centred at (0, 0, 5), 10 degrees off z toward x"},
           asm_expect(s52, sockets={
               "PLATE": [e_socket([e_axial("bore", (0, 0, 0), Z, [(0, 10, 10, None, "not_read", True)], open_at=[0, 10],
                                           flags=["occupant_unread"])], unresolved={"skew": 1})],
               "PIN": [e_socket([], unresolved={"skew": 1})]}))

    # s53: a dia 50 flange standing on a plate, over two dia 6.6 holes 18 mm off its axis: C1's seat, no H1
    # ring (H2); the holes hold nothing.
    plate53 = cut(box(-40, -40, -10, 80, 80, 10), cyl((18, 0, -11), UP, 3.3, 12), cyl((-18, 0, -11), UP, 3.3, 12))
    s53 = Node("s53_flange").add(Leaf("PLATE", plate53)).add(Leaf("FLANGE", cyl((0, 0, 0), UP, 25, 8)))
    a.case("s53_flange", s53, "a flange over holes off its axis", ["H2", "C1", "A1"], False,
           {"plate": "80 x 80 x 10, top at z 0, dia 6.6 at x -18 and 18", "flange": "dia 50 x 8 on it"},
           asm_expect(s53, sockets={
               "PLATE": [e_socket([], left_out=[e_left((-18, 0, -10), Z, 6.6, wall=(0, 10)),
                                                e_left((18, 0, -10), Z, 6.6, wall=(0, 10))], seats=[e_seat((0, 0, 0), Z)])],
               "FLANGE": [e_socket([], left_out=[e_left((0, 0, 0), Z, 50, wall=(0, 8))],
                                   seats=[e_seat((0, 0, 0), (0, 0, -1))])]}))

    # s15: a block and its mirror image, two products, each on a dia 6 pin standing in a plate. The mirror is
    # modelled mirrored, so its socket is the block's mirrored.
    block15 = cut(box(0, -10, 0, 20, 20, 8), cyl((12, 5, -1), UP, 3.3, 10))
    mirror = gp_Trsf()
    mirror.SetMirror(gp_Ax2(gp_Pnt(0, 0, 0), gp_Dir(1, 0, 0)))
    mirror15 = BRepBuilderAPI_Transform(block15, mirror, True).Shape()
    plate15 = cut(box(-30, -20, -10, 60, 40, 10), cyl((12, 5, -11), UP, 3, 12), cyl((-12, 5, -11), UP, 3, 12))
    s15 = Node("s15_mirror_part").add(Leaf("PLATE", plate15)).add(Leaf("BLOCK", block15))
    pin15 = Leaf("PIN", cyl((0, 0, -10), UP, 3, 18))
    s15.add(Leaf("MIRROR", mirror15)).add(pin15, m_tr((12, 5, 0))).add(pin15, m_tr((-12, 5, 0)))
    a.case("s15_mirror_part", s15, "a block and its mirror image as two products", ["P7"], False,
           {"plate": "60 x 40 x 10, top at z 0, dia 6 at (12, 5) and (-12, 5)",
            "block": "20 x 20 x 8 from x 0, dia 6.6 at (12, 5)", "mirror": "the block mirrored in x = 0",
            "pins": "dia 6, z -10 to 8"}, asm_expect(s15, sockets={
               "BLOCK": [e_socket([e_axial("bore", (12, 5, 0), Z, [(0, 8, 6.6, 6, "clearance", False)], open_at=[0])],
                                  seats=[e_seat((0, 0, 0), (-1, 0, 0)), e_seat((0, 0, 0), (0, 0, -1))])],
               "MIRROR": [e_socket([e_axial("bore", (-12, 5, 0), Z, [(0, 8, 6.6, 6, "clearance", False)], open_at=[0])],
                                   seats=[e_seat((0, 0, 0), (1, 0, 0)), e_seat((0, 0, 0), (0, 0, -1))])]}))

    # s22: a block meant to rest flipped (mirrored in z = 4, then up 8) on a pedestal, round a pin through the
    # pedestal's dia 6 hole at (5, 0). Open CASCADE 7.9.3 writes that mirror as its shift alone (P8): the file
    # places the block unturned, 16 mm up, 8 mm above the pedestal. The answers are the file's: the pin runs
    # through the block's dia 6.6 hole, and nothing seats it. The generator checks Open CASCADE still writes so.
    block22 = cut(box(-10, -5, 0, 20, 10, 8), cyl((5, 0, -1), UP, 3.3, 10))
    pedestal22 = cut(box(-15, -10, 0, 30, 20, 8), cyl((5, 0, -1), UP, 3, 10))
    mirror22 = [[1, 0, 0, 0], [0, 1, 0, 0], [0, 0, -1, 16], [0, 0, 0, 1]]
    written22 = m_tr((0, 0, 16))
    s22_file = Node("s22_mirror_intent").add(Leaf("PEDESTAL", pedestal22))
    s22_file.add(Leaf("BLOCK", block22), mirror22).add(Leaf("PIN", cyl((5, 0, 0), UP, 3, 30)))
    s22 = Node("s22_mirror_intent").add(s22_file.children[0][0]).add(s22_file.children[1][0], written22)
    s22.add(s22_file.children[2][0])
    a.case("s22_mirror_intent", s22, "a block placed through a mirror, as Open CASCADE writes it", ["P8"], False,
           {"pedestal": "30 x 20 x 8, dia 6 at (5, 0)", "block": "20 x 10 x 8, dia 6.6 at (5, 0), mirrored in z = 4 "
            "and moved up 8", "pin": "dia 6, z 0 to 30 at (5, 0)", "written": "the shift alone"},
           asm_expect(s22, sockets={"BLOCK": [e_socket([e_axial("bore", (5, 0, 0), Z, [(0, 8, 6.6, 6, "clearance", False)],
                                                                open_at=[0, 8])], at=(0, 0, 16))]}),
           written=s22_file)

    # s54: two stepped blind holes in a base (top at z 0), dia 6.2 to 10 deep and dia 4 to 15 deep; a block on
    # it carries a dia 6 pin to 6 deep in one and a dia 3.5 pin to 6 deep in the other. The dia 6 pin can go 4
    # further, to the step; the dia 3.5 one passes the step, 9 to the floor (I4).
    def stepped(x):
        return [cyl((x, 0, -10), UP, 3.1, 11), cyl((x, 0, -15), UP, 2, 6)]
    base54 = cut(box(-25, -15, -20, 50, 30, 20), *stepped(-10), *stepped(10), cyl((0, 0, -10), UP, 3.1, 11))
    block54 = fuse(box(-20, -10, 0, 40, 20, 8), cyl((-10, 0, -6), UP, 3, 7), cyl((10, 0, -6), UP, 1.75, 7))
    s54 = Node("s54_stepped").add(Leaf("BASE", base54)).add(Leaf("BLOCK", block54))
    # And a loose dia 6 pin, both ends free, buried in a third, plain blind hole at x 0: 0.5 off its floor.
    s54.add(Leaf("PIN", cyl((0, 0, -9.5), UP, 3, 8)))
    a.case("s54_stepped", s54, "pins ending in the wider segment of stepped blind holes", ["I4", "I1", "U11"], False,
           {"base": "50 x 30 x 20, top at z 0; at x -10 and 10: dia 6.2 to z -10, dia 4 to z -15",
            "block": "40 x 20 x 8 on it, with pins dia 6 at x -10 and dia 3.5 at x 10 to z -6",
            "pin": "dia 6, z -9.5 to -1.5, in a dia 6.2 hole to z -10 at x 0"},
           # Each pin's core, unbounded across its stations, holds the other pin: both unverifiable (U11).
           asm_expect(s54, sockets={"BLOCK": [e_socket(
               [e_axial("shaft", (-10, 0, 0), (0, 0, -1), [(0, 6, 6, 6.2, "clearance", False)], max_extra=4,
                        flags=["core_not_clear"]),
                e_axial("shaft", (10, 0, 0), (0, 0, -1), [(0, 6, 3.5, 6.2, "clearance", False)], max_extra=9,
                        flags=["core_not_clear"])],
               seats=[e_seat((0, 0, 0), (0, 0, -1))])],
               # Both its ends free: the lower meets the floor (I4).
               "PIN": [e_socket([e_axial("shaft", (0, 0, -9.5), Z, [(0, 8, 6, 6.2, "clearance", False)], max_extra=0.5)],
                                seats=[e_seat((0, 0, -9.5), (0, 0, -1), flags=["near_gap"],
                                              touchers=[("BASE", 0.5, ["near_gap"])])])]}))

    # s55: a plate fused from 60 x 60 tiles 2 mm square, its top face 3,600 coplanar faces, and a 20 x 20 block on
    # it: 144 tiles touch it (the 100 under it and the 44 its edges meet; touching counts), all one seat, and
    # the block's seat has those 144 faces on it (N4: the searches keep this near linear). No variants: the
    # fuse is slow to repeat.
    tiles = [box(i * 2, j * 2, -10, 2, 2, 10) for i in range(60) for j in range(60)]
    first, rest = TopTools_ListOfShape(), TopTools_ListOfShape()
    first.Append(tiles[0])
    for t in tiles[1:]:
        rest.Append(t)
    fused = BRepAlgoAPI_Fuse()
    fused.SetArguments(first)
    fused.SetTools(rest)
    fused.Build()
    s55 = Node("s55_tiles").add(Leaf("PLATE", fused.Shape())).add(Leaf("BLOCK", box(10, 10, 0, 20, 20, 5)))
    a.case("s55_tiles", s55, "a block on a plate whose top is 3,600 coplanar faces", ["N4", "C1"], False,
           {"plate": "60 x 60 tiles 2 x 2 x 10 fused, top at z 0", "block": "20 x 20 x 5 at (10, 10, 0)",
            "socket_oracle": "skipped: Open CASCADE classifies points against 7,440 faces for hours"},
           asm_expect(s55, sockets={"PLATE": [e_socket([], seats=[e_seat((0, 0, 0), Z, basis=("target_face",) * 144)])],
                                    "BLOCK": [e_socket([], seats=[e_seat((0, 0, 0), (0, 0, -1), by=144)])]}),
           variants=False)

    # s56: a dia 8 / 4 tube 1000 mm above a plate's dia 4 hole, its axis leaning 0.009 off the hole's: within X1's
    # window, it continues the hole (J2) however far. The searches look along a cone about each axis, not a
    # cylinder (PR 7b's review).
    lean56 = (0.009, 0, math.sqrt(1 - 0.009 ** 2))
    tube56 = cut(cyl((0, 0, 1000), lean56, 4, 10), cyl((-lean56[0], 0, 1000 - lean56[2]), lean56, 2, 12))
    s56 = Node("s56_far_lean").add(Leaf("PLATE", cut(box(-20, -20, 0, 40, 40, 10), cyl((0, 0, -1), UP, 2, 12))))
    s56.add(Leaf("TUBE", tube56))
    a.case("s56_far_lean", s56, "a leaning tube 1000 mm along a hole's axis continues it", ["J2", "N4"], False,
           {"plate": "40 x 40 x 10, dia 4", "tube": "dia 8 / 4 x 10 from (0, 0, 1000), leaning 0.009 toward x"},
           asm_expect(s56, sockets={"PLATE": [e_socket([e_axial("bore", (0, 0, 0), Z, [(0, 10, 4, None, "not_read", True)],
                                                                open_at=[10])])]}))

    # s57: a dia 40 disc with a dia 8 hole, tilted 0.5 degrees on a 10 mm block: C1's gap is the least over the
    # disc's outline, its hole's vertices among them (PR 7b's review: the index keeps their hull).
    disc57 = cut(cyl((0, 0, 0), UP, 20, 2), cyl((-10, 0, -1), UP, 4, 4))
    s57 = Node("s57_tilted_disc").add(Leaf("BLOCK", box(-5, -5, -10, 10, 10, 10)))
    s57.add(Leaf("DISC", disc57), m_rot((0, 1, 0), -0.5, (0, 0, 10 * math.sin(math.radians(0.5)))))
    a.case("s57_tilted_disc", s57, "a holed disc tilted half a degree on a block", ["C1", "C5"], False,
           {"block": "10 x 10 x 10, top at z 0", "disc": "dia 40 x 2, dia 8 hole at (-10, 0), turned -0.5 degrees "
            "about y and lifted 10 sin 0.5 degrees"},
           asm_expect(s57, sockets={"BLOCK": [e_socket([], seats=[e_seat(
               (0, 0, 0), Z, flags=["near_gap"], touchers=[("DISC", 4 * math.sin(math.radians(0.5)), ["near_gap"])])])]}))

    # s47: a dia 6 rod lying across a plate's top face: a curved face on a flat, which no rule reads (C6).
    s47 = Node("s47_rod_on_flat").add(Leaf("PLATE", box(-30, -20, -10, 60, 40, 10)))
    s47.add(Leaf("ROD", cyl((-20, 0, 3), (1, 0, 0), 3, 40)))
    a.case("s47_rod_on_flat", s47, "a rod lying across a plate", ["C6"], False,
           {"plate": "60 x 40 x 10, top at z 0", "rod": "dia 6 x 40 along x, axis at z 3"},
           asm_expect(s47, sockets={"PLATE": [e_socket([], unresolved={"contact_no_rule": 1})]}))

    # Refusals and ambiguities.

    s21 = Node("r21_name_ambiguous").add(long_plate, m_tr((50, 0, 0)))
    s21.add(Leaf("BRACKET-A", a_bracket())).add(Leaf("BRACKET-B", a_bracket()), turned)

    def rename(f: Part21):
        for n in f.find("PRODUCT("):
            f.inst[n] = re.sub(r"^PRODUCT\('(BRACKET-[AB])','[^']*'", r"PRODUCT('\1','BRACKET'", f.inst[n])

    exp21 = asm_expect(s21, select=[{"part": "BRACKET", "error": "part_ambiguous",
                                     "suggestions": ["BRACKET-A", "BRACKET-B"]},
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
    exp32 = asm_expect(r32, select=[{"part": "BOLT", "error": "part_ambiguous", "suggestions": ["BOLT", "BOLT"]}])
    exp32["listing"]["parts"] = sorted([{"id": "BOLT", "name": "BOLT", "kind": "part", "quantity": 1}] * 2 + [
        p for p in exp32["listing"]["parts"] if p["id"] != "BOLT"], key=lambda p: (p["id"], p["kind"]))
    exp32["occurrences"] = {"BOLT": occurrences(r32)["BOLT"]}
    a.case("r32_id_ambiguous", r32, "two different parts, both with PRODUCT.id BOLT, in two sub-assemblies", ["T3"],
           False, {"bolts": "M6 x 25 and M8 x 30"}, exp32)

    def refused33(kind):
        return {"result": "refused", "check": 33, "refusals": ["placement_unreadable"], "kind": kind}

    def unplaced(f: Part21):
        cdsr, _, _ = placement_of(f, usage_of(f, "SCREW"))
        del f.inst[cdsr]

    a.case("r24_unplaced", s02, "s02 without the screw usage's context_dependent_shape_representation", ["m3.4.1#33"], False,
           {"from": "s02"}, asm_expect(s02, listing_result=refused33("unplaced")), edit=unplaced)

    def divergent(f: Part21):
        point = f.add("CARTESIAN_POINT('',(10.,0.,0.))")
        z, x = f.add("DIRECTION('',(0.,0.,1.))"), f.add("DIRECTION('',(1.,0.,0.))")
        replace_component_item(f, usage_of(f, "SCREW"), f"AXIS2_PLACEMENT_3D('',#{point},#{z},#{x})")

    a.case("r25_divergent", s02, "s02 with the screw's component-side frame moved to (10, 0, 0)", ["m3.4.1#33"], False,
           {"from": "s02"}, asm_expect(s02, listing_result=refused33("divergent")), edit=divergent)

    def mirrored(f: Part21):
        o = f.add("CARTESIAN_POINT('',(0.,0.,0.))")
        x = f.add("DIRECTION('',(-1.,0.,0.))")
        y = f.add("DIRECTION('',(0.,1.,0.))")
        z = f.add("DIRECTION('',(0.,0.,1.))")
        replace_component_item(f, usage_of(f, "SCREW"),
                               f"CARTESIAN_TRANSFORMATION_OPERATOR_3D('','',#{x},#{y},#{o},1.,#{z})")

    a.case("r26_mirrored", s02, "s02 with the screw's component-side frame a (mirroring) cartesian_transformation_operator_3d, "
           "which stepq does not read",
           ["m3.4.1#33"], False, {"from": "s02"}, asm_expect(s02, listing_result=refused33("unsupported")), edit=mirrored)

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
           asm_expect(s02, listing_result=refused33("quantified")), edit=quantified)

    def flip_face(child_name, pick):
        """Flips the same_sense of one face of `child_name`'s solid, `pick`
        choosing among its advanced faces by surface entity."""

        def edit(f: Part21):
            product = next(n for n in f.find("PRODUCT(") if f.inst[n].startswith(f"PRODUCT('{child_name}'"))
            # The solid's faces come after its product's records in Open
            # CASCADE's output, before the next product's.
            later = sorted(n for n in f.find("PRODUCT(") if n > product)
            end = later[0] if later else max(f.inst) + 1
            face = next(n for n in f.find("ADVANCED_FACE") if product < n < end
                        and f.inst[f.refs(n)[-1]].startswith(pick))
            f.inst[face] = flip_sense(f.inst[face])
        return edit

    def refused36(kind, component):
        return {"result": "refused", "check": 36, "refusals": ["component_refused"], "kind": kind,
                "component": component}

    a.case("r23_component_refused", s02, "s02 with one cylinder face of the screw flipped (a text edit)",
           ["m3.4.1#36"], False, {"from": "s02"},
           asm_expect(s02, gate=refused36("face_orientation", "SCREW")),
           edit=flip_face("SCREW", "CYLINDRICAL_SURFACE"))
    a.case("r27_target_refused", s01, "s01 with one plane face of the bracket flipped (a text edit)",
           ["m3.4.1#36"], False, {"from": "s01"},
           asm_expect(s01, gate=refused36("face_orientation", "BRACKET")), edit=flip_face("BRACKET", "PLANE"))

    def other_frame(f: Part21):
        """The screw's usage places a second shape representation of the
        screw, which holds only a frame: its solid is not listed from it."""
        usage = usage_of(f, "SCREW")
        cdsr, relationship, idt = placement_of(f, usage)
        _, item, rep = component_item(f, usage)
        context = f.refs(rep)[-1]
        origin = f.add("CARTESIAN_POINT('',(0.,0.,0.))")
        frame = f.add(f"AXIS2_PLACEMENT_3D('',#{origin},$,$)")
        empty = f.add(f"SHAPE_REPRESENTATION('',(#{frame}),#{context})")
        child = f.refs(usage)[1]
        pds = next(n for n in f.find("PRODUCT_DEFINITION_SHAPE") if f.refs(n) == [child])
        f.add(f"SHAPE_DEFINITION_REPRESENTATION(#{pds},#{empty})")
        f.inst[relationship] = re.sub(rf"#{rep}\b", f"#{empty}", f.inst[relationship], count=1)
        f.inst[idt] = re.sub(rf"#{item}\b", f"#{frame}", f.inst[idt], count=1)

    a.case("r29_solid_frame", s02, "s02 with the screw placed through a second, empty shape representation",
           ["m3.4.1#37", "P3"], False, {"from": "s02"},
           asm_expect(s02, gate={"result": "refused", "check": 37, "refusals": ["solid_frame_unplaced"]}),
           edit=other_frame)

    s24 = Node("s24_coincident").add(Leaf("PLATE", a_plate())).add(bracket)
    for x, y in PTS:
        s24.add(bolt, m_tr((x, y, 8)))
    s24.add(bolt, m_tr((PTS[0][0], PTS[0][1], 8)))
    a.case("s24_coincident", s24, "a plate and bracket with a bolt placed twice at one place", [], False,
           {"from": "s01, without nuts, one bolt twice"}, asm_expect(s24, notes=["coincident_occurrences"]))

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


# ------------------------------------------------------------ match cases (M4)
# docs/research/m4-match.md, section 15: candidate parts against the sockets of the assemblies above. Each is built
# by a parametric builder whose defaults rebuild the source's leaf exactly (checked below), written in a frame, with
# the placement from the candidate's frame into the target's (the inverse of the frame motion, unrounded, KC2), its
# physical truth (KC3) and, per policy, the verdict and the failures expected: requirements named by what they
# constrain in the target's frame (KC5), never by id. A bad case's `must_fail` names its defect's requirement; an
# entry may be {"any_of": [...]} where which of several requirements catches it is verification's to decide (KH2).

M_TOL = 0.01  # mm, D9's default linear tolerance
M_ATOL = 0.01  # deg, D9's default angular tolerance


def c_bracket(lx=80.0, ly=50.0, t=8.0, holes=None, centre=8.0, extra=(), pocket=None, lug=False, side_hole=False):
    """s01's BRACKET with its interfaces as parameters; the defaults are `a_bracket`. `holes` maps each of PTS
    to (x, y, d, kind, depth): kind through, blind_top (from z = t down `depth`) or missing."""
    body = box(-lx / 2, -ly / 2, 0, lx, ly, t)
    if lug:
        # A tab off the -y side reaching 5 mm down past the bottom: inside the plate below, in no seat region.
        body = fuse(body, box(-10, -ly / 2 - 3, -5, 20, 3, t + 5))
    holes = holes or {p: (p[0], p[1], 6.6, "through", None) for p in PTS}
    tools = []
    for _, (x, y, d, kind, depth) in sorted(holes.items()):
        if kind == "through":
            tools.append(cyl((x, y, -1), UP, d / 2, t + 2))
        elif kind == "blind_top":
            tools.append(cyl((x, y, t - depth), UP, d / 2, depth + 1))
    if centre:
        tools.append(cyl((0, 0, -1), UP, centre / 2, t + 2))
    for x, y, d in extra:
        tools.append(cyl((x, y, -1), UP, d / 2, t + 2))
    if pocket:
        px, py, depth = pocket
        tools.append(box(-px / 2, -py / 2, t - depth, px, py, depth + 1))
    if side_hole:
        # A dia 3 hole along y at x = 10, z = 4: through both sides, no seat face.
        tools.append(cyl((10, -ly / 2 - 1, 4), (0, 1, 0), 1.5, ly + 2))
    return cut(body, *tools)


def c_plate03(dia=None, shift=None):
    """s03's PLATE: 80 x 40 x 10, holes at x -30, -10, 10, 30 of 5.97, 6.2, 6 and 6.015; `dia` and `shift`
    (in x) by hole x."""
    holes = {-30: 5.97, -10: 6.2, 10: 6.0, 30: 6.015}
    holes.update(dia or {})
    shape = box(-40, -20, 0, 80, 40, 10)
    for x, d in holes.items():
        shape = through(shape, [(x + (shift or {}).get(x, 0.0), 0)], d, 0, 10)
    return shape


def c_nut(d=5.0, kind="orig"):
    """s01's NUT, dia `d`; `kind` the review's: ext (5 mm taller below, the hole continued as dia 3), long (the
    dia 5 hole straight through the taller nut), bar (long, with a dia 1 bar across it at z -6)."""
    if kind == "orig":
        return through(hex_prism(10, -5, 5), [(0, 0)], d, -5, 0)
    if kind == "ext":
        return cut(hex_prism(10, -10, 10), cyl((0, 0, -5.0), UP, d / 2, 6.0), cyl((0, 0, -11.0), UP, 1.5, 7.0))
    if kind == "lip":
        # PR 3's review: the wall ends at the original's rim, then an annulus out to r 4.5 and a cone back in to
        # r 1.5 a millimetre below: an opening VR8 does not take.
        body = cut(fuse(hex_prism(10, -5, 5), cyl((0, 0, -6), UP, 4.5, 1.0)), cyl((0, 0, -5), UP, d / 2, 6.0))
        return cut(body, cone((0, 0, -6.001), UP, 1.5 - 0.0025, 4.0, 1.001))
    if kind == "ext_lip":
        # The same lip past an extension: dia 6.6 down to z -6, then the lip to z -7.
        body = cut(fuse(hex_prism(10, -5, 5), cyl((0, 0, -7), UP, 4.5, 2.0)), cyl((0, 0, -5), UP, d / 2, 6.0),
                   cyl((0, 0, -6), UP, 3.3, 1.0))
        return cut(body, cone((0, 0, -7.001), UP, 1.5 - 0.0025, 4.0, 1.001))
    body = cut(hex_prism(10, -10, 10), cyl((0, 0, -11.0), UP, d / 2, 12.0))
    if kind == "long":
        return body
    return fuse(body, cyl((-4.0, 0, -6.0), (1, 0, 0), 0.5, 8.0))


def c_block02(cb_d=11.0, cb_depth=6.5, d=6.6, h=15.0):
    """s02's BLOCK: 40 x 40 x h, dia d through, a dia cb_d counterbore cb_depth deep from the top."""
    tools = [cyl((0, 0, -1), UP, d / 2, h + 2)]
    if cb_d:
        tools.append(cyl((0, 0, h - cb_depth), UP, cb_d / 2, cb_depth + 1))
    return cut(box(-20, -20, 0, 40, 40, h), *tools)


def c_plate18(angle=90.0, d0=12.4, counterbore=False):
    """s18's PLATE: 40 x 40 x 10, dia 6.6 through, countersunk `angle` to dia d0 at the top; or a dia d0
    counterbore 3.2 deep in its place."""
    if counterbore:
        return cut(box(-20, -20, 0, 40, 40, 10), cyl((0, 0, -1), UP, 3.3, 12), cyl((0, 0, 6.8), UP, d0 / 2, 4))
    return cut(box(-20, -20, 0, 40, 40, 10), tool(Hole((0, 0, 10), DOWN, [(6.6, 10)], through=10,
                                                           countersink=(d0, angle))))


def c_base02(post=False):
    """s02's BASE: 60 x 60 x 10, top at z 0, a dia 5 blind hole 9 deep; `post`: a dia 2 post from its floor up
    to z -3 (the review's RS1)."""
    body = cut(box(-30, -30, -10, 60, 60, 10), cyl((0, 0, -9), UP, 2.5, 10))
    return fuse(body, cyl((0, 0, -9), UP, 1.0, 6.0)) if post else body


def c_housing(kind="orig"):
    """s19's HOUSING: 40 x 40 x 20, top at z 0, a dia 12.1 bore 8 deep; `kind` one of the review's (RS1 to RS5):
    post, spike, dome, bar, cross, lip, split."""
    body = box(-20, -20, -20, 40, 40, 20)
    bore = cyl((0, 0, -8), UP, 6.05, 9)
    if kind == "orig":
        return cut(body, bore)
    if kind == "post":
        return fuse(cut(body, bore), cyl((0, 0, -8), UP, 2.0, 7.0))
    if kind == "spike":
        return fuse(cut(body, bore), cone((0, 0, -8), UP, 3.0, 0.0, 6.0))
    if kind == "dome":
        from OCP.BRepPrimAPI import BRepPrimAPI_MakeSphere
        radius = 52.6025 / 8.0  # (8R - 16 = 6.05^2): meets the wall at z -7, top at z -3
        cap = cut(cyl((0, 0, -9), UP, 6.05, 10), BRepPrimAPI_MakeSphere(gp_Pnt(0, 0, -3.0 - radius), radius).Shape())
        return cut(body, cap)
    if kind == "bar":
        return fuse(cut(body, bore), cyl((-20, 0, -3), (1, 0, 0), 1.0, 40))
    if kind == "cross":
        return cut(body, bore, cyl((-21, 0, -3), (1, 0, 0), 1.0, 42))
    if kind == "lip":
        return cut(body, cyl((0, 0, -8), UP, 6.05, 7.5), cone((0, 0, -1), UP, 8.0, 5.0, 1.0),
                   cyl((0, 0, -0.01), UP, 5.0, 1.0))
    raise ValueError(kind)


def c_tube50(r_out=20.0):
    """s50's TUBE: dia 2 r_out outside, dia 16 bore, z 0 to 30."""
    return cut(cyl((0, 0, 0), UP, r_out, 30), cyl((0, 0, -1), UP, 8, 32))


def same_solid(a, b):
    """True if two shapes enclose the same volume: both boolean differences empty (to 1e-9 mm³)."""
    from OCP.BRepGProp import BRepGProp
    from OCP.GProp import GProp_GProps

    def volume(shape):
        props = GProp_GProps()
        BRepGProp.VolumeProperties_s(shape, props)
        return abs(props.Mass())
    return volume(cut(a, b)) < 1e-9 and volume(cut(b, a)) < 1e-9


def l_bore(point, direction=(0, 0, 1)):
    return {"kind": "bore", "axis_point": r3(point), "axis_direction": r3(direction)}


def l_shaft(point, direction=(0, 0, 1)):
    return {"kind": "shaft", "axis_point": r3(point), "axis_direction": r3(direction)}


def l_seat(normal, offset):
    return {"kind": "seat", "normal": r3(normal), "offset": r(offset)}


def l_grip(length):
    return {"kind": "grip", "length": r(length)}


def l_cone(apex):
    return {"kind": "conical_seat", "apex": r3(apex)}


def must(locator, *codes):
    """A requirement expected to fail with any of `codes` (KH2)."""
    return {**locator, "codes": sorted(codes)}


def any_of(*entries):
    """At least one of `entries` fails (KH2.3)."""
    return {"any_of": list(entries)}


def m_ok(flags=()):
    return {"verdict": "interface_match", "must_fail": [], "may_fail": [], **({"flags": list(flags)} if flags else {})}


def m_rej(must_fail=(), may_fail=()):
    return {"verdict": "reject", "must_fail": list(must_fail), "may_fail": list(may_fail)}


# KC15: the committed candidates, against the committed s01, s02 and s03, in MAPPED.
CANDIDATE_SET = {"m_bracket", "m_bracket_missing_hole", "m_bracket_dia_small", "m_bracket_thick", "m_bracket_blind",
                 "m_bracket_lug", "m_bracket_top_pocket", "m_plate03", "m_plate03_pin_shift_tol",
                 "m_plate03_press_15tol", "m_block02", "m_block02_cb_deep", "m_nut_pinned_15tol"}


# Candidates whose shape, unmoved, is not where the original is: the motion that takes it there, before the frame's
# inverse, so the stated placement is the construction's (KO1).
PLACED = {
    # The mirror image of s15's block, a half turn about y and 8 up from it.
    "m_mirror15": m_rot((0, 1, 0), 180.0, (0, 0, 8)),
}


def m_defects():
    """KH8 for bad cases: the requirement each defect breaks, which must fail at the construction's own
    placement, where `must_fail` may only say that one of several does (alignment may place the part elsewhere).
    Cases whose `must_fail` names one requirement need no entry: it is their defect."""
    hole, cs, cb = l_bore((30, 15, 0)), l_cone((0, 0, 3.8)), l_bore((0, 0, 0))
    cone_codes = ("no_feature", "cone_mismatch:angle", "cone_mismatch:apex", "cone_mismatch:range")
    return {
        "m_bracket_missing_hole": [must(hole, "no_feature", "axis_offset")],
        "m_bracket_dia_small": [must(hole, "diameter_out_of_limits:min")],
        "m_bracket_blind": [must(hole, "end_not_open", "wall_missing")],
        "m_plate03_pin_shift_tol": [must(l_bore((10, 0, 0)), "axis_offset", "no_feature")],
        "m_housing_lip": [must(l_bore((0, 0, -8)), "core_intruded", "wall_missing")],
        "m_plate18_82": [must(cs, *cone_codes)],
        "m_plate18_small_rim": [must(cs, *cone_codes)],
        "m_plate18_cbore": [must(cs, *cone_codes)],
        "m_bracket_spot": [must(l_seat((0, 0, 1), 8), "contact_not_covered")],
        "m_bracket_centre_10": [must(l_seat((0, 0, -1), 0), "contact_not_covered")],
        "m_bracket_shift_05": [must(hole, "no_feature", "axis_offset")],
        "m_plate16_635": [must(l_bore((0, 0, 0)), "diameter_out_of_limits:min", "core_intruded")],
        "m_bracket_decoy": [must(hole, "no_feature", "axis_offset")],
        **{f"m_bracket_shift_tol_{f}": [must(hole, "no_feature", "axis_offset")] for f in ("id", "var", "tilt", "skew")},
        **{f"m_bracket_dia_clear_{f}": [must(hole, "diameter_out_of_limits:min")] for f in ("id", "var", "tilt", "skew")},
        "m_plate01_blind": [must(l_bore((30, 15, -10)), "end_not_open", "wall_missing")],
        "m_bolt_head_moved": [must(l_seat((0, 0, -1), 0), "no_feature", "contact_not_covered")],
    }


def match_cases():
    """(id, (source case, target), shape, motion, truth, class, expect, construction, rules): the slice of the
    known-answer corpus D designed (section 15.5) and the review's candidate-side shapes (RS1 to RS6, RS13, RS14).
    `expect` is the default policy's, the functional one's equal unless stated as {"default": ..,
    "functional": ..}."""
    B, K, D3, N, C = (("s01_bolted", "BRACKET"), ("s02_cbore", "BLOCK"), ("s03_dowels", "PLATE"),
                      ("s01_bolted", "NUT"), ("s18_csink", "PLATE"))
    H, BA, T = ("s19_boss", "HOUSING"), ("s02_cbore", "BASE"), ("s50_cap", "TUBE")
    top, bot, g8 = l_seat((0, 0, 1), 8), l_seat((0, 0, -1), 0), l_grip(8)
    bores = [l_bore((x, y, 0)) for x, y in PTS]
    br = {p: (p[0], p[1], 6.6, "through", None) for p in PTS}

    def one(**change):
        holes = dict(br)
        x, y, d, kind, depth = holes[(30, 15)]
        v = {"x": x, "y": y, "d": d, "kind": kind, "depth": depth, **change}
        holes[(30, 15)] = (v["x"], v["y"], v["d"], v["kind"], v["depth"])
        return holes

    near_hole = [must(top, "contact_not_covered"), must(bot, "contact_not_covered")]

    def at_a_hole(*codes):
        """The bracket is symmetric under a half turn about each axis: the defect's hole may be placed at any of
        the four bores."""
        return any_of(*[must(b, *codes) for b in bores])

    def may_holes(*codes):
        return [must(b, *codes) for b in bores]
    out = [
        # The committed set (KC15), in MAPPED.
        ("m_bracket", B, c_bracket(), MAPPED, "fits", "g_frame", m_ok(), "the original", ["KC8", "KC15"]),
        ("m_bracket_missing_hole", B, c_bracket(holes=one(kind="missing")), MAPPED, "does_not_fit",
         "b_missing_hole", m_rej([at_a_hole("no_feature")], near_hole + may_holes("no_feature")),
         "no hole at (30, 15)", ["KC9", "VR7"]),
        ("m_bracket_dia_small", B, c_bracket(holes=one(d=5.9)), MAPPED, "does_not_fit", "b_dia_small",
         m_rej([at_a_hole("diameter_out_of_limits:min")],
               near_hole + may_holes("diameter_out_of_limits:min", "core_intruded")),
         "dia 5.9 under the bolt's dia 6 shank", ["KC9", "VR4", "VR22"]),
        ("m_bracket_thick", B, c_bracket(t=8.5), MAPPED, "fn_rule", "b_thickness",
         m_rej([must(g8, "grip_length", "seat_missing")],
               [must(top, "no_feature", "contact_not_covered"), must(bot, "no_feature", "contact_not_covered")]
               + [must(b, "wall_missing", "end_not_open") for b in bores]),
         "8.5 thick: the bolts still reach, D7 is hard (M4-8)", ["KC10", "VR12"]),
        ("m_bracket_blind", B, c_bracket(holes=one(kind="blind_top", depth=6)), MAPPED, "does_not_fit", "b_blind",
         m_rej([at_a_hole("end_not_open", "wall_missing")],
               near_hole + may_holes("end_not_open", "wall_missing", "core_intruded")),
         "hole (30, 15) blind 6 deep from the top: its bottom must be open", ["KC9", "VR7"]),
        ("m_bracket_lug", B, c_bracket(lug=True), MAPPED, "collides", "b_collide", m_ok(),
         "a tab 5 mm into the plate, in no requirement's region: InterfaceMatch until M7", ["KC3"]),
        ("m_bracket_top_pocket", B, c_bracket(pocket=(20, 12, 2)), MAPPED, "fn_rule", "fn_r14",
         m_rej([must(top, "contact_not_covered")], [must(g8, "seat_missing")]),
         "a 20 x 12 x 2 pocket round the centre hole, clear of the "
         "heads: R14's target-face basis rejects it", ["KC10", "VR11"]),
        ("m_plate03", D3, c_plate03(), MAPPED, "fits", "g_frame", m_ok(["thread_not_confirmed"]), "the original", ["KC8"]),
        ("m_plate03_pin_shift_tol", D3, c_plate03(shift={10: M_TOL}), MAPPED, "edge", "b_hole_shift",
         m_rej([any_of(must(l_bore((10, 0, 0)), "axis_offset", "no_feature"),
                       must(l_seat((0, 0, -1), 0), "contact_not_covered"))],
               [must(l_seat((0, 0, -1), 0), "contact_not_covered")]),
         "the line-to-line dia 6 hole moved exactly the tolerance", ["KC9", "VR3"]),
        ("m_plate03_press_15tol", D3, c_plate03(dia={-30: 5.97 + 1.5 * M_TOL}), MAPPED, "does_not_fit",
         "b_dia_pinned", m_rej([must(l_bore((-30, 0, 0)), "diameter_out_of_limits:max")],
                               [must(l_seat((0, 0, -1), 0), "contact_not_covered")]),
         "the press fit's dia 5.97 opened 1.5 tol", ["KC9", "VR7"]),
        ("m_block02", K, c_block02(), MAPPED, "fits", "g_frame", m_ok(), "the original", ["KC8"]),
        ("m_block02_cb_deep", K, c_block02(cb_depth=7.0), MAPPED, "does_not_fit", "b_seat_moved",
         m_rej([must(l_seat((0, 0, 1), 8.5), "no_feature", "contact_not_covered"),
                must(l_grip(8.5), "grip_length", "seat_missing")],
               [must(l_bore((0, 0, 0)), "wall_missing", "core_intruded", "diameter_out_of_limits:min",
                     "diameter_out_of_limits:max"), must(l_seat((0, 0, -1), 0), "no_feature")]),
         "the counterbore 7 deep: the head's seat 0.5 lower", ["KC9", "VR10", "VR12"]),
        ("m_nut_pinned_15tol", N, c_nut(5 + 1.5 * M_TOL), MAPPED, "does_not_fit", "b_dia_pinned",
         m_rej([must(l_bore((0, 0, -5)), "diameter_out_of_limits:max")], [must(l_seat((0, 0, 1), 0), "contact_not_covered")]),
         "the nut's pinned dia 5 opened 1.5 tol", ["KC9", "VR7"]),
    ]
    # The review's candidate-side shapes (section 28), against the neighbour their passing requirement cited.
    hb, hs = l_bore((0, 0, -8)), l_seat((0, 0, 1), 0)
    core = lambda b: must(b, "core_intruded", "wall_missing", "diameter_out_of_limits:min",  # noqa: E731
                          "diameter_out_of_limits:max", "wall_opening", "candidate_feature_unknown")
    out += [
        ("m_housing", H, c_housing(), VARIANT, "fits", "g_frame", m_ok(), "the original", ["KC8"]),
        ("m_housing_post", H, c_housing("post"), VARIANT, "does_not_fit", "b_post", m_rej([core(hb)]),
         "a dia 4 post from the floor to z -1", ["KC9", "VR8", "VR22"]),
        ("m_housing_spike", H, c_housing("spike"), VARIANT, "does_not_fit", "b_unknown_end", m_rej([core(hb)]),
         "a cone from the floor to an apex at z -2", ["KC9", "VR22", "CF5"]),
        ("m_housing_dome", H, c_housing("dome"), VARIANT, "does_not_fit", "b_unknown_end", m_rej([core(hb)]),
         "a spherical floor rising to z -3", ["KC9", "VR22", "CF4"]),
        ("m_housing_bar", H, c_housing("bar"), VARIANT, "does_not_fit", "b_wall_opening", m_rej([core(hb)]),
         "a dia 2 bar across the bore at z -3", ["KC9", "CF8", "VR22"]),
        ("m_housing_cross", H, c_housing("cross"), VARIANT, "fn_rule", "g_cross_hole", m_rej([core(hb)]),
         "a dia 2 cross hole at z -3: air, but M2 states no wall opening's faces, so CF8 rejects it (3a)",
         ["KC10", "CF8"]),
        ("m_housing_lip", H, c_housing("lip"), VARIANT, "does_not_fit", "b_end_zone",
         m_rej([any_of(core(hb), must(hs, "contact_not_covered", "no_feature"))],
               [core(hb), must(hs, "contact_not_covered", "no_feature")]),
         "the mouth: an annulus out to r 8, a cone back in to r 5", ["KC9", "VR8"]),
        ("m_nut_ext", N, c_nut(kind="ext"), VARIANT, "does_not_fit", "b_open_past_rim",
         m_rej([must(l_bore((0, 0, -5)), "end_not_open", "diameter_out_of_limits:min", "core_intruded")]),
         "5 mm taller below, the hole narrowing to dia 3 past the original's rim", ["KC9", "VR7"]),
        ("m_nut_long", N, c_nut(kind="long"), VARIANT, "does_not_fit", "b_open_past_rim",
         m_rej([must(l_bore((0, 0, -5)), "end_not_open", "diameter_out_of_limits:min", "core_intruded")]),
         "the pinned dia 5 continued 5 mm past the original's rim", ["KC9", "VR7"]),
        ("m_nut_bar", N, c_nut(kind="bar"), VARIANT, "does_not_fit", "b_open_past_rim",
         m_rej([must(l_bore((0, 0, -5)), "end_not_open", "wall_opening", "core_intruded",
                     "diameter_out_of_limits:min")]), "nut_long with a dia 1 bar across the extension",
         ["KC9", "CF8", "VR7"]),
        ("m_base02", BA, c_base02(), VARIANT, "fits", "g_frame", m_ok(["thread_not_confirmed"]), "the original", ["KC8"]),
        ("m_base02_post", BA, c_base02(post=True), VARIANT, "does_not_fit", "b_post",
         m_rej([core(l_bore((0, 0, -9)))]), "a dia 2 post in the pinned span", ["KC9", "VR8", "VR22"]),
        ("m_tube50", T, c_tube50(), VARIANT, "fits", "g_frame", m_ok(["thread_not_confirmed"]), "the original", ["KC8"]),
    ]
    # The countersunk plate (s18): a 90 deg conical seat, apex at z 3.8, radii 3.3 to 6.2; the countersink a
    # zone of the bore whose profile the socket states (X8).
    C = ("s18_csink", "PLATE")
    cs, cg, cb, cbot = l_cone((0, 0, 3.8)), l_grip(3.8), l_bore((0, 0, 0)), l_seat((0, 0, -1), 0)
    cone_codes = ("no_feature", "cone_mismatch:angle", "cone_mismatch:apex", "cone_mismatch:range",
                  "cone_mismatch:sense", "axis_angle")
    may18 = [must(cs, *cone_codes), must(cg, "grip_length", "seat_missing"),
             must(cb, "diameter_out_of_limits:profile", "diameter_out_of_limits:max", "core_intruded",
                  "wall_missing", "end_not_open", "no_feature"), must(cbot, "contact_not_covered", "no_feature")]
    out += [
        ("m_plate18", C, c_plate18(), VARIANT, "fits", "g_frame", m_ok(), "the original", ["KC8", "VR13"]),
        ("m_plate18_82", C, c_plate18(angle=82.0), VARIANT, "does_not_fit", "b_cone_angle",
         m_rej([any_of(must(cs, *cone_codes), must(cb, "diameter_out_of_limits:profile"))], may18),
         "82 deg, the same rim dia", ["KC9", "VR13", "VR7"]),
        ("m_plate18_angle_15atol", C, c_plate18(angle=90.0 + 1.5 * M_ATOL), VARIANT, "does_not_fit",
         "b_cone_angle", m_rej([must(cs, "cone_mismatch:angle")], may18), "90 deg + 1.5 angular tol",
         ["KC9", "VR13"]),
        ("m_plate18_small_rim", C, c_plate18(d0=11.0), VARIANT, "does_not_fit", "b_cone_range",
         m_rej([any_of(must(cs, *cone_codes), must(cb, "diameter_out_of_limits:profile"))], may18),
         "rim dia 11: the cone stops short of the head's 12.4", ["KC9", "VR13", "VR7"]),
        ("m_plate18_cbore", C, c_plate18(counterbore=True), VARIANT, "does_not_fit", "b_cone_missing",
         m_rej([any_of(must(cs, *cone_codes), must(cb, "diameter_out_of_limits:profile"))], may18),
         "a dia 12.4 counterbore in place of the countersink", ["KC9", "VR13", "VR7"]),
    ]
    # PR 3's review: candidates the rules as first built passed, each against the neighbour it collides with.
    nb = l_bore((0, 0, -5))
    out += [
        ("m_nut_lip", N, c_nut(kind="lip"), VARIANT, "does_not_fit", "b_end_zone",
         m_rej([must(nb, "end_not_open", "core_intruded", "wall_missing")],
               [must(l_seat((0, 0, 1), 0), "contact_not_covered", "no_feature")]),
         "an annulus out and a cone back in to r 1.5 below the rim: the bolt's thread runs there", ["KC9", "VR7", "VR8"]),
        ("m_nut_ext_lip", N, c_nut(kind="ext_lip"), VARIANT, "does_not_fit", "b_open_past_rim",
         m_rej([must(nb, "end_not_open", "core_intruded", "wall_missing", "diameter_out_of_limits:min")],
               [must(l_seat((0, 0, 1), 0), "contact_not_covered", "no_feature")]),
         "the lip past a dia 6.6 extension", ["KC9", "VR7", "VR8"]),
        ("m_plate18_finger", C, fuse(c_plate18(), box(7, -1, 9.999, 2, 2, 1.001), box(4.5, -1, 10.2, 4.5, 2, 0.8),
                                     box(4.5, -0.5, 9.3, 0.5, 1, 1.2)), VARIANT, "does_not_fit", "b_post",
         m_rej([must(cb, "core_intruded", "diameter_out_of_limits:profile", "wall_missing", "end_not_open")],
               may18),
         "an arch above the top face with a finger into the countersink, inside the head's cone", ["KC9", "VR22"]),
        ("m_bracket_spot", B, cut(c_bracket(), *[cyl((x, y, 6), UP, 6.0, 3) for x, y in PTS]), VARIANT,
         "does_not_fit", "b_thickness",
         m_rej([any_of(must(top, "contact_not_covered", "no_feature"), must(g8, "seat_missing", "grip_length"))],
               [must(top, "contact_not_covered", "no_feature"), must(g8, "seat_missing", "grip_length"),
                must(bot, "contact_not_covered", "no_feature")]
               + may_holes("no_feature", "axis_offset", "diameter_out_of_limits:max", "diameter_out_of_limits:min",
                           "wall_missing", "end_not_open", "core_intruded")),
         "a dia 12 spot face 2 deep under each head: 6 clamped, not 8", ["KC9", "VR12"]),
        # A separate bar hung 20 um below the open end, across the bolt's path: material of the candidate past a
        # real opening is no requirement's (section 8.6, Openings): the body's, M7's.
        ("m_nut_bridge", N, fuse(c_nut(), box(-4.0, -0.5, -6.02, 8.0, 1.0, 1.0),
                                 box(-4.0, -0.5, -6.02, 1.0, 1.0, 3.02), box(3.0, -0.5, -6.02, 1.0, 1.0, 3.02)),
         VARIANT, "collides", "b_collide", m_ok(["thread_not_confirmed"]),
         "a bar across the axis 20 um below the bottom face, on legs from the flanks: InterfaceMatch until M7",
         ["KC3"]),
        ("m_block02_membrane", K, cut(box(-20, -20, 0, 40, 40, 15), cyl((0, 0, -1), UP, 1.0, 17),
                                      cyl((0, 0, -1), UP, 3.3, 9.491), cyl((0, 0, 8.5), UP, 5.5, 7.5)),
         VARIANT, "does_not_fit", "b_dia_small",
         m_rej([must(l_bore((0, 0, 0)), "diameter_out_of_limits:min", "core_intruded")],
               [must(l_seat((0, 0, 1), 8.5), "contact_not_covered", "no_feature"),
                must(l_seat((0, 0, -1), 0), "contact_not_covered", "no_feature"),
                must(l_grip(8.5), "seat_missing", "grip_length")]),
         "a 9 um ring of dia 2 between the bore and the counterbore", ["KC9", "VR4"]),
    ]
    out += m_families(B, N, K, D3, BA, H, T, (top, bot, g8), one, near_hole, at_a_hole, may_holes)
    # VR4's edge in four frames (KC7): the tube's outside at exactly c - 2 tau.
    for name, motion in [("id", IDENTITY), ("var", VARIANT), ("tilt", TILT), ("skew", Motion((3, -2, 5), 71.0, (12.5, 7.0, -40.0)))]:
        out.append((f"m_tube50_tau_edge_{name}", T, c_tube50(20.99), motion, "edge", "e_tau",
                    m_rej([must(l_shaft((0, 0, 0)), "diameter_out_of_limits:max")]),
                    "outside dia 41.98: a margin of exactly tau beyond the cap's clear dia 42", ["KC7", "VR4"]))
    return out


SKEW = Motion((3, -2, 5), 71.0, (12.5, 7.0, -40.0))


def c_bolt(length=25.0, d=6.0, head_at=0.0):
    """s01's BOLT: hex head (across flats 10) from z `head_at` to 4, its bearing face at `head_at`; the shank,
    dia d, from z -length to it."""
    return fuse(hex_prism(10, head_at, 4 - head_at), cyl((0, 0, -length), UP, d / 2, length + head_at))


def c_screw(head_d=10.0, shank_d=6.0):
    """s02's SCREW: head dia `head_d`, z 0 to 6; shank dia `shank_d` down to -16."""
    return fuse(cyl((0, 0, 0), UP, head_d / 2, 6), cyl((0, 0, -16), UP, shank_d / 2, 16))


def c_bearing(outer=22.0, inner=8.0):
    """s04's BEARING: dia `outer` / `inner` x 7."""
    return cut(cyl((0, 0, 0), UP, outer / 2, 7), cyl((0, 0, -1), UP, inner / 2, 9))


def c_washer(h=1.6):
    """s05's WASHER: dia 12 / 6.4, `h` thick."""
    return cut(cyl((0, 0, 0), UP, 6, h), cyl((0, 0, -1), UP, 3.2, h + 2))


def c_cover(boss_d=12.0, boss_h=5.0):
    """s19's COVER: 40 x 40 x 5, a boss dia `boss_d` and `boss_h` long below it."""
    return fuse(box(-20, -20, 0, 40, 40, 5), cyl((0, 0, -boss_h), UP, boss_d / 2, boss_h))


def c_plate16(d0=6.6):
    """s16's PLATE, its offset pin's hole dia `d0`."""
    return through(through(box(-10, -20, 0, 40, 40, 8), [(0, 0)], d0, 0, 8), [(15, 0)], 7.0, 0, 8)


def c_housing04(seat=22.0):
    """s04's HOUSING: 40 x 40 x 20, a dia 18 bore through, the bearing's seat dia `seat` from z 13."""
    return cut(box(-20, -20, 0, 40, 40, 20), cyl((0, 0, -1), UP, 9, 22), cyl((0, 0, 13), UP, seat / 2, 8))


def c_block15():
    """s15's BLOCK, which its mirror image's half turn maps onto."""
    return cut(box(0, -10, 0, 20, 20, 8), cyl((12, 5, -1), UP, 3.3, 10))


# 15.4: each builder with its defaults against the leaf the assembly section built (full corpus only: the
# candidate set is written without the assemblies).
BUILT_FROM = [("c_bracket", c_bracket, ("s01_bolted", "BRACKET")), ("c_nut", c_nut, ("s01_bolted", "NUT")),
              ("c_bolt", c_bolt, ("s01_bolted", "BOLT")), ("c_block02", c_block02, ("s02_cbore", "BLOCK")),
              ("c_base02", c_base02, ("s02_cbore", "BASE")), ("c_screw", c_screw, ("s02_cbore", "SCREW")),
              ("c_plate03", c_plate03, ("s03_dowels", "PLATE")),
              ("c_bearing", c_bearing, ("s04_bearing", "BEARING")),
              ("c_housing04", c_housing04, ("s04_bearing", "HOUSING")),
              ("c_washer", c_washer, ("s05_stack", "WASHER")),
              ("c_block15", c_block15, ("s15_mirror_part", "BLOCK")),
              ("c_plate16", c_plate16, ("s16_offaxis", "PLATE")),
              ("c_plate18", c_plate18, ("s18_csink", "PLATE")), ("c_cover", c_cover, ("s19_boss", "COVER")),
              ("c_housing", c_housing, ("s19_boss", "HOUSING")), ("c_tube50", c_tube50, ("s50_cap", "TUBE"))]


def m_families(B, N, K, D3, BA, H, T, seats, one, near_hole, at_a_hole, may_holes):
    """The families of section 15.3 beyond the committed set and the review's shapes: each source part's good
    frames and its defects, one thing broken, just past the tolerance or grossly."""
    top, bot, g8 = seats
    tol, ok = M_TOL, m_ok
    rej = m_rej
    any_bore = ("no_feature", "axis_offset", "axis_angle", "diameter_out_of_limits:min", "diameter_out_of_limits:max",
                "wall_missing", "end_not_open", "core_intruded", "wall_opening", "candidate_feature_unknown")
    any_seat = ("no_feature", "contact_not_covered", "unknown_outline")
    bracket_may = [must(top, *any_seat), must(bot, *any_seat), must(g8, "grip_length", "seat_missing")] + \
        may_holes(*any_bore)
    out = [
        # The bracket's other classes (KC8 to KC10).
        ("m_bracket_tilt_inch", B, c_bracket(), TILT, "fits", "g_unit", ok(), "in TILT, written in inches",
         ["KC6", "KC8", "CF9"]),
        ("m_bracket_side_hole", B, c_bracket(side_hole=True), VARIANT, "fits", "g_extra_hole", ok(),
         "a dia 3 hole along y through the sides", ["KC8"]),
        ("m_bracket_larger", B, c_bracket(lx=90, ly=56), VARIANT, "fits", "g_body", ok(), "90 x 56, the same holes",
         ["KC8"]),
        ("m_bracket_centre_10", B, c_bracket(centre=10.0), VARIANT, "does_not_fit", "b_seat_region",
         rej([any_of(must(top, "contact_not_covered"), must(bot, "contact_not_covered"))], bracket_may),
         "the left-out dia 8 hole made dia 10: it takes about 28 mm2 of the plate's true contact (KC3)",
         ["KC9", "VR10", "VR11"]),
        ("m_bracket_centre_none", B, c_bracket(centre=0), VARIANT, "fits", "g_left_out", ok(),
         "the left-out dia 8 hole absent", ["KC8", "VR15"]),
        ("m_bracket_inside_tol", B, c_bracket(holes=one(x=30 + tol / 4)), VARIANT, "fits", "g_inside_tol", ok(),
         "hole (30, 15) moved a quarter of the tolerance", ["KC8"]),
        ("m_bracket_face_hole", B, c_bracket(extra=[(0, 15, 4)]), VARIANT, "does_not_fit", "b_seat_region",
         rej([must(bot, "contact_not_covered")], bracket_may), "a dia 4 hole in the plate's contact", ["KC9", "VR11"]),
        ("m_bracket_dia_large", B, c_bracket(holes=one(d=6.8)), VARIANT, "fits_functional", "b_dia_large",
         {"default": rej([at_a_hole("diameter_out_of_limits:max")], bracket_may),
          "functional": rej([any_of(must(top, "contact_not_covered"), must(bot, "contact_not_covered"))],
                            bracket_may)},
         "dia 6.8: strict rejects; functional too, for the ring its seats lose (M4-7)", ["KC9", "VR7"]),
        ("m_bracket_thin_15tol", B, c_bracket(t=8 - 1.5 * tol), VARIANT, "fn_rule", "b_thickness",
         rej([must(g8, "grip_length", "seat_missing")], bracket_may),
         "8 - 1.5 tol thick: the bolts still clamp it, a grip-only change (M4-8)", ["KC10", "VR12"]),
        ("m_bracket_shift_05", B, c_bracket(holes=one(x=30.5)), VARIANT, "does_not_fit", "b_hole_shift",
         rej([at_a_hole("no_feature", "axis_offset")], bracket_may), "hole (30, 15) moved 0.5 mm", ["KC9", "VR3"]),
        ("m_bracket_shift_15tol", B, c_bracket(holes=one(x=30 + 1.5 * tol)), VARIANT, "fn_rule", "fn_position",
         rej([], bracket_may), "moved 1.5 tol: the bolt still passes, the axis rejects", ["KC10", "VR3"]),
        ("m_bracket_decoy", B, c_bracket(holes=one(kind="missing"), extra=[(30, -5, 6.6)]), VARIANT, "does_not_fit",
         "b_decoy", rej([at_a_hole("no_feature", "axis_offset")], bracket_may),
         "the hole at (30, 15) missing, one at (30, -5) that only a wrong placement would use", ["KC9", "AL4"]),
    ]
    # Edge cases in four frames (KC7): the hole moved exactly the tolerance; the clear diameter plus it.
    for name, motion in [("id", IDENTITY), ("var", VARIANT), ("tilt", TILT), ("skew", SKEW)]:
        out.append((f"m_bracket_shift_tol_{name}", B, c_bracket(holes=one(x=30 + tol)), motion, "edge",
                    "b_hole_shift", rej([any_of(*at_a_hole("no_feature", "axis_offset")["any_of"],
                                                must(top, "contact_not_covered"), must(bot, "contact_not_covered"))],
                                        bracket_may),
                    "hole (30, 15) moved exactly the tolerance", ["KC7", "VR3"]))
        out.append((f"m_bracket_dia_clear_{name}", B, c_bracket(holes=one(d=6.0 + 2 * 0.01)), motion, "edge",
                    "e_tau", rej([at_a_hole("diameter_out_of_limits:min", "core_intruded")], bracket_may),
                    "dia 6.02: a margin of exactly tau beyond the bolt's clear dia 6", ["KC7", "VR4"]))
    # s01's plate: through bores open both ends.
    P = ("s01_bolted", "PLATE")
    p_top, p_bot, p_g = l_seat((0, 0, 1), 0), l_seat((0, 0, -1), 10), l_grip(10)
    p_bores = [l_bore((x, y, -10)) for x, y in PTS]
    plate_may = [must(p_top, *any_seat), must(p_bot, *any_seat), must(p_g, "grip_length", "seat_missing")] + \
        [must(b, *any_bore) for b in p_bores]
    blind = cut(box(-50, -30, -10, 100, 60, 10), *[cyl((x, y, -11), UP, 3.3, 12) for x, y in PTS if (x, y) != (30, 15)],
                cyl((30, 15, -5), UP, 3.3, 6))
    out += [
        ("m_plate01", P, a_plate(), VARIANT, "fits", "g_frame", ok(), "the original", ["KC8"]),
        ("m_plate01_blind", P, blind, VARIANT, "does_not_fit", "b_blind",
         rej([any_of(*[must(b, "end_not_open", "wall_missing", "core_intruded") for b in p_bores])], plate_may),
         "hole (30, 15) blind 5 deep from the top", ["KC9", "VR7"]),
        ("m_plate01_thin", P, a_plate(t=9.5), VARIANT, "fn_rule", "b_thickness",
         rej([must(p_g, "grip_length", "seat_missing")], plate_may),
         "9.5 thick: the bolts still clamp it, a grip-only change (M4-8)", ["KC10", "VR12"]),
    ]
    # s01's nut: a pinned interference, a thread not confirmed.
    nb, ns = l_bore((0, 0, -5)), l_seat((0, 0, 1), 0)
    out += [
        ("m_nut", N, c_nut(), VARIANT, "fits", "g_thread", ok(["thread_not_confirmed"]), "the original",
         ["KC8", "VR19"]),
        ("m_nut_clearance", N, c_nut(6.0), VARIANT, "does_not_fit", "b_thread",
         rej([must(nb, "diameter_out_of_limits:max")], [must(ns, *any_seat)]), "dia 6: a clearance nut, no thread",
         ["KC9", "VR7"]),
    ]
    for name, motion in [("id", IDENTITY), ("var", VARIANT), ("tilt", TILT), ("skew", SKEW)]:
        out.append((f"m_nut_pinned_tol_{name}", N, c_nut(5 + tol), motion, "edge", "b_dia_pinned",
                    rej([must(nb, "diameter_out_of_limits:max")], [must(ns, *any_seat)]),
                    "dia 5 + tol: the edge rejects", ["KC7", "VR3"]))
    # s01's bolt: clearance and pinned shaft spans, its head's seat.
    BO = ("s01_bolted", "BOLT")
    sh, hs = l_shaft((0, 0, 0), (0, 0, -1)), l_seat((0, 0, -1), 0)
    out += [
        ("m_bolt", BO, c_bolt(), VARIANT, "fits", "g_frame", ok(["thread_not_confirmed"]), "the original", ["KC8"]),
        ("m_bolt_thick", BO, c_bolt(d=6.7), VARIANT, "does_not_fit", "b_shaft",
         rej([must(sh, "diameter_out_of_limits:max", "diameter_out_of_limits:min")], [must(hs, *any_seat)]),
         "a dia 6.7 shank in the bracket's dia 6.6", ["KC9", "VR9"]),
        ("m_bolt_short", BO, c_bolt(length=20.0), VARIANT, "does_not_fit", "b_shaft",
         rej([must(sh, "wall_missing")], [must(hs, *any_seat)]), "20 long: short of the nut's thread", ["KC9", "VR9"]),
        ("m_bolt_longer", BO, c_bolt(length=25.5), VARIANT, "fits", "g_body", ok(["thread_not_confirmed"]),
         "a shank 0.5 longer: it seats alike, 0.5 more below the nut", ["KC8"]),
    ]
    # s02's base and screw.
    bb = l_bore((0, 0, -9))
    S = ("s02_cbore", "SCREW")
    ssh, sseat = l_shaft((0, 0, -16)), l_seat((0, 0, -1), 0)
    out += [
        ("m_base02_short", BA, cut(box(-30, -30, -10, 60, 60, 10), cyl((0, 0, -7), UP, 2.5, 8)), VARIANT,
         "does_not_fit", "b_depth", rej([must(bb, "wall_missing", "core_intruded", "no_feature")],
                                        [must(l_seat((0, 0, 1), 0), *any_seat)]),
         "the dia 5 hole 7 deep where the thread reaches 7.5", ["KC9", "VR7"]),
        ("m_screw02", S, c_screw(), VARIANT, "fits", "g_frame", ok(["thread_not_confirmed"]), "the original", ["KC8"]),
        ("m_screw02_head", S, c_screw(head_d=11.2), VARIANT, "does_not_fit", "b_shaft",
         rej([must(ssh, "diameter_out_of_limits:max")], [must(sseat, *any_seat)]),
         "a dia 11.2 head in the dia 11 counterbore", ["KC9", "VR9"]),
        ("m_screw02_shank", S, c_screw(shank_d=6.7), VARIANT, "does_not_fit", "b_shaft",
         rej([must(ssh, "diameter_out_of_limits:max", "diameter_out_of_limits:min")], [must(sseat, *any_seat)]),
         "a dia 6.7 shank in the block's dia 6.6", ["KC9", "VR9"]),
    ]
    # s04's bearing and housing: line to line, pinned both ways.
    BE, HO = ("s04_bearing", "BEARING"), ("s04_bearing", "HOUSING")
    be_out, be_in, be_seat = l_shaft((0, 0, 0)), l_bore((0, 0, 0)), l_seat((0, 0, -1), 0)
    ho_bore, ho_seat = l_bore((0, 0, 0)), l_seat((0, 0, 1), 13)
    housing = c_housing04
    out += [
        ("m_bearing04", BE, c_bearing(), VARIANT, "fits", "g_frame", ok(["thread_not_confirmed"]), "the original",
         ["KC8"]),
        ("m_bearing04_outer", BE, c_bearing(outer=22.0 + 1.5 * tol), VARIANT, "does_not_fit", "b_dia_pinned",
         rej([must(be_out, "diameter_out_of_limits:max")], [must(be_in, *any_bore), must(be_seat, *any_seat)]),
         "dia 22 + 1.5 tol outside, pinned line to line", ["KC9", "VR9"]),
        ("m_housing04", HO, housing(), VARIANT, "fits", "g_frame", ok(["thread_not_confirmed"]), "the original",
         ["KC8"]),
        ("m_housing04_seat", HO, housing(22.0 + 1.5 * tol), VARIANT, "does_not_fit", "b_dia_pinned",
         rej([must(ho_bore, "diameter_out_of_limits:max")], [must(ho_seat, *any_seat)]),
         "the bearing's seat dia 22 + 1.5 tol", ["KC9", "VR7"]),
    ]
    # s05's washer, the other copy its neighbour (self).
    W = ("s05_stack", "WASHER")
    wb, wg = l_bore((0, 0, 0)), l_grip(1.6)
    w_may = [must(wb, *any_bore), must(l_seat((0, 0, -1), 0), *any_seat), must(l_seat((0, 0, 1), 1.6), *any_seat),
             must(l_shaft((0, 0, 0)), *any_bore)]
    out += [
        ("m_washer05", W, c_washer(), VARIANT, "fits", "g_frame", ok(), "the original", ["KC8", "VR17"]),
        ("m_washer05_thick", W, c_washer(h=1.7), VARIANT, "fn_rule", "b_thickness",
         rej([must(wg, "grip_length", "seat_missing")], w_may),
         "1.7 thick: both copies' grips and the seat between; a grip-only change (M4-8)", ["KC10", "VR12", "VR17"]),
    ]
    # s15: the mirror image, which a rotation maps onto the block.
    M = ("s15_mirror_part", "BLOCK")
    block15 = c_block15()
    mirror = gp_Trsf()
    mirror.SetMirror(gp_Ax2(gp_Pnt(0, 0, 0), gp_Dir(1, 0, 0)))
    out.append(("m_mirror15", M, BRepBuilderAPI_Transform(block15, mirror, True).Shape(), VARIANT, "fits",
                "g_mirror", ok(), "the mirror image, a half turn of the block", ["KC8", "AL9"]))
    # s16's plate: the offset pin needs clear dia 6.4.
    O = ("s16_offaxis", "PLATE")
    out += [
        ("m_plate16", O, c_plate16(), VARIANT, "fits", "g_frame", ok(), "the original", ["KC8"]),
        ("m_plate16_635", O, c_plate16(6.35), VARIANT, "fits_functional", "b_offset_clear",
         {"default": rej([any_of(must(l_bore((0, 0, 0)), "diameter_out_of_limits:min", "core_intruded"),
                                 must(l_bore((15, 0, 0)), "diameter_out_of_limits:max"))],
                         [must(l_bore((0, 0, 0)), *any_bore), must(l_bore((15, 0, 0)), *any_bore)]),
          "functional": ok()},
         "dia 6.35: not the offset pin's clear 6.4; turned a half turn, the dia 7 takes it and the 6.35 the tilted"
         " pin, which functional allows", ["KC9", "VR4"]),
    ]
    # s19's cover: its boss in the housing's blind bore.
    C19 = ("s19_boss", "COVER")
    boss, cseat = l_shaft((0, 0, 0), (0, 0, -1)), l_seat((0, 0, -1), 0)
    out += [
        ("m_cover19", C19, c_cover(), VARIANT, "fits", "g_frame", ok(), "the original", ["KC8"]),
        ("m_cover19_long", C19, c_cover(boss_h=8.5), VARIANT, "collides", "b_collide", ok(),
         "the boss 3.5 longer, into the bore's floor: max_extra_mm is information (M4-11), M7's", ["KC3", "VR9"]),
        ("m_cover19_wide", C19, c_cover(boss_d=12.15), VARIANT, "does_not_fit", "b_shaft",
         rej([must(boss, "diameter_out_of_limits:max", "core_intruded")], [must(cseat, *any_seat)]),
         "a dia 12.15 boss in the dia 12.1 bore", ["KC9", "VR9"]),
    ]
    return out



def build_candidates(w: Writer, which):
    """Writes the match cases `which(unit_set)` selects; checks each builder rebuilds its source's leaf."""
    for name, builder, leaf in [("c_bracket", c_bracket(), a_bracket()), ("c_nut", c_nut(), a_hex_nut()),
                                ("c_block02", c_block02(), cut(box(-20, -20, 0, 40, 40, 15),
                                                                cyl((0, 0, -1), UP, 3.3, 17),
                                                                cyl((0, 0, 8.5), UP, 5.5, 7.5))),
                                ("c_plate18", c_plate18(), cut(box(-20, -20, 0, 40, 40, 10),
                                                                tool(Hole((0, 0, 10), DOWN, [(6.6, 10)], through=10,
                                                                          countersink=(12.4, 90))))),
                                ("c_base02", c_base02(), cut(box(-30, -30, -10, 60, 60, 10),
                                                              cyl((0, 0, -9), UP, 2.5, 10))),
                                ("c_housing", c_housing(), cut(box(-20, -20, -20, 40, 40, 20),
                                                                cyl((0, 0, -8), UP, 6.05, 9))),
                                ("c_tube50", c_tube50(), cut(cyl((0, 0, 0), UP, 20, 30), cyl((0, 0, -1), UP, 8, 32)))]:
        if not same_solid(builder, leaf):
            raise RuntimeError(f"{name}'s defaults do not rebuild its source's leaf")
    if LEAVES:
        for name, builder, source in BUILT_FROM:
            if source not in LEAVES:
                raise RuntimeError(f"{name}: the assembly section built no leaf {source}")
            if not same_solid(builder(), LEAVES[source]):
                raise RuntimeError(f"{name}'s defaults do not rebuild {source}'s leaf")
    for cid, (case, part), shape, motion, truth, klass, expect, construction, rules in match_cases():
        unit_set = cid in CANDIDATE_SET
        if not which(unit_set):
            continue
        moved = BRepBuilderAPI_Transform(shape, motion.trsf(), True).Shape()
        w.shape(moved, cid, unit="INCH" if cid.endswith("_inch") else "MM")
        expects = expect if "default" in expect else {"default": expect, "functional": "default"}
        default = expects["default"]
        defect = m_defects().get(cid, [m for m in default["must_fail"] if "any_of" not in m])
        if truth in ("does_not_fit", "edge") and not defect:
            raise RuntimeError(f"{cid}: a bad case names no defect")
        w.cases.append({
            "id": cid, "kind": "match", "file": f"{cid}.stp", "set": "unit" if unit_set else "corpus",
            "socket": {"case": case, "part": part, "occurrence": None, "review": []},
            "candidate": {"file": f"{cid}.stp"}, "class": klass, "truth": truth,
            "construction": {"from": part, "defect": construction},
            "frame": {"axis": list(motion.axis), "angle_deg": motion.angle, "shift": list(motion.shift)},
            # Unrounded (KC2): the inverse of the frame motion, candidate to target.
            "placement": [row[:4] for row in m_mul(PLACED.get(cid, m_tr((0, 0, 0))), m_inv(m_motion(motion)))[:3]],
            "expect": expects, "defect": defect, "rules": rules, "tests": construction,
        })



# VD5 (KH14): targets whose sockets state requirements no geometry can pass, each read in place as its own
# candidate. Every one of them fails, as M3's construction counts them, and nothing else does.
VD5_TARGETS = [("s11_unread_neighbour", "BRACKET"), ("s12_undecided_target", "ANGLED"),
               ("s13_unknown_outline", "BLOCK"), ("s13_unknown_outline", "PLATE"), ("s17_slot", "PLATE"),
               ("s47_rod_on_flat", "PLATE"), ("s48_dshaft", "DSHAFT"), ("s52_skew", "PLATE"),
               ("s52_skew", "PIN"), ("s58_csink_tilted", "PLATE")]


def build_component_cases(w: Writer):
    """The VD5 cases: a component candidate (KC2), its expectation the count of unverifiable requirements the
    source's expected sockets state, under both policies (the corpus states no floor)."""
    expected = {c["id"]: c["expect"].get("sockets") or {} for c in w.cases if c["kind"] == "assembly"}
    identity = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]]
    for case, part in VD5_TARGETS:
        n = sum(sock["unverifiable"] for sock in expected[case][part])
        if n == 0:
            raise RuntimeError(f"{case} {part}: no unverifiable requirement to show")
        cid = f"m_{case.split('_')[0]}_{part.lower()}_own"
        w.cases.append({
            "id": cid, "kind": "match", "file": f"{case}.stp", "set": "corpus",
            "socket": {"case": case, "part": part, "occurrence": None, "review": []},
            "candidate": {"component": {"case": case, "part": part, "occurrence": None}},
            "class": "b_unverifiable", "truth": "fn_rule",
            "construction": {"from": part, "defect": "none: the original, read in place"},
            "frame": {"axis": [0, 0, 1], "angle_deg": 0.0, "shift": [0, 0, 0]}, "placement": identity,
            "expect": {"default": {**m_rej(), "unverifiable": n}, "functional": "default"}, "defect": [],
            "rules": ["KC10", "VD5", "VR6", "VR14"],
            "tests": f"{n} requirements no candidate can pass: the original fails exactly them",
        })

def stated_uncertainty(text, mm, root=None):
    """`text` with each representation context's length uncertainty set to `mm`, or only the context of
    `root`'s own shape representation (the placement floor, as AS1-md states it)."""
    f = Part21(text)
    contexts = set()
    for sdr in f.find("SHAPE_DEFINITION_REPRESENTATION"):
        pds, rep = f.refs(sdr)[:2]
        pd = f.refs(pds)[0]
        product = f.refs(f.refs(pd)[0])[0]
        if root is None or f.inst[product].startswith(f"PRODUCT('{root}'"):
            contexts.add(f.refs(rep)[-1])
    changed = 0
    for context in contexts:
        for u in re.findall(r"GLOBAL_UNCERTAINTY_ASSIGNED_CONTEXT\(\(([^)]*)\)\)", f.inst[context]):
            for n in re.findall(r"#(\d+)", u):
                f.inst[int(n)] = re.sub(r"LENGTH_MEASURE\([^)]*\)", f"LENGTH_MEASURE({real(mm)})",
                                        f.inst[int(n)])
                changed += 1
    if changed == 0:
        raise RuntimeError("no uncertainty to state")
    return f.text()


def build_gate_cases(w: Writer):
    """KH14's cases for the floor and the gates (RM1): s61 (every part's uncertainty 0.02 mm, past the
    tolerance: VR2, VD6) and s62 (the root context's 0.01 mm, a placement floor that fails nothing alone: VR5),
    both s01 restated; each refused part of the corpus as a candidate (CF2, VD10); s01's socket stale, its
    digest broken, unreviewed under --require-reviewed, and r27's refused socket document (VD9, MF2)."""
    s01 = (w.out / "s01_bolted.stp").read_text(encoding="latin-1")
    w.text(stated_uncertainty(s01, 0.02), "s61_floor_parts", "s01_bolted")
    w.text(stated_uncertainty(s01, 0.01, root="s01_bolted"), "s62_floor_placement", "s01_bolted")
    # The bracket file's own frame and placement, which these cases share.
    bracket = next(c for c in w.cases if c["id"] == "m_bracket")
    identity, frame = bracket["placement"], bracket["frame"]

    def case(cid, socket, candidate, klass, truth, expect, construction, rules):
        w.cases.append({
            "id": cid, "kind": "match", "file": candidate, "set": "corpus",
            "socket": {"case": "s01_bolted", "part": "BRACKET", "occurrence": None, "review": [], **socket},
            "candidate": {"file": candidate}, "class": klass, "truth": truth,
            "construction": {"from": "BRACKET", "defect": construction}, "frame": frame, "placement": identity,
            "expect": {"default": expect, "functional": "default"}, "defect": [], "rules": rules,
            "tests": construction,
        })
    case("m_s61_bracket", {"case": "s61_floor_parts", "floor": {"parts_mm": 0.02}}, "m_bracket.stp",
         "fn_floor", "fn_rule", {**m_rej(), "unverifiable": "all"},
         "the bracket against s01 stated at 0.02 mm: no tolerance is left above the parts' own",
         ["KC10", "VR2", "VD6"])
    case("m_s62_bracket", {"case": "s62_floor_placement", "floor": {"placement_mm": 0.01}}, "m_bracket.stp",
         "g_floor", "fits", m_ok(), "the bracket against s01 placed to 0.01 mm: the floor alone fails nothing",
         ["KC8", "VR5"])
    refused = {c["id"]: c["expect"]["refusals"] for c in w.cases
               if c["kind"] == "part" and c["id"].startswith("r") and c["expect"].get("result") == "refused"}
    for rid, codes in sorted(refused.items()):
        case(f"m_refused_{rid}", {}, f"{rid}.stp", "b_refused", "refused",
             {**m_rej(), "refused": sorted(codes)}, f"{rid}, which M2's gate refuses", ["CF2", "VD10"])
    for edit, options in [("stale", {}), ("digest", {}), ("unreviewed", {"require_reviewed": True})]:
        case(f"m_s01_socket_{edit}", {"edit": edit}, "m_bracket.stp", "b_socket", "refused",
             {"verdict": "socket_refused", **({"options": options} if options else {})},
             f"the bracket against s01's socket {edit}: refused before any candidate", ["VD9", "MF2"])
    case("m_r27_socket", {"case": "r27_target_refused"}, "m_bracket.stp", "b_socket", "refused",
         {"verdict": "socket_refused"}, "the bracket against r27's refused socket document", ["VD9", "MF2"])

def c_block59():
    """s59's BLOCK: 30 x 25 x 15 from (10, 5, 0); at (18, 12) a dia 6.6 hole through with a dia 11 counterbore
    from z 8.5; at (33, 24) a dia 5 hole through. Neither the counterbore nor the side its stop seats is
    symmetric, so no proper rotation maps its mirror image onto it."""
    return cut(box(10, 5, 0, 30, 25, 15), cyl((18, 12, -1), UP, 3.3, 17), cyl((18, 12, 8.5), UP, 5.5, 7),
               cyl((33, 24, -1), UP, 2.5, 17))


def build_mirror_cases(w: Writer):
    """b_mirror (KC9, AL9's proper rotations): s59, a match-only source, the block on a plate, a screw's head on
    its counterbore's floor and its shank in the plate, a dia 5 pin through its off-centre hole, a stop against
    its x = 40 side; the block itself fits, its mirror image in y = 17.5 does not, in any frame."""
    plate = cut(box(0, 0, -10, 60, 40, 10), cyl((18, 12, -11), UP, 3, 12), cyl((33, 24, -11), UP, 2.5, 12))
    screw = fuse(cyl((0, 0, 8.5), UP, 5, 6), cyl((0, 0, -8), UP, 3, 16.5))
    root = Node("s59_mirror_block").add(Leaf("PLATE", plate)).add(Leaf("BLOCK", c_block59()))
    root.add(Leaf("SCREW", screw), m_tr((18, 12, 0))).add(Leaf("PIN", cyl((33, 24, -10), UP, 2.5, 25)))
    root.add(Leaf("STOP", box(40, 5, 0, 5, 25, 15)))
    write_xcaf(w, root, "s59_mirror_block")
    mirror = gp_Trsf()
    mirror.SetMirror(gp_Ax2(gp_Pnt(0, 17.5, 0), gp_Dir(0, 1, 0)))
    mirrored = BRepBuilderAPI_Transform(c_block59(), mirror, True).Shape()
    if same_solid(mirrored, c_block59()):
        raise RuntimeError("s59's block is its own mirror image")
    bores = [l_bore((18, 12, 0)), l_bore((33, 24, 0))]
    seats = [l_seat((0, 0, -1), 0), l_seat((1, 0, 0), 40), l_seat((0, 0, 1), 8.5)]
    every = ("no_feature", "axis_offset", "axis_angle", "diameter_out_of_limits:min", "diameter_out_of_limits:max",
             "wall_missing", "end_not_open", "core_intruded", "contact_not_covered", "seat_missing", "grip_length",
             "no_placement", "alignment_exhausted")
    may = [must(x, *every) for x in bores + seats] + [must(l_grip(8.5), *every)]
    for cid, shape, motion, truth, klass, expect, defect, text in [
        ("m_s59_block", c_block59(), VARIANT, "fits", "g_frame", m_ok(["thread_not_confirmed"]), [],
         "the block itself"),
        ("m_s59_mirror", mirrored, VARIANT, "does_not_fit", "b_mirror", m_rej([any_of(*may)], may),
         [must(l_bore((33, 24, 0)), "no_feature", "axis_offset", "diameter_out_of_limits:min", "wall_missing")],
         "the block mirrored in y = 17.5: no proper rotation maps it onto the block"),
        ("m_s59_mirror_tilt", mirrored, TILT, "does_not_fit", "b_mirror", m_rej([any_of(*may)], may),
         [must(l_bore((33, 24, 0)), "no_feature", "axis_offset", "diameter_out_of_limits:min", "wall_missing")],
         "the mirror image, in TILT"),
    ]:
        w.shape(BRepBuilderAPI_Transform(shape, motion.trsf(), True).Shape(), cid)
        w.cases.append({
            "id": cid, "kind": "match", "file": f"{cid}.stp", "set": "corpus",
            "socket": {"case": "s59_mirror_block", "part": "BLOCK", "occurrence": None, "review": []},
            "candidate": {"file": f"{cid}.stp"}, "class": klass, "truth": truth,
            "construction": {"from": "BLOCK", "defect": text},
            "frame": {"axis": list(motion.axis), "angle_deg": motion.angle, "shift": list(motion.shift)},
            "placement": [row[:4] for row in m_inv(m_motion(motion))[:3]],
            "expect": {"default": expect, "functional": "default"}, "defect": defect,
            "rules": ["KC9", "AL9"] if truth == "does_not_fit" else ["KC8"], "tests": text,
        })

def many(op, first, rest):
    """One general-fuse boolean of `first` with every shape of `rest` (cut or fuse), run in parallel."""
    args, tools = TopTools_ListOfShape(), TopTools_ListOfShape()
    args.Append(first)
    for t in rest:
        tools.Append(t)
    out = op()
    out.SetArguments(args)
    out.SetTools(tools)
    out.SetRunParallel(True)
    out.Build()
    if not out.IsDone():
        raise RuntimeError("a general fuse failed")
    return out.Shape()


def hole_grid(n, pitch=10.0, d=6.6, t=8.0):
    """A plate n x n pitches square and `t` thick, a dia `d` hole through at the middle of every pitch."""
    return many(BRepAlgoAPI_Cut, box(0, 0, 0, n * pitch, n * pitch, t),
                [cyl((pitch * (i + 0.5), pitch * (j + 0.5), -1), UP, d / 2, t + 2) for i in range(n) for j in range(n)])


def build_hostile_cases(w: Writer):
    """M4-13 and AL11's bounds on hostile inputs, each judged by matching.rs within a time bound: never a pass
    from a bound (a fit rejected only by checks carrying `alignment_exhausted` or `coverage_exhausted`), and no
    pass that does not fit; where a case names the bound that fires, it fires. 10,000 identical holes against
    s01's bracket (alignment's placement bound); a 1,600-bore socket (h01, a match-only source: a plate on 1,600
    pins standing in a base) against a 1,681-hole grid (coverage's); the bracket with its body fused from 1,000
    tiles, so each of its faces is hundreds; and the bracket with ten rings of 400 dia 0.4 stubs on its top face
    round one bore, fused into scalloped walls of about 29,000 faces outside the bore's core."""
    def case(cid, source, part, truth, text, rules, **expect):
        w.cases.append({"id": cid, "kind": "hostile", "file": f"{cid}.stp", "set": "corpus",
                        "socket": {"case": source, "part": part, "occurrence": None, "review": []},
                        "candidate": {"file": f"{cid}.stp"}, "class": "h_bounds", "truth": truth,
                        "expect": {"within_s": 30, **expect}, "rules": rules, "tests": text})
    w.shape(hole_grid(100, d=6.6), "h_hole_grid")
    case("h_hole_grid", "s01_bolted", "BRACKET", "does_not_fit",
         "10,000 dia 6.6 holes on a 10 mm pitch: alignment reaches its 4,096 placements, none covering the "
         "bracket's seats", ["AL11", "KC9"], placements_exhausted=True)
    pins = Leaf("PIN", cyl((0, 0, -10), UP, 3, 18))
    root = Node("h01_bore_grid").add(Leaf("BASE", hole_grid(40, d=6.0, t=10.0)), m_tr((0, 0, -10)))
    root.add(Leaf("PLATE", hole_grid(40)))
    for i in range(40):
        for j in range(40):
            root.add(pins, m_tr((10 * (i + 0.5), 10 * (j + 0.5), 0)))
    write_xcaf(w, root, "h01_bore_grid")
    w.shape(hole_grid(41), "h_bore_grid")
    case("h_bore_grid", "h01_bore_grid", "PLATE", "fits",
         "a 41 x 41 grid of the same holes against the 40 x 40 one's socket: it fits, a row and a column over; "
         "the 1,601 requirements' coverage exhausts its bound, as the original's own does", ["AL11", "SL4", "KC8"],
         bound="coverage_exhausted")
    tiles = many(BRepAlgoAPI_Fuse, box(-40, -25, 0, 2, 2, 8),
                 [box(-40 + 2 * i, -25 + 2 * j, 0, 2, 2, 8) for i in range(40) for j in range(25)][1:])
    cuts = [cyl((x, y, -1), UP, 3.3, 10) for x, y in PTS] + [cyl((0, 0, -1), UP, 4.0, 10)]
    w.shape(many(BRepAlgoAPI_Cut, tiles, cuts), "h_bracket_tiles")
    case("h_bracket_tiles", "s01_bolted", "BRACKET", "fits",
         "the bracket fused from 1,000 tiles 2 x 2 x 8: its seats hundreds of coplanar faces", ["AL11", "KC8"])
    stubs = [cyl((30 + r * math.cos(a), 15 + r * math.sin(a), 8), UP, 0.2, 1.0)
             for r in (7.5 + 0.5 * k for k in range(10)) for a in (2 * math.pi * m / 400 for m in range(400))]
    w.shape(many(BRepAlgoAPI_Fuse, c_bracket(), stubs), "h_bracket_stubs")
    case("h_bracket_stubs", "s01_bolted", "BRACKET", "fn_rule",
         "the bracket with ten rings of 400 dia 0.4 stubs, 1 tall, on its top face 7.5 to 12 from the hole at "
         "(30, 15), each ring fused into a scalloped wall, about 29,000 faces, clear of the bolt's head and of the "
         "bore's core: their feet cut the top face the seat's region is (R14's target-face basis)",
         ["AL11", "KC10"])

def build_family_cases(w: Writer):
    """The families PR 5 left (15.3): s07b's two sockets of one part, s21's self-paired hole, s18's countersink
    turned to a conical boss, and the bracket written with pcurves and with its closed faces split. Each states
    its placement as the construction's (KO1)."""
    def case(cid, source, part, shape, motion, truth, klass, expect, defect, text, rules, pcurves=False):
        w.shape(BRepBuilderAPI_Transform(shape, motion.trsf(), True).Shape(), cid, pcurves=pcurves)
        w.cases.append({
            "id": cid, "kind": "match", "file": f"{cid}.stp", "set": "corpus",
            "socket": {"case": source, "part": part, "occurrence": None, "review": []},
            "candidate": {"file": f"{cid}.stp"}, "class": klass, "truth": truth,
            "construction": {"from": part, "defect": text},
            "frame": {"axis": list(motion.axis), "angle_deg": motion.angle, "shift": list(motion.shift)},
            "placement": [row[:4] for row in m_inv(m_motion(motion))[:3]],
            "expect": expect if "default" in expect else {"default": expect, "functional": "default"},
            "defect": defect, "rules": rules, "tests": text,
        })
    B7, B = ("s07b_one_missing", "BRACKET"), ("s01_bolted", "BRACKET")
    bores = [l_bore((x, y, 0)) for x, y in PTS]
    seats = [l_seat((0, 0, 1), 8), l_seat((0, 0, -1), 0)]
    every = ("no_feature", "axis_offset", "diameter_out_of_limits:min", "diameter_out_of_limits:max",
             "wall_missing", "end_not_open", "core_intruded", "contact_not_covered", "seat_missing")
    may = [must(b, *every) for b in bores] + [must(x, *every) for x in seats] + [must(l_grip(8), "grip_length", "seat_missing")]
    small = {p: (p[0], p[1], 6.4, "through", None) for p in PTS}
    case("m_s07b_bracket", *B7, c_bracket(), VARIANT, "fits", "g_some_sockets_all", m_ok(), [],
         "the original against both its sockets", ["KC8"])
    case("m_s07b_dia_64", *B7, c_bracket(holes=small), VARIANT, "fn_rule", "b_some_sockets",
         m_rej([any_of(*[must(b, "diameter_out_of_limits:min") for b in bores])], may), [],
         "every hole dia 6.4: socket-1's clearance bores pass it, socket-2's pinned bore, the plate's hole with "
         "nothing across (J1), does not; it fits, the rule rejects it", ["KC10", "VR7", "VD2"])
    block10 = through(box(-20, -20, 0, 40, 40, 10), [(0, 0)], 10, 0, 10)
    case("m_s21_block", "s21_self_rod", "BLOCK10", block10, VARIANT, "fits", "g_frame", m_ok(), [],
         "the original against both copies' sockets", ["KC8"])
    case("m_s21_dia_1002", "s21_self_rod", "BLOCK10", through(box(-20, -20, 0, 40, 40, 10), [(0, 0)], 10.02, 0, 10),
         VARIANT, "fn_rule", "b_self", m_rej([must(l_bore((0, 0, 0)), "diameter_out_of_limits:max")],
                                             [must(l_bore((0, 0, 0)), *every)]), [],
         "its pinned dia 10 hole at 10.02: nothing passes through it, J1's pinned rule rejects it",
         ["KC10", "VR7", "VR17"])
    boss = fuse(cut(box(-20, -20, 0, 40, 40, 10), cyl((0, 0, -1), UP, 3.3, 12)),
                cut(cone((0, 0, 10), UP, 8.2, 3.3, 4.9), cyl((0, 0, 9), UP, 3.3, 7)))
    cs = l_cone((0, 0, 3.8))
    case("m_plate18_boss", "s18_csink", "PLATE", boss, VARIANT, "does_not_fit", "b_cone_sense",
         m_rej([must(cs, "no_feature", "cone_mismatch:angle", "cone_mismatch:apex", "cone_mismatch:range")],
               [must(cs, *every, "cone_mismatch:angle", "cone_mismatch:apex", "cone_mismatch:range"),
                must(l_bore((0, 0, 0)), *every), must(l_seat((0, 0, -1), 0), *every),
                must(l_grip(3.8), "grip_length", "seat_missing")]),
         [must(cs, "no_feature", "cone_mismatch:angle", "cone_mismatch:apex", "cone_mismatch:range")],
         "the countersink filled, a convex cone standing on the top face round the hole in its place: the "
         "screw's head loses its seat", ["KC9", "VR13"])
    case("m_bracket_pcurves", *B, c_bracket(), TILT, "fits", "g_writer", m_ok(), [],
         "the original written with pcurves", ["KC6", "KC8"], pcurves=True)
    divide = ShapeUpgrade_ShapeDivideClosed(c_bracket())
    divide.Perform()
    case("m_bracket_split", *B, divide.Result(), VARIANT, "fits", "g_writer", m_ok(), [],
         "the original with every closed face split in two", ["KC6", "KC8"])

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
            # The first line that differs, so a difference on another
            # platform can be seen in its CI log.
            pairs = zip(x.splitlines(), y.splitlines())
            first = next(((u, v) for u, v in pairs if NUMBER.sub("#", u) != NUMBER.sub("#", v)), ("", ""))
            found.append(f"{name}: differs in more than its numbers: generated {first[0][:200]!r}, "
                         f"committed {first[1][:200]!r}")
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
    parser.add_argument("--candidate-set", action="store_true",
                        help="write only the committed candidates (crates/fitment-core/tests/candidates)")
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
    elif args.candidate_set:
        build_candidates(w, lambda unit_set: unit_set)
    else:
        build(w, args.unit_set)
        if not args.unit_set:
            build_assemblies(w, lambda unit_set: True)
            build_candidates(w, lambda unit_set: True)
            build_component_cases(w)
            build_gate_cases(w)
            build_mirror_cases(w)
            build_hostile_cases(w)
            build_family_cases(w)
    committed = args.unit_set or args.assembly_set or args.candidate_set
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
        mode = "--assembly-set" if args.assembly_set else "--candidate-set" if args.candidate_set else "--unit-set"
        if found:
            raise SystemExit(f"{args.check} is not what the generator writes: regenerate it with "
                             f"tools/gen-corpus.py {mode} {args.check}")
        print(f"{args.check} matches the generator")


if __name__ == "__main__":
    main()
