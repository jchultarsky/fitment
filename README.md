# fitment

[![CI](https://github.com/jchultarsky/fitment/actions/workflows/ci.yml/badge.svg)](https://github.com/jchultarsky/fitment/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

Find catalog parts that can replace a part in a STEP assembly.

Given an assembly and one part in it that is no longer available, fitment
suggests catalog parts whose interfaces match: the same hole pattern on the
same axes, diameters within limits, the same seating planes and grip
length. It is deterministic (rules and arithmetic, no machine learning),
written in Rust, and built on [stepq](https://github.com/jchultarsky/stepq).

> **Status: M6 done (the REST API).** `fitment-server` serves the catalog
> and the matcher over HTTP, with jobs, uploads by SHA-256, sockets with
> revisions, bearer tokens and roles; every result document it serves is
> byte for byte what the CLI prints for the same inputs. Every match case
> of the corpus passes through the API, under both policies, with no false
> positive and the library's report to the byte. It ships as a container
> image and is not published as a crate. Version 0.3.0 (M6) follows 0.2.0
> (M5, the catalog) and 0.1.0 (M4).
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
  bytes. `delete`, `prune`, `verify` and `gc` take `--format json` as the
  other commands do: `fitment.catalog` with its `changes` for the first
  two, and `fitment.verify` and `fitment.gc`, which name no path, for the
  others.
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
catalog` and `fitment match --catalog` exit 2 and say so.

## Running the server

`fitment-server` is the REST API over one catalog and the matcher. It is
not published as a crate: build it from this repository (`cargo build
--release -p fitment-server`), or build the container image, which holds
the server and the `fitment` CLI of the same version and no Python, uv or
Open CASCADE:

```console
$ docker build -f deploy/Containerfile -t fitment-server .
$ docker run --rm -v cat:/var/lib/fitment/catalog --entrypoint fitment \
    fitment-server catalog init -C /var/lib/fitment/catalog
$ docker run -d --name fitment --read-only -p 127.0.0.1:8080:8080 \
    -v cat:/var/lib/fitment/catalog -v state:/var/lib/fitment/state fitment-server
$ docker exec fitment fitment-server admin bootstrap --username ada
fitment_pat_…
```

Outside the image, `fitment-server serve --catalog DIR --state DIR`
serves an existing catalog (make it with `fitment catalog init`) and keeps
its own state, users, uploads, sockets and jobs, in the state directory;
the two must not nest. It listens on `127.0.0.1:8080` unless told
otherwise, writes JSON logs on stderr and one line on stdout, the address
it bound. Every setting is a key of a TOML file (`--config`, or
`FITMENT_SERVER_CONFIG`), and each key can be set by a variable such as
`FITMENT_SERVER__LIMITS__MAX_UPLOAD_MIB`; unknown keys refuse the start,
and `fitment-server check-config` checks a configuration without starting.
`deploy/fitment-server.toml`, the image's own, shows the keys a deployment
usually sets. SIGTERM or Ctrl-C stops it gracefully: a running read or
match goes back to the queue for the next start, and a catalog change cut
short keeps the batches it committed and says so.

- **The first admin** is made offline, beside the running server or
  before it starts: `fitment-server admin bootstrap --state DIR --username
  NAME` prints an admin's token to stdout, once. `admin user` and `admin
  token` manage users and tokens the same way, and are the way back in if
  every admin token is lost. Through the API, `POST /v1/auth/login` turns
  a password into a token, `POST /v1/auth/tokens` mints a token within the
  presenting one's role and expiry (a read-only token for CI, say), and an
  admin manages users under `/v1/auth/users`. Every request but health,
  readiness, the OpenAPI document and login carries `Authorization: Bearer
  TOKEN`; roles are reader, writer (changes the catalog) and admin.
- **Passwords** are hashed with argon2id. Give the server a pepper, a file
  of 16 to 1,024 random bytes that only it can read (`auth.pepper_file`),
  and pass the same configuration to `fitment-server admin`; without one
  it warns at start.
- **TLS** is a reverse proxy's job: the server speaks plain HTTP/1.1. List
  the proxy's own addresses in `server.trusted_proxies`, so that the
  client's address, on which login limits are kept, is read from
  `X-Forwarded-For`.
- **Where to run it:** one server per catalog, both directories on a local
  disk, never a network file system or a sync folder: SQLite's write-ahead
  log needs one host's shared memory, and a second server on the catalog
  is not supported. The `fitment` CLI may work on the same catalog while
  the server runs, under the catalog's own locking; the server sees its
  changes at its next snapshot. Back up both directories; a catalog
  copied elsewhere is trusted after `fitment catalog verify --deep`.

A match over HTTP, the s01 case above: the bytes go in by their SHA-256,
work that reads STEP runs as a job, and `Prefer: wait=N` (at most 60 s)
answers with the finished job:

```console
$ curl -X PUT --data-binary @s01_bolted.stp -H "$AUTH" \
    -H 'Content-Type: application/octet-stream' localhost:8080/v1/uploads/sha256:$ASM
$ curl -H "$AUTH" -H 'Prefer: wait=30' -H 'Content-Type: application/json' \
    -d '{"upload": "sha256:'$ASM'", "file_name": "s01_bolted.stp"}' localhost:8080/v1/assemblies
$ curl -H "$AUTH" -H 'Prefer: wait=30' -H 'Content-Type: application/json' \
    -d '{"assembly": "sha256:'$ASM'", "part": "BRACKET"}' localhost:8080/v1/sockets
{"format": "fitment.job", "kind": "socket_extract", "state": "succeeded", "exit_code": 0,
 "result": {"href": "/v1/sockets/sk_94a02d9adb35b429e8d1@r1", …}, …}
$ curl -X PUT --data-binary @m_bracket_blind.stp -H "$AUTH" \
    -H 'Content-Type: application/octet-stream' localhost:8080/v1/uploads/sha256:$CAND
$ curl -H "$AUTH" -H 'Prefer: wait=30' -H 'Content-Type: application/json' \
    -d '{"socket": "sk_94a02d9adb35b429e8d1@r1", "candidates": {"files":
         [{"upload": "sha256:'$CAND'", "label": "m_bracket_blind.stp"}]}}' localhost:8080/v1/matches
{"format": "fitment.job", "id": "j_…", "kind": "match", "state": "succeeded", "exit_code": 5,
 "outcome": "reject", …}
$ curl -H "$AUTH" localhost:8080/v1/matches/j_…/document
```

The socket at `/v1/sockets/sk_…@r1` is byte for byte the file `fitment
socket s01_bolted.stp --part BRACKET` writes, and the match document what
`fitment match` prints with `--format json`, its exit code in the
`Fitment-Exit-Code` header. A review is `POST /v1/sockets/sk_…@r1/review`,
which makes `@r2`; `candidates` may also name leaves of your registered
assemblies (`components`) or the catalog (`{"catalog": {"selection":
"full"}}`). The catalog's commands are under `/v1/catalog`, each a job or
a request with the CLI's document. `GET /v1/openapi.json` describes every
operation, document and error code; an error is `application/problem+json`
with a stable `code`, never a path. A job states its progress at
`/v1/jobs/{id}/events` and stops on `POST /v1/jobs/{id}/cancel`; a match
that did not succeed has no verdict, anywhere.

## Roadmap

| Milestone | Delivers |
| --- | --- |
| M0 | Repository, workspace, plan, Vectera audit, decision D1 |
| M1 | stepq 0.6: units, geometry values, placements, topology, solids, PMI to faces |
| M2 | Features of one part |
| M3 | Socket extraction |
| M4 | Matching, evidence, and the known-answer corpus (done, 0.1.0) |
| M5 | Catalog (done, 0.2.0) |
| M6 | REST API (done, 0.3.0) |
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
