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
    let bound = Duration::from_secs(if cfg!(debug_assertions) { 10 } else { 5 });
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
