//! The signature and the shortlist (`docs/research/m4-match.md`, CF, SG,
//! SL) on the committed assemblies and parts: the original is kept by its
//! own sockets under every policy, a part that lacks what a socket demands
//! is dropped with what it lacks, and level 1's index returns whatever
//! level 2 keeps.

use fitment_core::{
    Clearance, ComponentRead, FeaturesDocument, Grip, IndexTokens, MatchPolicy, MissingElement,
    Part, PartSignature, ShortlistIndex, SocketDocument, SocketSignature, SocketView,
    TolerancePolicy, read_assembly, shortlist,
};

const ASSEMBLIES: [(&str, &[u8], &[&str]); 4] = [
    (
        "s01",
        include_bytes!("assemblies/s01_bolted.stp"),
        &["BRACKET", "PLATE", "BOLT", "NUT"],
    ),
    (
        "s02",
        include_bytes!("assemblies/s02_cbore.stp"),
        &["BLOCK", "BASE", "SCREW"],
    ),
    (
        "s03",
        include_bytes!("assemblies/s03_dowels.stp"),
        &["PLATE", "DOWEL"],
    ),
    (
        "s08x",
        include_bytes!("assemblies/s08x_nested.stp"),
        &["BRACKET", "BOLT"],
    ),
];

/// Part `name` of an assembly, as a candidate reads it.
fn component(bytes: &[u8], name: &str) -> Part {
    let read = read_assembly(bytes);
    let listing = read.listing().unwrap();
    let selection = listing.select(Some(name), None).unwrap();
    match &listing.definitions()[selection.definition].read {
        Some(ComponentRead::Accepted(part)) => (**part).clone(),
        other => panic!("{name} is not read: {other:?}"),
    }
}

fn policies() -> Vec<MatchPolicy> {
    let tolerances = [
        TolerancePolicy::default(),
        TolerancePolicy::new("fine", 0.002, 0.002).unwrap(),
        TolerancePolicy::new("coarse", 0.1, 0.1).unwrap(),
    ];
    tolerances
        .iter()
        .flat_map(|t| {
            [Clearance::Strict, Clearance::Functional]
                .map(|c| MatchPolicy::new(t.clone(), c, Grip::Hard))
        })
        .collect()
}

/// Every socket of every committed target, under every policy.
fn sockets() -> Vec<(String, SocketDocument)> {
    let mut out = Vec::new();
    for (case, bytes, parts) in ASSEMBLIES {
        for part in parts {
            let document = SocketDocument::extract(bytes, Some(part), None).unwrap();
            out.push((format!("{case} {part}"), document));
        }
    }
    out
}

/// Section 6.4 on the originals: each part's own sockets keep it.
#[test]
fn the_original_is_kept_by_its_own_sockets() {
    let mut failures = Vec::new();
    for (case, bytes, parts) in ASSEMBLIES {
        for part in parts {
            let original = PartSignature::of(&component(bytes, part));
            let document = SocketDocument::extract(bytes, Some(part), None).unwrap();
            for socket in &document.extracted().unwrap().sockets {
                let view = SocketView::new(&document, &socket.id).unwrap();
                for policy in policies() {
                    let signature = SocketSignature::of(&view, &policy);
                    match shortlist(&signature, &original) {
                        // SL6: every requirement keeps a witness to align on.
                        Ok(kept) if kept.domains.iter().all(|(_, d)| !d.is_empty()) => {}
                        other => failures.push(format!(
                            "{case} {part} {} {:?}: {other:?}",
                            socket.id,
                            policy.tolerance().name()
                        )),
                    }
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// A part with none of what a socket demands is dropped, saying so; and
/// the drop names a requirement of that socket.
#[test]
fn a_part_lacking_the_socket_s_elements_is_dropped() {
    let plain = FeaturesDocument::read(include_bytes!("data/p21_planes.stp"));
    let plain = PartSignature::of(plain.part().unwrap());
    let bolt = PartSignature::of(&component(ASSEMBLIES[0].1, "BOLT"));
    let document = SocketDocument::extract(ASSEMBLIES[0].1, Some("BRACKET"), None).unwrap();
    let view = SocketView::new(&document, "socket-1").unwrap();
    let signature = SocketSignature::of(&view, &MatchPolicy::default());
    for candidate in [&plain, &bolt] {
        let missing = shortlist(&signature, candidate).unwrap_err();
        let ids: Vec<String> = view.active().iter().map(|r| r.id.clone()).collect();
        assert!(
            missing
                .iter()
                .flat_map(|m| m.requirements.iter())
                .all(|r| ids.contains(&r.as_str().to_owned()))
        );
        assert_ne!(missing.len(), 0);
    }
    // The bracket's bores want holes: the bolt has only a shaft.
    let missing = shortlist(&signature, &bolt).unwrap_err();
    assert!(matches!(
        missing[0].element,
        MissingElement::Diameter | MissingElement::Openings | MissingElement::Extent
    ));
}

/// SL9: level 1 returns whatever level 2 keeps, over every committed part
/// and assembly component against every committed socket.
#[test]
fn level_one_returns_whatever_level_two_keeps() {
    let mut pool: Vec<PartSignature> = Vec::new();
    for (_, bytes, parts) in ASSEMBLIES {
        pool.extend(
            parts
                .iter()
                .map(|p| PartSignature::of(&component(bytes, p))),
        );
    }
    let parts: [&[u8]; 8] = [
        include_bytes!("data/p01_through.stp"),
        include_bytes!("data/p03_blind_flat.stp"),
        include_bytes!("data/p05_cbore.stp"),
        include_bytes!("data/p06_csink.stp"),
        include_bytes!("data/p09_pin.stp"),
        include_bytes!("data/p10_boss_hole.stp"),
        include_bytes!("data/p21_planes.stp"),
        include_bytes!("data/p22_coaxial_blind.stp"),
    ];
    for bytes in parts {
        let read = FeaturesDocument::read(bytes);
        pool.push(PartSignature::of(read.part().unwrap()));
    }
    let mut index = ShortlistIndex::new();
    for (id, part) in (0_u64..).zip(&pool) {
        index.insert(id, &IndexTokens::of(part));
    }
    let mut kept = 0;
    for (case, document) in sockets() {
        for socket in &document.extracted().unwrap().sockets {
            let view = SocketView::new(&document, &socket.id).unwrap();
            for policy in policies() {
                let signature = SocketSignature::of(&view, &policy);
                let Ok(found) = index.query(&signature) else {
                    continue;
                };
                for (id, part) in (0_u64..).zip(&pool) {
                    if shortlist(&signature, part).is_ok() {
                        kept += 1;
                        assert!(
                            found.contains(&id),
                            "{case} {}: level 2 keeps {id}, level 1 does not",
                            socket.id
                        );
                    }
                }
            }
        }
    }
    assert!(kept > 0);
}

/// SG7: a requirement every candidate fails is listed, not demanded; so is
/// one removed in review.
#[test]
fn blocked_and_removed_requirements_demand_nothing() {
    let mut document = SocketDocument::extract(ASSEMBLIES[0].1, Some("BRACKET"), None).unwrap();
    let all = {
        let view = SocketView::new(&document, "socket-1").unwrap();
        SocketSignature::of(&view, &MatchPolicy::default())
    };
    assert_eq!(all.blocked_by().len(), 0);
    // A tolerance below the target's own uncertainty blocks everything.
    let tiny = MatchPolicy::new(
        TolerancePolicy::new("tiny", 1e-7, 0.01).unwrap(),
        Clearance::Strict,
        Grip::Hard,
    );
    let view = SocketView::new(&document, "socket-1").unwrap();
    let blocked = SocketSignature::of(&view, &tiny);
    assert_eq!(blocked.blocked_by().len(), view.active().len());
    assert!(blocked.is_empty());
    document
        .decide(
            "grip:plane-467+plane-570",
            None,
            fitment_core::ReviewStatus::Removed,
            Some("may be thicker"),
        )
        .unwrap();
    let view = SocketView::new(&document, "socket-1").unwrap();
    let reviewed = SocketSignature::of(&view, &MatchPolicy::default());
    assert_eq!(reviewed.removed().len(), 1);
}
