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
