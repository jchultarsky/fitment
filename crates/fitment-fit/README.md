# fitment-fit

The fit verifier's process and protocol for [fitment](https://github.com/jchultarsky/fitment)
(`docs/research/m7-fit.md`). It drives an Open CASCADE helper in a separate
process and hands `fitment-core` the kernel's measures; `fitment-core` alone
decides a fit's status from them. This crate is pure Rust and MIT: no
Open CASCADE, binding or C++ bridge is in it or its dependencies (D1). It is
the only fitment crate that spawns a process.

- **The fit directory** (FL2, FL6): `fitment fit setup` writes the helper's
  script, its lock and `manifest.json`, which names the base interpreter and
  the environment's site-packages. A run never runs uv: it spawns
  `<interpreter> -I <dir>/fit_helper.py --site <site-packages>` with an
  environment built from nothing, after checking the script and lock on disk
  are the ones this build embeds.
- **Frames** (FH1): `FFIT`, a 4-byte big-endian header length, a JSON header,
  then raw payloads; every cap checked before anything is allocated.
- **The protocol** (FH2 to FH5): `hello`, `scene`, `place`, `check`, `bye`,
  one request in flight per helper; every reply echoes its request's nonce,
  the SHA-256 of its header, and core's digest of what it asks, and is decoded
  strictly.
- **The session** (FL1, FL3 to FL5): `OcctSession` implements
  `fitment_core::FitVerifier`, with at most `jobs` long-lived helpers; every
  write and reply has a deadline, and how a helper ended is classified (FV8):
  a crash is retried once, a timeout or resource kill makes the run
  incomplete, and nothing is ever `verified` but by core.

The helper script itself arrives with M7's PR 2b.
