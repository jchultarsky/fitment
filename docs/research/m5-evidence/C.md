# fitment M5, design C: the catalog as the user sees it

Designer C · 16 Oct 2026 · emphasis: the CLI, CRUD semantics, D10 (properties), the catalog match as a user reads it.

Read against `origin/main` and the `m4/exit` worktree: PLAN.md (Metadata from STEP only, Components, M5's row, D5, D10), ARCHITECTURE.md, research/workspace-layout.md, vectera-audit.md, research/m4-match.md (sections 6.2, 9, 11, 13, 14), `crates/fitment/src/{main.rs,cmd/*.rs}`, `crates/fitment/tests/{cli.rs,match_cli.rs,fixtures.rs}`, `fixtures/manifest.json` and all 45 fixtures.

Prototypes, in `proto/` beside this file:

| File | What it is |
| --- | --- |
| `proto_catalog.py` | The CLI's CRUD semantics on sqlite3 and content-addressed blobs, with the **real** gate (`target/release/fitment features`) and `stepq props` |
| `scenario.sh`, `run.txt` | A scripted session on the unit parts, candidates and fixtures, with every exit code |
| `props_census.py`, `props_census.txt` | Every property name in the 45 fixtures, through `stepq props --format csv` |
| `raw_scan.py`, `raw_scan.txt` | A raw record scan for material and thread text (`MATERIAL_DESIGNATION`, `DESCRIPTIVE_REPRESENTATION_ITEM`, `PROPERTY_DEFINITION`, thread words) |
| `props_map.py`, `props_map.txt` | Section 6's mapping applied to every fixture's part-level properties |

---

## 1. Summary

- **A catalog is a directory** made by `fitment catalog init -C DIR`: one SQLite file and a store of STEP files named by their SHA-256. Every catalog command and `fitment match --catalog` names it with `-C/--catalog DIR`; there is no default location and no environment variable.
- **An entry has three names, each for one job.** The **id** is the user's handle (`BRKT-0042`), unique, ASCII, defaulting to the file's stem. The **SHA-256** is the content: what was judged. The **revision** counts the contents an id has held. PRODUCT.id is shown but is never an identity: the fixtures give two different parts the same PRODUCT.id `'1'`, and one part four spellings across five exports (section 6.3, E7 and E8).
- **The same bytes cannot be in the catalog twice.** A duplicate is not added and names the id that holds them.
- **`replace` keeps the id and gives it new bytes**, only if the new file passes the canonical gate. `remove` deletes the bytes and everything derived from them, and keeps a tombstone, so `(id, revision)` names one file forever.
- **A file the gate refuses is quarantined**: kept with its reasons, listed, never searched. `reindex` gates it again after an upgrade. A match says how many quarantined parts it did not search.
- **`fitment match SOCKET --catalog DIR`** searches the index and never opens a STEP file. A catalog with a stale entry is refused (exit 3) until `fitment catalog reindex`. Exit 3 now means "the search could not run" (a refused socket, a stale or empty catalog, a socket that blocks every catalog query), and 5 keeps "it ran and nothing passed".
- **A catalog run is the same judgment as an explicit run.** `--catalog DIR --id A --id B` gives the same `fitment.match` digest as passing A's and B's files; candidates are labelled by their catalog id.
- **D10, read off the fixtures:** no fixture states a material or a thread in a form fitment can use. One property name has a meaning fitment can read soundly: Siemens NX's `MaterialMissingAssignments` = `TRUE` (two accepted files) says the part has no material. Material notes are free text in drawing notes, threads are presentation strings and semantic diameters on cosmetic curves, never on the hole's faces. So M5 ships the property machinery (capture at ingest, a mapping table, a per-candidate record) with one row, and **neither M4 flag changes meaning**: `thread_not_confirmed` stays exactly VR19's, and `material_not_compared` stays on every verdict until a positive row is backed by a real sample. M5 says *why* per side and *which* requirements need a thread check.
- **Six PRs**, the last running the M4 corpus through a catalog: same verdicts, no false positive.

## 2. Decisions for the user, with defaults

Each has a default that applies unless decided otherwise.

| # | Decision | Default | Why | Alternative |
| --- | --- | --- | --- | --- |
| CD-1 | What identifies a catalog part | The user's **id** (default: the file stem mapped to the id alphabet) is the handle; the **SHA-256** is the content; **revision** numbers the contents an id has held | PRODUCT.id collides (`'1'` in clevis21 and gear) and diverges (AS1's bracket as `L-BRACKET`, `l-bracket`, `LBRACKET`, `l_bracket`); a hash changes with every re-export | PRODUCT.id as the key; the hash as the only name |
| CD-2 | The same bytes added twice | Not added, naming the id that holds them; exit 0 under a default id (the content is searchable), exit 2 when the user asked for the id with `--id` (it was not created) | Same bytes are the same part in every respect fitment knows (D8): a second id adds nothing and confuses replace | Aliases (one file, several ids) |
| CD-3 | `replace` with a file the gate refuses | Refused, exit 3; the entry, its bytes and its index are unchanged | Replacing a searchable part with a broken export would silently drop it from every search | `--allow-quarantine` |
| CD-4 | `remove` | Deletes the bytes and everything derived at once; a tombstone keeps the id, its revisions and their SHA-256s | PLAN: withdrawing a part is a delete; the layout: delete removes everything derived. The tombstone keeps old match documents auditable | A trash directory and `catalog gc` |
| CD-5 | Where the catalog is | `-C/--catalog DIR` on every catalog command and on `match`; no default, no environment variable | A write never lands somewhere the user did not name; tests never inherit a catalog | `FITMENT_CATALOG` as a fallback |
| CD-6 | The match document for catalog runs | `fitment.match` **v2**: v1 plus a top-level `catalog` object, a per-candidate `catalog` object, the `properties` record of section 6, and the flag `material_differs`; null where they do not apply. Explicit runs write v2 too | v1 is frozen and "a format change bumps the version" (layout, Documents). An audit needs to know what was searched. 0.1.0 is days old | v1 unchanged, the catalog named only in labels and stderr |
| CD-7 | Exit code when the search cannot run | 3: a refused socket (as now), a stale catalog, a catalog with nothing indexed, a socket that blocks every catalog query (SL8) | Exit 5 must keep meaning "searched, nothing fits". A script must never order a new part because the catalog path was wrong or a reindex is pending | 5 with a stderr warning |
| CD-8 | Stale entries in a catalog run | The run is refused (exit 3) until `fitment catalog reindex`; nothing is judged | A stored reading of older rules must not stand in for the current one (soundness rule 2 in time) | Search the current entries only, with a warning |
| CD-9 | Properties (D10) | Captured raw at ingest, mapped at match time by a versioned table that ships with one row (section 6.6); material compared only on exact stated equality; a difference is a flag, never a verdict change; no thread confirmed in M5 | Section 6's evidence | Ship guessed rows for formats no sample shows |
| CD-10 | Time in the catalog | None: a per-catalog **generation** counter orders every change | Determinism (ARCHITECTURE, rule 4) and reproducible tests | Wall-clock timestamps in `get` |
| CD-11 | Stored assemblies, sockets, report history | Not in M5's CLI: the user's socket file is the record; M6 adds them with the API | M5's exit is ingest, list, get, replace, delete and an index-only match | Store them now |
| CD-12 | Rejects in a catalog run's table | All passes, then at most 10 rejects (`--rejects N`, `--rejects all`); the document has every candidate | A one-line socket kept 34,718 of 10⁵ parts at level 1 (m4-match 27.1) | Every candidate always |
| CD-13 | Largest file `add` accepts | 256 MiB (`--max-mib`) | stepq's graph is about ten times the file (MF3); Vectera's upload cap was 256 MiB | No cap |
| CD-14 | Where the original's properties come from | Only `--original ASM` (the target's own definition); without it material is `not_compared`, reason `original_unknown` | `fitment.socket` v1 is frozen and carries no property | `fitment.socket` v2 with the target's properties |

D8 (ground truth) and D9 (default tolerance) were deferred to M5 "with catalog parts". Neither changes the catalog's design; the corpus PR (PR 6) reports what a real catalog would need, and both stay open with their M4 defaults.

## 3. Concepts

```text
DIR/                         made by `fitment catalog init`; nothing else may be in it
  catalog.sqlite             entries, tombstones, history, derived rows, generation (WAL mode)
  blobs/<2 hex>/<64 hex>     the STEP bytes, read-only, named by SHA-256 only
  blobs/tmp/                 a writer's temporary files; empty when no writer runs
```

An **entry** is one id with its current content. Its **status** is one of:

| Status | Meaning | Searched by a catalog match? |
| --- | --- | --- |
| `indexed` | The gate accepted the file; features, signature tokens and properties are stored by this build's versions | yes |
| `quarantined` | The gate refused it; its refusals are stored by this build's extractor | no; counted and named |
| `stale` | Either of the above, computed by another `EXTRACTOR_VERSION`, `SIGNATURE_VERSION` or `PROPERTIES_VERSION` | refuses the run (CD-8) |

`stale` is not stored: it is computed on every read by comparing the stored versions with the build's, so an upgrade or a downgrade makes the whole catalog stale at once, and `reindex` with any build makes it current for that build.

A **tombstone** is a removed id: its last revision and every SHA-256 it held, no bytes, no derived rows. An id can be added again; its revisions continue (CI6).

The **generation** is a counter in the catalog, raised by one in every transaction that changes an entry. History rows record it; `list` prints it; a match document records it without letting it into the digest.

## 4. Rules

Each rule has an id and a test (section 10). "Exit" codes are section 5's.

### 4.1 Identity (CI)

- **CI1. Ids.** An id matches `[A-Za-z0-9][A-Za-z0-9._+-]{0,63}`. It contains no `/`, `\`, `:`, space, control character or non-ASCII character, and never starts with `-` or `.`. An id outside the alphabet is a usage error (exit 2), never mapped when the user typed it.
- **CI2. Unique without case.** Two live entries never have ids equal under ASCII case folding. Lookup ignores ASCII case; output shows the id as stored.
- **CI3. Default id.** Without `--id`, the id is the file's name without its `.stp`, `.step` or `.p21` extension (any case), each run of characters outside the alphabet replaced by one `_`, leading `.`, `_`, `+`, `-` dropped, cut at 64 characters. An empty result is a conflict: "no valid id: give --id". The line that reports the file shows the mapped id. (`a b.stp` is `a_b`; a name holding an escape sequence and U+202E became `evil_31mred_` in the prototype.)
- **CI4. Exact stems first.** In one `add`, files whose stem is already a valid id are processed before files whose id had to be mapped, each group in byte order of its path. *Prototype finding:* in sorted order, `a b.stp` took the id `a_b` from `a_b.stp`, and `copy of m_bracket.stp` took `m_bracket.stp`'s bytes, leaving the real file a duplicate (`run.txt`, first `add`). With CI4 the exact names win.
- **CI5. One file, one entry.** No two live entries have the same SHA-256 (CD-2). Adding bytes held by entry X under X's id is `unchanged`; under any other id it is `duplicate of X`, nothing written. A duplicate under a default id exits 0: its content is in the catalog and a nightly `add vendor/` must not fail forever on a file the vendor ships twice. A duplicate under `--id` exits 2: the id asked for does not exist. `replace` with another entry's bytes exits 2 (5.5).
- **CI6. Revisions never repeat.** An entry's revision starts at 1 and rises by one with every `replace`. A removed id keeps its last revision in its tombstone; adding the id again continues from it. So `(id, revision)` names one SHA-256 forever, and a match document's `catalog.id` and `catalog.revision` can always be checked against the catalog.
- **CI7. References.** Commands that take a part accept `ID` or `sha256:HEX` with at least 12 lowercase or uppercase hex digits, matching one live entry. No match: exit 2 with up to five ids that share the longest prefix, case-folded. Several: exit 2 with them listed. `remove` accepts ids only, never `sha256:` (section 8, attack A12).
- **CI8. The bytes judged are the bytes stored.** A file is read once into memory; that buffer is hashed, gated, read for properties and written. A file changed while `add` runs cannot be stored under another file's hash.
- **CI9. PRODUCT.id and the file name are information.** Each is stored (escaped wherever printed) and shown; neither is unique, neither is a key, neither is ever read as a material, thread or size (PR9).

### 4.2 Store and safety (ST)

- **ST1. Paths come from hashes only.** No path inside DIR is built from an id, a file name or anything a STEP file states. A blob's path is `blobs/<first 2 hex>/<64 hex>`.
- **ST2. A blob is durable before its row.** A writer writes `blobs/tmp/<hex>.<pid>`, fsyncs it, renames it into place, makes it read-only, and fsyncs its directory (on Windows the directory sync is skipped); only then does it commit the row. A crash leaves at worst a temporary file or a blob with no row (an orphan), never a row without its blob.
- **ST3. Rows before unlinks.** `remove` and `replace` commit the row change first and unlink the old blob after. A crash leaves an orphan, never a row naming a missing blob.
- **ST4. One writer at a time.** A command that changes the catalog takes SQLite's write lock (`BEGIN IMMEDIATE`) only around its blob writes and its rows, never around a gate or an extraction. A second writer waits up to 10 s, then exits 1: "the catalog is busy".
- **ST5. Readers see one snapshot.** `list`, `get`, `quarantine`, `verify` without `--repair`, and `match --catalog` each read inside one read transaction, so a concurrent change never gives them half of one state.
- **ST6. SQLite settings.** WAL journal, `synchronous=FULL`, `foreign_keys=ON`, `trusted_schema=OFF`, `cell_size_check=ON`. Every statement is prepared with bound parameters; none is built from user text.
- **ST7. Schema version.** `catalog.sqlite` records `schema_version` (1). A newer one is refused by every command (exit 3: "this catalog was made by a newer fitment"). An older one does not exist yet; M6's first migration defines the rule.
- **ST8. Nothing else in DIR.** `init` refuses a non-empty directory that is not a catalog (exit 2). `add` and `replace` refuse an input inside DIR (exit 2). `match -o OUT` refuses an OUT inside DIR (exit 2, OUT untouched).
- **ST9. Read-only blobs.** Blobs are written read-only so an editor cannot change one in place. `remove`, `replace` and `verify --repair` clear the flag before unlinking (Windows refuses to delete a read-only file).
- **ST10. Size.** `add` and `replace` refuse a file over `--max-mib` (default 256) without reading past the limit: status `refused`, nothing stored (exit 3).

### 4.3 Quarantine (CQ)

- **CQ1. What is quarantined.** A file whose `features.v1` document is `refused` (M2's gate, D2): not STEP, an assembly, two solids, an unread shape item, and every other refusal code. The entry keeps its bytes and its refused `features.v1` document (header, refusals with code, message and `#id`, and the extractor version that refused it), so CQ3 and `get --format features` need no STEP read.
- **CQ2. Never searched.** A full catalog run neither reads nor counts a quarantined entry as a candidate. Its table header, its stderr summary and its document's `catalog.quarantined` say how many were not searched, and the stderr line names the command that lists them.
- **CQ3. Named, it is a refused candidate.** `match --catalog DIR --id Q` with Q quarantined judges Q as an explicit refused candidate: `Reject`, reason `candidate_refused`, with its stored refusal codes (VD10), exactly as `fitment match S Q.stp` would.
- **CQ4. An internal refusal is a bug.** A file refused with code `internal` is quarantined (so a later build can gate it again), and `add` exits 1 with "this is a fitment bug".
- **CQ5. Retry is reindex.** A quarantined entry is gated again by `reindex` when it is stale, or when named. Fixing the file is `replace ID fixed.stp`.

### 4.4 Staleness and reindex (RX)

- **RX1. Stale is computed.** An entry is stale when any stored version differs from the build's: `extractor_version` (both statuses), `signature_version` and `properties_version` (indexed entries).
- **RX2. A catalog run refuses staleness.** A full run refuses the catalog if any live entry is stale; an `--id` run refuses if any named entry is. Exit 3: "N parts were indexed by another fitment (extractor 6, this build 7): run `fitment catalog reindex -C DIR`".
- **RX3. One entry, one transaction.** `reindex` reads each entry's blob, checks its SHA-256, gates and extracts it outside the lock, and commits its new derived rows in a transaction of its own. An interrupted reindex leaves every entry either old (still stale) or new; running it again continues.
- **RX4. What it processes.** No argument: every stale entry. `ID...`: those entries, current or not. `--all`: every live entry.
- **RX5. A part that disappears is loud.** An entry that goes from `indexed` to `quarantined` is reported on stderr with its first reason ("BRKT-0042: no longer searched: …") and makes `reindex` exit 3.
- **RX6. A damaged blob is never indexed.** A blob that is missing or whose hash differs from its name makes `reindex` report it and exit 1; its entry keeps its old rows and stays stale, so catalog runs keep refusing until the user replaces or removes it.

### 4.5 Verify (VF)

- **VF1.** `verify` checks, in one snapshot: SQLite's `integrity_check`; the schema version; every live entry's blob exists with its recorded size; no live row names a blob another row names; every derived row belongs to a live entry; nothing in `blobs/` lacks a row (orphans) and nothing is in `blobs/tmp/`.
- **VF2.** `--deep` adds: every blob's SHA-256 recomputed; every stored features document read back by `FeaturesDocument::from_json` and its signature tokens recomputed and compared with the stored ones; every blob gated again and compared with the stored document byte for byte (the determinism check, on the user's own parts).
- **VF3.** `--repair` takes the write lock and deletes orphan blobs and temporary files, the only repair that cannot lose a part. It never deletes or edits a row; a row whose blob is missing or damaged is reported for the user to `replace` or `remove`.
- **VF4.** Each problem is one line on stdout: `missing`, `damaged`, `orphan`, `temporary`, `mismatch` (VF2's comparisons), `integrity`; exit 3 if there is any, 0 if none.
- **VF5. A catalog from elsewhere is trusted only after `verify --deep`.** The index is what a catalog match believes (CM1). A tampered `catalog.sqlite` whose features document states a hole the STEP file does not have would pass a candidate falsely; `--deep` re-reads every file and finds it. The user documentation says so where it explains copying a catalog.

### 4.6 CLI conventions (CC)

These extend the existing commands' conventions (`main.rs`, `cmd/*.rs`).

- **CC1. Exit codes.** 0 done; 1 error (unreadable, unwritable, a fitment bug); 2 usage (bad arguments, an unknown or ambiguous reference, an id conflict, a duplicate under `--id`, not a catalog); 3 refused input (quarantined, refused by the gate, stale, newer schema, verify's problems). In a command that handles several files, the run's code is the highest-precedence code of its files, in the order 1, 2, 3, 0.
- **CC2. Text for people goes through `printable()`.** Ids are safe by CI1; file names, PRODUCT names, refusal messages, property names and values, and DIR itself are escaped wherever printed to stdout or stderr.
- **CC3. Formats.** Each command that prints has `--format table|json`. `table` is for people and not a stable format. `json` is a `fitment.catalog` document (section 5.10), pretty-printed with a final newline, ordered by id case-folded then by bytes.
- **CC4. stderr.** One summary line per command, `fitment: <DIR>: …`, then warnings, one per line. stdout carries only the table or the document.
- **CC5. A closed stdout** (`| head`) ends output quietly and keeps the command's code.
- **CC6. No prompts.** No command asks a question. Destructive commands take exact ids (CI7) and say what they did.
- **CC7. Atomic outputs.** Every file the CLI writes outside DIR (`get --format step -o`, `match -o`) is written to a temporary file, fsynced and renamed; an output equal to an input or inside DIR is refused with exit 2 and left untouched. Today's `write_atomically` (`cmd/socket.rs`) renames without an fsync, so a crash can leave an empty socket file where a reviewed one was; PR 1 adds the fsync there too, which hardens `socket -o` and `review` (Vectera's lesson, audit section 1).
- **CC8. Without the feature.** A build without the `catalog` feature has no `catalog` command; `match --catalog` there exits 2 with "this build has no catalog (cargo feature `catalog`)".
- **CC9. Determinism of documents.** A `fitment.catalog` document holds no time, host or absolute path; `file_name` is the name the file had when added, and `changes[].input` is the argument as given. The same catalog state gives the same `list` bytes on every OS.
- **CC10. Counts are plain.** Numbers in tables have no thousands separators and no locale.

### 4.7 The catalog match (CM)

- **CM1. The index only.** A catalog run never opens a blob. It reads features documents, signature tokens and properties from `catalog.sqlite`. (Test: delete every blob; the run gives the same document.)
- **CM2. Exclusive inputs.** `--catalog` and positional candidates are exclusive (exit 2 if both or neither). `--id` and `--exhaustive` need `--catalog` and exclude each other.
- **CM3. A full run.** The socket's signature queries level 1 (SL7 to SL9) over every indexed entry; level 2 and verification run on what it keeps, each candidate's stored features read, judged and dropped before the next (MF3's memory rule). A level-2 drop ends the candidate (SL1, EV5): it is listed as `Reject` at stage `shortlist` with its `missing`, without checks.
- **CM4. An `--id` run.** The named entries are explicit candidates (M4-17): a shortlist drop still runs alignment and verification for the evidence, and a verification pass after a drop exits 1 as a fitment bug. A blocked socket does not stop them (M4-16). A quarantined one is CQ3's.
- **CM5. Same judgment as files.** For any set of entries E, `match S --catalog DIR --id E...` writes a document whose `digest` equals that of `match S <E's files>`. Judging goes through one core entry point for both (`Matcher::judge_document`, section 7), and the candidate's `source` is `{"kind": "file", sha256, bytes, header}` in both.
- **CM6. `--exhaustive`.** Every indexed entry is judged as an explicit candidate, level 1 bypassed. It exists to check SL9 on a user's catalog: a full run's passing candidates must be exactly the `--exhaustive` run's (PR 6 asserts it on the corpus).
- **CM7. Exit 3 when the search cannot run** (CD-7), in this order, before any candidate is judged and with nothing on stdout: the socket gate (MF2, unchanged); the catalog's schema (ST7); staleness (RX2); in a full run, no indexed entry ("the catalog holds no searchable part: 3 parts, all quarantined"); in a full run, a blocked socket (SL8): "socket-1: bore:hole-88 fails every candidate (unverifiable: unresolved): remove it in review, or no catalog part can be found". Everything after is VD14 unchanged: 1 over every verdict, else 0, 4 or 5.
- **CM8. Labels and names.** A catalog candidate's label is its id. The candidate's `catalog` object is `{"id", "revision"}`. The table shows `id rN`, the first 8 hex of the SHA-256, and PRODUCT.id (or the name if the id is empty, or `-`), escaped.
- **CM9. The `catalog` object** (top level, null for an explicit run): `generation`; `contents` (`"sha256:<hex>"` over the sorted lines `<sha256> <extractor_version> <signature_version> <properties_version>\n` of the entries the run could judge); `entries`, `indexed`, `quarantined` (live counts); `selection` (`all`, `ids` or `exhaustive`); `searched` (entries the run considered); `kept_by_index` (level 1's count; null unless `all`).
- **CM10. The digest.** MF7's digest also leaves out the top-level `catalog` object and every candidate's `catalog` object: they name what was searched, not what was judged. CM5 depends on it. `contents` lets M6 cache a full run by (socket digest, contents, policy, options).
- **CM11. Order** is MF5's: never by id.
- **CM12. The summary line** of a full run: `fitment: <socket>: catalog <DIR>: 1200 parts, 20 quarantined and not searched; 37 kept by the index; 37 judged: 0 match, 3 interface match, 34 reject; fit not checked (no fit verifier in this build)`.
- **CM13. The table** adds, after MF9's header, one line: `catalog generation 41  parts 1200: 1180 indexed, 20 quarantined (not searched)  kept by the index 37`. Candidate lines are MF9's with the id, revision and PRODUCT.id. After the passes, at most `--rejects N` rejects (default 10 in catalog runs, `all` in explicit runs), then `… 24 more rejects (in the document)`.
- **CM14. `--original`** works as in M4 and also supplies the original's properties (PR6).

### 4.8 Hostile names (HN)

- **HN1.** No path inside DIR comes from text (ST1).
- **HN2.** Ids cannot hold control, bidirectional or path characters, or start with `-` (CI1), so an id can neither move the terminal, nor escape DIR, nor be read as an option.
- **HN3.** Every other string reaches a terminal only through `printable()` (CC2): file names, PRODUCT.id and name, refusal messages, property names and values, DIR.
- **HN4.** `get --format step` writes bytes only to a file named by `-o`, never to stdout, so a STEP file's text never reaches a terminal raw.
- **HN5.** Strings from a STEP file are stored as stepq decodes them; a file name that is not UTF-8 is stored lossily (`to_string_lossy`), and CI3 maps it.
- **HN6.** SQL never contains user text (ST6); a `sha256:` reference is validated as hex before any query.
- **HN7.** Property capture is bounded (PR2): a file stating thousands of properties or megabyte values cannot grow the catalog without limit.

## 5. CLI reference

`-C DIR` is accepted anywhere after `catalog` (clap `global = true`). Synopses use clap's notation.

### 5.1 `fitment catalog init -C <DIR>`

Makes a catalog in DIR, creating DIR if needed.

| Exit | When |
| --- | --- |
| 0 | Created; or DIR is already a catalog of this schema ("already a catalog: unchanged") |
| 1 | DIR or its files cannot be created |
| 2 | DIR exists, is not empty and is not a catalog |
| 3 | DIR is a catalog of a newer schema |

stderr: `fitment: parts: catalog created`.

### 5.2 `fitment catalog add -C <DIR> <FILE|DIR>... [--id <ID>] [--max-mib <N>] [--format table|json]`

Adds each STEP file; a directory argument contributes its `.stp`, `.step` and `.p21` files (any case), not recursively (MF1's rule). `--id` needs exactly one file. Files are processed in CI4's order.

One line per file on stdout:

```text
added        m_bracket                1b77ea97  'm_bracket'  5 holes, 0 shafts, 6 planes
unchanged    a_b                      b3795fa5  'p03_blind_flat'
quarantined  r04_assembly             f8386a47  ''  refused: assembly: the file is an assembly with 2 usages of 3 product definitions; features reads one part
duplicate    copy_of_m_bracket        1b77ea97  the same bytes are m_bracket: not added
conflict     a_b                      e0382a9d  the id a_b is taken by another file (give --id): not added
refused      huge                     -         larger than --max-mib 256: not added
error        nowhere.stp              -         cannot read nowhere.stp: No such file or directory
```

(The first five lines are `proto/run.txt`'s, with the planned PRODUCT.id column added; the last two show the other results.) stderr: `fitment: parts: 8 files: 4 added, 2 quarantined, 1 duplicate, 1 conflict`; then one line per quarantined file with its first reason, and CQ4's line for an internal refusal.

| Exit | When (CC1's precedence across files) |
| --- | --- |
| 0 | Every file `added`, `unchanged`, or a `duplicate` under a default id |
| 1 | A file unreadable, a blob or row unwritable, an internal refusal |
| 2 | `--id` with several files, an invalid `--id`, an input inside DIR, a `conflict`, a `duplicate` under `--id` |
| 3 | A file `quarantined` or `refused` by size |

### 5.3 `fitment catalog list -C <DIR> [--status indexed|quarantined|stale|all] [--format table|json]`

Default `--status all`. Table, sorted by id case-folded:

```text
id                       rev  status       sha256    holes shafts planes  product            file
a_b                      r1   indexed      b3795fa5      1      0      7  'p03_blind_flat'   a_b.stp
clevis21                 r1   indexed      4fff4203      6      0     16  '1'                clevis21.stp
evil_31mred_             r1   indexed      622fec33      1      0      7  'p05_cbore'        evil\u{1b}[31mred\u{202e}.stp
junk                     r1   quarantined  f3ccaf5b      -      -      -  ''                 junk.stp
nist_stc_09_asme1_ap242-e3 r1 indexed      737423af     29      0     48  -                  nist_stc_09_asme1_ap242-e3.stp
```

An incomplete part (VD11) shows `indexed*` and a footnote. stderr: `fitment: parts: generation 14: 11 parts: 8 indexed, 3 quarantined, 0 stale`. Exit 0; 2 not a catalog; 3 newer schema; 1 unreadable.

### 5.4 `fitment catalog get -C <DIR> <REF> [--format table|json|features|step] [-o <OUT>]`

| Format | Prints |
| --- | --- |
| `table` (default) | The entry: id, revision, status, SHA-256, bytes, file name, PRODUCT.id and name, versions, feature counts; refusals; stored part-level properties one per line (`#7270 MaterialMissingAssignments = TRUE`), escaped; the history (`r1 1b77ea9724e6 generation 3 added`, …) |
| `json` | A `fitment.catalog` document with this entry, `properties` and `history` included |
| `features` | The stored `features.v1` document of an indexed entry, byte-identical to `fitment features <its file>` with the same build; exit 3 for a quarantined entry (its refused document) |
| `step` | The stored bytes, to `-o OUT` only (HN4), atomically (CC7); without `-o`, exit 2 |

Exit 0; 2 unknown or ambiguous REF (with suggestions), `step` without `-o`, OUT inside DIR; 3 stale for `features` ("indexed by another fitment") or newer schema; 1 unreadable blob or row.

### 5.5 `fitment catalog replace -C <DIR> <REF> <FILE> [--max-mib <N>] [--format table|json]`

Gives the entry new bytes, keeping its id; revision +1. The old blob is unlinked after the commit (ST3).

```text
replaced     m_bracket  r2  46cd8a91  'm_bracket_thick'  5 holes, 0 shafts, 6 planes   (was r1 1b77ea97)
```

| Exit | When |
| --- | --- |
| 0 | `replaced`, or `unchanged` (the same bytes) |
| 1 | Unreadable or unwritable |
| 2 | Unknown REF; FILE's bytes are another entry's (`duplicate`); FILE inside DIR |
| 3 | FILE refused by the gate or by size: entry unchanged (CD-3), with the refusal printed |

A quarantined entry can be replaced by an accepted file; it becomes `indexed`.

### 5.6 `fitment catalog remove -C <DIR> <ID>...`

Removes each entry: its row becomes a tombstone (CI6), its derived rows go, then its blob is unlinked. All or nothing: if any ID is unknown, nothing is removed (exit 2, the unknown ones listed). One line per entry: `removed  m_bracket  r2  46cd8a91`. Exit 0; 1 an unlink failed after the commit (the orphan is reported; `verify --repair` clears it); 2 unknown ids or a `sha256:` reference.

### 5.7 `fitment catalog reindex -C <DIR> [<REF>...] [--all] [--format table|json]`

Processes RX4's set. One line per entry processed, with its transition:

```text
indexed      m_bracket   r3  1b77ea97  stale (extractor 6) -> indexed
indexed      junk2fix    r1  9a0b11c2  quarantined -> indexed (now searched)
quarantined  BRKT-0042   r2  77aa01de  indexed -> quarantined: unread_shape_item: … (no longer searched)
```

stderr: a summary (`fitment: parts: 11 reindexed: 9 indexed, 2 quarantined; 1 no longer searched`) and RX5's lines. Exit 0 all current and none newly quarantined; 3 RX5; 1 RX6 or unwritable; 2 unknown REF.

### 5.8 `fitment catalog verify -C <DIR> [--deep] [--repair]`

VF1 to VF4. Output, one problem per line:

```text
damaged    clevis21  4fff420338c4  the file's SHA-256 is not its name
orphan     blobs/tmp/leftover.1234
```

stderr: `fitment: parts: 2 problems` (or `consistent`). Exit 0 consistent; 3 problems (after `--repair`, the problems left); 1 unreadable; 2 not a catalog.

### 5.9 `fitment catalog quarantine -C <DIR> [--format table|json]`

Every quarantined entry with every refusal, and the extractor that refused it:

```text
nist_ctc_04_asme1_ap242-e1  r1  20b43b54  refused by extractor 6
    unread_shape_item: #17462 (SHELL_BASED_SURFACE_MODEL) is in the part's shape but is not read; a canonical part's shape is one solid, plus curve and point sets
r04_assembly                r1  f8386a47  refused by extractor 6
    assembly: the file is an assembly with 2 usages of 3 product definitions; features reads one part
```

stderr ends with the two ways out: `fitment catalog replace -C DIR ID FIXED.stp` or `fitment catalog reindex -C DIR` after an upgrade. Exit 0.

### 5.10 The `fitment.catalog` document (v1)

```json
{
  "format": "fitment.catalog", "format_version": 1,
  "schema_version": 1, "generation": 14,
  "build": { "extractor_version": 6, "signature_version": 1, "properties_version": 1 },
  "counts": { "entries": 11, "indexed": 8, "quarantined": 3, "stale": 0 },
  "entries": [
    { "id": "m_bracket", "revision": 3, "status": "indexed",
      "sha256": "1b77ea9724e67c4bdc709a871a00a05580dfebb36e4109f3c027d84afe826c33", "bytes": 14213,
      "file_name": "m_bracket.stp",
      "product": { "id": "m_bracket", "name": "m_bracket" },
      "versions": { "extractor_version": 6, "signature_version": 1, "properties_version": 1 },
      "complete": true,
      "features": { "planes": 6, "holes": 5, "shafts": 0, "cones": 0, "other_faces": 0 },
      "refusals": [],
      "properties": null,
      "history": null }
  ],
  "changes": null
}
```

- `status` is `indexed`, `quarantined` or `stale` (computed, RX1). `refusals` are `{code, message, instance}` as `features.v1` writes them.
- `properties` (`get` only, else null): `{"truncated": false, "stated": [{"instance": 7270, "name": "MaterialMissingAssignments", "value": "TRUE", "measure": null, "unit": null}]}`.
- `history` (`get` only, else null): `[{"revision", "sha256", "generation", "event": "added|replaced|removed"}]`.
- `changes` (`add`, `replace`, `remove`, `reindex` with `--format json`): one per input or entry, `{"input", "result", "id", "revision", "sha256", "detail"}`; `entries` then holds the entries changed.

### 5.11 `fitment match` with a catalog

```text
fitment match <SOCKET> (<CANDIDATE>... | --catalog <DIR> [--id <ID>]... [--exhaustive])
              [--original <ASSEMBLY>] [--format table|json] [-o <OUT>] [--explain]
              [--rejects <N|all>] [--require-fit] [--require-reviewed]
              [--clearance strict|functional] [--tolerance-mm <X>] [--tolerance-deg <Y>]
```

| Exit | When | Order |
| --- | --- | --- |
| 2 | Usage: CM2; an unknown or ambiguous `--id`; DIR not a catalog; OUT inside DIR (ST8); every M4 usage error | first |
| 3 | CM7: the socket gate; a newer schema; stale entries; nothing indexed; a blocked socket in a full run | before any candidate; nothing on stdout |
| 1 | `catalog.sqlite` unreadable; a stored features document that does not read back ("the index is damaged: run `fitment catalog verify --deep`"); every M4 error | over every verdict |
| 0, 4, 5 | VD14 | |

A full run's table against s01's bracket socket, abbreviated (the shape is fixed by CM13; the values are illustrative):

```text
socket sha256:124f4f64627f  target 'BRACKET'  1 socket(s)  policy 0.01 mm / 0.01 deg, clearance strict, grip hard  fit: not checked (no fit verifier in this build)
catalog generation 41  parts 1200: 1180 indexed, 20 quarantined (not searched)  kept by the index 37
INTERFACE  BRKT-0042 r1  23cb289f  'BRACKET'  all requirements verified; material_not_compared, requirements_unreviewed
    socket-1/seat:plane-467  passed  plane-88  margin 0.01 mm
    …
INTERFACE  NUT-M5 r2  0be1c2aa  'NUT'  all requirements verified; material_not_compared, thread_not_confirmed
    thread not confirmed at socket-1/bore:hole-88 (pinned, interference): check that the hole is tapped
    …
reject     BRKT-0107 r3  9a0c11e2  'L-BRKT'  1 failed: socket-1/bore:hole-654 no_feature
reject     BRKT-0311 r1  5d0e9f01  -  shortlist: socket-1/bore:hole-654 diameter [6.590001, 6.609999]
  … 24 more rejects (in the document)
```

A catalog candidate in the v2 document (only the fields that differ from an explicit run's):

```json
{ "labels": ["BRKT-0042"],
  "catalog": { "id": "BRKT-0042", "revision": 1 },
  "source": { "kind": "file", "sha256": "23cb289f…", "bytes": 14213, "header": { "…": "…" } },
  "flags": ["material_not_compared", "requirements_unreviewed"],
  "properties": {
    "material": { "status": "not_compared", "reason": "original_unknown",
                  "detail": "the original's properties are not known to this run (give --original)",
                  "original": null,
                  "candidate": { "stated": [], "unassigned": false, "truncated": false } } } }
```

and at the top level:

```json
"catalog": { "generation": 41, "contents": "sha256:…", "entries": 1200, "indexed": 1180, "quarantined": 20,
             "selection": "all", "searched": 1180, "kept_by_index": 37 }
```

### 5.12 A session

```text
$ fitment catalog init -C parts
fitment: parts: catalog created
$ fitment catalog add -C parts vendor/                     # 1,200 files
fitment: parts: 1200 files: 1176 added, 20 quarantined, 4 duplicate
$ echo $?
3                                                          # some files are not searchable
$ fitment catalog quarantine -C parts                      # why: 14 assemblies, 6 surface bodies
$ fitment socket asm.stp --part BRACKET -o bracket.socket.json
$ fitment review bracket.socket.json                       # keep or remove requirements
$ fitment match bracket.socket.json --catalog parts --original asm.stp
…                                                          # the table of 5.11
$ echo $?
4                                                          # interface matches; fit not checked
$ fitment catalog get -C parts BRKT-0042 --format step -o BRKT-0042.stp
$ fitment catalog replace -C parts BRKT-0107 vendor-2027/BRKT-0107.stp
$ brew upgrade fitment                                     # a new extractor
$ fitment match bracket.socket.json --catalog parts         # exit 3: the socket gate comes first (CM7)
fitment: bracket.socket.json: refused: extracted by extractor 6 and socket rules 6; this build is 7 and 6: extract it again (`fitment socket … -o FILE` carries decisions over)
$ fitment socket asm.stp --part BRACKET -o bracket.socket.json   # decisions carried over
$ fitment match bracket.socket.json --catalog parts         # exit 3: then the catalog
fitment: parts: refused: 1196 parts were indexed by another fitment (extractor 6, this build 7): run `fitment catalog reindex -C parts`
$ fitment catalog reindex -C parts
fitment: parts: 1196 reindexed: 1178 indexed, 18 quarantined; 2 newly searched, 0 no longer searched
```

## 6. Properties (D10)

### 6.1 Method

`stepq props --format csv` and `--format json` over all 45 fixtures (`props_census.py`), a raw scan of every record for material and thread text (`raw_scan.py`), `stepq pmi --geometry` on the two files with thread callouts, and `fitment features` on each file to know whether a catalog could hold it at all. The NIST README says the originating system "has been removed" from its files, so an exporter is named below only where a property in the file names it.

### 6.2 What the files state

| | Count |
| --- | --- |
| Fixtures | 45; 29 with any property (as M4 found) |
| Files with part-level user properties (subject the PRODUCT_DEFINITION) | 9 (`props_map.txt`); 5 of them accepted by the gate |
| `MATERIAL_DESIGNATION` records | 0 in all 45 |
| A material name or grade as a property value | 0 |
| A thread as a property | 0 |
| A material anywhere | 2 drawing notes (E3) |
| A thread anywhere | presentation text in 2 files and cosmetic curves in 2 (E5, E6) |

### 6.3 Evidence

| # | File (gate) | Record | Text | What it means for fitment |
| --- | --- | --- | --- | --- |
| E1 | `nist_ctc_05_asme1_ap242-e1.stp` (accepted, 21 holes) | `#1095`, value `#897` | `PROPERTY_DEFINITION('MaterialMissingAssignments',$,#13386)`; `DESCRIPTIVE_REPRESENTATION_ITEM('MaterialMissingAssignments', 'TRUE')` | The file states that no material is assigned. `#1101` `CAD_SOURCE` = `'ug'` (value `#902`): Unigraphics, now NX |
| E1b | same | `#1096`, value `#898` | `MaterialMultipleAssigned` = `'FALSE'` | Says nothing about which material |
| E1c | same | `#1099` | `Part Number` = `'NIST PMI CTC 05 ASME1'`, while PRODUCT.id (`#13393`) is `'nist_ctc_05_asme1'` | Even "the part number" has two spellings in one file (CI9) |
| E2 | `nist_stc_09_asme1_ap242-e3.stp` (accepted, 29 holes) | `#7270`, value `#7268` | `PROPERTY_DEFINITION('MaterialMissingAssignments','User Defined Property',#29)`; `DESCRIPTIVE_REPRESENTATION_ITEM('MaterialMissingAssignments','TRUE')` | As E1. `#7300` `Part File Format` = `'NXV2007'` names NX |
| E3 | same | `#8518` | `DESCRIPTIVE_REPRESENTATION_ITEM('','NOTES (UNLESS OTHERWISE SPECIFIED): … 5. MATERIAL: (.1195 THICK) 11 GA 304 SST SHT, ASTM A240. 6. UNITS: INCHES')` | The material is a sentence in a drawing note: semantic text, not a property. `nist_ftc_09_asme1_ap242-e1.stp` `#9638` holds the same note. Reading it would be parsing prose: never (PR9). And the same file's `MaterialMissingAssignments` says TRUE: the CAD model has no material even where the drawing names one |
| E4 | `nist_ctc_04_asme1_ap242-e1.stp` (refused: `unread_shape_item`, `#17462` SHELL_BASED_SURFACE_MODEL) | `#18079`, value `#18078` | `PROPERTY_DEFINITION('Density','',#11)`; `MEASURE_REPRESENTATION_ITEM('',POSITIVE_RATIO_MEASURE(1000.0),#18077)` | One of a mass-properties set (`Mass`, `Volume`, `Surface`, `Ixx`…`Izz`, `x/y/zCenterOfGravity`). The same set with `Density` 1000.0 is in `nist_ftc_07` (`#13557`) and `nist_ftc_10` (`#17982`): the same value in three different parts, a CAD default, not a material |
| E5 | `nist_ctc_04_asme1_ap242-e1.stp` | `#18961` | `DESCRIPTIVE_REPRESENTATION_ITEM('equivalent unicode string','DIM\\w4X M12x1.75-6H')` | The thread is presentation text (a PMI validation string). The semantic PMI is `dimension diameter 12.0` on `COMPOSITE_GROUP_SHAPE_ASPECT #18866`, whose geometry is `GEOMETRIC_CURVE_SET #17495 #17519 #17539 #17559` named `'Thread Cylinder.1'` (`CIRCLE #17483`, radius 6.0): cosmetic curves, not faces. The hole itself is `ADVANCED_FACE #6050` on `CYLINDRICAL_SURFACE #6049` radius **5.1**, `same_sense .F.`: a plain Ø10.2 hole (the M12×1.75 tap drill), coaxial with a bare construction surface `#18007 'Extended Cylinder.5'` radius 6.0 |
| E6 | `nist_stc_10_asme1_ap242-e2.stp` (accepted) | `#1238`, `#1240`, `#1241` | `DESCRIPTIVE_REPRESENTATION_ITEM('equivalent unicode string','DIM\\w2 XM3 x 0.5-6g')` in `REPRESENTATION #3032` with a tessellated curve's length and centre (`#2509`, `#11860`) | Validation text of a `'Radial Hole and Thread Callout'` (`DRAUGHTING_CALLOUT #1572` …). No semantic thread; `fitment features` reads six plain blind Ø3 holes and eight Ø2.5 holes of unknown kind |
| E7 | `nist-edm/clevis21.stp`, `nist-edm/gear.stp` (both accepted) | their PRODUCTs | PRODUCT.id `'1'`, name `'Product'` in both | Two different parts, one PRODUCT.id (CD-1) |
| E8 | `steptools/as1-{ac,md,tc,ug}-214.stp`, `nist-edm/as1_pe.stp` | their PRODUCTs | the bracket as `L-BRACKET` (ac, pe), `LBRACKET` (md), `l-bracket` (tc), `l_bracket` (ug); `as1-ec` numbers its products `as1 1`, `as1 1.1`, `as1 1.1.2` … | One part, four spellings across five exports, and a sixth export with none of them (CD-1) |
| E9 | `nist_stc_06_asme1_ap242-e3.stp` (accepted) | `#5087` … | 440 part-level user properties `p1`, `p3`, … each a `LENGTH_MEASURE` ("user defined attribute") | CAD parameters. A real file states hundreds of properties: capture must be bounded and must say when it is cut (PR2) |

### 6.4 What this means

1. **Material.** No accepted fixture states a material fitment could compare. The one sound reading is negative: NX's `MaterialMissingAssignments` = `TRUE` says the model has no material.
2. **Thread.** Threads are stated only as presentation strings or as semantic diameters on cosmetic curves (E5, E6). Tying either to a hole means matching curves to a cylinder's axis by geometry, and reading a designation means parsing presentation text. Both are inference. The modeled hole is the tap drill, Ø10.2 for an M12, so PLAN's stated limit holds on real data: a tapped hole reads as a plain one, here even of another diameter than its thread.
3. **The cosmetic-thread files are not the problem for the gate.** `ctc_04` and `ftc_10` are refused for surface bodies (`SHELL_BASED_SURFACE_MODEL`), not for their thread curves; curve sets are allowed (D2). `stc_10`, with thread callouts, is accepted.

### 6.5 Rules (PR)

- **PR1. From STEP only (D8).** Properties are read through fitment-core (`stepq::props` stays behind core's wall) from the bytes being added, and from the original's definition in its assembly under `--original`. Nothing else is a source: not the file name, not the id, not PRODUCT.name.
- **PR2. What is captured, bounded.** User-defined properties whose subject is the part's own PRODUCT_DEFINITION (or its PRODUCT_DEFINITION_SHAPE), never a shape aspect's (a feature's) and never an enclosing assembly's (an assembly's "material" does not say what each component is made of); plus any `MATERIAL_DESIGNATION` whose definitions include the part's. Each value as written: name, value text, measure type, unit, the property's `#id`. At most 1,024 values of at most 4 KiB each; past either bound the set is marked `truncated` and PR5 compares nothing from it (E9 has 440).
- **PR3. Captured at ingest, mapped at match.** The raw set is stored with the entry and versioned by `PROPERTIES_VERSION` (core), which rises only when the capture changes (RX1 makes such entries stale). The mapping is applied when a match runs, so a new mapping row needs no reindex. The mapping is versioned with `MATCH_RULES_VERSION` (M4, section 14).
- **PR4. The mapping table.** Rows of `(name, value or any) → (field, meaning)`, the name compared after trimming and ASCII case folding, the value as written. Fields: `material`, `thread`, `finish`. Meanings: `designation` (the value names it), `unassigned` (the file states there is none). Every row cites a sample file and record that exercise it, and a test pins that record (PR 5's `properties.rs`). A row with no real sample is not added.
- **PR5. Material is compared only when both sides state exactly one designation**, the original's (PR6) and the candidate's, each from a mapped row, neither set truncated. Equal when the two values are byte-equal after trimming, collapsing whitespace to one space, and ASCII upper-casing; no synonyms, no grades parsed, no partial match.
  - `equal`: `material_not_compared` is not set; the record shows both values.
  - `differs`: `material_not_compared` is not set; the flag `material_differs` is set; the record and the table show both values.
  - otherwise `not_compared`, with the first reason code that applies, in this order: `original_unknown` (no `--original`), `truncated` (either side), `several_stated` (either side), `original_unassigned`, `original_states_none`, `candidate_unassigned`, `candidate_states_none`. `material_not_compared` is set. Each side's record (`stated`, `unassigned`, `truncated`) is written whatever the reason, so the user sees both.
- **PR6. The original's properties** come from the target's own definition in the assembly named by `--original`, whose SHA-256 must be the socket's source (VD15). Without it, `original_unknown`.
- **PR7. No thread is confirmed in M5.** `thread_not_confirmed` is set exactly as VR19 sets it. A thread may be confirmed by a later milestone only when the original's hole and the candidate's hole that passed the requirement each carry semantic PMI that stepq links to **that hole's faces** (PMI to faces, stepq 0.6), stating the same designation; a part-level property, a callout's text, a cosmetic curve set and a diameter never confirm one. The socket would need to carry the original's designation, a `fitment.socket` v2: later work.
- **PR8. Properties never change a verdict kind** and never pass or fail a requirement. They change flags and the `properties` record only.
- **PR9. No reading of prose or names.** Drawing notes (E3), presentation strings (E5, E6), PRODUCT.id and name, file names and catalog ids are never parsed for a material, a thread or a size, even when they say `M6x20` or `304 SST`.
- **PR10. Where a thread needs checking.** The candidate's `properties.thread` is present when `thread_not_confirmed` is set: `{"status": "not_confirmed", "requirements": ["socket-1/bore:hole-88", …]}`, the passed requirements that carry the socket's flag. The table prints one line per requirement: "thread not confirmed at socket-1/bore:hole-88 (pinned, interference): check that the hole is tapped".

### 6.6 The mapping as shipped

| Name (case-folded) | Value | Field | Meaning | Sample (record) |
| --- | --- | --- | --- | --- |
| `materialmissingassignments` | `TRUE` | material | `unassigned` | E1 `nist_ctc_05_asme1_ap242-e1.stp` `#1095`/`#897`; E2 `nist_stc_09_asme1_ap242-e3.stp` `#7270`/`#7268` |

Considered and **not** mapped, with the reason kept in the table's documentation: `Density` (E4: a default value, not a material); `MaterialMultipleAssigned` (E1b: says nothing about which); `Part Number`, `Description`, `CAD_SOURCE`, `Part File Format` (identification, not properties of the part's material or threads); AP242's `MATERIAL_DESIGNATION` with a `material property` density (the documented practice, likely the first positive row, but in no sample yet).

`props_map.txt` applies this table to every fixture: two files map (E1, E2), both to `unassigned`. So on the fixtures no material can be compared, and two parts can say why from what the file itself states.

### 6.7 Do the M4 flags change?

- **`thread_not_confirmed`:** no. Same condition (VR19), same meaning; M5 adds PR10's list of where.
- **`material_not_compared`:** its condition becomes PR5's (set unless the material was compared), its meaning does not change. With the shipped table no comparison can happen, so it stays on every M5 verdict, as in M4. What changes is the record: it says why, per side. An original read with `--original` from an NX assembly and a candidate from an NX file would both say `unassigned`.
- **New, reserved in practice:** `material_differs` joins VD12's closed list (v2). No shipped row can set it until a positive row exists.

## 7. What M5 needs from fitment-core

The CLI never names stepq; these keep it so.

| Need | API | Why |
| --- | --- | --- |
| Judge from a stored document | `Matcher::judge_document(&FeaturesDocument, CandidateSource) -> CandidateResult`, with `judge_step` defined as `judge_document(&FeaturesDocument::read(bytes), File(..))` | CM5 by construction; CQ3 (a refused document gives VD10's verdict) without re-reading STEP |
| The candidate's file source from a document | `Source::from_document(&FeaturesDocument, sha256, bytes)` or a public constructor | CM5's identical `source` |
| Stored tokens | `IndexTokens: Deserialize` (or an encoding the storage design chooses), `SIGNATURE_VERSION` recorded beside them | Level 1 from the index without re-deriving every signature (CM1) |
| Properties | `properties::read(step: &[u8]) -> StatedProperties` and `read_component(assembly: &[u8], definition: u64)`, `PROPERTIES_VERSION`, PR2's bounds | PR1, PR6 |
| The mapping | `PropertyMap::shipped()`, `PropertyMap::compare(original, candidate) -> MaterialRecord`; a test-only constructor for other rows | PR4, PR5; the comparison is tested with rows that do not ship |
| Document v2 | `MATCH_FORMAT_VERSION` 2, `MatchDocument` with the catalog objects and CM10's digest; `CandidateFlag::MaterialDiffers` | CD-6 |
| Blocked, explained | `SocketSignature::blocked_by()` already exists; the CLI needs each blocking requirement's reason | CM7's message |

## 8. Attacks on this design, and their fixes

Each asks: can a user be misled into accepting a part that does not fit, or lose catalog data? The fix is in the rules already, by id.

| # | Attack | What would go wrong | Fix |
| --- | --- | --- | --- |
| A1 | Upgrade fitment; match before reindexing | Features read by older rules stand in for the current ones; a rule fixed for a false positive is bypassed | RX1, RX2: stale computed on every read, the run refused (exit 3) |
| A2 | A downgrade | The same, the other way | RX1 compares for inequality, not age |
| A3 | `replace` with a broken export | The part silently leaves every search | CD-3: refused, entry unchanged, exit 3 |
| A4 | Yesterday's match says `BRKT r1` is an interface match; today someone replaced BRKT; the user orders "BRKT" | The id now names other bytes | CI6 and CM8: the document records the revision and SHA-256; `get` shows the history; revisions never repeat, even after remove and re-add |
| A5 | A crash in the middle of `add`, `replace` or `remove` | A row whose file is gone, or a file lost | ST2, ST3: blob durable before its row, row changed before the unlink; the worst left is an orphan, which `verify --repair` clears |
| A6 | A match while `add` or `reindex` runs | A half-changed catalog: a mixed set, or a row whose features are from one file and tokens from another | ST5: one read transaction; derived rows change in the entry's own transaction (RX3). A run during a reindex sees a mix of versions and refuses (RX2) rather than judging a mix |
| A7 | A tampered or corrupted `catalog.sqlite` (copied from someone else, a bad disk) | Stored features stating a hole the file lacks pass a candidate falsely | VF2, VF5: `verify --deep` re-reads every file and compares; ST6's pragmas against hostile database files; `from_json` refuses documents that break `read_part`'s guarantees (CM's exit 1 "the index is damaged") |
| A8 | A damaged blob | `reindex` indexes bytes that are not the file added | RX6: the hash is checked first; the entry stays stale and blocks catalog runs |
| A9 | Level 1 drops a part that would pass | A false negative the user cannot see | SL9 measured in M4; CM6's `--exhaustive` on a user's catalog; PR 6 asserts equal passing sets on the corpus |
| A10 | Every catalog part quarantined, or the wrong DIR | Exit 5, "nothing fits", and the user buys a new part | CD-7, CM7: no indexed part is exit 3; a DIR that is not a catalog is exit 2; CQ2's count on every run |
| A11 | A socket with an unverifiable requirement in a full run | Exit 5 again, though nothing could ever pass | CM7: blocked is exit 3, naming the requirement and `fitment review` |
| A12 | `remove` with a typo, or a hash prefix | Another part deleted | CI7: `remove` takes exact ids only; all or nothing on unknown ids; the tombstone keeps the SHA-256 so the user knows what went |
| A13 | Batch `add` of a vendor directory with similar names | Default ids collide; the wrong file gets the clean id | CI4 (found by the prototype); every conflict printed, exit 2, nothing overwritten |
| A14 | The same bytes under two ids | Two candidates identical, one replaced later: two lineages of one file | CI5: not added, naming the holder |
| A15 | A file name or PRODUCT.name holding escape sequences or U+202E | The table rewrites itself or reorders a line, hiding a reject | CC2, HN2, HN3; the prototype's hostile name prints escaped (`run.txt`) |
| A16 | `get --format step` to a terminal | A STEP file's text reaches the terminal raw | HN4 |
| A17 | A property says `Material = Steel` in both files, both CAD defaults | "equal" read as "verified" | PR5 shows both stated values, never "verified"; PR8: no verdict changes. No shipped row can produce it yet; each future row must bring its exporter's default values from samples (PR4). Listed as R8 |
| A18 | Material stated twice, one value cut by the bound | Compared as one value, wrongly equal | PR2: a truncated set compares nothing; PR5: `several_stated` |
| A19 | An assembly-level material applied to the original | The original's material guessed from its parent | PR2: the target's own definition only |
| A20 | A catalog id `SHCS-M6x20`, or a note "M6 TAPPED" | Read as a thread confirmation | PR9 |
| A21 | `match -o DIR/catalog.sqlite` or `get -o` into DIR | The catalog overwritten by a report | ST8, CC7 |
| A22 | A blob edited in place by another program | Its bytes no longer what was judged | ST9 read-only; VF2 finds it |
| A23 | Windows: read-only blobs cannot be deleted; directories cannot be fsynced | `remove` fails; ST2 errors on open | ST9 clears the flag first; ST2 skips the directory sync on Windows |
| A24 | A huge or endless file | Memory exhausted at ingest | ST10 reads at most the limit |
| A25 | The obsolete part itself is in the catalog | It is "found" as a perfect match | Catalog membership means available (PLAN). Not detected: comparing it with the original is guessing. The documentation says to remove withdrawn parts; R9 |
| A26 | Two catalogs with the same contents, different histories | Different digests for the same judgment | CM10: generation and ids are out of the digest; `contents` is history-free |
| A27 | A catalog in a synced or network folder | SQLite's WAL is unsafe there; silent corruption | R5: documented; `verify` finds the result. Not detectable reliably |

## 9. What the prototype showed

`proto/proto_catalog.py` ran the semantics of sections 4 and 5 with the real gate over the unit parts, three candidates and five fixtures (`proto/run.txt`). It confirmed the design where noted and changed it in two places:

- **Changed: CI4.** In plain sorted order, `a b.stp` (mapped to `a_b`) was processed before `a_b.stp` and took its id, and `copy of m_bracket.stp` took `m_bracket.stp`'s bytes, so the real file was reported a duplicate. Processing exact stems first fixed both (second run in `run.txt`).
- **Changed: CI5's exit.** Re-running `add` on a directory that holds a file twice exited 2 on every run, so a scheduled sync would always fail on a vendor's harmless duplicate. A duplicate now exits 2 only under `--id`.
- **Confirmed:** `unchanged` on a second `add`; `replace` with a refused file left the entry at its revision and SHA-256 (exit 3); `remove` with one unknown id removed nothing; re-adding a removed id continued its revisions (r1, r2, removed, r3); `verify` found an appended blob (`damaged`) and a stray temporary file; every entry listed as `stale` under a different extractor version; the hostile file name printed escaped; `r04_assembly.stp`, a junk file and `nist_ctc_04` were quarantined with the gate's own messages.
- **Measured for PR2:** `nist_stc_06` states 440 part-level properties, so the capture bound is 1,024, not the 256 first planned.

## 10. Test plan

New and extended files, in the style of `cli.rs` and `match_cli.rs`: `assert_cmd`, a fresh `scratch` directory per test, inputs from `crates/fitment-core/tests/{data,candidates,assemblies}`, fixtures optional unless `FITMENT_REQUIRE_FIXTURES=1`, every document checked by the test-side checker (`common/checker.rs`, taught v2).

### 10.1 `crates/fitment-catalog` unit and integration tests

| Test | Rules |
| --- | --- |
| `ids_follow_the_alphabet` (property test over arbitrary strings: CI3's mapping gives a valid id or none; valid ids map to themselves) | CI1, CI3 |
| `ids_are_unique_without_case` | CI2 |
| `a_crash_never_leaves_a_row_without_its_blob`: a fault hook (a `cfg(test)` closure the store calls between steps) stops `add`, `replace`, `remove` at each step; after each, open, `verify`: only orphans or temporary files, every row's blob present | ST2, ST3, A5 |
| `writers_wait_then_fail` (two connections, `BEGIN IMMEDIATE`) | ST4 |
| `a_reader_sees_one_snapshot`: a reader transaction open across a `replace` sees the old entry with its old tokens | ST5, A6 |
| `revisions_never_repeat`: add, replace, remove, add again: r1, r2, tombstone r2, r3 | CI6 |
| `stale_is_computed`: a row written with an older version (test constructor) lists as `stale` | RX1 |
| `reindex_resumes`: a fault after the third entry; a second reindex processes only the rest | RX3 |
| `a_newer_schema_is_refused` | ST7 |
| `sql_never_holds_user_text`: ids and refs with `'`, `%`, `_`, `;` find exactly their entry or nothing | ST6, HN6 |

### 10.2 `crates/fitment/tests/catalog_cli.rs`

| Test | What it runs and asserts | Rules |
| --- | --- | --- |
| `init_makes_a_catalog_once` | `init` twice: 0, 0 ("unchanged"); on a non-empty directory: 2; `list` on a plain directory: 2 | 5.1, ST8 |
| `add_reports_each_file_and_exits_by_the_worst` | the prototype's set: `m_bracket.stp`, `copy of m_bracket.stp`, `a b.stp`, `a_b.stp`, a junk file, `r04_assembly.stp`: lines `added`, `duplicate`, `conflict`, `quarantined`; exit 2 (the conflict); again: `unchanged` for the same ids, exit 2; without `a b.stp`: exit 3 (the quarantined files), the duplicate not raising it; `add --id X m_bracket.stp`: duplicate, exit 2 | CI3, CI4, CI5, CC1 |
| `add_of_a_clean_directory_is_idempotent` | the unit parts' accepted files: exit 0 twice; `list` byte-identical after each | CC9 |
| `a_refused_file_is_quarantined_and_listed` | `r03_two_solids.stp`, `r05_no_length_unit.stp`: exit 3; `quarantine` shows each refusal code; `list --status quarantined` | CQ1, 5.9 |
| `get_features_is_what_features_prints` | for each unit part, `get --format features` equals `fitment features FILE` byte for byte | 5.4 |
| `get_step_needs_an_output_and_round_trips` | without `-o`: 2; with: the bytes equal the input; `-o` inside DIR: 2 | HN4, CC7 |
| `replace_keeps_the_id_and_refuses_a_broken_file` | replace with `m_bracket_thick.stp`: r2; with junk: 3, `get` still r2 and its SHA-256; with another entry's bytes: 2 | CD-3, CI5, CI6 |
| `remove_is_all_or_nothing` | `remove a nosuch`: 2, `a` still listed; `remove a`: 0, blob gone; `remove sha256:…`: 2 | CI7, A12 |
| `references_resolve_or_suggest` | unknown id: 2 with suggestions; a 12-hex prefix: found; an 8-hex prefix: 2 | CI7 |
| `reindex_brings_a_stale_catalog_back` | a catalog with a row made stale through the library's test constructor (the CLI test links `fitment-catalog` as a dev-dependency): `match --catalog` exits 3 naming reindex; `reindex` 0; `match` runs | RX1, RX2, RX4 |
| `reindex_says_when_a_part_leaves_the_search` | a row whose stored status is `indexed` for a refused file (test constructor): `reindex` exits 3, stderr "no longer searched" | RX5 |
| `verify_finds_damage_and_repairs_only_orphans` | append to a blob: `damaged`, 3; drop a temporary file: `temporary`; `--repair`: temporary gone, `damaged` remains, 3 | VF1, VF3, VF4 |
| `verify_deep_finds_a_lying_index` | edit a stored features document to add a hole (library test hook): plain `verify` 0, `--deep` 3 `mismatch` | VF2, A7 |
| `hostile_names_are_escaped` | a file named with ESC, newline and U+202E, and a PRODUCT.name with them (an edited unit part): no raw control or bidirectional character on stdout or stderr of `add`, `list`, `get`, `quarantine`, `match --catalog`; JSON still holds the exact strings | CC2, HN2, HN3, A15 |
| `large_files_are_refused_unread` | `--max-mib 1` on a 2 MiB file: `refused`, 3, no blob | ST10 |
| `inputs_inside_the_catalog_are_refused` | `add -C cat cat/blobs/…`: 2 | ST8 |
| `the_catalog_document_is_stable` | `list --format json` twice and on a rebuilt catalog with the same adds in the same order: identical bytes; a golden for the unit set | CC3, CC9 |
| `no_catalog_feature_no_catalog` (`--no-default-features` CI leg only) | `fitment catalog`: 2; `match --catalog`: 2 with CC8's message | CC8 |

### 10.3 `crates/fitment/tests/match_cli.rs`, extended

| Test | What it runs and asserts | Rules |
| --- | --- | --- |
| `a_catalog_run_is_the_explicit_run` | s01's bracket socket; a catalog of `crates/fitment-core/tests/candidates`; for every subset of ids used by the existing tests, `match --catalog --id …` and `match <files>`: same `digest`, same exit, same verdict order; labels are the ids | CM5, CM10, CM11 |
| `a_catalog_run_reads_no_step_file` | delete `DIR/blobs`; a full run gives the same document as before; `verify` reports `missing` | CM1 |
| `the_search_that_cannot_run_exits_3` | stale (test constructor), all quarantined, a blocked socket (s01's socket with a requirement made unverifiable by an edited unit assembly, as KC's `b_unverifiable`), a newer schema: each exit 3, empty stdout, its message; the same blocked socket with `--id`: candidates judged, exit 5 | CM7, CD-7, A10, A11 |
| `catalog_inputs_are_exclusive` | files with `--catalog`; `--id` without it; `--id` with `--exhaustive`: 2 | CM2 |
| `a_named_quarantined_part_is_a_refused_candidate` | `--id r04_assembly`: `Reject`, `candidate_refused`, the same entry as `match S r04_assembly.stp` | CQ3 |
| `exhaustive_and_full_runs_pass_the_same_parts` | on the candidates' catalog: equal sets of non-rejected SHA-256s and equal entries for them | CM6, A9 |
| `the_catalog_table_names_ids_and_counts_quarantine` | table holds the `catalog generation` line, `id rN`, at most 10 rejects then "… N more"; `--rejects all` lists all | CM8, CM13 |
| `the_original_supplies_properties` | s01 with `--original`: `properties.material.reason` is `original_states_none`; without it `original_unknown` | PR6, CM14 |
| `thread_requirements_are_named` | s03's PLATE socket (`s03_dowels.stp`) against `m_plate03.stp` (the original plate in another frame; the candidates' manifest expects `interface_match` with `thread_not_confirmed`): `properties.thread.requirements` lists exactly the passed requirements carrying the socket's flag, and the table prints one "thread not confirmed at" line each | PR10, VR19 |

### 10.4 Properties (`crates/fitment-core/tests/properties.rs` and `crates/fitment/tests/fixtures.rs`)

| Test | Asserts | Rules |
| --- | --- | --- |
| `every_mapping_row_has_its_sample` | for each row of `PropertyMap::shipped()`, its cited fixture and record give that row's field and meaning (skipped with a note without fixtures, required in CI) | PR4 |
| `the_nx_material_flag_reads_as_unassigned` | E1, E2: `unassigned`, with `#1095` and `#7270` | 6.6 |
| `density_and_notes_are_not_material` | E3 (`#8518`), E4 (`#18079`): no field mapped | PR9, 6.6 |
| `capture_is_bounded` | E9 (440 values) captured in full; a generated file with 2,000 properties: `truncated`, no comparison | PR2, A18 |
| `only_the_part_s_own_properties` | a unit part edited to carry a property on a shape aspect and one on a parent assembly: neither captured | PR2, A19 |
| `comparison_is_exact_and_explained` | with a test-only row: `Steel`/` steel `: equal; `Steel`/`Steel 1020`: differs, flag `material_differs`, kind unchanged; one side two values: `several_stated` | PR5, PR8 |
| `no_thread_is_confirmed` | E5, E6 read as components and as candidates: `thread_not_confirmed` wherever VR19 sets it; PRODUCT.name `M6x20` changes nothing | PR7, PR9 |

### 10.5 The corpus through a catalog (`crates/fitment/tests/corpus.rs`, PR 6)

- **`the_corpus_judged_from_a_catalog`:** every candidate of the 125 match cases added to one catalog (the generated tree, as `corpus.rs` builds it); each case run with `--id` under both policies: the same digest and outcome as the explicit run; the strict gate per case (KH) and zero false positives, read from the catalog runs.
- **`full_runs_keep_what_passes`:** for each base socket, a full run and an `--exhaustive` run: equal passing sets (SL9 through the catalog); the level-1 counts reported, as M4's section 20.1 did.
- **`a_hostile_catalog_is_bounded`:** 10,000 entries (the bracket family, copies with one coordinate perturbed so each SHA-256 differs): `add` of the directory, a full run, `list`, `verify` each within a stated time bound (30 s for the run, as `hostile.rs`); memory of the run bounded by one candidate's features (CM3).

## 11. PR plan

Each PR: tests, two adversarial reviews, `MATCH_RULES_VERSION` or a version raised where output changes.

| PR | Contents | Done when |
| --- | --- | --- |
| 1 | **The store.** `fitment-catalog` crate (lib, sync; rusqlite bundled; `fitment_core::sha256_hex` reused, so no `sha2`); schema v1; blobs (ST1 to ST3, ST9); locking and snapshots (ST4 to ST6); ids, references, generation, history, tombstones (CI1, CI2, CI6, CI7); CLI `catalog init`, `list`, `get` (table, json); the `catalog` feature on by default and the `--no-default-features` clippy leg; deny allowlist and licence review (rusqlite, libsqlite3-sys) | 10.1's store tests; `init`, `list`, `get` CLI tests on a library-seeded catalog |
| 2 | **Ingest and quarantine.** `add` (CI3 to CI5, CI8, ST8, ST10; gate, features, tokens), `quarantine`, `get --format features|step`; core: `judge_document`, the source constructor, `IndexTokens` storable | `add`/`quarantine`/`get` CLI tests; the unit set and fixtures added with the manifest's pinned gate results |
| 3 | **Change and repair.** `replace`, `remove`, `reindex`, `verify` (`--deep`, `--repair`); staleness (RX); crash tests | 10.1's crash and resume tests; 10.2's replace, remove, reindex and verify tests |
| 4 | **The catalog match.** `match --catalog` (CM1 to CM13); level 1 from stored tokens; `fitment.match` v2 (CD-6, CM9, CM10) and the checker; exit 3's new causes; the table | 10.3 but properties; CHANGELOG entry for v2 |
| 5 | **Properties (D10).** Core capture and `PROPERTIES_VERSION`; the mapping (6.6); `properties` record, `material_differs`, PR10's thread list; `--original` properties | 10.4 and 10.3's property tests; PLAN's D10 row marked decided with section 6's evidence |
| 6 | **The corpus as a catalog, and the exit.** 10.5; ARCHITECTURE ("The catalog" section: the flow add → gate → store → index → match, the CQ/RX/CM rules' homes), PLAN (M5 done, Components row, Resume here), README (a catalog walkthrough, VF5's note on copied catalogs, R5's on synced folders); release 0.2.0 with `fitment-catalog` published as internal and unstable | M5's exit: ingest, list, get, replace and delete from the CLI; a match reads only the index (CM1's test); zero false positives through the catalog |

## 12. Risks

| # | Risk | Mitigation |
| --- | --- | --- |
| R1 | `fitment.match` v2 days after v1 froze | One additive bump, every field null on explicit runs, a CHANGELOG entry, the checker reading both; or CD-6's alternative |
| R2 | D10 cannot grow without user samples: the fixtures hold no positive material and no linkable thread | The machinery and its tests ship; each row waits for a real file (PR4). Ask early users for exports from SolidWorks, Creo, Inventor and NX with materials assigned; the AP242 `MATERIAL_DESIGNATION` practice is the first candidate |
| R3 | A copied catalog is trusted blindly (A7) | VF5 and `--deep`; documentation |
| R4 | Unselective sockets on large catalogs: a full run of tens of thousands of judgments, a document of tens of MB | CD-12's table; MF3's one-at-a-time memory; M6's jobs. The document stays complete |
| R5 | SQLite in synced or network folders | Documented; `verify` |
| R6 | A reindex of 10⁵ parts takes hours and blocks catalog runs meanwhile (RX2) | RX3: resumable, per entry; the refusal message says a reindex may be running; M6 runs it as a job |
| R7 | Default ids surprise users (mapping, CI4's order) | Every mapped id printed on its line; `--id` for single files; conflicts never overwrite |
| R8 | CAD default materials compared as equal once positive rows exist | PR4 requires each exporter's default values from samples before a row ships; PR5 prints both values |
| R9 | The obsolete part left in the catalog is recommended | Documentation (A25) |
| R10 | Bundled SQLite lengthens `cargo install` (decision 10 of the layout accepted it) | None needed |

## 13. Changes to PLAN.md and ARCHITECTURE.md

- **PLAN.md:** D10 decided, with section 6.3's table in brief and its consequence (no material or thread confirmed from properties in M5; the mapping ships one row). CD-1 to CD-14 as M5's decisions. Exit codes: 3 also "the search could not run". "Metadata from STEP only": the evidence that a tapped M12 hole is modeled as a Ø10.2 plain hole (E5), sharpening the stated limit.
- **ARCHITECTURE.md:** `fitment-catalog` in the crate map (`fitment (CLI) → fitment-catalog → fitment-core`); a "The catalog" section; `fitment.catalog` v1 and `fitment.match` v2 under "Formats and versions"; `PROPERTIES_VERSION`; the catalog's soundness points (stale refused, quarantine never searched, the index trusted only after `verify --deep` when copied); the test table's new rows.
