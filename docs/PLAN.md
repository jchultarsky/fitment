# fitment (Part Substitution Finder): Project Plan

As of 2026-10-09 · Julian Chultarsky
Living copy of this plan: https://claude.ai/code/artifact/b5f9353c-62ad-4f43-868d-9b7b5eebbb27

Given an assembly and one part in it that is no longer available, the system suggests catalog parts whose interfaces match. It is named `fitment`, will be published as open source, and is deterministic, written in Rust and built on stepq. Nothing is built yet except a throwaway spike, which confirmed that holes, placements and a socket can be read with stepq's current public API.

## Resume here

Work starts at milestone M0. No code should be written before decision D1 is answered.

1. On the laptop, create an empty project folder and open it in Code mode.
2. Save this plan into it as `docs/PLAN.md`, and the two spike files (`spike.rs`, `gen.py`) under `docs/spike/`.
3. Paste the kickoff prompt below, with the Vectera path filled in.

```text
Read docs/PLAN.md. It is the plan for fitment, a part substitution finder built on my stepq crate.
We are at milestone M0. Do these in order and stop after each for my review:
1. Audit the Vectera project at <path> against the "Vectera audit checklist" in the plan.
   Report what can be reused and how.
2. Ask me open decision D1.
3. Propose the workspace layout for the fitment crates.
Do not write code yet.
```

The project is named `fitment` and will be published as open source. stepq is at [github.com/jchultarsky/stepq](https://github.com/jchultarsky/stepq), version 0.4.1 when this plan was written.

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
| 9 | Vectera is checked for reuse, but the system must be buildable without it. | Audit in M0; every component has a fallback. |

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

Ingest runs once per catalog part and fills the index. A query extracts the socket, takes a shortlist from the index, then places each shortlisted candidate in the socket and checks every requirement.

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

One Cargo workspace holds four new crates on top of stepq; the fit verifier and the viewer are optional later additions.

| Component | Where | Responsibility | Reuse |
| --- | --- | --- | --- |
| STEP reading | stepq (extended) | Parsing, product structure, placements, geometry values, units, properties, PMI | Exists; six additions in M1 |
| Matcher | New library crate | Features, socket, signature, alignment, verification, evidence; a `FitVerifier` trait. No storage, no network. | New |
| CLI | New binary crate | `features`, `socket`, `match`, later `catalog`. Proves the matcher before any server exists. | stepq's CLI pattern |
| Catalog | New crate | File store, feature index, canonical gate, CRUD, re-index when the extractor version changes | Check Vectera |
| API | New crate | REST over catalog and matcher. Ingest and matching run as jobs. | Check Vectera |
| Fit verifier | Optional crate | Body clearance against the neighbors, using a kernel | Decision D1 |
| Viewer | Later | Shows the socket and a candidate in 3D. Needs tessellation, which stepq will not do. | Check Vectera |
| Test corpus | Dev-time scripts | Generated good and bad candidates with known answers | stepq's Open CASCADE oracle pattern |

The catalog keeps two things per part: the STEP file, addressed by its content hash, and the extracted features with the extractor version. A query reads the index and never re-parses a STEP file.

Assemblies, the chosen part, the extracted socket and the user's corrections to it are stored too, so a match can be re-run and audited.

## Changes to stepq

stepq 0.4.1 reads the file and the assembly structure but interprets no geometry. Six read-only additions close that gap, proposed as stepq 0.5 (M1).

| # | Addition | Today in 0.4.1 | Needed |
| --- | --- | --- | --- |
| 1 | Placement transforms | `model::Placement` holds only `#id`s; its doc says no transformation is evaluated | A rigid transform from each `axis2_placement_3d`; `item_defined_transformation` as M(item_2) · M(item_1)⁻¹; `mapped_item` origin to target; composed from root to every occurrence |
| 2 | Geometry values | Only `Literal::to_f64` | Typed readers for `cartesian_point`, `direction`, `axis2_placement_3d`, `plane`, `cylindrical_surface`, `conical_surface`, `toroidal_surface`, `circle`, `line`; every other surface reported as "other" |
| 3 | Topology walk | None | Solid → shell → `advanced_face` with `same_sense` → bounds → loops → oriented edges → `edge_curve` → `vertex_point`; face adjacency through shared edges; `brep_with_voids` handled or refused |
| 4 | Unit scale | `info::Units` gives names only | Numeric factor to millimetres per representation context (SI prefix, `conversion_based_unit`), plus the file's `uncertainty_measure_with_unit` |
| 5 | PMI to faces | `pmi` resolves a tolerance to a shape aspect and its product | Follow `geometric_item_specific_usage` to the faces a tolerance or dimension applies to |
| 6 | Solids of a definition | `Definition.shape_representations` lists `#id`s | A helper that returns the solids, following a simple `shape_representation_relationship` when the solid sits in a second representation |

The rule to keep, reworded: stepq reads what the file states and never computes what it does not. A radius, an axis and a placement are stated. Intersections, trimmed areas, volumes and tessellation are not, and stay out.

None of this changes output: the additions only read, so unchanged entities are still written byte for byte. No new dependency is needed.

Documents that state the old rule and need editing: `README.md` ("What it will not do"), `CLAUDE.md` (hard rules), `docs/ARCHITECTURE.md` (non-goals) and `ROADMAP.md` ("Not planned").

Test oracle, as elsewhere in stepq: compare the placements and cylinder axes stepq reads with what Open CASCADE reports for the same fixtures, by extending `tools/verify-occt.py`.

## Choosing the part to replace

The user names a part by its part number, and every occurrence of it is replaced. This is the recommended default (D11) and is not yet confirmed.

- `fitment parts asm.stp` lists the parts in an assembly: id, name and quantity, read from `PRODUCT.id`, `PRODUCT.name` and the usage occurrences.
- `--part <id>` selects by `PRODUCT.id`, usually the part number, with the name as a fallback. A value that matches no part, or more than one, is refused with the candidates listed.
- A part used several times has one socket per occurrence. A candidate must fit every one. Sockets that are identical apart from their position are merged.
- `--occurrence <path>` narrows the choice to one occurrence, addressed by its path through the assembly tree.
- The first version accepts only a leaf part, not a sub-assembly.
- In the API the assembly is stored by its content hash, so the `#id`s of the chosen part and its occurrences stay valid and are stored with the socket.

## Matching method

A candidate is never compared with the original part. It is tested against the socket, in seven steps.

1. **Features of one part**, read without a kernel:
    - Faces on a `cylindrical_surface` with the same axis line and radius are one cylinder. `same_sense = .F.` marks a hole and `.T.` a shaft, because a cylinder's surface normal points away from its axis and a face normal points out of the material.
    - Only a full circle is a hole. The arcs at each end must sum to 360°; a partial concave cylinder is a fillet or a slot end.
    - The extent along the axis comes from the vertices of the bounding edges. It gives the depth, and what closes the end gives through or blind.
    - Two coaxial radii joined by a flat ring are a counterbore. A coaxial `conical_surface` is a countersink or a drill point.
    - A plane is kept with its outward normal and its outline from the loop vertices.
    - Every other surface is recorded as "other" with its type.
2. **Socket from the assembly.** Every occurrence is placed in the assembly frame, then the target part is tested against each neighbor:
    - Coaxial and overlapping along the axis, neighbor shaft smaller than the target hole: a clearance fit. The hole must be at least the shaft's diameter and, under the strict policy (D6), no larger than the original.
    - Coaxial and overlapping, neighbor shaft larger than the hole: a threaded or press fit, inferred. The hole diameter is pinned.
    - Coaxial, not overlapping, neighbor hole: the hole continues into the neighbor, a bolted joint. The axis is required.
    - Coaxial, not overlapping, a larger neighbor cylinder ending at the target's face: a head or shoulder bears there. A seating plane and a clear ring around the hole are required.
    - Coplanar faces with opposed normals whose outlines overlap: a seating plane at that offset. Two opposed seating planes on one axis fix the grip length.
    - A target hole with nothing on its axis is not a requirement.
    - The space the target occupies is kept as the envelope for the fit verifier.
    - The socket is written as a JSON file the user reviews and can edit.
3. **Signature.** Per part, values that survive any rigid motion: the hole list (diameter, through or blind, depth, stack type), distances between parallel hole axes, angles between the others, and plane-to-axis offsets. Values are quantized at the tolerance and looked up with neighboring bins, so a value on a bin edge is not missed.
4. **Shortlist.** Candidates whose signature contains the socket's. Extra holes on a candidate are allowed.
5. **Alignment.** The candidate sits in its own frame. Each pair of candidate holes that matches two socket axes in diameter and spacing gives a placement, tried in both axis directions. A seating plane fixes the slide along the axes. A symmetric part gives several valid placements; one is enough.
6. **Verification.** Under each placement every requirement must find a candidate feature within tolerance: axis position and angle, diameter within its limits, depth at least as required, seating plane present and covering the contact outline, grip length equal.
7. **Verdict.** `Match`: every requirement and the fit are verified. `InterfaceMatch`: every requirement is verified and the fit is not checked. `Reject`: with the failed requirements listed. Flags ride along, such as "thread not confirmed" and "material not compared".

Tolerances are one linear and one angular value, held in a named policy stored with each result. The floor is the larger stated uncertainty of the two files. Lengths are converted to millimetres before any comparison.

## Metadata from STEP only

The system has no data source besides the STEP files, so every non-geometric fact comes from `stepq::props` and `stepq::pmi` or is reported as unknown.

- **Availability** is not in a STEP file. Catalog membership stands in for it: a part in the catalog is available, and withdrawing a part is a delete. The user picks the part to replace.
- **Threads** are usually not modeled, and a tapped hole looks like a plain one. The socket infers "threaded or press fit" when a neighbor's shaft is larger than the hole it sits in. The candidate must then have the same hole diameter, and the verdict carries "thread not confirmed" unless a property or PMI on both parts states the thread.
- **Material, finish and ratings** are compared only when both files carry the property. Otherwise the result says "not compared".
- **Property names** differ between exporters, so the catalog needs a small mapping from property names to fields, built from real sample files (D10).
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
3. Open CASCADE writes a full cylinder as one face with a seam. Exporters that write two half faces are handled by the grouping step, which this test did not exercise.

Not covered: nested assemblies, `mapped_item` placements, unit scale, other exporters, counterbores, blind holes and outline overlap of planes.

The spike is throwaway code. Its files, `spike.rs` and `gen.py`, are a starting reference for M1 and M2. To rerun it: `python3 gen.py step/` (needs cadquery), then build `spike.rs` as `src/main.rs` of a crate that depends on stepq with `default-features = false`, and run it with `step/` as its argument.

## Milestones

M2 to M4 carry all the technical risk, so the catalog and API (M5, M6) do not start until M4's exit test passes.

| # | Delivers | Done when |
| --- | --- | --- |
| M0 | Repository and workspace, this plan in `docs/`, the Vectera audit, decision D1 | The audit is written and D1 is recorded |
| M1 | stepq 0.5: the six additions | stepq's placements and cylinder axes equal Open CASCADE's on the fixtures; the four documents are updated |
| M2 | Matcher crate: features of one part; `features part.stp` prints JSON | Holes, shafts, depths and planes are correct on generated parts with known features; non-canonical input is refused with a reason |
| M3 | Socket extraction and the reviewable socket file; `socket asm.stp --part X` | Generated assemblies and the AS1 fixture give the expected requirements; holes with nothing on their axis are left out |
| M4 | Signature, alignment, verification, evidence; `match`; the corpus and its harness | Zero false positives on the known-bad set; the false-negative rate is reported |
| M5 | Catalog: store, index, canonical gate, CRUD, re-index | Ingest, list, get, replace and delete work from the CLI; a match reads only the index |
| M6 | REST API with jobs | The M4 corpus passes end to end through the API |
| M7 | Fit verifier, per D1 | Candidates that collide with a neighbor are rejected on the corpus |
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

Nine decisions are open and two were settled on 9 Oct 2026; D1 is needed before any code. Each has a default that applies if nothing else is decided.

| # | Decision | Recommended default | Needed by |
| --- | --- | --- | --- |
| D1 | Body-fit check: use a kernel, or stop at the interface | Ship `InterfaceMatch` with fit not checked. Add an Open CASCADE-backed verifier as an optional crate in M7, and evaluate monstertruck as the pure-Rust alternative. | M0 for result wording, M7 to build |
| D2 | What "canonical" guarantees: exporter, application protocol, analytic surfaces, units, one solid per part | Write it as the ingest gate's checklist, from three real sample files | M2 |
| D3 | Whether the matcher is public like stepq or private | Decided: open source. | M0 |
| D4 | Project and crate names | Decided: fitment | M0 |
| D5 | Catalog storage and API stack | Decide after the Vectera audit. Fallback: SQLite, content-addressed files on disk, axum. | M5 |
| D6 | Clearance holes: strict (no larger than the original) or functional (any hole the shaft passes) | Strict | M4 |
| D7 | Grip length: hard requirement or warning | Hard requirement | M4 |
| D8 | Real ground truth: obsolete parts with the replacement someone accepted | Collect any that exist; use the generated corpus until then | M4 |
| D9 | Default tolerances | Start at 0.01 mm and 0.01°, tune on the corpus | M4 |
| D10 | Property names that carry thread and material | Read them off real sample files | M5 |
| D11 | How the user names the part to replace | By part number, all occurrences; see Choosing the part to replace | M3 |

D1 is still unanswered from the planning conversation. It is the one decision that changes what the system can honestly claim: without a fit check, a candidate with the right interface and a body that collides is reported as an interface match, not a match.

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

- [cadrum](https://docs.rs/crate/cadrum/latest): Rust bindings to Open CASCADE 7.9.3 with STEP reading, booleans and face traversal. A C++ build dependency.
- `opencascade-rs`: the older set of Open CASCADE bindings.
- [monstertruck](https://docs.rs/monstertruck): a pure-Rust B-rep kernel forked from truck, with a STEP reader and booleans. Much less proven on real exports.

**Related**

- [stepq](https://github.com/jchultarsky/stepq) and its `docs/ARCHITECTURE.md`, which records the placement conventions: `transform_item_1` is the child frame, `transform_item_2` the parent frame, and the usage occurrence wins when a file reverses the relationship.
- [Inside the STEP File](https://jchultarsky.github.io/step-book/), the companion book, for the entity vocabulary used here.
