//! The committed unit set, read against its manifest.
//!
//! `tests/data` holds the parts `tools/gen-corpus.py --unit-set` writes and
//! the manifest with their answers, which come from the construction alone.
//! Core's tests do no file I/O, so each part is included here by name, and
//! the list is checked against the manifest.
//!
//! What is checked grows with the extractor: so far, whether each part is
//! accepted or refused, and with which reasons; for an accepted part, its
//! planes, how many cones, tori and other faces it has, and that every face
//! is in exactly one primary feature. A case refused by a gate stage not
//! built yet is skipped, and listed so the skip is visible.

use fitment_core::{FeaturesDocument, Part, Status};
use serde_json::Value;

/// Lengths agree within this, in mm (design, section 13.2).
const LENGTH_MM: f64 = 1e-6;

/// Unit directions agree within this. The manifest rounds values to 9
/// decimals, which moves a unit vector by up to 9e-10.
const DIRECTION: f64 = 2e-9;

const MANIFEST: &str = include_str!("data/manifest.json");

macro_rules! parts {
    ($($name:literal),* $(,)?) => {
        &[$(($name, include_bytes!(concat!("data/", $name, ".stp")) as &[u8])),*]
    };
}

const PARTS: &[(&str, &[u8])] = parts![
    "p01_through",
    "p03_blind_flat",
    "p04_blind_drill",
    "p05_cbore",
    "p06_csink",
    "p09_pin",
    "p10_boss_hole",
    "p11_fillet_slot",
    "p12_edge_notch",
    "p14s_rotated_cbore",
    "p16_inch",
    "p20_split_surfaces",
    "p21_planes",
    "p22_coaxial_blind",
    "p25_angled",
    "p26_all_bspline",
    "p30_round_bore",
    "p32_torus_cbore",
    "r03_two_solids",
    "r04_assembly",
    "r05_no_length_unit",
    "r06_shell",
    "r12_flipped_cylinder",
    "u01_undercut_floor",
];

/// The gate stages built so far (`docs/research/m2-features.md`, 4.2).
const BUILT_STAGES: u64 = 6;

/// Refusals by stages not built yet: these parts are accepted for now.
const NOT_YET: &[&str] = &[];

fn cases() -> Vec<Value> {
    let manifest: Value = serde_json::from_str(MANIFEST).unwrap();
    assert_eq!(manifest["format"], "fitment.test-parts");
    assert_eq!(manifest["format_version"], 1);
    manifest["cases"].as_array().unwrap().clone()
}

#[test]
fn the_parts_listed_here_are_the_manifests() {
    let listed: Vec<&str> = PARTS.iter().map(|(name, _)| *name).collect();
    let manifest: Vec<String> = cases()
        .iter()
        .map(|case| {
            assert_eq!(case["set"], "unit", "{case}");
            assert_eq!(
                case["file"],
                format!("{}.stp", case["id"].as_str().unwrap())
            );
            case["id"].as_str().unwrap().to_owned()
        })
        .collect();
    assert_eq!(listed, manifest);
}

#[test]
fn each_part_is_accepted_or_refused_as_its_construction_says() {
    let mut skipped = Vec::new();
    for case in cases() {
        let name = case["id"].as_str().unwrap();
        let (_, bytes) = PARTS.iter().find(|(part, _)| *part == name).unwrap();
        let document = FeaturesDocument::read(bytes);
        let expect = &case["expect"];
        let refused_now =
            expect["result"] == "refused" && expect["stage"].as_u64().unwrap() <= BUILT_STAGES;
        if expect["result"] == "refused" && !refused_now {
            skipped.push(name.to_owned());
        }
        if refused_now {
            // The distinct codes, in order: a code repeats once per
            // instance at fault, and the manifest names each code once.
            let mut codes: Vec<String> = document
                .refusals()
                .iter()
                .map(|refusal| refusal.code.name().to_owned())
                .collect();
            codes.dedup();
            let expected: Vec<String> = expect["refusals"]
                .as_array()
                .unwrap()
                .iter()
                .map(|code| code.as_str().unwrap().to_owned())
                .collect();
            assert_eq!(document.status(), Status::Refused, "{name}");
            assert_eq!(codes, expected, "{name}");
        } else {
            assert_eq!(
                document.status(),
                Status::Accepted,
                "{name}: {:?}",
                document.refusals()
            );
            assert_eq!(
                FeaturesDocument::from_json(&document.to_json()).as_ref(),
                Ok(&document),
                "{name}: the document does not read back"
            );
            let part = document.part().unwrap();
            if let Some(features) = expect.get("features") {
                check_planes(name, part, features);
                for (key, found) in [
                    ("cones", part.cones.len()),
                    ("tori", part.tori.len()),
                    ("other_faces", part.other_faces.len()),
                ] {
                    assert_eq!(Some(found as u64), features[key].as_u64(), "{name}: {key}");
                }
            }
            check_partition(name, part);
        }
    }
    assert_eq!(skipped, NOT_YET, "the cases waiting for a later gate stage");
}

/// The planes match the manifest's one to one: normal, offset, face count
/// and shell.
fn check_planes(name: &str, part: &Part, features: &Value) {
    let expected = features["planes"].as_array().unwrap();
    let mut unmatched: Vec<_> = part.planes.iter().collect();
    for plane in expected {
        let normal: Vec<f64> = plane["normal"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c.as_f64().unwrap())
            .collect();
        let found = unmatched.iter().position(|p| {
            let n = p.normal.to_array();
            let off: f64 = n.iter().zip(&normal).map(|(a, b)| (a - b) * (a - b)).sum();
            off.sqrt() <= DIRECTION
                && (p.offset_mm - plane["offset"].as_f64().unwrap()).abs() <= LENGTH_MM
                && p.faces.len() as u64 == plane["faces"].as_u64().unwrap()
                && u64::from(p.shell) == plane["shell"].as_u64().unwrap()
        });
        assert!(found.is_some(), "{name}: no plane {plane}");
        unmatched.remove(found.unwrap());
    }
    assert!(
        unmatched.is_empty(),
        "{name}: planes the construction has not: {:?}",
        unmatched
            .iter()
            .map(|p| p.id.to_string())
            .collect::<Vec<_>>()
    );
}

/// Every face is in exactly one primary feature, and the features hold
/// as many faces as the solid has.
fn check_partition(name: &str, part: &Part) {
    let mut faces: Vec<u64> = Vec::new();
    for plane in &part.planes {
        faces.extend(plane.faces.iter().map(|f| f.face));
    }
    for cylinder in &part.cylinders {
        faces.extend(&cylinder.faces);
    }
    for cone in &part.cones {
        faces.extend(&cone.faces);
    }
    for torus in &part.tori {
        faces.extend(&torus.faces);
    }
    faces.extend(part.other_faces.iter().map(|f| f.face));
    let count = faces.len();
    faces.sort_unstable();
    faces.dedup();
    assert_eq!(faces.len(), count, "{name}: a face is in two features");
    assert_eq!(
        count as u64,
        u64::from(part.face_counts.total),
        "{name}: the features hold {count} faces"
    );
    assert_eq!(part.face_counts.total, part.solids[0].faces, "{name}");
}
