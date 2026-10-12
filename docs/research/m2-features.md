# fitment M2: features of one part

As of 2026-10-11. Status: **accepted** on 2026-10-11, with every default in section 2 (R1 to R11). Nothing here is implemented yet; section 16 is the order of work.

M2 delivers `fitment features part.stp`, which prints a part's features as JSON. It is done when holes, shafts, depths and planes are correct on generated parts with known features, and non-canonical input is refused with a reason ([PLAN.md](../PLAN.md), Milestones).

Four agents designed one area each:

- A: cylinders, holes and shafts;
- B: planes, the part model, the JSON format and the API;
- C: the canonical gate (decision D2);
- D: the generated test parts, the oracle, the fixtures and CI.

Each measured its rules on stepq 0.6.0 (`78e46e8`) and on parts generated with Open CASCADE. A (cylinders) and C (gate) read all 55 fixtures stepq fetches. B's plane and loop census read the 48 files under 16 MB, and its numbers say so. This document merges their work and settles where they disagreed (section 18).

Two adversarial reviewers then attacked the merged design: one for soundness, one for consistency and evidence. Their findings are applied here; section 18 lists the soundness fixes, each reproduced with a generated part. Probe code, the review reports and raw output are in [m2-evidence/](m2-evidence/), which is not committed.

## 1. Summary

- **The gate refuses a file that is not canonical, with every reason.**
  - A canonical part has exactly one product definition, no assembly usage and a complete shape.
  - The shape holds exactly one closed, connected B-rep solid, plus curve and point sets only.
  - Its units, uncertainty, topology and every supported surface, curve and vertex can be read.
  - Its vertices lie on their surfaces and curves within the stated uncertainty.
  - Of the 55 fixtures, 24 are accepted and 31 refused: 19 are assemblies, 10 have surface models beside the solid, 1 has only a tessellated solid, and 1 has four product definitions.
  - Every face's stated orientation is cross-checked against its loops, so one flipped `same_sense` cannot turn a hole into a pin.
- **B-spline faces do not refuse a part.** They become "other" faces, which never become features, and anything that depends on them is `unknown`. Refusing them would refuse 24% of otherwise valid parts and buy no safety, because an unread face can only cause a missed match.
- **Every face of the solid lands in exactly one primary feature.** The primary features are planes, cylinders, cones, tori and other faces. Holes and shafts are compound features built on top of them. The JSON states the face counts, so a test can prove nothing was dropped.
- **Planes** have an outward normal, an offset and an exact outline: loops of line segments and circular arcs, never sampled. A face whose outline has a B-spline or ellipse edge keeps its plane, but its outline is `unknown`.
- **Cylinders** group edge-connected faces with the same sense on one stated cylinder. `face_sense` false is a hole wall and true is a shaft wall. A cylinder is full only when stated circles close 2π around it. (An axial seam was to prove it too; review of PR 5 withdrew that rule, F3.) Partial cylinders (fillets, slot ends) are never holes. On the 55 fixtures:
  - 9,344 full cylinders: 7,626 holes, 1,573 shafts and 145 in voids;
  - 43,664 partial cylinders;
  - 889 undecided cylinders, reported with a reason.
- **Hole ends come from a walk along the axis** across coaxial annuli, cones and tori. The walk gives through, blind (flat, drill point, annular), counterbores, countersinks and step drills. Every plane the walk meets must agree on air or material, so a blind undercut cannot read as through. Depth comes from stated circle centres, never from vertices, and a rim sits on the plane the hole opens onto. Anything the walk cannot decide is `Unknown`, never a guess.
- **`features.v1`** is a fixed JSON envelope. It is deterministic byte for byte on every OS, carries `extractor_version`, and traces every value to a STEP `#id`.
- **Tests:**
  - 43 generated parts with known answers, 6 of which must read as `unknown`, and 9 refusal cases, plus hand-written STEP snippets for what no part can reach;
  - 25 of them committed as a unit set within the 300 KB cap, which runs everywhere without Python;
  - an Open CASCADE oracle that checks the parts against their construction and fitment against Open CASCADE;
  - the third-party fixtures, with each file's gate result pinned.
- **Eight PRs** (section 16).

## 2. Decisions for review

Each has a recommended default. The rest of the document assumes the defaults.

| # | Question | Default | What it costs or buys |
| --- | --- | --- | --- |
| R1 | B-spline and other unread faces: refuse the part, or keep it and report the faces as "other"? | Keep and report | Refusing would cost 223 of 916 parts (24%) for no gain in soundness. Features never use an unread face. |
| R2 | A surface model (`shell_based_surface_model`) beside the solid in the part's shape | Refuse (`unread_shape_item`) | It may be material the solid lacks, which the M7 fit check would miss. Costs 38 leaf parts that fail only this check, and 10 NIST files, including `nist_ctc_02`. |
| R3 | Several solids in one definition | Refuse (`several_solids`) | Costs 12 leaf parts that fail only this check (27 fail it). Every feature names its solid, so this can be lifted later without a format change. |
| R4 | Several product definitions and no assembly usage | Refuse (`several_definitions`) | Costs 1 file (`nist_ftc_09-e1`, whose other definitions hold surface models and curve sets). |
| R5 | A solid with voids | Accept. Void faces never form holes or shafts. | Keeps 22 parts. A void's cylinder is a cavity, not a hole. |
| R6 | A part with undecided cylinders or unknown hole ends | Accept with `complete: false` | 16 of 55 files have undecided cylinders, but only 1 of the 24 accepted files does (`nist_ftc_07_asme1_rd.stp`, 7). Refusing would quarantine every file whose circles are written as B-splines. |
| R7 | Grouping tolerance: a fixed 1e-6 mm, or each context's stated uncertainty (up to 0.672 mm)? | Fixed 1e-6 mm. The uncertainty is carried as the matcher's floor. | The fixed value already joins 98.3% of adjacent faces bit for bit. Widening it gains 19 full cylinders, all shafts and no holes, and merges surfaces 0.5 mm apart. |
| R8 | Angled holes whose ends are ellipses, and holes ending in a cross bore | An `undecided` cylinder with its axis and diameter, `Irregular` ends, length `null`. A seam was to prove such a hole full (F3), but without the curves on the surface a slit looks the same, so review of PR 5 withdrew it | stepq reports ellipses and the B-spline edges of a cross bore as `Curve::Other`. Reading ellipses is a possible stepq 0.7 addition (section 19). |
| R9 | Commit the unit set without pcurves, within the 300 KB cap | Yes | The 20 parts measured so far total 224,380 bytes. The pcurve variants run in CI from the generated corpus. |
| R11 | A whole shell written inside out, consistently: every face's sense and loops reversed together | Not detected at M2; a residual risk. The oracle checks that every accepted fixture and corpus solid has positive volume in Open CASCADE | A per-face check cannot see it, and a naive global test flags 21 valid fixture shells. A sound global test needs more work (section 19). |
| R10 | D2's "three real sample files" | The fixtures stand in now. Three vendor parts (a screw, a bracket, a bearing) are fetched with the D10 work at M5. | No fixture is a vendor catalog part. |

## 3. How a file is read

`fitment_core::read_part(bytes)` runs these steps. Each one either refuses with reasons or hands a checked value to the next.

1. **Gate, stages 1 to 3** (section 4): parse the file, check the schema, choose the part, check its shape.
2. **Read the solid** with stepq: `topology::Solid::read`, a `geometry::Reader` for the solid, and its context units.
3. **Gate, stages 4 to 6:** topology, units, geometry values and vertex consistency.
4. **Group faces into primary features** (sections 6 to 8): planes, cylinders, cones, tori and other faces. Every face lands in exactly one.
5. **Build holes and shafts** from full cylinders by the end walk (section 9).
6. **Check the face partition**, convert to `geom` types, normalise `-0.0`, and return the `Part`.

stepq types appear only in `features/step.rs` and the files of `gate/`. Everything public is fitment's own types (section 11).

## 4. The canonical gate (D2)

### 4.1 What an accepted part guarantees

Extraction may assume all of the following:

1. The file parses, has no dangling reference and one data section, and names AP203, AP214 or AP242.
2. It holds exactly one product definition and no assembly usage (M2; section 4.5 covers M3).
3. That definition's shape is complete (`DefinitionShape::is_complete`). It holds exactly one `manifold_solid_brep` or `brep_with_voids`, and its other items are only curve and point sets.
4. The solid's map into the definition's shape representation can be read (`ShapeItem::transform`).
5. `Solid::read` reads the solid. Every edge is used exactly twice, in opposite directions, and each shell is one connected piece.
6. The solid's contexts agree. Each states a readable length unit, plane angle unit and at least one length uncertainty.
7. Every vertex point can be read, and so can every surface and curve of a type stepq reads.
8. Each vertex of a plane, cylinder, cone, sphere or torus face lies on that surface, and each end vertex of a line or circle edge lies on that curve, within `max(u, 1e-6 mm) × (1 + 1e-6)`. Here u is the largest stated length uncertainty.
9. Each face's stated sense agrees with the direction its loops run (check 25).

It does **not** guarantee:

- analytic surfaces only (R1);
- geometric validity beyond item 5;
- that the stated unit is the true one;
- anything about faces or edges of unread types;
- that `PRODUCT.id` is a part number.

### 4.2 The checklist

The checks run in stages. A stage that refuses skips the later stages, which need what it establishes, so one fault does not cascade. Within a stage every check runs, so a file can carry several reasons.

"Files" counts the 55 fixtures as M2 input. "Leaves" counts the 1,348 leaf product definitions taken as parts, which is how M3 will check an assembly's components.

| Stage | # | Check | Code | Fixtures |
| --- | --- | --- | --- | --- |
| File | 1 | `p21::parse` succeeds | `unparseable` | 0 |
| | 2 | `Graph::new` succeeds: no reference to an undefined instance | `dangling_reference` | 0 |
| | 3 | Exactly one data section | `several_data_sections` | 0 |
| | 4 | The first `FILE_SCHEMA` name, before `{`, trimmed and upper-cased, is an AP203, AP214 or AP242 schema. Any further name is `GEOMETRIC_VALIDATION_PROPERTIES_MIM` | `schema_unrecognized` | 0. The files spell 16 identifiers; only the name is compared |
| Structure | 5 | No assembly usage | `assembly` | 19 files |
| | 6 | At least one product definition | `no_definition` | 0 |
| | 7 | At most one product definition (R4) | `several_definitions` | 1 file |
| Shape | 8 | `is_complete()`. One refusal per link not followed, with stepq's reason. A failure ends the stage, since what the shape holds beyond those links is unknown | `shape_incomplete` | 0 of 1,660 definitions |
| | 9 | At least one solid | `no_solid` | 1 file (tessellated only); 364 leaves, 340 of them empty definitions in the OCP enclosure |
| | 10 | At most one placed solid (R3). stepq lists a solid once for each path that reaches it. Paths that place it the same way are one solid; a mapped item that places the same `#id` elsewhere is another copy, so placements, not `#id`s, are counted (found in review of PR 1) | `several_solids` | 27 leaves |
| | 11 | Every other item is a `geometric_curve_set` or `geometric_set` of points and curves only (R2) | `unread_shape_item` | 11 files (10 with surface models; 1 tessellated only, also `no_solid`), 63 leaves: 85 surface models and 1 tessellated solid |
| | 12 | `ShapeItem::transform` succeeds for the solid | `solid_placement_unreadable` | 0. No fixture places a solid through a mapped item; a generated case covers it (section 13) |
| Topology | 13 | Not `TopologyError::Unsupported` | `topology_unsupported` | 0 |
| | 14 | Not `Malformed`, `Missing` or `UnresolvedReference` | `topology_malformed` | 1 solid: a Creo chassis loop that does not close |
| | 15 | Every edge has exactly two uses, with opposite `forward`, by faces of one shell | `solid_not_closed` | 0 of 1,260 |
| | 16 | Each shell is connected through shared edges. Runs only if check 15 passed: walking a face's neighbours is linear only once every edge has two uses, and a hostile file with one edge used 80,000 times took 15 s before (found in review of PR 3) | `shell_disconnected` | 0 of 1,260 |
| Units | 17 | `ContextUnits::of_item(solid)` succeeds | `units_unreadable` | 0 |
| | 18 | The length unit reads | `length_unit_unreadable` | 0; accepted parts are 840 mm, 76 inch |
| | 19 | The plane angle unit reads | `angle_unit_unreadable` | 0 |
| | 20 | The uncertainties read | `uncertainty_unreadable` | 0 |
| | 21 | At least one uncertainty has a length unit, under any name | `uncertainty_missing` | 0. Names vary: `closure` 819, `distance_accuracy_value` 422, others 19 |
| Geometry | 22 | Every vertex point, every face surface and every edge curve reads. `Surface::Other` and `Curve::Other` are not errors, but must name a surface or curve entity: stepq reads any entity it does not interpret as one, so a face on a `CARTESIAN_POINT` would otherwise pass (found in review of PR 3) | `geometry_unreadable` | 0 |
| | 23 | Vertices lie on their analytic surfaces (section 4.1, item 8) | `vertex_off_surface` | 2 of 1,260 solids, up to 0.040 mm against u = 0.0018 mm. 1,384,196 vertex uses checked |
| | 24 | End vertices lie on their lines and circles | `vertex_off_curve` | 0 |
| | 25 | **Face orientation agrees with the loops.** (a) A planar face whose outline is known has positive net signed area about its outward normal. (b) On a cylinder or cone face, each coaxial circle at the face's lowest or highest station runs, in the face's loop, counterclockwise about z exactly when the face lies on the +z side of that circle and the face is a shaft wall, or on the −z side and a hole wall. Section 18, C1 | `face_orientation` | 0. 118,773 circle uses agree and none disagree over all 55 files; net area is positive on all 8,922 readable planar faces in the 48 files under 16 MB. One flipped `same_sense` in a generated part gives 2 disagreements. As built in PR 3, after review: (a) is measured from the face's first vertex, and a circle's term is its exact projection onto the plane (scaled by the cosine of its axis to the normal), so an arc standing across the plane counts as its chord and cannot turn the sign; (b) judges a face only if every edge is a coaxial circle or a line between the extreme circles' stations, since otherwise the circles need not bound the face |
| Extraction | 26 | Every coordinate is finite and below 1e6 mm in magnitude: vertices, every surface's and circle's location and radii, and every line's point. It runs before checks 23 to 25, which need finite arithmetic: a cylinder placed at 1e308 made a vertex's distance NaN, which no bound refused (found in review of PR 3). Every bound is tested so that NaN fails it | `out_of_range` | 0. The fixtures reach about 1e4 mm |
| | 27 | Cylinder faces that share an edge, on one cylinder within 1e-6 mm, have opposite senses (a wall of zero thickness) | `contradiction` | 0 |
| | 28 | A perpendicular annulus whose sense contradicts its two radii | `contradiction` | 0 |
| | 29 | A closed chain of end arcs sweeps a multiple of 2π other than 2π | `contradiction` | 0 |
| | 30 | A step of extraction finds its own invariant broken, such as a face in no feature | `internal` | 0. A test asserts no fixture produces it. It is a fitment bug, not a property of the input, so the CLI exits 1 for it (section 12) |

Recorded for every file, accepted or refused, in the document's `header` (section 10.1), but never gated:

- the schema family and the identifier as written;
- the originating system and preprocessor version (blank in 33 of 55 files);
- the implementation level.

Recorded for every accepted solid: `max_vertex_offset_mm`, the largest distance checks 23 and 24 measured. M4 needs it (section 19).

Checks 23 and 24 compute distances from stated values, which is fitment's job, not stepq's. The cone distance uses `libm::tan` and `libm::cos`, so a borderline result cannot differ between operating systems.

### 4.3 Why each failure refuses or not

A refused part is never indexed, so it can only cause false negatives. Refusing is always safe; its only price is coverage. A per-feature "other" is safe only if no feature value depends on an unread entity.

- **Checks 1 to 12 refuse.** Without one parsed graph, one definition and one complete shape, every feature is in doubt. A second body or a surface model could hold material that features miss, and so would the M7 fit check, which could then pass a body that collides. That would be a false `Match`.
- **Topology (13 to 16) refuses.** The end walk and the full-circle test walk loops and edge uses. An open solid makes "through or blind" and "hole or slot end" unreliable, and a wrongly read hole is a false-positive risk.
- **Units (17 to 21) refuse.** Every length uses the length unit, and the tolerance floor needs the uncertainty.
- **Geometry (22) refuses.** stepq raises `GeometryError` only for a supported entity that contradicts the schema, such as a zero radius.
- **Vertices (23, 24) refuse.** Depths, outlines and arc spans use vertex positions. If a vertex contradicts its surface, a value read from it can be wrong and look valid. Confining that to one feature is not cheap, and it costs 2 solids.
- **Face orientation (25) refuses.** Every hole-or-shaft and every outward normal rests on `face_sense`. stepq's `EdgeUse::forward` does not involve `same_sense`, so a flipped `same_sense` passes every other check, and Open CASCADE reads it the same way, so the oracle cannot catch it. Without this check, one changed character turns a Ø6.6 through hole into a Ø6.6 pin. A whole shell written inside out passes it (R11).
- **Contradictions (27 to 29) refuse.** Each is a file stating two things that cannot both be true.
- **Unread surfaces and curves do not refuse (R1).** The extraction contract:
  - A face of an unread type is never a plane, cylinder, cone or torus feature. It is listed in `other_faces` with its entity type.
  - Any attribute that would depend on an unread entity is `unknown`, never guessed.
  - Soundness rule 2 still holds: "a spline surface on an interface" is rejected at verification (M4), because no read feature can meet that requirement. The gate cannot know at ingest which faces will be interfaces.
- **Voids do not refuse (R5).** Void faces are partitioned like any other faces, but they never form holes or shafts.

### 4.4 Reason codes

```rust
/// Why a file is not canonical. Variant order is the order the checks run
/// and the order refusals are reported in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum RefusalCode {
    Unparseable, DanglingReference, SeveralDataSections, SchemaUnrecognized,
    Assembly, NoDefinition, SeveralDefinitions,
    ShapeIncomplete, NoSolid, SeveralSolids, UnreadShapeItem, SolidPlacementUnreadable,
    TopologyUnsupported, TopologyMalformed, SolidNotClosed, ShellDisconnected,
    UnitsUnreadable, LengthUnitUnreadable, AngleUnitUnreadable,
    UncertaintyUnreadable, UncertaintyMissing,
    GeometryUnreadable, VertexOffSurface, VertexOffCurve, FaceOrientation,
    OutOfRange, Contradiction, Internal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Refusal {
    pub code: RefusalCode,
    /// The innermost `#id` at fault, if there is one.
    pub instance: Option<u64>,
    /// For people: a fixed sentence for the code, then stepq's error text.
    pub message: String,
}
```

- **Machine names never change meaning.** They are the snake_case of the variant, and a new check gets a new code.
- **`message` may change between versions,** because it quotes stepq. It is deterministic for one input and one version.
- **Refusals are sorted** by (code, instance, message), and a refused document never lists any features.

### 4.5 What changes at M3

At M3, `socket asm.stp --part X` reads assemblies.

- **Check 5 becomes the assembly gate,** and check 7 becomes "exactly one root".
- **Every usage must place** (`Placements::usage`). A failure is a new code, `placement_unreadable`, that carries stepq's `PlacementError` kind. All 3,481 usages in the 19 assembly fixtures place.
- **Every distinct leaf definition passes stages 3 to 6** once. A component that fails refuses the assembly, with a new code `component_refused` that carries the component's reasons.
  - The reason is soundness: an unread neighbour removes socket requirements, and a missed requirement is a false positive (Soundness rule 3).
  - 10 of the 19 assembly fixtures pass under this rule.
- **The solid's frame must be the one the usage places** (new code `solid_frame_unplaced`).

### 4.6 Three sample files

D2's default asks for three real samples. These three fixtures come from three exporters and three APs.

| | `nist-edm/clevis21.stp` | `nist_ctc_02_asme1_ap242-e2.stp` | `steptools/as1-ug-214.stp` |
| --- | --- | --- | --- |
| Schema | AP203 ed1 | AP242 | AP214 |
| Originating system | ST-ACIS | blank (NIST anonymised) | Unigraphics 16.0 |
| Definitions / usages | 1 / 0 | 1 / 0 | 9 / 13 |
| Shape items | 1 solid | 1 solid, 2 surface models ('Fill.1', 'Split.1'), 1 curve set | 5 leaves, 1 solid each |
| Units, uncertainty | mm; `closure` 1e-6 mm | mm; 0.01 mm | mm; 2e-5 mm |
| Faces | 48: 30 plane, 12 cylinder, 6 cone | 635, including 34 B-spline | 3 to 12 per part, planes and cylinders |
| Largest vertex offset | 1.5e-11 mm | 1.0e-5 mm | 7e-15 mm |
| **M2 result** | **accepted** | **refused:** `unread_shape_item` ×2 | **refused:** `assembly` |
| As M3 components | | Its solid passes every later check. With R2 relaxed it would be accepted with 34 "other" faces. | All 5 leaves pass |

## 5. Reading the solid

### 5.1 Frame and units

- **Frame.** Features are stated in the frame of the part's shape representation, which is the space an assembly placement maps. A solid reached through mapped items is moved into that frame with `ShapeItem::transform`, and `solids[].transform` records it. It is `null` when the path holds no mapped item.
- **Units.** Every length is in millimetres and every angle in radians, as stepq's `Reader` returns them. Field names carry the unit (`_mm`, `_mm2`, `_rad`); directions are unitless.
- **Recorded per solid:** the length unit (name and factor), the plane angle unit, and every stated uncertainty (name, value in mm).
- **`max_uncertainty_mm`** is the part's largest stated length uncertainty. It is the matcher's tolerance floor (PLAN.md, Matching method), not an extraction tolerance (R7).

### 5.2 Extraction constants

These decide which faces the file states as one surface. They are written into every document's `extraction` block, and changing one bumps `EXTRACTOR_VERSION`. They are not the matching tolerance (D9).

| Constant | Value | Used for |
| --- | --- | --- |
| `IDENTITY_MM` (ε) | 1e-6 mm | "The file states the same value twice": grouping, circle on cylinder, coaxial transitions, station clustering, coplanar offsets |
| `PARALLEL_SINE` | 1e-9 | Plane normals in one direction class |
| Lever-arm rule | s·ℓ ≤ ε | Cylinder and cone axis directions and circle normals: the sine s at lever arm ℓ must be within ε. For a circle normal, ℓ is the radius. For two faces' axes, ℓ is the larger of the two faces' extents along the axis, from their own vertices, and at least `LEVER_MIN_MM`. The probe used a fixed sine of 1e-9 instead, so this rule is not yet measured |
| `VERTEX_SIDE_MM` (ε_v) | 1e-3 mm | Which side of the stations a non-circular boundary lies on (F4), and whether a cone's vertex is at its apex (W4) |
| `LEVER_MIN_MM` | 1 mm | The smallest lever arm ℓ |
| `SWEEP_RAD` | 1e-9 rad | How close a station's summed sweep must be to 2π (F2) |
| `WALK_MAX` | 8 | The most transition faces one end walk crosses (W7) |
| `GATE_SLACK` | 1e-6 mm, ×(1 + 1e-6) | The floor and relative slack of checks 23 and 24 |
| `RANGE_MM` | 1e6 mm | The largest coordinate magnitude (check 26) |

Evidence:

- Of 20,930 adjacent face pairs on one cylinder (within 0.1 mm), 20,583 (98.3%) state the axis and radius bit for bit, and 212 more agree within 1e-9 mm. The other 135 differ by 1e-9 to 0.1 mm, all in the seven large Open Compute assemblies. All 3,121 pairs in the 48 files under 16 MB are exact.
- Plane grouping gives identical groups at 1e-9 and at 1e-6: 1,643 groups of two or more faces, in the 48 files under 16 MB. Exact bit equality finds only 1,494.
- Using each context's uncertainty instead (R7) gains 19 full cylinders, all shafts, and no holes: 42 more are full at both ends, 23 of which were already full with one irregular end. It accepts surfaces up to 0.672 mm apart as one, and leaves 126 small cylinders too small to resolve.

Near misses, where two adjacent faces agree within the uncertainty but not within ε, are reported as `near_miss` notes. There are 128 such pairs, all in Creo files.

### 5.3 Loops

A loop is walked by this rule:

- take the oriented edges in written order if the bound's `orientation` equals its shell's, else in reverse order;
- run each edge from start to end if `EdgeUse::forward`, else from end to start.

The edges then form a closed chain on all 24,625 loops in the 48 fixtures under 16 MB. Written order alone breaks 14,000 of them.

On a planar face, the outer loop is the one loop with positive signed area about the outward normal, not the loop `face_outer_bound` names. One NIST exporter labels the wrong loop on 23 of 24 multi-loop faces. A stated outer bound that disagrees is a note, `stated_outer_bound_differs`.

### 5.4 Determinism

The same bytes give the same output bytes on Linux, macOS and Windows.

1. **No hash order.** `HashMap` and `HashSet` are banned in core. Every array is in a stated order: features by smallest face `#id`, faces by `#id`, loops outer first then as written, segments in traversal order, notes by instance then code.
2. **No platform maths.** Only `+ − × ÷` and `sqrt`, which are correctly rounded. `libm::atan2` and `libm::tan` are used for angles; clippy already bans `f64::atan2`.
3. **Finite numbers only.** `geom` constructors return `None` on a non-finite value, which becomes `out_of_range`. serde_json would otherwise silently write NaN as `null`.
4. **No negative zero.** Every stored float passes through `finite()`, which adds `0.0`. Open CASCADE's flipped normals produce `-0.0` otherwise.
5. **Fixed summation order:** segments, then loops, then faces.
6. **Nothing from the environment:** no path, time, host or locale in the document.
7. **Text:** serde_json is pinned by `Cargo.lock` with `float_roundtrip`. The CLI writes pretty JSON plus `\n`. A serde_json update that changes float text bumps `EXTRACTOR_VERSION`.

## 6. Primary features: the face partition

Every face of the solid lands in exactly one primary feature:

| Primary feature | Faces | Section |
| --- | --- | --- |
| `plane` | coplanar plane faces with the same outward normal | 7 |
| `cylinder` | edge-connected cylinder faces with the same sense on one cylinder | 8 |
| `cone` | edge-connected cone faces on one cone | 8.6 |
| `torus` | edge-connected torus faces on one torus | 8.6 |
| `other` | one face each: spheres and every unread surface (B-spline, extrusion, revolution, offset, complex) | 8.7 |

- **Ids are `<kind>-<smallest face #id>`,** for example `plane-213` or `cylinder-490`. Holes and shafts take `hole-` and `shaft-` with the smallest face `#id` of their cylinders. Ordering is numeric by that `#id`, never by the string.
- **`face_counts`** gives the faces of each surface kind and the total. `read_part` checks on every input that each face appears in exactly one primary feature, and refuses with `internal` otherwise. A core test asserts it on every unit part (`include_bytes!`), and `crates/fitment/tests/fixtures.rs` asserts it from the JSON of every accepted fixture. Core's tests cannot read fixtures, because core bans file I/O, tests included.
- **Every feature, primary or compound, names its `solid`** (`solid-<#id>`), so the one-solid rule (R3) can be lifted without a format change.
- **Every primary feature records its `shell`:** 0 for the outer shell, k for the k-th void. No primary feature spans two shells.
- **Holes and shafts are compound.** They reference primary features by id and never take a face away from one.

Other items of the shape are listed in `other_items` with `#id` and entity type. After the gate, only curve and point sets remain.

## 7. Planes

### 7.1 What a plane holds

A `Plane` is one infinite plane of the solid.

- **`solid` and `shell`.**
- **`normal`:** the outward unit normal. It is the plane's `position` z if `Solid::face_sense(face)` is true, else −z. 41% of planar faces in the 48 fixtures under 16 MB need the flip.
- **`offset_mm`:** the plane is every x with `normal · x = offset_mm`.
- **`spread_mm`:** the largest minus the smallest face offset, at most 1e-6.
- **`area_mm2`:** the sum of its faces' areas, or `null` if any outline is unknown.
- **`faces`:** one `PlaneFace` per face, sorted by `#id`. A `PlaneFace` records its face, surface, its own offset, its net area, and an `outline`.

### 7.2 Grouping

Faces of one solid and one shell are grouped in two stages. Planes never cross shells, so a void's face cannot join an outer seating plane.

1. **Direction class.** A face joins the class whose normal agrees with its own: `|n_c × n_f| ≤ PARALLEL_SINE` and `n_c · n_f > 0`. The class normal is that of its smallest-`#id` face. Faces are compared only with that face, so classes cannot drift by chaining. Candidate classes are found through quantized keys and the 27 neighbouring bins; the exact test decides membership.
2. **Offset runs.** Within a class, faces are sorted by `(n_c · p_f, #id)`, and a run of offsets within `IDENTITY_MM` of its first face is one plane.

As built after review of PR 4:

- **Offsets are measured at the face.** A plane's stated location may lie anywhere on it, up to 1e6 mm from its face, and a normal within `PARALLEL_SINE` of the class's then moves the offset there by up to 1e-3 mm: a 0.0005 mm step read as one plane. Each face is measured at its anchor, its first vertex projected onto its own plane, and joins a class only if its normal's sine, times its extent from the anchor, is also within ε (the lever-arm rule of section 5.2).
- **Bins are 2 × `PARALLEL_SINE` wide.** Bins 1e-6 wide held thousands of classes on a jittered grid, which made grouping quadratic (14 s for 180,000 faces).

Disjoint coplanar faces, such as two boss tops, are one plane. Opposed coincident faces are never grouped; there are 391 such pairs in the 48 fixtures under 16 MB. The cost is O(F log F).

### 7.3 Outline: exact, never sampled

A polygon through sample points lies inside an arc on one side and outside it on the other. A seating test on a sampled outline therefore proves nothing about the real one. So the outline keeps the vertices and each edge's stated line or circle.

- **Loops:** the outer loop first (section 5.3), then the inner loops as the face lists its bounds. Each loop records its bound `#id`, `outer`, its signed area and its segments.
- **Segments,** in traversal order:
  - a `line` holds the edge `#id`, `from_mm` and `to_mm` (the stated vertex points), and `across`;
  - an `arc` also holds `center_mm`, `radius_mm` and `sweep_rad`, the signed angle swept about the outward normal, in [−2π, 2π]: a whole circle sweeps 2π or −2π as the loop runs it.
  - **`across`** is the id of the primary feature on the other side of the edge. It links a plane's inner loop to the cylinder that opens there, without any geometry.
- **Arc sweep.** For a segment `a → b` with centre c and circle axis z, φ(u → v) is the counterclockwise angle about z from `u − c` to `v − c`. It is `libm::atan2(((u−c)×(v−c))·z, (u−c)·(v−c))` moved into (0, 2π], and a closed edge has φ = 2π. The traversal runs along the circle iff `forward == edge.same_sense`. Then θ_z is φ(a → b) along, or −φ(b → a) against, and `sweep_rad = θ_z · sign(z · normal)`.
- **Signed area,** about the outward normal n, with o the face's stated plane location:
  - a line: `½ ((a − o) × (b − o)) · n`;
  - an arc: `½ ((c − o) × (b − a)) · n + ½ r² θ`.
  These are exact for lines and circular arcs (Green's theorem). The area agreed with Open CASCADE's `BRepGProp` to 4.5e-13 mm² on 14 generated faces, including fillet arcs and a counterbore floor.
- **Points stay as stated.** Vertices are not projected onto the plane. The gate already bounds their distance from it.
- **As built in PR 4:** every loop of a face is measured from the face's first vertex rather than the plane's location, which changes no exact value and loses no digits on a plane placed far from its face. Against Open CASCADE's `BRepGProp`, 1,737 of 1,761 known planar faces in the corpus and the 24 accepted fixtures agree within 1e-9 relative. The other 24, in six NIST files, differ by up to 3% on faces under 1 mm²: a vertex stated up to 0.0037 mm off its edge's line, within the file's uncertainty, moves the area measured through the vertices but not the one integrated along the curves. Moving each vertex by d changes a loop's area by at most d times its perimeter, and every difference is within that bound for d = `max_vertex_offset_mm`; the oracle allows it and tallies those faces apart.

The outline becomes `unknown`, with a problem code and the `#id` at fault, when:

- an edge's curve is neither a line nor a circle (`curve_not_read`);
- a circle's axis is not parallel to the normal, or its centre is off the plane, by more than ε at its radius (`circle_not_parallel`);
- a loop is a single vertex (`vertex_loop`; added in PR 4);
- an arc's two distinct vertices are within u of each other, so its sweep is ambiguous (`ambiguous_arc`);
- not exactly one loop has positive area (`loop_orientation`).

The plane, its normal and offset, and the face id stay. In the 48 fixtures under 16 MB, 475 of 9,397 planar faces (5.1%) have an unknown outline, almost all from B-spline or ellipse edges. On the 916 accepted leaf parts of all 55 files, 7,794 of 142,581 planar faces (5.5%) have an unread edge.

### 7.4 What M3 and M4 need, and the contract they must keep

- Placement: `n' = R n` and `offset' = offset + n' · t`.
- Opposed coplanar faces: `n_T · n_N ≈ −1` and `offset_T + offset_N ≈ 0`.
- Grip length: the distance between opposed parallel planes, `offset_1 + offset_2`.
- Which planes a hole passes through: inner loops whose `across` is that hole's cylinder.
- **M3 (socket, Soundness rule 3):** an unknown outline on either side of an opposed coplanar pair counts as overlapping, so a seating requirement is added and the user can remove it in review.
- **M4 (verification, Soundness rule 4):** a candidate face with an unknown outline covers nothing, so a requirement that needs coverage fails.

## 8. Cylinders, cones and tori

Notation: a cylinder group has a reference axis line L through P with unit direction z, a radius r, and frame axes x and y. The station of a point Q is `t(Q) = (Q − P)·z`.

### 8.1 Grouping (G)

- **G1. Same surface.** Two faces are on the same cylinder when they have the same `face_sense`, `|r₁ − r₂| ≤ ε`, each axis point is within ε of the other's line, and `|z₁ × z₂|·ℓ ≤ ε`. The sign of z is ignored.
- **G2. Groups are edge-connected.** A group is a connected component of "shares an edge and is on the same surface", built by union-find with the lowest face index as root. Its reference surface is that of its lowest-index face.
- **G3. Across one end station,** plane faces met at that station that have the same outward normal (`n_c · n_f > 0`, one direction class of section 7.2) are taken as one group even if they do not touch. A plane that contains an arc of a coaxial circle is that circle's plane, so they are coplanar. Without this, 84 hole ends meeting a top face split by the hole itself would be `Unknown`. Faces of opposite normal at one station are never joined: a hole half open and half capped then meets two groups and its end is `Unknown` (section 18, H1). The probe did not implement G3; PR 5 measures it.
- **G4. Seams and split faces need no special case.** A seam edge, and the line or arc between two halves, have all their uses inside the group, so they are internal and every later rule ignores them. The fixtures write full cylinders three ways:

| How a full cylinder is written | Groups | Where |
| --- | --- | --- |
| Two or more faces, up to 75 | 8,808 | AS1 ac/pe, NIST AP242, every Creo file |
| One face, two closed circle loops, no seam | 507 | AS1 md/ug, clevis21, valve, NIST AP203 |
| One face with a seam | 29 | NIST stc_09, and every Open CASCADE part |

  Grouping is therefore the normal path. Open CASCADE's seam style, which the generated parts use, is the rare one (section 13.3).
- **G5. Coaxial is not same.** Two coaxial groups of equal radius that share no edge stay separate features: a clevis gives two holes, and two blind holes on one axis are two holes, not one through hole. There are 1,325 such pairs in the fixtures. Joining them along an axis is M3's job.

### 8.2 Hole wall or shaft wall (S)

- **S1.** `face_sense` false means the outward normal points at the axis and material lies outside r: a **hole wall**. True is a **shaft wall**. `face_sense` already composes `same_sense` with an `oriented_closed_shell`.
- **S2.** A group's faces cannot disagree, because sense is part of G1. Adjacent faces on one cylinder with opposite senses are check 27, a contradiction.
- **S3.** A cylinder in a void shell keeps its sense and shell index, but never joins a hole or shaft.

### 8.3 Full or partial (F)

For a group G:

- A **boundary edge** is used by a face of G and by a face outside it.
- An **end arc** is a boundary edge whose curve (`basis()`) is a circle lying on the cylinder: `|n × z|·r ≤ ε`, its centre within ε of L, and `|r_c − r| ≤ ε`. Its station is `t(centre)`, from the stated centre.
- An **axial line** is a boundary line along the cylinder.
- Every other boundary edge is **other**: a non-axial line, an off-cylinder circle, or `Curve::Other`.

The rules:

- **F1. Stations.** Sort end arcs by station, starting a new station where the gap exceeds ε.
- **F2. A full station** is either one closed edge, or open arcs whose vertices each occur exactly twice and form one closed chain whose sweeps sum to 2π within 1e-9 rad.
  - Sweeps use `libm::atan2` in the group's frame. An edge runs counterclockwise about z iff `same_sense == (n·z > 0)`.
  - Consecutive arcs share vertices, so the sum telescopes, and a vertex off its circle cannot change it.
  - The union of the arcs' angular intervals must also cover 2π, with gaps no wider than ε/r. The sum alone would accept two arcs over the same half circle in opposite directions.
  - A chain that sums to 4π is check 29.
  - 18,821 stations are full by this rule, and the union test agrees on every one.
- **F3. Seam proof (withdrawn in review of PR 5).** A seam was to prove a face wraps 2π around the axis where the seam runs. It does not: a seam used twice by one loop shows only that the face lies on both sides of the seam's line. A slit along the axis, on the cylinder, is used the same way, and a 0.6 rad strip with such a slit read as full; on p25 the claimed extent overran the real wrap by 3.5 mm. Telling a seam from a slit needs the curves on the surface (pcurves), which stepq does not read. As first written, F3 applied when all of these held:
  - the edge's curve (`basis()`) is a line along the axis, on the cylinder (the lever-arm test of section 5.2);
  - both of its uses are in one loop of one face of the group, in opposite directions;
  - the group's boundary has no axial line and no station that is not full.

  The group is then full over the seam's span, from its two vertices' stations, recorded as `proven_extent_mm`. F3 never overrides `partial`. A bridge or slit edge, also used twice by one face, fails the first condition. All 29 cylinder seams in the 55 fixtures meet all three conditions. (This rule was added during the merge, for R8, and guarded after review; the probe did not run it.)
- **F4. Classification.** Let `[s_lo, s_hi]` be the lowest and highest stations, and V the vertices of the group's "other" edges.
  - **Partial:** any axial line on the boundary, or any station not full. The largest station sweep is recorded.
  - **Full:** every station full, at least one station, and V within `[s_lo − ε_v, s_hi + ε_v]`. Vertices of V strictly inside the range are **wall openings**, such as a cross hole entering the wall. Their count is reported, and 164 full groups in the fixtures have them.
  - **Full, one end irregular:** every station full, and V extends beyond exactly one side. That end is `Irregular`.
  - **Full by seam** (withdrawn with F3): a group with no station is `undecided`, `no_end_circle`.
  - **One station** (added in review of PR 5): the wall runs past it on the side its arcs face (counterclockwise about z for a shaft means the wall lies above, as in check 25), so that end is irregular even when a closed curve's one vertex touches the station. With two or more stations, the lowest must face up and the highest down, or the group is `undecided`, `ends_disagree`.
  - **Undecided:** everything else, with a reason such as "coverage not stated: bounded by B-spline".
- **F5. Unread curves are never interpreted.** A B-spline that happens to be a circle is still `Other`. The AS1 export from CAS.CADE writes every circle as a rational B-spline, so all 14 of its cylinders are undecided, while four other exports of the same model give 11 holes and 3 shafts each.

Results on the fixtures:

| Class | Groups |
| --- | --- |
| Full (both ends) | 9,199 |
| Full, one end irregular | 145 |
| Partial: 21,269 concave, 22,395 convex | 43,664 |
| Undecided | 889 |

The 9,344 full groups are 7,652 hole-sense and 1,692 shaft-sense; 145 of them (26 hole-sense, 119 shaft-sense) are in voids, leaving 7,626 holes and 1,573 shafts.

The undecided groups:

- 785 are bounded by B-splines and lines that are not on the cylinder within ε;
- 39 are bounded by B-splines only;
- 14 by rational B-splines;
- 44 by B-splines and off-cylinder circles;
- 7 by ellipses.

Partial cylinders by their largest swept angle:

- 33,582 sweep 90° or less;
- 5,347 sweep 90° to 180°;
- 699 sweep more than 180°;
- 4,036 have no end circle.

### 8.4 What a cylinder feature holds

- `id`, `solid`, `shell`, `faces`, `surface` (the reference face's surface `#id`), `sense` (`hole` or `shaft`);
- `axis`: a point on L and the direction z, with the sign chosen as in section 9.4;
- `radius_mm` and `spread_mm`, the largest G1 deviation of a member from the reference;
- `class`: `full`, `full_one_end_irregular` (with `irregular`), `partial` (with `sweep_rad`, or `null` with no end circle) or `undecided` (with a reason);
- `stations_mm`: the stations, as distances along the axis;
- `wall_openings`: a count;
- `near_misses`: the ids of the cylinders it nearly joins; each shared edge also gets a `near_miss` note.

### 8.5 The fixtures, by exporter

The same model exported by different systems gives the same holes wherever circles are stated. Differences come only from ends written as B-splines or ellipses, which become `Irregular` or `Undecided`. Those are false negatives, never false holes.

| Model | Exports and full holes |
| --- | --- |
| AS1 | pe, ac, md, ug: 11 holes and 3 shafts each. ec: 14 undecided (B-spline circles). tc: no cylinders |
| NIST ctc_01 | 10 in each of three exports |
| NIST ctc_04 | 60, 60, and 59 in AP242 (16 with an irregular end) |
| NIST ftc_10 | 42 (14 irregular) and 34 in AP242 (26 irregular, 8 undecided) |

### 8.6 Cones and tori

Cone and torus faces are grouped like cylinders:

- **Cones** group by apex within ε, axis direction (lever-arm rule), and semi-angle within ε/ℓ. The apex is `P − (r / tan α) z`, computed with `libm::tan`.
- **Tori** group by centre and axis within ε and both radii within ε.

Each group is a primary feature. It also appears by id in the hole or shaft that crosses it in its end walk: a countersink, a drill point, an edge round, or a step. A group no walk crosses is still listed, with its axis and parameters.

### 8.7 Other faces

Each sphere face and each face with an unread surface is its own `other` feature, with:

- its shell, its surface `#id` and its entity type, for example `B_SPLINE_SURFACE_WITH_KNOTS`;
- `reason: "no_rule"`.

Spheres are read by stepq, but no M2 rule uses them. A ball end at a hole bottom therefore makes that end `Unknown`.

## 9. Holes and shafts: the end walk

### 9.1 The walk (W)

For a full group, each full end station starts a walk outward. The direction is u = −z at `s_lo` and u = +z at `s_hi`. The current circles are that station's end arcs, and the current radius is r.

- **W1. Next group.** The faces beyond the current circles must form exactly one walk group H. Otherwise the result is `Unknown("n groups beyond")`.
  - A walk group is local: the edge-connected faces of one surface (G2), plus, for planes, G3's same-normal faces at the station. It is never the whole primary plane of section 7, whose disjoint coplanar faces elsewhere on the part would add unrelated edges.
- **W2. Far circles of H** are its coaxial boundary circles at another station or radius. The rest of its boundary is its "other" edges.
- **W3. The first face beyond the end decides air or material inside radius r:**

| First face H | Air beyond when |
| --- | --- |
| Perpendicular plane | its outward normal points along u |
| Coaxial cone | it widens along u for a hole, or narrows for a shaft |
| Coaxial torus | its far radius is larger than r for a hole, or smaller for a shaft |
| Anything else | `Unknown` |

  A perpendicular annulus whose "air" disagrees with its radii is check 28, a contradiction.

  **Every later plane must agree.** Each perpendicular plane the walk crosses or stops at gives its own answer (its outward normal against u). If any answer differs from the first face's, the end is `Unknown("air then material")`. Without this, a blind hole with a conical undercut, whose chamber floor closes it, reads as through (section 18, C2).
- **W4. Continue or stop:**

| H | Action |
| --- | --- |
| Perpendicular plane, no far circle and no other edge (a disk) | Stop: terminal `disk` |
| Perpendicular plane whose remaining edges are all far circles (an annulus) | Continue; r becomes the far radius |
| Perpendicular plane with other edges | Stop: terminal `face` |
| Coaxial cone narrowing along u that closes at its apex and has no other edges | Stop: terminal `apex` |
| Coaxial cone or torus with only far circles beyond | Continue |
| Coaxial cone or torus, anything else | `Unknown` |
| Coaxial cylinder of the same sense, after at least one transition | Stop: `Step(H)` |
| Coaxial cylinder of the opposite sense | Stop: terminal `post` (a boss, or a bore through a boss) |
| Sphere, non-coaxial surface, unread surface | Stop: terminal `face` if the walk has crossed only planes. `Unknown` if it is the first face, or if a cone or torus was crossed before it, since no plane then confirms what lies beyond |

  A cone closes at its apex when one of its vertices is within ε_v of the apex, when it has a vertex loop, or when its whole boundary is the current circles. The last case occurs in NIST ftc_07, a drill point bounded by one closed circle. As built in PR 6, after review: the cone closes when nothing but the current circles bounds it (a vertex loop at the apex is not an edge), its apex lies ahead along u, and no plane was crossed first; a dimple in a flat floor is then `unknown`, not a point, and the tip length is measured to the apex, not taken as r / tan α from the wall.
- **W5. The end, from W3 and W4:**

| Group | Beyond | Terminal | End |
| --- | --- | --- | --- |
| hole | air | `face` or `post` | `opening` with its rim |
| hole | material | `disk` | `bottom: flat`, or `cone_then_flat` if a cone was crossed |
| hole | material | `apex` | `bottom: point`, with the included angle and the tip length |
| hole | material | `post` | `bottom: annular`, with the post radius |
| shaft | air | any | `free_end` with its rim |
| shaft | material | `face` or `post` | `root` with its rim |
| any | any | `Step(H)` | `step` to H, if mutual (W6) |
| any | | no full station | `irregular` |
| any | any | any other combination | `unknown` with a reason |

  For example, a hole with material beyond that ends on a general face is `unknown`. The probe met that 16 times.
- **W6. Steps must be mutual.** A step from G to H counts only if H's walk from its facing end returns G. 56 steps in the fixtures are not mutual and stay `unknown`.
- **W7. Bound.** A walk crosses at most 8 transition faces and visits each group at most once.
- **W7b. No turning back** (added in PR 6). The end is `unknown`, `turns_back`, when a perpendicular plane the walk meets lies back along the axis from the walk's start or from a plane it met before (a pocket's floor, then the face around the pocket), or when the first face beyond is a cone or torus whose far circle lies back along the axis (a conical washer's face from its bore: W3 would read its widening as air or material by the cone's own axis, and once read a through bore as blind). A cone or torus after a plane may lead back: a round down to a boss's wall after its top (C3) meets no plane behind.
- **W8. Rim.** The rim is the station of the last perpendicular plane the walk crossed or stopped at: the plane the hole opens onto, or the shaft rises from. If the walk met no perpendicular plane, the rim is the last circle crossed:
  - a plain hole: the face it opens onto, at its own end circle;
  - a countersink: the face the cone opens onto, at the cone's outer circle;
  - a bore through a boss: the boss top, even when the boss edge is rounded or chamfered beyond it.

  "The last circle crossed" alone would land on a neighbour's round: a bore through a boss with a 1 mm round reads 15 mm long instead of 16, and a post in a filleted pocket 2 mm high instead of 3 (section 18, C3).

Hole ends in the fixtures, as A's probe counted them (15,304 ends of the 7,652 hole-sense full groups, 26 of them in voids). The probe did not implement G3, the plane-agreement rule or the new rim: about 84 of the 108 unknown ends are G3 cases the rules resolve, and 21 of the 291 bottoms close onto a general face, which W5 makes unknown. PR 6 re-measures.

| Opening | Step | Point | Flat, cone then flat, or annular | Irregular | Unknown | In a void |
| --- | --- | --- | --- | --- | --- | --- |
| 13,821 | 549 | 346 | 291 | 137 | 108 | 52 |

### 9.2 Stacks (K)

- **K1.** Full hole groups in the outer shell that are joined by mutual steps form a stack. A stack must be a chain with exactly two non-step ends; otherwise its kind is `unknown`.
- **K2. Kind:**
  - `through`: two openings;
  - `blind`: one opening and one bottom;
  - `enclosed`: two bottoms;
  - `unknown`: any irregular or unknown end.
- **K3. Treatments are read off the walk, not inferred:**
  - **counterbore:** a larger segment joined to the next smaller one by an annulus;
  - **countersink:** the opening is reached through a widening cone, with the included angle and the outer diameter;
  - **step drill:** two segments joined by a cone;
  - **edge round:** the opening is reached through a torus.
  - A spotface is a short counterbore. Telling them apart is a depth threshold, which is policy, so it belongs to the M4 signature.

The fixtures have 7,361 stacks:

| Through | Blind | Enclosed | Unknown |
| --- | --- | --- | --- |
| 6,488 | 611 | 7 | 255 |

- **Through:** 5,035 plain, 1,185 with countersinks, 127 with an edge round, 137 with several segments.
- **Blind:** 346 with a drill point, 154 flat, 23 cone then flat.

### 9.3 Lengths (E)

- **E1.** A stack's axis is that of its outermost segment. Every station is a stated circle centre projected on it, never a vertex. Of 248,330 end-arc vertices, 4,684 lie more than 1e-6 mm off their stated circle, 40 of them more than 1e-2 mm, almost all in Creo files.
- **E2. Through length:** from the rim at one opening to the rim at the other.
- **E3. Blind depth:** from the opening's rim to the end of the deepest full-diameter wall. This is the ISO and ASME convention for a drilled hole: it includes any counterbore or countersink and excludes the drill point. For a point, the included angle 2α and the tip length `r / tan α` are reported separately, along with the depth to the tip.
- **E4. Shafts mirror this.** Each segment has its extent. A shaft's protrusion runs from its root's rim to its free end's rim. A pin with two free ends, or a bar between two roots, reports the length between them.

### 9.4 Direction and frame of a hole or shaft

- **Blind holes** point from the bottom to the opening, out of the material.
- **Shafts** point from the root to the free end.
- **Through holes, enclosed and unknown stacks, and shafts with two free ends or two roots** use a canonical sign: the largest absolute component is positive, with ties broken in the order x, y, z.
- **Positions:** `axis.point_mm` is the axis point at the start end. Every station in the feature is a distance along `axis.direction` from it.

### 9.5 What a hole or shaft holds

- `id`, `solid`, `kind`, `axis`. A hole's kind is `through`, `blind`, `enclosed` or `unknown`. A shaft's is `free_free` (a pin), `root_free` (a boss or stub), `root_root` (a bar between two faces) or `unknown`;
- `segments`: in order along the axis, each with its cylinder id, `diameter_mm`, `from_mm` and `to_mm`;
- `transitions`: the cone, torus and annulus plane ids the walk crossed, in order, not counting the terminal face;
- `ends`: two entries, each with its end kind (section 9.1, W5), `at_mm`, and its fields: the rim, `closed_by` or `opens_into` (the `FeatureId` of the terminal face, which may be a plane, a cylinder for a `post`, or an `other` face), the point angle and tip length, the post radius;
- `depth_mm` (blind) or `length_mm` (through), or `null` when an end is unknown or irregular;
- `wall_openings`.

### 9.6 Unknowns, and what M3 and M4 must do with them

Every cylinder group ends in exactly one class: hole segment, shaft segment, void, partial, or undecided. A full hole whose end is unknown or irregular stays a hole with that end. Its stack is `unknown` and its depth `null`.

`opening` means only that air lies beyond the end. It does not mean a fastener can pass; clearance is M4's question, answered from diameters along the whole axis.

`part.complete` is true iff no cylinder is undecided and no end or stack is unknown or irregular. It is information, not a refusal (R6).

- **M3:** a target hole with an unknown end or `null` depth gives no depth requirement, and the socket says so. An undecided or partial cylinder on the target with a neighbour on its axis is a requirement the socket cannot express, so socket extraction refuses or flags it (Soundness rule 3).
- **M4:** a candidate's undecided, partial or unknown features never satisfy a requirement. They can cost only false negatives. An `undecided` cylinder, such as an angled hole whose ends are ellipses, may at most locate an axis and diameter for review; it never satisfies a requirement.

### 9.7 Shafts and bosses

The rules mirror holes. A bore through a boss is two features: a shaft (the boss, with its root on the plate and a free end at its top) and a hole whose opening is at the boss top.

Threads, knurls and splines are not cylinders under F4. They are partial or other, never shafts. Threads are not confirmed geometrically (PLAN.md, Metadata from STEP only).

## 10. The JSON format: `features.v1`

### 10.1 Envelope

| Field | Meaning |
| --- | --- |
| `format` | Always `"fitment.features"` |
| `format_version` | `1`. Frozen at the first publish (M4). Before then it may change, together with `extractor_version` |
| `extractor_version` | `EXTRACTOR_VERSION`, a `u32` starting at 1. Bumped whenever the output for any input could change. The catalog re-indexes on it (M5) |
| `extraction` | Every constant of section 5.2 |
| `header` | The file's recorded facts (section 4.2): `schema` (the family), `schema_identifiers` (every one, as written), `originating_system`, `preprocessor_version`, `implementation_level`. Present for refused documents too, `null` where the file could not be parsed |
| `status` | `"accepted"` or `"refused"` |
| `refusals` | Empty when accepted; sorted and non-empty when refused |
| `part` | The part, or `null` when refused. A refused document never lists features |

Core refuses to read a document with another `format` or `format_version`. It reads any `extractor_version`, and `FeaturesDocument::is_current()` tells the catalog whether to re-index.

### 10.2 Conventions

- **Field names** are snake_case, with units as suffixes (`offset_mm`, `area_mm2`, `sweep_rad`).
- **Points** are `[x, y, z]` arrays in mm named `*_mm`. Directions are unit arrays with no suffix.
- **STEP references** are bare integers; feature ids are strings.
- **Enums with data** are internally tagged with struct variants only: `kind` for shapes and classes, `status` for outcomes, matching `FitStatus`. **Fieldless enums** (`sense`, a hole's `kind`) are snake_case strings.
- **Optional values** are written as `null`, never skipped, so every document of one version has the same keys.
- **Reading** uses `deny_unknown_fields` and checked raw types (finite values, unit directions, chained segments, sorted ids). As built in PR 4, after review, `from_json` also refuses a part whose features break what `read_part` guarantees: a face in no primary feature or in two; a list or a feature's faces out of order, or an id not named after its smallest face; a feature of another solid or shell; face counts that differ from the lists; an outline whose first loop is not the one positive outer loop, whose segments do not chain and close, whose arcs sweep beyond ±2π, or whose `across` names no feature of the part; an area that is not its outline's, or a plane's that is not its faces' sum; unsorted notes. Every document of the unit set, the corpus and the fixtures reads back.

### 10.3 The part

`part` holds:

- `definition`: the `#id`, and the product's `#id`, id and name;
- `frame`: the shape representation features are stated in; with several listings of the one solid, the smallest `#id`;
- `solids`: each solid's `#id`, entity, the `representation` that lists it, transform, shells, face count, units and `max_vertex_offset_mm`;
- `max_uncertainty_mm`, `complete`, `face_counts`;
- the primary features: `planes`, `cylinders`, `cones`, `tori`, `other_faces`;
- the compound features: `holes`, `shafts`;
- `other_items` and `notes`.

### 10.4 Example

This is a 60 × 40 × 10 mm plate generated with Open CASCADE 8.0.1. It has two Ø6.6 through holes at (15, 20) and (45, 20), and a Ø5 blind hole 6 mm deep at (30, 30). The `#id`s, coordinates and areas are what stepq reads from that file. Four of the seven planes are left out to save space.

```json
{
  "format": "fitment.features",
  "format_version": 1,
  "extractor_version": 1,
  "extraction": { "identity_mm": 1e-6, "parallel_sine": 1e-9, "vertex_side_mm": 1e-3, "lever_min_mm": 1.0,
                  "sweep_rad": 1e-9, "walk_max": 8, "gate_slack_mm": 1e-6, "gate_slack_rel": 1e-6, "range_mm": 1e6 },
  "header": { "schema": "ap214", "schema_identifiers": [ "AUTOMOTIVE_DESIGN { 1 0 10303 214 1 1 1 1 }" ],
              "originating_system": "Open CASCADE 8.0", "preprocessor_version": "Open CASCADE STEP processor 8.0",
              "implementation_level": "2;1" },
  "status": "accepted",
  "refusals": [],
  "part": {
    "definition": { "instance": 5, "product": { "instance": 7, "id": "Open CASCADE STEP translator 8.0 1", "name": "Open CASCADE STEP translator 8.0 1" } },
    "frame": { "shape_representation": 10 },
    "solids": [ { "id": "solid-15", "instance": 15, "entity": "MANIFOLD_SOLID_BREP", "representation": 10, "transform": null,
                  "shells": 1, "faces": 10,
                  "units": { "context": 12,
                             "length": { "instance": 13, "name": "millimetre", "factor": 1.0 },
                             "plane_angle": { "instance": 14, "name": "radian", "factor": 1.0 },
                             "uncertainty": [ { "instance": 11, "name": "distance_accuracy_value", "value_mm": 1e-7 } ] },
                  "max_vertex_offset_mm": 0.0 } ],
    "max_uncertainty_mm": 1e-7,
    "complete": true,
    "face_counts": { "plane": 7, "cylindrical": 3, "conical": 0, "spherical": 0, "toroidal": 0, "other": 0, "total": 10 },
    "planes": [
      { "id": "plane-213", "solid": "solid-15", "shell": 0, "normal": [0.0, 0.0, 1.0], "offset_mm": 10.0, "spread_mm": 0.0,
        "area_mm2": 2311.9411579198786,
        "faces": [ { "face": 213, "surface": 126, "offset_mm": 10.0, "area_mm2": 2311.9411579198786,
          "outline": { "status": "known", "loops": [
            { "bound": 214, "outer": true, "area_mm2": 2400.0, "segments": [
              { "kind": "line", "edge": 112, "from_mm": [0.0, 40.0, 10.0], "to_mm": [0.0, 0.0, 10.0], "across": "plane-17" },
              { "kind": "line", "edge": 165, "from_mm": [0.0, 0.0, 10.0], "to_mm": [60.0, 0.0, 10.0], "across": "plane-137" },
              { "kind": "line", "edge": 219, "from_mm": [60.0, 0.0, 10.0], "to_mm": [60.0, 40.0, 10.0], "across": "plane-483" },
              { "kind": "line", "edge": 242, "from_mm": [60.0, 40.0, 10.0], "to_mm": [0.0, 40.0, 10.0], "across": "plane-355" } ] },
            { "bound": 262, "outer": false, "area_mm2": -34.21194399759284, "segments": [
              { "kind": "arc", "edge": 265, "from_mm": [18.3, 20.0, 10.0], "to_mm": [18.3, 20.0, 10.0],
                "center_mm": [15.0, 20.0, 10.0], "radius_mm": 3.3, "sweep_rad": -6.283185307179586, "across": "cylinder-490" } ] },
            { "bound": 293, "outer": false, "area_mm2": -34.21194399759284, "segments": [
              { "kind": "arc", "edge": 296, "from_mm": [48.3, 20.0, 10.0], "to_mm": [48.3, 20.0, 10.0],
                "center_mm": [45.0, 20.0, 10.0], "radius_mm": 3.3, "sweep_rad": -6.283185307179586, "across": "cylinder-517" } ] },
            { "bound": 324, "outer": false, "area_mm2": -19.634954084936208, "segments": [
              { "kind": "arc", "edge": 327, "from_mm": [32.5, 30.0, 10.0], "to_mm": [32.5, 30.0, 10.0],
                "center_mm": [30.0, 30.0, 10.0], "radius_mm": 2.5, "sweep_rad": -6.283185307179586, "across": "cylinder-544" } ] }
          ] } } ] },
      { "id": "plane-404", "solid": "solid-15", "shell": 0, "normal": [0.0, 0.0, -1.0], "offset_mm": 0.0, "spread_mm": 0.0,
        "area_mm2": 2331.5761120048146, "faces": [ "…two inner loops, across cylinder-490 and cylinder-517…" ] },
      { "id": "plane-599", "solid": "solid-15", "shell": 0, "normal": [0.0, 0.0, 1.0], "offset_mm": 4.0, "spread_mm": 0.0,
        "area_mm2": 19.634954084936208, "faces": [ "…one full circle, across cylinder-544…" ] }
    ],
    "cylinders": [
      { "id": "cylinder-490", "solid": "solid-15", "shell": 0, "faces": [490], "surface": 486, "sense": "hole",
        "axis": { "point_mm": [15.0, 20.0, 0.0], "direction": [0.0, 0.0, 1.0] }, "radius_mm": 3.3, "spread_mm": 0.0,
        "class": { "kind": "full" }, "stations_mm": [0.0, 10.0], "wall_openings": 0, "near_misses": [] },
      { "id": "cylinder-517", "…": "as cylinder-490, at (45, 20)" },
      { "id": "cylinder-544", "solid": "solid-15", "shell": 0, "faces": [544], "surface": 540, "sense": "hole",
        "axis": { "point_mm": [30.0, 30.0, 4.0], "direction": [0.0, 0.0, 1.0] }, "radius_mm": 2.5, "spread_mm": 0.0,
        "class": { "kind": "full" }, "stations_mm": [0.0, 6.0], "wall_openings": 0, "near_misses": [] }
    ],
    "cones": [], "tori": [], "other_faces": [],
    "holes": [
      { "id": "hole-490", "solid": "solid-15", "kind": "through", "axis": { "point_mm": [15.0, 20.0, 0.0], "direction": [0.0, 0.0, 1.0] },
        "segments": [ { "cylinder": "cylinder-490", "diameter_mm": 6.6, "from_mm": 0.0, "to_mm": 10.0 } ],
        "transitions": [],
        "ends": [ { "kind": "opening", "at_mm": 0.0, "rim_mm": 0.0, "opens_into": "plane-404", "countersink": null },
                  { "kind": "opening", "at_mm": 10.0, "rim_mm": 10.0, "opens_into": "plane-213", "countersink": null } ],
        "length_mm": 10.0, "depth_mm": null, "wall_openings": 0 },
      { "id": "hole-517", "…": "as hole-490, at (45, 20)" },
      { "id": "hole-544", "solid": "solid-15", "kind": "blind", "axis": { "point_mm": [30.0, 30.0, 4.0], "direction": [0.0, 0.0, 1.0] },
        "segments": [ { "cylinder": "cylinder-544", "diameter_mm": 5.0, "from_mm": 0.0, "to_mm": 6.0 } ],
        "transitions": [],
        "ends": [ { "kind": "bottom", "bottom": { "kind": "flat" }, "at_mm": 0.0, "closed_by": "plane-599" },
                  { "kind": "opening", "at_mm": 6.0, "rim_mm": 6.0, "opens_into": "plane-213", "countersink": null } ],
        "length_mm": null, "depth_mm": 6.0, "wall_openings": 0 }
    ],
    "shafts": [], "other_items": [], "notes": []
  }
}
```

The `"…"` strings stand for content left out of this example. The real document repeats the full structure.

The `#id`s, plane values and areas were measured: stepq read them from the generated file. The header strings and the cylinders' surface `#id`s are illustrative. The cylinder and hole values follow from the rules and the construction; no probe produced them from this file.

Checkable facts:

- The top's area is 2400 − 2π·3.3² − π·2.5² = 2311.94116 mm².
- The top and bottom are opposed at offsets 10 and 0, so the plate is 10 mm thick.
- The top's inner loops lead to all three holes, and the bottom's to the two through holes only.
- The 7 planes and 3 cylinders hold 10 faces, equal to `face_counts.total`.

A refused document has `"status": "refused"`, its sorted reasons and `"part": null`. For example: `{"code": "assembly", "instance": null, "message": "the file is an assembly with 5 parts that have solids (#20 'PLATE', ...); features reads one part"}`.

## 11. Data model and API in fitment-core

### 11.1 Modules

```text
crates/fitment-core/src/
  lib.rs            re-exports; EXTRACTOR_VERSION
  geom.rs           Point3, Vec3, Dir3, Rigid; finite(); angle helpers on libm
  gate.rs           the checklist (section 4) → Vec<Refusal>       [names stepq]
  features/
    mod.rs          Part, read_part, part_of_definition (pub(crate), for M3), FeatureId, the partition check
    step.rs         the stepq adapter: parse, choose the part, read the solid, convert values   [names stepq]
    group.rs        grouping of planes, cylinders, cones and tori, with the constants of section 5.2
    plane.rs        Plane, PlaneFace
    outline.rs      Outline, loops, segments, signed area, checks (reused by socket/ at M3)
    cylinder.rs     Cylinder, full or partial (F)
    walk.rs         the end walk (W)
    stack.rs        Hole, Shaft, stacks and lengths (K, E)
    other.rs        Cone, Torus, OtherFace, OtherItem
    document.rs     FeaturesDocument: envelope, to_json, from_json
```

- **stepq stays inside `gate/` and `features/step.rs`.** As built in PR 4: gate stage 6 copies the solid that passed stages 4 and 5 into a crate-private `Topo` (`features/topo.rs`): faces with their surfaces in fitment's types, `face_sense` and `same_sense`, bounds with their loops already in traversal order (section 5.3), edges with their curves (`basis()`) and the faces that use them, shells and vertices, each with its `#id`. Checks 22 (the rest), 26, 23, 24 and 25 then run on the copy (`gate/geometry.rs`), so the gate's orientation check and the plane outlines measure loops with one implementation (`Travel::area`). No module outside `gate/` and `features/step.rs` sees a stepq value. A CI grep checks that `stepq` appears in no other file, including in `use` lines, so a re-export cannot slip through. stepq stays a private dependency, so stepq 0.7 is a one-crate change.
- **No panics.** stepq's `face_sense`, `uses` and `neighbours` panic on an out-of-range index, so `step.rs` calls them only with indices stepq returned for the same solid. An invariant that "cannot fail" returns `internal`.
- **Dependencies.** stepq 0.6 is added by fitment #4 (M2, step 1): at workspace level with `default-features = false`, allowlisted in `deny/core.toml` with its `default` and `cli` features banned, and wrapped to `fitment-core` in `deny.toml`. `libm` 0.2 (rust-version 1.63) is added the same way in PR 4, where trigonometry is first used, at workspace level and in `deny/core.toml`, with the reason "bit-identical trigonometry on every OS".
- **Packaging:** `include = ["src/**", "README.md", "LICENSE"]`, so `tests/data/*.stp` never ships (it would today).

### 11.2 Public API

```rust
pub const EXTRACTOR_VERSION: u32 = 1;

/// Reads the one part a STEP file holds and extracts its features. Bytes in, values out.
/// Every way the input can fail is a refusal with reasons; nothing panics.
pub fn read_part(step: &[u8]) -> Result<Part, Refused>;

// As built in PR 1, after review: a FeaturesDocument comes only from `read` or
// `from_json`, its fields are private with accessors, and the file's header lives in
// the document, not in Part, so no caller can build a document `from_json` would reject.

#[non_exhaustive]
pub struct Part {
    pub definition: DefinitionRef,
    pub frame: Frame,
    pub solids: Vec<SolidInfo>,         // serde writes fields in this order; the example follows it
    pub max_uncertainty_mm: f64,
    pub complete: bool,
    pub face_counts: FaceCounts,
    pub planes: Vec<Plane>,
    pub cylinders: Vec<Cylinder>,
    pub cones: Vec<Cone>,
    pub tori: Vec<Torus>,
    pub other_faces: Vec<OtherFace>,
    pub holes: Vec<Hole>,
    pub shafts: Vec<Shaft>,
    pub other_items: Vec<OtherItem>,
    pub notes: Vec<Note>,
}

pub struct FeaturesDocument { /* section 10.1; private fields */ }
impl FeaturesDocument {
    pub fn read(step: &[u8]) -> Self;             // the document for a file, accepted or refused
    pub fn status(&self) -> Status;               // and format(), header(), refusals(), part(), ...
    pub fn to_json(&self) -> String;              // pretty, ends with "\n"
    pub fn from_json(text: &str) -> Result<Self, FormatError>;
    pub fn is_current(&self) -> bool;
}
```

- **Fields are `pub` and the structs `#[non_exhaustive]`.** Other crates can read them but not build them, so every `Part` comes from `read_part` or from checked JSON.
- **`FeatureId` is a newtype,** `{ kind, face }`, ordered by face number. It serializes as `"<kind>-<face>"`.
- **Errors:** `Refused { refusals }` from `read_part`, whose `Display` gives the first reason and the count. `FormatError` covers reading a document back. `geom` constructors return `Option`.
- **`geom`:** `Point3`, `Vec3` and `Dir3` are finite by construction and never `-0.0`. `Dir3::from_unit` keeps a stated direction's bits when its length is within 1e-12 of 1. `Rigid` uses stepq's convention (`p_parent = R · p_child + t`), so the adapter copies values without arithmetic.

## 12. The CLI: `fitment features`

```text
fitment features <FILE> [--format json|table]
```

- **Input:** the CLI reads the file; core only sees bytes. Reading stdin is not in M2.
- **`--format json`** (the default) prints the document for accepted and refused input alike, so scripts can parse the reasons.
- **`--format table`** prints one line per feature for people. It is not a stable format.
- **stderr** carries human messages only: one line per refusal reason, or the error.
- **Exit codes:**

| Code | Meaning |
| --- | --- |
| 0 | Accepted |
| 1 | Error: the file cannot be read, stdout cannot be written, or an `internal` refusal, which is a fitment bug. The document is still printed |
| 2 | Usage error |
| 3 | Refused: the input is not a canonical part |

- Code 3 means "refused" for every command; M4 picks the codes for `Match`, `InterfaceMatch` and `Reject` from 4 upward.
- A broken pipe exits 0 quietly, as stepq's CLI does.
- The CLI never names stepq.

## 13. Test parts with known answers

### 13.1 Generator and manifest

- **`tools/gen-corpus.py`** is a uv script on cadquery 2.8.0 (Open CASCADE 7.9.3), with exact pins and a committed `.lock`, run with `uv run --locked --script`. Locking and running were checked on macOS arm64; the locks hold wheels for macOS, Linux (x86_64, aarch64) and Windows.
- **Every positive part is built from explicit primitives:** boxes, cylinders, cones, tori and one prism, except p26 to p28, which test unread surfaces. Every expected value is therefore a construction parameter.
- **Tool overshoot.** A cutting tool overshoots the material by at least r·tan(tilt) + 1 mm, so no tool face survives. The oracle caught a part that broke this.
- **The manifest** (`manifest.json`) states, per case:
  - `id`, `file`, `set` (unit or corpus);
  - `tests`: one sentence;
  - `rules`: the rule ids it exercises, such as `G5`, `F4`, `W4.apex`, `K3.counterbore`, `7.3` or `4.2#11`;
  - `construction`: the arguments, so a reviewer can recompute any value;
  - `expect`: `features` with the expected features, or `refused` with a reason code.

  Expected values come only from the generator's arguments and Python arithmetic, never from reading a written file, and are rounded to 9 decimals.
- **Coverage is checked, not claimed.** The manifest lists `rule_ids`: every rule and gate check this document defines, each with its cases or marked `deferred` with a reason. A test fails if a listed id has no case and is not deferred. Every W5 row that yields `unknown` needs a must-be-`unknown` case. Gate checks that no generated part can reach (checks 2 to 4, 6, 8, 13, 14, 16, 17, 19 to 22 and 26) get hand-written STEP snippets in the Rust tests.
- **Generation:** 33 files take 1.8 s. Two runs on one machine were byte-identical apart from the `FILE_NAME` timestamp. The generator must write a fixed timestamp; cross-platform byte equality is untested (section 19).

### 13.2 How tests compare

- **Tolerances:** lengths within 1e-6 mm; directions and cone angles within 1e-9 rad.
  - Open CASCADE writes values back with noise near 1e-13 at these sizes, so 1e-6 never fails on rounding.
  - The near-tolerance parts differ by 0.005 to 0.02 mm. A test tolerance of 0.01 mm (D9) would let p23 or p24 pass with the wrong answer. This tolerance checks the extractor, not a match.
- **Holes are matched one to one** by axis line and position along it, which separates the two coaxial holes of p22. A hole missing on either side fails, and so does an extra one.
- **Through holes** are compared under the canonical sign of section 9.4.
- **Stacks:** segment count, diameters, from/to, the end kinds and the depth. Labels such as "counterbore" are compared only where they are geometric (K3).
- **Planes:** a multiset of (normal, offset, inner-loop count). Outer-loop vertices are compared where the loop is a polygon. Areas are compared against closed forms.
- **The face partition** is checked on every case.
- **Determinism:** each case runs twice, and the two outputs must be byte-identical.

### 13.3 The parts

"U" means the part is committed to the unit set; "G" means it is generated in CI only. Sizes are measured, written without pcurves.

| id | Construction | Expected | Set | Bytes |
| --- | --- | --- | --- | --- |
| p01_through | box 40×30×8; Ø6.6 through at (20, 15) | through hole, length 8; 6 planes | U | 8,336 |
| p03_blind_flat | box 40×40×20; Ø8 to depth 12 | blind, flat, depth 12; floor plane at z = 8 | U | 8,564 |
| p04_blind_drill | as p03, plus a 118° point | blind, point 118°, depth 12, tip 4/tan 59° = 2.40344 | U | 8,973 |
| p05_cbore | box 40×40×15; Ø11×6.5 over Ø6.6 through | through stack along +z from z = 0: Ø6.6 0–8.5, Ø11 8.5–15; annulus at z = 8.5 | U | 10,031 |
| p06_csink | box 40×40×10; 90° countersink to Ø12.4 over Ø6.6 | through, opening through a 90° cone, outer Ø12.4 | U | 9,352 |
| p07_spotface | Ø16×0.5 over Ø6.6 through | a counterbore 0.5 deep | G | 10,025 |
| p08_multi | three stacks: plain, counterbored, counterbored blind with a point | three holes as built | G | 15,271 |
| p09_pin | cylinder Ø6, length 20 | a shaft with two free ends, length 20 | U | 3,548 |
| p10_boss_hole | plate with boss Ø12 × 10; Ø6.6 through both | shaft Ø12 (root and free end); hole through 16, opening on the boss top | U | 10,003 |
| p11_fillet_slot | L block with a R3 fillet, an obround slot, one Ø6.6 hole | 1 hole; fillet 90° and two slot ends 180°, all partial | U | 22,570 |
| p12_edge_notch | Ø8 through, cutting a plate edge | no hole; a partial concave cylinder of 240° | U | 10,771 |
| p13_cross | Ø8 along x and Ø6 along y, crossing | Ø8 through, length 30, with wall openings. Ø6: two cylinders (G5), each with one opening and one irregular end, length `null` (R8) | G | 29,545 |
| p13b_cross_equal | as p13, both Ø8 | four Ø8 cylinders, two per bore (G2: the halves meet only at two points, sharing no edge), each with one opening and one irregular end; stacks `unknown`, length `null` | G | 13,441 |
| p14s_rotated_cbore | p05 rotated 37° about (1, 2, 3) and moved | p05's answers, moved | U | 12,920 |
| p14_rotated | p08 under the same motion | p08's answers, moved | G | 20,016 |
| p16_inch | p05 written in inches | p05's answers, in mm | U | 11,246 |
| p17_split_halves | p05 with each cylinder split in two faces (Open CASCADE's `ShapeDivideClosed`) | p05's answers | G | 12,856 |
| p18_split_quarters | p05 split in four (`ShapeDivideAngle`) | p05's answers | G | 18,492 |
| p20_split_surfaces | p17 with the second half's placement moved 7 mm along the axis, reversed and turned 90° (one documented text edit) | p05's answers: grouping ignores the placement and the axis sign | U | 13,165 |
| p21_planes | box with a channel and a 45° chamfer | 10 planes over 11 faces: z = 20 holds two faces (x 0–20 and 40–56); one normal is (1, 0, 1)/√2 at offset 76/√2 = 53.74012 | U | 13,025 |
| p22_coaxial_blind | two Ø6×4 blind holes on one axis from opposite faces; a second pair 0.02 mm off axis | 4 blind holes, no through hole | U | 13,579 |
| p23_thin_floor | Ø6 to depth 9.995 in a 10 mm plate | blind, depth 9.995 | G | 8,611 |
| p24_radius_step | Ø10.02×8 over Ø10.00 through | through stack along +z from z = 0: Ø10.00 0–12, Ø10.02 12–20, joined by a 0.01 mm annulus | G | 10,027 |
| p25_angled | Ø6 through, tilted 30° | cylinder class `undecided` (`no_end_circle`), axis and Ø6 exact; hole kind `unknown`, both ends irregular, length `null` (R8). It was `full_by_seam` until review of PR 5 withdrew F3 | U | 8,556 |
| p26_all_bspline | p01 converted to NURBS | accepted: 0 planes, 0 holes, 7 other faces (R1) | U | 12,254 |
| p27_freeform | a profile with a spline top, a Ø6.6 hole away from it | accepted: the hole is read, the spline face is other | G | 9,037 |
| p28_void | 20 mm cube with a Ø8 spherical void | accepted: the sphere is an other face in shell 1, no hole (R5) | G | 7,338 |
| p29_mapped | a solid placed in its definition through a mapped item with transform T (text edit) | p01's answers moved by T, which `construction` states; `solids[0].transform` is T | G | 8,600 |
| p30_round_bore | plate 10 mm, boss Ø16 × 6 with a 1 mm round on its top edge, Ø4 bore through both | hole through, length 16 (rim on the boss top, W8); boss shaft `root_free`, protrusion 6 | U | (to measure) |
| p31_fillet_post | Ø16 pocket 8 deep with a 1 mm floor fillet, Ø4 × 3 post on its floor | post shaft `root_free`, protrusion 3; the pocket is blind with an annular bottom whose wall ends at z = 13 and whose floor (`floor_at`) is at z = 12; the fillet is a torus | G | (to measure) |
| p32_torus_cbore | Ø11 counterbore over Ø6.6 through, with a 0.5 mm fillet on the counterbore floor and a 0.5 mm round at the rim, cut as one revolved tool | through stack; walk crosses torus, annulus, torus (W4); rim on the top face. Filleted with `BRepFilletAPI`, Open CASCADE 7.9 wrote the rim round's torus inside out, which check 15 refuses (found in PR 3) | U | (to measure) |
| p33_step_drill | Ø8 to depth 10, then Ø5 through via a 118° cone | through stack of two segments joined by a cone (K3 step drill) | G | (to measure) |
| p34_cbore_blind | Ø11×4 counterbore over Ø6 blind to 12 with a flat floor, then a 118° point ending on a Ø3 flat | blind; bottom `cone_then_flat` | G | (to measure) |
| p35_annular_bottom | Ø10 blind hole 8 deep around a Ø4 post rising from its floor | blind; bottom `annular`, post radius 2 | G | (to measure) |
| p36_inclined_exit | Ø6 hole from the top whose far end meets a 30° inclined face | one opening at the top, the other end irregular (ellipse); kind `unknown` | G | (to measure) |
| p37_cbore_shoulder | p05 plus a milled shoulder elsewhere, coplanar with the counterbore floor at z = 8.5 | p05's answers: the walk group is local (W1), so the shoulder does not change the counterbore | G | (to measure) |
| p38_void_coplanar | p28 with a box void whose floor is coplanar with a step top, same normal | the void floor is its own plane in shell 1, not part of the step-top plane (section 7.2) | G | (to measure) |
| u01_undercut_floor | Ø10 hole 20 deep; a 90° cone widens it straight onto an Ø16 flat floor that holds an off-axis Ø2 blind hole | must read `unknown` at the bottom end ("air then material": the cone says air, the floor material), never through. With an Ø16 cylinder between cone and floor, the walk would stop at a mutual step and never test plane agreement (found in review of PR 2) | U | (to measure) |
| u02_undercut_post | as u01, the floor an annulus around a Ø4 post | must read `unknown`, never through | G | (to measure) |
| u03_half_capped | Ø6 through hole in a 10 mm plate, half of its top covered by a block | must read `unknown` at the top ("2 groups beyond"), whatever the face order | G | (to measure) |
| u04_ball_end | Ø6 hole ending in a hemisphere | must read `unknown` at the bottom (section 20, item 7) | G | (to measure) |
| u05_drill_pierced | p04 with a Ø3 cross bore at z = 6.4 through its drill point | must read `unknown` at the bottom, not `point` (W4). The bore pierces the cone but not the Ø8 wall, and a strip of its own wall survives below the tip, so it is one through hole with a wall opening | G | (to measure) |
| u06_general_face | Ø6 hole whose bottom is closed by a face that also bounds something else | must read `unknown` (W5) | G | (to measure) |
| r03_two_solids | a compound of two cubes, written with `write.step.assembly` off | refused: `several_solids`. With assembly mode on, Open CASCADE writes a compound as two products under a third (found in PR 1); the generator writes every file with it off | U | 15,526 |
| r04_assembly | `cq.Assembly` of two cubes | refused: `assembly` | U | 15,295 |
| r05_no_length_unit | p01 without its `LENGTH_UNIT` (text edit) | refused: `length_unit_unreadable` | U | 8,319 |
| r06_shell | p01's shell written alone | refused: `no_solid` and `unread_shape_item` | U | 8,347 |
| r12_flipped_cylinder | p01 with the hole face's `same_sense` flipped (one character) | refused: `face_orientation` | U | (to measure) |
| r11_flipped_plane | p01 with the top face's `same_sense` flipped | refused: `face_orientation` | G | (to measure) |
| r08_surface_model | p01 plus a shell-based surface model in the same shape | refused: `unread_shape_item` (R2) | G | (to measure) |
| r09_two_definitions | two products, no usage | refused: `several_definitions` | G | (to measure) |
| r10_off_surface | p01 with one vertex moved 0.1 mm off its plane (text edit) | refused: `vertex_off_surface` and `vertex_off_curve` (the vertex also leaves the edges in that plane) | G | (to measure) |

Every positive part is also generated with pcurves (`_pc`, the way Open CASCADE writes by default) and in the rotated frame (`_rot`), in CI only.

Hand-written STEP snippets inside the Rust tests cover what generated files cannot reach: the contradiction checks (27 to 29), a non-mutual step (W6), a slit cylinder strip that must not be full (once F3's case; now any group with no end circle is undecided), truncated input, and the gate checks listed in section 13.1.

**Split cylinders, honestly.** Every tool available writes through Open CASCADE, so there is no independent second exporter. Three sources stand in:

- Open CASCADE's own documented split operators (p17, p18);
- one documented text edit that changes a placement only (p20). Open CASCADE re-reads p20 as a valid solid with p05's exact volume;
- the NIST fixtures, where most holes are two half faces. In ctc_02, all 77 are.

### 13.4 The unit set

- **Committed in `crates/fitment-core/tests/data/`:** the parts marked U, 24 files, and the manifest, one case per line: 282,934 bytes as generated in PR 2, against the 300,000 cap. p29 moved to the generated corpus to make room. Products are named after their case, because Open CASCADE otherwise numbers them by how many it has written in the process, which made a file's bytes depend on the cases before it.
- **Without pcurves (R9).** Pcurves make the files 2.4 times larger, about 530 KB in all. stepq reads 3D curves, and the `_pc` variants cover pcurves in CI.
- **Guards:**
  - a CLI test fails if `tests/data` exceeds 300,000 bytes, or if its listing differs from the manifest;
  - CI regenerates the set and checks it against the committed copy (`gen-corpus.py --check`): the manifest identical, and each part the same text with every number within 1e-9 mm;
  - `cargo package --list` must show no `.stp` or `.step` file.
- **Core test** `tests/parts.rs` has one `include_bytes!` per file and asserts each part against the manifest. Core tests cannot list directories, because clippy bans `std::fs` in core.

## 14. The oracle: `tools/verify-occt.py`

A uv script pinned to `cadquery-ocp-novtk==8.0.1.1.0`, the M7 helper's pin (decision 8), with a committed `.lock`. It has one function per check, as stepq's does.

As built in PR 2 (`manifest` mode), after review:

- **Every face is accounted for** in every accepted case: cylinder groups, planes with their face counts and shells, the number of distinct cones and tori, and other faces. A stray face a tool left behind is always a disagreement.
- **Cylinder faces are grouped as G2 groups them**, by shared edges, so the oracle cannot agree with an answer the rules would not give. Grouping by touching spans did, for p13b.
- **Sweeps are exact**, from the faces' parameter ranges.
- **Every end of a known hole is tested on both sides**, by classifying points against the solid: air past an opening and material beside its rim; material past a bottom and air inside it. A drill point's or countersink's cone must exist with its apex and angle, an annular bottom's post must have its diameter, and a shaft's root and free end are tested the same way.
- **Refusals are checked from the file text and Open CASCADE's solids**: assembly usages, product definitions, solid count, surface models, a length unit; and the stated stage must be the codes' stage.
- **Not checked, so fitment's own tests are the only check:** a flipped `same_sense` and a vertex moved off its face (Open CASCADE repairs orientation as it reads), and the rules' own labels: `irregular` and `unknown` ends, and whether a hole is stated as `unknown`.
- **Mutation score:** the review's 54 changed answers, 51 caught (the three left are those labels); its 7 deliberately wrong parts, all caught.

The `features` mode below, fitment against Open CASCADE, arrived with PR 4 for planes; the cylinder and hole checks come with PRs 5 and 6.

- **`verify-occt.py manifest`: Open CASCADE against construction.** It catches generator bugs, and it caught one while this was designed: an angled-hole part whose tool left an extra plane, so Open CASCADE read 7 planes where the manifest said 6.
  - Positive cases: one valid solid with positive volume; full concave groups match the expected hole segments; convex groups match the shafts; partial groups match within 0.5°; planes match; axis-line hits give through or blind and the depths.
  - Refusal cases: Open CASCADE's facts (solid count, B-spline count) match the construction. It reads a file with no length unit silently as mm, so r05 is checked against the manifest only.
- **`verify-occt.py features`: fitment against Open CASCADE,** on generated parts and fixtures.
  - **Faces are bound to their `#id`s** as stepq's oracle does: the entity Open CASCADE transferred for each face (`EntityFromShapeResult`), then its rank in the file, then the `#id`. Every face of all 51 files probed was bound.
  - **Per cylinder:** radius and axis agree. Concavity agrees: a point 1 µm outside the wall at mid-face is classified against the solid.
  - **Per hole:** coverage reaches 360° from the faces' parameter ranges. Through or blind and the depth agree, measured by intersecting the axis line with the solid (never from parameter bounds, which were off by 0.025 mm).
  - **Per partial cylinder:** coverage is below 360°.
  - **The other direction:** every full concave group Open CASCADE finds (split by contiguity and by sense) must be a fitment hole unless fitment refused the file. A missed hole is a missed socket requirement later.
  - **Per planar face:** normal, offset and area (`BRepGProp`, expected within 1e-9 relative).
  - **Tallies are pinned** with `--expect`, as stepq does. Known disagreements go in a named, reasoned allow-list.
- **Positive volume:** every accepted solid, generated or fixture, must have positive volume in Open CASCADE. It is the only check on a whole shell written inside out (R11).
- **What the oracle cannot check:** refusal reasons; naming conventions such as depth to shoulder or tip; files that both readers misread the same way, which includes a flipped `same_sense`, since Open CASCADE honours it too (check 25 covers that); faces Open CASCADE heals; and unread faces. Its axis-line test catches a misread end only on parts that exist, which is why section 13.3 has the `u` parts.
- **Cost:** 0.54 s on 31 generated files, 4.8 s on 20 fixture parts.

## 15. Fixtures and CI

### 15.1 Fixtures

- `tools/fetch-fixtures.sh` is copied from stepq, with the same URLs and SHA-256 pins, into `fixtures/`. Only `README.md` and `manifest.json` are tracked.
- M2 uses stepq's `core` set (about 20 MB). The `ocp` set is all assemblies and stays manual.
- **Each file's gate result is pinned** in `fixtures/manifest.json` (JSON, which the tests already read), for example `"features": "refused", "reasons": ["assembly"]`. The first pins, from PR 3, match the census in section 4.2: of the 45 core files, 24 accepted, 9 assemblies, 10 with surface models, 1 tessellated only and 1 with several definitions.
- **Fixture tests assert:**
  - no panic and no `internal` refusal on any file;
  - the pinned gate result;
  - agreement with the oracle on accepted files;
  - pinned counts of holes, shafts, partial cylinders and planes per accepted file. These are regression pins, not truth, and are re-pinned only in a PR that says why.

### 15.2 CI

- **A new `corpus` job** on Ubuntu, estimated under 6 minutes:
  1. fetch the fixtures (cached);
  2. generate the corpus;
  3. regenerate the unit set and diff it against the committed copy;
  4. run `verify-occt.py manifest`;
  5. build `fitment` in release;
  6. run `verify-occt.py features` on the corpus and fixtures, with pinned tallies;
  7. run the corpus and fixture tests with `FITMENT_REQUIRE_CORPUS=1` and `FITMENT_REQUIRE_FIXTURES=1`;
  8. check that `cargo package --list` shows no STEP file;
  9. upload `corpus/out` as an artifact.
- **The 3-OS test job** gets `needs: corpus`, downloads the artifact, and sets `FITMENT_REQUIRE_CORPUS=1`. The whole generated corpus then runs on Linux, macOS and Windows without Python. The committed unit set runs everywhere with no setting.
- Fixtures stay Linux-only. They are never uploaded as an artifact, because their redistribution terms are unknown.
- **A test with a `FITMENT_REQUIRE_*` variable set** fails when its input is missing, or when it ran fewer cases than the manifest lists. A skipped gate cannot pass green.

### 15.3 Goldens

- **One golden per unit-set input:** `crates/fitment/tests/golden/features/<stem>.json`, the exact stdout of `fitment features`. Refusals have goldens too.
- `golden.rs` runs the built binary and compares bytes and the exit code. It fails on a golden with no input, or an input with no golden.
- **`FITMENT_BLESS=1`** writes missing goldens. It rewrites a changed golden only if `extractor_version` went up; otherwise it fails and names the file. It refuses to run when `CI` is set. Blessing therefore cannot hide an unversioned change.
- **Goldens freeze bytes; they do not prove them right.** Correctness comes from `tests/parts.rs`, which checks against construction parameters.

## 16. PR plan

Each PR comes with its tests. `EXTRACTOR_VERSION` goes up with every PR that changes output.

1. **Skeleton.** It builds on fitment #4, which adds stepq.
   - `geom.rs` (`libm` waits for PR 4, where trigonometry is first used, so no PR adds an unused dependency);
   - `FeaturesDocument` and a `Part` with empty feature lists;
   - `read_part` with gate stages 1 to 3;
   - `fitment features` with its exit codes;
   - `golden.rs` with blessing;
   - the `include` list and the package check;
   - `EXTRACTOR_VERSION = 1`.
2. **Generator and unit set.**
   - `tools/gen-corpus.py` and its lock;
   - the manifest and the unit set;
   - `tests/parts.rs`, which at first asserts only refusals and acceptance, and skips cases whose `rules` name a gate stage not built yet (r05, r10, r11, r12 until PR 3), with that list emptied in PR 3;
   - `verify-occt.py manifest`;
   - the `corpus` CI job.
3. **Gate stages 4 to 6.**
   - topology, units, geometry, vertex and face-orientation checks, and `libm` (moved here from PR 4: the planar orientation check sums arc sweeps);
   - `fetch-fixtures.sh` and the pinned gate census;
   - the fixture test.
4. **Planes and the partition.** As built:
   - the `Topo` copy, with the gate's checks 22 to 26 moved onto it (section 11.1);
   - grouping, outlines, areas, and the `stated_outer_bound_differs` note;
   - cylinders, cones and tori grouped (G1, G2 and section 8.6) with their sense, axis and radii, but not yet classified; other faces;
   - features stated in the part's frame: a solid placed by a mapped item is moved before it is read (p29 caught this), and check 26 runs again on the moved solid (a translation of 1e17 mm otherwise snapped vertices to a 16 mm grid and stated wrong areas as known);
   - the partition check, in `read_part`, in `from_json`, and on every unit part, corpus part and fixture;
   - the planes of every corpus part checked against the construction, and each accepted fixture's plane count pinned;
   - `verify-occt.py features` for planes, in CI on the corpus and the fixtures.
5. **Cylinders:** full or partial F, the near-miss notes, contradictions 27 and 29, and the oracle's cylinder checks. Grouping G and sense S arrived in PR 4. As built:
   - `class` is `full`, `full_one_end_irregular` (with `irregular`: `start` or `finish`, the end at the lowest or highest station), `partial` (with `sweep_rad`, the widest station's cover) or `undecided` (with `reason`: `no_end_circle`, `irregular_at_both_ends` or `ends_disagree`). F3 and `full_by_seam` were withdrawn in review (section 8.3);
   - a cylinder's axis has the canonical sign of section 9.4 and its point at the lowest station (else the stated location), so `stations_mm` starts at 0;
   - `near_misses` lists the other cylinders' ids, and each shared edge gets a `near_miss` note;
   - wall openings are counted as connected chains of "other" boundary edges with a vertex strictly between the stations;
   - every corpus part's cylinders match the construction one to one (`tests/support/expect.rs`, shared by the core and CLI tests), and the oracle checks every cylinder's radius, axis line, sense, 360° cover where fitment says full, and extent where both ends are known: 1,728 cylinders over the corpus and the 24 accepted fixtures, 0 disagreements. A full cylinder must go all the way around just inside each station, by points classified against its faces; its stations must be the circles among its edges; its faces must run past the stations on exactly the side it calls irregular; a partial sweep must lie within the faces' parameter bounds. The first version measured cover from parameter bounds, a bounding box that a face with a cutout fills: it called 14 partial cylinders full and could not have caught a false `full` on a slotted bore (review of PR 5). Mutations of fitment's output (a partial called full, the irregular end swapped, a sweep 0.1 rad wider, a station 0.5 µm off) are all caught;
   - on the fixtures, the census of section 8.5 holds: NIST ctc_01 has 10 full holes in each of its three exports, and ftc_10 has 42 (14 with one irregular end).
6. **Holes and shafts:** the end walk W, stacks K, lengths E, cones and tori in walks, `complete`, and the oracle's through/blind and depth checks. As built:
   - every full cylinder of the outer shell walks from each regular end station (`features/walk.rs`); mutual steps chain them into stacks, each a `Hole` or `Shaft` with `kind`, `axis` (section 9.4), `segments`, `transitions`, two `ends`, its length, depth or protrusion, and `wall_openings` (`features/stack.rs`);
   - an end is `opening` (with `rim_mm`, `opens_into` and any `countersink`), `bottom` (`flat`, `cone_then_flat`, `point` or `annular`, with `closed_by`), `free_end`, `root`, `irregular`, or `unknown` with a `reason`; `part.complete` is section 9.6's;
   - **W4's apex** is a narrowing cone bounded by the current circles alone (a vertex loop at the apex is not an edge); any other edge leaves the end unknown (L1);
   - **planes met must not turn back** along the axis (reason `turns_back`, added in PR 6). A gear hub rising from a pocket's floor walked across the floor, up the pocket's chamfered wall and onto the face around the pocket, which W8 then took as the root: the oracle caught the root 1.5 mm off. A round that turns back to a boss's wall after its top (C3) meets no plane, so p30 reads as before;
   - every corpus part's holes and shafts match the construction one to one: kind, axis point and direction, segments, ends with their rims, bottoms (angle, tip length, floor, post) and countersinks, and lengths; a hole whose both ends are irregular reads as no stack, since its cylinder is undecided;
   - the oracle runs the manifest mode's two-sided tests on fitment's own holes and shafts: 434 through and blind holes and 18 shafts probed over the corpus and the 24 accepted fixtures, 0 disagreements. On real parts its probes first misfired: 1 mm beside an opening left a 6 mm part, and a root's fillet or a hollow free end hid the plane at the rim. They now scale to the wall and try eight directions, within the wall's width unless the walk crossed a round, fillet or countersink; review of PR 6 showed that probing further out everywhere let a bore through a boss pass on the plate around it. It also checks each shaft's ends against its kind and length, and that an opening past its wall's end without a reported countersink does not widen; every mutation the review tried (through and blind swapped, rims and lengths moved, countersinks dropped, shaft ends swapped) is caught;
   - a segment's `from_mm` or `to_mm` is `null` on an irregular side, where its wall runs past its end circle;
   - `from_json` checks that every id an end, segment or transition names exists with the right kind, segment diameters and lengths agree with their cylinders, end kinds fit the stack's sense and kind, the start end is at 0, and `complete` is recomputed;
   - on the fixtures: 250 through, 146 blind and 32 unknown holes; each model's exports give the same holes (ctc_01: 8 through and 2 blind in all three), and the counts by kind are pinned;
   - check 28 refuses an annulus whose side contradicts its radii; a constructed solid tests it, since a file that reaches it fails check 25 first.
7. **Hardening.**
   - the `_pc` and `_rot` variants;
   - the oracle on all fixtures, with pinned tallies;
   - the table view;
   - `docs/ARCHITECTURE.md` (crate graph, format and extractor versions, determinism rules).

   As built: every positive part of the corpus is written again with pcurves (`_pc`, 41 cases) and moved by a second skew motion (`_rot`, 39 cases; two parts whose answers are hand-stated rather than moved have none), for 132 cases; fitment and both oracle modes agree on all of them. `verify-occt.py features --expect fixtures/manifest.json` fails when the fixtures' tallies move. `fitment features --format table` prints one line per feature.
8. **M2 exit.** PLAN.md is updated (section 17), and the README status. The rule-coverage test is turned on.

## 17. Changes to PLAN.md

- **Matching method, step 1:**
  - "The extent along the axis comes from the vertices of the bounding edges" becomes "from the centres of the stated end circles".
  - "Only a full circle is a hole. The arcs at each end must sum to 360°" stands as written: the seam proof proposed here was withdrawn in review of PR 5.
  - "`same_sense = .F.` marks a hole and `.T.` a shaft" becomes "`face_sense` (`same_sense` composed with the shell's orientation) false marks a hole and true a shaft; the gate checks it against the loops".
  - "Faces ... with the same axis line and radius are one cylinder" becomes "edge-connected faces with the same sense on one stated cylinder are one cylinder; coaxial cylinders that do not touch stay separate".
  - "A plane is kept with its outward normal and its outline from the loop vertices" becomes "...its outline from the loop vertices and each edge's line or circle, exactly".
- **Open decisions:** D2 is decided by section 4, R1 to R5 and R10.
- **Spike results, item 3:** most exporters write split faces; grouping handles them (section 8.1, G4).

## 18. How the four designs were reconciled

| Disagreement | Settled as | Why |
| --- | --- | --- |
| D refused B-spline faces by default; C kept them as "other" | Keep them (R1) | C's census: refusing costs 24% of valid parts. An unread face can only miss a match. |
| B accepted a file with several definitions, one holding the solid; C refused it | Refuse (R4) | The other definition in the only such fixture holds surface models. |
| B and C counted assemblies and multi-body parts differently (12 and 0, against 19 and 27) | C's numbers | B skipped the seven files over 16 MB, where the remaining assemblies and all multi-body parts are. |
| A reported void cylinders in a separate list; C put void faces in no feature | Partitioned like other faces, with a shell index, never in a hole or shaft (R5) | Keeps the face partition exact and the rule simple. |
| B's placeholder hole JSON differed from A's stacks | Two levels: primary surface groups that partition the faces, and compound holes and shafts on top (sections 6, 9.5) | It matches B's partition rule and A's stacks. |
| A's lever-arm angular test and B's sine test | Both kept, named in `extraction` (section 5.2) | Planes have no natural lever arm. Cylinders do. |
| D expected p13 and p25 to read as through holes with lengths | They read with irregular ends and `null` depth; F3 added (R8) | stepq reads no ellipses or intersection curves. A guessed length would break Soundness rule 2. |
| D's r01, r02 and r07 refusals | Became p26, p27 and p28, all accepted | They follow R1 and R5. |
| D's brief named the oracle pin as stepq's | `cadquery-ocp-novtk==8.0.1.1.0` (decision 8) | stepq pins `cadquery-ocp==8.0.1.0.0`. fitment follows its own D1 decision. |

The soundness review then found these, each reproduced with a generated or edited part run through A's probe:

| Finding | Failure as first written | Fix |
| --- | --- | --- |
| C1 | One flipped `same_sense` in p01 read the Ø6.6 through hole as a Ø6.6 pin with two free ends. On a plane it flips the normal and makes the hole's circle the outer loop. Nothing cross-checked `face_sense`, and Open CASCADE reads it the same way | Gate check 25, `face_orientation`; r11 and r12. A whole inverted shell remains (R11) |
| C2 | A blind Ø10 hole with a conical undercut read as through: the widening cone said "air" and nothing later was compared | Every plane the walk meets must agree (W3); a non-plane terminal after a cone or torus is `unknown` (W4); u01, u02 |
| C3 | A bore through a boss with a 1 mm round read 15 mm long, not 16; a post in a filleted pocket 2 mm high, not 3; one post's root landed above its free end | The rim is the last perpendicular plane met (W8); p30, p31 |
| H1 | G3 joined plane faces of opposite normal at a station, so a half-capped hole read as through or unknown depending on face order | G3 joins only same-normal faces; u03 |
| H2 | F3 was purely topological: a bridge or slit edge also counts as a seam, and it gave no extent | F3 requires an axial line in one loop, never overrides `partial`, and records `proven_extent_mm`; M4 uses such a hole for axis and diameter only; a bridged-loop snippet |
| M1 | A void face could join an outer seating plane | Planes never cross shells (7.2); p38 |
| M2 | Read as the whole primary plane, W1's group made a counterbore floor coplanar with a shoulder elsewhere into a `face`, dropping the counterbore | The walk group is local (W1); p37 |
| L1 | A drill point pierced by a cross bore still closed at its apex | `apex` requires no other edges; u05 |
| L2 | The sweep sum alone accepts two arcs over the same half circle | The interval-union test is kept beside it (F2) |

The consistency review corrected populations (48 or 55 files), counts, six expected answers, missing fields (`header`, `max_vertex_offset_mm`, `solid` on every feature), the `internal` exit code, the stepq boundary and the rule-coverage plan. Its report is in [m2-evidence/review-consistency.md](m2-evidence/review-consistency.md).

## 19. Risks

- **Real exporters split faces; the generated parts mostly do not.** 94% of full cylinders in the fixtures are split. p17, p18 and p20 and the NIST fixtures cover the split path, but the generator can only produce Open CASCADE's way of splitting.
- **Open CASCADE 7.9 writes some filleted tori inside out.** In PR 3, two generated parts filleted with `BRepFilletAPI` (p31's floor fillet, p32's rim round) had a torus face whose loop, as its `FACE_BOUND` and `same_sense` state it, runs the same way as both its neighbours along their shared circles (p32's states `.F.` on the bound and `.T.` on the face, p31's the reverse). Open CASCADE repairs orientation as it reads, so it called the solids valid; fitment's check 15 refuses them as `solid_not_closed`. The generator now cuts those parts with revolved tools, which Open CASCADE writes consistently. Real parts exported by Open CASCADE with such fillets will be refused too: a false negative, never a misread. None of the 45 core fixtures is affected.
- **The end walk is the most complex rule.** In A's probe it gave 108 unknown hole ends (about 84 of them G3 cases the rules resolve) and 56 non-mutual steps. Each unknown must stay `unknown`, never default to open or closed, or a blind hole could read as through. The review found two such defaults (C2, C3) after the rules were written; every W4 and W5 row now has a case, and the `u` parts must read `unknown`.
- **A whole shell written inside out** passes every per-face check (R11). Only the oracle's positive-volume check sees it, and only on files CI reads. A sound global test, for example on support planes extended with circle extremes, needs measuring: a naive version flags 21 valid fixture shells.
- **Ellipses and B-spline circles lose holes.** Angled holes and CAS.CADE-style exports give irregular or undecided cylinders. That is safe but costs coverage. Reading ellipses (stated centre, axes and radii) would be a natural stepq 0.7 addition, if the corpus shows it matters.
- **Vertices off their circles by up to 1e-2 mm in Creo files.** No length uses vertices. Vertices enter only the side test (F4) and the apex test (W4), both with ε_v, and arc sweeps (F2), where shared vertices cancel. Their worst outcome is an `Irregular` or `Unknown` end, a false negative.
- **The generator's bytes differ by platform.** PR 2's first CI run found two parts (p14s, p25) whose numbers differ between macOS and Linux in the last digit, or as noise near zero (`0.` against `-1.15E-15`): Open CASCADE's trigonometry rounds by the platform's maths library. The check is therefore the same text with numbers within 1e-9 mm, not the same bytes. Tests read only the committed files, so their results do not depend on the platform that generated them.
- **The probes differ from the rules in about 240 cases, in both directions,** listed in [m2-evidence/A-cylinders.md](m2-evidence/A-cylinders.md), section 12.1, plus every rule changed after review (G3, F2's union test, F3, W3, W4, W8). The implementation follows the rules as written, so fixture counts will move. The pins come from the implementation, not from this document.
- **The uncertainty floor.** 179 of 916 accepted parts state more than 0.01 mm, while only 9 measure more. M4 (D9) must reject, not widen its tolerance, when a vertex-derived value is below the larger of the two. Widening is a false-positive path.

## 20. Smaller open questions, with defaults

1. A stated outer bound that disagrees with orientation: a note, not a refusal.
2. Partial features in a refused document: none.
3. Reading stdin: not in M2.
4. Pretty JSON on stdout: yes, for readable golden diffs. A `--compact` flag later if size matters.
5. `across` on every segment: yes. It costs about 25 bytes per segment and gives M3 hole-to-plane links without geometry.
6. Area per loop as well as per face: yes.
7. Spheres (ball-end bottoms): an `unknown` end in M2.
8. Coordinates beyond 1e6 mm: refused as `out_of_range`.
9. Fixtures on three OSes: no; Linux only until M4's determinism job.
10. A stated uncertainty with no upper bound: a file that states u = 1e5 mm passes checks 23 and 24 with a vertex 82 mm off its cylinder, and `max_vertex_offset_mm` reports it honestly. Default for M2: accepted, since the uncertainty is recorded and becomes the matcher's floor, which M3 must then reject as too coarse to seat anything. A gate bound, such as u above 1 mm or a fraction of the solid's extent, needs measuring on the fixtures first (found in review of PR 3).

## Evidence

[m2-evidence/](m2-evidence/) (not committed) holds the four design sections as written, and:

- `probe-gate/`: the gate census over all 55 fixtures (`gate2.py` applies the checklist);
- `probe-cyl/`: the cylinder, end-walk and stack rules over all 55 fixtures and five crafted parts (`run-all-v6.txt` is the final run);
- `probe-planes/`: the plane census and the loop-traversal check;
- `gen-b/`: two Open CASCADE plates and their areas;
- `probe-json/`: serde_json's handling of `-0.0`, NaN and infinity;
- `probe-gen/`: the generator and oracle prototypes, with their outputs;
- `review-soundness.md` and `review-soundness/`: the soundness review, its probe and the parts that reproduce each finding;
- `review-consistency.md` and `review-consistency/`: the consistency review and its tally script.

Each probe depends on stepq by path with `default-features = false`.
