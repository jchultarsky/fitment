//! Third-party STEP files, run through the built `fitment` as a user would.
//!
//! `tools/fetch-fixtures.sh core` downloads them into `fixtures/`; they are
//! never committed. `fixtures/manifest.json` pins what fitment's gate makes
//! of each one. Each file must give exactly its pinned result and exit
//! code, and never a panic or an `internal` refusal; an accepted one must
//! have its pinned numbers of planes, of unknown plane outlines, of full
//! cylinders, and of holes and shafts by kind, and
//! every face in exactly one primary
//! feature (design, section 6). Every STEP file
//! fetched outside the manual `ocp` set must be pinned. Without the files
//! the test passes with a note, unless `FITMENT_REQUIRE_FIXTURES=1`, which
//! CI sets.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use assert_cmd::Command;
use serde_json::Value;

mod common;

#[test]
fn every_fixture_gives_its_pinned_gate_result() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
    let manifest: Value =
        serde_json::from_str(&fs::read_to_string(root.join("manifest.json")).unwrap()).unwrap();
    let fixtures = manifest["fixtures"].as_array().unwrap();
    let required = std::env::var_os("FITMENT_REQUIRE_FIXTURES").is_some_and(|v| v == "1");
    let missing: Vec<&str> = fixtures
        .iter()
        .map(|f| f["path"].as_str().unwrap())
        .filter(|path| !root.join(path).is_file())
        .collect();
    if !missing.is_empty() {
        assert!(
            !required,
            "FITMENT_REQUIRE_FIXTURES=1 but {} fixtures are missing, such as {}; run tools/fetch-fixtures.sh core",
            missing.len(),
            missing[0]
        );
        eprintln!("skipped: run tools/fetch-fixtures.sh core");
        return;
    }
    let mut failures = Vec::new();
    for fixture in fixtures {
        let path = fixture["path"].as_str().unwrap();
        let output = Command::cargo_bin("fitment")
            .unwrap()
            .arg("features")
            .arg(root.join(path))
            .output()
            .unwrap();
        let Ok(document) = serde_json::from_slice::<Value>(&output.stdout) else {
            failures.push(format!(
                "{path}: no document; exit {:?}",
                output.status.code()
            ));
            continue;
        };
        let mut reasons: Vec<&str> = document["refusals"]
            .as_array()
            .unwrap()
            .iter()
            .map(|refusal| refusal["code"].as_str().unwrap())
            .collect();
        reasons.dedup();
        let code = if fixture["features"] == "accepted" {
            0
        } else {
            3
        };
        if output.status.code() != Some(code) {
            failures.push(format!(
                "{path}: exit {:?}, expected {code}",
                output.status.code()
            ));
        }
        let pinned: Vec<&str> = fixture["reasons"]
            .as_array()
            .map(|r| r.iter().map(|c| c.as_str().unwrap()).collect())
            .unwrap_or_default();
        if let Some(problem) = common::reads_back(&output.stdout) {
            failures.push(format!("{path}: {problem}"));
        }
        if document["status"] == "accepted" {
            failures.extend(accepted(&document["part"], fixture).map(|f| format!("{path}: {f}")));
        }
        if document["status"] != fixture["features"]
            || reasons != pinned
            || reasons.contains(&"internal")
        {
            failures.push(format!(
                "{path}: {} {reasons:?}, pinned {} {pinned:?}",
                document["status"], fixture["features"]
            ));
        }
    }
    let pinned: BTreeSet<String> = fixtures
        .iter()
        .map(|f| f["path"].as_str().unwrap().to_owned())
        .collect();
    for path in step_files(&root, &root) {
        if !path.starts_with("ocp/") && !pinned.contains(&path) {
            failures.push(format!("{path}: fetched but not pinned"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// What is wrong with an accepted fixture's part: its numbers of planes
/// and of unknown plane outlines against the pins, and the face partition.
fn accepted(part: &Value, fixture: &Value) -> impl Iterator<Item = String> {
    let mut problems = Vec::new();
    let planes = part["planes"].as_array().map(Vec::len);
    if planes.map(|n| n as u64) != fixture["planes"].as_u64() {
        problems.push(format!("{planes:?} planes, pinned {}", fixture["planes"]));
    }
    let unknown = part["planes"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|plane| plane["faces"].as_array().into_iter().flatten())
        .filter(|face| face["outline"]["status"] == "unknown")
        .count();
    if Some(unknown as u64) != fixture["unknown_outlines"].as_u64() {
        problems.push(format!(
            "{unknown} unknown outlines, pinned {}",
            fixture["unknown_outlines"]
        ));
    }
    let full = part["cylinders"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|c| c["class"]["kind"] == "full" || c["class"]["kind"] == "full_one_end_irregular")
        .count();
    if Some(full as u64) != fixture["full_cylinders"].as_u64() {
        problems.push(format!(
            "{full} full cylinders, pinned {}",
            fixture["full_cylinders"]
        ));
    }
    for key in ["holes", "shafts"] {
        let mut kinds = serde_json::Map::new();
        for stack in part[key].as_array().into_iter().flatten() {
            let kind = stack["kind"].as_str().unwrap_or("").to_owned();
            let n = kinds.get(&kind).and_then(Value::as_u64).unwrap_or(0);
            kinds.insert(kind, (n + 1).into());
        }
        if Value::Object(kinds.clone()) != fixture[key] {
            problems.push(format!("{key} by kind {kinds:?}, pinned {}", fixture[key]));
        }
    }
    problems.extend(common::expect::partition(part));
    problems.into_iter()
}

/// Every `.stp` and `.step` file under `dir`, as paths relative to `root`
/// with `/` separators.
fn step_files(root: &Path, dir: &Path) -> Vec<String> {
    let mut found = Vec::new();
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            found.extend(step_files(root, &path));
        } else if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("stp") || e.eq_ignore_ascii_case("step"))
        {
            let relative = path.strip_prefix(root).unwrap();
            let parts: Vec<_> = relative
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect();
            found.push(parts.join("/"));
        }
    }
    found
}

/// Every assembly fixture with a `parts` pin: its leaves, their
/// occurrences, how many are refused (check 36), the assembly gate's
/// codes and the notes (`docs/research/m3-socket.md`, 14.3). Regression
/// pins, not truth; and every parts document reads back.
#[test]
fn every_assembly_fixture_gives_its_pinned_parts_list() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
    let manifest: Value =
        serde_json::from_str(&fs::read_to_string(root.join("manifest.json")).unwrap()).unwrap();
    let pinned: Vec<&Value> = manifest["fixtures"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f.get("parts").is_some())
        .collect();
    assert!(!pinned.is_empty(), "no assembly fixture is pinned");
    if pinned
        .iter()
        .any(|f| !root.join(f["path"].as_str().unwrap()).is_file())
    {
        let required = std::env::var_os("FITMENT_REQUIRE_FIXTURES").is_some_and(|v| v == "1");
        assert!(
            !required,
            "FITMENT_REQUIRE_FIXTURES=1 but fixtures are missing"
        );
        eprintln!("skipped: run tools/fetch-fixtures.sh core");
        return;
    }
    let mut failures = Vec::new();
    for fixture in pinned {
        let path = fixture["path"].as_str().unwrap();
        let output = Command::cargo_bin("fitment")
            .unwrap()
            .args(["parts", "--format", "json"])
            .arg(root.join(path))
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&output.stdout);
        if let Err(error) = fitment_core::PartsDocument::from_json(&text) {
            failures.push(format!(
                "{path}: the parts document does not read back: {error}"
            ));
            continue;
        }
        let document: Value = serde_json::from_str(&text).unwrap();
        let leaves: Vec<&Value> = document["parts"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|p| p["kind"] == "part")
            .collect();
        let mut gate: Vec<&str> = document["assembly_gate"]["refusals"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["code"].as_str().unwrap())
            .collect();
        gate.dedup();
        let notes: BTreeSet<&str> = document["notes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| n["code"].as_str().unwrap())
            .collect();
        let found = serde_json::json!({
            "leaves": leaves.len(),
            "leaf_occurrences": leaves.iter().map(|p| p["quantity"].as_u64().unwrap()).sum::<u64>(),
            "refused_leaves": leaves.iter().filter(|p| p["component"]["gate"]["status"] == "refused").count(),
            "assembly_gate": gate,
            "notes": notes,
        });
        if found != fixture["parts"] {
            failures.push(format!("{path}: {found}, pinned {}", fixture["parts"]));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// One target's socket, summarised for a regression pin (design, 14.3):
/// status, occurrences, sockets after merging, requirements by kind, spans
/// by fit, features left out and unverifiable requirements.
fn socket_summary(document: &fitment_core::SocketDocument) -> Value {
    let Some(extracted) = document.extracted() else {
        return serde_json::json!({ "status": "refused" });
    };
    let mut kinds = std::collections::BTreeMap::<String, u64>::new();
    let mut fits = std::collections::BTreeMap::<String, u64>::new();
    let mut left_out = 0;
    let mut unverifiable = 0;
    for socket in &extracted.sockets {
        left_out += socket.left_out.len();
        for requirement in &socket.requirements {
            let value = serde_json::to_value(requirement).unwrap();
            let kind = value["requires"]["kind"].as_str().unwrap().to_owned();
            *kinds.entry(kind).or_default() += 1;
            for span in value["requires"]["spans"].as_array().into_iter().flatten() {
                *fits
                    .entry(span["fit"].as_str().unwrap().to_owned())
                    .or_default() += 1;
            }
            if !requirement.verifiable() {
                unverifiable += 1;
            }
        }
    }
    serde_json::json!({
        "status": "accepted",
        "occurrences": extracted.occurrences.len(),
        "sockets": extracted.sockets.len(),
        "requirements": kinds,
        "fits": fits,
        "left_out": left_out,
        "unverifiable": unverifiable,
    })
}

/// Every pinned socket target of every assembly fixture gives its pinned
/// summary, and its document reads back (design, 14.3). Regression pins,
/// not truth: `tools/pin-sockets.py` re-pins them, in a PR that says why.
#[test]
fn every_pinned_socket_target_gives_its_pinned_summary() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
    let manifest: Value =
        serde_json::from_str(&fs::read_to_string(root.join("manifest.json")).unwrap()).unwrap();
    let pinned: Vec<&Value> = manifest["fixtures"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f.get("sockets").is_some())
        .collect();
    assert!(!pinned.is_empty(), "no socket is pinned");
    if pinned
        .iter()
        .any(|f| !root.join(f["path"].as_str().unwrap()).is_file())
    {
        let required = std::env::var_os("FITMENT_REQUIRE_FIXTURES").is_some_and(|v| v == "1");
        assert!(
            !required,
            "FITMENT_REQUIRE_FIXTURES=1 but fixtures are missing"
        );
        eprintln!("skipped: run tools/fetch-fixtures.sh core");
        return;
    }
    let mut failures = Vec::new();
    for fixture in pinned {
        let path = fixture["path"].as_str().unwrap();
        let bytes = fs::read(root.join(path)).unwrap();
        for (target, pin) in fixture["sockets"].as_object().unwrap() {
            let document = match fitment_core::SocketDocument::extract(&bytes, Some(target), None) {
                Ok(document) => document,
                Err(error) => {
                    failures.push(format!("{path} {target}: {error}"));
                    continue;
                }
            };
            if let Err(error) = fitment_core::SocketDocument::from_json(&document.to_json()) {
                failures.push(format!("{path} {target}: does not read back: {error}"));
            }
            let found = socket_summary(&document);
            // The fit's control is fit_original.rs's pin (VT59), not the
            // socket's.
            let mut pin = pin.clone();
            if let Some(pin) = pin.as_object_mut() {
                pin.remove("fit_control");
            }
            if found != pin {
                failures.push(format!("{path} {target}: {found}, pinned {pin}"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Spans and grips as (kind, fit, diameter, length), and axis distances.
type Core = (BTreeSet<(String, String, i64, i64)>, Vec<i64>);

/// What one export's target requires, scaled to millimetres of AS1: each
/// span as (kind, fit, diameter, length) and each grip's length, in
/// hundredths, as a set; and the distances between its bores' axes.
fn as1_core(bytes: &[u8], part: &str, scale: f64) -> Core {
    #[allow(clippy::cast_possible_truncation)]
    let hundredths = |x: f64| (x / scale * 100.0).round() as i64;
    let document = fitment_core::SocketDocument::extract(bytes, Some(part), None).unwrap();
    let extracted = document.extracted().unwrap();
    let mut core = BTreeSet::new();
    let mut axes = Vec::new();
    for socket in &extracted.sockets {
        for requirement in &socket.requirements {
            let value = serde_json::to_value(&requirement.requires).unwrap();
            let kind = value["kind"].as_str().unwrap().to_owned();
            for span in value["spans"].as_array().into_iter().flatten() {
                let diameter = span["hole_diameter_mm"]
                    .as_f64()
                    .or_else(|| span["shaft_diameter_mm"].as_f64())
                    .unwrap();
                let length = span["to_mm"].as_f64().unwrap() - span["from_mm"].as_f64().unwrap();
                core.insert((
                    kind.clone(),
                    span["fit"].as_str().unwrap().to_owned(),
                    hundredths(diameter),
                    hundredths(length),
                ));
            }
            if kind == "grip" {
                let length = value["length_mm"].as_f64().unwrap().abs();
                core.insert((kind.clone(), String::new(), 0, hundredths(length)));
            }
            if kind == "bore" && socket.id == "socket-1" {
                let point: Vec<f64> =
                    serde_json::from_value(value["axis"]["point_mm"].clone()).unwrap();
                let direction: Vec<f64> =
                    serde_json::from_value(value["axis"]["direction"].clone()).unwrap();
                axes.push((point, direction));
            }
        }
    }
    let mut distances = Vec::new();
    for (i, (p, d)) in axes.iter().enumerate() {
        for (q, _) in axes.iter().skip(i + 1) {
            let v: Vec<f64> = (0..3).map(|k| q[k] - p[k]).collect();
            let along: f64 = (0..3).map(|k| v[k] * d[k]).sum();
            let across: f64 = (0..3)
                .map(|k| (v[k] - along * d[k]).powi(2))
                .sum::<f64>()
                .sqrt();
            distances.push(hundredths(across));
        }
    }
    distances.sort_unstable();
    (core, distances)
}

/// Design 14.2: AS1's ac, md, ug and pe exports (pe ÷ 25.4) share a core:
/// what each part requires, and the bolt holes' equilateral triangle of
/// side 15√3 = 25.98 mm. The exports are not one geometry; their known
/// differences are named.
#[test]
fn as1_exports_share_their_core() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
    let exports = [
        (
            "ac",
            "steptools/as1-ac-214.stp",
            ["PLATE", "L-BRACKET", "BOLT", "NUT", "ROD"],
            1.0,
        ),
        (
            "md",
            "steptools/as1-md-214.stp",
            ["PLATE", "LBRACKET", "BOLT", "NUT", "ROD"],
            1.0,
        ),
        (
            "ug",
            "steptools/as1-ug-214.stp",
            ["plate", "l_bracket", "bolt", "nut", "rod"],
            1.0,
        ),
        (
            "pe",
            "nist-edm/as1_pe.stp",
            ["PLATE", "L-BRACKET", "BOLT", "NUT", "ROD"],
            25.4,
        ),
    ];
    if exports
        .iter()
        .any(|(_, path, _, _)| !root.join(path).is_file())
    {
        let required = std::env::var_os("FITMENT_REQUIRE_FIXTURES").is_some_and(|v| v == "1");
        assert!(
            !required,
            "FITMENT_REQUIRE_FIXTURES=1 but fixtures are missing"
        );
        eprintln!("skipped: run tools/fetch-fixtures.sh core");
        return;
    }
    let span = |kind: &str, fit: &str, diameter: i64, length: i64| {
        (kind.to_owned(), fit.to_owned(), diameter, length)
    };
    let grip = |length: i64| ("grip".to_owned(), String::new(), 0, length);
    let ltl = "line_to_line";
    // What every export requires, part by part (ac's). A shaft's span
    // reaches τ past each end of a neighbour inside it (I3, the socket
    // PR's SR7): the bolt's past its nut, the rod's past both of its.
    let shared: [BTreeSet<_>; 5] = [
        [span("bore", ltl, 1000, 2000), grip(2000)].into(),
        [span("bore", ltl, 1000, 1000), grip(1000)].into(),
        [span("shaft", ltl, 1000, 3301)].into(),
        [span("bore", ltl, 1000, 300)].into(),
        [span("shaft", ltl, 1000, 1302)].into(),
    ];
    // The known differences.
    let differs = |export: &str, part: usize| -> Option<BTreeSet<_>> {
        match (export, part) {
            // No nuts on pe's bolts: no grip on the plate, and the bolts
            // engage 30 mm, not 33.
            ("pe", 0) => Some([span("bore", ltl, 1000, 2000)].into()),
            // pe's τ is its floor, 0.672 mm: 0.026 of its inch.
            ("pe", 2) => Some([span("shaft", ltl, 1000, 3003)].into()),
            ("pe", 4) => Some([span("shaft", ltl, 1000, 1305)].into()),
            // md's and ug's rod nuts stand 12 mm off the uprights: the rod
            // is held in two spans, not one.
            ("md" | "ug", 4) => Some(
                [
                    span("shaft", ltl, 1000, 301),
                    span("shaft", ltl, 1000, 1002),
                ]
                .into(),
            ),
            _ => None,
        }
    };
    let mut failures = Vec::new();
    for (export, path, parts, scale) in exports {
        let bytes = fs::read(root.join(path)).unwrap();
        for (k, part) in parts.iter().enumerate() {
            let (core, distances) = as1_core(&bytes, part, scale);
            let wanted = differs(export, k).unwrap_or_else(|| shared[k].clone());
            if core != wanted {
                failures.push(format!("{export} {part}: {core:?}, expected {wanted:?}"));
            }
            // The bolt holes' triangle: three sides of 25.98 on each
            // bracket, six on the plate (the rod hole lies elsewhere in
            // each export).
            let sides = distances.iter().filter(|d| **d == 2598).count();
            let triangle = match k {
                0 => 6,
                1 => 3,
                _ => 0,
            };
            if sides != triangle {
                failures.push(format!(
                    "{export} {part}: {sides} sides of 25.98, expected {triangle}"
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// M5's CT5 on the fixtures: each one's features document reads back from
/// its compact form equal and writing the same JSON, its tokens read back
/// as written (IX9), and judged from it against a committed socket it gets
/// the result its bytes get.
#[test]
fn every_fixture_is_judged_from_its_stored_document_as_from_its_file() {
    use fitment_core::{
        FeaturesDocument, IndexTokens, MatchOptions, MatchPolicy, Matcher, NoFitVerifier,
        PartSignature, SocketDocument, Source, sha256_hex,
    };
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
    let manifest: Value =
        serde_json::from_str(&fs::read_to_string(root.join("manifest.json")).unwrap()).unwrap();
    let present: Vec<&str> = manifest["fixtures"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["path"].as_str().unwrap())
        .filter(|path| root.join(path).is_file())
        .collect();
    if present.is_empty() {
        let required = std::env::var_os("FITMENT_REQUIRE_FIXTURES").is_some_and(|v| v == "1");
        assert!(
            !required,
            "FITMENT_REQUIRE_FIXTURES=1 but fixtures are missing"
        );
        eprintln!("skipped: run tools/fetch-fixtures.sh core");
        return;
    }
    let assembly = include_bytes!("../../fitment-core/tests/assemblies/s01_bolted.stp");
    let socket = SocketDocument::extract(assembly, Some("BRACKET"), None).unwrap();
    let matcher = Matcher::new(
        &socket,
        MatchPolicy::default(),
        MatchOptions::default(),
        &NoFitVerifier,
    )
    .unwrap();
    for path in present {
        let bytes = fs::read(root.join(path)).unwrap();
        let read = FeaturesDocument::read(&bytes);
        let back = FeaturesDocument::from_json(&read.to_compact_json()).unwrap();
        assert_eq!(back, read, "{path}");
        assert_eq!(back.to_json(), read.to_json(), "{path}");
        if let Some(part) = back.part() {
            let tokens = IndexTokens::of(&PartSignature::of(part));
            let again = IndexTokens::from_json(&tokens.to_json());
            assert_eq!(again.as_ref(), Ok(&tokens), "{path}");
        }
        let source = Source::new(
            &sha256_hex(&bytes),
            u64::try_from(bytes.len()).unwrap(),
            back.header().cloned(),
        )
        .unwrap();
        let stored = matcher.judge_document(&back, source).unwrap();
        assert_eq!(stored, matcher.judge_step(&bytes), "{path}");
    }
}

/// M5 PR 2's exit (`docs/research/m5-catalog.md`, section 10): every
/// fixture ingests into a catalog with its pinned gate result, and `get
/// --format features` gives `fitment features` of the file, byte for byte.
#[cfg(feature = "catalog")]
#[test]
fn every_fixture_ingests_with_its_pinned_gate_result() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
    let manifest: Value =
        serde_json::from_str(&fs::read_to_string(root.join("manifest.json")).unwrap()).unwrap();
    let fixtures: Vec<&Value> = manifest["fixtures"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| root.join(f["path"].as_str().unwrap()).is_file())
        .collect();
    if fixtures.is_empty() {
        let required = std::env::var_os("FITMENT_REQUIRE_FIXTURES").is_some_and(|v| v == "1");
        assert!(
            !required,
            "FITMENT_REQUIRE_FIXTURES=1 but fixtures are missing"
        );
        eprintln!("skipped: run tools/fetch-fixtures.sh core");
        return;
    }
    let dir = std::env::temp_dir().join(format!("fitment-fixtures-catalog-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let catalog = dir.join("cat");
    let fitment = || Command::cargo_bin("fitment").unwrap();
    fitment()
        .args(["catalog", "init", "-C", catalog.to_str().unwrap()])
        .assert()
        .success();
    let mut failures = Vec::new();
    for fixture in fixtures {
        let path = root.join(fixture["path"].as_str().unwrap());
        let sha = fitment_core::sha256_hex(&fs::read(&path).unwrap());
        // Ids from file names may clash across sets; the content names it.
        let id = format!("f{}", &sha[..16]);
        let ingest = fitment()
            .args([
                "catalog",
                "ingest",
                "-C",
                catalog.to_str().unwrap(),
                "--id",
                &id,
            ])
            .arg(&path)
            .output()
            .unwrap();
        let accepted = fixture["features"] == "accepted";
        let want = if accepted { 0 } else { 3 };
        if ingest.status.code() != Some(want) {
            failures.push(format!(
                "{}: ingest exit {:?}",
                path.display(),
                ingest.status.code()
            ));
            continue;
        }
        let stored = fitment()
            .args([
                "catalog",
                "get",
                "-C",
                catalog.to_str().unwrap(),
                &id,
                "--format",
                "features",
            ])
            .output()
            .unwrap();
        let direct = fitment().arg("features").arg(&path).output().unwrap();
        if stored.stdout != direct.stdout || stored.status.code() != Some(want) {
            failures.push(format!("{}: get --format features differs", path.display()));
        }
    }
    let _ = fs::remove_dir_all(&dir);
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
