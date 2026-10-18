//! VT7's `features`, `parts` and `socket` rows (docs/research/m6-api.md,
//! DC1, DC3, JB4, XH4, XH7): each job's document is the CLI's stdout for
//! the same bytes, byte for byte, and its state and exit code are the
//! CLI's exit code mapped as JB3 says; a refused input's document too; a
//! selection error, the CLI's exit 2, is the job `refused`, exit 2, with
//! no document. On the committed unit parts and assemblies, every OS,
//! debug. The CLI is the built `fitment` (`support::cli`).

mod support;

use std::path::Path;

use serde_json::{Value, json};
use support::{Dirs, TestServer, cli, waiting};

/// The job's state for the CLI's exit `code` (JB3, XH7).
fn state_of(code: i32) -> &'static str {
    match code {
        0 | 4 | 5 => "succeeded",
        2 | 3 => "refused",
        _ => "failed",
    }
}

/// A directory holding `bytes` under `name`, for the CLI.
fn file_in(dirs: &Dirs, name: &str, bytes: &[u8]) -> std::path::PathBuf {
    let dir = dirs.root.join("cli");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(name), bytes).unwrap();
    dir
}

/// Compares a finished job and its document with the CLI's run.
async fn same(
    server: &TestServer,
    what: &str,
    job: &Value,
    cli: (Option<i32>, Vec<u8>, String),
    failures: &mut Vec<String>,
) {
    let (code, stdout, stderr) = cli;
    let code = code.unwrap_or(-1);
    if job["state"] != state_of(code) || job["exit_code"] != json!(code) {
        failures.push(format!(
            "{what}: the job {} exit {}, the CLI exit {code} ({stderr})",
            job["state"], job["exit_code"]
        ));
        return;
    }
    if stdout.is_empty() {
        if !job["document"].is_null() {
            failures.push(format!("{what}: a document where the CLI printed none"));
        }
        return;
    }
    let reply = server.document(job).await;
    assert_eq!(reply.status, 200, "{what}");
    if let Some(diff) = fitment_corpus::parity::bytes(what, &stdout, &reply.body) {
        failures.push(diff);
    }
    if reply.header("fitment-exit-code") != Some(code.to_string().as_str()) {
        failures.push(format!(
            "{what}: Fitment-Exit-Code {:?}",
            reply.header("fitment-exit-code")
        ));
    }
}

/// DC1, DC3, JB4: `fitment features FILE` and the job `features` on every
/// committed unit part, and on bytes that are no STEP file (exit 3,
/// `refused`, the refused document).
#[tokio::test]
async fn features_are_the_clis() {
    let server = TestServer::fresh();
    let data = Path::new(env!("CARGO_MANIFEST_DIR")).join("../fitment-core/tests/data");
    let mut files: Vec<(String, Vec<u8>)> = std::fs::read_dir(&data)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "stp"))
        .map(|p| {
            (
                p.file_name().unwrap().to_string_lossy().into_owned(),
                std::fs::read(&p).unwrap(),
            )
        })
        .collect();
    files.sort();
    files.push(("not_step.stp".into(), b"this is no STEP file\n".to_vec()));
    assert!(files.len() > 20);
    let mut failures = Vec::new();
    for (name, bytes) in &files {
        let sha = server.upload(bytes).await;
        let body = json!({ "upload": format!("sha256:{sha}") });
        let job = server.send(waiting("/v1/features", &body)).await.json();
        let dir = file_in(&server.dirs, name, bytes);
        same(
            &server,
            name,
            &job,
            cli(&dir, &["features", name]),
            &mut failures,
        )
        .await;
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// DC1, DC3, JB4, RA3: `fitment parts FILE --format json` and the job
/// `assembly_read` on every committed unit assembly and on bytes that are
/// none: the job's document and `…/parts` are the CLI's bytes.
#[tokio::test]
async fn parts_are_the_clis() {
    let server = TestServer::fresh();
    let mut failures = Vec::new();
    let mut inputs: Vec<(String, Vec<u8>)> = [
        "s01_bolted.stp",
        "s02_cbore.stp",
        "s03_dowels.stp",
        "s08x_nested.stp",
    ]
    .iter()
    .map(|n| ((*n).to_owned(), support::assembly(n)))
    .collect();
    inputs.push((
        "not_step.stp".into(),
        b"this is no STEP file either\n".to_vec(),
    ));
    for (name, bytes) in &inputs {
        let job = server.register(bytes, name).await;
        let dir = file_in(&server.dirs, name, bytes);
        let run = cli(&dir, &["parts", name, "--format", "json"]);
        let stdout = run.1.clone();
        same(&server, name, &job, run, &mut failures).await;
        let sha = fitment_core::sha256_hex(bytes);
        let parts = server
            .get(&format!("/v1/assemblies/sha256:{sha}/parts"))
            .await;
        assert_eq!(parts.status, 200, "{name}");
        if parts.body != stdout {
            failures.push(format!("{name}: …/parts is not the CLI's bytes"));
        }
        assert_eq!(
            parts.header("cache-control"),
            Some("no-cache"),
            "{name}: VN5"
        );
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// DC1, DC3, JB4, RA4: `fitment socket FILE --part P --format json` and
/// the job `socket_extract` for every target the CLI's socket goldens
/// name, a refused assembly's (exit 3, the refused document) and a
/// selection error (exit 2, `refused`, `part_not_found`, no document).
#[tokio::test]
async fn sockets_are_the_clis() {
    let server = TestServer::fresh();
    let golden = Path::new(env!("CARGO_MANIFEST_DIR")).join("../fitment/tests/golden/socket");
    let mut targets: Vec<(String, String)> = std::fs::read_dir(&golden)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter_map(|n| {
            let stem = n.strip_suffix(".json")?;
            let (case, part) = stem.split_once("--")?;
            Some((format!("{case}.stp"), part.to_owned()))
        })
        .collect();
    targets.sort();
    assert!(targets.len() >= 10);
    targets.push(("s01_bolted.stp".into(), "NOPE".into()));
    targets.push(("not_step.stp".into(), "BRACKET".into()));
    let mut failures = Vec::new();
    for (name, part) in &targets {
        let bytes = if name == "not_step.stp" {
            b"this is no STEP file at all\n".to_vec()
        } else {
            support::assembly(name)
        };
        let registered = server.register(&bytes, name).await;
        // A second registration of current bytes is the assembly, no job.
        assert!(
            registered["format"] == "fitment.assembly"
                || registered["state"] == "succeeded"
                || registered["state"] == "refused",
            "{registered}"
        );
        let sha = fitment_core::sha256_hex(&bytes);
        let job = server.extract(&sha, part).await;
        let dir = file_in(&server.dirs, name, &bytes);
        let what = format!("{name} --part {part}");
        same(
            &server,
            &what,
            &job,
            cli(&dir, &["socket", name, "--part", part, "--format", "json"]),
            &mut failures,
        )
        .await;
        if part == "NOPE" {
            assert_eq!(job["error"]["code"], "part_not_found", "{job}");
            assert!(job["document"].is_null());
            assert!(job["outcome"].is_null(), "XH12: no verdict");
        }
        if job["state"] == "succeeded" {
            let href = job["result"]["href"].as_str().unwrap();
            let revision = server.get(href).await;
            let document = server.document(&job).await;
            assert_eq!(
                revision.body, document.body,
                "{what}: the revision is the job's document"
            );
            assert!(
                revision
                    .header("fitment-digest")
                    .is_some_and(|d| d.starts_with("sha256:"))
            );
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// MF11, RA5, DC3: carrying decisions as `fitment socket FILE -o OUT`
/// carries OUT's: an extraction with `carry_from` an uploaded document
/// the CLI reviewed is the CLI's re-extraction onto that file, byte for
/// byte; extracting again carries from the line's current revision and is
/// `unchanged`; `carry: false` starts from the unreviewed document; a
/// document of another file is `refused`, exit 2, `carry_from_foreign`, as
/// the CLI refuses that OUT.
#[tokio::test]
async fn carrying_is_the_clis() {
    let server = TestServer::fresh();
    let name = "s02_cbore.stp";
    let bytes = support::assembly(name);
    let dir = file_in(&server.dirs, name, &bytes);
    let socket = |extra: &[&str]| {
        let mut args = vec!["socket", name, "--part", "BLOCK", "-o", "OUT.json"];
        args.extend_from_slice(extra);
        cli(&dir, &args)
    };
    assert_eq!(socket(&[]).0, Some(0));
    let review = cli(&dir, &["review", "OUT.json", "--keep", "bore:hole-427"]);
    assert_eq!(review.0, Some(0), "{}", review.2);
    let reviewed = std::fs::read(dir.join("OUT.json")).unwrap();
    assert_eq!(socket(&[]).0, Some(0));
    let carried = std::fs::read(dir.join("OUT.json")).unwrap();
    server.register(&bytes, name).await;
    let sha = fitment_core::sha256_hex(&bytes);
    let upload = server.upload(&reviewed).await;
    let body = json!({ "assembly": format!("sha256:{sha}"), "part": "BLOCK",
                       "carry_from": { "upload": format!("sha256:{upload}") } });
    let job = server.send(waiting("/v1/sockets", &body)).await.json();
    assert_eq!(job["state"], "succeeded", "{job}");
    let document = server.document(&job).await.body;
    assert!(fitment_corpus::parity::bytes("carried", &carried, &document).is_none());
    let line = server.get("/v1/sockets").await.json()["items"][0].clone();
    assert_eq!(line["current"]["carried"], 1);
    assert_eq!(line["current"]["unreviewed"], 3);
    let again = server.extract(&sha, "BLOCK").await;
    assert_eq!(
        again["outcome"], "unchanged",
        "carried from the current revision"
    );
    let fresh = json!({ "assembly": format!("sha256:{sha}"), "part": "BLOCK", "carry": false });
    let plain = server.send(waiting("/v1/sockets", &fresh)).await.json();
    assert!(plain["outcome"].is_null());
    assert_eq!(
        server.document(&plain).await.body,
        support::cli_golden("socket", "s02_cbore--BLOCK.json")
    );
    let other = support::cli_golden("socket", "s01_bolted--BRACKET.json");
    let foreign = server.upload(&other).await;
    let body = json!({ "assembly": format!("sha256:{sha}"), "part": "BLOCK",
                       "carry_from": { "upload": format!("sha256:{foreign}") } });
    let job = server.send(waiting("/v1/sockets", &body)).await.json();
    assert_eq!(
        (job["state"].as_str(), job["exit_code"].as_u64()),
        (Some("refused"), Some(2))
    );
    assert_eq!(job["error"]["code"], "carry_from_foreign");
}

/// The exit's file half for PR 5 (section 12, item 5's "done when"): every
/// assembly of the corpus registered through `assembly_read` gives `fitment
/// parts FILE --format json`'s bytes and exit code, and every socket its
/// cases name (the match cases' and the assemblies' own) extracted through
/// `socket_extract` gives `fitment socket FILE --part P --format json`'s.
/// Release only, as the corpus suites are.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_corpus_source_is_the_clis_through_jobs() {
    let Some(corpus) = fitment_corpus::corpus::Corpus::locate_release_only() else {
        return;
    };
    let dirs = Dirs::new();
    let mut config = dirs.config();
    config.auth.rate_per_second = 10_000;
    config.auth.rate_burst = 10_000;
    let server = TestServer::in_process(dirs, config);
    let mut sockets = std::collections::BTreeSet::new();
    let mut files = std::collections::BTreeMap::new();
    for case in corpus.cases() {
        let id = case["id"].as_str().unwrap().to_owned();
        if case["kind"] == "assembly" {
            files.insert(id.clone(), case["file"].as_str().unwrap().to_owned());
            if let Some(targets) = case["expect"]["sockets"].as_object() {
                for part in targets.keys() {
                    sockets.insert((id.clone(), part.clone()));
                }
            }
        }
        if case["kind"] == "match" {
            let s = &case["socket"];
            sockets.insert((
                s["case"].as_str().unwrap().to_owned(),
                s["part"].as_str().unwrap().to_owned(),
            ));
        }
    }
    // A match case's source may be an assembly no case of its own states
    // (a mirror's): its file is the case's name.
    for (source, _) in &sockets {
        files
            .entry(source.clone())
            .or_insert_with(|| format!("{source}.stp"));
    }
    let mut failures = Vec::new();
    for (id, file) in &files {
        let bytes = std::fs::read(corpus.dir().join(file)).unwrap();
        let job = server.register(&bytes, file).await;
        let run = cli(corpus.dir(), &["parts", file, "--format", "json"]);
        same(&server, file, &job, run, &mut failures).await;
        let _ = id;
    }
    for (source, part) in &sockets {
        let file = &files[source];
        let sha = fitment_core::sha256_hex(&std::fs::read(corpus.dir().join(file)).unwrap());
        let job = server.extract(&sha, part).await;
        let run = cli(
            corpus.dir(),
            &["socket", file, "--part", part, "--format", "json"],
        );
        same(
            &server,
            &format!("{file} --part {part}"),
            &job,
            run,
            &mut failures,
        )
        .await;
    }
    eprintln!(
        "PR 5's corpus parity: {} assemblies, {} sockets, {} differences",
        files.len(),
        sockets.len(),
        failures.len()
    );
    assert!(files.len() >= 200 && sockets.len() >= 33);
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
