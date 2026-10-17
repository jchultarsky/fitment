#!/usr/bin/env python3
"""Scaled copies of STEP parts for the catalog's scale tests (CT11).

`docs/research/m5-catalog.md`, section 8.3: each copy of a base part has
its length unit, an SI millimetre, replaced by a CONVERSION_BASED_UNIT of
s mm, with s log-uniform in [0.5, 2), so that every length scales exactly
as a reader applies it and no two copies are the same part. The first copy
of each base keeps s = 1, so the set holds every base's own interfaces and
the parts that pass a corpus socket appear in it, not only scaled ones that
mostly fail. No CAD kernel is involved: the edit is to the unit alone.

    tools/scale-step.py BASE_DIR OUT_DIR COUNT [--seed N] [--manifest PATH]

BASE_DIR holds the bases; with --manifest (a corpus manifest), only its
part cases' files and its match cases' candidates are taken, never an
assembly or a hostile part; a base without exactly one SI millimetre is
left out, and says so. COUNT copies are written, the bases taken in turn,
as OUT_DIR/<stem>-<n>.stp. The same arguments give the same bytes.
"""

import argparse
import json
import math
import random
import re
import sys
from pathlib import Path

MILLIMETRE = re.compile(
    r"^(#\d+)\s*=\s*\(\s*LENGTH_UNIT\s*\(\s*\)\s*NAMED_UNIT\s*\(\s*\*\s*\)\s*"
    r"SI_UNIT\s*\(\s*\.MILLI\.\s*,\s*\.METRE\.\s*\)\s*\)\s*;\s*$",
    re.MULTILINE,
)
INSTANCE = re.compile(r"^#(\d+)\s*=", re.MULTILINE)


def bases(base_dir, manifest):
    """The base files, sorted."""
    if manifest is None:
        files = [p for p in base_dir.glob("*.stp") if not p.name.startswith("h_")]
        return sorted(files)
    cases = json.loads(Path(manifest).read_text())["cases"]
    names = set()
    for case in cases:
        if case["kind"] == "part":
            names.add(case["file"])
        elif case["kind"] == "match":
            names.add((case.get("candidate") or {}).get("file"))
    return sorted(base_dir / name for name in names if name and (base_dir / name).is_file())


def scaled(text, scale, tag):
    """`text` with its millimetre made `scale` mm, or None if it has no
    single SI millimetre to replace."""
    found = MILLIMETRE.findall(text)
    if len(found) != 1:
        return None
    unit = found[0]
    top = max(int(n) for n in INSTANCE.findall(text))
    measure, exponents, millimetre = (f"#{top + k}" for k in (1, 2, 3))
    replaced = MILLIMETRE.sub(
        f"{unit} = ( CONVERSION_BASED_UNIT('SCALED MILLIMETRE',{measure}) LENGTH_UNIT() "
        f"NAMED_UNIT({exponents}) );",
        text,
        count=1,
    )
    added = (
        f"{measure} = LENGTH_MEASURE_WITH_UNIT(LENGTH_MEASURE({scale!r}),{millimetre});\n"
        f"{exponents} = DIMENSIONAL_EXPONENTS(1.,0.,0.,0.,0.,0.,0.);\n"
        f"{millimetre} = ( LENGTH_UNIT() NAMED_UNIT(*) SI_UNIT(.MILLI.,.METRE.) );\n"
    )
    end = replaced.rindex("ENDSEC;")
    replaced = replaced[:end] + added + replaced[end:]
    # The header names the copy, so equal scales still differ in bytes.
    return replaced.replace("FILE_NAME('", f"FILE_NAME('{tag} ", 1)


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("base_dir", type=Path)
    parser.add_argument("out_dir", type=Path)
    parser.add_argument("count", type=int)
    parser.add_argument("--seed", type=int, default=5)
    parser.add_argument("--manifest")
    args = parser.parse_args()
    files = bases(args.base_dir, args.manifest)
    texts = []
    for path in files:
        text = path.read_text(encoding="latin-1")
        if len(MILLIMETRE.findall(text)) == 1:
            texts.append((path.stem, text))
        else:
            print(f"left out: {path.name}: not one SI millimetre", file=sys.stderr)
    if not texts:
        sys.exit("no base with an SI millimetre")
    args.out_dir.mkdir(parents=True, exist_ok=True)
    rng = random.Random(args.seed)
    # No base twice at one scale: each copy is another part.
    used = set()
    written = 0
    for n in range(args.count):
        stem, text = texts[n % len(texts)]
        scale = 1.0
        while n >= len(texts) and (stem, scale) in used:
            scale = round(math.exp(rng.uniform(math.log(0.5), math.log(2.0))), 6)
        used.add((stem, scale))
        out = scaled(text, scale, f"scaled {n}")
        (args.out_dir / f"{stem}-{n}.stp").write_text(out, encoding="latin-1")
        written += 1
    print(f"{written} copies of {len(texts)} bases in {args.out_dir}", file=sys.stderr)


if __name__ == "__main__":
    main()
