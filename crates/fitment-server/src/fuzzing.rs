//! VT26's harness (docs/research/m6-api.md, M6-42, XH14), under
//! `test-support` only: the two fuzz targets of `fuzz/` are thin wrappers
//! over [`Target::route`] and [`body`], and the coverage test runs every
//! request of the seed corpus through the same code on every PR.
//!
//! A request is written as VT6's goldens write one
//! (`tests/golden/api/<operation>/<scenario>.request`): a line `METHOD
//! PATH`, one line per header `name: value`, a blank line, then the body's
//! bytes. So the goldens' requests are the route target's seed corpus
//! (`fuzz.yml` copies them), and a credential a golden writes as `Bearer
//! <…>` is the target's own admin token.
//!
//! The route target's property: no panic; no `5xx` but the declared load
//! refusals ([`LOAD`]); every error a valid `fitment.error` document whose
//! `status` and `code` are the response's. The body target's: each request
//! type's decoding and validation never panic, and an accepted body,
//! written again (compact, pretty, its keys in another order), is accepted
//! as the same value with the same JB9 fingerprint.

use std::net::SocketAddr;
use std::path::Path;

use axum::body::Body;
use axum::http::{HeaderName, HeaderValue, Method, Request, StatusCode};
use http_body_util::BodyExt as _;
use serde::de::DeserializeOwned;
use serde_json::Value;
use tower::ServiceExt as _;

use crate::http::error::ErrorCode;
use crate::{Config, Flags, Server};

/// The `5xx` codes a request may meet under load, which a fuzzed request
/// may meet too: none is a bug.
pub const LOAD: [&str; 5] = [
    "queue_full",
    "request_timeout",
    "catalog_busy",
    "storage_full",
    "quota_exceeded",
];

/// The target's admin.
const ADMIN: &str = "fuzzer";

/// The server a route target sends to: in process, on a current-thread
/// runtime, its ids fixed (OP11) so a crash replays; its clock is the
/// real one, which the offline admin's tokens are minted by.
pub struct Target {
    runtime: tokio::runtime::Runtime,
    router: axum::Router,
    admin: crate::admin::Admin,
    /// The admin's token, minted again when a request revoked it (a
    /// password change, a revocation, a disabling).
    token: std::sync::Mutex<String>,
    _server: Server,
}

impl std::fmt::Debug for Target {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Target").finish_non_exhaustive()
    }
}

impl Target {
    /// A server on the catalog at `catalog`, which the caller made (a small
    /// one), and a state directory made at `state`, with its first admin's
    /// token; uploads capped at 1 MiB and each user's quota at 1 GiB, so a
    /// long run stays small, and AU7's rates out of the way.
    ///
    /// # Panics
    ///
    /// When the runtime, the server or the admin cannot be made: the
    /// harness cannot run.
    #[allow(clippy::expect_used)]
    pub fn new(catalog: &Path, state: &Path) -> Self {
        let flags = Flags {
            catalog: Some(catalog.to_path_buf()),
            state: Some(state.to_path_buf()),
            listen: Some("127.0.0.1:0".into()),
            ..Flags::default()
        };
        let mut config = Config::from_sources(&flags, &std::collections::BTreeMap::new())
            .expect("the fuzz target's configuration");
        config.limits.max_upload_mib = 1;
        config.limits.user_quota_gib = 1;
        // AU7's buckets would answer most of a fast run `429`.
        config.auth.rate_per_second = 1_000_000;
        config.auth.rate_burst = 1_000_000;
        config.auth.login_per_minute = 1_000_000;
        config.auth.login_failures = 1_000_000;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a runtime");
        let server = runtime.block_on(async {
            let server = Server::build(config.clone()).expect("the fuzz target's server");
            server.seams().ids.fix_ids();
            server
        });
        let admin = crate::admin::Admin::open(&config).expect("the state, offline");
        let token = admin
            .bootstrap(ADMIN, None)
            .expect("the first admin")
            .secret
            .expose()
            .to_owned();
        let router = server.router();
        Self {
            runtime,
            router,
            admin,
            token: std::sync::Mutex::new(token),
            _server: server,
        }
    }

    fn token(&self) -> std::sync::MutexGuard<'_, String> {
        self.token
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The admin restored (an admin, enabled) and a token minted for it,
    /// its old ones revoked when the live-token cap is met.
    #[allow(clippy::expect_used)]
    fn renew(&self) {
        let _ = self
            .admin
            .set_user(ADMIN, Some(crate::auth::Role::Admin), Some(false));
        let minted = self.admin.mint(ADMIN, "fuzz", None, None).or_else(|_| {
            for token in self.admin.tokens(ADMIN).unwrap_or_default() {
                let _ = self.admin.revoke(&token.id);
            }
            self.admin.mint(ADMIN, "fuzz", None, None)
        });
        minted
            .expect("a token")
            .secret
            .expose()
            .clone_into(&mut self.token());
    }

    /// Sends the request `input` writes, if it writes one, and checks the
    /// answer: its status, or `None` when `input` is no request.
    ///
    /// # Panics
    ///
    /// When the answer breaks the route target's property: that is a
    /// finding.
    #[allow(clippy::panic)]
    pub fn route(&self, input: &[u8]) -> Option<StatusCode> {
        let token = self.token().clone();
        let (request, substituted) = decode(input, &token)?;
        let method = request.method().clone();
        let (status, content_type, body) = self.runtime.block_on(async {
            let response = self
                .router
                .clone()
                .oneshot(request)
                .await
                .unwrap_or_else(|never| match never {});
            let status = response.status();
            let content_type = response
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned);
            let body = response
                .into_body()
                .collect()
                .await
                .map(|b| b.to_bytes().to_vec())
                .unwrap_or_default();
            (status, content_type, body)
        });
        if let Err(problem) = check(&method, status, content_type.as_deref(), &body) {
            panic!("{method} answered {status}: {problem}");
        }
        if substituted && status == StatusCode::UNAUTHORIZED {
            self.renew();
        }
        Some(status)
    }
}

/// The request `input` writes (the module's format), its `Bearer <…>`
/// credential replaced by `token`, from a loopback peer; `None` when
/// `input` writes none (no line, a method or path that does not parse).
pub fn request(input: &[u8], token: &str) -> Option<Request<Body>> {
    decode(input, token).map(|(request, _)| request)
}

/// [`request`], and whether its credential was the target's.
fn decode(input: &[u8], token: &str) -> Option<(Request<Body>, bool)> {
    let (head, body) = split_head(input);
    let head = std::str::from_utf8(head).ok()?;
    let mut lines = head.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l));
    let (method, target) = lines.next()?.split_once(' ')?;
    let method = Method::from_bytes(method.as_bytes()).ok()?;
    let mut builder = Request::builder().method(method).uri(target);
    let mut substituted = false;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim_start();
        let value = if name.eq_ignore_ascii_case("authorization") && value.starts_with("Bearer <") {
            substituted = true;
            format!("Bearer {token}")
        } else {
            value.to_owned()
        };
        let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(name.as_bytes()),
            HeaderValue::from_str(&value),
        ) else {
            continue;
        };
        builder = builder.header(name, value);
    }
    let mut request = builder.body(Body::from(body.to_vec())).ok()?;
    request
        .extensions_mut()
        .insert(axum::extract::ConnectInfo(SocketAddr::from((
            [127, 0, 0, 1],
            40_000,
        ))));
    Some((request, substituted))
}

/// The head (before the first blank line) and the body.
fn split_head(input: &[u8]) -> (&[u8], &[u8]) {
    let lf = find(input, b"\n\n").map(|at| (at, 2));
    let crlf = find(input, b"\r\n\r\n").map(|at| (at, 4));
    match lf.into_iter().chain(crlf).min_by_key(|(at, _)| *at) {
        Some((at, n)) => (&input[..at], &input[at + n..]),
        None => (input, &[]),
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// The route target's property for one answer (VT26): no `5xx` but
/// [`LOAD`]'s; an error is a `fitment.error` document whose `status` and
/// `code` are the response's, its code one of `ErrorCode::ALL` answering
/// that status. A `HEAD`'s answer has no body to read.
///
/// # Errors
///
/// What is wrong, for the panic's message.
pub fn check(
    method: &Method,
    status: StatusCode,
    content_type: Option<&str>,
    body: &[u8],
) -> Result<(), String> {
    let error = content_type == Some(crate::http::json::PROBLEM_JSON);
    if !status.is_client_error() && !status.is_server_error() {
        return Ok(());
    }
    if *method == Method::HEAD {
        return if status.is_server_error() && status != StatusCode::SERVICE_UNAVAILABLE {
            Err("a HEAD failed with a server error".into())
        } else {
            Ok(())
        };
    }
    // hyper's own refusals never reach a router in process: every error
    // here is the server's, so a document.
    if !error {
        return Err(format!("an error answered as {content_type:?}"));
    }
    let document: Value =
        serde_json::from_slice(body).map_err(|_| "an error body that is not JSON".to_owned())?;
    let code = document["code"].as_str().unwrap_or_default();
    let known = ErrorCode::ALL
        .iter()
        .find(|c| c.name() == code)
        .ok_or_else(|| format!("an unknown code {code:?}"))?;
    if document["format"] != "fitment.error"
        || document["format_version"] != 1
        || document["status"] != status.as_u16()
        || known.status() != Some(status)
        || !document["title"].is_string()
        || !document["detail"].is_string()
    {
        return Err(format!("an error document out of its contract: {document}"));
    }
    if status.is_server_error() && !LOAD.contains(&code) {
        return Err(format!("a server error: {code}"));
    }
    Ok(())
}

/// The body target (VT26): `input` as every request type's body, each
/// decoding checked as the module says, and as each reference and name
/// the requests validate.
///
/// # Panics
///
/// When a property fails: that is a finding.
pub fn body(input: &[u8]) {
    use crate::auth::requests as auth;
    use crate::jobs::request as jobs;
    same::<auth::LoginRequest>(input, false);
    same::<auth::PasswordRequest>(input, false);
    same::<auth::MintRequest>(input, true);
    same::<auth::CreateUserRequest>(input, false);
    same::<auth::UpdateUserRequest>(input, true);
    same::<jobs::FeaturesRequest>(input, true);
    same::<jobs::AssemblyRequest>(input, true);
    same::<jobs::SocketRequest>(input, true);
    same::<jobs::PinRequest>(input, true);
    same::<jobs::ReviewRequest>(input, true);
    same::<jobs::ImportRequest>(input, true);
    same::<jobs::MatchRequest>(input, true);
    same::<jobs::IngestRequest>(input, true);
    same::<jobs::ReplaceRequest>(input, true);
    same::<jobs::ReindexRequest>(input, true);
    same::<jobs::VerifyRequest>(input, true);
    same::<jobs::GcRequest>(input, true);
    same::<jobs::PruneRequest>(input, true);
    same::<jobs::DeleteRequest>(input, true);
    if let Ok(text) = std::str::from_utf8(input) {
        names(text);
    }
}

/// One request type: decoding never panics; an accepted body written
/// again is accepted, the same value (by its `Debug`, where it has one:
/// the password requests have none, so no password is printed) with the
/// same fingerprint.
#[allow(clippy::panic)]
fn same<T: DeserializeOwned + 'static>(input: &[u8], debug: bool) {
    let Ok(first) = crate::http::json::decode::<T>(input) else {
        return;
    };
    let Ok(value) = serde_json::from_slice::<Value>(input) else {
        panic!("{}: accepted, yet not JSON", std::any::type_name::<T>());
    };
    let fingerprint = crate::jobs::request::fingerprint("POST", "/v1/x", input);
    let compact = value.to_string();
    let pretty = serde_json::to_string_pretty(&value).unwrap_or_default();
    for again in [compact, pretty] {
        let Ok(second) = crate::http::json::decode::<T>(again.as_bytes()) else {
            panic!(
                "{}: accepted, then refused written again",
                std::any::type_name::<T>()
            );
        };
        assert!(
            !debug || described(&first) == described(&second),
            "{}: another value written again",
            std::any::type_name::<T>()
        );
        assert_eq!(
            crate::jobs::request::fingerprint("POST", "/v1/x", again.as_bytes()),
            fingerprint,
            "JB9: a body written again has another fingerprint"
        );
    }
}

/// `value`'s `Debug`, for the types that have one.
fn described<T: 'static>(value: &T) -> String {
    use crate::auth::requests as auth;
    use crate::jobs::request as jobs;
    let any: &dyn std::any::Any = value;
    macro_rules! of {
        ($($t:ty),*) => {$(
            if let Some(v) = any.downcast_ref::<$t>() {
                return format!("{v:?}");
            }
        )*};
    }
    of!(
        auth::MintRequest,
        auth::UpdateUserRequest,
        jobs::FeaturesRequest,
        jobs::AssemblyRequest,
        jobs::SocketRequest,
        jobs::PinRequest,
        jobs::ReviewRequest,
        jobs::ImportRequest,
        jobs::MatchRequest,
        jobs::IngestRequest,
        jobs::ReplaceRequest,
        jobs::ReindexRequest,
        jobs::VerifyRequest,
        jobs::GcRequest,
        jobs::PruneRequest,
        jobs::DeleteRequest
    );
    String::new()
}

/// The references and names the requests validate (HX5, RA5, RA8): none
/// panics, and what each accepts reads back as itself.
#[allow(clippy::panic)]
fn names(text: &str) {
    use crate::jobs::request as jobs;
    if let Ok(sha) = jobs::sha256_ref(text, "upload") {
        let again = jobs::sha256_ref(&format!("sha256:{sha}"), "upload").ok();
        assert_eq!(
            again.as_deref(),
            Some(sha.as_str()),
            "HX5: a SHA-256 reference does not read back"
        );
    }
    if let Ok(reference) = jobs::SocketRef::parse(text) {
        assert_eq!(
            jobs::SocketRef::parse(&reference.text()).ok(),
            Some(reference),
            "RA5: a socket reference does not read back"
        );
    }
    let _ = jobs::text(text, "label");
    assert!(
        jobs::file_name(text).is_err() || jobs::text(text, "file_name").is_ok(),
        "RA8: a file name that is not text"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn problem(status: u16, code: &str) -> Vec<u8> {
        serde_json::json!({
            "format": "fitment.error", "format_version": 1, "title": "t", "detail": "d",
            "status": status, "code": code,
        })
        .to_string()
        .into_bytes()
    }

    /// VT26's property and its controls: a load refusal and a usage error
    /// pass; an `internal`, an error that is no document, a document whose
    /// status or code is not the response's, each fail.
    #[test]
    fn the_route_property_has_its_controls() {
        let get = Method::GET;
        let json = Some(crate::http::json::PROBLEM_JSON);
        let s = |n| StatusCode::from_u16(n).unwrap();
        assert!(check(&get, s(503), json, &problem(503, "queue_full")).is_ok());
        assert!(check(&get, s(404), json, &problem(404, "not_found")).is_ok());
        assert!(check(&get, s(200), Some("application/json"), b"{}").is_ok());
        assert!(check(&get, s(500), json, &problem(500, "internal")).is_err());
        assert!(check(&get, s(404), Some("text/plain"), b"nope").is_err());
        assert!(check(&get, s(404), json, &problem(400, "not_found")).is_err());
        assert!(check(&get, s(404), json, &problem(404, "bad_request")).is_err());
        assert!(check(&get, s(404), json, &problem(404, "no_such_code")).is_err());
    }

    /// The seed format: a golden's request, its credential the target's.
    #[test]
    fn a_golden_request_reads_as_its_request() {
        let seed = b"PUT /v1/uploads/sha256:00?x=1\nauthorization: Bearer <ada's token>\ncontent-type: model/step\n\nbytes\n\nmore";
        let request = request(seed, "the-token").unwrap();
        assert_eq!(request.method(), Method::PUT);
        assert_eq!(request.uri(), "/v1/uploads/sha256:00?x=1");
        assert_eq!(request.headers()["authorization"], "Bearer the-token");
        assert_eq!(request.headers()["content-type"], "model/step");
        assert!(
            request
                .extensions()
                .get::<axum::extract::ConnectInfo<SocketAddr>>()
                .is_some()
        );
        assert!(super::request(b"", "t").is_none());
        assert!(super::request(b"GET\n\n", "t").is_none());
        assert_eq!(split_head(b"GET /\n\nbytes\n\nmore").1, b"bytes\n\nmore");
    }

    /// The body target on a few bodies, each request type's decoding run.
    #[test]
    fn the_body_property_holds_on_known_bodies() {
        for input in [
            &br#"{"upload": "sha256:62048ce1d1d48e0786d0a9e001ab0c72354bb9faa408a4984f9ceb2b9f73ccff"}"#[..],
            br#"{"socket": "sk_4c08cde1bddb3df26f3b@r1", "candidates": {"catalog": {"selection": "full"}}}"#,
            br#"{"name": "minted", "days": 3}"#,
            b"sk_4c08cde1bddb3df26f3b@r12",
            b"\xef\xbb\xbf{}",
            b"[[[[[[[[[[",
        ] {
            body(input);
        }
    }
}
