//! The corpus's match cases (`docs/research/m4-match.md`, sections 15 and
//! 16): every candidate the generator built, judged through the library
//! (KH1) against its source's sockets under both policies M4 offers, held
//! to the strict gate (KH2, KH3) and its defect at the construction's
//! placement (KH8); the false negatives against the committed baseline
//! (KH4, KH5); the report and its rates (KH6, KH7); the shortlist never
//! changing a verdict (KH9); every run the same twice (KH11); every file
//! judged from its stored document as from its bytes, and every pass
//! admitted by level 1 (`docs/research/m5-catalog.md`, CT5, QR3). Without
//! the corpus these pass with a note, unless `FITMENT_REQUIRE_CORPUS=1`.

#[path = "../../fitment-core/tests/support/gate.rs"]
mod gate;

#[path = "common/checker.rs"]
mod checker;

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use fitment_core::{
    Admitted, CandidateResult, CandidateSource, Clearance, ComponentRead, FeaturesDocument, Grip,
    IndexTokens, MatchDocument, MatchOptions, MatchPolicy, Matcher, NoFitVerifier, Part,
    PartSignature, ShortlistIndex, Socket, SocketDocument, Source, Stage, TolerancePolicy,
    VerdictKind, read_assembly,
};
use serde_json::{Value, json};

fn corpus() -> Option<PathBuf> {
    let dir = std::env::var_os("FITMENT_CORPUS").map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/out"),
        PathBuf::from,
    );
    let required = std::env::var_os("FITMENT_REQUIRE_CORPUS").is_some_and(|v| v == "1");
    if dir.join("manifest.json").is_file() {
        return Some(dir);
    }
    assert!(!required, "FITMENT_REQUIRE_CORPUS=1 but there is no corpus");
    eprintln!("skipped: no corpus; run tools/gen-corpus.py corpus/out");
    None
}

const POLICIES: [&str; 2] = ["default", "functional"];

fn policy(name: &str) -> MatchPolicy {
    match name {
        "functional" => MatchPolicy::new(
            TolerancePolicy::default(),
            Clearance::Functional,
            Grip::Hard,
        ),
        _ => MatchPolicy::default(),
    }
}

/// A case's candidate (KC2): a file's bytes, or a component read in place
/// from a corpus assembly (the target of its own socket, VD5).
enum Input {
    File(Vec<u8>),
    Component(Box<Part>, String, u64),
}

impl Input {
    fn read(dir: &Path, case: &Value) -> Self {
        let candidate = &case["candidate"];
        if let Some(file) = candidate["file"].as_str() {
            return Self::File(fs::read(dir.join(file)).unwrap());
        }
        let component = &candidate["component"];
        let source = component["case"].as_str().unwrap();
        assert!(
            component["occurrence"].is_null(),
            "a component by occurrence"
        );
        let bytes = fs::read(dir.join(format!("{source}.stp"))).unwrap();
        let read = read_assembly(&bytes);
        let listing = read.listing().unwrap();
        let selection = listing.select(component["part"].as_str(), None).unwrap();
        let definition = &listing.definitions()[selection.definition];
        let Some(ComponentRead::Accepted(part)) = &definition.read else {
            panic!("{source}: the component is not read");
        };
        let instance = definition.definition.instance;
        Self::Component(part.clone(), fitment_core::sha256_hex(&bytes), instance)
    }

    fn judge(&self, matcher: &Matcher<'_>) -> CandidateResult {
        match self {
            Self::File(bytes) => matcher.judge_step(bytes),
            Self::Component(part, sha, instance) => {
                matcher.judge_part(part, CandidateSource::component(sha.clone(), *instance))
            }
        }
    }

    /// CT5: judged from the document a catalog stores, read back from its
    /// compact form, the file gets the result its bytes get; its tokens
    /// read back as written (IX9). `None` for a component, which no catalog
    /// stores.
    fn judge_stored(&self, matcher: &Matcher<'_>) -> Option<CandidateResult> {
        let Self::File(bytes) = self else {
            return None;
        };
        let read = FeaturesDocument::read(bytes);
        let back = FeaturesDocument::from_json(&read.to_compact_json()).unwrap();
        if let Some(part) = back.part() {
            let tokens = IndexTokens::of(&PartSignature::of(part));
            assert_eq!(IndexTokens::from_json(&tokens.to_json()).unwrap(), tokens);
        }
        let source = Source::new(
            &fitment_core::sha256_hex(bytes),
            u64::try_from(bytes.len()).unwrap(),
            back.header().cloned(),
        )
        .unwrap();
        Some(matcher.judge_document(&back, source).unwrap())
    }

    fn part(&self) -> Option<Part> {
        match self {
            Self::File(bytes) => fitment_core::read_part(bytes).ok(),
            Self::Component(part, ..) => Some((**part).clone()),
        }
    }
}

/// A case of the socket gate (VD9, MF2): refused before any candidate,
/// so it runs through the binary alone.
fn socket_gate(case: &Value) -> bool {
    gate::expectation(case, "default")["verdict"] == "socket_refused"
}

/// One case judged under one policy, with what the harness asks of it.
struct Run {
    case: Value,
    policy: &'static str,
    result: CandidateResult,
    sockets: Vec<Socket>,
    /// The gate's and KH8's problems.
    problems: Vec<String>,
    /// KH9: the verdict with the shortlist off.
    without_shortlist: VerdictKind,
    /// KH11: the document twice, byte for byte.
    same_twice: bool,
    /// CT5: the result judged from the stored document, if a file.
    stored: Option<CandidateResult>,
    /// Whether level 1, over an index of this candidate alone, admits it.
    admitted: bool,
    /// KH8: every check passes at the construction's placement.
    passes_at_construction: bool,
}

/// Every run, computed once per test binary.
fn runs() -> Option<&'static Vec<Run>> {
    static RUNS: OnceLock<Option<Vec<Run>>> = OnceLock::new();
    RUNS.get_or_init(|| corpus().map(|dir| compute(&dir)))
        .as_ref()
}

fn compute(dir: &Path) -> Vec<Run> {
    let text = fs::read_to_string(dir.join("manifest.json")).unwrap();
    let manifest: Value = serde_json::from_str(&text).unwrap();
    let cases: Vec<&Value> = manifest["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["kind"] == "match" && !socket_gate(c))
        .collect();
    let mut documents: BTreeMap<(String, String), SocketDocument> = BTreeMap::new();
    let mut out = Vec::new();
    for case in cases {
        let source = case["socket"]["case"].as_str().unwrap().to_owned();
        let part = case["socket"]["part"].as_str().unwrap().to_owned();
        let document = documents
            .entry((source.clone(), part.clone()))
            .or_insert_with(|| {
                let bytes = fs::read(dir.join(format!("{source}.stp"))).unwrap();
                SocketDocument::extract(&bytes, Some(&part), None).unwrap()
            });
        let sockets = document.extracted().unwrap().sockets.clone();
        let input = Input::read(dir, case);
        for name in POLICIES {
            let matcher = Matcher::new(
                document,
                policy(name),
                MatchOptions::default(),
                &NoFitVerifier,
            )
            .unwrap();
            let result = input.judge(&matcher);
            let mut problems = gate::gate(case, name, &sockets, &result);
            let part = input.part();
            let at = gate::placement(case).unwrap();
            let built: Vec<_> = part
                .iter()
                .flat_map(|p| {
                    sockets
                        .iter()
                        .filter_map(|s| matcher.verify_at(p, &s.id, &at))
                })
                .collect();
            if !case["defect"].as_array().is_none_or(Vec::is_empty) {
                problems.extend(gate::defect(case, &sockets, &built));
            }
            let passes_at_construction = !built.is_empty()
                && built
                    .iter()
                    .all(|s| s.checks.iter().all(|(_, c)| c.passed()));
            let label = vec![case["id"].as_str().unwrap().to_owned()];
            let once = MatchDocument::new(&matcher, vec![(result.clone(), label.clone())]);
            let again = MatchDocument::new(&matcher, vec![(input.judge(&matcher), label)]);
            let admitted = part.as_ref().is_some_and(|p| {
                let mut index = ShortlistIndex::new();
                index.insert(0, &IndexTokens::of(&PartSignature::of(p)));
                matches!(
                    matcher.admitted(&index),
                    Ok(Admitted::Parts { candidates, .. }) if candidates == [0]
                )
            });
            let unshortlisted = Matcher::new(
                document,
                policy(name),
                MatchOptions::default(),
                &NoFitVerifier,
            )
            .unwrap()
            .without_shortlist();
            out.push(Run {
                case: case.clone(),
                policy: name,
                without_shortlist: input.judge(&unshortlisted).verdict.kind(),
                same_twice: once.to_json() == again.to_json(),
                stored: input.judge_stored(&matcher),
                admitted,
                result,
                sockets: sockets.clone(),
                problems,
                passes_at_construction,
            });
        }
    }
    out
}

/// KH1's library path, KH2, KH3 and KH8 over every match case.
#[test]
fn every_match_case_meets_its_construction() {
    let Some(runs) = runs() else {
        return;
    };
    assert!(runs.len() > 40, "{} runs", runs.len());
    let problems: Vec<&String> = runs.iter().flat_map(|r| &r.problems).collect();
    assert!(
        problems.is_empty(),
        "{}",
        problems
            .iter()
            .map(|p| p.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// The truth a case's construction states fits under `policy`.
fn fits(case: &Value, policy: &str) -> bool {
    match case["truth"].as_str().unwrap_or("") {
        "fits" | "fn_rule" | "collides" => true,
        "fits_functional" => policy == "functional",
        _ => false,
    }
}

/// A run that claims a fit the construction says does not hold (KH7).
fn false_positive(run: &Run) -> bool {
    run.result.verdict.kind() != VerdictKind::Reject && !fits(&run.case, run.policy)
}

/// KH4: one entry per (case, policy) that fits and is rejected: the
/// requirements that failed, with their codes, and where the rejection
/// comes from: a stated rule, alignment (the construction's placement
/// passes, KH8), or verification.
fn false_negatives(runs: &[Run]) -> Vec<Value> {
    let mut out: Vec<Value> = runs
        .iter()
        .filter(|r| fits(&r.case, r.policy) && r.result.verdict.kind() == VerdictKind::Reject)
        .map(|r| {
            let failed: Vec<Value> = r
                .result
                .sockets
                .iter()
                .flat_map(|s| {
                    s.checks.iter().filter(|(_, c)| !c.passed()).map(move |(id, c)| {
                        let mut codes: Vec<String> = c
                            .evidence()
                            .failures()
                            .iter()
                            .filter_map(|f| serde_json::to_value(f.code).ok())
                            .filter_map(|v| v.as_str().map(str::to_owned))
                            .collect();
                        codes.sort();
                        codes.dedup();
                        json!({"requirement": format!("{}/{}", s.socket, id.as_str()), "codes": codes})
                    })
                })
                .collect();
            let cause = if r.case["truth"] == "fn_rule" || r.case["truth"] == "fits_functional" {
                "rule"
            } else if r.passes_at_construction {
                "alignment"
            } else {
                "verification"
            };
            json!({"case": r.case["id"], "policy": r.policy, "class": r.case["class"],
                   "failed": failed, "cause": cause})
        })
        .collect();
    out.sort_by_key(|v| (v["case"].to_string(), v["policy"].to_string()));
    out
}

fn baseline_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/false-negatives.json")
}

/// KH4, KH5: the false negatives are exactly the committed baseline.
/// `FITMENT_BLESS=1` rewrites it, never under `CI`, and never to add an
/// entry for a case the baseline already knew unless the matching rules'
/// version rose: a new false negative comes from a new case or a rule
/// change, never from an unrelated edit.
#[test]
fn the_false_negatives_are_the_baseline() {
    let Some(runs) = runs() else {
        return;
    };
    let found = false_negatives(runs);
    let cases: Vec<Value> = {
        let mut ids: Vec<Value> = runs.iter().map(|r| r.case["id"].clone()).collect();
        ids.dedup();
        ids
    };
    let document = json!({
        "match_rules_version": fitment_core::MATCH_RULES_VERSION,
        "signature_version": fitment_core::SIGNATURE_VERSION,
        "extractor_version": fitment_core::EXTRACTOR_VERSION,
        "socket_rules_version": fitment_core::SOCKET_RULES_VERSION,
        "cases": cases,
        "false_negatives": found,
    });
    let path = baseline_path();
    let committed: Option<Value> = fs::read_to_string(&path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok());
    let bless = std::env::var_os("FITMENT_BLESS").is_some_and(|v| v == "1");
    let old = committed
        .as_ref()
        .map(|c| c["false_negatives"].as_array().cloned().unwrap_or_default())
        .unwrap_or_default();
    let added: Vec<&Value> = found.iter().filter(|f| !old.contains(f)).collect();
    let removed: Vec<&Value> = old.iter().filter(|f| !found.contains(f)).collect();
    // The whole record: a case added, or a version risen, is blessed too,
    // so the refusal below always judges against the cases and the rules
    // version of the last run.
    if committed.as_ref() == Some(&document) {
        return;
    }
    if bless {
        assert!(
            std::env::var_os("CI").is_none(),
            "the baseline is blessed locally, never in CI"
        );
        let known: Vec<Value> = committed
            .as_ref()
            .map(|c| c["cases"].as_array().cloned().unwrap_or_default())
            .unwrap_or_default();
        let same_rules = committed
            .as_ref()
            .is_some_and(|c| c["match_rules_version"] == fitment_core::MATCH_RULES_VERSION);
        let refused: Vec<&&Value> = added
            .iter()
            .filter(|f| same_rules && known.contains(&f["case"]))
            .collect();
        assert!(
            refused.is_empty(),
            "new false negatives on known cases without a rules version rise: {refused:?}"
        );
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            serde_json::to_string_pretty(&document).unwrap() + "\n",
        )
        .unwrap();
        return;
    }
    panic!(
        "the false negatives, the cases or the versions differ from {}:\nadded: {added:#?}\nremoved: {removed:#?}\n\
         if intended, run FITMENT_BLESS=1 cargo test -p fitment --test matching",
        path.display()
    );
}

/// KH6, KH7: the rates, per policy and class, written to
/// `target/fitment-corpus/report.json` and printed; false positives are
/// zero under both policies M4 offers.
#[test]
fn the_report_states_the_rates() {
    let Some(runs) = runs() else {
        return;
    };
    let mut rows: BTreeMap<(String, String), [usize; 7]> = BTreeMap::new();
    for r in runs {
        let class = r.case["class"].as_str().unwrap_or("").to_owned();
        let row = rows.entry((r.policy.to_owned(), class)).or_default();
        row[0] += 1;
        match r.result.verdict.kind() {
            VerdictKind::Match => row[1] += 1,
            VerdictKind::InterfaceMatch => row[2] += 1,
            VerdictKind::Reject => row[3] += 1,
        }
        if false_positive(r) {
            row[4] += 1;
        }
        let rejected = r.result.verdict.kind() == VerdictKind::Reject;
        if fits(&r.case, r.policy) && rejected {
            row[5] += 1;
            if r.case["truth"] == "fits" {
                row[6] += 1;
            }
        }
    }
    let mut table = Vec::new();
    let mut text = String::from(
        "policy      class               cases match inter reject FP FN(phys) FN(spec)\n",
    );
    for ((policy, class), [n, m, i, rj, fp, fnp, fns]) in &rows {
        let _ = writeln!(
            text,
            "{policy:11} {class:19} {n:5} {m:5} {i:5} {rj:6} {fp:2} {fnp:8} {fns:8}"
        );
        table.push(
            json!({"policy": policy, "class": class, "cases": n, "match": m, "interface_match": i,
                          "reject": rj, "false_positives": fp, "false_negatives_physics": fnp,
                          "false_negatives_specification": fns}),
        );
    }
    let mut totals = Vec::new();
    for policy in POLICIES {
        let of =
            |f: &dyn Fn(&Run) -> bool| runs.iter().filter(|r| r.policy == policy && f(r)).count();
        let fit = of(&|r| fits(&r.case, r.policy) && r.case["truth"] != "collides");
        let missed = of(&|r| {
            fits(&r.case, r.policy)
                && r.case["truth"] != "collides"
                && r.result.verdict.kind() == VerdictKind::Reject
        });
        let fp = of(&false_positive);
        let _ = writeln!(
            text,
            "{policy}: false positives {fp}; false negatives against physics {missed} of {fit}"
        );
        totals.push(json!({"policy": policy, "false_positives": fp, "fits": fit, "false_negatives": missed}));
    }
    eprintln!("{text}");
    let report = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/fitment-corpus");
    fs::create_dir_all(&report).unwrap();
    fs::write(
        report.join("report.json"),
        serde_json::to_string_pretty(&json!({"rows": table, "totals": totals})).unwrap(),
    )
    .unwrap();
    let fps: Vec<String> = runs
        .iter()
        .filter(|r| false_positive(r))
        .map(|r| format!("{} {}", r.case["id"], r.policy))
        .collect();
    assert!(fps.is_empty(), "false positives: {fps:?}");
}

/// KH9: the shortlist never drops what verification passes, never drops a
/// good case, and turning it off changes no verdict.
#[test]
fn the_shortlist_never_drops_what_verification_passes() {
    let Some(runs) = runs() else {
        return;
    };
    let mut problems = Vec::new();
    for r in runs {
        let id = &r.case["id"];
        if r.without_shortlist != r.result.verdict.kind() {
            problems.push(format!(
                "{id} {}: {:?} without the shortlist",
                r.policy, r.without_shortlist
            ));
        }
        let dropped = r.result.sockets.iter().any(|s| s.stage == Stage::Shortlist);
        if dropped && fits(&r.case, r.policy) && r.case["truth"] != "fn_rule" {
            problems.push(format!(
                "{id} {}: a good case dropped by the shortlist",
                r.policy
            ));
        }
        let _ = &r.sockets;
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// CT5: every file case judged from its stored document gets the result
/// its bytes get, whole.
#[test]
fn stored_documents_are_judged_as_their_files() {
    let Some(runs) = runs() else {
        return;
    };
    let mut files = 0;
    for r in runs {
        if let Some(stored) = &r.stored {
            files += 1;
            assert_eq!(stored, &r.result, "{} {}", r.case["id"], r.policy);
        }
    }
    assert!(files > 0);
}

/// M5's QR3 and section 4.17: level 1 never hides a part verification
/// passes, with the shortlist or without it.
#[test]
fn level_one_admits_every_pass() {
    let Some(runs) = runs() else {
        return;
    };
    let mut passes = 0;
    for r in runs {
        let any_pass = [r.result.verdict.kind(), r.without_shortlist]
            .iter()
            .any(|k| *k != VerdictKind::Reject);
        if any_pass {
            passes += 1;
            assert!(
                r.admitted,
                "{} {}: a pass level 1 hides",
                r.case["id"], r.policy
            );
        }
    }
    assert!(passes > 0);
}

/// KH11: every run gives the same document twice in one process.
#[test]
fn matching_is_deterministic() {
    let Some(runs) = runs() else {
        return;
    };
    let differ: Vec<String> = runs
        .iter()
        .filter(|r| !r.same_twice)
        .map(|r| format!("{} {}", r.case["id"], r.policy))
        .collect();
    assert!(differ.is_empty(), "{differ:?}");
}

/// KH1's binary path and KH12 on the corpus: each source's socket written
/// by `fitment socket`, each policy's file candidates judged by one
/// `fitment match`, whose document the checker finds sound, whose exit
/// code is its outcome's, and whose verdicts are the library's.
#[test]
fn the_binary_agrees_with_the_library() {
    let (Some(dir), Some(runs)) = (corpus(), runs()) else {
        return;
    };
    let scratch = scratch("binary");
    let mut groups: BTreeMap<(String, String), Vec<&Run>> = BTreeMap::new();
    for run in runs
        .iter()
        .filter(|r| r.case["candidate"]["file"].is_string())
    {
        let socket = &run.case["socket"];
        let key = (
            socket["case"].as_str().unwrap().to_owned(),
            socket["part"].as_str().unwrap().to_owned(),
        );
        groups.entry(key).or_default().push(run);
    }
    let mut problems = Vec::new();
    for ((source, part), group) in &groups {
        let socket = write_socket(&dir, &scratch, source, part);
        for policy in POLICIES {
            let mut files: Vec<&str> = group
                .iter()
                .filter(|r| r.policy == policy)
                .map(|r| r.case["candidate"]["file"].as_str().unwrap())
                .collect();
            files.sort_unstable();
            files.dedup();
            let output = fitment()
                .current_dir(&dir)
                .arg("match")
                .arg(&socket)
                .args(&files)
                .args([
                    "--format",
                    "json",
                    "--clearance",
                    if policy == "functional" {
                        "functional"
                    } else {
                        "strict"
                    },
                ])
                .output()
                .unwrap();
            let doc: Value = serde_json::from_slice(&output.stdout).unwrap();
            let at = format!("{source} {part} {policy}");
            // KH2.6: the document states the run's policy.
            let clearance = if policy == "functional" {
                "functional"
            } else {
                "strict"
            };
            if doc["policy"]["clearance"] != clearance || doc["policy"]["name"] != "default" {
                problems.push(format!("{at}: the document's policy is {}", doc["policy"]));
            }
            problems.extend(
                checker::problems(&doc)
                    .into_iter()
                    .map(|p| format!("{at}: {p}")),
            );
            let code = match (
                doc["summary"]["match"].as_u64(),
                doc["summary"]["interface_match"].as_u64(),
            ) {
                (Some(m), _) if m > 0 => 0,
                (_, Some(i)) if i > 0 => 4,
                _ => 5,
            };
            if output.status.code() != Some(code) {
                problems.push(format!(
                    "{at}: exit {:?}, its outcome says {code}",
                    output.status.code()
                ));
            }
            for run in group.iter().filter(|r| r.policy == policy) {
                let file = run.case["candidate"]["file"].as_str().unwrap();
                let entry = doc["candidates"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|c| c["labels"].as_array().unwrap().iter().any(|l| l == file));
                let library = serde_json::to_value(run.result.verdict.kind()).unwrap();
                if entry.map(|e| &e["verdict"]) != Some(&library) {
                    problems.push(format!(
                        "{at}: {} is {library} in the library",
                        run.case["id"]
                    ));
                }
            }
        }
    }
    let _ = fs::remove_dir_all(&scratch);
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// VD9 and MF2 on the corpus: a socket document stale, with a broken
/// digest, unreviewed under `--require-reviewed`, or refused, is refused
/// (exit 3) before any candidate is read.
#[test]
fn the_socket_gate_refuses_before_any_candidate() {
    let Some(dir) = corpus() else {
        return;
    };
    let text = fs::read_to_string(dir.join("manifest.json")).unwrap();
    let manifest: Value = serde_json::from_str(&text).unwrap();
    let scratch = scratch("gate");
    let mut seen = 0;
    let mut problems = Vec::new();
    for case in manifest["cases"].as_array().unwrap() {
        if case["kind"] != "match" || !socket_gate(case) {
            continue;
        }
        seen += 1;
        let socket = &case["socket"];
        let id = case["id"].as_str().unwrap();
        let path = write_socket(
            &dir,
            &scratch,
            socket["case"].as_str().unwrap(),
            socket["part"].as_str().unwrap(),
        );
        let mut doc: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        let mut args = vec!["match".to_owned()];
        // Each refused for its own reason, not another's.
        let reason = match socket["edit"].as_str() {
            Some("stale") => "extract it again",
            Some("digest") => "digest does not match",
            Some("unreviewed") => "unreviewed",
            _ => "holds no socket",
        };
        match socket["edit"].as_str() {
            Some("stale") => {
                doc["socket_rules_version"] =
                    json!(doc["socket_rules_version"].as_u64().unwrap() - 1);
            }
            Some("digest") => {
                let reach = &mut doc["extracted"]["sockets"][0]["requirements"][0]["id"];
                *reach = json!(format!("{}-edited", reach.as_str().unwrap()));
            }
            Some("unreviewed") => args.push("--require-reviewed".to_owned()),
            _ => {}
        }
        let edited = scratch.join(format!("{id}.socket.json"));
        fs::write(&edited, serde_json::to_string_pretty(&doc).unwrap()).unwrap();
        args.push(edited.display().to_string());
        // A candidate that cannot be read: the gate must refuse first.
        args.push(dir.join("no-such-candidate.stp").display().to_string());
        let output = fitment().args(&args).output().unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        if output.status.code() != Some(3) || !output.stdout.is_empty() || !stderr.contains(reason)
        {
            problems.push(format!(
                "{id}: exit {:?}: {}",
                output.status.code(),
                String::from_utf8_lossy(&output.stderr)
            ));
        }
    }
    assert!(seen >= 4, "{seen} socket gate cases");
    let _ = fs::remove_dir_all(&scratch);
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

fn fitment() -> std::process::Command {
    std::process::Command::new(env!("CARGO_BIN_EXE_fitment"))
}

fn scratch(test: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("fitment-matching-{test}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// `fitment socket` on a corpus source, written to `scratch`; a refused
/// document (exit 3) is kept from stdout.
fn write_socket(dir: &Path, scratch: &Path, source: &str, part: &str) -> PathBuf {
    let path = scratch.join(format!("{source}--{part}.json"));
    let output = fitment()
        .arg("socket")
        .arg(dir.join(format!("{source}.stp")))
        .args(["--part", part, "--format", "json"])
        .output()
        .unwrap();
    assert!(
        matches!(output.status.code(), Some(0 | 3)),
        "fitment socket {source} {part}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    fs::write(&path, &output.stdout).unwrap();
    path
}

/// M4-13 and AL11 on hostile inputs (10,000 identical holes, a 1,600-bore
/// socket against a 1,681-hole grid, a bracket fused from 1,000 tiles, 4,000
/// stubs near one bore): each judged within its bound in time (in an
/// optimised build), a candidate that does not fit rejected, and one that
/// fits rejected, if at all, only by a bound: never a pass from a bound.
#[test]
fn hostile_inputs_stay_within_their_bounds() {
    let Some(dir) = corpus() else {
        return;
    };
    let text = fs::read_to_string(dir.join("manifest.json")).unwrap();
    let manifest: Value = serde_json::from_str(&text).unwrap();
    let mut seen = 0;
    let mut problems = Vec::new();
    for case in manifest["cases"].as_array().unwrap() {
        if case["kind"] != "hostile" {
            continue;
        }
        seen += 1;
        let id = case["id"].as_str().unwrap();
        let source = case["socket"]["case"].as_str().unwrap();
        let part = case["socket"]["part"].as_str().unwrap();
        let bytes = fs::read(dir.join(format!("{source}.stp"))).unwrap();
        let document = SocketDocument::extract(&bytes, Some(part), None).unwrap();
        let candidate = fs::read(dir.join(case["candidate"]["file"].as_str().unwrap())).unwrap();
        let matcher = Matcher::new(
            &document,
            MatchPolicy::default(),
            MatchOptions::default(),
            &NoFitVerifier,
        )
        .unwrap();
        let start = std::time::Instant::now();
        let result = matcher.judge_step(&candidate);
        let took = start.elapsed().as_secs_f64();
        let bound = case["expect"]["within_s"].as_f64().unwrap();
        if !cfg!(debug_assertions) && took > bound {
            problems.push(format!("{id}: {took:.1} s, past its {bound} s"));
        }
        problems.extend(hostile_problems(id, case, &result));
        eprintln!("{id}: {:?} in {took:.2} s", result.verdict.kind());
    }
    assert!(seen >= 4, "{seen} hostile cases");
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// One hostile case's verdict against its truth and the bound it states:
/// a fit rejected only by a bound (read, never dropped by the shortlist,
/// every failed check carrying a bound's code); a non-fit rejected; the
/// bound it names fired, and alignment's placement bound reached where it
/// says so.
fn hostile_problems(id: &str, case: &Value, result: &CandidateResult) -> Vec<String> {
    let bounds = ["alignment_exhausted", "coverage_exhausted"];
    let codes_of = |c: &fitment_core::Check| -> Vec<String> {
        c.evidence()
            .failures()
            .iter()
            .filter_map(|f| serde_json::to_value(f.code).ok())
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect()
    };
    let failed: Vec<Vec<String>> = result
        .sockets
        .iter()
        .flat_map(|s| s.checks.iter())
        .filter(|(_, c)| !c.passed())
        .map(|(_, c)| codes_of(c))
        .collect();
    let rejected = result.verdict.kind() == VerdictKind::Reject;
    let mut out = Vec::new();
    match case["truth"].as_str().unwrap_or("") {
        "fits" if rejected => {
            let read = result.status == fitment_core::Status::Accepted;
            let dropped = result.sockets.iter().any(|s| s.stage == Stage::Shortlist);
            let by_bound = !failed.is_empty()
                && failed
                    .iter()
                    .all(|codes| codes.iter().any(|c| bounds.contains(&c.as_str())));
            if !read || dropped || !by_bound {
                out.push(format!(
                    "{id}: fits, rejected otherwise than by a bound (read {read}, dropped {dropped}): {failed:?}"
                ));
            }
        }
        "does_not_fit" | "fn_rule" if !rejected => out.push(format!("{id}: passed")),
        "fits" | "does_not_fit" | "fn_rule" => {}
        other => out.push(format!("{id}: an unknown truth {other:?}")),
    }
    if let Some(bound) = case["expect"]["bound"].as_str() {
        if !failed.iter().flatten().any(|c| c == bound) {
            out.push(format!("{id}: {bound} did not fire: {failed:?}"));
        }
    }
    if case["expect"]["placements_exhausted"] == true
        && !result.sockets.iter().any(|s| s.placements.1)
    {
        out.push(format!("{id}: alignment's placement bound was not reached"));
    }
    out
}
