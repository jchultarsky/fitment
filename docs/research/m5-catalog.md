# fitment M5: the catalog

As of 2026-10-16. Status: **proposal for review**, merged from three designs and revised after its soundness review (`m5-evidence/review/soundness.md`; section "Review applied" at the end lists where each finding landed). Nothing here is implemented; section 10 is the order of work. The rest of the document assumes every default in section 2.

M5 delivers the catalog. Part files pass the canonical gate and are stored by content and indexed. The CLI lists, reads, replaces and deletes them, and a socket is matched against them from the index alone. PLAN's exit for M5: "Ingest, list, get, replace and delete work from the CLI; a match reads only the index." This document is that design.

Three agents designed one area each, to one brief:

- **A:** the store and its integrity: blobs, the database, crash and race safety, deletion, re-indexing, verification.
- **B:** the index and the query path at catalog scale: what is stored per part, level 1 in SQLite, the query from 10³ to 10⁵ parts, and why the index neither passes a bad part nor hides a good one.
- **C:** the catalog as the user sees it: the CLI, identity and CRUD, quarantine, and D10 (properties) read off the fixtures.

Where and how they measured:

- **A:** on fitment `a42853c` (branch `m4/exit`: `EXTRACTOR_VERSION` 6, `SIGNATURE_VERSION` 1, `MATCH_RULES_VERSION` 1), with rusqlite 0.40.2 and bundled SQLite 3.53.2, Rust 1.99.1 (and a build check on 1.85), on macOS with APFS.
- **B:** on a snapshot of the same tree with M4's PR 7 uncommitted changes, with rusqlite 0.40 bundled, on an Apple M5 Max (18 cores, 64 GB).
- **C:** on `origin/main` and the `m4/exit` worktree, running the release `fitment features` and `stepq props` over the 45 fixtures.

A coordinator settled eighteen points before the merge (M5-1 to M5-18). The merge settled the rest where the designs disagreed (M5-19 to M5-39). The coordinator settled the soundness review's findings and the merge's open questions afterwards (M5-40 onward, and changes to earlier rows, each marked "review"). Section 14 says what each designer lost and why.

**Rule ids.** Rule ids are renumbered into one scheme by area, and each rule names its origin in brackets, as (A ST5) or (B IX5). Two prefixes differ from the brief:

- the query's rules are QR, not Q;
- the store's invariants are IN, not I.

M3's Q and I rules already hold those letters in the coverage registry (`tools/gen-corpus.py`), so the coverage test would confuse them. Test ids (CT) are not rule ids and are never written in the bold rule form.

**Evidence.** The designers' reports are `m5-evidence/A.md`, `B.md` and `C.md`, the review `m5-evidence/review/`, and their experiments' sources and small results `m5-evidence/A/`, `B/` and `C/` (the build trees, regenerated corpora and large raw outputs are not committed). They are cited by path, as `A/exp/src/bin/race.rs`, `B/out/dedup.txt` and `C/proto/run.txt`.

## 1. Summary

- **A catalog is a directory** made by `fitment catalog init -C DIR`: one SQLite database, and the original STEP bytes as files named by their SHA-256. Every command names it with `-C/--catalog DIR`; there is no default and no environment variable (M5-9).
- **An entry has three names, each for one job:** the user's **id** (`BRKT-0042`), unique among live entries without case and defaulting to the file's stem; its **revision**, which never repeats, even across delete and re-ingest; and its **SHA-256**, the content judged. Inside the crate an integer key, never reused, ties the rows together. PRODUCT.id is shown but never a key: the fixtures give two parts the PRODUCT.id `'1'`, and one part four spellings across five exports (M5-1). A strict `ID@rN` names one revision; a replace keeps the old revision's bytes until `fitment catalog prune --revisions` (M5-41).
- **The store's safety rests on two rules:** a blob, its directory entry included, is durable before the commit that names it, whether this ingest renamed it into place or found it there; and a blob file is unlinked only under SQLite's write lock after a re-check that no row names it. A measured 300 kill -9 rounds without a broken invariant (a row-first control broke in round 0), and a 20 s race of ingest, delete and gc with 0 rows without a blob in 5,120 checks (an unlink-after-commit control found 6).
- **Derived rows are keyed by content and version, never by entry,** and are insert-only: the gate's compact `features.v1` document by (SHA-256, `EXTRACTOR_VERSION`); the level-1 tokens and postings also by `SIGNATURE_VERSION`; the part-level properties by `PROPERTIES_VERSION`. Each row's hash covers a preimage that names the row (its content SHA-256, versions, status and part digest, then the bytes), checked on every load, so a document moved to another content's key fails its hash (M5-40). A version rise adds rows beside the old ones, so a re-index resumes and two fitment versions can share a catalog.
- **Verification runs from the stored document alone.** 243 of 243 accepted documents (A) and 232 of 232 corpus files (B, pretty and compact) round-trip byte for byte, and `judge_step` on the file equals judging the stored document in 90 of 90 corpus match cases (A, digest included) and 105 of 105 (B). A new `Matcher::judge_document` refuses a document of another `EXTRACTOR_VERSION` and says which; it and `judge_step` share one private path, and `judge_step` still hands the fit request its STEP bytes (M5-10).
- **Level 1 is one algorithm in core,** over an `IndexSource` that the in-memory index and the SQLite tables both implement, with each part tested at its own lever so its admission never depends on the rest of the catalog. Pair tokens become compact and `SIGNATURE_VERSION` rises to 2 (M5-11). The pair filter skips lines of the selective line's SG5 group, as M4's exit PR already made it do (IX7).
- **A catalog run is a file run restricted to level 1's survivors.** Each survivor is judged as `fitment match` judges a file, M4-17 included (a level-2 drop is still aligned, verified and listed as a `Reject` at stage `shortlist`), so its entry is byte-identical to the file run's, labels aside. The document is `fitment.match` v1 unchanged, labels are `catalog:<id>@r<n>`, and the catalog's counts go to stderr and the table header (M5-2, M5-4). Survivors are grouped by the part digest recomputed from each checked document, never by the stored column (M5-12). At 10⁵ parts, 20 sockets gave the same digest with level 1 and with it bypassed; the documented `--exhaustive` bypasses it, so tests, and users, can show that on any run (M5-42).
- **Staleness is refused (exit 3), with no override:** stale tokens can hide a good part and stale features can pass a bad one. `fitment catalog reindex` rebuilds tokens from the stored documents when only the signature version rose, and re-reads the STEP files, quarantined ones included, when the extractor version rose (M5-3).
- **D10:** no fixture states a material or a thread in a form fitment can use. M5 captures part-level properties at ingest, bounded, and ships one evidenced mapping row (NX's `MaterialMissingAssignments` = `TRUE`: no material assigned). Nothing changes in verdicts or flags; `fitment catalog get` shows them (M5-5).
- **Cost at 10⁵ parts** (B; corpus parts at 439 scales each; 16 threads): 8 to 415 ms a query, level 1 0.2 to 45 ms, a 1.32 GB database with compact documents. Judging the parts that really fit dominates; judging once per distinct `Part` turned 48.5 s into 1.76 s on 10⁵ exact copies, with a byte-identical document. These times end a candidate at a level-2 drop; M5-4 judges the drops too, and PR 4 measures what that costs at 10⁵ (risk R2). If one query takes over 60 s on 16 threads, the contingency is M5-43: level-2 drops counted, not listed, with `--exhaustive` restoring them.
- **What `verify` proves without `--deep`:** every row's preimage hash, every part-digest column against its document, and every posting against its hash-checked tokens (M5-40). A corrupt posting can hide a part until `verify` runs, and can never pass one (M5-32, R20).
- **Seven PRs:** core API and the `write_atomically` fsync fix first, M5's exit test in PR 4, the release in PR 7 (section 10).

## 2. Decisions for review

Each row has a default. Rows marked **Your call.** change what fitment promises, stores or prints; the rest are engineering defaults. M5-1 to M5-18 were settled by the coordinator, and M5-19 to M5-39 by the merge, where the designs disagreed or left a gap. M5-40 to M5-44, and every change marked **Review:** in an earlier row, apply the coordinator's resolutions of the soundness review (F1 to F7 are its findings) and of the merge's open questions.

| # | Question | Default | What it costs or buys | Alternatives |
| --- | --- | --- | --- | --- |
| M5-1 | **Your call.** What names a catalog part | **The handle:** a user-facing string id, `[A-Za-z0-9][A-Za-z0-9._+-]{0,63}`, unique without case among live entries, defaulting to the file's stem (ID1 to ID5, C's CI rules). **Internally:** an integer key, never reused (A). **Revisions:** never repeat per id, across delete and re-ingest (ID6, C). **The content:** the SHA-256. **PRODUCT.id:** shown, never a key (ID8) | **Buys:** replace keeps the user's handle; `(id, revision)` names one SHA-256 forever; a deleted id's history stays readable. **Costs:** two names to keep apart (the key never leaves the crate); a default id can surprise (every mapped id is printed, and `--id` names one exactly) | A's integer entry id (`e17`) as the only handle. The SHA-256 alone, which makes "replace" meaningless. PRODUCT.id as the key, which collides (`'1'` in `clevis21` and `gear`) and diverges (`L-BRACKET`, `LBRACKET`, `l-bracket`, `l_bracket`) in the fixtures |
| M5-2 | **Your call.** The document a catalog run writes | **Format:** `fitment.match` v1 unchanged (B8). **A candidate's `source`:** the stored file's own (SHA-256, bytes, header). **Its one label:** `catalog:<id>@r<n>`, the id as stored and the revision judged (**Review,** F4(b): the label lies outside the digest, so the revision costs nothing; `ID@rN` is also a strict reference, M5-21). **The counts** (entries, searched, quarantined and not searched, level-1 survivors) and the generation go to stderr and the table header, not into the document | **Buys:** every entry compares byte for byte with a file run's, and the frozen format holds; a past document names the revision whose bytes it judged, and those bytes stay retrievable after a replace (M5-41). **Costs:** the document alone does not say how much of the catalog was searched; M6's stored run record must | C's `fitment.match` v2: v1 plus a top-level `catalog` object (generation, a contents digest, the counts), a per-candidate `catalog` object (id, revision), a `properties` record and a `material_differs` flag. The merge's `catalog:<id>` without the revision |
| M5-3 | **Your call.** A catalog with stale entries | **Refused,** exit 3, with the count and `fitment catalog reindex`; no `--allow-stale`. **Stale:** a live entry whose features are not at this build's `EXTRACTOR_VERSION` (either status), or an accepted one whose tokens are not at its `SIGNATURE_VERSION`; older or newer alike (RX2). **Reindex:** rebuilds tokens from the stored features when only `SIGNATURE_VERSION` differs; re-reads STEP (quarantined files too) when `EXTRACTOR_VERSION` does | **Costs:** after an upgrade nothing can be searched until reindex ends. Measured: 10⁵ corpus parts re-read in 4.9 s at 16 threads. Estimated: 10⁵ real parts at 7.9 ms each, about 13 min of CPU, about 1 min at 16 threads. **Buys:** stale tokens can hide a good part, and stale features can pass a bad one; neither can happen | A's `--allow-stale` (judge current rows only and say how many were skipped). Searching the current entries with a warning (C's alternative) |
| M5-4 | **Your call.** What a full catalog run lists | **Listed:** exactly the file run's document restricted to the level-1 survivors. Each survivor is judged with the file run's semantics, M4-17 included: a level-2 drop is still aligned and verified, and listed as `Reject` at stage `shortlist`; a pass after a drop exits 1 as a fitment bug. Each entry is byte-identical to the file run's, labels aside. **Not listed:** level-1 drops, which are only counted. **`--exhaustive`:** a documented mode that bypasses level 1 and judges and lists every live entry (M5-42). **Review,** the merge's open question 1: M5-4 is kept. PR 4 measures it at 10⁵; if one query exceeds 60 s on 16 threads, M5-43's contingency applies | **Buys:** KH9's check on every survivor; the exit test becomes a digest comparison. **Costs:** every survivor that level 2 drops is aligned and verified. Unmeasured at scale: at 10⁵, s02's block has 8,844 survivors of which level 2 keeps 36, and a seats-only socket 98,766 of which 202. Their time and document size are measured in PR 4 (R2) | B's QR5: a level-2 drop ends the candidate, unlisted. Listing level-1 drops as well |
| M5-5 | **Your call.** D10, properties | **Capture:** part-level properties at ingest, bounded to 1,024 values of at most 4 KiB, versioned by `PROPERTIES_VERSION`. **The mapping table:** C's one evidenced row (`MaterialMissingAssignments` = `TRUE` → material unassigned). **No change in M5** to verdicts or flags: `material_not_compared` stays on every verdict; `thread_not_confirmed` is VR19's, unchanged. **Display:** `fitment catalog get` only, not the match document. **Future rows:** each must cite a real sample file and record | **Costs:** properties stored and versioned before anything uses them. **Buys:** a positive sample later needs a mapping row and a comparison, not a new capture | C's whole design: a `properties` record per candidate in a v2 document, material compared on exact stated equality, `material_differs`, the requirements needing a thread check, the original's properties through `--original`. Or capture nothing until a positive sample exists |
| M5-6 | Delete | **A tombstone:** keeps the id, its revisions and their SHA-256s. **Removed in the delete's transaction:** the derived rows and the blob rows of its current and kept revisions, where no other live entry holds them (IN3). **The file:** unlinked by gc, only under SQLite's write lock, after re-checking that no row names it (CR1, ST5). A replace, unlike a delete, keeps the old revision's blob (M5-41) | **Costs:** a deleted part cannot be re-judged (its blob is gone). **Buys:** a withdrawn part is never offered again; ids are never reused; old documents stay auditable; the race safety measured above | C's unlink right after the commit; a trash directory; A's `--keep-file` (not offered) |
| M5-7 | Replace with a file the gate refuses | Refused, exit 3; the entry, its bytes and its index are unchanged (CR4) | **Costs:** taking a part out of the search is a delete. **Buys:** a broken export never silently drops a searchable part | A's `--quarantine` override |
| M5-8 | Quarantine | **Kept:** the file and the gate's refused `features.v1` document (header and refusals); never tokens or postings. **Retried by:** reindex, when the extractor rose or when named. **Fixed by:** replace | **Costs:** refused bytes take space. **Buys:** soundness rule 6's reason in the format users already read; a fixed gate can admit the file | B's quarantine row without the document |
| M5-9 | Where the catalog is | `-C/--catalog DIR` only: no environment variable, no default | **Costs:** typed every time. **Buys:** no doubt about which catalog a run used | A's `FITMENT_CATALOG`; a default directory |
| M5-10 | Stored features | **Storage:** compact `features.v1` JSON in SQLite (B), keyed by (SHA-256, `EXTRACTOR_VERSION`), with the SHA-256 of a preimage naming its row checked on every load (A; **Review,** F2: M5-40). **Core:** a new `Matcher::judge_document` refuses a document of another `EXTRACTOR_VERSION` and reports the document's version; it takes the `Source` built from the row's own checked content SHA-256 (QR7). **Review,** F5: `judge_step` and `judge_document` share a private path that takes the STEP bytes; `judge_step` passes its bytes to `FitRequest.candidate_step`, so M7's file runs keep their fit check, and `judge_document` is used for stored documents | **Costs, measured:** 1.32 GB at 10⁵ corpus parts (3.12 GB pretty). **Costs, estimated:** about 19 GB at 10⁵ fixture-like parts. **Buys:** one snapshot covers a whole match, no blob is opened, and verification needs nothing else | A's exact pretty bytes; documents as files beside the blobs; a stored `PartSignature` (13.2 KB against 135.7 KB on NIST-like parts, but it needs a reader and can go stale) |
| M5-11 | Level 1 | **One algorithm in core:** `level_one` over an `IndexSource` trait (B); the in-memory index and the SQLite tables are both sources. **The lever:** each part at its own, with seat groups bucketed by powers of two, the bucket never below the lever: `max(bucket, lever)` (**Review,** IX5). **Pairs:** lines of the selective line's SG5 group are skipped, as M4's exit PR already does in `ShortlistIndex::pairs_hold` (**Review,** F3: inherited and done; `level_one` keeps it). **B4:** compact pair tokens, `SIGNATURE_VERSION` 2, the match goldens re-blessed in PR 1 | **Costs:** a version rise and a re-bless; M4's level 1 changes (never used by a file run). **Buys:** no shortlist logic in SQL; a part's admission independent of the catalog; a 20 to 25 ms scan per query gone; s01's level 1 down from 286 to 33 ms | A's `PostingSource` at the catalog's largest lever; tokens kept at version 1 (a re-index later) |
| M5-12 | Judging the same part twice | Judge once per `Part` digest while no fit verifier names a kernel (B); the result is reused, with its own source, for each entry of that digest. **Review,** F1: the group key is the digest recomputed from each loaded, hash-checked document, never the stored column; a column that disagrees is corruption, so that row is skipped and named and the run exits 1 (QR4, QR6) | **Costs:** a digest per accepted part, stored, and one recomputed per survivor in each run (a compact serialization of its `Part`). **Buys:** 48.5 s became 1.76 s on 10⁵ copies, with a byte-identical document. M7 must ask the fit per member | Judge every entry |
| M5-13 | Exit codes | **As M4:** 0 `Match`, 4 `InterfaceMatch`, 5 every candidate rejected. **3:** refused input, and also a search that could not run: a stale catalog, nothing indexed, a socket that blocks every query (C). **1:** error. **2:** usage | **Costs:** 3 is wider. **Buys:** 5 keeps meaning "searched, and nothing passed", so a script never orders a new part because the catalog was empty or stale | B's 5 for an empty catalog and a blocked socket |
| M5-14 | STEP bytes in a package | None. A's embedded extractor canary (M5A-16) is rejected because M4's PR 7 made it a packaging rule that no package holds a STEP file (`tools/check-core.sh`) | **Costs:** an extractor that changed without a version rise (a development build, a forgotten bump) can write rows that look current. The goldens, the version discipline and `verify --deep` remain the checks (attack 4) | A's canary: three probe parts embedded in `fitment-catalog`, their documents' hashes recorded per `EXTRACTOR_VERSION` |
| M5-15 | Concurrency, durability, size, location | **Writes:** `BEGIN IMMEDIATE` for every one (A measured 1,543 of 1,600 failures under `DEFERRED`, 0 of 1,600 under `IMMEDIATE`). **Durability:** full fsync (blob `sync_all`, `F_FULLFSYNC` on Apple; WAL with `synchronous=FULL`; `fullfsync=ON` on Apple). **Review,** F6: the fan-out directory of every blob a commit names is synced before that commit, including a blob found already in place and reused (ST3). **Size:** a cap of 128 MiB per file. **Location:** network file systems and sync folders unsupported and documented | **Costs:** about 3.3 ms per blob and 3.74 ms per commit on the Mac, so 10,000 files cost about 33 s of syncing. **Buys:** a committed delete survives power loss | C's 256 MiB; `synchronous=NORMAL` |
| M5-16 | `write_atomically` | PR 1 adds the fsync of the temporary file before the rename, and of its directory after, in the existing CLI (C's finding) | **Costs:** a sync per `socket -o` and `review` write. **Buys:** a crash can no longer leave an empty file where a reviewed socket was | A separate PR |
| M5-17 | Out of M5, waiting for M6 | Stored assemblies, sockets and reports; assembly leaves as catalog parts. **Review,** the merge's open questions 4 and 6, deferred to M6: whether the server holds level 1 in memory, and whether a stored run records the generation and CM8's counts | **Costs:** users keep their socket files; the corpus's 10 component cases stay file runs. **Buys:** M5 stays its exit. Section 6.3 reserves the schema's shape | Store them now |
| M5-18 | SQLite access | rusqlite with bundled SQLite in the new synchronous crate `fitment-catalog`. fitment-core stays free of I/O. Only `fitment-catalog` may depend on rusqlite (`deny.toml`) | **Costs:** `cargo install` needs a C compiler; `hashbrown` and `foldhash` join the graph, never core's. **Buys:** D5 as decided | — |
| M5-19 | Command names | **Catalog commands:** `fitment catalog init`, `ingest`, `list`, `get`, `replace`, `delete`, `reindex`, `verify`, `gc`, and `prune` (**Review,** M5-41). **A catalog match:** `fitment match SOCKET --catalog DIR` (B, C) | **Buys:** PLAN's exit names its verbs; one match command whatever the candidates | C's `add`, `remove` and `quarantine`; A's `fitment catalog match` and `log` |
| M5-20 | The same bytes added twice | **Never stored twice.** **Under the holder's own id:** `unchanged`. **Under a default id:** `duplicate of X`, exit 0, since the content is searchable. **Under `--id`:** exit 2, since the id asked for was not created (ID5, C) | **Buys:** a nightly `ingest vendor/` does not fail forever on a file the vendor ships twice | A's "always exit 0"; aliases (one file, several ids) |
| M5-21 | How a command names an entry | **Accepted:** `ID` (case-folded), or `sha256:HEX` with at least 12 hex digits, matching one live entry. **Review,** F4(b): also a strict `ID@rN`, one exact revision of that id. `get` takes any revision of it; every other command only the current one, so a script can say which revision it means to change. **`delete`:** takes ids, bare or `@rN`, only. **`get`:** also shows an id's tombstone (ID7) | **Buys:** a typo or a prefix never deletes the wrong part; a `catalog:<id>@r<n>` label is itself a reference | A's `e17`, its 8-digit prefix and its exact `product_id` |
| M5-22 | Named entries | `--catalog DIR --id ID...` judges the named entries as explicit candidates: level 1 is not used, a blocked socket does not stop them, and a quarantined one is a refused candidate. The digest equals a file run over their files (CM3, C) | **Buys:** a catalog part can be re-checked by name without finding its file | Only full runs |
| M5-23 | Time | **The generation:** one counter, +1 per write transaction, with an `op` row per entry or content it touched. **Wall time:** none stored (C) | **Costs:** no "when" in the catalog. **Buys:** listings and history identical across runs and OSes | A's `op.at_unix_ms` with `fitment catalog log` |
| M5-24 | Write transactions | **Batches:** ingest and reindex commit at most 64 files or 1 s of work each, all or nothing (A). **Waiting:** `busy_timeout` 60 s, then exit 1, "the catalog is busy" | **Costs:** a full disk rolls back up to 64 files of a batch. **Buys:** one 3.74 ms commit per batch, not per file | One transaction per part (B, C); C's 10 s timeout |
| M5-25 | Repair | **`verify`:** only reads, and exits 3 when it finds a problem. Without `--deep` it checks every preimage hash, part-digest column and posting (**Review,** M5-40). **`gc`, the one repair:** orphan blobs, temporary files older than 1 h, with `--versions` rows of older versions, with `--vacuum` `VACUUM`. `prune --revisions` (M5-41) is not a repair: it drops history the user chose to drop | **Buys:** one command that can unlink, under one rule (ST5) | C's `verify --repair`; A's exit 1 for findings |
| M5-26 | A `PROPERTIES_VERSION` mismatch | **Marks the entry:** `stale (properties)` in `list`; reindex re-captures. **A match:** never refused for it | **Buys:** an upgrade that changes only the capture never blocks a search, and nothing a match reads depends on properties (M5-5) | C's RX1, which refuses the run |
| M5-27 | Layout | A's: the marker `FITMENT-CATALOG`, `catalog.db`, `blobs/sha256/<2 hex>/<64 hex>` and `tmp/` | **Buys:** a later hash is a sibling directory | C's `catalog.sqlite`, `blobs/<2 hex>/<64 hex>` and `blobs/tmp/` |
| M5-28 | How derived rows are keyed | **By content and version, insert-only,** enforced by triggers (A). **Level 1's ids:** `blob.key`, an `AUTOINCREMENT` content key (merge) | **Buys:** replace and delete never rewrite derived rows; a re-index adds rows; an id read by level 1 never names other content | B's one `part` row per file, carrying its versions and updated by re-index |
| M5-29 | Two fitment versions on one catalog | **Reads and writes:** each build reads only its own version's rows and never writes a newer version's. **gc `--versions`:** removes only older versions' rows, and only where current ones exist (RX8, A) | **Buys:** going back to an older fitment after an upgrade loses nothing | B's RX3: a catalog written by a newer fitment refused by every command but `list` |
| M5-30 | Parallel judging | `--jobs N`, default `available_parallelism()`, in catalog runs (B11) and, **Review** (the merge's open question 3), in file runs too: a file run reads and judges at most N files at once, so MF3's one-at-a-time becomes N-at-a-time, and `--jobs 1` keeps M4's memory. In both, the output does not depend on N | **Costs:** memory for up to N documents, or N STEP files and their graphs, at once. **Buys:** 4 to 8 times less wall time on queries over 100 ms; one flag with one meaning for both kinds of run | Sequential file runs, as M4 left them |
| M5-31 | Rejects in a catalog run's table | Every pass, then at most 10 rejects (`--rejects N` or `all`); the document holds every candidate (C) | **Buys:** a readable table when level 1 admits thousands | Every candidate always; B's first 200 |
| M5-32 | A stored row that fails its checks during a run | **That candidate:** not judged, named on stderr with `fitment catalog verify`. **The rest:** judged. **Exit:** 1, as M4-21 does for an unreadable file (A). **The checks** include the preimage hash and the recomputed part digest (M5-40) | **Buys:** a corrupt row that a run reads is never judged and never silently skipped. **Review,** F7, the wording corrected: postings carry no hash and a run reads only those its probes return, so a corrupt or lost posting can hide a part (exit 5 where 0 or 4 was due) until `verify` runs; it can never pass one (IX9, R20) | B's "fail the run"; skipping silently |
| M5-33 | Directory arguments | Not recursive; `.stp`, `.step`, `.p21` in any case: M4's MF1 rule | **Buys:** one directory rule for `ingest` and `match` | A's `--recursive` |
| M5-34 | Compression | None; an `encoding` column (`json` only) is reserved (A, B3) | **Costs:** see M5-10. **Buys:** no new dependency. Compact JSON is 37.3% of pretty JSON's bytes; deflate would be 8% | zstd or deflate now |
| M5-35 | Plane tokens (grip separation, seat area) | Deferred (B5) | **Costs:** a seats-only socket admits 98,766 of 100,083 parts at level 1 (measured with drops ending the candidate: 2.2 s of wall time, about 10 s of CPU) | A grip-separation token: 5.8% admitted at 10⁴ |
| M5-36 | A limit on candidates or time | None (B10) | **Costs:** 9,219 interface matches made a 111 MB document | A ranked `--limit`, which needs a ranking MF5 does not define |
| M5-37 | D8 and D9 | Stay open with M4's defaults; revisit when real catalog parts arrive (C) | — | — |
| M5-38 | Release | **Crate:** `fitment-catalog` published as internal and unstable, behind the CLI's default `catalog` feature. **Version:** 0.2.0 for all crates. **New format:** `fitment.catalog` v1 for `list` and `get` | — | — |
| M5-39 | `--catalog` with candidate files | A usage error, exit 2 (B9, C) | **Buys:** one set of semantics per document | Mixed runs |
| M5-40 | A derived row bound to the content it is stored under (review, F1, F2, F7) | **Hashes:** each derived row's hash covers a preimage naming the row (GT2): a `features` row's content SHA-256, extractor version, status and part digest, then the document bytes; a `tokens` row's content SHA-256, extractor version and signature version, then the token bytes; a `properties` row's content SHA-256, properties version and truncation, then the stated bytes. **Dedup:** by the part digest recomputed from each loaded, hash-checked document (M5-12). **The source:** built from the row's own checked content SHA-256 (QR7). **Plain `verify`:** checks every preimage hash, every part-digest column against its recomputed digest, and every posting against postings recomputed from the hash-checked tokens (VF1) | **Buys:** a document read from file X and stored under key Y, a copied row, or a digest column naming another part, fails on the match path itself and in plain `verify`, not only in `--deep`. **Costs:** a compact serialization of each survivor's `Part` per run, and a parse of every document and tokens row per `verify` (measured in PR 3 and PR 4). A wrong value written consistently by ingest itself (a wrong document for the right key) is still `--deep`'s to find | The review's alternatives, one of the two: recompute only, or bind only. Both are taken: the binding is free on load, and only the recomputation catches a digest written wrong at ingest |
| M5-41 | **Your call.** The bytes a past report judged (review, F4(b)) | **Replace keeps the old revision's blob** (row and file), without its derived rows, while the entry is live, so `get ID@rN --format step` writes the bytes revision N had (IN3). **`fitment catalog prune -C DIR --revisions`** drops every kept blob of a superseded revision; it takes the write lock as gc does, records its `op` rows, and its inline gc unlinks the files (ST5). Delete still drops them (M5-6). **`get --format step`** re-hashes the bytes before writing, and on a mismatch exits 1 with "damaged" and `fitment catalog verify` (F4(a), ST11) | **Buys:** a past document's `catalog:<id>@r<n>` and its SHA-256 still lead to the exact bytes judged, and a blob altered in place is never handed out as the matched part. **Costs:** a catalog replaced often holds every old revision's STEP bytes until pruned; `list`'s stderr line counts them (R21) | The merge's default: the old blob unlinked by the replace's inline gc, so the judged bytes were lost |
| M5-42 | Bypassing level 1 (review) | **`--exhaustive`**, documented in `fitment match --help` and the README: level 1 is bypassed and every live entry is judged and listed, quarantined ones as refused (QR10). It replaces the hidden `--audit`. Tests use it to show that level 1 hides nothing (CT2, and the F3 and F7 cases) | **Buys:** a user who suspects a miss can check one run; R18's only on-catalog guard is no longer hidden. **Costs:** at 10⁵, about 1 s of wall time and 10 s of CPU before M5-4's drops are judged, more with them | The hidden `--audit` |
| M5-43 | Contingency for M5-4's cost (review; the merge's open question 1) | **Not taken unless measured:** PR 4 runs CT11 at 10⁵ on 16 threads. If any one query takes over 60 s, a full run counts level-2 drops instead of listing them (stderr and the table header: `level 2 dropped 8808 (not listed: --exhaustive lists them)`), and `--exhaustive` restores them as M5-4 lists them. The decision and its measurement are recorded in PR 4's "As built" note | **Costs, if taken:** a full run's document is no longer the file run's restricted to the survivors, but to level 2's kept parts, so CT1 compares against that, and KH9 runs on drops only under `--exhaustive`. **Buys:** a bounded query on unselective sockets | Ending drops now, without a measurement |
| M5-44 | Catalogs per run (review; the merge's open question 5) | **One.** `--catalog` given twice is a usage error, exit 2 (CM1) | **Buys:** ids, labels and the generation each name one catalog | Several catalogs, which would need a catalog name in every label |

## 3. How the catalog fits together

### 3.1 The flow

```text
fitment catalog ingest -C DIR a.stp dir/        fitment match socket.json --catalog DIR
  │ per file, in ID4's order, no lock:            │ socket gate (MF2) ─────────────────── refused: exit 3
  │   read once (≤ 128 MiB + 1), SHA-256          │ open DIR: marker, application_id, schema ─ newer: exit 3
  │   FeaturesDocument::read   (the gate)         │ BEGIN (one read snapshot, generation G)
  │   accepted: tokens, postings, part digest     │ stale entries? ──────────────────────── exit 3 (RX3)
  │   properties::read (bounded)                  │ nothing indexed? ────────────────────── exit 3
  │   blob: tmp → sync → rename → 0444            │ level 1: Matcher::admitted(&SqlSource) ─ blocked: exit 3
  │     (or reuse an equal final name)            │   core's level_one over SQL postings, each part at its lever
  │ sync the fan-out dir of every blob named      │ load each survivor's document: preimage SHA-256, from_json,
  │ BEGIN IMMEDIATE  (≤ 64 files or 1 s)          │   current, part digest recomputed = column
  │   live elsewhere? → unchanged / duplicate     │ group by the recomputed digest; one judgment per group
  │   blob present? (else rewrite from buffer)    │ judge_document(doc, Source from checked row), --jobs threads,
  │   blob, entry, revision, features, tokens,    │   file-run semantics: M4-17 on every survivor
  │   postings, properties, op; generation += 1   │ MatchDocument::new (MF5 order, labels catalog:<id>@r<n>)
  │ COMMIT                                        │ END; table or JSON; stderr: the counts; VD14's exit
  ▼                                               ▼
DIR/catalog.db  ◄────────────────────────── reads only this
DIR/blobs/sha256/ab/ab12…  (read by ingest, reindex, verify and get --format step, each re-hashing; never by a match)
```

Delete, replace, reindex, prune and gc take the same write lock (`BEGIN IMMEDIATE`). Ingest and replace write blobs; reindex, `verify` and `get --format step` read them, hashing what they read before using it (ST11); gc unlinks them, including those `prune --revisions` released. A match never touches one.

### 3.2 Crates and modules

```text
fitment (CLI) ──> fitment-catalog ──> fitment-core ──> stepq (private)
      │                  └─> rusqlite (bundled SQLite)
      └─────────────────────────────> fitment-core
```

`fitment-catalog` is synchronous, has no async runtime and no network, and is the only crate that depends on rusqlite. Its `clippy.toml` bans `HashMap`, `HashSet`, `SystemTime::now`, `std::env::var` and `std::process`. Its modules:

| Module | Holds | Rules |
| --- | --- | --- |
| `lib.rs` | `Catalog::init` and `Catalog::open`, `Options` (size cap, `max_db_pages`, busy timeout), the error types (`thiserror`) | DB1, PH2 |
| `fs.rs` | `trait Fs` (every file operation, and a `point` hook just before and after each `COMMIT`), `StdFs`; `CountingFs` and `FaultFs` behind a `test-support` feature | ST3, CT6, CT13 |
| `hex.rs` | `Sha256Hex`: exactly 64 characters `[0-9a-f]`, the only type a store path is built from | PH1 |
| `preimage.rs` | The row preimages and their hashes (`features_sha256`, `tokens_sha256`, `properties_sha256`), the only code that writes or checks a row hash | GT2, IX9, PR3 |
| `layout.rs` | The marker, the paths, the symbolic-link checks | ST1, PH2 |
| `schema.rs` | DDL, pragmas, `user_version`, the migration mechanism (no migration in M5) | DB1, DB5 |
| `blobs.rs` | `Blobs`: write, read, unlink under the lock. Reachable only from `Catalog` | ST1 to ST7 |
| `ids.rs` | The id alphabet, folding, default ids, references (`ID`, `ID@rN`, `sha256:HEX`) | ID1 to ID7, ID9 |
| `ingest.rs`, `change.rs`, `gc.rs`, `reindex.rs` | Ingest; replace, delete and `prune --revisions`; gc; reindex | GT, CR, ST5, RX |
| `snapshot.rs` | `Snapshot`: one read transaction and the build's versions, with no field that reaches `Blobs`; the stale counts; the reads of `list` and `get` | DB4, QR11, RX2 |
| `source.rs` | `SqlSource`, core's `IndexSource` over `posting` and `tokens` | IX1 to IX3 |
| `query.rs` | `match_catalog(&Snapshot, &Matcher, Selection, jobs) -> CatalogRun`: the gate order, level 1, load and check (`CheckedRow`, constructed only by QR4's checks), dedup, workers, the counts | QR, CM |
| `verify.rs` | `verify` (preimages, digests, postings recomputed) and `--deep` | VF |
| `document.rs` | The `fitment.catalog` v1 document | CL3 |

In `fitment` (the CLI):

- `cmd/catalog.rs` holds the subcommands;
- `cmd/match_.rs` gains `--catalog`, `--id`, `--jobs` (for file runs too), `--rejects` and `--exhaustive`;
- `write_atomically` moves from `cmd/socket.rs` to `cmd/mod.rs` with its fsyncs (CL7), so `socket -o`, `review`, `match -o` and `get -o` share it.

In `fitment-core` (section 5):

- `matching/index.rs` gains `IndexSource` and `level_one`;
- `matching/matcher.rs` gains `judge_document` and `admitted`, and a private path that `judge_document` and `judge_step` share, carrying the STEP bytes when there are some (F5);
- `features/document.rs` gains the compact form and the part digest;
- a new `properties/` module (`mod.rs`, `map.rs`, and `step.rs`, the one file in it that names stepq, added to `tools/check-core.sh`'s wall list).

## 4. Rules

Each rule is stated so that a test can fail it. The test that does is named in brackets and listed in section 9. The origin follows each rule's title: A, B or C with the designer's own id, or "merge" for a rule the merge wrote.

### 4.1 The file store (ST)

- **ST1. Content address** (A ST1, C ST1).
  - A blob's name is `sha256_hex(bytes)`, core's function. It lives at `blobs/sha256/<first 2 hex>/<64 hex>`, lower-case only, so a case-insensitive file system cannot collide two names.
  - Every final file under `blobs/` hashes to its own name. [CT12]
- **ST2. One buffer** (A ST2, B CS2, C CI8).
  - Ingest reads the source once, through one handle, into memory, with a limit of cap + 1 bytes.
  - That buffer is what is hashed, gated, read for properties and tokens, and written as the blob. The source path is never reopened, so a file rewritten during ingest cannot give a blob that differs from its document. [CT13: the seam swaps the source after the read]
- **ST3. Write protocol** (A ST3, C ST2).
  - **Write and rename:** `tmp/<pid>-<n>.part` opened with `create_new`, then `write_all`, `sync_all`, `rename` to the final name, and read-only.
  - **A final name that already exists** is re-hashed: if equal, it is reused; if not, the rename replaces it and the replacement is reported.
  - **Directories** (review, F6): the fan-out directory of **every blob a commit names** is synced before that commit, whether this process renamed the blob into place, found it already there and reused it, or rewrote it under ST6. A reused name may be another process's rename not yet synced (CC2), or the orphan of an ingest killed between its rename and its sync, so "touched by this process" is not enough. Each directory is synced once per commit however many of its blobs the commit names. On Unix the directory is opened and `sync_all` called. On Windows std cannot, so the sync is skipped (R7). [CT13: `fault.rs::every_named_blob_has_its_directory_synced_before_commit`, `fault.rs::a_reused_orphan_has_its_directory_synced`]
- **ST4. Immutable blobs** (A ST4, C ST9). A final blob is mode 0444 and never opened for writing. Its read-only flag is cleared only just before gc unlinks it, since Windows refuses to delete a read-only file.
- **ST5. Unlink rule** (A ST5; M5-6). A file under `blobs/` is unlinked only inside a `BEGIN IMMEDIATE` transaction that has just checked that no `blob` row names it.
  - Delete, replace, prune and reindex never unlink: they remove rows, and the inline gc after their commit, or a later `gc`, unlinks under this rule. A replace removes no `blob` row (CR3); a prune removes those it releases (CR11).
  - gc unlinks at most 1,000 files per transaction, so writers wait at most one batch. [CT14: the race test and its control]
- **ST6. Commit-time recheck** (A ST6). Inside its write transaction, ingest checks that the final blob exists with the right size before inserting its `blob` row. If not, it rewrites the blob from the buffer (ST3), its directory synced before the commit. With ST5, a committed `blob` row always has its file.
- **ST7. Temporary files** (A ST7). Every error path removes its temporary file. gc removes those older than one hour. The age decides only liveness, never safety: a removed temporary file fails its ingest, and nothing is committed.
- **ST8. No lock beyond SQLite's** (A ST8). SQLite's write lock is the catalog's only mutex. `File::lock` needs Rust 1.89 (the MSRV is 1.85), and a second lock would be a second order to get wrong.
- **ST9. Durability** (A ST9; M5-15).
  - Each blob is synced with `File::sync_all` (on Apple, `F_FULLFSYNC`).
  - The database is WAL with `synchronous=FULL`, plus `fullfsync=ON` and `checkpoint_fullfsync=ON` on Apple.
  - A blob is durable before the commit that names it: its bytes, and its directory entry (ST3, review F6).
- **ST10. Nothing else in DIR** (C ST8).
  - `init` refuses a non-empty directory that is not a catalog (exit 2).
  - `ingest` and `replace` refuse an input inside DIR (exit 2).
  - `match -o` and `get -o` refuse an output inside DIR (exit 2, the output untouched).
- **ST11. Bytes leave the store only checked** (review, F4(a); A IX7, C RX6). Whatever reads a blob reads it once into one buffer, hashes that buffer against the row's SHA-256, and uses only that buffer: `get --format step` writes it, `reindex` gates it (RX5), `verify` reports it, and M7's fit stage hands it over (QR13). On a mismatch nothing is written or indexed. `get` exits 1 with `damaged: the file's SHA-256 is not its name: run fitment catalog verify -C DIR`, and `-o`'s target is left untouched. ST4's mode 0444 is advisory, so this check, not the mode, is what stops an altered blob being handed out as the part judged. [CT16: `catalog_cli.rs::get_step_refuses_a_damaged_blob`]

### 4.2 The database (DB)

- **DB1. Engine** (A DB1, C ST6).
  - **The engine:** rusqlite with bundled SQLite (3.53.2 today), WAL, STRICT tables, `foreign_keys=ON`, `trusted_schema=OFF`, `cell_size_check=ON`.
  - **Identity:** `application_id` and the marker file identify a catalog; anything else is refused (exit 2).
  - **Statements:** every one is prepared with bound parameters, and none is built from user text. [CT15]
- **DB2. Every write transaction is `BEGIN IMMEDIATE`** (A DB2; M5-15), with `busy_timeout` 60 s. Every decision a write depends on ("is this content live?", "is this blob referenced?", "is this id taken?") is read inside the transaction that acts on it. [CT14]
- **DB3. Short write transactions** (A DB3; M5-24).
  - Gating, extraction, property capture, hashing and blob I/O all happen outside the write transaction. ST6's rewrite is the one exception.
  - Ingest and reindex commit batches of at most 64 files or 1 s of work.
- **DB4. One snapshot per read command** (A DB4, B QR2, C ST5). `list`, `get`, `verify` and a catalog match each run in one read transaction and report its generation. [CT9]
- **DB5. Schema versions** (A DB5, C ST7). `user_version` is the schema version, 1 in M5.
  - A binary refuses, for reading or writing, a catalog whose schema is newer than its own (exit 3: "this catalog was made by a newer fitment").
  - It migrates an older one forward only, in one `BEGIN EXCLUSIVE` transaction, recording an `op` row. No migration exists in M5; the mechanism and its test do. [CT8]
- **DB6. Constraint plus validation** (A DB6). Hex columns carry `CHECK`s, and the code validates every hex value it reads again, through `Sha256Hex`, before building a path. [CT15]
- **DB7. Derived rows** (A DB7; M5-10, M5-28).
  - `features` is keyed by (SHA-256, `EXTRACTOR_VERSION`), `tokens` by (SHA-256, `EXTRACTOR_VERSION`, `SIGNATURE_VERSION`), `properties` by (SHA-256, `PROPERTIES_VERSION`).
  - All are insert-only, by triggers. A match reads only rows of this build's versions.
  - Each row's hash covers a preimage that names the row (GT2, review F2), so the key a row sits under is checked with its bytes. [CT12]
- **DB8. The generation** (A DB8, B RX5, C CD-10; M5-23). Each write transaction raises `clock.generation` by one and writes one `op` row per entry or content it touched. The generation orders history. No wall time is stored.
- **DB9. Every query whose rows reach output or decide an order ends in `ORDER BY` over a unique key** (A OR1). The integration tests run every command twice, once with `PRAGMA reverse_unordered_selects=ON`, and require identical output. A measured that the pragma reverses an unordered `SELECT` and leaves `ORDER BY` alone. [CT7]

### 4.3 The gate at ingest (GT)

- **GT1. Every file passes M2's gate** (A GT1, B CS1, C CQ1), through `FeaturesDocument::read(buffer)`.
  - **Accepted:** a `blob` row, an `entry` and its current `revision`, a `features` row with status `accepted`, a `tokens` row with its `posting` rows, and a `properties` row. All in one transaction.
  - **Refused:** quarantined. The same rows, but the `features` row has status `refused` and holds the gate's refused document (header and refusals), with no `tokens` or `posting` rows.
- **GT2. The stored document** (B CS2, A GT2; M5-10).
  - **What is stored:** `doc` is core's compact form of the document read from the buffer (`FeaturesDocument::to_compact_json`).
  - **Its hash names its row** (review, F1 and F2; M5-40). `doc_sha256` is the SHA-256 of a preimage, not of `doc` alone. The preimage is the row's fields, each as ASCII followed by one `\n`, then the document bytes:

    ```text
    <content sha256, 64 hex>\n<extractor_version, decimal>\n<status: accepted|refused>\n<part_digest, 64 hex, or ->\n<doc bytes>
    ```

    Every field has a fixed alphabet without `\n`, so the encoding is injective. `tokens_sha256` covers `<content sha256>\n<extractor_version>\n<signature_version>\n<tokens bytes>`, and `stated_sha256` covers `<content sha256>\n<properties_version>\n<truncated: 0|1>\n<stated bytes>`. `preimage.rs` is the only code that builds them.
  - **What it catches.** A document read from file X stored under key Y, a row copied to another key, a status or digest column edited, or a version column changed, fails the load's hash check (QR4) and plain `verify` (VF1). What it cannot catch is a wrong value computed consistently at ingest. For the part digest, QR4's recomputation catches that too; for the document itself, `verify --deep` does.
  - **What `get --format features` prints:** `to_json()` of the parsed document, which is byte-identical to `fitment features FILE` with the same build. [CT5: every unit, corpus and fixture part; CT12: `store.rs::each_preimage_field_is_bound`, `store.rs::a_features_row_copied_to_another_key_fails_its_hash`, `store.rs::a_tokens_row_copied_to_another_key_fails_its_hash`]
- **GT3. Refused at the door, nothing stored** (A GT3, C ST10). The run goes on with the next file.
  - **A file over the cap (SZ1):** `refused`, exit 3.
  - **A path that is not a regular file (PH4):** `refused`, exit 3.
  - **An unreadable file:** `error`, exit 1.
  - **Not at the door:** an empty file or an image is not STEP, but the gate refuses it and it is quarantined like any other.
- **GT4. Quarantine never reaches the index** (A GT4). A trigger refuses a `tokens` row unless the `features` row of the same content and extractor version is `accepted`, and postings exist only through tokens. [CT12: inserting tokens for a refused row aborts]
- **GT5. Columns for people** (A GT5). `product_id`, `product_name`, the feature counts, `complete` and `first_refusal` are written from the document by the function that writes `doc`. They serve `list` and `get`, and are never read by matching.
- **GT6. Derived values come from core only** (B CS3, CS4).
  - **Tokens:** `IndexTokens::of(&PartSignature::of(part))`, stored with `IndexTokens::to_json`.
  - **Postings:** `IndexTokens::postings()`.
  - **The part digest:** `FeaturesDocument::part_digest()`.
  - The catalog computes no bin, token or digest itself.
- **GT7. Assemblies** (A GT7, B12; M5-17). The gate refuses an assembly (M2's `assembly` code), so it is quarantined. M5 stores no assembly's leaves.
- **GT8. An internal refusal is a bug** (C CQ4). A file refused with code `internal` is quarantined, so a later build can gate it again, and `ingest` exits 1 with "this is a fitment bug".
- **GT9. Nothing else is stored for matching** (B CS7, A RX7). No `PartSignature`, no `CandidateView`, no verdict, check, shortlist result or placement. A cached verdict would need every version, the policy, the socket digest and the fit verifier in its key, and one missed field is a false positive.

### 4.4 Identity (ID)

- **ID1. Ids** (C CI1). An id matches `[A-Za-z0-9][A-Za-z0-9._+-]{0,63}`. It holds no `/`, `\`, `:`, space, control or non-ASCII character, and never starts with `-` or `.`. An id outside the alphabet that the user typed is a usage error (exit 2), never mapped. [CT17: a property test over arbitrary strings]
- **ID2. Unique without case** (C CI2). No two live entries have ids equal under ASCII case folding. Lookup ignores ASCII case, and output shows the id as stored. [CT17]
- **ID3. Default ids** (C CI3). Without `--id`, the id is built from the file's name:
  1. drop its `.stp`, `.step` or `.p21` extension, in any case;
  2. replace each run of characters outside the alphabet by one `_`;
  3. drop leading `.`, `_`, `+` and `-`, and cut it at 64 characters.

  An empty result is a conflict: "no valid id: give --id". Each file's line shows the mapped id. For example, `a b.stp` becomes `a_b`; a name holding an escape sequence and U+202E became `evil_31mred_` in C's prototype. [CT17]
- **ID4. Exact stems first** (C CI4). In one `ingest`, files whose stem is already a valid id are processed before files whose id had to be mapped, each group in byte order of its path.
  - C's prototype found the need: in plain sorted order, `a b.stp` took `a_b.stp`'s id, and `copy of m_bracket.stp` took `m_bracket.stp`'s bytes, leaving the real file reported as a duplicate. [CT16]
- **ID5. One content, one live entry** (C CI5, A GT6; M5-20). No two live revisions have the same SHA-256, enforced by a partial unique index and checked inside the write transaction. Adding bytes that entry X holds:
  - **under X's id:** `unchanged`;
  - **under a default id:** `duplicate of X`, nothing written, exit 0;
  - **under `--id`:** exit 2.

  If two processes race, the loser reports the winner's entry. [CT14, CT16]
- **ID6. Revisions never repeat, keys are never reused** (C CI6, A DL1, DL5; M5-1).
  - **Revisions:** an id's revision starts at 1 and rises by one with every `replace`. The `name` table keeps an id's last revision across delete, so re-ingesting a deleted id continues from it: r1, r2, deleted, r3.
  - **Keys:** each new entry gets a new integer key (`AUTOINCREMENT`), so a re-ingested id is a new entry with a continued history.
  - So `(id, revision)` names one SHA-256 forever. [CT17]
- **ID7. References** (C CI7; M5-21). Commands that take an entry accept `ID`, `ID@rN` (ID9) or `sha256:HEX` with at least 12 hex digits, in either case, matching one live entry.
  - **No match:** exit 2, with up to five ids that share the longest prefix, case-folded.
  - **Several matches:** exit 2, with them listed.
  - **`delete`** accepts ids only, bare or with `@rN`. **`get`** also finds the latest tombstone of an id. The internal key is never accepted or printed. [CT16]
- **ID8. PRODUCT.id and file names are information** (C CI9; M5-1). Each is stored, escaped wherever printed, and shown. Neither is unique, neither is a key, and neither is ever read as a material, a thread or a size (PR6).
- **ID9. A strict revision reference** (review, F4(b); M5-21, M5-41).
  - **The form:** `ID@rN`, where ID is an id (ID1, case-folded), `@r` is literal and lower-case, and N is a decimal revision ≥ 1 with no sign and no leading zero. `@` lies outside ID1's alphabet, so the form is unambiguous. Anything else after an `@` (`BRKT@R1`, `BRKT@r01`, `BRKT@r0`, `BRKT@1`) is exit 2, never read as a bare id. It is exactly the part of a `catalog:<id>@r<n>` label after `catalog:`.
  - **`get`:** N may be any revision the id ever had, live or tombstoned. A superseded revision shows its metadata (`table`, `json`) and its bytes (`step`) while they are kept (CR3). Its `features` format, or its `step` format after a prune or a delete, exits 2: "BRKT r1's bytes are no longer kept (pruned at generation 52)". `get` writes on stderr the id, revision and full SHA-256 of what it printed or wrote.
  - **Every other command** (`replace`, `reindex`, `delete`, `match --id`) accepts `ID@rN` only when N is the id's current revision, and exits 2 otherwise ("BRKT is at r3, not r1"), changing nothing. A script can so make sure it changes the revision it read.
  - [CT16: `catalog_cli.rs::strict_revision_references`, `catalog_cli.rs::replace_keeps_the_old_revisions_bytes`]

### 4.5 Change and quarantine (CR)

- **CR1. Delete is one transaction** (A DL2; M5-6):
  1. set `entry.deleted_generation`;
  2. set the current revision's `superseded_generation`;
  3. bring each content its revisions name, current or kept (CR3), to IN3's state: a `blob` row that no live entry still holds is deleted, which cascades to `features`, `tokens`, `posting` and `properties`;
  4. write the `op` row.

  After the commit, an inline gc (ST5) unlinks the file. If the unlink fails, the delete has still happened: the command exits 0, and a warning names the orphan and `fitment catalog gc`. [CT13]
- **CR2. All or nothing across ids** (C 5.6). `delete A B C` with any id unknown deletes nothing (exit 2, the unknown ids listed). Otherwise all are deleted in one transaction. [CT16]
- **CR3. Replace is one transaction** (A DL3).
  - The new revision becomes current and the old one is superseded. The new content's rows are inserted, or found, in the same transaction.
  - **The old content's derived rows** (`features`, `tokens`, `posting`, `properties`) are deleted explicitly; no other live revision can name it (ID5). **Its `blob` row and file are kept** (review, F4(b); M5-41): the revision is *kept* until `prune --revisions` (CR11) or the entry's delete (CR1), so `get ID@rN --format step` can write the bytes a past report judged. A kept content has no derived rows, so level 1 never sees it (IN9).
  - A reader sees the old revision or the new one, never both and never neither (the partial unique indexes). [CT9; CT16: `catalog_cli.rs::replace_keeps_the_old_revisions_bytes`]
- **CR4. What replace refuses** (M5-7; A DL3, C 5.5).
  - **A file the gate refuses:** refused, exit 3, the refusal printed, and nothing changed.
  - **A file over the cap:** the same.
  - **Bytes live in another entry:** refused, exit 2, naming it.
  - **The entry's own bytes:** `unchanged`, exit 0. [CT16]
- **CR5. A quarantined entry replaced by an accepted file becomes `indexed`** (C 5.5).
- **CR6. Tombstones keep metadata only** (A DL6, C CD-4). The id, every revision with its SHA-256 and file name, and their generations. No derived rows, no blob.
- **CR7. Commands and concurrent changes** (A DL4). A command's snapshot (DB4) fixes what it sees.
  - A match that began before a delete judges the deleted part and reports generation G; one that begins after does not see it.
  - A match that took deletions mid-run would judge a set that never existed. [CT9]
- **CR8. Quarantine is never searched, always counted** (C CQ2; M5-8). A full run neither reads nor judges a quarantined entry. Its stderr line and table header count the entries not searched, and the stderr line names `fitment catalog list -C DIR --status quarantined`. [CT1]
- **CR9. A quarantined entry named is a refused candidate** (C CQ3). `--id Q` with Q quarantined judges the stored refused document: `Reject`, reason `candidate_refused`, with its refusal codes (VD10), the same entry as `fitment match S Q.stp`. `--exhaustive` does the same for every quarantined entry. [CT1, CT2]
- **CR10. Retry and repair** (C CQ5; M5-8). A quarantined entry is gated again by `reindex` when its extractor version is not the build's, or when named. Fixing the file is `replace ID fixed.stp`.
- **CR11. Prune** (review, F4(b); M5-41). `fitment catalog prune -C DIR --revisions` runs in one `BEGIN IMMEDIATE` transaction, as gc does:
  1. set `revision.pruned_generation` on every superseded revision of a live entry that has none;
  2. bring each content those revisions name to IN3's state, deleting the `blob` rows that no current or kept revision of a live entry still holds;
  3. write one `op` row (`prune`) per revision pruned.

  After the commit the inline gc unlinks the released files (ST5). Without `--revisions` it is a usage error (exit 2), so the command never drops history by default, and later kinds of pruning get their own flags. Pruning twice prunes nothing the second time. [CT13, CT14, CT16: `catalog_cli.rs::replace_keeps_the_old_revisions_bytes`; `fault.rs::prune_is_all_or_nothing`]

### 4.6 Re-indexing (RX)

- **RX1. Current means this build's versions** (A RX1). A `features` row is current when its `extractor_version` is `EXTRACTOR_VERSION`; a `tokens` row, and its postings, when its pair is (`EXTRACTOR_VERSION`, `SIGNATURE_VERSION`); a `properties` row when its version is `PROPERTIES_VERSION`. A loaded document must also say so itself (`FeaturesDocument::is_current()`), and the column must agree with the JSON, or the load fails (QR4).
- **RX2. Stale** (M5-3; A RX2, B RX2, C RX1). A live entry is stale for matching when it has no current `features` row (either status: a new gate might accept a quarantined file), or when its current `features` row is `accepted` and it has no current `tokens` row. Older and newer versions both count, so a downgrade is stale too (C A2). Rows of other versions kept beside current ones are never read and make nothing stale (merge finding 1). An entry without a current `properties` row lists as `stale (properties)` and is never stale for matching (M5-26). [CT8]
- **RX3. No match on a stale catalog** (M5-3; A RX3, B RX2, C RX2). A full or exhaustive run refuses when any live entry is stale, an `--id` run when any named entry is: exit 3, nothing on stdout, and `catalog parts: refused: 1196 parts were read by another fitment (extractor 6, this build 7): run fitment catalog reindex -C parts`. There is no override, and stale rows are never skipped, which would make the result depend on how far a re-index had got. [CT8]
- **RX4. Reindex** (M5-3; A RX4, B RX4).
  - **Signature-stale:** tokens and postings are rebuilt from the stored current document (`from_json`, then the signature), with no blob read, about 0.1 ms a part (A).
  - **Extractor-stale, quarantined included:** the blob is read and its hash checked (ST11), gated and extracted again, and its new-version `features`, `tokens`, `posting` and `properties` rows are inserted in batches (DB3), each with its preimage hash (GT2). Its status may flip either way.
  - **Kept revisions** (CR3) have no derived rows and are never reindexed; `get` reads only their bytes.
  - **Properties-stale:** the blob is read and the properties captured again. [CT8]
- **RX5. A damaged blob is never indexed** (C RX6, A RX4). A missing blob, or one whose hash is not its name, is reported and `reindex` exits 1; the entry keeps its old rows and stays stale, so catalog runs keep refusing until the user replaces or deletes it. [CT8]
- **RX6. A part that leaves the search is loud** (C RX5). An entry going from `indexed` to `quarantined` is reported with its first reason ("BRKT-0042: no longer searched: …"), and `reindex` exits 3. [CT8]
- **RX7. Resumable and idempotent** (A RX6, B RX4, C RX3). Each batch is one transaction. A crash keeps the committed batches, the match stays refused, and a rerun does exactly what is missing. [CT8, CT13]
- **RX8. Never downgrade** (A RX5; M5-29). Rows of a version newer than the build's are never written, rewritten or deleted. `gc --versions` removes older versions' rows only for content whose current rows exist, so a half-finished reindex never loses rows another binary may still read. [CT8]
- **RX9. What reindex processes** (C RX4). With no argument, every stale entry; with `ID...`, those entries, current or not; with `--all`, every live entry.
- **RX10. Nothing stored depends on `MATCH_RULES_VERSION` or `SOCKET_RULES_VERSION`** (A RX7, RX8). No verdict is cached (GT9) and no socket stored (M5-17); a socket file passed to a catalog match goes through `Matcher::new`, which refuses a stale one (VD9).
- **RX11. Versions rise when they must** (B RX1). Any change to `bin`, `BIN_MM`, `IndexTokens::of`, the pair-token rule or `PartSignature::of` raises `SIGNATURE_VERSION`; any change to property capture raises `PROPERTIES_VERSION`. Goldens of tokens and properties for the committed candidates fail while the version stands still. [CT12]

### 4.7 Level 1, the index (IX)

- **IX1. One algorithm, in core** (B IX1; M5-11). M4's level-1 query becomes `level_one<S: IndexSource>(socket, source)`. `ShortlistIndex` is one `IndexSource`, used in memory by M4's tests and the harness; the catalog's `SqlSource` is another. `fitment-catalog` contains no window, no bin arithmetic and no filter. Its SQL is three plain reads (section 6.2). [CT3: the two sources agree, id for id]
- **IX2. Bins come from core only** (B IX2). `IndexTokens::postings()` yields what ingest writes, and `level_one` computes every probe's bin range. `bin()` and `BIN_MM` stay private to core; a change to either is a `SIGNATURE_VERSION` rise (RX11).
- **IX3. Exact comparison after the probe** (B IX3). The probe reads bins `bin(lo) − 1` to `bin(hi) + 1`; an unbounded end reads to `i64::MIN` or `i64::MAX`. Each diameter that comes back is tested by `Window::holds` on the same `f64`. SQL never compares a diameter. A `REAL` column returns an IEEE double bit for bit, and every stored diameter is finite and positive, so neither of SQLite's lossy cases arises (NaN as NULL, −0.0 as +0.0). [CT4]
- **IX4. Narrowest window first** (B IX4). Each distinct (sense, narrowest window of a line) is probed in ascending order of width, and the set is intersected after each probe; an empty set ends level 1. The result is M4's; only the work changes. [CT3]
- **IX5. Each part at its own lever** (B IX5; M5-11).
  - **The lever:** the pair filter and the seat-group count use max(socket lever, the part's own `lever_mm`), never the catalog's largest. Level 2 uses exactly `max(socket.lever_mm, part.lever_mm())`, and `IndexTokens.lever_mm` is that same value, so level 1's windows contain level 2's for every part.
  - **Buckets:** seat groups only coarsen as the lever grows, so rounding the lever up to a power of two never asks for more planes than level 2 does. This bounds the group computations to about 40 per query.
  - **A bucket never rounds down** (review, IX5 note; B's prototype). The bucket of a lever x is `max(2^ceil(log2(x)), x)`. The guard is needed, not decorative: for x one ulp above 2^k, `log2(x)` rounds to exactly k in `f64`, so `2^ceil(log2(x))` is 2^k, one ulp *below* x (checked for k ≥ 5: 32, 256, 1024). A bucket below the lever gives a narrower W_L, finer seat groups, and so more planes asked than level 2 asks: a hidden part. A lever of 0 buckets to 0.
  - **Measured:** s01's bracket took 286 ms of level 1 without buckets and 33 ms with them. [CT3: `index.rs::a_seat_bucket_is_never_below_the_lever`]
- **IX6. Independence** (B IX6). Whether a part is admitted depends only on the socket, the policy and the part's own tokens. Adding, replacing or deleting other parts cannot change it. M4's index widened every query's windows to the largest lever indexed, so one 4.7 m part widened every query. [CT3: 1,000 scaled parts added, one with a 5 m lever, change no corpus part's admission]
- **IX7. What level 1 admits, unchanged from M4** (B IX7).
  - **Lines:** for each line, some stack of its sense with a diameter in the line's narrowest window.
  - **Planes:** at least as many planes as the socket has seat groups, at IX5's lever.
  - **Pairs:** for each line parallel to the most selective line and **outside its SG5 group**, a pair token in range. A part whose pairs are incomplete, or a policy whose angular window exceeds the tokens' 0.1°, bypasses the pair filter.
  - **Lines of one group are skipped** (review, F3: inherited and done). SG5 lets one stack serve every line of a group, the two coaxial holes of a clevis, and level 2 and verification allow it; a token joins two distinct stacks only, so demanding one dropped a part that a file run passes. M4's exit PR fixed `ShortlistIndex::pairs_hold` to skip lines of the selective line's SG5 group, at a lever no smaller than any indexed part's (regression test `signature.rs::one_stack_for_a_group::level_one_keeps_a_part_one_stack_serves_a_group_with`). PR 1's `level_one` keeps the skip over any `IndexSource`, with the groups formed at the part's own bucketed lever (IX5). Groups only coarsen as the lever grows, so the groups at the bucket contain every group level 2 forms at the part's own lever, and skipping more lines only admits more.
  - **A socket that demands no line** sends every indexed part to the plane and pair filters. [CT3: `index.rs::sql_source_keeps_a_part_one_stack_serves_a_group_with`; CT1: `catalog_corpus.rs::a_clevis_pin_one_shaft_for_two_holes_is_found`]
- **IX8. Blocked** (B IX8; M5-13). The whole query has no candidate when any socket of the document is blocked: an active requirement fails every candidate (SG7), or the socket demands nothing (SL8). The candidate must pass every socket (VD2). A full run then exits 3 (CM9). [CT1]
- **IX9. Tokens are never trusted for a pass** (B IX9, A S7).
  - `IndexTokens` and `PairToken` gain `Deserialize`, and `from_json` validates them: finite, non-negative, sorted as `of` writes them.
  - Each stored token's `tokens_sha256`, over its preimage (GT2), is checked on load; a mismatch is a corrupt row (QR4).
  - A wrong token can only make level 1 admit too much, which level 2 and verification correct, or too little, which hides a part. It can never make a part pass, because everything after level 1 is recomputed from the document.
  - **Postings carry no hash** (review, F7). A run reads only the postings its probes return, so a posting altered or lost on disk is not seen by the run it hides a part from: that run exits 5 where 0 or 4 was due. A posting can never pass a part, for the reason above. Plain `verify` recomputes every posting from its hash-checked tokens and reports any difference (VF1); between `verify` runs the risk stands (R20). [CT12: `verify.rs::plain_verify_recomputes_postings`; CT16: `match_cli.rs::a_lost_posting_hides_a_part_until_verify`, `match_cli.rs::a_corrupt_posting_never_passes_a_part`]
- **IX10. Level 1's ids** (merge, B A19). The id an `IndexSource` yields is `blob.key`, an `AUTOINCREMENT` content key, so a key read in one snapshot never names other content in another. SQLite's `i64` is converted with `u64::try_from`; a failure is a catalog error, never a wrap. [CT3]

### 4.8 The query (QR)

- **QR1. The order of refusals** (C CM7, B QR1; M5-13). Before any candidate is judged, with nothing on stdout, the first that applies decides:
  1. the socket gate (MF2, unchanged);
  2. DIR is not a catalog (exit 2);
  3. a newer schema (exit 3);
  4. staleness (RX3, exit 3);
  5. in a full run, no indexed entry (exit 3: "the catalog holds no searchable part: 3 parts, all quarantined");
  6. in a full run, a blocked socket (exit 3: "socket-1: bore:hole-88 fails every candidate (unverifiable: unresolved): remove it in review, or no catalog part can be found").

  The stale check is two counting joins over the live entries (section 6.2); B measured 17 µs for an index lookup and 21 ms for a scan at 10⁵ on its one-row-per-part schema, and PR 4 measures the merged schema's. [CT8, CT16]
- **QR2. One snapshot, one reader** (B QR2, A DB4). Every read of a run is made on one connection inside one read transaction: level 1, the tokens and every document. Workers receive parsed documents and never touch the database. With threads on their own connections, each would see its own snapshot, and a part replaced in between would mix two states. [CT9]
- **QR3. Level 1 under the run's own signatures** (B QR3, A12). Level 1 is `Matcher::admitted(&SqlSource)`: core's `level_one` for each socket's signature as `Matcher::new` built it under the run's policy, intersected over the sockets, or `Blocked`. The catalog builds no signature of its own. [CT1 under `--tolerance-mm 0.05` as well as the default]
- **QR4. Load and check** (A IX3, B QR4; M5-32, M5-40). In the snapshot, for each survivor in ascending key:
  1. read the row's `sha256`, `extractor_version`, `status`, `part_digest`, `doc` and `doc_sha256`;
  2. check that the SHA-256 of GT2's preimage, built from those columns and `doc`, equals `doc_sha256` (review, F2: a row moved or copied to another key fails here);
  3. parse with `FeaturesDocument::from_json`, which checks the format, the version and `read_part`'s structural guarantees;
  4. check `is_current()`, and that the document's status equals the column;
  5. for an accepted document, recompute `doc.part_digest()` and check that it equals the `part_digest` column (review, F1). The recomputed value is the dedup key (QR6); the column serves only diagnostics and `verify`.

  The checks yield a `CheckedRow` (the row's checked content SHA-256, the parsed document, the recomputed digest), the only input QR6 and QR7 accept. In a full run every survivor is `accepted` by construction (GT4). A refused document is judged only when named or under `--exhaustive` (CR9). A failure at any step means a corrupt row: that candidate is not judged, stderr names its id and `fitment catalog verify -C DIR`, the rest are judged, and the run exits 1. Nothing is judged from a partly read document. [CT12; CT16: `match_cli.rs::a_part_digest_column_that_disagrees_is_corruption`, `match_cli.rs::a_part_digest_forged_with_its_hash_is_still_caught`, `match_cli.rs::a_features_row_moved_to_another_key_is_not_judged`]
- **QR5. File-run semantics on every survivor** (M5-4; A M5A-15). Each survivor is judged by `judge_document` exactly as `fitment match` judges a file, with M4-17's explicit semantics. A level-2 drop is aligned and verified for its evidence, and listed as `Reject` at stage `shortlist` with its `missing`. A verification pass after a drop exits 1 as a fitment bug (KH9 at runtime). [CT1]
- **QR6. Dedup** (B QR6; M5-12). Survivors are grouped by the part digest QR4 recomputed from each loaded, hash-checked document, never by the stored `part_digest` column (review, F1: a column naming good part Y in bad part X's row would otherwise copy Y's pass onto X, reported under X's SHA-256). A column that disagrees with its document is corruption, caught by QR4 before grouping. One representative per group (its lowest key) is judged, and each member gets a copy of the result with its own `source`. This is exact because `CandidateResult` is a function of the `Part` and the matcher alone, the source only copied in. It applies only while the fit verifier names no kernel: with one (M7), the interface checks may be shared but the fit is asked per member, since two files with equal features can carry different bodies. [CT7: dedup on and off give the same bytes; a fake verifier naming a kernel is asked per member; CT16: `match_cli.rs::dedup_groups_by_the_recomputed_digest`]
- **QR7. One judging path** (A IX4, B QR7; M5-10).
  - **A stored document** is judged by `Matcher::judge_document(&doc, source)`, with `source` = `Source::new(row.sha256, blob.bytes, doc.header())`, where `row.sha256` is the `CheckedRow`'s content SHA-256, the one QR4's preimage hash just bound to this document (review, F2), and `blob.bytes` comes from that SHA-256's `blob` row (IN1 checks it).
  - **A file** is judged by `judge_step(bytes)`. Both are thin wrappers over one private path, `judge_document_with(doc, source, step: Option<&[u8]>)`: `judge_step` passes `Some(bytes)`, which reaches `FitRequest.candidate_step` as today; `judge_document` passes `None` (review, F5). So a catalog candidate and the same file on the command line go through one code path, and M7's file runs keep their fit check. With `NoFitVerifier`, M5's only verifier, the bytes change nothing; in M7 the catalog hands over a blob's checked bytes through the same private path (QR13).
  - Because `judge_document` refuses a non-current document, a result's `read.extractor_version` cannot claim a reader that did not read it. [CT5: `stored.rs::judge_step_hands_its_bytes_to_the_fit_request`; `query.rs::the_source_is_the_checked_rows_own_sha256`]
- **QR8. Parallel and deterministic** (B QR8; M5-30). Representatives are judged on `--jobs` threads and the results handed to `MatchDocument::new`, whose MF5 order is total (CM7). The bytes depend neither on the number of jobs, nor on the order threads finish, nor on the order of SQL rows; B measured byte-identical documents across thread counts and with level 1 from SQL or from memory (`B/out/determinism.txt`). **File runs too** (review, the merge's open question 3): `--jobs N` reads and judges at most N candidate files at once, each file read once into one buffer as MF3 reads it, and the results go to the same `MatchDocument::new`, so the bytes do not depend on N. A file's read error is reported in the files' argument order, not the threads'. [CT7: `match_cli.rs::jobs_do_not_change_a_file_runs_bytes`]
- **QR9. Budgets** (B QR9). Each candidate keeps M4's bounds (`MAX_SHORTLIST_CHECKS`, `MAX_PLACEMENTS`, `MAX_COVERAGE_STEPS`), about 5 s per candidate and socket at worst. The catalog adds no time limit (M5-36). A hostile part costs its bound whenever it is admitted; `h_hole_grid` took 3.2 s against s01's bracket. [CT10]
- **QR10. Exhaustive mode** (B QR10, C CM6; M5-4, M5-42). `--exhaustive`, documented (review: it was the hidden `--audit`), bypasses level 1 and judges every live entry as an explicit candidate, quarantined ones as refused (CR9); blocked sockets do not stop it (M4-16). Its document equals a file run over the catalog's live files, digest included. Tests use it to show level 1 hides nothing (the corpus, F3's clevis case, F7's lost posting); a user uses it when a miss is suspected, and `--help` says it costs a judgment per live entry. Under M5-43's contingency, if taken, it also restores the level-2 drops a full run counts. [CT2: `catalog_corpus.rs::exhaustive_equals_the_file_run`]
- **QR11. Only the index** (A IX1, IX2, B QR11, C CM1). `match_catalog` takes a `Snapshot`, which has no field reaching `Blobs`; `Blobs` is reachable only from `Catalog`, through ingest, replace, reindex, verify and `get --format step`. A match opens no file under `blobs/` and parses no STEP. [CT6: a compile-fail doc test, a counting file-system seam, and the blob directory moved away]
- **QR12. The original** (B QR12). `--original ASM` works as in M4. It reads the assembly the user names, which is not a catalog read.
- **QR13. The fit stage is the one later exception** (A IX7). `FitRequest.candidate_step` needs bytes. In M7 the catalog will read a blob only for a candidate whose interface passed, and check its hash before handing it over (ST11), through `judge_document_with`'s `Some` (QR7). M5 runs with `NoFitVerifier` and reads none.

### 4.9 Concurrency (CC)

- **CC1. Many readers, one writer** (A CC1). Readers never block writers or each other (WAL); writers queue on the write lock (DB2). [CT14]
- **CC2. The same bytes ingested at once** (A CC2). Both processes write the same blob, an identical rename. The first to commit wins, and the second finds the live content inside its transaction and reports the winner's entry (ID5). Different bytes never conflict. [CT14]
- **CC3. A long match holds one snapshot** (A CC3). The WAL grows with concurrent writes until the match ends: A measured 13.0 MB for 300 inserts of 30 KB. After its snapshot ends, a catalog match runs `wal_checkpoint(PASSIVE)`, and `gc` runs `TRUNCATE`.
- **CC4. `verify` raises no false alarm** (A CC4). When a blob is missing or wrong, `verify` re-checks, in a fresh snapshot, that the content is still referenced before reporting it. [CT14]
- **CC5. One host** (A CC5; M5-15). WAL needs shared memory. A catalog on a network file system, or in a folder a sync tool copies (iCloud Drive, Dropbox), is unsupported, and the README says so. Opening checks that `journal_mode` came back `wal`, and refuses otherwise.
- **CC6. A busy catalog** (C ST4; M5-24). A writer that cannot take the lock within `busy_timeout` (60 s) exits 1: "the catalog is busy". No command fails with `SQLITE_BUSY` under the measured load (8 processes). [CT14]

### 4.10 Paths and hostile names (PH)

- **PH1. Paths come from validated hex only** (A PH1, C ST1, HN1). A path under the root is built from a `Sha256Hex` (exactly 64 characters `[0-9a-f]`) or a temporary name the writer made. No id, file name, STEP string, argument or database value becomes a path component. [CT15: a `revision.sha256` of `../../x` violates the CHECK; through a raw connection with checks off, `verify` and `gc` refuse it without touching the file system]
- **PH2. The root** (A PH2). Every command refuses a directory whose marker or `application_id` is wrong. It also refuses a root in which `blobs`, `blobs/sha256`, a fan-out directory or `tmp` is a symbolic link, checked with `symlink_metadata` at open and before gc: a planted link could otherwise point gc at someone else's files. [CT15]
- **PH3. Source names are for display** (A PH3, C HN5). The source's final path component is stored as text, lossily if not UTF-8, up to 4,096 bytes. It is never an identity, an order key or a path.
- **PH4. Directory arguments** (A PH4, C 5.2; M5-33).
  - A directory argument contributes its entries whose names end in `.stp`, `.step` or `.p21`, in any case, not recursively. Files named explicitly are taken whatever their extension.
  - Only regular files are read, checked by `metadata` before opening and again on the opened handle. A FIFO, socket or device is reported and skipped.
  - A file that disappears between the listing and the open is reported, and the run goes on. [CT15]
- **PH5. Windows** (A PH5). A root whose path is not Unicode is refused, since rusqlite needs UTF-8 there.
- **PH6. Text reaches a terminal only through `printable()`** (C CC2, HN3). File names, PRODUCT.id and name, refusal messages, property names and values, and DIR itself are escaped wherever printed. Control characters, bidirectional overrides and newlines are escaped. Ids need no escaping by ID1. JSON holds the exact strings. [CT15]
- **PH7. STEP bytes never reach a terminal** (C HN4). `get --format step` writes only to the file `-o` names; without `-o` it exits 2.
- **PH8. SQL never holds user text** (C ST6, HN6). A `sha256:` reference is validated as hex before any query, and ids are bound, never interpolated. [CT15: ids and references with `'`, `%`, `_`, `;` find exactly their entry or nothing]

### 4.11 Sizes (SZ)

- **SZ1. 128 MiB per file** (A SZ1; M5-15). `--max-mib` changes it. It is checked with `metadata` before reading, and again by reading with `take(cap + 1)`, so a file that grows during the read is still refused. stepq's graph is about 10 times the file, so the peak is about 1.3 GB; the largest fixture is 7.4 MB. [CT15: a sparse 129 MiB file is refused, nothing stored]
- **SZ2. Derived documents** (A SZ2).
  - **The measured ratio:** a features document is at most 1.39 times its STEP file (`nist_ftc_08_asme1_rc`: 450,741 → 628,571 bytes, pretty).
  - **The guard:** a document over 512 MiB is refused as `derived_too_large` and not stored. Under SZ1 that needs a ratio above 4, so it is a guard, not a path.
- **SZ3. Tokens** (A SZ3). Core bounds them (`MAX_PAIR_TOKENS` 20,000, `pairs_incomplete`). A measured version-1 tokens at a median of 126 B, a 90th percentile of 1.9 KB, and at most 348 KB (`nist_ctc_02_asme1_ap203`, 84 stacks).
- **SZ4. Growth** (A SZ4). The database never shrinks by itself. `gc --vacuum` runs `VACUUM`, which needs free space about the size of the database; it checks the free space first and says so.
- **SZ5. A quota** (A SZ5). `Options::max_db_pages` sets `PRAGMA max_page_count`. It is a real option, and it is how the disk-full tests run on every OS. [CT13]

### 4.12 Verification (VF)

- **VF1. `fitment catalog verify`** (A VF1, C VF1) runs in one snapshot and checks:
  - `PRAGMA quick_check` (`integrity_check` with `--full`) and `foreign_key_check`;
  - invariants IN1 to IN9 (section 4.16), each blob re-hashed, kept revisions' included;
  - **the preimage hashes** (review, F1, F2; M5-40): every `features`, `tokens` and `properties` row's hash over its GT2 preimage, so a row moved or copied to another key, or a key, status, version or digest column edited, is reported without `--deep`;
  - each current document's `from_json`, and its column and JSON agreement;
  - **the part-digest columns** (review, F1): each accepted document's `part_digest()` recomputed and compared with its column, so a digest written wrong at ingest, with its hash consistent, is found too;
  - **the postings** (review, F7): for each current `tokens` row, its postings recomputed with `IndexTokens::postings()` from the hash-checked tokens and compared, as a set and bit for bit in each diameter, with the stored rows: an altered, lost or extra `posting` row is a `mismatch`;
  - no file under `tmp/` older than one hour.

  Its cost is a parse of every current document and tokens row, about 0.03 ms a corpus part (8.1), so seconds at 10⁵; PR 3 measures it. `--deep` adds the re-extraction. [CT12: `verify.rs::plain_verify_checks_preimages_digests_and_postings`, `verify.rs::plain_verify_recomputes_postings`]
- **VF2. `verify --deep`** (A VF2, B RX6, C VF2). It re-extracts every live content from its blob and requires the stored document, tokens, postings, part digest and properties to be equal byte for byte. This is the check that the index is derived from the store, whatever happened to either. It costs a reindex's time. After the review, what only `--deep` finds is a row consistent in itself but wrong for its bytes: a document, tokens or properties computed wrongly and hashed consistently, by a bug at ingest or a forger. [CT12: a stored document edited to add a hole, with its preimage hash and part digest recomputed, passes plain `verify` and fails `--deep`]
- **VF3. Output and exit** (C VF4; M5-25).
  - **Output:** one problem per line on stdout, as `missing`, `damaged`, `orphan`, `temporary`, `mismatch` or `integrity`. A row whose preimage hash fails is `damaged`; a digest column or a posting that differs from what its checked row gives is `mismatch`.
  - **Exit:** 0 when consistent, 3 when anything is wrong, 1 when `verify` itself cannot run.
- **VF4. `verify` never repairs** (merge; M5-25). `gc` is the one repair, and only of what cannot lose a part: orphan blobs and old temporary files, and, with `--versions`, older-version rows (RX8). A row whose blob is missing or damaged is reported for the user to `replace` or `delete`.
- **VF5. A catalog from elsewhere is trusted only after `verify --deep`** (C VF5, B section 9). A copied or tampered `catalog.db` with consistent hashes can state a hole its file lacks, and a match believes the index. The README says so where it explains copying a catalog.

### 4.13 Properties, D10 (PR)

What the fixtures state (C, `C/proto/props_census.txt`, `raw_scan.txt`, `props_map.txt`):

- **Census:** of the 45 fixtures, 29 state some property. Part-level user properties appear in 9 files, 5 of them accepted by the gate. `MATERIAL_DESIGNATION` appears in none.
- **Material:**
  - No material name or grade is ever a property value.
  - Two drawing notes state one in prose: `nist_stc_09_asme1_ap242-e3.stp` `#8518`, "MATERIAL: … 304 SST SHT, ASTM A240", and the same note in `nist_ftc_09`.
  - `Density` = 1000.0 appears in three different parts (`nist_ctc_04` `#18079`, `nist_ftc_07`, `nist_ftc_10`): a CAD default, not a material.
- **Thread:**
  - No thread is ever a property.
  - Threads appear only as presentation strings (`'DIM\w4X M12x1.75-6H'` in `nist_ctc_04` `#18961`, `'DIM\w2 XM3 x 0.5-6g'` in `nist_stc_10` `#1238`), or as semantic diameters on cosmetic curves (`'Thread Cylinder.1'`).
  - The modelled hole under the M12 callout is a plain Ø10.2 cylinder, the tap drill: PLAN's stated limit, seen on real data.
- **The one sound reading:** Siemens NX's `MaterialMissingAssignments` = `TRUE`, in `nist_ctc_05_asme1_ap242-e1.stp` (`#1095`, value `#897`) and `nist_stc_09_asme1_ap242-e3.stp` (`#7270`, value `#7268`). It says the model has no material. It is a negative statement, and both files are accepted.
- **Volume:** one accepted file (`nist_stc_06`) states 440 part-level properties (`p1`, `p3`, … CAD parameters).

The rules:

- **PR1. From STEP only** (C PR1). Properties are read through `fitment-core` (`properties::read`, with stepq behind core's wall) from the bytes being ingested (ST2). Nothing else is a source: not the file name, the id or PRODUCT.name.
- **PR2. What is captured, bounded** (C PR2; M5-5).
  - **What:** user-defined properties whose subject is the part's own PRODUCT_DEFINITION or its PRODUCT_DEFINITION_SHAPE, never a shape aspect's and never an enclosing assembly's; plus any `MATERIAL_DESIGNATION` whose definitions include the part's.
  - **How each value is kept:** as written, with its name, value text, measure type, unit and the property's `#id`.
  - **The bound:** at most 1,024 values of at most 4 KiB each, since 440 were seen in one file. Past either bound the set is marked `truncated`. [CT18]
- **PR3. Captured at ingest, versioned** (C PR3; M5-26). The captured set is stored per content and `PROPERTIES_VERSION` (core, starting at 1), which rises only when the capture changes, with `stated_sha256` over GT2's preimage (content SHA-256, version, truncation, then the bytes; review, M5-40). The mapping is applied when `get` displays the set, so a new mapping row needs no reindex.
- **PR4. The mapping table** (C PR4; M5-5).
  - **A row:** `(name, value or any) → (field, meaning)`. The name is compared after trimming and ASCII case folding, the value as written. The fields are `material`, `thread` and `finish`; the meanings `designation` and `unassigned`.
  - **Evidence:** every row cites a sample file and record that exercise it, and a test pins that record. A row with no real sample is not added.
  - **M5 ships one row:** `materialmissingassignments`, value `TRUE`, maps to material `unassigned`, citing `nist_ctc_05_asme1_ap242-e1.stp` `#1095` and `nist_stc_09_asme1_ap242-e3.stp` `#7270`.
  - **Considered and not mapped,** with the reason kept beside the table:
    - `Density`: a default value;
    - `MaterialMultipleAssigned`: says nothing about which material;
    - `Part Number`, `Description`, `CAD_SOURCE` and `Part File Format`: identification;
    - AP242's `MATERIAL_DESIGNATION`: in no sample yet. [CT18]
- **PR5. No verdict or flag changes in M5** (M5-5; C PR7, PR8). Properties never change a verdict kind, pass or fail a requirement, or set or clear a flag.
  - `material_not_compared` stays on every verdict.
  - `thread_not_confirmed` is set exactly as VR19 sets it.
  - A thread may be confirmed in a later milestone only when the original's hole and the candidate's hole each carry semantic PMI that stepq links to that hole's own faces, stating the same designation; a part-level property, a callout's text, a cosmetic curve set or a diameter never confirms one. The socket would then need to carry the original's designation, a `fitment.socket` change. [CT18]
- **PR6. No reading of prose or names** (C PR9). Drawing notes, presentation strings, PRODUCT.id and name, file names and catalog ids are never parsed for a material, a thread or a size, even when they say `M6x20` or `304 SST`. [CT18]
- **PR7. Shown by `get` only** (M5-5). `fitment catalog get` prints each stored property (`#7270 MaterialMissingAssignments = TRUE`), escaped, with its mapped meaning where a row applies (`material: unassigned`). The match document and its table carry none.

### 4.14 The CLI's conventions (CL)

These extend the existing commands' conventions (`main.rs`, `cmd/*.rs`).

- **CL1. One way to name a catalog** (C CD-5; M5-9). `-C/--catalog DIR` is accepted anywhere after `catalog` (clap `global = true`) and on `fitment match`. There is no default and no environment variable. A directory that is not a catalog is exit 2 ("not a catalog: run fitment catalog init -C DIR"). [CT16]
- **CL2. Exit codes** (C CC1; M5-13).
  - **The codes:** 0 done; 1 error (unreadable, unwritable, busy, a fitment bug); 2 usage (bad arguments, an unknown or ambiguous reference, an id conflict, a duplicate under `--id`, not a catalog); 3 refused input (a refused or oversized file, a stale catalog, a newer schema, `verify`'s findings, a search that could not run).
  - **Several files:** the run's code is the highest-precedence code among them, in the order 1, 2, 3, 0.
  - **`fitment match`:** keeps VD14's 0, 4 and 5 after QR1's refusals. [CT16]
- **CL3. Formats** (C CC3). Every command that prints has `--format table|json`.
  - **`table`** is for people and is not a stable format.
  - **`json`** is a `fitment.catalog` v1 document (section 7.11), pretty-printed with a final newline, ordered by id case-folded, then by its bytes.
- **CL4. stderr** (C CC4). One summary line per command, `fitment: <DIR>: …`, then warnings, one per line. stdout carries only the table or the document. [CT16]
- **CL5. A closed stdout** (C CC5). `| head` ends output quietly and keeps the command's code.
- **CL6. No prompts** (C CC6). No command asks a question. Destructive commands take exact ids (ID7) and say what they did.
- **CL7. Atomic outputs** (C CC7; M5-16). Every file the CLI writes outside DIR (`socket -o`, `review`, `match -o`, `get --format step -o`) goes through `write_atomically`:
  1. write a temporary file beside the target;
  2. `sync_all` it;
  3. rename it over the target;
  4. sync the directory (Unix).

  An output equal to an input, or inside DIR, is refused (exit 2) and left untouched. PR 1 adds steps 2 and 4 to today's function, which renames without a sync: a crash could leave an empty file where a reviewed socket was. [CT16]
- **CL8. Without the feature** (C CC8). A build without the `catalog` feature has no `catalog` command. `match --catalog` there exits 2 with "this build has no catalog (cargo feature `catalog`)". [CT20]
- **CL9. Deterministic documents** (C CC9, A OR2, OR3). A `fitment.catalog` document holds no time, host or absolute path. `file_name` is the name the file had when ingested, and `changes[].input` the argument as given. The same catalog state gives the same `list` bytes on every OS, and the same ingest sequence gives the same ids. [CT7]
- **CL10. Counts are plain** (C CC10). Numbers in tables and stderr have no thousands separators and no locale.

### 4.15 The catalog match (CM)

- **CM1. Exclusive inputs** (C CM2, B B9; M5-39, M5-44). Candidate files and `--catalog` exclude each other, and one of them is required (exit 2). `--catalog` is given at most once: one catalog per run (review, the merge's open question 5). `--id` needs `--catalog`, and `--exhaustive` needs `--catalog` without `--id` (exit 2). `--jobs` applies to both kinds of run (M5-30). [CT16: `match_cli.rs::one_catalog_per_run`]
- **CM2. A full run** (M5-4). `fitment match S --catalog DIR` judges the level-1 survivors (QR5) and lists exactly them. The document is the file run's over the catalog's files, restricted to those candidates. [CT1]
- **CM3. A named run** (C CM4; M5-22). `--catalog DIR --id A --id B` judges the named entries as explicit candidates. Level 1 is not used, a blocked socket does not stop them (M4-16), and a quarantined one is CR9's. Its digest equals that of `fitment match S <A's file> <B's file>`. [CT1]
- **CM4. Each entry is a file run's entry** (B CD2, C CM5; M5-4). For an entry with bytes B, the catalog run's entry, labels aside, is byte-identical to the entry `fitment match S B.stp` writes, under the same policy and options. [CT1]
- **CM5. `fitment.match` v1, unchanged** (B CD1; M5-2). No field is added, removed or reinterpreted. `summary.candidates` counts the judged candidates. `CandidateSource::Catalog { id }`, planned in M4's section 12 and never built, is not added. [CT1, the test-side checker on every catalog document]
- **CM6. Labels** (B CD3; M5-2). Each catalog candidate has exactly one label, `catalog:<id>@r<n>`, with the id as stored and the revision judged (review, F4(b)). What follows `catalog:` is a strict reference (ID9), so `fitment catalog get -C DIR BRKT-0042@r1 --format step -o x.stp` writes the bytes that entry judged, while they are kept (M5-41). Labels stay outside the digest and the order (MF4), so two catalogs with the same contents give the same digest whatever their ids or histories (C A26). [CT7; CT16: `match_cli.rs::labels_carry_the_revision`]
- **CM7. The source** (B CD5, A10). A catalog candidate's `source` is `File { sha256, bytes, header }`, from the row (QR7). Each SHA-256 is live at most once in a catalog (ID5), so MF5's order is total. A `Given` source would tie on an empty key and leak the threads' finishing order into the document. [CT7]
- **CM8. What the document leaves out, stderr says** (B CD4, C CM12; M5-2). One stderr line per run states what was searched and what was not:

  ```text
  fitment: bracket.socket.json: catalog parts (generation 412): 100083 entries: 100066 searched, 17 quarantined and not searched; level 1 kept 95; 95 candidates: 0 match, 11 interface match, 84 reject; fit not checked (no fit verifier in this build)
  ```

  The table's header carries the same counts on one line after MF9's header. A named run says `3 named entries (level 1 not used)`, and an exhaustive run says `exhaustive: level 1 bypassed`. [CT16]
- **CM9. Exit codes** (M5-13). QR1's refusals first (2 or 3). Then 1 over every verdict (a corrupt row, a pass after a drop, an unreadable database). Then VD14: 0, 4 or 5. `--require-fit` turns 4 into 5 as before. A named or exhaustive run with a blocked socket judges its candidates and exits 5 when all reject. [CT16]
- **CM10. The table** (C CM8, CM13; M5-31).
  - **A candidate line** is MF9's, with the label `catalog:<id>@r<n>`, the first 8 hex digits of the SHA-256 and the PRODUCT.id (or `-`), escaped.
  - **Rejects:** after the passes, at most `--rejects N` (default 10 in catalog runs, `all` in file runs), then `… 24 more rejects (in the document)`. [CT16]
- **CM11. Order** (C CM11). MF5's: verdict, stage, failed checks, SHA-256. Never by id, key or generation.

### 4.16 Invariants (IN)

Checked by `verify` and after every crash and fault test (A section 8.1, extended for ids and properties).

- **IN1.** Every `blob` row has a file at its path whose bytes hash to its key and whose length is `bytes`. It holds after a power loss, not only a crash: every blob a commit names had its bytes and its fan-out directory synced before that commit, whether renamed or reused (ST3, ST9; review, F6).
- **IN2.** Every final file under `blobs/` hashes to its own name. Orphans (a file without a row) are allowed.
- **IN3.** A `blob` row exists exactly when some revision of a live entry names its content and is either current, or superseded and not pruned (`pruned_generation` null): a *kept* revision (CR3, CR11; review, F4(b)). Every change brings each content it touched to this state.
- **IN4.** An entry is deleted exactly when it has no current revision. No two live entries share an id under folding (ID2) or a content (ID5).
- **IN5.** Every `features` row's GT2 preimage hashes to `doc_sha256`; its `doc` reads back with `from_json` and states the row's status and extractor version; and an accepted document's recomputed `part_digest()` equals its column (review, F1, F2).
- **IN6.** Orphan files and temporary files are the only residue a crash may leave. No partial final file ever exists.
- **IN7.** `tokens` exist only for accepted documents of the same extractor version. The `posting` rows of each tokens row are exactly those `IndexTokens::postings()` recomputes from its tokens, bit for bit in each diameter (review, F7). `tokens_sha256` holds over its preimage, as `stated_sha256` does over its own.
- **IN8.** `clock.generation` equals the largest `op.generation`. Every `revision` of an id has a distinct revision number, and `name.last_revision` is the largest of them.
- **IN9.** Derived rows (`features`, `tokens`, `posting`, `properties`) exist only for content that a current revision of a live entry names. A kept revision's content has a `blob` row and none of these, so level 1 never reads it (review, F4(b)).

### 4.17 Why a catalog run is a file run, restricted

**Claim** (B section 9, restated for M5-4). Fix the following:

- a socket document S, accepted and current;
- a policy P;
- a catalog C, current (RX2) and read in one snapshot at generation G.

Let F be the document of `fitment match S` over C's live files under P, and K the full catalog run's document. Then:

1. every entry of K is byte-identical, labels aside, to F's entry for the same bytes;
2. every candidate F passes (`Match` or `InterfaceMatch`) is in K.

So a catalog pass is exactly a file-run pass, M4's zero-false-positive gate carries over, and level 1 can only omit rejects.

**Proof.**

1. **Same input.**
   - The stored document is `read(B)` under this build's extractor (ST2, GT2, RX2, QR4), and it parses back to an equal `Part`. This is tested on every corpus, unit and fixture part (CT5); A measured 243 of 243 and B 232 of 232.
   - The stored document belongs to that SHA-256: its preimage hash names the content key (GT2, QR4), and the source is built from that checked key (QR7). So no document of other bytes can be judged under this one's name (review, F2).
   - The source is the same SHA-256, size and header (QR7, CM7), and `judge_document` and `judge_step` share one private path, differing only in the STEP bytes handed to the fit request, which `NoFitVerifier` ignores (review, F5).
   - So each survivor's `CandidateResult` equals the file run's. A measured 90 of 90 and B 105 of 105.
2. **Same semantics.** Each survivor is judged as an explicit candidate (QR5), as the file run judges every file, so a level-2 drop gives the same `Reject` at stage `shortlist`, with the same evidence.
3. **Level 1 contains level 2, part by part.**
   - Each part's stored tokens are `IndexTokens::of(PartSignature::of(part))` at this build's version (GT6, RX2, checked by VF2).
   - Level 1's diameter windows are level 2's (IX7), and its bins cannot miss (IX3).
   - Its pair and plane tests use the part's own lever, which is level 2's (IX5), bucketed upward and never below it, and incomplete pairs and wide angles bypass.
   - Its pair test skips the lines of the selective line's SG5 group, which level 2 lets one stack serve (IX7; review, F3, fixed in M4's exit PR). Without the skip, a part one stack of which serves a group was dropped though level 2 kept it.
   - So every part level 2 keeps, level 1 admits. B measured this at 10⁵: 20 sockets gave equal digests with level 1 and without it; CT1's clevis case and CT3's probe case cover the group skip, which no corpus socket exercises.
4. **Level 2 contains verification.** M4's section 6.4 proves this, and KH9 checks it at runtime on every survivor (QR5). So every part F passes survives level 1 and is in K.
5. **Blocking passes nothing.** A blocked socket fails every candidate (SG7, SL8, IX8): B measured 0 passes among 457 files for each of 11 blocked sockets. The full run refuses (exit 3) rather than listing a set that F would reject entirely.
6. **One state, one assembler.** Dedup is exact, since it groups by the digest recomputed from each checked document (QR6; review, F1), the snapshot is one state (QR2), and `MatchDocument::new` orders a total key (CM7).

**What the argument assumes, and what checks each assumption:**

- **The catalog directory is trusted like the binary.** Whoever can write `catalog.db` can store any document with a consistent hash. `verify --deep` detects it but cannot prevent it (VF5).
- **The stored postings are the tokens' postings.** Postings carry no hash (IX9). A corrupt one can drop a part from step 3's containment, never add a pass; plain `verify` finds it (VF1; review, F7, R20). Claim 2 holds for a catalog that `verify` last found consistent and that nothing has damaged since.
- **`EXTRACTOR_VERSION` and `SIGNATURE_VERSION` rise when they must.** The goldens and RX11 check this. M5-14 removed the canary that would catch a build that changed without a rise.
- **M4's section 6.4 holds.** KH9 on every survivor and `--exhaustive` (CT2) check it.

## 5. Changes to fitment-core

Core stays free of I/O and threads. Most of this is additive and lands in PR 1, with `PROPERTIES_VERSION` and `properties/` in PR 5. One change is not additive: B4's compact pair tokens raise `SIGNATURE_VERSION` to 2. `MATCH_RULES_VERSION` and `EXTRACTOR_VERSION` do not change, because no file run's verdict or document changes except the recorded `signature_version`.

```rust
// features/document.rs
impl FeaturesDocument {
    /// The catalog's stored form: serde_json compact, float_roundtrip (GT2).
    pub fn to_compact_json(&self) -> String;
    /// SHA-256 of the compact JSON of the Part alone: no header, source or
    /// versions; None for a refused document (GT6, QR6).
    pub fn part_digest(&self) -> Option<String>;
}

// assembly/parts.rs (Source exists; it gains a checked constructor)
impl Source {
    pub fn new(sha256: &str, bytes: u64, header: Option<FileHeader>) -> Result<Self, BadHex>;
}

// matching/matcher.rs
#[non_exhaustive]
pub struct NotCurrent { pub extractor_version: u32 }       // the document's own version
impl Matcher<'_> {
    /// CF1 for a stored document. Accepted and current: judged as judge_step
    /// judges the file. Refused: the same Reject (candidate_refused) judge_step
    /// gives. Another EXTRACTOR_VERSION: Err(NotCurrent) (QR7, attack 2).
    /// `source` must name the content the document was checked against: the
    /// catalog builds it from the row's own checked SHA-256 (QR7, review F2).
    /// The fit request gets no STEP bytes (candidate_step = None).
    pub fn judge_document(&self, doc: &FeaturesDocument, source: Source)
        -> Result<CandidateResult, NotCurrent>;
    /// As today, now `judge_document_with(&FeaturesDocument::read(step),
    /// Source::of(step, header), Some(step))`: the fit request keeps its bytes
    /// (review F5).
    pub fn judge_step(&self, step: &[u8]) -> CandidateResult;
    /// The one private path both share (QR7). M7's catalog reaches it with a
    /// blob's checked bytes (QR13).
    fn judge_document_with(&self, doc: &FeaturesDocument, source: Source,
                           step: Option<&[u8]>) -> Result<CandidateResult, NotCurrent>;
    /// Level 1 for every socket of the document under this matcher's own
    /// signatures, intersected (QR3).
    pub fn admitted<S: IndexSource>(&self, index: &S) -> Result<Admitted, S::Error>;
}
#[non_exhaustive]
pub enum Admitted { Parts(Vec<u64>), Blocked { socket: String, why: Blocked } }

// matching/index.rs
pub trait IndexSource {
    type Error;
    /// Every posting of `sense` whose bin lies in `bins`: (id, stack, diameter).
    fn postings(&self, sense: Sense, bins: RangeInclusive<i64>,
                each: &mut dyn FnMut(u64, u32, f64)) -> Result<(), Self::Error>;
    /// Every indexed id, ascending (a socket that demands no line).
    fn all(&self) -> Result<Vec<u64>, Self::Error>;
    /// The tokens of each of `ids`, ascending; an id with none is an error.
    fn tokens(&self, ids: &[u64], each: &mut dyn FnMut(u64, &IndexTokens)) -> Result<(), Self::Error>;
}
pub fn level_one<S: IndexSource>(socket: &SocketSignature, source: &S)
    -> Result<Result<Vec<u64>, Blocked>, S::Error>;              // IX1, IX4 to IX8
                          // pairs skip the selective line's SG5 group (IX7, F3)
/// IX5's seat bucket: max(2^ceil(log2(lever)), lever), never below the lever.
fn seat_bucket(lever_mm: f64) -> f64;
impl IndexSource for ShortlistIndex { /* in memory */ }
impl ShortlistIndex { pub fn query(&self, s: &SocketSignature) -> Result<Vec<u64>, Blocked>; } // = level_one
#[derive(Serialize, Deserialize)] pub struct IndexTokens { /* as M4; pairs compact (B4) */ }
#[derive(Serialize, Deserialize)] pub struct PairToken { pub a: u32, pub b: u32, pub distance_mm: f64 }
pub struct Posting { pub sense: Sense, pub bin: i64, pub stack: u32, pub diameter_mm: f64 }
impl IndexTokens {
    pub fn to_json(&self) -> String;                                   // canonical
    pub fn from_json(text: &str) -> Result<Self, FormatError>;         // IX9's checks
    pub fn postings(&self) -> impl Iterator<Item = Posting> + '_;      // IX2
}
pub const SIGNATURE_VERSION: u32 = 2;                                  // B4

// properties/ (PR 5)
pub const PROPERTIES_VERSION: u32 = 1;
pub fn read(step: &[u8]) -> StatedProperties;                          // PR1, PR2
#[non_exhaustive] pub struct StatedProperties { pub stated: Vec<StatedProperty>, pub truncated: bool }
#[non_exhaustive] pub struct StatedProperty {
    pub instance: u64, pub name: String, pub value: String,
    pub measure: Option<String>, pub unit: Option<String>,
}
pub struct PropertyMap;                                                // PR4
impl PropertyMap {
    pub fn shipped() -> Self;
    pub fn meaning(&self, p: &StatedProperty) -> Option<Mapped>;       // (field, meaning, citation)
}
```

Notes on these changes:

- **The `judge_part` trap** (A, finding S1). Today `judge_part` records `ReadFacts.extractor_version = EXTRACTOR_VERSION` whatever `Part` it is given. Fed a stale document's part, it would judge with current rules and claim a current read, so a false positive would be invisible. `judge_document` closes the trap for any store. `judge_part` keeps its documented use: parts this build has just read, such as M4's originals.
- **`judge_step` becomes a wrapper** over the private `judge_document_with(&FeaturesDocument::read(bytes), Source::of(bytes, header), Some(bytes))`. Its output is unchanged, which the match goldens check (after B4's re-bless), and its fit request still carries the bytes (review, F5: a `judge_step` rebuilt on the public `judge_document` would have asked every verifier with `None`, silently losing M7's file-run fit check).
- **`ShortlistIndex::query` becomes `level_one` over the in-memory source,** so the catalog and M4's tests run one algorithm. IX5's per-part lever and its buckets change level 1's internal windows; `seat_bucket` takes `max` with the lever (review, IX5 note). Level 1 is never used by a file run, and the change is proved and tested to keep level 1 ⊇ level 2 (CT3). B's prototype ran on a copy of `index.rs` that has since moved (`Blocked::FailsEvery` is now `#[non_exhaustive]`), so PR 1 re-derives it from the repository's current file.
- **The SG5 skip is inherited** (review, F3, done in M4's exit PR): `ShortlistIndex::pairs_hold` already skips lines of the selective line's SG5 group, at a lever no smaller than any indexed part's, with its regression test `signature.rs::one_stack_for_a_group::level_one_keeps_a_part_one_stack_serves_a_group_with`. PR 1's `level_one` keeps that skip for every `IndexSource`, forming the groups at each part's bucketed lever, and that test keeps passing through `ShortlistIndex::query`.
- **What is not added:**
  - A's `index_bin` and `PostingSource` (bins stay private, and the catalog's largest lever is gone);
  - B's `Matcher::keeps` (a level-2 drop is judged, not ended, under M5-4);
  - C's `fitment.match` v2, `CandidateFlag::MaterialDiffers` and `read_component`.
- **Properties need one more parse.** `properties::read` parses the bytes again unless it shares `FeaturesDocument::read`'s parse. PR 5 measures the second parse against ingest's 0.8 ms average extraction, and shares it if it matters.
- **The review's other fixes need no core API** beyond what is above. The preimages (F1, F2) are `fitment-catalog`'s `preimage.rs`, over core's `sha256_hex`; the recomputed digest is the existing `part_digest()`; F7's posting check is the existing `IndexTokens::postings()`; F4's re-hash, revision references and prune, and F6's directory sync, are the store's. `--jobs` for file runs (M5-30) lives in the CLI, over `judge_step` and `MatchDocument::new`, sharing one `Matcher` across threads as the catalog's workers already do (QR8).

## 6. The schema and the layout

### 6.1 On disk

```text
<DIR>/                            named by -C/--catalog only (M5-9)
  FITMENT-CATALOG                 marker, "fitment.catalog 1\n"; written last by init
  catalog.db                      SQLite, WAL; catalog.db-wal and catalog.db-shm beside it
  blobs/
    sha256/                       the algorithm; a later hash is a sibling
      00/ … ff/                   256 fan-out directories, made by init (about 390 files each at 10⁵)
        ab12…ef                   64 lower-case hex digits; the exact original bytes; mode 0444
  tmp/
    <pid>-<n>.part                in-flight blob writes; never read back; gc removes ones older than 1 h
```

A catalog copied between operating systems stays valid. Documents are byte-identical on Linux, macOS and Windows, and SQLite files are portable.

### 6.2 The database

```sql
PRAGMA application_id = 1718449219;   -- 0x666D7443, 'fmtC'
PRAGMA user_version = 1;              -- the schema version (DB5)
-- per connection: journal_mode=WAL (checked), synchronous=FULL, foreign_keys=ON,
-- fullfsync=ON and checkpoint_fullfsync=ON on Apple, busy_timeout=60000,
-- trusted_schema=OFF, cell_size_check=ON
-- HEX64(x) below stands for: length(x) = 64 AND NOT x GLOB '*[^0-9a-f]*'

CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL) STRICT;   -- format
CREATE TABLE clock (                  -- DB8: +1 per write transaction
  one        INTEGER PRIMARY KEY CHECK (one = 1),
  generation INTEGER NOT NULL CHECK (generation >= 0)
) STRICT;
CREATE TABLE op (                     -- what each write transaction did; history and the tests
  generation      INTEGER NOT NULL,
  n               INTEGER NOT NULL,   -- 0, 1, … within the transaction
  kind            TEXT NOT NULL CHECK (kind IN ('init','ingest','replace','delete','reindex','gc','prune','migrate')),
  entry           INTEGER,
  sha256          TEXT,
  fitment_version TEXT NOT NULL,      -- CARGO_PKG_VERSION of the writer; no wall time (M5-23)
  PRIMARY KEY (generation, n)
) STRICT;

CREATE TABLE blob (                   -- each row promises a file whose bytes hash to sha256 (IN1);
                                      -- held by a current or kept revision of a live entry (IN3)
  key              INTEGER PRIMARY KEY AUTOINCREMENT,   -- level 1's id (IX10); never reused
  sha256           TEXT NOT NULL UNIQUE CHECK (HEX64(sha256)),
  bytes            INTEGER NOT NULL CHECK (bytes >= 0),
  added_generation INTEGER NOT NULL
) STRICT;

CREATE TABLE name (                   -- ID6: an id's revisions, across delete and re-ingest
  id_fold       TEXT PRIMARY KEY,     -- the id under ASCII case folding
  last_revision INTEGER NOT NULL CHECK (last_revision >= 1)
) STRICT;

CREATE TABLE entry (
  key                INTEGER PRIMARY KEY AUTOINCREMENT,  -- internal, never reused, never shown (M5-1)
  id                 TEXT NOT NULL CHECK (length(id) BETWEEN 1 AND 64),   -- ID1, checked again in code
  id_fold            TEXT NOT NULL REFERENCES name(id_fold),
  created_generation INTEGER NOT NULL,
  deleted_generation INTEGER CHECK (deleted_generation IS NULL OR deleted_generation >= created_generation)
) STRICT;
CREATE UNIQUE INDEX entry_live_id ON entry(id_fold) WHERE deleted_generation IS NULL;   -- ID2

CREATE TABLE revision (
  key                   INTEGER PRIMARY KEY AUTOINCREMENT,
  entry                 INTEGER NOT NULL REFERENCES entry(key),
  id_fold               TEXT NOT NULL,
  revision              INTEGER NOT NULL CHECK (revision >= 1),
  sha256                TEXT NOT NULL CHECK (HEX64(sha256)),   -- no FK: a tombstone keeps it (CR6)
  file_name             TEXT CHECK (file_name IS NULL OR length(CAST(file_name AS BLOB)) <= 4096),
  added_generation      INTEGER NOT NULL,
  superseded_generation INTEGER CHECK (superseded_generation IS NULL OR superseded_generation >= added_generation),
  pruned_generation     INTEGER CHECK (pruned_generation IS NULL OR            -- CR11: its bytes released
                          (superseded_generation IS NOT NULL AND pruned_generation >= superseded_generation)),
  UNIQUE (id_fold, revision)                                   -- ID6
) STRICT;
CREATE UNIQUE INDEX revision_current      ON revision(entry)  WHERE superseded_generation IS NULL;  -- CR3
CREATE UNIQUE INDEX revision_live_content ON revision(sha256) WHERE superseded_generation IS NULL;  -- ID5

CREATE TABLE features (               -- one features.v1 document per content and extractor version
  sha256            TEXT NOT NULL REFERENCES blob(sha256) ON DELETE CASCADE,
  extractor_version INTEGER NOT NULL CHECK (extractor_version > 0),
  status            TEXT NOT NULL CHECK (status IN ('accepted', 'refused')),
  encoding          TEXT NOT NULL DEFAULT 'json' CHECK (encoding = 'json'),     -- M5-34
  doc               BLOB NOT NULL,    -- FeaturesDocument::to_compact_json(), exactly (GT2)
  doc_sha256        TEXT NOT NULL CHECK (HEX64(doc_sha256)),   -- over GT2's preimage:
                                      -- sha256, extractor_version, status, part_digest, then doc
  part_digest       TEXT CHECK (part_digest IS NULL OR HEX64(part_digest)),   -- diagnostics and verify;
                                      -- QR6 groups by the digest recomputed from doc
  product_id TEXT, product_name TEXT,                                          -- GT5: for people only
  holes INTEGER, shafts INTEGER, planes INTEGER, complete INTEGER, first_refusal TEXT,
  CHECK ((status = 'accepted') = (first_refusal IS NULL)),
  CHECK ((status = 'accepted') = (part_digest IS NOT NULL)),
  PRIMARY KEY (sha256, extractor_version)
) STRICT;
CREATE TRIGGER features_immutable BEFORE UPDATE ON features
BEGIN SELECT RAISE(ABORT, 'features rows are insert-only'); END;

CREATE TABLE tokens (                 -- IndexTokens of an accepted document; insert-only
  sha256            TEXT NOT NULL,
  extractor_version INTEGER NOT NULL,
  signature_version INTEGER NOT NULL CHECK (signature_version > 0),
  tokens            BLOB NOT NULL,    -- IndexTokens::to_json()
  tokens_sha256     TEXT NOT NULL CHECK (HEX64(tokens_sha256)),   -- over sha256, extractor_version,
                                      -- signature_version, then tokens (GT2)
  lever_mm          REAL NOT NULL CHECK (lever_mm >= 0),          -- for inspection
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

CREATE TABLE posting (                -- one row per stack diameter: level 1's range lookup
  extractor_version INTEGER NOT NULL,
  signature_version INTEGER NOT NULL,
  sense             INTEGER NOT NULL CHECK (sense IN (0, 1)),   -- 0 hole, 1 shaft
  bin               INTEGER NOT NULL,                           -- from IndexTokens::postings()
  content           INTEGER NOT NULL REFERENCES blob(key) ON DELETE CASCADE,
  stack             INTEGER NOT NULL CHECK (stack >= 0),
  diameter_mm       REAL NOT NULL CHECK (diameter_mm > 0),
  PRIMARY KEY (extractor_version, signature_version, sense, bin, content, stack, diameter_mm)
) STRICT, WITHOUT ROWID;
CREATE INDEX posting_owner ON posting(content, extractor_version, signature_version);

CREATE TABLE properties (             -- PR2's bounded capture; insert-only
  sha256             TEXT NOT NULL REFERENCES blob(sha256) ON DELETE CASCADE,
  properties_version INTEGER NOT NULL CHECK (properties_version > 0),
  truncated          INTEGER NOT NULL CHECK (truncated IN (0, 1)),
  stated             BLOB NOT NULL,   -- StatedProperties as JSON, at most 1,024 values
  stated_sha256      TEXT NOT NULL CHECK (HEX64(stated_sha256)),  -- over sha256, properties_version,
                                      -- truncated, then stated (GT2)
  PRIMARY KEY (sha256, properties_version)
) STRICT;
CREATE TRIGGER properties_immutable BEFORE UPDATE ON properties
BEGIN SELECT RAISE(ABORT, 'properties rows are insert-only'); END;

CREATE VIEW live AS                   -- live entries and their current revisions
SELECT e.key AS entry, e.id, r.revision, r.sha256, r.file_name, r.added_generation
FROM entry e JOIN revision r ON r.entry = e.key AND r.superseded_generation IS NULL
WHERE e.deleted_generation IS NULL;
```

Why it is shaped this way:

- **Derived rows are a pure function of (bytes, versions)** (A), and entries only point at them.
  - Replace and delete never rewrite derived data, a reindex never touches entries, and `verify --deep` can recompute any row from its blob alone.
  - Postings are deleted with their tokens, explicitly, when gc removes an older version's rows (RX8).
- **Cascades from `blob`** let delete, prune and gc remove a content's derived rows with one `DELETE FROM blob`, in one transaction (CR1, CR11). A replace keeps the old content's `blob` row and deletes its derived rows explicitly (CR3, IN9).
- **Every derived row's hash names its row** (GT2): the key columns are inside the hashed preimage, so the hash checks where a row sits as well as what it holds.
- **The four partial and unique indexes** enforce ID2, ID5, ID6 and CR3 in the database itself, so no interleaving of processes can create two live entries with one id or one content, or two current revisions of an entry.
- **`posting` is `WITHOUT ROWID`, clustered on its range key** (B). A level-1 probe is one range scan. `features` keeps rowids, since documents of up to 0.7 MB (and 80.8 MB pretty for a hostile part) are poor `WITHOUT ROWID` rows (A).

The SQL that `SqlSource` and the gates run, all in the run's snapshot:

```sql
-- IndexSource::postings
SELECT content, stack, diameter_mm FROM posting
 WHERE extractor_version = :ev AND signature_version = :sv AND sense = :sense
   AND bin BETWEEN :lo AND :hi
 ORDER BY content, stack, diameter_mm;
-- IndexSource::all
SELECT b.key FROM tokens t JOIN blob b ON b.sha256 = t.sha256
 WHERE t.extractor_version = :ev AND t.signature_version = :sv ORDER BY b.key;
-- IndexSource::tokens (one id at a time, ascending); t.sha256 and the bound
-- versions rebuild the preimage that tokens_sha256 is checked against (GT2)
SELECT t.sha256, t.tokens, t.tokens_sha256 FROM tokens t JOIN blob b ON b.sha256 = t.sha256
 WHERE b.key = :id AND t.extractor_version = :ev AND t.signature_version = :sv;
-- QR4: a survivor's features row, every column of the preimage
SELECT f.sha256, f.extractor_version, f.status, f.part_digest, f.doc, f.doc_sha256
  FROM features f JOIN blob b ON b.sha256 = f.sha256
 WHERE b.key = :id AND f.extractor_version = :ev;

-- RX2: extractor-stale live entries, then signature-stale ones
SELECT count(*) FROM live l
  LEFT JOIN features f ON f.sha256 = l.sha256 AND f.extractor_version = :ev
 WHERE f.sha256 IS NULL;
SELECT count(*) FROM live l
  JOIN features f ON f.sha256 = l.sha256 AND f.extractor_version = :ev AND f.status = 'accepted'
  LEFT JOIN tokens t ON t.sha256 = l.sha256 AND t.extractor_version = :ev AND t.signature_version = :sv
 WHERE t.sha256 IS NULL;
```

By IN9, every `posting` row belongs to content a current revision of a live entry names, never to a kept revision's, so level 1 needs no join with `live`. A survivor's key maps to its live entry through `blob.sha256` and `revision_live_content`.

### 6.3 What M6 adds (not built in M5)

The shape is reserved so that M5's schema needs no rework (A section 8.6):

- **Assemblies** are blobs like parts, with a `kind` column (default `part`) added by a DB5 migration.
- **Sockets** go in a `socket` table keyed by (assembly SHA-256, selection, `SOCKET_RULES_VERSION`), holding the reviewed `fitment.socket` document and its digest. The table is insert-only: a review is a new row, carrying its predecessor's generation.
- **Reports** go in a `report` table holding `fitment.match` JSON, its digest, every version, and the generation and the counts of CM8. They are history, never input (M4-23).

## 7. The CLI

`-C/--catalog DIR` is accepted anywhere after `catalog`. Synopses use clap's notation. Every command's exit codes follow CL2; the tables say when each applies.

### 7.1 `fitment catalog init -C <DIR>`

Makes a catalog in DIR, creating DIR if needed. The marker is written last. stderr: `fitment: parts: catalog created`.

| Exit | When |
| --- | --- |
| 0 | Created; or DIR is already a catalog of this schema ("already a catalog: unchanged") |
| 1 | DIR or its files cannot be created |
| 2 | DIR exists, is not empty and is not a catalog |
| 3 | DIR is a catalog of a newer schema |

### 7.2 `fitment catalog ingest -C <DIR> <FILE|DIR>... [--id <ID>] [--max-mib <N>] [--format table|json]`

Ingests each STEP file. A directory argument contributes its `.stp`, `.step` and `.p21` files, in any case, not recursively (PH4). `--id` needs exactly one file. Files go in ID4's order. One line per file on stdout:

```text
added        m_bracket            1b77ea97  'm_bracket'  5 holes, 0 shafts, 6 planes
unchanged    a_b                  b3795fa5  'p03_blind_flat'
quarantined  r04_assembly         f8386a47  ''  refused: assembly: the file is an assembly with 2 usages of 3 product definitions; features reads one part
duplicate    copy_of_m_bracket    1b77ea97  the same bytes are m_bracket: not added
conflict     a_b                  e0382a9d  the id a_b is taken by another file (give --id): not added
refused      huge                 -         larger than --max-mib 128: not added
error        nowhere.stp          -         cannot read nowhere.stp: No such file or directory
```

The first five lines are C's prototype's (`C/proto/run.txt`), with the PRODUCT.id column added. stderr: `fitment: parts: 8 files: 4 added, 2 quarantined, 1 duplicate, 1 conflict`, then one line per quarantined file with its first reason, and GT8's line for an internal refusal.

| Exit | When (CL2's precedence across files) |
| --- | --- |
| 0 | Every file `added`, `unchanged`, or a `duplicate` under a default id |
| 1 | A file unreadable; a blob or row unwritable; an internal refusal; the catalog busy |
| 2 | `--id` with several files; an invalid `--id`; an input inside DIR; a `conflict`; a `duplicate` under `--id`; not a catalog |
| 3 | A file `quarantined` or `refused` (size, not a regular file); a newer schema |

### 7.3 `fitment catalog list -C <DIR> [--status indexed|quarantined|stale|all] [--format table|json]`

The default is `--status all`. A table, sorted by id case-folded:

```text
id                        rev  status       sha256    holes shafts planes  product           file
a_b                       r1   indexed      b3795fa5      1      0      7  'p03_blind_flat'  a_b.stp
clevis21                  r1   indexed      4fff4203      6      0     16  '1'               clevis21.stp
evil_31mred_              r1   indexed      622fec33      1      0      7  'p05_cbore'       evil\u{1b}[31mred\u{202e}.stp
junk                      r1   quarantined  f3ccaf5b      -      -      -  ''                junk.stp
```

The `status` column shows `indexed`, `quarantined`, `stale (extractor 6)`, `stale (signature 1)` or `stale (properties)` (RX2), computed on every read and never stored. An incomplete part (VD11) shows `indexed*` with a footnote.

With `--status quarantined`, each entry is followed by every refusal and the extractor version that refused it, one per line (C's `quarantine` command, folded in):

```text
r04_assembly              r1   quarantined  f8386a47  refused by extractor 6
    assembly: the file is an assembly with 2 usages of 3 product definitions; features reads one part
```

stderr: `fitment: parts: generation 14: 11 entries: 8 indexed, 3 quarantined, 0 stale; 2 old revisions kept (4.1 MB: fitment catalog prune --revisions)` (the last clause only when some are kept; M5-41). Exit 0; 1 unreadable; 2 not a catalog; 3 newer schema.

### 7.4 `fitment catalog get -C <DIR> <REF> [--format table|json|features|step] [-o <OUT>]`

REF is `ID`, `sha256:HEX` or a strict `ID@rN` (ID9). `ID@rN` may name a superseded revision; the other forms name the current one.

| Format | Prints |
| --- | --- |
| `table` (the default) | **The entry:** id, revision, status, SHA-256, bytes, file name, PRODUCT.id and name, versions, feature counts. **Then:** its refusals; the stored part-level properties one per line, escaped, with their mapped meaning (PR7); its history (`r1 1b77ea9724e6 generation 3 ingest (bytes kept)`, `r2 46cd8a91f0a2 generation 9 replace`, …). A tombstone prints its history with `deleted`. A superseded revision prints its own line of the history and whether its bytes are kept |
| `json` | A `fitment.catalog` document holding this entry, with `properties` and `history` |
| `features` | The stored document as `to_json()`, byte-identical to `fitment features <its file>` with this build (GT2). For a quarantined entry, its refused document, exit 3. For a superseded revision, exit 2: its derived rows are gone (IN9) |
| `step` | The stored bytes, to `-o OUT` only (PH7), atomically (CL7); without `-o`, exit 2. **Re-hashed first** (ST11; review, F4(a)): the blob is read once into one buffer and its SHA-256 compared with the revision's; only that buffer is written. On a mismatch nothing is written, OUT is untouched, and the exit is 1: `fitment: parts: BRKT-0042 r1: damaged: the file's SHA-256 is not its name: run fitment catalog verify -C parts`. A superseded revision's kept bytes are written the same way; pruned or deleted bytes are exit 2 (ID9) |

stderr, for every format: what was printed or written, as `fitment: parts: BRKT-0042 r1 sha256:1b77ea9724e67c4bdc709a871a00a05580dfebb36e4109f3c027d84afe826c33 -> bracket.stp` (review, F4(b)).

Exit 0; 1 unreadable blob or row, or a damaged blob (`step`); 2 unknown or ambiguous REF (with suggestions), a malformed `@r`, a revision the id never had, `features` of a superseded revision, `step` of bytes no longer kept, `step` without `-o`, OUT inside DIR; 3 a quarantined entry's `features`, a stale entry's `features` ("read by another fitment"), or a newer schema.

### 7.5 `fitment catalog replace -C <DIR> <REF> <FILE> [--max-mib <N>] [--format table|json]`

Gives the entry new bytes and keeps its id; its revision rises by one. REF may be `ID@rN` naming the current revision, so a script replaces only the revision it read (ID9). The old content's derived rows go in the same transaction; its blob is **kept**, so `get ID@r<old> --format step` still writes the bytes a past report judged, until `prune --revisions` (CR3, CR11; review, F4(b)).

```text
replaced     m_bracket  r2  46cd8a91  'm_bracket_thick'  5 holes, 0 shafts, 6 planes   (was r1 1b77ea97, bytes kept)
```

| Exit | When |
| --- | --- |
| 0 | `replaced`; or `unchanged` (the same bytes) |
| 1 | Unreadable or unwritable; busy |
| 2 | Unknown REF; `ID@rN` not the current revision; FILE's bytes are another entry's (`duplicate`); FILE inside DIR |
| 3 | FILE refused by the gate or by size: the entry unchanged (CR4), with the refusal printed |

### 7.6 `fitment catalog delete -C <DIR> <ID>...`

Deletes each entry: it becomes a tombstone (CR6), its content's rows go, with the blobs of its kept revisions (CR1), and the inline gc unlinks them. Each ID may be `ID@rN` naming the current revision (ID9). All or nothing on unknown ids (CR2). One line per entry: `deleted  m_bracket  r2  46cd8a91`. If the unlink fails, a warning names the orphan and `fitment catalog gc`.

Exit 0; 1 unwritable or busy; 2 an unknown id, an `ID@rN` that is not current, or a `sha256:` reference.

### 7.7 `fitment catalog reindex -C <DIR> [<REF>...] [--all] [--format table|json]`

Processes RX9's set. One line per entry processed, with its transition:

```text
indexed      m_bracket   r3  1b77ea97  stale (extractor 6) -> indexed
indexed      junk2fix    r1  9a0b11c2  quarantined -> indexed (now searched)
quarantined  BRKT-0042   r2  77aa01de  indexed -> quarantined: unread_shape_item: … (no longer searched)
```

stderr: `fitment: parts: 11 reindexed: 9 indexed, 2 quarantined; 1 newly searched, 1 no longer searched`, then RX6's lines.

| Exit | When |
| --- | --- |
| 0 | Every entry processed is current, and none newly quarantined |
| 1 | A missing or damaged blob (RX5); unwritable; busy |
| 2 | Unknown REF, or an `ID@rN` that is not current (ID9) |
| 3 | An entry newly quarantined (RX6) |

### 7.8 `fitment catalog verify -C <DIR> [--deep] [--full]`

VF1 to VF3. One problem per line:

```text
damaged    clevis21  4fff420338c4  the file's SHA-256 is not its name
damaged    a_b       b3795fa58c1e  features (extractor 6): the row's hash does not cover its key, status or digest
mismatch   m_bracket 1b77ea9724e6  features (extractor 6): part_digest is not the document's
mismatch   m_bracket 1b77ea9724e6  postings (signature 2): 1 missing, 0 extra, 0 changed against the tokens
temporary  tmp/4411-3.part
```

Without `--deep`, it checks every row's preimage hash, every part-digest column and every posting (VF1; review, M5-40). stderr: `fitment: parts: 5 problems` (or `consistent`). Exit 0 consistent; 3 problems; 1 cannot run; 2 not a catalog.

### 7.9 `fitment catalog gc -C <DIR> [--versions] [--vacuum]`

ST5 and ST7: unlinks orphan blobs and temporary files older than one hour, then runs `wal_checkpoint(TRUNCATE)`. With `--versions`, it also deletes derived rows of older versions where current ones exist (RX8). With `--vacuum`, it runs `VACUUM` after checking the free space (SZ4). gc writes no row when it only unlinks, so a full disk never blocks its own cure.

Exit 0; 1 unwritable or busy; 2 not a catalog.

### 7.10 `fitment catalog prune -C <DIR> --revisions`

New after the review (F4(b); M5-41, CR11). Releases the bytes of every superseded revision of a live entry that replace kept, in one write transaction under the lock, as gc takes it, then unlinks them by the inline gc (ST5). `--revisions` is required. One line per revision pruned, then stderr's count:

```text
pruned       m_bracket  r1  1b77ea97  14213 bytes
```

stderr: `fitment: parts: 1 revision pruned, 14213 bytes released`. After it, `get m_bracket@r1 --format step` exits 2, and its table still shows r1's history and SHA-256.

Exit 0 (also when nothing was kept); 1 unwritable or busy; 2 without `--revisions`, or not a catalog; 3 a newer schema.

### 7.11 The `fitment.catalog` document (v1)

```json
{
  "format": "fitment.catalog", "format_version": 1,
  "schema_version": 1, "generation": 14,
  "build": { "extractor_version": 6, "signature_version": 2, "properties_version": 1 },
  "counts": { "entries": 11, "indexed": 8, "quarantined": 3, "stale": 0 },
  "entries": [
    { "id": "m_bracket", "revision": 3, "status": "indexed",
      "sha256": "1b77ea9724e67c4bdc709a871a00a05580dfebb36e4109f3c027d84afe826c33", "bytes": 14213,
      "file_name": "m_bracket.stp",
      "product": { "id": "m_bracket", "name": "m_bracket" },
      "versions": { "extractor_version": 6, "signature_version": 2, "properties_version": 1 },
      "complete": true,
      "features": { "planes": 6, "holes": 5, "shafts": 0, "cones": 0, "other_faces": 0 },
      "refusals": [],
      "properties": null,
      "history": null }
  ],
  "changes": null
}
```

- **`status`** is one of `indexed`, `quarantined` and `stale`; `versions` says which version made it stale.
- **`refusals`** are `{code, message, instance}`, as `features.v1` writes them.
- **`properties`** is filled by `get` and null elsewhere. For example: `{"truncated": false, "stated": [{"instance": 7270, "name": "MaterialMissingAssignments", "value": "TRUE", "measure": null, "unit": null, "mapped": {"field": "material", "meaning": "unassigned"}}]}`.
- **`history`** is filled by `get` and null elsewhere: `[{"revision", "sha256", "generation", "event": "ingest|replace|delete|prune", "bytes_kept": true|false}]`.
- **`changes`** is filled by `ingest`, `replace`, `delete`, `reindex` and `prune` with `--format json`: one object per input or entry, `{"input", "result", "id", "revision", "sha256", "detail"}`. `entries` then holds the entries changed.
- **The internal key** appears nowhere.

### 7.12 `fitment match` with a catalog

```text
fitment match <SOCKET> (<CANDIDATE>... | -C/--catalog <DIR> [--id <ID>... | --exhaustive])
              [--jobs <N>] [--rejects <N|all>] [--original <ASSEMBLY>]
              [--format table|json] [-o <OUT>] [--explain] [--require-fit] [--require-reviewed]
              [--clearance strict|functional] [--tolerance-mm <X>] [--tolerance-deg <Y>]
```

- **`--exhaustive`** (review; M5-42, QR10) is documented: "judge every live catalog entry, bypassing level 1; slower, and lists every entry". It replaces the hidden `--audit`.
- **`--jobs N`** applies to file runs and catalog runs alike, default `available_parallelism()`; the document does not depend on N (review; M5-30, QR8).
- **`--catalog`** is given once (review; M5-44). `--id` takes `ID` or a current `ID@rN` (ID9).
- **Labels** are `catalog:<id>@r<n>` (CM6).

| Exit | When | Order |
| --- | --- | --- |
| 2 | Usage: CM1 (`--catalog` twice included); an unknown or ambiguous `--id`, or an `--id ID@rN` that is not current; DIR not a catalog; OUT inside DIR or equal to an input; every M4 usage error | first |
| 3 | QR1: the socket gate; a newer schema; stale entries; in a full run, nothing indexed or a blocked socket. Nothing on stdout | before any candidate |
| 1 | `catalog.db` unreadable; a stored row that fails QR4 (its preimage hash, its parse, its version or status, or its recomputed part digest); a pass after a level-2 drop; every M4 error | over every verdict |
| 0, 4, 5 | VD14 | |

A full run's table against s01's bracket socket, abbreviated. CM8 and CM10 fix the shape; the values are illustrative:

```text
socket sha256:124f4f64627f  target 'BRACKET'  1 socket(s)  policy 0.01 mm / 0.01 deg, clearance strict, grip hard  fit: not checked (no fit verifier in this build)
catalog parts  generation 41  entries 1200: 1180 searched, 20 quarantined (not searched)  level 1 kept 37
INTERFACE  catalog:BRKT-0042@r1  23cb289f  'BRACKET'  all requirements verified; material_not_compared, requirements_unreviewed
    socket-1/seat:plane-467  passed  plane-88  margin 0.01 mm
    …
reject     catalog:BRKT-0107@r3  9a0c11e2  'L-BRKT'  1 failed: socket-1/bore:hole-654 no_feature
reject     catalog:BRKT-0311@r1  5d0e9f01  -  shortlist: socket-1/bore:hole-654 diameter [6.590001, 6.609999]
  … 24 more rejects (in the document)
```

## 8. Measurements

The numbers are as the designers measured them, on their machines (see the preamble). They come from prototypes, not from the design as merged. Three differences matter:

- B's query times end a candidate at a level-2 drop, where M5-4 judges it.
- B's level-1 table before IX5 used the catalog's largest lever.
- A's documents are pretty JSON, where M5-10 stores compact JSON.

Where a number is an estimate, it says so.

### 8.1 Stored documents judge as files do

| What | Result | Source |
| --- | --- | --- |
| Round trip: `from_json(to_json(read(x))) == read(x)`, and the JSON written again byte-equal | 243 of 243 accepted documents among 490 STEP files (corpus and fixtures; 247 refused, mostly assemblies) | A `exp/src/bin/roundtrip.rs` |
| The same, from pretty and from compact JSON | 232 of 232 accepted corpus files (the 4 hostile ones included) | B `out/roundtrip.txt`, `out/roundtrip-bases.txt` |
| `judge_step(bytes)` against judging the stored document, the whole `CandidateResult` compared | 90 of 90 corpus match cases, with sockets from `fitment socket`; the `MatchDocument` digest equal in 90 of 90 | A `exp/src/bin/equiv.rs` |
| The same | 105 of 105 file-candidate match cases with a socket (the other 10 are refused files or the refused `r27` socket) | B `out/roundtrip.txt` |
| A catalog run against a file run over 457 files (228 parts, 229 refused) | All 21 unblocked sockets: the catalog lists exactly the accepted candidates level 2 kept, each entry equal, labels aside, and in the same order. The 11 blocked sockets pass 0 of 457 each in the file run | B `compare.py`, `out/equivalence.txt` |
| Level 1 bypassed against level 1 used, at 10⁵ | The same digest for all 20 sockets tried, at the catalog's largest lever and again at each part's own | B `out/level1-sound-1e5.txt`, `out/level1-own-lever-1e5.txt` |
| Cost over A's 243 accepted documents | Reading STEP 187 ms; `from_json` 21 ms (at most 0.9 ms for a 689 KB document); `CandidateView::of` 3.1 ms; signature and tokens 0.7 ms | A `exp/src/bin/roundtrip.rs` |
| Compact against pretty JSON | 37.3% of the bytes; `from_json` 0.029 ms a part against 0.038 ms (corpus parts) | B `out/sizes.txt` |

### 8.2 Sizes

| What | Measured | Source |
| --- | --- | --- |
| Documents against STEP, A's 243 accepted files (pretty) | 15.0 MB of documents for 35.7 MB of STEP (0.42); median 27.6 KB, 90th percentile 83.7 KB, largest 688.6 KB; at most 1.39 times the file (`nist_ftc_08_asme1_rc`, 450,741 → 628,571 bytes); 62 KB on average, so 6.2 GB pretty at 10⁵ | A `exp/featsize.py`, `tokdist.rs` |
| Per part, compact, corpus parts | Features 10.6 KB, signature 1.9 KB, tokens 0.5 KB | B `out/sizes.txt` |
| Per part, compact, 24 NIST-like fixture parts | Features 135.7 KB, signature 13.2 KB, tokens 36.6 KB | B `out/sizes.txt` |
| Tokens (version 1), A's documents | Median 126 B, 90th percentile 1.9 KB, largest 348 KB (`nist_ctc_02_asme1_ap203`, 84 stacks) | A `tokdist.rs` |
| Largest features document | 80.8 MB pretty, for `h_bracket_stubs` (a 36.7 MB STEP file) | B |
| Compression | Deflate would store 8% of pretty JSON's bytes; compact JSON stores 37% | B (B3) |

### 8.3 Catalogs built (B)

`probe ingest` builds the catalogs. Each base part is copied with its length unit replaced by a `CONVERSION_BASED_UNIT` of s mm, with s log-uniform in [0.5, 2), so every length scales exactly as a reader applies it. The 228 bases are the generated corpus's accepted parts, less the 4 hostile ones. The "dup" catalogs change only the header's `FILE_NAME`: identical parts with different SHA-256s. "fx" is the same from the 24 accepted fixture parts.

| Catalog | Parts | Postings | Ingest wall (16 threads) | Read a part (CPU) | Database | Of which features / tokens |
| --- | --- | --- | --- | --- | --- | --- |
| c3, scaled | 1,140 | 2,515 | 0.11 s | 0.28 ms | 35 MB (pretty) | 33 / 0.6 MB |
| c4, scaled | 10,032 | — | — | — | 313 MB (pretty) | — |
| c5, scaled | 100,083 | 220,800 | 9.8 s | 0.23 ms | 3.12 GB (pretty) | 2.94 GB / 55 MB |
| c5c, scaled, compact | 100,083 | 220,800 | 4.9 s | 0.26 ms | **1.32 GB** | 1.16 GB / 55 MB |
| d5, dup | 100,092 | 220,817 | 8.3 s | 0.23 ms | 3.01 GB (pretty) | 2.84 GB / 46 MB |
| fx4, NIST-like dup, compact | 7,928 | 193,984 | 16.6 s | 7.9 ms | 1.53 GB | 1.18 GB / 337 MB |

For corpus-sized parts the single writer bounds ingest: about 0.05 ms a part with compact JSON, 0.1 ms with pretty. For NIST-like parts reading the STEP file bounds it, at 7.9 ms of CPU each. At that size, 10⁵ parts would be about 19 GB (an estimate).

### 8.4 Query cost (B)

**Whole queries, before IX5** (`out/bench.txt`; SQL level 1 at the global lever, pretty JSON, default policy). "L1/L2" is level-1 hits over level-2 kept; times are in ms with 1 thread and 16 threads.

| Socket | 10³: L1/L2, 1 t, 16 t | 10⁴: L1/L2, 1 t, 16 t | 10⁵: L1/L2, 1 t, 16 t |
| --- | --- | --- | --- |
| s01 BRACKET (4 bores, 2 seats, grip) | 36/21, 1,172, 213 | 46/23, 1,123, 167 | 177/32, 1,182, 225 |
| s01 NUT | 21/17, 27, 12 | 44/32, 74, 23 | 345/271, 656, 114 |
| s02 BLOCK (one line) | 208/21, 207, 38 | 992/22, 246, 49 | 8,844/36, 713, 185 |
| s05 WASHER (2 sockets) | 85/1, 15, 13 | 608/1, 46, 24 | 6,061/2, 379, 161 |
| s07b BRACKET (2 sockets) | 35/20, 2,187, 310 | 36/21, 2,174, 322 | 56/28, 2,172, 410 |
| s19 HOUSING | 13/12, 62, 14 | 92/84, 386, 64 | 748/650, 2,848, 343 |
| s03 PLATE | 2/2, 24, 27 | 2/2, 28, 30 | 3/2, 47, 50 |

**Level 1 after IX5,** at 10⁵ and 16 threads (`out/level1-bucket-lever-1e5.txt`; each run's document equal to the bypass run's):

| Socket | Level-1 hits | Level 1 | Whole query |
| --- | --- | --- | --- |
| s02 SCREW | 31 | 0.22 ms | — |
| s01 BRACKET | 95 (was 177) | 33 ms | 196 ms |
| s01 PLATE | 99 (was 177) | 31 ms | 92 ms |
| s07b BRACKET | 56 | 35 ms | 362 ms |
| s02 BLOCK | 8,844 | 31 ms | 162 ms |
| s05 WASHER | 6,061 | 43 ms | 122 ms |

- **Where the time goes.** Judging the survivors dominates every query over 50 ms. A bracket costs 84 ms of CPU against s01, and one s07b candidate cost up to 205 ms. Loading and parsing cost 0.13 ms a hit with pretty JSON; level 2 about 6 µs a hit.
- **Before IX5,** level 1 took 19 to 82 ms at 10⁵, with a floor of 20 to 25 ms: `SELECT MAX(lever_mm)` alone scanned 10⁵ rows in 24.5 ms. Per-part seat groups without buckets took 286 ms for s01's 177 hits.
- **Level 1 held in memory instead:** loading every token takes 2 ms at 10³, 16 ms at 10⁴ and 153 ms at 10⁵. Per query that was 1.5 to 3.6 times slower than SQL at 10⁵, with no gain below. A measured the same shape on real tokens: a full load, parse and index of 10⁴ and 10⁵ parts took 167 ms and 1,729 ms (42.6 and 427.5 MB of tokens and postings), against one posting range query of 1.15 ms and 13.7 ms (`tokdist.rs`).
- **Bypassing level 1** at 10⁵ (level 2 on every part) takes 0.97 to 1.23 s of wall time at 16 threads and about 10 s of CPU, mostly loading and parsing (9.6 s for the 98,766 parts of the seats-only run).
- **The stale check** on B's schema: 17 µs through an index at 10⁵; a scan took 21 ms.
- **Threads:** 16 threads cut wall time 4 to 8 times on queries over 100 ms. Byte-identical documents across thread counts and with level 1 from SQL or from memory (`out/determinism.txt` records 1 and 16 threads and the in-memory level 1).

**s01's bracket against 10⁵ parts** (B section 6.1, default policy): one probe (bins 58 to 67) gives 177 parts, and the plane and pair filters at each part's own lever leave 95, in 33 ms (at the catalog's largest lever, 314 mm, all 177 would pass). Their 95 documents parse in about 10 ms of CPU, and level 2 keeps 32, with 32 distinct digests. Judging them takes about 1.4 s of CPU (the slowest part 92 ms), 0.17 s of wall time at 16 threads: 11 interface matches (the corpus's passing brackets at scale 1) and 21 rejects, 196 ms in all. Under M5-4 the other 63 survivors are judged too and listed at stage `shortlist`; that time is not measured.

### 8.5 Worst cases (B, `out/dedup.txt`, `out/query-*.txt`)

| Case | Measured | With the fix |
| --- | --- | --- |
| 10⁵ exact copies, s01 BRACKET | L1 15,365, L2 9,219 (4,829 interface matches): 48.5 s at 16 threads, 771 s of CPU; document 111 MB | Dedup (QR6): 21 distinct parts judged, 1.76 s, byte-identical document |
| 10⁵ exact copies, s02 BLOCK | L1 54,875, L2 9,219: 8.6 s | Dedup: 1.84 s, byte-identical |
| Seats-only socket (s01's bracket with its 4 bores removed in review), c5 | L1 98,766 (planes filter only), L2 202: 2.2 s wall, 10 s CPU in load and parse | Plane tokens (M5-35) would admit about 5.8% (measured at 10⁴) |
| Seats-only, 10⁵ exact copies | L2 11,853: 86 s wall, 1,263 s CPU | Dedup: the distinct bases that fit (not measured) |
| Hostile parts in the catalog (`h_*`) | s01 BRACKET: 3.26 s, of which 3.17 s is `h_hole_grid`'s judgment | None needed: bounded per candidate (QR9) |
| NIST-like parts, 7,928 copies | Level 1 alone 255 to 262 ms (1,668 hits parse 42 KB of tokens each) | Compact pair tokens (M5-11), estimated 2 to 3 times less (not measured) |

**Re-reading the catalog:** 10⁵ corpus parts in 4.9 s at 16 threads (0.26 ms a part). For NIST-like parts at 7.9 ms each, about 13 minutes of CPU at 10⁵, or about 1 minute at 16 threads (estimated).

### 8.6 The store (A, `exp/src/bin/`)

| Experiment | Measured |
| --- | --- |
| `sq.rs fsync` | A 30 KB blob: 0.21 ms unsynced; 4.05 ms with `fsync(2)` and a directory sync; 7.90 ms with `sync_all` (`F_FULLFSYNC`) and a directory sync |
| `psync.rs` | `sync_all` per blob with one directory sync per batch: 3.30, 3.27 and 3.31 ms a blob on 1, 4 and 8 threads (the flush is per device) |
| `sq.rs commit` | A one-row WAL commit: 0.09 ms with `synchronous=NORMAL`, 0.12 ms with `FULL`, 3.74 ms with `FULL` and `fullfsync=ON` |
| (derived) | Ingesting 10,000 files: about 33 s of syncing beside about 8 s of extraction (0.8 ms a file in process, at most 21 ms) |
| `sq.rs conc` | 8 processes × 200 read-then-write transactions: `BEGIN DEFERRED` gave 1,543 `SQLITE_BUSY` failures despite a 30 s timeout, and 57 rows; `BEGIN IMMEDIATE` gave 0 failures and 1,600 rows in 780 ms |
| `sq.rs snapshot` | A reader's snapshot survives a committed delete and a checkpoint attempt, which returns busy; the WAL reached 13.0 MB after 300 × 30 KB inserts and went to 0 once the reader ended |
| `sq.rs crash` | 300 kill -9 rounds of the ingest protocol: every invariant held every round; orphans and temporary files reached 151 and 136 without gc, at most one of each per kill. Committing the row first broke an invariant in round 0 |
| `race.rs` | gc in a loop against four ingest and delete processes for 20 s: 0 missing in 5,120 checks; the unlink-after-commit control: 6 missing in 5,301 |
| `sq.rs full`, `diskfull.rs` | `max_page_count` 200: `SQLITE_FULL` after 26 commits, 26 rows, integrity ok. On a 16 MB APFS image, the blob write failed with ENOSPC (temporary file removed) and SQLite failed with "database or disk is full", every committed row intact, `integrity_check` ok |
| `sq.rs unordered` | `reverse_unordered_selects` reverses a `SELECT` without `ORDER BY` and leaves `ORDER BY` alone |
| `hash.rs` | SHA-256 of 7.4 MB: core 327 MB/s, `sha2` 535 MB/s, equal digests; so 23 ms for the largest fixture, and no `sha2` dependency |
| `cargo +1.85 build` | rusqlite 0.40.2 with bundled SQLite 3.53.2 builds on Rust 1.85 |
| (from M5A-4) | A signature-only rebuild costs about 0.1 ms a part |

### 8.7 What C's prototype showed (`C/proto/run.txt`)

`proto_catalog.py` ran C's CRUD semantics with the real gate over the unit parts, three candidates and five fixtures. It confirmed:

- a second ingest is `unchanged`, and a replace with a refused file leaves the entry at its revision and SHA-256 (exit 3);
- a delete with one unknown id deletes nothing, and re-ingesting a deleted id continues its revisions (r1, r2, deleted, r3);
- `verify` finds an appended blob and a stray temporary file, and every entry lists as `stale` under another extractor version;
- a hostile file name prints escaped, and `r04_assembly.stp`, a junk file and `nist_ctc_04` are quarantined with the gate's own messages.

It changed three rules: ID4 (exact stems first), ID5's exit (a duplicate exits 2 only under `--id`, or a scheduled sync would fail on every run), and PR2's bound (1,024, not 256, since `nist_stc_06` states 440 part-level properties).

## 9. Tests

New and extended files, in the style of `cli.rs` and `match_cli.rs`: `crates/fitment-catalog/src` (`#[cfg(test)]`) and `tests/{store,fault,soak}.rs`; `crates/fitment/tests/{catalog_cli,match_cli,catalog_corpus}.rs`; `crates/fitment-core/tests/{index,stored,properties}.rs`. `assert_cmd` and a fresh scratch directory per test; inputs from `crates/fitment-core/tests/{data,candidates,assemblies}`; fixtures optional unless `FITMENT_REQUIRE_FIXTURES=1`, the corpus under `FITMENT_REQUIRE_CORPUS=1` in CI. Every match document goes through the test-side checker (`tests/common/checker.rs`, KH12), unchanged since v1 is. The CLI tests link `fitment-catalog` with its `test-support` feature, for the seams and for constructors that write rows of other versions.

- **CT1, the M4 corpus through a catalog** (B CT1, A T-ix-exit, C 10.5; M5's exit test). Every corpus part file is ingested into a fresh catalog, assemblies and refused files quarantined. For every match-case socket, under `default`, `functional` and `--tolerance-mm 0.05` (QR3):
  - **the full run's** envelope equals that of `fitment match S <every live file>`; its entries are exactly that run's entries for the level-1 survivors, byte-identical, labels aside, and in the same order; its digest equals `fitment match S <the survivors' files>`; every candidate the file run passes is in it;
  - **a named run** (`--id`) for each case's candidate has the file run's digest and exit code, and a quarantined candidate gives CR9's `Reject`;
  - **KH2's strict gate** holds on the catalog runs: every passing case appears with the same entry, every bad case is rejected for its injected defect with the same evidence, and there is no false positive;
  - **blocked sockets** exit 3 in a full run with nothing on stdout (M5-13; B's prototype expected 5) and 5 in a named run;
  - **the 10 component cases** (`m_s11_bracket_own` …) are listed as not run through a catalog (M5-17), so the coverage test sees them; the false-negative baseline (KH4) is unchanged;
  - **every label** is `catalog:<id>@r<n>`, and `get` of that reference writes the bytes whose SHA-256 the entry's `source` states;
  - **`catalog_corpus.rs::a_clevis_pin_one_shaft_for_two_holes_is_found`** (review, F3): no corpus socket has two same-sense coaxial lines in one SG5 group (the review scanned B's 32), so a constructed case is added beside the corpus. A clevis socket with two coaxial Ø10 ± 0.01 holes over spans [0, 5] and [15, 20], one SG5 group; a pin with one Ø10 shaft over [−1, 21]; a decoy pin whose shaft is Ø10.2. The full run lists the pin and passes it exactly as `fitment match S pin.stp` does, digest included, and `--exhaustive` gives the file run's digest.
- **CT2, exhaustive equals the file run** (B CT2, C CM6; M5-42). **`catalog_corpus.rs::exhaustive_equals_the_file_run`:** for every corpus socket and both policies, `--exhaustive` has the digest of a file run over all the catalog's files, quarantined ones included, and its passes are the full run's: level 1 hides nothing on the corpus. The same comparison runs on CT1's clevis case and on CT16's lost-posting case, where it is the guard that sees the hidden part.
- **CT3, level 1** (B CT3, A T-l1). `level_one` over `SqlSource` equals it over `ShortlistIndex`, id for id, for every corpus socket against the corpus catalog and a scaled 10³ one. For every (socket, part) pair of the corpus, level 2 keeping implies level 1 admitting. Adding 1,000 scaled parts, one with a 5 m lever, changes no corpus part's admission (IX6). A proptest on random tokens and windows, unbounded ends included, with and without IX4's ordering; it also draws SG5 groups of two and three coaxial lines, served by one stack or by several (review, F3). `u64::try_from` refuses a negative key. Named for the review:
  - **`signature.rs::one_stack_for_a_group::level_one_keeps_a_part_one_stack_serves_a_group_with`**, M4's exit PR's regression test for F3, inherited, which must keep passing through `level_one` over `ShortlistIndex`;
  - **`index.rs::sql_source_keeps_a_part_one_stack_serves_a_group_with`**, the review's probe (`review/probe/ws`, `m5_review_probe`) over `SqlSource`: the groups are `[0, 0]`, level 2 keeps the part, and level 1 admits it;
  - **`index.rs::a_seat_bucket_is_never_below_the_lever`** (review, IX5 note): for x = 2^k and the next `f64` above it, k from −10 to 12 (32, 256 and 1024 among them), and 10⁴ random levers, `seat_bucket(x) >= x`, and the seat-group count at the bucket is never above the count at x.
- **CT4, floats and bins in SQLite** (B CT4). Diameters at k · 0.1 mm ± 1 ulp, the extremes 1e-6 and 2e6 mm, and unbounded window ends round-trip bit-equal through `posting.diameter_mm`; the probe returns them, and `Window::holds` decides as in memory.
- **CT5, stored documents judge as files do** (B CT5, A GT2). For every unit, committed-candidate, corpus and fixture part: `from_json(to_compact_json(read(x))) == read(x)`, `to_json` byte-equal to `fitment features`, and `judge_document == judge_step` with the whole `CandidateResult` compared. `judge_document` returns `NotCurrent` with the document's own version for a document edited to version 5. `get --format features` equals `fitment features FILE`. Named for the review:
  - **`stored.rs::judge_step_hands_its_bytes_to_the_fit_request`** (F5): a recording `FitVerifier` sees `candidate_step == Some(bytes)` from `judge_step`, and `None` from `judge_document`, with otherwise equal requests;
  - **`query.rs::the_source_is_the_checked_rows_own_sha256`** (F2): a catalog candidate's `source.sha256` is the `CheckedRow`'s, and a `Source` cannot be built for `judge_document` from anything else in `query.rs` (a `CheckedRow` is constructed only by QR4's checks).
- **CT6, a match reads only the index** (A IX8, B CT6, C CM1; M5's exit criterion). All three are required: a compile-fail doc test (`match_catalog` cannot be written against `Blobs`); the counting file-system seam records zero opens of a blob path across a catalog run on the corpus catalog; with `blobs/` renamed away after ingest, every CT1 query gives a byte-identical document and the same exit code, and `verify` reports each blob `missing`.
- **CT7, determinism** (B CT7, A OR1, OR4). `--jobs` 1, 2 and 16, and dedup on and off, give identical bytes. **`match_cli.rs::jobs_do_not_change_a_file_runs_bytes`** (review, the merge's open question 3): a file run over the committed candidates and a directory of the corpus, with `--jobs` 1, 2 and 16, gives identical documents, tables and exit codes, an unreadable file among them reported in argument order. Two catalogs ingested in permuted orders give equal digests and differ only in labels. Every command gives identical output under `PRAGMA reverse_unordered_selects=ON`. The same ingest sequence on Linux, macOS and Windows gives identical `list --format json` and match documents, compared by CI's determinism job.
- **CT8, staleness and reindex** (B CT8, A T-rx6, C reindex tests).
  - One row's `extractor_version`, then its `signature_version`, edited down by one and up by one: each time a match exits 3 with the count, and `reindex` restores it; a signature-only rise is rebuilt with zero blob reads (`CountingFs`).
  - Every row at version 5: a match exits 3; `reindex` killed after its second batch leaves it refused; a rerun finishes; the match then equals a fresh catalog's and `verify --deep` is clean.
  - A quarantined entry the new gate accepts becomes searched; an indexed one it refuses exits 3, "no longer searched" (RX6); a damaged blob fails `reindex` with exit 1 and stays stale (RX5).
  - Old rows beside current ones do not make a catalog stale, and `gc --versions` removes them only where current ones exist (RX8). A `PROPERTIES_VERSION` mismatch lists as `stale (properties)` and does not refuse a match. A newer schema is refused by every command.
- **CT9, one snapshot** (B CT9, A DL4, C ST5). A hook replaces a part between level 1 and loading: the document equals the before state or the after state, never a mix. A reader open across a `replace` sees the old entry with its old tokens.
- **CT10, hostile parts** (B CT10, C 10.5). The `h_*` parts ingested, every corpus socket queried under 30 s each (M4's hostile bound); `h_bracket_stubs`'s 80.8 MB document loads; a catalog holding only `h_hole_grid` returns within M4's bounds. A 10,000-entry catalog (the bracket family, each copy with one coordinate perturbed) is ingested, listed, verified and searched each within a stated bound (30 s for the run).
- **CT11, scale** (B CT11; weekly CI or manual). A 10⁴ scaled catalog (about 4 s to build), and 10⁵ manually: every corpus socket with level 1 and with `--exhaustive`, compared by digest; timings reported, not gated, but for a ceiling against an order-of-magnitude regression (a query under 10 s at 16 threads on 10⁴). It also measures M5-4's cost at 10⁵ on s02's block and a seats-only socket (R2), and plain `verify`'s cost with M5-40's checks. **The contingency's trigger** (review; M5-43): if any one query at 10⁵ takes over 60 s of wall time on 16 threads, PR 4 records it and takes M5-43; its test is then `match_cli.rs::counted_drops_are_restored_by_exhaustive` (a full run counts the drops on stderr and in the header and lists none; `--exhaustive` lists each as M5-4 does).
- **CT12, consistency** (A VF, B CT12, C verify tests). `verify` and `--deep` pass on every test catalog. Plain `verify` fails on a hand-edited `posting`, token, `tokens_sha256`, `doc_sha256`, `part_digest` or properties row, a features or tokens row copied to another content's key, an appended blob and a stray temporary file; only `--deep` fails on a document edited to add a hole with its preimage hash and part digest recomputed. Every CHECK, trigger and partial unique index refuses its bad value: a non-hex or 63-character or upper-case SHA, tokens for a refused row, an update of `features`, a second current revision, live content or live id under folding, a `pruned_generation` on a current revision. The token and properties goldens fail while their versions stand still (RX11). Named for the review:
  - **`store.rs::each_preimage_field_is_bound`** (F2): for each of `features`, `tokens` and `properties`, changing any one key or preimage column (content SHA-256, each version, status, part digest, truncation) through a raw connection, the hash kept, fails the load check and plain `verify` (`damaged`);
  - **`store.rs::a_features_row_copied_to_another_key_fails_its_hash`** (F2, the review's CT12 case): X's `features` row copied under Y's SHA-256 (Y's own removed) fails QR4 and plain `verify`; **`store.rs::a_tokens_row_copied_to_another_key_fails_its_hash`** the same for tokens;
  - **`verify.rs::plain_verify_checks_preimages_digests_and_postings`** (F1, F2, F7): one catalog with a moved row, a `part_digest` forged with its preimage hash recomputed, and a posting changed; plain `verify` reports all three, exit 3, and opens no blob beyond IN1's re-hash;
  - **`verify.rs::plain_verify_recomputes_postings`** (F7): a posting's diameter changed by one ulp, a posting deleted, and a posting added for a stack that has none: each is a `mismatch`.
- **CT13, crashes and faults** (A T-fault, T-full, T-crash, T-st2; C's crash test).
  - Each operation (an ingest batch of 3 files with one duplicate and one refused, a replace, a delete, a prune, a gc, a reindex batch) is run once to count its file calls N; then for each k in 0..N and each fault (`ENOSPC`, `EIO`, `ENOENT`, or `abort()` in a re-executed child) it runs with the fault at k and is reopened with `StdFs`. IN1 to IN9 hold, the database effect is all or nothing, and a rerun completes it. On Linux, macOS and Windows.
  - Disk full: a blob write fails with ENOSPC and nothing is committed; `max_db_pages` gives `SQLITE_FULL`, the batch rolls back, and `gc` then clears the orphans without a database write. The source swapped after the read: blob and document come from the bytes read (ST2).
  - Nightly on Unix: a child ingesting in a loop killed at a pseudo-random time, 300 rounds, IN1 to IN9 after each; the row-first control must fail.
  - Named for the review (F6): kill -9 cannot show a lost directory entry, since a killed process loses no page cache, so the order is checked through the `Fs` seam. **`fault.rs::every_named_blob_has_its_directory_synced_before_commit`:** `CountingFs` records every rename, reuse and directory sync with the `point` hook at `COMMIT`; for ingest batches with fresh, reused and ST6-rewritten blobs, every blob a commit names has a sync of its fan-out directory after its rename or reuse check and before that commit. **`fault.rs::a_reused_orphan_has_its_directory_synced`:** an ingest is stopped by `FaultFs` after its rename and before its directory sync; a second ingest of the same bytes reuses the orphan, and its commit is preceded by that directory's sync. The unsynced-reuse control, with the sync limited to renamed blobs, must fail the first test.
  - **`fault.rs::prune_is_all_or_nothing`** (F4(b)): a prune with a fault at each file call leaves every kept revision kept, or every one pruned, IN1 to IN9 holding, and a rerun completes it.
- **CT14, concurrency** (A T-race, T-conc, C writers_wait).
  - Nightly on Unix: for 20 s, four processes ingest, replace and delete from a pool of 40 contents while one runs gc and `prune --revisions` in turn; every 100 ms a reader checks that each `blob` row's file exists and hashes, re-checking in a fresh snapshot (CC4); the unlink-after-commit control must fail.
  - Eight processes ingest overlapping directories at once: every content ends live exactly once, IN1 to IN9 hold, no command fails with `SQLITE_BUSY`. A ninth runs a catalog match in a loop; each document equals an explicit `fitment match` over the files live at its reported generation, replayed from `op`. A writer that cannot get the lock within a test-sized timeout exits 1, "busy".
- **CT15, paths and hostile names** (A T-ph, T-ph1, C hostile_names_are_escaped, sql_never_holds_user_text). Stored, listed and escaped, with no raw control or bidirectional character on stdout or stderr of `ingest`, `list`, `get`, `verify` or a catalog match, and the exact strings in JSON: file names with ESC, a newline, U+202E, spaces, 255 bytes and non-UTF-8 bytes (Linux); a PRODUCT.name with ESC and U+202E. A FIFO in a directory and a file that vanishes between listing and open are reported and skipped; a sparse 129 MiB file is refused with nothing stored; an empty file is quarantined. A symlinked `blobs/ab` is refused by every command; a `revision.sha256` of `../../x` written through a raw connection is refused by `verify` and `gc` without touching the file system. Ids and references with `'`, `%`, `_` and `;` find exactly their entry or nothing.
- **CT16, the CLI** (C 10.2 and 10.3, A T-crud).
  - `init` twice: 0, 0; on a non-empty directory: 2; `list` on a plain directory: 2.
  - `ingest` of C's prototype set (`m_bracket.stp`, `copy of m_bracket.stp`, `a b.stp`, `a_b.stp`, a junk file, `r04_assembly.stp`): `added`, `duplicate`, `conflict`, `quarantined`, exit 2; again: `unchanged`, exit 2; without `a b.stp`: exit 3; `--id X m_bracket.stp`: 2. A clean directory twice: 0, and `list` byte-identical.
  - `get --format step` without `-o`: 2; with: the input's bytes; inside DIR: 2. A tombstone shows its history. `replace` with `m_bracket_thick.stp`: r2; with junk: 3, `get` still r2; with another entry's bytes: 2. `delete a nosuch`: 2, `a` still listed; `delete a`: 0, the blob gone; `delete sha256:…`: 2. An unknown id: 2 with suggestions; a 12-hex prefix found; an 8-hex prefix: 2.
  - Every match exit of section 7.12 (`--catalog` with files 2; stale 3; all quarantined 3; blocked 3 in a full run and 5 in a named run; a corrupt row 1 with the rest judged; a newer schema 3); CM8's stderr line and header; at most 10 rejects then "… N more", and `--rejects all`; a `fitment.catalog` golden for the unit set, byte-identical on a rebuilt catalog; `write_atomically` leaving the old file intact when the rename fails.
  - Named for the review, `catalog_cli.rs`:
    - **`get_step_refuses_a_damaged_blob`** (F4(a)): a blob made writable and one byte changed; `get --format step -o x.stp` exits 1 with "damaged" and `fitment catalog verify`, and `x.stp`, present before, is byte-identical after; a blob truncated by one byte gives the same;
    - **`strict_revision_references`** (F4(b)): `BRKT@R1`, `BRKT@r01`, `BRKT@r0`, `BRKT@1`, `BRKT@r` and `BRKT@r1x` exit 2; after a replace, `replace BRKT@r1`, `delete BRKT@r1`, `reindex BRKT@r1` and `match --id BRKT@r1` exit 2 ("BRKT is at r2, not r1") and change nothing, while `BRKT@r2` works for each; `get BRKT@r9` exits 2;
    - **`replace_keeps_the_old_revisions_bytes`** (F4(b)): ingest BRKT r1 and match, keeping the document; replace with other bytes; `get BRKT@r1 --format step` writes bytes whose SHA-256 is the old document's `source.sha256`, and stderr names r1 and that SHA-256; `get BRKT@r1 --format features` exits 2; a full match does not list r1; `verify` is clean; `prune` without `--revisions` exits 2; `prune --revisions` prints r1, unlinks its blob, and `get BRKT@r1 --format step` then exits 2 while `get BRKT@r1` still shows its history; a second prune prunes nothing; deleting an entry with a kept revision unlinks both blobs.
  - Named for the review, `match_cli.rs`:
    - **`labels_carry_the_revision`** (F4(b)): every catalog candidate's one label is `catalog:<id>@r<n>`; after a replace the new run's label is `@r2` and the digest is unchanged where the bytes are;
    - **`one_catalog_per_run`** (M5-44): `--catalog A --catalog B` exits 2;
    - **`a_features_row_moved_to_another_key_is_not_judged`** (F2): X's features row placed under Y's key (through test-support): the run names Y as corrupt on stderr, does not judge it, judges the rest, and exits 1;
    - **`a_part_digest_column_that_disagrees_is_corruption`** (F1): Y's `part_digest` written into bad part X's row, X's hash left stale: X fails QR4's step 2, is named and not judged, exit 1;
    - **`a_part_digest_forged_with_its_hash_is_still_caught`** (F1, the review's scenario): the same with X's preimage hash recomputed, as a bug at ingest would write it; QR4's step 5 catches it, X is not judged, Y is judged as itself, nothing is reported `InterfaceMatch` under X's SHA-256, and the run exits 1, never 0 or 4;
    - **`dedup_groups_by_the_recomputed_digest`** (F1): two byte-different files with equal parts and a third with another part are judged as two groups (a test-support counter of representatives reads 2). With QR4's step 5 switched off (test-support only), X's column forged to Y's digest with its hash recomputed still leaves X in its own group and judged as itself: the grouping never reads the column, and step 5 is a second, independent guard;
    - **`a_lost_posting_hides_a_part_until_verify`** (F7, the remaining risk recorded): a passing part's only posting deleted; the full run omits it and exits 5; `--exhaustive` passes it; plain `verify` reports the `mismatch` and exits 3;
    - **`a_corrupt_posting_never_passes_a_part`** (F7): a posting's diameter rewritten into a socket's window for a part whose hole is the wrong size; level 1 admits it, and it is listed as a `Reject` (at stage `shortlist`, with its evidence), never a pass.
- **CT17, identity** (C 10.1). A property test over arbitrary strings: ID3's mapping gives a valid id or none, and valid ids map to themselves. Ids are unique without case. Ingest, replace, delete and ingest again give r1, r2, tombstone r2, r3, under a new internal key.
- **CT18, properties** (C 10.4; PR 5). Each row of `PropertyMap::shipped()` gives its field and meaning on its cited record (required in CI, skipped with a note without fixtures): `nist_ctc_05` `#1095` and `nist_stc_09` `#7270` read `unassigned`. `Density` and the drawing note (`#18079`, `#8518`) map to nothing. `nist_stc_06`'s 440 values are captured in full; a generated file with 2,000 is `truncated`. Properties on a shape aspect or a parent assembly are not captured. Every corpus match document is byte-identical with and without captured properties, and a PRODUCT.name `M6x20` changes nothing (PR5, PR6). `get` prints them escaped.
- **CT19, coverage** (B CT13). Every rule id here joins `RULES` in PR 1, deferred to the PR that builds it; `design_rules` reads `docs/research/m5-catalog.md`; each rule has a test or a reason by PR 7. The review's new ids, ST11, ID9, CR11 and IN9, join with the rest.
- **CT20, without the feature** (A T-cli, C CC8). `cargo clippy -p fitment --no-default-features` builds; `fitment catalog` exits 2; `match --catalog` exits 2 with CL8's message.

## 10. PR plan

Each PR comes with its tests and two adversarial reviews, as in M4, and no PR depends on a later one. Every M5 rule id is in `RULES` from PR 1, deferred to the PR that builds it (CT19). `SIGNATURE_VERSION` rises to 2 in PR 1, `PROPERTIES_VERSION` starts at 1 in PR 5, and the catalog schema is version 1 from PR 2; it stays unpublished until PR 7, so PR 5 adds the `properties` table to schema 1 rather than migrating. Each item leaves room for an "As built" note, as M4's section 22 did.

1. **Core for a stored catalog, and the atomic write.**
   - **Contents:** section 5 but `properties/`: `judge_document` and `judge_step` over the private `judge_document_with`, `judge_step` still passing its bytes to the fit request (review, F5), `NotCurrent`, `Source::new`, `to_compact_json`, `part_digest`; `IndexTokens` and `PairToken` with `Serialize`, `Deserialize`, `to_json`, `from_json`, `postings()`; `IndexSource`, `level_one` (IX4 to IX8, IX5's per-part lever and buckets with `seat_bucket`'s `max` (review, IX5 note), and the SG5 skip M4's exit PR put in `pairs_hold` (review, F3, inherited)), `ShortlistIndex` as a source, `Matcher::admitted`; B4's compact pair tokens and `SIGNATURE_VERSION` 2, with the match goldens re-blessed; `write_atomically` moved to `cmd/mod.rs` with its two fsyncs (CL7, M5-16); every M5 rule id in `RULES`, deferred.
   - **Tests:** CT3 in core (containment per pair, independence, the proptest with SG5 groups, `one_stack_for_a_group::level_one_keeps_a_part_one_stack_serves_a_group_with` through `level_one`, `a_seat_bucket_is_never_below_the_lever`); CT4 in memory; CT5 with `judge_step_hands_its_bytes_to_the_fit_request`; the token goldens; `judge_document`'s refusal; `write_atomically`'s failure path; `cargo semver-checks`.
   - **Done when** every match golden differs only in `signature_version`, and CT3 and CT5 pass on the unit set, the committed candidates and the corpus.
2. **The store and ingest.**
   - **Contents:** the `fitment-catalog` crate (`Fs`, `StdFs`, `Sha256Hex`, `preimage.rs`, the layout and marker, schema 1 with DB5's mechanism and `revision.pruned_generation`, the generation and `op`); ST1 to ST4 (ST3's directory sync for every blob a commit names, reused ones included: review, F6), ST6 to ST11 (ST11's re-hash in `get --format step`: review, F4(a)), DB1 to DB9, GT1 to GT9 (GT2's preimage hashes: review, F1, F2), ID1 to ID9 (ID9's `ID@rN`: review, F4(b)), PH1 to PH8, SZ1 to SZ5, CL1 to CL6, CL8 to CL10, CR8, CC1, CC2, CC5, CC6; `fitment catalog init`, `ingest`, `list` and `get` (all four formats, `get` printing what it wrote on stderr) behind the default `catalog` feature; `deny.toml`'s rule that only `fitment-catalog` depends on rusqlite, its `clippy.toml`, and the licence review (rusqlite and libsqlite3-sys MIT, SQLite public domain).
   - **Tests:** CT13 for ingest, with `every_named_blob_has_its_directory_synced_before_commit` and `a_reused_orphan_has_its_directory_synced`; CT15; CT16's `init`, `ingest`, `list`, `get`, with `get_step_refuses_a_damaged_blob` and the current-revision half of `strict_revision_references`; CT12's `each_preimage_field_is_bound` and the two copied-row tests at the load check; CT17; CT20; the schema's unit tests; CT7's ordering pragma for these commands.
   - **Done when** the unit set, the committed candidates and the fixtures ingest with the manifest's pinned gate results, and `get --format features` equals `fitment features` for each.
3. **Delete, replace, prune, gc and verify.**
   - **Contents:** CR1 to CR7, CR10, CR11 (replace keeping the old blob, `prune --revisions`: review, F4(b)); IN1 to IN9; ST5's gc and its inline use; VF1 to VF5, plain `verify` checking every preimage hash, part-digest column and posting (review, F1, F2, F7); CC3, CC4; `fitment catalog replace`, `delete`, `prune`, `gc`, `verify`.
   - **Tests:** CT12, with `plain_verify_checks_preimages_digests_and_postings` and `plain_verify_recomputes_postings`; CT13 for every operation, with `prune_is_all_or_nothing`; CT14 without the match loop, prune in the gc process (the soaks nightly); CT16's `replace`, `delete`, `prune` and references, with `replace_keeps_the_old_revisions_bytes` and the rest of `strict_revision_references`. Plain `verify`'s cost at 10⁴ is recorded.
   - **Done when** the race and crash soaks and their controls behave as A measured: nothing missing with the rules, failures without them.
4. **The catalog match, and M5's exit test.**
   - **Contents:** `Snapshot`, `SqlSource`, `match_catalog`; the stale gate (RX1 to RX3, with test-support constructors for old rows); QR1 to QR13 (QR4's preimage check and recomputed digest, `CheckedRow`, QR6's grouping by it, QR7's source from it: review, F1, F2), CM1 to CM11 (labels `catalog:<id>@r<n>`, one `--catalog`), IX9, IX10; `fitment match --catalog` with `--id`, `--jobs`, `--rejects` and the documented `--exhaustive`, its stderr line and table header; `--jobs` in file runs (M5-30).
   - **Tests:** CT1 with the clevis case (review, F3); CT2 (`exhaustive_equals_the_file_run`); CT3 against SQL, with `sql_source_keeps_a_part_one_stack_serves_a_group_with`; CT4 through SQL, CT6, CT7 with `jobs_do_not_change_a_file_runs_bytes`; CT8's refusals, CT9, CT10, CT14's match loop; CT16's match exits and the review's `match_cli.rs` tests (moved row, forged digest both ways, recomputed grouping, lost and corrupt postings, labels, one catalog); `the_source_is_the_checked_rows_own_sha256`. A first CT11 run on 10⁴ and 10⁵ measures M5-4's cost on unselective sockets (R2), the recomputed digest's cost per survivor, and the merged stale check (QR1), recorded in the "As built" note. **If any one query at 10⁵ exceeds 60 s on 16 threads,** M5-43 is taken in this PR, with `counted_drops_are_restored_by_exhaustive`, and the note says so.
   - **Done when** CT1 and CT6 pass under both policies on the corpus with zero false positives through the catalog, and CT1's clevis case passes.
5. **Properties, D10.**
   - **Contents:** core's `properties/` (`read`, `PROPERTIES_VERSION` 1, PR2's bounds, `PropertyMap::shipped()` with its one row) behind stepq's wall, extended to `properties/step.rs`; the `properties` table, `stated_sha256` over its preimage (review, M5-40); capture at ingest from the one buffer; `get` showing the set and its meanings (PR7); a properties golden.
   - **Tests:** CT18 and CT12's properties cases.
   - **Done when** PLAN's D10 row is recorded as decided with section 4.13's evidence, and every corpus match document is unchanged.
6. **Re-indexing.**
   - **Contents:** RX4 to RX11; `fitment catalog reindex` with both paths and the properties re-capture, every row written with its preimage hash and every blob read through ST11; `gc --versions` (RX8).
   - **Tests:** CT8 in full; CT13 for reindex batches.
   - **Done when** catalogs built at the previous `SIGNATURE_VERSION` and at a previous `EXTRACTOR_VERSION` (test-support) reindex, resume after a kill, and then match exactly as fresh catalogs do.
7. **Scale, the exit and the release.**
   - **Contents:** `tools/scale-step.py` (section 8.3's unit edit, no kernel) and CT11 weekly; the reviews' hardening; CI's no-default-features leg, the nightly soaks and the three-OS comparison of catalog outputs; the registry gate on for M5's ids with no M5 deferral left (CT19); `tools/check-core.sh`'s package checks extended to `fitment-catalog` (no STEP file in any package, M5-14; a LICENSE in each); section 12's documents, the README (a catalog walkthrough, VF5's note on copied catalogs, CC5's on synced folders, R20's advice to run `verify` regularly and `--exhaustive` when a miss is suspected, and `prune --revisions`) and CHANGELOG; version 0.2.0 for every crate, `fitment-catalog` published as internal and unstable.
   - **Done when** PLAN's exit holds from the CLI: ingest, list, get, replace and delete work, and a match reads only the index (CT6). The publish itself is the user's.

## 11. Risks

- R1. **Many parts that really fit are expensive** (B). Every survivor that passes level 2 is aligned and verified, at 10 to 200 ms each. A catalog of 10⁴ distinct brackets that all fit s01's socket costs about 14 minutes of CPU, about a minute at 16 threads (estimated). Dedup helps only with identical parts, and a ranked, limited query needs a ranking that v1 lacks (M5-36). This is M5's main cost risk.
- R2. **M5-4 judges level-2 drops at catalog scale** (merge). B's timings ended a candidate at a level-2 drop. Under M5-4 every survivor level 2 drops is aligned and verified for its evidence, as M4-17 does for files.
  - **The exposure:** s02's block at 10⁵ has 8,844 survivors of which 36 are kept; a seats-only socket has 98,766 of which 202.
  - **The cost:** a few ms per drop by M4-17's estimate, so tens of seconds of CPU for the block, and a document of the size of a file run over most of the catalog for the seats-only socket. Neither is measured.
  - **The plan** (review, the merge's open question 1, answered): M5-4 is kept, and PR 4 measures both at 10⁵ (CT11). The lasting remedy is a more selective level 1 (M5-35's plane tokens, a `SIGNATURE_VERSION` rise). **If one query exceeds 60 s on 16 threads,** M5-43's contingency is taken: level-2 drops counted rather than listed in a full run, `--exhaustive` restoring them.
- R3. **Large documents** (B, C R4). 9,219 candidates made 111 MB. M4 already noted 8 MB for 100 candidates on a large socket. M6 will want pagination. The table prints the passes and at most 10 rejects (M5-31).
- R4. **Storage for real parts** (A R1, B). Fixture-like parts' documents are about 13 times larger than generated ones, so 10⁵ parts take an estimated 15 to 20 GB without compression. `VACUUM` needs as much free space again. The `encoding` column is ready for compression (M5-34).
- R5. **fsync cost on macOS** (A R2). 3.3 ms a blob and 3.74 ms a commit: syncing dominates ingest (about 33 s per 10,000 files against 8 s of extraction). Accepted for durability (M5-15); batches bound the commit cost.
- R6. **Upgrades block searches until reindex ends** (B, C R6; M5-3). For 10⁵ real parts, about a minute at 16 threads, or 13 minutes on one core (estimated). Reindex can be resumed, and M6 runs it as a job.
- R7. **Windows** (A R7, C A23).
  - **Directories:** std cannot sync a directory, so a power loss can lose a rename the commit then names, renamed or reused (ST3's F6 rule cannot apply). `verify` reports it as IN1, and the next ingest of that file restores it.
  - **Open files:** they cannot be unlinked, so gc skips sharing violations and retries on the next run.
  - **Antivirus scanners:** they can fail a rename transiently, so it is retried 5 times with backoff.
- R8. **Trust in the catalog directory** (B, C R3). Stored documents decide verdicts without the STEP file being read again. Tampering with consistent hashes is caught by `verify --deep`, not by the query (VF5).
- R9. **An extractor that changed without a version rise** (A S2; M5-14). With the canary rejected, rows written by such a build look current. The goldens and the version discipline must catch the change before release, and `verify --deep` finds the effect on a user's catalog.
- R10. **WAL growth** under a long match with heavy concurrent ingest (A R3). It is bounded by what is written during the match and reclaimed when the match ends (CC3).
- R11. **Synced and network folders** (A R6, C R5). These can corrupt a WAL database. They are documented, and WAL mode is checked at open, but neither can be detected in general.
- R12. **rusqlite and bundled SQLite** (A R4, R5). rusqlite declares no MSRV: 0.40.2 builds on 1.85 today, the lock file keeps it, and CI's MSRV job catches a break. The bundled SQLite is C, so `cargo install fitment` needs a C compiler. `hashbrown` and `foldhash` come with rusqlite; `tools/core-deps.py` keeps them out of core.
- R13. **Level 1 is weak on single-line and seats-only sockets** (B). At 10⁵ it admits 8,844 parts for s02's block and 98,766 for a seats-only socket. Under M5-4 this matters more (R2).
- R14. **D10 cannot grow without user samples** (C R2, R8). The fixtures hold no positive material and no thread linked to a hole. Each row waits for a real file (PR4). The first candidate is AP242's `MATERIAL_DESIGNATION` practice. Once rows exist, CAD default materials could compare as equal, so a row must bring its exporter's default values from samples.
- R15. **Default ids surprise users** (C R7). Every mapped id is printed on its line, conflicts never overwrite, and `--id` names a single file exactly.
- R16. **The obsolete part itself left in the catalog** (C A25, R9). It would be found as a perfect match. Catalog membership means available (PLAN), and comparing a part with the original is guessing, so the documentation says to delete withdrawn parts.
- R17. **Measured on scaled generated parts** (B). A real catalog's distribution, with standard hole sizes repeated across thousands of parts, is unknown until real catalogs arrive (D8, D9, M5-37).
- R18. **Two kinds of runs** (B). A full run lists only level-1 survivors, so a level-1 bug hides parts silently. `--exhaustive`, CT1, CT2 and CT11 are the only guards that see it. The review found one such bug (F3, a group served by one stack, fixed in M4's exit PR) that no corpus socket exercised, so CT1 gained a constructed case and CT3's proptest draws groups. **Review:** `--exhaustive` is documented (M5-42), so a user who suspects a miss can run it; nothing runs it on a user's catalog unasked.
- R20. **Postings carry no hash** (review, F7). A posting altered or lost on disk is not seen by the run it hides a part from: that run exits 5 where 0 or 4 was due, until `verify` reports the `mismatch`. A posting can never pass a part (IX9). Plain `verify` recomputes every posting (VF1), and the README advises running it regularly, after copying a catalog or a crash, and `--exhaustive` when a miss is suspected. Not adopted: a warning on matches when no consistent `verify` has been recorded since some generation (the review's option), a hash per posting row, which cannot show a lost row, and checking every posting on every run, which would read every indexed part's tokens.
- R21. **Kept revisions take space** (review, F4(b); M5-41). A replace keeps the old revision's STEP bytes until `prune --revisions` or a delete. A catalog replaced nightly grows by every old revision. `list`'s stderr line counts them and names `prune`.
- R19. **M7** (A IX7, B). The fit verifier needs each passing candidate's bytes, read from the blob store: the one sanctioned read outside the index, limited to candidates whose interface passed. Dedup must stop at the interface (QR6).

## 12. Changes to PLAN.md and ARCHITECTURE.md

At PR 7, as built:

- **PLAN.md:** M5 done, with the exit numbers (CT1's false positives through the catalog, CT6, CT11's timings); Components and How it works (two stores, a query reads the index only); D5's notes as built; D10 decided (M5-5) with section 4.13's evidence in brief; D8 and D9 still open (M5-37); exit code 3 also "the search could not run"; "Metadata from STEP only" gains `nist_ctc_04`'s tapped M12 hole modelled as a plain Ø10.2 one; Resume here.
- **ARCHITECTURE.md:** the crate map with `fitment-catalog` and rusqlite; a section "Storing a part and matching from a catalog" (section 3.1's flow and modules, and the store's two rules); Formats and versions (`fitment.catalog` v1, `SIGNATURE_VERSION` 2, `PROPERTIES_VERSION`, the schema version); level 1 at each part's own lever; Soundness (stale catalogs refused, quarantine never searched, a catalog run equal to a file run restricted, row hashes naming their rows, dedup by recomputed digest, a corrupt posting able to hide but never pass a part until `verify`, a copied index trusted only after `verify --deep`); kept revisions and `prune --revisions`; Determinism (`ORDER BY`, the generation instead of time); Hostile input (names, paths, sizes); the test table's CT rows.
- **m4-match.md:** M4-17's last sentence ("In a catalog query (M5) the drop ends the candidate") amended by M5-4; section 6.2's largest lever amended by IX5; section 12's planned `CandidateSource::Catalog { id }` withdrawn (CM5); MF3's one-at-a-time amended by `--jobs` (M5-30). F3's fix to `pairs_hold` is already recorded by M4's exit PR.

## 13. Attacks and their fixes

Each designer attacked their own design. How could the catalog pass a part that does not fit, hide one that does, make a catalog run differ from a file run, or lose the user's data? Their findings are merged here, each once, with where it came from: A's S-numbers, B's and C's A-numbers. Each changed or confirmed a rule above. The soundness review then attacked the merged design; its findings are in section 13.3, and rows of 13.1 and 13.2 whose fix it changed say so.

### 13.1 The designers' findings

| # | Attack | What would go wrong | Fix | From |
| --- | --- | --- | --- | --- |
| 1 | Judge a document written by an older or newer extractor | Its part reflects rules since fixed: an old reading may state as known what new rules read as unknown, and verification trusts known values. A false positive | RX2 and RX3 refuse the run; QR4 requires `is_current()`; `judge_document` refuses a non-current document whatever the gate concluded | A S1, B A5, C A1, A2 |
| 2 | Evidence that lies about its reader | `judge_part` writes `read.extractor_version` from the build's constant, so a stale document judged anyway would claim the current extractor and the false positive would be invisible | `judge_document` (QR7) refuses, so the field is always true | A S1, B A6 |
| 3 | Stale tokens or postings | A new `BIN_MM` or pair rule leaves no posting where level 1 now probes: a good part hidden. A token can never pass a part | RX2 counts signature staleness too; RX4's tokens-only reindex; `tokens_sha256` checked (IX9) | B A4, A S7 |
| 4 | Two builds that both say `EXTRACTOR_VERSION` 6 extract differently | Rows look current but were written by other rules | The goldens and the version discipline (RX11); `verify --deep` proves rows against blobs. A's canary was rejected (M5-14); risk R9 | A S2 |
| 5 | A stored document corrupted (bit rot, a bad sector, a hand edit) so a diameter changes and still reads back | A candidate verified against features it does not have | `doc_sha256` on every load, then `from_json`'s structural checks (QR4); `verify` and `--deep` | A S3, C A7 |
| 6 | A tampered catalog with consistent hashes | A stored document stating a hole the file lacks passes a candidate | Trusted only after `verify --deep` (VF5), documented; risk R8 | C A7, B section 9 |
| 7 | Features that belong to other bytes: a replace updates the SHA-256 but not the features, or a bug attaches one content's document to another's key | Bad file X judged by good features Y and reported under X | One buffer for hash, blob and document (ST2); derived rows keyed by the content's SHA-256 (DB7); replace is a delete and an insert, never an update (CR3, insert-only triggers); `verify --deep`; CT5 against `fitment features`. The review found the key itself unchecked on the match path (13.3, R-F1, R-F2): the row hash now names the key (GT2) | A S4, B A7 |
| 8 | A deleted part still offered | Membership is availability (PLAN), so a withdrawn part would be recommended | CR1 removes the derived rows in the delete's transaction; postings exist only for content a current revision of a live entry names (IN9); durable commits stop power loss undoing a delete (ST9) | A S5 |
| 9 | Replace leaves both revisions live, or neither | An outdated model matched, or a part vanishes | `revision_current`, and CR3 in one transaction | A S6 |
| 10 | A quarantined document reaches matching | A part the gate refused is judged | GT4's trigger; QR4; `judge_document` gives a refused document `Reject` anyway | A S8 |
| 11 | gc deletes a blob a concurrent ingest just committed | A row without a file; matching is safe (QR11), but reindex and M7's fit break | ST5 and ST6; CT14's race and its control | A S9 |
| 12 | A crash between write and rename, or mid-file | A torn blob under its final name | ST3: sync before rename, names from content; IN2, IN6; CT13 | A S10 |
| 13 | A row committed before its blob is durable, then power is lost | A row without a blob after reboot | ST3 orders the sync before the commit; ST9's `F_FULLFSYNC` on Apple; R7 on Windows. The review found a reused blob's directory unsynced (13.3, R-F6) | A S11, C A5 |
| 14 | Read-then-write decisions on a stale snapshot | Measured: 1,543 of 1,600 `DEFERRED` transactions failed; under retries, a check made on old data | DB2: `BEGIN IMMEDIATE`, every check inside the transaction | A S12 |
| 15 | `verify` reports a blob missing because a delete committed after its snapshot | A false alarm that could prompt a destructive repair | CC4 re-checks in a fresh snapshot; `verify` never repairs (VF4) | A S13 |
| 16 | A hand-edited `sha256` of `../../…` | gc or verify reads or unlinks outside the store | CHECKs and `Sha256Hex` (PH1, DB6); CT15 | A S14 |
| 17 | A symlinked fan-out directory | gc unlinks someone else's files | PH2 refuses symbolic links in the store | A S15 |
| 18 | A missing `ORDER BY` | Output that depends on SQLite's plan, different between versions or after `ANALYZE` | DB9 with `reverse_unordered_selects` (CT7) | A S16 |
| 19 | A half-finished reindex | Some entries at the new version, some at the old: a match silently judges part of the catalog | RX3 refuses while any live entry is stale; RX7 resumes | A S17, C A6, B RX4 |
| 20 | An older binary garbage-collects a newer binary's rows, or reindexes them down | A ping-pong between versions, and lost rows | RX8: never write or delete a newer version's rows; gc removes only older ones | A S18 |
| 21 | A verdict cache keyed too narrowly | A cached `Match` survives a rule fix or a policy change | GT9, RX10: nothing cached | A S19, B CS7 |
| 22 | The source file changes during ingest | The blob is one version of the file and the document another | ST2's one buffer; CT13's source swap | A S20, C CI8 |
| 23 | Level 1's lever taken over a subset of the catalog, or before an insert | Level 1 narrower than level 2: a missed candidate | IX5: each part at its own lever, so there is no catalog-wide value to get wrong; IX6 | A S21, B A2 |
| 24 | Threads, or a long run, see different states of the catalog | A part replaced after level 1 judged as its new self, or a set that never existed | QR2: one reader, one read transaction, workers never touch the database; CR7; CT9 | A S22, B A8, C A6 |
| 25 | SQL and core disagree on windows, bins or joins | Hidden parts | IX1, IX2: core's `level_one` is the only implementation, and SQL fetches by an integer range; CT3 | B A1, A R9 |
| 26 | Per-part seat groups are slow (found by measurement: 286 ms for 177 hits) | A slow level 1 | IX5's power-of-two buckets, sound because seat groups only coarsen: 33 ms. The review found the merged text had dropped the `max` with the lever (13.3, R-IX5) | B A3 |
| 27 | Dedup when the fit depends on the file | Two files with equal features can have different bodies (an `other` face is recorded by type) | QR6 applies only while no verifier names a kernel; M7 asks the fit per member | B A9 |
| 28 | Ties in the order | A `Given` source has an empty identity under MF5, so equal verdicts would keep the threads' finishing order | CM7: the file's source and its SHA-256, unique per catalog | B A10 |
| 29 | A blocked socket | Only skipping it wastes work. Exit 5, "nothing fits", misleads a user into buying a part, though nothing could ever pass | IX8 blocks the whole query; CM9 and QR1 make it exit 3 naming the requirement and `fitment review` | B A11, C A11 |
| 30 | Level 2 or level 1 built under another policy | The prototype first built socket signatures with the default policy; under `--tolerance-mm` it would drop parts verification passes | QR3: the matcher's own signatures; the catalog builds none | B A12 |
| 31 | The catalog run loses KH9's runtime check | A level-2 bug hides parts silently | M5-4 judges every survivor with M4-17's semantics, so KH9 runs on each; `--exhaustive` (QR10, documented after the review) and CT2 check level 1 | B A13 |
| 32 | A pair token's first stack holds the window only through a neighbour (SL2) | Level 1 could drop what level 2 keeps | Analysed sound by B: the token for that stack and the other line's stack exists, and its distance lies within the reach; CT3's per-pair test covers it | B A14 |
| 33 | Quarantine hides a part forever after a gate fix | A good part never searched | RX4 re-gates quarantined files on every extractor rise; CR10 | B A15 |
| 34 | A row that does not parse is skipped | Hidden parts that depend on corruption | QR4: not judged, named, exit 1 (M5-32); `verify` finds the row. A posting is not such a row: the review corrected M5-32's claim (13.3, R-F7) | B A16 |
| 35 | A huge part or file | Memory exhausted at ingest; every query slowed | SZ1 reads at most the cap; documents loaded only when admitted; M4's bounds per candidate (QR9); CT10 | B A17, C A24 |
| 36 | Floats through SQLite | NaN becomes NULL, −0.0 may come back as +0.0 | Stored diameters are finite and positive (the gate, `geom::finite`); exact comparison in core (IX3); CT4 | B A18 |
| 37 | An integer id wraps | SQLite's `i64` against core's `u64` | `u64::try_from`; a failure is a catalog error (IX10) | B A19 |
| 38 | `replace` with a broken export | The part silently leaves every search | CR4: refused, the entry unchanged | C A3 |
| 39 | Yesterday's run says `BRKT` passed; today `BRKT` was replaced and the user orders "BRKT" | The id now names other bytes | The document's `source` keeps the SHA-256; revisions never repeat (ID6); `get` shows the history. After the review, the label itself carries the revision, `catalog:BRKT@r1`, and `get BRKT@r1 --format step` still writes the bytes judged (13.3, R-F4b) | C A4 |
| 40 | Every part quarantined, or the wrong DIR | Exit 5, "nothing fits" | QR1: nothing indexed is exit 3; a DIR that is not a catalog is exit 2; CR8's count on every run | C A10 |
| 41 | `delete` with a typo or a hash prefix | Another part deleted | ID7: ids only; CR2: all or nothing on unknown ids | C A12 |
| 42 | Batch ingest of a vendor directory with similar names | Default ids collide; the wrong file gets the clean id | ID4 (found by C's prototype); conflicts printed, exit 2, nothing overwritten | C A13 |
| 43 | The same bytes under two ids | Two lineages of one file | ID5 | C A14 |
| 44 | A file name or PRODUCT.name holding escape sequences or U+202E | The table rewrites itself or reorders a line, hiding a reject | PH6, ID1; CT15 | C A15 |
| 45 | `get --format step` to a terminal | A STEP file's text reaches the terminal raw | PH7 | C A16 |
| 46 | A property states `Material = Steel` in both files, both CAD defaults | "Equal" read as "verified" | Moot in M5 (PR5: no comparison); a future row needs its exporter's defaults from samples (PR4); R14 | C A17 |
| 47 | Material stated twice, or one value cut by the bound | Compared as one value, wrongly equal | PR2 marks a cut set `truncated`; no comparison in M5 | C A18 |
| 48 | An assembly-level material applied to the part | A guess from the parent | PR2: the part's own definition only | C A19 |
| 49 | A catalog id `SHCS-M6x20`, or a note "M6 TAPPED" | Read as a thread | PR6 | C A20 |
| 50 | `match -o DIR/catalog.db`, or `get -o` into DIR | The catalog overwritten by a report | ST10, CL7 | C A21 |
| 51 | A blob edited in place by another program | Its bytes no longer what was judged | ST4 (read-only); `verify` re-hashes; after the review, `get --format step` re-hashes before writing (ST11; 13.3, R-F4a) | C A22 |
| 52 | Windows: read-only blobs cannot be deleted; directories cannot be synced | `delete` fails; ST3 errors on open | ST4 clears the flag before unlinking; ST3 skips the directory sync (R7) | C A23, A R7 |
| 53 | Two catalogs with the same contents and different histories | Different digests for the same judgment | Labels and the generation stay outside the digest (CM6) | C A26 |
| 54 | A catalog in a synced or network folder | WAL is unsafe there: silent corruption | CC5: unsupported, documented; WAL mode checked at open; `verify` finds damage | C A27, A CC5 |

### 13.2 The merge's own findings

Reading the three designs against each other, and against the coordinator's decisions, found these. Each changed a rule above.

| # | Finding | Fix |
| --- | --- | --- |
| 1 | M5-3 reads "stale means any row at another version", but A's rows are insert-only and versioned. After a reindex the old version's rows remain beside the new until `gc --versions`, so the literal reading would refuse forever. The danger is a row a run would *read* at another version, and a run reads only current rows (DB7) | RX2 defines staleness per live entry: no current row where the run needs one. Old rows beside current ones are inert, and CT8 tests both |
| 2 | M5-4 makes every level-2 drop among the survivors an explicit candidate, but every query time B measured ended the drop. On unselective sockets the survivors are mostly drops: 8,808 of 8,844 for s02's block at 10⁵, and 98,564 of 98,766 for a seats-only socket | Stated as M5-4's cost and risk R2. PR 4 measures it at 10⁵; after the review, M5-43 is the contingency if one query exceeds 60 s on 16 threads |
| 3 | Core's level 1 takes `u64` ids. A's postings named content by SHA-256 string, and B's by `part.id`, a per-file row that M5-28's content-keyed rows replace. The merged schema needs an integer content key, and a SQLite rowid can be reused after a delete unless the table is `AUTOINCREMENT` | IX10: level 1's ids are `blob.key`, `AUTOINCREMENT`; postings reference it with a cascade |
| 4 | Under C's `--id` and B's `--audit` (now `--exhaustive`, M5-42), a quarantined entry must be judged from its stored refused document. A's load check (IX3) required `status = accepted` for every candidate | QR4: a refused document is judged only when named or under `--exhaustive`, and gives CR9's `Reject`; in a full run every survivor is accepted by construction |
| 5 | B's CT1 expected the 11 blocked corpus sockets to exit 5. Under M5-13 a full run on a blocked socket exits 3 | CT1 expects 3 for full runs and 5 for named runs |
| 6 | A's exit test compared the catalog run with `fitment match S <the survivors' files>`. Under M5-4 the comparison is exact: the same digest, drops included | CT1 compares digests and entries, both ways |
| 7 | A stored pretty documents byte-exact, so `get --features` was a copy; M5-10 stores compact JSON | GT2: `get --format features` prints `to_json()` of the parsed document; CT5 checks it equals `fitment features` |
| 8 | C's `PROPERTIES_VERSION` made an entry stale and refused a match, though under M5-5 nothing a match reads depends on properties | M5-26: properties staleness shows in `list` and is redone by reindex, and never refuses a match |
| 9 | M5-2's label `catalog:<id>` carries no revision. After a replace, an old document's label names an id whose bytes changed | The `source` SHA-256 identifies the bytes judged; `get` maps (id, SHA-256) to revisions through the history (ID6). Superseded by the review (13.3, R-F4b): the label is `catalog:<id>@r<n>`, and a replace keeps the old bytes (M5-2, M5-41) |
| 10 | C unlinked a deleted blob right after the commit (ST3), and M5-6 asks for A's lock and re-check. C's delete exited 1 when that unlink failed, though the delete had committed and a rerun would fail on an unknown id | CR1: the unlink is the inline gc's under ST5. A failure leaves an orphan, a warning and exit 0 |
| 11 | M3 owns rule prefixes Q and I in the coverage registry | The query's rules are QR, the invariants IN |
| 12 | B's stale check was 17 µs through one index on its one-row-per-part schema. The merged schema needs two counting joins over the live entries | QR1 states the merged check as not measured; PR 4 measures it, and a per-version count kept by the writers is the fallback if it matters |

### 13.3 Found by the review, fixed so

The soundness review (`m5/review/soundness.md`) read the merged design against the `m4/exit` worktree and ran a probe (`review/probe/ws`, `m5_review_probe`). It listed findings at 80/100 confidence or above. Each row is marked as the review found it, then fixed as the coordinator resolved it.

| # | Attack | What would go wrong | Found by the review, fixed so | Tests |
| --- | --- | --- | --- | --- |
| R-F1 | Dedup copies a verdict on an unchecked column (high): a batch-ingest or reindex bug, or a hand edit, writes good part Y's `part_digest` into bad part X's row | X and Y share a group, Y represents it, and X is reported `InterfaceMatch` under its own SHA-256: exit 0 or 4 on a corrupt row, against M5-32. Plain `verify` did not check the column | **Found by the review, fixed so:** QR6 groups by the digest recomputed from each loaded, hash-checked document (QR4 step 5), never by the column; a column that disagrees is corruption, so the row is skipped and named, and the run exits 1. The column is also inside the preimage (GT2), and plain `verify` compares it with the recomputed digest (VF1, IN5). M5-12, M5-40 | `a_part_digest_column_that_disagrees_is_corruption`, `a_part_digest_forged_with_its_hash_is_still_caught`, `dedup_groups_by_the_recomputed_digest`, `plain_verify_checks_preimages_digests_and_postings` |
| R-F2 | A stored document not bound to its content (medium): a misaligned batch, a reindex that zips results in completion order, or a copied row puts file X's document under key Y | Y is judged by X's features and reported as Y; `doc_sha256` covered the document alone, so the hash, `from_json`, `is_current` and plain `verify` all passed. The same for `tokens_sha256` | **Found by the review, fixed so:** each derived row's hash covers a preimage naming the row: content SHA-256, extractor version, status, part digest, then the document; tokens likewise with the signature version; properties with theirs (GT2, DB7, IX9, PR3). QR4's existing check now catches a moved row at no cost; `judge_document` takes the source built from the row's own checked SHA-256 (QR7, `CheckedRow`); plain `verify` checks every preimage (VF1). M5-10, M5-40 | `each_preimage_field_is_bound`, `a_features_row_copied_to_another_key_fails_its_hash`, `a_tokens_row_copied_to_another_key_fails_its_hash`, `a_features_row_moved_to_another_key_is_not_judged`, `the_source_is_the_checked_rows_own_sha256` |
| R-F3 | Level 1 drops a part that level 2 keeps (high): SG5 lets one stack serve a group of coaxial lines, a clevis's two holes, but `pairs_hold` demanded a pair token for each parallel line, and tokens join distinct stacks only | A full catalog run silently omits a part `fitment match S file.stp` passes, and may exit 5. The probe: a Ø10 shaft over [−1, 21] against two coaxial Ø10 lines over [0, 5] and [15, 20] in one group; level 2 kept it, level 1 returned `Ok([])`. No corpus socket exercises it | **Found by the review, fixed so:** already fixed in core by M4's exit PR, recorded here as inherited and done: `ShortlistIndex::pairs_hold` skips lines of the selective line's SG5 group, at a lever no smaller than any indexed part's. PR 1's `level_one` keeps the skip over every `IndexSource`, with groups at each part's bucketed lever (IX7). CT1 gains a constructed clevis-like case, so an end-to-end test exercises it; CT3's proptest draws groups; `--exhaustive` is documented (M5-42). M5-11 | `signature.rs::one_stack_for_a_group::level_one_keeps_a_part_one_stack_serves_a_group_with` (inherited), `sql_source_keeps_a_part_one_stack_serves_a_group_with`, `a_clevis_pin_one_shaft_for_two_holes_is_found`, `exhaustive_equals_the_file_run` |
| R-F4a | `get --format step` writes a blob without checking it (medium) | A blob edited in place (0444 is advisory) or damaged is written to `-o` and passed off as the matched part; the match never reads blobs, so nothing noticed | **Found by the review, fixed so:** `get --format step` reads the blob once, re-hashes it against the revision's SHA-256, and writes only that buffer; on a mismatch it writes nothing, exits 1, says "damaged" and points to `verify` (ST11, section 7.4). M5-41 | `get_step_refuses_a_damaged_blob` |
| R-F4b | A replace loses the bytes a past report judged (medium): a run passes `catalog:BRKT` at r1 (SHA X); BRKT is replaced by Y | `get BRKT --format step` writes Y without a word, and X's blob was unlinked by the replace's inline gc, so the judged bytes cannot be recovered | **Found by the review, fixed so:** labels are `catalog:<id>@r<n>` (CM6), outside the digest; references accept a strict `ID@rN` (ID9), any revision for `get`, only the current one elsewhere; a replace keeps the old revision's blob (CR3, IN3, IN9) until an explicit `fitment catalog prune --revisions` (CR11, section 7.10), which takes the write lock as gc does; `get` names on stderr the revision and SHA-256 it wrote. M5-2, M5-21, M5-41; the merge's open question 2 answered yes | `strict_revision_references`, `replace_keeps_the_old_revisions_bytes`, `labels_carry_the_revision`, `prune_is_all_or_nothing` |
| R-F5 | `judge_step` rebuilt on `judge_document` drops the STEP bytes from the fit request (low) | No cost in M5 (`NoFitVerifier`); in M7 every file run's fit request would lose its bytes, and with them its fit check, with no error | **Found by the review, fixed so:** a private `judge_document_with(doc, source, step)` is the one path; `judge_step` passes its bytes to `FitRequest.candidate_step`, `judge_document` passes none and is used for stored documents (QR7, section 5); M7's catalog passes checked blob bytes through it (QR13). M5-10 | `judge_step_hands_its_bytes_to_the_fit_request` |
| R-F6 | A reused blob skips the directory sync (low): concurrent ingest (CC2) reuses another process's rename before that process syncs, or an ingest reuses the orphan of one killed between rename and sync | A power loss leaves a committed row whose file's directory entry was never durable, breaking IN1; `get`, reindex and M7 fail, and the user's bytes may be gone. Kill -9 rounds cannot show it | **Found by the review, fixed so:** the fan-out directory of every blob a commit names is synced before that commit, renamed, reused or rewritten (ST3, ST6, ST9); IN1 states it holds across power loss. M5-15 | `every_named_blob_has_its_directory_synced_before_commit`, `a_reused_orphan_has_its_directory_synced` |
| R-F7 | A corrupt posting hides a part (low): postings carry no hash, and a run reads only those its probes return | Bit rot in a posting's diameter, or a lost row, hides a part, and the run exits 5, not 1; M5-32 and CM9 claimed "never hidden" | **Found by the review, fixed so:** plain `verify` recomputes postings from the hash-checked tokens and compares them (VF1, IN7); M5-32 says what holds, that a corrupt posting can hide a part until `verify` runs and can never pass one (IX9); the remaining risk is R20, and the README advises `verify` regularly. M5-32, M5-40 | `plain_verify_recomputes_postings`, `a_lost_posting_hides_a_part_until_verify`, `a_corrupt_posting_never_passes_a_part` |
| R-IX5 | A power-of-two bucket rounds down (note) | `2^ceil(log2(x))` is one ulp below x for x one ulp above 2^k (k ≥ 5: 32, 256, 1024), so the bucket would ask for more seat groups than level 2: a hidden part. B's prototype had `bucket.max(lever)`; the merged text dropped it | **Found by the review, fixed so:** IX5 states the bucket as `max(2^ceil(log2(x)), x)`, with the ulp reason; `seat_bucket` in section 5. M5-11 | `a_seat_bucket_is_never_below_the_lever` |

## 14. What was rejected from each designer, and why

Ids in brackets are the designer's own; the M5-n decision that settled each point follows.

**From A (the store):**

- **The extractor canary** (M5A-16, RX9, `reindex --force-version`): it embeds STEP bytes in a package, which M4's PR 7 packaging rule forbids (M5-14).
- **`--allow-stale`** (M5A-4): a partial search would depend on how far a reindex had got (M5-3).
- **`FITMENT_CATALOG`** (M5A-8): one way to name a catalog (M5-9).
- **`replace --quarantine`** (M5A-3): a broken update never takes a part out of the search (M5-7).
- **Selectors:** `e17` entry ids, 8-hex prefixes and exact `product_id` (PH6). The internal key stays internal, PRODUCT.id is never a key, and a 12-hex prefix leaves less room for a wrong match (M5-1, M5-21).
- **`fitment catalog match`:** one match command, `fitment match --catalog` (M5-19).
- **`fitment catalog log` and `op.at_unix_ms`:** no wall time, so history is identical everywhere (M5-23).
- **Pretty, byte-exact stored documents** (GT2): compact storage is 37% of the bytes and round-trips equally (M5-10).
- **`PostingSource`, `largest_lever_mm` and a public `index_bin`:** B's per-part lever removes the catalog-wide value and its scan, and bins stay private to core (M5-11).
- **Ingest's idempotent exit 0 for every duplicate:** refined by C's `--id` rule (M5-20).
- **`--recursive`** (PH4): one directory rule with `fitment match` (M5-33).
- **Exit 1 for `verify`'s findings:** 3 separates "found damage" from "could not run" (M5-25).

**From B (the index):**

- **QR5, a level-2 drop ends the candidate,** with `Matcher::keeps`: M5-4 lists drops with their evidence, so KH9 runs on every survivor.
- **Exit 5 for an empty catalog or a blocked socket** (CD7): the search could not run (M5-13).
- **The single `part` row per file,** updated by reindex: versioned, insert-only rows let a reindex resume and two versions share a catalog (M5-28, M5-29).
- **A quarantined row without its refused document:** the document is the reason users read, and a named refused candidate needs it (M5-8, CR9).
- **A catalog written by a newer fitment refused by every command but `list`** (RX3): A's never-downgrade rule lets an older build keep working on its own rows (M5-29).
- **One transaction per part:** batches cost one 3.74 ms commit per 64 files (M5-24).
- **`catalog check`** (RX6): it is `verify`, under one name. Its recomputation of the part digest, which the merge dropped, is restored in plain `verify` after the review (M5-40, R-F1).
- **A hidden `fitment catalog add --scaled`:** `tools/scale-step.py` instead, keeping test machinery out of the binary.

**From C (the CLI and D10):**

- **`fitment.match` v2:** the top-level and per-candidate `catalog` objects, the contents digest, the `properties` record and `material_differs`. v1 stays frozen and comparable (M5-2).
- **Material comparison and the per-candidate properties record** (C's PR5, PR10 and CD-14, `read_component`, `--original`'s properties): no positive mapping row exists, so nothing could be compared. Properties are shown by `get` (M5-5).
- **`--exhaustive` as a public flag** (CM6): rejected by the merge in favour of a hidden `--audit`, then **adopted after the review** (M5-42), so C's name and C's public flag stand.
- **256 MiB** (CD-13): the cap is 128 MiB, with stepq's graph about 10 times the file (M5-15).
- **A 10 s busy timeout:** 60 s, since a gc or reindex batch may hold the lock for seconds (M5-24).
- **`verify --repair`:** `gc` is the one repair (M5-25).
- **`catalog quarantine`:** folded into `list --status quarantined` (section 7.3).
- **`add` and `remove`:** `ingest` and `delete`, PLAN's exit verbs (M5-19).
- **`catalog.sqlite`, `blobs/<2>/<64>` and `blobs/tmp/`:** A's layout (M5-27).
- **`PROPERTIES_VERSION` refusing a match:** M5-26.
- **The unlink right after the commit, with exit 1 when it fails** (ST3, 5.6): A's gc under the lock (M5-6, merge finding 10).

## 15. Open questions, answered

Every question the merge left open, and every question the soundness review asked, is answered by the coordinator's resolutions. None remains open for M5.

**The merge's questions:**

1. **If PR 4 measures M5-4's cost as too high at 10⁵ on unselective sockets** (R2), what then? **Answered:** keep M5-4. PR 4 measures it at 10⁵; if one query exceeds 60 s on 16 threads, the fallback is level-2 drops counted rather than listed, with `--exhaustive` restoring them, recorded as the contingency decision M5-43. The lasting remedy stays a more selective level 1 (M5-35).
2. **Should the label carry the revision,** as `catalog:<id>@r<n>`? **Answered yes** (review, F4(b)): M5-2, CM6, with a strict `ID@rN` reference (ID9) and the old revision's bytes kept until `prune --revisions` (M5-41).
3. **Should `--jobs` apply to file runs too?** **Answered yes,** with the same deterministic output (M5-30, QR8); `--jobs 1` keeps MF3's memory.
4. **Should the server (M6) hold level 1 in memory?** **Deferred to M6** (M5-17). Built per query, memory was 1.5 to 3.6 times slower than SQL at 10⁵, and a cache needs invalidating on every write.
5. **Should `--catalog` accept several catalogs?** **Answered no:** one catalog per run in M5 (M5-44, CM1).
6. **Should a stored run (M6) record the generation and the counts of CM8?** **Deferred to M6** (M5-17), where section 6.3's `report` table is the default place, not the document.

**The soundness review's questions:**

1. **F3:** skip lines of the same SG5 group, or count one stack as a token at distance 0? And should `--audit` stop being hidden? **Answered:** the skip, already in core from M4's exit PR (IX7); `--audit` becomes the documented `--exhaustive` (M5-42).
2. **F1 and F2:** recompute the group key, bind the columns into the preimage, or both? **Answered: both** (M5-40). The binding catches a moved or edited row for free; the recomputation catches a digest written wrong at ingest.
3. **Open question 2 and F4(b):** the label `catalog:<id>@r<n>` and a strict `ID@rN`? **Answered yes** (M5-2, ID9).
4. **F4(b):** keep the old blob after a replace until an explicit flag? **Answered yes:** until `fitment catalog prune --revisions` (M5-41, CR11).
5. **F7:** is "a corrupt posting can hide a part until `verify` runs" acceptable once documented? **Answered yes:** M5-32's wording is corrected, plain `verify` recomputes postings, and the risk is recorded (R20). The match-time warning is not adopted.

## Evidence

Under the session's scratchpad `m5/`, not committed:

- **`A/design.md`,** the store design, and **`A/exp/`**: a Rust crate against the worktree's `fitment-core` (`cargo build --release`, rusqlite 0.40.2 bundled).
  - **The binaries:** `src/bin/roundtrip.rs`, `equiv.rs`, `tokdist.rs`, `sq.rs` (`fsync`, `commit`, `conc`, `snapshot`, `crash`, `full`, `unordered`), `psync.rs`, `race.rs`, `diskfull.rs`, `hash.rs`.
  - **The rest:** `featsize.py` with `files.txt` (the 490 STEP files), `sockets/` (the 17 sockets of the 90 match cases), and `work/`.
- **`B/design.md`,** the index design, and its experiments:
  - **`proto/`:** copies of `fitment-core` and `fitment` from the snapshot. `IndexSource`, `query_source`, `Deserialize` on the tokens and the per-part lever are added to `fitment-core/src/matching/index.rs`, and `probe/src/main.rs` (about 770 lines) has the subcommands `ingest`, `query`, `dedup`, `roundtrip`, `sigsize` and `scale`.
  - **Inputs:** `corpus/` (463 files generated by `tools/gen-corpus.py`, 81 MB), with `bases/` and `fx/` as symlinks to the parts used, and `sockets/` (the 32 match-case sockets).
  - **Scripts:** `compare.py`, `bench.sh`, `planes_probe.py`.
  - **`out/`:** `roundtrip*.txt`, `equivalence.txt`, `query-1e5-sql.txt`, `bench.txt`, `level1-sound-1e5.txt`, `level1-own-lever-1e5.txt`, `level1-bucket-lever-1e5.txt`, `query-dup-1e5.txt`, `dedup.txt`, `query-seats-only.txt`, `query-hostile.txt`, `query-fx-1e4.txt`, `ingest-*.txt`, `sizes.txt`, `plane-tokens.txt`, `determinism.txt`.
  - **The catalogs,** about 10 GB in all, are safe to delete.
- **`C/design.md`,** the CLI and D10 design, and **`C/proto/`:**
  - **CRUD:** `proto_catalog.py` (the CLI's CRUD semantics on sqlite3 with the real gate), with `scenario.sh` and `run.txt` (a scripted session with every exit code).
  - **D10:** `props_census.py` and `.txt` (every property name in the 45 fixtures, through `stepq props`), `raw_scan.py` and `.txt` (a record scan for material and thread text), `props_map.py` and `.txt` (section 4.13's mapping applied to every fixture), and `props_rows.json`.
- **`review/soundness.md`,** the soundness review, and **`review/probe/ws`**, its scratch copy of `fitment-core` with the F3 probe (`m5_review_probe` at the end of `matching/signature.rs`; `cargo test -p fitment-core --lib m5_review_probe`).
- **The repository read for context:** `docs/PLAN.md`, `docs/ARCHITECTURE.md`, `docs/research/m4-match.md`, `crates/fitment-core/src/matching/{index,matcher}.rs`, `crates/fitment/src/cmd/socket.rs` (`write_atomically`), `tools/check-core.sh` (the package check for STEP files), and `crates/fitment/tests/corpus.rs` (`design_rules`, the rule-id shape).

## Review applied

The soundness review's findings and the merge's open questions, as the coordinator resolved them, and where each landed. Section 13.3 has each finding's attack, fix and tests in one row.

| Finding | Resolution | Decisions | Rules | Core API, schema, CLI | Tests | PR |
| --- | --- | --- | --- | --- | --- | --- |
| F1, dedup on an unchecked column | Group by the digest recomputed from each checked document; a disagreeing column is corruption, skipped, named, exit 1; plain `verify` checks the column | M5-12, M5-32, M5-40 | QR4 step 5, QR6, IN5, VF1, 4.17 step 6 | `CheckedRow` (3.2); `part_digest` column comment (6.2); exit 1 (7.12); `verify` output (7.8) | `a_part_digest_column_that_disagrees_is_corruption`, `a_part_digest_forged_with_its_hash_is_still_caught`, `dedup_groups_by_the_recomputed_digest`, `plain_verify_checks_preimages_digests_and_postings` | 3, 4 |
| F2, a document not bound to its content | Each derived row's hash covers a preimage naming the row (tokens with the signature version, properties likewise); the source built from the checked row's SHA-256; plain `verify` checks the preimages | M5-10, M5-40 | GT2, DB7, IX9, PR3, QR4 step 2, QR7, IN5, IN7, VF1, VF2, 4.17 step 1 | `preimage.rs` (3.2); `judge_document`'s contract (5); hash column comments and the QR4 and tokens SQL (6.2); `verify` output (7.8) | `each_preimage_field_is_bound`, `a_features_row_copied_to_another_key_fails_its_hash`, `a_tokens_row_copied_to_another_key_fails_its_hash`, `a_features_row_moved_to_another_key_is_not_judged`, `the_source_is_the_checked_rows_own_sha256` | 2, 3, 4, 5 |
| F3, level 1 drops a group served by one stack | Inherited and done in M4's exit PR; `level_one` keeps the skip; a clevis-like case in CT1 | M5-11 | IX7, 4.17 step 3, R18 | `level_one`'s comment and the inherited-fix note (5) | `signature.rs::one_stack_for_a_group::level_one_keeps_a_part_one_stack_serves_a_group_with` (inherited), `sql_source_keeps_a_part_one_stack_serves_a_group_with`, `a_clevis_pin_one_shaft_for_two_holes_is_found`, CT3's proptest groups | 1, 4 |
| F4(a), `get --format step` unchecked | Re-hash before writing; mismatch exits 1, "damaged", points to `verify` | M5-41 | ST11 | `get` (7.4) | `get_step_refuses_a_damaged_blob` | 2 |
| F4(b), the judged bytes lost on replace | Labels `catalog:<id>@r<n>`; strict `ID@rN`; replace keeps the old blob until `prune --revisions`, under the write lock | M5-2, M5-6, M5-19, M5-21, M5-41 | ID7, ID9, CR1, CR3, CR11, CM6, CM10, IN3, IN9, ST5, RX4 | `revision.pruned_generation`, `op` kind `prune` (6.2); `list`, `get`, `replace`, `delete`, `reindex` (7.3 to 7.7); `prune` (7.10); `history.bytes_kept` (7.11); labels (7.12) | `strict_revision_references`, `replace_keeps_the_old_revisions_bytes`, `labels_carry_the_revision`, `prune_is_all_or_nothing` | 2, 3, 4 |
| F5, `judge_step` loses its STEP bytes | A private `judge_document_with` keeps the bytes for `judge_step`; `judge_document` for stored documents | M5-10 | QR7, QR13, 4.17 step 1 | `judge_step`, `judge_document_with` (5) | `judge_step_hands_its_bytes_to_the_fit_request` | 1 |
| F6, a reused blob's directory unsynced | Sync the directory of every blob a commit names, reused ones included | M5-15 | ST3, ST6, ST9, IN1, R7 | The flow (3.1) | `every_named_blob_has_its_directory_synced_before_commit`, `a_reused_orphan_has_its_directory_synced` | 2 |
| F7, "never hidden" holds only for survivors | Plain `verify` recomputes postings; M5-32's wording corrected; the remaining risk recorded | M5-25, M5-32, M5-40 | IX9, IN7, VF1, 4.17's assumptions, R20 | `verify` output (7.8) | `plain_verify_recomputes_postings`, `a_lost_posting_hides_a_part_until_verify`, `a_corrupt_posting_never_passes_a_part` | 3, 4, 7 (README) |
| IX5 note, a bucket rounding down | `max(bucket, lever)`, with the ulp reason | M5-11 | IX5 | `seat_bucket` (5) | `a_seat_bucket_is_never_below_the_lever` | 1 |
| `--audit` | Documented as `--exhaustive`; bypasses level 1 and judges every entry; tests use it to show level 1 hides nothing | M5-4, M5-42 | QR10, CR9, RX3, CM1, CM8, CM9, R18 | `cmd/match_.rs` (3.2); the synopsis (7.12) | `exhaustive_equals_the_file_run`, and the F3 and F7 cases | 4 |
| Merge Q1, M5-4's cost | Keep M5-4; PR 4 measures at 10⁵; over 60 s on 16 threads, drops counted not listed, `--exhaustive` restoring them | M5-4, M5-43 | R2 | — | CT11's trigger; `counted_drops_are_restored_by_exhaustive` if taken | 4 |
| Merge Q2, the label's revision | Answered yes by F4(b) | M5-2, M5-41 | ID9, CM6 | As F4(b) | As F4(b) | 2 to 4 |
| Merge Q3, `--jobs` in file runs | Yes, with the same deterministic output | M5-30 | QR8, CM1 | Synopsis (7.12); section 5's note | `jobs_do_not_change_a_file_runs_bytes` | 4 |
| Merge Q4 and Q6 | Deferred to M6 | M5-17 | — | — | — | — |
| Merge Q5, several catalogs | One catalog per run | M5-44 | CM1 | Synopsis and exit 2 (7.12) | `one_catalog_per_run` | 4 |
| The review's five questions | All answered by the rows above | — | — | — | — | — |

New rule ids: ST11, ID9, CR11, IN9. New decisions: M5-40 to M5-44. New risks: R20, R21. New command: `fitment catalog prune --revisions`. Renumbered: section 7.10 is now `prune`, the `fitment.catalog` document 7.11 and `fitment match` 7.12. No existing rule id changed.
