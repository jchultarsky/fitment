//! The committed false negatives (`docs/research/m4-match.md`, KH4, KH5;
//! `docs/research/m6-api.md`, SC5, M6-46): where they live, the one path
//! that blesses them, and the comparison every other path makes.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::policy::Policy;

/// `crates/fitment/tests/corpus/false-negatives.json`, where it has
/// always been (M6-46).
pub fn path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../fitment/tests/corpus/false-negatives.json")
}

/// The committed baseline, if there is one and it is JSON.
pub fn committed() -> Option<Value> {
    fs::read_to_string(path())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
}

/// The cause the baseline states for (`case`, `policy`), if it lists it.
pub fn cause(committed: Option<&Value>, case: &str, policy: Policy) -> Option<String> {
    committed?["false_negatives"]
        .as_array()?
        .iter()
        .find(|f| f["case"] == case && f["policy"] == policy.name())
        .and_then(|f| f["cause"].as_str().map(str::to_owned))
}

/// The whole record: the build's versions, the cases run and their false
/// negatives.
pub fn document(found: &[Value], cases: &[Value]) -> Value {
    json!({
        "match_rules_version": fitment_core::MATCH_RULES_VERSION,
        "signature_version": fitment_core::SIGNATURE_VERSION,
        "extractor_version": fitment_core::EXTRACTOR_VERSION,
        "socket_rules_version": fitment_core::SOCKET_RULES_VERSION,
        "cases": cases,
        "false_negatives": found,
    })
}

fn entries(committed: Option<&Value>) -> Vec<Value> {
    committed
        .and_then(|c| c["false_negatives"].as_array().cloned())
        .unwrap_or_default()
}

fn differ(found: &[Value], old: &[Value]) -> String {
    let added: Vec<&Value> = found.iter().filter(|f| !old.contains(f)).collect();
    let removed: Vec<&Value> = old.iter().filter(|f| !found.contains(f)).collect();
    format!("added: {added:#?}\nremoved: {removed:#?}")
}

/// KH4, KH5, for the library suite alone (SC5): the false negatives are
/// exactly the committed baseline. `FITMENT_BLESS=1` rewrites it, never
/// under `CI`, and never to add an entry for a case the baseline already
/// knew unless the matching rules' version rose: a new false negative
/// comes from a new case or a rule change, never from an unrelated edit.
pub fn bless_or_compare(found: &[Value], cases: &[Value]) -> Result<(), String> {
    let document = document(found, cases);
    let committed = committed();
    // The whole record: a case added, or a version risen, is blessed too,
    // so the refusal below always judges against the cases and the rules
    // version of the last run.
    if committed.as_ref() == Some(&document) {
        return Ok(());
    }
    let old = entries(committed.as_ref());
    let bless = std::env::var_os("FITMENT_BLESS").is_some_and(|v| v == "1");
    if !bless {
        return Err(format!(
            "the false negatives, the cases or the versions differ from {}:\n{}\n\
             if intended, run FITMENT_BLESS=1 cargo test -p fitment --test matching",
            path().display(),
            differ(found, &old)
        ));
    }
    if std::env::var_os("CI").is_some() {
        return Err("the baseline is blessed locally, never in CI".to_owned());
    }
    let known: Vec<Value> = committed
        .as_ref()
        .and_then(|c| c["cases"].as_array().cloned())
        .unwrap_or_default();
    let same_rules = committed
        .as_ref()
        .is_some_and(|c| c["match_rules_version"] == fitment_core::MATCH_RULES_VERSION);
    let refused: Vec<&Value> = found
        .iter()
        .filter(|f| !old.contains(f) && same_rules && known.contains(&f["case"]))
        .collect();
    if !refused.is_empty() {
        return Err(format!(
            "new false negatives on known cases without a rules version rise: {refused:?}"
        ));
    }
    let text = serde_json::to_string_pretty(&document).map_err(|e| e.to_string())? + "\n";
    if let Some(parent) = path().parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    fs::write(path(), text).map_err(|e| format!("cannot write {}: {e}", path().display()))
}

/// SC5, for every path but the library's: the false negatives of the
/// cases run are the baseline's entries for those cases, keyed by (case,
/// policy, class, failed) with the baseline's cause. It never writes.
pub fn compare(found: &[Value], cases: &[Value]) -> Result<(), String> {
    let old: Vec<Value> = entries(committed().as_ref())
        .into_iter()
        .filter(|f| cases.contains(&f["case"]))
        .collect();
    if found == old.as_slice() {
        return Ok(());
    }
    Err(format!(
        "the false negatives differ from {}, which only the library suite blesses:\n{}",
        path().display(),
        differ(found, &old)
    ))
}
