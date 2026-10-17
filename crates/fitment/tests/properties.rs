//! CT18 (`docs/research/m5-catalog.md`, section 4.13, PR1 to PR6): what
//! the capture keeps of the fixtures' stated properties and of generated
//! ones, what the shipped map makes of them, and that none of it changes a
//! verdict. The fixtures are optional unless `FITMENT_REQUIRE_FIXTURES=1`.

use std::fs;
use std::path::{Path, PathBuf};

use fitment_core::{
    Clearance, FeaturesDocument, Field, Grip, MAX_STATED, MatchOptions, MatchPolicy, Matcher,
    Meaning, NoFitVerifier, PropertyMap, SocketDocument, StatedProperties, TolerancePolicy,
};

fn fixture(name: &str) -> Option<Vec<u8>> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/nist/NIST-PMI-STEP-Files")
        .join(name);
    if let Ok(bytes) = fs::read(&path) {
        return Some(bytes);
    }
    let required = std::env::var_os("FITMENT_REQUIRE_FIXTURES").is_some_and(|v| v == "1");
    assert!(
        !required,
        "FITMENT_REQUIRE_FIXTURES=1 but {name} is missing"
    );
    eprintln!("skipped: {name}; run tools/fetch-fixtures.sh core");
    None
}

fn core(path: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../fitment-core/tests")
        .join(path)
}

/// The committed bracket with `extra` instances added to its data section;
/// its product definition is `#5`, its shape `#4`, its context `#2`.
fn bracket_with(extra: &str) -> Vec<u8> {
    let text = fs::read_to_string(core("candidates/m_bracket.stp")).unwrap();
    let end = text.rfind("ENDSEC;").unwrap();
    format!("{}{extra}{}", &text[..end], &text[end..]).into_bytes()
}

/// A user-defined property `#id` of `subject`, with one descriptive value.
fn user(id: u64, subject: u64, name: &str, value: &str) -> String {
    format!(
        "#{id}=PROPERTY_DEFINITION('{name}','user defined attribute',#{subject});\n\
         #{}=PROPERTY_DEFINITION_REPRESENTATION(#{id},#{});\n\
         #{}=REPRESENTATION('',(#{}),#2);\n\
         #{}=DESCRIPTIVE_REPRESENTATION_ITEM('{name}','{value}');\n",
        id + 1,
        id + 2,
        id + 2,
        id + 3,
        id + 3
    )
}

/// PR4's evidence: each shipped row on its cited record.
#[test]
fn the_shipped_row_reads_its_cited_records() {
    let map = PropertyMap::shipped();
    for (file, instance) in [
        ("nist_ctc_05_asme1_ap242-e1.stp", 1095),
        ("nist_stc_09_asme1_ap242-e3.stp", 7270),
    ] {
        let Some(bytes) = fixture(file) else {
            return;
        };
        let (document, stated) = FeaturesDocument::read_with_properties(&bytes);
        assert!(document.part().is_some(), "{file} is accepted");
        assert_eq!(
            stated,
            StatedProperties::read(&bytes),
            "{file}: one parse or two"
        );
        let property = stated
            .stated
            .iter()
            .find(|p| p.instance == instance)
            .unwrap_or_else(|| panic!("{file} #{instance}"));
        assert_eq!(property.name, "MaterialMissingAssignments");
        assert_eq!(property.value, "TRUE");
        let mapped = map.meaning(property).unwrap();
        assert_eq!(
            (mapped.field, mapped.meaning),
            (Field::Material, Meaning::Unassigned)
        );
        // Nothing else in the file maps (PR4, PR6).
        let others = stated
            .stated
            .iter()
            .filter(|p| p.instance != instance)
            .filter(|p| map.meaning(p).is_some())
            .count();
        assert_eq!(others, 0, "{file}");
    }
}

/// PR4 and PR6: a CAD default and a drawing note map to nothing; the
/// note, a shape aspect's, is not even captured.
#[test]
fn defaults_and_notes_mean_nothing() {
    let map = PropertyMap::shipped();
    if let Some(bytes) = fixture("nist_ctc_04_asme1_ap242-e1.stp") {
        let stated = StatedProperties::read(&bytes);
        let density = stated.stated.iter().find(|p| p.instance == 18079).unwrap();
        assert_eq!(
            (density.name.as_str(), density.value.as_str()),
            ("Density", "1000.0")
        );
        assert_eq!(density.measure.as_deref(), Some("POSITIVE_RATIO_MEASURE"));
        assert_eq!(density.unit.as_deref(), Some("#18077"));
        assert_eq!(map.meaning(density), None);
    }
    if let Some(bytes) = fixture("nist_stc_09_asme1_ap242-e3.stp") {
        let stated = StatedProperties::read(&bytes);
        assert!(
            stated
                .stated
                .iter()
                .all(|p| p.instance != 8517 && p.instance != 8518)
        );
        assert!(!stated.stated.iter().any(|p| p.value.contains("304")));
    }
}

/// PR2's bound: `nist_stc_06`'s hundreds of values are kept in full.
#[test]
fn hundreds_of_values_are_kept() {
    let Some(bytes) = fixture("nist_stc_06_asme1_ap242-e3.stp") else {
        return;
    };
    let stated = StatedProperties::read(&bytes);
    assert!(!stated.truncated);
    assert!(stated.stated.len() > 440, "{}", stated.stated.len());
}

/// PR2: past 1,024 values the set is truncated; a shape aspect's property,
/// a property not user-defined and a designation of something else are
/// not captured; the part's own material designation is.
#[test]
fn what_is_captured_from_a_generated_file() {
    let many: String = (0..2000)
        .map(|i| user(100_000 + 4 * i, 5, &format!("p{i}"), "1"))
        .collect();
    let stated = StatedProperties::read(&bracket_with(&many));
    assert!(stated.truncated);
    assert_eq!(stated.stated.len(), MAX_STATED);

    let extra = [
        user(90_000, 5, "OnTheDefinition", "a"),
        user(90_010, 4, "OnTheShape", "b"),
        "#90100=SHAPE_ASPECT('hole','',#4,.F.);\n".to_owned(),
        user(90_110, 90_100, "OnAnAspect", "c"),
        // A shape of the aspect, which leads back to the part (review).
        "#90150=PRODUCT_DEFINITION_SHAPE('','',#90100);\n".to_owned(),
        user(90_160, 90_150, "MaterialMissingAssignments", "TRUE"),
        "#90200=PROPERTY_DEFINITION('NotUser','',#5);\n\
         #90201=PROPERTY_DEFINITION_REPRESENTATION(#90200,#90202);\n\
         #90202=REPRESENTATION('',(#90203),#2);\n\
         #90203=DESCRIPTIVE_REPRESENTATION_ITEM('NotUser','d');\n"
            .to_owned(),
        "#90300=MATERIAL_DESIGNATION('Steel 304',(#5));\n".to_owned(),
        "#90301=MATERIAL_DESIGNATION('Elsewhere',(#90100));\n".to_owned(),
    ]
    .concat();
    let bytes = bracket_with(&extra);
    let (document, stated) = FeaturesDocument::read_with_properties(&bytes);
    assert!(document.part().is_some(), "{:?}", document.refusals());
    let kept: Vec<(u64, &str, &str)> = stated
        .stated
        .iter()
        .map(|p| (p.instance, p.name.as_str(), p.value.as_str()))
        .collect();
    assert_eq!(
        kept,
        [
            (90_000, "OnTheDefinition", "a"),
            (90_010, "OnTheShape", "b"),
            (90_300, "MATERIAL_DESIGNATION", "Steel 304"),
        ]
    );
    assert!(!stated.truncated);
    // An assembly's file holds several definitions: nothing is captured.
    let assembly = fs::read(core("assemblies/s01_bolted.stp")).unwrap();
    assert_eq!(
        StatedProperties::read(&assembly),
        StatedProperties::default()
    );
}

/// PR5 and PR6: stated properties and a PRODUCT.name that reads like a
/// size change no verdict, no flag and no requirement.
#[test]
fn properties_and_names_change_no_verdict() {
    let socket = {
        let bytes = fs::read(core("assemblies/s01_bolted.stp")).unwrap();
        SocketDocument::extract(&bytes, Some("BRACKET"), None).unwrap()
    };
    let policy = MatchPolicy::new(TolerancePolicy::default(), Clearance::Strict, Grip::Hard);
    let matcher = Matcher::new(&socket, policy, MatchOptions::default(), &NoFitVerifier).unwrap();
    let plain = fs::read(core("candidates/m_bracket.stp")).unwrap();
    let stated = bracket_with(
        &[
            user(90_000, 5, "MATERIAL", "304 SST"),
            user(90_010, 5, "Thread", "M6x1.0-6H"),
            user(90_020, 5, "MaterialMissingAssignments", "TRUE"),
            "#90300=MATERIAL_DESIGNATION('Steel 304',(#5));\n".to_owned(),
        ]
        .concat(),
    );
    let named = String::from_utf8(plain.clone())
        .unwrap()
        .replacen("PRODUCT('m_bracket'", "PRODUCT('M6x20'", 1)
        .into_bytes();
    let judge = |bytes: &[u8]| {
        let result = matcher.judge_step(bytes);
        (result.verdict.kind(), format!("{:?}", result.verdict))
    };
    let base = judge(&plain);
    assert_eq!(judge(&stated), base);
    assert_eq!(judge(&named), base);
}
