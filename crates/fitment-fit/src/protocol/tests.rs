//! FH2, FH5, FY4 on hand-made replies: the envelope's echoes, strict
//! decoding, and the ranges.

use fitment_core::fit::{
    BodyMeasure, FitBox, LandmarkMeasure, PairMeasure, SceneMeasures, ZoneMeasure,
};
use serde_json::{Value, json};

use super::*;

/// A named edit of a reply's body.
type Edit = (&'static str, fn(&mut Value));

fn sent(op: Op, expect: Expect) -> Sent {
    Sent {
        op,
        nonce: "ab".repeat(16),
        header_sha256: "cd".repeat(32),
        digest: Some("ef".repeat(32)),
        expect,
    }
}

fn envelope(op: &str, reply: &Value) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "op": op,
        "nonce": "ab".repeat(16),
        "request_sha256": "cd".repeat(32),
        "digest": "ef".repeat(32),
        "reply": reply,
    }))
    .unwrap()
}

fn a_body() -> BodyMeasure {
    let mut b = BodyMeasure::default();
    b.path = vec![7, 8];
    b.found = true;
    b.valid = true;
    b.volume_mm3 = 1000.0;
    b.tolerance_mm = 1e-7;
    b.bounds = FitBox {
        min_mm: [0.0; 3],
        max_mm: [1.0, 2.0, 3.0],
    };
    b.landmarks = vec![
        LandmarkMeasure::plane([0.0, 0.0, 1.0], 3.0),
        LandmarkMeasure::default(),
    ];
    b
}

fn one_body() -> SceneMeasures {
    let mut scene = SceneMeasures::default();
    scene.bodies = vec![a_body()];
    scene
}

fn scene_expect() -> Expect {
    Expect::Scene(vec![(vec![7, 8], 2)])
}

#[test]
fn a_scene_reply_round_trips_as_measures() {
    let mut scene = SceneMeasures::default();
    scene.bodies = vec![a_body()];
    let body = serde_json::to_value(scene_reply(&scene)).unwrap();
    let read = scene_measures(&envelope("scene", &body), &sent(Op::Scene, scene_expect())).unwrap();
    assert_eq!(read.bodies, scene.bodies);
    assert_eq!(read.request_sha256, Some("ef".repeat(32)));
    // A box no reader can give crosses as null and comes back unsound.
    scene.bodies[0].bounds.min_mm[0] = f64::NAN;
    let body = serde_json::to_value(scene_reply(&scene)).unwrap();
    assert_eq!(body["bodies"][0]["box"], Value::Null);
    let read = scene_measures(&envelope("scene", &body), &sent(Op::Scene, scene_expect())).unwrap();
    assert!(!read.bodies[0].bounds.sound());
}

#[test]
fn the_envelope_must_echo_its_request() {
    let body = serde_json::to_value(scene_reply(&one_body())).unwrap();
    let good = envelope("scene", &body);
    let expect = || sent(Op::Scene, scene_expect());
    assert!(scene_measures(&good, &expect()).is_ok());
    let mut broken: Value = serde_json::from_slice(&good).unwrap();
    for (key, value) in [
        ("op", json!("place")),
        ("nonce", json!("00".repeat(16))),
        ("request_sha256", json!("00".repeat(32))),
        ("digest", json!("00".repeat(32))),
        ("payloads", json!([1])),
        ("extra", json!(1)),
    ] {
        let kept = broken.get(key).cloned();
        broken[key] = value;
        let bytes = serde_json::to_vec(&broken).unwrap();
        assert!(scene_measures(&bytes, &expect()).is_err(), "{key}");
        match kept {
            Some(v) => broken[key] = v,
            None => {
                broken.as_object_mut().unwrap().remove(key);
            }
        }
    }
    broken.as_object_mut().unwrap().remove("digest");
    let bytes = serde_json::to_vec(&broken).unwrap();
    assert!(scene_measures(&bytes, &expect()).is_err(), "no digest");
    let error = json!({"op": "error", "nonce": null, "error": {"code": "x", "sentence": "y"}});
    let refused = scene_measures(&serde_json::to_vec(&error).unwrap(), &expect()).unwrap_err();
    assert!(refused.0.contains("refused"), "{refused}");
}

#[test]
fn a_reply_is_decoded_strictly() {
    let body = serde_json::to_value(scene_reply(&one_body())).unwrap();
    let expect = || sent(Op::Scene, scene_expect());
    let cases: [Edit; 7] = [
        ("an unknown key", |b| b["bodies"][0]["extra"] = json!(1)),
        ("a nullable key left out", |b| {
            b["bodies"][0].as_object_mut().unwrap().remove("box");
        }),
        ("a body not asked", |b| b["bodies"][0]["key"] = json!([9])),
        ("a body missing", |b| b["bodies"] = json!([])),
        ("a landmark missing", |b| {
            b["bodies"][0]["landmarks"] = json!([null]);
        }),
        ("a negative volume", |b| {
            b["bodies"][0]["volume_mm3"] = json!(-1.0);
        }),
        ("an unsorted solid", |b| {
            b["solids"] = json!([{"faces": [3, 1], "location": [[1,0,0,0],[0,1,0,0],[0,0,1,0]], "box": null}]);
        }),
    ];
    for (what, edit) in cases {
        let mut broken = body.clone();
        edit(&mut broken);
        assert!(
            scene_measures(&envelope("scene", &broken), &expect()).is_err(),
            "{what}"
        );
    }
    // FH5: NaN and overflowing numbers are no JSON; −0.0 is not kept.
    let text = String::from_utf8(envelope("scene", &body)).unwrap();
    for bad in ["NaN", "1e999", "-Infinity"] {
        let edited = text.replacen("1000.0", bad, 1);
        assert!(
            scene_measures(edited.as_bytes(), &expect()).is_err(),
            "{bad}"
        );
    }
    let edited = text.replacen("1000.0", "-0.0", 1);
    let read = scene_measures(edited.as_bytes(), &expect()).unwrap();
    assert!(read.bodies[0].volume_mm3.is_sign_positive());
}

fn a_pair() -> PairMeasure {
    let mut p = PairMeasure::default();
    p.distance_mm = Some(0.0);
    p.zoned_distance_mm = Some(0.3);
    p.body_kept_mm3 = Some(1000.0);
    p.body_inside_mm3 = Some(0.0);
    p.zones = vec![ZoneMeasure::new(
        0.5,
        [0.0; 3],
        FitBox {
            min_mm: [0.0; 3],
            max_mm: [1.0; 3],
        },
    )];
    p.proximity_zones = p.zones.clone();
    p
}

fn check_body(pairs: &[PairMeasure]) -> Value {
    let pairs: Vec<Value> = pairs
        .iter()
        .enumerate()
        .map(|(i, m)| {
            serde_json::to_value(wire::WirePair::of((&[1], &[i as u64], false), m)).unwrap()
        })
        .collect();
    json!({"pairs": pairs, "reach": []})
}

fn check_expect(n: usize) -> Expect {
    Expect::Check(
        (0..n)
            .map(|i| PairKey {
                occurrence: vec![1],
                body: vec![i as u64],
                copy: false,
                zones: 1,
            })
            .collect(),
    )
}

#[test]
fn a_check_reply_holds_every_pair_asked_in_range() {
    let pairs = [a_pair(), a_pair()];
    let good = check_body(&pairs);
    let read =
        check_measures(&envelope("check", &good), &sent(Op::Check, check_expect(2))).unwrap();
    assert_eq!(read.pairs, pairs);
    let cases: [Edit; 8] = [
        ("a pair missing", |b| {
            b["pairs"].as_array_mut().unwrap().pop();
        }),
        ("a pair extra", |b| {
            let first = b["pairs"][0].clone();
            b["pairs"].as_array_mut().unwrap().push(first);
        }),
        ("a pair out of order", |b| {
            b["pairs"][0]["body"] = json!([1]);
        }),
        ("a zone missing", |b| b["pairs"][0]["zones"] = json!([])),
        ("a residual above its Common", |b| {
            b["pairs"][0]["residual_mm3"] = json!(1.0);
        }),
        ("more within the zones than the Common", |b| {
            b["pairs"][0]["inside_mm3"] = json!(1.0);
        }),
        ("a negative distance", |b| {
            b["pairs"][0]["distance_mm"] = json!(-1e-9);
        }),
        ("unsorted alerts", |b| {
            b["pairs"][0]["alerts"] = json!(["b", "a"]);
        }),
    ];
    for (what, edit) in cases {
        let mut broken = good.clone();
        edit(&mut broken);
        let read = check_measures(
            &envelope("check", &broken),
            &sent(Op::Check, check_expect(2)),
        );
        assert!(read.is_err(), "{what}");
    }
    // A residual at its Common's rounding is in range, as core's FY4 has it.
    let mut close = good.clone();
    close["pairs"][0]["common_mm3"] = json!(1000.0);
    close["pairs"][0]["residual_mm3"] = json!(1000.0 + 5e-8);
    assert!(
        check_measures(
            &envelope("check", &close),
            &sent(Op::Check, check_expect(2))
        )
        .is_ok()
    );
}

/// The reviews of PR 2a (L3): an error frame's code and sentence, a
/// reply's operation and serde's own messages, whatever the helper put in
/// them, reach a refusal escaped and capped: no escape, newline or
/// carriage return, a forged log line kept on one line, never 100,000
/// characters.
#[test]
fn a_helper_s_text_is_escaped_and_capped_in_every_refusal() {
    let forged = "\u{1b}[31mok\nERROR fitment: verified\r";
    let long = format!("{forged}{}", "y".repeat(100_000));
    let expect = || sent(Op::Scene, scene_expect());
    let error = json!({"op": "error", "error": {"code": forged, "sentence": long}});
    let op = json!({"op": long, "nonce": "", "request_sha256": "", "reply": {}});
    let field = format!(r#"{{"op":"scene","{long}":1}}"#);
    for header in [
        serde_json::to_vec(&error).unwrap(),
        serde_json::to_vec(&op).unwrap(),
        envelope(
            &long,
            &serde_json::to_value(scene_reply(&one_body())).unwrap(),
        ),
        field.into_bytes(),
    ] {
        let refused = scene_measures(&header, &expect()).unwrap_err().0;
        assert!(refused.len() < 1000, "{} characters", refused.len());
        assert!(!refused.contains(['\u{1b}', '\n', '\r']), "{refused}");
    }
    assert_eq!(quoted(forged), r"\x1b[31mok\nERROR fitment: verified\r");
}

/// The reviews of PR 2a: the measures carry core's digest as the helper
/// echoed it, so core's own check reads the helper's answer.
#[test]
fn the_measures_carry_the_helper_s_echo() {
    let body = serde_json::to_value(scene_reply(&one_body())).unwrap();
    let read = scene_measures(&envelope("scene", &body), &sent(Op::Scene, scene_expect())).unwrap();
    assert_eq!(
        read.request_sha256.as_deref(),
        Some("ef".repeat(32).as_str())
    );
}
