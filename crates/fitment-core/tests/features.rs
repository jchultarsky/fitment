//! Reading a part: the canonical gate's first three stages, and the
//! features document. Inputs are hand-written STEP snippets and the
//! committed unit parts in `tests/data`, included at compile time: core's
//! tests do no file I/O.

use fitment_core::{
    FEATURES_FORMAT_VERSION, FeatureKind, FeaturesDocument, FormatError, RefusalCode, Status,
    read_part,
};

const P01_THROUGH: &[u8] = include_bytes!("data/p01_through.stp");
const R03_TWO_SOLIDS: &[u8] = include_bytes!("data/r03_two_solids.stp");
const R04_ASSEMBLY: &[u8] = include_bytes!("data/r04_assembly.stp");
const R06_SHELL: &[u8] = include_bytes!("data/r06_shell.stp");

/// A file with `schema` and the given data section.
fn step(schema: &str, data: &str) -> Vec<u8> {
    format!(
        "ISO-10303-21;HEADER;FILE_DESCRIPTION((''),'2;1');\
         FILE_NAME('t','2026-01-01T00:00:00',(''),(''),'pre','sys','');\
         FILE_SCHEMA(('{schema}'));ENDSEC;{data}END-ISO-10303-21;"
    )
    .into_bytes()
}

const AP214: &str = "AUTOMOTIVE_DESIGN { 1 0 10303 214 1 1 1 1 }";

/// A product, its formation and its definition, numbered from `n`.
fn product(n: u64, name: &str) -> String {
    format!(
        "#{a}=PRODUCT('{name}','{name}','',(#{b}));\
         #{b}=PRODUCT_CONTEXT('',#{c},'mechanical');\
         #{c}=APPLICATION_CONTEXT('automotive design');\
         #{d}=PRODUCT_DEFINITION_FORMATION('','',#{a});\
         #{e}=PRODUCT_DEFINITION('design','',#{d},#{f});\
         #{f}=PRODUCT_DEFINITION_CONTEXT('part definition',#{c},'design');",
        a = n,
        b = n + 1,
        c = n + 2,
        d = n + 3,
        e = n + 4,
        f = n + 5,
    )
}

/// The codes `input` is refused with, in order.
fn refusal_codes(input: &[u8]) -> Vec<RefusalCode> {
    match read_part(input) {
        Ok(part) => panic!("accepted: {part:?}"),
        Err(refused) => refused.refusals.iter().map(|r| r.code).collect(),
    }
}

#[test]
fn a_file_that_is_not_step_is_unparseable() {
    let refused = read_part(b"solid cube\nfacet normal 0 0 1\n").unwrap_err();
    assert_eq!(refused.refusals.len(), 1);
    assert_eq!(refused.refusals[0].code, RefusalCode::Unparseable);
    assert_eq!(refused.header, None);
    assert!(
        refused.to_string().starts_with("refused: unparseable: "),
        "{refused}"
    );
}

#[test]
fn a_dangling_reference_is_refused_with_its_instance() {
    let input = step(AP214, "DATA;#1=PRODUCT('a','a','',(#9));ENDSEC;");
    let refused = read_part(&input).unwrap_err();
    assert_eq!(refused.refusals[0].code, RefusalCode::DanglingReference);
    assert_eq!(refused.refusals[0].instance, Some(1));
    assert!(refused.refusals[0].message.contains("#9"));
    // The header is still recorded.
    assert_eq!(refused.header.unwrap().schema.as_deref(), Some("ap214"));
}

#[test]
fn a_file_with_two_data_sections_is_refused() {
    let input = step(
        AP214,
        "DATA;#1=CARTESIAN_POINT('',(0.,0.,0.));ENDSEC;DATA;#2=CARTESIAN_POINT('',(1.,0.,0.));ENDSEC;",
    );
    assert!(refusal_codes(&input).contains(&RefusalCode::SeveralDataSections));
}

#[test]
fn an_unknown_schema_is_refused() {
    let input = step("IFC2X3", &format!("DATA;{}ENDSEC;", product(1, "a")));
    assert_eq!(refusal_codes(&input), [RefusalCode::SchemaUnrecognized]);
}

#[test]
fn a_later_stage_is_skipped_when_an_earlier_one_refuses() {
    // An unknown schema and no product: only stage 1's reason is given.
    let input = step("IFC2X3", "DATA;#1=CARTESIAN_POINT('',(0.,0.,0.));ENDSEC;");
    assert_eq!(refusal_codes(&input), [RefusalCode::SchemaUnrecognized]);
}

#[test]
fn a_file_with_no_product_definition_is_refused() {
    let input = step(AP214, "DATA;#1=CARTESIAN_POINT('',(0.,0.,0.));ENDSEC;");
    assert_eq!(refusal_codes(&input), [RefusalCode::NoDefinition]);
}

#[test]
fn two_product_definitions_without_an_assembly_are_refused() {
    let input = step(
        AP214,
        &format!("DATA;{}{}ENDSEC;", product(1, "a"), product(11, "b")),
    );
    assert_eq!(refusal_codes(&input), [RefusalCode::SeveralDefinitions]);
}

#[test]
fn a_definition_with_no_shape_has_no_solid() {
    let input = step(AP214, &format!("DATA;{}ENDSEC;", product(1, "a")));
    assert_eq!(refusal_codes(&input), [RefusalCode::NoSolid]);
}

#[test]
fn an_assembly_is_refused() {
    assert_eq!(refusal_codes(R04_ASSEMBLY), [RefusalCode::Assembly]);
}

#[test]
fn open_cascade_writes_two_solids_as_an_assembly() {
    // A compound of two solids comes out as two products under a third, so
    // it is refused as an assembly; `several_solids` needs a file that
    // lists two solids in one shape (below).
    assert_eq!(refusal_codes(R03_TWO_SOLIDS), [RefusalCode::Assembly]);
}

/// Product 1 with a shape representation #20 holding `items`, followed by
/// `more` instances. Stage 3 reads entity types, not topology, so a solid
/// can be an empty `manifold_solid_brep`.
fn shaped(items: &str, more: &str) -> Vec<u8> {
    step(
        AP214,
        &format!(
            "DATA;{}#7=PRODUCT_DEFINITION_SHAPE('','',#5);\
             #8=SHAPE_DEFINITION_REPRESENTATION(#7,#20);\
             #20=SHAPE_REPRESENTATION('',({items}),#21);\
             #21=REPRESENTATION_CONTEXT('','3D');{more}ENDSEC;",
            product(1, "a")
        ),
    )
}

const SOLID_30: &str = "#30=MANIFOLD_SOLID_BREP('',#31);#31=CLOSED_SHELL('',());";

#[test]
fn one_solid_passes_the_first_three_stages() {
    let part = read_part(&shaped("#30", SOLID_30)).unwrap();
    assert_eq!(part.solids[0].instance, 30);
    assert_eq!(part.frame.shape_representation, 20);
}

#[test]
fn two_solids_in_one_shape_are_refused() {
    let input = shaped(
        "#30,#40",
        &format!("{SOLID_30}#40=MANIFOLD_SOLID_BREP('',#41);#41=CLOSED_SHELL('',());"),
    );
    let refused = read_part(&input).unwrap_err();
    assert_eq!(refused.refusals.len(), 1);
    assert_eq!(refused.refusals[0].code, RefusalCode::SeveralSolids);
    assert_eq!(refused.refusals[0].instance, Some(40));
}

#[test]
fn a_solid_listed_twice_is_one_solid() {
    let input = shaped(
        "#30",
        &format!(
            "{SOLID_30}#9=SHAPE_DEFINITION_REPRESENTATION(#7,#22);#22=SHAPE_REPRESENTATION('',(#30),#21);"
        ),
    );
    assert!(read_part(&input).is_ok());
}

#[test]
fn a_curve_and_point_set_is_kept_beside_the_solid() {
    let input = shaped(
        "#30,#50",
        &format!(
            "{SOLID_30}#50=GEOMETRIC_CURVE_SET('',(#51,#52));\
             #51=CARTESIAN_POINT('',(0.,0.,0.));\
             #52=LINE('',#51,#53);#53=VECTOR('',#54,1.);#54=DIRECTION('',(1.,0.,0.));"
        ),
    );
    let part = read_part(&input).unwrap();
    assert_eq!(part.other_items.len(), 1);
    assert_eq!(part.other_items[0].instance, 50);
    assert_eq!(part.other_items[0].entity, "GEOMETRIC_CURVE_SET");
}

#[test]
fn a_set_holding_a_surface_is_refused() {
    let input = shaped(
        "#30,#50",
        &format!(
            "{SOLID_30}#50=GEOMETRIC_SET('',(#51,#55));\
             #51=CARTESIAN_POINT('',(0.,0.,0.));\
             #55=PLANE('',#56);#56=AXIS2_PLACEMENT_3D('',#51,$,$);"
        ),
    );
    let refused = read_part(&input).unwrap_err();
    assert_eq!(refused.refusals[0].code, RefusalCode::UnreadShapeItem);
    assert_eq!(refused.refusals[0].instance, Some(50));
    assert!(
        refused.refusals[0].message.contains("#55"),
        "{}",
        refused.refusals[0].message
    );
}

#[test]
fn a_surface_model_beside_the_solid_is_refused() {
    let input = shaped(
        "#30,#60",
        &format!("{SOLID_30}#60=SHELL_BASED_SURFACE_MODEL('',(#61));#61=OPEN_SHELL('',());"),
    );
    assert_eq!(refusal_codes(&input), [RefusalCode::UnreadShapeItem]);
}

#[test]
fn a_shell_alone_has_no_solid_and_an_unread_item() {
    assert_eq!(
        refusal_codes(R06_SHELL),
        [RefusalCode::NoSolid, RefusalCode::UnreadShapeItem]
    );
}

#[test]
fn a_canonical_part_is_read() {
    let part = read_part(P01_THROUGH).unwrap();
    assert_eq!(part.solids.len(), 1);
    let solid = &part.solids[0];
    assert_eq!(solid.entity, "MANIFOLD_SOLID_BREP");
    assert_eq!(solid.id.kind(), FeatureKind::Solid);
    assert_eq!(solid.id.instance(), solid.instance);
    assert_eq!(solid.transform, None);
    assert_eq!(part.other_items, []);
    let header = part.header();
    assert_eq!(header.schema.as_deref(), Some("ap214"));
    assert_eq!(header.implementation_level.as_deref(), Some("2;1"));
}

#[test]
fn reading_is_deterministic() {
    for input in [P01_THROUGH, R03_TWO_SOLIDS, R04_ASSEMBLY, R06_SHELL] {
        let document = |input: &[u8]| match read_part(input) {
            Ok(part) => FeaturesDocument::accepted(part).to_json(),
            Err(refused) => FeaturesDocument::refused(refused).to_json(),
        };
        assert_eq!(document(input), document(input));
    }
}

#[test]
fn documents_round_trip() {
    let accepted = FeaturesDocument::accepted(read_part(P01_THROUGH).unwrap());
    let text = accepted.to_json();
    assert!(text.ends_with("}\n"));
    let read = FeaturesDocument::from_json(&text).unwrap();
    assert_eq!(read, accepted);
    assert!(read.is_current());
    assert_eq!(
        read.part.as_ref().unwrap().header(),
        accepted.part.as_ref().unwrap().header()
    );

    let refused = FeaturesDocument::refused(read_part(R04_ASSEMBLY).unwrap_err());
    assert_eq!(refused.status, Status::Refused);
    assert_eq!(
        FeaturesDocument::from_json(&refused.to_json()).unwrap(),
        refused
    );
}

#[test]
fn documents_that_break_their_rules_are_not_read() {
    let text = FeaturesDocument::accepted(read_part(P01_THROUGH).unwrap()).to_json();
    let with = |from: &str, to: &str| {
        assert!(text.contains(from), "{from}");
        FeaturesDocument::from_json(&text.replacen(from, to, 1))
    };
    assert!(matches!(
        with("\"fitment.features\"", "\"other\""),
        Err(FormatError::Format { .. })
    ));
    assert!(matches!(
        with(
            &format!("\"format_version\": {FEATURES_FORMAT_VERSION}"),
            "\"format_version\": 99"
        ),
        Err(FormatError::Version { found: 99 })
    ));
    assert!(matches!(
        with("\"status\": \"accepted\"", "\"status\": \"refused\""),
        Err(FormatError::Invalid(_))
    ));
    assert!(matches!(
        with("\"format\":", "\"surprise\": 1,\n  \"format\":"),
        Err(FormatError::Json { .. })
    ));
    assert!(matches!(
        with("\"id\": \"solid-", "\"id\": \"slab-"),
        Err(FormatError::Json { .. })
    ));
}
