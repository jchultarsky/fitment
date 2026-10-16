//! The strict gate of a match case (`docs/research/m4-match.md`, KH2,
//! KH3): the verdict is the one expected, and the requirements that failed
//! are those the case names, each with a code it allows. Requirements are
//! named by locators (KC5): what they constrain, in the target's frame.
//! Shared by fitment-core's tests (the committed candidates) and fitment's
//! (the corpus).

use fitment_core::{CandidateResult, Requirement, Requires, Socket, VerdictKind};
use serde_json::Value;

/// How far a locator's numbers may lie from the requirement's: the
/// manifest rounds them to 9 decimals.
const NEAR: f64 = 1e-6;

fn array(v: &Value) -> Option<[f64; 3]> {
    let a = v.as_array()?;
    Some([a.first()?.as_f64()?, a.get(1)?.as_f64()?, a.get(2)?.as_f64()?])
}

fn near3(a: [f64; 3], b: [f64; 3]) -> bool {
    (0..3).all(|k| (a[k] - b[k]).abs() <= NEAR)
}

/// True if the line through `p` along `d` is the line through `q` along
/// `e`, whatever their signs.
fn same_line(p: [f64; 3], d: [f64; 3], q: [f64; 3], e: [f64; 3]) -> bool {
    let cross = [
        d[1] * e[2] - d[2] * e[1],
        d[2] * e[0] - d[0] * e[2],
        d[0] * e[1] - d[1] * e[0],
    ];
    let w = [q[0] - p[0], q[1] - p[1], q[2] - p[2]];
    let along = w[0] * d[0] + w[1] * d[1] + w[2] * d[2];
    let off = [w[0] - along * d[0], w[1] - along * d[1], w[2] - along * d[2]];
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
            kind == "conical_seat" && array(&locator["apex"]).is_some_and(|a| near3(a, apex_mm.to_array()))
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
        _ => Err(format!("{locator} names several requirements of {}", socket.id)),
    }
}

/// An expected failure: the requirement it names and the codes it allows
/// (any, when none are listed).
struct Entry<'s> {
    requirement: &'s str,
    codes: Vec<String>,
}

fn entry<'s>(socket: &'s Socket, value: &Value) -> Result<Entry<'s>, String> {
    let requirement = locate(socket, value)?;
    let codes = value["codes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|c| !c.is_empty())
        .map(str::to_owned)
        .collect();
    Ok(Entry {
        requirement: &requirement.id,
        codes,
    })
}

impl Entry<'_> {
    fn meets(&self, requirement: &str, codes: &[String]) -> bool {
        self.requirement == requirement
            && (self.codes.is_empty() || codes.iter().any(|c| self.codes.contains(c)))
    }
}

/// The expectation of `case` under `policy`: an object, or the name of the
/// policy it equals.
pub fn expectation<'c>(case: &'c Value, policy: &str) -> &'c Value {
    let mut at = &case["expect"][policy];
    while let Some(other) = at.as_str() {
        at = &case["expect"][other];
    }
    at
}

/// KH2 and KH3 on one candidate's result against one socket: every way
/// it departs from the case's expectation under `policy`.
pub fn gate(case: &Value, policy: &str, socket: &Socket, result: &CandidateResult) -> Vec<String> {
    let id = case["id"].as_str().unwrap_or("?");
    let expect = expectation(case, policy);
    let mut problems = Vec::new();
    let want = match expect["verdict"].as_str() {
        Some("interface_match") => VerdictKind::InterfaceMatch,
        Some("match") => VerdictKind::Match,
        _ => VerdictKind::Reject,
    };
    if result.verdict.kind() != want {
        problems.push(format!(
            "{id} {policy}: {:?}, expected {want:?}",
            result.verdict.kind()
        ));
    }
    // F: each failed requirement with its codes.
    let failed: Vec<(String, Vec<String>)> = result
        .sockets
        .iter()
        .filter(|s| s.socket == socket.id)
        .flat_map(|s| s.checks.iter())
        .filter(|(_, c)| !c.passed())
        .map(|(r, c)| {
            let codes = c
                .evidence()
                .failures()
                .iter()
                .map(|f| serde_json::to_value(f.code).unwrap_or_default())
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect();
            (r.as_str().to_owned(), codes)
        })
        .collect();
    let list = |key: &str| -> Result<Vec<Vec<Entry<'_>>>, String> {
        expect[key]
            .as_array()
            .into_iter()
            .flatten()
            .map(|v| match v["any_of"].as_array() {
                Some(options) => options.iter().map(|o| entry(socket, o)).collect(),
                None => entry(socket, v).map(|e| vec![e]),
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
            let names: Vec<&str> = options.iter().map(|e| e.requirement).collect();
            problems.push(format!(
                "{id} {policy}: {names:?} did not fail as expected; failed: {failed:?}"
            ));
        }
    }
    // F ⊆ must ∪ may: every failure is one the case allows.
    for (r, codes) in &failed {
        let allowed = must
            .iter()
            .chain(&may)
            .flatten()
            .any(|e| e.meets(r, codes));
        if !allowed {
            problems.push(format!("{id} {policy}: {r} failed with {codes:?}, not expected"));
        }
    }
    // A bad case names its defect: `must` is never empty.
    let truth = case["truth"].as_str().unwrap_or("");
    if matches!(truth, "does_not_fit" | "edge") && must.is_empty() {
        problems.push(format!("{id} {policy}: a bad case names no requirement"));
    }
    if truth == "fn_rule" && failed.is_empty() && want == VerdictKind::Reject {
        problems.push(format!("{id} {policy}: a false negative by rule failed nothing"));
    }
    problems
}
