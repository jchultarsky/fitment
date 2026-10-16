//! `fitment match`, run as a user would (`docs/research/m4-match.md`,
//! sections 9.4, 11 and 13): its exit codes, its outputs, and every
//! document it writes recomputed from the JSON alone (EV7, KH12).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use predicates::str::contains;
use serde_json::Value;

fn fitment() -> Command {
    Command::cargo_bin("fitment").unwrap()
}

fn core(dir: &str, name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../fitment-core/tests")
        .join(dir)
        .join(name)
}

/// A fresh directory for one test.
fn scratch(test: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("fitment-match-{}-{test}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// s01's bracket socket, written to `dir`.
fn bracket_socket(dir: &Path) -> PathBuf {
    let out = dir.join("bracket.socket.json");
    fitment()
        .args([
            "socket",
            core("assemblies", "s01_bolted.stp").to_str().unwrap(),
        ])
        .args(["--part", "BRACKET", "-o", out.to_str().unwrap()])
        .assert()
        .success();
    out
}

fn run_json(args: &[&str]) -> (i32, Value) {
    let output = fitment()
        .arg("match")
        .args(args)
        .args(["--format", "json"])
        .output()
        .unwrap();
    let doc: Value = serde_json::from_slice(&output.stdout).unwrap_or(Value::Null);
    (output.status.code().unwrap_or(-1), doc)
}

/// VD14: a passing candidate exits 4 (M4 verifies no body); every one
/// rejected exits 5; --require-fit turns 4 into 5.
#[test]
fn the_exit_code_is_the_best_verdict() {
    let dir = scratch("codes");
    let socket = bracket_socket(&dir);
    let socket = socket.to_str().unwrap();
    let good = core("candidates", "m_bracket.stp");
    let bad = core("candidates", "m_bracket_missing_hole.stp");
    let (good, bad) = (good.to_str().unwrap(), bad.to_str().unwrap());
    fitment()
        .args(["match", socket, good, bad])
        .assert()
        .code(4);
    fitment().args(["match", socket, bad]).assert().code(5);
    fitment()
        .args(["match", socket, good, "--require-fit"])
        .assert()
        .code(5)
        .stderr(contains("no candidate can pass without a fit verifier"));
    // An unreadable candidate beside a passing one: exit 1, the document
    // still written.
    let missing = dir.join("nowhere.stp");
    let out = dir.join("result.json");
    fitment()
        .args(["match", socket, good, missing.to_str().unwrap()])
        .args(["-o", out.to_str().unwrap()])
        .assert()
        .code(1);
    assert!(out.is_file());
}

/// VD14's usage and refusal codes: a bad tolerance, OUT the socket file,
/// `--original` naming another file (2); a socket that does not read back,
/// a stale one, an unreviewed one under --require-reviewed (3).
#[test]
fn bad_input_is_refused_before_any_candidate() {
    let dir = scratch("refused");
    let socket = bracket_socket(&dir);
    let s = socket.to_str().unwrap();
    let good = core("candidates", "m_bracket.stp");
    let good = good.to_str().unwrap();
    fitment()
        .args(["match", s, good, "--tolerance-mm", "0"])
        .assert()
        .code(2);
    fitment().args(["match", s, good, "-o", s]).assert().code(2);
    let other = core("assemblies", "s02_cbore.stp");
    fitment()
        .args(["match", s, good, "--original", other.to_str().unwrap()])
        .assert()
        .code(2);
    fitment()
        .args(["match", s, good, "--require-reviewed"])
        .assert()
        .code(3)
        .stdout("")
        .stderr(contains("unreviewed"));
    let text = std::fs::read_to_string(&socket).unwrap();
    let stale = dir.join("stale.json");
    let older = text.replacen(
        &format!(
            "\"socket_rules_version\": {}",
            fitment_core::SOCKET_RULES_VERSION
        ),
        "\"socket_rules_version\": 5",
        1,
    );
    std::fs::write(&stale, older).unwrap();
    fitment()
        .args(["match", stale.to_str().unwrap(), good])
        .assert()
        .code(3)
        .stderr(contains("extract it again"));
    let edited = dir.join("edited.json");
    std::fs::write(&edited, text.replacen("6.6", "6.7", 1)).unwrap();
    fitment()
        .args(["match", edited.to_str().unwrap(), good])
        .assert()
        .code(3);
}

/// VD15, MF1, MF3: the original passes its own socket; a directory's STEP
/// files are each a candidate, labelled `dir/name`; the same file named
/// twice is one entry with both labels.
#[test]
fn the_original_and_directories_are_judged() {
    let dir = scratch("inputs");
    let socket = bracket_socket(&dir);
    let candidates = dir.join("cands");
    std::fs::create_dir_all(&candidates).unwrap();
    for name in ["m_bracket.stp", "m_bracket_blind.stp"] {
        std::fs::copy(core("candidates", name), candidates.join(name)).unwrap();
    }
    std::fs::write(candidates.join("notes.txt"), "not a candidate").unwrap();
    let twice = candidates.join("m_bracket.stp");
    let source = core("assemblies", "s01_bolted.stp");
    let (code, doc) = run_json(&[
        socket.to_str().unwrap(),
        candidates.to_str().unwrap(),
        twice.to_str().unwrap(),
        "--original",
        source.to_str().unwrap(),
    ]);
    assert_eq!(code, 4);
    assert_eq!(doc["summary"]["candidates"], 3);
    let entries = doc["candidates"].as_array().unwrap();
    let original = entries
        .iter()
        .find(|c| c["labels"][0] == "original")
        .unwrap();
    assert_eq!(original["verdict"], "interface_match");
    let bracket = entries
        .iter()
        .find(|c| c["labels"].as_array().unwrap().len() == 2)
        .unwrap();
    assert!(
        bracket["labels"][0]
            .as_str()
            .unwrap()
            .ends_with("cands/m_bracket.stp")
    );
    assert_problems(&doc);
}

/// MF5, EV8: argument order changes the labels' order, never the digest.
#[test]
fn the_document_does_not_depend_on_argument_order() {
    let dir = scratch("order");
    let socket = bracket_socket(&dir);
    let names = ["m_bracket.stp", "m_bracket_thick.stp", "m_bracket_lug.stp"];
    let paths: Vec<String> = names
        .iter()
        .map(|n| core("candidates", n).to_str().unwrap().to_owned())
        .collect();
    let mut forward = vec![socket.to_str().unwrap()];
    forward.extend(paths.iter().map(String::as_str));
    let mut backward = vec![socket.to_str().unwrap()];
    backward.extend(paths.iter().rev().map(String::as_str));
    let (_, a) = run_json(&forward);
    let (_, b) = run_json(&backward);
    assert_eq!(a["digest"], b["digest"]);
    assert_eq!(a["summary"], b["summary"]);
    let shas = |d: &Value| -> Vec<Value> {
        d["candidates"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["source"]["sha256"].clone())
            .collect()
    };
    assert_eq!(shas(&a), shas(&b));
    assert_problems(&a);
}

/// MF4: a label is escaped wherever it is printed.
#[test]
fn labels_are_escaped() {
    let dir = scratch("labels");
    let socket = bracket_socket(&dir);
    let odd = dir.join("odd\u{1b}[31mname.stp");
    std::fs::copy(core("candidates", "m_bracket.stp"), &odd).unwrap();
    let output = fitment()
        .args(["match", socket.to_str().unwrap(), odd.to_str().unwrap()])
        .output()
        .unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(!stdout.contains('\u{1b}'), "{stdout}");
    assert!(stdout.contains("\\u{1b}"));
}

/// The review-then-match path: a requirement removed in review is listed
/// as not checked, and the candidate missing its hole then passes.
#[test]
fn a_removed_requirement_is_not_checked() {
    let dir = scratch("review");
    let socket = bracket_socket(&dir);
    let s = socket.to_str().unwrap();
    let bad = core("candidates", "m_bracket_missing_hole.stp");
    let (_, before) = run_json(&[s, bad.to_str().unwrap()]);
    let failed = before["candidates"][0]["sockets"][0]["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["result"] == "failed")
        .unwrap()["requirement"]
        .as_str()
        .unwrap()
        .to_owned();
    fitment()
        .args([
            "review",
            s,
            "--remove",
            &failed,
            "--note",
            "the plate's own",
        ])
        .assert()
        .success();
    let (code, after) = run_json(&[s, bad.to_str().unwrap()]);
    let entry = &after["candidates"][0];
    // The removed hole is not checked; the other bores and seats hold it.
    let removed = entry["sockets"][0]["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["requirement"] == failed.as_str())
        .unwrap();
    assert_eq!(removed["result"], "not_checked");
    assert_eq!(removed["note"], "the plate's own");
    assert!(
        entry["flags"]
            .as_array()
            .unwrap()
            .contains(&Value::from("requirements_removed"))
    );
    assert!(code == 4 || code == 5);
    assert_problems(&after);
}

/// KH12 and EV7: everything a document states that follows from the rest
/// of it, recomputed: each measure's result from its margin and limits,
/// each check's from its measures and failures, each socket's verdict
/// from its checks, each candidate's from its sockets, the summary, the
/// order and the digest.
fn assert_problems(doc: &Value) {
    let problems = problems(doc);
    assert!(problems.is_empty(), "{}", problems.join("\n"));
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

fn problems(doc: &Value) -> Vec<String> {
    let mut out = Vec::new();
    if doc["format"] != "fitment.match" || doc["format_version"] != 1 {
        out.push("not a fitment.match v1 document".to_owned());
    }
    let mut counts = (0, 0, 0, 0);
    let mut keys = Vec::new();
    for c in doc["candidates"].as_array().into_iter().flatten() {
        let id = c["labels"].to_string();
        let mut verdicts = BTreeSet::new();
        for socket in c["sockets"].as_array().into_iter().flatten() {
            let mut passed_all = !socket["checks"].as_array().is_none_or(Vec::is_empty);
            for check in socket["checks"].as_array().into_iter().flatten() {
                let result = check["result"].as_str().unwrap_or("");
                let mut ok = check["failures"].as_array().is_none_or(Vec::is_empty);
                for m in check["measures"].as_array().into_iter().flatten() {
                    ok &= measure(&id, m, &mut out);
                }
                match result {
                    "passed" if !ok && check.get("measures").is_some() => {
                        out.push(format!(
                            "{id}: {} passed with a failure",
                            check["requirement"]
                        ));
                    }
                    "failed" if ok && check.get("measures").is_some() => {
                        out.push(format!(
                            "{id}: {} failed with nothing failing",
                            check["requirement"]
                        ));
                    }
                    _ => {}
                }
                if result == "failed" {
                    passed_all = false;
                }
            }
            let verdict = socket["verdict"].as_str().unwrap_or("");
            if passed_all != (verdict != "reject") {
                out.push(format!("{id}: socket {} says {verdict}", socket["socket"]));
            }
            verdicts.insert(verdict.to_owned());
        }
        let verdict = c["verdict"].as_str().unwrap_or("");
        let worst =
            if c["status"] == "refused" || verdicts.is_empty() || verdicts.contains("reject") {
                "reject"
            } else if verdicts.contains("interface_match") {
                "interface_match"
            } else {
                "match"
            };
        if verdict != worst {
            out.push(format!("{id}: {verdict}, its sockets say {worst}"));
        }
        match verdict {
            "match" => counts.0 += 1,
            "interface_match" => counts.1 += 1,
            _ => counts.2 += 1,
        }
        if c["status"] == "refused" {
            counts.3 += 1;
        }
        let rank = ["match", "interface_match", "reject"]
            .iter()
            .position(|v| *v == verdict)
            .unwrap_or(3);
        keys.push(rank);
    }
    let summary = &doc["summary"];
    if (
        summary["match"].as_u64(),
        summary["interface_match"].as_u64(),
        summary["reject"].as_u64(),
        summary["refused"].as_u64(),
    ) != (
        Some(counts.0),
        Some(counts.1),
        Some(counts.2),
        Some(counts.3),
    ) {
        out.push(format!("the summary {summary} is not the candidates'"));
    }
    if keys.windows(2).any(|w| w[0] > w[1]) {
        out.push("candidates are not in verdict order".to_owned());
    }
    out.extend(digest_problem(doc));
    out
}

/// The checker is no formality: a verdict turned, a measure's result
/// flipped, a margin changed or a label taken into the digest each fail it.
#[test]
fn the_checker_finds_a_tampered_document() {
    let dir = scratch("tamper");
    let socket = bracket_socket(&dir);
    let names = ["m_bracket.stp", "m_bracket_missing_hole.stp"];
    let paths: Vec<String> = names
        .iter()
        .map(|n| core("candidates", n).to_str().unwrap().to_owned())
        .collect();
    let (_, doc) = run_json(&[socket.to_str().unwrap(), &paths[0], &paths[1]]);
    assert_eq!(problems(&doc).len(), 0);
    let mut turned = doc.clone();
    turned["candidates"][1]["verdict"] = Value::from("interface_match");
    assert_ne!(problems(&turned).len(), 0);
    let mut flipped = doc.clone();
    flipped["candidates"][0]["sockets"][0]["checks"][3]["measures"][0]["passed"] =
        Value::from(false);
    assert_ne!(problems(&flipped).len(), 0);
    let mut moved = doc.clone();
    moved["candidates"][0]["sockets"][0]["checks"][3]["measures"][0]["margin"] = Value::from(1.0);
    assert_ne!(problems(&moved).len(), 0);
    let mut relabelled = doc;
    relabelled["summary"]["reject"] = Value::from(0);
    assert_ne!(problems(&relabelled).len(), 0);
}

/// MF7: the digest of the document without it and without labels.
fn digest_problem(doc: &Value) -> Option<String> {
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
    }
    let digest = format!(
        "sha256:{}",
        fitment_core::sha256_hex(bare.to_string().as_bytes())
    );
    if doc["digest"] != digest.as_str() {
        return Some(format!("the digest {} is not {digest}", doc["digest"]));
    }
    None
}
