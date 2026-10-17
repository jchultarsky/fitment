# fitment (Part Substitution Finder): Project Plan

As of 2026-10-12 · Julian Chultarsky
Living copy of this plan: https://claude.ai/code/artifact/b5f9353c-62ad-4f43-868d-9b7b5eebbb27

Given an assembly and one part in it that is no longer available, the system suggests catalog parts whose interfaces match. It is named `fitment`, will be published as open source, and is deterministic, written in Rust and built on stepq. fitment reads the features of one part from a STEP file (M2): `fitment features part.stp` gates the file as canonical or refuses it with reasons, and prints its planes, cylinders, holes and shafts as `features.v1` JSON. It reads the socket of a part in an assembly (M3): `fitment socket asm.stp --part X` writes what the part's neighbours require of any replacement as a reviewable `fitment.socket` JSON file, and `fitment review` records the user's decisions in it. Matching is next. A throwaway spike confirmed that holes, placements and a socket can be read with stepq's public API.

## Resume here

M0 was completed on 9 Oct 2026, M1 on 11 Oct 2026, M2 and M3 on 12 Oct 2026, M4 on 16 Oct 2026 and M5 on 17 Oct 2026: `fitment features` reads one part, `fitment socket` reads a part's socket from an assembly, `fitment match` judges candidates against it, and `fitment catalog` stores parts and `fitment match --catalog` searches them, as designed in [research/m2-features.md](research/m2-features.md), [research/m3-socket.md](research/m3-socket.md), [research/m4-match.md](research/m4-match.md) and [research/m5-catalog.md](research/m5-catalog.md) and mapped in [ARCHITECTURE.md](ARCHITECTURE.md). Version 0.1.0 is ready to publish from commit af38965 on main, the last code before M5 (M4's exit with the level-1 fix its CHANGELOG lists; `fitment-core` first, then `fitment`), and 0.2.0 from M5's release PR once merged (`fitment-core`, then `fitment-catalog`, internal and unstable, then `fitment`). Tag each `v0.1.0` and `v0.2.0` at the commit published. Work continues at M6, the REST API, whose design is not yet written. M4's resume prompt, kept for its reading list (open this repository in Code mode, with ../stepq added):

```text
Read docs/PLAN.md, sections "Decisions made", "How it works", "Soundness rules",
"Choosing the part to replace", "Matching method" (steps 2 to 7 and its
paragraph on tolerances), "Metadata from STEP only", decisions D6 to D10 and
"Milestones"; docs/ARCHITECTURE.md; docs/research/m3-socket.md sections 1 and 2
(what the socket records and what M4 decides), 6.2 (the floor), 7.8 and 7.10
(seat regions, unknown outlines, the causes of unresolved requirements), 8
(what each requirement states, Y4 and the envelope), 9 (the socket document,
its review and carry-over), 11 (the API), 13 and 14 (the generator, the
manifest, the RULES registry and the oracle) and 18 (risks, and what it hands
to M4); and docs/research/m2-features.md sections 7.4 and 9.6, which bind the
candidate side (unknown outlines, unknown ends, undecided cylinders). We are at
fitment's milestone
M4: signature, alignment, verification and evidence; `fitment match
socket.json candidates/*.stp`; the known-answer corpus and its harness.
1. Propose the signature, alignment, verification and verdict types and the
   corpus of good and bad candidates in docs/research/m4-match.md, then stop
   for my review. The original part, run as a candidate against its own
   socket, must fail exactly its unverifiable requirements (Y4, deferred to
   M4 in RULES); M4's rules join the RULES registry and m4-match.md joins the
   coverage test.
2. Implement them one PR at a time, each with tests and two adversarial
   reviews, and publish fitment at the exit.
Done when there are zero false positives on the known-bad set and the
false-negative rate is reported. A false positive is `Match` on any bad case,
or any pass on a bad interface. `InterfaceMatch` on the colliding-body case is
correct until M7.
```

The project is named `fitment` and will be published as open source. stepq is at [github.com/jchultarsky/stepq](https://github.com/jchultarsky/stepq); it was at 0.4.1 when this plan was written and is at 0.6.0 since M1.

## Decisions made

These were settled in the planning conversation and should not be reopened without a reason.

| # | Decision | Consequence |
| --- | --- | --- |
| 1 | Deterministic, no machine learning. Comparison within a tolerance is fine. | Rules and arithmetic only; coverage equals the rules written. |
| 2 | Rust, reusing stepq as much as possible. stepq may be extended; its "no geometry" rule was scope control. | Geometry reading goes into stepq (M1). |
| 3 | False positives are the failure to avoid. Some false negatives are acceptable. | Unknown means reject; see Soundness rules. |
| 4 | Catalog parts and assemblies are both canonical. | Enforced by a gate at ingest, not assumed (D2). |
| 5 | The matcher is its own crate, standalone or part of the new project. | A library with no storage or network code. |
| 6 | A catalog of STEP models with full create, read, update and delete. | Catalog crate with a feature index. |
| 7 | An API, probably REST, to operate the system. | API crate over catalog and matcher. |
| 8 | No other data sources. Any metadata comes from the STEP file as properties. | See Metadata from STEP only. |
| 9 | Vectera is checked for reuse, but the system must be buildable without it. | Audit in M0 ([vectera-audit.md](vectera-audit.md)); nothing becomes a dependency. |
| 10 | D1: `Match` requires a verified body fit. Without a fit verifier the best verdict is `InterfaceMatch`. The M7 verifier runs Open CASCADE in a separate helper process (Python, pinned through uv), and official release artifacts never bundle Open CASCADE while [OCCT #1564](https://github.com/Open-Cascade-SAS/OCCT/issues/1564) is open. | Fit is a first-class field of every verdict. Published crates stay pure Rust and MIT. Research: [research/d1-body-fit.md](research/d1-body-fit.md). |
| 11 | Workspace layout, as proposed in [research/workspace-layout.md](research/workspace-layout.md) with every recommendation in its section 10 accepted. | See below. |

Decision 11 in brief (details and reasons in the layout document):

- **Crates:** `fitment-core` (library) and `fitment` (CLI) now; `fitment-catalog` at M5, `fitment-server` and the test-only `fitment-corpus` at M6, `fitment-fit` at M7. All published crates share one version.
- **Rust 1.85**, matching stepq. No nalgebra, whose recent versions need 1.87 or later; the small amount of 3D maths is written by hand.
- **stepq from crates.io:** the layout planned a pinned git commit through `[patch.crates-io]` until stepq was published. stepq 0.6.0 was published at the end of M1, before M2, so fitment depends on `stepq = "0.6"` from the start and the patch is never needed.
- **First publish at M4 exit**, accepting the risk to the `fitment` name until then.
- **Errors:** `thiserror` in libraries, `anyhow` only in binaries and tests, in fitment and stepq alike.
- **`fitment-core` is not `no_std`;** clippy bans and the dependency allowlist enforce its rules.
- **A small set of generated unit parts is committed** (M2, at most 300 KB, excluded from packages) so extraction tests run on every OS without Python.
- **Strict corpus gate:** a bad case must be rejected for its injected defect, and any change in the false-negative set fails until the baseline is re-blessed.
- **Two kernels in tools:** the corpus generator may use cadquery's Open CASCADE 7.9; the oracle and the M7 helper pin 8.0.1.
- **The server is its own package**, shipped as a container image without uv or Open CASCADE.
- **CLI features:** `catalog` is on by default from M5; `fit` is compiled into release binaries from M7, which embed only the helper script, so no Open CASCADE ships.

## How it works

A candidate is tested against a socket derived from the assembly, never against the original part.

```text
INGEST, once per catalog part            QUERY, once per part to replace

Catalog part (STEP)                      Assembly and the part to replace
  added, replaced or removed               one STEP file; the user picks the part
        |                                        |
        v                                        v
Gate and feature extraction              Socket extraction
  canonical files only;                    what the neighbors require;
  the rest quarantined                     user reviews
        |                                        |
        v                                        v
Feature index  ----------------------->  Shortlist
  hole sizes, spacings and                 index lookup; no STEP file is re-read
  planes per part                                |
                                                 v
                                         Align and verify
                                           every requirement checked, else rejected
                                                 |
                                                 v
                                         Verdict with evidence
                                           match, interface match, or reject with reasons

stepq underneath: parsing, assembly structure, placements, geometry values, units, properties
```

The socket is what the neighbors require of the part: holes on given axes with diameter limits, seating planes, and the space the part may occupy. The original part is only one example that fits it, so a candidate with extra holes or a different body can still pass.

Ingest runs once per catalog part and fills the index. A query extracts the socket, takes a shortlist from the index, then places each shortlisted candidate in the socket and checks every requirement. As built in M5, the catalog holds two stores: every file by its SHA-256, and a database of what was derived from it (the features document, level 1's tokens and postings, the stated properties). A query reads the database alone, never a stored file, and its result is the one a file run over the same files gives.

## Soundness rules

These seven rules are how "no false positives" is enforced. Every design choice below follows from them.

1. A candidate passes only when every socket requirement is positively verified.
2. Anything the code does not recognize is a rejection: a spline surface on an interface, an unreadable unit, a feature with no rule.
3. Socket extraction errs toward more requirements. A missed requirement becomes a false positive; an extra one becomes a false negative the user can remove in review.
4. Verification errs toward rejection. A value at the edge of the tolerance rejects.
5. Every result states what was checked and what was not. "Fit not checked" and "thread not confirmed" are part of the verdict.
6. The catalog indexes only files that pass the canonical gate. The rest are quarantined with a reason.
7. The false-positive count is measured on a corpus of known-bad candidates and must be zero before the catalog and API are built (M4).

## Components

One Cargo workspace grows one crate per milestone on top of stepq (decision 11); the fit verifier and the viewer are optional later additions.

| Component | Where | Responsibility | Reuse |
| --- | --- | --- | --- |
| STEP reading | stepq (extended) | Parsing, product structure, placements, geometry values, units, properties, PMI | Exists; six additions in M1 (0.6.0) |
| Matcher | `fitment-core` (M0) | Canonical gate and features (M2), socket (M3), signature, alignment, verification, evidence; a `FitVerifier` trait. No storage, no network, enforced by CI. The only crate that depends on stepq. | New |
| CLI | `fitment`, binary `fitment` (M0) | `features`, `parts`, `socket`, `review`, then `match`, later `catalog`. Proves the matcher before any server exists. | stepq's CLI pattern |
| Catalog | `fitment-catalog` (M5), synchronous | File store, feature index, applies the canonical gate (built in M2, in `fitment-core`), CRUD, re-index when the extractor version changes. As built: SQLite in WAL mode and a content-addressed blob store; `fitment catalog init`, `ingest`, `list`, `get`, `replace`, `delete`, `prune`, `reindex`, `gc` and `verify`, and `fitment match --catalog` | Vectera's lessons only |
| API | `fitment-server` (M6), the only async crate | REST over catalog and matcher. Ingest and matching run as jobs. | Vectera's auth, ported |
| Fit verifier | Optional crate, pure Rust, plus a helper script | Body clearance against the neighbors. The crate implements `FitVerifier` and drives a pinned Open CASCADE helper in a separate process; a crash, timeout or warning there means "not verified", never a pass | New; stepq's Open CASCADE oracle stack (D1) |
| Viewer | Later | Shows the socket and a candidate in 3D. Needs tessellation, which stepq will not do. | Check Vectera |
| Test corpus | Dev-time scripts | Generated good and bad candidates with known answers | stepq's Open CASCADE oracle pattern |

The catalog keeps two things per part: the STEP file, addressed by its content hash, and the extracted features with the extractor version. A query reads the index and never re-parses a STEP file.

Assemblies, the chosen part, the extracted socket and the user's corrections to it are stored too, so a match can be re-run and audited.

## Changes to stepq

stepq 0.4.1 read the file and the assembly structure but interpreted no geometry. Six read-only additions close that gap, following the accepted design, PR by PR, in [research/m1-stepq-read.md](research/m1-stepq-read.md). The design planned five for stepq 0.5 and PMI to faces (row 5) for 0.6; all six were merged before either was released, so they shipped together as stepq 0.6.0 on 11 Oct 2026, and 0.5.0 was never published. The PRs are stepq #43, #45, #46, #47, #50 and #49, in the order units, geometry values, placements, topology, solids of a definition, PMI to faces.

| # | Addition | Today in 0.4.1 | Needed |
| --- | --- | --- | --- |
| 1 | Placement transforms | `model::Placement` holds only `#id`s; its doc says no transformation is evaluated | A rigid transform from each `axis2_placement_3d`; `item_defined_transformation` as M(assembly-side item) · M(component-side item)⁻¹, the component side decided by the usage, not the relationship's order (refused where ISO and Open CASCADE disagree); `mapped_item` origin to target; composed from root to every occurrence |
| 2 | Geometry values | Only `Literal::to_f64` | Typed readers for `cartesian_point`, `direction`, `axis2_placement_3d`, `plane`, `cylindrical_surface`, `conical_surface`, `toroidal_surface`, `circle`, `line`; every other surface reported as "other" |
| 3 | Topology walk | None | Solid → shell → `advanced_face` with `same_sense` → bounds → loops → oriented edges → `edge_curve` → `vertex_point`; face adjacency through shared edges; `brep_with_voids` handled or refused |
| 4 | Unit scale | `info::Units` gives names only | Numeric factor to millimetres per representation context (SI prefix, `conversion_based_unit`), plus each context's `uncertainty_measure_with_unit`, in its own unit |
| 5 | PMI to faces | `pmi` resolves a tolerance to a shape aspect and its product | Follow `geometric_item_specific_usage` to the faces a tolerance or dimension applies to |
| 6 | Solids of a definition | `Definition.shape_representations` lists `#id`s | A helper that returns the solids, following a simple `shape_representation_relationship` when the solid sits in a second representation |

The rule to keep, reworded: stepq reads what the file states and never computes what it does not. A radius, an axis and a placement are stated. Intersections, trimmed areas, volumes and tessellation are not, and stay out.

None of this changes output: the additions only read, so unchanged entities are still written byte for byte. No new dependency is needed.

The documents that stated the old rule were edited in M1: `README.md` ("What it will not do"), `CLAUDE.md` (hard rules), `docs/ARCHITECTURE.md` (non-goals) and `ROADMAP.md` ("Not planned").

Test oracle, as elsewhere in stepq: compare the placements and cylinder axes stepq reads with what Open CASCADE reports for the same fixtures, by extending `tools/verify-occt.py`.

## Choosing the part to replace

The user names a part by its part number, and every occurrence of it is replaced. This was the recommended default (D11), confirmed by M3's design (section 5).

- `fitment parts asm.stp` lists the parts in an assembly: id, name and quantity, read from `PRODUCT.id`, `PRODUCT.name` and the usage occurrences.
- `--part <id>` selects by `PRODUCT.id`, usually the part number, with the name as a fallback, or by a definition's `#<n>`. A value that matches more than one part is refused with them listed, and one that matches none with any similar parts suggested.
- A part used several times has one socket per occurrence. A candidate must fit every one. Sockets that are identical apart from their position are merged.
- `--occurrence <path>` narrows the choice to one occurrence, addressed by its path through the assembly tree.
- The first version accepts only a leaf part, not a sub-assembly.
- In the API the assembly is stored by its content hash, so the `#id`s of the chosen part and its occurrences stay valid and are stored with the socket.

## Matching method

A candidate is never compared with the original part. It is tested against the socket, in seven steps.

1. **Features of one part**, read without a kernel:
    - Edge-connected faces with the same sense on `cylindrical_surface`s with the same axis line and radius (within 1e-6 mm) are one cylinder, whether they share one surface entity or not; coaxial cylinders that do not touch stay separate. `face_sense` (`same_sense` composed with the shell's orientation) false marks a hole and true a shaft, because a cylinder's surface normal points away from its axis and a face normal points out of the material; the gate checks it against the loops wherever they decide it (a whole shell written inside out passes, R11 in the M2 design).
    - Only a full circle is a hole. The arcs at each end must sum to 360°; a partial concave cylinder is a fillet or a slot end.
    - The extent along the axis comes from the centres of the stated end circles. It gives the depth, and what closes the end gives through or blind.
    - Two coaxial radii joined by a flat ring are a counterbore. A coaxial `conical_surface` is a countersink or a drill point.
    - A plane is kept with its outward normal and its outline from the loop vertices and each edge's line or circle, exactly.
    - Every other surface is recorded as "other" with its type.
2. **Socket from the assembly.** Every occurrence is placed in the assembly frame, then the target part is tested against each neighbor:
    - Parallel, with cross-sections meeting over an axial overlap, neighbor shaft smaller than the target hole: a clearance fit, its bound the shaft's diameter plus twice the offset. The hole must be at least that bound and, under the strict policy (D6), no larger than the original.
    - Coaxial and overlapping, neighbor shaft larger than the hole: a threaded or press fit, inferred. The hole diameter is pinned.
    - Coaxial, not overlapping, neighbor hole: the hole continues into the neighbor, a bolted joint. The axis is required, also beyond a gap, unless a plane face of known outline blocks the line between them; with no fastener read, the hole is pinned.
    - Coaxial, not overlapping, a larger neighbor cylinder ending at the target's face: a head or shoulder bears there. A seating plane and a clear ring around the hole are required.
    - Coplanar faces with opposed normals whose outlines overlap: a seating plane at that offset. Two opposed seating planes on one axis fix the grip length.
    - A target hole with nothing on its axis is left out and listed for review; never while an unread face could be in it, and material that is not a read shaft pins the hole.
    - The space the target occupies is kept as the envelope for the fit verifier.
    - The socket is written as a JSON file the user reviews: each requirement can be kept or removed with a note; values cannot be edited.
    - Unread faces on the target or a neighbour, and contacts no rule reads, are `unresolved` requirements that fail until removed in review.
3. **Signature.** Per part, rigid invariants: its stacks (diameters, extents, ends), the distances and angles between its axes, plane-to-axis offsets and seats' areas, in policy-free bins of 0.1 mm looked up with the bins either side, so a value on an edge is never missed and the index does not depend on the tolerance (SG, M4's design).
4. **Shortlist.** A necessary condition with a proof: a candidate is dropped only where no placement could pass, so the shortlist never changes a verdict (SL1, checked on the corpus). Extra holes on a candidate are allowed.
5. **Alignment.** The candidate sits in its own frame. Anchor pairs (two axes, an axis and a seat, landmarks) give placements, ranked, not filtered, at most 4,096 per socket (AL11); proper rotations only, so a mirror image never aligns. One placement per socket is chosen: the first that passes every requirement, or the one with fewest failures.
6. **Verification.** Under the placement every requirement must find a candidate feature within tolerance less a guard band: axis position and angle, diameters within their limits, walls over every span, openings where required, the clear core of each bore free of material (VR22), seating planes present and covering the contact outline (certified coverage), grip length equal, conical seats on their apex and angle.
7. **Verdict.** `Match`: every requirement and the fit are verified. `InterfaceMatch`: every requirement is verified and the fit is not checked or could not be verified. `Reject`: with the failed requirements listed, or a body that interferes. Flags ride along, such as "thread not confirmed" and "material not compared".
    - Fit is its own field: `verified` (with the kernel and its version), `interferes` (with evidence), `not_verified` (with a reason) or `not_checked` (with a reason). Only `verified` can make a `Match`; `interferes` makes a `Reject`.
    - Without a verifier, the CLI says `InterfaceMatch: interface verified; body fit not checked (no fit verifier in this build)`.
    - `Match`, `InterfaceMatch` and `Reject` have separate exit codes: 0, 4 and 5 (an error 1, usage 2, a refused socket or a search that could not run 3: a stale or empty catalog, a blocked socket). `--require-fit` treats `InterfaceMatch` as a failure.

Tolerances are one linear and one angular value, held in a named policy stored with each result. The floor is split (M4-2): comparisons with the original's own geometry use the parts' own stated uncertainty, and the floor of a relation, the largest stated uncertainty of the contexts it involves including the placement path's (R17 in M3's design), bounds only limits derived from neighbours. A requirement whose tolerance leaves nothing above the floor fails every candidate (`floor_above_tolerance`). Uncertainty is stated per representation context and varies by exporter (Pro/E's "closure" reaches 0.672 mm). Lengths are converted to millimetres before any comparison.

## Metadata from STEP only

The system has no data source besides the STEP files, so every non-geometric fact comes from `stepq::props` and `stepq::pmi` or is reported as unknown.

- **Availability** is not in a STEP file. Catalog membership stands in for it: a part in the catalog is available, and withdrawing a part is a delete. The user picks the part to replace.
- **Threads** are usually not modeled, and a tapped hole looks like a plain one. The socket infers "threaded or press fit" when a neighbor's shaft is larger than the hole it sits in. The candidate must then have the same hole diameter, and the verdict carries "thread not confirmed" unless a property or PMI on both parts states the thread. Seen on real data (M5): `nist_ctc_04`'s M12x1.75 callout sits on a hole modelled as a plain Ø10.2 cylinder, the tap drill.
- **Material, finish and ratings** are compared only when both files carry the property. Otherwise the result says "not compared".
- **Property names** differ between exporters, so the catalog needs a small mapping from property names to fields, built from real sample files (D10). As built in M5: the catalog captures each part's own stated properties and shows them with `fitment catalog get`; one mapping row ships, and no fixture states a material or a thread in a form fitment can use.
- **Declared volume, area and centroid** are sanity checks at ingest, never match criteria.

The limit to state plainly to users: without properties, a geometric match cannot tell a tapped hole from a plain one of the same diameter.

## Spike results

A 240-line Rust program on stepq 0.4.1's public API, with no changes to stepq, found the holes, placed the parts, derived a socket and judged four candidates correctly at the level it checks.

The test assembly was generated with cadquery on Open CASCADE 7.9: a plate with four Ø5 holes, a bracket with four Ø6.6 holes, one Ø8 hole and a boss, and four bolts with Ø6 shafts and Ø10 heads. The four candidate brackets were each written in a rotated and shifted frame.

What it confirmed:

- `same_sense` separated holes from shafts for every cylinder in the three parts.
- Placements read from `item_defined_transformation` put the four bolts at (±30, ±15, 0), as modeled.
- The socket held the four bolted holes with the right reasons: Ø6 shaft passing with clearance, hole continuing as Ø5 in the plate, Ø10 head bearing on the face. The Ø8 hole was excluded. Seating planes were found at both faces.
- Candidates: same pattern in another frame passed; one hole 0.5 mm off, holes of Ø5.5 and a missing hole were each rejected with the right reason.

What it exposed:

1. Coaxial is not enough. The first version read the bolt head, which sits above the bracket, as an oversized shaft inside the hole. Overlap along the axis must be tested. This is now in the method.
2. The passing candidate is 6 mm thick against the original's 8 mm. The spike ignores seating planes, so it passed; with the grip-length requirement it must be rejected. The corpus needs a good candidate of the right thickness.
3. Open CASCADE writes a full cylinder as one face with a seam. Exporters that write two half faces are handled by the grouping step, which this test did not exercise. M2 found that most exporters write split faces, and grouping handles them ([m2-features.md](research/m2-features.md), section 8.1, G4).

Not covered: nested assemblies, `mapped_item` placements, unit scale, other exporters, counterbores, blind holes and outline overlap of planes.

The spike is throwaway code. Its files, `spike.rs` and `gen.py`, are a starting reference for M1 and M2. To rerun it: `python3 gen.py step/` (needs cadquery), then build `spike.rs` as `src/main.rs` of a crate that depends on stepq with `default-features = false`, and run it with `step/` as its argument.

## Milestones

M2 to M4 carry all the technical risk, so the catalog and API (M5, M6) do not start until M4's exit test passes.

| # | Delivers | Done when |
| --- | --- | --- |
| M0 | Repository and workspace, this plan in `docs/`, the Vectera audit, decision D1 | The audit is written and D1 is recorded |
| M1 | stepq 0.6: units, geometry values, placements, topology, solids of a definition, PMI to faces (six PRs; [design](research/m1-stepq-read.md)). Done 11 Oct 2026 | stepq's placements and cylinder axes equal Open CASCADE's on the fixtures; the four documents are updated; stepq 0.6.0 is published |
| M2 | Matcher crate: features of one part; `features part.stp` prints JSON ([design](research/m2-features.md)). Done 12 Oct 2026 | Holes, shafts, depths and planes are correct on generated parts with known features; non-canonical input is refused with a reason |
| M3 | Socket extraction and the reviewable socket file; `socket asm.stp --part X` ([design](research/m3-socket.md)). Done 12 Oct 2026 | Generated assemblies and the AS1 fixture give the expected requirements; holes with nothing on their axis are left out |
| M4 | Signature, alignment, verification, evidence; `match`; the corpus and its harness ([design](research/m4-match.md)). Done 16 Oct 2026: no false positive on the corpus's 125 match cases under either policy; against physics, 20 of the 49 that fit rejected under `default` (21 of 51 under `functional`), each by a stated rule, none against the specification | Zero false positives on the known-bad set; the false-negative rate is reported. A false positive is `Match` on any bad case, or any pass on a bad interface. `InterfaceMatch` on the colliding-body case is correct until M7 |
| M5 | Catalog: store, index, canonical gate, CRUD, re-index ([design](research/m5-catalog.md)). Done 17 Oct 2026: on the generated corpus every catalog run is the file run over level 1's survivors, digest for digest, holding every pass (no false positive through the catalog); a match reads only the index, also with every stored file moved away; at 99,156 scaled parts the slowest full query took 6.9 s on 16 threads and none hid a pass | Ingest, list, get, replace and delete work from the CLI; a match reads only the index |
| M6 | REST API with jobs | The M4 corpus passes end to end through the API |
| M7 | Fit verifier: Open CASCADE in a separate helper process (D1); it also tests the candidate's placed copies against each other (V1 in M3's design) | On the corpus, colliding candidates end as `Reject`, or as `InterfaceMatch` with fit not verified, never as `Match`; the original part passes its own fit check |
| M8 | Viewer, per the Vectera audit | A socket and a candidate can be inspected in 3D |

The corpus for M4 is generated the way the spike's was: a script builds an assembly and candidates with known answers. Each bad candidate breaks one thing: a hole shifted, a wrong diameter, a missing hole, a wrong thickness, a blind hole where a through hole is needed, a body that collides.

### Vectera audit checklist

For each item found, note its technology and decide: depend on it, copy it, or skip it.

- [ ] Storage of STEP or CAD files: upload, content hashing, versioning
- [ ] A metadata or index database, with its schema and migrations
- [ ] REST scaffolding: routing, errors, authentication, OpenAPI
- [ ] Background jobs or a queue
- [ ] A 3D viewer or any tessellation path
- [ ] Geometry, vector or similarity code that could serve the shortlist
- [ ] Configuration, CI and release patterns worth keeping consistent

## Open decisions

No decision is open. D1, D3, D4 and D5 were settled on 9 Oct 2026, D2 on 11 Oct 2026, D11 on 12 Oct 2026, D6 to D9 at M4's exit on 16 Oct 2026, and D10 in M5 on 16 Oct 2026.

| # | Decision | Recommended default | Needed by |
| --- | --- | --- | --- |
| D1 | Body-fit check: use a kernel, or stop at the interface | Decided: `Match` needs a verified fit; until then `InterfaceMatch`. M7 runs Open CASCADE out of process; no Open CASCADE in official artifacts. See Decisions made, row 10. | M0 |
| D2 | What "canonical" guarantees: exporter, application protocol, analytic surfaces, units, one solid per part | Decided 11 Oct 2026: the gate checklist in [research/m2-features.md](research/m2-features.md), section 4. One solid, read units and topology, vertices on their surfaces, face orientation checked; B-spline faces kept as "other", never refused; exporter recorded, not gated | M2 |
| D3 | Whether the matcher is public like stepq or private | Decided: open source. | M0 |
| D4 | Project and crate names | Decided: fitment | M0 |
| D5 | Catalog storage and API stack | Decided: SQLite through rusqlite, content-addressed files on disk, axum. Keeps the catalog synchronous and the `fitment` binary free of an async runtime. As built in M5: rusqlite with SQLite bundled, WAL, one writer at a time under SQLite's lock; only `fitment-catalog` may depend on it. | M5 |
| D6 | Clearance holes: strict (no larger than the original) or functional (any hole the shaft passes) | Decided 16 Oct 2026 (M4-7): strict by default; functional on request drops the upper diameter bound only, and a pass strict would refuse carries `larger_than_original` | M4 |
| D7 | Grip length: hard requirement or warning | Decided 16 Oct 2026 (M4-8): hard only in M4; a warning waits for M7, which can check where neighbours reseat. Every grip-only change of thickness is a false negative until then | M4 |
| D8 | Real ground truth: obsolete parts with the replacement someone accepted | Decided 16 Oct 2026 (M4-38): none collected in M4; the generated corpus and AS1's exports only; revisit at M5 with catalog parts. M5 (M5-37): still none; revisit when real catalog parts arrive | M4 |
| D9 | Default tolerances | Decided 16 Oct 2026 (M4-37): 0.01 mm and 0.01°, not tuned on the generated corpus; tuning waits for real catalog parts (M5). M5 (M5-37): unchanged, since M5's catalogs were generated; revisit when real catalog parts arrive | M4 |
| D10 | Property names that carry thread and material | Decided 16 Oct 2026 (M5-5), read off the 45 fixtures ([research/m5-catalog.md](research/m5-catalog.md), section 4.13): none states a material or a thread in a form fitment can use. Part-level properties are captured at ingest, bounded and versioned, and shown by `fitment catalog get`; one evidenced mapping row ships (NX's `MaterialMissingAssignments` = `TRUE`: no material assigned). No verdict or flag changes; each later row must cite a real sample | M5 |
| D11 | How the user names the part to replace | Decided 12 Oct 2026: by part number (or name, or `#<n>`), all occurrences, merged where they ask the same; see Choosing the part to replace | M3 |

D1 was the one decision that changes what the system can honestly claim. It was answered on 9 Oct 2026 after the research in [research/d1-body-fit.md](research/d1-body-fit.md). Without a fit check, a candidate with the right interface and a body that collides is reported as an interface match, never a match. In-process bindings, monstertruck, and meshes checked with parry3d were each rejected as a source of `Match`, because each can wrongly pass a candidate. That research also defines what a sound fit check must test.

## Reference

Everything here was checked against the stepq repository at 0.4.1; the spike exercised most of it.

**stepq API to build on**

- `p21::parse`, then `model::Graph::new`; `Graph::node`, `instance`, `references`, `referenced_by`, `exchange`
- `Exchange::records` and `instances_of`; `Record::is`, `param`, `params`
- `Param::reference`, `list`, `literal`, `typed`; `Literal::to_f64`, `enumeration`, `decode`
- `model::ProductStructure::new`, `definitions`, `usages`, `roots`, `children`
- `Definition.shape_representations`, `Usage.placement`, `Placement::ShapeRelationship` and `Placement::MappedItem`
- Also part of the plan: `props::properties`, `pmi::pmi`, `info::Info::new`, `model::extract`

**Record layouts the spike read**

```text
ADVANCED_FACE(name, bounds, face_geometry, same_sense)
CYLINDRICAL_SURFACE(name, position, radius)
PLANE(name, position)
AXIS2_PLACEMENT_3D(name, location, axis, ref_direction)
ITEM_DEFINED_TRANSFORMATION(name, description, transform_item_1, transform_item_2)
MANIFOLD_SOLID_BREP(name, outer)
CLOSED_SHELL(name, cfs_faces)
VERTEX_POINT(name, vertex_geometry)
```

**Kernel options for D1**

- [cadrum](https://docs.rs/crate/cadrum/latest): Rust bindings to Open CASCADE 8.0.1 (0.8.20, MIT) with STEP reading, booleans and face traversal. A C++ build dependency; no distance or validity API.
- [opencascade](https://crates.io/crates/opencascade) 0.3.0 and `occt-sys` (LGPL-2.1, Open CASCADE 7.8.1), from opencascade-rs: the older set of Open CASCADE bindings.
- [cadquery-ocp](https://pypi.org/project/cadquery-ocp/): Python bindings to Open CASCADE 8.0.1, used by stepq's oracle and chosen for the M7 helper (D1).
- [monstertruck](https://docs.rs/monstertruck): a pure-Rust B-rep kernel forked from truck, with a STEP reader and booleans. Much less proven on real exports.

**Related**

- [stepq](https://github.com/jchultarsky/stepq) and its `docs/ARCHITECTURE.md`, which records the placement conventions: `transform_item_k` is an item of `rep_k`, and the usage, not the relationship's order, decides which side is the component's (131 reversed usages in the Pro/ENGINEER fixtures). Corrected in stepq 0.6.0 (read PR 3, stepq #46).
- [Inside the STEP File](https://jchultarsky.github.io/step-book/), the companion book, for the entity vocabulary used here.
