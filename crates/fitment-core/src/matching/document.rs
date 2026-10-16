//! The `fitment.match` document (`docs/research/m4-match.md`, section
//! 11): one run's candidates, judged against one socket document, in a
//! stated order, with a digest that identifies the result on every OS.
//! There is no reader: a stored or edited report never becomes a verdict
//! (M4-23).

use serde::Serialize;
use serde_json::Value;

use super::evidence::{Evidence, Failure, Measure, Warning};
use super::matcher::{CandidateFlag, CandidateResult, Chosen, NotChecked, SocketResult, Stage};
use super::{MATCH_RULES_VERSION, MatchOptions, Matcher, SIGNATURE_VERSION, Seed, Window};
use crate::features::{FeatureId, ProductRef, Status};
use crate::fit::{FitStatus, FitVerifier};
use crate::gate::Refusal;
use crate::socket::{ReviewStatus, SocketDocument};
use crate::verdict::{Check, RejectReason, RequirementId, VerdictKind};

/// The document's format name.
pub const MATCH_FORMAT: &str = "fitment.match";

/// Its version: frozen at the first publish (M4-28).
pub const MATCH_FORMAT_VERSION: u32 = 1;

/// How many candidates got each verdict (MF6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[non_exhaustive]
pub struct Summary {
    /// Distinct candidates.
    pub candidates: usize,
    /// `Match`.
    #[serde(rename = "match")]
    pub matched: usize,
    /// `InterfaceMatch`.
    pub interface_match: usize,
    /// `Reject`, refused ones included.
    pub reject: usize,
    /// Refused by M2's gate.
    pub refused: usize,
}

/// A run's outcome (VD14): its best verdict, `--require-fit` taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Outcome {
    /// Some candidate matches.
    Match,
    /// None matches, some passes the interface.
    InterfaceMatch,
    /// Every candidate is rejected.
    Reject,
}

/// One run's result, ready to write.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct MatchDocument {
    text: String,
    digest: String,
    summary: Summary,
    outcome: Outcome,
}

impl MatchDocument {
    /// The document of `results` judged by `matcher`, each with the labels
    /// the caller named it by: candidates of one SHA-256 are one entry
    /// with every label (MF3), in MF5's order.
    pub fn new(matcher: &Matcher<'_>, results: Vec<(CandidateResult, Vec<String>)>) -> Self {
        let entries = merged(results);
        let mut summary = Summary {
            candidates: entries.len(),
            ..Summary::default()
        };
        for (result, _) in &entries {
            match result.verdict.kind() {
                VerdictKind::Match => summary.matched += 1,
                VerdictKind::InterfaceMatch => summary.interface_match += 1,
                VerdictKind::Reject => summary.reject += 1,
            }
            if result.status == Status::Refused {
                summary.refused += 1;
            }
        }
        let outcome = if summary.matched > 0 {
            Outcome::Match
        } else if summary.interface_match > 0 && !matcher.options().require_fit() {
            Outcome::InterfaceMatch
        } else {
            Outcome::Reject
        };
        let mut document = Document {
            format: MATCH_FORMAT,
            format_version: MATCH_FORMAT_VERSION,
            extractor_version: crate::EXTRACTOR_VERSION,
            socket_rules_version: crate::SOCKET_RULES_VERSION,
            signature_version: SIGNATURE_VERSION,
            match_rules_version: MATCH_RULES_VERSION,
            policy: policy(matcher),
            options: matcher.options(),
            fit_verifier: None,
            socket: socket_part(matcher),
            summary,
            candidates: entries
                .iter()
                .map(|(r, l)| candidate(matcher, r, l))
                .collect(),
            digest: String::new(),
        };
        let digest = digest_of(&document);
        document.digest.clone_from(&digest);
        let mut text = serde_json::to_string_pretty(&document).unwrap_or_default();
        text.push('\n');
        Self {
            text,
            digest,
            summary,
            outcome,
        }
    }

    /// The document as pretty JSON, ending with a newline.
    pub fn to_json(&self) -> String {
        self.text.clone()
    }

    /// Its digest, `sha256:<hex>` (MF7).
    pub fn digest(&self) -> &str {
        &self.digest
    }

    /// The verdicts' counts.
    pub fn summary(&self) -> Summary {
        self.summary
    }

    /// The run's outcome, after `require_fit`.
    pub fn outcome(&self) -> Outcome {
        self.outcome
    }
}

/// The digest: the compact JSON of the document without its digest and
/// without every candidate's labels (MF7), sorted by key.
fn digest_of(document: &Document) -> String {
    let mut value = serde_json::to_value(document).unwrap_or(Value::Null);
    if let Some(map) = value.as_object_mut() {
        map.remove("digest");
        if let Some(Value::Array(candidates)) = map.get_mut("candidates") {
            for c in candidates.iter_mut() {
                if let Some(entry) = c.as_object_mut() {
                    entry.remove("labels");
                }
            }
        }
    }
    format!("sha256:{}", crate::sha256_hex(value.to_string().as_bytes()))
}

/// Candidates of one SHA-256 merged, labels in order of appearance; then
/// MF5's order: verdict, stage reached, failed checks, SHA-256.
fn merged(results: Vec<(CandidateResult, Vec<String>)>) -> Vec<(CandidateResult, Vec<String>)> {
    let mut out: Vec<(CandidateResult, Vec<String>)> = Vec::new();
    for (result, labels) in results {
        let key = sha(&result);
        match out.iter_mut().find(|(r, _)| key.is_some() && sha(r) == key) {
            Some((_, seen)) => seen.extend(labels),
            None => out.push((result, labels)),
        }
    }
    out.sort_by_key(|(r, _)| order(r));
    out
}

fn sha(result: &CandidateResult) -> Option<String> {
    match &result.source {
        super::CandidateSource::File(source) => Some(source.sha256.clone()),
        _ => None,
    }
}

/// MF5's key: verdict, stage reached, failed checks, SHA-256.
fn order(result: &CandidateResult) -> (u8, u8, usize, String) {
    let verdict = match result.verdict.kind() {
        VerdictKind::Match => 0,
        VerdictKind::InterfaceMatch => 1,
        VerdictKind::Reject => 2,
    };
    let stage = match result.stage() {
        Some(Stage::Verification) => 0,
        Some(Stage::Alignment) => 1,
        Some(Stage::Shortlist) => 2,
        _ => 3,
    };
    let failed = result.sockets.iter().map(SocketResult::failed).sum();
    (verdict, stage, failed, sha(result).unwrap_or_default())
}

#[derive(Serialize)]
struct Document {
    format: &'static str,
    format_version: u32,
    extractor_version: u32,
    socket_rules_version: u32,
    signature_version: u32,
    match_rules_version: u32,
    policy: PolicyOut,
    options: MatchOptions,
    fit_verifier: Option<String>,
    socket: SocketPart,
    summary: Summary,
    candidates: Vec<CandidateOut>,
    digest: String,
}

#[derive(Serialize, Clone)]
struct PolicyOut {
    name: String,
    linear_mm: f64,
    angular_deg: f64,
    clearance: super::Clearance,
    grip: super::Grip,
}

fn policy(matcher: &Matcher<'_>) -> PolicyOut {
    let p = matcher.policy();
    PolicyOut {
        name: p.tolerance().name().to_owned(),
        linear_mm: p.tolerance().linear_mm(),
        angular_deg: p.tolerance().angular_deg(),
        clearance: p.clearance(),
        grip: p.grip(),
    }
}

#[derive(Serialize, Clone)]
struct SocketPart {
    digest: Option<String>,
    source: SourceOut,
    target: Value,
    selection: Value,
    sockets: Vec<SocketSummary>,
}

#[derive(Serialize, Clone)]
struct SourceOut {
    sha256: String,
    bytes: u64,
}

#[derive(Serialize, Clone)]
struct SocketSummary {
    id: String,
    occurrences: usize,
    active: usize,
    unreviewed: usize,
    removed: Vec<Removed>,
    unverifiable: Vec<Unverifiable>,
    left_out: Vec<FeatureId>,
}

#[derive(Serialize, Clone)]
struct Removed {
    requirement: String,
    note: String,
}

#[derive(Serialize, Clone)]
struct Unverifiable {
    requirement: String,
    /// True when the floor fails it for the target's own u (VD6); false
    /// when no geometry can pass it (VD5).
    floor: bool,
}

fn socket_part(matcher: &Matcher<'_>) -> SocketPart {
    let document: &SocketDocument = matcher.document();
    let extracted = document.extracted();
    let unreviewed = matcher.unreviewed();
    let sockets = extracted
        .map(|e| e.sockets.as_slice())
        .unwrap_or_default()
        .iter()
        .filter_map(|socket| {
            let view = super::SocketView::new(document, &socket.id)?;
            let unverifiable = view
                .active()
                .iter()
                .filter_map(|r| {
                    if !r.verifiable() {
                        Some(Unverifiable {
                            requirement: r.id.clone(),
                            floor: false,
                        })
                    } else if view.blocked(r, matcher.policy()) {
                        Some(Unverifiable {
                            requirement: r.id.clone(),
                            floor: true,
                        })
                    } else {
                        None
                    }
                })
                .collect();
            Some(SocketSummary {
                id: socket.id.clone(),
                occurrences: socket.occurrences.len(),
                active: view.active().len(),
                unreviewed: unreviewed
                    .iter()
                    .find(|(id, _)| *id == socket.id)
                    .map_or(0, |u| u.1),
                removed: view
                    .removed()
                    .iter()
                    .zip(view.removal_notes())
                    .map(|(r, n)| Removed {
                        requirement: r.id.clone(),
                        note: (*n).to_owned(),
                    })
                    .collect(),
                unverifiable,
                left_out: socket.left_out.iter().map(|l| l.feature).collect(),
            })
        })
        .collect();
    SocketPart {
        digest: document.digest().map(str::to_owned),
        source: SourceOut {
            sha256: document.source().sha256.clone(),
            bytes: document.source().bytes,
        },
        target: extracted
            .and_then(|e| serde_json::to_value(&e.target.definition).ok())
            .unwrap_or(Value::Null),
        selection: document
            .selection()
            .and_then(|s| serde_json::to_value(s).ok())
            .unwrap_or(Value::Null),
        sockets,
    }
}

#[derive(Serialize, Clone)]
struct CandidateOut {
    labels: Vec<String>,
    source: Value,
    product: Option<ProductRef>,
    status: Status,
    refusals: Vec<Refusal>,
    read: Value,
    verdict: VerdictKind,
    fit: FitStatus,
    flags: Vec<CandidateFlag>,
    warnings: Vec<Warning>,
    properties: Value,
    reasons: Vec<RejectReason>,
    sockets: Vec<SocketOut>,
}

fn candidate(matcher: &Matcher<'_>, result: &CandidateResult, labels: &[String]) -> CandidateOut {
    let rejected = result.verdict.kind() == VerdictKind::Reject;
    CandidateOut {
        labels: labels.to_vec(),
        source: serde_json::to_value(&result.source).unwrap_or(Value::Null),
        product: result.product.clone(),
        status: result.status,
        refusals: result.refusals.clone(),
        read: serde_json::to_value(result.read).unwrap_or(Value::Null),
        verdict: result.verdict.kind(),
        fit: result.verdict.fit().clone(),
        flags: result.flags.clone(),
        warnings: result.warnings.clone(),
        properties: serde_json::json!({
            "material": {"status": "not_compared", "reason": "no property fitment maps in either file"}
        }),
        reasons: result.verdict.reasons().to_vec(),
        sockets: result
            .sockets
            .iter()
            .map(|s| socket_out(matcher, s, rejected))
            .collect(),
    }
}

#[derive(Serialize, Clone)]
struct SocketOut {
    socket: String,
    stage: Stage,
    verdict: VerdictKind,
    fit: FitStatus,
    placement: Option<PlacementOut>,
    missing: Vec<MissingOut>,
    failed: usize,
    checks: Vec<CheckOut>,
    not_checked: Vec<NotChecked>,
}

#[derive(Serialize, Clone)]
struct PlacementOut {
    rotation: [[f64; 3]; 3],
    translation_mm: [f64; 3],
    seeded_by: Seed,
    tried: usize,
    exhausted: bool,
    chosen: Chosen,
    u_mm: f64,
    epsilon_mm: f64,
}

#[derive(Serialize, Clone)]
struct MissingOut {
    requirements: Vec<RequirementId>,
    element: super::MissingElement,
    window: Option<Window>,
}

/// One check record (EV1): in full, or, for a rejected candidate's passed
/// check, its requirement, result and feature alone (M4-19).
#[derive(Serialize, Clone)]
struct CheckOut {
    requirement: String,
    review: ReviewStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    note: Option<String>,
    result: &'static str,
    feature: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    measures: Option<Vec<Measure>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    failures: Option<Vec<Failure>>,
}

fn feature(evidence: &Evidence) -> Option<String> {
    let names: Vec<String> = evidence.feature().iter().map(ToString::to_string).collect();
    (!names.is_empty()).then(|| names.join("+"))
}

fn socket_out(matcher: &Matcher<'_>, result: &SocketResult, rejected: bool) -> SocketOut {
    let document = matcher.document();
    let id = &result.socket;
    let mut checks: Vec<CheckOut> = Vec::new();
    // In the socket's requirement order, removed ones included.
    let order: Vec<String> = document
        .extracted()
        .and_then(|e| e.sockets.iter().find(|s| &s.id == id))
        .map(|s| s.requirements.iter().map(|r| r.id.clone()).collect())
        .unwrap_or_default();
    for requirement in &order {
        let review = document.decision(id, requirement);
        if let Some((_, check)) = result.checks.iter().find(|(r, _)| r.as_str() == requirement) {
            checks.push(check_out(requirement, review, check, rejected));
        } else if let Some(NotChecked::Removed { note, .. }) = result.not_checked.iter().find(
            |n| matches!(n, NotChecked::Removed { requirement: r, .. } if r.as_str() == requirement),
        ) {
            checks.push(CheckOut {
                requirement: requirement.clone(),
                review,
                note: Some(note.clone()),
                result: "not_checked",
                feature: None,
                measures: None,
                failures: None,
            });
        }
    }
    SocketOut {
        socket: id.clone(),
        stage: result.stage,
        verdict: result.verdict.kind(),
        fit: result.verdict.fit().clone(),
        placement: result.placement.as_ref().map(|p| PlacementOut {
            rotation: p.transform.rotation(),
            translation_mm: p.transform.translation(),
            seeded_by: p.seed.clone(),
            tried: result.placements.0,
            exhausted: result.placements.1,
            chosen: result.chosen,
            u_mm: result.u_mm,
            epsilon_mm: result.epsilon_mm,
        }),
        missing: result
            .missing
            .iter()
            .map(|m| MissingOut {
                requirements: m.requirements.clone(),
                element: m.element.clone(),
                window: m.window,
            })
            .collect(),
        failed: result.failed(),
        checks,
        not_checked: result.not_checked.clone(),
    }
}

fn check_out(requirement: &str, review: ReviewStatus, check: &Check, rejected: bool) -> CheckOut {
    let evidence = check.evidence();
    let passed = check.passed();
    let full = !(rejected && passed);
    CheckOut {
        requirement: requirement.to_owned(),
        review,
        note: None,
        result: if passed { "passed" } else { "failed" },
        feature: feature(evidence),
        measures: full.then(|| evidence.measures().to_vec()),
        failures: full.then(|| evidence.failures().to_vec()),
    }
}

/// Every candidate of `candidates` (bytes, label) judged against `socket`:
/// for tests and small sets, holding every candidate's bytes at once. The
/// CLI judges one file at a time instead (MF3).
///
/// # Errors
///
/// [`super::SocketRefused`] when the socket document fails MF2's gate.
pub fn match_candidates(
    socket: &SocketDocument,
    candidates: &[(&[u8], &str)],
    policy: super::MatchPolicy,
    options: MatchOptions,
    fit: &dyn FitVerifier,
) -> Result<MatchDocument, super::SocketRefused> {
    let matcher = Matcher::new(socket, policy, options, fit)?;
    let results = candidates
        .iter()
        .map(|(bytes, label)| (matcher.judge_step(bytes), vec![(*label).to_owned()]))
        .collect();
    Ok(MatchDocument::new(&matcher, results))
}
