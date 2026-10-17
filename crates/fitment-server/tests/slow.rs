//! Slow and hostile clients on a bound socket (docs/research/m6-api.md,
//! section 9.4 and 10.4; VT18): what only the wire shows.

mod support;

use std::io::{Read as _, Write as _};
use std::net::TcpStream;
use std::time::{Duration, Instant};

use support::{Dirs, TestServer};

/// Reads what the server sends until it closes or `limit` passes: the
/// bytes, and whether it closed.
fn read_all(stream: &mut TcpStream, limit: Duration) -> (Vec<u8>, bool) {
    // macOS refuses the option on a socket the server already reset.
    if stream.set_read_timeout(Some(limit)).is_err() {
        return (Vec::new(), true);
    }
    let mut out = Vec::new();
    let mut buf = [0_u8; 4096];
    loop {
        match stream.read(&mut buf) {
            Ok(0) => return (out, true),
            Ok(n) => out.extend_from_slice(&buf[..n]),
            // Windows answers a closed socket with a reset or an abort.
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::ConnectionAborted
                ) =>
            {
                return (out, true);
            }
            Err(_) => return (out, false),
        }
    }
}

async fn bound(configure: impl FnOnce(&mut fitment_server::Config)) -> TestServer {
    let dirs = Dirs::new();
    let mut config = dirs.config();
    configure(&mut config);
    TestServer::bound(dirs, config).await
}

/// HX1: a client that sends a request line and one header, then stalls,
/// is disconnected at the header timeout with nothing served.
#[tokio::test(flavor = "multi_thread")]
async fn a_stalled_header_is_cut_at_the_timeout() {
    let mut server = bound(|c| c.server.header_timeout_seconds = 1).await;
    let addr = server.addr();
    let (bytes, closed, elapsed) = tokio::task::spawn_blocking(move || {
        let mut stream = TcpStream::connect(addr).unwrap();
        stream
            .write_all(b"GET /v1/health HTTP/1.1\r\nHost: x\r\n")
            .unwrap();
        let start = Instant::now();
        let (bytes, closed) = read_all(&mut stream, Duration::from_secs(10));
        (bytes, closed, start.elapsed())
    })
    .await
    .unwrap();
    assert!(closed, "the connection stayed open");
    assert!(
        !String::from_utf8_lossy(&bytes).contains(" 204 "),
        "it was served"
    );
    assert!(
        elapsed >= Duration::from_millis(900) && elapsed < Duration::from_secs(5),
        "{elapsed:?}"
    );
    server.stop().await;
}

/// HX1: at most `server.max_connections` connections; one more is closed
/// at accept, and a slot freed is a slot again.
#[tokio::test(flavor = "multi_thread")]
async fn connections_past_the_cap_are_closed_at_accept() {
    let mut server = bound(|c| c.server.max_connections = 2).await;
    let addr = server.addr();
    tokio::task::spawn_blocking(move || {
        let first = TcpStream::connect(addr).unwrap();
        let second = TcpStream::connect(addr).unwrap();
        std::thread::sleep(Duration::from_millis(200));
        let mut third = TcpStream::connect(addr).unwrap();
        let _ = third.write_all(b"GET /v1/health HTTP/1.1\r\nHost: x\r\n\r\n");
        let (bytes, closed) = read_all(&mut third, Duration::from_secs(5));
        assert!(
            closed && bytes.is_empty(),
            "{}",
            String::from_utf8_lossy(&bytes)
        );
        drop(first);
        drop(second);
        std::thread::sleep(Duration::from_millis(300));
        let mut fourth = TcpStream::connect(addr).unwrap();
        fourth
            .write_all(b"GET /v1/health HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
            .unwrap();
        let (bytes, _) = read_all(&mut fourth, Duration::from_secs(5));
        assert!(String::from_utf8_lossy(&bytes).starts_with("HTTP/1.1 204"));
    })
    .await
    .unwrap();
    server.stop().await;
}

/// HX1: the request line and headers are bounded at 64 KiB: a head
/// within it is served, a larger one refused by hyper with `431` and
/// closed, never served and never a `500`.
#[tokio::test(flavor = "multi_thread")]
async fn a_head_past_64_kib_is_refused() {
    let mut server = bound(|_| {}).await;
    let addr = server.addr();
    tokio::task::spawn_blocking(move || {
        for (kib, status) in [(60, "204"), (65, "431"), (300, "431")] {
            let mut stream = TcpStream::connect(addr).unwrap();
            let header = "a".repeat(kib * 1024);
            let _ = write!(
                stream,
                "GET /v1/health HTTP/1.1\r\nHost: x\r\nX-Big: {header}\r\n\r\n"
            );
            let (bytes, _) = read_all(&mut stream, Duration::from_secs(5));
            let text = String::from_utf8_lossy(&bytes);
            assert!(
                text.starts_with(&format!("HTTP/1.1 {status}")),
                "{kib} KiB: {text}"
            );
        }
    })
    .await
    .unwrap();
    server.stop().await;
}

/// HX1: HTTP/1.1 only: an HTTP/2 preface or an upgrade is never answered
/// in HTTP/2 or switched.
#[tokio::test(flavor = "multi_thread")]
async fn no_http2_and_no_upgrade() {
    let mut server = bound(|_| {}).await;
    let addr = server.addr();
    tokio::task::spawn_blocking(move || {
        let mut stream = TcpStream::connect(addr).unwrap();
        stream.write_all(b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n").unwrap();
        let (bytes, _) = read_all(&mut stream, Duration::from_secs(5));
        assert!(bytes.is_empty() || bytes.starts_with(b"HTTP/1.1 4"), "{bytes:?}");
        let mut stream = TcpStream::connect(addr).unwrap();
        stream
            .write_all(
                b"GET /v1/health HTTP/1.1\r\nHost: x\r\nConnection: Upgrade, close\r\nUpgrade: h2c\r\n\r\n",
            )
            .unwrap();
        let (bytes, _) = read_all(&mut stream, Duration::from_secs(5));
        let text = String::from_utf8_lossy(&bytes);
        assert!(!text.contains("101"), "{text}");
    })
    .await
    .unwrap();
    server.stop().await;
}

/// HX3 on the wire: a handler past its time answers `503 request_timeout`
/// over HTTP too, and the connection serves the next request.
#[tokio::test(flavor = "multi_thread")]
async fn a_slow_handler_times_out_on_the_wire() {
    let mut server = bound(|c| c.server.handler_timeout_seconds = 1).await;
    server.server().seams().hooks.set(
        "info",
        fitment_server::hooks::Action::Delay(Duration::from_secs(3)),
    );
    let reply = server.get("/v1/info").await;
    assert_eq!(reply.code(), "request_timeout");
    assert_eq!(server.get("/v1/health").await.status, 204);
    server.stop().await;
}

/// HX3's keep-alive: an idle connection is closed. hyper's header timer
/// runs from the moment a connection waits for its next request, so the
/// header timeout bounds the idle time too (10 s by default, below HX3's
/// 60 s).
#[tokio::test(flavor = "multi_thread")]
async fn an_idle_connection_is_closed() {
    let mut server = bound(|c| c.server.header_timeout_seconds = 1).await;
    let addr = server.addr();
    tokio::task::spawn_blocking(move || {
        let mut stream = TcpStream::connect(addr).unwrap();
        stream
            .write_all(b"GET /v1/health HTTP/1.1\r\nHost: x\r\n\r\n")
            .unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut buf = [0_u8; 4096];
        let n = stream.read(&mut buf).unwrap();
        assert!(buf[..n].starts_with(b"HTTP/1.1 204"));
        let start = Instant::now();
        let (rest, closed) = read_all(&mut stream, Duration::from_secs(10));
        assert!(closed && rest.is_empty(), "{rest:?}");
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "{:?}",
            start.elapsed()
        );
    })
    .await
    .unwrap();
    server.stop().await;
}

/// Review S1: a client that pipelines requests and never reads holds its
/// connection only until a write makes no progress for
/// `server.write_timeout_seconds`; its slot is then free for another
/// client, with `server.max_connections = 1`.
#[tokio::test(flavor = "multi_thread")]
async fn a_client_that_never_reads_loses_its_connection() {
    let mut server = bound(|c| {
        c.server.max_connections = 1;
        c.server.write_timeout_seconds = 1;
    })
    .await;
    let addr = server.addr();
    let socket = tokio::net::TcpSocket::new_v4().unwrap();
    socket.set_recv_buffer_size(4096).unwrap();
    let mut stalled = socket.connect(addr).await.unwrap();
    let requests = "GET /v1/openapi.json HTTP/1.1\r\nHost: x\r\n\r\n".repeat(64);
    tokio::io::AsyncWriteExt::write_all(&mut stalled, requests.as_bytes())
        .await
        .unwrap();
    // The stalled connection holds the one slot until its write times
    // out; a second client is then served, the first still connected.
    let started = Instant::now();
    let answered = loop {
        let reply = tokio::task::spawn_blocking(move || {
            let mut stream = TcpStream::connect(addr).unwrap();
            stream
                .write_all(b"GET /v1/health HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
                .unwrap();
            read_all(&mut stream, Duration::from_secs(2)).0
        })
        .await
        .unwrap();
        if reply.starts_with(b"HTTP/1.1 204") {
            break true;
        }
        if started.elapsed() > Duration::from_secs(20) {
            break false;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    };
    assert!(answered, "the stalled client kept the only slot");
    assert!(
        started.elapsed() < Duration::from_secs(15),
        "{:?}",
        started.elapsed()
    );
    drop(stalled);
    server.stop().await;
}

/// Review S1: a connection lives at most `server.max_connection_seconds`.
#[tokio::test(flavor = "multi_thread")]
async fn a_connection_lives_at_most_its_lifetime() {
    let mut server = bound(|c| {
        c.server.max_connection_seconds = 1;
        c.server.header_timeout_seconds = 30;
    })
    .await;
    let addr = server.addr();
    tokio::task::spawn_blocking(move || {
        let mut stream = TcpStream::connect(addr).unwrap();
        let start = Instant::now();
        // A request trickled a byte at a time never completes its head
        // before the lifetime ends.
        for byte in b"GET /v1/health HTTP/1.1\r\nHost: x\r\n" {
            if stream.write_all(&[*byte]).is_err() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let (_, closed) = read_all(&mut stream, Duration::from_secs(10));
        assert!(closed);
        assert!(
            start.elapsed() < Duration::from_secs(8),
            "{:?}",
            start.elapsed()
        );
    })
    .await
    .unwrap();
    server.stop().await;
}
