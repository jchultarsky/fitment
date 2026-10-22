//! VT53 (`docs/research/m7-fit.md`, section 6): every FV rule from
//! constructed measures. The plan is s01's bracket socket with the
//! bracket itself as the candidate, at the identity; a kernel that reads
//! the assembly exactly as fitment does gives the clean measures, and each
//! test changes one of them.

use super::*;
use crate::features::Part;
use crate::fit::{BodyMeasure, CheckMeasures, Zone};
use crate::fit::{
    FitBody, FitCandidate, FitIdentity, FitScene, PairMeasure, PlaceMeasures, Placed,
    PlacementMeasure, PlanInput, ReachClaim, SceneMeasures, SolidMeasure, ZoneMeasure, plan, zones,
};
use crate::matching::MatchPolicy;
use crate::socket::SocketDocument;

const S01: &[u8] = include_bytes!("../../../tests/assemblies/s01_bolted.stp");
const VOLUME: f64 = 1000.0;

/// s01's bracket socket and everything a plan of it needs.
struct Rig {
    scene: FitScene,
    zones: Vec<Zone>,
    part: Part,
    candidate: FitCandidate,
    placements: Vec<Placed>,
    policy: MatchPolicy,
}

impl Rig {
    fn s01() -> Self {
        let document = SocketDocument::extract(S01, Some("BRACKET"), None).unwrap();
        let scene = FitScene::of(S01, &document).unwrap();
        let zones = zones(&document, &scene);
        let bracket = scene.bodies().iter().find(|b| b.selected).unwrap();
        let part = scene.part(bracket).unwrap().clone();
        let body = FitBody::component(scene.sha256(), bracket.path.clone(), bracket.faces.clone());
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

    fn plan(&self, measures: &FitMeasures) -> FitPlan {
        let input = PlanInput::new(
            &self.scene,
            &self.zones,
            &self.candidate,
            &self.placements,
            &self.policy,
            Self::identity(),
        );
        plan(&input, &measures.scene, &measures.place)
    }

    /// The kernel's reading of the scene and the placed candidate, as
    /// fitment's own.
    fn read(&self) -> FitMeasures {
        let mut scene = SceneMeasures::default();
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
            });
        }
        let mut place = PlaceMeasures {
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
    let mut check = CheckMeasures::default();
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
    bare.zones.retain(|z| z.body != plate_path);
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
    let interfering_control = decide(&plan, &measures);
    let under = passing.controlled_by(&interfering_control);
    assert!(under.findings()[0].detail.contains("interferes"));
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
    measures.check.pairs[at].distance_mm = None;
    assert_eq!(codes(&decide(&plan, &measures)), [FitCode::BindingFailed]);
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
    measures.check.pairs[plate(&plan)].body_kept_mm3 = Some(VOLUME * (1.0 - 1e-10));
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
    });
    assert_eq!(codes(&decide(&plan, &measures)), [FitCode::UnclaimedBody]);
    let (plan, mut measures) = rig.clean();
    measures.scene.solids.push(SolidMeasure {
        faces: vec![999_999],
        location: location(&Rigid::IDENTITY),
        bounds: FitBox {
            min_mm: [1000.0; 3],
            max_mm: [1001.0; 3],
        },
    });
    assert!(verified(decide(&plan, &measures).candidate()));
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
            FitFailure::StoredBytesUnreadable,
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
    let mut rig = Rig::s01();
    // Two occurrences of the bracket in two sockets, the second 50 mm up:
    // a copy pair between them, within reach once lowered onto the first.
    let first = rig.placements[0].clone();
    let up = Rigid::new(Rigid::IDENTITY.rotation(), [0.0, 0.0, 4.0]).unwrap();
    let second = Placed::new("socket-2", vec![424_242], up);
    rig.placements.push(second.clone());
    let mut measures = rig.read();
    let mut copy = measures.place.placements[0].clone();
    copy.occurrence = second.occurrence.clone();
    copy.bounds = moved(&rig.part.extent.unwrap(), &up).unwrap();
    copy.landmarks = rig
        .candidate
        .landmarks
        .iter()
        .map(|l| measure_of(&l.placed(&up).unwrap()))
        .collect();
    measures.place.placements.push(copy);
    let plan = rig.plan(&measures);
    let copies: Vec<&Pair> = plan.asked().filter(|p| p.copy).collect();
    assert_eq!(copies.len(), 2, "each copy pair once from each side");
    measures.check = clean_check(&plan);
    let at = plan
        .asked()
        .position(|p| p.copy && p.occurrence == first.occurrence)
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
