# fitment M7: the fit verifier

As of 2026-10-22. Status: **proposal for review**. Nothing here is implemented; section 11 is the order of work. The rest of the document assumes every default in section 2.

M7 delivers the fit verifier. A candidate whose interface passes is placed in the assembly at every occurrence of the part it replaces, and its body is checked against every other solid and against its own other copies by Open CASCADE, run in a separate helper process (D1). Only a verified fit makes a `Match`; a body that interferes makes a `Reject`; everything else leaves an `InterfaceMatch` with the reason the fit is not verified. PLAN's exit for M7: "On the corpus, colliding candidates end as `Reject`, or as `InterfaceMatch` with fit not verified, never as `Match`; the original part passes its own fit check." This document is that design.

**How it was made.** One designer, on `m7/design` (cut from `m6/release`, `51d671d`: fitment 0.3.0, `EXTRACTOR_VERSION` 6, `SOCKET_RULES_VERSION` 7, `SIGNATURE_VERSION` 2, `MATCH_RULES_VERSION` 1), read PLAN, ARCHITECTURE, the D1 research, M4's verdicts, fit status, MF rules, corpus and gate, M3's V1, M5's QR6, QR13 and ST11, M6's OP9, SN11, SV8, JS1 to JS8 and its image, the code's `FitVerifier` seam (`crates/fitment-core/src/fit.rs`, `matching/matcher.rs`, `fitment-server/src/matching.rs`) and the two Open CASCADE tools (`tools/gen-corpus.py`, `tools/verify-occt.py`). Every claim that rests on Open CASCADE's behaviour was checked by a probe on cadquery-ocp-novtk 8.0.1.1.0 (the pin of `tools/verify-occt.py`, and so of the helper), on an Apple M5 Max under macOS (Darwin 27), with the corpus regenerated at this commit (`tools/gen-corpus.py corpus/out`: 463 files, 129 match cases) and the release `fitment` built from it. The most important probe is a working prototype of the whole check (P2), run over the corpus and the fixtures.

**Evidence.** The probes and their outputs are committed in [m7-evidence/](m7-evidence/), as M5's were (M1 to M4's were not): each script is a `uv` script with the helper's pin, and each `.txt` is its output with local paths replaced. They are cited by name:

| Probe | What it shows |
| --- | --- |
| `p1_kernel.py`, `p1_kernel.txt` | Open CASCADE's Common and distance on overlaps from 1e-3 to 1e-8 mm, touching and gaps, in the identity and a skew frame; containment; shafts in holes; one Common's volume 20 times, serial and parallel; face thickening; a corpus solid offset by 0.01 mm |
| `p2_fit.py`, `p2_fit.txt` | The fit check of sections 4.1 and 4.2, prototyped on `tools/verify-occt.py`'s readers: every corpus original at its own occurrences (the control) and every file match case at fitment's own placements |
| `p2_fixtures.txt` | The control on every fixture target `fixtures/manifest.json` pins a socket for |
| `p2_determinism.txt` | P2 run twice: identical |
| `p3_process.py`, `p3_process.txt` | A helper's start, import and peak memory; seven hostile files read by Open CASCADE |
| `p4_tolerance.py`, `p4_tolerance.txt` | The largest sub-shape tolerance of every solid of the fixture assemblies; the binding's version strings |
| `p5_binding.py`, `p5_binding.txt` | How the helper can find fitment's bodies: by a solid's own `#id` (it cannot), by its faces' `#id`s and its placement (it can, uniquely, on 13 assemblies) |

**Rule ids.** Rules are numbered by area from 1, in bold (`**FT4.**`), each fitting the coverage test's pattern (`[A-Z]{1,2}[0-9]+[a-z]?`). Every prefix was checked unused in `docs`, `crates` and `tools` (`git grep -nE '\bPREFIX[0-9]+[a-z]?\b'`, zero hits each): FT (the check), FZ (zones), FB (binding), FV (the fit's status from measures), FH (the helper's protocol), FL (the helper's lifetime), FY (security), FC (fitment-core), FM (the document), FG (packaging), FU (the CLI), FX (the server), FK (the corpus) and FO (the oracle). Test ids are VT51 onward, so they never collide with M6's VT1 to VT50; they are not rule ids and are never written in the bold rule form. Decision ids are M7-n.

## 1. Summary

- **What is checked.** At each occurrence k of the target part, the candidate placed by its interface's own placement, T_k · A, never re-aligned (FT2), against every solid of the assembly but the selected occurrences, and against its own copies at every other occurrence of every socket (FT3, M3's V1). Two tests per pair: **interference**, the Common of the two bodies less the zones the socket declares, must have no volume (FT4); and **proximity**, the body less the zones taken twice as deep, must stay farther than δ from the candidate (FT5), because Open CASCADE absorbs an overlap of 1e-7 mm and less (P1). δ is the policy's linear tolerance, 0.01 mm by default, never below the stated uncertainty of the bodies (FT7).
- **Zones are the assembly's declared contacts,** derived by fitment-core from the socket's requirements, never by the helper: each seat contact's neighbour face swept δ either side of its plane, each interference, line-to-line or unread-occupant span's annulus, each conical seat's shell (FZ1 to FZ8). Clearance spans, grips and continuations declare none: a shaft that touches a clearance hole is not verified.
- **The original is the control** (FT9): checked once per run at its own occurrences with the same zones; if it is not verified, no candidate's fit is. P2: every corpus original whose interface passes is verified (56 of 57) but s20's, whose assembly puts a block 0.05 mm into the plate by construction; every fixture original whose interface passes is verified (12 of 12: AS1's ac, md and ug exports).
- **The prototype works on the corpus.** P2 at fitment's own placements: the 3 `collides` cases interfere (299.4, 56.5 and 4 × 5.97 mm³), the 28 `fits` cases that pass their interface are verified, in 0.00 to 0.27 s each; of the 41 `does_not_fit` cases fitment already rejects at a placement, 27 also interfere, 2 raise a kernel warning and 12 are verified (their defects keep the body clear). Two runs are identical to the last digit (`p2_determinism.txt`).
- **Rules in Rust, geometry in Python** (M7-9). fitment-core lists the bodies, their transforms, landmark faces and zones from its own reading; the helper builds solids, runs booleans and distances, and returns measures; Rust decides the status from the measures by closed rules (FV1 to FV9). The helper holds no rule a test of fitment cannot see.
- **The status.** Interference of at least 1e-6 mm³ outside the zones: `interferes`, a `Reject`. Any kernel warning, an invalid body, a shape tolerance above δ/10, a binding that disagrees, a proximity, an interference below 1e-6 mm³, a control that fails: `not_verified`, an `InterfaceMatch`. Only everything clear is `verified` (FV2 to FV7). Open CASCADE's Python binding exposes no warnings on `BRepAlgoAPI_Common` in 8.0.1; the helper uses `BOPAlgo_BOP`, which does (P1, M7-17).
- **The helper's process.** One session per run (the CLI) or per job (the server), holding at most one helper per judging thread; each helper is long-lived, loads the assembly once and judges candidate after candidate, and is replaced after any failure or 256 requests (FL1 to FL3). uv prepares its environment once; the helper then runs as that environment's own interpreter, so a kill reaches the process that holds the kernel (uv's child would be orphaned) (FL2, M7-12). Frames are length-prefixed binary on a private file descriptor: Open CASCADE prints its errors on standard output (P3), so the helper moves fd 1 onto stderr before importing it (FH2).
- **A stale or wrong helper is refused** (FH3): the helper script and its lock are embedded in `fitment-fit`, written to a cache directory under their SHA-256, checked on disk before every spawn, and the helper's handshake must state protocol 1, the script's and the lock's SHA-256, cadquery-ocp-novtk 8.0.1.1.0 and Open CASCADE 8.0.1. A helper that cannot start refuses `--fit` (exit 1) or the server's start, never a silent `"none"`.
- **Failures are never a pass** (FV8). A timeout kills the helper; that candidate's fit is `not_verified` (`helper_timeout`) and the run is incomplete (exit 1, a `failed` job), since a timeout depends on the machine. A crash is retried once in a fresh helper; a second is `not_verified` (`kernel_crashed`). Memory is bounded by `RLIMIT_AS` where the OS honours it (Linux), and by the timeout everywhere.
- **The document** (M7-8). A run without a fit verifier writes `fitment.match` v1, byte for byte as 0.3.0 does; `MATCH_RULES_VERSION` stays 1. A run with one writes `fitment.match` **v2**, v1's superset: the verifier's identity, the fit's parameters, and per candidate and socket the fit's evidence; the kernel's measured numbers sit in `measured` objects outside the digest, so two OSes agree on the digest wherever they agree on every decision (FM1 to FM7). The fit's own rules are versioned by `FIT_RULES_VERSION` 1.
- **Free placements** (M7-5). Where the interface leaves a turn about an axis or a slide along it free, the fit is judged at the stated placement and the evidence says what is free; an interference there is `not_verified`, not a `Reject`, since another turn might fit. D1 asked `not_verified` for every free placement; that would cost every one-axis socket a `Match` (18 of the 33 socket results of P2's passing cases are seeded by one axis).
- **Packaging.** `fitment-fit`, a published, pure-Rust, MIT library holding the protocol, the session and the embedded script, is the only crate that spawns a process. The CLI's `fit` feature, on by default, adds `fitment match --fit ASM` and `fitment fit setup`; no Open CASCADE is in any crate, binary or official image (FG1 to FG6). The official image is unchanged and refuses `fit.verifier = "occt-helper"` at start; a committed recipe, `deploy/Containerfile.fit`, builds a user's image with uv and the locked environment beside the official binaries, and CI builds and smoke-tests it without ever pushing it (M7-20, FX8).
- **The catalog** reads a blob only for a candidate whose every socket interface passed, its SHA-256 checked (QR13, ST11), and asks the fit per member of a dedup group (QR6) (FC6).
- **D7's grip as a warning is deferred** past M7 (M7-23): it needs neighbours moved along their axes to reseat, a kinematic model of fastener stacks that the fit check does not need, and it would recover about four corpus cases (section 12).
- **Six PRs** (section 11): core's seam and the v2 document; `fitment-fit` and the helper; the CLI and the catalog; the corpus's fit cases, the oracle and the three-OS fit job; the server and the image recipe; the exit and the release 0.4.0.

## 2. Decisions for review

Each row has a default. Rows marked **Your call.** change what fitment promises, ships or prints; the rest are engineering defaults.

| # | Question | Default | What it costs or buys | Alternatives |
| --- | --- | --- | --- | --- |
| M7-1 | **Your call.** What a fit is judged against | **Declared zones** (D1 section 3): interference and proximity are allowed only inside zones derived from the socket's requirements, with the original as a control (FT9) | **Buys:** a candidate is held to what the neighbours require, never to what the original happens to do, as PLAN says of the interface; the control catches what the socket missed. **Costs:** an original whose assembly already interferes, or touches where no requirement reads, leaves every candidate `not_verified` (s20 in the corpus) | **Differential** (the oracle's KO1): the candidate may add no common volume to the original's. Simpler, but it excuses whatever the original does wrong, and it compares a candidate with the original |
| M7-2 | What a fit tests | Interference outside zones at depth δ; proximity within δ outside zones at depth 2δ; every occurrence of every socket; every leaf solid but the selected occurrences, and the candidate's other copies (FT2 to FT5) | P2: the 3 `collides` cases interfere, every interface-passing `fits` case is verified | Interference only (D1's test 2): a 1e-7 mm overlap is invisible to the kernel (P1), so a candidate could pass at the tolerance's edge |
| M7-3 | δ, the guard and the volume resolution | δ = max(the policy's linear tolerance, the largest stated uncertainty of the two bodies); guard 1e-6 mm (`GUARD_MM`); volume resolution 1e-6 mm³ (KO1's `VOLUME`) (FT7, FV3) | The edge rejects as M4's does: a clearance of exactly c + 2τ reads as a proximity (P2: the four `bracket_dia_clear` frames at 0.0099999999 mm) | δ fixed at 0.01 mm whatever the policy |
| M7-4 | A proximity or a sub-resolution interference | **`not_verified`** (FV3, FV4) | An `InterfaceMatch`, never a false `Reject` of a body that may only touch | D1's `Reject("undeclared contact")`: touching is not interfering (KC3's `fits`), so a reject there is a false negative |
| M7-5 | **Your call.** A placement the interface leaves free (a turn about the axis of a symmetric socket, a slide no seat fixes) | **Judged at the stated placement,** the freedom named in the evidence; an interference there is `not_verified` (`interferes_at_free_placement`), not `Reject` (FT10, FV6) | **Buys:** `Match` stays possible for nuts, washers, pins and every one-axis socket (18 of 33 socket results of P2's passing cases are one-axis seeds). **Costs:** a `Match` says the body fits at the placement it states; the installer must hold that turn. Motion is out of scope (FT12) anyway | D1's rule: every free placement `not_verified`. Or sweep the turn (many kernel calls per candidate, and still a sample) |
| M7-6 | The original as control | Once per run and socket document, with the same helper and zones; not verified means every candidate's fit `not_verified`, `original_fails_fit` (FT9, FV5) | Catches socket omissions and assemblies that already interfere. P2: 56 of 57 interface-passing corpus originals and 12 of 12 fixture ones verified | No control (an omission in the socket would read as clear) |
| M7-7 | When the fit is asked | **Once per candidate, after every socket's interface passed,** with every occurrence of every socket in one request (FT11, FC2) | V1's copies are tested across sockets (a copy in `socket-2` against one in `socket-1`); no kernel time on a candidate that already rejects (M4-26 extended) | Per socket, as M4's seam asks: copies in other sockets would be unknown |
| M7-8 | **Your call.** The document | **v1 unchanged without a verifier; `fitment.match` v2 with one.** v2 adds the verifier's identity, the fit's parameters, the evidence, and `measured` objects outside the digest. `MATCH_RULES_VERSION` stays 1; `FIT_RULES_VERSION` 1 is new (FM1 to FM7) | **Buys:** every 0.3.0 document and golden byte-identical (VT52); v1's closed schema (`additionalProperties: false` on `verified` and `interference`) is not broken. **Costs:** two writers of one format family, and two schemas in the OpenAPI document | v1 only, the evidence squeezed into its four shapes (a `neighbor` string, a `version` string carrying the helper's hash): no box, no occurrence, nothing machine-readable; or v2 for every run, which changes every document |
| M7-9 | Where the rules live | **Rust.** fitment-core derives bodies, landmarks and zones; the helper builds and measures; fitment-fit and core decide (FV1) | The rules are tested without Python on three OSes; the helper is a calculator, its script small | The helper decides (D1's sketch): the rules would be Python, tested only where uv runs |
| M7-10 | How the helper's solids are known to be fitment's | **By their faces' `#id`s and their placement:** each body fitment names is the one XCAF leaf whose faces bind to those `#id`s and whose location agrees with fitment's transform; up to four landmark faces per body compared with fitment's own reading (FB1 to FB5) | Catches units, frames and placements read differently by the two readers. P5: unique on 265 leaves of 13 assemblies. XCAF's transforms alone do not identify a leaf (`moon_buggy_asm`, `p2_fixtures.txt`), and a solid's own `#id` finds nothing (P5) | Leaves matched by product name and transform, as the oracle does |
| M7-11 | The process model | **A session per run or job; at most one helper per judging thread; long-lived, reused, replaced after a failure or 256 requests** (FL1, FL3) | Start-up (0.3 s warm, P3) and the assembly's read are paid once per helper; a hostile file's damage is limited to one helper; no helper is shared between jobs or users | One process per request: about 0.3 s and an assembly read per candidate; or one helper per machine: one crash stops every run |
| M7-12 | How the helper is started | **uv once, then the environment's interpreter directly** (`python -I helper.py`), the interpreter's path read from `uv run --locked --script helper.py --print-env` and kept per session (FL2) | `Child::kill` reaches the kernel's process. `uv run` keeps Python as its child, which a SIGKILL of uv orphans | `uv run --locked --script` per spawn (D1's sketch) |
| M7-13 | Framing | **Binary, length-prefixed, on a private fd;** fd 1 moved onto stderr before Open CASCADE is imported (FH1, FH2) | P3: Open CASCADE prints `**** ERR StepFile …` on stdout, which would corrupt line-based JSON | JSON lines on stdout (D1, workspace layout) |
| M7-14 | Identity and versions | Protocol 1; script and lock SHA-256 embedded and checked on disk and in the handshake; cadquery-ocp-novtk 8.0.1.1.0, Open CASCADE 8.0.1 and Python 3.13 stated by the helper and required (FH3, FH6) | A stale, edited or wrong helper never answers | Version strings only |
| M7-15 | Timeouts and crashes | Per candidate `fit.timeout_seconds` (120); on expiry the helper is killed, the fit is `not_verified` (`helper_timeout`) and the run is incomplete (exit 1). A crash: one retry in a fresh helper, then `not_verified` (`kernel_crashed`), the run complete (FV8) | A document never depends on the machine's speed without saying so through exit 1; a file that crashes the kernel is a stable `not_verified` | A timeout as a plain `not_verified`: the same inputs could give two documents |
| M7-16 | Memory | The helper sets `RLIMIT_AS` (default 4 GiB), `RLIMIT_CPU`, `RLIMIT_CORE` 0, `RLIMIT_NOFILE` and `RLIMIT_FSIZE` on itself before importing the kernel; honoured on Linux; on macOS and Windows the timeout is the only bound (FY3) | No `libc` or `unsafe` in fitment-fit (`unsafe_code = "forbid"`); the server's image is Linux | A memory poll from Rust: std has none |
| M7-17 | Kernel warnings | `BOPAlgo_BOP` for every boolean, its warnings and errors read; any alert is `not_verified` (FV2) | P1: OCP 8.0.1 binds `HasWarnings` on `BOPAlgo_BOP`, not on `BRepAlgoAPI_Common`; P2 met `BOPAlgo_AlertFaceBuilderUnusedEdges` on two bad cases and none on a good one | `BRepAlgoAPI_*` with `IsDone` alone: warnings unseen |
| M7-18 | What Open CASCADE may read | Only the socket's assembly (its SHA-256 the socket's `source`) and candidates M2's gate accepted, their SHA-256 checked again in the helper (FY1) | P3: a file with `1.E400` coordinates transfers in Open CASCADE without complaint; fitment's gate refuses it before the helper ever sees it | Every candidate file |
| M7-19 | **Your call.** The CLI | The `fit` cargo feature **on by default**; `fitment match … --fit ASM` (the socket's assembly); `fitment fit setup` prepares and checks the helper; `--fit-timeout`; `--uv` (FU1 to FU6) | `cargo install fitment` can check fits wherever uv runs; the image's CLI has the feature and no uv, so `--fit` there exits 1 saying so | Off by default (PLAN: "compiled into release binaries", which are crates.io and the image) |
| M7-20 | **Your call.** The server's image | **Unchanged and official; no Open CASCADE.** `fitment-server` is built with `fit`; `fit.verifier = "occt-helper"` needs uv and the environment, which the official image lacks, so it refuses to start (M6's F28). A committed recipe, `deploy/Containerfile.fit`, builds a user's image (Debian slim, the official binaries, uv, Python and the locked environment), built and smoke-tested by CI on every PR and never pushed (FX8) | Keeps D1's "no Open CASCADE in official artifacts" while issue #1564 is open; the user who builds the image is its distributor | A sidecar container serving fits over a socket (a second protocol, authentication, a shared kernel across users); a second official image (D1 forbids it) |
| M7-21 | The catalog | A blob is read only for a candidate whose every socket interface passed, hash-checked (QR13, ST11); dedup shares the interface, the fit is asked per member (QR6) (FC6) | CT6 stays true with `NoFitVerifier`; with a verifier, blob reads equal interface passes (VT65) | Fit for every survivor |
| M7-22 | Component candidates | A component's body comes from its assembly by its occurrence and its faces' `#id`s; the server gives the bytes of any stored assembly by SHA-256 (FC5, FX5) | The original's control and the API's component cases run through the same path | Components never fit-checked |
| M7-23 | **Your call.** D7, grip as a warning | **Deferred past M7.** `Grip::Hard` only; `grip_differs` stays reserved | Grip-only thickness changes stay false negatives (`bracket_thick`, `b_thickness`, `washer05_thick`, `plate01_thin`) | Build it: the neighbours on the gripped seats (bolts, nuts, washers) moved along their axes to reseat, then the fit checked in that moved scene. Section 12 |
| M7-24 | Shape tolerance | A body in a checked pair whose largest sub-shape tolerance exceeds δ/10 makes the fit `not_verified` (FT6) | P4: AS1's exports at 1e-7 or 1e-5 mm pass; two of vaccase's solids (0.0029 mm) and one of the weldment's (0.058 mm) fail | No cap: Open CASCADE absorbs overlaps up to a shape's tolerance |
| M7-25 | Determinism | Same bytes on one platform and helper (measured); across OSes the digest, which leaves out the kernel's measures, is compared by CI on three OSes (FM5) | v2's bytes may differ across OSes in the last digits of a volume; its digest may not | Round the measures (a value at a rounding edge still flips) |
| M7-26 | CI | A `fit` job on Ubuntu, macOS and Windows with uv, `FITMENT_REQUIRE_FIT=1`, the corpus under `--fit`, reports and digests compared across OSes; adversarial helper tests on three OSes with no Python (section 7) | About 4 minutes per OS; wheels exist for all three | Linux and macOS only, if Windows' kernel disagrees (open question 7) |
| M7-27 | The corpus | New fit cases (FK2): D1's probes as candidates (touching, 1 µm overlap, contained, a bolt at major diameter in a hole at minor), a copy colliding with a copy, a collision at a free turn, a proximity only; `interferes_by_construction` on s14 and s20 | Every FV rule that can end a fit has a case | |
| M7-28 | The exit | PLAN's sentence, measured through the CLI and the API: every `collides` case `Reject` (`interferes`) or `InterfaceMatch` (`not_verified`), no `Match` on any bad case; every corpus and fixture original whose interface passes and whose assembly is not `interferes_by_construction` verified (FK1) | s14, s20 and the weldment's bearing and arm are named, with their measured overlaps | |
| M7-29 | The release | 0.4.0 at M7's exit: `fitment-core`, `fitment-catalog`, `fitment-fit`, then `fitment` (FG6) | `fitment-fit` reserves its name and carries the helper | |

## 3. How it fits together

### 3.1 The flow

```text
fitment match S.json CAND… --fit ASM            POST /v1/matches (fit.verifier = "occt-helper")
      │ ASM's SHA-256 = S's source, else exit 2        │ the socket line's assembly, by SHA-256
      ▼                                                ▼
OcctSession::open(helper, assembly bytes, socket)  ── fitment-fit
      │ the helper prepared once (uv), checked (FH3); FitScene from core: bodies, landmarks (FB1)
      ▼
Matcher::new(S, policy, options, &session)        ── fitment-core, unchanged contract
      │
      ├── the control: the original at its own occurrences (FT9), once
      │
per candidate: interface at every socket (M4)
      │ any socket rejected → no fit asked (FT11)
      ▼
FitRequest: the body (bytes or a component), every occurrence's placement T_k·A,
      │ the freedom of each placement, the zones of each (occurrence, body) pair (FZ)
      ▼
helper (one per judging thread, long-lived):  load scene once │ check: per pair Common,
      │   cut by zones δ, distance to the body cut by zones 2δ, alerts, validity, tolerance
      ▼
measures ──> FV rules in Rust ──> FitStatus per socket and per candidate (VD4)
      ▼
decide / combine (M4) ──> fitment.match v2 ──> exit 0 Match, 4, 5; 1 if a fit timed out
```

### 3.2 Crates and modules

```text
fitment-fit (new, published, pure Rust, MIT) ──> fitment-core, serde, serde_json, thiserror
  src/lib.rs          OcctHelper (a prepared, checked helper), OcctSession (FitVerifier)
  src/prepare.rs      the cache directory, the embedded files written and checked, uv (FL2, FL6)
  src/process.rs      spawn, the private fd, reader thread, deadline, kill, reap (FL1 to FL5)
  src/frame.rs        frames, caps, strict decoding (FH1, FH5)
  src/protocol.rs     hello, scene, check, bye (FH3, FH4)
  helper/fit_helper.py, helper/fit_helper.py.lock    embedded with include_bytes!
  tests/adversarial.rs  the test binary re-executed as a fake helper (VT55)

fitment-core (changed)
  src/fit.rs          FitRequest v2, FitBody, Freedom, FitMeasures, FitEvidence, FitReason,
                      FIT_RULES_VERSION, fit::decide (FV1 to FV9)
  src/fit/scene.rs    FitScene: every leaf solid, its faces' #ids and transform, landmarks (FB1, FB3)
  src/fit/zones.rs    zones from a socket's requirements (FZ1 to FZ8)
  src/matching/       the matcher asks once per candidate (FC2); placement freedom (FC4);
                      document.rs writes v2 with a kernel (FM1)

fitment-catalog       match_catalog takes a FitBlobs reader, used only past the interface (FC6)
fitment (CLI)         feature fit = ["dep:fitment-fit"], default; match --fit; fit setup (FU)
fitment-server        feature fit; [fit] verifier = "occt-helper"; sessions per job (FX)
deploy/Containerfile.fit, deploy/fit-smoke.sh        the user's image (FX8)
```

`fitment-fit` is the only crate that spawns a process; `fitment-core`'s clippy ban on `std::process::Command` stays. `deny.toml` keeps every Open CASCADE crate and `cxx` banned; `tools/check-core.sh` gains a check that `fitment-fit`'s normal graph is the four crates above.

### 3.3 One candidate, measured

s01's bracket socket, the lug candidate (`m_bracket_lug`: a tab 5 mm into the plate, in no requirement's region). Its interface passes at an axis-pair placement. The fit request holds one occurrence, five bodies within reach (the plate, four bolts; the nuts are culled by their boxes) and, for the plate, the slab of its top face cited by `seat:plane-…`. The plate's Common with the placed bracket is 300.0 mm³; less the 0.6 mm³ of it inside that slab, 299.4 mm³ remains: `interferes`, a `Reject` with the plate, its occurrence and the box of the residual as evidence. The run took 0.05 s in P2. The bracket without the lug (`m_bracket`): every Common empty or inside a slab, every distance outside the slabs above δ: `verified`, a `Match`.

## 4. Rules

### 4.1 The check (FT)

- **FT1. What a fit claims.** Static interference and clearance of the candidate's placed body against the modelled solids of this assembly, within δ, at the placements the evidence states. Nothing about motion, tolerance stack-up, assembly order or tool access, unmodelled items (fasteners not in the file, adhesives, cables), threads, or the fasteners a new part might need; every fit's evidence lists these as not checked (FT12).
- **FT2. Placements.** At each occurrence k of each socket, the candidate is placed by T_k · A, where A is the placement the interface passed at (M4's AL1, `SocketResult.placement`) and T_k the occurrence's transform from the socket document. Never re-aligned: the fit is a check of the interface's placement, not a search for another. An occurrence the matcher could not place gets no fit (M4's existing rule: "an occurrence of the socket could not be placed").
- **FT3. The check set.** For occurrence k: every solid of every leaf occurrence of the assembly but the selected occurrences of the target; the unselected occurrences of the target (under `--occurrence`) as ordinary neighbours; and the candidate's own placed copy at every other selected occurrence, in every socket (M3's V1). Neighbour-to-neighbour pairs are never tested. Each copy pair is tested once for each of its two occurrences, so each side's zones apply.
- **FT4. Interference.** For each pair (C_k, B) within reach (FT8): V = Common(C_k, B) through `BOPAlgo_BOP`, no fuzzy value, serial; R = V less every zone of (k, B) at depth δ (FZ6). R's volume is the interference. Common catches containment as well as crossing (P1: a 2 mm box inside a 10 mm box gives 8 mm³).
- **FT5. Proximity.** For each pair within reach whose distance (BRepExtrema, with its inner solution read as 0) is at most δ + guard: the distance from C_k to B less every zone of (k, B) at depth 2δ. If that is at most δ + guard, the pair is in proximity. Depth 2δ keeps the zone's own cut faces δ beyond any face a declared contact holds, so a contact inside a zone never reads as one outside it (P2: no original of the corpus with a passing interface reads a proximity).
- **FT6. Inputs fit to be judged.** The candidate and every body of a checked pair are valid (`BRepCheck_Analyzer`), with positive volume, and the largest sub-shape tolerance of each (`ShapeAnalysis_ShapeTolerance`) is at most δ/10: Open CASCADE absorbs overlaps up to a shape's tolerance (P1 absorbs 1e-7 mm at tolerance 1e-7). P4: the fixtures' solids are at 1e-7 or 1e-5 mm but three: two of vaccase's (up to 0.0029 mm) and one of the weldment's (0.058 mm).
- **FT7. δ and the guard.** δ = max(the policy's `linear_mm`, the largest `part_uncertainty_mm` and `placement_uncertainty_mm` the socket states for the occurrence and for B, and the candidate's `max_uncertainty_mm`). The guard is `GUARD_MM` (1e-6 mm), as VR2's. A proximity at δ exactly is a proximity (Soundness rule 4): P2 reads the four `bracket_dia_clear` frames, clearances at exactly c + 2τ, at 0.009999999993 to 0.00999999999999 mm.
- **FT8. Reach.** A pair is tested when the two bodies' boxes, each enlarged by 2δ plus its largest shape tolerance plus 1e-6 mm, overlap. Boxes are Open CASCADE's ordinary (non-optimal) ones: D1 found the optimal box too small on one B-spline case (OCCT #1604). A body outside reach is clear and is counted, not listed.
- **FT9. The control.** Before the first candidate's fit, the original is checked as a candidate would be: its own solid at each selected occurrence (T_k, the identity placement), against the same check set, with the same zones, its copies being the originals. Its status is recorded once in the document (FM3). If it is not `verified`, every candidate's fit is `not_verified` with reason `original_fails_fit` and the control's own reason (FV5). Computed once per session and socket document (the CLI's run, the server's job).
- **FT10. Free placements.** The matcher states for each socket's placement whether a turn about its seed axis is free (AL8 tried one turn because every active requirement is symmetric about the line) and whether a slide along it is free (no seat or end fixed it) (FC4). The fit is judged at the stated placement; `verified` records the freedom; an interference at a free placement is `not_verified` (FV6). A corpus `collides` case must collide at every placement its interface leaves free (FK3).
- **FT11. When.** The fit is asked once per candidate, after its interface passed at every socket, with every occurrence of every socket in one request. A candidate that rejects at any socket gets no fit; each socket that passed records `not_checked`, reason `another socket's interface rejected` (`INTERFACE_REJECTED_ELSEWHERE`), and each that failed `interface rejected` as today. With `NoFitVerifier` nothing changes: every passing socket records `no fit verifier in this build`, so every 0.3.0 document is unchanged (VT52).
- **FT12. The scope stated.** Every fit's evidence carries the closed list `scope_not_checked`: `motion`, `tolerance_stack_up`, `assembly_order`, `tool_access`, `unmodelled_items`, `threads`, `new_fasteners`. Soundness rule 5.

### 4.2 Zones (FZ)

A zone is a region where the socket declares that the target and a named body meet: there, a body may touch the candidate and overlap it by up to δ, and nowhere else.

- **FZ1. From the socket, in core.** `fitment_core::fit::zones` derives every zone from the socket document's requirements, for each (occurrence, body) pair, as an analytic description (FZ2 to FZ4) the helper builds (FZ7). `unresolved` requirements declare none. A requirement removed in review keeps its zones: a removal waives what a candidate must satisfy, not the contact the assembly makes. Zones never come from the helper's own reading.
- **FZ2. A seat's slab.** For each contact of each `seat` and each entry of its `by` list (neighbour path, neighbour face `#id`): the neighbour's face, a plane, moved by −d along its normal and swept 2d along it (a prism). Only that neighbour, only at the occurrences the contact's sources name.
- **FZ3. A span's annulus.** For each span of a `bore` or `shaft` whose `fit` is `interference`, `line_to_line` or (pinned) `not_read`, and each source that names it (`span_mm` set) at this occurrence: the annulus about the requirement's axis, placed by T_k, from station `from_mm − d` to `to_mm + d`, between the smaller radius less d and the larger plus d, the radii being the span's own (hole or shaft) and the neighbour's: own − margin in a bore, own + margin about a shaft (the source's `margin_mm`, 0 when null). Only the source's neighbour. P2 found both needs: the bolt in s01's nut overlaps by a 0.5 mm radial margin (the annulus from 2.5 − δ to 3.0 + δ), and s48's bushing holds a D-shaft its rules do not read (`not_read`, pinned: the wall's own radius ± d).
- **FZ4. A conical seat's shell.** For each `conical_seat` with a radial range: the cone surface over `radii_mm` widened by d each way, swept d either side of the surface along its normal, opening from the apex towards the axis's point; for each source's neighbour at this occurrence. Without it, P2 read every countersunk screw against its plate as a proximity (s18, s58, s60).
- **FZ5. Copies.** A zone whose neighbour is another selected occurrence (a source flagged `self`) applies to the pair of copies at those two occurrences; its face is the original's at that occurrence, which the candidate's face is within ε of by the interface.
- **FZ6. Depths.** d = δ for interference (FT4), 2δ for proximity (FT5). An overlap deeper than δ inside a slab is interference: a seat face 0.015 mm proud is not a contact.
- **FZ7. Built from primitives, and checked.** The helper builds a slab with `BRepPrimAPI_MakePrism` of the cited face and annuli and shells with `BRepPrimAPI_MakeRevol` of a quadrilateral; it computes each zone's volume and core compares it with the closed form (a prism's area × 2d, a revolved quadrilateral by Pappus) within 1e-9 relative; a zone that disagrees makes the fit `not_verified` (`zone_unbuilt`). P1: `BRepOffsetAPI_MakeOffsetShape` thickening a face reports done with an empty solid for a plane and a wrong one for a cylinder (189.75 mm³ for 1.88), so no kernel offset builds a zone.
- **FZ8. Nothing else is declared.** Clearance spans, grips, continuations (J rules), shaft ends past their spans (`max_extra_mm`, M4-11) and left-out holes declare no zone: a shaft that touches the wall of its clearance hole, or a boss that reaches a floor, is a proximity or an interference.

### 4.3 Binding: the helper's scene is fitment's (FB)

- **FB1. Core lists the bodies.** `FitScene::of(assembly_bytes, &SocketDocument)` reads the assembly with fitment's own reader (the same `read_assembly` the socket came from; its SHA-256 must be the socket's `source`, else `FitSceneError::NotTheSource`) and lists every leaf occurrence: its path, its definition, its solid's `#id` and its faces' `#id`s, its transform to the root, its stated uncertainties, and whether it is selected. Bodies are keyed by occurrence path. A refused or changed assembly is no scene: the run is refused before any candidate (FU2).
- **FB2. The helper finds each body by its faces and its placement.** It reads the assembly once through XCAF (`STEPCAFControl_Reader`, as the oracle does), binds every face of every leaf to its `#id` through the transfer reader (`EntityFromShapeResult`, the oracle's `face_id`), and identifies each leaf by the set of its faces' `#id`s and its composed location. Core sends each body's face `#id`s (from fitment's own topology of that solid) and transform; the helper must find exactly one leaf with those faces whose location agrees with fitment's transform within 1e-9 in rotation and 1e-6 mm in translation, else `binding_failed`. P5: on every corpus and fixture assembly probed (13 files, 265 leaves, `moon_buggy_asm` and the weldment included) every face binds and the (faces, transform) keys are all distinct; a solid's own `#id` finds nothing, since `ShapeResult` holds results for the roots only; and XCAF's transforms alone do not identify a leaf (P2: `moon_buggy_asm` has two leaves at one transform).
- **FB3. Landmarks.** For each body and the candidate, core names up to four faces in a fixed order (the two largest planes by known area, then the two largest full cylinders) with fitment's own values in the root frame: a plane's unit normal and offset, a cylinder's axis and radius. The helper reads each face by its `#id`, places it, and returns its values; core requires agreement within 1e-6 mm and 1e-9 in direction. A disagreement is `not_verified` (`binding_failed`). This catches a unit, a frame, a mapped item or a placement read one way by stepq and another by Open CASCADE.
- **FB4. The candidate.** A file candidate's bytes are the ones M2's gate accepted; the helper hashes what it received and reports the SHA-256, which core compares with the candidate's `source.sha256`. It must hold one solid. A component candidate is named by its assembly's SHA-256, its occurrence and its faces' `#id`s, and that assembly is loaded as a scene of its own when it is not the socket's.
- **FB5. One assembly, one scene.** A helper holds at most two scenes (the socket's and one component source), keyed by SHA-256; a scene request for a third replaces the older component scene.

### 4.4 The fit's status from measures (FV)

- **FV1. Decided in core.** `fitment_core::fit::decide(&FitPlan, &FitMeasures) -> FitOutcome` turns the helper's measures into the per-socket and per-candidate `FitStatus` and its evidence. It is pure, and every rule below is a branch of it. The helper returns numbers, alert keys and booleans, never a status.
- **FV2. Alerts.** Any warning or error a boolean or a distance reports (`BOPAlgo_Options::HasWarnings`, `HasErrors`, `BRepExtrema_DistShapeShape::IsDone` false), for any pair, a zone or the control: `not_verified`, code `kernel_alert`, with the alert's class name (`BOPAlgo_AlertFaceBuilderUnusedEdges`) in the evidence.
- **FV3. Interference.** A residual R ≥ `FIT_VOLUME_MM3` (1e-6 mm³): `interferes`, with the pair, R, V and R's box. 0 < R < 1e-6 mm³: `not_verified`, `below_resolution`. P2: every residual of every passing original and `fits` case is exactly 0; the smallest interference of a bad case is 0.0295 mm³ (`plate18_angle_15atol`).
- **FV4. Proximity.** A pair in proximity (FT5): `not_verified`, `proximity`, with the pair and the distance. Never `interferes`: a body within δ may only touch.
- **FV5. The control.** A control not `verified`: every candidate `not_verified`, `original_fails_fit`, the control's own code beside it.
- **FV6. Free placements.** An interference at a placement with a free turn or slide (FT10): `not_verified`, `interferes_at_free_placement`, the interference in the evidence.
- **FV7. Aggregation.** Per socket: `interferes` if any of its occurrences' pairs interferes (FV3); else `not_verified` if any rule of FV2 to FV6 or FV8 applies to it; else `verified`, with the kernel and version. Per candidate: M4's VD4, unchanged (`interferes` over `not_verified` over `verified`; the kernel and version must agree). A copy pair's interference counts for both occurrences' sockets.
- **FV8. Process failures.** A timeout (FL4): the helper killed, that candidate's fit `not_verified`, `helper_timeout`, and the run marked incomplete: the CLI exits 1 after writing the document, the server's job ends `failed`, its document `complete: false` (M6's rule for a run the CLI ends with exit 1). An exit or signal before the reply: the request is retried once in a fresh helper; a second failure is `not_verified`, `kernel_crashed`, and the run stays complete, since the same file crashes the same kernel. A malformed frame or a reply that breaks the protocol: `not_verified`, `helper_protocol`, the helper replaced, the run incomplete. None is ever `verified`.
- **FV9. Closed reasons.** Every `not_verified` and `not_checked` reason is a code from a closed list (`kernel_alert`, `below_resolution`, `proximity`, `original_fails_fit`, `interferes_at_free_placement`, `invalid_body`, `tolerance_too_large`, `binding_failed`, `zone_unbuilt`, `helper_timeout`, `kernel_crashed`, `helper_protocol`, `helper_unavailable`) with a fixed sentence built from values. No path, host, time or helper stderr enters a document; stderr's last 64 KiB go to the log only, escaped.

### 4.5 The helper's protocol (FH)

- **FH1. Frames.** Each message in either direction is one frame: the four bytes `FFIT`, a 4-byte big-endian header length, the header (UTF-8 JSON, one object), then the payloads the header announces (`"payloads": [n₁, n₂, …]`, byte counts), each raw. No line structure, no base64. Caps, checked before allocation: a header at most 16 MiB, a payload at most 1 GiB (the server's uploads stop at 128 MiB, UP2), at most 4 payloads; a reply's header at most 16 MiB with no payload.
- **FH2. A private channel.** fitment-fit spawns the helper with stdin and stdout piped and stderr piped. The helper's first statements, before any import of OCP, duplicate fd 1 to a new descriptor for frames, then make fd 1 a copy of fd 2, so anything the kernel's C++ writes to `std::cout` reaches stderr (P3: `**** ERR StepFile : Undefined Parsing …` on stdout for a truncated file). Frames are read from fd 0 and written to the private descriptor, both binary and unbuffered. fitment-fit drains stderr on its own thread into a ring of 64 KiB, for the log.
- **FH3. The handshake and the helper's identity.** The first frame is `hello` from fitment-fit (`protocol`, the expected script and lock SHA-256). The helper answers with: `protocol` (1); `helper_sha256`, the SHA-256 of its own file's bytes as it reads them; `lock_sha256`; `python` (`3.13.x`); `binding` (`cadquery-ocp-novtk 8.0.1.1.0`, from `importlib.metadata`); `kernel` (`Open CASCADE 8.0.1`, from OCP's `__version__` `8.0.1.1` and the library's own version string); `platform` (its `sysconfig` platform tag, for the log only); and `limits` (the rlimits it set, FY3). fitment-fit refuses a helper whose protocol, script or lock hash, binding or kernel is not the embedded constant: `helper_unavailable`, with what differed.
- **FH4. Operations.** Three after `hello`:
  - **`scene`** (payload: the assembly bytes): `scene` (its SHA-256, which the helper checks against the payload), `bodies` (each: key, faces' `#id`s, transform as 3 × 4 rows, landmarks). Reply per body: found or not, valid, volume, largest shape tolerance, box, each landmark's values (FB2, FB3, FT6). The helper keeps the scene's solids until a `scene` replaces it (FB5).
  - **`check`** (payload: the candidate's bytes, or none for a component): `scene`, `candidate` (its SHA-256, or a component's scene, faces and occurrence), its landmarks, `delta_mm`, `guard_mm`, `reach_mm`, `placements` (each: occurrence key, transform), `pairs` (each: occurrence, body key or copy occurrence, zones at δ, zones at 2δ). Reply: the candidate's validity, tolerance and landmarks; per placement its box; per pair its measures: `common_mm3`, `residual_mm3`, `residual_box`, `distance_mm` (null beyond reach), `zoned_distance_mm` (null when not asked), `alerts` (class names, sorted); per zone its built volume (FZ7). The pairs are those core lists after its own culling by the boxes the scene reply gave (FT8), so the helper never chooses what to test.
  - **`bye`**: the helper exits 0. Its end of stdin also ends it.
  The control (FT9) is a `check` whose candidate is the original component.
- **FH5. Strict decoding.** Both sides decode with `deny_unknown_fields`; every number must be finite (no NaN, no infinity, no −0.0 kept); every list is in the order the request gave; a reply to the wrong operation, with a missing pair, an extra pair, or a key not asked for is a protocol error (FV8). The helper refuses a request it cannot parse with an `error` frame (code, sentence) and stays up; fitment-fit treats an `error` frame as `helper_protocol`.
- **FH6. Versions.** `FIT_PROTOCOL` (1) rises with any change to a frame or an operation; the helper script's SHA-256 changes with any edit, and the embedded constant with it, so a stale script on disk is never run (FL6). `FIT_RULES_VERSION` (1, in core) rises with any change that could change a fit's status or evidence: a zone, a depth, a threshold, a reason, a pair chosen; it is in the v2 document and the fit baseline (FK6). The helper's pins (cadquery-ocp-novtk, Python) and the oracle's must be equal; CI's grep (`tools/verify-occt.py`'s and the helper's `dependencies` lines) fails otherwise, as decision 11 planned.

### 4.6 The helper's lifetime (FL)

- **FL1. Sessions.** An `OcctHelper` is the prepared, checked helper (FL2), built once (the CLI at the start of a run, the server at start). An `OcctSession` is built per run (the CLI) or per match job (the server) from a helper, the socket's assembly bytes and the socket document; it implements `FitVerifier`. It holds a pool of at most `threads` helper processes (the CLI's `--jobs`, the job's CPU permits), each taken by one judging thread for one candidate and given back. Which helper serves a candidate never changes a byte of its result: each request carries everything it needs, and the scene is the same bytes in every helper.
- **FL2. Preparing and spawning.** `OcctHelper::prepare` writes the embedded script and lock into the cache directory (FL6), then runs `uv run --locked --offline --script <script> --print-env` once (offline: `fitment fit setup` is the one step that may download, FU4): the script, in that mode, prints its interpreter's path (`sys.executable`) and exits without importing OCP. Every helper is then spawned as `<interpreter> -I <script>`: no uv between fitment and the kernel, so `Child::kill` ends the process holding Open CASCADE (P3: warm, 0.31 s to Python with OCP imported; `uv run` makes Python its own child). The first `fitment fit setup` on a machine downloads Python 3.13 and about 60 MB of wheels; P3 measured 6.3 s for a cold interpreter's first import, 0.31 s warm.
- **FL3. Reuse and replacement.** A helper serves requests until it fails (any of FV8's cases), until it has served 256 checks, or until the session ends; a failed or retired helper is killed and reaped, and the next request spawns a fresh one, which loads the scene again. A helper never serves two sessions.
- **FL4. Deadlines.** `hello` and `scene` have `fit.setup_timeout` (300 s); a `check` has `fit.timeout_seconds` (120 s, `--fit-timeout`). fitment-fit's reader thread delivers frames over a channel; the judging thread waits with the deadline, and in the server also wakes every 100 ms to read the job's cancel flag (FX4). On expiry or cancel: kill, reap, and FV8.
- **FL5. Ends.** At a session's end every helper gets `bye`, then 2 s, then a kill, and is reaped; a session dropped by a panic kills its helpers in `Drop`. A helper exits on its stdin's end of file, so fitment's own death ends it (the pipe closes); on Linux it also asks for `PR_SET_PDEATHSIG` through `ctypes`' prctl where available, a second guard, not one fitment relies on.
- **FL6. The cache directory.** `fitment-fit` writes `fit_helper.py` and `fit_helper.py.lock` to `<cache>/fitment/fit/<script sha256 prefix 16>/`, where `<cache>` is `--fit-dir`, else `XDG_CACHE_HOME`, `HOME/.cache` or `LOCALAPPDATA` (the server: `fit.dir`, required). The directory is created mode 0700 where the OS has modes, refused if it or a parent inside `<cache>/fitment` is a symbolic link; each file is written to a temporary name, synced and renamed, then read back and hashed before every spawn; a file whose hash is not the embedded one is rewritten once, and if it differs again the helper is `helper_unavailable`. uv's own cache stays where uv puts it (`UV_CACHE_DIR` passed through when set).

### 4.7 Security (FY)

- **FY1. Only gated bytes reach the kernel.** The assembly is the socket's own (its SHA-256 the socket's `source`, which the socket gate accepted; FB1), and a file candidate is one M2's gate accepted (its `status` accepted, its SHA-256 sent and checked by the helper). Coordinates beyond 1e6 mm, unread units, several solids and dangling references are refused by fitment before Open CASCADE could misread them (P3: `1.E400` coordinates transfer in Open CASCADE without complaint; a dangling reference is read past).
- **FY2. One process, little authority.** The helper runs with an environment fitment-fit builds from nothing (only `UV_CACHE_DIR` when the user set it, `SYSTEMROOT` on Windows), its working directory a fresh private temporary directory it writes the payloads to (0700, removed at exit), no inherited descriptor but its three pipes (Rust's `Command` sets close-on-exec), `-I` (no user site, no environment variables, the script's directory not on the path), and in a new session (`os.setsid`) so a terminal's signals do not reach it. It needs no network after `fitment fit setup`; prepare runs uv `--offline` by default (FL2).
- **FY3. Resource bounds.** Before importing OCP the helper sets on itself `RLIMIT_AS` (`fit.memory_mib`, 4,096), `RLIMIT_CPU` (256 times the check timeout: a backstop for a helper's whole life, the wall clock being the bound per check), `RLIMIT_CORE` 0 (no core file of a user's geometry), `RLIMIT_NOFILE` 64, `RLIMIT_FSIZE` (the largest payload plus 1 MiB) and reports what took effect (FH3). Linux honours them; macOS does not enforce `RLIMIT_AS`; Windows has none. Where a bound is missing, the deadline (FL4) and the server's container are the bounds, and `fitment fit setup` says which hold. P3: a helper is 240 MiB after import, 315 MiB after reading a 6 MB file, 478 MiB after a 17.8 MB file of 400,000 entities.
- **FY4. Strict replies.** FH5's decoding, FH1's caps; a reply's numbers outside their ranges (a negative volume, a residual above its common, a distance below zero) are a protocol error. A helper can make a fit `not_verified` or the run incomplete; no reply can make one `verified` but one in which every measure is clear, and every measure is cross-checked where core can: the landmarks against fitment's reading (FB3), the zones' volumes against their closed forms (FZ7), the candidate's SHA-256 (FB4).
- **FY5. The files on disk.** FL6's checks. The trust boundary is the user's account: a process of the same user can replace uv's environment or the cache, and then the handshake and the hashes catch a stale or edited script and a changed binding version, not a malicious one written to lie. The server runs as its own user, in its container, where nothing else writes.
- **FY6. In the server.** One session per job: a helper never holds two users' bytes (FX3). Helpers count against the job's CPU permits and its memory estimate (FX3), so the lanes bound them; a user's jobs are bounded by the per-owner caps and the job deadline; the fit's time is inside the deadline, never added to it. No response carries the helper's stderr, a path, or the platform tag; the log carries stderr escaped and capped (FV9).
- **FY7. What is not defended.** Native code execution through a crafted file that passes the gates is contained only by the process, its rlimits and, in the server, its container: fitment-fit forbids `unsafe` and has no seccomp or landlock policy. A user-built image (FX8) inherits the official image's non-root user and read-only root. Open question 9 asks whether a stronger sandbox (bubblewrap on Linux) is wanted later.

### 4.8 Changes to fitment-core (FC)

- **FC1. `FitRequest` v2.** Built only by core (it is `#[non_exhaustive]` already): `body` (`FitBody::Step { bytes, sha256 }`, `FitBody::Component { assembly_sha256, occurrence, faces }`, or `FitBody::Stored { sha256, fetch }` with a callback that returns the checked bytes, FC6), `socket_document` (a reference, from which the verifier takes the scene's source and zones), `placements` (per socket and occurrence: T_k · A as `Rigid`, and the `Freedom`, FC4), `plan` (a `FitPlan`: the pairs within core's reach and their zones, FZ, built by `fit::plan` from the scene's boxes), and `delta_mm`, `guard_mm`. `FitVerifier::check` returns a `FitOutcome` (per socket a `FitStatus` and evidence, per candidate the combined one); `FitVerifier::kernel` stays; `FitVerifier::identity` (new, default `None`) gives v2's `fit_verifier` object; `FitVerifier::control` (new, default `None`) gives the control's outcome. `NoFitVerifier` keeps its behaviour.
- **FC2. Once per candidate.** `Matcher::judge` judges every socket's interface first, then asks the verifier once if every socket passed and the verifier names a kernel (FT11), and splits the outcome into each socket's `fit`; with no kernel it keeps M4's per-socket call, so a `NoFitVerifier` run is unchanged to the byte (VT52).
- **FC3. `FitScene`, `fit::zones`, `fit::plan`, `fit::decide`.** In `fit/`: the scene from the assembly bytes (FB1), the zones from a socket (FZ1 to FZ8), the pairs from the scene and the placements (FT3, FT8), and the status from the measures (FV1 to FV9). All pure; no I/O.
- **FC4. Freedom.** `Placement` gains `freedom: Freedom { turn: bool, slide: bool }`, set by alignment: `turn` when AL8 tried one turn because the socket is symmetric about the line, `slide` when no seat or end fixed the station (AL7 tried breakpoints only). Serialized in v2 only.
- **FC5. Components.** `Matcher::judge_component` passes `FitBody::Component`; the original (`--original`, the control) is a component of the socket's own assembly.
- **FC6. Stored candidates.** `Matcher::judge_document_with_fetch(doc, source, fetch: &dyn Fn() -> Result<Vec<u8>, FetchError>)`: the closure is called only when the fit is asked (every socket's interface passed and a kernel is named); a fetch error is `not_verified`, `stored_bytes_unreadable`, and the run exits 1 (as a damaged row does). `fitment-catalog`'s `match_catalog` takes an optional `FitBlobs` (a read-only handle that reads one blob, hashes it against the row's SHA-256 and returns it, ST11) and passes a closure over it; `Snapshot` still has no field reaching `Blobs` (QR11). Dedup (QR6) with a kernel named: the group's interface result is shared and the fit asked per member, with the member's own bytes.
- **FC7. The document's writer.** `MatchDocument::new` writes v1 when the matcher's verifier names no kernel and v2 when it does (FM1); `to_json` and the digest follow. No `from_json` (M4-23).
- **FC8. Versions.** `MATCH_RULES_VERSION` stays 1: nothing a run without a verifier does changes. `FIT_RULES_VERSION` 1 and `FIT_PROTOCOL` 1 are new; `MATCH_FORMAT_VERSION` stays 1 and `MATCH_FORMAT_VERSION_FIT` is 2.

### 4.9 The document (FM)

- **FM1. Two versions, chosen by the verifier.** With no fit verifier (`fit_verifier: null`), `fitment.match` v1 exactly as 0.3.0 writes it. With one, `format_version: 2`. v2 is v1 with the changes of FM2 to FM5 and nothing removed; v1's schema stays closed and its documents keep their bytes and digests.
- **FM2. The envelope.** `fit_verifier` becomes an object: `kernel` (`"open_cascade"`), `kernel_version` (`"8.0.1"`), `binding` (`"cadquery-ocp-novtk 8.0.1.1.0"`), `python` (`"3.13"`), `protocol` (1), `helper_sha256`, `lock_sha256`, `fit_rules_version` (1). A new `fit` object: `delta_mm` (the policy's; the effective δ per pair is in the evidence), `guard_mm`, `volume_resolution_mm3`, `tolerance_cap` (`"delta/10"`), and `control`: per socket the original's fit, as a candidate's (FM3).
- **FM3. A fit's evidence.** Every `fit` (per candidate and per socket) keeps v1's `status` and, by status: `verified`: `kernel`, `version`, `occurrences`, `bodies` (checked), `pairs` (within reach), `copies`, `free` (each placement's freedom that is true: socket, occurrence, `turn` or `slide`); `interferes`: `interferences`, each `{occurrence, body: {kind: neighbour | copy, path, product}, measured: {common_mm3, residual_mm3, box_mm}}`; `not_verified`: `code` (FV9), `reason` (its sentence), `at` (the pairs or bodies concerned, as above), `measured`; `not_checked`: `reason`, as v1. Every fit carries `scope_not_checked` (FT12).
- **FM4. Placements.** Each socket's `placement` gains `freedom` (FC4).
- **FM5. The digest.** v2's digest is the SHA-256 of the compact document without `digest`, every `labels` and every `measured` object. Kernel numbers vary in their last digits between platforms where every decision agrees; the digest covers the decisions, the pairs and the codes, not the numbers. On one platform and helper the bytes are identical (P2, `p2_determinism.txt`).
- **FM6. Order and the summary.** MF5's order is unchanged (a `Match` first); the summary counts `match` for the first time. The table's header gains `fit: Open CASCADE 8.0.1 (helper 1a2b3c4d)` in place of `fit: not checked (no fit verifier in this build)`, and a candidate's line names its fit's status or code.
- **FM7. The schema.** The OpenAPI document gains v2's schema beside v1's (`https://fitment.dev/schemas/fitment.match/2` and `/1`), each closed (`additionalProperties: false`), generated from core's `schema` feature, and the served document stays the committed one (VN7's test); a match job's result names its document's version in `fitment.job` (an optional field within v1, M6-10).

An example, s01's bracket socket against the bracket and the lug, two candidates, cut where marked:

```json
{
  "format": "fitment.match", "format_version": 2,
  "extractor_version": 6, "socket_rules_version": 7, "signature_version": 2, "match_rules_version": 1,
  "policy": { "name": "default", "linear_mm": 0.01, "angular_deg": 0.01, "clearance": "strict", "grip": "hard" },
  "options": { "require_fit": false, "require_reviewed": false },
  "fit_verifier": { "kernel": "open_cascade", "kernel_version": "8.0.1", "binding": "cadquery-ocp-novtk 8.0.1.1.0",
                    "python": "3.13", "protocol": 1, "helper_sha256": "…", "lock_sha256": "…", "fit_rules_version": 1 },
  "fit": { "delta_mm": 0.01, "guard_mm": 1e-6, "volume_resolution_mm3": 1e-6, "tolerance_cap": "delta/10",
           "control": [ { "socket": "socket-1", "fit": { "status": "verified", "kernel": "open_cascade", "version": "8.0.1",
                                                          "occurrences": 1, "bodies": 9, "pairs": 5, "copies": 0, "free": [],
                                                          "scope_not_checked": ["motion", "…"] } } ] },
  "socket": { "…": "as v1" },
  "summary": { "candidates": 2, "match": 1, "interface_match": 0, "reject": 1, "refused": 0 },
  "candidates": [
    { "labels": ["m_bracket.stp"], "verdict": "match",
      "fit": { "status": "verified", "kernel": "open_cascade", "version": "8.0.1", "occurrences": 1, "bodies": 9,
               "pairs": 5, "copies": 0, "free": [],
               "scope_not_checked": ["motion", "tolerance_stack_up", "assembly_order", "tool_access",
                                     "unmodelled_items", "threads", "new_fasteners"] },
      "sockets": [ { "socket": "socket-1", "verdict": "match",
                     "placement": { "…": "as v1", "freedom": { "turn": false, "slide": false } },
                     "fit": { "status": "verified", "…": "as above" }, "…": "checks as v1" } ], "…": "as v1" },
    { "labels": ["m_bracket_lug.stp"], "verdict": "reject",
      "reasons": [ { "reason": "body_interferes", "socket": "socket-1", "requirement": null, "codes": [] } ],
      "fit": { "status": "interferes",
               "interferences": [ { "occurrence": [748], "body": { "kind": "neighbour", "path": […], "product": "PLATE" },
                                    "measured": { "common_mm3": 300.00000000000006, "residual_mm3": 299.4,
                                                  "box_mm": [[…], […]] } } ],
               "scope_not_checked": ["motion", "…"] },
      "…": "sockets, checks as v1" } ],
  "digest": "sha256:…"
}
```

### 4.10 Packaging (FG)

- **FG1. `fitment-fit`.** A library, published from 0.4.0 in lockstep with the others, MIT, `rust-version` 1.85, `unsafe_code = "forbid"`. Normal dependencies: `fitment-core`, `serde`, `serde_json`, `thiserror`, nothing else (SHA-256 is core's own). No Open CASCADE, no `cxx`, no binding of any kind (`deny.toml`'s bans stay); `tools/check-core.sh` checks the graph. Its package `include` is `src/**`, `helper/**`, `README.md` and `LICENSE`, since `include_bytes!` of a path outside the package would break `cargo package` (workspace layout, section 7).
- **FG2. What is embedded.** `helper/fit_helper.py` (the helper, a uv script whose inline metadata pins `cadquery-ocp-novtk==8.0.1.1.0` and `requires-python = "==3.13.*"`) and `helper/fit_helper.py.lock` (`uv lock --script`, with every wheel's hash), each with its SHA-256 as a constant generated by `build.rs`-free means: a test (`embedded_hashes_are_the_files`) recomputes them, so a stale constant fails CI. Its text is MIT, fitment's own; it imports OCP at run time and contains none.
- **FG3. The CLI's `fit` feature.** `fit = ["dep:fitment-fit"]`, in `default` beside `catalog` (M7-19). Without it, `--fit` and `fitment fit` exit 2 and say the build has no fit verifier, as `catalog` does.
- **FG4. The server's `fit` feature.** `fitment-server` gains `fit = ["dep:fitment-fit"]`, enabled in its release build and the image's; `fit.verifier = "occt-helper"` without the feature is a configuration error at start.
- **FG5. No Open CASCADE in official artifacts.** The crates hold the script's text; uv fetches Python and the wheel on the user's machine at `fitment fit setup`; `--fit` and the server never download (FL2). The official image stays as M6 shipped it (no Python, uv or Open CASCADE; the `container` job checks it). Where uv has no wheel (musl, Windows on ARM, macOS before 11), `prepare` fails and says so: `--fit` exits 1, the server refuses to start with `occt-helper`.
- **FG6. The release.** 0.4.0 at M7's exit, from the release PR's commit: `fitment-core`, `fitment-catalog`, `fitment-fit`, then `fitment`; the image `ghcr.io/jchultarsky/fitment-server:0.4.0` by `image.yml` as for 0.3.0. cargo-semver-checks joins for `fitment-fit` after its first publish.

### 4.11 The CLI (FU)

```text
fitment match <SOCKET> (<CANDIDATE>... | --catalog DIR …) [--original ASM]
              [--fit <ASSEMBLY>] [--fit-timeout <SECONDS>] [--fit-dir <DIR>] [--uv <PATH>]
              …M4's and M5's options
fitment fit setup [--fit-dir <DIR>] [--uv <PATH>]
```

- **FU1. `--fit ASSEMBLY`.** Turns the fit verifier on, with the socket's own assembly (its SHA-256 must be the socket document's `source`, else exit 2 before any candidate, as `--original` is checked). `--require-fit` keeps its meaning: an `InterfaceMatch` is a failure (exit 5).
- **FU2. Before any candidate.** With `--fit`: prepare the helper and check it (FL2, FH3), build the scene (FB1), load it in one helper, run the control (FT9). A helper that cannot be prepared or checked, or a scene that cannot be built: exit 1, with the reason (uv not found, no wheel for this platform, a hash or version that differs, the assembly refused), nothing on stdout. A control that fails is not an error: the run goes on, every fit `not_verified`, and stderr says once that the original fails its own fit check and why.
- **FU3. During the run.** Each candidate whose interface passes is fit-checked by one of the session's helpers (`--jobs` many at most); stderr's summary line names the fit: `fit: Open CASCADE 8.0.1` and the counts by status. A timeout makes the run exit 1 after the document is written (FV8).
- **FU4. `fitment fit setup`.** Prepares the helper online (uv may download), runs `hello` and a built-in self-test (two unit boxes touching, overlapping by 1e-3 mm and apart by 0.02 mm, each with its expected Common and distance), prints the identity, the rlimits that took effect and the cache paths, and exits 0, or 1 with the reason. CI and the image recipe run it.
- **FU5. Exit codes.** VD14's, with `Match` now reachable: 0 at least one `Match`; 4 an `InterfaceMatch` at best; 5 every candidate rejected; 1 an error (now also: a helper that cannot start, a fit timeout, a helper protocol failure, a stored blob unreadable); 2 usage (also `--fit` without the feature, or naming another assembly); 3 refused input.
- **FU6. A catalog run.** `fitment match S --catalog DIR --fit ASM`: the fit reads each passing candidate's blob through the catalog (FC6); stderr counts the blobs read.

### 4.12 The server (FX)

- **FX1. Configuration.** `[fit] verifier = "none" | "occt-helper"`, and for `occt-helper`: `dir` (the cache directory, required, on a writable volume), `uv` (default `uv` on `PATH`), `online` (false), `timeout_seconds` (120), `setup_timeout_seconds` (300), `memory_mib` (4,096). Unknown keys are refused (`deny_unknown_fields`, as every section).
- **FX2. The start.** With `occt-helper`, `fitment-server serve` prepares and checks the helper and runs `fitment fit setup`'s self-test before it binds its port; a failure refuses the start with the reason, never falling back to `"none"` (OP9, M6's F28). `fitment.info`'s `fit_verifier` names the kernel and version (DC11) and gains `fit`, the v2 envelope's `fit_verifier` object, an optional field within v1 (M6-10).
- **FX3. Sessions in jobs.** A match job with a verifier opens an `OcctSession` in its blocking thread (JS6) with the socket line's assembly bytes from the object store (OB3: read once, checked against its name), and holds at most its CPU permits' worth of helpers. JS3's memory estimate gains `fit.memory_mib` per helper. Helpers are spawned lazily, at the first candidate whose interface passes, and all killed when the job ends, however it ends. Each helper costs fitment-fit two threads that only block on its pipes (frames, stderr), alive as long as the helper; they do no compute, so they sit outside the CPU budget, and SV7's ban on `std::thread::spawn` stays true of the server's own code, as it is for the catalog's scoped threads. `fitment_fit_helpers` (a gauge), `fitment_fit_checks_total{status}` and `fitment_fit_seconds` join the metrics (OP6).
- **FX4. Stopping.** JS8 gains a stop point: inside a fit, the waiting thread reads the job's flag every 100 ms and kills its helper; the candidate is not judged and the job ends as its stop says, with no document. So a fit never needs JS8's stuck-call watchdog; the watchdog stays for a core call.
- **FX5. Bytes for components and the catalog.** A component candidate of another stored assembly: its bytes from the object store by SHA-256 (the job holds every assembly it names, OB4). A catalog run: the catalog's `FitBlobs` handle (FC6), opened read-only in the job's snapshot; a blob missing or damaged is `not_verified`, `stored_bytes_unreadable`, and the job ends `failed` with its document (`complete: false`), as the CLI exits 1.
- **FX6. Parity.** Under the same helper (the same script, lock and binding), every document a fit match job serves is the CLI's `--fit` bytes for the same inputs, and its exit code the CLI's; `fit_verifier` and `fit` included. A server built without `fit`, or configured `none`, serves v1 as in M6.
- **FX7. Faults.** XH12's fault tests gain the fit's: a helper crashing, hanging, answering garbage or a wrong protocol, killed mid-check by a cancel, a deadline or a drain, and the server killed during a fit, then restarted (SD4: a pure match job runs again once).
- **FX8. The image recipe.** `deploy/Containerfile.fit` builds `fitment-server-fit` (a local tag): a builder stage on `debian:bookworm-slim` installs uv (pinned version and checksum), writes the helper with `fitment fit setup --fit-dir /var/lib/fitment/fit` from the binaries of the official image (`COPY --from=ghcr.io/jchultarsky/fitment-server:VERSION`), so Python and the locked environment sit in the image's uv cache; the final stage is `debian:bookworm-slim` with those, the two binaries, the non-root user, `fit.verifier = "occt-helper"`, `fit.online = false`, and the official image's volumes and health check. Its header says what it contains (Open CASCADE under LGPL-2.1 with its exception, and the wheel's bundled libraries) and that whoever distributes the image owes their notices. CI's `container-fit` job builds it on every PR and runs `deploy/fit-smoke.sh` (the M6 smoke run, then s01's lug and bracket matched with `Reject` and `Match`), and pushes nothing; `image.yml` never builds it.
- **FX9. Exposure.** Only the match job's kinds spawn a helper; no endpoint reaches the helper, its directory or its stderr. A user's fit time is inside the job's deadline (JB7) and lane (JS1).

### 4.13 The corpus (FK)

- **FK1. The exit's truths.** A `collides` case's expectation with a verifier is `Reject` with `interferes` (the scorer's KH2.1 already says so), and `InterfaceMatch` with `not_verified` is allowed only with the code `interferes_at_free_placement` or `kernel_alert`; `Match` on any case whose truth is not `fits`, `fn_rule` or `fits_functional` (under functional) is a false positive. A `fits` case may end `Match` or `InterfaceMatch` with a stated code; each `InterfaceMatch` of a `fits` case is listed in the report by code, not counted as a false negative.
- **FK2. New cases** (generated, each a candidate of an existing source unless said):
  - `f_touch`: s01's bracket with a tab whose face touches the plate's side, gap 0 (proximity: `not_verified`);
  - `f_overlap_1um`: the same tab 0.001 mm into the plate (`interferes`, 1e-3 × the face's area);
  - `f_overlap_sub`: 1e-7 mm into it, below Open CASCADE's resolution (P1 reads 0): `not_verified`, `proximity`;
  - `f_contained`: a bolt candidate with a pin wholly inside the nut's material (containment: `interferes`);
  - `f_major_minor`: s01's nut with its hole at the minor diameter and the bolt at the major (the original's case: `verified`, the annulus zone);
  - `f_copy`: s05's washer with a lip that reaches into the other washer's copy only (V1: `interferes`, `copy`);
  - `f_free_turn`: on a one-axis socket AL8 finds symmetric (the generator states which; s05's washer first), a candidate with a tab that hits a neighbour at every turn (`not_verified`, `interferes_at_free_placement`), and its control, a tab clear at every turn (`verified`);
  - `f_head_touch`: s01's bracket with a boss on its top face whose side touches a bolt head's side, a contact no requirement declares (`not_verified`, `proximity`);
  - `f_warning`: a candidate whose boolean with its neighbour raises `BOPAlgo_AlertFaceBuilderUnusedEdges` (the shape P2 met in `bracket_shift_05`, moved to pass its interface), expecting `not_verified`, `kernel_alert`; kept only if the generator can build it deterministically, else listed as not built.
  Each states its truth and its expected fit status and code; each is in four frames where its fit depends on a 1e-3 mm value (KC7's rule).
- **FK3. `collides` at every free placement.** The generator states for each `collides` case whether its interface leaves a turn or slide free; the oracle checks the collision at the construction's placement and at 8 turns (FO2).
- **FK4. `interferes_by_construction`.** A source assembly whose own parts interfere is marked so in the manifest: s14 (`a box overlapping the block`), s20 (`blocks 0.05 mm above and 0.05 mm into a plate`), and among the fixtures the weldment's BEARING_208 and LINKAGE_ARM_SHORT_46 (P2: 3.58 and 3.55 mm³ outside every zone). Their controls must fail with `interferes`; every other control whose interface passes must be `verified`.
- **FK5. The originals.** VR21's runs gain the fit: every corpus and fixture original read in place is fit-checked by `--original` (the control itself), and FK4 states which may fail.
- **FK6. The fit baseline.** `crates/fitment/tests/corpus/fit-baseline.json`, committed: per (case, policy) run with the verifier, its fit status and code, with `FIT_RULES_VERSION`, the helper's SHA-256 and the binding. Any difference fails; `FITMENT_BLESS=1` rewrites it, refused under `CI`, and refused for a known case whose status changes without a `FIT_RULES_VERSION` rise or a helper change. The false-negative baseline (KH4) is unchanged: a fit never turns a pass into a `Reject` but by `interferes`, which on a `fits` case is a gate failure (the label or the fit is wrong).
- **FK7. Unit set.** The committed candidates (KC15) gain two with known fits: the lug (`interferes`) already there, and `f_copy` if it fits the 210 KB cap, else none: the fit's own tests on three OSes run the fake helper (VT55), not the kernel.
- **FK8. Registry.** M7's rules join the `RULES` registry in `tools/gen-corpus.py` and `m7-fit.md` joins the coverage test's `DESIGNS`; `M7_PR` deferrals are allowed while M7 is open and removed at its exit (PR 6), as M4 to M6 did. Two lists: `M7_FIT_CAN_END` (FV2 to FV6, FT4, FT5, FT6, FT9, FB3, FZ7), each needing a case or a fake-helper test that ends a fit by it; and `M7_PROCESS` (FV8, FL3 to FL6, FH1 to FH5, FY2 to FY4), each needing a test in `crates/fitment-fit/tests/adversarial.rs` or `process.rs`.

### 4.14 The oracle (FO)

- **FO1. Every `Match`, physically.** KO3 already checks every `InterfaceMatch` at fitment's placement; it now checks every `Match` the same way, with the fit's own measure: no common volume with any neighbour outside the regions of M3's expected sockets, by the oracle's own physics (`physics`, `regions`), which shares the kernel but not the code.
- **FO2. Every `interferes`, re-measured.** For each interference in a document, the oracle places the candidate and the named body as the document says and measures the Common less its own regions; it must exceed 1e-6 mm³ and agree with `residual_mm3` within 1e-6 relative. For `collides` cases, the collision is checked at the construction's placement and at 8 turns where FK3 says a turn is free.
- **FO3. Mutations.** KO6 gains three on v2 documents: an `interferes` turned `verified` (FO2 meets it), a `verified` on a `collides` case (FO1 meets it), and a `free` freedom dropped (the scorer's checker meets it, since the placement's seed is one-axis on a symmetric socket).

## 5. Measurements

All on cadquery-ocp-novtk 8.0.1.1.0 (OCP 8.0.1.1, Open CASCADE 8.0.1), Python 3.13, macOS on an Apple M5 Max, the corpus regenerated at `51d671d` and `target/release/fitment` built from it.

### 5.1 Open CASCADE at the edge (P1)

| Two 10 mm boxes | Common, identity frame | Common, skew frame | Distance |
| --- | --- | --- | --- |
| overlap 1e-3, 1e-4, 1e-5, 1e-6 mm | exact (0.1 to 1e-4 mm³) | exact | 0 |
| overlap 1e-7, 1e-8 mm | **0** (absorbed) | **0** | 0 or 8.9e-16 |
| touching | 0 | 0 | 0 |
| gap 1e-7 mm | 0 | 0 | 0 or 8.9e-16 |
| gap 1e-5, 0.005, 0.02 mm | 0 | 0 | exact |

- No boolean raised a warning or an error on these. Containment (a 2 mm box inside a 10 mm one): Common 8 mm³; distance 0 with `InnerSolution()` true, so BRepExtrema in 8.0.1 does see a contained body, though FT4 relies on Common for it.
- A shaft in a hole: clearance 0.3 and 0.005 mm give Common 0 and the distance exactly; line to line gives 0 and 0; an interference of 0.5 mm radial gives the annulus' volume exactly (86.3938 mm³), and one of 1e-6 mm gives 1.885e-4 mm³, as computed.
- One Common's volume 20 times, serial and parallel (`SetRunParallel`), on two primitives and on two corpus brackets: one value each, bit for bit; the `Eps` overload of `VolumeProperties` gives another value, also stable. FT's measures use the default.
- `BRepOffsetAPI_MakeOffsetShape` thickening a face by 0.01 mm: `IsDone` true and volume 0 for each of a box's six planes; 189.75 mm³ for a cylinder's side whose skin is 1.88 mm³. The whole corpus bracket offset by 0.01 mm works (valid, 6 ms). FZ7 builds zones from prisms and revolutions only, and checks each.
- OCP 8.0.1 binds `HasWarnings` and `HasErrors` on `BOPAlgo_BOP` and `BOPAlgo_Options`, not on `BRepAlgoAPI_Common` (whose attributes are listed in P1's notes), so the helper uses `BOPAlgo_BOP` (M7-17); `p1_kernel.txt` ends with both classes' bound methods.

### 5.2 The prototype on the corpus (P2, `p2_fit.txt`)

At fitment's own placements, default policy, δ = 0.01 mm:

| Set | Verified | Interferes | Not verified |
| --- | --- | --- | --- |
| `fits`, interface passed | **28** | 0 | 0 |
| `collides`, interface passed | 0 | **3** (lug 299.4 mm³; cover19 boss 56.5 mm³; nut bridge 4 × 5.97 mm³) | 0 |
| `does_not_fit`, interface rejected (for information) | 12 | 27 | 2 (`BOPAlgo_AlertFaceBuilderUnusedEdges`) |
| `edge`, interface rejected | 10 | 0 | 7 (proximity at 0.0099999999 mm: the four `bracket_dia_clear` and three `tube50_tau_edge` frames) |
| `fn_rule`, interface rejected | 6 | 2 (`bracket_thick` with the plate, `washer05_thick` with its copy: D7's cases) | 1 (`plate01_thin`, proximity) |
| Originals, interface passed | **56** | 1 (s20: 16 mm³ into INTO, by construction) | 0 |
| Originals, interface rejected | 11 | 2 (s11's bolts into unread nuts; s14 by construction) | 4 |

- Six cases ran on reviewed or edited sockets and were skipped by the prototype (`m_review_*`, s61, s62); the design runs them (FK5).
- Cost: 624 Commons (2.4 s), 592 cuts (2.0 s), 1,011 distances (3.8 s), the whole run 14 s including fitment's own; one candidate's fit 0.00 to 0.27 s; reading a corpus assembly in a fresh helper about 0.01 s.
- Three rules came from this run: FZ3's margin (the bolt's original first read as interfering 42.3 mm³ with its nut, since a shaft's span states no neighbour radius); FZ3's `not_read` wall (s48's bushing read a proximity with its D-shaft); FZ4's cone shells (every countersunk screw read a proximity).
- Run twice: identical (`p2_determinism.txt`).

### 5.3 The fixtures (`p2_fixtures.txt`)

| Fixture | Control | Interface |
| --- | --- | --- |
| AS1 ac, md, ug (plate, bracket, nut, rod) | verified, 12 of 12 | passes |
| AS1 ac, md, ug bolts; as1_pe's five | verified | rejects (an unresolved requirement; pe's floor) |
| AS1 ec, tc (B-spline cylinders) | proximity, every target | rejects (`unverifiable`) |
| weldment SPACER_220, NUT_244 | verified | rejects |
| weldment BEARING_208, LINKAGE_ARM_SHORT_46 | interferes, 3.58 and 3.55 mm³ (with each other) | rejects |
| vaccase TEE | tolerance 0.0019 mm > δ/10 | rejects |
| moon_buggy | not bound by transform (two leaves at one transform) | (P5 binds it by faces) |

The slowest control was the weldment's bearing: 72 pairs in 2.84 s.

### 5.4 The process (P3, P4, P5)

- `uv run --locked --script` of the oracle, warm: 0.35 s to exit; the environment's own interpreter with `-I`, importing OCP: 0.31 s warm, 6.3 s on its first run after the environment was made (a cold file cache).
- A worker reading an assembly: 240 MiB after import; 315 MiB after a 6 MB part (0.11 s); 478 MiB after a 17.8 MB file of 400,000 points.
- Hostile files read by Open CASCADE (each in its own process, 120 s limit): a truncated file and random bytes fail to read and print `**** ERR StepFile …` on stdout; a self-reference and a 2,000,000-number list read and transfer nothing (0.07 s); a dangling reference and `1.E400` coordinates read and transfer as if nothing were wrong. No crash, no hang on these seven; stepq's oracle met a segfault in cadquery-ocp 8.0.1 before (D1, source 26), which FV8 covers.
- Shape tolerances (P4): 1e-7 mm (ac, ec, tc, pe) or 1e-5 mm (md, ug, NIST EDM parts) for every solid but vaccase's two (up to 0.0029 mm) and the weldment's one (0.058 mm).
- Binding (P5): a solid's own `#id` finds nothing after `TransferRoots`; faces bound by `#id` plus the composed location identify 265 of 265 leaves of 13 assemblies uniquely.

### 5.5 What was not measured

- **Other operating systems.** Every number is macOS's. Whether Linux and Windows give the same decisions on the corpus is unknown until the `fit` job runs (M7-25, VT63); the kernel's measures may differ in their last digits, which FM5 keeps out of the digest.
- **The design's exact zones and binding** differ from the prototype's in two ways: the prototype finds bodies by product and transform (the design by faces, FB2), and it builds zones for every source of a span without checking each zone's volume (FZ7). PR 2's helper and PR 4's corpus run measure the design itself.
- **Scale.** No catalog run with fit at 10⁴ parts; a fit costs 0.01 to 3 s per passing candidate, so a full catalog run's cost is its interface passes times that (R8).
- **Windows' process behaviour** (fd duplication, kill) and uv's environments there.

## 6. Tests

Numbered from VT51. Where a test needs uv and the kernel it says so; every other runs on three OSes without Python.

| Test | Where | What it shows |
| --- | --- | --- |
| **VT51** | `fitment-core/tests/verdict.rs` | Only `verified` makes a `Match`, `interferes` a `Reject`, the other two an `InterfaceMatch`, per socket and per candidate (D1's truth table, kept) |
| **VT52** | `fitment/tests/golden.rs`, every golden | With `NoFitVerifier` every document of the unit set, the corpus and the fixtures is byte-identical to 0.3.0's, and `MATCH_RULES_VERSION` is 1 (FT11, FC2, FM1) |
| **VT53** | `fitment-core/src/fit/decide.rs` unit tests | Every FV rule from constructed measures: each alert, a residual of 0, 1e-7 and 1e-3 mm³, a distance at δ − 1e-9, δ, δ + guard and δ + 1e-5, a failing control, a free placement, a binding off by 2e-6 mm, a zone's volume off by 1e-8 relative, a timeout, a crash then a success, two crashes (VT53 is FK8's list `M7_FIT_CAN_END` for the fake path) |
| **VT54** | `fitment-core/tests/zones.rs` | The zones of s01, s02, s03, s18 and s05 from their committed sockets equal the construction's (each slab's face and depth, each annulus' radii and stations, each cone's apex, angle and range), and removing a requirement in review keeps its zones (FZ1 to FZ8) |
| **VT55** | `fitment-fit/tests/adversarial.rs` | The test binary re-executed as a fake helper that: crashes before and after `hello`; hangs; drips one byte a second; sends garbage, a short frame, an oversized header, an announced payload it never sends, a reply with a NaN, an extra key, a missing pair, a pair not asked, a negative volume, a residual above its common; states another protocol, script hash, lock hash, binding or kernel; writes on fd 1 before its frames; exits 0 mid-reply. Each ends `not_verified` with the right code or a refused helper, never `verified`, on three OSes (FH1 to FH5, FV8, FY4) |
| **VT56** | `fitment-fit/tests/process.rs` | A fake helper that ignores its stdin is killed at its deadline and reaped, and no process of it remains (Linux and macOS: its pid gone; Windows: `Child::try_wait`); a fake that spawns a child of its own shows why FL2 spawns the interpreter directly (FL4, FL5) |
| **VT57** | `fitment-fit/tests/prepare.rs` | The cache: a tampered script rewritten once, then refused; a symbolic link refused; the embedded hashes are the files' (`embedded_hashes_are_the_files`) (FL6, FG2) |
| **VT58** | `fitment/tests/fit_corpus.rs` (uv) | Every match case under both policies with `--fit`, through the binary and the library: the strict gate (KH2) with FK1's expectations, no false positive, every `collides` case `Reject` (`interferes`) or `InterfaceMatch` with FK1's codes, the fit baseline (FK6) |
| **VT59** | `fitment/tests/fit_original.rs` (uv) | Every corpus and fixture original in place: its control `verified` wherever its interface passes and FK4 does not name it, and `interferes` where FK4 does (FT9, FK4, FK5) |
| **VT60** | VT58's cases | FK2's cases each end as stated: touch, 1 µm, sub-resolution, contained, major in minor, copy, free turn and its control, head touch, warning if built |
| **VT61** | `fitment-core/tests/matcher.rs` | The fit is asked once per candidate and only when every socket passed; a socket that passed beside one that failed records `another socket's interface rejected`; every occurrence of every socket is in the request (FT11, FC2) |
| **VT62** | `fitment-core/tests/align.rs` | `freedom` is set exactly where AL8 tried one turn and AL7 found no fixed station, on s01's nut, s05's washer and s21's rod (FC4, FT10) |
| **VT63** | VT58 run twice, and CI | Two runs give the same bytes; three OSes give the same digests on every v2 document of the corpus; a difference fails and lists the decisions that differ (FM5, M7-25) |
| **VT64** | `fitment/tests/fit_corpus.rs` (uv) | A catalog run with `--fit` equals the file run with `--fit` over level 1's survivors, digest for digest (M5's CT1 with fit), and dedup asks per member (FC6) |
| **VT65** | `fitment/tests/catalog_*.rs` | CT6 kept: with `NoFitVerifier` no blob is opened; with a fake verifier that names a kernel, blobs opened equal candidates whose every interface passed, each hash-checked; a damaged blob gives `stored_bytes_unreadable` and exit 1 (FC6, QR13) |
| **VT66** | `fitment-server/tests/parity.rs` (uv) | A fit match job's document and exit code are the CLI's `--fit` ones for files, components and a catalog, on the corpus's sockets (FX6) |
| **VT67** | `fitment-server/tests/fault.rs` | XH12 with fit: a fake helper crashing, hanging, garbling; a cancel, a deadline and a drain mid-check kill the helper (no process left), the job ends as its stop says with no document; a restart re-runs the job once (FX4, FX7) |
| **VT68** | `fitment-server/tests/process.rs` | `occt-helper` with no uv, a tampered script, or the official image refuses to start, never serving `none` (FX2, M6's F28) |
| **VT69** | CI's `container-fit` job | The recipe's image matches s01's lug as `Reject` and bracket as `Match` through the API; the official image still has no Python, uv or Open CASCADE (FX8, FG5) |
| **VT70** | `fitment-fit/tests/process.rs` (Linux) | A fake helper that allocates past `RLIMIT_AS` dies, and the fit is `kernel_crashed` after its retry; `fitment fit setup` reports which limits hold on each OS (FY3) |
| **VT71** | `fitment-fit/tests/kernel.rs` (uv) | The real helper on P3's hostile files, when handed them directly (bypassing the gate): no hang past its deadline, every reply a frame or a failure, stdout noise never in a frame (FH2, FY1) |
| **VT72** | `tools/verify-occt.py match` | FO1 to FO3 on every v2 document of the corpus |
| **VT73** | `fitment/tests/corpus.rs` | Coverage: every FT, FZ, FB, FV, FH, FL, FY, FC, FM, FG, FU, FX, FK and FO rule has a case, a test or a reason; FK8's two lists (FK8) |

## 7. CI

- **`fit` job,** a matrix of `ubuntu-latest`, `macos-latest` and `windows-latest`, after the `corpus` job: `astral-sh/setup-uv` (pinned as the oracle job pins it), `cargo build --release`, `fitment fit setup`, then `FITMENT_REQUIRE_FIT=1 cargo test --release -p fitment --test fit_corpus --test fit_original` and `-p fitment-fit --test kernel`, uploading each OS's fit report and the digests of every v2 document. A last step on Ubuntu downloads the three and compares the digests (VT63). About 4 minutes per OS: P2's whole corpus took 14 s here.
- **`test` job** (three OSes, no Python): VT51 to VT57, VT61, VT62, VT65, VT67, VT68 with fake helpers.
- **`oracle` job:** `verify-occt.py match` on the corpus with the documents the `fit` job wrote on Ubuntu (FO1 to FO3).
- **`container-fit` job** (Ubuntu): builds `deploy/Containerfile.fit` from the PR's official image and runs `deploy/fit-smoke.sh`; never pushes. The `container` job keeps checking that the official image holds no Python, uv or Open CASCADE.
- **The pin grep:** the `dependencies` lines of `tools/verify-occt.py` and `crates/fitment-fit/helper/fit_helper.py` must name the same `cadquery-ocp-novtk` (decision 11).
- **Windows,** if its kernel or process handling disagrees and cannot be fixed in M7: the `fit` matrix drops it, `fitment fit setup` says the fit is untested on Windows, and the README says so (open question 7).

## 8. Limits and failure modes

### 8.1 Limits

| Limit | Value | Where |
| --- | --- | --- |
| δ | max(policy `linear_mm`, the pair's stated uncertainties) | FT7 |
| Guard | 1e-6 mm | FT7 |
| Volume resolution | 1e-6 mm³ | FV3 |
| Shape tolerance cap | δ/10 | FT6 |
| Reach | boxes enlarged by 2δ + tolerance + 1e-6 mm | FT8 |
| Landmarks | 4 per body; 1e-6 mm, 1e-9 | FB3 |
| Frame | header 16 MiB, a payload 1 GiB, 4 payloads | FH1 |
| Check deadline | 120 s (`--fit-timeout`, `fit.timeout_seconds`) | FL4 |
| Setup deadline | 300 s | FL4 |
| Requests per helper | 256 | FL3 |
| Helpers per session | the judging threads | FL1 |
| Helper memory | 4,096 MiB (`RLIMIT_AS`, Linux) | FY3 |
| Retries after a crash | 1 | FV8 |
| Scenes per helper | 2 | FB5 |
| stderr kept | 64 KiB, the log only | FH2, FV9 |

### 8.2 Failure modes

| # | Failure | Detected by | Result | Test |
| --- | --- | --- | --- | --- |
| F1 | uv not installed | `prepare` | `--fit` exit 1; server refuses to start | VT57, VT68 |
| F2 | No wheel for the platform (musl, Windows ARM) | `prepare` (uv's error) | as F1, the platform named | VT57 |
| F3 | Offline on the first run | `prepare` with `--offline` | as F1: run `fitment fit setup` online | VT57 |
| F4 | Script or lock on disk edited | hash before spawn; handshake | rewritten once, then refused | VT57 |
| F5 | Environment upgraded under fitment (another binding) | handshake (`binding`, `kernel`) | refused | VT55 |
| F6 | Kernel prints on stdout | fd moved (FH2) | nothing in frames | VT55, VT71 |
| F7 | Helper hangs | deadline | killed; `helper_timeout`; run exit 1 | VT55, VT56 |
| F8 | Helper crashes (segfault, `RLIMIT_AS`) | EOF, exit status | retry once; `kernel_crashed` | VT55, VT70 |
| F9 | Garbage, oversized or partial frame | FH1, FH5 | `helper_protocol`; run exit 1 | VT55 |
| F10 | A boolean warns | `BOPAlgo_BOP` alerts | `kernel_alert` | VT53, VT60 |
| F11 | A zone builds wrong | FZ7's closed form | `zone_unbuilt` | VT53 |
| F12 | Readers disagree on a frame or unit | FB2, FB3 | `binding_failed` | VT53 |
| F13 | The original's assembly already interferes | the control | every candidate `not_verified` | VT59 |
| F14 | The assembly given is not the socket's | SHA-256 | exit 2 | VT58 |
| F15 | A catalog blob missing or damaged | ST11's hash | `stored_bytes_unreadable`; exit 1 | VT65 |
| F16 | A job cancelled mid-fit | the flag every 100 ms | helper killed; no document | VT67 |
| F17 | The server killed mid-fit | restart (SD4) | the job runs again once; helpers died with the server (stdin closed) | VT67 |
| F18 | A crafted file executes code in the kernel | (not detected) | contained by the process, its limits and the container | FY7 |
| F19 | macOS or Windows memory blow-up | deadline only | killed at the deadline, or the machine swaps first | FY3 |
| F20 | Two OSes decide differently at an edge | VT63 | CI fails; the edge's case is examined | VT63 |

## 9. Risks

- **R1. The kernel's edge.** Open CASCADE absorbs overlaps of 1e-7 mm and less (P1), more for shapes of larger tolerance. FT5's proximity at δ and FT6's cap of δ/10 together keep any absorbed overlap inside a proximity, which is `not_verified`. A shape whose tolerance a file understates is outside this argument; FB3's landmarks catch gross disagreement only.
- **R2. Zones too generous.** A zone excuses up to δ of overlap with one named body over one face or one span. A defect that lives entirely inside a zone and is shallower than δ is, by the policy, within the tolerance; a deeper one is interference (FZ6). The oracle's own regions (FO1) are wider (M3's NEAR of 1 mm), so FO1 is a check of the fit, not of its zones; VT54 checks the zones against the construction.
- **R3. Free placements.** A `Match` at a free turn tells the user how to place the part (the evidence's placement and `free`); a user who installs it at another turn may meet an interference fitment did not test. The README says so; open question 2 asks whether to accept it.
- **R4. False negatives from the fit.** Every `not_verified` costs a `Match`. The corpus measures: 0 of 28 `fits` cases; the fixtures with a passing interface, 0 of 12. Real assemblies with modelled thread overlaps no rule reads, cosmetic interferences, or B-spline contacts will cost more; the report lists every `not_verified` by code.
- **R5. Platforms.** Windows and Linux kernels may differ at an edge (F20); Windows' fd handling and process kill are untested here.
- **R6. uv's behaviour.** uv's environment paths, lock format and `--print-env` mode are uv's; FL2 depends on `sys.executable` in a script environment staying stable (P3: stable across runs) and on `--locked` refusing a stale lock (decision 11 checked it with uv 0.11.31). A uv upgrade that changes either fails `prepare` loudly.
- **R7. Licences.** Unchanged from D1: fitment distributes no Open CASCADE; a user-built image does, and its builder owes the notices (FX8's header says so); issue #1564 is still open.
- **R8. Cost at catalog scale.** A socket that many catalog parts pass at the interface costs a fit each: at 1 s a fit, a run with 1,000 interface passes takes about 17 minutes on one helper, less on more. The job deadline bounds it; open question 8 asks whether a catalog run should cap its fits.

## 10. Attacks and their fixes

How could M7 report a `Match` for a body that does not fit, or a fit for something it never checked?

| # | Attack | What would go wrong | Fix |
| --- | --- | --- | --- |
| A1 | A body overlapping a neighbour by 1e-7 mm | Common reads 0 (P1): `verified` | FT5: within δ is a proximity, `not_verified` |
| A2 | A zone over a whole plane | Any coplanar intrusion excused | FZ2: the cited neighbour face only, that neighbour only, depth δ |
| A3 | An interference fit's annulus over the whole axis | A bar inside the bore excused | FZ3: the span's stations ± δ, the two radii ± δ, the source's neighbour only |
| A4 | A requirement removed in review, its zone gone | The original fails its control; every candidate loses `Match` | FZ1: zones kept; removal waives a check, not a contact |
| A5 | An edited socket adds a zone | A collision excused | Zones come only from a socket the gate accepted: digest, `from_json`'s structural checks; values cannot be edited, only removed (M3 9.4) |
| A6 | The original already interferes | Candidates inherit an assembly that does not fit | FT9's control |
| A7 | A copy collides with a copy in another socket | Missed when fit is asked per socket | FT3, FT11: one request with every occurrence |
| A8 | A free turn chosen where the body fits by luck | A claim about the wrong turn | FT10: the placement and its freedom stated; FK3: a `collides` case collides at every free turn |
| A9 | A helper that answers `verified` for everything | `Match` from an empty check | Rust decides from measures (FV1); the helper names no status; landmarks, zone volumes and the candidate's hash are cross-checked (FY4) |
| A10 | A stale or edited helper script | Old rules | FH3, FL6: hashes on disk and in the handshake |
| A11 | The kernel's error text in the protocol | A frame misparsed | FH2: frames on a private fd |
| A12 | Killing uv leaves the kernel running | Leaked processes, memory | FL2: the interpreter spawned directly |
| A13 | A timeout reported as a plain `not_verified` | Two documents for one input | FV8: the run is incomplete, exit 1 |
| A14 | A crash read as no interference | `verified` | FV8: never; a crash is retried once, then `kernel_crashed` |
| A15 | Dedup copies a fit from a member with equal features | A different body's fit reused | FC6: fit per member (QR6) |
| A16 | A stored blob altered after ingest | The fit of other bytes | FC6: ST11's hash; mismatch `stored_bytes_unreadable` |
| A17 | Readers disagree on a mapped item's frame | The fit of a body elsewhere | FB2's transform agreement; FB3's landmarks |
| A18 | Two leaves at one transform | The wrong body tested | FB2: bodies keyed by their faces' `#id`s too (P5) |
| A19 | Optimal boxes too small cull a near body | A collision skipped | FT8: ordinary boxes, enlarged |
| A20 | A face thickening that reports done | A zone of no volume, or a wrong one | FZ7: primitives only, each volume checked |
| A21 | A hostile file that hangs the reader | A stuck job | FL4's deadline and kill; FY1: only gated files |
| A22 | A server helper shared between users | One user's geometry in another's process | FY6, FX3: one session per job |
| A23 | `measured` numbers differ by OS and break parity | API and CLI documents differ | FM5: outside the digest; parity on one platform (FX6) |
| A24 | A `fits` case rejected by a wrong fit | A false negative, silently | FK1: a `fits` case's `Reject` by `interferes` is a gate failure; FO2 re-measures |

## 11. PR plan

Each PR has two adversarial reviews before merge, as in M4 to M6, and its rules join `RULES` with their tests (FK8).

1. **Core's seam and v2.** `FitRequest` v2, `FitBody`, `Freedom`, `FitScene`, `fit::zones`, `fit::plan`, `fit::decide`, `FitOutcome`, `FIT_RULES_VERSION`; the matcher's single request per candidate (FC2); placement freedom (FC4); the v2 writer, its digest and schema (FM1 to FM7); a test-only `MeasuresVerifier` that answers with given measures. `m7-fit.md` joins `DESIGNS`, with `M7_PR` deferrals.
   - **Done when** VT51 to VT54, VT61 and VT62 pass on three OSes; every 0.3.0 golden is byte-identical (VT52); the OpenAPI test sees `fitment.match/2`.
2. **`fitment-fit` and the helper.** The crate (frames, protocol, prepare, process, session), the helper script and lock, `fitment fit setup`'s self-test inside the helper, the adversarial and process tests with a fake helper; the pin grep.
   - **Done when** VT55 to VT57 and VT70 pass on three OSes without Python, VT71 passes with uv on Ubuntu and macOS, and a session on s01 gives the lug `interferes` and the bracket `verified` through the real helper.
3. **The CLI and the catalog.** The `fit` feature (default), `--fit`, `--fit-timeout`, `--fit-dir`, `--uv`, `fitment fit setup`; the table and stderr; `FitBlobs` and `judge_document_with_fetch`; dedup per member.
   - **Done when** VT64 and VT65 pass, and `fitment match` with `--fit` on the corpus gives no `Match` on a bad case and `Reject` on the three `collides` cases.
4. **The corpus, the oracle and the fit job.** FK2's cases, FK3, FK4's marks, FK5's original runs, the fit baseline; FO1 to FO3; the `fit` CI job on three OSes.
   - **Done when** VT58 to VT60, VT63 and VT72 pass in CI on Ubuntu and macOS (and Windows, or open question 7 is answered), and the report states every `not_verified` by code.
5. **The server and the image recipe.** `fitment-server`'s `fit` feature, `[fit]`, the start probe, sessions in jobs with permits and memory, the cancel stop point, components and catalog bytes, metrics, `fitment.info`; `deploy/Containerfile.fit` and `deploy/fit-smoke.sh`; the `container-fit` job.
   - **Done when** VT66 to VT69 pass, the official image is unchanged, and the API's corpus run with `occt-helper` gives the CLI's documents.
6. **The exit and the release.** The exit through the CLI and the API (M7-28's sentence, measured and written into PLAN); every M7 rule cited, the `M7_PR` allowance removed; PLAN, ARCHITECTURE, README and CHANGELOG (section 14); 0.4.0 (FG6).
   - **Done when** the exit holds on CI's three OSes (or two, per open question 7), VT73 passes with no `M7_PR` deferral, and the release PR is ready for the user to publish.

## 12. What was rejected, and why

- **Differential fit against the original** (M7-1's alternative; the oracle's KO1): simpler and robust, but it accepts whatever the original does, including an assembly that already interferes, and it compares a candidate with the original, which PLAN rules out for the interface and this design keeps out of the fit.
- **Offsetting the candidate by δ** for the proximity test (D1's first sketch): the bracket offsets cleanly (P1), but an offset of a general solid can fail where the distance cannot, and the distance to the body less its 2δ zones answers the same question with one kernel call per pair.
- **Zones from thickened faces:** P1's `MakeOffsetShape` thickening gave an empty solid for a plane and a wrong one for a cylinder while saying done.
- **The helper deciding the status** (D1's sketch): the rules would be Python, untestable without uv, and a helper that lies would decide.
- **JSON lines on stdout** (D1, the workspace layout): the kernel writes on stdout (P3).
- **`uv run` per spawn:** a kill of uv leaves Python running.
- **A process per request:** 0.3 s and an assembly read per candidate, with nothing gained over FL3's replacement after any failure.
- **A sidecar container for the server:** a network protocol, authentication and a kernel shared across users, for what a child process does.
- **A second official image with Open CASCADE:** D1 forbids it while #1564 is open.
- **D1's `not_verified` for every free placement** (M7-5's alternative): every one-axis socket would lose `Match`.
- **D1's `Reject("undeclared contact")`** for a proximity (M7-4's alternative): a body that touches does not interfere (KC3).
- **A `fitment.match` v2 for every run:** every 0.3.0 document would change for no new content.
- **Squeezing the evidence into v1:** v1's schema is closed; the evidence would be strings.
- **D7's warning in M7** (M7-23). Building it means: finding the neighbours that bear on the gripped seats and every part that moves with them (a bolt, its nut, its washers, along one axis), moving each by the grip's difference, checking that the moved neighbours still meet their own mates, then running the fit in that moved scene; and a new format field whose meaning (a `Match` with a warning) needs its own review. It recovers the grip-only thickness changes (four corpus cases), and nothing in the fit check needs it. It is better done once real catalog parts show how often grips differ (D8, D9).
- **Monstertruck or a mesh check** as a source of `Match`: D1's reasons stand.

## 13. Open questions

Each has the default the document assumes.

1. **M7-1: declared zones** with the original as control, rather than a check against the original. Default: zones.
2. **M7-5: a `Match` at a free placement** (a nut's turn), stated in the evidence, rather than D1's `not_verified`. Default: accept it; R3 says what it costs.
3. **M7-8: `fitment.match` v2 only for runs with a verifier;** v1 unchanged otherwise. Default: yes.
4. **M7-19: the CLI's `fit` feature on by default, and `--fit ASSEMBLY`** as its switch. Default: yes.
5. **M7-20: a committed recipe** for a user-built image with Open CASCADE, which CI builds and smoke-tests on every PR and never pushes. Does a CI-built, never-pushed image and a recipe in the repository stay within D1's "no Open CASCADE in official artifacts"? Default: yes; the alternative is the recipe documented in the README only, untested.
6. **M7-23: D7's warning deferred past M7.** Default: deferred, `Grip::Hard` only.
7. **Windows in the `fit` job.** Wheels exist; nothing here ran on Windows. Default: in the matrix; if it disagrees and M7 cannot fix it, fit is documented as untested there.
8. **A cap on fits per catalog run** (R8). Default: none beyond the job's deadline; the CLI prints the count of fits before it starts them.
9. **A stronger sandbox** for the helper on Linux (bubblewrap, or seccomp from Python) beyond its process and rlimits (FY7). Default: not in M7.
10. **M7-15: a timeout makes the run incomplete** (exit 1, a `failed` job), not a plain `not_verified`. Default: yes.
11. **M7-28: the exit's named exceptions** (s14, s20, the weldment's bearing and arm, whose assemblies interfere by construction) as the meaning of "the original part passes its own fit check". Default: yes.
12. **M7-29: release 0.4.0 at M7's exit,** with `fitment-fit`. Default: yes.

## 14. Changes to PLAN.md, ARCHITECTURE.md, the README and earlier designs

- **PLAN.md:** Resume here pointing at M7's PRs, then M7 done with the exit's numbers (VT58, VT59: `collides` cases, originals, `not_verified` by code, the three OSes); the Components table's fit verifier row as built (`fitment-fit`, the helper, the session); matching method step 7 (the fit's statuses and what makes each, `Match` reachable, exit 0); decision 11's CLI features bullet as built (`fit` on by default, `--fit ASSEMBLY`); D1's row: the helper as built, the official image unchanged, the recipe; D7's row: deferred past M7, with section 12's reasons; Milestones: M7's done-when as measured, and 0.4.0.
- **ARCHITECTURE.md:** the crate map with `fitment-fit` (the only crate that spawns a process); a section "Checking a fit" from section 3.1's flow (the session, the helper, the scene, the zones, the measures, the decision in core); Soundness (a fit from measures decided in Rust; a helper failure never a pass; the control); Determinism (v2's digest without the kernel's measures; one platform's bytes identical); Hostile input (only gated bytes reach the kernel; the helper's limits); Formats and versions (`fitment.match` v2 with a verifier, `FIT_RULES_VERSION`, `FIT_PROTOCOL`); the testing table's rows for the fit's fake-helper tests, the fit corpus job, the oracle's FO rules and the image recipe; "Where things are decided" gains this document.
- **README:** a section "Checking the fit" (`fitment fit setup`, `--fit`, what uv downloads, what a `Match` claims and does not: FT1, FT12, free placements), the server's `[fit]` and the recipe, with its licence note; the status line for 0.4.0.
- **m4-match.md:** VD4's per-socket request becomes one per candidate (FT11), with the new reason `another socket's interface rejected`; VD13 records D7's deferral past M7; KC3's `collides` gains FK1's codes; section 16.5's CI gains the `fit` job.
- **m5-catalog.md:** QR13 as built (FC6), QR6's per-member fit as built, R19 closed.
- **m6-api.md:** OP9 as built (FX1, FX2, FX8), JS8's new stop point (FX4), SN11 (`Match` through the API with `occt-helper`), DC11's `fit` field; "Deferred to M7 and M8": M7's items closed, M8's kept.
- **workspace-layout.md:** a note that the helper's protocol is binary frames on a private fd, not JSON lines (FH1, FH2), and that uv spawns no helper after `prepare` (FL2).

## Evidence

- The repository read: `docs/PLAN.md`, `docs/ARCHITECTURE.md`, `docs/research/d1-body-fit.md`, `m3-socket.md` (V1, N2, 8.3, 9.3), `m4-match.md` (sections 2, 9 to 12, 15 to 18), `m5-catalog.md` (QR6, QR7, QR11, QR13, ST11, R19), `m6-api.md` (OP9, SN11, SV8, JS1 to JS8, DC11, the deferrals), `workspace-layout.md` (sections 3, 7, 8); `crates/fitment-core/src/fit.rs`, `matching/matcher.rs`, `matching/align.rs`; `crates/fitment-server/src/matching.rs`, `config.rs`; `crates/fitment-catalog/src/query.rs`; `crates/fitment/Cargo.toml`, `src/cmd/match_.rs`; `deny.toml`; `deploy/Containerfile`; `tools/verify-occt.py`, `tools/gen-corpus.py`; `.github/workflows/ci.yml`.
- The probes: [m7-evidence/](m7-evidence/), table at the top. Run them with `uv` from the repository's root after `tools/gen-corpus.py corpus/out` and `cargo build --release -p fitment`; P2 needs `target/release/fitment`, and its `fixtures` mode `tools/fetch-fixtures.sh`.
- Open CASCADE's documents D1 cites for booleans (its source 18), BRepExtrema (19) and BRepAlgoAPI_Check (20); OCCT issue #1564 (38) and #1604 (41).
