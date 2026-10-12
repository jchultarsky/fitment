//! Holes and shafts (`docs/research/m2-features.md`, section 9), on parts
//! the review of PR 6 built to break the end walk. The generated corpus
//! checks every hole of every test part against its construction
//! (`support/expect.rs`); these name the cases one by one. Inputs are
//! included at compile time: core's tests do no file I/O.

use fitment_core::{Bottom, HoleKind, ShaftKind, StackEnd, UnknownReason, read_part};

/// A conical washer: a Ø6 bore 2 mm long through a cone that falls from
/// the bore's top back to its outer rim. Its faces run back along the bore's
/// axis, which the cone rule once read as material beyond: the bore came
/// out blind with an annular bottom on the rim.
const BELLEVILLE: &[u8] = include_bytes!("edits/belleville.stp");

/// A Ø10 flat-bottomed hole with a 90° conical dimple of radius 3 in its
/// floor: not a drill point.
const DIMPLE: &[u8] = include_bytes!("edits/dimple.stp");

/// A Ø10 hole whose point is two cones, r 5 to 2 over 1.5 mm, then r 2 to
/// the apex over 4 mm.
const DOUBLE_POINT: &[u8] = include_bytes!("edits/double_point.stp");

fn reason(end: &StackEnd) -> Option<UnknownReason> {
    match end {
        StackEnd::Unknown { reason, .. } => Some(*reason),
        _ => None,
    }
}

#[test]
fn a_face_running_back_along_the_axis_says_nothing_about_the_end() {
    let part = read_part(BELLEVILLE).unwrap();
    let [hole] = &part.holes[..] else {
        panic!("{} holes", part.holes.len());
    };
    assert_eq!(hole.kind, HoleKind::Unknown);
    assert!(
        hole.ends
            .iter()
            .any(|e| reason(e) == Some(UnknownReason::TurnsBack))
    );
    // The washer's rim is not rooted on its bore.
    assert!(part.shafts.iter().all(|s| s.kind != ShaftKind::RootFree));
}

#[test]
fn a_dimple_in_a_flat_floor_is_no_drill_point() {
    let part = read_part(DIMPLE).unwrap();
    let [hole] = &part.holes[..] else {
        panic!("{} holes", part.holes.len());
    };
    assert_eq!(hole.kind, HoleKind::Unknown);
    assert!(hole.ends.iter().all(|e| !matches!(
        e,
        StackEnd::Bottom {
            bottom: Bottom::Point { .. },
            ..
        }
    )));
}

#[test]
fn a_point_of_two_cones_measures_its_tip_to_the_apex() {
    let part = read_part(DOUBLE_POINT).unwrap();
    let [hole] = &part.holes[..] else {
        panic!("{} holes", part.holes.len());
    };
    assert_eq!(hole.kind, HoleKind::Blind);
    let StackEnd::Bottom {
        bottom: Bottom::Point { tip_length_mm, .. },
        ..
    } = &hole.ends[0]
    else {
        panic!("{:?}", hole.ends[0]);
    };
    assert!((tip_length_mm - 5.5).abs() < 1e-6, "{tip_length_mm}");
}
