//! P2, P4 and P5 (`docs/research/m3-socket.md`, 6 and 13.4): a socket does
//! not depend on where the assembly is placed (`_rot`) nor on the frame
//! the target is modelled in (`_tgt`, its values moved by that frame).
//! Feature and face ids, neighbour paths and sources differ between files
//! and are left out; axes are compared as lines, with their stations.

use serde_json::{Map, Value};

/// The fields that name features, faces or occurrences of one file.
const DROPPED: [&str; 16] = [
    "instance",
    "id",
    "hole",
    "shaft",
    "plane",
    "cone",
    "cylinder",
    "feature",
    "faces",
    "face",
    "neighbour",
    "occurrence",
    "definition",
    "edge",
    "bound",
    "across",
];

type M = [[f64; 4]; 3];

fn array3(value: &Value) -> Option<[f64; 3]> {
    let a = value.as_array()?;
    Some([
        a.first()?.as_f64()?,
        a.get(1)?.as_f64()?,
        a.get(2)?.as_f64()?,
    ])
}

fn json3(v: [f64; 3]) -> Value {
    Value::from(v.to_vec())
}

fn turn(m: &M, v: [f64; 3]) -> [f64; 3] {
    [0, 1, 2].map(|i| m[i][0] * v[0] + m[i][1] * v[1] + m[i][2] * v[2])
}

fn place(m: &M, p: [f64; 3]) -> [f64; 3] {
    let t = turn(m, p);
    [0, 1, 2].map(|i| t[i] + m[i][3])
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Points and directions moved by `m`; a plane's offset with its normal.
pub fn moved(value: &mut Value, m: &M) {
    match value {
        Value::Object(map) => {
            for (key, item) in map.iter_mut() {
                match (key.as_str(), array3(item)) {
                    ("point_mm" | "apex_mm" | "from_mm" | "to_mm" | "center_mm", Some(p)) => {
                        *item = json3(place(m, p));
                    }
                    ("direction" | "normal", Some(d)) => *item = json3(turn(m, d)),
                    _ => moved(item, m),
                }
            }
            if let (Some(n), Some(offset)) = (
                map.get("normal").and_then(array3),
                map.get("offset_mm").and_then(Value::as_f64),
            ) {
                let shift = [m[0][3], m[1][3], m[2][3]];
                map.insert("offset_mm".to_owned(), Value::from(offset + dot(n, shift)));
            }
        }
        Value::Array(items) => items.iter_mut().for_each(|i| moved(i, m)),
        _ => {}
    }
}

/// An axis as a line: its direction with its first nonzero component
/// positive, its point the foot from the origin; returns the map from old
/// stations to new and whether the direction turned.
fn line(map: &mut Map<String, Value>) -> Option<(f64, f64)> {
    let axis = map.get_mut("axis")?.as_object_mut()?;
    let (p, d) = (
        array3(axis.get("point_mm")?)?,
        array3(axis.get("direction")?)?,
    );
    let first = d.iter().copied().find(|c| c.abs() > 1e-12).unwrap_or(1.0);
    let sign = first.signum();
    let e = d.map(|c| c * sign);
    let along = dot(p, e);
    let foot = [0, 1, 2].map(|i| p[i] - along * e[i]);
    axis.insert("point_mm".to_owned(), json3(foot));
    axis.insert("direction".to_owned(), json3(e));
    Some((along, sign))
}

/// Every axis-bearing object put on its line, its stations moved with it,
/// and its sources' spans too.
pub fn canonical(value: &mut Value) {
    match value {
        Value::Object(map) => {
            if let Some((along, sign)) = map.get("requires").and_then(|r| {
                let mut copy = r.as_object()?.clone();
                line(&mut copy)
            }) {
                for source in map
                    .get_mut("sources")
                    .and_then(Value::as_array_mut)
                    .into_iter()
                    .flatten()
                {
                    if let Some(span) = source.get_mut("span_mm").and_then(Value::as_array_mut) {
                        let mut ends: Vec<f64> = span
                            .iter()
                            .filter_map(Value::as_f64)
                            .map(|s| along + sign * s)
                            .collect();
                        ends.sort_by(f64::total_cmp);
                        *span = ends.into_iter().map(Value::from).collect();
                    }
                }
            }
            for item in map.values_mut() {
                canonical(item);
            }
            let Some((along, sign)) = line(map) else {
                return;
            };
            let station = |s: f64| along + sign * s;
            let swap = |a: &mut Map<String, Value>, x: &str, y: &str| {
                if let (Some(u), Some(v)) = (a.remove(x), a.remove(y)) {
                    a.insert(x.to_owned(), v);
                    a.insert(y.to_owned(), u);
                }
            };
            let restation = |a: &mut Map<String, Value>| {
                for key in ["from_mm", "to_mm"] {
                    if let Some(s) = a.get(key).and_then(Value::as_f64) {
                        a.insert(key.to_owned(), Value::from(station(s)));
                    }
                }
                if sign < 0.0 {
                    swap(a, "from_mm", "to_mm");
                }
            };
            restation(map);
            if let Some(spans) = map.get_mut("spans").and_then(Value::as_array_mut) {
                for span in spans.iter_mut().filter_map(Value::as_object_mut) {
                    restation(span);
                }
                if sign < 0.0 {
                    spans.reverse();
                }
            }
            if sign < 0.0 {
                if let Some(open) = map.get_mut("open").and_then(Value::as_object_mut) {
                    swap(open, "start", "finish");
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(canonical),
        _ => {}
    }
}

/// The values a socket states, without what names one file's entities.
pub fn stripped(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for key in DROPPED {
                map.remove(key);
            }
            // A grip's seats and holes are ids: their count only.
            for key in ["seats", "holes"] {
                if let Some(list) = map.get(key).and_then(Value::as_array) {
                    let count = list.len();
                    map.insert(key.to_owned(), Value::from(count));
                }
            }
            for item in map.values_mut() {
                stripped(item);
            }
        }
        Value::Array(items) => items.iter_mut().for_each(stripped),
        _ => {}
    }
}

/// A sort key: the value with its numbers rounded to a micrometre.
pub fn key(value: &Value) -> String {
    match value {
        // Whole tenths of a micrometre: -1e-14 and 0 are one key.
        #[allow(clippy::cast_possible_truncation)]
        Value::Number(n) => format!("{}", (n.as_f64().unwrap_or(f64::NAN) * 1e4).round() as i64),
        Value::Array(items) => format!("[{}]", items.iter().map(key).collect::<Vec<_>>().join(",")),
        Value::Object(map) => format!(
            "{{{}}}",
            map.iter()
                .map(|(k, v)| format!("{k}:{}", key(v)))
                .collect::<Vec<_>>()
                .join(",")
        ),
        other => other.to_string(),
    }
}

/// Lists of objects sorted by `key`, recursively; a coordinate triple or
/// any other list of plain values keeps its order.
pub fn sorted(value: &mut Value) {
    match value {
        Value::Object(map) => map.values_mut().for_each(sorted),
        Value::Array(items) => {
            items.iter_mut().for_each(sorted);
            if items.iter().all(Value::is_object) {
                items.sort_by_key(key);
            }
        }
        _ => {}
    }
}

/// What the part's box bounds left out of a requirement: its stations,
/// its openings and `open_at_unknown_end` (U3, U1 and I7, in the frame the
/// part is modelled in).
pub fn unbounded(requirement: &mut Value) {
    if let Some(requires) = requirement
        .get_mut("requires")
        .and_then(Value::as_object_mut)
    {
        requires.remove("open");
        requires.remove("from_mm");
        requires.remove("to_mm");
        requires.remove("spans");
    }
    if let Some(flags) = requirement.get_mut("flags").and_then(Value::as_array_mut) {
        flags.retain(|f| f != "open_at_unknown_end");
    }
    if let Some(sources) = requirement.get_mut("sources").and_then(Value::as_array_mut) {
        for source in sources.iter_mut().filter_map(Value::as_object_mut) {
            source.remove("span_mm");
            if let Some(flags) = source.get_mut("flags").and_then(Value::as_array_mut) {
                flags.retain(|f| f != "open_at_unknown_end");
            }
        }
    }
}

/// `a` and `b` equal, numbers within `tol`; the first difference if not.
pub fn differ(a: &Value, b: &Value, tol: f64, at: &str) -> Option<String> {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => {
            let (x, y) = (
                x.as_f64().unwrap_or(f64::NAN),
                y.as_f64().unwrap_or(f64::NAN),
            );
            ((x - y).abs() > tol).then(|| format!("{at}: {x} against {y}"))
        }
        (Value::Array(x), Value::Array(y)) => {
            if x.len() != y.len() {
                return Some(format!("{at}: {} items against {}", x.len(), y.len()));
            }
            x.iter()
                .zip(y)
                .enumerate()
                .find_map(|(i, (u, v))| differ(u, v, tol, &format!("{at}[{i}]")))
        }
        (Value::Object(x), Value::Object(y)) => {
            if x.len() != y.len() || x.keys().any(|k| !y.contains_key(k)) {
                return Some(format!("{at}: other keys"));
            }
            x.iter()
                .find_map(|(k, u)| differ(u, &y[k], tol, &format!("{at}.{k}")))
        }
        _ => (a != b).then(|| format!("{at}: {a} against {b}")),
    }
}

/// `_rot`: each occurrence placed by `m` ∘ its counterpart's placement.
pub fn placed_by(
    m: &M,
    base: &fitment_core::Extracted,
    variant: &fitment_core::Extracted,
    at: &str,
) -> Vec<String> {
    let mut failures = Vec::new();
    for (x, y) in base.occurrences.iter().zip(&variant.occurrences) {
        let (r, t) = (x.transform.rotation(), x.transform.translation());
        let (r2, t2) = (y.transform.rotation(), y.transform.translation());
        for i in 0..3 {
            for j in 0..3 {
                let want: f64 = (0..3).map(|k| m[i][k] * r[k][j]).sum();
                if (want - r2[i][j]).abs() > 1e-9 {
                    failures.push(format!("{at}: a rotation is not VARIANT's"));
                }
            }
            let want: f64 = (0..3).map(|k| m[i][k] * t[k]).sum::<f64>() + m[i][3];
            if (want - t2[i]).abs() > 1e-9 {
                failures.push(format!("{at}: a translation is not VARIANT's"));
            }
        }
    }
    failures
}
