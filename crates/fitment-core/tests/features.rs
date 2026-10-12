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

/// p01, a real solid, with its shape representation #10 listing `items`
/// instead of `(#11,#15)` (its origin and its solid #15), and `more`
/// instances added, numbered from #9001.
fn p01_with(items: &str, more: &str) -> Vec<u8> {
    let text = std::str::from_utf8(P01_THROUGH).unwrap();
    let listed = "#10 = ADVANCED_BREP_SHAPE_REPRESENTATION('',(#11,#15),#204);";
    assert!(text.contains(listed));
    let text = text.replace(
        listed,
        &format!("#10 = ADVANCED_BREP_SHAPE_REPRESENTATION('',({items}),#204);"),
    );
    let end = text.rfind("ENDSEC;").unwrap();
    format!("{}{more}\n{}", &text[..end], &text[end..]).into_bytes()
}

/// p01's solid moved into representation #9003 and held by #10 through
/// mapped item #9010 (moved 100 mm along x) and, if `twice`, #9011 (in
/// place).
fn mapped(twice: bool) -> Vec<u8> {
    let items = if twice {
        "#11,#9010,#9011"
    } else {
        "#11,#9010"
    };
    p01_with(
        items,
        "#9003=ADVANCED_BREP_SHAPE_REPRESENTATION('',(#11,#15),#204);\
         #9004=REPRESENTATION_MAP(#11,#9003);#9010=MAPPED_ITEM('',#9004,#9005);\
         #9011=MAPPED_ITEM('',#9004,#11);\
         #9005=AXIS2_PLACEMENT_3D('',#9006,$,$);#9006=CARTESIAN_POINT('',(100.,0.,0.));",
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
fn two_solids_in_one_product_are_refused() {
    // The generator writes a compound of two cubes with assembly mode off,
    // so they are one product with two solids, not an assembly.
    assert_eq!(refusal_codes(R03_TWO_SOLIDS), [RefusalCode::SeveralSolids]);
}

#[test]
fn a_shell_alone_has_no_solid_and_an_unread_item() {
    assert_eq!(
        refusal_codes(R06_SHELL),
        [RefusalCode::NoSolid, RefusalCode::UnreadShapeItem]
    );
}

#[test]
fn a_solid_carries_its_units_and_measured_offset() {
    let part = read_part(P01_THROUGH).unwrap();
    let solid = &part.solids[0];
    assert_eq!((solid.shells, solid.faces), (1, 7));
    assert_eq!(solid.units.length.name, "millimetre");
    assert_eq!(solid.units.length.factor, 1.0);
    assert_eq!(solid.units.plane_angle.name, "radian");
    assert_eq!(solid.units.uncertainty.len(), 1);
    assert_eq!(part.max_uncertainty_mm, solid.units.uncertainty[0].value_mm);
    assert!(
        solid.max_vertex_offset_mm < 1e-9,
        "{}",
        solid.max_vertex_offset_mm
    );
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
    let input = p01_with(
        "#11,#15",
        "#9001=SHAPE_DEFINITION_REPRESENTATION(#4,#9002);\
         #9002=ADVANCED_BREP_SHAPE_REPRESENTATION('',(#15),#204);",
    );
    let part = read_part(&input).unwrap();
    // The frame is the smallest shape representation, whatever the order
    // stepq walks them in.
    assert_eq!(part.frame.shape_representation, 10);
}

#[test]
fn a_solid_placed_by_a_mapped_item_has_its_transform() {
    let part = read_part(&mapped(false)).unwrap();
    let solid = &part.solids[0];
    assert_eq!(solid.instance, 15);
    assert_eq!(solid.representation, 9003);
    let transform = solid.transform.as_ref().unwrap();
    assert_eq!(transform.translation(), [100.0, 0.0, 0.0]);
    assert_eq!(part.frame.shape_representation, 10);
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
    let input = p01_with(
        "#11,#15,#9050",
        "#9050=GEOMETRIC_CURVE_SET('',(#9051,#9052,#9055));\
         #9051=CARTESIAN_POINT('',(0.,0.,0.));\
         #9052=LINE('',#9051,#9053);#9053=VECTOR('',#9054,1.);#9054=DIRECTION('',(1.,0.,0.));\
         #9055=(BOUNDED_CURVE()B_SPLINE_CURVE(1,(#9051,#9056),.UNSPECIFIED.,.F.,.F.)\
         B_SPLINE_CURVE_WITH_KNOTS((2,2),(0.,1.),.UNSPECIFIED.)CURVE()\
         GEOMETRIC_REPRESENTATION_ITEM()RATIONAL_B_SPLINE_CURVE((1.,1.))\
         REPRESENTATION_ITEM(''));\
         #9056=CARTESIAN_POINT('',(1.,0.,0.));",
    );
    let part = read_part(&input).unwrap();
    assert_eq!(part.other_items.len(), 1);
    assert_eq!(part.other_items[0].instance, 9050);
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
    // The solid's facts keep what the gate established.
    let broken: [fn(&mut serde_json::Value); 6] = [
        |v| v["part"]["solids"][0]["units"]["uncertainty"] = serde_json::json!([]),
        |v| v["part"]["solids"][0]["units"]["uncertainty"][0]["value_mm"] = (-1.0).into(),
        |v| v["part"]["solids"][0]["units"]["length"]["factor"] = (-25.4).into(),
        |v| v["part"]["solids"][0]["shells"] = 0.into(),
        |v| v["part"]["solids"][0]["max_vertex_offset_mm"] = (-1.0).into(),
        |v| v["part"]["max_uncertainty_mm"] = (-5.0).into(),
    ];
    for edit in broken {
        assert!(matches!(read(edit), Err(FormatError::Invalid(_))));
    }
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

/// p01 with `from` replaced by `to`, once: a hand-written fault in a real
/// solid, for the gate checks no generated part reaches.
fn p01_edit(from: &str, to: &str) -> Vec<u8> {
    edit(P01_THROUGH, &[(from, to)])
}

/// `input` with each `from` replaced by its `to`; each must occur once.
fn edit(input: &[u8], edits: &[(&str, &str)]) -> Vec<u8> {
    let mut text = std::str::from_utf8(input).unwrap().to_owned();
    for (from, to) in edits {
        assert_eq!(text.matches(from).count(), 1, "{from}");
        text = text.replacen(from, to, 1);
    }
    text.into_bytes()
}

/// The codes `input` is refused with, each once, in order.
fn distinct_codes(input: &[u8]) -> Vec<RefusalCode> {
    let mut codes = refusal_codes(input);
    codes.dedup();
    codes
}

#[test]
fn stage_4_refuses_a_solid_whose_topology_cannot_be_read() {
    let face_surface = p01_edit("#17 = ADVANCED_FACE(", "#17 = FACE_SURFACE(");
    let refused = read_part(&face_surface).unwrap_err();
    assert_eq!(refused.refusals()[0].code, RefusalCode::TopologyUnsupported);
    assert_eq!(refused.refusals()[0].instance, Some(17));
    let empty = p01_edit(
        "#16 = CLOSED_SHELL('',(#17,#57,#88,#123,#147,#175,#187));",
        "#16 = CLOSED_SHELL('',());",
    );
    assert_eq!(distinct_codes(&empty), [RefusalCode::TopologyMalformed]);
}

#[test]
fn stage_4_refuses_a_shell_with_a_face_missing() {
    let open = p01_edit("CLOSED_SHELL('',(#17,#57,", "CLOSED_SHELL('',(#57,");
    assert_eq!(distinct_codes(&open), [RefusalCode::SolidNotClosed]);
}

#[test]
fn stage_4_refuses_a_shell_split_across_two_shells() {
    // p01's seven faces, four in the outer shell and three in a void:
    // every edge is still used twice, but some by two shells.
    let split = edit(
        P01_THROUGH,
        &[
            (
                "#15 = MANIFOLD_SOLID_BREP('',#16);",
                "#15 = BREP_WITH_VOIDS('',#16,(#9001));\
                 #9000 = CLOSED_SHELL('',(#147,#175,#187));\
                 #9001 = ORIENTED_CLOSED_SHELL('',*,#9000,.T.);",
            ),
            (
                "#16 = CLOSED_SHELL('',(#17,#57,#88,#123,#147,#175,#187));",
                "#16 = CLOSED_SHELL('',(#17,#57,#88,#123));",
            ),
        ],
    );
    assert_eq!(distinct_codes(&split), [RefusalCode::SolidNotClosed]);
}

#[test]
fn stage_4_refuses_a_shell_in_two_pieces() {
    // r03's two cubes, their twelve faces in one shell: each cube is
    // closed, so only check 16 can see it.
    let pieces = edit(
        R03_TWO_SOLIDS,
        &[
            (
                "#10 = SHAPE_REPRESENTATION('',(#11,#15,#165),#315);",
                "#10 = SHAPE_REPRESENTATION('',(#11,#15),#315);",
            ),
            (
                "#16 = CLOSED_SHELL('',(#17,#57,#97,#119,#141,#153));",
                "#16 = CLOSED_SHELL('',(#17,#57,#97,#119,#141,#153,#167,#207,#247,#269,#291,#303));",
            ),
        ],
    );
    let refused = read_part(&pieces).unwrap_err();
    assert_eq!(refused.refusals().len(), 1);
    assert_eq!(refused.refusals()[0].code, RefusalCode::ShellDisconnected);
    assert_eq!(refused.refusals()[0].instance, Some(16));
}

#[test]
fn stage_5_refuses_missing_units() {
    let no_angle = p01_edit("((#205,#206,#207))", "((#205,#207))");
    assert_eq!(
        distinct_codes(&no_angle),
        [RefusalCode::AngleUnitUnreadable]
    );
    let no_uncertainty = p01_edit("GLOBAL_UNCERTAINTY_ASSIGNED_CONTEXT((#208)) ", "");
    assert_eq!(
        distinct_codes(&no_uncertainty),
        [RefusalCode::UncertaintyMissing]
    );
    let zero = p01_edit("LENGTH_MEASURE(1.E-07)", "LENGTH_MEASURE(0.)");
    assert_eq!(distinct_codes(&zero), [RefusalCode::UncertaintyUnreadable]);
}

#[test]
fn stage_6_refuses_unreadable_and_out_of_range_geometry() {
    let zero = p01_edit(
        "#199 = CYLINDRICAL_SURFACE('',#200,3.3);",
        "#199 = CYLINDRICAL_SURFACE('',#200,0.);",
    );
    let refused = read_part(&zero).unwrap_err();
    assert_eq!(refused.refusals()[0].code, RefusalCode::GeometryUnreadable);
    assert_eq!(refused.refusals()[0].instance, Some(199));
    let far = p01_edit(
        "#23 = CARTESIAN_POINT('',(0.,0.,0.));",
        "#23 = CARTESIAN_POINT('',(2.E+06,0.,0.));",
    );
    assert_eq!(distinct_codes(&far), [RefusalCode::OutOfRange]);
    // A surface far from its vertices: measured against it, their
    // distance would be NaN, which no bound refuses.
    let far_surface = p01_edit(
        "#201 = CARTESIAN_POINT('',(20.,15.,9.));",
        "#201 = CARTESIAN_POINT('',(20.,15.,1.E+300));",
    );
    let refused = read_part(&far_surface).unwrap_err();
    assert_eq!(refused.refusals().len(), 1);
    assert_eq!(refused.refusals()[0].code, RefusalCode::OutOfRange);
    assert_eq!(refused.refusals()[0].instance, Some(199));
}

#[test]
fn stage_6_refuses_a_face_or_edge_whose_geometry_is_not_geometry() {
    // stepq reads any entity it does not interpret as an unread surface
    // or curve; a product or a direction is neither.
    let product = p01_edit(
        "#187 = ADVANCED_FACE('',(#188),#199,.F.);",
        "#187 = ADVANCED_FACE('',(#188),#7,.F.);",
    );
    assert_eq!(distinct_codes(&product), [RefusalCode::GeometryUnreadable]);
    let direction = p01_edit(
        "#21 = EDGE_CURVE('',#22,#24,#26,.T.);",
        "#21 = EDGE_CURVE('',#22,#24,#29,.T.);",
    );
    assert_eq!(
        distinct_codes(&direction),
        [RefusalCode::GeometryUnreadable]
    );
}

#[test]
fn stage_6_refuses_a_plane_face_whose_sense_is_flipped() {
    // One character: the face at x = 0 now claims the plane's own normal,
    // +x, as outward, while its loop still runs as for -x.
    let flipped = p01_edit(
        "#17 = ADVANCED_FACE('',(#18),#52,.F.);",
        "#17 = ADVANCED_FACE('',(#18),#52,.T.);",
    );
    let refused = read_part(&flipped).unwrap_err();
    assert_eq!(refused.refusals().len(), 1);
    assert_eq!(refused.refusals()[0].code, RefusalCode::FaceOrientation);
    assert_eq!(refused.refusals()[0].instance, Some(17));
}

#[test]
fn stage_6_refuses_a_flipped_plane_face_with_an_arc_standing_across_it() {
    // The top face's edge at x = 0 becomes a major arc of r = 100 in the
    // plane x = 0, square to the face. Its area term is the projection
    // onto the face, so the arc counts as its chord and the flip shows.
    let flipped = edit(
        P01_THROUGH,
        &[
            (
                "#47 = EDGE_CURVE('',#24,#40,#48,.T.);",
                "#47 = EDGE_CURVE('',#24,#40,#48,.F.);",
            ),
            (
                "#48 = LINE('',#49,#50);",
                "#48 = CIRCLE('',#9000,100.);\
                 #9000 = AXIS2_PLACEMENT_3D('',#9001,#9002,#9003);\
                 #9001 = CARTESIAN_POINT('',(0.,15.,106.86859966642594));\
                 #9002 = DIRECTION('',(1.,0.,0.));\
                 #9003 = DIRECTION('',(0.,1.,0.));",
            ),
            (
                "#88 = ADVANCED_FACE('',(#89,#107),#118,.T.);",
                "#88 = ADVANCED_FACE('',(#89,#107),#118,.F.);",
            ),
        ],
    );
    let refused = read_part(&flipped).unwrap_err();
    assert_eq!(refused.refusals()[0].code, RefusalCode::FaceOrientation);
    assert_eq!(refused.refusals()[0].instance, Some(88));
}
