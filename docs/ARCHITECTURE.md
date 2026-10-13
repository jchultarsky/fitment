# Architecture

How fitment is put together, and the rules every change keeps. The design
of each part is in [PLAN.md](PLAN.md) and, for reading features (milestone
M2) and sockets (M3), in [research/m2-features.md](research/m2-features.md)
and [research/m3-socket.md](research/m3-socket.md); this page is the map.

## Crates

```text
fitment (CLI) ──> fitment-core ──> stepq (private: no type of it is public)
                        └────────> libm, serde, serde_json, thiserror
```

- **`fitment-core`** is the library: bytes in, values out. It does no I/O
  of any kind (files, network, processes, threads, clock, environment);
  clippy bans them, and `tools/check-core.sh` checks the dependency graph
  against `deny/core.toml`. It never panics on input: `unwrap`, `expect`
  and `panic!` are denied, and an invariant a step finds broken is an
  `internal` refusal, not a crash.
- **stepq stays behind walls.** Only `src/gate/`, `src/features/step.rs`
  and `src/assembly/step.rs` name it, which `tools/check-core.sh` checks;
  everything else works on fitment's own copy of the solid
  (`features/topo.rs`) and of the product structure (`assembly/`). A stepq
  release is then a one-crate change.
- **`fitment`** is the command line: it reads the file, calls the core,
  and prints. It never names stepq.

## Reading a part

```text
STEP bytes
  │ stepq: parse, build the instance graph            features/step.rs
  ▼
Gate, stages 1 to 3: file, product structure, shape   gate/mod.rs
  │   one product definition, one solid, nothing unread beside it
  ▼
Gate, stages 4 and 5: topology, units                 gate/solid.rs
  │   closed, connected shells; length, angle and uncertainty units
  ▼
Gate, stage 6: geometry
  │   every point, surface and curve read (check 22)  gate/solid.rs
  │   then copied into fitment's types (Topo)         features/topo.rs
  │   each a surface or curve, in range; vertices on  gate/geometry.rs
  │   their surfaces and curves within the stated
  │   uncertainty; faces oriented as they say
  ▼
Move into the part's frame (a mapped item's placement), range again
  ▼
Primary features: every face in exactly one           features/primary.rs, group.rs
  │   plane (with an exact outline)                    plane.rs, outline.rs
  │   cylinder (full or partial: rule F)               cylinder.rs, full.rs
  │   cone, torus, or other face                       other.rs
  ▼
End walks from every full cylinder's end circles      features/walk.rs
  ▼
Stacks: holes and shafts, with ends, rims, lengths    features/stack.rs
  ▼
features.v1 document (JSON)                           features/document.rs, readback.rs
```

A file the gate refuses produces a document with `status: refused` and its
reasons, never features. Each stage that refuses skips the later ones,
which need what it establishes.

## Reading an assembly

```text
STEP bytes
  │ stepq: product structure and placements           assembly/step.rs
  ▼
Assembly gate, checks 31 to 38: one root and no      gate/assembly.rs
  │   cycle, every usage placed and rigid, at most
  │   100,000 occurrences, no material in an
  │   assembly's own shape, components that pass M2's
  │   gate (36 refuses the whole assembly), frames and
  │   ranges; parts listed with their quantities and
  │   occurrences                                      assembly/parts.rs
  ▼
Selection: --part by id or name, --occurrence          assembly/mod.rs
  ▼
Each leaf definition read once, as a part (above)
  ▼
Per chosen occurrence, its neighbours in its frame:    socket/mod.rs, place.rs, index.rs
  │   search trees by box and along hole axes (N4)
  ▼
Axial rules: bores, shafts, continuations, unread      socket/axial.rs, rules.rs, block.rs
  │   occupants, partial cylinders, skew pairs
  ▼
Plane rules: seats, grips, conical seats, rings;       socket/planar.rs, overlap.rs, touch.rs
  │   contacts no rule reads (C6)
  ▼
Requirements per occurrence, then merged sockets       socket/combine.rs, merge.rs
  ▼
fitment.socket document (JSON), its digest, the        socket/model.rs, check.rs, review.rs
review and its carry-over
```

Every pair a search returns counts toward check 39 (at most 10⁷); past it
the socket is refused rather than read slowly. A document that breaks a
structural rule, even with its digest recomputed, is refused on reading
(`check.rs`).

## Soundness: refuse, or say unknown, but never guess

fitment may miss a match. It must never claim one. Reading features keeps
the same rule:

- **Refuse** what is not a canonical part, or states two things that
  cannot both be true (the contradiction checks). A refusal names the
  check and the `#id` at fault.
- **Say `unknown`** where a rule cannot decide: an outline with an unread
  edge, a cylinder bounded by B-splines (`undecided`), a hole end whose
  walk meets something the rules do not cover. `part.complete` is false
  when a cylinder is undecided or a hole or shaft has an irregular or
  unknown end; an unknown outline does not affect it, so a consumer checks
  each outline's status. M3 and M4 treat unknowns as unverified (design,
  section 9.6): a socket states what it cannot read as `unresolved`
  requirements, flagged unverifiable, which every candidate fails until a
  reviewer removes them.
- **Never state a wrong known value.** Every rule that was found to guess
  in review was withdrawn or made to say `unknown` instead: the seam proof
  (F3), a walk that turns back along its axis (W7b), a drill point after a
  flat floor. The design records each finding.

## Formats and versions

- **`features.v1`** (`FEATURES_FORMAT_VERSION`) is the document layout:
  frozen at the first publish (M4), changed freely before then, together
  with the extractor version.
- **`EXTRACTOR_VERSION`** rises with every change that could change any
  features output: a rule, a constant, an order, a field. A document
  records it, and the catalog (M5) re-reads every file whose document has
  another. **`SOCKET_RULES_VERSION`** does the same for sockets, and
  `fitment.socket` (`SOCKET_FORMAT_VERSION`) is frozen at the first
  publish like `features.v1`. Golden files can only be re-blessed when a
  version they record has risen (`FITMENT_BLESS=1`; never under `CI`).
- **The extraction constants** (`Extraction`, design section 5.2) are
  written into every document and are the ones the code uses: the struct
  takes its values from the modules' own constants.
- **`from_json` refuses a document that breaks what `read_part`
  guarantees** (the partition, sorted ids, closed outlines, classes and
  stations that agree, ends that fit their stack, references that exist),
  and every document fitment writes for the unit set, the corpus and the
  fixtures is checked to read back.

## Determinism

The same bytes give the same output bytes on Linux, macOS and Windows:

1. no hash order: `HashMap` and `HashSet` are banned in the core; every
   list is in a stated order (features by smallest face `#id`, faces by
   `#id`, loops outer first, segments in traversal order);
2. no platform maths: only `+ − × ÷` and `sqrt`, which are correctly
   rounded, and `libm` for trigonometry;
3. finite values only, and never `-0.0`: every stored number passes
   through `geom::finite`;
4. nothing from the environment in a document: no path, time, host or
   locale;
5. `serde_json` with `float_roundtrip`, pinned by `Cargo.lock`.

## Hostile input

A STEP file is untrusted. Beyond the gate's checks, every rule is meant to
be linear or `n log n` in the size of the file: reviews built files with
tens of thousands of faces sharing one edge, one plane, one cylinder or
one face full of holes, and each quadratic path they found was removed.
For sockets, `tests/hostile.rs` reads 10,008 bolts and refuses 36,008
coincident ones by check 39, and s55 is a plate of 3,600 coplanar tiles.
Coordinates beyond 1e6 mm are refused before any arithmetic depends on
them, and every bound is tested so that NaN fails it.

## Testing

| Layer | Where | What it shows |
| --- | --- | --- |
| Unit tests | `src/**` `#[cfg(test)]` | Rules on constructed solids, including the contradiction checks no valid file reaches |
| Unit set | `crates/fitment-core/tests/data`, `tests/parts.rs` | 24 generated parts (under 300 KB) checked against their construction on every OS |
| Review reproducers | `crates/fitment-core/tests/edits`, `tests/{planes,cylinders,holes}.rs` | Each input a review used to break a rule, with the answer it must give |
| Gate edits | `tests/features.rs` | Hand-edited STEP for the gate's checks, stage by stage |
| Unit assemblies | `crates/fitment-core/tests/assemblies`, `tests/assemblies.rs`, `tests/review.rs` | s01, s02, s03 and s08x, and the assembly gate's and review's checks as snippets and text edits |
| Hostile inputs | `tests/hostile.rs` | Assemblies grown from s08x: 10,008 bolts read, 36,008 coincident ones refused by check 39, 1,000 nuts on one axis |
| Generated corpus | `tools/gen-corpus.py`, `crates/fitment/tests/corpus.rs` | Parts and assemblies with answers from the construction alone: parts also with pcurves (`_pc`) and in a skew frame (`_rot`); every assembly with sockets also under a skew root (`_rot`), with its targets in a tilted frame (`_tgt`), with pcurves (`_pc`) and with its components in reverse order (`_perm`) |
| Goldens | `crates/fitment/tests/golden` | The exact bytes of each unit part's `features` document, and of each unit assembly's `parts` and `socket` documents; on three operating systems, the determinism check |
| Fixtures | `tools/fetch-fixtures.sh`, `fixtures/manifest.json`, `tests/fixtures.rs` | 45 public STEP files: pinned gate results and counts of planes, outlines, cylinders, holes and shafts; sockets pinned for chosen targets of the assemblies, and AS1's exports compared |
| Oracle | `tools/verify-occt.py` | Open CASCADE 8.0.1, a kernel the generator does not use: `manifest` checks the construction; `features` checks fitment's planes, cylinders, holes and shafts; `socket` checks every socket against the solids, both ways (spans, seats, touchers, grips, floors, touching faces, occupied holes), with the fixtures' tallies pinned; `mutations` edits fitment's sockets nine ways and requires each edit to meet a disagreement |

The test parts' answers come from the generator's arguments, never from
reading a written file; the oracle catches a generator whose part is not
what its construction says. The oracle has its own blind spots, listed in
its docstring: what both kernels would misread the same way is covered by
fitment's own tests.

## Where things are decided

- Canonical gate, rules G, S, F, W, K, E, the JSON format, the test parts:
  [research/m2-features.md](research/m2-features.md).
- Assembly gate, sockets, rules T, O, P, Z, N, X, B, I, J, H, Q, C, L, U,
  Y, A, V, review, the test assemblies and the socket oracle:
  [research/m3-socket.md](research/m3-socket.md).
- Milestones, matching method and verdicts: [PLAN.md](PLAN.md).
- stepq's reading of STEP: its own repository and
  [research/m1-stepq-read.md](research/m1-stepq-read.md).
