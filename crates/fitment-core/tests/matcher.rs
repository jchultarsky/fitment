//! The matcher (`docs/research/m4-match.md`, sections 3, 7 and 12) on the
//! committed assemblies and parts: each original, aligned from scratch,
//! fails exactly what no candidate can pass (KH10); a refused file is a
//! `Reject` with the gate's codes (CF2); a part lacking what a socket
//! demands is rejected at the shortlist, every requirement still checked
//! (M4-17); and a document that is not reviewed is refused when a review
//! is required (MF2).

use fitment_core::{
    ComponentRead, FailureCode, MatchOptions, MatchPolicy, Matcher, Part, RejectReason, Rigid,
    SocketDocument, SocketRefused, Stage, VerdictKind, read_assembly,
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
