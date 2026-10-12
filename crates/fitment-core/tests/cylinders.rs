//! Cylinders: grouping, sense and whether they wrap around their axis
//! (`docs/research/m2-features.md`, section 8), on the committed unit parts
//! and edits of them. Inputs are included at compile time: core's tests do
//! no file I/O. The generated corpus checks every class against its
//! construction (`support/expect.rs`); these name the cases one by one.

// The values compared exactly are stated in the files, or exact in binary.
#![allow(clippy::float_cmp)]

use fitment_core::{Cylinder, CylinderClass, End, NoteCode, Part, Sense, read_part};

const P01_THROUGH: &[u8] = include_bytes!("data/p01_through.stp");
const P11_FILLET_SLOT: &[u8] = include_bytes!("data/p11_fillet_slot.stp");
const P20_SPLIT_SURFACES: &[u8] = include_bytes!("data/p20_split_surfaces.stp");
const P25_ANGLED: &[u8] = include_bytes!("data/p25_angled.stp");

/// A D-shaped prism whose cylinder face spans 0.6 rad, bounded by lines
/// written as B-splines, with a slit along the axis that its loop uses
/// twice, as a seam would be. Written for the review of PR 5.
const SLIT_STRIP: &[u8] = include_bytes!("edits/slit_strip.stp");

/// A round bar of radius 5 cut by the plane z = 5 − x: its wall rises from
/// a full circle at z = 0 to an ellipse whose one vertex touches it.
/// Written for the review of PR 5.
const HOOF: &[u8] = include_bytes!("edits/hoof.stp");

/// A cylinder whose bottom circle is two edges between two vertices at one
/// point: a whole turn and an arc of no length. Written for the review of
/// PR 5.
const ZERO_ARC: &[u8] = include_bytes!("edits/zero_arc.stp");

/// `input` with each `from` replaced by its `to`; each must occur once.
fn edit(input: &[u8], edits: &[(&str, &str)]) -> Vec<u8> {
    let mut text = std::str::from_utf8(input).unwrap().to_owned();
    for (from, to) in edits {
        assert_eq!(text.matches(from).count(), 1, "{from}");
        text = text.replacen(from, to, 1);
    }
    text.into_bytes()
}

fn cylinder(part: &Part, id: &str) -> Cylinder {
    part.cylinders
        .iter()
        .find(|c| c.id.to_string() == id)
        .unwrap_or_else(|| panic!("no {id}"))
        .clone()
}

#[test]
fn a_through_hole_is_full_between_its_two_end_circles() {
    let part = read_part(P01_THROUGH).unwrap();
    let hole = cylinder(&part, "cylinder-187");
    assert_eq!(hole.sense, Sense::Hole);
    assert_eq!(hole.class, read_class("{\"kind\": \"full\"}"));
    // The axis has the canonical sign and starts at the lowest station.
    assert_eq!(hole.axis.direction.to_array(), [0.0, 0.0, 1.0]);
    assert_eq!(hole.axis.point_mm.to_array(), [20.0, 15.0, 0.0]);
    assert_eq!(hole.stations_mm, [0.0, 8.0]);
    assert_eq!(hole.wall_openings, 0);
}

fn read_class(json: &str) -> CylinderClass {
    serde_json::from_str(json).unwrap()
}

#[test]
fn a_fillet_and_slot_ends_are_partial_with_their_sweeps() {
    let part = read_part(P11_FILLET_SLOT).unwrap();
    let mut sweeps: Vec<f64> = part
        .cylinders
        .iter()
        .filter_map(|c| match c.class {
            CylinderClass::Partial { sweep_rad, .. } => sweep_rad,
            _ => None,
        })
        .map(f64::to_degrees)
        .collect();
    sweeps.sort_by(f64::total_cmp);
    assert_eq!(sweeps.len(), 3);
    assert!((sweeps[0] - 90.0).abs() < 1e-9, "{sweeps:?}");
    assert!((sweeps[1] - 180.0).abs() < 1e-9, "{sweeps:?}");
    assert!((sweeps[2] - 180.0).abs() < 1e-9, "{sweeps:?}");
}

#[test]
fn a_hole_with_no_end_circle_is_undecided_whatever_its_seam() {
    // A Ø6 hole tilted 30°: both ends are ellipses, which no rule reads. Its
    // seam is used twice by one loop, but so is a slit, and without the
    // curves on the surface the seam proves no wrap (review of PR 5).
    let part = read_part(P25_ANGLED).unwrap();
    let [hole] = &part.cylinders[..] else {
        panic!("p25 has {} cylinders", part.cylinders.len());
    };
    assert_eq!(
        hole.class,
        read_class("{\"kind\": \"undecided\", \"reason\": \"no_end_circle\"}")
    );
    assert_eq!(hole.stations_mm, Vec::<f64>::new());
    assert_eq!(hole.sense, Sense::Hole);
}

#[test]
fn split_halves_on_one_surface_are_one_full_cylinder() {
    let part = read_part(P20_SPLIT_SURFACES).unwrap();
    let counterbore = cylinder(&part, "cylinder-205");
    assert_eq!(counterbore.faces, [205, 236]);
    assert_eq!(counterbore.class, read_class("{\"kind\": \"full\"}"));
    assert_eq!(part.notes, []);
}

#[test]
fn halves_apart_by_less_than_the_uncertainty_are_a_near_miss() {
    // One half of p20's counterbore 0.5 µm wider, in a file that states
    // 0.01 mm: within the uncertainty, but not the same surface stated
    // twice: two cylinders, neither full, and a note on each edge they
    // share. The wider half's arcs and lines lie 0.5 µm off its own
    // surface, so no rule reads them: it is undecided.
    let near = edit(
        P20_SPLIT_SURFACES,
        &[
            (
                "#249 = CYLINDRICAL_SURFACE('',#326,5.5);",
                "#249 = CYLINDRICAL_SURFACE('',#326,5.5005);",
            ),
            ("LENGTH_MEASURE(5.E-07)", "LENGTH_MEASURE(1.E-02)"),
        ],
    );
    let part = read_part(&near).unwrap();
    let halves: Vec<&Cylinder> = part
        .cylinders
        .iter()
        .filter(|c| c.radius_mm > 5.0)
        .collect();
    assert_eq!(halves.len(), 2);
    for half in &halves {
        assert!(!half.class.is_full(), "{:?}", half.class);
        assert_eq!(half.near_misses.len(), 1);
    }
    assert_eq!(halves[0].near_misses[0], halves[1].id);
    assert_eq!(part.notes.len(), 2);
    assert!(part.notes.iter().all(|n| n.code == NoteCode::NearMiss));
}

#[test]
fn an_irregular_end_names_its_side() {
    // The serialized form of a class with one irregular end.
    let class = read_class("{\"kind\": \"full_one_end_irregular\", \"irregular\": \"finish\"}");
    assert!(class.is_full());
    assert!(matches!(
        class,
        CylinderClass::FullOneEndIrregular {
            irregular: End::Finish,
            ..
        }
    ));
}

#[test]
fn a_slit_used_twice_by_one_loop_proves_no_wrap() {
    let part = read_part(SLIT_STRIP).unwrap();
    let [strip] = &part.cylinders[..] else {
        panic!("{} cylinders", part.cylinders.len());
    };
    assert!(!strip.class.is_full(), "{:?}", strip.class);
}

#[test]
fn a_wall_past_its_one_station_is_irregular_there_however_its_curve_ends() {
    let part = read_part(HOOF).unwrap();
    let [bar] = &part.cylinders[..] else {
        panic!("{} cylinders", part.cylinders.len());
    };
    assert_eq!(bar.sense, Sense::Shaft);
    assert_eq!(bar.axis.direction.to_array(), [0.0, 0.0, 1.0]);
    assert_eq!(
        bar.class,
        read_class("{\"kind\": \"full_one_end_irregular\", \"irregular\": \"finish\"}")
    );
}

#[test]
fn an_arc_of_no_length_leaves_its_station_not_full_and_is_no_contradiction() {
    let part = read_part(ZERO_ARC).unwrap();
    assert!(part.cylinders.iter().all(|c| !c.class.is_full()));
}
