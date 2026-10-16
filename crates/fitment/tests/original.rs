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

/// KC12: AS1's five parts in each of its six exports, each export's part
/// against each export's socket of it (180 runs). The same model: among
/// ac, md and ug every socket passes every export's part, but those with a
/// requirement that fails every candidate (each bolt's `socket-2`), which
/// reject for exactly that; pe's parts state 0.115 to 0.672 mm and fail on
/// the floor; ec's and tc's sockets are unverifiable, and their parts have
/// no decided hole to place by. Nothing else passes.
#[test]
fn as1_parts_match_across_exports() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
    let exports = [
        (
            "ac",
            "steptools/as1-ac-214.stp",
            ["PLATE", "L-BRACKET", "BOLT", "NUT", "ROD"],
        ),
        (
            "md",
            "steptools/as1-md-214.stp",
            ["PLATE", "LBRACKET", "BOLT", "NUT", "ROD"],
        ),
        (
            "ug",
            "steptools/as1-ug-214.stp",
            ["plate", "l_bracket", "bolt", "nut", "rod"],
        ),
        (
            "pe",
            "nist-edm/as1_pe.stp",
            ["PLATE", "L-BRACKET", "BOLT", "NUT", "ROD"],
        ),
        (
            "ec",
            "steptools/as1-ec-214.stp",
            [
                "as1 1.2",
                "as1 1.1.1",
                "as1 1.1.2.2",
                "as1 1.1.2.1",
                "as1 1.4.1",
            ],
        ),
        (
            "tc",
            "steptools/as1-tc-214.stp",
            ["plate", "l-bracket", "bolt", "nut", "rod"],
        ),
    ];
    if exports.iter().any(|e| !root.join(e.1).is_file()) {
        let required = std::env::var_os("FITMENT_REQUIRE_FIXTURES").is_some_and(|v| v == "1");
        assert!(
            !required,
            "FITMENT_REQUIRE_FIXTURES=1 but fixtures are missing"
        );
        eprintln!("skipped: run tools/fetch-fixtures.sh core");
        return;
    }
    let bytes: Vec<Vec<u8>> = exports
        .iter()
        .map(|e| fs::read(root.join(e.1)).unwrap())
        .collect();
    let good = |name: &str| ["ac", "md", "ug"].contains(&name);
    let mut runs = 0;
    let mut failures = Vec::new();
    for k in 0..5 {
        for (si, socket_export) in exports.iter().enumerate() {
            let document =
                SocketDocument::extract(&bytes[si], Some(socket_export.2[k]), None).unwrap();
            let matcher = Matcher::new(
                &document,
                MatchPolicy::default(),
                MatchOptions::default(),
                &fitment_core::NoFitVerifier,
            )
            .unwrap();
            for (ci, candidate_export) in exports.iter().enumerate() {
                runs += 1;
                let part = component(&bytes[ci], candidate_export.2[k]).unwrap();
                let result = matcher.judge_part(&part, fitment_core::CandidateSource::Given);
                let name = format!(
                    "{} {} against {}",
                    candidate_export.0, socket_export.2[k], socket_export.0
                );
                let passed = result.verdict.kind() != fitment_core::VerdictKind::Reject;
                let pair = good(socket_export.0) && good(candidate_export.0);
                if passed && !pair {
                    failures.push(format!("{name}: passes"));
                }
                if pair {
                    // Fails exactly what no candidate can pass, as the
                    // original does.
                    for socket in &result.sockets {
                        let view = SocketView::new(&document, &socket.socket).unwrap();
                        for (id, check) in &socket.checks {
                            let requirement =
                                view.active().iter().find(|r| r.id == id.as_str()).unwrap();
                            let expected = requirement.verifiable()
                                && !view.blocked(requirement, &MatchPolicy::default());
                            if check.passed() != expected {
                                failures.push(format!("{name}: {} {id}", socket.socket));
                            }
                        }
                    }
                }
            }
        }
    }
    assert_eq!(runs, 180);
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// KC13: every corpus socket whose original fails requirements no
/// candidate can pass, with each of those removed in review: the original
/// then passes, the removed requirements are listed as not checked and no
/// check names them; and a part with none of the socket's elements is
/// still rejected, for removal waives only what it names.
#[test]
fn review_waives_only_what_it_removes() {
    let Some(dir) = corpus() else {
        return;
    };
    let plain = fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../fitment-core/tests/data/p21_planes.stp"),
    )
    .unwrap();
    let policy = MatchPolicy::default();
    let mut runs = 0;
    let mut failures = Vec::new();
    for (id, bytes, parts) in targets(&dir) {
        if ["_rot", "_tgt", "_pc", "_perm"]
            .iter()
            .any(|v| id.ends_with(v))
        {
            continue;
        }
        for part in parts {
            let mut document = SocketDocument::extract(&bytes, Some(&part), None).unwrap();
            let Some(extracted) = document.extracted().cloned() else {
                continue;
            };
            let mut removed = Vec::new();
            for socket in &extracted.sockets {
                let view = SocketView::new(&document, &socket.id).unwrap();
                for r in view.active() {
                    if !r.verifiable() || view.blocked(r, &policy) {
                        removed.push((socket.id.clone(), r.id.clone()));
                    }
                }
            }
            let active: usize = extracted.sockets.iter().map(|s| s.requirements.len()).sum();
            if removed.is_empty() || removed.len() == active {
                continue;
            }
            for (socket, requirement) in &removed {
                document
                    .decide(
                        requirement,
                        Some(socket),
                        fitment_core::ReviewStatus::Removed,
                        Some("waived"),
                    )
                    .unwrap();
            }
            runs += 1;
            let matcher = Matcher::new(
                &document,
                policy.clone(),
                MatchOptions::default(),
                &fitment_core::NoFitVerifier,
            )
            .unwrap();
            let original = component(&bytes, &part).unwrap();
            let result = matcher.judge_part(&original, fitment_core::CandidateSource::Given);
            let name = format!("{id} {part}");
            for socket in &result.sockets {
                // A socket left with nothing active rejects (VD7); others pass.
                let left = document.active(&socket.socket).count();
                if left > 0 && socket.verdict.kind() == fitment_core::VerdictKind::Reject {
                    failures.push(format!(
                        "{name} {}: rejected after review: {:?}",
                        socket.socket,
                        socket.verdict.reasons()
                    ));
                }
                for (socket_id, requirement) in &removed {
                    if socket_id == &socket.socket {
                        if socket.checks.iter().any(|(r, _)| r.as_str() == requirement) {
                            failures.push(format!("{name}: {requirement} checked though removed"));
                        }
                        let listed = socket.not_checked.iter().any(|n| matches!(n,
                            fitment_core::NotChecked::Removed { requirement: r, .. } if r.as_str() == requirement));
                        if !listed {
                            failures.push(format!("{name}: {requirement} not listed as removed"));
                        }
                    }
                }
            }
            // The plain block has no hole or shaft: where one is still asked,
            // it fails.
            let axial = extracted.sockets.iter().any(|s| {
                document.active(&s.id).any(|r| {
                    matches!(
                        r.requires,
                        fitment_core::Requires::Bore { .. } | fitment_core::Requires::Shaft { .. }
                    )
                })
            });
            if axial
                && matcher.judge_step(&plain).verdict.kind() != fitment_core::VerdictKind::Reject
            {
                failures.push(format!("{name}: a plain block passes after review"));
            }
        }
    }
    assert!(runs > 5, "{runs} runs");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
