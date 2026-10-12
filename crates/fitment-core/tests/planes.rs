//! Planes and the face partition (`docs/research/m2-features.md`, sections
//! 6 and 7), on the committed unit parts and edits of them. Inputs are
//! included at compile time: core's tests do no file I/O.

// The values compared exactly are stated in the files, or exact in binary.
#![allow(clippy::float_cmp)]

use std::f64::consts::PI;

use fitment_core::{
    FeatureKind, FeaturesDocument, FormatError, NoteCode, Outline, OutlineProblem, Part, Segment,
    read_part,
};

const P01_THROUGH: &[u8] = include_bytes!("data/p01_through.stp");
const P05_CBORE: &[u8] = include_bytes!("data/p05_cbore.stp");
const P21_PLANES: &[u8] = include_bytes!("data/p21_planes.stp");

/// `input` with each `from` replaced by its `to`; each must occur once.
fn edit(input: &[u8], edits: &[(&str, &str)]) -> Vec<u8> {
    let mut text = std::str::from_utf8(input).unwrap().to_owned();
    for (from, to) in edits {
        assert_eq!(text.matches(from).count(), 1, "{from}");
        text = text.replacen(from, to, 1);
    }
    text.into_bytes()
}

/// The plane holding face `face`.
fn plane_of(part: &Part, face: u64) -> &fitment_core::Plane {
    part.planes
        .iter()
        .find(|p| p.faces.iter().any(|f| f.face == face))
        .unwrap()
}

#[test]
fn a_plane_face_with_a_hole_has_its_exact_area_and_loops() {
    let part = read_part(P01_THROUGH).unwrap();
    let top = plane_of(&part, 88);
    assert_eq!(top.id.to_string(), "plane-88");
    assert_eq!(top.normal.to_array(), [0.0, 0.0, 1.0]);
    assert_eq!(top.offset_mm, 8.0);
    let expected = 40.0 * 30.0 - PI * 3.3 * 3.3;
    assert!((top.area_mm2.unwrap() - expected).abs() < 1e-9);
    let Outline::Known { loops, .. } = &top.faces[0].outline else {
        panic!("p01's top outline is unknown");
    };
    assert_eq!(loops.len(), 2);
    assert!(loops[0].outer && !loops[1].outer);
    assert_eq!(loops[0].area_mm2, 1200.0);
    assert_eq!(loops[0].segments.len(), 4);
    // The hole's loop is one whole circle, clockwise seen from above, and
    // leads to the hole's cylinder.
    let [
        Segment::Arc {
            sweep_rad, across, ..
        },
    ] = &loops[1].segments[..]
    else {
        panic!("the hole's loop is not one arc");
    };
    assert_eq!(*sweep_rad, -2.0 * PI);
    assert_eq!(across.to_string(), "cylinder-187");
}

#[test]
fn coplanar_faces_with_one_normal_are_one_plane() {
    let part = read_part(P21_PLANES).unwrap();
    assert_eq!(part.planes.len(), 10);
    let tops: Vec<_> = part
        .planes
        .iter()
        .filter(|p| p.normal.to_array() == [0.0, 0.0, 1.0] && p.offset_mm == 20.0)
        .collect();
    assert_eq!(tops.len(), 1);
    assert_eq!(tops[0].faces.len(), 2);
    assert_eq!(tops[0].area_mm2, Some(20.0 * 40.0 + 16.0 * 40.0));
}

#[test]
fn every_face_is_in_exactly_one_primary_feature() {
    let part = read_part(P05_CBORE).unwrap();
    let mut faces: Vec<u64> = part
        .planes
        .iter()
        .flat_map(|p| p.faces.iter().map(|f| f.face))
        .chain(part.cylinders.iter().flat_map(|c| c.faces.iter().copied()))
        .chain(part.cones.iter().flat_map(|c| c.faces.iter().copied()))
        .chain(part.tori.iter().flat_map(|t| t.faces.iter().copied()))
        .chain(part.other_faces.iter().map(|o| o.face))
        .collect();
    let count = faces.len();
    faces.sort_unstable();
    faces.dedup();
    assert_eq!(faces.len(), count);
    assert_eq!(u32::try_from(count).unwrap(), part.face_counts.total);
    assert_eq!(part.cylinders.len(), 2);
    assert!(
        part.cylinders
            .iter()
            .all(|c| c.id.kind() == FeatureKind::Cylinder)
    );
}

#[test]
fn an_edge_on_an_unread_curve_leaves_the_outline_unknown() {
    // The edge between the top and the x = 0 side becomes a polyline:
    // both faces keep their planes, but neither outline can be stated.
    let unread = edit(
        P01_THROUGH,
        &[("#48 = LINE('',#49,#50);", "#48 = POLYLINE('',(#49,#25));")],
    );
    let part = read_part(&unread).unwrap();
    for face in [17, 88] {
        let plane = plane_of(&part, face);
        assert_eq!(plane.area_mm2, None);
        assert!(matches!(
            plane.faces[0].outline,
            Outline::Unknown {
                problem: OutlineProblem::CurveNotRead,
                instance: 47,
                ..
            }
        ));
    }
}

#[test]
fn a_circle_tilted_off_its_plane_leaves_the_outline_unknown() {
    // The hole's top circle is turned 1e-3 rad about its own x axis, which
    // keeps its one vertex on it and on every face: the gate passes it,
    // but the arc is not in the top's plane.
    let tilted = edit(
        P01_THROUGH,
        &[(
            "#116 = DIRECTION('',(0.,0.,-1.));",
            "#116 = DIRECTION('',(0.,1.E-03,-1.));",
        )],
    );
    let part = read_part(&tilted).unwrap();
    assert!(matches!(
        plane_of(&part, 88).faces[0].outline,
        Outline::Unknown {
            problem: OutlineProblem::CircleNotParallel,
            instance: 110,
            ..
        }
    ));
}

#[test]
fn a_stated_outer_bound_that_is_not_the_outer_loop_is_a_note() {
    let mislabelled = edit(
        P01_THROUGH,
        &[(
            "#107 = FACE_BOUND('',#108,.T.);",
            "#107 = FACE_OUTER_BOUND('',#108,.T.);",
        )],
    );
    let part = read_part(&mislabelled).unwrap();
    assert_eq!(part.notes.len(), 1);
    assert_eq!(part.notes[0].code, NoteCode::StatedOuterBoundDiffers);
    assert_eq!(part.notes[0].instance, 88);
    // The outline is still read, with the loop of positive area first.
    let Outline::Known { loops, .. } = &plane_of(&part, 88).faces[0].outline else {
        panic!("the outline is unknown");
    };
    assert_eq!(loops[0].bound, 89);
}

#[test]
fn a_document_whose_features_break_the_partition_is_not_read() {
    let document = FeaturesDocument::read(P01_THROUGH);
    let mut value: serde_json::Value = serde_json::from_str(&document.to_json()).unwrap();
    // The hole's face also listed as a second cylinder.
    let mut copy = value["part"]["cylinders"][0].clone();
    copy["id"] = "cylinder-188".into();
    copy["faces"] = serde_json::json!([188, 187]);
    value["part"]["cylinders"]
        .as_array_mut()
        .unwrap()
        .push(copy);
    let text = serde_json::to_string(&value).unwrap();
    assert!(matches!(
        FeaturesDocument::from_json(&text),
        Err(FormatError::Invalid(_))
    ));
    // And the round trip of the real one holds.
    assert_eq!(
        FeaturesDocument::from_json(&document.to_json()).unwrap(),
        document
    );
}
