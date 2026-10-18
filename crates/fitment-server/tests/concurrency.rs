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
