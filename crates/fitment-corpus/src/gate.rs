//! The strict gate of a match case (`docs/research/m4-match.md`, KH2,
//! KH3) on an [`Outcome`] (`docs/research/m6-api.md`, SC2): the verdict is
//! the one expected, the requirements that failed are those the case
//! names, each with a code it allows, and every requirement the case's
//! review removes is listed as removed and not checked (KC13, SC8).
//! Requirements are named by locators (KC5): what they constrain, in the
//! target's frame; a locator that names none, or several, is a harness
//! error, never a pass (SC4). The defect check at the construction's
//! placement (KH8) stays typed: only the library can place a candidate by
//! the construction. Includable by path (SC6): it names only
//! `crate::outcome`, `fitment_core` and `serde_json`.

use fitment_core::{
    Requirement, Requires, ReviewStatus, Rigid, Socket, SocketDocument, SocketResult,
};
use serde_json::Value;

use crate::outcome::Outcome;

/// How far a locator's numbers may lie from the requirement's: the
/// manifest rounds them to 9 decimals.
const NEAR: f64 = 1e-6;

fn array(v: &Value) -> Option<[f64; 3]> {
    let a = v.as_array()?;
    Some([
        a.first()?.as_f64()?,
        a.get(1)?.as_f64()?,
        a.get(2)?.as_f64()?,
    ])
}

fn near3(a: [f64; 3], b: [f64; 3]) -> bool {
    (0..3).all(|k| (a[k] - b[k]).abs() <= NEAR)
}

/// True if the line through `point` along `d` is the line through
/// `other` along `e`, whatever their signs.
fn same_line(point: [f64; 3], d: [f64; 3], other: [f64; 3], e: [f64; 3]) -> bool {
    let cross = [
        d[1] * e[2] - d[2] * e[1],
        d[2] * e[0] - d[0] * e[2],
        d[0] * e[1] - d[1] * e[0],
    ];
    let w = [
        other[0] - point[0],
        other[1] - point[1],
        other[2] - point[2],
    ];
    let along = w[0] * d[0] + w[1] * d[1] + w[2] * d[2];
    let off = [
        w[0] - along * d[0],
        w[1] - along * d[1],
        w[2] - along * d[2],
    ];
    cross.iter().all(|c| c.abs() <= NEAR) && off.iter().all(|c| c.abs() <= NEAR)
}

/// True if `locator` names `requirement`.
fn names(locator: &Value, requirement: &Requirement) -> bool {
    let kind = locator["kind"].as_str().unwrap_or("");
    match &requirement.requires {
        Requires::Bore { axis, .. } | Requires::Shaft { axis, .. } => {
            let own = if matches!(requirement.requires, Requires::Bore { .. }) {
                "bore"
            } else {
                "shaft"
            };
            kind == own
                && array(&locator["axis_point"])
                    .zip(array(&locator["axis_direction"]))
                    .is_some_and(|(p, d)| {
                        same_line(axis.point_mm.to_array(), axis.direction.to_array(), p, d)
                    })
        }
        Requires::Seat {
            normal, offset_mm, ..
        } => {
            kind == "seat"
                && array(&locator["normal"]).is_some_and(|n| near3(n, normal.to_array()))
                && locator["offset"]
                    .as_f64()
                    .is_some_and(|o| (o - offset_mm).abs() <= NEAR)
        }
        Requires::Grip { length_mm, .. } => {
            kind == "grip"
                && locator["length"]
                    .as_f64()
                    .is_some_and(|l| (l - length_mm).abs() <= NEAR)
        }
        Requires::ConicalSeat { apex_mm, .. } => {
            kind == "conical_seat"
                && array(&locator["apex"]).is_some_and(|a| near3(a, apex_mm.to_array()))
        }
        _ => false,
    }
}

/// The one requirement of `socket` `locator` names; none or several is a
/// harness error, never a pass (KC5, SC4).
pub fn locate<'s>(socket: &'s Socket, locator: &Value) -> Result<&'s Requirement, String> {
    let found: Vec<&Requirement> = socket
        .requirements
        .iter()
        .filter(|r| names(locator, r))
        .collect();
    match found.as_slice() {
        [one] => Ok(one),
        [] => Err(format!("{locator} names no requirement of {}", socket.id)),
        _ => Err(format!(
            "{locator} names several requirements of {}",
            socket.id
        )),
    }
}

/// The requirement `value` names in every socket that has it, as
/// `(socket, requirement)`: one per socket at most, and in some socket.
fn named(sockets: &[Socket], value: &Value) -> Result<Vec<(String, String)>, String> {
    let mut found = Vec::new();
    let mut problems = Vec::new();
    for socket in sockets {
        match locate(socket, value) {
            Ok(r) => found.push((socket.id.clone(), r.id.clone())),
            Err(e) if e.contains("several") => return Err(e),
            Err(e) => problems.push(e),
        }
    }
    if found.is_empty() {
        return Err(problems.join("; "));
    }
    Ok(found)
}

/// An expected failure: the requirement it names, in each socket that has
/// it (`socket-1/bore:hole-88`), and the codes it allows (any, when none
/// are listed).
struct Entry {
    requirements: Vec<String>,
    codes: Vec<String>,
}

fn entry(sockets: &[Socket], value: &Value) -> Result<Entry, String> {
    let requirements = named(sockets, value)?
        .into_iter()
        .map(|(s, r)| format!("{s}/{r}"))
        .collect();
    let Some(listed) = value["codes"].as_array() else {
        return Err(format!("{value} lists no codes"));
    };
    let codes = listed
        .iter()
        .filter_map(Value::as_str)
        .filter(|c| !c.is_empty())
        .map(str::to_owned)
        .collect();
    Ok(Entry {
        requirements,
        codes,
    })
}

impl Entry {
    fn meets(&self, requirement: &str, codes: &[String]) -> bool {
        self.requirements.iter().any(|r| r == requirement)
            && (self.codes.is_empty() || codes.iter().any(|c| self.codes.contains(c)))
    }
}

/// One decision of a case's review (M6-38): a requirement of a socket,
/// the status the reviewer gave it and the note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    /// The socket's id.
    pub socket: String,
    /// The requirement's id.
    pub requirement: String,
    /// `removed`, `kept` or `unreviewed`.
    pub status: String,
    /// Why; required to remove.
    pub note: Option<String>,
}

/// The decisions `case`'s review states, in order: each entry's locator
/// resolved in every socket that has it (KC5), so a review never names a
/// requirement by an id the construction does not know.
pub fn decisions(case: &Value, sockets: &[Socket]) -> Result<Vec<Decision>, String> {
    let mut out = Vec::new();
    for value in case["socket"]["review"].as_array().into_iter().flatten() {
        let status = value["status"]
            .as_str()
            .ok_or_else(|| format!("{value} states no status"))?;
        for (socket, requirement) in named(sockets, value)? {
            out.push(Decision {
                socket,
                requirement,
                status: status.to_owned(),
                note: value["note"].as_str().map(str::to_owned),
            });
        }
    }
    Ok(out)
}

/// `case`'s review applied to `document`, a socket document as extracted
/// from the case's source: each decision as `fitment review` records it.
pub fn apply_review(case: &Value, document: &mut SocketDocument) -> Result<(), String> {
    let Some(extracted) = document.extracted() else {
        return Ok(());
    };
    for d in decisions(case, &extracted.sockets.clone())? {
        let status = match d.status.as_str() {
            "removed" => ReviewStatus::Removed,
            "kept" => ReviewStatus::Kept,
            _ => ReviewStatus::Unreviewed,
        };
        document
            .decide(&d.requirement, Some(&d.socket), status, d.note.as_deref())
            .map_err(|e| format!("{}/{}: {e}", d.socket, d.requirement))?;
    }
    Ok(())
}

/// The expectation of `case` under `policy`: an object, or the name of the
/// policy it equals.
pub fn expectation<'c>(case: &'c Value, policy: &str) -> &'c Value {
    let mut at = &case["expect"][policy];
    // Each name once at most: a cycle ends at no expectation, which fails.
    for _ in 0..4 {
        match at.as_str() {
            Some(other) => at = &case["expect"][other],
            None => return at,
        }
    }
    &Value::Null
}

/// The failed checks of a typed socket result: `socket/requirement` and
/// codes.
fn typed_failures(result: &SocketResult) -> Vec<(String, Vec<String>)> {
    result
        .checks
        .iter()
        .filter(|(_, c)| !c.passed())
        .map(|(r, c)| {
            let codes = c
                .evidence()
                .failures()
                .iter()
                .map(|f| serde_json::to_value(f.code).unwrap_or_default())
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect();
            (format!("{}/{}", result.socket, r.as_str()), codes)
        })
        .collect()
}

/// The failed checks of an outcome: `socket/requirement` and codes.
fn failures(outcome: &Outcome) -> Vec<(String, Vec<String>)> {
    outcome
        .sockets
        .iter()
        .flat_map(|s| {
            s.failed
                .iter()
                .map(move |(r, codes)| (format!("{}/{r}", s.socket), codes.clone()))
        })
        .collect()
}

/// The construction's placement of `case` (KC2: its 3 x 4 rows).
pub fn placement(case: &Value) -> Option<Rigid> {
    let rows = case["placement"].as_array()?;
    let row = |i: usize| -> Option<[f64; 4]> {
        let r = rows.get(i)?.as_array()?;
        Some([
            r.first()?.as_f64()?,
            r.get(1)?.as_f64()?,
            r.get(2)?.as_f64()?,
            r.get(3)?.as_f64()?,
        ])
    };
    let (a, b, c) = (row(0)?, row(1)?, row(2)?);
    Rigid::new(
        [[a[0], a[1], a[2]], [b[0], b[1], b[2]], [c[0], c[1], c[2]]],
        [a[3], b[3], c[3]],
    )
}

/// KH8 for a bad case: at the construction's own placement, every
/// requirement its defect breaks fails, with a code it allows; wherever
/// alignment placed it, the defect is there.
pub fn defect(case: &Value, sockets: &[Socket], at_construction: &[SocketResult]) -> Vec<String> {
    let id = case["id"].as_str().unwrap_or("?");
    let failed: Vec<(String, Vec<String>)> =
        at_construction.iter().flat_map(typed_failures).collect();
    let mut problems = Vec::new();
    for value in case["defect"].as_array().into_iter().flatten() {
        match entry(sockets, value) {
            Ok(e) if failed.iter().any(|(r, codes)| e.meets(r, codes)) => {}
            Ok(e) => problems.push(format!(
                "{id}: at the construction's placement {:?} did not fail as its defect; failed: {failed:?}",
                e.requirements
            )),
            Err(e) => problems.push(format!("{id}: harness: {e}")),
        }
    }
    problems
}

/// CF2, VD10: a file M2's gate refuses is a reject with its codes. VD5,
/// VD6: a case may state how many requirements fail every candidate (its
/// socket's construction says), each with no other code, or that all do
/// (a floor past the tolerance, VR2); those leave `failed`. Returns the
/// count stated.
fn blocked(
    at: &str,
    sockets: &[Socket],
    expect: &Value,
    outcome: &Outcome,
    failed: &mut Vec<(String, Vec<String>)>,
    problems: &mut Vec<String>,
) -> Option<u64> {
    if let Some(codes) = expect["refused"].as_array() {
        let mut want: Vec<&str> = codes.iter().filter_map(Value::as_str).collect();
        let mut got: Vec<&str> = outcome.refusals.iter().map(String::as_str).collect();
        want.sort_unstable();
        got.sort_unstable();
        got.dedup();
        if outcome.status != "refused" || got != want {
            problems.push(format!("{at}: refused with {got:?}, expected {want:?}"));
        }
    }
    // "all": every requirement the sockets state, not every check made.
    let all: u64 = sockets.iter().map(|s| s.requirements.len() as u64).sum();
    let n = match &expect["unverifiable"] {
        Value::String(s) if s == "all" => Some(all),
        v => v.as_u64(),
    }?;
    let is_blocked = |codes: &[String]| {
        !codes.is_empty()
            && codes
                .iter()
                .all(|c| c == "unverifiable" || c == "floor_above_tolerance")
    };
    let count = failed.iter().filter(|(_, codes)| is_blocked(codes)).count() as u64;
    if count != n {
        problems.push(format!(
            "{at}: {count} requirements fail every candidate, expected {n}"
        ));
    }
    failed.retain(|(_, codes)| !is_blocked(codes));
    Some(n)
}

/// VD12, VR19: the flags a case names are on the verdict, and
/// `thread_not_confirmed` only where named.
fn flags(at: &str, expect: &Value, outcome: &Outcome) -> Vec<String> {
    let got = &outcome.flags;
    let want: Vec<&str> = expect["flags"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let mut out: Vec<String> = want
        .iter()
        .filter(|f| !got.iter().any(|g| g == *f))
        .map(|f| format!("{at}: no flag {f}"))
        .collect();
    let thread = "thread_not_confirmed";
    // A reject names no flags: its passed checks may carry one.
    let passing = expect["verdict"] != "reject";
    if passing && got.iter().any(|g| g == thread) && !want.contains(&thread) {
        out.push(format!("{at}: flagged {thread}, not named"));
    }
    out
}

/// KC13, VD7: each requirement the case's review removes is listed as
/// removed in its socket and never checked; nothing else is removed.
fn removals(at: &str, case: &Value, sockets: &[Socket], outcome: &Outcome) -> Vec<String> {
    if outcome.status != "accepted" {
        return Vec::new();
    }
    let decisions = match decisions(case, sockets) {
        Ok(d) => d,
        Err(e) => return vec![format!("{at}: harness: {e}")],
    };
    let mut out = Vec::new();
    for socket in &outcome.sockets {
        let want: Vec<&str> = decisions
            .iter()
            .filter(|d| d.socket == socket.socket && d.status == "removed")
            .map(|d| d.requirement.as_str())
            .collect();
        let mut got: Vec<&str> = socket.removed.iter().map(String::as_str).collect();
        let mut sorted = want.clone();
        sorted.sort_unstable();
        got.sort_unstable();
        if got != sorted {
            out.push(format!(
                "{at}: {} lists {got:?} removed, its review {sorted:?}",
                socket.socket
            ));
        }
        for r in want
            .iter()
            .filter(|r| socket.checked.iter().any(|c| c == *r))
        {
            out.push(format!("{at}: {}/{r} removed, yet checked", socket.socket));
        }
    }
    out
}

/// The expected failures under `key` (`must_fail`, `may_fail`), each a
/// list of the entries one of which must meet it.
fn listed(sockets: &[Socket], expect: &Value, key: &str) -> Result<Vec<Vec<Entry>>, String> {
    expect[key]
        .as_array()
        .into_iter()
        .flatten()
        .map(|v| match v["any_of"].as_array() {
            Some(options) => options.iter().map(|o| entry(sockets, o)).collect(),
            None => entry(sockets, v).map(|e| vec![e]),
        })
        .collect()
}

/// must ⊆ F, with a code each allows, and F ⊆ must ∪ may.
fn failed_as_expected(
    at: &str,
    must: &[Vec<Entry>],
    may: &[Vec<Entry>],
    failed: &[(String, Vec<String>)],
) -> Vec<String> {
    let mut problems = Vec::new();
    for options in must {
        if !options
            .iter()
            .any(|e| failed.iter().any(|(r, codes)| e.meets(r, codes)))
        {
            let names: Vec<&String> = options.iter().flat_map(|e| &e.requirements).collect();
            problems.push(format!(
                "{at}: {names:?} did not fail as expected; failed: {failed:?}"
            ));
        }
    }
    for (r, codes) in failed {
        let allowed = must.iter().chain(may).flatten().any(|e| e.meets(r, codes));
        if !allowed {
            problems.push(format!("{at}: {r} failed with {codes:?}, not expected"));
        }
    }
    problems
}

/// KH2 and KH3 on one candidate's outcome against one socket document's
/// sockets: every way it departs from the case's expectation under
/// `policy`, and from its review.
pub fn gate(case: &Value, policy: &str, sockets: &[Socket], outcome: &Outcome) -> Vec<String> {
    let id = case["id"].as_str().unwrap_or("?");
    let at = format!("{id} {policy}");
    let expect = expectation(case, policy);
    let mut problems = Vec::new();
    let want = match expect["verdict"].as_str() {
        Some(v @ ("interface_match" | "match" | "reject")) => v,
        other => return vec![format!("{at}: harness: verdict {other:?}")],
    };
    if outcome.verdict != want {
        problems.push(format!("{at}: {}, expected {want}", outcome.verdict));
    }
    // F: each failed requirement, in every socket, with its codes.
    if outcome.sockets.len() != sockets.len() && outcome.status == "accepted" {
        return vec![format!(
            "{at}: harness: {} sockets judged of {}",
            outcome.sockets.len(),
            sockets.len()
        )];
    }
    let mut failed = failures(outcome);
    let blocked_count = blocked(&at, sockets, expect, outcome, &mut failed, &mut problems);
    problems.extend(flags(&at, expect, outcome));
    problems.extend(removals(&at, case, sockets, outcome));
    let (must, may) = match (
        listed(sockets, expect, "must_fail"),
        listed(sockets, expect, "may_fail"),
    ) {
        (Ok(must), Ok(may)) => (must, may),
        (Err(e), _) | (_, Err(e)) => return vec![format!("{at}: harness: {e}")],
    };
    problems.extend(failed_as_expected(&at, &must, &may, &failed));
    // A bad case names its defect: `must` is never empty.
    let truth = case["truth"].as_str().unwrap_or("");
    if matches!(truth, "does_not_fit" | "edge")
        && (must.is_empty() || case["defect"].as_array().is_none_or(Vec::is_empty))
    {
        problems.push(format!("{at}: a bad case names no defect"));
    }
    if truth == "fn_rule"
        && failed.is_empty()
        && want == "reject"
        && blocked_count.is_none_or(|n| n == 0)
    {
        problems.push(format!("{at}: a false negative by rule failed nothing"));
    }
    problems
}
