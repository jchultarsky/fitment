//! The error registry and the error document (docs/research/m6-api.md,
//! section 4.7, 7.11 and 7.12; ER1 to ER7, DC9).

mod support;

use std::path::Path;

use axum::http::Method;
use fitment_server::hooks::Action;
use fitment_server::http::error::{ApiError, CatalogReason, Class, ErrorCode, SPECS};
use support::{Call, TestServer};

/// One row of section 7.12, as the design writes it.
struct Row {
    codes: Vec<String>,
    class: String,
    http: Option<u16>,
    cli: String,
}

fn design_rows() -> Vec<Row> {
    let design = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/research/m6-api.md"),
    )
    .unwrap();
    let start = design.find("### 7.12 The error codes").unwrap();
    let end = design[start..].find("### 7.13").unwrap() + start;
    design[start..end]
        .lines()
        .filter(|l| l.starts_with("| `"))
        .map(|line| {
            let cells: Vec<&str> = line.split('|').map(str::trim).collect();
            Row {
                codes: cells[1]
                    .split(',')
                    .map(|c| c.trim().trim_matches('`').to_owned())
                    .collect(),
                class: cells[2].to_owned(),
                http: cells[3].parse().ok(),
                cli: cells[4].to_owned(),
            }
        })
        .collect()
}

/// ER1: `ErrorCode::ALL` is section 7.12, code for code in its order,
/// each with the table's class, HTTP status and CLI exit code; names are
/// unique and `snake_case`.
#[test]
fn the_registry_is_section_7_12() {
    let rows = design_rows();
    let codes: Vec<&str> = rows
        .iter()
        .flat_map(|r| r.codes.iter().map(String::as_str))
        .collect();
    let ours: Vec<&str> = ErrorCode::ALL.iter().map(|c| c.name()).collect();
    assert_eq!(ours, codes);
    let unique: std::collections::BTreeSet<&str> = ours.iter().copied().collect();
    assert_eq!(unique.len(), ours.len());
    for row in &rows {
        for name in &row.codes {
            let spec = SPECS.iter().find(|s| s.name == name).unwrap();
            assert!(
                name.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'),
                "{name}"
            );
            assert_eq!(spec.class.name(), row.class, "{name}");
            assert_eq!(spec.status, row.http, "{name}");
            let cli = spec
                .exit_code
                .map_or_else(|| "—".to_owned(), |c| c.to_string());
            if name == "catalog_unavailable" {
                assert_eq!(row.cli, "1, 2 or 3");
            } else {
                assert_eq!(cli, row.cli, "{name}");
            }
        }
    }
}

/// ER2: an error's exit code is its class's (usage 2, refused 3, error 1,
/// auth none) or none where the CLI has no counterpart, but for
/// `upload_not_found` (1) and `catalog_unavailable`, which takes its
/// reason's: 3 for a newer schema, 2 for not a catalog, linked or not
/// Unicode, 1 when unreadable.
#[test]
fn exit_codes_follow_the_class() {
    for spec in &SPECS {
        let class = match spec.class {
            Class::Usage => Some(2),
            Class::Refused => Some(3),
            Class::Error => Some(1),
            Class::Auth => None,
        };
        let allowed = match spec.name {
            "upload_not_found" => Some(1),
            _ => class,
        };
        assert!(
            spec.exit_code.is_none() || spec.exit_code == allowed,
            "{}",
            spec.name
        );
    }
    let exits: Vec<(&str, u8)> = [
        CatalogReason::NewerSchema,
        CatalogReason::NotACatalog,
        CatalogReason::Linked,
        CatalogReason::NotUnicode,
        CatalogReason::Unreadable,
    ]
    .iter()
    .map(|r| (r.name(), r.exit_code()))
    .collect();
    assert_eq!(
        exits,
        [
            ("newer_schema", 3),
            ("not_a_catalog", 2),
            ("linked", 2),
            ("not_unicode", 2),
            ("unreadable", 1)
        ]
    );
    let problem =
        ApiError::catalog_unavailable(CatalogReason::NewerSchema).document(&Method::GET, "r_x");
    assert_eq!(problem.exit_code, Some(3));
    let problem = ApiError::new(ErrorCode::NotFound, "x").document(&Method::GET, "r_x");
    assert_eq!(problem.exit_code, None);
}

/// ER3: an `internal` answer carries the code and the request id alone;
/// the panic's message, which names a path, is in the log only.
#[tokio::test]
async fn an_internal_error_says_nothing_but_its_code() {
    support::logs::capture();
    let server = TestServer::fresh();
    let planted = "boom at /etc/fitment/secret.rs:12";
    server
        .server()
        .seams()
        .hooks
        .set("info", Action::Panic(planted.into()));
    let reply = server.get("/v1/info").await;
    assert_eq!(reply.status, 500);
    assert_eq!(reply.code(), "internal");
    let doc = reply.json();
    assert_eq!(doc["details"], serde_json::json!({}));
    assert_eq!(doc["request_id"].as_str(), reply.header("x-request-id"));
    assert!(!String::from_utf8_lossy(&reply.body).contains("boom"));
    assert!(
        support::logs::text().contains(planted),
        "the panic is logged"
    );
    server
        .server()
        .seams()
        .hooks
        .set("info.blocking", Action::Panic(planted.into()));
    server.server().seams().hooks.clear("info");
    let reply = server.get("/v1/info").await;
    assert_eq!(reply.code(), "internal");
    assert!(!String::from_utf8_lossy(&reply.body).contains("boom"));
}

/// ER3: a catalog that cannot be opened is answered with its reason, never
/// the library error's text, which names the catalog's path; the log names
/// it relative to the configured root (OP1).
#[tokio::test]
async fn a_catalog_error_names_no_path() {
    support::logs::capture();
    let server = TestServer::fresh();
    std::fs::remove_file(server.dirs.catalog.join("FITMENT-CATALOG")).unwrap();
    let reply = server.get("/v1/info").await;
    assert_eq!(reply.code(), "catalog_unavailable");
    let text = String::from_utf8_lossy(&reply.body).into_owned();
    assert!(!text.contains(&server.dirs.catalog.display().to_string()));
    assert!(!text.contains("fitment catalog init"));
}

/// ER4: `details.refusals` is `[{code, message, instance}]` exactly as
/// `features.v1` writes a refused file's refusals.
#[test]
fn refusals_keep_their_own_codes() {
    let bytes = std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../fitment-core/tests/data/r04_assembly.stp"),
    )
    .unwrap();
    let document = fitment_core::FeaturesDocument::read(&bytes);
    assert_ne!(document.refusals().len(), 0);
    let written: serde_json::Value = serde_json::from_str(&document.to_json()).unwrap();
    let problem = ApiError::new(ErrorCode::SocketRefused, "refused")
        .with_refusals(document.refusals())
        .document(&Method::POST, "r_x");
    assert_eq!(problem.details["refusals"], written["refusals"]);
}

/// ER5: usage is answered before anything is read: the route and method
/// first, then a body where none is taken, then `Accept`, and only then
/// the catalog.
#[tokio::test]
async fn usage_errors_come_first() {
    let server = TestServer::fresh();
    std::fs::remove_file(server.dirs.catalog.join("FITMENT-CATALOG")).unwrap();
    let both = Call::get("/v1/info")
        .header("accept", "text/html")
        .body(b"{}");
    assert_eq!(server.send(both).await.code(), "body_too_large");
    let unacceptable = Call::get("/v1/info").header("accept", "text/html");
    assert_eq!(server.send(unacceptable).await.code(), "not_acceptable");
    let wrong_method = Call::new(Method::POST, "/v1/info").body(b"{}");
    assert_eq!(server.send(wrong_method).await.code(), "method_not_allowed");
    assert_eq!(server.get("/v1/info").await.code(), "catalog_unavailable");
}

/// ER6: the retryable codes are exactly section 7.12's;
/// `request_timeout` is retryable on `GET`, `HEAD` and an upload's `PUT`
/// only; a retryable answer carries `Retry-After`.
#[tokio::test]
async fn retryable_is_stated_and_depends_on_the_route() {
    let retryable: Vec<&str> = SPECS
        .iter()
        .filter(|s| s.retryable)
        .map(|s| s.name)
        .collect();
    assert_eq!(
        retryable,
        [
            "upload_too_slow",
            "rate_limited",
            "too_many_jobs",
            "too_many_streams",
            "catalog_busy",
            "queue_full",
            "shutting_down",
        ]
    );
    let timeout = ErrorCode::RequestTimeout;
    assert!(timeout.retryable_on(&Method::GET, false));
    assert!(timeout.retryable_on(&Method::HEAD, false));
    assert!(timeout.retryable_on(&Method::PUT, true));
    assert!(!timeout.retryable_on(&Method::PUT, false));
    assert!(!timeout.retryable_on(&Method::POST, false));
    assert!(!timeout.retryable_on(&Method::DELETE, false));
    let server = TestServer::fresh();
    server.server().drain();
    let reply = server.get("/v1/ready").await;
    assert_eq!(reply.json()["retryable"], true);
    assert!(reply.header("retry-after").is_some());
}

/// ER7: each class answers in its range: usage and refused `4xx`, error
/// `5xx`, auth `401` or `403`; a code with no status is job-only, and
/// answering one is a bug answered as `internal`.
#[tokio::test]
async fn statuses_follow_er7() {
    for spec in &SPECS {
        let Some(status) = spec.status else {
            continue;
        };
        let ok = match spec.class {
            Class::Usage | Class::Refused => (400..500).contains(&status),
            Class::Error => (500..600).contains(&status),
            Class::Auth => status == 401 || status == 403,
        };
        assert!(ok, "{} {status}", spec.name);
    }
    let status = |code: ErrorCode| code.status().map(|s| s.as_u16());
    assert_eq!(status(ErrorCode::UploadTooSlow), Some(408));
    assert_eq!(status(ErrorCode::BytesNotKept), Some(410));
    assert_eq!(status(ErrorCode::PreconditionFailed), Some(412));
    assert_eq!(status(ErrorCode::SocketStale), Some(422));
    assert_eq!(status(ErrorCode::RevisionRequired), Some(428));
    assert_eq!(status(ErrorCode::RateLimited), Some(429));
    assert_eq!(status(ErrorCode::CatalogBusy), Some(503));
    let response =
        axum::response::IntoResponse::into_response(ApiError::new(ErrorCode::CatalogStale, "x"));
    assert_eq!(response.status(), 500);
    assert_eq!(
        response.extensions().get::<ApiError>().map(ApiError::code),
        Some(ErrorCode::Internal)
    );
}

/// The keys of every object in `value`, recursively.
fn keys(value: &serde_json::Value, out: &mut Vec<String>) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, sub) in map {
                out.push(key.clone());
                keys(sub, out);
            }
        }
        serde_json::Value::Array(items) => items.iter().for_each(|v| keys(v, out)),
        _ => {}
    }
}

/// DC9: the API's documents have `format` and `format_version` first,
/// `snake_case` keys, `null` written rather than omitted, and pretty JSON
/// with a final newline.
#[tokio::test]
async fn the_apis_documents_follow_dc9() {
    let server = TestServer::fresh();
    for (path, format) in [("/v1/info", "fitment.info"), ("/v1/nope", "fitment.error")] {
        let reply = server.get(path).await;
        let text = String::from_utf8(reply.body.clone()).unwrap();
        assert!(
            text.starts_with(&format!(
                "{{\n  \"format\": \"{format}\",\n  \"format_version\": 1,\n"
            )),
            "{text}"
        );
        assert!(text.ends_with("}\n"));
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        // Pretty: one key per line, two spaces a level (serde_json's form).
        assert_eq!(
            text.lines().count(),
            serde_json::to_string_pretty(&value)
                .unwrap()
                .lines()
                .count()
        );
        let mut all = Vec::new();
        keys(&value, &mut all);
        for key in all {
            assert!(
                key.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_'),
                "{key}"
            );
        }
        assert_eq!(
            reply.header("fitment-format"),
            Some(format!("{format}/1").as_str())
        );
        let digest = format!(
            "sha-256=:{}:",
            fitment_server::http::json::base64(&fitment_core::sha256(&reply.body))
        );
        assert_eq!(reply.header("repr-digest"), Some(digest.as_str()));
    }
    let info = server.get("/v1/info").await.json();
    assert!(
        info.as_object().unwrap().contains_key("fit_verifier") && info["fit_verifier"].is_null()
    );
    let error = server.get("/v1/nope").await.json();
    assert!(error.as_object().unwrap().contains_key("exit_code") && error["exit_code"].is_null());
    assert_eq!(error["type"], "https://fitment.dev/errors/not_found");
    assert_eq!(error["class"], "usage");
    assert_eq!(error["retryable"], false);
}
