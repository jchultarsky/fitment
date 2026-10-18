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

/// UP8's comparison: the call lists of an upload of new bytes and of
/// bytes the store holds are the same calls in the same order, but for
/// one `rename` of the temporary file into place against one removal of
/// it, and neither reads the existing object.
fn same_work(new: &[FsCall], held: &[FsCall]) -> Result<(), String> {
    let (new, held): (Vec<String>, Vec<String>) = (
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
    if new.len() != held.len() {
        return Err(format!(
            "{} calls against {}: {new:?} {held:?}",
            new.len(),
            held.len()
        ));
    }
    let differ: Vec<(&String, &String)> = new.iter().zip(&held).filter(|(a, b)| a != b).collect();
    match differ.as_slice() {
        [(a, b)] if *a == "rename tmp object" && *b == "remove tmp" => Ok(()),
        _ => Err(format!("the lists differ by {differ:?}")),
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
/// lists are equal but for one rename against one removal, neither reads
/// the existing object, and the two answers have the same status and
/// headers, the bodies differing only in the SHA-256. The control: a
/// store that re-hashes the existing object on the upload path, or does
/// any other work in one branch, fails the comparison.
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
    assert_eq!(
        server
            .send(put_upload(&held).bearer(&ada_token))
            .await
            .status,
        201
    );
    let at = seam.calls().len();
    let reused = server.send(put_upload(&held).bearer(&bob_token)).await;
    let middle = seam.calls().len();
    let new = server.send(put_upload(&fresh).bearer(&bob_token)).await;
    let calls = seam.calls();
    let (reused_calls, new_calls) = (&calls[at..middle], &calls[middle..]);
    same_work(new_calls, reused_calls).unwrap();
    assert!(new_calls.iter().any(|c| matches!(c, FsCall::Rename(..))));
    assert_eq!(
        (reused.status, new.status),
        (new.status, axum::http::StatusCode::CREATED)
    );
    assert_eq!(headers(&reused), headers(&new));
    let (mut a, mut b) = (reused.json(), new.json());
    a["sha256"] = serde_json::Value::Null;
    b["sha256"] = serde_json::Value::Null;
    assert_eq!(a, b);
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
    streamed.insert(3, FsCall::OpenStream(object));
    assert!(same_work(new_calls, &streamed).is_err());
    assert!(same_work(new_calls, &reused_calls[1..]).is_err());
    assert!(
        same_work(new_calls, new_calls).is_err(),
        "no branch differs"
    );
}
