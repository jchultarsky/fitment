//! A part's features, as `features.v1` JSON, against the answers a test
//! manifest states from the construction (`tools/gen-corpus.py`). Shared by
//! fitment-core's `tests/parts.rs` and the CLI's `tests/corpus.rs`, which
//! includes this file by path.
//!
//! What is compared (design, section 13.2):
//!
//! - planes one to one: normal, offset, face count and shell;
//! - the counts of cones, tori and other faces;
//! - every face in exactly one primary feature;
//! - cylinders one to one: each hole or shaft segment a full cylinder of
//!   its diameter on its axis line, over its extent where the hole is
//!   known; with one irregular end, on the side the manifest names, or
//!   undecided with two, where it says so;
//!   each partial cylinder with its sweep; and no other cylinder.

use serde_json::Value;

/// Lengths agree within this, in mm (design, section 13.2).
pub const LENGTH_MM: f64 = 1e-6;

/// Unit directions agree within this. The manifest rounds values to 9
/// decimals, which moves a unit vector by up to 9e-10.
pub const DIRECTION: f64 = 2e-9;

/// Partial sweeps agree within this, in degrees.
const SWEEP_DEG: f64 = 1e-6;

fn numbers(value: &Value) -> Vec<f64> {
    value
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_f64).collect())
        .unwrap_or_default()
}

fn dot(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// The distance between unit directions `a` and `b`, ignoring sign if
/// `either_sign`.
fn apart(a: &[f64], b: &[f64], either_sign: bool) -> f64 {
    let sign = if either_sign && dot(a, b) < 0.0 {
        -1.0
    } else {
        1.0
    };
    a.iter()
        .zip(b)
        .map(|(x, y)| (x - sign * y) * (x - sign * y))
        .sum::<f64>()
        .sqrt()
}

/// True if the lines through `p` along unit `d` and through `q` along unit
/// `e` are one line.
fn same_line(p: &[f64], d: &[f64], q: &[f64], e: &[f64]) -> bool {
    let off: Vec<f64> = q.iter().zip(p).map(|(a, b)| a - b).collect();
    let along = dot(&off, d);
    let perpendicular = off
        .iter()
        .zip(d)
        .map(|(o, dd)| (o - along * dd) * (o - along * dd))
        .sum::<f64>()
        .sqrt();
    // Axis points in the manifest are rounded to 9 decimals as well.
    apart(d, e, true) <= 1e-8 && perpendicular <= LENGTH_MM
}

/// Every disagreement between `part` and the manifest's `expect`.
pub fn features(part: &Value, expect: &Value) -> Vec<String> {
    let mut problems = planes(part, expect);
    for key in ["cones", "tori", "other_faces"] {
        let found = part[key].as_array().map_or(0, Vec::len) as u64;
        if Some(found) != expect[key].as_u64() {
            problems.push(format!("{found} {key}, expected {}", expect[key]));
        }
    }
    problems.extend(partition(part));
    problems.extend(cylinders(part, expect));
    problems.extend(stacks(part, expect, "holes"));
    problems.extend(stacks(part, expect, "shafts"));
    if part["complete"] != expect_complete(expect) {
        problems.push(format!("complete is {}", part["complete"]));
    }
    problems
}

/// The manifest's answer for `complete`: every stack known.
fn expect_complete(expect: &Value) -> bool {
    ["holes", "shafts"]
        .iter()
        .flat_map(|k| expect[*k].as_array().into_iter().flatten())
        .all(|s| s["kind"] != "unknown")
}

fn close(a: &Value, b: f64) -> bool {
    a.as_f64().is_some_and(|a| (a - b).abs() <= LENGTH_MM)
}

/// What is wrong with fitment's `end` against the manifest's `want`, a
/// known stack's end.
fn end_problem(end: &Value, want: &Value) -> Option<String> {
    let bad = |what: &str| Some(format!("{what}: {end} for {want}"));
    if end["kind"] != want["kind"] {
        return bad("kind");
    }
    let num = |v: &Value| v.as_f64().unwrap_or(f64::NAN);
    if want["kind"] == "opening" {
        if !close(&end["rim_mm"], num(&want["rim"])) {
            return bad("rim");
        }
        let sink = &end["countersink"];
        match want.get("countersink") {
            Some(c) => {
                if !close(&sink["outer_diameter_mm"], num(&c["outer_diameter"]))
                    || !close(
                        &(sink["angle_rad"].as_f64().map(f64::to_degrees).into()),
                        num(&c["angle_deg"]),
                    )
                {
                    return bad("countersink");
                }
            }
            None if !sink.is_null() => return bad("countersink"),
            None => {}
        }
        return None;
    }
    // A bottom.
    let bottom = &end["bottom"];
    if !close(&end["at_mm"], num(&want["at"])) || bottom["kind"] != want["bottom"] {
        return bad("bottom");
    }
    let degrees = |v: &Value| v.as_f64().map_or(f64::NAN, f64::to_degrees);
    let ok = match want["bottom"].as_str() {
        Some("point") => {
            (degrees(&bottom["angle_rad"]) - num(&want["angle_deg"])).abs() <= 1e-6
                && close(&bottom["tip_length_mm"], num(&want["tip_length"]))
        }
        Some("cone_then_flat") => {
            (degrees(&bottom["angle_rad"]) - num(&want["angle_deg"])).abs() <= 1e-6
                && close(&bottom["floor_at_mm"], num(&want["floor_at"]))
        }
        Some("annular") => {
            close(&bottom["floor_at_mm"], num(&want["floor_at"]))
                && (2.0 * num(&bottom["post_radius_mm"]) - num(&want["post_diameter"])).abs()
                    <= LENGTH_MM
        }
        _ => true,
    };
    if ok { None } else { bad("bottom") }
}

/// Fitment's holes or shafts (`key`) one to one against the manifest's.
fn stacks(part: &Value, expect: &Value, key: &str) -> Vec<String> {
    let mut problems = Vec::new();
    let mut pool: Vec<&Value> = part[key].as_array().into_iter().flatten().collect();
    for (i, want) in expect[key].as_array().into_iter().flatten().enumerate() {
        let label = format!("{} {i}", &key[..key.len() - 1]);
        let ends: Vec<&Value> = want["ends"].as_array().into_iter().flatten().collect();
        if want["kind"] == "unknown" && ends.iter().filter(|e| **e == "irregular").count() == 2 {
            // Both ends irregular: the cylinder is undecided, and no stack
            // is read (the withdrawn rule F3).
            continue;
        }
        if let Some(at) = pool.iter().position(|s| stack_problem(s, want).is_none()) {
            pool.remove(at);
        } else {
            let why = pool
                .iter()
                .find_map(|s| stack_problem(s, want))
                .unwrap_or_else(|| "none read".to_owned());
            problems.push(format!("{label}: no match for {want}; nearest: {why}"));
        }
    }
    for extra in pool {
        problems.push(format!("a {key} the construction has not: {}", extra["id"]));
    }
    problems
}

/// Shaft ends follow from the kind and lengths the manifest states: a
/// root at 0, free ends at 0 and at the length or protrusion.
fn shaft_ends_agree(s: &Value, want: &Value) -> bool {
    let ends: Vec<&Value> = s["ends"].as_array().into_iter().flatten().collect();
    let far = want
        .get("protrusion")
        .or_else(|| want.get("length"))
        .and_then(Value::as_f64)
        .unwrap_or(f64::NAN);
    let first = if want["kind"] == "root_free" {
        "root"
    } else {
        "free_end"
    };
    matches!(ends[..], [a, b]
        if a["kind"] == first && b["kind"] == "free_end"
            && close(&a["rim_mm"], 0.0) && close(&b["rim_mm"], far))
}

/// What is wrong with fitment's unknown stack `s` against the manifest's
/// `want`: its diameter, axis line, ends in axis order, and where it is
/// entered.
fn unknown_problem(s: &Value, want: &Value, point: &[f64], direction: &[f64]) -> Option<String> {
    let bad = |what: &str| Some(format!("{}: {what}", s["id"]));
    let first = &s["segments"][0];
    let diameter = first["diameter_mm"].as_f64().unwrap_or(f64::NAN);
    if (diameter - want["diameter"].as_f64().unwrap_or(f64::NAN)).abs() > LENGTH_MM
        || !same_line(
            &numbers(&want["axis_line_point"]),
            &numbers(&want["axis_direction"]),
            point,
            direction,
        )
    {
        return bad("diameter or axis line");
    }
    let mut wanted: Vec<&str> = want["ends"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    if dot(direction, &numbers(&want["axis_direction"])) < 0.0 {
        wanted.reverse();
    }
    let found: Vec<&str> = s["ends"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|e| e["kind"].as_str())
        .collect();
    if found != wanted {
        return Some(format!("{}: ends {found:?}, wanted {wanted:?}", s["id"]));
    }
    // The manifest's point is where the hole is entered: the opening's
    // rim lies there.
    let entry = numbers(&want["axis_line_point"]);
    for end in s["ends"].as_array().into_iter().flatten() {
        if end["kind"] == "opening" {
            let rim = end["rim_mm"].as_f64().unwrap_or(f64::NAN);
            let at: Vec<f64> = point
                .iter()
                .zip(direction)
                .map(|(p, d)| p + rim * d)
                .collect();
            if at
                .iter()
                .zip(&entry)
                .any(|(a, b)| (a - b).abs() > LENGTH_MM)
            {
                return bad("opening not at the entry");
            }
        }
    }
    None
}

/// What is wrong with fitment's stack `s` against the manifest's `want`.
fn stack_problem(s: &Value, want: &Value) -> Option<String> {
    let bad = |what: &str| Some(format!("{}: {what}", s["id"]));
    if s["kind"] != want["kind"] {
        return bad("kind");
    }
    let point = numbers(&s["axis"]["point_mm"]);
    let direction = numbers(&s["axis"]["direction"]);
    if want["kind"] == "unknown" {
        return unknown_problem(s, want, &point, &direction);
    }
    if apart(&direction, &numbers(&want["axis_direction"]), false) > DIRECTION
        || point
            .iter()
            .zip(numbers(&want["axis_point"]))
            .any(|(a, b)| (a - b).abs() > LENGTH_MM)
    {
        return bad("axis");
    }
    let segments: Vec<Vec<f64>> = s["segments"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|g| {
            vec![
                g["diameter_mm"].as_f64().unwrap_or(f64::NAN),
                g["from_mm"].as_f64().unwrap_or(f64::NAN),
                g["to_mm"].as_f64().unwrap_or(f64::NAN),
            ]
        })
        .collect();
    let wanted: Vec<Vec<f64>> = want["segments"]
        .as_array()
        .into_iter()
        .flatten()
        .map(numbers)
        .collect();
    if segments.len() != wanted.len()
        || segments
            .iter()
            .zip(&wanted)
            .any(|(a, b)| a.iter().zip(b).any(|(x, y)| (x - y).abs() > LENGTH_MM))
    {
        return bad("segments");
    }
    for (field, key) in [
        ("length_mm", "length"),
        ("depth_mm", "depth"),
        ("protrusion_mm", "protrusion"),
    ] {
        if let Some(w) = want.get(key).and_then(Value::as_f64) {
            if !close(&s[field], w) {
                return bad(field);
            }
        }
    }
    if want.get("ends").is_none() && !shaft_ends_agree(s, want) {
        return bad("shaft ends");
    }
    if let Some(ends) = want["ends"].as_array() {
        for (end, w) in s["ends"].as_array().into_iter().flatten().zip(ends) {
            if let Some(problem) = end_problem(end, w) {
                return Some(format!("{}: {problem}", s["id"]));
            }
        }
    }
    None
}

fn planes(part: &Value, expect: &Value) -> Vec<String> {
    let mut problems = Vec::new();
    let mut planes: Vec<&Value> = part["planes"].as_array().into_iter().flatten().collect();
    for plane in expect["planes"].as_array().into_iter().flatten() {
        let normal = numbers(&plane["normal"]);
        let found = planes.iter().position(|p| {
            apart(&numbers(&p["normal"]), &normal, false) <= DIRECTION
                && (p["offset_mm"].as_f64().unwrap_or(f64::NAN)
                    - plane["offset"].as_f64().unwrap_or(f64::NAN))
                .abs()
                    <= LENGTH_MM
                && p["faces"].as_array().map(Vec::len).map(|n| n as u64) == plane["faces"].as_u64()
                && p["shell"] == plane["shell"]
        });
        match found {
            Some(i) => {
                planes.remove(i);
            }
            None => problems.push(format!("no plane {plane}")),
        }
    }
    for extra in planes {
        problems.push(format!("a plane the construction has not: {}", extra["id"]));
    }
    problems
}

/// What breaks the face partition: a face in two primary features, or a
/// count of faces that differs from the solid's.
pub fn partition(part: &Value) -> Option<String> {
    let list = |value: &Value| value.as_array().cloned().unwrap_or_default();
    let mut faces: Vec<u64> = Vec::new();
    for plane in list(&part["planes"]) {
        faces.extend(
            list(&plane["faces"])
                .iter()
                .filter_map(|f| f["face"].as_u64()),
        );
    }
    for key in ["cylinders", "cones", "tori"] {
        for feature in list(&part[key]) {
            faces.extend(list(&feature["faces"]).iter().filter_map(Value::as_u64));
        }
    }
    faces.extend(
        list(&part["other_faces"])
            .iter()
            .filter_map(|f| f["face"].as_u64()),
    );
    let count = faces.len() as u64;
    faces.sort_unstable();
    faces.dedup();
    let total = part["face_counts"]["total"].as_u64();
    let solid = part["solids"][0]["faces"].as_u64();
    (faces.len() as u64 != count || Some(count) != total || total != solid).then(|| {
        format!(
            "the features hold {count} faces, {} distinct, of {total:?} ({solid:?} in the solid)",
            faces.len()
        )
    })
}

/// One cylinder the construction says is there.
struct Wanted {
    label: String,
    diameter: f64,
    point: Vec<f64>,
    direction: Vec<f64>,
    hole: bool,
    /// Where it runs along `direction` from `point`, for a known stack.
    extent: Option<(f64, f64)>,
    /// The class `kind` it must have.
    class: &'static str,
    /// For a partial cylinder, its sweep in degrees.
    sweep_deg: Option<f64>,
    /// For one irregular end: true if it is the first along `direction`.
    irregular_first: Option<bool>,
}

fn wanted(expect: &Value) -> Vec<Wanted> {
    let mut out = Vec::new();
    for (key, hole) in [("holes", true), ("shafts", false)] {
        for (i, stack) in expect[key].as_array().into_iter().flatten().enumerate() {
            let label = format!("{} {i}", &key[..key.len() - 1]);
            if stack["kind"] == "unknown" {
                let ends: Vec<&Value> = stack["ends"].as_array().into_iter().flatten().collect();
                let irregular = ends.iter().filter(|e| **e == "irregular").count();
                out.push(Wanted {
                    label,
                    diameter: stack["diameter"].as_f64().unwrap_or(f64::NAN),
                    point: numbers(&stack["axis_line_point"]),
                    direction: numbers(&stack["axis_direction"]),
                    hole,
                    extent: None,
                    class: ["full", "full_one_end_irregular", "undecided"][irregular.min(2)],
                    sweep_deg: None,
                    irregular_first: (irregular == 1)
                        .then(|| ends.first().is_some_and(|e| *e == "irregular")),
                });
                continue;
            }
            for segment in stack["segments"].as_array().into_iter().flatten() {
                let s = numbers(segment);
                out.push(Wanted {
                    label: label.clone(),
                    diameter: s.first().copied().unwrap_or(f64::NAN),
                    point: numbers(&stack["axis_point"]),
                    direction: numbers(&stack["axis_direction"]),
                    hole,
                    extent: Some((
                        s.get(1).copied().unwrap_or(f64::NAN),
                        s.get(2).copied().unwrap_or(f64::NAN),
                    )),
                    class: "full",
                    sweep_deg: None,
                    irregular_first: None,
                });
            }
        }
    }
    for (i, partial) in expect["partial_cylinders"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
    {
        out.push(Wanted {
            label: format!("partial {i}"),
            diameter: partial["diameter"].as_f64().unwrap_or(f64::NAN),
            point: numbers(&partial["axis_line_point"]),
            direction: numbers(&partial["axis_direction"]),
            hole: partial["concave"] == true,
            extent: None,
            class: "partial",
            sweep_deg: partial["sweep_deg"].as_f64(),
            irregular_first: None,
        });
    }
    out
}

fn matches(cylinder: &Value, want: &Wanted) -> bool {
    let point = numbers(&cylinder["axis"]["point_mm"]);
    let direction = numbers(&cylinder["axis"]["direction"]);
    let class = &cylinder["class"];
    if (2.0 * cylinder["radius_mm"].as_f64().unwrap_or(f64::NAN) - want.diameter).abs() > LENGTH_MM
        || (cylinder["sense"] == "hole") != want.hole
        || class["kind"] != want.class
        || !same_line(&want.point, &want.direction, &point, &direction)
    {
        return false;
    }
    if let Some(first) = want.irregular_first {
        // The manifest's ends run along its direction, fitment's along its
        // canonical one.
        let along = dot(&direction, &want.direction) > 0.0;
        let start = if along { first } else { !first };
        if class["irregular"] != if start { "start" } else { "finish" } {
            return false;
        }
    }
    if let Some(sweep) = want.sweep_deg {
        let found = class["sweep_rad"].as_f64().map(f64::to_degrees);
        if found.is_none_or(|f| (f - sweep).abs() > SWEEP_DEG) {
            return false;
        }
    }
    if let Some((from, to)) = want.extent {
        let offset: Vec<f64> = point.iter().zip(&want.point).map(|(a, b)| a - b).collect();
        let base = dot(&offset, &want.direction);
        let sign = if dot(&direction, &want.direction) > 0.0 {
            1.0
        } else {
            -1.0
        };
        let mut stations: Vec<f64> = numbers(&cylinder["stations_mm"])
            .iter()
            .map(|s| base + sign * s)
            .collect();
        stations.sort_by(f64::total_cmp);
        let (Some(lo), Some(hi)) = (stations.first(), stations.last()) else {
            return false;
        };
        if (lo - from).abs() > LENGTH_MM || (hi - to).abs() > LENGTH_MM {
            return false;
        }
    }
    true
}

fn cylinders(part: &Value, expect: &Value) -> Vec<String> {
    let mut problems = Vec::new();
    let mut pool: Vec<&Value> = part["cylinders"].as_array().into_iter().flatten().collect();
    for want in wanted(expect) {
        match pool.iter().position(|c| matches(c, &want)) {
            Some(i) => {
                pool.remove(i);
            }
            None => problems.push(format!(
                "{}: no {} {} cylinder of dia {} on the axis through {:?} along {:?}{}{}",
                want.label,
                want.class,
                if want.hole { "hole" } else { "shaft" },
                want.diameter,
                want.point,
                want.direction,
                want.extent
                    .map(|e| format!(" over {e:?}"))
                    .unwrap_or_default(),
                want.sweep_deg
                    .map(|s| format!(" sweeping {s} deg"))
                    .unwrap_or_default(),
            )),
        }
    }
    for extra in pool {
        problems.push(format!(
            "a cylinder the construction has not: {} {} {}",
            extra["id"], extra["sense"], extra["class"]
        ));
    }
    problems
}
