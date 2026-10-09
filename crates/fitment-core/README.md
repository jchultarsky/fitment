# fitment-core

The matcher behind [fitment](https://github.com/jchultarsky/fitment): it
tests catalog parts against the *socket* an assembly requires of a part
(holes on given axes, diameter limits, seating planes) and returns a
verdict with its evidence.

> **Status: milestone M0.** Only the verdict types exist so far. Feature
> extraction, sockets and matching arrive in milestones M2 to M4; see the
> [plan](https://github.com/jchultarsky/fitment/blob/main/docs/PLAN.md).

## Verdicts

A verdict is built only by [`decide`], so no code path can produce a
`Match` that skipped a check:

- **`Match`**: every socket requirement and the body fit are verified.
- **`InterfaceMatch`**: every requirement is verified; the body fit was not
  checked or could not be verified.
- **`Reject`**: a requirement failed, the evidence was incomplete, or the
  body interferes with a neighbor.

Anything not positively verified rejects. This crate does no I/O: no
files, network, processes, threads, clock or environment, and its results
do not depend on the platform. Callers hand it values and get values back.

## License

MIT
