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

- **The helper** (FG2, FH, FY3): `helper/fit_helper.py` and its lock
  `helper/fit_helper.py.lock` are embedded in this crate (`include_bytes!`,
  each with its SHA-256) and written to the fit directory by
  `fitment_fit::setup`, `fitment fit setup`'s internals: the only step that runs
  uv, and the only one that may download (Python 3.13 and the pinned
  `cadquery-ocp-novtk` 8.0.1.1.0 wheel). The helper moves its standard
  output onto stderr before it imports Open CASCADE, sets its own bounds
  (`RLIMIT_AS` on Linux, a per-request CPU limit and alarm on Unix, a job
  object on Windows, `PR_SET_PDEATHSIG` on Linux), reads each assembly
  through XCAF with its faces bound to their STEP `#id`s, and measures
  Commons, cuts and distances with non-destructive `BOPAlgo_BOP` booleans,
  every alert and exception reported per pair. It decides nothing.
- **Windows** (M7-50): until the weekly Windows corpus run is green, a
  helper on Windows is refused after its handshake, so no fit is verified
  there.

## Licence

This crate is MIT, and so is the helper's script: it is fitment's own text.
The crate contains no Open CASCADE, binding or library, only that text and
the lock that names the wheels uv fetches on the user's machine. The helper
imports Open CASCADE (the `cadquery-ocp-novtk` wheel, LGPL-2.1 with its
exception, and the libraries the wheel bundles) at run time from the
environment `fitment fit setup` made; whoever builds an image with that
environment in it distributes Open CASCADE and owes its notices (FX8).
