//! VT59 (`docs/research/m7-fit.md`, FT9, FK4, FK5): every corpus and
//! fixture original read in place, judged as `--original` judges it, its
//! control run through the real helper wherever its interface passes. The
//! control is `verified` there, but where the construction says the
//! assembly already interferes (FK4's marks in the corpus manifest: s20,
//! by construction, the exit's one exception) and where the fixtures'
//! manifest pins another outcome (section 5.3: AS1-md's
//! `uncertainty_above_tolerance`); a mark of an original whose interface
//! already rejects (s14, the weldment) is checked as rejecting. Every
//! original of the corpus is judged in every frame and writer its case is
//! written in, each variant held to its base's marks.
//!
//! The real helper runs as `fit_common` says; on Windows it is refused
//! until VT77 is green (M7-50), and these say so and stop.

#![cfg(feature = "fit")]

mod fit_common;

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::sync::Mutex;
use std::time::Instant;

use fit_common::{inputs, kernel, proved, refused_here};
use fitment_core::{
    FitStatus, MatchOptions, MatchPolicy, Matcher, SelectedComponent, SocketDocument, VerdictKind,
};
use fitment_fit::{HelperFiles, HelperOptions, OcctHelper, OcctSession};
use serde_json::{Value, json};

/// One original: its assembly, its target, and what its control must
/// come to, if its source says.
struct Original {
    id: String,
    bytes: Vec<u8>,
    part: String,
    expect: Option<Value>,
}

/// What one original's run came to: whether its interface passed, and
/// its control's status and code.
#[derive(Debug, Clone, PartialEq)]
struct Control {
    passes: bool,
    status: Option<String>,
    code: Option<String>,
}

/// The corpus's assembly cases with expected sockets, and their variants
/// with their base's targets, each with its base's FK4 marks
/// (`expect.fit_control`).
fn corpus_originals(dir: &Path) -> Vec<Original> {
    let manifest: Value =
        serde_json::from_str(&fs::read_to_string(dir.join("manifest.json")).unwrap()).unwrap();
    let all = manifest["cases"].as_array().unwrap();
    let base_of = |c: &Value| -> String {
        c["construction"]["variant_of"]
            .as_str()
            .or_else(|| c["id"].as_str())
            .unwrap_or_default()
            .to_owned()
    };
    let mut out = Vec::new();
    for case in all.iter().filter(|c| c["kind"] == "assembly") {
        let Some(base) = all.iter().find(|c| c["id"] == base_of(case).as_str()) else {
            continue;
        };
        let Some(parts) = base["expect"]["sockets"].as_object() else {
            continue;
        };
        let bytes = fs::read(dir.join(case["file"].as_str().unwrap())).unwrap();
        for part in parts.keys() {
            out.push(Original {
                id: case["id"].as_str().unwrap().to_owned(),
                bytes: bytes.clone(),
                part: part.clone(),
                expect: base["expect"]["fit_control"].get(part).cloned(),
            });
        }
    }
    out
}

/// The fixtures whose manifest pins a socket, each socket with its pinned
/// control (`fit_control`); `None` with a note when the files are not
/// fetched, unless `FITMENT_REQUIRE_FIXTURES=1`.
fn fixture_originals() -> Option<Vec<Original>> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
    let manifest: Value =
        serde_json::from_str(&fs::read_to_string(root.join("manifest.json")).unwrap()).unwrap();
    let mut out = Vec::new();
    for fixture in manifest["fixtures"].as_array().unwrap() {
        let Some(sockets) = fixture["sockets"].as_object().filter(|s| !s.is_empty()) else {
            continue;
        };
        let path = fixture["path"].as_str().unwrap();
        let Ok(bytes) = fs::read(root.join(path)) else {
            let fixtures = std::env::var_os("FITMENT_REQUIRE_FIXTURES").is_some_and(|v| v == "1");
            assert!(
                !fixtures,
                "FITMENT_REQUIRE_FIXTURES=1 but {path} is missing"
            );
            eprintln!("skipped: run tools/fetch-fixtures.sh core");
            return None;
        };
        for (part, pinned) in sockets {
            out.push(Original {
                id: path.to_owned(),
                bytes: bytes.clone(),
                part: part.clone(),
                expect: Some(
                    pinned
                        .get("fit_control")
                        .cloned()
                        .unwrap_or_else(|| panic!("{path} {part}: no fit_control pinned (VT59)")),
                ),
            });
        }
    }
    Some(out)
}

/// The control of `original`, judged by `session`'s helper under the
/// default policy.
fn control(session: &OcctSession<'_>, original: &Original) -> Control {
    let document = SocketDocument::extract(&original.bytes, Some(&original.part), None)
        .unwrap_or_else(|e| panic!("{} {}: {e}", original.id, original.part));
    let matcher = Matcher::new(
        &document,
        MatchPolicy::default(),
        MatchOptions::default(),
        session,
    )
    .unwrap();
    let component = SelectedComponent::target(&original.bytes, &document)
        .unwrap_or_else(|e| panic!("{} {}: {e}", original.id, original.part));
    let result = matcher.judge_original(&component, component.source());
    // A reject whose fit was asked is the fit's (an interference); one
    // whose fit was not is the interface's.
    let passes = result.verdict.kind() != VerdictKind::Reject
        || !matches!(result.verdict.fit(), FitStatus::NotChecked { .. });
    let evidence = matcher.fit_control();
    let status = evidence.as_ref().map(|e| {
        let status = match e.outcome().candidate() {
            FitStatus::Verified { .. } => "verified",
            FitStatus::Interferes { .. } => "interferes",
            FitStatus::NotVerified { .. } => "not_verified",
            FitStatus::NotChecked { .. } => "not_checked",
        };
        status.to_owned()
    });
    let code = evidence
        .as_ref()
        .and_then(|e| e.outcome().code())
        .filter(|_| status.as_deref() == Some("not_verified"))
        .map(|c| c.as_str().to_owned());
    Control {
        passes,
        status,
        code,
    }
}

/// Where `found` departs from what `original`'s source says of its
/// control: a mark, or else `verified` wherever the interface passes.
fn problems(original: &Original, found: &Control) -> Vec<String> {
    let at = format!("{} {}", original.id, original.part);
    let mut out = Vec::new();
    let expect = original.expect.clone().unwrap_or(Value::Null);
    if expect["interface"] == "rejects" {
        if found.passes {
            out.push(format!(
                "{at}: its interface passes, its mark says it rejects"
            ));
        }
        return out;
    }
    if !found.passes {
        if expect.is_object() {
            out.push(format!(
                "{at}: its interface rejects, its mark says {expect}"
            ));
        }
        return out;
    }
    let status = expect["status"].as_str().unwrap_or("verified");
    if found.status.as_deref() != Some(status) {
        out.push(format!("{at}: its control is {found:?}, expected {status}"));
    }
    if let Some(code) = expect["code"].as_str() {
        if found.code.as_deref() != Some(code) {
            out.push(format!(
                "{at}: its control's code {:?}, expected {code}",
                found.code
            ));
        }
    }
    out
}

/// Every original judged, `workers` at a time, one session per assembly;
/// each with what its control came to, in the order given.
fn run(dir: &Path, originals: &[Original]) -> Vec<Control> {
    let helper = OcctHelper::open(
        dir,
        HelperFiles::embedded(),
        proved(HelperOptions::default()),
    )
    .unwrap();
    let mut by_assembly: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (i, o) in originals.iter().enumerate() {
        by_assembly.entry(o.id.as_str()).or_default().push(i);
    }
    let queue = Mutex::new(by_assembly.into_values().collect::<Vec<_>>());
    let done = Mutex::new(BTreeMap::new());
    std::thread::scope(|s| {
        for _ in 0..4 {
            s.spawn(|| {
                loop {
                    let Some(indices) = queue.lock().unwrap().pop() else {
                        break;
                    };
                    let first = &originals[indices[0]];
                    let document =
                        SocketDocument::extract(&first.bytes, Some(&first.part), None).unwrap();
                    let session =
                        OcctSession::open(&helper, first.bytes.clone(), &document).unwrap();
                    for i in indices {
                        let found = control(&session, &originals[i]);
                        done.lock().unwrap().insert(i, found);
                    }
                    let reports = session.take_reports();
                    assert!(reports.is_empty(), "{}: {reports:#?}", first.id);
                }
            });
        }
    });
    done.into_inner().unwrap().into_values().collect()
}

/// The counts the as-built note states: originals, interface passes, and
/// controls by status and code.
fn tally(found: &[Control]) -> Value {
    let mut by: BTreeMap<String, usize> = BTreeMap::new();
    for f in found.iter().filter(|f| f.passes) {
        let key = match (&f.status, &f.code) {
            (Some(s), Some(c)) => format!("{s} {c}"),
            (Some(s), None) => s.clone(),
            _ => "no control".to_owned(),
        };
        *by.entry(key).or_default() += 1;
    }
    json!({"originals": found.len(), "interface_passes": found.iter().filter(|f| f.passes).count(),
           "controls": by})
}

fn assert_none(problems: &[String]) {
    assert!(
        problems.is_empty(),
        "{} problems:\n{}",
        problems.len(),
        problems.join("\n")
    );
}

/// VT59, FK4, FK5 on the corpus: every original's control `verified`
/// wherever its interface passes, but s20's (`interferes`, by
/// construction); s14's interface rejects, as its mark says.
#[test]
fn every_corpus_original_s_control_is_as_its_construction_says() {
    let Some((corpus, dir)) = inputs() else {
        return;
    };
    if refused_here() {
        eprintln!("the helper is refused on Windows until VT77 is green (M7-50)");
        return;
    }
    let originals = corpus_originals(corpus.dir());
    assert!(originals.len() > 100, "{} originals", originals.len());
    let started = Instant::now();
    let found = run(dir, &originals);
    let summary = tally(&found);
    eprintln!(
        "the corpus's originals: {summary}, {:.1} s",
        started.elapsed().as_secs_f64()
    );
    fitment_corpus::report::write("fit-originals.json", &summary).unwrap();
    let all: Vec<String> = originals
        .iter()
        .zip(&found)
        .flat_map(|(o, f)| problems(o, f))
        .collect();
    assert_none(&all);
    let marked = originals.iter().filter(|o| o.expect.is_some()).count();
    assert!(marked >= 2, "FK4's marks: {marked}");
}

/// VT59 on the fixtures (section 5.3): each pinned socket's control as
/// the fixtures' manifest pins it (`fit_control`): AS1-ac's and -ug's
/// verified, AS1-md's `uncertainty_above_tolerance`, every other
/// interface rejecting first.
#[test]
fn every_fixture_original_s_control_is_pinned() {
    let Some(dir) = kernel() else {
        return;
    };
    if refused_here() {
        eprintln!("the helper is refused on Windows until VT77 is green (M7-50)");
        return;
    }
    let Some(originals) = fixture_originals() else {
        return;
    };
    let started = Instant::now();
    let found = run(dir, &originals);
    eprintln!(
        "the fixtures' originals: {}, {:.1} s",
        tally(&found),
        started.elapsed().as_secs_f64()
    );
    for (o, f) in originals.iter().zip(&found) {
        eprintln!("  {} {}: {f:?}", o.id, o.part);
    }
    let all: Vec<String> = originals
        .iter()
        .zip(&found)
        .flat_map(|(o, f)| problems(o, f))
        .collect();
    assert_none(&all);
}
