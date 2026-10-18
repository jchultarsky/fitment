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

/// The committed assemblies' targets, as the socket goldens name them.
const TARGETS: [(&str, &str); 3] = [
    ("s01_bolted", "BRACKET"),
    ("s02_cbore", "BLOCK"),
    ("s03_dowels", "PLATE"),
];

/// VT50, CO5 (`docs/research/m6-api.md`): a component read and chosen by
/// core and judged by `judge_component` is `judge_part` on the part the
/// listing reads for that selection, under both policies; a `source`
/// naming another definition, or a file, is an internal refusal (a
/// fitment bug); a selection that names nothing is the selection's error;
/// the target a socket names reads as the component its part names.
/// Core's tests read no files, so the corpus's component cases run in
/// `crates/fitment/tests/matching.rs`.
#[test]
fn judge_component_is_judge_part_on_the_read_component() {
    for (case, part) in TARGETS {
        let bytes = assembly(case);
        let document = SocketDocument::extract(bytes, Some(part), None).unwrap();
        let component = fitment_core::SelectedComponent::read(bytes, Some(part), None).unwrap();
        let target = fitment_core::SelectedComponent::target(bytes, &document).unwrap();
        assert_eq!(component, target, "{case}");
        let read = fitment_core::read_assembly(bytes);
        let listing = read.listing().unwrap();
        let selection = listing.select(Some(part), None).unwrap();
        let listed = &listing.definitions()[selection.definition];
        let Some(fitment_core::ComponentRead::Accepted(by_listing)) = &listed.read else {
            panic!("{case}: the target is not read");
        };
        assert_eq!(component.definition(), listed.definition.instance);
        for (name, policy) in policies() {
            let matcher = Matcher::new(
                &document,
                policy,
                MatchOptions::default(),
                &fitment_core::NoFitVerifier,
            )
            .unwrap();
            let judged = matcher.judge_component(&component, component.source());
            let source = fitment_core::CandidateSource::component(
                fitment_core::sha256_hex(bytes),
                listed.definition.instance,
            );
            assert_eq!(
                judged,
                matcher.judge_part(by_listing, source),
                "{case} {name}"
            );
            let other = fitment_core::CandidateSource::component(
                component.assembly_sha256().to_owned(),
                component.definition() + 1,
            );
            for wrong in [other, fitment_core::CandidateSource::Given] {
                let refused = matcher.judge_component(&component, wrong);
                assert_eq!(refused.status, fitment_core::Status::Refused, "{case}");
                assert!(refused.is_fitment_bug(), "{case}: an internal refusal");
            }
        }
    }
    let bytes = assembly("s01_bolted");
    assert!(matches!(
        fitment_core::SelectedComponent::read(bytes, Some("NOPE"), None),
        Err(fitment_core::ComponentError::Select(
            fitment_core::SelectError::PartNotFound { .. }
        ))
    ));
    let other = SocketDocument::extract(assembly("s02_cbore"), Some("BLOCK"), None).unwrap();
    assert_eq!(
        fitment_core::SelectedComponent::target(bytes, &other),
        Err(fitment_core::ComponentError::NotFound),
        "another file's socket names no target of this one"
    );
}

/// VT37, CO3: a component named twice is one entry with both labels, in
/// order of appearance, and the document is the same bytes whatever order
/// the two come in but for that order of labels; its digest the same
/// either way, and the same as one naming it once (MF7).
#[test]
fn a_component_named_twice_is_one_entry() {
    let bytes = assembly("s01_bolted");
    let document = SocketDocument::extract(bytes, Some("BRACKET"), None).unwrap();
    let matcher = Matcher::new(
        &document,
        MatchPolicy::default(),
        MatchOptions::default(),
        &fitment_core::NoFitVerifier,
    )
    .unwrap();
    let component = fitment_core::SelectedComponent::read(bytes, Some("BRACKET"), None).unwrap();
    let file = CANDIDATES[2].1;
    let judged = matcher.judge_component(&component, component.source());
    let run = |labels: [&str; 2]| {
        let results = vec![
            (judged.clone(), vec![labels[0].to_owned()]),
            (matcher.judge_step(file), vec!["m_bracket.stp".to_owned()]),
            (judged.clone(), vec![labels[1].to_owned()]),
        ];
        fitment_core::MatchDocument::new(&matcher, results)
    };
    let (ab, ba) = (run(["label-a", "label-b"]), run(["label-b", "label-a"]));
    let doc: Value = serde_json::from_str(&ab.to_json()).unwrap();
    let entries = doc["candidates"].as_array().unwrap();
    assert_eq!(entries.len(), 2, "one entry per candidate");
    assert_eq!(doc["summary"]["candidates"], 2);
    let labels: Vec<&Value> = entries
        .iter()
        .filter(|e| e["source"]["kind"] == "component")
        .map(|e| &e["labels"])
        .collect();
    assert_eq!(labels, [&serde_json::json!(["label-a", "label-b"])]);
    assert_eq!(ab.digest(), ba.digest());
    let swapped = ba
        .to_json()
        .replace("label-b", "label-t")
        .replace("label-a", "label-b")
        .replace("label-t", "label-a");
    assert_eq!(
        ab.to_json(),
        swapped,
        "the same bytes but for the labels' order"
    );
    let once = fitment_core::MatchDocument::new(
        &matcher,
        vec![
            (judged.clone(), vec!["label-a".to_owned()]),
            (matcher.judge_step(file), vec!["m_bracket.stp".to_owned()]),
        ],
    );
    assert_eq!(once.digest(), ab.digest());
}

/// CO5 (review of M6 PR 6): components chosen from one reading of an
/// assembly are the components `read` chooses from its bytes, each
/// naming the bytes' SHA-256; a selection's error and a refused
/// assembly's are `read`'s too.
#[test]
fn components_chosen_from_one_reading_are_reads() {
    let bytes = assembly("s01_bolted");
    let once = fitment_core::AssemblyComponents::read(bytes);
    assert_eq!(once.sha256(), fitment_core::sha256_hex(bytes));
    for (part, occurrence) in [
        (Some("BRACKET"), None),
        (None, Some("#1")),
        (Some("NO SUCH PART"), None),
    ] {
        let chosen = fitment_core::SelectedComponent::select(&once, part, occurrence);
        let read = fitment_core::SelectedComponent::read(bytes, part, occurrence);
        assert_eq!(chosen, read, "{part:?} {occurrence:?}");
        if let Ok(component) = chosen {
            assert_eq!(component.assembly_sha256(), once.sha256());
        }
    }
    let refused = fitment_core::AssemblyComponents::read(b"no STEP file");
    assert_eq!(
        fitment_core::SelectedComponent::select(&refused, Some("BRACKET"), None),
        fitment_core::SelectedComponent::read(b"no STEP file", Some("BRACKET"), None)
    );
}
