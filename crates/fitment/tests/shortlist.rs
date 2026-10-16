//! The signature and the shortlist over the generated corpus
//! (`docs/research/m4-match.md`, PR 2's tests): every original is kept by
//! its own sockets in every frame its case is written in, under every
//! policy; level 1 returns whatever level 2 keeps over the corpus's parts
//! and components; and a part's signature does not depend on its frame or
//! its writer. Without the corpus these pass with a note, unless
//! `FITMENT_REQUIRE_CORPUS=1`.

use std::fs;
use std::path::{Path, PathBuf};

use fitment_core::{
    Clearance, ComponentRead, FeaturesDocument, Grip, IndexTokens, MatchPolicy, Part,
    PartSignature, ShortlistIndex, SocketDocument, SocketSignature, SocketView, TolerancePolicy,
    read_assembly, shortlist,
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

fn cases(dir: &Path) -> Vec<Value> {
    let text = fs::read_to_string(dir.join("manifest.json")).unwrap();
    let manifest: Value = serde_json::from_str(&text).unwrap();
    manifest["cases"].as_array().unwrap().clone()
}

/// Part `name` of an assembly, as a candidate reads it.
fn component(bytes: &[u8], name: &str) -> Option<Part> {
    let read = read_assembly(bytes);
    let listing = read.listing()?;
    let selection = listing.select(Some(name), None).ok()?;
    match &listing.definitions()[selection.definition].read {
        Some(ComponentRead::Accepted(part)) => Some((**part).clone()),
        _ => None,
    }
}

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

/// The assembly cases with expected sockets, and their variants (a frame,
/// a writer, an order) with their base's targets: (id, bytes, targets).
fn targets(dir: &Path) -> Vec<(String, Vec<u8>, Vec<String>)> {
    let all = cases(dir);
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

/// Section 6.4 on every original, in every frame and writer its case is
/// written in: its own sockets keep it, under every policy.
#[test]
fn every_original_is_kept_by_its_own_sockets() {
    let Some(dir) = corpus() else {
        return;
    };
    let mut failures = Vec::new();
    let mut runs = 0;
    for (id, bytes, parts) in targets(&dir) {
        for part in parts {
            let original =
                component(&bytes, &part).unwrap_or_else(|| panic!("{id} {part} is read"));
            let original = PartSignature::of(&original);
            let document = SocketDocument::extract(&bytes, Some(&part), None).unwrap();
            let extracted = document
                .extracted()
                .unwrap_or_else(|| panic!("{id} {part} is accepted"));
            for socket in &extracted.sockets {
                let view = SocketView::new(&document, &socket.id).unwrap();
                for policy in policies() {
                    runs += 1;
                    if let Err(missing) = shortlist(&SocketSignature::of(&view, &policy), &original)
                    {
                        failures.push(format!("{id} {part} {}: {missing:?}", socket.id));
                    }
                }
            }
        }
    }
    assert!(runs > 1_000, "{runs} runs");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// SL9 over the corpus: every corpus part and every base assembly's
/// components, indexed; every base socket queried; whatever level 2 keeps
/// is among what level 1 returns.
#[test]
fn level_one_returns_whatever_level_two_keeps() {
    let Some(dir) = corpus() else {
        return;
    };
    let variant = |id: &str| {
        ["_rot", "_tgt", "_pc", "_perm"]
            .iter()
            .any(|v| id.ends_with(v))
    };
    let mut pool: Vec<PartSignature> = Vec::new();
    for case in cases(&dir) {
        let Some(id) = case["id"].as_str() else {
            continue;
        };
        if case["kind"] == "assembly" || variant(id) {
            continue;
        }
        let Some(file) = case["file"].as_str() else {
            continue;
        };
        let read = FeaturesDocument::read(&fs::read(dir.join(file)).unwrap());
        pool.extend(read.part().map(PartSignature::of));
    }
    let base: Vec<_> = targets(&dir)
        .into_iter()
        .filter(|(id, ..)| !variant(id))
        .collect();
    for (_, bytes, parts) in &base {
        pool.extend(
            parts
                .iter()
                .filter_map(|p| component(bytes, p))
                .map(|p| PartSignature::of(&p)),
        );
    }
    let mut index = ShortlistIndex::new();
    for (k, part) in (0_u64..).zip(&pool) {
        index.insert(k, &IndexTokens::of(part));
    }
    let policy = MatchPolicy::default();
    let (mut kept, mut returned) = (0_usize, 0_usize);
    let mut failures = Vec::new();
    for (id, bytes, parts) in &base {
        for part in parts {
            let Ok(document) = SocketDocument::extract(bytes, Some(part), None) else {
                continue;
            };
            let Some(extracted) = document.extracted() else {
                continue;
            };
            for socket in &extracted.sockets {
                let view = SocketView::new(&document, &socket.id).unwrap();
                let signature = SocketSignature::of(&view, &policy);
                let Ok(found) = index.query(&signature) else {
                    continue;
                };
                returned += found.len();
                for (k, candidate) in (0_u64..).zip(&pool) {
                    if shortlist(&signature, candidate).is_ok() {
                        kept += 1;
                        if found.binary_search(&k).is_err() {
                            failures.push(format!("{id} {part} {}: level 2 keeps {k}", socket.id));
                        }
                    }
                }
            }
        }
    }
    assert!(
        kept > 0 && returned >= kept,
        "{kept} kept, {returned} returned"
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// SG1: what the shortlist compares of a part is the same in every frame
/// and writer its case is written in.
#[test]
fn a_part_s_signature_does_not_depend_on_its_frame() {
    let Some(dir) = corpus() else {
        return;
    };
    let all = cases(&dir);
    let signature = |file: &str| -> Option<PartSignature> {
        let read = FeaturesDocument::read(&fs::read(dir.join(file)).ok()?);
        read.part().map(PartSignature::of)
    };
    let mut compared = 0;
    let mut failures = Vec::new();
    for case in &all {
        let (Some(id), Some(file)) = (case["id"].as_str(), case["file"].as_str()) else {
            continue;
        };
        if case["kind"] == "assembly" || ["_rot", "_pc"].iter().any(|v| id.ends_with(v)) {
            continue;
        }
        let Some(base) = signature(file) else {
            continue;
        };
        for v in ["_rot", "_pc"] {
            let Some(other) = signature(&format!("{id}{v}.stp")) else {
                continue;
            };
            compared += 1;
            if invariants(&base) != invariants(&other) {
                failures.push(format!(
                    "{id}{v}: {:?} against {:?}",
                    invariants(&other),
                    invariants(&base)
                ));
            }
        }
    }
    // CF9: features in millimetres whatever the file's unit: p16 is p05
    // written in inches.
    let (Some(mm), Some(inch)) = (signature("p05_cbore.stp"), signature("p16_inch.stp")) else {
        panic!("p05 and p16 are read");
    };
    assert_eq!(invariants(&mm), invariants(&inch));
    assert!(compared > 10, "{compared} compared");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The values the shortlist compares, rounded to 1e-6 and sorted: each
/// stack's sense, diameters, length and openings; each plane's known area
/// and faces; each cone's angle, sense and radii.
fn invariants(sig: &PartSignature) -> Vec<String> {
    let r = |x: f64| format!("{:.6}", x + 0.0);
    let mut out: Vec<String> = sig
        .stacks()
        .iter()
        .map(|s| {
            let length = match s.extent_mm {
                [Some(a), Some(b)] => r(b - a),
                _ => "open".to_owned(),
            };
            // In the order its axis runs, which a frame may reverse.
            let mut d: Vec<String> = s.diameters_mm.iter().map(|d| r(*d)).collect();
            d.sort();
            format!("stack {:?} {d:?} {length} {}", s.sense, s.openings)
        })
        .collect();
    out.extend(sig.planes().iter().map(|p| {
        format!(
            "plane {} {} {}",
            r(p.known_area_mm2),
            p.faces_known,
            p.faces_unknown
        )
    }));
    out.extend(sig.cones().iter().map(|c| {
        format!(
            "cone {} {:?} {:?}",
            r(c.included_angle_rad),
            c.sense,
            c.radii_mm.map(|[a, b]| (r(a), r(b)))
        )
    }));
    out.sort();
    out
}

/// Section 6.4 on the fixtures' pinned targets (AS1's exports, the
/// weldment, vaccase): each original is kept by its own sockets.
#[test]
fn every_pinned_fixture_original_is_kept_by_its_own_sockets() {
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
    let mut failures = Vec::new();
    let mut runs = 0;
    for fixture in pinned {
        let path = fixture["path"].as_str().unwrap();
        let bytes = fs::read(root.join(path)).unwrap();
        for target in fixture["sockets"].as_object().unwrap().keys() {
            let original =
                component(&bytes, target).unwrap_or_else(|| panic!("{path} {target} is read"));
            let original = PartSignature::of(&original);
            let document = SocketDocument::extract(&bytes, Some(target), None).unwrap();
            for socket in &document.extracted().unwrap().sockets {
                let view = SocketView::new(&document, &socket.id).unwrap();
                for policy in policies() {
                    runs += 1;
                    if let Err(missing) = shortlist(&SocketSignature::of(&view, &policy), &original)
                    {
                        failures.push(format!("{path} {target} {}: {missing:?}", socket.id));
                    }
                }
            }
        }
    }
    assert!(runs > 100, "{runs} runs");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// CF4 to CF7 against the `Part` itself, on every corpus part: a plane's
/// known area is its faces' of known outline alone, the others counted;
/// a cone keeps an unknown range unknown; a stack is unbounded exactly on
/// its unknown or irregular sides; undecided and partial cylinders and
/// `other` faces are counted, never stacks. The corpus has each.
#[test]
fn a_signature_states_exactly_what_its_part_reads() {
    use fitment_core::{CylinderClass, Outline, StackEnd};
    let Some(dir) = corpus() else {
        return;
    };
    let unknown = |e: Option<&StackEnd>| {
        matches!(
            e,
            None | Some(StackEnd::Irregular { .. } | StackEnd::Unknown { .. })
        )
    };
    let mut seen = [0_usize; 4];
    for case in cases(&dir) {
        if case["kind"] == "assembly" {
            continue;
        }
        let Some(file) = case["file"].as_str() else {
            continue;
        };
        let read = FeaturesDocument::read(&fs::read(dir.join(file)).unwrap());
        let Some(part) = read.part() else {
            continue;
        };
        let sig = PartSignature::of(part);
        for (plane, p) in part.planes.iter().zip(sig.planes()) {
            let known: f64 = plane
                .faces
                .iter()
                .filter_map(|f| match &f.outline {
                    Outline::Known { loops, .. } => {
                        Some(loops.iter().map(|l| l.area_mm2).sum::<f64>())
                    }
                    _ => None,
                })
                .sum();
            let unknown_faces = plane
                .faces
                .iter()
                .filter(|f| !matches!(f.outline, Outline::Known { .. }))
                .count();
            // An empty sum is −0.0; the signature stores +0.0.
            assert_eq!(
                p.known_area_mm2.to_bits(),
                (known + 0.0).to_bits(),
                "{file}"
            );
            assert_eq!(p.faces_unknown as usize, unknown_faces, "{file}");
            seen[0] += unknown_faces;
        }
        for (cone, c) in part.cones.iter().zip(sig.cones()) {
            assert_eq!(c.radii_mm, cone.radii_mm, "{file}");
            seen[1] += usize::from(cone.radii_mm.is_none());
        }
        let stacks = part
            .holes
            .iter()
            .map(|h| (h.id, &h.ends))
            .chain(part.shafts.iter().map(|s| (s.id, &s.ends)));
        for (id, ends) in stacks {
            let s = sig.stacks().iter().find(|s| s.feature == id).unwrap();
            assert_eq!(
                s.extent_mm[0].is_none(),
                unknown(ends.first()),
                "{file} {id:?}"
            );
            assert_eq!(
                s.extent_mm[1].is_none(),
                unknown(ends.get(1)),
                "{file} {id:?}"
            );
            seen[2] += usize::from(s.unknown_end);
        }
        let partial = part
            .cylinders
            .iter()
            .filter(|c| matches!(c.class, CylinderClass::Partial { .. }))
            .count();
        let undecided = part
            .cylinders
            .iter()
            .filter(|c| matches!(c.class, CylinderClass::Undecided { .. }))
            .count();
        assert_eq!(sig.unread().partial as usize, partial, "{file}");
        assert_eq!(sig.unread().undecided as usize, undecided, "{file}");
        assert_eq!(
            sig.unread().other_faces as usize,
            part.other_faces.len(),
            "{file}"
        );
        assert_eq!(
            sig.stacks().len(),
            part.holes.len() + part.shafts.len(),
            "{file}"
        );
        seen[3] += partial + undecided + part.other_faces.len();
    }
    assert!(seen.iter().all(|n| *n > 0), "{seen:?}");
}
