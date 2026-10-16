//! The strict gate of a match case (`docs/research/m4-match.md`, KH2,
//! KH3): the verdict is the one expected, and the requirements that failed
//! are those the case names, each with a code it allows. Requirements are
//! named by locators (KC5): what they constrain, in the target's frame.
//! Shared by fitment-core's tests (the committed candidates) and fitment's
//! (the corpus).

use fitment_core::{
    CandidateResult, Requirement, Requires, Rigid, Socket, SocketResult, VerdictKind,
};
use serde_json::Value;

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
/// harness error, never a pass (KC5).
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

/// An expected failure: the requirement it names, in each socket that has
/// it (`socket-1/bore:hole-88`), and the codes it allows (any, when none
/// are listed).
struct Entry {
    requirements: Vec<String>,
    codes: Vec<String>,
}

/// The requirement `value` names in every socket that has it: one per
/// socket at most, and in some socket.
fn entry(sockets: &[Socket], value: &Value) -> Result<Entry, String> {
    let mut requirements = Vec::new();
    let mut problems = Vec::new();
    for socket in sockets {
        match locate(socket, value) {
            Ok(r) => requirements.push(format!("{}/{}", socket.id, r.id)),
            Err(e) if e.contains("several") => return Err(e),
            Err(e) => problems.push(e),
        }
    }
    if requirements.is_empty() {
        return Err(problems.join("; "));
    }
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

/// The failed checks of a socket's result: `socket/requirement` and codes.
fn failures(result: &SocketResult) -> Vec<(String, Vec<String>)> {
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
    let failed: Vec<(String, Vec<String>)> = at_construction.iter().flat_map(failures).collect();
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
    expect: &Value,
    result: &CandidateResult,
    failed: &mut Vec<(String, Vec<String>)>,
    problems: &mut Vec<String>,
) -> Option<u64> {
    if let Some(codes) = expect["refused"].as_array() {
        let mut want: Vec<&str> = codes.iter().filter_map(Value::as_str).collect();
        let mut got: Vec<&str> = result.refusals.iter().map(|r| r.code.name()).collect();
        want.sort_unstable();
        got.sort_unstable();
        got.dedup();
        if result.status != fitment_core::Status::Refused || got != want {
            problems.push(format!("{at}: refused with {got:?}, expected {want:?}"));
        }
    }
    let all: u64 = result.sockets.iter().map(|s| s.checks.len() as u64).sum();
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

/// KH2 and KH3 on one candidate's result against one socket: every way
/// it departs from the case's expectation under `policy`.
pub fn gate(
    case: &Value,
    policy: &str,
    sockets: &[Socket],
    result: &CandidateResult,
) -> Vec<String> {
    let id = case["id"].as_str().unwrap_or("?");
    let expect = expectation(case, policy);
    let mut problems = Vec::new();
    let want = match expect["verdict"].as_str() {
        Some("interface_match") => VerdictKind::InterfaceMatch,
        Some("match") => VerdictKind::Match,
        Some("reject") => VerdictKind::Reject,
        other => return vec![format!("{id} {policy}: harness: verdict {other:?}")],
    };
    if result.verdict.kind() != want {
        problems.push(format!(
            "{id} {policy}: {:?}, expected {want:?}",
            result.verdict.kind()
        ));
    }
    // F: each failed requirement, in every socket, with its codes.
    if result.sockets.len() != sockets.len() && result.status == fitment_core::Status::Accepted {
        return vec![format!(
            "{id} {policy}: harness: {} sockets judged of {}",
            result.sockets.len(),
            sockets.len()
        )];
    }
    let mut failed: Vec<(String, Vec<String>)> = result.sockets.iter().flat_map(failures).collect();
    let at = format!("{id} {policy}");
    let blocked_count = blocked(&at, expect, result, &mut failed, &mut problems);
    let list = |key: &str| -> Result<Vec<Vec<Entry>>, String> {
        expect[key]
            .as_array()
            .into_iter()
            .flatten()
            .map(|v| match v["any_of"].as_array() {
                Some(options) => options.iter().map(|o| entry(sockets, o)).collect(),
                None => entry(sockets, v).map(|e| vec![e]),
            })
            .collect()
    };
    let (must, may) = match (list("must_fail"), list("may_fail")) {
        (Ok(must), Ok(may)) => (must, may),
        (Err(e), _) | (_, Err(e)) => return vec![format!("{id} {policy}: harness: {e}")],
    };
    // must ⊆ F: every entry (or one of an any_of) fails with a code it allows.
    for options in &must {
        if !options
            .iter()
            .any(|e| failed.iter().any(|(r, codes)| e.meets(r, codes)))
        {
            let names: Vec<&String> = options.iter().flat_map(|e| &e.requirements).collect();
            problems.push(format!(
                "{id} {policy}: {names:?} did not fail as expected; failed: {failed:?}"
            ));
        }
    }
    // F ⊆ must ∪ may: every failure is one the case allows.
    for (r, codes) in &failed {
        let allowed = must.iter().chain(&may).flatten().any(|e| e.meets(r, codes));
        if !allowed {
            problems.push(format!(
                "{id} {policy}: {r} failed with {codes:?}, not expected"
            ));
        }
    }
    // A bad case names its defect: `must` is never empty.
    let truth = case["truth"].as_str().unwrap_or("");
    if matches!(truth, "does_not_fit" | "edge")
        && (must.is_empty() || case["defect"].as_array().is_none_or(Vec::is_empty))
    {
        problems.push(format!("{id} {policy}: a bad case names no defect"));
    }
    if truth == "fn_rule"
        && failed.is_empty()
        && want == VerdictKind::Reject
        && blocked_count.is_none_or(|n| n == 0)
    {
        problems.push(format!(
            "{id} {policy}: a false negative by rule failed nothing"
        ));
    }
    problems
}
