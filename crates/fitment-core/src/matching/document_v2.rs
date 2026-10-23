//! `fitment.match` v2 (`docs/research/m7-fit.md`, FM1 to FM5, FC7): what
//! a run with a fit verifier that has an identity writes. It is v1 with
//! the verifier's identity, the fit's parameters and the original's
//! control (FM2), each fit's evidence (FM3), each placement's freedom and
//! alternatives (FM4, FM3), nothing removed; and it is written from its own
//! types, so v1's types, bytes and schema never change (FC7). The kernel's
//! numbers sit in `measured` objects outside the digest (FM5).

use serde::Serialize;
use serde_json::Value;

use super::document::{
    CheckOut, MATCH_FORMAT, MATCH_FORMAT_VERSION_FIT, MissingOut, PolicyOut, SocketPart, Summary,
    checks_out, missing_out, placement_out, policy, socket_part,
};
use super::matcher::{CandidateFlag, CandidateResult, NotChecked, SocketResult, Stage};
use super::{Chosen, MATCH_RULES_VERSION, MatchOptions, Matcher, SIGNATURE_VERSION, Seed};
use crate::features::{ProductRef, Status};
use crate::fit::{
    FIT_RULES_VERSION, FIT_VOLUME_MM3, FindingAt, FitBox, FitCode, FitEvidence, FitIdentity,
    FitStatus, FreeMotion, MotionKind, PairEvidence, PairInterference, PairMeasure,
    SCOPE_NOT_CHECKED, ZoneKind,
};
use crate::gate::Refusal;
use crate::geom::Rigid;
use crate::matching::{GUARD_MM, Warning};
use crate::verdict::{RejectReason, VerdictKind};

/// v2's text and digest for `entries`, judged by `matcher`, whose
/// verifier is `identity`.
pub(super) fn write(
    matcher: &Matcher<'_>,
    identity: &FitIdentity,
    entries: &[(CandidateResult, Vec<String>)],
    original: Option<&CandidateResult>,
    summary: Summary,
) -> (String, String) {
    let mut document = DocumentV2 {
        format: MATCH_FORMAT,
        format_version: MATCH_FORMAT_VERSION_FIT,
        extractor_version: crate::EXTRACTOR_VERSION,
        socket_rules_version: crate::SOCKET_RULES_VERSION,
        signature_version: SIGNATURE_VERSION,
        match_rules_version: MATCH_RULES_VERSION,
        policy: policy(matcher),
        options: matcher.options(),
        fit_verifier: VerifierV2::of(identity),
        fit: FitParameters::of(matcher),
        socket: socket_part(matcher),
        summary,
        candidates: entries
            .iter()
            .map(|(r, l)| candidate(matcher, r, l))
            .collect(),
        original: original.map(|r| candidate(matcher, r, &["original".to_owned()])),
        digest: String::new(),
    };
    let digest = digest_of(&document);
    document.digest.clone_from(&digest);
    let mut text = serde_json::to_string_pretty(&document).unwrap_or_default();
    text.push('\n');
    (text, digest)
}

/// FM5: the SHA-256 of the compact document, sorted by key, without its
/// digest, without every entry's labels and without every `measured`
/// object: the kernel's numbers vary in their last digits between
/// platforms where every decision agrees.
fn digest_of(document: &DocumentV2) -> String {
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
        if let Some(Value::Object(entry)) = map.get_mut("original") {
            entry.remove("labels");
        }
    }
    strip_measured(&mut value);
    format!("sha256:{}", crate::sha256_hex(value.to_string().as_bytes()))
}

/// Every `measured` object in `value`, removed.
fn strip_measured(value: &mut Value) {
    match value {
        Value::Object(map) => {
            map.remove("measured");
            map.values_mut().for_each(strip_measured);
        }
        Value::Array(items) => items.iter_mut().for_each(strip_measured),
        _ => {}
    }
}

/// CO4, FM7: the schema of the document v2 `to_json` writes, beside v1's
/// ([`super::MatchDocument`]'s). The document has no reader (M4-23).
#[cfg(feature = "schema")]
#[derive(Debug)]
#[non_exhaustive]
pub struct MatchDocumentV2;

#[cfg(feature = "schema")]
impl schemars::JsonSchema for MatchDocumentV2 {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "MatchDocumentV2".into()
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        <DocumentV2 as schemars::JsonSchema>::json_schema(generator)
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Serialize)]
struct DocumentV2 {
    format: &'static str,
    format_version: u32,
    extractor_version: u32,
    socket_rules_version: u32,
    signature_version: u32,
    match_rules_version: u32,
    policy: PolicyOut,
    options: MatchOptions,
    fit_verifier: VerifierV2,
    fit: FitParameters,
    socket: SocketPart,
    summary: Summary,
    candidates: Vec<CandidateV2>,
    #[serde(skip_serializing_if = "Option::is_none")]
    original: Option<CandidateV2>,
    digest: String,
}

/// FM2: the verifier's identity.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Serialize)]
struct VerifierV2 {
    kernel: String,
    kernel_version: String,
    binding: String,
    python: String,
    protocol: u32,
    helper_sha256: String,
    lock_sha256: String,
    fit_rules_version: u32,
}

impl VerifierV2 {
    fn of(identity: &FitIdentity) -> Self {
        Self {
            kernel: identity.kernel.clone(),
            kernel_version: identity.version.clone(),
            binding: identity.binding.clone(),
            python: identity.python.clone(),
            protocol: identity.protocol,
            helper_sha256: identity.helper_sha256.clone(),
            lock_sha256: identity.lock_sha256.clone(),
            fit_rules_version: FIT_RULES_VERSION,
        }
    }
}

/// FM2: the fit's parameters, and the control per socket, or `null` when
/// no candidate's interface passed (FT9).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Serialize)]
struct FitParameters {
    tolerance_mm: f64,
    guard_mm: f64,
    volume_resolution_mm3: f64,
    tolerance_cap: &'static str,
    max_fits: Option<u64>,
    control: Option<Vec<ControlV2>>,
}

impl FitParameters {
    fn of(matcher: &Matcher<'_>) -> Self {
        let control = matcher.fit_control().map(|evidence| {
            evidence
                .outcome()
                .sockets()
                .iter()
                .map(|(socket, status)| ControlV2 {
                    socket: socket.clone(),
                    fit: fit_v2(status, Some(&evidence), Some(socket)),
                })
                .collect()
        });
        Self {
            tolerance_mm: matcher.policy().linear_mm(),
            guard_mm: GUARD_MM,
            volume_resolution_mm3: FIT_VOLUME_MM3,
            tolerance_cap: "t/10",
            max_fits: matcher.max_fits(),
            control,
        }
    }
}

/// FT9: the original's fit at one socket.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Serialize)]
struct ControlV2 {
    socket: String,
    fit: FitV2,
}

/// FM3: a fit, by status, with its evidence; every fit states what no
/// fit checks (FT12).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum FitV2 {
    Verified {
        kernel: String,
        version: String,
        occurrences: usize,
        bodies: usize,
        copies: usize,
        pairs: Vec<PairV2>,
        free_turns: Vec<TurnV2>,
        scope_not_checked: Vec<&'static str>,
    },
    Interferes {
        interferences: Vec<PairV2>,
        scope_not_checked: Vec<&'static str>,
    },
    NotVerified {
        code: Option<FitCode>,
        reason: String,
        at: Vec<AtV2>,
        measured: FindingsMeasured,
        scope_not_checked: Vec<&'static str>,
    },
    NotChecked {
        reason: String,
        scope_not_checked: Vec<&'static str>,
    },
}

/// One pair (FM3): the other body, the pair's own tolerances (FT7) and
/// zones, and the kernel's measures, outside the digest.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Serialize)]
struct PairV2 {
    occurrence: Vec<u64>,
    body: BodyV2,
    u_pair_mm: f64,
    epsilon_mm: f64,
    threshold_mm: f64,
    zones: Vec<ZoneV2>,
    measured: Option<PairMeasured>,
}

/// The other body of a pair: a neighbour, or the candidate's copy at
/// another occurrence (FT3).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Serialize)]
struct BodyV2 {
    kind: BodyKind,
    path: Vec<u64>,
    product: Option<String>,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Serialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum BodyKind {
    Neighbour,
    Copy,
}

/// A zone of a pair: its kind and the requirement that declares it.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Serialize)]
struct ZoneV2 {
    kind: ZoneKind,
    requirement: String,
}

/// The kernel's measures of one pair (FT4, FT5, FY4): outside the digest.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Serialize)]
struct PairMeasured {
    common_mm3: f64,
    residual_mm3: f64,
    inside_mm3: f64,
    box_mm: Option<[[f64; 3]; 2]>,
    distance_mm: Option<f64>,
    zoned_distance_mm: Option<f64>,
    body_kept_mm3: Option<f64>,
    body_inside_mm3: Option<f64>,
    non_analytic: bool,
    alerts: Vec<String>,
}

impl PairMeasured {
    fn of(m: &PairMeasure) -> Self {
        Self {
            common_mm3: m.common_mm3,
            residual_mm3: m.residual_mm3,
            inside_mm3: m.inside_mm3,
            box_mm: m.residual_box.as_ref().map(corners),
            distance_mm: m.distance_mm,
            zoned_distance_mm: m.zoned_distance_mm,
            body_kept_mm3: m.body_kept_mm3,
            body_inside_mm3: m.body_inside_mm3,
            non_analytic: m.non_analytic,
            alerts: m.alerts.clone(),
        }
    }
}

fn corners(b: &FitBox) -> [[f64; 3]; 2] {
    [b.min_mm, b.max_mm]
}

/// An excused free turn (FT10): its socket and line, in the socket's
/// frame.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Serialize)]
struct TurnV2 {
    socket: String,
    about: [[f64; 3]; 2],
    axisymmetric: bool,
}

/// Where a reason a fit is not verified applies (FV9), with its code.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum AtV2 {
    Run {
        code: FitCode,
    },
    Candidate {
        code: FitCode,
    },
    Body {
        code: FitCode,
        path: Vec<u64>,
    },
    Placement {
        code: FitCode,
        occurrence: Vec<u64>,
    },
    /// A pair, with its own tolerances and zones (FT7) when it is one of
    /// the plan's, in the digest (FM5); `null` for a body the plan does not
    /// pair (FT8's `untested_body`).
    Pair {
        code: FitCode,
        occurrence: Vec<u64>,
        body: BodyV2,
        u_pair_mm: Option<f64>,
        epsilon_mm: Option<f64>,
        threshold_mm: Option<f64>,
        zones: Option<Vec<ZoneV2>>,
    },
    Socket {
        code: FitCode,
        socket: String,
    },
}

/// Each reason's sentence with its values, in the order found: outside
/// the digest, since a value may be the kernel's.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Serialize)]
struct FindingsMeasured {
    findings: Vec<String>,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Serialize)]
struct CandidateV2 {
    labels: Vec<String>,
    source: Value,
    product: Option<ProductRef>,
    status: Status,
    refusals: Vec<Refusal>,
    read: Value,
    verdict: VerdictKind,
    fit: FitV2,
    flags: Vec<CandidateFlag>,
    warnings: Vec<Warning>,
    properties: Value,
    reasons: Vec<RejectReason>,
    sockets: Vec<SocketV2>,
}

fn candidate(matcher: &Matcher<'_>, result: &CandidateResult, labels: &[String]) -> CandidateV2 {
    let rejected = result.verdict.kind() == VerdictKind::Reject;
    let evidence = result.fit.as_deref();
    CandidateV2 {
        labels: labels.to_vec(),
        source: serde_json::to_value(&result.source).unwrap_or(Value::Null),
        product: result.product.clone(),
        status: result.status,
        refusals: result.refusals.clone(),
        read: serde_json::to_value(result.read).unwrap_or(Value::Null),
        verdict: result.verdict.kind(),
        fit: fit_v2(result.verdict.fit(), evidence, None),
        flags: result.flags.clone(),
        warnings: result.warnings.clone(),
        properties: serde_json::json!({
            "material": {"status": "not_compared", "reason": "no property fitment maps in either file"}
        }),
        reasons: result.verdict.reasons().to_vec(),
        sockets: result
            .sockets
            .iter()
            .map(|s| socket_v2(matcher, s, evidence, rejected))
            .collect(),
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Serialize)]
struct SocketV2 {
    socket: String,
    stage: Stage,
    verdict: VerdictKind,
    fit: FitV2,
    placement: Option<PlacementV2>,
    alternatives: Option<Vec<TransformV2>>,
    missing: Vec<MissingOut>,
    failed: usize,
    checks: Vec<CheckOut>,
    not_checked: Vec<NotChecked>,
}

/// FM4: v1's placement with its free motions; `freedom` is `null` where
/// it was not worked out (FC2: only once every socket's interface passed).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Serialize)]
struct PlacementV2 {
    rotation: [[f64; 3]; 3],
    translation_mm: [f64; 3],
    seeded_by: Seed,
    tried: usize,
    exhausted: bool,
    chosen: Chosen,
    u_mm: f64,
    epsilon_mm: f64,
    freedom: Option<Vec<MotionV2>>,
}

/// One free motion (FM4): a turn about a line or a slide along one, in
/// the socket's frame, and whether the candidate is axisymmetric about it.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Serialize)]
struct MotionV2 {
    kind: MotionKind,
    about: [[f64; 3]; 2],
    axisymmetric: bool,
}

/// FM3: an alternative placement, as a transform.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Serialize)]
struct TransformV2 {
    rotation: [[f64; 3]; 3],
    translation_mm: [f64; 3],
}

impl TransformV2 {
    fn of(rigid: &Rigid) -> Self {
        Self {
            rotation: rigid.rotation(),
            translation_mm: rigid.translation(),
        }
    }
}

fn line(motion: &FreeMotion) -> [[f64; 3]; 2] {
    [
        motion.about.point_mm.to_array(),
        motion.about.direction.to_array(),
    ]
}

fn socket_v2(
    matcher: &Matcher<'_>,
    result: &SocketResult,
    evidence: Option<&FitEvidence>,
    rejected: bool,
) -> SocketV2 {
    let placement = placement_out(result).map(|p| PlacementV2 {
        rotation: p.rotation,
        translation_mm: p.translation_mm,
        seeded_by: p.seeded_by,
        tried: p.tried,
        exhausted: p.exhausted,
        chosen: p.chosen,
        u_mm: p.u_mm,
        epsilon_mm: p.epsilon_mm,
        freedom: result.freedom.as_ref().map(|f| {
            f.motions
                .iter()
                .map(|m| MotionV2 {
                    kind: m.kind,
                    about: line(m),
                    axisymmetric: m.axisymmetric,
                })
                .collect()
        }),
    });
    SocketV2 {
        socket: result.socket.clone(),
        stage: result.stage,
        verdict: result.verdict.kind(),
        fit: fit_v2(result.verdict.fit(), evidence, Some(&result.socket)),
        placement,
        alternatives: result
            .alternatives
            .as_ref()
            .map(|a| a.iter().map(TransformV2::of).collect()),
        missing: missing_out(result),
        failed: result.failed(),
        checks: checks_out(matcher, result, rejected),
        not_checked: result.not_checked.clone(),
    }
}

fn scope() -> Vec<&'static str> {
    SCOPE_NOT_CHECKED.to_vec()
}

/// FM3: `status`, at `socket` (or the candidate's, for `None`), with the
/// part of `evidence` that bears on it (FV7).
fn fit_v2(status: &FitStatus, evidence: Option<&FitEvidence>, socket: Option<&str>) -> FitV2 {
    match (status, evidence) {
        (
            FitStatus::Verified {
                kernel, version, ..
            },
            evidence,
        ) => {
            let (occurrences, bodies, copies, pairs, free_turns) =
                evidence.map_or_else(Default::default, |e| verified_parts(e, socket));
            FitV2::Verified {
                kernel: kernel.clone(),
                version: version.clone(),
                occurrences,
                bodies,
                copies,
                pairs,
                free_turns,
                scope_not_checked: scope(),
            }
        }
        (FitStatus::Interferes { .. }, Some(e)) => FitV2::Interferes {
            interferences: e
                .outcome()
                .interferences()
                .iter()
                .filter(|i| e.touches(socket, &i.occurrence, &i.body, i.copy))
                .map(|i| interference(e, i))
                .collect(),
            scope_not_checked: scope(),
        },
        (FitStatus::Interferes { .. }, None) => FitV2::Interferes {
            interferences: Vec::new(),
            scope_not_checked: scope(),
        },
        (FitStatus::NotVerified { reason }, evidence) => not_verified(reason, evidence, socket),
        (FitStatus::NotChecked { reason }, _) => FitV2::NotChecked {
            reason: reason.clone(),
            scope_not_checked: scope(),
        },
    }
}

type VerifiedParts = (usize, usize, usize, Vec<PairV2>, Vec<TurnV2>);

/// A verified fit's counts, pairs and excused turns at `socket`.
fn verified_parts(evidence: &FitEvidence, socket: Option<&str>) -> VerifiedParts {
    let occurrences = evidence
        .placements()
        .iter()
        .filter(|p| socket.is_none_or(|s| p.socket == s))
        .count();
    let copies = evidence.placements().len().saturating_sub(1);
    let pairs = evidence
        .pairs()
        .iter()
        .filter(|p| evidence.touches(socket, &p.pair.occurrence, &p.pair.body, p.pair.copy))
        .map(pair_v2)
        .collect();
    let turns = evidence
        .free_turns()
        .iter()
        .filter(|(s, _)| socket.is_none_or(|socket| s == socket))
        .map(|(s, m)| TurnV2 {
            socket: s.clone(),
            about: line(m),
            axisymmetric: m.axisymmetric,
        })
        .collect();
    (occurrences, evidence.bodies(), copies, pairs, turns)
}

fn body_v2(copy: bool, path: &[u64], product: Option<String>) -> BodyV2 {
    BodyV2 {
        kind: if copy {
            BodyKind::Copy
        } else {
            BodyKind::Neighbour
        },
        path: path.to_vec(),
        product,
    }
}

fn pair_v2(evidence: &PairEvidence) -> PairV2 {
    let pair = &evidence.pair;
    PairV2 {
        occurrence: pair.occurrence.clone(),
        body: body_v2(pair.copy, &pair.body, evidence.product.clone()),
        u_pair_mm: pair.values.u_pair_mm,
        epsilon_mm: pair.values.epsilon_mm,
        threshold_mm: pair.values.threshold_mm,
        zones: pair
            .zones
            .iter()
            .map(|z| ZoneV2 {
                kind: z.zone.kind,
                requirement: z.zone.requirement.clone(),
            })
            .collect(),
        measured: evidence.measure.as_ref().map(PairMeasured::of),
    }
}

/// FV3: an interference as its pair, with the kernel's measures.
fn interference(evidence: &FitEvidence, found: &PairInterference) -> PairV2 {
    let pair = evidence.pairs().iter().find(|p| {
        p.pair.occurrence == found.occurrence
            && p.pair.body == found.body
            && p.pair.copy == found.copy
    });
    match pair {
        Some(pair) => pair_v2(pair),
        None => PairV2 {
            occurrence: found.occurrence.clone(),
            body: body_v2(found.copy, &found.body, None),
            u_pair_mm: found.values.u_pair_mm,
            epsilon_mm: found.values.epsilon_mm,
            threshold_mm: found.values.threshold_mm,
            zones: Vec::new(),
            measured: Some(PairMeasured {
                common_mm3: found.common_mm3,
                residual_mm3: found.residual_mm3,
                inside_mm3: found.common_mm3 - found.residual_mm3,
                box_mm: found.residual_box.as_ref().map(corners),
                distance_mm: None,
                zoned_distance_mm: None,
                body_kept_mm3: None,
                body_inside_mm3: None,
                non_analytic: false,
                alerts: Vec::new(),
            }),
        },
    }
}

/// FM3, FV9: a fit not verified: the first reason's code and its fixed
/// sentence, where each reason applies, and each reason's sentence with
/// its values outside the digest. Without evidence, the status's own
/// reason and no code.
fn not_verified(reason: &str, evidence: Option<&FitEvidence>, socket: Option<&str>) -> FitV2 {
    let Some(evidence) = evidence else {
        return FitV2::NotVerified {
            code: None,
            reason: reason.to_owned(),
            at: Vec::new(),
            measured: FindingsMeasured {
                findings: Vec::new(),
            },
            scope_not_checked: scope(),
        };
    };
    let findings: Vec<_> = evidence
        .outcome()
        .findings()
        .iter()
        .filter(|f| evidence.applies(socket, &f.at))
        .collect();
    let code = findings.first().map(|f| f.code);
    let product = |path: &[u64]| {
        evidence
            .pairs()
            .iter()
            .find(|p| !p.pair.copy && p.pair.body == path)
            .and_then(|p| p.product.clone())
    };
    let at = findings
        .iter()
        .map(|f| {
            let code = f.code;
            match &f.at {
                FindingAt::Candidate => AtV2::Candidate { code },
                FindingAt::Body { path } => AtV2::Body {
                    code,
                    path: path.clone(),
                },
                FindingAt::Placement { occurrence } => AtV2::Placement {
                    code,
                    occurrence: occurrence.clone(),
                },
                FindingAt::Pair {
                    occurrence,
                    body,
                    copy,
                } => {
                    let pair = evidence.pairs().iter().find(|p| {
                        p.pair.occurrence == *occurrence
                            && p.pair.body == *body
                            && p.pair.copy == *copy
                    });
                    let values = pair.map(pair_v2);
                    AtV2::Pair {
                        code,
                        occurrence: occurrence.clone(),
                        body: body_v2(*copy, body, (!copy).then(|| product(body)).flatten()),
                        u_pair_mm: values.as_ref().map(|v| v.u_pair_mm),
                        epsilon_mm: values.as_ref().map(|v| v.epsilon_mm),
                        threshold_mm: values.as_ref().map(|v| v.threshold_mm),
                        zones: values.map(|v| v.zones),
                    }
                }
                FindingAt::Socket { socket } => AtV2::Socket {
                    code,
                    socket: socket.clone(),
                },
                FindingAt::Run => AtV2::Run { code },
            }
        })
        .collect();
    FitV2::NotVerified {
        code,
        reason: code.map_or_else(|| reason.to_owned(), |c| c.sentence().to_owned()),
        at,
        measured: FindingsMeasured {
            findings: findings.iter().map(|f| f.sentence()).collect(),
        },
        scope_not_checked: scope(),
    }
}
