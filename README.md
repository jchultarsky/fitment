# fitment

[![CI](https://github.com/jchultarsky/fitment/actions/workflows/ci.yml/badge.svg)](https://github.com/jchultarsky/fitment/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

Find catalog parts that can replace a part in a STEP assembly.

Given an assembly and one part in it that is no longer available, fitment
suggests catalog parts whose interfaces match: the same hole pattern on the
same axes, diameters within limits, the same seating planes and grip
length. It is deterministic (rules and arithmetic, no machine learning),
written in Rust, and built on [stepq](https://github.com/jchultarsky/stepq).

> **Status: early (M0 done, M1 next).** The workspace and the verdict
> types exist; nothing that reads a STEP file does yet. The design is in
> [docs/PLAN.md](docs/PLAN.md); the commands below are the intended
> interface, not a working one.

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

## Planned interface

```console
$ fitment parts assembly.stp                      # list parts and quantities
$ fitment features bracket.stp                    # holes, shafts, planes as JSON
$ fitment socket assembly.stp --part BRK-100      # reviewable socket file
$ fitment match socket.json candidates/*.stp      # verdicts with evidence
```

A catalog with create, read, update and delete, and a REST API over the
catalog and matcher, follow once the matcher passes its zero-false-positive
test.

## Roadmap

| Milestone | Delivers |
| --- | --- |
| M0 | Repository, workspace, plan, Vectera audit, decision D1 |
| M1 | stepq 0.5: placements, geometry values, topology, units |
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
