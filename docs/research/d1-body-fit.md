# D1 decision brief: body-fit check

As of 2026-10-09. Five research agents each covered one option or
question, and an independent fact-checker re-checked each report against
primary sources. The probe scripts cited as [22] ran in a temporary
directory and were not kept. To reproduce them, rewrite them as part of
the M7 corpus.

## 1. The question

Should fitment place the candidate in the assembly and check its body against the other solids with a geometry kernel, so that it can return `Match`? Or should it stop at the interface and return `InterfaceMatch` with "fit not checked"?

**Why it matters now.**
- **M0:** the answer fixes the verdict types, the CLI and JSON wording, the exit codes and the M4 false-positive metric. All of these are hard to change after the first publish.
- **Conflict in the plan:** the M4 corpus includes "a body that collides", and rule 7 requires zero false positives at M4. Collision rejection only arrives in M7. So M0 has to say how a colliding body scored `InterfaceMatch` is counted [1: PLAN.md:88, 217, 220, 252].
- **M7:** the answer decides what gets built.

## 2. Options

Effort is my own relative estimate.

| Option | Can it wrongly pass a candidate? | MIT crate / licence | Build burden | Maturity | Effort |
|---|---|---|---|---|---|
| **(a) Interface only, permanently** | No. It never claims fit, so `Match` is never produced. | Clean | None | n/a | None |
| **(b) OCCT in-process, through a binding in an optional crate.** Best published binding: cadrum 0.8.20 (on crates.io, MIT wrapper, OCCT 8.0.1). | Yes, as published. No binding exposes warnings, validity, tolerance or distance. An OCCT segfault or a C++ exception through cxx kills the host process. | Publishable. Needs deny.toml exceptions. Any shipped binary contains statically linked LGPL OCCT. | A C++17 compiler on every OS. Either a 33–131 MB prebuilt download with no checksum, or a 20–60 min CMake build. | cadrum is 7 months old with one maintainer. opencascade-rs has one spare-time maintainer and is on OCCT 7.8.1. | L–XL, and it still ends up needing a subprocess |
| **(c1) OCCT out of process: Python helper** (cadquery-ocp-novtk, pinned with uv) | Only through known failure modes, each guarded to "not verified". A crash or timeout never counts as a pass. | Rust side stays pure Rust and MIT. OCCT is not in the Cargo graph. OCCT is linked dynamically from the PyPI wheel. | Python 3.11–3.14 and uv. Wheels for macOS 11+ (arm64, x86_64), manylinux_2_28 (x86_64, aarch64) and win_amd64. None for musl or Windows ARM. | OCCT 8.0.1 via OCP 8.0.1.1. Same stack as stepq's oracle. | M |
| **(c2) OCCT out of process: Rust helper binary** on fitment's own C++ bridge | Same as (c1) | Publishable. The helper links OCCT statically (LGPL §6 on shipped binaries) and needs unsafe code and C++. | Same as (b) | Same as (b) | L |
| **(d) monstertruck** (pure-Rust kernel) | Yes. It ignores length and angle units, guesses cone-angle units, can produce NaN placements, its booleans fail on coplanar and coaxial contact, and it has no B-rep distance. | Apache-2.0. Its effective minimum Rust version is 1.88; fitment declares 1.85. | Pure Rust, but upstream CI runs only on nightly and only on Ubuntu. | First release 2026-03-03, about 450 downloads, one maintainer. | XL, unproven |
| **(e) Tessellate, then parry3d-f64** | Yes. Containment is invisible, touching reads the same as penetrating, and unsupported sub-queries read as "far". An uninflated mesh hides interference smaller than its deflection. | parry is Apache-2.0, but a tessellator is still needed (OCCT or monstertruck). | Whatever the tessellator needs | parry is mature as a physics library, but its semantics are wrong for solids. | L, plus a kernel anyway |

**(a) Interface only.** This is the behaviour the plan already describes [1]. The cost is that every passing result tells the user to check the body fit in CAD.

**(b) In-process binding.**

cadrum 0.8.20 (2026-09-13) is MIT, on crates.io, and statically links OCCT 8.0.1 [2][3]. PLAN.md's "7.9.3" is out of date. Its gaps:
- **Missing functions:** no shape-to-shape distance, and no validity check.
- **STEP reading:** `read_step` flattens the file into anonymous solids and silently drops non-solid bodies.
- **Booleans:** they check `HasErrors` only. Warnings are ignored and there is no fuzzy or tolerance control.
- **Crashes:** volume, contains and bounding box are bridged without `Result`, so an OCCT exception in them calls `std::terminate` [6]. Signal conversion is compiled out, so a segfault kills the process [7].
- **Releases:** breaking changes ship in 0.8.x patch releases [5]. The prebuilt OCCT tarballs are downloaded without a checksum and were replaced in place on 2026-09-03 under the same URL [4].
- **Platforms:** the Linux prebuilts need glibc 2.28 or later, and Windows MSVC needs a VS 2022 17.14+ linker [8].
- **Maintenance:** 557 of 561 commits are by the owner, and there is one reverse dependency [9].
- **cargo-deny:** it fails fitment's deny.toml, because a build dependency (minreq) pulls in webpki-roots, which is CDLA-Permissive-2.0 [39][40].

The other binding is opencascade 0.3.0 with occt-sys 7.8.1. These crates are LGPL-2.1, have been on crates.io since 2023 and got new versions in August 2026 [10]. Its problems:
- **CMake 4:** it fails unless `CMAKE_POLICY_VERSION_MINIMUM` is set; this is why the 0.3.0 docs.rs builds failed [11].
- **Windows:** MSVC builds are reported broken [12].
- **No fallible API** [13].
- **Exceptions:** in OCCT 7.8.1, `Standard_Failure` does not derive from `std::exception`, so even a function declared `-> Result` aborts unless the shim catches it explicitly [14].
- **Exit crash:** static OCCT 7.8 has a known crash at process exit, fixed in 7.9 [15].

A sound check needs fitment's own cxx bridge whichever binding is used; building on occt-sys this way is an existing pattern [16]. Segfaults also cannot be caught: stepq's oracle already hits one in cadquery-ocp 8.0.1 [26]. So (b) still needs a subprocess, which turns it into (c2).

**(c) Out-of-process OCCT helper.**

In (c1), a long-lived helper process exchanges JSON lines with fitment:
- It runs as `uv run --script --locked` with the cadquery-ocp-novtk version pinned exactly [23][25].
- The novtk build avoids the 81–146 MB VTK wheels. The OCP wheels themselves are 47–68 MB [23].
- On first run, uv downloads Python and the wheels [25].
- The Rust `FitVerifier` spawns the helper and enforces a wall-clock timeout. Any crash, timeout, non-zero exit or malformed reply becomes `NotVerified`.

A fact-checker re-ran probes on cadquery-ocp 8.0.1 [22]:
- Face-touching boxes give an empty Common. This matches the Boolean specification [18].
- A 1 µm overlap and a fully contained box are both detected.
- A 1e-7 mm overlap is absorbed and a 1e-5 mm overlap is detected.

(c2) uses the same architecture with a Rust helper binary. It trades the Python runtime for (b)'s build burden and static LGPL obligations. The M8 viewer needs tessellation, and the same helper could supply it; that is my inference.

**(d) monstertruck 0.4.1.**
- **STEP reader:** it ignores the file's length and angle units and does not follow `MAPPED_ITEM` [32]. It treats a cone semi-angle as degrees only when its magnitude exceeds π [30]. It yields NaN for an axis along −X with no ref_direction, and never normalises directions [31].
- **Booleans:** they are mesh-driven. Per PR #31, block-on-block, pocket and small-hole cases fail on master [29]. The maintainer said the truck CSG "isn't great/useful for production" [28].
- **Queries:** it has no B-rep distance or volume. Mesh collision ignores in-plane contact and misses containment [34].
- **Build:** the repo builds and runs CI on nightly only, and the code needs Rust 1.88 or later [33].

It is usable only as an experiment checked against an OCCT oracle.

**(e) Mesh plus parry3d-f64.**
- **Semantics:** parry 0.31.1 treats a TriMesh as a surface. In a test run, a mesh inside another reported "not intersecting" at distance 4.0 [22]. Touching and penetrating shapes both give distance 0, and no penetration depth is available. Unsupported sub-queries come back as distance `Real::MAX` [35].
- **Tessellation:** stepq does not tessellate, so this needs a kernel anyway. OCCT does not document deflection as a guaranteed bound [21]. An uninflated mesh check missed a real 4 µm interference at deflection 0.01 mm [22]. Inflating the mesh instead rejects every seated contact.

It cannot give a sound `Match`, and the proposed reject-only pre-filter cannot be built as described either.

**Licensing, common to all OCCT routes.** This is my reading of the licence text, not legal advice.
- **Licence:** OCCT is LGPL-2.1-only WITH OCCT-exception-1.0. The exception covers header material compiled into your own object code. It does not cover linking [37].
- **Source crates:** an MIT source crate that depends on an OCCT crate carries no OCCT itself.
- **Binaries and images:** whoever distributes a binary or image owes the §6 duties: notice, the LGPL text, the OCCT source including patches, and relink material (or dynamic linking).
- **Hosting:** running a hosted server is not distribution.
- **Issue #1564 (open, no milestone):** 8 core TKGeomBase files carry a proprietary "may not be provided … to any third party" header [38]. The header has been there since 7.7.0 and is still in 8.0.1. It is also inside the occt-sys crate source on crates.io.
- **Wheel contents:** the cadquery-ocp wheel bundles 78 dylibs, including FreeImage, FreeType, OpenEXR and libtiff, but ships only an Apache-2.0 LICENSE [42].

With (c1), fitment's repo and crates never redistribute OCCT. An official Docker image that included the wheel would.

## 3. Definition of a sound fit check

**What it claims.** Static interference and clearance against the modelled solids of this assembly, and nothing more. Each verdict always lists as not checked: motion, unmodelled items, tool access, tolerance stack-up, and fasteners for the new part.

**Inputs**
- **Placements T_k.** The placement under which the interface passed, one per occurrence k. Fit is tested under the same T_k and never re-aligned. If the interface leaves a degree of freedom free (one axis with no anti-rotation feature, or no seating plane fixing the slide), the result is `NotVerified("placement not unique")`.
- **Check set B_k.** Every solid in the assembly except the occurrences of the part being replaced, at stepq's placements. It also includes the candidate's copies at every other occurrence j. Neighbours are not culled, or are culled only with enlarged (non-optimal) boxes, because the optimal box came out too small in one OCCT 8.0.1 B-spline case [41]. Neighbour-to-neighbour pairs are never tested.
- **Declared zones, from the reviewed socket.**
  - Contact zones: each seating plane (the plane ±δ over its contact outline) and each clearance-fit bore.
  - Overlap zones, only for "threaded or press fit" requirements: the annulus between r_hole − δ and r_shaft + δ about that requirement's axis, over the engaged length ±δ.
  - Nothing else is declared.
- **Tolerance δ.** The D9 linear tolerance (default 0.01 mm), never below the files' stated uncertainty.

**Tests, for every occurrence k and every body B in its check set**

1. **Inputs are trustworthy.**
   - The candidate C and B are closed, valid, positive-volume solids (BRepCheck_Analyzer and BRepAlgoAPI_Check pass) [20].
   - Each shape's largest sub-shape tolerance is at most δ/10 (my proposal; tune on the corpus).
   - OCCT's solid count and per-solid volume agree with stepq's: relative 1e-9 for parts, 1e-6 for placed bodies, as in stepq's verify-split [26].
   - A placement and unit self-check passes: a matched interface axis as read by OCCT equals stepq's within δ.
   - Any failure gives `NotVerified`.
2. **Interference.**
   - Compute Common(T_k·C, B) with no fuzzy value, single-threaded. Any error or warning gives `NotVerified`.
   - Any result solid outside an overlap zone gives `Interferes`, with the neighbour, volume and bounding box as evidence.
   - There is no minimum-volume threshold: the real 4 µm interference had a Common of only 0.021 mm³ [22].
   - Common also catches full containment, which a boundary distance would miss [18][22].
3. **Undeclared proximity.**
   - Face contact gives an empty Common (Boolean spec case 20 [18]), so contacts need their own test: every place where the candidate comes within δ of B must lie in a declared zone.
   - Possible implementations, unproven until tested on the corpus:
     - Common(Offset(T_k·C, +δ), B), with the declared zones cut away, must be empty.
     - Section plus BRepExtrema_DistShapeShape, with each contact edge tested against its declared plane or cylinder [19].
   - An offset failure or an unclassifiable curve gives `NotVerified`.
   - Contact outside the zones gives `Reject("undeclared contact")`, since rule 4 rejects at the tolerance edge.
4. **Control.** Run tests 1–3 on the original part at its own placements. If the original is not `Verified`, every candidate's fit is `NotVerified("original fails its own fit check")`. This catches socket omissions (rule 3) and assemblies that already interfere.

**Fasteners.** Bolts are neighbours.
- A clearance-fit shank never overlaps, and its head touches only a declared seating plane.
- A bolt modelled at major diameter in a hole modelled at minor diameter (common CAD practice [44]) may overlap only inside its own requirement's annulus.

**Seating planes.** Touching faces give an empty Common, including for a part rotated away and back [22]. Such contact is allowed only on a declared plane.

**Tolerance.**
- Never set a fuzzy value: it makes embeddings up to that depth count as coincident [18].
- OCCT absorbs overlaps somewhere between 1e-7 and 1e-5 mm on clean primitives, and up to the shape tolerance on imported files. That is why test 1 caps the tolerance.

**Outcome.** `Verified` only if tests 1–4 pass for every k and B. Any crash, timeout, error, warning or unsupported geometry gives `NotVerified`, never `Match`.

**Determinism.**
- Pin the exact kernel build, run single-threaded, and record the kernel and version in the evidence.
- Compute volumes with the Eps overload per solid; the default quadrature varies by about 1e-6 between runs [26].
- Re-run the corpus on every kernel upgrade.

## 4. Recommendation

**M0: adopt (a)'s semantics now, with fit as a first-class field.**
- A `Match` can only be built when fit is `Verified`.
- The default `FitVerifier` returns `NotChecked`.

**M7: build (c1).** That means an optional `fitment-fit` crate in pure Rust, holding the protocol, the guards and the `FitVerifier` implementation. It drives a pinned, locked OCCT helper that implements section 3. (c2) is the fallback if Python is unacceptable in deployment.

**Rejected:**
- **(b):** segfaults cannot be contained in-process, and no published binding exposes the guard APIs. You would write a bridge and still need a subprocess.
- **(d) and (e) as sources of `Match`:** see section 2. Revisit monstertruck once it reads units, has a B-rep distance and handles coplanar booleans. It could be trialled against the (c1) helper as an oracle.

**Why (c1):**
- It is the only option where every known failure lands on "not verified" and none on "pass".
- Every published crate stays pure Rust, MIT and within the current deny.toml.
- It reuses the OCCT 8.0.1 stack that stepq's oracle already pins.
- It is the cheapest to build.

**Costs of (c1):**
- Python 3.11 or later and uv wherever fit checks run.
- No musl or Windows ARM support.
- Reading large assemblies in OCCT can take minutes, so the helper must be long-lived.

**What this means for M0's wording**
- **Types:**
  - `Verdict::{Match, InterfaceMatch, Reject}`.
  - `FitStatus::{Verified{kernel, version}, Interferes(evidence), NotVerified(reason), NotChecked(reason)}`.
  - A verified interface with fit `NotChecked` or `NotVerified` gives `InterfaceMatch`; `Interferes` gives `Reject`.
- **CLI:** `InterfaceMatch: interface verified; body fit not checked (no fit verifier in this build)`.
- **JSON:** `"fit": {"status": "not_checked", "reason": "no fit verifier in this build"}`, plus the not-covered list.
- **Exit codes:** separate codes for Match, InterfaceMatch and Reject, and a `--require-fit` flag that treats InterfaceMatch as a failure.
- **M4 metric:** a false positive is `Match` on any known-bad case, or `InterfaceMatch`/`Match` on a case whose interface is bad. `InterfaceMatch` with fit not checked on a colliding-body case is correct. M7's exit test becomes: those cases end as `Reject`, or stay `InterfaceMatch` with `NotVerified`, but are never `Match`.
- **Document fixes:**
  - PLAN.md "Kernel options" (line 282): cadrum is on OCCT 8.0.1, not 7.9.3; opencascade-rs is on crates.io (0.3.0, LGPL, OCCT 7.8.1).
  - The Vectera audit calls bschwind/opencascade-rs a "git fork", but it is the upstream repository [43][10].

## 5. Questions for the user

1. **Semantics.** `Match` requires a verified body fit. Until a verifier exists the best result is `InterfaceMatch`, and M4 counts `InterfaceMatch` on a colliding-body case as correct. Agree? (Recommended: yes.)
2. **Name and exit code.** Keep `InterfaceMatch`, or rename it before the first publish (for example `InterfaceOnly`) so nothing that filters on "Match" can confuse the two? And should it get its own exit code plus `--require-fit`? (Recommended: keep the name, give it its own exit code, add the flag.)
3. **M7 route.** Choose one:
   - Python helper: needs Python 3.11+ and uv where fit checks run; no musl or Windows ARM.
   - Rust helper binary: needs a C++ and CMake build; shipped binaries contain statically linked LGPL OCCT.
   - No M7: interface-only permanently.

   Also: may official release artifacts bundle OCCT while #1564 is open? (Recommended: Python helper, and no OCCT in official artifacts until #1564 is resolved. This can wait until M7 starts, but the default should be recorded now.)

## Still uncertain

- **Not built here:** cadrum, opencascade 0.3.0 from crates.io, and monstertruck on stable Rust were not built on this machine (macOS, Darwin 27).
- **Untested design pieces:** the offset-based proximity test, and how Section behaves on coincident faces.
- **OCCT absorption threshold:** not pinned down; 1e-6 mm was never tested.
- **Cross-platform determinism:** whether OCCT gives bit-identical results across platforms is unverified.
- **Legal readings:** "hosting is not distribution" and "an MIT crate depending on an OCCT crate carries no OCCT" are interpretations of the licence text. Whether and how #1564 will be resolved is unknown.
- **Search snapshot only:** the content of OCCT tracker issue 33865 (static linking) could not be confirmed; the tracker host does not resolve.
- **cadrum and glam:** whether cadrum 0.8.20 compiles against glam 0.34 is unverified.
- **parry issues:** whether #289 and #132 still reproduce on 0.31.1 is unverified.
- **Probe scripts:** they live in this session's scratchpad and will not persist [22].
- **Effort sizes:** my own estimates.

## Sources

1. /Users/julian/projects/fitment/docs/PLAN.md (lines 88, 159, 165, 217, 220, 240, 252, 282)
2. https://crates.io/api/v1/crates/cadrum
3. https://docs.rs/crate/cadrum/0.8.20/source/build.rs ; https://github.com/lzpel/cadrum/blob/8788df70c60b986b5ab387edb75a2f6f341a8c7a/build.rs#L7-L10
4. https://api.github.com/repos/lzpel/cadrum/releases/tags/occt-8_0_1_rev2
5. https://github.com/lzpel/cadrum/blob/main/CHANGELOG.md
6. https://github.com/lzpel/cadrum/blob/0a76b071f67f01ea0b4534458e0c2c6ab8f9c74f/src/ffi.cpp ; https://github.com/lzpel/cadrum/blob/0a76b071f67f01ea0b4534458e0c2c6ab8f9c74f/src/ffi.rs#L89-L102 ; https://docs.rs/cadrum/latest/cadrum/occt/solid/struct.Solid.html ; https://cxx.rs/binding/result.html
7. https://github.com/lzpel/cadrum/blob/0a76b071f67f01ea0b4534458e0c2c6ab8f9c74f/build.rs#L586-L608
8. https://github.com/lzpel/cadrum/issues/276 ; https://learn.microsoft.com/en-us/cpp/porting/binary-compat-2015-2017
9. https://api.github.com/repos/lzpel/cadrum/contributors?anon=1 ; https://crates.io/api/v1/crates/cadrum/reverse_dependencies
10. https://crates.io/api/v1/crates/opencascade ; https://crates.io/api/v1/crates/occt-sys ; https://api.github.com/repos/bschwind/opencascade-rs
11. https://github.com/bschwind/opencascade-rs/issues/209 ; https://docs.rs/crate/opencascade/0.3.0/builds/4202547
12. https://github.com/bschwind/opencascade-rs/pull/234 ; https://github.com/bschwind/opencascade-rs/pull/216
13. https://github.com/bschwind/opencascade-rs/issues/24 ; https://github.com/bschwind/opencascade-rs/issues/172
14. https://github.com/Open-Cascade-SAS/OCCT/blob/V7_8_1/src/Standard/Standard_Failure.hxx ; https://github.com/Open-Cascade-SAS/OCCT/blob/V8.0.1/src/FoundationClasses/TKernel/Standard/Standard_Failure.hxx
15. https://github.com/Open-Cascade-SAS/OCCT/issues/146
16. https://docs.rs/crate/runmat-geometry-io/0.6.2/source/build.rs
17. https://github.com/Open-Cascade-SAS/OCCT/releases/tag/V8.0.1
18. https://occt3d.com/dev/doc/overview/html/specification__boolean_operations.html
19. https://occt3d.com/dev/doc/refman/html/class_b_rep_extrema___dist_shape_shape.html
20. https://occt3d.com/dev/doc/refman/html/class_b_rep_algo_a_p_i___check.html ; https://occt3d.com/dev/doc/refman/html/class_shape_analysis___shape_tolerance.html
21. https://occt3d.com/dev/doc/overview/html/occt_user_guides__mesh.html
22. Local probes run on cadquery-ocp 8.0.1.0.0 and parry3d-f64 0.31.1 (not persistent): /private/tmp/claude-501/-Users-julian-projects-fitment/a5c45451-ac4a-479b-8b68-aa1013b0eef5/scratchpad/fc_rerun/ (occt_probe.py, hidden_probe.py, coincide_probe.py, parrytest/src/main.rs)
23. https://pypi.org/pypi/cadquery-ocp/json ; https://pypi.org/pypi/cadquery-ocp-novtk/json ; https://pypi.org/pypi/vtk/9.6.2/json
24. https://github.com/CadQuery/OCP/releases
25. https://docs.astral.sh/uv/guides/scripts/
26. /Users/julian/projects/stepq/tools/verify-occt.py (line 4, lines 96-98, lines 112-113) ; /Users/julian/projects/stepq/tools/verify-split.py:8-22 ; /Users/julian/projects/stepq/docs/ARCHITECTURE.md:51-58, 79-90
27. https://crates.io/api/v1/crates/monstertruck
28. https://github.com/virtualritz/monstertruck/issues/24
29. https://github.com/virtualritz/monstertruck/pull/31
30. https://github.com/virtualritz/monstertruck/blob/d2ad1ed7aa420b7f32cd2c1e3ad319b46d593652/monstertruck-io/src/step/load/step_types/surface.rs#L349-L355
31. https://github.com/virtualritz/monstertruck/blob/d2ad1ed7aa420b7f32cd2c1e3ad319b46d593652/monstertruck-io/src/step/load/step_types/placement.rs#L63-L94 and #L213-L247
32. https://github.com/virtualritz/monstertruck/blob/d2ad1ed7aa420b7f32cd2c1e3ad319b46d593652/monstertruck-io/src/step/load/mod.rs
33. https://github.com/virtualritz/monstertruck/blob/d2ad1ed7aa420b7f32cd2c1e3ad319b46d593652/rust-toolchain.toml ; https://github.com/virtualritz/monstertruck/blob/d2ad1ed7aa420b7f32cd2c1e3ad319b46d593652/monstertruck-meshing/src/tessellation/triangulation/boundary/loops.rs#L155
34. https://github.com/virtualritz/monstertruck/blob/d2ad1ed7aa420b7f32cd2c1e3ad319b46d593652/monstertruck-meshing/src/analyzers/collision.rs
35. https://docs.rs/parry3d/0.31.1/parry3d/query/fn.intersection_test.html ; https://docs.rs/parry3d/0.31.1/parry3d/query/fn.distance.html ; https://docs.rs/crate/parry3d-f64/0.31.1/source/src/query/distance/distance_composite_shape_shape.rs
36. https://crates.io/api/v1/crates/parry3d-f64
37. https://github.com/Open-Cascade-SAS/OCCT/blob/master/OCCT_LGPL_EXCEPTION.txt ; https://github.com/Open-Cascade-SAS/OCCT/blob/master/LICENSE_LGPL_21.txt ; https://spdx.org/licenses/OCCT-exception-1.0.html
38. https://github.com/Open-Cascade-SAS/OCCT/issues/1564 ; https://raw.githubusercontent.com/Open-Cascade-SAS/OCCT/V7_7_0/src/GeomConvert/GeomConvert_CurveToAnaCurve.cxx ; https://docs.rs/crate/occt-sys/7.8.1/source/OCCT/src/GeomConvert/GeomConvert_CurveToAnaCurve.cxx
39. https://embarkstudios.github.io/cargo-deny/checks/licenses/cfg.html ; https://crates.io/api/v1/crates/webpki-roots ; https://github.com/lzpel/cadrum/blob/main/Cargo.toml
40. /Users/julian/projects/fitment/deny.toml ; /Users/julian/projects/fitment/Cargo.toml
41. https://github.com/Open-Cascade-SAS/OCCT/issues/1604
42. Local wheel listing: ~/.cache/uv/archive-v0/4KYJgp-uKdo9WYbY/OCP/.dylibs
43. /Users/julian/projects/fitment/docs/vectera-audit.md:181-199
44. https://www.cati.com/?p=132094