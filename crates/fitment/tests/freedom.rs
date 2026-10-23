//! A placement's freedom on the corpus (`docs/research/m7-fit.md`, FT10,
//! FC4): VT62 on the originals the design names, each placed as the
//! matcher places it and, for AL9's seeds, as a cone or a landmark seeds
//! it; and VT76, P6 as a test: section 5.6's counts over every corpus
//! candidate whose interface passes. Without the corpus these pass with a
//! note, unless `FITMENT_REQUIRE_CORPUS=1`.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use fitment_core::fit::{Freedom, MotionKind, freedom};
use fitment_core::{
    CandidateSource, CandidateView, ComponentRead, MatchOptions, MatchPolicy, Matcher,
    NoFitVerifier, Part, Placement, Seed, SocketDocument, SocketView, VerdictKind, align,
    read_assembly, read_part,
};
use fitment_corpus::{CandidateRef, Corpus, Truth};

fn component(bytes: &[u8], name: &str) -> Part {
    let read = read_assembly(bytes);
    let listing = read.listing().unwrap();
    let selection = listing.select(Some(name), None).unwrap();
    match &listing.definitions()[selection.definition].read {
        Some(ComponentRead::Accepted(part)) => (**part).clone(),
        other => unreachable!("{name}: {other:?}"),
    }
}

/// The freedom at each socket of `part` of corpus case `case`, the
/// original placed where the matcher places it: (socket, seed, freedom).
fn original(dir: &Path, case: &str, part: &str) -> Vec<(String, Seed, Freedom)> {
    let bytes = fs::read(dir.join(format!("{case}.stp"))).unwrap();
    let document = SocketDocument::extract(&bytes, Some(part), None).unwrap();
    let policy = MatchPolicy::default();
    let matcher = Matcher::new(
        &document,
        policy.clone(),
        MatchOptions::default(),
        &NoFitVerifier,
    )
    .unwrap();
    let original = component(&bytes, part);
    let cand = CandidateView::of(&original);
    matcher
        .judge_part(&original, CandidateSource::Given)
        .sockets
        .iter()
        .map(|s| {
            let view = SocketView::new(&document, &s.socket).unwrap();
            let at = s.placement.clone().unwrap();
            (
                s.socket.clone(),
                at.seed.clone(),
                freedom(&view, &cand, &at, &policy),
            )
        })
        .collect()
}

/// The first placement AL9 seeds for `part` of `case` at its first
/// socket, and the freedom there.
fn seeded(dir: &Path, case: &str, part: &str, cone: bool) -> (Placement, Freedom) {
    let bytes = fs::read(dir.join(format!("{case}.stp"))).unwrap();
    let document = SocketDocument::extract(&bytes, Some(part), None).unwrap();
    let policy = MatchPolicy::default();
    let original = component(&bytes, part);
    let cand = CandidateView::of(&original);
    let view = SocketView::new(&document, "socket-1").unwrap();
    let at = align(&view, &cand, None, &policy)
        .ordered
        .into_iter()
        .find(|p| {
            if cone {
                matches!(p.seed, Seed::Cone { .. })
            } else {
                matches!(p.seed, Seed::Landmark { .. })
            }
        })
        .unwrap();
    let free = freedom(&view, &cand, &at, &policy);
    (at, free)
}

/// Every free motion a turn about the z axis of its socket, excused or not
/// as `axisymmetric` says.
fn turns_about_z(free: &Freedom, axisymmetric: bool) -> bool {
    !free.motions.is_empty()
        && free.motions.iter().all(|m| {
            let [x, y, z] = m.about.direction.to_array();
            let [px, py, _] = m.about.point_mm.to_array();
            m.kind == MotionKind::Turn
                && m.axisymmetric == axisymmetric
                && x.abs() < 1e-12
                && y.abs() < 1e-12
                && (z.abs() - 1.0).abs() < 1e-12
                && px.abs() < 1e-9
                && py.abs() < 1e-9
        })
}

/// VT62: s01's nut (its hex seat fixes the turn) and bracket (two axes)
/// are unique; s05's washers, s04's bearing and s41's screw are free only
/// to turn about their axis, excused as axisymmetric; s21's block and
/// s04's housing are free to turn and not axisymmetric, so their fit is
/// `placement_not_unique`.
#[test]
fn originals_are_unique_free_or_excused_as_their_shapes_say() {
    let Some(corpus) = Corpus::locate() else {
        return;
    };
    let dir = corpus.dir();
    let unique = [
        ("s01_bolted", "NUT"),
        ("s01_bolted", "BRACKET"),
        ("s18_csink", "PLATE"),
    ];
    for (case, part) in unique {
        for (socket, _, free) in original(dir, case, part) {
            assert!(free.unique(), "{case} {part} {socket}: {free:?}");
        }
    }
    let excused = [
        ("s05_stack", "WASHER", 2),
        ("s04_bearing", "BEARING", 1),
        ("s41_cone_head", "SCREW", 1),
    ];
    for (case, part, sockets) in excused {
        let found = original(dir, case, part);
        assert_eq!(found.len(), sockets, "{case} {part}");
        for (socket, _, free) in found {
            assert!(
                turns_about_z(&free, true),
                "{case} {part} {socket}: {free:?}"
            );
            assert!(free.excused() && !free.unique());
        }
    }
    for (case, part) in [("s21_self_rod", "BLOCK10"), ("s04_bearing", "HOUSING")] {
        for (socket, seed, free) in original(dir, case, part) {
            assert!(
                matches!(seed, Seed::OneAxis { .. }),
                "{case} {part}: {seed:?}"
            );
            assert!(
                turns_about_z(&free, false),
                "{case} {part} {socket}: {free:?}"
            );
            assert!(!free.excused());
            assert!(free.describe().starts_with("a turn about"));
        }
    }
}

/// VT62, FC4: freedom comes from the placement, whatever seeded it. The
/// countersunk screws of s18 and s58, placed by AL9's cone seed, and s53's
/// flange, placed by a landmark, are each free to turn about their axis,
/// and excused, each being axisymmetric.
#[test]
fn cone_and_landmark_seeds_are_judged_alike() {
    let Some(corpus) = Corpus::locate() else {
        return;
    };
    let dir = corpus.dir();
    for case in ["s18_csink", "s58_csink_tilted"] {
        let (at, free) = seeded(dir, case, "SCREW", true);
        assert!(matches!(at.seed, Seed::Cone { .. }));
        assert!(turns_about_z(&free, true), "{case}: {free:?}");
    }
    let (at, free) = seeded(dir, "s53_flange", "FLANGE", false);
    assert!(matches!(at.seed, Seed::Landmark { .. }));
    assert_eq!(free.motions.len(), 1, "{free:?}");
    assert!(free.excused());
    let found = original(dir, "s53_flange", "FLANGE");
    assert!(
        matches!(found[0].1, Seed::Landmark { .. }),
        "the matcher's own seed"
    );
    assert_eq!(found[0].2, free);
}

/// What FK3 states of a placement: `unique`, `turn_axisymmetric` (free
/// only to turn, excused), `slide` (free to slide), or `turn`.
fn statement(free: &Freedom) -> &'static str {
    if free.unique() {
        "unique"
    } else if free.excused() {
        "turn_axisymmetric"
    } else if free.motions.iter().any(|m| m.kind == MotionKind::Slide) {
        "slide"
    } else {
        "turn"
    }
}

/// A case's candidate file judged at its socket (read without review,
/// edit or floor) under `policy`, when its interface passes: the socket's
/// key in `documents`, the result and the file's bytes.
fn interface_pass(
    dir: &Path,
    case: fitment_corpus::Case<'_>,
    documents: &mut BTreeMap<(String, String), SocketDocument>,
    policy: &MatchPolicy,
) -> Option<((String, String), fitment_core::CandidateResult, Vec<u8>)> {
    let CandidateRef::File(file) = case.candidate() else {
        return None;
    };
    let socket = &case.value()["socket"];
    let reviewed = socket["review"].as_array().is_some_and(|r| !r.is_empty());
    let edited = !socket["edit"].is_null() || !socket["floor"].is_null();
    if case.truth() == Truth::Refused || reviewed || edited || !socket["occurrence"].is_null() {
        return None;
    }
    let key = (
        socket["case"].as_str()?.to_owned(),
        socket["part"].as_str()?.to_owned(),
    );
    let document = documents.entry(key.clone()).or_insert_with(|| {
        let bytes = fs::read(dir.join(format!("{}.stp", key.0))).unwrap();
        SocketDocument::extract(&bytes, Some(&key.1), None).unwrap()
    });
    let matcher = Matcher::new(
        document,
        policy.clone(),
        MatchOptions::default(),
        &NoFitVerifier,
    )
    .ok()?;
    let bytes = fs::read(dir.join(&file)).unwrap();
    let result = matcher.judge_step(&bytes);
    (result.verdict.kind() != VerdictKind::Reject).then_some((key, result, bytes))
}

/// VT76, P6 as a test (section 5.6): over every corpus match case whose
/// candidate file passes its interface under the default policy, on a
/// socket read without review, each socket result's freedom by its seed,
/// and each candidate's by its truth: section 5.6's counts over the cases
/// P6 ran (FK2's `f_` cases aside); and for every such case, FK2's
/// included, FC4's freedom at each socket is the one the manifest states
/// (FK3).
#[test]
fn the_corpus_s_free_placements_are_section_5_6_s() {
    let Some(corpus) = Corpus::locate() else {
        return;
    };
    let dir = corpus.dir();
    let policy = MatchPolicy::default();
    let mut documents: BTreeMap<(String, String), SocketDocument> = BTreeMap::new();
    let mut tally: BTreeMap<String, u32> = BTreeMap::new();
    let mut free: Vec<(String, &str)> = Vec::new();
    let mut misstated: Vec<String> = Vec::new();
    let mut stated = 0;
    for case in corpus.match_cases() {
        let Some((key, result, bytes)) = interface_pass(dir, case, &mut documents, &policy) else {
            continue;
        };
        let document = &documents[&key];
        stated += 1;
        let candidate = read_part(&bytes).unwrap();
        let cand = CandidateView::of(&candidate);
        let mut asked = true;
        let p6 = !case.id().starts_with("f_");
        for s in &result.sockets {
            let view = SocketView::new(document, &s.socket).unwrap();
            let at = s.placement.as_ref().unwrap();
            let free_motions = freedom(&view, &cand, at, &policy);
            let stated = case.value()["freedom"].as_str().unwrap_or("none stated");
            if statement(&free_motions) != stated {
                misstated.push(format!(
                    "{} {}: FC4 finds {}, the manifest states {stated}",
                    case.id(),
                    s.socket,
                    statement(&free_motions)
                ));
            }
            if !p6 {
                continue;
            }
            let seed = seed_name(&at.seed);
            let outcome = if free_motions.unique() {
                "unique"
            } else if free_motions.excused() {
                "free turn, axisymmetric"
            } else {
                asked = false;
                "placement_not_unique"
            };
            if outcome != "unique" {
                free.push((format!("{} {}", case.id(), s.socket), outcome));
            }
            if free_motions
                .motions
                .iter()
                .any(|m| m.kind == MotionKind::Slide)
            {
                *tally.entry("a free slide".to_owned()).or_default() += 1;
            }
            *tally.entry(format!("{seed}: {outcome}")).or_default() += 1;
        }
        if !p6 {
            continue;
        }
        let truth = case.value()["truth"].as_str().unwrap();
        let asked = if asked {
            "fit asked"
        } else {
            "placement_not_unique"
        };
        *tally
            .entry(format!("candidates ({truth}): {asked}"))
            .or_default() += 1;
    }
    let (expected, named) = section_5_6();
    assert_eq!(tally, expected);
    free.sort();
    assert_eq!(free, named);
    assert!(stated > 40, "{stated} cases");
    assert!(misstated.is_empty(), "{}", misstated.join("\n"));
}

fn seed_name(seed: &Seed) -> &'static str {
    match seed {
        Seed::AxisPair { .. } => "axis_pair",
        Seed::OneAxis { .. } => "one_axis",
        Seed::Cone { .. } => "cone",
        Seed::Landmark { .. } => "landmark",
        _ => "given",
    }
}

/// Section 5.6's counts, and which socket results are free: the
/// axisymmetric washers, bearing, tube and screw excused, the housing and
/// the block not.
fn section_5_6() -> (BTreeMap<String, u32>, Vec<(String, &'static str)>) {
    let counts = [
        ("axis_pair: unique", 15),
        ("one_axis: unique", 11),
        ("one_axis: free turn, axisymmetric", 5),
        ("one_axis: placement_not_unique", 2),
        ("candidates (fits): fit asked", 26),
        ("candidates (fits): placement_not_unique", 2),
        ("candidates (collides): fit asked", 3),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v))
    .collect();
    let excused = "free turn, axisymmetric";
    let named = [
        ("m_bearing04 socket-1", excused),
        ("m_housing04 socket-1", "placement_not_unique"),
        ("m_s21_block socket-1", "placement_not_unique"),
        ("m_screw02 socket-1", excused),
        ("m_tube50 socket-1", excused),
        ("m_washer05 socket-1", excused),
        ("m_washer05 socket-2", excused),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v))
    .collect();
    (counts, named)
}
