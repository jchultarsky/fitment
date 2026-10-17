//! XH13's scan and XH11's checks (docs/research/m6-api.md): every
//! response body is scanned for an absolute path of the test machine, SQL
//! text, a Rust type path or panic message, and a password, token or
//! `Authorization` value; every log line for secrets only, since logs
//! name paths for the operator, relative to the configured roots (OP1).
//! Every `fitment.match`, `fitment.socket` and `features.v1` a response
//! carries reads back and passes the checker.

use super::{PREFIX, Reply};

/// What a response must never hold: SQL, Rust's own paths and panics.
const NEVER_IN_A_RESPONSE: [&str; 11] = [
    "SELECT ",
    "INSERT INTO",
    "CREATE TABLE",
    "sqlite",
    "SQLite",
    "fitment_server::",
    "fitment_catalog::",
    "fitment_core::",
    "std::",
    "panicked at",
    ".rs:",
];

/// What neither a response nor a log line may hold: secrets.
const SECRETS: [&str; 4] = ["fitment_pat_", "Bearer ", "$argon2", "PEPPER-"];

/// Every leak in a response body: a root's path, the tests' directory
/// prefix, any of [`NEVER_IN_A_RESPONSE`] or a secret; and every document
/// check that fails (XH11).
pub fn response(reply: &Reply, roots: &[String]) -> Vec<String> {
    let text = String::from_utf8_lossy(&reply.body);
    let mut found = body(&text, roots);
    found.extend(documents(reply));
    found
}

/// The leaks in a body's text.
pub fn body(text: &str, roots: &[String]) -> Vec<String> {
    let mut found: Vec<String> = roots
        .iter()
        .filter(|root| !root.is_empty() && text.contains(root.as_str()))
        .map(|root| format!("a path of the test machine: {root}"))
        .collect();
    if text.contains(PREFIX) {
        found.push(format!("a test directory's name ({PREFIX}…)"));
    }
    for needle in NEVER_IN_A_RESPONSE.iter().chain(&SECRETS) {
        if text.contains(needle) {
            found.push(format!("`{needle}`"));
        }
    }
    found
}

/// The secrets in log lines (XH13): paths are allowed there.
pub fn logs(text: &str) -> Vec<String> {
    SECRETS
        .iter()
        .filter(|needle| text.contains(**needle))
        .map(|needle| format!("a secret in the log: `{needle}`"))
        .collect()
}

/// XH11: a result document in a response reads back and passes the
/// checker.
fn documents(reply: &Reply) -> Vec<String> {
    let json = reply
        .header("content-type")
        .is_some_and(|c| c.starts_with("application/json"));
    if !json {
        return Vec::new();
    }
    let Ok(text) = std::str::from_utf8(&reply.body) else {
        return vec!["a JSON body that is not UTF-8".into()];
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
        return vec!["a JSON body that does not parse".into()];
    };
    match value["format"].as_str() {
        Some("fitment.match") => fitment_corpus::checker::problems(&value),
        Some("fitment.socket") => fitment_core::SocketDocument::from_json(text)
            .err()
            .map(|e| vec![format!("fitment.socket: {e}")])
            .unwrap_or_default(),
        Some("fitment.features") => fitment_core::FeaturesDocument::from_json(text)
            .err()
            .map(|e| vec![format!("features.v1: {e}")])
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}
