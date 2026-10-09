//! The full truth table of `decide`, and the JSON shapes D1 fixed.

use fitment_core::{
    Check, FitStatus, Interference, NO_FIT_VERIFIER, RejectReason, RequirementId, VerdictKind,
    decide,
};
use serde_json::json;

fn ids(names: &[&str]) -> Vec<RequirementId> {
    names.iter().copied().map(RequirementId::from).collect()
}

fn passed(names: &[&str]) -> Vec<(RequirementId, Check)> {
    names
        .iter()
        .map(|&n| (RequirementId::from(n), Check::Passed))
        .collect()
}

fn failed(reason: &str) -> Check {
    Check::Failed {
        reason: reason.to_owned(),
    }
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
    RejectReason,
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
                ("h1".into(), Check::Passed),
                ("h2".into(), failed("Ø5.5, need Ø6.6 ± 0.01")),
            ],
            RejectReason::RequirementFailed {
                requirement: "h2".into(),
                detail: "Ø5.5, need Ø6.6 ± 0.01".to_owned(),
            },
        ),
        (
            "a check is missing",
            reqs.clone(),
            passed(&["h1"]),
            RejectReason::MissingCheck {
                requirement: "h2".into(),
            },
        ),
        (
            "a requirement was checked twice",
            reqs.clone(),
            vec![
                ("h1".into(), Check::Passed),
                ("h2".into(), Check::Passed),
                ("h2".into(), Check::Passed),
            ],
            RejectReason::DuplicateCheck {
                requirement: "h2".into(),
            },
        ),
        (
            "a check names an unknown requirement",
            reqs.clone(),
            passed(&["h1", "h2", "h9"]),
            RejectReason::UnknownRequirement {
                requirement: "h9".into(),
            },
        ),
        (
            "the socket repeats a requirement",
            ids(&["h1", "h2", "h1"]),
            passed(&["h1", "h2"]),
            RejectReason::DuplicateRequirement {
                requirement: "h1".into(),
            },
        ),
        (
            "the socket is empty",
            Vec::new(),
            Vec::new(),
            RejectReason::NoRequirements,
        ),
    ];

    for (name, reqs, checks, reason) in cases {
        for fit in every_fit() {
            let interferes = matches!(fit, FitStatus::Interferes { .. });
            let verdict = decide(&reqs, checks.clone(), fit);
            assert_eq!(verdict.kind(), VerdictKind::Reject, "{name}");
            let mut expected = vec![reason.clone()];
            if interferes {
                expected.push(RejectReason::BodyInterferes);
            }
            assert_eq!(verdict.reasons(), expected, "{name}");
        }
    }
}

/// A check that passed for one requirement cannot stand in for another.
#[test]
fn checks_are_matched_by_requirement_not_by_count() {
    let verdict = decide(&ids(&["h1", "h2"]), passed(&["h1", "h1"]), verified());
    assert_eq!(verdict.kind(), VerdictKind::Reject);
    assert_eq!(
        verdict.reasons(),
        [
            RejectReason::DuplicateCheck {
                requirement: "h1".into()
            },
            RejectReason::MissingCheck {
                requirement: "h2".into()
            },
        ]
    );
}

#[test]
fn reasons_are_in_a_fixed_order() {
    let reqs = ids(&["b", "a", "b", "c"]);
    let checks = vec![
        ("z".into(), Check::Passed),
        ("c".into(), failed("off axis")),
        ("y".into(), Check::Passed),
        ("b".into(), Check::Passed),
    ];
    let verdict = decide(&reqs, checks, interferes());
    assert_eq!(
        verdict.reasons(),
        [
            RejectReason::MissingCheck {
                requirement: "a".into()
            },
            RejectReason::DuplicateRequirement {
                requirement: "b".into()
            },
            RejectReason::RequirementFailed {
                requirement: "c".into(),
                detail: "off axis".to_owned()
            },
            RejectReason::UnknownRequirement {
                requirement: "y".into()
            },
            RejectReason::UnknownRequirement {
                requirement: "z".into()
            },
            RejectReason::BodyInterferes,
        ]
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
            "reasons": [{"reason": "requirement_failed", "requirement": "h1", "detail": "missing"}],
        })
    );
}
