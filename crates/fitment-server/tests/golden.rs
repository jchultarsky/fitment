//! VT6 (docs/research/m6-api.md; DC4, DC9 to DC11, ER1, ER7): the API's
//! own documents as goldens, `tests/golden/api/<operation>/<scenario>`
//! `.request` (method, path, headers, body) and `.response` (status, the
//! contract's headers, body), produced in process with ids and time fixed
//! (OP11), so every byte is exact. Every error code of `ErrorCode::ALL`
//! is produced by a real request here, or listed as job-only (section
//! 7.12's HTTP "—") or as waiting for the PR that adds its route.
//!
//! `FITMENT_BLESS=1` writes a golden that is missing, or one whose
//! format's version or the API's minor rose; never under `CI`.
//!
//! VT8: with `FITMENT_API_OUTPUTS=DIR`, `writes_the_api_outputs` writes
//! every response to DIR (`tools/api-outputs.sh`), which CI's
//! `determinism` job compares across Linux, macOS and Windows.
#![allow(clippy::disallowed_methods)] // reads FITMENT_BLESS, CI and FITMENT_API_OUTPUTS

mod support;

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use axum::http::Method;
use fitment_catalog::fs::CountingFs;
use fitment_server::Server;
use fitment_server::auth::Role;
use fitment_server::hooks::Action;
use fitment_server::http::error::ErrorCode;
use support::{Call, Dirs, Reply, TestServer};

/// The response headers a golden records: the contract's (DC4, `Allow`,
/// `Location`, `Retry-After`, `Preference-Applied`), the type, the cache
/// policy and the request id.
const HEADERS: [&str; 14] = [
    "allow",
    "cache-control",
    "content-type",
    "etag",
    "fitment-digest",
    "fitment-exit-code",
    "fitment-format",
    "location",
    "preference-applied",
    "repr-digest",
    "retry-after",
    "www-authenticate",
    "x-request-id",
    "content-location",
];

/// What a scenario does to a fresh server before its request.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Setup {
    None,
    /// The catalog's marker removed: `catalog_unavailable`.
    NoCatalog,
    /// Draining: `shutting_down`.
    Draining,
    /// A 1 s handler timeout and a 1.5 s hook: `request_timeout`.
    SlowHandler,
    /// A panicking handler: `internal`.
    Panic,
    /// `ada`, a reader with a password, and no token.
    Ada,
    /// As `Ada`, with one login a minute: the second is `rate_limited`.
    LoginOnce,
    /// One live token a user at most: a mint is `too_many_tokens`.
    OneToken,
    /// `ada` holds an upload of [`BRACKET`].
    Uploaded,
    /// `ada` holds uploads of [`BRACKET`] and [`SECOND`].
    TwoUploads,
    /// As `Uploaded`, and a queued job of hers names the upload (OB4).
    InUse,
    /// A 1 s throughput window and a 1.2 s wait before each chunk is read:
    /// `upload_too_slow`.
    SlowUpload,
    /// A 4 GiB upload cap over a 1 GiB quota: a declared 1.5 GiB is
    /// `quota_exceeded`.
    SmallQuota,
    /// The store's second file call (an upload's temporary file, after the
    /// start's look at `tmp/`) fails as a full disk: `storage_full`.
    DiskFull,
    /// Its third (the first chunk's write) fails otherwise: `io_error`.
    BrokenDisk,
}

/// The bytes of the upload goldens; their SHA-256s are in the paths.
const BRACKET: &[u8] = b"a bracket's bytes\n";
const SECOND: &[u8] = b"a second part's bytes\n";

/// Whose token a scenario presents (AU5): seeded with the fixed ids, so
/// every id in the golden is exact.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Principal {
    /// No credentials.
    None,
    /// `root`, an admin, and its admin token.
    Admin,
    /// `ada`, a reader with a password, and her reader token.
    Reader,
}

/// `ada`'s password in every scenario (registered with the leak scan).
const ADA: &str = "correct horse battery staple";

struct Scenario {
    operation: &'static str,
    name: &'static str,
    setup: Setup,
    principal: Principal,
    method: Method,
    path: &'static str,
    headers: &'static [(&'static str, &'static str)],
    body: &'static [u8],
}

#[allow(clippy::too_many_arguments)] // a scenario's fields, in its table's columns
const fn scenario(
    operation: &'static str,
    name: &'static str,
    setup: Setup,
    principal: Principal,
    method: Method,
    path: &'static str,
    headers: &'static [(&'static str, &'static str)],
    body: &'static [u8],
) -> Scenario {
    Scenario {
        operation,
        name,
        setup,
        principal,
        method,
        path,
        headers,
        body,
    }
}

const JSON: &[(&str, &str)] = &[("content-type", "application/json")];
const OCTETS: &[(&str, &str)] = &[("content-type", "application/octet-stream")];
const STEP: &[(&str, &str)] = &[("content-type", "model/step")];
const HUGE: &[(&str, &str)] = &[
    ("content-type", "application/octet-stream"),
    ("content-length", "200000000"),
];
const OVER_QUOTA: &[(&str, &str)] = &[
    ("content-type", "application/octet-stream"),
    ("content-length", "1610612736"),
];
const HTML: &[(&str, &str)] = &[("accept", "text/html")];
const TEXT: &[(&str, &str)] = &[("content-type", "text/plain")];

use Principal as P;
use Setup as S;

/// Every scenario: at least one per operation of `OPERATIONS` (XH14), and
/// one per error code a request can reach in this build.
#[rustfmt::skip]
const SCENARIOS: [Scenario; 56] = [
    scenario("health", "ok", S::None, P::None, Method::GET, "/v1/health", &[], b""),
    scenario("ready", "ok", S::None, P::None, Method::GET, "/v1/ready", &[], b""),
    scenario("ready", "catalog_unavailable", S::NoCatalog, P::None, Method::GET, "/v1/ready", &[], b""),
    scenario("ready", "shutting_down", S::Draining, P::None, Method::GET, "/v1/ready", &[], b""),
    scenario("ready", "bad_request", S::None, P::None, Method::GET, "/v1/ready?access_token=x", &[], b""),
    scenario("info", "ok", S::None, P::Admin, Method::GET, "/v1/info", &[], b""),
    scenario("info", "unauthenticated", S::None, P::None, Method::GET, "/v1/info", &[], b""),
    scenario("info", "not_acceptable", S::None, P::Admin, Method::GET, "/v1/info", HTML, b""),
    scenario("info", "body_too_large", S::None, P::Admin, Method::GET, "/v1/info", JSON, b"{}"),
    scenario("info", "method_not_allowed", S::None, P::None, Method::POST, "/v1/info", &[], b""),
    scenario("info", "request_timeout", S::SlowHandler, P::Admin, Method::GET, "/v1/info", &[], b""),
    scenario("info", "internal", S::Panic, P::Admin, Method::GET, "/v1/info", &[], b""),
    scenario("info", "invalid_parameter", S::None, P::Admin, Method::GET, "/v1/info?owner=u_x", &[], b""),
    scenario("openapi", "ok", S::None, P::None, Method::GET, "/v1/openapi.json", &[], b""),
    scenario("unrouted", "not_found", S::None, P::None, Method::GET, "/v1/nope", &[], b""),
    scenario("uploads_put", "created", S::None, P::Reader, Method::PUT,
        "/v1/uploads/sha256:62048ce1d1d48e0786d0a9e001ab0c72354bb9faa408a4984f9ceb2b9f73ccff", OCTETS, BRACKET),
    scenario("uploads_put", "ok", S::Uploaded, P::Reader, Method::PUT,
        "/v1/uploads/sha256:62048CE1D1D48E0786D0A9E001AB0C72354BB9FAA408A4984F9CEB2B9F73CCFF", OCTETS, BRACKET),
    scenario("uploads_put", "upload_digest_mismatch", S::None, P::Reader, Method::PUT,
        "/v1/uploads/sha256:953c8fe2df5fddd23a7174397482e83630d540574340fbcb5ffac147e2ee1cf8", OCTETS, BRACKET),
    scenario("uploads_put", "invalid_reference", S::None, P::Reader, Method::PUT,
        "/v1/uploads/sha256:62048ce1", OCTETS, BRACKET),
    scenario("uploads_put", "too_large", S::None, P::Reader, Method::PUT,
        "/v1/uploads/sha256:62048ce1d1d48e0786d0a9e001ab0c72354bb9faa408a4984f9ceb2b9f73ccff", HUGE, BRACKET),
    scenario("uploads_put", "upload_too_slow", S::SlowUpload, P::Reader, Method::PUT,
        "/v1/uploads/sha256:62048ce1d1d48e0786d0a9e001ab0c72354bb9faa408a4984f9ceb2b9f73ccff", OCTETS, BRACKET),
    scenario("uploads_put", "storage_full", S::DiskFull, P::Reader, Method::PUT,
        "/v1/uploads/sha256:62048ce1d1d48e0786d0a9e001ab0c72354bb9faa408a4984f9ceb2b9f73ccff", OCTETS, BRACKET),
    scenario("uploads_put", "io_error", S::BrokenDisk, P::Reader, Method::PUT,
        "/v1/uploads/sha256:62048ce1d1d48e0786d0a9e001ab0c72354bb9faa408a4984f9ceb2b9f73ccff", OCTETS, BRACKET),
    scenario("uploads_post", "created", S::None, P::Reader, Method::POST, "/v1/uploads", STEP, SECOND),
    scenario("uploads_post", "quota_exceeded", S::SmallQuota, P::Reader, Method::POST, "/v1/uploads",
        OVER_QUOTA, SECOND),
    scenario("uploads_head", "ok", S::Uploaded, P::Reader, Method::HEAD,
        "/v1/uploads/sha256:62048ce1d1d48e0786d0a9e001ab0c72354bb9faa408a4984f9ceb2b9f73ccff", &[], b""),
    scenario("uploads_head", "not_found", S::None, P::Reader, Method::HEAD,
        "/v1/uploads/sha256:62048ce1d1d48e0786d0a9e001ab0c72354bb9faa408a4984f9ceb2b9f73ccff", &[], b""),
    scenario("uploads_get", "ok", S::Uploaded, P::Reader, Method::GET,
        "/v1/uploads/sha256:62048ce1d1d48e0786d0a9e001ab0c72354bb9faa408a4984f9ceb2b9f73ccff", &[], b""),
    scenario("uploads_get", "not_found", S::Uploaded, P::Admin, Method::GET,
        "/v1/uploads/sha256:62048ce1d1d48e0786d0a9e001ab0c72354bb9faa408a4984f9ceb2b9f73ccff", &[], b""),
    scenario("uploads_list", "ok", S::TwoUploads, P::Reader, Method::GET, "/v1/uploads?limit=1", &[], b""),
    scenario("uploads_delete", "deleted", S::Uploaded, P::Reader, Method::DELETE,
        "/v1/uploads/sha256:62048ce1d1d48e0786d0a9e001ab0c72354bb9faa408a4984f9ceb2b9f73ccff", &[], b""),
    scenario("uploads_delete", "upload_in_use", S::InUse, P::Reader, Method::DELETE,
        "/v1/uploads/sha256:62048ce1d1d48e0786d0a9e001ab0c72354bb9faa408a4984f9ceb2b9f73ccff", &[], b""),
    scenario("uploads_delete", "not_found", S::None, P::Reader, Method::DELETE,
        "/v1/uploads/sha256:62048ce1d1d48e0786d0a9e001ab0c72354bb9faa408a4984f9ceb2b9f73ccff", &[], b""),
    scenario("login", "created", S::Ada, P::None, Method::POST, "/v1/auth/login", JSON,
        br#"{"username": "Ada", "password": "correct horse battery staple", "token_name": "laptop", "days": 30}"#),
    scenario("login", "invalid_credentials", S::Ada, P::None, Method::POST, "/v1/auth/login", JSON,
        br#"{"username": "ada", "password": "not her password at all"}"#),
    scenario("login", "rate_limited", S::LoginOnce, P::None, Method::POST, "/v1/auth/login", JSON,
        br#"{"username": "ada", "password": "correct horse battery staple"}"#),
    scenario("login", "bad_request", S::Ada, P::None, Method::POST, "/v1/auth/login", JSON,
        br#"{"username": "ada", "password": "correct horse battery staple", "role": "admin"}"#),
    scenario("login", "unsupported_media_type", S::Ada, P::None, Method::POST, "/v1/auth/login", TEXT,
        br#"{"username": "ada", "password": "correct horse battery staple"}"#),
    scenario("me", "ok", S::None, P::Reader, Method::GET, "/v1/auth/me", &[], b""),
    scenario("password", "changed", S::None, P::Reader, Method::POST, "/v1/auth/password", JSON,
        br#"{"current": "correct horse battery staple", "new": "a much longer passphrase now"}"#),
    scenario("password", "invalid_credentials", S::None, P::Reader, Method::POST, "/v1/auth/password", JSON,
        br#"{"current": "not her password at all", "new": "a much longer passphrase now"}"#),
    scenario("tokens_list", "ok", S::None, P::Reader, Method::GET, "/v1/auth/tokens", &[], b""),
    scenario("tokens_list", "forbidden", S::None, P::Reader, Method::GET,
        "/v1/auth/tokens?owner=u_7zzzzzzzzzzzzzzzzzzzzzzzzz", &[], b""),
    scenario("tokens_mint", "created", S::None, P::Reader, Method::POST, "/v1/auth/tokens", JSON,
        br#"{"name": "ci", "days": 7}"#),
    scenario("tokens_mint", "invalid_parameter", S::None, P::Reader, Method::POST, "/v1/auth/tokens", JSON,
        br#"{"name": "ci", "role": "writer"}"#),
    scenario("tokens_mint", "too_many_tokens", S::OneToken, P::Reader, Method::POST, "/v1/auth/tokens", JSON,
        br#"{"name": "ci"}"#),
    scenario("tokens_revoke", "current", S::None, P::Reader, Method::DELETE, "/v1/auth/tokens/current", &[], b""),
    scenario("tokens_revoke", "not_found", S::None, P::Reader, Method::DELETE,
        "/v1/auth/tokens/t_7zzzzzzzzzzzzzzzzzzzzzzzzz", &[], b""),
    scenario("users_list", "ok", S::Ada, P::Admin, Method::GET, "/v1/auth/users?limit=1", &[], b""),
    scenario("users_list", "forbidden", S::None, P::Reader, Method::GET, "/v1/auth/users", &[], b""),
    scenario("users_create", "created", S::None, P::Admin, Method::POST, "/v1/auth/users", JSON,
        br#"{"username": "grace", "role": "writer", "password": null}"#),
    scenario("users_create", "username_taken", S::None, P::Admin, Method::POST, "/v1/auth/users", JSON,
        br#"{"username": "ROOT", "role": "reader"}"#),
    scenario("users_update", "ok", S::None, P::Admin, Method::PATCH,
        "/v1/auth/users/u_00000000000000000000000001", JSON, br#"{"role": "admin", "disabled": false}"#),
    scenario("users_update", "last_admin", S::None, P::Admin, Method::PATCH,
        "/v1/auth/users/u_00000000000000000000000001", JSON, br#"{"role": "reader"}"#),
    scenario("users_update", "not_found", S::None, P::Admin, Method::PATCH,
        "/v1/auth/users/u_7zzzzzzzzzzzzzzzzzzzzzzzzz", JSON, br#"{"disabled": true}"#),
    scenario("users_update", "unauthenticated", S::None, P::None, Method::PATCH,
        "/v1/auth/users/u_00000000000000000000000001", JSON, br#"{"disabled": true}"#),
];

/// The codes whose routes come in a later PR of section 12, with that PR:
/// each must be produced by a golden by then. `catalog_busy` and
/// `catalog_damaged` are reachable by `/v1/info` today, but only through a
/// fault no golden stages without the seams of PR 7 (a busy holder, a
/// damaged row); `too_many_streams` by two uploads at once, which PR 5's
/// event streams' golden shows; `blob_damaged` by a read of a stored
/// object, which no route of PR 4 makes (PR 5's assembly bytes do). Section 7.12's job-only codes (HTTP "—")
/// are not listed: no request answers one; the job goldens show them.
const PENDING: [(&str, u32); 38] = [
    ("not_an_id", 7),
    ("name_invalid", 5),
    ("label_reserved", 6),
    ("no_candidates", 6),
    ("options_conflict", 6),
    ("upload_not_found", 5),
    ("entry_not_found", 7),
    ("no_document", 5),
    ("reference_ambiguous", 7),
    ("revision_not_current", 6),
    ("entry_deleted", 7),
    ("assembly_in_use", 5),
    ("original_not_stored", 6),
    ("job_finished", 5),
    ("bytes_not_kept", 7),
    ("derived_rows_gone", 7),
    ("precondition_failed", 7),
    ("idempotency_key_reused", 5),
    ("carry_from_foreign", 6),
    ("revision_required", 6),
    ("too_many_jobs", 5),
    ("too_many_streams", 5),
    ("review_unknown_requirement", 6),
    ("review_unknown_socket", 6),
    ("review_differs_between_sockets", 6),
    ("review_note_required", 6),
    ("review_note_too_long", 6),
    ("review_refused_document", 6),
    ("socket_unreadable", 6),
    ("socket_refused", 6),
    ("socket_stale", 6),
    ("socket_unreviewed", 6),
    ("entry_stale", 7),
    ("derived_stale", 5),
    ("catalog_busy", 7),
    ("queue_full", 5),
    ("blob_damaged", 5),
    ("catalog_damaged", 7),
];

/// The PR this tree builds (section 12): a pending code waits for a later
/// one, at most the exit.
const PR_NOW: u32 = 4;

fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/api")
}

fn openapi_marker() -> &'static str {
    "<the bytes of crates/fitment-server/openapi.json>\n"
}

/// The masks a golden writes for a secret (AU3): a token's secret, shown
/// once in its creating response, never in a committed file.
const SECRET_MASK: &str = "<the token's secret, shown once>";

/// Seeds `username` with the server's fixed ids and time: a person of
/// `role`, with `password` if given, and a token of the role if `token`.
fn seed(
    server: &Server,
    username: &str,
    role: Role,
    password: Option<&str>,
    token: bool,
) -> Option<String> {
    let seams = server.seams();
    let phc =
        password.map(|p| fitment_server::auth::password::hash(p, seams.auth.pepper()).unwrap());
    let id = seams.ids.id("u");
    let now = seams.ids.now_ms();
    let fold = username.to_ascii_lowercase();
    let user = seams
        .db
        .create_user(&id, username, &fold, role, phc.as_deref(), now)
        .unwrap();
    token.then(|| {
        let from = fitment_server::state::MintFrom::Offline { user: user.key };
        let minted = fitment_server::auth::mint(
            seams.db,
            seams.ids,
            &from,
            "seeded",
            role,
            now,
            now + 90 * fitment_server::auth::DAY_MS,
            20,
        )
        .unwrap();
        support::scan::secret(minted.secret.expose());
        minted.secret.expose().to_owned()
    })
}

/// Runs `scenario` on a fresh in-process server.
async fn run(scenario: &Scenario) -> Reply {
    let dirs = Dirs::new();
    let mut config = dirs.config();
    match scenario.setup {
        S::SlowHandler => config.server.handler_timeout_seconds = 1,
        S::LoginOnce => config.auth.login_per_minute = 1,
        S::OneToken => config.auth.max_live_tokens = 1,
        S::SlowUpload => config.limits.upload_window_seconds = 1,
        S::SmallQuota => {
            config.limits.max_upload_mib = 4096;
            config.limits.user_quota_gib = 1;
        }
        _ => {}
    }
    let fs: std::sync::Arc<dyn fitment_catalog::Fs> = match scenario.setup {
        S::DiskFull => std::sync::Arc::new(CountingFs::failing_at(1, io::ErrorKind::StorageFull)),
        S::BrokenDisk => std::sync::Arc::new(CountingFs::failing_at(2, io::ErrorKind::Other)),
        _ => std::sync::Arc::new(fitment_catalog::StdFs),
    };
    let server = TestServer::in_process_with_fs(dirs, config, fs);
    let hooks = &server.server().seams().hooks;
    match scenario.setup {
        S::NoCatalog => {
            std::fs::remove_file(server.dirs.catalog.join("FITMENT-CATALOG")).unwrap();
        }
        S::Draining => server.server().drain(),
        S::SlowHandler => hooks.set("info", Action::Delay(Duration::from_millis(1500))),
        S::Panic => hooks.set("info", Action::Panic("a golden's planted panic".into())),
        S::SlowUpload => hooks.set("upload.chunk", Action::Delay(Duration::from_millis(1200))),
        _ => {}
    }
    support::password(ADA);
    let token = match scenario.principal {
        P::Admin => seed(server.server(), "root", Role::Admin, None, true),
        P::Reader => seed(server.server(), "ada", Role::Reader, Some(ADA), true),
        P::None => None,
    };
    if matches!(scenario.setup, S::Ada | S::LoginOnce) {
        seed(server.server(), "ada", Role::Reader, Some(ADA), false);
    }
    if matches!(scenario.setup, S::Uploaded | S::TwoUploads | S::InUse) {
        let seams = server.server().seams();
        if seams.db.user_by_fold("ada").unwrap().is_none() {
            seed(server.server(), "ada", Role::Reader, Some(ADA), false);
        }
        let ada = seams.db.user_by_fold("ada").unwrap().unwrap();
        support::seed_upload(server.server(), ada.key, BRACKET);
        if scenario.setup == S::TwoUploads {
            support::seed_upload(server.server(), ada.key, SECOND);
        }
        if scenario.setup == S::InUse {
            let sha256 = fitment_core::sha256_hex(BRACKET);
            support::seed_job_input(&server, ada.key, &sha256, "match", "queued");
        }
    }
    if scenario.setup == S::LoginOnce {
        let login = br#"{"username": "ada", "password": "correct horse battery staple"}"#;
        let first = Call::new(Method::POST, "/v1/auth/login")
            .header("content-type", "application/json")
            .body(login)
            .anonymous();
        assert_eq!(server.send(first).await.status, 201);
    }
    let mut call = Call::new(scenario.method.clone(), scenario.path)
        .body(scenario.body)
        .anonymous();
    for (name, value) in scenario.headers {
        call = call.header(name, value);
    }
    if let Some(token) = token {
        call = call.bearer(&token);
    }
    server.send(call).await
}

fn render_request(scenario: &Scenario) -> String {
    let mut text = format!("{} {}\n", scenario.method, scenario.path);
    match scenario.principal {
        P::Admin => text.push_str("authorization: Bearer <root's admin token>\n"),
        P::Reader => text.push_str("authorization: Bearer <ada's reader token>\n"),
        P::None => {}
    }
    for (name, value) in scenario.headers {
        let _ = writeln!(text, "{name}: {value}");
    }
    text.push('\n');
    text.push_str(&String::from_utf8_lossy(scenario.body));
    text
}

fn render_response(scenario: &Scenario, reply: &Reply) -> String {
    let mut text = format!("{}\n", reply.status.as_u16());
    for name in HEADERS {
        if let Some(value) = reply.header(name) {
            let _ = writeln!(text, "{name}: {value}");
        }
    }
    text.push('\n');
    if scenario.operation == "openapi" {
        assert_eq!(
            reply.body,
            fitment_server::OPENAPI,
            "the served OpenAPI document"
        );
        text.push_str(openapi_marker());
    } else {
        let body = String::from_utf8_lossy(&reply.body).into_owned();
        let secret = serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .and_then(|v| v["secret"].as_str().map(str::to_owned));
        match secret {
            Some(secret) => text.push_str(&body.replace(&secret, SECRET_MASK)),
            None => text.push_str(&body),
        }
    }
    text
}

/// The versions a golden records: its `fitment-format` and, for the
/// OpenAPI document, the API's version. A rewrite needs one to rise.
fn recorded(text: &str) -> Vec<(String, u32)> {
    let mut out = Vec::new();
    for line in text.lines() {
        if let Some(value) = line.strip_prefix("fitment-format: ") {
            if let Some((format, version)) = value.rsplit_once('/') {
                out.push((format.to_owned(), version.parse().unwrap_or(0)));
            }
        }
    }
    if text.contains(openapi_marker()) {
        let minor = fitment_server::info::API_MINOR;
        out.push(("api".into(), minor));
    }
    out
}

fn rose(old: &str, new: &str) -> bool {
    let (old, new) = (recorded(old), recorded(new));
    new.iter().any(|(format, version)| {
        old.iter()
            .find(|(f, _)| f == format)
            .is_some_and(|(_, v)| version > v)
    })
}

/// Compares `text` with the golden at `path`, blessing as the module says.
fn compare(path: &Path, text: &str, failures: &mut Vec<String>) {
    let old = std::fs::read_to_string(path).ok();
    if old.as_deref() == Some(text) {
        return;
    }
    let bless = std::env::var_os("FITMENT_BLESS").is_some_and(|v| v == "1")
        && std::env::var_os("CI").is_none();
    let may = old.as_deref().is_none_or(|old| rose(old, text));
    if bless && may {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
        eprintln!("wrote {}", path.display());
        return;
    }
    failures.push(format!(
        "{} differs (bless only with a version's rise):\n--- golden\n{}\n--- now\n{text}",
        path.display(),
        old.unwrap_or_default()
    ));
}

/// VT6: every scenario's request and response equal their goldens, byte
/// for byte; every operation has one; every error code is produced by
/// one, or is job-only, or waits for a later PR (and not past the exit).
#[tokio::test]
async fn the_apis_documents_are_their_goldens() {
    let mut failures = Vec::new();
    let mut produced = BTreeSet::new();
    let mut operations = BTreeSet::new();
    for scenario in &SCENARIOS {
        let reply = run(scenario).await;
        // A HEAD's answer has no body: its code is not read from it.
        if reply.header("content-type") == Some("application/problem+json")
            && scenario.method != Method::HEAD
        {
            produced.insert(reply.code());
        }
        operations.insert(scenario.operation);
        let base = golden_dir().join(scenario.operation).join(scenario.name);
        compare(
            &base.with_extension("request"),
            &render_request(scenario),
            &mut failures,
        );
        compare(
            &base.with_extension("response"),
            &render_response(scenario, &reply),
            &mut failures,
        );
    }
    for op in &fitment_server::http::operations::OPERATIONS {
        assert!(
            operations.contains(op.id),
            "no golden for the operation {}",
            op.id
        );
    }
    for code in ErrorCode::ALL {
        let name = code.name();
        let job_only = code.status().is_none();
        let pending = PENDING.iter().find(|(n, _)| *n == name);
        match (produced.contains(name), pending) {
            (true, Some(_)) => failures.push(format!("{name}: produced, yet listed as pending")),
            (false, None) if !job_only => {
                failures.push(format!("{name}: neither produced, job-only nor pending"));
            }
            (_, Some((_, pr))) if *pr <= PR_NOW || *pr > 9 => {
                failures.push(format!(
                    "{name}: pending for PR {pr}, not a later PR up to the exit"
                ));
            }
            _ => {}
        }
    }
    for (name, _) in PENDING {
        assert!(
            ErrorCode::ALL.iter().any(|c| c.name() == name),
            "{name} is no code"
        );
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

/// VT8: with `FITMENT_API_OUTPUTS` set, every scenario's response (and
/// the OpenAPI document) is written under it, relative to the workspace,
/// for CI to compare across operating systems.
#[tokio::test]
async fn writes_the_api_outputs() {
    let Some(out) = std::env::var_os("FITMENT_API_OUTPUTS") else {
        return;
    };
    let out = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(out);
    for scenario in &SCENARIOS {
        let reply = run(scenario).await;
        let path = out
            .join(scenario.operation)
            .join(format!("{}.response", scenario.name));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, render_response(scenario, &reply)).unwrap();
    }
    std::fs::write(out.join("openapi.json"), fitment_server::OPENAPI).unwrap();
}
