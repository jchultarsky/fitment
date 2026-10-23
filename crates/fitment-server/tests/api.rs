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
/// `Content-Encoding` is `415`; `DELETE` removes the row, and the sweep
/// the object nothing else holds.
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
    assert_eq!(server.object_files().len(), 1, "the sweep's to unlink");
    server.server().sweep_store().unwrap();
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
/// missing; above the one-buffer cap `read` refuses it
/// (`TooLargeForBuffer`), and it is hashed in one pass of the streaming
/// reader and rewound on the same open file for the second, so damage
/// stops it before its first byte; both passes go through the seam (CA7).
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
            Err(fitment_server::state::ObjectError::TooLargeForBuffer)
        ),
        "above the cap, never one buffer"
    );
    let calls = seam.calls().len();
    let mut stream = objects.open_checked(&sha256, n).unwrap();
    assert_eq!(read_whole(&mut stream), bytes);
    let after = &seam.calls()[calls..];
    let count =
        |want: fn(&fitment_catalog::fs::Call) -> bool| after.iter().filter(|c| want(c)).count();
    let opens = count(|c| matches!(c, fitment_catalog::fs::Call::OpenStream(_)));
    let rewinds = count(|c| matches!(c, fitment_catalog::fs::Call::Rewind(_)));
    assert_eq!(
        (opens, rewinds),
        (1, 1),
        "one open file, rewound for the second pass"
    );
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

/// OB3 (L3 of PR 4's review): the second pass of a streamed object is
/// hashed again as it is read, so a file changed in place between the
/// passes fails the read that meets its end, and its sender aborts rather
/// than finish sending other bytes as the object.
#[tokio::test]
async fn a_file_changed_between_the_passes_fails_the_second() {
    let dirs = Dirs::new();
    let config = dirs.config();
    let object = std::sync::Arc::new(std::sync::Mutex::new(None::<std::path::PathBuf>));
    let seam = {
        let object = std::sync::Arc::clone(&object);
        fitment_catalog::fs::CountingFs::with_hook(move |call| {
            if matches!(call, fitment_catalog::fs::Call::Rewind(_)) {
                if let Some(path) = object.lock().unwrap().clone() {
                    use std::io::{Seek as _, Write as _};
                    let mut file = std::fs::OpenOptions::new().write(true).open(path).unwrap();
                    file.seek(std::io::SeekFrom::Start(10)).unwrap();
                    file.write_all(b"X").unwrap();
                }
            }
        })
    };
    let server = TestServer::in_process_with_fs(dirs, config, std::sync::Arc::new(seam));
    let ada = support::seed_user(
        server.server(),
        "ada",
        fitment_server::auth::Role::Reader,
        None,
    );
    let bytes = vec![b'b'; 5000];
    let sha256 = support::seed_upload(server.server(), ada.key, &bytes);
    let path = server
        .dirs
        .state
        .join("objects/sha256")
        .join(&sha256[..2])
        .join(&sha256);
    let mut permissions = std::fs::metadata(&path).unwrap().permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    permissions.set_readonly(false);
    std::fs::set_permissions(&path, permissions).unwrap();
    *object.lock().unwrap() = Some(path);
    let objects = server.server().seams().objects;
    objects.set_one_buffer_max(100);
    let mut stream = objects.open_checked(&sha256, 5000).unwrap();
    let mut buf = [0_u8; 1024];
    let error = loop {
        match stream.read_chunk(&mut buf) {
            Ok(0) => panic!("the changed file was sent whole"),
            Ok(_) => {}
            Err(error) => break error,
        }
    };
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
}

// ---------------------------------------------------------------- jobs

use std::time::Duration;

use serde_json::{Value, json};
use support::waiting;

const S02: &str = "s02_cbore.stp";

fn sha_of(bytes: &[u8]) -> String {
    fitment_core::sha256_hex(bytes)
}

/// JB1, JB3, RA9, JS11, DC9: a job is `202` with `Location` once its row
/// commits, queued at position 1 while the dispatcher holds; its request
/// is recorded normalized (the hex lower-case, defaults filled); it then
/// runs and ends `succeeded`, its times RFC 3339, its document linked.
#[tokio::test]
async fn a_job_is_queued_runs_and_ends() {
    let server = TestServer::fresh();
    let bytes = support::part("p01_through.stp");
    let sha = server.upload(&bytes).await;
    server.server().hold_jobs();
    let body = json!({ "upload": format!("sha256:{}", sha.to_ascii_uppercase()) });
    let reply = server
        .send(Call::json(Method::POST, "/v1/features", &body))
        .await;
    assert_eq!(reply.status, 202);
    let job = reply.json();
    let id = job["id"].as_str().unwrap().to_owned();
    assert_eq!(
        reply.header("location"),
        Some(format!("/v1/jobs/{id}").as_str())
    );
    assert!(fitment_server::ids::well_formed("j", &id));
    assert_eq!(
        (job["state"].as_str(), job["queue_position"].as_u64()),
        (Some("queued"), Some(1))
    );
    assert_eq!(job["request"], json!({ "upload": format!("sha256:{sha}") }));
    assert_eq!(job["request_sha256"].as_str().unwrap().len(), 64);
    assert!(job["exit_code"].is_null() && job["document"].is_null() && job["outcome"].is_null());
    server.server().resume_jobs();
    let done = server.finished_job(&id, Duration::from_secs(30)).await;
    assert_eq!(done["state"], "succeeded");
    assert_eq!(done["exit_code"], 0);
    assert!(done["queue_position"].is_null());
    assert!(done["times"]["finished"].as_str().unwrap().ends_with('Z'));
    assert_eq!(done["document"]["href"], format!("/v1/jobs/{id}/document"));
    assert_eq!(
        server.document(&done).await.body,
        support::cli_golden("features", "p01_through.json")
    );
}

/// VT40, RA4, RA5, HX5: a line's id is its data's (two servers, the same
/// bytes, the same id); extracting twice gives one line, the second
/// `unchanged` with the same revision; `sk_X` is the current revision
/// (`no-cache`), `sk_X@r1` exactly (`immutable`), `@r2` names nothing, and
/// references outside ID9's grammar are `400`.
#[tokio::test]
async fn a_lines_id_is_its_datas_and_extracting_twice_gives_one_line() {
    let bytes = support::assembly(S02);
    let sha = sha_of(&bytes);
    let mut hrefs = Vec::new();
    for _ in 0..2 {
        let server = TestServer::fresh();
        server.register(&bytes, S02).await;
        let first = server.extract(&sha, "BLOCK").await;
        let second = server.extract(&sha, "BLOCK").await;
        assert_eq!(first["state"], "succeeded");
        assert!(first["outcome"].is_null());
        assert_eq!(second["outcome"], "unchanged");
        assert_eq!(first["result"], second["result"]);
        let lines = server.get("/v1/sockets").await.json();
        assert_eq!(lines["items"].as_array().unwrap().len(), 1);
        let line = &lines["items"][0];
        assert_eq!(line["occurrence"], "*");
        assert_eq!(line["current"]["n"], 1);
        assert_eq!(line["current"]["how"], "extract");
        assert_eq!(line["current"]["verified"], true);
        let id = line["id"].as_str().unwrap();
        let bare = server.get(&format!("/v1/sockets/{id}")).await;
        assert_eq!(bare.header("cache-control"), Some("no-cache"));
        let exact = server.get(&format!("/v1/sockets/{id}@r1")).await;
        assert_eq!(
            exact.header("cache-control"),
            Some("private, max-age=31536000, immutable")
        );
        assert_eq!(exact.body, bare.body);
        assert_eq!(
            exact.body,
            support::cli_golden("socket", "s02_cbore--BLOCK.json")
        );
        assert_eq!(
            server.get(&format!("/v1/sockets/{id}@r2")).await.code(),
            "not_found"
        );
        for bad in ["@r0", "@r01", "@R1", "@1", "@r", "@r1x"] {
            let reply = server.get(&format!("/v1/sockets/{id}{bad}")).await;
            assert_eq!(reply.code(), "invalid_reference", "{bad}");
        }
        let history = server
            .get(&format!("/v1/sockets/{id}/revisions"))
            .await
            .json();
        assert_eq!(history["items"].as_array().unwrap().len(), 1);
        hrefs.push(first["result"]["href"].clone());
    }
    assert_eq!(hrefs[0], hrefs[1], "RA4: the same data, the same id");
}

/// The line's revisions, by number.
async fn numbers(server: &TestServer, id: &str) -> Vec<i64> {
    server
        .get(&format!("/v1/sockets/{id}/revisions"))
        .await
        .json()["items"]
        .as_array()
        .map(|a| a.iter().map(|r| r["n"].as_i64().unwrap()).collect())
        .unwrap_or_default()
}

/// VT49, RA4, RA5, RA6, OB4: a deleted line is a tombstone, its revisions
/// still readable by number until the sweep, which deletes those no job
/// cites; extracting its triple again revives the id as a new live row,
/// its revisions numbered on from the id's last, kept or swept, and
/// carrying no decision of the deleted row's.
#[tokio::test]
async fn a_deleted_line_revives_numbered_on() {
    let server = TestServer::fresh();
    let bytes = support::assembly(S02);
    let sha = sha_of(&bytes);
    server.register(&bytes, S02).await;
    let first = server.extract(&sha, "BLOCK").await;
    let href = first["result"]["href"].as_str().unwrap();
    let id = href
        .trim_start_matches("/v1/sockets/")
        .split('@')
        .next()
        .unwrap()
        .to_owned();
    let body = json!({ "assembly": format!("sha256:{sha}"), "part": "BLOCK", "carry": false });
    let again = server.send(waiting("/v1/sockets", &body)).await.json();
    assert_eq!(again["outcome"], "unchanged", "the same unreviewed bytes");
    let delete = Call::new(Method::DELETE, &format!("/v1/sockets/{id}"));
    assert_eq!(server.send(delete.clone()).await.status, 204);
    assert_eq!(
        server.get(&format!("/v1/sockets/{id}")).await.code(),
        "not_found"
    );
    assert_eq!(
        server.get(&format!("/v1/sockets/{id}@r1")).await.status,
        200,
        "RA6"
    );
    let live = server.get("/v1/sockets").await.json();
    assert_eq!(live["items"], json!([]));
    let revived = server.extract(&sha, "BLOCK").await;
    assert_eq!(revived["result"]["href"], format!("/v1/sockets/{id}@r2"));
    assert_eq!(numbers(&server, &id).await, [1, 2]);
    assert_eq!(server.send(delete.clone()).await.status, 204);
    let (_, revisions) = server.server().sweep_jobs().unwrap();
    assert_eq!(revisions, 2, "no job cites them");
    assert_eq!(
        server.get(&format!("/v1/sockets/{id}@r1")).await.code(),
        "not_found"
    );
    let third = server.extract(&sha, "BLOCK").await;
    assert_eq!(
        third["result"]["href"],
        format!("/v1/sockets/{id}@r3"),
        "numbered on"
    );
    let swept = server.server().sweep_store().unwrap();
    assert!(
        swept.unlinked.is_empty(),
        "the live revision holds the bytes"
    );
}

/// OB4, RA9, review S8 of PR 5: a revision an extraction carries from is
/// cited from its submission, so a sweep while the job is queued keeps it,
/// its line deleted; the job runs on it and succeeds.
#[tokio::test]
async fn a_carried_revision_stays_until_its_job_ends() {
    let server = TestServer::fresh();
    let bytes = support::assembly(S02);
    let sha = sha_of(&bytes);
    server.register(&bytes, S02).await;
    let first = server.extract(&sha, "BLOCK").await;
    let href = first["result"]["href"].as_str().unwrap();
    let carried = href.trim_start_matches("/v1/sockets/").to_owned();
    let id = carried.split('@').next().unwrap().to_owned();
    server.server().hold_jobs();
    let body = json!({
        "assembly": format!("sha256:{sha}"),
        "part": "BLOCK",
        "carry_from": carried,
    });
    let queued = server
        .send(Call::json(Method::POST, "/v1/sockets", &body))
        .await;
    assert_eq!(queued.status, 202);
    let delete = Call::new(Method::DELETE, &format!("/v1/sockets/{id}"));
    assert_eq!(server.send(delete).await.status, 204);
    let (_, revisions) = server.server().sweep_jobs().unwrap();
    assert_eq!(revisions, 0, "the queued job cites it");
    assert_eq!(
        server.get(&format!("/v1/sockets/{carried}")).await.status,
        200
    );
    server.server().resume_jobs();
    let job = server
        .finished_job(
            queued.json()["id"].as_str().unwrap(),
            Duration::from_secs(30),
        )
        .await;
    assert_eq!(job["state"], "succeeded", "{job}");
}

/// VT42, JB9: a key binds the request as sent: the same body again, or
/// with its keys reordered and its spacing changed, is the same job (`202`
/// while it runs, `200` once it ended, and once its upload is gone, review
/// S4 of PR 5); another body is `422`; a key whose job was cancelled is
/// released, so it makes a new job.
#[tokio::test]
async fn a_key_binds_the_request_as_sent() {
    let server = TestServer::fresh();
    let sha = server.upload(&support::part("p01_through.stp")).await;
    server.server().hold_jobs();
    let keyed = |body: &str, key: &str| {
        Call::new(Method::POST, "/v1/features")
            .header("content-type", "application/json")
            .header("idempotency-key", key)
            .body(body.as_bytes())
    };
    let body = format!(r#"{{"upload": "sha256:{sha}"}}"#);
    let first = server.send(keyed(&body, "k-1")).await;
    assert_eq!(first.status, 202);
    let spaced = format!("{{ \"upload\" :\n \"sha256:{sha}\" }}");
    let replay = server.send(keyed(&spaced, "k-1")).await;
    assert_eq!(replay.status, 202);
    assert_eq!(replay.json()["id"], first.json()["id"]);
    let other = format!(r#"{{"upload": "sha256:{}"}}"#, sha.to_ascii_uppercase());
    assert_eq!(
        server.send(keyed(&other, "k-1")).await.code(),
        "idempotency_key_reused"
    );
    let id = first.json()["id"].as_str().unwrap().to_owned();
    server.server().resume_jobs();
    server.finished_job(&id, Duration::from_secs(30)).await;
    let ended = server.send(keyed(&body, "k-1")).await;
    assert_eq!(
        (ended.status.as_u16(), ended.json()["id"].as_str()),
        (200, Some(id.as_str()))
    );
    // Review S4 of PR 5: the replay stands once its input is gone.
    let delete = Call::new(Method::DELETE, &format!("/v1/uploads/sha256:{sha}"));
    assert_eq!(server.send(delete).await.status, 204);
    let gone = server.send(keyed(&body, "k-1")).await;
    assert_eq!(
        (gone.status.as_u16(), gone.json()["id"].as_str()),
        (200, Some(id.as_str())),
        "{}",
        String::from_utf8_lossy(&gone.body)
    );
    let sha = server.upload(&support::part("p01_through.stp")).await;
    assert_eq!(body, format!(r#"{{"upload": "sha256:{sha}"}}"#));
    server.server().hold_jobs();
    let second = server.send(keyed(&body, "k-2")).await.json();
    let cancel = format!("/v1/jobs/{}/cancel", second["id"].as_str().unwrap());
    assert_eq!(
        server.send(Call::new(Method::POST, &cancel)).await.json()["state"],
        "cancelled"
    );
    let fresh = server.send(keyed(&body, "k-2")).await.json();
    assert_ne!(
        fresh["id"], second["id"],
        "a cancelled job's key is released"
    );
    for bad in ["", &"k".repeat(129), "tab\there"] {
        let refused = server.send(keyed(&body, bad)).await;
        assert_eq!(refused.code(), "invalid_parameter", "{bad:?}");
    }
    server.server().resume_jobs();
}

/// VN5, review F6: a job's ETag is `W/"j<seq>.<state>"`, its last persisted
/// event and its state, `.p` when pinned and `.c` when a cancel was asked
/// (review S5 of PR 5): `If-None-Match` answers `304` until a persisted
/// event, the state or a pin changes it; progress, which is transient,
/// changes nothing.
#[tokio::test]
async fn a_jobs_etag_follows_its_persisted_events() {
    let server = TestServer::fresh();
    let sha = server.upload(&support::part("p01_through.stp")).await;
    server.server().hold_jobs();
    let job = server
        .send(Call::json(
            Method::POST,
            "/v1/features",
            &json!({ "upload": format!("sha256:{sha}") }),
        ))
        .await;
    let etag = job.header("etag").unwrap().to_owned();
    assert_eq!(etag, "W/\"j1.queued\"");
    let id = job.json()["id"].as_str().unwrap().to_owned();
    let path = format!("/v1/jobs/{id}");
    let again = server
        .send(Call::get(&path).header("if-none-match", &etag))
        .await;
    assert_eq!(again.status, 304);
    server.server().resume_jobs();
    let done = server.finished_job(&id, Duration::from_secs(30)).await;
    assert_eq!(done["state"], "succeeded");
    let after = server
        .send(Call::get(&path).header("if-none-match", &etag))
        .await;
    assert_eq!(after.status, 200);
    let done = after.header("etag").unwrap().to_owned();
    assert_eq!(done, "W/\"j4.succeeded\"");
    // Review S5 of PR 5: pinning, which writes no event, changes it too.
    let pin = Call::json(
        Method::POST,
        &format!("{path}/pin"),
        &json!({ "pinned": true }),
    );
    assert_eq!(server.send(pin).await.json()["pinned"], true);
    let pinned = server
        .send(Call::get(&path).header("if-none-match", &done))
        .await;
    assert_eq!(pinned.status, 200);
    assert_eq!(pinned.header("etag"), Some("W/\"j4.succeeded.p\""));
}

/// The keys an event may carry: none states a verdict (M6-51).
const EVENT_KEYS: [&str; 17] = [
    "format",
    "format_version",
    "job",
    "seq",
    "type",
    "state",
    "attempt",
    "exit_code",
    "outcome",
    "result",
    "document",
    "error",
    "phase",
    "done",
    "total",
    "missed",
    "generation",
];

/// VT43, JB8, JS10, M6-51: `?after=N` replays every persisted event after
/// N, then live ones, without loss; persisted events are numbered 1, 2, 3
/// … without gaps and transient ones carry no `seq`; an idle stream gets a
/// heartbeat (the seam's short interval); no event carries a verdict.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn events_replay_then_follow_without_loss() {
    let server = TestServer::fresh();
    let sha = server.upload(&support::part("p01_through.stp")).await;
    server
        .server()
        .seams()
        .jobs
        .set_heartbeat(Duration::from_millis(50));
    server.server().hold_jobs();
    let job = server
        .send(Call::json(
            Method::POST,
            "/v1/features",
            &json!({ "upload": format!("sha256:{sha}") }),
        ))
        .await
        .json();
    let id = job["id"].as_str().unwrap().to_owned();
    let mut stream = server
        .open(Call::get(&format!("/v1/jobs/{id}/events")))
        .await;
    assert_eq!(
        stream.headers().get("content-type").unwrap(),
        "application/x-ndjson"
    );
    let body = stream.body_mut();
    let idle = support::lines(body, Duration::from_secs(10), |e| e["type"] == "heartbeat").await;
    assert_eq!(idle[0]["seq"], 1);
    assert!(
        idle.last().unwrap().get("seq").is_none(),
        "a heartbeat has no seq"
    );
    server.server().resume_jobs();
    let mut all = idle;
    all.extend(support::lines(body, Duration::from_secs(30), |e| e["type"] == "end").await);
    let seqs: Vec<i64> = all.iter().filter_map(|e| e["seq"].as_i64()).collect();
    assert_eq!(
        seqs,
        (1..=i64::try_from(seqs.len()).unwrap()).collect::<Vec<_>>()
    );
    for event in &all {
        assert_eq!(event["format"], "fitment.job.event");
        assert_eq!(event["job"], id.as_str());
        for key in event.as_object().unwrap().keys() {
            assert!(
                EVENT_KEYS.contains(&key.as_str()),
                "M6-51: {key} in {event}"
            );
        }
        if ["progress", "heartbeat", "lagged", "shutdown"]
            .contains(&event["type"].as_str().unwrap())
        {
            assert!(event.get("seq").is_none(), "review F6: {event}");
        }
    }
    let mut resumed = server
        .open(Call::get(&format!("/v1/jobs/{id}/events?after=2")))
        .await;
    let replayed = support::lines(resumed.body_mut(), Duration::from_secs(10), |e| {
        e["type"] == "end"
    })
    .await;
    let tail: Vec<i64> = replayed.iter().filter_map(|e| e["seq"].as_i64()).collect();
    assert_eq!(tail, seqs[2..].to_vec());
    drop(resumed);
    let start = std::time::Instant::now();
    while server.server().seams().jobs.channels_open() > 0 {
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "a channel stayed open"
        );
        tokio::task::yield_now().await;
    }
}

/// Waits up to ten seconds for every live-event channel to be dropped.
async fn channels_close(server: &TestServer) {
    let start = std::time::Instant::now();
    while server.server().seams().jobs.channels_open() > 0 {
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "a channel stayed open"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// JS10, review S9 of PR 5: a stream of a job that ended, `after` at or
/// past its `end` event, ends at once: no heartbeat, no channel left.
#[tokio::test]
async fn a_stream_past_a_finished_jobs_end_closes() {
    let server = TestServer::fresh();
    let sha = server.upload(&support::part("p01_through.stp")).await;
    server
        .server()
        .seams()
        .jobs
        .set_heartbeat(Duration::from_millis(50));
    let body = json!({ "upload": format!("sha256:{sha}") });
    let job = server
        .send(support::waiting("/v1/features", &body))
        .await
        .json();
    let id = job["id"].as_str().unwrap().to_owned();
    let mut all = server
        .open(Call::get(&format!("/v1/jobs/{id}/events")))
        .await;
    let events = support::lines(all.body_mut(), Duration::from_secs(10), |e| {
        e["type"] == "end"
    })
    .await;
    let end = events.last().unwrap()["seq"].as_i64().unwrap();
    for after in [end, end + 5] {
        let start = std::time::Instant::now();
        let mut stream = server
            .open(Call::get(&format!("/v1/jobs/{id}/events?after={after}")))
            .await;
        let lines = support::lines(stream.body_mut(), Duration::from_secs(5), |_| false).await;
        assert!(
            lines.is_empty(),
            "after={after}: {} lines, the first {:?}",
            lines.len(),
            lines.first()
        );
        assert!(
            start.elapsed() < Duration::from_secs(4),
            "after={after}: it ended"
        );
    }
    channels_close(&server).await;
}

/// JS10, review S10 of PR 5: a client that leaves while its stream's
/// replay is still being sent (more lines than its body buffers) leaves
/// no channel open.
#[tokio::test]
async fn a_stream_dropped_during_its_replay_leaves_no_channel() {
    let server = TestServer::fresh();
    let sha = server.upload(&support::part("p01_through.stp")).await;
    server.server().hold_jobs();
    let queued = server
        .send(Call::json(
            Method::POST,
            "/v1/features",
            &json!({ "upload": format!("sha256:{sha}") }),
        ))
        .await
        .json();
    let id = queued["id"].as_str().unwrap().to_owned();
    {
        let mut conn = rusqlite::Connection::open(server.dirs.state.join("server.db")).unwrap();
        let tx = conn.transaction().unwrap();
        let key: i64 = tx
            .query_row("SELECT key FROM job WHERE id = ?1", [&id], |r| r.get(0))
            .unwrap();
        let note = format!(r#"{{"text":"{}"}}"#, "n".repeat(100));
        for seq in 2..100_i64 {
            tx.execute(
                "INSERT INTO job_event (job, seq, at_ms, type, data) VALUES (?1, ?2, 0, 'note', ?3)",
                rusqlite::params![key, seq, note],
            )
            .unwrap();
        }
        tx.commit().unwrap();
    }
    let stream = server
        .open(Call::get(&format!("/v1/jobs/{id}/events")))
        .await;
    assert_eq!(stream.status(), 200);
    tokio::time::sleep(Duration::from_millis(50)).await;
    drop(stream);
    channels_close(&server).await;
    server.server().resume_jobs();
}

/// RA3, RA6, RA8: an assembly is registered once per caller: again, its
/// name added, with no job; the list and the document show it; its bytes
/// download re-hashed, named by their hash (HX6); a refused assembly is
/// registered too, `refused`; one a live line names is `409
/// assembly_in_use` to `DELETE`, and once the line is gone `204`.
#[tokio::test]
async fn assemblies_are_the_callers_registered_bytes() {
    let server = TestServer::fresh();
    let bytes = support::assembly(S02);
    let sha = sha_of(&bytes);
    let job = server.register(&bytes, S02).await;
    assert_eq!(
        job["result"]["href"],
        format!("/v1/assemblies/sha256:{sha}")
    );
    let again = server
        .send(waiting(
            "/v1/assemblies",
            &json!({ "upload": format!("sha256:{sha}"), "file_name": "base.stp" }),
        ))
        .await;
    assert_eq!(again.json()["format"], "fitment.assembly", "no job");
    assert_eq!(again.json()["file_names"], json!(["base.stp", S02]));
    assert_eq!(again.json()["status"], "accepted");
    assert_eq!(again.json()["parts"]["current"], true);
    let step = server
        .get(&format!("/v1/assemblies/sha256:{sha}/step"))
        .await;
    assert_eq!(step.body, bytes);
    assert_eq!(
        step.header("content-type"),
        Some("application/octet-stream")
    );
    assert_eq!(
        step.header("content-disposition"),
        Some(format!("attachment; filename=\"{}.stp\"", &sha[..12]).as_str())
    );
    let refused = server.register(b"no assembly here\n", "x.stp").await;
    assert_eq!(
        (refused["state"].as_str(), refused["exit_code"].as_u64()),
        (Some("refused"), Some(3))
    );
    let listed = server.get("/v1/assemblies").await.json();
    assert_eq!(listed["items"].as_array().unwrap().len(), 2);
    server.extract(&sha, "BLOCK").await;
    let path = format!("/v1/assemblies/sha256:{sha}");
    assert_eq!(server.get(&path).await.json()["sockets"]["count"], 1);
    let lines = server.get(&format!("{path}/sockets")).await.json();
    let id = lines["items"][0]["id"].as_str().unwrap().to_owned();
    assert_eq!(
        server.send(Call::new(Method::DELETE, &path)).await.code(),
        "assembly_in_use"
    );
    server
        .send(Call::new(Method::DELETE, &format!("/v1/sockets/{id}")))
        .await;
    assert_eq!(
        server.send(Call::new(Method::DELETE, &path)).await.status,
        204
    );
    assert_eq!(server.get(&path).await.code(), "not_found");
}

/// DC4, review of PR 5: an extraction `unchanged`, which made no
/// revision, serves its document with the `Fitment-Digest` of the
/// revision its `result` names, as the one that made it does.
#[tokio::test]
async fn an_unchanged_extraction_carries_its_revisions_digest() {
    let server = TestServer::fresh();
    let bytes = support::assembly(S02);
    let sha = sha_of(&bytes);
    server.register(&bytes, S02).await;
    let first = server.extract(&sha, "BLOCK").await;
    let body = json!({ "assembly": format!("sha256:{sha}"), "part": "BLOCK", "carry": false });
    let again = server.send(waiting("/v1/sockets", &body)).await.json();
    assert_eq!(again["outcome"], "unchanged");
    assert_eq!(again["result"], first["result"]);
    let golden = support::cli_golden("socket", "s02_cbore--BLOCK.json");
    let digest: Value = serde_json::from_slice(&golden).unwrap();
    for job in [&first, &again] {
        let path = format!("/v1/jobs/{}/document", job["id"].as_str().unwrap());
        let document = server.get(&path).await;
        assert_eq!(document.body, golden);
        assert_eq!(
            document.header("fitment-digest"),
            digest["digest"].as_str(),
            "{}",
            job["outcome"]
        );
    }
}

/// DC1, DC4 to DC6, VN5: a document is served with `Fitment-Format`,
/// `Fitment-Exit-Code`, a strong `ETag` and `Repr-Digest` of the body, and
/// for a socket `Fitment-Digest`, its own digest (two digests, two
/// meanings: DC5); `If-None-Match` answers `304`; `Accept-Encoding: gzip`
/// gets the same bytes gzipped, its ETag `-gz`.
#[tokio::test]
async fn documents_carry_their_headers_and_compress_on_request() {
    let server = TestServer::fresh();
    let bytes = support::assembly(S02);
    let sha = sha_of(&bytes);
    server.register(&bytes, S02).await;
    let job = server.extract(&sha, "BLOCK").await;
    let href = job["result"]["href"].as_str().unwrap().to_owned();
    let plain = server.get(&href).await;
    let golden = support::cli_golden("socket", "s02_cbore--BLOCK.json");
    assert_eq!(plain.body, golden);
    let doc: Value = serde_json::from_slice(&plain.body).unwrap();
    assert_eq!(plain.header("fitment-format"), Some("fitment.socket/1"));
    assert_eq!(plain.header("fitment-exit-code"), Some("0"));
    assert_eq!(plain.header("fitment-digest"), doc["digest"].as_str());
    assert_eq!(
        plain.header("etag"),
        Some(format!("\"sha256:{}\"", sha_of(&golden)).as_str())
    );
    assert_eq!(
        plain.header("repr-digest"),
        Some(fitment_server::http::json::repr_digest(&golden).as_str())
    );
    let cached = server
        .send(Call::get(&href).header("if-none-match", plain.header("etag").unwrap()))
        .await;
    assert_eq!(cached.status, 304);
    let zipped = server
        .send(Call::get(&href).header("accept-encoding", "gzip"))
        .await;
    assert_eq!(zipped.header("content-encoding"), Some("gzip"));
    assert_eq!(
        zipped.header("etag"),
        Some(format!("\"sha256:{}-gz\"", sha_of(&golden)).as_str())
    );
    let mut unzipped = Vec::new();
    std::io::Read::read_to_end(
        &mut flate2::read::GzDecoder::new(zipped.body.as_slice()),
        &mut unzipped,
    )
    .unwrap();
    assert_eq!(unzipped, golden, "DC6: transport only");
    let refused = server
        .send(Call::get(&href).header("accept-encoding", "gzip;q=0"))
        .await;
    assert!(refused.header("content-encoding").is_none());
}

/// DC2, DC8: a job's document holds nothing of the server (no job or user
/// id, no time, no path), and the server and the CLI are one build.
#[tokio::test]
async fn documents_hold_nothing_of_the_server_and_one_build_writes_them() {
    let server = TestServer::fresh();
    let bytes = support::assembly(S02);
    let job = server.register(&bytes, S02).await;
    let document = server.document(&job).await;
    let text = String::from_utf8(document.body).unwrap();
    for needle in [
        job["id"].as_str().unwrap(),
        job["owner"].as_str().unwrap(),
        "2026-",
        "/v1/",
    ] {
        assert!(!text.contains(needle), "DC2: {needle}");
    }
    let (_, version, _) = support::cli(&server.dirs.root, &["--version"]);
    let info = server.get("/v1/info").await.json();
    assert_eq!(
        String::from_utf8(version).unwrap().trim(),
        format!("fitment {}", info["server"]["version"].as_str().unwrap())
    );
}

/// OP6: the exposition counts requests by route template and jobs by
/// kind and state, and gauges the lanes and permits; labels are closed
/// sets.
#[tokio::test]
async fn metrics_count_requests_and_jobs() {
    let server = TestServer::fresh();
    let sha = server.upload(&support::part("p01_through.stp")).await;
    let job = server
        .send(waiting(
            "/v1/features",
            &json!({ "upload": format!("sha256:{sha}") }),
        ))
        .await;
    assert_eq!(job.json()["state"], "succeeded");
    let server_ref = server.server().clone();
    let text = tokio::task::spawn_blocking(move || server_ref.metrics_text())
        .await
        .unwrap();
    assert!(
        text.contains("fitment_jobs_total{kind=\"features\",state=\"succeeded\"} 1"),
        "{text}"
    );
    assert!(
        text.contains("fitment_http_requests_total{route=\"/v1/features\",code_class=\"2xx\"} 1")
    );
    assert!(text.contains("fitment_jobs_running{lane=\"query\"} 0"));
    assert!(text.contains("fitment_cpu_permits_free "));
    assert!(text.contains("fitment_catalog_generation "));
    assert!(!text.contains(&sha), "no raw path in a label");
}

/// The committed BRACKET candidates of s01, uploaded: their `files`.
async fn bracket_files(server: &TestServer) -> Vec<Value> {
    let mut files = Vec::new();
    for name in &support::candidates_by_socket()[&("s01_bolted".to_owned(), "BRACKET".to_owned())] {
        let sha = server.upload(&support::candidate(name)).await;
        files.push(json!({ "upload": format!("sha256:{sha}"), "label": name }));
    }
    files
}

/// Makes the stored object `sha256` writable and flips one byte at `at`.
fn damage(server: &TestServer, sha256: &str, at: usize) {
    let path = server
        .dirs
        .state
        .join(format!("objects/sha256/{}/{sha256}", &sha256[..2]));
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    perms.set_readonly(false);
    std::fs::set_permissions(&path, perms).unwrap();
    let mut bytes = std::fs::read(&path).unwrap();
    bytes[at] ^= 1;
    std::fs::write(&path, bytes).unwrap();
}

/// VT46, DC12, SN5: a run's candidates paged two at a time are its
/// document's own `candidates` elements, in order, each the same JSON as
/// the document's and every page stating the document's digest, `view`,
/// `complete` and `exit_code`; paging scans no document (a count the
/// server keeps); a byte flipped inside one candidate's bytes makes that
/// candidate's page `500 blob_damaged` while the others still serve.
#[tokio::test]
async fn candidate_pages_are_the_documents_bytes() {
    let server = TestServer::fresh();
    let socket = server.socket_of("s01_bolted", "BRACKET").await;
    let files = bracket_files(&server).await;
    let job = server
        .run_match(&json!({ "socket": socket, "candidates": { "files": files } }))
        .await;
    assert_eq!(job["state"], "succeeded", "{job}");
    let id = job["id"].as_str().unwrap();
    let document = server.get(&format!("/v1/matches/{id}/document")).await;
    let doc: Value = serde_json::from_slice(&document.body).unwrap();
    let candidates = doc["candidates"].as_array().unwrap();
    assert!(candidates.len() >= 5);
    let scanned = server.server().seams().jobs.documents_indexed();
    let mut next = Some(format!("/v1/matches/{id}/candidates?limit=2"));
    let mut seen = Vec::new();
    while let Some(url) = next {
        let page = server.get(&url).await;
        assert_eq!(page.status, 200);
        let value = page.json();
        assert_eq!(
            (value["view"].as_bool(), value["complete"].as_bool()),
            (Some(true), Some(true))
        );
        assert_eq!(value["digest"], doc["digest"]);
        assert_eq!(value["exit_code"], job["exit_code"]);
        for item in value["items"].as_array().unwrap() {
            assert_eq!(item["index"].as_u64().unwrap(), seen.len() as u64);
            seen.push(item["entry"].clone());
        }
        next = value["next"].as_str().map(str::to_owned);
    }
    assert_eq!(
        &seen, candidates,
        "the pages hold the document's candidates in order"
    );
    assert_eq!(
        server.server().seams().jobs.documents_indexed(),
        scanned,
        "no page scans"
    );
    let ranges = fitment_server::runs::index(&document.body).unwrap();
    for (range, element) in ranges.iter().zip(candidates) {
        let at = |n: u64| usize::try_from(n).unwrap();
        let slice = &document.body[at(range.start)..at(range.end)];
        let read: Value = serde_json::from_slice(slice).unwrap();
        assert_eq!(&read, element, "each range is one whole element");
    }
    let sha = document
        .header("etag")
        .unwrap()
        .trim_matches('"')
        .trim_start_matches("sha256:")
        .to_owned();
    damage(
        &server,
        &sha,
        usize::try_from(ranges[1].start).unwrap() + 10,
    );
    let broken = server
        .get(&format!("/v1/matches/{id}/candidates?offset=1&limit=1"))
        .await;
    assert_eq!(
        (broken.status.as_u16(), broken.code()),
        (500, "blob_damaged".to_owned())
    );
    let whole = server
        .get(&format!("/v1/matches/{id}/candidates?offset=0&limit=1"))
        .await;
    assert_eq!(whole.status, 200, "the other candidates still serve");
}

/// DC12 (review of M6 PR 6): a candidate page reads its own bytes: on the
/// seam, its document is opened once and sought to the page's first
/// candidate, and the bytes read are that candidate's, not the document's
/// before it.
#[tokio::test]
async fn a_candidate_page_seeks_to_its_bytes() {
    let dirs = Dirs::new();
    let config = dirs.config();
    let seam = fitment_catalog::fs::CountingFs::new();
    let server = TestServer::in_process_with_fs(dirs, config, std::sync::Arc::new(seam.clone()));
    let socket = server.socket_of("s01_bolted", "BRACKET").await;
    let files = bracket_files(&server).await;
    let job = server
        .run_match(&json!({ "socket": socket, "candidates": { "files": files } }))
        .await;
    let id = job["id"].as_str().unwrap();
    let document = server.get(&format!("/v1/matches/{id}/document")).await;
    let ranges = fitment_server::runs::index(&document.body).unwrap();
    let last = ranges.len() - 1;
    let before = seam.calls().len();
    let page = server
        .get(&format!(
            "/v1/matches/{id}/candidates?offset={last}&limit=1"
        ))
        .await;
    assert_eq!(page.status, 200);
    let calls = &seam.calls()[before..];
    let sought: Vec<u64> = calls
        .iter()
        .filter_map(|c| match c {
            fitment_catalog::fs::Call::Seek(_, at) => Some(*at),
            _ => None,
        })
        .collect();
    assert_eq!(sought, vec![ranges[last].start], "{calls:?}");
    let room: usize = calls
        .iter()
        .filter_map(|c| match c {
            fitment_catalog::fs::Call::ReadChunk(path, n)
                if path
                    .to_string_lossy()
                    .contains(&job["document"]["sha256"].as_str().unwrap()[..16]) =>
            {
                Some(*n)
            }
            _ => None,
        })
        .sum();
    let own = usize::try_from(ranges[last].end - ranges[last].start).unwrap();
    assert_eq!(room, own, "only the candidate's own bytes are read");
}

/// The job `matched`, over `candidate` against `file`, is `fitment match
/// S.json m_block02.stp --format json`'s: exit code and bytes.
async fn the_clis_match(server: &TestServer, file: &[u8], candidate: &[u8], matched: &Value) {
    let dir = server.dirs.root.join("cli");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("S.json"), file).unwrap();
    std::fs::write(dir.join("m_block02.stp"), candidate).unwrap();
    let (code, stdout, _) = support::cli(
        &dir,
        &["match", "S.json", "m_block02.stp", "--format", "json"],
    );
    assert_eq!(matched["exit_code"].as_i64(), code.map(i64::from));
    assert_eq!(
        server.document(matched).await.body,
        stdout,
        "the CLI's document either way"
    );
}

/// `file`, a socket document, with one value edited and its digest
/// recomputed: a forgery no extraction reproduces.
fn forged(file: &[u8]) -> Vec<u8> {
    let mut edited: Value = serde_json::from_slice(file).unwrap();
    edited["extracted"]["sockets"][0]["requirements"][0]["floor_mm"] = json!(0.5);
    let mut stated = edited.clone();
    for key in ["digest", "review", "stale_reviews"] {
        stated.as_object_mut().unwrap().remove(key);
    }
    edited["digest"] = json!(format!("sha256:{}", sha_of(stated.to_string().as_bytes())));
    serde_json::to_vec_pretty(&edited).unwrap()
}

/// VT47, RA10, RA5, UP6: an import of the CLI's socket file with its
/// assembly registered is a verified revision; the file with one value
/// edited and its digest recomputed ends `refused`, exit 3,
/// `socket_not_reproduced`, no revision written; without the assembly the
/// revision is `verified: false`, which its listing and `fitment.run`
/// state, and a match on it carries the note `unverified_socket` while its
/// document is the CLI's for the same file; registering the assembly and
/// importing again makes a new, verified revision of the same bytes.
#[tokio::test]
async fn imports_are_verified_or_marked() {
    let server = TestServer::fresh();
    let file = support::cli_golden("socket", "s02_cbore--BLOCK.json");
    let upload = server.upload(&file).await;
    let import = |sha: &str| {
        waiting(
            "/v1/sockets/import",
            &json!({ "upload": format!("sha256:{sha}") }),
        )
    };
    // Without the assembly: marked.
    let job = server.send(import(&upload)).await.json();
    assert_eq!(job["state"], "succeeded", "{job}");
    let href = job["result"]["href"].as_str().unwrap().to_owned();
    let line = href
        .trim_start_matches("/v1/sockets/")
        .split('@')
        .next()
        .unwrap()
        .to_owned();
    let revisions = server
        .get(&format!("/v1/sockets/{line}/revisions"))
        .await
        .json();
    assert_eq!(revisions["items"][0]["how"], "import");
    assert_eq!(revisions["items"][0]["verified"], false);
    assert_eq!(server.get(&href).await.body, file, "the imported bytes");
    let candidate = support::candidate("m_block02.stp");
    let sha = server.upload(&candidate).await;
    let body = json!({ "socket": format!("{line}@r1"), "candidates": { "files": [
        { "upload": format!("sha256:{sha}"), "label": "m_block02.stp" } ] } });
    let matched = server.run_match(&body).await;
    let codes: Vec<&str> = matched["notes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["code"].as_str().unwrap())
        .collect();
    assert_eq!(codes.last(), Some(&"unverified_socket"));
    let run = server
        .get(&format!("/v1/matches/{}", matched["id"].as_str().unwrap()))
        .await
        .json();
    assert_eq!(
        (
            run["socket"]["how"].as_str(),
            run["socket"]["verified"].as_bool()
        ),
        (Some("import"), Some(false))
    );
    the_clis_match(&server, &file, &candidate, &matched).await;
    // With the assembly registered: verified, a new revision of the same bytes.
    server.register(&support::assembly(S02), S02).await;
    let again = server.send(import(&upload)).await.json();
    assert_eq!(
        again["result"]["href"],
        format!("/v1/sockets/{line}@r2"),
        "{again}"
    );
    let revisions = server
        .get(&format!("/v1/sockets/{line}/revisions"))
        .await
        .json();
    assert_eq!(revisions["items"][1]["verified"], true);
    assert_eq!(
        revisions["items"][1]["sha256"],
        revisions["items"][0]["sha256"]
    );
    let unchanged = server.send(import(&upload)).await.json();
    assert_eq!(unchanged["outcome"], "unchanged");
    // A value edited, the digest recomputed: not reproduced, nothing written.
    let forged = forged(&file);
    let forged_sha = server.upload(&forged).await;
    let refused = server.send(import(&forged_sha)).await.json();
    assert_eq!(
        (
            refused["state"].as_str(),
            refused["exit_code"].as_u64(),
            refused["error"]["code"].as_str()
        ),
        (Some("refused"), Some(3), Some("socket_not_reproduced")),
        "{refused}"
    );
    assert!(refused["outcome"].is_null() && refused["result"].is_null());
    let revisions = server
        .get(&format!("/v1/sockets/{line}/revisions"))
        .await
        .json();
    assert_eq!(
        revisions["items"].as_array().unwrap().len(),
        2,
        "no revision written"
    );
}

/// VT50, CO5: a component chosen in an assembly the gate refuses ends the
/// job `refused`, exit 3, `component_refused`, with its refusals and no
/// document; a selection naming no part, `refused`, exit 2, its own code.
#[tokio::test]
async fn a_component_of_a_refused_assembly_is_refused() {
    let server = TestServer::fresh();
    let socket = server.socket_of("s01_bolted", "BRACKET").await;
    let junk = b"this assembly is no STEP file\n";
    let registered = server.register(junk, "junk.stp").await;
    assert_eq!(
        registered["state"], "refused",
        "a refused assembly is registered too (RA3)"
    );
    let body = json!({ "socket": socket, "candidates": { "components": [
        { "assembly": format!("sha256:{}", sha_of(junk)), "part": "BRACKET" } ] } });
    let job = server.run_match(&body).await;
    assert_eq!(
        (
            job["state"].as_str(),
            job["exit_code"].as_u64(),
            job["error"]["code"].as_str()
        ),
        (Some("refused"), Some(3), Some("component_refused")),
        "{job}"
    );
    assert!(job["document"].is_null() && job["outcome"].is_null());
    assert_ne!(job["error"]["details"]["refusals"], json!([]));
    let s01 = sha_of(&support::assembly("s01_bolted.stp"));
    let body = json!({ "socket": socket, "candidates": { "components": [
        { "assembly": format!("sha256:{s01}"), "part": "NOPE" } ] } });
    let job = server.run_match(&body).await;
    assert_eq!(
        (
            job["state"].as_str(),
            job["exit_code"].as_u64(),
            job["error"]["code"].as_str()
        ),
        (Some("refused"), Some(2), Some("part_not_found"))
    );
}

/// VT50, VT44's checks, CO5, SN9, M6-59: an `original: true` whose stored
/// assembly cannot be read fails the job `run_incomplete`, exit 1, its
/// document kept and served with `Fitment-Exit-Code: 1`; `fitment.run`
/// says `complete: false`, `summary` and `outcome` null and the document's
/// counts under `problems.incomplete_summary`; every candidates page says
/// `complete: false` and `exit_code: 1`; the notes say why.
#[tokio::test]
async fn an_unreadable_original_is_run_incomplete() {
    let server = TestServer::fresh();
    let socket = server.socket_of("s01_bolted", "BRACKET").await;
    let files = bracket_files(&server).await;
    damage(&server, &sha_of(&support::assembly("s01_bolted.stp")), 100);
    let job = server
        .run_match(&json!({ "socket": socket, "candidates": { "files": files }, "original": true }))
        .await;
    assert_eq!(
        (
            job["state"].as_str(),
            job["exit_code"].as_u64(),
            job["error"]["code"].as_str()
        ),
        (Some("failed"), Some(1), Some("run_incomplete")),
        "{job}"
    );
    assert!(job["outcome"].is_null());
    assert!(job["error"]["details"]["original"].is_string());
    let codes: Vec<&str> = job["notes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["code"].as_str().unwrap())
        .collect();
    assert_eq!(codes.first(), Some(&"original_unreadable"));
    let id = job["id"].as_str().unwrap();
    let document = server.get(&format!("/v1/matches/{id}/document")).await;
    assert_eq!(document.header("fitment-exit-code"), Some("1"));
    let doc: Value = serde_json::from_slice(&document.body).unwrap();
    let run = server.get(&format!("/v1/matches/{id}")).await.json();
    assert_eq!(run["complete"], false);
    assert!(run["summary"].is_null() && run["outcome"].is_null());
    assert_eq!(run["problems"]["incomplete_summary"], doc["summary"]);
    assert_eq!(run["exit_code"], 1);
    let page = server
        .get(&format!("/v1/matches/{id}/candidates"))
        .await
        .json();
    assert_eq!(
        (page["complete"].as_bool(), page["exit_code"].as_u64()),
        (Some(false), Some(1))
    );
}

/// VT40, RA7: two reviews racing on `@r1`: one makes `@r2`, the other is
/// `409 revision_not_current`; a bare id is `428 revision_required`; a
/// review of `@r1` once `@r2` is current is `409`; a label `catalog:x` is
/// `400 label_reserved` (RA8).
#[tokio::test]
async fn reviews_race_and_name_their_revision() {
    let server = TestServer::fresh();
    let socket = server.socket_of("s02_cbore", "BLOCK").await;
    let line = socket.split('@').next().unwrap().to_owned();
    let review = |status: &str| {
        Call::json(
            Method::POST,
            &format!("/v1/sockets/{socket}/review"),
            &json!({ "decisions": [{ "requirement": "bore:hole-427", "status": status }] }),
        )
    };
    // Both pass the request's check of the current revision and wait at
    // their parse, so the race is decided where the revision is written.
    let gate = fitment_server::hooks::Gate::new();
    let hooks = &server.server().seams().hooks;
    hooks.set(
        "socket.parse",
        fitment_server::hooks::Action::Pause(gate.clone()),
    );
    let opened = async {
        assert!(gate.arrived(2, std::time::Duration::from_secs(30)).await);
        gate.open();
    };
    let (a, b, ()) = tokio::join!(server.send(review("kept")), server.send(review("removed").body(
        json!({ "decisions": [{ "requirement": "bore:hole-427", "status": "removed", "note": "by hand" }] })
            .to_string()
            .as_bytes()
    )), opened);
    hooks.clear("socket.parse");
    let mut statuses = [a.status.as_u16(), b.status.as_u16()];
    statuses.sort_unstable();
    assert_eq!(statuses, [201, 409]);
    let loser = if a.status == 409 { &a } else { &b };
    assert_eq!(loser.code(), "revision_not_current");
    // The race's loser names the revision that is current now, as a
    // review of an old revision does (RA7).
    assert_eq!(loser.json()["details"]["current"], format!("{line}@r2"));
    let winner = if a.status == 201 { &a } else { &b };
    assert_eq!(
        winner.header("location"),
        Some(
            format!("{line}@r2")
                .replace(&line, &format!("/v1/sockets/{line}"))
                .as_str()
        )
    );
    let bare = Call::json(
        Method::POST,
        &format!("/v1/sockets/{line}/review"),
        &json!({ "decisions": [{ "requirement": "bore:hole-427", "status": "kept" }] }),
    );
    let bare = server.send(bare).await;
    assert_eq!(
        (bare.status.as_u16(), bare.code()),
        (428, "revision_required".to_owned())
    );
    let old = server.send(review("unreviewed")).await;
    assert_eq!(
        (old.status.as_u16(), old.code()),
        (409, "revision_not_current".to_owned())
    );
    assert_eq!(old.json()["details"]["current"], format!("{line}@r2"));
    let sha = server.upload(&support::candidate("m_block02.stp")).await;
    let body = json!({ "socket": line, "candidates": { "files": [
        { "upload": format!("sha256:{sha}"), "label": "catalog:BLOCK@r1" } ] } });
    let reserved = server.send(waiting("/v1/matches", &body)).await;
    assert_eq!(
        (reserved.status.as_u16(), reserved.code()),
        (400, "label_reserved".to_owned())
    );
}

/// VT49, M6-64, OB4, UP3, SN6: after a file match ends and its uploads
/// expire, its retained record still holds every candidate's object, and
/// the same request submitted again, naming the same SHA-256s, gives the
/// same document's bytes; once the records expire, the sweep unlinks them.
#[tokio::test]
async fn a_retained_match_runs_again_on_its_own_bytes() {
    let server = TestServer::fresh();
    let socket = server.socket_of("s02_cbore", "BLOCK").await;
    let candidate = support::candidate("m_block02_cb_deep.stp");
    let sha = server.upload(&candidate).await;
    let body = json!({ "socket": socket, "candidates": { "files": [
        { "upload": format!("sha256:{sha}"), "label": "deep.stp" } ] } });
    let first = server.run_match(&body).await;
    let bytes = server.document(&first).await.body;
    let seams = server.server().seams();
    let start = seams.ids.now_ms();
    seams.ids.fix_time(start + 25 * 3_600_000);
    server.server().sweep_store().unwrap();
    let head = server
        .send(Call::new(
            Method::HEAD,
            &format!("/v1/uploads/sha256:{sha}"),
        ))
        .await;
    assert_eq!(
        head.status, 200,
        "an input of a retained record stays nameable (UP3)"
    );
    assert!(
        server.object_files().contains(&sha),
        "the record holds the bytes"
    );
    let again = server.run_match(&body).await;
    assert_eq!(again["state"], "succeeded");
    assert_eq!(
        server.document(&again).await.body,
        bytes,
        "the same bytes again"
    );
    seams.ids.fix_time(start + 33 * 86_400_000);
    server.server().sweep_jobs().unwrap();
    server.server().sweep_store().unwrap();
    assert!(
        !server.object_files().contains(&sha),
        "the records expired: the sweep unlinked them"
    );
}

/// M6-64, UP3, OB4 (review of M6 PR 6): a retained match over a
/// component and its original holds the assembly's bytes, so after its
/// socket line and its assembly are deleted the same request, submitted
/// again, names them as registered and gives the same document's bytes;
/// another user, who holds no such record, cannot name them.
#[tokio::test]
async fn a_retained_component_match_runs_again_without_its_assembly() {
    let server = TestServer::fresh();
    let socket = server.socket_of("s02_cbore", "BLOCK").await;
    let line = socket.split('@').next().unwrap().to_owned();
    let sha = fitment_core::sha256_hex(&support::assembly("s02_cbore.stp"));
    let body = json!({ "socket": socket, "original": true, "candidates": { "components": [
        { "assembly": format!("sha256:{sha}"), "part": "BLOCK" } ] } });
    let first = server.run_match(&body).await;
    assert_eq!(first["state"], "succeeded", "{first}");
    let bytes = server.document(&first).await.body;
    for path in [
        format!("/v1/sockets/{line}"),
        format!("/v1/assemblies/sha256:{sha}"),
    ] {
        let deleted = server.send(Call::new(Method::DELETE, &path)).await;
        assert_eq!(deleted.status, 204, "{path}");
    }
    let again = server.run_match(&body).await;
    assert_eq!(again["state"], "succeeded", "{again}");
    assert_eq!(
        server.document(&again).await.body,
        bytes,
        "the same bytes again"
    );
    let bob = support::seed_user(
        server.server(),
        "bob",
        fitment_server::auth::Role::Reader,
        None,
    );
    let (_, token) = support::seed_token(
        server.server(),
        &bob,
        fitment_server::auth::Role::Reader,
        30,
    );
    let theirs = server
        .send(waiting("/v1/matches", &body).bearer(&token))
        .await;
    assert_eq!(
        theirs.status,
        404,
        "{}",
        String::from_utf8_lossy(&theirs.body)
    );
}

/// VT31, SN11, XH7: with the test-support fake verifier answering
/// `verified`, a match on s01's good bracket ends `succeeded`, `match`,
/// exit 0, and its document names the fake; with `NoFitVerifier` the best
/// is an interface match. Neither the OpenAPI document nor `--help` names
/// a way to choose a verifier.
#[tokio::test]
async fn the_fit_verifier_alone_makes_a_match() {
    let server = TestServer::fresh();
    let socket = server.socket_of("s01_bolted", "BRACKET").await;
    let sha = server.upload(&support::candidate("m_bracket.stp")).await;
    let body = json!({ "socket": socket, "candidates": { "files": [
        { "upload": format!("sha256:{sha}"), "label": "m_bracket.stp" } ] } });
    let plain = server.run_match(&body).await;
    assert_eq!(
        (plain["outcome"].as_str(), plain["exit_code"].as_u64()),
        (Some("interface_match"), Some(4))
    );
    server.server().set_fit_verifier(std::sync::Arc::new(
        fitment_server::matching::VerifiesEverything,
    ));
    let verified = server.run_match(&body).await;
    assert_eq!(
        (verified["outcome"].as_str(), verified["exit_code"].as_u64()),
        (Some("match"), Some(0))
    );
    let doc: Value = serde_json::from_slice(&server.document(&verified).await.body).unwrap();
    assert_eq!(doc["fit_verifier"], "test-support 1");
    assert_eq!(doc["summary"]["match"], 1);
    let openapi = String::from_utf8_lossy(fitment_server::OPENAPI);
    assert!(!openapi.contains("test-support") && !openapi.contains("VerifiesEverything"));
    let (_, help, _) = support::run(&["serve", "--help"], &[]);
    assert!(!help.to_ascii_lowercase().contains("verifier"), "{help}");
}

/// VT32's socket half (MF11, JB2, SN3): a stored revision of lower socket
/// rules than the build's (the test-support seam raises the build's) is
/// refused for matching and for review, `422 socket_stale` with both
/// versions; extracting again carries its decisions as MF11 does.
#[tokio::test]
async fn a_stale_socket_is_refused_for_matching_and_review() {
    let server = TestServer::fresh();
    let socket = server.socket_of("s02_cbore", "BLOCK").await;
    let reviewed = server
        .send(Call::json(
            Method::POST,
            &format!("/v1/sockets/{socket}/review"),
            &json!({ "decisions": [{ "requirement": "bore:hole-427", "status": "kept" }] }),
        ))
        .await;
    assert_eq!(reviewed.status, 201);
    let current = reviewed
        .header("location")
        .unwrap()
        .trim_start_matches("/v1/sockets/")
        .to_owned();
    server.server().set_rules_shift(1);
    let sha = server.upload(&support::candidate("m_block02.stp")).await;
    let body = json!({ "socket": current, "candidates": { "files": [
        { "upload": format!("sha256:{sha}"), "label": "m_block02.stp" } ] } });
    let refused = server.send(waiting("/v1/matches", &body)).await;
    assert_eq!(
        (refused.status.as_u16(), refused.code()),
        (422, "socket_stale".to_owned())
    );
    let details = &refused.json()["details"];
    assert_eq!(
        details["build_socket_rules_version"],
        fitment_core::SOCKET_RULES_VERSION + 1
    );
    let review = server
        .send(Call::json(
            Method::POST,
            &format!("/v1/sockets/{current}/review"),
            &json!({ "decisions": [{ "requirement": "bore:hole-427", "status": "unreviewed" }] }),
        ))
        .await;
    assert_eq!(
        (review.status.as_u16(), review.code()),
        (422, "socket_stale".to_owned())
    );
    server.server().set_rules_shift(0);
    let again = server
        .extract(&sha_of(&support::assembly(S02)), "BLOCK")
        .await;
    assert_eq!(
        again["outcome"], "unchanged",
        "the decisions carried: the same bytes"
    );
}

/// DC10, M6-25, VN3, AU8: a run's `fitment.run` states its socket (the
/// revision, its digest, how it was made, verified), its candidates and
/// the distinct ones, the document's policy, options, versions, summary,
/// outcome and exit code, `complete`, no problems, and the document's
/// link, SHA-256, length and digest; `GET /v1/matches?socket=` lists one
/// line's runs (or one revision's); a job of another kind is no match.
#[tokio::test]
async fn a_run_is_recorded_beside_its_document() {
    let server = TestServer::fresh();
    let socket = server.socket_of("s01_bolted", "BRACKET").await;
    let files = bracket_files(&server).await;
    let job = server
        .run_match(&json!({ "socket": socket, "candidates": { "files": files },
                            "policy": { "clearance": "functional" } }))
        .await;
    let id = job["id"].as_str().unwrap();
    let run = server.get(&format!("/v1/matches/{id}")).await;
    assert_eq!(run.header("fitment-format"), Some("fitment.run/1"));
    let run = run.json();
    let document = server.document(&job).await;
    let doc: Value = serde_json::from_slice(&document.body).unwrap();
    assert_eq!(run["socket"]["ref"], socket);
    assert_eq!(run["socket"]["digest"], doc["socket"]["digest"]);
    assert_eq!(
        (
            run["socket"]["how"].as_str(),
            run["socket"]["verified"].as_bool()
        ),
        (Some("extract"), Some(true))
    );
    assert_eq!(run["candidates"]["files"], files.len());
    assert_eq!(run["candidates"]["distinct"], doc["summary"]["candidates"]);
    assert_eq!(run["policy"], doc["policy"]);
    assert_eq!(run["summary"], doc["summary"]);
    assert_eq!(run["outcome"], job["outcome"]);
    assert_eq!(
        (run["complete"].as_bool(), run["exit_code"].as_u64()),
        (Some(true), job["exit_code"].as_u64())
    );
    assert_eq!(run["document"]["sha256"], job["document"]["sha256"]);
    assert_eq!(run["document"]["digest"], doc["digest"]);
    assert_eq!(run["versions"]["server"], env!("CARGO_PKG_VERSION"));
    // SN8: the socket's unreviewed requirements, every run.
    assert_eq!(run["socket"]["unreviewed"], 7);
    assert_eq!(run["problems"]["bugs"], json!([]));
    let line = socket.split('@').next().unwrap();
    let listed = server
        .get(&format!("/v1/matches?socket={line}"))
        .await
        .json();
    assert_eq!(listed["items"].as_array().unwrap().len(), 1);
    assert_eq!(listed["items"][0]["id"], id);
    let other = server
        .get(&format!("/v1/matches?socket={line}@r2"))
        .await
        .json();
    assert_eq!(other["items"], json!([]));
    let extraction = server.get("/v1/jobs?kind=socket_extract").await.json();
    let extraction = extraction["items"][0]["id"].as_str().unwrap();
    let not = server.get(&format!("/v1/matches/{extraction}")).await;
    assert_eq!(
        (not.status.as_u16(), not.code()),
        (404, "not_found".to_owned())
    );
}

/// RA6, OB4, SN6: a deleted line's revision a retained run cites stays
/// readable and may be named by a match again, so the run can be
/// submitted again; the revisions' sweep leaves it until the last record
/// citing it expires, then deletes it.
#[tokio::test]
async fn a_cited_revision_outlives_its_line() {
    let server = TestServer::fresh();
    let socket = server.socket_of("s02_cbore", "BLOCK").await;
    let line = socket.split('@').next().unwrap().to_owned();
    let sha = server.upload(&support::candidate("m_block02.stp")).await;
    let body = json!({ "socket": socket, "candidates": { "files": [
        { "upload": format!("sha256:{sha}"), "label": "m_block02.stp" } ] } });
    let first = server.run_match(&body).await;
    let deleted = server
        .send(Call::new(Method::DELETE, &format!("/v1/sockets/{line}")))
        .await;
    assert_eq!(deleted.status, 204);
    server.server().sweep_jobs().unwrap();
    assert_eq!(
        server.get(&format!("/v1/sockets/{socket}")).await.status,
        200,
        "cited: kept"
    );
    let again = server.run_match(&body).await;
    assert_eq!(
        server.document(&again).await.body,
        server.document(&first).await.body
    );
    let seams = server.server().seams();
    seams.ids.fix_time(seams.ids.now_ms() + 31 * 86_400_000);
    server.server().sweep_jobs().unwrap();
    let gone = server.get(&format!("/v1/sockets/{socket}")).await;
    assert_eq!(
        (gone.status.as_u16(), gone.code()),
        (404, "not_found".to_owned()),
        "the last record went"
    );
}

/// RA9, JB2: a match's inputs are frozen when its job is made: a bare
/// `sk_X` is its current revision then, recorded exact; a review that
/// makes `@r2` while the job waits changes nothing it judges.
#[tokio::test]
async fn a_match_judges_the_revision_it_was_given() {
    let server = TestServer::fresh();
    let socket = server.socket_of("s02_cbore", "BLOCK").await;
    let line = socket.split('@').next().unwrap().to_owned();
    let sha = server.upload(&support::candidate("m_block02.stp")).await;
    let body = json!({ "socket": line, "candidates": { "files": [
        { "upload": format!("sha256:{}", sha.to_ascii_uppercase()), "label": "m_block02.stp" } ] } });
    server.server().hold_jobs();
    let queued = server
        .send(Call::json(Method::POST, "/v1/matches", &body))
        .await
        .json();
    assert_eq!(
        queued["request"]["socket"],
        format!("{line}@r1"),
        "resolved at creation"
    );
    assert_eq!(
        queued["request"]["files"][0]["upload"],
        format!("sha256:{sha}")
    );
    let review = json!({ "decisions": [{ "requirement": "bore:hole-427", "status": "removed", "note": "by hand" }] });
    let reviewed = server
        .send(Call::json(
            Method::POST,
            &format!("/v1/sockets/{line}@r1/review"),
            &review,
        ))
        .await;
    assert_eq!(reviewed.status, 201);
    server.server().resume_jobs();
    let job = server
        .finished_job(queued["id"].as_str().unwrap(), Duration::from_secs(60))
        .await;
    let doc: Value = serde_json::from_slice(&server.document(&job).await.body).unwrap();
    assert!(
        doc["socket"]["sockets"][0]["removed"]
            .as_array()
            .unwrap()
            .is_empty(),
        "judged against @r1, as it was when the job was made"
    );
}

/// DC10, SN3, SV5 (review of M6 PR 6): `fitment.run` states the options
/// and versions its run recorded, not its request's nor the answering
/// build's: a match asked with no option and queued while the floor was
/// off, then run by a server whose floor is on, states `require_reviewed:
/// true` by `"server"`, and the versions `job.run` holds; a run that
/// recorded nothing (cancelled while queued) states its request's options
/// and `versions: null`.
#[tokio::test]
async fn a_run_states_the_options_and_versions_it_ran_with() {
    let server = TestServer::fresh();
    let socket = server.socket_of("s02_cbore", "BLOCK").await;
    let doc: Value = server.get(&format!("/v1/sockets/{socket}")).await.json();
    let decisions: Vec<Value> = doc["extracted"]["sockets"][0]["requirements"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| json!({ "requirement": r["id"], "status": "kept" }))
        .collect();
    let reviewed = server
        .send(Call::json(
            Method::POST,
            &format!("/v1/sockets/{socket}/review"),
            &json!({ "decisions": decisions }),
        ))
        .await;
    let current = reviewed
        .header("location")
        .unwrap()
        .trim_start_matches("/v1/sockets/")
        .to_owned();
    let sha = server.upload(&support::candidate("m_block02.stp")).await;
    let body = json!({ "socket": current, "candidates": { "files": [
        { "upload": format!("sha256:{sha}"), "label": "m_block02.stp" } ] } });
    server.server().hold_jobs();
    let mut ids = Vec::new();
    for _ in 0..2 {
        let queued = server
            .send(Call::json(Method::POST, "/v1/matches", &body))
            .await
            .json();
        ids.push(queued["id"].as_str().unwrap().to_owned());
    }
    let cancelled = server
        .send(Call::new(
            Method::POST,
            &format!("/v1/jobs/{}/cancel", ids[1]),
        ))
        .await;
    assert_eq!(cancelled.json()["state"], "cancelled");
    let unrun = server.get(&format!("/v1/matches/{}", ids[1])).await.json();
    assert_eq!(unrun["options"]["require_reviewed"], false, "{unrun}");
    assert!(unrun["options"]["require_reviewed_by"].is_null(), "{unrun}");
    assert!(unrun["versions"].is_null(), "{unrun}");
    let mut config = server.dirs.config();
    config.matching.require_reviewed_floor = true;
    let server = server.restart(config);
    let job = server.finished_job(&ids[0], Duration::from_secs(60)).await;
    assert_eq!(job["state"], "succeeded", "{job}");
    let run = server.get(&format!("/v1/matches/{}", ids[0])).await.json();
    assert_eq!(run["options"]["require_reviewed"], true, "{run}");
    assert_eq!(run["options"]["require_reviewed_by"], "server", "{run}");
    let conn = rusqlite::Connection::open(server.dirs.state.join("server.db")).unwrap();
    let recorded: String = conn
        .query_row("SELECT run FROM job WHERE id = ?1", [&ids[0]], |r| r.get(0))
        .unwrap();
    let recorded: Value = serde_json::from_str(&recorded).unwrap();
    assert_eq!(run["versions"], recorded["versions"]);
    assert_eq!(
        run["versions"]["socket_rules_version"],
        fitment_core::SOCKET_RULES_VERSION
    );
}

/// SN3, M6-12: the operator's `require_reviewed` floor forces the option:
/// an unreviewed socket is refused (`422 socket_unreviewed`) though the
/// request asks nothing; once every requirement is reviewed the run's
/// document states `require_reviewed: true`, and its `fitment.run` that the
/// server forced it (`require_reviewed_by: "server"`); a request asking
/// for it says `"request"`.
#[tokio::test]
async fn the_floor_tightens_and_is_recorded() {
    let dirs = Dirs::new();
    let mut config = dirs.config();
    config.matching.require_reviewed_floor = true;
    let server = TestServer::in_process(dirs, config);
    let socket = server.socket_of("s02_cbore", "BLOCK").await;
    let sha = server.upload(&support::candidate("m_block02.stp")).await;
    let files = json!([{ "upload": format!("sha256:{sha}"), "label": "m_block02.stp" }]);
    let refused = server
        .send(waiting(
            "/v1/matches",
            &json!({ "socket": socket, "candidates": { "files": files } }),
        ))
        .await;
    assert_eq!(refused.code(), "socket_unreviewed");
    let doc: Value = server.get(&format!("/v1/sockets/{socket}")).await.json();
    let decisions: Vec<Value> = doc["extracted"]["sockets"][0]["requirements"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| json!({ "requirement": r["id"], "status": "kept" }))
        .collect();
    let reviewed = server
        .send(Call::json(
            Method::POST,
            &format!("/v1/sockets/{socket}/review"),
            &json!({ "decisions": decisions }),
        ))
        .await;
    let current = reviewed
        .header("location")
        .unwrap()
        .trim_start_matches("/v1/sockets/")
        .to_owned();
    for (asked, by) in [
        (json!({}), "server"),
        (json!({ "require_reviewed": true }), "request"),
    ] {
        let job = server
            .run_match(
                &json!({ "socket": current, "options": asked, "candidates": { "files": files } }),
            )
            .await;
        let doc: Value = serde_json::from_slice(&server.document(&job).await.body).unwrap();
        assert_eq!(doc["options"]["require_reviewed"], true);
        let run = server
            .get(&format!("/v1/matches/{}", job["id"].as_str().unwrap()))
            .await
            .json();
        assert_eq!(run["options"]["require_reviewed_by"], by);
    }
}

/// The catalog's ETag of a reply, as its views carry it (VN5).
fn view_etag(reply: &support::Reply) -> String {
    reply
        .header("etag")
        .expect("a catalog view's ETag")
        .to_owned()
}

/// RA1: one server, one catalog, shared by every user: what a writer
/// ingests a reader lists and gets; nothing in a request names a
/// directory (a field naming one is refused as unknown).
#[tokio::test]
async fn the_catalog_is_one_and_shared() {
    let server = TestServer::fresh();
    let job = server
        .ingest(&[(support::candidate("m_block02.stp"), "m_block02.stp".into())])
        .await;
    assert_eq!(job["state"], "succeeded", "{job}");
    let reader = support::seed_user(
        server.server(),
        "ada",
        fitment_server::auth::Role::Reader,
        None,
    );
    let (_, token) = support::seed_token(
        server.server(),
        &reader,
        fitment_server::auth::Role::Reader,
        30,
    );
    let listed = server
        .send(Call::get("/v1/catalog/entries?limit=all").bearer(&token))
        .await;
    assert_eq!(listed.json()["counts"]["entries"], 1);
    let got = server
        .send(Call::get("/v1/catalog/entries/m_block02").bearer(&token))
        .await;
    assert_eq!(got.json()["entries"][0]["id"], "m_block02");
    let body = serde_json::json!({ "files": [], "catalog": "/elsewhere" });
    let reply = server
        .send(Call::json(Method::POST, "/v1/catalog/entries", &body))
        .await;
    assert_eq!(reply.code(), "bad_request");
}

/// VN5, M6-63, CA8, VN4: a catalog view's ETag is the instance's, the
/// generation's and the build's: two catalogs at one generation never
/// share one; `If-None-Match` answers `304`; a change gives another, and
/// a page asked under the first page's ETag is then `412
/// precondition_failed`. Derived documents are `no-cache`; stored bytes
/// named exactly `immutable`.
#[tokio::test]
async fn catalog_views_are_tagged_by_instance_and_generation() {
    let one = TestServer::fresh();
    let two = TestServer::fresh();
    let file = (
        support::candidate("m_block02.stp"),
        "m_block02.stp".to_owned(),
    );
    for server in [&one, &two] {
        server.ingest(std::slice::from_ref(&file)).await;
    }
    let (a, b) = (
        one.get("/v1/catalog/entries?limit=all").await,
        two.get("/v1/catalog/entries?limit=all").await,
    );
    assert_eq!(a.json()["generation"], b.json()["generation"]);
    assert_eq!(a.body, b.body, "the same catalog state, the same bytes");
    assert_ne!(view_etag(&a), view_etag(&b), "two instances, two tags");
    let etag = view_etag(&a);
    let instance = one.server().seams().catalog.instance().to_owned();
    assert!(etag.starts_with(&format!("\"{instance}.g")), "{etag}");
    let again = one
        .send(Call::get("/v1/catalog/entries?limit=1").header("if-none-match", &etag))
        .await;
    assert_eq!(again.status, 304);
    let info = one.get("/v1/catalog").await;
    let tagged = view_etag(&info);
    assert!(
        tagged.starts_with(etag.trim_end_matches('"')) && tagged != etag,
        "the catalog's tag is its views', its state's flags folded in: {tagged}"
    );
    one.ingest(&[(
        support::candidate("m_block02_cb_deep.stp"),
        "deep.stp".into(),
    )])
    .await;
    let moved = one.get("/v1/catalog/entries?limit=1").await;
    assert_ne!(view_etag(&moved), etag);
    let stale = one
        .send(Call::get("/v1/catalog/entries?limit=1").header("if-match", &etag))
        .await;
    assert_eq!(stale.code(), "precondition_failed");
    let fresh = one
        .send(Call::get("/v1/catalog/entries?limit=1").header("if-match", &view_etag(&moved)))
        .await;
    assert_eq!(fresh.status, 200);
    let features = one.get("/v1/catalog/entries/m_block02/features").await;
    assert_eq!(features.header("cache-control"), Some("no-cache"));
    let bare = one.get("/v1/catalog/entries/m_block02/step").await;
    assert_eq!(bare.header("cache-control"), Some("no-cache"));
    let exact = one.get("/v1/catalog/entries/m_block02@r1/step").await;
    assert_eq!(
        exact.header("cache-control"),
        Some("private, max-age=31536000, immutable")
    );
    assert_eq!(exact.body, support::candidate("m_block02.stp"));
}

/// VN3, VN4: keyset pages are total and stable: 250 entries in pages of
/// 100 are `limit=all`'s entries exactly, each once, every page at one
/// generation and its next in `Link`; a filtered page holds only its
/// status, read on until full.
#[tokio::test]
async fn catalog_pages_are_total_and_stable() {
    let server = TestServer::fresh();
    let mut files = support::junk_files(249);
    files.push((support::candidate("m_block02.stp"), "m_block02.stp".into()));
    let job = server.ingest(&files).await;
    assert_eq!(job["exit_code"], 3, "junk is quarantined");
    let all = server.get("/v1/catalog/entries?limit=all").await.json();
    let mut paged = Vec::new();
    let mut next = Some("/v1/catalog/entries?limit=100".to_owned());
    let mut count = 0;
    while let Some(url) = next {
        let reply = server.get(&url).await;
        assert_eq!(
            reply.json()["counts"],
            all["counts"],
            "the whole catalog's counts"
        );
        assert_eq!(reply.json()["generation"], all["generation"]);
        paged.extend(reply.json()["entries"].as_array().unwrap().clone());
        next = reply.header("link").map(|l| {
            l.trim_start_matches('<')
                .split('>')
                .next()
                .unwrap()
                .to_owned()
        });
        count += 1;
    }
    assert_eq!(count, 3);
    assert_eq!(&paged, all["entries"].as_array().unwrap());
    let indexed = server
        .get("/v1/catalog/entries?limit=1&status=indexed")
        .await;
    let entries = indexed.json()["entries"].as_array().unwrap().clone();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["id"], "m_block02");
    assert!(indexed.header("link").is_none(), "the only indexed entry");
}

/// VT32's catalog half (CX5, RX3, M6-28, M6-56): a catalog an older build
/// read refuses a full run, `refused`, `catalog_stale`, with the counts,
/// until a reindex job ends; the run then is a fresh catalog's. With
/// `jobs.reindex_on_start`, a start on a stale catalog queues the reindex
/// as the `system` user and `fitment.info` says so.
#[tokio::test]
async fn a_stale_catalog_refuses_until_reindexed() {
    let dirs = Dirs::new();
    let mut config = dirs.config();
    config.jobs.reindex_on_start = false;
    let server = TestServer::in_process(dirs, config.clone());
    let files: Vec<(Vec<u8>, String)> = ["m_block02.stp", "m_block02_cb_deep.stp"]
        .iter()
        .map(|n| (support::candidate(n), (*n).to_owned()))
        .collect();
    server.ingest(&files).await;
    let socket = server.socket_of("s02_cbore", "BLOCK").await;
    let full = serde_json::json!({ "socket": socket, "candidates": { "catalog": {} } });
    let fresh = server.run_match(&full).await;
    assert_eq!(fresh["state"], "succeeded", "{fresh}");
    let fresh_doc = server.document(&fresh).await.body;
    support::stale_catalog(&server.dirs.catalog, fitment_core::EXTRACTOR_VERSION - 1);
    let refused = server.run_match(&full).await;
    assert_eq!(
        (refused["state"].as_str(), refused["exit_code"].as_i64()),
        (Some("refused"), Some(3))
    );
    assert_eq!(refused["error"]["code"], "catalog_stale");
    assert_eq!(refused["error"]["details"]["count"], 2);
    assert!(refused["document"].is_null());
    let info = server.get("/v1/info").await.json();
    assert_eq!(info["catalog"]["state"]["stale"], true);
    // A restart with the reindex on start, the dispatcher held: queued as
    // the system user, and said so.
    config.jobs.reindex_on_start = true;
    let server = server.restart(config);
    server.server().hold_jobs();
    let info = server.get("/v1/info").await.json();
    assert_eq!(info["catalog"]["state"]["reindex"], "queued", "{info}");
    let system = server.server().seams().db.system_user().unwrap();
    let queued = server
        .get(&format!("/v1/jobs?kind=catalog_reindex&owner={system}"))
        .await
        .json();
    let job = queued["items"][0].clone();
    assert_eq!(job["owner"], system.as_str());
    server.server().resume_jobs();
    let reindexed = server
        .finished_job(
            job["id"].as_str().unwrap(),
            std::time::Duration::from_secs(120),
        )
        .await;
    assert_eq!(reindexed["state"], "succeeded", "{reindexed}");
    let again = server.run_match(&full).await;
    assert_eq!(again["state"], "succeeded", "{again}");
    assert_eq!(
        server.document(&again).await.body,
        fresh_doc,
        "a fresh catalog's run"
    );
    assert_eq!(support::verified(&server.dirs.catalog), Some(0));
}

/// M6-25, DC10, SN10: a catalog run's record states its snapshot's
/// generation and CM8's counts, never in the document (M5-2): a full
/// run's searched, quarantined and level 1's survivors; a named run's
/// count; each candidate labelled `catalog:<id>@r<n>`, and the API's
/// download of that reference the bytes its `source` states (ST11).
#[tokio::test]
async fn a_catalog_run_records_its_generation_and_counts() {
    let server = TestServer::fresh();
    let mut files: Vec<(Vec<u8>, String)> = ["m_block02.stp", "m_block02_cb_deep.stp"]
        .iter()
        .map(|n| (support::candidate(n), (*n).to_owned()))
        .collect();
    files.push((b"not STEP".to_vec(), "junk.stp".into()));
    server.ingest(&files).await;
    let generation = server.get("/v1/catalog").await.json()["generation"].clone();
    let socket = server.socket_of("s02_cbore", "BLOCK").await;
    let job = server
        .run_match(&serde_json::json!({ "socket": socket, "candidates": { "catalog": {} } }))
        .await;
    let run = server
        .get(&format!("/v1/matches/{}", job["id"].as_str().unwrap()))
        .await
        .json();
    let c = &run["candidates"];
    assert_eq!(c["kind"], "catalog");
    assert_eq!(c["selection"], "full");
    assert_eq!(c["generation"], generation);
    assert_eq!(
        (c["entries"].as_i64(), c["searched"].as_i64()),
        (Some(3), Some(2))
    );
    assert_eq!(c["quarantined_not_searched"], 1);
    assert_eq!(c["named"], serde_json::Value::Null);
    let doc: serde_json::Value = serde_json::from_slice(&server.document(&job).await.body).unwrap();
    for candidate in doc["candidates"].as_array().unwrap() {
        let label = candidate["labels"][0].as_str().unwrap();
        let reference = label.strip_prefix("catalog:").expect("a catalog label");
        let bytes = server
            .get(&format!("/v1/catalog/entries/{reference}/step"))
            .await
            .body;
        assert_eq!(
            fitment_core::sha256_hex(&bytes),
            candidate["source"]["sha256"].as_str().unwrap(),
            "{label}"
        );
    }
    let named = server
        .run_match(&serde_json::json!({ "socket": socket, "candidates": { "catalog": { "ids": ["m_block02"] } } }))
        .await;
    let run = server
        .get(&format!("/v1/matches/{}", named["id"].as_str().unwrap()))
        .await
        .json();
    assert_eq!(run["candidates"]["selection"], "named");
    assert_eq!(run["candidates"]["named"], 1);
    let unknown = server
        .run_match(&serde_json::json!({ "socket": socket, "candidates": { "catalog": { "ids": ["nothing"] } } }))
        .await;
    assert_eq!(
        (unknown["state"].as_str(), unknown["exit_code"].as_i64()),
        (Some("refused"), Some(2))
    );
    assert_eq!(unknown["error"]["code"], "entry_not_found");
}

/// JS9, M6-56, CX8: the daily catalog gc (`jobs.catalog_gc_at`, `HH:MM`
/// UTC, off by default) is a write-lane job of the `system` user, without
/// `versions` or `vacuum`, queued once while one is queued; an admin
/// lists it. A time that is no `HH:MM` refuses the start.
#[tokio::test]
async fn the_daily_gc_is_the_system_users() {
    let dirs = Dirs::new();
    let mut config = dirs.config();
    config.jobs.catalog_gc_at = "25:00".into();
    assert!(config.validate().is_err(), "no such time");
    config.jobs.catalog_gc_at = "03:30".into();
    assert_eq!(config.jobs.catalog_gc_minute(), Some(210));
    let server = TestServer::in_process(dirs, config);
    server.server().hold_jobs();
    let queued = server.server().queue_daily_gc().unwrap().expect("queued");
    assert_eq!(server.server().queue_daily_gc().unwrap(), None, "once");
    let system = server.server().seams().db.system_user().unwrap();
    let job = server.get(&format!("/v1/jobs/{queued}")).await.json();
    assert_eq!(job["owner"], system.as_str());
    assert_eq!(job["kind"], "catalog_gc");
    assert_eq!(
        job["request"],
        serde_json::json!({ "versions": false, "vacuum": false })
    );
    server.server().resume_jobs();
    let done = server
        .finished_job(&queued, std::time::Duration::from_secs(60))
        .await;
    assert_eq!(done["state"], "succeeded", "{done}");
}

/// VN5, DC11 (the review of PR 7b): `GET /v1/catalog`'s ETag folds in its
/// state's flags: a reindex queued at one generation is a new tag, never a
/// stale `304`, and the body says it is queued.
#[tokio::test]
async fn the_catalogs_tag_follows_its_state() {
    let server = TestServer::fresh();
    server
        .ingest(&[(
            support::candidate("m_block02.stp"),
            "m_block02.stp".to_owned(),
        )])
        .await;
    let first = server.get("/v1/catalog").await;
    let etag = view_etag(&first);
    let same = server
        .send(Call::get("/v1/catalog").header("if-none-match", &etag))
        .await;
    assert_eq!(same.status, 304);
    server.server().hold_jobs();
    server
        .submit("/v1/catalog/reindex", &json!({ "all": true }))
        .await;
    let after = server
        .send(Call::get("/v1/catalog").header("if-none-match", &etag))
        .await;
    assert_eq!(after.status, 200, "a new state, a new tag");
    assert_eq!(after.json()["generation"], first.json()["generation"]);
    assert_eq!(after.json()["state"]["reindex"], "queued");
    assert_ne!(view_etag(&after), etag);
    server.server().resume_jobs();
}

/// RA8, CL2 (the review of PR 7b): an ingest's lines are tied to their own
/// inputs by ID4's order, never by their labels, which several may share.
/// Two files of one label and one file name: the second is a conflict
/// under its file's id, the CLI's exit 2 with no code, nothing internal. A
/// conflict under its file's own id beside a file the request gave an id
/// is no `id_conflict`; a conflict under an id the request gave is.
#[tokio::test]
async fn ingest_lines_are_their_own_inputs() {
    let server = TestServer::fresh();
    let mut shas = Vec::new();
    for bytes in [
        support::candidate("m_block02.stp"),
        support::candidate("m_block02_cb_deep.stp"),
        support::candidate("m_bracket.stp"),
        b"not STEP".to_vec(),
    ] {
        shas.push(server.upload(&bytes).await);
    }
    let file = |sha: &str, name: &str, id: Option<&str>| {
        let mut file =
            json!({ "upload": format!("sha256:{sha}"), "file_name": name, "input": "same.stp" });
        if let Some(id) = id {
            file["id"] = json!(id);
        }
        file
    };
    let run = |files: Vec<Value>| {
        let server = &server;
        async move {
            let job = server
                .submit("/v1/catalog/entries", &json!({ "files": files }))
                .await;
            server
                .finished_job(job["id"].as_str().unwrap(), Duration::from_secs(60))
                .await
        }
    };
    let alike = run(vec![
        file(&shas[0], "p.stp", None),
        file(&shas[1], "p.stp", None),
    ])
    .await;
    assert_eq!(
        (alike["state"].as_str(), alike["exit_code"].as_i64()),
        (Some("refused"), Some(2)),
        "{alike}"
    );
    assert!(alike["error"].is_null(), "{alike}");
    let own = run(vec![
        file(&shas[2], "p.stp", None),
        file(&shas[1], "q.stp", Some("q")),
    ])
    .await;
    assert_eq!(own["exit_code"], 2, "{own}");
    assert!(own["error"].is_null(), "no id the request gave: {own}");
    let given = run(vec![
        file(&shas[2], "z.stp", Some("p")),
        file(&shas[3], "r.stp", None),
    ])
    .await;
    assert_eq!(
        (given["exit_code"].as_i64(), given["error"]["code"].as_str()),
        (Some(2), Some("id_conflict")),
        "{given}"
    );
}

/// RA7, as the review of PR 7b amends it: a delete or a replace under
/// `If-Match` is made on condition of the catalog's generation, which a
/// view's tag names: a tag of an older generation, or of no view of this
/// catalog, is `412 precondition_failed`, nothing changed; the current
/// tag, or `*`, makes the change; without the header it is unconditional.
#[tokio::test]
async fn catalog_writes_honour_if_match() {
    let server = TestServer::fresh();
    let files: Vec<(Vec<u8>, String)> = ["m_block02.stp", "m_block02_cb_deep.stp"]
        .iter()
        .map(|n| (support::candidate(n), (*n).to_owned()))
        .collect();
    server.ingest(&files).await;
    let old = view_etag(&server.get("/v1/catalog/entries/m_block02").await);
    server
        .ingest(&[(
            support::candidate("m_bracket.stp"),
            "m_bracket.stp".to_owned(),
        )])
        .await;
    let delete = |tag: &str, entry: &str| {
        Call::new(Method::DELETE, &format!("/v1/catalog/entries/{entry}")).header("if-match", tag)
    };
    for tag in [old.as_str(), "\"0123.g1.abcdef\"", "W/\"x\""] {
        let refused = server.send(delete(tag, "m_block02")).await;
        assert_eq!(
            (refused.status.as_u16(), refused.code()),
            (412, "precondition_failed".into()),
            "{tag}"
        );
    }
    assert_eq!(server.get("/v1/catalog").await.json()["entries"], 3);
    let current = view_etag(&server.get("/v1/catalog/entries/m_block02").await);
    assert_eq!(server.send(delete(&current, "m_block02")).await.status, 200);
    assert_eq!(
        server.send(delete("*", "m_block02_cb_deep")).await.status,
        200
    );
    let p01 = server.upload(&support::part("p01_through.stp")).await;
    let body = json!({ "upload": format!("sha256:{p01}"), "file_name": "p01_through.stp" });
    let put = |tag: &str| {
        Call::json(Method::PUT, "/v1/catalog/entries/m_bracket", &body).header("if-match", tag)
    };
    let stale = server.send(put(&current)).await;
    assert_eq!(
        (stale.status.as_u16(), stale.code()),
        (412, "precondition_failed".into())
    );
    let fresh = view_etag(&server.get("/v1/catalog/entries/m_bracket").await);
    let job = server.send(put(&fresh)).await.json();
    let job = server
        .finished_job(job["id"].as_str().unwrap(), Duration::from_secs(60))
        .await;
    assert_eq!(job["state"], "succeeded", "{job}");
}
