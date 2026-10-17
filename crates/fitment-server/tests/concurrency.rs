//! Concurrency (docs/research/m6-api.md, section 10.3): one server per
//! state directory (SD6, VT16's first half).

mod support;

use fitment_server::Server;
use support::{Call, Dirs, Process, TestServer};

fn lease(dirs: &Dirs) -> rusqlite::Connection {
    rusqlite::Connection::open(dirs.state.join("server.db")).unwrap()
}

/// SD6, VT16: a second `fitment-server` on the same state directory
/// refuses to start (exit 2), naming neither host nor path, while the
/// first holds the lease; once the first stops, the second starts.
#[test]
fn a_second_server_on_one_state_is_refused() {
    let dirs = Dirs::new();
    let first = Process::start(&dirs, "", &[]).expect("the first starts");
    let second = Process::start(&dirs, "", &[]);
    let Err(failed) = second else {
        panic!("a second server started on the same state directory");
    };
    assert_eq!(failed.code, Some(2), "{}", failed.stderr);
    let refusal = failed
        .stderr
        .lines()
        .find(|l| l.starts_with("fitment-server: "))
        .unwrap_or_default()
        .to_owned();
    assert!(refusal.contains("holds this state directory"), "{refusal}");
    assert!(
        !refusal.contains(&dirs.root.display().to_string()),
        "{refusal}"
    );
    assert!(!refusal.contains("127.0.0.1"), "{refusal}");
    let (code, _, _) = first.stop();
    assert_eq!(code, Some(0));
    let again = Process::start(&dirs, "", &[]).expect("the second starts once the first stopped");
    assert_eq!(again.stop().0, Some(0));
}

/// SD6: a lease renewed within 30 s by another holder refuses the start;
/// a staler one is taken over (its holder died without releasing it).
#[test]
fn a_stale_lease_is_taken_over() {
    let dirs = Dirs::new();
    drop(Server::build(dirs.config()).unwrap());
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis();
    let now = i64::try_from(now).unwrap();
    let db = lease(&dirs);
    let plant =
        "INSERT OR REPLACE INTO lease (one, holder, heartbeat_ms) VALUES (1, 'l_other', ?1)";
    db.execute(plant, [now - 5_000]).unwrap();
    assert!(
        Server::build(dirs.config()).is_err(),
        "a fresh lease was taken"
    );
    db.execute(plant, [now - 31_000]).unwrap();
    let server = Server::build(dirs.config()).expect("a stale lease is taken over");
    let holder: String = db
        .query_row("SELECT holder FROM lease", [], |r| r.get(0))
        .unwrap();
    assert_ne!(holder, "l_other");
    drop(server);
    let held: i64 = db
        .query_row("SELECT count(*) FROM lease", [], |r| r.get(0))
        .unwrap();
    assert_eq!(held, 0, "a dropped server gives its lease up");
}

/// SD6, OP7: a server whose lease another holder has taken is no longer
/// ready (`503 shutting_down`, `reason: lease`).
#[tokio::test]
async fn a_lost_lease_turns_readiness_off() {
    let server = TestServer::fresh();
    assert_eq!(server.get("/v1/ready").await.status, 204);
    lease(&server.dirs)
        .execute("UPDATE lease SET holder = 'l_other'", [])
        .unwrap();
    let reply = server.send(Call::get("/v1/ready")).await;
    assert_eq!(reply.code(), "shutting_down");
    assert_eq!(reply.json()["details"]["reason"], "lease");
}
