//! VT36 (docs/research/m6-api.md, VN7, CO4, CA6; M6-6): the OpenAPI
//! document generated from `OPERATIONS` and the schemars derives equals
//! the committed `openapi.json`, which the server serves; and every
//! document the goldens, the fixtures' unit set and the corpus hold
//! validates against the schema the OpenAPI document gives its format.
//! Behind the `schema` feature (`required-features`); CI runs it
//! explicitly.
#![allow(clippy::disallowed_methods)] // tests read FITMENT_BLESS and CI

use std::fs;
use std::path::{Path, PathBuf};

use fitment_corpus::schema;
use serde_json::Value;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn committed_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("openapi.json")
}

/// `info.version`'s minor, for VN7's bless rule.
fn minor(document: &str) -> Option<u32> {
    let value: Value = serde_json::from_str(document).ok()?;
    let version = value["info"]["version"].as_str()?;
    version.split_once('.')?.1.parse().ok()
}

/// VN7: the generated document is the committed one. `FITMENT_BLESS=1`
/// rewrites it, never under `CI`, and only when the API's minor rose (or
/// there is none yet), so every API change is a reviewed diff.
#[test]
fn the_openapi_document_is_the_committed_one() {
    let generated = fitment_server::openapi::document();
    let path = committed_path();
    let committed = fs::read_to_string(&path).unwrap_or_default();
    if generated == committed {
        assert_eq!(
            fitment_server::OPENAPI,
            committed.as_bytes(),
            "rebuild: the served bytes are stale"
        );
        return;
    }
    let bless = std::env::var_os("FITMENT_BLESS").is_some_and(|v| v == "1");
    let ci = std::env::var_os("CI").is_some();
    let rose = match (minor(&committed), minor(&generated)) {
        (None, _) => true,
        (Some(old), Some(new)) => new > old,
        (Some(_), None) => false,
    };
    if bless && !ci && rose {
        fs::write(&path, &generated).unwrap();
        eprintln!("wrote {}: rebuild so the server serves it", path.display());
        return;
    }
    let at = generated
        .bytes()
        .zip(committed.bytes())
        .position(|(a, b)| a != b)
        .unwrap_or(generated.len().min(committed.len()));
    let from = at.saturating_sub(200);
    panic!(
        "openapi.json differs from the generated document at byte {at} (rise the API's minor in \
         info.rs and bless with FITMENT_BLESS=1):\n--- committed\n{}\n--- generated\n{}",
        committed
            .get(from..(at + 200).min(committed.len()))
            .unwrap_or(""),
        generated
            .get(from..(at + 200).min(generated.len()))
            .unwrap_or(""),
    );
}

/// The schema of each format, from the generated document.
fn schemas() -> Value {
    fitment_server::openapi::value()["components"]["schemas"].clone()
}

/// Every JSON file under `dir`, recursively.
fn json_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            json_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "json") {
            out.push(path);
        }
    }
}

/// The bodies of the API's goldens (`tests/golden/api/**/*.response`):
/// each after the blank line that ends its headers (VT6, review S8).
fn response_goldens() -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut stack = vec![Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/api")];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "response") {
                let text = fs::read_to_string(&path).unwrap();
                if let Some((_, body)) = text.split_once("\n\n") {
                    out.push((path.display().to_string(), body.to_owned()));
                }
            }
        }
    }
    out
}

/// The documents fitment-core writes for the committed unit set,
/// assemblies and candidates (VT36's "fixture" documents).
fn library_documents() -> Vec<(String, String)> {
    let tests = root().join("crates/fitment-core/tests");
    let mut out = Vec::new();
    for set in ["data", "candidates"] {
        let mut files: Vec<PathBuf> = fs::read_dir(tests.join(set))
            .unwrap()
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|e| e == "stp"))
            .collect();
        files.sort();
        for file in files {
            let bytes = fs::read(&file).unwrap();
            let name = file.file_name().unwrap().to_string_lossy().into_owned();
            out.push((
                format!("features of {set}/{name}"),
                fitment_core::FeaturesDocument::read(&bytes).to_json(),
            ));
        }
    }
    let mut assemblies: Vec<PathBuf> = fs::read_dir(tests.join("assemblies"))
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "stp"))
        .collect();
    assemblies.sort();
    for file in assemblies {
        let bytes = fs::read(&file).unwrap();
        out.push((
            format!("parts of {}", file.display()),
            fitment_core::PartsDocument::read(&bytes).to_json(),
        ));
    }
    out.extend(catalog_documents());
    out
}

/// `fitment.catalog` documents as the CLI's `ingest`, `list` and `get`
/// build them, over the committed candidates and a refused file (CA6).
fn catalog_documents() -> Vec<(String, String)> {
    use fitment_catalog::document::{CatalogDocument, Change, Entry};
    let dir = std::env::temp_dir().join(format!("fitment-test-openapi-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let tests = root().join("crates/fitment-core/tests");
    let (mut catalog, _) =
        fitment_catalog::Catalog::init(&dir, fitment_catalog::Options::default()).unwrap();
    let ingest = catalog
        .ingest(
            &[
                tests.join("candidates"),
                tests.join("data/r04_assembly.stp"),
            ],
            None,
        )
        .unwrap();
    let snapshot = catalog.snapshot().unwrap();
    let all = snapshot.list().unwrap();
    let entries: Vec<Entry> = all.iter().map(|l| Entry::of(l, None)).collect();
    let changes = ingest.lines.iter().map(Change::of).collect();
    let mut out = vec![
        (
            "catalog ingest".to_owned(),
            CatalogDocument::new(snapshot.generation(), &all, entries.clone(), Some(changes))
                .to_json(),
        ),
        (
            "catalog list".to_owned(),
            CatalogDocument::new(snapshot.generation(), &all, entries, None).to_json(),
        ),
    ];
    let first = &all[0];
    let named = snapshot
        .resolve(&fitment_catalog::Reference::parse(&first.id).unwrap())
        .unwrap()
        .unwrap();
    let history = snapshot.history(&named).unwrap();
    out.push((
        "catalog get".to_owned(),
        CatalogDocument::new(
            snapshot.generation(),
            &all,
            vec![Entry::of(first, Some(&history))],
            None,
        )
        .to_json(),
    ));
    drop(snapshot);
    drop(catalog);
    let _ = fs::remove_dir_all(&dir);
    out
}

/// VT36: every golden, fixture and generated document of a result format
/// validates against its format's schema, and so does each document the
/// library writes for the committed STEP files.
#[test]
fn every_document_validates_against_its_schema() {
    let schemas = schemas();
    let mut files = Vec::new();
    json_files(&root().join("crates/fitment/tests/golden"), &mut files);
    json_files(&root().join("crates/fitment-core/tests"), &mut files);
    json_files(
        &root().join("crates/fitment-server/tests/golden"),
        &mut files,
    );
    let mut documents: Vec<(String, String)> = files
        .iter()
        .map(|p| (p.display().to_string(), fs::read_to_string(p).unwrap()))
        .collect();
    documents.extend(response_goldens());
    documents.extend(library_documents());
    let mut checked = std::collections::BTreeMap::<String, usize>::new();
    let mut failures = Vec::new();
    for (name, text) in &documents {
        let Ok(document) = serde_json::from_str::<Value>(text) else {
            continue;
        };
        let Some(format) = document["format"].as_str() else {
            continue;
        };
        // FM7: a later version by its own schema, the first by the format's.
        let format = match document["format_version"].as_u64() {
            Some(v) if v > 1 => {
                fitment_server::openapi::versioned(format, u32::try_from(v).unwrap_or(0))
            }
            _ => format.to_owned(),
        };
        let Some(schema) = schemas.get(&format) else {
            continue;
        };
        *checked.entry(format.clone()).or_default() += 1;
        for problem in schema::validate(schema, &document) {
            failures.push(format!("{name}: {problem}"));
        }
    }
    eprintln!("validated: {checked:?}");
    for format in [
        "fitment.info",
        "fitment.error",
        "fitment.features",
        "fitment.parts",
        "fitment.socket",
        "fitment.match",
        "fitment.match.v2",
        "fitment.catalog",
        "fitment.upload",
        "fitment.page",
        "fitment.job",
        "fitment.assembly",
    ] {
        assert!(
            checked.get(format).copied().unwrap_or(0) > 0,
            "no {format} document was validated"
        );
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// VT36's control: a document edited to add an unknown field, drop a
/// required one or change a type fails its schema; the original passes.
#[test]
fn an_edited_document_fails_its_schema() {
    let schemas = schemas();
    let bytes = fs::read(root().join("crates/fitment-core/tests/data/p01_through.stp")).unwrap();
    let document: Value =
        serde_json::from_str(&fitment_core::FeaturesDocument::read(&bytes).to_json()).unwrap();
    let schema = &schemas["fitment.features"];
    assert_eq!(schema::validate(schema, &document), Vec::<String>::new());
    let mut added = document.clone();
    added["part"]["unexpected"] = Value::Bool(true);
    assert!(
        !schema::validate(schema, &added).is_empty(),
        "an unknown field passed"
    );
    let mut dropped = document.clone();
    assert!(dropped.as_object_mut().unwrap().remove("header").is_some());
    assert!(
        !schema::validate(schema, &dropped).is_empty(),
        "a missing field passed"
    );
    let mut retyped = document;
    retyped["format_version"] = Value::String("1".into());
    assert!(
        !schema::validate(schema, &retyped).is_empty(),
        "a wrong type passed"
    );
}

/// FM7 (docs/research/m7-fit.md): `fitment.match` v2 is its own closed
/// schema beside v1's: a v2 golden validates against v2's and not v1's, a
/// v1 golden not against v2's, and a field v2 does not write fails it;
/// each pins its format and version by `const`.
#[test]
fn v2_is_its_own_closed_schema() {
    let schemas = schemas();
    let (v1, v2) = (&schemas["fitment.match"], &schemas["fitment.match.v2"]);
    assert_eq!(v2["$id"], "https://fitment.dev/schemas/fitment.match/2");
    assert_eq!(v1["$id"], "https://fitment.dev/schemas/fitment.match/1");
    let golden = root().join("crates/fitment/tests/golden");
    let read = |path: &str| -> Value {
        serde_json::from_str(&fs::read_to_string(golden.join(path)).unwrap()).unwrap()
    };
    let fit = read("match-v2/s01_bolted--BRACKET.json");
    let plain = read("match/s01_bolted--BRACKET.json");
    assert_eq!(schema::validate(v2, &fit), Vec::<String>::new());
    assert_eq!(schema::validate(v1, &plain), Vec::<String>::new());
    assert!(
        !schema::validate(v1, &fit).is_empty(),
        "v2 passed v1's schema"
    );
    assert!(
        !schema::validate(v2, &plain).is_empty(),
        "v1 passed v2's schema"
    );
    let mut added = fit.clone();
    added["fit"]["unexpected"] = Value::Bool(true);
    assert!(
        !schema::validate(v2, &added).is_empty(),
        "an unknown field passed"
    );
    // The review of PR 1b: each pins its format and version by `const`.
    for (schema, version) in [(v1, 1), (v2, 2)] {
        assert_eq!(schema["properties"]["format"]["const"], "fitment.match");
        assert_eq!(schema["properties"]["format_version"]["const"], version);
    }
    let mut relabelled = fit;
    relabelled["format_version"] = 3.into();
    assert!(
        !schema::validate(v2, &relabelled).is_empty(),
        "another version passed"
    );
}

/// VT36 on the corpus, in a release build: each part file's `features.v1`
/// and `fitment.parts` validate against their schemas.
#[test]
fn every_corpus_document_validates_against_its_schema() {
    let Some(corpus) = fitment_corpus::corpus::Corpus::locate_release_only() else {
        return;
    };
    let schemas = schemas();
    let mut failures = Vec::new();
    let files = corpus.part_files();
    for file in &files {
        let bytes = fs::read(file).unwrap();
        for (format, text) in [
            (
                "fitment.features",
                fitment_core::FeaturesDocument::read(&bytes).to_json(),
            ),
            (
                "fitment.parts",
                fitment_core::PartsDocument::read(&bytes).to_json(),
            ),
        ] {
            let document: Value = serde_json::from_str(&text).unwrap();
            for problem in schema::validate(&schemas[format], &document) {
                failures.push(format!("{}: {format}: {problem}", file.display()));
            }
        }
    }
    assert_ne!(files.len(), 0);
    eprintln!("validated the documents of {} corpus files", files.len());
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// R1 of PR 2's review: closing a schema never treats a `properties` or
/// `$defs` map as a schema, so no map gains a property named after a
/// keyword (no document field is named `additionalProperties` or
/// `unevaluatedProperties`).
#[test]
fn no_properties_map_names_a_keyword() {
    fn walk(value: &Value, at: &str, out: &mut Vec<String>) {
        match value {
            Value::Object(map) => {
                for (key, sub) in map {
                    if key == "properties" || key == "$defs" {
                        if let Some(entries) = sub.as_object() {
                            for keyword in ["additionalProperties", "unevaluatedProperties"] {
                                if entries.contains_key(keyword) {
                                    out.push(format!("{at}/{key} names {keyword}"));
                                }
                            }
                        }
                    }
                    walk(sub, &format!("{at}/{key}"), out);
                }
            }
            Value::Array(items) => items.iter().for_each(|v| walk(v, at, out)),
            _ => {}
        }
    }
    let mut found = Vec::new();
    walk(&schemas(), "", &mut found);
    assert_eq!(found, Vec::<String>::new());
    let committed: Value =
        serde_json::from_str(&fs::read_to_string(committed_path()).unwrap()).unwrap();
    walk(&committed, "", &mut found);
    assert_eq!(found, Vec::<String>::new());
}
