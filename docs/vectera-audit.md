# Vectera audit

As of 2026-10-09. Vectera at `1c01763` (develop, after release v2026-06-26).

This is the M0 audit against the checklist in [PLAN.md](PLAN.md#vectera-audit-checklist).
Plan decision 9 says fitment must build without Vectera, so the options are
**depend**, **copy** (with attribution: Vectera is MIT, same author) or
**skip**.

## Summary

Nothing in Vectera should become a dependency. Every crate that could be
useful is coupled to something fitment does not want:

- `vstore` is one Postgres `Storage` struct that covers every domain.
- `commons` holds Vectera's own model types.
- The `catalog` library pulls in Qdrant, three-d/GL, ONNX and Open CASCADE.
- `meshalign` works in f32, keeps its useful helpers private, and scales
  meshes to a unit cube, which throws away absolute size.

Vectera is a similarity search over triangle meshes. It has no B-rep code
at all: nothing that detects holes, cylinders or planes, or walks faces.
So the reuse is at the edges (auth, a few math routines, the viewer, build
and CI patterns), not in the matcher.

| Checklist item | Verdict | What to take |
| --- | --- | --- |
| Storage of CAD files | Skip | Lessons only; see below |
| Metadata or index database | Copy two pieces | The auth tables; the parameterized key/value filter builder |
| REST scaffolding | Copy auth only | Password hashing, hashed tokens, roles. Redesign errors; there is no OpenAPI |
| Background jobs | Skip; none exist | The NDJSON progress-stream shape |
| 3D viewer, tessellation | Copy later (M8) | The three.js viewer; the Open CASCADE build setup |
| Geometry and similarity | Copy three functions | Kabsch, the 24 proper rotations, the log-ratio tolerance window |
| Configuration, CI, release | Mostly already covered | The documented advisory-ignore list; `.env.example` layout; docker-compose for Postgres |

## 1. Storage of STEP or CAD files: skip

How it works: `POST /api/v1/assets` creates a row with a random UUID, then
`POST /api/v1/files/{uuid}` sends the whole body (buffered, 256 MiB cap,
no multipart or streaming). Blobs sit flat on disk as `<uuid>.bin`, with
`<uuid>.source.step` beside it for STEP uploads.

Why it does not fit fitment, which wants content-addressed storage:

- Identity is a random UUID per asset, not a content hash. A duplicate
  file is stored twice.
- For a STEP upload, the stored `sha256` is the hash of the derived STL.
  The STEP file itself is never hashed.
- There is no versioning. Re-uploading overwrites the blob in place.
- Writes are not atomic: there is no temp-file-and-rename, no fsync, and
  the file write and its four row updates are not in one transaction
  (`catalog/src/service.rs:590-631`).
- Deleting an asset removes only `.bin`, so `.source.step` and
  `.cleaned.png` files are left behind.

The `canonical.rs` hash is a *mesh* canonicalization: centre the
triangles, scale them, quantize, sort, then SHA-256. It changes with
rotation and with re-tessellation. It has nothing to do with the
plan's canonical STEP gate (D2) and should not be confused with it.

Lessons for the fitment catalog:

- Key blobs by SHA-256 of the original STEP bytes.
- Write to a temp file, fsync, then rename.
- Put the file write and the row in one unit of work.
- Make delete remove everything derived from a file.

Found in passing, a Vectera bug: `service.rs:598` and `:623` pass the
raw STEP bytes (`data`) instead of the converted STL (`effective_data`).
So every STEP upload gets NULL `canonical_sha256` and NULL mesh stats.

## 2. Metadata or index database: copy two pieces

Vectera uses Postgres only, through sqlx 0.8, with runtime queries and
12 migrations run by `sqlx-cli`. `data/sqlite/vectera.sqlite` is a
leftover that no code references.

The schema relies on many Postgres-only features: UUID, JSONB, ENUM,
plpgsql triggers, `pg_trgm`, `DISTINCT ON` and expression indexes. It is
built around a virtual folder tree and UUID identity. None of that matches
fitment's index, which is per-part features keyed by content hash and
extractor version.

Worth copying:

- **The auth tables**: `users`, `sessions`, `personal_access_tokens` (see §3).
- **The filter builder** in `vstore/src/service.rs:860-1018`
  (`advanced_search_uuids` / `push_filter_clause`). It builds a
  parameterized `QueryBuilder` filter over key/value metadata rows. It
  suits filtering on STEP properties (D10). On SQLite it needs light
  porting.

Quality issues not to carry over:

- The same 13-column SELECT is pasted about eight times.
- N+1 queries: folder ancestry, search re-fetch, and the tree build.
- The trigram index is on `lower(value)` but the queries use
  `value ILIKE`, so the index is likely unused.
- There are no database tests.

Effect on D5 (storage and API stack): nothing here argues against the
plan's fallback of SQLite, content-addressed files and axum. Choosing
Postgres only to match Vectera would buy no reusable code.

## 3. REST scaffolding: copy auth, redesign the rest

Vectera's REST service is actix-web 4. The route handlers live in the
binary (`catalog/src/main.rs:13`), so they cannot be imported. There is
no logging, CORS or request-ID middleware, no rate limiting, and no
OpenAPI.

Errors are handled ad hoc. NotFound becomes 404 and everything else
becomes 500, with the error text as the response body. That leaks sqlx
messages, and conflicts come back as 500. Some errors are mapped by
matching their message text. **Redesign:** one error enum with an
`IntoResponse` impl, and stable machine-readable codes.

Auth is the best-factored code in the repository, in
`catalog/src/auth.rs` and `commons/src/auth.rs`. **Copy the logic and port
the extractors to axum.** What it does:

- Passwords: argon2id with an optional pepper and a 12-character minimum.
- Tokens: 256-bit opaque values, stored only as SHA-256 hashes.
- Sessions: a `HttpOnly` cookie with `SameSite=Strict` and a configurable
  `Secure` flag; a 30-day hard limit plus a 7-day inactivity window.
- Personal access tokens: prefixed, with optional expiry.
- Roles: admin, editor and viewer, enforced by `ReadPerm`, `WritePerm` and
  `AdminPerm` extractors.
- The first admin account is created from environment variables.

Fix while porting:

- Unknown usernames return before argon2 runs, so login timing reveals
  which usernames exist (`auth.rs:348-352`).
- Only admins can create personal access tokens.
- The bootstrap password is written to the log.
- Expired sessions are never cleaned up.
- Every authenticated request costs one or two database writes.

The HTTP client in `catalog/src/client.rs` (reqwest, Bearer header, 404
mapped to `None`) is a fine pattern for a CLI talking to the API. It needs
timeouts, and the builder's errors should not be swallowed.

## 4. Background jobs or a queue: none exist

Long operations run inside the HTTP request: STEP conversion,
recomputing statistics for every asset, and calls to the Claude API.

The one reusable shape is `catalog/src/nl_query.rs:409-411`: work is
started with `tokio::spawn`, sends events over an mpsc channel, and they
are streamed to the client as NDJSON. This fits progress reporting for
fitment's ingest and match jobs.

The job table, worker, retry and persistence needed for M6 are new work.

The CLI ingest uses `buffer_unordered` with a progress bar, which is a
fine pattern for `fitment catalog ingest`. Note that Vectera's
`ingest-corpora.sh` passes `*.step` files to a reader that only accepts
STL.

## 5. 3D viewer or tessellation path: copy later, for M8

The viewer is three.js 0.160 in inline JavaScript (`web/index.html:29-757`,
about 730 lines), driven from Leptos through `stl_preview.rs`. Leptos
calls into the JavaScript and does no rendering itself. The viewer offers:

- STL loading with trackball controls
- Cached thumbnails
- A side-by-side view with one shared camera
- An overlay with a blend slider and a deviation heatmap

It is generic apart from its download URLs. For M8 it is a good base:
show the socket's axes and planes as overlays on the assembly and the
candidate. Before copying, vendor three.js or add SRI hashes; today it
is loaded from unpkg without integrity checks.

Skip the headless three-d renderers (`search3d/src/geometry/render.rs`
and its copy in `catalog/src/view_renderer.rs`). They need a GL context,
are `!Send`, and are never tested in CI.

Tessellation runs through `step2stl`: 31 lines wrapping `opencascade-rs`,
pinned as a git dependency on the upstream repository
(`bschwind/opencascade-rs@7e8d78a`). It builds Open CASCADE **7.8.1** from
source with CMake and links it statically.
It converts a file path to an STL path and exposes no faces, surfaces or
edges. Its one test passes vacuously when its sample file is missing.

Skip the wrapper. Keep the build lessons, which bear on D1:

- `CMAKE_POLICY_VERSION_MINIMUM=3.5` is needed with CMake 4.
- CI must install `cmake build-essential clang` and cache the Open CASCADE
  build.
- A git dependency cannot be published to crates.io. Since fitment will
  be published, the optional fit-verifier crate needs a kernel binding
  from crates.io (cadrum 0.8.20, or opencascade 0.3.0, released
  2026-08-24) or must stay unpublished. The D1 research
  ([d1-body-fit.md](research/d1-body-fit.md)) favours keeping Open CASCADE
  out of the Cargo graph altogether.
- Run Open CASCADE out of process, or at least behind `spawn_blocking`,
  with an eye on crashes. Vectera runs it inside the server process, so a
  crash in Open CASCADE takes the server down.

## 6. Geometry, vector or similarity code: copy three functions

The 1226-dimension embedding and the Qdrant search measure fuzzy shape
similarity on meshes normalized to unit size. That is the opposite of
fitment's exact invariants, so **skip** them. Fitment's shortlist is a
range lookup on quantized hole diameters and spacings in an ordinary
index; it does not need a vector database.

Worth copying, rewritten in f64 with nalgebra's `Isometry3`:

- **Kabsch best-fit rotation**, `meshalign/src/align.rs:245-274`. Once
  hole-axis correspondences are found, the best rigid transform between
  the two sets of hole centres is exactly this. It already handles a
  reflection and a degenerate SVD.

  Under the soundness rules, the identity fallback must become an error.
  A degenerate fit has to reject, not silently align.

- **The 24 proper axis rotations**, `align.rs:335-375`. Useful for
  enumerating the placements of a symmetric part.
- **The log-ratio tolerance window**, `catalog/src/search.rs:392-414`.
  It is relevant only if a relative tolerance is ever wanted. The plan
  uses absolute tolerances (D9), so this is low priority.

For the M7 clearance check, use `parry3d` rather than Vectera's BVH. The
BVH answers ray and nearest-point queries but has no mesh-to-mesh
distance. Its `nearest_hit` also uses a fixed 64-entry stack that can
silently drop nodes.

## 7. Configuration, CI and release patterns

fitment's scaffolding follows **stepq**, not Vectera, because the two are
published together. stepq is also ahead on every count: workspace lints,
an MSRV job, a pinned toolchain, rustfmt and cargo-deny configuration,
issue templates, and dist releases.

| Vectera pattern | Decision |
| --- | --- |
| `[workspace.dependencies]` with a reason next to each pin | Copy, when the workspace lands |
| Release profile: thin LTO, one codegen unit, strip | Already in place, via stepq |
| `.cargo/audit.toml`: each ignored advisory with its reason | Copy the habit into `deny.toml`'s `ignore` |
| `scripts/ci.sh` as a pre-push hook mirroring CI | Optional. Vectera needed it because its remote is a private NAS; fitment has GitHub Actions |
| Git-flow (`develop`, `main`, `feature/*`) with date tags `vYYYY-MM-DD` | Skip. fitment is a published crate: SemVer tags and trunk-based `main`, like stepq |
| `CHANGELOG.md` in Keep a Changelog format | Already in place, with SemVer headings |
| `CLAUDE.md` rules: thiserror only, no anyhow; `tracing` with `EnvFilter`; document all public items | Copy when the code starts. One difference: stepq's CLI uses anyhow. Pick one rule for both |
| `.env.example`, grouped and every variable commented; `docker-compose.yml` for Postgres | Copy for M5 and M6 if Postgres is chosen. Load `.env` in the binary, never inside a library |
| Workspace hygiene to avoid: mixed editions, every crate at 0.1.0, a stub `prelude` crate, a cargo-machete `ignored` entry that does nothing | Skip |
