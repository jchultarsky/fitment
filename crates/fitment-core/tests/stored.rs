//! What a catalog stores, judged (`docs/research/m5-catalog.md`, section 5,
//! QR3, QR7, IX4 to IX9): a stored features document is judged exactly as
//! the file it was read from, and refused when another extractor wrote it;
//! a file run's fit request keeps the file's bytes; level-1 tokens read
//! back as written; and a matcher's admitted candidates hold whatever level
//! 2 keeps in every socket, whatever else the index holds.

use std::sync::Mutex;

use fitment_core::{
    Admitted, CandidateResult, FeaturesDocument, FitRequest, FitStatus, FitVerifier, IndexTokens,
    MatchOptions, MatchPolicy, Matcher, NoFitVerifier, PartSignature, ShortlistIndex,
    SocketDocument, SocketSignature, SocketView, Source, sha256_hex, shortlist,
};

const ASSEMBLIES: [(&[u8], &str); 3] = [
    (include_bytes!("assemblies/s01_bolted.stp"), "BRACKET"),
    (include_bytes!("assemblies/s02_cbore.stp"), "BLOCK"),
    (include_bytes!("assemblies/s03_dowels.stp"), "PLATE"),
];

const CANDIDATES: [&[u8]; 6] = [
    include_bytes!("candidates/m_bracket.stp"),
    include_bytes!("candidates/m_bracket_blind.stp"),
    include_bytes!("candidates/m_bracket_dia_small.stp"),
    include_bytes!("candidates/m_block02.stp"),
    include_bytes!("candidates/m_block02_cb_deep.stp"),
    include_bytes!("candidates/m_plate03.stp"),
];

/// Every committed part file: the unit set and the candidates, by name.
const INPUTS: [(&str, &[u8]); 37] = [
    (
        "data/p01_through.stp",
        include_bytes!("data/p01_through.stp"),
    ),
    (
        "data/p03_blind_flat.stp",
        include_bytes!("data/p03_blind_flat.stp"),
    ),
    (
        "data/p04_blind_drill.stp",
        include_bytes!("data/p04_blind_drill.stp"),
    ),
    ("data/p05_cbore.stp", include_bytes!("data/p05_cbore.stp")),
    ("data/p06_csink.stp", include_bytes!("data/p06_csink.stp")),
    ("data/p09_pin.stp", include_bytes!("data/p09_pin.stp")),
    (
        "data/p10_boss_hole.stp",
        include_bytes!("data/p10_boss_hole.stp"),
    ),
    (
        "data/p11_fillet_slot.stp",
        include_bytes!("data/p11_fillet_slot.stp"),
    ),
    (
        "data/p12_edge_notch.stp",
        include_bytes!("data/p12_edge_notch.stp"),
    ),
    (
        "data/p14s_rotated_cbore.stp",
        include_bytes!("data/p14s_rotated_cbore.stp"),
    ),
    ("data/p16_inch.stp", include_bytes!("data/p16_inch.stp")),
    (
        "data/p20_split_surfaces.stp",
        include_bytes!("data/p20_split_surfaces.stp"),
    ),
    ("data/p21_planes.stp", include_bytes!("data/p21_planes.stp")),
    (
        "data/p22_coaxial_blind.stp",
        include_bytes!("data/p22_coaxial_blind.stp"),
    ),
    ("data/p25_angled.stp", include_bytes!("data/p25_angled.stp")),
    (
        "data/p26_all_bspline.stp",
        include_bytes!("data/p26_all_bspline.stp"),
    ),
    (
        "data/p30_round_bore.stp",
        include_bytes!("data/p30_round_bore.stp"),
    ),
    (
        "data/p32_torus_cbore.stp",
        include_bytes!("data/p32_torus_cbore.stp"),
    ),
    (
        "data/r03_two_solids.stp",
        include_bytes!("data/r03_two_solids.stp"),
    ),
    (
        "data/r04_assembly.stp",
        include_bytes!("data/r04_assembly.stp"),
    ),
    (
        "data/r05_no_length_unit.stp",
        include_bytes!("data/r05_no_length_unit.stp"),
    ),
    ("data/r06_shell.stp", include_bytes!("data/r06_shell.stp")),
    (
        "data/r12_flipped_cylinder.stp",
        include_bytes!("data/r12_flipped_cylinder.stp"),
    ),
    (
        "data/u01_undercut_floor.stp",
        include_bytes!("data/u01_undercut_floor.stp"),
    ),
    (
        "candidates/m_block02.stp",
        include_bytes!("candidates/m_block02.stp"),
    ),
    (
        "candidates/m_block02_cb_deep.stp",
        include_bytes!("candidates/m_block02_cb_deep.stp"),
    ),
    (
        "candidates/m_bracket.stp",
        include_bytes!("candidates/m_bracket.stp"),
    ),
    (
        "candidates/m_bracket_blind.stp",
        include_bytes!("candidates/m_bracket_blind.stp"),
    ),
    (
        "candidates/m_bracket_dia_small.stp",
        include_bytes!("candidates/m_bracket_dia_small.stp"),
    ),
    (
        "candidates/m_bracket_lug.stp",
        include_bytes!("candidates/m_bracket_lug.stp"),
    ),
    (
        "candidates/m_bracket_missing_hole.stp",
        include_bytes!("candidates/m_bracket_missing_hole.stp"),
    ),
    (
        "candidates/m_bracket_thick.stp",
        include_bytes!("candidates/m_bracket_thick.stp"),
    ),
    (
        "candidates/m_bracket_top_pocket.stp",
        include_bytes!("candidates/m_bracket_top_pocket.stp"),
    ),
    (
        "candidates/m_nut_pinned_15tol.stp",
        include_bytes!("candidates/m_nut_pinned_15tol.stp"),
    ),
    (
        "candidates/m_plate03.stp",
        include_bytes!("candidates/m_plate03.stp"),
    ),
    (
        "candidates/m_plate03_pin_shift_tol.stp",
        include_bytes!("candidates/m_plate03_pin_shift_tol.stp"),
    ),
    (
        "candidates/m_plate03_press_15tol.stp",
        include_bytes!("candidates/m_plate03_press_15tol.stp"),
    ),
];

fn sockets() -> Vec<SocketDocument> {
    ASSEMBLIES
        .iter()
        .map(|(bytes, part)| SocketDocument::extract(bytes, Some(part), None).unwrap())
        .collect()
}

/// The document a catalog stores for `bytes`, read back from its compact
/// JSON, with the source built from the file's own SHA-256.
fn stored(bytes: &[u8]) -> (FeaturesDocument, Source) {
    let read = FeaturesDocument::read(bytes);
    let back = FeaturesDocument::from_json(&read.to_compact_json()).unwrap();
    assert_eq!(back, read, "the compact form reads back equal");
    assert_eq!(back.to_json(), read.to_json(), "and writes back the same");
    let source = Source::new(
        &sha256_hex(bytes),
        u64::try_from(bytes.len()).unwrap(),
        read.header().cloned(),
    )
    .unwrap();
    (back, source)
}

/// QR7 and CT5: judged from its stored document, every committed part
/// file, refused ones included, gets the result its file gets, every
/// field, against every committed socket.
#[test]
fn a_stored_document_is_judged_as_its_file() {
    let stored: Vec<_> = INPUTS.iter().map(|(_, bytes)| stored(bytes)).collect();
    for document in sockets() {
        let matcher = Matcher::new(
            &document,
            MatchPolicy::default(),
            MatchOptions::default(),
            &NoFitVerifier,
        )
        .unwrap();
        for ((name, bytes), (doc, source)) in INPUTS.iter().zip(&stored) {
            let from_store: CandidateResult = matcher.judge_document(doc, source.clone()).unwrap();
            assert_eq!(from_store, matcher.judge_step(bytes), "{name}");
        }
    }
}

/// QR7: a document another extractor version wrote is not judged as this
/// build's read.
#[test]
fn a_document_of_another_extractor_is_not_judged() {
    let document = &sockets()[0];
    let matcher = Matcher::new(
        document,
        MatchPolicy::default(),
        MatchOptions::default(),
        &NoFitVerifier,
    )
    .unwrap();
    let (doc, source) = stored(CANDIDATES[0]);
    let mut value: serde_json::Value = serde_json::from_str(&doc.to_compact_json()).unwrap();
    let older = value["extractor_version"].as_u64().unwrap() - 1;
    value["extractor_version"] = older.into();
    let stale = FeaturesDocument::from_json(&value.to_string()).unwrap();
    let refused = matcher.judge_document(&stale, source).unwrap_err();
    assert_eq!(u64::from(refused.extractor_version), older);
}

/// Records whether each fit request carried the candidate's bytes.
struct Recording(Mutex<Vec<bool>>);

impl FitVerifier for Recording {
    fn check(&self, request: &FitRequest<'_>) -> FitStatus {
        self.0
            .lock()
            .unwrap()
            .push(request.candidate_step.is_some());
        FitStatus::no_verifier()
    }
}

/// The M5 design review's F5: a file run's fit request keeps the file's
/// bytes, so M7's verifier can read them; a stored document's has none.
#[test]
fn judge_step_hands_its_bytes_to_the_fit_request() {
    let document = &sockets()[0];
    let verifier = Recording(Mutex::new(Vec::new()));
    let matcher = Matcher::new(
        document,
        MatchPolicy::default(),
        MatchOptions::default(),
        &verifier,
    )
    .unwrap();
    let bracket = CANDIDATES[0];
    let _ = matcher.judge_step(bracket);
    assert_eq!(verifier.0.lock().unwrap().as_slice(), [true]);
    let (doc, source) = stored(bracket);
    let _ = matcher.judge_document(&doc, source);
    assert_eq!(verifier.0.lock().unwrap().as_slice(), [true, false]);
}

/// IX9: tokens read back as written and post every diameter; they are
/// refused when a pair names a stack there is not, a length is negative,
/// `-0.0` or not finite, or the pairs are out of order.
#[test]
fn tokens_read_back_as_written() {
    for (_, bytes) in INPUTS {
        let read = FeaturesDocument::read(bytes);
        let Some(part) = read.part() else {
            continue;
        };
        let tokens = IndexTokens::of(&PartSignature::of(part));
        assert_eq!(IndexTokens::from_json(&tokens.to_json()).unwrap(), tokens);
        let diameters: usize = tokens.stacks.iter().map(|(_, d)| d.len()).sum();
        assert_eq!(tokens.postings().count(), diameters);
    }
    let read = FeaturesDocument::read(CANDIDATES[0]);
    let tokens = IndexTokens::of(&PartSignature::of(read.part().unwrap()));
    assert!(!tokens.pairs.is_empty(), "the bracket's holes pair");
    let mut value: serde_json::Value = serde_json::from_str(&tokens.to_json()).unwrap();
    value["pairs"][0]["b"] = 999.into();
    assert!(IndexTokens::from_json(&value.to_string()).is_err());
    let mut value: serde_json::Value = serde_json::from_str(&tokens.to_json()).unwrap();
    value["stacks"][0][1][0] = (-6.6).into();
    assert!(IndexTokens::from_json(&value.to_string()).is_err());
    let mut value: serde_json::Value = serde_json::from_str(&tokens.to_json()).unwrap();
    value["unknown"] = 1.into();
    assert!(IndexTokens::from_json(&value.to_string()).is_err());
    let mut value: serde_json::Value = serde_json::from_str(&tokens.to_json()).unwrap();
    value["stacks"][0][1][0] = serde_json::json!(-0.0);
    assert!(IndexTokens::from_json(&value.to_string()).is_err());
    let mut value: serde_json::Value = serde_json::from_str(&tokens.to_json()).unwrap();
    value["lever_mm"] = serde_json::json!(-0.0);
    assert!(IndexTokens::from_json(&value.to_string()).is_err());
    let mut value: serde_json::Value = serde_json::from_str(&tokens.to_json()).unwrap();
    let pairs = value["pairs"].as_array_mut().unwrap();
    assert!(pairs.len() > 1, "the bracket's holes pair both ways");
    pairs.swap(0, 1);
    assert!(IndexTokens::from_json(&value.to_string()).is_err());
}

/// QR3: a matcher admits, from an index of every committed candidate,
/// every candidate level 2 keeps in each of its sockets; and IX6: what it
/// admits of them does not change when the index also holds parts of a
/// far larger lever.
#[test]
fn admitted_holds_what_level_two_keeps() {
    let parts: Vec<PartSignature> = CANDIDATES
        .iter()
        .map(|b| PartSignature::of(FeaturesDocument::read(b).part().unwrap()))
        .collect();
    let mut index = ShortlistIndex::new();
    for (id, part) in (0_u64..).zip(&parts) {
        index.insert(id, &IndexTokens::of(part));
    }
    let mut crowded = index.clone();
    for (id, part) in (1000_u64..).zip(&parts) {
        let mut tokens: serde_json::Value =
            serde_json::from_str(&IndexTokens::of(part).to_json()).unwrap();
        tokens["lever_mm"] = 5000.0.into();
        crowded.insert(id, &IndexTokens::from_json(&tokens.to_string()).unwrap());
    }
    let mut kept_somewhere = 0;
    for document in sockets() {
        let policy = MatchPolicy::default();
        let matcher = Matcher::new(
            &document,
            policy.clone(),
            MatchOptions::default(),
            &NoFitVerifier,
        )
        .unwrap();
        let answer = matcher.admitted(&index).unwrap();
        let json = serde_json::to_value(&answer).unwrap();
        assert_eq!(json["admitted"], "parts", "{json}");
        let Admitted::Parts {
            candidates: admitted,
            ..
        } = answer
        else {
            panic!("a committed socket blocks every query");
        };
        assert_eq!(json["candidates"], serde_json::json!(admitted));
        let Ok(Admitted::Parts {
            candidates: among_more,
            ..
        }) = matcher.admitted(&crowded)
        else {
            panic!("a committed socket blocks every query");
        };
        let ours: Vec<u64> = among_more.into_iter().filter(|id| *id < 1000).collect();
        assert_eq!(ours, admitted, "the other parts change what is admitted");
        let signatures: Vec<SocketSignature> = document
            .extracted()
            .unwrap()
            .sockets
            .iter()
            .map(|s| SocketSignature::of(&SocketView::new(&document, &s.id).unwrap(), &policy))
            .collect();
        for (id, part) in (0_u64..).zip(&parts) {
            if signatures.iter().all(|s| shortlist(s, part).is_ok()) {
                kept_somewhere += 1;
                assert!(
                    admitted.contains(&id),
                    "level 2 keeps {id}, level 1 does not"
                );
            }
        }
    }
    assert!(
        kept_somewhere > 0,
        "no candidate kept: the test shows nothing"
    );
}
