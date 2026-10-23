//! The scene fitment reads from a socket's assembly (`docs/research/m7-fit.md`,
//! FB1, FB3): every leaf occurrence with its solid's and faces' `#id`s, its
//! transform to the root, its stated uncertainties and up to four landmark
//! faces, read by fitment's own reader from the bytes the socket came from.
//! The helper is handed this list and must find each body in its own
//! reading (FB2, FB6); core compares what it finds with what is here.

use std::collections::{BTreeMap, BTreeSet};

use crate::assembly::{ComponentRead, DefinitionKind, read_assembly};
use crate::features::{Axis, CylinderClass, DefinitionRef, Extent, Part};
use crate::gate::Refusal;
use crate::geom::{Dir3, Rigid};
use crate::sha256::sha256_hex;
use crate::socket::SocketDocument;

/// How many plane and cylinder faces each body names as landmarks (FB3).
const LANDMARK_PLANES: usize = 2;
const LANDMARK_CYLINDERS: usize = 2;

/// Why no scene can be read (FB1): the run is refused before any
/// candidate.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum FitSceneError {
    /// The socket document is refused: it names no occurrence to fit.
    #[error("the socket document is refused, so it has no scene")]
    NotAccepted,
    /// The assembly's SHA-256 is not the socket's `source`.
    #[error(
        "the assembly is not the socket's source: its SHA-256 is {found}, the socket's {expected}"
    )]
    NotTheSource {
        /// The socket's `source.sha256`.
        expected: String,
        /// The assembly's.
        found: String,
    },
    /// fitment's reader refuses the assembly, with the gate's reasons.
    #[error("the assembly is refused")]
    Refused(Vec<Refusal>),
    /// The assembly and the socket disagree on a selected occurrence.
    #[error("the assembly and the socket disagree: {0}")]
    Mismatch(String),
}

/// What a landmark face states, in the frame it is given in (FB3): a
/// plane's outward unit normal and offset, or a cylinder's axis and radius.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub enum LandmarkValue {
    /// A plane face: every x with `normal · x = offset_mm`.
    #[non_exhaustive]
    Plane {
        /// Its outward unit normal.
        normal: Dir3,
        /// Its offset, in mm.
        offset_mm: f64,
    },
    /// A face of a full cylinder.
    #[non_exhaustive]
    Cylinder {
        /// Its axis.
        axis: Axis,
        /// Its radius, in mm.
        radius_mm: f64,
    },
}

/// One landmark face of a body (FB3): its `#id` and fitment's own value
/// of it.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct Landmark {
    /// The face's `#id`.
    pub face: u64,
    /// fitment's value of it.
    pub value: LandmarkValue,
}

impl Landmark {
    /// This landmark moved by `by`, from the frame it is stated in into
    /// `by`'s parent; `None` if a value overflows.
    pub fn placed(&self, by: &Rigid) -> Option<Self> {
        let value = match self.value {
            LandmarkValue::Plane { normal, offset_mm } => {
                let moved = by.apply_dir(normal)?;
                let shift: f64 = moved
                    .to_array()
                    .iter()
                    .zip(by.translation())
                    .map(|(n, t)| n * t)
                    .sum();
                LandmarkValue::Plane {
                    normal: moved,
                    offset_mm: crate::geom::finite(offset_mm + shift)?,
                }
            }
            LandmarkValue::Cylinder { axis, radius_mm } => LandmarkValue::Cylinder {
                axis: Axis {
                    point_mm: by.apply_point(axis.point_mm)?,
                    direction: by.apply_dir(axis.direction)?,
                },
                radius_mm,
            },
        };
        Some(Self {
            face: self.face,
            value,
        })
    }
}

/// FB3: the landmark faces of `part`, in its own frame, in a fixed order:
/// the two largest planes by known area (each by its largest face of known
/// area, ties to the smaller `#id`), then the two largest full cylinders
/// by radius (each by its first face); then, while their normals and axes
/// span fewer than three directions or leave a slide that moves none of
/// them, each further plane (by area, those of unknown area last) and full
/// cylinder (by radius) that adds a direction or fixes a slide. A plane's
/// value is its normal with that face's own offset. So a body moved by any
/// rigid motion moves a landmark, unless its faces themselves allow none.
pub fn landmarks(part: &Part) -> Vec<Landmark> {
    let mut planes: Vec<(f64, u64, Landmark)> = part
        .planes
        .iter()
        .filter_map(|plane| {
            let known = plane
                .faces
                .iter()
                .filter_map(|f| f.area_mm2.map(|a| (a, f)))
                .max_by(|(a, f), (b, g)| a.total_cmp(b).then(g.face.cmp(&f.face)))
                .map(|(_, f)| f);
            let face = known.or_else(|| plane.faces.first())?;
            Some((
                plane.area_mm2.unwrap_or(f64::NEG_INFINITY),
                plane.id.instance(),
                Landmark {
                    face: face.face,
                    value: LandmarkValue::Plane {
                        normal: plane.normal,
                        offset_mm: face.offset_mm,
                    },
                },
            ))
        })
        .collect();
    planes.sort_by(|(a, i, _), (b, j, _)| b.total_cmp(a).then(i.cmp(j)));
    let mut cylinders: Vec<(f64, u64, Landmark)> = part
        .cylinders
        .iter()
        .filter(|c| {
            matches!(
                c.class,
                CylinderClass::Full { .. } | CylinderClass::FullOneEndIrregular { .. }
            )
        })
        .filter_map(|c| {
            Some((
                c.radius_mm,
                c.id.instance(),
                Landmark {
                    face: *c.faces.first()?,
                    value: LandmarkValue::Cylinder {
                        axis: c.axis,
                        radius_mm: c.radius_mm,
                    },
                },
            ))
        })
        .collect();
    cylinders.sort_by(|(a, i, _), (b, j, _)| b.total_cmp(a).then(i.cmp(j)));
    let (planes, cylinders): (Vec<Landmark>, Vec<Landmark>) = (
        planes.into_iter().map(|(_, _, l)| l).collect(),
        cylinders.into_iter().map(|(_, _, l)| l).collect(),
    );
    let mut chosen: Vec<Landmark> = planes
        .iter()
        .take(LANDMARK_PLANES)
        .chain(cylinders.iter().take(LANDMARK_CYLINDERS))
        .copied()
        .collect();
    let rest = planes
        .iter()
        .skip(LANDMARK_PLANES)
        .chain(cylinders.iter().skip(LANDMARK_CYLINDERS));
    for landmark in rest {
        let now = fixes(&chosen);
        if now == (3, 3) {
            break;
        }
        let mut more = chosen.clone();
        more.push(*landmark);
        let then = fixes(&more);
        if then.0 > now.0 || then.1 > now.1 {
            chosen = more;
        }
    }
    chosen
}

/// How far `landmarks` fix a rigid motion: the number of independent
/// directions among their normals and axes, and of independent slides
/// they see (a plane sees a slide along its normal, a cylinder one across
/// its axis).
fn fixes(landmarks: &[Landmark]) -> (usize, usize) {
    let mut directions = Vec::new();
    let mut slides = Vec::new();
    for landmark in landmarks {
        match landmark.value {
            LandmarkValue::Plane { normal, .. } => {
                directions.push(normal.to_array());
                slides.push(normal.to_array());
            }
            LandmarkValue::Cylinder { axis, .. } => {
                let d = axis.direction.to_array();
                let (u, v) = crate::socket::geo::perpendiculars(d);
                directions.push(d);
                slides.extend([u, v]);
            }
        }
    }
    (rank(&directions), rank(&slides))
}

/// The number of independent vectors among unit `vectors`, by
/// Gram-Schmidt with a floor of 1e-6.
fn rank(vectors: &[[f64; 3]]) -> usize {
    use crate::socket::geo::{dot, norm, scale, sub};
    let mut basis: Vec<[f64; 3]> = Vec::new();
    for v in vectors {
        let mut rest = *v;
        for b in &basis {
            rest = sub(rest, scale(*b, dot(rest, *b)));
        }
        let length = norm(rest);
        if length > 1e-6 && basis.len() < 3 {
            basis.push(scale(rest, 1.0 / length));
        }
    }
    basis.len()
}

/// Every face `#id` of `part`'s solid, sorted.
pub(crate) fn faces_of(part: &Part) -> Vec<u64> {
    let mut faces: Vec<u64> = part
        .planes
        .iter()
        .flat_map(|p| p.faces.iter().map(|f| f.face))
        .chain(part.cylinders.iter().flat_map(|c| c.faces.iter().copied()))
        .chain(part.cones.iter().flat_map(|c| c.faces.iter().copied()))
        .chain(part.tori.iter().flat_map(|t| t.faces.iter().copied()))
        .chain(part.other_faces.iter().map(|o| o.face))
        .collect();
    faces.sort_unstable();
    faces.dedup();
    faces
}

/// One leaf occurrence of the assembly whose part fitment reads (FB1).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct SceneBody {
    /// Its path, by usage `#id`s: the body's key.
    pub path: Vec<u64>,
    /// Its path as people write it.
    pub name: String,
    /// Its definition.
    pub definition: DefinitionRef,
    /// Its solid's `#id`.
    pub solid: u64,
    /// Its solid's faces' `#id`s, sorted.
    pub faces: Vec<u64>,
    /// Its frame to the root's.
    pub transform: Rigid,
    /// Its part's largest stated uncertainty, in mm.
    pub part_uncertainty_mm: f64,
    /// Its part's solid's largest vertex offset, in mm.
    pub vertex_offset_mm: f64,
    /// The largest uncertainty the contexts it is placed through state.
    pub placement_uncertainty_mm: f64,
    /// True for an occurrence of the target the socket selects.
    pub selected: bool,
    /// Its landmarks, in the root frame (FB3).
    pub landmarks: Vec<Landmark>,
    /// Its part's sound box, in the part's own frame, if it has one.
    pub extent: Option<Extent>,
    part: usize,
}

impl SceneBody {
    /// The body for people: its product's id (else its name, else its
    /// definition's `#id`), then its path.
    pub fn label(&self) -> String {
        let product = self
            .definition
            .product
            .as_ref()
            .and_then(|p| p.id.clone().or_else(|| p.name.clone()))
            .unwrap_or_else(|| format!("#{}", self.definition.instance));
        format!("{product} ({})", self.name)
    }
}

/// A leaf occurrence whose part fitment refuses: it has no body fitment
/// can name, so the helper's solid for it is unclaimed (FB6).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct UnreadLeaf {
    /// Its path, by usage `#id`s.
    pub path: Vec<u64>,
    /// Its path as people write it.
    pub name: String,
    /// Its definition.
    pub definition: DefinitionRef,
}

/// The assembly as fitment reads it, for a fit (FB1): every leaf
/// occurrence, by path.
#[derive(Debug, Clone, PartialEq)]
pub struct FitScene {
    sha256: String,
    bodies: Vec<SceneBody>,
    unread: Vec<UnreadLeaf>,
    parts: Vec<Part>,
}

impl FitScene {
    /// FB1: the scene of `assembly`, the bytes `socket` was read from.
    ///
    /// # Errors
    ///
    /// [`FitSceneError`] for a refused socket document, bytes that are not
    /// its source, an assembly fitment refuses, or an assembly whose
    /// selected occurrences are not the socket's.
    pub fn of(assembly: &[u8], socket: &SocketDocument) -> Result<Self, FitSceneError> {
        let extracted = socket.extracted().ok_or(FitSceneError::NotAccepted)?;
        let found = sha256_hex(assembly);
        if found != socket.source().sha256 {
            return Err(FitSceneError::NotTheSource {
                expected: socket.source().sha256.clone(),
                found,
            });
        }
        let read = read_assembly(assembly);
        let Some(listing) = read.listing().filter(|_| read.gate_refusals().is_empty()) else {
            return Err(FitSceneError::Refused(read.gate_refusals().to_vec()));
        };
        let selected: BTreeMap<&[u64], &Rigid> = extracted
            .occurrences
            .iter()
            .map(|o| (o.path.as_slice(), &o.transform))
            .collect();
        let mut scene = Self {
            sha256: found,
            bodies: Vec::new(),
            unread: Vec::new(),
            parts: Vec::new(),
        };
        let mut parts: BTreeMap<usize, usize> = BTreeMap::new();
        for occurrence in listing.occurrences() {
            let Some(listed) = listing.definitions().get(occurrence.definition) else {
                continue;
            };
            if listed.kind != DefinitionKind::Part {
                continue;
            }
            let path = occurrence.path.instances();
            let name = occurrence.path.display(listing.path_form());
            let part = match &listed.read {
                Some(ComponentRead::Accepted(part)) if !part.solids.is_empty() => part,
                _ => {
                    scene.unread.push(UnreadLeaf {
                        path,
                        name,
                        definition: listed.definition.clone(),
                    });
                    continue;
                }
            };
            let index = *parts.entry(occurrence.definition).or_insert_with(|| {
                scene.parts.push((**part).clone());
                scene.parts.len() - 1
            });
            let landmarks = landmarks(part)
                .iter()
                .map(|l| l.placed(&occurrence.transform))
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| FitSceneError::Mismatch(format!("{name}: a landmark overflows")))?;
            scene.bodies.push(SceneBody {
                selected: selected.contains_key(path.as_slice()),
                path,
                name,
                definition: listed.definition.clone(),
                solid: part.solids.first().map_or(0, |s| s.instance),
                faces: faces_of(part),
                transform: occurrence.transform,
                part_uncertainty_mm: occurrence.part_uncertainty_mm,
                vertex_offset_mm: occurrence.vertex_offset_mm,
                placement_uncertainty_mm: occurrence.placement_uncertainty_mm,
                landmarks,
                extent: part.extent,
                part: index,
            });
        }
        scene.check_selected(&selected)?;
        Ok(scene)
    }

    /// Every selected occurrence is a body of the scene at the transform
    /// the socket states.
    fn check_selected(&self, selected: &BTreeMap<&[u64], &Rigid>) -> Result<(), FitSceneError> {
        let bodies: BTreeSet<&[u64]> = self
            .bodies
            .iter()
            .filter(|b| b.selected)
            .map(|b| b.path.as_slice())
            .collect();
        for (path, transform) in selected {
            match self.body(path) {
                Some(body) if body.transform == **transform && bodies.contains(path) => {}
                Some(_) => {
                    return Err(FitSceneError::Mismatch(format!(
                        "occurrence {path:?} is placed elsewhere in the assembly"
                    )));
                }
                None => {
                    return Err(FitSceneError::Mismatch(format!(
                        "occurrence {path:?} is not a part of the assembly fitment reads"
                    )));
                }
            }
        }
        Ok(())
    }

    /// The SHA-256 of what the `scene` request names: the assembly's
    /// SHA-256 and every body's path, faces, transform and landmarks. The
    /// helper echoes it in its reply, and [`super::decide`] requires it
    /// (FH2, beside the protocol's nonce).
    pub fn request_digest(&self) -> String {
        let mut d = super::plan::Digest::new("scene");
        d.text(&self.sha256);
        d.count(self.bodies.len() as u64);
        for body in &self.bodies {
            d.ids(&body.path);
            d.ids(&body.faces);
            d.rigid(&body.transform);
            d.landmarks(&body.landmarks);
        }
        d.finish()
    }

    /// The assembly's SHA-256: the socket's `source`.
    pub fn sha256(&self) -> &str {
        &self.sha256
    }

    /// Every body, by path.
    pub fn bodies(&self) -> &[SceneBody] {
        &self.bodies
    }

    /// The body at `path`, if one.
    pub fn body(&self, path: &[u64]) -> Option<&SceneBody> {
        self.bodies.iter().find(|b| b.path == path)
    }

    /// The leaves whose part fitment refuses, by path.
    pub fn unread(&self) -> &[UnreadLeaf] {
        &self.unread
    }

    /// The part `body` is an occurrence of, in its own frame.
    pub fn part(&self, body: &SceneBody) -> Option<&Part> {
        self.parts.get(body.part)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const S01: &[u8] = include_bytes!("../../tests/assemblies/s01_bolted.stp");
    const S08X: &[u8] = include_bytes!("../../tests/assemblies/s08x_nested.stp");

    /// FB1: every leaf of s01 is a body, the bracket selected, each with
    /// its solid's faces, its placement and the parts' stated floors.
    #[test]
    fn every_leaf_is_a_body_with_its_faces_and_floors() {
        let socket = SocketDocument::extract(S01, Some("BRACKET"), None).unwrap();
        let scene = FitScene::of(S01, &socket).unwrap();
        assert_eq!(scene.bodies().len(), 10, "plate, bracket, 4 bolts, 4 nuts");
        assert_eq!(scene.unread(), []);
        let selected: Vec<&SceneBody> = scene.bodies().iter().filter(|b| b.selected).collect();
        assert_eq!(selected.len(), 1);
        for body in scene.bodies() {
            let part = scene.part(body).unwrap();
            assert_eq!(body.faces, faces_of(part));
            assert_ne!(body.faces.len(), 0);
            assert_eq!(body.solid, part.solids[0].instance);
            assert!(
                (body.part_uncertainty_mm - 1e-7).abs() < 1e-12,
                "{}",
                body.name
            );
            assert_eq!(fixes(&body.landmarks), (3, 3), "{}", body.name);
            assert!(scene.body(&body.path).is_some());
        }
        assert_eq!(scene.sha256(), socket.source().sha256);
    }

    /// FB1: bytes that are not the socket's source are no scene, nor is a
    /// socket document that is refused.
    #[test]
    fn another_assembly_is_not_the_source() {
        let socket = SocketDocument::extract(S01, Some("BRACKET"), None).unwrap();
        assert!(matches!(
            FitScene::of(S08X, &socket),
            Err(FitSceneError::NotTheSource { .. })
        ));
        let refused = SocketDocument::extract(b"not a step file", None, None).unwrap();
        assert_eq!(
            FitScene::of(b"x", &refused),
            Err(FitSceneError::NotAccepted)
        );
    }

    /// FB1: one definition used on two paths is two bodies, each at its
    /// own transform, sharing one part.
    #[test]
    fn a_definition_used_twice_is_two_bodies() {
        let socket = SocketDocument::extract(S08X, Some("BRACKET"), None).unwrap();
        let scene = FitScene::of(S08X, &socket).unwrap();
        let brackets: Vec<&SceneBody> = scene.bodies().iter().filter(|b| b.selected).collect();
        assert_eq!(brackets.len(), 2);
        assert_ne!(brackets[0].transform, brackets[1].transform);
        assert_eq!(brackets[0].faces, brackets[1].faces);
        assert_ne!(brackets[0].landmarks, brackets[1].landmarks);
        assert_eq!(scene.part(brackets[0]), scene.part(brackets[1]));
    }

    /// FB3: the bracket's landmarks are its two largest planes, then its
    /// largest full cylinders, and a plane placed by a transform keeps its
    /// normal's offset through the shift.
    #[test]
    fn landmarks_are_the_largest_planes_then_cylinders() {
        let socket = SocketDocument::extract(S01, Some("BRACKET"), None).unwrap();
        let scene = FitScene::of(S01, &socket).unwrap();
        let bracket = scene.bodies().iter().find(|b| b.selected).unwrap();
        let part = scene.part(bracket).unwrap();
        let own = landmarks(part);
        assert!(own.len() > 4, "{own:?}");
        assert_eq!(fixes(&own), (3, 3));
        assert_eq!(fixes(&own[..4]), (1, 3), "the planes and holes all on z");
        assert!(matches!(own[0].value, LandmarkValue::Plane { .. }));
        assert!(matches!(own[1].value, LandmarkValue::Plane { .. }));
        assert!(matches!(own[2].value, LandmarkValue::Cylinder { .. }));
        assert!(matches!(own[3].value, LandmarkValue::Cylinder { .. }));
        // The bracket is 80 x 50 x 8: its two largest planes are its top
        // and bottom, 8 mm apart.
        let offsets: Vec<(f64, [f64; 3])> = own[..2]
            .iter()
            .map(|l| match l.value {
                LandmarkValue::Plane { normal, offset_mm } => (offset_mm, normal.to_array()),
                LandmarkValue::Cylinder { .. } => unreachable!(),
            })
            .collect();
        assert!((offsets[0].1[2].abs() - 1.0).abs() < 1e-12);
        assert!(((offsets[0].0 + offsets[1].0).abs() - 8.0).abs() < 1e-9);
        let shift = Rigid::new(Rigid::IDENTITY.rotation(), [0.0, 0.0, 5.0]).unwrap();
        let LandmarkValue::Plane { normal, offset_mm } = own[0].placed(&shift).unwrap().value
        else {
            unreachable!()
        };
        assert_eq!(
            normal,
            own[0]
                .placed(&Rigid::IDENTITY)
                .map(|l| match l.value {
                    LandmarkValue::Plane { normal, .. } => normal,
                    LandmarkValue::Cylinder { .. } => unreachable!(),
                })
                .unwrap()
        );
        assert!((offset_mm - (offsets[0].0 + 5.0 * normal.to_array()[2])).abs() < 1e-12);
    }
}
