# /// script
# requires-python = "==3.13.*"
# dependencies = ["cadquery-ocp-novtk==8.0.1.1.0"]
# ///
"""fitment's fit helper (docs/research/m7-fit.md, FH1 to FH6, FL2, FL4, FL5, FY3).

fitment-fit starts it as `<base interpreter> -I fit_helper.py --site <site-packages>`,
never through uv or a launcher (FL2). It speaks FH1's frames on its standard input and
on a private copy of its standard output, measures with Open CASCADE what fitment-core
asks, and decides nothing: every status is core's (FV1). It never uses the network.

    uv run --locked --script fit_helper.py --print-env

is `fitment fit setup`'s only use of uv: it prints the environment for the manifest.

This text is fitment's own, under the MIT licence. It imports Open CASCADE (the OCP
binding, LGPL-2.1 with its exception) at run time from the environment uv made, and
contains none of it.
"""

import os
import sys

# FH2: before anything can print, frames get a descriptor of their own and fd 1 becomes
# a copy of fd 2, so whatever Open CASCADE writes on std::cout reaches stderr (P3).
FRAMES = os.dup(1)
os.dup2(2, 1)
if sys.platform == "win32":
    import msvcrt

    for _fd in (0, FRAMES):
        msvcrt.setmode(_fd, os.O_BINARY)
PARENT = os.getppid()

import contextlib  # noqa: E402
import hashlib  # noqa: E402
import json  # noqa: E402
import math  # noqa: E402
import platform  # noqa: E402
import struct  # noqa: E402
import sysconfig  # noqa: E402
from importlib import metadata  # noqa: E402

PROTOCOL = 1
BINDING = "cadquery-ocp-novtk"
MAGIC = b"FFIT"
MAX_HEADER = 16 << 20
MAX_PAYLOAD = 1 << 30
MAX_PAYLOADS = 4
GRACE_S = 5  # the alarm's margin past a deadline (FL4)
FSIZE = MAX_PAYLOAD + (1 << 20)  # the largest payload, plus 1 MiB (FY3)
NOFILE = 64
REACH_SLACK_MM = 1e-6  # FT8's slack, as fitment-core's REACH_SLACK_MM
GUARD_MM = 1e-6  # the least u_pair (FT7)
M7_50 = None  # Windows' refusal is fitment-fit's, from this helper's platform (M7-50)


# ------------------------------------------------------------------ frames (FH1)


class Refused(Exception):
    """A request this helper cannot read: an `error` frame, and the helper stays up (FH5)."""

    def __init__(self, code: str, sentence: str):
        super().__init__(sentence)
        self.code, self.sentence = code, sentence


class Broken(Exception):
    """A channel that no longer carries frames: the helper ends."""


def read_exactly(n: int) -> bytes:
    parts, left = [], n
    while left:
        chunk = os.read(0, min(left, 1 << 20))
        if not chunk:
            raise EOFError
        parts.append(chunk)
        left -= len(chunk)
    return b"".join(parts)


def read_frame():
    """One request: its header's bytes, the header, its payloads. EOFError at the end."""
    start = read_exactly(8)
    if start[:4] != MAGIC:
        raise Broken("a frame does not start with FFIT")
    (length,) = struct.unpack(">I", start[4:])
    if length > MAX_HEADER:
        raise Broken("a header over its cap")
    raw = read_exactly(length)
    try:
        header = json.loads(raw)
    except ValueError as exc:
        raise Broken(f"a header that is not JSON: {exc}") from None
    if not isinstance(header, dict):
        raise Broken("a header that is not an object")
    sizes = header.get("payloads", [])
    if (not isinstance(sizes, list) or len(sizes) > MAX_PAYLOADS
            or not all(isinstance(n, int) and not isinstance(n, bool) and 0 <= n <= MAX_PAYLOAD for n in sizes)):
        raise Broken("payloads that break FH1")
    return raw, header, [read_exactly(n) for n in sizes]


def write_frame(header: dict) -> None:
    body = json.dumps(header, allow_nan=False, separators=(",", ":")).encode()
    data = MAGIC + struct.pack(">I", len(body)) + body
    while data:
        written = os.write(FRAMES, data)
        data = data[written:]


def error_frame(header, raw, code: str, sentence: str) -> None:
    nonce = header.get("nonce") if isinstance(header, dict) else None
    write_frame({"op": "error", "nonce": nonce if isinstance(nonce, str) else None,
                 "request_sha256": hashlib.sha256(raw).hexdigest() if raw is not None else None,
                 "error": {"code": code, "sentence": sentence}})


# ------------------------------------------------------------- strict reading (FH5)


def fields(obj, required, optional=()):
    """`obj` as a dict holding every key of `required`, and nothing but those and `optional`."""
    if not isinstance(obj, dict):
        raise Refused("unreadable", "a request's value is not an object")
    keys = set(obj)
    missing, extra = set(required) - keys, keys - set(required) - set(optional)
    if missing or extra:
        raise Refused("unreadable", f"keys missing {sorted(missing)} or not known {sorted(extra)}")
    return obj


def number(x) -> float:
    if isinstance(x, bool) or not isinstance(x, (int, float)) or not math.isfinite(x):
        raise Refused("unreadable", "a number that is not finite")
    return float(x)


def numbers(xs, n: int):
    if not isinstance(xs, list) or len(xs) != n:
        raise Refused("unreadable", f"not {n} numbers")
    return [number(x) for x in xs]


def ids(xs):
    if not isinstance(xs, list) or not all(isinstance(x, int) and not isinstance(x, bool) and x >= 0 for x in xs):
        raise Refused("unreadable", "ids that are not non-negative integers")
    return tuple(xs)


def rows(xs):
    if not isinstance(xs, list) or len(xs) != 3:
        raise Refused("unreadable", "a transform that is not three rows")
    return [numbers(r, 4) for r in xs]


def tagged(obj, kinds):
    """An externally tagged enum, `{"kind": {...}}`: its kind and its fields."""
    if not isinstance(obj, dict) or len(obj) != 1:
        raise Refused("unreadable", "an enum that is not one tagged object")
    ((kind, body),) = obj.items()
    if kind not in kinds:
        raise Refused("unreadable", f"an unknown kind {kind!r}")
    return kind, fields(body, kinds[kind])


LANDMARK_KINDS = {"plane": ("normal", "offset_mm"), "cylinder": ("point_mm", "direction", "radius_mm")}


def landmark(obj):
    f = fields(obj, ("face", "value"))
    kind, v = tagged(f["value"], LANDMARK_KINDS)
    if kind == "plane":
        numbers(v["normal"], 3), number(v["offset_mm"])
    else:
        numbers(v["point_mm"], 3), numbers(v["direction"], 3), number(v["radius_mm"])
    return ids([f["face"]])[0], kind


# ----------------------------------------------------------------- limits (FY3, FL5)


LIMITS = {}  # name -> (took, value), reported in `hello`


def note(name: str, took: bool, value=None) -> None:
    LIMITS[name] = (bool(took), value if took else None)


def early_limits() -> None:
    """The bounds that need no request: no core file, few descriptors, payload-sized files, and an end with fitment."""
    try:
        import resource
    except ImportError:
        for name in ("RLIMIT_CORE", "RLIMIT_NOFILE", "RLIMIT_FSIZE"):
            note(name, False)
    else:
        for name, value in (("RLIMIT_CORE", 0), ("RLIMIT_NOFILE", NOFILE), ("RLIMIT_FSIZE", FSIZE)):
            what = getattr(resource, name)
            try:
                _, hard = resource.getrlimit(what)
                value = value if hard == resource.RLIM_INFINITY else min(value, hard)
                resource.setrlimit(what, (value, value))
                note(name, True, value)
            except (ValueError, OSError, OverflowError):
                note(name, False)
    took = False
    if sys.platform.startswith("linux"):
        # FL5: fitment-fit spawns from a thread that lives as long as its helpers.
        try:
            import ctypes
            import signal

            took = ctypes.CDLL(None, use_errno=True).prctl(1, int(signal.SIGKILL), 0, 0, 0) == 0
        except (OSError, AttributeError):
            took = False
        if os.getppid() != PARENT:
            os._exit(0)  # fitment ended before the signal was asked for
    note("PR_SET_PDEATHSIG", took, 9 if took else None)


CPU = {"hard": None}


def request_limits(memory_mib: int, timeout_s: int, setup_s: int, checks: int) -> None:
    """The bounds `hello` sizes: memory (RLIMIT_AS on Linux, a job object on Windows) and the CPU's hard limit."""
    memory = memory_mib << 20
    try:
        import resource
    except ImportError:
        note("RLIMIT_AS", False)
        note("RLIMIT_CPU", False)
    else:
        try:
            if not sys.platform.startswith("linux"):
                raise ValueError("RLIMIT_AS is enforced on Linux only")  # macOS raises ValueError setting it
            resource.setrlimit(resource.RLIMIT_AS, (memory, memory))
            note("RLIMIT_AS", True, memory)
        except (ValueError, OSError, OverflowError):
            note("RLIMIT_AS", False)
        try:
            used = cpu_used()
            hard = int(used) + (checks + 8) * (max(timeout_s, setup_s) + GRACE_S)
            _, current = resource.getrlimit(resource.RLIMIT_CPU)
            if current != resource.RLIM_INFINITY:
                hard = min(hard, current)
            resource.setrlimit(resource.RLIMIT_CPU, (min(hard, int(used) + setup_s + GRACE_S), hard))
            CPU["hard"] = hard
            note("RLIMIT_CPU", True, hard)
        except (ValueError, OSError, OverflowError):
            note("RLIMIT_CPU", False)
    note("JOB_OBJECT", job_object(memory), memory)


JOB = []  # the job's handle, held for the process's life


def job_object(memory: int) -> bool:
    """FL5, FY3 on Windows: this process in a job with a memory limit, killed with the job's last handle."""
    if sys.platform != "win32":
        return False
    try:
        import ctypes
        from ctypes import wintypes

        class Basic(ctypes.Structure):
            _fields_ = [("PerProcessUserTimeLimit", ctypes.c_int64), ("PerJobUserTimeLimit", ctypes.c_int64),
                        ("LimitFlags", wintypes.DWORD), ("MinimumWorkingSetSize", ctypes.c_size_t),
                        ("MaximumWorkingSetSize", ctypes.c_size_t), ("ActiveProcessLimit", wintypes.DWORD),
                        ("Affinity", ctypes.c_size_t), ("PriorityClass", wintypes.DWORD),
                        ("SchedulingClass", wintypes.DWORD)]

        class Counters(ctypes.Structure):
            _fields_ = [(n, ctypes.c_uint64) for n in ("ReadOperationCount", "WriteOperationCount",
                                                       "OtherOperationCount", "ReadTransferCount",
                                                       "WriteTransferCount", "OtherTransferCount")]

        class Extended(ctypes.Structure):
            _fields_ = [("BasicLimitInformation", Basic), ("IoInfo", Counters),
                        ("ProcessMemoryLimit", ctypes.c_size_t), ("JobMemoryLimit", ctypes.c_size_t),
                        ("PeakProcessMemoryUsed", ctypes.c_size_t), ("PeakJobMemoryUsed", ctypes.c_size_t)]

        kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
        kernel32.CreateJobObjectW.restype = wintypes.HANDLE
        kernel32.CreateJobObjectW.argtypes = [ctypes.c_void_p, wintypes.LPCWSTR]
        kernel32.GetCurrentProcess.restype = wintypes.HANDLE
        kernel32.SetInformationJobObject.argtypes = [wintypes.HANDLE, ctypes.c_int, ctypes.c_void_p, wintypes.DWORD]
        kernel32.AssignProcessToJobObject.argtypes = [wintypes.HANDLE, wintypes.HANDLE]
        job = kernel32.CreateJobObjectW(None, None)
        if not job:
            return False
        info = Extended()
        # JOB_OBJECT_LIMIT_PROCESS_MEMORY | JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
        info.BasicLimitInformation.LimitFlags = 0x100 | 0x2000
        info.ProcessMemoryLimit = memory
        if not kernel32.SetInformationJobObject(job, 9, ctypes.byref(info), ctypes.sizeof(info)):
            return False
        if not kernel32.AssignProcessToJobObject(job, kernel32.GetCurrentProcess()):
            return False
        JOB.append(job)
        return True
    except (OSError, AttributeError, ValueError):
        return False


def cpu_used() -> float:
    times = os.times()
    return times.user + times.system


def arm(seconds: int) -> None:
    """FL4: a step's own bounds, so a helper whose parent cannot kill it still ends: an alarm past fitment's
    deadline, and a soft CPU limit of the CPU used so far plus the deadline (SIGXCPU, a resource kill)."""
    if sys.platform == "win32":
        return
    import signal

    signal.alarm(seconds + GRACE_S)
    if CPU["hard"] is not None:
        import resource

        with contextlib.suppress(ValueError, OSError, OverflowError):
            resource.setrlimit(resource.RLIMIT_CPU, (min(CPU["hard"], int(cpu_used()) + 1 + seconds), CPU["hard"]))


def out_of_memory():
    """FY3: memory a bound refused ends the helper as a resource kill, never as an answer (FV8)."""
    if sys.platform == "win32":
        os._exit(-1073741801)  # STATUS_NO_MEMORY, 0xC0000017
    import signal

    os.kill(os.getpid(), signal.SIGKILL)


def disarm() -> None:
    if sys.platform != "win32":
        import signal

        signal.alarm(0)


# ----------------------------------------------------------------- print-env (FL2)


def site_packages() -> str:
    return sysconfig.get_paths()["purelib"]


def print_env() -> None:
    """`fitment fit setup`'s question, under `uv run --locked --script`: the base interpreter (never a
    launcher), the environment's site-packages, the Python and the binding."""
    base = getattr(sys, "_base_executable", None) or sys.executable
    out = {"interpreter": os.path.realpath(base), "site": os.path.realpath(site_packages()),
           "python": platform.python_version(), "binding": f"{BINDING} {metadata.version(BINDING)}"}
    os.write(FRAMES, (json.dumps(out) + "\n").encode())


if len(sys.argv) == 2 and sys.argv[1] == "--print-env":
    print_env()
    sys.exit(0)
if len(sys.argv) != 3 or sys.argv[1] != "--site":
    sys.stderr.write("fit_helper.py: started as `-I fit_helper.py --site <site-packages>` or `--print-env`\n")
    sys.exit(2)
SITE = sys.argv[2]
early_limits()


# ----------------------------------------------------------------- hello (FH3)


def own_sha256(path: str) -> str:
    try:
        with open(path, "rb") as f:
            return hashlib.sha256(f.read()).hexdigest()
    except OSError:
        return ""


HELLO = ("protocol", "helper_sha256", "lock_sha256", "binding", "kernel", "python", "site", "timeout_seconds",
         "setup_timeout_seconds", "memory_mib", "max_checks")


BOUND_CAP = 10_000_000  # seconds, checks or MiB: about 115 days, 9.5 TiB; larger bounds are this


def positive(x) -> int:
    if isinstance(x, bool) or not isinstance(x, int) or x <= 0:
        raise Refused("unreadable", "a bound that is not a positive integer")
    return min(x, BOUND_CAP)


def first_hello():
    """The first frame must be `hello`: its bounds set the limits before Open CASCADE is imported."""
    while True:
        raw, header, _ = read_frame()
        try:
            if header.get("op") != "hello":
                raise Refused("unreadable", "the first request is not hello")
            f = fields(header, ("op", "nonce", "payloads", "request"))
            body = fields(f["request"], HELLO)
            bounds = [positive(body[k]) for k in ("memory_mib", "timeout_seconds", "setup_timeout_seconds",
                                                  "max_checks")]
            return raw, header, bounds
        except Refused as exc:
            error_frame(header, raw, exc.code, exc.sentence)


try:
    RAW_HELLO, HEADER_HELLO, BOUNDS = first_hello()
except (EOFError, Broken):
    sys.exit(0)
request_limits(*BOUNDS)
TIMEOUT_S, SETUP_S = BOUNDS[1], BOUNDS[2]

UNAVAILABLE = None
KERNEL = ""
try:
    import site as _site

    _site.addsitedir(SITE)
    import OCP
    from OCP.BOPAlgo import BOPAlgo_BOP, BOPAlgo_COMMON, BOPAlgo_CUT, BOPAlgo_FUSE
    from OCP.Bnd import Bnd_Box
    from OCP.BRepAdaptor import BRepAdaptor_Surface
    from OCP.BRepBndLib import BRepBndLib
    from OCP.BRepBuilderAPI import (BRepBuilderAPI_MakeFace, BRepBuilderAPI_MakePolygon,
                                    BRepBuilderAPI_Transform)
    from OCP.BRepCheck import BRepCheck_Analyzer
    from OCP.BRepExtrema import BRepExtrema_DistShapeShape
    from OCP.BRepGProp import BRepGProp
    from OCP.BRepPrimAPI import BRepPrimAPI_MakeBox, BRepPrimAPI_MakePrism, BRepPrimAPI_MakeRevol
    from OCP.GeomAbs import GeomAbs_SurfaceType as Surface
    from OCP.GProp import GProp_GProps
    from OCP.gp import gp_Ax1, gp_Dir, gp_Pnt, gp_Trsf, gp_Vec
    from OCP.IFSelect import IFSelect_RetDone
    from OCP.Message import Message, Message_Gravity
    from OCP.ShapeAnalysis import ShapeAnalysis_ShapeTolerance
    from OCP.STEPCAFControl import STEPCAFControl_Reader
    from OCP.STEPControl import STEPControl_Reader
    from OCP.TCollection import TCollection_ExtendedString
    from OCP.TDF import TDF_Label
    from OCP.TDocStd import TDocStd_Document
    from OCP.TopAbs import TopAbs_FACE, TopAbs_FORWARD, TopAbs_REVERSED, TopAbs_SHAPE, TopAbs_SOLID
    from OCP.TopExp import TopExp_Explorer
    from OCP.TopLoc import TopLoc_Location
    from OCP.TopoDS import TopoDS
    from OCP.XCAFDoc import XCAFDoc_DocumentTool
    from OCP.collections import Sequence_TDF_Label

    as_face = getattr(TopoDS, "Face_s", None) or TopoDS.Face  # OCP 8 names the casts without `_s`
    as_solid = getattr(TopoDS, "Solid_s", None) or TopoDS.Solid

    KERNEL = "Open CASCADE " + ".".join(str(OCP.__version__).split(".")[:3])
    messenger = Message.DefaultMessenger_s()
    printers = messenger.Printers()
    for _i in range(printers.Length(), 0, -1):
        messenger.RemovePrinter(printers.Value(_i))
except ImportError as exc:
    # FH3: a missing shared library is named (headless Linux: libGL.so.1, libX11.so.6, libexpat.so.1).
    UNAVAILABLE = f"Open CASCADE cannot be imported: {exc}"


def binding_version() -> str:
    try:
        return f"{BINDING} {metadata.version(BINDING)}"
    except metadata.PackageNotFoundError:
        return ""


def limits_report():
    order = ("RLIMIT_AS", "RLIMIT_CPU", "RLIMIT_CORE", "RLIMIT_NOFILE", "RLIMIT_FSIZE", "PR_SET_PDEATHSIG",
             "JOB_OBJECT")
    return [{"name": n, "took": LIMITS.get(n, (False, None))[0], "value": LIMITS.get(n, (False, None))[1]}
            for n in order]


def hello_reply() -> dict:
    script = os.path.abspath(__file__)
    return {"protocol": PROTOCOL, "pid": os.getpid(), "helper_sha256": own_sha256(script),
            "lock_sha256": own_sha256(script + ".lock"), "python": platform.python_version(),
            "binding": binding_version(), "kernel": KERNEL, "site": SITE, "platform": sysconfig.get_platform(),
            "limits": limits_report(), "unavailable": UNAVAILABLE}


# ------------------------------------------------------------------- the kernel


class Alerted(Exception):
    """A null result: the pair's `helper_exception`, never a zero (FV2)."""


def located(shape, m):
    """`shape` moved by the rows `m`, its geometry copied; the transform, for its faces' images."""
    t = gp_Trsf()
    t.SetValues(*[m[i][j] for i in range(3) for j in range(4)])
    moving = BRepBuilderAPI_Transform(shape, t, True)
    return moving.Shape(), moving


def volume(shape) -> float:
    if shape is None or shape.IsNull():
        raise Alerted("a null shape")
    props = GProp_GProps()
    BRepGProp.VolumeProperties_s(shape, props)
    v = props.Mass()
    if not math.isfinite(v) or v < -1e-9:
        raise Alerted("a volume that is not a number or negative")
    return max(v, 0.0)


def centroid(shape):
    props = GProp_GProps()
    BRepGProp.VolumeProperties_s(shape, props)
    c = props.CentreOfMass()
    return [c.X(), c.Y(), c.Z()]


def box_of(shape, optimal=False):
    """Open CASCADE's box of `shape`: ordinary (FT8), or optimal for a zone of primitives; None if it has none."""
    if shape is None or shape.IsNull():
        return None
    b = Bnd_Box()
    if optimal:
        BRepBndLib.AddOptimal_s(shape, b, False, False)
    else:
        BRepBndLib.Add_s(shape, b)
    if b.IsVoid():
        return None
    lo, hi = b.CornerMin(), b.CornerMax()
    out = {"min_mm": [lo.X(), lo.Y(), lo.Z()], "max_mm": [hi.X(), hi.Y(), hi.Z()]}
    return out if all(math.isfinite(x) for x in out["min_mm"] + out["max_mm"]) else None


def tolerance(shape) -> float:
    return ShapeAnalysis_ShapeTolerance().Tolerance(shape, 1, TopAbs_SHAPE)


def valid(shape) -> bool:
    return BRepCheck_Analyzer(shape).IsValid()


def explore(shape, kind):
    e = TopExp_Explorer(shape, kind)
    while e.More():
        yield e.Current()
        e.Next()


def alert_names(algo) -> set:
    report = algo.GetReport()
    names = set()
    for gravity in (Message_Gravity.Message_Warning, Message_Gravity.Message_Fail):
        for alert in report.GetAlerts(gravity):
            names.add(str(alert.GetMessageKey()))
    if algo.HasErrors() and not names:
        names.add("BOPAlgo_Error")
    if algo.HasWarnings() and not names:
        names.add("BOPAlgo_Warning")
    return names


def boolean(operation, argument, tools, alerts: set):
    """FT4, FH4: one boolean through `BOPAlgo_BOP`, serial, without a fuzzy value, its arguments never modified;
    every warning and error kept by its class name (FV2)."""
    algo = BOPAlgo_BOP()
    algo.AddArgument(argument)
    for tool in tools:
        algo.AddTool(tool)
    algo.SetOperation(operation)
    algo.SetRunParallel(False)
    algo.SetFuzzyValue(0.0)
    algo.SetNonDestructive(True)
    algo.Perform()
    if algo.HasWarnings() or algo.HasErrors():
        alerts |= alert_names(algo)
    shape = algo.Shape()
    if shape is None or shape.IsNull():
        raise Alerted("a boolean gave no shape")
    return shape


def united(shapes, alerts: set):
    return shapes[0] if len(shapes) == 1 else boolean(BOPAlgo_FUSE, shapes[0], shapes[1:], alerts)


def distance(a, b, alerts: set) -> float:
    d = BRepExtrema_DistShapeShape(a, b)
    if not d.IsDone():
        alerts.add("BRepExtrema_DistShapeShape_NotDone")
        raise Alerted("a distance did not finish")
    value = 0.0 if d.InnerSolution() else d.Value()
    if not math.isfinite(value) or value < 0.0:
        raise Alerted("a distance that is not a number")
    return value


def box_gap(a, b) -> float:
    """The distance between two boxes, a bound below the bodies' own."""
    if a is None or b is None:
        return 0.0
    gaps = [max(a["min_mm"][i] - b["max_mm"][i], b["min_mm"][i] - a["max_mm"][i], 0.0) for i in range(3)]
    return math.sqrt(sum(g * g for g in gaps))


def grown_meet(a, a_by, b, b_by) -> bool:
    if a is None or b is None:
        return True
    return all(a["min_mm"][i] - a_by <= b["max_mm"][i] + b_by and b["min_mm"][i] - b_by <= a["max_mm"][i] + a_by
               for i in range(3))


ANALYTIC = {Surface.GeomAbs_Plane, Surface.GeomAbs_Cylinder, Surface.GeomAbs_Cone, Surface.GeomAbs_Sphere,
            Surface.GeomAbs_Torus}


def non_analytic_near(shape, other_box, reach: float) -> bool:
    """FT5: a face of `shape` that is not analytic, within `reach` of `other_box`."""
    return any(BRepAdaptor_Surface(as_face(face)).GetType() not in ANALYTIC
               and grown_meet(box_of(face), reach, other_box, 0.0) for face in explore(shape, TopAbs_FACE))


def plane_value(face):
    surface = BRepAdaptor_Surface(face)
    if surface.GetType() != Surface.GeomAbs_Plane:
        return None
    position = surface.Plane().Position()
    d, p = position.Direction(), position.Location()
    n = [d.X(), d.Y(), d.Z()]
    if not position.Direct():
        n = [-c for c in n]
    if face.Orientation() == TopAbs_REVERSED:
        n = [-c for c in n]
    return {"plane": {"normal": n, "offset_mm": n[0] * p.X() + n[1] * p.Y() + n[2] * p.Z()}}


def cylinder_value(face):
    surface = BRepAdaptor_Surface(face)
    if surface.GetType() != Surface.GeomAbs_Cylinder:
        return None
    cylinder = surface.Cylinder()
    axis = cylinder.Axis()
    p, d = axis.Location(), axis.Direction()
    return {"cylinder": {"point_mm": [p.X(), p.Y(), p.Z()], "direction": [d.X(), d.Y(), d.Z()],
                         "radius_mm": cylinder.Radius()}}


def landmark_value(face, kind):
    """FB3: a landmark face as the kernel reads it, in the root frame; None if it is not of fitment's kind."""
    if face is None:
        return None
    value = plane_value(face) if kind == "plane" else cylinder_value(face)
    if value is None:
        return None
    flat = [x for v in value[kind].values() for x in (v if isinstance(v, list) else [v])]
    return value if all(math.isfinite(x) for x in flat) else None


def rows_of(location):
    t = location.Transformation()
    return [[t.Value(i, j) for j in range(1, 5)] for i in range(1, 4)]


def rows_agree(a, b) -> bool:
    """FB2: a location agrees with fitment's transform within 1e-9 in rotation and 1e-6 mm in translation."""
    return all(abs(a[i][j] - b[i][j]) <= 1e-9 for i in range(3) for j in range(3)) and all(
        abs(a[i][3] - b[i][3]) <= 1e-6 for i in range(3))


def file_ranks(data: bytes):
    """Each instance's entity number (its 1-based position in the file) by #id, as the oracle reads it."""
    import re

    text = data.decode("latin-1")
    start = text.find("DATA;")
    ranks = {}
    for match in re.finditer(r"^\s*#(\d+)\s*=", text[start if start >= 0 else 0:], re.M):
        ranks[int(match.group(1))] = len(ranks) + 1
    return ranks


def binder(reader, data: bytes):
    """FB2: a face's #id through the entity it was transferred from, or None."""
    model = reader.StepModel() if hasattr(reader, "StepModel") else reader.Reader().StepModel()
    entities = [model.Value(rank) for rank in range(1, model.NbEntities() + 1)]
    by_rank = {id(entity): rank for rank, entity in enumerate(entities, 1)}
    ranks = file_ranks(data)
    numbers_by_rank = {rank: number_ for number_, rank in ranks.items()} if len(ranks) == len(entities) else {}
    session = reader.WS() if hasattr(reader, "WS") else reader.Reader().WS()
    transfers = session.TransferReader()

    def face_id(face):
        key = face.Located(TopLoc_Location()).Oriented(TopAbs_FORWARD)
        entity = transfers.EntityFromShapeResult(key, 1)
        return None if entity is None else numbers_by_rank.get(by_rank.get(id(entity)))

    face_id.keep = (entities, reader)
    return face_id


def faces_by_id(shape, face_id):
    """Every face of `shape` by its #id (the first of a #id read twice); None if a face binds to none."""
    out, unbound = {}, False
    for f in explore(shape, TopAbs_FACE):
        face = as_face(f)
        number_ = face_id(face)
        if number_ is None:
            unbound = True
            continue
        out.setdefault(number_, face)
    return out, unbound


def write_payload(name: str, data: bytes) -> str:
    """FL6: a payload in this helper's own working directory, for Open CASCADE's file reader."""
    path = os.path.abspath(name)
    with open(path, "wb") as f:
        f.write(data)
    return path


# ------------------------------------------------------------------ scenes (FB2, FB5, FB6)


class Solid:
    """One solid Open CASCADE transferred from a leaf: in its definition's frame, placed, and its faces."""

    def __init__(self, local, leaf_location, face_id):
        self.local = local
        self.placed = local.Moved(leaf_location)
        self.rows = rows_of(leaf_location.Multiplied(local.Location()))
        self.faces, unbound = faces_by_id(self.placed, face_id)
        self.ids = sorted(self.faces)
        self.unbound = unbound
        if not all(math.isfinite(x) for row in self.rows for x in row):
            # A location no number states binds to no body: the solid is reported faceless, unclaimed (FB6).
            self.rows = [[1.0 * (i == j) for j in range(4)] for i in range(3)]
            self.faces, self.ids, self.unbound = {}, [], True
        self._box = None
        self._facts = None

    def box(self):
        if self._box is None:
            self._box = [box_of(self.placed)]
        return self._box[0]

    def facts(self):
        """Valid, volume and largest shape tolerance (FT6); a body whose measures are not numbers is invalid."""
        if self._facts is None:
            try:
                v, t = volume(self.placed), tolerance(self.placed)
                ok = valid(self.placed) and math.isfinite(t) and t >= 0.0
                self._facts = (ok, v, t if math.isfinite(t) and t >= 0 else 0.0)
            except Alerted:
                self._facts = (False, 0.0, 0.0)
        return self._facts


class Scene:
    """One assembly as XCAF reads it: every leaf's solids, and core's bodies bound to them (FB2, FB6)."""

    def __init__(self, data: bytes, name: str):
        self.solids = []
        self.bodies = {}  # core's key -> Solid, for those found
        self.asked = []  # (core's key, Solid or None), in the request's order
        path = write_payload(name, data)
        try:
            doc = TDocStd_Document(TCollection_ExtendedString("MDTV-XCAF"))
            reader = STEPCAFControl_Reader()
            reader.SetNameMode(True)
            if reader.ReadFile(path) != IFSelect_RetDone or not reader.Transfer(doc):
                return
            face_id = binder(reader.Reader(), data)
            tool = XCAFDoc_DocumentTool.ShapeTool_s(doc.Main())
            self.keep = (doc, reader, face_id)
            free = Sequence_TDF_Label()
            tool.GetFreeShapes(free)
            for i in range(1, free.Length() + 1):
                self.walk(tool, free.Value(i), TopLoc_Location(), face_id)
        except MemoryError:
            raise
        except Exception:  # noqa: BLE001 - a reader that raises read nothing: no body binds (FB2)
            self.solids = []
        finally:
            os.remove(path)

    def walk(self, tool, label, location, face_id):
        if tool.IsAssembly_s(label):
            components = Sequence_TDF_Label()
            tool.GetComponents_s(label, components, False)
            for i in range(1, components.Length() + 1):
                component, referred = components.Value(i), TDF_Label()
                if tool.GetReferredShape_s(component, referred):
                    self.walk(tool, referred, location.Multiplied(tool.GetLocation_s(component)), face_id)
            return
        shape = tool.GetShape_s(label)
        for s in explore(shape, TopAbs_SOLID):
            self.solids.append(Solid(as_solid(s), location, face_id))

    def bind(self, key, faces, transform):
        """FB2: the one solid whose faces are core's and whose location is core's transform, or None."""
        found = [s for s in self.solids if not s.unbound and s.ids == sorted(set(faces))
                 and rows_agree(s.rows, transform)]
        if len(found) == 1:
            self.bodies[key] = found[0]
            return found[0]
        return None


SCENES = {}  # sha256 -> Scene: at most two, the first loaded and one other (FB5)
ORDER = []


def hold(sha: str, scene: Scene) -> None:
    if sha in SCENES:
        ORDER.remove(sha)
    elif len(SCENES) >= 2:
        # FB5: a third replaces the older scene that is not the first loaded (the socket's).
        old = ORDER.pop(1)
        del SCENES[old]
    SCENES[sha] = scene
    ORDER.append(sha)


def scene_op(body, payloads):
    """FH4's `scene`: every body core names, found or not, and every solid the reader transferred."""
    f = fields(body, ("scene", "bodies"))
    sha = f["scene"]
    if not isinstance(sha, str) or len(payloads) != 1:
        raise Refused("unreadable", "a scene request without its SHA-256 and one payload")
    if hashlib.sha256(payloads[0]).hexdigest() != sha:
        raise Refused("unreadable", "the assembly's bytes are not the scene's SHA-256")
    asked = []
    for b in f["bodies"] if isinstance(f["bodies"], list) else []:
        g = fields(b, ("key", "faces", "transform", "landmarks"))
        asked.append((ids(g["key"]), ids(g["faces"]), rows(g["transform"]),
                      [landmark(x) for x in (g["landmarks"] if isinstance(g["landmarks"], list) else [])]))
    if not isinstance(f["bodies"], list):
        raise Refused("unreadable", "bodies that are not a list")
    scene = Scene(payloads[0], "scene.stp")
    hold(sha, scene)
    out = []
    for key, faces, transform, marks in asked:
        solid = scene.bind(key, faces, transform)
        scene.asked.append((key, solid))
        if solid is None:
            out.append({"key": list(key), "found": False, "valid": False, "volume_mm3": 0.0, "tolerance_mm": 0.0,
                        "box": None, "landmarks": [None] * len(marks)})
            continue
        ok, v, t = solid.facts()
        out.append({"key": list(key), "found": True, "valid": ok, "volume_mm3": v, "tolerance_mm": t,
                    "box": solid.box(),
                    "landmarks": [landmark_value(solid.faces.get(face), kind) for face, kind in marks]})
    solids = [{"faces": s.ids, "location": s.rows, "box": s.box()} for s in scene.solids]
    return {"bodies": out, "solids": solids}


# ------------------------------------------------------------------- place (FB3, FB4, FT8)


class Candidate:
    """The candidate in its own frame, and placed at each occurrence."""

    def __init__(self):
        self.shape = None
        self.faces = {}
        self.placed = {}  # occurrence -> (shape, box)
        self.tolerance = 0.0
        self.keep = None


CURRENT = {"candidate": None, "scene": None, "placements": []}


def read_candidate(data: bytes) -> Candidate:
    """FB4: a file candidate's one solid, its faces bound to their #ids in its own file."""
    c = Candidate()
    path = write_payload("candidate.stp", data)
    try:
        reader = STEPControl_Reader()
        if reader.ReadFile(path) != IFSelect_RetDone:
            return c
        reader.TransferRoots()
        solids = [as_solid(s) for i in range(1, reader.NbShapes() + 1)
                  for s in explore(reader.Shape(i), TopAbs_SOLID)]
        if len(solids) == 1:
            face_id = binder(reader, data)
            c.shape = solids[0]
            c.faces, _ = faces_by_id(c.shape, face_id)
            c.keep = (reader, face_id)
    except MemoryError:
        raise
    except Exception:  # noqa: BLE001 - a reader that raises read no solid: the candidate is invalid (FT6)
        c = Candidate()
    finally:
        os.remove(path)
    return c


def component(body) -> Candidate:
    """FC5, FB5: a component of a scene this helper holds, by its occurrence or its faces, in its definition's frame."""
    f = fields(body, ("assembly_sha256", "occurrence", "faces"))
    scene = SCENES.get(f["assembly_sha256"]) if isinstance(f["assembly_sha256"], str) else None
    occurrence, faces = ids(f["occurrence"]), sorted(set(ids(f["faces"])))
    c = Candidate()
    if scene is None:
        return c
    if occurrence:
        solid = scene.bodies.get(occurrence)
        matches = [solid] if solid is not None and solid.ids == faces else []
    else:
        matches = [s for s in scene.solids if s.ids == faces and not s.unbound]
    if matches:
        solid = matches[0]
        c.shape = solid.local
        c.faces, _ = faces_by_id(solid.local, lambda face: solid_face_id(solid, face))
    return c


def solid_face_id(solid, face):
    """A face of a solid's definition, by the #id its placed image binds to."""
    for number_, placed in solid.faces.items():
        if placed.Located(TopLoc_Location()).IsSame(face.Located(TopLoc_Location())):
            return number_
    return None


CANDIDATE_KINDS = {"step": ("sha256",), "component": ("assembly_sha256", "occurrence", "faces")}


def place_op(body, payloads):
    """FH4's `place`: the candidate's validity, tolerance and SHA-256; per placement its box and landmarks."""
    f = fields(body, ("scene", "candidate", "landmarks", "placements"))
    kind, which = tagged(f["candidate"], CANDIDATE_KINDS)
    marks = [landmark(x) for x in f["landmarks"]] if isinstance(f["landmarks"], list) else None
    if marks is None or not isinstance(f["placements"], list):
        raise Refused("unreadable", "landmarks or placements that are not lists")
    placements = []
    for p in f["placements"]:
        g = fields(p, ("socket", "occurrence", "transform"))
        placements.append((ids(g["occurrence"]), rows(g["transform"])))
    sha = None
    if kind == "step":
        if len(payloads) != 1:
            raise Refused("unreadable", "a file candidate without its one payload")
        sha = hashlib.sha256(payloads[0]).hexdigest()
        c = read_candidate(payloads[0])
    else:
        if payloads:
            raise Refused("unreadable", "a component candidate with a payload")
        c = component(which)
    CURRENT["candidate"] = c
    CURRENT["scene"] = SCENES.get(f["scene"]) if isinstance(f["scene"], str) else None
    CURRENT["placements"] = [occurrence for occurrence, _ in placements]
    out = []
    ok, v, t = False, 0.0, 0.0
    if c.shape is not None:
        try:
            v, t = volume(c.shape), tolerance(c.shape)
            ok = valid(c.shape) and math.isfinite(t) and t >= 0.0
            t = t if math.isfinite(t) and t >= 0.0 else 0.0
        except Alerted:
            ok, v, t = False, 0.0, 0.0
    c.tolerance = t
    for occurrence, transform in placements:
        out.append(placement(c, occurrence, transform, marks))
    return {"valid": ok, "volume_mm3": v, "tolerance_mm": t, "sha256": sha, "placements": out}


def placement(c, occurrence, transform, marks) -> dict:
    """FB3, FT8: the candidate placed at one occurrence, its box and its landmarks; nothing where it cannot be
    placed, so it is not paired and its landmarks are not fitment's."""
    nothing = {"occurrence": list(occurrence), "box": None, "landmarks": [None] * len(marks)}
    if c.shape is None:
        return nothing
    try:
        placed, moving = located(c.shape, transform)
        b = box_of(placed)
        values = []
        for face, mark_kind in marks:
            local = c.faces.get(face)
            # The image is FORWARD; given the face's own orientation in its solid, its normal is outward.
            image = as_face(moving.ModifiedShape(local).Oriented(local.Orientation())) if local is not None else None
            values.append(landmark_value(image, mark_kind))
    except MemoryError:
        raise
    except Exception:  # noqa: BLE001 - a placement the kernel cannot make is none (FB3)
        return nothing
    c.placed[occurrence] = (placed, b)
    return {"occurrence": list(occurrence), "box": b, "landmarks": values}


# ---------------------------------------------------------------------- zones (FZ2 to FZ4, FZ7)


ZONE_SHAPES = {"slab": ("face", "normal", "offset_mm"),
               "annulus": ("point_mm", "direction", "from_mm", "to_mm", "inner_mm", "outer_mm"),
               "shell": ("apex_mm", "direction", "half_angle_rad", "radii_mm")}


def side(w):
    """A unit vector square to `w`."""
    a = [1.0, 0.0, 0.0] if abs(w[0]) < 0.9 else [0.0, 1.0, 0.0]
    u = [a[1] * w[2] - a[2] * w[1], a[2] * w[0] - a[0] * w[2], a[0] * w[1] - a[1] * w[0]]
    n = math.sqrt(sum(x * x for x in u))
    return [x / n for x in u]


def revolved(base, w, corners):
    """The solid of revolution of a quadrilateral in the half-plane (station along w, radius)."""
    u = side(w)
    poly = BRepBuilderAPI_MakePolygon()
    for s, rho in corners:
        poly.Add(gp_Pnt(*[base[i] + s * w[i] + rho * u[i] for i in range(3)]))
    poly.Close()
    face = BRepBuilderAPI_MakeFace(poly.Wire(), True).Face()
    return BRepPrimAPI_MakeRevol(face, gp_Ax1(gp_Pnt(*base), gp_Dir(*w)), 2 * math.pi).Shape()


def unit(v):
    n = math.sqrt(sum(x * x for x in v))
    if not n > 0.0:
        raise Alerted("a direction of no length")
    return [x / n for x in v]


def zone_solid(kind, shape, d, body):
    """FZ7: a zone built from primitives at depth `d`, as fitment-core's closed forms state it."""
    if kind == "slab":
        face = body.faces.get(shape["face"]) if body is not None else None
        if face is None:
            raise Alerted("a slab's face is not the body's")
        n = unit(numbers(shape["normal"], 3))
        start, _ = located(face, [[1.0 * (i == j) for j in range(3)] + [-d * n[i]] for i in range(3)])
        return BRepPrimAPI_MakePrism(start, gp_Vec(*[2 * d * c for c in n])).Shape()
    if kind == "annulus":
        base, w = numbers(shape["point_mm"], 3), unit(numbers(shape["direction"], 3))
        s0, s1 = number(shape["from_mm"]) - d, number(shape["to_mm"]) + d
        r0, r1 = max(number(shape["inner_mm"]) - d, 0.0), number(shape["outer_mm"]) + d
        return revolved(base, w, [[s0, r0], [s1, r0], [s1, r1], [s0, r1]])
    apex, w = numbers(shape["apex_mm"], 3), unit(numbers(shape["direction"], 3))
    alpha, radii = number(shape["half_angle_rad"]), numbers(shape["radii_mm"], 2)
    sin, cos = math.sin(alpha), math.cos(alpha)
    if not (sin > 0.0 and cos > 0.0):
        raise Alerted("a cone with no closed form")

    def on_line(rho):
        return [rho * cos / sin, rho]

    normal = [-sin, cos]
    a, b = on_line(max(radii[0] - d, 0.0)), on_line(radii[1] + d)
    corners = [[p[0] + k * d * normal[0], p[1] + k * d * normal[1]] for p, k in ((a, -1), (b, -1), (b, 1), (a, 1))]
    if any(c[1] < 0.0 for c in corners):
        raise Alerted("a shell that crosses its axis")
    return revolved(apex, w, corners)


def zone_measure(solid):
    return {"volume_mm3": volume(solid), "centroid_mm": centroid(solid), "box": box_of(solid, optimal=True)}


NO_ZONE = {"volume_mm3": 0.0, "centroid_mm": [0.0, 0.0, 0.0], "box": None}


def zones_at(zones, depth_key, body, alerts):
    """Each zone built at its depth: the solids, and their measures in the request's order (FZ7)."""
    solids, measures = [], []
    for z in zones:
        try:
            solid = zone_solid(z["kind"], z["shape"], z[depth_key], body)
            measures.append(zone_measure(solid))
            solids.append(solid)
        except MemoryError:
            raise
        except Exception:  # noqa: BLE001 - FV2: every exception is an alert, never a zero
            alerts.add("helper_exception")
            measures.append(dict(NO_ZONE))
    return solids, measures


# ---------------------------------------------------------------------------- check (FT4, FT5, FY4)


ZONE_FIELDS = ("kind", "requirement", "shape", "depth_mm", "proximity_depth_mm")
PAIR_FIELDS = ("occurrence", "body", "copy", "u_pair_mm", "epsilon_mm", "threshold_mm", "zones")


def read_pair(p):
    f = fields(p, PAIR_FIELDS)
    if not isinstance(f["copy"], bool) or not isinstance(f["zones"], list):
        raise Refused("unreadable", "a pair whose copy or zones are not of their kinds")
    zones = []
    for z in f["zones"]:
        g = fields(z, ZONE_FIELDS)
        kind, shape = tagged(g["shape"], ZONE_SHAPES)
        if g["kind"] != kind:
            raise Refused("unreadable", "a zone whose kind is not its shape's")
        zones.append({"kind": kind, "shape": shape, "depth_mm": number(g["depth_mm"]),
                      "proximity_depth_mm": number(g["proximity_depth_mm"])})
    return {"occurrence": ids(f["occurrence"]), "body": ids(f["body"]), "copy": f["copy"],
            "epsilon_mm": number(f["epsilon_mm"]), "threshold_mm": number(f["threshold_mm"]),
            "u_pair_mm": number(f["u_pair_mm"]), "zones": zones}


def empty_pair(p, zone_measures):
    return {"occurrence": list(p["occurrence"]), "body": list(p["body"]), "copy": p["copy"], "common_mm3": 0.0,
            "residual_mm3": 0.0, "inside_mm3": 0.0, "residual_box": None, "distance_mm": None,
            "zoned_distance_mm": None, "body_kept_mm3": None, "body_inside_mm3": None, "non_analytic": False,
            "alerts": [], "zones": zone_measures, "proximity_zones": []}


def measure_pair(p, c, scene, raise_here: bool):
    """FT4, FT5, FY4 for one pair: the Common and its cut by the zones at epsilon, the distance, and within the
    threshold the distance to the body cut by its zones at epsilon + threshold. Any exception or null result is
    the pair's `helper_exception` alert (FV2), the request carrying on with the next pair."""
    alerts = set()
    if p["copy"]:
        other = c.placed.get(p["body"])
        body_shape, body_box, body_tol = (other[0], other[1], c.tolerance) if other else (None, None, 0.0)
        zone_body = scene.bodies.get(p["body"]) if scene is not None else None
    else:
        zone_body = scene.bodies.get(p["body"]) if scene is not None else None
        body_shape = zone_body.placed if zone_body is not None else None
        body_box = zone_body.box() if zone_body is not None else None
        body_tol = zone_body.facts()[2] if zone_body is not None else 0.0
    zones, zone_measures = zones_at(p["zones"], "depth_mm", zone_body, alerts)
    out = empty_pair(p, zone_measures)
    try:
        if raise_here:
            raise RuntimeError("an exception injected by a test (VT71)")
        mine = c.placed.get(p["occurrence"])
        if mine is None or body_shape is None:
            raise Alerted("a pair whose bodies are not both held")
        shape, my_box = mine
        threshold = p["threshold_mm"]
        apart = box_gap(my_box, body_box)
        # Boxes that hold each body and do not meet: nothing in common, no boolean needed.
        common = None if apart > 0.0 else boolean(BOPAlgo_COMMON, shape, [body_shape], alerts)
        v = 0.0 if common is None else volume(common)
        out["common_mm3"], out["residual_mm3"] = v, v
        if v > 0.0 and zones and len(zones) == len(p["zones"]):
            residual = boolean(BOPAlgo_CUT, common, zones, alerts)
            inside = boolean(BOPAlgo_COMMON, common, [united(zones, alerts)], alerts)
            out["residual_mm3"], out["inside_mm3"] = volume(residual), volume(inside)
            if out["residual_mm3"] > 0.0:
                out["residual_box"] = box_of(residual)
        elif v > 0.0:
            if p["zones"]:
                raise Alerted("a zone was not built")
            out["residual_box"] = box_of(common)
        if apart <= threshold:
            gap = distance(shape, body_shape, alerts)
            out["distance_mm"] = gap if gap <= threshold else None
        reach = p["epsilon_mm"] + 2 * threshold + REACH_SLACK_MM
        out["non_analytic"] = (non_analytic_near(shape, body_box, reach + c.tolerance + body_tol)
                               or non_analytic_near(body_shape, my_box, reach + c.tolerance + body_tol))
        if p["zones"] and out["distance_mm"] is not None:
            zoned(p, out, shape, body_shape, zone_body, alerts)
    except MemoryError:
        raise
    except Exception:  # noqa: BLE001 - FV2: every exception or null result is the pair's alert, never a zero
        alerts.add("helper_exception")
        out = empty_pair(p, zone_measures)
    out["alerts"] = sorted(alerts)
    return out


def zoned(p, out, shape, body_shape, zone_body, alerts):
    """FT5, FY4: the body less its zones at epsilon + threshold, its two parts' volumes, and its distance."""
    wide, wide_measures = zones_at(p["zones"], "proximity_depth_mm", zone_body, alerts)
    out["proximity_zones"] = wide_measures
    if len(wide) != len(p["zones"]):
        raise Alerted("a proximity zone was not built")
    kept = boolean(BOPAlgo_CUT, body_shape, wide, alerts)
    inside = boolean(BOPAlgo_COMMON, body_shape, [united(wide, alerts)], alerts)
    out["body_kept_mm3"], out["body_inside_mm3"] = volume(kept), volume(inside)
    if not any(True for _ in explore(kept, TopAbs_FACE)):
        raise Alerted("the body lies wholly within its zones")
    out["zoned_distance_mm"] = max(distance(shape, kept, alerts), out["distance_mm"])


def reach_claims(c, scene, linear_mm, placements):
    """FT8: for every scene body that is not a placed occurrence, at every placement, whether this helper's own
    boxes come within the least reach any pair of them could have (u_pair at its guard)."""
    claims = []
    reach = 3 * linear_mm + GUARD_MM + REACH_SLACK_MM
    for occurrence in placements:
        mine = c.placed.get(occurrence)
        for key, solid in (scene.asked if scene is not None else []):
            if key in placements:
                continue
            within = mine is None or solid is None or grown_meet(
                mine[1], reach + c.tolerance, solid.box(), reach + solid.facts()[2])
            claims.append({"body": list(key), "occurrence": list(occurrence), "within": bool(within)})
    return claims


def check_op(body, payloads):
    """FH4's `check`: every pair asked, in its order, and every claim of reach (FT8)."""
    f = fields(body, ("linear_mm", "pairs"), ("raise_at",))
    linear_mm = number(f["linear_mm"])
    raise_at = f.get("raise_at")
    if raise_at is not None and (isinstance(raise_at, bool) or not isinstance(raise_at, int)):
        raise Refused("unreadable", "raise_at is not an index")
    if payloads or not isinstance(f["pairs"], list):
        raise Refused("unreadable", "a check request with a payload, or pairs that are not a list")
    pairs = [read_pair(p) for p in f["pairs"]]
    c, scene = CURRENT["candidate"], CURRENT["scene"]
    if c is None:
        raise Refused("unreadable", "a check request before any place")
    out = [measure_pair(p, c, scene, i == raise_at) for i, p in enumerate(pairs)]
    return {"pairs": out, "reach": reach_claims(c, scene, linear_mm, CURRENT["placements"])}


# --------------------------------------------------------------------------- self-test (FU4)


def stress(body) -> None:
    """VT70 only: memory to take or CPU to burn, so a test sees this helper's own bounds end it."""
    f = fields(body, (), ("allocate_mib", "cpu_seconds"))
    if "allocate_mib" in f:
        held = bytearray(positive(f["allocate_mib"]) << 20)
        del held
    if "cpu_seconds" in f:
        until = cpu_used() + positive(f["cpu_seconds"])
        while cpu_used() < until:
            sum(i * i for i in range(10_000))


def self_test_op(body, payloads):
    """`fitment fit setup`'s self-test: two unit boxes touching, overlapping by 1e-3 mm and apart by 0.02 mm."""
    f = fields(body, (), ("stress",))
    if payloads:
        raise Refused("unreadable", "a self-test with a payload")
    if "stress" in f:
        stress(f["stress"])
    a = BRepPrimAPI_MakeBox(gp_Pnt(0, 0, 0), 1, 1, 1).Shape()
    cases = []
    for name, x in (("touching", 1.0), ("overlapping", 1.0 - 1e-3), ("apart", 1.02)):
        alerts = set()
        b = BRepPrimAPI_MakeBox(gp_Pnt(x, 0, 0), 1, 1, 1).Shape()
        try:
            common, gap = volume(boolean(BOPAlgo_COMMON, a, [b], alerts)), distance(a, b, alerts)
        except MemoryError:
            raise
        except Exception:  # noqa: BLE001 - FV2
            alerts.add("helper_exception")
            common, gap = 0.0, 0.0
        cases.append({"name": name, "common_mm3": common, "distance_mm": gap, "alerts": sorted(alerts)})
    return {"cases": cases}


# ------------------------------------------------------------------------------- the loop


OPS = {"scene": scene_op, "place": place_op, "check": check_op, "selftest": self_test_op}


def reply(op, raw, header, body) -> None:
    envelope = {"op": op, "nonce": header["nonce"], "request_sha256": hashlib.sha256(raw).hexdigest(),
                "reply": body}
    if "digest" in header:
        envelope["digest"] = header["digest"]
    try:
        write_frame(envelope)
    except ValueError:
        # FH5: a number no JSON can state is never sent; the request is refused instead.
        raise Refused("not_finite", "a measure is not a finite number") from None


def answer(raw, header, payloads) -> bool:
    """One request answered; False after `bye`."""
    op = header.get("op")
    if op == "bye":
        return False
    if op not in OPS:
        raise Refused("unreadable", f"an unknown operation {str(op)[:40]!r}")
    if UNAVAILABLE is not None:
        raise Refused("unavailable", UNAVAILABLE)
    f = fields(header, ("op", "nonce", "payloads", "request"), ("digest",))
    if not isinstance(f["nonce"], str):
        raise Refused("unreadable", "a nonce that is not a string")
    arm(SETUP_S if op == "scene" else TIMEOUT_S)
    try:
        body = OPS[op](f["request"], payloads)
    except (Refused, MemoryError):
        raise
    except Exception as exc:  # noqa: BLE001 - FH5: a request the helper could not answer is refused, never a zero
        raise Refused("helper_exception", f"{type(exc).__name__}: {exc}") from None
    finally:
        disarm()
    reply(op, raw, header, body)
    return True


def main() -> int:
    reply("hello", RAW_HELLO, HEADER_HELLO, hello_reply())
    while True:
        try:
            raw, header, payloads = read_frame()
        except MemoryError:
            out_of_memory()
        except EOFError:
            return 0  # FL5: fitment's end closes the pipe
        except Broken as exc:
            error_frame(None, None, "unreadable", str(exc))
            return 3
        try:
            if not answer(raw, header, payloads):
                return 0
        except Refused as exc:
            error_frame(header, raw, exc.code, exc.sentence)
        except MemoryError:
            out_of_memory()


if __name__ == "__main__":
    sys.exit(main())
