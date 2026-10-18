//! The exit's file half (docs/research/m6-api.md, VT1: XH1 parts 1 to 4,
//! XH2 to XH7, XH9 to XH11): every match case of the corpus through the
//! built `fitment-server`, a child over loopback HTTP (M6-36). For each
//! socket key, in manifest order: the source uploaded and registered (its
//! `fitment.parts` the CLI's), the socket extracted (the CLI's
//! `fitment socket`), reviewed as the case states (the CLI's `fitment
//! review`, once per decision), and per policy one match job over its file
//! candidates, the CLI's `fitment match` byte for byte; each component
//! candidate a match job of its own, the library's document byte for byte
//! (DC3). Every case's entry, found by content (SC9), is held to the
//! strict gate (XH5); the false negatives are the baseline's; the report
//! goes to `target/fitment-corpus/api-report.json`, which CI's
//! `server-corpus` job diffs against the library's `report.json` (XH6,
//! VT3). The socket gate's cases, KC13's reviews and KC11's originals run
//! too, and the file half twice more, on one CPU in manifest order and on
//! the default budget in reverse, giving the same bytes (XH9).
//!
//! Without the corpus, or in a debug build, each passes with a note,
//! unless `FITMENT_REQUIRE_CORPUS=1` (XH10).

mod support;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use fitment_core::{
    MatchDocument, MatchOptions, Matcher, NoFitVerifier, SelectedComponent, Socket, SocketDocument,
};
use fitment_corpus::{
    CandidateRef, Case, Corpus, Outcome, Policy, Run, RunPath, Score, SocketKey, baseline, checker,
    gate, parity, report,
};
use serde_json::{Value, json};
use support::{Call, Dirs, TestServer, cli, waiting};

/// The server's configuration for the exit: its rate and queue limits
/// raised for one client submitting every case (`extra` appended).
fn config(extra: &str) -> String {
    format!(
        "[auth]\nrate_per_second = 100000\nrate_burst = 100000\n\
         [jobs]\nper_owner_queued = 1000\nqueue_query = 1000\n{extra}"
    )
}

/// The corpus, in a release build only (XH10).
fn corpus() -> Option<Corpus> {
    Corpus::locate_release_only()
}

/// What one run of the file half found.
#[derive(Default)]
struct FileHalf {
    runs: Vec<Run>,
    /// Every document a job made, by what it judged: the comparison XH9
    /// makes between two servers.
    documents: BTreeMap<String, Vec<u8>>,
    /// Parity's and the checks' failures.
    problems: Vec<String>,
    /// The sockets as extracted, by key: what the gate reads (KH2).
    sockets: BTreeMap<SocketKey, Vec<Socket>>,
    /// Counts of what ran, for the report (XH1).
    counted: BTreeMap<&'static str, usize>,
}

impl FileHalf {
    fn count(&mut self, what: &'static str) {
        *self.counted.entry(what).or_default() += 1;
    }
}

/// The CLI's run in the corpus directory.
fn cli_in(corpus: &Corpus, args: &[&str]) -> (Option<i32>, Vec<u8>, String) {
    cli(corpus.dir(), args)
}

/// A job's document, or `None` with a problem.
async fn document_of(
    server: &TestServer,
    job: &Value,
    what: &str,
    half: &mut FileHalf,
) -> Option<Vec<u8>> {
    if job["document"].is_null() {
        half.problems.push(format!("{what}: no document: {job}"));
        return None;
    }
    let reply = server.document(job).await;
    Some(reply.body)
}

/// XH3.1: the source uploaded (the SHA-256 the API states is the test's)
/// and registered, its `fitment.parts` the CLI's; once per source.
async fn register(
    server: &TestServer,
    corpus: &Corpus,
    source: &str,
    registered: &mut BTreeSet<String>,
    half: &mut FileHalf,
) -> String {
    let file = format!("{source}.stp");
    let bytes = std::fs::read(corpus.dir().join(&file)).unwrap();
    let sha = fitment_core::sha256_hex(&bytes);
    if !registered.insert(source.to_owned()) {
        return sha;
    }
    let reply = server.send(support::put_upload(&bytes)).await;
    if reply.json()["sha256"] != sha.as_str() {
        half.problems
            .push(format!("{file}: the API's SHA-256 is not the test's"));
    }
    let body = json!({ "upload": format!("sha256:{sha}"), "file_name": file });
    let job = server.send(waiting("/v1/assemblies", &body)).await.json();
    let job = server
        .finished_job(job["id"].as_str().unwrap(), Duration::from_secs(300))
        .await;
    let (_, stdout, _) = cli_in(corpus, &["parts", &file, "--format", "json"]);
    if let Some(got) = document_of(server, &job, &file, half).await {
        half.problems
            .extend(parity::bytes(&format!("{file} parts"), &stdout, &got));
    }
    half.count("assemblies registered");
    sha
}

/// XH3.2, XH3.3: the socket extracted unreviewed (`carry: false`, so a
/// line's earlier review carries nothing), the CLI's `fitment socket`;
/// then the case's review, the CLI's `fitment review` once per decision.
/// The revision judged, its bytes, and the sockets as extracted.
async fn socket_of(
    server: &TestServer,
    corpus: &Corpus,
    scratch: &Path,
    key: &SocketKey,
    case: Case<'_>,
    half: &mut FileHalf,
) -> Option<(String, Vec<u8>, Vec<Socket>)> {
    let file = format!("{}.stp", key.source);
    let sha = fitment_core::sha256_hex(&std::fs::read(corpus.dir().join(&file)).unwrap());
    let occurrence: Option<String> = key
        .occurrence
        .as_deref()
        .and_then(|o| serde_json::from_str(o).ok());
    let body = json!({ "assembly": format!("sha256:{sha}"), "part": key.part,
                       "occurrence": occurrence, "carry": false });
    let job = server.send(waiting("/v1/sockets", &body)).await.json();
    let job = server
        .finished_job(job["id"].as_str().unwrap(), Duration::from_secs(300))
        .await;
    let what = format!("{} {}", key.source, key.part);
    let mut args = vec![
        "socket",
        file.as_str(),
        "--part",
        key.part.as_str(),
        "--format",
        "json",
    ];
    if let Some(o) = &occurrence {
        args.extend(["--occurrence", o.as_str()]);
    }
    let (code, stdout, _) = cli_in(corpus, &args);
    let got = document_of(server, &job, &what, half).await?;
    half.problems
        .extend(parity::bytes(&format!("{what} socket"), &stdout, &got));
    if code != Some(0) {
        half.problems
            .push(format!("{what}: the CLI's socket exits {code:?}"));
        return None;
    }
    let extracted = SocketDocument::from_json(&String::from_utf8_lossy(&got)).ok()?;
    let sockets = extracted.extracted()?.sockets.clone();
    let mut revision = job["result"]["href"]
        .as_str()?
        .trim_start_matches("/v1/sockets/")
        .to_owned();
    let path = scratch.join(format!(
        "{}.json",
        fitment_core::sha256_hex(format!("{key:?}").as_bytes())
    ));
    std::fs::write(&path, &got).unwrap();
    let decisions = gate::decisions(case.value(), &sockets).unwrap();
    if decisions.is_empty() {
        return Some((revision, got, sockets));
    }
    let body = json!({ "decisions": decisions.iter().map(|d| json!({
        "requirement": d.requirement, "socket": d.socket, "status": d.status, "note": d.note })).collect::<Vec<_>>() });
    let reply = server
        .send(Call::json(
            axum::http::Method::POST,
            &format!("/v1/sockets/{revision}/review"),
            &body,
        ))
        .await;
    for d in &decisions {
        let flag = match d.status.as_str() {
            "removed" => "--remove",
            "kept" => "--keep",
            _ => "--unreview",
        };
        let shown = path.display().to_string();
        let mut args = vec![
            "review",
            shown.as_str(),
            flag,
            d.requirement.as_str(),
            "--socket",
            d.socket.as_str(),
        ];
        if let Some(note) = &d.note {
            args.extend(["--note", note.as_str()]);
        }
        assert_eq!(cli(scratch, &args).0, Some(0), "{what}: fitment review");
    }
    let reviewed = std::fs::read(&path).unwrap();
    half.problems.extend(parity::bytes(
        &format!("{what} review"),
        &reviewed,
        &reply.body,
    ));
    reply
        .header("location")?
        .trim_start_matches("/v1/sockets/")
        .clone_into(&mut revision);
    half.count("reviews");
    Some((revision, reply.body, sockets))
}

/// XH3.6, KH2.6: a document passes the checker (the harness's XH11 runs it
/// on every response too), states the policy asked for and the socket's
/// digest, and names as sources exactly `sources`.
fn checks(
    doc: &Value,
    policy: Policy,
    socket_digest: &Value,
    sources: &BTreeSet<String>,
    what: &str,
) -> Vec<String> {
    let mut out: Vec<String> = checker::problems(doc)
        .into_iter()
        .map(|p| format!("{what}: {p}"))
        .collect();
    if doc["policy"] != fitment_corpus::outcome::policy_value(&policy.match_policy()) {
        out.push(format!("{what}: the policy {}", doc["policy"]));
    }
    if &doc["socket"]["digest"] != socket_digest {
        out.push(format!("{what}: another socket's digest"));
    }
    let named: BTreeSet<String> = doc["candidates"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|c| match c["source"]["kind"].as_str() {
            Some("file") => c["source"]["sha256"].as_str().unwrap_or("").to_owned(),
            _ => format!(
                "{}#{}",
                c["source"]["assembly_sha256"].as_str().unwrap_or(""),
                c["source"]["definition"]
            ),
        })
        .collect();
    if &named != sources {
        out.push(format!("{what}: its sources are another job's"));
    }
    out
}

/// XH3.4: one match job per policy over the key's file candidates, labelled
/// by their file names, the CLI's `fitment match S f1 f2 …` byte for byte;
/// each case's outcome found by its file's SHA-256 (SC9).
#[allow(clippy::too_many_arguments)] // one key's sequence
async fn file_runs(
    server: &TestServer,
    corpus: &Corpus,
    socket: (&str, &[u8], &Path),
    cases: &[(Case<'_>, String)],
    uploaded: &mut BTreeSet<String>,
    half: &mut FileHalf,
) {
    let (revision, bytes, socket_file) = socket;
    let mut files: Vec<&str> = cases.iter().map(|(_, f)| f.as_str()).collect();
    files.sort_unstable();
    files.dedup();
    let mut list = Vec::new();
    let mut sources = BTreeSet::new();
    for file in &files {
        let content = std::fs::read(corpus.dir().join(file)).unwrap();
        let sha = fitment_core::sha256_hex(&content);
        if uploaded.insert(sha.clone()) {
            server.send(support::put_upload(&content)).await;
        }
        sources.insert(sha.clone());
        list.push(json!({ "upload": format!("sha256:{sha}"), "label": file }));
    }
    std::fs::write(socket_file, bytes).unwrap();
    let digest: Value = serde_json::from_slice::<Value>(bytes).unwrap()["digest"].clone();
    let shown = socket_file.display().to_string();
    for policy in Policy::M4 {
        let body = json!({ "socket": revision, "candidates": { "files": list }, "policy": policy.request() });
        let job = server.run_match(&body).await;
        let mut args = vec!["match", shown.as_str()];
        args.extend(files.iter().copied());
        args.extend(["--format", "json"]);
        args.extend(policy.cli_args().iter().copied());
        let (code, stdout, _) = cli_in(corpus, &args);
        let what = format!("{revision} {}", policy.name());
        let Some(got) = document_of(server, &job, &what, half).await else {
            continue;
        };
        half.problems.extend(parity::bytes(&what, &stdout, &got));
        if job["exit_code"].as_i64() != code.map(i64::from) {
            half.problems.push(format!(
                "{what}: exit {} and the CLI's {code:?} (XH7)",
                job["exit_code"]
            ));
        }
        let doc: Value = serde_json::from_slice(&got).unwrap();
        half.problems
            .extend(checks(&doc, policy, &digest, &sources, &what));
        for (case, file) in cases {
            let sha = fitment_core::sha256_hex(&std::fs::read(corpus.dir().join(file)).unwrap());
            push_run(half, *case, policy, Outcome::find(&doc, &sha));
        }
        // By the key's first case, which no revision number of either
        // server names (XH9).
        half.documents
            .insert(format!("{} {}", cases[0].0.id(), policy.name()), got);
        half.count("file runs");
    }
}

fn push_run(half: &mut FileHalf, case: Case<'_>, policy: Policy, outcome: Result<Outcome, String>) {
    match outcome {
        Ok(outcome) => half.runs.push(Run {
            case: case.id().to_owned(),
            policy,
            outcome,
            path: RunPath::Api,
            at_construction: None,
        }),
        Err(e) => half
            .problems
            .push(format!("{} {}: {e}", case.id(), policy.name())),
    }
}

/// XH3.5: a component candidate's match job per policy, the library's
/// `MatchDocument` of `judge_component` with the API's default label, byte
/// for byte (DC3, CO5).
#[allow(clippy::too_many_arguments)] // one case's sequence
async fn component_runs(
    server: &TestServer,
    corpus: &Corpus,
    socket: (&str, &[u8]),
    case: Case<'_>,
    source: &str,
    part: &str,
    registered: &mut BTreeSet<String>,
    half: &mut FileHalf,
) {
    let (revision, bytes) = socket;
    let sha = register(server, corpus, source, registered, half).await;
    let assembly = std::fs::read(corpus.dir().join(format!("{source}.stp"))).unwrap();
    let document = SocketDocument::from_json(&String::from_utf8_lossy(bytes)).unwrap();
    let component = SelectedComponent::read(&assembly, Some(part), None).unwrap();
    let digest: Value = serde_json::from_slice::<Value>(bytes).unwrap()["digest"].clone();
    for policy in Policy::M4 {
        let body = json!({ "socket": revision, "policy": policy.request(), "candidates": {
            "components": [{ "assembly": format!("sha256:{sha}"), "part": part }] } });
        let job = server.run_match(&body).await;
        let matcher = Matcher::new(
            &document,
            policy.match_policy(),
            MatchOptions::default(),
            &NoFitVerifier,
        )
        .unwrap();
        let label = format!("component:{}/#{}", &sha[..12], component.definition());
        let expected = MatchDocument::new(
            &matcher,
            vec![(
                matcher.judge_component(&component, component.source()),
                vec![label],
            )],
        );
        let what = format!("{} {}", case.id(), policy.name());
        let Some(got) = document_of(server, &job, &what, half).await else {
            continue;
        };
        half.problems
            .extend(parity::bytes(&what, expected.to_json().as_bytes(), &got));
        let doc: Value = serde_json::from_slice(&got).unwrap();
        let sources = BTreeSet::from([format!("{sha}#{}", component.definition())]);
        half.problems
            .extend(checks(&doc, policy, &digest, &sources, &what));
        push_run(
            half,
            case,
            policy,
            Outcome::find_component(&doc, &sha, component.definition()),
        );
        half.documents.insert(what, got);
        half.count("component runs");
    }
}

/// XH3 for one socket key: its source, its socket and review, its file
/// runs and its component runs.
async fn one_key(
    server: &TestServer,
    corpus: &Corpus,
    scratch: &Path,
    (key, group): (&SocketKey, &[Case<'_>]),
    seen: &mut (BTreeSet<String>, BTreeSet<String>),
    half: &mut FileHalf,
) {
    let (registered, uploaded) = seen;
    let Some(first) = group.first().copied() else {
        return;
    };
    register(server, corpus, &key.source, registered, half).await;
    let Some((revision, bytes, sockets)) =
        socket_of(server, corpus, scratch, key, first, half).await
    else {
        return;
    };
    half.sockets.insert(key.clone(), sockets);
    let files: Vec<(Case<'_>, String)> = group
        .iter()
        .filter_map(|c| match c.candidate() {
            CandidateRef::File(file) => Some((*c, file)),
            CandidateRef::Component { .. } => None,
        })
        .collect();
    if !files.is_empty() {
        let socket_file = scratch.join(format!(
            "S-{}.json",
            fitment_core::sha256_hex(revision.as_bytes())
        ));
        file_runs(
            server,
            corpus,
            (&revision, &bytes, &socket_file),
            &files,
            uploaded,
            half,
        )
        .await;
    }
    for case in group {
        if let CandidateRef::Component { source, part } = case.candidate() {
            component_runs(
                server,
                corpus,
                (&revision, &bytes),
                *case,
                &source,
                &part,
                registered,
                half,
            )
            .await;
        }
    }
}

/// The file half on `server`: every non-gate socket key in manifest order,
/// or in reverse.
async fn file_half(server: &TestServer, corpus: &Corpus, reverse: bool) -> FileHalf {
    let mut half = FileHalf::default();
    let scratch = server.dirs.root.join("exit");
    std::fs::create_dir_all(&scratch).unwrap();
    let groups = corpus.socket_groups();
    let mut keys: Vec<(&SocketKey, Vec<Case<'_>>)> = groups
        .iter()
        .map(|(k, g)| {
            (
                k,
                g.iter()
                    .copied()
                    .filter(|c| !c.socket_gate())
                    .collect::<Vec<_>>(),
            )
        })
        .filter(|(_, g)| !g.is_empty())
        .collect();
    if reverse {
        keys.reverse();
    }
    let mut seen = (BTreeSet::new(), BTreeSet::new());
    for (key, group) in &keys {
        one_key(server, corpus, &scratch, (key, group), &mut seen, &mut half).await;
    }
    half
}

/// The built server, a child, over loopback (M6-36, XH2).
fn process(extra: &str) -> TestServer {
    TestServer::process(Dirs::new(), &config(extra))
}

/// VT1, XH1.1, XH3 to XH7, XH10, XH11: every match case of the corpus,
/// under `default` and `functional`, file and component candidates alike,
/// through the API: documents byte for byte the CLI's (or the library's),
/// every case held to the strict gate, no false positive, the false
/// negatives the baseline's, the report written to `api-report.json`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_corpus_passes_through_the_api() {
    let Some(corpus) = corpus() else {
        return;
    };
    let server = process("");
    let half = file_half(&server, &corpus, false).await;
    let mut problems = half.problems.clone();
    for run in &half.runs {
        let case = corpus.case(&run.case).unwrap();
        let sockets = &half.sockets[&case.socket()];
        problems.extend(case.gate(run.policy, sockets, &run.outcome));
    }
    let gated = corpus
        .match_cases()
        .iter()
        .filter(|c| !c.socket_gate())
        .count();
    eprintln!(
        "the exit's file half: {} runs of {gated} cases under 2 policies; {:?}",
        half.runs.len(),
        half.counted
    );
    assert!(
        half.runs.len() >= gated * 2,
        "XH10: {} runs, fewer than the manifest's",
        half.runs.len()
    );
    let score = Score::new(&corpus, half.runs);
    let found = score.false_negatives().unwrap_or_else(|e| panic!("{e}"));
    if let Err(e) = baseline::compare(&found, &score.ran()) {
        problems.push(e);
    }
    let fps = score.false_positives();
    eprintln!("{}", score.report_text());
    let path = report::write("api-report.json", &score.report()).unwrap();
    eprintln!("wrote {}", path.display());
    assert!(fps.is_empty(), "false positives: {fps:?}");
    assert!(
        problems.is_empty(),
        "{} problems:\n{}",
        problems.len(),
        problems.join("\n")
    );
}

/// XH9: the file half on two fresh servers, one on one CPU with its jobs
/// in manifest order, one on the default budget in reverse order: every
/// document the same bytes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_exit_is_the_same_twice() {
    let Some(corpus) = corpus() else {
        return;
    };
    let one = process("cpu = 1\n");
    let first = file_half(&one, &corpus, false).await;
    drop(one);
    let other = process("");
    let second = file_half(&other, &corpus, true).await;
    assert!(
        first.documents.len() > 50,
        "{} documents",
        first.documents.len()
    );
    let differ: Vec<&String> = first
        .documents
        .iter()
        .filter(|(k, v)| second.documents.get(*k) != Some(*v))
        .map(|(k, _)| k)
        .collect();
    assert_eq!(first.documents.len(), second.documents.len());
    assert!(
        differ.is_empty(),
        "{} documents differ: {differ:?}",
        differ.len()
    );
}

/// XH1.2, UP6, JB2, VD9: each socket-gate case (an edit: stale, its digest
/// broken, unreviewed under `require_reviewed`; and r27's refused
/// document), its socket uploaded edited, is refused with its code at the
/// request, before the unreadable candidate beside it is read, and no job
/// is made: the CLI's exit 3 for the same file.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_socket_gate_refuses_through_the_api() {
    let Some(corpus) = corpus() else {
        return;
    };
    let server = process("");
    let junk = server.upload(b"no STEP file: never read\n").await;
    let mut seen = 0;
    let mut registered = BTreeSet::new();
    let mut half = FileHalf::default();
    for case in corpus.match_cases().into_iter().filter(Case::socket_gate) {
        seen += 1;
        let key = case.socket();
        let sha = register(&server, &corpus, &key.source, &mut registered, &mut half).await;
        let body = json!({ "assembly": format!("sha256:{sha}"), "part": key.part, "carry": false });
        let job = server.send(waiting("/v1/sockets", &body)).await.json();
        let job = server
            .finished_job(job["id"].as_str().unwrap(), Duration::from_secs(300))
            .await;
        let bytes = server.document(&job).await.body;
        let mut doc: Value = serde_json::from_slice(&bytes).unwrap();
        let (code, require_reviewed) = match key.edit.as_deref() {
            Some("stale") => {
                doc["socket_rules_version"] =
                    json!(doc["socket_rules_version"].as_u64().unwrap() - 1);
                ("socket_stale", false)
            }
            Some("digest") => {
                let reach = &mut doc["extracted"]["sockets"][0]["requirements"][0]["id"];
                *reach = json!(format!("{}-edited", reach.as_str().unwrap()));
                ("socket_unreadable", false)
            }
            Some("unreviewed") => ("socket_unreviewed", true),
            _ => ("socket_refused", false),
        };
        let edited = serde_json::to_vec_pretty(&doc).unwrap();
        let upload = server.upload(&edited).await;
        let before = server.get("/v1/jobs?kind=match").await.json()["items"]
            .as_array()
            .unwrap()
            .len();
        let body = json!({ "socket": { "upload": format!("sha256:{upload}") },
                           "options": { "require_reviewed": require_reviewed },
                           "candidates": { "files": [{ "upload": format!("sha256:{junk}"), "label": "junk.stp" }] } });
        let reply = server.send(waiting("/v1/matches", &body)).await;
        assert_eq!(
            (reply.status.as_u16(), reply.code().as_str()),
            (422, code),
            "{}",
            case.id()
        );
        let after = server.get("/v1/jobs?kind=match").await.json()["items"]
            .as_array()
            .unwrap()
            .len();
        assert_eq!(before, after, "{}: no job", case.id());
        let file = server.dirs.root.join(format!("{}.json", case.id()));
        std::fs::write(&file, &edited).unwrap();
        let shown = file.display().to_string();
        let mut args = vec![
            "match",
            shown.as_str(),
            "no-such-candidate.stp",
            "--format",
            "json",
        ];
        if require_reviewed {
            args.push("--require-reviewed");
        }
        let (exit, stdout, _) = cli_in(&corpus, &args);
        assert_eq!((exit, stdout.is_empty()), (Some(3), true), "{}", case.id());
    }
    assert!(seen >= 4, "{seen} socket gate cases");
    assert!(half.problems.is_empty(), "{}", half.problems.join("\n"));
}

/// The assembly cases with expected sockets, and their variants with
/// their base's targets, as `original.rs` reads them: (id, file, parts).
fn targets(corpus: &Corpus) -> Vec<(String, String, Vec<String>)> {
    let all = corpus.cases();
    let of = |id: &str| -> Option<Vec<String>> {
        let base = all.iter().find(|c| c["id"] == id)?;
        Some(
            base["expect"]["sockets"]
                .as_object()?
                .keys()
                .cloned()
                .collect(),
        )
    };
    all.iter()
        .filter(|c| c["kind"] == "assembly")
        .filter_map(|c| {
            let id = c["id"].as_str()?.to_owned();
            let parts = match c["construction"]["variant_of"].as_str() {
                Some(base) => of(base)?,
                None => of(&id)?,
            };
            Some((id, c["file"].as_str()?.to_owned(), parts))
        })
        .collect()
}

/// The plain block of KC13's runs: a part with no hole or shaft.
fn plain() -> Vec<u8> {
    std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../fitment-core/tests/data/p21_planes.stp"),
    )
    .unwrap()
}

/// What KC13 removes from a socket: each active requirement no candidate
/// can pass under D9's default (unverifiable, or blocked by the target's
/// own uncertainty).
fn waivable(document: &SocketDocument) -> Vec<(String, String)> {
    let policy = fitment_core::MatchPolicy::default();
    let mut removed = Vec::new();
    for socket in &document.extracted().unwrap().sockets {
        let view = fitment_core::SocketView::new(document, &socket.id).unwrap();
        for r in view.active() {
            if !r.verifiable() || view.blocked(r, &policy) {
                removed.push((socket.id.clone(), r.id.clone()));
            }
        }
    }
    removed
}

/// XH1.3, KC13 (`original.rs::review_waives_only_what_it_removes`): every
/// corpus socket whose original fails requirements no candidate can pass,
/// with those removed in review through the API: the original then passes
/// (the job succeeds), each socket of the document's `original` checks
/// exactly what was kept and lists the removed as not checked; a plain
/// block is still rejected by a kept hole or shaft.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn review_waives_only_what_it_removes_through_the_api() {
    let Some(corpus) = corpus() else {
        return;
    };
    let server = process("");
    let block = server.upload(&plain()).await;
    let mut runs = 0;
    let mut problems = Vec::new();
    let mut registered = BTreeSet::new();
    let mut half = FileHalf::default();
    for (id, _, parts) in targets(&corpus) {
        if ["_rot", "_tgt", "_pc", "_perm"]
            .iter()
            .any(|v| id.ends_with(v))
        {
            continue;
        }
        for part in parts {
            let bytes = std::fs::read(corpus.dir().join(format!("{id}.stp"))).unwrap();
            let document = SocketDocument::extract(&bytes, Some(&part), None).unwrap();
            let Some(extracted) = document.extracted().cloned() else {
                continue;
            };
            let removed = waivable(&document);
            let active: usize = extracted.sockets.iter().map(|s| s.requirements.len()).sum();
            if removed.is_empty() || removed.len() == active {
                continue;
            }
            runs += 1;
            let sha = register(&server, &corpus, &id, &mut registered, &mut half).await;
            let body = json!({ "assembly": format!("sha256:{sha}"), "part": part, "carry": false });
            let job = server.send(waiting("/v1/sockets", &body)).await.json();
            let job = server
                .finished_job(job["id"].as_str().unwrap(), Duration::from_secs(300))
                .await;
            let revision = job["result"]["href"]
                .as_str()
                .unwrap()
                .trim_start_matches("/v1/sockets/")
                .to_owned();
            let decisions: Vec<Value> = removed
                .iter()
                .map(|(s, r)| {
                    json!({
                "requirement": r, "socket": s, "status": "removed", "note": "waived" })
                })
                .collect();
            let reply = server
                .send(Call::json(
                    axum::http::Method::POST,
                    &format!("/v1/sockets/{revision}/review"),
                    &json!({ "decisions": decisions }),
                ))
                .await;
            let reviewed = reply
                .header("location")
                .unwrap()
                .trim_start_matches("/v1/sockets/")
                .to_owned();
            let matched = server
                .run_match(
                    &json!({ "socket": reviewed, "original": true, "candidates": { "files": [
                    { "upload": format!("sha256:{block}"), "label": "plain.stp" } ] } }),
                )
                .await;
            let name = format!("{id} {part}");
            problems
                .extend(kept_checked(&server, &matched, &extracted, &removed, &name, &block).await);
        }
    }
    assert!(runs > 5, "{runs} runs");
    problems.extend(half.problems);
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// KC13's checks on a match's document: the job succeeded (the original
/// passes); each socket of its `original` checks exactly the kept
/// requirements, rejects nothing it kept, and lists every removed one as
/// not checked; the plain block is rejected.
async fn kept_checked(
    server: &TestServer,
    job: &Value,
    extracted: &fitment_core::Extracted,
    removed: &[(String, String)],
    name: &str,
    block: &str,
) -> Vec<String> {
    let mut out = Vec::new();
    if job["state"] != "succeeded" {
        out.push(format!("{name}: {} {}", job["state"], job["error"]["code"]));
        return out;
    }
    let doc: Value = serde_json::from_slice(&server.document(job).await.body).unwrap();
    let original = Outcome::of_entry(&doc["original"], &doc["policy"]).unwrap();
    for socket in &original.sockets {
        let want: BTreeSet<String> = extracted
            .sockets
            .iter()
            .filter(|s| s.id == socket.socket)
            .flat_map(|s| &s.requirements)
            .map(|r| r.id.clone())
            .filter(|r| !removed.iter().any(|(s, q)| *s == socket.socket && q == r))
            .collect();
        let checked: BTreeSet<String> = socket.checked.iter().cloned().collect();
        if checked != want {
            out.push(format!(
                "{name} {}: checks {checked:?}, not {want:?}",
                socket.socket
            ));
        }
        if !want.is_empty() && socket.verdict == "reject" {
            out.push(format!(
                "{name} {}: the original rejected after review",
                socket.socket
            ));
        }
        for (_, r) in removed.iter().filter(|(s, _)| *s == socket.socket) {
            if !socket.removed.contains(r) {
                out.push(format!("{name}: {r} not listed as removed"));
            }
        }
    }
    // Where review kept a hole or a shaft, the plain block, which has none,
    // is rejected (as `original.rs`'s `plain_rejected` holds it).
    let axial = extracted.sockets.iter().any(|s| {
        s.requirements.iter().any(|r| {
            matches!(
                r.requires,
                fitment_core::Requires::Bore { .. } | fitment_core::Requires::Shaft { .. }
            ) && !removed.iter().any(|(sk, q)| *sk == s.id && *q == r.id)
        })
    });
    let plain = Outcome::find(&doc, block).unwrap();
    if axial && plain.verdict != "reject" {
        out.push(format!("{name}: a plain block passes a kept hole or shaft"));
    }
    out
}

/// XH1.4, KC11, VD15, SN9: every corpus target's original, in its own
/// frame, judged through `original: true` beside a plain block: the job
/// succeeds (the original fails nothing beyond VD5 and VD6, s55's plate
/// excused by its bound, as the CLI notes it), and the document is the
/// CLI's `fitment match S plain.stp --original SRC` byte for byte.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_original_through_the_api() {
    let Some(corpus) = corpus() else {
        return;
    };
    let server = process("");
    let plain_bytes = plain();
    let block = server.upload(&plain_bytes).await;
    let scratch: PathBuf = server.dirs.root.join("originals");
    std::fs::create_dir_all(&scratch).unwrap();
    std::fs::write(scratch.join("plain.stp"), &plain_bytes).unwrap();
    let mut registered = BTreeSet::new();
    let mut half = FileHalf::default();
    let mut runs = 0;
    for (id, file, parts) in targets(&corpus) {
        let sha = register(&server, &corpus, &id, &mut registered, &mut half).await;
        for part in parts {
            let body = json!({ "assembly": format!("sha256:{sha}"), "part": part, "carry": false });
            let job = server.send(waiting("/v1/sockets", &body)).await.json();
            let job = server
                .finished_job(job["id"].as_str().unwrap(), Duration::from_secs(300))
                .await;
            let Some(href) = job["result"]["href"].as_str() else {
                continue;
            };
            let revision = href.trim_start_matches("/v1/sockets/").to_owned();
            let socket = server.get(href).await.body;
            std::fs::write(scratch.join("S.json"), &socket).unwrap();
            std::fs::copy(corpus.dir().join(&file), scratch.join(&file)).unwrap();
            let matched = server
                .run_match(
                    &json!({ "socket": revision, "original": true, "candidates": { "files": [
                    { "upload": format!("sha256:{block}"), "label": "plain.stp" } ] } }),
                )
                .await;
            let (code, stdout, _) = cli(
                &scratch,
                &[
                    "match",
                    "S.json",
                    "plain.stp",
                    "--original",
                    &file,
                    "--format",
                    "json",
                ],
            );
            let what = format!("{id} {part}");
            if matched["state"] != "succeeded" {
                half.problems.push(format!(
                    "{what}: {} {}",
                    matched["state"], matched["error"]["code"]
                ));
            }
            if matched["exit_code"].as_i64() != code.map(i64::from) {
                half.problems.push(format!(
                    "{what}: exit {} and the CLI's {code:?}",
                    matched["exit_code"]
                ));
            }
            if let Some(got) = document_of(&server, &matched, &what, &mut half).await {
                half.problems.extend(parity::bytes(&what, &stdout, &got));
            }
            let _ = std::fs::remove_file(scratch.join(&file));
            runs += 1;
        }
    }
    eprintln!("{runs} originals through the API");
    assert!(runs > 50, "{runs} originals");
    assert!(half.problems.is_empty(), "{}", half.problems.join("\n"));
}

// ------------------------------------------------------- the catalog half

/// The CLI's run of `fitment match S --catalog DIR …` in `scratch`.
fn cli_catalog_match(
    scratch: &Path,
    socket: &Path,
    dir: &Path,
    extra: &[&str],
) -> (Option<i32>, Vec<u8>, String) {
    let (socket, dir) = (socket.display().to_string(), dir.display().to_string());
    let mut args = vec!["match", socket.as_str(), "--catalog", dir.as_str()];
    args.extend(extra.iter().copied());
    args.extend(["--format", "json"]);
    cli(scratch, &args)
}

/// What the catalog half found: its runs' counts and problems.
#[derive(Default)]
struct CatalogHalf {
    problems: Vec<String>,
    counted: BTreeMap<&'static str, usize>,
    /// The labels whose bytes were downloaded and checked (XH8.4).
    labels: BTreeSet<String>,
}

impl CatalogHalf {
    fn count(&mut self, what: &'static str) {
        *self.counted.entry(what).or_default() += 1;
    }
}

/// XH8.1: `files` uploaded and ingested in one job, in sorted name order,
/// and by the CLI into `cli`, a fresh catalog, in the same order: each
/// line and entry the CLI's, the generation masked (DB3's batches end by
/// time too). The entries' content to id, for the named runs.
async fn ingested(
    server: &TestServer,
    corpus: &Corpus,
    cli_catalog: &Path,
    half: &mut CatalogHalf,
) -> BTreeMap<String, String> {
    let names: Vec<String> = corpus
        .part_files()
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    let mut files = Vec::with_capacity(names.len());
    for name in &names {
        let sha = server
            .upload(&std::fs::read(corpus.dir().join(name)).unwrap())
            .await;
        files.push(json!({ "upload": format!("sha256:{sha}"), "file_name": name }));
    }
    let job = server
        .send(waiting("/v1/catalog/entries", &json!({ "files": files })))
        .await
        .json();
    let job = server
        .finished_job(job["id"].as_str().unwrap(), Duration::from_secs(1800))
        .await;
    let shown = cli_catalog.display().to_string();
    assert_eq!(
        cli_in(corpus, &["catalog", "init", "-C", &shown]).0,
        Some(0)
    );
    let mut args = vec![
        "catalog",
        "ingest",
        "-C",
        shown.as_str(),
        "--format",
        "json",
    ];
    args.extend(names.iter().map(String::as_str));
    let (code, stdout, _) = cli_in(corpus, &args);
    if job["exit_code"].as_i64() != code.map(i64::from) {
        half.problems.push(format!(
            "ingest: exit {} and the CLI's {code:?}",
            job["exit_code"]
        ));
    }
    let got = server.document(&job).await.body;
    half.problems
        .extend(parity::bytes("ingest", &masked(&stdout), &masked(&got)));
    half.count("files ingested");
    let listed = server.get("/v1/catalog/entries?limit=all").await.json();
    listed["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            (
                e["sha256"].as_str().unwrap().to_owned(),
                e["id"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

/// A `fitment.catalog` document's bytes with its generation written `_`.
fn masked(document: &[u8]) -> Vec<u8> {
    let text = String::from_utf8_lossy(document);
    text.lines()
        .map(|l| {
            if l.starts_with("  \"generation\": ") {
                "  \"generation\": _,".to_owned()
            } else {
                l.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        .into_bytes()
}

/// XH8.4: every label of a catalog run's document is `catalog:<id>@r<n>`,
/// and the API's download of it returns the bytes its `source` states.
async fn labels_download(server: &TestServer, doc: &Value, what: &str, half: &mut CatalogHalf) {
    for candidate in doc["candidates"].as_array().into_iter().flatten() {
        let label = candidate["labels"][0].as_str().unwrap_or("").to_owned();
        let Some(reference) = label.strip_prefix("catalog:") else {
            half.problems.push(format!("{what}: the label {label}"));
            continue;
        };
        if !half.labels.insert(label.clone()) {
            continue;
        }
        let bytes = server
            .get(&format!("/v1/catalog/entries/{reference}/step"))
            .await
            .body;
        if candidate["source"]["sha256"] != fitment_core::sha256_hex(&bytes).as_str() {
            half.problems
                .push(format!("{what}: {label}'s bytes are not its source's"));
        }
    }
}

/// XH8.2: one socket's full and exhaustive runs under `policy`, each the
/// CLI's `fitment match S --catalog <the server's catalog>` byte for byte,
/// the full run's digest the CLI's on the CLI-built catalog too; a blocked
/// socket refused (`socket_blocked`, exit 3), as the CLI refuses it.
async fn catalog_runs(
    server: &TestServer,
    (revision, socket_file): (&str, &Path),
    cli_catalog: &Path,
    policy: Policy,
    half: &mut CatalogHalf,
) {
    let scratch = socket_file.parent().unwrap();
    for (selection, flags) in [("full", vec![]), ("exhaustive", vec!["--exhaustive"])] {
        let body = json!({ "socket": revision, "policy": policy.request(),
                           "candidates": { "catalog": { "selection": selection } } });
        let job = server.run_match(&body).await;
        let mut extra = flags.clone();
        extra.extend(policy.cli_args().iter().copied());
        let (code, stdout, _) =
            cli_catalog_match(scratch, socket_file, &server.dirs.catalog, &extra);
        let what = format!("{revision} {selection} {}", policy.name());
        if job["exit_code"].as_i64() != code.map(i64::from) {
            half.problems.push(format!(
                "{what}: exit {} and the CLI's {code:?}",
                job["exit_code"]
            ));
        }
        if stdout.is_empty() {
            if !job["document"].is_null() {
                half.problems
                    .push(format!("{what}: a document where the CLI printed none"));
            }
            if job["error"]["code"] == "socket_blocked" {
                half.count("full runs blocked");
            }
            continue;
        }
        let got = server.document(&job).await.body;
        half.problems.extend(parity::bytes(&what, &stdout, &got));
        let doc: Value = serde_json::from_slice(&got).unwrap();
        half.problems.extend(
            checker::problems(&doc)
                .into_iter()
                .map(|p| format!("{what}: {p}")),
        );
        if selection == "full" {
            let (_, twin, _) = cli_catalog_match(scratch, socket_file, cli_catalog, &extra);
            let twin: Value = serde_json::from_slice(&twin).unwrap_or(Value::Null);
            if twin["digest"] != doc["digest"] {
                half.problems.push(format!(
                    "{what}: another digest than the CLI-built catalog's"
                ));
            }
            labels_download(server, &doc, &what, half).await;
        }
        half.count(if selection == "full" {
            "full runs"
        } else {
            "exhaustive runs"
        });
    }
}

/// XH8.3: each case's file candidate as a named run under the case's
/// policies: its digest the file run's over that file (the library's, the
/// same labels aside: MF7), its entry held to the strict gate (XH5).
#[allow(clippy::too_many_arguments)] // one key's named runs
async fn named_runs(
    server: &TestServer,
    corpus: &Corpus,
    (revision, bytes): (&str, &[u8]),
    sockets: &[Socket],
    group: &[Case<'_>],
    ids: &BTreeMap<String, String>,
    half: &mut CatalogHalf,
) {
    let document = SocketDocument::from_json(&String::from_utf8_lossy(bytes)).unwrap();
    for case in group {
        let CandidateRef::File(file) = case.candidate() else {
            continue;
        };
        let content = std::fs::read(corpus.dir().join(&file)).unwrap();
        let sha = fitment_core::sha256_hex(&content);
        let Some(id) = ids.get(&sha) else {
            half.problems
                .push(format!("{}: {file} is no entry", case.id()));
            continue;
        };
        for policy in Policy::M4 {
            let body = json!({ "socket": revision, "policy": policy.request(),
                               "candidates": { "catalog": { "ids": [id] } } });
            let job = server.run_match(&body).await;
            let what = format!("{} named {}", case.id(), policy.name());
            let Ok(doc) = serde_json::from_slice::<Value>(&server.document(&job).await.body) else {
                half.problems.push(format!("{what}: no document: {job}"));
                continue;
            };
            let matcher = Matcher::new(
                &document,
                policy.match_policy(),
                MatchOptions::default(),
                &NoFitVerifier,
            )
            .unwrap();
            let expected = MatchDocument::new(
                &matcher,
                vec![(matcher.judge_step(&content), vec![file.clone()])],
            );
            if doc["digest"] != expected.digest() {
                half.problems
                    .push(format!("{what}: another digest than the file run's"));
            }
            match Outcome::find(&doc, &sha) {
                Ok(outcome) => half.problems.extend(case.gate(policy, sockets, &outcome)),
                Err(e) => half.problems.push(format!("{what}: {e}")),
            }
            half.count("named runs");
        }
    }
}

/// VT2, XH8, XH1.5, XH2, SN10, M6-36: the corpus through the API's
/// catalog: every part file but the hostile ones ingested, each line the
/// CLI's; for every socket of the exit set and each of three policies
/// (`default`, `functional`, `tolerance_mm` 0.05, as CT1), the full and the
/// exhaustive run the CLI's `fitment match S --catalog <the server's
/// catalog>` byte for byte, the full run's digest the CLI-built catalog's,
/// a blocked socket refused as M5 refuses it; each case's file a named
/// run whose digest is the file run's and whose entry passes the strict
/// gate; every label downloads to its source's bytes; and afterwards,
/// the server idle, `fitment catalog verify --deep` on its catalog
/// reports nothing. Through the built server over loopback (XH2).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_corpus_through_the_apis_catalog_is_the_cli_run() {
    let Some(corpus) = corpus() else {
        return;
    };
    let server = process("[limits]\nmax_candidates = 100000\n");
    let scratch = server.dirs.root.join("exit-catalog");
    std::fs::create_dir_all(&scratch).unwrap();
    let cli_catalog = scratch.join("cli");
    let mut half = CatalogHalf::default();
    let ids = ingested(&server, &corpus, &cli_catalog, &mut half).await;
    let mut file_half_problems = FileHalf::default();
    let mut registered = BTreeSet::new();
    let groups = corpus.socket_groups();
    for (key, group) in &groups {
        let group: Vec<Case<'_>> = group.iter().copied().filter(|c| !c.socket_gate()).collect();
        let Some(first) = group.first().copied() else {
            continue;
        };
        register(
            &server,
            &corpus,
            &key.source,
            &mut registered,
            &mut file_half_problems,
        )
        .await;
        let Some((revision, bytes, sockets)) = socket_of(
            &server,
            &corpus,
            &scratch,
            key,
            first,
            &mut file_half_problems,
        )
        .await
        else {
            continue;
        };
        let socket_file = scratch.join(format!(
            "C-{}.json",
            fitment_core::sha256_hex(revision.as_bytes())
        ));
        std::fs::write(&socket_file, &bytes).unwrap();
        for policy in [Policy::Default, Policy::Functional, Policy::Tolerance005] {
            catalog_runs(
                &server,
                (&revision, &socket_file),
                &cli_catalog,
                policy,
                &mut half,
            )
            .await;
        }
        named_runs(
            &server,
            &corpus,
            (&revision, &bytes),
            &sockets,
            &group,
            &ids,
            &mut half,
        )
        .await;
    }
    half.problems.extend(file_half_problems.problems);
    eprintln!("the exit's catalog half: {:?}", half.counted);
    let deep = cli(
        &scratch,
        &[
            "catalog",
            "verify",
            "-C",
            &server.dirs.catalog.display().to_string(),
            "--deep",
        ],
    );
    assert_eq!(deep.0, Some(0), "XH8.5: verify --deep: {}", deep.2);
    let file_cases = corpus
        .match_cases()
        .iter()
        .filter(|c| !c.socket_gate() && matches!(c.candidate(), CandidateRef::File(_)))
        .count();
    assert!(
        half.counted.get("named runs").copied().unwrap_or(0) >= file_cases * 2,
        "XH10: {:?}",
        half.counted
    );
    assert!(
        half.counted.get("full runs").copied().unwrap_or(0)
            + half.counted.get("full runs blocked").copied().unwrap_or(0)
            >= 60
    );
    assert!(
        half.problems.is_empty(),
        "{} problems:\n{}",
        half.problems.len(),
        half.problems.join("\n")
    );
}

/// VT2, CT10, QR9: a catalog of the corpus's four hostile parts (`h_*`)
/// alone, every corpus socket run against it, full and exhaustive: each
/// run within M4's hostile bound of 30 s and its job's deadline, each the
/// CLI's `fitment match S --catalog` byte for byte.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hostile_parts_through_the_apis_catalog() {
    let Some(corpus) = corpus() else {
        return;
    };
    let server = process("");
    let scratch = server.dirs.root.join("exit-hostile");
    std::fs::create_dir_all(&scratch).unwrap();
    let hostile: Vec<(Vec<u8>, String)> = std::fs::read_dir(corpus.dir())
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.file_name().unwrap().to_string_lossy().starts_with("h_"))
        .filter(|p| p.extension().is_some_and(|e| e == "stp"))
        .map(|p| {
            (
                std::fs::read(&p).unwrap(),
                p.file_name().unwrap().to_string_lossy().into_owned(),
            )
        })
        .collect();
    assert_eq!(hostile.len(), 4, "the four h_ parts");
    server.ingest(&hostile).await;
    let mut file_half = FileHalf::default();
    let mut registered = BTreeSet::new();
    let mut problems = Vec::new();
    let mut checked = 0;
    for (key, group) in &corpus.socket_groups() {
        let Some(first) = group.iter().copied().find(|c| !c.socket_gate()) else {
            continue;
        };
        register(
            &server,
            &corpus,
            &key.source,
            &mut registered,
            &mut file_half,
        )
        .await;
        let Some((revision, bytes, _)) =
            socket_of(&server, &corpus, &scratch, key, first, &mut file_half).await
        else {
            continue;
        };
        let socket_file = scratch.join(format!(
            "H-{}.json",
            fitment_core::sha256_hex(revision.as_bytes())
        ));
        std::fs::write(&socket_file, &bytes).unwrap();
        for (selection, flags) in [("full", vec![]), ("exhaustive", vec!["--exhaustive"])] {
            let started = std::time::Instant::now();
            let job = server
                .run_match(&json!({ "socket": revision, "candidates": { "catalog": { "selection": selection } } }))
                .await;
            let took = started.elapsed();
            if took > Duration::from_secs(30) {
                problems.push(format!("{revision} {selection}: {took:?}, past M4's bound"));
            }
            let (code, stdout, _) =
                cli_catalog_match(&scratch, &socket_file, &server.dirs.catalog, &flags);
            if job["exit_code"].as_i64() != code.map(i64::from) {
                problems.push(format!(
                    "{revision} {selection}: exit {} and {code:?}",
                    job["exit_code"]
                ));
            }
            if !stdout.is_empty() {
                let got = server.document(&job).await.body;
                problems.extend(parity::bytes(
                    &format!("{revision} {selection}"),
                    &stdout,
                    &got,
                ));
            }
            checked += 1;
        }
    }
    problems.extend(file_half.problems);
    assert!(checked > 40, "{checked} runs");
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}
