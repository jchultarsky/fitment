# fitment M3: the socket of a part in an assembly

As of 2026-10-12. Status: **accepted** on 2026-10-12, with every default in section 2 (R1 to R24). Nothing here is implemented yet; section 15 is the order of work.

M3 delivers `fitment socket asm.stp --part X`, which reads an assembly and writes what the neighbours of part X require of any part that replaces it, as a JSON file the user reviews. It is done when generated assemblies and the AS1 fixture give the expected requirements, and holes with nothing on their axis are left out ([PLAN.md](../PLAN.md), Milestones).

Four agents designed one area each:

- A: reading an assembly, the assembly gate, choosing the part and placing its features;
- B: the rules that turn placed features into requirements;
- C: the socket as data: the model, the `fitment.socket` file, review, the API and the CLI;
- D: the generated test assemblies, the oracle, the fixtures and CI.

Each measured its rules on stepq 0.6.0 (`78e46e8`) and fitment `ec9f9c6` (`EXTRACTOR_VERSION` 5), on the 19 assembly fixtures (the 9 of fitment's core set and the 10 of the `ocp` set) and on assemblies generated with cadquery 2.8.0 (Open CASCADE 7.9.3). This document merges their work and settles where they disagreed (section 17). Probe code, the four reports and raw output are in [m3-evidence/](m3-evidence/), which is not committed.

## 1. Summary

- **An assembly is read once, fully, before any part is chosen.** Parse, product structure, placements and occurrences; then every distinct leaf definition goes once through M2's gate and extraction. Features stay in each part's frame, and an occurrence is a path plus one rigid transform.
- **The assembly gate is strict.** A refused component, an unplaced usage, an empty component or material in an assembly's own shape refuses the whole assembly, with every reason. 10 of the 19 fixture assemblies pass. An empty component is never "nothing": 352 of the 418 refused leaves are empty, and their names are screws, nuts, pins and welds the exporter left out.
- **`--part` takes a part number** (`PRODUCT.id`), then a name, or a definition `#id`. Every occurrence is selected by default (D11); `--occurrence` narrows to one.
- **A socket is stated in the target part's own frame,** the frame `features.v1` uses. Each occurrence carries its transform to the assembly root. The original part's features then line up with its socket at the identity, and occurrences that differ only in position merge by comparing values.
- **Every other leaf occurrence is a neighbour,** including other occurrences of the target (flagged `self`, and pinned on both sides, since one candidate replaces both). Relations are found from exact features (axis lines, planes, outlines) through bounding-volume trees, so the work is bounded and no distance cut-off drops a relation.
- **A shaft is "in" a hole when the axes are parallel and the cross-sections meet, not only when they are coaxial.** The clearance bound accounts for the offset e: the hole must be at least d_shaft + 2e. 18 related pairs in the weldment fixture are 0.03 to 0.099 mm off axis, beyond their stated uncertainty.
- **One requirement per target feature.** A target hole gets one `bore` requirement, with spans along its axis; a target plane one `seat` with every contact; two opposed seats one `grip`. Every neighbour that contributed is listed as provenance.
- **Facts, not policy.** The socket records the original diameters, the diameter each neighbour needs, the fit and the contact faces. M4 applies D6 (strict or functional clearance), D7 (grip) and D9 (tolerances).
- **Threads are inferred, never confirmed.** A neighbour shaft as large as the hole or larger pins the hole's diameter and adds `thread_not_confirmed`. Every AS1 joint is modelled line to line (Ø10 in Ø10), so every AS1 hole is pinned.
- **Unknowns never drop a requirement.** An unknown extent is unbounded on that side. Material in a hole that is not a read shaft (a D-shaft, a key, a B-spline bolt) pins the hole. An unread face on the target or on a neighbour, and a contact no rule reads, become `unresolved` requirements, which fail every candidate until the user removes them in review. AS1 as written by think3 reads no hole at all and still reports `complete: true`; without these rules its sockets would hold no bore.
- **Holes with nothing on their axis are left out, and listed as left out.** Every target hole, shaft and undecided cylinder appears in each socket either as the anchor of requirements or in `left_out` with a reason, never both. A hole is never left out while an unread neighbour face could be in it.
- **The original satisfies its own socket,** except for the requirements that fail every candidate by design. Every verifiable limit contains the original's own value; tests assert it on every socket.
- **Review keeps or removes, never edits.** A removal needs a note. A SHA-256 digest over everything but the review makes `from_json` refuse a file whose values, versions or constants were changed. Re-extracting the same bytes carries decisions over; a decision that no longer applies is kept aside and never applied.
- **Tests:** 25 generated assemblies and 14 refusal and selection cases with answers from the construction, 4 of them committed; AS1 in six exports, pinned per export; an Open CASCADE oracle that checks fitment's requirements and looks for ones it missed.
- **Eight PRs** (section 15).

## 2. Decisions for review

Each has a recommended default. The rest of the document assumes the defaults.

| # | Question | Default | What it costs or buys |
| --- | --- | --- | --- |
| R1 | A component that fails the gate: refuse the assembly, or only when it is near the target? | Refuse the assembly (section 4). A scoped rule is specified for later (4.4) | Strict passes 10 of 19 fixtures (110 of 891 possible targets); the scoped rule with a 1 mm margin would pass 12 (205). It needs a sound bound for material fitment refuses to read |
| R2 | An empty leaf definition (no geometry) | A refused component (`no_solid`) | 352 definitions, named as screws, nuts, pins and welds. Under the scoped rule, ignoring them would pass 11 more targets (216 against 205), every one a possible false positive |
| R3 | Which parts can be chosen | Leaf definitions only | 293 sub-assemblies are listed by `parts` but not selectable |
| R4 | `--part` matching | `#<n>` (a definition `#id`), else exact `PRODUCT.id`, else exact `PRODUCT.name`; none or several is a usage error listing candidates | `PRODUCT.id` is present and unique among leaves in all 19 fixtures; names are missing (md) or shared (moon buggy) |
| R5 | Which occurrences (D11) | All occurrences of the part; `--occurrence <path>` narrows to one | Up to 162 occurrences of one part (BBU shelf) |
| R6 | The frame of a socket | The target part's frame; each occurrence carries its transform to the root | Merging is a value comparison; the original passes at the identity; M7 places the candidate at `T_k · alignment` |
| R7 | One socket per occurrence, or one document per part? | One document per part, holding a list of sockets; occurrences whose requirements agree within 1e-6 mm share one | AS1-AC: 2 L-brackets give 1 socket, 6 bolts 1, 8 nuts 4 |
| R8 | Other occurrences of the target | Neighbours, with their requirements flagged `self`; a hole or shaft related to another copy is pinned, and M7 checks the copies against each other | Stacked washers and mirrored brackets constrain each other. The two sides of such a relation are both the candidate, so independent limits would let a candidate's pin and hole clash |
| R9 | What makes a shaft "in" a hole | Parallel within a sine of 0.01, cross-sections meeting over an axial overlap; coaxiality only labels | Coaxial-only detection drops the weldment's 18 off-axis pairs |
| R10 | Equal diameters (line to line) | Pinned, `thread_not_confirmed` | A thread, a press fit and a nominal clearance cannot be told apart; pinning is the strict reading. AS1 pins every hole |
| R11 | A coaxial neighbour hole beyond a gap, with nothing read in it | A requirement, unless a plane face of known outline blocks the line of sight between them | Omitted fasteners. Weldment: 39 holes rely on it, 17 are blocked and left out, 22 stay (mostly mirrored parts 144 to 197 mm apart) |
| R12 | Diameter of a hole whose fastener was not read | Pinned to the original | The unseen fastener's size is unknown, so no smaller hole is safe |
| R13 | Contacts between planes | Opposed faces within the contact tolerance whose outlines may overlap (a conservative test); within 1 mm, a seat flagged `near_gap` or `interference` | Weldment: 46 opposed pairs at 0.1 to 0.84 mm |
| R14 | The region a seat requires | The target faces in contact, stated exactly (`basis: target_face`); unverifiable (`basis: unknown`) when the target's outline is unknown | Sound: the true contact lies inside both. Stating the neighbours' outlines broke merging (411 values differ on AS1's two brackets) and made the file 3.7 times larger. Costs false negatives on a smaller flange |
| R15 | Unread faces (B-spline and other), and contacts no rule reads | On the target: one `unresolved` per face. On a neighbour: one `unresolved` per neighbour definition, and every target hole such a face may meet is pinned (so none is left out). A touching pair no rule reads: `unresolved` | Moon buggy: 7 of 19 occurrences carry unread faces; AS1 think3: all 18. Every such socket needs review until stepq reads B-spline control points |
| R16 | Undecided or partial target cylinders with something in or around them | A requirement M4 v1 cannot verify (`target_undecided`, `partial_bore`, `partial_shaft`); it fails until review removes it | M2 section 9.6: refuse or flag. Flagging keeps the rest of the socket |
| R17 | The uncertainty floor of a relation | The largest of the part and placement uncertainties and vertex offsets of the target and the neighbour | The placement path's uncertainty is larger for 3,124 of 3,224 occurrences; Creo floors rise to 0.1 to 0.4 mm. Larger floors only add requirements |
| R18 | What a user may change in review | Keep or remove a requirement; a removal needs a note. No value edits, no added requirements in M3 | Removal is the only loosening, and it is explicit and audited |
| R19 | Catching edited values | A SHA-256 `digest` of everything but `review` and `stale_reviews`; `from_json` refuses a mismatch and re-checks every structural rule anyway | Catches accidents, including a version or constant raised by hand; not a security boundary |
| R20 | Versions | `socket_rules_version`, separate from `EXTRACTOR_VERSION` | A socket rule change does not re-index the catalog's features |
| R21 | SHA-256 | In-crate (FIPS 180-4, about 100 lines, tested on the NIST vectors) | The `sha2` crate would add about 8 crates to the core's allow-list, `libc` among them |
| R22 | Additions to `features.v1` | A sound part `extent` (null when unbounded) and each cone's radial range; `EXTRACTOR_VERSION` 6 | The extent bounds unknown ends and the search; the radii state a conical seat |
| R23 | AS1's Euclid and think3 exports, which read no hole | Accepted sockets with `unresolved` and `target_undecided` requirements, and a warning | Never accepted with holes dropped; refusing would hide what was read |
| R24 | Committed assemblies | 4 generated assemblies (about 195 KB) under their own 250 KB cap | `cargo test` without Python then covers a socket |

## 3. How an assembly becomes a socket

`SocketDocument::extract(bytes, part, occurrence)` runs these steps. Each refuses with reasons or hands checked values on.

1. **Stage 1, the file** (M2 checks 1 to 4), unchanged.
2. **Stage 2, the structure and placements** (checks 31 to 34): one root, every usage placed, every occurrence composed, at most `MAX_OCCURRENCES`.
3. **Stage 3, assembly shapes** (check 35): an assembly's own shape holds curve and point sets only.
4. **Components** (check 36): each distinct leaf definition goes once through M2's stages 3 to 6 and extraction.
5. **Frames and range** (checks 37, 38).
6. **Selection** (section 5): `--part` and `--occurrence`. Not a gate: a failure is a usage error.
7. **Relations** (section 7), per selected occurrence: neighbour features moved into the target's frame, found through search trees and compared with the target's features, up to `MAX_RELATIONS` candidate pairs (check 39).
8. **Requirements** (section 8): relations combined into one requirement per target feature; left-out features listed.
9. **Sockets** (section 9): occurrences with equal requirements merged; the document built, digested and returned.

`fitment parts asm.stp` runs steps 1 to 5 and lists the parts without refusing for a component.

stepq appears only in `src/gate/`, `src/features/step.rs` and a new adapter, `src/assembly/step.rs`, which copies occurrences into fitment's own types; the CI grep gains that file.

## 4. The assembly gate

### 4.1 The checklist

M2's checks 1 to 30 keep their numbers and meaning. Check 5 (`assembly`) and check 7 (`several_definitions`) do not apply to an assembly; check 6 still does. Assembly mode adds 31 to 39. "Fixtures" counts the 19 assembly fixtures.

| Stage | # | Check | Code | Fixtures |
| --- | --- | --- | --- | --- |
| Structure | 31 | The root uses something. `socket` on a part file refuses; `parts` lists the one part | `not_an_assembly` | 0 of 19 |
| | 32 | Exactly one root, and every definition reached from it. An unused definition is a root, so this also catches orphans; definitions that use each other in a cycle are nobody's root and are caught as unreached | `no_root`, `several_roots` | 0 |
| Placements | 33 | Every usage in the file is one stepq reads (a usage whose parent or child is not a reference would otherwise vanish), every usage places (`Placements::usage`), and every occurrence's composed transform reads (including `Cycle`, `TooDeep` and composition `Inconsistent`) and passes `Rigid::new` (finite, orthonormal within 1e-9, determinant +1). One refusal per failing usage, with stepq's `PlacementError` kind first in the message | `placement_unreadable` | 0 of 3,481 usages and 4,320 occurrences |
| | 34 | At most `MAX_OCCURRENCES` (100,000) occurrences under the root, 2,000,000 usages on their paths together and 16 MiB of usage ids on them, counted while enumerating; a failure ends the stage, keeping check 33's refusals | `too_many_occurrences` | Largest 1,049. A 26 KB diamond of 40 usages reaches 2²⁰ |
| Assembly shapes | 35 | Each assembly definition reached has a complete own shape (`ProductStructure::shape`, which skips component placements) holding only curve and point sets | `assembly_shape_item` | 5 surface models in 1 file; 0 solids |
| Components | 36 | Each distinct leaf definition passes checks 8 to 30, once. One refusal per refused component, `instance` its definition `#id`, naming its `PRODUCT.id`, quantity and codes; its full refusals are kept in the component list | `component_refused` | 418 of 1,309 distinct leaves, in 9 files |
| Frames | 37 | Each occurrence of an accepted part places a shape representation its solid is listed from, with the solid transform its features were read with | `solid_frame_unplaced` | 0 of 3,224; an edit reproduces it |
| | 38 | Every placed coordinate stays finite and below `RANGE_MM` (1e6 mm), as check 26 for one part | `out_of_range` (existing; `instance` the occurrence's last usage) | 0; largest translation 3,861 mm |
| Relations | 39 | At most `MAX_RELATIONS` (10,000,000) candidate pairs examined over all selected occurrences, counted while searching (N4) | `too_many_relations` | Largest well under 10⁵ |

Stages run in order, and a stage that refuses skips the later ones. Within the components step every check runs, so every refused component is listed. Refusals are sorted and capped per code as in M2 (32, then "and N more"): the OCP enclosure has 341 refused components.

New `RefusalCode`s, appended in check order: `NotAnAssembly`, `NoRoot`, `SeveralRoots`, `PlacementUnreadable`, `TooManyOccurrences`, `AssemblyShapeItem`, `ComponentRefused`, `SolidFrameUnplaced`, `TooManyRelations`. A machine name never changes meaning; a new check gets a new code.

New notes (not refusals): `coincident_occurrences` (two occurrences of one definition whose transforms agree within `IDENTITY_MM`: 2 in Olympus 1U) and `assembly_uncertainty_missing` (an assembly context that states no length uncertainty: all 4 of AS1 think3's).

### 4.2 What fails, by file

| File | Usages | Occurrences | Distinct leaves | Refused | Of which empty | Strict |
| --- | --- | --- | --- | --- | --- | --- |
| AS1 ×5 (STEP Tools), `as1_pe` | 13 (pe 9) | 28 (pe 15) | 5 | 0 | 0 | pass |
| `moon_buggy_asm` | 23 (mapped items) | 30 | 12 | 0 | 0 | pass |
| `vaccase_asm_solid` | 14 | 15 | 8 | 0 | 0 | pass |
| `weldment_asm_solid` | 108 | 109 | 33 | 0 | 0 | pass |
| ORv3 BBU unit | 74 | 75 | 27 | 0 | 0 | pass |
| OCP enclosure 6OU | 612 | 613 | 437 | 341 | 340 | refuse |
| Olympus chassis | 903 | 1,049 | 186 | 21 | 0 | refuse |
| Olympus 1U | 487 | 735 | 279 | 17 | 6 | refuse (and 5 assembly-level surface models) |
| Olympus motherboard | 153 | 218 | 73 | 26 | 6 | refuse |
| ORv3 power shelves, PMI module, PSU, BBU shelf | 65 to 377 | 66 to 469 | 32 to 58 | 1 to 5 | 0 | refuse |
| **19 files** | **3,481** | **4,320** | **1,309** | **418** | **352** | **10 pass** |

The refused components: `no_solid` 360, `unread_shape_item` 51, `several_solids` 27, `vertex_off_surface` 2, `topology_malformed` 1. The non-empty ones are exactly the neighbours that create requirements: standoffs and nuts (64 and 55 occurrences in the chassis), screws, and housings that surround everything else (`SMC_HOUSING__`, 1,140 mm long).

### 4.3 Why each failure refuses

A refused assembly costs only false negatives. A missed neighbour costs a requirement, and a missed requirement is a false positive (Soundness rule 3).

- **31, 32:** with no usage there is no neighbour; with two roots, two unrelated frames.
- **33, 34:** an unplaced component is a neighbour at an unknown place.
- **35:** material in an assembly's own shape (a weld bead, a quilt) is a neighbour with no part. Reading it as a pseudo-part is possible later; no fixture has a solid there.
- **36 refuses the whole assembly (R1).** A refused component is a neighbour whose features are unknown.
- **Empty components refuse (R2).** In stepq's terms the shape is complete and states nothing. The names say otherwise: the OCP enclosure's 340 include `FILLET_WELD_ID_*` and numbered hardware; Olympus has `ZZ_SCREW_601`, `SCREW_PAD_NUT`, `15581_TOP_PIN`. A screw left out makes its hole read as "nothing on its axis".
- **37:** features are stated in the frame of the representation the solid is listed from (M2, 5.1). A usage that places another representation places them in an unknown frame.
- **38, 39:** as check 26, and a bound on work.

### 4.4 The scoped rule, for later (R1's alternative)

Specified so it can be adopted without redesign; not built at M3.

- A refused component, or assembly-level geometry, would matter for a target only if its *sound outer box*, placed by each of its occurrences, came within `NEIGHBOUR_MARGIN_MM` of some occurrence of the target. Otherwise it is listed as "unread, far from the part".
- A sound outer box is the union of: every vertex and `cartesian_point`; each circle's centre ± its radius; each sphere's and torus's centre ± (major + minor); every control point of a B-spline with positive weights. It is unknown, and blocks every target, for an item it cannot bound (surfaces of revolution, offsets, tessellations, anything unlisted), and for empty or unplaced components.
- `NEIGHBOUR_MARGIN_MM` is at least `NEAR_MM` plus the floor, and the M7 fit check treats an unread component in reach as "fit not verified".

Measured (probe boxes, not yet sound): 12 assemblies and 205 targets at 1 mm, against 10 and 110. It rescues 68 targets in the Olympus chassis and 27 in the BBU shelf, and none elsewhere, because there the refused component is a housing whose box holds everything.

## 5. Choosing the part

### 5.1 `fitment parts asm.stp`

Lists each definition reached from the root: `definition` (`#id`), `product` (`#id`, `id`, `name`), `kind` (`part` or `assembly`), `quantity` (its occurrences under the root), its occurrence paths, and for a part its gate result (accepted, with counts of holes and shafts and `complete`, or refused with its codes), plus the result of the whole assembly gate. It refuses only for stage 1 and checks 32 to 34, without which quantities and paths are unknown; never for a component, so the user sees what blocks `socket` before running it. On a part file it lists the one part. Order: by definition `#id`.

AS1 as exported by Unigraphics:

| Definition | `PRODUCT.id` | Quantity | First path | Gate |
| --- | --- | --- | --- | --- |
| #119 | `bolt` | 6 | `1011/1007/1004` | accepted: 1 shaft |
| #120 | `nut` | 8 | `1011/1007/1005` | accepted: 1 hole |
| #121 | `rod` | 1 | `1013/1001` | accepted: 1 shaft |
| #122 | `l_bracket` | 2 | `1011/1006` | accepted: 4 holes |
| #123 | `plate` | 1 | `1010` | accepted: 6 holes |

### 5.2 `--part <value>` (T)

- **T1.** Candidates are the leaf definitions reached by an occurrence under the root (R3).
- **T2.** `#<digits>` names the `product_definition` with that `#id`. If some `PRODUCT.id` is literally that string, the selection is ambiguous.
- **T3.** Otherwise the value is compared, byte for byte after stepq's string decoding, with no trimming or case folding, to each candidate's `PRODUCT.id`. One match selects it; several are `part_ambiguous`.
- **T4.** With no id match, the same with `PRODUCT.name`.
- **T5.** With no match: `part_is_assembly` if a sub-assembly's id or name matches; else `part_not_found`, with up to 20 suggestions whose id or name contains the value case-insensitively, sorted by id.
- **T6.** Selection errors are usage errors (exit 2), with the candidates on stderr: definition `#id`, id, name, quantity and first path.

Measured: every one of the 1,309 distinct leaves has a non-blank `PRODUCT.id`, none shared, none equal to a sub-assembly's, and no product has two definitions. `id` equals `name` in 17 of 19 files; `as1-md-214` has no names, and `moon_buggy_asm` names 12 leaves "Moon Buggy". cadquery writes two different parts named `BOLT` in two sub-assemblies, which is T3's test.

### 5.3 Occurrences and their paths (O)

- **O1.** An occurrence is the list of usages from the root down to it. The root has the empty path.
- **O2.** **Written form.** Each segment is the usage's `id` attribute when every usage in the file has a non-empty id that contains no `/`, does not begin with `#`, and is unique among its siblings; otherwise every segment is `#<usage instance>`. Segments are joined by `/`: `1011/1007/1004`.
- **O3.** **Read form.** A segment is a usage id or `#<instance>`; both are accepted in every file, and a segment that names one usage by id and another by instance is refused as ambiguous. A path is valid only if it runs from the root down usage by usage and ends at the selected part (or names it, when `--part` is absent; if both are given they must agree). Otherwise `occurrence_not_found` or `occurrence_mismatch`, listing the part's occurrences.
- **O4.** **Order.** Occurrences are ordered by their paths, compared segment by segment as usage `#id`s (numbers), never by stepq's traversal order.
- **O5.** **Stored form.** A path is stored as its usage `#id`s (`[1764, 1247]`) plus `label`, the usage ids (`["NAUO8", "NAUO2"]`), so both forms print back.
- **O6.** **Default (D11).** Without `--occurrence`, every occurrence of the part, in O4 order.

Measured: every one of the 3,481 fixture usages has an id, unique in its file. Usage names are `'Next assembly relationship'` on 3,393 of them, and Open CASCADE writes empty ones for sub-assemblies: they cannot address anything. 147 sub-assembly occurrences are repeated, so one leaf can have paths that differ only in their first segments.

## 6. Placing features and the floor

### 6.1 Placing (P)

- **P1.** **Read once.** Each distinct leaf definition is read once into one `Part`, in its own frame (M2, 5.1). 3,861 leaf occurrences share 1,309 definitions.
- **P2.** **One transform per occurrence.** `T_occ` is stepq's composed occurrence transform (root to component, in mm through each context's unit), copied into `geom::Rigid` by `Rigid::new`. The solid's own transform is not applied again: the `Part` already includes it.
- **P3.** **Frame.** `T_occ` maps a representation the solid is listed from (check 37). All 3,224 accepted fixture occurrences place exactly the part's frame.
- **P4.** **Moving a value.** A point `p ↦ R p + t`; a direction `d ↦ R d`; an axis maps its point and direction; a plane `n ↦ R n`, `offset ↦ offset + (R n)·t` (M2, 7.4); radii, lengths, stations, sweeps and areas are unchanged. Outline vertices and arc centres move as points. Values are moved on demand, never stored per occurrence, and pass `geom::finite`.
- **P5.** **Into the target's frame.** A neighbour j is compared with target occurrence k in k's part frame, through `T_k⁻¹ · T_j`. That adds one transform of rounding (about 1e-11 mm in the fixtures) and keeps coordinates small.
- **P6.** **Sign.** M2's canonical sign of a through hole or two-ended shaft (9.4) is not canonical after a move. Axes are compared as lines; blind holes and one-rooted shafts keep their meaningful direction.
- **P7.** **No reflections.** An `axis2_placement_3d` cannot state a mirror, stepq refuses every `cartesian_transformation_operator`, and `Rigid::new` refuses a determinant of −1. 0 of 3,481 usages are reflections. A mirrored part is its own definition and is read like any part.
- **P8.** **Mirror intent is lost by writers.** cadquery's mirrored placement is written by Open CASCADE as a proper rotation, so the body lands on the other side of its hole pattern. fitment follows the file, as Open CASCADE does when it reads it back.
- **P9.** **Accuracy.** Composed placements matched the construction to 9.4e-12 mm at 12 levels deep; composed rotations are orthonormal to 7e-16; Open CASCADE's XCAF locations equal stepq's one to one (worst 2.8e-14 mm, weldment).

### 6.2 The floor and the tolerances (Z)

- **Z1.** Each occurrence carries `part_uncertainty_mm` (its part's `max_uncertainty_mm`), `vertex_offset_mm` (its solid's `max_vertex_offset_mm`) and `placement_uncertainty_mm`: the largest length uncertainty stated by the contexts it is placed through, the root's included. A context that states none contributes nothing, with a note.
- **Z2.** **The floor of a relation** between occurrences X and Y is the largest of those three values of X and of Y (R17). A requirement's floor is the largest over its relations. Every relation records it.
- **Z3.** **The contact tolerance** τ = max(`CONTACT_MM`, floor), with `CONTACT_MM` = 0.01 mm (D9's default, as an extraction constant). It decides coaxial labels, axial touch, line-to-line margins and plane contact.
- **Z4.** **A value within τ of a boundary resolves to the case with more requirements:** touching counts as touching, a margin within τ pins, a doubt about coaxiality is decided by whichever reading requires more. A larger floor can only add requirements or pin limits: on `as1_pe` (0.672 mm) a 0.3 mm clearance pins its hole, a false negative.
- **Z5.** **`PARALLEL_SINE_REL`** = 0.01 decides which axes and planes are compared as parallel; the bounds of section 7 hold for any angle within it (X4).
- **Z6.** **`NEAR_MM`** = 1 mm bounds the near band of plane contacts (C5) and the reach of the searches of the bounded rules (N3). It is an extraction constant, not a match tolerance.
- **Z7.** The floor never widens M2's extraction tolerances (`IDENTITY_MM`, R7 of M2), and is never a reason to call two features one.

Measured, per accepted leaf occurrence:

| File | Part (mm) | Placement path (mm) | Path larger | Median floor, part only → with path |
| --- | --- | --- | --- | --- |
| AS1 AC, EC, UG | 1e-6, 1e-7, 2e-5 | same | 0 of 18 | unchanged |
| AS1 MD | 2.1e-6 | 0.01 | 18 of 18 | 2.1e-6 → 0.01 |
| AS1 TC | 1e-13 | none stated | 0 | unchanged |
| `as1_pe` | 0.115 to 0.672 | 0.672 | 11 of 12 | 0.36 → 0.67 |
| `weldment_asm_solid` | 0.0014 to 0.058 | 0.058 | 105 of 108 | 0.0036 → 0.058 |
| Creo (OCP set, accepted leaves) | 0.0004 to 0.22 | 0.11 to 0.40 | all | about 0.001 → 0.1 to 0.4 |
| Generated | 1e-7 | 1e-7 | 0 | unchanged |

Creo states a closure proportional to model size, so the assembly's is larger than any part's. With the floor above D9's 0.01 mm, M4 rejects what it cannot verify (Soundness rule 4): a coverage question for M4, not a soundness one (section 18).

## 7. Relations

Rules in this section and the next are in the target part's frame (P5), for one target occurrence and all its neighbours.

### 7.1 Neighbours (N)

- **N1.** **Every leaf occurrence except the target occurrence is a neighbour.**
- **N2.** **Other occurrences of the target part are neighbours,** and every requirement they contribute to, even as provenance only, is flagged `self`. The candidate replaces both sides, so the two sides of a `self` relation are coupled: a B or I relation between two selected occurrences pins both features to their original diameters (whatever its fit), and M7 checks the candidate's copies against each other (V1). With `--occurrence`, the unselected occurrences stay as they are and are ordinary neighbours. AS1: the two L-brackets' rod holes are coaxial, 150 mm apart.
- **N3.** **Search, not pruning by distance.** Candidates for every rule come from bounding-volume trees over all neighbours' placed elements, each with a sound box: axial segments grown by their radius, whatever their direction; plane faces by their outline's box (arcs' extreme points included); other faces, and anything with no sound box, always. The geometric tests of the rules come after. Only rules whose reach is bounded (B, I, H, Q, C, A's occupant test) query a box grown by `NEAR_MM` plus the floor; J2 queries the whole axis line (J2).
- **N4.** **Bounded work.** Every candidate pair the search returns counts toward `MAX_RELATIONS` (check 39), whether or not it becomes a relation, so a hostile file (10⁵ coincident occurrences, tiled coplanar faces) refuses instead of running quadratically.
- **N5.** **Order.** Neighbours by occurrence path, features by id; the output does not depend on search order.

### 7.2 Axial geometry (X)

The axial features of an occurrence are its hole stacks and shaft stacks (M2 9.5), segment by segment, plus every cylinder in no stack (partial and undecided ones; cylinders in voids are skipped). A stack's segment spans its stations. A partial or undecided cylinder spans the stations of all its boundary vertices and arcs, and is unbounded on any side where it has an edge M2 did not read (a B-spline or ellipse): its end-arc stations alone do not bound it.

For a target axial feature A (unit direction a, point P, radius r_A) and a neighbour axial feature B (direction b, radius r_B), with s(Q) = (Q − P)·a the station along A:

- **X1.** **Parallel:** sin θ = |a × b| ≤ `PARALLEL_SINE_REL`. Every related pair in the 9 core fixtures is parallel to sin θ ≤ 1e-12. A neighbour shaft, cone or partial convex cylinder beyond that angle whose cross-section may meet a target hole or shaft is `skew`, an `unresolved` requirement (U7). A skew neighbour hole is air and constrains nothing.
- **X2.** **Interval.** B's segment maps to [lo, hi] on A's axis through s. For material inside a target hole (B) it is widened by r_B sin θ, the tilt of its end discs; for a neighbour hole around a target shaft (I) it is shrunk by the same, where the tilted hole no longer contains the full disc. An unknown side is −∞ or +∞ (U3). The overlap is L = min(hi_A, hi_B) − max(lo_A, lo_B): `overlap` if L > τ, `touch` if |L| ≤ τ, `gap` if L < −τ.
- **X3.** **Offset.** Over the overlap (or, for a gap, over the two facing ends), e_max and e_min are the largest and smallest distances from A's axis line to B's axis. The distance is convex along B's axis, so e_max is at an end of the interval and e_min is found in closed form.
- **X4.** **Tilt.** Within the overlap, a shaft of radius r tilted by θ lies inside the cylinder of radius e_max + r / cos θ about A's axis; a hole of radius r tilted by θ contains the cylinder of radius r − e_max about A's axis. Every bound uses these radii, so X1's window costs no soundness.
- **X5.** **Meeting.** The cross-sections meet when e_min < r_A + r_B / cos θ + τ. Otherwise there is no axial relation; the other rules still apply.
- **X6.** **Coaxial** means e_max ≤ τ. It labels a relation and decides between interference and conflict (B3, B4); it never decides whether a relation exists.
- **X7.** **Zones.** A stack's segment stands for more than its cylinder (PR 3 review): each transition between two segments (a cone step, a fillet, a cone under a head) goes to the hole's narrower or the shaft's wider neighbouring segment, and each end segment reaches its rim (a chamfer, a countersink, a pin's tapered tip). A zone is taken only when every cone and torus of the stack that may reach into it lies outside the hole's cylinder or inside the shaft's (each cone's stations follow from its apex, angle and radii, each torus's from its centre and minor radius; one with unknown radii forbids the zone), so every clearance bound stays sound there: a chamfer or countersink qualifies, a countersunk head's cone or a bead does not, and its zone stays as before (s18). A line-to-line or interference fit holds only where both cylinders themselves overlap: within a zone it is a cone or step bearing, which Q, C1 and C6 read, so the relation is clipped to the cylinders' overlap, or with none, kept as B6's provenance (if it opens a rim) or dropped. A piece of a span in a zone is never pinned, for the original has no single diameter there (Y4).

### 7.3 Target hole, neighbour shaft (B)

For a target hole segment (radius r_h, interval I_h) and a neighbour shaft segment (radius r_s) whose intervals overlap or touch (L > −τ: a shaft that enters by less than τ still counts, Z4), the margin is m = r_h − (e_max + r_s / cos θ) and the clear diameter is c = 2(e_max + r_s / cos θ), a hole about the target's axis that the shaft is sure to pass.

- **B1.** **Clearance:** m > τ. Over the overlap, the hole must be at least c across; under D6 strict, no larger than the original. Labelled `offset`, with e_max, when e_max > τ: a Ø6 bolt 0.2 mm off a Ø6.6 hole needs at least 6.4.
- **B2.** **Line to line:** |m| ≤ τ. Pinned to the original diameter, `line_to_line`, `thread_not_confirmed`. All of AS1.
- **B3.** **Interference, coaxial:** m < −τ and e_max ≤ τ. Pinned, `interference`, `thread_not_confirmed`: a threaded or press fit. An Ø6 screw in an Ø5 tapped hole.
- **B4.** **Conflict:** m < −τ and e_max > τ. The shaft runs through the hole wall in the original. Pinned, `interference`, flagged `conflict` (not `thread_not_confirmed`) for review.
- **B5.** **Occupied span.** Every B relation carries its overlap in the target hole's own stations. The candidate's hole wall must exist over all of it: for a blind target hole, a minimum full-diameter depth. A touch gives a span of zero length, which still requires the wall at that station.
- **B6.** **Opening.** If the shaft continues past a target rim by more than τ, the candidate's hole must be open on that side. That end of the original is then an `opening`, or unknown (U8). Both reach their rims (X7), so a chamfered tip that pokes out counts.
- **B7.** **Steps.** Each target segment is judged against each shaft segment it overlaps: a cap screw's Ø10 head in an Ø11 counterbore needs [10, 11] over the counterbore, its Ø6 shank [6, 6.6] below.
- **B8.** **Unread occupants.** Material in a target hole that no shaft segment accounts for is not "nothing": a D-shaped or hexagonal shaft, a key, a tapered pin (a cone), a set screw's end, a B-spline bolt. Any neighbour face that may meet the open core of a target hole segment (radius r_h − τ, its interval shrunk by τ at both ends) pins that segment to its original diameter, fit `not_read`, flagged `occupant_unread`. The faces B already reads are excluded: a read shaft stack's segment walls, transitions and end faces, and the walls of a neighbour hole J reads. "May meet" is decided conservatively: a plane face by its over-approximated region (C2) against the core cylinder, a cylinder, cone or torus face by its sound box in the hole's axis frame, an `other` face always. A neighbour body that swallows the hole with no face inside it is a clash of the originals, M7's to report (s14). The core runs over the segment's zones (X7), a parallel hole-sense neighbour cylinder that stays outside the core is no occupant (a wider bore beside it; a shaft's cylinder, even a D-shaft's partial one, always may be), the span pinned is the cylinder's own, and a face whose box runs more than τ past an end segment's rim requires that end open, as B6 (s45: a hex bar through a round hole).

Stations that no span covers are not required: a counterbore's top above a cap screw's head is free.

### 7.4 Target shaft, neighbour hole (I)

The mirror image, for pins, bosses, bolts, rods and the outside of washers and spacers.

- **I1.** **Clearance:** m = r_h − (e_max + r_s / cos θ) > τ. Over the overlap (X2, shrunk), the shaft may be at most 2(r_h − e_max) across (X4); under D6 strict, no smaller than the original.
- **I2.** **Line to line or interference:** m ≤ τ. Pinned, with the label of B2, B3 or B4.
- **I3.** **Engagement.** The occupied span, in the shaft's own stations: a bolt must reach through its nut, a screw must engage its tapped hole as far as the original did.
- **I4.** **Bottoming.** When the neighbour hole is blind and the shaft's end reaches its floor, the end face against the floor is a seat (C1). Otherwise the gap to the floor is recorded as `max_extra_mm`, for information.
- **I5.** **Shaft in shaft:** coaxial neighbour and target shafts that overlap carry no requirement from each other (a bolt inside a washer is the washer's hole and the bolt's shaft).
- **I6.** **Shafts end to end:** coaxial shafts that touch end to end within τ (a pin abutting a pin) require the target shaft's axis, flagged `abutting`.
- **I7.** **Partial holes around a shaft.** A target shaft inside a neighbour's partial concave cylinder (a slot, a V-groove) is pinned to its original diameter over the overlap, fit `not_read`, flagged `partial_neighbour`.

### 7.5 Target hole, neighbour hole (J)

J applies only at a target hole end of kind `opening` (M2 9.1), to a neighbour hole that begins at or beyond that rim: two holes that overlap along the axis are nested, not continued, and the material between them is B8's.

- **J1.** **Continuation.** A neighbour hole on the target hole's axis (X5) whose near end touches the rim (|L| ≤ τ): the target hole's axis is required, and the candidate's hole must be open on that side. If a read shaft spans the joint (it occupies the target hole over more than τ and runs past the rim into the neighbour hole, further than the gap by more than τ), J1 adds provenance only; a shaft flush with the rim (s46) or short of the neighbour hole (s44) does not span it. Otherwise no fastener was read across the joint: every span of the target hole that no read shaft occupies is pinned to its original diameter, fit `not_read` (R12).
- **J2.** **Beyond a gap.** The same with L < −τ, recording `gap_mm`, unless J3 applies. The neighbour holes on the axis beyond each rim are examined nearest first, and the first that J3 does not block is taken; a farther one adds only provenance. A blocked hole does not block a farther one (PR 3 review, s43): J3's disc is as narrow as the narrower hole, and a farther hole may be wider. The search runs along the whole axis line (N3), not within `NEAR_MM`.
- **J3.** **Blocked line of sight.** J2 is dropped when, between the two rims, the axis crosses a plane face of any occurrence whose under-approximated region (C2, inverted: arcs replaced on the side toward the face's interior) contains the disc of radius min(r_A, r_B) + e_max + τ about the crossing point: every straight fastener that could pass both holes would pass through material. A face with an unknown outline or a curved face never proves a block. The faces are found by one sorted sweep along the axis. Weldment (probe, point test): 17 of 39 J2-only holes are blocked.

### 7.6 Heads and shoulders (H)

- **H1.** **A larger neighbour cylinder or cone ending at a target hole's rim**, including a step's rim inside a stack (s02's counterbore floor) (touch, r_N > r_h, e_max ≤ r_N − r_h): the target's plane at the rim is a seat, with the ring from d_h to 2 r_N recorded for review. Usually C1 finds the same seat from the head's end face, and H1 merges into it; H1 matters when the head's end is not a plane C1 can read (a cone under a pan head, an unread face).
- **H2.** A large neighbour cylinder whose end covers a hole off its axis (vaccase: a Ø152 flange over Ø7.9 bolt holes 65 mm off) is left to C1, which sees the flange's face.

### 7.7 Conical seats (Q)

- **Q1.** A target cone and a neighbour cone of opposite sense, coaxial, with apexes within τ of each other's surface (across the axis by at most τ, along it by at most τ / sin of the semi-angle: a shallow taper sits deep, PR 4 review) and semi-angles within `PARALLEL_SINE_REL`: a conical seat with the axis, apex, included angle and the radial range they share. A 90° countersunk screw in a 90° countersink. Cones that touch at other angles (a 90° head in an 82° countersink) are C6's.
- **Q2.** A conical seat and an opposed plane seat on the same axis give a grip from the apex to the plane (L2).

The radial range needs each cone's radii, which M2's `Cone` does not carry; R22 adds them.

### 7.8 Contacts (C)

- **C1.** **Opposed faces.** A target plane face f_T and a neighbour plane face f_N whose normals are opposed (n_T·n_N ≤ −cos θ with sin θ ≤ `PARALLEL_SINE_REL`) and whose gap g lies within τ, where g is the neighbour plane's signed distance from f_T evaluated over f_T's outline (its smallest absolute value, the side with more contacts), and whose outlines may overlap (C2): a contact. Negative g is interference. The gap is also measured at the neighbour outline's vertices, and where the two planes cross between the points measured it is 0 (a block tilted within the window, resting on one edge).
- **C2.** **May overlap.** The two face regions, projected onto the target plane, may share interior. M3 decides "certainly disjoint" or "may overlap", never the exact intersection:
  - certainly disjoint when the loops' bounding boxes, each including its arcs' extreme points and grown by τ, are disjoint;
  - otherwise each region is over-approximated by polygons: each arc is replaced by a polyline on its far side from the region, its chord when the arc's centre lies outside the region (a fillet in an inner corner, a tab on a hole) and its circumscribing polyline when the centre lies inside (a rounded outer corner, a circular hole). The pair may overlap when the polygons' interiors intersect;
  - this only adds contacts. Arcs that merely touch read as overlapping, the safe side.
- **C3.** **Unknown outline.** If either outline is unknown, the pair may overlap (M2 7.4).
- **C4.** **Required region (R14).** A contact requires the target face's region (`basis: target_face`), stated as the target face's exact outline. If the target face's outline is unknown, `basis: unknown`: M4 cannot cover it (M2 7.4), so the seat is unverifiable. The true contact lies inside the target face, so the basis is sound, and the original covers it.
- **C5.** **Near band.** τ < |g| ≤ max(`NEAR_MM`, τ) gives the same contact flagged `near_gap` (g > 0) or `interference` (g < 0), at the target's own plane.
- **C6.** **Contacts no rule reads.** A target face and a neighbour face that touch in a way B, I, J, H, Q and C1 do not read give an `unresolved` requirement (`contact_no_rule`), anchored on the target face (Soundness rule 2): a rod lying on a flat or in a V-block, a set screw's flat on a shaft, a 90° head in an 82° countersink, a wedge's edge. "Touch" is decided conservatively and in closed form where it exists: a neighbour vertex, line edge or circle edge within τ of a target face's surface and possibly inside its region; a cylinder tangent to a plane (axis parallel, distance r ± τ) or to another cylinder; and, for other pairs, their sound boxes grown by τ. Edges that bound a neighbour face already in a relation with that target face are explained by it: a bolt head's hex edges on the face it seats on add nothing. A pair with an `other` face is U4's or U5's, not C6's. As built (`touch.rs`, PR 4 review): every target face of any kind is an anchor (a rod lying on a floor is the rod's as much as the floor's), and every neighbour face whose sound box, grown by τ, meets it a candidate (a face with no sound box always is). Explained are the pairs C1, Q1 and H1 read and an axial relation's target wall against the neighbour faces it names, then the faces sharing an edge with either face of any of these (one step, from plane outlines), and for Q1 and H1 the other faces of both stacks, which meet the seated ones only along shared rims; B8's and U7's faces against their whole target feature. Closed forms: two planes not facing each other touch where the part of one within τ of the other's plane reaches the inside of its region (not merely its boundary edge, which the face across that edge carries), and nothing where either face lies wholly off the other's plane; a cylinder parallel to a plane touches it in the bands where it crosses within τ, square to it by its disc (through a hole of the face, nothing); a cone square to a plane by its disc likewise; parallel cylinders where tangent (two shafts outside each other, a shaft against a hole's wall from inside) or crossing over common stations; a cylinder and a parallel cone where the cone's radius over their common stations reaches it. Faces sunk past τ into each other inside a region are a clash of the originals, M7's.

### 7.9 Grip (L)

- **L1.** Two target seats whose normals are opposed and whose required regions may overlap when projected along the normal (C2's test): a grip of length offset₁ + offset₂ (M2 7.4). The target holes that open onto both seats are recorded.
- **L2.** A conical seat and an opposed plane seat give a grip from the apex. Its length is signed, negative where the apex lies beyond the plane (a countersink through a thin plate).

PLAN says "on one axis". L1 is broader: a plate clamped without a fastener has its thickness fixed too, and more requirements is the safe side.

### 7.10 Unknowns (U)

M2 section 9.6 binds M3: never drop a requirement because something is unknown. Every requirement any reading of the unknown would give is produced and flagged; where none can be stated, an unverifiable requirement fails every candidate until the user removes it.

- **U1.** **Partial target cylinders with something in them or around them:** a partial concave cylinder (a slot end, a notch, a half hole) with a neighbour shaft in it gives a `partial_bore` with the axis, the original and clear diameters, the sweep and the span; a partial convex cylinder (a D-shaft, a fillet) inside a neighbour hole gives a `partial_shaft` (R16). The open end of a slot with nothing in it is not required.
- **U2.** **Undecided target cylinder** (an angled hole, a B-spline-bounded bore): its axis and diameter are known and its extent is not, so U3 applies and its requirement is flagged `target_undecided`. M4 may satisfy it only with a full candidate hole, and v1 does not: it fails until removed.
- **U3.** **Unknown extent** (an irregular or unknown end, an undecided cylinder): unbounded on that side, or bounded by the part's `extent` projected on the axis when the part has one. A relation that exists only through that side is flagged `extent_unknown`. No fit is read there: the material past a known end may flare (a countersunk head past a shank whose end the walk could not read), so a relation's fit holds only over the stations both elements know (their cylinders and zones), and the rest of its span is `not_read`, flagged `extent_unknown` (s18). Vaccase: 17 pinned bores on the cross come from flanges up to 200 mm past a bore's known end, because the cross's bores end in each other.
- **U4.** **Unread neighbour faces.** A neighbour definition with any `other` face, or with an undecided cylinder, has no sound extent, so its occurrences are candidates for every rule (N3) and its `other` faces may meet every target hole (B8). It also adds one `unresolved` requirement per definition, naming it and its unread faces; the target's own definition is no exception when other occurrences of it are selected. Moon buggy: 7 of 19 occurrences; AS1 think3: all 18.
- **U5.** **Unread target faces.** Each target `other` face is an `unresolved` requirement: it may be an interface (Soundness rule 2). `part.complete` does not count them (M2 9.6), so M3 reads `other_faces` itself. AS1 think3: 2 to 12 per part.
- **U6.** **Unknown outlines** follow C3 and C4.
- **U7.** **Skew relations** (X1) are `unresolved`.
- **U8.** **An unknown target hole end** gives no depth requirement (M2 9.6); the bore says so (`depth: unknown_end`), and B5's spans still apply. An opening required at an unknown end (B6) makes the bore unverifiable (flag `open_at_unknown_end`).

## 8. Requirements

### 8.1 One per target feature (Y)

- **Y1.** **Combining.** All relations on one target feature become one requirement:
  - a hole stack (or undecided cylinder) gets one `bore`, a partial concave cylinder one `partial_bore`: its axis, and spans, the maximal intervals of its stations over which the set of relations is constant. In each span the clear diameter is the largest, the fit the strictest present (`interference`, then `line_to_line`, then `not_read`, then `clearance`), and the span is pinned unless the fit is `clearance`;
  - a shaft stack gets one `shaft`, combined the same way with the smallest allowed diameter; a partial convex cylinder one `partial_shaft`;
  - a target plane (M2's plane, the union of its coplanar faces) gets one `seat`, with one contact per target face in contact, each listing every neighbour face that touches it;
  - a target cone gets one `conical_seat`;
  - a pair of seats gets one `grip`;
  - every unread situation on one anchor gets one `unresolved`, listing its causes.
- **Y2.** **Provenance.** Each relation stays listed under its requirement as a source (8.4): the rule, the target occurrence, the neighbour occurrence, the neighbour feature and faces (`#id`s), e_max, θ, the margin or gap, the span, the floor and the flags.
- **Y3.** **Identity.** A requirement is named by its kind and the target features it constrains (section 9.2), never by a neighbour.
- **Y4.** **The original is a witness of every verifiable requirement.** Every requirement names the target feature it was derived from, and every limit of a verifiable one contains that feature's own value: a clearance span is [c, d_orig] with c < d_orig (since m > τ), a pinned span is d_orig, a seat is at the target's own plane and requires the target's own face, a grip is the target's own separation, a required opening is an `opening` of the original. Combining (Y1) intersects intervals that all contain d_orig, so the result does too. The unverifiable requirements fail the original as they fail every candidate, by design: `unresolved`, `partial_bore`, `partial_shaft`, a bore or shaft flagged `target_undecided`, a bore flagged `open_at_unknown_end`, a seat contact with `basis: unknown`, and a conical seat with no radial range. A test asserts both halves on every socket; M4 runs the original as a candidate and expects exactly the unverifiable ones to fail.

### 8.2 Left out (A)

- **A1.** A target hole stack or undecided cylinder with no requirement from B (B8 included), J, H, Q or U is left out, with reason `nothing_on_axis` or `blocked_gap` (J3), and listed in the socket's `left_out` with its axis and diameter. Because B8 counts every unread neighbour face as a possible occupant, no hole is left out while such a face exists in the assembly.
- **A2.** A target shaft with no requirement is left out the same way (a boss in free air). Any collision is M7's.
- **A3.** **The anchor partition.** In every socket, each target hole, shaft and undecided cylinder is either the anchor of one or more requirements or listed in `left_out`, never both and never neither. `from_json` checks it, so the M3 exit criterion is a property of every socket file. Partial cylinders appear only through their requirements; those with nothing near them (fillets) are not listed.

### 8.3 The envelope (V)

- **V1.** The envelope is not a requirement. M7 checks each placed copy of the candidate against every solid of the assembly except the selected occurrences, and against the candidate's other placed copies (`check_set: others_and_copies`), with the socket's requirements as its declared contact zones.
- **V2.** The socket records the target's `extent` in its own frame (null when unbounded), for review and a later "within the original envelope" shortcut.

M3 computes no volumes: fitment computes only what its rules need.

### 8.4 Kinds

| Kind | Id | What it requires | Fields |
| --- | --- | --- | --- |
| `bore` | `bore:hole-88` | A hole on this axis, with the diameters of each span over its length, open where stated | `hole` (or undecided `cylinder`), `axis`, `spans` [{`from_mm`, `to_mm`, `hole_diameter_mm` (the original), `clear_diameter_mm` (null if no shaft read), `fit`}], `open` {`start`, `finish`}, `depth` (`known` or `unknown_end`) |
| `shaft` | `shaft:shaft-12` | A shaft on this axis | `shaft`, `axis`, `spans` [{`from_mm`, `to_mm`, `shaft_diameter_mm`, `clear_diameter_mm` (the largest allowed, null if pinned), `fit`}], `max_extra_mm` |
| `seat` | `seat:plane-362` | Flat material at this plane covering each contact's region | `plane`, `normal`, `offset_mm`, `contacts` [{`face`, `basis`, `outline`, `by` [{`neighbour`, `face`, `gap_mm`, `flags`}]}], `rings` (H1, for review) |
| `conical_seat` | `conical_seat:cone-40` | A cone on this axis, apex and angle, covering the radial range | `cone`, `axis`, `apex_mm`, `included_angle_rad`, `radii_mm` (both cones' common range; the target cone's own where the neighbour's is unknown; null, and the seat unverifiable, where the target's is) |
| `grip` | `grip:plane-362+plane-506` | Both seats at this separation | `seats` (two plane or cone ids), `length_mm`, `holes` (target holes opening onto both) |
| `partial_bore`, `partial_shaft` | `partial_bore:cylinder-412` | Not verifiable in M4 v1 | `cylinder`, `axis`, `diameter_mm`, `clear_diameter_mm`, `sweep_rad`, `from_mm`, `to_mm` |
| `unresolved` | `unresolved:other-77`, `unresolved:plane-30`, `unresolved:part-1268` | Never verified; fails until removed | `causes` [{`cause` (`target_unread_face`, `neighbour_unread`, `skew`, `contact_no_rule`), `faces`}], `feature` or `definition` |

Every requirement carries `floor_mm`, sorted `flags` and `sources`. A source is {`rule`, `occurrence`, `neighbour`, `feature`, `faces`, `e_max_mm`, `sine`, `margin_mm` or `gap_mm`, `span_mm`, `floor_mm`, `flags`}, with null where a field does not apply. Flags: `self`, `thread_not_confirmed`, `offset`, `conflict`, `extent_unknown`, `target_undecided`, `occupant_unread`, `partial_neighbour`, `abutting`, `open_at_unknown_end`, `near_gap`, `interference`, `unknown_outline`. Stations are mm along `axis.direction` from `axis.point_mm`, the target feature's own axis with M2's 9.4 conventions (a blind hole's stations run from its bottom). `extent` is `{ "min_mm": [x, y, z], "max_mm": [x, y, z] }`, or null when unbounded. An `unresolved:part-<n>` names a neighbour definition `#id`; it is the same in every socket of the document.

## 9. The socket document

### 9.1 Sockets and merging

- **One document per target part.** It holds the selected occurrences and a list of sockets, each covering the occurrences it merges.
- **Key.** Only occurrences with the same requirement ids and the same `left_out` can merge.
- **Values.** Within a key, an occurrence joins the first socket whose every value is within `MERGE_MM` = 1e-6 mm (lengths) or `MERGE_SINE` = 1e-9 (directions) of its own; otherwise it starts one. Values are bucketed by a quantized key, so merging stays n log n; a value on a bin edge only costs an unmerged socket.
- **Merged values are the strictest** of the members: the larger clear diameter in a bore, the smaller allowed diameter in a shaft, the union of spans, contacts and openings, the larger floor. Values from the target's own features are bit-identical across occurrences and must be equal. `sources` is the union.
- **Numbering:** `socket-1`, `socket-2`, … by smallest occurrence path.

AS1-AC: the two L-brackets' values differ by about 1e-13 mm, so exact equality gives 2 sockets and `MERGE_MM` gives 1. Bolts 6 → 1; nuts 8 → 4 (on bolts and on the rod, in two orientations).

### 9.2 Requirement ids

- **Grammar:** `<kind>:<anchor>[+<anchor>]`. An anchor is a target `FeatureId` (`hole-88`, `other-77`), or `part-<n>` for a neighbour definition (U4), which is not a feature; anchors are sorted by their `#id`s.
- **Derived, not free text.** `from_json` refuses an id that is not what the requirement gives.
- **Order** within a socket: by smallest anchor `#id` (numerically), then kind in the order of the table above; `unresolved:part-<n>` last, by `#id`.
- **Stable for the same bytes,** and nothing more. Across re-exports nothing identifies a requirement: in the six AS1 exports, product ids, usage ids, `#id`s, tree shapes and even part frames all differ. `source.sha256` scopes every id.
- **For M4,** a verdict names `<socket>/<requirement>`, such as `socket-1/bore:hole-88`.

### 9.3 `fitment.socket` v1

The envelope:

| Field | Meaning |
| --- | --- |
| `format`, `format_version` | `"fitment.socket"`, 1. Frozen at the first publish (M4) |
| `extractor_version`, `socket_rules_version` | `EXTRACTOR_VERSION` (features) and `SOCKET_RULES_VERSION` (sections 7 to 9). `is_current()` needs both (R20) |
| `extraction` | `{ "features": <features.v1 extraction>, "socket": { contact_mm, near_mm, parallel_sine_rel, merge_mm, merge_sine, max_occurrences, max_relations } }` |
| `source` | `sha256`, `bytes` and `header`. No file name or path |
| `selection` | `part` (as given, or null when only `--occurrence` was), `matched_by` (`definition_instance`, `product_id`, `product_name`, `occurrence`), `occurrence` (a path or null) |
| `status`, `refusals`, `notes` | As in features.v1. A refused document has `extracted`, `digest` null and empty review lists |
| `extracted` | `target` (definition, frame, solid, uncertainties, `complete`, `extent`, `anchors`), `occurrences` (path, label, transform, uncertainties, socket), `neighbours` (those cited by a source: path, label, definition, transform, uncertainties), `sockets` (id, occurrences, requirements, left_out), `envelope` |
| `digest` | `"sha256:<64 hex>"` of the compact canonical JSON of the document without `digest`, `review` and `stale_reviews`: versions, constants, source, selection and `extracted`. M4 uses only constants inside the digest |
| `review` | One entry per (socket, requirement): `status` (`unreviewed`, `kept`, `removed`) and `note` |
| `stale_reviews` | Decisions that could not be carried over (9.5) |

Conventions are features.v1's (M2 10.2): snake_case with unit suffixes, `[x, y, z]` points, STEP references as bare integers, feature, requirement and socket ids as strings, internally tagged enums, `null` written, `deny_unknown_fields`, pretty JSON plus `\n`, `float_roundtrip` (which the digest relies on).

An example, for the generated case s01 (section 13.3), target BRACKET. The feature `#id`s are illustrative (and in the order 9.2 gives); every value is the construction's.

```json
{
  "format": "fitment.socket", "format_version": 1,
  "extractor_version": 6, "socket_rules_version": 1,
  "extraction": { "features": { "identity_mm": 1e-6, "…": "as features.v1" },
                  "socket": { "contact_mm": 0.01, "near_mm": 1.0, "parallel_sine_rel": 0.01,
                              "merge_mm": 1e-6, "merge_sine": 1e-9,
                              "max_occurrences": 100000, "max_relations": 10000000 } },
  "source": { "sha256": "…", "bytes": 56564, "header": { "…": "…" } },
  "selection": { "part": "BRACKET", "matched_by": "product_id", "occurrence": null },
  "status": "accepted", "refusals": [], "notes": [],
  "extracted": {
    "target": { "definition": { "instance": 30, "product": { "instance": 28, "id": "BRACKET", "name": "BRACKET" } },
                "frame": { "shape_representation": 34 }, "solid": "solid-120",
                "part_uncertainty_mm": 1e-7, "vertex_offset_mm": 0.0, "complete": true,
                "extent": { "min_mm": [-40.0, -25.0, 0.0], "max_mm": [40.0, 25.0, 8.0] },
                "anchors": ["hole-41", "hole-51", "hole-61", "hole-71", "hole-81"] },
    "occurrences": [ { "path": [12], "label": ["2"], "socket": "socket-1",
                       "transform": { "rotation": [[1, 0, 0], [0, 1, 0], [0, 0, 1]], "translation_mm": [0, 0, 0] },
                       "part_uncertainty_mm": 1e-7, "placement_uncertainty_mm": 1e-7, "vertex_offset_mm": 0.0 } ],
    "neighbours": [ { "path": [14], "label": ["3"], "definition": { "…": "BOLT" }, "transform": { "…": "at (-30, -15, 8)" },
                      "part_uncertainty_mm": 1e-7, "placement_uncertainty_mm": 1e-7, "vertex_offset_mm": 0.0 },
                    { "…": "the plate, the other three bolts and the four nuts" } ],
    "sockets": [ {
      "id": "socket-1", "occurrences": [[12]],
      "requirements": [
        { "id": "bore:hole-41",
          "requires": { "kind": "bore", "hole": "hole-41",
                        "axis": { "point_mm": [-30.0, -15.0, 0.0], "direction": [0.0, 0.0, 1.0] },
                        "spans": [ { "from_mm": 0.0, "to_mm": 8.0, "hole_diameter_mm": 6.6,
                                     "clear_diameter_mm": 6.0, "fit": "clearance" } ],
                        "open": { "start": true, "finish": false }, "depth": "known" },
          "floor_mm": 1e-7, "flags": [],
          "sources": [ { "rule": "B1", "occurrence": [12], "neighbour": [14], "feature": "shaft-88", "faces": [90],
                         "e_max_mm": 0.0, "sine": 0.0, "margin_mm": 0.3, "gap_mm": null,
                         "span_mm": [0.0, 8.0], "floor_mm": 1e-7, "flags": [] },
                       { "rule": "J1", "occurrence": [12], "neighbour": [10], "feature": "hole-160", "faces": [162],
                         "e_max_mm": 0.0, "sine": 0.0, "margin_mm": null, "gap_mm": 0.0,
                         "span_mm": null, "floor_mm": 1e-7, "flags": [] } ] },
        { "…": "bore:hole-51, bore:hole-61, bore:hole-71 alike" },
        { "id": "seat:plane-150",
          "requires": { "kind": "seat", "plane": "plane-150", "normal": [0.0, 0.0, -1.0], "offset_mm": 0.0,
                        "contacts": [ { "face": 150, "basis": "target_face",
                                        "outline": { "status": "known", "loops": [ "…80 × 50 minus four Ø6.6 and one Ø8 circle…" ] },
                                        "by": [ { "neighbour": [10], "face": 170, "gap_mm": 0.0, "flags": [] } ] } ],
                        "rings": [] },
          "floor_mm": 1e-7, "flags": [], "sources": [ { "rule": "C1", "neighbour": [10], "…": "the plate's top face" } ] },
        { "id": "grip:plane-150+plane-180",
          "requires": { "kind": "grip", "seats": ["plane-150", "plane-180"], "length_mm": 8.0,
                        "holes": ["hole-41", "hole-51", "hole-61", "hole-71"] },
          "floor_mm": 1e-7, "flags": [], "sources": [] },
        { "id": "seat:plane-180", "…": "z = 8, +z: one contact, face 180, by the four bolt heads' bearing faces" }
      ],
      "left_out": [ { "feature": "hole-81", "reason": "nothing_on_axis",
                      "axis": { "point_mm": [0.0, 0.0, 0.0], "direction": [0.0, 0.0, 1.0] }, "diameter_mm": 8.0 } ] } ],
    "envelope": { "check_set": "others_and_copies" }
  },
  "digest": "sha256:…",
  "review": [ { "socket": "socket-1", "requirement": "bore:hole-41", "status": "unreviewed", "note": null },
              { "…": "one entry per requirement" } ],
  "stale_reviews": []
}
```

### 9.4 Review (R18, R19)

1. `fitment parts asm.stp` lists the parts and whether the assembly passes the gate.
2. `fitment socket asm.stp --part BRACKET -o bracket.socket.json` writes the document; stderr gives a one-line summary and the warnings of section 12.
3. The user reviews, by hand in `review` or with `fitment review bracket.socket.json --remove grip:plane-150+plane-180 --note "…"`.
4. M4: `fitment match bracket.socket.json candidate.stp` verifies every requirement not removed, and lists every removed one as "not checked: removed in review", with its note (Soundness rule 5).

- **`removed`** takes a requirement out of matching. It is the only loosening, and it needs a non-empty note of at most 2,000 characters. Removing every requirement leaves M4 with none, which rejects.
- **`kept`** marks it reviewed. **`unreviewed`** is the default and is matched like `kept`; `match --require-reviewed` (M4) can refuse a socket with any.
- **Not in M3:** changing a value, adding a requirement, or removing a requirement for one occurrence of a merged socket.

`from_json` refuses a file when:

1. the envelope is wrong (format, versions, unknown or missing keys, status and refusals that disagree);
2. `digest` is not the SHA-256 of the digested fields as fitment serializes them, which catches a changed value, version or constant and an added or deleted requirement; the message says to record changes under `review` or extract again;
3. a review entry names a socket or requirement that does not exist, repeats one, is `removed` without a note, or has a note too long (a missing entry reads as `unreviewed`);
4. a structural rule is broken, checked even when the digest matches, because anyone can recompute a digest:
   - lists are sorted and unique, and each id is the one its requirement gives;
   - each occurrence is in exactly one socket; every source's occurrence belongs to its socket and every neighbour it names is listed, and every listed neighbour is cited;
   - transforms pass `Rigid::new`, directions are unit, diameters and lengths are positive and finite, spans are ordered and disjoint, a clear diameter of a clearance span is at most the original, a grip names two distinct seats;
   - merged values are the strictest of their sources, `floor_mm` the largest of its sources' floors, `fit` agrees with its sources' margins, an `interference` or `line_to_line` fit carries `thread_not_confirmed` unless its source is flagged `conflict`, and a span with a `self` source is pinned;
   - every outline passes M2's outline checks;
   - the anchor partition (A3) holds in every socket.

### 9.5 Re-extraction

`fitment socket asm.stp --part X -o FILE` onto an existing FILE:

- **FILE is not a socket document, or is one for another `source.sha256` or another target:** exit 2, FILE untouched, unless `--force`. Ids from another file mean nothing here.
- **Same source and target:** each old decision is carried to a new requirement only when every occurrence of the new socket had that decision, the decisions agree, and the requirement's `requires`, `floor_mm` and `flags` are byte-equal (sources may differ). Otherwise the requirement is `unreviewed` and the old decision goes to `stale_reviews` with its reason (`requirement_changed`, `requirement_gone`, `occurrences_disagree`). A removal is therefore never applied to an occurrence or a value the user did not see.
- **A refused result never overwrites FILE:** exit 3, and the document goes to stdout.
- **Stale documents:** M4 refuses a socket whose `is_current()` is false, with a message to extract it again; decisions carry over. Rule fixes usually add requirements, so matching against a stale socket risks a false positive.

## 10. `fitment.parts` v1

```json
{
  "format": "fitment.parts", "format_version": 1, "extractor_version": 6, "socket_rules_version": 1,
  "source": { "sha256": "…", "bytes": 73325, "header": { "…": "…" } },
  "status": "accepted", "refusals": [], "notes": [],
  "assembly_gate": { "status": "accepted", "refusals": [] },
  "root": { "instance": 127, "product": { "instance": 163, "id": "as1-ug", "name": "as1-ug" } },
  "parts": [
    { "definition": { "instance": 122, "product": { "instance": 158, "id": "l_bracket", "name": "l_bracket" } },
      "kind": "part", "quantity": 2,
      "occurrences": [ { "path": [33, 28], "label": ["1011", "1006"] }, { "path": [34, 28], "label": ["1012", "1006"] } ],
      "gate": { "status": "accepted", "refusals": [] }, "holes": 4, "shafts": 0, "complete": true },
    { "…": "plate, bolt, nut, rod; then the sub-assemblies with kind: assembly" }
  ]
}
```

`status` says whether a parts list could be read (stage 1, checks 32 to 34); `assembly_gate` whether `socket` would pass checks 31 to 38.

## 11. API and modules

```rust
// Reading (A)
pub fn read_assembly(step: &[u8]) -> AssemblyRead;       // never panics
pub struct AssemblyRead { header, refusals, root, components, assemblies, occurrences, notes }
impl AssemblyRead { pub fn assembly(&self) -> Option<Assembly<'_>>; }   // only when accepted
pub struct Component { pub definition: DefinitionRef, pub quantity: u32, pub read: ComponentRead }
pub enum ComponentRead { Accepted(Box<Part>), Refused(Vec<Refusal>) }
pub struct PlacedOccurrence { pub path: OccurrencePath, pub component: usize, pub representation: u64,
                              pub transform: Rigid, pub part_uncertainty_mm: f64,
                              pub placement_uncertainty_mm: f64, pub vertex_offset_mm: f64 }
pub struct OccurrencePath(Vec<UsageRef>);                // Ord by usage #ids (O4)
pub struct UsageRef { pub instance: u64, pub id: Option<String> }
impl Assembly<'_> {
    pub fn select(&self, part: Option<&str>, occurrence: Option<&str>) -> Result<Selection, SelectError>;
}
pub enum SelectError { PartNotFound { suggestions }, PartAmbiguous { candidates }, PartIsAssembly { assembly },
                       OccurrenceNotFound { occurrences }, OccurrenceMismatch { part, occurrence } }
impl Rigid { pub fn apply_dir, apply_axis, apply_plane, inverse, compose }

// Documents (C)
impl PartsDocument  { pub fn read(step: &[u8]) -> Self; to_json; from_json }
impl SocketDocument {
    /// A selection error is Err (usage); a refused assembly is Ok, with status refused.
    pub fn extract(step: &[u8], part: Option<&str>, occurrence: Option<&str>) -> Result<Self, SelectError>;
    pub fn carry_review_from(&mut self, previous: &Self) -> Result<CarryReport, CarryError>;
    pub fn set_review(&mut self, socket: Option<SocketId>, requirement: &RequirementId,
                      status: ReviewStatus, note: Option<String>) -> Result<usize, ReviewError>;
    pub fn to_json(&self) -> String;
    pub fn from_json(text: &str) -> Result<Self, SocketFormatError>;
    pub fn is_current(&self) -> bool;
    pub fn active(&self, socket: SocketId) -> impl Iterator<Item = &Requirement>;      // what M4 verifies
    pub fn removed(&self, socket: SocketId) -> impl Iterator<Item = (&Requirement, &str)>;
}
pub fn sha256(bytes: &[u8]) -> [u8; 32];                 // in-crate (R21); reused by the catalog (M5)
```

Public structs and enums are `#[non_exhaustive]` with read-only fields, as in M2; documents come only from `extract`, `read` or `from_json`.

```text
crates/fitment-core/src/
  sha256.rs              FIPS 180-4, NIST vectors
  gate/assembly.rs       checks 31 to 38                               [names stepq]
  assembly/
    mod.rs               AssemblyRead, Assembly, OccurrencePath, select
    step.rs              stepq adapter: structure, placements, leaves  [names stepq]
    parts.rs             PartsDocument
  socket/
    mod.rs               public types (section 8.4)
    place.rs             N rules: moving neighbours, the search trees, check 39
    axial.rs             X, B, I, J, H
    planar.rs            C, L, Q
    unknown.rs           U, A
    combine.rs           Y: spans, strictest values, provenance
    merge.rs             occurrences into sockets
    id.rs                requirement and socket ids
    document.rs          SocketDocument, digest, to_json
    readback.rs          from_json's checks
    review.rs            set_review, carry-over
crates/fitment/src/cmd/
  text.rs                printable() and number formatting, moved out of features.rs
  parts.rs  socket.rs  review.rs
```

`features::step::part` splits into stages 1 and 2 (unchanged for `read_part`) and `part_of_definition`, which returns a definition's `Part` and the representations its solid is listed from. The probe's version gives byte-identical JSON to `read_part` on all 24 accepted part fixtures, other items aside (it does not collect them). Holding every component's `Part` in memory is cheap next to stepq's graph.

## 12. The CLI

```text
fitment parts <FILE> [--format table|json]
fitment socket <FILE> [--part <ID>] [--occurrence <PATH>] [--format json|table] [-o <OUT>] [--force]
fitment review <FILE> [--format table|json]
fitment review <FILE> (--keep <ID> | --remove <ID> | --unreview <ID>) [--note <TEXT>] [--socket <S>]
```

- **`parts`** prints a table by default (people pick a part from it): product id, name, kind, quantity, definition `#id`, gate result, holes and shafts, and a last line with the assembly gate. `--format json` gives `fitment.parts`.
- **`socket`** needs `--part`, `--occurrence` or both. Without `-o`, stdout gets `--format` (default `json`). With `-o OUT`, the document goes to OUT atomically (a temporary file, then a rename), carrying decisions over (9.5).
- **`review`** without a decision checks FILE and prints the review view. With one decision (`--remove` needs `--note`), it applies it to every socket holding the id, or only to `--socket S`, and rewrites FILE atomically. Without `--socket`, an id whose requirement differs between sockets is a usage error that lists the sockets: removing s07b's clearance bore must not silently remove the other socket's pinned one.
- **stderr** carries one-line human messages, each escaped with `printable()`: refusals, the summary (`fitment: asm.stp: BRACKET: 1 occurrence, 1 socket, 7 requirements (7 unreviewed), 1 left out`), carry-over counts, and warnings: one per unverifiable requirement (8.1, Y4: every candidate fails it until it is removed in review), one per socket whose floor is above 0.01 mm, and one per socket with no requirement (M4 rejects every candidate for it).
- **The review view** (`--format table`, not a stable format): source and target lines, then per socket one line per requirement with its decision (`-`, `kept`, `removed`), id, values, the neighbours that gave it, and the left-out features. Coordinates are in the part frame, as `fitment features` prints them; a floor above 0.01 mm is marked `coarse`.

| Code | `parts` | `socket` | `review` |
| --- | --- | --- | --- |
| 0 | A parts list was read (the gate result is in it) | Socket extracted and written | FILE valid, and updated |
| 1 | FILE unreadable, output unwritable, `internal` | The same | The same |
| 2 | Usage | Usage; a selection error; OUT holds something else (without `--force`) | Usage; an unknown id; a removal without a note |
| 3 | Stage 1 or checks 32 to 34 refused | The assembly gate refused; the document is printed | `from_json` refused FILE |

A broken pipe exits 0 quietly, as in `features`.

## 13. Test assemblies with known answers

### 13.1 Generator and manifest

- **Same generator, same manifest.** `tools/gen-corpus.py` gains an assembly section. Cases carry `"kind": "assembly"` (parts default to `"part"`), so M2's runners skip them, and one `RULES` registry covers M2 and M3. The lock stays (cadquery 2.8.0).
- **Parts from explicit primitives,** as in M2: boxes, cylinders, hexagonal prisms (`BRepPrimAPI_MakePrism`), cuts with at least 1 mm overshoot. Each part is built once and added several times, so it is one product named after its first `add`.
- **`fix()` per file:** a fixed `FILE_NAME` stamp, and usage ids renumbered from 1 in file order. Open CASCADE numbers usage ids with a counter that runs across the whole process: s08 written alone differed in 21 lines before the fix.
- **A sub-assembly used twice as one product** (as in AS1) is written through XCAF directly (`AddComponent` twice with one label); `cq.Assembly` writes two products sharing their leaves, which is s08's case.
- **Text edits** use M2's `Part21` class, each documented in its case: a mapped item, renamed products, a removed CDSR, a moved component-side item, a CTO-3D target, a flipped `same_sense`, a quantified usage.
- **Answers come from the construction only:** pure-Python 4×4 transforms and closed forms, rounded to 9 decimals.
- **Expected sockets are in the target's frame,** keyed by the `--part` value: occurrences by transform, sockets by their set of occurrence transforms, requirements by kind and axis line or plane, then every field. Expected selection errors are listed as such.

Measured on the six built so far: 25.6 to 71.2 KB each without pcurves (×2.25 with), byte-identical between runs, placements equal to the construction within 1e-10 mm.

### 13.2 Parts library (mm)

| Part | Construction |
| --- | --- |
| PLATE(lx, ly, t, d, pts) | box centred in x and y, top face at z = 0; through holes Ø d at pts |
| BRACKET | 80 × 50 × 8, bottom at z = 0; Ø6.6 at (±30, ±15); Ø8 at (0, 0) |
| BRACKET_ROW | 120 × 30 × 8, bottom at z = 0; Ø6.6 at x = −48, −24, 0, 24, 48 on y = 0 |
| HEX_BOLT M6 | hex head, across flats 10, z 0..4 (bearing face at z = 0); shank Ø6, z −L..0 |
| HEX_NUT | across flats 10, z −5..0 (bearing face at z = 0); hole Ø5, so it interferes with the shank |
| WASHER | Ø12 × 1.6, hole Ø6.4 |
| SHCS | head Ø10, z 0..6; shank Ø6, z −L..0 |
| DOWEL | Ø6 × 16 |
| BEARING_608 | ring OD 22, ID 8, width 7 |
| HOUSING | 40 × 40 × 20; Ø22 seat 7 deep from the top over an Ø18 through bore |
| BLOCK | 40 × 40 × 15; Ø11 × 6.5 counterbore over Ø6.6 through (M2's p05) |
| BLOCK10 | 40 × 40 × 10; one Ø10 through hole on its axis |

### 13.3 Positive cases

Cases marked ✓ were built as described and their facts confirmed by the oracle prototype; the others are drafts. "Unit" cases are committed. Stations follow M2 9.4: a blind hole's run from its bottom.

| Case | Construction | Target: expected |
| --- | --- | --- |
| s01_bolted ✓ unit | PLATE 100 × 60 × 10 with 4 × Ø6.6; BRACKET on it; 4 HEX_BOLT (L 25) with 4 HEX_NUT under the plate | BRACKET: 4 bores [0, 8] Ø6.6, clear 6, clearance, open at the bottom (B1, J1); seat at z = 0 (the plate) and z = 8 (one contact, by 4 heads); grip 8 through the 5 holes (the Ø8 opens onto both seats too); Ø8 left out `nothing_on_axis`. NUT: bore pinned Ø5, `interference`, `thread_not_confirmed`; seat on the plate. PLATE: 4 bores clear 6; 2 seats; grip 10. BOLT: shaft pinned in the nut over its 5 mm, at most 6.6 in bracket and plate; head seat |
| s02_cbore ✓ unit | BASE 60 × 60 × 10 with an Ø5 blind hole 9 deep; BLOCK on it; SHCS (L 16) | BLOCK: bore spans [0, 8.5] clear 6 in Ø6.6, [8.5, 14.5] clear 10 in Ø11, nothing over [14.5, 15]; seat on the counterbore floor (head: C1 and H1 agree); seat on BASE; grip 8.5. BASE: bore pinned Ø5 over [1.5, 9] (a full-diameter depth of 7.5 from the rim), `interference` |
| s03_dowels unit | BASE with four Ø6 blind holes; PLATE 80 × 40 × 10 with holes Ø5.97, Ø6.2, Ø6 and Ø6.015; DOWEL Ø6 × 16 from z −8 to 8 in each (the three-hole version with Ø5.99 was built ✓) | PLATE: four bores over [0, 8] of 10: `interference` pinned 5.97 (m = −0.015); clearance [6, 6.2]; `line_to_line` pinned 6; and `line_to_line` pinned 6.015 (m = 0.0075 ≤ τ, Z4); seat on BASE; no grip (nothing on the top). DOWEL: shaft pinned in each |
| s04_bearing | HOUSING; BEARING_608 in the Ø22 seat (z 13..20); shaft Ø8 from z −10 to 30 | HOUSING: bore spans [13, 20] pinned 22 (line to line), [0, 13] clear 8 in Ø18; seat on the seat floor (the ring's end face). BEARING: shaft pinned 22; bore pinned 8 |
| s05_stack | plates T (8), M (6), B (10) on one HEX_BOLT (L 35) with a nut; two WASHERs under the head | M: bore clear 6; two seats; grip 6. T: seats (washer, M); grip 8. WASHER: bore clear 6 in Ø6.4; two seats, one against the other washer, flagged `self`; grip 1.6 |
| s06_open_holes | BRACKET_ROW on a PLATE (top at z = 0) that ends at x = −36, with a Ø6.6 hole under x = 0 and a 20 mm square window under x = 48; a BLOCK10-like bar (top at z = −30) with Ø6.6 holes under x = 24 and 48; no bolts | x = −48 (beyond the plate) and x = −24 (over plain plate): left out `nothing_on_axis`; x = 0: bore pinned `not_read` (J1); x = 24: left out `blocked_gap` (J3, the plate lies between); x = 48: bore pinned `not_read`, `gap_mm` 30 (J2 through the window) |
| s07_twice ✓ | PLATE 220 × 60 holds two BRACKETs, the second turned 180° about z and moved 100 along x, each with 4 bolts and nuts | BRACKET: 2 occurrences, 1 socket, s01's requirements |
| s07b | s07 with one bolt and its nut removed from the second bracket | BRACKET: 2 sockets; the second has 3 clearance bores and 1 bore pinned `not_read` (the plate hole continues it, J1) |
| s08_nested ✓ | LB_ASSEM (BRACKET, 4 bolts, 4 nuts) twice under the root, through `cq.Assembly` (two products sharing their leaves) | BRACKET: 2 occurrences, 1 socket |
| s08x unit | s08 written through XCAF: one LB_ASSEM product used twice (not yet built) | BRACKET: 2 occurrences, 1 socket. `--part LB_ASSEM`: r22 |
| s09_tilted ✓ | s01 with BRACKET modelled in the frame TILT (M2's) and placed by TILT⁻¹, the whole under VARIANT | s01's socket moved by TILT |
| s10_mapped | s02 with the screw placed by a mapped item (text edit) | s02's socket |
| s11_unread_neighbour | s01 with the bolts converted to NURBS (as M2's p26) | BRACKET: `unresolved:part-<bolt>`; all five holes are bores pinned `not_read`, flagged `occupant_unread` (B8: the bolts' faces have no sound box); the four also continued by the plate (J1); nothing left out; no head seat (the heads are unread) |
| s12_undecided_target | a pin through an angled hole (M2's p25) in the target | bore flagged `target_undecided`, `extent_unknown`: unverifiable |
| s13_unknown_outline | the target's seating face has a B-spline edge | seat whose contact has `basis: unknown`, `unknown_outline`: unverifiable |
| s14_clash | a neighbour box overlaps the target with no feature relation | no requirement (M7 catches the clash; C6 leaves faces sunk into each other to it) |
| s15_mirror_part | the bracket and its mirror image as two products | each its own socket, mirrored values |
| s16_offaxis | an Ø6 bolt 0.2 mm off an Ø6.6 hole; an Ø6 pin tilted 0.5° in an Ø7 hole | bore clear 6.4 `offset` (B1); the tilted pin's clear diameter 2(e_max + 3 / cos θ) (X4) |
| s17_slot | an Ø6 bolt at one end of a 6.6-wide slot | `partial_bore` clear 6 in 6.6; the other slot end not required |
| s18_csink | a 90° countersunk screw, flush, with a nut | `conical_seat` apex and 90°; bore clear 6 in 6.6; grip from the apex (Q2) |
| s19_boss | an Ø12 boss on a cover in an Ø12.1 blind bore of a housing, 3 mm short of the floor | cover: shaft at most 12.1 over [0, 5], `max_extra_mm` 3; housing: bore clear 12 in 12.1 over [3, 8] |
| s20_near | two blocks on a plate's top face, one 0.05 mm above it and one 0.05 mm into it | plate: one seat, one contact by both blocks, flagged `near_gap` (+0.05) and `interference` (−0.05) |
| s21_self_rod | two BLOCK10 150 mm apart on one hole axis, the second turned 180° about an axis across the hole, nothing between | each: bore pinned `not_read`, `gap_mm` 140, flagged `self`, open on the facing side; 2 occurrences, 1 socket (once PR 5 merges) |
| s22_mirror_intent | a bracket added through a mirror `gp_Trsf` in cadquery, which Open CASCADE writes as a proper rotation (P8) | the socket from the file's frames, not the cadquery intent |
| s23_deep | a joint nested 12 levels deep, each level turned 30° about a skew axis (A's `a3_deep`) | placements within 1e-9 mm of the construction (P9); s01's socket |
| s24_coincident | s01 with one bolt added twice at the same place | note `coincident_occurrences`; BRACKET's socket unchanged |
| s25_usage_ids | s01 with two sibling usages given the same id (text edit) | paths written `#<instance>` (O2); the socket otherwise s01's |

Variants, in the corpus only:

- `_rot`: every positive case under the skew root motion VARIANT. The socket must equal the base's within 1e-9; each transform is VARIANT ∘ base.
- `_tgt`: the target modelled in TILT and placed by TILT⁻¹. Answers are moved by TILT.
- `_pc`: written with pcurves. The socket must equal the base's.
- `_perm`: components added in reverse order. The socket must equal the base's once matched one to one (feature `#id`s move, so requirement order may too).

### 13.4 Refusal and selection cases

| Case | Construction | Expected |
| --- | --- | --- |
| r20_part_not_found | s01, `--part NOPE` | selection error `part_not_found`, with no suggestion (no id or name contains "NOPE"); exit 2 |
| r21_name_ambiguous | s07 with the two brackets as two products, ids `BRACKET-A` and `BRACKET-B`, both named `BRACKET`; `--part BRACKET` | `part_ambiguous` by name (T4), both listed |
| r22_subassembly | s08x, `--part LB_ASSEM` | `part_is_assembly` |
| r23_component_refused | s02 with one `same_sense` on the screw flipped | `component_refused`, carrying `face_orientation` |
| r24_unplaced | s02 without the screw's CDSR | `placement_unreadable` (`unplaced`) |
| r25_divergent | s02 with the screw's component-side item moved to (10, 0, 0) | `placement_unreadable` (`divergent`) |
| r26_mirrored | s02 with s10's mapped-item edit, whose target is a CTO-3D with axis1 = −x | `placement_unreadable` (`unsupported`) |
| r27_target_refused | s01 with one face of BRACKET flipped | `component_refused` (the target is a component like any other) |
| r28_quantified | s02 with a usage rewritten as a quantified usage of 4 | `placement_unreadable` (`quantified`) |
| r29_solid_frame | s02 with the screw's solid in a second representation the usage does not place | `solid_frame_unplaced` |
| r30_two_roots | two free shapes | `several_roots` |
| r31_cycle | none (cq cannot write it) | a hand-written snippet in the Rust tests: `placement_unreadable` (`Cycle`) |
| r32_id_ambiguous | two different parts both written with `PRODUCT.id` `BOLT`, in two sub-assemblies (A's `a5`); `--part BOLT` | `part_ambiguous` by id (T3) |
| r33_occurrence | s01, `--part BRACKET --occurrence <a bolt's path>` | `occurrence_mismatch`, listing BRACKET's occurrences |

More checks reach only through snippets in the Rust tests: an empty component (`component_refused` with `no_solid`), an assembly-level surface model (35), a diamond past 100,000 occurrences (34), a translation of 1e17 mm at the root (38), a part file (31), and a hostile pair count (39).

### 13.5 Unit set

`crates/fitment-core/tests/assemblies/` holds s01, s02, s03 and s08x and their manifest, estimated at 195 KB without pcurves (s08x is not built yet, and s03 gains a hole), under their own cap beside M2's 300 KB (whose set is at 281,072 bytes). The cap is 250 KB: PR 1 measures the built set and lowers the cap to fit it with 10% headroom. Refusals r23 to r29 are applied to the committed s01 and s02 bytes by exact string edits inside the Rust tests. The guards are M2's: the cap, the listing against the manifest, `--unit-set --check` within 1e-9 over both directories, and no STEP file in `cargo package --list`. Goldens: `golden/socket/<case>--<part>.json` and `golden/parts/<case>.json`, blessed only when `EXTRACTOR_VERSION` or `SOCKET_RULES_VERSION` rises.

## 14. Oracle, fixtures and CI

### 14.1 The oracle: `verify-occt.py`

The same uv script, pinned to `cadquery-ocp-novtk==8.0.1.1.0`.

- **`manifest` mode, extended to assemblies** (generator bugs): XCAF's located leaves, composed from the root, matched one to one with the manifest's occurrence transforms (by transform and volume, never by name: a leaf placed through a mapped item comes back named `SOLID`); and for each expected requirement, the facts it rests on, measured from the solids:
  - a hole wall's extent, by classifying points just inside and outside *its own* radius (otherwise an Ø11 counterbore "extends" down the Ø6.6 bore);
  - a neighbour shaft's extent, from its own cylinder, not its solid (otherwise a head in a hole looks like an oversized shaft: the spike's first finding);
  - the fit, labelled with fitment's own τ: the radial gap g between the hole wall and the shaft (negative where their common volume is positive, by its depth) gives clearance for g > τ, line to line for |g| ≤ τ and interference for g < −τ;
  - adjacency along the axis; contacts as face pairs within max(`NEAR_MM`, τ) with opposed normals and a positive common area of their projections; grips as separations;
  - every left-out hole: no neighbour solid holds a point of its axis over the wall's span (`BRepClass3d_SolidClassifier`).
- **`socket` mode** (fitment against Open CASCADE, on the corpus and fixtures):
  - occurrence transforms against XCAF's, one to one;
  - each requirement fitment states, checked as above at its stated axis or plane; a fact Open CASCADE does not see is a disagreement;
  - **the other direction, for missed requirements:** every target hole wall whose axis passes through any neighbour solid within the wall's span must carry a requirement; every target plane face within max(`NEAR_MM`, τ) of an opposed neighbour face with a positive common area must be in a seat; every other target face within τ of a neighbour face must be in some requirement (C6); every positive common volume must be explained by an `interference` fit or seat, or is listed as a clash note. Open CASCADE classifies B-spline solids like any other, so this sees an unread neighbour (s11, AS1 think3);
  - each `extent` contains the solid's bounding box;
  - `--expect` pins the fixtures' tallies, as in M2.
- **Cost:** the prototype took 218 s on the weldment's `SPACER_220` (108 occurrences). The real oracle prefilters by bounding box (grown 1 mm), groups split cylinder faces as M2's `groups()` does, and bisects spans from stated circle stations instead of sampling. Target: generated cases under 1 s each; weldment measured in PR 3.

What it cannot check: labels and policies (D6, `thread_not_confirmed`, review flags), selection errors, merging beyond comparing requirement sets, and what both readers get wrong alike. XCAF applies a mapping target twice when it is also a plain item of the representation, where stepq does not: the generator avoids that layout, and s10 is checked against the construction.

### 14.2 AS1, six exports

| Export | Exporter | Leaves | Holes read | Notes |
| --- | --- | --- | --- | --- |
| `as1-ac-214` | AutoCAD 2000 | 5 accepted | yes | rod nuts touch the uprights; split cylinders |
| `as1-md-214` | MicroStation/J | 5 | yes | no product names; rod nuts 12 mm off the uprights |
| `as1-ug-214` | Unigraphics 16 | 5 | yes | rod nuts 12 mm off |
| `as1_pe` | Pro/ENGINEER, inches | 5 | yes (Ø254 = 10 in) | 12 occurrences: no nuts on the bolts |
| `as1-ec-214` | Euclid | 5, `complete: false` | no: 14 cylinders undecided | 18 of 18 placed |
| `as1-tc-214` | think3 | 5 | no: 28 cylindrical faces are B-spline `other` | mapped items; no assembly uncertainty |

Expected sockets, pinned per export (the exports are not one geometry):

- **l_bracket:** 2 occurrences, 1 socket. 3 bolt holes and the rod hole: bores pinned Ø10 (`line_to_line`, `thread_not_confirmed`), the bolt holes continued by the plate; the rod hole is pinned by the rod, and the rod nut's hole beyond it (touching in ac, 12 mm off in md and ug) adds provenance only, as the rod spans the joint. Seats: the base's top (3 heads, ring Ø10 to Ø15, 98.17 mm²), the base's bottom on the plate; in ac and pe also the upright's outer face, where the rod nut bears. Grip 10. Nothing left out.
- **plate:** 6 bores pinned Ø10, continued by the brackets; seats against both brackets and (md, ug, ac) the bolt nuts; grip 20 where nuts bear (not pe).
- **bolt, nut, rod:** shafts and bores pinned Ø10 where they engage; head and nut seats.
- **ec:** the same bores, from undecided cylinders, flagged `target_undecided` and `extent_unknown`; unknown outlines give extra seats.
- **tc:** no hole or shaft can be read, so there is nothing to leave out. Each part's `other` faces are `unresolved` (U5), every part with unread faces is `unresolved:part-<n>` (U4), and seats come from the planes. The CLI warns.

A cross-export test compares the shared core of ac, md, ug and pe (÷ 25.4): the multiset of (kind, fit, diameter, span length, grip) and the pairwise distances between requirement axes (the bolt holes form an equilateral triangle of side 15√3 = 25.98 mm), with the known differences (the rod nut's seat, pe's missing nuts) in a named allow-list.

### 14.3 Other assembly fixtures

Pinned per file, as regression and not as truth: the `parts` listing (parts, kinds, quantities) and the assembly gate result for the 9 core-set assemblies in CI (the 10 OCP files are not fetched in CI, and are pinned through a local `--expect` run only); and for chosen targets, occurrences, sockets after merging, requirement counts by kind and fit, left-out count and status. Targets: all 5 AS1 leaves × 6 exports; weldment `SPACER_220` (6 occurrences, 5 distinct contexts: the merging stress case), `BEARING_208` (23), `NUT_244` (18) and `LINKAGE_ARM_SHORT_46`; moon buggy's mapped-item part; vaccase `TEE` (undecided cylinders). They are re-pinned only in a PR that says why.

Measured with the probe rules (weldment, 332 target hole features): 267 related by contact, 39 only beyond a gap (17 of them blocked), 26 with nothing on their axis.

### 14.4 CI

- **`corpus` job:** `gen-corpus.py corpus/out` also writes the assemblies and their variants (about 6 MB, 4 s more); `verify-occt.py manifest`, then `verify-occt.py socket corpus/out` and `verify-occt.py socket --expect fixtures/manifest.json fixtures`; then the corpus, fixtures and new `sockets` tests with `FITMENT_REQUIRE_CORPUS=1` and `FITMENT_REQUIRE_FIXTURES=1`. Estimate: under 3 minutes more, once the oracle prefilters.
- **`unit-set` job:** regenerates both committed directories and checks them.
- **Three-OS `test` job:** unchanged; the corpus artifact carries the assemblies, so every socket case runs on Linux, macOS and Windows without Python.
- **Determinism:** each case runs twice with byte-identical output, `parts` included.
- **Read-back:** every socket and parts document fitment writes for the unit set, the corpus and the fixtures passes `from_json`.
- **Rule registry:** `RULES` gains this document's rule ids (T, O, P, Z, N, X, B, I, J, H, Q, C, L, U, Y, A, V; none collides with M2's G, S, F, W, K, E) and checks 31 to 39, keyed `m3.4.1#31` to `m3.4.1#39`. `corpus.rs`'s coverage test, which today reads only `m2-features.md` and keys checks 1 to 30 as `4.2#n`, gains this document and those keys (PR 1). It keeps M2's must-refuse rule: every rule that yields a refusal or an unverifiable requirement needs a case that shows it (B8 and U4 need s11).

## 15. PR plan

Each PR comes with its tests and two adversarial reviews. `EXTRACTOR_VERSION` rises with PR 2; `SOCKET_RULES_VERSION` starts at 1 in PR 3 and rises with every later change to socket output. Until PR 5 merges occurrences, PR 3 and PR 4 write one socket per occurrence, and their tests compare per occurrence.

1. **Reading an assembly, `parts`, choosing the part.**
   - `sha256.rs`; `assembly/` with checks 31 to 35, placements, occurrences, the O and T rules and `Assembly::select`; `PartsDocument`; `fitment parts`. `fitment socket` arrives in PR 3; selection is tested through the API;
   - the generator's assembly section (`kind: "assembly"`, `fix()`, the mapped-item edit), s01, s02, s03, s08x committed with their cap; s23, s25; r20 to r22, r24 to r26, r28, r30, r32, r33, and the r31 snippet;
   - the coverage test reads this document (14.4); `verify-occt.py manifest` for placements.
   - As built: every assembly is written through XCAF directly from a tree of parts and sub-assemblies (one Python object, one product), not through `cq.Assembly`, so product names and shared sub-assemblies are exactly the construction's; expected transforms are multiplied from the same tree. Open CASCADE's XCAF writer resets the STEP statics when first constructed, so pcurves are switched off after it. The committed set is 187,691 bytes and its cap 210,000. r20, r22 and r33 are selection expectations of s01 and s08x rather than files of their own; s07 and s08 are written already, r21 needing s07's layout. Open CASCADE shares one identity frame among every usage, so r25 and r26 give the screw a frame of its own before editing it. `fitment.parts` carries each occurrence's transform, and `path_form`.
   - Review of PR 1: an orphan cycle beside the root passed checks 31 and 32 (now refused as unreached); a usage stepq cannot read lowered a quantity silently (check 33 counts them); a 31 KB file wrote 725 MB of parts list through long usage ids on many paths (check 34 now bounds path usages and id bytes); check 34 hid check 33; `from_json` accepted trees `read` cannot give; the tests never checked which occurrence a path chose or what the parts document said; the generator's bytes depended on what ran before in the process (XCAF naming, line wrapping), now re-serialised one instance per line. The oracle matches leaves by product name and transform, so two leaves of one name are told apart only by where they are; refused placement edits are only read, as Open CASCADE may read them another way. The refusal edits r24 to r28 live in the corpus, which CI requires; r28 states its quantity with the file's length unit.
2. **Components, frames, extent.**
   - checks 36 to 38, `part_of_definition`, the Z1 values per occurrence, the notes;
   - in `features.v1`: `extent` (sound when every face is a plane, cylinder, cone or torus and every edge a line or circle: planar, cylindrical and conical faces lie in their boundary's hull, and a torus takes its whole surface's box; else null) and each cone's radii; `EXTRACTOR_VERSION` 6, goldens re-blessed;
   - s24; r23, r27, r29; the core assemblies' `parts` and gate pins; the oracle's extent check.
   - As built: `features::step::part_of_definition` reads any definition through stages 3 to 6, returning also the representations its solid is listed from (check 37) and its reach, the farthest any stated vertex, curve, surface or cone apex lies from its origin; check 38 refuses an occurrence whose translation plus that reach leaves the range. The extent is grown by the solid's largest vertex offset; spheres count as unbounded, as M2 reads them as `other`. Cone radii come from the faces' vertices and coaxial circles: 5 of 242 cones over the fixtures and corpus have none (a drill point pierced by a cross bore). 101 of 152 accepted parts have an extent; the oracle checks each against Open CASCADE's box (92 corpus, 9 fixture). A placement's floor is the largest uncertainty of the root's and each enclosing assembly's context; think3's AS1 states none and gets the note. 10 of the 19 fixture assemblies pass checks 31 to 38, as designer A measured; the 9 core ones are pinned in `fixtures/manifest.json`.
   - Review of PR 2: a cone face bounded by one circle holds its apex with no vertex there, so the extent missed a pointed tip and the radii missed 0 (a face holds its apex when an odd number of its loops wind about the axis); a translation of 1e17 mm overflowed the coincidence grid, and occurrences turned apart at one place made it quadratic (now skipped out of range, and capped at 64 comparisons a cell); a solid several definitions share was read once per definition (now refused unread, `solid_shared`); `from_json` accepted floors of `-0.0`, floors unlike their definition's, and refused components under an accepted gate; check 38 bounds by √2 times the reach, as a vertex's foot on an axis can lie that far; tests now tell the floor of the whole path from the parent's alone, check the vertex offset, check 38's reach, rotation in the coincidence check and which component is refused.
3. **Axial rules, the search and the socket document.**
   - N (with check 39), X, B, I, J, U, A and Y for axial features; the socket types, ids, `to_json` (digest included) and a first `from_json`; `fitment socket` with its exit codes; `SOCKET_RULES_VERSION` 1;
   - s06, s11, s12, s16, s19, s21, s22; the oracle's `socket` mode for holes and shafts in both directions, with the prefilter; the weldment timed.
   - As built: one socket per occurrence until PR 5 merges them; `socket/` holds the elements and pair geometry (`axial.rs`), the rules (`rules.rs`), J3's exact point-in-outline (`block.rs`), the combining (`combine.rs`) and the document. A neighbour's candidates are examined directly, every pair counted toward check 39; the bounded rules skip a neighbour whose extent stays out of reach. A wider cylinder that only touches a rim is no occupant (a bolt head is H1's, PR 4); B8 pins the whole segment; an undecided target with no extent is unbounded, its spans those of what occupies it. Each span states `pinned`, and each left-out feature its wall's stations. The oracle's `socket` mode checks every left-out hole's axis for material in every other solid, every clearance span's gap for air and every interference span's surface for material; with every relation dropped it finds 41 disagreements in 13 of 17 cases. It does not yet probe J's requirements or the openings a span asks for; PR 7 adds them. s22 moves to PR 5 with the frames. The weldment's 23 sockets of its most frequent part take 0.06 s.
   - Review (two adversarial reviews, all findings taken):
     - **Zones (X7).** Transitions and end chamfers lay in no span: a pin crossing a cone step, a cone under a head, a chamfered tip short of a blind floor all left stations free. Segments now stand for their zones (s40 to s42). Interference inside a zone is left to the seat rules, and zone pieces are never pinned.
     - **J.** A shaft flush with a rim no longer cancels J1's pin, nor one that stops short of the far hole: it must run past the rim into the neighbour hole (s44, s46). A blocked nearest hole no longer blocks a wider one beyond it (s43).
     - **B8** requires an end open when an unread face runs past the rim (s45), pins the cylinder's own stations and looks for occupants over the zones too.
     - **Unknown ends.** A target stack's side with no known end runs to the part's extent, as a neighbour's does (U3), and an undecided target shaft is flagged `target_undecided` (U2) and unverifiable. Past a known end a span is `not_read`: s18's countersunk head had read as the shank's dia 6 running on, asking only clear 6 of the countersink (found while building s18).
     - **Zones' limits.** A zone is taken only where every cone and torus there stays on the safe side of the cylinder (a flared end does not).
     - **Check 39** counts each neighbour occurrence of each selected occurrence, across the whole extraction; every definition's elements are read once. 2,000 copies among 2,200 occurrences take 0.4 s (1.1 s before); 4,000 among 4,400 are refused in 0.8 s.
     - **Read-back** (9.4, in part): every occurrence is in the one socket it names, every source's occurrence belongs to its socket, and every listed neighbour is cited. `Requirement::verifiable()` states Y4's kinds.
     - **CLI.** `-o` never replaces the assembly being read nor a file that is not this part's socket of this file, unless `--force`; its temporary file is unique and removed on failure. Warnings name every unverifiable requirement, a socket with none, and a socket read only to within more than 0.01 mm.
     - **Tests.** Each expected socket states where its occurrence is, so requirements handed to another occurrence fail; bores state their depth, and a cylinder in no stack may open no end; left-out features match one to one with their walls; requirement order (N5) and the extent (V2) are checked; s11's B-spline bolt (U5), s16's pins and s12's spans are expected. The oracle takes every leaf but the target occurrence itself as the others (it had dropped every neighbour placed at the target's own transform).
4. **Planes and contacts.**
   - C (C6 included), H, L, Q, `partial_bore` and `partial_shaft`; the seat, grip and conical-seat kinds;
   - s04, s05, s13, s14, s15, s17, s18, s20; the oracle's contact and grip checks, and its check for any touching pair.
   - As built: `planar.rs` reads C1 to C6 and Q1 for each target occurrence, `overlap.rs` decides C2 (circumscribed arcs, π/8 a piece, and holes in a face by inscribed polylines, so a pin's end inside a plate's hole, or a shaft square to a face through one of its holes, touches nothing), and `combine.rs` turns contacts and H1's rings into seats, opposed seats into grips (L1, with the holes that open onto both) and a conical seat with an opposed plane seat into one from the apex (L2). H1 reads a step inside a stack as a rim (s02's counterbore floor, s04's bearing seat); the plane a hole opens onto is found by geometry, for M2 names the face the walk lands on (a washer's outside). C6 leaves out what Q1 seats and what the axial rules read; B8 leaves out a parallel cylinder that stays outside the core (s04's dia 18 bore beside the bearing's dia 8). Every plane pass counts toward check 39 with the axial rules. `SOCKET_RULES_VERSION` is 2. Cases: s04, s05, s13, s14, s17, s18's seat and grip, s20, and s47 (a rod on a flat, C6); s15 moves to PR 7 (a mirrored product reads as any other; P8's s22 is mirror intent). The oracle checks each seat's plane against the target's faces with a solid within the near band, each grip's separation, each conical seat's apex, and the other direction: every target face with an opposed face within the band sharing area is in a seat; moving every seat by 0.5 mm, dropping them or lengthening a grip by 0.01 mm is caught. Its check for any touching pair (C6's direction) and `max_extra_mm` (I4) move to PR 7 and PR 5.
   - Review (two adversarial reviews, all findings taken):
     - **C6 was narrower than designed.** It read only convex curved neighbour faces against target planes, explained a neighbour wholly once any axial rule read it, and skipped faces with no sound box: a rod on a floor (as the rod's requirement), a wedge's edge, a bar in a cradle, a 90° head in an 82° countersink and a rod both through a hole and on a floor went unrequired. It is now the design's C6, above, in `touch.rs`.
     - **B8** skipped a D-shaft as if it were a wider bore: only hole-sense cylinders are skipped now (s48).
     - **C1** missed a block tilted within the window resting on one edge of a large face; **Q1** a shallow taper seated off its apex; a conical seat with the neighbour's radii unknown was verifiable with none; **L2** dropped a grip whose apex lies past the plane. All fixed as the rules above now say.
     - **Tests.** Each seat's touchers (neighbour product, gap, flags), each partial cylinder's span order and sweep, each conical seat's axis and radii, and each socket's count of unverifiable requirements (Y4, derived from the expectation) are checked; s05's bolt and nut and s48 are expected; C2 and I5 are cited where exercised. The table shows each requirement's flags.
     - **Oracle.** A seat must have an opposed face sharing area within the band (an edge-adjacent solid no longer passes), its `near_gap` and `interference` flags must match the measured gaps' signs, a conical seat's angle must match, and every pair of opposed seats sharing area must have a grip. Mutations caught: seats moved, dropped or added on a side face, flags toggled, cones dropped or widened, grips dropped, lengthened or naming another seat. Not caught by it (the Rust checker catches most): `by`, bases, rings, partial cylinders, C6's entries, grip holes, toucher gaps.
5. **Occurrences, merging and review.** Also I4's `max_extra_mm`.
   - merging, the full `from_json` (section 9.4), `review` and carry-over, `fitment review`;
   - s07, s07b, s09, s10, the `_rot` and `_tgt` invariance tests, and the review tests (a changed value, a changed constant, a deleted requirement, a removal without a note, an unknown id, a duplicate entry, a removal whose id differs between sockets, carry-over with a changed requirement and with sockets that split).
   - As built: `merge.rs` buckets each occurrence's socket by its requirement ids, left-out features and values quantized to `MERGE_MM` (lengths) or `MERGE_SINE` (directions and angles), leaving out neighbour paths, sources and toucher identities (their counts stay); merged values are the strictest, touchers, rings, causes and sources the union, and sockets are numbered by their smallest occurrence path. `check.rs` is 9.4's structural read-back: sockets numbered in order, occurrences sorted, requirements unique and in order, flags sorted, each floor at least its sources', spans ordered and disjoint, clearance diameters on the right side of the original, tight fits labelled `thread_not_confirmed` (or `conflict`), seats with touchers and a basis that agrees with the outline (which passes M2's outline checks), grips of two seats, cones and partial cylinders in range, A3's anchor partition (partial cylinders are no anchors), and the review (entries existing and unique, removals with a note, notes at most 2,000 characters; a missing entry reads as unreviewed). Not yet checked: merged values against their sources' margins, and `self` pinning (X7's zone pieces are never pinned, so the rule needs X7's exception first). `review.rs` applies decisions (`SocketDocument::decide`, `fitment review`) and carries them over (9.5) with the reasons `requirement_changed`, `requirement_gone` and `occurrences_disagree`; `fitment socket -o` onto its own earlier document carries them and says how many. I4 records `max_extra_mm` from the shaft's free end to the floor of a blind hole holding it (s19: 3, s02's screw 1.5, s03's dowels 0). The `_rot` and `_tgt` variants are written for every case with sockets and compared in `corpus.rs` with ids, paths and sources left out and axes as lines: making them pass took exact stations for full cylinders and cones (their end circles) wherever C6 and B8 had used boxes, closed forms for B8 (a plane face by its region clipped to the hole's stations, a parallel cylinder by the side its material lies on), and joining span pieces and cuts that differ by rounding alone (`MERGE_MM`). What the part's box still bounds depends on the frame it is modelled in: U3's unknown ends, U1's partial cylinders and I7, which `_tgt` leaves out of the comparison. s07 merges into one socket and s07b splits into two; s09's ground is covered by `_tgt` and `_rot`. s10 (a component placed by a mapped item) moves to PR 6, with the exports that place by mapped items. `SOCKET_RULES_VERSION` is 3.
   - Review (two adversarial reviews, all findings taken):
     - **C6** lost a knife edge lying through two corners of a face: the crossing test that ignored rounding also ignored real crossings near vertices. Each edge is now cut where it meets the face's edges and vertices, and reaches the interior if a piece's middle lies deep inside (exact).
     - **H1** found no plane around a chamfered or countersunk rim (its loop borders the cone), and wrote a seat with no contact, which the read-back then refused: rings now find planes across the hole's transitions too, and a seat with no face is an `unresolved` contact instead.
     - **Read-back** also checks tight fits pinned, every list in the document's order and unique (sources, contacts, touchers, rings, grip holes, cause faces), a source for every kind a relation gives, `conflict` only with a conflicted source, positive left-out and ring sizes, and every neighbour a toucher or ring names listed; each rule has a test that edits a document and recomputes its digest. Still not checked: merged values against their sources' margins (so a bore's clear diameter may be lowered by hand), and `self` pinning.
     - **Review.** `decide` compares a requirement across sockets without its touchers; with `-o`, a refused result skips the other-file guard (exit 3, document printed). Tests cover decided and undecided occurrences merged, notes that disagree, a requirement gone and kept stale, an unknown socket, and another file's document.
     - **Tests.** Merged seats must list touchers for every occurrence, each as its sources say; the variant comparison keeps coordinate triples in order, compares sources and touchers without their ids, pairs each occurrence with its counterpart, reads each document back, and in `_tgt` leaves out only the stations, openings and `open_at_unknown_end` of what the box bounds (s17's partial flag and s18's opening are the frame dependences it showed). Merging is unit-tested for touchers, flags, left-out features and numbering. Not done: I4 for a tip in a non-last segment of a stepped blind hole (`null`), the oracle's `max_extra_mm` check (PR 7), and s09's own construction (only the bracket tilted, under VARIANT).
6. **Fixtures.** AS1's expected sockets per export in `fixtures/manifest.json`, the cross-export test, the pins of 14.3, `--expect` tallies.
7. **Hardening.** Also s15 (a mirrored product), H1's seat where no C1 contact coincides (a head whose end is no plane), and the oracle's check of any touching pair (C6's direction) and of partial cylinders, touchers and grip holes. `_pc` and `_perm`; an oracle mutation run (drop a requirement, move an axis by 0.01 mm, swap a fit, shrink a contact, merge two distinct sockets: every geometric mutation caught); hostile inputs (10,000 bolt occurrences, 10⁵ coincident occurrences, a hole with 1,000 coaxial neighbours, tiled coplanar faces, the diamond past 100,000 occurrences, a pair count past check 39) with n log n checked; determinism on three OSes.
8. **M3 exit.** The registry gate on for M3's ids; PLAN, README and ARCHITECTURE updated (section 16).

## 16. Changes to PLAN.md and ARCHITECTURE.md

- **Matching method, step 2:**
  - "Coaxial and overlapping along the axis" becomes "parallel, with cross-sections meeting over an axial overlap"; the clearance bound is the shaft's diameter plus twice the offset.
  - "Coaxial, not overlapping, neighbor hole: … The axis is required" gains "also beyond a gap, unless a plane face of known outline blocks the line between them; with no fastener read, the hole is pinned".
  - "A target hole with nothing on its axis is not a requirement" becomes "… is left out and listed for review; never while an unread face could be in it, and material that is not a read shaft pins the hole".
  - "The socket is written as a JSON file the user reviews and can edit" becomes "… reviews: each requirement can be kept or removed with a note; values cannot be edited".
  - Add: unread faces on the target or a neighbour, and contacts no rule reads, are `unresolved` requirements that fail until removed in review.
- **Choosing the part to replace:** D11 is confirmed as section 5; `--part` also takes `#<n>`.
- **Tolerances:** the floor of a relation includes the placement path's uncertainty (R17).
- **M7:** the fit check also tests the candidate's placed copies against each other (V1).
- **ARCHITECTURE.md:** stepq is named also in `src/assembly/step.rs` (`tools/check-core.sh` already admits `gate/assembly.rs`); "the extractor version rises with every change that could change any output" becomes "… any features output; `SOCKET_RULES_VERSION` does the same for sockets"; a section "Reading an assembly" beside "Reading a part"; the test table gains the assemblies and the socket oracle.

## 17. How the four designs were reconciled

| Disagreement | Settled as | Why |
| --- | --- | --- |
| B stated requirements in the assembly frame; C and D in the target's frame | The target's frame (R6); B's rules move neighbours by `T_k⁻¹ · T_j` (P5) | Merging becomes a value comparison, the original passes at the identity, and `_rot` answers come free |
| B wrote limits (d_min, d_max under D6 strict); C wrote facts and left D6 to M4 | Facts, with B's offset-aware clear diameter as one of them (8.4) | A policy change must not need re-extraction; the clear diameter is a geometric fact, not policy |
| Kinds: B (`bore`, `shaft`, `seat`, `conical_seat`, `grip`, `partial_bore`, `unresolved`); C (`bore`, `pin`, `axis`, `bearing`, `seat`, `grip`, `unexpressible`); D (`hole_fit`, `continuation`, `bearing`, `seating`, …) | B's kinds; a continuation is part of the bore (`open`, `not_read` spans), a head's bearing is a seat contact | B's "one requirement per target feature" is C's "one per (kind, anchors)" with fewer kinds |
| Contact region: B stored both outlines; C only the target faces; D expected the intersection's area | Target faces (R14); unverifiable when the target's outline is unknown; the oracle checks the true contact area | Sound, merges, and 3.7 times smaller. D's s03 area becomes an oracle fact |
| Other occurrences of the target: B made them neighbours (`self`); C excluded selected occurrences from each other | Neighbours, flagged `self`, with B and I relations pinned on both sides (R8, N2) | Excluding them drops requirements (stacked washers); independent limits on two copies of one candidate are unsound (soundness review) |
| A coaxial hole beyond a gap: D said not a continuation (F3); B said a requirement unless blocked (J2, J3) | B's rule (R11), nearest hole only, searched along the whole axis | Omitted fasteners are common (R2's 352 empty components); the cost is a reviewable false negative. D's s06(d) became a blocked case and (e) an open one |
| The floor: A included the placement path; B the parts only; C added the vertex offset | All three, of both sides (R17) | It is the plan's "contexts involved"; larger floors only add requirements |
| Adjacency and coaxial tolerance: D 1e-6 mm; B max(D9, floor) | B's τ (Z3), as an extraction constant `CONTACT_MM` | Real files are exact or visibly off (B: 752 pairs ≤ 1e-9 mm, then 28 between 1e-9 and 0.01); the larger tolerance errs toward more requirements |
| AS1 Euclid and think3: D refused or flagged; B flagged | Flagged (R23) | Refusing hides what was read; flagged requirements already fail until reviewed |
| Unread neighbours: B one `unresolved` per neighbour occurrence; C an `unknowns` list that blocks nothing | One `unresolved` per neighbour definition, the same in every socket (U4), and B8 pins the holes its faces may reach | Per occurrence was quadratic and broke merging; an unknown that blocks nothing is a false positive waiting |
| Left-out and empty sockets | A partition over holes, shafts and undecided cylinders (A3), checked on read | Makes the exit criterion a property of every file |
| Paths: A wrote `{instance, id}` pairs; C an array of `#id`s plus labels | C's form, with A's rules for printing and parsing (O2, O3) | Compact; both forms print back |
| Suggestions for an unmatched `--part`: A up to 10, C up to 20 | Up to 20, sorted by id | More help at no cost |
| `parts` and the component gate: A listed per-part results and never refused for a component; C ran the gate by default | Both: `parts` runs the gate and reports it, refusing only for the structure | The user sees what blocks `socket` before choosing |
| SHA-256: A wanted the CLI to hash (core has no hash); C in-crate | In-crate (R21) | `from_json` needs the digest in core; no new dependency |
| B proposed six additions to M2 | Two now (R22: extent, cone radii); the rest recorded (section 18) | The extent bounds unknown ends and prunes the search; the radii state conical seats |
| B's coaxial shafts that touch (`coaxial_shaft_touch`) | Kept as I6 (`abutting`) | — |
| M2's 4.5 said check 5 becomes the assembly gate and check 7 "exactly one root" | Checks 5 and 7 keep their meaning for a part file; assembly mode adds 31 and 32 | A machine name never changes meaning (M2 4.4) |
| Case names: B g01 to g12, D s01 to s15, A a1 to a7 | One list, s01 to s25 (13.3) | g02 and g04 are in s02, g09 in s06, g10 in s05; A's a3, a4, a5 and a7 are s23, s22, r32 and s24 |

The soundness review then attacked the merged design. Each finding below changed a rule:

| Finding | Failure as first written | Fix |
| --- | --- | --- |
| Coupled `self` limits | A part with an Ø6 pin and an Ø6.6 hole, stacked: the socket said bore ≥ 6 and shaft ≤ 6.6, so a candidate with a Ø6.5 pin and a Ø6.2 hole passed, clashing with itself; M7 excluded the copies | `self` B and I relations pin both sides; M7 checks copies against each other (N2, V1) |
| Unread occupants | A D-shaft, a hex bar, a key or a tapered pin in a bore is no shaft stack, so the bore was left out `nothing_on_axis` | B8 pins any hole a neighbour face may reach; `partial_shaft` and I7 for partial cylinders |
| Pruning dropped J2 | The prefilter's 1 mm reach dropped partners 150 mm away; a direction-binned index missed skew and off-axis neighbours | Search trees over all neighbours regardless of direction; J2 along the whole axis (N3) |
| Partial cylinders' extent | End-arc stations do not bound a partial cylinder with axial or B-spline edges | All boundary vertices and arcs, unbounded beside an unread edge (7.2) |
| Contacts no rule reads | A rod on a flat, a set screw on a shaft, a 90° head in an 82° countersink gave nothing | C6: `unresolved` (`contact_no_rule`); the oracle checks every touching pair |
| Concave arcs in C2 | A circumscribed polyline on an inner-corner fillet cut real face away, so a pad there read "certainly disjoint" | Chord or circumscribed polyline by the side of the arc's centre; boxes include arc extremes |
| The witness | J required an opening at a blind bottom; the neighbour-face basis asked the original to cover a larger plate; unverifiable kinds fail the original | J only at an `opening` end and only end to end; basis `unknown` instead; Y4 lists the unverifiable kinds |
| The touch band | A shaft entering a hole by less than τ (up to 0.67 mm in `as1_pe`) gave nothing | B and I apply when L > −τ |
| Unbounded work | Check 39 counted relations, not pairs examined | It counts candidate pairs; J2 nearest only; J3 by sweep |
| Review across sockets, the digest | `--remove` hit every socket holding an id; a version or `contact_mm` could be edited outside the digest | Usage error when the requirement differs; the digest covers everything but the review |
| J3 and J1 | One axis point inside a face proved a block that a thinner rod passes; any shaft in a hole made J1 provenance only | J3 needs a disc of the possible fastener's radius; J1 is provenance only when the shaft spans the joint |
| H1 and skew | H1 required e ≤ τ; a skew neighbour hole (air) became `unresolved` | e ≤ r_N − r_h; skew only for neighbour material |

The consistency review corrected counts (9 refusing files, the 3.7× size, the Creo floors), the s03 Ø5.99 hole (line to line at τ, now Ø5.97), s02's blind-hole stations, the s05 bolt length, the parts example's `#id`s, the JSON example's order and fields, constructions that could not be built (s06, s20, s21, r21), the coverage test's real scope, the fixture pins CI can fetch, the PR plan's placement of check 39, r26 and merging, and dropped cases and rules from A and B.

## 18. Risks and open questions

### 18.1 Risks

- **Strict refuses most real Creo assemblies.** 9 of the 10 OCP-set files are refused, for surface models, multi-body parts and empty fasteners. `parts` names the component that blocks; the scoped rule (4.4) is the planned relief.
- **Unread faces make sockets need review** (B8, U4, U5) until stepq reads B-spline control points (a stepq 0.7 candidate: a B-spline with positive weights lies in its control net's hull, which gives a sound extent).
- **Pinning is strong.** AS1 pins every hole; a catalog bracket with Ø10.5 clearance holes is rejected for the AS1 bracket. It is the honest reading of a line-to-line model, and review can remove it, but the false-negative rate on real assemblies may be high. D8's ground truth would show it. B8 adds pins wherever a screw's socket recess or a key sits in a hole.
- **J2 costs about 7% of the weldment's holes** (22 of 332), for mirrored parts. If review fatigue becomes the problem, a recorded policy `coaxial_gap_reach_mm` (default unbounded) can be added.
- **The floor may reject most Creo matches at M4** (section 6.2): a coverage loss, not a soundness one.
- **The target-face contact basis is strict** (R14): a candidate with a smaller flange that still covers the true contact is rejected. An exact intersection basis is later work.
- **No added or tightened requirements in M3.** A requirement the rules miss cannot be fixed in review; only better rules, the oracle's missed-requirement checks and M7 catch it.
- **Interfering originals** (weldment: 33 face pairs up to 0.84 mm into each other) give seats flagged `interference`, and M7 will report the original as colliding too; the review view says so.
- **The digest is not a security boundary.** The catalog (M6) must recompute the extraction itself, never trusting an uploaded `extracted`.
- **Exporters disagree about the same model** (AS1's rod nuts, pe's missing nuts), so expected sockets are per file.
- **Both kernels are Open CASCADE,** and the generator's writer loses mirror intent (P8) and numbers usage ids per process. Answers from the construction stay the reference.
- **Memory is stepq's,** about 10 times the file size (2.0 GB for 207 MB). The server (M6) needs a size limit.
- **In-crate SHA-256 is code to get right;** the NIST vectors and a cross-check against `shasum` in the CLI tests cover it.

### 18.2 Open questions, with defaults

1. Adopt the scoped rule (4.4)? Default: not at M3; revisit with real user assemblies.
2. Should J2 apply to a neighbour shaft that stops short of the target hole? Default: yes, an axis requirement with the shaft's diameter as the clear diameter. No fixture reaches it.
3. Thread properties: if both parts state a thread (D10), B2 and B3 could confirm it; M3 copies any property or PMI on the hole's faces into the requirement.
4. Under D6 functional, a larger hole passes the bore but cuts into the seat's required region (the target face's inner loop). M4 must shrink the region by the allowed growth. The strict default avoids it.
5. Grip without a shared axis (L1): hard (default), or only when a fastener joins the seats?
6. An empty socket (moon buggy has parts with no relation): M4 rejects it (`NoRequirements`); `socket` warns.
7. `NEAR_MM`: 1 mm covers every weldment near contact (all ≤ 0.84 mm) but is not derived from anything.
8. `MAX_OCCURRENCES` 100,000: vehicles exceed it; raise it once relations are indexed and measured.
9. `fitment features asm.stp --part X`, printing one component's document: free with this design; add it with the CLI work?
10. Partial-cylinder sectors and countersunk screw ends in `features.v1` (B's other proposals): when M4 needs them.
11. Should `socket` refuse when a floor exceeds 0.01 mm? Default: no; it warns and M4 rejects what it cannot verify.

## Evidence

Under [m3-evidence/](m3-evidence/), not committed: the four designers' reports (`A.md` to `D.md`) and their probes.

- `A/`: fitment-core with `definition_read` and a probe; the generated assemblies (`gen/`), crafted diamonds, edits e1 to e3; `all.jsonl` and the scripts behind sections 4 and 6.
- `B/`: a fitment copy with `probe_assembly` and `asmdump`; `sockprobe.py` (the rules in Python, on dumps); the censuses (`census2.py`, `c2block.py`, `merged.py`); 12 generated assemblies.
- `C/`: identity statistics and occurrences on stepq 0.6.0; `proto2.py` (rough rules, merging, sizes).
- `D/`: the prototype generator, the oracle prototype (`oracle_proto.py`) and its outputs on the generated cases, AS1 and the weldment; the mapped-item and refusal edits.
