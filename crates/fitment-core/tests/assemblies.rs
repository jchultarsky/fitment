//! Reading assemblies (`docs/research/m3-socket.md`, sections 3 to 6).
//!
//! `tests/assemblies` holds the assemblies `tools/gen-corpus.py
//! --assembly-set` writes and the manifest with their answers, which come
//! from the construction alone. What no generated assembly can reach is
//! written here as small STEP snippets: a cycle, a diamond past the
//! occurrence limit, roots, material in an assembly's own shape, and paths
//! that read two ways.

use std::fmt::Write as _;

use fitment_core::{
    DefinitionKind, MatchedBy, PartsDocument, PathForm, RefusalCode, SelectError, read_assembly,
};
use serde_json::Value;

#[path = "support/assembly.rs"]
mod assembly;

const MANIFEST: &str = include_str!("assemblies/manifest.json");

macro_rules! assemblies {
    ($($name:literal),* $(,)?) => {
        &[$(($name, include_bytes!(concat!("assemblies/", $name, ".stp")) as &[u8])),*]
    };
}

const ASSEMBLIES: &[(&str, &[u8])] =
    assemblies!["s01_bolted", "s02_cbore", "s03_dowels", "s08x_nested"];

#[test]
fn every_committed_assembly_reads_as_its_construction_says() {
    let manifest: Value = serde_json::from_str(MANIFEST).unwrap();
    let cases = manifest["cases"].as_array().unwrap();
    let listed: Vec<&str> = cases.iter().map(|c| c["id"].as_str().unwrap()).collect();
    let included: Vec<&str> = ASSEMBLIES.iter().map(|(name, _)| *name).collect();
    assert_eq!(listed, included, "the manifest and the files included here");
    let mut failures = Vec::new();
    for (case, (name, bytes)) in cases.iter().zip(ASSEMBLIES) {
        assert_eq!(case["kind"], "assembly");
        failures.extend(
            assembly::check(case, bytes)
                .into_iter()
                .map(|f| format!("{name}: {f}")),
        );
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// A small assembly in STEP: definitions `names` (product id and name the
/// same), each with a shape representation holding one frame, and usages
/// `(parent, child, id)`, each placed at the identity by an
/// `item_defined_transformation`. `extra` is added to the data section,
/// and `extra_items[i]` to definition i's representation.
fn snippet(
    names: &[&str],
    usages: &[(usize, usize, &str)],
    extra: &str,
    extra_items: &[(usize, u64)],
) -> String {
    let mut text = String::from(
        "ISO-10303-21;HEADER;FILE_DESCRIPTION((''),'2;1');FILE_NAME('','',(''),(''),'','','');\
         FILE_SCHEMA(('AUTOMOTIVE_DESIGN'));ENDSEC;DATA;\n\
         #1=(LENGTH_UNIT()NAMED_UNIT(*)SI_UNIT(.MILLI.,.METRE.));\n\
         #2=(GEOMETRIC_REPRESENTATION_CONTEXT(3)GLOBAL_UNIT_ASSIGNED_CONTEXT((#1))\
         REPRESENTATION_CONTEXT('',''));\n\
         #3=CARTESIAN_POINT('',(0.,0.,0.));\n\
         #4=APPLICATION_CONTEXT('');\n",
    );
    for (i, name) in names.iter().enumerate() {
        let b = 100 * (i as u64 + 1);
        let items: Vec<String> = std::iter::once(format!("#{}", b + 6))
            .chain(
                extra_items
                    .iter()
                    .filter(|(d, _)| *d == i)
                    .map(|(_, item)| format!("#{item}")),
            )
            .collect();
        let _ = write!(
            text,
            "#{b}=PRODUCT('{name}','{name}','',());\n\
             #{}=PRODUCT_DEFINITION_FORMATION('','',#{b});\n\
             #{}=PRODUCT_DEFINITION('','',#{},$);\n\
             #{}=PRODUCT_DEFINITION_SHAPE('','',#{});\n\
             #{}=SHAPE_DEFINITION_REPRESENTATION(#{},#{});\n\
             #{}=SHAPE_REPRESENTATION('',({}),#2);\n\
             #{}=AXIS2_PLACEMENT_3D('',#3,$,$);\n",
            b + 1,
            b + 2,
            b + 1,
            b + 3,
            b + 2,
            b + 4,
            b + 3,
            b + 5,
            b + 5,
            items.join(","),
            b + 6,
        );
    }
    for (j, (parent, child, id)) in usages.iter().enumerate() {
        let u = 100_000 + 10 * j as u64;
        let (p, c) = (100 * (*parent as u64 + 1), 100 * (*child as u64 + 1));
        let _ = write!(
            text,
            "#{u}=NEXT_ASSEMBLY_USAGE_OCCURRENCE('{id}','','',#{},#{},$);\n\
             #{}=PRODUCT_DEFINITION_SHAPE('','',#{u});\n\
             #{}=CONTEXT_DEPENDENT_SHAPE_REPRESENTATION(#{},#{});\n\
             #{}=(REPRESENTATION_RELATIONSHIP('','',#{},#{})\
             REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION(#{})SHAPE_REPRESENTATION_RELATIONSHIP());\n\
             #{}=ITEM_DEFINED_TRANSFORMATION('','',#{},#{});\n",
            p + 2,
            c + 2,
            u + 1,
            u + 2,
            u + 3,
            u + 1,
            u + 3,
            c + 5,
            p + 5,
            u + 4,
            u + 4,
            c + 6,
            p + 6,
        );
    }
    text.push_str(extra);
    text.push_str("ENDSEC;END-ISO-10303-21;\n");
    text
}

fn codes(refusals: &[fitment_core::Refusal]) -> Vec<RefusalCode> {
    let mut codes: Vec<RefusalCode> = refusals.iter().map(|r| r.code).collect();
    codes.dedup();
    codes
}

#[test]
fn a_snippet_assembly_reads() {
    let text = snippet(
        &["ROOT", "A", "B"],
        &[(0, 1, "u1"), (1, 2, "u2"), (1, 2, "u3")],
        "",
        &[],
    );
    let read = read_assembly(text.as_bytes());
    assert_eq!(read.refusals(), &[], "{:?}", read.refusals());
    // The snippet's parts hold no solid: empty components refuse (R2), as
    // a screw the exporter left out must; the parts list stands.
    assert_eq!(codes(read.gate_refusals()), [RefusalCode::ComponentRefused]);
    assert!(read.gate_refusals()[0].message.contains("no_solid"));
    let listing = read.listing().unwrap();
    let quantities: Vec<(String, usize)> = listing
        .definitions()
        .iter()
        .map(|d| {
            (
                d.definition.product.as_ref().unwrap().id.clone().unwrap(),
                d.occurrences.len(),
            )
        })
        .collect();
    assert_eq!(
        quantities,
        [
            ("ROOT".to_owned(), 1),
            ("A".to_owned(), 1),
            ("B".to_owned(), 2)
        ]
    );
    let b = listing.select(Some("B"), None).unwrap();
    assert_eq!(b.occurrences.len(), 2);
    let paths: Vec<String> = b
        .occurrences
        .iter()
        .map(|&i| listing.occurrences()[i].path.display(PathForm::UsageId))
        .collect();
    assert_eq!(paths, ["u1/u2", "u1/u3"]);
    // Both written forms of a path read, and mixed; each names its own
    // occurrence.
    for (path, nth) in [
        ("u1/u3", 1),
        ("#100000/#100020", 1),
        ("u1/#100020", 1),
        ("u1/u2", 0),
        ("#100000/u2", 0),
    ] {
        let one = listing.select(Some("B"), Some(path)).unwrap();
        assert_eq!(one.occurrences, [b.occurrences[nth]], "{path}");
    }
}

#[test]
fn a_part_file_lists_one_part_and_is_not_an_assembly() {
    let text = snippet(&["ONLY"], &[], "", &[]);
    let read = read_assembly(text.as_bytes());
    let listing = read.listing().unwrap();
    assert_eq!(listing.definitions().len(), 1);
    assert_eq!(listing.definitions()[0].kind, DefinitionKind::Part);
    assert_eq!(listing.occurrences().len(), 1);
    assert_eq!(listing.occurrences()[0].path.usages(), &[]);
    assert_eq!(
        codes(read.gate_refusals()),
        [RefusalCode::NotAnAssembly, RefusalCode::ComponentRefused]
    );
    let document = PartsDocument::from_json(&PartsDocument::read(text.as_bytes()).to_json());
    assert!(document.is_ok(), "{document:?}");
}

#[test]
fn an_assembly_needs_exactly_one_root() {
    // A cycle with nothing above it: no root.
    let none = snippet(&["A", "B"], &[(0, 1, "u1"), (1, 0, "u2")], "", &[]);
    let read = read_assembly(none.as_bytes());
    assert!(read.listing().is_none());
    assert_eq!(codes(read.refusals()), [RefusalCode::NoRoot]);
    // An orphan beside the assembly: two roots.
    let two = snippet(&["ROOT", "A", "ORPHAN"], &[(0, 1, "u1")], "", &[]);
    let read = read_assembly(two.as_bytes());
    assert!(read.listing().is_none());
    assert_eq!(codes(read.refusals()), [RefusalCode::SeveralRoots]);
    assert_eq!(codes(read.gate_refusals()), [RefusalCode::SeveralRoots]);
}

#[test]
fn a_cycle_is_an_unreadable_placement() {
    let text = snippet(
        &["ROOT", "A", "B"],
        &[(0, 1, "u1"), (1, 2, "u2"), (2, 1, "u3")],
        "",
        &[],
    );
    let read = read_assembly(text.as_bytes());
    assert!(read.listing().is_none());
    assert_eq!(codes(read.refusals()), [RefusalCode::PlacementUnreadable]);
    let refusal = &read.refusals()[0];
    assert_eq!(
        refusal.instance,
        Some(100_020),
        "the usage that closes the cycle"
    );
    assert!(
        refusal.message.starts_with("cycle: "),
        "{}",
        refusal.message
    );
}

#[test]
fn an_unplaced_usage_is_reported_once() {
    // Two levels below an unplaced usage: one refusal, for that usage.
    let mut text = snippet(
        &["ROOT", "A", "B"],
        &[(0, 1, "u1"), (1, 2, "u2"), (1, 2, "u3")],
        "",
        &[],
    );
    text = text.replace(
        "#100002=CONTEXT_DEPENDENT_SHAPE_REPRESENTATION(#100003,#100001);\n",
        "",
    );
    let read = read_assembly(text.as_bytes());
    assert_eq!(codes(read.refusals()), [RefusalCode::PlacementUnreadable]);
    assert_eq!(read.refusals().len(), 1, "{:?}", read.refusals());
    assert_eq!(read.refusals()[0].instance, Some(100_000));
    assert!(read.refusals()[0].message.starts_with("unplaced: "));
}

#[test]
fn a_diamond_past_the_occurrence_limit_is_refused() {
    // 21 definitions, each using the next twice: 2^20 leaf occurrences
    // from 40 usages.
    let names: Vec<String> = (0..21).map(|k| format!("D{k}")).collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let ids: Vec<String> = (0..40).map(|k| format!("u{k}")).collect();
    let usages: Vec<(usize, usize, &str)> = (0..40)
        .map(|k| (k / 2, k / 2 + 1, ids[k].as_str()))
        .collect();
    let text = snippet(&names, &usages, "", &[]);
    let read = read_assembly(text.as_bytes());
    assert!(read.listing().is_none());
    assert_eq!(codes(read.refusals()), [RefusalCode::TooManyOccurrences]);
}

#[test]
fn material_in_an_assembly_s_own_shape_is_refused() {
    let text = snippet(
        &["ROOT", "A"],
        &[(0, 1, "u1")],
        "#900=OPEN_SHELL('',());\n#901=SHELL_BASED_SURFACE_MODEL('',(#900));\n",
        &[(0, 901)],
    );
    let read = read_assembly(text.as_bytes());
    // The parts list stands; the socket gate refuses.
    assert!(read.listing().is_some());
    assert_eq!(
        codes(read.gate_refusals()),
        [
            RefusalCode::AssemblyShapeItem,
            RefusalCode::ComponentRefused
        ]
    );
    assert_eq!(read.gate_refusals()[0].instance, Some(901));
    // The same item in the part's own shape is the part's affair: check
    // 36 refuses the component for it, not check 35 the assembly.
    let in_part = snippet(
        &["ROOT", "A"],
        &[(0, 1, "u1")],
        "#900=OPEN_SHELL('',());\n#901=SHELL_BASED_SURFACE_MODEL('',(#900));\n",
        &[(1, 901)],
    );
    let read = read_assembly(in_part.as_bytes());
    assert_eq!(codes(read.gate_refusals()), [RefusalCode::ComponentRefused]);
    assert!(
        read.gate_refusals()[0]
            .message
            .contains("unread_shape_item")
    );
}

#[test]
fn a_definition_instance_selects_unless_an_id_reads_the_same() {
    let text = snippet(&["ROOT", "A", "B"], &[(0, 1, "u1"), (0, 2, "u2")], "", &[]);
    let read = read_assembly(text.as_bytes());
    let listing = read.listing().unwrap();
    let a = listing.select(Some("#202"), None).unwrap();
    assert_eq!(a.matched_by, MatchedBy::DefinitionInstance);
    assert_eq!(listing.definitions()[a.definition].definition.instance, 202);
    assert!(matches!(
        listing.select(Some("#102"), None),
        Err(SelectError::PartIsAssembly { .. })
    ));
    // B's product id is literally "#202", A's definition's #id: ambiguous.
    // A definition whose own product id is its #id is not.
    let text = snippet(
        &["ROOT", "A", "#202"],
        &[(0, 1, "u1"), (0, 2, "u2")],
        "",
        &[],
    );
    let read = read_assembly(text.as_bytes());
    let listing = read.listing().unwrap();
    let error = listing.select(Some("#202"), None).unwrap_err();
    assert_eq!(error.code(), "part_ambiguous");
    assert_eq!(error.candidates().len(), 2);
    let text = snippet(
        &["ROOT", "A", "#302"],
        &[(0, 1, "u1"), (0, 2, "u2")],
        "",
        &[],
    );
    let read = read_assembly(text.as_bytes());
    let listing = read.listing().unwrap();
    let b = listing.select(Some("#302"), None).unwrap();
    assert_eq!(listing.definitions()[b.definition].definition.instance, 302);
}

#[test]
fn a_path_segment_that_names_two_usages_is_refused() {
    // Two sibling usages share the id "a": paths are written by instance,
    // and "a" is refused rather than guessed.
    let text = snippet(&["ROOT", "A"], &[(0, 1, "a"), (0, 1, "a")], "", &[]);
    let read = read_assembly(text.as_bytes());
    let listing = read.listing().unwrap();
    assert_eq!(listing.path_form(), PathForm::Instance);
    assert_eq!(
        listing.select(Some("A"), Some("a")).unwrap_err().code(),
        "occurrence_ambiguous"
    );
    assert_eq!(
        listing
            .select(Some("A"), Some("#100010"))
            .unwrap()
            .occurrences
            .len(),
        1
    );
    // A usage whose id is "#100010" while another's instance is 100010.
    let text = snippet(&["ROOT", "A"], &[(0, 1, "#100010"), (0, 1, "b")], "", &[]);
    let read = read_assembly(text.as_bytes());
    let listing = read.listing().unwrap();
    assert_eq!(
        listing.path_form(),
        PathForm::Instance,
        "an id may not begin with #"
    );
    assert_eq!(
        listing.select(None, Some("#100010")).unwrap_err().code(),
        "occurrence_ambiguous"
    );
}

#[test]
fn reading_is_deterministic_and_the_document_reads_back() {
    for (name, bytes) in ASSEMBLIES {
        let text = PartsDocument::read(bytes).to_json();
        assert_eq!(text, PartsDocument::read(bytes).to_json(), "{name}");
        let back = PartsDocument::from_json(&text).unwrap();
        assert_eq!(back.to_json(), text, "{name}");
    }
}

#[test]
fn a_parts_document_that_breaks_its_rules_is_not_read() {
    let (_, bytes) = ASSEMBLIES[0];
    let text = PartsDocument::read(bytes).to_json();
    let mut json: Value = serde_json::from_str(&text).unwrap();
    json["parts"][0]["quantity"] = Value::from(7);
    let error = PartsDocument::from_json(&json.to_string()).unwrap_err();
    assert!(error.to_string().contains("quantity"), "{error}");
    let mut json: Value = serde_json::from_str(&text).unwrap();
    json["source"]["sha256"] = Value::from("abc");
    assert!(PartsDocument::from_json(&json.to_string()).is_err());
    let mut json: Value = serde_json::from_str(&text).unwrap();
    json["parts"].as_array_mut().unwrap().reverse();
    assert!(PartsDocument::from_json(&json.to_string()).is_err());
}

#[test]
fn definitions_in_a_cycle_beside_the_root_are_refused() {
    // A lone part beside two definitions using each other: one root, yet
    // no assembly (review of M3 PR 1).
    let lone = snippet(&["R", "A", "B"], &[(1, 2, "u1"), (2, 1, "u2")], "", &[]);
    let read = read_assembly(lone.as_bytes());
    assert!(read.listing().is_none());
    assert_eq!(codes(read.refusals()), [RefusalCode::SeveralRoots]);
    // The root places P; apart from it, a cycle places Q somewhere.
    let beside = snippet(
        &["ROOT", "P", "A", "B", "Q"],
        &[(0, 1, "u1"), (2, 3, "u2"), (3, 2, "u3"), (3, 4, "u4")],
        "",
        &[],
    );
    let read = read_assembly(beside.as_bytes());
    assert_eq!(codes(read.refusals()), [RefusalCode::SeveralRoots]);
    assert!(
        read.refusals()[0]
            .message
            .contains("not reached from the root")
    );
}

#[test]
fn a_usage_stepq_cannot_read_is_refused_not_dropped() {
    // The second usage's child is not a reference: stepq skips it, which
    // would make B's quantity 1.
    let text = snippet(&["ROOT", "B"], &[(0, 1, "u1"), (0, 1, "u2")], "", &[]).replace(
        "#100010=NEXT_ASSEMBLY_USAGE_OCCURRENCE('u2','','',#102,#202,$);",
        "#100010=NEXT_ASSEMBLY_USAGE_OCCURRENCE('u2','','',#102,$,$);",
    );
    let read = read_assembly(text.as_bytes());
    assert!(read.listing().is_none());
    assert!(
        read.refusals()
            .iter()
            .any(|r| r.code == RefusalCode::PlacementUnreadable && r.instance == Some(100_010)),
        "{:?}",
        read.refusals()
    );
}

#[test]
fn long_usage_ids_on_many_paths_are_refused() {
    // 2^15 leaf occurrences, each path carrying a 1,000-byte id: within
    // the occurrence limit, past the label budget of check 34.
    let long = "x".repeat(1_000);
    let names: Vec<String> = (0..17).map(|k| format!("D{k}")).collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let ids: Vec<String> = (0..30).map(|k| format!("u{k}")).collect();
    let mut usages: Vec<(usize, usize, &str)> = vec![(0, 1, long.as_str())];
    usages.extend((0..30).map(|k| (k / 2 + 1, k / 2 + 2, ids[k].as_str())));
    let text = snippet(&names, &usages, "", &[]);
    let read = read_assembly(text.as_bytes());
    assert!(read.listing().is_none());
    assert_eq!(codes(read.refusals()), [RefusalCode::TooManyOccurrences]);
    assert!(read.refusals()[0].message.contains("bytes of usage ids"));
}

#[test]
fn the_occurrence_limit_keeps_the_usages_that_cannot_be_placed() {
    // An unplaced usage above a diamond past the limit: both reasons.
    let names: Vec<String> = (0..22).map(|k| format!("D{k}")).collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let ids: Vec<String> = (0..41).map(|k| format!("u{k}")).collect();
    let mut usages: Vec<(usize, usize, &str)> = vec![(0, 1, ids[40].as_str())];
    usages.extend((0..40).map(|k| (k / 2 + 1, k / 2 + 2, ids[k].as_str())));
    let text = snippet(&names, &usages, "", &[]).replace(
        "#100002=CONTEXT_DEPENDENT_SHAPE_REPRESENTATION(#100003,#100001);\n",
        "",
    );
    let read = read_assembly(text.as_bytes());
    assert_eq!(
        codes(read.refusals()),
        [
            RefusalCode::PlacementUnreadable,
            RefusalCode::TooManyOccurrences
        ]
    );
}

#[test]
fn a_parts_document_whose_tree_is_broken_is_not_read() {
    let (_, bytes) = ASSEMBLIES[3];
    let text = PartsDocument::read(bytes).to_json();
    let json: Value = serde_json::from_str(&text).unwrap();
    let part = |json: &Value, id: &str| {
        json["parts"]
            .as_array()
            .unwrap()
            .iter()
            .position(|p| p["definition"]["product"]["id"] == id)
            .unwrap()
    };
    let refuses = |edit: &dyn Fn(&mut Value), why: &str| {
        let mut changed = json.clone();
        edit(&mut changed);
        let error = PartsDocument::from_json(&changed.to_string());
        assert!(error.is_err(), "{why}: read");
    };
    let bracket = part(&json, "BRACKET");
    let lb = part(&json, "LB_ASSEM");
    refuses(
        &|j| j["parts"][bracket]["kind"] = Value::from("assembly"),
        "a part marked assembly",
    );
    refuses(
        &|j| j["parts"][lb]["kind"] = Value::from("part"),
        "an assembly marked part",
    );
    refuses(
        &|j| j["parts"][bracket]["occurrences"][0]["path"][0] = Value::from(999_999),
        "a path whose parent does not occur",
    );
    refuses(
        &|j| j["parts"][bracket]["occurrences"][0]["label"][1] = Value::from(" "),
        "a blank label",
    );
    refuses(
        &|j| j["parts"][bracket]["occurrences"][0]["label"][1] = Value::Null,
        "a missing label where paths are written by id",
    );
    refuses(&|j| j["source"]["header"] = Value::Null, "no header");
}

#[test]
fn a_placement_far_out_of_range_is_refused() {
    // s01 with the first bolt's assembly-side frame moved 2 km along x.
    let (_, bytes) = ASSEMBLIES[0];
    let text = String::from_utf8_lossy(bytes).replace(
        "#24 = CARTESIAN_POINT('',(-30.,-15.,8.));",
        "#24 = CARTESIAN_POINT('',(2.E+06,-15.,8.));",
    );
    assert_ne!(text.as_bytes(), bytes, "the edit applies");
    let read = read_assembly(text.as_bytes());
    assert!(read.listing().is_some());
    assert_eq!(codes(read.gate_refusals()), [RefusalCode::OutOfRange]);
}

#[test]
fn every_occurrence_carries_its_floors() {
    let (_, bytes) = ASSEMBLIES[3];
    let read = read_assembly(bytes);
    let listing = read.listing().unwrap();
    for occurrence in listing.occurrences() {
        let definition = &listing.definitions()[occurrence.definition];
        match definition.kind {
            DefinitionKind::Part => {
                assert_eq!(occurrence.part_uncertainty_mm, 1e-7);
                assert_eq!(occurrence.placement_uncertainty_mm, 1e-7);
            }
            DefinitionKind::Assembly => assert_eq!(occurrence.part_uncertainty_mm, 0.0),
        }
    }
}

#[test]
fn a_translation_of_1e17_mm_is_refused_without_panic() {
    // The design's snippet for check 38; the grid of coincident
    // occurrences once overflowed on it (review of M3 PR 2).
    let (_, bytes) = ASSEMBLIES[0];
    let text = String::from_utf8_lossy(bytes).replace(
        "#24 = CARTESIAN_POINT('',(-30.,-15.,8.));",
        "#24 = CARTESIAN_POINT('',(1.E+17,-15.,8.));",
    );
    let read = read_assembly(text.as_bytes());
    assert_eq!(codes(read.gate_refusals()), [RefusalCode::OutOfRange]);
}

#[test]
fn a_solid_two_definitions_share_is_refused_unread() {
    // s02 with the screw's representation holding the base's solid: each
    // definition would read it in full.
    let (_, bytes) = ASSEMBLIES[1];
    let text = String::from_utf8_lossy(bytes).replace(
        "#507 = ADVANCED_BREP_SHAPE_REPRESENTATION('',(#11,#508),#606);",
        "#507 = ADVANCED_BREP_SHAPE_REPRESENTATION('',(#11,#41),#606);",
    );
    assert_ne!(text.as_bytes(), bytes, "the edit applies");
    let read = read_assembly(text.as_bytes());
    let refused: Vec<&str> = read
        .gate_refusals()
        .iter()
        .filter(|r| r.code == RefusalCode::ComponentRefused)
        .map(|r| r.message.as_str())
        .collect();
    assert_eq!(refused.len(), 2, "{refused:?}");
    assert!(
        refused.iter().all(|m| m.contains("solid_shared")),
        "{refused:?}"
    );
}

#[test]
fn a_parts_document_with_impossible_floors_is_not_read() {
    let (_, bytes) = ASSEMBLIES[3];
    let text = PartsDocument::read(bytes).to_json();
    let json: Value = serde_json::from_str(&text).unwrap();
    let index = |id: &str| {
        json["parts"]
            .as_array()
            .unwrap()
            .iter()
            .position(|p| p["definition"]["product"]["id"] == id)
            .unwrap()
    };
    let (bolt, lb) = (index("BOLT"), index("LB_ASSEM"));
    let refuses = |edit: &dyn Fn(&mut Value), why: &str| {
        let mut changed = json.clone();
        edit(&mut changed);
        assert!(
            PartsDocument::from_json(&changed.to_string()).is_err(),
            "{why}: read"
        );
    };
    refuses(
        &|j| j["parts"][bolt]["occurrences"][0]["placement_uncertainty_mm"] = Value::from(-0.0),
        "a floor of -0.0",
    );
    refuses(
        &|j| j["parts"][bolt]["occurrences"][1]["part_uncertainty_mm"] = Value::from(0.5),
        "one occurrence's part floor unlike the others'",
    );
    refuses(
        &|j| j["parts"][lb]["occurrences"][0]["part_uncertainty_mm"] = Value::from(0.5),
        "a part floor on an assembly",
    );
    refuses(
        &|j| {
            j["parts"][bolt]["component"] = serde_json::json!({
                "gate": {"status": "refused", "refusals": [{"code": "no_solid", "instance": null, "message": "x"}]},
                "holes": null, "shafts": null, "complete": null
            });
        },
        "a refused component under an accepted gate",
    );
}

#[test]
fn check_38_counts_the_part_s_reach_beside_the_translation() {
    // The bolt reaches 25 mm below its frame. 20 mm short of the range,
    // its translation is in range and the bolt is not; 100 mm short, both
    // are.
    let (_, bytes) = ASSEMBLIES[0];
    let moved = |x: &str| {
        String::from_utf8_lossy(bytes).replace(
            "#24 = CARTESIAN_POINT('',(-30.,-15.,8.));",
            &format!("#24 = CARTESIAN_POINT('',({x},-15.,8.));"),
        )
    };
    let near = read_assembly(moved("999980.").as_bytes());
    assert_eq!(codes(near.gate_refusals()), [RefusalCode::OutOfRange]);
    let inside = read_assembly(moved("999900.").as_bytes());
    assert_eq!(inside.gate_refusals(), &[]);
}

/// `text` with definition `index`'s representation in a context of its own
/// stating `uncertainty` mm, or none.
fn with_context(text: &str, index: usize, uncertainty: Option<f64>) -> String {
    let b = 100 * (index as u64 + 1);
    let (u, c) = (9000 + 10 * index as u64, 9001 + 10 * index as u64);
    let stated = match uncertainty {
        Some(value) => format!(
            "#{u}=UNCERTAINTY_MEASURE_WITH_UNIT(LENGTH_MEASURE({value:?}),#1,'distance_accuracy_value','');\n\
             #{c}=(GEOMETRIC_REPRESENTATION_CONTEXT(3)GLOBAL_UNCERTAINTY_ASSIGNED_CONTEXT((#{u}))\
             GLOBAL_UNIT_ASSIGNED_CONTEXT((#1))REPRESENTATION_CONTEXT('',''));\n"
        ),
        None => format!(
            "#{c}=(GEOMETRIC_REPRESENTATION_CONTEXT(3)GLOBAL_UNIT_ASSIGNED_CONTEXT((#1))\
             REPRESENTATION_CONTEXT('',''));\n"
        ),
    };
    let rep = format!("#{}=SHAPE_REPRESENTATION('',(#{}),#2);", b + 5, b + 6);
    assert!(text.contains(&rep), "{rep}");
    text.replace(
        &rep,
        &format!("#{}=SHAPE_REPRESENTATION('',(#{}),#{c});", b + 5, b + 6),
    )
    .replace(
        "ENDSEC;END-ISO-10303-21;",
        &format!("{stated}ENDSEC;END-ISO-10303-21;"),
    )
}

#[test]
fn a_placement_floor_is_the_largest_its_path_states() {
    // ROOT (1e-7) holds SUB (1e-3), which holds LEAF; ROOT also holds SUB2,
    // whose context states none, which holds LEAF2.
    let mut text = snippet(
        &["ROOT", "SUB", "LEAF", "SUB2", "LEAF2"],
        &[(0, 1, "a"), (1, 2, "b"), (0, 3, "c"), (3, 4, "d")],
        "",
        &[],
    );
    text = with_context(&text, 0, Some(1e-7));
    text = with_context(&text, 1, Some(1e-3));
    text = with_context(&text, 2, Some(1e-5));
    text = with_context(&text, 3, None);
    let read = read_assembly(text.as_bytes());
    let listing = read.listing().unwrap();
    let floor = |id: &str| {
        let selected = listing.select(Some(id), None).unwrap();
        listing.occurrences()[selected.occurrences[0]].placement_uncertainty_mm
    };
    assert_eq!(
        floor("LEAF"),
        1e-3,
        "the sub-assembly's, the largest on the path"
    );
    assert_eq!(floor("LEAF2"), 1e-7, "the root's, as SUB2 states none");
    assert!(
        read.notes().iter().any(|n| n.code
            == fitment_core::AssemblyNoteCode::AssemblyUncertaintyMissing
            && n.instance == Some(405)),
        "{:?}",
        read.notes()
    );
}

#[test]
fn two_occurrences_at_one_place_but_turned_apart_are_not_coincident() {
    let both = snippet(&["ROOT", "A"], &[(0, 1, "u1"), (0, 1, "u2")], "", &[]);
    let coincident = |text: &str| {
        read_assembly(text.as_bytes())
            .notes()
            .iter()
            .filter(|n| n.code == fitment_core::AssemblyNoteCode::CoincidentOccurrences)
            .count()
    };
    assert_eq!(coincident(&both), 1, "both at the identity");
    // The second usage's assembly-side frame turned a quarter about z.
    let turned = both
        .replace(
            "#100014=ITEM_DEFINED_TRANSFORMATION('','',#206,#106);",
            "#100014=ITEM_DEFINED_TRANSFORMATION('','',#206,#9100);",
        )
        .replace(
            "#105=SHAPE_REPRESENTATION('',(#106),#2);",
            "#105=SHAPE_REPRESENTATION('',(#106,#9100),#2);",
        )
        .replace(
            "ENDSEC;END-ISO-10303-21;",
            "#9100=AXIS2_PLACEMENT_3D('',#3,#9101,#9102);#9101=DIRECTION('',(0.,0.,1.));\
             #9102=DIRECTION('',(0.,1.,0.));\nENDSEC;END-ISO-10303-21;",
        );
    assert_ne!(turned, both);
    assert_eq!(coincident(&turned), 0);
}
