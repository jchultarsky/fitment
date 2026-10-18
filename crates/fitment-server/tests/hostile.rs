//! Hostile requests in process (docs/research/m6-api.md, section 9.4 and
//! 10.4): each bound of `M6_LIMITS` that needs no wire, exceeded; auth's
//! (AU1, AU7, AU10) among them, VT24's limit cases.

mod support;

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use axum::http::Method;
use fitment_server::auth::Role;
use fitment_server::hooks::Action;
use serde_json::json;
use support::{Call, Dirs, TestServer, post, seed_token, seed_user};

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

/// Open question 2 of PR 2's review: at most four `/v1/info` requests
/// read the catalog at once until PR 7's reader pool; a fifth waits for a
/// slot within the handler's time.
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

/// AU1, fix 7: a password is 12 to 256 characters (not bytes), at most
/// 1,024 bytes; one outside is `400 invalid_parameter` before any hash, at
/// user creation and at a password change, and a login with one is a
/// failure that hashes nothing.
#[tokio::test]
async fn passwords_past_their_bounds_are_refused_before_hashing() {
    let server = TestServer::fresh();
    let admin = server.token().to_owned();
    let hashes = || server.server().seams().auth.hashes();
    let mut n = 0;
    let mut create = |password: String| {
        n += 1;
        support::password(&password);
        post(
            "/v1/auth/users",
            &admin,
            &json!({ "username": format!("u{n}"), "role": "reader", "password": password }),
        )
    };
    for (password, ok) in [
        ("a".repeat(11), false),
        ("é".repeat(6), false),
        ("a".repeat(12), true),
        ("é".repeat(12), true),
        ("a".repeat(256), true),
        ("\u{1F600}".repeat(256), true),
        ("a".repeat(257), false),
        ("a".repeat(5000), false),
    ] {
        let before = hashes();
        let reply = server.send(create(password.clone())).await;
        if ok {
            assert_eq!(reply.status, 201, "{} characters", password.chars().count());
            assert_eq!(hashes(), before + 1);
        } else {
            assert_eq!(
                reply.code(),
                "invalid_parameter",
                "{} characters",
                password.chars().count()
            );
            assert_eq!(hashes(), before, "hashed a refused password");
        }
    }
    let before = hashes();
    let long = support::password(&"b".repeat(2000));
    let login = json!({ "username": "u3", "password": long });
    let reply = server
        .send(Call::json(Method::POST, "/v1/auth/login", &login))
        .await;
    assert_eq!(reply.code(), "invalid_credentials");
    assert_eq!(hashes(), before);
    let change = json!({ "current": "a".repeat(12), "new": "a".repeat(11) });
    let user = seed_user(server.server(), "chg", Role::Reader, Some(&"a".repeat(12)));
    let (_, token) = seed_token(server.server(), &user, Role::Reader, 30);
    let reply = server
        .send(post("/v1/auth/password", &token, &change))
        .await;
    assert_eq!(reply.code(), "invalid_parameter");
    assert_eq!(reply.json()["details"]["parameter"], "new");
}

/// AU7, M6-61 (VT24): requests are counted per user over all their
/// tokens, `auth.rate_burst` (100) at once and `auth.rate_per_second`
/// (50) after: twenty tokens of one user share one bucket, and another
/// user has their own.
#[tokio::test]
async fn the_request_rate_is_per_user_over_all_their_tokens() {
    let server = TestServer::fresh();
    let ada = seed_user(server.server(), "ada", Role::Reader, None);
    let tokens: Vec<String> = (0..20)
        .map(|_| seed_token(server.server(), &ada, Role::Reader, 30).1)
        .collect();
    let bob = seed_user(server.server(), "bob", Role::Reader, None);
    let (_, bobs) = seed_token(server.server(), &bob, Role::Reader, 30);
    let clock = &server.server().seams().auth.clock;
    clock.freeze();
    for i in 0..100 {
        let reply = server
            .send(Call::get("/v1/auth/me").bearer(&tokens[i % 20]))
            .await;
        assert_eq!(reply.status, 200, "request {i}");
    }
    for token in &tokens {
        let reply = server.send(Call::get("/v1/auth/me").bearer(token)).await;
        assert_eq!(reply.code(), "rate_limited");
        assert_eq!(reply.header("retry-after"), Some("1"));
        assert_eq!(reply.json()["retryable"], true);
    }
    assert_eq!(
        server
            .send(Call::get("/v1/auth/me").bearer(&bobs))
            .await
            .status,
        200
    );
    clock.advance(Duration::from_millis(20));
    assert_eq!(
        server
            .send(Call::get("/v1/auth/me").bearer(&tokens[0]))
            .await
            .status,
        200
    );
    assert_eq!(
        server
            .send(Call::get("/v1/auth/me").bearer(&tokens[1]))
            .await
            .code(),
        "rate_limited"
    );
}

fn at(address: &str) -> SocketAddr {
    format!("{address}:40000").parse().unwrap()
}

fn login(username: &str, password: &str, from: &str) -> Call {
    let body = json!({ "username": username, "password": password });
    Call::json(Method::POST, "/v1/auth/login", &body).peer(at(from))
}

/// AU7, M6-61 (VT24): ten failures per (folded username, address) per 15
/// minutes, then even the right password is `429` from that address; the
/// same username logs in from another, so no one locks an admin out; an
/// unknown username meets the same answers and limits; thirty attempts
/// per address per minute, checked first.
#[tokio::test]
async fn logins_are_limited_per_pair_and_per_address() {
    let server = TestServer::fresh();
    let password = support::password("ada's right password");
    seed_user(server.server(), "ada", Role::Admin, Some(&password));
    let wrong = support::password("a wrong password here");
    let clock = &server.server().seams().auth.clock;
    clock.freeze();
    for username in ["ada", "nobody"] {
        for i in 0..10 {
            let reply = server.send(login(username, &wrong, "198.51.100.1")).await;
            assert_eq!(reply.code(), "invalid_credentials", "{username} {i}");
        }
        let refused = server
            .send(login(username, &password, "198.51.100.1"))
            .await;
        assert_eq!(refused.code(), "rate_limited", "{username}");
        assert_eq!(refused.header("retry-after"), Some("90"));
    }
    let elsewhere = server.send(login("ADA", &password, "198.51.100.2")).await;
    assert_eq!(
        elsewhere.status, 201,
        "the same username from another address"
    );
    for i in 0..29 {
        let reply = server
            .send(login(&format!("user{i}"), "short", "198.51.100.3"))
            .await;
        assert_eq!(reply.code(), "invalid_credentials");
    }
    assert_eq!(
        server
            .send(login("ada", &password, "198.51.100.3"))
            .await
            .status,
        201
    );
    let flood = server.send(login("ada", &password, "198.51.100.3")).await;
    assert_eq!(
        flood.code(),
        "rate_limited",
        "the address's thirty a minute"
    );
    assert_eq!(flood.header("retry-after"), Some("2"));
    clock.advance(Duration::from_secs(90));
    assert_eq!(
        server
            .send(login("ada", &password, "198.51.100.1"))
            .await
            .status,
        201
    );
}

/// AU7: behind a trusted proxy the client is the right-most address of
/// `X-Forwarded-For` that is not a proxy; the buckets are bounded by
/// `auth.login_buckets`, the least recently used evicted, for 10⁵
/// usernames (the limiter itself) as for logins over HTTP.
#[tokio::test]
async fn login_buckets_are_keyed_by_the_client_and_bounded() {
    let dirs = Dirs::new();
    let mut config = dirs.config();
    config.server.trusted_proxies = vec!["10.0.0.1".into(), "10.0.0.2".into()];
    config.auth.login_buckets = 64;
    let server = TestServer::in_process(dirs, config);
    let clock = &server.server().seams().auth.clock;
    clock.freeze();
    let through = |username: &str, forwarded: &str| {
        login(username, "short", "10.0.0.1").header("x-forwarded-for", forwarded)
    };
    for i in 0..30 {
        let reply = server
            .send(through(&format!("x{i}"), "203.0.113.9, 10.0.0.2"))
            .await;
        assert_eq!(reply.code(), "invalid_credentials");
    }
    let reply = server.send(through("y", "203.0.113.9")).await;
    assert_eq!(
        reply.code(),
        "rate_limited",
        "keyed by the forwarded client"
    );
    let reply = server.send(through("y", "203.0.113.10")).await;
    assert_eq!(
        reply.code(),
        "invalid_credentials",
        "another client behind the proxy"
    );
    let spoofed = server.send(through("y", "203.0.113.9, garbage")).await;
    assert_eq!(
        spoofed.code(),
        "invalid_credentials",
        "an unreadable entry: the proxy's address"
    );
    for i in 0..500 {
        let reply = server
            .send(through(
                &format!("user{i}"),
                &format!("192.0.2.{}", i % 250),
            ))
            .await;
        assert_ne!(reply.status, 500);
    }
    assert!(server.server().seams().auth.login_buckets() <= 64);
    let limits = fitment_server::auth::limit::LoginLimits::new(
        fitment_server::auth::limit::Rate::per(30, 60_000),
        fitment_server::auth::limit::Rate::per(10, 900_000),
        10_000,
    );
    for i in 0..100_000_u32 {
        let address = std::net::IpAddr::from(std::net::Ipv4Addr::from(0x0a00_0000 + i % 5000));
        let user = fitment_server::auth::login_key(&format!("user{i}"));
        if let Ok(attempt) = limits.admit(&user, address, u64::from(i)) {
            attempt.failed();
        }
    }
    assert!(limits.len() <= 10_000, "{}", limits.len());
}

/// AU10, review F1 (VT24): a minted token's role is at most the presenting
/// token's effective role and its expiry at most the presenting token's;
/// asking more is `400 invalid_parameter` and mints nothing, never a
/// silent cap; a reader token minted from a writer's login cannot mint a
/// writer token.
#[tokio::test]
async fn a_token_mints_only_within_itself() {
    let server = TestServer::fresh();
    let password = support::password("the writer's password");
    seed_user(server.server(), "wes", Role::Writer, Some(&password));
    let body = json!({ "username": "wes", "password": password });
    let login = server
        .send(Call::json(Method::POST, "/v1/auth/login", &body))
        .await
        .json();
    let writer = login["secret"].as_str().unwrap().to_owned();
    let count = || -> i64 {
        let db = rusqlite::Connection::open(server.dirs.state.join("server.db")).unwrap();
        db.query_row("SELECT count(*) FROM token", [], |r| r.get(0))
            .unwrap()
    };
    let mint = |token: &str, body: serde_json::Value| post("/v1/auth/tokens", token, &body);
    let before = count();
    let reply = server
        .send(mint(&writer, json!({ "name": "x", "role": "admin" })))
        .await;
    assert_eq!(
        (reply.code(), reply.json()["details"]["parameter"].clone()),
        ("invalid_parameter".into(), json!("role"))
    );
    let reader = server
        .send(mint(
            &writer,
            json!({ "name": "ci", "role": "reader", "days": 7 }),
        ))
        .await
        .json();
    assert_eq!(reader["parent"], login["id"]);
    let reader = reader["secret"].as_str().unwrap().to_owned();
    for (body, field) in [
        (json!({ "name": "x", "role": "writer" }), "role"),
        (json!({ "name": "x", "days": 8 }), "days"),
        (json!({ "name": "x", "days": 366 }), "days"),
        (json!({ "name": "x", "days": 0 }), "days"),
        (json!({ "name": "" }), "name"),
    ] {
        let reply = server.send(mint(&reader, body.clone())).await;
        assert_eq!(reply.code(), "invalid_parameter", "{body}");
        assert_eq!(reply.json()["details"]["parameter"], field, "{body}");
    }
    assert_eq!(count(), before + 1, "a refused mint minted nothing");
    let at_most = server
        .send(mint(&reader, json!({ "name": "y" })))
        .await
        .json();
    let parent_expires = server
        .send(Call::get("/v1/auth/tokens").bearer(&reader))
        .await
        .json()["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "ci")
        .unwrap()["expires"]
        .clone();
    assert_eq!(
        at_most["expires"], parent_expires,
        "the parent's expiry, not 90 days"
    );
    assert_eq!(at_most["role"], "reader");
    let same_day = server
        .send(mint(&reader, json!({ "name": "z", "days": 7 })))
        .await;
    assert_eq!(same_day.status, 201, "an expiry equal to the parent's");
}

/// AU10, M6-61 (VT24): a user holds at most `auth.max_live_tokens` (20)
/// live tokens: a 21st by minting is `409 too_many_tokens`, and by login it
/// evicts the oldest live root token (review); expired and revoked ones do
/// not count.
#[tokio::test]
async fn the_live_token_cap_holds_without_locking_anyone_out() {
    let server = TestServer::fresh();
    let password = support::password("the capped password");
    let ada = seed_user(server.server(), "ada", Role::Reader, Some(&password));
    seed_token(server.server(), &ada, Role::Reader, -1);
    let (revoked, _) = seed_token(server.server(), &ada, Role::Reader, 30);
    let seams = server.server().seams();
    seams.db.revoke(revoked.key, seams.ids.now_ms()).unwrap();
    let live: Vec<String> = (0..19)
        .map(|_| seed_token(server.server(), &ada, Role::Reader, 30).1)
        .collect();
    let body = json!({ "username": "ada", "password": password });
    let login = || Call::json(Method::POST, "/v1/auth/login", &body);
    let twentieth = server.send(login()).await.json();
    assert_eq!(twentieth["id"].as_str().map(|_| ()), Some(()), "the 20th");
    // Review: at the cap a login evicts the oldest live root token, so the
    // cap never locks its user out; the 21st login is in, live[0] is out.
    let reply = server.send(login()).await;
    assert_eq!(reply.status, 201, "the 21st login evicts, not refuses");
    let me = server.send(Call::get("/v1/auth/me").bearer(&live[0])).await;
    assert_eq!(
        me.code(),
        "unauthenticated",
        "the oldest root token was evicted"
    );
    let reply = server
        .send(post(
            "/v1/auth/tokens",
            &live[1],
            &json!({ "name": "one more" }),
        ))
        .await;
    assert_eq!(
        reply.code(),
        "too_many_tokens",
        "a mint at the cap is refused"
    );
    assert_eq!(reply.status, 409);
    let logout = Call::new(Method::DELETE, "/v1/auth/tokens/current").bearer(&live[2]);
    assert_eq!(server.send(logout).await.status, 204);
    let reply = server
        .send(post(
            "/v1/auth/tokens",
            &live[1],
            &json!({ "name": "one more" }),
        ))
        .await;
    assert_eq!(reply.status, 201);
}

/// AU7 (review): the pair's failure is taken before the hash, so thirty
/// concurrent wrong guesses for one username from one address verify
/// exactly ten and the rest are `429`, however they interleave.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_wrong_guesses_cannot_pass_the_pair_limit() {
    let server = std::sync::Arc::new(TestServer::fresh());
    let password = support::password("the guessed password");
    seed_user(server.server(), "ada", Role::Reader, Some(&password));
    server.server().seams().auth.clock.freeze();
    let wrong = support::password("a wrong guess, again");
    let before = server.server().seams().auth.hashes();
    let mut guesses = tokio::task::JoinSet::new();
    for _ in 0..30 {
        let (server, wrong) = (std::sync::Arc::clone(&server), wrong.clone());
        guesses.spawn(async move { server.send(login("ada", &wrong, "198.51.100.7")).await });
    }
    let mut codes = Vec::new();
    while let Some(reply) = guesses.join_next().await {
        codes.push(reply.unwrap().code());
    }
    let verified = codes.iter().filter(|c| *c == "invalid_credentials").count();
    let limited = codes.iter().filter(|c| *c == "rate_limited").count();
    assert_eq!((verified, limited), (10, 20), "{codes:?}");
    assert_eq!(server.server().seams().auth.hashes(), before + 10);
}

/// AU7 (review): a success and an exit that never judged the credentials
/// refund the pair's failure; an IPv6 client is keyed by its /64.
#[tokio::test]
async fn successes_refund_and_ipv6_is_keyed_by_its_64() {
    let server = TestServer::fresh();
    let password = support::password("ada's refunded password");
    seed_user(server.server(), "ada", Role::Reader, Some(&password));
    let wrong = support::password("a wrong password, refunded");
    server.server().seams().auth.clock.freeze();
    for _ in 0..9 {
        let reply = server.send(login("ada", &wrong, "198.51.100.8")).await;
        assert_eq!(reply.code(), "invalid_credentials");
    }
    for _ in 0..3 {
        assert_eq!(
            server
                .send(login("ada", &password, "198.51.100.8"))
                .await
                .status,
            201
        );
    }
    let tenth = server.send(login("ada", &wrong, "198.51.100.8")).await;
    assert_eq!(
        tenth.code(),
        "invalid_credentials",
        "successes cost no failure"
    );
    let limited = server.send(login("ada", &password, "198.51.100.8")).await;
    assert_eq!(limited.code(), "rate_limited");
    let six = |host: &str| SocketAddr::new(host.parse().unwrap(), 40000);
    for i in 0..10 {
        let call = login("ada", &wrong, "198.51.100.9").peer(six(&format!("2001:db8:a:b::{i:x}")));
        assert_eq!(server.send(call).await.code(), "invalid_credentials");
    }
    let same = login("ada", &password, "198.51.100.9").peer(six("2001:db8:a:b:ffff::1"));
    assert_eq!(
        server.send(same).await.code(),
        "rate_limited",
        "the same /64"
    );
    let other = login("ada", &password, "198.51.100.9").peer(six("2001:db8:a:c::1"));
    assert_eq!(server.send(other).await.status, 201, "another /64");
}

/// AU10 (review): a user mints at most `auth.mints_per_minute` (10)
/// tokens a minute, so dead tokens cannot pile up faster than the sweep.
#[tokio::test]
async fn minting_is_limited_per_user() {
    let server = TestServer::fresh();
    let ada = seed_user(server.server(), "ada", Role::Reader, None);
    let (_, token) = seed_token(server.server(), &ada, Role::Reader, 30);
    let clock = &server.server().seams().auth.clock;
    clock.freeze();
    for i in 0..10 {
        let reply = server
            .send(post(
                "/v1/auth/tokens",
                &token,
                &json!({ "name": format!("m{i}") }),
            ))
            .await;
        assert_eq!(reply.status, 201, "mint {i}");
        let secret = reply.json()["secret"].as_str().unwrap().to_owned();
        let revoke = Call::new(Method::DELETE, "/v1/auth/tokens/current").bearer(&secret);
        assert_eq!(server.send(revoke).await.status, 204);
    }
    let reply = server
        .send(post("/v1/auth/tokens", &token, &json!({ "name": "m10" })))
        .await;
    assert_eq!(reply.code(), "rate_limited");
    assert_eq!(reply.header("retry-after"), Some("6"));
    clock.advance(Duration::from_secs(6));
    let reply = server
        .send(post("/v1/auth/tokens", &token, &json!({ "name": "m11" })))
        .await;
    assert_eq!(reply.status, 201);
}

/// UP2, VT17: the upload cap. A declared `Content-Length` past it is `413
/// too_large` before a byte is read (no chunk of the body taken); a body
/// at the cap is kept; one streamed 100 times past it without a length
/// is cut after the cap and one chunk, its temporary file removed and
/// nothing kept.
#[tokio::test]
async fn an_upload_past_the_cap_is_refused_before_or_as_it_streams() {
    let dirs = Dirs::new();
    let mut config = dirs.config();
    config.limits.max_upload_mib = 1;
    let server = TestServer::in_process(dirs, config);
    let cap = 1024 * 1024;
    let at_cap = vec![b'c'; cap];
    let sha256 = fitment_core::sha256_hex(&at_cap);
    let path = format!("/v1/uploads/sha256:{sha256}");
    let declared = Call::new(Method::PUT, &path)
        .header("content-type", "application/octet-stream")
        .header("content-length", &(cap + 1).to_string());
    let (body, taken) = support::endless(100, 64 * 1024);
    let reply = server.send_body(declared, body).await;
    assert_eq!(
        (reply.status.as_u16(), reply.code()),
        (413, "too_large".into())
    );
    assert_eq!(reply.json()["details"]["max_bytes"], cap);
    assert_eq!(
        taken.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "no byte read"
    );
    assert_eq!(server.send(support::put_upload(&at_cap)).await.status, 201);
    let chunk = 64 * 1024;
    let (body, taken) = support::endless(100 * cap / chunk, chunk);
    let streamed =
        Call::new(Method::POST, "/v1/uploads").header("content-type", "application/octet-stream");
    let reply = server.send_body(streamed, body).await;
    assert_eq!(reply.code(), "too_large");
    let read = taken.load(std::sync::atomic::Ordering::SeqCst);
    assert_eq!(read, cap / chunk + 1, "the cap and one chunk, then cut");
    assert!(
        server.temporaries().is_empty(),
        "the temporary file is removed"
    );
    assert_eq!(server.object_files(), [sha256], "nothing else kept");
    // An upload route takes no JSON cap: a 2 MiB JSON body under a 3 MiB
    // upload cap is bytes.
    let dirs = Dirs::new();
    let mut config = dirs.config();
    config.limits.max_upload_mib = 3;
    config.limits.max_json_kib = 1;
    let server = TestServer::in_process(dirs, config);
    let json = format!("[{}0]", "0,".repeat(1024 * 1024));
    let reply = server
        .send(support::put_upload(json.as_bytes()).header("content-type", "application/json"))
        .await;
    assert_eq!(reply.status, 201);
}

/// UP4, VT24's per-user upload cases: at most
/// `limits.upload_concurrency_per_user` uploads at once per user, over
/// all their tokens, and `limits.upload_concurrency` per server: past
/// either `429 too_many_streams`, before a byte is read, retryable; a
/// slot is free again when its upload ends.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn uploads_at_once_are_capped_per_user_and_per_server() {
    let dirs = Dirs::new();
    let mut config = dirs.config();
    config.limits.upload_concurrency = 3;
    config.limits.upload_concurrency_per_user = 2;
    let server = std::sync::Arc::new(TestServer::in_process(dirs, config));
    let ada = seed_user(server.server(), "ada", Role::Reader, None);
    let bob = seed_user(server.server(), "bob", Role::Reader, None);
    let eve = seed_user(server.server(), "eve", Role::Reader, None);
    let tokens: Vec<String> = (0..3)
        .map(|_| seed_token(server.server(), &ada, Role::Reader, 30).1)
        .collect();
    let (_, bob_token) = seed_token(server.server(), &bob, Role::Reader, 30);
    let (_, eve_token) = seed_token(server.server(), &eve, Role::Reader, 30);
    let start = |token: String| {
        let (body, tx, _) = support::fed();
        let server = std::sync::Arc::clone(&server);
        let call = Call::new(Method::POST, "/v1/uploads")
            .header("content-type", "model/step")
            .bearer(&token);
        let task = tokio::spawn(async move { server.send_body(call, body).await });
        (task, tx)
    };
    let running = |user: i64| server.server().seams().uploads.running(user);
    let wait_for = |user: i64, n: usize| async move {
        let started = Instant::now();
        while running(user).1 < n {
            assert!(started.elapsed() < Duration::from_secs(10), "no slot taken");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    };
    let first = start(tokens[0].clone());
    let second = start(tokens[1].clone());
    wait_for(ada.key, 2).await;
    let third = Call::new(Method::POST, "/v1/uploads")
        .header("content-type", "model/step")
        .body(b"x")
        .bearer(&tokens[2]);
    let refused = server.send(third.clone()).await;
    assert_eq!(
        (refused.status.as_u16(), refused.code()),
        (429, "too_many_streams".into())
    );
    assert_eq!(
        refused.json()["details"]["limit"],
        "upload_concurrency_per_user"
    );
    assert_eq!(refused.json()["retryable"], true);
    assert!(refused.header("retry-after").is_some());
    let bobs = start(bob_token.clone());
    wait_for(bob.key, 1).await;
    let eves = Call::new(Method::POST, "/v1/uploads")
        .header("content-type", "model/step")
        .body(b"y")
        .bearer(&eve_token);
    let full = server.send(eves.clone()).await;
    assert_eq!(full.json()["details"]["limit"], "upload_concurrency");
    for (task, tx) in [first, second, bobs] {
        tx.send(axum::body::Bytes::from_static(b"some bytes"))
            .await
            .unwrap();
        drop(tx);
        assert!(task.await.unwrap().status.is_success());
    }
    assert_eq!(running(ada.key), (0, 0));
    assert_eq!(server.send(third).await.status, 201);
    assert_eq!(server.send(eves).await.status, 201);
}

/// One GiB, the least quota a key sets.
const GIB: u64 = 1024 * 1024 * 1024;

/// A server whose upload cap passes its 1 GiB quotas, and `n` readers of
/// it with their tokens.
fn quota_world(n: usize) -> (std::sync::Arc<TestServer>, Vec<(i64, String)>) {
    let dirs = Dirs::new();
    let mut config = dirs.config();
    config.limits.max_upload_mib = 4096;
    config.limits.user_quota_gib = 1;
    config.limits.staging_gib = 1;
    let server = std::sync::Arc::new(TestServer::in_process(dirs, config));
    let users = (0..n)
        .map(|i| {
            let user = seed_user(server.server(), &format!("user{i}"), Role::Reader, None);
            let (_, token) = seed_token(server.server(), &user, Role::Reader, 30);
            (user.key, token)
        })
        .collect();
    (server, users)
}

/// A row of `owner` standing for an upload of `bytes` bytes (no file): a
/// holding the quota counts.
fn hold(server: &TestServer, owner: i64, name: char, bytes: u64) {
    let conn = rusqlite::Connection::open(server.dirs.state.join("server.db")).unwrap();
    let sha256 = name.to_string().repeat(64);
    conn.execute(
        "INSERT INTO object (sha256, bytes, added_ms) VALUES (?1, ?2, 0)",
        rusqlite::params![sha256, i64::try_from(bytes).unwrap()],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO upload (owner, sha256, created_ms, expires_ms) VALUES (?1, ?2, 0, ?3)",
        rusqlite::params![owner, sha256, i64::MAX],
    )
    .unwrap();
}

fn post_as(token: &str) -> Call {
    Call::new(Method::POST, "/v1/uploads")
        .header("content-type", "application/octet-stream")
        .bearer(token)
}

/// UP4: the per-user quota counts the caller's holdings, each charged
/// `max(bytes, 4096) + 512` (M4 of PR 4's review), before a byte is read
/// for a declared length and as the body streams without one; past it,
/// `507 quota_exceeded`, nothing kept.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn quotas_are_checked_before_bytes_and_as_they_stream() {
    let (server, users) = quota_world(2);
    let ((ada, ada_token), (_, bob_token)) = (users[0].clone(), users[1].clone());
    // Ada's holding leaves room for less than one small upload's charge.
    hold(&server, ada, 'a', GIB - 5000);
    let declared = post_as(&ada_token).header("content-length", "20");
    let (body, taken) = support::endless(1, 20);
    let reply = server.send_body(declared, body).await;
    assert_eq!(
        (reply.status.as_u16(), reply.code()),
        (507, "quota_exceeded".into())
    );
    assert_eq!(reply.json()["details"]["limit"], "user_quota_gib");
    assert_eq!(
        taken.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "before a byte"
    );
    let (body, _) = support::endless(2, 8);
    let streamed = server.send_body(post_as(&ada_token), body).await;
    assert_eq!(
        streamed.json()["details"]["limit"],
        "user_quota_gib",
        "as it streams"
    );
    assert_eq!(
        server
            .send(post_as(&bob_token).body(b"bob's bytes"))
            .await
            .status,
        201
    );
    let seams = server.server().seams();
    let charged = seams.db.charged_bytes(ada, seams.ids.now_ms()).unwrap();
    assert_eq!(
        charged,
        GIB - 5000 + 512,
        "a large object: its bytes and its row"
    );
    assert_eq!(server.temporaries(), Vec::<std::path::PathBuf>::new());
}

/// UP4: the staging quota counts the bytes of open temporary files, a
/// declared length reserved up front and streamed bytes past it; past it,
/// `507 quota_exceeded`, and the bytes are released with the file.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_staging_quota_counts_open_temporary_files() {
    let (server, users) = quota_world(2);
    let ((_, dan), (_, cara)) = (users[0].clone(), users[1].clone());
    let (body, tx, _) = support::fed();
    let holder = {
        let server = std::sync::Arc::clone(&server);
        let call = post_as(&dan).header("content-length", &(GIB - 6000).to_string());
        tokio::spawn(async move { server.send_body(call, body).await })
    };
    let started = Instant::now();
    while server.server().seams().uploads.staged() < GIB - 6000 {
        assert!(started.elapsed() < Duration::from_secs(10));
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let declared = server.send(post_as(&cara).body(&[b'z'; 7000])).await;
    assert_eq!(declared.json()["details"]["limit"], "staging_gib");
    let (body, _) = support::endless(8, 1000);
    let streamed = server.send_body(post_as(&cara), body).await;
    assert_eq!(
        streamed.json()["details"]["limit"],
        "staging_gib",
        "as it streams"
    );
    drop(tx);
    let _ = holder.await.unwrap();
    assert_eq!(server.server().seams().uploads.staged(), 0, "released");
    assert_eq!(server.temporaries(), Vec::<std::path::PathBuf>::new());
    assert_eq!(
        server.send(post_as(&cara).body(&[b'z'; 7000])).await.status,
        201
    );
}

/// UP4, L1 of PR 4's review: two uploads of one user at once share one
/// quota (the first's charge counts while it runs), and a `PUT` of bytes
/// the caller holds is not charged again: at the quota it still extends
/// their lifetime.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn uploads_at_once_share_their_users_quota() {
    let (server, users) = quota_world(1);
    let (ada, token) = users[0].clone();
    let (body, tx, _) = support::fed();
    let first = {
        let server = std::sync::Arc::clone(&server);
        let call = post_as(&token).header("content-length", &(GIB - 10_000).to_string());
        tokio::spawn(async move { server.send_body(call, body).await })
    };
    let started = Instant::now();
    while server.server().seams().uploads.charged(ada) == 0 {
        assert!(started.elapsed() < Duration::from_secs(10));
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let second = server.send(post_as(&token).body(&[b's'; 10_000])).await;
    assert_eq!(second.json()["details"]["limit"], "user_quota_gib");
    drop(tx);
    let _ = first.await.unwrap();
    assert_eq!(server.server().seams().uploads.charged(ada), 0, "released");
    // At the quota, her own bytes again are only a lifetime extended.
    assert_eq!(
        server
            .send(support::put_upload(b"ada's own").bearer(&token))
            .await
            .status,
        201
    );
    hold(&server, ada, 'b', GIB);
    let refused = server
        .send(support::put_upload(b"new bytes").bearer(&token))
        .await;
    assert_eq!(refused.code(), "quota_exceeded");
    let again = server
        .send(support::put_upload(b"ada's own").bearer(&token))
        .await;
    assert_eq!(again.status, 200, "held bytes are not charged again");
}

/// UP4, M4 of PR 4's review: a user holds at most
/// `limits.user_max_uploads` live uploads, so tiny ones cannot use the
/// disk's inodes up within a quota of bytes: one more is `507
/// quota_exceeded` (`details.limit` `user_max_uploads`), before a byte is
/// read; a tiny object is charged 4,096 bytes and its row's 512.
#[tokio::test]
async fn a_user_holds_a_bounded_number_of_uploads() {
    let dirs = Dirs::new();
    let mut config = dirs.config();
    config.limits.user_max_uploads = 3;
    let server = TestServer::in_process(dirs, config);
    let ada = seed_user(server.server(), "ada", Role::Reader, None);
    let (_, token) = seed_token(server.server(), &ada, Role::Reader, 30);
    for i in 0..3 {
        let bytes = format!("tiny {i}");
        assert_eq!(
            server
                .send(support::put_upload(bytes.as_bytes()).bearer(&token))
                .await
                .status,
            201
        );
    }
    let (body, chunks) = support::endless(1, 5);
    let call = post_as(&token).header("content-length", "5");
    let refused = server.send_body(call, body).await;
    assert_eq!(
        (refused.status.as_u16(), refused.code()),
        (507, "quota_exceeded".into())
    );
    assert_eq!(refused.json()["details"]["limit"], "user_max_uploads");
    assert_eq!(chunks.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert_eq!(
        server
            .send(support::put_upload(b"tiny 0").bearer(&token))
            .await
            .status,
        200
    );
    let seams = server.server().seams();
    let charged = seams.db.charged_bytes(ada.key, seams.ids.now_ms()).unwrap();
    assert_eq!(charged, 3 * (4096 + 512));
}

/// HX3, UP2 (M3 of PR 4's review): an upload's first bytes come within
/// `limits.upload_first_byte_seconds`, or it is cut `408
/// upload_too_slow`; from then until its window has passed, it owes the
/// minimum for every second it ran, so a trickle of a few bytes cannot
/// hold a slot for the window; a body at the minimum completes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_upload_owes_its_first_bytes_and_then_its_rate() {
    let dirs = Dirs::new();
    let mut config = dirs.config();
    config.limits.upload_first_byte_seconds = 1;
    config.limits.upload_window_seconds = 30;
    let server = TestServer::in_process(dirs, config);
    let started = Instant::now();
    let (body, tx, _) = support::fed();
    let silent = server.send_body(post_as(server.token()), body).await;
    assert_eq!(silent.code(), "upload_too_slow");
    assert_eq!(silent.json()["details"]["received"], 0);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
    drop(tx);
    let started = Instant::now();
    let (body, tx, _) = support::fed();
    tx.send(axum::body::Bytes::from_static(b"a few bytes"))
        .await
        .unwrap();
    let trickle = server.send_body(post_as(server.token()), body).await;
    assert_eq!(trickle.code(), "upload_too_slow");
    assert_eq!(trickle.json()["details"]["received"], 11);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "cut long before its 30 s window"
    );
    drop(tx);
    let (body, tx, _) = support::fed();
    let sender = tokio::spawn(async move {
        for _ in 0..15 {
            tx.send(axum::body::Bytes::from(vec![b'r'; 16 * 1024]))
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    });
    let paced = server.send_body(post_as(server.token()), body).await;
    sender.await.unwrap();
    assert_eq!(paced.status, 201, "160 KiB/s for 1.5 s, over the 64");
    assert_eq!(server.temporaries(), Vec::<std::path::PathBuf>::new());
}

/// HX3, UP2 (H1 of PR 4's review): an upload of tiny frames keeps about 64
/// throughput samples a window, not one a frame, so its memory stays flat
/// however many frames it sends.
#[tokio::test]
async fn tiny_frames_keep_the_samples_bounded() {
    let server = TestServer::fresh();
    let (body, taken) = support::endless(100_000, 1);
    let reply = server.send_body(post_as(server.token()), body).await;
    assert_eq!(reply.status, 201);
    assert_eq!(reply.json()["bytes"], 100_000);
    assert_eq!(taken.load(std::sync::atomic::Ordering::SeqCst), 100_000);
    let most = server.server().seams().uploads.most_samples();
    assert!(
        (1..=70).contains(&most),
        "{most} samples for 100,000 frames"
    );
}

/// VN3, VT42's pages for uploads: keyset by SHA-256, every upload once
/// across the pages, each page's `next` and `Link` the same URL; `limit`
/// 1 to `limits.page_max` (past it, or 0, `400 invalid_parameter`), and
/// a cursor this list did not write `400` too.
#[tokio::test]
async fn upload_pages_are_keyset_and_bounded() {
    let dirs = Dirs::new();
    let mut config = dirs.config();
    config.limits.page_max = 5;
    let server = TestServer::in_process(dirs, config);
    let mut all = Vec::new();
    for i in 0..7 {
        let bytes = format!("part {i}");
        all.push(fitment_core::sha256_hex(bytes.as_bytes()));
        assert_eq!(
            server
                .send(support::put_upload(bytes.as_bytes()))
                .await
                .status,
            201
        );
    }
    all.sort();
    let mut seen = Vec::new();
    let mut next = Some("/v1/uploads?limit=3".to_owned());
    while let Some(url) = next {
        let reply = server.get(&url).await;
        assert_eq!(reply.status, 200, "{url}");
        let page = reply.json();
        assert_eq!(page["kind"], "fitment.upload");
        for item in page["items"].as_array().unwrap() {
            seen.push(item["sha256"].as_str().unwrap().to_owned());
        }
        next = page["next"].as_str().map(str::to_owned);
        let link = reply.header("link").map(str::to_owned);
        assert_eq!(link, next.as_ref().map(|n| format!("<{n}>; rel=\"next\"")));
    }
    assert_eq!(seen, all, "every upload once, by SHA-256");
    assert_eq!(
        server.get("/v1/uploads?limit=5").await.json()["items"]
            .as_array()
            .unwrap()
            .len(),
        5
    );
    for bad in [
        "/v1/uploads?limit=6",
        "/v1/uploads?limit=0",
        "/v1/uploads?limit=x",
        "/v1/uploads?after=not-a-cursor",
        "/v1/uploads?after=YWJj",
        "/v1/uploads?nope=1",
    ] {
        let reply = server.get(bad).await;
        assert_eq!(reply.code(), "invalid_parameter", "{bad}");
    }
}

/// VT20 for uploads (HX5, RA8): a name is `sha256:` and 64 hex digits,
/// either case, written lower-case, its colon percent-encoded or not;
/// anything else is `400 invalid_reference` on every method, never a
/// path and never echoed; hostile segments are `404` or `400`, never a
/// `500`, and the state directory holds nothing but its own layout after.
#[tokio::test]
async fn upload_names_are_strict() {
    let server = TestServer::fresh();
    let bytes = b"a strict name";
    let sha256 = fitment_core::sha256_hex(bytes);
    assert_eq!(server.send(support::put_upload(bytes)).await.status, 201);
    for name in [
        format!("sha256%3A{sha256}"),
        format!("sha256:{}", sha256.to_ascii_uppercase()),
    ] {
        let reply = server.get(&format!("/v1/uploads/{name}")).await;
        assert_eq!(reply.status, 200, "{name}");
        assert_eq!(reply.json()["sha256"], sha256.as_str());
    }
    let long = "a".repeat(64 * 1024 - 32);
    for name in [
        format!("sha256:{}", &sha256[..12]),
        format!("sha256:{sha256}0"),
        format!("SHA256:{sha256}"),
        sha256.clone(),
        format!("sha256:{}g", &sha256[..63]),
        "sha256:..".to_owned(),
        "%2e%2e".to_owned(),
        "sha256:%00".to_owned(),
        "sha256:%2f".to_owned(),
        "C:%5Cx.stp".to_owned(),
        "sha256:%E2%80%AE".to_owned(),
        long,
    ] {
        for method in [Method::GET, Method::HEAD, Method::DELETE, Method::PUT] {
            let mut call = Call::new(method.clone(), &format!("/v1/uploads/{name}"));
            if method == Method::PUT {
                call = call.header("content-type", "model/step").body(bytes);
            }
            let reply = server.send(call).await;
            assert!(
                matches!(reply.status.as_u16(), 400 | 404),
                "{method} {}: {}",
                &name[..name.len().min(40)],
                reply.status
            );
            if method != Method::HEAD {
                assert!(
                    matches!(reply.code().as_str(), "invalid_reference" | "not_found"),
                    "{method}"
                );
                let text = String::from_utf8_lossy(&reply.body);
                assert!(!text.contains("x.stp") && !text.contains(".."), "echoed");
            }
        }
    }
    let mut names: Vec<String> = std::fs::read_dir(&server.dirs.state)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names.retain(|n| !n.starts_with("server.db-"));
    assert_eq!(
        names,
        ["FITMENT-SERVER-STATE", "objects", "server.db", "tmp"]
    );
    assert_eq!(server.object_files(), [sha256]);
}

// ---------------------------------------------------------------- jobs

use fitment_server::hooks::Gate;

/// A server of `config`, with ada and bob, readers, each holding an upload
/// of the unit part `p01_through.stp`: their tokens and its SHA-256.
fn jobs_world(
    config: impl FnOnce(&mut fitment_server::Config),
) -> (TestServer, Vec<String>, String) {
    let dirs = Dirs::new();
    let mut c = dirs.config();
    config(&mut c);
    let server = TestServer::in_process(dirs, c);
    let bytes = support::part("p01_through.stp");
    let mut tokens = Vec::new();
    for name in ["ada", "bob", "cyd"] {
        let user = seed_user(server.server(), name, Role::Reader, None);
        tokens.push(seed_token(server.server(), &user, Role::Reader, 60).1);
        support::seed_upload(server.server(), user.key, &bytes);
    }
    (server, tokens, fitment_core::sha256_hex(&bytes))
}

fn submission(sha: &str, token: &str) -> Call {
    post(
        "/v1/features",
        token,
        &json!({ "upload": format!("sha256:{sha}") }),
    )
}

fn job_rows(server: &TestServer) -> i64 {
    let conn = rusqlite::Connection::open(server.dirs.state.join("server.db")).unwrap();
    conn.query_row("SELECT count(*) FROM job", [], |r| r.get(0))
        .unwrap()
}

/// JS5, JS4, VT21: past its owner's cap of queued jobs a submission is
/// `429 too_many_jobs`, past its lane's `503 queue_full`, each retryable
/// with `Retry-After`, and neither writes a row; `fitment.info` says a
/// queue is full (DC11).
#[tokio::test]
async fn queues_and_owners_are_capped_before_any_row() {
    let (server, tokens, sha) = jobs_world(|c| {
        c.jobs.queue_query = 3;
        c.jobs.per_owner_queued = 2;
    });
    server.server().hold_jobs();
    for _ in 0..2 {
        assert_eq!(server.send(submission(&sha, &tokens[0])).await.status, 202);
    }
    let owner = server.send(submission(&sha, &tokens[0])).await;
    assert_eq!(
        (owner.status.as_u16(), owner.code()),
        (429, "too_many_jobs".into())
    );
    assert!(owner.header("retry-after").is_some());
    assert_eq!(owner.json()["retryable"], true);
    assert_eq!(server.send(submission(&sha, &tokens[1])).await.status, 202);
    let lane = server.send(submission(&sha, &tokens[2])).await;
    assert_eq!(
        (lane.status.as_u16(), lane.code()),
        (503, "queue_full".into())
    );
    assert!(lane.header("retry-after").is_some());
    assert_eq!(job_rows(&server), 3, "a refusal leaves nothing behind");
    let info = server.get("/v1/info").await.json();
    assert_eq!(info["catalog"]["state"]["queues_full"], true);
    server.server().resume_jobs();
    let listed = server
        .send(Call::get("/v1/jobs?wait=0").bearer(&tokens[0]))
        .await;
    assert_eq!(listed.status, 400, "the list takes no wait");
}

/// JS10, VT24's stream case: at most `jobs.streams_per_user` event streams
/// a user over all their tokens, and `jobs.streams` in all: past either,
/// `429 too_many_streams`; a stream closed gives its slot back.
#[tokio::test]
async fn event_streams_are_capped_per_user_and_in_all() {
    let (server, tokens, sha) = jobs_world(|c| {
        c.jobs.streams_per_user = 2;
        c.jobs.streams = 3;
    });
    server.server().hold_jobs();
    let mut ids = Vec::new();
    for token in &tokens {
        ids.push(
            server.send(submission(&sha, token)).await.json()["id"]
                .as_str()
                .unwrap()
                .to_owned(),
        );
    }
    let stream = |i: usize| Call::get(&format!("/v1/jobs/{}/events", ids[i])).bearer(&tokens[i]);
    let first = server.open(stream(0)).await;
    let second = server.open(stream(0)).await;
    assert_eq!(
        (first.status().as_u16(), second.status().as_u16()),
        (200, 200)
    );
    let refused = server.send(stream(0)).await;
    assert_eq!(refused.code(), "too_many_streams", "per user");
    let bob = server.open(stream(1)).await;
    assert_eq!(bob.status(), 200);
    assert_eq!(
        server.send(stream(2)).await.code(),
        "too_many_streams",
        "in all"
    );
    drop(first);
    // The slot is given back once the stream's task sees its client gone.
    let start = Instant::now();
    loop {
        let reply = server.open(stream(2)).await;
        if reply.status() == 200 {
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "the slot never came back"
        );
        tokio::task::yield_now().await;
    }
    drop((second, bob));
}

/// JS10, JB11 (decided in PR 5's review): a wait holds one of its user's
/// stream slots while it waits, so past `jobs.streams_per_user` a
/// `?wait=N` or a `Prefer: wait=N` submission is `429 too_many_streams`,
/// the latter before any row; a wait of zero takes no slot; a slot given
/// back serves the wait.
#[tokio::test]
async fn waits_count_against_the_stream_cap() {
    let (server, tokens, sha) = jobs_world(|c| c.jobs.streams_per_user = 1);
    server.server().hold_jobs();
    let job = server.send(submission(&sha, &tokens[0])).await.json();
    let id = job["id"].as_str().unwrap().to_owned();
    let stream = server
        .open(Call::get(&format!("/v1/jobs/{id}/events")).bearer(&tokens[0]))
        .await;
    assert_eq!(stream.status(), 200);
    let poll = |wait: u32| Call::get(&format!("/v1/jobs/{id}?wait={wait}")).bearer(&tokens[0]);
    assert_eq!(server.send(poll(5)).await.code(), "too_many_streams");
    assert_eq!(server.send(poll(0)).await.status, 200, "no wait, no slot");
    let rows = job_rows(&server);
    let waiting = submission(&sha, &tokens[0]).header("prefer", "wait=5");
    assert_eq!(server.send(waiting).await.code(), "too_many_streams");
    assert_eq!(job_rows(&server), rows, "refused before any row");
    drop(stream);
    server.server().resume_jobs();
    let start = Instant::now();
    loop {
        let reply = server.send(poll(5)).await;
        if reply.status == 200 {
            assert_eq!(reply.json()["state"], "succeeded");
            break;
        }
        assert_eq!(reply.code(), "too_many_streams");
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "the slot never came back"
        );
        tokio::task::yield_now().await;
    }
}

/// JB7, JS8, SN7, VT41's deadline case: a job past its deadline is made
/// `cancelling`, then, when its call returns, `failed`,
/// `deadline_exceeded`, with no document and no verdict; a cancel in
/// between changes neither (`202`, the first reason stands: review S7 of
/// PR 5), and its key is kept (JB9).
#[tokio::test]
async fn a_job_past_its_deadline_fails_with_no_document() {
    let (server, tokens, sha) = jobs_world(|_| {});
    let seams = server.server().seams();
    seams.jobs.set_deadline(Duration::from_millis(200));
    let gate = Gate::new();
    seams.hooks.set("job.read", Action::Pause(gate.clone()));
    let keyed = submission(&sha, &tokens[0]).header("idempotency-key", "late");
    let job = server.send(keyed.clone()).await.json();
    let id = job["id"].as_str().unwrap().to_owned();
    assert!(gate.arrived(1, Duration::from_secs(30)).await);
    let events = Call::get(&format!("/v1/jobs/{id}/events")).bearer(&tokens[0]);
    let mut body = server.open(events).await.into_body();
    let seen = support::lines(&mut body, Duration::from_secs(30), |e| {
        e["state"] == "cancelling"
    })
    .await;
    assert_eq!(seen.last().unwrap()["state"], "cancelling");
    // Review S7 of PR 5: a cancel after the deadline leaves its reason.
    let cancel = Call::new(Method::POST, &format!("/v1/jobs/{id}/cancel")).bearer(&tokens[0]);
    let cancelling = server.send(cancel).await;
    gate.open();
    assert_eq!(cancelling.status, 202);
    assert_eq!(
        cancelling.json()["cancel_requested"],
        false,
        "the first reason stands"
    );
    let ended = support::lines(&mut body, Duration::from_secs(30), |e| e["type"] == "end").await;
    let end = ended.last().unwrap();
    assert_eq!(end["state"], "failed");
    assert_eq!(end["error"]["code"], "deadline_exceeded");
    assert!(end["document"].is_null() && end["outcome"].is_null());
    let job = server
        .send(Call::get(&format!("/v1/jobs/{id}")).bearer(&tokens[0]))
        .await
        .json();
    assert_eq!(job["state"], "failed");
    assert_eq!(
        job["exit_code"],
        serde_json::Value::Null,
        "no CLI run passes a deadline"
    );
    let again = server.send(keyed).await;
    assert_eq!(
        again.json()["id"],
        id.as_str(),
        "JB9: a deadline's end keeps its key"
    );
}

/// JB11, M6-53: `Prefer: wait=N` past `limits.wait_max_seconds` is applied
/// as the cap and `Preference-Applied` says so; a wait outlives the
/// handler's own time; it answers `200` when the job ends within it and
/// `202` with the job as it stands when it does not; `?wait=N` long-polls
/// the same way.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn waits_are_capped_and_outlive_the_handler() {
    let (server, tokens, sha) = jobs_world(|c| {
        c.server.handler_timeout_seconds = 1;
        c.limits.wait_max_seconds = 2;
    });
    let server = std::sync::Arc::new(server);
    let gate = Gate::new();
    server
        .server()
        .seams()
        .hooks
        .set("job.read", Action::Pause(gate.clone()));
    let call = submission(&sha, &tokens[0]).header("prefer", "wait=999");
    let start = Instant::now();
    let reply = server.send(call).await;
    assert_eq!(
        reply.status,
        202,
        "{}",
        String::from_utf8_lossy(&reply.body)
    );
    assert!(
        start.elapsed() >= Duration::from_secs(2),
        "it waited past the handler's 1 s"
    );
    assert_eq!(reply.header("preference-applied"), Some("wait=2"));
    assert_eq!(reply.json()["state"], "running");
    let id = reply.json()["id"].as_str().unwrap().to_owned();
    let polling = std::sync::Arc::clone(&server);
    let (path, token) = (format!("/v1/jobs/{id}?wait=60"), tokens[0].clone());
    let poll = tokio::spawn(async move { polling.send(Call::get(&path).bearer(&token)).await });
    gate.open();
    let polled = poll.await.unwrap();
    assert_eq!(polled.json()["state"], "succeeded");
    server.server().seams().hooks.clear("job.read");
    let waited = server
        .send(submission(&sha, &tokens[0]).header("prefer", "wait=2"))
        .await;
    assert_eq!(waited.status, 200);
    assert_eq!(waited.json()["state"], "succeeded");
}

/// VT21, JB12, JS9, VN3: 10,000 finished jobs list once each across pages
/// of at most `limits.page_max`, each page bounded in bytes; past their
/// retention (the test-support clock) the sweep deletes all but a pinned
/// one, whose record stays.
#[tokio::test]
async fn ten_thousand_jobs_list_expire_and_one_stays_pinned() {
    let (server, tokens, sha) = jobs_world(|_| {});
    let job = server
        .send(submission(&sha, &tokens[0]).header("prefer", "wait=30"))
        .await
        .json();
    assert_eq!(job["state"], "succeeded");
    let conn = rusqlite::Connection::open(server.dirs.state.join("server.db")).unwrap();
    let owner: i64 = conn
        .query_row(
            "SELECT owner FROM job WHERE id = ?1",
            [job["id"].as_str().unwrap()],
            |r| r.get(0),
        )
        .unwrap();
    let tx = conn.unchecked_transaction().unwrap();
    for n in 0..9_999 {
        tx.execute(
            "INSERT INTO job (id, owner, kind, lane, state, request, request_sha256, request_id,
                              created_ms, finished_ms, exit, expires_ms)
             VALUES (?1, ?2, 'features', 'query', 'succeeded', x'7b7d', ?3, 'r_seeded', ?4, ?4, 0, ?5)",
            rusqlite::params![
                format!("j_{n:026}"),
                owner,
                "0".repeat(64),
                1_790_000_000_000_i64 - 1 - i64::from(n),
                1_790_000_000_000_i64 + 30 * 86_400_000
            ],
        )
        .unwrap();
    }
    tx.commit().unwrap();
    let mut seen = std::collections::BTreeSet::new();
    let mut next = Some("/v1/jobs?limit=1000".to_owned());
    let mut pages = 0;
    while let Some(path) = next {
        let page = server.send(Call::get(&path).bearer(&tokens[0])).await;
        assert!(page.body.len() < 4 << 20, "a page is bounded");
        let page = page.json();
        for item in page["items"].as_array().unwrap() {
            assert!(
                seen.insert(item["id"].as_str().unwrap().to_owned()),
                "listed once"
            );
        }
        next = page["next"].as_str().map(str::to_owned);
        pages += 1;
    }
    assert_eq!((seen.len(), pages), (10_000, 10));
    let admin = seed_user(server.server(), "root", Role::Admin, None);
    let (_, root) = seed_token(server.server(), &admin, Role::Admin, 400);
    let pin = post(
        &format!("/v1/jobs/{}/pin", job["id"].as_str().unwrap()),
        &root,
        &json!({ "pinned": true }),
    );
    assert_eq!(server.send(pin).await.json()["pinned"], true);
    server
        .server()
        .seams()
        .ids
        .fix_time(1_790_000_000_000 + 31 * 86_400_000);
    let (jobs, _) = server.server().sweep_jobs().unwrap();
    assert_eq!(jobs, 9_999);
    let left = server
        .send(Call::get("/v1/jobs").bearer(&tokens[0]))
        .await
        .json();
    assert_eq!(left["items"].as_array().unwrap().len(), 1);
    assert_eq!(left["items"][0]["id"], job["id"]);
}

/// A server whose harness admin holds s01's BRACKET socket and the
/// committed bracket: its socket revision and the candidate's `files`.
async fn match_world(
    config: impl FnOnce(&mut fitment_server::Config),
) -> (TestServer, String, serde_json::Value) {
    let dirs = Dirs::new();
    let mut c = dirs.config();
    config(&mut c);
    let server = TestServer::in_process(dirs, c);
    let socket = server.socket_of("s01_bolted", "BRACKET").await;
    let sha = server.upload(&support::candidate("m_bracket.stp")).await;
    let files = json!([{ "upload": format!("sha256:{sha}"), "label": "m_bracket.stp" }]);
    (server, socket, files)
}

/// UP6, VT45: a socket document is read from at most
/// `limits.max_socket_mib` bytes: one past it, as an import, a match's
/// socket or a `carry_from`, is `413 too_large` (`details.limit`) before
/// it is read; one at it is read (and refused here for what it says).
#[tokio::test]
async fn a_socket_document_past_its_cap_is_refused() {
    let (server, socket, files) = match_world(|c| c.limits.max_socket_mib = 1).await;
    let at = vec![b' '; 1024 * 1024];
    let mut past = at.clone();
    past.push(b' ');
    let at = server.upload(&at).await;
    let past = server.upload(&past).await;
    let s01 = fitment_core::sha256_hex(&support::assembly("s01_bolted.stp"));
    for (sha, status, code) in [(&past, 413, "too_large"), (&at, 422, "socket_unreadable")] {
        let upload = json!({ "upload": format!("sha256:{sha}") });
        let calls = [
            support::waiting("/v1/sockets/import", &upload),
            support::waiting(
                "/v1/matches",
                &json!({ "socket": upload, "candidates": { "files": files } }),
            ),
            support::waiting(
                "/v1/sockets",
                &json!({ "assembly": format!("sha256:{s01}"), "part": "BRACKET", "carry_from": upload }),
            ),
        ];
        for (i, call) in calls.into_iter().enumerate() {
            let reply = server.send(call).await;
            let code = if i == 2 && status == 422 {
                "carry_from_foreign"
            } else {
                code
            };
            assert_eq!(
                (reply.status.as_u16(), reply.code().as_str()),
                (status, code),
                "{i}"
            );
            if status == 413 {
                assert_eq!(reply.json()["details"]["limit"], "max_socket_mib");
            }
        }
    }
    let _ = socket;
}

/// HX8's parse slots (VT45's parse half, review F22): socket documents are
/// parsed at requests at most `limits.parse_concurrency` at once; with
/// four slots, five imports held at the parse leave the fifth waiting, and
/// past the handler's time it is `503 request_timeout`, not retryable on a
/// write (ER6); the four go on when let through.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn socket_documents_parse_four_at_a_time() {
    let (server, _, _) = match_world(|c| {
        c.limits.parse_concurrency = 4;
        c.server.handler_timeout_seconds = 2;
    })
    .await;
    let file = support::cli_golden("socket", "s01_bolted--BRACKET.json");
    let sha = server.upload(&file).await;
    let gate = Gate::new();
    let seams = server.server().seams();
    seams.hooks.set("socket.parse", Action::Pause(gate.clone()));
    let import = || {
        Call::json(
            Method::POST,
            "/v1/sockets/import",
            &json!({ "upload": format!("sha256:{sha}") }),
        )
    };
    let server = std::sync::Arc::new(server);
    let held: Vec<_> = (0..4)
        .map(|_| {
            let (server, call) = (std::sync::Arc::clone(&server), import());
            tokio::spawn(async move { server.send(call).await })
        })
        .collect();
    assert!(
        gate.arrived(4, Duration::from_secs(30)).await,
        "four parse at once"
    );
    let started = Instant::now();
    let fifth = server.send(import()).await;
    assert_eq!(
        (fifth.status.as_u16(), fifth.code()),
        (503, "request_timeout".to_owned())
    );
    assert_eq!(fifth.json()["retryable"], false);
    assert!(
        started.elapsed() >= Duration::from_secs(2),
        "it waited the handler's time"
    );
    assert_eq!(gate.reached(), 4, "the fifth never parsed");
    gate.open();
    for held in held {
        assert_eq!(held.await.unwrap().status, 202);
    }
    server.server().seams().hooks.clear("socket.parse");
}

/// RA8, HX2, CM1: a match's names and candidates within their bounds: a
/// label of 1,024 bytes is taken, one more byte or a control character is
/// `400 name_invalid`, one starting `catalog:` `400 label_reserved`; past
/// `limits.max_candidates` candidates `400 invalid_parameter`; a component
/// naming neither a part nor an occurrence, and a tolerance of `-0.0`,
/// zero or 90 degrees, `400 invalid_parameter`; none at all `400
/// no_candidates`. Each before any job.
#[tokio::test]
async fn match_names_and_candidates_past_their_bounds_are_refused() {
    let (server, socket, files) = match_world(|c| c.limits.max_candidates = 2).await;
    let sha = files[0]["upload"].as_str().unwrap().to_owned();
    let file = |label: &str| json!({ "upload": sha, "label": label });
    let s01 = fitment_core::sha256_hex(&support::assembly("s01_bolted.stp"));
    let with = |candidates: serde_json::Value, extra: serde_json::Value| {
        let mut body = json!({ "socket": socket, "candidates": candidates });
        for (k, v) in extra.as_object().unwrap() {
            body[k] = v.clone();
        }
        support::waiting("/v1/matches", &body)
    };
    let long = "l".repeat(1024);
    let ok = server
        .send(with(json!({ "files": [file(&long)] }), json!({})))
        .await;
    assert_eq!(ok.status, 200, "{}", String::from_utf8_lossy(&ok.body));
    let cases = [
        (
            json!({ "files": [file(&format!("{long}l"))] }),
            json!({}),
            "name_invalid",
        ),
        (
            json!({ "files": [file("tab\there")] }),
            json!({}),
            "name_invalid",
        ),
        (
            json!({ "files": [file("catalog:BRACKET@r1")] }),
            json!({}),
            "label_reserved",
        ),
        (
            json!({ "files": [file("a"), file("b"), file("c")] }),
            json!({}),
            "invalid_parameter",
        ),
        (
            json!({ "components": [{ "assembly": format!("sha256:{s01}") }] }),
            json!({}),
            "invalid_parameter",
        ),
        (
            json!({ "files": [file("a")] }),
            json!({ "policy": { "tolerance_mm": -0.0 } }),
            "invalid_parameter",
        ),
        (
            json!({ "files": [file("a")] }),
            json!({ "policy": { "tolerance_mm": 0.0 } }),
            "invalid_parameter",
        ),
        (
            json!({ "files": [file("a")] }),
            json!({ "policy": { "tolerance_deg": 90.0 } }),
            "invalid_parameter",
        ),
        (json!({}), json!({}), "no_candidates"),
    ];
    let before = server.get("/v1/jobs?kind=match").await.json()["items"]
        .as_array()
        .unwrap()
        .len();
    for (candidates, extra, code) in cases {
        let reply = server.send(with(candidates.clone(), extra.clone())).await;
        assert_eq!(
            (reply.status.as_u16(), reply.code().as_str()),
            (400, code),
            "{candidates} {extra}"
        );
    }
    let after = server.get("/v1/jobs?kind=match").await.json()["items"]
        .as_array()
        .unwrap()
        .len();
    assert_eq!(before, after, "no job was made");
}

/// JB7, SN7, VT22's deadline: a match past a test-sized deadline is
/// stopped between windows and ends `failed`, `deadline_exceeded`, with no
/// document and no outcome, never `reject`.
#[tokio::test]
async fn a_match_past_its_deadline_fails_with_no_document() {
    let (server, socket, files) = match_world(|_| {}).await;
    let seams = server.server().seams();
    seams.jobs.set_deadline(Duration::from_millis(200));
    seams
        .hooks
        .set("match.window", Action::Delay(Duration::from_millis(600)));
    let job = server
        .run_match(&json!({ "socket": socket, "candidates": { "files": files } }))
        .await;
    assert_eq!(
        (job["state"].as_str(), job["error"]["code"].as_str()),
        (Some("failed"), Some("deadline_exceeded")),
        "{job}"
    );
    assert!(job["document"].is_null() && job["outcome"].is_null() && job["exit_code"].is_null());
    let id = job["id"].as_str().unwrap();
    let document = server.get(&format!("/v1/matches/{id}/document")).await;
    assert_eq!(document.code(), "no_document");
    let run = server.get(&format!("/v1/matches/{id}")).await.json();
    assert!(run["outcome"].is_null() && run["summary"].is_null() && run["document"].is_null());
}

/// VT22, JB7: the corpus's hostile parts (`h_*`) as uploaded candidates
/// against their sockets: each judged within M4's bound and the job's
/// deadline, its document the CLI's for the same file, byte for byte.
/// Release only, as the corpus suites are.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hostile_parts_through_the_api_are_the_clis() {
    let Some(corpus) = fitment_corpus::Corpus::locate_release_only() else {
        return;
    };
    let server = TestServer::fresh();
    let mut seen = 0;
    for case in corpus.cases().iter().filter(|c| c["kind"] == "hostile") {
        seen += 1;
        let id = case["id"].as_str().unwrap();
        let source = case["socket"]["case"].as_str().unwrap();
        let part = case["socket"]["part"].as_str().unwrap();
        let bytes = std::fs::read(corpus.dir().join(format!("{source}.stp"))).unwrap();
        server.register(&bytes, &format!("{source}.stp")).await;
        let extracted = server
            .extract(&fitment_core::sha256_hex(&bytes), part)
            .await;
        let socket = extracted["result"]["href"]
            .as_str()
            .unwrap()
            .trim_start_matches("/v1/sockets/")
            .to_owned();
        let file = case["candidate"]["file"].as_str().unwrap();
        let sha = server
            .upload(&std::fs::read(corpus.dir().join(file)).unwrap())
            .await;
        let started = Instant::now();
        let job = server
            .run_match(&json!({ "socket": socket, "candidates": { "files": [
                { "upload": format!("sha256:{sha}"), "label": file } ] } }))
            .await;
        let bound = case["expect"]["within_s"].as_f64().unwrap();
        assert!(
            started.elapsed().as_secs_f64() < bound + 5.0,
            "{id}: past its bound"
        );
        assert_eq!(job["state"], "succeeded", "{id}: {job}");
        let scratch = server.dirs.root.join(id);
        std::fs::create_dir_all(&scratch).unwrap();
        std::fs::write(
            scratch.join("S.json"),
            server.get(&format!("/v1/sockets/{socket}")).await.body,
        )
        .unwrap();
        std::fs::write(
            scratch.join(file),
            std::fs::read(corpus.dir().join(file)).unwrap(),
        )
        .unwrap();
        let (code, stdout, _) =
            support::cli(&scratch, &["match", "S.json", file, "--format", "json"]);
        assert_eq!(job["exit_code"].as_i64(), code.map(i64::from), "{id}");
        assert_eq!(server.document(&job).await.body, stdout, "{id}");
    }
    assert!(seen >= 4, "{seen} hostile cases");
}
