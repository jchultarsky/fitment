//! The corpus with a fit verifier (`docs/research/m7-fit.md`, FK1, FK6):
//! what a fitted run must come to beside what its interface comes to, and
//! the fit baseline, the one record of every (case, policy)'s fit status
//! and code that only the library suite blesses.
//!
//! The interface's gate (KH2, KH3) is [`crate::gate::gate`], judging the
//! outcome as its interface left it: a `Match` whose fit is verified and a
//! `Reject` by `interferes` alone were each an `InterfaceMatch` there. On
//! top of it, FK1: a `Match` only on a case whose truth fits under the
//! policy; a `collides` case `Reject` by `interferes`, or `InterfaceMatch`
//! with one of four codes; a `fits` case never `Reject` by `interferes`;
//! and the fit the case states, where it states one (FK2's cases, the
//! waived ones).

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use fitment_core::Socket;
use serde_json::{Value, json};

use crate::gate;
use crate::outcome::Outcome;

/// FK1: the codes a `collides` case may end `InterfaceMatch` with, each
/// listed in the report.
pub const COLLIDES_MAY_END: [&str; 4] = [
    "placement_not_unique",
    "kernel_alert",
    "unsupported_geometry",
    "uncertainty_above_tolerance",
];

/// The outcome as the interface left it: a `Match` with a verified fit,
/// or a `Reject` whose every socket passed and whose fit interferes, was
/// an `InterfaceMatch` before the fit.
pub fn interface_view(outcome: &Outcome) -> Outcome {
    let mut out = outcome.clone();
    let passed = outcome.status == "accepted"
        && !outcome.sockets.is_empty()
        && outcome.sockets.iter().all(|s| s.failed.is_empty());
    let fitted = match (outcome.verdict.as_str(), outcome.fit.as_str()) {
        ("match", "verified") => true,
        ("reject", "interferes") => passed,
        _ => false,
    };
    if fitted {
        "interface_match".clone_into(&mut out.verdict);
    }
    out
}

/// True if `truth` fits physically under `policy`, so that a `Match` on it
/// is no false positive (FK1): `fits`, `fn_rule`, and `fits_functional`
/// under functional.
pub fn may_match(truth: &str, policy: &str) -> bool {
    matches!(truth, "fits" | "fn_rule") || (truth == "fits_functional" && policy == "functional")
}

/// The fit `case` states under `policy`, if any: `{"status", "code"}`.
pub fn stated<'c>(case: &'c Value, policy: &str) -> Option<&'c Value> {
    let fit = &gate::expectation(case, policy)["fit"];
    fit.is_object().then_some(fit)
}

/// KH2, KH3 on the outcome as its interface left it, and FK1 on the fit:
/// every way a fitted run departs from its case under `policy`.
pub fn gate(case: &Value, policy: &str, sockets: &[Socket], outcome: &Outcome) -> Vec<String> {
    let id = case["id"].as_str().unwrap_or("?");
    let at = format!("{id} {policy}");
    let mut problems = gate::gate(case, policy, sockets, &interface_view(outcome));
    problems.extend(truths(&at, case, policy, outcome));
    if let Some(fit) = stated(case, policy) {
        problems.extend(as_stated(&at, fit, outcome));
    }
    problems
}

/// FK1 by the case's truth.
fn truths(at: &str, case: &Value, policy: &str, outcome: &Outcome) -> Vec<String> {
    let truth = case["truth"].as_str().unwrap_or("");
    let code = outcome.fit_code.as_deref().unwrap_or("");
    let mut out = Vec::new();
    let passed = outcome.status == "accepted"
        && !outcome.sockets.is_empty()
        && outcome.sockets.iter().all(|s| s.failed.is_empty());
    if outcome.verdict == "match" && !may_match(truth, policy) {
        out.push(format!("{at}: a false positive: Match on a {truth} case"));
    }
    if outcome.verdict == "match" && outcome.fit != "verified" {
        out.push(format!("{at}: Match with the fit {}", outcome.fit));
    }
    if passed && outcome.fit == "not_checked" {
        out.push(format!("{at}: every socket passed, and no fit was asked"));
    }
    if outcome.fit == "not_verified" && code.is_empty() {
        out.push(format!("{at}: not verified with no code"));
    }
    if truth == "collides" {
        let rejected = outcome.verdict == "reject" && outcome.fit == "interferes";
        let excused = outcome.verdict == "interface_match"
            && outcome.fit == "not_verified"
            && COLLIDES_MAY_END.contains(&code);
        if !rejected && !excused {
            out.push(format!(
                "{at}: a collides case ends {} with the fit {} {code}",
                outcome.verdict, outcome.fit
            ));
        }
    }
    out.extend(free(at, case, outcome));
    if truth == "fits" && outcome.fit == "interferes" {
        out.push(format!(
            "{at}: a fits case Reject by interferes: the label or the fit is wrong"
        ));
    }
    out
}

/// FK3: a `collides` case free to turn about an axis its candidate is not
/// symmetric about ends `placement_not_unique`; one whose candidate is
/// axisymmetric must collide at every turn, so its fit interferes.
fn free(at: &str, case: &Value, outcome: &Outcome) -> Option<String> {
    if case["truth"] != "collides" {
        return None;
    }
    let code = outcome.fit_code.as_deref();
    match case["freedom"].as_str() {
        Some("turn") if code != Some("placement_not_unique") => Some(format!(
            "{at}: a collides case free to turn, not axisymmetric, ends {} {code:?} (FK3)",
            outcome.fit
        )),
        Some("turn_axisymmetric") if outcome.fit != "interferes" => Some(format!(
            "{at}: a collides case free to turn, axisymmetric, ends {} {code:?}: it must collide at every \
             turn (FK3)",
            outcome.fit
        )),
        _ => None,
    }
}

/// The fit the case states: its status, and its code where it names one.
fn as_stated(at: &str, fit: &Value, outcome: &Outcome) -> Vec<String> {
    let mut out = Vec::new();
    let status = fit["status"].as_str().unwrap_or("");
    if outcome.fit != status {
        out.push(format!(
            "{at}: the fit {} {:?}, its case states {status}",
            outcome.fit, outcome.fit_code
        ));
    }
    if let Some(code) = fit["code"].as_str() {
        if outcome.fit_code.as_deref() != Some(code) {
            out.push(format!(
                "{at}: the fit's code {:?}, its case states {code}",
                outcome.fit_code
            ));
        }
    }
    out
}

/// One run's fit as the baseline records it.
pub fn entry(case: &str, policy: &str, outcome: &Outcome) -> Value {
    json!({
        "case": case,
        "policy": policy,
        "verdict": outcome.verdict,
        "status": outcome.fit,
        "code": outcome.fit_code,
    })
}

/// `crates/fitment/tests/corpus/fit-baseline.json` (FK6).
pub fn baseline_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../fitment/tests/corpus/fit-baseline.json")
}

/// What a fit baseline is recorded against: the fit's rules and the
/// helper that measured.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Against {
    /// The helper script's SHA-256.
    pub helper_sha256: String,
    /// The binding the helper states.
    pub binding: String,
}

/// The whole record.
pub fn baseline_document(runs: &[Value], against: &Against) -> Value {
    json!({
        "fit_rules_version": fitment_core::fit::FIT_RULES_VERSION,
        "helper_sha256": against.helper_sha256,
        "binding": against.binding,
        "runs": runs,
    })
}

fn committed_baseline() -> Option<Value> {
    fs::read_to_string(baseline_path())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
}

fn runs_of(document: Option<&Value>) -> Vec<Value> {
    document
        .and_then(|d| d["runs"].as_array().cloned())
        .unwrap_or_default()
}

fn key(run: &Value) -> (String, String) {
    (run["case"].to_string(), run["policy"].to_string())
}

fn differ(found: &[Value], old: &[Value]) -> String {
    let added: Vec<&Value> = found.iter().filter(|f| !old.contains(f)).collect();
    let removed: Vec<&Value> = old.iter().filter(|f| !found.contains(f)).collect();
    format!("now: {added:#?}\nwas: {removed:#?}")
}

/// How a run may treat the fit baseline: compare only, or bless outside CI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Blessing {
    /// Compare: any difference fails.
    Compare,
    /// `FITMENT_BLESS=1` in CI: refused.
    InCi,
    /// `FITMENT_BLESS=1` locally: rewrite, within FK6's limits.
    Bless,
}

impl Blessing {
    /// From the environment: `FITMENT_BLESS=1`, and `CI`.
    pub fn from_env() -> Self {
        if std::env::var_os("FITMENT_BLESS").is_none_or(|v| v != "1") {
            Self::Compare
        } else if std::env::var_os("CI").is_some() {
            Self::InCi
        } else {
            Self::Bless
        }
    }
}

/// FK6's judgment of `runs` against the `committed` baseline: `Ok(None)`
/// when they are its record, `Ok(Some(document))` to write when blessing
/// may rewrite it, else why not: a difference when comparing, a blessing in
/// CI, or a known case whose fit changed with neither `FIT_RULES_VERSION`
/// nor the helper changed.
pub fn judge_baseline(
    committed: Option<&Value>,
    runs: &[Value],
    against: &Against,
    blessing: Blessing,
) -> Result<Option<Value>, String> {
    let document = baseline_document(runs, against);
    if committed == Some(&document) {
        return Ok(None);
    }
    let old = runs_of(committed);
    match blessing {
        Blessing::Compare => {
            return Err(format!(
                "the fit baseline differs from {}:\n{}\n\
                 if intended, run FITMENT_BLESS=1 with the fit corpus suite",
                baseline_path().display(),
                differ(runs, &old)
            ));
        }
        Blessing::InCi => return Err("the fit baseline is blessed locally, never in CI".to_owned()),
        Blessing::Bless => {}
    }
    let same = committed.is_some_and(|c| {
        c["fit_rules_version"] == fitment_core::fit::FIT_RULES_VERSION
            && c["helper_sha256"] == against.helper_sha256.as_str()
            && c["binding"] == against.binding.as_str()
    });
    let changed: Vec<&Value> = runs
        .iter()
        .filter(|r| {
            old.iter()
                .any(|o| key(o) == key(r) && (o["status"] != r["status"] || o["code"] != r["code"]))
        })
        .collect();
    if same && !changed.is_empty() {
        return Err(format!(
            "known cases' fits changed without a FIT_RULES_VERSION rise or a helper change: {changed:#?}"
        ));
    }
    Ok(Some(document))
}

/// FK6, for the library suite alone: every run's fit is the committed
/// baseline's (`judge_baseline`, as the environment asks).
pub fn bless_or_compare(runs: &[Value], against: &Against) -> Result<(), String> {
    let committed = committed_baseline();
    let Some(document) = judge_baseline(committed.as_ref(), runs, against, Blessing::from_env())?
    else {
        return Ok(());
    };
    fs::write(baseline_path(), baseline_text(&document)?)
        .map_err(|e| format!("cannot write {}: {e}", baseline_path().display()))
}

/// The baseline as written: its versions on a line each and one run per
/// line, so its diffs read by case.
fn baseline_text(document: &Value) -> Result<String, String> {
    let line = |v: &Value| serde_json::to_string(v).map_err(|e| e.to_string());
    let mut text = String::from("{\n");
    for key in ["fit_rules_version", "helper_sha256", "binding"] {
        let _ = writeln!(text, "  \"{key}\": {},", line(&document[key])?);
    }
    text += "  \"runs\": [\n";
    let runs = runs_of(Some(document));
    for (i, run) in runs.iter().enumerate() {
        let comma = if i + 1 < runs.len() { "," } else { "" };
        let _ = writeln!(text, "    {}{comma}", line(run)?);
    }
    text += "  ]\n}\n";
    Ok(text)
}

/// FK6, for every path but the library's: each run's fit is the
/// baseline's entry for its (case, policy). It never writes.
pub fn compare(runs: &[Value]) -> Result<(), String> {
    let old = runs_of(committed_baseline().as_ref());
    let differing: Vec<String> = runs
        .iter()
        .filter(|r| !old.contains(r))
        .map(|r| {
            let was = old.iter().find(|o| key(o) == key(r));
            format!("{r} (the baseline: {was:?})")
        })
        .collect();
    if differing.is_empty() {
        return Ok(());
    }
    Err(format!(
        "fits differ from {}, which only the library suite blesses:\n{}",
        baseline_path().display(),
        differing.join("\n")
    ))
}

/// The report's counts (`docs/research/m7-fit.md`, section 11, PR 4's
/// "done when"): per policy, the runs by verdict and fit status, every
/// `not_verified` by code with its cases, and FK1's lists.
pub fn report(runs: &[(String, String, String, Outcome)]) -> Value {
    let mut by: std::collections::BTreeMap<String, Value> = std::collections::BTreeMap::new();
    for (case, policy, truth, outcome) in runs {
        let row = by.entry(policy.clone()).or_insert_with(
            || json!({"runs": 0, "verdicts": {}, "fit": {}, "not_verified": {}, "by_truth": {}}),
        );
        row["runs"] = json!(row["runs"].as_u64().unwrap_or(0) + 1);
        bump(&mut row["verdicts"][&outcome.verdict]);
        bump(&mut row["fit"][&outcome.fit]);
        bump(&mut row["by_truth"][truth][format!("{} {}", outcome.verdict, outcome.fit)]);
        if let Some(code) = &outcome.fit_code {
            let list = &mut row["not_verified"][code];
            if !list.is_array() {
                *list = json!([]);
            }
            if let Some(list) = list.as_array_mut() {
                list.push(json!(case));
            }
        }
    }
    json!(by)
}

fn bump(v: &mut Value) {
    *v = json!(v.as_u64().unwrap_or(0) + 1);
}
