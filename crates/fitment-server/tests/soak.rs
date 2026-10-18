//! The weekly soak and the full disk (docs/research/m6-api.md, VT10, VT11;
//! PR 8), run by `.github/workflows/scale.yml` on Linux, each only when its
//! variable is set:
//!
//! - `FITMENT_SOAK_ROUNDS=N`: the built server killed (SIGKILL) at a
//!   pseudo-random time in each of N rounds while jobs of every pure kind
//!   and a catalog ingest run, then started again on the same directories:
//!   XH12 after each round.
//! - `FITMENT_TMPFS=DIR`: a small real file system (CI mounts a 32 MiB
//!   tmpfs) as the state directory, filled by uploads until refused; the
//!   server stays up, and once space is freed an ingest and a match
//!   succeed and the catalog verifies clean.
#![cfg(unix)]
#![allow(clippy::disallowed_methods)] // reads FITMENT_SOAK_ROUNDS and FITMENT_TMPFS

mod support;

use std::collections::BTreeMap;
use std::time::Duration;

use axum::http::Method;
use serde_json::{Value, json};
use support::{Call, Dirs, TestServer};

/// A restart takes the lease at once (SD6's wait shortened, OP11).
const RESTART: &str = "[auth]\nrate_per_second = 100000\nrate_burst = 100000\n\
                       [test]\nlease_stale_ms = 1\n";

/// What a soak job is, for its document's check.
#[derive(Clone, Copy, Debug)]
enum Kind {
    Features,
    Socket,
    Parts,
    Match,
    Ingest,
}

/// The inputs a round's jobs name, uploaded once: `sha256:` references;
/// `assembly` registered before the rounds, `other` registered by the
/// first round's reading (later ones answer the assembly, no job).
struct Inputs {
    part: String,
    assembly: String,
    other: String,
    socket: String,
    candidates: Value,
}

/// XH12's second invariant: a succeeded job's document is the CLI's.
fn golden(kind: Kind) -> Option<Vec<u8>> {
    match kind {
        Kind::Features => Some(support::cli_golden("features", "p01_through.json")),
        Kind::Socket => Some(support::cli_golden("socket", "s01_bolted--BRACKET.json")),
        Kind::Parts => Some(support::cli_golden("parts", "s02_cbore.json")),
        Kind::Match | Kind::Ingest => None,
    }
}

/// Submits one job of `kind` without waiting: its id, or `None` when the
/// request's answer was lost to the kill.
async fn submit(server: &TestServer, kind: Kind, inputs: &Inputs, round: u64) -> Option<String> {
    let (path, body) = match kind {
        Kind::Features => ("/v1/features", json!({ "upload": inputs.part })),
        Kind::Parts => ("/v1/assemblies", json!({ "upload": inputs.other })),
        Kind::Socket => (
            "/v1/sockets",
            json!({ "assembly": inputs.assembly, "part": "BRACKET", "carry": false }),
        ),
        Kind::Match => (
            "/v1/matches",
            json!({ "socket": { "upload": inputs.socket }, "candidates": inputs.candidates }),
        ),
        Kind::Ingest => (
            "/v1/catalog/entries",
            json!({ "files": [{ "upload": inputs.part, "file_name": format!("soak{round}.stp") }] }),
        ),
    };
    let reply = server
        .try_send(Call::json(Method::POST, path, &body))
        .await?;
    reply.json()["id"].as_str().map(str::to_owned)
}

/// A pseudo-random number below `n`, from `seed` (a 64-bit LCG).
fn below(seed: &mut u64, n: u64) -> u64 {
    *seed = seed
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    (*seed >> 33) % n
}

/// XH12 over every job submitted so far: each has ended, once, as it ended
/// before (its first terminal state kept across restarts); a succeeded
/// one's document is the CLI's; one that did not succeed states no
/// verdict; nothing is left in `tmp/`.
async fn settled(
    server: &TestServer,
    jobs: &BTreeMap<String, Kind>,
    ended: &mut BTreeMap<String, String>,
) -> Vec<String> {
    let mut problems = Vec::new();
    for (id, kind) in jobs {
        let job = server.finished_job(id, Duration::from_secs(300)).await;
        let state = job["state"].as_str().unwrap_or("").to_owned();
        if let Some(before) = ended.get(id) {
            if *before != state {
                problems.push(format!("{id} ({kind:?}): {before}, now {state}"));
            }
        }
        // A job run again after a kill (SD4, SD5) is counted for the summary.
        if ended.insert(id.clone(), state.clone()).is_none() && job["attempt"] != 1 {
            ended.insert(format!("{id} again"), "run again".into());
        }
        if state == "succeeded" {
            if let Some(golden) = golden(*kind) {
                let document = server.document(&job).await;
                if document.body != golden {
                    problems.push(format!("{id} ({kind:?}): not the CLI's document"));
                }
            }
        } else if !job["outcome"].is_null() {
            problems.push(format!("{id} ({kind:?}): a verdict, {state}"));
        }
    }
    if !server.temporaries().is_empty() {
        problems.push(format!("temporaries left: {:?}", server.temporaries()));
    }
    problems
}

/// VT10's weekly soak (JB6, JS11, JS13, SD1 to SD5, XH12): N rounds, each
/// submitting two features jobs, two socket extractions, two matches, an
/// assembly reading (a job the first round only) and a catalog ingest of a
/// part under a new name, without waiting, then killing the server
/// (SIGKILL) at a pseudo-random time within 400 ms and starting it again:
/// after each round every job ever submitted has ended exactly once, as it
/// ended before, a succeeded one with the CLI's document, one that did not
/// succeed with no verdict, no temporary left; at the end the catalog
/// verifies clean (XH12's fourth invariant).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restarts_at_random_times_keep_every_job() {
    let Some(rounds) = std::env::var("FITMENT_SOAK_ROUNDS")
        .ok()
        .and_then(|n| n.parse::<u64>().ok())
    else {
        eprintln!("FITMENT_SOAK_ROUNDS is not set: the soak is weekly");
        return;
    };
    let mut server = TestServer::process(Dirs::new(), RESTART);
    let part = server.upload(&support::part("p01_through.stp")).await;
    let registered = server
        .register(&support::assembly("s01_bolted.stp"), "s01_bolted.stp")
        .await;
    assert_eq!(registered["state"], "succeeded", "{registered}");
    let assembly = fitment_core::sha256_hex(&support::assembly("s01_bolted.stp"));
    let other = server.upload(&support::assembly("s02_cbore.stp")).await;
    let candidate = server.upload(&support::candidate("m_bracket.stp")).await;
    let socket = server
        .upload(&support::cli_golden("socket", "s01_bolted--BRACKET.json"))
        .await;
    let inputs = Inputs {
        part: format!("sha256:{part}"),
        assembly: format!("sha256:{assembly}"),
        other: format!("sha256:{other}"),
        socket: format!("sha256:{socket}"),
        candidates: json!({ "files": [{ "upload": format!("sha256:{candidate}"), "label": "m_bracket.stp" }] }),
    };
    let (mut jobs, mut ended, mut seed) = (BTreeMap::new(), BTreeMap::new(), 0x5eed_u64);
    let mut problems = Vec::new();
    for round in 0..rounds {
        for kind in [
            Kind::Features,
            Kind::Socket,
            Kind::Match,
            Kind::Ingest,
            Kind::Features,
            Kind::Socket,
            Kind::Match,
            Kind::Parts,
        ] {
            let submitted = submit(&server, kind, &inputs, round).await;
            if let Some(id) = submitted {
                jobs.insert(id, kind);
            }
        }
        tokio::time::sleep(Duration::from_millis(below(&mut seed, 400))).await;
        if let Some(child) = server.child.as_mut() {
            let _ = child.child.kill();
            let _ = child.child.wait();
        }
        server.restart_process(RESTART);
        let found = settled(&server, &jobs, &mut ended).await;
        problems.extend(found.into_iter().map(|p| format!("round {round}: {p}")));
    }
    let catalog = server.dirs.catalog.clone();
    if let Some(child) = server.child.take() {
        let _ = child.stop();
    }
    assert_eq!(
        support::verified(&catalog),
        Some(0),
        "XH12: verify after the soak"
    );
    let states = ended.values().fold(BTreeMap::new(), |mut m, s| {
        *m.entry(s.clone()).or_insert(0) += 1;
        m
    });
    eprintln!(
        "VT10's soak: {rounds} rounds, {} jobs: {states:?}",
        jobs.len()
    );
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// VT11's weekly leg (UP2, UP4, OB2, OB5; M6-44): the state directory on
/// `FITMENT_TMPFS`, a small real file system, beside a ballast file the
/// test writes first; uploads of distinct 1 MiB bodies until one is
/// refused `507 storage_full`. Meanwhile the server answers health; no
/// upload is half-stored (every object whole, no temporary left). With
/// the ballast removed, an upload, an ingest, a socket extraction and a
/// match succeed (each document checked by XH11 as it is read), and the
/// catalog verifies clean.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_full_disk_refuses_then_recovers() {
    let Some(tmpfs) = std::env::var_os("FITMENT_TMPFS").map(std::path::PathBuf::from) else {
        eprintln!("FITMENT_TMPFS is not set: the full disk's leg is weekly, on Linux");
        return;
    };
    let mut dirs = Dirs::new();
    dirs.state = tmpfs.join(format!("state-{}", std::process::id()));
    let ballast = tmpfs.join(format!("ballast-{}", std::process::id()));
    std::fs::write(&ballast, vec![0_u8; 8 << 20]).unwrap();
    let toml = format!(
        "{RESTART}[paths]\ncatalog = {:?}\nstate = {:?}\n",
        dirs.catalog.display().to_string(),
        dirs.state.display().to_string()
    );
    let server = TestServer::process(dirs, &toml);
    let mut refused = None;
    for n in 0..200_u32 {
        let mut body = vec![0_u8; 1 << 20];
        body[..4].copy_from_slice(&n.to_le_bytes());
        let reply = server.send(support::put_upload(&body)).await;
        if reply.status != 201 {
            refused = Some((n, reply.status.as_u16(), reply.code()));
            break;
        }
    }
    let (count, status, code) = refused.expect("the disk never filled");
    assert_eq!(
        (status, code.as_str()),
        (507, "storage_full"),
        "after {count} uploads"
    );
    assert_eq!(server.get("/v1/health").await.status, 204);
    assert!(server.temporaries().is_empty(), "a temporary stayed");
    // OB2: every object in the store is whole, its bytes its name.
    for name in server.object_files() {
        let path = server
            .dirs
            .state
            .join("objects/sha256")
            .join(&name[..2])
            .join(&name);
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(
            fitment_core::sha256_hex(&bytes),
            name,
            "a half-stored object"
        );
    }
    std::fs::remove_file(&ballast).unwrap();
    let ingest = server
        .ingest(&[(support::part("p01_through.stp"), "p01_through.stp".into())])
        .await;
    assert_eq!(ingest["state"], "succeeded", "{ingest}");
    let socket = server.socket_of("s01_bolted", "BRACKET").await;
    let candidate = server.upload(&support::candidate("m_bracket.stp")).await;
    let job = server
        .run_match(&json!({ "socket": socket, "candidates": { "files": [
            { "upload": format!("sha256:{candidate}"), "label": "m_bracket.stp" }] } }))
        .await;
    assert_eq!(job["state"], "succeeded", "{job}");
    assert!(!server.document(&job).await.body.is_empty(), "its document");
    eprintln!("VT11's full disk: refused after {count} uploads of 1 MiB, then recovered");
    assert_eq!(
        support::verified(&server.dirs.catalog),
        Some(0),
        "XH12: verify after the full disk"
    );
    let state = server.dirs.state.clone();
    drop(server);
    let _ = std::fs::remove_dir_all(state);
}
