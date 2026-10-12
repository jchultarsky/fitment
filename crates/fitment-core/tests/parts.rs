//! The committed unit set, read against its manifest.
//!
//! `tests/data` holds the parts `tools/gen-corpus.py --unit-set` writes and
//! the manifest with their answers, which come from the construction alone.
//! Core's tests do no file I/O, so each part is included here by name, and
//! the list is checked against the manifest.
//!
//! What is checked grows with the extractor: so far, whether each part is
//! accepted or refused, and with which reasons. A case refused by a gate
//! stage not built yet is skipped, and listed so the skip is visible.

use fitment_core::{FeaturesDocument, Status};
use serde_json::Value;

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
        }
    }
    assert_eq!(skipped, NOT_YET, "the cases waiting for a later gate stage");
}
