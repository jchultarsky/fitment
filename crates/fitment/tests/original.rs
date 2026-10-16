//! Y4 (`docs/research/m4-match.md`, section 18, VR21, KH10): every corpus
//! original, read in place from its assembly, against its own sockets.
//! At the identity, it passes every requirement M4 can verify and fails
//! each other one with `unverifiable` or `floor_above_tolerance`. Without
//! the corpus this passes with a note, unless `FITMENT_REQUIRE_CORPUS=1`.

use std::fs;
use std::path::{Path, PathBuf};

use fitment_core::{
    CandidateView, Clearance, ComponentRead, FailureCode, Grip, MatchOptions, MatchPolicy, Matcher,
    Part, Placement, Rigid, SocketDocument, SocketView, TolerancePolicy, read_assembly, verify,
};
use serde_json::Value;

fn corpus() -> Option<PathBuf> {
    let dir = std::env::var_os("FITMENT_CORPUS").map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/out"),
        PathBuf::from,
    );
    let required = std::env::var_os("FITMENT_REQUIRE_CORPUS").is_some_and(|v| v == "1");
    if dir.join("manifest.json").is_file() {
        return Some(dir);
    }
    assert!(!required, "FITMENT_REQUIRE_CORPUS=1 but there is no corpus");
    eprintln!("skipped: no corpus; run tools/gen-corpus.py corpus/out");
    None
}

fn component(bytes: &[u8], name: &str) -> Option<Part> {
    let read = read_assembly(bytes);
    let listing = read.listing()?;
    let selection = listing.select(Some(name), None).ok()?;
    match &listing.definitions()[selection.definition].read {
        Some(ComponentRead::Accepted(part)) => Some((**part).clone()),
        _ => None,
    }
}

/// The assembly cases with expected sockets, and their variants with their
/// base's targets: (id, bytes, targets).
fn targets(dir: &Path) -> Vec<(String, Vec<u8>, Vec<String>)> {
    let text = fs::read_to_string(dir.join("manifest.json")).unwrap();
    let manifest: Value = serde_json::from_str(&text).unwrap();
    let all = manifest["cases"].as_array().unwrap();
    let of = |id: &str| -> Option<Vec<String>> {
        let base = all.iter().find(|c| c["id"] == id)?;
        Some(
            base["expect"]["sockets"]
                .as_object()?
                .keys()
                .cloned()
                .collect(),
        )
    };
    all.iter()
        .filter(|c| c["kind"] == "assembly")
        .filter_map(|c| {
            let id = c["id"].as_str()?.to_owned();
            let parts = match c["construction"]["variant_of"].as_str() {
                Some(base) => of(base)?,
                None => of(&id)?,
            };
            let bytes = fs::read(dir.join(c["file"].as_str()?)).ok()?;
            Some((id, bytes, parts))
        })
        .collect()
}

/// The policies of the matrix (VR21): strict and functional, at the default
/// tolerance and a coarse one.
fn policies() -> Vec<MatchPolicy> {
    [
        TolerancePolicy::default(),
        TolerancePolicy::new("coarse", 0.1, 0.1).unwrap(),
    ]
    .iter()
    .flat_map(|t| {
        [Clearance::Strict, Clearance::Functional]
            .map(|c| MatchPolicy::new(t.clone(), c, Grip::Hard))
    })
    .collect()
}

/// What one original's run against its own sockets found: socket runs,
/// and each requirement whose result is not the one Y4 expects.
fn run_original(name: &str, bytes: &[u8], target: &str, failures: &mut Vec<String>) -> usize {
    let at = Placement::given(Rigid::IDENTITY);
    let original = component(bytes, target).unwrap_or_else(|| panic!("{name} {target}"));
    let candidate = CandidateView::of(&original);
    let document = SocketDocument::extract(bytes, Some(target), None).unwrap();
    let Some(extracted) = document.extracted() else {
        panic!("{name} {target} is accepted");
    };
    let mut runs = 0;
    for socket in &extracted.sockets {
        let view = SocketView::new(&document, &socket.id).unwrap();
        for policy in policies() {
            runs += 1;
            for requirement in view.active() {
                let check = verify(&view, requirement, &candidate, &at, &policy);
                let expected = requirement.verifiable() && !view.blocked(requirement, &policy);
                let ok = if check.passed() {
                    expected
                } else {
                    !expected
                        && check.evidence().failures().iter().all(|f| {
                            matches!(
                                f.code,
                                FailureCode::Unverifiable | FailureCode::FloorAboveTolerance
                            )
                        })
                };
                if !ok {
                    failures.push(format!(
                        "{name} {target} {} {} {} {:?}: {}",
                        socket.id,
                        requirement.id,
                        policy.tolerance().name(),
                        policy.clearance(),
                        serde_json::to_string(check.evidence().failures()).unwrap()
                    ));
                }
            }
        }
    }
    runs
}

/// Y4 at the identity (VR21 seeded), over every corpus original in every
/// frame and writer its case is written in.
#[test]
fn every_original_passes_what_m4_can_verify_at_the_identity() {
    let Some(dir) = corpus() else {
        return;
    };
    let mut runs = 0;
    let mut failures = Vec::new();
    for (id, bytes, parts) in targets(&dir) {
        for part in parts {
            runs += run_original(&id, &bytes, &part, &mut failures);
        }
    }
    assert!(runs > 1_000, "{runs} runs");
    assert!(
        failures.is_empty(),
        "{} failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// Y4 at the identity over every pinned fixture target (KC11): AS1 in its
/// exports, the weldment, vaccase and the rest.
#[test]
fn every_pinned_fixture_original_passes_what_m4_can_verify_at_the_identity() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
    let manifest: Value =
        serde_json::from_str(&fs::read_to_string(root.join("manifest.json")).unwrap()).unwrap();
    let pinned: Vec<&Value> = manifest["fixtures"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f.get("sockets").is_some())
        .collect();
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
    let mut runs = 0;
    let mut failures = Vec::new();
    for fixture in pinned {
        let path = fixture["path"].as_str().unwrap();
        let bytes = fs::read(root.join(path)).unwrap();
        for target in fixture["sockets"].as_object().unwrap().keys() {
            runs += run_original(path, &bytes, target, &mut failures);
        }
    }
    assert!(runs > 100, "{runs} runs");
    assert!(
        failures.is_empty(),
        "{} failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// KH10, VR21 unseeded: `original`, aligned from scratch against every
/// socket of `document`, passes every requirement M4 can verify and fails
/// every other one with `unverifiable` or `floor_above_tolerance`. A socket
/// whose alignment exhausts its bound is allowed only as `allowed` says.
fn run_unseeded(
    name: &str,
    original: &Part,
    document: &SocketDocument,
    allowed: bool,
    failures: &mut Vec<String>,
) -> usize {
    let mut runs = 0;
    for policy in policies() {
        let matcher = Matcher::new(
            document,
            policy.clone(),
            MatchOptions::default(),
            &fitment_core::NoFitVerifier,
        )
        .unwrap();
        let result = matcher.judge_part(original, fitment_core::CandidateSource::Given);
        for socket in &result.sockets {
            runs += 1;
            let view = SocketView::new(document, &socket.socket).unwrap();
            for (id, check) in &socket.checks {
                let requirement = view.active().iter().find(|r| r.id == id.as_str()).unwrap();
                let expected = requirement.verifiable() && !view.blocked(requirement, &policy);
                let ok = if check.passed() {
                    expected
                } else {
                    !expected
                        && check.evidence().failures().iter().all(|f| {
                            matches!(
                                f.code,
                                FailureCode::Unverifiable | FailureCode::FloorAboveTolerance
                            )
                        })
                };
                let bounded = check.evidence().failures().iter().any(|f| {
                    matches!(
                        f.code,
                        FailureCode::AlignmentExhausted | FailureCode::CoverageExhausted
                    )
                });
                if !(ok || allowed && bounded) {
                    failures.push(format!(
                        "{name} {} {id} {} {:?} ({:?}, {} placements): {}",
                        socket.socket,
                        policy.tolerance().name(),
                        policy.clearance(),
                        socket.stage,
                        socket.placements.0,
                        serde_json::to_string(check.evidence().failures()).unwrap()
                    ));
                }
            }
        }
    }
    runs
}

/// KH10 (VR21, Y4's original run): every corpus original, unseeded, in its
/// own frame; the `_tgt` and `_pc` copies against the base's socket, and
/// the base's original against the `_tgt` socket. s55's plate, whose
/// landmarks exhaust the bound before reaching the tiles under its block,
/// is the one named exception; at the identity it passes (above).
#[test]
fn every_original_fails_exactly_what_no_candidate_can_pass() {
    let Some(dir) = corpus() else {
        return;
    };
    let all = targets(&dir);
    let mut runs = 0;
    let mut failures = Vec::new();
    for (id, bytes, parts) in &all {
        for part in parts {
            let document = SocketDocument::extract(bytes, Some(part), None).unwrap();
            let original = component(bytes, part).unwrap();
            let allowed = id.starts_with("s55_") && part == "PLATE";
            runs += run_unseeded(
                &format!("{id} {part}"),
                &original,
                &document,
                allowed,
                &mut failures,
            );
            for suffix in ["_tgt", "_pc"] {
                let Some((_, copy, _)) = all.iter().find(|(c, ..)| *c == format!("{id}{suffix}"))
                else {
                    continue;
                };
                let Some(other) = component(copy, part) else {
                    continue;
                };
                let name = format!("{id}{suffix} {part} against {id}");
                runs += run_unseeded(&name, &other, &document, allowed, &mut failures);
                if suffix == "_tgt" {
                    let theirs = SocketDocument::extract(copy, Some(part), None).unwrap();
                    let name = format!("{id} {part} against {id}{suffix}");
                    runs += run_unseeded(&name, &original, &theirs, allowed, &mut failures);
                }
            }
        }
    }
    assert!(runs > 1_000, "{runs} runs");
    assert!(
        failures.is_empty(),
        "{} failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// KH10 over every pinned fixture target (KC11), unseeded.
#[test]
fn every_pinned_fixture_original_fails_exactly_what_no_candidate_can_pass() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
    let manifest: Value =
        serde_json::from_str(&fs::read_to_string(root.join("manifest.json")).unwrap()).unwrap();
    let pinned: Vec<&Value> = manifest["fixtures"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f.get("sockets").is_some())
        .collect();
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
    let mut runs = 0;
    let mut failures = Vec::new();
    for fixture in pinned {
        let path = fixture["path"].as_str().unwrap();
        let bytes = fs::read(root.join(path)).unwrap();
        for target in fixture["sockets"].as_object().unwrap().keys() {
            let document = SocketDocument::extract(&bytes, Some(target), None).unwrap();
            let original = component(&bytes, target).unwrap();
            let name = format!("{path} {target}");
            runs += run_unseeded(&name, &original, &document, false, &mut failures);
        }
    }
    assert!(runs > 100, "{runs} runs");
    assert!(
        failures.is_empty(),
        "{} failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
