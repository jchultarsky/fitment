//! Slow and hostile clients on a bound socket (docs/research/m6-api.md,
//! section 9.4 and 10.4; VT18): what only the wire shows.

mod support;

use std::io::{Read as _, Write as _};
use std::net::TcpStream;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
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

/// Starts an upload of `length` declared bytes on a raw connection and
/// sends `pieces` of `piece` bytes, `every` apart, until the server
/// answers; returns its answer. A reader thread takes the answer as it
/// comes, before the reset that bytes sent after the server's close
/// draw.
fn trickle(
    addr: std::net::SocketAddr,
    token: &str,
    length: usize,
    piece: usize,
    pieces: usize,
    every: Duration,
) -> Vec<u8> {
    let mut stream = TcpStream::connect(addr).unwrap();
    let head = format!(
        "POST /v1/uploads HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {token}\r\n\
         Content-Type: application/octet-stream\r\nContent-Length: {length}\r\n\r\n"
    );
    stream.write_all(head.as_bytes()).unwrap();
    let answer = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut reading = stream.try_clone().unwrap();
    let sink = std::sync::Arc::clone(&answer);
    #[allow(clippy::disallowed_methods)] // a client's reader, not the server's work
    let reader = std::thread::spawn(move || {
        let mut buf = [0_u8; 4096];
        while let Ok(n) = reading.read(&mut buf) {
            if n == 0 {
                break;
            }
            sink.lock().unwrap().extend_from_slice(&buf[..n]);
        }
    });
    let chunk = vec![b't'; piece];
    for _ in 0..pieces {
        if !answer.lock().unwrap().is_empty() || stream.write_all(&chunk).is_err() {
            break;
        }
        std::thread::sleep(every);
    }
    let started = Instant::now();
    while answer.lock().unwrap().is_empty() && started.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(10));
    }
    // The answer has come, or the wait is over: the connection is done.
    std::thread::sleep(Duration::from_millis(50));
    let _ = stream.shutdown(std::net::Shutdown::Both);
    let _ = reader.join();
    answer.lock().unwrap().clone()
}

/// Waits up to 5 s for the server to hold no upload and no temporary
/// file (the removal runs as the answer is sent).
async fn settled(server: &TestServer) {
    let started = Instant::now();
    loop {
        let idle = server.server().seams().uploads.running(0).0 == 0;
        if idle && server.temporaries().is_empty() {
            return;
        }
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "an upload stayed"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// HX3, UP2 (VT18's upload throughput): an upload trickled below
/// `limits.upload_min_kib_s`, averaged over the window, is cut `408
/// upload_too_slow` once its first window is past, its temporary file
/// removed and its slot free; one just above it completes.
#[tokio::test(flavor = "multi_thread")]
async fn an_upload_below_the_minimum_throughput_is_cut() {
    let mut server = bound(|c| c.limits.upload_window_seconds = 1).await;
    let (addr, token) = (server.addr(), server.token().to_owned());
    let started = Instant::now();
    let slow = tokio::task::spawn_blocking({
        let token = token.clone();
        // 1 KiB every 50 ms: 20 KiB/s, under 64.
        move || trickle(addr, &token, 1 << 20, 1024, 1024, Duration::from_millis(50))
    })
    .await
    .unwrap();
    let text = String::from_utf8_lossy(&slow);
    assert!(text.starts_with("HTTP/1.1 408"), "{text}");
    assert!(text.contains("upload_too_slow"));
    let elapsed = started.elapsed();
    assert!(
        elapsed >= Duration::from_secs(1) && elapsed < Duration::from_secs(8),
        "{elapsed:?}"
    );
    settled(&server).await;
    assert_eq!(server.object_files(), Vec::<String>::new());
    let fast = tokio::task::spawn_blocking(move || {
        // 16 KiB every 100 ms: 160 KiB/s, over 64, for 2 s.
        trickle(
            addr,
            &token,
            20 * 16 * 1024,
            16 * 1024,
            20,
            Duration::from_millis(100),
        )
    })
    .await
    .unwrap();
    let text = String::from_utf8_lossy(&fast);
    assert!(text.starts_with("HTTP/1.1 201"), "{text}");
    settled(&server).await;
    assert_eq!(server.object_files().len(), 1);
    server.stop().await;
}

/// HX3, UP2 (VT18): a body that stops coming is cut after
/// `limits.upload_chunk_seconds` without a chunk, `408 upload_too_slow`,
/// though its throughput window is long; nothing is kept.
#[tokio::test(flavor = "multi_thread")]
async fn a_stalled_upload_is_cut_at_the_chunk_wait() {
    let mut server = bound(|c| {
        c.limits.upload_chunk_seconds = 1;
        c.limits.upload_window_seconds = 300;
    })
    .await;
    let (addr, token) = (server.addr(), server.token().to_owned());
    let started = Instant::now();
    let reply = tokio::task::spawn_blocking(move || {
        trickle(addr, &token, 4096, 100, 1, Duration::from_millis(1))
    })
    .await
    .unwrap();
    let text = String::from_utf8_lossy(&reply);
    assert!(text.starts_with("HTTP/1.1 408"), "{text}");
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
    settled(&server).await;
    assert_eq!(server.object_files(), Vec::<String>::new());
    server.stop().await;
}

/// UP2, VT13's upload half: a client that disconnects mid-upload leaves
/// no object, no temporary file and no slot taken; the server serves the
/// next client.
#[tokio::test(flavor = "multi_thread")]
async fn a_client_gone_mid_upload_leaves_nothing() {
    let mut server = bound(|_| {}).await;
    let (addr, token) = (server.addr(), server.token().to_owned());
    tokio::task::spawn_blocking(move || {
        let mut stream = TcpStream::connect(addr).unwrap();
        let head = format!(
            "PUT /v1/uploads/sha256:{} HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {token}\r\n\
             Content-Type: model/step\r\nContent-Length: 4194304\r\n\r\n",
            "0".repeat(64)
        );
        stream.write_all(head.as_bytes()).unwrap();
        stream.write_all(&vec![b'g'; 1_500_000]).unwrap();
        std::thread::sleep(Duration::from_millis(200));
        drop(stream);
    })
    .await
    .unwrap();
    settled(&server).await;
    assert_eq!(server.object_files(), Vec::<String>::new());
    assert_eq!(server.get("/v1/health").await.status, 204);
    server.stop().await;
}

/// HX1 (VT18): a head trickled one byte every 500 ms, never finished, is
/// cut at the header timeout, nothing served; the trickle does not keep
/// it alive.
#[tokio::test(flavor = "multi_thread")]
async fn a_trickled_head_is_cut_at_the_header_timeout() {
    let mut server = bound(|c| c.server.header_timeout_seconds = 2).await;
    let addr = server.addr();
    let (bytes, closed, elapsed) = tokio::task::spawn_blocking(move || {
        let mut stream = TcpStream::connect(addr).unwrap();
        let mut reading = stream.try_clone().unwrap();
        let start = Instant::now();
        #[allow(clippy::disallowed_methods)] // a client's reader, not the server's work
        let reader = std::thread::spawn(move || {
            let (bytes, closed) = read_all(&mut reading, Duration::from_secs(15));
            (bytes, closed, start.elapsed())
        });
        let head = b"GET /v1/health HTTP/1.1\r\nHost: x\r\nX-Slow: ";
        for byte in head.iter().chain(std::iter::repeat(&b'a')).take(40) {
            if reader.is_finished() || stream.write_all(&[*byte]).is_err() {
                break;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        reader.join().unwrap()
    })
    .await
    .unwrap();
    let answered = String::from_utf8_lossy(&bytes).contains(" 204 ");
    assert!(closed, "the connection stayed open");
    assert!(!answered, "it was served");
    assert!(
        elapsed >= Duration::from_millis(1900) && elapsed < Duration::from_secs(12),
        "{elapsed:?}"
    );
    server.stop().await;
}

/// HX3, UP2 (VT18): an upload's body trickled one byte every 1.5 s, under
/// a chunk wait of 1 s, is cut `408 upload_too_slow` at the chunk wait,
/// though bytes keep coming and its throughput window is long; nothing is
/// kept.
#[tokio::test(flavor = "multi_thread")]
async fn a_trickled_body_is_cut_at_the_chunk_wait() {
    let mut server = bound(|c| {
        c.limits.upload_chunk_seconds = 1;
        c.limits.upload_first_byte_seconds = 1;
        c.limits.upload_window_seconds = 300;
    })
    .await;
    let (addr, token) = (server.addr(), server.token().to_owned());
    let started = Instant::now();
    let reply = tokio::task::spawn_blocking(move || {
        trickle(addr, &token, 4096, 1, 20, Duration::from_millis(1500))
    })
    .await
    .unwrap();
    let text = String::from_utf8_lossy(&reply);
    assert!(text.starts_with("HTTP/1.1 408"), "{text}");
    assert!(text.contains("upload_too_slow"), "{text}");
    assert!(
        started.elapsed() < Duration::from_secs(8),
        "{:?}",
        started.elapsed()
    );
    settled(&server).await;
    assert_eq!(server.object_files(), Vec::<String>::new());
    server.stop().await;
}

/// One slow client of VT18's 256, until `stop`: a head trickled a byte
/// every 500 ms (`head`), or a JSON body trickled so, its length declared
/// and never sent; a connection the server closes is opened again.
async fn slow_client(addr: std::net::SocketAddr, head: bool, token: String, stop: Arc<AtomicBool>) {
    use tokio::io::AsyncWriteExt as _;
    while !stop.load(Ordering::SeqCst) {
        let Ok(mut stream) = tokio::net::TcpStream::connect(addr).await else {
            tokio::time::sleep(Duration::from_millis(100)).await;
            continue;
        };
        let opening = if head {
            "GET /v1/health HTTP/1.1\r\nHost: x\r\nX-Slow: ".to_owned()
        } else {
            format!(
                "POST /v1/features HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {token}\r\n\
                 Content-Type: application/json\r\nContent-Length: 100000\r\n\r\n{{\"upload\": \""
            )
        };
        if stream.write_all(opening.as_bytes()).await.is_err() {
            continue;
        }
        while !stop.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(500)).await;
            if stream.write_all(b"a").await.is_err() {
                break;
            }
        }
    }
}

/// A match job of `body`, waited for: how long it took.
async fn timed_match(server: &TestServer, body: &serde_json::Value) -> Duration {
    let started = Instant::now();
    let job = server.run_match(body).await;
    assert_eq!(job["state"], "succeeded", "{job}");
    started.elapsed()
}

/// HX1, HX3 (VT18): 256 slow clients at once (half trickling heads, half
/// trickling JSON bodies, each reconnecting when cut) do not delay a
/// normal client's match job beyond its time alone, doubled, plus 2 s:
/// the slack stated, since the slow clients hold connections, not
/// workers.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn slow_clients_do_not_delay_a_normal_one() {
    let mut server = bound(|c| c.auth.rate_burst = 10_000).await;
    let socket = server.socket_of("s01_bolted", "BRACKET").await;
    let sha = server.upload(&support::candidate("m_bracket.stp")).await;
    let body = serde_json::json!({
        "socket": socket,
        "candidates": { "files": [{ "upload": format!("sha256:{sha}"), "label": "m_bracket.stp" }] },
    });
    let mut alone = Duration::ZERO;
    for _ in 0..3 {
        alone = alone.max(timed_match(&server, &body).await);
    }
    let stop = Arc::new(AtomicBool::new(false));
    let token = server.token().to_owned();
    let clients: Vec<_> = (0..256)
        .map(|n| {
            tokio::spawn(slow_client(
                server.addr(),
                n % 2 == 0,
                token.clone(),
                Arc::clone(&stop),
            ))
        })
        .collect();
    tokio::time::sleep(Duration::from_secs(2)).await;
    let mut loaded = Duration::ZERO;
    for _ in 0..3 {
        loaded = loaded.max(timed_match(&server, &body).await);
    }
    stop.store(true, Ordering::SeqCst);
    for client in clients {
        let _ = client.await;
    }
    eprintln!("VT18: a match alone in {alone:?}, beside 256 slow clients in {loaded:?}");
    assert!(
        loaded <= alone * 2 + Duration::from_secs(2),
        "alone {alone:?}, loaded {loaded:?}"
    );
    server.stop().await;
}
