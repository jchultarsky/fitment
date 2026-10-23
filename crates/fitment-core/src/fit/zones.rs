//! Zones (`docs/research/m7-fit.md`, FZ1 to FZ9): where the socket
//! declares that the target and a named body meet. There, and only there,
//! a body may touch the candidate and overlap it by up to the pair's
//! depth. Each zone is derived here from the socket's requirements as an
//! analytic description the helper builds from primitives, with its closed
//! form (volume, centroid, box) at a depth, which core compares with what
//! the helper built (FZ7).

use std::collections::BTreeSet;
use std::f64::consts::PI;

use crate::features::{Axis, Outline, Part, Segment};
use crate::geom::{Dir3, Point3, Rigid};
use crate::socket::geo::{V, add, cross, dot, norm, perpendiculars, scale, sub};
use crate::socket::{Fit, Flag, Requirement, Requires, SocketDocument, SourceRef};

use super::measures::FitBox;
use super::scene::FitScene;

/// The kind of a zone (FZ2 to FZ4).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ZoneKind {
    /// A seat contact's neighbour face, swept either side of its plane
    /// (FZ2).
    Slab,
    /// An interference, line-to-line or unread-occupant span's annulus
    /// about its axis (FZ3).
    Annulus,
    /// A conical seat's shell about its cone (FZ4).
    Shell,
}

/// Why a zone has no closed form in fitment's reading (FZ7): its pair is
/// `zone_unbuilt`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[non_exhaustive]
pub enum Unbuilt {
    /// The zone's body is a leaf whose part fitment does not read.
    BodyNotRead,
    /// The cited face is not a face of a plane of the body's part.
    NotAPlane,
    /// The cited face's outline is unknown.
    UnknownOutline,
    /// The description overflows, or a revolved profile crosses its
    /// axis.
    Degenerate,
}

/// A zone's analytic description, in the root frame, before any depth
/// (FZ2 to FZ4).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum ZoneShape {
    /// FZ2: the plane face `face` of the zone's body, swept `d` either
    /// side of its plane.
    #[non_exhaustive]
    Slab {
        /// The face's `#id`.
        face: u64,
        /// Its outward unit normal.
        normal: Dir3,
        /// Its plane's offset along the normal, in mm.
        offset_mm: f64,
        /// Its area, in mm².
        area_mm2: f64,
        /// Its centroid.
        centroid_mm: Point3,
        /// Its box.
        face_box: FitBox,
    },
    /// FZ3: the annulus about `axis` from station `from_mm − d` to
    /// `to_mm + d`, between radius `inner_mm − d` (at least 0) and
    /// `outer_mm + d`.
    #[non_exhaustive]
    Annulus {
        /// The axis; stations are measured from its point.
        axis: Axis,
        /// The span's first station.
        from_mm: f64,
        /// Its last.
        to_mm: f64,
        /// The smaller of the two radii, the span's own and the
        /// neighbour's.
        inner_mm: f64,
        /// The larger.
        outer_mm: f64,
    },
    /// FZ4: the cone with apex `apex_mm` opening along `direction` at
    /// `half_angle_rad`, over radii from `radii_mm[0] − d` to
    /// `radii_mm[1] + d`, swept `d` either side of its surface.
    #[non_exhaustive]
    Shell {
        /// The apex.
        apex_mm: Point3,
        /// The direction in which the cone widens.
        direction: Dir3,
        /// The angle between the axis and the surface, in radians.
        half_angle_rad: f64,
        /// The radial range the seat covers.
        radii_mm: [f64; 2],
    },
    /// No closed form: the pair is `zone_unbuilt` (FZ7).
    Unbuilt {
        /// Why.
        reason: Unbuilt,
    },
}

/// One zone, at one occurrence of the target, for one body (FZ1).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Zone {
    /// The socket of the requirement it comes from.
    pub socket: String,
    /// The requirement.
    pub requirement: String,
    /// The target occurrence it applies at, by usage `#id`s.
    pub occurrence: Vec<u64>,
    /// The body it excuses: a neighbour's path, or another selected
    /// occurrence's for a copy (FZ5).
    pub body: Vec<u64>,
    /// True if the body is a copy of the candidate (FZ5).
    pub copy: bool,
    /// Its kind.
    pub kind: ZoneKind,
    /// Its description.
    pub shape: ZoneShape,
}

/// A zone's closed form at one depth (FZ7).
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct Built {
    /// The depth `d`, in mm.
    pub depth_mm: f64,
    /// Its volume, in mm³.
    pub volume_mm3: f64,
    /// Its centroid.
    pub centroid_mm: [f64; 3],
    /// Its box.
    pub bounds: FitBox,
}

impl Zone {
    /// FZ7: the zone's closed form at depth `depth_mm`; `None` for a zone
    /// with no closed form, a depth that is not positive, or a value that
    /// overflows.
    pub fn at(&self, depth_mm: f64) -> Option<Built> {
        if !(depth_mm > 0.0 && depth_mm.is_finite()) {
            return None;
        }
        let built = match &self.shape {
            ZoneShape::Slab {
                normal,
                area_mm2,
                centroid_mm,
                face_box,
                ..
            } => {
                let n = normal.to_array();
                Built {
                    depth_mm,
                    volume_mm3: area_mm2 * 2.0 * depth_mm,
                    centroid_mm: centroid_mm.to_array(),
                    bounds: FitBox {
                        min_mm: [0, 1, 2].map(|i| face_box.min_mm[i] - depth_mm * n[i].abs()),
                        max_mm: [0, 1, 2].map(|i| face_box.max_mm[i] + depth_mm * n[i].abs()),
                    },
                }
            }
            ZoneShape::Annulus {
                axis,
                from_mm,
                to_mm,
                inner_mm,
                outer_mm,
            } => {
                let (s0, s1) = (from_mm - depth_mm, to_mm + depth_mm);
                let (r0, r1) = ((inner_mm - depth_mm).max(0.0), outer_mm + depth_mm);
                revolved(
                    axis.point_mm.to_array(),
                    axis.direction.to_array(),
                    &[[s0, r0], [s1, r0], [s1, r1], [s0, r1]],
                    depth_mm,
                )?
            }
            ZoneShape::Shell {
                apex_mm,
                direction,
                half_angle_rad,
                radii_mm,
            } => shell(
                apex_mm.to_array(),
                direction.to_array(),
                *half_angle_rad,
                *radii_mm,
                depth_mm,
            )?,
            ZoneShape::Unbuilt { .. } => return None,
        };
        let finite = built.volume_mm3.is_finite()
            && built
                .centroid_mm
                .iter()
                .chain(&built.bounds.min_mm)
                .chain(&built.bounds.max_mm)
                .all(|x| x.is_finite());
        finite.then_some(built)
    }

    /// Why the zone has no closed form, if it has none.
    pub fn unbuilt(&self) -> Option<Unbuilt> {
        match &self.shape {
            ZoneShape::Unbuilt { reason } => Some(*reason),
            _ => None,
        }
    }
}

/// The shell of FZ4 at depth `d`: the cone's line over the radii widened
/// by `d`, swept `d` either side along its normal, revolved.
fn shell(apex: V, w: V, alpha: f64, radii: [f64; 2], d: f64) -> Option<Built> {
    let (sin, cos) = (libm::sin(alpha), libm::cos(alpha));
    if !(sin > 0.0 && cos > 0.0) {
        return None;
    }
    let on_line = |rho: f64| [rho * cos / sin, rho];
    let normal = [-sin, cos];
    let (a, b) = (on_line((radii[0] - d).max(0.0)), on_line(radii[1] + d));
    let corner = |p: [f64; 2], k: f64| [p[0] + k * d * normal[0], p[1] + k * d * normal[1]];
    let corners = [
        corner(a, -1.0),
        corner(b, -1.0),
        corner(b, 1.0),
        corner(a, 1.0),
    ];
    if corners.iter().any(|c| c[1] < 0.0) {
        return None;
    }
    revolved(apex, w, &corners, d)
}

/// The solid of revolution of a quadrilateral in the half-plane of
/// (station `s` along unit `w` from `base`, radius `ρ`): its volume and
/// axial centroid by Pappus, from the polygon's moments, and its exact
/// box, whose extremes are at the profile's corners (each circle at
/// station `s` of radius `ρ` reaches `ρ·√(1 − wᵢ²)` either way along axis
/// i, which is linear in the corner).
fn revolved(base: V, w: V, corners: &[[f64; 2]; 4], d: f64) -> Option<Built> {
    let (mut area, mut first, mut second) = (0.0, 0.0, 0.0);
    for (k, p) in corners.iter().enumerate() {
        let q = corners[(k + 1) % corners.len()];
        let c = p[0] * q[1] - q[0] * p[1];
        area += c;
        // ∫ρ dA and ∫sρ dA over the polygon (Green's theorem).
        first += c * (p[1] + q[1]);
        second += c * (p[0] * q[1] + 2.0 * p[0] * p[1] + 2.0 * q[0] * q[1] + q[0] * p[1]);
    }
    let sign = if area < 0.0 { -1.0 } else { 1.0 };
    let (first, second) = (sign * first / 6.0, sign * second / 24.0);
    if first <= 0.0 {
        return None;
    }
    let axial = second / first;
    let reach = w.map(|wi| (1.0 - wi * wi).max(0.0).sqrt());
    let mut bounds = FitBox {
        min_mm: [f64::INFINITY; 3],
        max_mm: [f64::NEG_INFINITY; 3],
    };
    for [s, rho] in corners {
        for i in 0..3 {
            let at = base[i] + s * w[i];
            bounds.min_mm[i] = bounds.min_mm[i].min(at - rho * reach[i]);
            bounds.max_mm[i] = bounds.max_mm[i].max(at + rho * reach[i]);
        }
    }
    Some(Built {
        depth_mm: d,
        volume_mm3: 2.0 * PI * first,
        centroid_mm: add(base, scale(w, axial)),
        bounds,
    })
}

/// Every zone of a socket document, as [`zones`] derives them: only core
/// builds them, so a plan's zones are always fitment's own reading.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Zones(pub(crate) Vec<Zone>);

impl Zones {
    /// The zones, in socket, occurrence and requirement order.
    pub fn as_slice(&self) -> &[Zone] {
        &self.0
    }
}

/// FZ1: every zone of `document`'s sockets, for each occurrence and body,
/// from every requirement, removed ones included (a removal waives what a
/// candidate must satisfy, not the contact the assembly makes), in socket,
/// occurrence and requirement order. `unresolved` requirements, clearance
/// spans, grips and continuations declare none (FZ8).
pub fn zones(document: &SocketDocument, scene: &FitScene) -> Zones {
    let Some(extracted) = document.extracted() else {
        return Zones::default();
    };
    let mut out = Vec::new();
    for socket in &extracted.sockets {
        for occurrence in &socket.occurrences {
            let Some(target) = scene.body(occurrence) else {
                continue;
            };
            for requirement in &socket.requirements {
                let at = At {
                    scene,
                    socket: &socket.id,
                    requirement,
                    occurrence,
                    placed: &target.transform,
                };
                at.zones(&mut out);
            }
        }
    }
    Zones(out)
}

/// One requirement at one occurrence of the target.
struct At<'a> {
    scene: &'a FitScene,
    socket: &'a str,
    requirement: &'a Requirement,
    occurrence: &'a [u64],
    /// `T_k`: the target's frame to the root's at this occurrence.
    placed: &'a Rigid,
}

impl At<'_> {
    fn zones(&self, out: &mut Vec<Zone>) {
        match &self.requirement.requires {
            Requires::Seat { contacts, .. } => {
                let mut seen = BTreeSet::new();
                for toucher in contacts.iter().flat_map(|c| &c.by) {
                    // A plane contact's source (C1, C5) names it: a ring
                    // (H1) is an edge on the plane, not a contact of faces.
                    let named = self.sources().any(|s| {
                        s.rule.starts_with('C')
                            && s.neighbour == toucher.neighbour
                            && s.faces.contains(&toucher.face)
                    });
                    if named && seen.insert((toucher.neighbour.clone(), toucher.face)) {
                        out.push(self.zone(&toucher.neighbour, ZoneKind::Slab, || {
                            self.slab(&toucher.neighbour, toucher.face)
                        }));
                    }
                }
            }
            Requires::Bore { axis, spans, .. } => {
                for span in spans.iter().filter(|s| zoned(s.fit, s.pinned)) {
                    let own = span.hole_diameter_mm / 2.0;
                    self.annuli(axis, (span.from_mm, span.to_mm), own, -1.0, out);
                }
            }
            Requires::Shaft { axis, spans, .. } => {
                for span in spans.iter().filter(|s| zoned(s.fit, s.pinned)) {
                    let own = span.shaft_diameter_mm / 2.0;
                    self.annuli(axis, (span.from_mm, span.to_mm), own, 1.0, out);
                }
            }
            Requires::ConicalSeat {
                axis,
                apex_mm,
                included_angle_rad,
                radii_mm: Some(radii),
                ..
            } => {
                let mut seen = BTreeSet::new();
                for source in self.sources().filter(|s| !s.neighbour.is_empty()) {
                    if seen.insert(source.neighbour.clone()) {
                        out.push(self.zone(&source.neighbour, ZoneKind::Shell, || {
                            self.shell(axis, *apex_mm, included_angle_rad / 2.0, *radii)
                        }));
                    }
                }
            }
            _ => {}
        }
    }

    /// The requirement's sources at this occurrence.
    fn sources(&self) -> impl Iterator<Item = &SourceRef> {
        self.requirement
            .sources
            .iter()
            .filter(|s| s.occurrence == self.occurrence)
    }

    fn zone(&self, body: &[u64], kind: ZoneKind, shape: impl FnOnce() -> ZoneShape) -> Zone {
        let copy = self.scene.body(body).is_some_and(|b| b.selected)
            || self
                .sources()
                .any(|s| s.neighbour == body && s.flags.contains(&Flag::SelfPaired));
        Zone {
            socket: self.socket.to_owned(),
            requirement: self.requirement.id.clone(),
            occurrence: self.occurrence.to_vec(),
            body: body.to_vec(),
            copy,
            kind,
            shape: shape(),
        }
    }

    /// FZ3: one annulus for each neighbour a tight source at this
    /// occurrence names over the span, over the stations where it names it;
    /// the neighbour's radius is the span's own less the source's margin in
    /// a bore (`sense` −1), plus it about a shaft (+1).
    fn annuli(
        &self,
        axis: &Axis,
        (from, to): (f64, f64),
        own: f64,
        sense: f64,
        out: &mut Vec<Zone>,
    ) {
        let e = 1e-9 * (1.0 + from.abs().max(to.abs()));
        let mut seen = BTreeSet::new();
        for source in self
            .sources()
            .filter(|s| tight(s) && !s.neighbour.is_empty())
        {
            let Some([a, b]) = source.span_mm else {
                continue;
            };
            let (lo, hi) = (from.max(a.min(b)), to.min(a.max(b)));
            if hi - lo <= e {
                continue;
            }
            let theirs = own + sense * source.margin_mm.unwrap_or(0.0);
            let key = (
                source.neighbour.clone(),
                theirs.to_bits(),
                lo.to_bits(),
                hi.to_bits(),
            );
            if !seen.insert(key) {
                continue;
            }
            out.push(self.zone(&source.neighbour, ZoneKind::Annulus, || {
                let placed = self
                    .placed
                    .apply_point(axis.point_mm)
                    .zip(self.placed.apply_dir(axis.direction));
                match placed {
                    Some((point_mm, direction)) if theirs.is_finite() => ZoneShape::Annulus {
                        axis: Axis {
                            point_mm,
                            direction,
                        },
                        from_mm: lo,
                        to_mm: hi,
                        inner_mm: own.min(theirs),
                        outer_mm: own.max(theirs),
                    },
                    _ => unbuilt(Unbuilt::Degenerate),
                }
            }));
        }
    }

    /// FZ4: the shell of a conical seat, its cone opening from the apex
    /// towards the axis's point.
    fn shell(&self, axis: &Axis, apex: Point3, half_angle: f64, radii: [f64; 2]) -> ZoneShape {
        let towards = axis
            .point_mm
            .minus(apex)
            .and_then(|v| v.dot(crate::geom::Vec3::new(axis.direction.to_array())?));
        let direction = if towards.is_some_and(|t| t < 0.0) {
            axis.direction.reversed()
        } else {
            axis.direction
        };
        match (
            self.placed.apply_point(apex),
            self.placed.apply_dir(direction),
        ) {
            (Some(apex_mm), Some(direction)) if half_angle > 0.0 && half_angle < PI / 2.0 => {
                ZoneShape::Shell {
                    apex_mm,
                    direction,
                    half_angle_rad: half_angle,
                    radii_mm: [radii[0].min(radii[1]), radii[0].max(radii[1])],
                }
            }
            _ => unbuilt(Unbuilt::Degenerate),
        }
    }

    /// FZ2: the slab of `body`'s face `face`, placed where the body is.
    fn slab(&self, body: &[u64], face: u64) -> ZoneShape {
        let Some((part, placed)) = self
            .scene
            .body(body)
            .and_then(|b| Some((self.scene.part(b)?, b.transform)))
        else {
            return unbuilt(Unbuilt::BodyNotRead);
        };
        slab(part, &placed, face).unwrap_or_else(unbuilt)
    }
}

fn unbuilt(reason: Unbuilt) -> ZoneShape {
    ZoneShape::Unbuilt { reason }
}

/// FZ3, FZ8: a source whose own relation is tight: line to line (B2, I2),
/// an interference (B3, B4, I2) or an unread occupant (B8, I7). A
/// clearance (B1, I1), a continuation (J1, J2), an abutment (I6) or a
/// rim's provenance (B6) declares no contact.
fn tight(source: &SourceRef) -> bool {
    matches!(
        source.rule.as_str(),
        "B2" | "B3" | "B4" | "B8" | "I2" | "I7"
    )
}

/// FZ3: a span whose fit is an interference, line to line, or a pinned
/// unread occupant declares a zone; a clearance span declares none (FZ8).
fn zoned(fit: Fit, pinned: bool) -> bool {
    match fit {
        Fit::Interference | Fit::LineToLine => true,
        Fit::NotRead => pinned,
        Fit::Clearance => false,
    }
}

/// FZ2's description of plane face `face` of `part` placed by `placed`:
/// its normal, offset, area, centroid and box in the root frame, from its
/// outline's lines and arcs, exactly.
pub(crate) fn slab(part: &Part, placed: &Rigid, face: u64) -> Result<ZoneShape, Unbuilt> {
    let (plane, own) = part
        .planes
        .iter()
        .find_map(|p| Some((p, p.faces.iter().find(|f| f.face == face)?)))
        .ok_or(Unbuilt::NotAPlane)?;
    let Outline::Known { loops } = &own.outline else {
        return Err(Unbuilt::UnknownOutline);
    };
    let normal = placed.apply_dir(plane.normal).ok_or(Unbuilt::Degenerate)?;
    let n = normal.to_array();
    let point = |p: &Point3| placed.apply_point(*p).map(Point3::to_array);
    let mut segments = Vec::new();
    for segment in loops.iter().flat_map(|l| &l.segments) {
        segments.push(match segment {
            Segment::Line { from_mm, to_mm, .. } => Piece::Line {
                a: point(from_mm).ok_or(Unbuilt::Degenerate)?,
                b: point(to_mm).ok_or(Unbuilt::Degenerate)?,
            },
            Segment::Arc {
                from_mm,
                to_mm,
                center_mm,
                radius_mm,
                sweep_rad,
                ..
            } => Piece::Arc {
                a: point(from_mm).ok_or(Unbuilt::Degenerate)?,
                b: point(to_mm).ok_or(Unbuilt::Degenerate)?,
                c: point(center_mm).ok_or(Unbuilt::Degenerate)?,
                r: *radius_mm,
                sweep: *sweep_rad,
            },
        });
    }
    let first = segments.first().ok_or(Unbuilt::UnknownOutline)?.start();
    let (area, centroid) = region(&segments, n, first).ok_or(Unbuilt::Degenerate)?;
    let face_box = face_box(&segments, n);
    let offset_mm = dot(n, first);
    let shape = ZoneShape::Slab {
        face,
        normal,
        offset_mm,
        area_mm2: area,
        centroid_mm: Point3::new(centroid).ok_or(Unbuilt::Degenerate)?,
        face_box,
    };
    (area > 0.0 && offset_mm.is_finite())
        .then_some(shape)
        .ok_or(Unbuilt::Degenerate)
}

/// One edge of a face's outline, in the root frame.
#[derive(Debug, Clone, Copy)]
enum Piece {
    Line {
        a: V,
        b: V,
    },
    Arc {
        a: V,
        b: V,
        c: V,
        r: f64,
        sweep: f64,
    },
}

impl Piece {
    fn start(&self) -> V {
        match self {
            Self::Line { a, .. } | Self::Arc { a, .. } => *a,
        }
    }
}

/// The area and centroid of a plane region bounded by `pieces` (outer
/// loop counterclockwise about `n`, holes clockwise), from a fan of
/// triangles about `origin` and, for each arc, the circular segment
/// between its chord and itself.
fn region(pieces: &[Piece], normal: V, origin: V) -> Option<(f64, V)> {
    let (u, v) = perpendiculars(normal);
    let flat = |p: V| {
        let from = sub(p, origin);
        [dot(from, u), dot(from, v)]
    };
    let (mut area, mut moment) = (0.0, [0.0, 0.0]);
    let mut add_part = |a: f64, at: [f64; 2]| {
        area += a;
        moment[0] += a * at[0];
        moment[1] += a * at[1];
    };
    for piece in pieces {
        let (from, to) = match piece {
            Piece::Line { a, b } | Piece::Arc { a, b, .. } => (flat(*a), flat(*b)),
        };
        let triangle = 0.5 * (from[0] * to[1] - from[1] * to[0]);
        add_part(triangle, [(from[0] + to[0]) / 3.0, (from[1] + to[1]) / 3.0]);
        if let Piece::Arc { c, r, sweep, .. } = piece {
            add_segment(flat(*c), from, *r, *sweep, &mut add_part)?;
        }
    }
    if area <= 0.0 || !area.is_finite() {
        return None;
    }
    let at = [moment[0] / area, moment[1] / area];
    Some((area, add(origin, add(scale(u, at[0]), scale(v, at[1])))))
}

/// The circular segment between an arc's chord and itself, of radius `r`
/// about `centre`, from `from` sweeping `sweep`: its signed area
/// `r²(θ − sin θ)/2` and its centroid `4r·sin³(θ/2) / 3(θ − sin θ)` from
/// the centre towards the arc's middle; `None` for an arc of no radius.
fn add_segment(
    centre: [f64; 2],
    from: [f64; 2],
    r: f64,
    sweep: f64,
    add_part: &mut impl FnMut(f64, [f64; 2]),
) -> Option<()> {
    let lens = sweep - libm::sin(sweep);
    if lens.abs() == 0.0 {
        return Some(());
    }
    let (sin, cos) = libm::sincos(sweep / 2.0);
    let start = [from[0] - centre[0], from[1] - centre[1]];
    let length = (start[0] * start[0] + start[1] * start[1]).sqrt();
    if length <= 0.0 {
        return None;
    }
    // The direction from the centre to the arc's middle.
    let middle = [
        (start[0] * cos - start[1] * sin) / length,
        (start[0] * sin + start[1] * cos) / length,
    ];
    let reach = 4.0 * r * sin * sin * sin / (3.0 * lens);
    add_part(
        0.5 * r * r * lens,
        [centre[0] + middle[0] * reach, centre[1] + middle[1] * reach],
    );
    Some(())
}

/// The exact box of a plane region bounded by `pieces`: each line by its
/// ends, each arc by its ends and the extremes along each axis it sweeps
/// through.
fn face_box(pieces: &[Piece], n: V) -> FitBox {
    let mut bounds = FitBox {
        min_mm: [f64::INFINITY; 3],
        max_mm: [f64::NEG_INFINITY; 3],
    };
    let mut take = |p: V| {
        for (i, x) in p.iter().enumerate() {
            bounds.min_mm[i] = bounds.min_mm[i].min(*x);
            bounds.max_mm[i] = bounds.max_mm[i].max(*x);
        }
    };
    for piece in pieces {
        match *piece {
            Piece::Line { a, b } => {
                take(a);
                take(b);
            }
            Piece::Arc { a, b, c, r, sweep } => {
                take(a);
                take(b);
                let from = sub(a, c);
                let length = norm(from);
                if length <= 0.0 {
                    continue;
                }
                let e1 = scale(from, 1.0 / length);
                let e2 = cross(n, e1);
                for i in 0..3 {
                    let peak = libm::atan2(e2[i], e1[i]);
                    for angle in [peak, peak + PI] {
                        if swept(angle, sweep) {
                            let (sin, cos) = libm::sincos(angle);
                            take(add(c, add(scale(e1, r * cos), scale(e2, r * sin))));
                        }
                    }
                }
            }
        }
    }
    bounds
}

/// True if `angle` lies within a sweep from 0 to `sweep` (signed).
fn swept(angle: f64, sweep: f64) -> bool {
    let turn = 2.0 * PI;
    if sweep.abs() >= turn {
        return true;
    }
    let a = angle.rem_euclid(turn);
    if sweep >= 0.0 {
        a <= sweep
    } else {
        a == 0.0 || a - turn >= sweep
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64, by: f64) -> bool {
        (a - b).abs() <= by
    }

    fn line(a: V, b: V) -> Piece {
        Piece::Line { a, b }
    }

    /// FZ7's closed forms for a slab: a 10 × 20 rectangle with a hole of
    /// radius 2 at (3, 4): area, centroid by moments, the box of its
    /// outer edges; at depth d its volume is area × 2d and its box the
    /// face's widened by d along the normal.
    #[test]
    fn a_slab_has_its_faces_moments() {
        let n = [0.0, 0.0, 1.0];
        let outer = [
            line([0.0, 0.0, 5.0], [10.0, 0.0, 5.0]),
            line([10.0, 0.0, 5.0], [10.0, 20.0, 5.0]),
            line([10.0, 20.0, 5.0], [0.0, 20.0, 5.0]),
            line([0.0, 20.0, 5.0], [0.0, 0.0, 5.0]),
        ];
        let hole = Piece::Arc {
            a: [5.0, 4.0, 5.0],
            b: [5.0, 4.0, 5.0],
            c: [3.0, 4.0, 5.0],
            r: 2.0,
            sweep: -2.0 * PI,
        };
        let mut pieces = outer.to_vec();
        pieces.push(hole);
        let (area, centroid) = region(&pieces, n, [0.0, 0.0, 5.0]).unwrap();
        let hole_area = PI * 4.0;
        assert!(close(area, 200.0 - hole_area, 1e-9));
        let cx = (200.0 * 5.0 - hole_area * 3.0) / area;
        let cy = (200.0 * 10.0 - hole_area * 4.0) / area;
        assert!(close(centroid[0], cx, 1e-9), "{centroid:?}");
        assert!(close(centroid[1], cy, 1e-9), "{centroid:?}");
        assert!(close(centroid[2], 5.0, 1e-12));
        let bounds = face_box(&pieces, n);
        assert_eq!(bounds.min_mm, [0.0, 0.0, 5.0]);
        assert_eq!(bounds.max_mm, [10.0, 20.0, 5.0]);
        let zone = Zone {
            socket: "socket-1".to_owned(),
            requirement: "seat:plane-1".to_owned(),
            occurrence: vec![1],
            body: vec![2],
            copy: false,
            kind: ZoneKind::Slab,
            shape: ZoneShape::Slab {
                face: 7,
                normal: Dir3::from_unit(n).unwrap(),
                offset_mm: 5.0,
                area_mm2: area,
                centroid_mm: Point3::new(centroid).unwrap(),
                face_box: bounds,
            },
        };
        let built = zone.at(0.01).unwrap();
        assert!(close(built.volume_mm3, area * 0.02, 1e-12));
        assert_eq!(built.bounds.min_mm, [0.0, 0.0, 4.99]);
        assert_eq!(built.bounds.max_mm, [10.0, 20.0, 5.01]);
        assert_eq!(zone.at(0.0), None);
        assert_eq!(zone.at(-1.0), None);
    }

    /// A quarter disc bounded by two lines and a counterclockwise arc: its
    /// area πr²/4 and its centroid 4r/3π from each edge; its box reaches
    /// the arc's ends only, since the arc passes no extreme between them.
    #[test]
    fn an_arc_adds_its_segment() {
        let n = [0.0, 0.0, 1.0];
        let r = 3.0;
        let pieces = [
            line([0.0, 0.0, 0.0], [r, 0.0, 0.0]),
            Piece::Arc {
                a: [r, 0.0, 0.0],
                b: [0.0, r, 0.0],
                c: [0.0, 0.0, 0.0],
                r,
                sweep: PI / 2.0,
            },
            line([0.0, r, 0.0], [0.0, 0.0, 0.0]),
        ];
        let (area, centroid) = region(&pieces, n, [0.0; 3]).unwrap();
        assert!(close(area, PI * r * r / 4.0, 1e-12));
        let at = 4.0 * r / (3.0 * PI);
        assert!(close(centroid[0], at, 1e-12) && close(centroid[1], at, 1e-12));
        let bounds = face_box(&pieces, n);
        assert_eq!(bounds.max_mm, [r, r, 0.0]);
        // A half disc, the arc through +y: its box reaches y = r at the
        // arc's middle, between its ends.
        let half = [
            Piece::Arc {
                a: [r, 0.0, 0.0],
                b: [-r, 0.0, 0.0],
                c: [0.0, 0.0, 0.0],
                r,
                sweep: PI,
            },
            line([-r, 0.0, 0.0], [r, 0.0, 0.0]),
        ];
        let bounds = face_box(&half, n);
        assert!(close(bounds.max_mm[1], r, 1e-12));
        assert!(close(bounds.min_mm[1], 0.0, 1e-12));
        let (area, centroid) = region(&half, n, [r, 0.0, 0.0]).unwrap();
        assert!(close(area, PI * r * r / 2.0, 1e-12));
        assert!(close(centroid[1], 4.0 * r / (3.0 * PI), 1e-12));
    }

    /// FZ3's annulus by Pappus: π(R² − r²)·L, its centroid at the middle
    /// station on the axis, its box the outer circles' at both ends; the
    /// inner radius never below 0.
    #[test]
    fn an_annulus_is_its_ring() {
        let axis = Axis {
            point_mm: Point3::new([1.0, 2.0, 3.0]).unwrap(),
            direction: Dir3::from_unit([0.0, 0.0, 1.0]).unwrap(),
        };
        let zone = Zone {
            socket: "socket-1".to_owned(),
            requirement: "bore:hole-1".to_owned(),
            occurrence: vec![1],
            body: vec![2],
            copy: false,
            kind: ZoneKind::Annulus,
            shape: ZoneShape::Annulus {
                axis,
                from_mm: 0.0,
                to_mm: 10.0,
                inner_mm: 2.5,
                outer_mm: 3.0,
            },
        };
        let d = 0.01;
        let built = zone.at(d).unwrap();
        let (r, big, length) = (2.5 - d, 3.0 + d, 10.0 + 2.0 * d);
        assert!(close(
            built.volume_mm3,
            PI * (big * big - r * r) * length,
            1e-9
        ));
        assert!(close(built.centroid_mm[2], 3.0 + 5.0, 1e-12));
        assert!(close(built.centroid_mm[0], 1.0, 1e-12));
        assert!(close(built.bounds.min_mm[0], 1.0 - big, 1e-12));
        assert!(close(built.bounds.max_mm[1], 2.0 + big, 1e-12));
        assert!(close(built.bounds.min_mm[2], 3.0 - d, 1e-12));
        assert!(close(built.bounds.max_mm[2], 13.0 + d, 1e-12));
        let ZoneShape::Annulus { .. } = zone.shape else {
            unreachable!()
        };
        let mut thin = zone.clone();
        thin.shape = ZoneShape::Annulus {
            axis,
            from_mm: 0.0,
            to_mm: 1.0,
            inner_mm: 0.001,
            outer_mm: 0.001,
        };
        let solid = thin.at(0.01).unwrap();
        assert!(close(solid.volume_mm3, PI * 0.011 * 0.011 * 1.02, 1e-15));
    }

    /// FZ4's shell by Pappus: a cone of half angle 45° about z from the
    /// origin, over radii 2 to 4, swept d either side: its volume is
    /// 2π · ρ̄ · (length · 2d), and its centroid's station the moments'.
    #[test]
    fn a_shell_is_its_revolved_strip() {
        let alpha = PI / 4.0;
        let depth = 0.01;
        let built = shell([0.0; 3], [0.0, 0.0, 1.0], alpha, [2.0, 4.0], depth).unwrap();
        let (low, high) = (2.0 - depth, 4.0 + depth);
        let length = (high - low) / libm::sin(alpha);
        let mean = f64::midpoint(low, high);
        let expected = 2.0 * PI * mean * length * 2.0 * depth;
        assert!(close(built.volume_mm3, expected, 1e-9));
        // Numerically, by the strip's own integral.
        let (steps, mut weight, mut moment) = (2000, 0.0, 0.0);
        let (sin, cos) = libm::sincos(alpha);
        for along in 0..steps {
            for across in 0..20 {
                let run = (f64::from(along) + 0.5) / f64::from(steps) * length;
                let off = -depth + (f64::from(across) + 0.5) / 20.0 * 2.0 * depth;
                let station = low / libm::tan(alpha) + run * cos - off * sin;
                let rho = low + run * sin + off * cos;
                weight += rho;
                moment += station * rho;
            }
        }
        let axial = moment / weight;
        assert!(
            close(built.centroid_mm[2], axial, 1e-6),
            "{:?}",
            built.centroid_mm
        );
        assert!(close(built.bounds.max_mm[0], high + depth * cos, 1e-12));
        assert!(built.bounds.min_mm[2] < 2.0 && built.bounds.max_mm[2] > 4.0);
        // A strip that would cross the axis has no closed form.
        assert_eq!(
            shell([0.0; 3], [0.0, 0.0, 1.0], alpha, [0.0, 1.0], depth),
            None
        );
    }

    /// FZ7: a cited face that is not a plane's, or whose outline is
    /// unknown, has no closed form; the zone is unbuilt, and so has no
    /// form at any depth.
    #[test]
    fn a_face_without_a_closed_form_is_unbuilt() {
        let bytes = include_bytes!("../../tests/assemblies/s01_bolted.stp");
        let document = SocketDocument::extract(bytes, Some("BRACKET"), None).unwrap();
        let scene = FitScene::of(bytes, &document).unwrap();
        let plate = scene
            .bodies()
            .iter()
            .find(|b| b.label().starts_with("PLATE"))
            .unwrap();
        let part = scene.part(plate).unwrap();
        let cylinder = part.cylinders[0].faces[0];
        assert_eq!(
            slab(part, &plate.transform, cylinder),
            Err(Unbuilt::NotAPlane)
        );
        let top = part.planes[0].faces[0].face;
        assert!(slab(part, &plate.transform, top).is_ok());
        let mut unknown = part.clone();
        unknown.planes[0].faces[0].outline = Outline::Unknown {
            problem: crate::features::OutlineProblem::CurveNotRead,
            instance: top,
        };
        assert_eq!(
            slab(&unknown, &plate.transform, top),
            Err(Unbuilt::UnknownOutline)
        );
        let zone = Zone {
            socket: "socket-1".to_owned(),
            requirement: "seat:plane-1".to_owned(),
            occurrence: vec![1],
            body: plate.path.clone(),
            copy: false,
            kind: ZoneKind::Slab,
            shape: unbuilt(Unbuilt::NotAPlane),
        };
        assert_eq!(zone.at(0.01), None);
        assert_eq!(zone.unbuilt(), Some(Unbuilt::NotAPlane));
    }

    /// An arc sweeps the angles between 0 and its sweep, either way.
    #[test]
    fn sweeps_hold_their_angles() {
        assert!(swept(0.5, 1.0));
        assert!(!swept(1.5, 1.0));
        assert!(swept(-0.5, -1.0));
        assert!(!swept(0.5, -1.0));
        assert!(swept(3.0, 2.0 * PI));
        assert!(swept(0.0, -1.0));
    }

    /// FZ3 and FZ8: interference, line-to-line and pinned unread spans
    /// declare a zone; clearance spans and unpinned unread ones none.
    #[test]
    fn only_tight_spans_are_zoned() {
        assert!(zoned(Fit::Interference, true));
        assert!(zoned(Fit::LineToLine, true));
        assert!(zoned(Fit::NotRead, true));
        assert!(!zoned(Fit::NotRead, false));
        assert!(!zoned(Fit::Clearance, false));
        assert!(!zoned(Fit::Clearance, true));
    }
}
