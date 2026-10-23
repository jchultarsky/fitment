//! The matcher (`docs/research/m4-match.md`, sections 3, 7 and 12) on the
//! committed assemblies and parts: each original, aligned from scratch,
//! fails exactly what no candidate can pass (KH10); a refused file is a
//! `Reject` with the gate's codes (CF2); a part lacking what a socket
//! demands is rejected at the shortlist, every requirement still checked
//! (M4-17); and a document that is not reviewed is refused when a review
//! is required (MF2). VT61 (`docs/research/m7-fit.md`, FC2, FT2, FT9,
//! FT11): with a verifier that has an identity, the fit is asked once per
//! candidate whose every socket passed, at every occurrence of every
//! socket, the control once before the first, and never for a placement
//! the interface leaves free.

use fitment_core::fit::testing::{MeasuresVerifier, interfere};
use fitment_core::fit::{FitBody, FitCode, MotionKind, Placed};
use fitment_core::{
    Clearance, ComponentRead, FailureCode, FitIdentity, FitStatus, Grip, INTERFACE_REJECTED,
    INTERFACE_REJECTED_ELSEWHERE, MatchDocument, MatchOptions, MatchPolicy, Matcher, Part,
    RejectReason, ReviewStatus, Rigid, SelectedComponent, SocketDocument, SocketRefused, Stage,
    TolerancePolicy, VerdictKind, read_assembly,
};

const ASSEMBLIES: [(&str, &[u8], &[&str]); 4] = [
    (
        "s01",
        include_bytes!("assemblies/s01_bolted.stp"),
        &["BRACKET", "PLATE", "BOLT", "NUT"],
    ),
    (
        "s02",
        include_bytes!("assemblies/s02_cbore.stp"),
        &["BLOCK", "BASE", "SCREW"],
    ),
    (
        "s03",
        include_bytes!("assemblies/s03_dowels.stp"),
        &["PLATE", "DOWEL"],
    ),
    (
        "s08x",
        include_bytes!("assemblies/s08x_nested.stp"),
        &["BRACKET", "BOLT"],
    ),
];

fn component(bytes: &[u8], name: &str) -> Part {
    let read = read_assembly(bytes);
    let listing = read.listing().unwrap();
    let selection = listing.select(Some(name), None).unwrap();
    match &listing.definitions()[selection.definition].read {
        Some(ComponentRead::Accepted(part)) => (**part).clone(),
        other => panic!("{name} is not read: {other:?}"),
    }
}

/// KH10 on the committed assemblies: unseeded, each original passes every
/// requirement M4 can verify, fails the rest for the reason that fails
/// every candidate, and is placed where the identity is (AL13: any
/// placement that passes is as good).
#[test]
fn each_original_fails_exactly_what_no_candidate_can_pass() {
    let policy = MatchPolicy::default();
    for (case, bytes, parts) in ASSEMBLIES {
        for part in parts {
            let document = SocketDocument::extract(bytes, Some(part), None).unwrap();
            let matcher = Matcher::new(
                &document,
                policy.clone(),
                MatchOptions::default(),
                &fitment_core::NoFitVerifier,
            )
            .unwrap();
            let original = component(bytes, part);
            let result = matcher.judge_part(&original, fitment_core::CandidateSource::Given);
            assert_ne!(result.sockets.len(), 0);
            for socket in &result.sockets {
                assert_eq!(socket.stage, Stage::Verification, "{case} {part}");
                for (id, check) in &socket.checks {
                    let failed = check.evidence().failures();
                    assert!(
                        check.passed()
                            || failed.iter().all(|f| matches!(
                                f.code,
                                FailureCode::Unverifiable | FailureCode::FloorAboveTolerance
                            )),
                        "{case} {part} {} {id}: {failed:?}",
                        socket.socket
                    );
                }
                let seeded = matcher
                    .verify_at(&original, &socket.socket, &Rigid::IDENTITY)
                    .unwrap();
                let passed = |r: &fitment_core::SocketResult| {
                    r.checks
                        .iter()
                        .map(|(id, c)| (id.clone(), c.passed()))
                        .collect::<Vec<_>>()
                };
                assert_eq!(passed(socket), passed(&seeded), "{case} {part}");
            }
            let all_pass = result
                .sockets
                .iter()
                .all(|s| s.checks.iter().all(|(_, c)| c.passed()));
            let expected = if all_pass {
                VerdictKind::InterfaceMatch
            } else {
                VerdictKind::Reject
            };
            assert_eq!(result.verdict.kind(), expected, "{case} {part}");
        }
    }
}

/// CF2: a file the gate refuses is a `Reject` with its codes, judged
/// against no socket.
#[test]
fn a_refused_file_is_a_reject_with_the_gate_s_codes() {
    let (_, bytes, _) = ASSEMBLIES[0];
    let document = SocketDocument::extract(bytes, Some("BRACKET"), None).unwrap();
    let matcher = Matcher::new(
        &document,
        MatchPolicy::default(),
        MatchOptions::default(),
        &fitment_core::NoFitVerifier,
    )
    .unwrap();
    let result = matcher.judge_step(include_bytes!("data/r03_two_solids.stp"));
    assert_eq!(result.verdict.kind(), VerdictKind::Reject);
    assert!(result.sockets.is_empty() && !result.refusals.is_empty());
    assert!(matches!(
        result.verdict.reasons(),
        [RejectReason::CandidateRefused { codes, .. }] if !codes.is_empty()
    ));
}

/// M4-17: a part lacking what the socket demands is rejected at the
/// shortlist, with what is missing; alignment and verification still run,
/// so every active requirement has its check.
#[test]
fn a_part_lacking_the_socket_s_elements_is_rejected_at_the_shortlist() {
    let (_, bytes, _) = ASSEMBLIES[0];
    let document = SocketDocument::extract(bytes, Some("BRACKET"), None).unwrap();
    let matcher = Matcher::new(
        &document,
        MatchPolicy::default(),
        MatchOptions::default(),
        &fitment_core::NoFitVerifier,
    )
    .unwrap();
    let result = matcher.judge_step(include_bytes!("data/p21_planes.stp"));
    assert_eq!(result.verdict.kind(), VerdictKind::Reject);
    let socket = &result.sockets[0];
    assert_eq!(socket.stage, Stage::Shortlist);
    assert_ne!(socket.missing.len(), 0);
    assert!(matches!(
        socket.verdict.reasons().first(),
        Some(RejectReason::Shortlist { .. })
    ));
    let extracted = document.extracted().unwrap();
    assert_eq!(socket.checks.len(), extracted.sockets[0].requirements.len());
    assert!(socket.checks.iter().any(|(_, c)| !c.passed()));
}

/// MF2: with a review required, a document whose requirements are not all
/// reviewed is refused, naming them.
#[test]
fn an_unreviewed_document_is_refused_when_a_review_is_required() {
    let (_, bytes, _) = ASSEMBLIES[0];
    let document = SocketDocument::extract(bytes, Some("BRACKET"), None).unwrap();
    let required = MatchOptions::new(false, true);
    match Matcher::new(
        &document,
        MatchPolicy::default(),
        required,
        &fitment_core::NoFitVerifier,
    ) {
        Err(SocketRefused::Unreviewed { count, first, .. }) => {
            assert!(count > 0 && !first.is_empty() && first.len() <= 5);
            assert!(first.iter().all(|r| r.starts_with("socket-1/")));
        }
        other => panic!("{other:?}"),
    }
}

/// VD11, VD12: a candidate whose features are not complete says so; every
/// verdict says no material was compared.
#[test]
fn an_incomplete_candidate_is_flagged() {
    let (_, bytes, _) = ASSEMBLIES[0];
    let document = SocketDocument::extract(bytes, Some("BRACKET"), None).unwrap();
    let matcher = Matcher::new(
        &document,
        MatchPolicy::default(),
        MatchOptions::default(),
        &fitment_core::NoFitVerifier,
    )
    .unwrap();
    let step: &[u8] = include_bytes!("data/u01_undercut_floor.stp");
    let part = fitment_core::FeaturesDocument::read(step);
    assert!(!part.part().unwrap().complete);
    let result = matcher.judge_step(step);
    assert!(
        result
            .flags
            .contains(&fitment_core::CandidateFlag::CandidateIncomplete)
    );
    assert!(
        result
            .flags
            .contains(&fitment_core::CandidateFlag::MaterialNotCompared)
    );
    assert!(result.flags.windows(2).all(|w| w[0] < w[1]));
}

const S03: &[u8] = include_bytes!("assemblies/s03_dowels.stp");
const PIN: &str = include_str!("data/p09_pin.stp");

fn exact(bytes: &[u8]) -> MeasuresVerifier {
    MeasuresVerifier::exact(FitIdentity::new("open_cascade", "8.0.1"), bytes.to_vec())
}

fn dowels() -> SocketDocument {
    SocketDocument::extract(S03, Some("DOWEL"), None).unwrap()
}

/// The placements a request names: (socket, occurrence).
fn at(placements: &[Placed]) -> Vec<(String, Vec<u64>)> {
    placements
        .iter()
        .map(|p| (p.socket.clone(), p.occurrence.clone()))
        .collect()
}

/// VT61, FC2, FT2, FT11, FC5: s03's dowel has three sockets and four
/// occurrences. The original, judged as a component, is asked once, with
/// the control before it: each request holds every occurrence of every
/// socket, the candidate's at `T_k · A` from each socket's own placement,
/// and names the component by the scene's occurrence of its definition.
/// A second candidate is asked once more; the control is not asked again.
#[test]
fn the_fit_is_asked_once_per_candidate_at_every_occurrence() {
    let document = dowels();
    let verifier = exact(S03);
    let matcher = Matcher::new(
        &document,
        MatchPolicy::default(),
        MatchOptions::default(),
        &verifier,
    )
    .unwrap();
    let original = SelectedComponent::target(S03, &document).unwrap();
    let result = matcher.judge_component(&original, original.source());
    let every: Vec<(String, Vec<u64>)> = document
        .extracted()
        .unwrap()
        .sockets
        .iter()
        .flat_map(|s| s.occurrences.iter().map(|o| (s.id.clone(), o.clone())))
        .collect();
    assert_eq!(every.len(), 4);
    let asked = verifier.asked();
    assert_eq!(asked.len(), 2, "the control, then the candidate");
    for request in &asked {
        assert_eq!(request.placements, every);
        assert!(
            request.candidate.starts_with("component "),
            "{}",
            request.candidate
        );
        assert!(
            request.candidate.ends_with("[789]"),
            "{}",
            request.candidate
        );
    }
    let evidence = result.fit.as_deref().unwrap();
    assert_eq!(at(evidence.placements()), every);
    for socket in &result.sockets {
        let a = socket.placement.as_ref().unwrap().transform;
        let expected = Placed::every(&document, &socket.socket, &a).unwrap();
        let ours: Vec<&Placed> = evidence
            .placements()
            .iter()
            .filter(|p| p.socket == socket.socket)
            .collect();
        assert_eq!(ours.len(), expected.len());
        for (p, e) in ours.iter().zip(&expected) {
            assert_eq!(p.transform, e.transform, "T_k · A at {:?}", p.occurrence);
        }
        assert!(
            socket.freedom.is_some(),
            "freedom worked out at every socket"
        );
        assert!(matches!(socket.verdict.fit(), FitStatus::Verified { .. }));
    }
    assert!(matcher.fit_control().is_some());
    let pin = matcher.judge_step(PIN.as_bytes());
    assert_eq!(pin.verdict.kind(), VerdictKind::Match);
    let asked = verifier.asked();
    assert_eq!(asked.len(), 3, "one request more, the control not again");
    assert_eq!(asked[2].candidate, fitment_core::sha256_hex(PIN.as_bytes()));
}

/// FT11: a candidate whose interface rejects at one socket gets no fit:
/// a socket that passed beside one that failed records `another socket's
/// interface rejected`, each that failed `interface rejected`, and nothing
/// is asked, the control included. A 12 mm pin rejects at the dowels'
/// sockets but socket-2, whose shaft a review removed.
#[test]
fn a_socket_that_passed_beside_one_that_failed_gets_no_fit() {
    let mut document = dowels();
    document
        .decide(
            "shaft:shaft-729",
            Some("socket-2"),
            ReviewStatus::Removed,
            Some("the pin's length is free there"),
        )
        .unwrap();
    let verifier = exact(S03);
    let matcher = Matcher::new(
        &document,
        MatchPolicy::default(),
        MatchOptions::default(),
        &verifier,
    )
    .unwrap();
    let short = PIN.replace(",20.)", ",12.)");
    let result = matcher.judge_step(short.as_bytes());
    let reasons: Vec<(VerdictKind, String)> = result
        .sockets
        .iter()
        .map(|s| match s.verdict.fit() {
            FitStatus::NotChecked { reason } => (s.verdict.kind(), reason.clone()),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(
        reasons,
        [
            (VerdictKind::Reject, INTERFACE_REJECTED.to_owned()),
            (
                VerdictKind::InterfaceMatch,
                INTERFACE_REJECTED_ELSEWHERE.to_owned()
            ),
            (VerdictKind::Reject, INTERFACE_REJECTED.to_owned()),
        ]
    );
    assert_eq!(result.verdict.kind(), VerdictKind::Reject);
    assert!(result.fit.is_none());
    assert!(result.sockets.iter().all(|s| s.freedom.is_none()));
    assert_eq!(verifier.asked(), Vec::new());
    assert!(matcher.fit_control().is_none(), "no interface passed");
}

/// VT61, FT9: the control runs once, at the first candidate whose
/// interface passes, before its fit, and not at all while none does; a
/// document of rejected candidates states no control (`fit.control:
/// null`).
#[test]
fn the_control_runs_once_at_the_first_interface_pass() {
    let (_, s01, _) = ASSEMBLIES[0];
    let document = SocketDocument::extract(s01, Some("BRACKET"), None).unwrap();
    let verifier = exact(s01);
    let matcher = Matcher::new(
        &document,
        MatchPolicy::default(),
        MatchOptions::default(),
        &verifier,
    )
    .unwrap();
    let small = matcher.judge_step(include_bytes!("candidates/m_bracket_dia_small.stp"));
    assert_eq!(small.verdict.kind(), VerdictKind::Reject);
    assert_eq!(verifier.asked(), Vec::new());
    let none = MatchDocument::new(&matcher, vec![(small, vec!["small".to_owned()])]);
    let none: serde_json::Value = serde_json::from_str(&none.to_json()).unwrap();
    assert_eq!(none["format_version"], 2);
    assert_eq!(none["fit"]["control"], serde_json::Value::Null);
    let bracket: &[u8] = include_bytes!("candidates/m_bracket.stp");
    for n in [2, 3] {
        let result = matcher.judge_step(bracket);
        assert_eq!(result.verdict.kind(), VerdictKind::Match);
        let asked = verifier.asked();
        assert_eq!(asked.len(), n);
        assert!(
            asked[0].candidate.starts_with("component "),
            "the control first"
        );
    }
    let control = matcher.fit_control().unwrap();
    assert!(matches!(
        control.outcome().candidate(),
        FitStatus::Verified { .. }
    ));
    assert!(control.free_turns().is_empty(), "FT10 does not apply to it");
}

/// FT9, FV5: a control that is not verified withholds every candidate's
/// `verified`, `original_fails_fit` with the control's own code, and a
/// candidate that interferes still interferes.
#[test]
fn a_failing_control_withholds_verified_and_keeps_reject() {
    let (_, s01, _) = ASSEMBLIES[0];
    let document = SocketDocument::extract(s01, Some("BRACKET"), None).unwrap();
    let lug = fitment_core::sha256_hex(include_bytes!("candidates/m_bracket_lug.stp"));
    let verifier = MeasuresVerifier::new(
        FitIdentity::new("open_cascade", "8.0.1"),
        s01.to_vec(),
        move |request| {
            let mut measures = fitment_core::fit::testing::exact(request);
            let into_plate = match request.body() {
                FitBody::Component { .. } => true,
                FitBody::Step { sha256, .. } => *sha256 == lug,
                _ => false,
            };
            if into_plate {
                let plan = request.plan(&measures.scene, &measures.place);
                let plate = plan.asked().position(|p| p.name.starts_with("PLATE"));
                assert!(interfere(&mut measures, &plan, plate.unwrap(), 16.0));
            }
            Ok(measures)
        },
    );
    let matcher = Matcher::new(
        &document,
        MatchPolicy::default(),
        MatchOptions::default(),
        &verifier,
    )
    .unwrap();
    let bracket = matcher.judge_step(include_bytes!("candidates/m_bracket.stp"));
    assert_eq!(bracket.verdict.kind(), VerdictKind::InterfaceMatch);
    let outcome = bracket.fit.as_deref().unwrap().outcome();
    assert!(
        outcome
            .findings()
            .iter()
            .any(|f| f.code == FitCode::OriginalFailsFit && f.detail.contains("interferes")),
        "{:?}",
        outcome.findings()
    );
    let lug = matcher.judge_step(include_bytes!("candidates/m_bracket_lug.stp"));
    assert_eq!(lug.verdict.kind(), VerdictKind::Reject);
    assert!(matches!(lug.verdict.fit(), FitStatus::Interferes { .. }));
}

/// FV6, FC2: under a tolerance of 0.05 mm the dowels' interface leaves the
/// pin free to slide along its axis: its freedom says so at every socket,
/// no fit is asked for it (the control only), and every socket is
/// `placement_not_unique`.
#[test]
fn a_placement_free_to_slide_is_asked_no_fit() {
    let document = dowels();
    let verifier = exact(S03);
    let loose = TolerancePolicy::new("loose", 0.05, 0.01).unwrap();
    let policy = MatchPolicy::new(loose, Clearance::Strict, Grip::Hard);
    let matcher = Matcher::new(&document, policy, MatchOptions::default(), &verifier).unwrap();
    let result = matcher.judge_step(PIN.as_bytes());
    assert_eq!(result.verdict.kind(), VerdictKind::InterfaceMatch);
    for socket in &result.sockets {
        let freedom = socket.freedom.as_ref().unwrap();
        assert!(!freedom.excused(), "{freedom:?}");
        assert!(freedom.motions.iter().any(|m| m.kind == MotionKind::Slide));
    }
    let outcome = result.fit.as_deref().unwrap().outcome();
    assert_eq!(outcome.code(), Some(FitCode::PlacementNotUnique));
    assert_eq!(verifier.asked().len(), 1, "the control alone");
}

/// FC2: with no verifier, or one with no identity, nothing changes from
/// 0.3.0: no fit evidence, no freedom, no alternatives, every passing
/// socket `no fit verifier in this build`, and the document is v1.
#[test]
fn with_no_verifier_nothing_changes() {
    let (_, s01, _) = ASSEMBLIES[0];
    let document = SocketDocument::extract(s01, Some("BRACKET"), None).unwrap();
    let matcher = Matcher::new(
        &document,
        MatchPolicy::default(),
        MatchOptions::default(),
        &fitment_core::NoFitVerifier,
    )
    .unwrap();
    let result = matcher.judge_step(include_bytes!("candidates/m_bracket.stp"));
    assert_eq!(result.verdict.kind(), VerdictKind::InterfaceMatch);
    assert_eq!(result.verdict.fit(), &FitStatus::no_verifier());
    assert!(result.fit.is_none());
    let socket = &result.sockets[0];
    assert!(socket.freedom.is_none() && socket.alternatives.is_none());
    assert_eq!(matcher.fit_identity(), None);
    assert!(matcher.fit_control().is_none());
    let doc = MatchDocument::new(&matcher, vec![(result, vec!["m_bracket.stp".to_owned()])]);
    assert_eq!(doc.format_version(), fitment_core::MATCH_FORMAT_VERSION);
}
