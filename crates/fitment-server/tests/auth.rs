//! Authentication and permissions in process (docs/research/m6-api.md,
//! section 10.5): VT23's matrix over every operation of `OPERATIONS`, and
//! VT24's properties of auth that bound nothing (the bounds are in
//! `hostile.rs`): AU1 to AU6 and AU10's cascade, each of section 4.11's
//! fixes of Vectera's flaws shown.

mod support;

use std::collections::BTreeMap;
use std::time::Duration;

use axum::http::Method;
use fitment_server::auth::Role;
use fitment_server::auth::password::{Pepper, Verified, verify};
use fitment_server::hooks::Action;
use fitment_server::http::operations::{OPERATIONS, Operation, Role as Needed};
use fitment_server::state::UserRow;
use serde_json::json;
use support::{Call, Dirs, TestServer, post, seed_token, seed_user};

/// VT23's principals.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Who {
    None,
    Malformed,
    Expired,
    Revoked,
    Disabled,
    Reader,
    Writer,
    Admin,
    /// A writer's token of role reader.
    WriterAsReader,
}

const WHO: [Who; 9] = [
    Who::None,
    Who::Malformed,
    Who::Expired,
    Who::Revoked,
    Who::Disabled,
    Who::Reader,
    Who::Writer,
    Who::Admin,
    Who::WriterAsReader,
];

/// The password every matrix user holds, where an operation needs one.
const PASSWORD: &str = "the matrix's own password";

/// One operation's server and its principals: each its own user, so one
/// principal's request changes nothing another's reads.
struct World {
    server: TestServer,
    users: BTreeMap<Who, UserRow>,
    tokens: BTreeMap<Who, String>,
    /// A second token of each user: the owner's target for a revocation.
    spare: BTreeMap<Who, String>,
    /// A user no principal is: the non-owner's target.
    other: UserRow,
    other_token: String,
}

fn world(passwords: bool) -> World {
    let server = TestServer::fresh();
    let password = passwords.then_some(PASSWORD);
    let (mut users, mut tokens, mut spare) = (BTreeMap::new(), BTreeMap::new(), BTreeMap::new());
    for who in WHO
        .into_iter()
        .filter(|w| *w != Who::None && *w != Who::Malformed)
    {
        let role = match who {
            Who::Writer | Who::WriterAsReader => Role::Writer,
            Who::Admin => Role::Admin,
            _ => Role::Reader,
        };
        let name = format!("{who:?}").to_ascii_lowercase();
        let user = seed_user(server.server(), &name, role, password);
        let token_role = if who == Who::WriterAsReader {
            Role::Reader
        } else {
            role
        };
        let days = if who == Who::Expired { -1 } else { 30 };
        let (row, token) = seed_token(server.server(), &user, token_role, days);
        let (spare_row, spare_token) = seed_token(server.server(), &user, token_role, 30);
        let seams = server.server().seams();
        match who {
            Who::Revoked => {
                seams.db.revoke(row.key, seams.ids.now_ms()).unwrap();
            }
            Who::Disabled => {
                seams
                    .db
                    .update_user(&user.id, None, Some(true), true, seams.ids.now_ms())
                    .unwrap();
            }
            _ => {}
        }
        let _ = spare_row;
        users.insert(who, user);
        tokens.insert(who, token);
        spare.insert(who, spare_token);
    }
    let other = seed_user(server.server(), "other", Role::Reader, None);
    let (other_row, other_token) = seed_token(server.server(), &other, Role::Reader, 30);
    let _ = other_row;
    World {
        server,
        users,
        tokens,
        spare,
        other,
        other_token,
    }
}

/// The token id of a secret.
fn token_id(world: &World, secret: &str) -> String {
    let sha = fitment_core::sha256_hex(secret.as_bytes());
    let seams = world.server.server().seams();
    seams.db.presented(&sha).unwrap().unwrap().token.id
}

/// The request `who` sends for `op`: well formed, so only its credentials
/// decide the answer. `owner`: the target is the principal's own.
fn request(op: &Operation, world: &World, who: Who, owner: bool) -> Call {
    let username = format!("{who:?}").to_ascii_lowercase();
    let call = match op.id {
        "info" | "health" | "ready" | "openapi" | "me" | "tokens_list" | "users_list" => {
            Call::get(op.path)
        }
        "login" => Call::json(
            Method::POST,
            op.path,
            &json!({ "username": "other", "password": PASSWORD }),
        ),
        "password" => Call::json(
            Method::POST,
            op.path,
            &json!({ "current": PASSWORD, "new": "a new password for the matrix" }),
        ),
        "tokens_mint" => Call::json(Method::POST, op.path, &json!({ "name": "minted" })),
        "tokens_revoke" => {
            let target = match world.spare.get(&who) {
                Some(spare) if owner => token_id(world, spare),
                _ => token_id(world, &world.other_token),
            };
            Call::new(Method::DELETE, &op.path.replace("{id}", &target))
        }
        "users_create" => Call::json(
            Method::POST,
            op.path,
            &json!({ "username": format!("new-{username}"), "role": "reader" }),
        ),
        "users_update" => Call::json(
            Method::PATCH,
            &op.path.replace("{id}", &world.other.id),
            &json!({ "disabled": false }),
        ),
        other => panic!("VT23 has no row for the operation {other} (XH14)"),
    };
    match who {
        Who::None => call.anonymous(),
        Who::Malformed => call.header("authorization", "Bearer fitment_pat_not-a-token"),
        _ => call.bearer(&world.tokens[&who]),
    }
}

/// The effective role of a valid principal, `None` for one without valid
/// credentials.
fn effective(who: Who) -> Option<Role> {
    match who {
        Who::Reader | Who::WriterAsReader => Some(Role::Reader),
        Who::Writer => Some(Role::Writer),
        Who::Admin => Some(Role::Admin),
        _ => None,
    }
}

/// VT23 (AU5, AU8, XH14): for every operation of `OPERATIONS` and every
/// principal, the status its role implies: `401 unauthenticated` without
/// valid credentials, `403 forbidden` below its role, neither at or above
/// it; health, readiness, OpenAPI and login need none; a non-owner's
/// token is `404` but to an admin. The login user `other` has the
/// password where login is the operation.
#[tokio::test]
async fn every_operation_answers_each_principal_as_its_role_says() {
    let mut failures = Vec::new();
    for op in &OPERATIONS {
        let world = world(matches!(op.id, "password" | "login"));
        if op.id == "login" {
            let seams = world.server.server().seams();
            let phc = fitment_server::auth::password::hash(PASSWORD, seams.auth.pepper()).unwrap();
            seams
                .db
                .set_password(world.other.key, &phc, None, seams.ids.now_ms())
                .unwrap();
        }
        for who in WHO {
            let reply = world.server.send(request(op, &world, who, true)).await;
            let status = reply.status.as_u16();
            let expected = match (op.role, effective(who)) {
                (Needed::None, _) => None,
                (_, None) => Some((401, "unauthenticated")),
                (needed, Some(role)) if !role.permits(needed) => Some((403, "forbidden")),
                _ => None,
            };
            match expected {
                Some((want, code)) => {
                    if status != want || reply.code() != code {
                        failures.push(format!("{} {who:?}: {status}, not {want} {code}", op.id));
                    } else if want == 401 && reply.header("www-authenticate") != Some("Bearer") {
                        failures.push(format!("{} {who:?}: 401 without WWW-Authenticate", op.id));
                    }
                }
                None if status == 401 || status == 403 => {
                    failures.push(format!("{} {who:?}: {status} at or above its role", op.id));
                }
                None if status >= 400 => {
                    failures.push(format!("{} {who:?}: {status} {}", op.id, reply.code()));
                }
                None => {}
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// VT23, AU8: a token of another user is `404` to its non-owner, the
/// answer for one that does not exist, and an admin revokes it; an admin's
/// `?owner=` is honoured on `GET` and refused on every other method (`400
/// invalid_parameter`).
#[tokio::test]
async fn owners_and_admins_reach_other_users_tokens_as_au8_says() {
    let world = world(false);
    let op = OPERATIONS.iter().find(|o| o.id == "tokens_revoke").unwrap();
    for who in [Who::Reader, Who::Writer, Who::WriterAsReader] {
        let reply = world.server.send(request(op, &world, who, false)).await;
        assert_eq!(
            (reply.status.as_u16(), reply.code()),
            (404, "not_found".into()),
            "{who:?}"
        );
    }
    let missing = Call::new(
        Method::DELETE,
        "/v1/auth/tokens/t_7zzzzzzzzzzzzzzzzzzzzzzzzz",
    )
    .bearer(&world.tokens[&Who::Reader]);
    let unknown = world.server.send(missing).await;
    assert_eq!(unknown.code(), "not_found");
    let reply = world
        .server
        .send(request(op, &world, Who::Admin, false))
        .await;
    assert_eq!(reply.status, 204, "an admin revokes another's token");
    let other = Call::get("/v1/auth/me").bearer(&world.other_token);
    assert_eq!(world.server.send(other).await.status, 401);
    let admin = &world.tokens[&Who::Admin];
    let owner = &world.users[&Who::Reader].id;
    let listed = world
        .server
        .send(Call::get(&format!("/v1/auth/tokens?owner={owner}")).bearer(admin))
        .await;
    assert_eq!(listed.status, 200);
    assert_eq!(listed.json()["items"].as_array().unwrap().len(), 2);
    let reader = &world.tokens[&Who::Reader];
    let refused = world
        .server
        .send(Call::get(&format!("/v1/auth/tokens?owner={}", world.other.id)).bearer(reader))
        .await;
    assert_eq!(refused.code(), "forbidden");
    let unknown = world
        .server
        .send(Call::get("/v1/auth/tokens?owner=u_7zzzzzzzzzzzzzzzzzzzzzzzzz").bearer(admin))
        .await;
    assert_eq!(unknown.code(), "not_found");
    for (method, path) in [
        (Method::POST, "/v1/auth/tokens"),
        (Method::POST, "/v1/auth/password"),
        (Method::DELETE, "/v1/auth/tokens/current"),
        (Method::PATCH, "/v1/auth/users/u_7zzzzzzzzzzzzzzzzzzzzzzzzz"),
        (Method::POST, "/v1/auth/users"),
    ] {
        let at = format!("{path}?owner={owner}");
        let call = if method == Method::DELETE {
            Call::new(method.clone(), &at)
        } else {
            Call::json(method.clone(), &at, &json!({}))
        };
        let reply = world.server.send(call.bearer(admin)).await;
        assert_eq!(reply.code(), "invalid_parameter", "{method} {path}");
    }
}

/// AU2, fix 1 (VT24): a login runs one argon2 verification whether the
/// username exists, is disabled, has no password or is the `system` user,
/// and every failure is the same `401 invalid_credentials`.
#[tokio::test]
async fn a_login_costs_one_hash_whatever_the_username() {
    let server = TestServer::fresh();
    let password = support::password("ada's real password");
    let ada = seed_user(server.server(), "ada", Role::Reader, Some(&password));
    seed_user(server.server(), "eve", Role::Reader, Some(&password));
    let seams = server.server().seams();
    let eve = seams.db.user_by_fold("eve").unwrap().unwrap();
    seams
        .db
        .update_user(&eve.id, None, Some(true), true, seams.ids.now_ms())
        .unwrap();
    seed_user(server.server(), "tok", Role::Reader, None);
    let mut answers = Vec::new();
    for username in ["ada", "nobody", "eve", "tok", "system", "not a username!"] {
        let before = seams.auth.hashes();
        let wrong = support::password("a wrong password!");
        let body = json!({ "username": username, "password": wrong });
        let reply = server
            .send(Call::json(Method::POST, "/v1/auth/login", &body))
            .await;
        assert_eq!(seams.auth.hashes(), before + 1, "{username}");
        let mut doc = reply.json();
        doc["request_id"] = json!(null);
        answers.push((reply.status.as_u16(), doc));
    }
    assert!(answers.windows(2).all(|w| w[0] == w[1]), "{answers:?}");
    assert_eq!(answers[0].0, 401);
    assert_eq!(answers[0].1["code"], "invalid_credentials");
    let body = json!({ "username": "ADA", "password": password });
    let reply = server
        .send(Call::json(Method::POST, "/v1/auth/login", &body))
        .await;
    assert_eq!(reply.status, 201);
    let token = reply.json();
    assert_eq!(token["parent"], json!(null));
    assert_eq!(token["role"], "reader");
    let _ = ada;
}

/// Every text value of every table of `server.db`.
fn every_value(dirs: &Dirs) -> Vec<String> {
    let db = rusqlite::Connection::open_with_flags(
        dirs.state.join("server.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let tables: Vec<String> = db
        .prepare("SELECT name FROM sqlite_schema WHERE type = 'table'")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let mut values = Vec::new();
    for table in tables {
        let mut statement = db.prepare(&format!("SELECT * FROM \"{table}\"")).unwrap();
        let columns = statement.column_count();
        let mut rows = statement.query([]).unwrap();
        while let Some(row) = rows.next().unwrap() {
            for i in 0..columns {
                if let Ok(text) = row.get::<_, String>(i) {
                    values.push(text);
                }
            }
        }
    }
    values
}

/// AU3, AU1 (VT24): tokens are in `server.db` only as SHA-256 and their
/// hint, passwords only as PHC strings; neither reaches a log (XH13, the
/// harness's scan at the server's end).
#[tokio::test]
async fn secrets_are_stored_only_as_hashes() {
    let server = TestServer::fresh();
    let password = support::password("the stored password");
    let user = seed_user(server.server(), "ada", Role::Writer, Some(&password));
    let body = json!({ "username": "ada", "password": password });
    let login = server
        .send(Call::json(Method::POST, "/v1/auth/login", &body))
        .await
        .json();
    let secret = login["secret"].as_str().unwrap().to_owned();
    let minted = server
        .send(post(
            "/v1/auth/tokens",
            &secret,
            &json!({ "name": "ci", "role": "reader" }),
        ))
        .await
        .json();
    let child = minted["secret"].as_str().unwrap().to_owned();
    let values = every_value(&server.dirs);
    for plain in [&secret, &child, &password] {
        let body = plain.strip_prefix("fitment_pat_").unwrap_or(plain);
        assert!(!values.iter().any(|v| v.contains(body)), "stored in plain");
    }
    for token in [&secret, &child] {
        let sha = fitment_core::sha256_hex(token.as_bytes());
        assert!(values.contains(&sha), "the SHA-256 is the stored name");
    }
    let phc = values.iter().find(|v| v.starts_with("$argon2id$")).unwrap();
    assert!(phc.starts_with("$argon2id$v=19$m=19456,t=2,p=1$"), "{phc}");
    let listed = server
        .send(Call::get("/v1/auth/tokens").bearer(&secret))
        .await;
    assert!(
        listed.json()["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|t| t["secret"].is_null())
    );
    let _ = user;
}

/// AU6, fix 5 (VT24): an authenticated read writes nothing to `server.db`
/// (its `data_version` seen from another connection does not move); the
/// last uses are batched and written by the sweep.
#[tokio::test]
async fn authentication_writes_nothing() {
    let server = TestServer::fresh();
    let user = seed_user(server.server(), "ada", Role::Reader, None);
    let (row, token) = seed_token(server.server(), &user, Role::Reader, 30);
    let watcher = rusqlite::Connection::open(server.dirs.state.join("server.db")).unwrap();
    let version = |c: &rusqlite::Connection| -> i64 {
        c.query_row("PRAGMA data_version", [], |r| r.get(0))
            .unwrap()
    };
    let before = version(&watcher);
    for path in ["/v1/auth/me", "/v1/info", "/v1/auth/tokens", "/v1/auth/me"] {
        assert_eq!(
            server.send(Call::get(path).bearer(&token)).await.status,
            200,
            "{path}"
        );
    }
    assert_eq!(version(&watcher), before, "a request wrote to server.db");
    let stored: Option<i64> = watcher
        .query_row(
            "SELECT last_used_ms FROM token WHERE key = ?1",
            [row.key],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(stored, None);
    server.server().sweep_auth().unwrap();
    assert_ne!(version(&watcher), before);
    let stored: Option<i64> = watcher
        .query_row(
            "SELECT last_used_ms FROM token WHERE key = ?1",
            [row.key],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(stored, Some(server.server().seams().ids.now_ms()));
}

/// AU4, AU5 (VT24): a revoked token and a disabled user are refused at the
/// next request, with no cache; disabling revokes every token of the user,
/// and enabling restores none (review); a token's effective role is capped by its
/// user's, and demoting the user demotes every token at once.
#[tokio::test]
async fn revocation_disabling_and_demotion_take_effect_at_once() {
    let server = TestServer::fresh();
    let admin = seed_user(server.server(), "root", Role::Admin, None);
    let (_, root) = seed_token(server.server(), &admin, Role::Admin, 30);
    let ada = seed_user(server.server(), "ada", Role::Admin, None);
    let (_, token) = seed_token(server.server(), &ada, Role::Admin, 30);
    let (_, second) = seed_token(server.server(), &ada, Role::Admin, 30);
    assert_eq!(
        server
            .send(Call::get("/v1/auth/users").bearer(&token))
            .await
            .status,
        200
    );
    let demote = Call::json(
        Method::PATCH,
        &format!("/v1/auth/users/{}", ada.id),
        &json!({ "role": "reader" }),
    );
    let reply = server.send(demote.bearer(&root)).await;
    assert_eq!(reply.json()["role"], "reader");
    for t in [&token, &second] {
        let reply = server.send(Call::get("/v1/auth/users").bearer(t)).await;
        assert_eq!(reply.code(), "forbidden");
        let me = server.send(Call::get("/v1/auth/me").bearer(t)).await.json();
        assert_eq!(
            (me["role"].clone(), me["token"]["role"].clone()),
            (json!("reader"), json!("reader"))
        );
    }
    let logout = Call::new(Method::DELETE, "/v1/auth/tokens/current").bearer(&token);
    assert_eq!(server.send(logout).await.status, 204);
    assert_eq!(
        server
            .send(Call::get("/v1/auth/me").bearer(&token))
            .await
            .code(),
        "unauthenticated"
    );
    assert_eq!(
        server
            .send(Call::get("/v1/auth/me").bearer(&second))
            .await
            .status,
        200
    );
    let disable = Call::json(
        Method::PATCH,
        &format!("/v1/auth/users/{}", ada.id),
        &json!({ "disabled": true }),
    );
    assert_eq!(
        server.send(disable.bearer(&root)).await.json()["disabled"],
        true
    );
    assert_eq!(
        server
            .send(Call::get("/v1/auth/me").bearer(&second))
            .await
            .code(),
        "unauthenticated"
    );
    let enable = Call::json(
        Method::PATCH,
        &format!("/v1/auth/users/{}", ada.id),
        &json!({ "disabled": false }),
    );
    assert_eq!(server.send(enable.bearer(&root)).await.status, 200);
    // Review: disabling revoked every token; enabling restores none.
    let me = server.send(Call::get("/v1/auth/me").bearer(&second)).await;
    assert_eq!(me.code(), "unauthenticated");
    let listed = server
        .send(Call::get(&format!("/v1/auth/tokens?owner={}", ada.id)).bearer(&root))
        .await
        .json();
    let items = listed["items"].as_array().unwrap();
    assert!(!items.is_empty() && items.iter().all(|t| !t["revoked"].is_null()));
}

/// AU4, fix 11 (VT24): every token expires: a login's in
/// `auth.token_default_days` (90) unless asked, at most
/// `auth.token_max_days` (365); past its expiry it is refused.
#[tokio::test]
async fn every_token_expires() {
    let server = TestServer::fresh();
    let password = support::password("an expiring password");
    seed_user(server.server(), "ada", Role::Reader, Some(&password));
    let login = |days: Option<u32>| {
        let mut body = json!({ "username": "ada", "password": password });
        if let Some(days) = days {
            body["days"] = json!(days);
        }
        Call::json(Method::POST, "/v1/auth/login", &body)
    };
    let now = server.server().seams().ids.now_ms();
    let default = server.send(login(None)).await.json();
    assert_eq!(
        default["expires"],
        fitment_server::ids::rfc3339(now + 90 * 86_400_000)
    );
    let year = server.send(login(Some(365))).await.json();
    assert_eq!(
        year["expires"],
        fitment_server::ids::rfc3339(now + 365 * 86_400_000)
    );
    for days in [0, 366] {
        let reply = server.send(login(Some(days))).await;
        assert_eq!(reply.code(), "invalid_parameter", "{days}");
    }
    let token = default["secret"].as_str().unwrap().to_owned();
    assert_eq!(
        server
            .send(Call::get("/v1/auth/me").bearer(&token))
            .await
            .status,
        200
    );
    server.server().seams().ids.fix_time(now + 90 * 86_400_000);
    assert_eq!(
        server
            .send(Call::get("/v1/auth/me").bearer(&token))
            .await
            .code(),
        "unauthenticated"
    );
}

/// AU3 (VT24): a token is taken only from the `Authorization` header; one
/// in a query string is refused on every route, a valid header beside it
/// or not, and a header that is not a bearer token is `401`.
#[tokio::test]
async fn tokens_are_taken_from_the_header_only() {
    let server = TestServer::fresh();
    let token = server.token().to_owned();
    for path in [
        format!("/v1/auth/me?access_token={token}"),
        format!("/v1/info?token={token}"),
        format!("/v1/health?x={}", token.replace('_', "%5F")),
        "/v1/health?x=fitment_pat_%zz".to_owned(),
        "/v1/auth/tokens?after=fitment%5Fpat%5F%ff".to_owned(),
        "/v1/ready?access_token=x".to_owned(),
    ] {
        let reply = server.send(Call::get(&path).bearer(&token)).await;
        assert_eq!(reply.code(), "bad_request", "{path}");
    }
    for header in [
        format!("Basic {token}"),
        token.clone(),
        format!("Bearer {}", &token[..token.len() - 1]),
        format!("Bearer {token}, Bearer {token}"),
    ] {
        let call = Call::get("/v1/auth/me").header("authorization", &header);
        assert_eq!(server.send(call).await.code(), "unauthenticated");
    }
    let twice = Call::get("/v1/auth/me")
        .header("authorization", &format!("Bearer {token}"))
        .header("authorization", &format!("Bearer {token}"));
    assert_eq!(server.send(twice).await.code(), "unauthenticated");
}

/// AU4, AU10 (VT24): revoking a token revokes every token minted from it,
/// two generations down, and no other; a password change revokes every
/// other token of the user and keeps the presenting one.
#[tokio::test]
async fn revocation_cascades_and_a_password_change_revokes_the_rest() {
    let server = TestServer::fresh();
    let password = support::password("the cascade's password");
    seed_user(server.server(), "ada", Role::Writer, Some(&password));
    let body = json!({ "username": "ada", "password": password });
    let login = || Call::json(Method::POST, "/v1/auth/login", &body);
    let root = server.send(login()).await.json()["secret"]
        .as_str()
        .unwrap()
        .to_owned();
    let sibling = server.send(login()).await.json()["secret"]
        .as_str()
        .unwrap()
        .to_owned();
    let mint = |parent: String| post("/v1/auth/tokens", &parent, &json!({ "name": "child" }));
    let child = server.send(mint(root.clone())).await.json();
    let child_secret = child["secret"].as_str().unwrap().to_owned();
    let grandchild = server.send(mint(child_secret.clone())).await.json();
    assert_eq!(grandchild["parent"], child["id"]);
    let grandchild = grandchild["secret"].as_str().unwrap().to_owned();
    let nephew = server.send(mint(sibling.clone())).await.json()["secret"]
        .as_str()
        .unwrap()
        .to_owned();
    let me = |t: &str| server.send(Call::get("/v1/auth/me").bearer(t));
    let revoke = Call::new(Method::DELETE, "/v1/auth/tokens/current").bearer(&root);
    assert_eq!(server.send(revoke).await.status, 204);
    for gone in [&root, &child_secret, &grandchild] {
        assert_eq!(me(gone).await.code(), "unauthenticated");
    }
    for live in [&sibling, &nephew] {
        assert_eq!(me(live).await.status, 200);
    }
    let change =
        json!({ "current": password, "new": support::password("the cascade's new password") });
    let reply = server
        .send(post("/v1/auth/password", &nephew, &change))
        .await;
    assert_eq!(reply.status, 204);
    assert_eq!(me(&nephew).await.status, 200, "the presenting token stays");
    assert_eq!(
        me(&sibling).await.code(),
        "unauthenticated",
        "every other token goes"
    );
    let old = server
        .send(Call::json(Method::POST, "/v1/auth/login", &body))
        .await;
    assert_eq!(old.code(), "invalid_credentials");
    let wrong = json!({ "current": password, "new": "yet another password here" });
    assert_eq!(
        server
            .send(post("/v1/auth/password", &nephew, &wrong))
            .await
            .code(),
        "invalid_credentials"
    );
}

/// AU1, fix 6: the pepper keys argon2: a password set under one pepper
/// logs in under it and not under another, and the pepper is in no log
/// (XH13: registered with the scan).
#[tokio::test]
async fn the_pepper_keys_every_hash() {
    let dirs = Dirs::new();
    let pepper = dirs.root.join("pepper");
    let text = "PEPPER-the-tests-own-0123456789";
    support::scan::secret(text);
    std::fs::write(&pepper, format!("{text}\n")).unwrap();
    #[cfg(unix)]
    std::fs::set_permissions(&pepper, std::os::unix::fs::PermissionsExt::from_mode(0o600)).unwrap();
    let mut config = dirs.config();
    config.auth.pepper_file = pepper.display().to_string();
    let server = TestServer::in_process(dirs, config.clone());
    let password = support::password("a peppered password");
    let admin = server.token().to_owned();
    let create = json!({ "username": "ada", "role": "reader", "password": password });
    assert_eq!(
        server
            .send(post("/v1/auth/users", &admin, &create))
            .await
            .status,
        201
    );
    let login = json!({ "username": "ada", "password": password });
    let reply = server
        .send(Call::json(Method::POST, "/v1/auth/login", &login))
        .await;
    assert_eq!(reply.status, 201);
    let phc = server
        .server()
        .seams()
        .db
        .user_by_fold("ada")
        .unwrap()
        .unwrap()
        .password_phc
        .unwrap();
    assert_eq!(verify(&password, &Pepper::none(), &phc), Verified::NoMatch);
    assert_eq!(
        verify(&password, &Pepper::new(text.as_bytes().to_vec()), &phc),
        Verified::Match { rehash: false }
    );
    #[cfg(unix)]
    {
        std::fs::set_permissions(&pepper, std::os::unix::fs::PermissionsExt::from_mode(0o644))
            .unwrap();
        let error = fitment_server::check(&config).unwrap_err();
        assert_eq!(error.exit_code(), 2, "{error}");
    }
}

/// AU1, fix 9: a password stored under older parameters verifies, and the
/// login stores it again under this build's, recorded in the PHC string.
#[tokio::test]
async fn older_parameters_are_rehashed_at_login() {
    let server = TestServer::fresh();
    let password = support::password("an old-policy password");
    let seams = server.server().seams();
    let old =
        fitment_server::auth::password::hash_with_old_parameters(&password, seams.auth.pepper())
            .unwrap();
    let ada = seed_user(server.server(), "ada", Role::Reader, None);
    seams
        .db
        .set_password(ada.key, &old, None, seams.ids.now_ms())
        .unwrap();
    let login = json!({ "username": "ada", "password": password });
    assert_eq!(
        server
            .send(Call::json(Method::POST, "/v1/auth/login", &login))
            .await
            .status,
        201
    );
    let phc = seams
        .db
        .user_by_fold("ada")
        .unwrap()
        .unwrap()
        .password_phc
        .unwrap();
    assert_ne!(phc, old);
    assert!(phc.starts_with("$argon2id$v=19$m=19456,t=2,p=1$"), "{phc}");
    assert_eq!(
        server
            .send(Call::json(Method::POST, "/v1/auth/login", &login))
            .await
            .status,
        201
    );
}

/// AU2, fix 8: argon2 runs on the blocking pool, at most
/// `auth.hash_concurrency` at once; a login that waits past
/// `auth.hash_wait_seconds` for a slot is `503 request_timeout`, and the
/// runtime keeps answering meanwhile.
#[tokio::test(flavor = "multi_thread")]
async fn argon2_runs_on_the_blocking_pool_two_at_a_time() {
    let dirs = Dirs::new();
    let mut config = dirs.config();
    config.auth.hash_wait_seconds = 1;
    let server = std::sync::Arc::new(TestServer::in_process(dirs, config));
    let password = support::password("a slow password!");
    seed_user(server.server(), "ada", Role::Reader, Some(&password));
    let seams = server.server().seams();
    seams
        .hooks
        .set("auth.hash", Action::Delay(Duration::from_millis(1500)));
    let mut calls = Vec::new();
    for _ in 0..4 {
        let (server, password) = (std::sync::Arc::clone(&server), password.clone());
        calls.push(tokio::spawn(async move {
            let body = json!({ "username": "ada", "password": password });
            server
                .send(Call::json(Method::POST, "/v1/auth/login", &body))
                .await
        }));
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        server.get("/v1/health").await.status,
        204,
        "the runtime answers"
    );
    let mut statuses: Vec<u16> = Vec::new();
    for call in calls {
        let reply = call.await.unwrap();
        if reply.status == 503 {
            assert_eq!(reply.code(), "request_timeout");
            assert_eq!(reply.json()["retryable"], false);
        }
        statuses.push(reply.status.as_u16());
    }
    statuses.sort_unstable();
    assert_eq!(statuses, [201, 201, 503, 503]);
    assert_eq!(server.server().seams().auth.most_hashing_at_once(), 2);
}

/// Audit flaw 4: tokens expired or revoked 30 days ago are swept; a live
/// one, and a revoked one that a live token was minted from (a password
/// change keeps the presenting token, AU4), stay.
#[tokio::test]
async fn dead_tokens_are_swept_after_30_days() {
    let server = TestServer::fresh();
    let seams = server.server().seams();
    let ada = seed_user(server.server(), "ada", Role::Reader, None);
    let (revoked, _) = seed_token(server.server(), &ada, Role::Reader, 30);
    seams.db.revoke(revoked.key, seams.ids.now_ms()).unwrap();
    seed_token(server.server(), &ada, Role::Reader, 1);
    let (live, _) = seed_token(server.server(), &ada, Role::Reader, 365);
    let password = support::password("bob's own password");
    let bob = seed_user(server.server(), "bob", Role::Reader, Some(&password));
    let (parent, parent_secret) = seed_token(server.server(), &bob, Role::Reader, 365);
    let child = server
        .send(post(
            "/v1/auth/tokens",
            &parent_secret,
            &json!({ "name": "child" }),
        ))
        .await
        .json();
    let child_secret = child["secret"].as_str().unwrap().to_owned();
    let change = json!({ "current": password, "new": support::password("bob's next password") });
    assert_eq!(
        server
            .send(post("/v1/auth/password", &child_secret, &change))
            .await
            .status,
        204
    );
    let child = seams
        .db
        .token_by_id(child["id"].as_str().unwrap())
        .unwrap()
        .unwrap();
    let now = seams.ids.now_ms();
    seams.ids.fix_time(now + 29 * 86_400_000);
    assert_eq!(server.server().sweep_auth().unwrap(), 0, "within 30 days");
    seams.ids.fix_time(now + 32 * 86_400_000);
    assert_eq!(
        server.server().sweep_auth().unwrap(),
        2,
        "the revoked and the expired one"
    );
    let watcher = rusqlite::Connection::open(server.dirs.state.join("server.db")).unwrap();
    let left: Vec<i64> = watcher
        .prepare("SELECT key FROM token ORDER BY key")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(left, [live.key, parent.key, child.key]);
}

/// OP1, AU6: a request's log span names its user by id, never by username,
/// and holds no token.
#[tokio::test]
async fn the_log_names_the_user_by_id_only() {
    support::logs::capture();
    let server = TestServer::fresh();
    let user = seed_user(server.server(), "zelda-of-the-log", Role::Reader, None);
    let (_, token) = seed_token(server.server(), &user, Role::Reader, 30);
    assert_eq!(
        server
            .send(Call::get("/v1/auth/me").bearer(&token))
            .await
            .status,
        200
    );
    let logs = support::logs::text();
    assert!(
        logs.contains(&format!("\"user\":\"{}\"", user.id)),
        "the span names the user's id"
    );
    assert!(
        !logs.contains("zelda-of-the-log"),
        "the log names the username"
    );
    assert_eq!(support::scan::logs(&logs), Vec::<String>::new());
}

/// AU10 (review): a root token (a login's, no parent) lists and revokes
/// every token of its user; a minted token itself and what was minted from
/// it only, so a child cannot revoke its parent or a sibling.
#[tokio::test]
async fn a_minted_token_reaches_only_its_own_subtree() {
    let server = TestServer::fresh();
    let ada = seed_user(server.server(), "ada", Role::Reader, None);
    let (_, root) = seed_token(server.server(), &ada, Role::Reader, 30);
    let mint = |parent: &str, name: &str| post("/v1/auth/tokens", parent, &json!({ "name": name }));
    let secret = |v: &serde_json::Value| v["secret"].as_str().unwrap().to_owned();
    let child = server.send(mint(&root, "child")).await.json();
    let grandchild = server
        .send(mint(&secret(&child), "grandchild"))
        .await
        .json();
    let sibling = server.send(mint(&root, "sibling")).await.json();
    let names = |reply: support::Reply| -> Vec<String> {
        let mut names: Vec<String> = reply.json()["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_owned())
            .collect();
        names.sort();
        names
    };
    let seen = names(
        server
            .send(Call::get("/v1/auth/tokens").bearer(&secret(&child)))
            .await,
    );
    assert_eq!(seen, ["child", "grandchild"]);
    let seen = names(
        server
            .send(Call::get("/v1/auth/tokens").bearer(&root))
            .await,
    );
    assert_eq!(seen, ["child", "grandchild", "seeded", "sibling"]);
    let revoke = |by: &str, id: &serde_json::Value| {
        Call::new(
            Method::DELETE,
            &format!("/v1/auth/tokens/{}", id.as_str().unwrap()),
        )
        .bearer(by)
    };
    let root_id = server
        .send(Call::get("/v1/auth/me").bearer(&root))
        .await
        .json()["token"]["id"]
        .clone();
    for (target, what) in [(&root_id, "its parent"), (&sibling["id"], "a sibling")] {
        let reply = server.send(revoke(&secret(&child), target)).await;
        assert_eq!(reply.code(), "not_found", "a child revoked {what}");
    }
    assert_eq!(
        server
            .send(revoke(&secret(&child), &grandchild["id"]))
            .await
            .status,
        204
    );
    assert_eq!(server.send(revoke(&root, &sibling["id"])).await.status, 204);
    let me = server
        .send(Call::get("/v1/auth/me").bearer(&secret(&sibling)))
        .await;
    assert_eq!(me.code(), "unauthenticated");
}

/// Review: the API refuses to disable or demote the last enabled admin
/// (`409 last_admin`); with a second admin it may.
#[tokio::test]
async fn the_last_enabled_admin_stays() {
    let server = TestServer::fresh();
    let root = seed_user(server.server(), "root", Role::Admin, None);
    let (_, token) = seed_token(server.server(), &root, Role::Admin, 30);
    let patch = |body: serde_json::Value| {
        Call::json(Method::PATCH, &format!("/v1/auth/users/{}", root.id), &body).bearer(&token)
    };
    let harness = server.token().to_owned();
    let other = server
        .send(Call::get("/v1/auth/me").bearer(&harness))
        .await
        .json()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let off = Call::json(
        Method::PATCH,
        &format!("/v1/auth/users/{other}"),
        &json!({ "disabled": true }),
    );
    assert_eq!(
        server.send(off.bearer(&token)).await.status,
        200,
        "another admin, not the last"
    );
    for body in [json!({ "role": "writer" }), json!({ "disabled": true })] {
        let reply = server.send(patch(body.clone())).await;
        assert_eq!(
            (reply.status.as_u16(), reply.code()),
            (409, "last_admin".into()),
            "{body}"
        );
    }
    assert_eq!(
        server.send(patch(json!({ "role": "admin" }))).await.status,
        200
    );
}

/// AU4 (review): two concurrent password changes from two tokens of one
/// user: one wins; the other, whose token the winner revoked, is refused
/// in its own transaction, never setting its password after the first.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn of_two_concurrent_password_changes_one_wins() {
    let server = std::sync::Arc::new(TestServer::fresh());
    let password = support::password("the shared old password");
    let ada = seed_user(server.server(), "ada", Role::Reader, Some(&password));
    let (_, a) = seed_token(server.server(), &ada, Role::Reader, 30);
    let (_, b) = seed_token(server.server(), &ada, Role::Reader, 30);
    let hooks = &server.server().seams().hooks;
    hooks.set("auth.hash", Action::Delay(Duration::from_millis(400)));
    let mut changes = Vec::new();
    for (token, new) in [
        (a, "the first new password"),
        (b, "the second new password"),
    ] {
        let (server, password) = (std::sync::Arc::clone(&server), password.clone());
        let new = support::password(new);
        changes.push(tokio::spawn(async move {
            let body = json!({ "current": password, "new": new });
            (
                server.send(post("/v1/auth/password", &token, &body)).await,
                new,
            )
        }));
    }
    let mut won = Vec::new();
    for change in changes {
        let (reply, new) = change.await.unwrap();
        match reply.status.as_u16() {
            204 => won.push(new),
            _ => assert_eq!(reply.code(), "unauthenticated"),
        }
    }
    assert_eq!(won.len(), 1, "exactly one change wins");
    server.server().seams().hooks.clear("auth.hash");
    let login = json!({ "username": "ada", "password": won[0] });
    let reply = server
        .send(Call::json(Method::POST, "/v1/auth/login", &login))
        .await;
    assert_eq!(reply.status, 201, "the winner's password is the one set");
}

/// JS9, review: the sweep deletes in batches of `SWEEP_BATCH` keys, each
/// its own transaction, in one pass whatever the chains of minted tokens:
/// 20,000 dead tokens, a chain of 1,000 among them, go within a bound.
#[tokio::test]
async fn the_sweep_is_linear_in_the_tokens() {
    let server = TestServer::fresh();
    let ada = seed_user(server.server(), "ada", Role::Reader, None);
    let now = server.server().seams().ids.now_ms();
    let mut db = rusqlite::Connection::open(server.dirs.state.join("server.db")).unwrap();
    let tx = db.transaction().unwrap();
    let mut parent: Option<i64> = None;
    for i in 0..20_000_i64 {
        tx.execute(
            "INSERT INTO token (id, user, name, secret_sha256, hint, role, parent, created_ms, expires_ms)
             VALUES (?1, ?2, 'dead', ?3, 'hint', 'reader', ?4, ?5, ?5)",
            rusqlite::params![format!("t_dead{i:022}"), ada.key, format!("{i:064}"), parent, now - 1],
        )
        .unwrap();
        parent = (i < 1000).then(|| tx.last_insert_rowid());
    }
    tx.commit().unwrap();
    server.server().seams().ids.fix_time(now + 31 * 86_400_000);
    let started = std::time::Instant::now();
    assert_eq!(server.server().sweep_auth().unwrap(), 20_000);
    let took = started.elapsed();
    assert!(took < Duration::from_secs(20), "{took:?}");
    let left: i64 = db
        .query_row("SELECT count(*) FROM token", [], |r| r.get(0))
        .unwrap();
    assert_eq!(left, 0);
    assert_eq!(fitment_server::state::db::SWEEP_BATCH, 500);
}

/// AU7, HX7 (review): `X-Forwarded-For` entries with a port are read
/// (`1.2.3.4:5678`, `[::1]:5678`); from a peer that is not a trusted proxy
/// the header is ignored with one warning a minute, naming no value.
#[tokio::test]
async fn forwarded_addresses_with_ports_and_untrusted_peers() {
    use fitment_server::http::extract::forwarded_address;
    assert_eq!(forwarded_address(" 1.2.3.4:5678"), "1.2.3.4".parse().ok());
    assert_eq!(forwarded_address("[::1]:5678"), "::1".parse().ok());
    assert_eq!(
        forwarded_address("[::ffff:10.0.0.1]:1"),
        "10.0.0.1".parse().ok()
    );
    assert_eq!(forwarded_address("2001:db8::1"), "2001:db8::1".parse().ok());
    assert_eq!(forwarded_address("unknown"), None);
    support::logs::capture();
    let server = TestServer::fresh();
    let peer: std::net::SocketAddr = "192.0.2.77:4000".parse().unwrap();
    for _ in 0..3 {
        let call = Call::get("/v1/health")
            .peer(peer)
            .header("x-forwarded-for", "203.0.113.200, 198.51.100.200");
        assert_eq!(server.send(call).await.status, 204);
    }
    let logs = support::logs::text();
    let warned = logs
        .lines()
        .filter(|l| l.contains("X-Forwarded-For from a peer"))
        .count();
    assert!(warned >= 1, "no warning");
    let mine: Vec<&str> = logs
        .lines()
        .filter(|l| l.contains("X-Forwarded-For from a peer"))
        .collect();
    assert!(
        mine.iter()
            .all(|l| !l.contains("203.0.113.200") && !l.contains("192.0.2.77"))
    );
    assert!(warned <= 2, "rate-limited: {warned}");
}
