//! What a fit checks (`docs/research/m7-fit.md`, FT2, FT3, FT7, FT8): the
//! candidate at every selected occurrence, each placement paired with every
//! leaf of the assembly but the selected occurrences and with the
//! candidate's own copies at the others; each pair's own tolerances from
//! the uncertainties its two bodies state; and only the pairs whose boxes
//! come within the pair's reach, each box the hull of the kernel's and of
//! fitment's own sound box where fitment places the body, so that neither
//! reader's box alone can cull a body.

use crate::features::{Extent, Part};
use crate::geom::{Point3, Rigid};
use crate::matching::{GUARD_MM, MatchPolicy};
use crate::sha256::Sha256;
use crate::socket::SocketDocument;

use super::measures::{FitBox, PlaceMeasures, SceneMeasures};
use super::scene::{FitScene, Landmark, LandmarkValue, SceneBody, landmarks};
use super::zones::{Built, Zone, Zones};
use super::{FitBody, FitIdentity};

/// Boxes are enlarged by this beyond their reach, in mm (FT8).
pub(crate) const REACH_SLACK_MM: f64 = 1e-6;

/// FT7: one pair's own tolerances. With t the policy's linear tolerance
/// and `u_pair` the largest uncertainty either body states (at least
/// `GUARD_MM`), interference zones are `ε_pair = t − u_pair` deep, a gap
/// at or under `t + u_pair` is a proximity, and proximity zones are
/// `ε_pair + t + u_pair` deep.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct PairValues {
    /// `u_pair`, in mm.
    pub u_pair_mm: f64,
    /// `ε_pair = t − u_pair`, in mm.
    pub epsilon_mm: f64,
    /// The proximity threshold `t + u_pair`, in mm.
    pub threshold_mm: f64,
}

impl PairValues {
    /// FT7's values under tolerance `linear_mm`, from every uncertainty
    /// the pair states. An uncertainty that is not finite fails closed:
    /// `u_pair` is infinite, and the pair `uncertainty_above_tolerance`.
    pub fn of(linear_mm: f64, stated: impl IntoIterator<Item = f64>) -> Self {
        let u = stated.into_iter().fold(GUARD_MM, |u, x| {
            if x.is_finite() {
                u.max(x)
            } else {
                f64::INFINITY
            }
        });
        Self {
            u_pair_mm: u,
            epsilon_mm: linear_mm - u,
            threshold_mm: linear_mm + u,
        }
    }

    /// True if `ε_pair ≤ 0`, or not a number: the files do not locate the
    /// two bodies within the tolerance, and the pair is
    /// `uncertainty_above_tolerance`.
    pub fn uncertain(&self) -> bool {
        self.epsilon_mm.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater)
    }

    /// The depth of the pair's proximity zones, `ε_pair + t + u_pair`.
    pub fn proximity_depth_mm(&self) -> f64 {
        self.epsilon_mm + self.threshold_mm
    }

    /// FT8: how far each box of the pair is enlarged before its own
    /// largest shape tolerance and the slack, `ε_pair + 2(t + u_pair)`.
    pub fn reach_mm(&self) -> f64 {
        self.epsilon_mm + 2.0 * self.threshold_mm
    }
}

/// What core knows of the candidate before the kernel reads it: its
/// stated uncertainty and vertex offset, its landmarks and its sound box
/// in its own frame, and a file's SHA-256 (FT7, FB3, FB4). Built only by
/// [`FitCandidate::of`], from the part fitment read.
#[derive(Debug, Clone, PartialEq)]
pub struct FitCandidate {
    pub(crate) uncertainty_mm: f64,
    pub(crate) vertex_offset_mm: f64,
    pub(crate) landmarks: Vec<Landmark>,
    pub(crate) extent: Option<Extent>,
    pub(crate) sha256: Option<String>,
}

impl FitCandidate {
    /// The candidate `part`, whose body is `body`.
    pub fn of(part: &Part, body: &FitBody<'_>) -> Self {
        Self {
            uncertainty_mm: part.max_uncertainty_mm,
            vertex_offset_mm: part
                .solids
                .iter()
                .map(|s| s.max_vertex_offset_mm)
                .fold(0.0, f64::max),
            landmarks: landmarks(part),
            extent: part.extent,
            sha256: match body {
                FitBody::Step { sha256, .. } => Some(sha256.clone()),
                FitBody::Component { .. } => None,
            },
        }
    }

    /// Its largest stated uncertainty, in mm (for the control, the
    /// original's own).
    pub fn uncertainty_mm(&self) -> f64 {
        self.uncertainty_mm
    }

    /// Its solid's largest vertex offset, in mm.
    pub fn vertex_offset_mm(&self) -> f64 {
        self.vertex_offset_mm
    }

    /// Its landmarks, in its own frame (FB3).
    pub fn landmarks(&self) -> &[Landmark] {
        &self.landmarks
    }

    /// Its sound box, in its own frame, if it has one.
    pub fn extent(&self) -> Option<&Extent> {
        self.extent.as_ref()
    }

    /// The SHA-256 of a file candidate's bytes (FB4).
    pub fn sha256(&self) -> Option<&str> {
        self.sha256.as_deref()
    }
}

/// The candidate placed at one occurrence (FT2): `T_k · A`, from its frame
/// to the root's.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Placed {
    /// The socket the occurrence is in.
    pub socket: String,
    /// The occurrence, by usage `#id`s.
    pub occurrence: Vec<u64>,
    /// `T_k · A`.
    pub transform: Rigid,
}

impl Placed {
    /// The candidate at `occurrence` of `socket`, placed by `transform`.
    pub fn new(socket: impl Into<String>, occurrence: Vec<u64>, transform: Rigid) -> Self {
        Self {
            socket: socket.into(),
            occurrence,
            transform,
        }
    }

    /// FT2: the candidate at every occurrence of `socket` of `document`,
    /// placed by `T_k · at`, `at` the placement its interface passed at;
    /// `None` if the socket is not the document's or a placement
    /// overflows.
    pub fn every(document: &SocketDocument, socket: &str, at: &Rigid) -> Option<Vec<Self>> {
        let extracted = document.extracted()?;
        let found = extracted.sockets.iter().find(|s| s.id == socket)?;
        found
            .occurrences
            .iter()
            .map(|path| {
                let occurrence = extracted.occurrences.iter().find(|o| &o.path == path)?;
                Some(Self::new(
                    socket,
                    path.clone(),
                    at.then(&occurrence.transform)?,
                ))
            })
            .collect()
    }
}

/// Whether a pair is asked of the kernel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PairState {
    /// Within reach, with every zone built: the `check` request holds it.
    Asked,
    /// `ε_pair ≤ 0`: not asked, `uncertainty_above_tolerance` (FT7).
    Uncertain,
    /// A zone of it has no closed form: not asked, `zone_unbuilt` (FZ7).
    ZoneUnbuilt,
}

/// One zone of a pair, with its closed forms at the pair's two depths.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct PairZone {
    /// The zone.
    pub zone: Zone,
    /// At `ε_pair`, for the interference test (FT4).
    pub interference: Option<Built>,
    /// At `ε_pair + t + u_pair`, for the proximity test (FT5).
    pub proximity: Option<Built>,
}

/// One pair within reach (FT3, FT8): the candidate at `occurrence` and a
/// neighbour, or the candidate's copy at another selected occurrence.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Pair {
    /// The occurrence the candidate is placed at.
    pub occurrence: Vec<u64>,
    /// The other body's path: a neighbour's, or the copy's occurrence.
    pub body: Vec<u64>,
    /// True if the other body is the candidate's copy.
    pub copy: bool,
    /// The other body's name, for people.
    pub name: String,
    /// The pair's tolerances.
    pub values: PairValues,
    /// The zones of `(occurrence, body)`.
    pub zones: Vec<PairZone>,
    /// Whether it is asked.
    pub state: PairState,
}

/// What [`plan`] needs besides the kernel's boxes; built only by
/// [`PlanInput::new`], from the scene, its zones and the candidate core
/// read, so a plan holds fitment's own tolerances and zones.
#[derive(Debug, Clone)]
pub struct PlanInput<'a> {
    pub(crate) scene: &'a FitScene,
    pub(crate) zones: &'a Zones,
    pub(crate) candidate: &'a FitCandidate,
    pub(crate) placements: &'a [Placed],
    pub(crate) linear_mm: f64,
    pub(crate) identity: FitIdentity,
}

impl<'a> PlanInput<'a> {
    /// What [`plan`] reads: the scene, its zones (from [`super::zones()`]),
    /// the candidate, its placements at every occurrence of every socket,
    /// the policy's tolerance and the verifier.
    pub fn new(
        scene: &'a FitScene,
        zones: &'a Zones,
        candidate: &'a FitCandidate,
        placements: &'a [Placed],
        policy: &MatchPolicy,
        identity: FitIdentity,
    ) -> Self {
        Self {
            scene,
            zones,
            candidate,
            placements,
            linear_mm: policy.tolerance().linear_mm(),
            identity,
        }
    }

    /// The SHA-256 of what the `place` request asks: the candidate's own
    /// facts and every placement. The helper echoes it in its reply, and
    /// [`super::decide`] requires it (FH2, beside the protocol's nonce).
    pub fn place_digest(&self) -> String {
        place_digest(self.candidate, self.placements)
    }
}

/// The SHA-256s of the three requests a fit sends, each echoed by the
/// helper's reply.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Digests {
    scene: String,
    place: String,
    check: String,
}

/// A fit's plan (FT3, FT7, FT8): every placement, every pair within reach
/// with its tolerances and zones, and what core expects the kernel to
/// read. Built only by [`plan`].
#[derive(Debug, Clone, PartialEq)]
pub struct FitPlan {
    identity: FitIdentity,
    linear_mm: f64,
    candidate: FitCandidate,
    placements: Vec<Placed>,
    bodies: Vec<SceneBody>,
    unread: usize,
    pairs: Vec<Pair>,
    clear: usize,
    digests: Digests,
}

impl FitPlan {
    /// The verifier the plan is for.
    pub fn identity(&self) -> &FitIdentity {
        &self.identity
    }

    /// The linear tolerance t, in mm.
    pub fn linear_mm(&self) -> f64 {
        self.linear_mm
    }

    /// The candidate.
    pub fn candidate(&self) -> &FitCandidate {
        &self.candidate
    }

    /// Its placements, in the order given.
    pub fn placements(&self) -> &[Placed] {
        &self.placements
    }

    /// Every body of the scene, as the `scene` request names them.
    pub fn bodies(&self) -> &[SceneBody] {
        &self.bodies
    }

    /// How many leaves of the assembly fitment does not read.
    pub fn unread(&self) -> usize {
        self.unread
    }

    /// Every pair within reach, asked or not, in placement order.
    pub fn pairs(&self) -> &[Pair] {
        &self.pairs
    }

    /// The pairs the `check` request holds, in its order.
    pub fn asked(&self) -> impl Iterator<Item = &Pair> {
        self.pairs.iter().filter(|p| p.state == PairState::Asked)
    }

    /// How many pairs were outside reach: clear, counted, not listed.
    pub fn clear(&self) -> usize {
        self.clear
    }

    /// The SHA-256 of the `scene` request, which its reply echoes.
    pub fn scene_digest(&self) -> &str {
        &self.digests.scene
    }

    /// The SHA-256 of the `place` request, which its reply echoes.
    pub fn place_digest(&self) -> &str {
        &self.digests.place
    }

    /// The SHA-256 of the `check` request, which its reply echoes: the
    /// asked pairs, their tolerances and their zones' closed forms.
    pub fn check_digest(&self) -> &str {
        &self.digests.check
    }

    /// The sockets placed, each once, in placement order.
    pub fn sockets(&self) -> Vec<&str> {
        let mut out: Vec<&str> = Vec::new();
        for placed in &self.placements {
            if !out.contains(&placed.socket.as_str()) {
                out.push(&placed.socket);
            }
        }
        out
    }

    /// The socket of the placement at `occurrence`.
    pub(crate) fn socket_of(&self, occurrence: &[u64]) -> Option<&str> {
        self.placements
            .iter()
            .find(|p| p.occurrence == occurrence)
            .map(|p| p.socket.as_str())
    }

    /// The scene body at `path`.
    pub(crate) fn body(&self, path: &[u64]) -> Option<&SceneBody> {
        self.bodies.iter().find(|b| b.path == path)
    }

    /// FT7's values for the candidate at `occurrence` beside a body that
    /// states `theirs`: the occurrence's and the candidate's own floors
    /// with them.
    pub(crate) fn values(&self, occurrence: &[u64], theirs: &[f64]) -> PairValues {
        let ours = self.body(occurrence).map_or([0.0; 3], stated);
        PairValues::of(
            self.linear_mm,
            ours.into_iter()
                .chain([
                    self.candidate.uncertainty_mm,
                    self.candidate.vertex_offset_mm,
                ])
                .chain(theirs.iter().copied()),
        )
    }

    /// FT8: the box the candidate at `occurrence` is judged within reach
    /// by: the hull of the kernel's placed box and fitment's own moved
    /// there; `None`, within reach of everything, if either is unknown.
    pub(crate) fn candidate_box(&self, place: &PlaceMeasures, occurrence: &[u64]) -> Reach {
        let ours = self.placements.iter().find(|p| p.occurrence == occurrence);
        let kernel = place
            .placements
            .iter()
            .find(|m| m.occurrence == occurrence)
            .map(|m| (m.bounds, place.tolerance_mm));
        let moved = ours.and_then(|p| moved(self.candidate.extent.as_ref()?, &p.transform));
        hull(kernel, moved)
    }
}

/// FT8: the box a scene body is judged within reach by, as the
/// candidate's: the hull of the kernel's and fitment's own.
pub(crate) fn body_box(scene: &SceneMeasures, body: &SceneBody) -> Reach {
    let kernel = scene
        .bodies
        .iter()
        .find(|m| m.path == body.path && m.found)
        .map(|m| (m.bounds, m.tolerance_mm));
    hull(
        kernel,
        body.extent.as_ref().and_then(|e| moved(e, &body.transform)),
    )
}

/// A body's three stated floors (FT7): its part's uncertainty, its
/// vertex offset (as M4's `u_T` counts it) and its placement's.
fn stated(body: &SceneBody) -> [f64; 3] {
    [
        body.part_uncertainty_mm,
        body.vertex_offset_mm,
        body.placement_uncertainty_mm,
    ]
}

/// A box and its largest shape tolerance, or `None` for a body whose box
/// is not known: it is within reach of everything.
pub(crate) type Reach = Option<(FitBox, f64)>;

/// FT8: the hull of the kernel's box and fitment's own; `None` if either
/// is missing or unsound, or the kernel's tolerance is not a finite,
/// non-negative number. fitment's box holds every point of the body, so a
/// kernel box too small never culls.
fn hull(kernel: Option<(FitBox, f64)>, ours: Option<FitBox>) -> Reach {
    let (kernel, tolerance) = kernel?;
    let ours = ours?;
    if !(kernel.sound() && ours.sound() && tolerance.is_finite() && tolerance >= 0.0) {
        return None;
    }
    Some((
        FitBox {
            min_mm: [0, 1, 2].map(|i| kernel.min_mm[i].min(ours.min_mm[i])),
            max_mm: [0, 1, 2].map(|i| kernel.max_mm[i].max(ours.max_mm[i])),
        },
        tolerance,
    ))
}

/// FT3, FT7, FT8: the plan for `input`, its pairs chosen on the hull of
/// the kernel's boxes (the scene's bodies' from `scene`, the placed
/// candidate's from `place`) and fitment's own. A pair whose boxes, each
/// enlarged by the pair's reach, its own largest shape tolerance and
/// 1e-6 mm, do not meet is clear; a body whose box either reader cannot
/// give is within reach of everything, never culled.
pub fn plan(input: &PlanInput<'_>, scene: &SceneMeasures, place: &PlaceMeasures) -> FitPlan {
    let mut out = FitPlan {
        identity: input.identity.clone(),
        linear_mm: input.linear_mm,
        candidate: input.candidate.clone(),
        placements: input.placements.to_vec(),
        bodies: input.scene.bodies().to_vec(),
        unread: input.scene.unread().len(),
        pairs: Vec::new(),
        clear: 0,
        digests: Digests {
            scene: input.scene.request_digest(),
            place: input.place_digest(),
            check: String::new(),
        },
    };
    for k in input.placements {
        let ours = out.candidate_box(place, &k.occurrence);
        let neighbours = input
            .scene
            .bodies()
            .iter()
            .filter(|b| !b.selected)
            .map(|b| {
                let theirs = body_box(scene, b);
                (b.path.clone(), b.label(), false, stated(b), theirs)
            });
        let copies = input
            .placements
            .iter()
            .filter(|j| j.occurrence != k.occurrence)
            .map(|j| {
                let theirs = input.scene.body(&j.occurrence);
                let floors = theirs.map_or([0.0; 3], stated);
                let name = theirs.map_or_else(
                    || format!("the candidate's copy at {:?}", j.occurrence),
                    |b| format!("the candidate's copy at {}", b.name),
                );
                let boxed = out.candidate_box(place, &j.occurrence);
                (j.occurrence.clone(), name, true, floors, boxed)
            });
        let candidates: Vec<_> = neighbours.chain(copies).collect();
        for (body, name, copy, floors, theirs) in candidates {
            let values = out.values(&k.occurrence, &floors);
            if !within(&values, ours, theirs) {
                out.clear += 1;
                continue;
            }
            out.pairs.push(pair(input, k, body, name, copy, values));
        }
    }
    out.digests.check = check_digest(&out);
    out
}

/// One pair within reach, with its zones' closed forms and its state.
fn pair(
    input: &PlanInput<'_>,
    k: &Placed,
    body: Vec<u64>,
    name: String,
    copy: bool,
    values: PairValues,
) -> Pair {
    let zones: Vec<PairZone> = input
        .zones
        .as_slice()
        .iter()
        .filter(|z| z.occurrence == k.occurrence && z.body == body)
        .map(|z| PairZone {
            zone: z.clone(),
            interference: z.at(values.epsilon_mm),
            proximity: z.at(values.proximity_depth_mm()),
        })
        .collect();
    let state = if values.uncertain() {
        PairState::Uncertain
    } else if zones
        .iter()
        .any(|z| z.interference.is_none() || z.proximity.is_none())
    {
        PairState::ZoneUnbuilt
    } else {
        PairState::Asked
    };
    Pair {
        occurrence: k.occurrence.clone(),
        body,
        copy,
        name,
        values,
        zones,
        state,
    }
}

/// FT8: two boxes, each with its largest shape tolerance, within a pair's
/// reach of each other; true for a box not known, or a reach that is not
/// a finite number.
pub(crate) fn within(values: &PairValues, a: Reach, b: Reach) -> bool {
    let reach = values.reach_mm() + REACH_SLACK_MM;
    let (Some((a, a_tol)), Some((b, b_tol))) = (a, b) else {
        return true;
    };
    if !reach.is_finite() {
        return true;
    }
    a.grown(reach + a_tol).meets(&b.grown(reach + b_tol))
}

/// The box, in the root frame, of `extent` moved by `by`: every corner
/// moved; `None` if one overflows.
pub(crate) fn moved(extent: &Extent, by: &Rigid) -> Option<FitBox> {
    let (lo, hi) = (extent.min_mm.to_array(), extent.max_mm.to_array());
    let mut out = FitBox {
        min_mm: [f64::INFINITY; 3],
        max_mm: [f64::NEG_INFINITY; 3],
    };
    for corner in 0..8 {
        let pick = |i: usize| if corner >> i & 1 == 0 { lo[i] } else { hi[i] };
        let p = by
            .apply_point(Point3::new([pick(0), pick(1), pick(2)])?)?
            .to_array();
        for (i, x) in p.iter().enumerate() {
            out.min_mm[i] = out.min_mm[i].min(*x);
            out.max_mm[i] = out.max_mm[i].max(*x);
        }
    }
    Some(out)
}

/// A request's content, hashed in a fixed encoding: each number as its
/// bits, each list with its length.
pub(crate) struct Digest(Sha256);

impl Digest {
    pub(crate) fn new(kind: &str) -> Self {
        let mut d = Self(Sha256::new());
        d.text(kind);
        d
    }

    pub(crate) fn number(&mut self, x: f64) {
        self.0.update(&x.to_bits().to_be_bytes());
    }

    pub(crate) fn count(&mut self, n: u64) {
        self.0.update(&n.to_be_bytes());
    }

    pub(crate) fn text(&mut self, text: &str) {
        self.count(text.len() as u64);
        self.0.update(text.as_bytes());
    }

    pub(crate) fn ids(&mut self, ids: &[u64]) {
        self.count(ids.len() as u64);
        for id in ids {
            self.count(*id);
        }
    }

    pub(crate) fn numbers(&mut self, numbers: impl IntoIterator<Item = f64>) {
        for x in numbers {
            self.number(x);
        }
    }

    pub(crate) fn rigid(&mut self, rigid: &Rigid) {
        for row in rigid.rotation() {
            self.numbers(row);
        }
        self.numbers(rigid.translation());
    }

    pub(crate) fn landmarks(&mut self, landmarks: &[Landmark]) {
        self.count(landmarks.len() as u64);
        for landmark in landmarks {
            self.count(landmark.face);
            match landmark.value {
                LandmarkValue::Plane { normal, offset_mm } => {
                    self.numbers(normal.to_array());
                    self.number(offset_mm);
                }
                LandmarkValue::Cylinder { axis, radius_mm } => {
                    self.numbers(axis.point_mm.to_array());
                    self.numbers(axis.direction.to_array());
                    self.number(radius_mm);
                }
            }
        }
    }

    fn built(&mut self, built: Option<Built>) {
        let Some(b) = built else {
            self.count(0);
            return;
        };
        self.count(1);
        for x in [b.depth_mm, b.volume_mm3]
            .iter()
            .chain(&b.centroid_mm)
            .chain(&b.bounds.min_mm)
            .chain(&b.bounds.max_mm)
        {
            self.number(*x);
        }
    }

    pub(crate) fn finish(self) -> String {
        self.0.finish_hex()
    }
}

/// The SHA-256 of a `place` request: the candidate's SHA-256, landmarks
/// and box, and every placement.
fn place_digest(candidate: &FitCandidate, placements: &[Placed]) -> String {
    let mut d = Digest::new("place");
    d.text(candidate.sha256.as_deref().unwrap_or(""));
    d.landmarks(&candidate.landmarks);
    d.count(placements.len() as u64);
    for placed in placements {
        d.text(&placed.socket);
        d.ids(&placed.occurrence);
        d.rigid(&placed.transform);
    }
    d.finish()
}

/// The SHA-256 of a `check` request: the tolerance and every asked pair
/// with its values and its zones' closed forms.
fn check_digest(plan: &FitPlan) -> String {
    let mut d = Digest::new("check");
    d.number(plan.linear_mm);
    d.text(&plan.digests.place);
    let asked: Vec<&Pair> = plan.asked().collect();
    d.count(asked.len() as u64);
    for pair in asked {
        d.ids(&pair.occurrence);
        d.ids(&pair.body);
        d.count(u64::from(pair.copy));
        for x in [
            pair.values.u_pair_mm,
            pair.values.epsilon_mm,
            pair.values.threshold_mm,
        ] {
            d.number(x);
        }
        d.count(pair.zones.len() as u64);
        for zone in &pair.zones {
            d.text(&zone.zone.requirement);
            d.built(zone.interference);
            d.built(zone.proximity);
        }
    }
    d.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// FT7: `u_pair` is the largest stated uncertainty, at least the
    /// guard; ε and the threshold straddle t by it; ε ≤ 0 is uncertain.
    #[test]
    fn a_pairs_values_straddle_the_tolerance() {
        let v = PairValues::of(0.01, [1e-7, 1e-7]);
        assert_eq!(v.u_pair_mm, GUARD_MM);
        assert!((v.epsilon_mm - 0.009_999).abs() < 1e-15);
        assert!((v.threshold_mm - 0.010_001).abs() < 1e-15);
        assert!((v.proximity_depth_mm() - 0.02).abs() < 1e-15);
        assert!(!v.uncertain());
        let md = PairValues::of(0.01, [2.1e-6, 0.01]);
        assert_eq!(md.u_pair_mm, 0.01);
        assert!(md.uncertain(), "u = t: ε = 0");
        let weldment = PairValues::of(0.01, [0.058]);
        assert!(weldment.uncertain());
        let p7 = PairValues::of(0.01, [0.0099]);
        assert!(!p7.uncertain());
        assert!((p7.epsilon_mm - 0.0001).abs() < 1e-15);
        // A stated uncertainty that is not a number fails closed.
        for odd in [f64::NAN, f64::INFINITY] {
            let v = PairValues::of(0.01, [1e-7, odd, 1e-7]);
            assert!(v.uncertain(), "{odd}");
            assert_eq!(v.u_pair_mm, f64::INFINITY);
        }
    }

    /// FT8: two boxes 0.02 mm apart meet within a reach of
    /// ε + 2(t + u) each; 0.1 mm apart they do not; a shape tolerance adds
    /// to its own box.
    #[test]
    fn reach_is_the_pairs_enlargement() {
        let v = PairValues::of(0.01, []);
        let a = FitBox {
            min_mm: [0.0; 3],
            max_mm: [1.0; 3],
        };
        let at = |gap: f64| FitBox {
            min_mm: [1.0 + gap, 0.0, 0.0],
            max_mm: [2.0 + gap, 1.0, 1.0],
        };
        let some = |b: FitBox, t: f64| Some((b, t));
        assert!(within(&v, some(a, 0.0), some(at(0.02), 0.0)));
        assert!(!within(&v, some(a, 0.0), some(at(0.1), 0.0)));
        assert!(within(&v, some(a, 0.04), some(at(0.1), 0.0)));
        let reach = 2.0 * (v.reach_mm() + REACH_SLACK_MM);
        assert!(within(&v, some(a, 0.0), some(at(reach), 0.0)));
        assert!(!within(&v, some(a, 0.0), some(at(reach + 1e-9), 0.0)));
        // A box not known is within reach of everything, as is any box
        // under a reach that is not a number.
        assert!(within(&v, None, some(at(100.0), 0.0)));
        let uncertain = PairValues::of(0.01, [f64::NAN]);
        assert!(within(&uncertain, some(a, 0.0), some(at(100.0), 0.0)));
    }

    /// FT8: the hull of the kernel's box and fitment's own: a kernel box
    /// too small is grown to fitment's; one unsound (OCCT's empty box, a
    /// NaN), or a tolerance that is not a finite non-negative number, leaves
    /// no box at all.
    #[test]
    fn reach_is_judged_on_the_hull_of_both_boxes() {
        let ours = FitBox {
            min_mm: [0.0; 3],
            max_mm: [10.0; 3],
        };
        let small = FitBox {
            min_mm: [4.0; 3],
            max_mm: [5.0; 3],
        };
        assert_eq!(hull(Some((small, 1e-7)), Some(ours)), Some((ours, 1e-7)));
        let empty = FitBox {
            min_mm: [1e100; 3],
            max_mm: [-1e100; 3],
        };
        let nan = FitBox {
            min_mm: [f64::NAN; 3],
            max_mm: [1.0; 3],
        };
        for kernel in [empty, nan] {
            assert_eq!(hull(Some((kernel, 1e-7)), Some(ours)), None);
        }
        for tolerance in [f64::NAN, -1.0, f64::INFINITY] {
            assert_eq!(hull(Some((small, tolerance)), Some(ours)), None);
        }
        assert_eq!(hull(None, Some(ours)), None);
        assert_eq!(hull(Some((small, 0.0)), None), None);
    }

    #[test]
    fn a_moved_extent_is_its_corners_box() {
        let extent = Extent {
            min_mm: Point3::new([0.0, 0.0, 0.0]).unwrap(),
            max_mm: Point3::new([2.0, 1.0, 1.0]).unwrap(),
        };
        let quarter = Rigid::new(
            [[0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]],
            [10.0, 0.0, 0.0],
        )
        .unwrap();
        let b = moved(&extent, &quarter).unwrap();
        assert_eq!(b.min_mm, [9.0, 0.0, 0.0]);
        assert_eq!(b.max_mm, [10.0, 2.0, 1.0]);
    }
}
