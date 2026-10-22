//! VT62 on the committed assemblies (`docs/research/m7-fit.md`, FT10,
//! FC4): each original, placed where the matcher places it, is unique,
//! or free only to turn about an axis it is axisymmetric about; a turn
//! the shape sees is never excused. The corpus's originals and AL9's
//! seeds are `crates/fitment/tests/freedom.rs`'s.

use fitment_core::fit::{FreeMotion, Freedom, MotionKind, freedom};
use fitment_core::{
    CandidateSource, CandidateView, ComponentRead, MatchOptions, MatchPolicy, Matcher,
    NoFitVerifier, Part, Placement, Rigid, SocketDocument, SocketView, read_assembly,
};

const S01: &[u8] = include_bytes!("assemblies/s01_bolted.stp");
const S02: &[u8] = include_bytes!("assemblies/s02_cbore.stp");
const S03: &[u8] = include_bytes!("assemblies/s03_dowels.stp");

fn component(bytes: &[u8], name: &str) -> Part {
    let read = read_assembly(bytes);
    let listing = read.listing().unwrap();
    let selection = listing.select(Some(name), None).unwrap();
    match &listing.definitions()[selection.definition].read {
        Some(ComponentRead::Accepted(part)) => (**part).clone(),
        other => unreachable!("{name}: {other:?}"),
    }
}

/// Each socket's freedom for the original `part` of `bytes`, at the
/// placement the matcher chose, and at that placement turned a quarter
/// about z (where the original's own shape decides what is free).
fn freedoms(bytes: &[u8], part: &str) -> Vec<(Freedom, Placement)> {
    let document = SocketDocument::extract(bytes, Some(part), None).unwrap();
    let policy = MatchPolicy::default();
    let matcher = Matcher::new(
        &document,
        policy.clone(),
        MatchOptions::default(),
        &NoFitVerifier,
    )
    .unwrap();
    let original = component(bytes, part);
    let cand = CandidateView::of(&original);
    matcher
        .judge_part(&original, CandidateSource::Given)
        .sockets
        .iter()
        .map(|s| {
            let view = SocketView::new(&document, &s.socket).unwrap();
            let at = s.placement.clone().unwrap();
            (freedom(&view, &cand, &at, &policy), at)
        })
        .collect()
}

fn axisymmetric_turns(motions: &[FreeMotion]) -> bool {
    !motions.is_empty()
        && motions
            .iter()
            .all(|m| m.kind == MotionKind::Turn && m.axisymmetric)
}

/// s01's bracket (two axes) and nut (its hex seat fixes the turn) and
/// s03's plate (four dowels) are unique.
#[test]
fn placements_fixed_by_their_features_are_unique() {
    for (bytes, part) in [(S01, "BRACKET"), (S01, "NUT"), (S03, "PLATE")] {
        for (free, _) in freedoms(bytes, part) {
            assert!(free.unique(), "{part}: {free:?}");
            assert!(free.excused());
        }
    }
}

/// s03's dowels and s02's screw are free to turn about their axes, and
/// excused: every face is a plane square to the axis or a surface of
/// revolution on it. No slide is free.
#[test]
fn round_parts_are_free_to_turn_and_excused() {
    for (bytes, part) in [(S03, "DOWEL"), (S02, "SCREW")] {
        let found = freedoms(bytes, part);
        assert_ne!(found.len(), 0);
        for (free, _) in found {
            assert!(axisymmetric_turns(&free.motions), "{part}: {free:?}");
            assert!(free.excused() && !free.unique());
            assert_eq!(free.describe(), "");
        }
    }
}

/// FT10: the turn is about the axis in the socket's frame, so a placement
/// that is itself turned keeps the same freedom; a free motion is found
/// from the placement, never its seed.
#[test]
fn freedom_is_the_placement_s_own() {
    let document = SocketDocument::extract(S03, Some("DOWEL"), None).unwrap();
    let policy = MatchPolicy::default();
    let original = component(S03, "DOWEL");
    let cand = CandidateView::of(&original);
    let view = SocketView::new(&document, "socket-1").unwrap();
    let (free, at) = freedoms(S03, "DOWEL").remove(0);
    let quarter = Rigid::new(
        [[0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]],
        [0.0; 3],
    )
    .unwrap();
    // A quarter turn about the dowel's own axis first: the same body.
    let turned = Placement::given(quarter.then(&at.transform).unwrap());
    let again = freedom(&view, &cand, &turned, &policy);
    assert_eq!(again, free);
}
