# The test corpus

fitment is tested on parts it generates, so that every expected answer
comes from how a part was built, never from fitment's own output.

- `tools/gen-corpus.py corpus/out` writes every case into `corpus/out`
  (ignored by git), with `manifest.json`. CI does this in its `corpus` job
  and passes the result to the test job on every OS.
- `tools/gen-corpus.py --unit-set DIR` writes only the unit set, the cases
  committed in `crates/fitment-core/tests/data`.
- `tools/verify-occt.py manifest corpus/out` checks every part with a
  second kernel against the manifest.

The design is `docs/research/m2-features.md`, section 13.

## manifest.json

```text
format          "fitment.test-parts"
format_version  1
generator       the script and the cadquery and cadquery-ocp versions it ran on
tolerance       linear_mm 1e-6, angular_rad 1e-9: how close a read value must be
cases           one per part, sorted by id:
  id, file      the case and its .stp
  set           "unit" (committed) or "corpus" (generated in CI)
  tests         one sentence: what the case is for
  rules         the design's rule and check ids it exercises
  construction  the arguments it was built from
  expect        {"result": "accepted", "features": {...}}
                or {"result": "refused", "stage": N, "refusals": [codes]}
```

Expected features are stated in the design's convention (section 9):
a hole's axis points from its bottom to its opening when blind, and has
its largest component positive otherwise; `axis_point` is the axis point
at the start end; a segment is `[diameter, from, to]` in mm along the axis.
A hole whose ends the rules cannot read has `"kind": "unknown"` with only
its axis line and diameter. `"planes": null` means the planes are not
stated yet.

## What counts as a false positive

From M4, a false positive is `Match` on any known-bad case, or any pass on
a bad interface; the count must be zero (docs/PLAN.md, Soundness rules).
M2's cases test reading, not matching.
