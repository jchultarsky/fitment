//! VT7's `features`, `parts` and `socket` rows (docs/research/m6-api.md,
//! DC1, DC3, JB4, XH4, XH7): each job's document is the CLI's stdout for
//! the same bytes, byte for byte, and its state and exit code are the
//! CLI's exit code mapped as JB3 says; a refused input's document too; a
//! selection error, the CLI's exit 2, is the job `refused`, exit 2, with
//! no document. On the committed unit parts and assemblies, every OS,
//! debug. The CLI is the built `fitment` (`support::cli`).
//!
//! The catalog's documents (CA1, CU1, CU2): the library's, which the API
//! serves (PR 7b), are the CLI's `--format json` byte for byte, for an
//! ingest from bytes and for each change.

mod support;

use std::path::Path;

use fitment_catalog::document::{CatalogDocument, GcDocument, VerifyDocument};
use fitment_catalog::{Catalog, Input, NoObserver, Options, Reference, Scope};
use serde_json::{Value, json};
use support::{Dirs, TestServer, cli, waiting};

/// The job's state for the CLI's exit `code` (JB3, XH7).
fn state_of(code: i32) -> &'static str {
    match code {
        0 | 4 | 5 => "succeeded",
        2 | 3 => "refused",
        _ => "failed",
    }
}

/// A directory holding `bytes` under `name`, for the CLI.
fn file_in(dirs: &Dirs, name: &str, bytes: &[u8]) -> std::path::PathBuf {
    let dir = dirs.root.join("cli");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(name), bytes).unwrap();
    dir
}

/// Compares a finished job and its document with the CLI's run, and its
/// notes with the CLI's stderr (JB10) unless the CLI listed a selection
/// error's candidates there, which the job's error details hold.
async fn same(
    server: &TestServer,
    what: &str,
    job: &Value,
    cli: (Option<i32>, Vec<u8>, String),
    failures: &mut Vec<String>,
) {
    let (code, stdout, stderr) = cli;
    let code = code.unwrap_or(-1);
    let names: Vec<&str> = what.split_whitespace().take(1).collect();
    let notes = !stderr.is_empty() || job["notes"].as_array().is_some_and(Vec::is_empty);
    if code != 2 && notes && cli_notes(&stderr, &names) != api_notes(job) {
        failures.push(format!(
            "{what}: the notes {:?}, the CLI's stderr {:?}",
            api_notes(job),
            cli_notes(&stderr, &names)
        ));
    }
    if job["state"] != state_of(code) || job["exit_code"] != json!(code) {
        failures.push(format!(
            "{what}: the job {} exit {}, the CLI exit {code} ({stderr})",
            job["state"], job["exit_code"]
        ));
        return;
    }
    if stdout.is_empty() {
        if !job["document"].is_null() {
            failures.push(format!("{what}: a document where the CLI printed none"));
        }
        return;
    }
    let reply = server.document(job).await;
    assert_eq!(reply.status, 200, "{what}");
    if let Some(diff) = fitment_corpus::parity::bytes(what, &stdout, &reply.body) {
        failures.push(diff);
    }
    if reply.header("fitment-exit-code") != Some(code.to_string().as_str()) {
        failures.push(format!(
            "{what}: Fitment-Exit-Code {:?}",
            reply.header("fitment-exit-code")
        ));
    }
}

/// DC1, DC3, JB4: `fitment features FILE` and the job `features` on every
/// committed unit part, and on bytes that are no STEP file (exit 3,
/// `refused`, the refused document).
#[tokio::test]
async fn features_are_the_clis() {
    let server = TestServer::fresh();
    let data = Path::new(env!("CARGO_MANIFEST_DIR")).join("../fitment-core/tests/data");
    let mut files: Vec<(String, Vec<u8>)> = std::fs::read_dir(&data)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "stp"))
        .map(|p| {
            (
                p.file_name().unwrap().to_string_lossy().into_owned(),
                std::fs::read(&p).unwrap(),
            )
        })
        .collect();
    files.sort();
    files.push(("not_step.stp".into(), b"this is no STEP file\n".to_vec()));
    assert!(files.len() > 20);
    let mut failures = Vec::new();
    for (name, bytes) in &files {
        let sha = server.upload(bytes).await;
        let body = json!({ "upload": format!("sha256:{sha}") });
        let job = server.send(waiting("/v1/features", &body)).await.json();
        let dir = file_in(&server.dirs, name, bytes);
        same(
            &server,
            name,
            &job,
            cli(&dir, &["features", name]),
            &mut failures,
        )
        .await;
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// DC1, DC3, JB4, RA3: `fitment parts FILE --format json` and the job
/// `assembly_read` on every committed unit assembly and on bytes that are
/// none: the job's document and `…/parts` are the CLI's bytes.
#[tokio::test]
async fn parts_are_the_clis() {
    let server = TestServer::fresh();
    let mut failures = Vec::new();
    let mut inputs: Vec<(String, Vec<u8>)> = [
        "s01_bolted.stp",
        "s02_cbore.stp",
        "s03_dowels.stp",
        "s08x_nested.stp",
    ]
    .iter()
    .map(|n| ((*n).to_owned(), support::assembly(n)))
    .collect();
    inputs.push((
        "not_step.stp".into(),
        b"this is no STEP file either\n".to_vec(),
    ));
    for (name, bytes) in &inputs {
        let job = server.register(bytes, name).await;
        let dir = file_in(&server.dirs, name, bytes);
        let run = cli(&dir, &["parts", name, "--format", "json"]);
        let stdout = run.1.clone();
        same(&server, name, &job, run, &mut failures).await;
        let sha = fitment_core::sha256_hex(bytes);
        let parts = server
            .get(&format!("/v1/assemblies/sha256:{sha}/parts"))
            .await;
        assert_eq!(parts.status, 200, "{name}");
        if parts.body != stdout {
            failures.push(format!("{name}: …/parts is not the CLI's bytes"));
        }
        assert_eq!(
            parts.header("cache-control"),
            Some("no-cache"),
            "{name}: VN5"
        );
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// DC1, DC3, JB4, RA4: `fitment socket FILE --part P --format json` and
/// the job `socket_extract` for every target the CLI's socket goldens
/// name, a refused assembly's (exit 3, the refused document) and a
/// selection error (exit 2, `refused`, `part_not_found`, no document).
#[tokio::test]
async fn sockets_are_the_clis() {
    let server = TestServer::fresh();
    let golden = Path::new(env!("CARGO_MANIFEST_DIR")).join("../fitment/tests/golden/socket");
    let mut targets: Vec<(String, String)> = std::fs::read_dir(&golden)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter_map(|n| {
            let stem = n.strip_suffix(".json")?;
            let (case, part) = stem.split_once("--")?;
            Some((format!("{case}.stp"), part.to_owned()))
        })
        .collect();
    targets.sort();
    assert!(targets.len() >= 10);
    targets.push(("s01_bolted.stp".into(), "NOPE".into()));
    targets.push(("not_step.stp".into(), "BRACKET".into()));
    let mut failures = Vec::new();
    for (name, part) in &targets {
        let bytes = if name == "not_step.stp" {
            b"this is no STEP file at all\n".to_vec()
        } else {
            support::assembly(name)
        };
        let registered = server.register(&bytes, name).await;
        // A second registration of current bytes is the assembly, no job.
        assert!(
            registered["format"] == "fitment.assembly"
                || registered["state"] == "succeeded"
                || registered["state"] == "refused",
            "{registered}"
        );
        let sha = fitment_core::sha256_hex(&bytes);
        let job = server.extract(&sha, part).await;
        let dir = file_in(&server.dirs, name, &bytes);
        let what = format!("{name} --part {part}");
        same(
            &server,
            &what,
            &job,
            cli(&dir, &["socket", name, "--part", part, "--format", "json"]),
            &mut failures,
        )
        .await;
        if part == "NOPE" {
            assert_eq!(job["error"]["code"], "part_not_found", "{job}");
            assert!(job["document"].is_null());
            assert!(job["outcome"].is_null(), "XH12: no verdict");
        }
        if job["state"] == "succeeded" {
            let href = job["result"]["href"].as_str().unwrap();
            let revision = server.get(href).await;
            let document = server.document(&job).await;
            assert_eq!(
                revision.body, document.body,
                "{what}: the revision is the job's document"
            );
            assert!(
                revision
                    .header("fitment-digest")
                    .is_some_and(|d| d.starts_with("sha256:"))
            );
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// MF11, RA5, DC3: carrying decisions as `fitment socket FILE -o OUT`
/// carries OUT's: an extraction with `carry_from` an uploaded document
/// the CLI reviewed is the CLI's re-extraction onto that file, byte for
/// byte; extracting again carries from the line's current revision and is
/// `unchanged`; `carry: false` starts from the unreviewed document; a
/// document of another file is `422 carry_from_foreign` (exit 2), read at
/// the request (HX8), as the CLI refuses that OUT.
#[tokio::test]
async fn carrying_is_the_clis() {
    let server = TestServer::fresh();
    let name = "s02_cbore.stp";
    let bytes = support::assembly(name);
    let dir = file_in(&server.dirs, name, &bytes);
    let socket = |extra: &[&str]| {
        let mut args = vec!["socket", name, "--part", "BLOCK", "-o", "OUT.json"];
        args.extend_from_slice(extra);
        cli(&dir, &args)
    };
    assert_eq!(socket(&[]).0, Some(0));
    let review = cli(&dir, &["review", "OUT.json", "--keep", "bore:hole-427"]);
    assert_eq!(review.0, Some(0), "{}", review.2);
    let reviewed = std::fs::read(dir.join("OUT.json")).unwrap();
    assert_eq!(socket(&[]).0, Some(0));
    let carried = std::fs::read(dir.join("OUT.json")).unwrap();
    server.register(&bytes, name).await;
    let sha = fitment_core::sha256_hex(&bytes);
    let upload = server.upload(&reviewed).await;
    let body = json!({ "assembly": format!("sha256:{sha}"), "part": "BLOCK",
                       "carry_from": { "upload": format!("sha256:{upload}") } });
    let job = server.send(waiting("/v1/sockets", &body)).await.json();
    assert_eq!(job["state"], "succeeded", "{job}");
    let document = server.document(&job).await.body;
    assert!(fitment_corpus::parity::bytes("carried", &carried, &document).is_none());
    let line = server.get("/v1/sockets").await.json()["items"][0].clone();
    assert_eq!(line["current"]["carried"], 1);
    assert_eq!(line["current"]["unreviewed"], 3);
    let again = server.extract(&sha, "BLOCK").await;
    assert_eq!(
        again["outcome"], "unchanged",
        "carried from the current revision"
    );
    let fresh = json!({ "assembly": format!("sha256:{sha}"), "part": "BLOCK", "carry": false });
    let plain = server.send(waiting("/v1/sockets", &fresh)).await.json();
    assert!(plain["outcome"].is_null());
    assert_eq!(
        server.document(&plain).await.body,
        support::cli_golden("socket", "s02_cbore--BLOCK.json")
    );
    let other = support::cli_golden("socket", "s01_bolted--BRACKET.json");
    let foreign = server.upload(&other).await;
    let body = json!({ "assembly": format!("sha256:{sha}"), "part": "BLOCK",
                       "carry_from": { "upload": format!("sha256:{foreign}") } });
    // HX8 (PR 6): read at the request, before any job; the CLI's exit 2.
    let reply = server.send(waiting("/v1/sockets", &body)).await;
    assert_eq!(
        (reply.status.as_u16(), reply.code()),
        (422, "carry_from_foreign".to_owned())
    );
    assert_eq!(reply.json()["exit_code"], 2);
}

/// MF11, review S12 of PR 5: a `carry_from` of the same file that names
/// no target (an older document whose target was edited out) is neither
/// refused nor carried from, as the CLI's `carry_over` leaves such an OUT:
/// the extraction is the CLI's onto that file, byte for byte, and the
/// unreviewed document.
#[tokio::test]
async fn a_carry_from_naming_no_target_carries_nothing() {
    let server = TestServer::fresh();
    let name = "s02_cbore.stp";
    let bytes = support::assembly(name);
    let dir = file_in(&server.dirs, name, &bytes);
    assert_eq!(
        cli(&dir, &["socket", name, "--part", "BLOCK", "-o", "OUT.json"]).0,
        Some(0)
    );
    let review = cli(&dir, &["review", "OUT.json", "--keep", "bore:hole-427"]);
    assert_eq!(review.0, Some(0), "{}", review.2);
    let mut edited: Value =
        serde_json::from_slice(&std::fs::read(dir.join("OUT.json")).unwrap()).unwrap();
    edited["socket_rules_version"] = json!(fitment_core::SOCKET_RULES_VERSION - 1);
    edited["extracted"]
        .as_object_mut()
        .unwrap()
        .remove("target");
    let mut stated = edited.clone();
    for key in ["digest", "review", "stale_reviews"] {
        stated.as_object_mut().unwrap().remove(key);
    }
    let digest = fitment_core::sha256_hex(stated.to_string().as_bytes());
    edited["digest"] = json!(format!("sha256:{digest}"));
    let edited = serde_json::to_vec(&edited).unwrap();
    std::fs::write(dir.join("OUT.json"), &edited).unwrap();
    let again = cli(&dir, &["socket", name, "--part", "BLOCK", "-o", "OUT.json"]);
    assert_eq!(again.0, Some(0), "not refused: {}", again.2);
    let carried = std::fs::read(dir.join("OUT.json")).unwrap();
    assert_eq!(
        carried,
        support::cli_golden("socket", "s02_cbore--BLOCK.json"),
        "nothing carried"
    );
    server.register(&bytes, name).await;
    let sha = fitment_core::sha256_hex(&bytes);
    let upload = server.upload(&edited).await;
    let body = json!({ "assembly": format!("sha256:{sha}"), "part": "BLOCK",
                       "carry_from": { "upload": format!("sha256:{upload}") } });
    let job = server.send(waiting("/v1/sockets", &body)).await.json();
    assert_eq!(job["state"], "succeeded", "{job}");
    let document = server.document(&job).await.body;
    assert!(fitment_corpus::parity::bytes("carried", &carried, &document).is_none());
}

/// The exit's file half for PR 5 (section 12, item 5's "done when"): every
/// assembly of the corpus registered through `assembly_read` gives `fitment
/// parts FILE --format json`'s bytes and exit code, and every socket its
/// cases name (the match cases' and the assemblies' own) extracted through
/// `socket_extract` gives `fitment socket FILE --part P --format json`'s.
/// Release only, as the corpus suites are.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_corpus_source_is_the_clis_through_jobs() {
    let Some(corpus) = fitment_corpus::corpus::Corpus::locate_release_only() else {
        return;
    };
    let dirs = Dirs::new();
    let mut config = dirs.config();
    config.auth.rate_per_second = 10_000;
    config.auth.rate_burst = 10_000;
    let server = TestServer::in_process(dirs, config);
    let mut sockets = std::collections::BTreeSet::new();
    let mut files = std::collections::BTreeMap::new();
    for case in corpus.cases() {
        let id = case["id"].as_str().unwrap().to_owned();
        if case["kind"] == "assembly" {
            files.insert(id.clone(), case["file"].as_str().unwrap().to_owned());
            if let Some(targets) = case["expect"]["sockets"].as_object() {
                for part in targets.keys() {
                    sockets.insert((id.clone(), part.clone()));
                }
            }
        }
        if case["kind"] == "match" {
            let s = &case["socket"];
            sockets.insert((
                s["case"].as_str().unwrap().to_owned(),
                s["part"].as_str().unwrap().to_owned(),
            ));
        }
    }
    // A match case's source may be an assembly no case of its own states
    // (a mirror's): its file is the case's name.
    for (source, _) in &sockets {
        files
            .entry(source.clone())
            .or_insert_with(|| format!("{source}.stp"));
    }
    let mut failures = Vec::new();
    for (id, file) in &files {
        let bytes = std::fs::read(corpus.dir().join(file)).unwrap();
        let job = server.register(&bytes, file).await;
        let run = cli(corpus.dir(), &["parts", file, "--format", "json"]);
        same(&server, file, &job, run, &mut failures).await;
        let _ = id;
    }
    for (source, part) in &sockets {
        let file = &files[source];
        let sha = fitment_core::sha256_hex(&std::fs::read(corpus.dir().join(file)).unwrap());
        let job = server.extract(&sha, part).await;
        let run = cli(
            corpus.dir(),
            &["socket", file, "--part", part, "--format", "json"],
        );
        same(
            &server,
            &format!("{file} --part {part}"),
            &job,
            run,
            &mut failures,
        )
        .await;
    }
    eprintln!(
        "PR 5's corpus parity: {} assemblies, {} sockets, {} differences",
        files.len(),
        sockets.len(),
        failures.len()
    );
    assert!(files.len() >= 200 && sockets.len() >= 33);
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// JB10: the CLI's stderr lines as the job's notes state them: without
/// `fitment: `, the names it ran on (`names`) and `warning: `, and with
/// the CLI's hints replaced by the endpoints that do the same.
fn cli_notes(stderr: &str, names: &[&str]) -> Vec<String> {
    stderr
        .lines()
        .map(|line| {
            let mut line = line.strip_prefix("fitment: ").unwrap_or(line).to_owned();
            if let Some(name) = names.iter().find(|n| line.starts_with(&format!("{n}: "))) {
                line = line[name.len() + 2..].to_owned();
            }
            let line = line.strip_prefix("warning: ").unwrap_or(&line).to_owned();
            line.replace(
                "(`fitment review --remove ID --note …`)",
                "(POST /v1/sockets/{socket}/review)",
            )
            .replace("--require-fit:", "require_fit:")
        })
        .collect()
}

/// The messages of a job's notes, but the API's own (RA10's).
fn api_notes(job: &Value) -> Vec<String> {
    job["notes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|n| n["code"] != "unverified_socket")
        .map(|n| n["message"].as_str().unwrap().to_owned())
        .collect()
}

/// The API's `policy` and `options` for the CLI's flags (DC7), and the
/// flags' name for a failure.
fn variants() -> Vec<(&'static str, Value, Vec<&'static str>)> {
    vec![
        ("default", json!({}), vec![]),
        (
            "functional",
            json!({"policy": {"clearance": "functional"}}),
            vec!["--clearance", "functional"],
        ),
        (
            "strict",
            json!({"policy": {"clearance": "strict"}}),
            vec!["--clearance", "strict"],
        ),
        (
            "0.05 mm",
            json!({"policy": {"tolerance_mm": 0.05}}),
            vec!["--tolerance-mm", "0.05"],
        ),
        (
            "0.5 deg",
            json!({"policy": {"tolerance_deg": 0.5}}),
            vec!["--tolerance-deg", "0.5"],
        ),
        (
            "require fit",
            json!({"options": {"require_fit": true}}),
            vec!["--require-fit"],
        ),
    ]
}

/// VT7's `match` row (DC1, DC3, DC7, JB4, JB10, XH7): `fitment match S
/// FILE… --format json` with each of the CLI's policy and option flags, and
/// the job `match` over the same uploads with the same labels and the
/// request's fields for those flags: the document byte for byte, the
/// state and exit code the CLI's exit code maps to, and the notes the
/// CLI's stderr lines; `--require-reviewed` on an unreviewed socket is
/// exit 3 with nothing printed, and `422 socket_unreviewed` before any job
/// (JB2). On the committed candidates and assemblies, every OS.
#[tokio::test]
async fn matches_are_the_clis() {
    let server = TestServer::fresh();
    let dir = support::candidates_dir();
    let mut failures = Vec::new();
    let mut runs = 0;
    for ((case, part), files) in support::candidates_by_socket() {
        let socket = server.socket_of(&case, &part).await;
        let doc = server.get(&format!("/v1/sockets/{socket}")).await.body;
        let scratch = server.dirs.root.join("cli");
        std::fs::create_dir_all(&scratch).unwrap();
        let socket_file = scratch.join(format!("{case}--{part}.json"));
        std::fs::write(&socket_file, &doc).unwrap();
        let socket_arg = socket_file.display().to_string();
        let mut uploads = Vec::new();
        for file in &files {
            let sha = server.upload(&support::candidate(file)).await;
            uploads.push(json!({"upload": format!("sha256:{sha}"), "label": file}));
        }
        let mut names: Vec<&str> = files.iter().map(String::as_str).collect();
        names.push(&socket_arg);
        for (what, extra, flags) in variants() {
            let mut body = json!({"socket": socket, "candidates": {"files": uploads}});
            for (key, value) in extra.as_object().unwrap() {
                body[key] = value.clone();
            }
            let job = server.run_match(&body).await;
            let mut args = vec!["match", socket_arg.as_str()];
            args.extend(files.iter().map(String::as_str));
            args.extend(["--format", "json"]);
            args.extend(flags.iter().copied());
            let run = cli(&dir, &args);
            let what = format!("{case} {part} {what}");
            let notes = (cli_notes(&run.2, &names), api_notes(&job));
            if notes.0 != notes.1 {
                failures.push(format!(
                    "{what}: the notes {:?}, the CLI's {:?}",
                    notes.1, notes.0
                ));
            }
            let run = (run.0, run.1, String::new());
            same(&server, &what, &job, run, &mut failures).await;
            runs += 1;
        }
        let body = json!({"socket": socket, "candidates": {"files": uploads},
                          "options": {"require_reviewed": true}});
        let reply = server.send(waiting("/v1/matches", &body)).await;
        assert_eq!(
            (reply.status.as_u16(), reply.code()),
            (422, "socket_unreviewed".to_owned())
        );
        let mut args = vec!["match", socket_arg.as_str()];
        args.extend(files.iter().map(String::as_str));
        args.extend(["--format", "json", "--require-reviewed"]);
        let (code, stdout, _) = cli(&dir, &args);
        assert_eq!((code, stdout.is_empty()), (Some(3), true), "{case} {part}");
    }
    assert!(runs >= 24, "{runs} runs");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The committed sockets the CLI's goldens name: (case, part).
fn golden_targets() -> Vec<(String, String)> {
    let golden = Path::new(env!("CARGO_MANIFEST_DIR")).join("../fitment/tests/golden/socket");
    let mut targets: Vec<(String, String)> = std::fs::read_dir(&golden)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter_map(|n| {
            let (case, part) = n.strip_suffix(".json")?.split_once("--")?;
            Some((case.to_owned(), part.to_owned()))
        })
        .collect();
    targets.sort();
    targets
}

/// VT7's `review` row (OB5, RA5, DC1): `fitment review FILE --keep A
/// --socket S`, `--remove B --note T --socket S` and `--unreview A
/// --socket S`, one run per decision, and one review of the three in that
/// order, all or none: the same bytes, the new revision's; a decision
/// naming no requirement is the CLI's exit 2 and `422
/// review_unknown_requirement` with `exit_code` 2, nothing written.
#[tokio::test]
async fn reviews_are_the_clis() {
    let server = TestServer::fresh();
    let mut failures = Vec::new();
    let mut reviewed = 0;
    for (case, part) in golden_targets() {
        let socket = server.socket_of(&case, &part).await;
        let document = server.get(&format!("/v1/sockets/{socket}")).await.body;
        let doc: Value = serde_json::from_slice(&document).unwrap();
        let Some(first) = doc["extracted"]["sockets"][0].as_object() else {
            continue;
        };
        let id = first["id"].as_str().unwrap();
        let requirements: Vec<&str> = first["requirements"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["id"].as_str().unwrap())
            .collect();
        if requirements.len() < 2 {
            continue;
        }
        let (a, b) = (requirements[0], requirements[1]);
        let dir = file_in(&server.dirs, "S.json", &document);
        for args in [
            vec!["--keep", a, "--socket", id],
            vec!["--remove", b, "--note", "checked by hand", "--socket", id],
            vec!["--unreview", a, "--socket", id],
        ] {
            let mut all = vec!["review", "S.json"];
            all.extend(args);
            assert_eq!(cli(&dir, &all).0, Some(0), "{case} {part}");
        }
        let expected = std::fs::read(dir.join("S.json")).unwrap();
        let body = json!({ "decisions": [
            { "requirement": a, "socket": id, "status": "kept" },
            { "requirement": b, "socket": id, "status": "removed", "note": "checked by hand" },
            { "requirement": a, "socket": id, "status": "unreviewed" } ] });
        let reply = server
            .send(support::Call::json(
                axum::http::Method::POST,
                &format!("/v1/sockets/{socket}/review"),
                &body,
            ))
            .await;
        assert_eq!(reply.status, 201, "{case} {part}");
        if let Some(diff) =
            fitment_corpus::parity::bytes(&format!("{case} {part}"), &expected, &reply.body)
        {
            failures.push(diff);
        }
        let current = reply
            .header("location")
            .unwrap()
            .trim_start_matches("/v1/sockets/")
            .to_owned();
        let unknown = json!({ "decisions": [{ "requirement": "bore:nowhere", "status": "kept" }] });
        let refused = server
            .send(support::Call::json(
                axum::http::Method::POST,
                &format!("/v1/sockets/{current}/review"),
                &unknown,
            ))
            .await;
        assert_eq!(refused.code(), "review_unknown_requirement");
        assert_eq!(refused.json()["exit_code"], 2);
        let (code, _, _) = cli(&dir, &["review", "S.json", "--keep", "bore:nowhere"]);
        assert_eq!(code, Some(2));
        assert_eq!(
            std::fs::read(dir.join("S.json")).unwrap(),
            expected,
            "nothing written"
        );
        reviewed += 1;
    }
    assert!(reviewed >= 5, "{reviewed} sockets reviewed");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// DC3, CO3, CO5, M6-5: component candidates have no CLI: the reference
/// is the library's `MatchDocument` of `judge_component` with the labels
/// the API used, byte for byte: each committed target as a component of
/// its own assembly, another part of it, one named twice (one entry, both
/// labels: CO3) and one by its occurrence, under the default label
/// `component:<sha256, first 12>/#<n>` where none is given.
#[tokio::test]
async fn component_matches_are_the_librarys() {
    use fitment_core::{
        MatchDocument, MatchOptions, MatchPolicy, Matcher, NoFitVerifier, SelectedComponent,
    };
    let server = TestServer::fresh();
    let mut failures = Vec::new();
    for ((case, part), _) in support::candidates_by_socket() {
        let socket = server.socket_of(&case, &part).await;
        let text =
            String::from_utf8(server.get(&format!("/v1/sockets/{socket}")).await.body).unwrap();
        let document = fitment_core::SocketDocument::from_json(&text).unwrap();
        let bytes = support::assembly(&format!("{case}.stp"));
        let sha = fitment_core::sha256_hex(&bytes);
        let occurrence = document.extracted().unwrap().occurrences[0]
            .path
            .iter()
            .map(|n| format!("#{n}"))
            .collect::<Vec<_>>()
            .join("/");
        let asked = [
            (Some(part.as_str()), None, Some("mine")),
            (Some(part.as_str()), None, Some("again")),
            (None, Some(occurrence.as_str()), None),
        ];
        let components: Vec<Value> = asked
            .iter()
            .map(|(p, o, l)| json!({ "assembly": format!("sha256:{sha}"), "part": p, "occurrence": o, "label": l }))
            .collect();
        let job = server
            .run_match(&json!({ "socket": socket, "candidates": { "components": components } }))
            .await;
        let matcher = Matcher::new(
            &document,
            MatchPolicy::default(),
            MatchOptions::default(),
            &NoFitVerifier,
        )
        .unwrap();
        let results = asked
            .iter()
            .map(|(p, o, l)| {
                let component = SelectedComponent::read(&bytes, *p, *o).unwrap();
                let label = l.map_or_else(
                    || format!("component:{}/#{}", &sha[..12], component.definition()),
                    str::to_owned,
                );
                (
                    matcher.judge_component(&component, component.source()),
                    vec![label],
                )
            })
            .collect();
        let expected = MatchDocument::new(&matcher, results).to_json();
        let doc: Value = serde_json::from_str(&expected).unwrap();
        assert_eq!(
            doc["summary"]["candidates"], 1,
            "one component, one entry (CO3)"
        );
        let got = server.document(&job).await.body;
        if let Some(diff) =
            fitment_corpus::parity::bytes(&format!("{case} {part}"), expected.as_bytes(), &got)
        {
            failures.push(diff);
        }
        let exit = fitment_corpus::parity::exit_code(
            "succeeded",
            Some(fitment_corpus::parity::outcome_of(&doc)),
        );
        assert_eq!(
            job["exit_code"].as_i64(),
            exit.map(i64::from),
            "{case} {part}: JB4"
        );
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// VT7's `--original` row (VD15, SN9): `fitment match S FILE… --original
/// ASM --format json` and a match with `original: true` over the same
/// uploads: the same bytes, state, exit code and notes.
#[tokio::test]
async fn originals_are_the_clis() {
    let server = TestServer::fresh();
    let dir = support::candidates_dir();
    let mut failures = Vec::new();
    for ((case, part), files) in support::candidates_by_socket() {
        let socket = server.socket_of(&case, &part).await;
        let doc = server.get(&format!("/v1/sockets/{socket}")).await.body;
        let scratch = server.dirs.root.join("original");
        std::fs::create_dir_all(&scratch).unwrap();
        let socket_file = scratch.join("S.json");
        std::fs::write(&socket_file, &doc).unwrap();
        let assembly_file = scratch.join(format!("{case}.stp"));
        std::fs::write(&assembly_file, support::assembly(&format!("{case}.stp"))).unwrap();
        let mut uploads = Vec::new();
        for file in &files {
            let sha = server.upload(&support::candidate(file)).await;
            uploads.push(json!({"upload": format!("sha256:{sha}"), "label": file}));
        }
        let job = server
            .run_match(
                &json!({"socket": socket, "candidates": {"files": uploads}, "original": true}),
            )
            .await;
        let (socket_arg, original_arg) = (
            socket_file.display().to_string(),
            assembly_file.display().to_string(),
        );
        let mut args = vec!["match", socket_arg.as_str()];
        args.extend(files.iter().map(String::as_str));
        args.extend(["--original", original_arg.as_str(), "--format", "json"]);
        let run = cli(&dir, &args);
        let mut names: Vec<&str> = files.iter().map(String::as_str).collect();
        names.extend([socket_arg.as_str(), original_arg.as_str(), "original"]);
        let notes = (cli_notes(&run.2, &names), api_notes(&job));
        if notes.0 != notes.1 {
            failures.push(format!(
                "{case} {part}: the notes {:?}, the CLI's {:?}",
                notes.1, notes.0
            ));
        }
        let run = (run.0, run.1, String::new());
        same(
            &server,
            &format!("{case} {part} original"),
            &job,
            run,
            &mut failures,
        )
        .await;
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// VT7's MF11 row: `fitment socket FILE --part P -o OUT` onto an OUT of
/// older socket rules the CLI reviewed, and an extraction whose
/// `carry_from` is that file uploaded, read leniently: the same bytes,
/// the decisions carried, and the same carry line in the notes.
#[tokio::test]
async fn an_older_socket_carries_as_the_cli_carries_it() {
    let server = TestServer::fresh();
    let name = "s02_cbore.stp";
    let bytes = support::assembly(name);
    let dir = file_in(&server.dirs, name, &bytes);
    assert_eq!(
        cli(&dir, &["socket", name, "--part", "BLOCK", "-o", "OUT.json"]).0,
        Some(0)
    );
    let review = cli(&dir, &["review", "OUT.json", "--keep", "bore:hole-427"]);
    assert_eq!(review.0, Some(0), "{}", review.2);
    let mut older: Value =
        serde_json::from_slice(&std::fs::read(dir.join("OUT.json")).unwrap()).unwrap();
    older["socket_rules_version"] = json!(fitment_core::SOCKET_RULES_VERSION - 1);
    let mut stated = older.clone();
    for key in ["digest", "review", "stale_reviews"] {
        stated.as_object_mut().unwrap().remove(key);
    }
    older["digest"] = json!(format!(
        "sha256:{}",
        fitment_core::sha256_hex(stated.to_string().as_bytes())
    ));
    let older = serde_json::to_vec_pretty(&older).unwrap();
    std::fs::write(dir.join("OUT.json"), &older).unwrap();
    let run = cli(&dir, &["socket", name, "--part", "BLOCK", "-o", "OUT.json"]);
    assert_eq!(run.0, Some(0), "{}", run.2);
    let carried = std::fs::read(dir.join("OUT.json")).unwrap();
    server.register(&bytes, name).await;
    let upload = server.upload(&older).await;
    let sha = fitment_core::sha256_hex(&bytes);
    let body = json!({ "assembly": format!("sha256:{sha}"), "part": "BLOCK",
                       "carry_from": { "upload": format!("sha256:{upload}") } });
    let job = server.send(waiting("/v1/sockets", &body)).await.json();
    assert_eq!(job["state"], "succeeded", "{job}");
    let document = server.document(&job).await.body;
    assert!(fitment_corpus::parity::bytes("carried", &carried, &document).is_none());
    assert_eq!(cli_notes(&run.2, &[name, "OUT.json"]), api_notes(&job));
    assert_eq!(job["notes"][0]["code"], "carried_over");
}

/// JB2: the gate again at dispatch: a match queued, then the build's
/// socket rules raised (as a restart into an upgraded build does) ends
/// `refused`, exit 3, `socket_stale`, before any candidate is read (its
/// one candidate's stored bytes are damaged, which a read would fail on);
/// one queued on an unreviewed socket before a restart that sets the
/// `require_reviewed` floor ends `refused`, `socket_unreviewed`. Neither
/// states a verdict.
#[tokio::test]
async fn the_gate_is_checked_again_at_dispatch() {
    let server = TestServer::fresh();
    let socket = server.socket_of("s02_cbore", "BLOCK").await;
    let candidate = support::candidate("m_block02.stp");
    let sha = server.upload(&candidate).await;
    let body = json!({ "socket": socket, "candidates": { "files": [
        { "upload": format!("sha256:{sha}"), "label": "m_block02.stp" } ] } });
    server.server().hold_jobs();
    let queued = server
        .send(support::Call::json(
            axum::http::Method::POST,
            "/v1/matches",
            &body,
        ))
        .await
        .json();
    let path = server
        .dirs
        .state
        .join(format!("objects/sha256/{}/{sha}", &sha[..2]));
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    perms.set_readonly(false);
    std::fs::set_permissions(&path, perms.clone()).unwrap();
    std::fs::write(&path, b"damaged").unwrap();
    server.server().set_rules_shift(1);
    server.server().resume_jobs();
    let id = queued["id"].as_str().unwrap();
    let job = server
        .finished_job(id, std::time::Duration::from_secs(60))
        .await;
    assert_eq!(
        (
            job["state"].as_str(),
            job["exit_code"].as_u64(),
            job["error"]["code"].as_str()
        ),
        (Some("refused"), Some(3), Some("socket_stale")),
        "{job}"
    );
    assert!(job["document"].is_null() && job["outcome"].is_null());
    std::fs::write(&path, &candidate).unwrap();
    server.server().set_rules_shift(0);
    server.server().hold_jobs();
    let queued = server
        .send(support::Call::json(
            axum::http::Method::POST,
            "/v1/matches",
            &body,
        ))
        .await
        .json();
    let mut config = server.dirs.config();
    config.matching.require_reviewed_floor = true;
    let server = server.restart(config);
    let id = queued["id"].as_str().unwrap();
    let job = server
        .finished_job(id, std::time::Duration::from_secs(60))
        .await;
    assert_eq!(
        (
            job["state"].as_str(),
            job["exit_code"].as_u64(),
            job["error"]["code"].as_str()
        ),
        (Some("refused"), Some(3), Some("socket_unreviewed")),
        "{job}"
    );
    assert!(job["outcome"].is_null());
}

/// `file` with a second `socket_rules_version` of `rules` as its last key.
fn duplicated(file: &[u8], rules: u32) -> Vec<u8> {
    let text = std::str::from_utf8(file).unwrap().trim_end();
    let body = text.strip_suffix('}').unwrap().trim_end();
    format!("{body},\n  \"socket_rules_version\": {rules}\n}}\n").into_bytes()
}

/// UP6, JB2, VD9: an uploaded socket document is read as `fitment match`
/// reads a socket file: stale, its digest broken, refused, not JSON, not
/// UTF-8, an array, a duplicated version key, and unreviewed under
/// `require_reviewed`, each refused before any job with the CLI's exit
/// code 3 (nothing on its stdout), stale on both sides or on neither; the
/// file itself, uploaded, gives the CLI's document.
#[tokio::test]
async fn an_uploaded_socket_is_read_as_the_cli_reads_it() {
    let server = TestServer::fresh();
    let file = support::cli_golden("socket", "s01_bolted--BRACKET.json");
    let candidate = support::candidate("m_bracket.stp");
    let candidate_sha = server.upload(&candidate).await;
    let edited = |edit: &dyn Fn(&mut Value)| {
        let mut doc: Value = serde_json::from_slice(&file).unwrap();
        edit(&mut doc);
        serde_json::to_vec_pretty(&doc).unwrap()
    };
    let refused = fitment_core::SocketDocument::extract(b"no STEP file", Some("X"), None)
        .unwrap()
        .to_json();
    let cases: Vec<(&str, Vec<u8>, bool, &str)> = vec![
        (
            "stale",
            edited(&|d| d["socket_rules_version"] = json!(fitment_core::SOCKET_RULES_VERSION - 1)),
            false,
            "socket_stale",
        ),
        (
            "digest",
            edited(&|d| d["extracted"]["sockets"][0]["requirements"][0]["floor_mm"] = json!(0.5)),
            false,
            "socket_unreadable",
        ),
        ("refused", refused.into_bytes(), false, "socket_refused"),
        (
            "not utf-8",
            b"\xff\xfe not a document".to_vec(),
            false,
            "socket_unreadable",
        ),
        (
            "not json",
            b"{ this is not JSON".to_vec(),
            false,
            "socket_unreadable",
        ),
        ("unreviewed", file.clone(), true, "socket_unreviewed"),
        // CO2: the versions read by one core helper on both sides: an
        // array states none, and of a duplicated key the last counts.
        ("an array", b"[6, 6]".to_vec(), false, "socket_unreadable"),
        (
            "a duplicated key",
            duplicated(&file, fitment_core::SOCKET_RULES_VERSION - 1),
            false,
            "socket_stale",
        ),
        (
            "a duplicated key, current last",
            duplicated(&file, fitment_core::SOCKET_RULES_VERSION),
            false,
            "socket_unreadable",
        ),
        // SN6: a report is never input: a match document is no socket.
        (
            "a match document",
            support::cli_golden("match", "s01_bolted--BRACKET.json"),
            false,
            "socket_unreadable",
        ),
    ];
    let dir = file_in(&server.dirs, "m_bracket.stp", &candidate);
    for (what, bytes, reviewed, code) in cases {
        let sha = server.upload(&bytes).await;
        let body = json!({ "socket": { "upload": format!("sha256:{sha}") },
                           "options": { "require_reviewed": reviewed },
                           "candidates": { "files": [
                               { "upload": format!("sha256:{candidate_sha}"), "label": "m_bracket.stp" } ] } });
        let reply = server.send(waiting("/v1/matches", &body)).await;
        assert_eq!(
            (reply.status.as_u16(), reply.code().as_str()),
            (422, code),
            "{what}"
        );
        assert_eq!(reply.json()["exit_code"], 3, "{what}");
        std::fs::write(dir.join("S.json"), &bytes).unwrap();
        let mut args = vec!["match", "S.json", "m_bracket.stp", "--format", "json"];
        if reviewed {
            args.push("--require-reviewed");
        }
        let (exit, stdout, stderr) = cli(&dir, &args);
        // Bytes that are not UTF-8 are no socket document to either (exit
        // 3); the CLI names staleness exactly where the API does.
        assert_eq!((exit, stdout.is_empty()), (Some(3), true), "{what}");
        assert_eq!(
            stderr.contains("extract it again"),
            code == "socket_stale",
            "{what}: {stderr}"
        );
    }
    let sha = server.upload(&file).await;
    let body = json!({ "socket": { "upload": format!("sha256:{sha}") },
                       "candidates": { "files": [
                           { "upload": format!("sha256:{candidate_sha}"), "label": "m_bracket.stp" } ] } });
    let job = server.run_match(&body).await;
    std::fs::write(dir.join("S.json"), &file).unwrap();
    let run = cli(
        &dir,
        &["match", "S.json", "m_bracket.stp", "--format", "json"],
    );
    let mut failures = Vec::new();
    let run = (run.0, run.1, String::new());
    same(&server, "uploaded socket", &job, run, &mut failures).await;
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// SN2: no request field overrides a refusal: `force`, `skip_stale` or
/// `audit` anywhere is `400 bad_request` (VN6's unknown fields), as the
/// CLI has no such flag; a stale socket stays refused whatever else the
/// request says.
#[tokio::test]
async fn no_field_overrides_a_refusal() {
    let server = TestServer::fresh();
    let socket = server.socket_of("s01_bolted", "BRACKET").await;
    let sha = server.upload(&support::candidate("m_bracket.stp")).await;
    let files = json!([{ "upload": format!("sha256:{sha}"), "label": "m_bracket.stp" }]);
    for body in [
        json!({ "socket": socket, "candidates": { "files": files }, "force": true }),
        json!({ "socket": socket, "candidates": { "files": files }, "options": { "skip_stale": true } }),
        json!({ "socket": socket, "candidates": { "files": files }, "policy": { "audit": true } }),
        json!({ "socket": socket, "candidates": { "files": files, "force": true } }),
    ] {
        let reply = server.send(waiting("/v1/matches", &body)).await;
        assert_eq!(
            (reply.status.as_u16(), reply.code()),
            (400, "bad_request".to_owned()),
            "{body}"
        );
    }
    server.server().set_rules_shift(1);
    let body = json!({ "socket": socket, "candidates": { "files": files },
                       "options": { "require_reviewed": false, "require_fit": false } });
    let reply = server.send(waiting("/v1/matches", &body)).await;
    assert_eq!(reply.code(), "socket_stale");
}

/// The committed unit parts and candidates and a file that is no STEP
/// file, written to `root/in/` for the CLI: each one's bytes, its path as
/// the CLI's expansion of `in` names it, and its name.
fn catalog_inputs(root: &Path) -> Vec<(Vec<u8>, String, String)> {
    let core = Path::new(env!("CARGO_MANIFEST_DIR")).join("../fitment-core/tests");
    let dir = root.join("in");
    std::fs::create_dir_all(&dir).unwrap();
    let mut files: Vec<(Vec<u8>, String, String)> = ["data", "candidates"]
        .iter()
        .flat_map(|d| std::fs::read_dir(core.join(d)).unwrap())
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "stp"))
        .map(|p| {
            let name = p.file_name().unwrap().to_string_lossy().into_owned();
            (std::fs::read(&p).unwrap(), name)
        })
        .chain([(b"not STEP".to_vec(), "junk.stp".to_owned())])
        .map(|(bytes, name)| {
            std::fs::write(dir.join(&name), &bytes).unwrap();
            let input = Path::new("in").join(&name).to_string_lossy().into_owned();
            (bytes, input, name)
        })
        .collect();
    files.sort_by(|a, b| b.1.cmp(&a.1));
    files
}

/// `fitment catalog ARGS -C cli --format json` in `root`: its stdout,
/// exit 0 or 3 (a quarantined file, a problem found).
fn catalog_cli(root: &Path, args: &[&str]) -> Vec<u8> {
    let args = [&["catalog"], args, &["-C", "cli", "--format", "json"]].concat();
    let (code, stdout, stderr) = cli(root, &args);
    assert!(matches!(code, Some(0 | 3)), "{args:?}: {code:?} {stderr}");
    stdout
}

/// `document` with its generation written as `_`: DB3's batches end after
/// a second as well as after 64 files, so how many generations an ingest
/// of many files raises depends on its time, in the CLI as in the library.
fn without_generation(document: &[u8]) -> Vec<u8> {
    let text = String::from_utf8(document.to_vec()).unwrap();
    let mut lines: Vec<String> = text.split('\n').map(str::to_owned).collect();
    if let Some(line) = lines
        .iter_mut()
        .find(|l| l.starts_with("  \"generation\": "))
    {
        "  \"generation\": _,".clone_into(line);
    }
    lines.join("\n").into_bytes()
}

/// CA1 (`M6_PARITY`): an ingest from bytes, read as the server reads its
/// objects and named as the CLI was given them, prints the CLI's
/// `fitment.catalog` byte for byte: ids, order, lines and entries, its
/// generation aside, which the batches' time decides; and one file's, one
/// batch, whole.
#[test]
fn catalog_ingests_from_bytes_are_the_clis() {
    let dirs = Dirs::bare();
    let files = catalog_inputs(&dirs.root);
    assert!(files.len() > 20);
    cli(&dirs.root, &["catalog", "init", "-C", "cli"]);
    let expected = catalog_cli(&dirs.root, &["ingest", "in"]);
    let (mut catalog, _) = Catalog::init(&dirs.root.join("lib"), Options::default()).unwrap();
    let inputs: Vec<Input<'_>> = files
        .iter()
        .map(|(bytes, input, name)| Input::new(bytes, input, name))
        .collect();
    let run = catalog.ingest_inputs(&inputs, None, &NoObserver).unwrap();
    let snapshot = catalog.snapshot().unwrap();
    let document = CatalogDocument::ingested(&snapshot, &run)
        .unwrap()
        .to_json();
    let (expected, document) = (
        without_generation(&expected),
        without_generation(document.as_bytes()),
    );
    if let Some(diff) = fitment_corpus::parity::bytes("ingest", &expected, &document) {
        panic!("{diff}");
    }
    // One file, one batch: the generation too.
    cli(&dirs.root, &["catalog", "init", "-C", "one"]);
    let args = [
        "catalog",
        "ingest",
        "-C",
        "one",
        &files[0].1,
        "--format",
        "json",
    ];
    let (_, expected, _) = cli(&dirs.root, &args);
    let (mut one, _) = Catalog::init(&dirs.root.join("lib-one"), Options::default()).unwrap();
    let run = one.ingest_inputs(&inputs[..1], None, &NoObserver).unwrap();
    let snapshot = one.snapshot().unwrap();
    let document = CatalogDocument::ingested(&snapshot, &run)
        .unwrap()
        .to_json();
    if let Some(diff) = fitment_corpus::parity::bytes("one", &expected, document.as_bytes()) {
        panic!("{diff}");
    }
}

/// Two catalogs of the same files, `root/cli` by the CLI and `root/lib` by
/// the library: each file ingested alone, one batch, so both stand at one
/// generation whatever DB3's second makes of a batch.
fn twins(root: &Path) -> (Catalog, Vec<(Vec<u8>, String, String)>) {
    let files = catalog_inputs(root);
    cli(root, &["catalog", "init", "-C", "cli"]);
    let (mut catalog, _) = Catalog::init(&root.join("lib"), Options::default()).unwrap();
    for (bytes, input, name) in &files {
        catalog_cli(root, &["ingest", input]);
        let one = [Input::new(bytes, input, name)];
        catalog.ingest_inputs(&one, None, &NoObserver).unwrap();
    }
    (catalog, files)
}

/// Writes `bytes` at `path` and marks it written long ago: abandoned, not
/// in flight.
fn abandoned(path: &Path, bytes: &[u8]) {
    std::fs::write(path, bytes).unwrap();
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(86_400))
        .unwrap();
}

/// CU2 on damaged catalogs (the review of PR 7a): the two catalogs
/// damaged alike, a blob's bytes altered, an orphan and an old temporary
/// file planted, the library's `fitment.verify`, plain and deep, and
/// `fitment.gc` are the CLI's bytes, and so is verify after gc.
#[test]
fn catalog_verify_and_gc_of_damage_are_the_clis() {
    let dirs = Dirs::bare();
    let (mut catalog, files) = twins(&dirs.root);
    let bracket = files.iter().find(|f| f.2 == "m_bracket.stp").unwrap();
    let sha = fitment_core::sha256_hex(&bracket.0);
    let orphan = fitment_core::sha256_hex(b"an orphan");
    for side in ["cli", "lib"] {
        let blobs = dirs.root.join(side).join("blobs/sha256");
        let blob = blobs.join(&sha[..2]).join(&sha);
        let mut writable = std::fs::metadata(&blob).unwrap().permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        writable.set_readonly(false);
        std::fs::set_permissions(&blob, writable).unwrap();
        std::fs::write(&blob, b"other").unwrap();
        abandoned(&blobs.join(&orphan[..2]).join(&orphan), b"an orphan");
        abandoned(&dirs.root.join(side).join("tmp/old.part"), b"abandoned");
    }
    let mut failures = Vec::new();
    let mut same = |what: &str, cli: Vec<u8>, library: String| {
        failures.extend(fitment_corpus::parity::bytes(
            what,
            &cli,
            library.as_bytes(),
        ));
    };
    for deep in [false, true] {
        let args: &[&str] = if deep {
            &["verify", "--deep"]
        } else {
            &["verify"]
        };
        let cli_doc = catalog_cli(&dirs.root, args);
        let verified = catalog.verify_observed(deep, false, &NoObserver).unwrap();
        assert_eq!(verified.problems.len(), 3, "{:?}", verified.problems);
        same(
            "verify",
            cli_doc,
            VerifyDocument::new(&verified, deep, false).to_json(),
        );
    }
    let cli_doc = catalog_cli(&dirs.root, &["gc", "--versions"]);
    let collected = catalog.gc(true, false).unwrap();
    same(
        "gc",
        cli_doc,
        GcDocument::new(&collected, true, false).to_json(),
    );
    let cli_doc = catalog_cli(&dirs.root, &["verify"]);
    let verified = catalog.verify_observed(false, false, &NoObserver).unwrap();
    same(
        "verify after gc",
        cli_doc,
        VerifyDocument::new(&verified, false, false).to_json(),
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// CU1 and CU2 (`M6_PARITY`): the library's document of each change, the
/// one the API serves, is the CLI's `--format json` byte for byte, for a
/// replace, a reindex, `prune --revisions`, a delete, `verify --deep` and
/// `gc --versions`, the two catalogs changed alike: each file ingested
/// alone, one batch, so both stand at one generation.
#[test]
fn catalog_documents_of_changes_are_the_clis() {
    let dirs = Dirs::bare();
    let (mut catalog, files) = twins(&dirs.root);
    let mut failures = Vec::new();
    let mut same = |what: &str, cli: Vec<u8>, library: String| {
        failures.extend(fitment_corpus::parity::bytes(
            what,
            &cli,
            library.as_bytes(),
        ));
    };
    // A replace: the bracket's bytes become another bracket's, its header
    // naming another file.
    let bracket = files.iter().find(|f| f.2 == "m_bracket.stp").unwrap();
    let text = String::from_utf8(bracket.0.clone()).unwrap();
    let next = text
        .replacen("FILE_NAME('", "FILE_NAME('next", 1)
        .into_bytes();
    std::fs::create_dir_all(dirs.root.join("next")).unwrap();
    std::fs::write(dirs.root.join("next/m_bracket.stp"), &next).unwrap();
    let given = Path::new("next")
        .join("m_bracket.stp")
        .to_string_lossy()
        .into_owned();
    let cli_doc = catalog_cli(&dirs.root, &["replace", "m_bracket", &given]);
    let reference = Reference::parse("m_bracket").unwrap();
    let input = Input::new(&next, &given, "m_bracket.stp");
    let done = catalog
        .replace_input(&reference, &input, None, &NoObserver)
        .unwrap()
        .unwrap();
    let snapshot = catalog.snapshot().unwrap();
    let document = CatalogDocument::replaced(&snapshot, &given, &done).unwrap();
    same("replace", cli_doc, document.to_json());
    drop(snapshot);
    let cli_doc = catalog_cli(&dirs.root, &["reindex", "--all"]);
    let run = catalog.reindex(&Scope::All).unwrap().unwrap();
    let snapshot = catalog.snapshot().unwrap();
    same(
        "reindex",
        cli_doc,
        CatalogDocument::reindexed(&snapshot, &run)
            .unwrap()
            .to_json(),
    );
    drop(snapshot);
    let cli_doc = catalog_cli(&dirs.root, &["prune", "--revisions"]);
    let (pruned, _) = catalog.prune_revisions().unwrap();
    assert_eq!(pruned.len(), 1);
    let snapshot = catalog.snapshot().unwrap();
    same(
        "prune",
        cli_doc,
        CatalogDocument::pruned(&snapshot, &pruned)
            .unwrap()
            .to_json(),
    );
    drop(snapshot);
    let named = ["junk", "M_BRACKET@r2", "junk"];
    let cli_doc = catalog_cli(&dirs.root, &[&["delete"], &named[..]].concat());
    let references: Vec<Reference> = named.iter().map(|n| Reference::parse(n).unwrap()).collect();
    let deletion = catalog.delete(&references).unwrap().unwrap();
    let snapshot = catalog.snapshot().unwrap();
    same(
        "delete",
        cli_doc,
        CatalogDocument::deleted(&snapshot, &named, &deletion)
            .unwrap()
            .to_json(),
    );
    drop(snapshot);
    let cli_doc = catalog_cli(&dirs.root, &["verify", "--deep"]);
    let verified = catalog.verify_observed(true, false, &NoObserver).unwrap();
    same(
        "verify",
        cli_doc,
        VerifyDocument::new(&verified, true, false).to_json(),
    );
    let cli_doc = catalog_cli(&dirs.root, &["gc", "--versions"]);
    let collected = catalog.gc(true, false).unwrap();
    same(
        "gc",
        cli_doc,
        GcDocument::new(&collected, true, false).to_json(),
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
