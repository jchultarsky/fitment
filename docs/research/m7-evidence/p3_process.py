#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11,<3.14"
# dependencies = ["cadquery-ocp-novtk==8.0.1.1.0"]
# ///
"""P3: what a helper process costs, and what hostile STEP does to Open CASCADE.

    docs/research/m7-evidence/p3_process.py driver FILE... > p3_process.txt
    docs/research/m7-evidence/p3_process.py read FILE     (one worker; prints one JSON line)

The driver times a worker's start (uv's environment warm, then Python and
OCP's import), then reads each FILE in its own worker under a wall-clock
limit, reporting the time, the peak RSS, the solids and the exit (a
negative code is a signal: a crash). It then writes hostile files into a
scratch directory and reads each the same way.
"""

import json
import os
import resource
import subprocess
import sys
import tempfile
import time
from pathlib import Path

LIMIT_S = 120


def worker(path):
    t0 = time.perf_counter()
    from OCP.IFSelect import IFSelect_RetDone  # noqa: F401
    from OCP.STEPCAFControl import STEPCAFControl_Reader
    from OCP.TCollection import TCollection_ExtendedString
    from OCP.TDocStd import TDocStd_Document
    from OCP.TopAbs import TopAbs_SOLID
    from OCP.TopExp import TopExp_Explorer
    from OCP.XCAFDoc import XCAFDoc_DocumentTool
    from OCP.collections import Sequence_TDF_Label
    t_import = time.perf_counter() - t0
    t1 = time.perf_counter()
    doc = TDocStd_Document(TCollection_ExtendedString("MDTV-XCAF"))
    reader = STEPCAFControl_Reader()
    status = reader.ReadFile(path)
    ok = status == IFSelect_RetDone and reader.Transfer(doc)
    solids = 0
    if ok:
        tool = XCAFDoc_DocumentTool.ShapeTool_s(doc.Main())
        free = Sequence_TDF_Label()
        tool.GetFreeShapes(free)
        for i in range(1, free.Length() + 1):
            e = TopExp_Explorer(tool.GetShape_s(free.Value(i)), TopAbs_SOLID)
            while e.More():
                solids += 1
                e.Next()
    rss = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
    rss_mib = rss / (1 << 20) if sys.platform == "darwin" else rss / 1024
    print(json.dumps({"import_s": round(t_import, 3), "read_s": round(time.perf_counter() - t1, 3),
                      "read_status": int(status), "transferred": bool(ok), "solids": solids,
                      "peak_rss_mib": round(rss_mib, 1)}))


def spawn(path):
    t0 = time.perf_counter()
    try:
        out = subprocess.run([sys.executable, __file__, "read", str(path)], capture_output=True, text=True,
                             timeout=LIMIT_S)
    except subprocess.TimeoutExpired:
        return {"exit": "timeout", "wall_s": LIMIT_S}
    line = out.stdout.strip().splitlines()[-1] if out.stdout.strip() else ""
    result = {"exit": out.returncode, "wall_s": round(time.perf_counter() - t0, 3)}
    try:
        result.update(json.loads(line))
    except ValueError:
        result["stdout_tail"] = line[-200:]
    noise = [l for l in out.stdout.splitlines()[:-1] if l.strip()]
    if noise:
        result["kernel_lines_on_stdout"] = len(noise)
    if out.stderr.strip():
        result["stderr_tail"] = out.stderr.strip().splitlines()[-1][-200:]
    return result


HEADER = ("ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION(('h'),'2;1');\nFILE_NAME('h','2026-10-18T00:00:00',(''),(''),"
          "'','','');\nFILE_SCHEMA(('AUTOMOTIVE_DESIGN { 1 0 10303 214 1 1 1 1 }'));\nENDSEC;\nDATA;\n")


def hostile(scratch, sample):
    out = {}
    text = Path(sample).read_text(errors="replace")
    out["truncated_half"] = text[: len(text) // 2]
    out["random_bytes"] = os.urandom(200_000).decode("latin-1")
    out["dangling_reference"] = text.replace("#1=", "#999999999=", 1)
    out["self_reference"] = HEADER + "#1=CARTESIAN_POINT('',(#1,#1,#1));\n#2=VERTEX_POINT('',#1);\nENDSEC;\nEND-ISO-10303-21;\n"
    pts = ",".join("0." for _ in range(2_000_000))
    out["huge_list"] = HEADER + f"#1=CARTESIAN_POINT('',({pts}));\nENDSEC;\nEND-ISO-10303-21;\n"
    chain = "".join(f"#{i}=CARTESIAN_POINT('',(0.,0.,{i}.));\n" for i in range(1, 400_001))
    out["many_entities_400k"] = HEADER + chain + "ENDSEC;\nEND-ISO-10303-21;\n"
    out["nan_coordinates"] = text.replace("CARTESIAN_POINT('',(", "CARTESIAN_POINT('',(1.E400,", 5)
    files = {}
    for name, body in out.items():
        p = scratch / f"{name}.stp"
        p.write_text(body, encoding="latin-1")
        files[name] = p
    return files


def driver(paths):
    t0 = time.perf_counter()
    subprocess.run([sys.executable, "-c", "import OCP.BRepAlgoAPI"], check=True)
    print(f"python + import of OCP.BRepAlgoAPI, warm: {time.perf_counter() - t0:.2f} s")
    for p in paths:
        print(f"{Path(p).name} ({Path(p).stat().st_size} bytes): {json.dumps(spawn(p))}")
    with tempfile.TemporaryDirectory() as d:
        for name, p in hostile(Path(d), paths[0]).items():
            print(f"hostile {name} ({p.stat().st_size} bytes): {json.dumps(spawn(p))}")


if __name__ == "__main__":
    if sys.argv[1] == "read":
        worker(sys.argv[2])
    else:
        driver(sys.argv[2:])
