# fitment

[![CI](https://github.com/jchultarsky/fitment/actions/workflows/ci.yml/badge.svg)](https://github.com/jchultarsky/fitment/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

Find catalog parts that can replace a part in a STEP assembly.

Given an assembly and one part in it that is no longer available, fitment
suggests catalog parts whose interfaces match: the same hole pattern on the
same axes, diameters within limits, the same seating planes and grip
length. It is deterministic (rules and arithmetic, no machine learning),
written in Rust, and built on [stepq](https://github.com/jchultarsky/stepq).

> **Status: M5 done (the catalog).** `fitment catalog` stores STEP parts
> by content, gated and indexed, and `fitment match --catalog` searches
> them: a catalog run is the file run over the parts its index admits,
> digest for digest, and on the 10⁵-part scaled corpus every full run
> holds every part an exhaustive run passes. Version 0.2.0 (M5) follows
> 0.1.0 (M4).
>
> `fitment features` reads one part from a
> STEP file, refusing a file that is not a canonical part, with reasons.
> `fitment socket` reads what the neighbours of a part in an assembly
> require of any part that replaces it, as a reviewable socket file, and
> `fitment review` records the user's decisions in it. `fitment match`
> judges candidate parts against it, with the evidence. On its
> known-answer corpus of 125 match cases it passes no candidate whose
> interface does not fit, under either policy (a body that collides with
> a neighbour no requirement concerns is M7's, and passes as an
> `InterfaceMatch`); it rejects 20 of the 49 that fit (21 of 51 under
> functional clearance), each by a rule it states. An independent Open
> CASCADE oracle checks the truths it can judge physically, and every
> pass and failure fitment reports. The design is in
> [docs/PLAN.md](docs/PLAN.md), and the code is mapped in
> [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

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
fitment: s01_bolted.stp: BRACKET: 1 occurrences, 1 sockets, 7 requirements (7 unreviewed), 1 left out
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

`fitment match socket.json candidates/` judges each candidate against
the socket: it is placed where the original was, from features that
match, and every requirement is checked, with the measures and failures
behind each result.

```console
$ fitment match bracket.socket.json m_bracket.stp m_bracket_missing_hole.stp m_bracket_blind.stp
socket sha256:34a9bf48c882  target 'BRACKET'  1 socket(s)  policy 0.01 mm / 0.01 deg, clearance strict, grip hard  fit: not checked (no fit verifier in this build)
INTERFACE  m_bracket.stp  1b77ea97  all requirements verified; material_not_compared, requirements_unreviewed
    socket-1/seat:plane-467  passed  plane-88  margin 0.01 mm
  …
reject     m_bracket_missing_hole.stp  1e4e1790  1 failed: socket-1/bore:hole-688 no_feature
reject     m_bracket_blind.stp  a8ed725f  1 failed: socket-1/bore:hole-722 core_intruded,end_not_open,wall_missing
```

`--format json` prints the `fitment.match` document. The exit code is the
best verdict: 0 a match, 4 an interface match (the body's fit is not
checked until M7, so every passing run exits 4), 5 every candidate
rejected; 1 an error, 2 usage, 3 a socket document refused.
`--clearance functional` drops a clearance hole's upper diameter bound
only (D6): its seats must still be covered, and a pass strict would
refuse carries `larger_than_original`; `--tolerance-mm` and `--tolerance-deg` set the policy;
`--require-reviewed` refuses a socket with unreviewed requirements;
`--original ASM` also judges the original part from the socket's own
assembly, as a check of fitment itself.

## Working with a catalog

A catalog is a directory holding a SQLite database and every stored file,
by its SHA-256. `ingest` gates each file once, as `fitment features` does,
and indexes what it reads; a file the gate refuses is quarantined with its
reasons and never searched.

```console
$ fitment catalog init -C parts
fitment: parts: catalog created
$ fitment catalog ingest -C parts in/
fitment: parts: 7 files: 7 added
added        m_bracket            1b77ea97  'm_bracket'  5 holes, 0 shafts, 6 planes
…
$ fitment catalog list -C parts
fitment: parts: generation 2: 7 entries: 7 indexed, 0 quarantined, 0 stale
id                        rev  status               sha256   holes shafts planes  product           file
m_bracket                 r1   indexed              1b77ea97     5      0      6  'm_bracket'       m_bracket.stp
…
$ fitment match bracket.socket.json -C parts
fitment: bracket.socket.json: catalog parts  generation 2  entries 7: 7 searched, 0 quarantined, not searched  level 1 kept 7
…
INTERFACE  catalog:m_bracket@r1  1b77ea97  'm_bracket'  all requirements verified; material_not_compared, requirements_unreviewed
INTERFACE  catalog:m_bracket_lug@r1  1b9724ef  'm_bracket_lug'  all requirements verified; material_not_compared, requirements_unreviewed
reject     catalog:m_bracket_missing_hole@r1  1e4e1790  'm_bracket_missing_hole'  1 failed: socket-1/bore:hole-688 no_feature
…
```

A catalog match reads the index alone, never a stored file. Its document
is the `fitment.match` document a file run over the same files gives,
each candidate labelled `catalog:<id>@r<revision>`. `--id ID` judges
named entries; `--exhaustive` judges every entry, the index aside; `--jobs
N` judges N candidates at once and changes no byte of the result.

- **Entries:** `get ID` shows an entry, its stated properties and its
  history; `get ID --format step -o FILE` writes its bytes, checked
  against their SHA-256 first. `replace ID FILE` gives an entry a new
  revision and keeps the old one's bytes for `get ID@rN`; `delete ID...`
  deletes all or none; `prune --revisions` releases kept revisions'
  bytes.
- **Upgrades:** a fitment that reads parts differently marks the catalog
  stale, and every match refuses it until `fitment catalog reindex -C
  DIR` has brought each entry up to date. A part that a new gate no
  longer accepts is reported "no longer searched", and `reindex` exits 3.
  `gc --versions` then removes the older rows.
- **Upkeep:** run `fitment catalog verify -C DIR` regularly. It checks
  every stored file, every row's hash and every index posting. A
  damaged posting can hide a part from a match until `verify` reports it
  (it can never pass one); when a part you expected is missing, run the
  match again with `--exhaustive`. `gc` removes orphan files; `gc
  --vacuum` compacts the database and needs free space about its size.
- **Copying a catalog:** a copied or received catalog is trusted only
  after `fitment catalog verify --deep`, which reads every part again
  from its stored file: a database whose rows are consistent with each
  other can still state a hole the file lacks, and a match believes the
  index.
- **Where to keep it:** on a local disk. SQLite's write-ahead log is not
  safe on network file systems, and a folder a sync service copies while
  fitment writes can give a torn copy.

A build without the `catalog` cargo feature has no catalog: `fitment
catalog` and `fitment match --catalog` exit 2 and say so. A REST API over
the catalog and the matcher is M6.

## Roadmap

| Milestone | Delivers |
| --- | --- |
| M0 | Repository, workspace, plan, Vectera audit, decision D1 |
| M1 | stepq 0.6: units, geometry values, placements, topology, solids, PMI to faces |
| M2 | Features of one part |
| M3 | Socket extraction |
| M4 | Matching, evidence, and the known-answer corpus (done, 0.1.0) |
| M5 | Catalog (done, 0.2.0) |
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
