//! Faults (docs/research/m6-api.md, section 10.2): VT11 at every file
//! call of an upload, through the catalog's `CountingFs::failing_at` over
//! CA7's streaming calls (M6-44, review F14), and the store's protocol
//! (OB2): objects in place before their rows, unlinked only inside a
//! transaction that has just found no row naming them.
#![allow(clippy::disallowed_methods)] // a test races a sweep on a thread

mod support;

use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use axum::http::Method;
use fitment_catalog::fs::{Call as FsCall, CountingFs};
use fitment_server::auth::Role;
use support::{Call, Dirs, TestServer, put_upload, seed_token, seed_upload, seed_user};

/// The bytes every fault test uploads.
const BYTES: &[u8] = b"the bytes of a part, uploaded under faults";

/// A fresh server on `seam`, with ada (her token returned) and, when
/// `held`, bob holding an upload of [`BYTES`] already.
fn world(seam: CountingFs, held: bool) -> (TestServer, String) {
    let dirs = Dirs::new();
    let config = dirs.config();
    let server = TestServer::in_process_with_fs(dirs, config, Arc::new(seam));
    let ada = seed_user(server.server(), "ada", Role::Reader, None);
    let (_, token) = seed_token(server.server(), &ada, Role::Reader, 30);
    if held {
        let bob = seed_user(server.server(), "bob", Role::Reader, None);
        seed_upload(server.server(), bob.key, BYTES);
    }
    (server, token)
}

/// VT11, OB2, UP2, UP4, CA7: an upload is failed at each of its file
/// calls in turn, as a full disk (`507 storage_full`) and as any other
/// error (`500 io_error`), in both of UP8's branches: nothing half-stored
/// is ever visible (no row of the caller's, every object row's file
/// whole), no temporary file stays, a sweep clears what a failure after
/// the rename left, and the same upload then succeeds.
#[tokio::test]
async fn every_file_call_of_an_upload_fails_in_turn() {
    let sha256 = fitment_core::sha256_hex(BYTES);
    let path = format!("/v1/uploads/sha256:{sha256}");
    for held in [false, true] {
        let probe = CountingFs::new();
        let (server, token) = world(probe.clone(), held);
        let before = probe.calls().len();
        assert_eq!(
            server.send(put_upload(BYTES).bearer(&token)).await.status,
            201
        );
        let calls = probe.calls()[before..].to_vec();
        assert!(calls.iter().any(|c| matches!(c, FsCall::WriteChunk(..))));
        assert!(calls.iter().any(|c| matches!(c, FsCall::SyncDir(_))));
        for k in before..before + calls.len() {
            for kind in [io::ErrorKind::StorageFull, io::ErrorKind::Other] {
                let what = format!("held {held}, {:?} failed as {kind:?}", calls[k - before]);
                let (server, token) = world(CountingFs::failing_at(k, kind), held);
                let reply = server.send(put_upload(BYTES).bearer(&token)).await;
                let code = match kind {
                    io::ErrorKind::StorageFull => "storage_full",
                    _ => "io_error",
                };
                assert_eq!(reply.code(), code, "{what}");
                let head = Call::new(Method::HEAD, &path).bearer(&token);
                assert_eq!(server.send(head).await.status, 404, "{what}");
                assert!(server.store_problems().is_empty(), "{what}");
                assert!(server.temporaries().is_empty(), "{what}");
                server.server().sweep_store().unwrap();
                let left = server.object_files();
                assert_eq!(left.len(), usize::from(held), "{what}: {left:?}");
                let again = server.send(put_upload(BYTES).bearer(&token)).await;
                assert_eq!(again.status, 201, "{what}: the rerun");
                let head = Call::new(Method::HEAD, &path).bearer(&token);
                assert_eq!(server.send(head).await.status, 200, "{what}");
                assert!(server.store_problems().is_empty(), "{what}");
            }
        }
    }
}

/// OB2, UP3: the store's protocol for a new object: written under
/// `tmp/`, synced, renamed to `objects/sha256/<2 hex>/<64 hex>`, made
/// read-only (0444 on Unix) and its fan-out directory synced, all before
/// the row that names it commits, so no reader sees a row without its
/// file; the temporary file is gone.
#[tokio::test]
async fn objects_are_named_by_hash_read_only_and_synced_before_their_row() {
    let dirs = Dirs::new();
    let config = dirs.config();
    let database = dirs.state.join("server.db");
    let sha256 = fitment_core::sha256_hex(BYTES);
    let fan = dirs.state.join("objects/sha256").join(&sha256[..2]);
    let seen = Arc::new(AtomicBool::new(false));
    let checked = Arc::clone(&seen);
    let watched = fan.clone();
    let sha = sha256.clone();
    let seam = CountingFs::with_hook(move |call| {
        if *call == FsCall::SyncDir(watched.clone()) {
            let conn = rusqlite::Connection::open(&database).unwrap();
            let rows: i64 = conn
                .query_row(
                    "SELECT count(*) FROM object WHERE sha256 = ?1",
                    [&sha],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(
                rows, 0,
                "the row commits after the fan-out directory's sync"
            );
            checked.store(true, Ordering::SeqCst);
        }
    });
    let server = TestServer::in_process_with_fs(dirs, config, Arc::new(seam.clone()));
    assert_eq!(server.send(put_upload(BYTES)).await.status, 201);
    assert!(
        seen.load(Ordering::SeqCst),
        "the fan-out directory was synced"
    );
    let object = fan.join(&sha256);
    assert_eq!(std::fs::read(&object).unwrap(), BYTES);
    assert!(std::fs::metadata(&object).unwrap().permissions().readonly());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(&object).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o444);
    }
    let calls = seam.calls();
    let at = |want: &dyn Fn(&FsCall) -> bool| calls.iter().position(want).unwrap();
    let rename = at(&|c| {
        matches!(c, FsCall::Rename(from, to) if to == &object
        && from.extension().is_some_and(|e| e == "part"))
    });
    let readonly = at(&|c| *c == FsCall::SetReadonly(object.clone(), true));
    let synced = at(&|c| *c == FsCall::SyncDir(fan.clone()));
    let stream_synced = at(&|c| matches!(c, FsCall::SyncStream(_)));
    assert!(stream_synced < rename && rename < readonly && readonly < synced);
    assert_eq!(server.temporaries(), Vec::<std::path::PathBuf>::new());
    assert_eq!(server.store_problems(), Vec::<String>::new());
}

/// The HEAD of an upload of `bytes`, with `token`.
fn head(bytes: &[u8], token: &str) -> Call {
    let path = format!("/v1/uploads/sha256:{}", fitment_core::sha256_hex(bytes));
    Call::new(Method::HEAD, &path).bearer(token)
}

/// OB2, OB4, UP3: the sweep deletes expired uploads (an expired one names
/// nothing even before it), then unlinks only objects no row names: an
/// object another user's row holds stays; a file without a row goes, a
/// file not named as an object stays; temporary files older than any
/// upload lives go, newer ones stay.
#[tokio::test]
async fn a_sweep_unlinks_only_what_no_row_names() {
    let (start, day) = (1_790_000_000_000_i64, 86_400_000_i64);
    let server = TestServer::fresh();
    let seams = server.server().seams();
    let ada = seed_user(server.server(), "ada", Role::Reader, None);
    let bob = seed_user(server.server(), "bob", Role::Reader, None);
    let (_, ada_token) = seed_token(server.server(), &ada, Role::Reader, 30);
    let (_, bob_token) = seed_token(server.server(), &bob, Role::Reader, 30);
    let (x, y) = (b"shared bytes".as_slice(), b"ada's own".as_slice());
    for (bytes, token) in [(x, &ada_token), (y, &ada_token)] {
        assert_eq!(
            server.send(put_upload(bytes).bearer(token)).await.status,
            201
        );
    }
    seams.ids.fix_time(start + 2 * 3_600_000);
    assert_eq!(
        server.send(put_upload(x).bearer(&bob_token)).await.status,
        201
    );
    // Ada's rows expire a day after her uploads: nothing is named by them
    // from then, swept or not.
    seams.ids.fix_time(start + day + 1);
    assert_eq!(server.send(head(x, &ada_token)).await.status, 404);
    assert_eq!(server.send(head(x, &bob_token)).await.status, 200);
    let fan = server.dirs.state.join("objects/sha256/ab");
    std::fs::create_dir_all(&fan).unwrap();
    let orphan = "ab".to_owned() + &"0".repeat(62);
    std::fs::write(fan.join(&orphan), b"no row names me").unwrap();
    std::fs::write(fan.join("README"), b"not an object's name").unwrap();
    let tmp = server.dirs.state.join("tmp");
    let old = std::fs::File::create(tmp.join("old.part")).unwrap();
    old.set_modified(std::time::SystemTime::now() - Duration::from_secs(7200))
        .unwrap();
    std::fs::write(tmp.join("new.part"), b"an upload in flight").unwrap();
    let swept = server.server().sweep_store().unwrap();
    assert_eq!(swept.expired, 2);
    assert_eq!(swept.unlinked, [fitment_core::sha256_hex(y)]);
    assert_eq!((swept.orphans, swept.temporaries), (1, 1));
    let mut left = server.object_files();
    left.retain(|n| n != "README");
    assert_eq!(left, [fitment_core::sha256_hex(x)], "bob's row holds x");
    assert!(fan.join("README").exists());
    assert!(tmp.join("new.part").exists() && !tmp.join("old.part").exists());
    std::fs::remove_file(tmp.join("new.part")).unwrap();
    // Bob's row expires too: x goes with its last row.
    seams.ids.fix_time(start + 2 * day);
    let swept = server.server().sweep_store().unwrap();
    assert_eq!(swept.unlinked, [fitment_core::sha256_hex(x)]);
    left = server.object_files();
    left.retain(|n| n != "README");
    assert_eq!(left, Vec::<String>::new());
}

/// OB2, UP8: an upload racing a sweep keeps its object. Its bytes' only
/// row has expired, unswept; a sweep starts inside the upload's
/// transaction, just after its look for the object's row, and waits for
/// it to commit, then finds the upload's row: the object stays whole.
#[tokio::test]
async fn a_sweep_racing_an_upload_keeps_its_object() {
    let race = Arc::new(AtomicBool::new(false));
    let sweeper: Arc<std::sync::Mutex<Option<fitment_server::Server>>> = Arc::default();
    let thread: Arc<std::sync::Mutex<Option<std::thread::JoinHandle<()>>>> = Arc::default();
    let seam = {
        let (race, sweeper, thread) =
            (Arc::clone(&race), Arc::clone(&sweeper), Arc::clone(&thread));
        CountingFs::with_hook(move |call| {
            // Inside the upload's transaction, after its look for the row.
            if matches!(call, FsCall::Metadata(_)) && race.swap(false, Ordering::SeqCst) {
                let server = sweeper.lock().unwrap().clone().unwrap();
                *thread.lock().unwrap() = Some(std::thread::spawn(move || {
                    server.sweep_store().unwrap();
                }));
                std::thread::sleep(Duration::from_millis(100));
            }
        })
    };
    let dirs = Dirs::new();
    let config = dirs.config();
    let server = TestServer::in_process_with_fs(dirs, config, Arc::new(seam));
    *sweeper.lock().unwrap() = Some(server.server().clone());
    let ada = seed_user(server.server(), "ada", Role::Reader, None);
    let bob = seed_user(server.server(), "bob", Role::Reader, None);
    let (_, ada_token) = seed_token(server.server(), &ada, Role::Reader, 30);
    let (_, bob_token) = seed_token(server.server(), &bob, Role::Reader, 30);
    assert_eq!(
        server
            .send(put_upload(BYTES).bearer(&bob_token))
            .await
            .status,
        201
    );
    server
        .server()
        .seams()
        .ids
        .fix_time(1_790_000_000_000 + 86_400_000 + 1);
    race.store(true, Ordering::SeqCst);
    assert_eq!(
        server
            .send(put_upload(BYTES).bearer(&ada_token))
            .await
            .status,
        201
    );
    thread.lock().unwrap().take().unwrap().join().unwrap();
    assert_eq!(server.send(head(BYTES, &ada_token)).await.status, 200);
    assert_eq!(server.send(head(BYTES, &bob_token)).await.status, 404);
    assert_eq!(server.object_files().len(), 1);
    assert_eq!(
        server.store_problems(),
        Vec::<String>::new(),
        "x stayed whole"
    );
    let swept = server.server().sweep_store().unwrap();
    assert!(swept.unlinked.is_empty(), "ada's new row holds it");
    // The hook's handle on the server goes, so the server drops whole.
    sweeper.lock().unwrap().take();
}

/// F20, OB2, UP3, UP4: a job's inputs hold its uploads from its
/// submission, in the one hold window OB4 states (the rows PR 5's
/// submission writes are seeded here): while the job is not terminal its
/// upload cannot be deleted (`409 upload_in_use`); once it ends, the
/// caller may delete the row, and the object stays while the job's input
/// names it; a retained match's input is still the caller's to name after
/// the upload expires, a non-match job's is not; the inputs count in the
/// caller's quota; and the sweep unlinks the object only when no input
/// names it.
#[tokio::test]
async fn a_jobs_inputs_hold_its_uploads_from_submission() {
    let server = TestServer::fresh();
    let seams = server.server().seams();
    let ada = seed_user(server.server(), "ada", Role::Reader, None);
    let (_, token) = seed_token(server.server(), &ada, Role::Reader, 30);
    let (matched, read) = (
        b"a match's candidate".as_slice(),
        b"a features job's".as_slice(),
    );
    // Seeded rows the dispatcher must leave queued (its reconciliation).
    server.server().hold_jobs();
    let mut jobs = Vec::new();
    for (bytes, kind) in [(matched, "match"), (read, "features")] {
        assert_eq!(
            server.send(put_upload(bytes).bearer(&token)).await.status,
            201
        );
        let sha256 = fitment_core::sha256_hex(bytes);
        jobs.push(support::seed_job_input(
            &server, ada.key, &sha256, kind, "queued",
        ));
    }
    let delete = |bytes: &[u8]| {
        let path = format!("/v1/uploads/sha256:{}", fitment_core::sha256_hex(bytes));
        Call::new(Method::DELETE, &path).bearer(&token)
    };
    let refused = server.send(delete(matched)).await;
    assert_eq!(
        (refused.status.as_u16(), refused.code()),
        (409, "upload_in_use".into())
    );
    let held = server
        .send(
            Call::get(&format!(
                "/v1/uploads/sha256:{}",
                fitment_core::sha256_hex(matched)
            ))
            .bearer(&token),
        )
        .await;
    assert_eq!(held.json()["held_by"], serde_json::json!(["upload", "job"]));
    assert_eq!(held.json()["pinned_by_jobs"], 1);
    let charge = |bytes: &[u8]| fitment_server::state::charge(u64::try_from(bytes.len()).unwrap());
    let total = charge(matched) + charge(read);
    assert_eq!(
        seams.db.charged_bytes(ada.key, seams.ids.now_ms()).unwrap(),
        total
    );
    for id in &jobs {
        support::set_job_state(&server, id, "succeeded");
    }
    assert_eq!(server.send(delete(matched)).await.status, 204);
    // The uploads expire; the jobs' records are retained.
    seams.ids.fix_time(1_790_000_000_000 + 2 * 86_400_000);
    assert_eq!(
        server.send(head(matched, &token)).await.status,
        200,
        "a match's input"
    );
    assert_eq!(
        server.send(head(read, &token)).await.status,
        404,
        "not a match's"
    );
    assert_eq!(
        seams.db.charged_bytes(ada.key, seams.ids.now_ms()).unwrap(),
        total
    );
    let swept = server.server().sweep_store().unwrap();
    assert!(swept.unlinked.is_empty(), "the inputs hold both");
    assert_eq!(server.object_files().len(), 2);
    let conn = rusqlite::Connection::open(server.dirs.state.join("server.db")).unwrap();
    conn.execute("DELETE FROM job_input", []).unwrap();
    let swept = server.server().sweep_store().unwrap();
    assert_eq!(swept.unlinked.len(), 2, "no row names them now");
    assert_eq!(server.object_files(), Vec::<String>::new());
}

/// OB2, OB4 (H2 of PR 4's review): every column naming an object is
/// indexed, and the sweep looks only at candidates (objects whose naming
/// row went), in short batches: over a store of 20,000 objects a sweep
/// with nothing to do is quick, one that unlinks half of them is bounded,
/// a `PUT` beside it waits at most a batch, and a `DELETE` and a `HEAD`
/// are one indexed probe each.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_sweep_follows_its_candidates_not_the_store() {
    let server = TestServer::fresh();
    let ada = seed_user(server.server(), "ada", Role::Reader, None);
    let (_, token) = seed_token(server.server(), &ada, Role::Reader, 30);
    let mut conn = rusqlite::Connection::open(server.dirs.state.join("server.db")).unwrap();
    let tx = conn.transaction().unwrap();
    for i in 0..20_000_u32 {
        let sha256 = fitment_core::sha256_hex(&i.to_be_bytes());
        tx.execute(
            "INSERT INTO object (sha256, bytes, added_ms) VALUES (?1, 10, 0)",
            [&sha256],
        )
        .unwrap();
        // Half expire at 1, half live on.
        let expires = if i % 2 == 0 { 1_i64 } else { i64::MAX };
        tx.execute(
            "INSERT INTO upload (owner, sha256, created_ms, expires_ms) VALUES (?1, ?2, 0, ?3)",
            rusqlite::params![ada.key, sha256, expires],
        )
        .unwrap();
    }
    tx.commit().unwrap();
    drop(conn);
    // The sweep makes one file call per candidate: 1.7 s for the 10,000 on
    // macOS, but 33 s on CI's Windows runner (about 3 ms a call there), so
    // Windows has its own bound; the idle sweep below is the scale check.
    let bound = Duration::from_secs(if cfg!(windows) {
        90
    } else if cfg!(debug_assertions) {
        10
    } else {
        5
    });
    let started = std::time::Instant::now();
    let sweeping = server.server().clone();
    let sweep = tokio::task::spawn_blocking(move || sweeping.sweep_store().unwrap());
    // Another user's: ada holds 10,000 live uploads, the most allowed.
    let bob = seed_user(server.server(), "bob", Role::Reader, None);
    let (_, bob_token) = seed_token(server.server(), &bob, Role::Reader, 30);
    let put = std::time::Instant::now();
    assert_eq!(
        server
            .send(put_upload(BYTES).bearer(&bob_token))
            .await
            .status,
        201
    );
    assert!(
        put.elapsed() < Duration::from_secs(2),
        "a PUT waited {:?}",
        put.elapsed()
    );
    let swept = sweep.await.unwrap();
    assert_eq!((swept.expired, swept.unlinked.len()), (10_000, 10_000));
    assert!(
        started.elapsed() < bound,
        "the sweep took {:?}",
        started.elapsed()
    );
    let idle = std::time::Instant::now();
    let swept = server.server().sweep_store().unwrap();
    assert!(swept.unlinked.is_empty() && swept.expired == 0);
    assert!(
        idle.elapsed() < Duration::from_secs(2),
        "an idle sweep took {:?}",
        idle.elapsed()
    );
    let live = fitment_core::sha256_hex(&1_u32.to_be_bytes());
    let path = format!("/v1/uploads/sha256:{live}");
    let probes = std::time::Instant::now();
    assert_eq!(
        server
            .send(Call::new(Method::HEAD, &path).bearer(&token))
            .await
            .status,
        200
    );
    assert_eq!(
        server
            .send(Call::new(Method::DELETE, &path).bearer(&token))
            .await
            .status,
        204
    );
    assert!(
        probes.elapsed() < Duration::from_secs(1),
        "{:?}",
        probes.elapsed()
    );
}

/// OB2, UP8 (M2 and L2 of PR 4's review): an upload renames its file over
/// whatever stands at the object's name: a directory planted there is
/// removed, and a file of the right length but the wrong bytes under
/// another user's row is replaced by the bytes just hashed, so the store
/// heals; the read-only object is renamed over on every platform.
#[tokio::test]
async fn whatever_stands_at_an_objects_name_is_replaced() {
    let server = TestServer::fresh();
    let ada = seed_user(server.server(), "ada", Role::Reader, None);
    let bob = seed_user(server.server(), "bob", Role::Reader, None);
    let (_, ada_token) = seed_token(server.server(), &ada, Role::Reader, 30);
    let (_, bob_token) = seed_token(server.server(), &bob, Role::Reader, 30);
    let sha256 = fitment_core::sha256_hex(BYTES);
    let object = server
        .dirs
        .state
        .join("objects/sha256")
        .join(&sha256[..2])
        .join(&sha256);
    std::fs::create_dir(&object).unwrap();
    assert_eq!(
        server
            .send(put_upload(BYTES).bearer(&ada_token))
            .await
            .status,
        201
    );
    assert_eq!(
        std::fs::read(&object).unwrap(),
        BYTES,
        "the directory replaced"
    );
    let mut permissions = std::fs::metadata(&object).unwrap().permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    permissions.set_readonly(false);
    std::fs::set_permissions(&object, permissions).unwrap();
    let wrong = vec![b'w'; BYTES.len()];
    std::fs::write(&object, &wrong).unwrap();
    assert_eq!(server.store_problems().len(), 1, "damaged under ada's row");
    assert_eq!(
        server
            .send(put_upload(BYTES).bearer(&bob_token))
            .await
            .status,
        201
    );
    assert_eq!(
        std::fs::read(&object).unwrap(),
        BYTES,
        "healed by bob's upload"
    );
    assert!(std::fs::metadata(&object).unwrap().permissions().readonly());
    assert_eq!(server.store_problems(), Vec::<String>::new());
    assert_eq!(
        server
            .send(put_upload(BYTES).bearer(&bob_token))
            .await
            .status,
        200
    );
    assert_eq!(server.temporaries(), Vec::<std::path::PathBuf>::new());
}

// ---------------------------------------------------------------- jobs

use fitment_server::hooks::{Action, Gate};
use serde_json::{Value, json};
use support::waiting;

/// The unit part the job tests read, and the CLI's features of it.
fn p01() -> (Vec<u8>, Vec<u8>) {
    (
        support::part("p01_through.stp"),
        support::cli_golden("features", "p01_through.json"),
    )
}

/// XH12's third invariant for a job that did not succeed: no verdict.
fn no_verdict(job: &Value) {
    if job["state"] != "succeeded" {
        assert!(job["outcome"].is_null(), "XH12: {job}");
    }
}

/// The body of a features job of `sha`.
fn features(sha: &str) -> Value {
    json!({ "upload": format!("sha256:{sha}") })
}

/// VT9, JS6, SN7: a job that panics at each named point (its input read,
/// its call returned, its document stored, just before its finishing
/// transaction) ends `failed`, `internal`, with no document and no
/// verdict; the panic's text is in the log under the job's id and in no
/// response (the harness's scan); the server keeps serving, and the job
/// after it ends with the CLI's bytes. A panic after the commit leaves the
/// job's end as it committed (XH12's first invariant).
#[tokio::test]
async fn a_job_that_panics_fails_internal_and_the_server_goes_on() {
    let server = TestServer::fresh();
    let (bytes, golden) = p01();
    let sha = server.upload(&bytes).await;
    let hooks = &server.server().seams().hooks;
    for point in ["job.read", "job.ran", "job.stored", "job.committing"] {
        hooks.set(point, Action::Panic(format!("a planted panic at {point}")));
        let job = server
            .send(waiting("/v1/features", &features(&sha)))
            .await
            .json();
        assert_eq!(job["state"], "failed", "{point}: {job}");
        assert_eq!(job["error"]["code"], "internal", "{point}");
        assert!(job["document"].is_null(), "{point}");
        no_verdict(&job);
        let id = job["id"].as_str().unwrap();
        let log = support::logs::text();
        assert!(
            log.lines()
                .any(|l| l.contains(id) && l.contains(&format!("a planted panic at {point}"))),
            "{point}: the panic is logged under the job's id"
        );
        hooks.clear(point);
        let next = server
            .send(waiting("/v1/features", &features(&sha)))
            .await
            .json();
        assert_eq!(next["state"], "succeeded", "{point}");
        assert_eq!(server.document(&next).await.body, golden, "{point}");
    }
    hooks.set(
        "job.committed",
        Action::Panic("a planted panic after the commit".into()),
    );
    let job = server
        .send(waiting("/v1/features", &features(&sha)))
        .await
        .json();
    hooks.clear("job.committed");
    assert_eq!(job["state"], "succeeded", "{job}");
    assert_eq!(server.document(&job).await.body, golden);
}

/// VT11, OB2, OB5, UP7: a features job on a seam that fails each file call
/// of its run in turn (its input's read, its document's temporary file,
/// chunk, sync, rename, flag and directory sync), as a full disk and as
/// any other error: the job never succeeds with a short document (it
/// fails, with no document, `storage_full`, `io_error` or `blob_damaged`),
/// nothing half-stored is visible, a sweep clears what is left, and the
/// same job then succeeds with the CLI's bytes.
#[tokio::test]
async fn every_file_call_of_a_jobs_document_fails_in_turn() {
    let (bytes, golden) = p01();
    let run = |seam: CountingFs| {
        let bytes = bytes.clone();
        async move {
            let dirs = Dirs::new();
            let config = dirs.config();
            let server = TestServer::in_process_with_fs(dirs, config, Arc::new(seam));
            // Uploaded as the harness's user, the same calls each time; an
            // upload the seam fails is VT11's upload half, not this one.
            let reply = server.send(put_upload(&bytes)).await;
            let sha = reply
                .status
                .is_success()
                .then(|| fitment_core::sha256_hex(&bytes));
            (server, sha)
        }
    };
    let probe = CountingFs::new();
    let (server, sha) = run(probe.clone()).await;
    let sha = sha.unwrap();
    let before = probe.calls().len();
    let job = server
        .send(waiting("/v1/features", &features(&sha)))
        .await
        .json();
    assert_eq!(job["state"], "succeeded");
    let calls = probe.calls()[before..].to_vec();
    assert!(calls.iter().any(|c| matches!(c, FsCall::WriteChunk(..))));
    drop(server);
    for k in before..before + calls.len() {
        for kind in [io::ErrorKind::StorageFull, io::ErrorKind::Other] {
            let what = format!("{:?} failed as {kind:?}", calls[k - before]);
            let (server, sha) = run(CountingFs::failing_at(k, kind)).await;
            let Some(sha) = sha else { continue };
            let job = server
                .send(waiting("/v1/features", &features(&sha)))
                .await
                .json();
            if job["state"] == "succeeded" {
                assert_eq!(
                    server.document(&job).await.body,
                    golden,
                    "{what}: a whole document"
                );
            } else {
                assert_eq!(job["state"], "failed", "{what}: {job}");
                assert!(job["document"].is_null(), "{what}");
                let code = job["error"]["code"].as_str().unwrap().to_owned();
                assert!(
                    ["storage_full", "io_error", "blob_damaged", "internal"]
                        .contains(&code.as_str()),
                    "{what}: {code}"
                );
                no_verdict(&job);
            }
            assert!(server.store_problems().is_empty(), "{what}");
            server.server().sweep_store().unwrap();
            assert!(server.temporaries().is_empty(), "{what}");
            let again = server
                .send(waiting("/v1/features", &features(&sha)))
                .await
                .json();
            assert_eq!(again["state"], "succeeded", "{what}: the rerun");
            assert_eq!(server.document(&again).await.body, golden, "{what}");
        }
    }
}

/// A fresh server with `s02_cbore.stp` registered: its SHA-256.
async fn registered() -> (TestServer, String) {
    let server = TestServer::fresh();
    let job = server
        .register(&support::assembly("s02_cbore.stp"), "s02_cbore.stp")
        .await;
    assert_eq!(job["state"], "succeeded");
    let sha = fitment_core::sha256_hex(&support::assembly("s02_cbore.stp"));
    (server, sha)
}

/// The extraction of `BLOCK` from `sha`, submitted without a wait: its id.
async fn submit_extraction(server: &TestServer, sha: &str) -> String {
    let body = json!({ "assembly": format!("sha256:{sha}"), "part": "BLOCK" });
    let reply = server
        .send(Call::json(Method::POST, "/v1/sockets", &body))
        .await;
    assert_eq!(
        reply.status,
        202,
        "{}",
        String::from_utf8_lossy(&reply.body)
    );
    reply.json()["id"].as_str().unwrap().to_owned()
}

/// The caller's socket lines.
async fn lines(server: &TestServer) -> Vec<Value> {
    server.get("/v1/sockets").await.json()["items"]
        .as_array()
        .unwrap()
        .clone()
}

/// VT41, JB5, JS8 for these kinds: a cancel at each point a running
/// extraction may be stopped at (its input read, its call returned, its
/// document stored, just before its finishing transaction) makes it
/// `cancelling` until its call returns, then `cancelled`, with no
/// document and no revision; one after its commit is `409 job_finished`,
/// the job as it ended; a queued job is `cancelled` at once.
#[tokio::test]
async fn a_cancel_stops_a_job_where_it_may_stop() {
    for point in [
        "job.read",
        "job.ran",
        "job.stored",
        "job.committing",
        "job.committed",
    ] {
        let (server, sha) = registered().await;
        let gate = Gate::new();
        server
            .server()
            .seams()
            .hooks
            .set(point, Action::Pause(gate.clone()));
        let id = submit_extraction(&server, &sha).await;
        assert!(gate.arrived(1, Duration::from_secs(30)).await, "{point}");
        let cancel = server
            .send(Call::new(Method::POST, &format!("/v1/jobs/{id}/cancel")))
            .await;
        gate.open();
        let job = server.finished_job(&id, Duration::from_secs(60)).await;
        if point == "job.committed" {
            assert_eq!(cancel.code(), "job_finished", "{point}");
            assert_eq!(job["state"], "succeeded");
            assert_eq!(lines(&server).await.len(), 1);
            continue;
        }
        assert_eq!(cancel.status, 202, "{point}");
        assert_eq!(cancel.json()["state"], "cancelling", "{point}");
        assert_eq!(cancel.json()["cancel_requested"], true);
        assert_eq!(job["state"], "cancelled", "{point}: {job}");
        assert!(
            job["document"].is_null() && job["result"].is_null(),
            "{point}"
        );
        no_verdict(&job);
        assert!(lines(&server).await.is_empty(), "{point}: no revision");
        assert!(server.temporaries().is_empty(), "{point}");
    }
    let (server, sha) = registered().await;
    server.server().hold_jobs();
    let id = submit_extraction(&server, &sha).await;
    let cancel = server
        .send(Call::new(Method::POST, &format!("/v1/jobs/{id}/cancel")))
        .await;
    assert_eq!(cancel.json()["state"], "cancelled");
    server.server().resume_jobs();
    let job = server.finished_job(&id, Duration::from_secs(30)).await;
    assert_eq!(job["state"], "cancelled");
}

/// VT49, OB4, JS11, UP5: a job's inputs are held from the commit that
/// creates it (its `job_input` rows exist at `job.submitted`, before it is
/// queued, and its upload's `DELETE` is `409 upload_in_use`) until it
/// ends (the rows gone, the `DELETE` `204`); an upload a queued job names
/// outlives its lifetime: the sweep keeps its object and the job runs on
/// it.
#[tokio::test]
async fn a_job_holds_its_inputs_from_submission_to_its_end() {
    let server = Arc::new(TestServer::fresh());
    let (bytes, golden) = p01();
    let sha = server.upload(&bytes).await;
    let gate = Gate::new();
    server
        .server()
        .seams()
        .hooks
        .set("job.submitted", Action::Pause(gate.clone()));
    let submitting = Arc::clone(&server);
    let body = features(&sha);
    let task = tokio::spawn(async move {
        submitting
            .send(Call::json(Method::POST, "/v1/features", &body))
            .await
    });
    assert!(gate.arrived(1, Duration::from_secs(30)).await);
    let conn = rusqlite::Connection::open(server.dirs.state.join("server.db")).unwrap();
    let inputs = |conn: &rusqlite::Connection| -> i64 {
        conn.query_row(
            "SELECT count(*) FROM job_input WHERE sha256 = ?1",
            [&sha],
            |r| r.get(0),
        )
        .unwrap()
    };
    assert_eq!(inputs(&conn), 1, "held from its commit");
    let delete = Call::new(Method::DELETE, &format!("/v1/uploads/sha256:{sha}"));
    assert_eq!(server.send(delete.clone()).await.code(), "upload_in_use");
    gate.open();
    let reply = task.await.unwrap();
    assert_eq!(reply.status, 202);
    let id = reply.json()["id"].as_str().unwrap().to_owned();
    let job = server.finished_job(&id, Duration::from_secs(30)).await;
    assert_eq!(job["state"], "succeeded");
    assert_eq!(inputs(&conn), 0, "released at its end");
    assert_eq!(server.send(delete).await.status, 204);
    // A queued job keeps its expired upload's object.
    let sha = server.upload(&bytes).await;
    server.server().hold_jobs();
    let queued = server
        .send(Call::json(Method::POST, "/v1/features", &features(&sha)))
        .await
        .json();
    let seams = server.server().seams();
    seams.ids.fix_time(1_790_000_000_000 + 2 * 86_400_000);
    let swept = server.server().sweep_store().unwrap();
    assert!(swept.expired >= 1 && swept.unlinked.is_empty(), "{swept:?}");
    server.server().resume_jobs();
    let job = server
        .finished_job(queued["id"].as_str().unwrap(), Duration::from_secs(30))
        .await;
    assert_eq!(job["state"], "succeeded", "{job}");
    assert_eq!(server.document(&job).await.body, golden);
}

/// SD4, JB6, SD5, JS13: a start settles what the last process left, from
/// `server.db` alone: a running pure job is queued again with its next
/// attempt and runs; one on its second attempt is `interrupted`, not
/// retryable (PR 5's review), its key released; a cancelled one is
/// `cancelled`; one past
/// its deadline `failed`, `deadline_exceeded`; an extraction whose
/// revision committed ends with it and is not run again (one revision);
/// queued jobs run in their order. XH12: each ends once, and stays so.
#[tokio::test]
async fn a_start_settles_what_the_last_process_left() {
    let (server, sha) = registered().await;
    let socket = server.extract(&sha, "BLOCK").await;
    assert_eq!(socket["state"], "succeeded");
    let (bytes, golden) = p01();
    let part = server.upload(&bytes).await;
    server.server().hold_jobs();
    let mut ids = Vec::new();
    for n in 0..5 {
        let call = Call::json(Method::POST, "/v1/features", &features(&part))
            .header("idempotency-key", &format!("key-{n}"));
        ids.push(
            server.send(call).await.json()["id"]
                .as_str()
                .unwrap()
                .to_owned(),
        );
    }
    let config = server.dirs.config();
    let conn = rusqlite::Connection::open(server.dirs.state.join("server.db")).unwrap();
    let set = |id: &str, sql: &str| {
        conn.execute(&format!("UPDATE job SET {sql} WHERE id = ?1"), [id])
            .unwrap();
    };
    set(&ids[0], "state = 'running', attempt = 1");
    set(&ids[1], "state = 'running', attempt = 2");
    set(&ids[2], "state = 'cancelling', cancel_requested = 1");
    set(&ids[3], "state = 'cancelling', cancel_requested = 0");
    set(
        socket["id"].as_str().unwrap(),
        "state = 'running', exit = NULL",
    );
    drop(conn);
    let server = server.restart(config);
    let ended = |id: &str| {
        let id = id.to_owned();
        let server = &server;
        async move { server.finished_job(&id, Duration::from_secs(60)).await }
    };
    let first = ended(&ids[0]).await;
    assert_eq!(
        (first["state"].as_str(), first["attempt"].as_i64()),
        (Some("succeeded"), Some(2))
    );
    assert_eq!(server.document(&first).await.body, golden);
    let second = ended(&ids[1]).await;
    assert_eq!(second["state"], "interrupted");
    assert_eq!(second["error"]["code"], "job_interrupted");
    assert_eq!(
        second["error"]["retryable"], false,
        "a pure job's second attempt: PR 5's review"
    );
    no_verdict(&second);
    let again = Call::json(Method::POST, "/v1/features", &features(&part))
        .header("idempotency-key", "key-1");
    let resubmitted = server.send(again).await.json();
    assert_ne!(resubmitted["id"], second["id"], "JB9: the key was released");
    assert_eq!(ended(&ids[2]).await["state"], "cancelled");
    let deadline = ended(&ids[3]).await;
    assert_eq!(deadline["error"]["code"], "deadline_exceeded");
    no_verdict(&deadline);
    assert_eq!(ended(&ids[4]).await["state"], "succeeded");
    let extraction = ended(socket["id"].as_str().unwrap()).await;
    assert_eq!(extraction["state"], "succeeded");
    assert_eq!(extraction["result"], socket["result"]);
    let line = &lines(&server).await[0];
    assert_eq!(line["current"]["n"], 1, "JS13: not run again");
}

/// The request a kind's job is made by, after its setup on `server`:
/// `socket` and `import` on a registered s02, `match` on its extracted
/// BLOCK and the committed candidates, `assembly` on the bytes alone.
async fn abort_setup(server: &TestServer, kind: &str, sha: &str) -> support::Call {
    let register = support::waiting(
        "/v1/assemblies",
        &json!({ "upload": format!("sha256:{sha}"), "file_name": "s02_cbore.stp" }),
    );
    if kind == "assembly" {
        return register;
    }
    assert_eq!(server.send(register).await.json()["state"], "succeeded");
    match kind {
        "socket" => support::waiting(
            "/v1/sockets",
            &json!({ "assembly": format!("sha256:{sha}"), "part": "BLOCK" }),
        ),
        "import" => {
            let file = server
                .upload(&support::cli_golden("socket", "s02_cbore--BLOCK.json"))
                .await;
            support::waiting(
                "/v1/sockets/import",
                &json!({ "upload": format!("sha256:{file}") }),
            )
        }
        _ => {
            let job = server.extract(sha, "BLOCK").await;
            let socket = job["result"]["href"]
                .as_str()
                .unwrap()
                .trim_start_matches("/v1/sockets/");
            let mut files = Vec::new();
            for name in ["m_block02.stp", "m_block02_cb_deep.stp"] {
                let upload = server.upload(&support::candidate(name)).await;
                files.push(json!({ "upload": format!("sha256:{upload}"), "label": name }));
            }
            support::waiting(
                "/v1/matches",
                &json!({ "socket": socket, "candidates": { "files": files } }),
            )
        }
    }
}

/// VT10, JB6, JS11, JS13, SD2, SD4, SD5: the built binary aborted at each
/// named point of a job's life, then started again on the same
/// directories, for extractions, imports, assembly readings and matches:
/// the job ends once, as JB6 says; a pure job run again ends with the
/// CLI's bytes, on its second attempt if the first had started; one whose
/// result committed is not run again (one revision, its attempt 1); one
/// killed on both attempts is `interrupted`; a request whose answer the
/// abort lost is found again by its `Idempotency-Key`. On Unix and Windows
/// alike (`abort()` at the hook).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_abort_at_every_hook_point_leaves_each_job_ending_once() {
    const RESTART: &str = "[test]\nlease_stale_ms = 1\n";
    let golden = support::cli_golden("socket", "s02_cbore--BLOCK.json");
    let points = [
        "job.submitted",
        "job.dispatched",
        "job.read",
        "job.ran",
        "job.stored",
        "job.committing",
        "job.committed",
    ];
    for point in points {
        for kind in ["socket", "assembly", "import", "match"] {
            let mut server = TestServer::process(Dirs::new(), RESTART);
            let bytes = support::assembly("s02_cbore.stp");
            let sha = server.upload(&bytes).await;
            let call = abort_setup(&server, kind, &sha)
                .await
                .header("idempotency-key", "the-one");
            let hooks = format!("{RESTART}hooks = {{ \"{point}\" = \"abort\" }}\n");
            server.restart_process(&hooks);
            let answered = server.try_send(call.clone()).await;
            assert!(answered.is_none(), "{point} {kind}: the abort answered");
            server.restart_process(RESTART);
            let what = format!("{point} {kind}");
            let job = server.send(call).await.json();
            let id = job["id"]
                .as_str()
                .unwrap_or_else(|| panic!("{what}: {job}"))
                .to_owned();
            let job = server.finished_job(&id, Duration::from_secs(60)).await;
            assert_eq!(job["state"], "succeeded", "{what}: {job}");
            assert!(
                server.temporaries().is_empty(),
                "{what}: SD2, SD4: no temporary left"
            );
            let started = !matches!(point, "job.submitted" | "job.committed");
            assert_eq!(job["attempt"], if started { 2 } else { 1 }, "{what}");
            match kind {
                "socket" | "import" => {
                    assert_eq!(server.document(&job).await.body, golden, "{what}");
                    let lines = server.get("/v1/sockets").await.json();
                    assert_eq!(lines["items"][0]["current"]["n"], 1, "{what}: one revision");
                }
                "assembly" => {
                    let parts = server
                        .get(&format!("/v1/assemblies/sha256:{sha}/parts"))
                        .await;
                    assert_eq!(
                        parts.body,
                        support::cli_golden("parts", "s02_cbore.json"),
                        "{what}"
                    );
                }
                _ => assert_eq!(
                    server.document(&job).await.body,
                    support::cli_golden("match", "s02_cbore--BLOCK.json"),
                    "{what}"
                ),
            }
            let kind_name = match kind {
                "socket" => "socket_extract",
                "import" => "socket_import",
                "match" => "match",
                _ => "assembly_read",
            };
            let all = server
                .get(&format!("/v1/jobs?kind={kind_name}"))
                .await
                .json();
            if kind != "assembly" {
                assert_eq!(all["items"].as_array().unwrap().len(), 1, "{what}: one job");
            }
        }
    }
}

/// VT10, JB6, SD5, JB9: a pure job killed on both its attempts is
/// `interrupted`, `job_interrupted`, with no verdict, and its key is
/// released, so the same request submits anew.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_job_killed_on_both_attempts_is_interrupted() {
    const RESTART: &str = "[test]\nlease_stale_ms = 1\n";
    let mut server = TestServer::process(Dirs::new(), RESTART);
    let sha = server.upload(&support::assembly("s02_cbore.stp")).await;
    let hooks = format!("{RESTART}hooks = {{ \"job.read\" = \"abort\" }}\n");
    server.restart_process(&hooks);
    let register = support::waiting(
        "/v1/assemblies",
        &json!({ "upload": format!("sha256:{sha}") }),
    )
    .header("idempotency-key", "twice");
    assert!(server.try_send(register.clone()).await.is_none());
    server.restart_process(&hooks);
    // The re-queued job is dispatched at the start, and aborts again.
    let mut tries = 0;
    while server.try_send(Call::get("/v1/health")).await.is_some() {
        tries += 1;
        assert!(tries < 600, "the second attempt never aborted");
        let _ = server.try_send(Call::get("/v1/jobs?wait=1")).await;
    }
    server.restart_process(RESTART);
    let listed = server.get("/v1/jobs?kind=assembly_read").await.json();
    let job = &listed["items"][0];
    assert_eq!(job["state"], "interrupted", "{job}");
    assert_eq!(job["attempt"], 2);
    assert_eq!(job["error"]["code"], "job_interrupted");
    no_verdict(job);
    // JB9: the key was released with it, so it submits anew.
    let again = server.send(register).await.json();
    assert_ne!(again["id"], job["id"]);
}

/// SD1, JB6, JB8: a graceful stop (stdin's end of file, the portable
/// trigger) with a job running: streams get `shutdown`; the job, still
/// running when `jobs.drain_seconds` end, is queued again as its call
/// returns, its attempt unchanged; the process exits 0; the next start
/// runs it to the CLI's bytes on attempt 1.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_graceful_stop_queues_a_running_job_again() {
    let config = "[jobs]\ndrain_seconds = 1\n[test]\nhooks = { \"job.read\" = \"delay:3000\" }\n";
    let mut server = TestServer::process(Dirs::new(), config);
    let (bytes, golden) = p01();
    let sha = server.upload(&bytes).await;
    let job = server
        .send(Call::json(Method::POST, "/v1/features", &features(&sha)))
        .await
        .json();
    let id = job["id"].as_str().unwrap().to_owned();
    let token = server.token().to_owned();
    let mut stream = support::wire_open(
        server.addr(),
        &Call::get(&format!("/v1/jobs/{id}/events")).bearer(&token),
    )
    .await
    .into_body();
    let seen = support::lines(&mut stream, Duration::from_secs(30), |e| {
        e["state"] == "running"
    })
    .await;
    assert_eq!(seen.last().unwrap()["state"], "running");
    let child = server.child.take().unwrap();
    let rest = tokio::spawn(async move {
        support::lines(&mut stream, Duration::from_secs(30), |e| {
            e["type"] == "shutdown"
        })
        .await
    });
    let (code, _, stderr) = tokio::task::spawn_blocking(move || child.stop())
        .await
        .unwrap();
    assert_eq!(code, Some(0), "{stderr}");
    let rest = rest.await.unwrap();
    assert!(rest.iter().any(|e| e["type"] == "shutdown"), "{rest:?}");
    assert!(
        rest.iter().all(|e| e["type"] != "end"),
        "no end: it was stopped"
    );
    server.restart_process("");
    let job = server.finished_job(&id, Duration::from_secs(60)).await;
    assert_eq!(job["state"], "succeeded", "{job}");
    assert_eq!(job["attempt"], 1, "SD1: a drain consumes no attempt");
    assert_eq!(server.document(&job).await.body, golden);
    // Decided in PR 5's review: the drain's requeue is a persisted event.
    let token = server.token().to_owned();
    let mut replay = support::wire_open(
        server.addr(),
        &Call::get(&format!("/v1/jobs/{id}/events")).bearer(&token),
    )
    .await
    .into_body();
    let events = support::lines(&mut replay, Duration::from_secs(10), |e| e["type"] == "end").await;
    let states: Vec<&str> = events.iter().filter_map(|e| e["state"].as_str()).collect();
    assert_eq!(
        states[..4],
        ["queued", "running", "queued", "running"],
        "{states:?}"
    );
}

/// RA6, OB4, review S3 of PR 5: an assembly a job that has not ended
/// names cannot be deleted (`409 assembly_in_use`), so an extraction
/// queued on it runs on a registered assembly; once it ended and its line
/// is gone, the delete is `204`.
#[tokio::test]
async fn an_assembly_a_queued_job_names_stays() {
    let (server, sha) = registered().await;
    server.server().hold_jobs();
    let id = submit_extraction(&server, &sha).await;
    let path = format!("/v1/assemblies/sha256:{sha}");
    assert_eq!(
        server.send(Call::new(Method::DELETE, &path)).await.code(),
        "assembly_in_use"
    );
    server.server().resume_jobs();
    let job = server.finished_job(&id, Duration::from_secs(30)).await;
    assert_eq!(job["state"], "succeeded", "{job}");
    let line = lines(&server).await[0]["id"].as_str().unwrap().to_owned();
    server
        .send(Call::new(Method::DELETE, &format!("/v1/sockets/{line}")))
        .await;
    assert_eq!(
        server.send(Call::new(Method::DELETE, &path)).await.status,
        204
    );
}

/// UP7, OB3, SD4: a job reads each input checked against its name: an
/// input whose stored bytes changed under it fails the job `failed`,
/// `blob_damaged`, with no document and no verdict.
#[tokio::test]
async fn a_damaged_input_fails_its_job() {
    let server = TestServer::fresh();
    let (bytes, _) = p01();
    let sha = server.upload(&bytes).await;
    let path = server
        .dirs
        .state
        .join(format!("objects/sha256/{}/{sha}", &sha[..2]));
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    perms.set_readonly(false);
    std::fs::set_permissions(&path, perms).unwrap();
    let mut damaged = bytes.clone();
    damaged[10] ^= 1;
    std::fs::write(&path, damaged).unwrap();
    let job = server
        .send(waiting("/v1/features", &features(&sha)))
        .await
        .json();
    assert_eq!(job["state"], "failed", "{job}");
    assert_eq!(job["error"]["code"], "blob_damaged");
    assert!(job["document"].is_null());
    no_verdict(&job);
}

/// JS11, review S2 of PR 5: a submission whose handler is cut after its
/// commit (`job.submitted` delayed past a 1 s handler time: `503
/// request_timeout`) is still queued and run, since the commit and the
/// queueing run on their own task; and a queued row the dispatcher never
/// held (as a failed start or a lost submission leaves one) is taken up at
/// its next dispatch.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_committed_job_runs_though_its_answer_was_lost() {
    let dirs = Dirs::new();
    let mut config = dirs.config();
    config.server.handler_timeout_seconds = 1;
    let server = TestServer::in_process(dirs, config);
    let (bytes, golden) = p01();
    let sha = server.upload(&bytes).await;
    let hooks = &server.server().seams().hooks;
    hooks.set("job.submitted", Action::Delay(Duration::from_millis(2_000)));
    let cut = server
        .send(Call::json(Method::POST, "/v1/features", &features(&sha)))
        .await;
    assert_eq!(cut.code(), "request_timeout");
    hooks.clear("job.submitted");
    let listed = server.get("/v1/jobs").await.json();
    let id = listed["items"][0]["id"].as_str().unwrap().to_owned();
    let job = server.finished_job(&id, Duration::from_secs(30)).await;
    assert_eq!(job["state"], "succeeded", "{job}");
    assert_eq!(server.document(&job).await.body, golden);
    // A queued row of server.db's alone.
    let conn = rusqlite::Connection::open(server.dirs.state.join("server.db")).unwrap();
    let owner: i64 = conn
        .query_row("SELECT owner FROM job WHERE id = ?1", [&id], |r| r.get(0))
        .unwrap();
    let orphan = support::seed_job_input(&server, owner, &sha, "features", "queued");
    conn.execute(
        "UPDATE job SET request = CAST(?2 AS BLOB) WHERE id = ?1",
        rusqlite::params![
            orphan,
            json!({ "upload": format!("sha256:{sha}") }).to_string()
        ],
    )
    .unwrap();
    let next = server
        .send(waiting("/v1/features", &features(&sha)))
        .await
        .json();
    assert_eq!(next["state"], "succeeded");
    let taken = server.finished_job(&orphan, Duration::from_secs(30)).await;
    assert_eq!(taken["state"], "succeeded", "{taken}");
}

/// s02's BLOCK socket and its two committed candidates, uploaded: the
/// match's body, and the CLI's document for it.
async fn block_match(server: &TestServer) -> (Value, Vec<u8>) {
    let socket = server.socket_of("s02_cbore", "BLOCK").await;
    let mut files = Vec::new();
    for name in ["m_block02.stp", "m_block02_cb_deep.stp"] {
        let sha = server.upload(&support::candidate(name)).await;
        files.push(json!({ "upload": format!("sha256:{sha}"), "label": name }));
    }
    let body = json!({ "socket": socket, "candidates": { "files": files } });
    (body, support::cli_golden("match", "s02_cbore--BLOCK.json"))
}

/// VT9 for matches (JS6, SN7): a match that panics after its socket is
/// read, after its window is judged, or before its document is stored ends
/// `failed`, `internal`, with no document and no verdict; a judgment that
/// panics is caught as the CLI catches it: the run `failed`,
/// `run_incomplete`, its document kept without that candidate; the server
/// goes on, and the next match is the CLI's.
#[tokio::test]
async fn a_match_that_panics_fails_and_the_server_goes_on() {
    let server = TestServer::fresh();
    let (body, golden) = block_match(&server).await;
    let hooks = &server.server().seams().hooks;
    for point in ["job.read", "match.window", "job.ran", "job.stored"] {
        hooks.set(point, Action::Panic(format!("a planted panic at {point}")));
        let job = server.run_match(&body).await;
        hooks.clear(point);
        assert_eq!(
            (job["state"].as_str(), job["error"]["code"].as_str()),
            (Some("failed"), Some("internal")),
            "{point}: {job}"
        );
        assert!(job["document"].is_null(), "{point}");
        no_verdict(&job);
        let next = server.run_match(&body).await;
        assert_eq!(server.document(&next).await.body, golden, "{point}");
    }
    hooks.set(
        "match.judge",
        Action::Panic("a planted judgment's panic".into()),
    );
    let job = server.run_match(&body).await;
    hooks.clear("match.judge");
    assert_eq!(
        (
            job["state"].as_str(),
            job["exit_code"].as_u64(),
            job["error"]["code"].as_str()
        ),
        (Some("failed"), Some(1), Some("run_incomplete")),
        "{job}"
    );
    no_verdict(&job);
    let doc: Value = serde_json::from_slice(&server.document(&job).await.body).unwrap();
    assert_eq!(
        doc["summary"]["candidates"], 0,
        "no judgment that panicked is in it"
    );
    let codes: Vec<&str> = job["notes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["code"].as_str().unwrap())
        .collect();
    assert_eq!(codes.iter().filter(|c| **c == "judging_failed").count(), 2);
}

/// VT41 for matches (JB5, JS8, SN7): a queued match cancelled is
/// `cancelled` at once; a running one, cancelled while a window waits, is
/// `cancelling` and then `cancelled` before the next window: neither
/// leaves a document, a run's summary or an outcome.
#[tokio::test]
async fn a_cancelled_match_leaves_nothing() {
    let server = TestServer::fresh();
    let (body, _) = block_match(&server).await;
    server.server().hold_jobs();
    let queued = server
        .send(Call::json(Method::POST, "/v1/matches", &body))
        .await
        .json();
    let id = queued["id"].as_str().unwrap().to_owned();
    let cancel = server
        .send(Call::new(Method::POST, &format!("/v1/jobs/{id}/cancel")))
        .await;
    assert_eq!(cancel.json()["state"], "cancelled");
    server.server().resume_jobs();
    let gate = Gate::new();
    server
        .server()
        .seams()
        .hooks
        .set("match.window", Action::Pause(gate.clone()));
    let running = server
        .send(Call::json(Method::POST, "/v1/matches", &body))
        .await
        .json();
    let running = running["id"].as_str().unwrap().to_owned();
    assert!(gate.arrived(1, Duration::from_secs(30)).await);
    let cancel = server
        .send(Call::new(
            Method::POST,
            &format!("/v1/jobs/{running}/cancel"),
        ))
        .await;
    assert_eq!(cancel.json()["state"], "cancelling");
    gate.open();
    server.server().seams().hooks.clear("match.window");
    for id in [id, running] {
        let job = server.finished_job(&id, Duration::from_secs(60)).await;
        assert_eq!(job["state"], "cancelled", "{job}");
        assert!(job["document"].is_null() && job["result"].is_null() && job["outcome"].is_null());
        let run = server.get(&format!("/v1/matches/{id}")).await.json();
        assert!(run["document"].is_null() && run["summary"].is_null() && run["outcome"].is_null());
        assert_eq!(run["complete"], false);
        let document = server.get(&format!("/v1/matches/{id}/document")).await;
        assert_eq!(document.code(), "no_document");
    }
}

/// XH12.3 on every read of a match (VT44): `outcome` null in
/// `fitment.job`, `fitment.run`, the `end` event, `GET /v1/jobs` and
/// `GET /v1/matches`; a kept document served with `Fitment-Exit-Code: 1`,
/// its run `complete: false`, `summary` null, its counts under
/// `problems.incomplete_summary`, every page `complete: false`.
async fn states_no_verdict(server: &TestServer, id: &str, kept: bool) {
    let job = server.get(&format!("/v1/jobs/{id}")).await.json();
    assert_ne!(job["state"], "succeeded");
    assert!(job["outcome"].is_null(), "{job}");
    let run = server.get(&format!("/v1/matches/{id}")).await.json();
    assert!(
        run["outcome"].is_null() && run["summary"].is_null(),
        "{run}"
    );
    assert_eq!(run["complete"], false);
    let jobs = server.get("/v1/jobs?kind=match").await.json();
    let listed = jobs["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|j| j["id"] == id)
        .unwrap()
        .clone();
    assert!(listed["outcome"].is_null());
    let runs = server.get("/v1/matches").await.json();
    let listed = runs["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == id)
        .unwrap()
        .clone();
    assert!(listed["outcome"].is_null() && listed["summary"].is_null());
    let events = server
        .open(Call::get(&format!("/v1/jobs/{id}/events")))
        .await;
    let mut events = events.into_body();
    let lines = support::lines(&mut events, Duration::from_secs(30), |e| e["type"] == "end").await;
    let end = lines.last().unwrap();
    assert_eq!(end["type"], "end");
    assert!(end["outcome"].is_null(), "{end}");
    let document = server.get(&format!("/v1/matches/{id}/document")).await;
    if kept {
        assert_eq!(document.header("fitment-exit-code"), Some("1"));
        let doc: Value = serde_json::from_slice(&document.body).unwrap();
        assert_eq!(run["problems"]["incomplete_summary"], doc["summary"]);
        let page = server
            .get(&format!("/v1/matches/{id}/candidates"))
            .await
            .json();
        assert_eq!(
            (page["complete"].as_bool(), page["exit_code"].as_u64()),
            (Some(false), Some(1))
        );
    } else {
        assert_eq!(document.code(), "no_document");
        assert!(run["problems"]["incomplete_summary"].is_null());
    }
}

/// VT44, XH12, JB3, SN4, DC10, DC12, M6-59: a match driven to every end
/// but success states no verdict anywhere: refused at the request (no
/// job), refused at dispatch (the build's socket rules raised), failed by
/// a panicking judgment and by an original that fails its own socket (a
/// socket file edited and imported without its assembly, whose original
/// then fails the edited requirement), past its deadline, cancelled, and
/// interrupted (two attempts killed: in `a_killed_match_twice_is_interrupted`).
/// The control: a succeeded run states its outcome, and no
/// `incomplete_summary`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_job_that_did_not_succeed_states_a_verdict() {
    let server = TestServer::fresh();
    let (body, _) = block_match(&server).await;
    let good = server.run_match(&body).await;
    let run = server
        .get(&format!("/v1/matches/{}", good["id"].as_str().unwrap()))
        .await
        .json();
    assert!(good["outcome"].is_string() && run["outcome"].is_string());
    assert!(
        run["problems"]["incomplete_summary"].is_null(),
        "the control"
    );
    let mut unreviewed = body.clone();
    unreviewed["options"] = json!({ "require_reviewed": true });
    let refused = server
        .send(support::waiting("/v1/matches", &unreviewed))
        .await;
    assert_eq!(refused.status, 422, "refused at the request: no job");
    let seams = server.server().seams();
    // Refused at dispatch.
    server.server().hold_jobs();
    let queued = server
        .send(Call::json(Method::POST, "/v1/matches", &body))
        .await
        .json();
    server.server().set_rules_shift(1);
    server.server().resume_jobs();
    let id = queued["id"].as_str().unwrap().to_owned();
    assert_eq!(
        server.finished_job(&id, Duration::from_secs(60)).await["state"],
        "refused"
    );
    server.server().set_rules_shift(0);
    states_no_verdict(&server, &id, false).await;
    // A panicking judgment.
    seams.hooks.set(
        "match.judge",
        Action::Panic("a planted judgment's panic".into()),
    );
    let failed = server.run_match(&body).await;
    seams.hooks.clear("match.judge");
    states_no_verdict(&server, failed["id"].as_str().unwrap(), true).await;
    // Past its deadline.
    seams.jobs.set_deadline(Duration::from_millis(100));
    seams
        .hooks
        .set("match.window", Action::Delay(Duration::from_millis(400)));
    let late = server.run_match(&body).await;
    seams.hooks.clear("match.window");
    seams.jobs.set_deadline(Duration::from_secs(3600));
    assert_eq!(late["error"]["code"], "deadline_exceeded");
    states_no_verdict(&server, late["id"].as_str().unwrap(), false).await;
    // Cancelled.
    server.server().hold_jobs();
    let queued = server
        .send(Call::json(Method::POST, "/v1/matches", &body))
        .await
        .json();
    let id = queued["id"].as_str().unwrap().to_owned();
    server
        .send(Call::new(Method::POST, &format!("/v1/jobs/{id}/cancel")))
        .await;
    server.server().resume_jobs();
    states_no_verdict(&server, &id, false).await;
    // The original failing its own socket.
    let original = original_failing(&server).await;
    states_no_verdict(&server, original["id"].as_str().unwrap(), true).await;
}

/// VT44's `interrupted` end, JB6, SD5: a match whose process is killed
/// on both attempts ends `interrupted`, `job_interrupted`, with no
/// document, and no verdict in `fitment.job`, `fitment.run` or the lists.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_killed_match_twice_is_interrupted() {
    const RESTART: &str = "[test]\nlease_stale_ms = 1\n";
    let mut server = TestServer::process(Dirs::new(), RESTART);
    let sha = server.upload(&support::assembly("s02_cbore.stp")).await;
    let call = abort_setup(&server, "match", &sha).await;
    let hooks = format!("{RESTART}hooks = {{ \"job.read\" = \"abort\" }}\n");
    server.restart_process(&hooks);
    assert!(server.try_send(call).await.is_none());
    server.restart_process(&hooks);
    let mut tries = 0;
    while server.try_send(Call::get("/v1/health")).await.is_some() {
        tries += 1;
        assert!(tries < 600, "the second attempt never aborted");
        let _ = server.try_send(Call::get("/v1/jobs?wait=1")).await;
    }
    server.restart_process(RESTART);
    let listed = server.get("/v1/jobs?kind=match").await.json();
    let job = &listed["items"][0];
    assert_eq!(
        (
            job["state"].as_str(),
            job["error"]["code"].as_str(),
            job["attempt"].as_u64()
        ),
        (Some("interrupted"), Some("job_interrupted"), Some(2)),
        "{job}"
    );
    assert!(job["outcome"].is_null() && job["document"].is_null());
    let id = job["id"].as_str().unwrap();
    let run = server.get(&format!("/v1/matches/{id}")).await.json();
    assert!(run["outcome"].is_null() && run["summary"].is_null() && run["document"].is_null());
    let runs = server.get("/v1/matches").await.json();
    assert!(runs["items"][0]["outcome"].is_null());
}

/// A match whose original fails its own socket (VD15): the CLI's s01
/// BRACKET socket file with one bore's shaft widened past the original's
/// hole and its digest recomputed, imported without its assembly
/// (unverified, so nothing reproduces it), then the assembly registered
/// so the original can be read.
async fn original_failing(server: &TestServer) -> Value {
    let mut edited: Value =
        serde_json::from_slice(&support::cli_golden("socket", "s01_bolted--BRACKET.json")).unwrap();
    let requirements = edited["extracted"]["sockets"][0]["requirements"]
        .as_array_mut()
        .unwrap();
    let bore = requirements
        .iter_mut()
        .find(|r| r["id"].as_str().unwrap().starts_with("bore:"))
        .unwrap();
    // A bore whose shaft no hole of the original clears.
    bore["requires"]["spans"][0]["hole_diameter_mm"] = json!(7.5);
    bore["requires"]["spans"][0]["clear_diameter_mm"] = json!(7.0);
    let mut stated = edited.clone();
    for key in ["digest", "review", "stale_reviews"] {
        stated.as_object_mut().unwrap().remove(key);
    }
    edited["digest"] = json!(format!(
        "sha256:{}",
        fitment_core::sha256_hex(stated.to_string().as_bytes())
    ));
    let file = serde_json::to_vec_pretty(&edited).unwrap();
    let upload = server.upload(&file).await;
    let imported = server
        .send(support::waiting(
            "/v1/sockets/import",
            &json!({ "upload": format!("sha256:{upload}") }),
        ))
        .await
        .json();
    assert_eq!(
        imported["state"], "succeeded",
        "unverified, nothing parsed but the file: {imported}"
    );
    let socket = imported["result"]["href"]
        .as_str()
        .unwrap()
        .trim_start_matches("/v1/sockets/")
        .to_owned();
    server
        .register(&support::assembly("s01_bolted.stp"), "s01_bolted.stp")
        .await;
    let candidate = server.upload(&support::candidate("m_bracket.stp")).await;
    let original = server
        .run_match(
            &json!({ "socket": socket, "original": true, "candidates": { "files": [
            { "upload": format!("sha256:{candidate}"), "label": "m_bracket.stp" } ] } }),
        )
        .await;
    assert_eq!(
        (
            original["state"].as_str(),
            original["error"]["code"].as_str()
        ),
        (Some("failed"), Some("original_fails_own_socket")),
        "{original}"
    );
    original
}
