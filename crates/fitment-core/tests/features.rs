//! Reading a part: the canonical gate's first three stages, and the
//! features document. Inputs are hand-written STEP snippets and the
//! committed unit parts in `tests/data`, included at compile time: core's
//! tests do no file I/O.

use std::fmt::Write as _;

use fitment_core::{
    FEATURES_FORMAT_VERSION, FeatureKind, FeaturesDocument, FormatError, RefusalCode, Status,
    read_part,
};

const P01_THROUGH: &[u8] = include_bytes!("data/p01_through.stp");
const R03_TWO_SOLIDS: &[u8] = include_bytes!("data/r03_two_solids.stp");
const R04_ASSEMBLY: &[u8] = include_bytes!("data/r04_assembly.stp");
const R06_SHELL: &[u8] = include_bytes!("data/r06_shell.stp");

/// A file naming `schemas` with the given data section(s).
fn step_with(schemas: &str, data: &str) -> Vec<u8> {
    format!(
        "ISO-10303-21;HEADER;FILE_DESCRIPTION((''),'2;1');\
         FILE_NAME('t','2026-01-01T00:00:00',(''),(''),'pre','sys','');\
         FILE_SCHEMA(({schemas}));ENDSEC;{data}END-ISO-10303-21;"
    )
    .into_bytes()
}

fn step(schema: &str, data: &str) -> Vec<u8> {
    step_with(&format!("'{schema}'"), data)
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

/// Product 1 with a shape representation #20 holding `items`, in a
/// millimetre context #21, followed by `more` instances. Stage 3 reads
/// entity types, not topology, so a solid can be an empty
/// `manifold_solid_brep`.
fn shaped(items: &str, more: &str) -> Vec<u8> {
    step(
        AP214,
        &format!(
            "DATA;{}#7=PRODUCT_DEFINITION_SHAPE('','',#5);\
             #8=SHAPE_DEFINITION_REPRESENTATION(#7,#20);\
             #20=SHAPE_REPRESENTATION('',({items}),#21);\
             #21=(GEOMETRIC_REPRESENTATION_CONTEXT(3)GLOBAL_UNIT_ASSIGNED_CONTEXT((#22))\
             REPRESENTATION_CONTEXT('',''));\
             #22=(LENGTH_UNIT()NAMED_UNIT(*)SI_UNIT(.MILLI.,.METRE.));{more}ENDSEC;",
            product(1, "a")
        ),
    )
}

const SOLID_30: &str = "#30=MANIFOLD_SOLID_BREP('',#31);#31=CLOSED_SHELL('',());";

/// Solid #30 in representation #14, held by #20 through mapped item #71
/// (moved 100 mm along x) and, if `twice`, #70 (at the origin).
fn mapped(twice: bool) -> Vec<u8> {
    let items = if twice { "#70,#71" } else { "#71" };
    shaped(
        items,
        &format!(
            "{SOLID_30}#14=SHAPE_REPRESENTATION('',(#30,#24),#21);\
             #24=AXIS2_PLACEMENT_3D('',#80,$,$);#80=CARTESIAN_POINT('',(0.,0.,0.));\
             #69=REPRESENTATION_MAP(#24,#14);#70=MAPPED_ITEM('',#69,#24);\
             #71=MAPPED_ITEM('',#69,#25);#25=AXIS2_PLACEMENT_3D('',#81,$,$);\
             #81=CARTESIAN_POINT('',(100.,0.,0.));"
        ),
    )
}

/// The codes `input` is refused with, in order.
fn refusal_codes(input: &[u8]) -> Vec<RefusalCode> {
    match read_part(input) {
        Ok(part) => panic!("accepted: {part:?}"),
        Err(refused) => refused.refusals().iter().map(|r| r.code).collect(),
    }
}

#[test]
fn a_file_that_is_not_step_is_unparseable() {
    let document = FeaturesDocument::read(b"solid cube\nfacet normal 0 0 1\n");
    assert_eq!(document.status(), Status::Refused);
    assert_eq!(document.refusals().len(), 1);
    assert_eq!(document.refusals()[0].code, RefusalCode::Unparseable);
    assert_eq!(document.header(), None);
    let refused = read_part(b"solid cube").unwrap_err();
    assert!(
        refused.to_string().starts_with("refused: unparseable: "),
        "{refused}"
    );
}

#[test]
fn a_dangling_reference_is_refused_with_its_instance() {
    let input = step(AP214, "DATA;#1=PRODUCT('a','a','',(#9));ENDSEC;");
    let document = FeaturesDocument::read(&input);
    let refusal = &document.refusals()[0];
    assert_eq!(refusal.code, RefusalCode::DanglingReference);
    assert_eq!(refusal.instance, Some(1));
    assert!(refusal.message.contains("#9"));
    // The header is still recorded.
    assert_eq!(document.header().unwrap().schema.as_deref(), Some("ap214"));
}

#[test]
fn many_dangling_references_are_capped_and_stay_sorted() {
    let mut data = String::new();
    for i in 1..=40 {
        writeln!(data, "#{i}=PRODUCT('a','a','',(#{}));", 1000 + i).unwrap();
    }
    let document = FeaturesDocument::read(&step(AP214, &format!("DATA;{data}ENDSEC;")));
    let refusals = document.refusals();
    assert_eq!(refusals.len(), 32);
    assert_eq!(refusals[0].instance, None);
    assert_eq!(refusals[0].message, "and 9 more");
    assert_eq!(refusals[1].instance, Some(1));
    // The document reads back: its refusals are in order.
    let read = FeaturesDocument::from_json(&document.to_json()).unwrap();
    assert_eq!(read, document);
}

#[test]
fn every_check_in_stage_1_runs() {
    let input = step_with(
        &format!("'IFC2X3','{AP214}'"),
        "DATA;#1=CARTESIAN_POINT('',(0.,0.,0.));ENDSEC;DATA;#2=CARTESIAN_POINT('',(1.,0.,0.));ENDSEC;",
    );
    assert_eq!(
        refusal_codes(&input),
        [
            RefusalCode::SeveralDataSections,
            RefusalCode::SchemaUnrecognized,
            RefusalCode::SchemaUnrecognized,
        ]
    );
}

#[test]
fn only_geometric_validation_may_follow_the_schema() {
    let ok = step_with(
        &format!("'{AP214}','GEOMETRIC_VALIDATION_PROPERTIES_MIM'"),
        &format!("DATA;{}ENDSEC;", product(1, "a")),
    );
    assert_eq!(refusal_codes(&ok), [RefusalCode::NoSolid], "passes stage 1");
    let extra = step_with(
        &format!("'{AP214}','CONFIG_CONTROL_DESIGN'"),
        &format!("DATA;{}ENDSEC;", product(1, "a")),
    );
    assert_eq!(refusal_codes(&extra), [RefusalCode::SchemaUnrecognized]);
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

#[test]
fn a_shell_alone_has_no_solid_and_an_unread_item() {
    assert_eq!(
        refusal_codes(R06_SHELL),
        [RefusalCode::NoSolid, RefusalCode::UnreadShapeItem]
    );
}

#[test]
fn one_solid_passes_the_first_three_stages() {
    let part = read_part(&shaped("#30", SOLID_30)).unwrap();
    assert_eq!(part.solids[0].instance, 30);
    assert_eq!(part.solids[0].transform, None);
    assert_eq!(part.frame.shape_representation, 20);
}

#[test]
fn two_solids_in_one_shape_are_refused() {
    let input = shaped(
        "#30,#40",
        &format!("{SOLID_30}#40=MANIFOLD_SOLID_BREP('',#41);#41=CLOSED_SHELL('',());"),
    );
    let refused = read_part(&input).unwrap_err();
    assert_eq!(refused.refusals().len(), 1);
    assert_eq!(refused.refusals()[0].code, RefusalCode::SeveralSolids);
    assert_eq!(refused.refusals()[0].instance, Some(40));
}

#[test]
fn a_solid_listed_twice_in_one_place_is_one_solid() {
    let input = shaped(
        "#30",
        &format!(
            "{SOLID_30}#9=SHAPE_DEFINITION_REPRESENTATION(#7,#23);\
             #23=SHAPE_REPRESENTATION('',(#30),#21);"
        ),
    );
    let part = read_part(&input).unwrap();
    // The frame is the smallest shape representation, whatever the order
    // stepq walks them in.
    assert_eq!(part.frame.shape_representation, 20);
}

#[test]
fn a_solid_placed_by_a_mapped_item_has_its_transform() {
    let part = read_part(&mapped(false)).unwrap();
    let solid = &part.solids[0];
    assert_eq!(solid.instance, 30);
    assert_eq!(solid.representation, 14);
    let transform = solid.transform.as_ref().unwrap();
    assert_eq!(transform.translation(), [100.0, 0.0, 0.0]);
    assert_eq!(part.frame.shape_representation, 20);
}

#[test]
fn a_solid_placed_twice_is_two_solids() {
    // The same `#id`, placed at the origin and 100 mm away: two bodies.
    let refused = read_part(&mapped(true)).unwrap_err();
    assert_eq!(refused.refusals().len(), 1);
    let refusal = &refused.refusals()[0];
    assert_eq!(refusal.code, RefusalCode::SeveralSolids);
    assert!(refusal.message.contains("2 solids"), "{}", refusal.message);
}

#[test]
fn an_incomplete_shape_is_refused_and_ends_the_stage() {
    // A relationship to a representation in another context is not
    // followed, and a surface model would also be refused; only the
    // incompleteness is reported, since what the shape holds is unknown.
    let input = shaped(
        "#30,#60",
        &format!(
            "{SOLID_30}#60=SHELL_BASED_SURFACE_MODEL('',(#61));#61=OPEN_SHELL('',());\
             #32=SHAPE_REPRESENTATION_RELATIONSHIP('','',#20,#33);\
             #33=SHAPE_REPRESENTATION('',(#30),#34);\
             #34=(GEOMETRIC_REPRESENTATION_CONTEXT(3)GLOBAL_UNIT_ASSIGNED_CONTEXT((#22))\
             REPRESENTATION_CONTEXT('',''));"
        ),
    );
    let refused = read_part(&input).unwrap_err();
    assert_eq!(refused.refusals().len(), 1);
    assert_eq!(refused.refusals()[0].code, RefusalCode::ShapeIncomplete);
    assert_eq!(refused.refusals()[0].instance, Some(32));
}

#[test]
fn a_curve_and_point_set_is_kept_beside_the_solid() {
    let input = shaped(
        "#30,#50",
        &format!(
            "{SOLID_30}#50=GEOMETRIC_CURVE_SET('',(#51,#52,#55));\
             #51=CARTESIAN_POINT('',(0.,0.,0.));\
             #52=LINE('',#51,#53);#53=VECTOR('',#54,1.);#54=DIRECTION('',(1.,0.,0.));\
             #55=(BOUNDED_CURVE()B_SPLINE_CURVE(1,(#51,#56),.UNSPECIFIED.,.F.,.F.)\
             B_SPLINE_CURVE_WITH_KNOTS((2,2),(0.,1.),.UNSPECIFIED.)CURVE()\
             GEOMETRIC_REPRESENTATION_ITEM()RATIONAL_B_SPLINE_CURVE((1.,1.))\
             REPRESENTATION_ITEM(''));\
             #56=CARTESIAN_POINT('',(1.,0.,0.));"
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
    let refusal = &refused.refusals()[0];
    assert_eq!(refusal.code, RefusalCode::UnreadShapeItem);
    assert_eq!(refusal.instance, Some(50));
    assert!(refusal.message.contains("#55"), "{}", refusal.message);
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
fn a_canonical_part_is_read() {
    let document = FeaturesDocument::read(P01_THROUGH);
    assert_eq!(document.status(), Status::Accepted);
    let part = document.part().unwrap();
    assert_eq!(part.solids.len(), 1);
    let solid = &part.solids[0];
    assert_eq!(solid.entity, "MANIFOLD_SOLID_BREP");
    assert_eq!(solid.id.kind(), FeatureKind::Solid);
    assert_eq!(solid.id.instance(), solid.instance);
    assert_eq!(solid.transform, None);
    assert_eq!(part.other_items, []);
    let header = document.header().unwrap();
    assert_eq!(header.schema.as_deref(), Some("ap214"));
    assert_eq!(header.implementation_level.as_deref(), Some("2;1"));
    assert_eq!(read_part(P01_THROUGH).unwrap(), *part);
}

#[test]
fn reading_is_deterministic() {
    for input in [P01_THROUGH, R03_TWO_SOLIDS, R04_ASSEMBLY, R06_SHELL] {
        let json = |input: &[u8]| FeaturesDocument::read(input).to_json();
        assert_eq!(json(input), json(input));
    }
}

#[test]
fn documents_round_trip() {
    for input in [P01_THROUGH, R04_ASSEMBLY] {
        let document = FeaturesDocument::read(input);
        let text = document.to_json();
        assert!(text.ends_with("}\n"));
        let read = FeaturesDocument::from_json(&text).unwrap();
        assert_eq!(read, document);
        assert!(read.is_current());
    }
}

/// `document` as JSON with `edit` applied to its value.
fn edited(document: &FeaturesDocument, edit: impl FnOnce(&mut serde_json::Value)) -> String {
    let mut value: serde_json::Value = serde_json::from_str(&document.to_json()).unwrap();
    edit(&mut value);
    serde_json::to_string_pretty(&value).unwrap()
}

#[test]
fn documents_that_break_their_rules_are_not_read() {
    let accepted = FeaturesDocument::read(P01_THROUGH);
    let read =
        |edit: fn(&mut serde_json::Value)| FeaturesDocument::from_json(&edited(&accepted, edit));
    assert!(matches!(
        read(|v| v["format"] = "other".into()),
        Err(FormatError::Format { .. })
    ));
    assert!(matches!(
        read(|v| v["format_version"] = (FEATURES_FORMAT_VERSION + 98).into()),
        Err(FormatError::Version { found: 99 })
    ));
    assert!(matches!(
        read(|v| v["status"] = "refused".into()),
        Err(FormatError::Invalid(_))
    ));
    assert!(matches!(
        read(|v| v["header"] = serde_json::Value::Null),
        Err(FormatError::Invalid(_))
    ));
    assert!(matches!(
        read(|v| v["surprise"] = 1.into()),
        Err(FormatError::Json { .. })
    ));
    assert!(matches!(
        read(|v| v["part"]["solids"][0]["id"] = "slab-15".into()),
        Err(FormatError::Json { .. })
    ));
    assert!(matches!(
        read(|v| v["part"]["solids"][0]["id"] = "solid-16".into()),
        Err(FormatError::Invalid(_))
    ));
    // Every key is written, so none may be missing, `null` ones included.
    assert!(matches!(
        read(|v| {
            v["part"]["solids"][0]
                .as_object_mut()
                .unwrap()
                .remove("transform");
        }),
        Err(FormatError::Json { .. })
    ));
}

#[test]
fn refusals_read_back_must_be_sorted() {
    let input = step(
        AP214,
        "DATA;#1=PRODUCT('a','a','',(#8));#2=PRODUCT('b','b','',(#9));ENDSEC;",
    );
    let refused = FeaturesDocument::read(&input);
    assert_eq!(refused.refusals().len(), 2);
    let reversed = edited(&refused, |v| {
        v["refusals"].as_array_mut().unwrap().reverse();
    });
    assert!(matches!(
        FeaturesDocument::from_json(&reversed),
        Err(FormatError::Invalid(_))
    ));
}
