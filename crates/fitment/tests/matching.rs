//! The corpus's match cases (`docs/research/m4-match.md`, sections 15 and
//! 16): every candidate the generator built, judged through the library
//! (KH1) against its source's sockets under both policies M4 offers, held
//! to the strict gate (KH2, KH3) and its defect at the construction's
//! placement (KH8); the false negatives against the committed baseline
//! (KH4, KH5); the report and its rates (KH6, KH7); the shortlist never
//! changing a verdict (KH9); every run the same twice (KH11); every file
//! judged from its stored document as from its bytes, and every pass
//! admitted by level 1 (`docs/research/m5-catalog.md`, CT5, QR3). The
//! binary's documents meet the same gate and baseline, each case's
//! outcome in them the library's (`docs/research/m6-api.md`, VT4, VT5),
//! all scored by `fitment-corpus` (SC1), with each case's review applied
//! on both paths (M6-38); core's run helpers give the CLI's exit codes
//! (CO2, VT37). Without the corpus these pass with a note, unless
//! `FITMENT_REQUIRE_CORPUS=1`.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use fitment_core::{
    Admitted, CandidateResult, CandidateSource, ComponentRead, FeaturesDocument, IndexTokens,
    MatchDocument, MatchOptions, MatchPolicy, Matcher, NoFitVerifier, Part, PartSignature,
    ShortlistIndex, Socket, SocketDocument, Source, Stage, VerdictKind, read_assembly,
};
use fitment_corpus::{
    CandidateRef, Case, Corpus, Outcome, Policy, Run, RunPath, Score, SocketKey, Truth, baseline,
    checker, gate, parity, report,
};
use serde_json::{Value, json};

/// The corpus, located once per test binary (SC7).
fn corpus() -> Option<&'static Corpus> {
    static CORPUS: OnceLock<Option<Corpus>> = OnceLock::new();
    CORPUS.get_or_init(Corpus::locate).as_ref()
}

/// A case's candidate (KC2): a file's bytes, or a component read in place
/// from a corpus assembly (the target of its own socket, VD5).
enum Input {
    File(Vec<u8>),
    Component(Box<Part>, String, u64),
}

impl Input {
    fn read(dir: &Path, case: Case<'_>) -> Self {
        let (source, part) = match case.candidate() {
            CandidateRef::File(file) => return Self::File(fs::read(dir.join(file)).unwrap()),
            CandidateRef::Component { source, part } => (source, part),
        };
        assert!(
            case.value()["candidate"]["component"]["occurrence"].is_null(),
            "a component by occurrence"
        );
        let bytes = fs::read(dir.join(format!("{source}.stp"))).unwrap();
        let read = read_assembly(&bytes);
        let listing = read.listing().unwrap();
        let selection = listing.select(Some(&part), None).unwrap();
        let definition = &listing.definitions()[selection.definition];
        let Some(ComponentRead::Accepted(read)) = &definition.read else {
            panic!("{source}: the component is not read");
        };
        let instance = definition.definition.instance;
        Self::Component(read.clone(), fitment_core::sha256_hex(&bytes), instance)
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

/// The socket `case` is judged against: its source's target, extracted,
/// with the case's review applied (M6-38).
struct Judged {
    /// As reviewed: what the case is judged against.
    reviewed: SocketDocument,
    /// As extracted, unreviewed: what a waived case's defect breaks (SC8).
    extracted: SocketDocument,
    /// Its sockets, as extracted.
    sockets: Vec<Socket>,
}

fn socket_document(dir: &Path, case: Case<'_>) -> Judged {
    let key = case.socket();
    let bytes = fs::read(dir.join(format!("{}.stp", key.source))).unwrap();
    let extracted = SocketDocument::extract(&bytes, Some(&key.part), None).unwrap();
    let sockets = extracted.extracted().unwrap().sockets.clone();
    let mut reviewed = extracted.clone();
    gate::apply_review(case.value(), &mut reviewed)
        .unwrap_or_else(|e| panic!("{}: harness: {e}", case.id()));
    Judged {
        reviewed,
        extracted,
        sockets,
    }
}

/// One case judged by the library under one policy, with what the harness
/// asks of it.
struct LibraryRun {
    case: Value,
    policy: Policy,
    result: CandidateResult,
    outcome: Outcome,
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

/// Every library run, computed once per test binary.
fn runs() -> Option<&'static Vec<LibraryRun>> {
    static RUNS: OnceLock<Option<Vec<LibraryRun>>> = OnceLock::new();
    RUNS.get_or_init(|| corpus().map(compute)).as_ref()
}

fn compute(corpus: &Corpus) -> Vec<LibraryRun> {
    let mut documents: BTreeMap<SocketKey, Judged> = BTreeMap::new();
    let mut out = Vec::new();
    for case in corpus
        .match_cases()
        .into_iter()
        .filter(|c| !c.socket_gate())
    {
        let judged = documents
            .entry(case.socket())
            .or_insert_with(|| socket_document(corpus.dir(), case));
        let input = Input::read(corpus.dir(), case);
        for policy in Policy::M4 {
            out.push(judge(case, policy, judged, &input));
        }
    }
    out
}

/// One case under one policy, and everything the library tests ask of
/// it.
fn judge(case: Case<'_>, policy: Policy, judged: &Judged, input: &Input) -> LibraryRun {
    let sockets = &judged.sockets;
    let matcher_of = |document, p: Policy| {
        Matcher::new(
            document,
            p.match_policy(),
            MatchOptions::default(),
            &NoFitVerifier,
        )
        .unwrap()
    };
    let matcher = |p: Policy| matcher_of(&judged.reviewed, p);
    let own = matcher(policy);
    let result = input.judge(&own);
    let outcome = Outcome::of_result(&result, own.policy());
    let mut problems = case.gate(policy, sockets, &outcome);
    let part = input.part();
    let at = gate::placement(case.value()).unwrap();
    let built: Vec<_> = part
        .iter()
        .flat_map(|p| sockets.iter().filter_map(|s| own.verify_at(p, &s.id, &at)))
        .collect();
    if case.truth() == Truth::Waived {
        // SC8: the defect, unreviewed, is exactly what the review removes.
        let unreviewed = matcher_of(&judged.extracted, policy);
        let at_construction: Vec<_> = part
            .iter()
            .flat_map(|p| {
                sockets
                    .iter()
                    .filter_map(|s| unreviewed.verify_at(p, &s.id, &at))
            })
            .collect();
        problems.extend(gate::waived(case.value(), sockets, &at_construction));
    } else if !case.value()["defect"].as_array().is_none_or(Vec::is_empty) {
        problems.extend(gate::defect(case.value(), sockets, &built));
    }
    let passes_at_construction = !built.is_empty()
        && built
            .iter()
            .all(|s| s.checks.iter().all(|(_, c)| c.passed()));
    let label = vec![case.id().to_owned()];
    let once = MatchDocument::new(&own, vec![(result.clone(), label.clone())]);
    let again = MatchDocument::new(&own, vec![(input.judge(&own), label)]);
    let admitted = part.as_ref().is_some_and(|p| {
        let mut index = ShortlistIndex::new();
        index.insert(0, &IndexTokens::of(&PartSignature::of(p)));
        matches!(
            own.admitted(&index),
            Ok(Admitted::Parts { candidates, .. }) if candidates == [0]
        )
    });
    LibraryRun {
        case: case.value().clone(),
        policy,
        without_shortlist: input
            .judge(&matcher(policy).without_shortlist())
            .verdict
            .kind(),
        same_twice: once.to_json() == again.to_json(),
        stored: input.judge_stored(&own),
        admitted,
        result,
        outcome,
        problems,
        passes_at_construction,
    }
}

/// The library's runs, scored (SC1).
fn library_score(corpus: &Corpus, runs: &[LibraryRun]) -> Score {
    let runs = runs
        .iter()
        .map(|r| Run {
            case: r.case["id"].as_str().unwrap().to_owned(),
            policy: r.policy,
            outcome: r.outcome.clone(),
            path: RunPath::Library,
            at_construction: Some(r.passes_at_construction),
        })
        .collect();
    Score::new(corpus, runs)
}

/// KH1's library path, KH2, KH3 and KH8 over every match case, reviewed
/// ones included (SC8).
#[test]
fn every_match_case_meets_its_construction() {
    let Some(runs) = runs() else {
        return;
    };
    assert!(runs.len() > 40, "{} runs", runs.len());
    let reviewed = runs
        .iter()
        .filter(|r| !Case::new(&r.case).review().is_empty())
        .count();
    assert!(reviewed > 0, "no run applies a review");
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

/// KH4, KH5, SC5: the library's false negatives are exactly the committed
/// baseline, which only this suite blesses: `FITMENT_BLESS=1` rewrites it,
/// never under `CI`, and never to add an entry for a case the baseline
/// already knew unless the matching rules' version rose.
#[test]
fn the_false_negatives_are_the_baseline() {
    let (Some(corpus), Some(runs)) = (corpus(), runs()) else {
        return;
    };
    let score = library_score(corpus, runs);
    let found = score.false_negatives().unwrap();
    if let Err(e) = baseline::bless_or_compare(&found, &score.cases()) {
        panic!("{e}");
    }
}

/// KH6, KH7, SC1: the rates, per policy and class, written to
/// `target/fitment-corpus/report.json` and printed; false positives are
/// zero under both policies M4 offers.
#[test]
fn the_report_states_the_rates() {
    let (Some(corpus), Some(runs)) = (corpus(), runs()) else {
        return;
    };
    let score = library_score(corpus, runs);
    eprintln!("{}", score.report_text());
    report::write("report.json", &score.report()).unwrap();
    let fps = score.false_positives();
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
        let (id, policy) = (&r.case["id"], r.policy.name());
        if r.without_shortlist != r.result.verdict.kind() {
            problems.push(format!(
                "{id} {policy}: {:?} without the shortlist",
                r.without_shortlist
            ));
        }
        let dropped = r.result.sockets.iter().any(|s| s.stage == Stage::Shortlist);
        if dropped && Case::new(&r.case).fits(r.policy) && r.case["truth"] != "fn_rule" {
            problems.push(format!(
                "{id} {policy}: a good case dropped by the shortlist"
            ));
        }
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
            assert_eq!(stored, &r.result, "{} {}", r.case["id"], r.policy.name());
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
                r.case["id"],
                r.policy.name()
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
        .map(|r| format!("{} {}", r.case["id"], r.policy.name()))
        .collect();
    assert!(differ.is_empty(), "{differ:?}");
}

/// One case's outcome read from the binary's document, by its file's
/// SHA-256 (SC9).
struct BinaryRun {
    case: String,
    policy: Policy,
    outcome: Result<Outcome, String>,
}

/// The binary path (KH1): each socket written by `fitment socket` and
/// reviewed by `fitment review` as its cases state (M6-38), each policy's
/// file candidates judged by one `fitment match`; its documents' problems
/// (KH2.6, KH12, VD14) and every case's outcome.
struct Binary {
    problems: Vec<String>,
    runs: Vec<BinaryRun>,
    /// Each `fitment match`'s exit code, with its key, policy and cases.
    exits: Vec<(SocketKey, Policy, Option<i32>, Vec<String>)>,
}

/// The binary path, run once per test binary.
fn binary() -> Option<&'static Binary> {
    static BINARY: OnceLock<Option<Binary>> = OnceLock::new();
    BINARY.get_or_init(|| corpus().map(run_binary)).as_ref()
}

fn run_binary(corpus: &Corpus) -> Binary {
    let scratch = scratch("binary");
    let mut out = Binary {
        problems: Vec::new(),
        runs: Vec::new(),
        exits: Vec::new(),
    };
    for (key, group) in corpus.socket_groups() {
        let files: Vec<(Case<'_>, String)> = group
            .iter()
            .copied()
            .filter(|c| !c.socket_gate())
            .filter_map(|c| match c.candidate() {
                CandidateRef::File(file) => Some((c, file)),
                CandidateRef::Component { .. } => None,
            })
            .collect();
        let Some((first, _)) = files.first() else {
            continue;
        };
        let socket = write_socket(corpus.dir(), &scratch, &key, *first);
        for policy in Policy::M4 {
            run_match(corpus.dir(), &socket, &key, policy, &files, &mut out);
        }
    }
    let _ = fs::remove_dir_all(&scratch);
    out
}

/// One `fitment match` over a socket's file candidates under `policy`:
/// the document's own problems, and each case's outcome in it.
fn run_match(
    dir: &Path,
    socket: &Path,
    key: &SocketKey,
    policy: Policy,
    cases: &[(Case<'_>, String)],
    out: &mut Binary,
) {
    let mut files: Vec<&str> = cases.iter().map(|(_, f)| f.as_str()).collect();
    files.sort_unstable();
    files.dedup();
    let output = fitment()
        .current_dir(dir)
        .arg("match")
        .arg(socket)
        .args(&files)
        .args(["--format", "json"])
        .args(policy.cli_args())
        .output()
        .unwrap();
    let doc: Value = serde_json::from_slice(&output.stdout).unwrap();
    let at = format!(
        "{} {} {} {}",
        key.source,
        key.part,
        key.review,
        policy.name()
    );
    // KH2.6: the document states the run's policy.
    let policy_value = fitment_corpus::outcome::policy_value(&policy.match_policy());
    if doc["policy"] != policy_value {
        out.problems
            .push(format!("{at}: the document's policy is {}", doc["policy"]));
    }
    out.problems.extend(
        checker::problems(&doc)
            .into_iter()
            .map(|p| format!("{at}: {p}")),
    );
    out.exits.push((
        key.clone(),
        policy,
        output.status.code(),
        cases.iter().map(|(c, _)| c.id().to_owned()).collect(),
    ));
    let code = parity::exit_code("succeeded", Some(parity::outcome_of(&doc)));
    if output.status.code() != code {
        out.problems.push(format!(
            "{at}: exit {:?}, its outcome says {code:?}",
            output.status.code()
        ));
    }
    for (case, file) in cases {
        let sha256 = fitment_core::sha256_hex(&fs::read(dir.join(file)).unwrap());
        out.runs.push(BinaryRun {
            case: case.id().to_owned(),
            policy,
            outcome: Outcome::find(&doc, &sha256),
        });
    }
}

/// KH1's binary path and KH12 on the corpus: each document the checker
/// finds sound, stating its policy, its exit code its outcome's, and each
/// case's verdict the library's.
#[test]
fn the_binary_agrees_with_the_library() {
    let (Some(binary), Some(runs)) = (binary(), runs()) else {
        return;
    };
    let mut problems = binary.problems.clone();
    for run in &binary.runs {
        let library = library_run(runs, &run.case, run.policy);
        let verdict = run.outcome.as_ref().map(|o| o.verdict.as_str());
        if verdict != Ok(library.outcome.verdict.as_str()) {
            problems.push(format!(
                "{} {}: {verdict:?}, {} in the library",
                run.case,
                run.policy.name(),
                library.outcome.verdict
            ));
        }
    }
    assert!(binary.runs.len() > 80, "{} binary runs", binary.runs.len());
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

fn library_run<'r>(runs: &'r [LibraryRun], case: &str, policy: Policy) -> &'r LibraryRun {
    runs.iter()
        .find(|r| r.case["id"] == case && r.policy == policy)
        .unwrap_or_else(|| panic!("no library run of {case} {}", policy.name()))
}

/// VT4, SC2, SC9: on every corpus run of the binary path, the outcome read
/// from the binary's document, found by the candidate's SHA-256, is the
/// outcome of the library's result.
#[test]
fn a_documents_outcome_is_the_librarys() {
    let (Some(binary), Some(runs)) = (binary(), runs()) else {
        return;
    };
    let mut problems = Vec::new();
    for run in &binary.runs {
        let library = &library_run(runs, &run.case, run.policy).outcome;
        match &run.outcome {
            Ok(o) if o == library => {}
            Ok(o) => problems.push(format!(
                "{} {}: the document's {o:?}, the library's {library:?}",
                run.case,
                run.policy.name()
            )),
            Err(e) => problems.push(format!("{} {}: {e}", run.case, run.policy.name())),
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// VT37, CO2 (`docs/research/m6-api.md`), JB4: core's run helpers give the
/// CLI's exit codes on every corpus run of the binary path: the exit code
/// of each `fitment match` is 1 if a library result of its cases is a
/// fitment bug (`is_fitment_bug`), else `Outcome::exit_code` of the
/// library's document over the same results; no corpus result is a bug,
/// and every corpus component judged through `judge_component` (CO5) is
/// `judge_part` on the part the listing reads.
#[test]
fn exit_codes_are_the_clis() {
    let (Some(corpus), Some(binary), Some(runs)) = (corpus(), binary(), runs()) else {
        return;
    };
    let mut problems = Vec::new();
    for (key, policy, code, cases) in &binary.exits {
        let case = corpus.case(&cases[0]).unwrap();
        let judged = socket_document(corpus.dir(), case);
        let matcher = Matcher::new(
            &judged.reviewed,
            policy.match_policy(),
            MatchOptions::default(),
            &NoFitVerifier,
        )
        .unwrap();
        let results: Vec<_> = cases
            .iter()
            .map(|c| {
                (
                    library_run(runs, c, *policy).result.clone(),
                    vec![c.clone()],
                )
            })
            .collect();
        let bug = results.iter().any(|(r, _)| r.is_fitment_bug());
        let core = if bug {
            1
        } else {
            MatchDocument::new(&matcher, results).outcome().exit_code()
        };
        if *code != Some(i32::from(core)) {
            problems.push(format!(
                "{key:?} {}: the CLI's {code:?}, core's {core}",
                policy.name()
            ));
        }
    }
    for run in runs {
        if run.result.is_fitment_bug() || run.result.shortlist_unsound() {
            problems.push(format!(
                "{} {}: a fitment bug",
                run.case["id"],
                run.policy.name()
            ));
        }
        let case = Case::new(&run.case);
        if let CandidateRef::Component { source, part } = case.candidate() {
            let bytes = fs::read(corpus.dir().join(format!("{source}.stp"))).unwrap();
            let component =
                fitment_core::SelectedComponent::read(&bytes, Some(&part), None).unwrap();
            let judged = socket_document(corpus.dir(), case);
            let matcher = Matcher::new(
                &judged.reviewed,
                run.policy.match_policy(),
                MatchOptions::default(),
                &NoFitVerifier,
            )
            .unwrap();
            if matcher.judge_component(&component, component.source()) != run.result {
                problems.push(format!(
                    "{} {}: judge_component is not judge_part",
                    run.case["id"],
                    run.policy.name()
                ));
            }
        }
    }
    assert!(binary.exits.len() > 40, "{} runs", binary.exits.len());
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// The binary's runs, scored (SC1).
fn binary_score(corpus: &Corpus, binary: &Binary) -> Score {
    let runs = binary
        .runs
        .iter()
        .filter_map(|r| {
            Some(Run {
                case: r.case.clone(),
                policy: r.policy,
                outcome: r.outcome.clone().ok()?,
                path: RunPath::Binary,
                at_construction: None,
            })
        })
        .collect();
    Score::new(corpus, runs)
}

/// VT4, KH2, KH3 on the binary path: every case's outcome in the binary's
/// documents meets its construction under the strict gate, as the
/// library's does.
#[test]
fn the_binary_meets_the_construction() {
    let (Some(corpus), Some(binary)) = (corpus(), binary()) else {
        return;
    };
    let mut sockets: BTreeMap<SocketKey, Vec<Socket>> = BTreeMap::new();
    let mut problems = Vec::new();
    for run in &binary.runs {
        let case = corpus.case(&run.case).unwrap();
        let sockets = sockets
            .entry(case.socket())
            .or_insert_with(|| socket_document(corpus.dir(), case).sockets);
        match &run.outcome {
            Ok(outcome) => problems.extend(case.gate(run.policy, sockets, outcome)),
            Err(e) => problems.push(format!("{} {}: {e}", run.case, run.policy.name())),
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// VT5, SC5: the binary's false negatives are the baseline's for the cases
/// it ran, with the baseline's causes; it never writes the baseline, and
/// it states no false positive.
#[test]
fn the_binary_false_negatives_are_the_baseline() {
    let (Some(corpus), Some(binary)) = (corpus(), binary()) else {
        return;
    };
    let score = binary_score(corpus, binary);
    let found = score.false_negatives().unwrap_or_else(|e| panic!("{e}"));
    if let Err(e) = baseline::compare(&found, &score.ran()) {
        panic!("{e}");
    }
    let fps = score.false_positives();
    assert!(fps.is_empty(), "false positives: {fps:?}");
}

/// VD9 and MF2 on the corpus: a socket document stale, with a broken
/// digest, unreviewed under `--require-reviewed`, or refused, is refused
/// (exit 3) before any candidate is read.
#[test]
fn the_socket_gate_refuses_before_any_candidate() {
    let Some(corpus) = corpus() else {
        return;
    };
    let dir = corpus.dir();
    let scratch = scratch("gate");
    let mut seen = 0;
    let mut problems = Vec::new();
    for case in corpus.match_cases().into_iter().filter(Case::socket_gate) {
        seen += 1;
        let key = case.socket();
        let id = case.id();
        let path = write_socket(dir, &scratch, &key, case);
        let mut doc: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        let mut args = vec!["match".to_owned()];
        // Each refused for its own reason, not another's.
        let reason = match key.edit.as_deref() {
            Some("stale") => "extract it again",
            Some("digest") => "digest does not match",
            Some("unreviewed") => "unreviewed",
            _ => "holds no socket",
        };
        match key.edit.as_deref() {
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

/// `fitment socket` on a corpus source, written to `scratch` (a refused
/// document, exit 3, kept from stdout), then reviewed by `fitment review`
/// once per decision `case`'s review states (M6-38).
fn write_socket(dir: &Path, scratch: &Path, key: &SocketKey, case: Case<'_>) -> PathBuf {
    let (source, part) = (&key.source, &key.part);
    let name = format!(
        "{source}--{part}--{}.json",
        fitment_core::sha256_hex(key.review.as_bytes())
    );
    let path = scratch.join(name);
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
    if case.review().is_empty() {
        return path;
    }
    let document = SocketDocument::from_json(&String::from_utf8_lossy(&output.stdout)).unwrap();
    let sockets = &document.extracted().unwrap().sockets;
    for d in gate::decisions(case.value(), sockets).unwrap() {
        let flag = match d.status.as_str() {
            "removed" => "--remove",
            "kept" => "--keep",
            _ => "--unreview",
        };
        let mut review = fitment();
        review.arg("review").arg(&path).args([flag, &d.requirement]);
        review.args(["--socket", &d.socket]);
        if let Some(note) = &d.note {
            review.args(["--note", note]);
        }
        let output = review.output().unwrap();
        assert!(
            output.status.success(),
            "fitment review {source} {part}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    path
}

/// M4-13 and AL11 on hostile inputs (10,000 identical holes, a 1,600-bore
/// socket against a 1,681-hole grid, a bracket fused from 1,000 tiles, 4,000
/// stubs near one bore): each judged within its bound in time (in an
/// optimised build), a candidate that does not fit rejected, and one that
/// fits rejected, if at all, only by a bound: never a pass from a bound.
#[test]
fn hostile_inputs_stay_within_their_bounds() {
    let Some(corpus) = corpus() else {
        return;
    };
    let dir = corpus.dir();
    let mut seen = 0;
    let mut problems = Vec::new();
    for case in corpus.cases() {
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
