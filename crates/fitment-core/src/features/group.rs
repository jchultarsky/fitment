//! Which faces the file states as one surface (`docs/research/m2-features.md`,
//! sections 5.2, 7.2, 8.1 and 8.6), with the extraction constants.

use std::collections::BTreeMap;

use libm::tan;

use super::topo::{Face, P, Surface, Topo, cross, dot, length, neg, radial, scale, sub};

/// "The file states the same value twice", in mm.
pub(crate) const IDENTITY_MM: f64 = crate::gate::geometry::IDENTITY_MM;

/// Plane normals in one direction class.
pub(crate) const PARALLEL_SINE: f64 = 1e-9;

/// The smallest lever arm of an angular test, in mm.
pub(crate) const LEVER_MIN_MM: f64 = 1.0;

/// The bins direction classes are looked up in: twice [`PARALLEL_SINE`]
/// wide, so that two normals in one class lie in neighbouring bins, and a
/// bin holds only a few classes however many a file states (a bin much
/// wider made grouping quadratic on a jittered grid; review of PR 4).
const BINS_PER_UNIT: f64 = 1.0 / (2.0 * PARALLEL_SINE);

/// A face's outward normal, if it is a plane face.
pub(crate) fn outward(face: &Face) -> Option<P> {
    match &face.surface {
        Surface::Plane { z, .. } => Some(if face.sense { *z } else { neg(*z) }),
        _ => None,
    }
}

/// A point of plane face `face` on its own plane, and the largest distance
/// from it to the face's vertices: its first vertex projected onto the
/// plane the face states, or its stated location if it has no vertex. A
/// plane's stated location may lie anywhere on
/// it, up to 1e6 mm away; offsets and angles are measured here instead, so
/// that a normal within [`PARALLEL_SINE`] of another cannot hide a step of
/// up to 1e-3 mm between them (review of PR 4).
pub(crate) fn anchor(topo: &Topo, face: &Face) -> Option<(P, f64)> {
    let Surface::Plane { location, z, .. } = &face.surface else {
        return None;
    };
    let points: Vec<P> = topo
        .face_vertices(face)
        .into_iter()
        .filter_map(|v| topo.vertices.get(v).map(|v| v.point))
        .collect();
    // A face with no vertex, which no valid solid has, keeps its location.
    let first = points.first().copied().unwrap_or(*location);
    let q = sub(first, scale(*z, dot(sub(first, *location), *z)));
    let extent = points
        .iter()
        .map(|p| length(sub(*p, q)))
        .fold(0.0, f64::max);
    Some((q, extent))
}

/// One plane: faces of one shell, coplanar, with one outward normal.
pub(crate) struct PlaneGroup {
    /// The direction class's normal: its smallest-`#id` face's.
    pub(crate) normal: P,
    /// Face indices, sorted by `#id`.
    pub(crate) faces: Vec<usize>,
}

/// Groups the plane faces (section 7.2): first into direction classes,
/// each compared with its smallest-`#id` face only, then into runs of
/// offsets along the class normal within [`IDENTITY_MM`] of the run's first
/// face. A face joins a class only if its normal's sine to the class's,
/// times the face's extent from its [`anchor`], is within [`IDENTITY_MM`]
/// too, and its offset is measured at its anchor. Planes come sorted by their smallest face `#id`.
pub(crate) fn planes(topo: &Topo) -> Vec<PlaneGroup> {
    let mut faces: Vec<(usize, P, P, f64)> = topo
        .faces
        .iter()
        .enumerate()
        .filter_map(|(index, face)| {
            let (q, extent) = anchor(topo, face)?;
            Some((index, outward(face)?, q, extent))
        })
        .collect();
    faces.sort_by_key(|&(index, ..)| topo.faces.get(index).map(|f| f.instance));
    let anchors: BTreeMap<usize, P> = faces.iter().map(|&(i, _, q, _)| (i, q)).collect();

    // Direction classes, by shell: normal, faces.
    let mut classes: Vec<(usize, P, Vec<usize>)> = Vec::new();
    let mut bins: BTreeMap<(usize, [i64; 3]), Vec<usize>> = BTreeMap::new();
    for (index, n, _, extent) in faces {
        let Some(shell) = topo.faces.get(index).map(|f| f.shell) else {
            continue;
        };
        let key = bin(n);
        let mut candidates: Vec<usize> = neighbours(key)
            .filter_map(|k| bins.get(&(shell, k)))
            .flatten()
            .copied()
            .collect();
        candidates.sort_unstable();
        let found = candidates.into_iter().find(|&c| {
            classes.get(c).is_some_and(|(_, m, _)| {
                let sine = length(cross(*m, n));
                sine <= PARALLEL_SINE && sine * extent <= IDENTITY_MM && dot(*m, n) > 0.0
            })
        });
        if let Some((_, _, members)) = found.and_then(|c| classes.get_mut(c)) {
            members.push(index);
        } else {
            bins.entry((shell, key)).or_default().push(classes.len());
            classes.push((shell, n, vec![index]));
        }
    }

    let mut planes = Vec::new();
    for (_, normal, members) in classes {
        let mut offsets: Vec<(f64, u64, usize)> = members
            .iter()
            .filter_map(|&index| {
                let face = topo.faces.get(index)?;
                Some((dot(normal, *anchors.get(&index)?), face.instance, index))
            })
            .collect();
        offsets.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        let mut run: Option<(f64, Vec<(u64, usize)>)> = None;
        for (offset, instance, index) in offsets {
            match &mut run {
                Some((first, faces)) if offset - *first <= IDENTITY_MM => {
                    faces.push((instance, index));
                }
                _ => {
                    if let Some((_, faces)) = run.take() {
                        planes.push(plane(normal, faces));
                    }
                    run = Some((offset, vec![(instance, index)]));
                }
            }
        }
        if let Some((_, faces)) = run {
            planes.push(plane(normal, faces));
        }
    }
    planes.sort_by_key(|p| {
        p.faces
            .first()
            .and_then(|&i| topo.faces.get(i))
            .map(|f| f.instance)
    });
    planes
}

fn plane(normal: P, mut faces: Vec<(u64, usize)>) -> PlaneGroup {
    faces.sort_unstable();
    PlaneGroup {
        normal,
        faces: faces.into_iter().map(|(_, index)| index).collect(),
    }
}

#[allow(clippy::cast_possible_truncation)] // |n| ≤ 1, so the bins are small
fn bin(n: P) -> [i64; 3] {
    n.map(|c| (c * BINS_PER_UNIT).round() as i64)
}

fn neighbours(key: [i64; 3]) -> impl Iterator<Item = [i64; 3]> {
    (0..27).map(move |k: i64| {
        [
            key[0] + k % 3 - 1,
            key[1] + (k / 3) % 3 - 1,
            key[2] + k / 9 - 1,
        ]
    })
}

/// The faces of one kind grouped by "shares an edge and is on the same
/// surface" (rule G2), by union-find with the lowest face index as root.
/// Groups come sorted by their smallest face `#id`, faces by `#id`.
pub(crate) fn connected(
    topo: &Topo,
    kind: impl Fn(&Surface) -> bool,
    same: impl Fn(&Topo, usize, usize) -> bool,
) -> Vec<Vec<usize>> {
    let mut root: Vec<usize> = (0..topo.faces.len()).collect();
    let member = |i: usize| topo.faces.get(i).is_some_and(|f| kind(&f.surface));
    for edge in &topo.edges {
        if let [a, b] = edge.faces[..] {
            if a != b && member(a) && member(b) && same(topo, a, b) {
                let (ra, rb) = (find(&mut root, a), find(&mut root, b));
                let (low, high) = (ra.min(rb), ra.max(rb));
                if let Some(slot) = root.get_mut(high) {
                    *slot = low;
                }
            }
        }
    }
    let mut groups: BTreeMap<usize, Vec<(u64, usize)>> = BTreeMap::new();
    for (index, face) in topo.faces.iter().enumerate() {
        if kind(&face.surface) {
            groups
                .entry(find(&mut root, index))
                .or_default()
                .push((face.instance, index));
        }
    }
    let mut groups: Vec<Vec<(u64, usize)>> = groups.into_values().collect();
    for group in &mut groups {
        group.sort_unstable();
    }
    groups.sort_unstable();
    groups
        .into_iter()
        .map(|g| g.into_iter().map(|(_, index)| index).collect())
        .collect()
}

/// The root of `i`'s set, halving the path on the way so that long chains
/// cannot make grouping quadratic.
pub(crate) fn find(root: &mut [usize], mut i: usize) -> usize {
    loop {
        let Some(&parent) = root.get(i) else {
            return i;
        };
        if parent == i {
            return i;
        }
        let grandparent = root.get(parent).copied().unwrap_or(parent);
        if let Some(slot) = root.get_mut(i) {
            *slot = grandparent;
        }
        i = grandparent;
    }
}

/// The extent of face `index` along unit `axis`, from its own vertices.
fn extent(topo: &Topo, index: usize, axis: P) -> f64 {
    let Some(face) = topo.faces.get(index) else {
        return 0.0;
    };
    let stations: Vec<f64> = topo
        .face_vertices(face)
        .into_iter()
        .filter_map(|v| topo.vertices.get(v))
        .map(|v| dot(v.point, axis))
        .collect();
    let low = stations.iter().copied().fold(f64::INFINITY, f64::min);
    let high = stations.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    if high >= low { high - low } else { 0.0 }
}

/// The lever arm of faces `a` and `b` about unit `axis` (section 5.2): the
/// larger of their extents along it, and at least [`LEVER_MIN_MM`].
fn lever(topo: &Topo, a: usize, b: usize, axis: P) -> f64 {
    extent(topo, a, axis)
        .max(extent(topo, b, axis))
        .max(LEVER_MIN_MM)
}

/// True if unit axes `z1` and `z2` agree within ε at lever arm `lever`;
/// the sign is ignored.
fn axes_agree(z1: P, z2: P, lever: f64) -> bool {
    length(cross(z1, z2)) * lever <= IDENTITY_MM
}

fn sense(topo: &Topo, i: usize) -> Option<bool> {
    topo.faces.get(i).map(|f| f.sense)
}

/// Rule G1: faces `a` and `b` lie on one cylinder with one sense.
pub(crate) fn same_cylinder(topo: &Topo, a: usize, b: usize) -> bool {
    sense(topo, a) == sense(topo, b) && one_cylinder(topo, a, b, IDENTITY_MM)
}

/// Faces `a` and `b` lie on one cylinder within `tolerance` mm, whatever
/// their senses: radii, each axis point off the other's line, and the axes'
/// angle at the faces' lever arm.
pub(crate) fn one_cylinder(topo: &Topo, a: usize, b: usize, tolerance: f64) -> bool {
    let surfaces = (
        topo.faces.get(a).map(|f| &f.surface),
        topo.faces.get(b).map(|f| &f.surface),
    );
    let (
        Some(Surface::Cylinder {
            location: p1,
            z: z1,
            radius: r1,
            ..
        }),
        Some(Surface::Cylinder {
            location: p2,
            z: z2,
            radius: r2,
            ..
        }),
    ) = surfaces
    else {
        return false;
    };
    (r1 - r2).abs() <= tolerance
        && radial(*p1, *p2, *z2).0 <= tolerance
        && radial(*p2, *p1, *z1).0 <= tolerance
        && length(cross(*z1, *z2)) * lever(topo, a, b, *z1) <= tolerance
}

/// A cone's apex: `location − (radius / tan α) z`.
pub(crate) fn apex(location: P, z: P, radius: f64, semi_angle: f64) -> P {
    sub(location, scale(z, radius / tan(semi_angle)))
}

/// Section 8.6: faces `a` and `b` lie on one cone (apex, axis direction and
/// semi-angle) with one sense.
pub(crate) fn same_cone(topo: &Topo, a: usize, b: usize) -> bool {
    let surfaces = (
        topo.faces.get(a).map(|f| &f.surface),
        topo.faces.get(b).map(|f| &f.surface),
    );
    let (
        Some(Surface::Cone {
            location: p1,
            z: z1,
            radius: r1,
            semi_angle: s1,
            ..
        }),
        Some(Surface::Cone {
            location: p2,
            z: z2,
            radius: r2,
            semi_angle: s2,
            ..
        }),
    ) = surfaces
    else {
        return false;
    };
    let arm = lever(topo, a, b, *z1);
    sense(topo, a) == sense(topo, b)
        && length(sub(apex(*p1, *z1, *r1, *s1), apex(*p2, *z2, *r2, *s2))) <= IDENTITY_MM
        && dot(*z1, *z2) > 0.0
        && axes_agree(*z1, *z2, arm)
        && (s1 - s2).abs() * arm <= IDENTITY_MM
}

/// Section 8.6: faces `a` and `b` lie on one torus (centre, axis and both
/// radii) with one sense.
pub(crate) fn same_torus(topo: &Topo, a: usize, b: usize) -> bool {
    let surfaces = (
        topo.faces.get(a).map(|f| &f.surface),
        topo.faces.get(b).map(|f| &f.surface),
    );
    let (
        Some(Surface::Torus {
            location: p1,
            z: z1,
            major: big1,
            minor: small1,
            ..
        }),
        Some(Surface::Torus {
            location: p2,
            z: z2,
            major: big2,
            minor: small2,
            ..
        }),
    ) = surfaces
    else {
        return false;
    };
    sense(topo, a) == sense(topo, b)
        && length(sub(*p1, *p2)) <= IDENTITY_MM
        && axes_agree(*z1, *z2, (big1 + small1).max(LEVER_MIN_MM))
        && (big1 - big2).abs() <= IDENTITY_MM
        && (small1 - small2).abs() <= IDENTITY_MM
}
