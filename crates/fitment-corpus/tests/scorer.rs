//! The scorer's own rules (`docs/research/m6-api.md`, section 9.2) on the
//! committed s01 and its candidates: an outcome reads the same from the
//! library and from a document (SC2), a locator names exactly one
//! requirement or the gate reports a harness error (SC4), a missing corpus
//! passes with a note unless required (SC7), a waived case is neither a fit
//! nor a false positive (SC8), and entries are found by content (SC9).

use std::path::Path;

use fitment_core::{MatchDocument, MatchOptions, Matcher, NoFitVerifier, SocketDocument};
use fitment_corpus::{Corpus, Outcome, Policy, Run, RunPath, Score, Truth, gate, parity};
use serde_json::{Value, json};

const S01: &[u8] = include_bytes!("../../fitment-core/tests/assemblies/s01_bolted.stp");
const MISSING_HOLE: &[u8] =
    include_bytes!("../../fitment-core/tests/candidates/m_bracket_missing_hole.stp");
const BRACKET: &[u8] = include_bytes!("../../fitment-core/tests/candidates/m_bracket.stp");

/// The bore at (30, 15) of s01's bracket, as a locator (KC5).
fn hole() -> Value {
    json!({"kind": "bore", "axis_point": [30.0, 15.0, 0.0], "axis_direction": [0.0, 0.0, 1.0]})
}

/// A case of s01's bracket with the candidate `file`, its truth and class,
/// its review and what it expects under both policies.
fn case(id: &str, truth: &str, class: &str, review: &[Value], expect: &Value) -> Value {
    json!({
        "id": id, "kind": "match", "file": "m_bracket_missing_hole.stp", "class": class, "truth": truth,
        "socket": {"case": "s01_bolted", "part": "BRACKET", "occurrence": null, "review": review},
        "candidate": {"file": "m_bracket_missing_hole.stp"},
        "expect": {"default": expect, "functional": "default"}, "defect": [],
    })
}

/// The waived case: the bracket without its hole at (30, 15), that hole's
/// bore removed in review (M6-38).
fn waived() -> Value {
    let mut removed = hole();
    removed["status"] = json!("removed");
    removed["note"] = json!("the bolt at (30, 15) is left out");
    case(
        "waived",
        "waived",
        "review_waived",
        &[removed],
        &json!({"verdict": "interface_match", "must_fail": [], "may_fail": [],
                "flags": ["requirements_removed"]}),
    )
}

/// `case`'s socket, extracted and reviewed, and its sockets as extracted.
fn socket(case: &Value) -> (SocketDocument, Vec<fitment_core::Socket>) {
    let mut document = SocketDocument::extract(S01, Some("BRACKET"), None).unwrap();
    let sockets = document.extracted().unwrap().sockets.clone();
    gate::apply_review(case, &mut document).unwrap();
    (document, sockets)
}

/// SC2: the library's result and the document's entry for it give equal
/// outcomes, a review's removals among them.
#[test]
fn an_outcome_reads_as_its_entry() {
    let (document, _) = socket(&waived());
    for policy in Policy::M4 {
        let matcher = Matcher::new(
            &document,
            policy.match_policy(),
            MatchOptions::default(),
            &NoFitVerifier,
        )
        .unwrap();
        let results: Vec<_> = [MISSING_HOLE, BRACKET]
            .iter()
            .map(|b| matcher.judge_step(b))
            .collect();
        let doc = MatchDocument::new(
            &matcher,
            results
                .iter()
                .map(|r| (r.clone(), vec!["x".to_owned()]))
                .collect(),
        );
        let doc: Value = serde_json::from_str(&doc.to_json()).unwrap();
        for (bytes, result) in [MISSING_HOLE, BRACKET].iter().zip(&results) {
            let library = Outcome::of_result(result, matcher.policy());
            let read = Outcome::find(&doc, &fitment_core::sha256_hex(bytes)).unwrap();
            assert_eq!(read, library);
            assert_eq!(library.sockets[0].removed, ["bore:hole-722"]);
            assert_eq!(library.verdict, "interface_match");
        }
    }
}

/// SC4: a locator that names no requirement, or several, is a harness
/// error, never a pass.
#[test]
fn a_locator_naming_no_requirement_or_several_is_a_harness_error() {
    let (_, sockets) = socket(&waived());
    let none =
        json!({"kind": "bore", "axis_point": [99.0, 99.0, 0.0], "axis_direction": [0.0, 0.0, 1.0]});
    let missing = gate::locate(&sockets[0], &none).unwrap_err();
    assert!(missing.contains("names no requirement"), "{missing}");
    let mut twice = sockets[0].clone();
    let mut copy = twice
        .requirements
        .iter()
        .find(|r| r.id == "bore:hole-722")
        .unwrap()
        .clone();
    copy.id = "bore:copy".to_owned();
    twice.requirements.push(copy);
    let several = gate::locate(&twice, &hole()).unwrap_err();
    assert!(several.contains("names several"), "{several}");
    // Through the gate: an expectation naming nothing fails as a harness
    // error whatever the outcome.
    let mut expect_none = none.clone();
    expect_none["codes"] = json!([]);
    let bad = case(
        "bad",
        "does_not_fit",
        "b",
        &[],
        &json!({"verdict": "reject", "must_fail": [expect_none], "may_fail": []}),
    );
    let (document, _) = socket(&bad);
    let matcher = Matcher::new(
        &document,
        Policy::Default.match_policy(),
        MatchOptions::default(),
        &NoFitVerifier,
    )
    .unwrap();
    let outcome = Outcome::of_result(&matcher.judge_step(MISSING_HOLE), matcher.policy());
    let problems = gate::gate(&bad, "default", &sockets, &outcome);
    assert!(
        problems.iter().any(|p| p.contains("harness")),
        "{problems:?}"
    );
}

/// SC7: a missing corpus passes with a note.
#[test]
fn a_missing_corpus_passes_with_a_note() {
    assert!(Corpus::at(Path::new("no/such/corpus"), false).is_none());
}

/// SC7: a required corpus that is missing fails: a gate that did not run
/// never passes green.
#[test]
#[should_panic(expected = "FITMENT_REQUIRE_CORPUS=1 but there is no corpus")]
fn a_required_corpus_must_be_there() {
    let _ = Corpus::at(Path::new("no/such/corpus"), true);
}

/// SC8: `waived` fits under no policy; its pass is no false positive, it
/// counts in its own row, outside the rates; rejected, it fails the gate
/// and is no false negative.
#[test]
fn waived_is_neither_a_fit_nor_a_false_positive() {
    assert!(!Truth::Waived.fits(Policy::Default) && !Truth::Waived.fits(Policy::Functional));
    let case = waived();
    let corpus = Corpus::of(Path::new("."), json!({"cases": [case]}));
    let (document, sockets) = socket(&case);
    let judged = |bytes: &[u8]| {
        let matcher = Matcher::new(
            &document,
            Policy::Default.match_policy(),
            MatchOptions::default(),
            &NoFitVerifier,
        )
        .unwrap();
        Outcome::of_result(&matcher.judge_step(bytes), matcher.policy())
    };
    let passed = judged(MISSING_HOLE);
    assert_eq!(
        gate::gate(&case, "default", &sockets, &passed),
        Vec::<String>::new()
    );
    let mut rejected = passed.clone();
    rejected.verdict = "reject".to_owned();
    let problems = gate::gate(&case, "default", &sockets, &rejected);
    assert!(
        problems
            .iter()
            .any(|p| p.contains("expected interface_match")),
        "{problems:?}"
    );
    for outcome in [passed, rejected] {
        let run = Run {
            case: "waived".to_owned(),
            policy: Policy::Default,
            outcome,
            path: RunPath::Library,
            at_construction: Some(false),
        };
        let score = Score::new(&corpus, vec![run]);
        assert_eq!(score.false_positives(), Vec::<String>::new());
        assert_eq!(score.false_negatives(), Vec::<Value>::new());
        let report = score.report();
        assert_eq!(report["rows"][0]["class"], "review_waived");
        assert_eq!(report["totals"][0]["fits"], 0);
    }
}

/// SC9: an entry is found by its candidate's SHA-256, whatever its label;
/// none, or two, is an error.
#[test]
fn entries_are_found_by_content() {
    let entry = |sha: &str, label: &str, verdict: &str| {
        json!({"labels": [label], "source": {"kind": "file", "sha256": sha}, "verdict": verdict,
               "status": "accepted", "refusals": [], "sockets": [], "flags": [],
               "fit": {"status": "not_checked"}})
    };
    let doc = json!({"policy": {"name": "default"}, "candidates": [
        entry("aa", "b.stp", "reject"),
        entry("bb", "a.stp", "interface_match"),
    ]});
    assert_eq!(Outcome::find(&doc, "aa").unwrap().verdict, "reject");
    assert_eq!(
        Outcome::find(&doc, "bb").unwrap().verdict,
        "interface_match"
    );
    assert!(Outcome::find(&doc, "a.stp").is_err());
    let twice = json!({"policy": {}, "candidates": [entry("aa", "x", "reject"), entry("aa", "y", "reject")]});
    assert!(Outcome::find(&twice, "aa").unwrap_err().contains("several"));
}

/// The parity helpers: the first difference of two byte strings, a
/// document without labels, XH7's exit codes, and the policies as the CLI
/// names them.
#[test]
fn parity_reads_differences_and_exit_codes() {
    assert_eq!(parity::bytes("doc", b"same", b"same"), None);
    let found = parity::bytes("doc", b"abcdef", b"abcXef").unwrap();
    assert!(found.contains("byte 3"), "{found}");
    let doc = json!({"candidates": [{"labels": ["a"], "verdict": "reject"}], "original": {"labels": ["o"]}});
    let bare = parity::without_labels(&doc);
    assert!(
        bare["candidates"][0].get("labels").is_none() && bare["original"].get("labels").is_none()
    );
    assert_eq!(
        parity::exit_code("succeeded", Some("interface_match")),
        Some(4)
    );
    assert_eq!(parity::exit_code("succeeded", Some("reject")), Some(5));
    assert_eq!(parity::exit_code("succeeded", Some("match")), Some(0));
    assert_eq!(parity::exit_code("failed", None), Some(1));
    assert_eq!(parity::exit_code("refused", None), None);
    let wide = fitment_corpus::outcome::policy_value(&Policy::Tolerance005.match_policy());
    assert_eq!(
        (wide["name"].as_str(), wide["linear_mm"].as_f64()),
        (Some("cli"), Some(0.05))
    );
}
