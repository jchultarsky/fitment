# Contributing

Thanks for your interest. This document explains how the repository is
organised and what a good contribution looks like.

By participating you agree to follow the [Code of Conduct](CODE_OF_CONDUCT.md).

> fitment is early: the workspace exists, the matcher does not yet. The
> most useful contributions now are comments on [docs/PLAN.md](docs/PLAN.md)
> and real STEP files that exercise its assumptions (see below).

## Ground rules

* **No false positives.** A candidate passes only when every requirement
  is positively verified. Anything the code does not recognize is a
  rejection, not a pass. Read the soundness rules in
  [docs/PLAN.md](docs/PLAN.md#soundness-rules) before changing the
  matcher; a change that trades false positives for fewer false negatives
  will not be merged.
* **Deterministic.** Rules and arithmetic only, no machine learning.
  The same inputs and tolerance policy always give the same verdict.
* **STEP is the only data source.** Metadata comes from properties and PMI
  in the file, or is reported as unknown.
* **File reading belongs in stepq.** Reading what a STEP file states
  (placements, surface parameters, topology, units) goes into
  [stepq](https://github.com/jchultarsky/stepq). fitment interprets those
  values; it does not parse STEP itself, and only `fitment-core` may
  depend on stepq.
* **The matcher has no I/O and no nondeterminism.** `fitment-core` takes
  values and returns values: no files, network, processes, threads, clock,
  environment or async code, no `HashMap`/`HashSet` (use `BTreeMap`/
  `BTreeSet`), and f64 transcendental functions come from `libm`, not the
  platform. This is enforced, not just asked for; see
  [Repository layout](#repository-layout).
* **Only `decide()` makes a verdict.** A `Verdict` cannot be built or
  deserialized any other way, so no code path can produce a `Match` that
  skipped a check.
* **Errors:** `thiserror` in libraries, `anyhow` only in binaries and tests.
* **No `unsafe`.** **No Open CASCADE in the Cargo graph** (decision D1); the
  fit verifier runs it in a separate process.

New to STEP? The companion book
[*Inside the STEP File*](https://jchultarsky.github.io/step-book/) explains
the entity model the code takes for granted.

## Getting set up

You need Rust 1.85 or newer (the MSRV, checked in CI) and, for the
dependency audit, [`cargo-deny`](https://github.com/EmbarkStudios/cargo-deny)
(`cargo install cargo-deny`).

```console
$ git clone https://github.com/jchultarsky/fitment
$ cd fitment
$ cargo test --workspace --all-features --locked
```

### Working against an unreleased stepq

From milestone M2 until stepq 0.5.0 is on crates.io, the root
`Cargo.toml` has a `[patch.crates-io]` entry that points stepq at a pinned
commit on GitHub, and `deny.toml` allows that one git source. Both are
removed in the same PR, at the latest before fitment's first publish. To
build against a local stepq checkout instead:

```console
$ cargo --config 'patch.crates-io.stepq.path="../stepq"' test --workspace
```

That rewrites `Cargo.lock` to a path source; do not commit that lock. CI
builds with `--locked` and will refuse it.

### Test data

The test corpus is generated, not vendored: scripts build assemblies and
candidates with known answers using [cadquery](https://cadquery.readthedocs.io/)
on Open CASCADE. They run through [uv](https://docs.astral.sh/uv/), which
installs the Python dependencies on first use.

## Before you open a pull request

CI runs exactly these; run them locally first:

```console
$ export RUSTFLAGS="-D warnings" RUSTDOCFLAGS="-D warnings"
$ cargo fmt --all -- --check
$ cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
$ cargo test --workspace --all-features --locked
$ cargo +1.85 check --workspace --all-features --locked
$ cargo doc --workspace --no-deps --all-features --locked
$ cargo deny check
$ tools/check-core.sh
```

`cargo +1.85` needs `rustup toolchain install 1.85`; 1.85 is the
`rust-version` in `Cargo.toml`, which CI reads. `tools/check-core.sh` needs
cargo-deny and Python 3.

Changes to feature extraction, socket extraction or verification must keep
the false-positive count on the known-bad corpus at zero, and should report
any change in the false-negative rate in the PR description.

Pedantic clippy is on. If a lint is wrong for a specific line, `allow` it
there with a comment; do not disable it workspace-wide without discussion.

## Repository layout

The workspace grows one crate per milestone; dependencies always point
toward `fitment-core`, and only `fitment-core` reads STEP (through stepq).

```
crates/
  fitment-core/   lib: the matcher and every shared type (verdicts, fit
                  status, tolerances; later features, sockets, evidence)
    clippy.toml   source-level bans for this crate only, tests included
  fitment/        bin `fitment`: arguments, files, output, exit codes
deny.toml         workspace policy: licences, sources, D1 crate bans
deny/core.toml    the exact crates fitment-core may depend on
tools/            check-core.sh and core-deps.py: fitment-core's checks
docs/             PLAN.md, the Vectera audit, decision research, the spike
```

Coming later: `fitment-catalog` (M5, synchronous, SQLite), `fitment-server`
(M6, the only crate with an async runtime), `fitment-corpus` (M6, test-only,
never published) and `fitment-fit` (M7, drives the Open CASCADE helper).
The full design is in
[docs/research/workspace-layout.md](docs/research/workspace-layout.md).

How `fitment-core`'s rules are enforced, each by CI on its own:

1. **Package boundary.** It depends on no other workspace crate. Cargo
   still unifies features across a build, so a sibling that turns on a
   feature of a shared dependency (say `serde_json/preserve_order`) changes
   fitment-core's build too; the next two layers catch that.
2. **Dependency allowlist.** `deny/core.toml` lists every crate it may
   depend on, checked both with fitment-core alone and against the
   workspace-wide graph. `deny.toml` also pins serde_json's exact features.
   Adding a crate is a soundness change: say why in the PR.
3. **Source bans.** `crates/fitment-core/clippy.toml` is the authoritative
   list: I/O, environment, clock, threads, randomly seeded hashing, global
   state macros and platform-dependent float maths. `lib.rs` makes those
   lints errors, along with explicit panics (`panic!`, `unwrap`, `expect`,
   `todo!`, `unimplemented!`). Implicit panics (indexing, arithmetic,
   `assert!`) are not lints; reviewers look for them.
4. **Source checks.** `tools/check-core.sh` fails on any use of the `fs`,
   `net`, `env`, `process`, `thread` or `os` modules, on async code and on
   `static` items, which clippy cannot ban as a whole.
5. **D1.** `deny.toml` bans Open CASCADE and C++ bridge crates everywhere.

An `#[allow]` for one of these needs a comment saying why, and will be
questioned in review.

### Where tests go

`fitment-core`'s tests take their inputs only from `include_bytes!` or
`include_str!`, because its bans apply to its tests too. Any test that
reads or writes files at run time (fixtures, the corpus, golden files)
lives in `crates/fitment/tests`.

## Commit and PR conventions

* One logical change per PR. Small PRs get reviewed; large ones get
  postponed.
* Write the commit message for the person running `git blame` in two
  years: say *why*, not just what.
* Add a line to `CHANGELOG.md` under **Unreleased** for anything a user
  would notice.
* A new rule (a feature type, a socket relation, a verification check)
  needs at least one known-good and one known-bad case in the corpus.

## Sample files

Real STEP files are the scarcest input. If you can share an assembly, or a
part with its accepted replacement, open an issue. Do not commit STEP files
whose license does not permit redistribution; fetch them by script with a
pinned SHA-256 instead, as stepq does. If a file is confidential,
`stepq strip FILE --anonymize -o OUT` removes names and text and changes
no geometry.

## Reporting bugs

Use the issue template. A wrong verdict is only actionable with the files
that produced it, or a minimal reproduction. A false positive (a candidate
accepted that does not fit) is the most serious kind of bug; say so in the
title.

Panics, unbounded memory use or hangs on crafted input are security
issues: report them privately as described in [SECURITY.md](SECURITY.md).

## Licensing

fitment is licensed under the [MIT License](LICENSE). By submitting a pull
request you agree that your contribution is licensed under the same terms,
and you confirm that you have the right to license it that way. Do not add
code copied from projects under incompatible licenses.
