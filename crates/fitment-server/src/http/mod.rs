//! The router and its layers (docs/research/m6-api.md, section 4.12).
//!
//! From the outside in: sensitive headers marked (OP1); the request id,
//! the error body and the response headers (HX7, ER, HX6); the trace span
//! (OP1); CORS, only with an allowlist (HX4); panics caught (JS6); then,
//! per route, the handler timeout (HX3: an upload's is the whole upload's,
//! `limits.upload_minutes`), the body limit (HX2, UP2) and `Accept` (VN8),
//! so a usage error is answered before anything is read (ER5). The routes
//! come from `OPERATIONS` alone (XH14).

pub(crate) mod cache;
pub mod error;
pub mod extract;
pub mod json;
pub mod operations;
pub mod query;
mod routes;

use std::collections::BTreeMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::{Body, Bytes};
use axum::error_handling::HandleErrorLayer;
use axum::extract::{ConnectInfo, DefaultBodyLimit, MatchedPath, Request, State};
use axum::http::{HeaderName, HeaderValue, Method, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{MethodFilter, MethodRouter};
use tower::ServiceBuilder;
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::sensitive_headers::SetSensitiveRequestHeadersLayer;
use tower_http::trace::TraceLayer;

use crate::Shared;
use error::{ApiError, ErrorCode};
use operations::{OPERATIONS, Operation, RequestBody};

/// `X-Request-Id`.
pub const REQUEST_ID: HeaderName = HeaderName::from_static("x-request-id");

/// The request's id (HX7), in its extensions and on its response.
#[derive(Debug, Clone)]
pub struct RequestId(pub String);

/// The router: every operation of `OPERATIONS`, the fallbacks, the layers.
pub(crate) fn router(shared: Arc<Shared>) -> Router {
    let mut paths: BTreeMap<&str, MethodRouter<Arc<Shared>>> = BTreeMap::new();
    for op in &OPERATIONS {
        let route = per_route(&shared, op, routes::handler(op));
        let merged = match paths.remove(op.path) {
            Some(existing) => existing.merge(route),
            None => route,
        };
        paths.insert(op.path, merged);
    }
    let mut router = Router::new();
    for (path, route) in paths {
        router = router.route(path, route);
    }
    let router = router
        .fallback(not_found)
        .method_not_allowed_fallback(method_not_allowed)
        .layer(DefaultBodyLimit::disable())
        .layer(CatchPanicLayer::custom(panicked));
    let router = match cors(&shared.config.server.cors_origins) {
        Some(layer) => router.layer(layer),
        None => router,
    };
    // OP1's request span: the request id, the method, the matched route
    // template (never the raw URI or query), the user's id once
    // authenticated (never the username), then the status and latency.
    let trace = TraceLayer::new_for_http()
        .make_span_with(|request: &Request<Body>| {
            let id = request
                .extensions()
                .get::<RequestId>()
                .map_or("", |r| r.0.as_str());
            let route = request
                .extensions()
                .get::<MatchedPath>()
                .map_or("(none)", MatchedPath::as_str);
            tracing::info_span!(
                "request",
                request_id = id,
                method = %request.method(),
                route,
                user = tracing::field::Empty,
                job = tracing::field::Empty
            )
        })
        .on_response(
            |response: &Response<Body>, latency: Duration, _: &tracing::Span| {
                tracing::info!(
                    status = response.status().as_u16(),
                    latency_ms = u64::try_from(latency.as_millis()).unwrap_or(u64::MAX),
                    "answered"
                );
            },
        );
    router
        .layer(trace)
        .layer(middleware::from_fn_with_state(Arc::clone(&shared), finish))
        .layer(SetSensitiveRequestHeadersLayer::new([
            header::AUTHORIZATION,
            header::COOKIE,
            header::PROXY_AUTHORIZATION,
        ]))
        .with_state(shared)
}

/// A route's own layers, from its operation, outermost first: no body
/// where it takes none (HX2), `Accept` (VN8), then its role (AU5: the
/// authentication every route but the four open ones and login carries,
/// XH14), so usage is answered before credentials are read (ER5); a JSON
/// body is read by the handler, after them, under its cap (HX2).
fn per_route(
    shared: &Arc<Shared>,
    op: &'static Operation,
    route: MethodRouter<Arc<Shared>>,
) -> MethodRouter<Arc<Shared>> {
    let route = route.layer(middleware::from_fn_with_state(
        (Arc::clone(shared), op.role),
        extract::authenticate,
    ));
    let allowed = op.query;
    let route = route.layer(middleware::from_fn(move |request: Request, next: Next| {
        known_query(allowed, request, next)
    }));
    let media = op.answers.iter().find_map(|a| a.media);
    let route = match media {
        Some(media) => route.layer(middleware::from_fn(move |request: Request, next: Next| {
            accept(media, request, next)
        })),
        None => route,
    };
    let route: MethodRouter<Arc<Shared>> = match op.body {
        RequestBody::None => route.layer(middleware::from_fn(no_body)),
        RequestBody::Json { .. } => {
            let cap = usize::try_from(shared.config.limits.max_json_kib)
                .unwrap_or(usize::MAX / 1024)
                .saturating_mul(1024);
            route.layer(middleware::from_fn(
                move |mut request: Request, next: Next| {
                    request.extensions_mut().insert(json::JsonCap(cap));
                    json_body(cap, request, next)
                },
            ))
        }
        RequestBody::Upload => {
            let limits = shared.config.limits.clone();
            route.layer(middleware::from_fn(move |request: Request, next: Next| {
                let refused = crate::uploads::check_declared(request.headers(), &limits).err();
                async move {
                    match refused {
                        Some(error) => error.into_response(),
                        None => next.run(request).await,
                    }
                }
            }))
        }
    };
    // HX3: a handler's time; an upload's is the whole upload's, which the
    // upload checks itself, so the layer stands behind it by a minute.
    let upload = op.body == RequestBody::Upload;
    // JB11, M6-53: a wait is not handler work; the handler times its own
    // part, and the layer stands behind the longest wait.
    let waits = op.mode == operations::Mode::Job || op.id == "jobs_get";
    let timeout = if waits {
        Duration::from_secs(
            u64::from(shared.config.server.handler_timeout_seconds)
                + u64::from(shared.config.limits.wait_max_seconds)
                + 10,
        )
    } else if upload {
        shared.config.upload_time() + Duration::from_secs(60)
    } else {
        Duration::from_secs(u64::from(shared.config.server.handler_timeout_seconds))
    };
    route.layer(
        ServiceBuilder::new()
            .layer(HandleErrorLayer::new(
                move |method: Method, error: tower::BoxError| {
                    std::future::ready(timed_out(&method, &error, upload))
                },
            ))
            .timeout(timeout),
    )
}

/// HX2: a JSON body whose declared length passes its cap is `413
/// body_too_large` before a byte is read; the extractor cuts a chunked one
/// at the cap.
async fn json_body(cap: usize, request: Request, next: Next) -> Response {
    let declared = request
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok());
    if declared.is_some_and(|n| n > u64::try_from(cap).unwrap_or(u64::MAX)) {
        return ApiError::new(
            ErrorCode::BodyTooLarge,
            format!("a JSON body is at most {} KiB", cap / 1024),
        )
        .into_response();
    }
    next.run(request).await
}

/// AU3: a token in the query is `400 bad_request`. VN3, AU8: a query
/// parameter the operation does not take is `400 invalid_parameter`, so `?owner=` on a route that takes none is refused,
/// never ignored. Routes that take none accept an empty query.
async fn known_query(allowed: &'static [&'static str], request: Request, next: Next) -> Response {
    // AU3: a token in a query string would reach logs and proxies; refused
    // first, whatever else the request holds.
    if request
        .uri()
        .query()
        .is_some_and(crate::auth::token::in_query)
    {
        return ApiError::new(
            ErrorCode::BadRequest,
            "a token is taken only in the Authorization header, never in the query",
        )
        .into_response();
    }
    if let Err(error) = query::params(request.uri().query(), allowed) {
        return error.into_response();
    }
    next.run(request).await
}

/// Runs blocking work on the blocking pool (SV3); a panic in it is
/// `internal`, its message logged (JS6).
pub(crate) async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, ApiError> + Send + 'static,
) -> Result<T, ApiError> {
    match tokio::task::spawn_blocking(work).await {
        Ok(result) => result,
        Err(error) => {
            let message = error.try_into_panic().ok().and_then(|p| {
                p.downcast_ref::<String>()
                    .cloned()
                    .or_else(|| p.downcast_ref::<&str>().map(|s| (*s).to_owned()))
            });
            tracing::error!(
                panic = message.as_deref().unwrap_or("cancelled"),
                "blocking work failed"
            );
            Err(ApiError::internal())
        }
    }
}

/// OB3, DC12: a read of stored bytes (a document read whole or hashed
/// before it streams, a candidate page) on the blocking pool behind one of
/// `limits.read_concurrency` slots, owned by the read and moved into it,
/// so a request dropped mid-read frees its slot only when the read ends.
/// The wait for one is the parse slots' (half the handler's time); past
/// it, `503 request_timeout` with `details.limit = "read_concurrency"`.
/// Hook point (OP11): `stored.read`, inside the slot.
///
/// # Errors
///
/// `request_timeout`, or `work`'s.
pub(crate) async fn stored<T: Send + 'static>(
    shared: &Arc<Shared>,
    work: impl FnOnce() -> Result<T, ApiError> + Send + 'static,
) -> Result<T, ApiError> {
    let slots = Arc::clone(&shared.read_slots);
    let slot = tokio::time::timeout(crate::gate::slot_wait(shared), slots.acquire_owned())
        .await
        .map_err(|_| {
            ApiError::new(
                ErrorCode::RequestTimeout,
                "the server is busy reading stored bytes: try again later",
            )
            .with("limit", "read_concurrency")
        })?
        .map_err(|_| ApiError::internal())?;
    let hooked = Arc::clone(shared);
    blocking(move || {
        crate::jobs::run::hook(&hooked, "stored.read");
        let result = work();
        drop(slot);
        result
    })
    .await
}

/// CX1, CX2: a catalog read a request makes (a page, a whole list, an
/// entry, its bytes) on the blocking pool behind one of
/// `limits.read_concurrency` slots, as stored bytes' reads are; past the
/// wait, `503 request_timeout`, `details.limit = "read_concurrency"`.
///
/// # Errors
///
/// `request_timeout`, or `work`'s.
pub(crate) async fn catalog_read<T: Send + 'static>(
    shared: &Arc<Shared>,
    work: impl FnOnce() -> Result<T, ApiError> + Send + 'static,
) -> Result<T, ApiError> {
    let slots = Arc::clone(&shared.read_slots);
    let slot = tokio::time::timeout(crate::gate::slot_wait(shared), slots.acquire_owned())
        .await
        .map_err(|_| {
            ApiError::new(
                ErrorCode::RequestTimeout,
                "the server is busy reading the catalog: try again later",
            )
            .with("limit", "read_concurrency")
        })?
        .map_err(|_| ApiError::internal())?;
    blocking(move || {
        let result = work();
        drop(slot);
        result
    })
    .await
}

/// M6-50: the synchronous deletes run at once; past them a delete waits
/// for one as a read waits for its slot, then `503 catalog_busy` (the
/// review of PR 7b).
pub(crate) const DELETE_CONCURRENCY: usize = 2;

/// What one user holds of a shared pool of slots: the catalog's reads
/// and its `limit=all` listings (the review of PR 7b), by user.
#[derive(Debug, Default)]
pub(crate) struct Shares {
    held: std::sync::Mutex<std::collections::HashMap<(i64, &'static str), usize>>,
}

/// One share of a user's, given back when it drops.
pub(crate) struct Share {
    shared: Arc<Shared>,
    key: (i64, &'static str),
}

impl Drop for Share {
    fn drop(&mut self) {
        let mut held = self
            .shared
            .catalog_shares
            .held
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(n) = held.get_mut(&self.key) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                held.remove(&self.key);
            }
        }
    }
}

/// A share of `pool` (the key its `details.limit` names) for user `user`,
/// at most `cap` held at once: past it, `429 too_many_streams`,
/// retryable, `details.per_user` true, as UP4's per-user slots answer.
///
/// # Errors
///
/// `too_many_streams`.
pub(crate) fn share(
    shared: &Arc<Shared>,
    user: i64,
    pool: &'static str,
    cap: usize,
) -> Result<Share, ApiError> {
    let mut held = shared
        .catalog_shares
        .held
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let n = held.entry((user, pool)).or_insert(0);
    if *n >= cap.max(1) {
        return Err(ApiError::new(
            ErrorCode::TooManyStreams,
            "you hold your share of the catalog's reads: try again once one ends",
        )
        .with("limit", pool)
        .with("per_user", true)
        .retry_after(1));
    }
    *n += 1;
    Ok(Share {
        shared: Arc::clone(shared),
        key: (user, pool),
    })
}

/// `500 io_error` for a `server.db` failure, logged, never sent (ER3).
pub(crate) fn db_error(error: &crate::state::DbError) -> ApiError {
    tracing::error!(error = %error, "server.db failed");
    ApiError::new(
        ErrorCode::IoError,
        "the server's state could not be read or written",
    )
}

/// The method filter of an operation's method.
pub(crate) fn method_filter(method: &str) -> MethodFilter {
    match method {
        "POST" => MethodFilter::POST,
        "PUT" => MethodFilter::PUT,
        "DELETE" => MethodFilter::DELETE,
        "PATCH" => MethodFilter::PATCH,
        "HEAD" => MethodFilter::HEAD,
        _ => MethodFilter::GET,
    }
}

/// HX2: a route that takes no body answers a body `413 body_too_large`,
/// before reading it.
async fn no_body(request: Request, next: Next) -> Response {
    let headers = request.headers();
    let length = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.trim() != "0");
    if length || headers.contains_key(header::TRANSFER_ENCODING) {
        return ApiError::new(ErrorCode::BodyTooLarge, "this route takes no request body")
            .into_response();
    }
    next.run(request).await
}

/// VN8: an `Accept` that excludes the route's one representation is `406`.
async fn accept(media: &'static str, request: Request, next: Next) -> Response {
    if json::acceptable(request.headers().get(header::ACCEPT), media) {
        return next.run(request).await;
    }
    ApiError::new(
        ErrorCode::NotAcceptable,
        format!("this route answers {media} only"),
    )
    .with("media_type", media)
    .into_response()
}

/// HX5: a path no operation names is `404`; the path is not echoed.
async fn not_found() -> ApiError {
    ApiError::new(ErrorCode::NotFound, "no resource has this path")
}

/// A path whose operations take other methods (section 7.12).
async fn method_not_allowed(request: Request) -> ApiError {
    let path = request
        .extensions()
        .get::<MatchedPath>()
        .map(|p| p.as_str().to_owned());
    let mut allowed: Vec<&str> = Vec::new();
    for op in OPERATIONS
        .iter()
        .filter(|op| Some(op.path) == path.as_deref())
    {
        let methods: &[&str] = if op.method == "GET" {
            &["GET", "HEAD"]
        } else {
            std::slice::from_ref(&op.method)
        };
        for method in methods {
            if !allowed.contains(method) {
                allowed.push(method);
            }
        }
    }
    ApiError::new(
        ErrorCode::MethodNotAllowed,
        "this method is not allowed on this route",
    )
    .with("allowed", allowed.clone())
    .allow(allowed.join(", "))
}

/// HX3: a handler past its time is `503 request_timeout`, retryable on
/// `GET`, `HEAD` and an upload's `PUT` only (ER6).
fn timed_out(method: &Method, error: &tower::BoxError, upload: bool) -> ApiError {
    if error.is::<tower::timeout::error::Elapsed>() {
        let retryable = ErrorCode::RequestTimeout.retryable_on(method, upload);
        let error = ApiError::new(ErrorCode::RequestTimeout, "the request took too long")
            .retryable(retryable);
        if retryable {
            error.retry_after(1)
        } else {
            error
        }
    } else {
        tracing::error!(error = %error, "a layer failed");
        ApiError::internal()
    }
}

/// JS6's handler half: a panic is `internal`, its message in the log
/// only, under the request's span (ER3, XH13).
#[allow(clippy::needless_pass_by_value)] // CatchPanicLayer's signature
fn panicked(panic: Box<dyn std::any::Any + Send + 'static>) -> Response {
    let message = panic
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| panic.downcast_ref::<&str>().copied())
        .unwrap_or("a panic");
    tracing::error!(panic = message, "a handler panicked");
    ApiError::internal().into_response()
}

/// HX4: CORS only with an allowlist of exact origins.
fn cors(origins: &[String]) -> Option<CorsLayer> {
    let list: Vec<HeaderValue> = origins
        .iter()
        .filter_map(|o| HeaderValue::from_str(o).ok())
        .collect();
    if list.is_empty() {
        return None;
    }
    Some(
        CorsLayer::new()
            .allow_origin(AllowOrigin::list(list))
            .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE])
            .allow_headers([
                header::AUTHORIZATION,
                header::CONTENT_TYPE,
                HeaderName::from_static("idempotency-key"),
                HeaderName::from_static("prefer"),
            ]),
    )
}

/// HX7: whether `peer` is a trusted proxy. An IPv4 peer on a dual-stack
/// socket arrives as `::ffff:a.b.c.d` and is compared as its IPv4 address;
/// `trusted` holds canonical addresses.
fn is_trusted(trusted: &[IpAddr], peer: IpAddr) -> bool {
    trusted.contains(&peer.to_canonical())
}

/// HX7: the id a trusted proxy sent, if well formed (at most 64 of
/// `[A-Za-z0-9._-]`), else a new one.
fn request_id(shared: &Shared, peer: Option<IpAddr>, given: Option<&HeaderValue>) -> String {
    let trusted = peer.is_some_and(|p| is_trusted(&shared.trusted_proxies, p));
    let well_formed = |v: &&str| {
        !v.is_empty()
            && v.len() <= 64
            && v.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    };
    match given.and_then(|v| v.to_str().ok()).filter(well_formed) {
        Some(id) if trusted => id.to_owned(),
        _ => shared.ids.id("r"),
    }
}

/// How often, at most, the warning below is logged.
const FORWARDED_WARNING_MS: i64 = 60_000;

/// AU7, HX7: `X-Forwarded-For` from a peer that is not a trusted proxy is
/// ignored; the first such request a minute logs one warning naming
/// neither the peer nor the header's values, so the operator learns that
/// `server.trusted_proxies` may lack the proxy (review).
fn forwarded_from_untrusted(shared: &Shared, peer: IpAddr, headers: &axum::http::HeaderMap) {
    if !headers.contains_key("x-forwarded-for") || is_trusted(&shared.trusted_proxies, peer) {
        return;
    }
    let now = shared.ids.now_ms();
    let last = shared
        .forwarded_warned_ms
        .load(std::sync::atomic::Ordering::Relaxed);
    if last != 0 && now.saturating_sub(last) < FORWARDED_WARNING_MS {
        return;
    }
    if shared
        .forwarded_warned_ms
        .compare_exchange(
            last,
            now.max(1),
            std::sync::atomic::Ordering::Relaxed,
            std::sync::atomic::Ordering::Relaxed,
        )
        .is_ok()
    {
        tracing::warn!(
            "X-Forwarded-For from a peer that is not in server.trusted_proxies was ignored \
             (logged at most once a minute)"
        );
    }
}

/// The outer layer: the request id (HX7); an [`ApiError`] written as the
/// problem document with that id (ER, DC9); HX6's headers on every
/// response.
async fn finish(State(shared): State<Arc<Shared>>, mut request: Request, next: Next) -> Response {
    let peer = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|c| c.0.ip());
    let id = request_id(&shared, peer, request.headers().get(&REQUEST_ID));
    if let Some(peer) = peer {
        forwarded_from_untrusted(&shared, peer, request.headers());
    }
    let method = request.method().clone();
    if let Ok(value) = HeaderValue::from_str(&id) {
        request.headers_mut().insert(&REQUEST_ID, value);
    }
    request.extensions_mut().insert(RequestId(id.clone()));
    // SD4: jobs recovery queued again run from the first request, for a
    // server that has not served yet (in-process tests).
    shared.jobs.kick_once(&shared);
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map_or_else(|| "(none)".to_owned(), |p| p.as_str().to_owned());
    let started = std::time::Instant::now();
    let mut response = next.run(request).await;
    shared
        .metrics
        .request(&route, response.status().as_u16(), started.elapsed());
    if let Some(error) = response.extensions_mut().remove::<ApiError>() {
        let problem = error.document(&method, &id);
        let body = json::bytes(
            response.status(),
            json::PROBLEM_JSON,
            Some((error::ERROR_FORMAT, error::ERROR_FORMAT_VERSION)),
            Bytes::from(json::pretty(&problem)),
        );
        let (mut parts, body) = body.into_parts();
        for (name, value) in response.headers() {
            parts.headers.insert(name.clone(), value.clone());
        }
        parts.status = response.status();
        response = Response::from_parts(parts, body);
    }
    let no_content = response.status() == StatusCode::NO_CONTENT;
    let headers = response.headers_mut();
    if let Ok(value) = HeaderValue::from_str(&id) {
        headers.insert(&REQUEST_ID, value);
    }
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("default-src 'none'; frame-ancestors 'none'"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    if !headers.contains_key(header::CACHE_CONTROL) {
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    headers.remove(header::SERVER);
    if no_content {
        headers.remove(header::CONTENT_TYPE);
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    /// HX7: an IPv4-mapped IPv6 peer is its IPv4 address.
    #[test]
    fn a_mapped_peer_is_its_ipv4_address() {
        let trusted: Vec<IpAddr> = vec!["10.0.0.1".parse().unwrap()];
        assert!(is_trusted(&trusted, "10.0.0.1".parse().unwrap()));
        assert!(is_trusted(&trusted, "::ffff:10.0.0.1".parse().unwrap()));
        assert!(!is_trusted(&trusted, "10.0.0.2".parse().unwrap()));
        assert!(!is_trusted(&trusted, "::1".parse().unwrap()));
    }
}
