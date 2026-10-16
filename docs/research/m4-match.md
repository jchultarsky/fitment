# fitment M4: matching a candidate to a socket

As of 2026-10-15. Status: **approved** on 2026-10-15 with every default of section 2, after one consistency and three soundness reviews (section 28). Nothing here is implemented; section 22 is the order of work. Every default in section 2 is assumed by the rest of the document.

M4 delivers matching: a candidate part is read, cheaply shortlisted, placed in a socket and checked requirement by requirement, and the result is a verdict with its evidence. `fitment match socket.json candidates/*.stp` runs it, and a known-answer corpus with its harness measures it. It is done when the known-bad set gives zero false positives and the false-negative rate is reported. A false positive is `Match` on any bad case, or any pass on a bad interface. `InterfaceMatch` on the colliding-body case is correct until M7 ([PLAN.md](../PLAN.md), Milestones and the M4 prompt).

Four agents designed one area each, to one brief:

- A: candidate features (CF), the signature (SG) and the shortlist (SL);
- B: alignment (AL) and verification (VR);
- C: verdicts (VD), evidence (EV), the `fitment.match` document and the CLI (MF), and the release;
- D: the known-answer corpus (KC), its harness (KH) and its oracle (KO).

They measured on 2026-10-13 at fitment `08c70a9`, since rebased away; its tree is the tree of `3078db8`, the head of `m4/design` (`git diff 08c70a9 3078db8` is empty), so this document cites `3078db8` ( `EXTRACTOR_VERSION` 6, `SOCKET_RULES_VERSION` 5), stepq 0.6.0, the generated corpus (`tools/gen-corpus.py`, cadquery 2.8.0), the 45 fetched fixtures and the release binary. `main` has since gained only documentation commits; the code is unchanged. This document merges their work and settles where they disagreed (section 26). Their reports (`A.md` to `D.md`), probes and raw output are in [m4-evidence/](m4-evidence/), which is not committed; a probe is cited by its path there, as `B/out/y4.txt`. Rules keep their designers' ids; no rule was renumbered.

## 1. Summary

- **Decisions that need your judgment** (section 2, marked "Your call"): M4-1 (a socket PR first, changing `fitment.socket` before it freezes), M4-2 (the split floor), M4-7 (D6), M4-8 (D7: `hard` only in M4, `warning` deferred to M7), M4-18 (exit 4 as the normal success), M4-28 (formats frozen at the publish), M4-37 (D9, the default tolerance) and M4-38 (D8, ground truth). The other rows are engineering defaults.
- **A candidate is a canonical part, read once by M2's gate.** A refused file is a `Reject` and goes no further. An accepted one offers its hole and shaft stacks, the planes of its shell and its cones. Undecided and partial cylinders, faces of unknown outline and `other` faces never satisfy a check, and a stack with an unknown or irregular end satisfies nothing past its last known station (CF5, M2 9.6). Over every required span, no candidate face but the stack's own walls may lie in the required core (VR22): a post, a dome, a bar across the bore or a lip rejects.
- **The signature is a small graph of rigid invariants:** diameters, the angle and distance between axes, plane separations, plane-to-axis distances. It is policy-free, so the catalog (M5) stores it once. Invariants agree to 3.0e-11 mm across frames and writers and to 6.8e-12 mm across AS1's exporters (`A/out/report.txt`).
- **The shortlist is necessary, never sufficient.** A candidate is dropped only when some required element or pair of elements has no counterpart within windows at least as wide as anything verification accepts. Section 6.4 proves it never drops a candidate verification would pass. The original keeps its own socket's shortlist in 157 of 157 sockets.
- **One rigid placement per socket.** Alignment builds placements from pairs of candidate features that match the socket's lines, seats and cones, ranked but never filtered by diameter, and every requirement of the socket is checked under that one placement. Placement error can only cause a false negative.
- **The floor splits in two (M4-2).** A comparison with the original's own geometry passes only when `|found − original| < t − u`, with u the two parts' own stated uncertainty (at least 1e-6 mm). A requirement's `floor_mm` enters only the limits derived from neighbours, where a clearance needs a radial margin above τ = max(`contact_mm`, the span's floor, u), as the socket's own rule B1 did. AS1's MicroStation export then passes its own sockets; `as1_pe`, the weldment's bearing and vaccase's tee still fail every candidate, as `floor_above_tolerance`.
- **Seats are covered by a certified test,** exact lines and arcs marched with a Lipschitz bound, never sampled.
- **Unverifiable requirements fail every candidate,** the original included, and do not stop the run. Removed ones are listed and not checked.
- **A verdict per socket, then per candidate:** the worst of its sockets, so a candidate that fits one occurrence and not another is a `Reject`. Zero sockets reject. Every check records its measures (found, nominal, effective limits, margin) and every failure with a code from a closed list; a test recomputes each verdict from its evidence alone.
- **Exit codes:** 0 `Match`, 4 `InterfaceMatch`, 5 every candidate rejected, 1 error, 2 usage, 3 refused input. M4 has no fit verifier, so every passing run exits 4; `fitment match … && …` can never take an unchecked body for a fit.
- **Y4 holds.** The original, aligned from scratch, passes everything M4 can verify and fails exactly the rest, on 224 of 225 corpus socket runs and all 78 fixture sockets (`B/out/y4.txt`, `B/out/fix.txt`), measured before the review's fixes; VR22 as revised was then measured on every original's own geometry: every span passes but 4 whose core holds the original's own material, which M3's U11 makes unverifiable (s50 CAP's skirt bore and spigot, s54's two pins; `merge/y4probe/y4-vr22.txt`). The one exception is s55's plate of 3,600 tiles, which exhausts the placement budget unseeded and passes when seeded with the identity; the test names it and runs with the shipped budgets.
- **Zero false positives on the first slice.** D's 40 candidates against s01, s02, s03 and s18: every bad and edge case rejected, every `fits` case passed but one, the colliding lug passed (M7's). The miss, s18's countersunk plate, waits for a socket change (M4-1). The soundness review then built candidates the rules as first written passed, each interfering with the very neighbour its passing requirement cites: a post or a dome rising inside a bore, a bar across it, a lip in its mouth, a pocket hidden in a seat face of unknown outline, an opening past the original's rim, and a clearance at exactly its τ limit. The rules now reject each, and each is a corpus case (section 28). Two later fixes that added passes (a per-span free radius for shafts, a floating seat under D7 warning) opened new paths and were withdrawn for conservative rules: a shaft's core is unbounded, the original's own material in a core makes a requirement unverifiable, and D7 is `hard` only.
- **The known-answer corpus:** about 175 generated candidate files with physical truth labels, checked by an Open CASCADE physics oracle that caught 3 of its first 35 labels wrong; about 530 runs on components read in place (the originals in up to four writings, AS1 across exports, review runs); about 1,200 match runs in all, the generated cases and the originals under both policies M4 offers. A bad case must be rejected for its injected defect, named by locators and failure codes.
- **Seven PRs,** the first a socket PR that settles what M4 asks of M3 before `fitment.socket` v1 freezes; the last publishes `fitment-core` and `fitment` 0.1.0 (section 22).

## 2. Decisions for review

Each has a recommended default. The rest of the document assumes the defaults. Rows marked **Your call.** change scope, a format or what fitment promises; the rest are engineering defaults. Section 26 says which designer decisions each row came from; section 28 says which rows the reviews changed.

| # | Question | Default | What it costs or buys |
| --- | --- | --- | --- |
| M4-1 | **Your call.** Should M4's first PR change the socket? | Yes. A socket PR (`SOCKET_RULES_VERSION` 6), before `fitment.socket` v1 freezes, making the socket requests SR1 to SR9 (section 21): each span piece states its basis and a zone piece the original's profile (SR1); a conical seat states its sense (SR2) and the union of its seats' radial ranges (SR8); `active`, `removed` and `contact_mm` accessors (SR4); `Requirement::verifiable()` the one predicate M4's verification and M3's Y4 witness test both call (SR5); B1's guard band (SR6); interior span ends widened by τ (SR7); a lenient reader of older documents' reviews (SR9, MF11). SR3 (the end `max_extra_mm` is measured from) is optional | Every socket document is extracted again. What is kept (MF11): every review decision on a requirement whose id still exists and whose fields common to both versions are equal; a decision on a requirement whose values changed (a span end widened by SR7, a fit re-read under SR6, a cone's range under SR8) goes to `stale_reviews` as `requirement_changed`; `fitment socket -o` never refuses to overwrite the user's own older document for its age (one edited by hand, its digest no longer its own, is refused as any is). It makes s18's countersunk plate verifiable, and M3 and M4 can no longer disagree about what is verifiable. If declined: M4 fails zone spans itself (`zone_not_read`, VR6), VR4's guard has no counterpart in B1 (an original clearance whose margin lies within u of τ then fails as `floor_above_tolerance` by VR4's no-room branch: Y4 stays exact, a requirement is lost), and the format freezes without the fields |
| M4-2 | **Your call.** The floor | Split (VR2, VR4, VR5). Comparisons with the original's own geometry use u = max(both parts' `part_uncertainty_mm` and vertex offsets, 1e-6 mm); `floor_mm` enters only limits from neighbours. A requirement is unverifiable under the policy (`floor_above_tolerance`, VD6) when t − u ≤ 0, or when a neighbour limit leaves no room | AS1-md (placement floor 0.01 mm, parts 2.1e-6) and 67 weldment requirements become checkable, and their originals pass them; a single floor failed all of them and the originals with them. `as1_pe` (0.115 to 0.672 mm), the weldment's bearing (0.0165) and vaccase's tee (0.13) still fail every candidate |
| M4-3 | A value at the edge of the tolerance | Passes only when `deviation < t − u` (VR2, VR3) | All of D's edge cases reject in every frame measured; a value within u of the edge is a false negative |
| M4-4 | Clear diameters (limits from neighbours) | Radial margin above τ + u, with τ = max(`contact_mm`, span floor, u); never widened by t (VR4); M3's B1 takes the same guard (SR6) | The socket's own rule B1 applied to the candidate, with VR2's guard so the edge rejects in every frame: at exactly c + 2τ a margin read in 716 of 1,901 decimal clear diameters above τ (section 28). Clear + 0.01 rejects |
| M4-5 | A candidate hole or shaft with an unknown or irregular end | M2 9.6 as written: the stack satisfies nothing past its last known station; the unknown end is never an opening or a bottom; every span on it is also subject to VR22's core test (CF5) | The face the walk could not read may rise back inside the bore (a dome, a spike). Costs Y4 nothing: the 3 original requirements on such stacks are unverifiable anyway (section 26) |
| M4-6 | Spans with no clear diameter and no pin (a zone whose occupant was not read, a piece past a known end) | Zone pieces: checked against the original's profile once M3 states it (M4-1); until then unverifiable (`zone_not_read`). Pieces past a known end: unverifiable | s18 ×2, s58 and vaccase's tee carry such spans. A plain hole under a countersunk head must not pass |
| M4-7 | **Your call.** D6 | Strict. Functional drops the upper diameter bound only; seat regions are not shrunk (VR7, VR10). A functional pass that strict would refuse carries `larger_than_original` | Functional gains little: a wider hole cuts the target face, so the seat still rejects (`bracket_dia_large` stays a false negative) |
| M4-8 | **Your call.** D7 | `hard` only in M4: a grip is a requirement like any other. D7 `warning` is deferred to M7, when the body-fit verifier can check where the neighbours reseat; the CLI has no `--grip` option in M4, and the document records `"grip": "hard"` so the frozen format already holds the field (VR12, VD13) | Every grip-only change of thickness is a false negative in M4 (`bracket_thick`, `b_thickness`). A warning on the grip check alone would change nothing (two hard seats fix the separation within 2ε), and the floating seat that made it mean something failed three rounds of review (section 28.4), so M4 promises nothing it cannot check |
| M4-9 | Axis position of a clearance bore | Within ε of the original's, like every bore and shaft (VR7) | A clearance hole moved within its clearance is a false negative (`fn_position`); the seat region rejects it anyway |
| M4-10 | Seat coverage | Certified at ε (VR11): no candidate edge deeper than ε/2 inside the target face, the ε-offset curves covered | Never sampled; every point deeper than ε is covered; a candidate edge between ε/2 and ε inside rejects though within the tolerance, a false negative |
| M4-11 | `max_extra_mm` (M3's I4, recorded "for information") | Not a requirement and not checked; the evidence lists it as not checked; a shaft bottoming out is M7's collision | `screw_long_25` passes the interface and collides (`collides` truth) |
| M4-12 | Placements | One rigid placement per socket; every occurrence k of it at `T_k · A`; the first placement passing every requirement, else the one passing most (ties first) for the evidence (AL1, AL10) | Occurrences of a merged socket share one frame; different sockets may need different placements (a washer flipped) |
| M4-13 | Work bounds | Per candidate and socket: `MAX_PLACEMENTS` 4,096, `MAX_ANCHOR_PAIRS` 16 and `MAX_COVERAGE_STEPS` 2·10⁷ (coverage marching and VR22's face tests together), each past its bound a reject (`alignment_exhausted`, `coverage_exhausted`); the shortlist's `MAX_SHORTLIST_CHECKS` 10⁶ comparisons, past them the candidate kept | Bounded time on hostile files: about 5 s of coverage at the measured 4·10⁶ steps a second, under 0.1 s of shortlist; never a pass from a bound, never a drop from the shortlist's. s55 exceeds every bound and is Y4's named exception; every other socket measured needs at most 98 placements and 1.6·10⁶ steps |
| M4-14 | What a candidate can be | A part file; through the API also a leaf of an assembly (`part_of_definition`); an assembly file is refused (`assembly`, M2 check 5) | The originals and AS1's cross-export runs (about 490) need no file; the CLI stays one part per file |
| M4-15 | The signature | Policy-free (`SIGNATURE_VERSION` 1); bins of 0.1 mm; pair tokens for axes within 0.1°; seat area as a filter; the lever is the larger box diagonal of socket and candidate | M5 stores it once, never re-indexed for a policy. Seat-only sockets keep 190 of 291 instead of 282. 3.3 mm position windows on a 4.7 m part |
| M4-16 | Unverifiable requirements in the shortlist | Contribute nothing. Explicit candidates are still judged; a catalog query on a socket with an active unverifiable requirement returns nothing and says why | 69 of 157 sockets by M3's predicate and the floor; 71 under VR6 without M4-1, 70 with it (section 20.1) |
| M4-17 | A shortlist drop of an explicit candidate | `Reject` at stage `shortlist` with what is missing, and alignment and verification still run for the evidence; a verification pass after a drop is a fitment bug (exit 1). In a catalog query (M5) the drop ends the candidate | Every reject is attributed per requirement (KH2) and the shortlist's soundness (KH9) is checked on every run, for a few ms per reject |
| M4-18 | **Your call.** Exit codes | 0 `Match`, 4 `InterfaceMatch`, 5 every candidate rejected; 1 error, 2 usage, 3 refused input; the run's code is the best verdict; `--require-fit` turns 4 into 5 | Every M4 run that passes anything exits 4; scripts test for 0 or 4. M2 reserved "4 upward" for the three verdicts; `Match` departs from that and exits 0, so success means success to a shell (PR 4 amends m2-features.md's note) |
| M4-19 | Evidence of a `Reject` | Every failed check in full; passed checks as id, result and feature | 100 candidates on s01: 956 KB instead of 1.17 MB |
| M4-20 | Result identity and order | Labels (the paths given) kept, outside the digest and the order; order by verdict, stage, failed checks, SHA-256; the same bytes judged once | Independent of argument order; nearest rejects first |
| M4-21 | Unreadable candidates, output, arguments | Judge the rest, write the document, exit 1. A table on stdout; `--format json` or `-o OUT`. Candidates are files or directories (not recursive; `.stp`, `.step`, `.p21`, any case) | An incomplete run never exits 0 or 4; Windows shells do not expand `*.stp` |
| M4-22 | A stale socket (`is_current() == false`) | Refused, exit 3, no override | Rule fixes add requirements; an old socket risks a false positive |
| M4-23 | A reader for `fitment.match` | None in core; tests and M6 read it as JSON | A stored or edited report never becomes a `Verdict` |
| M4-24 | Running the original | `--original ASM` judges the target read from the socket's own assembly; a failure beyond its unverifiable requirements is exit 1 | A self-check of the rules on real files, at the cost of a second read |
| M4-25 | Properties (D10) | None compared; `material_not_compared` on every verdict; the mapping format designed, empty | No fixture states a material or thread as a property |
| M4-26 | A fit check for a socket whose interface failed | Not attempted: `not_checked`, "interface rejected" | M7's kernel time goes to candidates that can still pass |
| M4-27 | Versions | `MATCH_RULES_VERSION` 1, rising with any change to shortlist, alignment, verification or verdict output; `SIGNATURE_VERSION` 1, rising with the stored signature | Goldens and stored results know what judged them; M5 knows when to recompute signatures |
| M4-28 | **Your call.** Formats at the publish | `features.v1`, `fitment.parts` v1, `fitment.socket` v1 and `fitment.match` v1 frozen at 0.1.0; every 0.x release reads `fitment.socket` v1 | Users' reviews survive upgrades |
| M4-29 | Where candidates are built | A new section of `tools/gen-corpus.py`, one manifest, `"kind": "match"` | One lock, one registry, one coverage test; about 900 more lines of generator |
| M4-30 | Truth labels | Physical, checked by the oracle at the construction's placement (KC3, KO1) | Caught 3 of 35 labels wrong; separates the rules' cost from bugs |
| M4-31 | Failure attribution | Locators and failure codes; `must ⊆ F ⊆ must ∪ may`; a `must` entry may be `any_of`; a bad case's `must` is never empty (KH2) | Survives which seat alignment slides onto, and which of a seat or a bore catches a moved hole; forbids "rejected for another reason" |
| M4-32 | False-negative baseline | Every good-truth case and policy that rejects, committed with its failures and cause; re-blessed only with `FITMENT_BLESS=1`, never under `CI`, and for an old case only when `MATCH_RULES_VERSION` rose (KH4, KH5) | Bad cases stay a hard gate; a new false negative never appears silently |
| M4-33 | Gated policies | Zero false positives under `default` and `functional`, the two policies M4 offers. The tolerance sweep is reported, not gated | D7 `warning` returns with M7 (M4-8), gated then |
| M4-34 | Edge cases | Each written in four frames (KC7) | 21 of 40 frames read a 0.01 mm shift inside the tolerance |
| M4-35 | Committed candidates | 13 candidates against the committed s01, s02, s03, in a quarter-turn frame, under a 210 KB cap (KC15) | `cargo test` shows the gate without Python |
| M4-36 | Oracle in CI | Physics on every match case, re-measurement on every verdict, mutations on the corpus | About 3 minutes beyond M3's 11 |
| M4-37 | **Your call.** D9, the default tolerance | 0.01 mm and 0.01°, not tuned in M4; the sweep (0.001, 0.05, 0.1 mm) reported; PLAN's "tune on the corpus" left to M5, with real catalog parts | Tuning on a generated corpus built at 0.01 mm would tune to the generator; changing it later regenerates the edge cases |
| M4-38 | **Your call.** D8, real ground truth | None collected in M4; the generated corpus and AS1's exports only; revisit at M5 with catalog parts and any accepted replacements users supply | The false-negative rate is measured against fitment's own generator family (section 27.1) |

## 3. How a match runs

```text
socket.json ─ the socket gate (MF2) ─────────────────── refused input: exit 3
              reads back, accepted, current,
              reviewed if --require-reviewed
                      │ per socket: active requirements, span floors,
                      │ contact_mm, the target's u, its signature (SG)
candidate bytes ─ M2's gate and features (CF) ───────── refused: Reject (candidate_refused)
   one at a time        │ CandidateView and PartSignature, once
                per socket, in socket order
                      │ shortlist (SL) ── drop: Reject, stage shortlist, what is missing;
                      │                   an explicit candidate goes on for the evidence (M4-17)
                      │ alignment (AL) ── none: Reject, stage alignment, the requirements
                      │                   no candidate feature could anchor
                      │ verification (VR): every active requirement, at one placement
                      │ decide (VD): the socket's interface verdict
                      │ fit (M7; in M4 not checked)
                      ▼
                combine (VD): the candidate's verdict, flags, warnings, reasons
                      │
all candidates ─ order (MF5) ─ fitment.match ─ table, JSON, exit code
```

Each candidate is judged alone: nothing passes from one candidate to the next, which makes the order of arguments irrelevant (tested by permutation). The socket's signature, its alignment tables and its active requirements are built once, in `Matcher::new`.

## 4. Candidates (CF)

A candidate becomes features M4 may use. The rules bind M2's sections 7.4 and 9.6 on the candidate side.

- **CF1.** **Input.** A candidate is the bytes of one STEP part file, read by `FeaturesDocument::read` (M2's stages 1 to 6 and extraction), or a leaf definition of an assembly read by `part_of_definition` (M3's path, which gives byte-identical features). Its `Part` is read once per run, however many sockets judge it (`Matcher::judge_step`, `judge_part`).
- **CF2.** **Refused.** A file the gate refuses is a `Reject` with reason `candidate_refused` and the gate's codes (VD10). It gets no signature, is never indexed (Soundness rule 6) and is never aligned. Measured: 21 of 312 part files are refused (9 corpus `r*`, 12 NIST files: surface models, several definitions, no solid; `A/out/report.txt`).
- **CF3.** **Stacks.** Every hole and shaft of the `Part`, of any kind, is a stack: its sense, its axis as stated in the part's frame, its segments (each a full cylinder, M2's F4 rule) with their diameters and stations, its transitions and their kinds and faces, its ends with their stations, rims and kinds (a `bottom`'s kind, a post's shaft stack), its wall openings, the number of `opening` ends, and whether an end is unknown or irregular. Its extent runs from rim to rim; a bottom's end includes a drill point's tip or the floor of a cone-then-flat or annular bottom. The extent serves the shortlist only; what verification may use is VR8's profile, which stops at a post's free end. There are no void stacks (M2's R5).
- **CF4.** **What never satisfies.** Undecided cylinders, partial cylinders, `other` faces, cones with unknown radii and plane faces of unknown outline are never the element a check passes on (M2 9.6, 7.4). They are counted in the signature's `unread` summary, and the verdict carries `candidate_incomplete` when the part's `complete` is false (VD11). Nothing is assumed about what they do inside a required bore: M2's walk stops at the first face it can classify, so a post rising from an annular bottom, or a face it could not read, may lie inside the bore's core with both ends known. VR22 tests the core instead (section 28, RS1). What they do to the body elsewhere is M7's.
- **CF5.** **Unknown ends** (M2 9.6 as written). A stack with an unknown or irregular end satisfies nothing past its last known station: on that side its usable profile ends at the far station of its last full segment, nothing is credited between that station and the end, and the unknown end is never an opening or a bottom. Its walls before that station may serve a span, and every such span is also tested by VR22, since the face the walk could not read may rise back inside the bore (a dome, a spike: RS1). The shortlist's extent test treats that side as unbounded (a filter must not be stricter than verification). Measured: 103 of 1,114 stacks in the pool have an unknown end. In the measured sockets this decides no original's verdict: the 3 original requirements resting on such stacks are unverifiable for other reasons (section 26).
- **CF6.** **Planes.** Every plane of the candidate enters, with its outward normal, its offset, the area of its faces of known outline (0 when none is known) and the counts of known and unknown faces. A plane of unknown outline covers no seat (CF4) and, since a grip between seats of unknown outline is unverifiable (VR6), places no grip in M4 v1; it is kept so that the stored signature need not change if a later socket rule makes such a grip verifiable.
- **CF7.** **Cones.** Every cone a walk crossed (a countersink or transition of a full stack, so its circles are whole): axis, apex, included angle, sense, and radial range or none (CF4).
- **CF8.** **Wall openings** (a cross hole through a wall, M2's `wall_openings`) are carried with the faces that share their edges. A required span whose stations, widened by ε, meet one fails (`wall_opening`, VR7) unless every face sharing the opening's edges is a hole-sense cylinder or cone, which is air on the core's side: a cross hole passes, a bar across the bore does not (RS2). The originals' openings (p13, vaccase's cross) are cross holes, so Y4 is kept. VR22 is the net behind this rule.
- **CF9.** **Frame and units.** Features stay in the candidate's own frame, in millimetres (an inch part reads Ø254 for AS1 pe's 10 in). Nothing in the signature depends on the frame (SG1).
- **CF10.** **Computed once, versioned.** `CandidateView::of(&Part)` builds once per candidate what alignment and verification read (section 12), and `PartSignature::of(&Part)` the policy-free invariants M5 stores, recorded with `SIGNATURE_VERSION` (1) beside `EXTRACTOR_VERSION` and recomputed when either rises. Both record u_c, the candidate's `max_uncertainty_mm` and its solids' largest `max_vertex_offset_mm`, which VR2 uses.

Measured over the pool of 291 accepted candidates (the fixtures' 24 part files and 83 assembly leaves, the corpus's 45 parts and 139 leaves; `A/out/report.txt`):

| Origin | Parts | Holes | Shafts | Unknown-end stacks | Undecided cyl. | Partial cyl. | `other` faces | Unknown-outline faces | `complete: false` |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Corpus parts | 45 | 55 | 6 | 16 | 1 | 4 | 10 | 5 | 12 |
| Corpus leaves | 139 | 195 | 49 | 3 | 1 | 3 | 11 | 5 | 4 |
| Fixture parts | 24 | 428 | 22 | 42 | 7 | 1,099 | 186 | 61 | 9 |
| Fixture leaves | 83 | 306 | 53 | 42 | 19 | 56 | 35 | 36 | 18 |
| **Pool** | **291** | **984** | **130** | **103** | **28** | **1,162** | **242** | **107** | **43** |

Stacks per part: median 1, 90th percentile 10, largest 84 (`nist_ctc_02_asme1_ap203`, with 82 planes and 82 cones); 41 parts have none. A signature takes a median 0.6 µs from a `Part` (95th percentile 5.6 µs, largest 82 µs); reading the STEP file dominates.

## 5. The signature (SG)

### 5.1 Elements and invariants

- **SG1.** **Invariance.** Every value the shortlist compares is unchanged by a proper rigid motion: a diameter, a length along an axis, the angle between two directions, the distance between two parallel axes, the separation of two parallel planes (o₁ + o₂ for opposed normals, o₂ − o₁ for equal ones, M2 7.4), and a plane's signed distance to an axis parallel to it. They are also unchanged by a reflection, so the signature cannot tell a part from its mirror image; alignment, which uses proper rotations only (M3's P7), does. Measured: s15's block and its mirror keep each other.
- **SG2.** **The candidate's side** is CF3, CF6 and CF7: stacks, planes, cones.
- **SG3.** **The socket's side, per active requirement that is verifiable under the policy (SG7):**

| Requirement | Element it demands | Window (σ the policy's linear tolerance t) |
| --- | --- | --- |
| `bore` span, pinned | a hole-sense segment diameter on the line | [d − σ, d + σ] |
| `bore` span, clearance, D6 strict | the same | [c − σ, d + σ] |
| `bore` span, clearance, D6 functional | the same | [c − σ, ∞) |
| `bore` or `shaft` span piece in a zone or past a known end | nothing about the diameter | none |
| `shaft` span, pinned | a shaft-sense segment diameter | [d − σ, d + σ] |
| `shaft` span, clearance, strict / functional | the same | [d − σ, c + σ] / [0, c + σ] |
| `bore` `open` | as many `opening` ends among the stacks serving the line | a count, not a side (a candidate may be reversed) |
| `bore`, `shaft` spans | the stacks' extent along the line covers the spans' | length ≥ to − from − W_L |
| `seat` | a plane with known area of at least SG6's bound | SG6 |
| `grip`, D7 hard | the two seats' planes at the separation (through seat pairs) | W_L |
| `conical_seat` with radii | a cone of included angle within W_A, radii covering the range ± σ, coaxial with any bore on its axis | W_A, σ, W_L |
| `unresolved`, `partial_*`, unverifiable, removed | nothing | none |

  A span's diameter window comes from `pinned` and `clear_diameter_mm` alone, never from `fit`: `fit` is a label, and a `not_read` span need not be pinned (s18's screw has one with neither).
- **SG4.** **Pairs.** For two required lines: the angle between them and, when the socket states them parallel (sine ≤ 1e-9), the distance from one to the other's span midpoint. For two seats: the angle between their normals and, when parallel or opposed and D7 is hard, their separation. For a seat and a line: the angle between normal and axis and, when the axis is parallel to the plane, the plane's signed distance to it. For a conical seat and a coaxial bore: coaxiality. Parallelism is decided on the socket's side only, on its exact values; the candidate's side is judged by the window.
- **SG5.** **Groups.** Required lines of one sense whose axes lie within W_L and W_A of each other over their spans, and seats facing one way within W_L and W_A (offsets compared only between exactly parallel normals), are grouped, transitively: one candidate element may serve a group's members together (two coaxial holes of a clevis). Each requirement keeps its own domain and windows; a group only lets its members share a witness in pairs, and asks SL5 for distinct witnesses between groups. (As designed, groups were merged into one element whose windows and areas added: that is narrower than verification, which passes each member on its own witness up to W_L + 2ε apart; the PR 2 review showed it, section 22.)
- **SG6.** **Windows.** Every window is at least as wide as what verification accepts (section 6.4), plus a rounding slack of 1e-9 × (1 + |v|):
  - a diameter: σ about the limit;
  - a position, distance or separation: W_L = 2σ + 4Λσ_ang, with Λ the larger box diagonal of the socket's elements and the candidate's (two axes, each within σ of its requirement and tilted by at most σ_ang over at most Λ);
  - an angle: W_A = 2σ_ang;
  - an area: the contact area minus (twice its perimeter × σ + π σ² for each of its loops). What verification may leave uncovered lies within ε of the region's boundary (VR11), and the band of width w about a closed curve of length L has area at most 2wL + πw² (section 25.2, merge finding 2; RS11).
- **SG7.** **What contributes nothing.** A requirement that is not M4-verifiable (VR6), or that fails every candidate under the policy for the target's own uncertainty (VD6 with u_T = max(the target's `part_uncertainty_mm`, `vertex_offset_mm`, 1e-6 mm): t − u_T ≤ 0, or a clearance span with c + 2(τ_T + u_T) ≥ d), contributes nothing and is listed in `blocked_by`. A removed one contributes nothing and is listed in `removed`. The candidate's own uncertainty is not known to the socket's signature; windows built on σ = t contain what VR2's narrower ε accepts, so they stay supersets. Measured under this rule (derived, section 20.1): 69 of 157 sockets have an active requirement every candidate fails, 64 of them by M3's `verifiable()` and 5 more only by the floor (`as1_pe`). VR6's predicate adds s18's plate and screw without M4-1 (71), and s18's screw alone with it (70).
- **SG8.** **Several sockets.** Each socket of a document gets its signature; identical ones are computed once; a candidate must pass every socket's (VD2), and nothing couples two sockets' signatures. `self`-flagged requirements (M3's N2) need nothing more: M3 pins both sides.
- **SG9.** **Determinism.** Angles through `libm::atan2` of |a × b| and a · b (never `acos` near 1); every list sorted by feature id or requirement id; no hash map; every stored number finite and never −0.0.

### 5.2 Quantization (level 1 only)

- **SG10.** **Bins.** A length v is binned as `floor(v / 0.1 mm)` by one function on the same `f64` at ingest and at query. A window [lo, hi], already widened (SG6), is looked up in every bin from `bin(lo) − 1` to `bin(hi) + 1`, and every value found is compared exactly with [lo, hi]. Correctly rounded division by a fixed positive width is monotone, and so is `floor`, so lo ≤ v ≤ hi gives bin(lo) ≤ bin(v) ≤ bin(hi): no value inside is missed. The neighbouring bins defend against a store that writes values with another rounding; they cost two bins.
- **SG11.** **Policy-free bins.** The bin width does not depend on the policy, so the index is built once. A query at D9's default spans 3 bins for a pinned diameter; a clearance window [6, 6.61] spans 9.

## 6. The shortlist (SL)

### 6.1 Level 2: one socket, one candidate

- **SL1.** **Necessary only.** The shortlist keeps or drops; it never passes. A drop is a `Reject` at stage `shortlist` naming the requirements whose element or pair was not found, with the window (`RejectReason::Shortlist`, EV5). For an explicit candidate (`fitment match` with files, `judge_step`, `judge_part`) alignment and verification still run after a drop, so that every failed requirement is named; if verification then passes, the shortlist is wrong: the verdict stays `Reject` and the run exits 1 as a fitment bug (M4-17). In a catalog query (M5) a drop ends the candidate.
- **SL2.** **Domains.** For each required line, the candidate stacks that could be its witness: same sense and, over the stack's neighbourhood (the stacks of its sense whose axis lies within W_L and W_A of its own, not transitively), every span's window holds some segment diameter, the openings reach the line's count, and the extent covers the spans' length less W_L. For each required seat, the planes whose known area, with the planes within W_L and W_A of them, reaches SG6's bound. For each conical seat, the cones of SG3. An empty domain drops the candidate.
- **SL3.** **Pairs.** All pairs of required elements when there are at most 64 of a kind (all 157 measured sockets); beyond that, a spanning set: each line with the most selective line and with its neighbour in id order, each seat with the next. Any subset of pairs is sound: it only weakens the test.
- **SL4.** **Arc consistency.** Remove from each domain every candidate element with no partner in the other domain of some pair (distinct elements, the invariant within the window), until nothing changes. Partners are found through a grid over the plane normal to the axis direction class (lines) or over offsets (planes), so a revision costs O(|D_a| (log |D_b| + k)). Every comparison counts toward `MAX_SHORTLIST_CHECKS` = 10⁶; past it the candidate is kept and flagged `shortlist_budget`.
- **SL5.** **Matching.** Distinct required lines need distinct witness stacks, distinct seats distinct planes (Hopcroft–Karp on the pruned domains). Distinctness is sound because SG5 merged whatever one candidate element could serve twice.
- **SL6.** **Output.** Kept: the pruned domains, from which alignment starts (only these stacks can carry a required axis; AL2). Dropped: the first empty domain, failed pair or failed matching, as `Missing { requirements, element, window }`.

### 6.2 Level 1: the index (M5)

- **SL7.** **Tokens per candidate,** policy-free: each stack's distinct segment diameters with its sense; each ordered pair of stacks whose axes are within 0.1°, with both senses, both diameters and the distance from one axis to the other's midpoint; the number of planes; the lever; the floor. A part with more than 20,000 pair tokens is marked `pairs_incomplete` and bypasses the pair filter. Measured: 62 tokens a part over the pool, none incomplete.
- **SL8.** **Query.** Per required line, its narrowest span window, by range over the diameter bins, with multiplicity (as many distinct stacks as lines share that window); per parallel pair with the most selective line, the pair tokens in range; the plane count against the seats. Intersect; level 2 runs on the result. A socket whose signature is empty, or that has an active requirement failing every candidate (SG7), is **blocked**: a catalog query returns no candidate and says why; explicit candidates are judged anyway (M4-16). A policy with σ_ang above 0.05° bypasses the pair filter, whose tokens stop at 0.1°.
- **SL9.** **Superset.** Level 1's windows contain level 2's (with the catalog's largest lever), so whatever level 2 keeps, level 1 returns. Measured: no violation in 157 sockets; over the 90 sockets with something verifiable, level 1 returns a mean of 46.4 against level 2's 20.7 (computed from the level-1 lines of `A/out/report-vd6-off.txt`; A.md's 40.4 against 20.1 does not reproduce).

### 6.3 Cost

- **SL10.** A candidate signature is O(n log n) in its features (neighbourhoods by a sort on canonical line keys). Level 2 is O((n_r + n_c) log n_c) plus the budget. Level 1 costs the posting lists the query's bins name. Measured in section 20.2.

### 6.4 Why the shortlist never drops a passing candidate

The proof rests on six properties of verification, each stated as a rule there:

| | What verification guarantees | Rule |
| --- | --- | --- |
| (w1) | Every check accepts only within SG6's windows or narrower: an axis within ε ≤ σ of the requirement's at the stations checked, a direction within σ_ang; a diameter within ε of a pinned value, or inside (c + 2τ, d + ε) under strict; a plane within ε over the contact and within σ_ang in normal | VR3, VR4, VR7, VR10 |
| (w2) | A span's diameter is checked on a candidate segment; in a zone, at the narrower neighbouring segment for a hole, the wider for a shaft | VR7, VR8 |
| (w3) | One requirement may be served by several coaxial candidate stacks, each within ε of its axis | VR7 |
| (w4) | A seat's region is covered by the union of known-outline faces on one plane within ε, up to a band of width ε at its boundary; never by faces of unknown outline | VR10, VR11 |
| (w5) | A required opening is an `opening` end within ε of the span's end, or reached through a profile meeting the span's limits; a required wall lies within the stacks' usable profile | VR7, VR8, CF5 |
| (w6) | No undecided or partial cylinder, `other` face or cone without radii is a witness | CF4, VR18 |

Let a candidate pass alignment and verification for a socket: some proper rigid placement P positively verifies every active requirement that is verifiable under the policy. Then:

1. **Witnesses.** For each such requirement pick the candidate element it was verified on: a stack for a bore or shaft (any of several coaxial ones, w3), a plane for a seat (with the planes whose faces cover the rest of its contacts, w4), a cone for a conical seat.
2. **Nodes.** Each witness is in its domain (SL2). Its sense is the requirement's; each span's diameter was checked on a segment (w2) and accepted only inside SG3's window (w1); the openings required are at known opening ends and the wall within the extent (w5); a covered contact leaves uncovered at most the ε band, so the known area reaches SG6's bound (w4). Candidate features M2 marks unknown are never witnesses (w6), and CF4 and CF5 exclude exactly those. The checks the reviews added (VR22's core, VR7's wall openings and rim, VR8's post and zone conditions, VR4's guard) only narrow what verification accepts, so the windows still contain it.
3. **Pairs.** For any two requirements, P moves both witnesses within σ and σ_ang of their requirements (w1). A rigid motion keeps the witnesses' invariant, so it differs from the requirements' by at most W_L (2σ from the two offsets, 4Λσ_ang from the tilts over the lever) or W_A. Each witness has a partner in every pair and survives every revision of SL4; by induction, no witness is removed.
4. **Matching.** Two required lines in different SG5 groups are more than W_L apart somewhere on their spans; a stack within σ of both would put them within 2σ plus the tilt, inside W_L, so their witnesses differ. The same holds for seats. Mapping each group to any member's witness is a matching of the groups' united domains, and SL5 finds one.
5. **Budget.** Exhausting it keeps the candidate.
6. **Level 1.** Its windows contain level 2's (SL9) and its bins cannot miss (SG10); a part with incomplete pairs, or a policy beyond the pair cap, bypasses the pair filter.
7. **Several sockets.** A candidate that passes every socket passes each socket's shortlist.

So the shortlist keeps every candidate that verification passes, under every policy, for every socket. KH9 checks it on every corpus run (and SL1 on every explicit run). If verification ever widens a window, it must export the window and the signature must use it; a wider acceptance without that would make the shortlist drop good candidates.

## 7. Alignment (AL)

A placement A maps the candidate's frame into the target's: `p_target = R · p_cand + t`. Notation: a socket **line** is the axis of an active, M4-verifiable bore or shaft (VR6), one per distinct line (two requirements on one line within 1e-6 mm and 1e-6 in sine are one); a conical seat contributes its axis and apex. t is the policy's linear tolerance and s_a the sine of its angular tolerance.

- **AL1.** **One placement per socket.** A socket's requirements are stated in the target's frame (M3's R6). Every requirement of the socket is verified under one A, and the candidate passes the socket only if some single A passes them all. For M7, each occurrence k of the socket places the candidate at `T_k · A`, and the candidate's copies are checked against each other (M3's V1). Checking each requirement at its own best placement would pass a part whose holes each fit somewhere but never together.
- **AL2.** **Candidate anchors.** A socket line is matched by a candidate hole stack (for a bore) or shaft stack (for a shaft), whose axis is stated whatever its ends; a conical seat by a walked cone; a seat by a plane of the candidate's shell. Nothing else anchors: an undecided cylinder's axis may be shown for review (M2 9.6) but never places a candidate. When the shortlist kept the candidate, anchors come from its pruned domains (SL6); a dropped explicit candidate (SL1) is aligned over all its stacks.
- **AL3.** **Ranked, not filtered.** For each socket line, candidate stacks whose diameters could meet every span (a segment within ±t of each pinned diameter; in (clear, original + t) under strict) come first, the others after. A diameter never removes an anchor: a candidate whose only defect is a diameter is placed and rejected for that diameter, not for "no placement" (KH2 needs `diameter_out_of_limits`). Measured: before this rule, `nut_pinned_15tol`, `plate03_press_15tol`, `block_cb_small` and `block_no_cb` were rejected with no placement (`B/out/dcands.txt`).
- **AL4.** **Two or more lines: the anchor pair.** Pairs of socket lines are tried in order: non-parallel pairs by decreasing sine (they fix all six degrees of freedom), then parallel pairs by decreasing separation (the longest lever), up to `MAX_ANCHOR_PAIRS` = 16. The first pair that yields any placement is used; a pair no candidate pair matches gives way to the next, so a filled or moved hole that happens to anchor does not hide the others. Measured: AS1's filled bolt hole went from "no placement" to that bore's `no_feature`.
- **AL5.** **Placements from a parallel pair** (lines ℓ₁ and ℓ₂, direction d, perpendicular w from ℓ₁ to ℓ₂, separation D): every candidate pair (a, b) of anchors parallel within 2 s_a whose separation is within 2t of D. Both signs σ = ±1: R takes the candidate frame (d_a, ŵ_ab, d_a × ŵ_ab) onto (σd, ŵ, σd × ŵ), and the midpoint of the candidate pair's feet goes onto the midpoint of the socket's, so the two anchors share any error. The slide along d comes from AL7.
- **AL6.** **Placements from a non-parallel pair:** candidate pairs whose angle matches within 2 s_a (for some signs σ1, σ2) and whose common perpendicular's length matches within 2t; R takes the frame of (d_a, d_b) onto that of (σ1 d1, σ2 d2), and the midpoint of the candidate's closest points onto the socket's. No slide is left. AS1's `l_bracket` aligns this way (the rod hole across the bolt holes).
- **AL7.** **The slide along an axis direction d,** in this order: (a) the most selective seat across the axis (normal within the angular tolerance of ±d), one slide per candidate plane facing the same way, the seat with the fewest such planes chosen; (b) a conical seat on the axis: the candidate cone's apex onto the socket's; (c) neither: breakpoints, the slides that put each span end of the anchor's requirement on each end of the anchor's profile pieces, and the midpoints between consecutive breakpoints. For interval constraints these contain a passing slide whenever one exists.
- **AL8.** **One line.** For each anchor and sign, the rotation taking the candidate axis onto ±d about any perpendicular, the anchor's axis point onto the line, then turns θ about the placed axis and slides (AL7). **The turn is irrelevant** when every active M4-verifiable requirement is symmetric about the line: bores, shafts and conical seats on it; seats whose normal is along it and whose every contact outline is whole circles centred on it; grips of such seats. Then θ = 0 only. Otherwise θ comes from seats across the axis with straight edges (every pair of a target outline line edge and a candidate line edge on a plane facing the same way, loops oriented alike, material on the left), from seats along the axis (candidate planes at the same angle to the axis), and from off-axis arcs (a target arc and a candidate arc of the same radius at the same distance from the axis; designed, not prototyped). Deduplicated, in ascending order. Measured: s02's block is placed by its square's edges; s05's washers, s01's nuts and every pin are symmetric.
- **AL9.** **No line.** A conical seat fixes its axis and apex; turns as AL8. Otherwise seats: the seat with the fewest outline segments, and landmark pairs: a target vertex with its outgoing line edge against each candidate vertex with its outgoing line edge on any plane (normals together, edges together, vertex onto vertex, the offset onto the seat's), and a target arc's centre against a candidate arc's of the same radius within 2t. Pairs whose edge lengths or radii agree are tried first. Two seats with non-parallel normals fix the rotation outright (designed, not prototyped). Measured on 11 sockets: s53's flange (a disc) aligns by its arc; s55's tiled plate exhausts the bound.
- **AL10.** **Choosing the placement.** Placements are verified in their fixed order; checks run axial requirements first, and a placement that fails an axial one is not taken further during the search. The first placement passing every active M4-verifiable requirement is chosen. If none does, the one passing most is chosen, ties to the first. At the chosen placement every active requirement is evaluated in full, none skipped, for the evidence (EV4, KH2). Measured: `bracket_thin_15tol` then names its grip, its seats and its bores; without this it named only the bores.
- **AL11.** **Bounded work.** Per candidate and socket: at most `MAX_PLACEMENTS` = 4,096 placements (20,000 in the prototype), `MAX_ANCHOR_PAIRS` = 16 anchor pairs, and `MAX_COVERAGE_STEPS` = 2·10⁷ steps of coverage marching (VR11) and core face tests (VR22) together (4·10⁸ in the prototype). Past any bound the socket rejects with `alignment_exhausted` or `coverage_exhausted`: never a pass. At the measured rate (s55: 1.17·10⁸ steps in 28.9 s, about 4·10⁶ a second) the coverage bound is about 5 s; a placement failing an axial check never reaches coverage (AL10), and the axial checks of 4,096 placements cost a few ms, so a hostile candidate costs at most about 5 s per socket. Every corpus and fixture socket but s55 needs at most 98 placements (`B/out/dcands.txt`, s18's countersink cases) and 1.6·10⁶ steps; s55 exceeds every bound and is VR21's named exception. Candidate pairs for AL5 and AL6 come from an index of axis feet by direction class and separation (a bounding-volume tree as M3's N3), so the work is n log n plus the matches; the prototype enumerates pairs, which is quadratic. Measured: a plate with 169 holes at the bracket's pitches against s01 gives 1,832 placements and rejects in 45 ms once a one-pass edge test precedes the coverage march.
- **AL12.** **Determinism.** Placements are generated in a stated order (anchor pairs as AL4; candidate stacks by `FeatureId`; σ = +1 first; θ and slides ascending), with `libm` trigonometry and every stored number through `geom::finite`. The chosen placement is the same on every OS.
- **AL13.** **Placement error stays a false negative.** Whatever motion AL5 to AL9 construct, every check is made against the placed candidate's own values under that motion; a motion that is a little off can fail a requirement another motion would pass, never pass one that no motion passes. A least-squares refinement over every matched axis foot, re-verified, can recover such cases (designed, off by default; section 25.2, merge finding 1, shows where it would help).
- **AL14.** **Given placements.** `Matcher::verify_at` checks a socket at a placement the caller gives (KH8: the construction's; `--original` and VR21's seeded run: the identity), with seed `Given`. A given placement never replaces the search in a verdict; it says where a false negative comes from.

What anchors a socket, over 157 sockets (`B/out/census.txt`): one verifiable line 95, two parallel lines 20, two skew lines 5, no line (seats or a conical seat) 11, nothing M4 can verify 26. Placements generated per candidate and socket: median 8, 90th percentile 36, largest 20,000 (s55).

## 8. Verification (VR)

### 8.1 The tolerance, the floor and the edge

- **VR1.** **One check per active requirement,** with every measure evaluated and every failure coded. A requirement passes only when every measure passes (Soundness rule 1); a failure inside a requirement does not stop its other measures.
- **VR2.** **The guard band.** u = max(the target's `part_uncertainty_mm` and `vertex_offset_mm`, the candidate's `max_uncertainty_mm` and largest `max_vertex_offset_mm`, `GUARD_MM`), with `GUARD_MM` = 1e-6 mm (M2's `IDENTITY_MM`), and ε = t − u. If ε ≤ 0 the requirement fails with `floor_above_tolerance` and both values: the two parts' stated geometry is not consistent to within the tolerance. Angular comparisons keep a guard of 1e-9 rad.
- **VR3.** **Equal to the original.** A value the socket states from the target's own geometry (a pinned diameter, an axis, a plane's offset, a grip, an apex, an included angle, a strict upper bound) passes when `|found − original| < ε`, strictly; at ε it rejects (Soundness rule 4). Positions are compared where the requirement acts: an axis by its distance from the socket's at both extreme stations of its spans; a plane over every vertex of the contact regions and over each arc (its centre's distance plus the radius times the in-plane slope); the angle also within the angular tolerance. The lever is thereby counted: 0.01° over 100 mm is 0.017 mm, beyond the linear tolerance.
- **VR4.** **Limits from neighbours.** A clear diameter (a bore's least, a shaft's most) is derived from positions, known to the floor of the relations behind it. A clearance span passes only when the radial margin exceeds τ + u, with τ = max(`contact_mm`, the span's floor, u): for a bore `r_min − δ − c/2 > τ + u`, for a shaft `c/2 − r_max − δ > τ + u`, with δ the candidate axis's largest offset over the span and the span's floor the largest `floor_mm` of the sources whose `span_mm` meets it. This is the socket's own rule B1 (clearance needs m > τ) applied to the candidate, with VR2's guard u so that a candidate exactly at c + 2τ rejects in every frame (RS6). M3's B1 takes the same guard in the socket PR (SR6: a margin within τ + the target's own u, at least `GUARD_MM`, is line to line), so no original is a clearance whose margin lies within the guard. The limit is never widened by t. If the limits leave the original no room (c + 2(τ + u) ≥ d for a bore, or d ≥ c − 2(τ + u) for a shaft, which happens when a span's floor comes from another source than its clear diameter's), the requirement fails with `floor_above_tolerance` rather than a diameter code (VD6), so Y4 stays exact.
- **VR5.** **What the floor is for.** A requirement's `floor_mm` includes the placement path and the neighbours (M3's Z2). It bounds how well the socket knows where the neighbours are, which matters only where the socket licenses a departure from the original (VR4). Where the candidate must equal the original (VR3), the socket's value is the target's own stated geometry, known to the target's own uncertainty, which u holds. So `floor_mm` never fails a VR3 comparison by itself.

### 8.2 Axial requirements

- **VR6.** **What M4 can verify.** A requirement is M4-verifiable exactly when `Requirement::verifiable()` holds. The socket PR (M4-1, SR5) makes that one predicate cover what M4 needs beyond M3's Y4 list: a bore or shaft with a span piece past a known end that is neither pinned nor bounded by a clear diameter (s18's screw, s58's plate, vaccase's tee); a zone piece without the original's profile; a bore whose original's own annular post reaches into a span (a sleeve over a post: the original's own core is not clear, VR22); a grip either of whose seats is unverifiable (AS1-ec ×2, AS1-tc ×2: a grip between seats of unknown outline cannot be located); and a conical seat whose sense is not stated. A zone piece with its profile is verifiable (VR7). M3's Y4 witness test calls the same predicate, so the two cannot disagree. If M4-1 is declined, M4 adds these conditions itself, a zone piece with no clear diameter included, with reasons `zone_not_read`, `grip_seat_unverifiable`, `own_post` and `cone_sense_unknown`, and M3's witness test must call M4's function. Measured before M4-1: 8 sockets have such requirements (s18 ×2, s58, vaccase's tee, AS1-ec ×2, AS1-tc ×2; `B/out/census.txt`); measured over the 110 targets, one original has its own post in a span: s50 CAP's skirt bore, the spigot rising inside it (`review-soundness/u11.txt`); with U11 extended to any own material in a core and a shaft's core unbounded, s50 CAP's spigot and s54's two pins join it (`merge/y4probe/y4-vr22.txt`).
- **VR7.** **Bore.** Passes when one candidate hole stack (never an undecided or partial cylinder) satisfies all of:
  - **on the axis:** sine to the bore's axis below s_a, and distance below ε at both extreme stations of the spans (at the axis point when there are none);
  - **each span** [from, to] as the socket states it (with SR7, an interior span end comes widened by its relation's τ, clipped to the target's own wall), mapped to the candidate's stations through the placed axis: the candidate's contiguous usable profile (VR8) reaches both ends with a shortfall below ε (`wall_missing` otherwise; a blind candidate short of a full-diameter depth fails here);
    - pinned: one cylinder piece (not a zone) covers the span to within ε at both ends, its diameter within ε of the pinned one (`diameter_out_of_limits`);
    - clearance: VR4 on the smallest radius met over the span (`diameter_out_of_limits:min`); under D6 strict also every radius met below the original's diameter + ε (`:max`), zones counting at their owner segment's radius;
    - a zone piece with no pin and no clear diameter (M4-1): the candidate's radius at every station of the piece within ε of the original's profile there, as M3's R12 pins a hole whose fastener was not read (`:profile`);
    - a piece past a known end: unverifiable (VR6);
  - **openings:** at each end the bore requires open, the candidate has an `opening` facing the same way along the axis, and either that opening (its wall's end, or its rim through an end zone VR8 took) lies within ε of the span's end on that side (the original's rim, since M3's B6 spans reach the rim), or (branch 2) the profile out to it is air for the shaft that runs past the rim. Branch 2 is available only when the end span states a clear diameter c (the protruding shaft's, which the socket states for clearance spans and for pinned interference and line-to-line spans whose shaft was read; s01's nut: c = 6.0): the extension from the span's end out to the candidate's opening is then a span of its own for this check, for CF8's wall openings and for VR22's core, with the clearance core of c (ρ = c/2 + τ + u), and its profile must clear c by VR4's margin all the way, through zones VR8 took; never the pinned diameter, which would continue an interference past the original's engagement (RS14). With no clear diameter on the end span (a pinned `not_read` span), the opening must lie within ε of the span's end. Otherwise `end_not_open`: a stack that narrows at the original's rim and opens further on fails (RS5), and so does one that continues the pinned bore past the rim (`nut_long`) or carries a bar across the extension (`nut_bar`; RS13). An unknown or irregular end is not open (CF5);
  - **wall openings:** CF8: a span widened by ε that meets a wall opening fails (`wall_opening`) unless every face sharing the opening's edges is a hole-sense cylinder or cone of whole sweep (a cross hole);
  - **the core:** VR22 (`core_intruded`);
  - **depth:** a known depth is required only through the spans (M3's B5); `unknown_end` asks nothing more.
  The flag `thread_not_confirmed` rides along (VR19).
- **VR8.** **The candidate's usable profile** (M3's X7 on the candidate side). Along a stack's own stations: each segment's cylinder over its stations (a `None` there ends the profile on that side: nothing past it is known). Each zone, a transition between two segments or an end zone from the wall's end to an `opening` or `free_end` rim, counts at the narrower (hole) or wider (shaft) neighbouring segment's radius, and is taken only when every face the walk crossed there stays outside the hole's cylinder at that radius (inside the shaft's): each cone and torus from its own radii and stations, each plane annulus from its radii, face by face. A walk that runs out along an annulus and back in along a narrowing cone, or crosses annulus, cone and annulus and dips below both segments, takes no zone (RS3). A zone not taken ends the profile at the wall's end on that side. A `bottom` or `root` ends the profile at the wall's end. An `annular` bottom (a post rising from the floor: a coaxial cylinder of the opposite sense, M2's W5) bounds the usable profile at the post's free end, the far station of the shaft stack whose root rises from that floor; if that end is unknown, nothing on that side is usable. The mirror holds for a shaft whose root is reached through a post (a hole-sense cylinder round it); a sleeve or skirt round a shaft lies in VR22's unbounded core anyway, and where the original has one, U11 makes that shaft's requirement unverifiable (RS19). An unknown or irregular end ends the profile at its last known station (CF5).
- **VR9.** **Shaft.** The mirror of VR7 on a candidate shaft stack: on the axis; each span covered (engagement, M3's I3); pinned: one cylinder within ε of the pinned diameter; clearance: VR4 on the largest radius met; under strict, every radius met above the original's − ε; the core outside the shaft clear (VR22). `max_extra_mm` is not checked (M4-11): M3 records it for information, the evidence lists it as not checked, and a shaft that bottoms out is M7's collision.

- **VR22.** **The core is clear** (`core_intruded`; the candidate side of M3's B8). For each span of a bore or shaft, at the chosen placement:
  - **The core.** For a bore, the solid cylinder about the requirement's axis of radius ρ = c/2 + τ + u for a clearance span (the clear disc the occupant needs, with VR4's margin), or ρ = d/2 − τ for a pinned span or a zone piece (the original's own radius, less the contact band). For a shaft, everything outside radius ρ = c/2 − τ − u (clearance) or d/2 + τ (pinned), unbounded radially over the span's stations, where the neighbour's hole and material lie (RS19: no free radius bounds it). Axially, the span widened by ε at both ends, [from − ε, to + ε], except where the candidate stack's own `root` or `bottom` lies within ε of the span's end: there the core stops at that end's station, exclusive, so the face the shaft rises from or the floor that closes the hole is not in it; a post, spike or dome rising from that face still meets the core by its own faces (RS16, RS22). Where the original's own material lies in a span's core (a skirt round a spigot, a second pin beside the first, a post in a bore), the requirement is unverifiable by M3's U11 (SR5), so the original is never asked to clear its own material.
  - **Exempt faces.** The faces VR8's usable profile used: the stack's segment cylinders, and the cones, tori and plane annuli of the transitions and end zones VR8 took. And, for a bore, the hole-sense cylinders and cones of whole sweep that share the edges of a wall opening VR7 accepted (a cross hole: a tube of air whose material lies outside it). No other face is exempt, whatever its kind, sense or outline.
  - **The test.** Every other candidate face fails the span when it may meet the core. A plane face is tested by the exact distance of its region (its lines and arcs, its holes included) from the core's section on its plane: a disc for a plane square to the axis, a band for one along it, otherwise the ellipse the cylinder cuts. A cylinder, cone or torus coaxial with the axis is tested by its radius over its stations. Any other face, and any face of unknown outline, is tested by its sound box in the axis frame (the box's least distance from the axis against ρ, its stations against the core's). A bore's face meets when its distance from the axis is at most ρ, a shaft's when it reaches ρ or beyond: the edge rejects. The failure names the face and its distance (measure `core_distance_mm`).
  - **Work.** The candidate's faces sit in a bounding-volume tree in its own frame, built once per candidate as M3's `index.rs` builds one per definition. A span queries it with the core's box mapped back through the placement, and tests only the faces returned; each test counts toward `MAX_COVERAGE_STEPS` (AL11). The work is n log n plus the faces near the bore, and bounded.

  By the rule (derived by hand from the probes' features; the soundness review's round 2 agrees): `housing_post` (a shaft-sense cylinder at r 2, inside ρ = 6.01), `base_post`, `housing_spike` (a cone down to r 1.5 at the span's start), `housing_dome` (a sphere, an `other` face, by its box), `housing_bar` (a shaft-sense cylinder across the bore) and `housing_lip` (a cone narrowing to r 5) each meet the core. `housing_spike` is the case only VR22 rejects: CF5 keeps its wall to station 8, so VR22 is `CAN_FAIL`'s case for it. `nut_ext` is not a VR22 case: its step annulus is a transition VR8 takes, so VR22 exempts it, and only VR7's rim test rejects it (RS18). `housing_cross`'s two cylinder pieces are exempt, hole-sense of whole sweep at a wall opening, and it passes; their sound boxes alone would not do (the boxes reach r 5.97, inside ρ).

  **Y4 under VR22, measured** (`m4-evidence/merge/y4probe/`, extended from the review's `u11` probe; output `y4-vr22.txt`): every original among the 110 targets, at the identity, against every active bore and shaft requirement M3 calls verifiable, with a shaft's core unbounded and U11 as extended. 103 targets state less than t; the other 7 fail every requirement by the floor anyway (VD6). Their 172 verifiable bore and shaft requirements have 213 spans. 3 requirements are set aside before the core test: s18's screw and s58's plate (U9: a span past a known end) and s18's plate (its countersink zone span, verifiable only with SR1's profile, which today's socket does not state; the probe cannot tell the two kinds of piece apart). U11 as extended makes 4 more unverifiable, each because the original's own material lies in the core: s50 CAP's skirt bore (its spigot rises inside it), s50 CAP's spigot (its skirt surrounds it), and s54's BLOCK's two pins (each lies in the other's core). The other 209 spans all have a clear core: no face of the original but its own walls, transitions and cross holes lies in it, with exact distances for planes square to the axis and for coaxial cylinders, cones and tori, and sound boxes otherwise (no `other` face was needed). Without the stop at the stack's own root or bottom (RS16), 3 more shafts would be unverifiable needlessly, s01's and s05's bolts and s19's cover (`y4-vr22-without-n4a.txt`). The round-2 runs, with the free radius since withdrawn, are kept as `y4-vr22-round2*.txt`. The review's round 3 re-ran the probe with no transition and no cross hole exempt: the transitions it exempts are all ones VR8 takes, and the cross-hole exemption is unused (`review-soundness/y4probe-check/`). The probe tests VR22 and VR4's guard only (no original's clearance margin lies within τ + u); the full VR21 run, every rule at once, is PR 3's.

### 8.3 Seats, grips and cones

- **VR10.** **Seat.** Passes when one plane of the candidate's shell, placed, faces the seat's normal within the angular tolerance, lies within ε of the seat's plane over every contact region (VR3), and its faces of known outline together cover each contact's region within ε (VR11). The region is the target face (`basis: target_face`, M3's R14); a candidate face of unknown outline covers nothing (`unknown_outline`). Codes: `no_feature` (no plane), `contact_not_covered` (with the first uncovered point, in the seat's plane frame). Under D6 functional the region is not shrunk (M4-7).
- **VR11.** **Coverage, certified.** R is the target face's region (outer loop counterclockwise about the seat normal, holes clockwise, material on the left), U the union of the candidate's faces on the matched plane, projected exactly (arcs stay arcs). With f(q) the signed depth of q in R (distance to R's boundary, positive inside), which is 1-Lipschitz along any curve:
  1. **No candidate edge deep inside.** For every edge of U, march from x with step ε/2 − f(x): if f ≥ ε/2 anywhere, fail; if the step falls below ε·10⁻⁴, fail as "at the edge". An edge is skipped only when both faces sharing it are in U (known outlines, on the matched plane); an edge between a face in U and one that is not, such as a coplanar face of unknown outline, is a boundary of U and is marched (RS4). A first pass tests each edge's start, which ends most wrong placements at once.
  2. **The ε-offset curves are covered.** For each edge of R, its curve at distance ε on the material side (a shifted line, a concentric arc), and a circle of radius ε about each vertex: march with step |f − ε/2|; at every point with f > ε/2, the point must be in U (by winding number; a whole circle winds once from inside).

  Step 1 makes membership in U constant on each component of {f > ε/2}. Any point at depth ≥ ε has a nearest boundary point y, and y + ε·n(y) lies on a curve of step 2, at depth ε, in the same component; so every point of R deeper than ε lies in U, and what is uncovered lies within ε of R's boundary. (A component of {f > ε/2} with no point of depth ε, such as a narrow lobe, may be uncovered to a depth below ε: within the tolerance, so not a false positive; RS11.) The cost is proportional to perimeter over ε (s01's bracket: 3·10⁵ steps, 75 ms; at t = 0.002 mm the corpus run takes 4.4 times as long).
- **VR12.** **Grip.** Both seats' planes are found as in VR10 (re-found if a seat was removed in review: the grip still names them), and the candidate's separation (o₁ + o₂ of the placed planes; for a conical side, the apex's distance from the plane, M3's L2) is within ε of the target's (`grip_length`, or `seat_missing`). M4 checks every grip this way: D7 is `hard` only (M4-8). D7 `warning` is deferred to M7. A warning on the grip check alone would change nothing, since the two seats already fix the separation within 2ε; and letting the far seat float, which made the warning mean something, needs every neighbour that reseats with it to be found and checked at its new place, which the socket's target-to-neighbour relations cannot do (a washer under a head chains the reseating; section 28.4, RS20 and RS21). M7's body-fit verifier places the neighbours themselves and can.
- **VR13.** **Conical seat.** Passes when a walked cone of the candidate has its placed axis along the requirement's (same widening direction, within the angular tolerance), its apex within ε, its included angle within the angular tolerance less the guard, its radial range covering `radii_mm` to within ε at both ends, and the target's sense. Codes: `cone_mismatch:apex`, `:angle`, `:range`, `:sense`, or `no_feature`. The sense is the socket's (SR2, M4-1), and `radii_mm` the union of the ranges of every neighbour cone seated on the target cone (SR8); without M4-1 it is inferred: hole if a bore of the socket is on the cone's axis, shaft if a shaft is, else the seat is unverifiable (`cone_sense_unknown`). Every conical seat in the corpus has a bore or shaft on its axis.

### 8.4 The rest of a socket

- **VR14.** **Unverifiable kinds** (`unresolved`, `partial_bore`, `partial_shaft`, and every requirement VR6 excludes) fail with `unverifiable` and the socket's reason, for every candidate (VD5). They do not stop the run: every other requirement is still placed and checked.
- **VR15.** **Left out:** nothing is checked. A candidate may have a hole there, or not, or material; a collision there is M7's.
- **VR16.** **Review:** a removed requirement is not checked, not given to `decide`, and listed with its note; an unreviewed one is checked as kept. A grip whose seat was removed is still checked (its seats' planes are in the document).
- **VR17.** **Several sockets.** The candidate replaces every occurrence, so it must pass every socket, each under its own placement (M4-12). A `self` requirement is checked against the original's values like any other; the socket pins B and I relations between selected occurrences on both sides (M3's N2), so two copies that each equal the original within ε meet as the originals did within 2ε. Their body clash is M7's (V1).
- **VR18.** **Candidate unknowns never satisfy:** an undecided or partial cylinder, an unknown or irregular end (nothing past its last known station, CF5), an unknown outline, an `other` face (CF4). None of them is assumed clear of a required core either (VR22).
- **VR19.** **Threads.** A passed requirement flagged `thread_not_confirmed` keeps the flag on the verdict; no M4 rule confirms a thread (section 14).
- **VR20.** **The measures recorded** are the comparisons above (EV2): `found` the candidate's value, `nominal` the socket's, `min` and `max` the effective limits VR3 and VR4 compute (pinned d: (d − ε, d + ε); clearance: a radial margin above τ + u, and under strict a diameter below d + ε; a plane: offset ± ε; the core: a distance beyond ρ), `margin` the distance to the nearer limit; `passed ⇔ margin > 0`.

### 8.5 Failure codes

One closed list; a machine name never changes meaning.

| Code | Meaning |
| --- | --- |
| `unverifiable` | VR14, VD5; with the socket's reason (its flag or kind), or, without M4-1, `zone_not_read`, `grip_seat_unverifiable`, `own_post`, `cone_sense_unknown` |
| `floor_above_tolerance` | VR2's ε ≤ 0, or VR4's limits leave no room; both values stated (VD6) |
| `no_placement` | no anchor matched; the requirements alignment could not anchor are named (KH3) |
| `alignment_exhausted`, `coverage_exhausted` | a work bound reached (AL11) |
| `no_feature` | no candidate feature on the axis or plane; the nearest named with its offset |
| `axis_offset`, `axis_angle` | a feature found, off the axis beyond tolerance |
| `diameter_out_of_limits` (`:min`, `:max`, `:profile`) | a pinned diameter, a clear margin, a strict maximum, a zone profile |
| `wall_missing` | no candidate wall over part of a span, as an interval of the requirement's stations |
| `end_not_open` | an end the socket requires open is closed or unknown, or its opening is neither within ε of the span's end nor reached through a profile meeting the span's limits (VR7) |
| `wall_opening` | a required span meets a wall opening whose faces are not all hole-sense cylinders or cones of whole sweep (CF8) |
| `core_intruded` | a candidate face other than the exempt ones may meet a required span's core (VR22); the face and its distance named |
| `contact_not_covered` | the candidate's material does not cover a seat contact's region; the first uncovered point named |
| `unknown_outline` | the candidate face there has an unknown outline, so it covers nothing |
| `candidate_feature_unknown` | the only candidate feature there is undecided, partial or past an unknown end |
| `seat_missing`, `grip_length` | a grip's seat has no plane; the seats' separation differs |
| `cone_mismatch` (`:apex`, `:angle`, `:range`, `:sense`) | a conical seat's apex, angle, radial range or sense |

D's `floor_exceeds_tolerance`, `angle_out_of_limits` and a cone's `contact_not_covered` are `floor_above_tolerance`, `cone_mismatch:angle` and `cone_mismatch:range`. C's `shaft_too_long` is dropped (M4-11).

### 8.6 Why a pass is a fit, check by check

Each argument assumes the socket is complete (M3's job, checked by its oracle) and that every check used the one placement of its socket (AL1).

| Check | Why a pass is sound | False negatives accepted |
| --- | --- | --- |
| Axis (VR7, VR9) | The candidate's axis is within ε of the original's over the span, so every neighbour on it meets the candidate's wall where it met the original's, up to ε | A hole off by more than ε that its bolt still clears (`fn_position`) |
| Pinned span | One cylinder of the original's diameter within ε over the whole span: a thread or press fit engages as the original did; a cylinder, not a zone, so no chamfer takes engagement away | A thread longer elsewhere; a fit read line to line that is really clearance (AS1 pins every hole) |
| Clearance span | Smallest radius minus offset beyond the clear radius by more than τ + u: the occupant, contained in the clear disc about the target axis (M3's X4), passes with the margin B1 asked of the original; zones count at the narrower segment only where every face crossed lies outside that cylinder (VR8) | A hole the occupant clears by less than τ + u; clear + 0.01 |
| The core (VR22) | No face but the stack's own walls and cross holes lies within the clear disc (or the pinned radius) over the span widened by ε: the walls alone say nothing about a post, a dome, a bar or a lip inside them | A candidate face whose sound box reaches the core while the face does not (an `other` face near the bore) |
| Zone piece (M4-1) | The candidate's profile within ε of the original's where the socket did not read what occupies it | Any larger countersink or chamfer that a head would still clear |
| Wall over the span | A usable wall is there wherever the occupant is (B5), up to a post's free end or an unknown end's last known station (VR8, CF5), and the core is clear (VR22) | A thinner part whose bolt would still pass (D7 hard) |
| Openings | An `opening` within ε of the original's rim, or an extension out to it that is air for the protruding shaft (it clears the end span's clear diameter, and CF8 and VR22 test it as a span), means air where the original had air past its rim (M2's W3), so a shaft that ran past the rim leaves the hole there | Material of a separate part of the candidate beyond a real opening at the rim (a U-channel's far leg) is not seen: M7's (section 25.1, row 18) |
| Wall openings | A span crosses only openings whose faces are air on the core's side (cross holes) | A wall opening whose faces are not all whole hole-sense surfaces, though air |
| Seat | A plane at the original's within ε, whose material covers the whole target face to within ε; the true contact lies inside the target face (M3's C4), so the neighbour bears on the candidate wherever it bore on the original | A smaller flange that still covers the true contact (R14); a face split into regions of unknown outline |
| Coverage (VR11) | Proven from the Lipschitz bound, never sampled | A candidate edge between ε/2 and ε inside the region |
| Grip | Separation within ε | A part of another thickness whose fasteners would still reach (D7 is hard only in M4) |
| Conical seat | Same cone, apex and angle, covering the range on a whole revolution (walked cones only), same sense | A cone the walk did not cross (a partial cone) |
| Unverifiable | Never passes | Everything behind it until review |
| One placement per socket | All checks share A, so their conclusions hold together | Placements the construction misses (AL13; s55 unseeded) |
| Several sockets | Each occurrence's socket passes at its placement; `self` relations are pinned to the original on both sides | Copies that would fit each other but differ from the original |

What M4 does not prove, and says: the body (fit `not_checked`), threads (`thread_not_confirmed`), materials (`material_not_compared`), `max_extra_mm`, and every requirement removed in review.

## 9. Verdicts (VD)

### 9.1 The constructor and the kinds

- **VD1.** **Three constructors.** A socket's `Verdict` is built only by `decide`, which keeps its M0 contract: the socket's active requirement ids, exactly one check per id, and the socket's `FitStatus`. A candidate's verdict is built only by `combine`, from its socket verdicts (zero sockets: `no_sockets`), or by `Verdict::refused(codes)` for a candidate M2's gate refused (VD10), crate-visible and called only by `Matcher::judge_step`; `decide` with no requirements gives `no_requirements`, never `candidate_refused`. `Verdict` stays `Serialize` only: nothing deserializes into a verdict.
- **VD2.** **Kinds (D1).** Per socket: `Match` when the interface passes and the fit is `verified`; `InterfaceMatch` when the interface passes and the fit is `not_checked` or `not_verified`; `Reject` when the interface fails or the fit `interferes`. Per candidate: the worst of its sockets (`Reject` over `InterfaceMatch` over `Match`), because one candidate replaces the part at every occurrence (PLAN, Choosing the part to replace). A document with no socket rejects with `no_sockets`: the worst over an empty set must not default to a pass.
- **VD3.** **The interface passes** only when the socket has at least one active requirement, every active requirement has exactly one check, and that check passed (M0's rules: missing, repeated and unknown checks reject). Active means not removed in review.
- **VD4.** **The fit field** keeps D1's four states. Per socket, the verifier's answer; per candidate, `interferes` if any socket interferes (with every interference), else `verified` only if every socket is verified (with the kernel and version, which must agree), else `not_verified` if any is (with every reason), else `not_checked`. M4 has no verifier: every fit is `not_checked` with `NO_FIT_VERIFIER`, and the CLI prints `InterfaceMatch: interface verified; body fit not checked (no fit verifier in this build)`. A socket whose interface failed gets no fit check: `not_checked`, reason `interface rejected` (`INTERFACE_REJECTED`, M4-26); a refused candidate's verdict has fit `not_checked`, reason `candidate refused` (`CANDIDATE_REFUSED`, VD10). The verifier receives a `FitRequest` per socket (section 12).

### 9.2 What each requirement does to a verdict

| Requirement state | Checked? | Effect on the verdict | Evidence |
| --- | --- | --- | --- |
| Active, M4-verifiable, positively verified | yes | none | `passed`, measures |
| Active, M4-verifiable, not verified | yes | `Reject` | `failed`, measures and every failure |
| Active, unverifiable (VR6) | no geometry can pass it | `Reject` (VD5) | `failed`, `unverifiable`, the reason |
| Active, unverifiable under the policy | no | `Reject` (VD6) | `failed`, `floor_above_tolerance`, both values |
| Removed in review | no | none, but listed | `not_checked`, the note (VD7) |
| Unreviewed | yes, as kept | none; counted | `review: unreviewed` (VD8) |
| Left out by the socket | no | none | listed under the socket |
| `max_extra_mm` of a shaft | no | none | listed as not checked (M4-11) |

- **VD5.** **Unverifiable requirements fail.** An active requirement that is not M4-verifiable (VR6: unresolved, partial bore or shaft, a bore or shaft flagged `target_undecided` or `open_at_unknown_end`, a seat with a contact of unknown basis, a conical seat of unknown range, and with M4-1 the cases VR6 adds) gets a failed check with code `unverifiable` and its reason, for every candidate, the original included. It does not stop the run: every other requirement is still aligned, checked and reported.
- **VD6.** **Unverifiable under the policy.** An active requirement fails with `floor_above_tolerance`, both values stated, when t − u ≤ 0 with u the two parts' own uncertainty (VR2), or when a limit derived from neighbours leaves the original no room (VR4). A requirement's `floor_mm` alone never does (VR5). The original's expected failures (Y4) are VD5's and VD6's together. `Requirement::verifiable()` is the socket's fact; the floor is M4's, and depends on the candidate: the gate's warnings (MF10) and the signature (SG7) use the target's u alone, and a candidate stating a larger uncertainty can fail more.
- **VD7.** **Removed requirements** are not given to `decide`; each is recorded as `not_checked` with its note, per socket. A socket with every requirement removed has none active and rejects (`no_requirements`, M3 9.4).
- **VD8.** **Unreviewed requirements** are checked as if kept, and the document counts them per socket. `--require-reviewed` refuses the socket document before any candidate (MF2). A requirement that is unverifiable and `kept` is the user's explicit choice: every candidate fails it, and stderr says so once.
- **VD9.** **A stale socket is refused input.** `is_current() == false` refuses the document with "extracted by extractor 5 and socket rules 4; this build is 6 and 6: extract it again (`fitment socket … -o FILE` carries decisions over)". No candidate is read; no flag overrides it.
- **VD10.** **A refused candidate** (M2's gate) is a `Reject` with reason `candidate_refused` and its refusal codes; no socket is checked. An `internal` refusal also makes the run exit 1, as `features` does.
- **VD11.** **Candidate unknowns never satisfy** (M2 9.6, 7.4): an undecided or partial cylinder and a face of unknown outline are never the feature a check passes on; a stack with an unknown or irregular end satisfies nothing past its last known station, and that end is never an opening or a bottom (CF5); none of them is assumed clear of a required core (VR22). The verdict carries `candidate_incomplete` when the candidate's `complete` is false, for information.

### 9.3 Flags and warnings

- **VD12.** **Flags ride along on every verdict,** sorted, from a closed list whose machine names never change meaning:
  - `thread_not_confirmed`: some passed requirement carries the socket flag (a pinned line-to-line or interference fit). No M4 rule confirms a thread;
  - `material_not_compared`: on every verdict (section 14);
  - `candidate_incomplete` (VD11);
  - `requirements_removed`, `requirements_unreviewed`;
  - `larger_than_original`: under D6 functional, some clearance span passed with a candidate hole wider than the original's (or a shaft narrower), which strict would have rejected.
  `grip_differs` is reserved for M7's D7 warning (VD13) and never set in M4.
  Soundness rule 5 is met by the flags with the fit field: the verdict states what was not checked.
- **VD13.** **D7 as a warning: deferred to M7.** M4 offers D7 `hard` only (M4-8, VR12): a grip is a requirement like any other, and no M4 verdict carries a warning. The candidate entry keeps an empty `warnings` list, so `fitment.match` v1 already has the field when M7 adds the warning `grip_differs`.

### 9.4 Exit codes

- **VD14.** `fitment match` exits with one code for the run:

| Code | Meaning | Precedence |
| --- | --- | --- |
| 2 | Usage: bad arguments, no candidate given, an invalid tolerance | first, before anything is read |
| 2 | Usage: `--original` naming another file than the socket's source (its SHA-256 against the socket's `source`) | after the socket gate, before any candidate |
| 3 | Refused input: the socket does not read back (`SocketFormatError`), is refused, is stale (VD9), or has unreviewed requirements under `--require-reviewed` | before any candidate is read; nothing on stdout |
| 1 | Error: a candidate or the socket unreadable, the output unwritable, an `internal` refusal, the original failing its own socket beyond VD5 and VD6 (VD15), a verification pass after a shortlist drop (SL1) | over every verdict: an incomplete run never claims a result |
| 0 | At least one candidate is `Match` | |
| 4 | No `Match`, at least one `InterfaceMatch`; 5 under `--require-fit` | |
| 5 | Every candidate rejected (refused candidates included) | |

A closed stdout (`| head`) ends output quietly and keeps the verdict's code.

- **VD15.** **The original's self-check.** With `--original ASM`, the target part read from the socket's own assembly is judged as a candidate labelled `original`. It must fail exactly its VD5 and VD6 requirements (VR21); if it fails any other, or fails with no code, or the shortlist drops it while it passes (SL1), stderr says "the original fails its own socket at socket-1/bore:hole-88: this is a fitment bug" and the run exits 1. A requirement it fails only by AL11's bounds is noted on stderr, as VR21 names s55. The original is a check of fitment, not a candidate: the document holds it as `original`, beside `candidates`, and neither the summary nor the exit code counts it, so a run whose candidates all reject exits 5 however the original fares.

## 10. Evidence (EV)

### 10.1 What one check records

- **EV1.** **One record per (socket, requirement),** in the socket's requirement order, removed ones included, for every socket that found a placement: `requirement`, `review` (with the note when removed), `result` (`passed`, `failed`, `not_checked`), `feature` (the candidate feature or features it passed or failed on, `+`-joined, or null), `measures`, `failures`.
- **EV2.** **A measure** is one comparison: `quantity` from a closed list (`axis_offset_mm`, `axis_sine`, `diameter_mm`, `clear_margin_mm`, `profile_offset_mm`, `core_distance_mm`, `wall_from_mm`, `wall_to_mm`, `plane_offset_mm`, `plane_sine`, `grip_length_mm`, `apex_offset_mm`, `cone_angle_rad`, `cone_range_mm`, `covers`, `open_start`, `open_finish`, `wall_opening`); where it applies (`span_mm`, `segment`, `contact_face`, `end`, `at_mm`); `found`; `nominal` (the socket's value: the original's, or the clear diameter for `clear_margin_mm`); `band` (`epsilon`, `tau`, `tau_guard` or `angle`) with `band_mm` or, for an angle, `band_rad`, the band the rule used (VR3's ε, VR4's τ + u, the angular tolerance less its guard); `min` and `max`, the effective limits (null when unbounded); `margin`, the distance from `found` to the nearer limit, positive inside; and `passed`, which is exactly `margin > 0`. An edge rejects: `margin == 0` fails. A non-numeric measure (`covers`, `open_*`) states `found`, `required` and `passed`.
- **EV3.** **A failure** is `{code, at, detail}`: a code from section 8.5, where it applies, and a sentence for people built from the values with Rust's shortest round-trip formatting. A failed check lists every failure it found, and a rejected candidate every failed check, not the first.

### 10.2 Placement and stage

- **EV4.** **Placement,** per socket: the rigid transform from the candidate's frame to the target's (`rotation`, `translation_mm`, as `Rigid` serializes), `seeded_by` (the seed: the requirement–feature pairs that fixed it and the seat that fixed the slide, or `given` for a supplied placement), `tried` (placements generated), `chosen` (`first_passing` for a pass; `fewest_failures` for a reject, stated as "the best placement found among `tried`"), and the socket's `u_mm` and `epsilon_mm` (VR2).
- **EV5.** **Stage,** per socket: the first stage that rejected, `shortlist` (with `Missing` entries in the socket's terms: requirement ids, the element and the window), `alignment` (no placement: the requirements no candidate feature could anchor, each failed with `no_placement`, the rest `not_checked`), or `verification`; a socket that passed says `verification`. An explicit candidate dropped by the shortlist still carries the checks of its placement (M4-17); a catalog run's drop carries none. A candidate's stage is its least advanced socket's.
- **EV6.** **Policy and versions** are recorded once at the top of the document (MF6), and per candidate the facts of the read that judged it: `extractor_version`, `complete`, `max_uncertainty_mm`, `max_vertex_offset_mm`.

### 10.3 Evidence proves the verdict

- **EV7.** **The verdict follows from the evidence alone.** A test, run on every match document the corpus, the fixtures and the unit set produce, recomputes from the JSON: each measure's `passed` from `found`, `min` and `max`; each check's result from its measures and failures (a passed check has no failure and only passed measures; a failed one at least one failure); each socket's interface verdict from its checks and fit (`decide`'s rules, removed ones aside); each candidate's verdict, flags and order from its sockets. A disagreement fails. This catches a check that reports a measure it did not apply, and a verdict assembled from the wrong records.
- **EV8.** **Determinism.** The same socket bytes, candidate bytes, policy and options give the same document bytes on Linux, macOS and Windows, labels aside: every list in a stated order; every number through `geom::finite` (no NaN, no −0.0); details from values only; nothing from the environment but labels. CI's three-OS goldens compare whole documents; the determinism test compares `digest`.
- **EV9.** **Size bounds.** A candidate's evidence is linear in its socket document: one record per requirement, each bounded by its spans, contacts and segments. A non-rejected candidate carries every measure; a rejected one carries its failed records in full and its passed records as `{requirement, result, feature}` (M4-19); a candidate with no placement, one record per requirement without measures.

## 11. The `fitment.match` document (MF)

### 11.1 Inputs and the socket gate

- **MF1.** **Inputs.** `fitment match <SOCKET> <CANDIDATE>...`: a socket document and one or more candidates, each a STEP file or a directory whose files ending in `.stp`, `.step` or `.p21` (any case) are each a candidate, not recursively. A catalog (M5) gives candidates by its ids instead. At least one candidate is required.
- **MF2.** **The socket gate** runs before any candidate is read, in this order: the file reads (else exit 1); `SocketDocument::from_json` accepts it (else exit 3 with its error); its status is accepted (else exit 3, with its refusals); `is_current()` (VD9); under `--require-reviewed`, no unreviewed requirement in any socket (else exit 3, with the count and the first ids). An accepted document with an empty socket, or with requirements that fail every candidate, passes the gate and warns.
- **MF3.** **Candidates are read one at a time,** their bytes handed to the core and dropped, so memory is one STEP file's (stepq's graph is about ten times the file). A file that cannot be read is reported on stderr and makes the run exit 1; the others are judged and the document written. Candidates with the same SHA-256 are judged once and listed once, with every label.
- **MF4.** **Labels** are what the caller named the candidate by: the argument as given, or `dir/name` for a file found in a directory argument; the catalog's id in M5. They are kept in order of appearance, escaped by `printable()` wherever printed, never used for order, and left out of the digest.
- **MF5.** **Order.** Candidates by verdict (`match`, `interface_match`, `reject`), then stage reached (`verification`, `alignment`, `shortlist`, `refused`), then the number of failed checks, then SHA-256. Sockets in the socket document's order, checks in each socket's requirement order. Independent of argument order and labels (tested by permutation).

### 11.2 The document

- **MF6.** **Envelope:**

| Field | Meaning |
| --- | --- |
| `format`, `format_version` | `"fitment.match"`, 1; frozen at the first publish |
| `extractor_version`, `socket_rules_version`, `signature_version`, `match_rules_version` | The versions that read the candidates, checked the socket's currency, built the signatures, and judged (M4-27) |
| `policy` | `name`, `linear_mm`, `angular_deg` (D9, `TolerancePolicy`), `clearance` (`strict` or `functional`, D6), `grip` (`hard`, D7; `warning` reserved for M7) |
| `options` | `require_fit`, `require_reviewed` |
| `fit_verifier` | The kernel and version the verifier names (`FitVerifier::kernel`): `null` in M4's CLI, which has none |
| `socket` | `digest` and `source` (`sha256`, `bytes`) of the socket document, `target`, `selection`, and per socket `id`, `occurrences`, `active`, `unreviewed`, `removed` (`[{requirement, note}]`), `unverifiable` (ids, those failing by the floor for the target's own u marked) and `left_out` |
| `summary` | `candidates`, `match`, `interface_match`, `reject`, `refused` |
| `candidates` | One entry per distinct candidate, in MF5's order |
| `original` | With `--original` only: the original's own entry (VD15), labelled `original`, in neither `summary` nor the outcome |
| `digest` | `"sha256:<hex>"` of the compact JSON without `digest` and without every `labels` (MF7) |

- **Candidate entry:** `labels`; `source` (`sha256`, `bytes`, `header` as `features.v1` records it; for a component, the assembly's and its `definition`); `product`; `status` and `refusals` (M2's gate); `read` (EV6); `verdict`; `fit`; `flags`; `warnings`; `properties` (`{"material": {"status": "not_compared", "reason": …}}`, section 14); `reasons` (each `{reason, socket, requirement, codes}`: `candidate_refused`, `no_sockets`, `no_requirements`, `requirement_failed`, `shortlist`, `no_placement`, `body_interferes`); `sockets` (each `{socket, stage, verdict, fit, placement, missing, failed, checks, not_checked}`, where `not_checked` lists what M4 does not check there, `max_extra_mm` among it).
- Conventions are `features.v1`'s (M2 10.2): snake_case with unit suffixes, `[x, y, z]` points, references as bare integers, ids as strings, internally tagged enums, `null` written, pretty JSON plus `\n`, `float_roundtrip`. Requirement ids are qualified by their socket where they leave it: `socket-2/bore:hole-838` (M3 9.2).
- **MF7.** **The digest identifies a result; it does not protect one.** Two runs with the same inputs have the same digest on every OS (EV8), so CI and M6 can compare and cache by it. Anyone can recompute it. There is no `from_json` for `fitment.match` (M4-23): nothing in fitment turns a stored report back into a `Verdict`, so an edited file can never become a `Match`. The structural read-back KH12 asks for is a test-side checker over the JSON, not a reader in core.

### 11.3 Example

s01's bracket socket against C's 14 candidates, two of them shown. The shape and the summary follow C's prototype (`C/s01_match.json`; section 19.1 says which of its verdicts the merged rules would change); the measures follow VR2 to VR4 and were computed by hand from the construction, since the prototype aligned at the identity with crude checks. `…` marks cut lists. `c00` is the original bracket written alone; `c06` has the hole at (−30, −15) blind from the top, 2 mm short of the bottom face. Both parts state 1e-7 mm, so u = 1e-6 mm (`GUARD_MM`), ε = 0.009999 mm, τ = max(`contact_mm` 0.01, span floor 1e-7, u) = 0.01 mm, and VR4's limit τ + u = 0.010001 mm. The core measures (VR22) are left out of the example.

```json
{
  "format": "fitment.match", "format_version": 1,
  "extractor_version": 6, "socket_rules_version": 6, "signature_version": 1, "match_rules_version": 1,
  "policy": { "name": "default", "linear_mm": 0.01, "angular_deg": 0.01, "clearance": "strict", "grip": "hard" },
  "options": { "require_fit": false, "require_reviewed": false },
  "fit_verifier": null,
  "socket": { "digest": "sha256:…", "source": { "sha256": "…", "bytes": 56247 },
              "target": { "instance": 30, "product": { "instance": 28, "id": "BRACKET", "name": "BRACKET" } },
              "selection": { "part": "BRACKET", "occurrence": null, "matched_by": "product_id" },
              "sockets": [ { "id": "socket-1", "occurrences": 1, "active": 7, "unreviewed": 7, "removed": [],
                             "unverifiable": [], "left_out": ["hole-705"] } ] },
  "summary": { "candidates": 14, "match": 0, "interface_match": 5, "reject": 9, "refused": 0 },
  "candidates": [
    { "labels": ["cand/c00_original.stp"],
      "source": { "sha256": "23cb289f…", "bytes": 14213, "header": { "…": "…" } },
      "product": { "instance": 7, "id": "c00_original", "name": "c00_original" },
      "status": "accepted", "refusals": [],
      "read": { "extractor_version": 6, "complete": true, "max_uncertainty_mm": 1e-7, "max_vertex_offset_mm": 0.0 },
      "verdict": "interface_match",
      "fit": { "status": "not_checked", "reason": "no fit verifier in this build" },
      "flags": ["material_not_compared", "requirements_unreviewed"], "warnings": [],
      "properties": { "material": { "status": "not_compared", "reason": "no property fitment maps in either file" } },
      "reasons": [],
      "sockets": [ {
        "socket": "socket-1", "stage": "verification", "verdict": "interface_match",
        "fit": { "status": "not_checked", "reason": "no fit verifier in this build" },
        "placement": { "rotation": [[1, 0, 0], [0, 1, 0], [0, 0, 1]], "translation_mm": [0, 0, 0],
                       "seeded_by": { "anchor_pair": ["bore:hole-654", "bore:hole-722"], "slide": "seat:plane-467" },
                       "tried": 8, "chosen": "first_passing", "u_mm": 1e-6, "epsilon_mm": 0.009999 },
        "missing": [], "failed": 0, "not_checked": [],
        "checks": [
          { "requirement": "seat:plane-467", "review": "unreviewed", "result": "passed", "feature": "plane-88", "…": "…" },
          { "requirement": "grip:plane-467+plane-570", "review": "unreviewed", "result": "passed", "feature": "plane-88+plane-191",
            "measures": [ { "quantity": "grip_length_mm", "found": 8.0, "nominal": 8.0, "band": "epsilon", "band_mm": 0.009999,
                            "min": 7.990001, "max": 8.009999, "margin": 0.009999, "passed": true } ], "failures": [] },
          { "requirement": "seat:plane-570", "…": "…" },
          { "requirement": "bore:hole-654", "review": "unreviewed", "result": "passed", "feature": "hole-275",
            "measures": [
              { "quantity": "axis_offset_mm", "found": 0.0, "nominal": 0.0, "band": "epsilon", "band_mm": 0.009999,
                "min": null, "max": 0.009999, "margin": 0.009999, "passed": true },
              { "quantity": "axis_sine", "found": 0.0, "nominal": 0.0, "band": "angle", "band_rad": 0.000174531924,
                "min": null, "max": 0.000174531924, "margin": 0.000174531924, "passed": true },
              { "quantity": "clear_margin_mm", "span_mm": [0.0, 8.0], "segment": "cylinder-275", "found": 0.3,
                "nominal": 6.0, "band": "tau_guard", "band_mm": 0.010001, "min": 0.010001, "max": null, "margin": 0.289999, "passed": true },
              { "quantity": "diameter_mm", "span_mm": [0.0, 8.0], "segment": "cylinder-275", "found": 6.6,
                "nominal": 6.6, "band": "epsilon", "band_mm": 0.009999, "min": null, "max": 6.609999, "margin": 0.009999, "passed": true },
              { "quantity": "open_start", "found": "opening", "at_mm": 0.0, "required": "opening", "passed": true } ],
            "failures": [] },
          { "…": "bore:hole-671, bore:hole-688, bore:hole-722 alike" } ] } ] },
    { "…": "four more interface matches, then the rejects; c06:" },
    { "labels": ["cand/c06_blind.stp"], "verdict": "reject",
      "reasons": [ { "reason": "requirement_failed", "socket": "socket-1", "requirement": "bore:hole-654",
                     "codes": ["end_not_open", "wall_missing"] } ],
      "sockets": [ { "socket": "socket-1", "stage": "verification", "verdict": "reject", "failed": 1,
        "checks": [
          { "requirement": "seat:plane-467", "review": "unreviewed", "result": "passed", "feature": "plane-…" },
          { "…": "the grip and the other seat, passed, ids and features only (M4-19)" },
          { "requirement": "bore:hole-654", "review": "unreviewed", "result": "failed", "feature": "hole-264",
            "measures": [ "…the axis, the clear margin and the diameter, passed…",
                          { "quantity": "open_start", "found": "bottom", "at_mm": 2.0, "required": "opening", "passed": false } ],
            "failures": [
              { "code": "wall_missing", "at": { "span_mm": [0.0, 8.0] }, "detail": "no candidate wall over [0, 2] mm of the span" },
              { "code": "end_not_open", "at": { "end": "start" }, "detail": "the start end is a bottom at 2 mm, where an opening is required at 0 mm" } ] },
          { "…": "three bores passed, ids and features only" } ] } ] }
  ],
  "digest": "sha256:…"
}
```

The original's margins on its own values are exactly ε = t − u: it passes by the tolerance less the guard, never by an edge.

## 12. API and modules

Bytes in, values out; no I/O; no thread. The public types are `#[non_exhaustive]` and built only by the functions below, their fields read through accessors where shown as private; `FitStatus` and `VerdictKind` stay exhaustive by design (D1). Each type is defined once; the per-candidate view is built once and shared by the shortlist, alignment and verification.

```rust
pub const MATCH_FORMAT: &str = "fitment.match";
pub const MATCH_FORMAT_VERSION: u32 = 1;
pub const MATCH_RULES_VERSION: u32 = 1;
pub const SIGNATURE_VERSION: u32 = 1;
pub const GUARD_MM: f64 = 1e-6;                          // VR2
pub const MAX_SHORTLIST_CHECKS: u64 = 1_000_000;         // SL4
pub const MAX_PLACEMENTS: usize = 4_096;                 // AL11
pub const MAX_ANCHOR_PAIRS: usize = 16;                  // AL4
pub const MAX_COVERAGE_STEPS: u64 = 20_000_000;          // AL11: VR11 and VR22, per candidate and socket

// policy.rs: D6, D7, D9
pub struct MatchPolicy { /* tolerance: TolerancePolicy, clearance: Clearance, grip: Grip */ }
pub enum Clearance { Strict, Functional }               // default Strict
pub enum Grip { Hard }                                   // D7; non_exhaustive: M7 adds Warning
impl MatchPolicy { pub fn new(t: TolerancePolicy, c: Clearance, g: Grip) -> Self; /* accessors */ }
impl Default for MatchPolicy { /* 0.01 mm, 0.01°, Strict, Hard */ }
pub struct MatchOptions { /* require_fit, require_reviewed */ }
impl MatchOptions { pub fn new(require_fit: bool, require_reviewed: bool) -> Self; /* accessors */ }

// candidate.rs (CF): what alignment and verification read, built once per candidate
pub struct CandidateView { /* stacks with profiles (VR8), shell planes with exact outlines and
                              shared-edge marks, walked cones, u_c, an index of axis feet */ }
impl CandidateView { pub fn of(part: &Part) -> Self; pub fn signature(&self) -> &PartSignature; }

// signature.rs (SG): policy-free, stored by M5
pub struct PartSignature { /* stacks, planes, cones, lever_mm, uncertainty_mm, unread */ }
impl PartSignature { pub fn of(part: &Part) -> Self; pub fn unread(&self) -> &Unread; }
pub struct SigStack { pub feature: FeatureId, pub sense: Sense, pub axis: Axis, pub diameters_mm: Vec<f64>,
                      pub extent_mm: [Option<f64>; 2], pub openings: u8, pub unknown_end: bool }
pub struct SigPlane { pub feature: FeatureId, pub normal: Dir3, pub offset_mm: f64,
                      pub known_area_mm2: f64, pub faces_known: u32, pub faces_unknown: u32 }
pub struct SigCone  { pub feature: FeatureId, pub axis: Axis, pub included_angle_rad: f64,
                      pub sense: Sense, pub radii_mm: Option<[f64; 2]> }
pub struct Unread   { pub undecided: u32, pub partial: u32, pub other_faces: u32,
                      pub unknown_outline_faces: u32, pub unknown_end_stacks: u32 }

// One socket, ready to judge: built once by Matcher::new
pub struct SocketView<'a> { /* the Socket, its active requirements, span floors, contact_mm,
                               the target's u, its SocketSignature, alignment tables */ }
pub struct SocketSignature { /* lines, seats, cones, pairs, blocked_by, removed, lever_mm, windows */ }
impl SocketSignature {
    pub fn of(socket: &SocketView<'_>, policy: &MatchPolicy) -> Self;
    pub fn blocked_by(&self) -> &[RequirementId];     // SG7
    pub fn is_empty(&self) -> bool;                     // SL8
}

// shortlist.rs (SL1 to SL6): level 2
pub fn shortlist(socket: &SocketSignature, part: &PartSignature) -> Result<Kept, Vec<Missing>>;
pub struct Kept { pub domains: Vec<(RequirementId, Vec<FeatureId>)>, pub budget_exhausted: bool }
pub struct Missing { pub requirements: Vec<RequirementId>, pub element: MissingElement, pub window: Window }
pub enum MissingElement { Diameter, Openings, Extent, Plane, Cone, Pair { with: Vec<RequirementId> }, Distinct }

// index.rs (SL7 to SL9): level 1, data only; M5 stores the tokens
pub struct IndexTokens { /* diameters, pairs, planes, lever, floor, pairs_incomplete */ }
impl IndexTokens { pub fn of(part: &PartSignature) -> Self; }
pub struct ShortlistIndex { /* BTreeMaps keyed by sense and bin */ }
impl ShortlistIndex { pub fn insert(&mut self, id: u64, t: &IndexTokens);
                      pub fn query(&self, s: &SocketSignature) -> Result<Vec<u64>, Blocked>; }

// align.rs (AL)
pub struct Placement { pub transform: Rigid, pub seed: Seed, pub index: usize }  // candidate → target
pub enum Seed { AxisPair { a: (RequirementId, FeatureId), b: (RequirementId, FeatureId), slide: Option<Slide> },
                OneAxis { axis: (RequirementId, FeatureId), turn: Option<Turn>, slide: Option<Slide> },
                Cone { cone: (RequirementId, FeatureId) },
                Landmark { seat: RequirementId, target_edge: u64, candidate_edge: u64 },
                Given }
pub struct Placements { pub ordered: Vec<Placement>, pub tried: usize, pub exhausted: bool,
                        pub unanchored: Vec<RequirementId> }
pub fn align(socket: &SocketView<'_>, cand: &CandidateView, kept: Option<&Kept>, policy: &MatchPolicy) -> Placements;

// verify.rs (VR)
pub fn verify(socket: &SocketView<'_>, requirement: &Requirement, cand: &CandidateView,
              at: &Placement, policy: &MatchPolicy) -> Check;

// evidence.rs (EV)
pub struct Evidence { /* feature: Vec<FeatureId>, measures: Vec<Measure>, failures: Vec<Failure>,
                        warnings: Vec<Warning> (empty in M4; M7's grip_differs, VD13) */ }
pub struct Warning { pub code: WarningCode, pub measure: Measure }
pub struct At { /* span_mm, segment, contact_face, end, at_mm: whichever apply */ }
pub struct Measure { /* quantity, at, found, nominal, band, band_mm, min, max, margin, passed */ }
pub struct Failure { pub code: FailureCode, pub at: At, pub detail: String }
pub enum FailureCode { /* section 8.5, each with a fixed machine name */ }

// verdict.rs, extended (M0's Check gains its evidence)
pub enum Check { Passed(Evidence), Failed(Evidence) }
pub fn decide(requirements: &[RequirementId], checks: impl IntoIterator<Item = (RequirementId, Check)>,
              fit: FitStatus) -> Verdict;               // per socket, M0's contract
pub fn combine(sockets: &[Verdict]) -> Verdict;          // VD2, VD4; empty → Reject(no_sockets)
impl Verdict { pub(crate) fn refused(codes: Vec<RefusalCode>) -> Self; } // VD1, VD10: judge_step only

// fit.rs: the hand-off to M7
pub trait FitVerifier { fn check(&self, request: &FitRequest<'_>) -> FitStatus; }
pub struct NoFitVerifier;                                // not_checked, NO_FIT_VERIFIER
pub struct FitRequest<'a> {
    pub socket: &'a str,                                 // socket-1
    pub candidate: &'a CandidateSource,                  // its SHA-256 and where it came from
    pub candidate_step: Option<&'a [u8]>,                // the bytes, when the run has them
    pub assembly_sha256: [u8; 32],                       // the socket's source
    pub placements: Vec<Rigid>,                          // T_k · A, one per occurrence of the socket
    pub check_set: CheckSet,                             // others_and_copies (M3's V1)
}
pub enum CheckSet { OthersAndCopies }                    // the socket stores it as a string today

// mod.rs: the entry points
pub struct Matcher<'a> { /* private */ }
impl<'a> Matcher<'a> {
    pub fn new(socket: &'a SocketDocument, policy: MatchPolicy, options: MatchOptions,
               fit: &'a dyn FitVerifier) -> Result<Self, SocketRefused>;   // MF2
    pub fn judge_step(&self, step: &[u8]) -> CandidateResult;              // M2's gate, then judge
    pub fn judge_part(&self, part: &Part, source: CandidateSource) -> CandidateResult; // M5, the original, KC11, KC12
    pub fn verify_at(&self, part: &Part, socket: &str, at: &Rigid) -> SocketResult;  // AL14, KH8
}
pub enum SocketRefused { Refused(Vec<Refusal>), Stale { extractor: u32, socket_rules: u32 },
                         Unreviewed { count: usize, first: Vec<String> } }
pub enum CandidateSource { File { sha256: [u8; 32], bytes: u64, header: FileHeader },
                           Component { assembly_sha256: [u8; 32], definition: DefinitionRef },
                           Catalog { id: u64 } }
pub struct CandidateResult { /* source, product, status, refusals, read, verdict, flags, warnings, sockets */ }
pub struct SocketResult { /* socket, stage, verdict, fit, placement, missing, checks, not_checked */ }

// document.rs (MF)
pub struct MatchDocument { /* private */ }
impl MatchDocument {
    pub fn new(matcher: &Matcher<'_>, results: Vec<(CandidateResult, Vec<String>)>) -> Self; // order, dedup, digest
    pub fn to_json(&self) -> String;
    pub fn summary(&self) -> Summary;
    pub fn outcome(&self) -> Outcome;                    // after require_fit
}
pub fn match_candidates(socket: &SocketDocument, candidates: &[(&[u8], &str)], policy: MatchPolicy,
                        options: MatchOptions, fit: &dyn FitVerifier) -> Result<MatchDocument, SocketRefused>;
                                                         // tests and small sets: holds every candidate's bytes
pub struct Summary { /* candidates, match, interface_match, reject, refused, best: VerdictKind */ }
pub enum Outcome { Match, InterfaceMatch, Reject }
// Also declared beside their users: Window (SG6: lo, hi), Blocked (SL8: why a query is blocked),
// Turn and Slide (AL7, AL8: the edge pair or plane that fixed them), WarningCode (VD12's closed list).
```

`RequirementId` gains its socket where it leaves one (`socket-1/bore:hole-88`). `RejectReason` gains `CandidateRefused { codes }`, `NoSockets`, `Shortlist { socket, missing }` and `NoPlacement { socket, unanchored }`; `RequirementFailed` carries `codes` beside its detail. From the socket PR (M4-1), `SocketDocument::active(socket)`, `removed(socket)` and `contact_mm()` are public. The CLI reads candidates one at a time through `Matcher::judge_step` and `MatchDocument::new` (MF3); `match_candidates` is the convenience for tests and small sets.

```text
crates/fitment-core/src/matching/
  mod.rs          Matcher, CandidateResult, SocketResult, SocketView
  policy.rs       MatchPolicy, Clearance, Grip, MatchOptions
  candidate.rs    CF: CandidateView
  signature.rs    SG: PartSignature, SocketSignature
  shortlist.rs    SL1 to SL6
  index.rs        SL7 to SL9
  align.rs        AL
  verify.rs       VR, but coverage
  region.rs       VR11: certified coverage
  evidence.rs     Measure, Failure, FailureCode
  document.rs     MatchDocument, order, digest
crates/fitment-core/src/verdict.rs, fit.rs   extended in place
crates/fitment/src/cmd/match_.rs             the CLI (`match` is a keyword)
```

## 13. The CLI

```text
fitment match <SOCKET> <CANDIDATE>... [--original <ASSEMBLY>]
              [--format table|json] [-o <OUT>] [--explain]
              [--require-fit] [--require-reviewed]
              [--clearance strict|functional]
              [--tolerance-mm <X>] [--tolerance-deg <Y>]
```

- **MF8.** **Output.** By default stdout gets the table. `--format json` prints the document. `-o OUT` writes the document atomically (a temporary file, then a rename, as `socket -o`), and stdout gets the table unless `--format json`. OUT may not be the socket file or a candidate (exit 2, OUT untouched). Policy flags build a `TolerancePolicy` named `cli` whenever a value differs from D9's (a zero, negative or non-finite value is a usage error), and the table's first line says the policy is not the default.
- **MF9.** **The table** (not a stable format): one header line (socket digest prefix, target, sockets, policy, fit); one line per candidate in MF5's order (verdict, label, the first 8 hex of its SHA-256, and either its flags or its failure count with the first failed requirement and codes); then, for every `Match` and `InterfaceMatch`, one line per check of each socket (id, result, candidate feature, tightest margin). `--explain` prints every candidate's every check. A pass whose sockets had removed requirements shows "5 of 7 checked, 2 removed in review".
- **MF10.** **stderr,** one line each, escaped: the summary (`fitment: bracket.socket.json: 14 candidates: 0 match, 5 interface match, 9 reject; fit not checked (no fit verifier in this build)`); one warning per socket that rejects every candidate (requirements failing every candidate, or none active), naming them and pointing to `fitment review`; the count of unreviewed requirements; each unreadable candidate; each refused candidate's first reason; under `--require-fit`, "no candidate can pass without a fit verifier in this build".

- **MF11.** **Older socket documents** (the socket PR, SR9). A document whose `socket_rules_version` is older than this build's is not read by the strict `from_json` (its spans lack SR1's fields, so it would fail on a missing field or a digest mismatch). `fitment socket -o`'s carry-over and its check for a foreign file (`carry_over` and `foreign()` in the CLI) read it leniently instead, as JSON, its digest checked (recomputed as the digest of what it states, as for any document) and its `review` held to 9.4's rules: its `format`, versions, `source`, target, `review` and each socket's requirements. A decision is carried to a new requirement when the requirement id exists in the new document and every field the older version states is still there and equal (a field the newer adds is not compared), arrays and nested objects compared whole on their shared keys (a span list that splits or merges counts as changed); otherwise it goes to `stale_reviews` as `requirement_changed`, and a missing id as `requirement_gone`. A document of the same `source.sha256` and target is never refused as foreign, so the user's reviewed file is always overwritten with its decisions carried. `fitment match` reads the versions first, the same way, and reports VD9's "stale" before any strict read. A CLI test runs `socket -o` onto a rules-5 document.

The exit codes are VD14's. The 100-candidate run against s01's bracket (`C/bulk_table.txt`, prototype output; the real table adds the SHA prefix):

```text
socket sha256:124f4f64627f  target 'BRACKET'  1 socket(s)  policy 0.01 mm / 0.01 deg, clearance strict, grip hard  fit: not checked (no fit verifier in this build)
INTERFACE  bulk/b064_d6.6_dx0.004_t8.0.stp    all requirements verified; material_not_compared
INTERFACE  bulk/b034_d6.3_dx0.004_t8.0.stp    all requirements verified; material_not_compared
  … 4 more interface matches (Ø6.3, 6.5, 6.6 holes; shifted 0 or 0.004 mm; 8 mm thick)
reject     bulk/b052_d6.5_dx0.02_t8.0.stp     1 failed: bore:hole-654 no_feature
reject     bulk/b062_d6.6_dx0.0_t10.0.stp     2 failed: seat:plane-467 no_feature …
  … 90 more, nearest first
```

## 14. Properties from STEP only (D10)

`stepq props --format json` over all 45 fixtures (`C/props.py`, `C/props.txt`):

| What | Count |
| --- | --- |
| Files with any property | 29 of 45 (none in the 7 NIST-EDM files, 8 of the 11 AP203 geometry-only files, or `as1-ac`) |
| Properties: validation / user-defined / other | 222 / 611 / 1,520 |
| Of the "other": `pmi validation property` | 1,022 |
| User attributes by name | `Part Number`, `Revision`, `Description`, `CAGE Code`, `Company`, `Modeled By`, `Density` (3), `Mass`, `Volume`, CATIA's inertia, `p1`…`p255` (one file), file metadata |
| A material name or designation, as a property or `material_designation` | **0** |
| A thread, as a property | **0** |
| `Density` | 3 CATIA AP242 files, each `1000.0` (CATIA's default, not a material) |
| A material anywhere | one drawing note in `nist_stc_09` and `nist_ftc_09` ("MATERIAL: … 304 SST SHT"), as semantic text |
| A thread anywhere | PMI text in `nist_ctc_04` (`4X M12x1.75-6H`) and `nist_stc_10` (`M3 x 0.5-6g`); cosmetic thread curves in `nist_ctc_04` and `nist_ftc_10` |

So M4 compares no property. Every verdict carries `material_not_compared`; every pass of a pinned tight fit carries `thread_not_confirmed`. PLAN's stated limit (without properties, a tapped hole and a plain one cannot be told apart) is the normal case. The candidate entry has `properties` from v1, with one value, so the field exists when the format freezes. D10's mapping (needed by M5) starts empty: a table from (property name as written, case-folded, kind) to a field (`material`, `finish`, `thread`), versioned with `MATCH_RULES_VERSION`, compared only when both files state the field. The likeliest route to "thread confirmed" is PMI linked to a hole's faces (stepq 0.6's PMI to faces); two NIST files would exercise it, no assembly fixture does. Recorded as later work.

## 15. The known-answer corpus (KC)

### 15.1 A match case

- **KC1.** **Answers from the construction only.** Every expected value comes from the generator's parameters and closed forms, never from reading a written file back, as in M2 and M3. The one exception is named: an AS1 cross-export run, whose truth is "the same part" (KC12).
- **KC2.** **One manifest, one generator.** Match cases join `corpus/out/manifest.json` as `"kind": "match"`, beside parts and assemblies, so the `rule_ids` registry and the coverage test cover them. A case:

```json
{"id": "m_s01_bracket_blind", "kind": "match", "set": "corpus",
 "socket": {"case": "s01_bolted", "part": "BRACKET", "occurrence": null, "review": []},
 "candidate": {"file": "m_s01_bracket_blind.stp"},
 "class": "b_blind", "truth": "does_not_fit",
 "construction": {"from": "BRACKET", "defect": "hole (30, 15) blind 6 deep from the top"},
 "frame": {"axis": [2, -1, 1], "angle_deg": 140.0, "shift": [-35.0, 12.0, 60.0]},
 "placement": [[...3x4, unrounded...]],
 "expect": {"default": {"verdict": "reject",
                        "must_fail": [{"kind": "bore", "axis_point": [30, 15, 0], "axis_direction": [0, 0, 1],
                                       "codes": ["end_not_open", "wall_missing"]}],
                        "may_fail": []},
            "functional": "default"},
 "rules": ["KC9", "KO1"], "tests": "a blind hole where the socket needs the bottom open"}
```

  `candidate` is `{"file": …}` or `{"component": {"case" | "fixture": …, "part": …, "occurrence": n}}`. `socket.review` lists `{"requirement": <locator>, "status": "removed", "note": …}` to apply before matching. `placement` maps the candidate's frame into the target's and is **written unrounded**: the generator's 9-decimal rounding moved a point 60 mm out by 1.3e-8 mm, larger than any representation noise the edge cases need (KC7). A policy's expectation is an object, or the name of the policy it equals.

  Three more fields let a case state what the socket gate and the floor rules need (RM1): `socket.floor` names a generated source assembly written with stated length uncertainties (`{"parts_mm": 0.02}`, or `{"placement_mm": 0.01}` for the root context only, as AS1-md); `socket.edit` edits the extracted document before matching (`"stale"`: `socket_rules_version` lowered and the digest recomputed; `"digest"`: one value changed; `"unreviewed"` with `"options": {"require_reviewed": true}`); and `socket.case` may name a refusal case (`r*`), whose refused document the gate must refuse. Every case gains a `kind`: part cases `"part"`, assemblies `"assembly"`, match cases `"match"`; the PR that adds match cases (PR 5) changes `corpus.rs` (its per-case `file` read and its `kind != "assembly"` loop) and `golden.rs` (its loop over every case) to select by kind (RM6).
- **KC3.** **Truth.** One of:
  - `fits`: placed by the construction, the candidate adds no common volume with any neighbour that has none with the original, keeps every interference fit's engagement within the tolerance, and keeps every contact the original makes within the tolerance times the contact's boundary;
  - `fn_rule`: fits physically, and a stated conservative rule rejects it (R14's target-face basis, M2 9.6's undecided cylinders, an axis tolerance tighter than a clearance, VR11's step 1, which rejects a candidate edge more than ε/2 inside a seat region, the floor). Expected `Reject`, counted as a false negative by design;
  - `fits_functional`: fits when D6 functional excuses the rim ring a larger clearance hole takes from its seats; does not fit under strict;
  - `does_not_fit`: a defect by construction; expected `Reject`;
  - `edge`: a stated limit hit exactly; Soundness rule 4 rejects it whatever the physics says;
  - `collides`: every requirement holds, and the body shares volume with a neighbour no requirement concerns. Expected `InterfaceMatch` (fit `not_checked`) until M7, `Reject` (`interferes`) with a fit verifier.
- **KC4.** **Sources.** Every M3 corpus case with expected sockets (74 case and target pairs, 79 sockets; 63 pairs with no unverifiable requirement, 11 with some) and every fixture target pinned in `fixtures/manifest.json` (36, AS1's six exports among them).
- **KC5.** **Locators.** A requirement is named by what it constrains, in the target's frame: a bore or shaft by kind and axis line (point and direction, compared as lines within 1e-6 mm and 1e-9), a seat by outward normal and offset, a conical seat by apex, a grip by length and its two seats' locators, an `unresolved` by cause and the target feature's locator (or `part` and the neighbour's product id), a partial cylinder by axis line and diameter. Never by requirement id, which holds the writer's `#id`s. The harness maps each locator to exactly one requirement of the socket it extracts; none or several is a harness error, never a pass. M3's `common::assembly` already compares requirements this way.
- **KC6.** **Frames, writers and units.** A generated candidate is written in a frame motion: `IDENTITY`, `VARIANT` (the `_rot` motion), `TILT`, or `MAPPED` (a quarter turn with short numbers, for the committed set). Defects default to `VARIANT`, so alignment is always exercised. Good cases add writer variants: pcurves on, closed faces split in two (`ShapeDivideClosed`), the file in inches.
- **KC7.** **Edge cases in four frames.** Each `edge` case is written in `IDENTITY`, `VARIANT`, `TILT` and one seeded skew frame; all four must reject. Measured (section 20.4): an exact 0.01 mm shift reads between −7.4e-11 and +6.5e-11 mm of the edge, inside it in 21 of 40 frames. The edges include VR4's limit, a clearance exactly at c + 2τ for a bore (s50's CAP) and a shaft (s50's TUBE): a tube of Ø41.98 there passed in IDENTITY and failed in VARIANT before VR4's guard (`review-soundness/proto.txt`; RS6).

### 15.2 The classes

Every class has a short id (the manifest's `class`), so the report counts per class.

- **KC8.** **Good** (`fits`; expected `InterfaceMatch`):
  - `g_orig`: the original, its own frame (KC11 covers it in place);
  - `g_frame`, `g_unit`, `g_writer`: the original in another frame, in inches, with pcurves or split faces;
  - `g_extra_hole`: an extra hole in no contact face and on no neighbour's axis (a Ø3 hole along y through the bracket's sides);
  - `g_body`: another body with the same interface: a larger outline (90 × 56 for 80 × 50), chamfered free edges, a rib on a free face;
  - `g_left_out`: a left-out feature changed or removed (s01's Ø8 hole made Ø10, or absent);
  - `g_inside_tol`: every limit approached to within a quarter of the tolerance, or to half of it where the feature anchors the placement (a hole moved, a pinned diameter, a thickness, an angle); the case logs the anchor pair (`seeded_by`). By VR11's rule, a hole off the anchor pair moved by more than ε/2 rejects at its seats; such a case is `fn_rule` (section 25.2, merge finding 1; derived, not measured);
  - `g_cross_hole`: s19's housing with a Ø2 air hole across its bore, inside the span (`housing_cross`): its wall openings are hole-sense, so it passes (CF8, VR22);
  - `g_mirror`: a mirrored product that a rotation maps onto the socket (s15's MIRROR against BLOCK);
  - `g_some_sockets_all`: s07b's original against both its sockets;
  - `g_thread`: s01's nut, pinned Ø5 interference: `InterfaceMatch` carrying `thread_not_confirmed`.
- **KC9.** **Bad** (`does_not_fit`; expected `Reject` for the named requirement), each made just past the tolerance (1.5 × tol) and grossly:
  - `b_hole_shift` on a pinned or line-to-line bore (s03's Ø6 dowel hole moved 0.015; an AS1-style Ø10 line to line), and on a clearance bore beyond its clearance (s01's hole moved 0.5 mm with 0.3 mm of clearance);
  - `b_dia_small` (below the clear diameter), `b_dia_pinned` (a pinned span ± 1.5 tol), `b_dia_large` (above the original under strict; `fits_functional`);
  - `b_offset_clear`: s16's offset bolt needs clear 6.4, so a Ø6.35 hole passes a naive "shaft diameter 6" check and must fail;
  - `b_missing_hole`, `b_blind` (blind where the socket needs an end open), `b_depth` (s02 BASE's blind hole short of its full-diameter depth 7.5);
  - `b_thickness` (grip ± 1.5 tol and ± 0.5 mm), `b_seat_moved` (s02's counterbore 0.5 deeper), `b_seat_region` (a hole inside the true contact: s01's bracket with a Ø4 hole in its bottom face, inside the plate's contact);
  - `b_cbore` (counterbore under the head's clear 10, or none), `b_cone_angle` (82° for 90°; 90° + 1.5 angular tol), `b_cone_range` (rim Ø11 short of the head's 12.4), `b_cone_missing` (a counterbore in its place), `b_cone_sense` (a conical boss at a countersink's place);
  - `b_shaft`: s01's bolt with a Ø6.7 shank, or too short to engage its nut over the pinned 5 mm span; s19's boss at Ø12.15 in its Ø12.1 bore;
  - `b_thread`: s01's nut with a Ø6 hole: a clearance nut is not a thread;
  - `b_self`: s05's washer 1.7 thick (both copies' grips, and the washer-to-washer `self` seat); s21's block with its pinned Ø10 hole at 10.02;
  - `b_some_sockets`: s07b's bracket with every hole Ø6.4: it passes socket-1 (four clearance bores) and fails socket-2's pinned `not_read` bore;
  - `b_mirror`: a new case s59, a block with a counterbore and an off-centre hole against a side seat, and its mirror image as the candidate: no rotation maps it;
  - `b_decoy`: the required hole missing and an extra one elsewhere that only a wrong placement would use; the pattern translated 0.5 mm against the body;
  - candidate-side reading (RS1 to RS5, RS10), each against the neighbour its passing requirement cites, from `review-soundness/gen.py`: `b_post` (s19 HOUSING with a Ø4 post from an annular floor, COVER dV +50.27 mm³; s02 BASE with a Ø2 post in a pinned span, SCREW +14.14), `b_unknown_end` (a cone spike, +7.07; a spherical dome floor, +74.25), `b_wall_opening` (a Ø2 bar across the bore, +37.57), `b_end_zone` (a lip: annulus out, cone back in to r 5, +5.93; and a transition of annulus, cone and annulus dipping below both segments), `b_seat_unknown_face` (the housing's top split into three coplanar faces, one of unknown outline hiding a pocket: contact −4.71 mm² where KO1 allows −1.98), `b_open_past_rim` (s01's nut 5 mm taller below: its hole narrowing to Ø3 past the original's rim, `nut_ext`, BOLT +42.41 mm³; the pinned Ø5 continued past the rim, `nut_long`, +17.28 over the original's 43.20, where KO1 allows 0.39; the same with a Ø1 bar across the extension, `nut_bar`, +3.91 more). (`b_float_lip`, s02's block with a deeper counterbore and a lip where a reseated head would sit, belongs to D7 `warning` and moves to M7 with it.) Each passed B's prototype (`review-soundness/proto.txt`, `physics.txt`) and must now reject for the rule that names it (VR22, VR8, CF5, CF8, VR11, VR7);
  - `b_unverifiable`: every candidate, the original included, against a socket with unverifiable requirements and no review (s11, s12, s13, s17, s47, s48, s52, s58): rejected with exactly those requirements failing, code `unverifiable`.
- **KC10.** **False negatives by design** (`fn_rule`; expected `Reject`, counted in the rate against physics): `fn_r14` (s01's bracket with a 20 × 12 × 2 pocket in its top face clear of the heads: the heads' true contact is covered, the target face is not), `fn_position` (a clearance hole moved 1.5 tol or 0.05 mm, which its bolt still passes), `fn_undecided` (a clearance hole tilted 1°, whose ends M2 reads as undecided), `fn_floor` (`as1_pe`'s sockets: its parts state 0.115 to 0.672 mm, VD6), `fn_cross_export` (AS1 ec and tc candidates, whose cylinders are undecided or B-spline), `fn_coverage` (a hole off the anchor pair moved between ε/2 and ε).
- **KC11.** **Original runs, in place** (VR21). For each of the 74 M3 targets, the target's own component read from the base assembly against the base's socket; and for the 68 targets with variants, also the component of its `_tgt` variant (modelled in TILT) and of its `_pc` variant against the base's socket, and the base's component against the `_tgt` socket. Six targets have no variant and run their own frame only: s10's BASE, BLOCK and SCREW, s22's BLOCK, s55's BLOCK and PLATE (RM2). For each of the 36 fixture targets, its component against its socket. That is 74 + 68 × 3 + 36 = 314 runs; the `_pc` copies have not been run yet. Expected: `InterfaceMatch` where the socket has no requirement failing every candidate; otherwise `Reject` with exactly those failing (code `unverifiable`, or `floor_above_tolerance` under VD6). After the socket PR, U11 adds s50 CAP's skirt bore and spigot and s54's two pins to the requirements its original fails (VR22). s14's original passes its interface and clashes (M7, V1). s55's plate is the one named exception: it must pass when seeded with the identity (VR21).
- **KC12.** **AS1 across exports, in place.** For each part (plate, l_bracket, bolt, nut, rod) and each ordered pair of exports (ac, md, ug, pe, ec, tc: 36 pairs), the candidate export's component against the socket export's socket: 180 runs. Truth is the construction's: the same model. Expected: `InterfaceMatch` among ac, md and ug where the socket has no unverifiable requirement (each bolt document has one socket with one, `socket-2`; its `socket-1` has none); pe's sockets and pe's candidates reject with `floor_above_tolerance` (pe's parts state 0.115 to 0.672 mm); ec's and tc's sockets reject (`unverifiable`), and ec's and tc's candidates reject (`no_feature`: no decided hole). Measured for `l_bracket` (`B/out/as1x.txt`): ac, md and ug pass each other's sockets, 9 of 9, md's candidate against ac's socket included (ac's has the rod nut's seat, md's candidate has that face); pe fails every comparison on its floor.
- **KC13.** **Review runs.** For each socket with unverifiable requirements, the socket with all of them removed in review (`fitment review --remove … --note`), against the original (expected `InterfaceMatch`, each removed requirement listed as not checked) and against one bad candidate (still `Reject` for its own defect: removal waives only what it names). A bad candidate whose only defect breaks a removed requirement is class `review_waived`: expected `InterfaceMatch` with the removal listed, reported, and not a false positive (the user accepted it).
- **KC14.** **Policies.** Every generated case and every original run runs under `default` (strict, grip hard, 0.01 mm and 0.01°) and `functional` (D6); the manifest states each outcome, by reference where equal. The AS1 cross-export runs (KC12) and the review runs (KC13) run under `default` only. D7 `warning` is not offered in M4 (M4-8). A tolerance sweep (0.001, 0.05, 0.1 mm) is run and reported, never gated: cases built at 0.01 change class there.
- **KC15.** **The committed subset.** `crates/fitment-core/tests/candidates/`: 13 candidates against the committed s01, s02 and s03, written in `MAPPED` (187,015 bytes measured; 234,700 in `VARIANT`), under a cap of 210 KB: bracket good, missing hole, Ø small, thick, blind, lug (collides), top pocket (`fn_r14`); s03 plate good, pinned shift at the edge, press fit opened; s02 block good, counterbore deep; s01 nut pinned 1.5 tol. Regenerated and diffed in the `unit-set` job, as M2's and M3's sets are.
- **KC16.** **Sizes.** Candidate files are 10.7 to 59.5 KB (the split-face one largest), 17.7 KB on average in the slice. The full corpus adds about 150 files and 2.7 MB to M3's 345 files and 17 MB.

### 15.3 The case list (planned)

Per family, counted as generated files; the last rows are runs that need no new file.

| Family | Source | Good | fn_rule | Bad | Edge (× 4 frames) | Files |
| --- | --- | --- | --- | --- | --- | --- |
| Bracket: clearance bores, two seats, grip, left-out hole | s01 BRACKET | 9 (frame, inch, split, pcurves, side hole, larger, chamfered, left-out Ø10 and absent, inside tol) | 5 (pocket, shift 1.5 tol, shift 0.05, tilt 1°, off-anchor shift tol/2) | 11 (shift 0.5, Ø5.9, Ø6.8, missing, blind, face hole, thick, thin, decoy × 2, lug) | 2 (shift tol, Ø clear + tol) | 33 |
| Plate: through bores open both ends | s01 PLATE | 2 | | 2 (blind, thin) | | 4 |
| Nut: pinned interference (thread) | s01 NUT | 2 | | 2 (Ø6, 1.5 tol) | 1 | 8 |
| Bolt: pinned and clearance shaft spans, head seat | s01 BOLT | 2 | | 3 (Ø6.7, short, head seat moved) | 1 | 9 |
| Counterbored block; blind base; stepped screw | s02 | 5 | | 7 | 2 | 20 |
| Press, clearance and line-to-line holes; dowel | s03 | 3 | 1 | 4 | 2 | 16 |
| Bearing seat; housing | s04 | 2 | | 3 | 1 | 9 |
| Stack: plates, two self-paired washers | s05 | 3 | | 4 | 1 | 11 |
| Open and pinned `not_read` holes | s06, s07b | 2 | | 3 (pinned 6.8, some sockets, decoy) | | 5 |
| Unread neighbour, partial, undecided, unknown outline | s11, s12, s13, s17, s58 | | | 5 (unverifiable, reviewed and still bad) | | 5 |
| Mirror | s15, new s59 | 2 | | 2 | | 4 |
| Offset clearance | s16 | 1 | | 2 | | 3 |
| Countersink | s18 | 2 | | 5 (with the conical boss) | 2 | 15 |
| Boss in blind bore | s19 | 1 | | 1, and 1 collides (boss 3.5 longer: `max_extra_mm` is M7's) | 1 | 7 |
| Self rod | s21 | 1 | | 1 | | 2 |
| Candidate-side reading (the review) | s19 HOUSING, s02 BASE, s01 NUT, s50 CAP and TUBE | 1 (cross hole) | | 11 (two posts, spike, dome, bar, lip, three-face transition, hidden pocket, three past the rim) | 2 (c + 2τ, bore and shaft) | 20 |
| **Generated** | | **38** | **6** | **67** | **15 (60 files)** | **about 175** |
| Floor and socket gate (RM1) | new s61 (parts at 0.02 mm), s62 (placement 0.01 mm), s63 (a span's floor from another source than its clear diameter); s01's socket stale, edited and unreviewed; a refused `r*` | | | | | about 10 runs |
| Two cones on one cone (SR8) | new s60, an M3 case | | | | | its socket |
| Original runs (KC11) | 74 M3 targets (68 × 4, 6 own only); 36 fixture targets | | | | | 314 runs |
| AS1 across exports (KC12) | 5 parts × 36 export pairs | | | | | 180 runs |
| Review runs (KC13) | 11 corpus sockets and the fixture ones with unverifiable requirements | | | | | about 30 runs |

Under the two policies M4 offers, about 175 × 2 generated runs and about 534 runs in place under the default policy make about 880 match runs; the original runs repeat under `functional` (VR21), 314 more, about 1,200 in all. D planned about 920 with three original runs per target; B's run of the original against the `_tgt` socket adds the fourth, and the review's cases add about 60.

### 15.4 How they are built

- A section `build_candidates(w, which)` in `tools/gen-corpus.py`, after the assemblies, uses the generator's primitives (`box`, `cyl`, `cone`, `cut`, `fuse`, `through`, `tool`, `Hole`, `hex_prism`) and one parametric builder per source part whose defaults rebuild the original exactly: `bracket(lx, ly, t, holes, centre, extra, pocket, lug, side_hole, tilted)`, `nut(d)`, `block02(cb_d, cb_depth, d, h)`, `plate03(dia, shift)`, `plate18(angle, d0, counterbore, boss)` and so on. A unit check asserts that each builder with its defaults is the leaf the assembly section writes (same faces, and a boolean difference of zero volume through Open CASCADE), so a defect is the only difference.
- Each case states its locators with M3's expectation helpers' conventions (`e_axial`'s points and directions); the generator asserts that every locator names a requirement of the source's expected socket, so a case cannot expect a failure of a requirement the socket does not state.
- `Writer.shape(moved, name, unit, pcurves)` writes each candidate; `fix_time` makes it reproducible. The probes (`D/gen_candidates.py`, and the review's `review-soundness/gen.py` for the candidate-side shapes) do exactly this by importing `tools/gen-corpus.py`, and are the starting point.
- The generator writes s61 to s63 with stated uncertainties by setting each representation context's `uncertainty_measure_with_unit` (and the root's for s62), and builds s60 (a countersunk head and a conical washer on one target cone).
- s59 (the asymmetric block and its mirror) is the only new assembly.

### 15.5 The first slice's expectations, as merged

D's 40 candidates (`D/out/manifest.json`) were run by B's prototype (`B/out/dcands.txt`, `B/out/attrib.txt`): 39 of 40 verdicts agree, with no false positive. The attribution showed where D's `must` lists assumed one failure and verification found another; KH2's `any_of` and `may` absorb them (M4-31):

| Case | D's `must` | Failed in B's run | As merged |
| --- | --- | --- | --- |
| `bracket_shift_tol`, `_tol_id` (edge) | bore `no_feature` | both seats only: the anchors share the shift, 0.005 each, and the shifted circles sit at the coverage band's edge | `must`: `any_of` [the bore `no_feature` or `axis_offset`, the top seat, the bottom seat `contact_not_covered`]; all three in `may` |
| `bracket_shift_05`, `plate03_pin_shift_15tol`, `plate03_pin_shift_tol` | bore `no_feature` | the bore, and the seats around the moved hole | seats `contact_not_covered` in `may` |
| Hole-diameter cases (`plate03_press_15tol`, `nut_pinned_15tol`, `nut_pinned_tol`, `nut_clearance`, `block_cb_small`, `bracket_dia_large`) | the bore's diameter code | the bore, and the seat whose region the changed circle cuts | the seats in `may` |
| `block_cb_deep` | floor seat `no_feature`, grip | also the bore `:max` (the Ø11 now reaches into the Ø6.6 span) | the bore in `may` with `:min`, `:max`, `wall_missing` |
| `bracket_thin_15tol` | grip | grip, seats, bores (`wall_missing`, and `no_feature` where the shortened wall left no stack on the axis) | bores in `may` with both codes |
| `csink_variant` (fits) | none | the bore `unverifiable` (`zone_not_read`) | with M4-1: `InterfaceMatch`. Without it: `Reject`, a baseline entry (`fn_rule`) |
| `csink_82`, `csink_angle_15atol`, `csink_small_rim`, `csink_cbore` | the conical seat (`angle_out_of_limits`, `contact_not_covered`, `no_feature`) | the conical seat, the grip, the bore (zone), and for the small rim the plane seat | codes renamed `cone_mismatch:angle` and `:range`; the grip, the plane seat and the bore's `:profile` in `may`; without M4-1 the bore's `unverifiable` in `must` |
| `bracket_dia_large`, functional | none (`fits_functional`) | both seats (the wider hole cuts the target face) | `Reject` under functional too (M4-7): a baseline entry |

## 16. The harness (KH)

### 16.1 Running a case

- **KH1.** **Two paths.** A file candidate runs end to end through the binary: `fitment socket <source> --part P -o S.json`, `fitment review S.json --remove … --note …` for each review entry, then `fitment match S.json cand.stp --format json [policy flags]`, with its exit code. A component candidate runs through the library: `SocketDocument::extract`, `SocketDocument::decide` for the review, the source's component `Part` from `read_assembly`, then `Matcher::judge_part`. Both read the same expectations, and the committed subset runs both, so they cannot drift.
- **KH2.** **The strict gate**, per case and policy:
  1. the verdict equals the expected one (`collides`: `InterfaceMatch` with fit `not_checked`; with a fit verifier, `Reject` with `interferes`);
  2. F, the set of (locator, code) the evidence reports failed, satisfies `must ⊆ F ⊆ must ∪ may`, codes matched as sets: a failed requirement's codes must meet the expected codes, and a qualified code (`diameter_out_of_limits:min`) also checks which bound the measure reports;
  3. a `must` entry may be `{"any_of": [loc, …]}`: at least one of them fails. Which seat alignment slides onto, and whether a moved hole is caught by its bore or by a seat around it, are verification's to decide;
  4. a bad case (`does_not_fit`, `edge`) has a non-empty `must`, which always names the defect's own requirement, alone or within an `any_of`; an `fn_rule` case needs F non-empty and inside `must ∪ may`;
  5. every removed requirement is listed as not checked, and no check names it;
  6. the evidence's policy is the run's.
  A bad case that fails 1 to 4 fails the test outright, whatever the baseline says.
- **KH3.** **No placement.** If alignment finds none, the evidence still names, per requirement, why: the requirements no candidate feature could anchor fail with `no_placement`; F is that set. A bare "no placement" with no requirement named fails KH2.

### 16.2 The false-negative baseline

- **KH4.** **What it holds.** `crates/fitment/tests/corpus/false-negatives.json`, committed: one entry per (case, policy) whose truth fits under that policy (`fits`, `fn_rule`, and `fits_functional` under `functional`) and whose verdict is `Reject`: `{case, policy, class, failed: [{locator, codes}], cause}`, sorted. `cause` is `rule` (an `fn_rule` case failing its expected set), `alignment` (verification passes at the construction's placement, KH8) or `verification`. The file records `match_rules_version`, `signature_version`, `extractor_version`, `socket_rules_version` and the digest of the manifest's match cases.
- **KH5.** **Blessing.** Any difference between the run and the baseline fails, listing entries added and removed. `FITMENT_BLESS=1` rewrites it, refused when `CI` is set, and refused when an added entry's case existed before and `MATCH_RULES_VERSION` has not risen: a new false negative must come from a new case or a rule change, never from an unrelated edit.

### 16.3 The report

- **KH6.** **Rates.** The corpus test prints, and writes to `target/fitment-corpus/report.json` (uploaded by CI), per policy and per class: cases, `Match`, `InterfaceMatch`, `Reject`, false positives, false negatives against the specification (good cases rejected for no stated rule: bugs, alignment misses, edges read wrongly) and against physics (`fits` and `fn_rule` rejected). The exit criterion reads off two numbers: **false positives (must be 0)** and **the false-negative rate against physics**. `edge` cases are a row of their own, outside both rates. The tolerance sweep's rows are marked "not gated".
- **KH7.** **The zero-FP gate holds under `default` and `functional`,** the two policies M4 offers (M4-33). D7 `warning`, deferred to M7 with its cases (a grip-only thickness change, and `b_float_lip`, s02's block whose deeper counterbore puts a lip where a reseated head would sit), is gated there.

### 16.4 Cross-checks

- **KH8.** **Where a false negative comes from.** For every good case that rejects, the harness calls `Matcher::verify_at` with the construction's placement: a pass there means alignment missed it (`alignment`); a failure means verification (`verification`, or `rule` when expected).
- **KH9.** **The shortlist is sound.** For every case, the shortlist is run on the candidate's signature; if it drops the candidate, full verification (which runs anyway, M4-17) must reject it too, and a good case must never be dropped by the shortlist alone. A shortlist drop of a good case is a hard failure, never a baseline entry. Every case also runs with the shortlist off (a hidden `--no-shortlist`), and the verdict must not change.
- **KH10.** **The original run** is KC11's set, asserted per run as VR21 states: the failed requirements are exactly those failing every candidate, with code `unverifiable` or `floor_above_tolerance`. It replaces the deferral `Y4.original_run` in `corpus.rs`.
- **KH11.** **Determinism.** Every run twice in one process: byte-identical match documents; and on the three OSes of the `test` job, from the corpus artifact, with the reports' counts compared across OSes.
- **KH12.** **Read-back.** Every match document fitment writes for the corpus passes a structural checker in the tests (envelope, orders, the digest recomputed, every id resolving, EV7's recomputation). There is no reader in core (M4-23).
- **KH13.** **Budgets and listing.** The committed candidate set fits its cap and lists exactly its files.
- **KH14.** **Coverage.** The coverage test's `design_rules` accepts two-letter prefixes (`[A-Z]{1,2}[0-9]+[a-z]?`, at most 5 characters, followed by `.`) and reads `docs/research/m4-match.md`. Every CF, SG, SL, AL, VR, VD, EV, MF, KC, KH and KO rule needs a case, a test or a reason. Two lists need cases, not tests alone: rules that yield an unverifiable requirement, a `floor_above_tolerance` or a refusal (VR2, VR4, VR6, VR14, VD5, VD6, VD9, VD10, CF2, MF2), as M3's `UNVERIFIABLE` list does, shown by the cases of KC2's floor and gate fields; and `CAN_FAIL`, every rule that can fail a candidate (CF4, CF5, CF8, VR3, VR4, VR7 to VR13, VR22), each needing a bad (`does_not_fit` or `edge`) case that cites it, so no check can exist without a candidate it rejects (RS10). These lists join the test when their cases land (PR 5); until then, and while M4 is open, the check honours a deferral to a named later M4 PR, which the exit PR removes; after it only reasons outside M4 may wait (M7's fit; `collides`' `Reject`).

### 16.5 CI

- `corpus` job: `gen-corpus.py corpus/out` (now with candidates, about 1 s more); `verify-occt.py manifest` (KO1, the labels); `verify-occt.py match corpus/out` (KO2 to KO5 on fitment's verdicts); `verify-occt.py mutations` (gains KO6); `verify-occt.py match --expect fixtures/manifest.json fixtures` (KO8); `cargo test --test corpus --test fixtures` with `FITMENT_REQUIRE_CORPUS=1`; uploads the corpus and the report.
- `unit-set` job: regenerates `tests/candidates` and diffs it (`--candidate-set --check`).
- `test` job on three OSes: the committed subset and the corpus artifact; the reports compared across OSes.

## 17. The oracle (KO)

`tools/verify-occt.py` gains a `match` mode and match mutations, on Open CASCADE 8.0.1, a kernel the generator does not use.

- **KO1.** **Physics at the construction's placement** (a generator check, `manifest` mode). The candidate is moved by the occurrence's transform composed with the case's placement into the source assembly; for every other leaf N: dV = vol(C ∩ N) − vol(T ∩ N), with T the original occurrence, and dA = contact(C, N) − contact(T, N). C fits when no dV exceeds 1e-6 mm³ where vol(T ∩ N) is 0, an interference fit's |dV| stays within the engagement the tolerance allows (a radius change of tol/2 over the original's common surface), and no dA falls below −(tol × the original contact's boundary length). The verdict must agree with the label: `fits` and `fn_rule` fit, `does_not_fit` and `fits_functional` (strict) do not, `collides` differs only by volume that lies outside every requirement's region, computed by the oracle from the socket (outside each bore's core over its spans and out to its required openings, as VR22 defines it; outside each shaft's neighbour-hole region over its spans; outside each seat region's prism within the near band of its plane), `edge` is recorded either way. A volume with a cited neighbour inside such a region is never excused: that is a defect, not a collision (RS9). A disagreement is a generator bug: the label or the part is wrong.
- **KO2.** **One thing broken.** A bad case's physical differences involve only the neighbours its `must` requirements cite as sources: `bracket_dia_small` differs with a BOLT only, `block_cb_small` with the SCREW only. A defect that disturbs another neighbour is not one defect.
- **KO3.** **Every pass, physically** (`match` mode). For each `InterfaceMatch` or `Match`, at the placement fitment's evidence states (per socket, for every occurrence), KO1's physics must say fits (strict or functional as the run's policy says; under functional the ring between the original and the candidate hole about each clearance bore is excused). This is socket-free: a requirement the M3 socket missed, or a check verification got wrong, shows here as a new interference or a lost contact. Under `collides`, only volume outside every requirement's region (KO1) is excused, and tallied as M7's.
- **KO4.** **Every failure, re-measured.** For each failed check with a measure, the oracle measures the same quantity on the placed candidate: a cylinder's axis and radius from the faces whose `#id`s the check names (faces bind through XCAF as in M3's oracle), or, with no feature named, the nearest coaxial cylindrical face within 1 mm; a plane face's offset and the area of the required region it leaves uncovered; a cone's apex and angle; a grip's separation. The stated `found` must agree within 1e-6 mm (1e-9 rad), and must violate its stated bound.
- **KO5.** **Placements.** For each passed check naming a candidate feature, that feature, placed, lies on the requirement's axis line or plane within the tolerance; the placement is rigid (orthonormal within 1e-12, determinant +1).
- **KO6.** **Mutations of fitment's match documents**, each required to meet a disagreement somewhere in the corpus, as M3's run does: a `Reject` turned `InterfaceMatch` (KO3 on every bad case), a placement moved 0.01 mm across an axis or turned 0.02° (KO3 on pinned bores, KO5), a placement turned 180° about a hole axis (KO5), a failed check's `found` changed by 0.1 (KO4), a passed check marked failed (KO4), failure codes swapped between two checks (KO4), a check dropped (`MissingCheck`), and a policy name changed (KH2.6).
- **KO7.** **What it cannot check.** Threads and press fits beyond geometry; materials and other properties; review semantics; the shortlist; whether no placement exists (it knows one placement for good cases, KH8, and checks only the placement fitment chose for the rest); policies beyond D6's ring; what unverifiable requirements mean; what both kernels misread alike (both are Open CASCADE). Body fit is reported, not judged, until M7. The probe measured contacts by exact coincidence, so a face within the tolerance but not coincident read as lost; the real oracle measures contact as M3's does, by projecting a face onto the other's plane within the band (`common_area`), and by sampled distance for cones and cylinders.
- **KO8.** **Fixtures.** For AS1's cross-export runs (KC12), the physics layer places each candidate export's component into the socket export's assembly; the tallies (cases, passes checked, contacts, interferences) are pinned under `match_oracle` in `fixtures/manifest.json`, as `socket_oracle` is.

## 18. Y4: the original against its own socket

PLAN's M4 prompt: the original part, run as a candidate against its own socket, must fail exactly its unverifiable requirements. M3 deferred the run to M4 (`Y4.original_run` in `corpus.rs`); M3's half, `Y4.witness`, already asserts that every verifiable limit contains the original's own value.

- **VR21.** **The original's run.** For every corpus socket and every pinned fixture socket, the target's own component (read in place from the assembly the socket came from) is matched as a candidate, from scratch, with no seed: in its own frame; the copy from the `_tgt` variant (the same part modelled in TILT) and from `_pc` against the base's socket; and the original against the `_tgt` socket. Each must pass every active requirement that is M4-verifiable (VR6) and not failed by VD6, and fail every other one with `unverifiable` or `floor_above_tolerance`. The run is repeated with the identity given (AL14), which must pass the same set. A socket that exhausts alignment unseeded is allowed only by name, with its reason, and must pass seeded: today only s55's plate, a seats-only socket on 3,600 coplanar tiles under a block, whose landmark placements (AL9) run past every bound before reaching the tiles under the block, as M3's oracle skips s55 by name for its 7,440 faces. The run uses the shipped `MAX_PLACEMENTS` and `MAX_COVERAGE_STEPS`, so no budget becomes an unnamed exception, and covers every policy of the matrix. Targets with no `_tgt` or `_pc` variant (s10's three, s22's BLOCK, s55's two) run their own frame only. It is M3's deferred `Y4.original_run`, VD15's self-check and KH10 together.

Measured with the split floor and B's rules as they stood before the reviews (`B/out/y4.txt`, `y4_verbose.txt`, `y4_seeded.txt`, `fix.txt`), at 20,000 placements and 4·10⁸ coverage steps; the `_pc` copies were not run:

| Set | Runs | Socket runs | Y4 holds | Notes |
| --- | --- | --- | --- | --- |
| Corpus, unseeded | 210 (74 targets: 68 × own, tilted original, tilted socket; 6 own only) | 225 | 224 | s55's plate: 20,000 landmark placements, 1.2·10⁸ coverage steps, 28.9 s, none on the tiles under the block |
| Corpus, identity given | 210 | 225 | 225 | 4.1 s in all |
| Fixtures, unseeded | 36 targets | 78 | 78 | 20 pass clean (AS1 ac 6, md 7, ug 7); 5 fail only on the floor (pe); 53 carry requirements failing every candidate |
| Both, functional, t = 0.05 and 0.002 mm | | | stated to hold (B.md 7); outputs not preserved | Not measured as a kept output; first measured in PR 3. By the rules, VR4 uses τ, not t, so loosening t never fails the original's clearance spans. (B also ran grip warning, which M4 no longer offers) |

What the original fails on the fixtures, by the split floor (`B/out/fix.txt`): `as1_pe`'s 22 M4-verifiable requirements in its 6 sockets (u = 0.115 to 0.672 mm); BEARING_208's 65 (0.0165 mm) beside its 54 unresolved; vaccase's tee's 23 (0.13 mm) beside 9 unresolved and one zone span. AS1-md's 25 verifiable requirements, and SPACER_220's, NUT_244's and LINKAGE_ARM_SHORT_46's 67 (parts 0.0025 to 0.0036 mm, requirement floors up to 0.058 mm), are checked at ε = 0.009998 mm (md) and 0.0064 to 0.0075 mm (the weldment), and the originals pass every one. A single floor (C's VD6 as first written, D's D-10) would have failed all of these, the originals with them, on requirements M4 can verify.

The prototype's first runs broke Y4 on 118 of 210 runs; each cause became a rule: a plane's deviation measured at arc points off the plane (VR3's arc term); a whole circle whose endpoints rounded apart wound zero times from inside (VR11); a turn about −d computed as a turn about d (AL8); a grip between unknown seats with nothing to locate it (VR6); a floor failure on an axis that skipped the seats (AL10). C's prototype failed the original four other ways, each now a rule of B's: span limits compared against a segment that only touches the span's end (s02's counterbore step), a seat region whose outer loop is a circle (a washer), a grip checked before its second seat was located, and coverage without tolerance.

The shortlist keeps every original: 157 of 157 sockets, as extracted and with every unverifiable requirement removed, in its own frame and under 20 random motions each (`A/out/report.txt`, `report-vd6-off.txt`).

Measured since: VR22 and VR4's guard on every original (VR22's paragraph: 209 of 209 spans clear; 4 requirements unverifiable by U11 as extended). Not yet measured: the full run with every reviewed rule at once (VR7's rim and wall openings, VR8's post and zone conditions, SR7's widened span ends; section 28 argues each keeps Y4), and the run with M4-1's socket changes. It makes s18's plate bore verifiable (checked against the original's countersink profile, which the original meets exactly) and moves s18's screw, s58's plate, vaccase's tee and the AS1-ec and tc grips from M4's own `zone_not_read` and `grip_seat_unverifiable` to `verifiable()`; the set of requirements the original fails changes: s18's plate bore leaves it (verifiable, and passed), and four requirements its original passes today join it, unverifiable by U11 as extended (s50 CAP's skirt bore and spigot, s54's two pins). Y4 under VR22 is measured (VR22, `merge/y4probe/y4-vr22.txt`).

## 19. Worked examples

Unless a line says otherwise: the default policy (strict, grip hard, 0.01 mm and 0.01°); "kept" counts the pool of 291 candidates (`A/out/report-vd6-off.txt`, which for these sockets equals the split floor's); verdicts are B's prototype's (`B/out/dcands.txt`); physics is D's oracle at the construction's placement (`D/oracle.txt`).

### 19.1 s01 BRACKET: two parallel axes

**Socket.** Four bores [0, 8], Ø6.6, clear 6 (the bolts' Ø6 shanks), clearance, open at the bottom; seat +z at 8 (four heads), seat −z at 0 (the plate), grip 8; the Ø8 hole left out (`nothing_on_axis`). All seven requirements verifiable; both parts state 1e-7 mm, so u = 1e-6 mm and ε = 0.009999 mm.

**Signature and shortlist.** Four hole lines, each one span with window [5.99, 6.61], one opening, extent at least 8 − W_L; two seats with the area of the 80 × 50 face less its holes; the grip as a separation of 8. Pairs: axes parallel at 30, 60 and 67.08 mm; seat normals parallel to every axis; seats opposed at 8. W_L = 0.02 + 4 × 94 × 1.75e-4 = 0.086 mm. **Kept 11**, all copies of this bracket in other corpus cases (s07, s07b, s08, s08x, s11, s23 to s25, r21's two). Under D7 warning, which M4 no longer offers (M4-8), 21 were kept: without seat separations, plates of the same hole pattern and other thicknesses pass.

**Alignment.** The farthest pair of bore lines (a diagonal, 67.08 mm) anchors; the candidate's Ø6.6 pairs at that separation, each sign, slid by the seat with fewest matching planes: 8 placements. The bracket is symmetric under a half turn, so more than one passes. Checks of the original: each axis within ε, the Ø6.6 run covering [0, 8], radial margin 0.3 > τ = 0.01, 6.6 < 6.6 + ε, open at the bottom; both faces covered (3·10⁵ coverage steps, 75 ms); grip 8.

**D's slice** (in the VARIANT frame unless named):

| Case | Truth | Verdict and what failed | Physics |
| --- | --- | --- | --- |
| `orig`, `variant`, `tilt_inch`, `split_pc` | fits | `InterfaceMatch` | nothing differs |
| `side_hole`, `larger` | fits | `InterfaceMatch` | nothing differs |
| `shift_half_tol` (one hole moved 0.005) | fits | `InterfaceMatch`; by VR11's rule only if the moved hole is in the anchor pair (merge finding 1; derived, not measured: the prototype did not log the pair) | plate contact −0.033 mm² (allowed −3.68) |
| `lug` (a tab off a side face, 5 mm into the plate) | collides | `InterfaceMatch` (fit not checked); M7 rejects | PLATE dV +300.00, no contact lost |
| `top_pocket` | fn_rule (R14) | top seat `contact_not_covered` | nothing: the heads' rings are intact |
| `shift_15tol` | fn_rule (position) | both seats (0.006 mm inside the region) | nothing: 0.3 mm of clearance |
| `tilt_hole` (1°) | fn_rule (undecided) | the bore `no_feature`, both seats | nothing |
| `shift_tol` (exactly 0.01) and `_tol_id` | edge | both seats, at the coverage band's edge: the anchors share the shift, 0.005 each, so the bores pass and the seats catch it | an edge |
| `dia_clear_edge` (Ø6.01) | edge | the bore `:min`: radial margin 0.005 < τ (VR4) | an edge |
| `shift_05` | does_not_fit | the bore `no_feature`, both seats (the shifted circle cuts 0.5 mm into each face) | BOLT dV +6.13 mm³ |
| `dia_small` (Ø5.9) | does_not_fit | the bore `:min` | BOLT dV +7.48 |
| `dia_large` (Ø6.8) | fits_functional | the bore `:max`, both seats; under functional both seats still (M4-7) | head and plate contact −2.10 each |
| `missing_hole` | does_not_fit | the bore `no_feature` | BOLT dV +226.19 |
| `blind` | does_not_fit | the bore `wall_missing` and not open | BOLT dV +56.55 |
| `face_hole` (Ø4 at (0, 15)) | does_not_fit | both seats `contact_not_covered` | PLATE contact −12.57 |
| `thick` (8.5) | does_not_fit | the grip, the bottom seat `no_feature` (alignment slid onto the top) | each BOLT dV +26.20, head contact −52.39 |
| `thin_15tol` | does_not_fit | the grip, both seats, the bores' walls | each head contact −52.39 |

Two things the table shows. A clearance hole moved past the tolerance but within its clearance fits physically, so the "just past tolerance" bad cases sit on pinned and contact requirements, and clearance shifts are `fn_rule`. And a defect is often caught by a requirement next to the one it was aimed at: a thickness by a seat as well as the grip, a hole shift by the seats rather than the bore. Hence `any_of` and `may` (KH2).

**C's 14 candidates** (`C/s01_match.json`, identity alignment, crude checks) show the document's shape. Under the merged rules most verdicts stand; three change or depend on the placement:

| Candidate | Prototype | Under the merged rules |
| --- | --- | --- |
| c00 original; c10 holes Ø6.605; c08 plate 100 × 60; c09 no Ø8 hole | `InterfaceMatch` | the same (c10's circles lie 0.0025 inside the region, under ε/2) |
| c12 one hole 0.009 off | `InterfaceMatch` | the bore passes (0.009 < ε); the seats reject unless the moved hole anchors the placement, since its circle lies 0.009 inside the region (VR11; not measured) |
| c11 holes Ø6.61 | `Reject`, margin −8.9e-16 | `Reject`, margin −1e-6 (the limit is 6.6 + ε), and both seats; under functional the seats still reject, where the prototype passed it |
| c06 blind; c04 missing; c13 plate 70 × 40; c07 extra holes in the face; c01 hole 0.5 off; c03 Ø5.5; c05 6 mm thick; c02 Ø7 | `Reject` | the same, with the same requirements |

c07 is worth stating: PLAN allows extra holes on a candidate, but a seat's region is the target face (R14), so an extra hole inside that face rejects. D's oracle independently calls such a case (`bracket_face_hole`) `does_not_fit`: the plate loses real contact. Whether that is a false negative depends on whether the hole lies under the neighbour's actual contact; the evidence names the opening and its place.

### 19.2 s02 BLOCK: one axis, a square seat

Socket: one bore with spans [0, 8.5] clear 6 in Ø6.6 and [8.5, 14.5] clear 10 in Ø11; seats −z at 0 (the 40 × 40 base face less the hole) and +z at 8.5 (the counterbore floor, ring Ø6.6 to Ø10 from H1); grip 8.5. **Kept 11:** this block's copies and the corpus parts with that counterbore (p05, p14s, p16, p17, p18, p20, p37). Both diameters 0.5 smaller is still kept (6.1 and 10.5 lie inside their windows): a legitimate strict pass. The bottom seat is not symmetric about the axis, so turns come from its four edges against the candidate's (AL8): 8 placements; `block_variant` passes at the fifth. `block_cb_small` (Ø9.9): the head span fails (margin −0.05) and the floor seat (SCREW dV +9.38); `block_cb_deep` (7 deep): the floor seat `no_feature`, the grip, and the bore `:max` where the Ø11 now reaches into the Ø6.6 span (SCREW head contact −44.33); `block_no_cb`: the head span, the floor seat and the grip (SCREW dV +265.97). Under functional a Ø11.5 counterbore passes the head span; under strict it fails `:max`. The screw (one symmetric axis): `screw_long_1` passes (1 mm of its 1.5 `max_extra_mm`); `screw_long_25` also passes the interface and bottoms out 1 mm into the floor: `collides`, M7's (M4-11; `B/out/bcands.txt`).

### 19.3 s03 PLATE: pinned, line to line, clearance

Four bores [0, 8]: Ø5.97 pinned (interference), Ø6.2 clearance [6, 6.2], Ø6 and Ø6.015 pinned (line to line); one seat. **Kept 1**, the plate itself; a diameter 0.02 smaller drops it. Anchors: the farthest pair (the two pinned outer holes, 60 mm). The Ø6.015 bore accepts diameters strictly between 6.005001 and 6.024999, and every pass carries `thread_not_confirmed`: a press fit and a plain hole cannot be told apart. `plate03_variant` passes; `pin_shift_15tol` and `pin_shift_tol` fail the bore `no_feature` (a pinned hole cannot move: 0.01 is the edge) and the seat (DOWEL dV +0.72 and +0.48; 150.80 mm² of line-to-line contact lost); `press_15tol` (5.985) `diameter_out_of_limits` and the seat (DOWEL dV −1.13: less engagement); `clear_shift` (the Ø6.2 moved 0.05) the bore and the seat, though physics says it fits (`fn_position`, M4-9).

### 19.4 s05: the stack, and washers that seat on each other

M (6 mm plate, one Ø6.6 hole, seats both sides): one line open at both ends, grip 6: **kept 2** (itself and p10). T (8 mm): **kept 13** (the s01 brackets have the same hole, thickness and seat orientation). Under D7 warning (measured; not offered in M4) M kept 47 and T 46: grips carry most of these sockets' selectivity.

WASHER: two sockets (the washers at z 24 and 25.6), each: bore [0, 1.6] Ø6.4 clear 6, `self`; a shaft on the Ø12 outside (`abutting`, `self`, no spans); two seats (one against the other washer, rings Ø6.4 to Ø12); grip 1.6. **Kept 1** in each. Every requirement is symmetric about the axis: θ = 0, 2 placements (the washer flips). B's candidates (`B/out/bcands.txt`): `washer_good` and `washer_hole_62` (Ø6.2, inside [6, 6.4]) pass both sockets; `hole_65` fails `:max` and both seats (0.05 mm into each face); `thick_17` the grip and a seat in both; `outside_off` (the outside 0.5 off) the bore and both seats, as the seat regions are annuli about the original axis. A candidate replaces both washers, so it must pass both sockets, each with its own placement; the `self` relations are pinned by M3 on both sides, and D's physics places the candidate at both occurrences at once, so the washer-to-washer contact is measured between two copies.

### 19.5 s07b: two sockets of one part

The bracket twice, one bolt and nut missing from the second, so socket-2's `bore:hole-838` is pinned Ø6.6 (`not_read`, M3's J1) where socket-1's is a clearance bore. D's `b_some_sockets`, every hole Ø6.4, passes socket-1 (6.4 < 6.6 + ε, margin 0.2 > τ, the smaller holes leave the seat regions covered) and fails socket-2's pinned bore: `Reject`, with socket-1 `interface_match` and socket-2 `reject` in the evidence (predicted by the rules; not measured). A verdict that looked at one socket, or that merged checks by requirement id across sockets, would pass it; ids are qualified by their socket (`socket-2/bore:hole-838`). C's prototype showed the same with c11 (Ø6.61) under functional; under the merged rules c11 fails socket-1's seats too, so D's case is the example.

### 19.6 s18: the countersunk screw

The plate's hole has a 90° countersink under the screw's countersunk head. Socket (PLATE): a bore, a conical seat (apex, angle 90°, the radial range both cones share), a seat on the plate's opposed face, and a grip from the cone's apex to that seat (M3's L2). The bore's piece in the countersink zone has no pin and no clear diameter: the head's cone occupies it, read as a conical seat, not as a shaft. **Kept 2** (p06 and the plate). Without M4-1 that piece is unverifiable, and every candidate fails it, the original included: `csink_variant` (the original in another frame) rejects with only `unverifiable:zone_not_read`, B's one disagreement with D's 40 expectations. With M4-1 the socket states the original's profile over the piece (Ø6.6 widening to Ø12.4 along a 90° cone), VR7 checks the candidate against it within ε, and `csink_variant` passes. `csink_82` and `csink_angle_15atol` fail the conical seat's angle and the grip, `csink_small_rim` its range, the plane seat and the grip, `csink_cbore` (a Ø12.4 counterbore in its place) the conical seat and the grip. A conical boss where the countersink was (`b_cone_sense`, not yet built) has the same apex, axis and angle and the other sense: VR13 needs SR2 to reject it by the cone alone. The screw's socket has a span past its shank's known end (the head flares where the walk could not read it): unverifiable either way (VR6), so the screw fails every candidate until review.

### 19.7 AS1

ug's `l_bracket`: two occurrences, one socket; four Ø10 bores pinned line to line (`thread_not_confirmed`; the rod hole `self`, open both ends); the three bolt holes along y form a triangle of side 25.98 mm; the rod hole along x crosses them, at signed distances −40 and 30 mm from the two seats (normals −y at 0, +y at 10); grip 10. **Kept 3**: ac's, md's and ug's brackets, whose invariants agree to 9.6e-14 mm. The plate keeps its three millimetre exports likewise (six lines at 25.98, 85, 88.88, 108.28 and 130 mm; grip 20); the nut keeps 20 (any part with a Ø10 hole at least 15 long and a seat).

Alignment is by the rod hole across a bolt hole (AL6): 8 placements, 40 to 125 ms each. Across exports (`B/out/as1x.txt`): ac, md and ug candidates pass all three of ac's, md's and ug's sockets, 9 of 9. pe's parts state 0.115 to 0.672 mm (the bracket 0.363), so pe's socket and pe's candidate fail every comparison with `floor_above_tolerance`; at a 0.8 mm tolerance pe passes its own socket (Y4) and none of the others: its geometry is not theirs. Broken from ug's part (`B/gen_cands.py`): one bolt hole filled gives that bore `no_feature` only (the next anchor pair takes over, AL4); moved 0.5 mm, that bore and both seats; opened to Ø10.5, that bore `diameter_out_of_limits` (pinned) and both seats. The same three against ac's socket fail the same way. ec reads only undecided cylinders and tc no stack (B-spline faces): every ec and tc socket has requirements failing every candidate, and their candidates have no decided hole. Across exports, the candidates agree but for ug's bolt, 37 mm long where the others are 40: the exporters' geometry differs, as M3 found for the rod nuts.

### 19.8 The weldment and vaccase

The weldment's four targets have 30 sockets and 304 requirements, 172 of them unresolved (15 to 54 per document), so no weldment socket can pass before review. BEARING_208's part states 0.0165 mm, above t, so its 65 other requirements fail `floor_above_tolerance`. SPACER_220, NUT_244 and LINKAGE_ARM_SHORT_46 state 0.0025 to 0.0036 mm, so their 67 other requirements are checked at ε = 0.0064 to 0.0075 mm, and the originals pass every one although their requirement floors reach 0.058 mm (the placement path). Vaccase's tee (0.13 mm) aligns by two skew bores of its cross and fails its 9 unresolved requirements, one span past a known end and, by the floor, its other 23. For the shortlist, the single floor emptied every weldment socket (its 0.058 mm placement floor). Under the split floor BEARING_208's 13 sockets, `as1_pe`'s 6 and vaccase's tee contribute nothing, and the other three weldment targets keep what they kept with no floor rule: LINKAGE_ARM_SHORT_46 only itself, NUT_244 2 to 10, SPACER_220 1 to 5 (BEARING_208 also kept only itself there).

## 20. What was measured

Probes are under [m4-evidence/](m4-evidence/), by designer. `A/probe` is a Rust program on `fitment-core`'s public API (`cargo run --release --manifest-path docs/research/m4-evidence/A/probe/Cargo.toml -- .`, about 2.8 s); `B/proto` is a Rust prototype of alignment and verification (about 1,500 lines); `C/` and `D/` are Python under `uv run` (cadquery 2.8.0, cadquery-ocp-novtk 8.0.1.1.0) plus `C/original/` in Rust. The corpus was regenerated with `tools/gen-corpus.py` (344 cases, 345 files, 17 MB, 8.5 s); the `corpus/out` committed in the checkout was stale (s58's five files were the old `s58_csink_spline`).

### 20.1 Sockets, the floor, and what can pass

| Over 157 sockets (79 corpus, 78 fixture) | No floor rule | Single floor (C's VD6 at 0.01 mm) | Split floor (M4-2; derived) |
| --- | --- | --- | --- |
| Sockets with a required line: count, kept median (mean) of 291 | 123, 3 (9.6) | 78, 10 (13.1) | 103, 4 (11.2) |
| Seats only: count, kept median | 9, 190 (282 without the area filter) | 9, 190 | 9, 190 |
| Nothing verifiable (all 291 kept by an explicit run; a catalog query blocked) | 25 | 70 | 45 |
| An active requirement failing every candidate | 64 | 64, and 45 sockets lose requirements to the floor | 69 (71 under VR6 without M4-1, 70 with it) |
| The original kept | 157 | 157 | 157 (the shortlist keeps it under any floor rule) |

The first two columns are A's runs (`A/out/report-vd6-off.txt`, `report.txt`). The third was not re-run: `merge/splitfloor.py` derives it from the first run's per-socket rows by blocking the 20 sockets whose target states its own uncertainty at or above 0.01 mm (`as1_pe`'s 6, BEARING_208's 13, vaccase's tee). The script reproduces both of A's columns from their rows. All three count by M3's `verifiable()`. B's census (`B/out/census.txt`), by M4's predicate and its anchor shape, counts 26 sockets with nothing M4 can verify, not 25, and 11 seats-only (a conical seat counted), not 9.

Socket documents (`C/unverifiable.txt`, written by the release binary):

| Set | Documents | Sockets | Requirements | Unverifiable (Y4) | Documents with a socket every candidate fails | Documents whose target part states ≥ 0.01 mm (`part_uncertainty_mm` in `C/sockets/`; `C/unverifiable.txt` counts 10 whose floor exceeds 0.01) |
| --- | --- | --- | --- | --- | --- | --- |
| Corpus | 74 | 79 | 205 | 25 | 11 | 0 |
| Fixtures | 36 | 78 | 699 | 437 | 20 | 7 (`as1_pe` ×5, BEARING_208, vaccase's tee) |

Fixture documents that can yield a pass as extracted: 12, the plate, bracket, nut and rod of AS1's ac, md and ug exports (17 sockets, `B/out/fix.txt`); every bolt document has an unresolved contact in its `socket-2`, though its `socket-1` passes clean (20 sockets pass clean in all, 3 of them bolts'). The single floor left 8 (md's four fail at exactly 0.01); no floor rule, 16 (pe's four too). Euclid and think3 (no hole read), the moon buggy, vaccase and the weldment need review first, as M3 warned.

### 20.2 Cost

| What | Measured | Probe |
| --- | --- | --- |
| A candidate's signature | median 0.6 µs, 95th percentile 5.6 µs | `A/out/report.txt` |
| Level 2 shortlist on real parts | 137,061 tests in 0.05 to 0.1 s (0.3 to 0.7 µs each); the slowest 0.45 to 0.9 ms; budget never reached | `A/out/report*.txt` |
| Level 1 on the pool | 17,991 tokens, built in 0.8 ms | `A/out/report.txt` |
| Synthetic catalog of 10⁵ parts (scaled 0.5 to 2) | 6.17 million tokens, built in 0.31 s; level-1 queries 0 to 2.5 ms, level 2 on the hits at most 6.8 ms | `A/out/report-vd6-off.txt` |
| 10⁵ exact copies, the worst case | level 1 returns up to 34,718 (s02's block: one line, no pair) in 4.5 ms; level 2 keeps 3,784 in 16 ms; s01's bracket returns 7,910 in 8.5 ms and level 2 takes 27 ms | `A/out/report-vd6-off.txt` |
| Hostile grid, s01's bracket socket | 40 × 40 identical holes: 5.9 million comparisons (budget reached, kept) in 64 to 85 ms; 100 × 100: 48 million in 0.7 to 1.2 s, because the probe counts per revision; SL4 counts per comparison | `A/out/report-vd6-off.txt` |
| A candidate against a socket (alignment and verification) | 1.2 to 125 ms; D's 40 candidates in 1.7 s; coverage dominates, ∝ perimeter / ε | `B/out/dcands.txt` |
| Placements generated | median 8, 90th percentile 36, largest 20,000 (s55, 28.9 s, the only run over a second) | `B/out/y4_verbose.txt` |
| A 400 × 200 plate with 169 holes against s01 | 1,832 placements, rejected in 45 ms | `B/out/bcands.txt` |
| Y4 corpus run | unseeded 30.9 s (28.9 of it s55), seeded 4.1 s; fixtures, the slowest target 0.92 s | `B/out/y4*.txt`, `fix.txt` |
| Candidate features | 2 ms each, 14.3 KB of JSON each (114 candidates) | `C/feat/` |

### 20.3 Mutations of the originals' signatures

Each original's signature, mutated, under 20 random rigid motions, over the 90 sockets with something verifiable (no floor rule; `A/out/report-vd6-off.txt`). In every socket and mutation the 20 motions agreed.

| Mutation | Dropped in | Note |
| --- | --- | --- |
| none (moved only) | 0 | invariance |
| one hole moved 0.5 mm | 23 | sockets with two parallel lines or a plane parallel to the axis |
| moved 0.05 mm | 4 | where the window is narrow (no lever) |
| moved 0.015, 0.005 mm | 0 | 0.015 is verification's to reject (beyond σ, within W_L) |
| diameter −0.5, −0.02 | 66, 62 | the rest are wide clearance windows the smaller hole still meets |
| diameter +0.005 | 0 | within tolerance: must be kept |
| a hole missing | 82 | the rest have no stack or do not use that one |
| a through hole made blind | 31 | where both ends must open |
| 2 mm thinner, 0.005 mm thinner | 63, 0 | |

No within-tolerance mutation was ever dropped, which is the soundness side. Policy sweep, mean kept over the 90 sockets: strict 20.4, functional 23.1, D7 warning 22.0 (not offered in M4), 0.05 mm and 0.05° 20.6, 0.1 mm and 0.1° 21.1, 0.672 mm 29.0. Functional loosens screws and pins most (s02's screw 3 → 16, s41's 1 → 38, s19's housing 2 → 37).

### 20.4 The corpus slice, edges and the physics oracle

- **The slice** (`D/gen_candidates.py`, `D/out/`): 40 candidates against 5 sources (s01 BRACKET 22, s01 NUT 4, s02 BLOCK 4, s03 PLATE 5, s18 PLATE 5), 709,313 bytes, written in 0.22 s; all 40 accepted by `fitment features` in 0.13 s. Each defect is visible in fitment's own features moved into the target's frame (`D/readback.txt`): the hole at (30.015, 15), Ø5.9, the blind hole's `bottom`, the plane at 8.5 or 7.985, the counterbore floor at 8, the cone at 82° with its apex at 2.868, the rim radius 5.5. The 1° tilted hole reads as an undecided cylinder (`no_end_circle`) and the part as `complete: false`.
- **Placement precision:** written in VARIANT, every feature lands within 3e-12 mm of the construction when the placement is unrounded; rounded to 9 decimals, 1.3e-8 mm.
- **Edges** (`D/edge_frames.py`, `D/edge.txt`, three edge cases in 40 seeded skew frames):

| Edge | Measured minus edge | Read inside the edge |
| --- | --- | --- |
| Hole moved exactly 0.01 mm | −7.4e-11 to +6.5e-11 mm | 21 of 40 |
| Diameter clear + 0.01 (6.01) | −2.1e-16 mm, every frame | 40 of 40 |
| Countersink 90.01° | −5.5e-12°, every frame | 40 of 40 |

  A naive `deviation < tol` passes edge cases about half the time; VR2's guard (u ≥ 1e-6 mm) and VR4's τ margin reject them all, and every case at half the tolerance stays inside.
- **Physics oracle** (`D/oracle_match.py`, `D/oracle.txt`): 1.4 s for 40 cases (0.01 to 0.16 s each). Its first run found 3 of 35 truth labels wrong: `bracket_face_hole` (called "fits, R14's cost"; the plate loses 12.57 mm² of real contact), `bracket_shift_15tol` and `bracket_tilt_hole` (called bad; their bolts pass with 0.3 mm of clearance). A first version that counted only plane contacts called `csink_82` and `csink_cbore` "fits"; counting every coincident face fixed it. After relabelling, 40 of 40 agree.
- **Sizes:** the 13-file committed subset is 187,015 bytes in MAPPED and 234,700 in VARIANT (`D/size_frames.py`).

### 20.5 Documents

Socket documents (`C/sockets/`, all 110 written with exit 0 in 0.8 s): corpus median 11.4 KB, largest 433 KB (s55); fixtures median 51 KB, largest 619 KB (BEARING_208: 13 sockets, 119 requirements, 356 sources).

| Match run (C's prototype) | Candidates | Pretty JSON | Minified |
| --- | --- | --- | --- |
| s01, 14 named, full evidence | 14 | 161 KB | |
| s01, 14 named, compact rejects (M4-19) | 14 | 85 KB | |
| s01, 100 bulk, full | 100 (6 pass) | 1.17 MB | 485 KB |
| s01, 100 bulk, compact | 100 | 956 KB | 409 KB |
| AS1-ac L-BRACKET, 100 bulk, all reject | 100 | 675 KB | |
| BEARING_208, 100 bulk, all reject, 119 checks each over 13 sockets | 100 | 8.0 MB | 3.9 MB |

Per check (minified, s01): a passed bore with its measures 654 bytes, a failed one 788; a seat 305 passed, 443 failed; a grip about 250; a candidate's header 530. The document is O(candidates × socket size); a catalog run bounds it further by the shortlist. The merged measures add `clear_margin_mm` beside `diameter_mm` and the band fields, about a fifth more per bore; not re-measured.

## 21. Requests to M3

`fitment.socket` v1 freezes at M4's exit (M4-28). What M4 needs of it must therefore land before: the socket PR, M4's first (M4-1), raising `SOCKET_RULES_VERSION` to 6. The requests are numbered SR1 to SR9 (socket requests; B's report called the first four F1 to F4, which collide with M2's rule ids). Each becomes a rule of m3-socket.md, with an id in `RULES`, and the socket PR edits m3-socket.md (8.1's Y4 list, 8.4's fields, 9.3's example) and ARCHITECTURE's "Soundness" to state them.

- SR1, span basis and zone profile (new rule X8). Each bore or shaft span piece states its basis: `cylinder` (on a segment's own cylinder), `zone` (a transition or end zone of M3's X7), or `past_known_end` (U3's stations beyond a known end). A `zone` piece with no pin and no clear diameter states the original's diameters at its two ends and whether a cone joins them (else unknown, and the piece is unverifiable). VR7 then checks the candidate's profile against the original's within ε, and s18's plate becomes verifiable.
- SR2, a conical seat's target sense (`hole` or `shaft`; new rule Q3), so VR13 can tell a countersink from a boss without inferring it from a bore on the axis.
- SR3, optional: the end `max_extra_mm` is measured from and the original's rim station there. With it a later rule could check bottoming; M4 does not (M4-11).
- SR4: public `SocketDocument::active(socket)`, `removed(socket)` (designed in M3, section 11, not built) and `contact_mm()`.
- SR5, one predicate. `Requirement::verifiable()` returns false also for: a bore or shaft with a `past_known_end` piece that is neither pinned nor bounded, or a zone piece without a profile (new rule U9); a grip either of whose seats is unverifiable (U10: a new grip flag `seat_unverifiable`, set at extraction, so the predicate stays a method of the requirement); a bore or shaft whose span's core, as VR22 defines it (a shaft's unbounded), holds the original's own material: a post in a bore, a skirt round a spigot, a second pin beside a pin (U11: the original's core is not clear; a reviewer may remove it after looking); and a conical seat with no sense (Q3). M3's `Y4.witness` test and M4's VR6 call it, so they cannot disagree. U9, U10 and U11 join `corpus.rs`'s `UNVERIFIABLE` list with their cases: s18's screw and s58 (U9), s13 extended with a second, opposed seat of unknown outline (U10, since AS1-tc is a fixture), and s50 CAP's skirt bore and spigot, s54's two pins and a new s64, a sleeve over a post (U11).
- SR6, B1's guard (B1 amended). A margin m with |m| ≤ τ + max(the target's own `part_uncertainty_mm` and `vertex_offset_mm`, `GUARD_MM`) is line to line (pinned), not clearance, so no original is a clearance whose margin VR4's guard would reject (RS6).
- SR7, interior span ends (B5 and I3 amended). A span end inside the target's wall, which is the occupant's end and so a neighbour position, is widened by its relation's τ, clipped to the target's own wall; VR7 then needs the candidate's wall that much further (RS7). An occupant within τ of the floor is a touch, so the floor is a seat and the span already reaches the wall's end: Y4 holds.
- SR8, a conical seat's radial range is the union of the ranges of every neighbour cone seated on the target cone (Q1 amended). Today `socket/combine.rs`'s `cones()` takes `radii_mm` from the first seat only, while `floor_mm` folds over all of them, so a countersunk head and a conical washer on one cone understate the range (RS12). Case: a new s60.
- SR9, older documents' reviews (MF11): `carry_over` and `foreign()` read a document of an older `SOCKET_RULES_VERSION` leniently, so the user's decisions carry where the requirement is unchanged and the user's file is never refused for overwrite for its age.
- Not asked: neighbour footprints for D6 functional's shrunk seat regions (M4-7 keeps them unshrunk), and the read-back of `self` pinning M3 left unchecked; both are recorded as open.

## 22. PR plan

Each PR comes with its tests and two adversarial reviews, as in M3. No PR depends on a later one. `SIGNATURE_VERSION` starts at 1 in PR 2 and `MATCH_RULES_VERSION` at 1 in PR 3; each rises with every later change to its output, so their values at the publish are whatever they have risen to (M3's rules went from 1 to 5). Every M4 rule id is in `RULES` (in `tools/gen-corpus.py`, the manifest's `rule_ids`) from PR 1, deferred to the PR that builds it until then.

1. **The socket PR** (M4-1), built in two parts: 1a (`SOCKET_RULES_VERSION` 6) states Q3, U10, B1's guard and SR8, the accessors, the lenient reader and the coverage test's widening; 1b (7) states SR1 (X8), SR7 and U9, U11 (m3-socket.md, section 15, item 9, records each as built). Three as-built differences bear on VR6 and VR22: U9 makes every piece past a known end unverifiable, pinned or bounded (the original witnesses nothing there, as VR18 says of any candidate); U11's core reaches D9's 0.01 mm past a span's ends whatever the policy, so under a coarser tolerance VR22 may find what U11 did not (a false negative on the original, never a pass); and U11 exempts no cross hole, so VR22, which exempts them, never finds the original's material where U11 did not. VR22 stops short of a root or bottom by u, as U11 does by g: a face's box is grown by its vertex offset. SR1, SR2, SR4 to SR9, and SR3 if taken; the new and amended M3 rules (X8, Q3, U9 to U11; B1, B5, I3 and Q1 amended) stated in m3-socket.md and ARCHITECTURE; `SOCKET_RULES_VERSION` 6; the corpus's and fixtures' socket pins and goldens re-blessed (the version rose); s18's, s58's, s13's, s50 CAP's and s54's expectations updated (U11 makes CAP's skirt bore and spigot and s54's pins unverifiable); new cases s60 and s64; M3's `Y4.witness` on the new predicate; MF11's lenient reader with a CLI test running `socket -o` onto a rules-5 document. The coverage test's `design_rules` widened to two-letter prefixes (`[A-Z]{1,2}[0-9]+[a-z]?`, at most 5 characters) and reading `docs/research/m4-match.md`; every M4 rule listed, each deferred to its PR, with the allowance for M4 deferrals that PR 7 removes.
2. **Candidates, signature, shortlist, evidence types.** `matching/` with `policy.rs`, `candidate.rs` (CF), `signature.rs` (SG), `shortlist.rs` and `index.rs` (SL), and `evidence.rs` (`Measure`, `Failure`, `FailureCode`, `Warning`); M0's `Check` gains its evidence and `Verdict` its `refused` constructor. Tests: CF on every unit and corpus part (unknown ends p25, p36, u01 to u06; unknown outlines p26, p27; cones without radii u05; a post, a wall opening); invariance on every `_rot`, `_pc` and `_tgt`; SG3's table per kind and policy; bins on edges; merging (a clevis, two holes 0.015 apart, a drifting chain); the mutations of section 20.3; level 1 ⊇ level 2; the original kept in every socket; hostile grids under a time bound.
   - As built: `matching/` holds `policy.rs`, `evidence.rs` (`Measure`, whose `passed` is exactly a positive margin and which fails with no limit or a value not finite; `Failure` and section 8.5's codes; `Warning`, reserved), `signature.rs`, `shortlist.rs`, `index.rs`, a `CandidateView` holding the signature and u_c (alignment's stacks and planes come in PR 3), and a `SocketView` (a socket's active and removed requirements, its contact tolerance, u_T, and SG7's test of a requirement every candidate fails, with span floors from the sources overlapping each span). `Check` is `Passed(Evidence)` or `Failed(Evidence)`, built by `Check::of`, and `decide` trusts a `Passed` only when its evidence shows the pass; `RequirementFailed` carries the failures' codes, and `Verdict::refused` builds a refused candidate's `Reject` (VD1, VD10). Review: SG5's merging made a merged element's domain narrower than verification (two bores 0.03 apart served by two stacks each 0.009 off; a 0.5 mm seat step under 0.1 mm), and offsets compared across normals tilted within W_A depended on the part's place in its frame (a seat face 1e-4 rad off, 10 m from the origin): each requirement now keeps its own domain, groups only allow sharing and set SL5's distinctness, and an offset is compared only across exactly parallel normals (`EXACT`, 1e-12); the five constructed cases are regression tests. A grip alone no longer leaves a socket "empty" for a catalog query. The seat domains now spend the budget and stop on it, each plane's nearby area computed once. Measures store their values first and derive the margin from what is stored, so EV7 recomputes them exactly, a limit that is not a number fails, and `band_mm` or `band_rad` is written as EV2 says. `span_floor` (`matching/mod.rs`) is the span floor VR4 must use in PR 3. Three departures from the text: level 1 asks for one stack in a line's narrowest window, not as many as lines share it, since two witnesses may take their diameters from one neighbouring stack (SL2) and the count could drop what level 2 keeps; level 1's pair window adds the drift of a midpoint along a line 0.1° off over the lever, since level 2 measures to a cluster's midpoint and level 1 to a stack's; and SL2's neighbourhoods are built pair by pair under the budget (SL10's sort is not needed while the budget bounds the work: a candidate of 10,000 holes is kept, untested, in milliseconds). CF1, CF2 and CF8 are the matcher's and verification's (PR 3). Measured: every original of the 76 corpus targets, in each of its frames and writers, and of the 36 pinned fixture targets, is kept by its own sockets under strict and functional clearance at 0.01 and 0.1 mm; over the corpus's parts and base components, level 1 returned whatever level 2 kept against every base socket; mutations within the tolerance were kept and a missing, too small or 0.5 mm displaced hole dropped (s01's bracket).
3. **Alignment and verification, with the committed candidates.** `align.rs`, `verify.rs`, `region.rs` (AL, VR, VR22 with its face tree), `Matcher::new`, `judge_step`, `judge_part`, `verify_at`. The generator's parametric builders for s01, s02 and s03, its `--candidate-set` mode and the match-case expectation schema (KC2, KC5), so that the committed subset of 13 candidates (KC15) is written and diffed here, and the strict gate runs on it through the library (KH1's library path, KH2, KH3). Unit tests of each review shape on constructed features (a post, a dome, a bar with and without a cross hole, a lip, a three-face transition, a split face with a pocket, an opening past the rim, a pinned bore continued past it, a bar across that extension, c + 2τ in four frames); the Y4 probe of `merge/y4probe` turned into a test over every original. VR21 on every corpus and fixture socket, unseeded, seeded and both ways against `_tgt`, under both policies, with the shipped `MAX_PLACEMENTS` and `MAX_COVERAGE_STEPS`, s55 the one named exception; `Y4.original_run` leaves `DEFERRED` in `corpus.rs` when this test (KH10) lands, and the coverage test cites it. The anchor pair logged for `g_inside_tol` and an off-anchor shift (merge finding 1). `MATCH_RULES_VERSION` 1.
   - As built, in three parts, one PR. **3a: verification at a given placement.** `matching/candidate.rs` gains each stack's usable profile (VR8: walls with both ends known, zones between them and out to the rims taken only when every face crossed there stays outside a hole's cylinder or inside a shaft's, a run broken where one is not, an annular bottom bounding the hole at its post's free end), its ends, its exempt faces and its walls' openings; `verify.rs` checks each requirement at a `Placement` with its evidence (`verify`, with `Seed::Given` alone until 3b), `axial.rs` bores and shafts (VR4's no-room branch, the axis at both extreme stations, each span's wall, pin, clearance or zone profile, the openings with RS14's extension, CF8, VR22), and `region.rs` VR11; VR22 asks M3's U11 machinery (`socket/own.rs`, now shared) about the requirement's axis carried into the candidate's frame. `MATCH_RULES_VERSION` 1, `MAX_COVERAGE_STEPS`. Y4 at the identity holds on every corpus original in every frame and writer (1,524 socket runs under strict and functional at 0.01 and 0.1 mm) and on every pinned fixture target. Three departures: the candidate's faces are boxed from what the part states, not from its topology (a judged `Part` carries none): a plane face by its outer loops, a whole cylinder by its end circles, any other cylinder by its surface within the part's box, a cone by its circles, a torus by its tube, an `other` face not at all, so it meets every query; a piece of the profile counts in a span only where it overlaps it by more than 1e-9 mm (a counterbore's step, touching the bore below it by 1e-14 mm in a tilted frame, read as part of it and failed three originals), a fixed slack, never the candidate's own u, which would let a part buy its own room; and CF8 rejects every span that meets a wall opening, a cross hole included, since M2 states how many openings a wall has but not the faces at them (`g_cross_hole` is a false negative until it does). The review's shapes (`review-soundness/gen.py`, run locally at the inverse of their frame motion) each reject, with `core_intruded`, `wall_missing`, `wall_opening`, `contact_not_covered`/`unknown_outline`, `end_not_open`, a diameter code or `candidate_feature_unknown`, and their originals pass; they join the corpus in 3c. **3b: alignment and the matcher.** `align.rs` builds placements as AL1 to AL12 say, from the socket's lines (one per distinct line whatever its sense, so a tube's bore and outside are one line), anchored on stacks within the shortlist's domains, those whose diameters fit first; `matcher.rs` holds `Matcher::new` (MF2's gate: refused, stale, unreviewed under `require_reviewed`), `judge_step` (CF2), `judge_part`, `verify_at` (AL14), the search of AL10 under one coverage budget per socket, and `combine` (VD2) in `verdict.rs`. KH10 holds unseeded: every corpus original in its own frame, the `_tgt` and `_pc` copies against the base's socket and the base's original against the `_tgt` socket, under strict and functional at 0.01 and 0.1 mm, and every pinned fixture target; s55's plate is the named exception (it exhausts the coverage bound unseeded and passes at the identity). `Y4.original_run` leaves `DEFERRED`. Departures: parallel lines closer than 2t fix no turn and are not an anchor pair (with none left, the first line anchors alone, AL8); AL5's candidate pairs are compared pair by pair under a bound of 4·10⁶ comparisons per socket (past it, `alignment_exhausted`), not through an index of axis feet; AL9's two non-parallel seats are not built (a socket of seats alone aligns by its landmarks); AL7 (a) skips a seat no candidate plane faces, so a missing seat fails as a seat, not as no placement; a socket with nothing M4 can verify is not aligned, its requirements failing as they fail every candidate. D's 40 candidates (`D/out`) give their expected verdicts under both policies, with every good case passing (`csink_variant` now included, by SR1's profile); some attribute their failures otherwise than D's lists (a moved hole caught at its seats, a thick bracket's grip as `seat_missing`), as KH2's `any_of` and `may` allow. **3c: the committed candidates and the strict gate.** `tools/gen-corpus.py` gains parametric builders whose defaults rebuild their sources' leaves (checked by a boolean difference at generation), the match cases (`kind: "match"`, every case now carrying its kind, RM6) with unrounded placements and locators (KC2, KC5), and `--candidate-set`, which writes the 13 committed candidates of KC15 in MAPPED (199 KB) to `crates/fitment-core/tests/candidates`, diffed in the `unit-set` job. The corpus adds D's slice for s01, s02, s03 and s18, the review's shapes, its edge cases in four frames and the counterexamples of this PR's review. `tests/support/gate.rs` holds KH2 and KH3 (`must ⊆ F ⊆ must ∪ may`, `any_of`, a failed requirement's codes meeting the expected ones), run on the committed set in core and on every corpus match case under both policies. One-hole defects on s01's bracket name `any_of` its four bores: it is symmetric under a half turn about each axis, so alignment may place the defect at any of them. **The review** (two rounds, soundness and Rust) found four candidates the rules as first built passed, each now a corpus case: an opening credited at its wall though the end zone past it was not taken (a lip narrowing back below the nut's rim, `m_nut_lip`, `m_nut_ext_lip`: an end is open only through a zone VR8 takes, and a zone is credited only at the stack's own end segment); a zone piece's core at its narrow radius (a finger into the countersink inside the head's cone, `m_plate18_finger`: the core follows the original's profile, cut into 16 pieces each at its largest radius); the slack above, which a candidate stating a large uncertainty widened (`m_block02_membrane`); and grip seats found without coverage (a spot face under each head, `m_bracket_spot`: a grip measures the plane that covers its seat). It also found a NaN dropped by `f64::max` in the zone profile, now kept; coverage charged one per step whatever a step reads (a seat of many edges spent minutes within the bound: each step now costs the edges it reads, containment asks only the cover faces a grid of their boxes returns, a plane polygon in VR22 costs its edges, plane searches cost planes times points, and coverage already found is reused by the grip and at the chosen placement); and CF8's fallback reading a cylinder's stations in its own frame. A second round on 3b and 3c found the strict gate too lax where `any_of` let an unrelated failure stand for the defect (`m_housing_lip` rejected at a seat, its bore passing at a placement alignment preferred): every bad case now states its `defect`, checked at the construction's own placement through `verify_at` (KH8), 32 of the 41 match cases. A separate bar hung 20 µm below a nut's open end passes, as section 8.6 says material past a real opening is the body's: `m_nut_bridge` is a `collides` case. The search keeps the chosen placement's checks instead of charging them twice; the coverage cache holds one placement's; AL8's turns, AL9's landmarks and AL7 (a)'s slides count toward alignment's bound; and the gate refuses an unknown verdict, an alias cycle, a locator without codes and a result of several sockets.
4. **Verdicts, the document, the CLI.** `decide` and `combine` extended, `MatchDocument`, the digest and order, `fitment match` with its exit codes (m2-features.md's note on exit codes amended: `Match` exits 0), `--original`, `--explain`; goldens under `tests/golden/match/` on three OSes; EV7's recomputation on every document; the socket gate's refusals; permutation and dedup tests.
   - As built: `Matcher::new` takes the fit verifier (`FitVerifier`, `NoFitVerifier`, `FitRequest` with `T_k · A` per occurrence, in `fit.rs`), asked only where a socket's interface passed (VD4, M4-26). `CandidateResult` carries the source (`CandidateSource`: a file's SHA-256, size and header, a component, or a part given), product, the gate's status and refusals, the read's facts (EV6), the flags (VD12, `larger_than_original` from the clearance a functional pass took past strict) and an empty `warnings`; `SocketResult` its placement's choice, u and ε (EV4) and what is not checked there (removed requirements with their notes, `max_extra_mm`). `combine` follows VD4 on the fits. `matching/document.rs` holds `MatchDocument` (MF6's envelope, MF5's order, MF3's one entry per SHA-256 with every label, M4-19's short passed records in a rejected candidate, the digest of the compact JSON without itself or labels) and `match_candidates`. `fitment match` (`cmd/match_.rs`) gates the socket document (a stale one by its versions, before any strict read), refuses an OUT that is an input, judges `--original` from the socket's own assembly, reads candidates and directories one file at a time, renders the table from the document itself, and exits as VD14 says; a closed stdout keeps the verdict's code. Tests: `match_cli.rs` (codes, refusals, the original, directories, permutation, escaping, review then match) with a test-side checker recomputing every measure, check, socket and candidate verdict, the summary, the order and the digest from the JSON alone (EV7, KH12), itself tested on tampered documents; match goldens for the committed candidates against their sockets, compared on three OSes. After the review: a socket verified by kernels that disagree makes no `Match` (`combine` demotes it to `InterfaceMatch`); the verifier is asked only with every occurrence placed and never after a shortlist drop; the original is the document's `original`, outside the summary and the exit code; a refused candidate carries the review's flags; the stderr warning names requirements blocked by the floor (VD6) as well as unverifiable ones (`Matcher::failing_all`); a directory with no STEP file and nothing else is a usage error, and a STEP name that leads nowhere an error; `--tolerance-deg` of 90 or more is a usage error; the checker (`tests/common/checker.rs`) also recomputes each socket's checks against the envelope's counts, `failed`, stage, fit and verdict, the flags the envelope implies, MF5's whole key and one entry per SHA-256, and runs on the match goldens. Not built: `--explain` prints every check but no extra text; the table's header shows the policy, not the per-socket counts of MF9's example.
5. **The corpus and its harness.** `build_candidates` in `gen-corpus.py` for every family (about 175 files, s59, s61 to s63 and the review's shapes), the manifest's match cases with `kind` on every case, and `corpus.rs` and `golden.rs` changed to select cases by kind (RM6); KH1's binary path, KH4 to KH9 and KH11 to KH14 in `corpus.rs` and `fixtures.rs` (KH10 came in PR 3); the `UNVERIFIABLE` and `CAN_FAIL` lists filled as their cases land; the false-negative baseline and its blessing; the report and its rates.
6. **The oracle and hardening.** `verify-occt.py match` (KO1 to KO5, with KO1's requirement regions), KO6's mutations, KO8's fixture tallies and KC12's pins; hostile candidates (10,000 identical holes, a 1,600-bore socket against a 1,681-hole grid, tiled planes, a face tree of 10⁵ faces near one bore) each within AL11's and SL4's bounds and a time bound; the three-OS report comparison.
7. **M4 exit and the release.** The registry gate on for M4's ids, the M4 allowance for deferrals removed (only reasons outside M4 remain: M7's fit and `collides`' `Reject`, if any rule needs one); PLAN, README, ARCHITECTURE and CHANGELOG (section 24) with the exit numbers (false positives 0; the false-negative rate against physics); then the release below.

### 22.1 The release at M4's exit

Checked on 2026-10-13 against this branch (C).

| Item | State | Needed |
| --- | --- | --- |
| Crate names `fitment`, `fitment-core` (and `-catalog`, `-server`, `-fit`, `-corpus`) | Free: crates.io answers 404 for all six | Publish before anyone takes them |
| `publish = false` in `[workspace.package]` | Blocks `cargo publish`, and `cargo package --workspace`, which then cannot find `fitment-core` for `fitment` | Flipped in PR 7; with it flipped (tested on a scratch copy) `cargo package --workspace` packages and verifies both through Cargo's local registry overlay |
| `fitment-core`'s package | 48 files, 679 KB (179 KB compressed): `src/**`, `README.md`, `Cargo.lock`; no STEP file | Add `LICENSE` (it ships none), listed in `include` |
| `fitment`'s package | 56 files, 892 KB (72 KB compressed), with `tests/*.rs` and 39 golden files that need the repository | `include = ["src/**", "README.md", "LICENSE"]`; `check-core.sh`'s STEP check extended to it |
| Building from the packages | `fitment-core` builds from its `.crate`; `fitment` against it through the overlay | `cargo package --workspace` as a CI step of PR 7 |
| Docs | `cargo doc -p fitment-core` clean with `-D warnings`; the core README says "Status: milestone M2 done" | The core README with the M4 API and compiling examples; the workspace README with `fitment match` and its exit codes |
| CHANGELOG | Stops at M0 | M1 to M4 under `[0.1.0]` |
| MSRV 1.85, stepq 0.6 | Checked by CI | Unchanged |
| Versions | `fitment-core = "=0.1.0"` in the workspace | Both at 0.1.0; `fitment-core` first, then `fitment` |
| Formats | `features.v1`, `fitment.parts` v1, `fitment.socket` v1, `fitment.match` v1 | Frozen (M4-28) |
| Public API | Every type `#[non_exhaustive]`, fields read-only | `cargo semver-checks` on later releases; until 1.0 a breaking change is a minor bump, stated in CHANGELOG |

## 23. Tests

By layer; the PR plan says where each lands.

- **Core units** (in-crate, no `unwrap`): each VR rule at its edge on constructed values (a pinned diameter at d ± ε rejects, at d ± ε/2 passes; a clearance margin at τ + u rejects, and at c + 2τ exactly in four frames; an axis at ε at the far station rejects; a plane tilted to ε at its farthest arc point rejects); `covers` on constructed regions (the region itself; a hole 0.6ε inside; a sliver uncovered between two faces sharing an edge; a face split along an internal edge; whole circles and arcs across the angle seam; an annulus; a whole circle's winding from inside, on and outside); profiles with zones, a torus dipping past the cylinder, a lip (annulus out, cone in), a three-face transition, a post from an annular bottom, an unknown end, a `None` end; VR22 on a post, a dome, a spike, a bar, a bar with a cross hole along it, a cross hole (exempt), a coplanar face of unknown outline, and a hostile tree of 10⁵ faces; VR7's rim on an opening past the rim; VR11 with an edge between a known and an unknown face; symmetric and non-symmetric single-axis sockets; AL3's ranking; AL4's fallback; AL11's bounds giving `alignment_exhausted`; the placement order; `decide` and `combine` (worst wins, empty is `no_sockets`, fits combined per VD4); removed requirements absent from `decide` and present in the evidence; SG and SL as PR 2 lists.
- **Rules with cases** (KH14): AL9's landmarks (s53, s55), AL6 (AS1), AL7 (c) (a pin with no seat), VR6 (s18, s13 extended, s64), VR13's sense (s18, `b_cone_sense`), VR12 under warning (`bracket_thick`), VR2 and VD6 (s61, and the fixtures `as1_pe`, BEARING_208), VR5's split (s62), VR4's no-room branch (s63), VD5 (s11, s12, s13, s17, s47, s48, s52, s58), CF2 (r03 to r12), VD9 and MF2 (s01's socket stale, edited and unreviewed; a refused `r*` document), and every `CAN_FAIL` rule by a bad case: CF4 and CF5 (`b_unknown_end`'s dome, whose wall ends at its last known station), CF8 (`b_wall_opening`, `nut_bar`), VR3 (shifts), VR4 (`e_tau`, Ø small), VR7 (`b_open_past_rim`: `nut_ext`, `nut_long`; blind), VR8 (`b_post`, `b_end_zone`), VR9 (`b_shaft`), VR10 and VR11 (`b_seat_region`, `b_seat_unknown_face`), VR12 (thickness), VR13 (cones), VR22 (`b_unknown_end`'s spike, which only VR22 rejects; `b_post`, `b_wall_opening`).
- **The original** (VR21, KH10): every corpus and fixture socket, unseeded and seeded, both ways against `_tgt`, both policies, with the shipped budgets; s55 by name.
- **The corpus** (`corpus.rs`): `every_match_case_meets_its_construction` (KH1 to KH3), `the_false_negatives_are_the_baseline` (KH4, KH5), `the_report_states_the_rates` (KH6, KH7), `false_negatives_say_where_they_come_from` (KH8), `the_shortlist_never_drops_what_verification_passes` (KH9), `every_original_fails_exactly_what_no_candidate_can_pass` (KH10), `matching_is_deterministic` (KH11), `every_match_document_reads_back` (KH12, with EV7), `the_candidate_set_fits_its_budget_and_its_manifest` (KH13), and the widened `every_rule_has_a_case_a_test_or_a_reason` (KH14).
- **Fixtures** (`fixtures.rs`): `as1_parts_match_across_exports` (KC12), `every_pinned_fixture_target_fails_exactly_what_no_candidate_can_pass` (KC11).
- **CLI** (`cli.rs`): exit codes 0 (with a fake verifier, in core: the CLI cannot reach 0 in M4), 4, 5, `--require-fit` turning 4 into 5 (the lug), 1 for an unreadable candidate beside a passing one, 2 for `--original` on another file (after the socket gate), `socket -o` onto a rules-5 document carrying its decisions (MF11), a directory argument, `-o` onto a candidate refused, a broken pipe, labels with control characters escaped; the review-then-match path on the committed s01.
- **Determinism:** match goldens for the unit assemblies against the committed candidates, on three OSes, blessed only when a version they record rises; permutation of arguments (byte-identical but for labels); the same file under two names (one entry, two labels).
- **The generator:** each parametric builder at its defaults equals the assembly's leaf; every locator names a requirement of its source's expected socket; edge cases come in four frames.
- **The oracle:** `match` mode, its `--expect` tallies, the mutation run (KO6).

## 24. Changes to PLAN.md and ARCHITECTURE.md

At PR 7, as built:

- **Matching method, steps 3 to 7:** the signature as rigid invariants with policy-free bins of 0.1 mm (not "quantized at the tolerance"); the shortlist as a necessary condition with a proof; alignment by anchor pairs, ranked not filtered, one placement per socket; verification with the guard band, the split floor, certified coverage and the clear core; `InterfaceMatch` exits 4 and `Match` 0 (m2-features.md's note amended in PR 4).
- **Tolerances:** the floor of a relation bounds limits derived from neighbours; comparisons with the original's own geometry use the parts' own uncertainty (M4-2).
- **D6, D7, D9, D8:** recorded as decided with section 2's defaults (M4-7, M4-8, M4-37, M4-38).
- **Milestones:** M4 done, with the exit numbers.
- **ARCHITECTURE.md:** a section "Matching a candidate" beside "Reading an assembly" (the flow of section 3, with its modules); `MATCH_RULES_VERSION` and `SIGNATURE_VERSION` in "Formats and versions"; `fitment.match` and its frozen formats; the test table gains the match corpus, the harness and the match oracle; "Soundness" gains Y4's longer list, the clear core (VR22) and the original's run.

## 25. Attacks and their fixes

Each designer attacked their own design: how could a bad candidate pass, or the original fail its own socket? Their findings are merged here, duplicates once, with where each came from (A, B, C or D, and the designer's own number where they gave one). Each changed a rule above.

### 25.1 The designers' findings

| # | Attack | What happened or would | Fix | From |
| --- | --- | --- | --- | --- |
| 1 | Coaxial candidate stacks clustered transitively | A chain of stacks each 0.015 mm off the last merges into one cluster; two required lines then find one cluster, and the matching drops a candidate verification passes | Non-transitive neighbourhoods (SL2), injectivity on stacks (SL5) | A 1 |
| 2 | A grip between seats of unknown outline | Located by any plane facing the right way, the grip passed over an empty region; leaving such planes out of the candidate instead failed AS1-tc's original on a grip the prototype passed | The grip is unverifiable (VR6, SR5's flag); planes of unknown outline stay in the signature (CF6) | A 2, A 12, B A13 |
| 3 | M2 9.6 read strictly: a stack with an unknown end satisfies nothing | 3 requirements of originals rest on unknown-end stacks (s18's screw, s58's plate, vaccase's tee) | First CF5 let the known walls count up to the unknown end; the soundness review showed a dome or spike rising inside them, so CF5 is now M2 9.6 as written, nothing past the last known station, with VR22's core test (RS1) | A 3 |
| 4 | Span windows taken from `fit` | s18's screw has a `not_read` span with neither pin nor clear diameter; reading `not_read` as pinned demands Ø6 past the screw's known end | SG3 uses `pinned` and `clear_diameter_mm` only | A 4 |
| 5 | A span with no diameter: a zone whose occupant was not read, or past a known end | Requiring only the segment's diameter passes a plain hole under a countersunk head; requiring a wall past the original's known end fails the original | M4-6: zone pieces checked against the original's profile once M3 states it (M4-1), unverifiable until then; pieces past a known end unverifiable | A 5, B A12 |
| 6 | Two required seats within 2σ | One candidate plane could cover both; requiring distinct planes drops it | SG5 merges seats as it merges lines | A 6 |
| 7 | The area filter per plane | Two candidate planes 0.005 mm apart may together cover a contact | SL2 adds the area of planes within W_L and W_A | A 7 |
| 8 | D7 warning with hard seat pairs | The shortlist enforced through seat pairs the separation the user made a warning | No seat separation under warning (SG3): s01's bracket keeps 21, not 11. Moot in M4, which offers D7 `hard` only (M4-8); it returns with M7 | A 8 |
| 9 | A bin edge | A value on an edge, or a window applied after binning, could miss | Widen first, look up bin(lo) − 1 to bin(hi) + 1, compare exactly (SG10) | A 9 |
| 10 | The budget counted per revision | One revision of a 10⁴ × 10⁴ pair ran to 48 million comparisons | Count per comparison; partners through grids (SL4) | A 10 |
| 11 | An angular tolerance beyond the index's pair cap | Pairs tilted more than 0.1° are not stored | Such a query bypasses the pair filter (SL8) | A 11 |
| 12 | The mirror image | Invariants cannot tell it; and s15's "mirror" is congruent to the block by a half turn, so it tests nothing | Alignment's proper rotations do; s15 is `g_mirror`, a new asymmetric s59 is `b_mirror` | A 13, D |
| 13 | Invariants rounded differently in two frames | Up to 7.3e-10 mm measured | 1e-9 × (1 + \|v\|) slack on every window (SG6) | A 14 |
| 14 | Requirements failing every candidate still filtering | Harmless for the verdict, but the shortlist's evidence would blame the wrong requirement | SG7: they contribute nothing and are listed in `blocked_by` | A 15 |
| 15 | Rounding puts an edge case inside | An exact 0.01 shift reads 7.4e-11 inside in 21 of 40 frames, clear + 0.01 inside in 40 of 40; a decimal edge (6.6 + 0.01) is decided by binary rounding | VR2's guard band, VR4's τ for clear diameters; edge cases in four frames (KC7) | B A1, C, D |
| 16 | Each requirement at its own best placement | Four holes each within tolerance of some placement, never of one | One placement per socket (AL1) | B A2 |
| 17 | A larger hole under D6 functional | It passes the bore, and the seat region is not re-checked | The region is not shrunk (M4-7): the seat rejects; functional gains little | B A3, C |
| 18 | A U-channel whose bolt hits the far leg past an open hole | An opening says only that air lies beyond the rim | Not caught by M4: the bolt is a neighbour, so M7's body check sees it | B A4 |
| 19 | A conical boss where the socket wants a countersink | Same apex, axis and angle, the other sense | VR13 requires the sense (SR2, M4-1), inferred from the socket until then | B A5 |
| 20 | A candidate face split along an edge inside the region | The shared edge looks like a boundary deep inside: a false negative | Edges shared by two faces of the same plane are skipped in VR11's step 1 | B A6 |
| 21 | A small hole in the candidate's face, between samples | A sampled test misses it | Every candidate edge is marched with the Lipschitz step (VR11) | B A7 |
| 22 | A candidate ring around the region without covering it | No candidate edge lies inside | VR11's step 2: the offset curves must be covered | B A8 |
| 23 | The anchors share a shift | `shift_tol`: each bore passes at 0.005 | Every check uses the same placement; the seats' circles move with the holes and catch it; KH2's `any_of` accepts a seat (B's F6) | B A9, D |
| 24 | A thread engaged only partly (a chamfer inside a pinned span) | A zone counted as cylinder would pass | Pinned spans need one cylinder piece (VR7) | B A10 |
| 25 | A torus transition bulging into the hole | Counting it at the narrower radius overstates clearance | VR8 takes a zone only when every torus stays outside the cylinder | B A11 |
| 26 | The original fails at a whole circle | Endpoints rounding apart wound zero from inside: s03's dowels and s05's washers failed their own seats | A whole circle winds once from inside (VR11) | B A14 |
| 27 | The original fails through its arcs' points | The plane deviation measured at c ± r along the normal | Over an arc, the centre's deviation plus r times the in-plane slope (VR3) | B A15 |
| 28 | The floor | A floor of 0.672 mm checked to 0.01 passes what the assembly cannot vouch for; widening the tolerance to the floor passes a Ø10.5 for a pinned Ø10; a single floor at exactly 0.01 fails AS1-md and its original | The split floor (VR2, VR4, VR5, VD6) | B A16, C, D |
| 29 | A floor that differs between a span's relations | The requirement's largest floor could fail the original where the span's own relations were tighter | VR4 takes the floor of the sources whose span meets it | B A17 |
| 30 | Hostile candidates: thousands of holes, a tiled plane | Quadratic pair enumeration; 20,000 placements of coverage | AL11's bounds and index, SL4's budget: past them a reject (alignment) or a keep (shortlist), never a pass | B A18, A |
| 31 | `self` couplings under different placements per socket | Two copies placed differently could clash | Both sides are pinned to the original (N2); copies' clashes are M7's (V1); the oracle places both copies at once | B A19, D |
| 32 | A tilted candidate hole whose ends read undecided | It is not a stack | Never matched (`fn_undecided`) | B A20 |
| 33 | A removed seat whose grip remains | The grip could pass without a plane | The grip re-finds both planes (VR12) | B A21 |
| 34 | A shaft longer than `max_extra_mm` | Passes the interface, bottoms out | Not checked and said so (M4-11); `collides`, M7's | B A22, D |
| 35 | A document with no socket | The worst over nothing defaulted to the best kind | `combine` of zero sockets rejects (`no_sockets`) | C |
| 36 | A candidate that fits one occurrence and not another | Judging one socket, or merging checks by requirement id across sockets | A verdict per socket, worst wins; ids qualified by socket | C |
| 37 | `InterfaceMatch` read as success | With 0 for both, `fitment match … && use-it` takes an unchecked body as fitting | Exit 4 (M4-18) | C |
| 38 | An unreadable candidate beside a passing one | Exit 0 or 4 hides the missing judgment | Exit 1 dominates (VD14) | C |
| 39 | A stale socket edited to read as current | `is_current` compares versions only; a recomputed digest passes | `from_json` re-checks structure; the match document records the socket's digest and source; the catalog (M6) re-extracts | C |
| 40 | Removing requirements in review until a candidate passes | The verdict looks complete | `requirements_removed`, `not_checked` records with notes, "5 of 7 checked" | C |
| 41 | A label that prints as another candidate, or carries terminal escapes | Two lines that look alike, or a forged line | `printable()`, and the SHA-256 prefix on every table line | C |
| 42 | Argument order changing the result | Shared state between candidates | Each candidate judged alone; order by content; a permutation test | C |
| 43 | The same file under two names | Double counting | Dedup by SHA-256 | C |
| 44 | A reject's evidence suggesting "almost fits" | The fewest-failures placement read as the candidate's | Stated as the best of `tried` (EV4) | C |
| 45 | An edge decided by platform maths | A value on the edge passes on one OS | Only `+ − × ÷ sqrt` and `libm`; margins recorded; `passed` is `margin > 0` | C |
| 46 | Evidence that does not support its verdict | A measure not applied, a verdict from the wrong records | EV7's recomputation on every document | C |
| 47 | A stored report edited to say `match` | A consumer deserializing reports | No reader; `Verdict` and `FitStatus` are not `Deserialize` | C |
| 48 | `--require-fit` with no verifier | Exit 5 read as "nothing fits" | A stderr line says why (MF10) | C |
| 49 | An unverifiable requirement kept in review | Every candidate rejected with no hint | One warning naming it (VD8, MF10) | C |
| 50 | The original fails its own socket on a real file | A rule wrong at an equal boundary: four seen in C's prototype, five in B's | VD15's self-check and VR21 on every socket | C, B |
| 51 | An `internal` refusal among ordinary rejects | A fitment bug hidden | Exit 1 (VD10) | C |
| 52 | Clearance "just past tolerance" called bad | The bracket's hole moved 0.015 passes its bolt; a looser rule would "fail" the gate | Truth is physical (KC3); just-past-tolerance bad cases sit on pinned and contact requirements; clearance shifts are `fn_rule` | D |
| 53 | Mislabelled cases | 3 of 35 labels wrong in the slice | KO1 checks every label in CI; a disagreement fails the generator | D |
| 54 | "Rejected for some other reason" too strict to state | Which seat alignment slides onto decides which seat fails | `any_of` and `may` (KH2) | D |
| 55 | `may` hiding a missed defect | A case rejected only by a side effect passes while the defect goes unseen | `must` never empty and names the defect's requirement; KO2 checks the defect touches only that requirement's neighbours | D |
| 56 | No placement, no attribution | "Rejected" says nothing about why | KH3: the unanchored requirements fail with `no_placement` | D |
| 57 | A socket that misses a requirement | A bad candidate passes every check the socket asks | KO3's physics on every pass is socket-free | D |
| 58 | The shortlist as a hidden reason | A good candidate dropped looks like a rule's false negative | KH9: a hard failure; SL1: verification runs after every explicit drop | D |
| 59 | Blessing a new false negative by accident | A refactor loses a good case and is blessed in passing | KH5: an old case's new entry needs `MATCH_RULES_VERSION` to rise | D |
| 60 | The original failing through representation | `_tgt` differs by about 1e-11 mm | The guard (≥ 1e-6 mm) is far above it and far below the tolerance | D |
| 61 | Review hiding a defect | A removed requirement is not checked | `review_waived` cases, reported and not counted as false positives; a second defect still rejects (KC13) | D |
| 62 | The colliding case broken by the interface | A lug from the bottom face cuts the seat region and rejects for a seat | The lug hangs off a side face, in no seat region | D |
| 63 | Physics blind to non-planar contacts | An 82° countersink read as "fits" | Contacts of every surface kind, within a band (KO7) | D |
| 64 | A stale corpus | `corpus/out` held s58's old files | CI regenerates (`FITMENT_REQUIRE_CORPUS=1`); the harness reads the manifest | D |

What none of this protects against: a requirement M3 never wrote (only the socket rules, M3's oracle and KO3 catch that), and a body collision (M7).

### 25.2 The merge's own findings

Reading the four designs against each other found six more. Each changed a rule or an expectation above.

| # | Finding | Fix |
| --- | --- | --- |
| 1 | `bracket_shift_half_tol` (one hole moved 0.005) passes, by VR11's rule, only if the placement's anchor pair holds the moved hole, sharing the shift 0.0025 a side; the prototype did not log which pair anchored ("2 of 8 placements" says nothing of it: the unmodified original also first passes at the second; RL8). A hole off the anchor pair moved by more than ε/2 = 0.005 rejects at its seats, its circle that deep inside the target face (VR11's step 1, which rejects a candidate edge more than ε/2 inside: a stated false negative). C's c12 (0.009 off) is the same case. Derived from the rules, not measured | `g_inside_tol` moves a seat-bearing hole by at most a quarter of the tolerance unless it anchors (KC8); a new `fn_coverage` case moves a hole off the anchor pair by tol/2 (KC10); AL13's refinement, off by default, is the candidate cure (open question 2) |
| 2 | SG6's area bound, the contact area minus its perimeter × σ, can exceed what verification leaves covered: the band of width w inside a region around a hole of radius r has area π(2rw + w²), more than its perimeter × w | The bound subtracts twice the perimeter × σ + πσ² per loop: verification leaves uncovered only the ε band (VR11, as corrected by RS11), and Steiner's bound for a closed curve gives at most 2εL + πε² ≤ 2σL + πσ² |
| 3 | The shortlist, run first, drops a candidate whose only defect is a diameter, so AL3's purpose (naming `diameter_out_of_limits`) and KH2's attribution are lost for exactly the cases the gate checks most | M4-17: explicit candidates are aligned and verified after a drop |
| 4 | C's example and table state margins of exactly the tolerance and c11's margin −8.9e-16, which VR2's guard changes | Recomputed: the original's margins are ε = t − u; c11's −1e-6 |
| 5 | D's KH12 read match documents back through a `from_json` that C's M4-23 forbids | A structural checker in the tests (KH12, MF7) |
| 6 | B's agreement under grip warning (22 of 22) was measured on a prototype without VR12's floating seat, so thickness cases rejected for a seat, as under hard | Moot: M4 offers D7 `hard` only (M4-8, section 28.4); the grip-warning expectations move to M7 |

## 26. How the four designs were reconciled

Where they disagreed, the settlement leans toward fewer passes (Soundness rule 4), never trading a false positive for a false negative; where one designer's claim met another's measurement, the measurement stands.

| Disagreement | Settled as | Why |
| --- | --- | --- |
| The floor. C (VD6, C2) and D (D-10) failed every requirement whose `floor_mm` reaches the tolerance; B (BD4, VR2, VR5) split it | B's split floor (M4-2). Comparisons with the original's own geometry use the two parts' own uncertainty; `floor_mm` enters only limits from neighbours (VR4's τ). `floor_above_tolerance` when t − u ≤ 0 or a neighbour limit leaves no room. SG7, VD6 and KC10 to KC12 restated; A's and C's counts restated where measured or derivable (sections 18, 20.1) | M3's Z2 builds the floor from placement uncertainty, which bears on where neighbours lie relative to the target, not on the target's own shape, which is known to its part uncertainty. A single floor failed AS1-md (placement 0.01 mm, parts 2.1e-6) and 67 checkable weldment requirements for every candidate, the original included. `as1_pe` still fails |
| The edge. C compared `margin > 0` with no guard; D asked for a guard of the requirement's floor and the candidate's uncertainty; B guarded by both parts' own uncertainty | B's VR2: `deviation < t − u`, u at least 1e-6 mm; KC7's four frames kept | Every one of D's edge cases rejects in every frame measured; a floor in the guard would reject AS1-md again |
| Unknown ends. C's VD11 and M2 9.6 read strictly; A's CF5 let a stack's known walls count up to an unknown end | First A's CF5; after the soundness review (RS1), M2 9.6 as written: a stack with an unknown end satisfies nothing past its last known station, and VR22 tests the core (M4-5); VD11 amended to match | The face the walk could not read may rise back inside the bore (`housing_dome`, `housing_spike`: COVER dV +74.25 and +7.07 mm³, both passed). Reverting costs Y4 nothing: the 3 original requirements on such stacks (s18's screw, s58's plate, vaccase's tee) also have a span past the known end and are unverifiable anyway |
| Spans with no clear diameter. A asked M3 to state each span's basis and make pieces past a known end unverifiable; B made zone spans unverifiable in M4 (BD10) until M3 states the profile (B's F1, now SR1) | Both, joined (M4-6, VR6, VR7): a basis for every piece, a profile for zone pieces; zone pieces verifiable with it, pieces past a known end unverifiable. The socket PR first (M4-1), a decision for the user | Without a profile, nothing states how open a candidate must be there; with it, s18's plate passes its own original. M3 and M4 then share one predicate |
| D7 warning. C's VD13 turned a failed grip check into a warning; B's VR12 floats the far seat; A's A-D9 drops seat separations from the signature | First VR12 with A-D9 and VD13 referring to them; after three rounds of soundness review (section 28), D7 `hard` only in M4, `warning` deferred to M7 (M4-8); A-D9 is moot in M4 | Two hard seats fix the separation within 2ε, so a warning on the grip check alone changes nothing (B: `bracket_thick` fails a seat under both); the floating seat that gave it meaning could not be made sound from the socket's relations (RS15, RS20, RS21) |
| Attribution. D's KH2 named one requirement per defect; B found the seats, not the bore, catch a shifted hole, and seats fail beside diameter defects (B's F6) | D's KH2 widened: `must` may hold `any_of` groups; a bad case's `must` is never empty and is met; F ⊆ must ∪ may still holds. The affected cases rewritten (section 15.5) | Which requirement catches a defect is verification's to decide; the gate still forbids "some other reason" |
| `max_extra_mm`. C proposed `shaft_too_long`; D called a long boss `collides`; B left it unchecked (BD11) | Not a requirement, not checked, listed as not checked; `collides`, `InterfaceMatch` correct until M7 (M4-11) | M3's I4 records it for information, without the end it applies to |
| Names. A: `PartSignature`, `SocketSignature`, `shortlist → Result<Kept, _>`; C: `shortlist → Result<(), Vec<Missing>>`, `align(&Socket, &Part, …)`, `verify(&Requirement, &Part, …)`; B: `placements`, `match_socket`, `SocketView`, `CandidateView`; D: `match_part`, `verify_at(socket, part, placement)` | C's public names (`Matcher`, `judge_step`, `judge_part`, `MatchPolicy`, `decide`, `combine`, `MatchDocument`), with `shortlist`'s Ok side carrying A's pruned domains (`Kept`), which `align` starts from; B's `SocketView` and `CandidateView` as the per-socket and per-candidate structures, `CandidateView` built once from the `Part` and holding the `PartSignature`; `align` and `verify` take the views (B's signatures under C's names); D's entries are `judge_part` and `Matcher::verify_at` (section 12) | Each type defined once |
| The original's run. B's VR21 (own frame, original vs `_tgt` socket, `_tgt` copy vs base socket; s55 named); C's VD15 (`--original`); D's KC11 and KH10 (base, `_tgt` and `_pc` copies; no exception) | One rule (VR21) on D's set plus B's run against the `_tgt` socket, seeded and unseeded; s55 the one named exception, as M3's oracle names it; VD15 its CLI form, KH10 its corpus test | s55's 3,600 tiles exhaust the placement budget unseeded and pass seeded at the identity |
| A grip whose seats are unverifiable. A kept it verifiable and kept planes of unknown outline for it (A 2, A 12); B made it unverifiable (VR6) | B's: unverifiable (with M4-1, by `verifiable()`); CF6 still keeps the planes, for the stored signature | Fewer passes: a grip between two seats nobody can locate is not verified by any plane facing the right way |
| A shortlist drop. A and C stopped there with one reason (A-D10, EV5); B's AL3 and D's KH2 need the defect's requirement named | Explicit candidates are verified after a drop (M4-17, SL1, EV5); a catalog query stops | Attribution for every reject, and KH9 on every run |
| Reading `fitment.match`. D's KH12 assumed C's `from_json`; C's C10 has no reader | No reader in core; a test-side structural checker (KH12, MF7) | A stored report never becomes a verdict |
| The grip warning's name. D: `grip_not_verified`; C: `grip_differs` | `grip_differs`, reserved for M7 | C owns the verdict's flags |
| Failure codes. D added `floor_exceeds_tolerance`, `angle_out_of_limits` and used `contact_not_covered` for a cone's range; C proposed `shaft_too_long`; B completed C's list | One list (section 8.5): `floor_above_tolerance`, `cone_mismatch:angle`, `cone_mismatch:range`; `shaft_too_long` dropped; `:profile` and `:sense` added for M4-1 | One closed list, machine names fixed |
| A measure's `tolerance`. C's measure had one; B's limits use two bands (ε for equality, τ for clearance) | `band` and `band_mm` per measure; a clearance span records `clear_margin_mm` (VR4) and `diameter_mm` (the strict maximum) apart; the socket's `u_mm` and `epsilon_mm` in its placement record | Each limit says what it was computed from |
| `MAX_PLACEMENTS`. B's prototype 20,000; B's own default 4,096 | 4,096 (M4-13) | Every socket but s55 needs at most 64 measured; s55 is named either way |
| D6 functional. C wanted the seat region shrunk before functional ships; D expected `bracket_dia_large` to pass under functional; B kept regions (BD6) | B's (M4-7); `bracket_dia_large` under functional is a baseline entry | Shrinking needs the neighbours' footprints, which M3's R14 dropped |
| A clearance hole off its axis but within its clearance. D asked for containment so `fn_position` passes; B kept the axis within ε (BD8) | B's (M4-9) | Fewer passes; the target-face seat rejects it anyway |
| C's prototype verdicts (c12, c11 under functional) against B's coverage rule | B's rules decide; the prototype's table is kept for the document's shape, its differences stated (section 19.1) | The prototype aligned at the identity with crude coverage |
| D's KC12: md's `l_bracket` against ac's socket "expected, not yet measured" | Measured by B: passes (9 of 9) | The measurement stands |
| B's fixture text (vaccase's tee "fails its 9 unresolved and one zone span") against B's own output (`B/out/fix.txt`: also 23 by the floor) | The output: 9 unresolved, one span, 23 by the floor | The measurement stands |
| Versions. A added `SIGNATURE_VERSION`; C's `MATCH_RULES_VERSION` rose with A's output too | Both: `SIGNATURE_VERSION` for the stored signature (M5 recomputes), `MATCH_RULES_VERSION` for any judged output, the shortlist's included (M4-27) | Different consumers |
| PR order. D put the corpus with C's evidence PR and the committed subset with B's first verification PR | Evidence types in PR 2, the subset with verification in PR 3, the document and CLI in PR 4, the corpus in PR 5 | The gate on the subset needs failure codes before the document exists |

The decisions table merges the designers' own:

| Row | From |
| --- | --- |
| M4-1 | B's BD10 and F1 to F4 (now SR1 to SR4); A's finding 5 and open questions 1 and 2; the reviews' RS6, RS7, RS12 and RH1 (SR6 to SR9) |
| M4-2 | B's BD4 against C's C2 and D's D-10 |
| M4-3, M4-4 | B's BD2 and BD3; D's D-7 in part |
| M4-5 | A's A-D2 |
| M4-6 | B's BD10; A's finding 5 |
| M4-7 to M4-11 | B's BD5 and BD6, BD7 with A's A-D9 and C's VD13, BD8, BD9, BD11 |
| M4-12, M4-13 | B's BD1, BD13, BD12; A's A-D7 |
| M4-14 | A's A-D1; D's D-9 |
| M4-15 | A's A-D4, A-D5, A-D6, A-D8 |
| M4-16 | A's A-D3 |
| M4-17 | New in the merge (finding 3), replacing A's A-D10 for explicit candidates |
| M4-18 to M4-28 | C's C1, C3, C4 to C6, C7 with C9 and C12, C8, C10, C11, C13, C14, C15 with A's CF10, C16 |
| M4-29 to M4-36 | D's D-1, D-2, D-3 with B's F6, D-4 with D-5, D-6, D-7, D-8, D-11 |
| M4-37, M4-38 | PLAN's D9 and D8, added after the consistency review (RL4) |

No rule was renumbered: every designer's ids are kept, each a rule once. Rules that overlapped were left in their areas and made to refer to each other rather than merged: VR14 (verification) and VD5 (the verdict) state unverifiable requirements from both sides; VR21, VD15 and KH10 state the original's run as a rule, a CLI check and a corpus test.

## 27. Risks and open questions

### 27.1 Risks

- **The shortlist's proof rests on verification's windows.** If verification ever accepts anything wider than (w1) to (w6), the shortlist drops good candidates. KH9 and SL1's check on every explicit run are the guards; a widened window must be exported to the signature.
- **Real sockets rarely pass.** 12 of 36 fixture target documents can yield a pass as extracted (section 20.1). The false-negative rate on real assemblies will be dominated by unresolved requirements and floors, not by M4's rules; the CLI's warnings and `--original` show users why.
- **R14 dominates the false negatives that remain on generated parts:** any change to a seat face's outline (a pocket, a chamfer, a hole grown, a smaller footprint) rejects.
- **The coverage band is a sharp edge.** A seat-bearing hole moved by more than ε/2 off the anchor pair rejects (merge finding 1, derived): within tolerance, physically fine, and dependent on which pair anchors.
- **The core test is conservative where it uses boxes.** An `other` face near a bore, or a cross hole at an angle whose faces are not of whole sweep, fails VR22 though it may be clear: false negatives, measured first in PR 3 on the corpus.
- **U11 as extended costs false negatives.** Wherever the original carries its own material within a shaft's span stations at any radius (a skirt round a spigot, two pins on one block), or inside a bore's core, that requirement is unverifiable: in the corpus, s50 CAP's spigot and skirt bore and s54's two pins. A reviewer may remove such a requirement after looking, which then leaves it unchecked. A free radius stated per station (the round-3 reviewer's alternative to the withdrawn R_f) would recover them; it is later work, after the freeze only through a new socket field (open question 17).
- **The rules grew after the prototype.** VR22, VR7's rim, extension and wall openings, VR8's post and zone conditions and VR4's guard were designed after the soundness review and are not prototyped on candidates. VR22 and VR4's guard are measured on every original (209 of 209 spans clear, 4 requirements unverifiable by U11); the false-negative rate and the full Y4 run are first measured in PR 3.
- **Coverage cost** grows as 1/ε and with perimeter; s55 shows a seats-only socket on a self-similar candidate can exhaust alignment. An exact polygon-with-arcs arrangement (sound, n log n) is the later replacement for marching.
- **Placement completeness.** Constructed placements can miss a passing one (AL13); VR21 and KH8 are the guards.
- **Seat-only sockets are barely filtered** (a median of 190 of 291 kept), and single-line sockets are unselective in a catalog (34,718 of 10⁵ copies for s02's block). Their cost is alignment time, not soundness.
- **The lever term is loose on large parts:** 3.3 mm windows on a 4.7 m part at 0.01°.
- **A third version.** `SIGNATURE_VERSION` must rise with any change to CF, SG or the tokens, or M5's stored signatures go stale.
- **The socket changes late** (M4-1): `SOCKET_RULES_VERSION` 6 re-extracts every socket a user has reviewed; carry-over keeps decisions where requirements are unchanged. There are no published users yet.
- **Exit code 4 for the normal M4 result** surprises scripts expecting 0; documented in `--help`, the README and the CHANGELOG.
- **Output size:** 8 MB for 100 candidates on the weldment's bearing. M6 will want pagination and the compact form.
- **No thread or material evidence exists in the data at hand;** the flags will ride on every verdict for a long time.
- **The corpus measures fitment against its own generator family.** Every generated candidate is Open CASCADE 7.9's; AS1's exports and the fixtures' originals are the only foreign writers. D8's real ground truth would be the first independent false-negative sample.
- **The physics oracle is a fit verifier in all but name.** M7 can start from it; until then it stays an oracle, never a source of `Match` (D1).
- **Tolerances interact with labels:** a case built at 0.01 mm changes class under another D9 value, and changing the default means regenerating the edge cases.
- **Oracle cost on fixtures:** M3's socket oracle takes 11 minutes in CI; AS1's cross-export physics adds 180 boolean placements of small parts (about 0.05 s each).
- **The release depends on the exit test,** and publishing freezes the formats; a format found wanting later costs a new `format_version` and, for sockets, a v1 reader kept alongside.

### 27.2 Open questions, with defaults

1. Pair-local lever instead of the global box diagonal: tighter windows, a longer proof. Default: global in v1.
2. Least-squares refinement (AL13): default off; merge finding 1's off-anchor shift is the first corpus case that would use it, so PR 3 measures it.
3. D6 functional with shrunk seat regions: default no; it needs neighbour footprints in the socket, a format change after the freeze.
4. A clearance hole off its axis within its clearance (`fn_position`): default reject (M4-9).
5. Seats-only sockets like s55: default reject past the bound; a smarter landmark choice later.
6. Wall openings crossing a candidate's required span (CF8): default not compared, stated in the evidence as not checked.
7. A chirality invariant to drop mirror images before alignment: default no.
8. Assemblies as candidates in the CLI (`--candidate-part`), or `fitment match` taking an assembly and `--part` and extracting the socket on the fly: default no in M4; the API takes leaves, and the reviewed socket file is the point of M3.
9. Should `fitment match` ever skip the shortlist? Default no; a hidden `--no-shortlist` exists for KH9.
10. `--policy FILE` for a named `TolerancePolicy`: default flags only; a policy file when the catalog stores policies (M5).
11. Parallel judging in the CLI: default sequential (0.2 s of features per 100 candidates); the order rule already makes parallel output identical.
12. `--no-labels` for content-addressed pipelines: default labels kept; a switch if M6 asks.
13. Should `edge` cases count in the false-negative rate against physics? Default no; their own row.
14. A fourth policy for the sweep in CI: default report-only, on Linux.
15. Cross-export expectations pinned per pair in `fixtures/manifest.json`: default yes, re-pinned only in a PR that says why.
16. SR3 in the socket PR: default no, since M4 does not check `max_extra_mm`; it is cheap to add then and impossible after the freeze without a v2.
17. A per-station free radius for shafts (RS19's alternative), letting a shaft beside the original's own skirt or pin be checked instead of unverifiable: default no in M4; a later socket field.

## 28. Review of the design

Two adversarial reviews read this document on 2026-10-15 at `3078db8`, the soundness review in three rounds (28.1, 28.3, 28.4): one for soundness (false-positive paths; `m4-evidence/review-soundness.md`, probes in `review-soundness/`), one for consistency, completeness and feasibility (`m4-evidence/review-consistency.md`). Their findings are named here RS1 to RS12 and RH1, RM1 to RM7, RL1 to RL13, after the reviewers' own F, H, M and L numbers. All were taken. Rule ids stayed stable: two rules were added at the ends of their prefixes (VR22, MF11) and none renumbered.

### 28.1 Soundness

The reviewer built candidates from `tools/gen-corpus.py`'s primitives (`review-soundness/gen.py`), read them with `fitment features`, ran B's prototype of AL and VR on them (`proto.txt`) and measured each against the neighbour its passing requirement cites, with Open CASCADE at the construction's placement (`physics.txt`). Five independent paths gave `InterfaceMatch` to a candidate that does not fit, each through the rules' text, not only the prototype, and none caught by the planned corpus.

- **RS1 (critical): material back inside a hole from its own end.** M2's walk stops at the first face it classifies, so an annular bottom's post, or a face an unknown end could not read, may rise inside the bore while the wall is credited as air. Shown on s19's HOUSING (bore Ø12.1, span [3, 8], clear 12): `housing_post` (a Ø4 post, `bottom: annular`, `complete: true`) PASS, COVER dV +50.27 mm³; `housing_spike` (a cone, unknown end) +7.07; `housing_dome` (a spherical floor, unknown end) +74.25; on s02's BASE, `base_post` in a pinned span +14.14 over the original's. Fix: VR8 bounds the usable profile at a post's free end; CF5 is M2 9.6 as written (nothing past the last known station), which costs Y4 nothing (section 26); a new rule, VR22, requires every required span's core to be clear of every face but the stack's own walls and cross holes; SR5's U11 makes a target bore whose own post reaches into a span unverifiable, so Y4 stays exact; M4-5, CF3, CF4, VR18 and VD11 restated; `b_post` and `b_unknown_end` are corpus classes.
- **RS2 (critical): a wall opening admits material.** CF8 carried wall openings uncompared. `housing_bar` (a Ø2 bar across the bore: a full hole with two wall openings) PASS under strict and functional, COVER dV +37.57; `housing_cross` (an air Ø2 cross hole there) passes and fits (0 mm³). Fix: CF8 and VR7 fail a span meeting a wall opening unless every face sharing its edges is a hole-sense cylinder or cone of whole sweep (`wall_opening`), with VR22 behind it; `housing_cross` passes both (section 8.2), and is `g_cross_hole`; `b_wall_opening` is a class.
- **RS3 (critical): end zones taken without the zone condition.** VR8 took an end zone unconditionally. `housing_lip` (the bore's mouth runs out along an annulus and back in along a cone to r 5; still `opening`, `complete: true`) PASS, COVER dV +5.93. A walk of annulus, cone and annulus can also dip below both segments of a transition. Fix: VR8 takes any zone, transition or end, only when every face crossed there, cone, torus or annulus, face by face, stays outside the hole's cylinder at the zone's radius, as M3's X7 states; `b_end_zone` covers the lip and the three-face transition.
- **RS4 (critical): coverage skipped edges beside an unknown-outline face.** VR11's step 1 skipped every edge "shared by two faces of the same plane", so an edge between a face in U and a coplanar face of unknown outline (not in U) was never marched. `housing_split` (the top split in three, one ring hiding an elliptic pocket, outline unknown) PASS, contact lost 4.71 mm² where KO1 allows 1.98. Fix: an edge is skipped only when both faces sharing it are in U; `b_seat_unknown_face` is a class. Y4 is unaffected: R is the target face, whose outline is known.
- **RS5 (critical): a required opening was not required at the rim.** VR7 checked the end's kind only. `nut_ext` (s01's nut 5 mm taller, its Ø5 hole narrowing to Ø3 past the original's rim, both ends `opening`) PASS, BOLT dV +42.41, the bore's own occupant. Fix: the opening must lie within ε of the span's end, or the profile out to it must meet the end span's limits (VR7, the code table, 8.6's row); `b_open_past_rim` is a class.
- **RS6 (high): VR4's limit had no guard band.** At exactly c + 2τ a margin reads above or below τ by rounding and frame: s50's tube at Ø41.98 passed in IDENTITY and failed in VARIANT; over decimal clear diameters 1 to 20 mm, 716 of 1,901 read above τ at the limit. Fix: VR4 passes only when m > τ + u; M3's B1 takes the same guard (SR6) so Y4 holds; KC7's four-frame edges include c + 2τ for a bore and a shaft (`e_tau`); M4-4, VR20 and the example's measures restated.
- **RS7 (medium): interior span ends are neighbour positions.** A span ending inside the target's wall ends where its occupant ends, known only to the relation's floor, yet VR7 compared it at ε. No current instance (every verifiable bore with a floor above 0.001 mm runs rim to rim). Fix: SR7 widens interior span ends by τ in the socket PR. Without M4-1 this stays open: M4 cannot tell an interior end from the socket alone.
- **RS8 (high): the warning policy was ungated** while VR12 changes alignment, a seat requirement and span clipping. Fix: `grip_warning` is gated like the default (M4-8, M4-33, KH7, KC14): only a grip-length defect, or the floating seat's offset, may pass, carrying `grip_differs`; a seat floats only if it is the far seat of every grip it belongs to. Superseded in round 3: M4 offers D7 `hard` only (28.4).
- **RS9 (medium): the oracle's `collides` rule contradicted its own cases** (the lug collides with the plate the bottom seat cites) and its obvious loosening would excuse RS1 to RS5. Fix: KO1 and KO3 excuse only volume outside every requirement's region, computed from the socket.
- **RS10 (medium): no bad case for candidate-side reading.** Fix: the classes above in KC9, with the review's `gen.py` as the generator's starting point (15.3, 15.4), and KH14's `CAN_FAIL` list: every rule that can fail a candidate needs a bad case citing it, checked by the coverage test.
- **RS11 (low): VR11's stated bound.** "Uncovered lies within ε/2 of R's boundary" does not follow (a narrow lobe); the proof gives every point deeper than ε covered. Fix: ε in VR11, (w4), SG6 and merge finding 2; SG6's area bound re-derived on the ε band (twice the perimeter × σ).
- **RS12 (low, M3 code): a conical seat stated only its first neighbour's radial range** (`socket/combine.rs` `cones()`), which the coordinator confirmed. Fix in the socket PR: the union of the seats' ranges (SR8), with s60, two neighbour cones on one target cone.
- **Y4 under the fixes.** The reviewer found Y4 holds as stated, at the prototype's budgets, and conditioned it on running with the shipped ones: VR21 now does. Each fix keeps Y4 by the argument given with it (cross holes are hole-sense, the original's openings sit at its span ends, its outline is known, B1 takes VR4's guard, span ends clip to the original's wall, its own posts make a bore unverifiable). Not yet measured: section 18.

### 28.2 Consistency

- **RH1 (high): M4-1's cost was wrong.** `SocketDocument::from_json` is strict (`deny_unknown_fields`, a digest recomputed from the structs), so a rules-5 document fails it after SR1's fields; the CLI's `carry_over` then carries nothing silently, and `foreign()` refuses to overwrite the user's own reviewed file. Fix: MF11 (SR9), a lenient reader of older documents' reviews and requirements; M4-1's cost states what is kept; a CLI test runs `socket -o` onto a rules-5 document.
- **RM1: the coverage gate could not show the floor rules or the socket gate in the corpus** (no corpus target states 0.01 mm or more; KC2 had no field for an edited socket; the `UNVERIFIABLE` check ignores deferral). Fix: KC2's `socket.floor`, `socket.edit` and `options`; s61 to s63; the lists join the test when their cases land, honouring M4 deferrals until the exit.
- **RM2: 74 × 4 original runs cannot be built.** Six targets have no variant (s10's three, s22's BLOCK, s55's two), and B's 210 runs were 68 × 3 + 6, not 70 × 3. Fix: KC11 names them and counts 314 runs; section 18's row corrected; the `_pc` copies marked as not yet run.
- **RM3: PR 3 depended on PR 5** (the builders, `--candidate-set` and the expectation schema). Fix: they move to PR 3; KH1's binary path waits for the CLI (PR 5).
- **RM4: no constructor gave a refused candidate's verdict.** Fix: `Verdict::refused(codes)`, crate-visible, called only by `judge_step` (VD1).
- **RM5: the coverage budget had no value.** Fix: `MAX_COVERAGE_STEPS` = 2·10⁷ per candidate and socket, about 5 s at the measured rate, shared by VR11 and VR22, with every budget's worst-case time stated (AL11, M4-13) and a hostile tree test.
- **RM6: match cases would break `corpus.rs` and `golden.rs`.** Fix: `kind` on every case and the loops filtered by kind in PR 5 (KC2).
- **RM7: Y4 under other policies was reported as measured** without kept output, and grip warning without VR12. Fix: section 18's row says so; VR21 under `grip_warning` is first measured in PR 3. Since round 3, grip warning is not offered in M4.
- **RL1 to RL13 (low).** Counts by M3's predicate restated with VR6's (71, 70; 25 against 26, 9 against 11). AS1's clean sockets: 12 documents, 17 sockets, 20 in all. The requests renamed SR1 to SR9, AL5's lines ℓ₁ and ℓ₂, "AL7 (c)", "B's F6". D9 and D8 got rows (M4-37, M4-38). Exit 0 for `Match` stated as a departure from M2's note. VD14's `--original` check moved after the socket gate. The API: `MatchOptions::new`, `Summary`, `Outcome`, `Window`, `Blocked`, `Turn`, `Slide`, `At`, `CheckSet`, `Warning` declared, `match_candidates` for tests, the opening sentence softened. Merge finding 1's citation dropped ("2 of 8" proves nothing). SL9's mean replaced by 46.4 against 20.7, which reproduces. Section 20.1's column cited to the parts' own uncertainties. U9 to U11, X8 and Q3 named as M3 rules with cases. KH10's PR, `MATCH_RULES_VERSION`'s value at the publish, KC14's policies and EV2's fields made consistent. The measured commit cited as `3078db8`, whose tree is `08c70a9`'s.

### 28.3 Soundness, round 2

The soundness reviewer read the revised design against its round-1 copy (`m4-evidence/merge/m4-match.before-review.md`), added `nut_long` and `nut_bar` to `review-soundness/gen.py` and wrote `review-soundness/u11/`, a Rust probe on `fitment-core` that examines every original's own stack against its verifiable bore and shaft requirements (`u11.txt`). Every round-1 false positive is now rejected by the rules as written, each by hand from its features (B's prototype implements none of the revised rules). Six new findings, all taken:

- **RS13 (critical): VR7's second branch extended the profile past the span, while CF8 and VR22 stopped at the span.** `nut_bar` (s01's nut 5 mm taller below, its Ø5 hole straight through to a new opening, a Ø1 bar across it at station 6) passed by the rules: the span covered, the opening reached through a Ø5 profile, the bar outside CF8's and VR22's stations; BOLT dV +3.91 mm³ beyond `nut_long`, and B's prototype passes it too. Fix: when branch 2 is used, the extension from the span's end out to the opening is a span for CF8 and VR22, with the end span's clearance core (VR7); `nut_bar` is a `b_open_past_rim` case.
- **RS14 (high): branch 2 continued a pinned interference past the original's engagement.** `nut_long` passed with the pinned Ø5 out to its opening, while the Ø6 bolt interferes over 2 mm more: +17.28 mm³ against the original's 43.20, where KO1 allows 0.39. Fix: branch 2 needs the end span's clear diameter, the protruding shaft's, and the extension must clear it by VR4's margin; with none stated (a pinned `not_read` span) the opening must be within ε of the span's end (VR7); `nut_long` is a case.
- **RS15 (high): under `grip_warning`, the floating seat's neighbour stayed put.** s02's block with its counterbore 2.5 deeper and a Ø9 lip inside the Ø11 passes by the rules as they stood (the floor floats, the shank span is clipped, the head span stays at [8.5, 14.5] above the lip), yet the reseated head cuts the lip, 14.9 mm³. Fix: every requirement sourced from a neighbour bearing on the floating seat moves with the float and is checked there; a seat whose requirement also comes from a neighbour not on it does not float, and its grip stays hard (VR12, M4-8, KH7); `b_float_lip` is a bad case under `grip_warning`. Superseded in round 3: M4 offers D7 `hard` only (28.4).
- **RS16 (high, Y4): VR22 failed originals.** `u11` found 6 verifiable requirements in 5 corpus targets whose original meets VR22's core: the face a shaft rises from at its span's end (s01's and s05's bolts, s19's cover, s54's two pins), the shaft's unbounded outer core (s50 CAP's own skirt round its spigot), and VR8's shaft mirror emptying the spigot. Fix: the core is not widened past an end that is the stack's own root or bottom, and stops there exclusively (VR22); a shaft's outer core is bounded by the original's free radius, which the socket PR states (SR10, new M3 rule X9); VR8's shaft mirror applies only to a post inside it. Then measured, with the review's probe extended (`m4-evidence/merge/y4probe/`): of the 110 targets, 7 fail everything by the floor; the other 103 have 172 verifiable bore and shaft requirements with 213 spans; 4 requirements are set aside (U9 ×2, s18's zone span pending SR1, U11 for s50 CAP's skirt bore), and the remaining 212 spans are all clear (`y4-vr22.txt`). The same probe reproduces the reviewer's 6 failures without the root and bottom rule (`y4-vr22-without-n4a.txt`) and finds 3 with an unbounded shaft core (`y4-vr22-unbounded.txt`), so each part of the fix is needed. No rule needed a further change and no requirement needed a new M3 rule beyond SR10. The free radius (SR10, X9) was withdrawn in round 3 for an unbounded core with U11 extended (28.4, RS19).
- **RS17 (medium): U11's premise, measured.** Exactly one original has its own post in a required span: s50 CAP's skirt bore (`u11.txt`). It passes Y4 today and becomes unverifiable under U11, so section 18's "changes by s18's plate bore only" was wrong; section 18, VR6 and SR5 now name s50 CAP, which is U11's corpus case beside the planned s64.
- **RS18 (low): text.** VR22 claimed `nut_ext`, whose step annulus is a transition VR8 takes and VR22 exempts: only VR7's rim test rejects it, and `housing_spike` is the case only VR22 rejects (`CAN_FAIL`). Merge finding 1 and M4-10 no longer state the coverage false negative as "at most ε/2". MF11 compares arrays and nested objects whole on their shared keys, so a span list that splits or merges counts as changed.

### 28.4 Soundness, round 3

The soundness reviewer read the round-2 changes against `m4-evidence/merge/m4-match.round1.md` and re-ran a copy of `merge/y4probe/` with its exemptions switched off (`review-soundness/y4probe-check/`). All three round-2 cases now reject. Two new findings came from the round-2 mechanisms themselves, the free radius and VR12's floating seat, so both mechanisms were withdrawn for simpler, conservative rules rather than patched a third time:

- **RS19 (critical): the free radius R_f was one radius per span, while the neighbour's region varies along it.** A base whose Ø6 pin is ringed by a short square tube (R_f = 10 from the tube) and a candidate whose tube is carried 4 mm higher: every face of the tube lies at or beyond R_f, so VR22 tested none, and the taller tube cuts the plate's groove, 270 mm³ with the neighbour the pin's requirement cites (by the rules; physics by hand). The reviewer proposed R_f stated per station. **Fix, withdrawn for a conservative rule:** R_f, request SR10 and M3 rule X9 are withdrawn. A shaft's outer core in VR22 is unbounded radially over the span's stations, still stopping at the candidate stack's own root or bottom within ε of the span's end. Where the original's own material lies in a span's core, bore or shaft, M3's U11 (extended, SR5) makes the requirement unverifiable, so a reviewer can remove it after looking. Measured with the probe so changed (`merge/y4probe/y4-vr22.txt`): 4 requirements become unverifiable, s50 CAP's skirt bore and spigot and s54's two pins (round 2's unbounded run had named the same three shafts); every other span passes, 209 of 209. The cost, false negatives on shafts beside the original's own material, is stated in 27.1 and the per-station radius left as open question 17.
- **RS20 (high): VR12's "does not float" clause was vacuous,** since a seat requirement's sources are by construction the neighbours bearing on it, and a chain of neighbours (a washer between a screw head and a floating counterbore floor) moved the washer's span but not the screw's, reopening the lip false positive under `grip_warning`.
- **RS21 (medium): moved spans ran past the datum seat,** so a thinner part rejected with `wall_missing` and KH7's grip-only cases would have failed their own gate.

  **Fix for RS20 and RS21, withdrawn for a conservative rule:** the floating seat is dropped. M4 offers D7 `hard` only; `warning` is deferred to M7, whose body-fit verifier places the neighbours themselves and can check where they reseat (M4-8, VR12, VD13). B had shown a warning on the grip check alone changes nothing, since the two seats fix the separation, so a warning without a float would be a false promise. The CLI has no `--grip` option in M4; the document still records `"grip": "hard"`, and `grip_differs` is reserved for M7. KH7, KC14, M4-33 and the run counts now name two policies; `b_float_lip` and the grip-only expectations move to M7; A's A-D9 (no seat separation in the signature under warning) is moot in M4. Cost: every grip-only change of thickness is a false negative in M4.
- **RS22 (none): the stop at the stack's own root or bottom opens no path.** A bottom or root placed above the span's end fails VR7's wall test first, and a thin root disc adds no material on the neighbour's side. VR22 now says "the candidate stack's own end within ε of the span's end", as the probe implements.
- **The probe, checked.** Read in full and spot-checked by hand (s19's cover and housing, s50 CAP). Two choices are more lenient than VR22's text: it exempts every transition outside the stack's narrowest wall, and every non-coaxial hole-sense cylinder when the stack has any wall opening. Re-run with neither: the 11 step annuli that then fail are all ones VR8's per-zone condition takes, and the cross-hole exemption is unused, so neither changes a result. Its limits are stated in VR22's paragraph: it tests VR22 and VR4's guard on the originals only, not VR7, VR8's zone rule as written, CF8 or alignment.

Why the clever fixes were withdrawn. Each round-2 mechanism answered a real round-1 finding, and each opened a new path within one round: R_f because one number cannot describe a region that varies along the span, the float because the socket records target-to-neighbour relations and a reseating neighbour can act through another. Both mechanisms add passes the original never had; the conservative replacements (an unbounded core with own material made unverifiable, and D7 `hard` only) add none, cost stated false negatives, and are simple enough that their soundness argument fits in a sentence.

## Evidence

Under [m4-evidence/](m4-evidence/), not committed:

- `BRIEF.md`, the brief all four worked to; `A.md` to `D.md`, their reports; `review-soundness.md` (with `review-soundness/`: `gen.py`, `physics.py`, `physics.txt`, `proto.txt`, `out/`) and `review-consistency.md`, the two reviews of section 28.
- `A/`: `probe/` (Rust on `fitment-core`: `sig.rs` the candidate signature, `req.rs` the socket signature, `shortlist.rs`, `index.rs`, `mutate.rs`, `main.rs` the censuses) and `out/report.txt` (C's single floor applied), `out/report-vd6-off.txt` (no floor rule: the mutations and the policy sweep).
- `B/`: `proto/` (Rust: `align.rs`, `verify.rs`, `region.rs`, `cand.rs`, `v.rs`), `gen_cands.py`, `attrib.py`, and `out/` (`census.txt`, `y4.txt`, `y4_verbose.txt`, `y4_seeded.txt`, `fix.txt`, `as1x.txt`, `dcands.txt`, `dcands_functional.txt`, `dcands_gripwarn.txt`, `attrib.txt`, `bcands.txt`).
- `C/`: `sockets.py` and `sockets/` (110 socket documents), `unverifiable.py` and `unverifiable.txt`, `props.py` and `props.txt`, `gen_cand.py`, `cand/`, `feat/`, `original/` and `orig/` (the originals' runs), `proto_match.py` and its documents (`s01_match*.json`, `s07b_match_functional.json`, `bulk_match.json`, `bulk_table.txt`).
- `D/`: `gen_candidates.py`, `readback.py`, `oracle_match.py`, `edge_frames.py`, `size_frames.py`; `out/` (the slice: 40 candidates and `manifest.json`), `edge/` (120 edge files), `sizes/`; `readback.txt`, `oracle.txt`, `edge.txt`.
- `merge/`: `splitfloor.py`, which derives section 20.1's split-floor column from A's per-socket rows; `y4probe/` (Rust on `fitment-core`, extended from `review-soundness/u11/`: `y4-vr22 CORPUS_DIR FIXTURES_DIR`; outputs `y4-vr22.txt`, `y4-vr22-without-n4a.txt`, `y4-vr22-unbounded.txt`), which measures VR22 and VR4's guard on every original; this document's earlier states (`m4-match.before-review.md`, `m4-match.round1.md`) and the parts it was assembled from.
