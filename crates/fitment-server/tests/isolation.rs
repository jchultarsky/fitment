//! Isolation between users (docs/research/m6-api.md, section 10.5; VT25,
//! VT48): a hash grants nothing (UP3, AU8), and an upload of bytes another
//! user holds does the same observable work as one of new bytes (UP8).

mod support;

use std::sync::Arc;

use axum::http::Method;
use fitment_catalog::fs::{Call as FsCall, CountingFs};
use fitment_server::auth::Role;
use support::{Call, Dirs, Reply, TestServer, put_upload, seed_token, seed_user};

/// VT25, UP3, AU8: another user's upload is `404` to `HEAD`, `GET` and
/// `DELETE`, the answer for bytes nobody holds; uploading those bytes is
/// `201`, as new bytes are, and gives the caller a row of their own;
/// deleting one user's row leaves the other's.
#[tokio::test]
async fn uploads_resolve_among_the_callers_own() {
    let server = TestServer::fresh();
    let ada = seed_user(server.server(), "ada", Role::Reader, None);
    let bob = seed_user(server.server(), "bob", Role::Reader, None);
    let (_, ada_token) = seed_token(server.server(), &ada, Role::Reader, 30);
    let (_, bob_token) = seed_token(server.server(), &bob, Role::Reader, 30);
    let bytes = b"ada's part";
    let sha256 = fitment_core::sha256_hex(bytes);
    let path = format!("/v1/uploads/sha256:{sha256}");
    let nobody = format!(
        "/v1/uploads/sha256:{}",
        fitment_core::sha256_hex(b"nobody's")
    );
    assert_eq!(
        server
            .send(put_upload(bytes).bearer(&ada_token))
            .await
            .status,
        201
    );
    for method in [Method::GET, Method::HEAD, Method::DELETE] {
        let theirs = server
            .send(Call::new(method.clone(), &path).bearer(&bob_token))
            .await;
        let none = server
            .send(Call::new(method.clone(), &nobody).bearer(&bob_token))
            .await;
        assert_eq!(theirs.status, 404, "{method}");
        assert_eq!(theirs.status, none.status, "{method}");
        if method != Method::HEAD {
            assert_eq!(theirs.json()["detail"], none.json()["detail"], "{method}");
        }
    }
    let listed = server
        .send(Call::get("/v1/uploads").bearer(&bob_token))
        .await;
    assert_eq!(listed.json()["items"], serde_json::json!([]));
    let bobs = server.send(put_upload(bytes).bearer(&bob_token)).await;
    assert_eq!(
        bobs.status, 201,
        "another user's bytes are answered as new bytes"
    );
    assert_eq!(
        server
            .send(Call::new(Method::HEAD, &path).bearer(&bob_token))
            .await
            .status,
        200
    );
    assert_eq!(server.object_files().len(), 1, "stored once");
    assert_eq!(
        server
            .send(Call::new(Method::DELETE, &path).bearer(&ada_token))
            .await
            .status,
        204
    );
    assert_eq!(
        server
            .send(Call::new(Method::HEAD, &path).bearer(&ada_token))
            .await
            .status,
        404
    );
    assert_eq!(
        server
            .send(Call::new(Method::HEAD, &path).bearer(&bob_token))
            .await
            .status,
        200
    );
    assert_eq!(server.object_files().len(), 1, "bob's row holds the object");
}

/// VT25, AU8, UP3: an admin reads another user's uploads with `?owner=`
/// on `GET`, `HEAD` and the list only; a reader's `?owner=` naming
/// another is `403`, an unknown user `404`, and no write takes it (`400
/// invalid_parameter`), so no upload is made or deleted in another's name.
#[tokio::test]
async fn an_admin_reads_another_users_uploads_on_reads_only() {
    let server = TestServer::fresh();
    let ada = seed_user(server.server(), "ada", Role::Reader, None);
    let bob = seed_user(server.server(), "bob", Role::Reader, None);
    let (_, bob_token) = seed_token(server.server(), &bob, Role::Reader, 30);
    let bytes = b"bob's part";
    let path = format!("/v1/uploads/sha256:{}", fitment_core::sha256_hex(bytes));
    assert_eq!(
        server
            .send(put_upload(bytes).bearer(&bob_token))
            .await
            .status,
        201
    );
    let admin = server.token().to_owned();
    let owned = |owner: &str| format!("{path}?owner={owner}");
    let read = server.send(Call::get(&owned(&bob.id)).bearer(&admin)).await;
    assert_eq!(read.status, 200);
    assert_eq!(read.json()["held_by"], serde_json::json!(["upload"]));
    let head = server
        .send(Call::new(Method::HEAD, &owned(&bob.id)).bearer(&admin))
        .await;
    assert_eq!(head.status, 200);
    assert_eq!(
        server
            .send(Call::get(&owned(&ada.id)).bearer(&admin))
            .await
            .status,
        404
    );
    let list = server
        .send(Call::get(&format!("/v1/uploads?owner={}", bob.id)).bearer(&admin))
        .await;
    assert_eq!(list.json()["items"].as_array().unwrap().len(), 1);
    let forbidden = server
        .send(Call::get(&owned(&ada.id)).bearer(&bob_token))
        .await;
    assert_eq!(forbidden.code(), "forbidden");
    let unknown = server
        .send(Call::get(&owned("u_7zzzzzzzzzzzzzzzzzzzzzzzzz")).bearer(&admin))
        .await;
    assert_eq!(unknown.code(), "not_found");
    let mut put = put_upload(bytes).bearer(&admin);
    put.path = owned(&bob.id);
    assert_eq!(server.send(put).await.code(), "invalid_parameter");
    let delete = Call::new(Method::DELETE, &owned(&bob.id)).bearer(&admin);
    assert_eq!(server.send(delete).await.code(), "invalid_parameter");
    let post = Call::new(Method::POST, &format!("/v1/uploads?owner={}", bob.id))
        .header("content-type", "model/step")
        .body(bytes)
        .bearer(&admin);
    assert_eq!(server.send(post).await.code(), "invalid_parameter");
}

/// What a call does, its paths read by role: the temporary file, the
/// object's final name, its fan-out directory, or another path.
fn shape(call: &FsCall) -> String {
    let role = |path: &std::path::Path| {
        let text = path.to_string_lossy().replace('\\', "/");
        if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("part"))
        {
            "tmp"
        } else if text.contains("/objects/sha256/")
            && text.rsplit('/').next().is_some_and(|n| n.len() == 64)
        {
            "object"
        } else if text.contains("/objects/sha256/") {
            "fan"
        } else {
            "other"
        }
    };
    match call {
        FsCall::CreateStream(p) => format!("create {}", role(p)),
        FsCall::WriteChunk(p, n) => format!("write {} {n}", role(p)),
        FsCall::SyncStream(p) => format!("sync {}", role(p)),
        FsCall::CreateDir(p) => format!("mkdir {}", role(p)),
        FsCall::Metadata(p) => format!("stat {}", role(p)),
        FsCall::Rename(a, b) => format!("rename {} {}", role(a), role(b)),
        FsCall::Remove(p) => format!("remove {}", role(p)),
        FsCall::SetReadonly(p, r) => format!("readonly {} {r}", role(p)),
        FsCall::SyncDir(p) => format!("syncdir {}", role(p)),
        FsCall::Read(p) => format!("read {}", role(p)),
        FsCall::OpenStream(p) => format!("open {}", role(p)),
        FsCall::ReadChunk(p, _) => format!("readchunk {}", role(p)),
        other => format!("{other:?}"),
    }
}

/// UP8's comparison (PR 4's review): the call lists of an upload of new
/// bytes and of bytes the store holds are the same calls in the same
/// order: both rename the temporary file over the final name. Windows'
/// one exception is the read-only flag cleared on a standing object before
/// it is renamed over, which its rename needs; neither branch reads the
/// existing object.
fn same_work(new: &[FsCall], held: &[FsCall]) -> Result<(), String> {
    let (new, mut held): (Vec<String>, Vec<String>) = (
        new.iter().map(shape).collect(),
        held.iter().map(shape).collect(),
    );
    for list in [&new, &held] {
        if let Some(read) = list
            .iter()
            .find(|c| c.starts_with("read object") || c.starts_with("open object"))
        {
            return Err(format!("the object is read on the upload path: {read}"));
        }
    }
    if cfg!(windows) {
        if let Some(at) = held.iter().position(|c| c == "readonly object false") {
            held.remove(at);
        }
    }
    if new == held {
        Ok(())
    } else {
        Err(format!("the lists differ: {new:?} against {held:?}"))
    }
}

/// The headers of an answer, but those that name the SHA-256 or digest
/// the body (which hold the bytes' own fields), and the request id.
fn headers(reply: &Reply) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = reply
        .headers
        .iter()
        .filter(|(n, _)| {
            !["location", "repr-digest", "x-request-id", "content-length"].contains(&n.as_str())
        })
        .map(|(n, v)| (n.to_string(), v.to_str().unwrap().to_owned()))
        .collect();
    out.sort();
    out
}

/// VT48, UP8, UP3, OB2: one user uploads bytes another user holds, and
/// bytes nobody holds, of one length, under a `CountingFs`: the two call
/// lists are the same (PR 4's review: both rename over the final name,
/// whose fan-out directory the state made), neither reads the existing
/// object, and the two answers have the same status and headers, the
/// bodies differing only in the SHA-256. `DELETE` of bytes another user
/// holds and of bytes nobody else holds makes no file call at all, and
/// answers the same (M1: only the sweep unlinks). The control: a store
/// that re-hashes the existing object, or does other work in one branch,
/// fails the comparison. Call shapes cannot see the disk's own time (a
/// dirty directory's sync, say); the branches' medians are held within a
/// generous bound, a regression's tripwire rather than a proof.
#[tokio::test]
async fn both_upload_branches_do_the_same_work() {
    let dirs = Dirs::new();
    let config = dirs.config();
    let seam = CountingFs::new();
    let server = TestServer::in_process_with_fs(dirs, config, Arc::new(seam.clone()));
    let ada = seed_user(server.server(), "ada", Role::Reader, None);
    let bob = seed_user(server.server(), "bob", Role::Reader, None);
    let (_, ada_token) = seed_token(server.server(), &ada, Role::Reader, 30);
    let (_, bob_token) = seed_token(server.server(), &bob, Role::Reader, 30);
    let held = vec![b'h'; 3000];
    let fresh = vec![b'f'; 3000];
    let put = server.send(put_upload(&held).bearer(&ada_token)).await;
    assert_eq!(put.status, 201);
    let at = seam.calls().len();
    let reused = server.send(put_upload(&held).bearer(&bob_token)).await;
    let middle = seam.calls().len();
    let new = server.send(put_upload(&fresh).bearer(&bob_token)).await;
    let calls = seam.calls();
    let (reused_calls, new_calls) = (&calls[at..middle], &calls[middle..]);
    same_work(new_calls, reused_calls).unwrap();
    assert!(new_calls.iter().any(|c| matches!(c, FsCall::Rename(..))));
    assert!(!calls.iter().any(|c| matches!(c, FsCall::CreateDir(_))));
    assert_eq!(
        (reused.status, new.status),
        (new.status, axum::http::StatusCode::CREATED)
    );
    assert_eq!(headers(&reused), headers(&new));
    let (mut a, mut b) = (reused.json(), new.json());
    a["sha256"] = serde_json::Value::Null;
    b["sha256"] = serde_json::Value::Null;
    assert_eq!(a, b);
    // DELETE: bob's row of the bytes ada holds, and of the bytes only he
    // holds: no file call either way, and the same answer.
    let delete = |bytes: &[u8]| {
        let path = format!("/v1/uploads/sha256:{}", fitment_core::sha256_hex(bytes));
        Call::new(Method::DELETE, &path).bearer(&bob_token)
    };
    let before = seam.calls().len();
    let shared_gone = server.send(delete(&held)).await;
    let own_gone = server.send(delete(&fresh)).await;
    assert_eq!(seam.calls().len(), before, "a delete makes no file call");
    assert_eq!(
        (shared_gone.status, headers(&shared_gone)),
        (own_gone.status, headers(&own_gone))
    );
    // The control: a re-hash of the existing object, or one more call in
    // one branch, is caught.
    let object = server
        .dirs
        .state
        .join("objects/sha256")
        .join(&fitment_core::sha256_hex(&held)[..2])
        .join(fitment_core::sha256_hex(&held));
    let mut rehashed = reused_calls.to_vec();
    rehashed.insert(3, FsCall::Read(object.clone()));
    assert!(same_work(new_calls, &rehashed).is_err());
    let mut streamed = reused_calls.to_vec();
    streamed.insert(3, FsCall::OpenStream(object.clone()));
    assert!(same_work(new_calls, &streamed).is_err());
    assert!(same_work(new_calls, &reused_calls[1..]).is_err());
    let mut removed = reused_calls.to_vec();
    let rename = removed
        .iter()
        .position(|c| matches!(c, FsCall::Rename(..)))
        .unwrap();
    removed[rename] = FsCall::Remove(object);
    assert!(
        same_work(new_calls, &removed).is_err(),
        "the old remove branch"
    );
    branches_take_like_time(&server, &ada_token, &bob_token).await;
}

/// VT48's timing tripwire: the median of 15 uploads of bytes another user
/// holds, and of 15 of new bytes, are within three times each other and
/// 20 ms.
async fn branches_take_like_time(server: &TestServer, ada: &str, bob: &str) {
    let mut times = [Vec::new(), Vec::new()];
    for i in 0..15_u32 {
        let held = format!("held {i:04} {}", "x".repeat(2000));
        let fresh = format!("new! {i:04} {}", "x".repeat(2000));
        server.send(put_upload(held.as_bytes()).bearer(ada)).await;
        for (k, bytes) in [held, fresh].iter().enumerate() {
            let started = std::time::Instant::now();
            let reply = server.send(put_upload(bytes.as_bytes()).bearer(bob)).await;
            times[k].push(started.elapsed());
            assert_eq!(reply.status, 201);
        }
    }
    let median = |list: &mut Vec<std::time::Duration>| {
        list.sort();
        list[list.len() / 2]
    };
    let (held, fresh) = (median(&mut times[0]), median(&mut times[1]));
    let slack = std::time::Duration::from_millis(20);
    assert!(
        held <= fresh * 3 + slack && fresh <= held * 3 + slack,
        "held {held:?} against new {fresh:?}"
    );
}

/// What one user made for VT25: an assembly, its socket line and revision,
/// an upload, a finished match over it, a queued job.
struct Made {
    token: String,
    id: String,
    assembly: String,
    line: String,
    revision: String,
    upload: String,
    matched: String,
    queued: String,
}

/// `token`'s user registers s02, extracts BLOCK, uploads the committed
/// block and matches it; a second match is left queued (the dispatcher
/// held by the caller).
async fn make(server: &TestServer, token: &str, id: &str) -> Made {
    let bytes = support::assembly("s02_cbore.stp");
    let assembly = fitment_core::sha256_hex(&bytes);
    assert!(
        server
            .send(put_upload(&bytes).bearer(token))
            .await
            .status
            .is_success()
    );
    let body = serde_json::json!({ "upload": format!("sha256:{assembly}") });
    server
        .send(support::waiting("/v1/assemblies", &body).bearer(token))
        .await;
    let body = serde_json::json!({ "assembly": format!("sha256:{assembly}"), "part": "BLOCK" });
    let job = server
        .send(support::waiting("/v1/sockets", &body).bearer(token))
        .await
        .json();
    let revision = job["result"]["href"]
        .as_str()
        .unwrap()
        .trim_start_matches("/v1/sockets/")
        .to_owned();
    let line = revision.split('@').next().unwrap().to_owned();
    let candidate = support::candidate("m_block02.stp");
    let upload = fitment_core::sha256_hex(&candidate);
    assert!(
        server
            .send(put_upload(&candidate).bearer(token))
            .await
            .status
            .is_success()
    );
    let body = serde_json::json!({ "socket": revision, "candidates": { "files": [
        { "upload": format!("sha256:{upload}"), "label": "m_block02.stp" } ] } });
    let matched = server
        .send(support::waiting("/v1/matches", &body).bearer(token))
        .await
        .json();
    assert_eq!(matched["state"], "succeeded", "{matched}");
    Made {
        token: token.to_owned(),
        id: id.to_owned(),
        assembly,
        line,
        revision,
        upload,
        matched: matched["id"].as_str().unwrap().to_owned(),
        queued: String::new(),
    }
}

/// A review of `a`'s revision, and an import naming her socket's bytes,
/// are another's as surely as a read (RA7, UP3): `404`, the review's as an
/// id that does not exist, the import's as an upload not held.
async fn others_writes_are_not_found(
    server: &TestServer,
    a: &Made,
    ada_token: &str,
    charlie_token: &str,
) {
    let review = Call::json(
        Method::POST,
        &format!("/v1/sockets/{}/review", a.revision),
        &serde_json::json!({ "decisions": [{ "requirement": "bore:hole-427", "status": "kept" }] }),
    );
    let reviewed = server.send(review.bearer(charlie_token)).await;
    assert_eq!(
        (reviewed.status.as_u16(), reviewed.code()),
        (404, "not_found".to_owned())
    );
    let document = server
        .send(Call::get(&format!("/v1/sockets/{}", a.revision)).bearer(ada_token))
        .await;
    assert_eq!(document.status, 200);
    let import = serde_json::json!({
        "upload": format!("sha256:{}", fitment_core::sha256_hex(&document.body))
    });
    let imported = server
        .send(support::waiting("/v1/sockets/import", &import).bearer(charlie_token))
        .await;
    assert_eq!(
        (imported.status.as_u16(), imported.code()),
        (404, "upload_not_found".to_owned())
    );
}

/// VT25 in full (AU8, UP3, RA4): a non-owner cannot read, list, cancel,
/// download or delete another's job, match, upload, assembly or socket
/// line, and gets the `404` of an id that does not exist; two users with
/// the same assembly hold two lines of one `sk_` id.
#[tokio::test]
async fn a_non_owner_meets_404_everywhere() {
    let server = TestServer::fresh();
    let ada = seed_user(server.server(), "ada", Role::Reader, None);
    let bob = seed_user(server.server(), "bob", Role::Reader, None);
    let (_, ada_token) = seed_token(server.server(), &ada, Role::Reader, 30);
    let (_, bob_token) = seed_token(server.server(), &bob, Role::Reader, 30);
    let mut ada_made = make(&server, &ada_token, &ada.id).await;
    let bob_made = make(&server, &bob_token, &bob.id).await;
    assert_eq!(
        ada_made.line, bob_made.line,
        "one id from the same data (RA4)"
    );
    server.server().hold_jobs();
    let body = serde_json::json!({ "upload": format!("sha256:{}", ada_made.upload) });
    let queued = server
        .send(Call::json(Method::POST, "/v1/features", &body).bearer(&ada_token))
        .await
        .json();
    ada_made.queued = queued["id"].as_str().unwrap().to_owned();
    let a = &ada_made;
    let reads = [
        (Method::GET, format!("/v1/jobs/{}", a.matched)),
        (Method::GET, format!("/v1/jobs/{}/document", a.matched)),
        (Method::GET, format!("/v1/jobs/{}/events", a.matched)),
        (Method::GET, format!("/v1/matches/{}", a.matched)),
        (Method::GET, format!("/v1/matches/{}/document", a.matched)),
        (Method::GET, format!("/v1/matches/{}/candidates", a.matched)),
        (Method::POST, format!("/v1/jobs/{}/cancel", a.queued)),
        (Method::GET, format!("/v1/uploads/sha256:{}", a.upload)),
        (
            Method::HEAD,
            format!(
                "/v1/uploads/sha256:{}",
                fitment_core::sha256_hex(b"only ada's")
            ),
        ),
        (
            Method::GET,
            format!(
                "/v1/assemblies/sha256:{}/step",
                fitment_core::sha256_hex(b"only ada's")
            ),
        ),
        (Method::GET, format!("/v1/sockets/{}", a.revision)),
    ];
    assert!(
        server
            .send(put_upload(b"only ada's").bearer(&ada_token))
            .await
            .status
            .is_success()
    );
    let charlie = seed_user(server.server(), "charlie", Role::Reader, None);
    let (_, charlie_token) = seed_token(server.server(), &charlie, Role::Reader, 30);
    for (method, path) in &reads {
        let theirs = server
            .send(Call::new(method.clone(), path).bearer(&charlie_token))
            .await;
        assert_eq!(theirs.status, 404, "{method} {path}");
        if *method != Method::HEAD {
            assert_eq!(
                theirs.code(),
                "not_found",
                "{path}: an id that does not exist's answer"
            );
        }
    }
    others_writes_are_not_found(&server, a, &ada_token, &charlie_token).await;
    let charlie_lists = [
        "/v1/jobs",
        "/v1/matches",
        "/v1/uploads",
        "/v1/assemblies",
        "/v1/sockets",
    ];
    for path in charlie_lists {
        let listed = server
            .send(Call::get(path).bearer(&charlie_token))
            .await
            .json();
        assert!(listed["items"].as_array().unwrap().is_empty(), "{path}");
    }
    let deletes = [
        format!("/v1/sockets/{}", a.line),
        format!("/v1/assemblies/sha256:{}", a.assembly),
        format!("/v1/uploads/sha256:{}", a.upload),
    ];
    for path in &deletes {
        let reply = server
            .send(Call::new(Method::DELETE, path).bearer(&charlie_token))
            .await;
        assert_eq!(reply.status, 404, "{path}");
    }
    let _ = bob_made.token;
}

/// VT25, AU8: an admin reads another user's runs, lines, revisions and
/// uploads with `?owner=` on `GET` and `HEAD` only, and names none of them
/// in a job (`?owner=` on a write is `400 invalid_parameter`; another's
/// socket is `404`); a user names neither another's upload (`404
/// upload_not_found`) nor an assembly they did not register (`404
/// not_found`).
#[tokio::test]
async fn an_admin_reads_another_users_work_and_names_none_of_it() {
    let server = TestServer::fresh();
    let ada = seed_user(server.server(), "ada", Role::Reader, None);
    let root = seed_user(server.server(), "root", Role::Admin, None);
    let charlie = seed_user(server.server(), "charlie", Role::Reader, None);
    let (_, ada_token) = seed_token(server.server(), &ada, Role::Reader, 30);
    let (_, admin) = seed_token(server.server(), &root, Role::Admin, 30);
    let (_, charlie_token) = seed_token(server.server(), &charlie, Role::Reader, 30);
    let a = make(&server, &ada_token, &ada.id).await;
    assert!(
        server
            .send(put_upload(b"only ada's").bearer(&ada_token))
            .await
            .status
            .is_success()
    );
    // An admin reads ada's with ?owner= on GET and HEAD.
    for path in [
        format!("/v1/matches?owner={}", a.id),
        format!("/v1/sockets?owner={}", a.id),
        format!("/v1/sockets/{}?owner={}", a.revision, a.id),
        format!("/v1/uploads/sha256:{}?owner={}", a.upload, a.id),
    ] {
        let reply = server.send(Call::get(&path).bearer(&admin)).await;
        assert_eq!(reply.status, 200, "{path}");
    }
    let head = server
        .send(
            Call::new(
                Method::HEAD,
                &format!("/v1/uploads/sha256:{}?owner={}", a.upload, a.id),
            )
            .bearer(&admin),
        )
        .await;
    assert_eq!(head.status, 200);
    // ... and cannot name them in a job.
    let body = serde_json::json!({ "socket": a.revision, "candidates": { "files": [
        { "upload": format!("sha256:{}", a.upload), "label": "x.stp" } ] } });
    let owner = server
        .send(
            Call::json(Method::POST, &format!("/v1/matches?owner={}", a.id), &body).bearer(&admin),
        )
        .await;
    assert_eq!(
        (owner.status.as_u16(), owner.code()),
        (400, "invalid_parameter".to_owned())
    );
    let named = server
        .send(support::waiting("/v1/matches", &body).bearer(&admin))
        .await;
    assert_eq!(
        (named.status.as_u16(), named.code()),
        (404, "not_found".to_owned()),
        "ada's socket is not the admin's"
    );
    let charlie_made = make(&server, &charlie_token, &charlie.id).await;
    let charlie_sock = charlie_made.revision;
    let body = serde_json::json!({ "socket": charlie_sock, "candidates": { "files": [
        { "upload": format!("sha256:{}", fitment_core::sha256_hex(b"only ada's")), "label": "x.stp" } ] } });
    let theirs = server
        .send(support::waiting("/v1/matches", &body).bearer(&charlie_token))
        .await;
    assert_eq!(theirs.code(), "upload_not_found");
    let body = serde_json::json!({ "socket": charlie_sock, "candidates": { "components": [
        { "assembly": format!("sha256:{}", fitment_core::sha256_hex(&support::assembly("s01_bolted.stp"))), "part": "BRACKET" } ] } });
    let theirs = server
        .send(support::waiting("/v1/matches", &body).bearer(&charlie_token))
        .await;
    assert_eq!(theirs.code(), "not_found");
    let _ = (a.token, charlie_made.token);
}
