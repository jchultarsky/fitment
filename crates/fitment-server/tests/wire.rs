//! The server on a bound socket (docs/research/m6-api.md, section 9.4):
//! `Server::serve` and its shutdown, request ids from a trusted proxy, and
//! blocking work kept off the runtime's workers.

mod support;

use std::time::{Duration, Instant};

use fitment_server::Server;
use fitment_server::hooks::Action;
use support::{Call, Dirs, TestServer};

/// SV2: the library serves a bound listener until its shutdown future
/// ends, then returns, releasing the lease (SD6), so a second server on
/// the same directories starts at once; `router()` serves the same routes
/// in process.
#[tokio::test(flavor = "multi_thread")]
async fn serve_runs_until_its_shutdown() {
    let dirs = Dirs::new();
    let config = dirs.config();
    let mut server = TestServer::bound(dirs, config).await;
    assert_eq!(server.get("/v1/health").await.status, 204);
    assert_eq!(server.get("/v1/info").await.status, 200);
    server.stop().await;
    assert!(
        tokio::net::TcpStream::connect(server.addr()).await.is_err(),
        "the listener is closed"
    );
    let again = Server::build(server.dirs.config()).expect("the lease was released");
    let response = tower::ServiceExt::oneshot(
        again.router(),
        axum::http::Request::get("/v1/health")
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(response.status(), 204);
}

/// SV3: blocking work runs on the blocking pool, never on a runtime
/// worker: on a one-thread runtime, a health check answers while `info`'s
/// blocking work is held for two seconds.
#[tokio::test(flavor = "current_thread")]
async fn blocking_work_never_holds_a_worker() {
    let dirs = Dirs::new();
    let config = dirs.config();
    let mut server = TestServer::bound(dirs, config).await;
    server
        .server()
        .seams()
        .hooks
        .set("info.blocking", Action::Delay(Duration::from_secs(2)));
    let addr = server.addr();
    let slow = tokio::spawn(async move { support::wire(addr, &Call::get("/v1/info")).await });
    tokio::time::sleep(Duration::from_millis(200)).await;
    let started = Instant::now();
    assert_eq!(server.get("/v1/health").await.status, 204);
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "{:?}",
        started.elapsed()
    );
    assert!(!slow.is_finished());
    assert_eq!(slow.await.unwrap().status, 200);
    server.stop().await;
}

/// HX7: a request id is generated unless a trusted proxy sent a well-formed
/// one (at most 64 of `[A-Za-z0-9._-]`); it is on the response and in
/// every error document.
#[tokio::test(flavor = "multi_thread")]
async fn request_ids_are_believed_from_trusted_proxies_only() {
    let given = "a".repeat(64);
    let too_long = "a".repeat(65);
    let trusted = {
        let dirs = Dirs::new();
        let mut config = dirs.config();
        config.server.trusted_proxies = vec!["127.0.0.1".into()];
        TestServer::bound(dirs, config).await
    };
    let untrusted = {
        let dirs = Dirs::new();
        let config = dirs.config();
        TestServer::bound(dirs, config).await
    };
    for (server, id, kept) in [
        (&trusted, given.as_str(), true),
        (&trusted, "proxy-1.2_x", true),
        (&trusted, too_long.as_str(), false),
        (&trusted, "bad id", false),
        (&trusted, "naïve", false),
        (&untrusted, given.as_str(), false),
    ] {
        let reply = server
            .send(Call::get("/v1/nope").header("x-request-id", id))
            .await;
        let answered = reply.header("x-request-id").unwrap().to_owned();
        assert_eq!(answered == id, kept, "{id}");
        if !kept {
            assert!(
                answered.starts_with("r_") && answered.len() == 28,
                "{answered}"
            );
        }
        assert_eq!(reply.json()["request_id"], answered);
    }
    let reply = trusted.get("/v1/health").await;
    assert!(reply.header("x-request-id").unwrap().starts_with("r_"));
    for mut server in [trusted, untrusted] {
        server.stop().await;
    }
}
