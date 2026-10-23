//! VT54 (`docs/research/m7-fit.md`, section 6): the zones of the committed
//! assemblies' sockets equal their construction's (FZ1 to FZ8): each
//! seat's slab is its neighbour's cited face where the construction puts
//! it, each tight span's annulus has its stations and the two radii, a
//! clearance span has none, and a requirement removed in review keeps its
//! zones. s05's copies and s18's cones are the corpus's
//! (`crates/fitment/tests/fit.rs`).

use fitment_core::fit::{FitScene, Zone, ZoneKind, ZoneShape, zones};
use fitment_core::{ReviewStatus, SocketDocument};

const S01: &[u8] = include_bytes!("assemblies/s01_bolted.stp");
const S02: &[u8] = include_bytes!("assemblies/s02_cbore.stp");
const S03: &[u8] = include_bytes!("assemblies/s03_dowels.stp");

/// s01's bolt and nut positions (the construction's `pts`).
const PTS: [[f64; 2]; 4] = [[-30.0, -15.0], [30.0, -15.0], [-30.0, 15.0], [30.0, 15.0]];

fn zones_of(bytes: &[u8], part: &str) -> (FitScene, Vec<Zone>) {
    let document = SocketDocument::extract(bytes, Some(part), None).unwrap();
    let scene = FitScene::of(bytes, &document).unwrap();
    let zones = zones(&document, &scene).as_slice().to_vec();
    (scene, zones)
}

/// The product of the zone's body.
fn product(scene: &FitScene, zone: &Zone) -> String {
    let label = scene.body(&zone.body).unwrap().label();
    label.split(' ').next().unwrap().to_owned()
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-9
}

/// A slab's normal, plane, centroid and box, and the cited face's area as
/// M2 states it.
fn slab(scene: &FitScene, zone: &Zone) -> ([f64; 3], f64, [f64; 3], [[f64; 3]; 2]) {
    let ZoneShape::Slab {
        face,
        normal,
        offset_mm,
        area_mm2,
        centroid_mm,
        face_box,
        ..
    } = &zone.shape
    else {
        unreachable!("{zone:?}");
    };
    let body = scene.body(&zone.body).unwrap();
    let stated = scene
        .part(body)
        .unwrap()
        .planes
        .iter()
        .flat_map(|p| &p.faces)
        .find(|f| f.face == *face)
        .and_then(|f| f.area_mm2)
        .unwrap();
    assert!(
        (area_mm2 - stated).abs() <= 1e-9 * stated,
        "{area_mm2} {stated}"
    );
    (
        normal.to_array(),
        *offset_mm,
        centroid_mm.to_array(),
        [face_box.min_mm, face_box.max_mm],
    )
}

/// An annulus's axis point and direction, stations and radii.
fn annulus(zone: &Zone) -> ([f64; 3], [f64; 3], [f64; 2], [f64; 2]) {
    let ZoneShape::Annulus {
        axis,
        from_mm,
        to_mm,
        inner_mm,
        outer_mm,
        ..
    } = &zone.shape
    else {
        unreachable!("{zone:?}");
    };
    (
        axis.point_mm.to_array(),
        axis.direction.to_array(),
        [*from_mm, *to_mm],
        [*inner_mm, *outer_mm],
    )
}

/// FZ2: the bracket's zones are the plate's top face (z 0, the plate's
/// 100 × 60 outline) and each bolt head's underside (z 8, at its point),
/// only for that neighbour; its clearance holes declare none (FZ8).
#[test]
fn the_bracket_sits_in_the_plate_s_and_the_heads_slabs() {
    let (scene, zones) = zones_of(S01, "BRACKET");
    assert_eq!(zones.len(), 5);
    assert!(zones.iter().all(|z| z.kind == ZoneKind::Slab && !z.copy));
    let plate: Vec<&Zone> = zones
        .iter()
        .filter(|z| product(&scene, z) == "PLATE")
        .collect();
    assert_eq!(plate.len(), 1);
    let (n, offset, centroid, bounds) = slab(&scene, plate[0]);
    assert_eq!(n, [0.0, 0.0, 1.0]);
    assert!(close(offset, 0.0));
    assert!(close(centroid[0], 0.0) && close(centroid[1], 0.0) && close(centroid[2], 0.0));
    assert_eq!(bounds, [[-50.0, -30.0, 0.0], [50.0, 30.0, 0.0]]);
    let mut heads: Vec<[f64; 3]> = zones
        .iter()
        .filter(|z| product(&scene, z) == "BOLT")
        .map(|z| {
            let (n, offset, centroid, _) = slab(&scene, z);
            assert_eq!(n, [0.0, 0.0, -1.0]);
            assert!(close(offset, -8.0));
            centroid
        })
        .collect();
    heads.sort_by(|a, b| a[1].total_cmp(&b[1]).then(a[0].total_cmp(&b[0])));
    for (head, [x, y]) in heads.iter().zip(PTS) {
        assert!(
            close(head[0], x) && close(head[1], y) && close(head[2], 8.0),
            "{head:?}"
        );
    }
}

/// FZ3: the bolt's thread in its nut is an annulus from the nut's minor
/// radius (2.5) to the bolt's (3), over the nut's 5 mm from z −10 down
/// (stations 18 to 23 from the head's face, each widened by the contact
/// tolerance); its clearance through the plate and the bracket declares
/// none; its head sits in the bracket's top face.
#[test]
fn the_bolt_s_thread_is_an_annulus_in_its_nut() {
    let (scene, zones) = zones_of(S01, "BOLT");
    assert_eq!(zones.len(), 8, "4 occurrences: a slab and an annulus each");
    for zone in &zones {
        match zone.kind {
            ZoneKind::Slab => {
                assert_eq!(product(&scene, zone), "BRACKET");
                let (n, offset, _, bounds) = slab(&scene, zone);
                assert_eq!(n, [0.0, 0.0, 1.0]);
                assert!(close(offset, 8.0));
                assert_eq!(bounds, [[-40.0, -25.0, 8.0], [40.0, 25.0, 8.0]]);
            }
            ZoneKind::Annulus => {
                assert_eq!(product(&scene, zone), "NUT");
                let (point, direction, stations, radii) = annulus(zone);
                assert!(
                    PTS.iter()
                        .any(|[x, y]| close(point[0], *x) && close(point[1], *y))
                );
                assert!(close(point[2], 8.0));
                assert_eq!(direction, [0.0, 0.0, -1.0]);
                assert!(
                    close(stations[0], 17.99) && close(stations[1], 23.01),
                    "{stations:?}"
                );
                assert!(close(radii[0], 2.5) && close(radii[1], 3.0), "{radii:?}");
            }
            _ => unreachable!("{zone:?}"),
        }
    }
}

/// FZ3 from the bore's side: the nut's hole holds the bolt over its own
/// 5 mm; its seat is the plate's underside.
#[test]
fn the_nut_holds_the_bolt_over_its_height() {
    let (scene, zones) = zones_of(S01, "NUT");
    assert_eq!(zones.len(), 8);
    for zone in zones.iter().filter(|z| z.kind == ZoneKind::Annulus) {
        assert_eq!(product(&scene, zone), "BOLT");
        let (point, direction, stations, radii) = annulus(zone);
        assert!(close(point[2], -15.0));
        assert_eq!(direction, [0.0, 0.0, 1.0]);
        assert!(
            close(stations[0], 0.0) && close(stations[1], 5.0),
            "{stations:?}"
        );
        assert!(close(radii[0], 2.5) && close(radii[1], 3.0));
    }
    for zone in zones.iter().filter(|z| z.kind == ZoneKind::Slab) {
        assert_eq!(product(&scene, zone), "PLATE");
        let (n, offset, _, _) = slab(&scene, zone);
        assert_eq!(n, [0.0, 0.0, -1.0]);
        assert!(close(offset, 10.0));
    }
}

/// s02: the block sits on the base's top and under the screw's head (a
/// dia 10 head, a dia 6 shank: an annular face at z 8.5); the screw's
/// thread is an annulus in the base's dia 5 hole, and its clearance
/// through the block declares none.
#[test]
fn the_counterbored_block_and_its_screw() {
    let (scene, zones) = zones_of(S02, "BLOCK");
    assert_eq!(zones.len(), 2);
    let base = zones.iter().find(|z| product(&scene, z) == "BASE").unwrap();
    let (n, offset, _, bounds) = slab(&scene, base);
    assert_eq!(n, [0.0, 0.0, 1.0]);
    assert!(close(offset, 0.0));
    assert_eq!(bounds, [[-30.0, -30.0, 0.0], [30.0, 30.0, 0.0]]);
    let head = zones
        .iter()
        .find(|z| product(&scene, z) == "SCREW")
        .unwrap();
    let (n, offset, centroid, bounds) = slab(&scene, head);
    assert_eq!(n, [0.0, 0.0, -1.0]);
    assert!(close(offset, -8.5) && close(centroid[2], 8.5));
    assert_eq!(bounds, [[-5.0, -5.0, 8.5], [5.0, 5.0, 8.5]]);
    let (scene, zones) = zones_of(S02, "SCREW");
    let thread: Vec<&Zone> = zones
        .iter()
        .filter(|z| z.kind == ZoneKind::Annulus)
        .collect();
    assert_eq!(thread.len(), 1, "the block's clearance declares none");
    assert_eq!(product(&scene, thread[0]), "BASE");
    let (_, _, _, radii) = annulus(thread[0]);
    assert!(close(radii[0], 2.5) && close(radii[1], 3.0));
}

/// s03: the plate's press-fit hole (dia 5.97) holds its dowel in an
/// annulus from 2.985 to 3, the line-to-line ones (dia 6 and 6.015) from
/// 3 to 3 and 3.0075, over the plate's 8 mm; the clearance hole (dia 6.2)
/// declares none.
#[test]
fn the_plate_s_dowel_holes_by_their_fits() {
    let (scene, zones) = zones_of(S03, "PLATE");
    let mut rings: Vec<([f64; 3], [f64; 2], [f64; 2])> = zones
        .iter()
        .filter(|z| z.kind == ZoneKind::Annulus)
        .map(|z| {
            assert_eq!(product(&scene, z), "DOWEL");
            let (point, _, stations, radii) = annulus(z);
            (point, stations, radii)
        })
        .collect();
    rings.sort_by(|a, b| a.0[0].total_cmp(&b.0[0]));
    let expected = [
        (-30.0, [2.985, 3.0]),
        (10.0, [3.0, 3.0]),
        (30.0, [3.0, 3.0075]),
    ];
    assert_eq!(rings.len(), expected.len(), "{rings:?}");
    for ((point, stations, radii), (x, r)) in rings.iter().zip(expected) {
        assert!(close(point[0], x), "{point:?}");
        assert!(
            close(stations[0], 0.0) && close(stations[1], 8.01),
            "{stations:?}"
        );
        assert!(
            close(radii[0], r[0]) && close(radii[1], r[1]),
            "{x}: {radii:?}"
        );
    }
}

/// FZ1: a requirement removed in review keeps its zones: a removal waives
/// what a candidate must satisfy, not the contact the assembly makes.
#[test]
fn a_removed_requirement_keeps_its_zones() {
    let mut document = SocketDocument::extract(S01, Some("BRACKET"), None).unwrap();
    let scene = FitScene::of(S01, &document).unwrap();
    let before = zones(&document, &scene);
    let seat = before
        .as_slice()
        .iter()
        .find(|z| product(&scene, z) == "PLATE")
        .unwrap()
        .requirement
        .clone();
    document
        .decide(
            &seat,
            None,
            ReviewStatus::Removed,
            Some("not in this review"),
        )
        .unwrap();
    assert_eq!(
        document.active("socket-1").filter(|r| r.id == seat).count(),
        0
    );
    assert_eq!(zones(&document, &scene), before);
}

/// FZ6, FT7: at the pair's depth ε = t − `u_pair` the plate's slab is the
/// face swept ε either side: its volume the area × 2ε, its box the face's
/// widened by ε along z; at ε + t + `u_pair`, the proximity zone.
#[test]
fn a_slab_at_the_pair_s_depth() {
    let (_, zones) = zones_of(S01, "BRACKET");
    let values = fitment_core::fit::PairValues::of(0.01, [1e-7, 1e-7]);
    let plate = zones
        .iter()
        .find(|z| matches!(&z.shape, ZoneShape::Slab { offset_mm, .. } if *offset_mm == 0.0))
        .unwrap();
    let ZoneShape::Slab { area_mm2, .. } = plate.shape else {
        unreachable!()
    };
    let built = plate.at(values.epsilon_mm).unwrap();
    assert!((built.volume_mm3 - area_mm2 * 2.0 * 0.009_999).abs() < 1e-9);
    assert!(close(built.bounds.min_mm[2], -0.009_999) && close(built.bounds.max_mm[2], 0.009_999));
    let wide = plate.at(values.proximity_depth_mm()).unwrap();
    assert!(close(wide.bounds.max_mm[2], 0.02));
}
