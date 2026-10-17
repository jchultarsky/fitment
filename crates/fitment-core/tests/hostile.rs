//! Hostile inputs (`docs/research/m3-socket.md`, N4 and section 15 item 7):
//! assemblies built from the committed s08x by adding usages, so large and
//! still small on disk. Check 39 counts every candidate pair the searches
//! return, deterministically, so an extraction that finishes under its
//! limit of 10⁷ shows the searches grew with what lies near each part: the
//! same inputs scanned pair by pair need more than 10⁸. What check 39 does
//! not count (merging sockets, gathering) is n log n by construction.

use std::fmt::Write as _;

use fitment_core::{RefusalCode, SocketDocument, Status};

const S08X: &str = include_str!("assemblies/s08x_nested.stp");

/// The joint sub-assembly's definition and shape representation, and the
/// nut's, in s08x.
const JOINT: (u64, u64) = (516, 521);
const NUT: (u64, u64) = (1215, 1220);
/// The root's definition, its shape representation and its identity frame.
const ROOT: (u64, u64, u64) = (5, 10, 11);

/// s08x with `child` (definition, shape representation) placed again under
/// the root at each of `at` (unturned), every usage a NAUO with its CDSR.
fn with_usages(child: (u64, u64), at: &[[f64; 3]]) -> String {
    let next = S08X
        .lines()
        .filter_map(|l| l.strip_prefix('#')?.split_once(' ')?.0.parse::<u64>().ok())
        .max()
        .unwrap()
        + 1;
    let mut added = String::new();
    let mut frames = Vec::new();
    for (k, p) in at.iter().enumerate() {
        let id = next + 10 * k as u64;
        let frame = id + 3;
        frames.push(format!("#{frame}"));
        writeln!(
            added,
            "#{id} = CARTESIAN_POINT('',({:?},{:?},{:?}));\n#{} = DIRECTION('',(0.,0.,1.));\n\
             #{} = DIRECTION('',(1.,0.,0.));\n#{frame} = AXIS2_PLACEMENT_3D('',#{id},#{},#{});\n\
             #{} = ITEM_DEFINED_TRANSFORMATION('','',#{},#{frame});\n\
             #{} = (REPRESENTATION_RELATIONSHIP('','',#{},#{})REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION(#{})SHAPE_REPRESENTATION_RELATIONSHIP());\n\
             #{} = NEXT_ASSEMBLY_USAGE_OCCURRENCE('h{k}','','',#{},#{},$);\n\
             #{} = PRODUCT_DEFINITION_SHAPE('Placement','Placement of an item',#{});\n\
             #{} = CONTEXT_DEPENDENT_SHAPE_REPRESENTATION(#{},#{});",
            p[0],
            p[1],
            p[2],
            id + 1,
            id + 2,
            id + 1,
            id + 2,
            id + 4,
            ROOT.2,
            id + 5,
            child.1,
            ROOT.1,
            id + 4,
            id + 6,
            ROOT.0,
            child.0,
            id + 7,
            id + 6,
            id + 8,
            id + 5,
            id + 7,
        )
        .unwrap();
    }
    // The root's representation lists the new frames among its items.
    let root = format!("#{} = SHAPE_REPRESENTATION('',(", ROOT.1);
    let text = S08X.replacen(&root, &format!("{root}{},", frames.join(",")), 1);
    text.replacen(
        "ENDSEC;\nEND-ISO-10303-21;",
        &format!("{added}ENDSEC;\nEND-ISO-10303-21;"),
        1,
    )
}

fn socket(text: &str, part: &str) -> SocketDocument {
    SocketDocument::extract(text.as_bytes(), Some(part), None).unwrap()
}

/// 2,500 more joints in a grid 200 mm apart: 10,008 bolts, each among
/// its own joint's parts only.
#[test]
fn ten_thousand_bolts_are_read_in_reach_of_their_own_joints() {
    let at: Vec<[f64; 3]> = (0..2500)
        .map(|k| {
            [
                200.0 * f64::from(k % 50),
                300.0 + 200.0 * f64::from(k / 50),
                0.0,
            ]
        })
        .collect();
    let text = with_usages(JOINT, &at);
    let bolts = socket(&text, "BOLT");
    assert_eq!(bolts.status(), Status::Accepted, "{:?}", bolts.refusals());
    let extracted = bolts.extracted().unwrap();
    assert_eq!(extracted.occurrences.len(), 10_008);
    // The 8 over s08x's plate share the original's socket; the 10,000
    // over nothing share another.
    let original = socket(S08X, "BOLT");
    let mut counts: Vec<usize> = extracted
        .sockets
        .iter()
        .map(|s| s.occurrences.len())
        .collect();
    counts.sort_unstable();
    assert_eq!(counts, [8, 10_000]);
    let on_plate = extracted
        .sockets
        .iter()
        .find(|s| s.occurrences.len() == 8)
        .unwrap();
    assert_eq!(
        on_plate.requirements,
        original.extracted().unwrap().sockets[0].requirements
    );
}

/// The joint 9,000 times more at one place: 36,008 bolts among 90,021
/// occurrences, each bolt at the same place as thousands of others, which
/// a socket must pair one by one. Check 39 refuses it, promptly.
#[test]
fn ten_thousand_coincident_joints_are_refused_by_check_39() {
    let at = vec![[0.0, 300.0, 0.0]; 9_000];
    let text = with_usages(JOINT, &at);
    let bolts = socket(&text, "BOLT");
    assert_eq!(bolts.status(), Status::Refused);
    assert!(
        bolts
            .refusals()
            .iter()
            .any(|r| r.code == RefusalCode::TooManyRelations),
        "{:?}",
        bolts.refusals()
    );
}

/// 1,000 nuts stacked on one axis, each 5 mm tall and touching the next:
/// each nut's hole is continued by the next (J1), and every other nut lies
/// along its axis, so this is read in about 10⁶ pairs. (At 4,000, J's
/// search along the axis returns all 4,000 to each nut, which picks the
/// nearest unblocked only later: check 39 refuses it.)
#[test]
fn a_hole_with_a_thousand_coaxial_neighbours_is_read() {
    let at: Vec<[f64; 3]> = (0..1000)
        .map(|k| [500.0, 0.0, -100.0 - 5.0 * f64::from(k)])
        .collect();
    let text = with_usages(NUT, &at);
    let nuts = socket(&text, "NUT");
    assert_eq!(nuts.status(), Status::Accepted, "{:?}", nuts.refusals());
    let extracted = nuts.extracted().unwrap();
    assert_eq!(extracted.occurrences.len(), 1008);
    let mut sizes: Vec<usize> = extracted
        .sockets
        .iter()
        .map(|s| s.occurrences.len())
        .collect();
    sizes.sort_unstable();
    // s08x's eight, the stack's two ends, and its 998 between.
    assert_eq!(sizes, [1, 1, 8, 998]);
    let middle = extracted
        .sockets
        .iter()
        .find(|s| s.occurrences.len() == 998)
        .unwrap();
    let bore = middle
        .requirements
        .iter()
        .find(|r| r.id.starts_with("bore:"))
        .unwrap();
    assert!(
        bore.sources.iter().any(|s| s.rule == "J1"),
        "{:?}",
        bore.sources
    );
    assert_eq!(
        middle
            .requirements
            .iter()
            .filter(|r| r.id.starts_with("seat:"))
            .count(),
        2
    );
}

/// The committed bracket with `extra` added to its data section; its
/// product definition is `#5`, its context `#2`.
fn bracket_with(extra: &str) -> String {
    let text = include_str!("candidates/m_bracket.stp");
    let end = text.rfind("ENDSEC;").unwrap();
    format!("{}{extra}{}", &text[..end], &text[end..])
}

/// PR2's bounds hold while reading (the PR 5 reviews): 4,000 user-defined
/// properties sharing one representation of 4,000 items would be 16·10⁶
/// values of 200 bytes, more than 3 GB, read in full; the capture stops at
/// its bound. One item of 1 MB shared by 1,000 properties is measured,
/// never copied, and nothing of it is kept.
#[test]
fn properties_sharing_a_representation_cost_what_is_kept() {
    use fitment_core::{MAX_STATED, StatedProperties};
    let value = "v".repeat(200);
    let mut extra = String::from("#200000=REPRESENTATION('',(");
    for i in 0..4000 {
        let sep = if i == 0 { "" } else { "," };
        write!(extra, "{sep}#{}", 210_000 + i).unwrap();
    }
    extra.push_str("),#2);\n");
    for i in 0..4000 {
        writeln!(
            extra,
            "#{}=DESCRIPTIVE_REPRESENTATION_ITEM('i{i}','{value}');",
            210_000 + i
        )
        .unwrap();
        writeln!(
            extra,
            "#{}=PROPERTY_DEFINITION('p{i}','user defined attribute',#5);",
            300_000 + 2 * i
        )
        .unwrap();
        writeln!(
            extra,
            "#{}=PROPERTY_DEFINITION_REPRESENTATION(#{},#200000);",
            300_001 + 2 * i,
            300_000 + 2 * i
        )
        .unwrap();
    }
    let stated = StatedProperties::read(bracket_with(&extra).as_bytes());
    assert!(stated.truncated);
    assert_eq!(stated.stated.len(), MAX_STATED);
    assert!(stated.stated.iter().all(|p| p.instance == 300_000));

    let mut extra = format!(
        "#200000=REPRESENTATION('',(#200001),#2);\n#200001=DESCRIPTIVE_REPRESENTATION_ITEM('big','{}');\n",
        "x".repeat(1 << 20)
    );
    for i in 0..1000 {
        writeln!(
            extra,
            "#{}=PROPERTY_DEFINITION('p{i}','user defined attribute',#5);",
            300_000 + 2 * i
        )
        .unwrap();
        writeln!(
            extra,
            "#{}=PROPERTY_DEFINITION_REPRESENTATION(#{},#200000);",
            300_001 + 2 * i,
            300_000 + 2 * i
        )
        .unwrap();
    }
    let stated = StatedProperties::read(bracket_with(&extra).as_bytes());
    assert!(stated.truncated);
    assert_eq!(stated.stated, Vec::new());
}
