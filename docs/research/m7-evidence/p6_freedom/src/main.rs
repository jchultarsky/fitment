//! P6: is the placement fitment chose for a passing candidate unique, and
//! where it is not, is the candidate axisymmetric about the free turn's
//! axis? (docs/research/m7-fit.md, FT10 and FC4, as the reviews set them.)
//!
//!     cargo run --release -- LIST.tsv > ../p6_freedom.txt
//!
//! LIST.tsv: one line per passing candidate, `case \t truth \t socket.json
//! \t candidate.stp \t match.json` (written by `p6_list.py`). For each socket
//! the candidate passed, the placement A from the match document is moved
//! by each motion M of a set derived from the socket's own geometry (a turn
//! of 1 degree either way about every requirement axis and every seat
//! normal, a slide of 0.1 mm either way along every axis, normal and two
//! in-plane directions of every seat), and the interface is checked again
//! at M after A with `Matcher::verify_at`. A motion after which every
//! requirement still passes is free. A free turn is excused when every face
//! of the candidate is a plane square to the axis or a cylinder, cone or
//! torus on it (its own topology, in its own frame); any other free motion
//! makes the fit `placement_not_unique` under D1's rule.

use std::collections::BTreeMap;
use std::fs;

use fitment_core::{
    MatchOptions, MatchPolicy, Matcher, NoFitVerifier, Part, Rigid, SocketDocument, VerdictKind,
    read_part,
};
use serde_json::Value;

type V = [f64; 3];

fn sub(a: V, b: V) -> V {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: V, b: V) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: V, b: V) -> V {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn norm(a: V) -> V {
    let n = dot(a, a).sqrt();
    [a[0] / n, a[1] / n, a[2] / n]
}
fn perp(d: V) -> V {
    let k = if d[0].abs() < 0.9 { [1.0, 0.0, 0.0] } else { [0.0, 1.0, 0.0] };
    norm(cross(d, k))
}
fn arr(v: &Value) -> V {
    [v[0].as_f64().unwrap(), v[1].as_f64().unwrap(), v[2].as_f64().unwrap()]
}

/// A turn by `deg` about the line (p, d), as a Rigid.
fn turn(p: V, d: V, deg: f64) -> Rigid {
    let (s, c) = deg.to_radians().sin_cos();
    let [x, y, z] = d;
    let t = 1.0 - c;
    let r = [
        [t * x * x + c, t * x * y - s * z, t * x * z + s * y],
        [t * x * y + s * z, t * y * y + c, t * y * z - s * x],
        [t * x * z - s * y, t * y * z + s * x, t * z * z + c],
    ];
    let rp = [dot(r[0], p), dot(r[1], p), dot(r[2], p)];
    Rigid::new(r, sub(p, rp)).unwrap()
}
fn slide(d: V, mm: f64) -> Rigid {
    Rigid::new([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]], [d[0] * mm, d[1] * mm, d[2] * mm])
        .unwrap()
}

/// The motions to try for one socket: (label, the axis a turn is about, the motion).
fn motions(socket: &Value) -> Vec<(String, Option<(V, V)>, Rigid)> {
    let mut lines: Vec<(V, V)> = Vec::new();
    let mut normals: Vec<(V, V)> = Vec::new();
    for r in socket["requirements"].as_array().unwrap() {
        let q = &r["requires"];
        match q["kind"].as_str().unwrap() {
            "bore" | "shaft" | "conical_seat" | "partial_bore" | "partial_shaft" => {
                lines.push((arr(&q["axis"]["point_mm"]), norm(arr(&q["axis"]["direction"]))))
            }
            "seat" => {
                let n = norm(arr(&q["normal"]));
                let o = q["offset_mm"].as_f64().unwrap();
                normals.push(([n[0] * o, n[1] * o, n[2] * o], n));
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    for (i, (p, d)) in lines.iter().enumerate() {
        for s in [1.0, -1.0] {
            out.push((format!("turn about axis {i}"), Some((*p, *d)), turn(*p, *d, s)));
            out.push((format!("slide along axis {i}"), None, slide(*d, 0.1 * s)));
        }
    }
    for (i, (p, n)) in normals.iter().enumerate() {
        let a = perp(*n);
        let b = cross(*n, a);
        for s in [1.0, -1.0] {
            out.push((format!("turn about normal {i}"), Some((*p, *n)), turn(*p, *n, s)));
            out.push((format!("slide along normal {i}"), None, slide(*n, 0.1 * s)));
            out.push((format!("slide in plane {i}"), None, slide(a, 0.1 * s)));
            out.push((format!("slide in plane {i}"), None, slide(b, 0.1 * s)));
        }
    }
    out
}

/// Every face of the part a plane square to the line (p, d) or a surface of
/// revolution on it, in the part's own frame.
fn axisymmetric(part: &Part, p: V, d: V) -> bool {
    let on = |q: V, e: V| {
        let off = sub(q, p);
        let along = dot(off, d);
        let perp = sub(off, [d[0] * along, d[1] * along, d[2] * along]);
        dot(cross(e, d), cross(e, d)).sqrt() < 1e-9 && dot(perp, perp).sqrt() < 1e-6
    };
    part.other_faces.is_empty()
        && part.planes.iter().all(|pl| {
            let n = pl.normal.to_array();
            dot(cross(n, d), cross(n, d)).sqrt() < 1e-9
        })
        && part.cylinders.iter().all(|c| on(c.axis.point_mm.to_array(), c.axis.direction.to_array()))
        && part.cones.iter().all(|c| on(c.axis.point_mm.to_array(), c.axis.direction.to_array()))
        && part.tori.iter().all(|t| on(t.axis.point_mm.to_array(), t.axis.direction.to_array()))
}

fn rigid(p: &Value) -> Rigid {
    let r = &p["rotation"];
    let rot = [arr(&r[0]), arr(&r[1]), arr(&r[2])];
    Rigid::new(rot, arr(&p["translation_mm"])).unwrap()
}

fn main() {
    let list = fs::read_to_string(std::env::args().nth(1).expect("LIST.tsv")).unwrap();
    let mut tally: BTreeMap<String, u32> = BTreeMap::new();
    for line in list.lines().filter(|l| !l.trim().is_empty()) {
        let f: Vec<&str> = line.split('\t').collect();
        let (case, truth) = (f[0], f[1]);
        let socket_text = fs::read_to_string(f[2]).unwrap();
        let socket_doc = SocketDocument::from_json(&socket_text).unwrap();
        let socket_json: Value = serde_json::from_str(&socket_text).unwrap();
        let part = read_part(&fs::read(f[3]).unwrap()).unwrap();
        let doc: Value = serde_json::from_str(&fs::read_to_string(f[4]).unwrap()).unwrap();
        let matcher =
            Matcher::new(&socket_doc, MatchPolicy::default(), MatchOptions::default(), &NoFitVerifier)
                .unwrap();
        let mut case_unique = true;
        for s in doc["candidates"][0]["sockets"].as_array().unwrap() {
            let id = s["socket"].as_str().unwrap();
            let seed = s["placement"]["seeded_by"].as_object().map_or("given".to_owned(), |o| {
                o.keys().next().cloned().unwrap_or_default()
            });
            let base = rigid(&s["placement"]);
            let sock = socket_json["extracted"]["sockets"]
                .as_array()
                .unwrap()
                .iter()
                .find(|x| x["id"] == id)
                .unwrap();
            let at_base = matcher.verify_at(&part, id, &base).unwrap().verdict.kind();
            let mut free: Vec<String> = Vec::new();
            let mut excused = true;
            let inverse = base.inverse().unwrap();
            for (label, about, m) in motions(sock) {
                let at = base.then(&m).unwrap();
                let kind = matcher.verify_at(&part, id, &at).unwrap().verdict.kind();
                if kind != VerdictKind::Reject {
                    let sym = about.is_some_and(|(p, d)| {
                        let pc = inverse
                            .apply_point(fitment_core::Point3::new(p).unwrap())
                            .unwrap()
                            .to_array();
                        let dc = inverse
                            .apply_dir(fitment_core::Dir3::from_unit(d).unwrap())
                            .unwrap()
                            .to_array();
                        axisymmetric(&part, pc, dc)
                    });
                    excused &= sym;
                    let tag = if sym { format!("{label} (axisymmetric)") } else { label };
                    if !free.contains(&tag) {
                        free.push(tag);
                    }
                }
            }
            let outcome = if free.is_empty() {
                "unique"
            } else if excused {
                "free turn, axisymmetric: kept"
            } else {
                "placement_not_unique"
            };
            case_unique &= free.is_empty() || excused;
            *tally.entry(format!("{seed}: {outcome}")).or_default() += 1;
            println!(
                "{case} ({truth}) {id}: seed {seed}, at A {:?}; {outcome}{}",
                at_base,
                if free.is_empty() { String::new() } else { format!(" [{}]", free.join(", ")) }
            );
        }
        *tally
            .entry(format!(
                "candidates ({truth}): {}",
                if case_unique { "fit asked" } else { "placement_not_unique" }
            ))
            .or_default() += 1;
    }
    println!("== tally");
    for (k, v) in tally {
        println!("{v:4} {k}");
    }
}
