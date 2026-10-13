//! What is searched of a part, built once per definition (N4): trees over
//! its faces' and elements' boxes in its own frame, where each face lies,
//! its plane faces' hole polygons with a tree over them, and which faces
//! share an edge. A neighbour then reads only what lies near the target,
//! so a plate with thousands of holes costs each bolt in it a few, not
//! thousands.

use std::collections::{BTreeMap, BTreeSet};

use super::axial::Element;
use super::geo::V;
use super::overlap;
use super::place::{Aabb, Tree};
use crate::features::Part;
use crate::features::extent::FaceBox;

/// Where a face lies in its part.
#[derive(Debug, Clone, Copy)]
pub(crate) enum FaceAt {
    /// `part.planes[i].faces[j]`.
    Plane(usize, usize),
    /// `part.cylinders[i]`.
    Cylinder(usize),
    /// `part.cones[i]`.
    Cone(usize),
    /// `part.tori[i]`.
    Torus(usize),
}

impl FaceAt {
    /// Its place in the part's order of faces: planes, cylinders, cones,
    /// tori.
    pub(crate) fn order(self) -> (u8, usize, usize) {
        match self {
            Self::Plane(i, j) => (0, i, j),
            Self::Cylinder(i) => (1, i, 0),
            Self::Cone(i) => (2, i, 0),
            Self::Torus(i) => (3, i, 0),
        }
    }
}

/// One definition's index.
pub(crate) struct Index {
    /// Over the definition's face boxes, by position.
    pub(crate) faces: Tree,
    /// Over its elements' boxes, in its own frame, by position.
    pub(crate) elements: Tree,
    /// Where each face lies.
    pub(crate) at: BTreeMap<u64, FaceAt>,
    /// Each plane face's hole polygons in the part's frame, and a tree
    /// over their boxes.
    pub(crate) holes: BTreeMap<u64, (Vec<Vec<V>>, Tree)>,
    /// The faces sharing an edge with each plane face, both ways.
    pub(crate) adjacent: BTreeMap<u64, BTreeSet<u64>>,
    /// Each plane face's loops' vertices, outer and holes, cut to their
    /// convex hull: an affine function (C1's gap) takes its least and
    /// greatest there, as over all of them.
    pub(crate) hulls: BTreeMap<u64, Vec<V>>,
}

/// The vertices of `points`' convex hull in the plane of normal `n`
/// (Andrew's monotone chain); collinear points left out.
fn hull(points: &[V], normal: V) -> Vec<V> {
    let (across, up) = super::geo::perpendiculars(normal);
    let mut flat: Vec<(f64, f64, usize)> = points
        .iter()
        .enumerate()
        .map(|(k, p)| (super::geo::dot(*p, across), super::geo::dot(*p, up), k))
        .collect();
    flat.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
    if flat.len() < 3 {
        return flat
            .iter()
            .filter_map(|f| points.get(f.2).copied())
            .collect();
    }
    let turn = |o: (f64, f64, usize), a: (f64, f64, usize), b: (f64, f64, usize)| {
        (a.0 - o.0) * (b.1 - o.1) - (a.1 - o.1) * (b.0 - o.0)
    };
    let mut chain: Vec<(f64, f64, usize)> = Vec::new();
    for pass in [false, true] {
        let start = chain.len();
        let ordered: Vec<(f64, f64, usize)> = if pass {
            flat.iter().rev().copied().collect()
        } else {
            flat.clone()
        };
        for p in ordered {
            while chain.len() >= start + 2 {
                let (Some(a), Some(b)) = (chain.get(chain.len() - 2), chain.last()) else {
                    break;
                };
                if turn(*a, *b, p) > 0.0 {
                    break;
                }
                chain.pop();
            }
            chain.push(p);
        }
        chain.pop();
    }
    chain
        .iter()
        .filter_map(|f| points.get(f.2).copied())
        .collect()
}

fn polygon_box(points: &[V]) -> Option<Aabb> {
    let first = points.first()?;
    let mut out = (*first, *first);
    for p in points {
        for ((low, high), x) in out.0.iter_mut().zip(out.1.iter_mut()).zip(p) {
            *low = low.min(*x);
            *high = high.max(*x);
        }
    }
    Some(out)
}

impl Index {
    pub(crate) fn of(part: &Part, boxes: &[FaceBox], elements: &[Element]) -> Self {
        let mut at = BTreeMap::new();
        let mut holes = BTreeMap::new();
        let mut hulls = BTreeMap::new();
        for (i, plane) in part.planes.iter().enumerate() {
            let n = plane.normal.to_array();
            for (j, f) in plane.faces.iter().enumerate() {
                at.insert(f.face, FaceAt::Plane(i, j));
                if let crate::features::Outline::Known { loops } = &f.outline {
                    let vertices: Vec<V> = loops
                        .iter()
                        .flat_map(|l| l.segments.iter())
                        .map(|s| match s {
                            crate::features::Segment::Line { from_mm, .. }
                            | crate::features::Segment::Arc { from_mm, .. } => from_mm.to_array(),
                        })
                        .collect();
                    hulls.insert(f.face, hull(&vertices, n));
                }
                let polygons = overlap::hole_polygons(&f.outline, n);
                let tree = Tree::new(polygons.iter().map(|p| polygon_box(p)).collect());
                holes.insert(f.face, (polygons, tree));
            }
        }
        for (i, c) in part.cylinders.iter().enumerate() {
            for f in &c.faces {
                at.insert(*f, FaceAt::Cylinder(i));
            }
        }
        for (i, c) in part.cones.iter().enumerate() {
            for f in &c.faces {
                at.insert(*f, FaceAt::Cone(i));
            }
        }
        for (i, t) in part.tori.iter().enumerate() {
            for f in &t.faces {
                at.insert(*f, FaceAt::Torus(i));
            }
        }
        Self {
            faces: Tree::new(boxes.iter().map(|b| b.bounds).collect()),
            elements: Tree::new(elements.iter().map(super::rules::element_box).collect()),
            at,
            holes,
            adjacent: super::touch::adjacent(part),
            hulls,
        }
    }

    /// The hole polygons of plane face `face` whose boxes meet `region`
    /// (all of them with no region), in the part's frame.
    pub(crate) fn holes_near(&self, face: u64, region: Option<&Aabb>) -> Vec<&Vec<V>> {
        let Some((polygons, tree)) = self.holes.get(&face) else {
            return Vec::new();
        };
        match region {
            Some(r) => tree
                .meeting(r, 0.0)
                .into_iter()
                .filter_map(|k| polygons.get(k))
                .collect(),
            None => polygons.iter().collect(),
        }
    }
}
