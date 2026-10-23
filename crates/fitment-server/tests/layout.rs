//! The crate and its dependencies (docs/research/m6-api.md, section 4.13,
//! SV1 to SV9), the image (OP8) and the seams (OP11): read from the
//! sources, the manifests and `cargo tree`, so a change that breaks one
//! fails here and not only in CI's scripts (VT35).
#![allow(clippy::disallowed_methods)] // reads files and runs cargo

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

fn crate_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_owned()
}

fn root() -> PathBuf {
    crate_dir().join("../..")
}

/// Every `.rs` file under `src`, with its path relative to it.
fn sources() -> Vec<(String, String)> {
    fn walk(dir: &Path, base: &Path, out: &mut Vec<(String, String)>) {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                walk(&path, base, out);
            } else if path.extension().is_some_and(|e| e == "rs" || e == "sql") {
                let name = path
                    .strip_prefix(base)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push((name, std::fs::read_to_string(&path).unwrap()));
            }
        }
    }
    let mut out = Vec::new();
    let src = crate_dir().join("src");
    walk(&src, &src, &mut out);
    out
}

/// The files of `src` whose code (not comments) holds `needle`.
fn naming(needle: &str) -> BTreeSet<String> {
    sources()
        .into_iter()
        .filter(|(_, text)| {
            text.lines()
                .map(str::trim_start)
                .filter(|l| !l.starts_with("//"))
                .any(|l| l.contains(needle))
        })
        .map(|(name, _)| name)
        .collect()
}

fn set(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|n| (*n).to_owned()).collect()
}

/// `cargo tree` of the workspace's lock, offline.
fn tree(args: &[&str]) -> String {
    let output = Command::new(env!("CARGO"))
        .current_dir(root())
        .args(["tree", "--locked", "--prefix", "none"])
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn crate_names(tree: &str) -> BTreeSet<String> {
    tree.lines()
        .filter_map(|l| l.split_whitespace().next())
        .map(str::to_owned)
        .collect()
}

/// SV1, VT35: no async runtime, executor or HTTP stack enters the graph of
/// `fitment` or `fitment-catalog`, whatever their features.
#[test]
fn the_cli_and_the_catalog_have_no_async_crate() {
    for package in ["fitment", "fitment-catalog", "fitment-core"] {
        let names = crate_names(&tree(&["-p", package, "-e", "normal", "--all-features"]));
        for banned in [
            "tokio",
            "mio",
            "hyper",
            "axum",
            "tower",
            "async-std",
            "smol",
            "futures-executor",
        ] {
            assert!(!names.contains(banned), "{package}'s graph holds {banned}");
        }
    }
    let server = crate_names(&tree(&["-p", "fitment-server", "-e", "normal"]));
    assert!(
        server.contains("tokio"),
        "the control: the server's graph holds tokio"
    );
}

/// SV9, VT35: the server's default runtime graph holds no schemars, no
/// `h2`, no TLS stack and no reqwest, and `serde_json` with exactly
/// `default`, `std` and `float_roundtrip`.
#[test]
fn the_servers_runtime_graph_is_sv9s() {
    let names = crate_names(&tree(&["-p", "fitment-server", "-e", "normal"]));
    for banned in [
        "schemars",
        "h2",
        "rustls",
        "ring",
        "aws-lc-rs",
        "aws-lc-sys",
        "native-tls",
        "openssl",
        "reqwest",
    ] {
        assert!(
            !names.contains(banned),
            "the server's runtime graph holds {banned}"
        );
    }
    let features = tree(&["-p", "fitment-server", "-e", "normal,features"]);
    let serde_json: BTreeSet<&str> = features
        .lines()
        .filter_map(|l| l.strip_prefix("serde_json feature \""))
        .filter_map(|l| l.split('"').next())
        .collect();
    assert_eq!(
        serde_json,
        ["default", "float_roundtrip", "std"]
            .into_iter()
            .collect::<BTreeSet<_>>()
    );
    let with_schema = crate_names(&tree(&[
        "-p",
        "fitment-server",
        "-e",
        "normal",
        "--features",
        "schema",
    ]));
    assert!(
        with_schema.contains("schemars"),
        "the control: the schema feature brings schemars"
    );
}

/// SV2: only `main.rs` reads the process's arguments, installs the log
/// subscriber or exits; `lib.rs` has `Server::build`, `router` and
/// `serve`.
#[test]
fn only_main_reads_arguments_and_exits() {
    assert_eq!(naming("Cli::parse"), set(&["main.rs"]));
    assert_eq!(naming("ExitCode"), set(&["main.rs"]));
    assert_eq!(naming("tracing_subscriber"), set(&["main.rs"]));
    assert!(naming("std::env::args").is_empty());
    assert!(naming("process::exit").is_empty());
    let lib = std::fs::read_to_string(crate_dir().join("src/lib.rs")).unwrap();
    for item in ["pub fn build(", "pub fn router(", "pub async fn serve("] {
        assert!(lib.contains(item), "lib.rs lacks {item}");
    }
}

/// SV4: `catalog.rs` is the only module naming `fitment_catalog`, read as
/// the module with its submodules under `catalog/` (as SV6 reads
/// `state::db`; PR 7's As-built note), and `matching.rs` the only one
/// naming a `Matcher` or a `MatchDocument`.
#[test]
fn only_catalog_rs_names_the_catalog_crate() {
    // As tools/check-core.sh's `\bfitment_catalog\b`: a metric named
    // `fitment_catalog_generation` names no crate.
    let crate_named: BTreeSet<String> = sources()
        .into_iter()
        .filter(|(_, text)| {
            text.lines()
                .map(str::trim_start)
                .filter(|l| !l.starts_with("//"))
                .any(|l| {
                    l.match_indices("fitment_catalog").any(|(at, n)| {
                        !l[at + n.len()..]
                            .chars()
                            .next()
                            .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
                    })
                })
        })
        .map(|(name, _)| name)
        .collect();
    assert!(crate_named.contains("catalog.rs"), "{crate_named:?}");
    assert!(
        crate_named
            .iter()
            .all(|n| n == "catalog.rs" || n.starts_with("catalog/")),
        "{crate_named:?}"
    );
    for name in naming("Matcher").into_iter().chain(naming("MatchDocument")) {
        assert!(
            name == "matching.rs" || name == "openapi.rs",
            "{name} names the matcher"
        );
    }
}

/// SV6: `server.db` is opened by the module `state::db` alone (`state/db.rs`
/// and its submodules under `state/db/`), the only one naming rusqlite; no
/// SQL of the server names a table of the catalog; `deny.toml` lets
/// rusqlite in through `fitment-catalog` and `fitment-server` only.
#[test]
fn the_two_databases_stay_apart() {
    let db = |name: &str| name == "state/db.rs" || name.starts_with("state/db/");
    let named = naming("rusqlite");
    assert!(named.contains("state/db.rs"), "{named:?}");
    assert!(named.iter().all(|n| db(n)), "{named:?}");
    let catalog_only = [
        "blob",
        "clock",
        "entry",
        "features",
        "name",
        "op",
        "posting",
        "properties",
        "revision",
        "tokens",
    ];
    for (file, text) in sources() {
        if !(db(&file) || file == "state/schema.sql") {
            continue;
        }
        let words: Vec<&str> = text
            .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .filter(|w| !w.is_empty())
            .collect();
        for pair in words.windows(2) {
            let keyword = pair[0].to_ascii_uppercase();
            if ["FROM", "JOIN", "INTO", "UPDATE", "TABLE", "REFERENCES"].contains(&keyword.as_str())
            {
                assert!(
                    !catalog_only.contains(&pair[1]),
                    "{file}: {} {}",
                    pair[0],
                    pair[1]
                );
            }
        }
    }
    let deny = std::fs::read_to_string(root().join("deny.toml")).unwrap();
    assert!(
        deny.contains(r#"{ crate = "rusqlite", wrappers = ["fitment-catalog", "fitment-server"]"#)
    );
}

/// SV7: clippy's bans for the server (the environment outside
/// `config.rs`, `process::exit`, `thread::spawn`, `block_in_place`) are in
/// its `clippy.toml` and denied in `lib.rs`; the one escape is
/// `config.rs`'s; `serde_json::Value`, `from_str` and `to_value` in
/// `matching.rs` and `objects.rs` are banned by `tools/check-core.sh`.
#[test]
fn the_bans_are_in_clippy_toml() {
    let clippy = std::fs::read_to_string(crate_dir().join("clippy.toml")).unwrap();
    for path in [
        "std::env::var",
        "std::env::var_os",
        "std::process::exit",
        "std::thread::spawn",
        "tokio::task::block_in_place",
    ] {
        assert!(
            clippy.contains(&format!("path = \"{path}\"")),
            "clippy.toml lacks {path}"
        );
    }
    let lib = std::fs::read_to_string(crate_dir().join("src/lib.rs")).unwrap();
    assert!(lib.contains("#![deny(clippy::disallowed_methods, clippy::disallowed_types)]"));
    assert_eq!(
        naming("allow(clippy::disallowed_methods)"),
        set(&["config.rs"])
    );
    let check = std::fs::read_to_string(root().join("tools/check-core.sh")).unwrap();
    assert!(check.contains("serde_json::(Value|from_str|to_value)"));
    for (file, text) in sources() {
        if file == "matching.rs" || file == "state/objects.rs" {
            for banned in [
                "serde_json::Value",
                "serde_json::from_str",
                "serde_json::to_value",
            ] {
                assert!(!text.contains(banned), "{file} names {banned}");
            }
        }
    }
}

/// SV8, OP9: the fit verifier is held as `Arc<dyn FitVerifier + Send +
/// Sync>`, built from `fit.verifier = "none"` as `NoFitVerifier`, and
/// moves into a thread; any other value is refused at start.
#[test]
fn the_fit_verifier_is_held_send_and_sync() {
    let fit = fitment_server::Config::default().fit;
    let verifier: fitment_server::matching::Verifier = fitment_server::matching::verifier(&fit);
    let kernel = std::thread::spawn(move || fitment_server::matching::kernel(&verifier))
        .join()
        .unwrap();
    assert_eq!(kernel, None);
    let mut config = fitment_server::Config::default();
    config.paths.catalog = "/c".into();
    config.paths.state = "/s".into();
    assert!(config.validate().is_ok());
    config.fit.verifier = "occt-helper".into();
    assert!(config.validate().is_err());
}

/// SV5: the server takes the workspace's version and depends on the
/// workspace's `fitment-core` and `fitment-catalog`, which are `=` that
/// version.
#[test]
fn versions_are_in_lockstep() {
    let manifest = std::fs::read_to_string(crate_dir().join("Cargo.toml")).unwrap();
    assert!(manifest.contains("version.workspace = true"));
    assert!(manifest.contains("fitment-core = { workspace = true }"));
    assert!(manifest.contains("fitment-catalog = { workspace = true }"));
    let workspace = std::fs::read_to_string(root().join("Cargo.toml")).unwrap();
    let version = env!("CARGO_PKG_VERSION");
    for name in ["fitment-core", "fitment-catalog"] {
        let line = workspace
            .lines()
            .find(|l| l.starts_with(&format!("{name} =")))
            .unwrap();
        assert!(
            line.contains(&format!("version = \"={version}\"")),
            "{line}"
        );
    }
}

/// OP8, OP11, M6-35: the image is built from `deploy/Containerfile` as OP8
/// says: pinned bases, the release build of both binaries with `--locked`
/// and no feature (so never `test-support`), a licence list, user 65532,
/// port 8080, both volumes, the health check and `serve`; the crate is
/// never published.
#[test]
fn the_image_is_built_as_op8_says() {
    let file = std::fs::read_to_string(root().join("deploy/Containerfile")).unwrap();
    let lines: Vec<&str> = file
        .lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .collect();
    let text = lines.join("\n");
    assert!(text.contains("FROM rust:1.87-bookworm@sha256:"));
    assert!(text.contains("FROM gcr.io/distroless/cc-debian12:nonroot@sha256:"));
    assert!(text.contains("cargo build --release --locked -p fitment-server -p fitment"));
    assert!(
        !text.contains("--features") && !text.contains("--all-features"),
        "a feature in the image"
    );
    for line in [
        "USER 65532",
        "EXPOSE 8080",
        r#"VOLUME ["/var/lib/fitment/catalog", "/var/lib/fitment/state"]"#,
        r#"CMD ["fitment-server", "healthcheck"]"#,
        r#"ENTRYPOINT ["fitment-server", "serve"]"#,
        "THIRD-PARTY.txt",
        "/usr/local/bin/fitment-server",
        "/usr/local/bin/fitment\n",
    ] {
        assert!(
            text.contains(line) || file.contains(line),
            "the Containerfile lacks {line}"
        );
    }
    let config = std::fs::read_to_string(root().join("deploy/fitment-server.toml")).unwrap();
    assert!(config.contains("listen = \"0.0.0.0:8080\""));
    let manifest = std::fs::read_to_string(crate_dir().join("Cargo.toml")).unwrap();
    assert!(manifest.lines().any(|l| l == "publish = false"));
    assert!(manifest.contains("[package.metadata.dist]\ndist = false"));
}

/// A file's words, comment markers and line breaks aside, in lower case,
/// so a sentence wrapped anywhere still reads as one.
fn prose(text: &str) -> String {
    text.lines()
        .map(|l| l.trim().trim_start_matches('#').trim())
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// OP10, CC5: one server per catalog, with both volumes, the catalog's
/// and the state's, on a local disk, never a network file system or a
/// sync folder, as the README, the Containerfile and the image's
/// configuration say, each naming both; the CLI on the same catalog is
/// `concurrency.rs::a_cli_ingest_beside_the_server_is_consistent`.
#[test]
fn one_replica_on_local_disks_as_op10_says() {
    for (file, both) in [
        ("README.md", "both directories on a local disk"),
        ("deploy/Containerfile", "both volumes on a local disk"),
        ("deploy/fitment-server.toml", "both on a local disk"),
    ] {
        let text = prose(&std::fs::read_to_string(root().join(file)).unwrap());
        for words in [
            "one server per catalog",
            both,
            "never a network file system or a sync folder",
        ] {
            assert!(text.contains(words), "{file} does not say {words:?}");
        }
    }
    // The configuration's note stands over the two paths it covers.
    let config = std::fs::read_to_string(root().join("deploy/fitment-server.toml")).unwrap();
    let note = config.find("Both on a local disk").unwrap();
    let paths = &config[note..];
    assert!(paths.contains("\ncatalog = ") && paths.contains("\nstate = "));
    // The README names what the two directories are.
    let readme = prose(&std::fs::read_to_string(root().join("README.md")).unwrap());
    assert!(readme.contains("serve --catalog dir --state dir"));
}

/// OP11: the seams exist under `test-support` only, which no default or
/// release build turns on: the hooks module, fixed ids and time, and
/// shutdown on stdin's end of file are each behind the feature.
#[test]
fn the_seams_are_test_support_only() {
    let manifest = std::fs::read_to_string(crate_dir().join("Cargo.toml")).unwrap();
    assert!(!manifest.contains("default ="), "a default feature");
    let lib = std::fs::read_to_string(crate_dir().join("src/lib.rs")).unwrap();
    assert!(lib.contains("#[cfg(feature = \"test-support\")]\npub mod hooks;"));
    // VT26's harness (PR 8) is a seam too: never in a release build.
    assert!(lib.contains("#[cfg(feature = \"test-support\")]\npub mod fuzzing;"));
    let ids = std::fs::read_to_string(crate_dir().join("src/ids.rs")).unwrap();
    for item in ["pub fn fix_ids", "pub fn fix_time"] {
        let at = ids.find(item).unwrap();
        assert!(
            ids[..at]
                .trim_end()
                .ends_with("#[cfg(feature = \"test-support\")]"),
            "{item}"
        );
    }
    let main = std::fs::read_to_string(crate_dir().join("src/main.rs")).unwrap();
    assert!(main.contains("#[cfg(feature = \"test-support\")]\n    let eof"));
    let release = tree(&[
        "-p",
        "fitment-server",
        "-e",
        "normal,features",
        "-i",
        "fitment-server",
    ]);
    assert!(
        !release.contains("fitment-server feature \"test-support\"\n"),
        "{release}"
    );
}
