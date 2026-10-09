//! Tolerance policies: D9 defaults, validation, and the file format.

use fitment_core::{ToleranceError, TolerancePolicy};
use serde_json::json;

#[test]
fn default_is_d9() {
    let policy = TolerancePolicy::default();
    assert_eq!(policy.name(), "default");
    assert_eq!(policy.linear_mm().to_bits(), 0.01_f64.to_bits());
    assert_eq!(policy.angular_deg().to_bits(), 0.01_f64.to_bits());
}

#[test]
fn refuses_values_that_cannot_be_compared_against() {
    for bad in [0.0, -0.0, -0.01, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert!(
            matches!(
                TolerancePolicy::new("p", bad, 0.01),
                Err(ToleranceError::Linear(_))
            ),
            "linear {bad}"
        );
        assert!(
            matches!(
                TolerancePolicy::new("p", 0.01, bad),
                Err(ToleranceError::Angular(_))
            ),
            "angular {bad}"
        );
    }
    assert_eq!(
        TolerancePolicy::new("  ", 0.01, 0.01),
        Err(ToleranceError::EmptyName)
    );
}

#[test]
fn json_round_trips_bit_for_bit() {
    // Values whose shortest decimal form needs all 17 digits.
    let policy = TolerancePolicy::new("tight", 0.1 + 0.2, 1.0 / 3.0).unwrap();
    let text = serde_json::to_string(&policy).unwrap();
    let back: TolerancePolicy = serde_json::from_str(&text).unwrap();
    assert_eq!(back.linear_mm().to_bits(), policy.linear_mm().to_bits());
    assert_eq!(back.angular_deg().to_bits(), policy.angular_deg().to_bits());
    assert_eq!(back, policy);
}

#[test]
fn json_is_validated_like_code() {
    let ok = json!({"name": "shop", "linear_mm": 0.02, "angular_deg": 0.05});
    let policy: TolerancePolicy = serde_json::from_value(ok).unwrap();
    assert_eq!(policy.name(), "shop");

    for bad in [
        json!({"name": "shop", "linear_mm": 0.0, "angular_deg": 0.05}),
        json!({"name": "shop", "linear_mm": -1.0, "angular_deg": 0.05}),
        json!({"name": "", "linear_mm": 0.02, "angular_deg": 0.05}),
        json!({"name": "shop", "linear_mm": 0.02}),
        // A mistyped key must fail, not be ignored.
        json!({"name": "shop", "linear_mm": 0.02, "angular_deg": 0.05, "lineer_mm": 1.0}),
    ] {
        assert!(
            serde_json::from_value::<TolerancePolicy>(bad.clone()).is_err(),
            "{bad}"
        );
    }
}
