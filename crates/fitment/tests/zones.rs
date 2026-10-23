//! VT54's corpus half (`docs/research/m7-fit.md`, FZ4, FZ5): s05's
//! washers, each seated on the other, zone their copies with the other's
//! face; s18's countersunk screw and plate zone each other with the 90°
//! cone's shell from the hole's radius to the countersink's rim. The
//! committed assemblies' zones are fitment-core's `tests/zones.rs`.
//! Without the corpus these pass with a note, unless
//! `FITMENT_REQUIRE_CORPUS=1`.

use std::f64::consts::PI;
use std::fs;
use std::path::Path;

use fitment_core::SocketDocument;
use fitment_core::fit::{FitScene, PairValues, Zone, ZoneKind, ZoneShape, zones};
use fitment_corpus::Corpus;

fn zones_of(dir: &Path, case: &str, part: &str) -> (FitScene, Vec<Zone>) {
    let bytes = fs::read(dir.join(format!("{case}.stp"))).unwrap();
    let document = SocketDocument::extract(&bytes, Some(part), None).unwrap();
    let scene = FitScene::of(&bytes, &document).unwrap();
    let zones = zones(&document, &scene).as_slice().to_vec();
    (scene, zones)
}

fn product(scene: &FitScene, zone: &Zone) -> String {
    let label = scene.body(&zone.body).unwrap().label();
    label.split(' ').next().unwrap().to_owned()
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-9
}

/// FZ5: s05's two washers (z 24 to 25.6 and 25.6 to 27.2) each zone the
/// other, a copy, by the other's face on their shared plane: the upper's
/// underside for the lower, the lower's top for the upper; each in its own
/// socket, beside the plate's top and the bolt head's underside.
#[test]
fn each_washer_zones_its_copy_by_the_other_s_face() {
    let Some(corpus) = Corpus::locate() else {
        return;
    };
    let (scene, zones) = zones_of(corpus.dir(), "s05_stack", "WASHER");
    assert_eq!(zones.len(), 4);
    let copies: Vec<&Zone> = zones.iter().filter(|z| z.copy).collect();
    assert_eq!(copies.len(), 2);
    for zone in &copies {
        assert_eq!(zone.kind, ZoneKind::Slab);
        assert_ne!(zone.body, zone.occurrence);
        assert!(scene.body(&zone.body).unwrap().selected);
        let ZoneShape::Slab {
            normal, offset_mm, ..
        } = &zone.shape
        else {
            unreachable!()
        };
        let n = normal.to_array()[2];
        assert!(close(n.abs(), 1.0));
        assert!(close(offset_mm * n, 25.6), "{offset_mm} {n}");
        let theirs = scene.body(&zone.occurrence).unwrap();
        let lower = theirs.transform.translation()[2] < 25.0;
        assert_eq!(
            n < 0.0,
            lower,
            "the lower washer meets the upper's underside"
        );
    }
    assert_ne!(copies[0].socket, copies[1].socket);
    let others: Vec<String> = zones
        .iter()
        .filter(|z| !z.copy)
        .map(|z| product(&scene, z))
        .collect();
    assert_eq!(others, ["T", "BOLT"]);
}

/// FZ4: s18's 90° countersink: the shell's cone has its apex 3.8 mm up
/// the axis, opens upward at 45° and covers the radii 3.3 (the dia 6.6
/// hole) to 6.2 (the dia 12.4 rim at the plate's top, z 10); the same
/// shell zones the plate from the screw and the screw from the plate. At
/// the pair's ε its volume is Pappus's.
#[test]
fn the_countersink_s_shell_is_the_cone_s() {
    let Some(corpus) = Corpus::locate() else {
        return;
    };
    let shells = |part: &str| -> Vec<(String, Zone)> {
        let (scene, zones) = zones_of(corpus.dir(), "s18_csink", part);
        zones
            .into_iter()
            .filter(|z| z.kind == ZoneKind::Shell)
            .map(|z| (product(&scene, &z), z))
            .collect()
    };
    let (screw, plate) = (shells("SCREW"), shells("PLATE"));
    assert_eq!(screw.len(), 1);
    assert_eq!(plate.len(), 1);
    assert_eq!(screw[0].0, "PLATE");
    assert_eq!(plate[0].0, "SCREW");
    for (_, zone) in [&screw[0], &plate[0]] {
        let ZoneShape::Shell {
            apex_mm,
            direction,
            half_angle_rad,
            radii_mm,
            ..
        } = &zone.shape
        else {
            unreachable!()
        };
        let apex = apex_mm.to_array();
        assert!(
            close(apex[0], 0.0) && close(apex[1], 0.0) && close(apex[2], 3.8),
            "{apex:?}"
        );
        assert_eq!(direction.to_array(), [0.0, 0.0, 1.0]);
        assert!(close(*half_angle_rad, PI / 4.0));
        assert!(
            close(radii_mm[0], 3.3) && close(radii_mm[1], 6.2),
            "{radii_mm:?}"
        );
        assert!(close(apex[2] + radii_mm[1] / half_angle_rad.tan(), 10.0));
        let d = PairValues::of(0.01, [1e-7]).epsilon_mm;
        let built = zone.at(d).unwrap();
        let (low, high) = (3.3 - d, 6.2 + d);
        let pappus = 2.0 * PI * (low + high) / 2.0 * ((high - low) * 2.0_f64.sqrt()) * 2.0 * d;
        assert!(
            (built.volume_mm3 - pappus).abs() <= 1e-9 * pappus,
            "{built:?}"
        );
        assert!(close(built.centroid_mm[0], 0.0) && close(built.centroid_mm[1], 0.0));
    }
}
