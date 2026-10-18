//! XH13's scan and XH11's checks (docs/research/m6-api.md): every
//! response body is scanned for an absolute path of the test machine, SQL
//! text, a Rust type path or panic message, and a password, token or
//! `Authorization` value; every log line for secrets only, since logs
//! name paths for the operator, relative to the configured roots (OP1).
//! Every `fitment.match`, `fitment.socket` and `features.v1` a response
//! carries reads back and passes the checker.

use std::fmt::Write as _;

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

/// The secrets the tests made: every password they used, every token a
/// response or a command gave them (whole and without its prefix) and
/// every pepper (XH13).
static KNOWN: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// Adds `secret` to what no response or log line may hold; a token is
/// registered with and without its prefix.
pub fn secret(secret: &str) {
    let mut known = KNOWN.lock().unwrap();
    let body = secret.strip_prefix("fitment_pat_").unwrap_or(secret);
    for text in [secret, body] {
        if text.len() >= 8 && !known.iter().any(|k| k == text) {
            known.push(text.to_owned());
        }
    }
}

/// Every secret, as the scan names it: the fixed needles whole, the known
/// ones by their first four characters only.
fn secrets() -> Vec<(String, String)> {
    let mut all: Vec<(String, String)> = SECRETS
        .iter()
        .map(|s| ((*s).to_owned(), format!("`{s}`")))
        .collect();
    for known in KNOWN.lock().unwrap().iter() {
        let shown = format!("a known secret `{}…`", known.get(..4).unwrap_or(""));
        all.push((known.clone(), shown));
    }
    all
}

/// A creating response's `fitment.token` (AU3) carries its secret once:
/// it is registered, and masked for the scan of the rest of the body.
fn masked(body: &str) -> String {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(body) else {
        return body.to_owned();
    };
    let Some(token) = value["secret"]
        .as_str()
        .filter(|_| value["format"] == "fitment.token")
    else {
        return body.to_owned();
    };
    secret(token);
    body.replace(token, "<the token's secret>")
}

/// Every leak in a response, body and header values: a root's path, the tests' directory
/// prefix, any of [`NEVER_IN_A_RESPONSE`] or a secret; and every document
/// check that fails (XH11).
pub fn response(reply: &Reply, roots: &[String]) -> Vec<String> {
    let mut text = masked(&String::from_utf8_lossy(&reply.body));
    // Header values too (review S9): a path or a token in a header leaks
    // as surely as in a body.
    for (name, value) in &reply.headers {
        let _ = write!(
            text,
            "\n{name}: {}",
            String::from_utf8_lossy(value.as_bytes())
        );
    }
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
    for needle in NEVER_IN_A_RESPONSE {
        if text.contains(needle) {
            found.push(format!("`{needle}`"));
        }
    }
    for (needle, shown) in secrets() {
        if text.contains(&needle) {
            found.push(shown);
        }
    }
    found
}

/// The secrets in log lines (XH13): paths are allowed there.
pub fn logs(text: &str) -> Vec<String> {
    secrets()
        .into_iter()
        .filter(|(needle, _)| text.contains(needle.as_str()))
        .map(|(_, shown)| format!("a secret in the log: {shown}"))
        .collect()
}

/// XH11: a result document in a response reads back and passes the
/// checker.
fn documents(reply: &Reply) -> Vec<String> {
    let json = reply
        .header("content-type")
        .is_some_and(|c| c.starts_with("application/json"));
    // A HEAD's answer carries the headers of a body it does not send; a
    // gzipped one is checked by the test that unzips it (DC6).
    if !json || reply.body.is_empty() || reply.header("content-encoding").is_some() {
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
        Some("fitment.parts") => fitment_core::PartsDocument::from_json(text)
            .err()
            .map(|e| vec![format!("fitment.parts: {e}")])
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}
