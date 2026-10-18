# Architecture

How fitment is put together, and the rules every change keeps. The design
of each part is in [PLAN.md](PLAN.md) and, for reading features (milestone
M2), sockets (M3), matching (M4), the catalog (M5) and the REST API (M6), in
[research/m2-features.md](research/m2-features.md),
[research/m3-socket.md](research/m3-socket.md),
[research/m4-match.md](research/m4-match.md),
[research/m5-catalog.md](research/m5-catalog.md) and
[research/m6-api.md](research/m6-api.md); this page is the map.

## Crates

```text
fitment-server ──> tokio, hyper, axum, tower, argon2; rusqlite ──> server.db
  │ (unpublished)
  ├──────────────────> fitment-catalog ──> rusqlite (SQLite, bundled) ──> catalog.db
  │                          │
fitment (CLI) ──> fitment-catalog                                (no async crate)
  │                          │
  └──────────────────────────┴──> fitment-core ──> stepq (private: no type of it is public)
                                        └────────> libm, serde, serde_json, thiserror
fitment-corpus (unpublished, a dev-dependency only) ──> fitment-core, serde_json
```

- **`fitment-core`** is the library: bytes in, values out. It does no I/O
  of any kind (files, network, processes, threads, clock, environment);
  clippy bans them, and `tools/check-core.sh` checks the dependency graph
  against `deny/core.toml`. It never panics on input: `unwrap`, `expect`
  and `panic!` are denied, and an invariant a step finds broken is an
  `internal` refusal, not a crash.
- **stepq stays behind walls.** Only `src/gate/`, `src/features/step.rs`,
  `src/assembly/step.rs` and `src/properties/step.rs` name it, which `tools/check-core.sh` checks;
  everything else works on fitment's own copy of the solid
  (`features/topo.rs`) and of the product structure (`assembly/`). A stepq
  release is then a one-crate change.
- **`fitment-catalog`** is the store (M5): a directory with a SQLite
  database and every stored file under its SHA-256. It is the only crate
  that may use rusqlite (`deny.toml`), and its clippy configuration bans
  hash containers, the clock and the environment as the core's does. It
  is internal and unstable: the CLI is its one user.
- **`fitment`** is the command line: it reads the file, calls the core
  and the catalog, and prints. It never names stepq. Its `catalog` cargo
  feature, on by default, brings in `fitment-catalog`; without it,
  `fitment catalog` and `match --catalog` exit 2.
- **`fitment-server`** is the REST API (M6), library and binary: the only
  async crate, never published (`publish = false`, `dist = false`) and
  shipped as a container image with the CLI of the same version. It
  makes the CLI's library calls and keeps its own state in `server.db`,
  the one other database rusqlite may open (`deny.toml`). Only
  `catalog.rs` and its submodules name `fitment-catalog`, only
  `matching.rs` builds a `Matcher` or a `MatchDocument`, and only
  `state/db` opens `server.db` (`tests/layout.rs`, `tools/check-core.sh`);
  its clippy configuration bans spawning a thread and blocking a runtime
  worker in place (blocking work runs in `spawn_blocking`), and reading
  the environment outside `config.rs`. `deny/sync.toml` keeps tokio, hyper, axum and tower out of
  `fitment` and `fitment-catalog`, and the server's runtime graph has no
  schemars, TLS stack or HTTP/2.
- **`fitment-corpus`** is the corpus's one scorer (M6), never published
  and a dev-dependency only: the strict gate, the false positives and
  negatives, the baseline, the report and the `fitment.match` checker,
  over an `Outcome` read from a library result or a document's entry.
  The library's, the binary's, the catalog's and the API's suites all
  score with it, and `tools/check-core.sh` checks no published crate
  names it.

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
Assembly gate, checks 31 to 35: one root and no      gate/assembly.rs
  │   cycle, every usage placed and rigid, at most
  │   100,000 occurrences, no material in an
  │   assembly's own shape
  ▼
Checks 36 to 38: each distinct leaf read once as a     assembly/step.rs
  │   part (above; a refused one refuses the whole
  │   assembly), its frame and its range; parts
  │   listed with quantities and occurrences            assembly/parts.rs
  ▼
Selection: --part by id, name or #<n>; --occurrence    assembly/mod.rs
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
Requirements per occurrence: what each span lies on,  socket/combine.rs, own.rs
  │   and whether the original's own material lies in
  │   a core (U11)
  ▼
Merged sockets                                         socket/merge.rs
  ▼
fitment.socket document (JSON), its digest, the        socket/model.rs, check.rs, review.rs
review and its carry-over
```

Every pair a search returns counts toward check 39 (at most 10⁷); past it
the socket is refused rather than read slowly. A document that breaks a
structural rule, even with its digest recomputed, is refused on reading
(`check.rs`).

## Matching a candidate

```text
socket document ─ one socket: its active requirements,  matching/mod.rs (SocketView)
  │   contact tolerance, the target's own uncertainty
  ▼
Socket signature under the policy: windows at least     matching/signature.rs
  │   as wide as verification accepts; what fails every
  │   candidate set aside
candidate Part ─ its signature: stacks, planes, cones,  matching/signature.rs
  │   in values a rigid motion keeps
  ▼
Shortlist: keep or drop, never pass (level 2); an       matching/shortlist.rs, index.rs
  │   index of tokens for a catalog (level 1)
  ▼
candidate view: stacks with their usable profiles,      matching/candidate.rs
  │   zones taken only where every face stays outside;
  │   a tree over the faces, each boxed from the part
  ▼
Alignment: placements from pairs of features that       matching/align.rs
  │   match the socket's lines, seats and cones, in a
  │   fixed order, at most MAX_PLACEMENTS
  ▼
Verification at a placement, requirement by            matching/verify.rs, axial.rs,
  │   requirement: axes, spans, openings, the core       region.rs
  │   clear, seats covered, grips, cones; every measure
  │   recorded, every failure coded
  ▼
The matcher: per socket the first placement passing    matching/matcher.rs, verdict.rs
    everything M4 can verify, or the one passing most;
    the candidate's verdict is its worst socket's
```

The `fitment.match` document (`matching/document.rs`) states every
candidate in a fixed order with its evidence, and a digest that is the
same on every OS; `fitment match` writes it, or a table from it, and exits
with the best verdict (0 `Match`, 4 `InterfaceMatch`, 5 every candidate
rejected). Every comparison the shortlist makes counts toward a budget;
past it the candidate is kept, never dropped. Alignment stops at
`MAX_PLACEMENTS`, verification's coverage and core tests at
`MAX_COVERAGE_STEPS`; past either the requirement fails with
`alignment_exhausted` or `coverage_exhausted`, never passes. The body fit
is a `FitVerifier` the caller hands the matcher; this build's is
`NoFitVerifier`, so `InterfaceMatch` is the best verdict until M7. The
design, every rule and its tests are in `docs/research/m4-match.md`.

## Storing a part and matching from a catalog

```text
ingest FILE ─ read once into one buffer, capped (SZ1)      ingest.rs
  │   SHA-256 of that buffer: the blob's name and key
  ▼
gate and features, and the stated properties, from one     FeaturesDocument::read_with_properties
  │   parse; a refusal is stored too: quarantined
  ▼
derived rows, each hashed over a preimage naming its       preimage.rs
  │   content and versions: the compact features
  │   document, level 1's tokens and postings, the
  │   properties; with the entry and its revision, in
  │   one transaction per batch of at most 64 files
  ▼
catalog match ─ one read snapshot                          query.rs
  │   stale or empty catalog, or blocked socket: refused
  │   level 1 over the stored postings and tokens
  ▼
each survivor's row checked: its hash, its parse, its      snapshot.rs (QR4)
  │   versions, its part digest recomputed; a damaged
  │   row is named and not judged (exit 1)
  ▼
judged as its file would be (Matcher::judge_document),     fitment-core
    equal parts once, then MatchDocument::new over the
    results: a file run's document, restricted
```

The store keeps two rules. **Bytes before rows:** a blob is written,
synced and renamed into place before the transaction that names it
commits, so a row never names a missing file; a file is unlinked only
under the write lock, after checking no row names it. **A match reads the
index alone:** no stored file is opened while matching (CT6), so a run
costs what the index admits, and the store's private blob module cannot be
reached from the match path. Kept revisions (`replace`) keep their bytes
for `get ID@rN` until `prune --revisions`; their derived rows go, so level
1 never sees them. `reindex` writes the rows a newer build reads, batch by
batch, and `gc --versions` removes older ones, never a newer build's.
The design, every rule and its tests are in `docs/research/m5-catalog.md`.

## Serving the API

```text
client ─HTTPS─> TLS proxy ─HTTP/1.1─> accept loop: header timeout, head   net.rs
  │                                     and connection caps, write timeout
  ▼
tower layers: request id, trace, sensitive headers, panic caught,         http/mod.rs
  │   per-route body limits and timeouts, response headers
  ▼
router from OPERATIONS; Reader | Writer | Admin by token and role         http/operations.rs,
  │                                                                          extract.rs, auth/
  ├─ uploads: streamed, hashed as they come, synced, renamed  uploads.rs, state/objects.rs
  ├─ sync reads: a catalog snapshot from the reader pool,     catalog/read.rs, http/cache.rs
  │    stored documents sent as their bytes, checked
  ├─ sync writes: a review (one parse slot), a delete         http/routes/sockets.rs, catalog/
  └─ a job: its row and inputs committed before the answer    jobs/request.rs
        ▼
     scheduler: lanes query │ write │ maintenance, owners in turn,         jobs/scheduler.rs,
       per-owner caps, one fair CPU budget and a memory budget             jobs/mod.rs
        ▼
     one blocking task per job: the CLI's library calls, a stop flag      jobs/run.rs, socket.rs,
       and the catalog's Observer asked between files, batches and        matches.rs, catalog.rs,
       windows; a watchdog for a call that never returns                  stuck.rs
        ▼
     the document stored as an object, then the job's end committed;      jobs/events.rs,
       progress and notes as NDJSON events, never a verdict                runs.rs
disk:  <catalog>/ catalog.db, blobs/, tmp/   opened only through fitment-catalog (schema 1, M5's)
       <state>/   server.db, objects/sha256/, tmp/   opened only by fitment-server
```

**One rule decides sync or job:** an operation that parses STEP bytes,
judges a candidate or scans the whole catalog is a job (`features`,
`assembly_read`, `socket_extract`, `socket_import`, `match` and the
catalog's ingest, replace, reindex, prune, gc and verify); everything else
answers in the request. `Prefer: wait=N` (at most 60 s) answers with the
finished job. A job's state follows the exit code the CLI would give:
`succeeded` (0, 4 or 5), `refused` (2 or 3), `failed` (1, or no answer),
`cancelled` or `interrupted`.

**Three lanes over one budget.** Queries (reads, extractions, matches)
run `max(1, cpu / match_threads)` at a time, catalog writes one at a time
on the one writer, and `verify` beside them on the maintenance lane, all
inside one fair semaphore of `jobs.cpu` permits and a memory semaphore
whose estimates are heuristic (21 times a job's largest input, or per
entry for a catalog match). Each owner's jobs wait in their
own queue, owners in turn, within per-owner caps; a full lane refuses a
submission with nothing written. A cancel, a deadline or a drain sets the
job's flag, read where the job may stop; a pure job whose call never
returns is given up past a grace, its permits released, and a write job
always ends as its work ended.

**Two stores, the server's state apart from the catalog.** The catalog is
M5's, at schema 1: the CLI may work on it beside the server, and the
server sees its writes at its next snapshot. Everything the server adds,
users and tokens, uploads, assemblies, socket lines and their insert-only
revisions, jobs, their events and run records, lives in `server.db`
beside a content-addressed object store that keeps the catalog's rules:
bytes before rows, an object read only checked against its name, unlinked
only by the sweep under the write lock once no row names it. A job holds
its inputs from its submission until it ends, a match for as long as its
record is kept (30 days unless pinned). A lease in `server.db` keeps one
server per state directory; one server per catalog, both on a local disk,
is the deployment's rule. A restart settles what a killed process left: a
pure job runs again once, a write job cut short is `interrupted` with the
catalog's generation at its start and at recovery, and a result whose end
committed is kept. The design, every rule and its tests are in
`docs/research/m6-api.md`.

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
  section 9.6): a socket states what it cannot verify as requirements
  every candidate fails until a reviewer removes them: `unresolved` ones,
  partial bores and shafts, bores and shafts flagged `target_undecided` or
  `open_at_unknown_end`, spans that state nothing of the original (past
  a known end, or a zone with no pin, clear diameter or profile), cores
  the original's own material reaches, seats of unknown outline, grips
  between such seats and conical seats of unknown range.
  `Requirement::verifiable()` is the one predicate M3's tests and M4's
  verification both ask. M4 adds the floor (VD6: a tolerance that leaves
  nothing above the parts' own uncertainty, or a clearance whose limits
  leave the original no room, fails every candidate), the clear core
  (VR22: a bore's core must be free of the candidate's material, from
  faces boxed soundly), and the original's own run (Y4, VR21): every
  original in the corpus and the pinned fixtures, read in place, passes
  every requirement M4 can verify and fails exactly the others.
- **A catalog run is a file run, restricted.** It refuses a stale
  catalog rather than mix versions, never searches a quarantined entry,
  and judges each survivor from its stored document exactly as from its
  file, so its document equals a file run's over level 1's survivors,
  digest for digest. Each derived row's hash covers a preimage naming its
  content and versions, so a row moved to another key is caught on load;
  equal parts are found by digests recomputed from the loaded documents.
  A damaged or lost posting can hide a part until `verify` reports it, and
  can never pass one; a copied database is trusted only after `verify
  --deep`, which reads every part again from its file.
- **An API document is the CLI's bytes.** Every `features.v1`,
  `fitment.parts`, `fitment.socket`, `fitment.match`, `fitment.catalog`,
  `fitment.verify` and `fitment.gc` the server serves is the string the
  library returned, stored as an object and sent unchanged, with the
  CLI's exit code beside it (`Fitment-Exit-Code`); `matching.rs` makes the
  CLI's calls and no verdict of its own. No request overrides a refusal
  the CLI makes (a stale or unreviewed socket, a stale or empty catalog, a
  blocked socket); configuration can only tighten.
- **An error or a stop is never a verdict.** A cancelled, interrupted,
  failed or timed-out match stores no verdict anywhere: its events carry
  counts and notes, never a candidate's verdict, its `outcome` is null in
  every envelope, and a run the CLI would end with exit 1 keeps its
  document as `failed`, `complete: false`. No verdict is shown before its
  document is stored, and no endpoint takes a match document or run record
  as input. `Match` needs a verified fit: the server judges with
  `NoFitVerifier`, so `InterfaceMatch` is the best verdict until M7.
- **Never state a wrong known value.** Every rule that was found to guess
  in review was withdrawn or made to say `unknown` instead: the seam proof
  (F3), a walk that turns back along its axis (W7b), a drill point after a
  flat floor. The design records each finding.

## Formats and versions

- **`features.v1`** (`FEATURES_FORMAT_VERSION`) is the document layout:
  frozen at the first publish (M4, 0.1.0), as are `fitment.parts`,
  `fitment.socket` and `fitment.match` v1, with the extractor version.
- **`MATCH_RULES_VERSION`** rises with every change that could change a
  verdict or its evidence, and **`SIGNATURE_VERSION`** with every change
  to a signature or the index's tokens; a `fitment.match` document records
  both, the false-negative baseline does too, and the baseline refuses to
  bless a new false negative on a known case without a rise.
- **`EXTRACTOR_VERSION`** rises with every change that could change any
  features output: a rule, a constant, an order, a field. A document
  records it, and the catalog (M5) re-reads every file whose document has
  another. **`SOCKET_RULES_VERSION`** does the same for sockets, and
  `fitment.socket` (`SOCKET_FORMAT_VERSION`) is frozen at the first
  publish like `features.v1`.
- **The catalog** writes `fitment.catalog` v1 (`list`, `get`, and the
  changes of `ingest`, `replace`, `delete`, `reindex` and `prune`), and
  since M6 `fitment.verify` and `fitment.gc` v1, which hold no path and no
  time. Its
  database has a schema version, and a newer one is refused by every
  command. Its rows are keyed by content and version: features by
  `EXTRACTOR_VERSION`, tokens and postings also by `SIGNATURE_VERSION`
  (2 since M5: pair tokens name their stacks), properties by
  `PROPERTIES_VERSION`. A catalog with rows of another extractor or
  signature version is stale and refuses a match until `reindex`; one
  without current properties lists as `stale (properties)` and is still
  searched. A socket document of older rules is still
  read for its review decisions alone, its digest checked, so extracting
  again onto a reviewed file keeps the decisions on requirements that did
  not change. Golden files can only be re-blessed when a version they
  record has risen (`FITMENT_BLESS=1`; never under `CI`).
- **The API** is `/v1`; its own documents (`fitment.job`,
  `fitment.job.event`, `fitment.run`, `fitment.page`, `fitment.error`,
  `fitment.info`, `fitment.upload`, `fitment.assembly`, `fitment.token`,
  `fitment.user`) are envelopes that may gain optional fields within v1,
  and `fitment.info` states the API's minor, every version above and the
  limits. The result documents never change through the API. Errors are
  one registry of stable codes (`ErrorCode::ALL`), each with its class,
  exit code, HTTP status and whether a retry may succeed. The committed
  `openapi.json`, generated from the operation table and the documents'
  JSON Schemas (each crate's `schema` feature), is what the server serves,
  and a test fails when it drifts. `server.db` has its own application id
  and schema version, refused when newer and migrated forward at start.
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
   locale; the catalog counts changes by a generation number, not a time,
   and every query that lists rows has an `ORDER BY` or is collected into
   an ordered set;
5. `serde_json` with `float_roundtrip`, pinned by `Cargo.lock`;
6. through the API, documents are stored and served as bytes, never
   parsed and written again: job ids, times, users and URLs live only in
   the envelopes, `server.db`, logs and metrics, and socket ids derive
   from their data, so two servers' documents of one input are the same
   bytes, and their run records differ only in ids.

## Hostile input

A STEP file is untrusted. Beyond the gate's checks, every rule is meant to
be linear or `n log n` in the size of the file: reviews built files with
tens of thousands of faces sharing one edge, one plane, one cylinder or
one face full of holes, and each quadratic path they found was removed.
For sockets, `tests/hostile.rs` reads 10,008 bolts and refuses 36,008
coincident ones by check 39, and s55 is a plate of 3,600 coplanar tiles.
Coordinates beyond 1e6 mm are refused before any arithmetic depends on
them, and every bound is tested so that NaN fails it. The catalog reads at
most `--max-mib` of a file, bounds a part's stated properties while reading
them, escapes every stored name it prints, refuses a symbolic link inside
its store, and keeps its own files apart from the user's: an input inside
the catalog, or an output into it, is refused.

Over HTTP every input has a bound and a test that exceeds it: a request's
head (64 KiB, read within 10 s), JSON bodies (1 MiB), uploads (128 MiB,
streamed, cut when slower than 64 KiB/s or idle 30 s), socket documents
(16 MiB, parsed four at a time), candidates per run, pages, connections,
concurrent uploads and event streams per user, and requests per second
per user and logins per address. Slow clients are cut by the header,
chunk and write timeouts without delaying others. A path parameter is
parsed into its type before any handler runs; no name a client gives
becomes a path. A user reaches only their own uploads, assemblies,
sockets, matches and jobs: another's is `404`, and bytes another user
stored are answered exactly as new ones, so no answer or its time tells
that they exist. No response holds a path, and no response or log a
token or a password.

## Testing

| Layer | Where | What it shows |
| --- | --- | --- |
| Unit tests | `src/**` `#[cfg(test)]` | Rules on constructed solids, including the contradiction checks no valid file reaches |
| Unit set | `crates/fitment-core/tests/data`, `tests/parts.rs` | 24 generated parts (under 300 KB) checked against their construction on every OS |
| Review reproducers | `crates/fitment-core/tests/edits`, `tests/{planes,cylinders,holes}.rs` | Each input a review used to break a rule, with the answer it must give |
| Gate edits | `tests/features.rs` | Hand-edited STEP for the gate's checks, stage by stage |
| Unit assemblies | `crates/fitment-core/tests/assemblies`, `tests/assemblies.rs`, `tests/review.rs` | s01, s02, s03 and s08x, and the assembly gate's and review's checks as snippets and text edits |
| Hostile inputs | `tests/hostile.rs`, `crates/fitment/tests/matching.rs` | Assemblies grown from s08x: 10,008 bolts read, 36,008 coincident ones refused by check 39, 1,000 nuts on one axis; candidates of 10,000 identical holes, a 1,600-bore socket against 1,681 holes, 1,000 tiles and about 29,000 faces round one bore, each judged within its bound and 30 s |
| Match corpus | `tools/gen-corpus.py`, `crates/fitment/tests/matching.rs`, `tests/original.rs` | 125 match cases with truths from the construction (fits, fails, false negatives by a stated rule, edges, collisions, refusals), under both policies, through the library and the binary: the strict gate per case, the false-negative baseline, the rates (no false positive), the shortlist cross-check, determinism; every corpus and pinned fixture original read in place, AS1's parts across its exports, review runs |
| Committed candidates | `crates/fitment-core/tests/candidates`, `tests/candidates.rs`, the match goldens | 13 candidates against s01, s02 and s03, checked against their construction on every OS |
| Generated corpus | `tools/gen-corpus.py`, `crates/fitment/tests/corpus.rs` | Parts and assemblies with answers from the construction alone: parts also with pcurves (`_pc`) and in a skew frame (`_rot`); every assembly with sockets, but those written by hand edits or with frames Open CASCADE rewrites, also under a skew root (`_rot`), with its targets in a tilted frame (`_tgt`), with pcurves (`_pc`) and with its components in reverse order (`_perm`) |
| Goldens | `crates/fitment/tests/golden` | The exact bytes of each unit part's `features` document, and of each unit assembly's `parts` and `socket` documents; on three operating systems, the determinism check |
| Fixtures | `tools/fetch-fixtures.sh`, `fixtures/manifest.json`, `tests/fixtures.rs` | 45 public STEP files: pinned gate results and counts of planes, outlines, cylinders, holes and shafts; sockets pinned for chosen targets of the assemblies, and AS1's exports compared |
| Oracle | `tools/verify-occt.py` | Open CASCADE 8.0.1, a kernel the generator does not use: `manifest` checks the construction; `features` checks fitment's planes, cylinders, holes and shafts; `socket` checks every socket against the solids, both ways (spans, seats, touchers, grips, floors, touching faces, occupied holes, conical seats' senses and ranges, zones' profiles, and cores clear of the part's own material where no requirement says otherwise), but s55's, which it skips by name (its 7,440-face plate takes Open CASCADE hours), with the fixtures' tallies pinned but the sampled count of touching faces; `mutations` edits fitment's sockets fourteen ways and requires each edit to meet a disagreement; `match` judges every corpus candidate with the built fitment and checks each document: every pass fits physically at the placement fitment states (KO3), every failure re-measured (KO4), placements rigid with their features on their requirements (KO5), each bad case but a mirror image disturbing only the neighbours its defect concerns (KO2), and ten mutations of the documents each met (KO6); with `--expect`, AS1's parts across its exports, the tallies pinned (KO8); `manifest` also places each candidate by its construction and requires the physics to agree with its truth (KO1; not `edge`, `refused`, originals read in place, or grip-only thickness changes) |
| Catalog | `crates/fitment-catalog/tests`, `crates/fitment/tests/catalog_*.rs` | The store's invariants after every operation, faults at every file call and races between writers; every preimage field bound; reindex from catalogs written by older and newer builds, killed and resumed; on the generated corpus, every socket's catalog run equal to the file run restricted, and its exhaustive run to the file run, hostile parts included; a match with every stored file moved away; on Linux, macOS and Windows, the same catalog outputs |
| Scale | `tools/scale-step.py`, `crates/fitment/tests/catalog_scale.rs` | Weekly at 10⁴ scaled corpus parts, by hand at 10⁵: every corpus socket's full run the exhaustive run restricted, no pass hidden, timings reported |
| The scorer | `crates/fitment-corpus`, its `tests/scorer.rs` | One strict gate, rates, baseline and document checker for every path, with a control for each refusal; no test file keeps a scorer of its own |
| Server layers | `crates/fitment-server/tests/{api,auth,isolation,hostile,slow,wire,errors,state,process,layout,leaks}.rs` | In process, over a bound socket and as the built binary: every operation against every principal (the permission matrix), each user's resources invisible to another, every limit exceeded, slow clients cut, every error code's document, configuration and startup refusals, the crate's layering, and no secret or path in any response or log |
| Goldens and OpenAPI | `crates/fitment-server/tests/golden.rs`, `openapi.rs`, `tests/golden/api` | Every operation's requests and responses as exact bytes, every reachable error code produced, the same on three operating systems; the served OpenAPI document the committed one, its schemas validating every golden body |
| Parity | `crates/fitment-server/tests/parity.rs` | Every document a job or route serves against the CLI's bytes and exit code for the same input (the library's for components): features, parts, sockets, reviews, imports, matches and every catalog command, on every OS; the corpus's assemblies and sockets in release |
| Faults and concurrency | `crates/fitment-server/tests/{fault,concurrency,soak}.rs` | An abort at every hook point of each job kind, then a restart: each job ends once, the catalog consistent; cancels, deadlines, drains, a stuck call given up, a failing database, a CLI writing beside the server, many clients' runs alike; weekly, a restart soak of random kills and a full disk |
| The exit | `crates/fitment-server/tests/exit.rs`, CI's `server-corpus` job | The built server as a child over loopback, in release: every corpus match case under both policies, file and component candidates, through upload, register, extract, review and match, each document the CLI's (or the library's), the strict gate, no false positive, the baseline's false negatives, and `api-report.json` equal to the library's `report.json`; the corpus ingested and every catalog run of the exit set under three policies the CLI's, `verify --deep` clean after |
| The container | `deploy/Containerfile`, `deploy/smoke.sh`, CI's `container` job | The image built on every PR: no Python, uv or Open CASCADE, a non-root user, both binaries of one version, the crates' licences; a smoke run with a read-only root through the first admin, uploads, a socket and a match to the CLI's goldens, and a clean stop |
| Fuzzing and load | `fuzz/`, `crates/fitment-server/src/fuzzing.rs`, `tests/scale.rs`, `fuzz.yml`, `scale.yml` | Weekly: arbitrary requests through the router and arbitrary bodies through every request type, seeded with the goldens' requests (no panic, no unexpected `5xx`, every error the error document); the 10⁴ scaled catalog through the API, sixteen clients at once, and 32 clients over the corpus |

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
- Signature, alignment, verification, verdicts and the corpus:
  [research/m4-match.md](research/m4-match.md).
- The catalog: store, index, identity, changes, re-indexing, properties
  and the catalog match: [research/m5-catalog.md](research/m5-catalog.md).
- The REST API: resources, jobs, uploads, the server's state, errors,
  authentication, hardening, deployment, the scorer and the exit:
  [research/m6-api.md](research/m6-api.md).
- Milestones, matching method and verdicts: [PLAN.md](PLAN.md).
- stepq's reading of STEP: its own repository and
  [research/m1-stepq-read.md](research/m1-stepq-read.md).
