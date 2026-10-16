//! Y4 (`docs/research/m4-match.md`, section 18, VR21, KH10): every corpus
//! original, read in place from its assembly, against its own sockets.
//! At the identity, it passes every requirement M4 can verify and fails
//! each other one with `unverifiable` or `floor_above_tolerance`. Without
//! the corpus this passes with a note, unless `FITMENT_REQUIRE_CORPUS=1`.

use std::fs;
use std::path::{Path, PathBuf};

use fitment_core::{
    CandidateView, Clearance, ComponentRead, FailureCode, Grip, MatchPolicy, Part, Placement,
    Rigid, SocketDocument, SocketView, TolerancePolicy, read_assembly, verify,
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
