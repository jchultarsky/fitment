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

/// What a scenario does to a fresh server before its request; PR 6's
/// from `Reviewed` on.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
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
    /// `ada` holds an upload of the unit part `p01_through.stp`.
    Part,
    /// `ada` holds an upload of `s02_cbore.stp`.
    Assembly,
    /// `ada` registered `s02_cbore.stp`.
    Registered,
    /// As `Registered`, its parts row of an older extractor: `derived_stale`.
    Stale,
    /// As `Registered`, its stored bytes changed on disk: `blob_damaged`.
    Damaged,
    /// As `Registered`, and `BLOCK`'s socket extracted.
    Extracted,
    /// As `Part`, and a `features` job of it finished.
    Ran,
    /// As `Part`, the dispatcher held: jobs stay queued.
    Held,
    /// As `Held`, one job queued.
    Queued,
    /// As `Queued`, one job queued a user at most: `too_many_jobs`.
    OwnerFull,
    /// As `Queued`, one job queued in the lane at most: `queue_full`.
    LaneFull,
    /// As `Part`, a `features` job made under `Idempotency-Key: golden-key`.
    Keyed,
    /// As `Part`, a job panics once it has read its input.
    JobPanic,
    /// As `Queued`, one stream a user at most, and one open.
    Streaming,
    /// As `Extracted`, and `@r1` reviewed: `@r2` is current.
    Reviewed,
    /// `ada` registered `s03_dowels.stp` and extracted `DOWEL`, whose
    /// sockets hold one shaft each that differs between them.
    Dowels,
    /// As `Extracted`, and a refused socket document seeded as `@r2`.
    RefusedRevision,
    /// As `Extracted`, the build's socket rules raised by one (VT32).
    Shifted,
    /// `ada` holds an upload of the CLI's `s02_cbore--BLOCK.json` and not
    /// its assembly.
    Document,
    /// `ada` holds an upload of that document, its socket rules one older.
    StaleDocument,
    /// `ada` holds an upload of that document, a value edited.
    BrokenDocument,
    /// `ada` holds an upload of a refused socket document.
    RefusedDocument,
    /// As `Registered`, and `ada` holds an upload of the CLI's
    /// `s01_bolted--BRACKET.json`.
    Foreign,
    /// As `Extracted`, and `ada` holds uploads of `m_block02.stp` and
    /// `m_block02_cb_deep.stp`.
    Candidates,
    /// As `Candidates`, and a match over both finished.
    Matched,
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
const WAIT: &[(&str, &str)] = &[("content-type", "application/json"), ("prefer", "wait=30")];
const KEYED: &[(&str, &str)] = &[
    ("content-type", "application/json"),
    ("prefer", "wait=30"),
    ("idempotency-key", "golden-key"),
];
const SAME_STEP: &[(&str, &str)] = &[(
    "if-none-match",
    "\"sha256:2b19eec602abd26780ffecf813b3f8a8c9198c7c3fb497fe8ccc99726ed9959a\"",
)];
const SAME_JOB: &[(&str, &str)] = &[("if-none-match", "W/\"j4.succeeded\"")];
const TEXT: &[(&str, &str)] = &[("content-type", "text/plain")];

/// The candidates of the match scenarios, by their SHA-256s.
const MATCH_BODY: &[u8] = br#"{"socket": "sk_4c08cde1bddb3df26f3b@r1", "candidates": {"files": [
    {"upload": "sha256:1f021de7913c486df1f45d44bc6306c39f1b7374d9eccb818e27fbc6990db4ea", "label": "m_block02.stp"},
    {"upload": "sha256:4f68da82466fb9b036f4773f9aeefa44a8abef375f45abf155b47f22f56ba6be", "label": "m_block02_cb_deep.stp"}]}}"#;

use Principal as P;
use Setup as S;

/// Every scenario: at least one per operation of `OPERATIONS` (XH14), and
/// one per error code a request can reach in this build.
#[rustfmt::skip]
const SCENARIOS: [Scenario; 126] = [
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
    scenario("features_create", "succeeded", S::Part, P::Reader, Method::POST, "/v1/features", WAIT,
        br#"{"upload": "sha256:e0382a9dd3495268bd9e66fc09851fc1c91ee91b2186e0a1f70d4f584a6b9945"}"#),
    scenario("features_create", "refused", S::Uploaded, P::Reader, Method::POST, "/v1/features", WAIT,
        br#"{"upload": "sha256:62048ce1d1d48e0786d0a9e001ab0c72354bb9faa408a4984f9ceb2b9f73ccff"}"#),
    scenario("features_create", "failed", S::JobPanic, P::Reader, Method::POST, "/v1/features", WAIT,
        br#"{"upload": "sha256:e0382a9dd3495268bd9e66fc09851fc1c91ee91b2186e0a1f70d4f584a6b9945"}"#),
    scenario("features_create", "queued", S::Held, P::Reader, Method::POST, "/v1/features", JSON,
        br#"{"upload": "sha256:e0382a9dd3495268bd9e66fc09851fc1c91ee91b2186e0a1f70d4f584a6b9945"}"#),
    scenario("features_create", "replayed", S::Keyed, P::Reader, Method::POST, "/v1/features", KEYED,
        br#"{ "upload" : "sha256:e0382a9dd3495268bd9e66fc09851fc1c91ee91b2186e0a1f70d4f584a6b9945" }"#),
    scenario("features_create", "idempotency_key_reused", S::Keyed, P::Reader, Method::POST,
        "/v1/features", KEYED,
        br#"{"upload": "sha256:E0382A9DD3495268BD9E66FC09851FC1C91EE91B2186E0A1F70D4F584A6B9945"}"#),
    scenario("features_create", "upload_not_found", S::None, P::Reader, Method::POST, "/v1/features", WAIT,
        br#"{"upload": "sha256:e0382a9dd3495268bd9e66fc09851fc1c91ee91b2186e0a1f70d4f584a6b9945"}"#),
    scenario("features_create", "too_many_jobs", S::OwnerFull, P::Reader, Method::POST, "/v1/features", JSON,
        br#"{"upload": "sha256:e0382a9dd3495268bd9e66fc09851fc1c91ee91b2186e0a1f70d4f584a6b9945"}"#),
    scenario("features_create", "queue_full", S::LaneFull, P::Reader, Method::POST, "/v1/features", JSON,
        br#"{"upload": "sha256:e0382a9dd3495268bd9e66fc09851fc1c91ee91b2186e0a1f70d4f584a6b9945"}"#),
    scenario("assemblies_create", "succeeded", S::Assembly, P::Reader, Method::POST, "/v1/assemblies", WAIT,
        br#"{"upload": "sha256:2b19eec602abd26780ffecf813b3f8a8c9198c7c3fb497fe8ccc99726ed9959a", "file_name": "s02_cbore.stp"}"#),
    scenario("assemblies_create", "registered", S::Registered, P::Reader, Method::POST, "/v1/assemblies", WAIT,
        br#"{"upload": "sha256:2b19eec602abd26780ffecf813b3f8a8c9198c7c3fb497fe8ccc99726ed9959a", "file_name": "base.stp"}"#),
    scenario("assemblies_create", "name_invalid", S::Assembly, P::Reader, Method::POST, "/v1/assemblies", WAIT,
        br#"{"upload": "sha256:2b19eec602abd26780ffecf813b3f8a8c9198c7c3fb497fe8ccc99726ed9959a", "file_name": "../s02.stp"}"#),
    scenario("assemblies_list", "ok", S::Registered, P::Reader, Method::GET, "/v1/assemblies", &[], b""),
    scenario("assemblies_get", "ok", S::Extracted, P::Reader, Method::GET,
        "/v1/assemblies/sha256:2b19eec602abd26780ffecf813b3f8a8c9198c7c3fb497fe8ccc99726ed9959a", &[], b""),
    scenario("assemblies_get", "not_found", S::None, P::Reader, Method::GET,
        "/v1/assemblies/sha256:2b19eec602abd26780ffecf813b3f8a8c9198c7c3fb497fe8ccc99726ed9959a", &[], b""),
    scenario("assemblies_parts", "ok", S::Registered, P::Reader, Method::GET,
        "/v1/assemblies/sha256:2b19eec602abd26780ffecf813b3f8a8c9198c7c3fb497fe8ccc99726ed9959a/parts", &[], b""),
    scenario("assemblies_parts", "derived_stale", S::Stale, P::Reader, Method::GET,
        "/v1/assemblies/sha256:2b19eec602abd26780ffecf813b3f8a8c9198c7c3fb497fe8ccc99726ed9959a/parts", &[], b""),
    scenario("assemblies_step", "not_modified", S::Registered, P::Reader, Method::GET,
        "/v1/assemblies/sha256:2b19eec602abd26780ffecf813b3f8a8c9198c7c3fb497fe8ccc99726ed9959a/step", SAME_STEP, b""),
    scenario("assemblies_step", "blob_damaged", S::Damaged, P::Reader, Method::GET,
        "/v1/assemblies/sha256:2b19eec602abd26780ffecf813b3f8a8c9198c7c3fb497fe8ccc99726ed9959a/step", &[], b""),
    scenario("assemblies_sockets", "ok", S::Extracted, P::Reader, Method::GET,
        "/v1/assemblies/sha256:2b19eec602abd26780ffecf813b3f8a8c9198c7c3fb497fe8ccc99726ed9959a/sockets", &[], b""),
    scenario("assemblies_delete", "deleted", S::Registered, P::Reader, Method::DELETE,
        "/v1/assemblies/sha256:2b19eec602abd26780ffecf813b3f8a8c9198c7c3fb497fe8ccc99726ed9959a", &[], b""),
    scenario("assemblies_delete", "assembly_in_use", S::Extracted, P::Reader, Method::DELETE,
        "/v1/assemblies/sha256:2b19eec602abd26780ffecf813b3f8a8c9198c7c3fb497fe8ccc99726ed9959a", &[], b""),
    scenario("sockets_create", "succeeded", S::Registered, P::Reader, Method::POST, "/v1/sockets", WAIT,
        br#"{"assembly": "sha256:2b19eec602abd26780ffecf813b3f8a8c9198c7c3fb497fe8ccc99726ed9959a", "part": "BLOCK"}"#),
    scenario("sockets_create", "unchanged", S::Extracted, P::Reader, Method::POST, "/v1/sockets", WAIT,
        br#"{"assembly": "sha256:2b19eec602abd26780ffecf813b3f8a8c9198c7c3fb497fe8ccc99726ed9959a", "part": "BLOCK"}"#),
    scenario("sockets_create", "part_not_found", S::Registered, P::Reader, Method::POST, "/v1/sockets", WAIT,
        br#"{"assembly": "sha256:2b19eec602abd26780ffecf813b3f8a8c9198c7c3fb497fe8ccc99726ed9959a", "part": "block"}"#),
    scenario("sockets_create", "not_found", S::Assembly, P::Reader, Method::POST, "/v1/sockets", WAIT,
        br#"{"assembly": "sha256:2b19eec602abd26780ffecf813b3f8a8c9198c7c3fb497fe8ccc99726ed9959a", "part": "BLOCK"}"#),
    scenario("sockets_list", "ok", S::Extracted, P::Reader, Method::GET, "/v1/sockets", &[], b""),
    scenario("sockets_get", "ok", S::Extracted, P::Reader, Method::GET, "/v1/sockets/sk_4c08cde1bddb3df26f3b@r1", &[], b""),
    scenario("sockets_get", "invalid_reference", S::Extracted, P::Reader, Method::GET,
        "/v1/sockets/sk_4c08cde1bddb3df26f3b@r01", &[], b""),
    scenario("sockets_revisions", "ok", S::Extracted, P::Reader, Method::GET,
        "/v1/sockets/sk_4c08cde1bddb3df26f3b/revisions", &[], b""),
    scenario("sockets_delete", "deleted", S::Extracted, P::Reader, Method::DELETE,
        "/v1/sockets/sk_4c08cde1bddb3df26f3b", &[], b""),
    scenario("jobs_list", "ok", S::Ran, P::Reader, Method::GET, "/v1/jobs", &[], b""),
    scenario("jobs_get", "ok", S::Ran, P::Reader, Method::GET, "/v1/jobs/{job}", &[], b""),
    scenario("jobs_get", "not_modified", S::Ran, P::Reader, Method::GET, "/v1/jobs/{job}",
        SAME_JOB, b""),
    scenario("jobs_get", "not_found", S::None, P::Reader, Method::GET, "/v1/jobs/{job}",
        &[], b""),
    scenario("jobs_events", "ok", S::Ran, P::Reader, Method::GET, "/v1/jobs/{job}/events",
        &[], b""),
    scenario("jobs_events", "too_many_streams", S::Streaming, P::Reader, Method::GET,
        "/v1/jobs/{job}/events", &[], b""),
    scenario("jobs_document", "ok", S::Ran, P::Reader, Method::GET,
        "/v1/jobs/{job}/document", &[], b""),
    scenario("jobs_document", "no_document", S::Queued, P::Reader, Method::GET,
        "/v1/jobs/{job}/document", &[], b""),
    scenario("jobs_cancel", "cancelled", S::Queued, P::Reader, Method::POST,
        "/v1/jobs/{job}/cancel", &[], b""),
    scenario("jobs_cancel", "job_finished", S::Ran, P::Reader, Method::POST,
        "/v1/jobs/{job}/cancel", &[], b""),
    scenario("jobs_pin", "ok", S::Ran, P::Admin, Method::POST, "/v1/jobs/{job}/pin", JSON,
        br#"{"pinned": true}"#),
    scenario("jobs_delete", "deleted", S::Ran, P::Admin, Method::DELETE,
        "/v1/jobs/{job}", &[], b""),
    scenario("jobs_delete", "job_not_finished", S::Queued, P::Admin, Method::DELETE,
        "/v1/jobs/{job}", &[], b""),
    scenario("sockets_create", "carry_from_foreign", S::Foreign, P::Reader, Method::POST, "/v1/sockets", WAIT,
        br#"{"assembly": "sha256:2b19eec602abd26780ffecf813b3f8a8c9198c7c3fb497fe8ccc99726ed9959a", "part": "BLOCK",
             "carry_from": {"upload": "sha256:fb29f29e08eebf6e6da8e3b9beddcdd94d9fbd82879c7f6b5b3ddb0871d75872"}}"#),
    scenario("sockets_review", "created", S::Extracted, P::Reader, Method::POST,
        "/v1/sockets/sk_4c08cde1bddb3df26f3b@r1/review", JSON,
        br#"{"decisions": [{"requirement": "bore:hole-427", "status": "kept"}]}"#),
    scenario("sockets_review", "revision_required", S::Extracted, P::Reader, Method::POST,
        "/v1/sockets/sk_4c08cde1bddb3df26f3b/review", JSON,
        br#"{"decisions": [{"requirement": "bore:hole-427", "status": "kept"}]}"#),
    scenario("sockets_review", "revision_not_current", S::Reviewed, P::Reader, Method::POST,
        "/v1/sockets/sk_4c08cde1bddb3df26f3b@r1/review", JSON,
        br#"{"decisions": [{"requirement": "bore:hole-427", "status": "unreviewed"}]}"#),
    scenario("sockets_review", "review_unknown_requirement", S::Extracted, P::Reader, Method::POST,
        "/v1/sockets/sk_4c08cde1bddb3df26f3b@r1/review", JSON,
        br#"{"decisions": [{"requirement": "bore:hole-1", "status": "kept"}]}"#),
    scenario("sockets_review", "review_unknown_socket", S::Extracted, P::Reader, Method::POST,
        "/v1/sockets/sk_4c08cde1bddb3df26f3b@r1/review", JSON,
        br#"{"decisions": [{"requirement": "bore:hole-427", "socket": "socket-9", "status": "kept"}]}"#),
    scenario("sockets_review", "review_note_required", S::Extracted, P::Reader, Method::POST,
        "/v1/sockets/sk_4c08cde1bddb3df26f3b@r1/review", JSON,
        br#"{"decisions": [{"requirement": "bore:hole-427", "status": "removed"}]}"#),
    scenario("sockets_review", "review_note_too_long", S::Extracted, P::Reader, Method::POST,
        "/v1/sockets/sk_4c08cde1bddb3df26f3b@r1/review", JSON,
        br#"{"decisions": [{"requirement": "bore:hole-427", "status": "removed", "note": "{long}"}]}"#),
    scenario("sockets_review", "review_differs_between_sockets", S::Dowels, P::Reader, Method::POST,
        "/v1/sockets/sk_27de35c9446dbfcdf440@r1/review", JSON,
        br#"{"decisions": [{"requirement": "shaft:shaft-729", "status": "removed", "note": "a press fit"}]}"#),
    scenario("sockets_review", "review_refused_document", S::RefusedRevision, P::Reader, Method::POST,
        "/v1/sockets/sk_4c08cde1bddb3df26f3b@r2/review", JSON,
        br#"{"decisions": [{"requirement": "bore:hole-427", "status": "kept"}]}"#),
    scenario("sockets_review", "socket_stale", S::Shifted, P::Reader, Method::POST,
        "/v1/sockets/sk_4c08cde1bddb3df26f3b@r1/review", JSON,
        br#"{"decisions": [{"requirement": "bore:hole-427", "status": "kept"}]}"#),
    scenario("sockets_import", "succeeded", S::Document, P::Reader, Method::POST, "/v1/sockets/import", WAIT,
        br#"{"upload": "sha256:3af767af3c3bcf21aee48b878302fb860eb3eca3b00bc731c50146617496f56d"}"#),
    scenario("sockets_import", "socket_stale", S::StaleDocument, P::Reader, Method::POST, "/v1/sockets/import",
        WAIT, br#"{"upload": "sha256:{seeded}"}"#),
    scenario("sockets_import", "socket_unreadable", S::BrokenDocument, P::Reader, Method::POST,
        "/v1/sockets/import", WAIT, br#"{"upload": "sha256:{seeded}"}"#),
    scenario("sockets_import", "socket_refused", S::RefusedDocument, P::Reader, Method::POST,
        "/v1/sockets/import", WAIT, br#"{"upload": "sha256:{seeded}"}"#),
    scenario("matches_create", "succeeded", S::Candidates, P::Reader, Method::POST, "/v1/matches", WAIT,
        MATCH_BODY),
    scenario("matches_create", "socket_unreviewed", S::Candidates, P::Reader, Method::POST, "/v1/matches", WAIT,
        br#"{"socket": "sk_4c08cde1bddb3df26f3b", "options": {"require_reviewed": true}, "candidates": {"files": [
            {"upload": "sha256:1f021de7913c486df1f45d44bc6306c39f1b7374d9eccb818e27fbc6990db4ea", "label": "m_block02.stp"}]}}"#),
    scenario("matches_create", "label_reserved", S::Candidates, P::Reader, Method::POST, "/v1/matches", WAIT,
        br#"{"socket": "sk_4c08cde1bddb3df26f3b@r1", "candidates": {"files": [
            {"upload": "sha256:1f021de7913c486df1f45d44bc6306c39f1b7374d9eccb818e27fbc6990db4ea", "label": "catalog:BLOCK@r1"}]}}"#),
    scenario("matches_create", "no_candidates", S::Extracted, P::Reader, Method::POST, "/v1/matches", WAIT,
        br#"{"socket": "sk_4c08cde1bddb3df26f3b@r1", "candidates": {"files": []}}"#),
    scenario("matches_create", "upload_not_found", S::Extracted, P::Reader, Method::POST, "/v1/matches", WAIT,
        MATCH_BODY),
    scenario("matches_create", "original_not_stored", S::Document, P::Reader, Method::POST, "/v1/matches", WAIT,
        br#"{"socket": {"upload": "sha256:3af767af3c3bcf21aee48b878302fb860eb3eca3b00bc731c50146617496f56d"},
             "candidates": {"files": [{"upload": "sha256:3af767af3c3bcf21aee48b878302fb860eb3eca3b00bc731c50146617496f56d",
                                       "label": "the socket file.stp"}]}, "original": true}"#),
    scenario("matches_list", "ok", S::Matched, P::Reader, Method::GET, "/v1/matches", &[], b""),
    scenario("matches_get", "ok", S::Matched, P::Reader, Method::GET, "/v1/matches/{job}", &[], b""),
    scenario("matches_get", "not_found", S::None, P::Reader, Method::GET,
        "/v1/matches/j_7zzzzzzzzzzzzzzzzzzzzzzzzz", &[], b""),
    scenario("matches_document", "ok", S::Matched, P::Reader, Method::GET, "/v1/matches/{job}/document",
        &[], b""),
    scenario("matches_candidates", "ok", S::Matched, P::Reader, Method::GET,
        "/v1/matches/{job}/candidates?limit=1", &[], b""),
];

/// The codes whose routes come in a later PR of section 12, with that PR:
/// each must be produced by a golden by then. `catalog_busy` and
/// `catalog_damaged` are reachable by `/v1/info` today, but only through a
/// fault no golden stages without the seams of PR 7 (a busy holder, a
/// damaged row). Section 7.12's job-only codes (HTTP "—")
/// are not listed: no request answers one; the job goldens show them.
const PENDING: [(&str, u32); 11] = [
    ("not_an_id", 7),
    ("options_conflict", 7),
    ("entry_not_found", 7),
    ("reference_ambiguous", 7),
    ("entry_deleted", 7),
    ("bytes_not_kept", 7),
    ("derived_rows_gone", 7),
    ("precondition_failed", 7),
    ("entry_stale", 7),
    ("catalog_busy", 7),
    ("catalog_damaged", 7),
];

/// The PR this tree builds (section 12): a pending code waits for a later
/// one, at most the exit.
const PR_NOW: u32 = 6;

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

/// Runs `scenario` on a fresh in-process server: its reply, and the path
/// and body sent, the setup's names filled in.
async fn run(scenario: &Scenario) -> (Reply, String, Vec<u8>) {
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
        S::OwnerFull => config.jobs.per_owner_queued = 1,
        S::LaneFull => config.jobs.queue_query = 1,
        S::Streaming => config.jobs.streams_per_user = 1,
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
            server.server().hold_jobs();
            let sha256 = fitment_core::sha256_hex(BRACKET);
            support::seed_job_input(&server, ada.key, &sha256, "match", "queued");
        }
    }
    let (mut job, _kept) = jobs_setup(&server, scenario.setup, token.as_deref()).await;
    let seeded = match_setup(&server, scenario.setup, token.as_deref(), &mut job).await;
    if scenario.setup == S::LoginOnce {
        let login = br#"{"username": "ada", "password": "correct horse battery staple"}"#;
        let first = Call::new(Method::POST, "/v1/auth/login")
            .header("content-type", "application/json")
            .body(login)
            .anonymous();
        assert_eq!(server.send(first).await.status, 201);
    }
    let path = scenario.path.replace("{job}", &job);
    let body = String::from_utf8_lossy(scenario.body)
        .replace("{seeded}", &seeded)
        .replace("{long}", &"a long note ".repeat(200))
        .into_bytes();
    let mut call = Call::new(scenario.method.clone(), &path)
        .body(&body)
        .anonymous();
    for (name, value) in scenario.headers {
        call = call.header(name, value);
    }
    if let Some(token) = token {
        call = call.bearer(&token);
    }
    (server.send(call).await, path, body)
}

/// The CLI's socket golden of `name`, edited by `edit`, as pretty JSON.
fn edited_socket(name: &str, edit: impl FnOnce(&mut serde_json::Value)) -> Vec<u8> {
    let mut doc: serde_json::Value =
        serde_json::from_slice(&support::cli_golden("socket", name)).unwrap();
    edit(&mut doc);
    (serde_json::to_string_pretty(&doc).unwrap() + "\n").into_bytes()
}

/// The socket file a setup's upload holds: the CLI's golden, edited, or a
/// refused socket document.
fn socket_file(setup: Setup) -> Vec<u8> {
    match setup {
        S::StaleDocument => edited_socket("s02_cbore--BLOCK.json", |doc| {
            doc["socket_rules_version"] = (fitment_core::SOCKET_RULES_VERSION - 1).into();
        }),
        S::BrokenDocument => edited_socket("s02_cbore--BLOCK.json", |doc| {
            doc["extracted"]["sockets"][0]["requirements"][0]["floor_mm"] = 0.5.into();
        }),
        S::RefusedDocument => {
            fitment_core::SocketDocument::extract(b"no STEP file", Some("X"), None)
                .unwrap()
                .to_json()
                .into_bytes()
        }
        S::Foreign => support::cli_golden("socket", "s01_bolted--BRACKET.json"),
        _ => support::cli_golden("socket", "s02_cbore--BLOCK.json"),
    }
}

/// PR 6's setups, as `ada`: the socket documents, reviews and matches the
/// scenarios name; a seeded upload's SHA-256 for `{seeded}`, and `job` set
/// to the match a `{job}` names.
async fn match_setup(
    server: &TestServer,
    setup: Setup,
    token: Option<&str>,
    job: &mut String,
) -> String {
    use S::{
        BrokenDocument, Candidates, Document, Dowels, Foreign, Matched, RefusedDocument,
        RefusedRevision, Reviewed, Shifted, StaleDocument,
    };
    if (setup as u8) < (Reviewed as u8) {
        return String::new();
    }
    let seams = server.server().seams();
    let ada_token = match seams.db.user_by_fold("ada").unwrap() {
        Some(_) => token.unwrap().to_owned(),
        None => seed(server.server(), "ada", Role::Reader, Some(ADA), true).unwrap(),
    };
    let ada = seams.db.user_by_fold("ada").unwrap().unwrap();
    let as_ada = |call: Call| call.bearer(&ada_token);
    let seed_bytes = |bytes: &[u8]| support::seed_upload(server.server(), ada.key, bytes);
    match setup {
        Reviewed | RefusedRevision | Shifted | Candidates | Matched => {
            assembly_setup(server, S::Extracted, &ada_token, ada.key).await;
        }
        Foreign => assembly_setup(server, S::Registered, &ada_token, ada.key).await,
        _ => {}
    }
    match setup {
        Reviewed => {
            let body = br#"{"decisions": [{"requirement": "bore:hole-427", "status": "kept"}]}"#;
            let call = Call::new(
                Method::POST,
                "/v1/sockets/sk_4c08cde1bddb3df26f3b@r1/review",
            )
            .header("content-type", "application/json")
            .body(body);
            assert_eq!(server.send(as_ada(call)).await.status, 201);
            String::new()
        }
        Dowels => {
            let bytes = support::assembly("s03_dowels.stp");
            let sha = seed_bytes(&bytes);
            for (path, body) in [
                (
                    "/v1/assemblies",
                    serde_json::json!({ "upload": format!("sha256:{sha}") }),
                ),
                (
                    "/v1/sockets",
                    serde_json::json!({ "assembly": format!("sha256:{sha}"), "part": "DOWEL" }),
                ),
            ] {
                let reply = server.send(as_ada(support::waiting(path, &body))).await;
                assert_eq!(reply.json()["state"], "succeeded");
            }
            String::new()
        }
        RefusedRevision => {
            let sha = seed_bytes(&socket_file(RefusedDocument));
            let db = rusqlite::Connection::open(server.dirs.state.join("server.db")).unwrap();
            db.execute(
                "INSERT INTO socket_revision (line, owner, line_id, n, how, verified, parent,
                    document_sha256, document_digest, extractor_version, socket_rules_version,
                    unreviewed, carried, stale, job, created_ms)
                 SELECT line, owner, line_id, 2, 'extract', 1, key, ?1, '', extractor_version,
                    socket_rules_version, 0, 0, 0, NULL, created_ms FROM socket_revision WHERE n = 1",
                [sha],
            )
            .unwrap();
            String::new()
        }
        Shifted => {
            server.server().set_rules_shift(1);
            String::new()
        }
        Document | StaleDocument | BrokenDocument | RefusedDocument | Foreign => {
            seed_bytes(&socket_file(setup))
        }
        Candidates | Matched => {
            for name in ["m_block02.stp", "m_block02_cb_deep.stp"] {
                seed_bytes(&support::candidate(name));
            }
            if setup == Matched {
                let call = Call::new(Method::POST, "/v1/matches")
                    .header("content-type", "application/json")
                    .header("prefer", "wait=30")
                    .body(MATCH_BODY);
                let reply = server.send(as_ada(call)).await;
                reply.json()["id"].as_str().unwrap().clone_into(job);
            }
            String::new()
        }
        _ => String::new(),
    }
}

/// The bytes of the unit part the job scenarios read.
fn p01() -> Vec<u8> {
    support::part("p01_through.stp")
}

/// The assembly setups of [`jobs_setup`]: `s02_cbore.stp` uploaded, and
/// registered, staled, damaged or extracted as `setup` says.
async fn assembly_setup(server: &TestServer, setup: Setup, ada_token: &str, ada: i64) {
    use S::{Assembly, Damaged, Extracted, Stale};
    let as_ada = |call: Call| call.bearer(ada_token);
    let bytes = support::assembly("s02_cbore.stp");
    let sha = support::seed_upload(server.server(), ada, &bytes);
    if setup != Assembly {
        let body =
            serde_json::json!({ "upload": format!("sha256:{sha}"), "file_name": "s02_cbore.stp" });
        let reply = server
            .send(as_ada(support::waiting("/v1/assemblies", &body)))
            .await;
        assert_eq!(reply.json()["state"], "succeeded");
    }
    if setup == Extracted {
        let body = serde_json::json!({ "assembly": format!("sha256:{sha}"), "part": "BLOCK" });
        let reply = server
            .send(as_ada(support::waiting("/v1/sockets", &body)))
            .await;
        assert_eq!(reply.json()["state"], "succeeded");
    }
    let db = rusqlite::Connection::open(server.dirs.state.join("server.db")).unwrap();
    if setup == Stale {
        db.execute(
            "UPDATE assembly_parts SET extractor_version = extractor_version - 1",
            [],
        )
        .unwrap();
    }
    if setup == Damaged {
        let path = server
            .dirs
            .state
            .join(format!("objects/sha256/{}/{sha}", &sha[..2]));
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        perms.set_readonly(false);
        std::fs::set_permissions(&path, perms).unwrap();
        let mut damaged = bytes.clone();
        damaged[100] ^= 1;
        std::fs::write(&path, damaged).unwrap();
    }
}

/// PR 5's setups, as `ada` (made with a token here unless she presents
/// one): the job the scenario's `{job}` names, and what must stay open.
async fn jobs_setup(
    server: &TestServer,
    setup: Setup,
    token: Option<&str>,
) -> (String, Option<axum::response::Response>) {
    use S::{
        Assembly, Damaged, Extracted, Held, JobPanic, Keyed, LaneFull, OwnerFull, Part, Queued,
        Ran, Registered, Stale, Streaming,
    };
    if !matches!(
        setup,
        Part | Assembly
            | Registered
            | Stale
            | Damaged
            | Extracted
            | Ran
            | Held
            | Queued
            | OwnerFull
            | LaneFull
            | Keyed
            | JobPanic
            | Streaming
    ) {
        return (String::new(), None);
    }
    let seams = server.server().seams();
    let ada_token = match seams.db.user_by_fold("ada").unwrap() {
        Some(_) => token.unwrap().to_owned(),
        None => seed(server.server(), "ada", Role::Reader, Some(ADA), true).unwrap(),
    };
    let ada = seams.db.user_by_fold("ada").unwrap().unwrap();
    let as_ada = |call: Call| call.bearer(&ada_token);
    let mut job = String::new();
    if matches!(setup, Assembly | Registered | Stale | Damaged | Extracted) {
        assembly_setup(server, setup, &ada_token, ada.key).await;
        return (job, None);
    }
    let sha = support::seed_upload(server.server(), ada.key, &p01());
    let body = serde_json::json!({ "upload": format!("sha256:{sha}") });
    if setup == JobPanic {
        seams.hooks.set(
            "job.read",
            Action::Panic("a golden's planted job panic".into()),
        );
    }
    if setup == Ran {
        let reply = server
            .send(as_ada(support::waiting("/v1/features", &body)))
            .await;
        reply.json()["id"].as_str().unwrap().clone_into(&mut job);
    }
    if setup == Keyed {
        let call = support::waiting("/v1/features", &body).header("idempotency-key", "golden-key");
        let reply = server.send(as_ada(call)).await;
        reply.json()["id"].as_str().unwrap().clone_into(&mut job);
    }
    if matches!(setup, Held | Queued | OwnerFull | LaneFull | Streaming) {
        server.server().hold_jobs();
    }
    if matches!(setup, Queued | OwnerFull | LaneFull | Streaming) {
        let call = Call::json(Method::POST, "/v1/features", &body);
        let reply = server.send(as_ada(call)).await;
        assert_eq!(reply.status, 202);
        reply.json()["id"].as_str().unwrap().clone_into(&mut job);
    }
    let kept = if setup == Streaming {
        let call = as_ada(Call::get(&format!("/v1/jobs/{job}/events")));
        Some(server.open(call).await)
    } else {
        None
    };
    (job, kept)
}

fn render_request(scenario: &Scenario, path: &str, body: &[u8]) -> String {
    let mut text = format!("{} {}\n", scenario.method, path);
    match scenario.principal {
        P::Admin => text.push_str("authorization: Bearer <root's admin token>\n"),
        P::Reader => text.push_str("authorization: Bearer <ada's reader token>\n"),
        P::None => {}
    }
    for (name, value) in scenario.headers {
        let _ = writeln!(text, "{name}: {value}");
    }
    text.push('\n');
    text.push_str(&String::from_utf8_lossy(body));
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
    } else if let Some(name) = core_document(&reply.body) {
        // VT6: core documents are not duplicated; the CLI's golden holds
        // these bytes.
        let _ = writeln!(text, "<the bytes of crates/fitment/tests/golden/{name}>");
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

/// The CLI's golden whose bytes `body` is, as `<kind>/<file>` (VT6).
fn core_document(body: &[u8]) -> Option<String> {
    if body.is_empty() {
        return None;
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../fitment/tests/golden");
    for kind in ["features", "parts", "socket", "match"] {
        for entry in std::fs::read_dir(root.join(kind)).ok()? {
            let path = entry.ok()?.path();
            if std::fs::read(&path).ok().as_deref() == Some(body) {
                return Some(format!("{kind}/{}", path.file_name()?.to_string_lossy()));
            }
        }
    }
    None
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
        let (reply, path, body) = run(scenario).await;
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
            &render_request(scenario, &path, &body),
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
        let (reply, ..) = run(scenario).await;
        let path = out
            .join(scenario.operation)
            .join(format!("{}.response", scenario.name));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, render_response(scenario, &reply)).unwrap();
    }
    std::fs::write(out.join("openapi.json"), fitment_server::OPENAPI).unwrap();
}
