//! What the scorer judges (`docs/research/m6-api.md`, SC2): one
//! candidate's result as a plain record, built from a library
//! `CandidateResult` or from an entry of a `fitment.match` document, so one
//! gate holds the library, the binary and the API to the same construction
//! (SC1). An `Outcome` is never turned back into a verdict or a document
//! (M4-23). Includable by path (SC6): it names only `fitment_core` and
//! `serde_json`.

use fitment_core::{CandidateResult, MatchPolicy, NotChecked, SocketResult};
use serde_json::{Value, json};

/// One candidate's result: its verdict, its gate status and refusal codes,
/// each socket's judgment, its flags, its fit's status and the policy it
/// was judged under, each as the document writes it.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    /// `match`, `interface_match` or `reject`.
    pub verdict: String,
    /// `accepted` or `refused`: M2's gate.
    pub status: String,
    /// The gate's refusal codes, in order.
    pub refusals: Vec<String>,
    /// Each socket's, in the document's order; none for a refused file.
    pub sockets: Vec<SocketOutcome>,
    /// The candidate's flags (VD12).
    pub flags: Vec<String>,
    /// The fit's status (VD4).
    pub fit: String,
    /// The fit's code when it is not verified (FV9; `docs/research/m7-fit.md`,
    /// FM3), as the document writes it.
    pub fit_code: Option<String>,
    /// The policy, as the document states it (KH2.6).
    pub policy: Value,
}

/// One socket's judgment of a candidate.
#[derive(Debug, Clone, PartialEq)]
pub struct SocketOutcome {
    /// The socket's id.
    pub socket: String,
    /// Its verdict.
    pub verdict: String,
    /// The requirements it checked, in order.
    pub checked: Vec<String>,
    /// Those that failed, each with its failures' codes in order.
    pub failed: Vec<(String, Vec<String>)>,
    /// Those removed in review, in order: not checked (VD7).
    pub removed: Vec<String>,
}

/// A serialized unit enum's name: `name(serde_json::to_value(x))`.
fn name(value: serde_json::Result<Value>) -> String {
    value
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// The policy as a `fitment.match` document states it (MF6).
pub fn policy_value(policy: &MatchPolicy) -> Value {
    json!({
        "name": policy.tolerance().name(),
        "linear_mm": policy.tolerance().linear_mm(),
        "angular_deg": policy.tolerance().angular_deg(),
        "clearance": serde_json::to_value(policy.clearance()).unwrap_or_default(),
        "grip": serde_json::to_value(policy.grip()).unwrap_or_default(),
    })
}

fn socket_of_result(result: &SocketResult) -> SocketOutcome {
    SocketOutcome {
        socket: result.socket.clone(),
        verdict: name(serde_json::to_value(result.verdict.kind())),
        checked: result
            .checks
            .iter()
            .map(|(r, _)| r.as_str().to_owned())
            .collect(),
        failed: result
            .checks
            .iter()
            .filter(|(_, c)| !c.passed())
            .map(|(r, c)| {
                let codes = c
                    .evidence()
                    .failures()
                    .iter()
                    .map(|f| name(serde_json::to_value(f.code)));
                (r.as_str().to_owned(), codes.collect())
            })
            .collect(),
        removed: result
            .not_checked
            .iter()
            .filter_map(|n| match n {
                NotChecked::Removed { requirement, .. } => Some(requirement.as_str().to_owned()),
                _ => None,
            })
            .collect(),
    }
}

fn text(value: &Value, what: &str) -> Result<String, String> {
    value
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| format!("no {what}"))
}

fn list<'v>(value: &'v Value, what: &str) -> Result<&'v Vec<Value>, String> {
    value.as_array().ok_or_else(|| format!("no {what}"))
}

fn socket_of_entry(socket: &Value) -> Result<SocketOutcome, String> {
    let id = text(&socket["socket"], "socket id")?;
    let mut checked = Vec::new();
    let mut failed = Vec::new();
    for check in list(&socket["checks"], "checks")? {
        let requirement = text(&check["requirement"], "requirement")?;
        match check["result"].as_str() {
            Some("not_checked") => {}
            Some("passed") => checked.push(requirement),
            Some("failed") => {
                let codes = list(&check["failures"], "failures of a failed check")?
                    .iter()
                    .map(|f| text(&f["code"], "failure code"))
                    .collect::<Result<_, _>>()?;
                checked.push(requirement.clone());
                failed.push((requirement, codes));
            }
            other => return Err(format!("{id}: a check's result {other:?}")),
        }
    }
    let removed = list(&socket["not_checked"], "not_checked")?
        .iter()
        .filter(|n| n["what"] == "removed")
        .map(|n| text(&n["requirement"], "removed requirement"))
        .collect::<Result<_, _>>()?;
    Ok(SocketOutcome {
        socket: id,
        verdict: text(&socket["verdict"], "socket verdict")?,
        checked,
        failed,
        removed,
    })
}

impl Outcome {
    /// The library's result under `policy`.
    pub fn of_result(result: &CandidateResult, policy: &MatchPolicy) -> Self {
        Self {
            verdict: name(serde_json::to_value(result.verdict.kind())),
            status: name(serde_json::to_value(result.status)),
            refusals: result
                .refusals
                .iter()
                .map(|r| r.code.name().to_owned())
                .collect(),
            sockets: result.sockets.iter().map(socket_of_result).collect(),
            flags: result
                .flags
                .iter()
                .map(|f| name(serde_json::to_value(f)))
                .collect(),
            fit: serde_json::to_value(result.verdict.fit())
                .ok()
                .and_then(|v| v["status"].as_str().map(str::to_owned))
                .unwrap_or_default(),
            fit_code: matches!(
                result.verdict.fit(),
                fitment_core::FitStatus::NotVerified { .. }
            )
            .then(|| result.fit.as_deref().and_then(|e| e.outcome().code()))
            .flatten()
            .map(|c| c.as_str().to_owned()),
            policy: policy_value(policy),
        }
    }

    /// One entry of a `fitment.match` document whose policy is `policy`.
    pub fn of_entry(entry: &Value, policy: &Value) -> Result<Self, String> {
        let strings = |key: &str| -> Result<Vec<String>, String> {
            list(&entry[key], key)?
                .iter()
                .map(|v| text(v, key))
                .collect()
        };
        Ok(Self {
            verdict: text(&entry["verdict"], "verdict")?,
            status: text(&entry["status"], "status")?,
            refusals: list(&entry["refusals"], "refusals")?
                .iter()
                .map(|r| text(&r["code"], "refusal code"))
                .collect::<Result<_, _>>()?,
            sockets: list(&entry["sockets"], "sockets")?
                .iter()
                .map(socket_of_entry)
                .collect::<Result<_, _>>()?,
            flags: strings("flags")?,
            fit: text(&entry["fit"]["status"], "fit status")?,
            fit_code: entry["fit"]["code"].as_str().map(str::to_owned),
            policy: policy.clone(),
        })
    }

    /// SC9: the entry of `doc` for the file whose SHA-256 is `sha256`
    /// (MF3: one entry per SHA-256), never found by its label.
    pub fn find(doc: &Value, sha256: &str) -> Result<Self, String> {
        Self::found(doc, &format!("the file {sha256}"), |source| {
            source["kind"] == "file" && source["sha256"] == sha256
        })
    }

    /// SC9: the entry of `doc` for a component, by its assembly's SHA-256
    /// and its definition's `#id`.
    pub fn find_component(
        doc: &Value,
        assembly_sha256: &str,
        definition: u64,
    ) -> Result<Self, String> {
        Self::found(doc, &format!("{assembly_sha256}#{definition}"), |source| {
            source["kind"] == "component"
                && source["assembly_sha256"] == assembly_sha256
                && source["definition"] == definition
        })
    }

    fn found(doc: &Value, what: &str, is: impl Fn(&Value) -> bool) -> Result<Self, String> {
        let entries: Vec<&Value> = list(&doc["candidates"], "candidates")?
            .iter()
            .filter(|c| is(&c["source"]))
            .collect();
        match entries.as_slice() {
            [one] => Self::of_entry(one, &doc["policy"]),
            [] => Err(format!("no entry for {what}")),
            _ => Err(format!("several entries for {what}")),
        }
    }
}
