//! Review and read-back of socket documents (`docs/research/m3-socket.md`,
//! 9.4 and 9.5): what `from_json` refuses, what a decision may do, and
//! what carries over to a document extracted again.

use fitment_core::{ReviewError, ReviewStatus, SocketDocument, SocketFormatError, StaleReason};
use serde_json::Value;

const S01: &[u8] = include_bytes!("assemblies/s01_bolted.stp");
const S03: &[u8] = include_bytes!("assemblies/s03_dowels.stp");

/// The line placing s01's first bolt at (-30, -15, 8).
const BOLT: &str = "#24 = CARTESIAN_POINT('',(-30.,-15.,8.));";

fn socket(step: &[u8], part: &str) -> SocketDocument {
    SocketDocument::extract(step, Some(part), None).unwrap()
}

/// s01 with its first bolt moved 0.1 mm along x: its bracket hole is now
/// offset, and the bolts no longer all share one socket.
fn moved_bolt() -> Vec<u8> {
    let text = String::from_utf8(S01.to_vec()).unwrap();
    assert!(text.contains(BOLT));
    text.replace(BOLT, "#24 = CARTESIAN_POINT('',(-29.9,-15.,8.));")
        .into_bytes()
}

/// `document` as JSON, edited by `edit`, read back.
fn edited(
    document: &SocketDocument,
    edit: impl FnOnce(&mut Value),
) -> Result<SocketDocument, SocketFormatError> {
    let mut value: Value = serde_json::from_str(&document.to_json()).unwrap();
    edit(&mut value);
    SocketDocument::from_json(&value.to_string())
}

#[test]
fn a_changed_value_constant_or_requirement_breaks_the_digest() {
    let document = socket(S01, "BRACKET");
    let changed = edited(&document, |v| {
        let spans = &mut v["extracted"]["sockets"][0]["requirements"][3]["requires"]["spans"][0];
        spans["clear_diameter_mm"] = Value::from(6.1);
    });
    assert_eq!(changed.err(), Some(SocketFormatError::Digest));
    let constant = edited(&document, |v| {
        v["extraction"]["socket"]["contact_mm"] = Value::from(0.02);
    });
    assert_eq!(constant.err(), Some(SocketFormatError::Digest));
    let deleted = edited(&document, |v| {
        v["extracted"]["sockets"][0]["requirements"]
            .as_array_mut()
            .unwrap()
            .remove(3);
        v["review"].as_array_mut().unwrap().remove(3);
    });
    assert_eq!(deleted.err(), Some(SocketFormatError::Digest));
}

#[test]
fn a_review_that_breaks_its_rules_is_refused() {
    let document = socket(S01, "BRACKET");
    let invalid = |result: Result<SocketDocument, SocketFormatError>| {
        matches!(result, Err(SocketFormatError::Invalid(_)))
    };
    // A removal without a note.
    assert!(invalid(edited(&document, |v| {
        v["review"][0]["status"] = Value::from("removed");
    })));
    // An entry for a requirement the document does not hold.
    assert!(invalid(edited(&document, |v| {
        v["review"][0]["requirement"] = Value::from("bore:hole-1");
    })));
    // An entry twice.
    assert!(invalid(edited(&document, |v| {
        let first = v["review"][0].clone();
        v["review"].as_array_mut().unwrap().push(first);
    })));
    // A note past 2,000 characters.
    assert!(invalid(edited(&document, |v| {
        v["review"][0]["status"] = Value::from("kept");
        v["review"][0]["note"] = Value::from("x".repeat(2001));
    })));
    // A missing entry reads as unreviewed.
    let missing = edited(&document, |v| {
        v["review"].as_array_mut().unwrap().remove(0);
    });
    assert!(missing.is_ok());
}

#[test]
fn a_decision_names_a_requirement_and_a_removal_has_a_note() {
    let mut document = socket(S01, "BRACKET");
    assert_eq!(
        document.decide("bore:hole-1", None, ReviewStatus::Kept, None),
        Err(ReviewError::UnknownId("bore:hole-1".to_owned()))
    );
    let grip = document.extracted().unwrap().sockets[0].requirements[1]
        .id
        .clone();
    assert!(grip.starts_with("grip:"));
    assert_eq!(
        document.decide(&grip, None, ReviewStatus::Removed, Some(" ")),
        Err(ReviewError::NoteRequired(grip.clone()))
    );
    assert_eq!(
        document.decide(
            &grip,
            None,
            ReviewStatus::Removed,
            Some("the bracket may be thicker")
        ),
        Ok(1)
    );
    // The decision is outside the digest: the document reads back.
    let back = SocketDocument::from_json(&document.to_json()).unwrap();
    let entry = back
        .review()
        .iter()
        .find(|e| e.requirement == grip)
        .unwrap();
    assert_eq!(entry.status, ReviewStatus::Removed);
}

/// s07b's lesson on s03: the dowels' shafts differ between sockets, so
/// removing one by id alone must name the socket.
#[test]
fn a_removal_whose_requirement_differs_between_sockets_names_one() {
    let mut document = socket(S03, "DOWEL");
    let extracted = document.extracted().unwrap();
    assert!(extracted.sockets.len() > 1);
    let shaft = extracted.sockets[0]
        .requirements
        .iter()
        .find(|r| r.id.starts_with("shaft:"))
        .unwrap()
        .id
        .clone();
    let result = document.decide(&shaft, None, ReviewStatus::Removed, Some("press fit"));
    assert!(
        matches!(result, Err(ReviewError::DiffersBetweenSockets { .. })),
        "{result:?}"
    );
    assert_eq!(
        document.decide(
            &shaft,
            Some("socket-1"),
            ReviewStatus::Removed,
            Some("press fit")
        ),
        Ok(1)
    );
}

#[test]
fn decisions_carry_over_to_the_same_requirements() {
    let mut old = socket(S01, "BRACKET");
    let grip = old.extracted().unwrap().sockets[0].requirements[1]
        .id
        .clone();
    old.decide(&grip, None, ReviewStatus::Removed, Some("thicker is fine"))
        .unwrap();
    let mut new = socket(S01, "BRACKET");
    let outcome = new.carry_over(&old);
    assert_eq!((outcome.carried, outcome.stale), (1, 0));
    let entry = new.review().iter().find(|e| e.requirement == grip).unwrap();
    assert_eq!(entry.status, ReviewStatus::Removed);
    assert_eq!(entry.note.as_deref(), Some("thicker is fine"));
}

#[test]
fn a_decision_on_a_changed_requirement_goes_stale() {
    // The bracket's hole under the moved bolt now has an offset occupant.
    let mut old = socket(S01, "BRACKET");
    let hole = old.extracted().unwrap().sockets[0]
        .requirements
        .iter()
        .find(|r| r.id.starts_with("bore:"))
        .unwrap()
        .id
        .clone();
    old.decide(&hole, None, ReviewStatus::Kept, None).unwrap();
    let mut new = socket(&moved_bolt(), "BRACKET");
    let outcome = new.carry_over(&old);
    assert_eq!((outcome.carried, outcome.stale), (0, 1));
    assert_eq!(
        new.stale_reviews()[0].reason,
        StaleReason::RequirementChanged
    );
    let entry = new.review().iter().find(|e| e.requirement == hole).unwrap();
    assert_eq!(entry.status, ReviewStatus::Unreviewed);
}

#[test]
fn decisions_follow_sockets_that_split_and_stop_where_they_merge() {
    // One socket for four bolts; then one bolt moves and splits off.
    let mut merged = socket(S01, "BOLT");
    assert_eq!(merged.extracted().unwrap().sockets.len(), 1);
    let seat = merged.extracted().unwrap().sockets[0]
        .requirements
        .iter()
        .find(|r| r.id.starts_with("seat:"))
        .unwrap()
        .id
        .clone();
    merged
        .decide(&seat, None, ReviewStatus::Kept, None)
        .unwrap();
    let mut split = socket(&moved_bolt(), "BOLT");
    let sockets = split.extracted().unwrap().sockets.len();
    assert_eq!(sockets, 2);
    let outcome = split.carry_over(&merged);
    // Every occurrence of each new socket had the decision, and the seat is
    // the same in both: it carries to both.
    assert_eq!((outcome.carried, outcome.stale), (2, 0));
    // Back the other way: the two sockets' decisions disagree, so the
    // merged socket's seat stays unreviewed.
    split
        .decide(
            &seat,
            Some("socket-1"),
            ReviewStatus::Removed,
            Some("not needed"),
        )
        .unwrap();
    let mut again = socket(S01, "BOLT");
    let outcome = again.carry_over(&split);
    assert_eq!(outcome.carried, 0);
    assert!(
        again
            .stale_reviews()
            .iter()
            .any(|s| s.reason == StaleReason::OccurrencesDisagree)
    );
}
