//! The corpus with the fit (`docs/research/m7-fit.md`, section 11, PR 4):
//! VT58, VT60 and VT63 through the real helper.
//!
//! - The library path: every match case, under both policies M4 offers,
//!   judged by fitment-core with one `OcctSession` per source assembly
//!   (FL1), held to the interface's strict gate (KH2, KH3) and FK1's truths
//!   by `fitment-corpus` (no false positive; every `collides` case `Reject`
//!   by `interferes` or `InterfaceMatch` with FK1's codes; a `fits` case
//!   never `Reject` by `interferes`), FK2's cases each ending as their
//!   manifest states (VT60), and every run's fit equal to the committed fit
//!   baseline (FK6). The report counts every run by verdict and fit and
//!   lists every `not_verified` by code (`target/fitment-corpus/
//!   fit-report.json`); every v2 document is written beside it with an
//!   index and its digest, for the oracle (FO1 to FO3) and for CI's
//!   comparison of the two OSes' digests (VT63, FM5).
//! - VT63 here: the run again in reverse order, every case in a fresh
//!   helper (`max_checks` 1), gives every document byte for byte.
//! - The binary path, on a named subset (the committed candidates, the
//!   `collides` cases and FK2's `f_` cases, on sockets read without
//!   review) with one helper (`--fit-jobs 1`): each document's entries are
//!   the library's outcomes, its exit code its outcome's.
//!
//! These need uv and the real helper (`fit_common`, which says when they
//! run). On Windows the helper is refused until VT77 is green (M7-50):
//! these say so and stop.

#![cfg(feature = "fit")]

mod fit_common;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use fit_common::{inputs, proved, refused_here};

use assert_cmd::Command;
use fitment_core::{
    CandidateResult, MatchDocument, MatchOptions, Matcher, SelectedComponent, Socket,
    SocketDocument,
};
use fitment_corpus::{CandidateRef, Case, Corpus, Outcome, Policy, SocketKey, fit, gate, report};
use fitment_fit::{HelperFiles, HelperOptions, OcctHelper, OcctSession};
use serde_json::{Value, json};

/// A socket as the cases of its group are judged against it.
struct Group<'c> {
    key: SocketKey,
    cases: Vec<Case<'c>>,
    /// The source assembly's bytes.
    assembly: Vec<u8>,
    /// The socket document, reviewed as the cases' review says.
    reviewed: SocketDocument,
    /// Its sockets as extracted (the gate's locators).
    sockets: Vec<Socket>,
    /// The document's file name, without the policy.
    name: String,
}

/// Every socket group of the corpus but the socket gate's, in key order.
fn groups(corpus: &Corpus) -> Vec<Group<'_>> {
    let mut reviews: BTreeMap<(String, String), usize> = BTreeMap::new();
    corpus
        .socket_groups()
        .into_iter()
        .filter_map(|(key, cases)| {
            let cases: Vec<Case<'_>> = cases.into_iter().filter(|c| !c.socket_gate()).collect();
            let first = *cases.first()?;
            let assembly = fs::read(corpus.dir().join(format!("{}.stp", key.source))).unwrap();
            let extracted = SocketDocument::extract(&assembly, Some(&key.part), None).unwrap();
            let sockets = extracted
                .extracted()
                .unwrap_or_else(|| panic!("{}: harness: its socket is refused", first.id()))
                .sockets
                .clone();
            let mut reviewed = extracted.clone();
            gate::apply_review(first.value(), &mut reviewed)
                .unwrap_or_else(|e| panic!("{}: harness: {e}", first.id()));
            let n = reviews
                .entry((key.source.clone(), key.part.clone()))
                .or_default();
            let name = if *n == 0 {
                format!("{}--{}", key.source, key.part)
            } else {
                format!("{}--{}--review{n}", key.source, key.part)
            };
            *n += 1;
            Some(Group {
                key,
                cases,
                assembly,
                reviewed,
                sockets,
                name,
            })
        })
        .collect()
}

/// One case judged under one policy through the library.
#[derive(Clone)]
struct FitRun {
    case: String,
    policy: Policy,
    outcome: Outcome,
    problems: Vec<String>,
}

/// One group's documents and runs, under each policy.
struct Judged {
    runs: Vec<FitRun>,
    /// (file name, document, the group's key as JSON) per policy.
    documents: Vec<(String, MatchDocument, Value)>,
    controls: Vec<(String, Value)>,
}

fn label(case: Case<'_>) -> String {
    match case.candidate() {
        CandidateRef::File(file) => file,
        CandidateRef::Component { source, part } => format!("{source}#{part}"),
    }
}

/// `case`'s candidate judged by `matcher`.
fn judge(corpus: &Corpus, case: Case<'_>, matcher: &Matcher<'_>) -> CandidateResult {
    match case.candidate() {
        CandidateRef::File(file) => matcher.judge_step(&fs::read(corpus.dir().join(file)).unwrap()),
        CandidateRef::Component { source, part } => {
            let bytes = fs::read(corpus.dir().join(format!("{source}.stp"))).unwrap();
            let component = SelectedComponent::read(&bytes, Some(&part), None).unwrap();
            if case.socket().source == source {
                matcher.judge_component(&component, component.source())
            } else {
                matcher.judge_component_in(&component, component.source(), &bytes)
            }
        }
    }
}

/// `group` judged with `session` under both policies, its cases in the
/// manifest's order or reversed; each document's entries in the
/// manifest's order whatever the order judged.
fn judge_group(
    corpus: &Corpus,
    group: &Group<'_>,
    session: &OcctSession<'_>,
    reverse: bool,
) -> Judged {
    let mut judged = Judged {
        runs: Vec::new(),
        documents: Vec::new(),
        controls: Vec::new(),
    };
    for policy in Policy::M4 {
        let matcher = Matcher::new(
            &group.reviewed,
            policy.match_policy(),
            MatchOptions::default(),
            session,
        )
        .unwrap();
        let mut order: Vec<usize> = (0..group.cases.len()).collect();
        if reverse {
            order.reverse();
        }
        let mut results: Vec<Option<CandidateResult>> = vec![None; group.cases.len()];
        for i in order {
            results[i] = Some(judge(corpus, group.cases[i], &matcher));
        }
        let mut entries = Vec::new();
        for (case, result) in group.cases.iter().zip(results) {
            let result = result.unwrap();
            let outcome = Outcome::of_result(&result, matcher.policy());
            let problems = fit::gate(case.value(), policy.name(), &group.sockets, &outcome);
            judged.runs.push(FitRun {
                case: case.id().to_owned(),
                policy,
                outcome,
                problems,
            });
            entries.push((result, vec![label(*case)]));
        }
        let key = json!({"source": group.key.source, "part": group.key.part,
                         "review": group.cases[0].review(), "policy": policy.name()});
        let name = format!("{}--{}.json", group.name, policy.name());
        let control = matcher.fit_control().map(|c| {
            let status = serde_json::to_value(c.outcome().candidate()).unwrap_or_default();
            json!({"status": status["status"], "code": c.outcome().code().map(fitment_core::fit::FitCode::as_str)})
        });
        judged.controls.push((name.clone(), json!(control)));
        judged
            .documents
            .push((name, MatchDocument::new(&matcher, entries), key));
    }
    judged
}

/// Every group judged, `workers` at a time, one session per source
/// assembly (FL1), the helpers built with `options`.
fn run_all(corpus: &Corpus, dir: &Path, options: &HelperOptions, reverse: bool) -> Vec<Judged> {
    let helper = OcctHelper::open(dir, HelperFiles::embedded(), options.clone()).unwrap();
    let all = groups(corpus);
    let mut by_source: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (i, g) in all.iter().enumerate() {
        by_source.entry(g.key.source.as_str()).or_default().push(i);
    }
    let mut work: Vec<Vec<usize>> = by_source.into_values().collect();
    if reverse {
        work.reverse();
        for w in &mut work {
            w.reverse();
        }
    }
    let queue = Mutex::new(work);
    let done: Mutex<Vec<(usize, Judged)>> = Mutex::new(Vec::new());
    std::thread::scope(|s| {
        for _ in 0..4 {
            s.spawn(|| {
                loop {
                    let Some(indices) = queue.lock().unwrap().pop() else {
                        break;
                    };
                    let first = &all[indices[0]];
                    let session =
                        OcctSession::open(&helper, first.assembly.clone(), &first.reviewed)
                            .unwrap();
                    for i in indices {
                        let judged = judge_group(corpus, &all[i], &session, reverse);
                        done.lock().unwrap().push((i, judged));
                    }
                    let reports = session.take_reports();
                    assert!(reports.is_empty(), "{}: {reports:#?}", first.key.source);
                }
            });
        }
    });
    let mut done = done.into_inner().unwrap();
    done.sort_by_key(|(i, _)| *i);
    done.into_iter().map(|(_, j)| j).collect()
}

/// The library's runs, once per test binary, with how long they took.
fn library() -> Option<&'static (Vec<Judged>, f64)> {
    static RUNS: OnceLock<Option<(Vec<Judged>, f64)>> = OnceLock::new();
    RUNS.get_or_init(|| {
        let (corpus, dir) = inputs()?;
        let started = Instant::now();
        let options = proved(HelperOptions::default().with_jobs(1));
        let judged = run_all(corpus, dir, &options, false);
        let seconds = started.elapsed().as_secs_f64();
        eprintln!(
            "the library path: {} documents, {} runs, {seconds:.1} s",
            judged.iter().map(|j| j.documents.len()).sum::<usize>(),
            judged.iter().map(|j| j.runs.len()).sum::<usize>()
        );
        write_documents(&judged);
        Some((judged, seconds))
    })
    .as_ref()
}

/// Every v2 document under `target/fitment-corpus/fit/`, with
/// `index.json` (each document's socket, review and policy) and
/// `digests.json` (each document's digest, for CI's comparison of OSes).
fn write_documents(judged: &[Judged]) {
    let dir = report::dir().join("fit");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let mut index = Vec::new();
    let mut digests = BTreeMap::new();
    for (name, document, key) in judged.iter().flat_map(|j| &j.documents) {
        fs::write(dir.join(name), document.to_json()).unwrap();
        index.push(json!({"file": name, "socket": key}));
        digests.insert(name.clone(), document.digest().to_owned());
    }
    fs::write(
        dir.join("index.json"),
        serde_json::to_string_pretty(&index).unwrap(),
    )
    .unwrap();
    report::write("fit-digests.json", &json!(digests)).unwrap();
}

macro_rules! need {
    () => {
        match library() {
            Some(library) if !refused_here() => library,
            Some(_) => {
                eprintln!("the helper is refused on Windows until VT77 is green (M7-50)");
                return;
            }
            None => return,
        }
    };
}

fn runs(judged: &[Judged]) -> impl Iterator<Item = &FitRun> {
    judged.iter().flat_map(|j| &j.runs)
}

/// VT58, VT60: every match case under both policies with the fit meets
/// its construction: the interface's strict gate, FK1's truths, and FK2's
/// cases' own fits.
#[test]
fn every_match_case_meets_its_construction_with_the_fit() {
    let (judged, _) = need!();
    let (corpus, _) = inputs().unwrap();
    let all: Vec<&FitRun> = runs(judged).collect();
    let cases = corpus
        .match_cases()
        .into_iter()
        .filter(|c| !c.socket_gate())
        .count();
    assert!(
        cases > 0 && all.len() == 2 * cases,
        "{} runs of {cases} cases",
        all.len()
    );
    let problems: Vec<&String> = all.iter().flat_map(|r| &r.problems).collect();
    assert!(
        problems.is_empty(),
        "{} problems:\n{}",
        problems.len(),
        problems
            .iter()
            .map(|p| p.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// FK6: every run's fit status and code are the committed fit baseline,
/// against `FIT_RULES_VERSION` and the helper; `FITMENT_BLESS=1` rewrites
/// it, never under CI.
#[test]
fn the_fits_are_the_baseline() {
    let (judged, _) = need!();
    let entries: Vec<Value> = runs(judged)
        .map(|r| fit::entry(&r.case, r.policy.name(), &r.outcome))
        .collect();
    let against = fit::Against {
        helper_sha256: fitment_fit::SCRIPT_SHA256.to_owned(),
        binding: fitment_fit::BINDING.to_owned(),
    };
    if let Err(e) = fit::bless_or_compare(&entries, &against) {
        panic!("{e}");
    }
}

/// Section 11, PR 4's "done when": the report states every run by verdict
/// and fit, and every `not_verified` by code, per policy; no `Match` on a
/// case whose truth does not fit, and every `collides` case `Reject` by
/// `interferes` or `InterfaceMatch` with FK1's codes.
#[test]
fn the_report_states_every_not_verified_by_code() {
    let (judged, seconds) = need!();
    let (corpus, _) = inputs().unwrap();
    let rows: Vec<(String, String, String, Outcome)> = runs(judged)
        .map(|r| {
            let truth = corpus.case(&r.case).unwrap().value()["truth"]
                .as_str()
                .unwrap()
                .to_owned();
            (
                r.case.clone(),
                r.policy.name().to_owned(),
                truth,
                r.outcome.clone(),
            )
        })
        .collect();
    let mut body = fit::report(&rows);
    body["seconds"] = json!(seconds);
    body["controls"] = json!(
        judged
            .iter()
            .flat_map(|j| &j.controls)
            .map(|(n, c)| json!({"document": n, "control": c}))
            .collect::<Vec<_>>()
    );
    let path = report::write("fit-report.json", &body).unwrap();
    eprintln!(
        "{}\n{}",
        path.display(),
        serde_json::to_string_pretty(&body["default"]).unwrap()
    );
    for (case, policy, truth, outcome) in &rows {
        assert!(
            outcome.verdict != "match" || fit::may_match(truth, policy),
            "a false positive: {case} {policy}"
        );
        if truth == "collides" {
            let ok = (outcome.verdict == "reject" && outcome.fit == "interferes")
                || outcome
                    .fit_code
                    .as_deref()
                    .is_some_and(|c| fit::COLLIDES_MAY_END.contains(&c));
            assert!(ok, "{case} {policy}: {outcome:?}");
        }
    }
}

/// VT63: the corpus again, groups and cases in reverse order, each check
/// in a fresh helper, gives every document byte for byte.
#[test]
fn the_fit_is_the_same_in_reverse_with_a_fresh_helper_per_check() {
    let (judged, _) = need!();
    let (corpus, dir) = inputs().unwrap();
    let started = Instant::now();
    let options = proved(HelperOptions::default().with_jobs(1).with_max_checks(1));
    let again = run_all(corpus, dir, &options, true);
    eprintln!(
        "in reverse, a fresh helper per check: {:.1} s",
        started.elapsed().as_secs_f64()
    );
    let texts = |all: &[Judged]| -> BTreeMap<String, String> {
        all.iter()
            .flat_map(|j| &j.documents)
            .map(|(n, d, _)| (n.clone(), d.to_json()))
            .collect()
    };
    let (once, twice) = (texts(judged), texts(&again));
    assert_eq!(once.len(), twice.len());
    for (name, text) in &once {
        assert!(twice.get(name) == Some(text), "{name} differs");
    }
}

/// The binary path's subset (M7-26): the committed candidates, the
/// `collides` cases and FK2's `f_` cases, as files, on sockets read
/// without review; every file case with `FITMENT_FIT_BINARY_ALL=1`, as the
/// as-built note measures it.
fn in_subset(case: Case<'_>) -> bool {
    let v = case.value();
    let file = matches!(case.candidate(), CandidateRef::File(_));
    if std::env::var_os("FITMENT_FIT_BINARY_ALL").is_some_and(|x| x == "1") {
        return file;
    }
    file && case.review().is_empty()
        && (v["set"] == "unit" || v["truth"] == "collides" || case.id().starts_with("f_"))
}

/// VT58's binary path: `fitment match --fit --fit-jobs 1` on the subset,
/// each socket and policy one run: every entry is the library's outcome,
/// and the exit code is the document's.
#[test]
fn the_binary_agrees_with_the_library_on_its_subset() {
    let (judged, _) = need!();
    if cfg!(windows) {
        eprintln!(
            "the release binary refuses the helper on Windows until VT77 is green (M7-50): \
             VT77 runs the library path alone"
        );
        return;
    }
    let (corpus, dir) = inputs().unwrap();
    let library: BTreeMap<(String, Policy), &Outcome> = runs(judged)
        .map(|r| ((r.case.clone(), r.policy), &r.outcome))
        .collect();
    let scratch = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("fit-corpus-binary");
    let _ = fs::remove_dir_all(&scratch);
    fs::create_dir_all(&scratch).unwrap();
    let started = Instant::now();
    let mut runs = 0;
    for group in groups(corpus) {
        let cases: Vec<Case<'_>> = group
            .cases
            .iter()
            .copied()
            .filter(|c| in_subset(*c))
            .collect();
        if cases.is_empty() {
            continue;
        }
        let assembly = corpus.dir().join(format!("{}.stp", group.key.source));
        let socket = scratch.join(format!("{}.json", group.name));
        fs::write(&socket, group.reviewed.to_json()).unwrap();
        for policy in Policy::M4 {
            runs += 1;
            binary_run(corpus, dir, &assembly, &socket, &cases, policy, &library);
        }
    }
    eprintln!(
        "the binary path: {runs} runs, {:.1} s",
        started.elapsed().as_secs_f64()
    );
    assert!(runs > 10, "{runs} runs");
}

fn binary_run(
    corpus: &Corpus,
    fit: &Path,
    assembly: &Path,
    socket: &Path,
    cases: &[Case<'_>],
    policy: Policy,
    library: &BTreeMap<(String, Policy), &Outcome>,
) {
    let mut args: Vec<String> = vec!["match".into(), socket.display().to_string()];
    for case in cases {
        if let CandidateRef::File(file) = case.candidate() {
            args.push(file);
        }
    }
    args.extend(policy.cli_args().iter().map(|s| (*s).to_owned()));
    args.extend([
        "--fit".to_owned(),
        assembly.display().to_string(),
        "--fit-dir".to_owned(),
        fit.display().to_string(),
        "--fit-jobs".to_owned(),
        "1".to_owned(),
        "--format".to_owned(),
        "json".to_owned(),
    ]);
    let out = Command::cargo_bin("fitment")
        .unwrap()
        .current_dir(corpus.dir())
        .args(&args)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    let doc: Value = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("{args:?}: no document ({e}): {stderr}"));
    assert_eq!(doc["format_version"], 2, "{stderr}");
    let code = out.status.code().unwrap_or(-1);
    let summary = &doc["summary"];
    let expected = if summary["match"].as_u64() > Some(0) {
        0
    } else if summary["interface_match"].as_u64() > Some(0) {
        4
    } else {
        5
    };
    assert_eq!(code, expected, "{args:?}: {stderr}");
    for case in cases {
        let CandidateRef::File(file) = case.candidate() else {
            continue;
        };
        let sha = fitment_core::sha256_hex(&fs::read(corpus.dir().join(&file)).unwrap());
        let found = Outcome::find(&doc, &sha).unwrap();
        let theirs = library[&(case.id().to_owned(), policy)];
        assert_eq!(&found, theirs, "{} {}", case.id(), policy.name());
    }
}
