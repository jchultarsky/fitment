//! The binary as a child process (docs/research/m6-api.md, section 9.4;
//! VT33): configuration, startup refusals, the announced address, the
//! logs, `check-config` and `healthcheck`.

mod support;

use std::collections::BTreeMap;
use std::path::Path;
use std::time::SystemTime;

use support::{Dirs, Process, TestServer, run, write_config};

/// M6-54, OP1: stdout carries one line, the bound address of `--listen
/// 127.0.0.1:0`; logs are JSON lines on stderr; a graceful stop exits 0.
#[test]
fn stdout_carries_the_address_alone() {
    let dirs = Dirs::new();
    let process = Process::start(&dirs, "", &[]).expect("starts");
    assert_ne!(process.addr.port(), 0);
    let (code, stdout, stderr) = process.stop();
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(stdout.lines().collect::<Vec<_>>().len(), 1, "{stdout}");
    assert!(stdout.starts_with("fitment-server: listening on 127.0.0.1:"));
    let lines: Vec<&str> = stderr.lines().collect();
    assert_ne!(lines.len(), 0);
    for line in &lines {
        let event: serde_json::Value =
            serde_json::from_str(line).unwrap_or_else(|_| panic!("not JSON: {line}"));
        assert!(
            event["level"].is_string() && event["fields"].is_object(),
            "{line}"
        );
    }
    assert_eq!(support::scan::logs(&stderr), Vec::<String>::new());
}

/// OP1: `log.format = "text"` writes plain lines; `--log-format` overrides
/// the file (OP2).
#[test]
fn logs_can_be_text() {
    let dirs = Dirs::new();
    let process = Process::start(
        &dirs,
        "[log]\nformat = \"json\"\n",
        &["--log-format", "text"],
    )
    .unwrap();
    let (code, _, stderr) = process.stop();
    assert_eq!(code, Some(0));
    assert!(
        stderr
            .lines()
            .all(|l| serde_json::from_str::<serde_json::Value>(l).is_err()),
        "{stderr}"
    );
    assert!(stderr.contains("listening"));
}

/// One refused start: its exit code, and nothing on stdout (never
/// listening).
fn refused(dirs: &Dirs, config: &str) -> (Option<i32>, String) {
    match Process::start(dirs, config, &[]) {
        Ok(process) => {
            let (_, stdout, _) = process.stop();
            panic!("it started: {stdout}");
        }
        Err(failed) => {
            assert!(failed.stdout.is_empty(), "{}", failed.stdout);
            assert_ne!(failed.stderr, "");
            (failed.code, failed.stderr)
        }
    }
}

/// VT33, SV5, SV6: each refused at startup with a message and its exit
/// code, before listening: a directory that is not a catalog (2), a
/// catalog of a newer schema (3), nested directories (2), an unreadable
/// configuration (2), an unknown key (2) or variable (2), a value out of
/// range (2).
#[test]
fn bad_starts_are_refused_before_listening() {
    let dirs = Dirs::bare();
    std::fs::create_dir_all(&dirs.catalog).unwrap();
    assert_eq!(refused(&dirs, "").0, Some(2));

    let dirs = Dirs::new();
    let db = rusqlite::Connection::open(dirs.catalog.join("catalog.db")).unwrap();
    db.pragma_update(None, "user_version", 99).unwrap();
    drop(db);
    let (code, message) = refused(&dirs, "");
    assert_eq!(code, Some(3), "{message}");

    let dirs = Dirs::new();
    let inside = dirs.catalog.join("state");
    let config = format!(
        "[paths]\ncatalog = {:?}\nstate = {:?}\n",
        dirs.catalog.display().to_string(),
        inside.display().to_string()
    );
    assert_eq!(refused(&dirs, &config).0, Some(2));
    let config = format!(
        "[paths]\ncatalog = {:?}\nstate = {:?}\n",
        dirs.catalog.display().to_string(),
        dirs.root.display().to_string()
    );
    assert_eq!(refused(&dirs, &config).0, Some(2));

    let dirs = Dirs::new();
    assert_eq!(refused(&dirs, "[limits]\nmax_upload_meb = 1\n").0, Some(2));
    assert_eq!(
        refused(&dirs, "[limits]\nmax_upload_mib = \"lots\"\n").0,
        Some(2)
    );
    assert_eq!(
        refused(&dirs, "[server]\nlisten = \"nowhere\"\n").0,
        Some(2)
    );
    assert_eq!(
        refused(&dirs, "[fit]\nverifier = \"occt-helper\"\n").0,
        Some(2)
    );
    assert_eq!(refused(&dirs, "not toml [").0, Some(2));

    let (code, _, stderr) = run(
        &["serve", "--config", "/nonexistent/fitment-server.toml"],
        &[],
    );
    assert_eq!(code, Some(2), "{stderr}");
    let file = write_config(&dirs, "");
    let file = file.to_str().unwrap();
    let (code, _, _) = run(
        &["serve", "--config", file],
        &[("FITMENT_SERVER__LIMITS__NOPE", "1")],
    );
    assert_eq!(code, Some(2));
}

/// VT33, OB1: a `server.db` of a newer schema (3) or of another
/// application (2), and a state directory that holds other files (2),
/// are refused.
#[test]
fn a_foreign_or_newer_state_is_refused() {
    let dirs = Dirs::new();
    std::fs::create_dir_all(&dirs.state).unwrap();
    let db = rusqlite::Connection::open(dirs.state.join("server.db")).unwrap();
    db.pragma_update(None, "application_id", 0x666D_7453)
        .unwrap();
    db.pragma_update(None, "user_version", 2).unwrap();
    db.execute_batch("CREATE TABLE later (x)").unwrap();
    drop(db);
    assert_eq!(refused(&dirs, "").0, Some(3));

    let dirs = Dirs::new();
    std::fs::create_dir_all(&dirs.state).unwrap();
    let db = rusqlite::Connection::open(dirs.state.join("server.db")).unwrap();
    db.execute_batch("CREATE TABLE theirs (x)").unwrap();
    drop(db);
    assert_eq!(refused(&dirs, "").0, Some(2));

    let dirs = Dirs::new();
    std::fs::create_dir_all(&dirs.state).unwrap();
    std::fs::write(dirs.state.join("notes.txt"), "mine").unwrap();
    assert_eq!(refused(&dirs, "").0, Some(2));
}

/// VT33: a port in use is refused with exit 1 (I/O), before stdout's line.
#[test]
fn a_port_in_use_is_refused() {
    let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let dirs = Dirs::new();
    let config = format!("[server]\nlisten = \"{}\"\n", taken.local_addr().unwrap());
    assert_eq!(refused(&dirs, &config).0, Some(1));
}

/// SD3: the address is announced last, once `server.db`, the marker and
/// the lease exist and the catalog was opened; a start the catalog
/// refuses announces nothing and leaves the lease free.
#[test]
fn the_address_is_announced_last() {
    let dirs = Dirs::new();
    let process = Process::start(&dirs, "", &[]).unwrap();
    assert!(support::is_state(&dirs.state));
    let db = rusqlite::Connection::open(dirs.state.join("server.db")).unwrap();
    let holders: i64 = db
        .query_row("SELECT count(*) FROM lease", [], |r| r.get(0))
        .unwrap();
    assert_eq!(holders, 1);
    let (code, _, _) = process.stop();
    assert_eq!(code, Some(0));
    let holders: i64 = db
        .query_row("SELECT count(*) FROM lease", [], |r| r.get(0))
        .unwrap();
    assert_eq!(holders, 0, "a graceful stop releases the lease");

    std::fs::remove_file(dirs.catalog.join("FITMENT-CATALOG")).unwrap();
    assert_eq!(refused(&dirs, "").0, Some(2));
    let holders: i64 = db
        .query_row("SELECT count(*) FROM lease", [], |r| r.get(0))
        .unwrap();
    assert_eq!(holders, 0, "a refused start leaves the lease free");
}

/// OP2: flags, then `FITMENT_SERVER__<SECTION>__<KEY>`, then the file
/// (`--config` or `FITMENT_SERVER_CONFIG`), then the defaults; another
/// `FITMENT_SERVER_*` variable is ignored with a warning.
#[test]
fn configuration_sources_take_their_order() {
    let dirs = Dirs::new();
    let file = write_config(&dirs, "[limits]\nmax_upload_mib = 32\nmax_json_kib = 512\n");
    let file = file.to_str().unwrap();
    let (code, stdout, stderr) = run(
        &["check-config", "--listen", "127.0.0.1:9999"],
        &[
            ("FITMENT_SERVER_CONFIG", file),
            ("FITMENT_SERVER__LIMITS__MAX_UPLOAD_MIB", "48"),
            ("FITMENT_SERVER__SERVER__LISTEN", "127.0.0.1:7777"),
            // Kubernetes' service links for a service named fitment-server:
            // not keys, ignored with a warning.
            ("FITMENT_SERVER_PORT", "tcp://10.0.0.9:8080"),
            ("FITMENT_SERVER_LIMITS_MAX_UPLOAD_MIB", "1"),
        ],
    );
    assert_eq!(code, Some(0), "{stderr}");
    assert!(stderr.contains("ignored FITMENT_SERVER_PORT"), "{stderr}");
    assert!(
        stderr.contains("ignored FITMENT_SERVER_LIMITS_MAX_UPLOAD_MIB"),
        "{stderr}"
    );
    let effective: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(effective["limits"]["max_upload_mib"], 48);
    assert_eq!(effective["limits"]["max_json_kib"], 512);
    assert_eq!(effective["limits"]["page_max"], 1000);
    assert_eq!(effective["server"]["listen"], "127.0.0.1:9999");
}

/// OP3: `paths.catalog` and `paths.state` are required; every limit is a
/// key with section 4.14's default.
#[test]
fn the_paths_are_required_and_limits_are_keys() {
    let dirs = Dirs::new();
    std::fs::write(dirs.root.join("empty.toml"), "").unwrap();
    let empty = dirs.root.join("empty.toml");
    let (code, _, stderr) = run(&["check-config", "--config", empty.to_str().unwrap()], &[]);
    assert_eq!(code, Some(2));
    assert!(stderr.contains("paths.catalog"), "{stderr}");
    let file = write_config(&dirs, "");
    let (code, stdout, _) = run(&["check-config", "--config", file.to_str().unwrap()], &[]);
    assert_eq!(code, Some(0));
    let effective: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    for (section, key, value) in [
        ("limits", "max_upload_mib", 128),
        ("limits", "max_socket_mib", 16),
        ("limits", "max_json_kib", 1024),
        ("limits", "max_candidates", 10_000),
        ("limits", "max_catalog_candidates", 100_000),
        ("jobs", "match_threads", 4),
        ("jobs", "per_owner_queued", 16),
        ("catalog", "read_busy_ms", 5_000),
        ("auth", "max_live_tokens", 20),
        ("server", "max_connections", 1024),
    ] {
        assert_eq!(effective[section][key], value, "{section}.{key}");
    }
    assert_eq!(effective["fit"]["verifier"], "none");
    assert_eq!(effective["log"]["format"], "json");
}

/// Every file under `dir`, with its size and modification time.
fn listing(dir: &Path) -> BTreeMap<String, (u64, SystemTime)> {
    let mut out = BTreeMap::new();
    let mut stack = vec![dir.to_owned()];
    while let Some(at) = stack.pop() {
        for entry in std::fs::read_dir(&at).unwrap() {
            let path = entry.unwrap().path();
            let meta = std::fs::symlink_metadata(&path).unwrap();
            if meta.is_dir() {
                stack.push(path.clone());
            }
            out.insert(
                path.display().to_string(),
                (meta.len(), meta.modified().unwrap()),
            );
        }
    }
    out
}

/// OP4, review R2: `check-config` loads and checks the configuration,
/// checks the catalog and reads `server.db`'s header as bytes, and writes
/// nothing: on a fresh configuration it makes no state directory, and on
/// a state and a catalog a server has used, every file's listing, size and
/// time are unchanged (no `-wal` or `-shm`, no migration). It prints the
/// effective configuration and exits 0, or 2 for what it refuses.
#[test]
fn check_config_checks_and_writes_nothing() {
    let dirs = Dirs::new();
    let file = write_config(&dirs, "");
    let (code, stdout, _) = run(&["check-config", "--config", file.to_str().unwrap()], &[]);
    assert_eq!(code, Some(0));
    assert!(stdout.contains("\"limits\""));
    assert!(
        !dirs.state.exists(),
        "check-config made the state directory"
    );

    let process = Process::start(&dirs, "", &[]).unwrap();
    assert_eq!(process.stop().0, Some(0));
    let (state, catalog) = (listing(&dirs.state), listing(&dirs.catalog));
    std::thread::sleep(std::time::Duration::from_millis(50));
    let (code, _, stderr) = run(&["check-config", "--config", file.to_str().unwrap()], &[]);
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(
        listing(&dirs.state),
        state,
        "check-config changed the state directory"
    );
    assert_eq!(
        listing(&dirs.catalog),
        catalog,
        "check-config changed the catalog"
    );

    std::fs::remove_file(dirs.catalog.join("FITMENT-CATALOG")).unwrap();
    let (code, _, stderr) = run(&["check-config", "--config", file.to_str().unwrap()], &[]);
    assert_eq!(code, Some(2), "{stderr}");
}

/// SV6, review S3: a `..` in either directory is refused (exit 2): through
/// a directory that does not exist yet it would defeat the nesting check
/// one way (a state inside the catalog) and refuse a sound layout the
/// other.
#[test]
fn a_parent_component_in_a_path_is_refused() {
    let dirs = Dirs::new();
    let cases = [
        (
            dirs.catalog.clone(),
            dirs.catalog.join("missing").join("..").join("state"),
        ),
        (
            dirs.root.join("missing").join("..").join("catalog"),
            dirs.state.clone(),
        ),
    ];
    for (catalog, state) in cases {
        let config = format!(
            "[paths]\ncatalog = {:?}\nstate = {:?}\n",
            catalog.display().to_string(),
            state.display().to_string()
        );
        let (code, message) = refused(&dirs, &config);
        assert_eq!(code, Some(2), "{message}");
        assert!(message.contains("`..`"), "{message}");
    }
}

/// OP5: `fitment-server healthcheck` asks the configured address's
/// `/v1/health` with a plain socket: 0 while the server answers, 1 when
/// nothing does.
#[test]
fn healthcheck_asks_the_server() {
    let dirs = Dirs::new();
    let process = Process::start(&dirs, "", &[]).unwrap();
    let file = write_config(&dirs, "");
    let file = file.to_str().unwrap();
    let addr = process.addr.to_string();
    let (code, _, stderr) = run(&["healthcheck", "--config", file, "--listen", &addr], &[]);
    assert_eq!(code, Some(0), "{stderr}");
    let (code, _, _) = process.stop();
    assert_eq!(code, Some(0));
    let (code, _, _) = run(&["healthcheck", "--config", file, "--listen", &addr], &[]);
    assert_eq!(code, Some(1));
}

/// SV5, OP8: `--version` prints the workspace's version, the CLI's.
#[test]
fn the_version_is_the_workspaces() {
    let (code, stdout, _) = run(&["--version"], &[]);
    assert_eq!(code, Some(0));
    assert_eq!(
        stdout.trim(),
        format!("fitment-server {}", env!("CARGO_PKG_VERSION"))
    );
}

/// The process answers over HTTP like the library (section 9.4's process
/// layer): health, info and an error, each scanned (XH13).
#[tokio::test(flavor = "multi_thread")]
async fn the_process_serves_the_routes() {
    let server = TestServer::process(Dirs::new(), "");
    assert_eq!(server.get("/v1/health").await.status, 204);
    assert_eq!(server.get("/v1/ready").await.status, 204);
    assert_eq!(
        server.get("/v1/info").await.json()["format"],
        "fitment.info"
    );
    assert_eq!(server.get("/v1/nope").await.code(), "not_found");
    let child = server.child.as_ref().unwrap();
    assert!(Path::new(&server.dirs.state).join("server.db").is_file());
    assert_ne!(child.addr.port(), 0);
}
