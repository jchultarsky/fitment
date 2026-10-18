//! The committed candidates (`docs/research/m4-match.md`, KC15): 13 parts
//! against the committed s01, s02 and s03, judged through the library
//! (KH1) and held to the strict gate (KH2, KH3) under both policies M4
//! offers. CI regenerates the set and diffs it against this copy. The gate
//! is the corpus scorer's, included by path (`docs/research/m6-api.md`,
//! SC6): fitment-core has no dev-dependencies.

#[path = "../../fitment-corpus/src/gate.rs"]
#[allow(dead_code)] // the scorer's own uses of it are elsewhere
mod gate;

#[path = "../../fitment-corpus/src/outcome.rs"]
#[allow(dead_code)] // the scorer's own uses of it are elsewhere
mod outcome;

use fitment_core::{
    Clearance, Grip, MatchOptions, MatchPolicy, Matcher, SocketDocument, TolerancePolicy,
};
use serde_json::Value;

const MANIFEST: &str = include_str!("candidates/manifest.json");

const CANDIDATES: [(&str, &[u8]); 13] = [
    ("m_block02.stp", include_bytes!("candidates/m_block02.stp")),
    (
        "m_block02_cb_deep.stp",
        include_bytes!("candidates/m_block02_cb_deep.stp"),
    ),
    ("m_bracket.stp", include_bytes!("candidates/m_bracket.stp")),
    (
        "m_bracket_blind.stp",
        include_bytes!("candidates/m_bracket_blind.stp"),
    ),
    (
        "m_bracket_dia_small.stp",
        include_bytes!("candidates/m_bracket_dia_small.stp"),
    ),
    (
        "m_bracket_lug.stp",
        include_bytes!("candidates/m_bracket_lug.stp"),
    ),
    (
        "m_bracket_missing_hole.stp",
        include_bytes!("candidates/m_bracket_missing_hole.stp"),
    ),
    (
        "m_bracket_thick.stp",
        include_bytes!("candidates/m_bracket_thick.stp"),
    ),
    (
        "m_bracket_top_pocket.stp",
        include_bytes!("candidates/m_bracket_top_pocket.stp"),
    ),
    (
        "m_nut_pinned_15tol.stp",
        include_bytes!("candidates/m_nut_pinned_15tol.stp"),
    ),
    ("m_plate03.stp", include_bytes!("candidates/m_plate03.stp")),
    (
        "m_plate03_pin_shift_tol.stp",
        include_bytes!("candidates/m_plate03_pin_shift_tol.stp"),
    ),
    (
        "m_plate03_press_15tol.stp",
        include_bytes!("candidates/m_plate03_press_15tol.stp"),
    ),
];

fn assembly(case: &str) -> &'static [u8] {
    match case {
        "s01_bolted" => include_bytes!("assemblies/s01_bolted.stp"),
        "s02_cbore" => include_bytes!("assemblies/s02_cbore.stp"),
        "s03_dowels" => include_bytes!("assemblies/s03_dowels.stp"),
        other => panic!("no committed assembly {other}"),
    }
}

fn policies() -> [(&'static str, MatchPolicy); 2] {
    [
        ("default", MatchPolicy::default()),
        (
            "functional",
            MatchPolicy::new(
                TolerancePolicy::default(),
                Clearance::Functional,
                Grip::Hard,
            ),
        ),
    ]
}

/// KH1's library path, KH2 and KH3, on every committed candidate: the
/// verdict and the failures its construction states, under both policies.
#[test]
fn every_committed_candidate_meets_its_construction() {
    let manifest: Value = serde_json::from_str(MANIFEST).unwrap();
    let cases = manifest["cases"].as_array().unwrap();
    assert_eq!(cases.len(), CANDIDATES.len());
    let mut problems = Vec::new();
    for case in cases {
        let file = case["candidate"]["file"].as_str().unwrap();
        let bytes = CANDIDATES
            .iter()
            .find(|(name, _)| *name == file)
            .unwrap_or_else(|| panic!("{file} is not listed"))
            .1;
        let source = assembly(case["socket"]["case"].as_str().unwrap());
        let document =
            SocketDocument::extract(source, case["socket"]["part"].as_str(), None).unwrap();
        let sockets = &document.extracted().unwrap().sockets;
        for (name, policy) in policies() {
            let matcher = Matcher::new(
                &document,
                policy,
                MatchOptions::default(),
                &fitment_core::NoFitVerifier,
            )
            .unwrap();
            let result = matcher.judge_step(bytes);
            let outcome = outcome::Outcome::of_result(&result, matcher.policy());
            problems.extend(gate::gate(case, name, sockets, &outcome));
            if !case["defect"].as_array().is_none_or(Vec::is_empty) {
                let part = fitment_core::read_part(bytes).unwrap();
                let at = gate::placement(case).unwrap();
                let built: Vec<_> = sockets
                    .iter()
                    .filter_map(|s| matcher.verify_at(&part, &s.id, &at))
                    .collect();
                problems.extend(gate::defect(case, sockets, &built));
            }
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}
