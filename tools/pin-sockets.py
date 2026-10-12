#!/usr/bin/env python3
"""Pin the sockets of the assembly fixtures' chosen targets
(docs/research/m3-socket.md, 14.3): regression pins, not truth.

    tools/pin-sockets.py            print what the built fitment gives
    tools/pin-sockets.py --write    write it into fixtures/manifest.json

Re-pin only in a PR that says why. The summary is the one
crates/fitment/tests/fixtures.rs checks: status, occurrences, sockets after
merging, requirements by kind, spans by fit, features left out and
unverifiable requirements (the CLI warns once for each).
"""

import json
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
FITMENT = ROOT / "target/release/fitment"

# The targets of section 14.3, by fixture.
AS1 = {
    "steptools/as1-ac-214.stp": ["PLATE", "L-BRACKET", "BOLT", "NUT", "ROD"],
    "steptools/as1-md-214.stp": ["PLATE", "LBRACKET", "BOLT", "NUT", "ROD"],
    "steptools/as1-ug-214.stp": ["plate", "l_bracket", "bolt", "nut", "rod"],
    "steptools/as1-ec-214.stp": ["as1 1.2", "as1 1.1.1", "as1 1.1.2.2", "as1 1.1.2.1", "as1 1.4.1"],
    "steptools/as1-tc-214.stp": ["plate", "l-bracket", "bolt", "nut", "rod"],
    "nist-edm/as1_pe.stp": ["PLATE", "L-BRACKET", "BOLT", "NUT", "ROD"],
}
OTHERS = {
    "nist-edm/weldment_asm_solid.stp": ["SPACER_220", "BEARING_208", "NUT_244", "LINKAGE_ARM_SHORT_46"],
    "nist-edm/moon_buggy_asm.stp": ["ph1m2-ug"],
    "nist-edm/vaccase_asm_solid.stp": ["TEE"],
}


def summary(path, target):
    run = subprocess.run([str(FITMENT), "socket", str(ROOT / "fixtures" / path), "--part", target],
                         capture_output=True, text=True)
    document = json.loads(run.stdout)
    extracted = document["extracted"]
    if extracted is None:
        return {"status": "refused"}
    kinds, fits, left, unverifiable = {}, {}, 0, 0
    for socket in extracted["sockets"]:
        left += len(socket["left_out"])
        for requirement in socket["requirements"]:
            kind = requirement["requires"]["kind"]
            kinds[kind] = kinds.get(kind, 0) + 1
            for span in requirement["requires"].get("spans", []):
                fits[span["fit"]] = fits.get(span["fit"], 0) + 1
    unverifiable = run.stderr.count("no candidate can be checked against it")
    return {"status": "accepted", "occurrences": len(extracted["occurrences"]), "sockets": len(extracted["sockets"]),
            "requirements": dict(sorted(kinds.items())), "fits": dict(sorted(fits.items())), "left_out": left,
            "unverifiable": unverifiable}


def main():
    if not FITMENT.is_file():
        sys.exit("build fitment first: cargo build --release -p fitment")
    manifest_path = ROOT / "fixtures/manifest.json"
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    by_path = {f["path"]: f for f in manifest["fixtures"]}
    for path, targets in {**AS1, **OTHERS}.items():
        pins = {target: summary(path, target) for target in targets}
        print(path, json.dumps(pins))
        by_path[path]["sockets"] = pins
    if "--write" in sys.argv[1:]:
        # The manifest's own layout: one line per fixture.
        lines = ["{", f' "comment": {json.dumps(manifest["comment"])},', f' "oracle": {json.dumps(manifest["oracle"])},',
                 ' "fixtures": [', ",\n".join("  " + json.dumps(f) for f in manifest["fixtures"]), " ]", "}"]
        manifest_path.write_text("\n".join(lines) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
