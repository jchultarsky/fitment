# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).
Until 1.0, minor versions may contain breaking changes.

## [Unreleased]

### Added

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
