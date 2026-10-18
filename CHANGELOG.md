# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).
Until 1.0, minor versions may contain breaking changes.

## [Unreleased]

### Added

- **M6, PR 1 (the scorer, the review cases and the registry):** the
  `fitment-corpus` crate, never published and a versionless dev-dependency
  only: the corpus's one scorer. The strict gate, false positives and
  negatives, the baseline, the report and the `fitment.match` checker move
  into it from three test files, and judge an `Outcome` read from a
  library result or from a document's entry, found by its SHA-256. The
  binary's documents meet the strict gate and the baseline as the
  library's results do. The corpus gains four review cases on s01's and
  s02's sockets, one requirement removed in each: two `waived` (their only
  defect removed, `InterfaceMatch`, outside both rates) and two still
  rejected for their own defect; the library and the binary apply each
  case's review. Every M6 rule is in the registry, deferred to the PR that
  builds it; the coverage test reads the M6 design, refuses a rule id two
  designs state, and finds `#[tokio::test]` tests. No output of `fitment`
  changes.
- **M6, PR 2 (the server's skeleton, its harness and the OpenAPI
  document):** the `fitment-server` crate, never published and shipped as
  a container image (`deploy/Containerfile`): plain HTTP/1.1 with
  header-read, handler and connection limits; `/v1/health`, `/v1/ready`,
  `/v1/info` (`fitment.info`) and `/v1/openapi.json`; one registry of
  stable error codes sent as `application/problem+json` (`fitment.error`),
  never with a path or a library error's text; configuration from flags,
  `FITMENT_SERVER__<SECTION>__<KEY>` variables and a TOML file, with
  `check-config` and `healthcheck`; JSON logs on stderr and one line on
  stdout, the bound address; the server's own `server.db` with its
  `system` user and a lease, so one server holds a state directory. The
  committed OpenAPI 3.1 document is generated from the operation table and
  the documents' JSON Schemas, which `fitment-core` and `fitment-catalog`
  derive behind a new, off-by-default `schema` feature; `fitment-catalog`
  also gains `Catalog::inspect`, which checks a catalog writing nothing,
  and `Snapshot::status_counts`. `deny/sync.toml` keeps tokio, hyper, axum
  and tower out of `fitment` and `fitment-catalog`. No output of `fitment`
  changes.
- **M6, PR 3 (authentication and permissions):** users and bearer tokens
  for `fitment-server`, ported from Vectera's auth with its flaws fixed.
  Passwords are argon2id (m = 19 MiB, t = 2, p = 1, recorded in the PHC
  string and rehashed at login when they change) with the pepper of
  `auth.pepper_file` as argon2's secret, 12 to 256 characters; a login
  costs one hash whatever the username. Tokens (`fitment_pat_…`) are
  stored as SHA-256 only, shown once, always expire (90 days by default,
  365 at most) and are taken only from the `Authorization` header. Roles
  reader, writer and admin guard every route but health, readiness, the
  OpenAPI document and login; a request's role is the lower of its user's
  and its token's. A token mints others only within its own role and
  expiry, revoking a token revokes those minted from it, a password change
  revokes every other token of the user, and a user holds at most 20 live
  tokens. Requests are rate-limited per user, logins per address and per
  (username, address), in a bounded set of buckets. New routes under
  `/v1/auth/` (login, me, password, tokens, users) and the documents
  `fitment.token`, `fitment.user` and `fitment.page`; `/v1/info` now needs a
  token; the error code `username_taken`. `fitment-server admin bootstrap`
  makes the first admin offline and prints its token once; `admin user`
  and `admin token` manage users and tokens beside a running server. No
  output of `fitment` changes. After review: login failures are charged
  before the hash (IPv6 clients by /64), a login at the token cap evicts
  the oldest login token, a minted token reaches only its own descendants,
  minting is limited to 10 a minute, disabling a user revokes their tokens,
  the last enabled admin cannot be disabled or demoted (`last_admin`), and
  the token sweep runs in short batches.

## [0.2.0] - 2026-10-17

M5: the catalog. `fitment catalog` stores STEP parts by content, gated
and indexed, and `fitment match --catalog` searches them: a catalog run is
the file run over the parts its index admits, digest for digest. Publish
`fitment-core` first, then `fitment-catalog` (new, internal and unstable:
the CLI is its one user), then `fitment`. The four document formats of
0.1.0 keep their layouts and versions, and `fitment.catalog` v1 is new;
`signature_version` is 2, and since a `fitment.match` document records it
inside its digest, a match document's digest changes with it.

### Added

- **M5, PR 1 (the catalog's core):** `Matcher::judge_document` judges a
  stored features document exactly as `judge_step` judges its file, and
  refuses one another extractor version wrote (`NotCurrent`);
  `Matcher::admitted` runs level 1 over any `IndexSource`, the trait a
  catalog's store implements, each socket within the ones before;
  `IndexTokens` read back from JSON, checked (finite, `-0.0` refused,
  pairs in order); `FeaturesDocument::to_compact_json` and `part_digest`;
  `Source::new`.

- **M5, PR 2a (the store):** the `fitment-catalog` crate, internal and
  unpublished until M5's release: `Catalog::init` and `open` (a marker, a
  SQLite database in WAL with schema 1, and a blob store under
  `blobs/sha256/`), and `Catalog::ingest`: each file read once, gated,
  stored by its SHA-256 with its features document, tokens and postings,
  each derived row hashed over a preimage naming its key; ids from file
  names, exact stems first; duplicates, conflicts and quarantine. Only
  `fitment-catalog` may depend on rusqlite (`deny.toml`).

- **M5, PR 2b (the catalog's CLI):** `fitment catalog init`, `ingest`,
  `list` and `get -C DIR`, behind the default `catalog` feature: one line
  per file and a summary on stderr; `list` with each entry's status
  (indexed, quarantined or stale); `get` by id, `ID@rN` or `sha256:`, as a
  table, a `fitment.catalog` document, the stored features document
  (checked against its row's hash, and byte-identical to `fitment
  features`), or the file's bytes to `-o`, re-hashed first. Without the
  feature, `fitment catalog` exits 2.

- **M5, PR 3 (changes and upkeep):** `fitment catalog replace`, `delete`,
  `prune --revisions`, `gc` and `verify`. A replace keeps the old
  revision's bytes for `get ID@rN` until a prune; delete is all or nothing
  across ids; gc unlinks a file only under the write lock, having checked
  no row names it; verify checks every stored file, row hash, part digest
  and posting, and with `--deep` extracts every part again.

- **M5, PR 4 (the catalog match):** `fitment match SOCKET --catalog DIR`,
  with `--id`, `--exhaustive` and `--rejects`: level 1 finds the
  candidates in the catalog's index, each survivor's stored document is
  checked and judged as its file would be, and the document is the file
  run's over those files, labelled `catalog:<id>@r<n>`. A stale catalog,
  an empty one or a blocked socket refuses a full run before any candidate.
  `--jobs N` judges N candidates at once, in file runs too, and changes no
  byte of the result.

- **M5, PR 5 (properties, D10):** `StatedProperties::read` and
  `FeaturesDocument::read_with_properties` capture a part's own
  user-defined properties and material designations as written, at most
  1,024 values of 4 KiB, versioned by `PROPERTIES_VERSION` 1; `PropertyMap`
  ships one evidenced row (NX's `MaterialMissingAssignments` = `TRUE`: no
  material assigned). A catalog stores them at ingest, hashed over their
  preimage; `fitment catalog get` shows them, escaped, with their meaning;
  `list` shows an entry without them as `stale (properties)`, which a
  match never refuses. No verdict or flag reads them.

- **M5, PR 6 (re-indexing):** `fitment catalog reindex` writes what this
  build reads where an entry lacks it: an extractor rise reads each file
  through its hash check and extracts it again, a signature rise rebuilds
  tokens from the stored documents without reading a file, and properties
  are captured again. Batches of at most 64 entries, each one
  transaction, so a killed reindex resumes; one line per entry with its
  transition, exit 3 when a part leaves the search, exit 1 for a missing
  or damaged file, which stays stale. `gc --versions` removes older
  versions' rows where current ones exist, never a newer version's.

- **M5, PR 7 (the exit):** `tools/scale-step.py` scales corpus parts by
  their length unit alone, and `catalog_scale.rs` runs every corpus socket
  against a scaled catalog (weekly at 10⁴; by hand at 10⁵); a weekly CI
  workflow runs it. The hostile corpus parts are matched through a
  catalog as through files; a match runs with every stored file moved
  away; CI compares one catalog's outputs across Linux, macOS and Windows.
  `gc --vacuum` says what VACUUM needs first. Every M5 rule has a test or
  a reason (CT19). The README walks through a catalog.

### Changed

- `signature_version` is 2: pair tokens name their two stacks by index
  instead of copying their senses and diameters, and level 1 takes each
  candidate's lever, rounded up to a power of two, in place of the
  largest indexed. Level 1 never hides a part verification passes; it no
  longer claims to return everything level 2 keeps, which level 2's
  budget and borrowed diameters do not allow. The pair filter now steps
  aside at level 2's own angular window. Every `fitment.match` document
  records the version inside its digest, so a document for the same
  inputs has a new digest; its verdicts and evidence are unchanged.
- `fitment socket`, `match` and `review` sync an output file's temporary
  copy and its directory around the rename, so a crash leaves the old file
  or the new one.

## [0.1.0] - 2026-10-16

The first release: milestones M0 to M4. `fitment-core` is published first,
then `fitment`. The four document formats below are frozen at version 1.

### Added

- **M1 (in stepq 0.6):** units, geometry values, placements, topology, the
  solids of a definition and PMI to faces, read from STEP.
- **M2, features of one part:** `fitment features part.stp` and
  `read_part`: one part through the canonical gate (refused with the check
  and the `#id` at fault), its planes with exact outlines, cylinders,
  cones, tori, holes and shafts with their ends, and every face no rule
  reads; `features.v1`.
- **M3, sockets:** `fitment parts asm.stp` lists an assembly's parts
  (`fitment.parts` v1); `fitment socket asm.stp --part X` reads what the
  neighbours of a part require of any part that replaces it: bores and
  shafts by span with their fits, seats with their contact outlines,
  grips, conical seats, what is left out, and what no candidate can pass
  until a reviewer removes it; `fitment review` records each decision
  outside the digest, and a document of older rules carries its decisions
  over (`fitment.socket` v1).
- **M4, matching:** `fitment match socket.json candidate.stp|dir/...` and
  `Matcher`: a candidate's signature in rigid invariants, a shortlist that
  never drops what verification passes, alignment by anchor pairs (proper
  rotations only), and verification of every requirement with its
  measures and coded failures: axes, diameters by span, walls, openings,
  the clear core, seats covered, grips, conical seats. Verdicts `Match`,
  `InterfaceMatch` and `Reject`, built only by `decide`; `Match` needs a
  verified body fit, so without M7's verifier the best is
  `InterfaceMatch`. Policies: D9's 0.01 mm and 0.01°, strict or
  functional clearance (D6), hard grips (D7). Exit codes 0, 4 and 5 by the
  best verdict; 1 an error, 2 usage, 3 a socket refused. The
  `fitment.match` v1 document, in a fixed order with a digest the same on
  every OS. Bounded work on hostile files: past `MAX_PLACEMENTS` or
  `MAX_COVERAGE_STEPS` the requirement fails (`alignment_exhausted`,
  `coverage_exhausted`); past `MAX_SHORTLIST_CHECKS` the candidate is
  kept, never dropped.
- **The known-answer corpus** (`tools/gen-corpus.py`): 125 match cases and
  four hostile inputs, with truths from the construction, judged under both
  policies through the library and the binary; the false-negative
  baseline; the rates: no candidate passed whose interface does not fit
  (a colliding body is M7's); against physics, 20 of 49 fits rejected
  under `default` and 21 of 51 under `functional`, each by a stated rule
  (four of the 49 are grip-only thickness changes, fits by M4-8's
  convention that neighbours reseat).
- **The Open CASCADE oracle** (`tools/verify-occt.py`, Open CASCADE 8.0.1,
  not the generator's kernel): the truths it can judge checked physically
  (`fits`, `does_not_fit`, `fits_functional`, `collides`; not `edge`,
  `refused`, originals read in place, or grip-only thickness changes);
  every pass fitment makes fits at the placement it states; every
  failure re-measured; mutations of fitment's documents each met.

### Fixed during M4's reviews

- Alignment named its placement bound when reached: before, the
  placement past it was never pushed, so `alignment_exhausted` never
  fired.
- `ShortlistIndex` (level 1) asked a pair token between two coaxial
  required lines one candidate stack may serve together (SG5), so it
  dropped a part the shortlist (level 2) keeps; found by M5's design
  review before any run relied on level 1.

### Earlier (M0)

- Project plan (`docs/PLAN.md`) and the feasibility spike (`docs/spike/`).
- The Vectera reuse audit (`docs/vectera-audit.md`) and decision D1 on
  body-fit checking, with its research (`docs/research/d1-body-fit.md`).
- Repository scaffolding: license, contributing guide, code of conduct,
  security policy, CI, and issue and pull request templates.
- Cargo workspace with `fitment-core` (the matcher library) and `fitment`
  (the CLI, which so far only prints its version and help).
- `fitment-core`: `Verdict`, built only by `decide()`; `FitStatus` with the
  JSON shape fixed by D1; `TolerancePolicy` with D9's defaults, refusing
  values that cannot be compared against.
- CI checks that `fitment-core` stays free of I/O and nondeterminism: an
  exact dependency allowlist (`deny/core.toml`), checked alone and against
  the workspace-wide graph; source bans (`crates/fitment-core/clippy.toml`);
  and `tools/check-core.sh` for banned std modules, async code and global
  state. Open CASCADE and C++ bridge crates are banned workspace-wide (D1).
