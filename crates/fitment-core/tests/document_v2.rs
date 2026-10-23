//! `fitment.match` v2 (`docs/research/m7-fit.md`, FM1 to FM5, FC7, FT12):
//! what a run with a fit verifier that has an identity writes, from its
//! own types, beside v1, which a run without one still writes. s01's
//! bracket socket with the test-only exact kernel, which measures the
//! lug's tab into the plate.

use std::collections::BTreeSet;

use fitment_core::fit::testing::{MeasuresVerifier, exact, interfere};
use fitment_core::fit::{FitBody, FitFailure, FitMeasures, SCOPE_NOT_CHECKED};
use fitment_core::{
    FitIdentity, FitRequest, FitVerifier, MatchDocument, MatchOptions, MatchPolicy, Matcher,
    NoFitVerifier, SocketDocument,
};
use serde_json::Value;

const S01: &[u8] = include_bytes!("assemblies/s01_bolted.stp");
const BRACKET: &[u8] = include_bytes!("candidates/m_bracket.stp");
const LUG: &[u8] = include_bytes!("candidates/m_bracket_lug.stp");
const BLIND: &[u8] = include_bytes!("candidates/m_bracket_blind.stp");

fn identity() -> FitIdentity {
    FitIdentity::new("open_cascade", "8.0.1").with_helper(
        "cadquery-ocp-novtk 8.0.1.1.0",
        "3.13",
        1,
        "a".repeat(64),
        "b".repeat(64),
    )
}

/// An exact kernel that measures the lug `residual` mm³ into the plate
/// (none for 0) and every gap outside a zone `gap` mm wide.
fn kernel(residual: f64, gap: f64) -> MeasuresVerifier {
    let lug = fitment_core::sha256_hex(LUG);
    MeasuresVerifier::new(identity(), S01.to_vec(), move |request| {
        let mut measures = exact(request);
        for pair in &mut measures.check.pairs {
            if pair.zoned_distance_mm.is_some() {
                pair.zoned_distance_mm = Some(gap);
            }
        }
        if residual > 0.0
            && matches!(request.body(), FitBody::Step { sha256, .. } if *sha256 == lug)
        {
            let plan = request.plan(&measures.scene, &measures.place);
            let plate = plan.asked().position(|p| p.name.starts_with("PLATE"));
            assert!(interfere(&mut measures, &plan, plate.unwrap(), residual));
        }
        Ok(measures)
    })
}

/// The document of the bracket, the lug and the blind bracket judged
/// against s01's bracket socket with `verifier`.
fn document(verifier: &dyn FitVerifier) -> MatchDocument {
    let socket = SocketDocument::extract(S01, Some("BRACKET"), None).unwrap();
    let matcher = Matcher::new(
        &socket,
        MatchPolicy::default(),
        MatchOptions::default(),
        verifier,
    )
    .unwrap();
    let results = [
        ("m_bracket.stp", BRACKET),
        ("m_bracket_lug.stp", LUG),
        ("m_bracket_blind.stp", BLIND),
    ]
    .into_iter()
    .map(|(label, bytes)| (matcher.judge_step(bytes), vec![label.to_owned()]))
    .collect();
    MatchDocument::new(&matcher, results)
}

fn json(document: &MatchDocument) -> Value {
    serde_json::from_str(&document.to_json()).unwrap()
}

/// The entry labelled `label`.
fn entry<'v>(doc: &'v Value, label: &str) -> &'v Value {
    doc["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["labels"][0] == label)
        .unwrap()
}

/// FM1, FM2, FM3, FM4, FT12: v2 states the verifier and the fit's
/// parameters, the control per socket, and each fit's evidence: a
/// verified fit its occurrences, bodies, copies and every pair with its
/// own tolerances and zones; an interference its pair, with the kernel's
/// measures; a fit never asked why. Every fit lists what no fit checks,
/// and each placement its freedom and alternatives.
#[test]
fn v2_states_the_verifier_the_fit_and_its_evidence() {
    let doc = json(&document(&kernel(299.4, 0.3)));
    assert_eq!(
        (&doc["format"], &doc["format_version"]),
        (&"fitment.match".into(), &2.into())
    );
    assert_eq!(
        doc["match_rules_version"],
        fitment_core::MATCH_RULES_VERSION
    );
    let named = &doc["fit_verifier"];
    assert_eq!(named["kernel"], "open_cascade");
    assert_eq!(named["kernel_version"], "8.0.1");
    assert_eq!(named["binding"], "cadquery-ocp-novtk 8.0.1.1.0");
    assert_eq!(named["protocol"], 1);
    assert_eq!(
        named["fit_rules_version"],
        fitment_core::fit::FIT_RULES_VERSION
    );
    let fit = &doc["fit"];
    assert_eq!(fit["tolerance_mm"], 0.01);
    assert_eq!(fit["guard_mm"], fitment_core::GUARD_MM);
    assert_eq!(
        fit["volume_resolution_mm3"],
        fitment_core::fit::FIT_VOLUME_MM3
    );
    assert_eq!(fit["tolerance_cap"], "t/10");
    assert_eq!(fit["max_fits"], Value::Null);
    assert_eq!(fit["control"][0]["socket"], "socket-1");
    assert_eq!(fit["control"][0]["fit"]["status"], "verified");
    let scope: Vec<Value> = SCOPE_NOT_CHECKED.iter().map(|s| (*s).into()).collect();
    let bracket = entry(&doc, "m_bracket.stp");
    assert_eq!(bracket["verdict"], "match");
    let verified = &bracket["fit"];
    assert_eq!(verified["status"], "verified");
    assert_eq!(
        (
            &verified["occurrences"],
            &verified["bodies"],
            &verified["copies"]
        ),
        (&1.into(), &9.into(), &0.into())
    );
    let pairs = verified["pairs"].as_array().unwrap();
    assert_eq!(pairs.len(), 5, "the plate and the four bolts");
    for pair in pairs {
        assert_eq!(pair["body"]["kind"], "neighbour");
        assert_eq!(pair["u_pair_mm"], fitment_core::GUARD_MM);
        assert_eq!(pair["threshold_mm"], 0.01 + fitment_core::GUARD_MM);
        assert_eq!(pair["zones"][0]["kind"], "slab");
        assert!(pair["measured"].is_object());
    }
    assert_eq!(verified["scope_not_checked"].as_array().unwrap(), &scope);
    let socket = &bracket["sockets"][0];
    assert_eq!(socket["fit"]["status"], "verified");
    assert_eq!(
        socket["placement"]["freedom"],
        Value::Array(Vec::new()),
        "unique (P6)"
    );
    assert!(socket["alternatives"].is_array());
    let lug = entry(&doc, "m_bracket_lug.stp");
    assert_eq!(lug["verdict"], "reject");
    assert_eq!(lug["reasons"][0]["reason"], "body_interferes");
    let hit = &lug["fit"]["interferences"][0];
    assert_eq!(hit["body"]["product"], "PLATE");
    assert_eq!(
        hit["zones"][0]["requirement"]
            .as_str()
            .map(|r| r.starts_with("seat:")),
        Some(true)
    );
    assert_eq!(hit["measured"]["residual_mm3"], 299.4);
    assert_eq!(lug["fit"]["scope_not_checked"].as_array().unwrap(), &scope);
    let blind = entry(&doc, "m_bracket_blind.stp");
    assert_eq!(blind["fit"]["status"], "not_checked");
    assert_eq!(blind["fit"]["reason"], "interface rejected");
    assert_eq!(
        blind["fit"]["scope_not_checked"].as_array().unwrap(),
        &scope
    );
    assert_eq!(
        blind["sockets"][0]["placement"]["freedom"],
        Value::Null,
        "not worked out"
    );
    assert_eq!(blind["sockets"][0]["alternatives"], Value::Null);
}

/// FM3, FV8: a helper's failure is `not_verified` with its code, where it
/// applies and its sentence outside the digest; a resource kill marks the
/// run incomplete.
#[test]
fn a_failure_is_not_verified_with_its_code() {
    struct Exhausted(MeasuresVerifier);
    impl FitVerifier for Exhausted {
        fn identity(&self) -> Option<FitIdentity> {
            self.0.identity()
        }
        fn assembly(&self) -> Option<&[u8]> {
            self.0.assembly()
        }
        fn measure(&self, request: &FitRequest<'_>) -> Result<FitMeasures, FitFailure> {
            match request.body() {
                FitBody::Step { .. } => Err(FitFailure::ResourceExhausted),
                _ => self.0.measure(request),
            }
        }
    }
    let verifier = Exhausted(MeasuresVerifier::exact(identity(), S01.to_vec()));
    let socket = SocketDocument::extract(S01, Some("BRACKET"), None).unwrap();
    let matcher = Matcher::new(
        &socket,
        MatchPolicy::default(),
        MatchOptions::default(),
        &verifier,
    )
    .unwrap();
    let result = matcher.judge_step(BRACKET);
    assert!(result.fit_incomplete());
    let doc = json(&MatchDocument::new(
        &matcher,
        vec![(result, vec!["b".to_owned()])],
    ));
    let fit = &doc["candidates"][0]["fit"];
    assert_eq!(fit["status"], "not_verified");
    assert_eq!(fit["code"], "resource_exhausted");
    assert_eq!(fit["reason"], "the fit check ran out of a resource");
    assert_eq!(
        fit["at"][0],
        serde_json::json!({ "kind": "run", "code": "resource_exhausted" })
    );
    assert_eq!(
        fit["measured"]["findings"][0],
        "the fit check ran out of a resource"
    );
    assert_eq!(doc["candidates"][0]["verdict"], "interface_match");
}

/// FM5: the digest leaves out every `measured` object and every label:
/// the kernel's numbers in their last digits change the bytes and not the
/// digest; a decision that changes changes it. The scorer's checker
/// computes the same digest from the JSON alone.
#[test]
fn v2_s_digest_leaves_out_the_kernel_s_measures() {
    let base = document(&kernel(299.4, 0.3));
    let digits = document(&kernel(299.400_000_000_1, 0.300_000_000_2));
    assert_ne!(base.to_json(), digits.to_json());
    assert_eq!(base.digest(), digits.digest());
    let clear = document(&kernel(0.0, 0.3));
    assert_ne!(base.digest(), clear.digest(), "the lug's verdict changed");
    for doc in [&base, &digits, &clear] {
        let value = json(doc);
        assert_eq!(value["digest"], doc.digest());
        let mut relabelled = value.clone();
        relabelled["candidates"][0]["labels"] = serde_json::json!(["another name"]);
        relabelled["candidates"][0]["fit"]["pairs"][0]["measured"]["common_mm3"] = 1.0.into();
        let recomputed = fitment_core_digest(&relabelled);
        assert_eq!(recomputed, doc.digest());
    }
}

/// FM5's digest recomputed from the JSON: without the digest, the
/// candidates' and the original's labels, and every `measured` object.
fn fitment_core_digest(doc: &Value) -> String {
    fn strip(v: &mut Value) {
        match v {
            Value::Object(map) => {
                map.remove("measured");
                map.values_mut().for_each(strip);
            }
            Value::Array(items) => items.iter_mut().for_each(strip),
            _ => {}
        }
    }
    let mut bare = doc.clone();
    let map = bare.as_object_mut().unwrap();
    map.remove("digest");
    for c in map.get_mut("candidates").unwrap().as_array_mut().unwrap() {
        c.as_object_mut().unwrap().remove("labels");
    }
    strip(&mut bare);
    format!(
        "sha256:{}",
        fitment_core::sha256_hex(bare.to_string().as_bytes())
    )
}

/// The object keys of `value`, recursively, as paths, arrays' items under
/// `[]`.
fn keys(value: &Value, at: &str, out: &mut BTreeSet<String>) {
    match value {
        Value::Object(map) => {
            for (k, v) in map {
                let here = format!("{at}/{k}");
                out.insert(here.clone());
                keys(v, &here, out);
            }
        }
        Value::Array(items) => items.iter().for_each(|v| keys(v, &format!("{at}[]"), out)),
        _ => {}
    }
}

/// FM1, FC7: v1 without an identity, v2 with one; v2 is v1's superset,
/// nothing removed: every key v1 writes for these candidates, v2 writes
/// too, but the fit's own (v2's `fit_verifier` is an object, and its fits
/// are its own types).
#[test]
fn v2_is_v1_with_nothing_removed() {
    let v1 = json(&document(&NoFitVerifier));
    assert_eq!(v1["format_version"], 1);
    assert_eq!(v1["fit_verifier"], Value::Null);
    assert!(v1.get("fit").is_none());
    let v2 = json(&document(&kernel(299.4, 0.3)));
    let (mut ours, mut theirs) = (BTreeSet::new(), BTreeSet::new());
    keys(&v1, "", &mut ours);
    keys(&v2, "", &mut theirs);
    let missing: Vec<&String> = ours
        .difference(&theirs)
        .filter(|k| !k.contains("/fit/") && !k.starts_with("/fit_verifier"))
        .collect();
    assert_eq!(missing, Vec::<&String>::new());
}
