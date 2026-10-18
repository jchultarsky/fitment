//! Concurrency (docs/research/m6-api.md, section 10.3): one server per
//! state directory (SD6, VT16's first half).
#![allow(clippy::disallowed_methods)] // a thread holds server.db's lock

mod support;

use std::time::Duration;

use fitment_server::Server;
use support::{Call, Dirs, Process, TestServer};

fn lease(dirs: &Dirs) -> rusqlite::Connection {
    rusqlite::Connection::open(dirs.state.join("server.db")).unwrap()
}

/// SD6, VT16: a second `fitment-server` on the same state directory
/// refuses to start (exit 2), naming neither host nor path, while the
/// first holds the lease; once the first stops, the second starts.
#[test]
fn a_second_server_on_one_state_is_refused() {
    let dirs = Dirs::new();
    let first = Process::start(&dirs, "", &[]).expect("the first starts");
    let second = Process::start(&dirs, "", &[]);
    let Err(failed) = second else {
        panic!("a second server started on the same state directory");
    };
    assert_eq!(failed.code, Some(2), "{}", failed.stderr);
    let refusal = failed
        .stderr
        .lines()
        .find(|l| l.starts_with("fitment-server: "))
        .unwrap_or_default()
        .to_owned();
    assert!(refusal.contains("holds this state directory"), "{refusal}");
    assert!(
        !refusal.contains(&dirs.root.display().to_string()),
        "{refusal}"
    );
    assert!(!refusal.contains("127.0.0.1"), "{refusal}");
    let (code, _, _) = first.stop();
    assert_eq!(code, Some(0));
    let again = Process::start(&dirs, "", &[]).expect("the second starts once the first stopped");
    assert_eq!(again.stop().0, Some(0));
}

/// SD6: a lease renewed within 30 s by another holder refuses the start;
/// a staler one is taken over (its holder died without releasing it).
#[test]
fn a_stale_lease_is_taken_over() {
    let dirs = Dirs::new();
    drop(Server::build(dirs.config()).unwrap());
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis();
    let now = i64::try_from(now).unwrap();
    let db = lease(&dirs);
    let plant =
        "INSERT OR REPLACE INTO lease (one, holder, heartbeat_ms) VALUES (1, 'l_other', ?1)";
    db.execute(plant, [now - 5_000]).unwrap();
    assert!(
        Server::build(dirs.config()).is_err(),
        "a fresh lease was taken"
    );
    db.execute(plant, [now - 31_000]).unwrap();
    let server = Server::build(dirs.config()).expect("a stale lease is taken over");
    let holder: String = db
        .query_row("SELECT holder FROM lease", [], |r| r.get(0))
        .unwrap();
    assert_ne!(holder, "l_other");
    drop(server);
    let held: i64 = db
        .query_row("SELECT count(*) FROM lease", [], |r| r.get(0))
        .unwrap();
    assert_eq!(held, 0, "a dropped server gives its lease up");
}

/// SD6, OP7: a server whose lease another holder has taken is no longer
/// ready (`503 shutting_down`, `reason: lease`).
#[tokio::test]
async fn a_lost_lease_turns_readiness_off() {
    let server = TestServer::fresh();
    assert_eq!(server.get("/v1/ready").await.status, 204);
    lease(&server.dirs)
        .execute("UPDATE lease SET holder = 'l_other'", [])
        .unwrap();
    let reply = server.send(Call::get("/v1/ready")).await;
    assert_eq!(reply.code(), "shutting_down");
    assert_eq!(reply.json()["details"]["reason"], "lease");
}

/// SD6, review S7: losing the lease to another holder stops the server:
/// readiness turns off and `serve` ends with an error, so the process
/// exits non-zero.
#[tokio::test(flavor = "multi_thread")]
async fn a_lost_lease_stops_the_server() {
    let dirs = Dirs::new();
    let config = dirs.config();
    let mut server = TestServer::bound_with(dirs, config, |s| {
        s.set_lease_renewal(std::time::Duration::from_millis(100));
    })
    .await;
    assert_eq!(server.get("/v1/ready").await.status, 204);
    lease(&server.dirs)
        .execute("UPDATE lease SET holder = 'l_other'", [])
        .unwrap();
    let ended = server.finished(std::time::Duration::from_secs(10)).await;
    let error = ended.expect("the server stopped").unwrap_err();
    assert!(error.to_string().contains("lease"), "{error}");
}

/// Review S7: a renewal that meets a busy database is retried, not taken
/// for a loss: with `server.db` locked for longer than a renewal's busy
/// timeout, the server keeps serving and its lease.
#[tokio::test(flavor = "multi_thread")]
async fn a_busy_renewal_is_retried() {
    let dirs = Dirs::new();
    let config = dirs.config();
    let mut server = TestServer::bound_with(dirs, config, |s| {
        s.set_lease_renewal(std::time::Duration::from_millis(100));
    })
    .await;
    let path = server.dirs.state.join("server.db");
    let holder = std::thread::spawn(move || {
        let conn = rusqlite::Connection::open(path).unwrap();
        conn.execute_batch("BEGIN IMMEDIATE").unwrap();
        std::thread::sleep(std::time::Duration::from_secs(6));
        conn.execute_batch("COMMIT").unwrap();
    });
    tokio::time::sleep(std::time::Duration::from_secs(7)).await;
    holder.join().unwrap();
    assert!(
        server
            .finished(std::time::Duration::from_millis(500))
            .await
            .is_none()
    );
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    assert_eq!(server.get("/v1/ready").await.status, 204);
    server.stop().await;
}

/// The targets of the CLI's socket goldens, `(assembly file, part)`.
fn socket_targets() -> Vec<(String, String)> {
    let golden =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../fitment/tests/golden/socket");
    let mut out: Vec<(String, String)> = std::fs::read_dir(golden)
        .unwrap()
        .filter_map(|e| {
            let name = e.unwrap().file_name().to_string_lossy().into_owned();
            let (case, part) = name.strip_suffix(".json")?.split_once("--")?;
            Some((format!("{case}.stp"), part.to_owned()))
        })
        .collect();
    out.sort();
    out
}

/// Sends `call`, retrying the declared load refusals (`429` and `503`
/// with their codes) with backoff, as VT14's harness does, and counting
/// them.
async fn patient(
    server: &TestServer,
    call: Call,
    refusals: &std::sync::atomic::AtomicUsize,
) -> support::Reply {
    for attempt in 0..200_u64 {
        let reply = server.send(call.clone()).await;
        if reply.status != 429 && reply.status != 503 {
            return reply;
        }
        assert!(
            [
                "too_many_streams",
                "too_many_jobs",
                "queue_full",
                "rate_limited"
            ]
            .contains(&reply.code().as_str()),
            "an undeclared refusal: {}",
            reply.code()
        );
        refusals.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        tokio::time::sleep(Duration::from_millis(10 * (attempt + 1).min(20))).await;
    }
    panic!("refused for load 200 times");
}

/// One gauge of the exposition.
fn gauge(text: &str, name: &str) -> i64 {
    text.lines()
        .find_map(|l| l.strip_prefix(name)?.trim().parse().ok())
        .unwrap_or(-1)
}

/// VT14, JS1 to JS4: 32 clients over the bound socket, each its own user,
/// each registering every committed assembly and extracting every socket
/// the CLI's goldens name, interleaved: every document is the CLI's bytes
/// (the sequential answer), a refusal for load is only a declared one
/// (retried with backoff and counted), and while they run the query lane
/// never runs more than its slots and the CPU permits never go below
/// zero (the per-owner cap and the turns are `jobs::scheduler`'s unit
/// tests).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn many_clients_extract_the_same_sockets() {
    use fitment_server::auth::Role;
    let dirs = Dirs::new();
    let mut config = dirs.config();
    config.jobs.cpu = 8;
    let slots = i64::from(config.jobs.query_slots());
    let server = std::sync::Arc::new(TestServer::bound(dirs, config).await);
    let targets = socket_targets();
    let mut tokens = Vec::new();
    for n in 0..32 {
        let user = support::seed_user(server.server(), &format!("client{n}"), Role::Reader, None);
        tokens.push(support::seed_token(server.server(), &user, Role::Reader, 30).1);
    }
    let running = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let watching = (
        std::sync::Arc::clone(&server),
        std::sync::Arc::clone(&running),
    );
    let watcher = tokio::spawn(async move {
        let mut most = 0;
        while watching.1.load(std::sync::atomic::Ordering::SeqCst) {
            let reading = watching.0.server().clone();
            let text = tokio::task::spawn_blocking(move || reading.metrics_text())
                .await
                .unwrap();
            let now = gauge(&text, "fitment_jobs_running{lane=\"query\"}");
            assert!(now <= slots, "{now} running past {slots} slots");
            assert!(gauge(&text, "fitment_cpu_permits_free") >= 0);
            most = most.max(now);
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        most
    });
    let refusals = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut clients = Vec::new();
    for token in tokens {
        let (server, targets) = (std::sync::Arc::clone(&server), targets.clone());
        let refusals = std::sync::Arc::clone(&refusals);
        clients.push(tokio::spawn(async move {
            let mut documents = Vec::new();
            let mut files: Vec<&String> = targets.iter().map(|(f, _)| f).collect();
            files.dedup();
            for file in files {
                let bytes = support::assembly(file);
                let put = support::put_upload(&bytes).bearer(&token);
                assert!(patient(&server, put, &refusals).await.status.is_success());
                let sha = fitment_core::sha256_hex(&bytes);
                let body =
                    serde_json::json!({ "upload": format!("sha256:{sha}"), "file_name": file });
                let job = patient(
                    &server,
                    support::waiting("/v1/assemblies", &body).bearer(&token),
                    &refusals,
                )
                .await;
                assert_eq!(job.json()["state"], "succeeded");
            }
            for (file, part) in &targets {
                let sha = fitment_core::sha256_hex(&support::assembly(file));
                let body = serde_json::json!({ "assembly": format!("sha256:{sha}"), "part": part });
                let job = patient(
                    &server,
                    support::waiting("/v1/sockets", &body).bearer(&token),
                    &refusals,
                )
                .await
                .json();
                assert_eq!(job["state"], "succeeded", "{job}");
                let id = job["id"].as_str().unwrap();
                let document = server
                    .send(Call::get(&format!("/v1/jobs/{id}/document")).bearer(&token))
                    .await;
                documents.push((
                    format!("{}--{part}.json", file.trim_end_matches(".stp")),
                    document.body,
                ));
            }
            documents
        }));
    }
    for client in clients {
        for (name, body) in client.await.unwrap() {
            assert_eq!(body, support::cli_golden("socket", &name), "{name}");
        }
    }
    running.store(false, std::sync::atomic::Ordering::SeqCst);
    let most = watcher.await.unwrap();
    eprintln!(
        "VT14: at most {most} of {slots} query slots ran at once; {} refusals for load, retried",
        refusals.load(std::sync::atomic::Ordering::Relaxed)
    );
}

// ------------------------------------------------------------- catalog

/// The committed parts and candidates, by file name: VT15's pool.
fn pool() -> Vec<(Vec<u8>, String)> {
    let core = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../fitment-core/tests");
    let mut out: Vec<(Vec<u8>, String)> = ["data", "candidates"]
        .iter()
        .flat_map(|d| std::fs::read_dir(core.join(d)).unwrap())
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "stp"))
        .map(|p| {
            let name = p.file_name().unwrap().to_string_lossy().into_owned();
            (std::fs::read(&p).unwrap(), name)
        })
        .collect();
    out.sort_by(|a, b| a.1.cmp(&b.1));
    out
}

/// `bytes`, a STEP file's header naming another file: other bytes, the
/// same part (a replace's new revision).
fn renamed(bytes: &[u8]) -> Vec<u8> {
    String::from_utf8_lossy(bytes)
        .replacen("FILE_NAME('", "FILE_NAME('v2", 1)
        .into_bytes()
}

/// VT15's oracle: the catalog's `op` rows replayed to `generation`: each
/// live entry's content, by entry.
fn live_at(
    ops: &[(i64, String, i64, Option<String>)],
    generation: i64,
) -> std::collections::BTreeMap<i64, String> {
    let mut live = std::collections::BTreeMap::new();
    for (g, kind, entry, sha) in ops.iter().filter(|o| o.0 <= generation) {
        let _ = g;
        match (kind.as_str(), sha) {
            ("ingest" | "replace", Some(sha)) => {
                live.insert(*entry, sha.clone());
            }
            ("delete", _) => {
                live.remove(entry);
            }
            _ => {}
        }
    }
    live
}

/// The catalog's `op` rows that change what is live, in order.
fn ops(catalog: &std::path::Path) -> Vec<(i64, String, i64, Option<String>)> {
    let conn = rusqlite::Connection::open(catalog.join("catalog.db")).unwrap();
    conn.prepare(
        "SELECT generation, kind, entry, sha256 FROM op
         WHERE entry IS NOT NULL ORDER BY generation, n",
    )
    .unwrap()
    .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
    .unwrap()
    .collect::<Result<_, _>>()
    .unwrap()
}

/// VT15's writes: `rest` ingested four at a time, then of `all`'s first
/// six, three replaced by their renamed bytes and three deleted, then a
/// reindex of every entry.
async fn writes(
    writing: std::sync::Arc<TestServer>,
    rest: Vec<(Vec<u8>, String)>,
    all: Vec<(Vec<u8>, String)>,
) {
    for chunk in rest.chunks(4) {
        writing.ingest(chunk).await;
    }
    for (n, (bytes, name)) in all.iter().take(6).enumerate() {
        let stem = name.trim_end_matches(".stp");
        if n % 2 == 0 {
            let sha = writing.upload(&renamed(bytes)).await;
            let body = serde_json::json!({ "upload": format!("sha256:{sha}"), "file_name": name });
            let call = support::Call::json(
                axum::http::Method::PUT,
                &format!("/v1/catalog/entries/{stem}"),
                &body,
            )
            .header("prefer", "wait=30");
            writing.send(call).await;
        } else {
            let call = support::Call::new(
                axum::http::Method::DELETE,
                &format!("/v1/catalog/entries/{stem}"),
            );
            let _ = writing.send(call).await;
        }
    }
    let reindex = writing
        .submit("/v1/catalog/reindex", &serde_json::json!({ "all": true }))
        .await;
    writing
        .finished_job(reindex["id"].as_str().unwrap(), Duration::from_secs(120))
        .await;
}

/// VT15 (CX2, CX5, CR7, CT14 through the API): while ingests, replaces,
/// deletes and a reindex run over a pool of the committed files, catalog
/// match jobs run in a loop. Each succeeded exhaustive run's document has
/// the digest of the file run over exactly the files live at the
/// generation its `fitment.run` records, the files replayed from the
/// catalog's `op` rows (CT2): no document mixes two generations. A
/// replace, then a named run, shows the new revision (nothing cached,
/// GT9).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn writes_during_matches_are_the_replayed_catalog() {
    use fitment_core::{MatchDocument, MatchOptions, MatchPolicy, Matcher, NoFitVerifier};
    let dirs = Dirs::new();
    let config = dirs.config();
    let server = std::sync::Arc::new(TestServer::bound(dirs, config).await);
    let files = pool();
    let (first, rest) = files.split_at(files.len() / 2);
    server.ingest(first).await;
    let socket = server.socket_of("s02_cbore", "BLOCK").await;
    let writer = tokio::spawn(writes(
        std::sync::Arc::clone(&server),
        rest.to_vec(),
        files.clone(),
    ));
    let exhaustive = serde_json::json!({ "socket": socket, "candidates": { "catalog": { "selection": "exhaustive" } } });
    let mut runs = Vec::new();
    while !writer.is_finished() || runs.len() < 3 {
        let job = server.run_match(&exhaustive).await;
        assert_eq!(job["state"], "succeeded", "{job}");
        let run = server
            .get(&format!("/v1/matches/{}", job["id"].as_str().unwrap()))
            .await
            .json();
        runs.push((
            run["candidates"]["generation"].as_i64().unwrap(),
            run["document"]["digest"].clone(),
        ));
    }
    writer.await.unwrap();
    let ops = ops(&server.dirs.catalog);
    let bytes: std::collections::BTreeMap<String, Vec<u8>> = files
        .iter()
        .flat_map(|(b, _)| [b.clone(), renamed(b)])
        .map(|b| (fitment_core::sha256_hex(&b), b))
        .collect();
    let document = server.get(&format!("/v1/sockets/{socket}")).await.body;
    let document =
        fitment_core::SocketDocument::from_json(&String::from_utf8(document).unwrap()).unwrap();
    let matcher = Matcher::new(
        &document,
        MatchPolicy::default(),
        MatchOptions::default(),
        &NoFitVerifier,
    )
    .unwrap();
    let mut judged = std::collections::BTreeMap::new();
    let generations: std::collections::BTreeSet<i64> = runs.iter().map(|r| r.0).collect();
    assert!(
        generations.len() >= 2,
        "matches ran at {generations:?} only"
    );
    for (generation, digest) in &runs {
        let live = live_at(&ops, *generation);
        let results = live
            .values()
            .map(|sha| {
                let result = judged
                    .entry(sha.clone())
                    .or_insert_with(|| matcher.judge_step(&bytes[sha]))
                    .clone();
                (result, vec![format!("catalog:{sha}")])
            })
            .collect();
        let expected = MatchDocument::new(&matcher, results);
        assert_eq!(
            digest.as_str(),
            Some(expected.digest()),
            "generation {generation}"
        );
    }
    let stem = files[0].1.trim_end_matches(".stp");
    let named = server
        .run_match(&serde_json::json!({ "socket": socket, "candidates": { "catalog": { "ids": [stem] } } }))
        .await;
    let doc: serde_json::Value =
        serde_json::from_slice(&server.document(&named).await.body).unwrap();
    assert_eq!(
        doc["candidates"][0]["labels"][0],
        format!("catalog:{stem}@r2"),
        "GT9"
    );
    assert_eq!(support::verified(&server.dirs.catalog), Some(0));
}

/// VT16, CX3, OP10: a `fitment catalog ingest` process beside the running
/// server, on its catalog, while the server ingests other files: both
/// sides' writes stand (IN1 to IN9: `verify --deep` clean, through the
/// API and the CLI), and the server sees the CLI's at its next snapshot.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cli_ingest_beside_the_server_is_consistent() {
    let dirs = Dirs::new();
    let config = dirs.config();
    let server = TestServer::bound(dirs, config).await;
    let files = pool();
    let (mine, theirs) = files.split_at(files.len() / 2);
    let dir = server.dirs.root.join("cli-in");
    std::fs::create_dir_all(&dir).unwrap();
    for (bytes, name) in theirs {
        std::fs::write(dir.join(name), bytes).unwrap();
    }
    let catalog = server.dirs.catalog.display().to_string();
    let cli_dir = dir.display().to_string();
    let cli = std::thread::spawn(move || {
        support::cli(
            std::path::Path::new("."),
            &["catalog", "ingest", "-C", &catalog, &cli_dir],
        )
    });
    let job = server.ingest(mine).await;
    assert!(matches!(job["exit_code"].as_i64(), Some(0 | 3)), "{job}");
    let (code, _, stderr) = cli.join().unwrap();
    assert!(matches!(code, Some(0 | 3)), "{stderr}");
    let listed = server.get("/v1/catalog/entries?limit=all").await.json();
    assert_eq!(listed["counts"]["entries"], files.len());
    let verify = server
        .submit("/v1/catalog/verify", &serde_json::json!({ "deep": true }))
        .await;
    let verify = server
        .finished_job(verify["id"].as_str().unwrap(), Duration::from_secs(300))
        .await;
    assert_eq!(
        (verify["state"].as_str(), verify["exit_code"].as_i64()),
        (Some("succeeded"), Some(0)),
        "{verify}"
    );
    assert_eq!(support::verified(&server.dirs.catalog), Some(0));
}

/// VT14 on catalog matches (JS1 to JS3, CX1, QR8): eight clients, each its
/// own user with its own socket of the same bytes, running full and
/// exhaustive catalog matches at once over the bound socket: every
/// document is the same bytes, whatever the permits each ran with.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn many_clients_run_catalog_matches_alike() {
    use fitment_server::auth::Role;
    let dirs = Dirs::new();
    let mut config = dirs.config();
    config.jobs.cpu = 4;
    let server = std::sync::Arc::new(TestServer::bound(dirs, config).await);
    server.ingest(&pool()).await;
    let refusals = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut clients = Vec::new();
    for n in 0..8 {
        let user = support::seed_user(server.server(), &format!("client{n}"), Role::Writer, None);
        let token = support::seed_token(server.server(), &user, Role::Writer, 30).1;
        let (server, refusals) = (
            std::sync::Arc::clone(&server),
            std::sync::Arc::clone(&refusals),
        );
        clients.push(tokio::spawn(async move {
            let bytes = support::assembly("s02_cbore.stp");
            let put = support::put_upload(&bytes).bearer(&token);
            assert!(patient(&server, put, &refusals).await.status.is_success());
            let sha = fitment_core::sha256_hex(&bytes);
            let body = serde_json::json!({ "upload": format!("sha256:{sha}") });
            patient(&server, support::waiting("/v1/assemblies", &body).bearer(&token), &refusals).await;
            let body = serde_json::json!({ "assembly": format!("sha256:{sha}"), "part": "BLOCK" });
            let job = patient(&server, support::waiting("/v1/sockets", &body).bearer(&token), &refusals)
                .await
                .json();
            let socket = job["result"]["href"].as_str().unwrap().trim_start_matches("/v1/sockets/").to_owned();
            let mut documents = Vec::new();
            for selection in ["full", "exhaustive"] {
                let body = serde_json::json!({ "socket": socket, "candidates": { "catalog": { "selection": selection } } });
                let job = patient(&server, support::waiting("/v1/matches", &body).bearer(&token), &refusals)
                    .await
                    .json();
                let id = job["id"].as_str().unwrap().to_owned();
                let job = server.finished_job(&id, Duration::from_secs(300)).await;
                assert_eq!(job["state"], "succeeded", "{job}");
                let document = server
                    .send(Call::get(&format!("/v1/matches/{id}/document")).bearer(&token))
                    .await;
                documents.push(document.body);
            }
            documents
        }));
    }
    let mut first: Option<Vec<Vec<u8>>> = None;
    for client in clients {
        let documents = client.await.unwrap();
        match &first {
            Some(first) => assert_eq!(first, &documents, "the same bytes"),
            None => first = Some(documents),
        }
    }
}

/// JS1, CX6 (VT45): `verify --deep` is the maintenance lane's, a reader
/// under its deadline: it runs, and ends, beside an ingest the write lane
/// runs, taking no barrier.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_deep_verify_runs_beside_a_running_ingest() {
    use fitment_server::hooks::{Action, Gate};
    let server = TestServer::fresh();
    server.ingest(&pool()[..4]).await;
    let seams = server.server().seams();
    let gate = Gate::new();
    seams
        .hooks
        .set("catalog.batch", Action::Pause(gate.clone()));
    let files: Vec<serde_json::Value> = {
        let mut out = Vec::new();
        for (bytes, name) in support::junk_files(3) {
            let sha = server.upload(&bytes).await;
            out.push(serde_json::json!({ "upload": format!("sha256:{sha}"), "file_name": name }));
        }
        out
    };
    let ingest = server
        .submit(
            "/v1/catalog/entries",
            &serde_json::json!({ "files": files }),
        )
        .await;
    assert!(gate.arrived(1, Duration::from_secs(60)).await);
    let verify = server
        .submit("/v1/catalog/verify", &serde_json::json!({ "deep": true }))
        .await;
    let verify = server
        .finished_job(verify["id"].as_str().unwrap(), Duration::from_secs(120))
        .await;
    assert_eq!(
        verify["state"], "succeeded",
        "beside a running ingest: {verify}"
    );
    assert!(!seams.catalog.barrier());
    gate.open();
    seams.hooks.clear("catalog.batch");
    let ingest = server
        .finished_job(ingest["id"].as_str().unwrap(), Duration::from_secs(60))
        .await;
    assert_eq!(ingest["exit_code"], 3, "{ingest}");
}
