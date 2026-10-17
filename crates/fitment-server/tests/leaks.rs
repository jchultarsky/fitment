//! VT34 (docs/research/m6-api.md, XH13, ER3): the leak scan's own
//! controls. A path, a token, SQL and a panic message planted in a
//! response through the `test-support` hooks are each found; a token
//! planted in a log line is found, and a path relative to a configured
//! root in a log line is not flagged (OP1).

mod support;

use fitment_server::hooks::Action;
use support::{Call, TestServer, scan};

/// XH13's response half: each planted leak is found, and the real
/// response, the same route without the plant, is clean.
#[tokio::test]
async fn planted_leaks_in_a_response_are_found() {
    let server = TestServer::fresh();
    let roots = server.dirs.roots();
    let state = server.dirs.state.display().to_string();
    let hooks = &server.server().seams().hooks;
    for (planted, kind) in [
        (
            format!("stored at {state}/objects"),
            "a path of the test machine",
        ),
        ("fitment_pat_Zm9vYmFyYmF6".to_owned(), "fitment_pat_"),
        ("Authorization: Bearer abc".to_owned(), "Bearer "),
        ("SELECT secret_sha256 FROM token".to_owned(), "SELECT "),
        ("no such table: user (sqlite)".to_owned(), "sqlite"),
        (
            "thread 'tokio' panicked at src/lib.rs:4:2".to_owned(),
            "panicked at",
        ),
        ("fitment_catalog::Error::Io".to_owned(), "fitment_catalog::"),
    ] {
        hooks.set("info", Action::Plant(planted.clone()));
        let reply = server.send_unchecked(Call::get("/v1/info")).await;
        let found = scan::response(&reply, &roots);
        assert!(
            found.iter().any(|f| f.contains(kind)),
            "{planted:?} was not found: {found:?}"
        );
    }
    hooks.clear("info");
    let reply = server.send_unchecked(Call::get("/v1/info")).await;
    assert_eq!(scan::response(&reply, &roots), Vec::<String>::new());
}

/// XH13's log half: a token in a log line is found; paths are allowed in
/// logs, and one written relative to a configured root is not flagged.
#[tokio::test]
async fn planted_leaks_in_the_log_are_found() {
    support::logs::capture();
    let server = TestServer::fresh();
    let token = "fitment_pat_cGxhbnRlZC10b2tlbg";
    support::logs::plant(token);
    let hooks = &server.server().seams().hooks;
    hooks.set("info", Action::Log(format!("presented {token}")));
    let _ = server.send_unchecked(Call::get("/v1/info")).await;
    let logs = support::logs::text();
    let line = logs
        .lines()
        .find(|l| l.contains(token))
        .expect("the planted line is logged");
    assert!(!scan::logs(line).is_empty(), "the token was not found");
    let relative = "wrote state:objects/sha256/ab/ab12 and catalog:blobs/sha256/cd";
    hooks.set("info", Action::Log(relative.into()));
    let _ = server.send_unchecked(Call::get("/v1/info")).await;
    let logs = support::logs::text();
    let line = logs
        .lines()
        .find(|l| l.contains(relative))
        .expect("the line is logged");
    assert_eq!(scan::logs(line), Vec::<String>::new());
}
