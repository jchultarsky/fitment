"""Selectivity of plane-pair (grip) tokens for a seats-only socket (design B, IX-P).

    planes_probe.py DB SEPARATION_MM WINDOW_MM

Counts parts with an exactly opposed pair of planes whose separation
(o1 + o2) lies within WINDOW_MM of SEPARATION_MM, against all parts.
"""
import json, math, sqlite3, sys
db, sep, win = sys.argv[1], float(sys.argv[2]), float(sys.argv[3])
c = sqlite3.connect(db)
n = hit = tilted = pairs_total = 0
for (j,) in c.execute("SELECT json FROM features"):
    planes = json.loads(j)["part"]["planes"]
    n += 1
    found = False
    ps = [(p["normal"], p["offset_mm"]) for p in planes]
    for i in range(len(ps)):
        for k in range(i + 1, len(ps)):
            a, b = ps[i][0], ps[k][0]
            d = a[0]*b[0] + a[1]*b[1] + a[2]*b[2]
            if d <= -math.cos(math.radians(0.1)):
                pairs_total += 1
                if d <= -1 + 1e-12:
                    if abs(ps[i][1] + ps[k][1] - sep) <= win:
                        found = True
                else:
                    tilted += 1
                    found = True
    hit += found
print(f"{db}: {hit} of {n} parts admitted by the grip token ({hit/n:.2%}); opposed pairs per part {pairs_total/n:.1f}; tilted-opposed pairs {tilted}")
