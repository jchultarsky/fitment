# Workspace layout proposal (M0, step 3)

As of 2026-10-09. Status: **accepted** on 2026-10-09, with every recommendation in section 10 taken and the first publish at M4. It is recorded in [PLAN.md](../PLAN.md), Decisions made, row 11.

How the M0 skeleton departs from this document:

- **The cargo-deny allowlist command.** `--config` is a global option and goes before `check`: `cargo deny --manifest-path crates/fitment-core/Cargo.toml --config deny/core.toml check bans`.
- **Deferred items.** The `FitVerifier` trait waits for M4, because its request type depends on sockets and placements that don't exist yet. `EXTRACTOR_VERSION` waits for M2, because nothing is extracted before then. At M0, `FitStatus::no_verifier()` stands in for `NoFitVerifier`.
- **The `fitment` crate does not depend on `fitment-core` yet.** At M0 it has nothing to call.
- **The allowlist includes `serde_core` and `zmij`.** Current serde and serde_json pull them in.
- **Changes from the pre-commit review.** An adversarial review of the skeleton led to five changes:
  - **The package boundary does not isolate features.** Section 4, layer 1 is wrong: Cargo unifies features across a build, so a sibling can add crates to `fitment-core`'s build. `tools/core-deps.py` checks the allowlist against the workspace-wide graph, and `deny.toml` pins serde_json's exact features.
  - **The clippy bans cover more.** Added: the rest of `std::fs`, `env` and `thread`, `RandomState`, `ToSocketAddrs`, `option_env!`, `thread_local!`, the f64 hyperbolic functions and `exp2`, and every f32 transcendental.
  - **The async grep is now one script.** It became `tools/check-core.sh`, which also rejects whole `std` I/O modules and `static` items.
  - **The MSRV job really runs 1.85.** `rust-toolchain.toml` had been overriding it with stable.
  - **The panic claim is narrowed.** `lib.rs` denies explicit panics only, because implicit ones are not lints.

Three agents each drafted a layout from one angle (publishing, soundness, incremental delivery). Two judges scored the drafts, and both picked incremental delivery. The final proposal starts from that draft, adds the best ideas from the other two, and fixes the errors the judges found. Claims marked "checked" were tested in a scratch workspace.

## 1. Summary

- **Starting point.** This builds on the winning "incremental delivery" layout. It begins with two crates at M0, `fitment-core` (the matcher library) and `fitment` (the CLI, binary `fitment`). Each later crate arrives in the milestone that first needs it: catalog at M5, server and corpus scorer at M6, fit verifier at M7. Dependencies point only toward `fitment-core`, so no later milestone moves or renames code.
- **Naming changed from the winner.** The CLI package is `fitment` and the library is `fitment-core`, not `fitment` plus `fitment-cli`. This fixes three problems the judges found: `cargo install fitment` failing, the `cargo doc` output collision, and the Homebrew formula being named `fitment-cli`. All names were free on crates.io today (section 11).
- **The no-I/O rule is enforced by tools, not review.** Four checks fail CI on their own:
  - `fitment-core` is a separate package and depends on no workspace crate.
  - cargo-deny allows only an exact list of crates in `fitment-core`'s dependency graph.
  - A workspace cargo-deny rule allows only `fitment-core` to depend on stepq.
  - A `clippy.toml` inside `fitment-core` bans file system, network, process, thread, environment and clock calls, `HashMap`/`HashSet`, and f64 trig/exp/log functions (use `libm` instead). A grep catches `async`.
- **Soundness lives in the types.** Only `decide()` can build a `Verdict`, and `Verdict` cannot be deserialized. `FitStatus` variants are struct variants, as D1's JSON shape needs. User-edited socket files reject unknown fields and parse floats exactly. Every JSON document carries its own format version.
- **The unreleased stepq 0.5 needs two temporary lines.** One git-rev `[patch]` in the root manifest and one `allow-git` line in `deny.toml`, from the first M2 PR until stepq 0.5.0 is on crates.io. Three separate guards stop a publish while they exist. All releases share one version.

## 2. Directory tree

End state. The milestone that creates each item is in brackets.

```text
fitment/
├── Cargo.toml              [M0] virtual workspace; temporary [patch.crates-io] stepq from the first M2 PR until stepq 0.5.0 ships
├── Cargo.lock              committed; every CI command uses --locked
├── deny.toml               [M0] D1 bans · [M2] stepq wrapper rule + temporary allow-git
├── deny/core.toml          [M0] exact dependency allowlist for fitment-core (run with core as sole root)
├── dist-workspace.toml     [M4] copied from stepq; builds only the `fitment` binary
├── rust-toolchain.toml  rustfmt.toml  .editorconfig  .gitattributes  .gitignore   (existing)
├── crates/
│   ├── fitment-core/       [M0] lib: the matcher and every shared type
│   │   ├── clippy.toml     [M0] I/O, clock, env, thread, HashMap and f64-trig bans; applies to this package only, tests included
│   │   ├── README.md       crate docs via #![doc = include_str!], doctested
│   │   ├── src/ lib.rs verdict.rs fit.rs tolerance.rs                          [M0]
│   │   │        gate.rs geom.rs features/                                      [M2]
│   │   │        parts.rs socket/                                               [M3]
│   │   │        signature.rs shortlist.rs align.rs verify.rs evidence.rs       [M4]
│   │   └── tests/          decide() truth table [M0], proptest [M2+]; inputs only via include_bytes!/include_str!
│   │       └── data/       [M2] small committed unit parts (decision 6); excluded from the package
│   ├── fitment/            [M0] bin `fitment`
│   │   ├── src/ main.rs  cmd/{features[M2], parts socket schema[M3], match[M4], catalog[M5]}.rs
│   │   └── tests/          cli.rs[M0] fixtures.rs golden.rs[M2] schemas.rs[M3] corpus.rs[M4]
│   │                       (every test that reads or writes files at run time lives here)
│   ├── fitment-catalog/    [M5] lib, synchronous
│   ├── fitment-server/     [M6] lib + bin `fitment-server`; web/ viewer [M8]
│   ├── fitment-corpus/     [M6] lib, publish = false: FP/FN scorer shared by the fitment and fitment-server tests
│   └── fitment-fit/        [M7] lib
│       ├── helper/ fit_helper.py  fit_helper.py.lock   (embedded with include_str!; tessellate op [M8])
│       └── tests/adversarial.rs                        (fake helpers; no Python needed)
├── schemas/                [M3] features.v1.json socket.v1.json report.v1.json (generated, drift-tested)
├── corpus/                 [M2] README.md (FP definition, case labels) · baseline.json [M4] · out/ (already gitignored)
├── fixtures/               [M2] README.md + manifest.toml committed; fetched files gitignored
├── tools/
│   ├── fetch-fixtures.sh       [M2] stepq's script and SHA-256 pins, destination fixtures/
│   ├── gen-corpus.py (+.lock)  [M2] uv script: parts [M2], assemblies [M3], bad candidates [M4], fit probes [M7]
│   └── verify-occt.py (+.lock) [M2] dev-time OCCT oracle, the same OCP pin as the M7 helper
├── fuzz/                   [M3–M4, once the stepq patch is gone] separate workspace as in stepq: features, socket_json, verify
├── docs/                   PLAN.md · ARCHITECTURE.md [M2] · vectera-audit.md · research/ · spike/ (not a member)
└── .github/workflows/      ci.yml (extended per milestone) · release.yml + release-check.yml [M4]
```

At the end of M0:

```text
fitment/
├── Cargo.toml          virtual; [workspace.package] publish = false; no stepq in the graph yet
├── Cargo.lock
├── deny.toml           + D1 bans
├── deny/core.toml      allowlist: thiserror, serde, serde_json and their macro dependencies
├── crates/
│   ├── fitment-core/   Cargo.toml clippy.toml README.md
│   │   ├── src/ lib.rs verdict.rs fit.rs tolerance.rs
│   │   │        (Verdict, FitStatus, decide(), the FitVerifier trait, NoFitVerifier, TolerancePolicy with D9 defaults, EXTRACTOR_VERSION)
│   │   └── tests/verdict.rs        (full decide() truth table)
│   └── fitment/        Cargo.toml src/main.rs (clap; --version) tests/cli.rs
├── docs/ … unchanged · .github/workflows/ci.yml (--locked + purity steps)
└── (root [package] and src/main.rs deleted)
```

## 3. Crates

| name | kind | published | owns | may depend on | created in |
|---|---|---|---|---|---|
| `fitment-core` | lib | Yes, first at M4 (0.1.0). Public semver, checked by cargo-semver-checks. docs.rs/fitment-core is the library's front page. | The matcher and every shared type: canonical gate (D2), parts (D11), features, socket and its file format, signature, shortlist over in-memory signatures, alignment (Kabsch and the 24 rotations ported from Vectera in f64; a degenerate fit is an error), verification, tolerance policy, `decide()`/Verdict/FitStatus/evidence/flags, the sync `FitVerifier` trait plus `NoFitVerifier`, `EXTRACTOR_VERSION`. Bytes and values in, values out. stepq types never appear in its public API. | Exact list in `deny/core.toml`: stepq (no default features, `cli` banned), thiserror, serde, serde_json, libm (M2), optional schemars (`schema` feature, M3). Dev: proptest. No workspace crate. | M0 skeleton; real content M2–M4 |
| `fitment` | bin `fitment` | Yes, M4. Semver covers commands, flags, exit codes and format versions. | Argument parsing, file I/O, output, separate exit codes for Match, InterfaceMatch and Reject, `--require-fit`, `fitment schema <kind>`. Catalog subcommands (feature `catalog`, M5) and `--fit` (feature `fit`, M7). Hosts every test that touches files at run time. | fitment-core[schema], clap, anyhow, serde_json. Optional: fitment-catalog, fitment-fit. Never stepq directly (deny wrapper rule) and never tokio or axum (the server is a separate package). Dev: assert_cmd, predicates, fitment-corpus (path-only, M6). | M0 |
| `fitment-catalog` | lib, sync | Yes, M5, documented as internal and unstable. It must be published because the CLI's `catalog` feature needs it. | Content-addressed store: SHA-256 of the original bytes; temp file, fsync, rename; delete removes everything derived. SQLite index keyed by hash and `EXTRACTOR_VERSION`. Quarantine records (the gate logic stays in core). CRUD, re-index, stored assemblies, sockets and corrections, `ReportRecord` history. | fitment-core, rusqlite (bundled; D5 fallback), sha2, thiserror, serde, serde_json. No tokio, no network. | M5 |
| `fitment-server` | lib + bin `fitment-server` | Decided at M6 (decision 9); publish = false until then. Not built by dist; shipped as a container image without uv or OCCT. | REST over catalog and matcher, a job table and worker (matcher and catalog run under spawn_blocking), NDJSON progress, auth ported from Vectera with the audit's fixes, one error enum with stable codes, OpenAPI from the schemars derives, the M8 viewer. Takes an `Arc<dyn FitVerifier>`. | fitment-core, fitment-catalog, axum, tokio, tower-http, argon2, tracing; optional fitment-fit (for the binary). The only crate that uses tokio. | M6 (viewer M8) |
| `fitment-fit` | lib, pure Rust | Yes, M7. Public semver. It must be published because the CLI's `fit` feature needs it. | The `FitVerifier` from D1 option (c1). Writes out the embedded helper and its lock and runs `uv run --locked --script`. JSON-lines protocol with a handshake that reports protocol, kernel and OCP version, which must equal the pins. Strict decoding and a wall-clock timeout. Any crash, timeout, non-zero exit, warning, malformed reply, or missing uv or wheel gives `NotVerified`. The only crate allowed `std::process`. | fitment-core, serde, serde_json, thiserror. No OCCT bindings or cxx (workspace ban). | M7 |
| `fitment-corpus` | lib | Never | Corpus manifest parsing and FP/FN scoring, moved out of `fitment/tests/corpus.rs` when the server becomes the second user. | fitment-core, serde, serde_json. Only ever a path-only dev-dependency, which `cargo publish` strips. | M6 |
| `fuzz/` | cargo-fuzz targets, separate workspace | Never | Arbitrary bytes must never panic the feature extractor, the socket parser or verify. Any Match must carry a Verified fit. | libfuzzer-sys, arbitrary, fitment-core (path) | M3–M4 |

Why each split exists:

- **core / CLI.** It is the only way to fence the matcher's dependency graph per package. stepq's lib+bin pattern with a `cli` feature fails here for two reasons: `--all-features` would pull clap into the matcher, and a catalog that needs the matcher, under a CLI that needs the catalog, would form a cycle.
- **catalog / server.** M5's exit test runs from the CLI before any server exists, so the catalog must build without tokio. A separate server also keeps tokio and axum out of the `fitment` binary.
- **fitment-fit.** It needs process spawning, is optional, and D1 requires it to stay pure Rust.
- **No types crate.** Every consumer needs the matcher anyway. A later split can re-export from core without a breaking change.
- **No xtask.** The checks are shell and uv scripts.

## 4. How the matcher's purity is enforced

Each layer fails CI on its own.

1. **Package boundary.** All I/O dependencies live in other manifests, and `fitment-core` depends on no workspace crate. (Corrected in M0: a feature that a sibling turns on in a *shared* dependency still reaches `fitment-core` through Cargo's feature unification; `tools/core-deps.py` catches it.)
2. **Exact dependency allowlist.** `deny/core.toml` sets:
   - `[graph] all-features = true, exclude-dev = true`
   - `[bans] allow = [<exact list>]`
   - `features = [{ crate = "stepq", deny = ["cli"] }]`

   The deny job runs `cargo deny --manifest-path crates/fitment-core/Cargo.toml --config deny/core.toml check bans` (`--config` is a global option and goes before `check`). Any new crate fails until the same pull request edits the list where a reviewer sees it: tokio, mio, rusqlite, reqwest, a hashbrown that reintroduces random ordering, nalgebra.

   Checked in a scratch workspace with cargo-deny 0.20.2:
   - the exact list passes;
   - removing one entry gives `not-allowed … bans FAILED`;
   - turning on stepq's banned `cli` feature gives `feature-banned`.
3. **Only the matcher reads STEP.** The workspace `deny.toml` has `deny = [{ crate = "stepq", wrappers = ["fitment-core"], reason = "STEP reading lives in fitment-core" }]`. Checked: a second member depending on the stand-in crate directly gives `banned … bans FAILED`.
4. **Source bans in `crates/fitment-core/clippy.toml`.** Clippy reads it for this package only, test targets included. Checked: a ban fired in `core/tests/`, and the same call in the CLI crate was not flagged.
   - `disallowed-types`:
     - `std::fs::{File, OpenOptions}`
     - `std::net::{TcpStream, TcpListener, UdpSocket}`
     - `std::process::Command`
     - `std::path::{Path, PathBuf}`
     - `std::collections::{HashMap, HashSet}` (random iteration order; use `BTreeMap`/`BTreeSet`)
   - `disallowed-methods`:
     - `std::fs::{read, read_to_string, write, read_dir, metadata, create_dir_all, remove_file, rename, copy}`
     - `std::env::{var, var_os, vars, args, args_os, current_dir}`
     - `std::thread::spawn`
     - `std::time::{Instant::now, SystemTime::now}`
     - `std::io::{stdin, stdout, stderr}`, `std::process::exit`
     - `f64::{sin, cos, tan, asin, acos, atan, atan2, sin_cos, exp, exp_m1, ln, ln_1p, log, log2, log10, powf, powi, cbrt, hypot}`: use `libm`, which gives bit-identical results on every OS, so a verdict at the tolerance edge cannot differ between platforms. Checked: `x.sin()` is flagged. `sqrt`, `abs` and `mul_add` are correctly rounded and stay allowed.
   - `allow-unwrap-in-tests = true`, `allow-expect-in-tests = true`.
   - `lib.rs` adds `#![deny(clippy::disallowed_methods, clippy::disallowed_types, clippy::print_stdout, clippy::print_stderr, clippy::dbg_macro, clippy::exit, clippy::unwrap_used, clippy::expect_used, clippy::panic)]`. These go in `lib.rs` because `[lints] workspace = true` cannot be extended per member. SECURITY.md already treats a panic on crafted input as a security issue.
   - Test placement rule: core's tests take inputs only through `include_bytes!`/`include_str!`. Anything that reads or writes files at run time (fixtures, corpus, schema drift, goldens, bless) lives in `crates/fitment/tests`. This avoids the contradiction in proposal 1, where core's own schema-drift test would have broken its own bans.
5. **No async.**
   - The clippy job runs `! grep -rnE '\basync[[:space:]]+(fn|move|\{)|\.await\b' crates/fitment-core/src`.
   - Layer 2 keeps every executor out.
   - `FitVerifier` is synchronous: `fn verify(&self, req: &FitRequest<'_>) -> FitStatus`, with `Send + Sync`. The server calls the matcher inside `spawn_blocking`.
6. **D1 in the graph.** The workspace `deny.toml` bans `occt-sys`, `opencascade`, `opencascade-sys`, `cadrum`, `cxx` and `cxx-build`, each with reason "D1: no Open CASCADE in the Cargo graph".
7. **Soundness in the types** (see section 5): `decide()` is the only way to build a Verdict, and a Verdict cannot be deserialized.

An `#[allow]` remains possible, but it is local and shows up in review. Adding a crate to `deny/core.toml` is a soundness change and needs a reason in the PR. Making the crate `no_std` is the stronger alternative (decision 3).

## 5. Shared types and file formats

All shared types live in `fitment-core`. The crate that relies on the socket invariants is the one that parses and validates the user-edited socket file.

- **Documents.** There are three top-level JSON documents. Each carries `"format": "fitment.features" | "fitment.socket" | "fitment.report"` and `"format_version": 1`.
  - Format versions are independent of crate semver.
  - v1 is frozen at the first publish (M4). A format change bumps the version and gets a CHANGELOG entry.
  - Core refuses an unknown format or version (rule 2).
- **Socket file (input, edited by the user).**
  - `#[serde(deny_unknown_fields)]`, so a mistyped key fails instead of silently dropping a requirement (rule 3).
  - Parsed through `#[serde(try_from = "raw::SocketFile")]`, which checks: finite values, millimetres, min ≤ max diameter, unique requirement ids, and tolerance at least the larger stated file uncertainty.
  - Holds the assembly SHA-256, the part selector, occurrence paths, requirements, envelope, the user's removals with reasons, `EXTRACTOR_VERSION` and the `TolerancePolicy`.
  - Core owns `SocketFile::from_json`/`to_json`, so the CLI and the server validate identically.
- **Floats.** `serde_json` is built with `float_roundtrip`, set at workspace level so feature unification cannot drop it. Without it, a value at the tolerance edge can be mis-parsed by 1 ULP. A proptest checks that socket → JSON → socket is bit-identical.
- **Verdict.**
  - `Verdict::{Match, InterfaceMatch, Reject}` with private payloads.
  - Only `decide(&Socket, outcomes, FitStatus)` builds one. It requires exactly one outcome per requirement id, and anything else gives Reject.
  - A full truth-table test proves that only Verified fit plus every requirement verified gives Match, and Interferes gives Reject.
  - `Verdict` implements `Serialize` but not `Deserialize`. Stored history uses the catalog's `ReportRecord`, which is plain data and can never be fed back into `decide()`.
- **FitStatus.** It uses `#[serde(tag = "status", rename_all = "snake_case")]` with struct variants: `Verified { kernel, version }`, `Interferes { evidence }`, `NotVerified { reason }`, `NotChecked { reason }`. This gives D1's `{"status":"not_checked","reason":"no fit verifier in this build"}`. Checked: the newtype form `NotChecked(String)` fails at run time with "cannot serialize tagged newtype variant … containing a string". That was an error in proposals 1 and 3.
- **Evidence and flags.**
  - Each requirement records expected, measured, tolerance, outcome and the placement used.
  - Flags include "thread not confirmed" and "material not compared".
  - A not-covered list goes in every report (rule 5).
  - `TolerancePolicy { name, linear_mm, angular_deg }` defaults to D9, and the effective floor is recorded.
- **`EXTRACTOR_VERSION: u32`.** Bumped whenever extraction output changes; the catalog re-indexes on it. The golden test in `fitment/tests/golden.rs` fails if the output changes while the version stays the same.
- **Semver hygiene.**
  - Core has its own f64 `Point3`/`Dir3`/`Rigid` types, so no stepq or geometry-library type appears in a public signature.
  - `#![warn(clippy::exhaustive_enums, clippy::exhaustive_structs)]` in core: growing enums (Requirement, Flag, RejectReason, Refusal, feature kinds) are `#[non_exhaustive]`. Verdict and FitStatus, fixed by D1, carry a reasoned `allow`.
  - serde is a mandatory dependency of core, not a feature: the JSON files are the product.
- **JSON Schema.**
  - schemars 1.x sits behind core's `schema` feature, which the CLI enables. `fitment schema features|socket|report` prints a schema.
  - The output is committed as `schemas/*.v1.json`. `fitment/tests/schemas.rs` fails on drift, and `FITMENT_BLESS=1` rewrites the files.
  - At M6 the OpenAPI comes from a schemars-based generator such as aide, so nothing is derived twice.
  - The M7 helper's wire protocol is private to `fitment-fit` and is versioned by its handshake.

## 6. stepq dependency during M1–M2 and at publish time

- **M0:** no stepq in the graph.
- **From the first M2 PR:**
  - `[workspace.dependencies] stepq = { version = "0.5", default-features = false }`. This is the final form.
  - `default-features = false` must sit at workspace level. Checked on Cargo 1.85 with edition 2024: a member's own `default-features = false` is a hard error ("cannot override workspace's `default-features`"), not the ignored warning one judge described. stepq's default feature is `cli`.
  - The temporary table goes at the end of the root manifest:
    ```toml
    [patch.crates-io]
    stepq = { git = "https://github.com/jchultarsky/stepq", rev = "<40-hex sha on stepq main>" }
    ```
  - Precondition: stepq main sets `version = "0.5.0"` at the first M1 merge. Neither 0.4.x nor a 0.5.0-alpha satisfies `^0.5`, so the patch would go unused.
  - The rev moves only in a deliberate fitment PR. stepq main must keep `rust-version` at 1.85 or lower, or fitment's MSRV job fails.
  - `deny.toml` gets `[sources] allow-git = ["https://github.com/jchultarsky/stepq"]` with the comment "remove with [patch]". The existing `unknown-git = "deny"` would otherwise fail.
  - CI needs nothing else: the repo is public, Cargo.lock records the rev, and rust-cache caches `~/.cargo/git`.
- **Local co-development:**
  - Run `cargo --config 'patch.crates-io.stepq.path="../stepq"' test --workspace`. A config patch overrides the manifest's.
  - This rewrites Cargo.lock to a path source, so do not commit that lock. CI's `--locked` rejects it.
  - CONTRIBUTING documents this. A gitignored `.cargo/config.toml` is not used, because it would dirty the lock silently on every build.
- **Publishing cannot leak.** Three guards:
  - `[workspace.package] publish = false` until the release PR;
  - release-check fails while `^\[patch` is in Cargo.toml or `allow-git` is in deny.toml;
  - `cargo publish` verifies against crates.io without the patch, so it fails until stepq 0.5.0 exists.
- **Exit.** One PR deletes the `[patch]` and the `allow-git` line and runs `cargo update -p stepq`. Target: at or before M2 exit; hard deadline: before fitment's first publish (M4). `fuzz/` is created after this PR, so it never needs a patch of its own.
- **Afterwards.** stepq is a private dependency of `fitment-core` only, so stepq 0.6 is a one-line change in one crate and not a fitment breaking change.

## 7. Tools, corpus, oracle, fixtures

- **Python, following stepq's convention.** uv scripts live in `tools/`, with exact pins in the inline metadata. Each has a committed `<script>.lock` from `uv lock --script` and runs with `uv run --locked --script`. Checked with uv 0.11.31: the lock is written as `s.py.lock`, and a stale lock fails `--locked`.
- **Corpus generator: `tools/gen-corpus.py`, from M2.**
  - It builds parts with known features (M2), assemblies (M3), and bad candidates that each break one thing (M4): shifted hole, wrong diameter, missing hole, wrong thickness, blind instead of through, colliding body. Plus 0.5δ, 1δ and 2δ edge sweeps, the original part as a control, rotated or shifted frames, and multi-occurrence sockets.
  - It adds the D1 probes at M7: touching, 1 µm overlap, containment, and a bolt modelled at major diameter in a hole at minor diameter.
  - Expected answers come only from construction parameters and are written to `corpus/out/manifest.json`, never derived from fitment output.
  - Its pin differs from the oracle's: cadquery 2.8.0 requires `cadquery-ocp>=7.9.3.1,<8.0` plus VTK (decision 8).
- **OCCT oracle: `tools/verify-occt.py`, from M2, development only.**
  - It compares OCCT's cylinders, planes and (from M3) placements with `fitment features|socket` JSON.
  - From M4 it re-derives the collision labels (non-empty Common means collides), so each label has two independent sources.
  - It pins `cadquery-ocp-novtk==8.0.1.1.0`. A CI grep fails if this differs from the helper's pin.
- **M7 helper: `crates/fitment-fit/helper/`.**
  - It sits inside the crate because `include_str!` of a path outside the package breaks `cargo package`, and `cargo install` users need it.
  - It implements section 3 of the D1 research, including the control run of the original part.
  - `tests/adversarial.rs` re-executes the test binary as a fake helper that crashes, hangs, prints garbage, reports the wrong protocol, or emits a warning. Each case must give `NotVerified`. These tests run on all three OSes with no Python.
- **Corpus harness.**
  - M4: `crates/fitment/tests/corpus.rs` runs the built binary (`CARGO_BIN_EXE_fitment`). It asserts verdict and exit code, requires FP = 0, and requires each bad case to be rejected citing its injected defect. It reports the FN rate against `corpus/baseline.json` (decision 7) and writes a canonical report.
  - M6: scoring moves to `fitment-corpus`, so the server test runs the same cases end to end.
  - M7: the same cases run with the real helper.
- **Fixtures policy.**
  - Third-party STEP is never committed and never uploaded as a CI artifact (no redistribution terms). `tools/fetch-fixtures.sh` fetches it into `fixtures/`, checked against SHA-256 pins. Only `README.md` and `manifest.toml` are tracked.
  - The generated corpus is not committed. CI shares it between jobs as an artifact; it is our own MIT output.
  - The only committed STEP is the small unit set (decision 6), excluded from packages.
  - Tests skip locally when inputs are missing, but fail when `FITMENT_REQUIRE_CORPUS`, `FITMENT_REQUIRE_FIXTURES` or `FITMENT_REQUIRE_FIT` is 1, which CI sets. A skipped gate must not pass green.
  - Each published crate has an `include` allowlist: `src/**`, `README.md`, `LICENSE` (symlinked), plus `helper/**` for fitment-fit.

## 8. Workspace config

```toml
[workspace]
resolver = "3"           # must be explicit: a virtual manifest has no edition and would default to resolver 1; 3 = MSRV-aware lock
members = ["crates/*"]   # fuzz/ stays a separate workspace, as in stepq

[workspace.package]
version = "0.1.0"        # lockstep for every published crate
edition = "2024"
rust-version = "1.85"    # the first ^rust-version line in this file: CI's MSRV job greps it
authors = ["Julian Chultarsky"]
license = "MIT"
repository = "https://github.com/jchultarsky/fitment"
homepage = "https://github.com/jchultarsky/fitment"
publish = false          # members: publish.workspace = true; flipped in the first release PR

[workspace.dependencies]  # one reason per pin (Vectera habit)
# Internal: exact pins, moved together.
fitment-core    = { path = "crates/fitment-core", version = "=0.1.0" }
# fitment-catalog = { path = "crates/fitment-catalog", version = "=0.1.0" }   # M5
# fitment-fit     = { path = "crates/fitment-fit",     version = "=0.1.0" }   # M7
# fitment-corpus  = { path = "crates/fitment-corpus" }                        # M6, path-only dev-dependency
# M2: STEP reading, fitment-core only (deny wrapper). default-features must be off HERE.
# stepq = { version = "0.5", default-features = false }
thiserror  = "2"                                               # libraries
serde      = { version = "1", features = ["derive"] }
serde_json = { version = "1", features = ["float_roundtrip"] } # exact floats in socket files
libm       = "0.2"                                             # M2: same transcendental results on every OS
schemars   = "1"                                               # M3: fitment-core `schema` feature
clap       = { version = "4", features = ["derive", "wrap_help"] }
anyhow     = "1"                                               # binaries only (decision 12)
proptest   = "1"
assert_cmd = "2"
predicates = "3"
# M5: rusqlite (bundled), sha2 · M6: axum, tokio, tower-http, argon2, tracing (each with a reason)

[workspace.lints.rust]
unsafe_code = "forbid"
missing_docs = "warn"
rust_2018_idioms = { level = "warn", priority = -1 }

[workspace.lints.clippy]  # unchanged; per-crate extras go in lib.rs attributes
pedantic = { level = "warn", priority = -1 }
module_name_repetitions = "allow"
must_use_candidate = "allow"
missing_errors_doc = "allow"

[profile.release]
lto = "thin"
codegen-units = 1
strip = true

[profile.dist]
inherits = "release"

# From the first M2 PR until stepq 0.5.0 is on crates.io. Remove together with deny.toml's allow-git.
# [patch.crates-io]
# stepq = { git = "https://github.com/jchultarsky/stepq", rev = "<sha>" }
```

Member manifests:

- **All members:**
  - inherit `version`, `edition`, `rust-version`, `license`, `authors`, `repository` and `publish`, and set `[lints] workspace = true`;
  - set their own `description`, `keywords` and `categories`;
  - library crates add `[package.metadata.docs.rs] all-features = true, rustdoc-args = ["--cfg", "docsrs"]`.
- **fitment-core:** `[features] schema = ["dep:schemars"]`.
- **fitment:**
  - `readme = "../../README.md"` and `documentation = "https://docs.rs/fitment-core"`, since docs.rs does not document binary-only crates;
  - from M5: `default = ["catalog"]`, `catalog = ["dep:fitment-catalog"]`;
  - from M7: `fit = ["dep:fitment-fit"]`.
- **fitment-corpus:** `publish = false`.
- **fitment-server:** `[package.metadata.dist] dist = false`.

## 9. Changes to CI, deny.toml, CONTRIBUTING.md, release

**ci.yml**

- **M0:**
  - add `--locked` to every cargo command;
  - the deny job adds `taiki-e/install-action@v2` (cargo-deny) and the per-crate allowlist command from section 4;
  - the clippy job adds the async grep step;
  - MSRV, docs and the 3-OS test job are otherwise unchanged. The grep still finds `rust-version` at column 0 under `[workspace.package]`.
- **M2, new `corpus` job (Ubuntu):**
  - `astral-sh/setup-uv@v10.1.0` (pinned, as in stepq);
  - caches `corpus/out` keyed on `hashFiles('tools/gen-corpus.py*')` and `fixtures/` keyed on the fetch script;
  - steps: generate, `fetch-fixtures.sh core`, build `fitment`, run `verify-occt.py`, then `FITMENT_REQUIRE_FIXTURES=1 cargo test -p fitment --test fixtures --locked`;
  - uploads `corpus/out` as an artifact.
- **M2, the 3-OS `test` job:** gets `needs: corpus`, downloads the artifact and sets `FITMENT_REQUIRE_CORPUS=1`. The gates therefore run on Linux, macOS and Windows without Python, which fixes the judges' "gate only on Linux" finding.
- **M4:**
  - each OS uploads its canonical corpus report, and a `determinism` job `cmp`s the three;
  - `dist init` generates `release.yml`, and a `release-check` plan job checks for `[patch`, `allow-git` and tag = workspace version;
  - once the patch is gone: a `fuzz` job (nightly, 60 s per target, copied from stepq) and a `package` job (`cargo package --workspace --locked` plus a `--list` check that no `.stp`/`.step` file ships).
- **M5:** `cargo clippy -p fitment --no-default-features --locked -- -D warnings`. This is the only feature-matrix leg; a core `--no-default-features` leg would do nothing, since core has no default features.
- **M7:** a `fit` job (Ubuntu and macOS, uv) runs `FITMENT_REQUIRE_FIT=1` on fitment-fit and the corpus with `--fit`. Also a grep that the oracle and helper pins are equal.
- **After the first publish:** a cargo-semver-checks job on fitment-core (and fitment-fit from M7).

**deny.toml**

- M0: D1 bans with reasons. Keep `[graph] all-features = true` so the optional catalog and fit crates are audited.
- M2: the stepq `wrappers = ["fitment-core"]` rule and the temporary `allow-git`.
- M6: `allow-wildcard-paths = true` for the path-only dev-dependency on fitment-corpus. Expect a licence review at M5 and M6, and avoid HTTP clients that pull in webpki-roots (CDLA-Permissive-2.0).
- Every advisory `ignore` entry carries a reason.
- New file `deny/core.toml` (M0).

**.gitignore:** add `/fixtures/*`, `!/fixtures/README.md`, `!/fixtures/manifest.toml`, `/fuzz/target`, `/fuzz/artifacts`, `/fuzz/corpus`.

**CONTRIBUTING.md**

- Replace "Until the workspace lands" with a **Repository layout** section: the tree, one row per crate (published?, may depend on, must not), and the arrow direction `server/fit/catalog/cli → fitment-core → stepq`.
- Reword "The matcher has no I/O" to name `fitment-core` and its enforcement. Add the determinism bans (HashMap, clock, env, f64 trig) and "a Verdict is built only by `decide()`".
- Add the test placement rule, and the rule that adding to `deny/core.toml` needs a reason.
- Add the error rule (decision 12).
- "Before you open a pull request": `--locked` on every command, the per-crate deny command, and the async grep.
- "Getting set up": uv is needed only for `tools/` and fit checks. Add "Working against an unreleased stepq" (section 6).
- Corpus: keep "generated, not vendored". Add that expected answers come only from construction parameters, and add the unit-set exception if decision 6 is accepted. Document `FITMENT_BLESS` and the `FITMENT_REQUIRE_*` variables.

**Also**

- New `docs/ARCHITECTURE.md` (M2): crate graph and boundaries, format versions vs crate versions, `EXTRACTOR_VERSION`, determinism rules.
- PLAN.md Components table: name the crates.
- README gains an Install section at M4.

**Release**

- **Versioning:** lockstep (one version, one `vX.Y.Z` tag, one CHANGELOG) with exact internal pins.
- **Publishing order:**
  - first publish at M4 exit: `fitment-core` and `fitment` 0.1.0, once stepq 0.5.0 is on crates.io, the patch is gone and the zero-FP gate is green;
  - `fitment-catalog` joins at M5, `fitment-fit` at M7, and `fitment-server` per decision 9.
- **crates.io:** `cargo publish --workspace --locked` on stable. It needs Cargo 1.90 or later. Checked with a dry run: it skips `publish = false` packages, so no `--exclude` is needed. After each crate's first publish (which needs a token), use crates.io trusted publishing through `rust-lang/crates-io-auth-action` in a dist custom publish job.
- **Binaries:** dist 0.32.0, the latest, with stepq's six targets and shell, PowerShell and Homebrew installers to `jchultarsky/homebrew-tap`. Only the `fitment` binary, whose formula is `fitment`. `[package.metadata.dist] features` chooses which features release binaries include (decision 10).
- **No OCCT in any artifact:** `fit` embeds only the script text, and uv fetches the wheel on the user's machine. On musl and Windows ARM, fit returns `NotVerified` with a reason.

## 10. Decisions for the owner

1. **Crate names.** *Recommended:* `fitment` = CLI package and binary (`cargo install fitment`, `brew install jchultarsky/tap/fitment`, matching stepq), `fitment-core` = library. *Alternative:* `fitment` = library and `fitment-cli` = binary. That needs `cargo install fitment-cli`, `doc = false` on the binary to avoid the `cargo doc` collision, and a dist formula rename.
2. **MSRV vs nalgebra.** nalgebra 0.35.0 declares Rust 1.89 and 0.34.2 declares 1.87; 0.33.3 declares none. *Recommended:* keep 1.85 (stepq parity) and hand-write the f64 3×3 geometry, as the spike already does, including Kabsch via a small Jacobi eigensolver. Revisit nalgebra at M4 only.
3. **`no_std` + alloc for fitment-core.** It would make the compiler enforce the bans and cannot be `#[allow]`ed. The costs: `f64::sqrt` and friends need `libm` too, and every future dependency must work without std. *Recommended:* no; clippy.toml plus the allowlist are enough. Revisit if a ban is ever bypassed.
4. **stepq route and timing.** *Recommended:* the git-rev `[patch]` with stepq main at 0.5.0 from the first M1 merge, and stepq 0.5.0 published at or before M2 exit (it must be before M4). *Alternative:* publish `0.5.0-alpha.N` to crates.io and pin it with `=`. That avoids the git source but leaves permanent alphas on crates.io.
5. **First publish and name risk.** *Recommended:* 0.1.0 at M4 exit, accepting that `fitment` (a common automotive word) could be taken before then. *Alternative:* publish a 0.0.1 at the end of M0 with the D1 types and a version-only CLI. This needs no stepq, but freezes the D1 type shapes in public early.
6. **Commit a small unit set** of our own generated parts (300 KB or less, MIT, excluded from packages) in `crates/fitment-core/tests/data`. *Recommended:* yes, so extraction tests run on all three OSes without Python. It amends CONTRIBUTING's "generated, not vendored" wording.
7. **Gate strictness.** *Recommended:* a bad case must be rejected citing its injected defect, and any change in the false-negative set fails until `corpus/baseline.json` is re-blessed. *Alternative:* any Reject counts as correct and FN is only reported.
8. **Generator kernel.** cadquery 2.8.0 pins OCCT 7.9.x with VTK, while the oracle and helper use novtk 8.0.1.1.0. *Recommended:* accept two kernels; the generator only writes inputs, and the oracle re-reads them independently. *Alternative:* write the generator on raw OCP 8.0.1.
9. **Server delivery (M6).** *Recommended:* a separate `fitment-server` package (lib + binary) shipped as a container image without uv or OCCT, also published to crates.io as internal. *Alternative:* `fitment serve` inside the CLI, which would put tokio in the CLI.
10. **CLI default features and dist builds.** *Recommended:* `catalog` on by default from M5 (it adds a bundled SQLite C build to `cargo install`), and `fit` compiled into dist binaries from M7. Please confirm that the latter meets D1's "no OCCT in official artifacts", since OCCT arrives only via uv at the first `--fit`.
11. **D5 now constrains the layering.** *Recommended:* SQLite through rusqlite, which keeps the catalog synchronous and the `fitment` binary free of tokio. Choosing Postgres or sqlx makes the catalog async and reopens the CLI/catalog split. Decide by M5.
12. **Error rule for both repos.** *Recommended:* thiserror in libraries, anyhow only in binaries and tests. This settles vectera-audit §7 and matches stepq's CLI.
13. **Versioning.** *Recommended:* lockstep with exact internal pins until 1.0, with cargo-semver-checks on fitment-core and fitment-fit only. *Alternative:* independent per-crate versions with release-plz.

## 11. Crates.io name check results

Method: `GET https://crates.io/api/v1/crates/<name>` on 2026-10-09 with a generic User-Agent, re-checked independently the same day.

| Name | HTTP | Result |
|---|---|---|
| `stepq` (control) | 200 | exists |
| `serde` (control) | 200 | exists |
| `fitment` | 404 | "crate `fitment` does not exist": free |
| `fitment-core` | 404 | free; `fitment_core` also 404 (crates.io treats `-` and `_` as the same name) |
| `fitment-cli` | 404 | free (only needed for the alternative in decision 1) |
| `fitment-catalog` | 404 | free |
| `fitment-server` | 404 | free |
| `fitment-api` | 404 | free (not proposed: on crates.io "-api" reads as client bindings) |
| `fitment-fit` | 404 | free |
| `fitment-corpus` | 404 | free (never published) |
| `fitment-types` | 404 | free (not created) |
| `fitment-occt` | 404 | free (not proposed: suggests bundled OCCT) |
| `fitment-fuzz` | 404 | free (never published) |

A search with `q=fitment` returned one crate, `powerlaw`, a keyword match rather than a name, so no similar name is taken.

Other facts re-checked today:

- **crates.io versions and declared MSRVs:**

  | Crate | Version | MSRV |
  |---|---|---|
  | clap | 4.6.7 | 1.85 |
  | schemars | 1.2.2 | 1.74 |
  | thiserror | 2.0.21 | 1.77 |
  | serde_json | 1.0.151 | 1.71 |
  | libm | 0.2.16 | 1.63 |
  | proptest | 1.11.0 | 1.85 |
  | assert_cmd | 2.2.2 | 1.85 |
  | predicates | 3.1.4 | 1.74 |
  | axum | 0.8.9 | 1.80 |
  | tokio | 1.53.2 | 1.71 |
  | rusqlite | 0.40.2 | none declared |
  | cargo-dist | 0.32.0 | latest |
  | nalgebra | 0.35.0 / 0.34.2 / 0.33.3 | 1.89 / 1.87 / none declared |

- **PyPI:** cadquery 2.8.0 requires `cadquery-ocp>=7.9.3.1,<8.0` and `trame-vtk`. cadquery-ocp-novtk 8.0.1.1.0 needs Python >=3.11,<3.15.
- **Scratch-workspace checks** (Cargo 1.85 and 1.99, cargo-deny 0.20.2, uv 0.11.31):
  - internal tagging rejects a `NotChecked(String)` newtype at run time; struct variants serialize as D1 specifies;
  - a member's `clippy.toml` applies to that package's integration tests and to no other package;
  - `f64::sin` can be banned through `disallowed-methods`;
  - the member-as-root cargo-deny allowlist, the `bans.features` ban on `cli`, and the stepq `wrappers` rule each pass and fail as intended;
  - on 1.85 with edition 2024, a member's `default-features = false` on an inherited dependency is a hard error;
  - `uv run --locked --script` fails on a stale `<script>.lock`;
  - `cargo publish --workspace --dry-run` skips `publish = false` packages.

  No repository files were changed.