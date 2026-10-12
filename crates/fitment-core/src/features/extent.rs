//! A sound outer box of a part (`docs/research/m3-socket.md`, R22): every
//! point of the solid lies inside it, or there is none.
//!
//! A planar, cylindrical or conical face lies in the convex hull of its
//! boundary: a planar region in its loops' hull, and every point of a
//! cylinder or cone face on a ruling whose two ends are on the boundary.
//! So the box of the boundary bounds such a face: each line edge by its
//! vertices, each circle edge by its whole circle. A torus face takes its
//! whole torus's box. A face on any other surface (a sphere, a B-spline)
//! or an edge on any other curve leaves the part without a sound box, as
//! a B-spline face can bulge past its edges.

use serde::{Deserialize, Serialize};

use super::topo::{Curve, Loop, P, Surface, Topo};
use crate::geom::Point3;

/// An axis-aligned box in the part's frame, in mm.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct Extent {
    /// The smallest x, y and z of the solid.
    pub min_mm: Point3,
    /// The largest x, y and z.
    pub max_mm: Point3,
}

/// The half extent along each axis of a circle of `radius` in the plane
/// normal to unit `z`: `radius · √(1 − zᵢ²)`.
fn circle_reach(z: P, radius: f64) -> P {
    let reach = |c: f64| radius * (1.0 - c * c).max(0.0).sqrt();
    [reach(z[0]), reach(z[1]), reach(z[2])]
}

struct Bounds {
    min: P,
    max: P,
}

impl Bounds {
    fn add(&mut self, centre: P, reach: P) {
        for i in 0..3 {
            self.min[i] = self.min[i].min(centre[i] - reach[i]);
            self.max[i] = self.max[i].max(centre[i] + reach[i]);
        }
    }
}

/// The box of `topo`, grown by `slack` (the largest distance a vertex lies
/// from its curves and surfaces), or `None` if the solid states a face or
/// edge this cannot bound.
pub(crate) fn of(topo: &Topo, slack: f64) -> Option<Extent> {
    let mut bounds = Bounds {
        min: [f64::INFINITY; 3],
        max: [f64::NEG_INFINITY; 3],
    };
    for face in &topo.faces {
        match &face.surface {
            Surface::Plane { .. } | Surface::Cylinder { .. } | Surface::Cone { .. } => {}
            Surface::Torus {
                location,
                z,
                major,
                minor,
                ..
            } => {
                let ring = circle_reach(*z, *major);
                bounds.add(*location, ring.map(|r| r + minor));
            }
            Surface::Sphere { .. } | Surface::Other { .. } => return None,
        }
        for bound in &face.bounds {
            if let Loop::Vertex(vertex) = bound.lp {
                bounds.add(topo.vertices.get(vertex)?.point, [0.0; 3]);
            }
        }
    }
    for edge in &topo.edges {
        match &edge.curve {
            Curve::Line { .. } => {}
            Curve::Circle {
                centre, z, radius, ..
            } => bounds.add(*centre, circle_reach(*z, *radius)),
            Curve::Other { .. } => return None,
        }
        for vertex in [edge.start, edge.end] {
            bounds.add(topo.vertices.get(vertex)?.point, [0.0; 3]);
        }
    }
    let grow = |p: P, by: f64| [p[0] + by, p[1] + by, p[2] + by];
    Some(Extent {
        min_mm: Point3::new(grow(bounds.min, -slack))?,
        max_mm: Point3::new(grow(bounds.max, slack))?,
    })
}

/// The largest distance from the origin of anything `topo` states: its
/// vertices, its curves' and surfaces' locations grown by their radii, and
/// a cone's apex. Every feature value is one of these or derived along
/// them, so a rigid motion by `t` keeps every placed coordinate within
/// this plus `|t|` (check 38).
pub(crate) fn reach(topo: &Topo) -> f64 {
    let norm = |p: P| (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt();
    let mut far: f64 = 0.0;
    for vertex in &topo.vertices {
        far = far.max(norm(vertex.point));
    }
    for edge in &topo.edges {
        match &edge.curve {
            Curve::Line { point, .. } => far = far.max(norm(*point)),
            Curve::Circle { centre, radius, .. } => far = far.max(norm(*centre) + radius),
            Curve::Other { .. } => {}
        }
    }
    for face in &topo.faces {
        let stated = match &face.surface {
            Surface::Plane { location, .. } => norm(*location),
            Surface::Cylinder {
                location, radius, ..
            }
            | Surface::Sphere {
                location, radius, ..
            } => norm(*location) + radius,
            Surface::Cone {
                location,
                z,
                radius,
                semi_angle,
                ..
            } => (norm(*location) + radius).max(norm(super::group::apex(
                *location,
                *z,
                *radius,
                *semi_angle,
            ))),
            Surface::Torus {
                location,
                major,
                minor,
                ..
            } => norm(*location) + major + minor,
            Surface::Other { .. } => 0.0,
        };
        far = far.max(stated);
    }
    far
}
