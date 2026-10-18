# fitment-core

The matcher behind [fitment](https://github.com/jchultarsky/fitment): it
tests catalog parts against the *socket* an assembly requires of a part
(holes on given axes, diameter limits, seating planes, grips, conical
seats) and returns a verdict with its evidence.

> **Status: milestone M5 done (0.2.0).** [`read_part`] reads one part from
> a STEP file through the canonical gate; [`read_assembly`] lists an
> assembly's parts; [`SocketDocument::extract`] reads what the neighbours
> of a part require of any part that replaces it; a [`Matcher`] judges
> candidates against that socket, with every check's evidence, and
> [`MatchDocument`] is a run's JSON form (`fitment.match` v1). For a
> catalog, [`Matcher::judge_document`] judges a stored features document
> exactly as its file, [`level_one`] finds candidates over any
> [`IndexSource`] of their [`IndexTokens`], and
> [`FeaturesDocument::read_with_properties`] also captures a part's stated
> properties ([`StatedProperties`], read through [`PropertyMap`]). The
> body fit (M7) is not checked yet: a candidate that passes every
> requirement is an `InterfaceMatch`. See the
> [plan](https://github.com/jchultarsky/fitment/blob/main/docs/PLAN.md).

## Matching

```rust
use fitment_core::{
    MatchOptions, MatchPolicy, Matcher, NoFitVerifier, SocketDocument, VerdictKind,
};

/// The verdict on one candidate part (STEP bytes) for the part named
/// `BRACKET` in an assembly (STEP bytes); `None` if the socket cannot be
/// read from the assembly. A candidate the gate refuses is a `Reject`.
fn judge(assembly: &[u8], candidate: &[u8]) -> Option<VerdictKind> {
    let socket = SocketDocument::extract(assembly, Some("BRACKET"), None).ok()?;
    let matcher = Matcher::new(
        &socket,
        MatchPolicy::default(),
        MatchOptions::default(),
        &NoFitVerifier,
    )
    .ok()?;
    let result = matcher.judge_step(candidate);
    // Every socket's checks, with what was measured and why it failed.
    for socket in &result.sockets {
        for (requirement, check) in &socket.checks {
            let _ = (requirement, check.passed(), check.evidence().failures());
        }
    }
    Some(result.verdict.kind())
}
```

The default policy is D9's tolerances (0.01 mm, 0.01°), D6's strict
clearance and D7's hard grip; `MatchPolicy::new` takes others. A socket
document is JSON the user reviews (`SocketDocument::to_json`,
`from_json`, `decide`); a stale one is refused before any candidate is
read, and with `MatchOptions::new(_, true)` an unreviewed one too;
otherwise each candidate is flagged `requirements_unreviewed`.

## Verdicts

A verdict is built only by [`decide`], so no code path can produce a
`Match` that skipped a check:

- **`Match`**: every socket requirement and the body fit are verified.
- **`InterfaceMatch`**: every requirement is verified; the body fit was not
  checked or could not be verified.
- **`Reject`**: a requirement failed, the evidence was incomplete, or the
  body interferes with a neighbor.

Anything not positively verified rejects: a requirement no candidate can
pass, a bound reached, a candidate file the gate refuses. This crate does
no I/O: no files, network, processes, threads, clock or environment, and
its results do not depend on the platform. Callers hand it values and get
values back.

## License

MIT

[`read_part`]: https://docs.rs/fitment-core/latest/fitment_core/fn.read_part.html
[`read_assembly`]: https://docs.rs/fitment-core/latest/fitment_core/fn.read_assembly.html
[`SocketDocument::extract`]: https://docs.rs/fitment-core/latest/fitment_core/struct.SocketDocument.html#method.extract
[`Matcher`]: https://docs.rs/fitment-core/latest/fitment_core/struct.Matcher.html
[`MatchDocument`]: https://docs.rs/fitment-core/latest/fitment_core/struct.MatchDocument.html
[`decide`]: https://docs.rs/fitment-core/latest/fitment_core/fn.decide.html
