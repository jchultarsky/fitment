# Contributing

Thanks for your interest. This document explains how the repository is
organised and what a good contribution looks like.

By participating you agree to follow the [Code of Conduct](CODE_OF_CONDUCT.md).

> fitment is at milestone M0 (planning). Until the workspace lands, the
> most useful contributions are comments on [docs/PLAN.md](docs/PLAN.md)
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
  values; it does not parse STEP itself.
* **The matcher has no I/O.** No storage, network or async code in the
  matcher crate; that belongs in the catalog and API crates.
* **No `unsafe`.**

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
$ cargo test --workspace --all-features
```

The test corpus is generated, not vendored: scripts build assemblies and
candidates with known answers using [cadquery](https://cadquery.readthedocs.io/)
on Open CASCADE. They run through [uv](https://docs.astral.sh/uv/), which
installs the Python dependencies on first use.

## Before you open a pull request

CI runs exactly these; run them locally first:

```console
$ cargo fmt --all -- --check
$ cargo clippy --workspace --all-targets --all-features -- -D warnings
$ cargo test --workspace --all-features
$ RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features
$ cargo deny check
```

Changes to feature extraction, socket extraction or verification must keep
the false-positive count on the known-bad corpus at zero, and should report
any change in the false-negative rate in the PR description.

Pedantic clippy is on. If a lint is wrong for a specific line, `allow` it
there with a comment; do not disable it workspace-wide without discussion.

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
