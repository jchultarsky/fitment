//! What the parity tests compare (`docs/research/m6-api.md`, DC3, XH4,
//! XH7): two byte strings, to their first difference; a document without
//! its labels; and a run's end as the CLI's exit code.

use serde_json::Value;

/// How many bytes of context a difference shows on each side.
const CONTEXT: usize = 40;

/// XH4: `None` when `cli` and `api` are the same bytes; otherwise where
/// they first differ, with context from each.
pub fn bytes(what: &str, cli: &[u8], api: &[u8]) -> Option<String> {
    if cli == api {
        return None;
    }
    let at = cli
        .iter()
        .zip(api)
        .position(|(a, b)| a != b)
        .unwrap_or(cli.len().min(api.len()));
    let around = |b: &[u8]| {
        let from = at.saturating_sub(CONTEXT);
        let to = (at + CONTEXT).min(b.len());
        String::from_utf8_lossy(&b[from.min(to)..to]).into_owned()
    };
    Some(format!(
        "{what}: {} and {} bytes, first differing at byte {at}: the CLI's {:?}, the API's {:?}",
        cli.len(),
        api.len(),
        around(cli),
        around(api)
    ))
}

/// `doc` without its candidates' and its original's labels, which a client
/// sets and MF7's digest leaves out.
pub fn without_labels(doc: &Value) -> Value {
    let mut bare = doc.clone();
    let Some(map) = bare.as_object_mut() else {
        return bare;
    };
    let mut entries: Vec<&mut Value> = Vec::new();
    for (key, value) in map.iter_mut() {
        match key.as_str() {
            "candidates" => entries.extend(value.as_array_mut().into_iter().flatten()),
            "original" => entries.push(value),
            _ => {}
        }
    }
    for entry in entries.into_iter().filter_map(Value::as_object_mut) {
        entry.remove("labels");
    }
    bare
}

/// The outcome a `fitment.match` document states: its best verdict, from
/// its summary (VD14).
pub fn outcome_of(doc: &Value) -> &'static str {
    let count = |key: &str| doc["summary"][key].as_u64().unwrap_or(0);
    if count("match") > 0 {
        "match"
    } else if count("interface_match") > 0 {
        "interface_match"
    } else {
        "reject"
    }
}

/// XH7: the CLI's exit code for a job that ended in `state` with
/// `outcome`: 0, 4 or 5 for a success by its outcome (0 for one that
/// states none, or `unchanged`), 1 for a failure; `None` where the state
/// alone does not say (a refusal is 2 or 3, which the job states) or the
/// CLI has no counterpart (a stop).
pub fn exit_code(state: &str, outcome: Option<&str>) -> Option<i32> {
    match (state, outcome) {
        ("succeeded", Some("interface_match")) => Some(4),
        ("succeeded", Some("reject")) => Some(5),
        ("succeeded", _) => Some(0),
        ("failed", _) => Some(1),
        _ => None,
    }
}
