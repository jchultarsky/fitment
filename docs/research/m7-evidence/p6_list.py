#!/usr/bin/env python3
"""P6's list: every corpus match case whose candidate file passes its interface under the default policy,
as `case \t truth \t socket.json \t candidate.stp \t match.json` (docs/research/m7-fit.md, FT10).

    python3 docs/research/m7-evidence/p6_list.py CORPUS FITMENT SCRATCH > SCRATCH/p6.tsv
"""
import json
import subprocess
import sys
from pathlib import Path

corpus, binary, scratch = Path(sys.argv[1]).resolve(), Path(sys.argv[2]).resolve(), Path(sys.argv[3]).resolve()
scratch.mkdir(parents=True, exist_ok=True)
docs = {}
for case in json.loads((corpus / "manifest.json").read_text())["cases"]:
    if case.get("kind") != "match" or "file" not in case["candidate"] or case["truth"] == "refused":
        continue
    sock = case["socket"]
    if sock.get("review") or sock.get("edit") or sock.get("floor"):
        continue
    key = (sock["case"], sock["part"])
    if key not in docs:
        out = scratch / f"{sock['case']}.{sock['part']}.socket.json"
        ok = subprocess.run([str(binary), "socket", str(corpus / f"{sock['case']}.stp"), "--part", sock["part"],
                             "-o", str(out)], capture_output=True).returncode == 0
        docs[key] = out if ok else None
    if docs[key] is None:
        continue
    cand = corpus / case["candidate"]["file"]
    out = subprocess.run([str(binary), "match", str(docs[key]), str(cand), "--format", "json"],
                         capture_output=True, text=True).stdout
    if json.loads(out)["candidates"][0]["verdict"] == "reject":
        continue
    path = scratch / f"{case['id']}.match.json"
    path.write_text(out)
    print("\t".join([case["id"], case["truth"], str(docs[key]), str(cand), str(path)]))
