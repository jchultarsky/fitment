//! A solid that passed gate stages 4 and 5, copied out of stepq: faces with
//! their surfaces, loops in traversal order, edges with their curves and
//! the faces that use them, and vertices with their points, each with its
//! `#id`. Every value is in millimetres and radians.
//!
//! The gate's geometry checks and every feature rule read this copy, so no
//! module but `gate/`, `features/step.rs` and `assembly/step.rs` sees a stepq value
//! (`docs/research/m2-features.md`, section 11.1). The vector arithmetic
//! and the loop geometry both share live here too, so that the gate's
//! orientation check and the plane outlines measure loops the same way.

use std::f64::consts::TAU;

use libm::atan2;

use crate::geom::Rigid;

/// A point or vector, in millimetres.
pub(crate) type P = [f64; 3];

pub(crate) fn sub(a: P, b: P) -> P {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub(crate) fn dot(a: P, b: P) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub(crate) fn cross(a: P, b: P) -> P {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

pub(crate) fn scale(a: P, k: f64) -> P {
    [a[0] * k, a[1] * k, a[2] * k]
}

pub(crate) fn neg(a: P) -> P {
    [-a[0], -a[1], -a[2]]
}

pub(crate) fn length(a: P) -> f64 {
    dot(a, a).sqrt()
}

/// The length of (x, y): `sqrt` is correctly rounded on every platform,
/// where `hypot` is not.
pub(crate) fn norm2(x: f64, y: f64) -> f64 {
    (x * x + y * y).sqrt()
}

/// The distance from `point` to the axis line through `origin` along unit
/// `axis`, and `point`'s station along it.
pub(crate) fn radial(point: P, origin: P, axis: P) -> (f64, f64) {
    let offset = sub(point, origin);
    let station = dot(offset, axis);
    (length(sub(offset, scale(axis, station))), station)
}

/// A face's surface.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Surface {
    /// The plane through `location` with unit normal `z`.
    Plane { instance: u64, location: P, z: P },
    /// The cylinder of `radius` about the line through `location` along
    /// unit `z`.
    Cylinder {
        instance: u64,
        location: P,
        z: P,
        radius: f64,
    },
    /// The cone of `radius` in the plane of `location`, widening along
    /// unit `z` at `semi_angle`.
    Cone {
        instance: u64,
        location: P,
        z: P,
        radius: f64,
        semi_angle: f64,
    },
    /// The sphere of `radius` about `location`.
    Sphere {
        instance: u64,
        location: P,
        radius: f64,
    },
    /// The torus about the line through `location` along unit `z`.
    Torus {
        instance: u64,
        location: P,
        z: P,
        major: f64,
        minor: f64,
    },
    /// A surface stepq does not read, by its entity type.
    Other { instance: u64, entity: String },
}

impl Surface {
    pub(crate) fn instance(&self) -> u64 {
        match self {
            Self::Plane { instance, .. }
            | Self::Cylinder { instance, .. }
            | Self::Cone { instance, .. }
            | Self::Sphere { instance, .. }
            | Self::Torus { instance, .. }
            | Self::Other { instance, .. } => *instance,
        }
    }
}

/// An edge's 3D curve (a surface curve's `curve_3d`).
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Curve {
    /// The line through `point` along unit `dir`.
    Line { instance: u64, point: P, dir: P },
    /// The circle of `radius` about `centre` in the plane normal to unit
    /// `z`, running counterclockwise about `z`.
    Circle {
        instance: u64,
        centre: P,
        z: P,
        radius: f64,
    },
    /// A curve stepq does not read, by its entity type.
    Other { instance: u64, entity: String },
}

impl Curve {
    pub(crate) fn instance(&self) -> u64 {
        match self {
            Self::Line { instance, .. }
            | Self::Circle { instance, .. }
            | Self::Other { instance, .. } => *instance,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Vertex {
    pub(crate) instance: u64,
    pub(crate) point: P,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Edge {
    pub(crate) instance: u64,
    /// Indices into [`Topo::vertices`].
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) curve: Curve,
    /// True if the edge runs along its curve's direction.
    pub(crate) same_sense: bool,
    /// The faces that use it, as indices into [`Topo::faces`]: two, since
    /// the gate passed it (check 15).
    pub(crate) faces: Vec<usize>,
}

impl Edge {
    pub(crate) fn is_closed(&self) -> bool {
        self.start == self.end
    }
}

/// One use of an edge by a loop: `forward` if the loop runs from the
/// edge's start to its end, as the solid holds the face.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Use {
    pub(crate) edge: usize,
    pub(crate) forward: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Loop {
    /// Edge uses in traversal order (section 5.3 of the design).
    Edges(Vec<Use>),
    /// One vertex, an index into [`Topo::vertices`].
    Vertex(usize),
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Bound {
    pub(crate) instance: u64,
    /// True if the file states it as the face's outer bound.
    pub(crate) stated_outer: bool,
    pub(crate) lp: Loop,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Face {
    pub(crate) instance: u64,
    /// An index into [`Topo::shells`]: 0 for the outer shell.
    pub(crate) shell: usize,
    pub(crate) surface: Surface,
    /// True if the outward normal is the surface's own: `same_sense`
    /// composed with the holding shell's orientation.
    pub(crate) sense: bool,
    /// The face's own `same_sense`, as written.
    pub(crate) same_sense: bool,
    pub(crate) bounds: Vec<Bound>,
    /// How far its vertices spread along its surface's axis, for a
    /// cylinder, cone or torus, else 0: the lever arm of section 5.2. Set
    /// once by [`Topo::measure`], so grouping never recomputes it.
    pub(crate) extent: f64,
}

/// The solid.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Topo {
    pub(crate) instance: u64,
    /// The shells' `#id`s, the outer one first.
    pub(crate) shells: Vec<u64>,
    pub(crate) faces: Vec<Face>,
    pub(crate) edges: Vec<Edge>,
    pub(crate) vertices: Vec<Vertex>,
}

impl Topo {
    /// Sets every face's [`Face::extent`].
    pub(crate) fn measure(mut self) -> Self {
        let extents: Vec<f64> = self
            .faces
            .iter()
            .map(|face| {
                let axis = match &face.surface {
                    Surface::Cylinder { z, .. }
                    | Surface::Cone { z, .. }
                    | Surface::Torus { z, .. } => *z,
                    _ => return 0.0,
                };
                let stations: Vec<f64> = self
                    .face_vertices(face)
                    .into_iter()
                    .filter_map(|v| self.vertices.get(v))
                    .map(|v| dot(v.point, axis))
                    .collect();
                let low = stations.iter().copied().fold(f64::INFINITY, f64::min);
                let high = stations.iter().copied().fold(f64::NEG_INFINITY, f64::max);
                if high >= low { high - low } else { 0.0 }
            })
            .collect();
        for (face, extent) in self.faces.iter_mut().zip(extents) {
            face.extent = extent;
        }
        self
    }

    /// The solid moved by `rigid` (`p ↦ R·p + t`), as when a mapped item
    /// places it in the part's frame. Directions are renormalized; a
    /// proper rotation keeps every face's sense.
    pub(crate) fn moved(mut self, rigid: &Rigid) -> Self {
        let rotation = rigid.rotation();
        let rotate = |d: P| rotation.map(|row| dot(row, d));
        let place = |p: P| {
            let r = rotate(p);
            let t = rigid.translation();
            [r[0] + t[0], r[1] + t[1], r[2] + t[2]]
        };
        let turn = |d: P| {
            let r = rotate(d);
            let n = length(r);
            if n > 0.0 { scale(r, 1.0 / n) } else { r }
        };
        for vertex in &mut self.vertices {
            vertex.point = place(vertex.point);
        }
        for face in &mut self.faces {
            match &mut face.surface {
                Surface::Plane { location, z, .. }
                | Surface::Cylinder { location, z, .. }
                | Surface::Cone { location, z, .. }
                | Surface::Torus { location, z, .. } => {
                    *location = place(*location);
                    *z = turn(*z);
                }
                Surface::Sphere { location, .. } => *location = place(*location),
                Surface::Other { .. } => {}
            }
        }
        for edge in &mut self.edges {
            match &mut edge.curve {
                Curve::Line { point, dir, .. } => {
                    *point = place(*point);
                    *dir = turn(*dir);
                }
                Curve::Circle { centre, z, .. } => {
                    *centre = place(*centre);
                    *z = turn(*z);
                }
                Curve::Other { .. } => {}
            }
        }
        self
    }

    /// The vertices on face `face`'s loops, sorted, each once.
    pub(crate) fn face_vertices(&self, face: &Face) -> Vec<usize> {
        let mut found = Vec::new();
        for bound in &face.bounds {
            match &bound.lp {
                Loop::Vertex(vertex) => found.push(*vertex),
                Loop::Edges(uses) => {
                    for edge in uses.iter().filter_map(|u| self.edges.get(u.edge)) {
                        found.extend([edge.start, edge.end]);
                    }
                }
            }
        }
        found.sort_unstable();
        found.dedup();
        found
    }

    /// The face across `edge` from face `face`: the edge's other user, or
    /// `face` itself for a seam.
    pub(crate) fn across(&self, edge: usize, face: usize) -> usize {
        let users = self.edges.get(edge).map_or(&[][..], |e| &e.faces[..]);
        match users {
            [a, b] if *a == face => *b,
            [a, _] => *a,
            _ => face,
        }
    }

    /// The edge uses of each of `face`'s bounds, with their geometry. A
    /// vertex loop has none.
    pub(crate) fn travels<'t>(&'t self, face: &Face) -> Vec<Vec<Travel<'t>>> {
        face.bounds
            .iter()
            .map(|bound| match &bound.lp {
                Loop::Vertex(_) => Vec::new(),
                Loop::Edges(uses) => uses.iter().filter_map(|u| self.travel(*u)).collect(),
            })
            .collect()
    }

    fn travel(&self, edge_use: Use) -> Option<Travel<'_>> {
        let edge = self.edges.get(edge_use.edge)?;
        let start = self.vertices.get(edge.start)?.point;
        let end = self.vertices.get(edge.end)?.point;
        let (from, to) = if edge_use.forward {
            (start, end)
        } else {
            (end, start)
        };
        let (first, last) = if edge_use.forward {
            (edge.start, edge.end)
        } else {
            (edge.end, edge.start)
        };
        Some(Travel {
            edge: edge_use.edge,
            first,
            last,
            curve: &edge.curve,
            along: edge_use.forward == edge.same_sense,
            closed: edge.is_closed(),
            from,
            to,
        })
    }
}

/// One edge use along a loop, with its geometry.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Travel<'t> {
    /// The edge, an index into [`Topo::edges`].
    pub(crate) edge: usize,
    /// The vertices it runs from and to, indices into [`Topo::vertices`].
    pub(crate) first: usize,
    pub(crate) last: usize,
    pub(crate) curve: &'t Curve,
    /// True if the loop runs along the curve's own direction.
    pub(crate) along: bool,
    pub(crate) closed: bool,
    pub(crate) from: P,
    pub(crate) to: P,
}

impl Travel<'_> {
    /// The signed angle a circular edge use sweeps about its circle's axis
    /// `z`, in [−2π, 2π]: counterclockwise is positive. A closed edge
    /// sweeps the whole circle, 2π or −2π as it runs.
    pub(crate) fn sweep(&self, centre: P, z: P) -> f64 {
        if self.closed {
            if self.along { TAU } else { -TAU }
        } else if self.along {
            ccw_angle(centre, self.from, self.to, z)
        } else {
            -ccw_angle(centre, self.to, self.from, z)
        }
    }

    /// This use's term of a loop's signed area about unit `normal`,
    /// measured from `origin` (Green's theorem, exact for lines and
    /// circular arcs). An arc's term is its exact projection along
    /// `normal`, scaled by the cosine between its axis and the normal;
    /// `None` for a curve that is neither.
    pub(crate) fn area(&self, origin: P, normal: P) -> Option<f64> {
        let (a, b) = (sub(self.from, origin), sub(self.to, origin));
        match self.curve {
            Curve::Line { .. } => Some(dot(cross(a, b), normal) / 2.0),
            Curve::Circle {
                centre, z, radius, ..
            } => {
                let c = sub(*centre, origin);
                let theta = self.sweep(*centre, *z) * dot(*z, normal);
                Some(dot(cross(c, sub(b, a)), normal) / 2.0 + radius * radius * theta / 2.0)
            }
            Curve::Other { .. } => None,
        }
    }
}

/// The angle from `from` to `to` counterclockwise about unit `axis` around
/// `centre`, in (0, 2π].
pub(crate) fn ccw_angle(centre: P, from: P, to: P, axis: P) -> f64 {
    let (start, end) = (sub(from, centre), sub(to, centre));
    let angle = atan2(dot(cross(start, end), axis), dot(start, end));
    if angle <= 0.0 { angle + TAU } else { angle }
}
