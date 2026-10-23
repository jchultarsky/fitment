//! Scale and load through the API (docs/research/m6-api.md, VT27, VT28;
//! M6-45; PR 8), weekly in `scale.yml`, or by hand: a catalog of
//! `tools/scale-step.py`'s scaled copies (CT11's) ingested through the
//! built server, a child over loopback, then every corpus socket's full
//! run, alone and under load. Timings are reported; the gates are
//! order-of-magnitude ceilings, as CT11's is (M6-45), and the documents
//! are the CLI's on the same catalog.
//!
//! ```text
//! tools/scale-step.py corpus/out scaled 10000 --manifest corpus/out/manifest.json
//! FITMENT_SCALE_DIR=scaled cargo test --release -p fitment-server --test scale -- --nocapture
//! ```
//!
//! Without `FITMENT_SCALE_DIR` it passes with a note; it needs the corpus.
#![allow(clippy::disallowed_methods)] // reads FITMENT_SCALE_DIR; times the CLI

mod support;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use fitment_corpus::Corpus;
use serde_json::{Value, json};
use support::{Dirs, TestServer, cli, waiting};

/// VT28's clients and how long they run.
const CLIENTS: usize = 16;
const LOAD: Duration = Duration::from_secs(120);

/// The server's limits for one client submitting 10⁴ files and sixteen
/// at once: rates, queues, caps, waits and live uploads raised, and the
/// JSON cap for one ingest of every file.
const CONFIG: &str = "[auth]\nrate_per_second = 100000\nrate_burst = 100000\n\
    [limits]\nmax_candidates = 100000\nmax_json_kib = 16384\nupload_concurrency_per_user = 8\n\
    user_max_uploads = 100000\n\
    [jobs]\nper_owner_running = 64\nper_owner_queued = 1000\nqueue_query = 1000\n\
    streams_per_user = 64\n";

/// A corpus socket through the API: its name, its revision, its file.
struct Socket {
    name: String,
    revision: String,
    file: PathBuf,
}

/// The server's resident set now, in KiB (`ps`), for the peak's report.
fn resident_kib(pid: u32) -> u64 {
    let out = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output();
    out.ok()
        .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse().ok())
        .unwrap_or(0)
}

/// Samples the server's resident set every 100 ms until dropped: the peak.
fn watch_memory(
    pid: u32,
) -> (
    Arc<std::sync::atomic::AtomicU64>,
    tokio::task::JoinHandle<()>,
) {
    let peak = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let keeping = Arc::clone(&peak);
    let task = tokio::spawn(async move {
        loop {
            let now = tokio::task::spawn_blocking(move || resident_kib(pid))
                .await
                .unwrap_or(0);
            keeping.fetch_max(now, std::sync::atomic::Ordering::Relaxed);
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    });
    (peak, task)
}

/// VT27's ingest: the scaled files uploaded (eight at a time) and
/// ingested in one job, timed against the CLI's ingest of the same files
/// into a catalog of its own: at most twice its time.
async fn ingest(server: &Arc<TestServer>, scaled: &Path, scratch: &Path) -> usize {
    let mut names: Vec<String> = std::fs::read_dir(scaled)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| {
            Path::new(n)
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("stp"))
        })
        .collect();
    names.sort();
    let started = Instant::now();
    let mut files = Vec::with_capacity(names.len());
    for chunk in names.chunks(8) {
        let uploads: Vec<_> = chunk
            .iter()
            .map(|name| {
                let (server, path) = (Arc::clone(server), scaled.join(name));
                tokio::spawn(async move { server.upload(&std::fs::read(path).unwrap()).await })
            })
            .collect();
        for (name, upload) in chunk.iter().zip(uploads) {
            let sha = upload.await.unwrap();
            files.push(json!({ "upload": format!("sha256:{sha}"), "file_name": name }));
        }
    }
    let uploaded = started.elapsed();
    let started = Instant::now();
    let job = server
        .send(waiting("/v1/catalog/entries", &json!({ "files": files })))
        .await
        .json();
    let job = server
        .finished_job(job["id"].as_str().unwrap(), Duration::from_secs(3600))
        .await;
    let api = started.elapsed();
    // A scaled copy the reader refuses is quarantined: the CLI exits 3,
    // and the job is `refused` with its document, as the CLI's is.
    assert!(
        job["state"] == "succeeded" || job["state"] == "refused",
        "{}",
        job["error"]
    );
    let cli_catalog = scratch.join("cli-catalog");
    let shown = cli_catalog.display().to_string();
    assert_eq!(cli(scratch, &["catalog", "init", "-C", &shown]).0, Some(0));
    let started = Instant::now();
    let (code, _, stderr) = cli(
        scratch,
        &[
            "catalog",
            "ingest",
            "-C",
            &shown,
            &scaled.display().to_string(),
        ],
    );
    let alone = started.elapsed();
    assert_eq!(job["exit_code"].as_i64(), code.map(i64::from), "{stderr}");
    eprintln!(
        "VT27 ingest: {} files uploaded in {uploaded:.2?}, ingested by one job in {api:.2?}; \
         the CLI's ingest in {alone:.2?}",
        names.len()
    );
    assert!(
        api <= alone * 2,
        "the API's ingest {api:?}, over twice the CLI's {alone:?}"
    );
    names.len()
}

/// The corpus's match sockets extracted through the API from their
/// sources (part only, as CT11 extracts them); a refused one is left out.
async fn sockets(server: &TestServer, corpus: &Corpus, scratch: &Path) -> Vec<Socket> {
    let mut out = Vec::new();
    let keys: std::collections::BTreeSet<(String, String)> = corpus
        .socket_groups()
        .into_keys()
        .map(|k| (k.source, k.part))
        .collect();
    for (case, part) in keys {
        let bytes = std::fs::read(corpus.dir().join(format!("{case}.stp"))).unwrap();
        server.register(&bytes, &format!("{case}.stp")).await;
        let job = server
            .extract(&fitment_core::sha256_hex(&bytes), &part)
            .await;
        if job["state"] != "succeeded" || job["exit_code"] != 0 {
            continue;
        }
        let revision = job["result"]["href"]
            .as_str()
            .unwrap()
            .trim_start_matches("/v1/sockets/");
        let file = scratch.join(format!("{case}--{part}.json"));
        std::fs::write(&file, server.document(&job).await.body).unwrap();
        out.push(Socket {
            name: format!("{case} {part}"),
            revision: revision.to_owned(),
            file,
        });
    }
    out
}

/// A full catalog run of `socket` through the API, waited for: its job.
async fn full_run(server: &TestServer, socket: &str, selection: &str) -> Value {
    let body = json!({ "socket": socket, "candidates": { "catalog": { "selection": selection } } });
    let mut reply = server.send(waiting("/v1/matches", &body)).await;
    // A declared load refusal (VT14's) is retried; none is expected.
    for attempt in 1..=100_u64 {
        if reply.status != 429 && reply.status != 503 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10 * attempt.min(20))).await;
        reply = server.send(waiting("/v1/matches", &body)).await;
    }
    let job = reply.json();
    let id = job["id"].as_str().unwrap_or_else(|| panic!("{job}"));
    server.finished_job(id, Duration::from_secs(3600)).await
}

/// VT27's runs: every socket's full run through the API, its document the
/// CLI's `fitment match S --catalog` on the server's catalog, byte for
/// byte (so digest for digest), its latency at most twice the CLI's and a
/// second. The slowest single-client latency, and the socket whose
/// document is the largest.
async fn runs(server: &TestServer, sockets: &[Socket], scratch: &Path) -> (Duration, String) {
    let catalog = server.dirs.catalog.display().to_string();
    let (mut slowest, mut largest) = (Duration::ZERO, (0, String::new()));
    for socket in sockets {
        let started = Instant::now();
        let job = full_run(server, &socket.revision, "full").await;
        let api = started.elapsed();
        let shown = socket.file.display().to_string();
        let started = Instant::now();
        let (code, stdout, _) = cli(
            scratch,
            &["match", &shown, "--catalog", &catalog, "--format", "json"],
        );
        let alone = started.elapsed();
        let document = if job["document"].is_null() {
            Vec::new()
        } else {
            server.document(&job).await.body
        };
        assert_eq!(document, stdout, "{}: not the CLI's document", socket.name);
        assert_eq!(
            job["exit_code"].as_i64(),
            code.map(i64::from),
            "{}",
            socket.name
        );
        eprintln!(
            "VT27 {:<32} {:>9} bytes; the API in {api:>9.2?}, the CLI in {alone:>9.2?}",
            socket.name,
            document.len()
        );
        assert!(
            api <= alone * 2 + Duration::from_secs(1),
            "{}: {api:?} against the CLI's {alone:?}",
            socket.name
        );
        slowest = slowest.max(api);
        if document.len() > largest.0 {
            largest = (document.len(), socket.revision.clone());
        }
    }
    (slowest, largest.1)
}

/// VT28: `CLIENTS` clients, every socket's full run in a loop for `LOAD`:
/// throughput and the 50th and 99th percentiles reported; no request over
/// ten times the slowest single-client run (M6-45's ceiling).
async fn load(server: &Arc<TestServer>, sockets: &[Socket], alone: Duration) {
    let revisions: Arc<Vec<String>> =
        Arc::new(sockets.iter().map(|s| s.revision.clone()).collect());
    let until = Instant::now() + LOAD;
    let mut clients = Vec::new();
    for n in 0..CLIENTS {
        let (server, revisions) = (Arc::clone(server), Arc::clone(&revisions));
        clients.push(tokio::spawn(async move {
            let mut latencies = Vec::new();
            let mut k = n;
            while Instant::now() < until {
                let started = Instant::now();
                let job = full_run(&server, &revisions[k % revisions.len()], "full").await;
                assert!(
                    job["state"] == "succeeded" || job["state"] == "refused",
                    "{job}"
                );
                latencies.push(started.elapsed());
                k += 1;
            }
            latencies
        }));
    }
    let mut latencies = Vec::new();
    for client in clients {
        latencies.extend(client.await.unwrap());
    }
    latencies.sort();
    // The q-th quantile, q in hundredths: nearest rank.
    let at = |q: usize| latencies[(latencies.len() - 1) * q / 100];
    eprintln!(
        "VT28 load: {CLIENTS} clients for {LOAD:?}: {} runs, {:.2} a second; p50 {:.2?}, p99 {:.2?}, \
         slowest {:.2?}; one client's slowest {alone:.2?}",
        latencies.len(),
        f64::from(u32::try_from(latencies.len()).unwrap_or(u32::MAX)) / LOAD.as_secs_f64(),
        at(50),
        at(99),
        latencies[latencies.len() - 1]
    );
    assert!(
        latencies[latencies.len() - 1] <= alone * 10,
        "a run over ten times the slowest single-client run"
    );
}

/// VT27 and VT28 (M6-45): the 10⁴ scaled catalog through the API.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_scaled_catalog_through_the_api() {
    let Some(scaled) = std::env::var_os("FITMENT_SCALE_DIR").map(PathBuf::from) else {
        eprintln!("skipped: set FITMENT_SCALE_DIR (tools/scale-step.py)");
        return;
    };
    let corpus = Corpus::at(&Corpus::default_dir(), true).expect("a required corpus");
    let server = Arc::new(TestServer::process(Dirs::new(), CONFIG));
    let pid = server.child.as_ref().unwrap().child.id();
    let (peak, watching) = watch_memory(pid);
    let scratch = server.dirs.root.join("scale");
    std::fs::create_dir_all(&scratch).unwrap();
    let parts = ingest(&server, &scaled, &scratch).await;
    let sockets = sockets(&server, &corpus, &scratch).await;
    assert!(sockets.len() > 15, "{} sockets", sockets.len());
    let (alone, largest) = runs(&server, &sockets, &scratch).await;
    // The largest document: the widest socket's exhaustive run, read whole.
    let started = Instant::now();
    let job = full_run(&server, &largest, "exhaustive").await;
    let document = server.document(&job).await.body;
    eprintln!(
        "VT27 {parts} parts: {largest}'s exhaustive document, {} bytes, run and downloaded in \
         {:.2?}; the server's peak resident set so far {} MiB",
        document.len(),
        started.elapsed(),
        peak.load(std::sync::atomic::Ordering::Relaxed) / 1024
    );
    load(&server, &sockets, alone).await;
    watching.abort();
    eprintln!(
        "the server's peak resident set: {} MiB",
        peak.load(std::sync::atomic::Ordering::Relaxed) / 1024
    );
}
