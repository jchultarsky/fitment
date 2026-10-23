//! Test-only fit verifiers (`docs/research/m7-fit.md`, FC1), under the
//! `test-support` feature, which no release build enables: a verifier that
//! answers with given measures, and the measures of an exact kernel, one
//! that reads every body exactly as fitment does and finds nothing in
//! common. Neither can make a fit `verified` by itself: the matcher still
//! plans and decides from the measures (FV1), so a measure that disagrees
//! with fitment's reading fails closed as a real helper's would.

use std::sync::Mutex;

use super::measures::{
    BodyMeasure, CheckMeasures, FitBox, FitFailure, FitMeasures, LandmarkMeasure, PairMeasure,
    PlaceMeasures, PlacementMeasure, ReachClaim, SceneMeasures, SolidMeasure, ZoneMeasure,
};
use super::plan::{FitPlan, moved};
use super::scene::{Landmark, LandmarkValue};
use super::zones::Built;
use super::{FitBody, FitIdentity, FitRequest, FitVerifier};
use crate::geom::Rigid;

/// The volume an exact kernel gives every body, in mm³.
pub const EXACT_VOLUME_MM3: f64 = 1000.0;

/// The shape tolerance it gives every body, in mm.
pub const EXACT_TOLERANCE_MM: f64 = 1e-7;

/// The gap it measures between a zoned body less its zones and the
/// candidate, in mm: well beyond any pair's threshold.
pub const EXACT_ZONED_GAP_MM: f64 = 0.3;

/// What a verifier answers: the kernel's measures of a request, or a
/// helper's failure.
type Answer = Box<dyn Fn(&FitRequest<'_>) -> Result<FitMeasures, FitFailure> + Send + Sync>;

/// One request a [`MeasuresVerifier`] was asked (VT61).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Asked {
    /// The candidate: a file's SHA-256, or `component <assembly
    /// SHA-256> <occurrence>`.
    pub candidate: String,
    /// Every placement asked, as (socket, occurrence), in order.
    pub placements: Vec<(String, Vec<u64>)>,
}

/// A verifier that answers with given measures (FC1), for tests: an
/// identity, the socket's assembly, and a function of the request. It
/// records every request it is asked.
pub struct MeasuresVerifier {
    identity: FitIdentity,
    assembly: Vec<u8>,
    answer: Answer,
    asked: Mutex<Vec<Asked>>,
}

impl std::fmt::Debug for MeasuresVerifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MeasuresVerifier")
            .field("identity", &self.identity)
            .finish_non_exhaustive()
    }
}

impl MeasuresVerifier {
    /// A verifier named `identity`, whose kernel reads `assembly`, that
    /// answers each request with `answer`'s measures.
    pub fn new(
        identity: FitIdentity,
        assembly: Vec<u8>,
        answer: impl Fn(&FitRequest<'_>) -> Result<FitMeasures, FitFailure> + Send + Sync + 'static,
    ) -> Self {
        Self {
            identity,
            assembly,
            answer: Box::new(answer),
            asked: Mutex::new(Vec::new()),
        }
    }

    /// A verifier that answers every request with [`exact`]'s measures.
    pub fn exact(identity: FitIdentity, assembly: Vec<u8>) -> Self {
        Self::new(identity, assembly, |request| Ok(exact(request)))
    }

    /// Every request asked so far, in order.
    pub fn asked(&self) -> Vec<Asked> {
        self.asked
            .lock()
            .map_or_else(|poisoned| poisoned.into_inner().clone(), |a| a.clone())
    }
}

impl FitVerifier for MeasuresVerifier {
    fn identity(&self) -> Option<FitIdentity> {
        Some(self.identity.clone())
    }

    fn assembly(&self) -> Option<&[u8]> {
        Some(&self.assembly)
    }

    fn measure(&self, request: &FitRequest<'_>) -> Result<FitMeasures, FitFailure> {
        let candidate = match request.body() {
            FitBody::Step { sha256, .. } => sha256.clone(),
            FitBody::Component {
                assembly_sha256,
                occurrence,
                ..
            } => format!("component {assembly_sha256} {occurrence:?}"),
        };
        let asked = Asked {
            candidate,
            placements: request
                .placements()
                .iter()
                .map(|p| (p.socket.clone(), p.occurrence.clone()))
                .collect(),
        };
        match self.asked.lock() {
            Ok(mut all) => all.push(asked),
            Err(poisoned) => poisoned.into_inner().push(asked),
        }
        (self.answer)(request)
    }
}

/// The measures of an exact kernel for `request`: every body found where
/// fitment places it, valid, of [`EXACT_VOLUME_MM3`] and
/// [`EXACT_TOLERANCE_MM`], its box fitment's own moved there and its
/// landmarks fitment's; a leaf fitment cannot read reported with no box
/// (so within reach of everything, and unclaimed: FB6); the candidate
/// placed likewise; and for every pair the plan asks, every zone built as
/// its closed form, nothing in common, a zoned body touching the candidate
/// and [`EXACT_ZONED_GAP_MM`] clear outside its zones, and every other
/// pair beyond its threshold. Each reply echoes its request's digest.
pub fn exact(request: &FitRequest<'_>) -> FitMeasures {
    let scene = request.scene();
    let mut read = SceneMeasures {
        request_sha256: Some(scene.request_digest()),
        ..SceneMeasures::default()
    };
    for body in scene.bodies() {
        let bounds = scene
            .part(body)
            .and_then(|p| p.extent.as_ref())
            .and_then(|e| moved(e, &body.transform))
            .unwrap_or(UNKNOWN_BOX);
        read.bodies.push(BodyMeasure {
            path: body.path.clone(),
            found: true,
            valid: true,
            volume_mm3: EXACT_VOLUME_MM3,
            tolerance_mm: EXACT_TOLERANCE_MM,
            bounds,
            landmarks: body.landmarks.iter().map(measure_of).collect(),
        });
        read.solids.push(SolidMeasure {
            faces: body.faces.clone(),
            location: location(&body.transform),
            bounds,
        });
    }
    for _ in scene.unread() {
        read.solids.push(SolidMeasure {
            faces: Vec::new(),
            location: location(&Rigid::IDENTITY),
            bounds: UNKNOWN_BOX,
        });
    }
    let candidate = request.candidate();
    let mut place = PlaceMeasures {
        request_sha256: Some(request.place_digest()),
        valid: true,
        volume_mm3: EXACT_VOLUME_MM3,
        tolerance_mm: EXACT_TOLERANCE_MM,
        sha256: candidate.sha256().map(str::to_owned),
        placements: Vec::new(),
    };
    for placed in request.placements() {
        place.placements.push(PlacementMeasure {
            occurrence: placed.occurrence.clone(),
            bounds: candidate
                .extent()
                .and_then(|e| moved(e, &placed.transform))
                .unwrap_or(UNKNOWN_BOX),
            landmarks: candidate
                .landmarks()
                .iter()
                .map(|l| {
                    l.placed(&placed.transform)
                        .map_or_else(LandmarkMeasure::default, |l| measure_of(&l))
                })
                .collect(),
        });
    }
    let plan = request.plan(&read, &place);
    FitMeasures::new(read, place, clean_check(&plan))
}

/// FT4: the measures of `plan`'s asked pair `index` made an interference
/// of `residual_mm3` outside every zone: in common, at distance 0, with a
/// box. False if the plan asks no such pair.
pub fn interfere(
    measures: &mut FitMeasures,
    plan: &FitPlan,
    index: usize,
    residual_mm3: f64,
) -> bool {
    if plan.asked().count() != measures.check.pairs.len() {
        return false;
    }
    let Some(pair) = measures.check.pairs.get_mut(index) else {
        return false;
    };
    pair.common_mm3 = residual_mm3;
    pair.residual_mm3 = residual_mm3;
    pair.inside_mm3 = 0.0;
    pair.residual_box = Some(FitBox {
        min_mm: [0.0; 3],
        max_mm: [1.0; 3],
    });
    pair.distance_mm = Some(0.0);
    true
}

/// A box no reader can give: within reach of everything (FT8).
const UNKNOWN_BOX: FitBox = FitBox {
    min_mm: [f64::NAN; 3],
    max_mm: [f64::NAN; 3],
};

/// The `check` reply of an exact kernel for `plan`.
fn clean_check(plan: &FitPlan) -> CheckMeasures {
    let mut check = CheckMeasures {
        request_sha256: Some(plan.check_digest().to_owned()),
        ..CheckMeasures::default()
    };
    let built = |b: Option<Built>| {
        b.map_or_else(ZoneMeasure::default, |b| {
            ZoneMeasure::new(b.volume_mm3, b.centroid_mm, b.bounds)
        })
    };
    for pair in plan.asked() {
        let zoned = !pair.zones.is_empty();
        check.pairs.push(PairMeasure {
            distance_mm: zoned.then_some(0.0),
            zoned_distance_mm: zoned.then_some(EXACT_ZONED_GAP_MM),
            body_kept_mm3: zoned.then_some(EXACT_VOLUME_MM3),
            body_inside_mm3: zoned.then_some(0.0),
            zones: pair.zones.iter().map(|z| built(z.interference)).collect(),
            proximity_zones: if zoned {
                pair.zones.iter().map(|z| built(z.proximity)).collect()
            } else {
                Vec::new()
            },
            ..PairMeasure::default()
        });
    }
    for placed in plan.placements() {
        for body in plan.bodies().iter().filter(|b| !b.selected) {
            let within = plan
                .pairs()
                .iter()
                .any(|p| p.occurrence == placed.occurrence && p.body == body.path);
            check.reach.push(ReachClaim::new(
                body.path.clone(),
                placed.occurrence.clone(),
                within,
            ));
        }
    }
    check
}

fn measure_of(landmark: &Landmark) -> LandmarkMeasure {
    match landmark.value {
        LandmarkValue::Plane { normal, offset_mm } => {
            LandmarkMeasure::plane(normal.to_array(), offset_mm)
        }
        LandmarkValue::Cylinder { axis, radius_mm } => LandmarkMeasure::cylinder(
            axis.point_mm.to_array(),
            axis.direction.to_array(),
            radius_mm,
        ),
    }
}

fn location(transform: &Rigid) -> [[f64; 4]; 3] {
    let (r, t) = (transform.rotation(), transform.translation());
    [0, 1, 2].map(|i| [r[i][0], r[i][1], r[i][2], t[i]])
}
