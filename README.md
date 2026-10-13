# fitment

[![CI](https://github.com/jchultarsky/fitment/actions/workflows/ci.yml/badge.svg)](https://github.com/jchultarsky/fitment/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

Find catalog parts that can replace a part in a STEP assembly.

Given an assembly and one part in it that is no longer available, fitment
suggests catalog parts whose interfaces match: the same hole pattern on the
same axes, diameters within limits, the same seating planes and grip
length. It is deterministic (rules and arithmetic, no machine learning),
written in Rust, and built on [stepq](https://github.com/jchultarsky/stepq).

> **Status: M3 done, M4 next.** `fitment features` reads one part from a
> STEP file, refusing a file that is not a canonical part, with reasons.
> `fitment socket` reads what the neighbours of a part in an assembly
> require of any part that replaces it, as a reviewable socket file, and
> `fitment review` records the user's decisions in it. Matching is not
> built yet. The design is in [docs/PLAN.md](docs/PLAN.md), and the code
> is mapped in [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

## How it works

A candidate is never compared with the original part. It is tested against
a *socket*: what the neighbors in the assembly require of the part.

```text
INGEST, once per catalog part            QUERY, once per part to replace

Catalog part (STEP)                      Assembly and the part to replace
        |                                        |
Gate and feature extraction              Socket extraction (user reviews)
        |                                        |
Feature index  ----------------------->  Shortlist from the index
                                                 |
                                         Align and verify every requirement
                                                 |
                                         Verdict with evidence
```

Each result is one of:

- **Match**: every requirement and the body fit are verified.
- **Interface match**: every requirement is verified; the fit is not checked.
- **Reject**: with the requirements that failed.

Flags such as "thread not confirmed" or "material not compared" ride along,
because a STEP file often does not state them.

## Soundness

False positives are the failure fitment is built to avoid. A candidate
passes only when every requirement is positively verified; anything the
code does not recognize is a rejection; a value at the edge of the
tolerance rejects; and every verdict states what was and was not checked.
The full rules are in [docs/PLAN.md](docs/PLAN.md#soundness-rules).

## Reading a part

`fitment features` prints a part's features as `features.v1` JSON (the
default), or one line per feature for people:

```console
$ fitment features --format table p05_cbore.stp
schema    ap214   Open CASCADE 7.9
part      #5 'p05_cbore'   frame #10
solid-15    MANIFOLD_SOLID_BREP
faces     9 (7 planar, 2 cylindrical, 0 conical, 0 toroidal, 0 other)   complete true
plane-17        normal (-1, 0, 0) offset 0   area 600   1 face(s)
plane-57        normal (0, -1, 0) offset 0   area 600   1 face(s)
plane-88        normal (0, 0, 1) offset 15   area 1504.9668   1 face(s)
plane-123       normal (0, 1, 0) offset 40   area 600   1 face(s)
plane-147       normal (0, 0, -1) offset 0   area 1565.7881   1 face(s)
plane-175       normal (1, 0, 0) offset 40   area 600   1 face(s)
plane-237       normal (0, 0, 1) offset 8.5   area 60.8212   1 face(s)
cylinder-187    hole Ø11 full   axis (0, 0, 1) through (20, 20, 8.5)
cylinder-212    hole Ø6.6 full   axis (0, 0, 1) through (20, 20, 0)
hole-187        through Ø6.6/Ø11 length 15   opening@0 → opening@15
```

It exits 0 for a part, 3 for a file it refuses (the reasons go to stderr
and into the JSON), 1 on an error and 2 on a usage error. Where a rule
cannot decide, it says so rather than guessing: a hole whose wall ends on
a curve stepq does not read is an `undecided` cylinder and no hole, a
hole whose end the rules cannot read has an `unknown` end, and a plane
with an unread edge has an `unknown` outline.

## Reading an assembly

`fitment parts` lists an assembly's parts, and `fitment socket` reads what
the neighbours of one of them require of any replacement: holes on given
axes with diameter limits (bores), shafts, seating planes and grip lengths,
and, where a rule cannot decide, `unresolved` requirements that fail every
candidate until a reviewer removes them.

```console
$ fitment parts s01_bolted.stp
id                       name                     kind      quantity definition  first occurrence
PLATE                    PLATE                    part             1        #63  1
BRACKET                  BRACKET                  part             1       #388  2
BOLT                     BOLT                     part             4       #752  3
NUT                      NUT                      part             4      #1032  4
s01_bolted               s01_bolted               assembly         1         #5  (root)
assembly gate: accepted
$ fitment socket --format table s01_bolted.stp --part BRACKET
socket-1  1 occurrences
  -        seat:plane-467                       1 faces, 4 contacts
  -        grip:plane-467+plane-570             8 mm thick, 5 holes through
  -        seat:plane-570                       1 faces, 1 contacts
  -        bore:hole-654                        0..8 Ø6.6 clearance clear Ø6 open at start
  -        bore:hole-671                        0..8 Ø6.6 clearance clear Ø6 open at start
  -        bore:hole-688                        0..8 Ø6.6 clearance clear Ø6 open at start
  -        bore:hole-722                        0..8 Ø6.6 clearance clear Ø6 open at start
  left out hole-705 Ø8 (NothingOnAxis)
```

The default output is the `fitment.socket` JSON document, with a digest
over everything fitment extracted. `fitment review socket.json --keep ID`
or `--remove ID --note "..."` records a decision outside the digest, and
`fitment socket -o socket.json` on the same file again carries decisions
over to requirements that did not change. Occurrences of a part that ask
the same of it share one socket.

## Planned interface

```console
$ fitment match socket.json candidates/*.stp      # verdicts with evidence
```

A catalog with create, read, update and delete, and a REST API over the
catalog and matcher, follow once the matcher passes its zero-false-positive
test.

## Roadmap

| Milestone | Delivers |
| --- | --- |
| M0 | Repository, workspace, plan, Vectera audit, decision D1 |
| M1 | stepq 0.6: units, geometry values, placements, topology, solids, PMI to faces |
| M2 | Features of one part |
| M3 | Socket extraction |
| M4 | Matching, evidence, and the known-answer corpus |
| M5 | Catalog |
| M6 | REST API |
| M7 | Optional body-fit verifier |
| M8 | Viewer |

Details and exit criteria are in [docs/PLAN.md](docs/PLAN.md#milestones).

## Contributing

Contributions are welcome. Please read [CONTRIBUTING.md](CONTRIBUTING.md)
and the [Code of Conduct](CODE_OF_CONDUCT.md). Security issues go through
[SECURITY.md](SECURITY.md), not public issues.

New to STEP? [*Inside the STEP File*](https://jchultarsky.github.io/step-book/)
explains the entity model fitment and stepq rely on.

## License

[MIT](LICENSE) © 2026 Julian Chultarsky
