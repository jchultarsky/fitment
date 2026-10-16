# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).
Until 1.0, minor versions may contain breaking changes.

## [Unreleased]

## [0.1.0] - 2026-10-16

The first release: milestones M0 to M4. `fitment-core` is published first,
then `fitment`. The four document formats below are frozen at version 1.

### Added

- **M1 (in stepq 0.6):** units, geometry values, placements, topology, the
  solids of a definition and PMI to faces, read from STEP.
- **M2, features of one part:** `fitment features part.stp` and
  `read_part`: one part through the canonical gate (refused with the check
  and the `#id` at fault), its planes with exact outlines, cylinders,
  cones, tori, holes and shafts with their ends, and every face no rule
  reads; `features.v1`.
- **M3, sockets:** `fitment parts asm.stp` lists an assembly's parts
  (`fitment.parts` v1); `fitment socket asm.stp --part X` reads what the
  neighbours of a part require of any part that replaces it: bores and
  shafts by span with their fits, seats with their contact outlines,
  grips, conical seats, what is left out, and what no candidate can pass
  until a reviewer removes it; `fitment review` records each decision
  outside the digest, and a document of older rules carries its decisions
  over (`fitment.socket` v1).
- **M4, matching:** `fitment match socket.json candidate.stp|dir/...` and
  `Matcher`: a candidate's signature in rigid invariants, a shortlist that
  never drops what verification passes, alignment by anchor pairs (proper
  rotations only), and verification of every requirement with its
  measures and coded failures: axes, diameters by span, walls, openings,
  the clear core, seats covered, grips, conical seats. Verdicts `Match`,
  `InterfaceMatch` and `Reject`, built only by `decide`; `Match` needs a
  verified body fit, so without M7's verifier the best is
  `InterfaceMatch`. Policies: D9's 0.01 mm and 0.01°, strict or
  functional clearance (D6), hard grips (D7). Exit codes 0, 4 and 5 by the
  best verdict; 1 an error, 2 usage, 3 a socket refused. The
  `fitment.match` v1 document, in a fixed order with a digest the same on
  every OS. Bounded work on hostile files: past `MAX_PLACEMENTS` or
  `MAX_COVERAGE_STEPS` the requirement fails (`alignment_exhausted`,
  `coverage_exhausted`); past `MAX_SHORTLIST_CHECKS` the candidate is
  kept, never dropped.
- **The known-answer corpus** (`tools/gen-corpus.py`): 125 match cases and
  four hostile inputs, with truths from the construction, judged under both
  policies through the library and the binary; the false-negative
  baseline; the rates: no candidate passed whose interface does not fit
  (a colliding body is M7's); against physics, 20 of 49 fits rejected
  under `default` and 21 of 51 under `functional`, each by a stated rule
  (four of the 49 are grip-only thickness changes, fits by M4-8's
  convention that neighbours reseat).
- **The Open CASCADE oracle** (`tools/verify-occt.py`, Open CASCADE 8.0.1,
  not the generator's kernel): the truths it can judge checked physically
  (`fits`, `does_not_fit`, `fits_functional`, `collides`; not `edge`,
  `refused`, originals read in place, or grip-only thickness changes);
  every pass fitment makes fits at the placement it states; every
  failure re-measured; mutations of fitment's documents each met.

### Fixed during M4's reviews

- Alignment named its placement bound when reached: before, the
  placement past it was never pushed, so `alignment_exhausted` never
  fired.
- `ShortlistIndex` (level 1) asked a pair token between two coaxial
  required lines one candidate stack may serve together (SG5), so it
  dropped a part the shortlist (level 2) keeps; found by M5's design
  review before any run relied on level 1.

### Earlier (M0)

- Project plan (`docs/PLAN.md`) and the feasibility spike (`docs/spike/`).
- The Vectera reuse audit (`docs/vectera-audit.md`) and decision D1 on
  body-fit checking, with its research (`docs/research/d1-body-fit.md`).
- Repository scaffolding: license, contributing guide, code of conduct,
  security policy, CI, and issue and pull request templates.
- Cargo workspace with `fitment-core` (the matcher library) and `fitment`
  (the CLI, which so far only prints its version and help).
- `fitment-core`: `Verdict`, built only by `decide()`; `FitStatus` with the
  JSON shape fixed by D1; `TolerancePolicy` with D9's defaults, refusing
  values that cannot be compared against.
- CI checks that `fitment-core` stays free of I/O and nondeterminism: an
  exact dependency allowlist (`deny/core.toml`), checked alone and against
  the workspace-wide graph; source bans (`crates/fitment-core/clippy.toml`);
  and `tools/check-core.sh` for banned std modules, async code and global
  state. Open CASCADE and C++ bridge crates are banned workspace-wide (D1).
