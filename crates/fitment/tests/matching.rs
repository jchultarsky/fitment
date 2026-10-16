//! The corpus's match cases (`docs/research/m4-match.md`, section 15):
//! every candidate the generator built, judged through the library (KH1)
//! against its source's socket and held to the strict gate (KH2, KH3)
//! under both policies M4 offers. Without the corpus this passes with a
//! note, unless `FITMENT_REQUIRE_CORPUS=1`.

#[path = "../../fitment-core/tests/support/gate.rs"]
mod gate;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use fitment_core::{
    Clearance, Grip, MatchOptions, MatchPolicy, Matcher, SocketDocument, TolerancePolicy,
};
use serde_json::Value;

fn corpus() -> Option<PathBuf> {
    let dir = std::env::var_os("FITMENT_CORPUS").map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/out"),
        PathBuf::from,
    );
    let required = std::env::var_os("FITMENT_REQUIRE_CORPUS").is_some_and(|v| v == "1");
    if dir.join("manifest.json").is_file() {
        return Some(dir);
    }
    assert!(!required, "FITMENT_REQUIRE_CORPUS=1 but there is no corpus");
    eprintln!("skipped: no corpus; run tools/gen-corpus.py corpus/out");
    None
}

/// KH1's library path, KH2 and KH3 over every match case of the corpus.
#[test]
fn every_match_case_meets_its_construction() {
    let Some(dir) = corpus() else {
        return;
    };
    let text = fs::read_to_string(dir.join("manifest.json")).unwrap();
    let manifest: Value = serde_json::from_str(&text).unwrap();
    let cases: Vec<&Value> = manifest["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["kind"] == "match")
        .collect();
    assert!(cases.len() > 20, "{} match cases", cases.len());
    let mut documents: BTreeMap<(String, String), SocketDocument> = BTreeMap::new();
    let mut problems = Vec::new();
    for case in cases {
        let source = case["socket"]["case"].as_str().unwrap().to_owned();
        let part = case["socket"]["part"].as_str().unwrap().to_owned();
        let document = documents
            .entry((source.clone(), part.clone()))
            .or_insert_with(|| {
                let bytes = fs::read(dir.join(format!("{source}.stp"))).unwrap();
                SocketDocument::extract(&bytes, Some(&part), None).unwrap()
            });
        let socket = &document.extracted().unwrap().sockets[0];
        let bytes = fs::read(dir.join(case["candidate"]["file"].as_str().unwrap())).unwrap();
        for (name, policy) in [
            ("default", MatchPolicy::default()),
            (
                "functional",
                MatchPolicy::new(
                    TolerancePolicy::default(),
                    Clearance::Functional,
                    Grip::Hard,
                ),
            ),
        ] {
            let matcher = Matcher::new(document, policy, MatchOptions::default()).unwrap();
            let result = matcher.judge_step(&bytes);
            problems.extend(gate::gate(case, name, socket, &result));
            if !case["defect"].as_array().is_none_or(Vec::is_empty) {
                let part = fitment_core::read_part(&bytes).unwrap();
                let at = gate::placement(case).unwrap();
                let built = matcher.verify_at(&part, &socket.id, &at).unwrap();
                problems.extend(gate::defect(case, socket, &built));
            }
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}
