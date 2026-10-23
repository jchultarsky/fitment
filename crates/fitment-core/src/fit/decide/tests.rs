//! VT53 (`docs/research/m7-fit.md`, section 6): every FV rule from
//! constructed measures. The plan is s01's bracket socket with the
//! bracket itself as the candidate, at the identity; a kernel that reads
//! the assembly exactly as fitment does gives the clean measures, and each
//! test changes one of them.

use super::*;
use crate::features::Part;
use crate::fit::{BodyMeasure, CheckMeasures, Zones};
use crate::fit::{
    FitBody, FitCandidate, FitIdentity, FitScene, PairMeasure, PlaceMeasures, Placed,
    PlacementMeasure, PlanInput, ReachClaim, SceneMeasures, SolidMeasure, ZoneMeasure, plan, zones,
};
use crate::matching::MatchPolicy;
use crate::socket::SocketDocument;

const S01: &[u8] = include_bytes!("../../../tests/assemblies/s01_bolted.stp");
const VOLUME: f64 = 1000.0;

/// A socket of a committed assembly, its original as the candidate, and
/// everything a plan of it needs.
struct Rig {
    scene: FitScene,
    zones: Zones,
    part: Part,
    candidate: FitCandidate,
    placements: Vec<Placed>,
    policy: MatchPolicy,
}

impl Rig {
    /// s01's bracket socket.
    fn s01() -> Self {
        Self::of(S01, "BRACKET")
    }

    fn of(bytes: &[u8], target: &str) -> Self {
        let document = SocketDocument::extract(bytes, Some(target), None).unwrap();
        let scene = FitScene::of(bytes, &document).unwrap();
        let zones = zones(&document, &scene);
        let original = scene.bodies().iter().find(|b| b.selected).unwrap();
        let part = scene.part(original).unwrap().clone();
        let body = FitBody::component(
            scene.sha256(),
            original.path.clone(),
            original.faces.clone(),
        );
        let candidate = FitCandidate::of(&part, &body);
        let placements = Placed::every(&document, "socket-1", &Rigid::IDENTITY).unwrap();
        Self {
            scene,
            zones,
            part,
            candidate,
            placements,
            policy: MatchPolicy::default(),
        }
    }

    fn identity() -> FitIdentity {
        FitIdentity::new("open_cascade", "8.0.1")
    }

    fn input(&self) -> PlanInput<'_> {
        PlanInput::new(
            &self.scene,
            &self.zones,
            &self.candidate,
            &self.placements,
            &self.policy,
            Self::identity(),
        )
    }

    fn plan(&self, measures: &FitMeasures) -> FitPlan {
        plan(&self.input(), &measures.scene, &measures.place)
    }

    /// The kernel's reading of the scene and the placed candidate, as
    /// fitment's own.
    fn read(&self) -> FitMeasures {
        let mut scene = SceneMeasures {
            request_sha256: Some(self.scene.request_digest()),
            ..SceneMeasures::default()
        };
        for body in self.scene.bodies() {
            let part = self.scene.part(body).unwrap();
            let bounds = moved(&part.extent.unwrap(), &body.transform).unwrap();
            scene.bodies.push(BodyMeasure {
                path: body.path.clone(),
                found: true,
                valid: true,
                volume_mm3: VOLUME,
                tolerance_mm: 1e-7,
                bounds,
                landmarks: body.landmarks.iter().map(measure_of).collect(),
            });
            scene.solids.push(SolidMeasure {
                faces: body.faces.clone(),
                location: location(&body.transform),
                bounds,
                non_solid: false,
            });
        }
        let mut place = PlaceMeasures {
            request_sha256: Some(self.input().place_digest()),
            valid: true,
            volume_mm3: VOLUME,
            tolerance_mm: 1e-7,
            sha256: None,
            placements: Vec::new(),
        };
        for placed in &self.placements {
            place.placements.push(PlacementMeasure {
                occurrence: placed.occurrence.clone(),
                bounds: moved(&self.part.extent.unwrap(), &placed.transform).unwrap(),
                landmarks: self
                    .candidate
                    .landmarks
                    .iter()
                    .map(|l| measure_of(&l.placed(&placed.transform).unwrap()))
                    .collect(),
            });
        }
        FitMeasures {
            scene,
            place,
            check: CheckMeasures::default(),
        }
    }

    /// Clean measures and their plan: every zone built as its closed form,
    /// every zoned body touching the candidate and 0.3 mm clear outside
    /// its zones, every other pair beyond its threshold, nothing common.
    fn clean(&self) -> (FitPlan, FitMeasures) {
        let mut measures = self.read();
        let plan = self.plan(&measures);
        measures.check = clean_check(&plan);
        (plan, measures)
    }
}

fn clean_check(plan: &FitPlan) -> CheckMeasures {
    let mut check = CheckMeasures {
        request_sha256: Some(plan.check_digest().to_owned()),
        ..CheckMeasures::default()
    };
    for pair in plan.asked() {
        let zoned = !pair.zones.is_empty();
        let built = |b: Option<Built>| {
            let b = b.unwrap();
            ZoneMeasure::new(b.volume_mm3, b.centroid_mm, b.bounds)
        };
        check.pairs.push(PairMeasure {
            distance_mm: zoned.then_some(0.0),
            zoned_distance_mm: zoned.then_some(0.3),
            body_kept_mm3: zoned.then_some(VOLUME),
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

fn codes(outcome: &FitOutcome) -> Vec<FitCode> {
    let mut codes: Vec<FitCode> = outcome.findings().iter().map(|f| f.code).collect();
    codes.dedup();
    codes
}

fn verified(status: &FitStatus) -> bool {
    matches!(status, FitStatus::Verified { kernel, version }
        if kernel == "open_cascade" && version == "8.0.1")
}

/// The index, among the asked pairs, of the plate's.
fn plate(plan: &FitPlan) -> usize {
    plan.asked()
        .position(|p| p.name.starts_with("PLATE"))
        .unwrap()
}

/// The plan of s01's bracket: the plate and the four bolts within reach,
/// each zoned by a slab of the face it seats on; the nuts clear; every
/// pair at `u_pair` = the guard.
#[test]
fn the_bracket_s_plan_pairs_its_seated_neighbours() {
    let rig = Rig::s01();
    let (plan, _) = rig.clean();
    let asked: Vec<&Pair> = plan.asked().collect();
    assert_eq!(
        asked.len(),
        5,
        "{:?}",
        asked.iter().map(|p| &p.name).collect::<Vec<_>>()
    );
    assert_eq!(plan.clear(), 4, "the nuts");
    for pair in &asked {
        assert!(!pair.copy);
        assert_eq!(pair.values.u_pair_mm, crate::matching::GUARD_MM);
        assert_eq!(pair.zones.len(), 1, "{}", pair.name);
        assert_eq!(pair.zones[0].zone.kind, super::super::ZoneKind::Slab);
    }
    assert_eq!(plan.sockets(), ["socket-1"]);
    assert_eq!(asked[plate(&plan)].name, "PLATE (1)");
}

/// FV7: clean measures verify, with the plan's identity, per socket and
/// for the candidate.
#[test]
fn clean_measures_verify() {
    let rig = Rig::s01();
    let (plan, measures) = rig.clean();
    let outcome = decide(&plan, &measures);
    assert_eq!(outcome.findings(), [], "{:?}", outcome.findings());
    assert!(verified(outcome.candidate()));
    assert!(verified(outcome.socket("socket-1").unwrap()));
    assert!(!outcome.incomplete());
    assert_eq!(outcome.code(), None);
}

/// FV2: any alert, and a helper's exception, is `kernel_alert` with its
/// class name; never `verified`, never `interferes`.
#[test]
fn an_alert_is_a_kernel_alert() {
    let rig = Rig::s01();
    for alert in ["BOPAlgo_AlertFaceBuilderUnusedEdges", "helper_exception"] {
        let (plan, mut measures) = rig.clean();
        let at = plate(&plan);
        measures.check.pairs[at].alerts = vec![alert.to_owned()];
        measures.check.pairs[at].common_mm3 = 5.0;
        measures.check.pairs[at].residual_mm3 = 5.0;
        let outcome = decide(&plan, &measures);
        assert_eq!(codes(&outcome), [FitCode::KernelAlert]);
        assert!(outcome.findings()[0].detail.contains(alert));
        assert!(matches!(outcome.candidate(), FitStatus::NotVerified { .. }));
    }
}

/// FV3: a residual of 0 is clear, 1e-7 mm³ `below_resolution`, 1e-3 mm³
/// `interferes` with the pair, R, V and R's box.
#[test]
fn a_residual_is_clear_below_resolution_or_an_interference() {
    let rig = Rig::s01();
    let run = |residual: f64| {
        let (plan, mut measures) = rig.clean();
        let at = plate(&plan);
        let pair = &mut measures.check.pairs[at];
        pair.common_mm3 = 0.6 + residual;
        pair.inside_mm3 = 0.6;
        pair.residual_mm3 = residual;
        pair.residual_box = (residual > 0.0).then_some(FitBox::default());
        decide(&plan, &measures)
    };
    assert!(verified(run(0.0).candidate()));
    let below = run(1e-7);
    assert_eq!(codes(&below), [FitCode::BelowResolution]);
    let hit = run(1e-3);
    assert_eq!(codes(&hit), []);
    let FitStatus::Interferes { interferences } = hit.candidate() else {
        unreachable!("{:?}", hit.candidate());
    };
    assert_eq!(interferences.len(), 1);
    assert!((interferences[0].volume_mm3 - 1e-3).abs() < 1e-18);
    assert_eq!(interferences[0].neighbor, "PLATE (1)");
    assert!(matches!(
        hit.socket("socket-1"),
        Some(FitStatus::Interferes { .. })
    ));
    assert_eq!(hit.interferences()[0].common_mm3, 0.6 + 1e-3);
    assert!(hit.interferences()[0].residual_box.is_some());
}

/// FV4: a gap under the threshold outside every zone is `proximity`, at
/// it too (the edge rejects); above it, beyond, the pair is clear.
#[test]
fn a_gap_at_or_under_the_threshold_is_a_proximity() {
    let rig = Rig::s01();
    let run = |zoned: f64| {
        let (plan, mut measures) = rig.clean();
        let at = plate(&plan);
        measures.check.pairs[at].zoned_distance_mm = Some(zoned);
        decide(&plan, &measures)
    };
    let threshold = 0.01 + crate::matching::GUARD_MM;
    assert_eq!(codes(&run(threshold - 1e-9)), [FitCode::Proximity]);
    assert_eq!(codes(&run(threshold)), [FitCode::Proximity]);
    assert!(verified(run(threshold + 1e-9).candidate()));
    // The plate with no zone: its own distance counts, at the threshold,
    // and beyond it (no distance) it is clear.
    let mut bare = Rig::s01();
    let plate_path = bare
        .scene
        .bodies()
        .iter()
        .find(|b| b.label().starts_with("PLATE"))
        .unwrap()
        .path
        .clone();
    bare.zones.0.retain(|z| z.body != plate_path);
    let (plan, mut measures) = bare.clean();
    let at = plate(&plan);
    assert_eq!(plan.asked().nth(at).unwrap().zones.len(), 0);
    measures.check.pairs[at].distance_mm = Some(threshold);
    assert_eq!(codes(&decide(&plan, &measures)), [FitCode::Proximity]);
    measures.check.pairs[at].distance_mm = None;
    assert!(verified(decide(&plan, &measures).candidate()));
}

/// FV4: a pair with a face within reach that is not analytic is
/// `unsupported_geometry`.
#[test]
fn a_non_analytic_face_is_unsupported() {
    let rig = Rig::s01();
    let (plan, mut measures) = rig.clean();
    measures.check.pairs[0].non_analytic = true;
    assert_eq!(
        codes(&decide(&plan, &measures)),
        [FitCode::UnsupportedGeometry]
    );
}

/// FT7: a candidate stating the tolerance makes every pair's ε 0: each
/// is `uncertainty_above_tolerance`, and none is asked.
#[test]
fn no_tolerance_above_the_uncertainty_is_uncertain() {
    let mut rig = Rig::s01();
    rig.candidate.uncertainty_mm = 0.01;
    let measures = rig.read();
    let plan = rig.plan(&measures);
    assert_eq!(plan.asked().count(), 0);
    assert!(plan.pairs().iter().all(|p| p.state == PairState::Uncertain));
    let outcome = decide(
        &plan,
        &FitMeasures {
            check: clean_check(&plan),
            ..measures
        },
    );
    assert_eq!(codes(&outcome), [FitCode::UncertaintyAboveTolerance]);
    assert_eq!(outcome.findings().len(), 5);
}

/// FV5: a control that is not verified makes a verified candidate
/// `original_fails_fit`, with the control's code; an `interferes` stays.
#[test]
fn a_failing_control_withholds_verified_only() {
    let rig = Rig::s01();
    let (plan, measures) = rig.clean();
    let control = FitOutcome::failed(&["socket-1"], &FitFailure::Crashed);
    let passing = decide(&plan, &measures);
    let held = passing.clone().controlled_by(&control);
    assert_eq!(codes(&held), [FitCode::OriginalFailsFit]);
    assert!(held.findings()[0].detail.contains("kernel_crashed"));
    assert!(matches!(held.candidate(), FitStatus::NotVerified { .. }));
    assert!(matches!(
        held.socket("socket-1"),
        Some(FitStatus::NotVerified { .. })
    ));
    assert_eq!(passing.clone().controlled_by(&passing), passing);
    let (plan, mut measures) = rig.clean();
    let at = plate(&plan);
    measures.check.pairs[at].common_mm3 = 1.0;
    measures.check.pairs[at].residual_mm3 = 1.0;
    let hitting = decide(&plan, &measures).controlled_by(&control);
    assert!(matches!(hitting.candidate(), FitStatus::Interferes { .. }));
    assert!(matches!(
        hitting.socket("socket-1"),
        Some(FitStatus::Interferes { .. })
    ));
    assert!(
        !codes(&hitting).contains(&FitCode::OriginalFailsFit),
        "nothing withheld, no finding"
    );
    let interfering_control = decide(&plan, &measures);
    let under = passing.clone().controlled_by(&interfering_control);
    assert!(under.findings()[0].detail.contains("interferes"));
    // The PR 1b reviews: a control whose run was incomplete makes every
    // outcome under it incomplete, withheld or not.
    let timed_out = FitOutcome::failed(&["socket-1"], &FitFailure::Timeout);
    assert!(timed_out.incomplete());
    assert!(passing.clone().controlled_by(&timed_out).incomplete());
    assert!(
        decide(&plan, &measures)
            .controlled_by(&timed_out)
            .incomplete()
    );
    assert!(
        !passing.controlled_by(&control).incomplete(),
        "a crash is complete"
    );
}

/// FB3: a landmark 2e-6 mm off is `binding_failed`, one 5e-7 mm off is
/// not; a placed candidate's landmark off likewise.
#[test]
fn a_landmark_off_by_two_microns_fails_binding() {
    let rig = Rig::s01();
    let shift = |by: f64| {
        let (plan, mut measures) = rig.clean();
        let body = measures
            .scene
            .bodies
            .iter_mut()
            .find(|b| !b.landmarks.is_empty())
            .unwrap();
        if let LandmarkMeasure::Plane { offset_mm, .. } = &mut body.landmarks[0] {
            *offset_mm += by;
        }
        decide(&plan, &measures)
    };
    assert_eq!(codes(&shift(2e-6)), [FitCode::BindingFailed]);
    assert!(verified(shift(5e-7).candidate()));
    let (plan, mut measures) = rig.clean();
    if let LandmarkMeasure::Cylinder { radius_mm, .. } =
        &mut measures.place.placements[0].landmarks[2]
    {
        *radius_mm += 2e-6;
    }
    assert_eq!(codes(&decide(&plan, &measures)), [FitCode::BindingFailed]);
}

/// FB2, FB6: a body found by no solid, or by two at one location, and a
/// solid placed elsewhere, are `binding_failed`; an interference measured
/// on such a scene is no evidence.
#[test]
fn a_body_found_twice_or_not_at_all_fails_binding() {
    let rig = Rig::s01();
    let (plan, mut measures) = rig.clean();
    measures.scene.solids.push(measures.scene.solids[0].clone());
    let at = plate(&plan);
    measures.check.pairs[at].common_mm3 = 1.0;
    measures.check.pairs[at].residual_mm3 = 1.0;
    let twice = decide(&plan, &measures);
    assert_eq!(codes(&twice), [FitCode::BindingFailed]);
    assert!(matches!(twice.candidate(), FitStatus::NotVerified { .. }));
    let (plan, mut measures) = rig.clean();
    measures.scene.solids[1].location[0][3] += 2e-6;
    assert!(codes(&decide(&plan, &measures)).contains(&FitCode::BindingFailed));
    let (plan, mut measures) = rig.clean();
    measures.scene.bodies[0].found = false;
    assert!(codes(&decide(&plan, &measures)).contains(&FitCode::BindingFailed));
}

/// FB3: the placed candidate's box must lie within fitment's own box of
/// it, moved by the same placement.
#[test]
fn a_placed_box_outside_fitment_s_fails_binding() {
    let rig = Rig::s01();
    let (plan, mut measures) = rig.clean();
    measures.place.placements[0].bounds.max_mm[0] += 1.0;
    assert_eq!(codes(&decide(&plan, &measures)), [FitCode::BindingFailed]);
    let (plan, mut measures) = rig.clean();
    measures.place.placements.clear();
    assert!(codes(&decide(&plan, &measures)).contains(&FitCode::BindingFailed));
}

/// FB4: a file candidate's bytes must be the ones the helper hashed.
#[test]
fn another_candidate_s_bytes_fail_binding() {
    let mut rig = Rig::s01();
    rig.candidate = FitCandidate::of(&rig.part, &FitBody::step(b"the candidate"));
    let (plan, mut measures) = rig.clean();
    measures.place.sha256 = Some(crate::sha256::sha256_hex(b"the candidate"));
    assert!(verified(decide(&plan, &measures).candidate()));
    measures.place.sha256 = Some(crate::sha256::sha256_hex(b"another"));
    assert_eq!(codes(&decide(&plan, &measures)), [FitCode::BindingFailed]);
}

/// FZ7: a zone whose volume is off by 1e-8 relative, or whose centroid
/// is off by 2e-6 mm, or whose box is, is `zone_unbuilt`; within the
/// limits it is not.
#[test]
fn a_zone_off_its_closed_form_is_unbuilt() {
    let rig = Rig::s01();
    let run = |change: &dyn Fn(&mut ZoneMeasure)| {
        let (plan, mut measures) = rig.clean();
        let at = plate(&plan);
        change(&mut measures.check.pairs[at].zones[0]);
        decide(&plan, &measures)
    };
    assert_eq!(
        codes(&run(&|z| z.volume_mm3 *= 1.0 + 1e-8)),
        [FitCode::ZoneUnbuilt]
    );
    assert!(verified(run(&|z| z.volume_mm3 *= 1.0 + 1e-10).candidate()));
    assert_eq!(
        codes(&run(&|z| z.centroid_mm[2] += 2e-6)),
        [FitCode::ZoneUnbuilt]
    );
    assert!(verified(run(&|z| z.centroid_mm[2] += 5e-7).candidate()));
    assert_eq!(
        codes(&run(&|z| z.bounds.min_mm[0] -= 2e-6)),
        [FitCode::ZoneUnbuilt]
    );
    let (plan, mut measures) = rig.clean();
    let at = plate(&plan);
    measures.check.pairs[at].proximity_zones[0].centroid_mm[1] += 2e-6;
    assert_eq!(codes(&decide(&plan, &measures)), [FitCode::ZoneUnbuilt]);
}

/// FZ7: a pair one of whose zones has no closed form is not asked, and
/// is `zone_unbuilt`.
#[test]
fn a_zone_with_no_closed_form_is_not_asked() {
    let mut rig = Rig::s01();
    let zone = rig
        .zones
        .0
        .iter_mut()
        .find(|z| z.kind == super::super::ZoneKind::Slab)
        .unwrap();
    zone.shape = super::super::ZoneShape::Unbuilt {
        reason: super::super::Unbuilt::UnknownOutline,
    };
    let (plan, measures) = rig.clean();
    assert_eq!(plan.asked().count(), 4);
    assert_eq!(codes(&decide(&plan, &measures)), [FitCode::ZoneUnbuilt]);
}

/// FZ9: a zoned pair whose body is beyond the threshold is
/// `binding_failed`: the contact the zone declares is not there.
#[test]
fn a_zone_s_body_beyond_reach_fails_binding() {
    let rig = Rig::s01();
    let (plan, mut measures) = rig.clean();
    let at = plate(&plan);
    let pair = &mut measures.check.pairs[at];
    pair.distance_mm = None;
    pair.zoned_distance_mm = None;
    pair.body_kept_mm3 = None;
    pair.body_inside_mm3 = None;
    pair.proximity_zones.clear();
    assert_eq!(codes(&decide(&plan, &measures)), [FitCode::BindingFailed]);
}

/// FZ9 asks only a zone whose source says the bodies touch: PR 4's corpus
/// met near-band seats with their faces apart (C5 `near_gap`: s42, s54,
/// s57) and unread occupants with a gap (B8, I7: s17, s45), whose bodies
/// are rightly away. Such a zone, its body beyond reach, excuses nothing,
/// and the fit verifies.
#[test]
fn a_zone_that_declares_no_touch_may_have_its_body_away() {
    let mut rig = Rig::s01();
    let plate_path = rig.scene.bodies()[0].path.clone();
    for zone in rig.zones.0.iter_mut().filter(|z| z.body == plate_path) {
        zone.touching = false;
    }
    let (plan, mut measures) = rig.clean();
    let at = plate(&plan);
    let pair = &mut measures.check.pairs[at];
    pair.distance_mm = None;
    pair.zoned_distance_mm = None;
    pair.body_kept_mm3 = None;
    pair.body_inside_mm3 = None;
    pair.proximity_zones.clear();
    let outcome = decide(&plan, &measures);
    assert_eq!(codes(&outcome), []);
    assert!(matches!(outcome.candidate(), FitStatus::Verified { .. }));
}

/// FY4: R plus the Common's volume within the zones must be V, and the
/// body kept plus within the proximity zones its volume, within 1e-9.
#[test]
fn a_cut_that_loses_volume_is_conservation() {
    let rig = Rig::s01();
    let (plan, mut measures) = rig.clean();
    let at = plate(&plan);
    measures.check.pairs[at].common_mm3 = 1.0;
    measures.check.pairs[at].inside_mm3 = 0.5;
    measures.check.pairs[at].residual_mm3 = 0.4;
    let outcome = decide(&plan, &measures);
    assert_eq!(codes(&outcome), [FitCode::Conservation]);
    assert!(
        matches!(outcome.candidate(), FitStatus::NotVerified { .. }),
        "a residual from a cut that lost volume is no interference"
    );
    let (plan, mut measures) = rig.clean();
    measures.check.pairs[plate(&plan)].body_kept_mm3 = Some(VOLUME * (1.0 - 1e-8));
    assert_eq!(codes(&decide(&plan, &measures)), [FitCode::Conservation]);
    let (plan, mut measures) = rig.clean();
    measures.check.pairs[plate(&plan)].body_kept_mm3 = Some(VOLUME * (1.0 - 1e-11));
    assert!(verified(decide(&plan, &measures).candidate()));
}

/// FB6: a solid no body claims, within reach of a placement, is
/// `unclaimed_body`; one far away is counted, not reported.
#[test]
fn an_unclaimed_solid_within_reach_is_reported() {
    let rig = Rig::s01();
    let (plan, mut measures) = rig.clean();
    let near = measures.place.placements[0].bounds;
    measures.scene.solids.push(SolidMeasure {
        faces: vec![999_999],
        location: location(&Rigid::IDENTITY),
        bounds: near,
        non_solid: false,
    });
    assert_eq!(
        codes(&decide(&plan, &measures)),
        [FitCode::BindingFailed, FitCode::UnclaimedBody]
    );
    // One far away is not unclaimed, but no leaf of fitment's accounts
    // for it: the kernel's scene is not fitment's (FB6, the review's H5).
    let (plan, mut measures) = rig.clean();
    measures.scene.solids.push(SolidMeasure {
        faces: vec![999_999],
        location: location(&Rigid::IDENTITY),
        bounds: FitBox {
            min_mm: [1000.0; 3],
            max_mm: [1001.0; 3],
        },
        non_solid: false,
    });
    assert_eq!(codes(&decide(&plan, &measures)), [FitCode::BindingFailed]);
}

/// The reviews of PR 2b (FB6): a leaf's shape that is not a solid (a shell,
/// a face) is no leaf's solid and no body's: within reach of a placement
/// it is `unclaimed_body`, and with no box it is within reach of
/// everything; far away it is counted. It never counts as a leaf's solid,
/// even with a body's faces.
#[test]
fn a_shape_that_is_not_a_solid_is_unclaimed_near() {
    let rig = Rig::s01();
    let (plan, mut measures) = rig.clean();
    let near = measures.place.placements[0].bounds;
    let shape = |bounds| SolidMeasure {
        faces: Vec::new(),
        location: location(&Rigid::IDENTITY),
        bounds,
        non_solid: true,
    };
    measures.scene.solids.push(shape(near));
    assert_eq!(codes(&decide(&plan, &measures)), [FitCode::UnclaimedBody]);
    let (plan, mut measures) = rig.clean();
    measures.scene.solids.push(shape(FitBox {
        min_mm: [f64::NAN; 3],
        max_mm: [f64::NAN; 3],
    }));
    assert_eq!(codes(&decide(&plan, &measures)), [FitCode::UnclaimedBody]);
    let (plan, mut measures) = rig.clean();
    measures.scene.solids.push(shape(FitBox {
        min_mm: [1000.0; 3],
        max_mm: [1001.0; 3],
    }));
    assert!(verified(decide(&plan, &measures).candidate()));
    // A leaf's own solid reported as not one: its leaf has no solid.
    let (plan, mut measures) = rig.clean();
    measures.scene.solids[0].non_solid = true;
    assert!(codes(&decide(&plan, &measures)).contains(&FitCode::BindingFailed));
}

/// FT8: a body the helper finds within reach that the plan does not pair
/// is `untested_body`; a missing claim is a protocol failure.
#[test]
fn a_body_within_the_helper_s_reach_and_untested() {
    let rig = Rig::s01();
    let (plan, mut measures) = rig.clean();
    let claim = measures.check.reach.iter_mut().find(|c| !c.within).unwrap();
    claim.within = true;
    assert_eq!(codes(&decide(&plan, &measures)), [FitCode::UntestedBody]);
    let (plan, mut measures) = rig.clean();
    measures.check.reach.pop();
    assert_eq!(codes(&decide(&plan, &measures)), [FitCode::HelperProtocol]);
}

/// FT6: the candidate or a body of an asked pair invalid, without volume,
/// or at a shape tolerance above t/10: `invalid_body`,
/// `tolerance_too_large`; t/10 itself passes.
#[test]
fn bodies_unfit_to_be_judged() {
    let rig = Rig::s01();
    let (plan, mut measures) = rig.clean();
    measures.place.valid = false;
    assert_eq!(codes(&decide(&plan, &measures)), [FitCode::InvalidBody]);
    let (plan, mut measures) = rig.clean();
    measures.place.volume_mm3 = 0.0;
    assert_eq!(codes(&decide(&plan, &measures)), [FitCode::InvalidBody]);
    let (plan, mut measures) = rig.clean();
    measures.place.tolerance_mm = 0.002;
    assert!(codes(&decide(&plan, &measures)).contains(&FitCode::ToleranceTooLarge));
    let (plan, mut measures) = rig.clean();
    let plate_path = plan.asked().nth(plate(&plan)).unwrap().body.clone();
    let body = measures
        .scene
        .bodies
        .iter_mut()
        .find(|b| b.path == plate_path)
        .unwrap();
    body.tolerance_mm = 0.001;
    assert!(
        verified(decide(&plan, &measures).candidate()),
        "t/10 itself"
    );
    let body = measures
        .scene
        .bodies
        .iter_mut()
        .find(|b| b.path == plate_path)
        .unwrap();
    body.tolerance_mm = 0.0019;
    assert_eq!(
        codes(&decide(&plan, &measures)),
        [FitCode::ToleranceTooLarge]
    );
}

/// FH5, FY4: measures out of range, or a pair missing, are a protocol
/// failure, never a pass.
#[test]
fn measures_out_of_range_break_the_protocol() {
    let rig = Rig::s01();
    let broken: [&dyn Fn(&mut PairMeasure); 5] = [
        &|p| p.common_mm3 = -1.0,
        &|p| p.residual_mm3 = f64::NAN,
        &|p| {
            p.common_mm3 = 1.0;
            p.residual_mm3 = 2.0;
        },
        &|p| p.distance_mm = Some(-0.1),
        &|p| p.zones.clear(),
    ];
    for change in broken {
        let (plan, mut measures) = rig.clean();
        change(&mut measures.check.pairs[0]);
        assert_eq!(codes(&decide(&plan, &measures)), [FitCode::HelperProtocol]);
    }
    let (plan, mut measures) = rig.clean();
    measures.check.pairs.pop();
    assert_eq!(codes(&decide(&plan, &measures)), [FitCode::HelperProtocol]);
    let (plan, mut measures) = rig.clean();
    let at = plate(&plan);
    measures.check.pairs[at].zoned_distance_mm = None;
    assert_eq!(codes(&decide(&plan, &measures)), [FitCode::HelperProtocol]);
}

/// FV8: a timeout, a resource kill, a cancel, a protocol failure and an
/// unreadable store leave the run incomplete; two crashes (the retry
/// failed too) and an unavailable helper do not; none is verified.
#[test]
fn process_failures_are_never_verified() {
    let cases = [
        (FitFailure::Timeout, FitCode::HelperTimeout, true),
        (
            FitFailure::ResourceExhausted,
            FitCode::ResourceExhausted,
            true,
        ),
        (FitFailure::Cancelled, FitCode::Cancelled, true),
        (FitFailure::Protocol, FitCode::HelperProtocol, true),
        (
            FitFailure::StoredBytesUnreadable("missing: no file for the content".to_owned()),
            FitCode::StoredBytesUnreadable,
            true,
        ),
        (FitFailure::Crashed, FitCode::KernelCrashed, false),
        (
            FitFailure::Unavailable("libGL.so.1 is missing".to_owned()),
            FitCode::HelperUnavailable,
            false,
        ),
    ];
    for (failure, code, incomplete) in cases {
        let outcome = FitOutcome::failed(&["socket-1", "socket-2"], &failure);
        assert_eq!(outcome.code(), Some(code));
        assert_eq!(outcome.incomplete(), incomplete, "{failure:?}");
        assert!(matches!(outcome.candidate(), FitStatus::NotVerified { .. }));
        assert_eq!(outcome.sockets().len(), 2);
        assert!(
            outcome
                .sockets()
                .iter()
                .all(|(_, s)| matches!(s, FitStatus::NotVerified { reason } if reason.starts_with(code.sentence())))
        );
    }
}

/// FV6: a placement free in a motion FT10 does not excuse is
/// `placement_not_unique` at every socket, the motions named.
#[test]
fn a_free_placement_is_not_unique() {
    let outcome = FitOutcome::not_unique(
        &["socket-1", "socket-2"],
        &[(
            "socket-2",
            "a turn about [0.0, 0.0, 0.0] [0.0, 0.0, 1.0]".to_owned(),
        )],
    );
    assert_eq!(codes(&outcome), [FitCode::PlacementNotUnique]);
    assert!(outcome.sockets().iter().all(
        |(_, s)| matches!(s, FitStatus::NotVerified { reason } if reason.contains("a turn about"))
    ));
    assert!(!outcome.incomplete());
}

/// FV7: a copy pair's interference counts for both occurrences'
/// sockets; a finding at one placement only for its own socket.
#[test]
fn a_copy_s_interference_counts_for_both_sockets() {
    // s01's four bolts, the second of them in a socket of its own and
    // placed 4 mm up the first's axis: a copy pair between them.
    let mut rig = Rig::of(S01, "BOLT");
    assert_eq!(rig.placements.len(), 4);
    let up = Rigid::new(Rigid::IDENTITY.rotation(), [0.0, 0.0, 4.0]).unwrap();
    rig.placements[1].transform = rig.placements[0].transform.then(&up).unwrap();
    rig.placements[1].socket = "socket-2".to_owned();
    let (first, second) = (rig.placements[0].clone(), rig.placements[1].clone());
    let (plan, mut measures) = rig.clean();
    let copies: Vec<&Pair> = plan
        .asked()
        .filter(|p| p.copy && [&first.occurrence, &second.occurrence].contains(&&p.occurrence))
        .collect();
    assert_eq!(copies.len(), 2, "each copy pair once from each side");
    assert!(verified(decide(&plan, &measures).candidate()));
    let at = plan
        .asked()
        .position(|p| p.copy && p.occurrence == first.occurrence && p.body == second.occurrence)
        .unwrap();
    measures.check.pairs[at].common_mm3 = 2.0;
    measures.check.pairs[at].residual_mm3 = 2.0;
    measures.check.pairs[at].distance_mm = Some(0.0);
    let outcome = decide(&plan, &measures);
    assert!(matches!(
        outcome.socket("socket-1"),
        Some(FitStatus::Interferes { .. })
    ));
    assert!(matches!(
        outcome.socket("socket-2"),
        Some(FitStatus::Interferes { .. })
    ));
    assert!(matches!(outcome.candidate(), FitStatus::Interferes { .. }));
    // A proximity at the second placement alone: socket-1 stays verified.
    measures.check = clean_check(&plan);
    let near = plan
        .asked()
        .position(|p| !p.copy && p.occurrence == second.occurrence && p.zones.is_empty())
        .unwrap();
    measures.check.pairs[near].distance_mm = Some(0.005);
    let outcome = decide(&plan, &measures);
    assert!(verified(outcome.socket("socket-1").unwrap()));
    assert!(matches!(
        outcome.socket("socket-2"),
        Some(FitStatus::NotVerified { .. })
    ));
    assert!(matches!(outcome.candidate(), FitStatus::NotVerified { .. }));
}

/// FV9: the reasons are a closed list of 21 codes, each with its own
/// code and sentence.
#[test]
fn the_reasons_are_a_closed_list() {
    let mut names: Vec<&str> = FitCode::ALL.iter().map(|c| c.as_str()).collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), 21);
    for code in FitCode::ALL {
        let written = serde_json::to_value(code).unwrap();
        assert_eq!(written, code.as_str());
        assert_ne!(code.sentence(), "");
    }
    let finding = Finding::new(FitCode::Proximity, FindingAt::Run, "0.005 mm");
    assert_eq!(
        finding.sentence(),
        "a body is within the threshold outside every declared contact: 0.005 mm"
    );
}

// The reviews' findings, each a test that passed a pass before its fix.

/// OCCT's empty box and a box with a NaN.
fn unsound_boxes() -> [FitBox; 2] {
    [
        FitBox {
            min_mm: [1e100; 3],
            max_mm: [-1e100; 3],
        },
        FitBox {
            min_mm: [f64::NAN, 0.0, 0.0],
            max_mm: [1.0; 3],
        },
    ]
}

/// The index of the measure of the scene body whose label starts with
/// `product`.
fn body_measure(rig: &Rig, measures: &FitMeasures, product: &str) -> usize {
    let path = &rig
        .scene
        .bodies()
        .iter()
        .find(|b| b.label().starts_with(product))
        .unwrap()
        .path;
    measures
        .scene
        .bodies
        .iter()
        .position(|m| &m.path == path)
        .unwrap()
}

/// The pair of the asked pairs whose body's label starts with `product`,
/// made to interfere.
fn interfering(plan: &FitPlan, measures: &mut FitMeasures, product: &str) {
    let at = plan
        .asked()
        .position(|p| p.name.starts_with(product))
        .unwrap();
    let pair = &mut measures.check.pairs[at];
    pair.common_mm3 = 3.0;
    pair.residual_mm3 = 3.0;
    pair.distance_mm = Some(0.0);
}

/// C1: a found body whose kernel box is unsound (OCCT's empty box, a NaN)
/// is never culled, and fails binding: here a nut, far from the bracket.
#[test]
fn an_unsound_kernel_box_is_never_culled() {
    let rig = Rig::s01();
    for bad in unsound_boxes() {
        let mut measures = rig.read();
        let nut = body_measure(&rig, &measures, "NUT");
        measures.scene.bodies[nut].bounds = bad;
        let plan = rig.plan(&measures);
        let path = &measures.scene.bodies[nut].path;
        assert!(plan.asked().any(|p| &p.body == path), "{bad:?}");
        measures.check = clean_check(&plan);
        let outcome = decide(&plan, &measures);
        assert_eq!(codes(&outcome), [FitCode::BindingFailed], "{bad:?}");
        assert!(matches!(outcome.candidate(), FitStatus::NotVerified { .. }));
    }
    // The candidate's own box likewise.
    for bad in unsound_boxes() {
        let mut measures = rig.read();
        measures.place.placements[0].bounds = bad;
        let plan = rig.plan(&measures);
        assert_eq!(plan.clear(), 0, "every body within reach of an unknown box");
        measures.check = clean_check(&plan);
        assert!(codes(&decide(&plan, &measures)).contains(&FitCode::BindingFailed));
    }
}

/// C2: a kernel box of the placed candidate too small (here a 1 mm cube
/// within it, at a skew placement) culls nothing: reach is judged on its
/// hull with fitment's own box, and the plate's interference is found.
#[test]
fn a_candidate_box_too_small_culls_nothing() {
    let mut rig = Rig::s01();
    let (sin, cos) = libm::sincos(5f64.to_radians());
    let skew = Rigid::new(
        [[cos, -sin, 0.0], [sin, cos, 0.0], [0.0, 0.0, 1.0]],
        [0.0; 3],
    )
    .unwrap();
    rig.placements[0].transform = skew;
    let mut measures = rig.read();
    measures.place.placements[0].bounds = FitBox {
        min_mm: [0.0, 0.0, 3.0],
        max_mm: [1.0, 1.0, 4.0],
    };
    let plan = rig.plan(&measures);
    measures.check = clean_check(&plan);
    interfering(&plan, &mut measures, "PLATE");
    let outcome = decide(&plan, &measures);
    assert!(
        matches!(outcome.candidate(), FitStatus::Interferes { .. }),
        "{:?}",
        outcome.findings()
    );
}

/// C2: a neighbour's kernel box too small (a 1 mm cube at the plate's
/// far corner) culls nothing either.
#[test]
fn a_neighbour_box_too_small_culls_nothing() {
    let rig = Rig::s01();
    let mut measures = rig.read();
    let plate = body_measure(&rig, &measures, "PLATE");
    measures.scene.bodies[plate].bounds = FitBox {
        min_mm: [-50.0, -30.0, -10.0],
        max_mm: [-49.0, -29.0, -9.0],
    };
    let plan = rig.plan(&measures);
    measures.check = clean_check(&plan);
    interfering(&plan, &mut measures, "PLATE");
    assert!(matches!(
        decide(&plan, &measures).candidate(),
        FitStatus::Interferes { .. }
    ));
}

/// H4: a neighbour's kernel box must lie within fitment's box of it, as
/// the candidate's must (FB3).
#[test]
fn a_neighbour_box_off_fitment_s_fails_binding() {
    let rig = Rig::s01();
    let (plan, mut measures) = rig.clean();
    let plate = body_measure(&rig, &measures, "PLATE");
    measures.scene.bodies[plate].bounds.max_mm[0] += 1.0;
    measures.scene.bodies[plate].bounds.min_mm[0] += 1.0;
    assert_eq!(codes(&decide(&plan, &measures)), [FitCode::BindingFailed]);
}

/// H4: a block of planes only (p21's channel), slid in its own plane, moves
/// a landmark: its landmarks fix every direction and every slide (FB3).
#[test]
fn an_in_plane_slide_moves_a_landmark() {
    let part =
        crate::features::read_part(include_bytes!("../../../tests/data/p21_planes.stp")).unwrap();
    assert_eq!(part.cylinders.len(), 0);
    let own = crate::fit::landmarks(&part);
    let read: Vec<LandmarkMeasure> = own.iter().map(measure_of).collect();
    assert!(landmarks_agree(&own, &read));
    for shift in [[0.5, 0.0, 0.0], [0.0, 0.5, 0.0], [0.0, 0.0, 0.5]] {
        let slid = Rigid::new(Rigid::IDENTITY.rotation(), shift).unwrap();
        let moved: Vec<Landmark> = own.iter().map(|l| l.placed(&slid).unwrap()).collect();
        assert!(!landmarks_agree(&moved, &read), "{shift:?}");
    }
}

/// H3: a pair with no zone is judged on its distance alone; a zoned
/// distance, a body's cut or a proximity zone measured for it answers no
/// request, and breaks the protocol.
#[test]
fn an_unasked_zoned_distance_breaks_the_protocol() {
    let mut bare = Rig::s01();
    let plate_path = bare.scene.bodies()[0].path.clone();
    assert!(bare.scene.bodies()[0].label().starts_with("PLATE"));
    bare.zones.0.retain(|z| z.body != plate_path);
    let answers: [&dyn Fn(&mut PairMeasure); 3] = [
        &|p| p.zoned_distance_mm = Some(0.3),
        &|p| p.body_kept_mm3 = Some(VOLUME),
        &|p| p.proximity_zones = vec![ZoneMeasure::default()],
    ];
    for answer in answers {
        let (plan, mut measures) = bare.clean();
        let at = plate(&plan);
        measures.check.pairs[at].distance_mm = Some(0.005);
        answer(&mut measures.check.pairs[at]);
        assert_eq!(codes(&decide(&plan, &measures)), [FitCode::HelperProtocol]);
    }
    let (plan, mut measures) = bare.clean();
    measures.check.pairs[plate(&plan)].distance_mm = Some(0.005);
    assert_eq!(codes(&decide(&plan, &measures)), [FitCode::Proximity]);
}

/// M6: measures that contradict themselves break the protocol: volume
/// within no zone, more within the zones than they hold, a Common between
/// bodies apart, a body less its zones nearer than the body.
#[test]
fn measures_that_contradict_themselves_break_the_protocol() {
    let rig = Rig::s01();
    let contradictions: [&dyn Fn(&mut PairMeasure); 3] = [
        &|p| {
            p.common_mm3 = 1000.0;
            p.inside_mm3 = 1000.0;
        },
        &|p| {
            p.common_mm3 = 1.0;
            p.inside_mm3 = 1.0;
            p.distance_mm = Some(0.5);
        },
        &|p| {
            p.distance_mm = Some(0.009);
            p.zoned_distance_mm = Some(0.005);
        },
    ];
    for contradiction in contradictions {
        let (plan, mut measures) = rig.clean();
        contradiction(&mut measures.check.pairs[plate(&plan)]);
        assert_eq!(codes(&decide(&plan, &measures)), [FitCode::HelperProtocol]);
    }
    let mut bare = Rig::s01();
    let plate_path = bare.scene.bodies()[0].path.clone();
    bare.zones.0.retain(|z| z.body != plate_path);
    let (plan, mut measures) = bare.clean();
    let at = plate(&plan);
    measures.check.pairs[at].common_mm3 = 0.1;
    measures.check.pairs[at].inside_mm3 = 0.1;
    measures.check.pairs[at].distance_mm = Some(0.0);
    assert_eq!(codes(&decide(&plan, &measures)), [FitCode::HelperProtocol]);
}

/// FY4 at the kernel's rounding (PR 4's corpus, `f_below_resolution` in
/// a skew frame): a body less its zones measured 1.7e-17 mm under the body
/// is one contact measured twice, a proximity, not a contradiction; 2e-9
/// mm under it still breaks the protocol.
#[test]
fn a_zoned_distance_under_the_distance_by_rounding_is_consistent() {
    let rig = Rig::s01();
    let (plan, mut measures) = rig.clean();
    let at = plate(&plan);
    measures.check.pairs[at].distance_mm = Some(1.734_723_475_976_807e-17);
    measures.check.pairs[at].zoned_distance_mm = Some(0.0);
    assert_eq!(codes(&decide(&plan, &measures)), [FitCode::Proximity]);
    let (plan, mut measures) = rig.clean();
    measures.check.pairs[at].distance_mm = Some(2e-9);
    measures.check.pairs[at].zoned_distance_mm = Some(0.0);
    assert_eq!(codes(&decide(&plan, &measures)), [FitCode::HelperProtocol]);
}

/// M6: conservation's slack is 1e-9 of the volume but never a tenth of
/// the resolution: 5e-7 mm³ lost from a 1000 mm³ body is a miss.
#[test]
fn conservation_s_slack_is_bounded() {
    assert!((slack(1000.0) - FIT_VOLUME_MM3 / 10.0).abs() < 1e-20);
    assert!((slack(1.0) - 1e-9).abs() < 1e-20);
    let rig = Rig::s01();
    let (plan, mut measures) = rig.clean();
    measures.check.pairs[plate(&plan)].body_kept_mm3 = Some(VOLUME - 5e-7);
    assert_eq!(codes(&decide(&plan, &measures)), [FitCode::Conservation]);
}

/// M7: a plan with no placement, or not one at each selected occurrence,
/// is not verified; `not_verified` with no finding has its code's
/// sentence.
#[test]
fn an_empty_or_partial_plan_is_not_verified() {
    let mut rig = Rig::s01();
    rig.placements.clear();
    let (plan, measures) = rig.clean();
    assert_eq!(plan.pairs().len(), 0);
    let outcome = decide(&plan, &measures);
    assert_eq!(codes(&outcome), [FitCode::BindingFailed]);
    assert!(matches!(outcome.candidate(), FitStatus::NotVerified { .. }));
    let mut rig = Rig::of(S01, "BOLT");
    rig.placements.pop();
    let (plan, measures) = rig.clean();
    assert!(codes(&decide(&plan, &measures)).contains(&FitCode::BindingFailed));
    let mut rig = Rig::of(S01, "BOLT");
    let again = rig.placements[0].clone();
    rig.placements[1] = again;
    let (plan, measures) = rig.clean();
    assert!(codes(&decide(&plan, &measures)).contains(&FitCode::BindingFailed));
    let free = FitOutcome::not_unique(&["socket-1"], &[]);
    assert_eq!(
        free.candidate(),
        &FitStatus::NotVerified {
            reason: FitCode::PlacementNotUnique.sentence().to_owned()
        }
    );
}

/// M9: each reply must echo its request's digest; one that answers another
/// request, or none, breaks the protocol.
#[test]
fn a_reply_to_another_request_breaks_the_protocol() {
    let rig = Rig::s01();
    let wrong = Some("0".repeat(64));
    let replies: [&dyn Fn(&mut FitMeasures); 4] = [
        &|m| m.scene.request_sha256.clone_from(&wrong),
        &|m| m.place.request_sha256.clone_from(&wrong),
        &|m| m.check.request_sha256.clone_from(&wrong),
        &|m| m.check.request_sha256 = None,
    ];
    for reply in replies {
        let (plan, mut measures) = rig.clean();
        reply(&mut measures);
        assert_eq!(codes(&decide(&plan, &measures)), [FitCode::HelperProtocol]);
    }
    // Another candidate's check: another digest.
    let mut other = Rig::s01();
    other.candidate.uncertainty_mm = 1e-4;
    let (theirs, _) = other.clean();
    let (plan, _) = rig.clean();
    assert_ne!(theirs.check_digest(), plan.check_digest());
}

/// LOW: a body, a placement or a claim of reach measured twice breaks the
/// protocol: no first one wins.
#[test]
fn measures_named_twice_break_the_protocol() {
    let rig = Rig::s01();
    let twice: [&dyn Fn(&mut FitMeasures); 3] = [
        &|m| {
            let again = m.scene.bodies[0].clone();
            m.scene.bodies.push(again);
        },
        &|m| {
            let again = m.place.placements[0].clone();
            m.place.placements.push(again);
        },
        &|m| {
            let again = m.check.reach[0].clone();
            m.check.reach.push(again);
        },
    ];
    for change in twice {
        let (plan, mut measures) = rig.clean();
        change(&mut measures);
        assert_eq!(codes(&decide(&plan, &measures)), [FitCode::HelperProtocol]);
    }
}

/// LOW: a body whose kernel tolerance is not a number is never culled,
/// and fails binding.
#[test]
fn a_nan_tolerance_is_never_culled() {
    let rig = Rig::s01();
    let mut measures = rig.read();
    let nut = body_measure(&rig, &measures, "NUT");
    measures.scene.bodies[nut].tolerance_mm = f64::NAN;
    let plan = rig.plan(&measures);
    let path = measures.scene.bodies[nut].path.clone();
    assert!(plan.asked().any(|p| p.body == path));
    measures.check = clean_check(&plan);
    assert!(codes(&decide(&plan, &measures)).contains(&FitCode::BindingFailed));
}

/// LOW: FT7 counts a vertex offset as M4's `u_T` does: a candidate whose
/// vertices lie 0.02 mm off its surfaces leaves no tolerance.
#[test]
fn a_vertex_offset_counts_in_u_pair() {
    let mut rig = Rig::s01();
    rig.candidate.vertex_offset_mm = 0.02;
    let (plan, measures) = rig.clean();
    assert_eq!(plan.asked().count(), 0);
    assert_eq!(
        codes(&decide(&plan, &measures)),
        [FitCode::UncertaintyAboveTolerance]
    );
}
