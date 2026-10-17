//! The router and its layers (docs/research/m6-api.md, section 4.12).
//!
//! From the outside in: sensitive headers marked (OP1); the request id,
//! the error body and the response headers (HX7, ER, HX6); the trace span
//! (OP1); CORS, only with an allowlist (HX4); panics caught (JS6); the
//! handler timeout (HX3); then, per route, the body limit (HX2) and
//! `Accept` (VN8), so a usage error is answered before anything is read
//! (ER5). The routes come from `OPERATIONS` alone (XH14).

pub mod error;
pub mod json;
pub mod operations;
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
        let route = per_route(op, routes::handler(op));
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
    let timeout = Duration::from_secs(u64::from(shared.config.server.handler_timeout_seconds));
    let router = router
        .fallback(not_found)
        .method_not_allowed_fallback(method_not_allowed)
        .layer(DefaultBodyLimit::disable())
        .layer(
            ServiceBuilder::new()
                .layer(HandleErrorLayer::new(timed_out))
                .timeout(timeout),
        )
        .layer(CatchPanicLayer::custom(panicked));
    let router = match cors(&shared.config.server.cors_origins) {
        Some(layer) => router.layer(layer),
        None => router,
    };
    // OP1's request span: the request id, the method, the matched route
    // template (never the raw URI or query), then the status and latency.
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
            tracing::info_span!("request", request_id = id, method = %request.method(), route)
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

/// A route's own layers, from its operation: no body where it takes none
/// (HX2), checked before `Accept` (VN8; ER5's order).
fn per_route(
    op: &'static Operation,
    route: MethodRouter<Arc<Shared>>,
) -> MethodRouter<Arc<Shared>> {
    let media = op.answers.iter().find_map(|a| a.media);
    let route = match media {
        Some(media) => route.layer(middleware::from_fn(move |request: Request, next: Next| {
            accept(media, request, next)
        })),
        None => route,
    };
    match op.body {
        RequestBody::None => route.layer(middleware::from_fn(no_body)),
    }
}

/// The method filter of an operation's method.
pub(crate) fn method_filter(method: &str) -> MethodFilter {
    match method {
        "POST" => MethodFilter::POST,
        "PUT" => MethodFilter::PUT,
        "DELETE" => MethodFilter::DELETE,
        "PATCH" => MethodFilter::PATCH,
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
    let allowed: Vec<&str> = OPERATIONS
        .iter()
        .filter(|op| Some(op.path) == path.as_deref())
        .flat_map(|op| {
            if op.method == "GET" {
                vec!["GET", "HEAD"]
            } else {
                vec![op.method]
            }
        })
        .collect();
    ApiError::new(
        ErrorCode::MethodNotAllowed,
        "this method is not allowed on this route",
    )
    .with("allowed", allowed.clone())
    .allow(allowed.join(", "))
}

/// HX3: a handler past its time is `503 request_timeout`, retryable on
/// `GET` and `HEAD` only (ER6).
async fn timed_out(method: Method, error: tower::BoxError) -> ApiError {
    if error.is::<tower::timeout::error::Elapsed>() {
        let retryable = ErrorCode::RequestTimeout.retryable_on(&method, false);
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

/// HX7: the id a trusted proxy sent, if well formed (at most 64 of
/// `[A-Za-z0-9._-]`), else a new one.
fn request_id(shared: &Shared, peer: Option<IpAddr>, given: Option<&HeaderValue>) -> String {
    let trusted = peer.is_some_and(|p| shared.trusted_proxies.contains(&p));
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

/// The outer layer: the request id (HX7); an [`ApiError`] written as the
/// problem document with that id (ER, DC9); HX6's headers on every
/// response.
async fn finish(State(shared): State<Arc<Shared>>, mut request: Request, next: Next) -> Response {
    let peer = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|c| c.0.ip());
    let id = request_id(&shared, peer, request.headers().get(&REQUEST_ID));
    let method = request.method().clone();
    if let Ok(value) = HeaderValue::from_str(&id) {
        request.headers_mut().insert(&REQUEST_ID, value);
    }
    request.extensions_mut().insert(RequestId(id.clone()));
    let mut response = next.run(request).await;
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
