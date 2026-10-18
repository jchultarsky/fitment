# fitment M6: the REST API

As of 2026-10-17. Status: **proposal for review**, merged from three designs under the coordinator's decisions. Nothing here is implemented; section 12 is the order of work. The rest of the document assumes every default in section 2.

M6 delivers `fitment-server`, a REST API with jobs over the catalog and the matcher. PLAN's exit for M6: "The M4 corpus passes end to end through the API." This document is that design.

Three agents designed one area each, to one brief:

- **A:** the API surface: resources, operations, documents, errors, uploads, versioning, and the soundness rules a client relies on.
- **B:** the server's internals: the crate and its dependencies, the job system, how the server shares the catalog, stored objects and uploads, authentication and hardening, configuration and deployment, shutdown and recovery, determinism.
- **C:** verification: what the exit sentence means as a test, the test layers a server needs, the scorer the suites share, the coverage registry, CI and the PR plan.

Where and how they worked:

- **A:** read the code at `m5/release` (`bbc89c0`: fitment 0.2.0, `EXTRACTOR_VERSION` 6, `SOCKET_RULES_VERSION` 7, `SIGNATURE_VERSION` 2, `MATCH_RULES_VERSION` 1, catalog schema 1): `crates/fitment/src/cmd` and the public APIs of `fitment-core` and `fitment-catalog`. No prototype.
- **B:** a throwaway prototype (`m6-proto/`) with path dependencies on the branch's `fitment-core` and `fitment-catalog`, built with `cargo +1.85 build --release`, checked with `cargo deny` under the repository's `deny.toml`, and smoke-tested (section 8.1).
- **C:** timed the CLI work the exit mirrors with `measure.py`, release build, on an Apple M5 Max (18 cores), over `corpus/out` (462 STEP files, 125 match cases on 33 socket keys) (section 8.2).

The coordinator settled forty-eight points before the merge (M6-1 to M6-48). The merge settled the rest where the designs disagreed or left a gap (M6-49 to M6-57). Section 15.2 lists what the merge found, and section 16 what each designer lost and why. A soundness review then attacked the merged design; the coordinator resolved its twenty-five findings, which changed the rules, decisions (M6-58 onward, and amendments marked **Review:**), tests and plan below, and section 15.3 lists each finding with its fix. Section 17 records the review's questions as answered.

**Rule ids.** Rules are numbered by area, from 1 without gaps, and each rule names its origin in brackets, as (A RA4) or (B JB13, A JB8). Where two or three designers stated one rule, it is stated once and cites them all. Test ids (VT) are not rule ids and are never written in the bold rule form; decision ids (M6-n) do not fit the rule pattern. Every prefix below was checked against the repository (`git grep -nE '\*\*PREFIX[0-9]+\.'` over `docs`, `crates` and `tools`, and the same prefix anywhere as a word) and is unused, and each fits the coverage test's pattern (`[A-Z]{1,2}[0-9]+[a-z]?`, KH14). No id has a letter suffix, so `RULES` lists each by its own key.

| Prefix | Area | Section | From |
| --- | --- | --- | --- |
| RA | resources and references | 4.1 | A RA |
| JB | jobs as a client sees them: kinds, states, exit codes, stops, progress, idempotency | 4.2 | A JB, B JB1 to JB4, JB12 to JB16 |
| JS | the job system: lanes, permits, fairness, running, sweeps, event channels | 4.3 | B JB5 to JB15 |
| DC | documents and parity | 4.4 | A DC, B DT |
| UP | uploads | 4.5 | A UP, B OB4, OB7 to OB10 |
| OB | the server's state: `server.db` and the object store | 4.6 | B OB1 to OB3, OB6, OB9 |
| ER | errors | 4.7 | A ER, B HX7, CX7 |
| VN | versions, OpenAPI, pages and caching | 4.8 | A VN |
| SN | soundness through the API | 4.9 | A SN, B DT3, DT6 |
| CX | the catalog in the server | 4.10 | B CX |
| AU | authentication and roles | 4.11 | B AU, A 12.3 |
| HX | HTTP hardening | 4.12 | B HX |
| SV | the crate and its dependencies | 4.13 | B SV |
| OP | logging, configuration, operations, deployment | 4.14 | B OP |
| SD | shutdown and recovery | 4.15 | B SD |
| CO | changes to `fitment-core` | 5.1 | B CO, A 15 |
| CA | changes to `fitment-catalog` | 5.2 | B CA, A 14, 15 |
| CU | changes to the CLI | 5.3 | A 15 |
| XH | the exit and parity harness | 9.1 | C XH |
| SC | the scorer, `fitment-corpus` | 9.2 | C SC |

Prefixes not taken, and why:

- **AP** was the brief's suggestion for the API. `AP203`, `AP214` and `AP242` already appear 148 times in the repository as STEP application protocols, and the pattern would read a bolded one as a rule.
- **JB** was used by both A and B. A's family (what a client sees) keeps JB; B's scheduling rules become JS. Rules both stated (states, cancellation, idempotency, progress) are stated once, in JB.
- **DT** (B's determinism) is folded into DC and SN: each DT rule restated a DC or SN rule from the other side.
- **AS** (C's assumptions about A and B) is not a rule family: the merge settles each assumption (section 9.3), and `AS1` is already the name of a fixture assembly in PLAN.

**Evidence.** The designers' reports (`m6-design/A.md`, `B.md`, `C.md`), B's prototype (`m6-proto/`) and C's `measure.py` are in the session's scratchpad, not committed; they are cited by name, and their measurements are restated in section 8.

## 1. Summary

- **One rule decides sync or job.** An operation that parses STEP bytes, judges a candidate or scans the whole catalog runs as a job; everything else is synchronous. A client that wants a short job to feel synchronous sends `Prefer: wait=N` (N ≤ 60) and gets the finished job in the same response (M6-1, JB1, JB11).
- **The CLI's documents, byte for byte.** Every response that carries a `features.v1`, `fitment.parts`, `fitment.socket`, `fitment.match`, `fitment.catalog`, `fitment.verify` or `fitment.gc` document carries exactly the bytes the matching CLI command prints for the same inputs and labels, with no envelope and no re-serialization, and a `Fitment-Exit-Code` header holding the code the CLI would exit with. What the CLI writes on stderr becomes structured `notes` on the job. Parity is then a byte comparison (DC1, DC3, JB10).
- **Bytes enter one way.** STEP files and socket documents are uploaded to `/v1/uploads/sha256:HEX`, hashed as they stream (an incremental SHA-256 in core, CO1), capped at 128 MiB by default, and named by hash in every later request. A hash grants nothing: it resolves only among the caller's own uploads (M6-15, UP1 to UP3).
- **One process, one catalog, one state directory.** The catalog is an M5 catalog, unchanged at schema 1, which the 0.2.0 CLI can share with the server. Jobs, run records, sockets, assemblies, uploads, users and tokens live in the server's own `server.db` beside a content-addressed object store; m5-catalog.md's section 6.3 reservation is withdrawn (M6-3, OB1).
- **Revisions, not ETags, guard writes.** Catalog entries keep M5's strict `ID@rN` (ID9). Sockets get the same form: a line `sk_…` per (assembly, target, occurrence) in its owner's namespace, with revisions `@r1`, `@r2`, one per extraction, import or review that changes the bytes; a review must name its `@rN` (M6-2, RA4, RA5, RA7).
- **Jobs are rows in `server.db`, run in three lanes over one CPU budget.** States `queued`, `running`, `cancelling`, then `succeeded`, `refused`, `failed`, `cancelled` or `interrupted`; the state follows the exit code the CLI would give. Query jobs and write jobs never wait for each other; a fair semaphore keeps the server's compute inside `cpu` threads; each owner has caps and owners are served round-robin (JB3, JS1 to JS5).
- **Cancellation is cooperative and never yields a partial verdict.** A new std-only `Observer` in `fitment-catalog` (CA2) is asked between files, batches and groups. A cancelled, interrupted or timed-out match leaves no document; a cancelled ingest keeps and reports what it committed (JB5, SN7).
- **Nothing the CLI refuses passes.** Stale sockets, stale or empty catalogs, blocked sockets in a full run, unreviewed sockets under `require_reviewed` and quarantined entries in a full run are refused as the CLI refuses them, with no override. No endpoint accepts a `fitment.match` as input; `Match` needs a verified fit, and the server judges with `NoFitVerifier` (SN2, SN6, SN11).
- **One error registry.** About sixty stable `snake_case` codes, each with a class (`usage`, `refused`, `error`, `auth`), the CLI exit code of that class, an HTTP status and a `retryable` flag, sent as `application/problem+json`. No message comes from a path or a library error's `Display` (M6-4, ER1 to ER7).
- **Auth is Vectera's, ported, with the audit's five flaws and six more fixed;** bearer tokens only, roles reader, writer and admin, the first admin created offline; a token mints only within itself, and caps are per user (M6-17 to M6-19, M6-58, M6-61, AU1 to AU10).
- **Plain HTTP/1.1 behind a TLS proxy,** header-read timeouts, a connection cap, per-route body limits, caps on costly synchronous work, no HTTP/2, no CORS by default (HX1 to HX8).
- **Changes to the existing crates, each small and synchronous:** bytes-based ingest and replace with per-file ids and display names (CA1), the `Observer` (CA2), WAL pressure (CA3), verify's paths made relative and error lines built from the error's kind (CA4), `fitment.verify` and `fitment.gc` documents (CA5), a streaming writer and reader in `Fs` (CA7), a catalog instance id (CA8) and the CLI's `--format json` for `delete`, `prune`, `verify` and `gc` (CU1, CU2); an incremental SHA-256 (CO1), the run-outcome helpers moved to core (CO2), duplicate components merged (CO3), `judge_component` (CO5); schemars derives behind each crate's `schema` feature (CO4, CA6) (M6-48).
- **The exit is a parity test and a truth test, on every case.** For every match case of the corpus, under `default` and `functional`, the built `fitment-server` binary is driven over loopback HTTP: upload, register, extract, review, match. The API's documents must equal the CLI's byte for byte (or the library's, for component candidates), and every candidate must pass M4's strict gate with no false positive and the false-negative set equal to the baseline. The catalog half ingests the corpus through the API and requires every full, exhaustive and named run to equal the CLI's on the server's own catalog (XH1 to XH14).
- **`fitment-corpus` is the one scorer** (SC1): the strict gate, the rates, the baseline and the document checker move out of three test files into a `publish = false` crate that scores an `Outcome` built from a library result or from a document entry.
- **What it costs.** The CLI work the exit mirrors takes about 20 s on the Mac (section 8.2). The API adds HTTP and job overhead; the budget is 5 minutes in CI's new `server-corpus` job.
- **Ten PRs:** the scorer and the registry first, with no server code; the exit's file half in PR 6, its catalog half in PR 7, the exit in PR 9 and 0.3.0 in PR 10. `fitment-server` is not published; it ships as a container image (M6-35).

## 2. Decisions for review

Each row has a default. Rows marked **Your call.** change what fitment promises, stores or prints; the rest are engineering defaults. M6-1 to M6-48 were settled by the coordinator (each names the designer whose default it took). M6-49 to M6-57 were settled by the merge, where the designs disagreed or left a gap; section 15.2 gives each finding. M6-58 to M6-64, and the amendments marked **Review:** in earlier rows, apply the soundness review's findings as the coordinator resolved them; section 15.3 gives each.

| # | Question | Default | What it costs or buys | Alternatives, and why not |
| --- | --- | --- | --- | --- |
| M6-1 | **Your call.** Sync or job | **A job** for every operation that parses STEP bytes, judges a candidate or scans the whole catalog: `features`, `assembly_read`, `socket_extract`, `match`, `catalog_ingest`, `catalog_replace`, `catalog_reindex`, `catalog_verify`, `catalog_gc`, `catalog_prune`. **Synchronous:** everything else. **`Prefer: wait=N`**, N ≤ 60, returns the finished job in the same response (RFC 7240) (A A1; JB1, JB11). **Review:** a socket import re-extracts from the caller's assembly (M6-62), so it parses STEP bytes and is the job `socket_import` (review F12) | **Buys:** one mechanism for cancellation, progress, idempotency and audit; no request held open for minutes unless the client asks. **Costs:** a short job costs a second round trip unless the client waits | (a) Sync for single-file operations, jobs for batches: two code paths per operation, and one large file still ties up a request. (b) Everything a job, reads included: needless round trips. (c) B's every catalog write a job, one `delete` included: replaced by M6-50 |
| M6-2 | **Your call.** Socket identity | **A socket line** per (assembly SHA-256, target definition `#id`, occurrence path or all), in its owner's namespace; id `sk_` and 20 hex of a SHA-256 over that triple. **Revisions** `@r1`, `@r2`, … as ID9's grammar, one per extraction, import or review that changes the document's bytes, never reused. **A review must name `@rN`** (A A2; RA4, RA5, RA7). **Review:** extracting onto a deleted line's triple revives the line: a new live row under the same id, its revisions numbered on from the deleted row's, so no `@rN` ever names two revisions (RA4, review F20) | **Buys:** extraction is idempotent; the same data gives the same ids on every server; carry-over keys on what the CLI's `foreign()` compares; two reviewers of one socket cannot lose a decision silently. **Costs:** an id is known only after the first extraction (question 1) | (a) An opaque server counter, or B's random row ids: ids differ between servers holding the same data. (b) Content-addressed revisions only: no stable handle for "this part's socket in this assembly" to review, carry over and re-extract onto |
| M6-3 | **Your call.** Where the server's state lives | **`server.db`**, a separate SQLite database in the server's state directory, beside a content-addressed object store: users, tokens, uploads, assemblies, socket lines and revisions, jobs and the run records they hold (B M6B-1; OB1). **The catalog stays at schema 1;** m5-catalog.md section 6.3's reservation (assemblies as `kind`ed blobs, `socket` and `report` tables) is withdrawn | **Buys:** the catalog's invariants (IN1 to IN9, ST5, gc, verify) untouched; the 0.2.0 CLI and the server share a catalog; job churn never raises the catalog's generation (DB8), never queues on its write lock and never grows its WAL. **Costs:** two databases to back up; a run's catalog generation is a recorded number, not a foreign key | (a) 6.3's migration: sockets and reports in `catalog.db` as schema 2, which 0.2.0 refuses (DB5), with IN3, gc and verify extended to socket-held blobs; C's assumption 8 took this. (b) Jobs in memory: no restart recovery and no audit |
| M6-4 | **Your call.** Errors | **`application/problem+json`** (RFC 9457) whose extension members are `format: "fitment.error"`, `format_version`, `code`, `class`, `exit_code`, `retryable`, `details` and `request_id`; one closed enum of stable codes (`ErrorCode::ALL`); messages built from codes and API names, never from a library error's `Display` or a path (A A3; ER1 to ER7) | **Buys:** standard tooling reads the body; fitment's codes stay stable; nothing of the server's file system reaches a client | A bare `fitment.error` as `application/json`: no standard reader. B's codes on their own: merged into one registry (section 7.12) |
| M6-5 | **Your call.** Component candidates and originals | **Required.** `candidates.components` names leaves of the caller's stored assemblies, judged by `Matcher::judge_part` with `CandidateSource::Component`; `original: true` judges the socket's own target (A A4, C C-3; SN9). **Review:** judged by a new core call, `Matcher::judge_component(&ComponentRead, source)`, so the server never reads or selects a component itself; a selection in a refused assembly is refused (exit 3, `component_refused`); an unreadable original is `run_incomplete`, exit 1 (CO5, review F13) | **Buys:** all 33 corpus sockets reach the API, not 23; M4-14's promise ("through the API also a leaf of an assembly") kept. **Costs:** a request shape, and a library reference for parity (DC3) | List the 10 component cases as library-only at the exit, as M5 did for the catalog: the exit sentence says the corpus, not most of it |
| M6-6 | **Your call.** OpenAPI and the documents' schemas | **schemars derives** on fitment-core's and fitment-catalog's document types behind each crate's `schema` feature (decision 11's plan, never built), and on the server's own types behind its own `schema` feature. **A test** assembles OpenAPI 3.1 from them and the server's operation table and compares it with the committed `crates/fitment-server/openapi.json`; **the server serves those bytes**, compiled in. schemars and serde_json's `alloc` and `raw_value` never enter the server's runtime graph (A A5, B M6B-13; VN7, SV9). If cargo-deny's feature check sees `alloc` (its `all-features` graph turns `schema` on), `deny.toml` allows it with a stated reason | **Buys:** every API change is a reviewed diff; a schema that drifts from its serializer fails a test; the runtime JSON writer is the CLI's. **Costs:** derives on two published crates; a JSON Schema validator in tests (question 9) | (a) aide generating OpenAPI at runtime from handler types: schemars and `alloc` in the runtime graph. (b) Hand-written schemas, tested the same way: more to keep in step. (c) Opaque `{format: const}` schemas: no description of the frozen documents |
| M6-7 | Catalogs per server | **One,** at `/v1/catalog`, opened at start (A A6; RA1) | **Buys:** ids, labels and generations each name one catalog (M5-44). **Costs:** a second catalog is a second server | `/v1/catalogs/{name}/…`; v2 can add names |
| M6-8 | Large match documents | **The document whole and exact** at `…/document`; **a paged view** of its candidates at `…/candidates`: re-serialized entries marked as a view, each page stating the document's digest (A A7; VN3). **Review:** no entry is re-serialized and no document re-parsed: each page is sliced from the stored bytes through a byte-offset index built once at the job's end, and states the run's `complete` and `exit_code` (DC12, review F2, review F3) | **Buys:** M5 measured 111 MB for 9,219 candidates (M5's R3): a UI pages, the parity test compares bytes | Only the whole document. Pages of the document itself, which break its digest and frozen format |
| M6-9 | Progress | **NDJSON** at `/v1/jobs/{id}/events`, resumable by `?after=SEQ` for every persisted event (A A8; JB8, JS10) | **Buys:** `curl` and every language read it; workspace-layout and the Vectera audit chose it | Server-sent events: nothing a resumable `after` lacks |
| M6-10 | Envelope documents within v1 | **The API's own documents** (`fitment.job`, `fitment.run`, `fitment.page`, `fitment.error`, `fitment.info`, `fitment.upload`, `fitment.assembly`, `fitment.token`, `fitment.user`, `fitment.job.event`) may gain optional fields within v1; clients ignore unknown fields. **Result documents** never change through the API (A A9; VN1, DC9) | **Buys:** operational records can grow (M7 adds the fit verifier's facts); result documents keep their digests and bytes | Freeze them all, with a format version rise per field |
| M6-11 | Time in records | **RFC 3339 UTC times only in operational records:** `fitment.job`'s `times`, and (merge) a token's `created`, `expires`, `last_used` and `revoked` in `fitment.token`. **No** result document, `fitment.run`, `fitment.assembly` or listing of revisions holds a time (A A10; DC2, DC9) | **Buys:** people see when a job ran and when a token expires; two servers' run records of one run compare equal but for ids | No times anywhere, only sequence numbers: a token's expiry would be unreadable |
| M6-12 | `require_reviewed` | **A request option, default `false`** as in the CLI; the operator may set a floor that forces it `true`, never `false`; what was forced is recorded in the document's `options` and `fitment.run`'s `require_reviewed_by` (A A11; SN3) | **Buys:** parity with the CLI's default; a team that wants every socket reviewed sets it once | Default `true` in the API: every parity run would need the option |
| M6-13 | A result cache | **None.** `Idempotency-Key`, bound to the SHA-256 of the canonical request, covers retries (A A12, B JB13; JB9). **Review:** the key's fingerprint is over the request as sent, syntax-normalized, never over resolved references; a key is released when its job ended `interrupted`, `cancelled`, or `failed` with a retryable code; a replay answers the job's current status code (review F19) | **Buys:** no verdict served from a cache keyed too narrowly (GT9, RX10) | Cache file runs by (socket bytes, candidate bytes, labels, policy, options, versions): MF7 makes it sound for files, but catalog runs depend on the generation, and nothing measured says it is needed |
| M6-14 | Socket files the CLI wrote | **`POST /v1/sockets/import`** takes an uploaded `fitment.socket` (strict `from_json`, then MF2) as a new revision of its line (A A14; UP6). **Review:** a job, `socket_import`, which re-extracts from the caller's assembly and requires an equal digest, or stores the revision `verified: false` (M6-62, RA10) | **Buys:** users keep their reviewed files (M5-17's cost); the import runs the reader `fitment match` runs | Sockets only from server-side extraction: every reviewed file reviewed again |
| M6-15 | **Your call.** Who can reach uploaded bytes | **Per user** (B OB4): an object is reachable only through the caller's own upload, assembly or socket rows; a hash grants nothing; `HEAD` reveals presence only among the caller's own uploads; bytes another user already stored are answered exactly as new bytes. **Content-addressed,** hashed as they stream (CO1), **capped** at 128 MiB by default, SZ1's cap (UP2, UP3) | **Buys:** a server with several users never answers "has anyone uploaded this file?"; dedup costs nothing extra in a content-addressed store. **Costs:** a user re-uploads bytes another user holds | A13: every upload visible to every reader, `HEAD` a server-wide oracle. Single-tenant was A's argument; B showed the oracle costs confidentiality for no gain |
| M6-16 | The upload hash | **An incremental SHA-256 in `fitment-core`** (CO1) over the existing compression function, so every content name in fitment comes from one implementation (B M6B-12) | **Costs:** a small core change | `sha2` 0.10.9 in the server: a second SHA-256. `sha2` 0.11 needs Rust 1.88 |
| M6-17 | **Your call.** Roles | **reader, writer, admin** with B's scopes (AU5, and section 7's Role columns): a reader reads the catalog and works on their own uploads, assemblies, sockets, matches, jobs and tokens; a writer also changes the catalog; an admin also maintains it and manages users. **Tokens** may carry a lower role than their user (B M6B-4; AU5). **Review:** an exhaustive catalog run, a catalog-wide reindex and `verify` (plain or `deep`) need the writer role, since their cost scales with the whole catalog (M6-60, review F3) | **Buys:** read-only CI tokens; Vectera's three levels kept. With per-user resources, a reader's work touches only their own | A's viewer, editor, admin, where a viewer could not upload or run a match: with M6-15 a reader's uploads harm no one. A role per endpoint |
| M6-18 | **Your call.** Credentials | **Bearer tokens only.** A password exists only to mint a token (`POST /v1/auth/login` returns one); no cookies and no sessions until M8's viewer (B M6B-5; AU3) | **Buys:** no CSRF surface, no session table, one credential path. **Costs:** M8 adds sessions | Vectera's cookie sessions now (A 12.3, C's assumption 11) |
| M6-19 | The first admin | **`fitment-server admin bootstrap --state DIR --username NAME`**, offline, while the server runs or not (WAL); it prints a fresh admin token to stdout once; `--password-stdin` sets a password too (B M6B-6; AU9) | **Buys:** no secret in the environment or a log (audit flaw 3). **Costs:** one manual step per deployment | `FITMENT_SERVER_BOOTSTRAP_TOKEN_SHA256`, the hash of a token the operator made, read at first start: suits Kubernetes; left for a later release |
| M6-20 | TLS | **At a reverse proxy;** the binary speaks plain HTTP/1.1 and binds `127.0.0.1:8080` by default (B M6B-7; HX1) | **Buys:** no rustls, ring or aws-lc-rs in the graph (aws-lc-rs's OpenSSL licence fails `deny.toml`). **Costs:** a deployment adds a proxy | A `tls` feature with rustls and ring (`Apache-2.0 AND ISC`, to be checked) |
| M6-21 | **Your call.** Job states | **B's states** with A's meanings: `queued`, `running`, `cancelling`; terminal `succeeded` (exit 0, 4 or 5), `refused` (exit 3, and by the merge exit 2: M6-52), `failed` (exit 1, or no answer), `cancelled`, `interrupted` (JB3) | **Buys:** "the input was refused" and "the server could not do it" are distinct states, as exit 3 and exit 1 are | A's `queued`, `running`, `succeeded`, `failed`, `canceled`, where `succeeded` carried exit 2 and 3 |
| M6-22 | **Your call.** Jobs running at a restart or shutdown | **Pure jobs** (`features`, `assembly_read`, `socket_extract`, `socket_import`, `match`, `catalog_verify`) re-queued once (`attempt` ≤ 2); **write jobs** become `interrupted`, and the client submits again (B M6B-8; JB6, SD4, SD5). **Review:** a graceful drain re-queues a pure job without consuming an `attempt`; after a hard kill a write job carries no document, its details give the catalog's generation at its start and at recovery, and an interrupted replace or prune is not retryable: the client reads the entry or the catalog first (review F10; question 5 of the review) | **Buys:** a deploy loses no query; a crash-looping input cannot loop forever; a re-run is a new run in a new snapshot, never a resumed one. **Costs:** an interrupted ingest's report covers only its committed batches | A's: every running job `interrupted`. Re-queue everything: an ingest's second run reports `unchanged` where the first added |
| M6-23 | **Your call.** Retention | **Terminal jobs and their run records:** 30 days unless pinned by an admin. **Uploads:** 24 h unless an assembly, a socket revision or a live job holds them. **Assemblies and socket lines:** until their owner deletes them; a deleted line's revisions stay while a retained run cites them. **Tokens:** removed 30 days after expiry or revocation (B M6B-9; JB12, OB4). **Review:** one hold window everywhere: a job's inputs and cited revisions are held from its submission until it is terminal, and a match's for as long as its record is retained (OB4, M6-64, review F20) | **Buys:** bounded disk (a 9,219-candidate document was 111 MB in M5). **Costs:** an unpinned run record expires; PLAN's "audited" means pinned (question 5) | A's default: run records and socket revisions forever. Per-user quotas only |
| M6-24 | Level 1 in the server (M5-17, question 4) | **Read from SQL per run,** in the run's own snapshot, as the CLI does (B M6B-10; CX2) | **Buys:** QR2's one snapshot holds; nothing to keep in step with writers. Level 1 took 0.2 to 45 ms at 10⁵ parts (M5, B) | An in-memory index rebuilt on each generation: 1.5 to 3.6 times slower than SQL at 10⁵ when built per query (M5) |
| M6-25 | A run's catalog context (M5-17, question 6) | **Recorded with the run, not in the document:** generation, entries, searched, quarantined and not searched, level-1 survivors, selection kind (`CatalogRun`'s fields, CM8) in the job row, shown in `fitment.run` (B M6B-11, A DC6; DC10) | **Buys:** a run record says how much was searched while `fitment.match` v1 stays frozen (M5-2) | A `fitment.match` v2 |
| M6-26 | Lanes, permits, fairness | **Three lanes** (query, write, maintenance) over **one fair CPU semaphore**, a heuristic memory semaphore, per-owner caps and round-robin across owners, bounded queues (B JB5 to JB9; JS1 to JS5). **Review:** `gc --vacuum` runs on the write lane, leaving `verify` alone on the maintenance lane; a catalog match's memory is estimated per selected entry (JS1, JS3, review F3, review F23) | **Buys:** a long ingest never delays a query; the catalog's scoped threads stay inside the budget; one user cannot hold the server | One FIFO queue; a thread pool per kind |
| M6-27 | How a running job stops | **Cooperatively,** through one flag the job's own loop and the std-only `Observer` (CA2) read between files, batches, windows and groups (B; JB5, JS8) | **Buys:** no partial write, no thread killed mid-transaction. **Costs:** a socket extraction or one candidate's judgment stops only when its bounded core call returns | Killing threads (unsound in Rust); checking only between file candidates, which leaves catalog runs and ingests uncancellable |
| M6-28 | Reindex after an upgrade | **`jobs.reindex_on_start = true`:** at boot, a stale catalog (RX2) gets a reindex job on the write lane; matches are refused as stale (RX3) until it ends, and `fitment.info` says so (B M6B-14; CX5) | **Buys:** a container upgrade (pull, restart) needs no CLI step | Off by default, an admin submits it |
| M6-29 | Metrics | **Prometheus text written by hand** from atomics, on a separate listener, off by default (B M6B-15; OP6) | **Buys:** no metrics crates; not reachable from the API port | On the API port behind the admin role; `metrics` and `metrics-exporter-prometheus` |
| M6-30 | Rate limits | **In-house token buckets,** about 80 lines (B M6B-16; AU7) | **Buys:** no `governor` (MSRV undeclared, dashmap) | `tower_governor` 0.8 |
| M6-31 | The CLI in the image | **Included,** same version: `docker exec … fitment catalog verify -C /var/lib/fitment/catalog` (B M6B-18; OP8) | **Costs:** about 10 MB. **Buys:** maintenance without a second image | The server alone |
| M6-32 | SQLite for `server.db` | **rusqlite in `fitment-server`;** `deny.toml`'s wrappers become `["fitment-catalog", "fitment-server"]`, and the server opens `catalog.db` only through `fitment-catalog` (B M6B-2; SV6) | **Buys:** one engine, already vetted. **Costs:** DB1's "only the catalog touches SQLite" becomes "only the catalog and the server's own state" | A `fitment-catalog::state` module, which puts server concerns in a published crate; a new `fitment-state` crate |
| M6-33 | Run logic the CLI and the server share | **Pure helpers move to `fitment-core`** (CO2): the "this result is a fitment bug" test (M4-17), the original's bug test and the outcome-to-exit-code mapping (VD14); each caller keeps its own threading (B M6B-3) | **Buys:** one definition of what makes a run exit 1 or 3. **Costs:** the CLI's file-run windowing stays duplicated, about 60 lines | A `fitment` library target the server depends on (clap in the server, a published API grows); a `fitment-run` crate |
| M6-34 | JSON in handlers | **The server's own `Json` extractor and responder** (about 40 lines over `serde_json::from_slice` and `to_vec`); axum's `json` feature off (B M6B-13; SV9) | **Buys:** serde_json's exact features unchanged in the runtime graph; the prototype showed axum's `json` adds `raw_value` | Allow `raw_value` in `deny.toml` with a reason |
| M6-35 | **Your call.** Publishing | **`fitment-server`: `publish = false`, `dist = false`,** shipped only as a container image. The workspace releases **0.3.0** at M6's exit for the published crates (`fitment-core`, `fitment-catalog`, `fitment`) (B M6B-17; OP8) | **Buys:** no public API to freeze. **Costs:** no `cargo install fitment-server`; the name is not reserved. It reverses part of workspace-layout's decision 9, which PLAN's decision 11 accepted ("also published to crates.io as internal"); PR 9 records the change | C-10: published internal and unstable, as `fitment-catalog` at 0.2.0, as decision 9 recommended, reserving the name |
| M6-36 | **Your call.** How the exit reaches the API | **The built `fitment-server` binary, as a child process, over loopback HTTP,** in release, in CI's `server-corpus` job on Linux; the same harness runs in process for the cross-OS outputs (C C-1; XH2) | **Buys:** the exit tests what ships, as M4's KH1 ran the built `fitment`. **Costs:** a release build of the server in CI, about a minute | In process only: the wire and the binary's configuration untested |
| M6-37 | **Your call.** What parity means | **Byte for byte, labels included,** for every document a CLI command also writes. Component candidates have no CLI: the reference is the library's `MatchDocument` built with the same labels, byte for byte (C C-2, strengthened by the merge; DC3) | **Buys:** a key reordered, a float rewritten or a field dropped outside the digest all fail. **Costs:** the API serves raw bytes and takes the client's labels | Digest only: misses everything outside the digest. C's labels-stripped fallback for components: not needed once the test hands the library the API's labels |
| M6-38 | **Your call.** Review cases in the corpus | **Added in PR 1:** file candidates on s01's and s02's sockets with one requirement removed in review, including a `review_waived` case with the new truth `waived`, and a still-bad case; KC13's library runs also run through the API (C C-4; XH1, SC8) | **Buys:** the exit's review step is not vacuous (no match case states a review today); the binary path gains its first review runs | Keep the review only in `original.rs`'s library test |
| M6-39 | What the scorer reads | **An `Outcome`,** built from a library `CandidateResult` or from a `fitment.match` entry (C C-5; SC2) | **Buys:** one gate for the library, the binary and the API; core gains no `fitment.match` reader (M4-23) | The gate on typed results only, which needs a reader in core |
| M6-40 | The HTTP client in tests | **`hyper-util`'s client over plain HTTP,** and raw `std::net::TcpStream` for byte-exact hostile requests (C C-6) | **Buys:** no TLS stack, no new licence (webpki-roots, workspace layout section 9) | `reqwest` without default features; `ureq` |
| M6-41 | Async tests and the coverage test | **`is_test` accepts `#[tokio::test]` and `#[tokio::test(...)]`** (C C-7). **Review:** and the `async fn` that follows either, with a control: an `async fn` without a test attribute cited in `RULES` fails (review F17) | **Buys:** a server test can be cited in `RULES` either way | `#[test]` with a `block_on` helper everywhere |
| M6-42 | Fuzzing | **Two `cargo-fuzz` targets, weekly** (C C-8; VT26) | **Buys:** panics and 5xx on odd input found without guessing them | None, relying on the hostile sweeps |
| M6-43 | The container in CI | **Built and smoke-tested on every PR** on Linux; pushed only by a manual workflow the user runs (C C-9; VT29) | **Buys:** the image cannot rot between releases | Built only at release |
| M6-44 | Disk full | **Through the `Fs` seam on every PR,** at every file call of an upload, an ingest batch, a socket write and a result write; **a real 32 MiB tmpfs weekly** on Linux (C C-11; VT11). **Review:** the seam is the catalog's `CountingFs::failing_at`, which CA7's streaming writer and reader let uploads and OB3 go through (review F14) | **Buys:** the seam names the call; the tmpfs shows the operating system's own behaviour | The seam only, as M5 did |
| M6-45 | Load and scale | **Reported, gated only by order-of-magnitude ceilings** (C C-12; VT27, VT28), as CT11 was | **Buys:** no flaky timing gate on shared runners | Hard latency targets |
| M6-46 | The false-negative baseline's place | **Where it is,** `crates/fitment/tests/corpus/false-negatives.json`, read by `fitment-corpus` through a path constant (C C-13) | **Buys:** no churn in a file every review cites | `corpus/baseline.json`, as the layout first planned |
| M6-47 | What blesses the baseline | **The library suite only** (`FITMENT_BLESS=1`, never under `CI`, KH5); the binary, catalog and API suites compare and never write (C C-14; SC5) | **Buys:** one writer; an API-only false negative is a parity failure first | Each path blesses its own |
| M6-48 | **Your call.** Changes asked of the existing crates | **fitment-catalog:** bytes-based ingest and replace with per-file ids and display names, so no server path reaches a document (CA1); the `Observer` (CA2); WAL pressure (CA3); verify's paths relative to the root (CA4); `fitment.verify` and `fitment.gc` documents (CA5); schemars derives (CA6). **fitment-core:** incremental SHA-256 (CO1); run-outcome helpers (CO2); duplicate components merged (CO3); schemars derives (CO4). **The CLI:** `--format json` for `delete` and `prune` (the frozen `fitment.catalog`, as M5's 7.11 already says) and for `verify` and `gc` (CU1, CU2); ingest and replace through CA1 with unchanged output (CU3). **`FitVerifier` stays `Sync` only;** the server holds `Arc<dyn FitVerifier + Send + Sync>` (SV8). The tolerance policy's name `cli` is kept for parity (DC7). **Review:** three more: `Matcher::judge_component` in core (CO5, review F13); a streaming writer and reader in the catalog's `Fs` (CA7, review F14); a catalog instance id in the catalog's `meta` table (CA8, review F24); and CA4 widened to the text of batch-failure and reindex lines, which changes the CLI's text there (CU3, review F7) | **Buys:** the server needs no workaround that would leak a temporary path into a document or parse a library's text. **Costs:** sixteen small changes to two published crates and the CLI, each with its own test | Server-side workarounds: ingest from temporary paths (the path enters `changes[].input` and error lines); rewriting verify's text; a second SHA-256 |
| M6-49 | A job past its deadline | **`failed`, code `deadline_exceeded`,** stopped through the cancellation path; a match leaves no document (merge of A JB10 and B JB12; JB7) | **Buys:** `cancelled` means someone asked; a deadline is the server's failure to answer, as exit 1 is | A's `failed`/`job_timeout` (the name); B's `cancelled`/`deadline_exceeded` (the state) |
| M6-50 | `DELETE` of catalog entries | **Synchronous** (M6-1: it parses nothing and scans nothing), run as one `BEGIN IMMEDIATE` transaction on a short-lived writer `Catalog`; it waits at most for a running job's current batch (≤ 64 files or 1 s, DB3), never for the whole job (merge of A 5.3 and B JB1; CX3) | **Buys:** a delete answers in the request, as the CLI's does | B's: a write-lane job; a sync wrapper around it would wait behind a whole ingest and time out |
| M6-51 | What a progress event may say | **Counts, phases, committed catalog lines and notes; never a verdict** (merge: B JB16 over A JB7's `candidate` verdicts; JB8) | **Buys:** a cancelled, interrupted or failed match shows no verdict anywhere, document or stream (SN7) | A's per-candidate verdict events: a client could act on a verdict from a run that never produced a document |
| M6-52 | Exit 2 inside a job | **`refused`,** with `exit_code` 2 kept distinct from 3 (merge: the coordinator's mapping names exit 3 and exit 1 only; B's `refused` named 2 and 3; JB3) | **Buys:** a selection error, an ingest conflict or a reference that changed between request and job is the input's fault, not the server's | `failed`, which would read as "the server could not do it" |
| M6-53 | `Prefer: wait` and the handler timeout | **A wait is not handler work:** the 30 s handler timeout (HX3) covers validation and submission; the wait is bounded by N ≤ 60 alone (merge of A A1 and B HX3; JB11) | **Buys:** `wait=60` is honoured | A 30 s cap on N |
| M6-54 | Where logs go | **stderr,** as JSON; **stdout carries one line,** `fitment-server: listening on ADDR`, after binding (merge of B OP1 and C's assumption 9; OP1, SD3) | **Buys:** a test or a supervisor reads the bound address of `--listen 127.0.0.1:0` without parsing logs; CLI convention (results on stdout, notes on stderr) | B's JSON logs on stdout, which put startup lines before the address |
| M6-55 | Job ids and list order | **128 random bits,** `j_` and 26 lower-case base32 characters (B JB2); job and match lists ordered by (creation time, id) (merge with A VN3's ULIDs; VN3) | **Buys:** ids are neither guessable nor sequential; the order is total without exposing an internal key | A's ULIDs: time-ordered ids |
| M6-56 | Jobs nobody submitted | **Owned by a reserved `system` user row** that cannot log in and holds no token: `reindex_on_start` (M6-28) and the daily catalog gc (JS9) (merge; OB1) | **Buys:** every job has an owner, as `job.owner NOT NULL` requires; admins see them | A nullable owner |
| M6-57 | Names in requests | **File candidates need a `label`;** components default to `component:<sha256 first 12>/#<n>`. A label or `input` is display text up to 1,024 bytes; a `file_name` (what ID3 builds a default id from) also passes B's name checks: no `/`, `\`, NUL, `.` or `..`, at most 255 bytes. Uploads carry no name (merge of A RA8, B OB5 and DT4; RA8) | **Buys:** labels such as `cand/c00_original.stp`, which the CLI prints, stay possible, while a stored file name is a final component only | B's display name on the upload row, used as the default label: one upload used under two names would have one label |
| M6-58 | **Review.** Tokens minted from tokens (review F1) | **A token mints only within itself:** a token made through `POST /v1/auth/tokens` has a role at most the presenting token's effective role and an expiry at most the presenting token's; it records the token it was minted from, and revoking a token revokes every token minted from it, transitively, in one transaction. **A password change** revokes every other token of the user; an offline `user reset-password` revokes all of them. **At most `auth.max_live_tokens` (20)** live tokens per user (AU4, AU10) | **Buys:** a stolen short-lived reader token cannot be turned into a longer-lived one or a writer's; a password change ends a thief's tokens; minting cannot multiply a user's caps (M6-61). **Costs:** revoking a parent ends the tokens minted from it, so a long-lived CI token should be minted from a login's token | Capped at the user's role and `token_max_days` only (the merged design): a leaked token renews itself for a year. A password change that revokes nothing: it would not lock a thief out |
| M6-59 | **Review.** What a run that did not succeed says (review F2) | **`outcome` is null for every job that did not succeed,** in `fitment.job`, `fitment.run`, the `end` event and every listing. A failed match whose document is kept states what it judged under `fitment.run`'s `problems.incomplete_summary`, never in `summary`; run listings and candidate pages carry `complete` and `exit_code` (JB3, SN4, DC10, DC12, XH12) | **Buys:** "the run failed" never reads as `reject` ("nothing fits") in any envelope; a client paging a failed run's kept document sees on every page that it is incomplete | Copying `summary` and `outcome` from every kept document (the merged design): a failed run whose judged candidates were all rejected read as `outcome: reject` |
| M6-60 | **Review.** **Your call.** Catalog-scale work (review F3) | **Writers only** for the operations whose cost scales with the whole catalog: an exhaustive run, a catalog-wide reindex and `verify`, plain or `deep` (AU5). **An exhaustive run over more live entries than `limits.max_catalog_candidates` (100,000)** is refused, `too_large`, exit 3 (JS12). **A catalog match's memory** is estimated per selected entry, not as a flat 512 MiB (JS3). **The candidates view** slices the stored bytes through a byte-offset index built at the job's end (DC12). **Job documents** count in their owner's quota (UP4). **Synchronous parsing** of socket documents runs behind a semaphore, and `limits.max_socket_mib` drops from 64 to 16 (HX8). **The WAL barrier** pauses no job for longer than `catalog.barrier_max_seconds` (60) (CX6) | **Buys:** a reader cannot spend the server's memory, disk and CPU at catalog scale; a 100 MB document is never parsed per page. **Costs:** a reader who needs an exhaustive run asks a writer; a larger exhaustive run needs the cap raised; a socket document over 16 MiB is refused | Admin only: the review asked, and the coordinator chose writers (section 17). A flat estimate: a 10⁵-entry exhaustive run counted as 512 MiB |
| M6-61 | **Review.** Whose caps (review F5) | **Per user, not per token:** the request rate (50 a second, burst 100), concurrent uploads (2) and event streams (8) are counted per user over all their tokens. **An upload slower than `limits.upload_min_kib_s` (64 KiB/s)**, averaged over any 30 s after its first, is cut (`408 upload_too_slow`). **Login buckets** are keyed by (folded username, client address) for every username, existing or not, and by address alone; at most `auth.login_buckets` (10,000) are kept, the least recently used evicted (AU7, AU10, UP4, HX3, JS10) | **Buys:** minting tokens multiplies no cap; a trickled upload cannot hold a slot for 30 minutes; the bucket map is bounded; buckets do not tell which usernames exist; failures from one address cannot lock an admin out at another. **Costs:** an attacker spread over many addresses gets 10 tries per 15 minutes from each, against argon2 and a 12-character minimum | Per token (the merged design): twenty tokens, twenty times the caps. A per-username lockout: anyone could lock the admin out |
| M6-62 | **Review.** **Your call.** Imported sockets (review F12) | **Re-extracted when the caller holds the assembly:** `POST /v1/sockets/import` creates the job `socket_import`, which extracts the document's target and occurrence from the caller's registered assembly and requires the extraction's `digest` to equal the import's (`socket_not_reproduced`, exit 3, otherwise); the revision is `verified: true`. **Without the assembly** the revision is stored `verified: false`. `how` and `verified` appear in revision listings and in `fitment.run`, and a run on an unverified socket says so in a note (RA10) | **Buys:** a hand-edited or forged socket file cannot pass for one extracted from the assembly, and a run record says which kind it judged. **Costs:** an import with the assembly held parses STEP and so is a job | A synchronous import trusted as given (the merged design). Refusing every import without the assembly: users could not bring reviewed files whose assembly they do not upload |
| M6-63 | **Review.** Catalog ETags (review F24) | **`"<instance>.g<generation>.<build>"`**, the instance a random 128-bit id stored in the catalog's `meta` table under the key `instance`, written once by the server's job writer at a start that finds none (CA8); nothing else reads it, and 0.2.0 ignores the row. **Derived documents** (`features` of any reference, `…/parts`) are `no-cache` (VN5) | **Buys:** two catalogs at one generation never share an ETag, so a `304` never confirms another catalog's view; a derived document read by another build is revalidated. **Costs:** a catalog change, a row and no schema change (CA8) | Derived from the catalog: nothing in schema 1 is unique to one catalog (its `init` op row holds generation 1 and a version). A nonce per server start: every restart invalidates every cache |
| M6-64 | **Review.** **Your call.** What a run record keeps (review F20) | **Its inputs, for its retention:** a match's `job_input` rows (every candidate's object, and an uploaded socket's) and `job_cites` row (its socket revision) are written at submission and live as long as the job record, so a retained or pinned record can be re-run on the same bytes and audited, as SN6 says (OB4) | **Buys:** SN6 holds for file runs, not only for catalog runs. **Costs:** a match's candidates stay on disk for its record's 30 days, counted in the owner's quota (UP4) | Narrowing SN6 to catalog runs and to runs whose uploads the user kept: a record would name SHA-256s nobody holds |

## 3. How it fits together

### 3.1 The flow

```text
             TLS proxy (operator's)                       fitment-server process
client ──HTTPS──► proxy ──HTTP/1.1──►  accept loop (hyper http1: header timeout, buffer cap, connection cap)
                                          │  tower: request id, trace, sensitive headers, catch panic,
                                          │  timeouts, per-route body limits, response headers
                                          ▼
                                       axum Router ── Reader | Writer | Admin extractor ── handlers
                ┌─────────────────────────┼──────────────────────────────┬──────────────────────────┐
     sync reads │              uploads    │ stream → tmp → SHA-256 (CO1) │ submit a job              │ sync writes
  (spawn_blocking: catalog     (UP2)      │ → sync → rename → object row │ (job + inputs committed   │ (reviews, deletes:
   snapshot per request)                  ▼                              ▼  before 202, JS11)         ▼  CX3)
     list, get, features,     objects/sha256/… + upload row        Scheduler (one task)          server.db / one
     step, info, pages                                             lanes: query │ write │ maint   catalog transaction
                                                                   CPU semaphore (fair), memory semaphore
                                                                              ▼
                                               spawn_blocking(run_job): fitment-core and fitment-catalog calls,
                                                 the cancel flag and the Observer (CA2), progress → broadcast
                                                                              │
                        NDJSON stream ◄── broadcast + job_event rows ◄────────┤
                        result bytes  ◄── objects (each document by SHA-256) ◄┘
disk:  <catalog>/  catalog.db (schema 1, unchanged), blobs/, tmp/        ← opened only by fitment-catalog
       <state>/    server.db, objects/sha256/xx/<hex>, tmp/             ← opened only by fitment-server
```

A job's life: the request is validated (shape, references, the socket gate for a match: JB2) without parsing STEP; the job row and its inputs commit; the response is `202` with the job, or with `Prefer: wait` the finished job. The scheduler dispatches it when its lane has a slot and its permits are free; the job runs in one blocking task with the library calls the CLI makes, checking its cancel flag where it may stop; its document's bytes are stored as an object and the row records the exit code, outcome, error and (for a catalog run) the generation and counts. The client reads `fitment.job`, the document's exact bytes, and for a match the `fitment.run` record.

### 3.2 The resource map

```text
/v1
  info                                   fitment.info: versions, limits, fit verifier, catalog state
  health, ready                          liveness, readiness (no auth)
  openapi.json                           the committed OpenAPI 3.1 document (no auth)
  uploads/sha256:{hex}                   the caller's staged bytes
  catalog                                the catalog's generation and counts
  catalog/entries                        list (fitment.catalog); ingest (job)
  catalog/entries/{ref}                  get (fitment.catalog); replace (job); delete
  catalog/entries/{ref}/features         the stored features.v1
  catalog/entries/{ref}/step             the stored bytes, re-hashed
  catalog/delete                         delete several, all or none (CR2)
  catalog/reindex | verify | gc | prune  maintenance (jobs)
  assemblies                             the caller's: list; register (job)
  assemblies/sha256:{hex}                fitment.assembly; delete
  assemblies/sha256:{hex}/parts          fitment.parts
  assemblies/sha256:{hex}/step           the stored bytes
  assemblies/sha256:{hex}/sockets        the socket lines of this assembly
  sockets                                the caller's lines: list; extract (job)
  sockets/import                         a socket document the CLI wrote (job)
  sockets/{ref}                          fitment.socket, exact bytes; delete the line
  sockets/{ref}/revisions                the line's history
  sockets/{ref}/review                   decisions: a new revision
  features                               read one uploaded part (job)
  matches                                the caller's runs: list; run (job)
  matches/{id}                           fitment.run
  matches/{id}/document                  fitment.match, exact bytes
  matches/{id}/candidates                a paged view of its candidates
  jobs                                   the caller's jobs: list
  jobs/{id}                              fitment.job; ?wait=N
  jobs/{id}/events                       NDJSON progress
  jobs/{id}/document                     the document the job produced, exact bytes
  jobs/{id}/cancel                       request cancellation
  jobs/{id}/pin                          keep past retention (admin)
  auth/login, me, password               tokens from a password; the caller
  auth/tokens, auth/tokens/{id}          the caller's tokens
  auth/users, auth/users/{id}            users (admin)
```

`{ref}` for a catalog entry is ID7's: `ID`, `ID@rN` (ID9) or `sha256:HEX` (12 to 64 hex digits, either case). `{ref}` for a socket is `sk_HEX` (the line's current revision) or `sk_HEX@rN`. `:` and `@` are legal in a path segment (RFC 3986 `pchar`); a client may percent-encode them, and the server decodes the segment once and then parses it strictly (HX5). `+`, which ID1 allows, is a literal in a path. Every list of the caller's own resources takes `?owner=USER_ID` for an admin (AU8).

### 3.3 Crates and modules

```text
fitment-server (publish = false) ──> fitment-catalog ──> rusqlite (SQLite, bundled) ──> catalog.db
      │   └──> rusqlite ──> server.db         │
      │   └──> tokio, hyper, axum, tower      └──> fitment-core ──> stepq (private)
      └────────────────────────────────────────────> fitment-core
fitment (CLI) ──> fitment-catalog ──> fitment-core                 (no async crate: SV1)
fitment-corpus (publish = false, a dev-dependency only) ──> fitment-core, serde_json
```

`fitment-server` is the only async crate. Its layout (B 4.1, with A's resources):

```text
crates/fitment-server/
  Cargo.toml        publish = false; [package.metadata.dist] dist = false (M6-35); features: schema, test-support
  clippy.toml       SV7
  openapi.json      the committed OpenAPI document (VN7)
  src/
    lib.rs          Server::build(Config) -> Server; Server::router(); Server::serve(listener, shutdown)
    main.rs         clap: serve | admin {bootstrap, user, token} | healthcheck | check-config
    config.rs       file, environment, flags (OP2, OP3); the only module that reads the environment
    net.rs          accept loop, connection cap, graceful shutdown (HX1, SD1)
    http/
      mod.rs        Router and the tower layers (HX)
      json.rs       Json extractor and responder (M6-34)
      error.rs      ApiError and ErrorCode: status, class, exit code, retryable (ER)
      extract.rs    Reader, Writer, Admin extractors; typed path parameters (HX5)
      operations.rs OPERATIONS: method, path, role, mode, request and response types, codes (VN7, AU5, XH14)
      routes/       one module per resource of section 7
    auth/           password.rs (argon2), token.rs, limit.rs (buckets), roles.rs (AU)
    jobs/
      model.rs      Job, Kind, Lane, State and the transitions (JB3)
      scheduler.rs  queues, lanes, permits, fairness (JS1 to JS5)
      run.rs        one function per kind, run inside spawn_blocking (JS6)
      observe.rs    the fitment_catalog::Observer implementation; progress fan-out (JS8, JS10)
      recover.rs    startup recovery (SD4)
    state/
      db.rs         server.db: schema, migrations, connections (OB1)
      objects.rs    the content-addressed store (OB2, OB3)
      sweep.rs      retention, expiry, object gc (JS9, OB4)
    catalog.rs      the only module naming fitment_catalog: pools, the writers, error mapping (CX)
    matching.rs     the only module building Matcher and MatchDocument (SN1)
    metrics.rs      counters and histograms; text exposition (OP6)
```

### 3.4 One corpus case through the API

The case `m_s01_bracket_blind` (KC2): the socket of BRACKET in `s01_bolted`, no review, one candidate, both policies (A 13).

```text
PUT  /v1/uploads/sha256:<asm>            body: s01_bolted.stp                           201 fitment.upload
POST /v1/assemblies                      {"upload": "sha256:<asm>", "file_name": "s01_bolted.stp"}
     Prefer: wait=30                                                                    200 job succeeded, exit 0
POST /v1/sockets                         {"assembly": "sha256:<asm>", "part": "BRACKET"}
     Prefer: wait=30                                                                    200 job succeeded, exit 0,
                                                                                        result /v1/sockets/sk_<id>@r1
PUT  /v1/uploads/sha256:<cand>           body: m_s01_bracket_blind.stp                  201
POST /v1/matches                         {"socket": "sk_<id>@r1",
                                          "candidates": {"files": [{"upload": "sha256:<cand>",
                                                                    "label": "m_s01_bracket_blind.stp"}]},
                                          "policy": {"clearance": "strict"}}
     Prefer: wait=30                                                                    200 job succeeded, exit 5
GET  /v1/matches/<job>/document                                                         200, Fitment-Exit-Code: 5
```

The last body equals, byte for byte, the stdout of `fitment match S.json m_s01_bracket_blind.stp --format json`, run in a directory holding the candidate after `fitment socket s01_bolted.stp --part BRACKET -o S.json`; and `GET /v1/sockets/sk_<id>@r1` equals `S.json`. A case with review entries adds one `POST /v1/sockets/sk_<id>@r1/review` before the match; a component case names `components` instead of `files`; the second policy is a second match with `"clearance": "functional"`.

## 4. Rules

Each rule is stated so that a test can fail it. The tests are in section 10, and the PR that builds each rule in section 12. The origin follows each rule's title: A, B or C with the designer's own id, or "merge" for a rule the merge wrote.

### 4.1 Resources and references (RA)

- **RA1. One server, one catalog** (A RA1; M6-7). `/v1/catalog` is the catalog named by `paths.catalog` (OP3) and opened at start. Nothing in a request names a directory or a path. Catalog entries are shared by every authenticated user: the catalog is one tenant (B OB4, OP10); several tenants are several servers.
- **RA2. Catalog references are the CLI's** (A RA2). Catalog entries take ID7's forms with ID9's strictness:
  - **`GET`** accepts any revision an id had;
  - **`PUT`, `DELETE`, `reindex` and a match's `ids`** accept `ID@rN` only for the current revision (`409 revision_not_current`, nothing changed);
  - **`delete`** takes ids only (`400 not_an_id`);
  - **lookup** ignores ASCII case (ID2); a response for a case-folded match carries `Content-Location` with the id as stored;
  - **an unknown reference** is `404 entry_not_found` with up to five suggestions; one that matches several is `409 reference_ambiguous` with them listed.
- **RA3. Assemblies are kept by their SHA-256** (A RA3; PLAN, Choosing the part to replace). Registering an uploaded assembly (`POST /v1/assemblies`) makes an assembly row in the caller's namespace.
  - **Stable ids:** its bytes never change under its name, so the `#id`s and occurrence paths a socket records stay valid while the row lives, and RA6 keeps it while a socket line names it.
  - **Its `fitment.parts` document** is derived and keyed by this build's `EXTRACTOR_VERSION` and `SOCKET_RULES_VERSION`, insert-only as the catalog's derived rows are (DB7). When no row of this build's versions exists, `GET …/parts` is `409 derived_stale`, and registering the same bytes again reads them anew.
  - **A refused assembly is registered too,** its `fitment.parts` saying why (exit 3), as the catalog keeps a quarantined part; extracting a socket from it gives the refused socket document, which is the CLI's answer.
- **RA4. Socket lines** (A RA4; M6-2). A line is the triple (assembly SHA-256, target definition `#id`, occurrence path or all) in its owner's namespace.
  - **Its id** is `sk_` followed by the first 20 hex digits of the SHA-256 of `"fitment.socket-line\n" + assembly_hex + "\n" + definition + "\n" + occurrence`, the occurrence written as its `#id`s joined by `/`, or `*` for all.
  - **The id depends on the data alone:** two servers, or two users, holding the same data compute the same id, and extracting twice gives one line.
  - **Lines are per owner** (merge; AU8): `sk_X` resolves only among the caller's own lines, so two users with the same data hold two lines of one id, and knowing an id grants nothing.
  - **A collision** in one namespace (another triple, the same id) is refused as `internal`.
  - **A deleted line revives** (review F20). Extracting or importing a deleted line's triple again makes a new live row under the same id: a partial unique index (`socket_line_live`) admits one live row per owner and id beside any tombstones. Its revisions are numbered on from the highest revision of that id, so `sk_X@rN` names one revision across the line's rows and a run that cited an old one still names it; the new revision carries decisions only from a `carry_from`.
- **RA5. Socket revisions** (A RA5). Numbered from 1, rising by one with each extraction, import or review that changes the document's bytes, or that makes a verified revision where the current one is not (RA10), and never reused or rewritten (OB5's rows are insert-only).
  - **Names:** `sk_X` names the current revision, `sk_X@rN` one exactly, in ID9's grammar (lower-case `@r`, no leading zero).
  - **What a revision records:** how it was made (`extract`, `review`, `import`), whether it is `verified` (RA10), its document's SHA-256 and `digest`, its `EXTRACTOR_VERSION` and `SOCKET_RULES_VERSION`, its parent revision, its unreviewed, carried and stale counts, and the job or request that made it.
- **RA6. Nothing a kept result cites disappears** (A RA6, B OB6; M6-23).
  - **An assembly** cannot be deleted while a live socket line of its owner names it (`409 assembly_in_use`).
  - **Deleting a socket line** leaves a tombstone. Its revisions stay readable while a retained run record cites them, and a match may name a cited one, so the run can be submitted again (SN6); they go with the last such record.
  - **An upload** a job names cannot be deleted while the job is not terminal (`409 upload_in_use`); OB4's hold keeps its object after that for as long as a retained run record names it.
  - **Catalog entries** keep M5's rules: a run's `catalog:<id>@r<n>` label resolves by `GET` while the catalog keeps its bytes (CM6, CR3, M5-41).
- **RA7. Optimistic concurrency by revision** (A RA7). Writes to a revisioned resource name the revision they change.
  - **Socket reviews must:** a bare id is `428 revision_required`, and a revision that is not current is `409 revision_not_current`. Two reviewers of one socket are the expected case, and a lost decision can let an unreviewed requirement stand unseen.
  - **Catalog writes may** (ID9), keeping the CLI's optional form for parity.
  - **ETags are not used for writes;** they serve caching (VN5).
- **RA8. Labels and names are the client's** (A RA8, SN11, B OB5, DT4; M6-57).
  - **A file candidate's `label`** is required: the client's string, at most 1,024 bytes of UTF-8 without control characters, as the CLI's label is the argument as given (`cand/c00_original.stp`). **An ingest's `input`** follows the same rule and defaults to its `file_name`.
  - **A `file_name`** (what ID3 builds a default id from and PH3 stores) passes these checks as well: no `/`, `\` or NUL, not `.` or `..`, at most 255 bytes. A name failing them is `400 name_invalid`, never repaired.
  - **A component's label** defaults to `component:<assembly sha256, first 12 hex>/#<n>`.
  - **A label starting `catalog:`** is `400 label_reserved`: a file a client uploads can never pass for a catalog entry's revision (CM6). Catalog candidates' labels are made by the server, `catalog:<id>@r<n>`, as the CLI makes them.
  - **Names are display text:** stored and returned as given, escaped wherever a table renders them (PH6), never a path, an order key, an identity or an SQL fragment. Labels stay out of the digest and the order (MF4). Uploads carry no name.
- **RA9. A run's inputs are frozen when the job is created** (A RA9). A bare `sk_X` resolves to its current revision and every upload named is held from creation (OB4); the job's normalized `request` records the revision and every SHA-256. A catalog run reads one snapshot, taken when the job starts (QR2), and records its generation (DC10).
- **RA10. An imported revision is verified or marked** (review F12; M6-14, M6-62). `POST /v1/sockets/import` reads the document as UP6 says before any job exists, then creates the job `socket_import`:
  - **With the document's assembly registered by the caller,** the job extracts the document's target and occurrence from those bytes (`SocketDocument::extract`, as `socket_extract` does) and compares the extraction's `digest` with the import's. Equal: the imported bytes, review included, become the revision, `verified: true`. Different: the job ends `refused`, exit 3, `socket_not_reproduced`, with both digests, and no revision is written.
  - **Without it,** the revision is stored `verified: false`; nothing is parsed but the document.
  - **Extracted revisions are verified, and a review's revision has its parent's flag.** An unverified revision becomes verified only through a new revision: the assembly registered and the document imported again, or an extraction with `carry_from` naming it.
  - **Where it shows:** `how` and `verified` in revision listings and in `fitment.run`'s `socket`; a match on an unverified revision carries the note `unverified_socket` ("the socket was imported without its assembly; the server did not extract it"). The `fitment.match` bytes are the CLI's either way (DC1).

### 4.2 Jobs as a client sees them (JB)

- **JB1. What a job is** (A JB1, B JB1; M6-1, M6-50).
  - **The kinds:** `features`, `assembly_read`, `socket_extract`, `socket_import` (RA10), `match`, `catalog_ingest`, `catalog_replace`, `catalog_reindex`, `catalog_verify`, `catalog_gc`, `catalog_prune`.
  - **Synchronous, and bounded by the references they name:** reads, uploads, socket reviews, deletes (catalog entries, assemblies, socket lines, uploads), and auth.
  - **Validation before the job exists:** what needs no STEP parse and no catalog snapshot is checked at the request and answered with an error status, and no job row is written: the request's shape, references that do not exist, labels and names (RA8), the role a field asks for (AU5), the socket gate (JB2), the caller's caps (JS5) and quota (UP4).
- **JB2. The socket gate is synchronous, and checked again at dispatch** (A JB2; review F21). On `POST /v1/matches`, MF11's version check and then MF2 run against the pinned socket revision or uploaded document, with the `require_reviewed` floor applied (SN3). A socket revision never changes, but the build and the floor can change while the job waits (a restart into an upgraded image, a new configuration), so the scheduler runs MF11, MF2 and the floor again when it dispatches the job; a failure ends the job `refused`, exit 3, with the gate's code (`socket_stale`, `socket_refused`, `socket_unreviewed`), before any candidate is read. QR1's catalog checks (stale, empty, blocked) run in the job's own snapshot, because the catalog can change between the request and the start; a pre-check at the request would be advisory, so there is none.
- **JB3. States** (B JB3, A JB3; M6-21, M6-52).

  ```text
              submit                    dispatch                    returns
    (none) ─────────► queued ─────────────► running ───────────────► succeeded | refused | failed
                        │  cancel             │ cancel, deadline, shutdown
                        ▼                     ▼
                    cancelled            cancelling ──(returns)──► cancelled | failed (deadline)
                                              │                    (or the state it reached, if it finished first)
                         restart, shutdown    ▼
                   running | cancelling ────► queued (pure, attempt ≤ 2: JB6) | interrupted | cancelled
  ```

  | State | Meaning | `exit_code` | Document |
  | --- | --- | --- | --- |
  | `queued`, `running`, `cancelling` | not finished | null | none |
  | `succeeded` | the job ran to its end and its document is the complete answer | 0, 4 or 5 | always |
  | `refused` | the CLI would exit 2 or 3 for the same inputs: a refused file's features, a refused assembly or socket, a selection error, an ingest batch with a quarantined or conflicting file, a stale or empty catalog, a blocked socket in a full run, a replace with a refused file, a `verify` that finds a problem (M5-25), a component selected in a refused assembly (CO5), a socket the running build or floor refuses at dispatch (JB2); or one of the server's stated tightenings refuses it (SN3): an import its assembly does not reproduce (RA10), an exhaustive run over the cap (JS12) | 2 or 3 | where the CLI prints one (a refused `features.v1`, `fitment.parts` or `fitment.socket`, a `fitment.catalog` with `changes`, a `fitment.verify` listing problems); none for QR1's refusals, where the CLI prints nothing |
  | `failed` | the CLI would exit 1, or the job could not answer: an error, a fitment bug, a damaged row, a deadline | 1, or null for a deadline, which no CLI run has | kept where the CLI prints one with exit 1 (a match with a damaged row or a bug, an original failing its own socket, an ingest whose batch could not commit), with `fitment.run`'s `complete: false` and `outcome` null; never presented as a result |
  | `cancelled` | stopped at its owner's or an admin's request | null | JB5 |
  | `interrupted` | a write job running when the process stopped | null | after a graceful drain, the document of its committed changes; after a hard kill, none (JB6) |

  The state follows the exit code, with CL2's precedence across a batch's files (1, 2, 3, 0): an ingest that added nine files and quarantined one ends `refused`, exit 3, its document listing every line, as the CLI exits 3 after printing them. `outcome` is set only for a job that `succeeded`: for `match`, `match`, `interface_match` or `reject`, from `MatchDocument::outcome()` after `require_fit`; for socket and catalog jobs that changed nothing, `unchanged`. In every other state it is null, in `fitment.job`, `fitment.run`, the `end` event and every listing, so no refused, failed or stopped job reads as a verdict (review F2; M6-59).
- **JB4. `exit_code` is the CLI's** (A JB4, B JB4; M6-33) for the same inputs, labels and build: VD14 for `match`, CL2 for the catalog's commands with its precedence across files, M3's section 12 for `parts` and `socket`, M2's for `features`. It is computed by CO2's helpers, which the CLI also calls, and the exit test compares it with the binary's (XH7).
- **JB5. What a stopped job leaves** (A JB5, B JB12, DT6; M6-27). `POST /v1/jobs/{id}/cancel`, by the owner or an admin, is a request: a queued job becomes `cancelled` at once; a running one becomes `cancelling` until its current call returns (JS8), then `cancelled`, or the state it reached if it finished first.

  | Kind | What a cancel leaves |
  | --- | --- |
  | `match` | nothing: no document and no run record. A document over the candidates judged so far would state a verdict over a set nobody asked for |
  | `features`, `assembly_read`, `socket_extract`, `socket_import`, `catalog_verify` | nothing; an extraction that finished is discarded, and no revision is written (a result already committed with the job's end, JS13, means the job had finished first) |
  | `catalog_ingest`, `catalog_reindex` | what its committed batches changed (RX7); its document lists exactly those changes, and `exit_code` is null |
  | `catalog_gc` | what it unlinked before the next batch of 1,000 (ST5); its `fitment.gc` document counts exactly that |
  | `catalog_replace`, `catalog_prune` | not cancellable once running: each is one transaction; the cancel answers `409 job_finished` once it has ended |
- **JB6. Restart** (B M6B-8, SD4, A JB6; M6-22; review F10, F11). At start, a pure job (`features`, `assembly_read`, `socket_extract`, `socket_import`, `match`, `catalog_verify`) that was running is first checked for a result it already committed (JS13); found, the job ends as that result says and does not run again. Otherwise it is re-queued with `attempt + 1` if `attempt` is below 2, and is otherwise `interrupted`. A write job that was running is `interrupted` with `job_interrupted`; one that had been asked to cancel is `cancelled`. A re-queued match is a new run in a new snapshot, never resumed: no document mixes two snapshots. A graceful drain re-queues a pure job without consuming an attempt (SD1).
  - **After a hard kill a write job carries no document:** its change lines were in the killed process's memory. Its `error.details` give the catalog's generation when the job started (`generation_at_start`, recorded at dispatch) and when recovery ran (`generation_at_recovery`), so a client can read what changed between them (the entries' history). After a graceful drain it keeps the document of its committed changes (SD1).
  - **Submitting again:** an interrupted ingest, reindex or gc does exactly what is missing (RX7, ID5's `unchanged`), and its `job_interrupted` is retryable. A replace or a prune is one transaction that may or may not have committed, so its `job_interrupted` is not retryable: the client first reads the entry (`GET /v1/catalog/entries/{id}`: is the new revision there?) or, for a prune, whether an old revision's bytes are still kept (`410 bytes_not_kept` once pruned), and submits again only if the change is missing. A replace naming `ID@rN` is refused `409 revision_not_current` if it had committed.
- **JB7. Deadlines** (A JB10, B JB12; M6-49). A pure job has a deadline: `jobs.deadline_extract_minutes` (10) for `features`, `assembly_read`, `socket_extract` and `socket_import`, `jobs.deadline_match_minutes` (60) for `match`, `jobs.deadline_verify_minutes` (60) for `catalog_verify`, plain or `deep` (review F23), none for the write lane (M5-36 sets no limit on a run). A job past its deadline is stopped through the cancellation path (JS8) and ends `failed` with `deadline_exceeded`; a match leaves no document. M4's bounds (about 5 s per candidate and socket at worst, QR9) make the default generous.
- **JB8. Progress events** (A JB7, B JB15, JB16; M6-9, M6-51). `GET /v1/jobs/{id}/events?after=SEQ` streams `application/x-ndjson`, one event per line, each `{"format": "fitment.job.event", "format_version": 1, "job": ID, "seq": N, "type": T, …}`, where only a persisted event has `seq`:

  | Type | Carries | Persisted |
  | --- | --- | --- |
  | `state` | the new state and `attempt` | yes |
  | `batch` | a catalog batch committed: its generation and its count of lines | yes |
  | `change` | one `fitment.catalog` change object as it is committed (the CLI's line per file) | yes |
  | `note` | a `notes` entry (JB10) as it is made | yes |
  | `end` | `{state, exit_code, outcome, result, document, error}`, `outcome` null unless `succeeded` (JB3); the stream closes after it | yes |
  | `progress` | `{phase: "reading" \| "level_one" \| "loading" \| "judging" \| "batch", done, total \| null}`, coalesced to at most ten a second | no |
  | `heartbeat` | nothing; every 15 s without another event | no |
  | `lagged` | `{missed}`: a slow reader skipped transient events | no |
  | `shutdown` | the server is draining (SD1); the stream closes | no |

  - **No verdict travels on the stream** (M6-51): no candidate's verdict, status or failed requirement, only counts. Events inform; only the document is a result (SN7).
  - **`seq`** numbers the persisted events alone, rising by one per persisted event; transient events (`progress`, `heartbeat`, `lagged`, `shutdown`) carry none (review F6), so a number is never given to an event that a replay cannot return. `?after=SEQ` replays every persisted event after SEQ, then follows live ones; a resumed reader misses only transient events.
  - **Persisted events** are kept as long as the job record (JB12).
- **JB9. Idempotency** (B JB13, A JB8; M6-13; review F19). A job-creating request may carry `Idempotency-Key`, at most 128 printable ASCII characters, stored per owner with the request's fingerprint, `idempotency_sha256`: the SHA-256 of the method, the path and the body as sent, normalized for syntax only (the body parsed under VN6 and written with its keys sorted, no whitespace and each number as parsed). References are never resolved for it: a bare `sk_X` stays `sk_X`, so a retry after the line gained a revision is the same request.
  - **The same key again** with the same fingerprint returns the existing job with the status the original request would get now: `202` while the job is not terminal, `200` once it is (with `Prefer: wait`, as JB11). With another fingerprint: `422 idempotency_key_reused`.
  - **A key is released** when its job ends `interrupted`, `cancelled`, or `failed` with a retryable code (ER6), so a retry with it creates a new job; otherwise a key lives as long as its job record.
  - **`request_sha256`** is separate: the SHA-256 of the canonical request (the kind, the parameters in a fixed field order with defaults filled, and every reference resolved to its SHA-256 or exact revision), recorded with the job as what it ran.
  - **No result cache:** a submission without a matching key runs (GT9, RX10). Without a key, the operations are idempotent in effect where the CLI's are: ingesting held bytes is `unchanged` (ID5), extracting an unchanged socket is `unchanged` (RA4, RA5).
- **JB10. stderr becomes `notes`** (A JB9). Every line the CLI writes on stderr for the same run is a `notes` entry `{code, message, …fields}`, in the CLI's order:
  - the summary;
  - each warning: `unverifiable_requirement`, `coarse_floor`, `empty_socket`, `rejects_every_candidate`, `unreviewed_requirements`, `require_fit_impossible`;
  - each refused candidate's first reason;
  - carry-over counts (`carried_over`), CM8's search line (`catalog_searched`), and quarantine and search transitions (`quarantined`, `left_search`, `joined_search`);
  - and one note the API adds, which no CLI run writes: `unverified_socket` (RA10).

  Messages are the CLI's sentences with every path removed and every "run `fitment catalog …`" replaced by the endpoint that does it (ER3).
- **JB11. Waiting for a job** (A A1, 5.8; M6-1, M6-53). A job-creating request with `Prefer: wait=N` holds the response until the job ends or N seconds pass, N at most 60 (a larger N is applied as 60, and `Preference-Applied: wait=60` says so): `200` with the finished `fitment.job`, or `202` with the job as it stands. `GET /v1/jobs/{id}?wait=N` long-polls the same way. A wait is not handler work: the 30 s handler timeout (HX3) covers validation and submission only.
- **JB12. Run records are kept, then expire** (B JB14, M6B-9; M6-23). A terminal job, its document and its run record stay `jobs.retention_days` (30) after the job ends; `fitment.job`'s `times.expires` says when. An admin pins one past retention (`POST /v1/jobs/{id}/pin`) or deletes a finished one earlier (`DELETE /v1/jobs/{id}`). What a job made durable (catalog changes, socket revisions, assemblies) is not removed with it.

### 4.3 The job system (JS)

- **JS1. Three lanes** (B JB5; M6-26; review F23).

  | Lane | Kinds | Concurrent | Permits each |
  | --- | --- | --- | --- |
  | query | `features`, `assembly_read`, `socket_extract`, `socket_import`, `match` | `query_slots` = max(1, cpu / match_threads) | 1; a match `match_threads` (4), but 1 for a file match with one candidate |
  | write | `catalog_ingest`, `catalog_replace`, `catalog_reindex`, `catalog_prune`, `catalog_gc` (with or without `vacuum`) | 1 | 1 (ingest and reindex are single-threaded in `fitment-catalog`); `vacuum` also takes the barrier (CX6) |
  | maintenance | `catalog_verify`, plain or `deep` | 1 | 1; no barrier |

  `VACUUM` writes the whole database, so it queues behind the catalog's other writers rather than beside them. `verify` only reads: it runs on the maintenance lane beside the write lane, holding one read transaction at a time and bounded by its deadline (JB7); `deep` re-reads blobs the same way and takes no barrier.

- **JS2. One CPU budget** (B JB6). A fair tokio `Semaphore` with `jobs.cpu` permits (default `available_parallelism()`).
  - **How it is used:** a job holds its permits from dispatch to return. A catalog match passes its permit count to the catalog as `jobs` (QR8); a file match uses it as its window (JS7). The server's compute never exceeds `cpu` threads, and the catalog's scoped threads stay inside the budget.
  - **Fairness:** tokio's semaphore is fair for `acquire_many`, so a four-permit match is never starved by one-permit jobs behind it.
  - **The bytes:** never depend on the permit count (QR8, DC3).
- **JS3. A memory budget, heuristic** (B JB7). A second weighted semaphore, in MiB (`jobs.memory_mib`, 4,096).
  - **The estimates:** `features`, `assembly_read`, `socket_extract` and `socket_import`: 10 × the file's bytes, since stepq's graph is about ten times the file (SZ1); a file match: window × 10 × the largest candidate; a catalog match (review F3): 64 MiB plus `jobs.match_entry_kib` (16) per entry its selection can judge, read from the catalog's counts at dispatch: every live entry for an exhaustive run (at most `max_catalog_candidates`, JS12), every searchable entry for a full run (level 1's survivors are unknown before its snapshot), the named entries for a named run; ingest and replace: 10 × the largest file in the batch.
  - **A job estimated above the budget** waits for the whole budget and runs alone; it is never refused for its estimate.
  - **Why only a heuristic:** std gives no memory query and the estimate is not a bound; the real guard is the container's memory limit, and an OOM kill is recovered by SD4.
- **JS4. Order and fairness within a lane** (B JB8).
  - **The order:** one FIFO per owner, owners taken round-robin.
  - **Per-owner caps:** at most `jobs.per_owner_running` (2) running and `jobs.per_owner_queued` (16) queued; an admin jumps no queue.
  - **What that gives:** a user with sixteen long catalog matches cannot hold off another user's socket job for more than one job's turn per lane slot; short queries never wait behind ingests (another lane), and wait behind long queries only for a free slot and permits; no job is preempted.
- **JS5. Backpressure** (B JB9). Lane queues hold at most `jobs.queue_query` (64), `jobs.queue_write` (16) and `jobs.queue_maintenance` (4). A submission past a lane's cap is `503 queue_full`, past its owner's cap `429 too_many_jobs`, each with `Retry-After`; no job row is written, so the refusal leaves nothing behind. Uploads and streams are bounded by UP4 and JS10, connections by HX1.
- **JS6. One function per kind, inside `spawn_blocking`** (B JB10). Its context: the job's frozen inputs, an `Arc<AtomicBool>` cancel flag, the deadline, a progress sender, and the CPU and memory permits, owned so that they drop when the closure ends, panic included. Each kind makes the CLI's calls (SN1):
  - **`features`:** read the upload (OB3); `FeaturesDocument::read`; store `to_json()`.
  - **`assembly_read`:** read the assembly; the `fitment parts` reading; store the `fitment.parts` bytes, then commit the `assembly_parts` row with the job's end (JS13).
  - **`socket_extract`:** read the assembly; `SocketDocument::extract(bytes, part, occurrence)`; carry decisions from the line's current revision or the request's `carry_from` through `SocketDocument::prior`, as `fitment socket -o` does; store the bytes, then commit the revision with the job's end (JS13), unless refused or unchanged.
  - **`socket_import`:** with the assembly held, read it and extract the document's target and occurrence, then compare digests (RA10); store nothing new but the revision, committed with the job's end (JS13).
  - **`match`, files and components:** the socket gated again (JB2); `Matcher::new(&doc, policy, MatchOptions::new(require_fit, require_reviewed), &*fit)`; candidates judged in windows (JS7) by `judge_step`, components by `judge_component` (CO5); the original if asked; `MatchDocument::with_original`; store `to_json()`, then its candidates index (DC12).
  - **`match`, catalog:** the socket gated again (JB2); a reader `Catalog` from the pool (CX1); a snapshot, JS12's check, and `match_catalog(&matcher, &selection, permits, &observer)` (CA2); drop the snapshot and checkpoint (CC3); `MatchDocument::with_original(&matcher, run.results, original)`; record `run.generation` and the counts (DC10); store the document and its candidates index (DC12).
  - **Catalog writes:** the writer `Catalog` (CX3): `ingest_inputs`, `replace_input` (CA1), `reindex`, `prune_revisions` or `gc` with the observer; the result is the `fitment.catalog`, or `fitment.gc`, document the CLI's `--format json` prints (CU1, CU2).
  - **`catalog_verify`:** `verify(deep, full)` on a fresh reader, under its deadline (JB7); the result is `fitment.verify` (CA5).
  - **A panic:** the closure's `JoinError` (or `CatalogRun.bugs`) fails the job `internal`; tower-http's `CatchPanicLayer` covers handlers. The release profile stays `panic = "unwind"`, which the catalog's own `catch_unwind` already relies on.
- **JS7. A file match's window** (B JB11). A file match judges at most `permits` candidates at a time, each read once into one buffer, and drops them before reading the next window, as the CLI's `judge_all` does (M5-30). Between windows it checks the cancel flag and the deadline, and sends progress.
- **JS8. Cancellation reaches a job through one flag** (B JB12; M6-27). A cancel, a deadline or a shutdown sets the job's flag and moves it to `cancelling`; the job sees the flag where it may stop:

  | Kind | Checks the flag |
  | --- | --- |
  | `features`, `assembly_read`, `socket_extract`, `socket_import` | only when its one core call returns; the call is bounded (the assembly gate's occurrence cap, check 39, SZ1) |
  | `match`, files | between windows; a window's candidates finish |
  | `match`, catalog | before each group a judging thread takes (CA2), which returns `NotRun::Cancelled` |
  | `catalog_ingest` | before each file and each batch commit (CA2) |
  | `catalog_reindex` | before each batch (RX7: resumable) |
  | `catalog_replace`, `catalog_prune` | never: one transaction each |
  | `catalog_gc` | between unlink batches of 1,000 (ST5) |
  | `catalog_verify` | between phases, and with `deep` before each blob (CA2) |

  A job that cannot stop at once keeps its permits until its call returns, so the accounting is never ahead of the truth. Every core call is bounded (`MAX_PLACEMENTS`, `MAX_COVERAGE_STEPS`, `MAX_SHORTLIST_CHECKS`, check 39, SZ1); a core call without a bound is a bug.
- **JS9. Sweeps** (B JB14; M6-23). A background task runs every `jobs.sweep_minutes` (60), each step on the blocking pool: expire uploads; delete terminal jobs past retention and not pinned (their events and inputs cascade); delete tokens 30 days after expiry or revocation; flush the batched `last_used_ms` values (AU6); then unlink objects nothing holds (OB4). It never touches the catalog. The catalog's own gc is a write-lane job, queued daily at `jobs.catalog_gc_at` (off by default) without `versions` or `vacuum`, owned by the `system` user (M6-56).
- **JS10. Two channels per job** (B JB15). Live events go through a `tokio::sync::broadcast` of capacity 1,024, whose `send` never blocks, so progress can never slow or stall a job; persisted events (JB8) are also written as `job_event` rows.
  - **A stream** subscribes to the broadcast first, then replays the rows after `after`, then forwards live events, persisted ones deduplicated by `seq` (transient ones have none and are never replayed), so nothing persisted is lost between replay and live.
  - **A lagging reader** gets one `lagged` line and continues from the next event.
  - **Limits:** 8 streams per user, over all their tokens (review F5), and 256 in all (`429 too_many_streams`). A disconnected client affects nothing.
- **JS11. A job exists once it is durable** (B SD5, merge; review F20). The job row, its inputs and citations (`job_input` for every object it names, `job_cites` for the socket revision it pinned, both under OB4's hold) and its idempotency key commit in one `server.db` transaction before the `202`; `attempt` is written before dispatch. A crash after the commit leaves a queued job that SD4 runs; a crash before it leaves nothing, and the client's retry with the same key creates it once.
- **JS12. An exhaustive run is bounded** (review F3; M6-60). An exhaustive run judges every live entry, and its document grows with them (M5 measured 111 MB for 9,219 candidates). It needs the writer role (AU5); in its snapshot, a catalog with more live entries than `limits.max_catalog_candidates` (100,000) ends the job `refused`, exit 3, `too_large` with `details.limit: "max_catalog_candidates"`, before anything is judged. The cap is a tightening over the CLI (SN3), whose `--exhaustive` has none. A full run judges only level 1's survivors, and a named run's `ids` count against HX2's `max_candidates`.
- **JS13. A job's result and its end commit together** (review F11). For `assembly_read`, `socket_extract` and `socket_import`, the rows that make the result durable (the `assembly_parts` row; the socket revision, and a revived line's row, RA4) commit in one `server.db` transaction with the job's terminal state, `exit_code` and `result`, each row naming the job; the document's object is stored before (OB5). For `features`, `match` and the catalog's jobs, the document's object is stored first and the job row names it in the transaction that ends the job. A re-run after a crash (JB6) first looks for a row naming its job: found, the job ends `succeeded` with that result and does not run again, so a crash never makes two revisions from one job or reports `unchanged` for the revision the job itself made.

### 4.4 Documents and parity (DC)

- **DC1. Result documents are served as the library's bytes** (A DC1, B DT2; C XH4). The body of every response carrying `features.v1`, `fitment.parts`, `fitment.socket`, `fitment.match`, `fitment.catalog`, `fitment.verify` or `fitment.gc` is the `String` the core or the catalog crate's `to_json()` returned, stored as an object (OB5) and written unchanged: pretty JSON with a final newline, `Content-Type: application/json`.
  - **Never:** an envelope, a field added or removed, or a re-serialization through another type, which would sort keys or reformat a float. The server never parses a document into a `serde_json::Value` to write it out (SV7 bans it in `matching.rs` and `objects.rs`).
  - **Envelopes link to documents** and never embed them, which would need serde_json's `raw_value` (M6-34).
- **DC2. Documents carry nothing of the server** (B DT1, A 3; M6-11). No job id, user, time, URL, host or path enters a result document; all of that goes in the envelopes and `server.db`. The server's clock and its hash maps reach only envelopes, logs and metrics. With CA1, no server path reaches a `fitment.catalog`'s `input`, `file_name` or error line.
- **DC3. Parity** (A DC2, B DT4, DT5, C C-2; M6-37). For each endpoint of section 7 that names a CLI command, the body equals that command's stdout for the same input bytes, arguments, labels and build, and `Fitment-Exit-Code` equals its exit code. The inputs map as follows:
  - a file argument is an upload, and the CLI's label (the argument as given) is the request's `label` or `input`;
  - a socket file is a socket revision, whose bytes are a file the CLI reads, or an uploaded socket document;
  - `-C DIR` is the server's catalog, and a catalog entry's label is `catalog:<id>@r<n>` in both;
  - `--jobs N` is the job's permit count, which changes no byte (QR8).

  Where the CLI cannot express a request (component candidates; an ingest naming ids for several files), the reference is the library call with the same arguments and labels, byte for byte: `judge_component` (CO5) and `MatchDocument::with_original`, or `ingest_inputs` (CA1). With the same labels in the same order the bytes are equal; whatever the labels, the digest is (MF7). With CA1, an ingest through the API makes the same ids, the same ID4 order and the same `fitment.catalog` change lines and entries as `fitment catalog ingest` over files of the same names, compared with every generation masked: a generation counts every write the catalog has seen (a reindex at start, a daily gc, a delete, an interrupted batch), which the CLI's catalog has not, so it is the one field ingest parity does not compare (review F9).

  The server's stated tightenings (SN3) have no parity row: where the API refuses and the CLI does not (a review of a stale revision, an exhaustive run over the cap, an import its assembly does not reproduce), no test compares the two (review F16).
- **DC4. Headers** (A DC3). Every document response carries:
  - `Fitment-Format: <format>/<format_version>` (`fitment.match/1`);
  - `Fitment-Exit-Code`, where DC3 applies;
  - `ETag` (VN5);
  - `Repr-Digest: sha-256=:<base64>:` (RFC 9530), the SHA-256 of the body;
  - `Fitment-Digest: sha256:<hex>`, the document's own `digest` field, for `fitment.socket` and `fitment.match`.
- **DC5. Two digests, two meanings** (A DC4). The body's SHA-256 identifies these exact bytes, labels and review included: it is what `fitment.run` and the job record store. The document's `digest` identifies the result: the same on every OS, without labels (MF7) or the review (M3 9.3); it is what a client compares across runs. Neither protects anything: no endpoint takes either as proof of a verdict (SN6).
- **DC6. Compression is transport only** (A DC5): `Content-Encoding: gzip` when the client asks; the ETag of a compressed representation gets a `-gz` suffix (RFC 9110 8.8.1). Request bodies are never decompressed: a `Content-Encoding` on a request is `415 unsupported_media_type`.
- **DC7. The policy as the CLI builds it** (A 5.7, A 15, B DT3; M6-48). A match's `policy` has the CLI's flags as fields: `tolerance_mm`, `tolerance_deg` and `clearance` (`strict` or `functional`); grip is hard (D7) and has no field. Omitted fields take D9's and D6's defaults. A tolerance that differs from D9's builds a `TolerancePolicy` named `cli`, exactly as `cmd/match_.rs` does, so the document's `policy.name` and digest equal the CLI's; naming it `api` would be a format change for no gain. `options` holds `require_fit` and `require_reviewed`. The CLI's `--jobs`, `--rejects`, `--explain`, `--format` and `-o` have no counterpart: the first is the server's (JS2), the rest shape a table the API does not serve.
- **DC8. One build** (B DT7). The server and the CLI in an image come from one workspace version and one `Cargo.lock`; serde_json's features in the runtime graph are `deny.toml`'s exact three (SV9), so the JSON writer cannot differ between them.
- **DC9. The documents the API adds** (A 8.2; M6-10, M6-11). `fitment.job`, `fitment.job.event`, `fitment.run`, `fitment.error`, `fitment.page`, `fitment.info`, `fitment.upload`, `fitment.assembly`, `fitment.token` and `fitment.user` (section 7.11) each have `format` and `format_version` first, snake_case keys, `null` written rather than omitted, and pretty JSON with a final newline, as the frozen documents do. What the server reads has `deny_unknown_fields`. None holds a path or a host; only `fitment.job` and `fitment.token` hold times. Each may gain optional fields within v1 (VN1).
- **DC10. `fitment.run`, the stored record of a match** (A DC6, B M6B-11; M6-25). It says what M5 deferred to M6 (M5-2: "the document alone does not say how much of the catalog was searched"): the socket revision or upload and its SHA-256 and digest, the candidates (for a catalog run: the generation, entries, searched, quarantined and not searched, level-1 survivors, named, judged, distinct judgments; for a file run: files, components, distinct), the policy and options with `require_reviewed_by`, the versions, the fit verifier, the summary and outcome, `exit_code`, `complete`, the problems (corrupt rows, bugs, the original) and the document's link, SHA-256, size and digest. `summary` and `outcome` are copied from `MatchDocument` (`summary()`, `outcome()`), never recomputed (SN4), for a job that succeeded; otherwise both are null and the counts of what was judged are under `problems.incomplete_summary` (M6-59). The socket's `how` and `verified` are the revision's (RA10). It holds no time, so two servers' records of one run differ only in ids.
- **DC11. `fitment.info`** (A DC7, B SV5). It states the server's version; every version constant (`EXTRACTOR_VERSION`, `SOCKET_RULES_VERSION`, `SIGNATURE_VERSION`, `MATCH_RULES_VERSION`, `PROPERTIES_VERSION`, each format's version, the catalog schema, the `server.db` schema); the API's `{major: 1, minor}`; `fit_verifier` (`null` until M7, then the kernel and version `FitVerifier::kernel` names); the catalog's generation, counts and state (`stale`, a reindex queued or running, `wal_barrier`, `queues_full`); and the limits (upload cap and minimum throughput, socket document cap, JSON body cap, candidates per run, the exhaustive cap, page size, `Prefer: wait` cap, `require_reviewed` floor, upload lifetime, live tokens per user). A client checks it before comparing documents across servers.
- **DC12. The candidates view slices the stored bytes** (review F2, F3; M6-8, M6-59). When a match job stores its document, it builds the document's candidates index once, in one pass over the bytes: for each element of `candidates`, its byte range and the SHA-256 of those bytes. The index is checked to cover the array exactly, in order, with nothing between elements but the writer's separators, and is stored as an object held with the document (OB4).
  - **A page** reads the index (checked as OB3 says), then the ranges it needs, each checked against the SHA-256 the index records for it, and writes `fitment.page` by hand around them: each item is `{"index": i, "entry": <the bytes of candidate i>}`, the document's own bytes, so nothing is parsed or re-serialized and no `raw_value` is needed (M6-34). A range that fails its check is `500 blob_damaged`, as OB3.
  - **Every page states** the document's `digest`, `view: true`, and the run's `complete` and `exit_code`, so a client paging a failed run's kept document reads on every page that it is not a result (M6-59).
  - **No page parses the document,** whatever its size: a 111 MB document costs one pass at the job's end, then only the bytes each page sends.

### 4.5 Uploads (UP)

- **UP1. Bytes enter only through uploads** (A UP1). Every request that needs STEP bytes or a socket document names an upload by `sha256:HEX`; no endpoint takes file bytes inline in JSON, and none takes a path.
- **UP2. Content-addressed, hashed and capped as they stream** (A UP2, UP4, B OB7; M6-15, M6-16). An upload body is consumed as a stream (`Body::into_data_stream`). Each chunk is counted, fed to CO1's hasher and written to `tmp/<random 128-bit hex>.part` through CA7's streaming writer (created new, mode 0600), so the `Fs` seam sees every call; at the end, the file is synced, then UP8 and OB2.
  - **`PUT /v1/uploads/sha256:{hex}`** keeps the bytes only if their SHA-256 equals `{hex}`; otherwise `422 upload_digest_mismatch`, and nothing is kept. **`POST /v1/uploads`** hashes and names, for a client that cannot hash first. Hex is accepted in either case and written lower-case.
  - **The cap** is `limits.max_upload_mib`, 128 by default, equal to the catalog's `Options::max_bytes`, so a file the server accepts is never refused by ingest for size (SZ1). A declared `Content-Length` over the cap is `413 too_large` before a byte is read; a chunked body is cut at cap + 1 bytes, as SZ1 reads with `take(cap + 1)`, and its temporary file deleted.
  - **Aborts:** a client abort, a body error, an idle timeout or a throughput below the minimum (HX3) deletes the temporary file.
- **UP3. A hash grants nothing** (B OB4; M6-15). An upload is a row (owner, SHA-256) over a shared object; a request names `sha256:HEX` and it resolves only among the caller's own unexpired uploads and the inputs held by the caller's retained match records (OB4, M6-64), so a retained run can be submitted again unchanged. An admin's `?owner=` reads another user's upload metadata and never names another's upload in a job (AU8).
  - **`HEAD /v1/uploads/sha256:{hex}`** answers `200` only for bytes the caller may name, else `404`, so a client that hashed locally skips the transfer of its own bytes only.
  - **Bytes another user already stored** are answered exactly as new bytes: `201` and a new row for the caller, after the same work as for new bytes (UP8), so neither the status nor the time tells. A second upload of the caller's own bytes is `200` and extends its row's lifetime. **Review (PR 4):** the same holds for `DELETE`: it removes only the caller's row and never unlinks the object, which only the sweep does (OB2, OB4), so its time tells nothing of whether another user holds the bytes.
  - **The catalog's bytes** are not uploads: catalog content is shared and read through the catalog's own endpoints (RA1).
- **UP4. Quotas before bytes** (B OB8; review F3, F5). At most `limits.upload_concurrency` (8) uploads at once per server and 2 per user, over all their tokens; a staging quota (`limits.staging_gib`, 8, the sum of open `.part` files) and a per-user quota (`limits.user_quota_gib`, 10) are checked before reading, with `507 quota_exceeded`. The per-user quota counts the caller's unexpired uploads and the documents and held inputs of the caller's retained jobs (OB4); it is also checked when a job is submitted, so a user at the quota creates no job. A running job's document is stored even if it crosses the quota: a run never fails for quota after judging. A disk-full error while writing is `507 storage_full`, the temporary file removed.
- **UP5. Staged, then held** (A UP5, B M6B-9; M6-23). An upload no row holds expires `limits.upload_ttl_hours` (24) after its last upload. Creating a job holds every upload it names under OB4's hold (JS11); registering an assembly, extracting or importing a socket and ingesting into the catalog make the bytes durable where they belong (an assembly row, a socket revision, the catalog's own store). An expired upload is `404 upload_not_found` when a job is requested, never mid-job. `DELETE` of an upload a job names is `409 upload_in_use` while the job is not terminal.
- **UP6. A socket document is read as `fitment match` reads a file** (A UP7, B OB9; M6-14; review F3, F15). An uploaded `fitment.socket` that is imported or matched is read from at most `limits.max_socket_mib` (16) bytes (`413 too_large` above), behind HX8's parse semaphore: MF11's version check first, then strict `SocketDocument::from_json` on the exact bytes, then MF2, before any row is made or any candidate is read. It must be UTF-8 (`422 socket_unreadable` otherwise). A `carry_from` document is not read this way: it is read leniently through `SocketDocument::prior`, as the CLI reads an older OUT, from at most the same number of bytes, since an older version is what it is for.
- **UP7. Inputs are read checked** (B OB10). A job reads each input object through OB3 (hashed against its name before use). A socket job calls `SocketDocument::extract` on the assembly's bytes; a file match reads each candidate when its window comes (JS7); a component match reads its assembly once. Catalog ingest and replace hand the verified bytes to CA1, and the catalog hashes its own buffer again (ST2); a SHA-256 that differs from the upload's fails the job `internal`.
- **UP8. Both branches of an upload do the same work** (review F4; M6-15). Whether or not an object of those bytes exists, an upload streams, counts, hashes, writes and syncs the whole body in `tmp/` (UP2); then, inside one `server.db` `BEGIN IMMEDIATE` (so a sweep cannot unlink the object between the check and the row, OB2), it renames the temporary file over the object's final name whether or not the object exists (**Review (PR 4):** an atomic replacement of identical bytes, which also repairs a missing or wrong file there; removing it in one branch left the fan-out directory clean there and dirty in the other, a difference of time), syncs the fan-out directory, which the state made with all 256 so neither branch makes one, and inserts the rows, the object's with `ON CONFLICT DO NOTHING` in both. The existing object is never read or re-hashed on this path; OB3 checks it whenever it is read. The answer's status and headers are the same in both branches, and under `CountingFs` the two make the same calls in the same order (VT48); on Windows, where a read-only file cannot be renamed over, the branch that finds the object clears its flag first, its one extra call.

### 4.6 The server's state (OB)

- **OB1. `server.db` is the catalog's kind of database** (B OB1; M6-3, M6-32). Its own `application_id` (`0x666D7453`, `fmtS`), a schema version refused when newer and migrated forward in one `BEGIN EXCLUSIVE`, STRICT tables, WAL with `synchronous=FULL` (and `fullfsync` on Apple, as ST9), `foreign_keys=ON`, `trusted_schema=OFF`, every statement prepared with bound parameters, every write `BEGIN IMMEDIATE`.
  - **Connections:** one writer behind a `std::sync::Mutex`, and a fixed pool of readers (eight by default).
  - **When it is written:** submissions, state changes, uploads, revisions, sweeps; a request never writes just to authenticate (AU6).
  - **The `system` user** (M6-56) is created with the schema; it has no password, cannot log in, and a trigger refuses a token for it.
- **OB2. Objects follow the catalog's store rules** (B OB2). Each object is named by its SHA-256, under `objects/sha256/<2 hex>/<64 hex>`, mode 0444 (ST1, ST4).
  - **Writing:** `create_new` in `tmp/`, synced, renamed, and the fan-out directory synced before the row that names the object commits (ST3, M5's F6); a final name already present is renamed over without being read (review F4; review of PR 4): OB3 checks an object whenever it is read, and the upload path does the same work either way (UP8). Whatever a rename cannot replace is cleared first, as the catalog's blobs do: a directory removed, another non-regular entry unlinked, and on Windows a regular file's read-only flag cleared.
  - **Unlinking:** only inside a `server.db` `BEGIN IMMEDIATE` that has just found no row naming the object (ST5), at most 1,000 per transaction, and only by the sweep (**Review (PR 4):** never by a `DELETE`, UP3). Every column naming an object is indexed, and a trigger makes an object whose naming row goes a candidate, so the sweep looks at candidates only, in short batches, never at the whole store (section 6.2).
  - **The code:** the protocol is copied into `objects.rs` against `fitment_catalog::fs::Fs`, which is public and, with CA7, streams, so every file call of an upload, a stored document and a read goes through the seam, and the catalog's `CountingFs::failing_at` fails each in turn (VT11; review F14). Exposing `fitment_catalog::blobs` instead would widen a published crate.
- **OB3. Bytes leave the store only checked** (B OB3; ST11). An object of at most 64 MiB is read into one buffer (`Fs::read_capped`) and hashed against its name before it is used or sent; a larger one is hashed in one pass and streamed in a second, both through CA7's streaming reader. A mismatch stops before the first byte is sent: `500 blob_damaged` and a log line naming the object by hash, never its path. `ETag` is the SHA-256; `Cache-Control` follows VN5 (`immutable` for stored bytes named exactly, `no-cache` for derived documents).
- **OB4. What holds an object, and for how long** (B OB6, A RA6; M6-23, M6-64; review F20). One hold window, stated here and cited everywhere else: a job's `job_input` rows (every object it names) and `job_cites` row (the socket revision a match pinned) are written at its submission (JS11) and hold what they name until the job is terminal. A non-match job's `job_input` rows are deleted in the transaction that ends it; a match's stay with its record and hold for as long as the record is retained or pinned (JB12), so a retained run can be re-run on its own bytes and audited (SN6). An object stays while any of these names it: an unexpired upload; an assembly row not deleted (its bytes and its `fitment.parts` documents); a socket revision of a live line, or of a deleted line a `job_cites` row names; a `job_input` row; a retained or pinned job's document and candidates index (DC12). JS9's sweep deletes expired rows, then unlinks objects no row names, under OB2's unlink rule.
- **OB5. Documents and revisions are objects and insert-only rows** (B OB9). Every document a job or a review produces is stored as an object before the row that names it commits; socket revisions are inserted, never updated (a trigger refuses any `UPDATE` of `socket_revision`). A review applies `SocketDocument::decide` to the stored document, in order, all or none, and stores `to_json()` as a new object and revision whose parent is the revision reviewed. It is synchronous, on the blocking pool, behind HX8's parse semaphore, since it is small and bounded.

### 4.7 Errors (ER)

- **ER1. One enum, stable codes** (A ER1; C's assumption 5; M6-4). `ErrorCode` is one closed enum, enumerable as `ErrorCode::ALL`, each with its class, HTTP status, CLI exit code and `retryable` flag (section 7.12). A code never changes meaning; codes are added within v1, and a client that meets an unknown code acts on `class`.
- **ER2. Four classes** (A ER2), three of them the CLI's (CL2, VD14): `usage` (exit 2: the request is wrong), `refused` (exit 3: the input or the catalog's state is refused), `error` (exit 1: the server could not do it, or fitment has a bug), and `auth` (no CLI counterpart: `exit_code` null). An error's `exit_code` is the code the CLI exits with in the same situation, section 7.12's CLI column: its class's, but for `upload_not_found` (1: the CLI cannot read the file) and `catalog_unavailable`, which takes the CLI's code for its reason (review F25): 3 for a newer schema (DB5), 2 for a directory that is not a catalog, is linked or is not Unicode (the CLI's `NotACatalog`, `Linked` and `NotUnicode`), 1 when it cannot be read; `null` where the CLI has no counterpart.
- **ER3. No path, no library text** (A ER3, B HX7, CX7). `title`, `detail` and `details` are built from the code and from values the client sent or the API defines: ids, revisions, SHA-256s, counts, versions, requirement ids. Never sent: a server path, an SQL statement or SQLite message, the `Display` of a library error (`fitment_catalog::Error`'s `NotACatalog`, `NotEmpty`, `Linked`, `NotUnicode`, `InsideCatalog` and `Io` name paths), an internal key (`blob.key`, `entry.key`, `user.key`, `job.key`), a stack trace, a token or a password hash. The full error is logged under the `request_id`, and an `internal` answer carries only the code and that id. CLI hints ("run fitment catalog verify -C DIR") become endpoint hints. Client-supplied text appears only after validation, as JSON strings.
- **ER4. Refusals keep their own codes** (A ER4). Gate refusals inside a document (`RefusalCode`'s names) are part of the document, not of an error. Where an error carries them (`file_refused`, `socket_refused`), `details.refusals` is `[{code, message, instance}]` as `features.v1` writes them.
- **ER5. Order** (A ER5). When several errors apply, the first in the CLI's order is reported: usage before anything is read, then MF2's socket gate, then QR1's catalog refusals, then errors over every verdict (VD14, QR1, CM9).
- **ER6. `retryable`** (B CX3, merge; review F10, F19). Every error states whether the same request may succeed later unchanged: true for `catalog_busy`, `queue_full`, `too_many_jobs`, `too_many_streams`, `rate_limited`, `upload_too_slow` and `shutting_down`, each answered with `Retry-After` where it is an HTTP response; false otherwise, with two codes whose flag depends on where they arose (`ErrorCode::retryable` takes the route or the job's kind):
  - **`request_timeout`** is retryable on `GET`, `HEAD` and an upload's `PUT` only. On any other route the work may have been done when the answer was cut (a review committed, a job created), so the client reads the resource, or resubmits a job with the same `Idempotency-Key` (JB9).
  - **`job_interrupted`** is retryable but for `catalog_replace` and `catalog_prune`, which the client checks first (JB6).

  A job that failed with a retryable code releases its idempotency key (JB9).
- **ER7. Status choices** (A 10.2). `400`: the request's form. `404`: names nothing the caller may see. `408`: an upload slower than the minimum (HX3). `409`: well formed, but conflicts with the server's current state (fixed by acting on the server). `410`: the thing existed and its bytes are gone. `412`: a precondition failed (VN4). `422`: the input was read and is refused for what it says (fixed by changing the input). `428`: a revision is required (RA7). `429` and `503`: try again later. Per-file outcomes of an ingest (`conflict`, `duplicate`, `quarantined`, `refused`, `error`) stay lines of the `fitment.catalog` document, as in the CLI; only the job's `exit_code` sums them. A `refused` or `failed` job's `error` is the same object without the HTTP framing; codes whose HTTP column is "—" occur only there.

### 4.8 Versions, OpenAPI, pages and caching (VN)

- **VN1. `/v1` is the major version** (A VN1; M6-10). Within v1 only additions: endpoints, job kinds, optional request fields whose absence keeps the old behaviour, error and note codes, fields of the envelope documents. Anything else is `/v2`, served beside `/v1` for a stated period. `fitment.info` reports `api: {major: 1, minor: N}`, the minor rising with each addition.
- **VN2. The result documents' versions are not the API's** (A VN2). A build that raises `EXTRACTOR_VERSION` or `MATCH_RULES_VERSION` serves other bytes under the same URLs; documents record their versions, and `fitment.info` states the build's. A format version change of a result document (none is planned) would need a new endpoint or `/v2`.
- **VN3. Pagination is keyset and deterministic** (A VN3; M6-8, M6-55). Each list has one order over a unique key:
  - catalog entries by id case-folded, then by its bytes (CL3);
  - assemblies by SHA-256; socket lines by id; revisions by number;
  - jobs and matches by (creation time, id); tokens by id; users by folded username;
  - a match's candidates by their index in the document (MF5's order).

  `limit` defaults to 100, at most 1,000. The cursor is opaque (base64url of the last key's public fields, never an internal key). `fitment.page` holds `next`, a URL or null, and every list also sends `Link: <…>; rel="next"`. The catalog list's body is `fitment.catalog` v1, which cannot gain a `next` field, so its cursor travels in `Link` alone; with `limit=all` its body is exactly the CLI's `list --format json`, built whole in memory inside one snapshot, which then closes, and sent from memory, so a slow client never holds a read transaction open (CC3); HX8 caps how many are built at once (review F22).
- **VN4. Pages and the generation** (A VN4, B CX2). Each catalog page is its own snapshot (DB4) and states its generation (the document's own `generation`; `fitment.page`'s for other lists that depend on it). A client that needs every page from one state sends `If-Match` with the first page's ETag; a page of another generation is `412 precondition_failed`, and the client starts over. A server cannot hold a SQLite snapshot open between requests (CC3: the WAL would grow), so consistency is checked, not provided.
- **VN5. ETags** (A VN5; review F6, F24).
  - **Stored bytes** (an exact revision's or content's STEP bytes, socket revisions `@rN`, match documents, assembly bytes): strong ETag `"sha256:<hex of the body>"`; when the URL names an exact revision or content, `Cache-Control: private, max-age=31536000, immutable`, otherwise `no-cache`.
  - **Derived documents** (`features` of any catalog reference, an assembly's `…/parts`): strong ETag `"sha256:<hex of the body>"` and always `no-cache`, since the same URL serves another build's reading after an upgrade.
  - **Catalog views** (`/v1/catalog`, entry lists, `get` by bare id or `sha256:`): strong ETag `"<instance>.g<generation>.<build>"`, `<instance>` the catalog's instance id (CA8, M6-63), so two catalogs at one generation never share an ETag, and `<build>` the first 12 hex of the SHA-256 of the server version and every version constant, because the same generation read by another build can list another `status` (RX2 is computed on every read). Sound because a catalog document holds nothing but the catalog's state and the build's versions (CL9).
  - **Jobs:** `W/"j<seq>.<state>"`, from the job's last persisted event (JB8) and its state, for polling with `If-None-Match`; a progress event changes no ETag.
  - `If-None-Match` answers `304`. Writes do not use `If-Match` (RA7).
- **VN6. Request conventions** (A VN6). JSON bodies are UTF-8 without a byte-order mark, snake_case, unknown fields refused, duplicate keys refused (never "last wins"), numbers finite, `-0.0` refused where a tolerance is asked; any of these is `400 bad_request` or `invalid_parameter`. Booleans default false and lists empty, as the CLI's flags do.
- **VN7. OpenAPI 3.1, committed and served as bytes** (A VN7, A5, B M6B-13; M6-6). `crates/fitment-server/openapi.json` is generated by a test (`tests/openapi.rs`) from the server's `OPERATIONS` table (method, path, role as `x-fitment-role` and any field that raises it as `x-fitment-role-fields` (AU5), mode as `x-fitment-mode`, request and response types, error codes) and from schemars derives: the result documents' types behind `fitment-core`'s and `fitment-catalog`'s `schema` features (CO4, CA6), the server's own types behind its `schema` feature. The test fails when the generated document differs from the committed one, so an API change is a reviewed diff; it rewrites the file only under `FITMENT_BLESS=1`, never under `CI`, and only when `info.version` (the API's `1.<minor>`) rose. The server serves the committed bytes, compiled in with `include_bytes!`, at `/v1/openapi.json`. Each result schema's `$id` names its format and version, and a test validates every document the corpus, the goldens and the fixtures produce against its schema (VT36).
- **VN8. Content negotiation is minimal** (A VN8). Each URL has one representation; an `Accept` that excludes it is `406 not_acceptable`. There is no `?format=` and no table rendering (CL3: tables are for people and are not stable).

### 4.9 Soundness through the API (SN)

- **SN1. One judging path** (A SN1, B DT3). Every verdict comes from `Matcher::new` and `judge_step`, `judge_component` (CO5) or (in a catalog run, inside `match_catalog`) `judge_document`, assembled by `MatchDocument::new` or `with_original`, exactly as the CLI calls them, with the policy built by the same constructors (DC7) and the fit verifier the server was built with (SV8). The server constructs no `Verdict`, no `CandidateResult` and no `fitment.match` of its own, and never uses `CandidateSource::Given` (CM7: it would tie in MF5's order). `matching.rs` is the only module that builds a `Matcher` or a `MatchDocument` (SV4).
- **SN2. No override of a refusal** (A SN2). No request field skips MF2, VD9, RX3, QR1 or CR8: a stale socket, an unreviewed socket under `require_reviewed`, a stale or empty catalog, a blocked socket in a full run and a quarantined entry in a full run are refused exactly as the CLI refuses them, in its order (ER5). The API has no `force`, `skip_stale` or `audit` field.
- **SN3. The server may only tighten** (A SN3; M6-12). Configuration can force `require_reviewed` and lower caps; it can never relax a refusal, raise a tolerance or change a default the document records. What it forces is recorded in the document's `options` and in `fitment.run`'s `require_reviewed_by` (`"request"` or `"server"`). The API's tightenings over the CLI are these, each stated where it is made and kept out of parity (DC3): the `require_reviewed` floor; a review of a stale revision, refused `422 socket_stale` though the CLI's `fitment review` accepts one, because a stale revision cannot be matched and extracting again carries its decisions (review F16); the exhaustive cap (JS12); an import its assembly does not reproduce (RA10).
- **SN4. Verdicts are read, never derived** (A SN4). `fitment.run`'s `summary` and `outcome`, and the job's `outcome` and `exit_code`, are copied from `MatchDocument::summary()` and `outcome()` and CO2's mapping, for a job that succeeded; a job in any other state has a null `outcome` everywhere (JB3, M6-59). Nothing collapses `InterfaceMatch` into a pass: there is no boolean "fits" field anywhere, and the three kinds appear by their names. Without a fit verifier, every run states `fit_verifier: null` and the summary note ends "fit not checked (no fit verifier in this build)" (VD4).
- **SN5. Flags and evidence travel whole** (A SN5). The document is served unchanged (DC1), so `thread_not_confirmed`, `material_not_compared`, `requirements_removed`, `requirements_unreviewed`, `larger_than_original` and every removed requirement's note reach the client as the CLI prints them. The candidates view (M6-8) is the document's own bytes of whole entries (DC12), never a filtered or re-serialized entry.
- **SN6. Reports are history, never input** (A SN6; M4-23). No endpoint reads a `fitment.match` or `fitment.run` as input, compares a client-supplied digest to decide anything, or turns a stored record into a verdict. A record names the socket revision, the candidates' SHA-256s and the catalog generation it judged, and while it is retained it holds that revision and every candidate's bytes (OB4, M6-64), so it can be re-run and audited; a catalog run's entries are read again from the catalog, which keeps a revision's bytes until `prune` (M5-41).
- **SN7. No partial result** (A SN7, B DT6; M6-51). A cancelled, interrupted or timed-out match stores no document (JB5, JB6, JB7), and its events carried no verdict (JB8). A run the CLI would end with exit 1 keeps its document but is `failed`, its `fitment.run` says `complete: false` with a null `outcome` (M6-59), and its `Fitment-Exit-Code` is 1. A partial catalog change document is a complete statement of what was committed, each line a fact the catalog holds.
- **SN8. Unreviewed is visible** (A SN8). Every `fitment.run` states the count of unreviewed requirements of the socket judged; the job's notes carry the CLI's warnings (one per unverifiable requirement, per empty socket, per socket failing every candidate). A socket's review changes only through `decide` (values are never editable: there is no `PUT` or `PATCH` of a socket document), and a removal needs its note (M3 9.4).
- **SN9. The original is a check, not a candidate** (A SN9; M6-5). `original: true` judges the target read from the socket's own assembly, which the caller must have registered (`409 original_not_stored`), beside the candidates (VD15). No request can name another assembly, so VD14's "another file than the socket's source" cannot arise. Its failure beyond VD5 and VD6 fails the job `original_fails_own_socket`, with the evidence kept in the document; an original whose assembly cannot be read fails it `run_incomplete`, exit 1 (CO5).
- **SN10. Catalog runs are file runs, restricted** (A SN10; M5 4.17). The server calls `Snapshot::match_catalog` in one snapshot, records the generation and CM8's counts (DC10) and labels candidates `catalog:<id>@r<n>`; `GET /v1/catalog/entries/<id>@r<n>/step` returns the bytes judged while they are kept.
- **SN11. `Match` only with a verified fit** (C's assumption 12, B OP9; D1). The server judges with the `FitVerifier` its configuration builds at start, `NoFitVerifier` in M6 (`fit.verifier = "none"`), so `Match` cannot occur and the best verdict is `InterfaceMatch`. A fake verifier exists only under `test-support`; neither `fitment-server --help` nor the OpenAPI document names a way to choose one.

### 4.10 The catalog in the server (CX)

- **CX1. One `Catalog` per concurrent user of it** (B CX1). `Catalog` is `Send` but not `Sync` (it holds a rusqlite `Connection`; `Fs: Send + Sync`), and `Catalog::snapshot` takes `&mut self`, so one catalog serves one snapshot at a time.
  - **Readers:** a pool of `query_slots + read_concurrency + 1` reader catalogs, each opened with `Catalog::open`, checked out by one blocking task and returned after it. Opening a fresh one is cheap (marker, link checks, pragmas) and is the fallback when a pooled one returns an error.
  - **The job writer:** exactly one writer catalog, owned by the write lane.
- **CX2. Each read is its own snapshot** (B CX2). A catalog match reads everything in one snapshot (QR2) and reports its generation, as the CLI does; a `list` or `get` request is one snapshot too (DB4). The server never spreads one logical read over two snapshots: each page of a list is its own snapshot and states its generation (VN4).
- **CX3. Writers** (B CX3, merge; M6-50). Each write job takes the write lane, then `BEGIN IMMEDIATE` through the catalog (DB2). The one other in-process writer is a synchronous delete, which opens a short-lived writer `Catalog`, runs one transaction, and closes it; it waits at most for a write job's current batch (≤ 64 files or 1 s, DB3).
  - **`SQLITE_BUSY`** can therefore come only from a running batch, from another process on the same catalog (the CLI, another server, a backup tool), or from WAL recovery.
  - **Waiting:** write jobs open their writer with `catalog.write_busy_ms` (60 s, the catalog's own busy timeout); a job that still cannot get the lock fails `catalog_busy` (retryable) and holds the lane while it waits, which is the right back-off since no other in-process job writer could have run. A synchronous delete waits at most `catalog.delete_busy_ms` (10 s, under the handler timeout), then answers `503 catalog_busy`.
- **CX4. Reads never wait long** (B CX4). Reader catalogs are opened with `catalog.read_busy_ms` (5 s). A WAL reader takes no lock writers hold, but can meet `SQLITE_BUSY` while the WAL index is rebuilt after a crash or during a `TRUNCATE` checkpoint. `Error::Busy` from a synchronous read is `503 catalog_busy` with `Retry-After: 1`; in a match job it fails the job `catalog_busy`. It is never a 500 or a hang.
- **CX5. Snapshots per job, and changes beside them** (B CX5; M6-28).
  - **Snapshot isolation (CR7):** a match running while ingest, replace, delete or prune commit judges the snapshot it began with and records its generation.
  - **`gc --versions`** deletes only older versions' rows where current ones exist (RX8); a match reads only current rows, so it never removes what a running match reads.
  - **An unfinished reindex** (after an extractor or signature rise) leaves catalog matches refused as stale (RX3). The server does not paper over it: such a job ends `refused` with `catalog_stale` and the counts, and `fitment.info` says whether a reindex is queued or running.
  - **Named and exhaustive runs** follow RX3 as the CLI's do.
- **CX6. WAL pressure and barriers** (B CX6; CA3). After every match, the catalog's PASSIVE checkpoint (CC3), and the write lane runs one after each committed batch.
  - **The barrier:** when CA3 reports the log above `catalog.wal_barrier_mib` (256) and a checkpoint cannot complete because readers pin it, the scheduler stops dispatching query jobs; running ones finish, the writer runs `checkpoint_truncate()`, and dispatch resumes.
  - **Who else takes it:** `gc` with `vacuum`, on the write lane, so `VACUUM` never waits forever behind overlapping readers. `verify --deep` does not: it is a reader under its deadline (JB7; review F23).
  - **The bound** (review F3): a barrier pauses dispatch for at most `catalog.barrier_max_seconds` (60). If running queries still pin the WAL then, dispatch resumes without the truncate, the barrier is tried again no sooner than `barrier_max_seconds` later, and `fitment.info`'s `wal_barrier` flag stays set; a `VACUUM` that could not take it fails `catalog_busy` (retryable). No queued job waits longer than that bound for one barrier, and none is failed by it.
- **CX7. The catalog's errors, mapped** (B CX7, A ER3). `fitment_catalog::Error` is mapped variant by variant in `catalog.rs`; its `Display` is never sent:

  | Error or result | Code |
  | --- | --- |
  | `Busy` | `catalog_busy` |
  | `Newer` | `catalog_unavailable`, `details.reason: "newer_schema"`, exit 3; at start the server refuses to run (SV5) |
  | `NotACatalog`, `Linked`, `NotUnicode` | `catalog_unavailable` with its reason and the CLI's exit 2 (ER2), logged with the path written relative to the configured catalog root (OP1), never answered with it |
  | `Corrupt` | `catalog_damaged` |
  | `Io`, `Database` | `io_error`, with the request or job id |
  | `NotRun::Stale`, `Empty`, `Blocked` | `catalog_stale`, `catalog_empty`, `socket_blocked` (refused, exit 3) |
  | `NotRun::Unnamed` | `entry_not_found`, `revision_not_current`, `entry_deleted` (refused, exit 2) |
  | `NotRun::Cancelled` | the job's `cancelled` state, no error |
- **CX8. The server never repairs the catalog behind the user's back** (B CX8). It runs gc, prune, vacuum and reindex only as jobs someone submitted, or as configured (M6-28 and JS9's daily gc), and each records its `op` rows as the CLI's do (DB8).

### 4.11 Authentication and roles (AU)

Ported from Vectera's `catalog/src/auth.rs` and `commons/src/auth.rs` (MIT, same author), with the logic copied and the actix extractors rewritten for axum: argon2id with a pepper and a 12-character minimum; 256-bit opaque tokens stored as SHA-256 and shown once; prefixed personal access tokens; the role hierarchy and its `permits` table; typed permission extractors (`Reader`, `Writer`, `Admin`); one answer for an unknown user, a wrong password and a disabled user; a bootstrap admin. Fixed (the first five are the audit's section 3 flaws; B found the rest reading the code):

| # | Flaw in Vectera | Fix |
| --- | --- | --- |
| 1 | Unknown usernames return before argon2 runs, so timing shows which exist | AU2: a dummy PHC hash with the same parameters is verified for unknown and disabled users |
| 2 | Only admins can create personal access tokens | AU3, AU5, AU10: every user mints and revokes their own, capped at the presenting token's role and expiry, at most 20 live |
| 3 | The bootstrap password is written to the log | AU9: printed to the bootstrap command's stdout only; the server logs "admin created", never a secret |
| 4 | Expired sessions are never cleaned up | No sessions in M6 (M6-18); expired and revoked tokens are swept (JS9) |
| 5 | One or two database writes on every authenticated request | AU6: `last_used_ms` batched in memory, flushed by the sweep; authentication is one indexed read |
| 6 | The pepper is appended to the password bytes | AU1: the pepper is argon2's `secret` (`Argon2::new_with_secret`), keyed hashing as the algorithm defines it |
| 7 | Password length checked in bytes, with no upper bound | AU1: 12 to 256 characters, at most 1,024 bytes, refused before hashing |
| 8 | argon2 runs on the async executor | SV3, AU2: on the blocking pool, at most 2 hashes at once |
| 9 | `Argon2::default()` parameters, an unrecorded policy | AU1: explicit parameters, carried in the PHC string, rehashed at login when they change |
| 10 | No rate limit on login | AU7 |
| 11 | Tokens never expire unless asked | AU4: every token expires (90 days by default, at most 365) |

- **AU1. Passwords** (B AU1). argon2id, version 0x13, m = 19 MiB, t = 2, p = 1 (OWASP's minimum); a random salt per password; the pepper as argon2's secret, read from `auth.pepper_file` (mode 0600 checked). Accepted: 12 to 256 Unicode scalar values, at most 1,024 bytes, compared as given, byte for byte (no Unicode normalization, which would need a dependency: a password typed in another normal form does not match, by decision of PR 3's review). A verification against older parameters rehashes and stores the new PHC string. Without a pepper the server starts, with one warning, and the offline commands warn whenever they hash without one. Rotating the pepper is not supported in M6: every password would have to be set again. Measured in the prototype: 20.9 ms for one hash and one verification.
- **AU2. Login costs the same for every username** (B AU2). `POST /v1/auth/login` runs one argon2 verification whether the username exists, is disabled or has no password; a dummy PHC string, made at startup with the same parameters, stands in. At most `auth.hash_concurrency` (2) verifications run at once on the blocking pool; the rest wait in a fair queue for at most 10 s (`503 request_timeout`). Every failure is `401 invalid_credentials`.
- **AU3. Tokens** (B AU3; M6-18). `fitment_pat_` followed by base64url (no padding) of 32 bytes from `getrandom`; the prefix lets secret scanners and logs recognize one.
  - **Stored:** `secret_sha256`, core's `sha256_hex` of the whole string, with a unique index; lookup is by that hash, which needs no constant-time comparison (a preimage-resistant hash of 256 random bits).
  - **The plaintext** is returned once, at creation (`fitment.token`'s `secret`), and is never logged or stored; in memory it is a `Secret` whose `Debug` and `Display` print `fitment_pat_…<hint>`.
  - **Presented only** as `Authorization: Bearer <token>`. A token in a query string is refused, since it would reach logs and proxies.
- **AU4. Expiry and revocation** (B AU4). Every token has an expiry, 90 days by default and at most `auth.token_max_days` (365). Its owner or an admin revokes it, and revoking a token revokes every token minted from it, transitively, in one transaction (AU10's `parent`). A password change through `POST /v1/auth/password` revokes every other token of the user, and an offline `user reset-password` (AU9) every one (review F1). Expiry and revocation are checked on every request from the database, with no cache, so a revocation or a disabled user takes effect at the next request. Disabling a user revokes every token of theirs; enabling restores none (PR 3's review). A password change re-checks, in the transaction that sets it, that its presenting token is still live and the password the one verified, so of two concurrent changes one wins and the other's token, revoked by the winner, is `401 unauthenticated`.
- **AU5. Roles** (B AU5, A 12.3; M6-17).

  | Role | Can |
  | --- | --- |
  | reader | read the catalog (`list`, `get`, features, stored bytes, history) and `fitment.info`; upload; register assemblies; run `features`; extract, import, review and delete their own sockets; run matches, full and named on the catalog; see and cancel their own jobs; manage their own tokens and password |
  | writer | also change the catalog: ingest, replace, delete, reindex (a catalog-wide one included); run exhaustive catalog matches; `verify`, plain or `deep` (review F3, M6-60) |
  | admin | also `prune` and `gc` (with `versions` and `vacuum`); users; reading others' tokens, jobs, uploads, assemblies and sockets (`?owner=` on `GET` and `HEAD`, AU8), revoking their tokens and cancelling their jobs; pinning and deleting job records |

  - **A request's effective role** is `min(user.role, token.role)`, read at each request, so demoting a user demotes all their tokens at once. A token's role is chosen at creation, at most the presenting token's effective role (AU10).
  - **A field can raise the role:** a match whose `candidates.catalog.selection` is `exhaustive` needs writer, and below it is `403 forbidden` before any job exists. `OPERATIONS` records such a field beside the operation's role (`x-fitment-role-fields` in the OpenAPI document), and VT23 tests both.
  - **The extractors** `Reader`, `Writer` and `Admin` run before any handler body, and the router helpers (`routes::reader(get(..))`) do not compile without one. Each operation's role is in `OPERATIONS` and the OpenAPI document (`x-fitment-role`); a test enumerates the router and requires every route but health, readiness, OpenAPI and login to carry one (XH14, VT23).
- **AU6. Authentication never writes** (B AU6). `last_used_ms` is kept per token in a `Mutex<BTreeMap>` and flushed by the sweep (JS9); a request's authentication is one indexed read on a reader connection.
- **AU7. Rate limits** (B AU7; M6-30; review F5, M6-61). In-memory token buckets, reset by a restart. Past one: `429 rate_limited` with `Retry-After`.
  - **Requests:** 50 a second per user, over all their tokens, with a burst of 100, so minting tokens multiplies nothing.
  - **Login:** 30 attempts per client address per minute, checked first; then 10 failures per (folded username, client address) per 15 minutes. A bucket is made for any username string, existing or not, so the limits show nothing about which usernames exist (AU2); and failures from one address never limit the same username from another, so no one can lock an admin out entirely (AU9 recovers offline in any case). The client address is the TCP peer's, unless the peer is in `server.trusted_proxies`; then it is the right-most address of `X-Forwarded-For` not in that list.
  - **Bounded:** at most `auth.login_buckets` (10,000) login buckets are kept, the least recently used evicted. One address makes at most 30 a minute, so it cannot evict its way to fresh allowances faster than its own address bucket allows.
  - **Charged up front** (PR 3's review): an attempt takes the address's token and the pair's failure under the buckets' lock before its hash, so concurrent wrong guesses cannot all pass one check; a success, or an exit that never judged the credentials (a cap, no argon2 slot, a storage error), gives the pair's failure back. A password change counts in its user's buckets the same way.
  - **Addresses:** an IPv6 client is keyed by its /64, which one host usually holds whole; an `X-Forwarded-For` entry may carry a port (`1.2.3.4:5678`, `[::1]:5678`); the header from a peer outside `server.trusted_proxies` is ignored with one warning a minute, naming no value.
- **AU8. Resources are owned** (B AU8, merge). A job, a match, an upload, an assembly or a socket line belongs to the user who created it. Another user's is `404 not_found` to anyone but an admin, the same answer as for an id that does not exist, so ids cannot be probed. Namespaced resources (uploads and assemblies by SHA-256, socket lines by derived id) resolve among the caller's own; an admin reads another owner's with `?owner=USER_ID`, on `GET` and `HEAD` only. A job-creating route, or any other write, never takes `?owner=`, so no job runs on another user's inputs (review question 8). Catalog entries are shared by every user (RA1).
- **AU9. The first admin and recovery are offline** (B AU9; M6-19). `fitment-server admin bootstrap|user|token … --state DIR` opens `server.db` directly, which WAL allows while the server runs, and never opens the catalog or takes the lease (SD6). It never migrates `server.db` while a server holds the lease (renewed within 30 s): an older schema is then refused with exit 2, and an upgrade migrates at the server's start (SD3; review question 2).
  - **`bootstrap`** refuses if an admin exists; it creates the user, mints a 90-day admin token and prints it to stdout once.
  - **`user reset-password --password-stdin`** and **`token revoke`** work when every admin token is lost; a reset revokes every token of the user (AU4).
- **AU10. A token mints only within itself** (review F1, F5; M6-58). `POST /v1/auth/tokens` makes a token whose role is at most the presenting token's effective role (AU5) and whose expiry is at most the presenting token's, and records the presenting token as its `parent`; asking for more is `400 invalid_parameter`, never a silent cap. A login's token has no parent and is capped by the user's role and `auth.token_max_days`. A user holds at most `auth.max_live_tokens` (20) live tokens: one more by minting is `409 too_many_tokens`, and a login at the cap evicts the user's oldest live root token (no parent: a login's or an offline command's) with what was minted from it, so a full cap never locks its user out (PR 3's review). A user mints at most `auth.mints_per_minute` (10) a minute, so dead tokens never outrun the sweep. A root token lists and revokes every token of its user; a minted token itself and what was minted from it only; an admin anyone's (PR 3's review). Revoking a token revokes its descendants (AU4). So a leaked token can neither outlive nor outrank itself, nor multiply its user's caps (AU7, UP4, JS10).

### 4.12 HTTP hardening (HX)

- **HX1. The accept loop** (B HX1; prototype `net.rs`). A `tokio::net::TcpListener` served by `hyper::server::conn::http1::Builder`; `axum::serve` is not used, since it exposes neither limit below.
  - **Limits:** `header_read_timeout` 10 s with `TokioTimer`; `max_buf_size` 64 KiB, which bounds the request line and headers; at most `server.max_connections` (1,024) open connections, a further one closed at accept.
  - **Protocols:** HTTP/1.1 only; no HTTP/2 (no `h2` in the graph) and no upgrades.
  - **Shutdown:** every connection is watched by `GracefulShutdown` (SD1).
  - **Smoke-tested:** a client that sent a request line and one header, then stalled, was disconnected at 10 s with nothing served.
  - **Review (PR 2):** a write that makes no progress for `server.write_timeout_seconds` (30) closes the connection, and no connection lives past `server.max_connection_seconds` (3,600), so a client that never reads its response cannot hold a slot; `max_header_size` bounds the head exactly at 64 KiB. hyper answers a request it cannot parse itself (`400`, or `431` for a head over the bound) with an empty body, outside the error contract: no route has seen it, so there is no `request_id` to give.
- **HX2. Body limits per route** (B HX2, A UP4). `DefaultBodyLimit` is disabled globally and set on each route:

  | Route | Limit |
  | --- | --- |
  | uploads | UP2's cap, streamed, never buffered |
  | JSON requests | `limits.max_json_kib` (1,024) |
  | everything else | 0: a body is `413 body_too_large` |

  A file match names at most `limits.max_candidates` (10,000) candidates, and a named catalog run at most as many ids (`400 invalid_parameter`).
- **HX3. Timeouts** (B HX3; M6-53).

  | What | Timeout |
  | --- | --- |
  | A non-streaming handler, before any `Prefer: wait` | 30 s (`TimeoutLayer`): `503 request_timeout`; a blocking task already started finishes and its result is dropped |
  | `Prefer: wait` and `?wait` | N seconds, at most 60 (JB11) |
  | The next chunk of an upload body | 30 s |
  | A whole upload | `limits.upload_minutes` (30) |
  | An upload's throughput, after its first 30 s | at least `limits.upload_min_kib_s` (64 KiB/s), averaged over any 30 s, else cut: `408 upload_too_slow`, the temporary file removed (review F5) |
  | Keep-alive idle | 60 s |

  NDJSON streams have no total timeout; they have heartbeats and stream caps (JS10).
- **HX4. No CORS by default** (B HX4). Without `server.cors_origins`, no CORS layer is installed and browsers keep cross-origin reads blocked. With an allowlist: exact origins only, never a wildcard with credentials; methods `GET`, `POST`, `PUT` and `DELETE`; headers `Authorization`, `Content-Type`, `Idempotency-Key`, `Prefer`. `PATCH` (`/v1/auth/users/{id}`) stays out in M6, which has no browser client; M8's viewer decides (PR 3's review).
- **HX5. Typed path parameters, fixed routes** (B HX5). Every path parameter is parsed into a type before any handler logic: a catalog `Reference` (`Reference::parse`), a socket reference (`sk_` and 20 hex, an optional `@rN`), `Sha256Hex`, a job, token or user id, or a closed enum. A malformed reference is `400 invalid_reference`; a malformed server id is `404 not_found`, the answer for an id that does not exist. A parameter's value is never echoed unescaped. There are no wildcard routes, no static file serving and no route that turns a parameter into a file-system path (PH1); axum percent-decodes before matching, and NUL, `/` and `..` after decoding fail the types.
- **HX6. Response headers** (B HX6). Every response carries `X-Content-Type-Options: nosniff`, `Content-Security-Policy: default-src 'none'; frame-ancestors 'none'`, `Referrer-Policy: no-referrer`, and `Cache-Control: no-store` except where VN5 says otherwise; no `Server` header. JSON is `application/json`, errors `application/problem+json`, NDJSON `application/x-ndjson`, STEP `application/octet-stream` with `Content-Disposition: attachment` and a filename built only from the catalog id and revision (`BRKT-0042_r3.stp`, ID1's alphabet) or the first 12 hex of an assembly's SHA-256.
- **HX7. Request ids** (B HX8; review question 7). `X-Request-Id` is generated (128 random bits) unless the peer is a trusted proxy that supplied one of at most 64 characters of `[A-Za-z0-9._-]`; any other value is ignored and one is generated; it is set on the response, recorded in the trace span, stored on any job created, and carried by every error (`request_id`).
- **HX8. Expensive synchronous work is capped** (review F3, F22; M6-60). Two kinds of synchronous work grow with their input; each runs on the blocking pool behind its own fair semaphore:
  - **Parsing a socket document:** a match's socket gate (JB2), an import's check (UP6), a review (OB5) and a `carry_from` read at the request, at most `limits.parse_concurrency` (4) at once, each of at most `limits.max_socket_mib` (16) bytes; a request waits for a permit within the handler's 30 s (HX3), then `503 request_timeout`.
  - **A `limit=all` catalog listing:** built whole in one snapshot and sent from memory (VN3), at most `limits.list_all_concurrency` (2) at once; past it, `429 too_many_streams` without waiting.

### 4.13 The crate and its dependencies (SV)

- **SV1. The only async crate** (B SV1; C VT35). tokio, mio, hyper, axum and tower never enter the graph of `fitment-core`, `fitment-catalog` or `fitment`. A new `deny/sync.toml` denies `tokio`, `mio`, `hyper`, `axum`, `tower`, `async-std`, `smol` and `futures-executor` and is run with each of `crates/fitment-catalog/Cargo.toml` and `crates/fitment/Cargo.toml` as the root, as `deny/core.toml` is for core; CI's async grep extends from `fitment-core/src` to `fitment-catalog/src` and `fitment/src`.
- **SV2. Library and binary** (B SV2; C's assumption 9). `lib.rs` exposes `Server::build(Config)`, `Server::router()` (for `oneshot` tests) and `Server::serve(listener, shutdown)`. `main.rs` is the only place that reads the process's arguments, installs the tracing subscriber, or exits.
- **SV3. Blocking work never runs on a runtime worker** (B SV3). Every call into `fitment_core` or `fitment_catalog`, every rusqlite call and every argon2 call runs inside `tokio::task::spawn_blocking`. The runtime has `worker_threads = min(4, cpu)` for I/O only and `max_blocking_threads = 64 + query_slots + 2`, so lanes, short reads and `tokio::fs` never starve one another; compute is bounded by JS2's permits, not by the pool.
- **SV4. Catalog and core calls only through two modules** (B SV4). `catalog.rs` is the only module naming `fitment_catalog`, and `matching.rs` the only one building `Matcher` and `MatchDocument`; a grep in CI keeps it so, which keeps the determinism argument (DC1, DC3, SN1) local.
- **SV5. Versions in lockstep** (B SV5). `fitment-server` takes the workspace version and depends on `fitment-core` and `fitment-catalog` at `=` that version. It refuses to start against a catalog whose schema is newer (DB5's `Error::Newer`); `fitment.info` and every job's `fitment.run` report its version and the core's version constants.
- **SV6. The two databases stay apart** (B SV6; M6-32). `catalog.db` is opened only by `fitment-catalog`; `server.db` only by `state/db.rs`, and no SQL in the server names a catalog table. `deny.toml`'s rusqlite wrappers become `["fitment-catalog", "fitment-server"]`. The state directory and the catalog directory must not contain each other: startup refuses either nesting after canonicalizing both, which also keeps uploaded objects outside the catalog (ST10).
- **SV7. The server's clippy bans** (B SV7). `std::env::var` and `var_os` outside `config.rs`; `std::process::exit` outside `main.rs`; `std::thread::spawn` (only the catalog's own scoped threads exist); `tokio::task::block_in_place`; `serde_json::Value`, `from_str` and `to_value` inside `matching.rs` and `objects.rs` (DC1). `HashMap` is allowed, since no document comes from the server's own maps (DC2).
- **SV8. `FitVerifier` stays `Sync` only** (B SV8; M6-48). The core trait is `FitVerifier: Sync`, unchanged. The server holds `Arc<dyn FitVerifier + Send + Sync>`, built once at start from `fit.verifier` (OP9), so it can be moved into blocking tasks.
- **SV9. The runtime graph** (B M6B-13, 4.2, A A5, merge; M6-6, M6-34). The server's normal dependency graph with its default features holds no schemars, no `h2`, no TLS stack, no reqwest, and serde_json with exactly `default`, `std` and `float_roundtrip`: axum without `json`, `multipart`, `http2` and `ws`; hyper-util without `server-auto`. `tools/check-core.sh` checks it with `cargo tree -p fitment-server -e normal --edges features`. schemars enters only through the `schema` features (CO4, CA6, and the server's own), which only the OpenAPI test enables. Because `deny.toml`'s `[graph] all-features = true` turns those features on, cargo-deny's exact feature list for serde_json gains `alloc`, with the reason "schemars, behind the `schema` features no runtime build enables; with `std` on, `alloc` changes no parsing, printing or key order"; `raw_value`, `preserve_order` and `arbitrary_precision` stay refused.

The dependencies, each resolved and built with `cargo +1.85 build --release` in B's prototype, every licence passing `cargo deny check licenses` under the repository's `deny.toml` unchanged:

| Crate | Version | MSRV | Licence | Features | Why |
| --- | --- | --- | --- | --- | --- |
| tokio | 1.53 | 1.71 | MIT | `rt-multi-thread`, `net`, `signal`, `sync`, `time`, `macros`, `fs`, `io-util` | runtime, fair semaphore, `spawn_blocking` |
| axum | 0.8.9 | 1.80 | MIT | `default-features = false`; `http1`, `query`, `tokio`, `tracing`, `matched-path` | routing and extractors |
| hyper | 1.x | (built) | MIT | `http1`, `server` | the accept loop's `http1::Builder` |
| hyper-util | 0.1.21 | (built) | MIT | `server`, `server-graceful`, `tokio`, `http1`; tests also `client-legacy` (M6-40) | `TokioIo`, `TokioTimer`, `GracefulShutdown`; the tests' client |
| tower | 0.5.3 | 1.64 | MIT | `limit`, `timeout`, `util` | concurrency limit, timeouts |
| tower-http | 0.7.1 | 1.65 | MIT | `limit`, `timeout`, `trace`, `request-id`, `sensitive-headers`, `catch-panic`, `set-header`; `cors` only with an allowlist (HX4) | middleware; 0.6.11 also builds |
| http-body-util, futures-util, tokio-util | 0.1, 0.3, 0.7 | (built) | MIT | `futures-util` with `std` only; `tokio-util` `io` | body streaming |
| argon2 | 0.6.0 | 1.85 | MIT OR Apache-2.0 | default | password hashing; 0.5.3 (MSRV 1.65) is the fallback, with another API (no `SaltString` in 0.6) |
| getrandom | 0.4 | 1.85 | MIT OR Apache-2.0 | — | token, id and salt bytes; argon2 0.6 already brings it |
| tracing, tracing-subscriber | 0.1.44, 0.3.23 | 1.65 | MIT | subscriber: `default-features = false`, `fmt`, `env-filter`, `json`, `ansi`, `std` | logs |
| flate2 | 1.1 (PR 5) | 1.67 | MIT OR Apache-2.0 | `default-features = false`, `rust_backend` (`miniz_oxide`) | DC6's `Content-Encoding: gzip` |
| toml | 1.1 | (built) | MIT OR Apache-2.0 | `default-features = false`, `parse`, `serde`, `std` | the configuration file; 0.9.12 also builds |
| clap | workspace (4.6) | 1.85 | MIT OR Apache-2.0 | adds `env` at the member | flags |
| rusqlite | workspace (0.40, bundled) | — | MIT | — | `server.db` (M6-32) |
| schemars | 1.2.3 | (built) | MIT | optional, behind `schema` only (SV9) | the OpenAPI test (VN7) |
| serde, serde_json, thiserror | workspace | — | — | unchanged | — |
| anyhow | 1 | — | MIT OR Apache-2.0 | — | `main.rs` only (decision 11) |
| fitment-core, fitment-catalog | workspace, `=0.3.0` | — | MIT | — | the work |

Rejected: `sha2` 0.11 (MSRV 1.88); chrono, uuid and rand (times are `i64` Unix milliseconds from `SystemTime`, ids random bytes from `getrandom`); reqwest (webpki-roots is CDLA, workspace layout section 9); rustls and aws-lc-rs (M6-20); dashmap, governor, metrics, sysinfo and r2d2 (M6-29, M6-30, OB1); aide (M6-6). The prototype's graph, with schemars, clap and toml, had 140 crates.

### 4.14 Logging, configuration, operations, deployment (OP)

- **OP1. tracing, JSON, on stderr** (B OP1, merge; M6-54). `tracing-subscriber` writes JSON to stderr (`log.format = "json" | "text"`), filtered by `RUST_LOG`, then `log.filter`, then `info`. Stdout carries one line, `fitment-server: listening on <address>`, printed once the listener is bound (SD3).
  - **The request span:** the request id, the method, the matched route template (`MatchedPath`, never the raw URI or query), the status, the latency, the user id (never the username), and the job id when one is made.
  - **Sensitive headers:** `SetSensitiveRequestHeadersLayer` marks `Authorization`, `Cookie` and `Proxy-Authorization`.
  - **Never logged:** bodies, tokens, passwords, password hashes, the pepper, STEP content, socket or match documents. Client strings only as `?Debug`, which escapes control characters (PH6's intent).
  - **Paths in logs** are written relative to the configured roots (`catalog:blobs/sha256/ab/…`, `state:objects/sha256/…`, `catalog:` for the root itself), never absolute (review F8). Logs may name paths, which an operator needs; responses never do (ER3, XH13).
  - **Startup:** the effective configuration is logged once, with the pepper's path but not its content.
- **OP2. Configuration sources** (B OP2). Flags, then environment, then the TOML file, then defaults. The file is `--config PATH` or `FITMENT_SERVER_CONFIG`; each key also reads `FITMENT_SERVER__<SECTION>__<KEY>`, its separators doubled (**Review (PR 2):** an orchestrator injects `FITMENT_SERVER_PORT` and the like for a service of that name, so any other `FITMENT_SERVER_*` but `FITMENT_SERVER_CONFIG` is ignored with one warning, while a doubled name that names no key refuses the start); the only flags are `--config`, `--listen`, `--catalog`, `--state` and `--log-format`. Unknown keys are refused (`deny_unknown_fields`), so a typo fails the start. Secrets are read only from files the configuration names (`auth.pepper_file`), never from a flag, which `ps` would show.
- **OP3. Keys and defaults** (B OP3; C's assumption 10). `paths.catalog` and `paths.state` are required, since a catalog has no default (M5-9); their nesting is checked (SV6), and neither may hold a `..` component (**Review (PR 2):** through a directory not yet made, `..` defeats the comparison). Every limit is a key, so a test sets a small one.

  ```toml
  [server]   listen = "127.0.0.1:8080"; max_connections = 1024; trusted_proxies = []; cors_origins = []
             header_timeout_seconds = 10; handler_timeout_seconds = 30; write_timeout_seconds = 30
             max_connection_seconds = 3600
  [paths]    catalog = "<required>"; state = "<required>"
  [limits]   max_upload_mib = 128; max_socket_mib = 16; max_json_kib = 1024; max_candidates = 10000
             max_catalog_candidates = 100000; upload_concurrency = 8; read_concurrency = 16
             parse_concurrency = 4; list_all_concurrency = 2; staging_gib = 8; user_quota_gib = 10
             upload_ttl_hours = 24; upload_minutes = 30; upload_min_kib_s = 64; page_max = 1000
             wait_max_seconds = 60; upload_concurrency_per_user = 2; upload_chunk_seconds = 30
             upload_window_seconds = 30; upload_first_byte_seconds = 5; user_max_uploads = 10000
  [jobs]     cpu = 0 (= available_parallelism); match_threads = 4; memory_mib = 4096; match_entry_kib = 16
             queue_query = 64; queue_write = 16; queue_maintenance = 4
             per_owner_running = 2; per_owner_queued = 16
             deadline_extract_minutes = 10; deadline_match_minutes = 60; deadline_verify_minutes = 60
             retention_days = 30; sweep_minutes = 60; reindex_on_start = true; catalog_gc_at = ""
             drain_seconds = 25; streams_per_user = 8; streams = 256
  [catalog]  write_busy_ms = 60000; delete_busy_ms = 10000; read_busy_ms = 5000; wal_barrier_mib = 256
             barrier_max_seconds = 60; max_db_pages = 0
  [match]    require_reviewed_floor = false
  [auth]     pepper_file = ""; token_default_days = 90; token_max_days = 365; hash_concurrency = 2
             hash_wait_seconds = 10; max_live_tokens = 20; login_buckets = 10000
             rate_per_second = 50; rate_burst = 100; login_per_minute = 30; login_failures = 10
             login_failure_minutes = 15; mints_per_minute = 10
  [fit]      verifier = "none"
  [log]      format = "json"; filter = "info"
  [metrics]  listen = ""            # off; for example "127.0.0.1:9464"
  ```

- **OP4. `fitment-server check-config`** (B OP4) loads and validates the configuration, opens the catalog and `server.db` read-only, prints the effective configuration, and exits 0 or 2. The container's pre-start and the tests use it.
- **OP5. Liveness** (A 5.1, B OP5, merge). `GET /v1/health` answers `204` while the accept loop runs; it needs no authentication and touches no database. `fitment-server healthcheck` makes that request with a std `TcpStream`, for the image's `HEALTHCHECK` (distroless has no curl).
- **OP6. Metrics** (B OP6; M6-29). Prometheus text on `metrics.listen` only, without authentication, meant for loopback or a scrape network: `fitment_http_requests_total{route,code_class}`, `fitment_http_request_seconds{route}` (fixed buckets), `fitment_jobs_total{kind,state}`, `fitment_jobs_queued{lane}`, `fitment_jobs_running{lane}`, `fitment_job_seconds{kind}`, `fitment_cpu_permits_free`, `fitment_memory_mib_free`, `fitment_upload_bytes_total`, `fitment_auth_failures_total{reason}`, `fitment_rate_limited_total{what}`, `fitment_catalog_generation`, `fitment_catalog_entries{status}`, `fitment_catalog_stale`, `fitment_catalog_busy_total`, `fitment_state_object_bytes`. Labels come from closed sets only: a route template, never a raw path.
- **OP7. Readiness** (A 5.1, B OP7). `GET /v1/ready` needs no authentication and answers `204`, or `503` with a `fitment.error` whose code is `catalog_unavailable` or `shutting_down` and no path, when the catalog cannot be opened (marker, schema, WAL), `server.db` is not writable, the lease is not held (SD6) or the server is draining. A stale catalog is still ready, since reads are served; `fitment.info` carries the flags (`stale`, a reindex queued or running, `wal_barrier`, `queues_full`).
- **OP8. The container image** (B OP8; M6-31, M6-35). A multi-stage `deploy/Containerfile`: the builder `rust:1.85-bookworm`, pinned by digest, runs `cargo build --release --locked -p fitment-server -p fitment`; the runtime is `gcr.io/distroless/cc-debian12:nonroot`, pinned by digest (glibc and libgcc only: SQLite is bundled and linked statically).

  | Item | In the image |
  | --- | --- |
  | `/usr/local/bin/fitment-server` and `/usr/local/bin/fitment`, one version | yes |
  | `LICENSE` and `THIRD-PARTY.txt`, generated by `cargo deny list` from the locked graph | yes |
  | `ENTRYPOINT ["fitment-server","serve"]`, `USER 65532`, `EXPOSE 8080`, a default configuration with `listen = "0.0.0.0:8080"` | yes |
  | `VOLUME ["/var/lib/fitment/catalog","/var/lib/fitment/state"]` | yes |
  | `HEALTHCHECK` through `fitment-server healthcheck` | yes |
  | a shell, a package manager, compilers, CA certificates (the server calls out to nothing) | no |
  | Python, uv, Open CASCADE or any of their libraries (D1, decision 11) | no |

  Built for linux/amd64 and linux/arm64, run with a read-only root file system (writable only on the two volumes). It is not a dist artifact (M6-35): CI builds it on every PR (VT29), and `image.yml`, run by the user, pushes `ghcr.io/jchultarsky/fitment-server:<version>` from a tag.
- **OP9. The `FitVerifier`, now and in M7** (B OP9; SN11). At start, `fit.verifier` builds an `Arc<dyn FitVerifier + Send + Sync>` (SV8); M6 knows `"none"` (`NoFitVerifier`). In M7 `fitment-server` gains an optional `fit` feature over `fitment-fit` and the value `"occt-helper"` with the helper's paths; at start it asks the helper for its kernel and version and refuses to start if the helper does not answer, never falling back to `"none"` silently. The official image has no uv or Open CASCADE, so it runs only `"none"`; a user-built image `FROM` it can add them. Fit checks run inside the job's blocking thread through core's `FitRequest`; for catalog runs the catalog reads the checked blob itself (QR13), and dedup asks the fit per member once a kernel is named (QR6). A document records the kernel (MF6), so server and CLI documents are byte-equal only under the same verifier; M6's exit runs both with `"none"`.
- **OP10. Topology** (B OP10). One replica per catalog: SQLite WAL needs one host's shared memory, and a catalog on a network file system or a sync folder is unsupported (CC5), so both volumes are local disks, as the README and the image's notes say. The CLI may run against the same catalog under the catalog's own concurrency rules (CC1 to CC6); the server sees its writes at its next snapshot.
- **OP11. Test seams only under `test-support`** (B 15, C's assumption 9). The `test-support` feature, never enabled in a release build or the image (VT29 checks), adds: an injectable `Clock` (retention, expiry, rate limits) and `IdGen` (fixed ids for goldens); named hook points in each job kind and in the catalog batch (pause, panic, abort) for cancellation, drain and restart tests; the catalog's `CountingFs` (`failing_at`, over CA7's streaming calls too) for the object store; a fake `FitVerifier` that names a kernel; and shutdown on stdin's end of file, the portable trigger for the Windows tests (SD1). Every limit is configuration (OP3), not a seam.

### 4.15 Shutdown and recovery (SD)

- **SD1. Graceful shutdown** (B SD1). On SIGTERM or SIGINT (Unix), on Ctrl-C, Ctrl-Break or a close event (Windows, `tokio::signal::windows`), and under `test-support` on stdin's end of file:
  1. **Drain:** readiness turns `503 shutting_down`, new submissions and uploads get `503 shutting_down`, the listener closes; in-flight requests continue, and `GracefulShutdown` waits for them up to 10 s.
  2. **The dispatcher stops:** it starts no queued job; queued rows stay `queued`.
  3. **Running jobs** get `jobs.drain_seconds` (25) to finish; then their cancel flags are set, and each is recorded per JB6 as its call returns: a pure job back to `queued` with `attempt` unchanged, since a drain is not the job's doing (no `job_event` for the stop; review question 5), a write job `interrupted` with its partial changes document.
  4. **NDJSON streams** each get a `shutdown` event and close.
  5. **Last writes:** `last_used_ms` flushed; `server.db` checkpointed (TRUNCATE); the catalog writer and readers checkpointed (PASSIVE) and closed; the process exits 0.

  The container's grace period must exceed `drain_seconds` + 10 s; the image's notes say 45 s. After a SIGKILL the next start recovers (SD4).
- **SD2. A hard kill never damages a store** (B SD2). Both stores rest on SQLite transactions and bytes before rows (ST3, ST6, OB2); M5 killed writers 300 times without breaking an invariant. A killed upload leaves only a `.part` file; a killed ingest keeps its committed batches (RX7).
- **SD3. Startup order** (B SD3; review question 2). (1) Configuration (OP2). (2) The nesting check (SV6). (3) Open or create `server.db` (OB1). (4) Take the lease (SD6), then migrate `server.db` if it is older, holding the lease, so neither a second server nor an admin command (AU9) migrates beside it. (5) Open the catalog's job writer first, so any DB5 migration happens once before readers open; a newer schema refuses the start (SV5); read or set the catalog's instance id (CA8). (6) Recovery (SD4). (7) Fill the reader pool. (8) Queue a reindex if stale and configured (M6-28). (9) Start the dispatcher and the sweep. (10) Bind the listener and print its address (OP1), so no request is answered before recovery is done.
- **SD4. Recovery** (B SD4). Every file in the state's `tmp/` is removed, since no upload survives its connection; the catalog's own `tmp/` is left to its gc's one-hour rule (ST7). Jobs `running` or `cancelling` are settled by JB6, after JS13's check for a result already committed. Queued jobs are re-enqueued in their submission order, their inputs still held (OB4). An object without a row is left to the next sweep; a row whose file is missing or damaged is found when read (OB3), and the job fails `blob_damaged`.
- **SD5. A crash loop cannot loop on one input** (B SD5). `attempt` is persisted before dispatch (JS11), and a pure job runs at most twice across crashes (a graceful drain consumes no attempt, SD1): a hostile file that kills the process (an OOM kill) ends `interrupted` after its second try.
- **SD6. One server per state directory** (B SD6). The `lease` row is taken in a `BEGIN IMMEDIATE` at start and renewed every 10 s; a start that finds a lease renewed within 30 s by another holder refuses with exit 2, naming neither host nor path. The holder is a random id made at start. The admin commands (AU9) do not take the lease. **Review (PR 2):** renewals run in a `BEGIN IMMEDIATE`; a transient failure (a busy database) is retried at the next tick, and the lease is lost only to another holder or after 30 s without a renewal; losing it stops the server, which exits non-zero, since another may then own the directory. A server killed without a graceful stop leaves its lease to go stale: the next start on that directory waits the 30 s, since in M6 no start reclaims a lease as its own. A lease rather than a file lock, since `File::lock` needs Rust 1.89 (ST8).

## 5. Changes to the existing crates

Each change is small, synchronous and std-only, keeps every existing document byte-identical, and has its own test (VT37 to VT39). They are what the server needs so that it never writes a temporary path into a document, parses a library's text or carries a second implementation of something the CLI does (M6-48).

### 5.1 fitment-core (CO)

- **CO1. An incremental SHA-256** (B CO1; M6-16). `Sha256::new()`, `update(&mut self, &[u8])` and `finish(self) -> [u8; 32]` over the existing compression function in `sha256.rs`; `sha256` and `sha256_hex` become wrappers. Every content name in fitment (blobs, objects, uploads, documents, tokens' stored hashes) then comes from one implementation. Tested equal to the one-shot function on the unit set and the standard vectors, at every split point of messages of 0 to 200 bytes (VT37).
- **CO2. The run's outcome helpers live in core** (B CO2; M6-33). Moved from `crates/fitment/src/cmd/match_.rs`, where they are private functions today:
  - `CandidateResult::is_fitment_bug()`: an internal refusal, or a verification pass after a level-2 drop (M4-17; the CLI's `reported`);
  - `CandidateResult::original_fails()`: the original failing beyond VD5 and VD6 (VD15; the CLI's `original_bug`);
  - `Outcome::exit_code()`: 0, 4 or 5 (VD14), on core's existing run `Outcome`.

  The CLI and the server both call them, so a job's `exit_code` equals the CLI's by construction (JB4); the CLI's file-run windowing stays its own (about 60 lines).
- **CO3. Duplicate components are merged** (A 15; M6-48). `MatchDocument` merges candidates by identity as MF3 does for files: files by SHA-256, components by (assembly SHA-256, definition `#id`), every label kept in order of appearance. Today a component named twice gives two entries that tie on every MF5 key and keep request order, so the request's order would reach the document. Every document an earlier build wrote for inputs without a repeated component is unchanged (the goldens and the corpus show it), and no CLI command can name a component, so `MATCH_RULES_VERSION` does not rise (question 8).
- **CO4. Schemas behind the `schema` feature** (A A5, decision 11; M6-6). The types `features.v1`, `fitment.parts`, `fitment.socket` and `fitment.match` serialize derive `schemars::JsonSchema` under `#[cfg_attr(feature = "schema", …)]`; the feature is off by default. `deny/core.toml` and `tools/core-deps.py` allow schemars and its own dependencies only under that feature; core's other rules (no I/O, no stepq outside its walls) are unchanged.
- **CO5. `Matcher::judge_component`** (review F13; M6-5, M6-48). `pub fn judge_component(&self, component: &ComponentRead, source: CandidateSource) -> CandidateResult`, where `ComponentRead` is core's reading of one component of an assembly, `ComponentRead::read(bytes, part, occurrence) -> Result<ComponentRead, ComponentError>`: the selection made by the same code the socket extractor selects with (`SelectError`), the assembly gate's refusal kept (`ComponentError::Refused`, with its refusals), and the assembly's SHA-256 and the definition's `#id` taken from the reading. `judge_component` judges it as `judge_part` does, and gives an `internal` refusal (a fitment bug, M4-17) when `source` is not `CandidateSource::Component` of that same assembly and definition. The server builds no `Part` and selects no component itself (SN1):
  - **A selection in a refused assembly** ends the job `refused`, exit 3, `component_refused`, with the refusals; nothing is judged from a partial reading.
  - **An original** (SN9) is read the same way; one whose assembly cannot be read ends the job `failed`, `run_incomplete`, exit 1, with `details.original`.
  - **The library reference** for component parity (DC3, XH3.5) calls the same function, so the API's documents are compared with core's own reading and judging.

### 5.2 fitment-catalog (CA)

- **CA1. Ingest and replace from bytes, with per-file ids and display names** (B CA1, A 14 item 1; M6-48). Today `Catalog::ingest(&[PathBuf], Option<&Id>)` takes one id for one file and derives the default id (ID3), the stored `file_name` (PH3), the batch order (ID4), `changes[].input` and the text of error lines from the path; the server's paths would be object names (`objects/sha256/ab/ab12…`).

  ```rust
  pub trait InputBytes: Sync {
      /// Reads the file once, at most `cap + 1` bytes (SZ1), when its batch reaches it (ST2).
      fn read(&self, cap: u64) -> std::io::Result<Vec<u8>>;
  }
  pub struct Input<'a> {
      pub bytes: &'a dyn InputBytes,
      pub input: &'a str,       // changes[].input and error lines: the argument as given
      pub file_name: &'a str,   // ID3's default id, PH3's stored name, ID4's stem test
      pub id: Option<Id>,       // ID1; per file
  }
  impl Catalog {
      pub fn ingest_inputs(&mut self, inputs: &[Input<'_>], observer: &dyn Observer) -> Result<Ingest, Error>;
      pub fn replace_input(&mut self, reference: &Reference, input: &Input<'_>, observer: &dyn Observer)
          -> Result<Replace, Error>;
  }
  ```

  - **Order:** ID4 with a file that names its id placed among the exact stems; each group in byte order of `input`.
  - **Bytes:** each file's `read` is called once, when its batch reaches it, so memory holds one file at a time, as now. The CLI's implementation reads the path with PH4's regular-file check and ST10's inside-DIR check; the server's reads the verified object (UP7).
  - **Paths:** no path the catalog did not receive as `input` reaches a document or an error line (DC2).
  - **The old entry points** `ingest` and `replace` become wrappers over these, so the CLI's output is unchanged (CU3).
- **CA2. `Observer`** (B CA2; M6-27). `pub trait Observer: Sync { fn cancelled(&self) -> bool; fn progress(&self, event: Progress<'_>); }`, and `NoObserver`.
  - **Where it is threaded:** `ingest_inputs`, `replace_input`, `reindex`, `Snapshot::match_catalog`, `verify` and `gc`; the existing signatures pass `NoObserver`.
  - **When `cancelled` is asked:** before each file and each batch commit (ingest, reindex), before each group a judging thread takes (match), between phases and before each blob under `deep` (verify), between unlink batches (gc). A cancelled match returns `NotRun::Cancelled`, with no results; a cancelled ingest, reindex or gc returns what it committed or unlinked, with `stopped: Cancelled`.
  - **What `progress` carries:** counts, each committed file's `Ingested` line and each batch's generation; never anything a result depends on.
  - **The test:** every corpus document is identical under `NoObserver` and under a recording observer (VT38).
- **CA3. WAL pressure** (B CA3). `Catalog::checkpoint()` returns SQLite's `(busy, log_pages, checkpointed_pages)` from `wal_checkpoint(PASSIVE)`, and `checkpoint_truncate()` runs `TRUNCATE`, so the server sees readers pinning the WAL (CX6) without reading the catalog's files.
- **CA4. No catalog document names an absolute path or a library error's text** (A 14 item 7; review F7). `Problem.what` for a missing fan-out directory is `fan.display()` today, an absolute path (`crates/fitment-catalog/src/verify.rs`, around line 630). Every path in a `Problem` becomes relative to the catalog's root (`blobs/sha256/ab`). The text of a `fitment.catalog` line for a batch that could not commit, and of a reindex line's error, is built from the error's kind (`io: not found`, `io: permission denied`, `database: busy`, `database: corrupt`, `storage full`), never from its `Display`, which for an `io::Error` or a SQLite error can carry a path or a statement. The CLI prints the same text, now relative or built from the kind (CU3).
- **CA5. `fitment.verify` and `fitment.gc` v1** (A 8.2, 15; M6-48). Two result documents in `fitment_catalog::document`, frozen from 0.3.0 like `fitment.catalog`:
  - `fitment.verify`: `{format, format_version, generation, deep, full, consistent, problems: [{kind, id, what, detail}]}`, problems in `verify`'s order;
  - `fitment.gc`: `{format, format_version, generation, versions, vacuum, unlinked, temporaries, old_rows, failed}`.

  Each holds only the catalog's state and the build's versions, never a path (CA4) or a time.
- **CA6. Schemas behind the `schema` feature** (A A5; M6-6). `fitment.catalog`, `fitment.verify` and `fitment.gc`'s types derive `JsonSchema` under fitment-catalog's own `schema` feature, which turns on core's; off by default.
- **CA7. A streaming writer and reader in `Fs`** (review F14; M6-44). The public `fitment_catalog::fs::Fs` trait gains `create_new_stream(path) -> io::Result<Box<dyn FsWrite>>`, a file created new with mode 0600 on Unix, whose `write_chunk(&[u8])` and `sync()` are seam calls, and `open_stream(path, cap) -> io::Result<Box<dyn FsRead>>`, whose `read_chunk` is; `rename`, `remove_file` and `sync_dir` exist already. `StdFs` implements them over `File`, and `CountingFs` records each as a `Call`, so `CountingFs::failing_at(k, kind)` can fail any of them. The server's uploads (UP2), stored documents (OB5) and object reads (OB3) go through them, so VT11 fails every file call of the server's store in turn. The catalog's own blob writes keep `write_new`, unchanged. Adding methods to a public trait is a breaking change, which 0.3.0 may make.
- **CA8. A catalog instance id** (review F24; M6-63). `Catalog::instance(&mut self, mint: impl FnOnce() -> [u8; 16]) -> Result<String, Error>` reads the `meta` row `instance` (32 lower-case hex), or, when there is none, inserts one from `mint` in one `BEGIN IMMEDIATE` and returns it. The catalog crate stays without a random source (the server passes 16 bytes from `getrandom`). The row is not an `op` and raises no generation; schema 1 is unchanged, nothing in 0.2.0 reads `meta`, and `verify` ignores the row. The server calls it on its job writer at start (SD3) and keeps the value for VN5's ETags; the CLI neither writes nor reads it.
- **Review (PR 2): two reads the server needed early.** `Catalog::inspect(root)` checks that a directory is a catalog this build opens (the marker, PH2's directories, and the database's header read as bytes) without a connection, so `check-config` (OP4) makes no `-wal` or `-shm`, sets no pragma and migrates nothing; `Snapshot::status_counts()` gives the live entries by status in one query, what counting `list`'s statuses gives, for `fitment.info` (DC11). Neither is a rule of its own; `crates/fitment-catalog/tests/inspect.rs` shows both, and schema 1 is unchanged.

### 5.3 The CLI (CU)

- **CU1. `delete` and `prune` print `fitment.catalog`** (A 15). `fitment catalog delete` and `fitment catalog prune --revisions` gain `--format table|json`; JSON is the `fitment.catalog` document with `changes`, as M5's section 7.11 already specifies for them (`DeleteArgs` and `PruneArgs` have no `format` today). Exit codes unchanged.
- **CU2. `verify` and `gc` print their documents** (A 15). `fitment catalog verify` and `fitment catalog gc` gain `--format table|json`; JSON is `fitment.verify` or `fitment.gc` (CA5). Exit codes unchanged: `verify` exits 3 when it finds a problem (M5-25).
- **CU3. The CLI goes through the shared paths** (B CA1, CO2). `catalog ingest` and `replace` call CA1 with `input` the argument as given and `file_name` its final component; `match` takes its exit code and bug tests from CO2. Every existing golden, CLI test and corpus report is byte-identical (VT39), but for the text CA4 changes: a `verify` problem's path, now relative, and the error text of a batch-failure or reindex line, now built from the error's kind. Each changed line and its new text is recorded in VT39's goldens and in the CHANGELOG (review F7).

## 6. The state store: schema and layout

### 6.1 On disk

```text
<state>/
  FITMENT-SERVER-STATE      marker, "fitment.server 1\n", written last by the first start
  server.db                 SQLite, WAL (+ -wal, -shm)
  objects/sha256/00..ff/    uploads, assemblies and stored documents: exact bytes, mode 0444
  tmp/<random>.part         uploads and documents in flight; emptied at start (SD4)
<catalog>/                  an M5 catalog, schema 1, unchanged (FITMENT-CATALOG, catalog.db, blobs/, tmp/)
```

Neither directory may contain the other (SV6).

### 6.2 `server.db`, schema 1

```sql
PRAGMA application_id = 1718449235;   -- 0x666D7453, 'fmtS'
PRAGMA user_version = 1;
-- per connection: journal_mode=WAL (checked), synchronous=FULL, foreign_keys=ON,
-- trusted_schema=OFF, busy_timeout=5000; fullfsync on Apple (as DB1, ST9)

CREATE TABLE meta  (key TEXT PRIMARY KEY, value TEXT NOT NULL) STRICT;
CREATE TABLE lease (one INTEGER PRIMARY KEY CHECK (one = 1),                 -- SD6
                    holder TEXT NOT NULL, heartbeat_ms INTEGER NOT NULL) STRICT;

CREATE TABLE user (
  key INTEGER PRIMARY KEY AUTOINCREMENT, id TEXT NOT NULL UNIQUE,            -- 'u_' + 26 base32
  kind TEXT NOT NULL DEFAULT 'person' CHECK (kind IN ('person','system')),  -- M6-56
  username_fold TEXT NOT NULL UNIQUE CHECK (length(username_fold) BETWEEN 1 AND 64),
  username TEXT NOT NULL,
  password_phc TEXT,                                                         -- NULL: token-only user
  role TEXT NOT NULL CHECK (role IN ('reader','writer','admin')),
  disabled INTEGER NOT NULL DEFAULT 0 CHECK (disabled IN (0,1)),
  created_ms INTEGER NOT NULL, updated_ms INTEGER NOT NULL) STRICT;

CREATE TABLE token (
  key INTEGER PRIMARY KEY AUTOINCREMENT, id TEXT NOT NULL UNIQUE,            -- 't_' + 26 base32
  user INTEGER NOT NULL REFERENCES user(key),
  name TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 128),
  secret_sha256 TEXT NOT NULL UNIQUE CHECK (length(secret_sha256) = 64),    -- AU3
  hint TEXT NOT NULL,                                                        -- first 4 characters after the prefix
  role TEXT NOT NULL CHECK (role IN ('reader','writer','admin')),            -- at most the presenting token's (AU10)
  parent INTEGER REFERENCES token(key),                                      -- AU10: minted from; NULL for a login's
  created_ms INTEGER NOT NULL, expires_ms INTEGER NOT NULL,                  -- AU4: always expires; AU10: ≤ the parent's
  last_used_ms INTEGER, revoked_ms INTEGER) STRICT;
CREATE INDEX token_user ON token(user, revoked_ms, expires_ms);             -- AU10's live count
CREATE INDEX token_parent ON token(parent);                                -- AU4's cascade, the sweep
CREATE TRIGGER token_not_for_system BEFORE INSERT ON token
WHEN (SELECT kind FROM user WHERE key = NEW.user) = 'system'
BEGIN SELECT RAISE(ABORT, 'the system user holds no token'); END;
CREATE TRIGGER token_not_moved_to_system BEFORE UPDATE OF user ON token
WHEN (SELECT kind FROM user WHERE key = NEW.user) = 'system'
BEGIN SELECT RAISE(ABORT, 'the system user holds no token'); END;
CREATE TRIGGER user_kind_fixed BEFORE UPDATE OF kind ON user           -- a person never becomes the system user
WHEN NEW.kind IS NOT OLD.kind
BEGIN SELECT RAISE(ABORT, 'a user''s kind never changes'); END;

CREATE TABLE object (                       -- OB2: each row promises a file hashing to sha256
  sha256 TEXT PRIMARY KEY CHECK (length(sha256) = 64 AND NOT sha256 GLOB '*[^0-9a-f]*'),
  bytes INTEGER NOT NULL CHECK (bytes >= 0), added_ms INTEGER NOT NULL) STRICT;

CREATE TABLE upload (                       -- UP3: a user's access to an object
  key INTEGER PRIMARY KEY AUTOINCREMENT,
  owner INTEGER NOT NULL REFERENCES user(key),
  sha256 TEXT NOT NULL REFERENCES object(sha256),
  created_ms INTEGER NOT NULL, expires_ms INTEGER NOT NULL,
  UNIQUE (owner, sha256)) STRICT;

CREATE TABLE assembly (                     -- RA3: a user's registered assembly
  key INTEGER PRIMARY KEY AUTOINCREMENT,
  owner INTEGER NOT NULL REFERENCES user(key),
  sha256 TEXT NOT NULL REFERENCES object(sha256),
  created_ms INTEGER NOT NULL, deleted_ms INTEGER) STRICT;
CREATE UNIQUE INDEX assembly_live ON assembly(owner, sha256) WHERE deleted_ms IS NULL;
CREATE TABLE assembly_name (assembly INTEGER NOT NULL REFERENCES assembly(key),
  name TEXT NOT NULL CHECK (length(CAST(name AS BLOB)) BETWEEN 1 AND 255),      -- RA8's file_name
  PRIMARY KEY (assembly, name)) STRICT;
CREATE TABLE assembly_parts (               -- derived, insert-only, by version (RA3, DB7's pattern)
  assembly INTEGER NOT NULL REFERENCES assembly(key),
  extractor_version INTEGER NOT NULL, socket_rules_version INTEGER NOT NULL,
  status TEXT NOT NULL CHECK (status IN ('accepted','refused')),
  document_sha256 TEXT NOT NULL REFERENCES object(sha256),                 -- fitment.parts bytes
  root TEXT,                                                               -- fitment.assembly's root, as JSON (PR 5)
  job INTEGER,                                                             -- JS13; no FK: the job record expires first
  PRIMARY KEY (assembly, extractor_version, socket_rules_version)) STRICT;

CREATE TABLE socket_line (                  -- RA4
  key INTEGER PRIMARY KEY AUTOINCREMENT,
  owner INTEGER NOT NULL REFERENCES user(key),
  id TEXT NOT NULL CHECK (length(id) = 23 AND id GLOB 'sk_*'),
  assembly_sha256 TEXT NOT NULL CHECK (length(assembly_sha256) = 64),     -- no FK: an import may name
  definition TEXT NOT NULL, occurrence TEXT NOT NULL,                      -- an assembly not stored
  last_n INTEGER NOT NULL DEFAULT 0,                                       -- RA4: its highest revision, kept when
  created_ms INTEGER NOT NULL, deleted_ms INTEGER) STRICT;                 -- its revisions are swept (PR 5)
CREATE UNIQUE INDEX socket_line_live ON socket_line(owner, id) WHERE deleted_ms IS NULL;   -- RA4: one live row,
CREATE INDEX socket_line_rows ON socket_line(owner, id);                                    -- tombstones beside it
CREATE INDEX socket_line_assembly ON socket_line(owner, assembly_sha256);                   -- RA6, the lists (PR 5)
CREATE TABLE socket_revision (              -- RA5, OB5: insert-only
  key INTEGER PRIMARY KEY AUTOINCREMENT,
  line INTEGER NOT NULL REFERENCES socket_line(key),
  owner INTEGER NOT NULL, line_id TEXT NOT NULL,                            -- copied from the line row: RA4's
  n INTEGER NOT NULL CHECK (n >= 1),                                        -- numbering spans a revived id's rows
  how TEXT NOT NULL CHECK (how IN ('extract','review','import')),
  verified INTEGER NOT NULL CHECK (verified IN (0,1)),                      -- RA10
  parent INTEGER REFERENCES socket_revision(key),
  document_sha256 TEXT NOT NULL REFERENCES object(sha256),                 -- exact fitment.socket bytes
  document_digest TEXT NOT NULL,                                           -- the document's own digest
  extractor_version INTEGER NOT NULL, socket_rules_version INTEGER NOT NULL,
  unreviewed INTEGER NOT NULL, carried INTEGER NOT NULL, stale INTEGER NOT NULL,
  job INTEGER, created_ms INTEGER NOT NULL,                               -- JS13; no FK: the job record expires first
  UNIQUE (owner, line_id, n)) STRICT;
CREATE TRIGGER socket_revision_insert_only BEFORE UPDATE ON socket_revision
BEGIN SELECT RAISE(ABORT, 'socket revisions are insert-only'); END;

CREATE TABLE job (
  key INTEGER PRIMARY KEY AUTOINCREMENT,                                   -- submission order
  id TEXT NOT NULL UNIQUE,                                                 -- 'j_' + 26 base32 (M6-55)
  owner INTEGER NOT NULL REFERENCES user(key),
  kind TEXT NOT NULL CHECK (kind IN ('features','assembly_read','socket_extract','socket_import','match',
         'catalog_ingest','catalog_replace','catalog_reindex','catalog_verify','catalog_gc','catalog_prune')),
  lane TEXT NOT NULL CHECK (lane IN ('query','write','maintenance')),
  state TEXT NOT NULL CHECK (state IN ('queued','running','cancelling','succeeded',
                                       'refused','failed','cancelled','interrupted')),
  request BLOB NOT NULL, request_sha256 TEXT NOT NULL,                     -- canonical (JB9)
  idempotency_key TEXT, idempotency_sha256 TEXT,                           -- JB9: the request as sent; a released
  released_key TEXT,                                                       -- key moves here, freeing the index
  attempt INTEGER NOT NULL DEFAULT 1, request_id TEXT NOT NULL,
  created_ms INTEGER NOT NULL, started_ms INTEGER, finished_ms INTEGER, deadline_ms INTEGER,
  cancel_requested INTEGER NOT NULL DEFAULT 0,
  exit INTEGER CHECK (exit IS NULL OR exit IN (0,1,2,3,4,5)),              -- the CLI's code (JB4)
  outcome TEXT, error_code TEXT, error_details TEXT,                       -- ER3: built from codes and values
  result_sha256 TEXT REFERENCES object(sha256), result_format TEXT,        -- the document's bytes
  result TEXT,                                                             -- what it made: fitment.job's result.href
  index_sha256 TEXT REFERENCES object(sha256),                             -- DC12: a match's candidates index
  catalog_generation INTEGER, catalog_counts TEXT,                         -- M6-25; a write job's at dispatch (JB6)
  notes TEXT NOT NULL DEFAULT '[]',                                        -- JB10
  pinned INTEGER NOT NULL DEFAULT 0, expires_ms INTEGER,
  UNIQUE (owner, idempotency_key)) STRICT;
CREATE INDEX job_queue ON job(lane, state, key) WHERE state = 'queued';
CREATE INDEX job_list ON job(owner, created_ms, id);
CREATE INDEX job_expiry ON job(expires_ms) WHERE expires_ms IS NOT NULL;      -- JS9's retention (PR 5)
CREATE INDEX job_live ON job(state) WHERE state IN ('running', 'cancelling'); -- SD4's recovery (PR 5)

CREATE TABLE job_input (job INTEGER NOT NULL REFERENCES job(key) ON DELETE CASCADE,
  n INTEGER NOT NULL, sha256 TEXT NOT NULL REFERENCES object(sha256),
  PRIMARY KEY (job, n)) STRICT;                                            -- OB4: written at submission; held until
                                                                           -- terminal, a match's for its retention
CREATE TABLE job_cites (job INTEGER NOT NULL REFERENCES job(key) ON DELETE CASCADE,
  revision INTEGER NOT NULL REFERENCES socket_revision(key),
  PRIMARY KEY (job, revision)) STRICT;                                     -- RA6, OB4: a match's socket revision,
                                                                           -- written at submission
CREATE TABLE job_event (job INTEGER NOT NULL REFERENCES job(key) ON DELETE CASCADE,
  seq INTEGER NOT NULL, at_ms INTEGER NOT NULL, type TEXT NOT NULL,
  data TEXT NOT NULL, PRIMARY KEY (job, seq)) STRICT;                      -- persisted events only (JB8)
-- OB2, OB4 (PR 4's review): every column naming an object is indexed, so
-- a lookup of what holds an object is one index probe per table; and an
-- object whose naming row goes becomes a candidate, so the sweep looks
-- only at candidates, never at every object.
CREATE INDEX upload_object ON upload(sha256);
CREATE INDEX upload_expiry ON upload(expires_ms);
CREATE INDEX assembly_object ON assembly(sha256);
CREATE INDEX assembly_parts_object ON assembly_parts(document_sha256);
CREATE INDEX socket_revision_object ON socket_revision(document_sha256);
CREATE INDEX job_input_object ON job_input(sha256);
CREATE INDEX job_result ON job(result_sha256);
CREATE INDEX job_index ON job(index_sha256);
-- JS13 (PR 5): a re-run after a crash looks for a row naming its job.
CREATE INDEX assembly_parts_job ON assembly_parts(job);
CREATE INDEX socket_revision_job ON socket_revision(job);
CREATE INDEX socket_revision_parent ON socket_revision(parent);
CREATE TABLE object_candidate (sha256 TEXT PRIMARY KEY) STRICT;            -- OB4: the sweep's to look at
CREATE TRIGGER upload_gone AFTER DELETE ON upload
BEGIN INSERT OR IGNORE INTO object_candidate VALUES (OLD.sha256); END;
CREATE TRIGGER assembly_gone AFTER DELETE ON assembly
BEGIN INSERT OR IGNORE INTO object_candidate VALUES (OLD.sha256); END;
CREATE TRIGGER assembly_parts_gone AFTER DELETE ON assembly_parts
BEGIN INSERT OR IGNORE INTO object_candidate VALUES (OLD.document_sha256); END;
CREATE TRIGGER socket_revision_gone AFTER DELETE ON socket_revision
BEGIN INSERT OR IGNORE INTO object_candidate VALUES (OLD.document_sha256); END;
CREATE TRIGGER job_input_gone AFTER DELETE ON job_input
BEGIN INSERT OR IGNORE INTO object_candidate VALUES (OLD.sha256); END;
CREATE TRIGGER job_gone AFTER DELETE ON job
BEGIN
  INSERT OR IGNORE INTO object_candidate SELECT OLD.result_sha256 WHERE OLD.result_sha256 IS NOT NULL;
  INSERT OR IGNORE INTO object_candidate SELECT OLD.index_sha256 WHERE OLD.index_sha256 IS NOT NULL;
END;
```

A job's run record (`fitment.run`) is built from its row, its normalized request and its stored document; there is no separate report table. The catalog's generation and counts are recorded numbers (M6-3's cost), not foreign keys.

### 6.3 What happens to m5-catalog.md section 6.3

That section reserved the catalog's schema for M6: assemblies as blobs with a `kind` column, a `socket` table keyed by (assembly SHA-256, selection, `SOCKET_RULES_VERSION`), and a `report` table. M6-3 withdraws the reservation: none of it is built, the catalog stays at schema 1, and its insert-only, revision-carrying shape for sockets lives on in `socket_revision`. PR 9 marks the section withdrawn, with a pointer here, and amends M5-17 as built.

## 7. The API

### 7.1 Conventions

- **Modes** (M6-1). A job-creating request answers `202 Accepted` with the `fitment.job` and `Location: /v1/jobs/{id}`; with `Prefer: wait=N` and the job finished in time, `200 OK` with the finished job (JB11). A synchronous request answers with its document or `204`.
- **Roles** are the least role needed (AU5); the column says `none` for the four unauthenticated routes. A field that raises the role says so (an exhaustive match, AU5). Every `GET` or `HEAD` of a user's own resources takes `?owner=USER_ID` for an admin; no other method does (AU8).
- **CLI** names the command whose stdout the body equals (DC3), and `Fitment-Exit-Code` carries its exit code.
- **Headers:** every request may carry `X-Request-Id` (honoured from a trusted proxy only, and only if well formed: HX7) and, when it creates a job, `Idempotency-Key` (JB9) and `Prefer: wait=N`. Every document response carries DC4's headers.
- **Bodies** are JSON under VN6's conventions, at most 1 MiB, except uploads.

### 7.2 Server

| Method and path | Mode | Role | CLI | Returns |
| --- | --- | --- | --- | --- |
| `GET /v1/info` | sync | reader | `fitment --version`; `catalog list`'s stderr line | `fitment.info` (DC11) |
| `GET /v1/health` | sync | none | — | `204` while the process runs (OP5) |
| `GET /v1/ready` | sync | none | — | `204`, or `503` with `catalog_unavailable` or `shutting_down` (OP7) |
| `GET /v1/openapi.json` | sync | none | — | the committed OpenAPI 3.1 bytes (VN7) |

There is no `catalog init` endpoint: the server opens its catalog at start (SD3), refusing a catalog of a newer schema (SV5).

### 7.3 Uploads

| Method and path | Mode | Role | Body | Returns |
| --- | --- | --- | --- | --- |
| `PUT /v1/uploads/sha256:{hex}` | sync | reader | the raw bytes (`application/octet-stream`, `model/step` or `application/json`) | `201` `fitment.upload` when the caller held no upload of these bytes, `200` when it did (its lifetime extended); `422 upload_digest_mismatch`, nothing kept; `413 too_large`; `408 upload_too_slow` (HX3); `507 quota_exceeded` or `storage_full` (UP2 to UP4); either success costs the same observable work (UP8) |
| `POST /v1/uploads` | sync | reader | the raw bytes | as `PUT`, with `Location: /v1/uploads/sha256:{hex}`; for a client that cannot hash first |
| `HEAD /v1/uploads/sha256:{hex}` | sync | reader | — | `200` if the caller may name it (an upload, or an input of a retained match record), `404` otherwise (UP3) |
| `GET /v1/uploads/sha256:{hex}` | sync | reader | — | `fitment.upload` (metadata, never the bytes) |
| `GET /v1/uploads?after=&limit=` | sync | reader | — | `fitment.page` of the caller's `fitment.upload`, by SHA-256 |
| `DELETE /v1/uploads/sha256:{hex}` | sync | reader | — | `204`; `409 upload_in_use` while a job that is not terminal names it (OB4); an object a retained run record holds stays, without the caller's row |

### 7.4 Catalog

| Method and path | Mode | Role | CLI | Returns |
| --- | --- | --- | --- | --- |
| `GET /v1/catalog` | sync | reader | `catalog list`'s summary | the catalog's generation, counts and state, `fitment.info`'s `catalog` object alone |
| `GET /v1/catalog/entries?status=&after=&limit=` | sync | reader | `catalog list --status S --format json` | `fitment.catalog` v1: a page's `entries` is a slice and its `counts` the whole catalog's, as `CatalogDocument::new` defines; the next page in `Link` (VN3). With `limit=all`, exactly the CLI's bytes, built in one snapshot and sent from memory (VN3, HX8) |
| `POST /v1/catalog/entries` | job | writer | `catalog ingest [--id ID] [--max-mib N] FILE… --format json` | job `catalog_ingest`; document `fitment.catalog` with `changes` |
| `GET /v1/catalog/entries/{ref}` | sync | reader | `catalog get REF --format json` | `fitment.catalog` with one entry, its properties and history; a tombstone too (ID7) |
| `GET /v1/catalog/entries/{ref}/features` | sync | reader | `catalog get REF --format features` | `features.v1` bytes, `no-cache` (VN5); quarantined: the refused document with `Fitment-Exit-Code: 3`; a superseded revision: `410 derived_rows_gone`; stale: `409 entry_stale` |
| `GET /v1/catalog/entries/{ref}/step` | sync | reader | `catalog get REF --format step -o OUT` | the stored bytes, re-hashed first (ST11): a mismatch is `500 blob_damaged` with no body; pruned or deleted bytes: `410 bytes_not_kept` |
| `PUT /v1/catalog/entries/{ref}` | job | writer | `catalog replace REF FILE --format json` | job `catalog_replace`; document `fitment.catalog` with `changes` |
| `DELETE /v1/catalog/entries/{ref}` | sync | writer | `catalog delete ID --format json` (CU1) | `200` `fitment.catalog` with `changes` (M6-50) |
| `POST /v1/catalog/delete` | sync | writer | `catalog delete ID… --format json` | the same, all or none (CR2): unknown ids are `404 entry_not_found` listing them, nothing deleted |
| `POST /v1/catalog/reindex` | job | writer | `catalog reindex [REF…] [--all] --format json` | job `catalog_reindex`; document `fitment.catalog` with `changes` |
| `POST /v1/catalog/verify` | job | writer | `catalog verify [--deep] [--full] --format json` (CU2) | job `catalog_verify`; document `fitment.verify` (M6-60) |
| `POST /v1/catalog/gc` | job | admin | `catalog gc [--versions] [--vacuum] --format json` (CU2) | job `catalog_gc`; document `fitment.gc` |
| `POST /v1/catalog/prune` | job | admin | `catalog prune --revisions --format json` (CU1) | job `catalog_prune`; document `fitment.catalog` with `changes`. `{"revisions": true}` is required, as the flag is (CR11) |

```json
// POST /v1/catalog/entries
{ "files": [ { "upload": "sha256:1b77ea97…", "file_name": "m_bracket.stp", "input": "parts/m_bracket.stp", "id": null } ],
  "max_mib": null }                          // at most the upload cap (SZ1)

// PUT /v1/catalog/entries/BRKT-0042@r3
{ "upload": "sha256:46cd8a91…", "file_name": "bracket_thick.stp", "input": null, "max_mib": null }

// POST /v1/catalog/reindex                    neither field: every stale entry (RX9)
{ "refs": [], "all": false }

// POST /v1/catalog/verify, gc, delete
{ "deep": false, "full": false }
{ "versions": false, "vacuum": false }
{ "ids": ["BRKT-0042", "m_bracket@r2"] }
```

`file_name` is what ID3 builds a default id from and what the entry shows as its file; `input` is what `changes[].input` shows, the CLI's "argument as given", and defaults to `file_name`. Both are the client's display strings (RA8), passed to CA1, never a path. A file's `id` is ID1's and names that file's entry, as the CLI's `--id` does for its one file.

### 7.5 Assemblies

| Method and path | Mode | Role | CLI | Returns |
| --- | --- | --- | --- | --- |
| `POST /v1/assemblies` | job | reader | `fitment parts FILE --format json` | job `assembly_read`; result `/v1/assemblies/sha256:{hex}`, document `fitment.parts`. Already registered by the caller and current at this build: `200` with the `fitment.assembly`, no job |
| `GET /v1/assemblies?after=&limit=` | sync | reader | — | `fitment.page` of the caller's `fitment.assembly`, by SHA-256 |
| `GET /v1/assemblies/sha256:{hex}` | sync | reader | — | `fitment.assembly` |
| `GET /v1/assemblies/sha256:{hex}/parts` | sync | reader | `fitment parts FILE --format json` | `fitment.parts` bytes, `no-cache` (VN5), `Fitment-Exit-Code` 0 or 3; read by another build: `409 derived_stale` (RA3) |
| `GET /v1/assemblies/sha256:{hex}/step` | sync | reader | — | the stored bytes, re-hashed (OB3) |
| `GET /v1/assemblies/sha256:{hex}/sockets?after=&limit=` | sync | reader | — | `fitment.page` of the caller's socket lines of this assembly |
| `DELETE /v1/assemblies/sha256:{hex}` | sync | reader | — | `204`; `409 assembly_in_use` while a live socket line names it (RA6) |

```json
// POST /v1/assemblies
{ "upload": "sha256:124f4f64…", "file_name": "s01_bolted.stp" }
```

### 7.6 Sockets

| Method and path | Mode | Role | CLI | Returns |
| --- | --- | --- | --- | --- |
| `POST /v1/sockets` | job | reader | `fitment socket FILE --part X [--occurrence P] -o OUT` | job `socket_extract`; result the line's new revision; document `fitment.socket` |
| `POST /v1/sockets/import` | job | reader | (a file the CLI wrote) | job `socket_import`; result the line's new revision, `verified` true or false (RA10), or outcome `unchanged`; before any job, stale or refused: `422 socket_stale`, `socket_refused`, `socket_unreadable` (UP6); an extraction from the caller's assembly with another digest: the job `refused`, `socket_not_reproduced` |
| `GET /v1/sockets?assembly=&after=&limit=` | sync | reader | — | `fitment.page` of the caller's lines: id, assembly, target, occurrence, current revision, counts |
| `GET /v1/sockets/{ref}` | sync | reader | `fitment review FILE --format json` | `fitment.socket` bytes of that revision (a bare id: the current one) |
| `GET /v1/sockets/{ref}/revisions?after=&limit=` | sync | reader | — | `fitment.page`: per revision its number, how it was made, whether it is `verified` (RA10), its parent, the document's SHA-256 and `digest`, its versions, its unreviewed, carried and stale counts |
| `POST /v1/sockets/{line}@r{n}/review` | sync | reader | `fitment review FILE --keep/--remove/--unreview ID [--note T] [--socket S]`, once per decision | `201` with revision n+1's bytes and `Location` naming it; `409 revision_not_current` if n is not current; a bare id: `428 revision_required`; a stale revision: `422 socket_stale`, a tightening over the CLI (SN3) |
| `DELETE /v1/sockets/{line}` | sync | reader | — | `204`: a tombstone; revisions a retained run cites stay readable (RA6) |

```json
// POST /v1/sockets
{ "assembly": "sha256:124f4f64…",
  "part": "BRACKET",                 // as --part: PRODUCT.id, name or "#<n>"
  "occurrence": null,                // as --occurrence
  "carry": true,                     // carry decisions from the line's current revision
  "carry_from": null }               // or "sk_…@r2", or {"upload": "sha256:…"}: an older document (MF11)

// POST /v1/sockets/import
{ "upload": "sha256:5e0b…" }

// POST /v1/sockets/sk_3f2a…@r1/review
{ "decisions": [
    { "requirement": "grip:plane-150+plane-180", "socket": null, "status": "removed",
      "note": "the plate is reseated; grip checked by hand" },
    { "requirement": "bore:hole-41", "socket": "socket-1", "status": "kept", "note": null } ] }
```

- **The assembly** must be registered by the caller (`404 not_found` with `details.resource: "assembly"` otherwise).
- **Re-extraction is the CLI's `-o FILE` onto FILE.** Extracting onto an existing line carries the decisions of its current revision (the same source and target by construction). If the new document's bytes equal the current revision's, nothing is written and the job ends `succeeded` with outcome `unchanged`, unless the current revision is unverified (RA10): the extraction then makes a verified revision of the same bytes. `carry: false` starts from an unreviewed document, which becomes the next revision; the reviewed one stays readable.
- **`carry_from`** names another revision of the caller's lines of the same assembly and target, or an uploaded document of any version, read leniently through `SocketDocument::prior` as the CLI reads an older OUT, from at most `limits.max_socket_mib` bytes (`413 too_large` above); UP6's strict reading does not apply to it. A document for another file or target is `422 carry_from_foreign`: the CLI refuses that OUT without `--force`, and the API has no reason to accept it.
- **A refused extraction writes no revision** ("a refused result never overwrites FILE"): the job ends `refused`, exit 3, its document the refused `fitment.socket` at `/v1/jobs/{id}/document`.
- **A selection error** (`SelectError`) ends the job `refused`, exit 2, with its own code (`part_not_found`, `part_ambiguous`, …) and the candidates the CLI lists in `details.candidates` (M6-52).
- **Review decisions** apply in order, all or none, and make one revision whose bytes equal those of running `fitment review` once per decision in the same order. Values cannot be edited (M3 9.4): there is no `PUT` or `PATCH` of a socket document.
- **An import** makes a revision of the line its document names (source SHA-256, target, occurrence). With that assembly registered by the caller, the job extracts the same target and occurrence from it and requires an equal `digest`, and the revision is `verified: true`; without it, the revision is `verified: false`, and `original: true` has nothing to read (`409 original_not_stored`) (RA10).
- **A deleted line revives** when its triple is extracted or imported again: a new live row under the same id, its revisions numbered on from the deleted row's, carrying decisions only from a `carry_from` (RA4).

### 7.7 Features

| Method and path | Mode | Role | CLI | Returns |
| --- | --- | --- | --- | --- |
| `POST /v1/features` | job | reader | `fitment features FILE` | job `features`; document `features.v1`; `succeeded` (exit 0), `refused` (exit 3, the refused document) or `failed` (exit 1) |

```json
{ "upload": "sha256:23cb289f…" }
```

### 7.8 Matches

| Method and path | Mode | Role | CLI | Returns |
| --- | --- | --- | --- | --- |
| `POST /v1/matches` | job | reader; writer for `exhaustive` (AU5) | `fitment match S (CANDIDATE… \| --catalog DIR [--id ID… \| --exhaustive]) [--original ASM] [--clearance C] [--tolerance-mm X] [--tolerance-deg Y] [--require-fit] [--require-reviewed] --format json` | job `match`; result `/v1/matches/{id}`; document `fitment.match` |
| `GET /v1/matches?socket=&after=&limit=` | sync | reader | — | `fitment.page` of the caller's `fitment.run`, each stating `complete`, `exit_code` and `outcome` (null unless the job succeeded) |
| `GET /v1/matches/{id}` | sync | reader | the CLI's stderr summary and CM8 line | `fitment.run` (DC10) |
| `GET /v1/matches/{id}/document` | sync | reader | `… --format json` | `fitment.match` bytes, `Fitment-Exit-Code` 0, 4, 5, or 1 for a run kept with `complete: false` |
| `GET /v1/matches/{id}/candidates?offset=&limit=` | sync | reader | — | `fitment.page` of `{index, entry}`, each entry the document's own bytes of that candidate, sliced through the index built at the job's end, in MF5's order; the page states the document's `digest`, that it is a view, and the run's `complete` and `exit_code` (M6-8, DC12) |

A match's id is the id of the job that made it; its record and document follow the job's retention (JB12).

```json
// POST /v1/matches: a file run (the corpus's path)
{ "socket": "sk_3f2a…@r2",                          // or {"upload": "sha256:…"}: a socket file as the CLI reads it
  "candidates": {
    "files": [ { "upload": "sha256:23cb289f…", "label": "cand/c00_original.stp" },
               { "upload": "sha256:9a0c11e2…", "label": "cand/c06_blind.stp" } ],
    "components": [ { "assembly": "sha256:…", "part": "BRACKET", "occurrence": null, "label": null } ] },
  "policy": { "tolerance_mm": 0.01, "tolerance_deg": 0.01, "clearance": "strict" },
  "options": { "require_fit": false, "require_reviewed": false },
  "original": false }                                // true: the socket's own assembly, which the caller stored

// POST /v1/matches: a catalog run
{ "socket": "sk_3f2a…",
  "candidates": { "catalog": { "selection": "full" } },       // "exhaustive", or {"ids": ["BRKT-0042", "m_bracket@r3"]}
  "policy": { "clearance": "functional" } }
```

- `files` and `components` may be combined; either excludes `catalog` (CM1, `400 options_conflict`), and at least one candidate is required (`400 no_candidates`). Files of equal bytes are judged once with every label (MF3); components that resolve to the same (assembly, definition) likewise (CO3).
- `policy` and `options` follow DC7 and SN3. `ids` with `exhaustive` is `400 options_conflict`. `exhaustive` needs the writer role (`403 forbidden` below it), and a catalog with more live entries than `limits.max_catalog_candidates` ends the job `refused`, `too_large`, exit 3 (JS12).
- A component names one of the caller's registered assemblies and a part as `--part` does, and is judged through CO5; a selection error ends the job `refused` with its code (M6-52), and a selection in a refused assembly ends it `refused`, exit 3, `component_refused`. An `original: true` whose assembly cannot be read ends the job `failed`, `run_incomplete`, exit 1, `details.original` giving the reason (CO5).

### 7.9 Jobs

| Method and path | Mode | Role | Returns |
| --- | --- | --- | --- |
| `GET /v1/jobs?kind=&state=&after=&limit=` | sync | reader | `fitment.page` of the caller's `fitment.job`, by (creation time, id) |
| `GET /v1/jobs/{id}` | sync | reader | `fitment.job`; `?wait=N` (N ≤ 60) long-polls until it ends (JB11) |
| `GET /v1/jobs/{id}/events?after=SEQ` | sync, streamed | reader | `application/x-ndjson` (JB8) |
| `GET /v1/jobs/{id}/document` | sync | reader | the document the job produced, exact bytes, with `Fitment-Exit-Code`; `404 no_document` when there is none |
| `POST /v1/jobs/{id}/cancel` | sync | reader (own), admin (any) | `202` with the job (`cancel_requested: true`); `409 job_finished` once it has ended or cannot stop (JB5) |
| `POST /v1/jobs/{id}/pin` | sync | admin | `200` with the job, `pinned` set from `{"pinned": true \| false}` (JB12) |
| `DELETE /v1/jobs/{id}` | sync | admin | `204` for a finished job's record; what it made durable stays |

### 7.10 Auth

| Method and path | Mode | Role | Returns |
| --- | --- | --- | --- |
| `POST /v1/auth/login` | sync | none | `{username, password, token_name, days}` → `201` `fitment.token` with its `secret`, once; failures `401 invalid_credentials` (AU2), `429 rate_limited` (AU7); a user at `auth.max_live_tokens`: their oldest live root token evicted (AU10) |
| `GET /v1/auth/me` | sync | reader | `fitment.user` of the caller, with the presenting token's id and effective role |
| `POST /v1/auth/password` | sync | reader | `{current, new}` → `204` (AU1); every other token of the caller revoked (AU4) |
| `GET /v1/auth/tokens` | sync | reader | `fitment.page` of the caller's `fitment.token`, never a secret |
| `POST /v1/auth/tokens` | sync | reader | `{name, role, days}` → `201` `fitment.token` with its `secret`, once, its `parent` the presenting token; a role above the presenting token's effective role, or an expiry past the presenting token's: `400 invalid_parameter`; at `auth.max_live_tokens`: `409 too_many_tokens` (AU10) |
| `DELETE /v1/auth/tokens/{id}` | sync | reader (own), admin (any) | `204`: revoked, with every token minted from it (AU4); `current` names the presenting token (a logout); a minted token reaches only itself and its descendants (AU10) |
| `GET /v1/auth/users`, `POST /v1/auth/users` | sync | admin | `fitment.page` of `fitment.user`; `{username, role, password?}` → `201` |
| `PATCH /v1/auth/users/{id}` | sync | admin | `{role?, disabled?}` → `200` `fitment.user`; takes effect at the next request, disabling revoking every token of the user (AU4); the last enabled admin is `409 last_admin` |

### 7.11 The documents the API adds

| Format | Served by | Holds | Within v1 |
| --- | --- | --- | --- |
| `fitment.job` | `/v1/jobs/{id}`, job-creating requests | below | may grow |
| `fitment.job.event` | `/v1/jobs/{id}/events` | JB8 | may grow |
| `fitment.run` | `/v1/matches/{id}` | below (DC10) | may grow |
| `fitment.error` | every error, inside problem+json; a failed job's `error` | section 7.12 | may grow; codes added, never changed |
| `fitment.page` | every list but the catalog's | `{kind, items, next, generation}` (VN3, VN4); a candidates page also `{digest, view, complete, exit_code}` (DC12) | may grow |
| `fitment.info` | `/v1/info` | DC11 | may grow |
| `fitment.upload` | `/v1/uploads/…` | `{sha256, bytes, held_by: ["upload", "assembly", "socket", "job"], pinned_by_jobs}`, the caller's holdings only | may grow |
| `fitment.assembly` | `/v1/assemblies/…` | `{sha256, bytes, file_names, status, root, parts: {href, extractor_version, socket_rules_version, current}, sockets: {href, count}}` | may grow |
| `fitment.token` | `/v1/auth/login`, `/v1/auth/tokens` | `{id, name, role, hint, parent, created, expires, last_used, revoked, secret}`, `parent` the token it was minted from (null for a login's), `secret` only in the creating response (M6-11, AU10) | may grow |
| `fitment.user` | `/v1/auth/me`, `/v1/auth/users` | `{id, username, role, disabled, token: {id, role} \| null}` | may grow |
| `fitment.verify`, `fitment.gc` | `catalog_verify`, `catalog_gc` jobs; the CLI's `--format json` | CA5 | frozen: result documents |

`fitment.job` v1:

```json
{ "format": "fitment.job", "format_version": 1,
  "id": "j_3vq8k2m4n7p1r5t9w0x6y2z4ab",
  "kind": "match",
  "owner": "u_8f2kq0w4e6r1t3y5u7i9o2p4as",
  "state": "succeeded",
  "attempt": 1,
  "request": { "socket": "sk_3f2a…@r2", "candidates": { "…": "the normalized request: defaults filled, references resolved, SHA-256s" } },
  "request_sha256": "…",
  "queue_position": null,
  "progress": { "phase": "judging", "done": 14, "total": 14 },
  "cancel_requested": false,
  "exit_code": 4,
  "outcome": "interface_match",
  "result": { "href": "/v1/matches/j_3vq8k2m4n7p1r5t9w0x6y2z4ab", "format": "fitment.run" },
  "document": { "href": "/v1/jobs/j_3vq8k2m4n7p1r5t9w0x6y2z4ab/document", "format": "fitment.match",
                "sha256": "…", "bytes": 482113 },
  "error": null,
  "notes": [ { "code": "summary", "message": "14 candidates: 0 match, 5 interface match, 9 reject; fit not checked (no fit verifier in this build)" } ],
  "pinned": false,
  "times": { "created": "2026-11-02T09:14:03Z", "started": "2026-11-02T09:14:03Z", "finished": "2026-11-02T09:14:09Z",
             "expires": "2026-12-02T09:14:09Z" } }
```

`fitment.run` v1 (a catalog run):

```json
{ "format": "fitment.run", "format_version": 1,
  "id": "j_3vq8k2m4n7p1r5t9w0x6y2z4ab",
  "socket": { "ref": "sk_3f2a…@r2", "upload": null, "sha256": "<the socket bytes' SHA-256>", "digest": "sha256:…",
              "assembly": "sha256:124f4f64…", "how": "review", "verified": true, "unreviewed": 7 },
  "candidates": { "kind": "catalog", "selection": "full",
                  "generation": 412, "entries": 100083, "searched": 100066, "quarantined_not_searched": 17,
                  "level_one_kept": 95, "named": null, "judged": 95, "distinct_judgments": 91 },
  "policy": { "name": "default", "linear_mm": 0.01, "angular_deg": 0.01, "clearance": "strict", "grip": "hard" },
  "options": { "require_fit": false, "require_reviewed": false, "require_reviewed_by": null },
  "original": null,
  "versions": { "extractor_version": 6, "socket_rules_version": 7, "signature_version": 2, "match_rules_version": 1,
                "server": "0.3.0" },
  "fit_verifier": null,
  "summary": { "candidates": 95, "match": 0, "interface_match": 11, "reject": 84, "refused": 0 },
  "outcome": "interface_match", "exit_code": 4, "complete": true,
  "problems": { "corrupt": [], "bugs": [], "original": null, "incomplete_summary": null },
  "document": { "href": "/v1/matches/j_3vq8k2m4n7p1r5t9w0x6y2z4ab/document", "sha256": "…", "bytes": 3311902, "digest": "sha256:…" } }
```

For a file run, `candidates` is `{"kind": "files", "files": n, "components": m, "distinct": k}`, and each upload's SHA-256 is in the job's `request`. `policy` repeats the document's. `socket.how` and `socket.verified` are the revision's (RA10; for an uploaded socket, `how` is null and `verified` false). For a run that did not succeed, `outcome` and `summary` are null and `problems.incomplete_summary` holds the counts of what was judged, in `summary`'s shape, beside `complete: false` (M6-59).

An error (problem details with fitment's members):

```json
HTTP/1.1 422 Unprocessable Content
Content-Type: application/problem+json

{ "type": "https://fitment.dev/errors/socket_stale",
  "title": "The socket was read by older rules",
  "status": 422,
  "detail": "the socket was extracted with socket rules 6; this build is 7: extract it again (POST /v1/sockets), which carries its decisions over",
  "format": "fitment.error", "format_version": 1,
  "code": "socket_stale",
  "class": "refused",
  "exit_code": 3,
  "retryable": false,
  "details": { "socket_rules_version": 6, "build_socket_rules_version": 7, "extractor_version": 6, "build_extractor_version": 6 },
  "request_id": "r_5k2m8n0p3q6s9t1v4w7x0y3z5a" }
```

The `type` URI's host is a placeholder for the project's documentation site (question 6); its last segment is the code.

### 7.12 The error codes

| Code | Class | HTTP | CLI | From |
| --- | --- | --- | --- | --- |
| `bad_request` | usage | 400 | 2 | malformed JSON, a wrong type, an unknown field, a duplicate key, a byte-order mark (VN6) |
| `invalid_parameter` | usage | 400 | 2 | a tolerance zero, negative, non-finite, `-0.0` or (angular) ≥ 90; a bad `limit` or `days`; `max_mib` above the cap; `prune` without `revisions`; too many candidates (HX2); a new token's role or expiry above the presenting token's (AU10) |
| `invalid_reference` | usage | 400 | 2 | `BadReference`: an id outside ID1, a malformed `@r` (ID9), a short or non-hex SHA-256, a malformed `sk_` reference |
| `not_an_id` | usage | 400 | 2 | `Unnamed::NotAnId`: a content reference to delete |
| `name_invalid` | usage | 400 | — | RA8: a label, `input` or `file_name` too long, with control characters, or (a `file_name`) a separator, NUL, `.` or `..` |
| `label_reserved` | usage | 400 | — | RA8: a label starting `catalog:` |
| `no_candidates` | usage | 400 | 2 | CM1: neither files, components nor a catalog |
| `options_conflict` | usage | 400 | 2 | CM1: files or components with a catalog; `ids` with `exhaustive` |
| `not_found` | usage | 404 | — | an unknown job, match, socket line, assembly, upload, token or user, or another user's (AU8); a malformed server id (HX5) |
| `upload_not_found` | usage | 404 | 1 | UP5: the CLI's "cannot read FILE" is exit 1, but a missing upload is the client's mistake |
| `entry_not_found` | usage | 404 | 2 | `BadName` none, with `details.suggestions` (ID7) |
| `no_document` | usage | 404 | — | a job that produced none |
| `method_not_allowed` | usage | 405 | — | |
| `not_acceptable` | usage | 406 | — | VN8 |
| `upload_too_slow` | usage | 408 | — | HX3: an upload under `limits.upload_min_kib_s`; nothing kept; retryable |
| `reference_ambiguous` | usage | 409 | 2 | `BadName` several, with `details.matches` |
| `revision_not_current` | usage | 409 | 2 | `Unnamed::NotCurrent` (ID9); a review naming an old revision (RA7) |
| `entry_deleted` | usage | 409 | 2 | `Unnamed::Deleted` |
| `assembly_in_use` | usage | 409 | — | RA6 |
| `upload_in_use` | usage | 409 | — | RA6, UP5 |
| `original_not_stored` | usage | 409 | — | `original: true` without the socket's assembly stored by the caller (SN9) |
| `job_finished` | usage | 409 | — | a cancel after the end, or of a job that cannot stop (JB5) |
| `job_not_finished` | usage | 409 | — | `DELETE /v1/jobs/{id}` of a job that has not ended (JB12; added in PR 5) |
| `too_many_tokens` | usage | 409 | — | AU10: the user holds `auth.max_live_tokens` live tokens |
| `username_taken` | usage | 409 | — | `POST /v1/auth/users` (and AU9's `user add`) with a username another user holds, compared folded (added in PR 3) |
| `last_admin` | usage | 409 | — | `PATCH /v1/auth/users/{id}` disabling or demoting the last enabled admin (added in PR 3's review); AU9's offline `user set`, the recovery path, may |
| `bytes_not_kept` | usage | 410 | 2 | ID9: pruned or deleted bytes |
| `derived_rows_gone` | usage | 410 | 2 | `get --format features` of a superseded revision (IN9) |
| `precondition_failed` | usage | 412 | — | `If-Match` on a page of another generation (VN4) |
| `body_too_large` | usage | 413 | — | a JSON body over its cap, or a body on a route that takes none (HX2) |
| `unsupported_media_type` | usage | 415 | — | a wrong `Content-Type`; any request `Content-Encoding` (DC6) |
| `upload_digest_mismatch` | usage | 422 | — | UP2 |
| `idempotency_key_reused` | usage | 422 | — | JB9 |
| `carry_from_foreign` | usage | 422 | 2 | another file or target ("OUT holds something else") |
| `revision_required` | usage | 428 | — | RA7 |
| `rate_limited` | usage | 429 | — | AU7; retryable |
| `too_many_jobs` | usage | 429 | — | JS5, the owner's cap; retryable |
| `too_many_streams` | usage | 429 | — | JS10; a `limit=all` listing past its cap (HX8); an upload past UP4's slots, per user or per server (added in PR 4); retryable |
| `nothing_selected`, `part_not_found`, `part_ambiguous`, `part_is_assembly`, `occurrence_not_found`, `occurrence_ambiguous`, `occurrence_mismatch` | usage | — | 2 | `SelectError::code()` inside a `socket_extract` or `match` job (`refused`), with `details.candidates` and `details.occurrences` |
| `review_unknown_requirement`, `review_unknown_socket`, `review_differs_between_sockets`, `review_note_required`, `review_note_too_long` | usage | 422 | 2 | `ReviewError` |
| `id_conflict` | usage | — | 2 | `Outcome::Conflict` under a given id, inside an ingest (`refused`); other conflicts are lines of the document |
| `duplicate_content` | usage | — | 2 | `NotReplaced::Duplicate`, `Outcome::DuplicateUnderId`, inside a replace or ingest (`refused`) |
| `review_refused_document` | refused | 422 | 3 | `ReviewError::Refused` |
| `socket_unreadable` | refused | 422 | 3 | `SocketFormatError`, with `details.reason` one of `json`, `format`, `version`, `digest`, `invalid`, `utf8`, and for `json` the line and column |
| `socket_refused` | refused | 422 | 3 | `SocketRefused::Refused`, with `details.refusals` |
| `socket_stale` | refused | 422 | 3 | VD9, MF11, with both pairs of versions and the hint to extract again |
| `socket_unreviewed` | refused | 422 | 3 | `SocketRefused::Unreviewed`, with `details.count` and `details.first` |
| `socket_not_reproduced` | refused | — | 3 | RA10: an import whose extraction from the caller's assembly has another `digest`, with both digests |
| `component_refused` | refused | — | 3 | CO5: a component selected in an assembly the gate refuses, with `details.refusals` |
| `entry_stale` | refused | 409 | 3 | `get --format features` of a stale entry |
| `derived_stale` | refused | 409 | — | RA3 |
| `too_large` | refused | 413 | 3 | UP2, UP6, SZ1, `derived_too_large` (SZ2); inside a job, an exhaustive run over more than `limits.max_catalog_candidates` live entries (JS12), with `details.limit` |
| `socket_blocked` | refused | — | 3 | `NotRun::Blocked` (IX8) in a full run, with the requirements that fail every candidate |
| `catalog_stale` | refused | — | 3 | `NotRun::Stale` (RX3), with the counts and whether a reindex is queued |
| `catalog_empty` | refused | — | 3 | `NotRun::Empty` |
| `file_refused` | refused | — | 3 | `NotReplaced::File`: a replace with a gate or size refusal (CR4), `details.refusals` |
| `catalog_unavailable` | error | 503 | 1, 2 or 3 | the catalog cannot be opened, `details.reason` one of `newer_schema` (3, as DB5), `not_a_catalog`, `linked`, `not_unicode` (2, as the CLI's `NotACatalog`, `Linked` and `NotUnicode`), `unreadable` (1) (CX7) |
| `catalog_busy` | error | 503 | 1 | `Error::Busy` (CC6, CX3, CX4); retryable |
| `request_timeout` | error | 503 | — | HX3, AU2's queue, HX8's parse queue; retryable on `GET`, `HEAD` and an upload's `PUT` only: on another write route the work may have been done (ER6) |
| `queue_full` | error | 503 | — | JS5, a lane's cap; retryable |
| `shutting_down` | error | 503 | — | SD1; retryable |
| `storage_full` | error | 507 | 1 | a write refused by the disk or the catalog's page quota (SZ5) |
| `quota_exceeded` | error | 507 | — | UP4: an upload, or a job submitted while the owner's held bytes are at the quota; an upload by a user at `limits.user_max_uploads` live uploads (added in PR 4's review) |
| `blob_damaged` | error | 500 | 1 | ST11 on a catalog download; RX5 inside a reindex; OB3 on a stored object |
| `catalog_damaged` | error | 500 | 1 | `Error::Corrupt` on a synchronous read |
| `io_error` | error | 500 | 1 | any other storage failure; its text logged, not sent |
| `internal` | error | 500 | 1 | an `internal` refusal, a panic, a broken invariant, a socket-line id collision; nothing beyond the code and `request_id` |
| `run_incomplete` | error | — | 1 | a match document written with exit 1: QR4's damaged rows, a judgment that panicked; `details.corrupt`, `details.bugs`; an `original: true` whose assembly cannot be read, `details.original` (CO5) |
| `original_fails_own_socket` | error | — | 1 | VD15: "this is a fitment bug" |
| `shortlist_unsound` | error | — | 1 | SL1 or KH9 at run time |
| `deadline_exceeded` | error | — | — | JB7 |
| `job_interrupted` | error | — | — | JB6, with `details.generation_at_start` and `generation_at_recovery` for a catalog job; retryable (submit again), except for `catalog_replace` and `catalog_prune`, whose client reads the entry or the catalog first (JB6, ER6) |
| `unauthenticated` | auth | 401 | — | no or bad credentials, an expired or revoked token, a disabled user (`WWW-Authenticate: Bearer`) |
| `invalid_credentials` | auth | 401 | — | a failed login (AU2) |
| `forbidden` | auth | 403 | — | a role too low for the operation, or for a field that raises it (`exhaustive`: AU5) |

A code with HTTP "—" occurs only in a job's `error`, the job `refused` for the usage and refused classes and `failed` for the error class.

### 7.13 What each CLI refusal does at the API

| CLI behaviour | API |
| --- | --- |
| Socket document unreadable, refused, stale, or unreviewed under `--require-reviewed`: exit 3, nothing on stdout | `POST /v1/matches` answers `422` with `socket_unreadable`, `socket_refused`, `socket_stale` or `socket_unreviewed`; no job (JB2). Checked again at dispatch: a build or `require_reviewed` floor that changed while the job waited ends it `refused`, exit 3, with the same code |
| Catalog stale, empty, or a blocked socket in a full run: exit 3, nothing on stdout | the job ends `refused`, exit 3, with `catalog_stale`, `catalog_empty` or `socket_blocked`; no document, no run record |
| Unknown or non-current `--id`: exit 2 | `404 entry_not_found` or `409 revision_not_current` at the request where it can be decided without a snapshot; inside the job otherwise (`NotRun::Unnamed`), `refused`, exit 2 |
| A corrupt row, a panicking judgment, a pass after a shortlist drop, the original failing its own socket: exit 1 with the document | `failed` (`run_incomplete`, `shortlist_unsound`, `original_fails_own_socket`); document kept, `complete: false`, `Fitment-Exit-Code: 1`; `outcome` null everywhere, what was judged under `problems.incomplete_summary` (M6-59) |
| Every candidate rejected: exit 5 | `succeeded`, `exit_code` 5, `outcome: reject` |
| `--require-fit` with interface matches only: exit 5 | `succeeded`, `exit_code` 5 |
| A refused candidate | a `Reject` entry with `candidate_refused`, as in the document; a note with its first reason |
| A file the gate refuses at ingest: quarantined, exit 3 | `refused`, `exit_code` 3, the line `quarantined`, the other lines as committed; never searched in a full run (CR8) |
| A file the gate refuses at replace: exit 3, entry unchanged | `refused`, `file_refused`; the entry unchanged |
| `verify` finds a problem: exit 3 | `refused`, `exit_code` 3, the `fitment.verify` document listing it |
| A refused file's features, a refused assembly, a refused socket extraction: exit 3 with the document | `refused`, `exit_code` 3, the refused document at `/v1/jobs/{id}/document` |

## 8. Measurements and findings

### 8.1 What B's prototype showed

The prototype (`m6-proto/`: `Cargo.toml`, `src/main.rs`, and `Cargo.toml.v1` and `.v2` as variants) is a throwaway crate with path dependencies on the branch's `fitment-core` and `fitment-catalog`.

- **It builds on Rust 1.85** (`cargo +1.85 build --release`) with axum 0.8.9, tokio 1.53.3, hyper 1.12, hyper-util 0.1.21, tower 0.5.3, tower-http 0.7.1 (and 0.6.11), argon2 0.6.0 (and 0.5.3), getrandom 0.4 (and 0.3), toml 1.1.8 (and 0.9.12), tracing-subscriber 0.3.23, schemars 1.2.3, rusqlite 0.40.2 and clap 4.6.7. One API difference: argon2 0.6 has no `SaltString`; `hash_password(&[u8])` makes the salt itself. `sha2` 0.11 declares 1.88 and was not tried.
- **`cargo deny check licenses` passed** with the repository's `deny.toml` unchanged.
- **`cargo deny check bans` failed in exactly two expected ways:** rusqlite used by a crate that is not a listed wrapper (M6-32), and serde_json's exact features, which gained `raw_value` from axum's `json` feature and `alloc` from schemars. With axum's `json` off and schemars removed, serde_json's features were exactly `default`, `std` and `float_roundtrip` (M6-34, SV9).
- **`hyper-util`'s `server-auto` pulls in `h2`,** and the plain `http1::Builder` does not, so the accept loop uses the latter (HX1); the final graph had no `h2`.
- **The pieces fit:** a `Catalog` opened inside `spawn_blocking`, with a snapshot, `list` and `generation`, behind a semaphore permit; an `Arc<dyn FitVerifier + Send + Sync>` holding `NoFitVerifier`; argon2id with the pepper as its secret, 20.9 ms for one hash and one verification at m = 19 MiB, t = 2, p = 1.
- **The graph** with schemars, clap and toml had 140 crates.
- **Smoke test:** the health route answered; a 1 MiB upload streamed to a temporary file hashed to the same SHA-256 as `shasum -a 256`; the catalog route on a fresh catalog answered "0 entries at generation 1"; a stalled header was dropped at 10 s.

### 8.2 The CLI work the exit mirrors (C)

Measured on 2026-10-17 at `bbc89c0`, release build, Apple M5 Max (18 cores), with `measure.py`, over `corpus/out`: 462 STEP files; 125 match cases on 33 socket keys, 23 with file candidates and 10 with only component candidates; r27's refused document and s01's three socket-gate edits among them. It times the floor under the API path's cost:

| Step | Runs | Time | Documents |
| --- | --- | --- | --- |
| `fitment socket` per socket key | 33 | 0.3 s | — |
| `fitment match` per socket and policy, over its file candidates (109 distinct files in 115 cases) | 44 (42 exit 4, 2 exit 5) | 1.6 s | 2.4 MB, largest 0.41 MB |
| `fitment catalog ingest` of every `.stp` but `h_*` | 458 files | 3.8 s | — |
| full catalog runs, both policies | 66 (42 exit 4, 24 exit 3: blocked or refused) | 4.2 s | 20.6 MB, largest 3.31 MB |
| exhaustive catalog runs, both policies | 66 | 10.0 s | 135.2 MB, largest 5.60 MB |

Corpus facts the design relies on, read from the manifest: no match case states a review (`socket.review` is empty in all 125), hence M6-38; the 10 component candidates are all `b_unverifiable` originals, each the only candidate of its socket (s11, s12, s13 twice, s17, s47, s48, s52 twice, s58), hence M6-5; the false-negative baseline holds 41 entries over 121 cases at `MATCH_RULES_VERSION` 1 (the four socket-gate cases are not judged); no rule id is stated in bold by two of M2 to M5's designs.

### 8.3 Findings in the current code

Read at `bbc89c0`; each is a change of section 5 or a rule that depends on it.

- **`Catalog::ingest` records paths** (A 14, B CA1): `ingest(&[PathBuf], Option<&Id>)` takes one id for one file and writes each path as `changes[].input`, its final component as `file_name`, and the path into error lines ("cannot read …"). Ingesting uploads from staging paths would put server paths into documents. Hence CA1.
- **`verify` reports an absolute path** (A 14): `Problem.what` for a missing fan-out directory is `fan.display()` (`crates/fitment-catalog/src/verify.rs`, around line 630). Hence CA4.
- **`fitment_catalog::Error`'s messages hold paths** (`NotACatalog`, `NotEmpty`, `Linked`, `NotUnicode`, `InsideCatalog`, `Io`): harmless in a CLI run on the user's own files; at the API they are mapped to codes (CX7, ER3).
- **The CLI has no JSON for `delete`, `prune`, `verify` and `gc`**, though M5's 7.11 says `changes` is filled by `delete` and `prune` with `--format json` (`DeleteArgs` and `PruneArgs` in `cmd/catalog/change.rs` have no `format`). Hence CU1, CU2 and CA5.
- **`schemars` was never adopted.** Decision 11 planned it behind `fitment-core`'s `schema` feature and `fitment schema <kind>`; neither exists. Hence CO4 and CA6 (the `fitment schema` command stays unbuilt).
- **The tolerance policy's name** is `cli` whenever the values differ from D9's (`cmd/match_.rs`, `TolerancePolicy::new("cli", …)`). The API keeps it (DC7). The CLI has no grip flag (D7), so A's `grip` request field is dropped.
- **Components are ordered but never merged.** MF5's last key for a component is `<assembly sha256>#<definition>` (`matching/document.rs`, `identity`), so two different components never tie, but the merge of equal candidates groups by a file's SHA-256 only. Hence CO3.
- **The run's exit rules are private to the CLI** (`reported`, `original_bug` in `cmd/match_.rs`). Hence CO2.
- **`FitVerifier: Sync`** (`fit.rs`, line 104), not `Send + Sync`. The server wraps it (SV8); core is unchanged.
- **`Catalog` is `Send`, not `Sync`, and `snapshot` takes `&mut self`** (`snapshot.rs`, line 194). Hence CX1's pool of reader catalogs.
- **Versions:** `EXTRACTOR_VERSION` 6, `SOCKET_RULES_VERSION` 7, `SIGNATURE_VERSION` 2, `MATCH_RULES_VERSION` 1, `PROPERTIES_VERSION` 1, catalog schema 1, workspace 0.2.0, `rust-version` 1.85; `deny.toml` allows serde_json's `default`, `std` and `float_roundtrip` exactly, and rusqlite only for `fitment-catalog`.

## 9. Verification

### 9.1 The exit test (XH)

- **XH1. The exit set** (C XH1; M6-5, M6-38). Through the API, the exit runs:
  1. every match case of the corpus manifest (125 today; whatever it holds at the exit), under `default` and `functional`, file and component candidates alike;
  2. the socket-gate cases (`edit`: `stale`, `digest`, `unreviewed`) and r27's refused document, each refused before any candidate is read (UP6, JB2);
  3. the review cases PR 1 adds, and KC13's runs as `original.rs::review_waives_only_what_it_removes` states them (every corpus socket whose original fails requirements no candidate can pass, with those removed in review: the original passes, a plain block is still rejected);
  4. KC11's own-frame original runs for the corpus targets, through `original: true` (SN9);
  5. the catalog path (XH8), under `default`, `functional` and `tolerance_mm` 0.05, as CT1 does.

  A part of the set the API cannot pose is listed in the exit report with its reason and counted; the report states how many runs of each kind ran, and the test fails if any count is below the manifest's.
- **XH2. Through the API means over HTTP** (C XH2; M6-36). Every input reaches the server as bytes in a request, and every output the test scores is read from a response. The test never writes into the server's state or catalog directory. It runs the CLI on the server's catalog, read-only, as a test harness, and only with the server stopped or idle (no job running or queued): XH8's parity on the same catalog and its closing `verify --deep` (review question 3).
- **XH3. One socket's sequence** (C XH3; M6-37). For each socket key (source, part, review, edit, options) of the set, in manifest order:
  1. upload the source assembly: the SHA-256 the API states equals the test's own of the same bytes; register it: its `fitment.parts` equals `fitment parts SRC --format json`, byte for byte;
  2. extract the socket: the document equals `fitment socket SRC --part P --format json` byte for byte, or the refused document byte for byte with the job `refused`, exit 3;
  3. apply the review entries, if any: the document equals `fitment review` applied to the CLI's copy, byte for byte;
  4. for each policy, upload every file candidate of the cases on that socket and label it with its manifest file name, then run one match job over all of them: its document equals `fitment match S f1 f2 … --format json --clearance strict|functional`, run in the corpus directory with the same bare names, byte for byte (as `the_binary_agrees_with_the_library` groups them today);
  5. for each component candidate, a match job over `{assembly, part}`: its document equals the library's `MatchDocument` from `judge_component` (CO5) with the label the API used (RA8's default), byte for byte;
  6. every document passes the checker (KH12, EV7), states the policy requested (KH2.6), names the socket digest of step 2 or 3, and names as sources exactly the SHA-256s uploaded in step 4 or 5: a job never answers with another job's document.
- **XH4. Documents are core's bytes** (C XH4; DC1). XH3's parity is the test of DC1: a document the API serves is the byte string core wrote, the form `--format json` prints. A response that differs only in whitespace or key order fails.
- **XH5. Truth on every case** (C XH5). Each case's entry is found in its job's document by `source.sha256`, or by component, never by label (SC9); it is turned into an `Outcome` and held to the strict gate (KH2, KH3) under that policy. Over the whole set: no false positive (KH7's definition, unchanged: a non-`Reject` on a case whose truth does not fit under the policy); the false negatives, as KH4 entries without `cause`, equal the committed baseline's entry for entry (case, policy, class, failed requirements and codes). The API suite never writes the baseline (SC5).
- **XH6. The report** (C XH6). The exit writes `target/fitment-corpus/api-report.json` with KH6's rows and totals, computed by `fitment-corpus` from the API's outcomes. It must equal `report.json`, which the library suite writes from the library's outcomes, byte for byte: the same rows, the same false positives (0), the same false negatives against physics (today 20 of 49 under `default`, 21 of 51 under `functional`). CI's `server-corpus` job diffs the two files.
- **XH7. Outcomes are exit codes** (C XH7; JB3, JB4). Every job's terminal state and exit code map to the code the CLI gave for the same run:

  | CLI exit | Meaning | API |
  | --- | --- | --- |
  | 0 | a `Match` | `succeeded`, outcome `match`; reachable only with the test-support fake verifier (VT31) |
  | 4 | the best is `InterfaceMatch` | `succeeded`, `interface_match` |
  | 5 | every candidate rejected, or `--require-fit` with interface matches only | `succeeded`, `reject` |
  | 3 | refused input; a search that could not run (stale or empty catalog, blocked socket) | a `422` before any job, or the job `refused`, with a stable code |
  | 2 | usage | a `4xx` before any job, or the job `refused` with exit 2 (a selection error, an ingest conflict) |
  | 1 | an error, an unreadable input, a damaged row, a fitment bug | the job `failed`, with a stable code; never `reject` |
- **XH8. The catalog path** (C XH8). On a fresh server:
  1. every corpus part file but the hostile `h_*` ones (458 today), uploaded and ingested through the API in sorted name order: each file's line (id, revision, SHA-256, status: added, duplicate, quarantined) equals the line of `fitment catalog ingest --format json` over the same files in the same order on a fresh CLI catalog;
  2. for every socket of the exit set and each of the three policies: the API's full run equals `fitment match S --catalog <the server's catalog directory> --format json …` byte for byte, and has the digest of the full run on the CLI-built catalog; the exhaustive run likewise, and its digest is the file run's over every live file (CT2); a blocked socket is refused in a full run (`refused`, exit 3), as in M5 (24 of 66 full runs today);
  3. for every case's file candidate, a named run (`ids: [<stem>]`): its digest is the file run's over that file, and its entry passes the strict gate (XH5);
  4. every label is `catalog:<id>@r<n>`, and the API's download of that reference returns the bytes whose SHA-256 the entry's `source` states (ST11's re-hash);
  5. afterwards, with the server idle, `fitment catalog verify --deep` on the server's catalog reports nothing.
- **XH9. Determinism** (C XH9). The file half of the exit runs twice in one test, against two fresh servers: one with `jobs.cpu = 1` and jobs submitted in manifest order, one with the default budget and jobs submitted in reverse order. Every document is the same bytes. Across operating systems, VT8 compares a smaller set on every PR.
- **XH10. A skipped exit is a failure in CI** (C XH10). Without the corpus the exit passes with a note, unless `FITMENT_REQUIRE_CORPUS=1`; in a debug build it passes with a note, as `catalog_corpus.rs` does. CI sets the variable and builds in release. The test asserts that it ran at least as many runs as the manifest's match cases times two, so a filter that drops cases cannot pass green.
- **XH11. No document goes unchecked** (C XH11). Every `fitment.match` any server test reads passes the checker, every `fitment.socket` reads back with `SocketDocument::from_json`, and every `features.v1` with `FeaturesDocument::from_json`. The shared test client does this on every response of those formats, so no test can forget.
- **XH12. Jobs keep four invariants under every fault** (C XH12; JB3; review F2). Every fault test (VT9 to VT13) and concurrency test (VT14 to VT16) checks:
  1. a job reaches exactly one terminal state and keeps it across restarts;
  2. a `succeeded` job has its document, which passes XH11 and equals the CLI's for its inputs;
  3. a `refused`, `failed`, `cancelled` or `interrupted` job states no verdict: its `outcome` is null in `fitment.job`, `fitment.run`, the `end` event and every listing, and a failed match's kept document is served only beside `complete: false` and `Fitment-Exit-Code: 1`, its counts under `problems.incomplete_summary`, never `summary` (an error is never a `reject`, which would read as "nothing fits");
  4. an ingest's effect on the catalog is all or nothing per batch, and `fitment catalog verify` is clean after the job ends, however it ends.

  VT44 tests the third against every way a job can end without succeeding.
- **XH13. No response leaks** (C XH13; ER3; review F8). Every response body captured in any server test is scanned for an absolute path of the test machine (the state and catalog directories are made under a recognizable prefix), SQL text, a Rust type path or panic message, and a password, token or `Authorization` value. Every log line is scanned for secrets only (a password, a token, an `Authorization` value, the pepper), since logs name paths for the operator, relative to the configured roots (OP1). The shared client and a tracing capture layer run the scan; a test that produces a leak fails.
- **XH14. Every route is covered** (C XH14, B AU5). Each operation of `OPERATIONS`, and so of the served OpenAPI document, has a row in the auth matrix (VT23), at least one golden (VT6) and a request in the fuzz seed corpus; an operation missing from one, a row naming no operation, or a route of the router missing from `OPERATIONS` fails.

```text
corpus/out/manifest.json  (125 match cases, 33 sockets)              fitment (CLI, release)
        │                                                                    │
        │ fitment-corpus: socket groups, cases, expectations                 │
        ▼                                                                    ▼
upload SRC.stp ──HTTP──► fitment-server ── assembly_read ─► fitment.parts  ══ fitment parts SRC
                                        ── socket job ────► fitment.socket ══ fitment socket SRC --part P
review entries ──HTTP──►                ── review ────────► fitment.socket ══ fitment review …
upload f1..fn  ──HTTP──►                ── match job ─────► fitment.match  ══ fitment match S f1..fn
{asm, part}    ──HTTP──►                ── match job ─────► fitment.match  ══ MatchDocument (library, same labels)
        │                                                     │
        │                          checker (KH12), policy, socket digest, sources (XH3.6, XH11)
        ▼                                                     ▼
  Outcome::find(doc, sha256) ──► gate(case, policy, sockets, outcome)   (KH2, KH3)
        │
        ▼
  Score: false positives = 0; false negatives == false-negatives.json; api-report.json == report.json
```

`══` is byte equality. The catalog half (XH8) has the same shape with ingest and catalog match jobs on the right and `fitment match --catalog` on the left.

**Why both parity and truth** (C 3.3). Truth without parity would accept an API document that passes the gate and differs from the CLI's in what the gate does not read: a measure, a flag the case does not name, a passed check's record, the order, the digest; each is something a user or M7 reads. Parity without truth would accept an API that shares a bug with the CLI, and both call the same core, so most bugs are shared; truth from the construction catches them, as it does for the CLI in M4. Together, by transitivity: the CLI's documents on the corpus are checked by M4's oracle (KO3 to KO6) and M5's catalog tests, and a byte-identical API document inherits every one of those checks without the oracle running against the server.

### 9.2 `fitment-corpus`, the scorer (SC)

What moves:

| From | What | To |
| --- | --- | --- |
| `crates/fitment-core/tests/support/gate.rs` (389 lines) | locators (KC5), `expectation`, the strict gate (KH2, KH3), the defect check at the construction's placement (KH8), `blocked` (VD5, VD6), `flags` (VD12, VR19) | `fitment-corpus/src/gate.rs`, on `Outcome` instead of `CandidateResult` (the defect check keeps a typed overload: only the library can place a candidate by the construction) |
| `crates/fitment/tests/matching.rs` | `fits`, `false_positive`, `false_negatives` (KH4's entries), the baseline's compare and bless (KH5), the report and its rates (KH6, KH7), the two policies | `fitment-corpus/src/{score,baseline,report,policy}.rs` |
| `crates/fitment/tests/common/checker.rs` (312 lines) | KH12, EV7's recomputation and the digest | `fitment-corpus/src/checker.rs`; `golden.rs`, `match_cli.rs`, `catalog_match_cli.rs` and `matching.rs` use it from there |
| five copies of `corpus()` (`corpus.rs`, `matching.rs`, `catalog_corpus.rs`, `original.rs`, `fixtures.rs`) | `FITMENT_CORPUS`, `FITMENT_REQUIRE_CORPUS`, the release-only skip | `Corpus::locate`, `Corpus::locate_release_only` |
| — | the manifest's match cases as a view: socket keys, candidates, truths, classes | `fitment-corpus/src/corpus.rs` |
| — | parity helpers: the first difference of two byte strings, a labels-stripped view, the outcome-to-exit table | `fitment-corpus/src/parity.rs` |

What stays: everything only the library can do. KH8's placement by the construction (`Matcher::verify_at`), KH9's shortlist cross-check, CT5's stored-document judging, level 1's admission, the original runs' typed checks (`original.rs`), the generator's own tests. `corpus.rs`'s coverage test stays in `fitment`, since it reads the repository's design documents, and grows as section 11 says.

- **SC1. One scorer** (C SC1; M6-39). The strict gate, what a false positive and a false negative are, the baseline, the report and the document checker exist once, in `fitment-corpus`; the library, binary, catalog and API suites all call it. PR 1 deletes the originals; no test file keeps a private copy of `fits` or `false_positive`.
- **SC2. The scorer judges an `Outcome`** (C SC2; M6-39). `fitment_corpus::Outcome` (a test view, not core's run `Outcome`) is a plain record of one candidate's result: verdict, status, refusal codes, per socket its checked requirements, its failed ones with their codes and its removed ones, the flags, the fit's kind and the policy. `Outcome::of_result` builds it from a library `CandidateResult`, `Outcome::of_entry` from a `fitment.match` entry. On every corpus run the two give equal `Outcome`s for the library's result and the binary's document (VT4). An `Outcome` is never turned back into a `Verdict` or a document (M4-23).
- **SC3. Never published** (C SC3). `publish = false` in its own manifest (the workspace's default is `true`); in `[workspace.dependencies]` as `fitment-corpus = { path = "crates/fitment-corpus" }`, without a version, and only ever a dev-dependency of `fitment` and `fitment-server`. `cargo publish` strips a dev-dependency without a version, so no published package names it; `tools/check-core.sh` checks that `cargo package --list` of each published crate is unchanged by its arrival and that no normal dependency edge reaches it.
- **SC4. Expectations from the construction only** (C SC4; KC1). The scorer reads what a case expects from the manifest, never from fitment's output; a locator that names no requirement, or several, is a harness error, never a pass (KC5).
- **SC5. One writer of the baseline** (C SC5; M6-47). Only the library suite blesses `false-negatives.json` (`FITMENT_BLESS=1`, refused under `CI`, refused for a known case without a `MATCH_RULES_VERSION` rise: KH5 unchanged). The binary, catalog and API suites compare their false negatives with it, keyed by (case, policy, class, failed), and take `cause` from it: whether alignment or verification missed a fit is a property of the case, decided by the library's `verify_at`.
- **SC6. Includable by path** (C SC6). `gate.rs` and `outcome.rs` sit at the crate's root, name each other as `crate::gate` and `crate::outcome`, and use nothing but `fitment_core` and `serde_json`, so `crates/fitment-core/tests/candidates.rs` includes them with `#[path]` as it includes `support/gate.rs` today. fitment-core has no dev-dependencies (M5 PR 1), and a dev-dependency on a crate that depends on it would link two copies of its types.
- **SC7. One way to find the corpus** (C SC7). `Corpus::locate` reads `FITMENT_CORPUS` or `corpus/out`, passes with a note when absent, and panics when `FITMENT_REQUIRE_CORPUS=1`; `locate_release_only` also passes with a note in a debug build. A suite that skipped says so on stderr.
- **SC8. Truths and rates** (C SC8; M6-38). `fits(case, policy)` is KC3's: `fits`, `fn_rule` and `collides` fit, `fits_functional` under `functional` only; `does_not_fit`, `edge` and `refused` do not. The new `waived` is neither: its expected verdict is `InterfaceMatch` with the removal listed, and it is counted in its own row, outside both rates, as `edge` is. A `waived` case passing is not a false positive (KC13: the user accepted it); a `waived` case rejected is a gate failure, not a false negative.
- **SC9. Entries by content** (C SC9). A case's entry in a document is found by its candidate's SHA-256 (MF3: one entry per SHA-256) or, for a component, by its source; never by label, which a client sets. Labels are compared separately, by parity.

Its API:

```rust
//! crates/fitment-corpus (publish = false): the corpus's scorer, shared by
//! the tests of fitment and fitment-server (docs/research/m6-api.md, SC1).

pub struct Corpus { /* dir, manifest */ }
impl Corpus {
    pub fn locate() -> Option<Corpus>;                      // SC7
    pub fn locate_release_only() -> Option<Corpus>;
    pub fn dir(&self) -> &Path;
    pub fn match_cases(&self) -> Vec<Case<'_>>;             // kind == "match", manifest order
    pub fn socket_groups(&self) -> BTreeMap<SocketKey, Vec<Case<'_>>>; // source, part, review, edit, options
    pub fn part_files(&self) -> Vec<PathBuf>;               // XH8's ingest set: every .stp but h_*
}

pub struct Case<'m> { /* the manifest entry */ }
impl Case<'_> {
    pub fn id(&self) -> &str;
    pub fn truth(&self) -> Truth;                           // Fits, FnRule, FitsFunctional, DoesNotFit, Edge, Collides, Refused, Waived
    pub fn class(&self) -> &str;
    pub fn socket(&self) -> &SocketKey;
    pub fn candidate(&self) -> CandidateRef;                // File(name) | Component { source, part }
    pub fn expectation(&self, policy: Policy) -> &Value;
    pub fn fits(&self, policy: Policy) -> bool;             // SC8
    pub fn socket_gate(&self) -> bool;                      // VD9, MF2
}

#[derive(Clone, Copy)]
pub enum Policy { Default, Functional, Tolerance005 }
impl Policy {
    pub fn name(self) -> &'static str;
    pub fn match_policy(self) -> fitment_core::MatchPolicy;
    pub fn cli_args(self) -> &'static [&'static str];       // --clearance …, --tolerance-mm 0.05
    pub fn request(self) -> Value;                          // the API's policy object (DC7)
}

#[derive(Debug, PartialEq)]
pub struct Outcome {                                        // SC2
    pub verdict: String, pub status: String, pub refusals: Vec<String>,
    pub sockets: Vec<SocketOutcome>, pub flags: Vec<String>, pub fit: String, pub policy: Value,
}
#[derive(Debug, PartialEq)]
pub struct SocketOutcome {
    pub socket: String, pub verdict: String, pub checked: Vec<String>,
    pub failed: Vec<(String, Vec<String>)>, pub removed: Vec<String>,
}
impl Outcome {
    pub fn of_result(result: &fitment_core::CandidateResult) -> Outcome;
    pub fn of_entry(entry: &Value) -> Result<Outcome, String>;
    pub fn find(doc: &Value, sha256: &str) -> Result<Outcome, String>; // SC9
}

pub mod gate {
    pub fn gate(case: &Case<'_>, policy: Policy, sockets: &[fitment_core::Socket], outcome: &Outcome) -> Vec<String>;
    pub fn defect(case: &Case<'_>, sockets: &[fitment_core::Socket], at_construction: &[fitment_core::SocketResult]) -> Vec<String>;
}

pub struct Run { pub case: String, pub policy: Policy, pub outcome: Outcome, pub path: RunPath } // Library | Binary | Catalog | Api
pub struct Score { /* runs */ }
impl Score {
    pub fn new(corpus: &Corpus, runs: Vec<Run>) -> Score;
    pub fn false_positives(&self) -> Vec<String>;
    pub fn false_negatives(&self) -> Vec<Value>;             // KH4 entries, cause from the baseline
    pub fn report(&self) -> Value;                           // KH6: byte-stable rows and totals
}
pub mod baseline {
    pub fn path() -> PathBuf;                                // crates/fitment/tests/corpus/false-negatives.json (M6-46)
    pub fn bless_or_compare(found: &[Value], cases: &[Value], causes: &[Value]) -> Result<(), String>; // library only (SC5)
    pub fn compare(found: &[Value]) -> Result<(), String>;   // every other path
}
pub mod checker { pub fn problems(doc: &Value) -> Vec<String>; }   // KH12, EV7
pub mod parity {
    pub fn bytes(what: &str, cli: &[u8], api: &[u8]) -> Option<String>;   // the first difference, with context
    pub fn without_labels(doc: &Value) -> Value;
    pub fn exit_code(state: &str, outcome: Option<&str>) -> Option<i32>;   // XH7's table
}
```

Its manifest holds `publish = false` (explicit: the workspace default is `true`), the workspace's version, edition, `rust-version` and licence, and depends on `fitment-core` and `serde_json` from the workspace. `deny.toml` gains `allow-wildcard-paths = true` under `[bans]` (the layout's M6 note) only if PR 1 shows that the pinned cargo-deny 0.20.2 needs it for this versionless dev-dependency (review question 6); if that key does not cover it either, PR 1's "As built" note says so and relaxes the wildcard check for that one edge in the narrowest way cargo-deny allows. A version may not be added instead: `cargo publish` would then keep the dev-dependency and ask crates.io for it.

### 9.3 C's assumptions, as the merge settles them

C stated what the exit needs of A and B as twelve assumptions, each with a fallback C would not accept for the first and ninth. The merge settles each:

| C's | Assumption | Settled |
| --- | --- | --- |
| 1 | Core documents served as their own resources, the exact bytes `to_json()` wrote | Holds: DC1, M6-37 |
| 2 | A file run's labels are the client's | Holds, and required for files: RA8, M6-57 |
| 3 | Component candidates and originals | Holds: M6-5, SN9 |
| 4 | Socket document upload through VD9's gate | Holds: M6-14, UP6, JB2 |
| 5 | Outcomes map one to one onto exit codes; an enumerable error set | Holds: JB3, JB4, XH7, `ErrorCode::ALL` (ER1) |
| 6 | Policy fields mirror the CLI's flags | Holds: DC7 (`tolerance_mm`, `tolerance_deg`, `clearance`, `require_fit`, `require_reviewed`) |
| 7 | A catalog run's record states the generation and CM8's counts | Holds: M6-25, DC10 |
| 8 | The server's stored inputs in the catalog directory (6.3's schema 2) | Does not hold (M6-3): they live in `server.db`. XH8 is unaffected: the catalog directory is an M5 catalog at schema 1, which the CLI reads |
| 9 | Test seams under `test-support`; the bound address first on stdout | Holds: OP11, and stdout carries only that line (M6-54) |
| 10 | Limits as configuration | Holds: OP3 |
| 11 | Vectera's auth with sessions; roles admin, editor, viewer; each route's role in OpenAPI | Changed: bearer tokens only (M6-18), roles reader, writer, admin (M6-17); each operation's role is `x-fitment-role` (VN7) |
| 12 | `NoFitVerifier`; a fake only under `test-support` | Holds: SN11, OP11 |

### 9.4 Test layers for the server

| Layer | Where | How | Shows | Runs |
| --- | --- | --- | --- | --- |
| Unit | `crates/fitment-server/src/**` | `#[cfg(test)]` | the job state machine, limits, the error mapping, the operation table | every PR, 3 OSes |
| Router, in process | `tests/{api,golden,errors,auth,validation,parity,openapi,hostile}.rs` | `Server::router().oneshot(request)` on a current-thread runtime, `Clock` and `IdGen` fixed (OP11) | every route's documents, codes and permissions; malformed bodies; goldens exact to the byte; in `hostile.rs`, every bound of `M6_LIMITS` that needs no wire (VT17, VT19, VT20, and the limit cases of VT24, VT41, VT42 and VT45) | every PR, 3 OSes, debug |
| Bound socket | `tests/{wire,slow,concurrency}.rs` | `TcpListener::bind("127.0.0.1:0")` and `Server::serve` on a multi-thread runtime; the `hyper-util` client and raw `TcpStream` (M6-40) | what only the wire shows: header and body timeouts, slow clients, disconnects mid-upload and mid-download, connection caps, many clients | every PR, 3 OSes |
| Process | `tests/{process,fault,exit}.rs` | `CARGO_BIN_EXE_fitment-server` as a child with `--listen 127.0.0.1:0`, the address read from stdout's one line; `abort()` at a hook, SIGTERM, `TerminateProcess` and stdin's end of file on Windows | configuration, startup refusals, shutdown, restart with jobs in flight, the exit (M6-36) | every PR (the exit: the corpus job, release) |
| Corpus | `tests/exit.rs` | the process layer and `fitment-corpus` | XH1 to XH11 | `server-corpus`, release |
| Container | CI `container` job | `docker build`, `docker run` | the image as shipped (VT29) | every PR, Linux |
| Fuzz | `fuzz/fuzz_targets/` | `cargo fuzz`, nightly | no panic, no unexpected 5xx (VT26) | weekly, Linux |
| Scale and load | `tests/scale.rs`, `.github/workflows/scale.yml` | CT11's scaled catalogs through the API | ceilings, timings reported (VT27, VT28) | weekly; 10⁵ by hand |

**Why three transports.** `oneshot` drives the router as a `tower::Service`: no connection and no hyper, so it is fast, deterministic and enough for everything the handlers decide. It cannot show a header timeout, a slow body, a disconnect, a connection cap, hyper's own header limit, or the binary's configuration and signal handling. The bound socket adds the wire without a process; the process adds the binary. Each test lives in the cheapest layer that can show its property.

**The harness** (`crates/fitment-server/tests/support/`, included by each test file): `TestServer::in_process(config)`, `TestServer::bound(config)` and `TestServer::process(config)`; one `Client` over all three with the same calls (upload, register, socket, review, match, ingest, job wait with a deadline, document download); XH11's checks and XH13's scan on every response; and the CLI runner. `CARGO_BIN_EXE_fitment` is not available to another package's tests, so the harness builds the path from `CARGO_TARGET_DIR` and the profile, and `server-corpus` builds both binaries first. Waits use hook barriers and a deadline with a message, never a bare sleep; no test is retried.

## 10. Tests

Test ids are VT; each names the rules it shows. New test files are under `crates/fitment-server/tests/` unless named otherwise; the shared harness is section 9.4's. Every test that reads a document goes through XH11's checks and XH13's scan.

### 10.1 The exit and parity

- **VT1, the corpus through the API** (XH1 to XH7, XH9 to XH11; the exit). `exit.rs::the_corpus_passes_through_the_api`: XH3 for every socket key and policy, XH5's truth on every case, XH6's report written; `exit.rs::the_socket_gate_refuses_through_the_api` (XH1.2: each edit refused with its code before the unreadable candidate placed beside it is read); `exit.rs::review_waives_only_what_it_removes_through_the_api` (XH1.3); `exit.rs::every_original_through_the_api` (XH1.4); `exit.rs::the_exit_is_the_same_twice` (XH9).
- **VT2, the catalog through the API** (XH8). `exit.rs::the_corpus_through_the_apis_catalog_is_the_cli_run`, under three policies, with `verify --deep` at the end; `exit.rs::hostile_parts_through_the_apis_catalog` (CT10's four `h_*` parts, each run within M4's 30 s bound and the job deadline, equal to the CLI's).
- **VT3, the report equals the library's** (XH6). CI's `server-corpus` job: `diff target/fitment-corpus/report.json target/fitment-corpus/api-report.json`, with `report.json` from the `corpus` job's artifact.
- **VT4, the two views of a result agree** (SC2). `matching.rs::a_documents_outcome_is_the_librarys`: for every corpus run, `Outcome::of_result` on the library's result equals `Outcome::find` on the binary's document; `matching.rs::the_binary_meets_the_construction` runs the strict gate on the binary's documents (the binary path's KH2, today only its verdict kinds).
- **VT5, moving the scorer changes nothing** (SC1, SC3, SC5). PR 1's evidence: `report.json` and `false-negatives.json` byte-identical before and after the move (the review cases' new rows listed apart), and `cargo package --list` of each published crate unchanged. Permanently, `matching.rs::the_binary_false_negatives_are_the_baseline`.
- **VT6, the API's own documents** (DC4, DC9 to DC11, ER1, ER7). Goldens at `tests/golden/api/<operation>/<scenario>.{request,response}`: method, path and body file; status, the contract's headers (DC4, `Location`, `Retry-After`, `Preference-Applied`) and body. Every job document (each state of each kind), every error document (one per `ErrorCode::ALL` entry, each produced by a real request, or listed as job-only or internal-only with the reason no request reaches it), the listings, `fitment.info`, `fitment.run`, `fitment.token` without its secret, and the served `openapi.json`. Clock and ids fixed (OP11), so the bodies are exact. Core documents are not duplicated: the API's documents for the committed candidates are compared with `crates/fitment/tests/golden/{socket,match}`. Blessing as `golden.rs` does: `FITMENT_BLESS=1`, never under `CI`, a golden rewritten only when a version it records rose (the envelope formats' `format_version`, the API's minor).
- **VT7, the parity matrix** (DC1, DC3, DC7, JB4, JB10, CU1, CU2). `parity.rs`, one test per row, each comparing documents byte for byte and the job's state and exit code with the CLI's (XH7), and its notes with the CLI's stderr, on the committed unit assemblies and candidates (every OS, debug). The server's tightenings (SN3) have no row (DC3):

  | CLI | API | Compared |
  | --- | --- | --- |
  | `fitment features FILE` | `features` job | `features.v1` bytes; a refused file 3 ↔ `refused` |
  | `fitment parts ASM` | `assembly_read` job, `…/parts` | `fitment.parts` bytes |
  | `fitment socket ASM --part P [--occurrence O]` | `socket_extract` job | `fitment.socket` bytes; refusals' bytes and codes; a selection error 2 ↔ `refused`, exit 2 |
  | `fitment review S --remove … --note …`, `--keep` | review | `fitment.socket` bytes; an unknown requirement 2 ↔ `422`; a fresh revision only (a stale one is a tightening, SN3) |
  | `fitment socket … -o OLD` (MF11) | re-extract onto an older stored revision; `carry_from` an upload of an older version | decisions carried, `stale_reviews` bytes; the upload read leniently, within the socket cap (UP6) |
  | `fitment match S FILES [--clearance] [--tolerance-mm] [--tolerance-deg] [--require-reviewed] [--require-fit]` | match job over uploads | `fitment.match` bytes, state, exit code, notes |
  | `fitment match S FILES --original ASM` | match job with `original: true` | bytes, state; exit 1 ↔ `failed` with the document kept |
  | `fitment match S --catalog DIR [--id] [--exhaustive]` | catalog match jobs | bytes, state; stale, empty, blocked 3 ↔ `refused` |
  | `fitment catalog ingest`, `replace`, `reindex`, `prune --revisions`, `gc`, each `--format json` | catalog jobs | `fitment.catalog` or `fitment.gc` bytes, state, exit code; an ingest's change lines and entries with every generation masked (DC3) |
  | `fitment catalog delete ID… --format json` | `DELETE`, `POST /v1/catalog/delete` | `fitment.catalog` bytes; unknown ids 2 ↔ `404`, nothing deleted |
  | `fitment catalog list`, `get --format json\|features\|step` | catalog reads | bytes; `step` re-hashed (ST11), damaged ↔ `500 blob_damaged` |
  | `fitment catalog verify [--deep] --format json` | verify job | `fitment.verify` bytes; problems 3 ↔ `refused` |
- **VT8, the same bytes on every OS** (DC1, DC2, DC8). `tools/api-outputs.sh`: the in-process server on the committed set (the s01 to s03 sockets, their reviews, match jobs over the 13 committed candidates, a catalog of them with a full and an exhaustive run, VT6's job and error documents, `openapi.json`) writes `target/api-outputs/`. The `test` job uploads it on Linux, macOS and Windows; the `determinism` job diffs the three, as it does `catalog-outputs`.
- **VT31, `Match` only with a verifier that verified** (SN11; D1). With the test-support fake verifier answering `verified`, a match job on s01's good bracket ends `match`, exit 0; with `NoFitVerifier`, no corpus run through the API is a `Match` (the exit report's `match` column is 0, as the library's). A build without `test-support` has no fake: `fitment-server --help` and the OpenAPI document name no way to choose one.
- **VT32, versions and staleness through the API** (CX5, RA3, UP6; M6-28). A catalog made stale through test-support (version columns rewritten, as M5's `reindex.rs` does) refuses a full run (`refused`, `catalog_stale`, the counts) until a reindex job ends; the run then equals a fresh catalog's. With `reindex_on_start`, a restart on a stale catalog queues the reindex as the `system` user and `fitment.info` says so. A stored socket revision of lower `SOCKET_RULES_VERSION` is refused for matching and for review (`422 socket_stale`; the review's refusal is a tightening, SN3), and re-extracting onto it carries its decisions as MF11 does. Checked again at dispatch (JB2): a match queued before a restart into a build of higher `SOCKET_RULES_VERSION` (test-support) ends `refused`, `socket_stale`, exit 3, with no candidate read, and one queued on an unreviewed socket before the `require_reviewed` floor was set ends `refused`, `socket_unreviewed`. An assembly whose parts row is of another build answers `409 derived_stale` until registered again.

### 10.2 Faults (XH12 in every one)

- **VT9, a job panics** (JS6, SN7). A test-support hook panics inside a job at each named point (after the socket is read, after k candidates judged, before the result is stored). The job ends `failed` with `internal`, no document; the server keeps serving; a job queued behind it completes and equals the CLI's; the panic text is in the log only under the job's id, never in a response (XH13).
- **VT10, a restart with jobs in flight** (JB6, JS11, JS13, SD1 to SD5). For each hook point in a job's life (accepted and committed; running; document stored but state not updated; state updated), for extraction, import and assembly reading just before and just after the transaction that commits the revision or parts row with the job's end (JS13), and in an ingest batch (each of the catalog's `COMMIT` points), the process is aborted there, restarted on the same directories, and the job awaited: XH12 holds; a pure job is re-run once and then `interrupted`; a job whose result had committed ends with it and is not run again, so a line never gains two revisions from one job and no job reports `unchanged` for its own revision; a `succeeded` document equals the CLI's; a write job ends `interrupted` with no document, its details giving `generation_at_start` and `generation_at_recovery`, and its idempotency key released (JB9); an interrupted ingest submitted again converges on the catalog an uninterrupted ingest gives (`list`'s entries equal, generations masked: DC3); an interrupted replace's `job_interrupted` is not retryable. On Unix by `abort()` at the hook; on Windows by `TerminateProcess` at the same hook. A graceful stop (SIGTERM on Unix, stdin's end of file on Windows) with jobs in flight: they finish or are recorded per JB6, a re-queued pure job keeping its `attempt`, a write job its committed changes' document; streams get `shutdown`, and the process exits 0 within the grace. Weekly, 100 rounds of a kill at a pseudo-random time with XH12 after each (`soak.rs`, Unix).
- **VT11, the disk fills** (UP2, UP4, OB2, OB5, CA7; M6-44). Through the `Fs` seam (`CountingFs::failing_at`, over CA7's streaming calls too), `ENOSPC` and `EIO` at each file call of an upload, an ingest batch, a socket revision's write and a document's write: nothing half-stored is ever visible; an upload is refused with `storage_full`; a job fails, never succeeds with a short document; a later sweep clears orphans; a rerun succeeds. Weekly on Linux, a real 32 MiB tmpfs as the state directory (`sudo mount -t tmpfs`), filled by uploads until refused: the server stays up and answers health, and after space is freed an ingest and a match succeed and `verify` is clean.
- **VT12, a busy catalog** (CX3, CX4; M6-50). A test-support connection holds SQLite's write lock (`BEGIN IMMEDIATE`) while the server runs with short busy timeouts (OP3): an ingest job fails `catalog_busy` after its timeout and changes nothing; a synchronous delete answers `503 catalog_busy`; match jobs, readers under WAL, run and equal the CLI's; once the lock is released the ingest and the delete succeed. The same with a `fitment catalog ingest` process as the holder, which shows the CLI and the server sharing one catalog safely. A delete during a running ingest waits for one batch, not the job.
- **VT13, a client goes away** (UP2, JS10). Over the bound socket: a disconnect mid-upload leaves no object and no temporary file; a disconnect mid-download of a 3 MB match document does not cancel or alter the job; a client that never reads its response does not hold a blocking thread past the write timeout; a dropped event stream affects nothing.
- **VT44, a job that did not succeed states no verdict** (XH12, JB3, SN4, DC10, DC12; M6-59). `fault.rs::no_job_that_did_not_succeed_states_a_verdict`: a match driven to every end but success (refused at the request and at dispatch; `failed` by a damaged row, a panicking judgment, an original failing its socket and a deadline; `cancelled`; `interrupted`): `outcome` is null in `fitment.job`, `fitment.run`, the `end` event, `GET /v1/jobs` and `GET /v1/matches`; a failed run's kept document is served with `Fitment-Exit-Code: 1`, its `fitment.run` has `complete: false`, `summary: null` and `problems.incomplete_summary` equal to the document's counts, and every candidates page states `complete: false` and `exit_code: 1`. The control: the same reads of a succeeded run find `outcome` set and `incomplete_summary` null.

### 10.3 Concurrency

- **VT14, many clients** (JS1 to JS5). 32 clients over the bound socket, each running every file-candidate socket of the corpus under both policies, interleaved: every document equals the sequential one; the lane, permit and owner caps hold (`fitment_jobs_running`, `fitment_cpu_permits_free`); a refusal for load is only the declared one (`429` or `503` with its code), which the harness retries with backoff and counts. Every PR on the committed set; on the corpus in `server-corpus`.
- **VT15, writes during matches** (CX2, CX5, M6-13; CT14 through the API). While ingest, replace and delete run over a pool of 40 corpus files and a reindex runs once, match jobs run in a loop. Each succeeded catalog match equals `fitment match S <the files live at its generation>`, the files replayed from the catalog's `op` rows by the generation in its `fitment.run`; a match refused as stale is refused for that reason only; no document mixes two generations. A replace, then a match, shows the new revision (nothing cached, GT9).
- **VT16, two writers on one directory** (SD6, CX3). A second `fitment-server` on the same state directory refuses to start (exit 2) while the first holds the lease; a `fitment catalog ingest` process beside a running server leaves IN1 to IN9 holding and both sides' results consistent.

### 10.4 Hostile input over HTTP

- **VT17, sizes** (UP2, UP4, UP6, HX2, AU1, RA8, VN3). For every size limit, one request at the limit (accepted), one just past it (refused with its code), and one streamed 100 times past it, refused after reading at most the limit and one chunk (a counting body in process) without the process's allocations growing by more than a stated bound (a counting global allocator in its own test binary). A request with any `Content-Encoding` is `415` (DC6). The STEP cap (128 MiB) holds for uploads as for `ingest`.
- **VT18, slow clients** (HX1, HX3). Headers at one byte per 500 ms are cut at the header timeout; a body trickled at one byte a second is cut at the chunk timeout; an upload trickled just below `upload_min_kib_s` is cut `408 upload_too_slow` once its first 30 s are past, its temporary file removed, and one just above it completes; 256 such connections at once do not delay a normal client's match job beyond its usual time plus a stated slack.
- **VT19, malformed bodies** (VN6, ER7). A sweep in process, each a `4xx` with a stable code and never a `500`: an empty body, a wrong or missing `Content-Type`, invalid UTF-8, a byte-order mark, trailing garbage, a truncated document, nesting 10⁵ deep (serde_json's recursion limit), numbers beyond `f64`, `NaN` and `-0.0` where a tolerance is asked, duplicate keys (refused: a duplicated `status` in a review decision is read neither way), unknown fields, a `fitment.socket` upload of another format version, a socket document with its digest broken (VD9's refusal, exit 3's code).
- **VT20, paths, ids and names** (RA2, RA8, HX5, PH6). Ids and references as M5's CT15 and ID9 (`'`, `%`, `_`, `;`, case folding, `BRKT@r01`, a 12-hex prefix, an 8-hex prefix) give exactly their entry or the not-found code; path segments `..`, `%2e%2e`, `%2f`, `%00`, overlong UTF-8, a 64 KiB segment, doubled and trailing slashes; labels and file names `../../etc/passwd`, `C:\x.stp`, `CON`, `a.stp:stream`, a 255-byte name, ESC and U+202E: never a path on the server, stored and returned exactly in JSON, escaped in every table. The state and catalog directories are unchanged outside their own layouts afterwards. An `X-Request-Id` from a trusted proxy of 65 characters, or with a character outside `[A-Za-z0-9._-]`, is replaced by a generated one; one of 64 allowed characters is kept (HX7).
- **VT21, many jobs** (JS4, JS5, JS9, JB12). Jobs past a lane's queue cap are `503 queue_full`, past the owner's cap `429 too_many_jobs`, never dropped; job lists are paginated and bounded in bytes; 10⁴ finished jobs listed, retained, pinned and expired as JB12 says (the test-support clock), with memory bounded.
- **VT22, hostile STEP through the API** (JB7, SN7). The corpus's `h_*` parts and `matching.rs`'s hostile candidates as uploaded candidates: each judged within M4's bounds and the job deadline, each document equal to the CLI's; a job past a test-sized deadline ends `failed`, `deadline_exceeded`, with no document, never `reject` (XH12).
- **VT45, catalog-scale work is bounded** (AU5, JS1, JS3, JS12, JB7, UP4, UP6, HX8, CX6; M6-60). In `hostile.rs` and `concurrency.rs`: an exhaustive match or a `verify` as a reader is `403 forbidden`, as a writer runs; with `max_catalog_candidates` below the catalog's live entries an exhaustive run ends `refused`, `too_large`, exit 3, before judging anything (a test-support count of judgments is zero), while a full run on the same catalog runs; a catalog match takes the memory permits its selection implies (a test-support readout); a user at the quota, a retained job's document counted, cannot submit a job (`507 quota_exceeded`), and a running job whose document crosses the quota stores it; with `parse_concurrency = 4`, five slow imports parse four at a time and one held past the handler timeout answers `503 request_timeout`, `retryable: false`; a socket upload of 16 MiB + 1 byte is `413`; a third concurrent `limit=all` listing is `429 too_many_streams`, and a `limit=all` whose client never reads its body holds no snapshot (the catalog's checkpoint completes meanwhile); a barrier whose readers do not finish releases dispatch within `barrier_max_seconds` (the test-support clock), a waiting `VACUUM` then failing `catalog_busy`; `verify --deep` runs beside a running ingest without a barrier, and past a test-sized deadline ends `failed`, `deadline_exceeded`.

### 10.5 Authentication and permissions

- **VT23, the matrix** (AU5, AU8, XH14). For every operation of `OPERATIONS`, and every field that raises its role (`exhaustive`), and each principal (none, a malformed token, an expired token, a revoked token, a disabled user's token, reader, writer, admin, a writer's token of role reader, and an owner and a non-owner of each owned resource): the status and code the operation's role implies: `401 unauthenticated` without a valid credential, `403 forbidden` below the role, neither at or above it, `404 not_found` for a non-owner. Health, readiness, OpenAPI and login need none. An admin's `?owner=` is honoured on `GET` and `HEAD` and refused on every other method (`400 invalid_parameter`).
- **VT24, auth's own properties** (AU1 to AU4, AU6, AU7, AU9, AU10). Login with an unknown user runs the password hash as a known user's does (a test-support counter of hash calls, not a timing); tokens appear in `server.db` only as SHA-256 (a scan of every table after the suite); the bootstrap token and every token are absent from logs (XH13); a revoked token and a disabled user are refused at the next request; a token's role is capped by its user's, and demoting the user demotes it; an authenticated read writes nothing to `server.db` (the audit's "one or two writes per request"); the rate limits answer `429` at their bounds; a token in a query string is refused. Minting (AU10, AU4): a token asking a role above the presenting token's effective role, or an expiry past the presenting token's, is `400` and nothing is minted; a reader token minted from a writer's login token cannot mint a writer token; revoking a token revokes every token minted from it, two generations down, and no other; a password change revokes every other token of the user and keeps the presenting one; an offline `user reset-password` revokes all; the 21st live token, by login or minting, is `409 too_many_tokens`. Per user (AU7, UP4, JS10): twenty tokens of one user share one request bucket, two upload slots and eight streams. Login buckets: an unknown username meets the same answers and limits as a known one; ten failures from one address leave the same username free from another; 10⁵ distinct usernames from many trusted-proxy addresses keep at most `login_buckets` buckets (a test-support count). AU9's admin commands refuse to migrate an older `server.db` while a server holds the lease.
- **VT25, isolation** (AU8, UP3, RA4). A non-owner cannot read, list, cancel, download or delete another's job, match, upload, assembly or socket line, and gets the same `404` as for an id that does not exist; two users with the same assembly hold two lines of one `sk_` id; `HEAD` of a hash another user uploaded is `404`, and uploading those bytes answers `201` like new bytes; an admin reads them with `?owner=` and cannot name them in a job.
- **VT48, both branches of an upload do the same work** (UP8, UP3, OB2; M6-15). `isolation.rs::both_upload_branches_do_the_same_work`: one user uploads bytes another user holds, and bytes nobody holds, each under a `CountingFs`; the two call lists are equal (**Review (PR 4):** both rename; on Windows one read-only flag cleared), neither reads the existing object, and the two answers have the same status and headers, differing only in the SHA-256's own fields; a `DELETE` of bytes another user holds and of bytes nobody else holds makes no file call either way and answers the same; the two branches' median times are within a generous bound, since call shapes cannot see a disk's own time. The control: a store that re-hashes an existing object on the upload path, or removes the temporary file in one branch, fails the test.

### 10.6 Fuzzing, scale, the container, startup

- **VT26, fuzzing** (M6-42). `fuzz/fuzz_targets/route.rs`: arbitrary bytes split into method, path, query, headers and body, through `Server::router().oneshot` on a current-thread runtime over a small catalog: no panic; no `5xx` but the declared load refusals; every `4xx` body a valid error document. `fuzz/fuzz_targets/body.rs`: each request type's decoding and validation; an accepted value serializes and decodes to itself. Seeded with VT6's golden requests. `fuzz.yml`, weekly, 10 minutes a target, nightly toolchain; a crash is filed with its input, which becomes a VT19 or VT20 case.
- **VT27, scale** (M6-45). `tests/scale.rs` with `FITMENT_SCALE_DIR` (CT11's `tools/scale-step.py` copies): the 10⁴ scaled catalog ingested through the API, in at most twice the CLI's ingest time for the same files in the same job; every corpus socket's full run equal by digest to the CLI's on the same catalog, its latency at most twice the CLI's plus one second; the seats-only socket's largest document downloaded, with the server's peak memory reported. Weekly in `scale.yml`; 10⁵ by hand, as M5 did.
- **VT28, load** (M6-45). 16 clients over the 10⁴ catalog, every corpus socket in a loop for 2 minutes: throughput and p50 and p99 latency reported; the gate an order of magnitude (no request over 10 times the single-client time), as CT11's ceiling is.
- **VT29, the container** (OP8, OP11; M6-43). CI's `container` job: `docker build` of `deploy/Containerfile` with `--locked` and no `test-support`; the image holds no Python, uv or Open CASCADE (a scan of `docker export`'s file list for `python`, `uv`, `libTK`, `OCCT` and `.so` files outside the base's, which works on a distroless image without a shell); its user is not root; `fitment-server --version` and `fitment --version` in it print the workspace version; a smoke run with two volumes: health, the bootstrap, an upload of s01 and the committed bracket, a socket job and a match job whose document equals `crates/fitment/tests/golden/match/`'s; the image's size reported, with a ceiling of twice the first measured.
- **VT33, startup and shutdown** (SD3, SD6, SV5, SV6, OP4). A catalog directory that is not a catalog, a catalog of a newer schema, a `server.db` of a newer schema or another application, nested state and catalog directories, an unreadable configuration, an unknown key, a port in use: each refused at startup with a message and a non-zero exit, before listening. At run time, a catalog that becomes unavailable answers `catalog_unavailable` with the CLI's exit code for its reason: 2 for `not_a_catalog`, `linked` and `not_unicode`, 3 for `newer_schema`, 1 for `unreadable` (ER2). Under `--listen 127.0.0.1:0` stdout carries one line, the bound address (M6-54).
- **VT34, no leaks** (XH13, ER3). `leaks.rs` runs the scan's own controls: a planted path, token and panic message in a test-support response are each found; a planted token in a log line is found, and a path relative to a configured root in a log line is not flagged (OP1).
- **VT35, the CLI stays synchronous** (SV1, SV9). `tools/check-core.sh`: `cargo tree -p fitment -e normal --all-features` names no tokio, hyper, axum or tower; `deny/sync.toml` passes with `fitment-catalog` and `fitment` as roots; the server's default normal graph holds no schemars, `h2` or TLS crate, and serde_json's three features exactly.

### 10.7 The schemas, the crate changes, and the API's own rules

- **VT36, schemas** (VN7, CO4, CA6; M6-6). `openapi.rs::the_openapi_document_is_the_committed_one` (behind the `schema` feature, run by CI explicitly); `openapi.rs::every_document_validates_against_its_schema`: every corpus, golden and fixture `features.v1`, `fitment.parts`, `fitment.socket`, `fitment.match`, `fitment.catalog`, `fitment.verify` and `fitment.gc`, and every VT6 envelope, validated against the schema the OpenAPI document gives its format; a document edited to add an unknown field or drop a required one fails (the validator's control).
- **VT37, core's changes** (CO1 to CO4). `sha256.rs`: `Sha256` equals `sha256` on the standard vectors and the unit set at every split point of messages of 0 to 200 bytes; `matching.rs::exit_codes_are_the_clis`: `Outcome::exit_code`, `is_fitment_bug` and `original_fails` give the CLI's codes on every corpus run; `candidates.rs::a_component_named_twice_is_one_entry` (two labels, one entry, the document the same whatever their order); every match golden unchanged; `cargo build -p fitment-core --features schema` and `tools/core-deps.py` with the feature.
- **VT38, the catalog's changes** (CA1 to CA8). `ingest.rs::inputs_from_bytes_are_ingest_from_paths`: `ingest_inputs` over the bytes and names of a directory's files gives the ids, order and `fitment.catalog` document `ingest` gives over the paths; per-file ids order with the exact stems; no document or error line holds a path that was not an `input`. `observer.rs::an_observer_changes_no_document`: every corpus document identical under `NoObserver` and a recording observer; `observer.rs::each_stop_point_stops` at every point CA2 names, a cancelled match returning `NotRun::Cancelled` and a cancelled ingest its committed batches. `checkpoint.rs` for CA3 with a reader pinning the WAL; `verify.rs::no_problem_names_an_absolute_path` and `ingest.rs::a_failed_batch_line_holds_only_the_error_kind`, with a reindex line likewise (CA4); the `fitment.verify` and `fitment.gc` goldens (CA5); `fs.rs::streams_go_through_the_seam`, each streaming call recorded by `CountingFs` and failed by `failing_at` (CA7); `meta.rs::the_instance_is_minted_once`, a catalog without one given one, a second call returning it unchanged, the generation unchanged and `verify` clean (CA8).
- **VT39, the CLI's changes** (CU1 to CU3). `catalog_cli.rs`: `delete` and `prune --revisions` with `--format json` print `fitment.catalog` with `changes` (goldens); `verify` and `gc` print their documents, exit codes unchanged; every existing golden, CLI test and `report.json` byte-identical after CU3, but for CA4's changed lines, each recorded with its old and new text in the goldens and the CHANGELOG.
- **VT40, references and revisions over HTTP** (RA2, RA4, RA5, RA7, ID9). M5's `strict_revision_references` through the API (`BRKT@R1`, `@r01`, `@r0`, `@1`, `@r`, `@r1x` are `400`; after a replace, writes naming `@r1` are `409` and change nothing); a line's id is the same on two servers given the same data, and extracting twice gives one line; two reviews racing on `@r1`: one makes `@r2`, the other is `409 revision_not_current`; a bare id on review is `428`; a label `catalog:x` is `400 label_reserved`.
- **VT41, stopping jobs** (JB5, JB7, JS8, SN7). A cancel injected at every stop point of JS8 for every kind: a match leaves no document and no run record; an ingest, reindex or gc leaves exactly its committed work, listed in its document; a replace or prune answers `409 job_finished` once running; a queued job is `cancelled` at once. A deadline at each point ends the job `failed`, `deadline_exceeded`, with no document; `verify`, plain and `deep`, now among them (JB7).
- **VT42, pages, caching, waiting and retries** (VN3 to VN5, JB9, JB11, JS11). Keyset pages are total and stable (every job, revision and catalog entry listed once across pages); a page of another generation under `If-Match` is `412`; `If-None-Match` answers `304`; `Prefer: wait=60` waits past the 30 s handler timeout and answers `200` when the job ends, `202` when it does not; a crash between commit and response, then a retry with the key, creates one job. Idempotency (JB9): the same key and body as sent returns the same job, `202` while it runs and `200` after; the body with its keys reordered or reformatted is the same request, and a bare `sk_X` sent again after the line gained a revision is too; another body is `422`; a key whose job ended `interrupted`, `cancelled`, or `failed` with a retryable code creates a new job, and one whose job `succeeded` does not; `request_timeout` on a review is `retryable: false`, on a `GET` `true`. ETags (VN5): two catalogs at the same generation give different ETags (CA8); `features` and `…/parts` are `no-cache`; a job's ETag is unchanged by its progress events and changes with a persisted event or its state.
- **VT43, progress events** (JB8, JS10; M6-51). `?after=N` replays every persisted event after N and then live ones, without loss across the switch; persisted events are numbered 1, 2, 3 … without gaps and transient events carry no `seq`; no event of any match, in any state, carries a verdict, status or failed requirement (a scan of every event the suite reads); a slow reader gets `lagged` and continues; a heartbeat comes every 15 s on an idle stream (the test-support clock).
- **VT46, the candidates view is sliced** (DC12, SN5; M6-8). `api.rs::candidate_pages_are_the_documents_bytes`: for every corpus match document, the pages' entries, concatenated, are the document's `candidates` elements byte for byte and in order, each page stating the digest, `view`, `complete` and `exit_code`; a test-support count of document parses stays zero while the 3 MB catalog document is paged; a stored document with one byte flipped inside one candidate makes that candidate's page `500 blob_damaged` while the other pages still serve.
- **VT47, imports are verified or marked** (RA10, RA5, UP6; M6-62). `api.rs`: an import of the CLI's socket file with its assembly registered makes a `verified: true` revision; the same file with one requirement's value edited and its digest recomputed ends `refused`, `socket_not_reproduced`, exit 3, with no revision written; without the assembly the revision is `verified: false`, the revision listing and `fitment.run` say so, the match carries the `unverified_socket` note, and its document still equals the CLI's for the same file; registering the assembly and importing the same file again makes a new, verified revision of the same bytes.
- **VT49, holds and revivals** (OB4, JS11, UP3, UP5, RA4, RA6, SN6; M6-64). In `fault.rs` and `api.rs`: a job's `job_input` and `job_cites` rows exist at a hook after its creating commit and before dispatch; an upload a queued job names survives its lifetime, and its `DELETE` is `409 upload_in_use`; after a file match ends and its uploads expire (the test-support clock), the retained record still holds every candidate's object, and the same request submitted again, naming the same SHA-256s, gives the same document bytes; once the record expires, the sweep unlinks them. A deleted socket line extracted again is live under the same id, its next revision numbered after the deleted row's last, and a retained run's `sk_X@rN` still names the old revision.
- **VT50, components and originals through core** (CO5, SN1, SN9, DC3; M6-5). `candidates.rs::judge_component_is_judge_part_on_the_read_component`: for every corpus component candidate, `judge_component` equals `judge_part` on the part the reading selects, and a `source` naming another definition is an internal refusal; `api.rs::a_component_of_a_refused_assembly_is_refused` (`refused`, exit 3, `component_refused`, no document); `api.rs::an_unreadable_original_is_run_incomplete` (an `original: true` whose stored assembly the seam makes unreadable: `failed`, `run_incomplete`, exit 1, and VT44's checks).

### 10.8 Coverage

- **VT30, every M6 rule has a test or a reason** (section 11.1). The coverage test reads this document; every M6 rule id is in `RULES`, deferred to its PR until built; at the exit none is deferred but for a reason outside M6. Its new checks each have a control: a duplicated id fails; a test cited as `#[tokio::test]`, `#[tokio::test(...)]` or an `async fn` under either passes, and an `async fn` without a test attribute fails (M6-41).

## 11. The registry, the coverage gate and CI

### 11.1 The rules registry and the coverage gate

M6's ids join `RULES` in `tools/gen-corpus.py` the way M4's and M5's did (C 7; M6-41):

- **In PR 1,** every rule id of this document enters `RULES` as `{"deferred": "M6 PR n"}`, from an `M6_COUNTS` table (each prefix's count; every prefix is numbered from 1 without gaps), an `M6_PR` table and an `M6_OTHER_PR` table for the exceptions, like `M5_COUNTS`, `M5_PR` and `M5_OTHER_PR` (`tools/gen-corpus.py`, around line 821). Each PR that builds a rule replaces its entry with the tests that show it, named `file::function`, in the same PR.

  | Prefix | Count | `M6_PR` | `M6_OTHER_PR` |
  | --- | --- | --- | --- |
  | SC | 9 | 1 | — |
  | SV | 9 | 2 | — |
  | ER | 7 | 2 | — |
  | VN | 8 | 2 | VN3: 4, VN4: 7, VN5: 5 |
  | HX | 8 | 2 | HX8: 7 |
  | OP | 11 | 2 | OP6: 5, OP10: 9 |
  | CO | 5 | 6 | CO1: 4, CO4: 2 |
  | CA | 8 | 7 | CA6: 2, CA7: 4 |
  | CU | 3 | 7 | — |
  | AU | 10 | 3 | AU8: 6 |
  | UP | 8 | 4 | UP5: 5, UP6: 6, UP7: 5 |
  | OB | 5 | 4 | OB1: 2, OB4: 5, OB5: 5 |
  | JB | 12 | 5 | JB2: 6, JB10: 6 |
  | JS | 13 | 5 | JS7: 6, JS8: 7, JS12: 7 |
  | SD | 6 | 5 | SD3: 2, SD6: 2 |
  | DC | 12 | 5 | DC3: 6, DC7: 6, DC9: 2, DC10: 6, DC11: 2, DC12: 6 |
  | RA | 10 | 5 | RA1: 7, RA2: 7, RA6: 6, RA7: 6, RA8: 6, RA9: 6, RA10: 6 |
  | SN | 11 | 6 | SN10: 7 |
  | CX | 8 | 7 | — |
  | XH | 14 | 6 | XH8: 7, XH12: 7, XH13: 2, XH14: 8 |

- **`design_rules` reads this document** (`docs/research/m6-api.md`) beside M2's to M5's, so a rule stated in bold and missing from `RULES` fails.
- **The deferral allowance.** The coverage test's `DEFERRED` check accepts `"M6 PR n"` deferrals while M6 is open, as it did for M4 and M5; PR 9 removes that acceptance. After it, `DEFERRED` grows only with reasons outside M6, each stated: M7 (the fit verifier in the server), M8 (the viewer's routes), a property that holds by construction, or a guard no input reaches, as M5's CC3, CL6, GT8, QR13, ST8, SZ2 and VF5 were.
- **Tests anywhere in the repository.** `is_test` already resolves `file::function` against any path, so `crates/fitment-server/tests/*.rs` and `crates/fitment-corpus` work as they are; PR 1 widens it to accept `#[tokio::test]` and `#[tokio::test(...)]` and the `async fn` that follows either, with VT30's control (M6-41).
- **No id stated twice** (new). `design_rules` is run per design document, and an id stated in bold by two documents fails. None is today (checked on M2 to M5, and every prefix here is new: section "Rule ids").
- **Lists that need a particular kind of test,** as M4's `CAN_FAIL` and `M4_UNVERIFIABLE` need a particular kind of case:
  - **`M6_LIMITS`:** every rule that bounds an input (a size, a count, a time, a depth) needs a test in `crates/fitment-server/tests/hostile.rs` or `slow.rs` (section 9.4) that exceeds it: UP2, UP4, UP6, HX1, HX2, HX3, HX8, JS5, JS10, JS12, JB7, JB11, AU1, AU7, AU10, RA8, VN3. VT17, VT19, VT20 and the limit cases of VT24, VT41, VT42 and VT45 live in `hostile.rs`, VT18 in `slow.rs`.
  - **`M6_PARITY`:** every rule that says the API does what a CLI command does (a document served, a refusal, an exit code mapped) needs a test in `parity.rs` or `exit.rs`: DC1, DC3, DC7, JB2, JB4, JB10, RA2, SN2, SN10, UP6, CU1, CU2, CA1.
  - **`M6_FAULT`:** every rule on durability, recovery or job state needs a test in `fault.rs`, `soak.rs` or `concurrency.rs`: JB5, JB6, JS8, JS11, JS13, OB2, OB4, OB5, SD1, SD2, SD4, SD5, SD6, CX3, CX4, CX6. VT41's cancellation cases run in `fault.rs` beside VT9, VT10 and VT44.
- **Cases.** PR 1's review cases cite KC13, XH1 and SC8 in their `rules`. The coverage test's `case_lists` is unchanged: a waived case shows that a review waives only what it names, not that a check can fail, so it never counts as the bad case `CAN_FAIL` asks for.
- **The gate at the exit** (PR 9): `M6_LIMITS`, `M6_PARITY` and `M6_FAULT` on and passing, no `"M6 PR"` deferral, every SC and XH rule with a test, as CT19 closed M5.

### 11.2 CI

| Job | Change | PR |
| --- | --- | --- |
| `clippy` | unchanged (`--workspace --all-targets --all-features` covers both new crates), plus `cargo clippy -p fitment-server --no-default-features` | 2 |
| `corpus` | uploads `target/fitment-corpus/report.json` as an artifact beside `corpus/out` | 1 |
| `test` (3 OSes) | runs the server's in-process, socket and process tests in debug (the exit skips there, XH10), `cargo test -p fitment-server --features schema --test openapi` explicitly (VT36); writes `api-outputs` and uploads it per OS | 2, 6 |
| `determinism` | also diffs the three `api-outputs` (VT8) | 2 |
| **`server-corpus`** (new, Ubuntu) | `needs: corpus`; downloads `corpus/out` and `report.json`; `cargo build --release -p fitment -p fitment-server --locked`; `cargo test --release -p fitment-server --test exit --locked` with `FITMENT_REQUIRE_CORPUS=1`; `diff report.json api-report.json` (VT3); uploads `api-report.json` | 6 (file half), 7 (catalog half) |
| **`container`** (new, Ubuntu) | VT29 | 2 |
| `msrv` | unchanged: `cargo check --workspace --all-features` on 1.85 now covers axum, tokio and tower-http | 2 |
| `package` | `cargo package --workspace --locked` with `fitment-corpus` and `fitment-server` unpublishable (PR 1 checks they are skipped, else `--exclude`); `check-core.sh` asserts `publish = false` and `dist = false` for both (M6-35) | 1, 2, 10 |
| `deny` | `allow-wildcard-paths`, only if PR 1 shows it is needed (SC3; review question 6); rusqlite's wrappers gain `fitment-server` (SV6); serde_json's exact list gains `alloc` with its reason (SV9); `deny/sync.toml` with `fitment-catalog` and `fitment` as roots (SV1); `check-core.sh`'s new checks (SC3, SV9, VT35) | 1, 2 |
| `scale.yml` (weekly) | a server leg over the same 10⁴ catalog (VT27, VT28), the restart soak and the tmpfs leg (VT10, VT11) | 8 |
| **`fuzz.yml`** (new, weekly) | VT26 | 8 |
| **`image.yml`** (new, manual) | builds and pushes `ghcr.io/jchultarsky/fitment-server:<version>` from a tag; run by the user (M6-43) | 10 |

## 12. PR plan

Each PR comes with its tests and two adversarial reviews, as in M4 and M5, and no PR depends on a later one. Every M6 rule id is in `RULES` from PR 1, deferred to the PR that builds it (section 11.1). A PR may land in two parts, as M5's did (an "a" for the library and a "b" for its surface); the plan names each test in the first PR that can run it. Each item leaves room for an "As built" note.

1. **The scorer, the review cases and the registry** (no server code).
   - **Contents:** `crates/fitment-corpus` (SC1 to SC9) with section 9.2's moves; `crates/fitment-core/tests/candidates.rs` including `gate.rs` and `outcome.rs` by path (SC6); `matching.rs`, `golden.rs`, `match_cli.rs`, `catalog_match_cli.rs`, `corpus.rs`, `original.rs`, `catalog_corpus.rs` and `fixtures.rs` on it; the binary path on the strict gate through `Outcome` (VT4); the generator's review cases and the `waived` truth (M6-38), both existing paths applying `socket.review` and grouping by it; every M6 id in `RULES`, deferred (section 11.1's tables); `design_rules` reading this document; `is_test` widened (M6-41); the duplicate-id check; `M6_LIMITS`, `M6_PARITY` and `M6_FAULT` declared, their rules deferred; `allow-wildcard-paths`, only if cargo-deny needs it (review question 6); `check-core.sh`'s SC3 check; the `corpus` job uploading `report.json`.
   - **Tests:** VT4, VT5, VT30's new checks with a control each (a duplicated id; an async test, and an `async fn` without a test attribute, cited in `RULES`).
   - **Done when** `report.json` and `false-negatives.json` are byte-identical to `main`'s but for the review cases' rows, which add no false positive and no false negative; no test file keeps a scorer of its own; `cargo package --list` of every published crate is unchanged; CI is green on three OSes.
   - **As built.** Done as planned. `fitment-corpus` holds `gate`, `outcome`, `checker`, `corpus` (`Corpus`, `Case`, `Truth`, `SocketKey`), `policy`, `score`, `baseline`, `report` and `parity`; `support/gate.rs` and `common/checker.rs` are gone; `matching.rs`, `corpus.rs`, `original.rs`, `shortlist.rs`, `catalog_corpus.rs` and `catalog_scale.rs` find the corpus through `Corpus`, `golden.rs`, `match_cli.rs` and `catalog_match_cli.rs` use its checker, and `corpus.rs::no_test_keeps_a_scorer_of_its_own` keeps it so (SC1). The review cases are `m_review_s01_hole_waived` and `m_review_s02_post_waived` (`waived`, class `review_waived`: s01's bracket without its hole at (30, 15) with that hole's bore removed, and s02's base with a post in its tapped hole with that bore removed; both `InterfaceMatch`, `requirements_removed`; each keeps its base's defect, which its review must remove whole), and `m_review_s01_grip_kept_hole` and `m_review_s02_grip_kept_seat` (`does_not_fit`, class `b_review`: the grip removed, still rejected for the missing hole and for the head's seat). Each reuses its base case's file, frame and placement; a review entry is a locator (KC5) with `status` and `note`, and the gate checks that each removed requirement is listed as removed and not checked, and that `requirements_removed` is flagged exactly when a review removes something (on the `b_review` cases too, and never on an unreviewed one). `report.json` gains those four rows (two classes, two policies), each with no false positive and no false negative; every other row and both totals are byte-identical (false positives 0; false negatives against physics 20 of 49 under `default`, 21 of 51 under `functional`); `false-negatives.json` gains only the four case ids, its 41 entries unchanged. The three published crates' `cargo package --list` are unchanged. Where the build departs from section 9.2:
     - **The gate's signature.** `gate::gate(case: &Value, policy: &str, sockets, outcome)` takes the manifest entry and the policy's name, since SC6 lets `gate.rs` name only `crate::outcome`, `fitment_core` and `serde_json`; `Case::gate(policy, sockets, outcome)` is the typed form. `gate::decisions` and `gate::apply_review` resolve a case's review the same way for every path.
     - **`Outcome`'s constructors.** A `CandidateResult` does not hold its policy, so `Outcome::of_result(result, &MatchPolicy)`, and `of_entry(entry, &policy)` with the document's; `find_component` finds a component's entry by (assembly SHA-256, definition).
     - **Causes.** `Run` carries `at_construction` (KH8's placement, which only the library knows) rather than a list of causes; `baseline::bless_or_compare(found, cases)` is the library's, `baseline::compare(found, cases)` every other path's, taking each false negative's cause from the baseline (SC5).
     - **The fifth `corpus()`** was in `shortlist.rs`, not `fixtures.rs`, which locates fixtures, not the corpus, and is unchanged; `catalog_scale.rs`'s own locator moved too (`Corpus::at`, `Corpus::default_dir`). `catalog_corpus.rs` also runs each reviewed socket.
     - **XH1 stays deferred to PR 6,** though the review cases cite it: the exit set holds them, but the library's and the binary's runs of them do not show XH1, which is the API's. The coverage test lets an M6 rule waiting for its PR be cited by supporting cases (a test still ends the wait). `M6_LIMITS`, `M6_PARITY` and `M6_FAULT` are declared and already checked: a listed rule that is not deferred needs a test in its list's files, cases alone never satisfy it, and each listed id is checked to be stated here.
     - **`allow-wildcard-paths = true`** is needed: cargo-deny 0.20.2 reports the versionless path dev-dependency as a wildcard. With the key it still refuses one in a published crate's normal dependencies (tried), so it relaxes nothing else (review question 6). `tools/check-core.sh` checks `publish = false` and that no published crate's normal graph holds the scorer.
     - **Packaging.** `cargo package --workspace` packages and verifies `fitment-corpus` too (`publish = false` does not skip it), so CI's `package` job excludes it; locally, with it included, every crate still verifies.
     - **The oracle.** `tools/verify-occt.py`'s `match` mode groups cases by their review as well and judges a reviewed group against its socket reviewed by `fitment review`; KO4, KO5 and the mutations run on it, and only KO3's physics is skipped for a waived pass. KO1 skips `waived` cases (the user's acceptance, never physics' answer) and judges the `b_review` ones. `manifest` and `match` modes both report 0 disagreements on the new corpus.
     - **Changed in review.** `waived` no longer exempts a pass on the manifest's word: the gate makes it a harness error without a removal in review and `requirements_removed` expected, and the library runs the candidate against the socket as extracted, unreviewed, at the construction's placement, where it must fail something and only what its review removes (`gate::waived`), its defect failing there as KH8 asks; `scorer.rs::a_mislabelled_waived_case_is_caught` is the control (no review, an unrelated removal, no flag expected). A review of a refused socket document, or a status `fitment review` does not know, is a harness error. `is_test` walks up from the function over attribute and doc-comment lines only, so a test above a helper no longer counts for the helper (`a_test_is_found_as_written`'s new controls). An M6 deferral must name a later PR than this one and at most PR 9 (`M6_PR_NOW` is 1), with `m6_deferrals_and_lists_hold` as the control. `tools/check-core.sh` packages the workspace without verifying and checks that no published crate's packaged `Cargo.toml` names `fitment-corpus` (SC3). `baseline::compare` narrows the baseline by (case, policy) to the pairs a path ran, and `Score::false_negatives` makes a false negative the baseline does not list a harness error rather than give it a cause (`a_path_is_compared_on_what_it_ran`).
2. **The server's skeleton, its harness and the OpenAPI document.**
   - **Contents:** the `fitment-server` crate, library and binary (SV1 to SV9): configuration (OP2 to OP4), `Server::build`, `router` and `serve`, the announced address (M6-54), `/v1/health`, `/v1/ready`, `/v1/info`, `/v1/openapi.json` (OP5, OP7, DC11), tracing (OP1); the hardening layers (HX1 to HX7); `ErrorCode` and the error document (ER1 to ER7, DC9); the `OPERATIONS` table; `server.db`'s creation, the `system` user and the lease (OB1, SD3, SD6); the `schema` features with CO4 and CA6 and the OpenAPI test (VN7); `deny.toml`'s and `deny/sync.toml`'s changes (SV1, SV6, SV9); the `test-support` seams (OP11). The harness: `tests/support/` (three transports, the client, XH11's checks, XH13's scan with VT34's controls), VT6's golden machinery, `deploy/Containerfile` with VT29 and the `container` job, `api-outputs` in the `test` and `determinism` jobs.
   - **Tests:** VT6 (OpenAPI, health, readiness, info, every error code that exists yet), VT8, VT29, VT33, VT34, VT35, VT36's OpenAPI half and its validation of the result documents; MSRV.
   - **Done when** the binary starts on `127.0.0.1:0` and announces it; health, info, OpenAPI and error documents are the same bytes on three OSes; the image builds, holds no Python, uv or Open CASCADE and passes its smoke run (health only until PR 6); `fitment`'s graph has no async crate and the server's runtime graph none of SV9's exclusions.
   - **As built.** Done as planned, every rule section 11.1 defers to PR 2 built and cited in `RULES` (`M6_PR_NOW` is 2, so none may wait for it): SV1 to SV9, ER1 to ER7, VN1, VN2, VN6 to VN8, HX1 to HX7, OP1 to OP5, OP7 to OP9, OP11, CO4, CA6, OB1, SD3, SD6, DC9, DC11 and XH13. VN1, VN2, VN8, OP8, OP9 and XH13 are not named in the contents above, but the registry defers them here; HX1, HX2 and HX3 have their tests in `hostile.rs` and `slow.rs`, SD6 in `concurrency.rs` (`M6_LIMITS`, `M6_FAULT`). `crates/fitment-server` holds `lib.rs` (`Server::build`, `router`, `serve`, `check`), `main.rs` (`serve`, `check-config`, `healthcheck`), `config.rs`, `net.rs`, `http/{mod,json,error,operations}.rs` and `http/routes/server.rs`, `catalog.rs`, `matching.rs` (the fit verifier only, until PR 6), `info.rs`, `ids.rs`, `state/{mod,db}.rs` with `state/schema.sql` (all of section 6.2's schema 1), `openapi.rs` (`schema` feature) and `hooks.rs` (`test-support`). Dependencies as section 4.13 names them, resolved to tokio 1.53.3, axum 0.8.9, hyper 1.12.0, hyper-util 0.1.21, tower 0.5.3, tower-http 0.7.1, http-body-util 0.1.5, getrandom 0.4.3, tracing 0.1.44, tracing-subscriber 0.3.23, toml 1.1.8, anyhow 1, schemars 1.2.3; tower-http with `trace`, `sensitive-headers`, `catch-panic` and `cors` only: its `limit`, `timeout`, `request-id` and `set-header` are not used, since the body check, the 503 timeout, HX7's id and HX6's headers are the server's own, each answering with or around the error document; futures-util and tokio-util are not needed yet. `ErrorCode::ALL` is section 7.12 entire (82 codes; `errors.rs::the_registry_is_section_7_12` reads the table); the goldens produce the 8 a request reaches today (`not_found`, `method_not_allowed`, `not_acceptable`, `body_too_large`, `request_timeout`, `catalog_unavailable`, `shutting_down`, `internal`), the job-only ones are known by their HTTP "—", and the rest are listed with the PR that adds their route, which `golden.rs` holds to a later PR. The image is 15,041,033 bytes (arm64), and its smoke run passed locally with Docker 29.6. Question 9 is decided: the small validator in `fitment-corpus` (`schema.rs`, serde_json only, with controls), not `jsonschema` (0.33 without default features: 78 crates, MSRV 1.71.1; licences not checked), to keep the lock and the dev graph small. Where the build departs from the design:
     - **Four limits are keys** beyond OP3's first list, now in it: `server.header_timeout_seconds` (10), `server.handler_timeout_seconds` (30), and from the review `server.write_timeout_seconds` (30) and `server.max_connection_seconds` (3,600), so tests set them small ("every limit is a key").
     - **The head is bounded exactly.** hyper's `max_buf_size` alone let a 100 KiB head through (its read buffer grows past the cap before the check), so `max_header_size` is set to the same 64 KiB: a larger head is `431`.
     - **Keep-alive idle is the header timeout's.** hyper runs the header timer while a connection waits for its next request, so an idle connection closes after 10 s, below HX3's 60 s; there is no separate key.
     - **The error document's order** is DC9's, `format` and `format_version` first, then RFC 9457's members; section 7.11's example lists `type` first.
     - **`fitment.info`'s `formats`** is a list of `{format, format_version}`, since DC9's snake_case keys rule out a map keyed by format names; `limits` adds `page_default`.
     - **Readiness without the lease** (another holder took it, or a renewal failed) is `503 shutting_down` with `details.reason: "lease"` (or `"draining"`): OP7 names those two codes only.
     - **`/v1/info` answers without authentication until PR 3**: `OPERATIONS` records its role (`reader`) and the OpenAPI document states it, but the extractors are PR 3's. Meanwhile at most four (`INFO_CONCURRENCY`) read the catalog at once, the rest waiting within the handler's time, and each counts the entries in one query (`Snapshot::status_counts`); it opens the catalog per request until PR 7's reader pool (CX1).
     - **The OpenAPI document** (308 KB with the doc comments as descriptions) gives each format a self-contained schema with its own `$defs` and an `$id` of `https://fitment.dev/schemas/<format>/<version>`, so a `$ref` resolves inside it; the frozen result documents are closed (`additionalProperties: false`, or `unevaluatedProperties: false` where serde's `flatten` composes an object), the envelopes stay open (VN1). `x-fitment-error-codes` lists `ErrorCode::ALL`. `CatalogDocument`'s schema is built in `catalog.rs`, the one module naming the catalog crate (SV4).
     - **The lease** is released by a graceful stop and also when the last handle of a `Server` that never served is dropped; a refused start releases it at once.
     - **`check-config`** writes nothing: `server.db`'s header and the catalog's are read as bytes (`Catalog::inspect`), with no connection, so no `-wal` or `-shm` appears and nothing migrates; a missing state directory is fine. `open` checkpoints after creating the schema so the file's header is current.
     - **The image's notices** come from `deploy/licences.sh`, not `cargo deny list`, so the builder needs no tool beyond cargo and sh: `THIRD-PARTY.txt` lists every crate of the two binaries' locked graph with its declared licence, and `licenses/<crate>-<version>/` holds each crate's own `LICENSE*`, `COPYING*` and `NOTICE*` files as published, no build path named. The scan for Python allows the distroless base's own gcc pretty-printer scripts (`usr/share/gcc/python/*.py`, no interpreter): it fails on an interpreter, uv or Open CASCADE anywhere and on any of their names among the files the image adds. The smoke run (`deploy/smoke.sh`) covers health, readiness, `fitment.info`, the image's `healthcheck` and a SIGTERM stop with exit 0; the bootstrap, uploads and the socket and match jobs join it in PR 3, 4 and 6. Its size ceiling is twice the first measure of the larger architecture: 15.0 MB on arm64, 37.7 MB on amd64 (CI), so 75.4 MB.
     - **`server.trusted_proxies`** takes exact addresses, no ranges. Under `test-support`, stdin's end of file always stops the server; the harness keeps stdin open.
     - **Packaging** excludes `fitment-server` as well as `fitment-corpus` (CI's `package` job and `tools/check-core.sh`): `cargo package --workspace` does not skip a `publish = false` crate.
     - **SD3's steps 5 to 9** are what PR 2 has: the catalog opened, refusing a newer schema; the instance id (CA8), recovery, the reader pool, the reindex and the dispatcher come with PR 5 and 7. With schema 1 the first, there is no migration; an empty `server.db` is created in one exclusive transaction before the lease is taken.
     - **ARCHITECTURE.md** is unchanged: section 18 gives its crate map to PR 9.
     - **Changed in review.**
       - *OpenAPI:* closing a schema recursed into the `properties` and `$defs` maps as if they were schemas, so `fitment.catalog`'s and `fitment.match`'s documents declared a field named `additionalProperties`; only subschemas are visited now, and `openapi.rs::no_properties_map_names_a_keyword` holds it. `fitment.error`'s `code` is an open string in the schema (codes are added within v1; the known ones are `info.x-fitment-error-codes`; a client acts on `class`). VT36 also validates the bodies of the API's `.response` goldens, `fitment.info` and `fitment.error` among them.
       - *check-config* is read-only (above); fitment-catalog gained `Catalog::inspect` and `Snapshot::status_counts` (section 5.2's review note).
       - *Connections:* a write that makes no progress for `server.write_timeout_seconds` closes the connection, and none lives past `server.max_connection_seconds` (HX1's review note); a trusted proxy is compared by its canonical address, so an IPv4 peer on a dual-stack socket matches. hyper's own `400` and `431` for a request it cannot parse have an empty body, outside the error contract (HX1).
       - *Environment:* a key's variable is `FITMENT_SERVER__<SECTION>__<KEY>`; another `FITMENT_SERVER_*` (Kubernetes' service links for a service of that name) is ignored with one warning (OP2).
       - *Paths:* a `..` in `paths.catalog` or `paths.state` is refused (exit 2), since through a directory not yet made it defeated SV6's comparison one way and refused a sound layout the other (OP3).
       - *The state directory:* a marker without a non-empty `server.db` is refused (exit 2), as a catalog without its database is; on Unix the directories are made 0700 and `server.db` 0600 before SQLite opens it, so its `-wal` and `-shm` take that mode; start errors name paths as `state:…` and `catalog:…` (OP1).
       - *The lease:* renewed and released in a `BEGIN IMMEDIATE`; a busy database is retried; another holder, or 30 s without a renewal, is a loss, which stops the server with a non-zero exit (SD6's review note). No start reclaims its own lease after a crash: the 30 s wait stands in M6.
       - *server.db:* the reader pool's fallback connection is configured as the pooled ones and is not kept past the pool's size; two triggers refuse a token moved to the `system` user and a change of a user's `kind` (section 6.2).
       - *The image* ships every crate's licence and notice texts (above); the smoke run checks them. SD1's Windows triggers now include Ctrl-Break and a console's close event.
       - *The harness* scans response header values as well as bodies, and the whole captured log (or a child's stderr) for secrets whenever a test's server is dropped, sparing only the lines a control planted.
       - *Design text:* F4 and F18 give exit 3 for a newer schema, as the code and DB5 do.
3. **Authentication and permissions.**
   - **Contents:** AU1 to AU7, AU9 and AU10 (Vectera's logic ported, the eleven fixes; minting within the presenting token, cascading revocation, revocation on a password change, the live-token cap, per-user caps and bounded login buckets), the `/v1/auth/*` routes, `admin bootstrap`, `user` and `token` commands, the rate limits, the extractors on every route of `OPERATIONS`.
   - **Tests:** VT23 over every operation that exists, VT24 (its per-user upload and stream cases in PRs 4 and 5); the `401` and `403` goldens.
   - **Done when** the matrix covers every operation that exists and passes, and each of the eleven fixes and each AU10 case has a test. XH14 stays deferred to PR 8, when every operation exists.
   - **As built.** Done as planned, AU1 to AU7, AU9 and AU10 built and cited in `RULES` (`M6_PR_NOW` is 3; AU8 waits for PR 6). Vectera's `catalog/src/auth.rs` and `commons/src/auth.rs` are ported with attribution in each file: `hash_password` and `verify_password` (`auth/password.rs`), `generate_token`, `hash_token` and the prefixed token (`auth/token.rs`), the role hierarchy and its `permits` table (`auth/roles.rs`), `CurrentUser` and the `ReadPerm`, `WritePerm` and `AdminPerm` extractors rewritten for axum as `Principal`, `Reader`, `Writer` and `Admin` (`http/extract.rs`), and the bootstrap admin, now offline (`admin.rs`). argon2 is 0.6.0 (`cargo deny` passes its licences unchanged), without its `getrandom` feature since the salt comes from our own `getrandom` call; the debug profile optimizes `argon2` and `blake2`, which changes no result, because the tests hash on every login. The routes are `/v1/auth/login`, `me`, `password`, `tokens` (list, mint), `tokens/{id}` (revoke; `current` is the presenting token), `users` (list, create) and `users/{id}` (`PATCH`); `OPERATIONS` holds 13 operations, each route's role layered from its entry, and the API's minor is 1. Each of section 4.11's fixes has its test: (1) `auth.rs::a_login_costs_one_hash_whatever_the_username`; (2) `hostile.rs::a_token_mints_only_within_itself` and `auth.rs::revocation_cascades_and_a_password_change_revokes_the_rest` (a reader mints and revokes their own); (3) `admin.rs::bootstrap_prints_the_first_admins_token_once` and the smoke run; (4) `auth.rs::dead_tokens_are_swept_after_30_days`; (5) `auth.rs::authentication_writes_nothing`; (6) `password.rs::the_pepper_is_argon2s_secret` and `auth.rs::the_pepper_keys_every_hash`; (7) `password.rs::lengths_are_characters_with_an_upper_bound` and `hostile.rs::passwords_past_their_bounds_are_refused_before_hashing`; (8) `auth.rs::argon2_runs_on_the_blocking_pool_two_at_a_time`; (9) `password.rs::the_parameters_are_recorded` and `auth.rs::older_parameters_are_rehashed_at_login`; (10) `hostile.rs::logins_are_limited_per_pair_and_per_address` and `login_buckets_are_keyed_by_the_client_and_bounded`; (11) `auth.rs::every_token_expires`. VT24's per-user upload and stream cases wait for PRs 4 and 5. Where the build departs from the design:
     - **A code added:** `username_taken` (usage, `409`), for `POST /v1/auth/users` and `admin user add` with a username another user holds, compared folded: no code of section 7.12 said it, and ER7 puts a conflict with the server's state at `409`. The table and `ErrorCode::ALL` gain it (83 codes).
     - **Six limits are keys** beyond OP3's first list, now in it: `auth.hash_wait_seconds` (AU2's 10 s), `auth.rate_per_second` and `auth.rate_burst` (50 and 100), `auth.login_per_minute`, `auth.login_failures` and `auth.login_failure_minutes` (30, 10 and 15), so tests set them small. The buckets' clock is monotonic, since a fixed wall clock (OP11) would never refill them; a `test-support` seam freezes and moves it, so a test meets a bound exactly.
     - **Usernames** are 1 to 64 characters of `[A-Za-z0-9._@+-]`, folded as ASCII lower case: Unicode case folding would need a dependency and could change with Unicode's tables. A login bucket's key is the SHA-256 of the folded string, so it is bounded for any username and holds none.
     - **The requests' optional fields:** a login's `token_name` (default `login`) and `days` (default `auth.token_default_days`); a mint's `role` (default the effective role) and `days`. A mint without `days` expires at the default or at the presenting token's expiry, whichever is sooner: nothing was asked, so nothing is capped; a `days` past the presenting token's is `400`, as AU10 says. `days` is 1 to `auth.token_max_days` everywhere.
     - **A login with a password outside AU1's bounds** matches nobody's, so it is a failure counted in the pair's bucket without a hash. A password change verifies `current` as a login does, in the caller's login buckets, so a stolen token cannot guess its user's password faster than a login can.
     - **A mint re-checks its source** inside its `BEGIN IMMEDIATE`: the presenting token still live and its user enabled, or, for a login, the user enabled and the PHC string the one verified, so a revocation or a password change that commits first is never outrun by a token minted beside it.
     - **Query parameters:** each operation lists those it takes (`OPERATIONS.query`); any other is `400 invalid_parameter`, so `?owner=` on a route that takes none, every write among them, is refused (AU8, VT23). A token in a query string is `400 bad_request` on every route, checked first (AU3). `GET /v1/auth/tokens?owner=` naming another user needs the admin role (`403 forbidden` below it, `x-fitment-role-fields`), an unknown one is `404`.
     - **The order of the checks:** a body where none is taken (HX2), then `Accept` (VN8), then the query, then authentication and the role, then the JSON body, read under its cap by the handler, so usage is answered before credentials are read (ER5) and an unauthenticated body is never read.
     - **The `system` user** is not listed by `GET /v1/auth/users` and is `404` to `PATCH`: no one manages it.
     - **`server.db`'s user and token SQL** is in `state/db/auth.rs`, a submodule of `state::db`; SV6's "opened by `state/db.rs` alone" is read as the module, and `layout.rs` and `tools/check-core.sh` allow `state/db/`.
     - **The auth half of JS9's sweep** (the batched `last_used_ms` written; tokens deleted 30 days after expiry or revocation) runs every `jobs.sweep_minutes` in a task of `serve`, and at a graceful stop and a server's drop (SD1's last writes); PR 5's sweep takes it over. It deletes leaves first: a revoked token a live one was minted from (a password change keeps the presenting token) stays until that one goes.
     - **The pepper file** holds 16 to 1,024 bytes once a final line break is dropped, and on Unix group or other bits refuse the start (exit 2). The offline commands read the same configuration (`--config`, or `FITMENT_SERVER_CONFIG`) for it, needing only `paths.state`.
     - **The offline commands** are `admin bootstrap`, `admin user add|list|set|reset-password` and `admin token mint|list|revoke`; a secret goes to stdout once, alone on its line, and stderr never holds one. They make a new state directory as a first start does. Schema 1 is the first, so no older `server.db` exists to migrate: the refusal beside a held lease is the decision `state::db::may_migrate`, tested on its own.
     - **Pages:** `fitment.page` of `fitment.token` (by id) and `fitment.user` (by folded username), with an opaque cursor and `Link`, are VN3's keyset pages built early for these two lists; VN3 stays deferred to PR 4, which adds the first owned resource's pages.
     - **The OpenAPI document** gains the request bodies (closed schemas, each with its `$id`), the path and query parameters, the `bearer` security scheme on every operation but the four open ones and login, and the `fitment.token`, `fitment.user` and `fitment.page` schemas; `fitment.info`'s formats list the three.
     - **The goldens** seed their principals with the fixed ids, record `authorization: Bearer <…>` rather than a token and mask a creating response's secret; `/v1/info`'s scenarios now authenticate, so their goldens and the OpenAPI document's were rewritten with the minor's rise. Under `test-support` with fixed ids a token's secret is derived from the id counter (`Ids::secret_bytes`), so a hint is exact.
     - **The harness** presents an admin token made with the offline library unless a call names its own credentials or is anonymous, and the leak scan knows every password, token (with and without its prefix) and pepper the tests use; a creating response's secret is registered and masked for the scan. VT24's 10⁵ usernames run on the limiter itself, and 500 over HTTP, with passwords too short to hash.
     - **The request's log span** names the user by id once authenticated, never by username (OP1).
     - **`/v1/info`** keeps `INFO_CONCURRENCY` until PR 7's reader pool, though it now needs a reader. CORS's methods stay HX4's, without `PATCH` (decided in review: no browser client until M8).
     - **Changed in review.**
       - *The pair limit:* it was peeked before the hash and charged after, so concurrent wrong guesses all passed (30 of 30 measured). `LoginLimits::admit` now takes the address's token and the pair's failure under the lock and returns an `Attempt`, which refunds the failure when dropped unless `failed()` charges it; a success and the exits that never judged the credentials refund. The password change uses it too. `hostile.rs::concurrent_wrong_guesses_cannot_pass_the_pair_limit` (30 at once: 10 hashed, 20 `429`) and `successes_refund_and_ipv6_is_keyed_by_its_64`.
       - *Addresses:* IPv6 keyed by /64 in the address's and the pair's buckets; `X-Forwarded-For` entries with a port read; one warning a minute for the header from an untrusted peer. `deploy/fitment-server.toml` shows `trusted_proxies` and `pepper_file` beside the proxy advice.
       - *Indexes and the sweep:* `token_user (user, revoked_ms, expires_ms)` and `token_parent (parent)` in schema 1 (section 6.2; schema 1 is not released, so it changes in place). The sweep walks keys downwards in batches of 500 (`SWEEP_BATCH`), each its own short transaction: a minted token has the larger key, so a dead child goes before its parent is looked at and one pass suffices, linear in the tokens (`auth.rs::the_sweep_is_linear_in_the_tokens`: 20,000 dead tokens, a chain of 1,000 among them). Minting is limited to `auth.mints_per_minute` (10) a user, a new key.
       - *The cap:* a login at it evicts the oldest live root token, with its descendants, instead of `409`; a mint at it is still `409 too_many_tokens`.
       - *Reach:* a minted token lists and revokes itself and its descendants only; a root token its user's every token; admins as before.
       - *Races:* `bootstrap` checks for an admin in the transaction that inserts (`create_first_admin`), and eight at once on a state not yet made make one admin; that showed a first open could fail `database is locked`, since SQLite answers a switch to WAL that meets another connection's lock at once, without its busy handler: the switch is retried within the busy timeout, the busy timeout is set before the first read, and only the connection that made the schema checkpoints. `set_password` re-checks the presenting token and the PHC string in its transaction.
       - *Disabling* revokes every token of the user; enabling restores none. The API refuses to disable or demote the last enabled admin, `409 last_admin` (a new code; the offline `user set`, the recovery path, may).
       - *The offline commands* warn on stderr whenever they hash without a pepper (whether the server has one is recorded nowhere) and refuse `--password-stdin` from a terminal (not tested: the tests have no terminal).
       - *A token-shaped query value* that does not decode is still `400 bad_request`: the query is decoded leniently for that check.
       - *Decided and recorded:* passwords stay byte-exact (AU1); pepper rotation is out of M6 (AU1, the image's configuration); CORS without `PATCH` (HX4).
       - *A password change that loses* answers `401 unauthenticated` rather than a `409`: the winner revoked its token, and the answer is the one its next request would get.
4. **Uploads and the object store.**
   - **Contents:** CA7 in `fitment-catalog` (the streaming writer and reader, with `CountingFs` over them); the object store and its rules (OB2, OB3, and OB4 for uploads) against `fitment_catalog::fs::Fs`; CO1; the upload routes with streaming, caps, the minimum throughput, quotas and per-user access (UP1 to UP4, UP8, AU8 for uploads); upload expiry in the sweep; keyset pages (VN3) for uploads.
   - **Tests:** VT17 and VT20 for uploads; VT18's upload throughput; VT13's upload half; VT11 at every upload file call through `CountingFs::failing_at`; VT25 for uploads; VT48; VT37's CO1; VT38's CA7; VT42's pages.
   - **Done when** every corpus file uploads with the SHA-256 the test computes, `HEAD` and the status reveal nothing of another user's bytes, and every upload limit has a test that exceeds it.
   - **As built.** Done as planned, every rule section 11.1 defers to PR 4 built and cited in `RULES` (`M6_PR_NOW` is 4): UP1 to UP4, UP8, OB2, OB3, VN3, CO1 and CA7; UP2, UP4 and VN3 have their tests in `hostile.rs` and `slow.rs` (`M6_LIMITS`), OB2 in `fault.rs` (`M6_FAULT`). UP5, UP6, UP7, OB4, OB5 and AU8 beyond uploads wait for PRs 5 and 6 as the registry says. `crates/fitment-server` gains `uploads.rs` (the stream, UP4's slots and staged bytes, `fitment.upload`), `http/routes/uploads.rs`, `state/objects.rs` (the store, OB2, OB3), `state/db/uploads.rs` (its SQL) and `state/sweep.rs` (the store's part of JS9's sweep); `objects.rs` names the catalog's `Fs` through `catalog.rs`, which re-exports it, so `catalog.rs` stays the one module naming `fitment_catalog` (SV4). `OPERATIONS` holds 19 operations and the API's minor is 2. Every corpus file (462) uploads by `PUT` and again by `POST` with the SHA-256 the test computes, in 13 s in release on the Mac (`api.rs::every_corpus_file_uploads_with_its_sha256`, release only, as the corpus suites are, and so run by CI's `corpus` job after them; its user's request rate is raised, since its 924 requests pass AU7's 50 a second). Where the build departs from the design:
     - **CO1** is `Sha256::new`, `update`, `finish` and `finish_hex` over the existing compression function, a block folded as soon as it is full; `sha256` and `sha256_hex` feed it once. The tests keep the one-shot function as it was as their oracle: every split point of messages of 0 to 200 bytes, every pair of split points up to 140, the standard vectors byte by byte and in uneven pieces, a clone hashing on alone.
     - **CA7** is two traits, `FsWrite` (`write_chunk`, `sync`) and `FsRead` (`read_chunk`), and two required methods of `Fs`: `create_new_stream(path)` (created new, 0600 on Unix) and `open_stream(path, cap)` (a regular file, its type checked before and after the open, its length against the cap, and a read that meets a byte past the cap refused as `InvalidData`). `CountingFs` records `CreateStream`, `WriteChunk` with the chunk's length, `SyncStream`, `OpenStream` and `ReadChunk` with the room given, and fails any of them through `failing_at`; it is now `Clone`, a clone and every stream it opens sharing its count, so a test keeps a handle on the seam it hands the server. The catalog's own blobs keep `write_new`, unchanged; the only change to its tests is the two forwarders its test seam (`tests/common`) needs.
     - **The upload's path** is one segment, `/v1/uploads/{upload}`, parsed by the handler as `sha256:` and 64 hex digits, either case (HX5): a percent-encoded colon reads the same, and anything else is `400 invalid_reference` on every method, as a malformed reference is, rather than a `404` from a route that did not match.
     - **`HEAD`** is its own operation (`uploads_head`, an OpenAPI `head`), answering what `GET` answers, headers and status, without a body: axum strips a `HEAD`'s body in process too, so VT23's matrix compares a `HEAD` by its status, and the harness's checks skip an empty body.
     - **Five limits are keys** beyond OP3's first list, now in it: `limits.upload_concurrency_per_user` (UP4's 2), `limits.upload_chunk_seconds` and `limits.upload_window_seconds` (HX3's 30 s for the next chunk and for the throughput's window), and from the review `limits.upload_first_byte_seconds` (5) and `limits.user_max_uploads` (10,000), so tests set them small. Every upload key is at least 1 (a zero refuses the start).
     - **Codes.** UP4's slots, per user and per server, answer `429 too_many_streams`, retryable with `Retry-After`, `details.limit` naming the key: section 7.12 named no code for them, and HX8's `limit=all` cap set the precedent (the table now says so). A body without a chunk for `upload_chunk_seconds` is `408 upload_too_slow`, as one under the minimum throughput is; one past `upload_minutes` is `503 request_timeout`, retryable on the upload's `PUT` (ER6); a body that breaks off is `400 bad_request`, and so is one whose length is not its `Content-Length` (hyper enforces that on the wire; a body handed in whole is held to it too). A body's type is one of section 7.3's three (parameters allowed); none, or another, is `415`, as any `Content-Encoding` is (DC6). A draining server answers an upload `503 shutting_down` (SD1's sentence, built here).
     - **`fitment.upload`** is `{format, format_version, sha256, bytes, held_by, pinned_by_jobs}`, `held_by` the caller's rows of `upload`, `assembly`, `socket` and `job` that hold the bytes, `pinned_by_jobs` how many of the caller's jobs hold them as an input; it holds no time (DC9). A re-upload of an expired row of the caller's is `201`: it named nothing.
     - **UP8's calls**, as built and as VT48 records them (as changed in review, below): the temporary file created, its chunks written and synced; then inside the `BEGIN IMMEDIATE`, in both branches, the final name's metadata read (never its bytes), the temporary file renamed over the final name, the object made read-only and the fan-out directory synced. It is 0600 until renamed, the object 0444 after, as the catalog's blobs are.
     - **Quotas.** The per-user quota counts the caller's unexpired uploads and the inputs and documents of their jobs (empty until PR 5), each object once and charged as the review set (below); it is checked before a byte is read against a declared length, and again as the bytes stream. The staging quota reserves a declared length up front and counts streamed bytes past it, so the open temporary files never pass `limits.staging_gib`; the bytes are released when the temporary file is gone. Bytes are written in pieces of up to 1 MiB, each hashed and written on the blocking pool (SV3).
     - **Timeouts.** The handler timeout moved from the router to each route, so an upload's is its own: `limits.upload_minutes` plus a minute behind the upload's own check of its minutes. A temporary file is removed before the answer on every refusal; an upload dropped mid-flight (a layer's timeout, the connection gone) drops it on the blocking pool.
     - **`DELETE`** removes the caller's row only; the sweep unlinks the object when no row names it (as changed in review, below). "A row names it" is any row of a table with a foreign key to `object`, so an object row is deleted only when nothing could refer to it; PR 5's sweep deletes the expired rows of the jobs' tables that would otherwise keep it.
     - **The sweep's store part** (JS9's, built for uploads here): expired upload rows deleted in batches of 500; candidate objects no row names unlinked with their rows, 256 to a transaction (as changed in review); files of the store without a row unlinked under the same check (a crash between an upload's rename and its commit leaves one); temporary files older than `upload_minutes` plus a minute removed. It runs with the auth sweep every `jobs.sweep_minutes`. A start removes uploads' temporary files from `tmp/` (SD4's first sentence, built early: a killed upload leaves a `.part`).
     - **OB3** is `Objects::read` (one buffer, at most 64 MiB, hashed against its name) and `Objects::open_checked` (above it: a first pass of the streaming reader to hash, a second to send). No route of PR 4 sends an object's bytes, so `blob_damaged`'s golden waits for PR 5's downloads; `too_many_streams`'s stays with PR 5's event streams.
     - **The hold window from the upload's side** (review F20): no route makes a job yet, so `fault.rs::a_jobs_inputs_hold_its_uploads_from_submission` seeds the `job` and `job_input` rows PR 5's submission will write (JS11) and shows what this PR's code reads of them: an upload a job that is not terminal names is `409 upload_in_use` to `DELETE`; a retained match's input stays the caller's to name after the upload expires (UP3), a non-match job's does not; the inputs count in the caller's quota (UP4); the sweep unlinks an object only when no input names it (OB2). The `upload_in_use` golden is seeded the same way. OB4 and UP5 themselves, the rows written and deleted at the job's submission and end, are PR 5's.
     - **Goldens** produce `invalid_reference`, `upload_too_slow` (a test-support hook point, `upload.chunk`, delays each read), `upload_digest_mismatch`, `too_large`, `storage_full` and `io_error` (the store on `CountingFs::failing_at`), `quota_exceeded` and `upload_in_use`, each with an upload route's scenario; the harness gains `TestServer::in_process_with_fs`, `seed_upload`, `put_upload`, bodies fed or made chunk by chunk, and `send_body`.
     - **VT18** here is the upload's throughput, its chunk wait and VT13's disconnect; its 256 connections at once wait for PR 8.
     - **Changed in review.**
       - *Throughput samples* (H1): one sample was kept per body frame, so 1-byte frames grew a single upload's memory to 1.59 GB. Samples closer than a 64th of the window are now one, the later count kept on the earlier time (which errs only toward the stricter answer), the `(started, 0)` base kept: about 64 a window whatever the frames. `hostile.rs::tiny_frames_keep_the_samples_bounded` sends 100,000 one-byte frames and reads a test-support count of the most samples held.
       - *Indexes and candidates* (H2): no column naming an object was indexed, so the sweep and `DELETE` were quadratic (a `PUT` stalled 12 s a sweep at 30,000 rows). Schema 1 (unreleased) gains an index on each such column (`upload`, `assembly`, `assembly_parts`, `socket_revision`, `job_input`, `job`'s two), one on `upload(expires_ms)`, and `object_candidate`, filled by triggers when a naming row is deleted: the sweep looks only at candidates, 256 to a short transaction, and `HOLDING` and `NAMED` are an index probe a table (section 6.2). `fault.rs::the_sweep_follows_its_candidates_not_the_store`: 20,000 objects, half expired, swept within a bound with a `PUT` beside it waiting at most a batch, an idle sweep quick.
       - *`DELETE` never unlinks* (M1): its time told whether another user held the bytes; it removes the caller's row, and only the sweep unlinks (UP3, OB2). VT48 now compares two deletes too: no file call either way.
       - *One branch* (M2): removing the temporary file in one branch and renaming it in the other differed in time on Linux (a clean against a dirty directory's sync), as did an extra `INSERT` and a fan-out directory made in one branch. Both branches now rename over the final name (an atomic replacement of identical bytes, which also repairs a wrong file of the right length, L2), both insert the object's row `ON CONFLICT DO NOTHING`, and the state makes all 256 fan-out directories at its creation. Before the rename, a directory standing at the name is removed, another non-regular entry unlinked, and on Windows a regular file's read-only flag cleared (as `blobs.rs` does), Windows' one extra call. `Fs` gains `remove_dir` for it (CA7). VT48 adds a timing tripwire, the two branches' medians within three times each other and 20 ms, since call shapes cannot see the disk's own time. `fault.rs::whatever_stands_at_an_objects_name_is_replaced`.
       - *First bytes and a pro-rated minimum* (M3): four readers could hold every upload slot with no bytes for a whole window. An upload's first bytes now come within `limits.upload_first_byte_seconds` (5, a new key; the window, if shorter), and from then until the window has passed it owes the minimum for every second it ran. `hostile.rs::an_upload_owes_its_first_bytes_and_then_its_rate`.
       - *What the quota charges* (M4): a quota of bytes let tiny objects use up the disk's inodes. An object is charged `max(bytes, 4096) + 512` (`state::charge`), and a user holds at most `limits.user_max_uploads` (10,000, a new key) live uploads, one more `507 quota_exceeded` with `details.limit: "user_max_uploads"`: a quota, not a rate, so 7.12's `quota_exceeded` rather than a `429` (the table now says so). `hostile.rs::a_user_holds_a_bounded_number_of_uploads`.
       - *Uploads at once share a quota* (L1): each upload checked the quota alone, so two of one user overshot it, and a re-upload of held bytes counted them twice. A slot now carries its upload's charge, and a user's running uploads together stay within the room their holdings leave; a `PUT` naming bytes the caller holds skips the quota and the live-upload cap, since it only extends a lifetime. `hostile.rs::uploads_at_once_share_their_users_quota`, `the_staging_quota_counts_open_temporary_files`.
       - *The second pass* (L3): `open_checked` opened the file again for its second pass, unverified. It now rewinds the same open file (`FsRead::rewind`, a seam call, CA7) and hashes the second pass as it goes, its last read failing `InvalidData` on a mismatch, so a sender aborts. `api.rs::a_file_changed_between_the_passes_fails_the_second`.
       - *`TooLargeForBuffer`* (L4): `Objects::read` above its one-buffer cap answered `Damaged`; it answers its own error.
       - *A start's temporaries:* m6/auth's fix for concurrent first starts writes the marker in `tmp/` and renames it into place, so a start now removes only uploads' temporary files (`<32 hex>.part`); the sweep still removes any file there older than an upload lives.
5. **Jobs, with features, assemblies and socket extraction as their first kinds.**
   - **Contents:** the job store, scheduler, lanes, permits and fairness (JS1 to JS6, JS9 to JS11, JS13; JS8 for these kinds, completed in PR 7); states, stops, restart, deadlines, events, idempotency, waiting and retention (JB1, JB3 to JB9, JB11, JB12); shutdown and recovery (SD1, SD2, SD4, SD5); the hold window (OB4, UP5); `features`, `assembly_read` and `socket_extract` with their routes, socket lines, revivals and revisions (RA3 to RA5, OB5, UP7); the document rules for these kinds (DC1, DC2, DC4 to DC6, DC8, VN5); metrics (OP6).
   - **Tests:** VT9; VT10 at every hook point, JS13's included (the soak waits for PR 8); VT11 for the job store; VT13's download half; VT14 on socket jobs; VT21; VT33's shutdown with jobs in flight; VT6's job goldens; VT7's `features`, `parts` and `socket` rows; VT40's id and revision cases; VT41 for these kinds; VT42's waiting, idempotency and job ETags; VT43; VT49's holds and revival; XH12 checked for these kinds (completed in PR 7).
   - **Done when** an abort at every named point leaves XH12 holding on three OSes, and every corpus source's `fitment.parts` and socket, extracted through jobs, equal `fitment parts`'s and `fitment socket`'s bytes.
   - **As built.** Done as planned, every rule section 11.1 defers to PR 5 built and cited in `RULES` (`M6_PR_NOW` is 5): JB1, JB3 to JB9, JB11, JB12, JS1 to JS6, JS9 to JS11, JS13, SD1, SD2, SD4, SD5, DC1, DC2, DC4 to DC6, DC8, RA3 to RA5, VN5, OP6, UP5, UP7, OB4 and OB5; JB5, JB6, JS11, JS13, OB4, OB5 and SD1 to SD5 have their tests in `fault.rs` and `concurrency.rs` (`M6_FAULT`), JS5, JS10, JB7 and JB11 in `hostile.rs` (`M6_LIMITS`), DC1 and JB4 in `parity.rs` (`M6_PARITY`). JS8 and XH12 are checked for these kinds (VT41, VT44's checks in every fault test) and stay deferred to PR 7, as the registry says; CA8 too, since no catalog view is served yet (VN5's catalog ETags come with it). `crates/fitment-server` gains `jobs/` (`model.rs`: kinds, lanes, states and JB3's transitions; `scheduler.rs`: per-lane FIFOs per owner, owners in turn, the slots and caps; `mod.rs`: the dispatcher, the fair CPU and memory semaphores, the stop flags, deadlines, waits, the drain and recovery; `run.rs` and `socket.rs`: one function per kind; `events.rs`: the broadcast, the replay-then-follow stream, the caps; `documents.rs`: `fitment.job` and `fitment.job.event`; `request.rs`: the bodies, references, names, keys, fingerprints and `Prefer`), `assemblies.rs` (`fitment.assembly` and the lines' and revisions' items), `metrics.rs` (OP6), `http/cache.rs` (stored bytes out: OB3, DC4 to DC6, VN5), `http/routes/{jobs,features,assemblies,sockets}.rs`, and `state/db/{jobs,assemblies}.rs` (their SQL). `OPERATIONS` holds 39 operations and the API's minor is 3. Every corpus assembly (220, the 217 assembly cases and three sources of match cases) registered through `assembly_read`, and every socket the cases name (80: the match cases' 33 and the assemblies' own), extracted through `socket_extract`, are `fitment parts --format json`'s and `fitment socket --part P --format json`'s bytes and exit codes, 0 differences, in 13 s in release on the Mac (`parity.rs::every_corpus_source_is_the_clis_through_jobs`, run by CI's `corpus` job); the committed unit parts, assemblies and sockets are compared on every OS (`parity.rs`), the CLI built beside the tests by the harness (`support::cli_binary`), since `CARGO_BIN_EXE_fitment` is another package's. An abort (`std::process::abort()` at a hook, so Windows too) at each of seven points of a job's life, for extractions and assembly readings, then a restart, leaves each job ending once (`fault.rs::an_abort_at_every_hook_point_leaves_each_job_ending_once`). Where the build departs from the design:
     - **Schema 1 changes in place** (unreleased): `job.result` (the href of what a job made, `fitment.job`'s `result`), `assembly_parts.root` (`fitment.assembly`'s root, as JSON), `socket_line.last_n` (a tombstoned row's highest revision, so RA4's numbering survives the sweep of its revisions), and indexes for recovery, retention, JS13's look-up, the lines of an assembly and a revision's children (section 6.2).
     - **A code added:** `job_not_finished` (usage, `409`), for `DELETE /v1/jobs/{id}` of a job that has not ended: no code of section 7.12 said it (85 codes).
     - **Two limits are keys** beyond OP3's list, now in it: `jobs.streams_per_user` (8) and `jobs.streams` (256), JS10's caps, so tests set them small. Under `test-support` only, a `[test]` section takes `hooks` (a point's action: `abort`, `panic` or `delay:<ms>`, for the built binary, VT10) and `lease_stale_ms` (SD6's 30 s, shortened so a killed child restarts at once); a release build refuses the section as an unknown key. Further seams: `Server::hold_jobs` and `resume_jobs` (jobs stay queued without a drain), `Jobs::set_deadline` and `set_heartbeat`, and a hook action `Pause(Gate)` a test opens, which counts arrivals, so tests wait for a point, never for a time.
     - **Caps per owner are per lane** (JS4): `jobs.per_owner_running` and `jobs.per_owner_queued` count an owner's jobs in each lane, so a long ingest never takes a user's query slot.
     - **`fitment.job`:** `queue_position` is the job's rank, from 1, among its lane's queued jobs in submission order (the round-robin may start it sooner); `progress` is shown while the job runs only, so a finished job's document is the same after a restart; `notes` is `[]` until JB10 (PR 6). A job whose document states its refusal (exit 3) has `error: null`, since the document is the answer (ER4); one whose reading refused the input as a fitment bug (an internal refusal, the CLI's exit 1) is `failed`, `internal`, its refusals in `details`, its document kept. The `end` event's fields follow JB8's order.
     - **Sockets:** a line's occurrence is `*`, or for an `occurrence` selection the chosen occurrences' usage `#id` paths; `carry` defaults to true (section 7.6's "extracting onto an existing line carries"), the one boolean that does not default false (VN6), and `carry_from` wins over it. `carry_from` is checked at the request (a revision of the caller's, a bare `sk_X` resolved to its current revision as RA9 says, or an upload held, within `limits.max_socket_mib`) and read in the job, leniently through `SocketDocument::prior`: one that is not a socket document or is another file's or target's ends the job `refused`, exit 2, `carry_from_foreign`, as the CLI refuses that OUT, since the parse semaphore is PR 6's. The job holds the carried document's object too (OB4). A finishing transaction commits a revision only over the current revision the job carried from; one made meanwhile has the job carry again (at most eight times, then `internal`). Revisions of a deleted line no job cites and no revision names as its parent are deleted by the sweep (OB4); the line's tombstone keeps its highest number.
     - **Assemblies:** the row is made with the job's end (JS13), so a failed reading registers nothing; `POST /v1/assemblies` of an assembly the caller holds at this build adds the name and answers `200` `fitment.assembly`, but a key's replay (JB9) answers the job it named first; `DELETE` deletes the rows (no tombstone: nothing cites an assembly before PR 6's originals), their objects then the sweep's.
     - **Pages of lines and revisions** have `kind` `socket_line` and `socket_revision`: their items are not documents of their own.
     - **Stored bytes:** a job's document is `immutable` (it never changes for its job), a bare socket reference `no-cache`, an exact one `immutable`; DC6's gzip is `flate2` 1.1 with its pure-Rust backend (`miniz_oxide`; MIT, Apache-2.0 or Zlib, `cargo deny` unchanged), applied to bytes within the one-buffer cap only; a streamed object is sent as it is. Section 4.13's table gains `flate2`.
     - **A wait** also ends at a drain, answering `202` with the job as it stands, so `GracefulShutdown` never waits for it (SD1).
     - **SD1:** streams are told and dispatch stops first; the requests' 10 s and the jobs' `drain_seconds` then run together; a pure job stopped by the drain goes back to `queued`, with a `state` event `queued` (decided in the review, below). A panic after a job's commit leaves its end, and its events are sent again (streams and waits take them by `seq`).
     - **JS13's check at a restart** stays, though a job's rows and its end commit in one transaction, so a running job never holds a committed result; the abort tests show both sides of that transaction.
     - **OP6's listener** answers `GET /metrics` alone, without authentication, with a header timeout; the catalog's gauges are read at each scrape.
     - **XH14's goldens:** every operation has one (100 scenarios); the core documents a job or a route serves are written as a marker naming the CLI's golden they equal, not duplicated (VT6).
     - **VT14** runs 32 clients over the bound socket, each its own user, on every committed socket: about 130 refusals for load (upload slots) per run, all declared and retried; the per-owner cap and the turns are the scheduler's unit tests.
     - **The reviews** (soundness and Rust, merged; each fix has a test that failed before it):
       - **Downloads (OB3, DC6):** stored bytes are read into one buffer only up to 1 MiB (`http::cache::SEND_BUFFER_MAX`), as `Bytes` without a copy, and only those are gzipped; a larger object is sent identity, hashed in one pass and streamed in a second from an async task, each 256 KiB chunk read in its own short `spawn_blocking`, the send awaited outside it, so no blocking thread waits on a slow client. The ETag and `Repr-Digest` are unchanged. `wire.rs` checks that a large object streams and that unread downloads on a runtime of four blocking threads leave `GET /v1/jobs` answering.
       - **Submission (JS11):** the row's insert and what follows it (the `job.submitted` hook, the first event, the enqueue) run in a task of their own that the handler awaits, so a client gone or HX3's timeout cannot cancel them after the commit. A kick also reconciles: a queued row of `server.db` that the scheduler does not hold is enqueued (within a second of a kick and at most once a second), which covers a start whose enqueue failed too.
       - **Assemblies (RA6, OB4):** `DELETE /v1/assemblies/{ref}` is `409 assembly_in_use` while a job that has not ended names the assembly as an input; an extraction whose assembly row is gone at its commit ends `failed`, `internal`.
       - **Keys (JB9):** `features`, `assemblies` and `sockets` look up the `Idempotency-Key` and replay before resolving any reference, so a replay stands once its inputs are deleted.
       - **Streams (JS10):** a stream's channel is released by a guard on its receiver, however the stream or wait ends; a stream whose replay holds no `end` rereads the job, and if it has ended sends what `server.db` holds past the replay and closes (`?after=` at or past the end closes at once). A live persisted event past `last + 1` makes the stream read the gap from `server.db` first, so events published out of order arrive in `seq` order, none twice.
       - **`Fitment-Digest` (DC4)** of a socket job's document is the digest of the revision its `result` names, so an `unchanged` extraction carries it too.
       - **Carried revisions (OB4):** an extraction's `carry_from` revision is a `job_cites` row from its submission, so the revisions' sweep keeps it until the job ends.
       - **The job ETag (VN5)** is `W/"j<seq>.<state>[.p][.c]"`: `.p` when pinned, `.c` when a cancel was asked, which write no event.
       - **Deadline and cancel (JB5, JB7):** the first reason stands: a cancel of a job a deadline made `cancelling` answers `202` and leaves `cancel_requested` false; the job ends `failed`, `deadline_exceeded`, its key kept.
       - **OP6's listener** waits 50 ms after an accept error and serves at most 16 connections at once, closing one more at accept.
       - **Carrying (MF11):** a `carry_from` is refused as the CLI refuses its OUT (another file, or another part when both name one) and carried from only when its target is the new document's, so one naming no target carries nothing, as `fitment socket -o` leaves it (`parity.rs`).
       - **JS3's estimate** is 21 times the largest input: the CLI's peak resident set measured on the corpus's files of 2 MB and more (`features`, `parts` and `socket` in release on the Mac) was 13 to 20 times the file (`h_bracket_stubs.stp`, 36.7 MB: 522 MB for `features`, 14.2; `h_hole_grid.stp`, 16.9 MB: 15.9; `s55_tiles.stp`, 7.5 MB: 15.3 for `parts`, 17.2 for a socket; `h01_bore_grid.stp`, 6.2 MB: 18.6 for a socket; `h_bracket_tiles.stp`, 2.1 MB: 19.7), so 20, and the input's own bytes, which the job holds. Below 2 MB the CLI's own 6 MB dominates and the floor of 1 MiB stands.
       - **Decided:** a pure job ending `job_interrupted` (its second attempt, SD5) is `retryable: false`, its key still released; a drain's requeue writes a `state` event `queued`; a wait (`Prefer: wait=N` or `?wait=N`, N above 0) holds one of the user's JS10 stream slots while it waits, so past `jobs.streams_per_user` it is `429 too_many_streams`, a submission's before any row; JS8's enforcement for a stuck core call (one that never reaches a check of its flag) is PR 8's.
6. **Review, import and matching through the API, and the exit's file half.**
   - **Contents:** reviews and imports, the import a `socket_import` job that verifies or marks (OB5, UP6, RA6, RA7, RA10, M6-14, M6-62); the parse semaphore (HX8's first half); match jobs over uploads and components, originals, policies and options, the gate checked again at dispatch (JB2, JS7, DC3, DC7, SN1 to SN9, SN11, RA8, RA9); CO2, CO3 and CO5, with the CLI on CO2 (CU3's match half); notes (JB10); `fitment.run` (DC10), a null `outcome` for every job that did not succeed (M6-59), and the sliced candidates view (DC12); a retained match's inputs (M6-64); ownership complete (AU8).
   - **Tests:** VT1 (all of it but the catalog: XH1 parts 1 to 4, XH3 to XH7, XH9 to XH11); VT3; VT7's `review`, MF11, `match` and `--original` rows; VT22; VT25 in full; VT31; VT32's socket half and its dispatch re-check; VT37's CO2 and CO3; VT40; VT41 for matches; VT44; VT45's parse half; VT46; VT47; VT49's re-run of a retained match; VT50; the `server-corpus` job.
   - **Done when** every corpus match case, under both policies, file and component candidates alike, passes the strict gate through the API with documents byte-identical to the CLI's (or the library's, for components), the API's false negatives equal the baseline, and `api-report.json` equals `report.json`.
   - **As built:** (left for the PR.)
7. **The catalog through the API, and the exit's catalog half.** Expected in two parts:
   - **7a, the library and the CLI:** CA1 to CA5, CA8 and CU1 to CU3 in `fitment-catalog` and `fitment`, each existing output unchanged but for CA4's recorded lines. **Tests:** VT38, VT39.
   - **7b, the API:** the catalog routes (RA1, RA2, CX1 to CX8, M6-50's synchronous delete), the catalog's jobs and catalog match jobs (full, named, exhaustive), the writer role and cap for exhaustive runs (JS12, M6-60), the run record's generation and counts (M6-25), `reindex_on_start` (M6-28), `limit=all` built in its snapshot (VN3, HX8's second half), catalog ETags with the instance id (VN5, M6-63), JS8 and XH12 for the catalog's kinds, VN4, SN10. **Tests:** VT2; VT7's catalog rows; VT12; VT14 on catalog matches; VT15 with the replay oracle; VT16; VT32's catalog half; VT41 for catalog jobs; VT42's `412` and catalog ETags; VT45's catalog half.
   - **Done when** XH8 holds on the corpus under three policies, `verify --deep` is clean after it, and the CLI's documents of 0.2.0 commands are byte-identical.
   - **As built:** (left for the PR.)
8. **Hardening.**
   - **Contents:** VT18 and VT19 in full; VT26 and `fuzz.yml`; VT27 and VT28 in `scale.yml`; the weekly restart soak and the tmpfs leg (VT10, VT11); VT14 on the corpus; XH14, once every operation exists (VT23's matrix, VT6's goldens and VT26's seed corpus each covering every operation); JS8's enforcement for a stuck core call, a call that never reaches a check of its flag (deferred from PR 5's review); the reviews' findings.
   - **Done when** XH14 holds, every rule of `M6_LIMITS` has a test that exceeds its bound, and each weekly workflow has run green once (linked in the "As built" note), with the first scale and load numbers recorded.
   - **As built:** (left for the PR.)
9. **M6's exit.**
   - **Contents:** the registry gate on (no `"M6 PR"` deferral; `M6_LIMITS`, `M6_PARITY` and `M6_FAULT` filled and passing); the exit's numbers from `api-report.json`; section 18's changes to PLAN.md, ARCHITECTURE.md, the README (OP10's notes among them), m4-match.md and m5-catalog.md; the CHANGELOG's unreleased section.
   - **Done when** PLAN's exit holds: the M4 corpus passes end to end through the API, by XH1 to XH11, with the numbers stated.
   - **As built:** (left for the PR.)
10. **The release, 0.3.0.**
    - **Contents:** every crate at 0.3.0; `fitment-core`, `fitment-catalog` and `fitment` packaged and checked by `check-core.sh` (no STEP file, a LICENSE in each); `fitment-server` and `fitment-corpus` unpublished (`publish = false`, and `dist = false` for the server, checked: M6-35); `cargo package --workspace` green; the CHANGELOG's `[0.3.0]`; `image.yml` for the user to push the image from the tag.
    - **Done when** the packages build from their `.crate` files and the image builds from the release commit. The publish (`fitment-core`, then `fitment-catalog`, then `fitment`), the tag `v0.3.0` and the image push are the user's.
    - **As built:** (left for the PR.)

**Deferred to M7 and M8.** M7: a real `FitVerifier` in the server (OP9), `Match` through the API for real, and the `collides` cases' `Reject` (`interferes`) gated through it; the fit helper's process model inside or beside the container, which ships no Open CASCADE (D1); D7's `warning` as a policy; QR13's sanctioned blob read for fit requests; a fit job's own faults added to XH12's tests. M8: the viewer's routes, sessions for a browser, and their tests. Not planned in M6: TLS inside the server, several server nodes on one catalog, Postgres, single sign-on, quotas beyond UP4's, real ground truth (D8, D9), a tolerance sweep through the API (KC14's stays a library report), the environment bootstrap (M6-19's alternative).

## 13. Limits and failure modes

### 13.1 Limits

Every limit is a configuration key (OP3), stated in `fitment.info` where a client needs it (DC11).

| Limit | Default | Where |
| --- | --- | --- |
| Upload (STEP) | 128 MiB, = the catalog's SZ1 | UP2 |
| Socket document read from an upload (imported, matched or carried from) | 16 MiB | UP6 |
| JSON request body | 1 MiB | HX2 |
| Candidates in a file match; ids in a named run | 10,000 | HX2 |
| Live entries for an exhaustive run | 100,000 | JS12 |
| Label or `input` / `file_name` | 1,024 bytes / 255 bytes | RA8 |
| `Idempotency-Key` | 128 printable ASCII characters | JB9 |
| Page size | 100 by default, at most 1,000 (`all` for catalog entries) | VN3 |
| `Prefer: wait`, `?wait` | 60 s | JB11 |
| Headers and request line | 64 KiB, read within 10 s | HX1 |
| Connections | 1,024 | HX1 |
| Concurrent uploads | 8 (2 per user) | UP4 |
| Upload throughput | first bytes within 5 s; then at least 64 KiB/s for every second until 30 s, and averaged over any 30 s after | HX3 |
| Live uploads per user; a quota's charge per object | 10,000; its bytes, at least 4,096, and 512 | UP4 |
| Socket documents parsed at once / `limit=all` listings at once | 4 / 2 | HX8 |
| Staging space / per-user quota (uploads, retained jobs' documents and inputs) | 8 GiB / 10 GiB | UP4 |
| Upload lifetime | 24 h | UP5 |
| Non-streaming handler | 30 s | HX3 |
| CPU permits | `available_parallelism()` | JS2 |
| Match threads per job | 4 | JS1 |
| Memory budget (heuristic); a catalog match's estimate | 4,096 MiB; 64 MiB + 16 KiB per selected entry | JS3 |
| Queues: query / write / maintenance | 64 / 16 / 4 | JS5 |
| Per owner: running / queued | 2 / 16 | JS4 |
| Deadlines: extraction and import / match / verify / writes | 10 min / 60 min / 60 min / none | JB7 |
| Job retention | 30 days (pinned: kept) | JB12 |
| NDJSON streams | 8 per user, 256 in all | JS10 |
| Progress events | at most 10 a second per job; broadcast 1,024 | JB8, JS10 |
| Password | 12 to 256 characters, at most 1,024 bytes | AU1 |
| Concurrent argon2 | 2 | AU2 |
| Token lifetime | 90 days by default, 365 at most; a minted token's at most its parent's | AU4, AU10 |
| Live tokens per user | 20 | AU10 |
| Tokens minted per user | 10 a minute | AU10 |
| Login | 30 per address per minute; 10 failures per (username, address) per 15 min; 10,000 buckets | AU7 |
| Requests per user | 50 a second, burst 100 | AU7 |
| Catalog busy timeouts: job writes / sync delete / reads | 60 s / 10 s / 5 s | CX3, CX4 |
| WAL barrier | at 256 MiB; pauses dispatch at most 60 s | CX6 |
| Drain on shutdown | 25 s for jobs, 10 s for requests | SD1 |

### 13.2 Failure modes

| # | Failure | Detected by | What happens | The client sees |
| --- | --- | --- | --- | --- |
| F1 | Another process holds the catalog's write lock | the write job's 60 s busy timeout | the job waits, then fails; the lane waited with it | job `failed`, `catalog_busy`, retryable |
| F2 | A catalog read meets `SQLITE_BUSY` (WAL recovery, a TRUNCATE) | the reader's 5 s busy timeout | the request ends | `503 catalog_busy`, `Retry-After: 1` |
| F3 | The catalog is stale after an upgrade | `NotRun::Stale` (RX3) | matches refused; a reindex queued (M6-28) | job `refused`, `catalog_stale` with the counts; `fitment.info`'s flag |
| F4 | A catalog of a newer schema | `Error::Newer` at start | the server does not start | exit 3 (as DB5), logged |
| F5 | The catalog directory missing, not a catalog, or linked | `NotACatalog`, `Linked` at start or open | start refused; at run time `catalog_unavailable`, the CLI's exit 2 (ER2) | readiness `503` |
| F6 | A damaged stored row in a run | QR4 | the document kept, the job failed with the corrupt ids | job `failed`, `run_incomplete`, `complete: false` |
| F7 | A blob or object whose bytes are not its name | ST11, OB3 | nothing sent or indexed | `500 blob_damaged`, or the job `failed`, `blob_damaged` |
| F8 | The disk is full: catalog | SQLite or `Fs` error; SZ5's quota | the batch (≤ 64 files) rolled back | job `failed`, `storage_full` |
| F9 | The disk is full: state | write error | the upload's temporary file removed; a job's document not stored, the job failed | `507 storage_full`, or job `failed` |
| F10 | An upload over the cap | `Content-Length` or the count | the temporary file removed | `413 too_large` |
| F11 | A client aborts an upload, or stalls | stream error, 30 s idle | the temporary file removed | — |
| F12 | Uploaded bytes changed between upload and ingest | CA1's SHA-256 differs from the upload's | the job fails, nothing committed for that file | job `failed`, `internal` |
| F13 | A queue is full | JS5 | not enqueued, no row | `503 queue_full` or `429 too_many_jobs`, `Retry-After` |
| F14 | A job past its deadline | JB7 | stopped at its next check; no document | job `failed`, `deadline_exceeded` |
| F15 | A panic in a handler | `CatchPanicLayer` | logged with the request id | `500 internal` and the request id |
| F16 | A panic in a job | `JoinError`; the catalog's per-group `catch_unwind` | the job fails, permits released | job `failed`, `internal`; a catalog bug is listed (`run_incomplete`) |
| F17 | The process is killed (OOM, SIGKILL) | the next start | SD4: a result already committed kept (JS13), other pure jobs re-run once, writes `interrupted` without a document (JB6) | the jobs' states after restart; a write job's generations at start and at recovery |
| F18 | `server.db` of a newer schema, or not the server's | `application_id`, `user_version` | start refused | exit 3 for a newer schema (as DB5), 2 for another application's |
| F19 | A second server on the same state directory | the lease (SD6) | start refused | exit 2 |
| F20 | A lost or slow NDJSON reader | broadcast lag | the job unaffected | a `lagged` event |
| F21 | A login flood | AU2's two-hash queue, AU7 | refused | `503 request_timeout`, `429 rate_limited`; `invalid_credentials` timing unchanged |
| F22 | An expired or revoked token, a disabled user | AU4 | refused at once | `401 unauthenticated` |
| F23 | The WAL pinned by overlapping matches | CA3 (CX6) | the barrier: queries paused at most 60 s, TRUNCATE | a bounded queue delay; `fitment.info`'s flag |
| F24 | Slowloris headers | `header_read_timeout` | the connection closed | — |
| F25 | Too many connections | the connection cap | new ones closed at accept | a connection reset |
| F26 | The clock jumps | — | affects expiry, retention and rate limits only, never a document | — |
| F27 | A catalog on NFS or in a sync folder | not reliably detectable; WAL checked at open (CC5) | unsupported, documented | — |
| F28 | The fit helper missing when configured (M7) | the startup probe | start refused, never a silent `none` | — |
| F29 | A delete while a long ingest runs | CX3 | the delete waits for the current batch, not the job | `200` after at most about a second |

## 14. Risks

- R1. **A shared bug** (C R-C1). The server calls the same core as the CLI, so parity cannot see a bug they share. *Mitigation:* XH5's truth from the construction on every case, and by byte equality every oracle check of the CLI's documents (section 9.1).
- R2. **Parity defeated by serialization** (C R-C2, B). A handler that re-serializes a document through `serde_json::Value` sorts its keys; a dependency that turns on `preserve_order` or `arbitrary_precision` changes how every document reads and writes. *Mitigation:* XH4 fails on either at the first byte; `deny.toml`'s exact features for serde_json (`[graph] all-features = true`) refuse both in any build; SV7's clippy ban; SV9's runtime-graph check, now that `alloc` is allowed for schemars.
- R3. **The parity on the server's own catalog is circular** (C R-C3) if the server's ingest wrote a wrong row: both sides read it. *Mitigation:* XH8 also compares digests with a catalog the CLI ingested itself, compares named runs with file runs over the original bytes, and ends with `verify --deep`, which re-reads every stored file.
- R4. **Flaky timing tests** (C R-C4). Slow-client, restart and load tests depend on time. *Mitigation:* hook points and barriers order events (OP11); a timeout under test is set small by configuration (OP3) and asserted with a generous slack; load is reported with only an order-of-magnitude gate (M6-45); no retries.
- R5. **Windows** (C R-C5). No signals, unlinks blocked by open files, `TerminateProcess` not `abort`, directory syncs impossible (M5's R7). *Mitigation:* VT10's restart runs on Windows at the same hook points by `TerminateProcess`; shutdown by Ctrl-Break or, in tests, stdin's end of file (SD1); the soak and tmpfs legs are Unix-only and say so.
- R6. **The exit's run time** (C R-C6). The CLI work is about 20 s on the Mac; GitHub's 4-core runners are slower, and HTTP and job overhead add to it. *Mitigation:* one server for the whole exit, one match job per socket and policy, named runs only for the catalog half; measured in PRs 6 and 7, with a 5-minute budget for `server-corpus`'s test.
- R7. **Moving the scorer breaks the M4 and M5 gates silently** (C R-C8). *Mitigation:* PR 1 alone, with no server; VT5's byte comparison before and after; the coverage test's citations updated in the same PR.
- R8. **Goldens churn with the schema generator** (C R-C9). *Mitigation:* `Cargo.lock` pins schemars; a golden changes only with a version rise; a schemars upgrade is its own PR with its own bless.
- R9. **The container job's cost and registry limits** (C R-C10). *Mitigation:* a distroless base from a registry that does not rate-limit pulls, the layer cache keyed on `Cargo.lock`, the job on Linux only.
- R10. **cargo-deny refuses a versionless dev-dependency of a published crate** (C R-C11). *Mitigation:* PR 1 tries `allow-wildcard-paths` on the pinned 0.20.2 before anything depends on it.
- R11. **The generated corpus is still fitment's own generator family** (C R-C12; M4's 27.1, D8). Through the API as through the CLI, the false-negative rate is measured against it. *Mitigation:* none in M6; real catalog parts remain the open item they were at M5.
- R12. **Memory accounting is a heuristic** (B 15 item 5; JS3). An estimate of ten times the file's bytes can be wrong for a pathological file, and a catalog match's 16 KiB per selected entry is unmeasured. *Mitigation:* PR 7 measures the per-entry figure at 10⁴ and VT27 at scale, and the key is configuration; the container's memory limit is the real guard, SD5 bounds a crash loop to two tries, and VT27 reports peak memory; if the scale runs show the estimate wrong, fewer `query_slots`. A real bound would need core to report its graph's size, out of scope.
- R13. **Ingest is serial** (B 15 item 4). `fitment-catalog` gates files one at a time, about 7.9 ms per real part (M5-3), so 10⁴ parts take about 80 s on the one write-lane permit, and a large ingest delays other writes (never queries). *Mitigation:* none needed for M6's exit; a later CA could gate a batch on several threads and commit in order.
- R14. **A JSON Schema validator for VT36** (merge). Validating every document needs a validator, and the usual crate (`jsonschema`) brings many dependencies whose MSRV and licences are unchecked. *Mitigation:* PR 2 tries it as a dev-dependency under 1.85 and `cargo deny`; failing either, a small validator in `fitment-corpus` for the keywords schemars emits (`type`, `properties`, `required`, `additionalProperties`, `items`, `enum`, `const`, `oneOf`, `anyOf`, `$ref`), with VT36's controls showing it refuses (question 9).
- R15. **Audit records expire** (merge; M6-23). PLAN wants a match re-runnable and audited; a run record unpinned for 30 days is gone. *Mitigation:* an admin pins what must be kept; while retained, a record holds its socket revision and every candidate's bytes and its owner can name them again (OB4, UP3, M6-64), and the catalog keeps a revision's bytes until `prune` (M5-41). Question 5 asks whether 30 days is right.
- R16. **A derived socket id names data the caller may not hold** (merge; RA4). Ids depend on the data alone, so one user can compute the id another user's line would have. *Mitigation:* AU8: an id resolves only among the caller's own lines; VT25 shows a computed id of another user's line is `404`.

## 15. Attacks and their fixes

How could the API pass a part that does not fit, report what the CLI would not, lose or leak a user's data, or let the exit pass while the API is wrong? Each designer attacked their own area; their findings are merged in 15.1, each once, with where it came from: A's rules, B's sections and failure modes, C's attack numbers. The merge's own findings are in 15.2. A soundness review then attacked the merged design; its findings, as the coordinator resolved them, are in 15.3, and rows of 15.1 and 15.2 whose fix it changed say so.

### 15.1 The designers' findings

| # | Attack | What would go wrong | Fix | From |
| --- | --- | --- | --- | --- |
| 1 | The API answers with the CLI's document because the test handed it the CLI's file | Nothing would be tested | XH2: inputs only as request bytes; the test never writes the server's directories | C 1 |
| 2 | A job answers with another job's document (an id mix-up under concurrency) | A wrong verdict for the right candidate | XH3.6: the document's socket digest and sources must be the job's own inputs; VT14 and VT15 interleave jobs | C 2 |
| 3 | A policy field ignored, the default applied | `functional` runs judged strict; only the two `fits_functional` cases would show it | KH2.6 through XH3.6: the document states the policy requested; DC7 builds it as the CLI does | C 3 |
| 4 | A review applied to the requirement of the same id in another socket | A requirement removed that the user kept | XH3.3: the socket document equals `fitment review`'s, byte for byte; decisions name `socket` | C 4 |
| 5 | The scorer finds an entry by its label, which a client sets | A good file's verdict scored for a bad case | SC9: entries by SHA-256 | C 5 |
| 6 | A job that timed out or failed reported as `reject` | "Nothing fits" read as an answer: a user buys a new part | JB3's states, XH7, XH12, VT22's deadline case. The review found a failed run's envelopes still stating `outcome` (15.3, F2): null unless succeeded, VT44 | C 6, A JB3 |
| 7 | A result cut short by a full disk stored as succeeded | A partial list of candidates | XH11's checker on every document (summary, order, digest recomputed); OB5: the document is stored before the row; VT11 | C 7, B F9 |
| 8 | A restart re-runs a finished ingest, or marks a half-finished one done | Duplicate revisions, or parts missing from every search | JB6: write jobs `interrupted`, never re-run; VT10: `list` bytes equal an uninterrupted ingest's; `verify` clean | C 8, B M6B-8 |
| 9 | A cache keyed too narrowly survives a replace | The old revision judged under the new label | No cache (M6-13, GT9); VT15's replace then match | C 9, A A12 |
| 10 | The exit skips its slow half in CI | A green exit that ran nothing | XH10: `FITMENT_REQUIRE_CORPUS=1` and the run count asserted | C 10 |
| 11 | The 10 component-only sockets quietly left out | A third of the sockets never through the API | XH1's counts per kind; M6-5 | C 11 |
| 12 | The API's false negatives blessed into the baseline | A new miss accepted silently | SC5: the API suite cannot write it | C 12 |
| 13 | A float written differently by the server (a `Value` round trip, another serde_json feature) | A value at a tolerance edge reads back one ulp away in a stored report | DC1, XH4's byte parity; `deny.toml`'s exact features; SV9 | C 13, B DT2 |
| 14 | `Match` reported without a fit verifier | D1 broken through the API | SN11, VT31; the exit report's `match` column is 0 | C 14 |
| 15 | A route added without a role | An anonymous write | AU5's extractors; XH14: no route outside `OPERATIONS`, no operation without a matrix row | C 15, B AU5 |
| 16 | An error body carries a path, SQL or a token | Information for an attacker | ER3; CA4; XH13 on every response and log line, VT34's controls | C 16, A ER3, B HX7 |
| 17 | The coverage gate satisfied by naming any test | A rule "covered" by an unrelated test | Review, as before; `M6_LIMITS`, `M6_PARITY` and `M6_FAULT` require the test to live where that kind of test lives | C 17 |
| 18 | Two designs give one id two meanings | One `RULES` entry covers two rules | Section 11.1's duplicate-id check; the prefixes of this document checked unused | C 18 |
| 19 | A server path in a catalog document: ingest from a staging path | `changes[].input` and error lines name `objects/sha256/…` on the server | CA1: bytes with display names; DC2 | A 14, B CA1 |
| 20 | A client label `catalog:BRKT@r1` on an uploaded file | The file passes for a catalog entry's revision in a stored report | RA8: `label_reserved` | A RA8, SN11 |
| 21 | A stored `fitment.match` or a digest accepted as proof | A forged or stale verdict re-served | SN6: no endpoint reads a report or compares a client digest to decide | A SN6 |
| 22 | A refusal skipped by a request field (`force`, `skip_stale`) | A stale socket or catalog judged: a false positive | SN2, SN3: no override, the server may only tighten | A SN2 |
| 23 | A partial match document after a cancel | A verdict over a set nobody asked for | JB5, SN7: no document; M6-51: no verdict on the stream either; after the review, no `outcome` in any envelope (15.3, F2) | A JB5, B DT6 |
| 24 | Two reviewers of one socket | One decision lost, an unreviewed requirement left standing unseen | RA7: a review names `@rN`; the second is `409` | A RA7 |
| 25 | `HEAD` or the upload status as a server-wide oracle | Anyone learns whether a given file was uploaded by anyone | UP3: per-user rows, the same answer for another user's bytes, the body streamed either way. The review found the re-hash of an existing object a timing oracle (15.3, F4): UP8, the same work either way | B OB4 |
| 26 | Login timing reveals usernames | An attacker enumerates accounts | AU2: a dummy hash for unknown and disabled users | B AU, audit flaw 1 |
| 27 | A token in a log or a URL | Its holder's access stolen | AU3: bearer header only, stored as SHA-256, `Secret`'s `Debug`; OP1's sensitive headers; VT24 | B AU3 |
| 28 | Job churn in the catalog's database | The generation races, writes queue behind ingests, the WAL grows, the 0.2.0 CLI refuses a migrated catalog | M6-3: `server.db` apart | B M6B-1 |
| 29 | A hostile file that kills the process, re-queued forever | A crash loop | SD5: `attempt` persisted, two tries | B SD5 |
| 30 | A long match pinning the WAL while ingests write | Unbounded WAL growth | CX6's barrier and checkpoints, the barrier bounded after the review (15.3, F3, F23) | B CX6 |
| 31 | One user's long jobs starve another's | A denial of service by one account | JS4's per-owner caps and round-robin; JS5's bounded queues; after the review, rate, upload and stream caps per user, not per token (15.3, F5) | B JB8 |
| 32 | A slow client holds a connection or a thread | Resource exhaustion | HX1's header timeout and cap; HX3's chunk and upload timeouts; streams that never block a job (JS10) | B HX1, HX3 |
| 33 | A request names an object by hash it never uploaded | Another user's bytes judged or downloaded | UP3, AU8: names resolve among the caller's own rows | B OB4 |
| 34 | Two catalogs mixed into one server | Labels and generations of two catalogs in one record | RA1: one catalog per server | A A6 |

### 15.2 The merge's own findings

Reading the three designs against each other and against the coordinator's decisions found these. Each changed a rule or a decision above.

| # | Finding | Fix |
| --- | --- | --- |
| 1 | A and B both used `JB`, for different families | A's keeps JB (what a client sees); B's scheduling rules are JS; rules both stated (states, cancellation, idempotency, progress) are stated once in JB, citing both |
| 2 | B's DT rules restated A's DC and SN rules from the other side | DT folded into DC1, DC2, DC3, DC8, SN1 and SN7, each citing both |
| 3 | A's progress stream carried each candidate's verdict; B's JB16 forbade any verdict in an event. A cancelled or failed match would have shown verdicts with no document | M6-51: counts only; JB8, SN7; VT43 scans every event |
| 4 | A deadline ended a job `failed` with `job_timeout` (A) or `cancelled` with `deadline_exceeded` (B); C's VT22 expected "fails with the timeout code" | M6-49: `failed`, `deadline_exceeded` (JB7) |
| 5 | A's `DELETE` was synchronous; B made every catalog write a write-lane job. A synchronous wrapper around B's job would wait behind a whole ingest and time out | M6-50: a synchronous delete runs one transaction on its own writer, waiting at most one batch (CX3, F29) |
| 6 | The coordinator's state mapping names exit 3 (`refused`) and exit 1 (`failed`) only; exit 2 arises inside jobs (a selection error, an ingest conflict, a reference that changed between request and job) | M6-52: `refused`, `exit_code` 2 kept |
| 7 | `Prefer: wait` allows 60 s, but B's handler timeout is 30 s | M6-53: a wait is not handler work (JB11, HX3) |
| 8 | A's derived socket ids are global; B's resources are per owner, and decision 7 says knowing a hash grants nothing. A global id derived from an assembly's hash would let anyone who knows the hash read the socket | RA4: ids derived from the data, resolved in the owner's namespace (AU8); R16, VT25 |
| 9 | M6-11 puts times only in `fitment.job`, but a token's expiry is what its owner must see | `fitment.token` also holds times; no result document, run record or other envelope does (M6-11, DC9). The coordinator should confirm (question 10) |
| 10 | M6-6's schemars derives are behind features that `deny.toml`'s `all-features` graph turns on, so cargo-deny sees `alloc` through the normal graph, not only the dev graph | SV9: `alloc` allowed with its reason; a `check-core.sh` check that the server's default normal graph has no schemars and serde_json's three features exactly |
| 11 | A named uploads by `sha256:`; B by upload ids, so a hash would never be a capability | Requests name `sha256:HEX`, resolved among the caller's own rows (UP3), which gives B's guarantee with A's names |
| 12 | A's uploads deduplicated across the upload, catalog and assembly stores, and its `HEAD` reported presence in any | The catalog's blobs are another store; uploads deduplicate among server objects only, and answer only for the caller's rows (UP3) |
| 13 | A's viewer could not upload or run a match; B's reader can. Reindex was A's admin, B's writer | Decision 8: B's scopes (AU5); reindex is a writer's |
| 14 | C's assumption 8 and PR 4 put stored inputs in the catalog at schema 2 | M6-3: `server.db`; PR 4 builds the object store, not a migration; XH8 unaffected |
| 15 | C's PR 4 tested `parts` and `features` rows, which are jobs under M6-1 and arrive with the job system | Those rows of VT7 move to PR 5 |
| 16 | A's health and readiness (`/v1/health`, `204`) and B's (`/healthz`, `/readyz`, `200` with JSON) differed | A's paths and statuses; B's readiness conditions; B's flags in `fitment.info` (OP5, OP7) |
| 17 | A's job ids were ULIDs (time-ordered), B's random | M6-55: random ids; lists by (creation time, id); the cursor holds no internal key |
| 18 | B's kinds lacked `features` and `assembly_read`, and had `delete` | A's kinds; the two in the query lane (JS1); delete synchronous (M6-50) |
| 19 | B's jobs need an owner, but `reindex_on_start` and the daily gc have none | M6-56: a `system` user that holds no token |
| 20 | B's logs on stdout conflict with C's address as stdout's first line | M6-54: logs on stderr, one line on stdout |
| 21 | Code names differed (A's `catalog_corrupt`, `storage_full`, `job_interrupted`, `rate_limited` reserved; B's `catalog_damaged`, `insufficient_storage`, `interrupted`, `bad_name`, `bad_parameter`, `timeout`, `409 idempotency_key_reused`) | One registry (section 7.12): `catalog_damaged`, `storage_full` and `quota_exceeded`, `job_interrupted`, `name_invalid`, `invalid_reference` or `not_found`, `request_timeout`, `422 idempotency_key_reused`, `rate_limited` enforced (M6-30) |
| 22 | B's upload rows held a display name used as the default label; A's file labels were per request, and a label like `cand/c00_original.stp` fails B's name checks | M6-57: uploads carry no name; labels per request with A's rules; `file_name` with B's (RA8) |
| 23 | C-2 accepted labels-stripped digests for components | The test hands the library the API's label, so components are compared byte for byte (M6-37, XH3.5) |
| 24 | A's request had a `grip` policy field; the CLI has no grip flag (D7) | Dropped (DC7) |
| 25 | A kept socket revisions and run records forever; B retained runs 30 days | M6-23: runs 30 days unless pinned; a deleted line's revisions kept while a retained run cites them (RA6, `job_cites`). After the review, one hold window, and a match's inputs kept with its record (15.3, F20; M6-64) |
| 26 | C's Windows shutdown needed a trigger tests can send without unsafe code | Under `test-support`, stdin's end of file starts SD1; production Windows uses Ctrl-C, Ctrl-Break and close events |
| 27 | A's `catalog_stale` was an HTTP `409`, but A's own JB2 runs QR1 only inside the job | A job-only code (`—` in section 7.12), the job `refused` |
| 28 | A `verify` that finds problems exits 3, which the state mapping makes `refused` | Stated in JB3's mapping and section 7.13, so a client reads `refused` with the `fitment.verify` document as "problems found", not "verify could not run" |

### 15.3 Found by the review, fixed so

The soundness review attacked the merged design and listed twenty-five findings, three of them high. The coordinator resolved each; the fixes are in the rules, decisions, tests and plan above. Findings are numbered as the review numbered them, F1 to F25, and the text cites them as "review F*n*", apart from section 13.2's failure modes F1 to F29. Its eight questions are answered in section 17.

| # | Finding | What it was | Fix | Rules and decisions | Tests |
| --- | --- | --- | --- | --- | --- |
| F1 | Tokens could outlive and outrank the token that minted them (high) | `POST /v1/auth/tokens` capped a new token at its user's role and `token_max_days` only, so a stolen reader token, or one about to expire, minted a year-long writer token; revoking the stolen token left what it minted; a password change revoked nothing | A minted token's role is at most the presenting token's effective role and its expiry at most the presenting token's; it records its `parent`; revoking a token revokes every token minted from it; a password change revokes every other token of the user, an offline reset all of them | AU4, AU5, AU10, section 7.10, `token.parent`; M6-58 | VT24's minting, cascade and password cases; VT23 |
| F2 | A failed run read as a verdict (high) | A failed match keeps its document, and `fitment.job` and `fitment.run` copied its `summary` and `outcome`: a run that exited 1 after rejecting what it had judged said `outcome: reject` in its envelope, its listing and its `end` event, which XH12 forbids; candidate pages did not say the run was incomplete | `outcome` is null for every job that did not succeed; what a failed run judged is under `problems.incomplete_summary`; run listings and candidate pages carry `complete` and `exit_code`; XH12 restated, its third invariant tested on every way a job can end without succeeding | JB3, JB8, SN4, DC10, DC12, XH12, sections 7.8, 7.11, 7.13; M6-8, M6-59 | VT44 (with its control), VT46 |
| F3 | Catalog-scale work was open to every reader (high) | A reader could run exhaustive catalog matches (111 MB for 9,219 candidates in M5) counted as a flat 512 MiB whatever the catalog's size; every candidates page re-parsed the document; job documents counted in no quota; synchronous socket parses of up to 64 MiB ran without limit; the WAL barrier could pause queries for a match's whole deadline | Writers only for exhaustive runs, a catalog-wide reindex and `verify`; a memory estimate per selected entry; `max_catalog_candidates` (100,000) refuses larger exhaustive runs; the candidates view sliced through a byte-offset index built at the job's end; job documents in the owner's quota; parsing behind a semaphore, `max_socket_mib` 16; the barrier bounded at 60 s | AU5, JS3, JS12, DC12, UP4, UP6, HX8, CX6; M6-8, M6-17, M6-60 | VT45, VT46, VT23 |
| F4 | The upload path was a timing oracle | OB2 re-hashed an existing object before reuse, so uploading bytes another user held took measurably longer than uploading new bytes, undoing UP3's "neither the status nor the time tells" | No re-hash of an existing object on the upload path (OB3's re-hash on every read stays); UP8 states that both branches do the same observable work | OB2, UP3, UP8 | VT48 (with its control) |
| F5 | Caps per token, tokens and login buckets unbounded | The request rate, uploads and streams were capped per token, and a user could mint tokens without limit to multiply them; a trickled upload held a slot for 30 minutes; the per-username login buckets grew without bound, and their lockout let anyone lock the admin out | Caps per user; at most 20 live tokens per user; uploads at least 64 KiB/s after their first 30 s; login buckets keyed by (folded username, address) for every username and by address, at most 10,000, least recently used evicted | AU7, AU10, UP4, JS10, HX3, section 7.12 (`upload_too_slow`, `too_many_tokens`); M6-61 | VT24's per-user and bucket cases, VT18 |
| F6 | Transient events were numbered | Every event took a `seq`, progress and heartbeats included, though only persisted events can be replayed; the job's ETag, `W/"j<seq>"`, changed with every progress event, so polling with `If-None-Match` never saw a `304` | Transient events carry no `seq`; persisted events alone are numbered; the job ETag is the persisted `seq` and the state | JB8, VN5 | VT43, VT42 |
| F7 | Library text in catalog documents | A batch that could not commit, and a reindex line's error, carried the error's `Display`, which for an I/O or SQLite error can hold a path or SQL, into `fitment.catalog`, a document the API serves byte for byte | That text is built from the error's kind (CA4 widened); the CLI's text changes with it, recorded in CU3 and VT39 | CA4, CU3, DC2; M6-48 | VT38, VT39 |
| F8 | The leak scan contradicted the logs | XH13 failed any log line holding an absolute path, though CX7 and OB3 log paths for the operator | Responses are scanned for paths; logs for secrets only; logged paths are relative to the configured roots | XH13, OP1, CX7 | VT34's controls |
| F9 | Ingest parity compared generations | DC3 claimed the API's ingest gives the CLI's `fitment.catalog` "generations included", and VT10 compared `list` bytes after a resubmitted ingest, but the server's catalog counts writes the CLI's never saw (a reindex at start, a daily gc, a delete, an interrupted batch) | Change lines and entries compared with every generation masked, said in DC3, VT7 and VT10 | DC3 | VT7, VT10 |
| F10 | Interrupted write jobs promised too much | JB3 gave an interrupted write job "its committed changes", but after a hard kill the change lines were only in the dead process's memory; `job_interrupted` was retryable for a replace or a prune, one transaction that may have committed | No document after a hard kill; `error.details` give the catalog's generation at the job's start and at recovery; `job_interrupted` not retryable for a replace or prune, whose client reads the entry or the catalog first | JB3, JB6, ER6, section 7.12, `job.catalog_generation`; M6-22 | VT10 |
| F11 | A crash between a result and its end | An extraction or an assembly reading committed its revision or parts row and then its terminal state; a crash between them re-ran the job, which made a second revision or reported `unchanged` for its own | The rows commit with the job's terminal state in one transaction, each naming the job; a re-run first looks for its job's committed result | JS13, JS6, JB6, SD4, `assembly_parts.job` | VT10's new hook points |
| F12 | An import could forge a socket | `POST /v1/sockets/import` stored any well-formed, MF2-passing document as a revision of the line it names, so a hand-edited socket with its requirements loosened and its digest recomputed passed for an extracted one, and run records cited it as the assembly's | The import is a job that re-extracts from the caller's assembly and requires an equal digest (`socket_not_reproduced` otherwise); without the assembly the revision is `verified: false`; `how` and `verified` in revision listings and `fitment.run`; a run on an unverified socket says so | RA10, RA5, JB1, JS1, JS6, section 7.6, `socket_revision.verified`; M6-1, M6-14, M6-62 | VT47 |
| F13 | Components were judged off core's path | A component was to be judged by `judge_part` on a part the server read and selected itself, a second reading beside the CLI's, with nothing said of a component in a refused assembly or of an original that could not be read | `Matcher::judge_component(&ComponentRead, source)` in core; a selection in a refused assembly is refused, exit 3, `component_refused`; an unreadable original is `run_incomplete`, exit 1 | CO5, SN1, SN9, DC3, XH3, section 7.8; M6-5, M6-48 | VT50 |
| F14 | The store had no fault seam | Uploads and object reads used files directly, since the catalog's `Fs` has no streaming calls, so the seam could not fail them; the design named a `FaultFs` that does not exist | CA7 adds a streaming writer (created new, mode 0600, chunks, sync) and reader to `Fs`, so uploads and OB3 go through it; the seam is `CountingFs::failing_at` | CA7, UP2, OB2, OB3, OP11; M6-44, M6-48 | VT11, VT38 |
| F15 | `carry_from` was read strictly | UP6 read every uploaded socket document strictly, MF11 first, "whether imported, carried from or matched", while section 7.6 read a `carry_from` leniently: carrying from an older document, its purpose, was refused as stale | `carry_from` is outside UP6, read through `SocketDocument::prior` within the socket cap | UP6, HX8, section 7.6 | VT7's MF11 row |
| F16 | A tightening counted as parity | Open question 2 said the CLI's `review` refuses a stale socket as the API does; it accepts one, so the API's `422 socket_stale` on a review is a tightening, which a parity row would have asserted against the CLI | Stated as a tightening over the CLI (SN3), kept out of DC3's parity rows; open question 2 corrected | SN3, DC3, section 7.6, section 17 | VT7's review row, VT32 |
| F17 | `is_test` missed the `async fn` | M6-41 accepted the `#[tokio::test]` attribute but not the `async fn` it decorates, so a cited async test would not resolve | `is_test` accepts the `async fn` after either attribute, with a control | section 11.1; M6-41 | VT30's controls |
| F18 | The plan and the registry misplaced rules | XH14 was due in PR 3, before most operations exist; UP5 and OB4 in PR 4, before jobs, assemblies and sockets hold anything; `M6_LIMITS` asked for tests in `hostile.rs`, a file no layer named; JS8 and XH12 were due in PR 5, before the catalog's kinds | XH14 in PR 8; UP5 and OB4 in PR 5; `hostile.rs` added to section 9.4, holding VT17, VT19, VT20 and the limit cases of VT24, VT41, VT42 and VT45; JS8 and XH12 in PR 7, PR 5 checking them for its kinds | sections 9.4, 11.1, 12 | VT30 |
| F19 | Idempotency keyed on resolved references | The key's fingerprint resolved references, so retrying `sk_X` after the line gained a revision was `422`; a key lived with its job, so after an interrupted or cancelled job it could never submit again; a replay always answered `200`; `request_timeout` was retryable where the work may have committed | The fingerprint is over the request as sent, syntax-normalized; a key is released when its job ended `interrupted`, `cancelled`, or `failed` retryably; a replay answers the job's current status; `request_timeout` is not retryable on a write route | JB9, ER6, section 7.12, `job.idempotency_sha256`; M6-13 | VT42, VT45 |
| F20 | Socket lines and holds were inconsistent | `UNIQUE (owner, id)` made extracting a deleted line's triple fail on its tombstone; `job_cites` had no time of writing; three rules gave three hold windows; a file run's uploads expired a day after it, so SN6's "re-run and audited" did not hold | Extracting a deleted line revives it under a partial unique index, its revisions numbered on; `job_input` and `job_cites` written at submission and holding until the job is terminal; one hold window (OB4); a match's inputs kept, and nameable again, for its record's retention | RA4, RA6, OB4, JS11, UP3, UP5, SN6, the schema; M6-2, M6-23, M6-64 | VT49 |
| F21 | The socket gate ran only at the request | JB2 argued a revision never changes, but the build and the `require_reviewed` floor can change while a job is queued, which then ran a socket the running build refuses | MF11, MF2 and the floor re-checked at dispatch; a failure ends the job `refused`, exit 3, with its code | JB2, JB3, JS6, section 7.13 | VT32 |
| F22 | `limit=all` held a snapshot open | The catalog list's `limit=all` streamed from an open snapshot, so any number of slow clients could each pin the WAL for as long as they liked | Built in memory inside the snapshot and sent from memory; at most 2 at once | VN3, HX8 | VT45 |
| F23 | `vacuum` and `verify --deep` shared a lane and the barrier | Both were maintenance jobs taking the barrier, so a deep verify, which only reads, stopped every query for as long as it ran, with no deadline | `gc --vacuum` on the write lane; `verify --deep` takes no barrier and has a deadline | JS1, JS6, JB7, CX6; M6-26 | VT45, VT41 |
| F24 | ETags crossed catalogs and builds | `"g<generation>.<build>"` was the same for two catalogs at one generation, so a `304` could confirm another catalog's list; `features` of a `sha256:` reference and `…/parts` were `immutable`, though another build serves another reading at the same URL | Catalog ETags carry an instance id stored in the catalog's `meta` table (CA8, a catalog change); derived documents are `no-cache` | VN5, CA8, SD3, sections 7.4, 7.5; M6-48, M6-63 | VT42, VT38 |
| F25 | Catalog errors took the wrong exit code | `catalog_unavailable` gave exit 1 for a directory that is not a catalog, is linked or is not Unicode, where the CLI exits 2 for `NotACatalog`, `Linked` and `NotUnicode` | The CLI's codes: 2 for those, 3 for a newer schema, 1 when unreadable | ER2, CX7, section 7.12 | VT33, VT6 |

## 16. What was rejected from each designer, and why

**From A (the API surface):**

- **Upload visibility for every reader, and `HEAD` as a server-wide oracle** (A13): decision 7 (M6-15, UP3).
- **Roles viewer, editor, admin with A's scopes** (12.3): decision 8 (M6-17).
- **Session cookies for the M8 viewer** (12.3): bearer tokens only in M6 (M6-18).
- **aide generating OpenAPI at runtime** (VN7's first form): schemars stays out of the runtime graph (M6-6, SV9).
- **ULID job ids** (VN3): M6-55.
- **Per-candidate verdicts on the event stream** (JB7): M6-51.
- **`job_timeout`** (JB10): `deadline_exceeded` (M6-49); and `succeeded` with exit 2 or 3 (JB3): `refused` (M6-21, M6-52).
- **Every running job `interrupted` at a restart** (JB6): pure jobs re-run once (M6-22).
- **Reports and socket revisions kept forever** (question 3): M6-23.
- **`rate_limited` reserved and not enforced:** M6-30 enforces it.
- **Deduplication across the catalog and assembly stores** (UP3): merge finding 12.
- **`grip` in the policy:** merge finding 24.

**From B (the internals):**

- **A catalog `delete` as a write-lane job** (JB1): M6-50.
- **A deadline ending `cancelled`** (JB12): M6-49.
- **Job inputs named by upload ids** (OB4): A's `sha256:` names, resolved per caller (merge finding 11).
- **A display name on the upload row, used as the default label** (OB5, DT4): M6-57.
- **`/healthz` and `/readyz` with JSON bodies** (OP5, OP7): merge finding 16.
- **JSON logs on stdout** (OP1): M6-54.
- **CA1's path-based `Input`** (`path`, `id`, `name`): decision 12's bytes-based form (CA1).
- **Code names** `catalog_newer`, `insufficient_storage`, `bad_name`, `bad_parameter`, `timeout`, `interrupted`, and `409 idempotency_key_reused`: section 7.12's registry.
- **The environment-variable bootstrap** (M6-19's alternative): left for a later release.

**From C (verification and the plan):**

- **The server's stored inputs in the catalog directory at schema 2** (assumption 8, PR 4): M6-3.
- **`fitment-server` published as internal and unstable** (C-10): M6-35.
- **Vectera's sessions and the viewer, editor, admin roles in the matrix** (assumption 11, VT23, VT24): M6-17, M6-18.
- **Labels-stripped comparison for components** (C-2): M6-37 compares bytes.
- **The `parts` and `features` parity rows in PR 4:** they are jobs, in PR 5.
- **Assumption 9's address as "the first line of stdout" amid logs:** stdout holds only that line (M6-54).

## 17. Open questions

**The merge's questions:**

1. **Socket ids before the first extraction** (A 16.1). A line's id depends on the target definition's `#id`, known only after extraction, so a client cannot compute it in advance. The default accepts this, since `POST /v1/sockets` returns it; the alternative, keying on the selection text, would split `BRACKET` and `#30` into two lines for one part.
2. **Reviewing a stale revision** (A 16.2). **Settled by the review (F16):** the merge said the CLI's `review` refuses an older socket as MF11 does; it accepts one. The API still refuses it (`422 socket_stale`), since a stale revision cannot be matched and extracting again carries its decisions over, and says so as a tightening over the CLI (SN3), kept out of parity (DC3).
3. **The `Prefer: wait` cap** (A 16.5). 60 s may exceed some proxies' idle timeouts; clients should fall back to polling or events. Is 60 s right?
4. **One catalog per server** (A 16.4, M6-7). If several are needed, v2 adds `/v2/catalogs/{name}`.
5. **Retention of run records** (A 16.3, M6-23, R15). Thirty days unless pinned; while retained, a record now holds its inputs and can be submitted again (M6-64). Is that what PLAN's "audited" asks, or should run records (not their large documents) be kept longer?
6. **The problem `type` URI's host** (A 16.6) needs a real documentation location before the API is published.
7. **Ingest parallelism** (B 15.4, R13): not needed for the exit; a later CA.
8. **CO3 without a `MATCH_RULES_VERSION` rise.** Merging repeated components changes no document an earlier build wrote for inputs without a repeat, and no CLI command can repeat one. Confirm that no rise is needed.
9. **The JSON Schema validator** (R14): the `jsonschema` crate as a dev-dependency, or a small validator of schemars' keywords. PR 2 decides by measurement, recorded in its "As built" note.
10. **Times in `fitment.token`** (merge finding 9). The coordinator's M6-11 names `fitment.job` alone; the merge adds a token's times because an expiry is what its owner must read. Confirm, or name another way to show a token's expiry.

**The soundness review's questions, answered** (settled by the coordinator; none remains open):

1. **Where plain `verify` runs.** **Answered:** on the maintenance lane beside the write lane; it only reads, one read transaction at a time, bounded by `jobs.deadline_verify_minutes` (JS1, JB7). `--deep` likewise, without the barrier (review F23).
2. **Admin commands and migrations.** **Answered:** AU9's commands refuse to migrate `server.db` while a server holds the state directory's lease; an upgrade migrates at the server's start, holding the lease (AU9, SD3).
3. **The CLI on the server's catalog in the exit test.** **Answered:** acceptable; it is a test harness, run with the server stopped or idle (XH2).
4. **Who runs catalog-scale work.** **Answered:** writers: exhaustive runs, a catalog-wide reindex and `verify` (review F3; AU5, M6-60).
5. **Whether a graceful drain consumes an attempt.** **Answered:** no; only a crash does (SD1, SD5, JB6).
6. **`allow-wildcard-paths`.** **Answered:** added only if PR 1 shows cargo-deny needs it (sections 9.2, 11.2 and 12).
7. **A proxy's `X-Request-Id`.** **Answered:** accepted from a trusted proxy only if it is at most 64 characters of `[A-Za-z0-9._-]`; otherwise the server generates one (HX7).
8. **An admin's `?owner=`.** **Answered:** on read routes only (`GET`, `HEAD`), never on a job-creating route or another write (AU8, section 7.1).

## 18. Changes to PLAN.md, ARCHITECTURE.md, the README and earlier designs

In PR 9, as built:

- **PLAN.md:** M6 done, with the exit's numbers (XH6's report: runs by kind, false positives 0 under `default` and `functional`, false negatives against physics as the library's, the catalog half's runs and refusals); the Components table's API row as built (`fitment-server`, unpublished, the container image); How it works with the server (a job per long operation, two stores, the server's state apart from the catalog); M5-17's two questions answered (M6-24, M6-25); decision 11's bullet on the server as shipped (a container image, not published); Resume here pointing at M7.
- **ARCHITECTURE.md:** the crate map with `fitment-server` and `fitment-corpus`; a section "Serving the API" from section 3.1's flow, the lanes and the two stores; Soundness (an API document is the CLI's bytes; an error or a stop is never a verdict; no verdict before a document; `Match` only with a verified fit); Determinism (job times and ids outside documents; documents stored and served as bytes); Hostile input over HTTP (sizes, slow clients, names, per-user access); Formats and versions (the API's envelope formats, `fitment.verify` and `fitment.gc`, `server.db`'s schema); the testing table's rows (server layers, parity, the exit, the container, fuzzing, the scorer).
- **README:** running the server and the image; the first admin and tokens; a match over HTTP (section 3.4); TLS at a reverse proxy; both volumes on a local disk, never a network or sync folder (OP10, CC5); the CLI's new `--format json` for `delete`, `prune`, `verify` and `gc`.
- **m4-match.md:** M4-14's promise ("through the API also a leaf of an assembly") marked as built (M6-5).
- **m5-catalog.md:** section 6.3 marked withdrawn, with a pointer to this document's section 6.3 (M6-3); M5-17 amended as built (stored assemblies, sockets and reports in `server.db`; level 1 per run from SQL; a run's context in its record).
- **workspace-layout.md:** decision 9 and the `fitment-server` row amended: unpublished (`publish = false`, `dist = false`), shipped as an image (M6-35); PLAN's decision 11 notes the one recommendation it no longer follows.
- **CHANGELOG:** the unreleased section, with the CLI's lines CA4 changes (CU3), then `[0.3.0]` in PR 10.

## Evidence

Under the session's scratchpad, not committed:

- **`m6-design/A.md`,** designer A's API surface: the resource map, endpoints, documents, error codes, versioning and soundness rules, read against `m5/release`, with section 15's findings in the code.
- **`m6-design/B.md`,** designer B's server internals, and **`m6-proto/`**, its throwaway prototype: `Cargo.toml` (with `Cargo.toml.v1` and `.v2` as dependency variants) and `src/main.rs`, path dependencies on the branch's `fitment-core` and `fitment-catalog`; built with `cargo +1.85 build --release`; `cargo deny check licenses` and `check bans` under the repository's `deny.toml` (section 8.1).
- **`m6-design/C.md`,** designer C's verification and plan, and **`measure.py`**, which timed the CLI work the exit mirrors over `corpus/out` (section 8.2).
- **The repository read for context:** `docs/PLAN.md`, `docs/ARCHITECTURE.md`, `docs/research/m5-catalog.md` (the model for this document's shape), `docs/research/workspace-layout.md`, `docs/vectera-audit.md` (section 3), `crates/fitment/tests/corpus.rs` (`design_rules`, `is_test`, `DEFERRED`, KH14's lists), `tools/gen-corpus.py` (`RULES`, `M5_COUNTS`, `M5_PR`), `deny.toml`, `crates/fitment/src/cmd/match_.rs` and `cmd/catalog/`, `crates/fitment-catalog/src/{ingest,verify,snapshot,query}.rs`, `crates/fitment-core/src/{fit,sha256}.rs` and `matching/{document,matcher}.rs`.
