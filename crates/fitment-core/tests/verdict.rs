//! The full truth table of `decide`, and the JSON shapes D1 fixed.

use fitment_core::{
    Check, Evidence, FailureCode, FitStatus, Interference, NO_FIT_VERIFIER, RejectReason,
    RequirementId, VerdictKind, combine, decide,
};
use serde_json::json;

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

fn verified() -> FitStatus {
    FitStatus::Verified {
        kernel: "Open CASCADE".to_owned(),
        version: "8.0.1".to_owned(),
    }
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
    let other = decide(
        &reqs,
        passed(&["h1", "seat"]),
        FitStatus::Verified {
            kernel: "Open CASCADE".to_owned(),
            version: "7.9.0".to_owned(),
        },
    );
    let disagree = combine(&named(&[("socket-1", &good), ("socket-2", &other)]));
    assert!(matches!(disagree.fit(), FitStatus::NotVerified { .. }));
    assert_eq!(disagree.kind(), VerdictKind::InterfaceMatch);
}
