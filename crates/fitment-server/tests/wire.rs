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
    let info = Call::get("/v1/info").bearer(server.token());
    let slow = tokio::spawn(async move { support::wire(addr, &info).await });
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

/// VT13's download half, JS10: over the bound socket, a client that drops
/// a stored file's download mid-way, through the two-pass streamed path,
/// changes nothing (the job and the object as they were, the next
/// download whole); a dropped event stream affects nothing, and its
/// channel closes when its job ends.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_client_gone_mid_download_or_mid_stream_changes_nothing() {
    use std::io::{Read as _, Write as _};
    let dirs = Dirs::new();
    let config = dirs.config();
    let server = TestServer::bound(dirs, config).await;
    server.server().seams().objects.set_one_buffer_max(1024);
    let big: Vec<u8> = (0..3_000_000_u32)
        .map(|i| u8::try_from(i % 251).unwrap())
        .collect();
    let job = server.register(&big, "big.stp").await;
    assert_eq!(job["state"], "refused", "no STEP file, registered refused");
    let sha = fitment_core::sha256_hex(&big);
    let token = server.token().to_owned();
    let addr = server.addr();
    let partial = tokio::task::spawn_blocking(move || {
        let mut stream = std::net::TcpStream::connect(addr).unwrap();
        write!(
            stream,
            "GET /v1/assemblies/sha256:{sha}/step HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {token}\r\n\r\n"
        )
        .unwrap();
        let mut first = vec![0_u8; 64 * 1024];
        stream.read_exact(&mut first).unwrap();
        first
    })
    .await
    .unwrap();
    assert!(partial.starts_with(b"HTTP/1.1 200"));
    let whole = server
        .get(&format!(
            "/v1/assemblies/sha256:{}/step",
            fitment_core::sha256_hex(&big)
        ))
        .await;
    assert_eq!(whole.body, big, "the next download is whole");
    let again = server
        .get(&format!("/v1/jobs/{}", job["id"].as_str().unwrap()))
        .await
        .json();
    assert_eq!(again["state"], job["state"]);
    server
        .server()
        .seams()
        .objects
        .set_one_buffer_max(fitment_server::state::objects::ONE_BUFFER_MAX);
    assert_eq!(server.store_problems(), [] as [std::string::String; 0]);
    let sha = server.upload(&support::part("p01_through.stp")).await;
    server.server().hold_jobs();
    let queued = server
        .send(Call::json(
            axum::http::Method::POST,
            "/v1/features",
            &serde_json::json!({ "upload": format!("sha256:{sha}") }),
        ))
        .await
        .json();
    let id = queued["id"].as_str().unwrap().to_owned();
    let token = server.token().to_owned();
    let stream = support::wire_open(
        server.addr(),
        &Call::get(&format!("/v1/jobs/{id}/events")).bearer(&token),
    )
    .await;
    drop(stream);
    server.server().resume_jobs();
    let done = server.finished_job(&id, Duration::from_secs(30)).await;
    assert_eq!(done["state"], "succeeded");
    let start = Instant::now();
    while server.server().seams().jobs.channels_open() > 0 {
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "a channel stayed open"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// OP6: `metrics.listen` serves Prometheus text at `/metrics` on its own
/// port, without authentication; any other path there is `404`; past
/// sixteen open at once, a connection is closed at accept
/// (review S11 of PR 5).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn metrics_are_served_on_their_own_listener() {
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let dirs = Dirs::new();
    let mut config = dirs.config();
    config.metrics.listen = format!("127.0.0.1:{port}");
    let mut server = TestServer::bound(dirs, config).await;
    let addr: std::net::SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    let start = Instant::now();
    let reply = loop {
        if let Some(reply) = support::try_wire(addr, &Call::get("/metrics").anonymous()).await {
            break reply;
        }
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "the metrics listener never bound"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert_eq!(reply.status, 200);
    assert!(
        reply
            .header("content-type")
            .unwrap()
            .starts_with("text/plain")
    );
    let text = String::from_utf8(reply.body).unwrap();
    assert!(text.contains("# TYPE fitment_jobs_queued gauge"), "{text}");
    let other = support::try_wire(addr, &Call::get("/v1/info").anonymous())
        .await
        .unwrap();
    assert_eq!(other.status, 404);
    // Review S11 of PR 5: past its cap (net.rs's METRICS_CONNECTIONS, 16),
    // a connection is closed at accept. Connections are opened one by one
    // until one is closed; at most 16 may have been held open, whatever
    // the earlier requests' connections had released by then (on CI's
    // Linux runner they had not, and a 17th fixed connection was served).
    let mut idle = Vec::new();
    let mut closed = false;
    for _ in 0..24 {
        let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
        let mut byte = [0_u8; 1];
        let read = tokio::time::timeout(
            Duration::from_millis(500),
            tokio::io::AsyncReadExt::read(&mut stream, &mut byte),
        )
        .await;
        match read {
            Ok(Ok(0) | Err(_)) => {
                closed = true;
                break;
            }
            Ok(Ok(_)) => panic!("an idle metrics connection was sent bytes"),
            Err(_) => idle.push(stream),
        }
    }
    assert!(closed, "no connection was closed past the cap");
    assert!(
        idle.len() <= 16,
        "{} connections were held open",
        idle.len()
    );
    drop(idle);
    let start = Instant::now();
    loop {
        let reply = support::try_wire(addr, &Call::get("/metrics").anonymous()).await;
        if reply.is_some_and(|r| r.status == 200) {
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "the slots never came back"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    server.stop().await;
}

/// Bytes that are no STEP file, of `n` bytes, distinct per `seed`.
fn junk(n: u32, seed: u8) -> Vec<u8> {
    (0..n)
        .map(|i| u8::try_from(i % 251).unwrap() ^ seed)
        .collect()
}

/// Review S1 of PR 5, DC6, OB3: an object above `SEND_BUFFER_MAX` (1 MiB)
/// is streamed (no `Content-Length`, chunked) and never gzipped, its bytes
/// and `Repr-Digest` the object's; one within it is sent from one buffer
/// (`Content-Length`) and gzipped on request.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_large_object_is_streamed_and_a_small_one_buffered() {
    let dirs = Dirs::new();
    let config = dirs.config();
    let server = TestServer::bound(dirs, config).await;
    for (bytes, streamed) in [(junk(2 << 20, 1), true), (junk(512 << 10, 2), false)] {
        server.register(&bytes, "blob.stp").await;
        let path = format!(
            "/v1/assemblies/sha256:{}/step",
            fitment_core::sha256_hex(&bytes)
        );
        let plain = server.get(&path).await;
        assert_eq!(plain.body, bytes);
        assert_eq!(
            plain.header("content-length").is_none(),
            streamed,
            "{streamed}"
        );
        assert_eq!(
            plain.header("repr-digest"),
            Some(fitment_server::http::json::repr_digest(&bytes).as_str())
        );
        let asked = server
            .send(Call::get(&path).header("accept-encoding", "gzip"))
            .await;
        assert_eq!(
            asked.header("content-encoding").is_some(),
            !streamed,
            "{streamed}"
        );
    }
}

/// Review R of PR 5: downloads nobody reads hold no blocking thread. On a
/// runtime of four blocking threads, twelve streamed downloads left unread
/// (each past the socket's buffers) leave `GET /v1/jobs`, which needs the
/// blocking pool, answering.
#[test]
fn unread_downloads_hold_no_blocking_thread() {
    use std::io::Write as _;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .max_blocking_threads(4)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let dirs = Dirs::new();
        let config = dirs.config();
        let server = TestServer::bound(dirs, config).await;
        let bytes = junk(16 << 20, 3);
        server.register(&bytes, "big.stp").await;
        // The store's one-buffer cap lowered, so a build that streamed only
        // above it streams these too.
        server.server().seams().objects.set_one_buffer_max(1 << 20);
        let token = server.token().to_owned();
        let path = format!(
            "/v1/assemblies/sha256:{}/step",
            fitment_core::sha256_hex(&bytes)
        );
        let mut held = Vec::new();
        for _ in 0..12 {
            let mut stream = std::net::TcpStream::connect(server.addr()).unwrap();
            write!(
                stream,
                "GET {path} HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {token}\r\n\r\n"
            )
            .unwrap();
            held.push(stream);
        }
        // Long enough for the downloads to fill their sockets' buffers.
        tokio::time::sleep(Duration::from_millis(500)).await;
        let listed =
            tokio::time::timeout(Duration::from_secs(10), server.send(Call::get("/v1/jobs")))
                .await
                .expect("GET /v1/jobs answers beside unread downloads");
        assert_eq!(listed.status, 200);
        drop(held);
    });
}

/// ST11, OB3, DC6 (the review of PR 7b): a catalog file above
/// `SEND_BUFFER_MAX` is checked in one pass and streamed in a second (no
/// `Content-Length`, chunked), never gzipped, its bytes and `Repr-Digest`
/// the file's; one within it is sent from one buffer and gzipped on
/// request, its digest the gzipped bytes'.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_large_catalog_file_is_streamed_and_a_small_one_buffered() {
    let dirs = Dirs::new();
    let config = dirs.config();
    let server = TestServer::bound(dirs, config).await;
    let files = [
        (junk(2 << 20, 4), "large.stp"),
        (junk(512 << 10, 5), "small.stp"),
    ];
    let listed: Vec<(Vec<u8>, String)> = files
        .iter()
        .map(|(bytes, name)| (bytes.clone(), (*name).to_owned()))
        .collect();
    server.ingest(&listed).await;
    for ((bytes, _), streamed) in files.iter().zip([true, false]) {
        let path = format!(
            "/v1/catalog/entries/sha256:{}/step",
            fitment_core::sha256_hex(bytes)
        );
        let plain = server.get(&path).await;
        assert_eq!(&plain.body, bytes);
        assert_eq!(
            plain.header("content-length").is_none(),
            streamed,
            "{streamed}"
        );
        assert_eq!(
            plain.header("repr-digest"),
            Some(fitment_server::http::json::repr_digest(bytes).as_str())
        );
        let asked = server
            .send(Call::get(&path).header("accept-encoding", "gzip"))
            .await;
        assert_eq!(
            asked.header("content-encoding").is_some(),
            !streamed,
            "{streamed}"
        );
    }
}
