//! `server.db` and the state directory (docs/research/m6-api.md, section
//! 6, OB1).

mod support;

use fitment_server::Server;
use support::Dirs;

fn open(dirs: &Dirs) -> rusqlite::Connection {
    rusqlite::Connection::open(dirs.state.join("server.db")).unwrap()
}

/// OB1, section 6.1: the first start makes the state directory: the
/// marker (last), `server.db` with its own `application_id` and schema 1,
/// WAL, STRICT tables, the object store's directories, and the `system`
/// user, which has no password and can hold no token (M6-56).
#[test]
fn the_first_start_makes_the_state() {
    let dirs = Dirs::new();
    drop(Server::build(dirs.config()).unwrap());
    assert_eq!(
        std::fs::read_to_string(dirs.state.join("FITMENT-SERVER-STATE")).unwrap(),
        "fitment.server 1\n"
    );
    assert!(dirs.state.join("objects/sha256").is_dir());
    assert!(dirs.state.join("tmp").is_dir());
    let db = open(&dirs);
    let id: i64 = db
        .query_row("PRAGMA application_id", [], |r| r.get(0))
        .unwrap();
    let version: i64 = db
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    let mode: String = db
        .query_row("PRAGMA journal_mode", [], |r| r.get(0))
        .unwrap();
    assert_eq!((id, version, mode.as_str()), (0x666D_7453, 1, "wal"));
    let tables: Vec<(String, String)> = db
        .prepare("SELECT name, sql FROM sqlite_schema WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let names: Vec<&str> = tables.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(
        names,
        [
            "assembly",
            "assembly_name",
            "assembly_parts",
            "job",
            "job_cites",
            "job_event",
            "job_input",
            "lease",
            "meta",
            "object",
            "socket_line",
            "socket_revision",
            "token",
            "upload",
            "user",
        ]
    );
    for (name, sql) in &tables {
        assert!(sql.trim_end().ends_with("STRICT"), "{name} is not STRICT");
    }
    let (kind, password, role): (String, Option<String>, String) = db
        .query_row("SELECT kind, password_phc, role FROM user", [], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .unwrap();
    assert_eq!(
        (kind.as_str(), password, role.as_str()),
        ("system", None, "admin")
    );
    let refused = db.execute(
        "INSERT INTO token (id, user, name, secret_sha256, hint, role, created_ms, expires_ms)
         VALUES ('t_x', 1, 'n', ?1, 'abcd', 'reader', 0, 1)",
        ["0".repeat(64)],
    );
    assert!(refused.is_err(), "the system user held a token");
}

/// OB1: a second start opens the same database: the `system` user is made
/// once, and nothing is recreated.
#[test]
fn a_restart_opens_the_same_state() {
    let dirs = Dirs::new();
    drop(Server::build(dirs.config()).unwrap());
    let first: String = open(&dirs)
        .query_row("SELECT id FROM user WHERE kind = 'system'", [], |r| {
            r.get(0)
        })
        .unwrap();
    drop(Server::build(dirs.config()).unwrap());
    let db = open(&dirs);
    let users: i64 = db
        .query_row("SELECT count(*) FROM user", [], |r| r.get(0))
        .unwrap();
    let again: String = db
        .query_row("SELECT id FROM user WHERE kind = 'system'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!((users, again), (1, first.clone()));
    assert!(first.starts_with("u_") && first.len() == 28, "{first}");
}

/// OB1: every connection runs with foreign keys on and `trusted_schema`
/// off; socket revisions are insert-only (OB5's trigger); a token never
/// moves to the system user and no user changes kind (M6-56).
#[test]
fn the_schema_guards_itself() {
    let dirs = Dirs::new();
    drop(Server::build(dirs.config()).unwrap());
    let db = open(&dirs);
    db.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
    let orphan = db.execute(
        "INSERT INTO upload (owner, sha256, created_ms, expires_ms) VALUES (99, ?1, 0, 1)",
        ["a".repeat(64)],
    );
    assert!(orphan.is_err(), "a foreign key was not checked");
    let bad_hash = db.execute(
        "INSERT INTO object (sha256, bytes, added_ms) VALUES ('NOT-HEX', 1, 0)",
        [],
    );
    assert!(bad_hash.is_err());
    let sql: String = db
        .query_row(
            "SELECT sql FROM sqlite_schema WHERE name = 'socket_revision_insert_only'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(sql.contains("insert-only"));
    // A token never moves to the system user, and no user changes kind.
    db.execute_batch(
        "INSERT INTO user (id, username_fold, username, role, created_ms, updated_ms)
         VALUES ('u_person', 'ann', 'ann', 'reader', 0, 0);",
    )
    .unwrap();
    db.execute(
        "INSERT INTO token (id, user, name, secret_sha256, hint, role, created_ms, expires_ms)
         VALUES ('t_x', 2, 'n', ?1, 'abcd', 'reader', 0, 1)",
        ["0".repeat(64)],
    )
    .unwrap();
    assert!(
        db.execute("UPDATE token SET user = 1 WHERE id = 't_x'", [])
            .is_err()
    );
    assert!(
        db.execute("UPDATE user SET kind = 'system' WHERE id = 'u_person'", [])
            .is_err()
    );
    assert!(
        db.execute("UPDATE user SET kind = 'person' WHERE kind = 'system'", [])
            .is_err()
    );
}

/// Review S6: a marker without its `server.db`, or with an empty one, is
/// refused (exit 2), at start and by `check-config`, as a catalog without
/// its database is not a catalog.
#[test]
fn a_marker_without_its_database_is_refused() {
    let dirs = Dirs::new();
    drop(Server::build(dirs.config()).unwrap());
    for name in ["server.db", "server.db-wal", "server.db-shm"] {
        let _ = std::fs::remove_file(dirs.state.join(name));
    }
    let error = Server::build(dirs.config()).unwrap_err();
    assert_eq!(error.exit_code(), 2, "{error}");
    assert!(fitment_server::check(&dirs.config()).is_err());
    std::fs::write(dirs.state.join("server.db"), b"").unwrap();
    let error = Server::build(dirs.config()).unwrap_err();
    assert_eq!(error.exit_code(), 2, "{error}");
    assert!(
        !error.to_string().contains(&dirs.root.display().to_string()),
        "{error}"
    );
}

/// Review S13: on Unix the state directory and its subdirectories are the
/// owner's alone (0700), and so are `server.db` and its WAL files (0600).
#[cfg(unix)]
#[test]
fn the_state_is_the_owners_alone() {
    use std::os::unix::fs::PermissionsExt as _;
    let dirs = Dirs::new();
    let server = Server::build(dirs.config()).unwrap();
    let mode =
        |path: &std::path::Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
    for dir in [
        &dirs.state,
        &dirs.state.join("objects"),
        &dirs.state.join("tmp"),
    ] {
        assert_eq!(mode(dir), 0o700, "{}", dir.display());
    }
    for name in ["server.db", "server.db-wal", "server.db-shm"] {
        let path = dirs.state.join(name);
        if path.exists() {
            assert_eq!(mode(&path), 0o600, "{name}");
        }
    }
    assert!(
        dirs.state.join("server.db-wal").exists(),
        "the WAL is open while the server is"
    );
    drop(server);
}
