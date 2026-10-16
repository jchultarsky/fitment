# fitment M5, design B: the index and the query path at catalog scale

As of 2026-10-16. Status: **proposal** by designer B, for merging with the other M5 designs. Nothing here is implemented in the repository. Everything measured was measured in a scratch prototype (section 15), on a snapshot of the `m4/exit` worktree taken on 16 Oct 2026 (`a42853c` plus its uncommitted PR 7 changes): `EXTRACTOR_VERSION` 6, `SIGNATURE_VERSION` 1, `MATCH_RULES_VERSION` 1, stepq 0.6.0, rusqlite 0.40 with bundled SQLite. The machine was an Apple M5 Max (18 cores, 64 GB).

Scope: what the catalog stores per part so that a query never reads a STEP file, how level 1 lives in SQLite, how level 2 and verification run over its hits, what the query costs from 10³ to 10⁵ parts, how the catalog run produces the same `fitment.match` document as the file run, and why the index can neither pass a bad part nor hide a good one. Content-addressed storage, CRUD, quarantine records and the server are other designers' areas. They appear here only where the query depends on them.

Rule prefixes are provisional, to be renumbered when the designs are merged. Each fits the coverage test's `[A-Z]{1,2}[0-9]+` pattern:

- CS: what is stored per part;
- IX: level 1, the index;
- QR: the query;
- CD: the document a catalog run writes;
- RX: versions, staleness and re-indexing;
- CT: tests.

## 1. Summary

- **A part is stored as its `features.v1` document and nothing else that verification reads.** Every corpus part's features read back from JSON to an identical `Part`, in pretty and in compact form (232 of 232). `judge_step` on the STEP bytes equals `judge_part` on the stored features in 105 of 105 corpus match cases. Verification therefore runs from stored features alone, and no `PartSignature` or `CandidateView` needs to be stored: level 2 recomputes the signature in about 1 µs.
- **Level 1 is M4's `ShortlistIndex` with its postings in SQLite, under one algorithm.** Core gains an `IndexSource` trait. The in-memory index and the catalog's tables both implement it, and the only SQL is three plain reads: postings by sense and bin range, every indexed id, and one part's tokens. The catalog holds no shortlist logic.
- **Level 1 judges each part at its own lever.** It no longer uses the catalog's largest lever. Level 2 uses `max(socket lever, part lever)`, so this is still a superset. A part's admission then never depends on what else is in the catalog, and it removed a 20 to 25 ms full scan from every query at 10⁵. Bucketing seat groups by powers of two keeps the cost flat.
- **A catalog query runs as follows:** the socket gate, the catalog gate (current versions only), then one read transaction. Level 1 runs in SQL and level 2 on each part's features; a level-2 drop ends that part (M4-17). Parts are then judged once per distinct `Part`, in parallel, and the result is a `fitment.match` v1 document. Every entry in it is identical, labels aside, to the one `fitment match SOCKET file.stp` writes for the same bytes. Measured on all 21 unblocked corpus sockets, the entries matched one for one and in the same order. The 11 blocked sockets pass nothing in a file run either (0 of 457 each).
- **At 10⁵ parts, level 1 drops nothing level 2 keeps.** Each of 20 sockets gives a document with the same digest whether level 1 runs or is bypassed. This held both before and after the per-candidate lever.
- **Cost at 10⁵ parts** (corpus parts at 439 scales each; 16 threads): 8 to 415 ms a query. Single-threaded it is 47 ms to 2.8 s, and judging the survivors dominates. Level 1 takes 0.2 to 45 ms. Bypassing level 1 entirely costs about 1 s of wall time and about 10 s of CPU time. Ingest runs at 0.23 ms a part (7.9 ms for real NIST-like parts). The database is 1.32 GB with compact JSON (3.1 GB pretty). Real parts are about 13 times larger.
- **The worst case is many parts that really fit.** With 439 exact copies of each part (10⁵ in all), s01's bracket keeps 9,219 candidates, which took 48.5 s and produced a 111 MB document. Judging once per `Part` digest gives a byte-identical document in 1.76 s. Distinct parts that all fit cannot be shortcut, and section 13 lists that as a risk.
- **Staleness can hide a good part (old tokens) or pass a bad one (old features).** A query therefore refuses a catalog with any row not at this build's `EXTRACTOR_VERSION` and `SIGNATURE_VERSION` (exit 3). The check costs 17 µs. `fitment catalog reindex` recomputes tokens from stored features when only the signature version rose, and re-reads the STEP files, quarantined ones included, when the extractor version rose.
- **Five PRs** (section 12): the core API, the tables and ingest, the query and `fitment match --catalog`, versions and re-indexing, and the scale tests and exit.

## 2. Decisions for the user

Each decision has a default that the rest of this document assumes. Rows marked **Your call.** change a format, a promise, or what the user sees.

| # | Question | Default | Cost and benefit |
| --- | --- | --- | --- |
| B1 | What verification reads | The stored `features.v1` document only. No stored `PartSignature`; level 2 recomputes it from the features (CS2, CS7) | One source of truth; a signature can never be stale. Cost: level-2 drops must load and parse their features, 0.03 ms each for corpus parts and 0.38 ms for real ones. A stored signature would be about 10 times smaller (13.2 KB a part against 135.7 KB on NIST-like parts) and would need `Deserialize` on `PartSignature`. Revisit if real catalogs show level-2 loading dominating |
| B2 | Where features live | In SQLite, in the same transaction as the part's tokens (CS6), as compact JSON (CS2) | One write keeps the index and its features consistent, and a query needs nothing else on disk. Measured: 1.32 GB for 10⁵ corpus parts; 1.53 GB for 7,928 NIST-like parts, so about 19 GB at 10⁵ (an estimate). Alternative: files beside the blobs, at the cost of cross-store consistency |
| B3 | Compression | None: no new dependency | Deflate would store 8% of pretty JSON's bytes, compact JSON stores 37%. Add it later behind a schema version if size hurts |
| B4 | **Your call.** Compact pair tokens | Yes, in PR 1: a pair token names its two stacks by index instead of repeating their diameter lists. `SIGNATURE_VERSION` becomes 2 | Real parts carry 36.6 KB of tokens each, more than their whole signature (13.2 KB), and level 1 parses them for every hit: 262 ms for 1,668 hits on 7,928 NIST-like parts. An estimated 2 to 3 times less (not measured). It changes `signature_version` in every `fitment.match` document, so the match goldens are re-blessed (allowed, since a version rose). Without it, M5 stores version 1 tokens and the change costs a re-index later |
| B5 | Plane tokens (grip separation, seat area) | Deferred: not in M5 | On a seats-only socket, level 1 admits 98,766 of 100,083 parts and level 2 keeps 202. That costs about 10 s of CPU and 2.2 s of wall time at 16 threads. A grip-separation token would admit 5.8% (measured at 10⁴). It is a `SIGNATURE_VERSION` rise, worth doing when real catalogs show seats-only sockets are common |
| B6 | **Your call.** A catalog with stale rows | The query refuses (exit 3) and names `fitment catalog reindex`. Stale rows are never skipped silently (RX2) | Determinism and no hidden parts. Cost: after an upgrade nothing can be queried until the re-index finishes. Measured at 10⁵ corpus parts: a full re-read takes about 5 s at 16 threads; tokens alone take less (estimated) |
| B7 | Judging once per distinct `Part` | Yes, while the fit verifier is `NoFitVerifier` (QR6) | Exact: the documents are byte-identical (measured). 48.5 s became 1.76 s on 439 copies of each part. M7 judges the fit per file, since the body can differ where the features do not |
| B8 | **Your call.** The document of a catalog run | `fitment.match` v1 unchanged. Labels are `catalog:<id>`; `source` is the file's (SHA-256, bytes, header). Catalog facts (parts searched, quarantined, level-1 hits, level-2 kept, generation) go to stderr and the table header, not into the document (CD1 to CD4) | Every entry is comparable to a file run's entry, and the frozen format holds. Cost: the document does not say how much of the catalog was searched; a stored run record must say it. Alternative: `fitment.match` v2 with a `catalog` block |
| B9 | `--catalog` together with file candidates | Not allowed in M5 (usage, exit 2) | M4-17's explicit semantics (drops listed) and the catalog's (drops end) do not mix in one document. Revisit with M6 |
| B10 | A limit on candidates or time | None: every candidate the index admits is judged and listed | Deterministic and complete. Cost: 9,219 interface matches make a 111 MB document. A ranked `--limit` needs a ranking MF5 does not define (later; M6 wants pagination anyway) |
| B11 | Parallel judging | `--jobs N`, default `std::thread::available_parallelism()`. Output is independent of N (QR8) | 16 threads cut wall time by 4 to 8 times on queries over 100 ms. M4 left parallel judging open (27.2, question 11); the catalog needs it |
| B12 | Assembly leaves as catalog parts | No in M5: one part per catalog entry, from a part file | The corpus's 10 component cases stay file and library runs (CT1) |
| B13 | Checking level 2 in a catalog run | A hidden `--audit` judges every indexed part as an explicit candidate. Level 1 is bypassed, drops are listed with their evidence, and KH9's check applies (QR10). Tests use it; users need not | A catalog run, unlike a file run, lists nothing it dropped, so a level-2 bug would hide parts silently. `--audit` restores the check where tests can see it |

## 3. How a catalog query runs

```text
socket.json ─ MF2's socket gate ──────────────────────────── refused: exit 3
                     │ Matcher::new (policy, options): per socket, its signature
catalog DIR ─ catalog gate (QR1) ─────────────────────────── not current: exit 3
              schema known; every indexed row at this         unreadable: exit 1
              EXTRACTOR_VERSION and SIGNATURE_VERSION
                     │ one read transaction from here to the end (QR2)
                     ▼
level 1 (IX): Matcher::admitted(&SqlSource) ───────────────── blocked: no candidate,
              per socket, core's level_one over the SQL         stderr says why, exit 5
              postings; intersected over sockets
                     │ ids, ascending
                     ▼
load (QR4): features row of each id, by SHA-256; from_json (a bad row: exit 1)
                     │
level 2 (QR5): Matcher::keeps(&PartSignature::of(part)) ───── drop: the part ends here
                     │
dedup (QR6): group by part_digest; one representative per group
                     │
judge (QR7): Matcher::judge_stored(features, source), workers over representatives
             (QR8); each member gets the result with its own source
                     │
MatchDocument::new(results, labels "catalog:<id>"): MF5's order, the digest (CD)
                     ▼
table or JSON, -o, the exit code (VD14); stderr: what was searched (CD4)
```

Nothing in this path opens the blob store (QR11). The fit verifier is the one exception to come. At M7 it needs the STEP bytes of each candidate that passed its interface, and the catalog will hand them over then (section 13).

## 4. What is stored per part (CS)

### 4.1 The rules

- **CS1. One row per distinct SHA-256.** A catalog entry is a part file named by the SHA-256 of its bytes. Adding bytes the catalog already holds returns the existing id and adds no row. An accepted file is `indexed`. A file M2's gate refuses is `quarantined`, with its refusals, and has no features, tokens or postings (Soundness rule 6). Ids are integers assigned at commit, never reused.
- **CS2. The features row is verification's only input.** It holds the `FeaturesDocument` read from exactly those bytes, stored as `serde_json::to_string(&doc)` (compact). It is keyed by the bytes' SHA-256, and only ingest writes it, through one function that hashes and reads the same in-memory buffer (`features_for(bytes) -> (sha256, FeaturesDocument)`). A row for SHA X can therefore only hold what bytes X read as. `fitment catalog get --features ID` prints `to_json()` of the parsed document, which is byte-for-byte what `fitment features FILE` prints.
- **CS3. Level-1 tokens are derived and versioned.** `IndexTokens::of(&PartSignature::of(part))` is stored as JSON in the part row, and its postings as rows of the `posting` table, both with `signature_version`. The catalog never computes a bin or a token itself: `IndexTokens::postings()` in core yields (sense, bin, stack, diameter) (IX2).
- **CS4. The part digest is derived.** It is the SHA-256 of the compact JSON of the `Part` alone: no header, no source, no versions. Two files whose parts read the same share a digest. Core computes it (`FeaturesDocument::part_digest()`), and QR6 uses it.
- **CS5. Versions per row.** Each row records `extractor_version` (from the features document) and `signature_version` (from the tokens). RX1 to RX5 decide what a mismatch does.
- **CS6. One transaction per part.** The part row, its features row and its postings are written in one transaction; replacing a part deletes its rows and inserts new ones in one transaction. A replace never updates a SHA in place. A reader sees all of a part or none of it.
- **CS7. Nothing else is stored for matching.** There is no `PartSignature` (B1), no `CandidateView`, and no verdict, check or socket result: a verdict depends on the socket, the policy and `MATCH_RULES_VERSION`, and caching it would need all three in the key and a reason. Stored match runs, if the store design keeps them, are records for audit (plain data, M4-23), never inputs.
- **CS8. The source comes from storage.** A candidate's `source` in the document is the row's SHA-256, its byte count, and the header the features document recorded. The header is read from the same bytes at ingest (CS2), so nothing is re-read.

### 4.2 The tables (the parts the query reads)

```sql
-- schema version and the write generation (QR2, CD4); the store design owns the rest
CREATE TABLE meta (key TEXT PRIMARY KEY, value INTEGER NOT NULL) WITHOUT ROWID;

CREATE TABLE features (
  sha256 TEXT PRIMARY KEY,                -- 64 lower-case hex digits of the STEP bytes
  json   TEXT NOT NULL                    -- compact features.v1 (CS2)
) WITHOUT ROWID;

CREATE TABLE part (
  id                INTEGER PRIMARY KEY AUTOINCREMENT,   -- never reused (CS1)
  sha256            TEXT NOT NULL UNIQUE,
  bytes             INTEGER NOT NULL,
  status            TEXT NOT NULL CHECK (status IN ('indexed', 'quarantined')),
  extractor_version INTEGER NOT NULL,
  signature_version INTEGER,              -- NULL when quarantined
  part_digest       TEXT,                 -- CS4
  planes            INTEGER,              -- IndexTokens.planes, for inspection
  lever_mm          REAL,                 -- IndexTokens.lever_mm, for inspection
  tokens            TEXT                  -- IndexTokens JSON (CS3)
  -- name, added time, and so on: the store design's
);
CREATE INDEX part_versions ON part (status, extractor_version, signature_version);
CREATE INDEX part_digest   ON part (part_digest);

CREATE TABLE posting (
  sense       INTEGER NOT NULL,           -- 0 hole, 1 shaft (core's sense_code)
  bin         INTEGER NOT NULL,           -- core's bin(diameter)
  part        INTEGER NOT NULL REFERENCES part (id) ON DELETE CASCADE,
  stack       INTEGER NOT NULL,           -- index into IndexTokens.stacks
  diameter_mm REAL NOT NULL,
  PRIMARY KEY (sense, bin, part, stack, diameter_mm)
) WITHOUT ROWID;
```

A `REAL` column stores an IEEE 754 double and returns it bit for bit. Every stored diameter is finite and positive, so neither of SQLite's lossy cases arises (NaN becomes NULL, and −0.0 can come back as +0.0). CT4 tests the round trip at bin edges. The exact comparison is made in core on the `f64` that comes back (IX3).

### 4.3 The write path the query relies on

The store design owns ingest. The query depends only on these steps, in this order, inside one write transaction per part:

1. Read the bytes into memory once. Compute their SHA-256. If a row with that SHA exists, return its id and stop (CS1).
2. Run `FeaturesDocument::read(&bytes)` on the same buffer. If it is refused, insert a `quarantined` part row with the refusals and stop.
3. Compute `PartSignature::of(part)`, then `IndexTokens::of(&signature)`, then `part_digest()`. These are pure functions of the `Part`.
4. Insert the `features` row keyed by the SHA, the `part` row (status `indexed`, both versions, digest, tokens), and one `posting` row per `IndexTokens::postings()` item.
5. Increment `meta.generation`. Commit.

Reading (step 2) runs on worker threads; the single writer serializes steps 4 and 5. A crash before the commit leaves nothing; the blob file, if written first (store design), is unreferenced and found by `catalog check`.

### 4.4 Why the features, and what they cost

`Matcher::judge_part` builds a `CandidateView` from a `&Part`: stacks with their usable profiles, shell planes with their outlines, cones, and a tree of face boxes for VR22, all from what the part states. Nothing comes from the STEP file except through `Part`. `Part` deserializes (it is `features.v1`'s payload), and `FeaturesDocument::from_json` checks everything `read_part` guarantees. The probe checked the round trip in both forms (`out/roundtrip.txt`, `out/roundtrip-bases.txt`):

| What | Result |
| --- | --- |
| `from_json(read(x).to_json()) == read(x)` and the JSON written again is byte-equal; the same from compact JSON | 232 of 232 accepted corpus files (4 hostile included) |
| `judge_step(bytes) == judge_part(stored part, File source)`, the whole `CandidateResult` compared | 105 of 105 file-candidate match cases with a socket (the other 10 are refused files or the refused `r27` socket) |
| Compact JSON against pretty | 37.3% of the bytes; `from_json` 0.029 ms a part against 0.038 (corpus parts) |
| Sizes a part, compact (`out/sizes.txt`) | Corpus: features 10.6 KB, signature 1.9 KB, tokens 0.5 KB. NIST-like (24 fixture parts): features 135.7 KB, signature 13.2 KB, tokens 36.6 KB |
| Largest features document | 80.8 MB pretty, for `h_bracket_stubs` (a 36.7 MB STEP file) |

Verification can run from stored features alone, so a query never needs the STEP file. The features are 5 to 10 times larger than a signature would be, but they are the one thing that must be stored anyway (PLAN, Components: "the extracted features with the extractor version").

## 5. Level 1 in SQLite (IX)

- **IX1. One algorithm, in core.** M4's SL8 becomes `level_one<S: IndexSource>(socket: &SocketSignature, source: &S)`. `ShortlistIndex` is one `IndexSource` (in memory; M4's tests, the harness); the catalog's `SqlSource` is another. `ShortlistIndex::query` is `level_one(self)`. The catalog crate contains no window, no bin arithmetic and no filter.

  ```rust
  pub trait IndexSource {
      type Error;
      /// Every posting of `sense` whose bin lies in `bins`: (part, stack, diameter), any order.
      fn postings(&self, sense: Sense, bins: RangeInclusive<i64>,
                  each: &mut dyn FnMut(u64, u32, f64)) -> Result<(), Self::Error>;
      /// Every indexed part, ascending (a socket that demands no line).
      fn all(&self) -> Result<Vec<u64>, Self::Error>;
      /// The tokens of each of `ids`, ascending; an id with none is an error.
      fn tokens(&self, ids: &[u64], each: &mut dyn FnMut(u64, &IndexTokens)) -> Result<(), Self::Error>;
  }
  ```

  The catalog's SQL, all of it:

  ```sql
  SELECT part, stack, diameter_mm FROM posting WHERE sense = ?1 AND bin BETWEEN ?2 AND ?3;
  SELECT id FROM part WHERE status = 'indexed' ORDER BY id;
  SELECT tokens FROM part WHERE id = ?1;
  ```
- **IX2. Bins come from core only.** `IndexTokens::postings()` yields what ingest writes, and `level_one` computes every probe's bin range. `bin()` and `BIN_MM` stay private to core; a change to either is a `SIGNATURE_VERSION` rise (RX1).
- **IX3. Exact comparison after the probe.** As SG10 states: the probe reads bins `bin(lo) − 1` to `bin(hi) + 1` (an unbounded end reads to `i64::MIN` or `i64::MAX`), and each diameter that comes back is tested with `Window::holds` on the same `f64`. SQL never compares a diameter.
- **IX4. Narrowest window first.** Each distinct (sense, narrowest window of a line) is probed in ascending order of width, and the set is intersected after each probe. An empty set ends level 1. The result is the same as M4's; only the work changes.
- **IX5. Each part at its own lever.** The pair filter and the seat-group count use `max(socket lever, the part's lever_mm)`, not the catalog's largest. Level 2 uses exactly `max(socket.lever_mm, part.lever_mm())` (shortlist.rs), and `IndexTokens.lever_mm` is that same value, so level 1's windows contain level 2's for every part. Seat groups only coarsen as the lever grows, so a lever rounded up to a power of two asks for no more planes than level 2 does. That bounds the group computations to about 40 per query. Measured: without buckets, s01's bracket took 286 ms of level 1; with them, 33 ms.
- **IX6. Independence.** Whether part p is admitted depends only on the socket, the policy and p's own tokens, never on the rest of the catalog. Adding, replacing or removing other parts cannot change p's admission. This follows from IX5, and CT3 tests it. (M4's in-memory index used the largest lever indexed; one 4.7 m part widened every query's pair window.)
- **IX7. What level 1 admits, unchanged from M4.** For each line, some stack of its sense with a diameter in its narrowest window. Each part needs at least as many planes as the socket has seat groups, at the lever of IX5. Each parallel pair with the most selective line needs a pair token in range. A part whose pairs are incomplete, or a policy whose angular window exceeds the tokens' 0.1°, bypasses the pair filter. A socket that demands no line admits every indexed part to the plane and pair filters.
- **IX8. Blocked.** If any socket of the document is blocked (an active requirement fails every candidate, SG7, or the socket demands nothing, SL8), the query has no candidate. The candidate must pass every socket (VD2), and a blocked socket fails all of them.
- **IX9. Tokens deserialize but are never trusted for a pass.** `IndexTokens` and `PairToken` gain `Deserialize` (with B4, a pair token is `{a, b, distance_mm}` with indices into `stacks`). A wrong token can only make level 1 admit too much, which level 2 corrects, or too little, which hides a part (the RX rules and `catalog check`, CT12). It can never make a part pass.

## 6. The query (QR)

- **QR1. The catalog gate.** The catalog opens read-only. `meta.schema_version` must be one this build reads, and no row with `status = 'indexed'` may have another `extractor_version` or `signature_version` (an index lookup, 17 µs at 10⁵; a scan took 21 ms). Otherwise the query is refused (RX2). An empty catalog is allowed: there is no candidate, and the exit code is 5.
- **QR2. One snapshot.** Every read of a run (level 1, the tokens, every features row) is made on one connection inside one read transaction. Workers receive parsed documents and never touch the database. With threads on their own connections, each would see its own snapshot, and a part replaced in between would mix two states. `meta.generation`, which every write transaction increments, is read at the start and reported (CD4).
- **QR3. Level 1** is `Matcher::admitted(&SqlSource)`: core's `level_one` for each socket's signature, built by `Matcher::new` under the run's policy, intersected over the sockets, or `Blocked { socket, why }`.
- **QR4. Load.** The admitted parts' features rows are read in ascending id and parsed with `FeaturesDocument::from_json`. A row that fails to parse, is not `accepted`, or is not current (RX7) is a catalog error. The run exits 1 and names the part; it never skips the row.
- **QR5. Level 2 ends a part.** `Matcher::keeps(&PartSignature::of(part))` runs M4's `shortlist` against each socket's signature, the same signatures `judge_part` uses. A drop ends the part, as M4-17 requires of a catalog. A dropped part is not listed (CD4).
- **QR6. Dedup.** Kept parts are grouped by `part_digest`. One representative per group (its lowest id) is judged, and each member gets a clone of the result with its own `source`. `CandidateResult` is a function of the `Part` alone; the `source` is only copied in (`Matcher::judge`). So this is exact, and the measured documents were byte-identical (`out/dedup.txt`). It applies only while the matcher's fit verifier names no kernel. With a verifier (M7), the interface checks may be shared but the fit is asked per member, since two files with equal features can carry different bodies (an `other` face differs in its surface, not in its record).
- **QR7. Judgment.** `Matcher::judge_stored(&FeaturesDocument, Source) -> Result<CandidateResult, NotCurrent>` refuses a document whose `extractor_version` is not this build's, then calls `judge_part`. Because of this refusal, the result's `read.extractor_version`, which `judge` fills from the build's constant, cannot state a version the features were not read by (attack A6).
- **QR8. Parallel and deterministic.** Representatives are judged on `--jobs` threads. Results are collected and handed to `MatchDocument::new`, whose MF5 order is total (CD5). The document's bytes do not depend on the number of jobs, the order threads finish in, or the order of SQL rows. Measured: 1, 7 and 16 threads, and level 1 from SQL or from memory, gave byte-identical documents (`out/determinism.txt`).
- **QR9. Budgets.** Each candidate keeps M4's bounds: `MAX_SHORTLIST_CHECKS`, `MAX_PLACEMENTS`, `MAX_COVERAGE_STEPS`, about 5 s per candidate and socket at worst. The catalog adds no time limit (B10). A hostile part in the catalog costs its bound whenever it is admitted. Measured: `h_hole_grid` (10,000 holes) took 3.2 s against s01's bracket.
- **QR10. Audit mode** (hidden `--audit`, B13). Level 1 is bypassed and every indexed part is judged with `judge_stored` under M4's explicit semantics: a level-2 drop still aligns and verifies, is listed with its `missing`, and a pass after a drop is a fitment bug (exit 1, M4-17). The document equals a file run over the catalog's files, digest included (CT2).
- **QR11. Only the index.** The query takes `&Index` (the SQLite connection), never the blob store's type. The blob store is not opened, and CT6 moves it away to prove it.
- **QR12. The original.** `--original ASM` works as in M4. It reads the assembly the user names, which is not a catalog read.

### 6.1 A worked example: s01's bracket against 10⁵ parts

The socket document `s01_bolted__BRACKET.json` has one socket with 7 active requirements, all unreviewed, and 1 hole left out. The requirements are:

- `seat:plane-467` and `seat:plane-570`;
- `grip:plane-467+plane-570` (8 mm);
- four bores `bore:hole-654` … `bore:hole-722`, each Ø6.6 clearance over a Ø6 shaft, 0 to 8 mm, open at the start.

The catalog is c5 (100,083 parts) and the policy is the default (0.01 mm, 0.01°, strict).

1. **Gate.** The socket document is accepted and current. The catalog is at schema 1, and every indexed row is at extractor 6 and signature 1, checked through `part_versions` in microseconds.
2. **Level 1.** All four bores have the same strict clearance window, [c − σ, d + σ] = [5.99, 6.61] widened by the slack, so IX4 makes one probe: holes, bins `bin(5.99) − 1` to `bin(6.61) + 1` (58 to 67). Of the postings returned, those whose diameter the window holds give 177 parts. The plane filter (two seat groups need two planes) and the pair filter then run at each part's own lever: 95 parts are admitted, in 33 ms. With the catalog's largest lever (314 mm), 177 would have passed.
3. **Load and level 2.** 95 features rows are read in one transaction and parsed (about 10 ms of CPU). Level 2 keeps 32 parts. For each of the other 63, the shortlist's `Missing` names what it lacks: a pair of holes at a required distance, a plane with a seat's area, or a pair of planes at the grip.
4. **Dedup.** The 32 parts have 32 distinct digests (scaled copies never share one), so 32 judgments run.
5. **Judgment.** About 1.4 s of CPU, mostly coverage marching on the seats. The slowest part took 92 ms. With 16 threads, the judgment's wall time is 0.17 s.
6. **The document.** 32 candidates: 11 interface matches and 21 rejects, nearest first. The 11 are the corpus's passing brackets at scale 1. No scaled copy keeps both its holes within [5.99, 6.61] mm and its grip within ε of 8 mm. The whole query took 196 ms. The same 32 entries, with the same digest, come from a run that bypasses level 1 (10.2), and the 11 are exactly the corpus's passing brackets.

## 7. The document a catalog run writes (CD)

- **CD1. `fitment.match` v1, unchanged.** No field is added, removed or reinterpreted (B8). `summary.candidates` counts the judged candidates, that is, the level-2 survivors.
- **CD2. Entries equal a file run's.** For a part with bytes B, the catalog run's entry, its labels aside, equals the entry `fitment match SOCKET B.stp` writes, whenever that file run reads B and level 2 keeps it. The `source` is `File { sha256, bytes, header }` (CS8), not M4's planned `Catalog { id }`, which is dropped. The catalog's id is a label, as MF4 already said. Measured on 21 sockets: equal one for one (section 10.2).
- **CD3. Labels.** Each candidate has exactly one label, `catalog:<id>`, with the id in decimal. Labels stay outside the digest and the order (MF4). The part's stored name, if the store keeps one, appears in the table and not in the document.
- **CD4. What the document leaves out, stderr says.** One stderr line states what was searched and what was not. For example: `fitment: bracket.socket.json: catalog parts/ (generation 412): 100,083 indexed, 17 quarantined not searched: level 1 95, level 2 32: 32 candidates: 0 match, 11 interface match, 21 reject; fit not checked (no fit verifier in this build)`. The table's header line carries the same counts. This follows Soundness rule 5 (state what was not checked) without changing v1.
- **CD5. Order and identity.** MF5 orders by verdict, stage, failed checks, then SHA-256. Each SHA-256 appears once in a catalog (CS1), so the order is total. A source without a SHA (`CandidateSource::Given`) would tie on an empty key, and the threads' finishing order would leak into the document. CD2's `File` source is therefore required, not cosmetic (attack A10).
- **CD6. The digest.** A catalog run's digest equals the digest of a file run over exactly the same candidates' bytes, since labels are outside it. An audit run (QR10) over a catalog equals a file run over that catalog's files.
- **CD7. Exit codes are VD14's.** 0 `Match`, 4 `InterfaceMatch`, 5 when nothing passes: every candidate rejected, none admitted, a blocked socket, or an empty catalog. 1 for an error (a database error, an unreadable row, a fitment bug in `--audit`). 2 for usage (`--catalog` with candidate files, B9). 3 for refused input (the socket gate, or a stale or unknown catalog, RX2). `--require-fit` turns 4 into 5 as before.
- **CD8. The CLI.**

  ```text
  fitment match <SOCKET> (<CANDIDATE>... | --catalog <DIR>) [--jobs <N>]
                [--original <ASSEMBLY>] [--format table|json] [-o <OUT>] [--explain]
                [--require-fit] [--require-reviewed] [--clearance strict|functional]
                [--tolerance-mm <X>] [--tolerance-deg <Y>]
  ```

  `-o OUT` may not be inside the catalog directory (exit 2). The table's header names the catalog and its counts (CD4). Each candidate line shows the label and, when the store keeps one, the part's name, escaped by `printable()`.

## 8. Versions, staleness and re-indexing (RX)

What each stale thing can do:

| Stored thing | Written under | If older than the build | If newer | Effect on a query |
| --- | --- | --- | --- | --- |
| Features | `EXTRACTOR_VERSION` | Can pass a bad part: an old reading may state as known what new rules read as unknown (an end, an outline), and verification trusts known values. **A false positive** | Unknown fields or meanings | Refused (RX2, QR4, QR7) |
| Tokens and postings | `SIGNATURE_VERSION` | Can hide a good part: a changed bin width, diameter list or pair rule leaves no posting where level 1 now probes. **SL1 broken at catalog scale.** It cannot pass a bad part (IX9) | Same | Refused (RX2) |
| `part_digest` | `EXTRACTOR_VERSION` (it hashes a `Part`) | Wrong grouping, so a wrong judgment is copied | Same | Recomputed whenever features are re-read (RX4) |
| Quarantine | `EXTRACTOR_VERSION` | Hides a part the new gate would accept | — | Re-gated by `reindex` (RX4) |
| Nothing else | — | No verdict is stored (CS7); `MATCH_RULES_VERSION` needs nothing re-indexed | — | — |

- **RX1. Versioned writes.** A row's features carry `extractor_version`, and its tokens and postings carry `signature_version`. Any change to `bin`, `BIN_MM`, `IndexTokens::of`, the pair-token rule or `PartSignature::of` raises `SIGNATURE_VERSION` (ARCHITECTURE, Formats and versions). The golden test for tokens (CT12) fails while the version stands still.
- **RX2. No query on a stale catalog.** If any indexed row is at a version other than this build's, older or newer, the query is refused with exit 3: `catalog parts/: 1,204 parts were read by extractor 6 and this build is 7: run fitment catalog reindex`. Skipping stale rows would hide parts silently and make the result depend on how far a re-index had got. Judging them could pass a bad part.
- **RX3. A newer catalog is refused.** A catalog written by a newer fitment (a schema, extractor or signature version above the build's) is refused by every command except `list`, which reads only the store's own columns.
- **RX4. Re-indexing.**
  - Where only `SIGNATURE_VERSION` differs, `fitment catalog reindex` recomputes tokens, postings and nothing else from the stored features, without reading any STEP file.
  - Where `EXTRACTOR_VERSION` differs, it re-reads the blob, re-gates it and writes the features, the digest, the tokens and the postings. A part the new gate refuses moves to quarantine.
  - Every quarantined file is re-gated too, since a gate fix may accept it.
  - Each part is one transaction (CS6), so an interrupted re-index leaves some rows current and some stale. The query stays refused (RX2), and the next `reindex` continues with the stale rows only.
  - Measured: re-reading 10⁵ corpus parts took 4.9 s at 16 threads (0.26 ms a part). NIST-like parts take 7.9 ms each, so about 13 minutes of CPU at 10⁵, or about 1 minute at 16 threads (an estimate).
- **RX5. The generation.** Every write transaction increments `meta.generation`. Two read transactions that see the same generation see the same catalog. Run records and stderr name it (CD4), so a result can be reproduced against a stated state.
- **RX6. `catalog check`.** For every indexed row, the check recomputes the tokens, the postings and `part_digest` from the stored features and compares them with what is stored. `--deep` also re-hashes each blob and re-reads it, and requires the features document to be byte-identical; extraction is deterministic, so any difference is corruption or a bug. Exit 1 on any difference.
- **RX7. Defence in depth.** `judge_stored` refuses a non-current features document (QR7), whatever the catalog gate concluded.

## 9. Why the index never passes a bad part nor hides a good one

**Claim.** Fix a socket document S (accepted, current), a policy P, and a current catalog C (RX2) read at one generation. For every indexed part p of C with bytes B:

1. If `fitment match S B.stp` under P gives p `Match` or `InterfaceMatch`, the catalog run lists p with the identical entry, labels aside.
2. Every entry the catalog run lists is identical to that file run's entry for B. A catalog pass is therefore exactly a file-run pass, and M4's zero-false-positive gate carries over unchanged.

**Proof.**

1. **Same input.** The stored features document is `read(B)` under this build's extractor (CS2, RX2, QR7). It parses back to an equal `Part`, which is tested on every corpus, unit and fixture part (CT5). `judge_part` is a deterministic function of the `Part`; the `source` is copied (CS8). So `judge_stored(features(p), source(p)) = judge_step(B)` (measured: 105 of 105).
2. **Level 2 is the same function.** `Matcher::keeps` calls the same `shortlist` with the same socket signatures as `judge_part`'s own shortlist (QR5). M4's section 6.4 shows that level 2 keeps every part that verification passes. So a passing p survives QR5, and a part QR5 drops would have had stage `shortlist` and verdict `Reject` in the file run. That is its whole entry.
3. **Level 1 contains level 2, per part.** p's stored tokens are `IndexTokens::of(PartSignature::of(part))` at this build's version (CS3, RX2, checked by RX6). Level 1's diameter windows are level 2's (IX7); its bins cannot miss (SG10, IX3); its pair and plane tests use p's own lever, which is level 2's (IX5); incomplete pairs and wide angles bypass (IX7). So every part level 2 keeps, level 1 admits (measured at 10⁵: 20 sockets, digests equal with level 1 and without).
4. **Blocking passes nothing.** If a socket is blocked, every candidate fails it (SG7, SL8, IX8). Measured: 11 blocked sockets, 0 passes among 457 files each.
5. **Dedup is exact** (QR6), and the snapshot is one state (QR2).
6. **The document is assembled by the same code** (`MatchDocument::new`) from equal results, ordered by a total key (CD5).

What the argument assumes, and what checks each assumption:

- The catalog directory is trusted like the binary: someone who can write its database can store any features. RX6 `--deep` detects it but cannot prevent it.
- `SIGNATURE_VERSION` and `EXTRACTOR_VERSION` rise when they must; the golden tests (RX1) check this.
- M4's section 6.4 holds; KH9 and `--audit` check it (QR10).

## 10. Measured performance

### 10.1 The catalogs

`probe ingest` reads each base part, and writes each copy k > 0 as the same STEP text with its length unit replaced by a `CONVERSION_BASED_UNIT` of s mm. Every length, the uncertainty included, then scales by s exactly as a reader applies it. The scale s is log-uniform in [0.5, 2) from a hash of (base, k), rounded to 6 decimals. The 228 bases are the generated corpus's accepted parts (`tools/gen-corpus.py`, 463 files), less the 4 hostile ones. "Dup" catalogs instead change only the header's `FILE_NAME`, giving identical parts with different SHA-256s: the many-identical-parts worst case. "fx" catalogs are made the same way from the 24 accepted fixture part files (NIST, NIST-EDM, STEP Tools).

| Catalog | Parts | Postings | Ingest wall (16 threads) | Read a part (CPU) | Database | Of which features / tokens |
| --- | --- | --- | --- | --- | --- | --- |
| c3, scaled | 1,140 | 2,515 | 0.11 s | 0.28 ms | 35 MB (pretty) | 33 / 0.6 MB |
| c4, scaled | 10,032 | — | — | — | 313 MB (pretty) | — |
| c5, scaled | 100,083 | 220,800 | 9.8 s | 0.23 ms | 3.12 GB (pretty) | 2.94 GB / 55 MB |
| c5c, scaled, compact | 100,083 | 220,800 | 4.9 s | 0.26 ms | **1.32 GB** | 1.16 GB / 55 MB |
| d5, dup | 100,092 | 220,817 | 8.3 s | 0.23 ms | 3.01 GB (pretty) | 2.84 GB / 46 MB |
| fx4, NIST-like dup, compact | 7,928 | 193,984 | 16.6 s | 7.9 ms | 1.53 GB | 1.18 GB / 337 MB |

For corpus-sized parts, the single writer bounds ingest: about 0.05 ms a part with compact JSON, 0.1 ms with pretty. For NIST-like parts, reading the STEP file does, at 7.9 ms of CPU each.

### 10.2 The catalog run equals the file run

`compare.py` checks each socket's file run (`fitment match S bases/ --format json`, 457 files: 228 parts and 229 refused files, mostly assemblies) against its catalog run over c1 (the 228 parts, once each). It requires the following:

- the envelope is equal;
- the catalog lists exactly the file run's accepted candidates that level 2 kept;
- each entry is equal, labels aside, and in the same order;
- no level-2 drop passes.

All 21 unblocked sockets passed, and the 11 blocked sockets passed nothing in the file run (`out/equivalence.txt`). Level 1 bypassed against level 1 used, on c5 (10⁵): the same digest for all 20 unblocked single-socket and multi-socket documents tried, with the catalog's largest lever and again with each part's own (`out/level1-sound-1e5.txt`, `out/level1-own-lever-1e5.txt`).

### 10.3 Query cost

The measurements used SQL level 1 with M4's global lever (before IX5), compact features not yet used, and default policy. "L1/L2" is level-1 hits / level-2 kept. Times are the whole query in ms, with 1 and 16 threads (`out/bench.txt`).

| Socket | 10³: L1/L2, 1 t, 16 t | 10⁴: L1/L2, 1 t, 16 t | 10⁵: L1/L2, 1 t, 16 t |
| --- | --- | --- | --- |
| s01 BRACKET (4 bores, 2 seats, grip) | 36/21, 1,172, 213 | 46/23, 1,123, 167 | 177/32, 1,182, 225 |
| s01 NUT | 21/17, 27, 12 | 44/32, 74, 23 | 345/271, 656, 114 |
| s02 BLOCK (one line) | 208/21, 207, 38 | 992/22, 246, 49 | 8,844/36, 713, 185 |
| s05 WASHER (2 sockets) | 85/1, 15, 13 | 608/1, 46, 24 | 6,061/2, 379, 161 |
| s07b BRACKET (2 sockets) | 35/20, 2,187, 310 | 36/21, 2,174, 322 | 56/28, 2,172, 410 |
| s19 HOUSING | 13/12, 62, 14 | 92/84, 386, 64 | 748/650, 2,848, 343 |
| s03 PLATE | 2/2, 24, 27 | 2/2, 28, 30 | 3/2, 47, 50 |

- **Where the time goes.** Judging the survivors dominates every query over 50 ms: 84 ms of CPU per bracket against s01 (M4's coverage marching), and up to 205 ms for one s07b candidate. Load and parse cost 0.13 ms a hit with pretty JSON. Level 2 costs about 6 µs a hit.
- **Level 1 before IX5.** It took 19 to 82 ms at 10⁵, with a floor of 20 to 25 ms: `SELECT MAX(lever_mm)` scanned 10⁵ rows (24.5 ms measured alone).
- **Level 1 after IX5** (`out/level1-bucket-lever-1e5.txt`), at 10⁵ and 16 threads:

  | Socket | Level-1 hits | Level 1 | Whole query |
  | --- | --- | --- | --- |
  | s02 SCREW | 31 | 0.22 ms | — |
  | s01 BRACKET | 95 (was 177) | 33 ms | 196 ms |
  | s01 PLATE | 99 (was 177) | 31 ms | 92 ms |
  | s02 BLOCK | 8,844 | 31 ms | 162 ms |
  | s05 WASHER | 6,061 | 43 ms | 122 ms |
- **A level-1 index held in memory instead of SQL.** Loading every token takes 2 ms at 10³, 16 ms at 10⁴ and 153 ms at 10⁵. This was 1.5 to 3.6 times slower per query than SQL at 10⁵ and gained nothing below. SQL is the default; a long-lived server could cache the index (M6's decision).
- **Bypassing level 1** (level 2 on every part at 10⁵) takes 0.97 to 1.23 s of wall time at 16 threads and about 10 s of CPU, mostly loading and parsing features (9.6 s for the 98,766 parts of the seats-only run). Level 1 saves about that much on every query. It is an optimization, which section 9 shows to be sound.

### 10.4 The worst cases

| Case | Measured | With the fix proposed |
| --- | --- | --- |
| 10⁵ exact copies, s01 BRACKET | L1 15,365, L2 9,219 (4,829 interface matches): 48.5 s at 16 threads, 771 s of CPU; document 111 MB | Dedup (QR6): 21 distinct parts judged, 1.76 s, byte-identical document |
| 10⁵ exact copies, s02 BLOCK | L1 54,875, L2 9,219: 8.6 s | Dedup: 1.84 s, byte-identical |
| Seats-only socket (s01 bracket's 4 bores removed in review), c5 | L1 98,766 (planes filter only), L2 202: 2.2 s wall, 10 s CPU in load and parse | Plane tokens (B5) would admit about 5.8% at level 1 (measured at 10⁴) |
| Seats-only, 10⁵ exact copies | L2 11,853: 86 s wall, 1,263 s CPU | Dedup brings this to the number of distinct bases that fit (not measured) |
| Hostile parts in the catalog (`h_*`, up to 80.8 MB of features) | s01 BRACKET: 3.26 s, of which 3.17 s is `h_hole_grid`'s judgment (M4's bounds) | None needed: bounded per candidate (QR9) |
| NIST-like parts, 7,928 copies | Level 1 alone 255 to 262 ms (1,668 hits parse 42 KB of tokens each) | Compact pair tokens (B4) |

**Conclusion.** At 10⁵ parts the index and the database are not the cost. Judging the parts that really match is, and so is writing them all out. Dedup removes exact duplicates. Distinct parts that all fit cost M4's per-candidate time, about 10 to 100 ms each, divided by the thread count (section 13).

## 11. Changes to fitment-core (PR 1)

All of these are additive; core stays free of I/O and threads.

```rust
// matching/index.rs
pub trait IndexSource { /* IX1 */ }
pub fn level_one<S: IndexSource>(socket: &SocketSignature, source: &S)
    -> Result<Result<Vec<u64>, Blocked>, S::Error>;          // SL8, IX4 to IX8
impl IndexSource for ShortlistIndex { /* the in-memory postings */ }
impl ShortlistIndex { pub fn query(&self, s: &SocketSignature) -> Result<Vec<u64>, Blocked>; } // = level_one
#[derive(Serialize, Deserialize)] pub struct IndexTokens { /* as M4; pairs compact under B4 */ }
#[derive(Serialize, Deserialize)] pub struct PairToken { pub a: u32, pub b: u32, pub distance_mm: f64 } // B4
pub struct Posting { pub sense: Sense, pub bin: i64, pub stack: u32, pub diameter_mm: f64 }
impl IndexTokens { pub fn postings(&self) -> impl Iterator<Item = Posting> + '_; }   // IX2

// matching/matcher.rs
impl Matcher<'_> {
    pub fn admitted<S: IndexSource>(&self, source: &S)
        -> Result<Admitted, S::Error>;                       // QR3: every socket, intersected
    pub fn keeps(&self, part: &PartSignature) -> bool;       // QR5: level 2, every socket
    pub fn judge_stored(&self, features: &FeaturesDocument, source: Source)
        -> Result<CandidateResult, NotCurrent>;              // QR7, RX7
}
pub enum Admitted { Parts(Vec<u64>), Blocked { socket: String, why: Blocked } }

// features/document.rs, assembly/parts.rs
impl FeaturesDocument { pub fn part_digest(&self) -> Option<String>; }   // CS4
impl Source { pub fn new(sha256: String, bytes: u64, header: Option<FileHeader>) -> Self; } // CS8
```

`CandidateSource::Catalog { id }`, planned in M4's section 12 and never built, is not added (CD2). Under B4, `SIGNATURE_VERSION` rises to 2 in this PR, and the match goldens are re-blessed under the risen version. `MATCH_RULES_VERSION` does not change, because no file run's output changes: level 1 is never used in a file run.

## 12. PR plan

Each PR comes with its tests and two adversarial reviews, as in M4.

1. **Core: level one over any source.** Everything in section 11, with the per-part lever (IX5) and its buckets. Tests:
   - CT3 in core, against the in-memory source: level 1 contains level 2 for every corpus socket and part, the independence test, and a proptest on random tokens and windows;
   - CT5 for the features round trip and `judge_stored` against `judge_step`, on the committed candidates and unit set;
   - token goldens.
2. **`fitment-catalog`: tables and ingest.** The schema of section 4.2 inside the store design's schema and its migrations table. `features_for(bytes)` and the one-transaction write (CS6), quarantine rows, `meta.generation`, and `catalog check` (RX6). Together with the store and CRUD PRs, which other designers plan, so the CLI's `catalog add/list/get/remove` lands here or beside it. Tests: CT4, CT12, and ingest of the committed candidates.
3. **The query and `fitment match --catalog`.** `SqlSource`, the snapshot (QR2), load, level 2, dedup, `--jobs`, CD1 to CD8, the stderr line, and the hidden `--audit`. Tests: CT1, CT2, CT6, CT7, CT9.
4. **Versions and re-indexing.** The catalog gate (QR1, RX2, RX3), `catalog reindex` with both paths (RX4), and quarantine re-gating. Tests: CT8 and CT10.
5. **Scale and exit.** A `tools/scale-step.py` (the unit edit of section 10.1, no kernel needed) or a hidden `fitment catalog add --scaled` for tests. CT11 as a weekly or manual job. ARCHITECTURE.md gains "Matching from a catalog". PLAN's M5 row gets the exit numbers. The coverage registry gains the CS, IX, QR, CD and RX rules.

## 13. Test plan (CT)

- **CT1. The M4 corpus through the catalog.** The corpus test runs, under `default` and `functional`, for every match-case socket:
  - ingest of every corpus file into a fresh catalog, with assemblies and refused files quarantined;
  - `fitment match S --catalog DIR --format json` against `fitment match S <the same files> --format json`, checked as `compare.py` does (section 10.2);
  - KH2's strict gate applied to the catalog run. Every case whose file run passes appears in the catalog run with the same entry. Every bad case is either absent (a level-2 drop, which is a `Reject` in the file run) or rejected for its injected defect with the same evidence. Every blocked socket has no candidate and exits 5.

  The 10 component cases (`m_s11_bracket_own` and so on) are listed as not run through the catalog (B12), with that reason, so the coverage test sees them. The test also checks that the false-negative baseline (KH4) is unchanged by the catalog path: a catalog run can only omit level-2 drops, which are rejects already.
- **CT2. Audit equals the file run.** On the corpus catalog, `--audit` gives the same digest as a file run over the catalog's files, for every socket and both policies.
- **CT3. The two level-1 sources agree.**
  - `level_one` over `SqlSource` equals `level_one` over `ShortlistIndex`, id for id, for every corpus socket against the corpus catalog and a scaled 10³ catalog.
  - For every (socket, part) pair of the corpus, level 2 keeping implies level 1 admitting (SL9 per part; at present measured only in aggregate).
  - The independence test (IX6): adding 1,000 scaled parts, one with a 5 m lever, leaves every corpus part's admission unchanged.
  - A proptest on random tokens and random socket windows, unbounded ends included.
- **CT4. Floats and bins in SQLite.** Diameters at k · 0.1 mm ± 1 ulp, subnormal-free extremes (1e-6 to 2e6 mm), and the windows' unbounded ends round-trip bit-equal through `posting.diameter_mm`. The probe returns them, and `Window::holds` decides as in memory.
- **CT5. Stored features judge as files do.**
  - Every unit-set, committed-candidate, corpus and fixture part: `from_json(compact) == read` and `to_json` is byte-equal. Measured: 232 of 232 corpus parts.
  - `judge_stored == judge_step` for every file candidate. Measured: 105 of 105.
- **CT6. A match reads only the index.** After ingest, the blob directory is renamed away, and every CT1 query gives byte-identical documents. This is the M5 exit criterion as a test.
- **CT7. Determinism.**
  - `--jobs` 1, 2 and 16 give identical bytes, and so do dedup on and off.
  - Two catalogs ingested in permuted orders give equal digests and differ only in labels.
  - The three-OS job compares corpus catalog runs' digests, as KH11 does for file runs.
- **CT8. Staleness.**
  - A test edits one row's `extractor_version` (and, separately, its `signature_version`) down by one, and up by one. Each time the query exits 3 with the count and the command; `reindex` restores it; and a re-index interrupted after half the rows is still refused.
  - A newer `schema_version` is refused by every command but `list`.
  - `judge_stored` refuses a non-current document even when called directly.
- **CT9. The snapshot.** A test hook replaces a part between level 1 and loading. The document then equals either the before state or the after state, never a mix. Built with two connections and WAL.
- **CT10. Hostile parts in a catalog.** The four `h_*` parts are ingested, every corpus socket is queried, and each query stays under 30 s (M4's hostile bound). The `h_bracket_stubs` features (80.8 MB pretty) load. A catalog whose only part is `h_hole_grid` returns its verdict within M4's bounds.
- **CT11. Scale** (not in the default `cargo test`; weekly CI or manual):
  - a 10⁴ scaled catalog (about 4 s to build);
  - every corpus socket with level 1 and with it bypassed, compared by digest;
  - the timings in section 10.3 reported and not gated, except a loose ceiling (a query under 10 s at 16 threads on 10⁴) that catches an order-of-magnitude regression.
- **CT12. Consistency.** `catalog check` passes on every test catalog, and fails on a hand-edited posting, token, digest or features row. A golden of `IndexTokens::of` for the committed candidates fails while `SIGNATURE_VERSION` stands still (RX1).
- **CT13. Coverage.** The CS, IX, QR, CD and RX rules join the RULES registry and the coverage test, each with a test or a reason.

### 13.1 Rules and the tests that hold them

| Rule | Held by |
| --- | --- |
| CS1 one row per SHA; quarantine | CT1 (refused corpus files quarantined); an ingest test adding the same bytes twice |
| CS2 features keyed by the bytes read | CT5, CT12 (`--deep`), A7's hand-edited row |
| CS3 derived tokens | CT12, the tokens golden |
| CS4 part digest | CT7 (dedup on and off), CT12 |
| CS5, RX1 to RX4 versions and re-indexing | CT8 |
| CS6 one transaction | CT9; a crash-injection test that kills the writer between statements |
| CS7 nothing else stored | Review; the schema test lists every column |
| CS8 the source from storage | CT1's equal entries |
| IX1 one algorithm | CT3 (the two sources agree) |
| IX2, IX3 bins and exact comparison | CT4; core's existing bin test |
| IX4 narrowest first | CT3 (same ids with and without the ordering) |
| IX5, IX6 the part's own lever, independence | CT3's independence test; CT11 |
| IX7 what is admitted | CT3's per-pair test (level 2 keeps implies level 1 admits) |
| IX8 blocked | CT1 (11 blocked corpus sockets, exit 5) |
| IX9 tokens never pass | CT12 (an edited token can only change which parts are admitted) |
| QR1 the catalog gate | CT8 |
| QR2 one snapshot | CT9 |
| QR3, QR5 the matcher's own signatures | CT1 under `--tolerance-mm 0.05` as well as the default |
| QR4 bad rows fail | A test with a truncated features row: exit 1 naming the part |
| QR6 dedup | CT7; with a fake fit verifier naming a kernel, a test that the fit is asked per member |
| QR7, RX7 `judge_stored` refuses stale documents | CT8 |
| QR8 parallel output | CT7 |
| QR9 budgets | CT10 |
| QR10 audit | CT2 |
| QR11 only the index | CT6 |
| QR12 the original | CT1 with `--original` on s01 |
| CD1 to CD6 the document | CT1, CT2, CT7, KH12's structural checker on catalog documents |
| CD7, CD8 exit codes and the CLI | CLI tests: `--catalog` with files (2), stale (3), empty (5), blocked (5), unreadable row (1) |

## 14. Risks

- **Many parts that really fit are expensive.** Every level-2 survivor is aligned and verified, at 10 to 200 ms each. A catalog of 10⁴ distinct brackets that all fit s01's socket costs about 14 minutes of CPU and, at 16 threads, about a minute. Dedup helps only with identical parts. A ranked, limited query needs a ranking v1 lacks (B10). This is the main cost risk of M5.
- **Large documents.** 9,219 candidates made 111 MB. M4 already noted 8 MB for 100 candidates on a large socket. M6 will want pagination; the CLI's table should summarize past a few hundred lines.
- **Storage for real parts.** Real parts' features are about 13 times larger than generated ones, which puts 10⁵ parts at an estimated 15 to 20 GB without compression (B3).
- **Weak level 1 on single-line and seats-only sockets.** At 10⁵, level 1 admits 8,844 parts for s02's block and 98,766 for a seats-only socket. Level 2 corrects this at about 1 to 10 s of CPU. Plane tokens (B5) are the remedy if real catalogs need one.
- **Upgrades block queries until a re-index finishes** (B6). For 10⁵ real parts that is estimated at about a minute at 16 threads, or about 13 minutes on one core.
- **Trust in the catalog directory.** Stored features decide verdicts without the STEP file being read again. Corruption or tampering is caught by `catalog check --deep`, not by the query (section 9).
- **Two kinds of judgment.** File runs list level-2 drops with their evidence; catalog runs do not. A bug in level 2 hides parts silently in a catalog run, and only `--audit`, CT1 and CT2 see it.
- **M7.** The fit verifier needs each passing candidate's STEP bytes, read from the blob store: the one sanctioned read outside the index, limited to candidates whose interface passed. Dedup must stop at the interface (QR6).
- **Measured on scaled generated parts.** The catalog's real distribution (standard hole sizes repeated across thousands of parts) is unknown until real catalogs arrive (D8, D9). The fixtures' 24 parts are the only foreign sample here, and they were used only for sizes.
- **The prototype's level 1 was a copy of M4's code**, refactored onto the trait in a scratch crate. PR 1 must re-derive it from the repository's current `index.rs`, which has moved since the snapshot (`Blocked::FailsEvery` became `#[non_exhaustive]`).

## 15. Attacks on this design and their fixes

Each attack asks how the index could pass a bad part or hide a good one, or how the catalog run could differ from the file run.

- **A1. The SQL and core disagree.** If the catalog re-implemented SL8 in SQL, a mistake in a window, a bin or a join would hide parts. *Fix:* IX1. Core's `level_one` is the only implementation; SQL fetches postings by an integer range and nothing else; CT3 compares the two sources.
- **A2. The catalog's largest lever couples parts** (found by measurement). M4's index widens every query's pair and seat windows to the largest lever in the catalog. That was sound, but it made each admission depend on the other parts and cost a 25 ms scan. *Fix:* IX5 and IX6, each part at its own lever (level 2's), with a proof and CT3's independence test.
- **A3. Per-part seat groups are slow** (found by measurement: 286 ms for 177 hits). *Fix:* round the lever up to a power of two. This is sound because seat groups only coarsen with the lever, so no more planes are asked than level 2 asks. It took 33 ms.
- **A4. Stale tokens hide a good part.** For example, a new `BIN_MM` leaves postings in old bins. *Fix:* RX1 and RX2, under which a mismatched `signature_version` refuses the query, and RX4's tokens-only re-index. Skipping stale rows was rejected because it hides parts silently.
- **A5. Stale features pass a bad part.** A Part read by an older extractor may state as known what newer rules read as unknown, and verification trusts known values: a false positive. *Fix:* RX2 at the gate and RX7 in `judge_stored`; never judge a non-current document.
- **A6. Evidence that lies about its reader.** `Matcher::judge` writes `read.extractor_version` from the build's constant, not from the document. A stale document judged anyway would claim the current extractor. *Fix:* `judge_stored` refuses a non-current document (QR7), so the field is always true.
- **A7. Features that belong to other bytes.** If a replace updated the SHA but not the features, a bad file X would be judged by good features Y and reported under X's SHA: a false positive. *Fix:* the following rules, with `catalog check --deep` to catch the rest:
  - features are keyed by the SHA of the bytes read, in the same in-memory buffer (CS2);
  - a replace is a delete and an insert, never an update in place (CS6).
- **A8. Threads see different snapshots.** Workers with their own connections could read a part replaced after level 1. *Fix:* QR2, one reader and one read transaction, with workers that never touch the database. CT9 tests it. Comparing the generation across connections was the alternative; one reader is simpler.
- **A9. Dedup is wrong when the fit depends on the file.** Two files with equal features (an `other` face is recorded by type, not by surface) can have different bodies. *Fix:* QR6 dedups only while no fit verifier names a kernel. M7 asks the fit per member.
- **A10. Ties in the order.** A catalog result with `CandidateSource::Given` has an empty identity key under MF5, so equal verdicts would keep the threads' finishing order: nondeterministic output. *Fix:* CD2 and CD5. Catalog results carry the file's source and SHA-256, unique per catalog (CS1).
- **A11. A blocked socket in a multi-socket document.** If only the blocked socket were skipped, the other sockets' hits would be judged and all rejected; that is wasted work, though harmless. *Fix:* IX8 blocks the whole query, since VD2 rejects every candidate.
- **A12. Level 2 in the catalog uses another policy's signatures.** The prototype first built socket signatures separately with the default policy. Under `--tolerance-mm` that would drop parts verification passes. *Fix:* `Matcher::keeps` and `Matcher::admitted` use the matcher's own signatures (QR3, QR5); the catalog builds none.
- **A13. The catalog run loses KH9's runtime check.** M4 checks every explicit drop by verifying anyway; a catalog run cannot afford to. *Fix:* `--audit` (QR10) restores explicit semantics for tests, with CT1 and CT2 on every corpus socket, and CT11 at scale.
- **A14. A pair token's first stack holds the window only through a neighbour.** SL2 lets a witness take its diameter from a coaxial neighbour, while the pair filter tests the token's own two stacks. *Analysis:* the stack verification checked the narrowest window's span on is itself within ε of the line (w3). The token (that stack, the other line's corresponding stack) exists, and its distance lies within the reach. Sound as written. CT3's per-pair test (level 2 keeps implies level 1 admits) covers it on the corpus.
- **A15. Quarantine hides a part forever** after a gate fix. *Fix:* RX4 re-gates quarantined files on every extractor rise.
- **A16. A row that does not parse is skipped.** Skipping would make a hidden part depend on corruption. *Fix:* QR4. The query fails with exit 1 and names the part, and `catalog check` finds the row.
- **A17. A huge part slows every query.** `h_bracket_stubs` has 80.8 MB of features. *Analysis:* it is loaded only when admitted. When it is, M4's bounds hold (QR9; measured: 3.2 s for the worst). Bounding a features document's size at ingest is the store design's choice; a cap would be a quarantine reason, not a silent drop.
- **A18. Floats through SQLite.** SQLite turns NaN into NULL and may return −0.0 as +0.0. *Analysis:* stored diameters are finite and positive (`geom::finite`, the gate), and the exact comparison is in core (IX3). CT4 pins it.
- **A19. An integer id wraps.** SQLite ids are `i64`, core's are `u64`. *Fix:* convert with `u64::try_from` and treat an error as a catalog error. The prototype used `as`, and the product must not.

## 16. Open questions, with defaults

1. Should a stored match run record the catalog generation and the counts of CD4? Default: yes, in whatever run record the store design keeps; not in the document (B8).
2. Should the server (M6) hold the level-1 index in memory? Default: M6's call. A query from memory was 1.5 to 3.6 times slower than from SQL at 10⁵ when the index was built per query, and memory needs invalidation on every write.
3. Should `--catalog` accept several catalogs? Default: no. Ids are per catalog, and the labels would need a catalog name.
4. Should the probe of a very wide window (D6 functional's unbounded clearance) be replaced by a check against the tokens of parts already admitted? Default: only if measured slow on real catalogs. The result is the same either way (IX4).
5. Should the table offer `--summary` for runs with thousands of passes? Default: the table prints the first 200 candidates and a count, and the JSON holds all. Coordinate with the CLI design.

## Evidence

Under this directory (`docs/research/m5-evidence/B/`), not committed:

- `proto/`: a scratch workspace holding copies of `fitment-core` and `fitment` from the snapshot, with `IndexSource`, `query_source`, `Deserialize` on the tokens and the per-part lever added to `fitment-core/src/matching/index.rs`, plus `probe/` (`probe/src/main.rs`, about 770 lines). The subcommands:
  - `ingest` (scaled or `dup` copies, 16 threads);
  - `query` (SQL or `mem` level 1, or `all` to bypass it);
  - `dedup`;
  - `roundtrip`;
  - `sigsize`;
  - `scale`.
- `corpus/`: the corpus generated by `tools/gen-corpus.py` from the worktree (463 files, 81 MB, 3 min 47 s); `bases/` and `fx/` are symlinks to the parts used.
- `sockets/`: the 32 match-case sockets, written by the snapshot's `fitment socket`.
- `compare.py` (the equality check of section 10.2), `bench.sh` (section 10.3), `planes_probe.py` (B5's selectivity).
- `out/`: `roundtrip*.txt`, `equivalence.txt`, `query-1e5-sql.txt`, `bench.txt`, `level1-sound-1e5.txt`, `level1-own-lever-1e5.txt`, `level1-bucket-lever-1e5.txt`, `query-dup-1e5.txt`, `dedup.txt`, `query-seats-only.txt`, `query-hostile.txt`, `query-fx-1e4.txt`, `ingest-*.txt`, `sizes.txt`, `plane-tokens.txt`, `determinism.txt`, `seats_only.json`.
- `db/`: the catalogs (about 10 GB in all); safe to delete.
