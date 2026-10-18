//! AU9's offline commands through the built binary (docs/research/m6-api.md,
//! section 9.4's process layer; VT24): `fitment-server admin bootstrap`,
//! `user` and `token`, on `server.db` directly, while a server runs or
//! not, a secret on stdout once and never on stderr or in the server's log
//! (audit flaw 3).
#![allow(clippy::disallowed_methods)] // runs the binary, clearing its environment

mod support;

use std::io::Write as _;
use std::process::{Command, Stdio};

use axum::http::Method;
use fitment_server::state::Kind;
use fitment_server::state::db::may_migrate;
use serde_json::json;
use support::{Call, Dirs, TestServer, binary, scan};

/// Runs `fitment-server` with `args`, `input` on its stdin: its exit code,
/// stdout and stderr.
fn run(args: &[&str], input: &str) -> (Option<i32>, String, String) {
    let mut command = Command::new(binary());
    command
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (name, _) in std::env::vars() {
        if name.starts_with("FITMENT_SERVER_") {
            command.env_remove(name);
        }
    }
    let mut child = command.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    (
        output.status.code(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

fn state(dirs: &Dirs) -> String {
    dirs.state.display().to_string()
}

/// `fitment-server admin <words> --state <at>`, `input` on stdin.
fn admin(at: &str, words: &str, input: &str) -> (Option<i32>, String, String) {
    let mut args = vec!["admin"];
    args.extend(words.split_whitespace());
    args.extend(["--state", at]);
    run(&args, input)
}

/// AU9, fix 3: `bootstrap` makes the first admin and prints its token to
/// stdout, alone and once, never to stderr; a second is refused (exit 2);
/// the token works against a server started on that state, and appears in
/// no log.
#[tokio::test(flavor = "multi_thread")]
async fn bootstrap_prints_the_first_admins_token_once() {
    let dirs = Dirs::new();
    let at = state(&dirs);
    let (code, stdout, stderr) = admin(&at, "bootstrap --username root", "");
    assert_eq!(code, Some(0), "{stderr}");
    let token = stdout.trim_end().to_owned();
    scan::secret(&token);
    assert_eq!(stdout, format!("{token}\n"), "stdout is the token alone");
    assert!(
        token.starts_with("fitment_pat_") && token.len() == 55,
        "{token}"
    );
    assert!(stderr.contains("admin root created"), "{stderr}");
    assert_eq!(
        scan::logs(&stderr),
        Vec::<String>::new(),
        "a secret on stderr"
    );
    let (code, stdout, stderr) = admin(&at, "bootstrap --username other", "");
    assert_eq!((code, stdout.as_str()), (Some(2), ""), "{stderr}");
    assert!(stderr.contains("an admin exists"), "{stderr}");
    let server = TestServer::process(dirs, "");
    let me = server
        .send(Call::get("/v1/auth/me").bearer(&token))
        .await
        .json();
    assert_eq!(
        (&me["username"], &me["role"]),
        (&json!("root"), &json!("admin"))
    );
    assert_eq!(me["token"]["role"], "admin");
    let tokens = server
        .send(Call::get("/v1/auth/tokens").bearer(&token))
        .await
        .json();
    let first = &tokens["items"][0];
    assert_eq!(
        (&first["name"], &first["parent"]),
        (&json!("bootstrap"), &json!(null))
    );
}

/// AU9: the commands work beside a running server (WAL), without its
/// lease: a password set offline logs in, and a role set offline is in
/// force at the next request, capped by the token's own role (AU5).
#[tokio::test(flavor = "multi_thread")]
async fn users_are_managed_offline_beside_the_server() {
    let server = TestServer::process(Dirs::new(), "");
    let at = state(&server.dirs);
    let password = support::password("an offline password");
    let (code, stdout, stderr) = admin(
        &at,
        "user add --username ada --password-stdin",
        &format!("{password}\n"),
    );
    assert_eq!(code, Some(0), "{stderr}");
    assert!(stdout.trim().starts_with("u_"), "{stdout}");
    let login = json!({ "username": "ada", "password": password });
    let reply = server
        .send(Call::json(Method::POST, "/v1/auth/login", &login))
        .await;
    assert_eq!(reply.status, 201);
    let session = reply.json()["secret"].as_str().unwrap().to_owned();
    let (code, _, stderr) = admin(&at, "user set --username ada --role admin", "");
    assert_eq!(code, Some(0), "{stderr}");
    let users = server
        .send(Call::get("/v1/auth/users").bearer(&session))
        .await;
    assert_eq!(users.status, 403, "the token's own role, reader, caps it");
    let (_, listing, _) = admin(&at, "user list", "");
    assert!(
        listing.lines().any(|l| l.ends_with(" ada admin enabled")),
        "{listing}"
    );
    let (code, _, _) = admin(&at, "user set --username ada --disabled true", "");
    assert_eq!(code, Some(0));
    let me = server.send(Call::get("/v1/auth/me").bearer(&session)).await;
    assert_eq!(me.code(), "unauthenticated", "disabled at the next request");
}

/// AU9, AU4: a token minted offline works, a listing shows no secret, its
/// revocation takes effect at the next request, and `reset-password`
/// revokes every token of the user, the new password logging in.
#[tokio::test(flavor = "multi_thread")]
async fn tokens_are_minted_and_revoked_offline() {
    let server = TestServer::process(Dirs::new(), "");
    let at = state(&server.dirs);
    let password = support::password("a second offline password");
    admin(
        &at,
        "user add --username ada --role writer --password-stdin",
        &format!("{password}\n"),
    );
    let login = json!({ "username": "ada", "password": password });
    let reply = server
        .send(Call::json(Method::POST, "/v1/auth/login", &login))
        .await;
    let session = reply.json()["secret"].as_str().unwrap().to_owned();
    let (code, stdout, stderr) = admin(&at, "token mint --username ada --name ops --days 3", "");
    assert_eq!(code, Some(0), "{stderr}");
    let minted = stdout.trim_end().to_owned();
    scan::secret(&minted);
    assert_eq!(scan::logs(&stderr), Vec::<String>::new());
    let me = server
        .send(Call::get("/v1/auth/me").bearer(&minted))
        .await
        .json();
    assert_eq!(me["token"]["role"], "writer");
    let (code, listing, _) = admin(&at, "token list --username ada", "");
    assert_eq!(code, Some(0));
    assert!(
        !listing.contains(&minted[12..]),
        "a listing shows no secret"
    );
    let line = listing.lines().find(|l| l.contains("\"ops\"")).unwrap();
    let id = line.split(' ').next().unwrap().to_owned();
    let (code, _, _) = admin(&at, &format!("token revoke --id {id}"), "");
    assert_eq!(code, Some(0));
    let reply = server.send(Call::get("/v1/auth/me").bearer(&minted)).await;
    assert_eq!(reply.code(), "unauthenticated");
    let fresh = support::password("a fresh offline password");
    let (code, _, stderr) = admin(
        &at,
        "user reset-password --username ada --password-stdin",
        &format!("{fresh}\n"),
    );
    assert_eq!(code, Some(0), "{stderr}");
    assert!(stderr.contains("1 tokens revoked"), "{stderr}");
    let reply = server.send(Call::get("/v1/auth/me").bearer(&session)).await;
    assert_eq!(reply.code(), "unauthenticated");
    let login = json!({ "username": "ada", "password": fresh });
    let reply = server
        .send(Call::json(Method::POST, "/v1/auth/login", &login))
        .await;
    assert_eq!(reply.status, 201);
}

/// AU9: usage is exit 2 (an unknown user, a bad password, a role above
/// the user's), a newer `server.db` exit 3, and nothing is written.
#[test]
fn the_commands_refuse_with_the_clis_exit_codes() {
    let dirs = Dirs::new();
    let at = state(&dirs);
    let (code, _, _) = run(
        &["admin", "user", "add", "--state", &at, "--username", "ada"],
        "",
    );
    assert_eq!(code, Some(0));
    for (args, input) in [
        (
            vec![
                "admin",
                "user",
                "set",
                "--state",
                &at,
                "--username",
                "nobody",
                "--role",
                "admin",
            ],
            "",
        ),
        (
            vec![
                "admin",
                "user",
                "add",
                "--state",
                &at,
                "--username",
                "bad name",
            ],
            "",
        ),
        (
            vec!["admin", "user", "add", "--state", &at, "--username", "ADA"],
            "",
        ),
        (
            vec![
                "admin",
                "user",
                "add",
                "--state",
                &at,
                "--username",
                "bob",
                "--password-stdin",
            ],
            "short\n",
        ),
        (
            vec![
                "admin",
                "token",
                "mint",
                "--state",
                &at,
                "--username",
                "ada",
                "--role",
                "admin",
            ],
            "",
        ),
        (
            vec![
                "admin",
                "token",
                "mint",
                "--state",
                &at,
                "--username",
                "ada",
                "--days",
                "366",
            ],
            "",
        ),
        (
            vec!["admin", "token", "revoke", "--state", &at, "--id", "t_nope"],
            "",
        ),
    ] {
        let (code, stdout, stderr) = run(&args, input);
        assert_eq!(code, Some(2), "{args:?}: {stderr}");
        assert_eq!(stdout, "", "{args:?}");
    }
    let db = rusqlite::Connection::open(dirs.state.join("server.db")).unwrap();
    db.pragma_update(None, "user_version", 99).unwrap();
    drop(db);
    let (code, _, stderr) = run(&["admin", "user", "list", "--state", &at], "");
    assert_eq!(code, Some(3), "{stderr}");
}

/// AU9, SD3 (review question 2): the offline commands never migrate an
/// older `server.db` while a server holds the lease; without a holder they
/// may, and a current schema needs nothing. (Schema 1 is the first: no
/// older one exists yet to migrate.)
#[test]
fn the_commands_never_migrate_beside_a_server() {
    assert!(may_migrate(Kind::Older(0), true).is_err());
    assert!(may_migrate(Kind::Older(0), false).is_ok());
    assert!(may_migrate(Kind::Current, true).is_ok());
    assert!(may_migrate(Kind::Empty, true).is_ok());
}

/// AU9 (review): eight concurrent bootstraps on one state directory not
/// yet made make one admin; the others are refused (exit 2), checked in
/// the transaction that inserts.
#[test]
fn concurrent_bootstraps_make_one_admin() {
    let dirs = Dirs::new();
    let at = state(&dirs);
    let runs: Vec<_> = (0..8)
        .map(|i| {
            let at = at.clone();
            std::thread::spawn(move || admin(&at, &format!("bootstrap --username root{i}"), ""))
        })
        .collect();
    let mut codes: Vec<Option<i32>> = Vec::new();
    for run in runs {
        let (code, stdout, stderr) = run.join().unwrap();
        if code == Some(0) {
            scan::secret(stdout.trim_end());
        } else {
            assert!(stderr.contains("an admin exists"), "{stderr}");
        }
        codes.push(code);
    }
    assert_eq!(
        codes.iter().filter(|c| **c == Some(0)).count(),
        1,
        "{codes:?}"
    );
    assert!(codes.iter().all(|c| matches!(c, Some(0 | 2))), "{codes:?}");
    let (_, listing, _) = admin(&at, "user list", "");
    assert_eq!(
        listing.lines().filter(|l| l.contains(" admin ")).count(),
        1,
        "{listing}"
    );
}

/// AU1 (review): hashing a password offline without a configured pepper
/// warns on stderr, since the server's pepper is recorded nowhere.
#[test]
fn hashing_without_a_pepper_warns() {
    let dirs = Dirs::new();
    let at = state(&dirs);
    let password = support::password("a pepperless password");
    let (code, _, stderr) = admin(
        &at,
        "user add --username ada --password-stdin",
        &format!("{password}\n"),
    );
    assert_eq!(code, Some(0), "{stderr}");
    assert!(
        stderr.contains("warning: no auth.pepper_file is configured"),
        "{stderr}"
    );
    let (code, _, stderr) = admin(&at, "user add --username bob", "");
    assert_eq!(code, Some(0));
    assert!(!stderr.contains("warning"), "no hash, no warning: {stderr}");
}
