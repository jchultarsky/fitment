# Feasibility spike

Throwaway code, kept as a reference for M1 and M2. It is not part of the
build. What it showed, and what it missed, is in
[../PLAN.md](../PLAN.md#spike-results).

To rerun it:

1. `python3 gen.py step/` writes the test assembly and four candidates
   (needs [cadquery](https://cadquery.readthedocs.io/)).
2. Build `spike.rs` as `src/main.rs` of a scratch crate that depends on
   `stepq = { version = "0.4.1", default-features = false }`.
3. Run it with `step/` as its argument.
