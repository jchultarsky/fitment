//! KH12 and EV7: everything a `fitment.match` document states that
//! follows from the rest of it, recomputed from the JSON alone: each
//! measure's result from its margin and limits, each check's from its
//! measures and failures, each socket's verdict from its checks, stage and
//! fit, each candidate's from its sockets and fit, the records and flags
//! the envelope implies, the summary, MF5's order and the digest. Both
//! versions: v1, and v2, a run's with a fit verifier
//! (`docs/research/m7-fit.md`, FM1), whose digest leaves out every
//! `measured` object too (FM5).

use std::collections::BTreeSet;

use serde_json::Value;

/// Every problem found; none for a sound document.
pub fn problems(doc: &Value) -> Vec<String> {
    let mut out = Vec::new();
    if doc["format"] != "fitment.match"
        || !(doc["format_version"] == 1 || doc["format_version"] == 2)
    {
        out.push("not a fitment.match v1 or v2 document".to_owned());
    }
    let mut counts = [0_u64; 4];
    let mut keys = Vec::new();
    let mut identities = BTreeSet::new();
    let entries = doc["candidates"].as_array().cloned().unwrap_or_default();
    for c in &entries {
        let id = c["labels"].to_string();
        candidate(doc, &id, c, &mut out);
        let verdict = s(&c["verdict"]);
        let rank = ["match", "interface_match", "reject"]
            .iter()
            .position(|v| *v == verdict)
            .unwrap_or(3);
        counts[rank.min(2)] += 1;
        if c["status"] == "refused" {
            counts[3] += 1;
        }
        let identity = s(&c["source"]["sha256"]);
        if !identity.is_empty() && !identities.insert(identity) {
            out.push(format!("{id}: a second entry for {identity}"));
        }
        keys.push((rank, stage_rank(c), failed_count(c), identity));
    }
    if let Some(original) = doc.get("original") {
        candidate(doc, "original", original, &mut out);
    }
    let summary = &doc["summary"];
    let stated = [
        summary["match"].as_u64(),
        summary["interface_match"].as_u64(),
        summary["reject"].as_u64(),
        summary["refused"].as_u64(),
    ];
    if stated != counts.map(Some) || summary["candidates"].as_u64() != Some(entries.len() as u64) {
        out.push(format!("the summary {summary} is not the candidates'"));
    }
    if keys.windows(2).any(|w| w[0] > w[1]) {
        out.push("candidates are not in MF5's order".to_owned());
    }
    out.extend(digest_problem(doc));
    out
}

/// MF7: the digest of the document without it and without labels; for
/// v2 (FM5), without the original's labels and every `measured` object
/// too.
pub fn digest(doc: &Value) -> String {
    let v2 = doc["format_version"] == 2;
    let mut bare = doc.clone();
    if let Some(map) = bare.as_object_mut() {
        map.remove("digest");
        for c in map
            .get_mut("candidates")
            .and_then(Value::as_array_mut)
            .into_iter()
            .flatten()
        {
            if let Some(entry) = c.as_object_mut() {
                entry.remove("labels");
            }
        }
        if v2 {
            if let Some(Value::Object(original)) = map.get_mut("original") {
                original.remove("labels");
            }
        }
    }
    if v2 {
        strip_measured(&mut bare);
    }
    format!(
        "sha256:{}",
        fitment_core::sha256_hex(bare.to_string().as_bytes())
    )
}

/// FM5: every `measured` object of `value`, removed.
fn strip_measured(value: &mut Value) {
    match value {
        Value::Object(map) => {
            map.remove("measured");
            map.values_mut().for_each(strip_measured);
        }
        Value::Array(items) => items.iter_mut().for_each(strip_measured),
        _ => {}
    }
}

fn s(v: &Value) -> &str {
    v.as_str().unwrap_or("")
}

fn array(v: &Value) -> &[Value] {
    v.as_array().map_or(&[], Vec::as_slice)
}

/// The stage a candidate reached: its sockets' least advanced.
fn stage_rank(c: &Value) -> u8 {
    array(&c["sockets"])
        .iter()
        .map(|socket| match s(&socket["stage"]) {
            "verification" => 0,
            "alignment" => 1,
            "shortlist" => 2,
            _ => 3,
        })
        .max()
        .unwrap_or(3)
}

fn failed_count(c: &Value) -> u64 {
    array(&c["sockets"])
        .iter()
        .map(|socket| socket["failed"].as_u64().unwrap_or(0))
        .sum()
}

/// One candidate: its sockets, its verdict from theirs and its fit, its
/// reasons and its flags.
fn candidate(doc: &Value, id: &str, c: &Value, out: &mut Vec<String>) {
    let refused = c["status"] == "refused";
    let envelope = array(&doc["socket"]["sockets"]);
    let results = array(&c["sockets"]);
    if !refused && results.len() != envelope.len() {
        out.push(format!(
            "{id}: {} socket results for {} sockets",
            results.len(),
            envelope.len()
        ));
    }
    let rejected = s(&c["verdict"]) == "reject";
    let mut verdicts = BTreeSet::new();
    for (socket, stated) in results.iter().zip(envelope) {
        verdicts.insert(socket_verdict(id, socket, stated, rejected, out));
    }
    let fit = s(&c["fit"]["status"]);
    let worst = if refused || verdicts.is_empty() || verdicts.contains("reject") {
        "reject"
    } else if verdicts.contains("interface_match") {
        "interface_match"
    } else {
        "match"
    };
    // A body that interferes rejects; only a verified one matches (VD4).
    let worst = match (worst, fit) {
        (_, "interferes") => "reject",
        ("match", f) if f != "verified" => "interface_match",
        (w, _) => w,
    };
    let verdict = s(&c["verdict"]);
    if verdict != worst {
        out.push(format!("{id}: {verdict}, its sockets say {worst}"));
    }
    let interferes = results.iter().any(|r| r["fit"]["status"] == "interferes");
    if interferes != (fit == "interferes") {
        out.push(format!("{id}: its fit {fit} is not its sockets'"));
    }
    if rejected == array(&c["reasons"]).is_empty() {
        out.push(format!(
            "{id}: {verdict} with {} reasons",
            array(&c["reasons"]).len()
        ));
    }
    verified_fit(doc, id, &c["fit"], out);
    for socket in results {
        let name = format!("{id} {}", socket["socket"]);
        verified_fit(doc, &name, &socket["fit"], out);
        freedom(doc, &name, socket, out);
    }
    flags(doc, id, c, out);
}

/// The fit's rules a `verified` must meet (`docs/research/m7-fit.md`,
/// FT9, FC1, FM1 to FM3, FT12; the review of M7 PR 1b): only in v2, with a
/// verifier, by its kernel and version, every control verified, with its
/// pairs and what no fit checks.
fn verified_fit(doc: &Value, name: &str, fit: &Value, out: &mut Vec<String>) {
    if fit["status"] != "verified" {
        return;
    }
    let verifier = &doc["fit_verifier"];
    if doc["format_version"] != 2 || !verifier.is_object() {
        out.push(format!("{name}: verified with no fit verifier"));
        return;
    }
    if fit["kernel"] != verifier["kernel"] || fit["version"] != verifier["kernel_version"] {
        out.push(format!(
            "{name}: verified by {} {}, not the fit verifier's",
            fit["kernel"], fit["version"]
        ));
    }
    let controls = array(&doc["fit"]["control"]);
    if controls.is_empty() || controls.iter().any(|c| c["fit"]["status"] != "verified") {
        out.push(format!("{name}: verified, and the control is not"));
    }
    if !fit["pairs"].is_array() || !fit["scope_not_checked"].is_array() {
        out.push(format!("{name}: verified with no pairs or scope"));
    }
}

/// FT10, FV6: in v2, a socket whose placement has a free motion other than
/// a turn about an axis the candidate is axisymmetric about is
/// `not_verified`, `placement_not_unique`; a verified socket's freedom
/// was worked out.
fn freedom(doc: &Value, name: &str, socket: &Value, out: &mut Vec<String>) {
    if doc["format_version"] != 2 {
        return;
    }
    let motions = &socket["placement"]["freedom"];
    let fit = &socket["fit"];
    if fit["status"] == "verified" && !motions.is_array() {
        out.push(format!("{name}: verified with no freedom worked out"));
    }
    let free = array(motions)
        .iter()
        .any(|m| m["kind"] != "turn" || m["axisymmetric"] != true);
    if free && (fit["status"] != "not_verified" || fit["code"] != "placement_not_unique") {
        out.push(format!(
            "{name}: a free placement with a fit {}",
            fit["status"]
        ));
    }
}

/// One socket's result: its checks against the envelope, `failed`, the
/// stage, and the verdict from them and its fit.
fn socket_verdict<'v>(
    id: &str,
    socket: &'v Value,
    stated: &Value,
    rejected: bool,
    out: &mut Vec<String>,
) -> &'v str {
    let name = format!("{id} {}", socket["socket"]);
    if socket["socket"] != stated["id"] {
        out.push(format!("{name}: not the envelope's {}", stated["id"]));
    }
    let checks = array(&socket["checks"]);
    let removed: BTreeSet<&str> = array(&stated["removed"])
        .iter()
        .map(|r| s(&r["requirement"]))
        .collect();
    let mut checked: u64 = 0;
    let mut failed: u64 = 0;
    let mut not_checked = BTreeSet::new();
    for check in checks {
        let requirement = s(&check["requirement"]);
        match s(&check["result"]) {
            "not_checked" => {
                not_checked.insert(requirement);
            }
            result => {
                checked += 1;
                failed += u64::from(result == "failed");
                one_check(&name, check, result, rejected, out);
            }
        }
    }
    if Some(checked) != stated["active"].as_u64() || not_checked != removed {
        out.push(format!(
            "{name}: {checked} checks and {} not checked, for {} active and {} removed",
            not_checked.len(),
            stated["active"],
            removed.len()
        ));
    }
    if socket["failed"].as_u64() != Some(failed) {
        out.push(format!(
            "{name}: failed {} counts {failed}",
            socket["failed"]
        ));
    }
    let stage = s(&socket["stage"]);
    let verdict = s(&socket["verdict"]);
    let fit = s(&socket["fit"]["status"]);
    let expected = if failed > 0 || checked == 0 || stage != "verification" || fit == "interferes" {
        "reject"
    } else if fit == "verified" {
        "match"
    } else {
        "interface_match"
    };
    if verdict != expected {
        out.push(format!("{name}: says {verdict}, recomputes {expected}"));
    }
    // The verifier is asked only where the interface passed (VD4).
    if expected == "reject" && fit != "not_checked" && fit != "interferes" {
        out.push(format!("{name}: a rejected interface with a fit {fit}"));
    }
    verdict
}

/// One check: its measures recomputed; its result from them and its
/// failures; in full unless a rejected candidate's pass (M4-19).
fn one_check(name: &str, check: &Value, result: &str, rejected: bool, out: &mut Vec<String>) {
    let requirement = &check["requirement"];
    let full = check.get("measures").is_some() && check.get("failures").is_some();
    if !full {
        if !(rejected && result == "passed") {
            out.push(format!("{name}: {requirement} has no full record"));
        }
        return;
    }
    let mut ok = array(&check["failures"]).is_empty();
    for m in array(&check["measures"]) {
        ok &= measure(name, m, out);
    }
    match result {
        "passed" if !ok => out.push(format!("{name}: {requirement} passed with a failure")),
        "failed" if ok => out.push(format!("{name}: {requirement} failed with nothing failing")),
        "passed" | "failed" => {}
        other => out.push(format!("{name}: {requirement} has a result {other}")),
    }
}

/// One measure's result recomputed from its found value and limits.
fn measure(id: &str, m: &Value, out: &mut Vec<String>) -> bool {
    let passed = match (m["found"].as_f64(), m["margin"].as_f64()) {
        (Some(f), Some(margin)) => {
            let below = m["min"].as_f64().map(|lo| f - lo);
            let above = m["max"].as_f64().map(|hi| hi - f);
            let recomputed = match (below, above) {
                (Some(a), Some(b)) => a.min(b),
                (Some(a), None) | (None, Some(a)) => a,
                (None, None) => f64::MIN,
            };
            if (recomputed - margin).abs() > 1e-12 * (1.0 + margin.abs()) {
                out.push(format!("{id}: a margin {margin} is not {recomputed}"));
            }
            margin > 0.0
        }
        _ => m["found"].as_bool().unwrap_or(false) || m["required"] == false,
    };
    if m["passed"].as_bool() != Some(passed) {
        out.push(format!(
            "{id}: a measure says {} but recomputes {passed}",
            m["passed"]
        ));
    }
    passed
}

/// VD12's flags that follow from the document: material never compared,
/// an incomplete read, requirements removed or unreviewed in any socket.
fn flags(doc: &Value, id: &str, c: &Value, out: &mut Vec<String>) {
    let stated: BTreeSet<&str> = array(&c["flags"]).iter().map(s).collect();
    let envelope = array(&doc["socket"]["sockets"]);
    let implied = [
        ("material_not_compared", true),
        ("candidate_incomplete", c["read"]["complete"] == false),
        (
            "requirements_removed",
            envelope.iter().any(|e| !array(&e["removed"]).is_empty()),
        ),
        (
            "requirements_unreviewed",
            envelope.iter().any(|e| e["unreviewed"].as_u64() > Some(0)),
        ),
    ];
    for (flag, set) in implied {
        if stated.contains(flag) != set {
            out.push(format!("{id}: flag {flag} is {}", !set));
        }
    }
    if array(&c["flags"]).len() != stated.len() {
        out.push(format!("{id}: a flag twice"));
    }
}

/// The digest stated is the document's.
fn digest_problem(doc: &Value) -> Option<String> {
    let digest = digest(doc);
    if doc["digest"] != digest.as_str() {
        return Some(format!("the digest {} is not {digest}", doc["digest"]));
    }
    None
}
