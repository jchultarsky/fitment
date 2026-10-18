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

/// `table` of the server's `server.db` renamed `to`, from outside the
/// server, so the queries naming it fail as a broken database's would.
fn rename_table(server: &TestServer, table: &str, to: &str) {
    let conn = rusqlite::Connection::open(server.dirs.state.join("server.db")).unwrap();
    conn.execute_batch(&format!("ALTER TABLE {table} RENAME TO {to}"))
        .unwrap();
}

/// ER3 (review of M6 PR 6): a failure of `server.db` is `500 io_error`,
/// never read as a missing row: registering an assembly, importing a
/// socket of an assembly the caller holds and asking for a match's
/// original all read the caller's assembly, and with its names' table
/// gone each answers `io_error`, not `404`, an unverified import or
/// `original_not_stored`; a match whose socket's revision cannot be read
/// when its run is stored fails `internal`, not a run that is silently
/// verified.
#[tokio::test]
async fn a_failing_database_is_never_read_as_a_missing_row() {
    let server = TestServer::fresh();
    let socket = server.socket_of("s01_bolted", "BRACKET").await;
    let assembly = support::assembly("s01_bolted.stp");
    let assembly_sha = server.upload(&assembly).await;
    let document = server
        .upload(&support::cli_golden("socket", "s01_bolted--BRACKET.json"))
        .await;
    let candidate = server.upload(&support::candidate("m_bracket.stp")).await;
    let files = json!([{ "upload": format!("sha256:{candidate}"), "label": "m_bracket.stp" }]);
    rename_table(&server, "assembly_name", "assembly_name_gone");
    let calls = [
        support::waiting(
            "/v1/assemblies",
            &json!({ "upload": format!("sha256:{assembly_sha}") }),
        ),
        support::waiting(
            "/v1/sockets/import",
            &json!({ "upload": format!("sha256:{document}") }),
        ),
        support::waiting(
            "/v1/matches",
            &json!({ "socket": socket, "original": true, "candidates": { "files": files } }),
        ),
    ];
    for call in calls {
        let reply = server.send(call).await;
        assert_eq!(
            (reply.status.as_u16(), reply.code()),
            (500, "io_error".to_owned()),
            "{}",
            String::from_utf8_lossy(&reply.body)
        );
    }
    rename_table(&server, "assembly_name_gone", "assembly_name");
    let gate = Gate::new();
    let hooks = &server.server().seams().hooks;
    hooks.set("job.ran", Action::Pause(gate.clone()));
    let body = json!({ "socket": socket, "candidates": { "files": files } });
    let queued = server
        .send(Call::json(Method::POST, "/v1/matches", &body))
        .await
        .json();
    let id = queued["id"].as_str().unwrap().to_owned();
    assert!(gate.arrived(1, Duration::from_secs(30)).await);
    rename_table(&server, "socket_revision", "socket_revision_gone");
    gate.open();
    hooks.clear("job.ran");
    let job = server.finished_job(&id, Duration::from_secs(60)).await;
    rename_table(&server, "socket_revision_gone", "socket_revision");
    assert_eq!(
        (job["state"].as_str(), job["error"]["code"].as_str()),
        (Some("failed"), Some("internal")),
        "{job}"
    );
}

/// JS11, OB4 (review of M6 PR 6): what a request resolved and the job's
/// insert leave no window for a foreign key's `500`: an extraction with a
/// `carry_from` resolves its assembly, then waits in its parse slot while
/// the assembly is deleted and the sweep unlinks its bytes; the insert
/// finds the assembly gone and answers as the request would have, `404
/// not_found`, nothing queued.
#[tokio::test]
async fn a_reference_swept_while_its_request_waits_is_not_found() {
    let server = Arc::new(TestServer::fresh());
    let assembly = support::assembly("s01_bolted.stp");
    let sha = fitment_core::sha256_hex(&assembly);
    let registered = server.register(&assembly, "s01_bolted.stp").await;
    assert_eq!(registered["state"], "succeeded", "{registered}");
    let seams = server.server().seams();
    let start = seams.ids.now_ms();
    seams.ids.fix_time(start + 20 * 3_600_000);
    let carry = server
        .upload(&support::cli_golden("socket", "s01_bolted--BRACKET.json"))
        .await;
    let gate = Gate::new();
    seams.hooks.set("socket.parse", Action::Pause(gate.clone()));
    let extraction = {
        let server = Arc::clone(&server);
        let body = json!({ "assembly": format!("sha256:{sha}"), "part": "BRACKET",
                           "carry_from": { "upload": format!("sha256:{carry}") } });
        tokio::spawn(async move {
            server
                .send(Call::json(Method::POST, "/v1/sockets", &body))
                .await
        })
    };
    assert!(gate.arrived(1, Duration::from_secs(30)).await);
    let deleted = server
        .send(Call::new(
            Method::DELETE,
            &format!("/v1/assemblies/sha256:{sha}"),
        ))
        .await;
    assert_eq!(deleted.status, 204);
    seams.ids.fix_time(start + 25 * 3_600_000);
    server.server().sweep_store().unwrap();
    let swept = !server.object_files().contains(&sha);
    gate.open();
    seams.hooks.clear("socket.parse");
    let reply = extraction.await.unwrap();
    assert!(swept, "the assembly's bytes were swept while it waited");
    assert_eq!(
        (reply.status.as_u16(), reply.code()),
        (404, "not_found".to_owned()),
        "{}",
        String::from_utf8_lossy(&reply.body)
    );
    assert_eq!(reply.json()["details"]["resource"], "assembly");
    let jobs = server.get("/v1/jobs").await.json();
    assert_eq!(
        jobs["items"].as_array().unwrap().len(),
        1,
        "only the registration's job: nothing queued"
    );
}

// ------------------------------------------------------------- catalog

/// XH12 for a catalog job that ended: one terminal state, which a reread
/// keeps; no verdict unless it succeeded; and the catalog consistent
/// (`fitment catalog verify` clean), however it ended.
async fn catalog_xh12(server: &TestServer, job: &Value) {
    let id = job["id"].as_str().unwrap();
    let again = server.get(&format!("/v1/jobs/{id}")).await.json();
    assert_eq!(again["state"], job["state"], "XH12: one terminal state");
    assert!(
        !["queued", "running", "cancelling"].contains(&job["state"].as_str().unwrap()),
        "{job}"
    );
    no_verdict(job);
    if job["state"] != "succeeded" {
        assert!(job["outcome"].is_null(), "{job}");
    }
    assert_eq!(
        support::verified(&server.dirs.catalog),
        Some(0),
        "XH12: {job}"
    );
}

/// The persisted events of job `id`, by type.
async fn events_of(server: &TestServer, id: &str) -> Vec<Value> {
    let body = server
        .open(Call::get(&format!("/v1/jobs/{id}/events")))
        .await;
    let mut body = body.into_body();
    support::lines(&mut body, Duration::from_secs(30), |e| e["type"] == "end").await
}

/// Cancels job `id` as the harness's admin: the answer.
async fn cancel(server: &TestServer, id: &str) -> support::Reply {
    server
        .send(Call::new(Method::POST, &format!("/v1/jobs/{id}/cancel")))
        .await
}

/// JS8, JB5, JB8, VT41, XH12: a cancel reaches each catalog kind where JS8
/// says it may stop. An ingest of 70 files stopped after its first batch
/// (CA2's check before each file) keeps that batch, its document listing
/// exactly those lines, `exit_code` null, its `change` and `batch` events
/// persisted; a reindex likewise; a gc asked before it began unlinks
/// nothing and says so; a verify and a catalog match leave nothing; a
/// running replace or prune is one transaction, `409 job_finished`, which
/// says it runs to its end, and ends as it ran; a queued job is
/// `cancelled` at once. The first batch's size is what its `batch` event
/// says: at most 64 files, fewer when DB3's second ends it under load (the
/// review of PR 7b).
#[tokio::test(flavor = "multi_thread")]
async fn catalog_jobs_stop_where_js8_says() {
    let server = TestServer::fresh();
    let seams = server.server().seams();
    let files = support::junk_files(70);
    let mut listed = Vec::new();
    for (bytes, name) in &files {
        let sha = server.upload(bytes).await;
        listed.push(json!({ "upload": format!("sha256:{sha}"), "file_name": name }));
    }
    let gate = Gate::new();
    seams
        .hooks
        .set("catalog.batch", Action::Pause(gate.clone()));
    let job = server
        .submit("/v1/catalog/entries", &json!({ "files": listed }))
        .await;
    let id = job["id"].as_str().unwrap().to_owned();
    assert!(gate.arrived(1, Duration::from_secs(60)).await);
    assert_eq!(cancel(&server, &id).await.status, 202);
    gate.open();
    seams.hooks.clear("catalog.batch");
    let ended = server.finished_job(&id, Duration::from_secs(60)).await;
    assert_eq!(ended["state"], "cancelled", "{ended}");
    assert!(ended["exit_code"].is_null());
    let events = events_of(&server, &id).await;
    let changes = events.iter().filter(|e| e["type"] == "change").count();
    let batches: Vec<&Value> = events.iter().filter(|e| e["type"] == "batch").collect();
    assert_eq!(batches.len(), 1, "{batches:?}");
    let first = usize::try_from(batches[0]["lines"].as_u64().unwrap()).unwrap();
    assert!((1..=64).contains(&first), "{first}");
    assert_eq!(changes, first);
    let document: Value = serde_json::from_slice(&server.document(&ended).await.body).unwrap();
    assert_eq!(
        document["changes"].as_array().unwrap().len(),
        first,
        "its committed batch"
    );
    assert_eq!(document["counts"]["entries"], first);
    catalog_xh12(&server, &ended).await;
    // A reindex of those and 70 more, stale, stopped after its first
    // batch: more than one batch's 64, so a second was still to come (a
    // cancel after the last batch stops nothing, the review of PR 7b).
    let more: Vec<(Vec<u8>, String)> = (0..70)
        .map(|n| {
            (
                format!("more {n:04}\n").into_bytes(),
                format!("more{n:04}.stp"),
            )
        })
        .collect();
    server.ingest(&more).await;
    support::stale_catalog(&server.dirs.catalog, fitment_core::EXTRACTOR_VERSION - 1);
    let reindexed = stopped_at_batch(&server, "/v1/catalog/reindex", &json!({ "all": true })).await;
    assert_eq!(reindexed["state"], "cancelled", "{reindexed}");
    let document: Value = serde_json::from_slice(&server.document(&reindexed).await.body).unwrap();
    let changes = document["changes"].as_array().unwrap();
    assert!(
        (1..first + 70).contains(&changes.len()),
        "its committed batch's lines: {}",
        changes.len()
    );
    catalog_xh12(&server, &reindexed).await;
    for (path, body, kept) in [
        ("/v1/catalog/gc", json!({}), true),
        ("/v1/catalog/verify", json!({ "deep": true }), false),
    ] {
        let ended = stopped_at_read(&server, path, &body).await;
        assert_eq!(ended["state"], "cancelled", "{path}: {ended}");
        assert_eq!(ended["document"].is_null(), !kept, "{path}: {ended}");
        catalog_xh12(&server, &ended).await;
    }
    let p01 = server.upload(&support::part("p01_through.stp")).await;
    let replace = json!({ "upload": format!("sha256:{p01}"), "file_name": "p01_through.stp" });
    for (method, path, body) in [
        (Method::PUT, "/v1/catalog/entries/junk0000", replace),
        (
            Method::POST,
            "/v1/catalog/prune",
            json!({ "revisions": true }),
        ),
    ] {
        let gate = Gate::new();
        seams.hooks.set("job.read", Action::Pause(gate.clone()));
        let job = server.send(Call::json(method, path, &body)).await.json();
        let id = job["id"].as_str().unwrap().to_owned();
        assert!(gate.arrived(1, Duration::from_secs(30)).await);
        let refused = cancel(&server, &id).await;
        assert_eq!(refused.code(), "job_finished", "{path}: one transaction");
        let detail = refused.json()["detail"].as_str().unwrap().to_owned();
        assert!(
            detail.contains("cannot be stopped once running"),
            "{path}: not ended, running to its end: {detail}"
        );
        gate.open();
        seams.hooks.clear("job.read");
        let ended = server.finished_job(&id, Duration::from_secs(60)).await;
        assert_eq!(ended["state"], "succeeded", "{path}: {ended}");
        catalog_xh12(&server, &ended).await;
    }
    server.server().hold_jobs();
    let queued = server.submit("/v1/catalog/gc", &json!({})).await;
    let id = queued["id"].as_str().unwrap();
    assert_eq!(cancel(&server, id).await.json()["state"], "cancelled");
    server.server().resume_jobs();
}

/// The job at `path`, its flag set while it waits after its first batch:
/// the job as it ended.
async fn stopped_at_batch(server: &TestServer, path: &str, body: &Value) -> Value {
    let seams = server.server().seams();
    let gate = Gate::new();
    seams
        .hooks
        .set("catalog.batch", Action::Pause(gate.clone()));
    let job = server.submit(path, body).await;
    let id = job["id"].as_str().unwrap().to_owned();
    assert!(gate.arrived(1, Duration::from_secs(60)).await);
    assert_eq!(cancel(server, &id).await.status, 202);
    gate.open();
    seams.hooks.clear("catalog.batch");
    server.finished_job(&id, Duration::from_secs(60)).await
}

/// The job at `path`, its flag set before its first catalog call.
async fn stopped_at_read(server: &TestServer, path: &str, body: &Value) -> Value {
    let seams = server.server().seams();
    let gate = Gate::new();
    seams.hooks.set("job.read", Action::Pause(gate.clone()));
    let job = server.submit(path, body).await;
    let id = job["id"].as_str().unwrap().to_owned();
    assert!(gate.arrived(1, Duration::from_secs(30)).await);
    assert_eq!(cancel(server, &id).await.status, 202);
    gate.open();
    seams.hooks.clear("job.read");
    server.finished_job(&id, Duration::from_secs(60)).await
}

/// JS8, SN7, VT41: a catalog match cancelled while it judges (CA2: before
/// each group a judging thread takes) leaves no document and no run's
/// facts; one stopped by its deadline likewise, `failed`,
/// `deadline_exceeded`.
#[tokio::test(flavor = "multi_thread")]
async fn a_cancelled_catalog_match_leaves_nothing() {
    let server = TestServer::fresh();
    let files: Vec<(Vec<u8>, String)> = ["m_block02.stp", "m_block02_cb_deep.stp", "m_bracket.stp"]
        .iter()
        .map(|n| (support::candidate(n), (*n).to_owned()))
        .collect();
    server.ingest(&files).await;
    let socket = server.socket_of("s02_cbore", "BLOCK").await;
    let seams = server.server().seams();
    let gate = Gate::new();
    seams
        .hooks
        .set("catalog.judge", Action::Pause(gate.clone()));
    let body =
        json!({ "socket": socket, "candidates": { "catalog": { "selection": "exhaustive" } } });
    let job = server.submit("/v1/matches", &body).await;
    let id = job["id"].as_str().unwrap().to_owned();
    assert!(gate.arrived(1, Duration::from_secs(60)).await);
    assert_eq!(cancel(&server, &id).await.status, 202);
    gate.open();
    seams.hooks.clear("catalog.judge");
    let ended = server.finished_job(&id, Duration::from_secs(60)).await;
    assert_eq!(ended["state"], "cancelled", "{ended}");
    states_no_verdict(&server, &id, false).await;
    let run = server.get(&format!("/v1/matches/{id}")).await.json();
    assert!(run["candidates"]["generation"].is_null(), "{run}");
    seams.jobs.set_deadline(Duration::from_millis(200));
    seams
        .hooks
        .set("catalog.judge", Action::Delay(Duration::from_millis(400)));
    let late = server.run_match(&body).await;
    assert_eq!(
        (late["state"].as_str(), late["error"]["code"].as_str()),
        (Some("failed"), Some("deadline_exceeded")),
        "{late}"
    );
    assert!(late["document"].is_null());
}

/// CX3, CX4, M6-50, VT12: a connection holding the catalog's write lock
/// (`BEGIN IMMEDIATE`, as a CLI's ingest or a backup tool may): an ingest
/// job waits `catalog.write_busy_ms`, then fails `catalog_busy`,
/// retryable, changing nothing; a synchronous delete waits
/// `catalog.delete_busy_ms`, then answers `503 catalog_busy`; readers
/// under WAL go on: a catalog match runs and lists read. Once the lock is
/// released, the ingest submitted again and the delete succeed.
#[tokio::test(flavor = "multi_thread")]
async fn a_busy_catalog_is_waited_for_then_refused() {
    let dirs = Dirs::new();
    let mut config = dirs.config();
    config.catalog.write_busy_ms = 300;
    config.catalog.delete_busy_ms = 300;
    let server = TestServer::in_process(dirs, config);
    let first = [(
        support::candidate("m_block02.stp"),
        "m_block02.stp".to_owned(),
    )];
    server.ingest(&first).await;
    let socket = server.socket_of("s02_cbore", "BLOCK").await;
    let generation = server.get("/v1/catalog").await.json()["generation"].clone();
    let holder = rusqlite::Connection::open(server.dirs.catalog.join("catalog.db")).unwrap();
    holder.execute_batch("BEGIN IMMEDIATE").unwrap();
    let more = [(
        support::candidate("m_block02_cb_deep.stp"),
        "deep.stp".to_owned(),
    )];
    let busy = server.ingest(&more).await;
    assert_eq!(
        (busy["state"].as_str(), busy["error"]["code"].as_str()),
        (Some("failed"), Some("catalog_busy")),
        "{busy}"
    );
    assert_eq!(busy["error"]["retryable"], true);
    let deleted = server
        .send(Call::new(Method::DELETE, "/v1/catalog/entries/m_block02"))
        .await;
    assert_eq!(
        (deleted.status.as_u16(), deleted.code()),
        (503, "catalog_busy".into())
    );
    assert!(deleted.header("retry-after").is_some());
    let matched = server
        .run_match(&json!({ "socket": socket, "candidates": { "catalog": {} } }))
        .await;
    assert_eq!(matched["state"], "succeeded", "readers go on: {matched}");
    let info = server.get("/v1/catalog").await.json();
    assert_eq!(info["generation"], generation, "nothing changed");
    holder.execute_batch("ROLLBACK").unwrap();
    drop(holder);
    let again = server.ingest(&more).await;
    assert_eq!(again["state"], "succeeded", "{again}");
    let deleted = server
        .send(Call::new(Method::DELETE, "/v1/catalog/entries/m_block02"))
        .await;
    assert_eq!(deleted.status, 200);
    catalog_xh12(&server, &again).await;
}

/// CX6 (VT45): the WAL barrier. With `catalog.wal_barrier_mib` 0 and a
/// catalog match holding its snapshot (readers pinning the log), a
/// write's batch raises the barrier: query dispatch stops, the running
/// match is given `catalog.barrier_max_seconds`, then dispatch resumes
/// without the truncate, `fitment.info`'s `wal_barrier` set, and a gc
/// with `vacuum`, which needs the barrier, fails `catalog_busy`,
/// retryable. Once the match ends, the next write's barrier is taken:
/// the log is truncated and the flag cleared.
#[tokio::test(flavor = "multi_thread")]
async fn the_wal_barrier_is_bounded() {
    let dirs = Dirs::new();
    let mut config = dirs.config();
    config.catalog.wal_barrier_mib = 0;
    config.catalog.barrier_max_seconds = 1;
    let server = TestServer::in_process(dirs, config);
    let files: Vec<(Vec<u8>, String)> = ["m_block02.stp", "m_block02_cb_deep.stp"]
        .iter()
        .map(|n| (support::candidate(n), (*n).to_owned()))
        .collect();
    server.ingest(&files).await;
    let socket = server.socket_of("s02_cbore", "BLOCK").await;
    let seams = server.server().seams();
    let gate = Gate::new();
    seams
        .hooks
        .set("catalog.judge", Action::Pause(gate.clone()));
    let body =
        json!({ "socket": socket, "candidates": { "catalog": { "selection": "exhaustive" } } });
    let held = server.submit("/v1/matches", &body).await;
    assert!(gate.arrived(1, Duration::from_secs(60)).await);
    let started = std::time::Instant::now();
    let write = server.ingest(&support::junk_files(1)).await;
    assert_eq!(write["exit_code"], 3, "{write}");
    assert!(started.elapsed() < Duration::from_secs(10), "bounded");
    let info = server.get("/v1/info").await.json();
    assert_eq!(info["catalog"]["state"]["wal_barrier"], true, "{info}");
    let vacuum = server
        .submit("/v1/catalog/gc", &json!({ "vacuum": true }))
        .await;
    let vacuum = server
        .finished_job(vacuum["id"].as_str().unwrap(), Duration::from_secs(60))
        .await;
    assert_eq!(
        (vacuum["state"].as_str(), vacuum["error"]["code"].as_str()),
        (Some("failed"), Some("catalog_busy")),
        "{vacuum}"
    );
    assert_eq!(vacuum["error"]["retryable"], true);
    // The barrier is taken around `VACUUM` alone (the review of PR 7b):
    // what the gc collected before it stands, its document kept.
    assert!(!vacuum["document"].is_null(), "{vacuum}");
    gate.open();
    seams.hooks.clear("catalog.judge");
    let held = server
        .finished_job(held["id"].as_str().unwrap(), Duration::from_secs(60))
        .await;
    assert_eq!(held["state"], "succeeded", "{held}");
    let vacuum = server
        .submit("/v1/catalog/gc", &json!({ "vacuum": true }))
        .await;
    let vacuum = server
        .finished_job(vacuum["id"].as_str().unwrap(), Duration::from_secs(60))
        .await;
    assert_eq!(vacuum["state"], "succeeded", "{vacuum}");
    assert!(!seams.catalog.barrier(), "the barrier lowered");
    catalog_xh12(&server, &vacuum).await;
}

/// VT10, JB6, SD4, XH12, JB9: the built binary aborted after an ingest's
/// first committed batch (`catalog.batch`), then started again: the job
/// is `interrupted`, `job_interrupted`, retryable, with no document and
/// the catalog's generation when it started and at recovery; its batch
/// stays (RX7), the catalog consistent; its key is released, and the
/// same request submitted again does exactly what is missing: the
/// catalog then is an uninterrupted ingest's (its entries equal).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_aborted_ingest_is_interrupted_and_converges() {
    const RESTART: &str = "[test]\nlease_stale_ms = 1\n";
    let mut server = TestServer::process(Dirs::new(), RESTART);
    let files = support::junk_files(70);
    let mut listed = Vec::new();
    for (bytes, name) in &files {
        let sha = server.upload(bytes).await;
        listed.push(json!({ "upload": format!("sha256:{sha}"), "file_name": name }));
    }
    let body = json!({ "files": listed });
    let call = support::waiting("/v1/catalog/entries", &body).header("idempotency-key", "ingest");
    let hooks = format!("{RESTART}hooks = {{ \"catalog.batch\" = \"abort\" }}\n");
    server.restart_process(&hooks);
    assert!(
        server.try_send(call.clone()).await.is_none(),
        "the abort answered"
    );
    server.restart_process(RESTART);
    let jobs = server.get("/v1/jobs?kind=catalog_ingest").await.json();
    let job = jobs["items"][0].clone();
    assert_eq!(
        (job["state"].as_str(), job["error"]["code"].as_str()),
        (Some("interrupted"), Some("job_interrupted")),
        "{job}"
    );
    assert_eq!(job["error"]["retryable"], true);
    assert!(job["document"].is_null() && job["outcome"].is_null());
    let details = &job["error"]["details"];
    assert!(details["generation_at_start"].is_i64(), "{details}");
    assert!(
        details["generation_at_recovery"].as_i64() > details["generation_at_start"].as_i64(),
        "{details}"
    );
    // Its first batch, however DB3 cut it (the review of PR 7b): what its
    // persisted `batch` event says.
    let id = job["id"].as_str().unwrap();
    let events = server.get(&format!("/v1/jobs/{id}/events")).await;
    let batch = String::from_utf8_lossy(&events.body)
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .find(|e| e["type"] == "batch")
        .expect("its committed batch's event");
    let listed_now = server.get("/v1/catalog").await.json();
    assert_eq!(
        listed_now["entries"], batch["lines"],
        "its committed batch stays"
    );
    assert_eq!(support::verified(&server.dirs.catalog), Some(0));
    let again = server.send(call).await.json();
    assert_ne!(again["id"], job["id"], "the key was released");
    let again = server
        .finished_job(again["id"].as_str().unwrap(), Duration::from_secs(120))
        .await;
    assert_eq!(again["exit_code"], 3, "{again}");
    let reference = TestServer::fresh();
    reference.ingest(&files).await;
    let entries = |reply: support::Reply| reply.json()["entries"].clone();
    assert_eq!(
        entries(server.get("/v1/catalog/entries?limit=all").await),
        entries(reference.get("/v1/catalog/entries?limit=all").await),
        "an uninterrupted ingest's catalog"
    );
}

/// SD1, JB6, JB5, XH12: a graceful stop while an ingest runs: given
/// `jobs.drain_seconds`, its flag is set; the ingest stops before its next
/// file (CA2) and is `interrupted`, `job_interrupted`, keeping its
/// committed batch's document and stating the generation it started at
/// (a write job is never queued again); the process exits 0, the catalog
/// consistent.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_drained_ingest_is_interrupted_with_its_committed_lines() {
    let config =
        "[jobs]\ndrain_seconds = 1\n[test]\nhooks = { \"catalog.batch\" = \"delay:3000\" }\n";
    let mut server = TestServer::process(Dirs::new(), config);
    let files = support::junk_files(70);
    let mut listed = Vec::new();
    for (bytes, name) in &files {
        let sha = server.upload(bytes).await;
        listed.push(json!({ "upload": format!("sha256:{sha}"), "file_name": name }));
    }
    let job = server
        .send(Call::json(
            Method::POST,
            "/v1/catalog/entries",
            &json!({ "files": listed }),
        ))
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
    let seen = support::lines(&mut stream, Duration::from_secs(60), |e| {
        e["type"] == "batch"
    })
    .await;
    // Its first batch, however DB3 cut it (the review of PR 7b).
    let first = seen.last().unwrap()["lines"].clone();
    let at_batch = seen.last().unwrap()["generation"].clone();
    assert!(
        first.as_u64().is_some_and(|n| (1..=64).contains(&n)),
        "{seen:?}"
    );
    let child = server.child.take().unwrap();
    let (code, _, stderr) = tokio::task::spawn_blocking(move || child.stop())
        .await
        .unwrap();
    assert_eq!(code, Some(0), "{stderr}");
    server.restart_process("");
    let job = server.get(&format!("/v1/jobs/{id}")).await.json();
    assert_eq!(
        (job["state"].as_str(), job["error"]["code"].as_str()),
        (Some("interrupted"), Some("job_interrupted")),
        "{job}"
    );
    let details = &job["error"]["details"];
    assert!(details["generation_at_start"].is_i64(), "{job}");
    assert_eq!(
        details["generation_at_stop"], at_batch,
        "where its committed batch left the catalog: {job}"
    );
    assert!(
        details["generation_at_stop"].as_i64() > details["generation_at_start"].as_i64(),
        "{job}"
    );
    let document: Value = serde_json::from_slice(&server.document(&job).await.body).unwrap();
    assert_eq!(
        Value::from(document["changes"].as_array().unwrap().len()),
        first
    );
    assert_eq!(server.get("/v1/catalog").await.json()["entries"], first);
    catalog_xh12(&server, &job).await;
}

// ------------------------------------------------- the review of PR 7b

/// `m_block02`, `m_block02_cb_deep` and `m_bracket`, as files to ingest.
fn three_parts() -> Vec<(Vec<u8>, String)> {
    ["m_block02.stp", "m_block02_cb_deep.stp", "m_bracket.stp"]
        .iter()
        .map(|n| (support::candidate(n), (*n).to_owned()))
        .collect()
}

/// Job `id` as it is now, once it has left `queued`, within 30 s.
async fn left_the_queue(server: &TestServer, id: &str) -> Value {
    for _ in 0..1500 {
        let job = server.get(&format!("/v1/jobs/{id}")).await.json();
        if job["state"] != "queued" {
            return job;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("{id} stayed queued");
}

/// ST11, CC4: a step read racing a delete. The read's snapshot named the
/// bytes; a delete released them before they were read: the file missing,
/// the read looks again in a fresh snapshot and answers `410
/// bytes_not_kept`, never `500 blob_damaged`. Eight reads beside a delete
/// each answer the bytes or `410`.
#[tokio::test(flavor = "multi_thread")]
async fn a_step_racing_a_delete_is_gone_never_damaged() {
    let server = Arc::new(TestServer::fresh());
    server.ingest(&three_parts()).await;
    let seams = server.server().seams();
    let gate = Gate::new();
    seams.hooks.set("catalog.step", Action::Pause(gate.clone()));
    let reading = Arc::clone(&server);
    let held = tokio::spawn(async move { reading.get("/v1/catalog/entries/m_block02/step").await });
    assert!(gate.arrived(1, Duration::from_secs(30)).await);
    let deleted = server
        .send(Call::new(Method::DELETE, "/v1/catalog/entries/m_block02"))
        .await;
    assert_eq!(deleted.status, 200);
    gate.open();
    seams.hooks.clear("catalog.step");
    let reply = held.await.unwrap();
    assert_eq!(
        (reply.status.as_u16(), reply.code()),
        (410, "bytes_not_kept".into())
    );
    let mut reads = Vec::new();
    for _ in 0..8 {
        let reading = Arc::clone(&server);
        reads.push(tokio::spawn(async move {
            reading.get("/v1/catalog/entries/m_bracket/step").await
        }));
    }
    let deleted = server
        .send(Call::new(Method::DELETE, "/v1/catalog/entries/m_bracket"))
        .await;
    assert_eq!(deleted.status, 200);
    for read in reads {
        let reply = read.await.unwrap();
        assert!(
            matches!(reply.status.as_u16(), 200 | 410),
            "{}: {}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
    }
}

/// CX6, M6-50: a gc truncates the log on a reader, whose wait for readers
/// is `catalog.read_busy_ms`, never on its writer, so a match pinning the
/// log holds no write lock for `catalog.write_busy_ms`: a delete during
/// the gc succeeds within `catalog.delete_busy_ms`.
#[tokio::test(flavor = "multi_thread")]
async fn a_delete_during_a_gc_is_not_held_by_pinned_readers() {
    let dirs = Dirs::new();
    let mut config = dirs.config();
    config.catalog.write_busy_ms = 5_000;
    config.catalog.delete_busy_ms = 1_000;
    config.catalog.read_busy_ms = 200;
    let server = TestServer::in_process(dirs, config);
    server.ingest(&three_parts()).await;
    let socket = server.socket_of("s02_cbore", "BLOCK").await;
    let seams = server.server().seams();
    let gate = Gate::new();
    seams
        .hooks
        .set("catalog.judge", Action::Pause(gate.clone()));
    let body =
        json!({ "socket": socket, "candidates": { "catalog": { "selection": "exhaustive" } } });
    let held = server.submit("/v1/matches", &body).await;
    assert!(gate.arrived(1, Duration::from_secs(60)).await);
    // A write after the match's snapshot: the log holds frames it pins.
    server.ingest(&support::junk_files(1)).await;
    let gc = server.submit("/v1/catalog/gc", &json!({})).await;
    let gc_id = gc["id"].as_str().unwrap().to_owned();
    left_the_queue(&server, &gc_id).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let deleted = server
        .send(Call::new(Method::DELETE, "/v1/catalog/entries/m_bracket"))
        .await;
    assert_eq!(
        deleted.status,
        200,
        "{}",
        String::from_utf8_lossy(&deleted.body)
    );
    gate.open();
    seams.hooks.clear("catalog.judge");
    let gc = server.finished_job(&gc_id, Duration::from_secs(60)).await;
    assert_eq!(gc["state"], "succeeded", "{gc}");
    let held = server
        .finished_job(held["id"].as_str().unwrap(), Duration::from_secs(60))
        .await;
    assert_eq!(held["state"], "succeeded", "{held}");
    catalog_xh12(&server, &gc).await;
}

/// M6-50: at most two synchronous deletes run at once; a third waits for
/// a slot as long as a read waits for its slot (half the handler's time),
/// then answers `503 catalog_busy`, `Retry-After`, before any writer
/// opens; the two then succeed, and so does the third again.
#[tokio::test(flavor = "multi_thread")]
async fn deletes_run_two_at_a_time() {
    let dirs = Dirs::new();
    let mut config = dirs.config();
    config.server.handler_timeout_seconds = 4;
    let server = Arc::new(TestServer::in_process(dirs, config));
    server.ingest(&three_parts()).await;
    let seams = server.server().seams();
    let gate = Gate::new();
    seams
        .hooks
        .set("catalog.delete", Action::Pause(gate.clone()));
    let mut held = Vec::new();
    for entry in ["m_block02", "m_block02_cb_deep"] {
        let deleting = Arc::clone(&server);
        held.push(tokio::spawn(async move {
            deleting
                .send(Call::new(
                    Method::DELETE,
                    &format!("/v1/catalog/entries/{entry}"),
                ))
                .await
        }));
    }
    assert!(gate.arrived(2, Duration::from_secs(30)).await);
    let third = tokio::time::timeout(
        Duration::from_secs(10),
        server.send(Call::new(Method::DELETE, "/v1/catalog/entries/m_bracket")),
    )
    .await
    .expect("answered without the two");
    assert_eq!(
        (third.status.as_u16(), third.code()),
        (503, "catalog_busy".into())
    );
    assert_eq!(third.json()["details"]["limit"], "delete_concurrency");
    assert!(third.header("retry-after").is_some());
    gate.open();
    seams.hooks.clear("catalog.delete");
    for delete in held {
        assert_eq!(delete.await.unwrap().status, 200);
    }
    let again = server
        .send(Call::new(Method::DELETE, "/v1/catalog/entries/m_bracket"))
        .await;
    assert_eq!(again.status, 200);
}

/// CX6, JS4: a query job picked before the WAL barrier rose, its permits
/// held, waits for the barrier to lower before it starts (it stays
/// `queued`), and the barrier counts only queries that started; once it
/// lowers, the job runs.
#[tokio::test(flavor = "multi_thread")]
async fn a_query_picked_before_the_barrier_waits_for_it() {
    let server = TestServer::fresh();
    server.ingest(&three_parts()).await;
    let socket = server.socket_of("s02_cbore", "BLOCK").await;
    let seams = server.server().seams();
    let gate = Gate::new();
    seams
        .hooks
        .set("job.permitted", Action::Pause(gate.clone()));
    let body = json!({ "socket": socket, "candidates": { "catalog": {} } });
    let job = server.submit("/v1/matches", &body).await;
    let id = job["id"].as_str().unwrap().to_owned();
    assert!(gate.arrived(1, Duration::from_secs(30)).await);
    seams.jobs.hold_queries(true);
    gate.open();
    seams.hooks.clear("job.permitted");
    tokio::time::sleep(Duration::from_millis(500)).await;
    let waiting = server.get(&format!("/v1/jobs/{id}")).await.json();
    assert_eq!(waiting["state"], "queued", "held by the barrier: {waiting}");
    seams.jobs.hold_queries(false);
    let ended = server.finished_job(&id, Duration::from_secs(60)).await;
    assert_eq!(ended["state"], "succeeded", "{ended}");
}

/// CX6: the barrier is held by a guard, so a panic under it (in `VACUUM`)
/// lowers it as the job fails `internal`: query dispatch goes on, a
/// catalog match runs.
#[tokio::test(flavor = "multi_thread")]
async fn a_panic_under_the_barrier_lowers_it() {
    let server = TestServer::fresh();
    server.ingest(&three_parts()).await;
    let socket = server.socket_of("s02_cbore", "BLOCK").await;
    let seams = server.server().seams();
    seams.hooks.set(
        "catalog.vacuum",
        Action::Panic("a planted panic under the barrier".into()),
    );
    let gc = server
        .submit("/v1/catalog/gc", &json!({ "vacuum": true }))
        .await;
    let gc = server
        .finished_job(gc["id"].as_str().unwrap(), Duration::from_secs(60))
        .await;
    assert_eq!(
        (gc["state"].as_str(), gc["error"]["code"].as_str()),
        (Some("failed"), Some("internal")),
        "{gc}"
    );
    seams.hooks.clear("catalog.vacuum");
    let matched = server
        .run_match(&json!({ "socket": socket, "candidates": { "catalog": {} } }))
        .await;
    assert_eq!(matched["state"], "succeeded", "dispatch went on: {matched}");
}

/// JB5: a cancel that lands after a write job's last check of its flag
/// stops nothing: the ingest ran to its end, a refused file's exit 3, and
/// the job ends as it ran, `refused`, exit 3, its document kept.
#[tokio::test(flavor = "multi_thread")]
async fn a_late_cancel_leaves_a_finished_ingest_as_it_ended() {
    let server = TestServer::fresh();
    let seams = server.server().seams();
    let (bytes, name) = support::junk_files(1).remove(0);
    let sha = server.upload(&bytes).await;
    let gate = Gate::new();
    seams.hooks.set("job.ran", Action::Pause(gate.clone()));
    let body = json!({ "files": [{ "upload": format!("sha256:{sha}"), "file_name": name }] });
    let job = server.submit("/v1/catalog/entries", &body).await;
    let id = job["id"].as_str().unwrap().to_owned();
    assert!(gate.arrived(1, Duration::from_secs(30)).await);
    let cancelling = cancel(&server, &id).await;
    assert_eq!(cancelling.status, 202);
    assert_eq!(cancelling.json()["state"], "cancelling");
    gate.open();
    seams.hooks.clear("job.ran");
    let ended = server.finished_job(&id, Duration::from_secs(60)).await;
    assert_eq!(
        (ended["state"].as_str(), ended["exit_code"].as_i64()),
        (Some("refused"), Some(3)),
        "{ended}"
    );
    assert!(!ended["document"].is_null(), "{ended}");
    catalog_xh12(&server, &ended).await;
}

/// The review of PR 7b: one user holds at most half the catalog's read
/// slots and one `limit=all` listing: past them, `429 too_many_streams`,
/// `details.per_user`, retryable, without waiting; another user reads on.
#[tokio::test(flavor = "multi_thread")]
async fn one_user_holds_a_share_of_the_catalogs_reads() {
    let dirs = Dirs::new();
    let mut config = dirs.config();
    config.limits.read_concurrency = 4;
    let server = Arc::new(TestServer::in_process(dirs, config));
    server.ingest(&three_parts()).await;
    let ada = seed_user(server.server(), "ada", Role::Reader, None);
    let (_, ada) = seed_token(server.server(), &ada, Role::Reader, 30);
    let seams = server.server().seams();
    for (point, path, limit, share) in [
        (
            "catalog.step",
            "/v1/catalog/entries/m_block02/step",
            "read_concurrency",
            2,
        ),
        (
            "catalog.list",
            "/v1/catalog/entries?limit=all",
            "list_all_concurrency",
            1,
        ),
    ] {
        let gate = Gate::new();
        seams.hooks.set(point, Action::Pause(gate.clone()));
        let mut held = Vec::new();
        for _ in 0..share {
            let reading = Arc::clone(&server);
            held.push(tokio::spawn(async move { reading.get(path).await }));
        }
        assert!(gate.arrived(share, Duration::from_secs(30)).await);
        let over = tokio::time::timeout(Duration::from_secs(10), server.get(path))
            .await
            .expect("answered without waiting");
        assert_eq!(
            (over.status.as_u16(), over.code()),
            (429, "too_many_streams".into()),
            "{path}"
        );
        assert_eq!(over.json()["details"]["limit"], limit);
        assert_eq!(over.json()["details"]["per_user"], true);
        assert!(over.header("retry-after").is_some());
        let other = server
            .send(Call::get("/v1/catalog/entries/m_block02").bearer(&ada))
            .await;
        assert_eq!(other.status, 200, "another user reads on");
        gate.open();
        seams.hooks.clear(point);
        for read in held {
            assert_eq!(read.await.unwrap().status, 200, "{path}");
        }
    }
}

/// VT10, XH12, JB6, SD4: the built binary aborted around each write and
/// maintenance kind's commits: after its catalog call (`job.ran`), and
/// after its end committed (`job.committed`), then started again. Each
/// job has one terminal state: a write job cut short before its end is
/// `interrupted`, `job_interrupted`, with the generations it started at
/// and at recovery; one whose end committed keeps it; a verify, pure, is
/// run again to its end; and the catalog is consistent every time.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn jobs_aborted_around_their_commits_are_settled() {
    const RESTART: &str = "[test]\nlease_stale_ms = 1\n";
    let mut server = TestServer::process(Dirs::new(), RESTART);
    server.ingest(&three_parts()).await;
    let p01 = server.upload(&support::part("p01_through.stp")).await;
    let replace = json!({ "upload": format!("sha256:{p01}"), "file_name": "p01_through.stp" });
    let cases = [
        (
            "catalog_replace",
            Method::PUT,
            "/v1/catalog/entries/m_block02",
            replace,
        ),
        (
            "catalog_prune",
            Method::POST,
            "/v1/catalog/prune",
            json!({ "revisions": true }),
        ),
        (
            "catalog_reindex",
            Method::POST,
            "/v1/catalog/reindex",
            json!({ "all": true }),
        ),
        ("catalog_gc", Method::POST, "/v1/catalog/gc", json!({})),
        (
            "catalog_verify",
            Method::POST,
            "/v1/catalog/verify",
            json!({ "deep": true }),
        ),
    ];
    for point in ["job.ran", "job.committed"] {
        for (kind, method, path, body) in &cases {
            let hooks = format!("{RESTART}hooks = {{ \"{point}\" = \"abort\" }}\n");
            server.restart_process(&hooks);
            let call = Call::json(method.clone(), path, body).header("prefer", "wait=60");
            assert!(
                server.try_send(call).await.is_none(),
                "{kind} at {point}: the abort answered"
            );
            server.restart_process(RESTART);
            let jobs = server.get(&format!("/v1/jobs?kind={kind}")).await.json();
            let job = jobs["items"].as_array().unwrap().last().unwrap().clone();
            let id = job["id"].as_str().unwrap();
            let job = server.finished_job(id, Duration::from_secs(120)).await;
            let state = job["state"].as_str().unwrap();
            match (*kind, point) {
                ("catalog_verify", _) | (_, "job.committed") => {
                    assert_eq!(state, "succeeded", "{kind} at {point}: {job}");
                }
                _ => {
                    assert_eq!(
                        (state, job["error"]["code"].as_str()),
                        ("interrupted", Some("job_interrupted")),
                        "{kind} at {point}: {job}"
                    );
                    let details = &job["error"]["details"];
                    assert!(details["generation_at_start"].is_i64(), "{job}");
                    assert!(details["generation_at_recovery"].is_i64(), "{job}");
                }
            }
            catalog_xh12(&server, &job).await;
        }
    }
}

/// CX6 (the review of PR 7b): the log's pages are counted in the
/// database's own page size, read once at start, not an assumed 4,096
/// bytes: a catalog made with pages of 8,192 bytes is counted so.
#[tokio::test]
async fn the_barrier_counts_the_catalogs_own_pages() {
    let dirs = Dirs::new();
    let db = rusqlite::Connection::open(dirs.catalog.join("catalog.db")).unwrap();
    db.execute_batch(
        "PRAGMA journal_mode = DELETE; PRAGMA page_size = 8192; VACUUM; PRAGMA journal_mode = WAL;",
    )
    .unwrap();
    drop(db);
    let config = dirs.config();
    let server = TestServer::in_process(dirs, config);
    assert_eq!(server.server().seams().catalog.page_bytes(), 8192);
    server.ingest(&three_parts()).await;
    assert_eq!(support::verified(&server.dirs.catalog), Some(0));
}

/// JS8's enforcement for a stuck core call (PR 8): extractions held
/// forever at `job.read`, where no flag is checked, as a call that never
/// returns. Past its deadline and `jobs.stuck_grace_seconds` the first is
/// given up: it ends `failed`, `deadline_exceeded`, with no document, its
/// permits and its slot released (the only CPU permit runs the next job),
/// and the server, at `jobs.abandoned_max`, is unready. At that bound a
/// second stuck job, cancelled, is not given up: it keeps its permits past
/// the grace, as before. When the calls return at last, the first's
/// result is discarded (no revision, its end unchanged), the second ends
/// `cancelled`, nothing is counted and the server is ready; a job then
/// runs as ever. XH12 holds throughout.
#[tokio::test]
async fn a_stuck_call_is_given_up_and_its_permits_released() {
    let dirs = Dirs::new();
    let mut config = dirs.config();
    config.jobs.cpu = 1;
    config.jobs.abandoned_max = 1;
    config.jobs.stuck_grace_seconds = 1;
    let server = TestServer::in_process(dirs, config);
    let assembly = support::assembly("s02_cbore.stp");
    assert_eq!(
        server.register(&assembly, "s02_cbore.stp").await["state"],
        "succeeded"
    );
    let sha = fitment_core::sha256_hex(&assembly);
    let seams = server.server().seams();
    let stuck = Gate::new();
    let _open = Opens(vec![stuck.clone()]);
    seams.hooks.set("job.read", Action::Pause(stuck.clone()));
    seams.jobs.set_deadline(Duration::from_millis(300));
    let first = submit_extraction(&server, &sha).await;
    assert!(stuck.arrived(1, Duration::from_secs(30)).await);
    let given_up = server.finished_job(&first, Duration::from_secs(30)).await;
    assert_eq!(given_up["state"], "failed", "{given_up}");
    assert_eq!(given_up["error"]["code"], "deadline_exceeded");
    assert!(given_up["document"].is_null() && given_up["result"].is_null());
    no_verdict(&given_up);
    assert_eq!(seams.jobs.abandoned(), 1);
    assert_eq!(seams.jobs.cpu_free(), (1, 1), "its permit is back");
    // CX6 (the review of PR 8): its thread may still pin the catalog's
    // log, so it is still counted for the barrier.
    assert_eq!(seams.jobs.queries_running(), 1, "counted until it returns");
    let ready = server.send(Call::get("/v1/ready").anonymous()).await;
    assert_eq!(
        (ready.status.as_u16(), ready.code()),
        (503, "shutting_down".into())
    );
    assert_eq!(ready.json()["details"]["reason"], "stuck");
    // At the bound: a second stuck job keeps its permit past the grace.
    seams.jobs.set_deadline(Duration::from_secs(600));
    let second = submit_extraction(&server, &sha).await;
    assert!(stuck.arrived(2, Duration::from_secs(30)).await, "it ran");
    let cancel = server
        .send(Call::new(
            Method::POST,
            &format!("/v1/jobs/{second}/cancel"),
        ))
        .await;
    assert_eq!(cancel.json()["state"], "cancelling");
    tokio::time::sleep(Duration::from_millis(2500)).await;
    let held = server.get(&format!("/v1/jobs/{second}")).await.json();
    assert_eq!(held["state"], "cancelling", "{held}");
    assert_eq!(seams.jobs.cpu_free(), (0, 1), "it keeps its permit");
    assert_eq!(seams.jobs.abandoned(), 1);
    // The calls return at last.
    seams.hooks.clear("job.read");
    stuck.open();
    let cancelled = server.finished_job(&second, Duration::from_secs(60)).await;
    assert_eq!(cancelled["state"], "cancelled", "{cancelled}");
    let started = std::time::Instant::now();
    while seams.jobs.abandoned() > 0 {
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "never returned"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let started = std::time::Instant::now();
    while seams.jobs.queries_running() > 0 {
        assert!(started.elapsed() < Duration::from_secs(30), "still counted");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let after = server.get(&format!("/v1/jobs/{first}")).await.json();
    assert_eq!(after["state"], "failed", "its end stands: {after}");
    assert!(after["document"].is_null() && after["result"].is_null());
    assert!(lines(&server).await.is_empty(), "no revision from either");
    assert_eq!(
        server.send(Call::get("/v1/ready").anonymous()).await.status,
        204
    );
    let (bytes, golden) = p01();
    let upload = server.upload(&bytes).await;
    let job = server
        .send(waiting("/v1/features", &features(&upload)))
        .await
        .json();
    assert_eq!(job["state"], "succeeded", "{job}");
    let document = server.document(&job).await;
    assert_eq!(document.body, golden);
    assert_eq!(seams.jobs.cpu_free(), (1, 1));
}

/// JS8 (PR 8), SD1, SD4: a graceful stop with a job stuck in its call
/// (the built binary held 10 minutes at `job.read`, where no flag is
/// read): the drain sets its flag and waits its bound, and the process
/// exits 0 without waiting for the stuck thread, as it would forever; the
/// next start finds the job running and queues it again (SD4), which then
/// runs to the CLI's bytes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stop_does_not_wait_for_a_stuck_call() {
    const RESTART: &str = "[test]\nlease_stale_ms = 1\n";
    let config = format!(
        "[jobs]\ndrain_seconds = 1\n{RESTART}hooks = {{ \"job.read\" = \"delay:600000\" }}\n"
    );
    let mut server = TestServer::process(Dirs::new(), &config);
    let (bytes, golden) = p01();
    let sha = server.upload(&bytes).await;
    let job = server
        .send(Call::json(Method::POST, "/v1/features", &features(&sha)))
        .await
        .json();
    let id = job["id"].as_str().unwrap().to_owned();
    let started = std::time::Instant::now();
    while server.get(&format!("/v1/jobs/{id}")).await.json()["state"] != "running" {
        assert!(started.elapsed() < Duration::from_secs(30), "never ran");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let child = server.child.take().unwrap();
    let stopping = std::time::Instant::now();
    let (code, _, stderr) = tokio::task::spawn_blocking(move || child.stop())
        .await
        .unwrap();
    assert_eq!(code, Some(0), "{stderr}");
    assert!(
        stopping.elapsed() < Duration::from_secs(25),
        "{:?}",
        stopping.elapsed()
    );
    server.restart_process(RESTART);
    let job = server.finished_job(&id, Duration::from_secs(60)).await;
    assert_eq!(job["state"], "succeeded", "{job}");
    assert_eq!(server.document(&job).await.body, golden);
}

/// A server for the stuck-call tests: `cpu` query slots, `max` jobs given up
/// at once, a 1 s grace; and `s02_cbore.stp` registered by the harness's
/// admin, its SHA-256 returned.
async fn stuck_world(cpu: u32, max: u32) -> (TestServer, String) {
    let dirs = Dirs::new();
    let mut config = dirs.config();
    config.jobs.cpu = cpu;
    // One permit a query job, so `cpu` of them run at once (JS1).
    config.jobs.match_threads = 1;
    config.jobs.abandoned_max = max;
    config.jobs.stuck_grace_seconds = 1;
    let server = TestServer::in_process(dirs, config);
    let assembly = support::assembly("s02_cbore.stp");
    assert_eq!(
        server.register(&assembly, "s02_cbore.stp").await["state"],
        "succeeded"
    );
    (server, fitment_core::sha256_hex(&assembly))
}

/// Opens its gates when dropped, so a failed assertion never leaves a
/// blocking task held and the runtime's shutdown waiting for it.
struct Opens(Vec<Gate>);

impl Drop for Opens {
    fn drop(&mut self) {
        for gate in &self.0 {
            gate.open();
        }
    }
}

/// Waits up to 30 s for `check`.
async fn until(what: &str, check: impl Fn() -> bool) {
    let started = std::time::Instant::now();
    while !check() {
        assert!(started.elapsed() < Duration::from_secs(30), "{what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// JS8, JB5 (the review of PR 8): a write job is never given up. An
/// ingest held after its first committed batch (`catalog.batch`, where no
/// flag is read), cancelled, stays `cancelling` past the grace, holding its
/// permit, nothing given up and the server ready; released, it stops before
/// its next file and ends `cancelled` with its committed batch's document.
#[tokio::test]
async fn a_stuck_write_job_is_never_given_up() {
    let (server, _) = stuck_world(1, 1).await;
    let seams = server.server().seams();
    let mut listed = Vec::new();
    for (bytes, name) in &support::junk_files(70) {
        let sha = server.upload(bytes).await;
        listed.push(json!({ "upload": format!("sha256:{sha}"), "file_name": name }));
    }
    let gate = Gate::new();
    let _open = Opens(vec![gate.clone()]);
    seams
        .hooks
        .set("catalog.batch", Action::Pause(gate.clone()));
    let job = server
        .submit("/v1/catalog/entries", &json!({ "files": listed }))
        .await;
    let id = job["id"].as_str().unwrap().to_owned();
    assert!(gate.arrived(1, Duration::from_secs(60)).await);
    assert_eq!(cancel(&server, &id).await.status, 202);
    tokio::time::sleep(Duration::from_millis(2500)).await;
    let held = server.get(&format!("/v1/jobs/{id}")).await.json();
    assert_eq!(held["state"], "cancelling", "{held}");
    assert_eq!(seams.jobs.abandoned(), 0, "not given up");
    assert_eq!(seams.jobs.cpu_free(), (0, 1), "it keeps its permit");
    assert_eq!(
        server.send(Call::get("/v1/ready").anonymous()).await.status,
        204
    );
    gate.open();
    seams.hooks.clear("catalog.batch");
    let ended = server.finished_job(&id, Duration::from_secs(60)).await;
    assert_eq!(ended["state"], "cancelled", "{ended}");
    let document: Value = serde_json::from_slice(&server.document(&ended).await.body).unwrap();
    assert!(
        !document["changes"].as_array().unwrap().is_empty(),
        "JB5: its committed batch's lines"
    );
    catalog_xh12(&server, &ended).await;
}

/// JS8 (the review of PR 8): a job refused at `jobs.abandoned_max` is
/// watched still. A, ada's, stuck and given up past its deadline; B, bob's,
/// stuck and refused at the bound: B keeps its permit, counts toward the
/// bound (the server unready while A's thread is out, and after), until
/// A's thread returns; then B is given up at once, and the server is ready
/// once B's thread returns too.
#[tokio::test]
async fn a_refused_stuck_job_is_given_up_when_a_place_frees() {
    let (server, sha) = stuck_world(2, 1).await;
    let seams = server.server().seams();
    let token = |name: &str| {
        let user = seed_user(server.server(), name, Role::Reader, None);
        seed_token(server.server(), &user, Role::Reader, 30).1
    };
    let (ada, bob) = (token("ada"), token("bob"));
    for (who, token) in [("ada", &ada), ("bob", &bob)] {
        server
            .send(put_upload(&support::assembly("s02_cbore.stp")).bearer(token))
            .await;
        let body = json!({ "upload": format!("sha256:{sha}"), "file_name": who });
        let job = server
            .send(waiting("/v1/assemblies", &body).bearer(token))
            .await
            .json();
        assert_eq!(job["state"], "succeeded", "{job}");
    }
    seams.jobs.set_deadline(Duration::from_millis(300));
    let extract = |token: &str| {
        let body = json!({ "assembly": format!("sha256:{sha}"), "part": "BLOCK" });
        Call::json(Method::POST, "/v1/sockets", &body).bearer(token)
    };
    let (gate_a, gate_b) = (Gate::new(), Gate::new());
    let _open = Opens(vec![gate_a.clone(), gate_b.clone()]);
    seams.hooks.set("job.read", Action::Pause(gate_a.clone()));
    let a = server.send(extract(&ada)).await.json()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(gate_a.arrived(1, Duration::from_secs(30)).await);
    seams.hooks.set("job.read", Action::Pause(gate_b.clone()));
    let b = server.send(extract(&bob)).await.json()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(gate_b.arrived(1, Duration::from_secs(30)).await);
    let started = std::time::Instant::now();
    let a_job = loop {
        let job = server
            .send(Call::get(&format!("/v1/jobs/{a}")).bearer(&ada))
            .await
            .json();
        if job["state"] == "failed" {
            break job;
        }
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "A never given up"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert_eq!(a_job["error"]["code"], "deadline_exceeded");
    tokio::time::sleep(Duration::from_millis(2500)).await;
    let held = server
        .send(Call::get(&format!("/v1/jobs/{b}")).bearer(&bob))
        .await
        .json();
    assert_eq!(held["state"], "cancelling", "B is refused: {held}");
    assert_eq!(seams.jobs.cpu_free(), (1, 2), "B keeps its permit");
    assert!(seams.jobs.stuck(), "the bound is met");
    gate_a.open();
    let started = std::time::Instant::now();
    let b_job = loop {
        let job = server
            .send(Call::get(&format!("/v1/jobs/{b}")).bearer(&bob))
            .await
            .json();
        if job["state"] == "failed" {
            break job;
        }
        assert_eq!(job["state"], "cancelling", "{job}");
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "B never given up"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert_eq!(b_job["error"]["code"], "deadline_exceeded", "B given up");
    assert_eq!(seams.jobs.abandoned(), 1, "B's thread is out");
    assert!(seams.jobs.stuck());
    seams.hooks.clear("job.read");
    gate_b.open();
    until("B's thread returns", || seams.jobs.abandoned() == 0).await;
    assert_eq!(
        server.send(Call::get("/v1/ready").anonymous()).await.status,
        204
    );
    assert_eq!(seams.jobs.cpu_free(), (2, 2));
}

/// JS8 (decided in the review of PR 8): at most one job given up an owner.
/// One user's two stuck jobs, both cancelled: the first is given up, the
/// second keeps its permit and is not counted toward the bound, so the
/// server stays ready; when the first's thread returns, the second is given
/// up.
#[tokio::test]
async fn an_owner_has_one_job_given_up_at_a_time() {
    let (server, sha) = stuck_world(2, 2).await;
    let seams = server.server().seams();
    let gate = Gate::new();
    let _open = Opens(vec![gate.clone()]);
    seams.hooks.set("job.read", Action::Pause(gate.clone()));
    let first = submit_extraction(&server, &sha).await;
    let second = submit_extraction(&server, &sha).await;
    assert!(gate.arrived(2, Duration::from_secs(30)).await);
    for id in [&first, &second] {
        assert_eq!(cancel(&server, id).await.status, 202);
    }
    let given = server.finished_job(&first, Duration::from_secs(30)).await;
    assert_eq!(given["state"], "cancelled", "{given}");
    tokio::time::sleep(Duration::from_millis(2500)).await;
    let held = server.get(&format!("/v1/jobs/{second}")).await.json();
    assert_eq!(held["state"], "cancelling", "{held}");
    assert_eq!(seams.jobs.abandoned(), 1);
    assert_eq!(seams.jobs.cpu_free(), (1, 2), "the second keeps its permit");
    assert!(
        !seams.jobs.stuck(),
        "an owner's own jobs do not make the server unready"
    );
    assert_eq!(
        server.send(Call::get("/v1/ready").anonymous()).await.status,
        204
    );
    seams.hooks.clear("job.read");
    gate.open();
    let ended = server.finished_job(&second, Duration::from_secs(30)).await;
    assert_eq!(ended["state"], "cancelled", "{ended}");
    until("both threads return", || seams.jobs.abandoned() == 0).await;
    assert_eq!(seams.jobs.cpu_free(), (2, 2));
}
