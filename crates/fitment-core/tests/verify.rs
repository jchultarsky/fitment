//! Verification (`docs/research/m4-match.md`, section 8) on the committed
//! assemblies: each original, at the identity, passes every requirement of
//! its own sockets that M4 can verify, and fails the rest as unverifiable
//! or for the floor (Y4, VR21 seeded).

use fitment_core::{
    CandidateView, ComponentRead, FailureCode, MatchPolicy, Part, Placement, Rigid, SocketDocument,
    SocketView, read_assembly, verify,
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

/// Y4 at the identity: every original passes every requirement M4 can
/// verify, and fails each other one for the reason that fails every
/// candidate.
#[test]
fn every_original_passes_its_own_sockets_at_the_identity() {
    let mut failures = Vec::new();
    let mut passed = 0;
    for (case, bytes, parts) in ASSEMBLIES {
        for part in parts {
            let original = component(bytes, part);
            let view = CandidateView::of(&original);
            let document = SocketDocument::extract(bytes, Some(part), None).unwrap();
            for socket in &document.extracted().unwrap().sockets {
                let socket_view = SocketView::new(&document, &socket.id).unwrap();
                let policy = MatchPolicy::default();
                for requirement in socket_view.active() {
                    let at = Placement::given(Rigid::IDENTITY);
                    let check = verify(&socket_view, requirement, &view, &at, &policy);
                    let expected =
                        requirement.verifiable() && !socket_view.blocked(requirement, &policy);
                    let ok = if check.passed() {
                        expected
                    } else {
                        !expected
                            && check.evidence().failures().iter().all(|f| {
                                matches!(
                                    f.code,
                                    FailureCode::Unverifiable | FailureCode::FloorAboveTolerance
                                )
                            })
                    };
                    if ok {
                        passed += 1;
                    } else {
                        failures.push(format!(
                            "{case} {part} {} {}: {}",
                            socket.id,
                            requirement.id,
                            serde_json::to_string(check.evidence()).unwrap()
                        ));
                    }
                }
            }
        }
    }
    assert!(passed > 20, "{passed}");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Every check of one original's sockets at `at`: (requirement, codes),
/// for the requirements M4 can verify.
fn judged(case: usize, part: &str, at: &Rigid) -> Vec<(String, Vec<FailureCode>)> {
    let (_, bytes, _) = ASSEMBLIES[case];
    let original = component(bytes, part);
    let view = CandidateView::of(&original);
    let document = SocketDocument::extract(bytes, Some(part), None).unwrap();
    let policy = MatchPolicy::default();
    let mut out = Vec::new();
    for socket in &document.extracted().unwrap().sockets {
        let socket_view = SocketView::new(&document, &socket.id).unwrap();
        for requirement in socket_view.active() {
            if !requirement.verifiable() {
                continue;
            }
            let check = verify(
                &socket_view,
                requirement,
                &view,
                &Placement::given(*at),
                &policy,
            );
            let codes = check.evidence().failures().iter().map(|f| f.code).collect();
            out.push((requirement.id.clone(), codes));
        }
    }
    out
}

fn shifted(by: [f64; 3]) -> Rigid {
    Rigid::new(Rigid::IDENTITY.rotation(), by).unwrap()
}

/// A rotation by `angle` about z.
fn turned(angle: f64) -> Rigid {
    let (s, c) = (libm::sin(angle), libm::cos(angle));
    Rigid::new([[c, -s, 0.0], [s, c, 0.0], [0.0, 0.0, 1.0]], [0.0; 3]).unwrap()
}

/// VR3 at its edge on s03's pinned plate and s01's bracket: a placement
/// moved within half the tolerance passes, across the bores; one moved by
/// the tolerance itself rejects in every direction, for the guard band
/// leaves ε = t − u (VR2); along the seats' normal, the seats reject.
#[test]
fn a_placement_off_by_the_tolerance_rejects() {
    let t = 0.01;
    for (case, part) in [(2, "PLATE"), (0, "BRACKET")] {
        let inside = judged(case, part, &shifted([0.4 * t, 0.0, 0.0]));
        assert!(
            inside.iter().all(|(_, codes)| codes.is_empty()),
            "{inside:?}"
        );
        let along = judged(case, part, &shifted([0.0, 0.0, 0.4 * t]));
        assert!(along.iter().all(|(_, codes)| codes.is_empty()), "{along:?}");
        for by in [[t, 0.0, 0.0], [0.0, t, 0.0]] {
            let off = judged(case, part, &shifted(by));
            let bores: Vec<_> = off
                .iter()
                .filter(|(id, _)| id.starts_with("bore:"))
                .collect();
            assert_ne!(bores.len(), 0);
            assert!(
                bores
                    .iter()
                    .all(|(_, codes)| codes.contains(&FailureCode::AxisOffset)),
                "{by:?}: {bores:?}"
            );
        }
        let lifted = judged(case, part, &shifted([0.0, 0.0, t]));
        let seats: Vec<_> = lifted
            .iter()
            .filter(|(id, _)| id.starts_with("seat:"))
            .collect();
        assert!(
            seats
                .iter()
                .all(|(_, codes)| codes == &[FailureCode::NoFeature]),
            "{seats:?}"
        );
    }
}

/// VR3's lever (an axis compared at both ends of its spans, a plane over
/// its region): a turn about z of the angular tolerance moves the bracket's
/// bores, 20 mm and more from the turn's axis, by more than ε, and they
/// reject; a turn of a tenth of it passes.
#[test]
fn a_turn_is_judged_by_where_it_moves_the_features() {
    let angle = 0.01_f64.to_radians();
    let slight = judged(0, "BRACKET", &turned(angle / 10.0));
    assert!(
        slight.iter().all(|(_, codes)| codes.is_empty()),
        "{slight:?}"
    );
    let full = judged(0, "BRACKET", &turned(angle * 4.0));
    let bores: Vec<_> = full
        .iter()
        .filter(|(id, _)| id.starts_with("bore:"))
        .collect();
    assert!(
        bores.iter().all(|(_, codes)| !codes.is_empty()),
        "{bores:?}"
    );
}
