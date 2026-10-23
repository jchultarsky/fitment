//! FH2 to FH5: the helper's operations, `hello`, `scene`, `place`,
//! `check` and `bye`, each one frame each way. A request's header is
//! `{"op", "nonce", "payloads", "digest", "request"}` and its reply's
//! `{"op", "nonce", "request_sha256", "digest", "reply"}`: every reply
//! echoes its request's fresh 128-bit nonce, the SHA-256 of the request's
//! header bytes, and core's digest of what the request asks
//! (`FitScene::request_digest`, `PlanInput::place_digest`,
//! `FitPlan::check_digest`), which core checks again when it decides.
//! A reply is decoded strictly: unknown and missing keys, a list out of
//! the request's order, a pair or body not asked, and numbers out of
//! their ranges (FY4) are each a protocol error.

pub(crate) mod wire;

use fitment_core::fit::{
    CheckMeasures, FitBody, FitPlan, FitScene, Pair, PlaceMeasures, SceneMeasures,
};
use fitment_core::{FitRequest, Rigid, sha256_hex};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::frame;
use wire::{
    CheckReply, ErrorBody, HelloReply, PlaceReply, Rows, SceneReply, WireFace, WireShape, finite,
    finite_rows,
};

/// The slack a volume may exceed the Common by, as fitment-core's FY4
/// allows it: 1e-9 of the Common, never a tenth of the volume resolution.
const SLACK_REL: f64 = 1e-9;
const SLACK_MM3: f64 = fitment_core::fit::FIT_VOLUME_MM3 / 10.0;

/// An operation (FH4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Op {
    Hello,
    Scene,
    Place,
    Check,
    SelfTest,
    Bye,
}

impl Op {
    pub fn name(self) -> &'static str {
        match self {
            Self::Hello => "hello",
            Self::Scene => "scene",
            Self::Place => "place",
            Self::Check => "check",
            Self::SelfTest => "selftest",
            Self::Bye => "bye",
        }
    }
}

/// What a request's reply must answer: its operation, nonce, header and
/// digest, and the keys of every list it must hold, in order.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Sent {
    pub op: Op,
    pub nonce: String,
    pub header_sha256: String,
    pub digest: Option<String>,
    pub expect: Expect,
}

/// The lists a reply must hold (FH5).
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Expect {
    Nothing,
    /// Each body's key and its number of landmarks.
    Scene(Vec<(Vec<u64>, usize)>),
    /// Each placement's occurrence, and the candidate's landmarks.
    Place(Vec<Vec<u64>>, usize),
    /// Each pair's keys and its number of zones.
    Check(Vec<PairKey>),
}

/// One asked pair's keys (FT3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PairKey {
    pub occurrence: Vec<u64>,
    pub body: Vec<u64>,
    pub copy: bool,
    pub zones: usize,
}

/// One request, encoded, and what its reply must answer.
#[derive(Debug, Clone)]
pub(crate) struct Request {
    pub sent: Sent,
    pub frame: Vec<u8>,
}

/// Why a request cannot be sent: core handed the protocol something it
/// cannot name (a kind newer than protocol 1, a number not finite).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("the request cannot be encoded: {0}")]
pub(crate) struct EncodeError(pub String);

/// Why a reply is refused (FH2, FH5, FY4): a sentence for the log.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub(crate) struct ProtocolError(pub String);

fn refuse<T>(why: impl Into<String>) -> Result<T, ProtocolError> {
    Err(ProtocolError(why.into()))
}

#[derive(Serialize)]
struct Header<'a, B> {
    op: &'static str,
    nonce: &'a str,
    payloads: Vec<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    digest: Option<&'a str>,
    request: B,
}

/// The request `op` with `body`, its payloads and core's `digest`.
fn request<B: Serialize>(
    op: Op,
    nonce: String,
    digest: Option<String>,
    body: B,
    payloads: &[&[u8]],
    expect: Expect,
) -> Result<Request, EncodeError> {
    let header = Header {
        op: op.name(),
        nonce: &nonce,
        payloads: payloads.iter().map(|p| p.len() as u64).collect(),
        digest: digest.as_deref(),
        request: body,
    };
    let header = serde_json::to_vec(&header).map_err(|e| EncodeError(e.to_string()))?;
    let bytes = frame::encode(&header, payloads).map_err(|e| EncodeError(e.to_string()))?;
    Ok(Request {
        sent: Sent {
            op,
            nonce,
            header_sha256: sha256_hex(&header),
            digest,
            expect,
        },
        frame: bytes,
    })
}

/// What `hello` states it expects of the helper: the manifest's values
/// (FL2, FH3), and the bounds the helper sets on itself before it imports
/// Open CASCADE (FY3, FL4): the memory it may map, a candidate's deadline,
/// `scene`'s, and the checks it serves before it is retired.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct HelloRequest {
    pub protocol: u32,
    pub helper_sha256: String,
    pub lock_sha256: String,
    pub binding: String,
    pub kernel: String,
    pub python: String,
    pub site: String,
    pub timeout_seconds: u64,
    pub setup_timeout_seconds: u64,
    pub memory_mib: u64,
    pub max_checks: u32,
}

/// FH3: the first frame.
pub(crate) fn hello(nonce: String, body: &HelloRequest) -> Result<Request, EncodeError> {
    request(Op::Hello, nonce, None, body, &[], Expect::Nothing)
}

/// FH4: `bye`, which the helper answers by exiting 0.
pub(crate) fn bye(nonce: String) -> Result<Request, EncodeError> {
    request(
        Op::Bye,
        nonce,
        None,
        serde_json::Map::new(),
        &[],
        Expect::Nothing,
    )
}

#[derive(Serialize)]
struct SceneBodyRequest<'a> {
    key: &'a [u64],
    faces: &'a [u64],
    #[serde(serialize_with = "finite_rows")]
    transform: Rows,
    landmarks: Vec<WireFace>,
}

#[derive(Serialize)]
struct SceneRequest<'a> {
    scene: &'a str,
    bodies: Vec<SceneBodyRequest<'a>>,
}

/// `[R | t]` as three rows.
pub(crate) fn rows(rigid: &Rigid) -> Rows {
    let (r, t) = (rigid.rotation(), rigid.translation());
    [0, 1, 2].map(|i| [r[i][0], r[i][1], r[i][2], t[i]])
}

/// FH4's `scene`: the assembly's bytes and every body core names (FB2,
/// FB3).
pub(crate) fn scene(
    nonce: String,
    scene: &FitScene,
    assembly: &[u8],
) -> Result<Request, EncodeError> {
    let bodies = scene
        .bodies()
        .iter()
        .map(|b| {
            Ok(SceneBodyRequest {
                key: &b.path,
                faces: &b.faces,
                transform: rows(&b.transform),
                landmarks: WireFace::all(&b.landmarks)
                    .ok_or_else(|| EncodeError("a landmark of an unknown kind".to_owned()))?,
            })
        })
        .collect::<Result<Vec<_>, EncodeError>>()?;
    let expect = Expect::Scene(
        scene
            .bodies()
            .iter()
            .map(|b| (b.path.clone(), b.landmarks.len()))
            .collect(),
    );
    let body = SceneRequest {
        scene: scene.sha256(),
        bodies,
    };
    request(
        Op::Scene,
        nonce,
        Some(scene.request_digest()),
        body,
        &[assembly],
        expect,
    )
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum CandidateRequest<'a> {
    Step {
        sha256: &'a str,
    },
    Component {
        assembly_sha256: &'a str,
        occurrence: &'a [u64],
        faces: &'a [u64],
    },
}

#[derive(Serialize)]
struct PlacementRequest<'a> {
    socket: &'a str,
    occurrence: &'a [u64],
    #[serde(serialize_with = "finite_rows")]
    transform: Rows,
}

#[derive(Serialize)]
struct PlaceRequest<'a> {
    scene: &'a str,
    candidate: CandidateRequest<'a>,
    landmarks: Vec<WireFace>,
    placements: Vec<PlacementRequest<'a>>,
}

/// FH4's `place`: the candidate (a file's bytes as the payload, or a
/// component by its faces), its landmarks and every placement (FT2, FB3,
/// FB4).
pub(crate) fn place(nonce: String, fit: &FitRequest<'_>) -> Result<Request, EncodeError> {
    let (candidate, payload): (CandidateRequest<'_>, Option<&[u8]>) = match fit.body() {
        FitBody::Step { bytes, sha256, .. } => (CandidateRequest::Step { sha256 }, Some(bytes)),
        FitBody::Component {
            assembly_sha256,
            occurrence,
            faces,
            ..
        } => (
            CandidateRequest::Component {
                assembly_sha256,
                occurrence,
                faces,
            },
            None,
        ),
        _ => {
            return Err(EncodeError(
                "a candidate body of an unknown kind".to_owned(),
            ));
        }
    };
    let landmarks = fit.candidate().landmarks();
    let body = PlaceRequest {
        scene: fit.scene().sha256(),
        candidate,
        landmarks: WireFace::all(landmarks)
            .ok_or_else(|| EncodeError("a landmark of an unknown kind".to_owned()))?,
        placements: fit
            .placements()
            .iter()
            .map(|p| PlacementRequest {
                socket: &p.socket,
                occurrence: &p.occurrence,
                transform: rows(&p.transform),
            })
            .collect(),
    };
    let expect = Expect::Place(
        fit.placements()
            .iter()
            .map(|p| p.occurrence.clone())
            .collect(),
        landmarks.len(),
    );
    let payloads: Vec<&[u8]> = payload.into_iter().collect();
    request(
        Op::Place,
        nonce,
        Some(fit.place_digest()),
        body,
        &payloads,
        expect,
    )
}

#[derive(Serialize)]
struct ZoneRequest<'a> {
    kind: fitment_core::fit::ZoneKind,
    requirement: &'a str,
    shape: WireShape,
    #[serde(serialize_with = "finite")]
    depth_mm: f64,
    #[serde(serialize_with = "finite")]
    proximity_depth_mm: f64,
}

#[derive(Serialize)]
struct PairRequest<'a> {
    occurrence: &'a [u64],
    body: &'a [u64],
    copy: bool,
    #[serde(serialize_with = "finite")]
    u_pair_mm: f64,
    #[serde(serialize_with = "finite")]
    epsilon_mm: f64,
    #[serde(serialize_with = "finite")]
    threshold_mm: f64,
    zones: Vec<ZoneRequest<'a>>,
}

#[derive(Serialize)]
struct CheckRequest<'a> {
    /// The policy's linear tolerance, by which the helper states its own
    /// claims of reach (FT8).
    #[serde(serialize_with = "finite")]
    linear_mm: f64,
    pairs: Vec<PairRequest<'a>>,
    /// VT71, under `test-support` only: the pair at which the helper
    /// raises, as if its kernel had, so its alert comes back for that pair
    /// alone. A release build never sends it.
    #[serde(skip_serializing_if = "Option::is_none")]
    raise_at: Option<usize>,
}

fn pair_request(pair: &Pair) -> Result<PairRequest<'_>, EncodeError> {
    let unbuilt = || EncodeError("an asked pair's zone without a closed form".to_owned());
    let zones = pair
        .zones
        .iter()
        .map(|z| {
            Ok(ZoneRequest {
                kind: z.zone.kind,
                requirement: &z.zone.requirement,
                shape: WireShape::of(&z.zone.shape).ok_or_else(unbuilt)?,
                depth_mm: z.interference.ok_or_else(unbuilt)?.depth_mm,
                proximity_depth_mm: z.proximity.ok_or_else(unbuilt)?.depth_mm,
            })
        })
        .collect::<Result<_, EncodeError>>()?;
    Ok(PairRequest {
        occurrence: &pair.occurrence,
        body: &pair.body,
        copy: pair.copy,
        u_pair_mm: pair.values.u_pair_mm,
        epsilon_mm: pair.values.epsilon_mm,
        threshold_mm: pair.values.threshold_mm,
        zones,
    })
}

/// FH4's `check`: every pair the plan asks, with its tolerances and its
/// zones at both depths (FT4, FT5, FT7, FZ6); `raise_at` is VT71's.
pub(crate) fn check(
    nonce: String,
    plan: &FitPlan,
    raise_at: Option<usize>,
) -> Result<Request, EncodeError> {
    let pairs = plan
        .asked()
        .map(pair_request)
        .collect::<Result<Vec<_>, _>>()?;
    let expect = Expect::Check(
        plan.asked()
            .map(|p| PairKey {
                occurrence: p.occurrence.clone(),
                body: p.body.clone(),
                copy: p.copy,
                zones: p.zones.len(),
            })
            .collect(),
    );
    request(
        Op::Check,
        nonce,
        Some(plan.check_digest().to_owned()),
        CheckRequest {
            linear_mm: plan.linear_mm(),
            pairs,
            raise_at,
        },
        &[],
        expect,
    )
}

/// What `selftest` may ask besides the self-test itself, for VT70 only:
/// memory to take and CPU to burn, so a test sees the helper's own bounds
/// end it. A release build never sends either.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub(crate) struct Stress {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allocate_mib: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kernel_allocate_mib: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_seconds: Option<u64>,
}

#[derive(Serialize)]
struct SelfTestRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    stress: Option<Stress>,
}

/// FU4's self-test: two unit boxes touching, overlapping by 1e-3 mm and
/// apart by 0.02 mm, each measured by the kernel; `stress` is VT70's.
pub(crate) fn self_test(nonce: String, stress: Option<Stress>) -> Result<Request, EncodeError> {
    request(
        Op::SelfTest,
        nonce,
        None,
        SelfTestRequest { stress },
        &[],
        Expect::Nothing,
    )
}

/// FB5: the `scene` request of a component's own assembly when it is not
/// the socket's: its bytes, naming no body, which the helper holds as its
/// second scene and finds the component in by its faces. Its digest names
/// the assembly; core plans nothing from its reply.
pub(crate) fn component_scene(nonce: String, assembly: &[u8]) -> Result<Request, EncodeError> {
    let sha256 = sha256_hex(assembly);
    let body = SceneRequest {
        scene: &sha256,
        bodies: Vec::new(),
    };
    request(
        Op::Scene,
        nonce,
        Some(component_scene_digest(&sha256)),
        body,
        &[assembly],
        Expect::Scene(Vec::new()),
    )
}

/// FB5: the digest a component scene's request carries, for its
/// assembly's SHA-256.
pub(crate) fn component_scene_digest(sha256: &str) -> String {
    format!("component-scene-{sha256}")
}

/// VT71, for the tests: a `scene` request for `assembly` that names no
/// body, claiming its SHA-256 is `sha256`.
#[cfg(feature = "test-support")]
pub(crate) fn scene_probe(
    nonce: String,
    sha256: &str,
    assembly: &[u8],
) -> Result<Request, EncodeError> {
    let body = SceneRequest {
        scene: sha256,
        bodies: Vec::new(),
    };
    let digest = Some(format!("probe {sha256}"));
    request(
        Op::Scene,
        nonce,
        digest,
        body,
        &[assembly],
        Expect::Scene(Vec::new()),
    )
}

/// VT71, for the tests: a `place` request for the file candidate `bytes`
/// at `placements`, with no landmark, in the scene `scene`.
#[cfg(feature = "test-support")]
pub(crate) fn place_probe(
    nonce: String,
    scene: &str,
    bytes: &[u8],
    placements: &[crate::session::ProbeAt],
) -> Result<Request, EncodeError> {
    let sha256 = sha256_hex(bytes);
    let body = PlaceRequest {
        scene,
        candidate: CandidateRequest::Step { sha256: &sha256 },
        landmarks: Vec::new(),
        placements: placements
            .iter()
            .map(|(socket, occurrence, at)| PlacementRequest {
                socket,
                occurrence,
                transform: rows(at),
            })
            .collect(),
    };
    let expect = Expect::Place(placements.iter().map(|p| p.1.clone()).collect(), 0);
    let digest = Some(format!("probe {sha256}"));
    request(Op::Place, nonce, digest, body, &[bytes], expect)
}

/// Only the operation, to tell an `error` frame from a reply.
#[derive(Deserialize)]
struct Head {
    op: String,
}

/// A reply's header (FH2).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope<T> {
    op: String,
    nonce: String,
    request_sha256: String,
    #[serde(default)]
    digest: Option<String>,
    #[serde(default)]
    payloads: Vec<u64>,
    reply: T,
}

/// An `error` frame's header (FH5).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ErrorEnvelope {
    // FH5: every key is known, though only the error is read; the helper
    // may not have read the request's nonce, so it may send none.
    #[serde(rename = "op")]
    _op: String,
    #[serde(rename = "nonce", default)]
    _nonce: Option<String>,
    #[serde(rename = "request_sha256", default)]
    _request_sha256: Option<String>,
    #[serde(rename = "payloads", default)]
    _payloads: Vec<u64>,
    error: ErrorBody,
}

/// Text a helper sent, made safe for a sentence and a log: every byte
/// that is not printable ASCII escaped, and capped at 200 characters
/// (the reviews of PR 2a, L3).
pub(crate) fn quoted(text: &str) -> String {
    let escaped = text.as_bytes().escape_ascii().to_string();
    escaped.chars().take(200).collect()
}

/// A reply decoded: its body, and core's digest of the request as the
/// helper echoed it (which core checks again when it decides).
pub(crate) struct Echoed<T> {
    pub reply: T,
    pub digest: Option<String>,
}

/// FH2, FH5: the reply to `sent` in `header`, its envelope checked: the
/// operation, nonce, request header's SHA-256 and core's digest echoed,
/// no payload. An `error` frame is a protocol error with the helper's
/// code. Every string the helper sent that a refusal repeats, serde's
/// messages included, is [`quoted`].
pub(crate) fn reply<T: DeserializeOwned>(
    header: &[u8],
    sent: &Sent,
) -> Result<Echoed<T>, ProtocolError> {
    let broke = |what: &str, e: &serde_json::Error| {
        ProtocolError(format!("{what}: {}", quoted(&e.to_string())))
    };
    let head: Head =
        serde_json::from_slice(header).map_err(|e| broke("a reply's header is not a reply", &e))?;
    if head.op == "error" {
        let error: ErrorEnvelope =
            serde_json::from_slice(header).map_err(|e| broke("an error frame breaks FH5", &e))?;
        return refuse(format!(
            "the helper refused the {} request: {} ({})",
            sent.op.name(),
            quoted(&error.error.code),
            quoted(&error.error.sentence)
        ));
    }
    let envelope: Envelope<T> = serde_json::from_slice(header)
        .map_err(|e| broke(&format!("the {} reply breaks FH5", sent.op.name()), &e))?;
    if envelope.op != sent.op.name() {
        return refuse(format!(
            "a {} reply to a {} request",
            quoted(&envelope.op),
            sent.op.name()
        ));
    }
    if envelope.nonce != sent.nonce {
        return refuse("a reply whose nonce is not its request's");
    }
    if envelope.request_sha256 != sent.header_sha256 {
        return refuse("a reply whose request digest is not its request's");
    }
    if envelope.digest != sent.digest {
        return refuse("a reply whose digest of the request is not core's");
    }
    if !envelope.payloads.is_empty() {
        return refuse("a reply that announces a payload");
    }
    Ok(Echoed {
        reply: envelope.reply,
        digest: envelope.digest,
    })
}

/// FY4: a volume, tolerance or distance is never negative.
fn non_negative(what: &str, x: f64) -> Result<(), ProtocolError> {
    if x.is_finite() && x >= 0.0 {
        Ok(())
    } else {
        refuse(format!("a {what} that is negative or not finite"))
    }
}

fn sorted<T: Ord>(xs: &[T]) -> bool {
    xs.windows(2).all(|w| w[0] <= w[1])
}

/// FH4, FH5, FY4: the `scene` reply as measures.
pub(crate) fn scene_measures(header: &[u8], sent: &Sent) -> Result<SceneMeasures, ProtocolError> {
    let Echoed {
        reply: body,
        digest,
    } = reply::<SceneReply>(header, sent)?;
    let Expect::Scene(expected) = &sent.expect else {
        return refuse("a scene reply to another request");
    };
    if body.bodies.len() != expected.len() {
        return refuse("a scene reply with a body missing or extra");
    }
    for (b, (key, landmarks)) in body.bodies.iter().zip(expected) {
        if &b.key != key {
            return refuse("a scene reply with a body not asked, or out of order");
        }
        if b.landmarks.len() != *landmarks {
            return refuse("a scene reply with a landmark missing or extra");
        }
        non_negative("body's volume", b.volume_mm3)?;
        non_negative("body's tolerance", b.tolerance_mm)?;
    }
    if body.solids.iter().any(|s| !sorted(&s.faces)) {
        return refuse("a scene reply with a solid's faces unsorted");
    }
    let mut out = SceneMeasures::default();
    // The helper's echo, which core checks against its own digest.
    out.request_sha256 = digest;
    out.bodies = body
        .bodies
        .into_iter()
        .map(wire::WireBody::measure)
        .collect();
    out.solids = body
        .solids
        .into_iter()
        .map(wire::WireSolid::measure)
        .collect();
    Ok(out)
}

/// FH4, FH5, FY4: the `place` reply as measures.
pub(crate) fn place_measures(header: &[u8], sent: &Sent) -> Result<PlaceMeasures, ProtocolError> {
    let Echoed {
        reply: body,
        digest,
    } = reply::<PlaceReply>(header, sent)?;
    let Expect::Place(occurrences, landmarks) = &sent.expect else {
        return refuse("a place reply to another request");
    };
    if body.placements.len() != occurrences.len()
        || body
            .placements
            .iter()
            .zip(occurrences)
            .any(|(p, o)| &p.occurrence != o)
    {
        return refuse("a place reply with a placement not asked, missing or out of order");
    }
    if body
        .placements
        .iter()
        .any(|p| p.landmarks.len() != *landmarks)
    {
        return refuse("a place reply with a landmark missing or extra");
    }
    non_negative("candidate's volume", body.volume_mm3)?;
    non_negative("candidate's tolerance", body.tolerance_mm)?;
    let mut out = PlaceMeasures::default();
    // The helper's echo, which core checks against its own digest.
    out.request_sha256 = digest;
    out.valid = body.valid;
    out.volume_mm3 = wire::zero(body.volume_mm3);
    out.tolerance_mm = wire::zero(body.tolerance_mm);
    out.sha256 = body.sha256;
    out.placements = body
        .placements
        .into_iter()
        .map(wire::WirePlacement::measure)
        .collect();
    Ok(out)
}

/// FY4's ranges for one pair: nothing negative, and neither the residual
/// nor the volume within the zones above the Common beyond core's slack.
fn pair_in_range(p: &wire::WirePair) -> Result<(), ProtocolError> {
    for (what, x) in [
        ("Common", Some(p.common_mm3)),
        ("residual", Some(p.residual_mm3)),
        ("volume within the zones", Some(p.inside_mm3)),
        ("distance", p.distance_mm),
        ("zoned distance", p.zoned_distance_mm),
        ("body's kept volume", p.body_kept_mm3),
        ("body's volume within the zones", p.body_inside_mm3),
    ] {
        if let Some(x) = x {
            non_negative(what, x)?;
        }
    }
    for z in p.zones.iter().chain(&p.proximity_zones) {
        non_negative("zone's volume", z.volume_mm3)?;
    }
    let slack = (SLACK_REL * p.common_mm3).min(SLACK_MM3);
    if p.residual_mm3 > p.common_mm3 + slack {
        return refuse("a pair whose residual is above its Common");
    }
    if p.inside_mm3 > p.common_mm3 + slack {
        return refuse("a pair with more within its zones than in its Common");
    }
    Ok(())
}

/// FH4, FH5, FY4: the `check` reply as measures.
pub(crate) fn check_measures(header: &[u8], sent: &Sent) -> Result<CheckMeasures, ProtocolError> {
    let Echoed {
        reply: body,
        digest,
    } = reply::<CheckReply>(header, sent)?;
    let Expect::Check(keys) = &sent.expect else {
        return refuse("a check reply to another request");
    };
    if body.pairs.len() != keys.len() {
        return refuse("a check reply with a pair missing or extra");
    }
    for (p, key) in body.pairs.iter().zip(keys) {
        if p.occurrence != key.occurrence || p.body != key.body || p.copy != key.copy {
            return refuse("a check reply with a pair not asked, or out of order");
        }
        if p.zones.len() != key.zones
            || !(p.proximity_zones.is_empty() || p.proximity_zones.len() == key.zones)
        {
            return refuse("a check reply with a zone missing or extra");
        }
        if !sorted(&p.alerts) {
            return refuse("a check reply with its alerts unsorted");
        }
        pair_in_range(p)?;
    }
    let mut out = CheckMeasures::default();
    // The helper's echo, which core checks against its own digest.
    out.request_sha256 = digest;
    out.pairs = body
        .pairs
        .into_iter()
        .map(wire::WirePair::measure)
        .collect();
    out.reach = body
        .reach
        .into_iter()
        .map(wire::WireReach::measure)
        .collect();
    Ok(out)
}

/// FH3: the `hello` reply, its envelope checked; the identity is the
/// session's to judge.
pub(crate) fn hello_reply(header: &[u8], sent: &Sent) -> Result<HelloReply, ProtocolError> {
    reply(header, sent).map(|e| e.reply)
}

/// FU4: the `selftest` reply, its envelope checked and its three cases
/// named in order; whether each is right is the helper's caller's to
/// judge.
pub(crate) fn self_test_reply(
    header: &[u8],
    sent: &Sent,
) -> Result<wire::SelfTestReply, ProtocolError> {
    let body: wire::SelfTestReply = reply(header, sent)?.reply;
    let names: Vec<&str> = body.cases.iter().map(|c| c.name.as_str()).collect();
    if names != wire::SELF_TEST_CASES {
        return refuse("a selftest reply whose cases are not the three asked");
    }
    if body.cases.iter().any(|c| !sorted(&c.alerts)) {
        return refuse("a selftest reply with its alerts unsorted");
    }
    Ok(body)
}

/// The reply bodies a kernel would send for `measures`, for the fake
/// helper and for comparing two helpers' scenes bit for bit (FL3).
pub(crate) fn scene_reply(measures: &SceneMeasures) -> SceneReply {
    SceneReply {
        bodies: measures.bodies.iter().map(wire::WireBody::of).collect(),
        solids: measures.solids.iter().map(wire::WireSolid::of).collect(),
    }
}

#[cfg(any(test, feature = "test-support"))]
/// As [`scene_reply`], for `place`.
pub(crate) fn place_reply(measures: &PlaceMeasures) -> PlaceReply {
    PlaceReply {
        valid: measures.valid,
        volume_mm3: measures.volume_mm3,
        tolerance_mm: measures.tolerance_mm,
        sha256: measures.sha256.clone(),
        placements: measures
            .placements
            .iter()
            .map(wire::WirePlacement::of)
            .collect(),
    }
}

#[cfg(any(test, feature = "test-support"))]
/// As [`scene_reply`], for `check`, each pair named by the plan's keys.
pub(crate) fn check_reply(measures: &CheckMeasures, plan: &FitPlan) -> CheckReply {
    CheckReply {
        pairs: plan
            .asked()
            .zip(&measures.pairs)
            .map(|(p, m)| wire::WirePair::of((&p.occurrence, &p.body, p.copy), m))
            .collect(),
        reach: measures.reach.iter().map(wire::WireReach::of).collect(),
    }
}

#[cfg(test)]
mod tests;
