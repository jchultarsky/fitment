//! The full truth table of `decide`, and the JSON shapes D1 fixed; VT51
//! (`docs/research/m7-fit.md`, FC1): only a `verified` fit makes a
//! `Match`, an `interferes` a `Reject` and the other two an
//! `InterfaceMatch`, per socket and per candidate, through the matcher as
//! through `decide`, and a `verified` is had only from `fit::decide` (its
//! compile-fail doctest is on `FitStatus`).

use std::sync::OnceLock;

use fitment_core::fit::FitFailure;
use fitment_core::fit::testing::{MeasuresVerifier, exact, interfere};
use fitment_core::{
    Check, Evidence, FailureCode, FitIdentity, FitStatus, FitVerifier, Interference, MatchOptions,
    MatchPolicy, Matcher, NO_FIT_VERIFIER, NoFitVerifier, RejectReason, RequirementId,
    SelectedComponent, SocketDocument, VerdictKind, combine, decide,
};
use serde_json::json;

const S01: &[u8] = include_bytes!("assemblies/s01_bolted.stp");
const LUG: &[u8] = include_bytes!("candidates/m_bracket_lug.stp");

fn ids(names: &[&str]) -> Vec<RequirementId> {
    names.iter().copied().map(RequirementId::from).collect()
}

fn passed(names: &[&str]) -> Vec<(RequirementId, Check)> {
    names
        .iter()
        .map(|&n| (RequirementId::from(n), pass()))
        .collect()
}

fn pass() -> Check {
    Check::of(Evidence::default())
}

fn failed(reason: &str) -> Check {
    Check::of(Evidence::failed(FailureCode::NoFeature, reason))
}

fn s01() -> SocketDocument {
    SocketDocument::extract(S01, Some("BRACKET"), None).unwrap()
}

/// s01's bracket, its own original, judged with `verifier`.
fn judged_original(verifier: &dyn FitVerifier) -> fitment_core::CandidateResult {
    let document = s01();
    let matcher = Matcher::new(
        &document,
        MatchPolicy::default(),
        MatchOptions::default(),
        verifier,
    )
    .unwrap();
    let original = SelectedComponent::target(S01, &document).unwrap();
    matcher.judge_component(&original, original.source())
}

/// A `verified` by `kernel` at `version`, as only `fit::decide` makes one
/// (FC1): s01's bracket judged by an exact kernel of that identity.
fn verified_by(version: &str) -> FitStatus {
    let identity = FitIdentity::new("Open CASCADE", version);
    let fit = judged_original(&MeasuresVerifier::exact(identity, S01.to_vec()))
        .verdict
        .fit()
        .clone();
    assert!(matches!(fit, FitStatus::Verified { .. }), "{fit:?}");
    fit
}

fn verified() -> FitStatus {
    static VERIFIED: OnceLock<FitStatus> = OnceLock::new();
    VERIFIED.get_or_init(|| verified_by("8.0.1")).clone()
}

fn interferes() -> FitStatus {
    FitStatus::Interferes {
        interferences: vec![Interference::new("plate", 0.021)],
    }
}

fn not_verified() -> FitStatus {
    FitStatus::NotVerified {
        reason: "helper timed out".to_owned(),
    }
}

fn every_fit() -> [FitStatus; 4] {
    [
        verified(),
        interferes(),
        not_verified(),
        FitStatus::no_verifier(),
    ]
}

/// The interface passes: the kind depends only on the fit.
#[test]
fn passing_interface_is_judged_by_fit_alone() {
    let reqs = ids(&["h1", "h2", "seat"]);
    for fit in every_fit() {
        let expected = match fit {
            FitStatus::Verified { .. } => VerdictKind::Match,
            FitStatus::Interferes { .. } => VerdictKind::Reject,
            FitStatus::NotVerified { .. } | FitStatus::NotChecked { .. } => {
                VerdictKind::InterfaceMatch
            }
        };
        let verdict = decide(&reqs, passed(&["h1", "h2", "seat"]), fit.clone());
        assert_eq!(verdict.kind(), expected, "fit {fit:?}");
        assert_eq!(verdict.fit(), &fit);
        if expected == VerdictKind::Reject {
            assert_eq!(verdict.reasons(), [RejectReason::BodyInterferes]);
        } else {
            assert_eq!(verdict.reasons(), []);
        }
    }
}

/// A socket, the checks made against it, and the one reason it must fail.
type Case = (
    &'static str,
    Vec<RequirementId>,
    Vec<(RequirementId, Check)>,
    serde_json::Value,
);

/// Every way the interface can fail, under every fit: always Reject, never
/// Match, even with a verified fit.
#[test]
fn failing_interface_rejects_under_every_fit() {
    let reqs = ids(&["h1", "h2"]);
    let cases: Vec<Case> = vec![
        (
            "a check failed",
            reqs.clone(),
            vec![
                ("h1".into(), pass()),
                ("h2".into(), failed("Ø5.5, need Ø6.6 ± 0.01")),
            ],
            json!({"reason": "requirement_failed", "requirement": "h2",
                   "detail": "Ø5.5, need Ø6.6 ± 0.01", "codes": ["no_feature"]}),
        ),
        (
            "a check is missing",
            reqs.clone(),
            passed(&["h1"]),
            json!({"reason": "missing_check", "requirement": "h2"}),
        ),
        (
            "a requirement was checked twice",
            reqs.clone(),
            vec![
                ("h1".into(), pass()),
                ("h2".into(), pass()),
                ("h2".into(), pass()),
            ],
            json!({"reason": "duplicate_check", "requirement": "h2"}),
        ),
        (
            "a check names an unknown requirement",
            reqs.clone(),
            passed(&["h1", "h2", "h9"]),
            json!({"reason": "unknown_requirement", "requirement": "h9"}),
        ),
        (
            "the socket repeats a requirement",
            ids(&["h1", "h2", "h1"]),
            passed(&["h1", "h2"]),
            json!({"reason": "duplicate_requirement", "requirement": "h1"}),
        ),
        (
            "the socket is empty",
            Vec::new(),
            Vec::new(),
            json!({"reason": "no_requirements"}),
        ),
    ];

    for (name, reqs, checks, reason) in cases {
        for fit in every_fit() {
            let interferes = matches!(fit, FitStatus::Interferes { .. });
            let verdict = decide(&reqs, checks.clone(), fit);
            assert_eq!(verdict.kind(), VerdictKind::Reject, "{name}");
            let mut expected = vec![reason.clone()];
            if interferes {
                expected.push(json!({"reason": "body_interferes"}));
            }
            assert_eq!(
                serde_json::to_value(verdict.reasons()).unwrap(),
                json!(expected),
                "{name}"
            );
        }
    }
}

/// A check that passed for one requirement cannot stand in for another.
#[test]
fn checks_are_matched_by_requirement_not_by_count() {
    let verdict = decide(&ids(&["h1", "h2"]), passed(&["h1", "h1"]), verified());
    assert_eq!(verdict.kind(), VerdictKind::Reject);
    assert_eq!(
        serde_json::to_value(verdict.reasons()).unwrap(),
        json!([
            {"reason": "duplicate_check", "requirement": "h1"},
            {"reason": "missing_check", "requirement": "h2"},
        ])
    );
}

#[test]
fn reasons_are_in_a_fixed_order() {
    let reqs = ids(&["b", "a", "b", "c"]);
    let checks = vec![
        ("z".into(), pass()),
        ("c".into(), failed("off axis")),
        ("y".into(), pass()),
        ("b".into(), pass()),
    ];
    let verdict = decide(&reqs, checks, interferes());
    assert_eq!(
        serde_json::to_value(verdict.reasons()).unwrap(),
        json!([
            {"reason": "missing_check", "requirement": "a"},
            {"reason": "duplicate_requirement", "requirement": "b"},
            {"reason": "requirement_failed", "requirement": "c", "detail": "off axis",
             "codes": ["no_feature"]},
            {"reason": "unknown_requirement", "requirement": "y"},
            {"reason": "unknown_requirement", "requirement": "z"},
            {"reason": "body_interferes"},
        ])
    );
}

#[test]
fn fit_status_json_matches_d1() {
    assert_eq!(
        serde_json::to_value(FitStatus::no_verifier()).unwrap(),
        json!({"status": "not_checked", "reason": NO_FIT_VERIFIER})
    );
    assert_eq!(NO_FIT_VERIFIER, "no fit verifier in this build");
    assert_eq!(
        serde_json::to_value(verified()).unwrap(),
        json!({"status": "verified", "kernel": "Open CASCADE", "version": "8.0.1"})
    );
    assert_eq!(
        serde_json::to_value(not_verified()).unwrap(),
        json!({"status": "not_verified", "reason": "helper timed out"})
    );
    assert_eq!(
        serde_json::to_value(interferes()).unwrap(),
        json!({"status": "interferes", "interferences": [{"neighbor": "plate", "volume_mm3": 0.021}]})
    );
}

#[test]
fn verdict_json_shape() {
    let verdict = decide(&ids(&["h1"]), passed(&["h1"]), FitStatus::no_verifier());
    assert_eq!(
        serde_json::to_value(&verdict).unwrap(),
        json!({
            "verdict": "interface_match",
            "fit": {"status": "not_checked", "reason": NO_FIT_VERIFIER},
            "reasons": [],
        })
    );

    let verdict = decide(
        &ids(&["h1"]),
        vec![("h1".into(), failed("missing"))],
        verified(),
    );
    assert_eq!(
        serde_json::to_value(&verdict).unwrap(),
        json!({
            "verdict": "reject",
            "fit": {"status": "verified", "kernel": "Open CASCADE", "version": "8.0.1"},
            "reasons": [{"reason": "requirement_failed", "requirement": "h1", "detail": "missing", "codes": ["no_feature"]}],
        })
    );
}

/// VD2: a candidate's verdict is the worst of its sockets'; a reject names
/// each failed requirement with its socket; no socket rejects; the body
/// interferes if it does in any socket, and is verified only in all.
#[test]
fn a_candidate_is_judged_by_its_worst_socket() {
    let reqs = ids(&["h1", "seat"]);
    let good = decide(&reqs, passed(&["h1", "seat"]), verified());
    let unchecked = decide(&reqs, passed(&["h1", "seat"]), FitStatus::no_verifier());
    let bad = decide(
        &reqs,
        vec![("h1".into(), pass()), ("seat".into(), failed("no plane"))],
        FitStatus::no_verifier(),
    );
    let named = |v: &[(&str, &fitment_core::Verdict)]| -> Vec<(String, fitment_core::Verdict)> {
        v.iter()
            .map(|(s, v)| ((*s).to_owned(), (*v).clone()))
            .collect()
    };
    assert_eq!(
        combine(&named(&[("socket-1", &good)])).kind(),
        VerdictKind::Match
    );
    let both = combine(&named(&[("socket-1", &good), ("socket-2", &unchecked)]));
    assert_eq!(both.kind(), VerdictKind::InterfaceMatch);
    assert!(matches!(both.fit(), FitStatus::NotChecked { .. }));
    let one_bad = combine(&named(&[("socket-1", &good), ("socket-2", &bad)]));
    assert_eq!(one_bad.kind(), VerdictKind::Reject);
    assert!(matches!(
        one_bad.reasons(),
        [RejectReason::RequirementFailed { requirement, .. }] if requirement.as_str() == "socket-2/seat"
    ));
    let none = combine(&[]);
    assert_eq!(none.kind(), VerdictKind::Reject);
    assert_eq!(none.reasons(), &[RejectReason::NoSockets]);
    let clash = decide(&reqs, passed(&["h1", "seat"]), interferes());
    let any = combine(&named(&[("socket-1", &good), ("socket-2", &clash)]));
    assert!(
        matches!(any.fit(), FitStatus::Interferes { interferences } if interferences.len() == 1)
    );
    assert_eq!(any.kind(), VerdictKind::Reject);
    // Two sockets verified by kernels that disagree: no Match (VD4).
    let other = decide(&reqs, passed(&["h1", "seat"]), verified_by("7.9.0"));
    let disagree = combine(&named(&[("socket-1", &good), ("socket-2", &other)]));
    assert!(matches!(disagree.fit(), FitStatus::NotVerified { .. }));
    assert_eq!(disagree.kind(), VerdictKind::InterfaceMatch);
}

/// VT51 through the matcher (FC1, FC2): s01's bracket and its lug, each
/// judged with every kind of verifier. An exact kernel verifies the
/// bracket, a `Match` at its socket and for the candidate; the lug's tab
/// measured into the plate interferes, a `Reject` with `body_interferes`;
/// a helper's failure is `not_verified` and a run without a verifier
/// `not_checked`, both an `InterfaceMatch`. The verdict follows the fit
/// alone: the interface passes in every case.
#[test]
fn only_a_verified_fit_makes_a_match_through_the_matcher() {
    let identity = || FitIdentity::new("open_cascade", "8.0.1");
    let exact_kernel = MeasuresVerifier::exact(identity(), S01.to_vec());
    let into_plate = MeasuresVerifier::new(identity(), S01.to_vec(), |request| {
        let mut measures = exact(request);
        let plan = request.plan(&measures.scene, &measures.place);
        let plate = plan
            .asked()
            .position(|p| p.name.starts_with("PLATE"))
            .unwrap();
        assert!(interfere(&mut measures, &plan, plate, 299.4));
        Ok(measures)
    });
    let timed_out = MeasuresVerifier::new(identity(), S01.to_vec(), |_| Err(FitFailure::Timeout));
    let document = s01();
    let cases: [(&dyn FitVerifier, &[u8], VerdictKind); 4] = [
        (
            &exact_kernel,
            include_bytes!("candidates/m_bracket.stp"),
            VerdictKind::Match,
        ),
        (&into_plate, LUG, VerdictKind::Reject),
        (&timed_out, LUG, VerdictKind::InterfaceMatch),
        (&NoFitVerifier, LUG, VerdictKind::InterfaceMatch),
    ];
    for (verifier, bytes, expected) in cases {
        let matcher = Matcher::new(
            &document,
            MatchPolicy::default(),
            MatchOptions::default(),
            verifier,
        )
        .unwrap();
        let result = matcher.judge_step(bytes);
        assert!(
            result.sockets[0].checks.iter().all(|(_, c)| c.passed()),
            "the interface passes"
        );
        assert_eq!(
            result.verdict.kind(),
            expected,
            "{:?}",
            result.verdict.fit()
        );
        assert_eq!(result.sockets[0].verdict.kind(), expected);
        let fit = result.verdict.fit();
        match expected {
            VerdictKind::Match => assert!(matches!(fit, FitStatus::Verified { .. })),
            VerdictKind::Reject => {
                assert!(matches!(fit, FitStatus::Interferes { .. }));
                assert_eq!(result.verdict.reasons(), [RejectReason::BodyInterferes]);
            }
            VerdictKind::InterfaceMatch => assert!(matches!(
                fit,
                FitStatus::NotVerified { .. } | FitStatus::NotChecked { .. }
            )),
        }
    }
}
