# fitment M5, designer A: the catalog's store and its integrity

As of 2026-10-16. Status: proposal for review. Measured on fitment `a42853c` (branch `m4/exit`, `EXTRACTOR_VERSION` 6, `SIGNATURE_VERSION` 1, `MATCH_RULES_VERSION` 1), rusqlite 0.40.2 with bundled SQLite 3.53.2, Rust 1.99.1 (and a build check on 1.85), macOS on APFS. The experiments are in `exp/` beside this file (section 13). Nothing in the repository was changed.

This design covers the store and its integrity. It does not cover the query's ranking, the CLI's table layout or D10's property mapping. Where those meet the store, it states what the store gives them.

## 1. Summary

- **Two stores, one source of truth.**
  - The files are the original STEP bytes, named by their SHA-256. They are immutable and written with a temp file, fsync and rename.
  - One SQLite database (WAL, STRICT tables) holds everything else: entries and their revisions, every derived document, the level-1 tokens and postings, a logical clock and an operation log.
- **A file's blob becomes durable before the commit that names it.** Blob files are unlinked only while the SQLite write lock is held, after a re-check that nothing references them. These two rules are the whole crash- and race-safety argument for the file store. The measurements:
  - **kill -9:** 300 rounds of kill -9 mid-ingest never broke an invariant. A control that committed the row first broke in round 0.
  - **Race:** a 20 s race of four ingest/delete processes against a gc loop found 0 rows without a blob in 5,120 checks. A control that unlinked after the commit found 6.
- **Derived data is keyed by content and version, never by entry.**
  - `features` is keyed by `(sha256, extractor_version)`; `tokens` and `posting` by `(sha256, extractor_version, signature_version)`. Rows are insert-only.
  - A version rise therefore adds rows beside the old ones. Re-indexing is resumable, two fitment versions can share a catalog, and staleness is one SQL count.
- **The soundness-critical artifact is the stored features document.** A corrupt or stale token can only drop a candidate, because level 1 is a superset and everything after it is recomputed from the document.
  - **Byte-exact:** the document is stored as the exact bytes of `fitment features`, with its SHA-256, and checked on every load.
  - **Current only:** it is judged only when it was written by this build's extractor.
  - **One judging path:** it is judged through the same core function `judge_step` uses.
  - **Measured:** 243 of 243 accepted parts round-trip byte for byte. 90 of 90 corpus match cases give an identical `CandidateResult` and match-document digest from the stored document and from the STEP file.
- **"A match reads only the index" is proved three ways:**
  - **By type:** the match takes a `Snapshot`, which has no access to blobs.
  - **By a counting file-system seam:** zero blob opens during a match.
  - **By an exit test:** with the `blobs/` directory moved away, `fitment catalog match` gives the same digest as `fitment match` on the same files.
- **Six PRs.** The first is additive core API (`judge_document`, deserializable `IndexTokens`, a level-1 query over an external posting source). Then store and ingest; delete, replace and gc; the index and `catalog match`; re-index; release.

## 2. Decisions for review

Each row has a default; the rest of the document assumes the defaults. **Your call** marks rows that change what fitment promises or stores.

| # | Question | Default | Cost or gain |
| --- | --- | --- | --- |
| M5A-1 | **Your call.** Entry identity | An integer entry id, never reused (`e17`). Each entry points to its current revision, a blob by SHA-256. Content can be live in at most one entry. | Replace keeps the id; a re-ingested deleted file gets a new id. The alternative, the SHA-256 alone, makes "replace" meaningless. |
| M5A-2 | **Your call.** What delete does | A tombstone: the entry and revision rows stay (id, part number, SHA-256, seqs). The derived rows and the blob row are removed in the delete's transaction; the file is unlinked by an inline gc after the commit. | Ids are never reused and old match reports stay readable. A deleted part cannot be re-judged (no blob); `--keep-file` is not offered in M5. |
| M5A-3 | **Your call.** Replace with a file the gate refuses | The replace is refused (exit 3) and nothing changes. `--quarantine` applies it anyway: the entry goes to quarantine and leaves the index. | The old model stays live until the user decides. Silently keeping it after a refused update is the other risk, and the user sees the refusal either way. |
| M5A-4 | **Your call.** A catalog match while any live content is stale (RX2) | Refused, exit 3, naming the count and `fitment catalog reindex`. `--allow-stale` judges the current rows only and says how many were skipped. | Never silently incomplete. An upgrade costs one `reindex`: extraction measured at 0.8 ms a file on average (21 ms at most) plus reading the blob, or about 0.1 ms a part when only the signature changed. |
| M5A-5 | Quarantine | Keep the blob and the refused `features.v1` document (status, refusals, header). Never tokens or postings. | Re-indexing re-gates quarantined files, so a gate fix can admit them, and the reason (rule 6) is the same JSON `fitment features` prints. |
| M5A-6 | Size cap | 128 MiB per file (`--max-bytes`). Larger files are refused at the door and nothing is stored. | stepq's graph is about 10× the file, so the peak is about 1.3 GB. The largest fixture is 7.4 MB. |
| M5A-7 | Durability | Blob `sync_all` (F_FULLFSYNC on Apple), one directory sync per batch, WAL with `synchronous=FULL`, and `fullfsync=ON` on Apple | About 3.3 ms per blob and 3.7 ms per commit on this Mac; commits are batched. A committed delete survives power loss, so a withdrawn part cannot come back. |
| M5A-8 | Where the catalog is | `--catalog DIR` or `FITMENT_CATALOG` only; no implicit default | A catalog is never created by accident in the working directory. |
| M5A-9 | Stored assemblies, sockets, reports | Deferred to M6, with the job table. M5 stores catalog parts only; socket documents stay files the user passes. | Keeps M5 to its exit. Section 8.6 says how stored sockets will go stale. |
| M5A-10 | Where features documents live | In SQLite, not as derived files | One snapshot covers a whole match. A concurrent delete can never pull a document from under a running query. |
| M5A-11 | Compression | None in M5. An `encoding` column is reserved (`json` only). | 62 KB a part on average, 6.2 GB at 100,000 parts. zstd can come later without a schema change. |
| M5A-12 | Ingesting bytes that are already live | Idempotent: it reports the existing entry and exits 0 | The unique index on live content decides it, even between processes. |
| M5A-13 | Level-1 storage | Per-part `IndexTokens` (canonical JSON) and a `posting` table by sense and bin. The exact window test is core's; core runs the query over the catalog's postings (PR 1). | At 100,000 parts, one posting range takes 13.7 ms, against 1.73 s to load every token. |
| M5A-14 | Hash | Core's `sha256_hex`; no `sha2` dependency | 327 MB/s, enough: hashing a 7.4 MB file takes 23 ms. Store keys and match documents use one implementation. |
| M5A-15 | Shortlist drops in a catalog match (M4-17 says "the drop ends the candidate") | M5 judges every level-1 survivor with the explicit semantics, so each candidate entry is byte-identical to `fitment match`'s. The early end waits until it is measured to matter. | The exit test becomes a digest comparison; costs a few ms per level-2 reject. |
| M5A-16 | Extractor canary | Three small probe parts (about 30 KB, ours, MIT) embedded in `fitment-catalog`. Their documents' SHA-256 is recorded per `EXTRACTOR_VERSION`, and a mismatch refuses ingest, reindex and match. | Catches an extractor that changed without a version rise (a dev build, a forgotten bump). It ships STEP bytes in a package, an exception to the unit-set rule. |

## 3. What M5 inherits

- **The canonical gate,** M2's `FeaturesDocument::read`, unchanged: the catalog stores its output (soundness rule 6).
- **The signature, policy-free and stored once** (M4-15, CF10). It is recomputed when `EXTRACTOR_VERSION` or `SIGNATURE_VERSION` rises.
- **Level 1** (SG10, SG11, SL7 to SL9). Tokens per part; bins of 0.1 mm looked up with a bin either side; windows that contain level 2's at the catalog's largest lever.
  - Level 1 is a superset, so a level-1 fault can only drop a candidate (a false negative), never pass one: whatever level 1 returns is re-shortlisted at level 2, aligned and verified from the document.
- **Blocked sockets** (M4-16, SL8). A query on a socket with an active requirement that fails every candidate returns nothing and says why.
- **Labels** (MF4): the catalog's id. They stay outside the digest and the order.
- **Stale sockets** are refused by `Matcher::new` (M4-22, VD9), so the catalog needs no rule of its own for socket inputs in M5.
- **No reader for `fitment.match`** (M4-23). The catalog never stores or reuses a verdict (RX7).
- **The release** (workspace layout §3): `fitment-catalog` is a sync library, published as internal and unstable, behind the CLI's default `catalog` feature, with rusqlite bundled (D5).

## 4. Design in one picture

```text
fitment catalog ingest a.stp dir/          fitment catalog match socket.json
  │ read bytes once (≤ cap), SHA-256          │ BEGIN (read snapshot, seq S)
  │ FeaturesDocument::read  (gate)            │ stale count = 0? canary ok? (RX3, RX9)
  │ tokens = IndexTokens::of(signature)       │ Matcher::new(socket)  (stale socket: exit 3)
  │ blob: tmp → sync → rename → dir sync      │ level 1: posting ranges + tokens → SHA-256s
  │ BEGIN IMMEDIATE                           │ per survivor, in SHA-256 order:
  │   blob present? (else rewrite)            │   doc row → sha check → from_json → current?
  │   INSERT blob, entry, revision,           │   judge_document(doc, Source from row)
  │   features, tokens, postings, op          │ MatchDocument (MF5 order, labels e<id>)
  │ COMMIT                                    │ COMMIT (end snapshot) — no blob opened
  ▼                                           ▼
<catalog>/catalog.db  ◄──────────── reads ───────── only this
<catalog>/blobs/sha256/ab/ab12…   (read by ingest, reindex, verify, get --file; never by match)
```

## 5. On-disk layout

```text
<catalog>/                         --catalog DIR or FITMENT_CATALOG (M5A-8)
  FITMENT-CATALOG                  marker: "fitment.catalog 1\n"; written last by init
  catalog.db                       SQLite, WAL; catalog.db-wal and catalog.db-shm beside it
  blobs/
    sha256/
      00/ … ff/                    256 fan-out directories, created by init
        ab12…ef                    64 lower-case hex digits; the exact original bytes; mode 0444
  tmp/
    <pid>-<n>.part                 in-flight blob writes; never read back; gc removes ones older than 1 h
```

- **Two fan-out levels:** `sha256/` names the algorithm, so a later hash is a sibling directory, and `ab/` keeps directories small. At 100,000 blobs that is about 390 files a directory.
- **No user-supplied text becomes a path.** Every path below the root is built from validated hex (PH1).
- **Lower-case hex only,** so case-insensitive file systems (APFS default, NTFS) cannot collide two names.
- **Portable between operating systems.** Documents are byte-identical on Linux, macOS and Windows (ARCHITECTURE, Determinism), and SQLite files are portable, so a catalog copied between them stays valid.

## 6. The schema (SQLite through rusqlite, D5)

```sql
PRAGMA application_id = 1718449219;   -- 0x666D7443, 'fmtC'
PRAGMA user_version = 1;              -- the schema version (DB5)
-- per connection: journal_mode=WAL, synchronous=FULL, foreign_keys=ON,
-- fullfsync=ON and checkpoint_fullfsync=ON on Apple, busy_timeout=60000,
-- trusted_schema=OFF, cell_size_check=ON

CREATE TABLE meta (                   -- format, schema_version, canary.<extractor_version>
  key   TEXT PRIMARY KEY,
  value TEXT NOT NULL
) STRICT;

CREATE TABLE clock (                  -- the logical clock: one row, +1 per write transaction (DB8)
  one INTEGER PRIMARY KEY CHECK (one = 1),
  seq INTEGER NOT NULL CHECK (seq >= 0)
) STRICT;

CREATE TABLE op (                     -- what each write transaction did; for audit and the race tests
  seq             INTEGER PRIMARY KEY,
  kind            TEXT NOT NULL CHECK (kind IN ('init','ingest','replace','delete','reindex','gc','migrate')),
  entry           INTEGER,
  sha256          TEXT,
  fitment_version TEXT NOT NULL,      -- CARGO_PKG_VERSION of the writer
  at_unix_ms      INTEGER NOT NULL    -- for people only: never ordered by, never in a digest (OR2)
) STRICT;

CREATE TABLE blob (                   -- each row promises a file whose bytes hash to sha256 (I1)
  sha256    TEXT PRIMARY KEY CHECK (length(sha256) = 64 AND NOT sha256 GLOB '*[^0-9a-f]*'),
  bytes     INTEGER NOT NULL CHECK (bytes >= 0),
  added_seq INTEGER NOT NULL
) STRICT;

CREATE TABLE entry (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,   -- never reused (DL1)
  created_seq INTEGER NOT NULL,
  deleted_seq INTEGER CHECK (deleted_seq IS NULL OR deleted_seq >= created_seq)
) STRICT;

CREATE TABLE revision (
  id             INTEGER PRIMARY KEY AUTOINCREMENT,
  entry          INTEGER NOT NULL REFERENCES entry(id),
  sha256         TEXT NOT NULL CHECK (length(sha256) = 64 AND NOT sha256 GLOB '*[^0-9a-f]*'),
  source_name    BLOB CHECK (source_name IS NULL OR length(source_name) <= 4096),  -- display only (PH3)
  added_seq      INTEGER NOT NULL,
  superseded_seq INTEGER CHECK (superseded_seq IS NULL OR superseded_seq >= added_seq)
) STRICT;
CREATE UNIQUE INDEX revision_current      ON revision(entry)  WHERE superseded_seq IS NULL;  -- one current per entry
CREATE UNIQUE INDEX revision_live_content ON revision(sha256) WHERE superseded_seq IS NULL;  -- one live entry per content

CREATE TABLE features (               -- one features.v1 document per content and extractor version; insert-only
  sha256            TEXT NOT NULL REFERENCES blob(sha256) ON DELETE CASCADE,
  extractor_version INTEGER NOT NULL CHECK (extractor_version > 0),
  status            TEXT NOT NULL CHECK (status IN ('accepted', 'refused')),
  encoding          TEXT NOT NULL DEFAULT 'json' CHECK (encoding = 'json'),   -- M5A-11
  doc               BLOB NOT NULL,    -- FeaturesDocument::to_json(), exactly (GT2)
  doc_sha256        TEXT NOT NULL CHECK (length(doc_sha256) = 64 AND NOT doc_sha256 GLOB '*[^0-9a-f]*'),
  product_id        TEXT,             -- denormalized for list and lookup; never read by matching (GT5)
  product_name      TEXT,
  holes INTEGER, shafts INTEGER, planes INTEGER, complete INTEGER,
  first_refusal     TEXT,
  CHECK ((status = 'accepted') = (first_refusal IS NULL)),
  PRIMARY KEY (sha256, extractor_version)
) STRICT;
CREATE TRIGGER features_immutable BEFORE UPDATE ON features
BEGIN SELECT RAISE(ABORT, 'features rows are insert-only'); END;

CREATE TABLE tokens (                 -- IndexTokens of an accepted document; insert-only
  sha256            TEXT NOT NULL,
  extractor_version INTEGER NOT NULL,
  signature_version INTEGER NOT NULL CHECK (signature_version > 0),
  tokens            BLOB NOT NULL,    -- IndexTokens canonical JSON (PR 1)
  tokens_sha256     TEXT NOT NULL,
  lever_mm          REAL NOT NULL CHECK (lever_mm >= 0),
  planes            INTEGER NOT NULL CHECK (planes >= 0),
  pairs_incomplete  INTEGER NOT NULL CHECK (pairs_incomplete IN (0, 1)),
  PRIMARY KEY (sha256, extractor_version, signature_version),
  FOREIGN KEY (sha256, extractor_version) REFERENCES features(sha256, extractor_version) ON DELETE CASCADE
) STRICT;
CREATE TRIGGER tokens_accepted_only BEFORE INSERT ON tokens
WHEN (SELECT status FROM features
      WHERE sha256 = NEW.sha256 AND extractor_version = NEW.extractor_version) IS NOT 'accepted'
BEGIN SELECT RAISE(ABORT, 'tokens for a refused or missing document'); END;   -- GT4
CREATE TRIGGER tokens_immutable BEFORE UPDATE ON tokens
BEGIN SELECT RAISE(ABORT, 'tokens rows are insert-only'); END;

CREATE TABLE posting (                -- one row per stack diameter: level 1's range lookup (SG10)
  sha256            TEXT NOT NULL,
  extractor_version INTEGER NOT NULL,
  signature_version INTEGER NOT NULL,
  sense             INTEGER NOT NULL CHECK (sense IN (0, 1)),     -- 0 hole, 1 shaft (core's sense_key)
  bin               INTEGER NOT NULL,                             -- core's bin(diameter_mm)
  diameter_mm       REAL NOT NULL CHECK (diameter_mm > 0),
  stack             INTEGER NOT NULL CHECK (stack >= 0),
  FOREIGN KEY (sha256, extractor_version, signature_version)
    REFERENCES tokens(sha256, extractor_version, signature_version) ON DELETE CASCADE
) STRICT;
CREATE INDEX posting_range ON posting(extractor_version, signature_version, sense, bin, sha256);
CREATE INDEX posting_owner ON posting(sha256, extractor_version, signature_version);

CREATE VIEW live AS                   -- the catalog as of the snapshot: live entries, current revisions
SELECT e.id AS entry, r.id AS revision, r.sha256, r.source_name, r.added_seq
FROM entry e JOIN revision r ON r.entry = e.id AND r.superseded_seq IS NULL
WHERE e.deleted_seq IS NULL;
```

Why it is shaped this way:

- **Content-keyed derived rows** make a derived row a pure function of (bytes, versions). Entries only point. So:
  - replace and delete never rewrite derived data;
  - a re-index never touches entries;
  - `verify --deep` can recompute any row from its blob alone.
- **Insert-only `features` and `tokens`** (triggers) mean a reader holding a document by key reads exactly what was written. A status change at a new extractor version is a new row, never an update.
- **Cascades from `blob`** let delete and gc remove derived data with one `DELETE FROM blob`, in one transaction.
- **`revision.sha256` has no foreign key to `blob`,** on purpose: a tombstoned revision keeps naming a content whose blob is gone (M5A-2).
- **The two partial unique indexes** enforce DL2 and GT6 in the database, so no interleaving of processes can create two current revisions of an entry or two live entries of one content.
- **Rowid tables with composite keys,** not `WITHOUT ROWID`: documents of up to 0.7 MB are poor `WITHOUT ROWID` rows.

The queries the rules rely on:

```sql
-- RX2: stale live content (extractor), then accepted content without current tokens (signature)
SELECT count(*) FROM live l
  LEFT JOIN features f ON f.sha256 = l.sha256 AND f.extractor_version = :ev
  WHERE f.sha256 IS NULL;
SELECT count(*) FROM live l
  JOIN features f ON f.sha256 = l.sha256 AND f.extractor_version = :ev AND f.status = 'accepted'
  LEFT JOIN tokens t ON t.sha256 = l.sha256 AND t.extractor_version = :ev AND t.signature_version = :sv
  WHERE t.sha256 IS NULL;

-- IX5: one level-1 range (the exact window test is core's, in Rust)
SELECT p.sha256, p.stack, p.diameter_mm FROM posting p JOIN live l ON l.sha256 = p.sha256
  WHERE p.extractor_version = :ev AND p.signature_version = :sv AND p.sense = :sense
    AND p.bin BETWEEN :lo AND :hi
  ORDER BY p.sha256, p.stack, p.diameter_mm;

-- IX5: the catalog's largest lever, in the same snapshot
SELECT max(t.lever_mm) FROM tokens t JOIN live l ON l.sha256 = t.sha256
  WHERE t.extractor_version = :ev AND t.signature_version = :sv;
```

## 7. Rules

Each rule is stated so that a test can fail it; the test that does is named in brackets and listed in section 10.

### 7.1 The file store (ST)

- **ST1. Content address.** A blob's name is `sha256_hex(bytes)` (core's function), and it lives at `blobs/sha256/<first 2 hex>/<64 hex>`. Every file under `blobs/` hashes to its own name. [I2; T-verify]
- **ST2. One buffer.** Ingest reads the source file once into memory, through a handle opened once, with a limit of cap + 1 bytes (SZ1). The bytes it hashes, gates, extracts and writes as the blob are that buffer.
  - The source path is never reopened or copied, so a file rewritten during ingest cannot give a blob that differs from the document.
  - [T-st2: the fault seam swaps the source after the read; blob = buffer, doc = read(buffer)]
- **ST3. Write protocol.**
  - **Write and rename:** `tmp/<pid>-<n>.part` with `create_new`, then `write_all`, then `sync_all`, then `rename` to the final path. The fan-out directory is synced before the commit that names the blob; on Unix by opening the directory and calling `sync_all`, on Windows not possible with std (risk R7).
  - **Batches:** for a batch, every blob is synced, and each fan-out directory touched is synced once, before the batch's commit.
  - **A final name that already exists** is re-hashed. If equal, it is reused. If different (corruption), it is replaced by the rename and the replacement is reported.
  - [T-crash; T-fault enumerates every call]
- **ST4. Immutable blobs.** A final blob is set read-only (0444) and never opened for writing. Only ST3's corruption path and gc (ST5) change a final name.
- **ST5. Unlink rule.** A file under `blobs/` is unlinked only inside a `BEGIN IMMEDIATE` transaction (holding SQLite's write lock) that has just checked that no `blob` row names it.
  - Delete, replace and reindex never unlink; they remove rows, and the inline gc after their commit, or a later `gc`, unlinks under this rule.
  - gc unlinks at most 1,000 files per transaction, so writers wait at most one batch.
  - [T-race: ingest/delete processes against a gc loop; control: unlink after commit must fail]
- **ST6. Commit-time recheck.** Inside its write transaction, ingest checks that the final blob exists with the right size, and rewrites it from the buffer (ST3) if not, before inserting the `blob` row.
  - With ST5, a committed `blob` row always has its file. A gc that ran between the rename and the commit can only have deleted a file the recheck then restores.
- **ST7. Temp files.** Every error path removes its temp file. gc removes temp files older than one hour; the age only decides liveness, never safety, since a removed temp file makes its ingest fail with an error and nothing is committed.
- **ST8. No lock beyond SQLite.** The SQLite write lock is the catalog's only mutex. `File::lock` needs Rust 1.89 and the MSRV is 1.85, and a second lock would add a second order to get wrong.
- **ST9. Durability** (M5A-7).
  - **Blobs:** `File::sync_all` on each blob. On Apple, std's `sync_all` is `F_FULLFSYNC`: measured 7.9 ms with a directory sync per file, against 4.05 ms for `fsync(2)`, so std does not stop at the drive cache.
  - **Database:** WAL with `synchronous=FULL`; `fullfsync=ON` and `checkpoint_fullfsync=ON` on Apple.
  - **Order:** blob durability always comes before the commit that names the blob (ST3).

### 7.2 The database (DB)

- **DB1. Engine.** rusqlite with `bundled` (SQLite 3.53.2 today), WAL, STRICT tables, foreign keys on, `trusted_schema=OFF`. `application_id` and the marker file identify a catalog; opening anything else is refused (PH2). [T-open]
- **DB2. Every write transaction is `BEGIN IMMEDIATE`,** with `busy_timeout` 60 s.
  - Measured: 8 processes × 200 read-then-write transactions. With `BEGIN DEFERRED`, 1,543 of 1,600 failed with `SQLITE_BUSY` despite a 30 s timeout, because a WAL reader cannot upgrade a stale snapshot. With `BEGIN IMMEDIATE`, none failed: 1,600 rows in 780 ms.
  - Every decision a write depends on ("is this content live?", "is this blob referenced?") is read inside the transaction that acts on it. [T-conc]
- **DB3. Short write transactions.** Gate, extraction, hashing and blob I/O all happen outside the write transaction. The one exception is ST6's rewrite of a blob that vanished.
  - Ingest commits batches of at most 64 files or 1 s of work. Reindex does the same.
- **DB4. One snapshot per read command.** `list`, `get`, `match` and `verify` each run in one read transaction and report its `seq`.
  - Measured: a reader that began before a delete still read the deleted row after the writer committed and checkpointed. `wal_checkpoint(TRUNCATE)` returned busy while the reader lived (WAL 13.0 MB after 300 × 30 KB inserts) and reset the WAL to 0 bytes once it ended.
- **DB5. Schema versions.** `user_version` is the schema version.
  - A binary refuses, read or write, a catalog whose schema is newer than its own.
  - It migrates an older schema forward only, in one `BEGIN EXCLUSIVE` transaction (SQLite's DDL is transactional), recording an `op` row.
  - No migration exists in M5; the mechanism and its test do.
- **DB6. Constraint plus validation.** Hex columns carry `CHECK`s, and the code validates every hex value read from the database again before building a path (PH1). A hand-edited row cannot reach the file system.
- **DB7. Derived rows are keyed by content and version** and are insert-only (triggers). Matching reads only rows of this build's `EXTRACTOR_VERSION` and `SIGNATURE_VERSION`.
- **DB8. The logical clock.** Each write transaction increments `clock.seq` once and writes one `op` row per affected entry or content. `seq` orders history; wall time never does.

### 7.3 The canonical gate at ingest (GT)

- **GT1. Every file passes M2's gate,** through `FeaturesDocument::read(buffer)`:
  - **Accepted:** a `blob` row; an `entry` and a current `revision`; a `features` row with status `accepted`; a `tokens` row (`IndexTokens::of(PartSignature::of(part))`); its `posting` rows. All in one transaction.
  - **Refused:** the content is quarantined. It gets the same `blob`, `entry`, `revision` and `features` rows, status `refused`, with the document the gate wrote (refusals and header). It gets no `tokens` and no `posting` rows.
- **GT2. Byte identity.** The stored `doc` is `FeaturesDocument::to_json()` of the buffer, so it is byte-identical to `fitment features FILE`. `doc_sha256` is its SHA-256. [T-gt2 over the unit set, corpus and fixtures]
- **GT3. Refused at the door, nothing stored.** These exit 3 per file, while the rest of the run goes on:
  - a file over the cap (SZ1);
  - a path that is not a regular file (PH4);
  - an unreadable file.

  A file that is not STEP at all (empty, an image) is not refused at the door: the gate refuses it and it is quarantined like any other.
- **GT4. Quarantine never reaches the index.** The `tokens_accepted_only` trigger refuses tokens for anything but an accepted document of the same version, and postings cascade from tokens. `catalog match` loads only `status = 'accepted'` documents, and `judge_document` would `Reject` a refused one anyway. [T-gt4: insert tokens for a refused row → abort]
- **GT5. Denormalized columns are for people only.** `product_id`, `product_name`, the counts and `first_refusal` are written from the document by the same function that writes `doc`. They are used for `list` and selectors, never for matching.
- **GT6. Deduplication.** Bytes already live in some entry are not stored again: ingest reports that entry (M5A-12). The check happens inside the write transaction. The partial unique index `revision_live_content` refuses a second live revision of the content even if two processes race; the loser reports the winner's entry. [T-conc]
- **GT7. Assemblies** are refused by the gate (M2's `assembly` code) and quarantined. M5 does not ingest an assembly's leaves; M4-14 leaves that to the API.
- **GT8. Why store refused files:**
  - soundness rule 6 asks for the reason, and the stored document is that reason, in the format users already read;
  - a later gate may accept the file, and reindex (RX4) re-gates quarantined content from its blob;
  - a later gate may also refuse a file it accepted, and RX4 then takes it out of the index in the same transaction.

### 7.4 Delete and replace (DL)

- **DL1. Ids are never reused** (`AUTOINCREMENT`). A deleted entry's id stays a tombstone.
- **DL2. Delete is one transaction:**
  1. set `entry.deleted_seq`;
  2. set the current revision's `superseded_seq`;
  3. `DELETE FROM blob` for the content when no other live revision names it, which cascades to `features`, `tokens` and `posting`;
  4. write an `op` row.

  After the commit, an inline gc (ST5) unlinks the file. A crash between the commit and the unlink leaves an orphan file, which is harmless (I6). [T-fault]
- **DL3. Replace is one transaction:** the new revision becomes current and the old one is superseded. The new content's rows are inserted, or found, in the same transaction, and the old content's are removed as in DL2.
  - A query sees the old revision or the new one, never both and never neither (two partial unique indexes).
  - A replace whose new file the gate refuses is refused (M5A-3), unless `--quarantine`.
  - A replace with bytes that are live in another entry is refused (GT6), naming that entry.
- **DL4. Queries and concurrent changes.** A command's snapshot (DB4) fixes what it sees.
  - A match that began before a delete judges the deleted part and reports `as of seq S`. A match that begins after does not see it.
  - This is the only consistent choice: a match that took deletions mid-run would judge a set that never existed.
- **DL5. Re-ingesting deleted bytes** creates a new entry with a new id. The tombstone keeps the old one.
- **DL6. Tombstones keep metadata only:** id, revisions with SHA-256 and `source_name`, seqs, and the part number through `op`. No derived rows, no blob.

### 7.5 Re-indexing when a version rises (RX)

- **RX1. Current means this build's versions.** A features row is current when its `extractor_version` is `EXTRACTOR_VERSION`; a tokens or posting row when its `(extractor_version, signature_version)` is `(EXTRACTOR_VERSION, SIGNATURE_VERSION)`.
  - A loaded document must also say so itself: `FeaturesDocument::is_current()`. The column and the JSON must agree, or the load fails (IX3).
- **RX2. Stale means:** live content with no current features row, or accepted content with no current tokens row. These are the two counts of section 6, run in the command's snapshot. Quarantined content counts too: a new gate might accept it.
- **RX3. A catalog match refuses** when either count is non-zero (M5A-4): exit 3, the counts, and the hint "fitment catalog reindex". `--allow-stale` judges only current rows and prints the skipped count on stderr.
  - Rows of other versions are inert: a match never reads them.
- **RX4. Reindex.**
  - **Extractor-stale content:** its blob is read and its hash verified. A mismatch fails that content, names it, and points to `verify`. It is then re-gated and extracted, and its new-version rows inserted in batches (DB3).
    - Status may flip either way: a previously refused file can enter the index, and a previously accepted one leaves it.
  - **Signature-stale content only:** tokens are rebuilt from the stored current document (`from_json`, then the signature), with no blob read: about 0.1 ms a part.
- **RX5. Never downgrade.** Rows of a newer version than the build's are never written, rewritten or deleted.
  - gc removes derived rows of older versions only, and only for content whose current-version rows exist, so a half-finished reindex never loses the old rows another binary may still read.
  - Two fitment versions can therefore share a catalog: each reads its own rows.
- **RX6. Resumable.** Each batch is one transaction. A crash or kill keeps the committed batches, and rerunning reindex does exactly the missing ones. Reindex is idempotent. [T-fault, T-rx6]
- **RX7. Nothing stored depends on `MATCH_RULES_VERSION`.** The catalog caches no verdict, shortlist result or placement, for any key.
  - A verdict cache would need every version, the policy, the socket digest and the fit verifier in its key, and a missed field is a false positive. None is worth that in M5.
  - M6's stored reports will record the versions (MF6) and stay history, never input (M4-23).
- **RX8. `SOCKET_RULES_VERSION`.** M5 stores no sockets (M5A-9). Socket files passed to `catalog match` go through `Matcher::new`, which refuses a stale one (VD9). When M6 stores sockets, they follow RX1 to RX6: keyed by (assembly SHA-256, selection, `SOCKET_RULES_VERSION`), re-extracted from the stored assembly blob, and with the user's decisions carried over by MF11 into a new row, never an update.
- **RX9. Canary** (M5A-16).
  - Before ingest, reindex or match, the binary extracts its three embedded probe parts and hashes the documents: a few ms (estimated, not measured). On first use for an `EXTRACTOR_VERSION`, it records the hashes in `meta` as `canary.<v>`.
  - A later mismatch for the same version means two builds that both say version v extract differently. The command refuses, exit 1: "extractor changed without a version rise; this catalog's v-rows may be wrong: `fitment catalog reindex --force-version`".
  - `--force-version` deletes all rows of the build's version and re-extracts. [T-rx9: tamper `meta`]

### 7.6 A match reads only the index (IX)

- **IX1. What a match reads:** the socket document (a file argument) and `catalog.db`. No file under `blobs/` is opened and no STEP is parsed.
- **IX2. By type.** `Catalog::snapshot()` returns a `Snapshot`: a rusqlite read transaction and the build's versions, with no field naming the blob store. `match_socket(&Snapshot, &Matcher)` is the only matching entry point in `fitment-catalog`.
  - The blob store (`Blobs`) is reachable only from `Catalog`, through the ingest, reindex, verify and `get --file` paths.
- **IX3. Loading a document.** In the snapshot, in SHA-256 order:
  1. read `doc`, `doc_sha256`, `status`, `extractor_version`;
  2. check `sha256_hex(doc) == doc_sha256`;
  3. parse with `FeaturesDocument::from_json`, which checks the format, the version and the part's structural rules;
  4. check `is_current()`, `status == accepted`, and that the document's status equals the column.

  Any failure means a corrupt row. That candidate is not judged; stderr names it and points to `fitment catalog verify`; the others are judged; the run exits 1 (as MF3 for an unreadable candidate). It is never judged from a partly read document.
- **IX4. Same judgment as a file.** The candidate is judged by `Matcher::judge_document(&doc, source)`, where `source` is `Source { sha256: row key, bytes: blob.bytes, header: doc.header() }`. `judge_step(bytes)` is defined in core as `judge_document(&FeaturesDocument::read(bytes), Source::of(bytes, …))` (PR 1).
  - So a candidate from the catalog and the same file named on the command line give the same `CandidateResult`, provided `from_json(to_json(d)) == d`.
  - Measured: 243 of 243 accepted parts round-trip, equal as values and as bytes; 90 of 90 match cases give equal results and match-document digests.
- **IX5. Level 1 from the postings.** For each socket:
  1. For each need (sense, window) of core's level 1, fetch the postings with `bin BETWEEN bin(lo) − 1 AND bin(hi) + 1`, using core's `bin`.
  2. Keep those whose `diameter_mm` passes core's `Window::holds`, and intersect over the needs.
  3. Load the survivors' tokens, check `tokens_sha256`, and run core's pair and plane filters at the snapshot's largest lever.

  This is the same result as core's `ShortlistIndex::query` over every current token in memory, checked by a test.
  - Stored tokens and postings feed level 1 only. Level 2, alignment and verification recompute everything from the document (`CandidateView::of`), so a corrupt token can only drop a candidate.
  - A candidate must pass every socket, so the survivor sets of a document's sockets are intersected. A blocked socket (SL8) gives an empty run, with the reason on stderr.
- **IX6. Labels and order.** A candidate's label is `e<id>`, then a space and its `product_id` when it has one, escaped by `printable()`. Labels stay outside the digest (MF4). The document's order is MF5's: content only.
- **IX7. The fit stage is the one later exception (M7).** `FitRequest.candidate_step` needs bytes. In M7 the catalog will read a blob only for a candidate whose interface passed, and verify its hash before handing it over.
  - M5 passes `None` to a `NoFitVerifier`, so no blob is read.
- **IX8. The proof** is three tests, all needed:
  - **T-ix-type:** a compile-fail doc test. `match_socket` cannot be written against `Blobs`.
  - **T-ix-count:** the counting file-system seam (section 10.2) records zero blob opens across a whole `match_socket` on the corpus catalog.
  - **T-ix-exit:** with `blobs/` renamed away (read-only on Windows: renamed), `fitment catalog match S` exits as before, and its digest equals that of `fitment match S <the survivors' files>`. Its passes equal the passes of `fitment match S <every accepted file in the catalog>`. This is M5's exit test.

### 7.7 Concurrency (CC)

- **CC1. Many processes, one writer at a time.** Readers never block writers or each other (WAL); writers queue on the write lock (DB2).
- **CC2. Same bytes ingested at once:**
  - both processes write the same blob (an identical rename, ST3);
  - the first to commit wins;
  - the second, inside its transaction, finds the live content and reports the winner's entry (GT6).
  - Different bytes never conflict.
- **CC3. A long match holds one snapshot** (DB4). The WAL then grows with concurrent writes until the match ends: measured 13 MB for 9 MB of inserts.
  - After its snapshot ends, `catalog match` runs `wal_checkpoint(PASSIVE)`.
  - `gc` runs `TRUNCATE`.
- **CC4. verify does not raise false alarms.** It reads blobs while others may delete. When a blob is missing or wrong, it re-checks, in a fresh snapshot, that the content is still referenced before reporting it.
- **CC5. One host.** WAL needs shared memory, so a catalog on a network file system, or in a folder a sync tool copies (iCloud Drive, Dropbox), is unsupported and documented.
  - Open checks that `journal_mode` came back `wal`, and refuses otherwise.
- **CC6. Mixed versions of fitment** work by RX5 and DB5: each binary reads its own version's rows and refuses a newer schema.

### 7.8 Determinism of order (OR)

- **OR1. Every query whose rows reach output, or decide an order, ends in `ORDER BY` over a unique key.**
  - The integration tests run every command twice, once with `PRAGMA reverse_unordered_selects=ON`, and require identical output.
  - Measured: the pragma reverses an unordered `SELECT` and leaves `ORDER BY` alone, so a missing `ORDER BY` is caught.
- **OR2. Nothing environmental in output order.** `list` orders by entry id (`--sort part` orders by product id, then SHA-256), and its JSON carries `seq`, not times.
  - Wall time is written only to `op.at_unix_ms` and shown only by `catalog log`.
- **OR3. Directory ingest goes in byte order of file names** (not recursive by default), so one process gives the same ids for the same tree on every OS. Concurrent ingest gives ids in commit order, recorded in `op`.
- **OR4. A catalog match document is identical** across runs and OSes for the same catalog contents and versions. Its digest does not depend on ids, insertion order or labels (MF5, MF7).
- **OR5. No `HashMap` or `HashSet`** in `fitment-catalog`. Its `clippy.toml` bans them, as core's does. Also banned: `SystemTime::now` outside `op`, `env::var` (the CLI resolves `FITMENT_CATALOG`), and `std::process`.

### 7.9 Path and file name hostility (PH)

- **PH1. Paths come from validated hex only.** A path under the root is built from a `Sha256Hex` newtype, whose constructor accepts exactly 64 characters `[0-9a-f]`. No other value from the database, a STEP file, an argument or a file name becomes a path component. [T-ph1: a `revision.sha256` of `../../x` violates the CHECK; through a raw connection with checks off, `verify` and `gc` refuse it without touching the file system]
- **PH2. The root.**
  - `init` refuses a non-empty directory without the marker.
  - Every command refuses a directory whose marker or `application_id` is wrong.
  - Every command refuses a root in which `blobs`, `blobs/sha256`, a fan-out directory or `tmp` is a symbolic link (checked with `symlink_metadata` at open and before gc). A planted link could otherwise point gc at someone else's files.
- **PH3. Source names are for display only.** The source's final path component is stored as bytes (`OsStr`; on Windows its UTF-8), up to 4,096 bytes.
  - It is shown only through `printable()`: control characters, bidirectional overrides and newlines are escaped.
  - It is never an identity, an order key or a path.
- **PH4. Directory ingest.**
  - A directory's entries are taken in byte order. A file qualifies when its name ends in `.stp`, `.step` or `.p21`, in any case.
  - A directory is not recursive by default. `--recursive` does not follow symbolic links to directories (no loops); it follows links to files.
  - Files named explicitly are taken whatever their extension.
  - Only regular files are read, checked by `metadata` before opening and again on the opened handle. A FIFO, socket or device is reported and skipped.
  - A file that disappears between the listing and the open is reported, and the run goes on.
- **PH5. On Windows, a root whose path is not Unicode is refused,** because rusqlite needs UTF-8 there. Unix paths are bytes and pass through.
- **PH6. Selectors** (`get`, `replace`, `delete`):
  - an entry id (`e17` or `17`);
  - a SHA-256 prefix of at least 8 hex characters;
  - an exact `product_id`.

  A value that matches more than one entry, or matches as more than one kind, is refused with the matches listed. A deleted id names its tombstone for `get` only.

### 7.10 Size limits (SZ)

- **SZ1. The size cap is 128 MiB** (`--max-bytes`, M5A-6). It is checked with `metadata` before reading and again by reading with `take(cap + 1)`, so a file that grows during the read is still refused.
- **SZ2. Derived documents.** The measured ratio of a features document to its STEP file is at most 1.39 (`nist_ftc_08_asme1_rc`: 450,741 → 628,571 bytes); the median document is 27.6 KB and the largest 688.6 KB.
  - SQLite's default limit for one value is 10⁹ bytes. A document over 512 MiB is refused as `derived_too_large` and not stored. Under SZ1 that needs a ratio above 4, so it is a guard, not a path.
- **SZ3. Tokens** are bounded by core (`MAX_PAIR_TOKENS` 20,000, `pairs_incomplete`). Measured: median 126 B, 90th percentile 1.9 KB, largest 348 KB (`nist_ctc_02_asme1_ap203`, 84 stacks).
- **SZ4. Growth.** The database never shrinks by itself. `gc --vacuum` runs `VACUUM`, which needs free space about the size of the database; the command checks the free space first and says so.
- **SZ5. A quota.** `Options::max_db_pages` sets `PRAGMA max_page_count`. It is a real option for users who want one, and it is also how the disk-full tests run on every OS (T-full).

### 7.11 Verification (VF)

- **VF1. `fitment catalog verify`** runs in one snapshot (DB4) and checks:
  - `PRAGMA quick_check` (`--full`: `integrity_check`) and `foreign_key_check`;
  - I1 to I8 of section 8.1, each blob re-hashed;
  - each current document's `doc_sha256`, its `from_json`, and its column/JSON agreement;
  - each token's `tokens_sha256`.

  Exit 0 when clean, 1 when anything is wrong, with one line per finding.
- **VF2. `verify --deep`** also re-extracts every current content from its blob and requires the stored `doc` and `tokens` to be equal byte for byte.
  - This is the check that the index is derived from the store, whatever happened to either.
  - It costs a reindex's time.

## 8. Algorithms and invariants

### 8.1 Invariants (checked by `verify`, and after every crash and fault test)

- **I1.** Every `blob` row has a file at its path whose bytes hash to its key and whose length is `bytes`.
- **I2.** Every final file under `blobs/` hashes to its own name. Orphans (no row) are allowed.
- **I3.** Every live revision's content has a `blob` row.
- **I4.** An entry is deleted exactly when it has no current revision.
- **I5.** Every `features` row's `doc` hashes to `doc_sha256`, reads back with `from_json`, and states the row's status and extractor version.
- **I6.** Orphan files and temp files are the only residue a crash may leave. No partial final file ever exists.
- **I7.** `tokens` exist only for accepted documents of the same extractor version, and every `posting` row matches its tokens, bit for bit in its diameter.
- **I8.** `clock.seq` equals the largest `op.seq`.

### 8.2 Ingest (one batch)

```text
for each file, in byte order of names (PH4):
    bytes  = read_once(file, cap)                         ST2, SZ1   (no lock)
    sha    = sha256_hex(bytes)
    doc    = FeaturesDocument::read(bytes); json = doc.to_json()   GT1, GT2
    tokens = doc.part().map(|p| IndexTokens::of(&PartSignature::of(p)))
    write_blob(sha, bytes)                                ST3 (tmp, sync, rename; reuse if equal)
sync each touched fan-out directory                       ST3
BEGIN IMMEDIATE                                           DB2
    seq += 1
    for each file:
        if content is live in entry E: report E, continue        GT6 (inside the txn)
        stat blob; rewrite from buffer if missing                ST6
        INSERT OR IGNORE blob; INSERT entry, revision
        INSERT OR IGNORE features (sha, EXTRACTOR_VERSION) ...   (content may exist from a tombstoned past)
        if accepted: INSERT OR IGNORE tokens; INSERT postings
        INSERT op
COMMIT                                                    (on SQLITE_FULL: rollback; files become orphans)
```

### 8.3 Delete, replace, gc

```text
delete E:   BEGIN IMMEDIATE; check E live; seq += 1
            UPDATE entry SET deleted_seq; UPDATE revision SET superseded_seq
            if no other live revision names sha: DELETE FROM blob WHERE sha256 = sha   (cascades)
            INSERT op; COMMIT; then gc_one(sha)
replace E F: prepare F as in ingest (outside the lock); refuse if F is refused (M5A-3)
            BEGIN IMMEDIATE; check E live; check F not live elsewhere; seq += 1
            UPDATE old revision superseded; INSERT new revision; insert F's rows (as ingest)
            remove old content's rows if unreferenced (as delete); INSERT op; COMMIT; gc_one(old)
gc:         repeat: BEGIN IMMEDIATE
                        for up to 1,000 files under blobs/ with no blob row: unlink    ST5
                        (with --versions: DELETE derived rows of older versions, RX5)
                    COMMIT
            remove tmp files older than 1 h (ST7); wal_checkpoint(TRUNCATE)
```

On a full disk, gc frees space without writing to the database: its transactions only hold the lock. A full disk therefore never blocks its own cure.

### 8.4 Reindex

```text
BEGIN (read): list stale contents (RX2) in SHA-256 order; END
for each batch of ≤ 64:
    extractor-stale: bytes = read blob; require sha256_hex(bytes) == key; doc = read(bytes)
    signature-stale: doc = from_json(stored current doc)
    BEGIN IMMEDIATE; INSERT OR IGNORE features / tokens / postings; seq += 1; INSERT op; COMMIT
(old-version rows stay until gc --versions: RX5)
```

### 8.5 Match

Section 4's right-hand column, with IX3 to IX6. The snapshot is held from the stale count to the last candidate.

### 8.6 What M6 adds to the store (not built in M5)

These are the stored assembly, socket and report tables; the shape is reserved here so M5's schema needs no rework:

- **Assemblies** are blobs like parts, with `kind = 'assembly'` in a new column, defaulting to `part`, added by a DB5 migration.
- **Sockets** are a `socket` table keyed by (assembly SHA-256, selection, `SOCKET_RULES_VERSION`) holding the reviewed `fitment.socket` document and its digest. They are insert-only, so a review is a new row with its predecessor's seq.
- **Reports** are a `report` table holding `fitment.match` JSON, its digest and every version (RX7).

## 9. Core API needed (PR 1)

All of this is additive to `fitment-core` 0.1. None of it changes an output, so no version rises. Core stays free of I/O.

```rust
impl Matcher<'_> {
    /// CF1 for a stored document: an accepted, current document is judged as judge_step
    /// judges the file; a refused one is the same Reject judge_step gives; a document
    /// that is not current is refused (ReadFacts states the document's own version).
    pub fn judge_document(&self, doc: &FeaturesDocument, source: Source) -> Result<CandidateResult, NotCurrent>;
}
impl Source { pub fn new(sha256: String, bytes: u64, header: Option<FileHeader>) -> Self; }  // validated hex
impl IndexTokens {
    pub fn to_json(&self) -> String;                                   // canonical, float_roundtrip
    pub fn from_json(text: &str) -> Result<Self, FormatError>;         // finite, ≥ 0, sorted as `of` writes
}
pub fn index_bin(diameter_mm: f64) -> i64;                             // SG10's bin, the one level 1 uses
pub trait PostingSource {                                              // level 1 over storage the caller owns
    fn diameters(&mut self, sense: Sense, bins: RangeInclusive<i64>) -> Vec<(u64, usize, f64)>;
    fn tokens(&mut self, id: u64) -> Option<IndexTokens>;
    fn largest_lever_mm(&mut self) -> f64;
}
impl ShortlistIndex {
    pub fn query_source(socket: &SocketSignature, source: &mut dyn PostingSource) -> Result<Vec<u64>, Blocked>;
}
```

- **The `judge_part` trap.** `judge_part` today records `ReadFacts.extractor_version = EXTRACTOR_VERSION` whatever `Part` it is given. Fed a stale document's part, it would judge with current rules and claim a current read. `judge_document` closes that, and `judge_part` keeps its documented use (M4's originals, already read by this build).
- **`judge_step` becomes a wrapper:** `judge_document(&read(bytes), Source::of(bytes, header))`. Its output is unchanged, checked by the match goldens.
- **`ShortlistIndex::query` becomes `query_source`** over an in-memory source, so the catalog and the M4 tests run the same code. Core's own windows and multiplicity stay where they are.
- **`IndexTokens` needs a reader,** because the catalog stores tokens. Its validation matters only to level 1 (IX5).

## 10. Tests

### 10.1 Unit tests (`crates/fitment-catalog/src`, `#[cfg(test)]`, in-memory or temp databases)

- **Schema:**
  - each CHECK refuses a bad value: a non-hex SHA, 63 characters, an upper-case digit, a negative size;
  - the triggers refuse a features update and tokens for a refused document;
  - the two partial unique indexes refuse a second current revision and a second live content;
  - the cascades remove tokens and postings with the blob.
- **`Sha256Hex`:** accepts exactly 64 `[0-9a-f]`; refuses `..`, `/`, NUL and upper case (PH1).
- **Selectors:** id, prefix and part number; ambiguity listed; prefix shorter than 8 refused (PH6).
- **Clock and op:** one increment per write transaction, I8.
- **Ordering:** every query module runs under `reverse_unordered_selects` with the same result (OR1).
- **Staleness:** the RX2 counts on hand-built rows of versions 5, 6 and 7 under a build at 6 (rows from edited JSON, `doc_sha256` recomputed). Only version-6 rows are current; version 7 is never touched (RX5).

### 10.2 The file-system seam and fault tests (`crates/fitment-catalog/tests/fault.rs`)

The catalog does every file operation through a `trait Fs`: `read_limited`, `create_new`, `write_all`, `sync_file`, `rename`, `sync_dir`, `remove_file`, `read_dir`, `metadata`, `symlink_metadata`, `set_readonly`. The trait also has `point(&'static str)`, called just before and just after each `COMMIT`. `StdFs` is the real one. The tests use:

- **`CountingFs`:** counts calls by kind and path class (blob, tmp, db). T-ix-count requires zero `read_limited` or `create_new` calls on a blob path during `match_socket`.
- **`FaultFs { fail_at: k, kind }`:** call k returns `ENOSPC`, `EIO` or `ENOENT`, or, in a child process, calls `std::process::abort()`.
- **T-fault.** For each operation (an ingest batch of 3 files with one duplicate and one refused, a replace, a delete, a gc, a reindex batch), run it once to count its calls N. Then, for each k in 0..N and each fault kind:
  1. run it with the fault at k (aborts in a re-executed test binary, as `fitment-fit`'s adversarial tests do);
  2. reopen with `StdFs`;
  3. require I1 to I8;
  4. require the operation's effect in the database to be all or nothing;
  5. require a rerun to complete it.

  This runs on Linux, macOS and Windows.
- **T-full (disk full, every OS).**
  - **Blob side:** `FaultFs` returns `ENOSPC` on each blob write. Nothing is committed, the temp file is removed and the run exits 1.
  - **Database side:** `Options::max_db_pages` small. `SQLITE_FULL` rolls the batch back, I1 to I8 hold, and `gc` then removes the orphans without a database write (8.3).
  - Measured: with `max_page_count` 200, 26 commits, then "database or disk is full", 26 rows, integrity ok. On a 16 MB APFS image (macOS, manual), the blob write failed with ENOSPC (temp file removed) and SQLite failed with "database or disk is full" with every committed row intact and `integrity_check` ok.
- **T-st2.** The source file is replaced between `metadata` and `read_limited` (a seam hook). The blob and the document come from the bytes read.

### 10.3 Crash and race soaks (Unix; `--ignored` locally, nightly in CI)

- **T-crash.** A child ingests in a loop and is sent SIGKILL at a pseudo-random time, 300 rounds; after each, I1 to I8.
  - Measured with the prototype protocol: 300 of 300 rounds clean. Orphans and temp files accumulated, without gc, to 151 and 136 over the 300 rounds, at most one of each per kill.
  - Control: committing the row before writing the blob broke I1 in round 0.
- **T-race.** For 20 s: four processes ingest and delete from a pool of 40 contents (forcing same-content races) and one runs gc in a loop. Every 100 ms a reader checks, in its snapshot, that every `blob` row's file exists and hashes, re-checking in a fresh snapshot (CC4).
  - Measured: 5,120 checks, 0 missing.
  - Control: unlinking after the commit, 6 missing in 5,301.
- **T-conc.**
  - Eight processes ingest overlapping directories concurrently. Every content ends live exactly once (GT6), I1 to I8 hold, and no command failed with `SQLITE_BUSY`.
  - A ninth runs `catalog match` in a loop. Each run's document equals the document of an explicit `fitment match` over the files of the entries live at its reported `seq`, replayed from `op`. This is DL4.

### 10.4 The corpus catalog (`crates/fitment/tests/catalog.rs`, `FITMENT_REQUIRE_CORPUS=1` in CI)

- **Build.** Ingest every part file of the generated corpus and the fixtures; assemblies and refused parts go to quarantine.
  - Measured inputs: 490 STEP files, of which 243 are accepted (15.0 MB of documents for 35.7 MB of STEP) and 247 refused, mostly assemblies.
  - Checks: the counts per status; `verify --deep` clean; GT2 for every file against `fitment features`.
- **T-ix-exit, M5's exit test.** For each of the 17 sockets the 90 match cases use, under both policies:
  - `catalog match` with `blobs/` renamed away;
  - its digest equals `fitment match S <survivors>`;
  - its passes equal the passes of `fitment match S <every accepted file>`;
  - every corpus case whose truth is `fits` and that passes explicitly passes through the catalog.
- **T-l1.** The catalog's level 1 equals `ShortlistIndex::query_source` over every current token in memory, for every socket. Every posting diameter is bit-equal to its token's (I7).
- **T-crud.** `ingest`, `list`, `get` (`--features`, `--file -o`), `replace`, `delete` and `gc` from the CLI on the corpus catalog:
  - after a delete, no match lists the part, and `get` shows the tombstone;
  - after a replace, the old content is unreferenced and gone after gc;
  - re-ingesting a deleted file gives a new id;
  - a replace with a refused file exits 3 and changes nothing (M5A-3).
- **T-rx6.** Rewrite every row's version to 5 (edited JSON, recomputed digests). `catalog match` exits 3 with the count. `reindex` is killed after its second batch and rerun. Then `catalog match` equals the fresh catalog's output and `verify --deep` is clean. A signature-only rise (rows at `signature_version` 0) is rebuilt with no blob read (`CountingFs`).
- **T-rx9.** A tampered `canary.6` value refuses ingest, reindex and match. `--force-version` recovers.
- **T-ph.** Each of these is stored, listed and escaped:
  - file names with a newline, a right-to-left override, a 255-byte name, spaces and non-UTF-8 bytes (Linux);
  - a FIFO in a directory, skipped and reported;
  - a symlink loop under `--recursive`, not followed;
  - a sparse 129 MiB file, refused at the door with nothing stored;
  - an empty file, quarantined;
  - a symlinked `blobs/ab`, so that every command refuses the catalog.
- **T-determinism.** The same ingest sequence on Linux, macOS and Windows gives identical `list --format json` and identical match documents. CI's determinism job `cmp`s them.
- **T-cli.** `cargo clippy -p fitment --no-default-features` builds without the catalog (layout §9, M5).

## 11. PR plan

Each PR has tests and two adversarial reviews, as in M4.

1. **Core API for a stored catalog** (fitment-core, additive):
   - `judge_document`, with `judge_step` rewritten through it; `Source::new`;
   - `IndexTokens::to_json` and `from_json`; `index_bin`; `PostingSource` and `query_source`, with `query` rewritten over it;
   - the round-trip property over the unit set, corpus and fixtures (`from_json(to_json(d)) == d`, bytes equal);
   - a goldens check that no match output changed.

   No version rises.
2. **The store and ingest** (features, tokens and postings all written at ingest, through PR 1's `IndexTokens::to_json`):
   - the `fitment-catalog` crate: `Fs` and `StdFs`, `Sha256Hex`, layout, marker, schema v1 with the DB5 mechanism, the clock and `op`;
   - ST1 to ST9, DB1 to DB8, GT1 to GT8; the canary (RX9);
   - `fitment catalog init | ingest | list | get` behind the default `catalog` feature;
   - the workspace `deny.toml` rule that only `fitment-catalog` may depend on rusqlite; the catalog's `clippy.toml` (OR5);
   - the T-fault and T-full cases for ingest, T-st2, T-ph, and unit tests.
3. **Delete, replace, gc, verify:** DL1 to DL6, ST5's gc, VF1 and VF2, CC1 to CC4; `fitment catalog replace | delete | gc | verify | log`; T-fault for each operation; T-crash, T-race and T-conc (without the match loop).
4. **The index and `catalog match`:**
   - `Snapshot`, IX1 to IX8, the level-1 query through `PostingSource`, `--allow-stale`;
   - T-ix-type, T-ix-count, T-ix-exit, T-l1, T-crud, and T-conc's match loop.

   **M5's exit test lands here.**
5. **Re-index:** RX1 to RX8; `fitment catalog reindex [--force-version]`; `gc --versions`; T-rx6, T-rx9, and T-fault for reindex batches.
6. **Release:**
   - the hardening the reviews ask for; CI's no-default-features leg, the nightly soaks and the determinism `cmp`;
   - PLAN.md (M5 row, D5 notes, Resume here) and ARCHITECTURE.md (a "Storing a part" section, the crate graph with `fitment-catalog`);
   - CHANGELOG; the measured numbers;
   - publish `fitment-catalog` with the workspace's next version.

## 12. Risks

- **R1. Database size.**
  - Features documents average 62 KB: 6.2 GB at 100,000 parts, plus 4 KB of tokens a part on average (the fixtures with many pairs dominate; the median is 126 B).
  - `VACUUM` needs that much free space again.
  - Compression (M5A-11) is the lever; the `encoding` column is ready.
- **R2. fsync cost on macOS.**
  - Measured: 3.3 ms a blob (`sync_all`, one directory sync per batch; one, four and eight threads gave the same, since the flush is per device). A one-row commit costs 0.09 ms in WAL with `synchronous=NORMAL`, 0.12 ms with `FULL`, and 3.74 ms with `fullfsync`.
  - Ingesting 10,000 files therefore costs about 33 s of syncing beside about 8 s of extraction (0.8 ms a file, measured in process), so syncing dominates. Accepted for durability (M5A-7).
- **R3. WAL growth** under a long match with heavy concurrent ingest (CC3). It is bounded by what is written during the match and reclaimed when the match ends.
- **R4. rusqlite declares no MSRV.**
  - 0.40.2 builds on Rust 1.85 today (checked).
  - The lock file and resolver 3 keep it, and the MSRV CI job catches a break.
  - Its graph brings `hashbrown` and `foldhash`, which never reach `fitment-core`; `tools/core-deps.py` checks the workspace-wide graph.
- **R5. The bundled SQLite is C.** `cargo install fitment` needs a C compiler (layout decision 10). The licences are fine: SQLite is public domain, and rusqlite and libsqlite3-sys are MIT.
- **R6. Sync folders and network file systems** (CC5) can corrupt a WAL database. It is documented and the WAL mode is checked at open, but it cannot be detected in general.
- **R7. Windows.**
  - std cannot sync a directory, so ST3's directory sync is skipped and a power loss can lose a rename the commit then names. `verify` reports it as I1, and the next ingest of that file restores it.
  - An open file cannot be unlinked, so gc skips sharing violations and retries on the next run.
  - Antivirus scanners can fail a rename transiently; the rename is retried 5 times with backoff.
- **R8. Core changes after the first publish.** PR 1 is additive and checked by cargo-semver-checks. `judge_step`'s rewrite must leave every golden byte unchanged.
- **R9. The posting query repeats core's bin arithmetic in SQL** (`bin BETWEEN …`). A drift can only drop candidates (false negatives), and T-l1 checks equality with the in-memory index.
- **R10. M5A-15 departs from M4-17** (judging after a level-2 drop). It costs time, not soundness; it can be revisited once a large catalog shows the cost.

## 13. Attacks on this design, and their fixes

How could the catalog serve a stale or corrupt index that makes a match pass wrongly? Each attack below was found while designing, and the rule that answers it is in section 7.

| # | Attack | What would go wrong | Fix |
| --- | --- | --- | --- |
| S1 | Judge a document written by an older extractor | Its part reflects rules since fixed. Through `judge_part`, the evidence would even claim the current extractor (`ReadFacts`), so a false positive would be invisible | DB7 and RX1 select current rows only; IX3 requires `is_current()`; `judge_document` refuses non-current documents and reports the document's own version (section 9) |
| S2 | Two builds that both say `EXTRACTOR_VERSION` 6 extract differently (a dev build, a missed bump) | Rows look current but were written by other rules | RX9's canary refuses; `verify --deep` (VF2) proves the rows against the blobs |
| S3 | A stored document is corrupted (bit rot, a hand edit, a bad sector), so a diameter changes and the part still reads back | A candidate is verified against features it does not have | IX3: `doc_sha256` on every load, then `from_json`'s structural checks; VF1 and VF2 |
| S4 | A bug attaches one content's document to another's key | The match document names content X as passing on Y's features | GT2 and ST2: one function derives the key, the blob and the document from one buffer; `verify --deep` checks every row; T-gt2 checks every corpus row against `fitment features` |
| S5 | A deleted part, a withdrawn one, is still offered | Catalog membership is availability (PLAN, Metadata), so it would be a recommendation of an unavailable part | DL2 removes the derived rows in the same transaction; the `live` view joins on live entries; ST9's durable commits stop power loss from undoing a delete |
| S6 | Replace leaves both revisions live, or neither | An outdated model is matched | Partial unique index `revision_current`; DL3 in one transaction |
| S7 | A corrupt or stale token or posting | Only a dropped candidate: level 2 onward recompute from the document (IX5) | RX2 also counts signature staleness; `tokens_sha256` is checked; T-l1 |
| S8 | A refused (quarantined) document reaches matching | A part the gate refused is judged | GT4's trigger; IX3 requires `accepted`; `judge_document` gives `Reject` for a refused document anyway |
| S9 | gc deletes a blob a concurrent ingest just committed | A row without a file. Matching is safe (IX1), but reindex and M7's fit stage break | ST5 and ST6; T-race and its control |
| S10 | A crash between the rename and the commit, or mid-file | A torn blob under its final name | ST3 (sync before rename, names from content); I2; T-crash |
| S11 | A row commits before its blob is durable, then power is lost | A row without a blob after reboot | ST3 orders the sync before the commit; ST9 F_FULLFSYNC on Apple; R7 on Windows |
| S12 | A read-then-write race (deferred transactions) | Measured: 1,543 of 1,600 transactions failed; under retries, a check made on a stale snapshot | DB2 `BEGIN IMMEDIATE`, with every check inside the transaction |
| S13 | `verify` reports a blob as missing because a delete committed after verify's snapshot | A false alarm that could prompt a destructive repair | CC4 re-checks in a fresh snapshot |
| S14 | A hand-edited `sha256` of `../../…` | gc or verify unlinks or reads outside the store | CHECKs and `Sha256Hex` (PH1, DB6); T-ph1 |
| S15 | A symlinked fan-out directory | gc unlinks files elsewhere | PH2 refuses symbolic links in the store |
| S16 | A missing `ORDER BY` | Output that depends on SQLite's plan, different between versions or after `ANALYZE` | OR1 with `reverse_unordered_selects` |
| S17 | A half-finished reindex | Some parts at the new version, some at the old; a match silently judges part of the catalog | RX3 refuses while anything is stale; RX6 resumes |
| S18 | An older binary garbage-collects a newer binary's rows, or reindexes them down | A ping-pong between versions, and lost rows | RX5: never downgrade, gc removes only older versions |
| S19 | A verdict cache keyed too narrowly | A cached `Match` survives a rule fix or a policy change | RX7: no cache in M5 |
| S20 | The source file changes during ingest | The blob is one version of the file and the document another | ST2: one buffer |
| S21 | The level-1 lever is computed over a subset, or before an insert | Level 1 narrower than level 2: a missed candidate | IX5 takes the largest lever over current live tokens in the same snapshot; T-l1 |
| S22 | A concurrent delete is half-seen during a long match | Judging a set that never existed | DB4, DL4: one snapshot, reported as `as of seq S` |

The fixes S1 and S2 needed core changes: S1 the `judge_part` trap, S2 the canary. They are the two findings this design adds to M4's code. S1 is not a fault in M4, whose `judge_part` only ever receives parts this build has just read. It is a trap for any store.

## 14. Evidence

Every figure in this document comes from the experiments in `exp/` (`exp/src/bin/*.rs`, built with `cargo build --release` against the worktree's `fitment-core`, rusqlite 0.40.2 bundled), run on 2026-10-16 on this Mac:

| Experiment | What it measured |
| --- | --- |
| `featsize.py` | The release `fitment features` over 490 STEP files: 243 accepted, 247 refused; documents 0.42 of accepted STEP bytes; the largest documents |
| `roundtrip.rs` | 243 of 243 accepted documents equal after `to_json` and `from_json`, as values and as bytes. Over all 243: reading STEP 187 ms, `from_json` 21 ms (at most 0.9 ms for a 689 KB document), `CandidateView::of` 3.1 ms, signature and tokens 0.7 ms |
| `equiv.rs` | The 90 corpus match cases against sockets from `fitment socket`: `judge_step(bytes)` against `judge_part(from_json(to_json(read(bytes))))` with a `Source` built from stored fields. `CandidateResult` equal 90/90; `MatchDocument` digest equal 90/90 |
| `tokdist.rs` | Token sizes (median 126 B, p90 1.9 KB, max 348 KB) and document sizes (median 27.6 KB, p90 83.7 KB, max 688.6 KB). The real tokens replicated to 10,000 and 100,000 parts: 42.6 and 427.5 MB of tokens and postings; a full token load, parse and index takes 167 ms and 1,729 ms; one posting range query takes 1.15 ms and 13.7 ms |
| `sq.rs fsync` | A 30 KB blob: 0.21 ms unsynced, 4.05 ms with `fsync(2)` and a directory sync, 7.90 ms with `sync_all` (F_FULLFSYNC) and a directory sync |
| `psync.rs` | `sync_all` per blob with one directory sync per batch: 3.30, 3.27 and 3.31 ms a blob on 1, 4 and 8 threads |
| `sq.rs commit` | A one-row commit in WAL: 0.09 ms with `synchronous=NORMAL`, 0.12 ms with `FULL`, 3.74 ms with `FULL` and `fullfsync=ON` |
| `sq.rs conc` | 8 processes × 200 read-then-write transactions: `DEFERRED` gave 1,543 failures and 57 rows; `IMMEDIATE` gave 0 failures and 1,600 rows in 780 ms |
| `sq.rs snapshot` | A reader's snapshot survives a committed delete and a checkpoint attempt (busy); WAL 13.0 MB, then 0 after the reader ends |
| `sq.rs crash` | 300 kill -9 rounds of the ingest protocol: every invariant held every round; the row-first control broke in round 0 |
| `race.rs` | gc against four ingest/delete processes for 20 s: 0 of 5,120 checks missing; the unlink-after-commit control: 6 of 5,301 |
| `sq.rs full`, `diskfull.rs` | `max_page_count` 200 gave `SQLITE_FULL` after 26 commits, integrity ok. On a 16 MB APFS image, the blob write failed with ENOSPC (temp file removed) and SQLite failed with "database or disk is full", with integrity ok |
| `sq.rs unordered` | `reverse_unordered_selects` reverses a `SELECT` without `ORDER BY` and leaves `ORDER BY` alone |
| `hash.rs` | SHA-256 of 7.4 MB: core 327 MB/s, `sha2` 535 MB/s, equal digests |
| `cargo +1.85 build` | rusqlite 0.40.2 with bundled SQLite 3.53.2 builds on Rust 1.85 |
