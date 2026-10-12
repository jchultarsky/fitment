# Fixtures

Public STEP files fitment is tested on, downloaded by
`tools/fetch-fixtures.sh` and never committed. The script is stepq's, with
the same URLs and SHA-256 pins; a file that changes upstream fails the
download rather than changing a test.

- `core`: the STEP Tools AS1 assembly as written by five exporters, the
  NIST PMI test cases (AP203 and AP242) and the NIST Engineering Design
  Model repository's AP203 files. About 20 MB. CI fetches this set.
- `ocp`: Open Compute Project assemblies, up to 200 MB each. Manual.

The NIST sets are US Government public domain; the others carry no
redistribution terms, which is why nothing here is committed or uploaded
as a CI artifact.

`manifest.json` pins what fitment's gate makes of each core file:
`accepted`, or `refused` with its reasons (each code once, in order).
`crates/fitment/tests/fixtures.rs` checks every file against it. The pins
are regression pins, not truth: they change only in a pull request that
says why.
