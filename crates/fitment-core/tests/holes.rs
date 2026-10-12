//! Holes and shafts (`docs/research/m2-features.md`, section 9), on parts
//! the review of PR 6 built to break the end walk. The generated corpus
//! checks every hole of every test part against its construction
//! (`support/expect.rs`); these name the cases one by one. Inputs are
//! included at compile time: core's tests do no file I/O.

use fitment_core::{Bottom, HoleKind, ShaftKind, StackEnd, UnknownReason, read_part};

/// `input` with each `from` replaced by its `to`; each must occur once.
fn edit(input: &[u8], edits: &[(&str, &str)]) -> Vec<u8> {
    let mut text = std::str::from_utf8(input).unwrap().to_owned();
    for (from, to) in edits {
        assert_eq!(text.matches(from).count(), 1, "{from}");
        text = text.replacen(from, to, 1);
    }
    text.into_bytes()
}

/// A conical washer: a Ø6 bore 2 mm long through a cone that falls from
/// the bore's top back to its outer rim. Its faces run back along the bore's
/// axis, which the cone rule once read as material beyond: the bore came
/// out blind with an annular bottom on the rim.
const P01_THROUGH: &[u8] = include_bytes!("data/p01_through.stp");

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

#[test]
fn stations_come_from_circle_centres_not_vertices() {
    // E1: the top circle's one vertex moved 0.005 mm up the axis, within a
    // stated uncertainty of 0.01 mm. A station read from the vertex would
    // make the hole 8.005 long; from the circle's centre it stays 8.
    let moved = edit(
        P01_THROUGH,
        &[
            ("LENGTH_MEASURE(1.E-07)", "LENGTH_MEASURE(1.E-02)"),
            (
                "#112 = CARTESIAN_POINT('',(16.7,15.,8.));",
                "#112 = CARTESIAN_POINT('',(16.7,15.,8.005));",
            ),
        ],
    );
    let part = read_part(&moved).unwrap();
    let [hole] = &part.holes[..] else {
        panic!("{} holes", part.holes.len());
    };
    assert_eq!(hole.kind, HoleKind::Through);
    assert_eq!(hole.length_mm, Some(8.0));
}

/// A pointed tip: one cone face bounded only by its base circle, with no
/// vertex at the apex. The face holds the apex, so the part's extent must
/// reach it and the cone's radii must run to 0 (review of M3 PR 2).
const CONE_TIP: &[u8] = include_bytes!("edits/cone_tip.stp");

#[test]
fn a_cone_face_holding_its_apex_bounds_it() {
    let document = fitment_core::FeaturesDocument::read(CONE_TIP);
    let part = document.part().expect("accepted");
    let extent = part.extent.expect("planes, a cone and a circle bound it");
    // Apex 10 / tan(0.2) below the base.
    let depth = 10.0 / libm::tan(0.2);
    assert!(extent.min_mm.to_array()[2] <= -depth + 1e-9, "{extent:?}");
    let [low, high] = part.cones[0].radii_mm.expect("one coaxial circle");
    assert_eq!((low, high), (0.0, 10.0));
}
