//! The comparison of an assembly read with what its test case's
//! construction says (`tools/gen-corpus.py`, the assembly section;
//! `docs/research/m3-socket.md`, section 13): its parts list, every leaf
//! occurrence's transform, the assembly gate, and the selections to try.
//! Shared by fitment-core's tests (the committed set) and fitment's
//! (the corpus).

use fitment_core::{AssemblyRead, DefinitionKind, Listing, PartsDocument, read_assembly};
use serde_json::Value;

/// The assembly gate checks built so far (`m3-socket.md`, 4.1).
pub const BUILT_CHECK: u64 = 38;

/// How far a transform entry may differ from the construction's (P9): its
/// values are rounded to 9 decimals, and Open CASCADE writes 12 to 15
/// significant digits. The worst measured is 4.5e-10, twelve levels deep.
const TRANSFORM_TOL: f64 = 1e-9;

fn product_id(listing: &Listing, definition: usize) -> Option<&str> {
    listing
        .definitions()
        .get(definition)?
        .definition
        .product
        .as_ref()?
        .id
        .as_deref()
}

/// The distinct refusal codes, in order.
fn codes(refusals: &[fitment_core::Refusal]) -> Vec<String> {
    let mut codes: Vec<String> = refusals.iter().map(|r| r.code.name().to_owned()).collect();
    codes.dedup();
    codes
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}

/// Every way `bytes` differs from what `case` expects; empty if none.
pub fn check(case: &Value, bytes: &[u8]) -> Vec<String> {
    let read = read_assembly(bytes);
    let expect = &case["expect"];
    let mut failures = Vec::new();
    let listing_expect = &expect["listing"];
    if listing_expect["result"] == "refused" {
        if listing_expect["check"].as_u64().unwrap_or(0) <= BUILT_CHECK {
            let wanted = strings(&listing_expect["refusals"]);
            let found = codes(read.refusals());
            if read.listing().is_some() || found != wanted {
                failures.push(format!("listing: refused {found:?}, expected {wanted:?}"));
            }
            // The kind of failure, which the message states first.
            if let Some(kind) = listing_expect["kind"].as_str() {
                let prefix = format!("{kind}: ");
                if !read
                    .refusals()
                    .iter()
                    .any(|r| r.message.starts_with(&prefix))
                {
                    failures.push(format!(
                        "listing: no refusal of kind {kind}: {:?}",
                        read.refusals()
                    ));
                }
            }
        }
        return failures;
    }
    let Some(listing) = read.listing() else {
        return vec![format!("listing: refused {:?}", codes(read.refusals()))];
    };
    failures.extend(parts(listing, listing_expect));
    failures.extend(gate(&read, &expect["assembly_gate"]));
    failures.extend(notes(&read, &expect["notes"]));
    failures.extend(floors(listing, &expect["floors"]));
    failures.extend(components(listing, &expect["components"]));
    failures.extend(sockets(bytes, listing, &expect["sockets"]));
    failures.extend(transforms(listing, &expect["occurrences"]));
    for (k, try_) in expect["select"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
    {
        if let Some(failure) = select(listing, try_) {
            failures.push(format!("select {k}: {failure}"));
        }
    }
    failures.extend(document(bytes, listing));
    failures
}

fn parts(listing: &Listing, expect: &Value) -> Vec<String> {
    let mut failures = Vec::new();
    let form = match listing.path_form() {
        fitment_core::PathForm::UsageId => "usage_id",
        fitment_core::PathForm::Instance => "instance",
    };
    if expect["path_form"] != form {
        failures.push(format!(
            "path form {form}, expected {}",
            expect["path_form"]
        ));
    }
    let mut found: Vec<(String, String, String, usize)> = listing
        .definitions()
        .iter()
        .map(|listed| {
            let product = listed.definition.product.as_ref();
            (
                product.and_then(|p| p.id.clone()).unwrap_or_default(),
                product.and_then(|p| p.name.clone()).unwrap_or_default(),
                match listed.kind {
                    DefinitionKind::Part => "part".to_owned(),
                    DefinitionKind::Assembly => "assembly".to_owned(),
                },
                listed.occurrences.len(),
            )
        })
        .collect();
    let mut wanted: Vec<(String, String, String, usize)> = expect["parts"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|p| {
            (
                p["id"].as_str().unwrap_or_default().to_owned(),
                p["name"].as_str().unwrap_or_default().to_owned(),
                p["kind"].as_str().unwrap_or_default().to_owned(),
                usize::try_from(p["quantity"].as_u64().unwrap_or(0)).unwrap_or(0),
            )
        })
        .collect();
    found.sort();
    wanted.sort();
    if found != wanted {
        failures.push(format!("parts {found:?}, expected {wanted:?}"));
    }
    let root_ok = listing
        .occurrences()
        .iter()
        .filter(|o| o.path.usages().is_empty())
        .map(|o| o.definition)
        .eq([listing.root()]);
    if !root_ok {
        failures.push("the root is not the one occurrence with the empty path".to_owned());
    }
    failures
}

fn notes(read: &AssemblyRead, expect: &Value) -> Vec<String> {
    let mut found: Vec<String> = read
        .notes()
        .iter()
        .filter_map(|n| serde_json::to_value(n.code).ok())
        .filter_map(|v| v.as_str().map(str::to_owned))
        .collect();
    found.dedup();
    let wanted = strings(expect);
    if found == wanted {
        Vec::new()
    } else {
        vec![format!("notes {found:?}, expected {wanted:?}")]
    }
}

/// Z1: every leaf occurrence of an accepted part carries the uncertainty
/// its part's context and its placement path state.
fn floors(listing: &Listing, expect: &Value) -> Vec<String> {
    let (Some(part), Some(placement)) =
        (expect["part_mm"].as_f64(), expect["placement_mm"].as_f64())
    else {
        return vec!["no expected floors".to_owned()];
    };
    let accepted = |o: &fitment_core::Occurrence| {
        matches!(
            listing
                .definitions()
                .get(o.definition)
                .and_then(|d| d.read.as_ref()),
            Some(fitment_core::ComponentRead::Accepted(_))
        )
    };
    listing
        .occurrences()
        .iter()
        .filter(|o| accepted(o))
        .filter(|o| {
            let offset = match listing
                .definitions()
                .get(o.definition)
                .and_then(|d| d.read.as_ref())
            {
                Some(fitment_core::ComponentRead::Accepted(read)) => read
                    .solids
                    .first()
                    .map_or(f64::NAN, |s| s.max_vertex_offset_mm),
                _ => f64::NAN,
            };
            o.part_uncertainty_mm.to_bits() != part.to_bits()
                || o.placement_uncertainty_mm.to_bits() != placement.to_bits()
                || o.vertex_offset_mm.to_bits() != offset.to_bits()
        })
        .map(|o| {
            format!(
                "{}: floors {} and {}, expected {part} and {placement}",
                o.path.display(listing.path_form()),
                o.part_uncertainty_mm,
                o.placement_uncertainty_mm
            )
        })
        .collect()
}

/// What some components read as: their counts of holes and shafts.
fn components(listing: &Listing, expect: &Value) -> Vec<String> {
    let Some(wanted) = expect.as_object() else {
        return Vec::new();
    };
    let mut failures = Vec::new();
    for (product, counts) in wanted {
        let read = listing
            .definitions()
            .iter()
            .find(|d| d.definition.product.as_ref().and_then(|p| p.id.as_deref()) == Some(product))
            .and_then(|d| d.read.as_ref());
        let Some(fitment_core::ComponentRead::Accepted(part)) = read else {
            failures.push(format!("{product}: not read as an accepted part"));
            continue;
        };
        let found = (part.holes.len() as u64, part.shafts.len() as u64);
        let expected = (
            counts["holes"].as_u64().unwrap_or(u64::MAX),
            counts["shafts"].as_u64().unwrap_or(u64::MAX),
        );
        if found != expected {
            failures.push(format!(
                "{product}: (holes, shafts) {found:?}, expected {expected:?}"
            ));
        }
    }
    failures
}

fn gate(read: &AssemblyRead, expect: &Value) -> Vec<String> {
    let found = codes(read.gate_refusals());
    let wanted = if expect["result"] == "accepted" {
        Vec::new()
    } else if expect["check"].as_u64().unwrap_or(0) > BUILT_CHECK {
        return Vec::new();
    } else {
        strings(&expect["refusals"])
    };
    let mut failures = Vec::new();
    if found != wanted {
        failures.push(format!("assembly gate {found:?}, expected {wanted:?}"));
    }
    // The kind of failure, and the component, which a component's refusal
    // names: `component #<n> ('<id>', ...): <codes>`.
    if let Some(kind) = expect["kind"].as_str() {
        let component = expect["component"].as_str().map(|id| format!("('{id}',"));
        let named = read.gate_refusals().iter().any(|r| {
            r.message.contains(kind)
                && component
                    .as_ref()
                    .is_none_or(|c| r.message.contains(c.as_str()))
        });
        if !named {
            failures.push(format!(
                "assembly gate: no refusal names {kind} for {}",
                expect["component"]
            ));
        }
    }
    failures
}

/// Every leaf occurrence's transform, one to one per product, as
/// multisets: the order of occurrences is the file's, not the
/// construction's.
fn transforms(listing: &Listing, expect: &Value) -> Vec<String> {
    let mut failures = Vec::new();
    let Some(wanted) = expect.as_object() else {
        return vec!["no expected occurrences".to_owned()];
    };
    let leaves = listing
        .occurrences()
        .iter()
        .filter(|o| {
            listing
                .definitions()
                .get(o.definition)
                .is_some_and(|d| d.kind == DefinitionKind::Part)
        })
        .count();
    let expected_count: usize = wanted
        .values()
        .filter_map(Value::as_array)
        .map(Vec::len)
        .sum();
    if leaves != expected_count {
        failures.push(format!(
            "{leaves} leaf occurrences, expected {expected_count}"
        ));
    }
    for (product, matrices) in wanted {
        let mut found: Vec<([[f64; 3]; 3], [f64; 3])> = listing
            .occurrences()
            .iter()
            .filter(|o| product_id(listing, o.definition) == Some(product.as_str()))
            .map(|o| (o.transform.rotation(), o.transform.translation()))
            .collect();
        let matrices = matrices.as_array().cloned().unwrap_or_default();
        if found.len() != matrices.len() {
            failures.push(format!(
                "{product}: {} occurrences, expected {}",
                found.len(),
                matrices.len()
            ));
            continue;
        }
        for matrix in matrices {
            let entry = |i: usize, j: usize| matrix[i][j].as_f64().unwrap_or(f64::NAN);
            let close = |(rotation, translation): &([[f64; 3]; 3], [f64; 3])| {
                (0..3).all(|i| {
                    (0..3).all(|j| (rotation[i][j] - entry(i, j)).abs() <= TRANSFORM_TOL)
                        && (translation[i] - entry(i, 3)).abs() <= TRANSFORM_TOL
                })
            };
            match found.iter().position(close) {
                Some(at) => {
                    found.swap_remove(at);
                }
                None => failures.push(format!("{product}: no occurrence at {matrix}")),
            }
        }
    }
    failures
}

/// The ids of the candidates an error offers.
fn candidate_ids(error: &fitment_core::SelectError) -> Vec<String> {
    error
        .candidates()
        .iter()
        .map(|c| {
            c.definition
                .product
                .as_ref()
                .and_then(|p| p.id.clone())
                .unwrap_or_default()
        })
        .collect()
}

/// One selection, against what it must choose or the error it must give.
/// `occurrence_of` names a product, and `nth` (default 0) which of its
/// occurrences, by path, to give as `--occurrence`.
fn select(listing: &Listing, try_: &Value) -> Option<String> {
    let part = try_["part"].as_str();
    let occurrence = match (&try_["occurrence"], try_["occurrence_of"].as_str()) {
        (Value::String(text), _) => Some(text.clone()),
        (_, Some(product)) => {
            let nth = usize::try_from(try_["nth"].as_u64().unwrap_or(0)).unwrap_or(usize::MAX);
            let Some(chosen) = listing
                .occurrences()
                .iter()
                .filter(|o| product_id(listing, o.definition) == Some(product))
                .nth(nth)
            else {
                return Some(format!("no occurrence {nth} of {product} to give"));
            };
            Some(chosen.path.display(listing.path_form()))
        }
        _ => None,
    };
    let result = listing.select(part, occurrence.as_deref());
    match (result, try_["error"].as_str()) {
        (Ok(selection), None) => {
            let chosen = product_id(listing, selection.definition);
            let matched_by = serde_json::to_value(selection.matched_by).ok();
            let count = u64::try_from(selection.occurrences.len()).ok();
            if chosen != try_["result"].as_str()
                || count != try_["occurrences"].as_u64()
                || matched_by.as_ref() != Some(&try_["matched_by"])
            {
                return Some(format!(
                    "chose {chosen:?} ({count:?} occurrences, {matched_by:?}), expected {try_}"
                ));
            }
            // The occurrence named is the one chosen, not just one of the
            // right part.
            if let Some(text) = &occurrence {
                let named: Vec<String> = selection
                    .occurrences
                    .iter()
                    .filter_map(|&i| listing.occurrences().get(i))
                    .map(|o| o.path.display(listing.path_form()))
                    .collect();
                if named != [text.clone()] {
                    return Some(format!("'{text}' chose {named:?}"));
                }
            }
            None
        }
        (Ok(selection), Some(error)) => Some(format!(
            "chose {:?}, expected {error}",
            product_id(listing, selection.definition)
        )),
        (Err(found), None) => Some(format!("{}: {found}, expected {try_}", found.code())),
        (Err(found), Some(error)) => {
            if found.code() != error {
                return Some(format!("{}, expected {error}", found.code()));
            }
            let ids = candidate_ids(&found);
            match try_.get("suggestions") {
                Some(wanted) if ids != strings(wanted) => {
                    Some(format!("offered {ids:?}, expected {wanted}"))
                }
                _ => None,
            }
        }
    }
}

/// The parts document reads back, the same bytes give the same text, and
/// it says what the listing says: every definition's kind and quantity,
/// and every occurrence's path, labels and transform (O5).
fn document(bytes: &[u8], listing: &Listing) -> Vec<String> {
    let document = PartsDocument::read(bytes);
    let text = document.to_json();
    let mut failures = Vec::new();
    if let Err(error) = PartsDocument::from_json(&text) {
        failures.push(format!("the parts document does not read back: {error}"));
    }
    if PartsDocument::read(bytes).to_json() != text {
        failures.push("the parts document differs between two reads".to_owned());
    }
    if document.parts().len() != listing.definitions().len() {
        failures.push("the parts document lists another number of definitions".to_owned());
    }
    for (entry, listed) in document.parts().iter().zip(listing.definitions()) {
        let instance = listed.definition.instance;
        if entry.definition != listed.definition || entry.kind != listed.kind {
            failures.push(format!("the parts document misstates #{instance}"));
        }
        let occurrences: Vec<_> = listed
            .occurrences
            .iter()
            .filter_map(|&i| listing.occurrences().get(i))
            .collect();
        if usize::try_from(entry.quantity).ok() != Some(occurrences.len())
            || entry.occurrences.len() != occurrences.len()
        {
            failures.push(format!("the parts document miscounts #{instance}"));
            continue;
        }
        for (written, occurrence) in entry.occurrences.iter().zip(occurrences) {
            let labels: Vec<Option<String>> = occurrence
                .path
                .usages()
                .iter()
                .map(|u| u.id.clone())
                .collect();
            if written.path != occurrence.path.instances()
                || written.label != labels
                || written.transform != occurrence.transform
            {
                failures.push(format!(
                    "the parts document misstates an occurrence of #{instance}: {:?}",
                    written.path
                ));
            }
        }
    }
    failures
}

/// How far an expected point may lie from fitment's: answers are rounded
/// to 9 decimals.
const POINT_TOL: f64 = 1e-6;

fn point(value: &Value) -> Option<[f64; 3]> {
    let a = value.as_array()?;
    Some([
        a.first()?.as_f64()?,
        a.get(1)?.as_f64()?,
        a.get(2)?.as_f64()?,
    ])
}

fn near(a: [f64; 3], b: [f64; 3]) -> bool {
    (0..3).all(|i| (a[i] - b[i]).abs() <= POINT_TOL)
}

/// True if the line through `p` along unit `d` holds the expected axis.
fn same_line(p: [f64; 3], d: [f64; 3], expect: &Value) -> bool {
    let (Some(q), Some(e)) = (
        point(&expect["axis_point"]),
        point(&expect["axis_direction"]),
    ) else {
        return false;
    };
    let off = [q[0] - p[0], q[1] - p[1], q[2] - p[2]];
    let along = off[0] * d[0] + off[1] * d[1] + off[2] * d[2];
    let across = (0..3)
        .map(|i| (off[i] - along * d[i]) * (off[i] - along * d[i]))
        .sum::<f64>()
        .sqrt();
    let cross = [
        d[1] * e[2] - d[2] * e[1],
        d[2] * e[0] - d[0] * e[2],
        d[0] * e[1] - d[1] * e[0],
    ];
    across <= POINT_TOL && cross.iter().all(|c| c.abs() <= 1e-9)
}

/// The rims of a target hole's open ends, as points: from its part.
fn rims(
    part: &fitment_core::Part,
    hole: fitment_core::FeatureId,
    open: fitment_core::Open,
) -> Vec<[f64; 3]> {
    // A cylinder in no stack has no rim: any open end is wrong.
    let Some(stack) = part.holes.iter().find(|h| h.id == hole) else {
        return [open.start, open.finish]
            .into_iter()
            .filter(|w| *w)
            .map(|_| [f64::NAN; 3])
            .collect();
    };
    let (p, d) = (
        stack.axis.point_mm.to_array(),
        stack.axis.direction.to_array(),
    );
    let at = |end: Option<&fitment_core::StackEnd>| match end {
        Some(fitment_core::StackEnd::Opening { rim_mm, .. }) => Some(*rim_mm),
        _ => None,
    };
    let mut out = Vec::new();
    for (wanted, end) in [
        (open.start, stack.ends.first()),
        (open.finish, stack.ends.get(1)),
    ] {
        if wanted {
            let s = at(end).unwrap_or(f64::NAN);
            out.push([p[0] + s * d[0], p[1] + s * d[1], p[2] + s * d[2]]);
        }
    }
    out
}

fn fit_name(fit: fitment_core::Fit) -> &'static str {
    match fit {
        fitment_core::Fit::Clearance => "clearance",
        fitment_core::Fit::NotRead => "not_read",
        fitment_core::Fit::LineToLine => "line_to_line",
        fitment_core::Fit::Interference => "interference",
    }
}

/// A span as fitment states it: stations, diameter, clear diameter, fit
/// and whether it is pinned.
type Span = (f64, f64, f64, Option<f64>, fitment_core::Fit, bool);

/// The spans of an axis through `p` along `d` against the expected ones,
/// one to one.
// p, d, a, b, x, y: an axis, a span's ends and an expected span's.
#[allow(clippy::many_single_char_names)]
fn spans_match(p: [f64; 3], d: [f64; 3], spans: &[Span], expected: &[Value]) -> Option<String> {
    let at = |s: f64| [p[0] + s * d[0], p[1] + s * d[1], p[2] + s * d[2]];
    let mut left: Vec<&Value> = expected.iter().collect();
    for (from, to, diameter, clear, fit, pinned) in spans {
        let (a, b) = (at(*from), at(*to));
        let found = left.iter().position(|e| {
            let (Some(x), Some(y)) = (point(&e["from"]), point(&e["to"])) else {
                return false;
            };
            let ends = (near(a, x) && near(b, y)) || (near(a, y) && near(b, x));
            let clear_ok = match (clear, e["clear"].as_f64()) {
                (None, None) => true,
                (Some(c), Some(w)) => (c - w).abs() <= POINT_TOL,
                _ => false,
            };
            ends && clear_ok
                && (diameter - e["diameter"].as_f64().unwrap_or(f64::NAN)).abs() <= POINT_TOL
                && e["fit"] == fit_name(*fit)
                && e["pinned"] == *pinned
        });
        match found {
            Some(i) => {
                left.swap_remove(i);
            }
            None => {
                return Some(format!(
                    "an unexpected span {from}..{to} Ø{diameter} {fit:?} clear {clear:?}"
                ));
            }
        }
    }
    if !left.is_empty() {
        return Some(format!("spans not found: {left:?}"));
    }
    None
}

/// One expected bore or shaft against one requirement: `None` if they
/// agree, else why not.
fn axial_matches(
    part: &fitment_core::Part,
    requirement: &fitment_core::Requirement,
    expect: &Value,
) -> Option<String> {
    use fitment_core::Requires;
    let (kind, feature, axis, spans, open): (&str, _, _, Vec<Span>, _) = match &requirement.requires
    {
        Requires::Bore {
            hole,
            axis,
            spans,
            open,
            depth,
            ..
        } => {
            let depth = serde_json::to_value(depth).ok()?;
            if expect.get("depth").is_some_and(|d| *d != depth) {
                return Some(format!("depth {depth}, expected {}", expect["depth"]));
            }
            (
                "bore",
                *hole,
                axis,
                spans
                    .iter()
                    .map(|s| {
                        (
                            s.from_mm,
                            s.to_mm,
                            s.hole_diameter_mm,
                            s.clear_diameter_mm,
                            s.fit,
                            s.pinned,
                        )
                    })
                    .collect(),
                Some(*open),
            )
        }
        Requires::Shaft {
            shaft, axis, spans, ..
        } => (
            "shaft",
            *shaft,
            axis,
            spans
                .iter()
                .map(|s| {
                    (
                        s.from_mm,
                        s.to_mm,
                        s.shaft_diameter_mm,
                        s.clear_diameter_mm,
                        s.fit,
                        s.pinned,
                    )
                })
                .collect(),
            None,
        ),
        _ => return Some("not a bore or shaft".to_owned()),
    };
    if expect["kind"] != kind {
        return Some("another kind".to_owned());
    }
    let (p, d) = (axis.point_mm.to_array(), axis.direction.to_array());
    if !same_line(p, d, expect) {
        return Some("another axis".to_owned());
    }
    let flags: Vec<String> = requirement
        .flags
        .iter()
        .filter_map(|f| serde_json::to_value(f).ok()?.as_str().map(str::to_owned))
        .collect();
    let mut wanted_flags = strings(&expect["flags"]);
    let mut found_flags = flags.clone();
    wanted_flags.sort();
    found_flags.sort();
    if found_flags != wanted_flags {
        return Some(format!("flags {found_flags:?}, expected {wanted_flags:?}"));
    }
    if let Some(expected) = expect["spans"].as_array() {
        if let Some(problem) = spans_match(p, d, &spans, expected) {
            return Some(problem);
        }
    }
    if let Some(open) = open {
        let found = rims(part, feature, open);
        let wanted: Vec<[f64; 3]> = expect["open"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(point)
            .collect();
        let all = found.len() == wanted.len()
            && wanted.iter().all(|w| found.iter().any(|f| near(*f, *w)));
        if !all {
            return Some(format!("open at {found:?}, expected {wanted:?}"));
        }
    }
    None
}

/// One socket's left-out features against the expected ones, one to one.
fn left_out_matches(socket: &fitment_core::Socket, expect: &Value) -> Option<String> {
    let left_out: Vec<&Value> = expect["left_out"]
        .as_array()
        .into_iter()
        .flatten()
        .collect();
    if socket.left_out.len() != left_out.len() {
        return Some(format!(
            "{} left out, expected {}",
            socket.left_out.len(),
            left_out.len()
        ));
    }
    // One to one, with each wall's stations where stated.
    let mut wanted_left = left_out.clone();
    for left in &socket.left_out {
        let reason = serde_json::to_value(left.reason).ok()?;
        let (p, d) = (
            left.axis.point_mm.to_array(),
            left.axis.direction.to_array(),
        );
        let ends =
            [left.from_mm, left.to_mm].map(|s| [p[0] + s * d[0], p[1] + s * d[1], p[2] + s * d[2]]);
        let found = wanted_left.iter().position(|w| {
            same_line(p, d, w)
                && (left.diameter_mm - w["diameter"].as_f64().unwrap_or(f64::NAN)).abs()
                    <= POINT_TOL
                && w["reason"] == reason
                && w["wall"].as_array().is_none_or(|wall| {
                    let wall: Vec<[f64; 3]> = wall.iter().filter_map(point).collect();
                    wall.len() == 2
                        && ((near(ends[0], wall[0]) && near(ends[1], wall[1]))
                            || (near(ends[0], wall[1]) && near(ends[1], wall[0])))
                })
        });
        match found {
            Some(i) => {
                wanted_left.swap_remove(i);
            }
            None => return Some(format!("{} left out unexpectedly ({reason})", left.feature)),
        }
    }
    None
}

/// One expected socket against one socket: `None` if they agree.
fn socket_matches(
    part: &fitment_core::Part,
    socket: &fitment_core::Socket,
    at: Option<[f64; 3]>,
    expect: &Value,
) -> Option<String> {
    use fitment_core::Requires;
    if let Some(wanted) = point(&expect["at"]) {
        if !at.is_some_and(|a| near(a, wanted)) {
            return Some(format!("its occurrence is at {at:?}, expected {wanted:?}"));
        }
    }
    let axial: Vec<&fitment_core::Requirement> = socket
        .requirements
        .iter()
        .filter(|r| matches!(r.requires, Requires::Bore { .. } | Requires::Shaft { .. }))
        .collect();
    let wanted: Vec<&Value> = expect["requirements"]
        .as_array()
        .into_iter()
        .flatten()
        .collect();
    if axial.len() != wanted.len() {
        return Some(format!(
            "{} bores and shafts, expected {}",
            axial.len(),
            wanted.len()
        ));
    }
    let mut left = wanted.clone();
    for requirement in &axial {
        let reasons: Vec<String> = left
            .iter()
            .filter_map(|w| axial_matches(part, requirement, w))
            .collect();
        match left
            .iter()
            .position(|w| axial_matches(part, requirement, w).is_none())
        {
            Some(i) => {
                left.swap_remove(i);
            }
            None => {
                return Some(format!(
                    "{} matches no expected one: {reasons:?}",
                    requirement.id
                ));
            }
        }
    }
    let mut causes: std::collections::BTreeMap<String, u64> = std::collections::BTreeMap::new();
    for requirement in &socket.requirements {
        if let Requires::Unresolved { causes: found, .. } = &requirement.requires {
            for cause in found {
                let name = serde_json::to_value(cause.cause).ok()?.as_str()?.to_owned();
                *causes.entry(name).or_default() += 1;
            }
        }
    }
    let wanted_causes: std::collections::BTreeMap<String, u64> = expect["unresolved"]
        .as_object()
        .into_iter()
        .flatten()
        .map(|(k, v)| (k.clone(), v.as_u64().unwrap_or(0)))
        .collect();
    if causes != wanted_causes {
        return Some(format!("unresolved {causes:?}, expected {wanted_causes:?}"));
    }
    left_out_matches(socket, expect)
}

/// A requirement's place in section 9.2's order, from its id alone.
fn order_key(requirement: &fitment_core::Requirement) -> (bool, u64, u8) {
    let (kind, anchor) = requirement.id.split_once(':').unwrap_or_default();
    let number = anchor
        .rsplit('-')
        .next()
        .and_then(|n| n.parse().ok())
        .unwrap_or(u64::MAX);
    let rank = match kind {
        "bore" => 0,
        "shaft" => 1,
        _ => 9,
    };
    (anchor.starts_with("part-"), number, rank)
}

/// Every expected socket of every target named, one to one.
pub fn sockets(bytes: &[u8], listing: &Listing, expect: &Value) -> Vec<String> {
    let Some(targets) = expect.as_object() else {
        return Vec::new();
    };
    let mut failures = Vec::new();
    for (name, wanted) in targets {
        let document = match fitment_core::SocketDocument::extract(bytes, Some(name), None) {
            Ok(document) => document,
            Err(error) => {
                failures.push(format!("socket of {name}: {error}"));
                continue;
            }
        };
        if let Err(error) = fitment_core::SocketDocument::from_json(&document.to_json()) {
            failures.push(format!("socket of {name} does not read back: {error}"));
        }
        let Some(extracted) = document.extracted() else {
            failures.push(format!(
                "socket of {name}: refused {:?}",
                document.refusals()
            ));
            continue;
        };
        let part = listing
            .definitions()
            .iter()
            .find(|d| {
                d.definition.product.as_ref().and_then(|p| p.id.as_deref()) == Some(name.as_str())
            })
            .and_then(|d| match d.read.as_ref() {
                Some(fitment_core::ComponentRead::Accepted(part)) => Some(part.as_ref()),
                _ => None,
            });
        let Some(part) = part else {
            failures.push(format!("{name}: not an accepted part"));
            continue;
        };
        // V2: the target's extent, as its part reads.
        if extracted.target.extent != part.extent {
            failures.push(format!("{name}: the socket's extent is not its part's"));
        }
        // N5: by smallest anchor #id, then kind; a neighbour definition's
        // unresolved last.
        for socket in &extracted.sockets {
            let keys: Vec<(bool, u64, u8)> = socket.requirements.iter().map(order_key).collect();
            if keys.windows(2).any(|w| w[0] > w[1]) {
                failures.push(format!("{name}: {} is out of order: {keys:?}", socket.id));
            }
        }
        let mut left: Vec<&Value> = wanted.as_array().into_iter().flatten().collect();
        if extracted.sockets.len() != left.len() {
            failures.push(format!(
                "{name}: {} sockets, expected {}",
                extracted.sockets.len(),
                left.len()
            ));
            continue;
        }
        for socket in &extracted.sockets {
            let at = extracted
                .occurrences
                .iter()
                .find(|o| socket.occurrences.first() == Some(&o.path))
                .map(|o| o.transform.translation());
            let reasons: Vec<String> = left
                .iter()
                .filter_map(|w| socket_matches(part, socket, at, w))
                .collect();
            match left
                .iter()
                .position(|w| socket_matches(part, socket, at, w).is_none())
            {
                Some(i) => {
                    left.swap_remove(i);
                }
                None => failures.push(format!(
                    "{name}: {} matches no expected socket: {reasons:?}",
                    socket.id
                )),
            }
        }
    }
    failures
}
