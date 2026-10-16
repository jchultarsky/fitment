//! What a catalog stores, judged (`docs/research/m5-catalog.md`, section 5,
//! QR3, QR7, IX4 to IX9): a stored features document is judged exactly as
//! the file it was read from, and refused when another extractor wrote it;
//! a file run's fit request keeps the file's bytes; level-1 tokens read
//! back as written; and a matcher's admitted candidates hold whatever level
//! 2 keeps in every socket.

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
    let source = Source::new(
        &sha256_hex(bytes),
        u64::try_from(bytes.len()).unwrap(),
        read.header().cloned(),
    )
    .unwrap();
    (back, source)
}

/// QR7: judged from its stored document, a candidate gets the result its
/// file gets, every field, against every committed socket.
#[test]
fn a_stored_document_is_judged_as_its_file() {
    for document in sockets() {
        let matcher = Matcher::new(
            &document,
            MatchPolicy::default(),
            MatchOptions::default(),
            &NoFitVerifier,
        )
        .unwrap();
        for bytes in CANDIDATES {
            let (doc, source) = stored(bytes);
            let from_store: CandidateResult = matcher.judge_document(&doc, source).unwrap();
            assert_eq!(from_store, matcher.judge_step(bytes));
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

/// IX9: tokens read back as written, post every diameter, and are refused
/// when a pair names a stack there is not or a length is not finite.
#[test]
fn tokens_read_back_as_written() {
    for bytes in CANDIDATES {
        let read = FeaturesDocument::read(bytes);
        let tokens = IndexTokens::of(&PartSignature::of(read.part().unwrap()));
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
}

/// QR3: a matcher admits, from an index of every committed candidate,
/// every candidate level 2 keeps in each of its sockets.
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
        let Ok(Admitted::Parts(admitted)) = matcher.admitted(&index) else {
            panic!("a committed socket blocks every query");
        };
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
