//! Feasibility spike: holes, placements, socket and candidate check using only
//! stepq's public API (0.4.1) plus arithmetic on numbers written in the file.
use stepq::model::{Graph, Placement, ProductStructure};
use stepq::p21::{Record, parse};

type V = [f64; 3];
fn sub(a: V, b: V) -> V { [a[0] - b[0], a[1] - b[1], a[2] - b[2]] }
fn add(a: V, b: V) -> V { [a[0] + b[0], a[1] + b[1], a[2] + b[2]] }
fn mul(a: V, s: f64) -> V { [a[0] * s, a[1] * s, a[2] * s] }
fn dot(a: V, b: V) -> f64 { a[0] * b[0] + a[1] * b[1] + a[2] * b[2] }
fn cross(a: V, b: V) -> V { [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]] }
fn norm(a: V) -> f64 { dot(a, a).sqrt() }
fn unit(a: V) -> V { mul(a, 1.0 / norm(a)) }

/// Rigid transform p' = R p + t, R stored as columns x, y, z.
#[derive(Clone, Copy, Debug)]
struct Xf { x: V, y: V, z: V, t: V }
impl Xf {
    const ID: Xf = Xf { x: [1., 0., 0.], y: [0., 1., 0.], z: [0., 0., 1.], t: [0., 0., 0.] };
    fn vec(&self, v: V) -> V { add(add(mul(self.x, v[0]), mul(self.y, v[1])), mul(self.z, v[2])) }
    fn pt(&self, p: V) -> V { add(self.vec(p), self.t) }
    fn inv(&self) -> Xf {
        let (x, y, z) = ([self.x[0], self.y[0], self.z[0]], [self.x[1], self.y[1], self.z[1]], [self.x[2], self.y[2], self.z[2]]);
        let r = Xf { x, y, z, t: [0.; 3] };
        Xf { t: mul(r.vec(self.t), -1.0), ..r }
    }
    fn then(&self, first: &Xf) -> Xf { // self ∘ first
        Xf { x: self.vec(first.x), y: self.vec(first.y), z: self.vec(first.z), t: self.pt(first.t) }
    }
}

fn rec<'e>(g: &'e Graph<'_>, id: u64, name: &str) -> Option<Record<'e>> {
    let node = g.node(id)?;
    g.exchange().records(g.instance(node)).find(|r| r.is(name))
}
fn first<'e>(g: &'e Graph<'_>, id: u64) -> Option<Record<'e>> {
    let node = g.node(id)?;
    g.exchange().records(g.instance(node)).next()
}
fn triple(g: &Graph<'_>, id: u64, name: &str) -> Option<V> {
    let mut it = rec(g, id, name)?.param(1)?.list()?;
    let mut v = [0.0; 3];
    for c in &mut v { *c = it.next()?.literal()?.to_f64()?; }
    Some(v)
}
/// AXIS2_PLACEMENT_3D(name, location, axis, ref_direction) as a transform.
fn frame(g: &Graph<'_>, id: u64) -> Option<Xf> {
    let r = rec(g, id, "AXIS2_PLACEMENT_3D")?;
    let o = triple(g, r.param(1)?.reference()?, "CARTESIAN_POINT")?;
    let z = r.param(2).and_then(|p| p.reference()).and_then(|d| triple(g, d, "DIRECTION")).map_or([0., 0., 1.], unit);
    let xr = r.param(3).and_then(|p| p.reference()).and_then(|d| triple(g, d, "DIRECTION")).unwrap_or([1., 0., 0.]);
    let x = unit(sub(xr, mul(z, dot(xr, z))));
    Some(Xf { x, y: cross(z, x), z, t: o })
}

#[derive(Debug, Clone)]
struct Cyl { hole: bool, r: f64, p: V, d: V, lo: f64, hi: f64, faces: usize }
#[derive(Debug, Clone)]
struct Pl { p: V, n: V }
#[derive(Default, Debug, Clone)]
struct Features { cyls: Vec<Cyl>, planes: Vec<Pl> }

fn refs(g: &Graph<'_>, id: u64, index: usize) -> Vec<u64> {
    first(g, id).and_then(|r| r.param(index)).and_then(|p| p.list())
        .map(|l| l.filter_map(|p| p.reference()).collect()).unwrap_or_default()
}
/// Vertex points reachable from a face (its loops' edges' vertices).
fn face_vertices(g: &Graph<'_>, face: u64) -> Vec<V> {
    let (mut out, mut stack, mut seen) = (vec![], vec![g.node(face).unwrap()], std::collections::HashSet::new());
    while let Some(n) = stack.pop() {
        if !seen.insert(n) { continue; }
        let id = g.instance(n).id;
        if let Some(r) = rec(g, id, "VERTEX_POINT") {
            if let Some(p) = r.param(1).and_then(|p| p.reference()).and_then(|c| triple(g, c, "CARTESIAN_POINT")) { out.push(p); }
            continue;
        }
        stack.extend_from_slice(g.references(n));
    }
    out
}
const TOL: f64 = 1e-3; // mm
const ATOL: f64 = 1e-6;

/// Analytic features of the solids in one shape representation, in its own frame.
fn features(g: &Graph<'_>, representation: u64) -> Features {
    let mut f = Features::default();
    for item in refs(g, representation, 1) {
        let Some(brep) = rec(g, item, "MANIFOLD_SOLID_BREP") else { continue };
        let shell = brep.param(1).unwrap().reference().unwrap();
        for face in refs(g, shell, 1) {
            let Some(af) = rec(g, face, "ADVANCED_FACE") else { continue };
            let surface = af.param(2).unwrap().reference().unwrap();
            let same = af.param(3).unwrap().literal().unwrap().enumeration() == Some("T");
            if let Some(c) = rec(g, surface, "CYLINDRICAL_SURFACE") {
                let fr = frame(g, c.param(1).unwrap().reference().unwrap()).unwrap();
                let r = c.param(2).unwrap().literal().unwrap().to_f64().unwrap();
                let ts: Vec<f64> = face_vertices(g, face).iter().map(|v| dot(sub(*v, fr.t), fr.z)).collect();
                let (lo, hi) = ts.iter().fold((f64::MAX, f64::MIN), |(a, b), t| (a.min(*t), b.max(*t)));
                // The surface normal of a cylinder points away from its axis; same_sense = .F.
                // means the face normal (out of the material) points at the axis: a hole.
                let new = Cyl { hole: !same, r, p: fr.t, d: fr.z, lo, hi, faces: 1 };
                match f.cyls.iter_mut().find(|k| k.hole == new.hole && (k.r - r).abs() < TOL && coaxial(k.p, k.d, new.p, new.d)) {
                    Some(k) => { // the same cylinder written as several faces
                        let s = dot(k.d, new.d).signum();
                        let off = dot(sub(new.p, k.p), k.d);
                        let (a, b) = (off + s * new.lo, off + s * new.hi);
                        k.lo = k.lo.min(a.min(b)); k.hi = k.hi.max(a.max(b)); k.faces += 1;
                    }
                    None => f.cyls.push(new),
                }
            } else if let Some(p) = rec(g, surface, "PLANE") {
                let fr = frame(g, p.param(1).unwrap().reference().unwrap()).unwrap();
                f.planes.push(Pl { p: fr.t, n: if same { fr.z } else { mul(fr.z, -1.0) } });
            }
        }
    }
    f
}
fn coaxial(p1: V, d1: V, p2: V, d2: V) -> bool {
    norm(cross(d1, d2)) < ATOL && norm(cross(sub(p2, p1), d1)) < TOL
}
fn moved(f: &Features, m: &Xf) -> Features {
    Features {
        cyls: f.cyls.iter().map(|c| Cyl { p: m.pt(c.p), d: m.vec(c.d), ..c.clone() }).collect(),
        planes: f.planes.iter().map(|p| Pl { p: m.pt(p.p), n: m.vec(p.n) }).collect(),
    }
}
fn load(path: &str) -> Vec<u8> { std::fs::read(path).unwrap() }

/// One requirement of the socket: an axis in the assembly frame the part must offer a hole on.
#[derive(Debug)]
struct Req { p: V, d: V, min_d: f64, max_d: f64, why: String }

fn main() {
    let dir = std::env::args().nth(1).unwrap();
    let src = load(&format!("{dir}/asm.step"));
    let g = Graph::new(parse(&src).unwrap()).unwrap();
    let s = ProductStructure::new(&g);
    let defs = s.definitions();
    let root = s.roots()[0];
    // Every placed occurrence of every part, in the assembly frame (one level here).
    let mut placed: Vec<(String, Features)> = vec![];
    for u in s.children(root) {
        let name = defs[u.child].product.as_ref().and_then(|p| p.name.clone()).unwrap_or_default();
        let m = match &u.placement {
            Some(Placement::ShapeRelationship { transformation: Some(t), .. }) => {
                let idt = rec(&g, *t, "ITEM_DEFINED_TRANSFORMATION").unwrap();
                let m1 = frame(&g, idt.param(2).unwrap().reference().unwrap()).unwrap();
                let m2 = frame(&g, idt.param(3).unwrap().reference().unwrap()).unwrap();
                m2.then(&m1.inv())
            }
            _ => Xf::ID,
        };
        let mut f = Features::default();
        for r in &defs[u.child].shape_representations { let x = features(&g, *r); f.cyls.extend(x.cyls); f.planes.extend(x.planes); }
        println!("{name:8} at ({:6.1},{:6.1},{:6.1})  cylinders: {}", m.t[0], m.t[1], m.t[2],
            f.cyls.iter().map(|c| format!("{}Ø{} len {:.1} ({} faces)", if c.hole { "hole " } else { "shaft " }, 2.0 * c.r, c.hi - c.lo, c.faces)).collect::<Vec<_>>().join(", "));
        placed.push((name, moved(&f, &m)));
    }
    // Socket of the target: what its neighbours ask of it.
    let target = "BRACKET";
    let t = &placed.iter().find(|(n, _)| n == target).unwrap().1;
    let mut socket: Vec<Req> = vec![];
    println!("\nSocket of {target}:");
    for h in t.cyls.iter().filter(|c| c.hole) {
        let mut why = vec![];
        let (mut min_d, mut max_d) = (0.0_f64, f64::MAX);
        for (n, f) in placed.iter().filter(|(n, _)| n != target) {
            for c in f.cyls.iter().filter(|c| coaxial(h.p, h.d, c.p, c.d)) {
                // Coaxial is not enough: the two must also share a stretch of the axis.
                let (off, sg) = (dot(sub(c.p, h.p), h.d), dot(c.d, h.d).signum());
                let (lo, hi) = ((off + sg * c.lo).min(off + sg * c.hi), (off + sg * c.lo).max(off + sg * c.hi));
                let overlap = hi.min(h.hi) - lo.max(h.lo);
                if overlap <= TOL {
                    if c.hole { why.push(format!("continues as hole Ø{} in {n}", 2.0 * c.r)); }
                    else if c.r > h.r { why.push(format!("Ø{} of {n} bears on the face around it (head or shoulder)", 2.0 * c.r)); }
                    continue;
                }
                if c.hole { why.push(format!("overlaps hole Ø{} in {n}", 2.0 * c.r)); }
                else if c.r < h.r { why.push(format!("shaft Ø{} of {n} passes through with clearance", 2.0 * c.r)); min_d = min_d.max(2.0 * c.r); }
                else { why.push(format!("shaft Ø{} of {n} is larger than the hole: threaded or press fit", 2.0 * c.r)); min_d = 2.0 * h.r; max_d = 2.0 * h.r; }
            }
        }
        if why.is_empty() { println!("  hole Ø{} at ({:.1},{:.1}): no neighbour on its axis -> not an interface", 2.0 * h.r, h.p[0], h.p[1]); continue; }
        if max_d == f64::MAX { max_d = 2.0 * h.r; } // strict policy: no larger than the original
        println!("  hole Ø{} at ({:.1},{:.1}): {}", 2.0 * h.r, h.p[0], h.p[1], why.join("; "));
        socket.push(Req { p: h.p, d: h.d, min_d, max_d, why: why.join("; ") });
    }
    // Planar contact: coplanar faces with opposed normals (extent overlap not tested in this spike).
    for (n, f) in placed.iter().filter(|(n, _)| n != target) {
        for a in &t.planes { for b in &f.planes {
            if dot(a.n, b.n) < -1.0 + ATOL && dot(sub(b.p, a.p), a.n).abs() < TOL {
                println!("  plane z={:.1} normal ({:.0},{:.0},{:.0}) is coplanar with an opposed face of {n}", a.p[2], a.n[0], a.n[1], a.n[2]);
            }
        }}
    }
    // Candidates, each in an arbitrary frame of its own.
    println!();
    for name in ["cand_good", "cand_shift", "cand_small", "cand_missing"] {
        let src = load(&format!("{dir}/{name}.step"));
        let g = Graph::new(parse(&src).unwrap()).unwrap();
        let s = ProductStructure::new(&g);
        let mut f = Features::default();
        for d in s.definitions() { for r in &d.shape_representations { let x = features(&g, *r); f.cyls.extend(x.cyls); f.planes.extend(x.planes); } }
        let holes: Vec<&Cyl> = f.cyls.iter().filter(|c| c.hole).collect();
        println!("{name}: {}", verdict(&socket, &holes));
    }
}

/// Tries every placement that puts two candidate holes on the first two socket axes; a pass needs
/// every requirement met. Parallel axes only; the slide along the axes is left free (through holes).
fn verdict(socket: &[Req], holes: &[&Cyl]) -> String {
    let (a, b) = (&socket[0], &socket[1]);
    let want = norm(cross(sub(b.p, a.p), a.d));
    let mut best = String::from("REJECT: no pair of holes at the socket's spacing");
    for (i, ha) in holes.iter().enumerate() { for (j, hb) in holes.iter().enumerate() {
        if i == j || norm(cross(ha.d, hb.d)) > ATOL { continue; }
        let got = norm(cross(sub(hb.p, ha.p), ha.d));
        if (got - want).abs() > TOL { continue; }
        for flip in [1.0, -1.0] {
            // Frames: z along the axis, x from hole A towards hole B, origin on axis A.
            let fr = |p: V, d: V, q: V| { let z = unit(d); let v = sub(q, p); let x = unit(sub(v, mul(z, dot(v, z)))); Xf { x, y: cross(z, x), z, t: p } };
            let m = fr(a.p, a.d, b.p).then(&fr(ha.p, mul(ha.d, flip), hb.p).inv());
            let mut fails = vec![];
            for (k, r) in socket.iter().enumerate() {
                let hit = holes.iter().find(|h| coaxial(r.p, r.d, m.pt(h.p), m.vec(h.d)));
                match hit {
                    None => fails.push(format!("requirement {} ({}): no hole on that axis", k + 1, r.why)),
                    Some(h) if 2.0 * h.r < r.min_d - TOL || 2.0 * h.r > r.max_d + TOL => fails.push(format!("requirement {}: hole Ø{} outside [{}, {}]", k + 1, 2.0 * h.r, r.min_d, r.max_d)),
                    Some(_) => {}
                }
            }
            if fails.is_empty() { return format!("INTERFACE MATCH ({} of {} requirements verified; fit not checked)", socket.len(), socket.len()); }
            if best.starts_with("REJECT: no pair") || fails.len() < best.matches("requirement").count() { best = format!("REJECT: {}", fails.join("; ")); }
        }
    }}
    best
}
