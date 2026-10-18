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
