"""Compare a catalog run with a file run over the same parts (M5 design B, CQ1-CQ3).

    compare.py FILE_RUN.json CATALOG_RUN.json

The catalog run must hold exactly the file run's candidates that were read
and that level 2 kept (stage != shortlist), each entry equal labels aside,
in the same relative order. Exit 1 on any difference.
"""
import json, sys

f = json.load(open(sys.argv[1]))
c = json.load(open(sys.argv[2]))

def key(e):
    return e["source"]["sha256"]

def strip(e):
    e = dict(e); e.pop("labels", None); return e

for k in ("format", "format_version", "extractor_version", "socket_rules_version",
          "signature_version", "match_rules_version", "policy", "options", "fit_verifier", "socket"):
    if f[k] != c[k]:
        print("envelope differs:", k); sys.exit(1)

expected = [e for e in f["candidates"]
            if e["status"] == "accepted" and all(s["stage"] != "shortlist" for s in e["sockets"])]
dropped = [e for e in f["candidates"] if e["status"] == "accepted" and any(s["stage"] == "shortlist" for s in e["sockets"])]
bad_drop = [e for e in dropped if e["verdict"] != "reject"]
got = c["candidates"]
ek = [key(e) for e in expected]; gk = [key(e) for e in got]
ok = True
if bad_drop:
    print("SL1 VIOLATION: a passing candidate dropped by level 2:", [key(e) for e in bad_drop]); ok = False
missing = set(ek) - set(gk)
extra = set(gk) - set(ek)
if missing:
    print("LEVEL 1 DROPPED what level 2 keeps:", len(missing), [ (e["verdict"], e["labels"]) for e in expected if key(e) in missing][:5]); ok = False
if extra:
    print("catalog judged what level 2 drops:", len(extra)); ok = False
if ok and ek != gk:
    print("order differs"); ok = False
if ok:
    for a, b in zip(expected, got):
        if strip(a) != strip(b):
            print("entry differs:", a["labels"], b["labels"]); ok = False; break
v = lambda es: sum(e["verdict"] != "reject" for e in es)
print(f"{'OK ' if ok else 'BAD'} file {len(f['candidates'])} (refused {sum(e['status']!='accepted' for e in f['candidates'])}, level-2 drops {len(dropped)}), catalog {len(got)}; passes file {v(f['candidates'])} catalog {v(got)}")
sys.exit(0 if ok else 1)
