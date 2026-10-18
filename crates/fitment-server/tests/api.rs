//! The server's own routes in process (docs/research/m6-api.md, section
//! 7.2): health, readiness, `fitment.info` and the OpenAPI document; the
//! fallbacks, negotiation, response headers and CORS (HX4 to HX6, VN8);
//! and uploads (section 7.3, UP1 to UP3) and the store's checked reads
//! (OB3).

mod support;

use axum::http::Method;
use fitment_server::http::operations::{OPERATIONS, RequestBody};
use support::{Call, Dirs, TestServer, put_upload};

/// OP5: `204` while the process runs, without authentication, touching no
/// database: it answers while the catalog cannot be opened, and HEAD too.
#[tokio::test]
async fn health_needs_nothing_but_the_process() {
    let server = TestServer::fresh();
    let reply = server.get("/v1/health").await;
    assert_eq!(reply.status, 204);
    assert_eq!(reply.body, Vec::<u8>::new());
    assert_eq!(
        server
            .send(Call::new(Method::HEAD, "/v1/health"))
            .await
            .status,
        204
    );
    std::fs::remove_file(server.dirs.catalog.join("FITMENT-CATALOG")).unwrap();
    assert_eq!(server.get("/v1/health").await.status, 204);
    assert_eq!(server.get("/v1/ready").await.status, 503);
}

/// OP7: `204`; `503 shutting_down` while draining; `503
/// catalog_unavailable` with the reason and the CLI's exit code for it
/// (ER2) while the catalog cannot be opened, and ready again once it can.
#[tokio::test]
async fn readiness_follows_the_catalog_and_the_drain() {
    let server = TestServer::fresh();
    assert_eq!(server.get("/v1/ready").await.status, 204);
    let marker = server.dirs.catalog.join("FITMENT-CATALOG");
    let text = std::fs::read(&marker).unwrap();
    std::fs::remove_file(&marker).unwrap();
    let reply = server.get("/v1/ready").await;
    assert_eq!(
        (reply.status.as_u16(), reply.code()),
        (503, "catalog_unavailable".into())
    );
    let doc = reply.json();
    assert_eq!(doc["details"]["reason"], "not_a_catalog");
    assert_eq!(doc["exit_code"], 2);
    std::fs::write(&marker, text).unwrap();
    assert_eq!(server.get("/v1/ready").await.status, 204);
    server.server().drain();
    let reply = server.get("/v1/ready").await;
    assert_eq!(reply.code(), "shutting_down");
    assert_eq!(reply.json()["details"]["reason"], "draining");
    assert_eq!(reply.json()["retryable"], true);
    assert_eq!(reply.header("retry-after"), Some("5"));
}

/// OP7, ER2: a catalog of a newer schema is `catalog_unavailable` with
/// `newer_schema` and exit 3 (DB5).
#[tokio::test]
async fn a_newer_catalog_is_unavailable_with_exit_3() {
    let server = TestServer::fresh();
    let db = rusqlite::Connection::open(server.dirs.catalog.join("catalog.db")).unwrap();
    db.pragma_update(None, "user_version", 99).unwrap();
    drop(db);
    for path in ["/v1/ready", "/v1/info"] {
        let reply = server.get(path).await;
        assert_eq!(reply.code(), "catalog_unavailable", "{path}");
        let doc = reply.json();
        assert_eq!(doc["details"]["reason"], "newer_schema");
        assert_eq!(doc["exit_code"], 3);
    }
}

/// OP7, ER2: a planted link in the store is `linked`, exit 2.
#[cfg(unix)]
#[tokio::test]
async fn a_linked_catalog_is_unavailable_with_exit_2() {
    let server = TestServer::fresh();
    let blobs = server.dirs.catalog.join("blobs");
    let elsewhere = server.dirs.root.join("elsewhere");
    std::fs::rename(&blobs, &elsewhere).unwrap();
    std::os::unix::fs::symlink(&elsewhere, &blobs).unwrap();
    let reply = server.get("/v1/ready").await;
    assert_eq!(reply.code(), "catalog_unavailable");
    assert_eq!(reply.json()["details"]["reason"], "linked");
    assert_eq!(reply.json()["exit_code"], 2);
}

/// DC11, VN1, VN2, SV5: `fitment.info` states the server's version, every
/// version constant, each format's version, the API's `{major, minor}`,
/// the fit verifier (null: SN11, OP9), the catalog's generation and
/// counts, and the limits as configured; it holds no time and no path.
#[tokio::test]
async fn info_states_the_build_the_catalog_and_the_limits() {
    let dirs = Dirs::new();
    let mut config = dirs.config();
    config.limits.max_upload_mib = 64;
    config.matching.require_reviewed_floor = true;
    let server = TestServer::in_process(dirs, config);
    let reply = server.get("/v1/info").await;
    assert_eq!(reply.status, 200);
    assert_eq!(reply.header("fitment-format"), Some("fitment.info/1"));
    assert_eq!(reply.header("content-type"), Some("application/json"));
    let doc = reply.json();
    assert_eq!(doc["server"]["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(
        doc["api"],
        serde_json::json!({ "major": 1, "minor": fitment_server::info::API_MINOR })
    );
    let versions = &doc["versions"];
    assert_eq!(
        versions["extractor_version"],
        fitment_core::EXTRACTOR_VERSION
    );
    assert_eq!(
        versions["socket_rules_version"],
        fitment_core::SOCKET_RULES_VERSION
    );
    assert_eq!(
        versions["signature_version"],
        fitment_core::SIGNATURE_VERSION
    );
    assert_eq!(
        versions["match_rules_version"],
        fitment_core::MATCH_RULES_VERSION
    );
    assert_eq!(
        versions["properties_version"],
        fitment_core::PROPERTIES_VERSION
    );
    assert_eq!(
        versions["catalog_schema"],
        fitment_catalog::schema::SCHEMA_VERSION
    );
    assert_eq!(versions["server_db_schema"], 1);
    let formats: Vec<(String, u64)> = doc["formats"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| {
            (
                f["format"].as_str().unwrap().to_owned(),
                f["format_version"].as_u64().unwrap(),
            )
        })
        .collect();
    for (format, version) in [
        (
            fitment_core::FEATURES_FORMAT,
            fitment_core::FEATURES_FORMAT_VERSION,
        ),
        (
            fitment_core::PARTS_FORMAT,
            fitment_core::PARTS_FORMAT_VERSION,
        ),
        (
            fitment_core::SOCKET_FORMAT,
            fitment_core::SOCKET_FORMAT_VERSION,
        ),
        (
            fitment_core::MATCH_FORMAT,
            fitment_core::MATCH_FORMAT_VERSION,
        ),
        ("fitment.catalog", 1),
        ("fitment.info", 1),
        ("fitment.error", 1),
        ("fitment.upload", 1),
    ] {
        assert!(
            formats.contains(&(format.to_owned(), u64::from(version))),
            "{format}"
        );
    }
    assert!(doc["fit_verifier"].is_null());
    assert_eq!(doc["catalog"]["generation"], 1);
    assert_eq!(doc["catalog"]["entries"], 0);
    assert_eq!(doc["catalog"]["state"]["stale"], false);
    assert_eq!(doc["limits"]["max_upload_mib"], 64);
    assert_eq!(doc["limits"]["require_reviewed_floor"], true);
    assert_eq!(doc["limits"]["page_max"], 1000);
    let text = String::from_utf8(reply.body.clone()).unwrap();
    for word in ["time", "created", "_ms\"", "path"] {
        assert!(!text.contains(word), "fitment.info holds {word}");
    }
}

/// DC11: the catalog's counts are its own, read in one snapshot.
#[tokio::test]
async fn info_counts_the_catalog() {
    let server = TestServer::fresh();
    let tests = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../fitment-core/tests");
    let mut catalog =
        fitment_catalog::Catalog::open(&server.dirs.catalog, fitment_catalog::Options::default())
            .unwrap();
    catalog
        .ingest(
            &[
                tests.join("candidates/m_bracket.stp"),
                tests.join("data/r04_assembly.stp"),
            ],
            None,
        )
        .unwrap();
    drop(catalog);
    let doc = server.get("/v1/info").await.json();
    assert_eq!(doc["catalog"]["entries"], 2);
    assert_eq!(doc["catalog"]["indexed"], 1);
    assert_eq!(doc["catalog"]["quarantined"], 1);
    assert_eq!(doc["catalog"]["stale"], 0);
    assert!(doc["catalog"]["generation"].as_i64().unwrap() > 1);
}

/// VN7: `/v1/openapi.json` serves the committed bytes, without
/// authentication.
#[tokio::test]
async fn the_openapi_document_is_served_as_committed() {
    let server = TestServer::fresh();
    let reply = server.get("/v1/openapi.json").await;
    assert_eq!(reply.status, 200);
    assert_eq!(reply.header("content-type"), Some("application/json"));
    let committed =
        std::fs::read(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("openapi.json"))
            .unwrap();
    assert_eq!(reply.body, committed);
    assert_eq!(reply.body, fitment_server::OPENAPI);
    assert_eq!(reply.json()["openapi"], "3.1.0");
}

/// HX5: fixed routes only. A path no operation names is `404 not_found`,
/// never a file and never a `500`, and the path is not echoed; there is
/// no wildcard route.
#[tokio::test]
async fn unknown_paths_are_not_found() {
    let server = TestServer::fresh();
    // The longest URI the http crate builds; past 64 KiB, hyper refuses
    // the request line itself (slow.rs).
    let long = format!("/v1/{}", "a".repeat(60 * 1024));
    for path in [
        "/",
        "/v1",
        "/v1/",
        "/v1/nope",
        "/v1/info/",
        "/v1/info/extra",
        "//v1/info",
        "/v1/../v1/info",
        "/v1/%2e%2e/v1/info",
        "/v1/info%00",
        "/v1/%2f",
        "/v2/info",
        "/v1/openapi.json/../../etc/passwd",
        long.as_str(),
    ] {
        let reply = server.get(path).await;
        assert_eq!(reply.status, 404, "{path}");
        assert_eq!(reply.code(), "not_found", "{path}");
        let text = String::from_utf8_lossy(&reply.body);
        assert!(
            !text.contains("nope") && !text.contains("passwd") && !text.contains("aaaa"),
            "{path}"
        );
    }
    // HX5: a parameter is a whole segment, typed by its handler; none is
    // a wildcard.
    for op in &fitment_server::http::operations::OPERATIONS {
        assert!(!op.path.contains('*'), "{}", op.path);
        for segment in op.path.split('/') {
            if segment.contains('{') || segment.contains('}') {
                let name = segment.strip_prefix('{').and_then(|s| s.strip_suffix('}'));
                assert!(
                    name.is_some_and(|n| n.bytes().all(|b| b.is_ascii_lowercase())),
                    "{}",
                    op.path
                );
            }
        }
    }
}

/// Section 7.12: a path whose operations take other methods is `405
/// method_not_allowed`, with `Allow`.
#[tokio::test]
async fn other_methods_are_not_allowed() {
    let server = TestServer::fresh();
    for method in [Method::POST, Method::PUT, Method::DELETE, Method::PATCH] {
        let reply = server.send(Call::new(method.clone(), "/v1/info")).await;
        assert_eq!(reply.status, 405, "{method}");
        assert_eq!(reply.code(), "method_not_allowed");
        assert_eq!(reply.header("allow"), Some("GET, HEAD"));
    }
}

/// VN8: one representation per URL; an `Accept` that excludes it is `406
/// not_acceptable`, and one that admits it is answered.
#[tokio::test]
async fn accept_is_honoured() {
    let server = TestServer::fresh();
    for path in ["/v1/info", "/v1/openapi.json"] {
        for accept in ["text/html", "application/xml", "application/json;q=0"] {
            let reply = server.send(Call::get(path).header("accept", accept)).await;
            assert_eq!(reply.status, 406, "{path} {accept}");
            assert_eq!(reply.code(), "not_acceptable");
        }
        for accept in [
            "*/*",
            "application/*",
            "application/json",
            "text/html, application/json;q=0.1",
        ] {
            let reply = server.send(Call::get(path).header("accept", accept)).await;
            assert_eq!(reply.status, 200, "{path} {accept}");
        }
    }
}

/// HX6: every response, success or error, carries the security headers
/// and `Cache-Control: no-store`, and no `Server` header.
#[tokio::test]
async fn every_response_carries_the_security_headers() {
    let server = TestServer::fresh();
    for (method, path) in [
        (Method::GET, "/v1/health"),
        (Method::GET, "/v1/ready"),
        (Method::GET, "/v1/info"),
        (Method::GET, "/v1/openapi.json"),
        (Method::GET, "/v1/nope"),
        (Method::POST, "/v1/health"),
    ] {
        let reply = server.send(Call::new(method.clone(), path)).await;
        assert_eq!(
            reply.header("x-content-type-options"),
            Some("nosniff"),
            "{path}"
        );
        assert_eq!(
            reply.header("content-security-policy"),
            Some("default-src 'none'; frame-ancestors 'none'"),
            "{path}"
        );
        assert_eq!(
            reply.header("referrer-policy"),
            Some("no-referrer"),
            "{path}"
        );
        assert_eq!(reply.header("cache-control"), Some("no-store"), "{path}");
        assert!(reply.header("server").is_none(), "{path}");
        let id = reply.header("x-request-id").unwrap();
        assert!(id.starts_with("r_") && id.len() == 28, "{id}");
    }
}

/// HX4: without `server.cors_origins` no CORS header is ever sent; with
/// an allowlist, only its exact origins are answered; a wildcard is
/// refused by the configuration.
#[tokio::test]
async fn cors_only_with_an_allowlist() {
    let origin = "https://app.example.org";
    let server = TestServer::fresh();
    let preflight = Call::new(Method::OPTIONS, "/v1/info")
        .header("origin", origin)
        .header("access-control-request-method", "GET");
    let reply = server.send(preflight.clone()).await;
    assert!(reply.header("access-control-allow-origin").is_none());
    let reply = server
        .send(Call::get("/v1/info").header("origin", origin))
        .await;
    assert!(reply.header("access-control-allow-origin").is_none());

    let dirs = Dirs::new();
    let mut config = dirs.config();
    config.server.cors_origins = vec![origin.into()];
    let server = TestServer::in_process(dirs, config);
    let reply = server
        .send(Call::get("/v1/info").header("origin", origin))
        .await;
    assert_eq!(reply.header("access-control-allow-origin"), Some(origin));
    let reply = server.send(preflight).await;
    assert_eq!(reply.header("access-control-allow-origin"), Some(origin));
    let methods = reply.header("access-control-allow-methods").unwrap_or("");
    assert!(
        methods.contains("GET") && methods.contains("DELETE"),
        "{methods}"
    );
    let reply = server
        .send(Call::get("/v1/info").header("origin", "https://evil.example.org"))
        .await;
    assert!(reply.header("access-control-allow-origin").is_none());

    let mut wildcard = Dirs::new().config();
    wildcard.server.cors_origins = vec!["*".into()];
    assert!(wildcard.validate().is_err());
}

/// UP1: bytes enter only through the upload routes: they alone take a
/// body of raw bytes; every other body is JSON of a closed request type,
/// and no parameter, path or query, names a file or takes bytes.
#[tokio::test]
async fn bytes_enter_only_through_uploads() {
    for op in &OPERATIONS {
        let upload = op.body == RequestBody::Upload;
        assert_eq!(
            upload,
            matches!(op.id, "uploads_put" | "uploads_post"),
            "{}",
            op.id
        );
        let mut names: Vec<&str> = op.query.to_vec();
        names.extend(
            op.path
                .split('/')
                .filter_map(|s| s.strip_prefix('{')?.strip_suffix('}')),
        );
        for name in names {
            assert!(
                !["path", "file", "dir", "bytes", "content", "data"].contains(&name),
                "{} takes {name}",
                op.id
            );
        }
    }
    // A JSON body on an upload route is bytes like any other: kept as
    // given, never read as a request.
    let server = TestServer::fresh();
    let body = br#"{"path": "/etc/passwd"}"#;
    let reply = server
        .send(put_upload(body).header("content-type", "application/json"))
        .await;
    assert_eq!(reply.status, 201);
    assert_eq!(reply.json()["bytes"], body.len());
}

/// UP2, UP3: `PUT` keeps the bytes only under their own SHA-256 (`422
/// upload_digest_mismatch` otherwise, nothing kept); either case of hex
/// names them, written lower-case; the caller's own bytes again are `200`
/// with the lifetime extended; `POST` names them and says where;
/// `fitment.upload` states the caller's holdings; a wrong type or any
/// `Content-Encoding` is `415`; `DELETE` removes the row and, with nothing
/// else holding it, the object.
#[tokio::test]
async fn an_upload_is_named_by_its_sha256_and_kept_once() {
    let server = TestServer::fresh();
    let bytes = b"ISO-10303-21; a part\n";
    let sha256 = fitment_core::sha256_hex(bytes);
    let wrong = fitment_core::sha256_hex(b"other bytes");
    let mismatch = Call::new(Method::PUT, &format!("/v1/uploads/sha256:{wrong}"))
        .header("content-type", "model/step")
        .body(bytes);
    let reply = server.send(mismatch).await;
    assert_eq!(
        (reply.status.as_u16(), reply.code()),
        (422, "upload_digest_mismatch".into())
    );
    assert_eq!(reply.json()["details"]["sha256"], sha256.as_str());
    assert!(server.object_files().is_empty() && server.temporaries().is_empty());
    let upper = format!("/v1/uploads/sha256:{}", sha256.to_ascii_uppercase());
    let first = server
        .send(
            Call::new(Method::PUT, &upper)
                .header("content-type", "model/step")
                .body(bytes),
        )
        .await;
    assert_eq!(first.status, 201);
    let doc = first.json();
    assert_eq!(doc["format"], "fitment.upload");
    assert_eq!(doc["sha256"], sha256.as_str());
    assert_eq!(doc["bytes"], bytes.len());
    assert_eq!(doc["held_by"], serde_json::json!(["upload"]));
    assert_eq!(doc["pinned_by_jobs"], 0);
    let location = format!("/v1/uploads/sha256:{sha256}");
    assert_eq!(first.header("location"), Some(location.as_str()));
    assert_eq!(server.object_files(), std::slice::from_ref(&sha256));
    let seams = server.server().seams();
    let expires = |server: &TestServer| -> i64 {
        let conn = rusqlite::Connection::open(server.dirs.state.join("server.db")).unwrap();
        conn.query_row("SELECT expires_ms FROM upload", [], |r| r.get(0))
            .unwrap()
    };
    let before = expires(&server);
    seams.ids.fix_time(1_790_000_000_000 + 3_600_000);
    let again = server.send(put_upload(bytes)).await;
    assert_eq!(again.status, 200, "the caller's own bytes again");
    assert_eq!(
        expires(&server),
        before + 3_600_000,
        "the lifetime extended"
    );
    let posted = server
        .send(
            Call::new(Method::POST, "/v1/uploads")
                .header("content-type", "application/octet-stream")
                .body(bytes),
        )
        .await;
    assert_eq!(posted.status, 200);
    assert_eq!(posted.header("location"), Some(location.as_str()));
    assert_eq!(server.get(&location).await.json(), again.json());
    for (name, value) in [("content-type", "text/plain"), ("content-encoding", "gzip")] {
        let mut call = put_upload(bytes);
        call.headers.retain(|(n, _)| n != "content-type");
        let call = match name {
            "content-type" => call.header(name, value),
            _ => call
                .header("content-type", "model/step")
                .header(name, value),
        };
        assert_eq!(
            server.send(call).await.code(),
            "unsupported_media_type",
            "{name}"
        );
    }
    let missing = Call::new(Method::PUT, &location).body(bytes);
    assert_eq!(server.send(missing).await.code(), "unsupported_media_type");
    let deleted = server.send(Call::new(Method::DELETE, &location)).await;
    assert_eq!(deleted.status, 204);
    assert!(
        server.object_files().is_empty(),
        "the object went with its last row"
    );
    assert_eq!(server.get(&location).await.code(), "not_found");
    let gone = server.send(Call::new(Method::DELETE, &location)).await;
    assert_eq!(gone.code(), "not_found");
    assert_eq!(server.store_problems(), Vec::<String>::new());
}

/// UP2 (section 12's "done when"): every corpus file uploads with the
/// SHA-256 this test computes, by `PUT` and by `POST`. Release only, as
/// the corpus suites are.
#[tokio::test(flavor = "multi_thread")]
async fn every_corpus_file_uploads_with_its_sha256() {
    let Some(corpus) = fitment_corpus::Corpus::locate_release_only() else {
        return;
    };
    // Two requests a file, faster than AU7's 50 a second allow one user.
    let dirs = Dirs::new();
    let mut config = dirs.config();
    config.auth.rate_per_second = 100_000;
    config.auth.rate_burst = 100_000;
    let server = TestServer::in_process(dirs, config);
    let started = std::time::Instant::now();
    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(corpus.dir())
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "stp"))
        .collect();
    files.sort();
    assert!(files.len() > 400, "{} files", files.len());
    for file in &files {
        let bytes = std::fs::read(file).unwrap();
        let sha256 = fitment_core::sha256_hex(&bytes);
        let reply = server.send(put_upload(&bytes)).await;
        assert_eq!(reply.status, 201, "{}", file.display());
        assert_eq!(reply.json()["sha256"], sha256.as_str());
        assert_eq!(reply.json()["bytes"], bytes.len());
        let posted = Call::new(Method::POST, "/v1/uploads")
            .header("content-type", "model/step")
            .body(&bytes);
        let posted = server.send(posted).await;
        assert_eq!(posted.status, 200, "{}", file.display());
        let location = format!("/v1/uploads/sha256:{sha256}");
        assert_eq!(posted.header("location"), Some(location.as_str()));
        let path = server
            .dirs
            .state
            .join("objects/sha256")
            .join(&sha256[..2])
            .join(&sha256);
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }
    assert_eq!(server.temporaries(), Vec::<std::path::PathBuf>::new());
    eprintln!(
        "{} files uploaded twice in {:?}",
        files.len(),
        started.elapsed()
    );
}

/// OB3: an object leaves the store only checked: read into one buffer and
/// hashed against its name; a byte flipped is damaged and a removed file
/// missing; above the one-buffer cap it is hashed in one pass of the
/// streaming reader and opened again for the second, so damage stops it
/// before its first byte; both passes go through the seam (CA7).
#[tokio::test]
async fn objects_leave_the_store_only_checked() {
    let dirs = Dirs::new();
    let config = dirs.config();
    let seam = fitment_catalog::fs::CountingFs::new();
    let server = TestServer::in_process_with_fs(dirs, config, std::sync::Arc::new(seam.clone()));
    let bytes: Vec<u8> = (0..10_000_u32)
        .map(|i| u8::try_from(i % 251).unwrap())
        .collect();
    let ada = support::seed_user(
        server.server(),
        "ada",
        fitment_server::auth::Role::Reader,
        None,
    );
    let sha256 = support::seed_upload(server.server(), ada.key, &bytes);
    let objects = server.server().seams().objects;
    let n = u64::try_from(bytes.len()).unwrap();
    assert_eq!(objects.read(&sha256, n).unwrap(), bytes);
    let read_whole = |stream: &mut Box<dyn fitment_catalog::FsRead>| {
        let mut out = Vec::new();
        let mut buf = [0_u8; 4096];
        loop {
            let got = stream.read_chunk(&mut buf).unwrap();
            if got == 0 {
                return out;
            }
            out.extend_from_slice(&buf[..got]);
        }
    };
    objects.set_one_buffer_max(1000);
    assert!(
        matches!(
            objects.read(&sha256, n),
            Err(fitment_server::state::ObjectError::Damaged)
        ),
        "above the cap, never one buffer"
    );
    let calls = seam.calls().len();
    let mut stream = objects.open_checked(&sha256, n).unwrap();
    assert_eq!(read_whole(&mut stream), bytes);
    let streamed = seam.calls()[calls..]
        .iter()
        .filter(|c| matches!(c, fitment_catalog::fs::Call::OpenStream(_)))
        .count();
    assert_eq!(streamed, 2, "one pass to hash, one to send");
    let path = server
        .dirs
        .state
        .join("objects/sha256")
        .join(&sha256[..2])
        .join(&sha256);
    let mut damaged = bytes.clone();
    damaged[5000] ^= 1;
    let mut permissions = std::fs::metadata(&path).unwrap().permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    permissions.set_readonly(false);
    std::fs::set_permissions(&path, permissions).unwrap();
    std::fs::write(&path, &damaged).unwrap();
    assert!(matches!(
        objects.open_checked(&sha256, n),
        Err(fitment_server::state::ObjectError::Damaged)
    ));
    objects.set_one_buffer_max(fitment_server::state::objects::ONE_BUFFER_MAX);
    assert!(matches!(
        objects.read(&sha256, n),
        Err(fitment_server::state::ObjectError::Damaged)
    ));
    std::fs::write(&path, &bytes[..9000]).unwrap();
    assert!(matches!(
        objects.read(&sha256, n),
        Err(fitment_server::state::ObjectError::Damaged)
    ));
    std::fs::remove_file(&path).unwrap();
    assert!(matches!(
        objects.read(&sha256, n),
        Err(fitment_server::state::ObjectError::Missing)
    ));
    assert_eq!(
        server.store_problems().len(),
        1,
        "the row's file is missing"
    );
}
