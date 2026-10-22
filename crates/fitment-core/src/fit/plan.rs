//! What a fit checks (`docs/research/m7-fit.md`, FT2, FT3, FT7, FT8): the
//! candidate at every selected occurrence, each placement paired with every
//! leaf of the assembly but the selected occurrences and with the
//! candidate's own copies at the others; each pair's own tolerances from
//! the uncertainties its two bodies state; and only the pairs whose boxes,
//! as the kernel measured them, come within the pair's reach.

use crate::features::{Extent, Part};
use crate::geom::{Point3, Rigid};
use crate::matching::{GUARD_MM, MatchPolicy};
use crate::socket::SocketDocument;

use super::measures::{FitBox, PlaceMeasures, SceneMeasures};
use super::scene::{FitScene, Landmark, SceneBody, landmarks};
use super::zones::{Built, Zone};
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
    /// the pair states.
    pub fn of(linear_mm: f64, stated: impl IntoIterator<Item = f64>) -> Self {
        let u = stated.into_iter().fold(GUARD_MM, f64::max);
        Self {
            u_pair_mm: u,
            epsilon_mm: linear_mm - u,
            threshold_mm: linear_mm + u,
        }
    }

    /// True if `ε_pair ≤ 0`: the files do not locate the two bodies within
    /// the tolerance, and the pair is `uncertainty_above_tolerance`.
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
/// stated uncertainty, its landmarks and its sound box in its own frame,
/// and a file's SHA-256 (FT7, FB3, FB4).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct FitCandidate {
    /// Its largest stated uncertainty, in mm (for the control, the
    /// original's own).
    pub uncertainty_mm: f64,
    /// Its landmarks, in its own frame.
    pub landmarks: Vec<Landmark>,
    /// Its sound box, if it has one.
    pub extent: Option<Extent>,
    /// The SHA-256 of a file candidate's bytes.
    pub sha256: Option<String>,
}

impl FitCandidate {
    /// The candidate `part`, whose body is `body`.
    pub fn of(part: &Part, body: &FitBody<'_>) -> Self {
        Self {
            uncertainty_mm: part.max_uncertainty_mm,
            landmarks: landmarks(part),
            extent: part.extent,
            sha256: match body {
                FitBody::Step { sha256, .. } => Some(sha256.clone()),
                FitBody::Component { .. } => None,
            },
        }
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

/// What [`plan`] needs besides the kernel's boxes.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct PlanInput<'a> {
    /// The scene.
    pub scene: &'a FitScene,
    /// The socket's zones, from [`super::zones`].
    pub zones: &'a [Zone],
    /// The candidate.
    pub candidate: &'a FitCandidate,
    /// Its placements, every occurrence of every socket.
    pub placements: &'a [Placed],
    /// The linear tolerance t, in mm.
    pub linear_mm: f64,
    /// The verifier the plan is for.
    pub identity: FitIdentity,
}

impl<'a> PlanInput<'a> {
    /// What [`plan`] reads.
    pub fn new(
        scene: &'a FitScene,
        zones: &'a [Zone],
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
}

/// A fit's plan (FT3, FT7, FT8): every placement, every pair within the
/// kernel's reach with its tolerances and zones, and what core expects the
/// kernel to read. Built only by [`plan`].
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
    /// states `theirs`.
    pub(crate) fn values(&self, occurrence: &[u64], theirs: &[f64]) -> PairValues {
        let ours = self.body(occurrence).map_or([0.0; 2], |b| {
            [b.part_uncertainty_mm, b.placement_uncertainty_mm]
        });
        PairValues::of(
            self.linear_mm,
            ours.into_iter()
                .chain([self.candidate.uncertainty_mm])
                .chain(theirs.iter().copied()),
        )
    }
}

/// FT3, FT7, FT8: the plan for `input`, its pairs chosen on the kernel's
/// own boxes: the scene's bodies' from `scene` and the placed candidate's
/// from `place`. A pair whose boxes, each enlarged by the pair's reach,
/// its own largest shape tolerance and 1e-6 mm, do not meet is clear; a
/// body or placement the kernel did not measure is in no pair, and
/// [`super::decide`] finds it unbound.
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
    };
    let boxes = |path: &[u64]| {
        scene
            .bodies
            .iter()
            .find(|m| m.path == path && m.found && m.bounds.sound())
            .map(|m| (m.bounds, m.tolerance_mm))
    };
    let placed = |occurrence: &[u64]| {
        place
            .placements
            .iter()
            .find(|m| m.occurrence == occurrence && m.bounds.sound())
            .map(|m| (m.bounds, place.tolerance_mm))
    };
    for k in input.placements {
        let Some(ours) = placed(&k.occurrence) else {
            continue;
        };
        let neighbours = input
            .scene
            .bodies()
            .iter()
            .filter(|b| !b.selected)
            .map(|b| {
                let stated = [b.part_uncertainty_mm, b.placement_uncertainty_mm];
                (b.path.clone(), b.label(), false, stated, boxes(&b.path))
            });
        let copies = input
            .placements
            .iter()
            .filter(|j| j.occurrence != k.occurrence)
            .map(|j| {
                let theirs = input.scene.body(&j.occurrence);
                let stated = theirs.map_or([0.0; 2], |b| {
                    [b.part_uncertainty_mm, b.placement_uncertainty_mm]
                });
                let name = theirs.map_or_else(
                    || format!("the candidate's copy at {:?}", j.occurrence),
                    |b| format!("the candidate's copy at {}", b.name),
                );
                (
                    j.occurrence.clone(),
                    name,
                    true,
                    stated,
                    placed(&j.occurrence),
                )
            });
        for (body, name, copy, stated, theirs) in neighbours.chain(copies) {
            let Some(theirs) = theirs else {
                continue;
            };
            let values = out.values(&k.occurrence, &stated);
            if !within(&values, ours, theirs) {
                out.clear += 1;
                continue;
            }
            let zones: Vec<PairZone> = input
                .zones
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
            out.pairs.push(Pair {
                occurrence: k.occurrence.clone(),
                body,
                copy,
                name,
                values,
                zones,
                state,
            });
        }
    }
    out
}

/// FT8: two boxes, each with its largest shape tolerance, within a pair's
/// reach of each other.
pub(crate) fn within(
    values: &PairValues,
    (a, a_tol): (FitBox, f64),
    (b, b_tol): (FitBox, f64),
) -> bool {
    let reach = values.reach_mm() + REACH_SLACK_MM;
    a.grown(reach + a_tol.max(0.0))
        .meets(&b.grown(reach + b_tol.max(0.0)))
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
        assert_eq!(PairValues::of(0.01, [f64::NAN]).u_pair_mm, GUARD_MM);
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
        assert!(within(&v, (a, 0.0), (at(0.02), 0.0)));
        assert!(!within(&v, (a, 0.0), (at(0.1), 0.0)));
        assert!(within(&v, (a, 0.04), (at(0.1), 0.0)));
        let reach = 2.0 * (v.reach_mm() + REACH_SLACK_MM);
        assert!(within(&v, (a, 0.0), (at(reach), 0.0)));
        assert!(!within(&v, (a, 0.0), (at(reach + 1e-9), 0.0)));
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
