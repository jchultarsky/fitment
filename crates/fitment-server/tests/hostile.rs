//! Hostile requests in process (docs/research/m6-api.md, section 9.4 and
//! 10.4): each bound of `M6_LIMITS` that needs no wire, exceeded.

mod support;

use std::time::{Duration, Instant};

use axum::http::Method;
use fitment_server::hooks::Action;
use support::{Call, Dirs, TestServer};

/// HX2: a route that takes no body answers any body `413 body_too_large`
/// before reading it, whatever its length or framing; an empty one is
/// fine.
#[tokio::test]
async fn a_body_where_none_is_taken_is_refused() {
    let server = TestServer::fresh();
    for path in ["/v1/health", "/v1/ready", "/v1/info", "/v1/openapi.json"] {
        let reply = server.send(Call::get(path).body(b"x")).await;
        assert_eq!(reply.status, 413, "{path}");
        assert_eq!(reply.code(), "body_too_large");
        let chunked = Call::get(path).header("transfer-encoding", "chunked");
        assert_eq!(
            server.send(chunked).await.code(),
            "body_too_large",
            "{path}"
        );
        let declared = Call::get(path).header("content-length", "1048576");
        assert_eq!(
            server.send(declared).await.code(),
            "body_too_large",
            "{path}"
        );
        let empty = Call::get(path).header("content-length", "0");
        assert_ne!(server.send(empty).await.status, 413, "{path}");
    }
}

/// HX3: a handler past `server.handler_timeout_seconds` is `503
/// request_timeout`, retryable on `GET` with `Retry-After` (ER6); a
/// handler within it answers.
#[tokio::test]
async fn a_handler_past_its_time_is_cut() {
    let dirs = Dirs::new();
    let mut config = dirs.config();
    config.server.handler_timeout_seconds = 1;
    let server = TestServer::in_process(dirs, config);
    server
        .server()
        .seams()
        .hooks
        .set("info", Action::Delay(Duration::from_millis(1500)));
    let started = Instant::now();
    let reply = server.get("/v1/info").await;
    assert!(
        started.elapsed() < Duration::from_millis(1400),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(
        (reply.status.as_u16(), reply.code()),
        (503, "request_timeout".to_owned())
    );
    assert_eq!(reply.json()["retryable"], true);
    assert_eq!(reply.header("retry-after"), Some("1"));
    let reply = server.send(Call::new(Method::HEAD, "/v1/info")).await;
    assert_eq!(reply.status, 503);
    server
        .server()
        .seams()
        .hooks
        .set("info", Action::Delay(Duration::from_millis(200)));
    assert_eq!(server.get("/v1/info").await.status, 200);
}

/// HX3: blocking work past the handler's time is cut too; its result,
/// when it comes, is dropped.
#[tokio::test]
async fn blocking_work_past_its_time_is_cut() {
    let dirs = Dirs::new();
    let mut config = dirs.config();
    config.server.handler_timeout_seconds = 1;
    let server = TestServer::in_process(dirs, config);
    server
        .server()
        .seams()
        .hooks
        .set("info.blocking", Action::Delay(Duration::from_millis(1500)));
    let reply = server.get("/v1/info").await;
    assert_eq!(reply.code(), "request_timeout");
}

/// Open question 2 of PR 2's review: `/v1/info` needs no authentication
/// until PR 3, so at most four read the catalog at once; a fifth waits for
/// a slot within the handler's time.
#[tokio::test(flavor = "multi_thread")]
async fn info_reads_the_catalog_four_at_a_time() {
    let server = std::sync::Arc::new(TestServer::fresh());
    server
        .server()
        .seams()
        .hooks
        .set("info.blocking", Action::Delay(Duration::from_millis(800)));
    let started = Instant::now();
    let mut calls = Vec::new();
    for _ in 0..5 {
        let server = std::sync::Arc::clone(&server);
        calls.push(tokio::spawn(
            async move { server.get("/v1/info").await.status },
        ));
    }
    for call in calls {
        assert_eq!(call.await.unwrap(), 200);
    }
    let elapsed = started.elapsed();
    assert!(
        elapsed >= Duration::from_millis(1600),
        "five ran at once: {elapsed:?}"
    );
    assert_eq!(fitment_server::INFO_CONCURRENCY, 4);
}
