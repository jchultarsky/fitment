#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11,<3.14"
# dependencies = ["cadquery-ocp-novtk==8.0.1.1.0"]
# ///
"""P2: a prototype of M7's fit check (docs/research/m7-fit.md, FT and FZ rules) on the corpus.

    docs/research/m7-evidence/p2_fit.py CORPUS FITMENT SCRATCH [originals|cases|both|fixtures] [ID...]

With `fixtures`, CORPUS is the fixture tree and its manifest's pinned socket targets are the originals.

For every socket the corpus's match cases name, and with `originals` for
every target an assembly case states sockets for, the original is checked
at its own occurrences (the control, FT9). For every file match case
the candidate is checked (for a case whose interface fitment rejects too, for
information: M7 asks no fit there, FT11)
at the placements fitment's document states, at every occurrence of
every socket (FT2, FT3). Each check, per occurrence k and body B of the
check set (every leaf but the selected occurrences, and the candidate's
other placed copies), with its own u_pair (the largest stated uncertainty of the
two, at least 1e-6 mm) and eps = t - u_pair (FT7, as the reviews set it):

  - eps <= 0: not_verified, uncertainty_above_tolerance;
  - interference: Common(C_k, B) less the zones the socket declares for
    (k, B) at depth eps; 1e-6 mm3 or more is an interference, less a
    below_resolution (FT4, FV3);
  - proximity: B less the zones at depth eps + t + u_pair must stay farther
    than t + u_pair from C_k (FT5);
  - conservation: each cut's kept volume plus the volume inside the zones is
    the whole, to 1e-9 relative (FY4);

zones being each seat contact's neighbour face swept either side of its
plane, each interference, line-to-line or unread-occupant span's annulus,
and each conical seat's shell (FZ2 to FZ4). Booleans run non-destructive
(FH4). This prototype binds bodies by product and transform, where the
design binds by faces (FB2), and does not apply FT10's placement rule (P6
measures it). P2_REVERSE=1 runs the cases in reverse order.
Prints one line per case and a tally; timings per check. Reuses
tools/verify-occt.py's readers (XCAF leaves, faces bound to #ids).
"""

import importlib.util
import json
import math
import os
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
spec = importlib.util.spec_from_file_location("oracle", ROOT / "tools" / "verify-occt.py")
oracle = importlib.util.module_from_spec(spec)
spec.loader.exec_module(oracle)

from OCP.BOPAlgo import BOPAlgo_BOP, BOPAlgo_COMMON, BOPAlgo_CUT, BOPAlgo_FUSE  # noqa: E402
from OCP.BRepBuilderAPI import BRepBuilderAPI_MakeFace, BRepBuilderAPI_MakePolygon  # noqa: E402
from OCP.BRepCheck import BRepCheck_Analyzer  # noqa: E402
from OCP.BRepExtrema import BRepExtrema_DistShapeShape  # noqa: E402
from OCP.BRepPrimAPI import BRepPrimAPI_MakePrism, BRepPrimAPI_MakeRevol  # noqa: E402
from OCP.ShapeAnalysis import ShapeAnalysis_ShapeTolerance  # noqa: E402
from OCP.TopAbs import TopAbs_FACE, TopAbs_SHAPE  # noqa: E402
from OCP.gp import gp_Ax1, gp_Dir, gp_Pnt, gp_Vec  # noqa: E402

T = 0.01  # mm: the default policy's linear tolerance t (D9)
GUARD = 1e-6  # mm: GUARD_MM, the least uncertainty a pair is given (FT7)
CONSERVATION = 1e-9  # relative: vol(R) + vol(V within the zones) = vol(V) (FY4)
STATS = {"common": 0, "distance": 0, "cut": 0, "seconds": {"common": 0.0, "distance": 0.0, "cut": 0.0}}


class Unverified(Exception):
    """A kernel call that did not finish cleanly: not verified, never a pass."""


def bop(a, tools, operation):
    op = BOPAlgo_BOP()
    op.AddArgument(a)
    for t in tools:
        op.AddTool(t)
    op.SetOperation(operation)
    op.SetRunParallel(False)
    op.SetFuzzyValue(0.0)
    op.SetNonDestructive(True)  # the arguments are never modified, so a reused scene stays as read
    op.Perform()
    if op.HasErrors() or op.HasWarnings():
        from OCP.Message import Message_Gravity
        report = op.GetReport()
        names = []
        for gravity in (Message_Gravity.Message_Warning, Message_Gravity.Message_Fail):
            alerts = report.GetAlerts(gravity)
            it = alerts.cbegin() if hasattr(alerts, "cbegin") else None
            try:
                for alert in alerts:
                    names.append(alert.GetMessageKey())
            except TypeError:
                names.append(str(alerts.Size()))
        raise Unverified(f"a boolean reported {'errors' if op.HasErrors() else 'warnings'}: {sorted(set(names))}")
    return op.Shape()


def timed(kind, f, *args):
    t0 = time.perf_counter()
    try:
        return f(*args)
    finally:
        STATS[kind] += 1
        STATS["seconds"][kind] += time.perf_counter() - t0


def common(a, b):
    return timed("common", bop, a, [b], BOPAlgo_COMMON)


def cut(a, tools):
    return timed("cut", bop, a, tools, BOPAlgo_CUT) if tools else a


def union(shapes):
    if len(shapes) == 1:
        return shapes[0]
    return timed("cut", bop, shapes[0], shapes[1:], BOPAlgo_FUSE)


def conserved(whole, kept, zones):
    """The relative error of vol(kept) + vol(whole within the union of zones) = vol(whole); 0 without
    zones. More than CONSERVATION is a disagreement (FY4)."""
    if not zones:
        return 0.0
    w = oracle.volume_of(whole)
    inside = oracle.volume_of(common(whole, union(zones)))
    return abs(oracle.volume_of(kept) + inside - w) / max(w, 1.0)


def distance(a, b):
    def f():
        d = BRepExtrema_DistShapeShape(a, b)
        if not d.IsDone():
            raise Unverified("a distance did not finish")
        return 0.0 if d.InnerSolution() else d.Value()
    return timed("distance", f)


def max_tolerance(shape):
    return ShapeAnalysis_ShapeTolerance().Tolerance(shape, 1, TopAbs_SHAPE)


def run(binary, args):
    out = subprocess.run([str(binary), *args], capture_output=True, text=True)
    return out.returncode, out.stdout, out.stderr


def rows_equal(a, b, tol=1e-9):
    return all(abs(a[i][j] - b[i][j]) <= tol for i in range(3) for j in range(4))


def revolve(points, axis_point, axis_dir):
    """The solid swept by the polygon `points` (in a plane through the axis) about the axis."""
    poly = BRepBuilderAPI_MakePolygon()
    for p in points:
        poly.Add(gp_Pnt(*p))
    poly.Close()
    face = BRepBuilderAPI_MakeFace(poly.Wire(), True).Face()
    return BRepPrimAPI_MakeRevol(face, gp_Ax1(gp_Pnt(*axis_point), gp_Dir(*axis_dir)), 2 * math.pi).Shape()


def annulus(m, axis, lo, hi, r_in, r_out):
    """FZ3: the annulus r_in..r_out over stations lo..hi of the target's axis, placed by m."""
    p = oracle.apply_point(m, axis["point_mm"])
    d = oracle.norm(oracle.turn(m, axis["direction"]))
    s = oracle.side(d)
    pt = lambda st, r: [p[i] + st * d[i] + r * s[i] for i in range(3)]  # noqa: E731
    return revolve([pt(lo, r_in), pt(hi, r_in), pt(hi, r_out), pt(lo, r_out)], p, d)


def cone_shell(m, q, depth):
    """FZ4: a conical seat's surface over its radial range (widened by depth), swept depth either side of
    it, placed by m. The cone opens from its apex towards the axis's point."""
    apex = oracle.apply_point(m, q["apex_mm"])
    d = oracle.norm(oracle.turn(m, q["axis"]["direction"]))
    towards = oracle.sub(oracle.apply_point(m, q["axis"]["point_mm"]), apex)
    if oracle.dot(towards, d) < 0:
        d = [-c for c in d]
    half = q["included_angle_rad"] / 2
    s = oracle.side(d)
    r0, r1 = q["radii_mm"][0] - depth, q["radii_mm"][1] + depth
    # A point at radius r on the cone, moved by o along the surface's normal (radial out, axial back).
    n = [math.cos(half) * s[i] - math.sin(half) * d[i] for i in range(3)]

    def at(r, o):
        z = max(r, 0.0) / math.tan(half)
        return [apex[i] + z * d[i] + max(r, 0.0) * s[i] + o * n[i] for i in range(3)]

    return revolve([at(r0, -depth), at(r1, -depth), at(r1, depth), at(r0, depth)], apex, d)


def slab(face, depth):
    """FZ2: a planar neighbour face swept `depth` either side of its plane."""
    normal, _, _ = oracle.plane_facts(face)
    start = oracle.translated(face, [-depth * c for c in normal])
    return BRepPrimAPI_MakePrism(start, gp_Vec(*[2 * depth * c for c in normal])).Shape()


class Scene:
    """One socket document's assembly as Open CASCADE reads it: the leaves, the selected occurrences, the
    neighbours fitment names, and faces bound to their #ids."""

    def __init__(self, source, socket):
        self.socket = socket
        leaves, self.face_id = oracle.xcaf_leaves(source, bind=True)
        self.leaves = leaves
        part = socket["selection"]["part"]
        self.occurrences = {}  # path -> (rows, leaf index)
        for o in socket["extracted"]["occurrences"]:
            rows = oracle.placement_rows(o["transform"])
            ks = [i for i, (name, t, _) in enumerate(leaves) if name in (part, "SOLID") and rows_equal(t, rows)]
            if len(ks) != 1:
                raise Unverified(f"occurrence {o['path']} binds to {len(ks)} XCAF leaves")
            k = ks[0]
            self.occurrences[tuple(o["path"])] = (rows, k, o["socket"])
        self.selected = {k for _, k, _ in self.occurrences.values()}
        unc = lambda o: max(o.get("part_uncertainty_mm") or 0.0, o.get("placement_uncertainty_mm") or 0.0)  # noqa: E731
        self.u_occ = {tuple(o["path"]): unc(o) for o in socket["extracted"]["occurrences"]}
        stated = [unc(o) for o in socket["extracted"]["occurrences"] + socket["extracted"]["neighbours"]]
        # A leaf no requirement cites is not listed in the socket: the largest stated uncertainty bounds it
        # here; the design reads every leaf's own (FB1).
        self.u_default = max(stated, default=0.0)
        self.u_leaf = {}
        self._zones = {}
        self.neighbours = {}
        for n in socket["extracted"]["neighbours"]:
            rows = oracle.placement_rows(n["transform"])
            ks = [i for i, (_, t, _) in enumerate(leaves) if rows_equal(t, rows) and i not in self.selected]
            name = (n["definition"].get("product") or {}).get("name")
            ks = [i for i in ks if leaves[i][0] in (name, "SOLID")] or ks
            if len(ks) == 1:
                self.neighbours[tuple(n["path"])] = ks[0]
                self.u_leaf[ks[0]] = unc(n)
        self._faces = {}

    def face(self, leaf, face_id):
        if leaf not in self._faces:
            self._faces[leaf] = {self.face_id(oracle.as_face(f)): oracle.as_face(f)
                                 for f in oracle.explore(self.leaves[leaf][2], TopAbs_FACE)}
        return self._faces[leaf].get(face_id)

    def key(self, path):
        """A neighbour path as a check-set key: ('copy', path) for another selected occurrence, else
        ('leaf', index)."""
        path = tuple(path)
        if path in self.occurrences:
            return ("copy", path)
        return ("leaf", self.neighbours.get(path))

    def zones(self, path, depth):
        """FZ1: {key: [zone solids]} declared for the occurrence `path`, cached per depth."""
        key = (tuple(path), round(depth, 12))
        if key not in self._zones:
            self._zones[key] = self._zones_at(path, depth)
        return self._zones[key]

    def _zones_at(self, path, depth):
        rows, _, socket_id = self.occurrences[tuple(path)]
        sock = next(s for s in self.socket["extracted"]["sockets"] if s["id"] == socket_id)
        removed = {(r["socket"], r["requirement"]) for r in self.socket.get("review", []) if r["status"] == "removed"}
        out = {}
        for r in sock["requirements"]:
            if (socket_id, r["id"]) in removed:
                continue
            q = r["requires"]
            if q["kind"] == "seat":
                for contact in q["contacts"]:
                    for by in contact["by"]:
                        k = self.key(by["neighbour"])
                        if k[0] == "copy":
                            krows, kleaf, _ = self.occurrences[k[1]]
                            face = self.face(kleaf, by["face"])
                        elif k[1] is None:
                            continue
                        else:
                            face = self.face(k[1], by["face"])
                        if face is not None and oracle.plane_facts(face) is not None:
                            out.setdefault(k, []).append(slab(face, depth))
            elif q["kind"] == "conical_seat" and q.get("radii_mm"):
                zone = cone_shell(rows, q, depth)
                for src in r["sources"]:
                    if tuple(src["occurrence"]) == tuple(path):
                        out.setdefault(self.key(src["neighbour"]), []).append(zone)
            elif q["kind"] in ("bore", "shaft"):
                for span in q.get("spans", []):
                    if span.get("fit") not in ("interference", "line_to_line", "not_read"):
                        continue
                    own = (span.get("hole_diameter_mm") or span.get("shaft_diameter_mm")) / 2
                    lo, hi = sorted((span["from_mm"], span["to_mm"]))
                    for src in r["sources"]:
                        if src.get("span_mm") is None:
                            continue
                        if tuple(src["occurrence"]) != tuple(path):
                            continue
                        # The radial margin: the neighbour's radius is own - margin in a bore, own + margin
                        # about a shaft (negative for an interference).
                        # A pinned span whose occupant is unread (not_read): the original's own wall.
                        margin = src.get("margin_mm") or 0.0
                        other = own - margin if q["kind"] == "bore" else own + margin
                        zone = annulus(rows, q["axis"], lo - depth, hi + depth,
                                       max(min(own, other) - depth, 0.0), max(own, other) + depth)
                        out.setdefault(self.key(src["neighbour"]), []).append(zone)
        return out


def check_occurrence(scene, path, placed, copies, record, u_body_of):
    """FT4 and FT5 for the placed body at `path` against every leaf but the selected ones and every other
    copy, each pair with its own epsilon (C1): eps = t - u_pair; interference zones at eps; the proximity
    threshold t + u_pair, its zones at eps + t + u_pair. Returns (interferences, proximities, problems)."""
    bodies = [(("leaf", i), shape) for i, (_, _, shape) in enumerate(scene.leaves) if i not in scene.selected]
    bodies += [(("copy", p), c) for p, c in copies.items() if p != tuple(path)]
    interferes, near, problems = [], [], []
    for key, body in bodies:
        u_pair = max(GUARD, u_body_of(("self", tuple(path))), u_body_of(key))
        eps, threshold = T - u_pair, T + u_pair
        if oracle.apart(placed, body, eps + 2 * threshold):
            continue
        record["pairs"] += 1
        record["max_tolerance"] = max(record["max_tolerance"], max_tolerance(body))
        record["u_pairs"].append(u_pair)
        name = scene.leaves[key[1]][0] if key[0] == "leaf" else f"copy {list(key[1])}"
        if eps <= 0:
            problems.append(("uncertainty_above_tolerance", name, u_pair))
            continue
        zones_e = scene.zones(path, eps).get(key, [])
        shared = common(placed, body)
        v = oracle.volume_of(shared)
        residual = 0.0
        if v > 0:
            kept = cut(shared, zones_e)
            residual = oracle.volume_of(kept)
            err = conserved(shared, kept, zones_e)
            if err > CONSERVATION:
                problems.append(("conservation", name, err))
        record["residuals"].append(residual)
        if residual >= 1e-6:
            interferes.append((name, residual, v))
            continue
        if residual > 0:
            problems.append(("below_resolution", name, residual))
            continue
        if distance(placed, body) <= threshold:
            zones_p = scene.zones(path, eps + threshold).get(key, [])
            trimmed = cut(body, zones_p)
            err = conserved(body, trimmed, zones_p)
            if err > CONSERVATION:
                problems.append(("conservation", name, err))
            gap = distance(placed, trimmed)
            if gap <= threshold:
                near.append((name, gap))
    return interferes, near, problems


def check(scene, solids_by_path, record, u_self):
    """The whole fit at every occurrence: (status, details). u_self is the placed body's own stated
    uncertainty (the candidate's, or the original's for the control)."""
    record.setdefault("u_pairs", [])

    def u_body_of(key):
        if key[0] == "self":
            return max(u_self, scene.u_occ.get(key[1], 0.0))
        if key[0] == "copy":
            return max(u_self, scene.u_occ.get(key[1], 0.0))
        return scene.u_leaf.get(key[1], scene.u_default)

    for s in solids_by_path.values():
        if not BRepCheck_Analyzer(s).IsValid():
            return "not_verified", "a body is not valid (BRepCheck)"
        record["max_tolerance"] = max(record["max_tolerance"], max_tolerance(s))
    if record["max_tolerance"] > T / 10:
        return "not_verified", f"a shape tolerance of {record['max_tolerance']:.3g} mm exceeds t/10"
    found_i, found_n, found_p = [], [], []
    for path, placed in solids_by_path.items():
        i, n, q = check_occurrence(scene, path, placed, solids_by_path, record, u_body_of)
        found_i += [(list(path),) + x for x in i]
        found_n += [(list(path),) + x for x in n]
        found_p += [(list(path),) + x for x in q]
    if found_i:
        return "interferes", found_i
    if found_p:
        return "not_verified", found_p
    if found_n:
        return "not_verified", ["undeclared proximity"] + found_n
    return "verified", None


def socket_document(binary, directory, case_id, part, review, scratch):
    out = scratch / f"{case_id.replace('/', '_')}.{part.replace(' ', '_')}.socket.json"
    code, _, err = run(binary, ["socket", str(directory / f"{case_id}.stp"), "--part", part, "-o", str(out)])
    if code != 0:
        return None
    doc = json.loads(out.read_text())
    for entry in review:
        # Locators are the harness's; the prototype skips reviewed cases.
        return None
    return out, doc


def main():
    directory, binary, scratch = Path(sys.argv[1]), Path(sys.argv[2]).resolve(), Path(sys.argv[3])
    which = sys.argv[4] if len(sys.argv) > 4 else "both"
    only = set(sys.argv[5:])
    scratch.mkdir(parents=True, exist_ok=True)
    oracle.quiet()
    manifest = json.loads((directory / "manifest.json").read_text())
    if os.environ.get("P2_REVERSE"):
        # The same cases in the opposite order, in one process: the bytes must not change (FL1).
        manifest["cases"].reverse()
    if which == "fixtures":
        manifest.setdefault("cases", [])
    scenes, tally = {}, {}

    def scene_of(case_id, part):
        key = (case_id, part)
        if key not in scenes:
            got = socket_document(binary, directory, case_id, part, [], scratch)
            try:
                scenes[key] = None if got is None else (got[0], Scene(directory / f"{case_id}.stp", got[1]))
            except Unverified as exc:
                scenes[key] = str(exc)
        return scenes[key]

    def original_run(case_id, part):
        got = scene_of(case_id, part)
        if got is None:
            return "socket refused", 0.0, {}
        if isinstance(got, str):
            return ("not_bound", got), 0.0, {}
        path, scene = got
        _, out, _ = run(binary, ["match", str(path), str(directory / f"{case_id}.stp"), "--original",
                                 str(directory / f"{case_id}.stp"), "--format", "json"])
        try:
            interface = json.loads(out)["original"]["verdict"]
        except (ValueError, KeyError, TypeError):
            interface = "?"
        record = {"pairs": 0, "max_tolerance": 0.0, "residuals": [], "u_pairs": [], "interface": interface}
        originals = {p: scene.leaves[k][2] for p, (_, k, _) in scene.occurrences.items()}
        t0 = time.perf_counter()
        try:
            status, details = check(scene, originals, record, 0.0)
        except Unverified as exc:
            status, details = "not_verified", str(exc)
        return (status, details), time.perf_counter() - t0, record

    if which in ("originals", "both", "fixtures"):
        print("== originals at their own occurrences (FT9)")
        if which == "fixtures":
            targets = [(f["path"][:-4], s_) for f in manifest["fixtures"] for s_ in
                       [k.get("part") if isinstance(k, dict) else k for k in
                        (lambda s: s if isinstance(s, list) else list(s))(f.get("sockets") or [])]]
        else:
            targets = sorted({(c["id"], part) for c in manifest["cases"] if c.get("kind") == "assembly"
                              for part in ((c.get("expect") or {}).get("sockets") or {})})
        for case_id, part in targets:
            if only and case_id not in only:
                continue
            if case_id.startswith("s55"):
                print(f"skip {case_id} {part}: 7,440 faces (as the socket oracle skips it)")
                continue
            got, dt, record = original_run(case_id, part)
            status = got[0] if isinstance(got, tuple) else got
            iface = record.get("interface", "?")
            tally[("original", iface, status)] = tally.get(("original", iface, status), 0) + 1
            worst = max(record.get("residuals") or [0.0])
            print(f"{status:13} {case_id} {part} (interface: {iface}): {record.get('pairs', 0)} pairs, {dt:.2f} s, "
                  f"max tolerance {record.get('max_tolerance', 0):.2g}, largest residual {worst:.3g}"
                  + (f"; {got[1]}" if isinstance(got, tuple) and got[1] else ""))

    if which in ("cases", "both"):
        print("== match cases: the candidate at fitment's placements (FT2, FT3)")
        for case in manifest["cases"]:
            if case.get("kind") != "match" or "file" not in case["candidate"] or case["truth"] == "refused":
                continue
            if only and case["id"] not in only:
                continue
            sock = case["socket"]
            if sock.get("review") or sock.get("edit") or sock.get("floor"):
                print(f"skip {case['id']}: a reviewed or edited socket")
                continue
            got = scene_of(sock["case"], sock["part"])
            if got is None or isinstance(got, str):
                print(f"skip {case['id']}: socket refused or not bound ({got})")
                continue
            path, scene = got
            code, out, _ = run(binary, ["match", str(path), str(directory / case["candidate"]["file"]),
                                        "--format", "json"])
            doc = json.loads(out)
            entry = doc["candidates"][0]
            verdict = entry["verdict"]
            placements = {s["socket"]: oracle.placement_rows(s["placement"]) for s in entry["sockets"]
                          if s.get("placement")}
            if verdict == "reject" and len(placements) < len(entry["sockets"]):
                print(f"{'-':13} {case['id']} ({case['truth']}): rejected at {entry['sockets'][0]['stage']}, "
                      f"no placement")
                continue
            candidate = oracle.one_solid(directory / case["candidate"]["file"])
            placed = {p: oracle.moved(candidate, oracle.compose(rows, placements[s]))
                      for p, (rows, _, s) in scene.occurrences.items()}
            record = {"pairs": 0, "max_tolerance": 0.0, "residuals": [], "u_pairs": []}
            t0 = time.perf_counter()
            try:
                status, details = check(scene, placed, record, entry["read"]["max_uncertainty_mm"] or 0.0)
            except Unverified as exc:
                status, details = "not_verified", str(exc)
            dt = time.perf_counter() - t0
            kind = "interface passed" if verdict != "reject" else "interface rejected"
            tally[(kind, case["truth"], status)] = tally.get((kind, case["truth"], status), 0) + 1
            print(f"{status:13} {case['id']} ({case['truth']}, {verdict}): {record['pairs']} pairs, {dt:.2f} s"
                  + (f"; {details}" if details else ""))

    print("== tally")
    for k, v in sorted(tally.items()):
        print(f"{v:4} {' / '.join(k)}")
    print(f"kernel calls: {STATS['common']} commons {STATS['seconds']['common']:.1f} s, "
          f"{STATS['cut']} cuts {STATS['seconds']['cut']:.1f} s, "
          f"{STATS['distance']} distances {STATS['seconds']['distance']:.1f} s")


if __name__ == "__main__":
    main()
