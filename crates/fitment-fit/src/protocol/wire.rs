//! The bodies of FH4's requests and replies as they cross the channel,
//! and their conversion from and to fitment-core's types. A reply is
//! decoded strictly (FH5): every key known and present, `null` written
//! where a value is absent, every list in the request's order.

use fitment_core::fit::{
    BodyMeasure, FitBox, LandmarkMeasure, PairMeasure, PlacementMeasure, ReachClaim, SolidMeasure,
    ZoneMeasure,
};
use fitment_core::fit::{Landmark, LandmarkValue, ZoneShape};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Three rows of `[R | t]`.
pub(crate) type Rows = [[f64; 4]; 3];

/// A required key whose value may be `null` (FH5: a key left out is a
/// protocol error, not an absent value).
pub(crate) fn nullable<'de, D, T>(d: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(d)
}

/// A number a request sends: never NaN or infinite (JSON has neither,
/// and `serde_json` would write `null`).
#[allow(clippy::trivially_copy_pass_by_ref)] // serde's `serialize_with` passes a reference
pub(crate) fn finite<S: Serializer>(x: &f64, s: S) -> Result<S::Ok, S::Error> {
    if x.is_finite() {
        s.serialize_f64(*x)
    } else {
        Err(serde::ser::Error::custom("a number that is not finite"))
    }
}

/// Every number of `xs` finite, as [`finite`].
pub(crate) fn finite3<S: Serializer>(xs: &[f64; 3], s: S) -> Result<S::Ok, S::Error> {
    if xs.iter().all(|x| x.is_finite()) {
        xs.serialize(s)
    } else {
        Err(serde::ser::Error::custom("a number that is not finite"))
    }
}

/// Every number of `rows` finite, as [`finite`].
pub(crate) fn finite_rows<S: Serializer>(rows: &Rows, s: S) -> Result<S::Ok, S::Error> {
    if rows.iter().flatten().all(|x| x.is_finite()) {
        rows.serialize(s)
    } else {
        Err(serde::ser::Error::custom("a number that is not finite"))
    }
}

/// An axis-aligned box.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WireBox {
    pub min_mm: [f64; 3],
    pub max_mm: [f64; 3],
}

impl WireBox {
    /// A measure's box: `None` for one with a number that is not finite,
    /// which no reader can give (FT8: within reach of everything).
    pub fn of(b: &FitBox) -> Option<Self> {
        b.min_mm
            .iter()
            .chain(&b.max_mm)
            .all(|x| x.is_finite())
            .then_some(Self {
                min_mm: b.min_mm,
                max_mm: b.max_mm,
            })
    }

    /// The box as a measure; `None` as the box no reader can give.
    pub fn measure(b: Option<Self>) -> FitBox {
        b.map_or(
            FitBox {
                min_mm: [f64::NAN; 3],
                max_mm: [f64::NAN; 3],
            },
            |b| FitBox {
                min_mm: b.min_mm.map(zero),
                max_mm: b.max_mm.map(zero),
            },
        )
    }
}

/// FH5: no −0.0 kept.
pub(crate) fn zero(x: f64) -> f64 {
    if x == 0.0 { 0.0 } else { x }
}

/// A landmark's value, in a request or a reply.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum WireLandmark {
    /// A plane: its outward unit normal and offset.
    Plane {
        #[serde(serialize_with = "finite3")]
        normal: [f64; 3],
        #[serde(serialize_with = "finite")]
        offset_mm: f64,
    },
    /// A full cylinder: a point of its axis, its direction, its radius.
    Cylinder {
        #[serde(serialize_with = "finite3")]
        point_mm: [f64; 3],
        #[serde(serialize_with = "finite3")]
        direction: [f64; 3],
        #[serde(serialize_with = "finite")]
        radius_mm: f64,
    },
}

impl WireLandmark {
    /// fitment's value of a landmark; `None` for a kind this protocol
    /// cannot name.
    pub fn of(value: &LandmarkValue) -> Option<Self> {
        match value {
            LandmarkValue::Plane {
                normal, offset_mm, ..
            } => Some(Self::Plane {
                normal: normal.to_array(),
                offset_mm: *offset_mm,
            }),
            LandmarkValue::Cylinder {
                axis, radius_mm, ..
            } => Some(Self::Cylinder {
                point_mm: axis.point_mm.to_array(),
                direction: axis.direction.to_array(),
                radius_mm: *radius_mm,
            }),
            _ => None,
        }
    }

    /// The kernel's reading of a landmark.
    pub fn of_measure(m: &LandmarkMeasure) -> Option<Self> {
        match m {
            LandmarkMeasure::Plane {
                normal, offset_mm, ..
            } => Some(Self::Plane {
                normal: *normal,
                offset_mm: *offset_mm,
            }),
            LandmarkMeasure::Cylinder {
                point_mm,
                direction,
                radius_mm,
                ..
            } => Some(Self::Cylinder {
                point_mm: *point_mm,
                direction: *direction,
                radius_mm: *radius_mm,
            }),
            _ => None,
        }
    }

    /// As a measure; `None` is a face not found (`Missing`).
    pub fn measure(m: Option<Self>) -> LandmarkMeasure {
        match m {
            None => LandmarkMeasure::default(),
            Some(Self::Plane { normal, offset_mm }) => {
                LandmarkMeasure::plane(normal.map(zero), zero(offset_mm))
            }
            Some(Self::Cylinder {
                point_mm,
                direction,
                radius_mm,
            }) => {
                LandmarkMeasure::cylinder(point_mm.map(zero), direction.map(zero), zero(radius_mm))
            }
        }
    }
}

/// A landmark face a request names: its `#id` and fitment's value.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WireFace {
    pub face: u64,
    pub value: WireLandmark,
}

impl WireFace {
    /// `landmarks` as a request names them; `None` if one is of a kind
    /// this protocol cannot name.
    pub fn all(landmarks: &[Landmark]) -> Option<Vec<Self>> {
        landmarks
            .iter()
            .map(|l| {
                WireLandmark::of(&l.value).map(|value| Self {
                    face: l.face,
                    value,
                })
            })
            .collect()
    }
}

/// A zone's description, as the helper builds it (FZ2 to FZ4, FZ7).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum WireShape {
    /// FZ2: the plane face `face` of the zone's body, swept either side.
    Slab {
        face: u64,
        #[serde(serialize_with = "finite3")]
        normal: [f64; 3],
        #[serde(serialize_with = "finite")]
        offset_mm: f64,
    },
    /// FZ3: the annulus about an axis between two stations and two radii.
    Annulus {
        #[serde(serialize_with = "finite3")]
        point_mm: [f64; 3],
        #[serde(serialize_with = "finite3")]
        direction: [f64; 3],
        #[serde(serialize_with = "finite")]
        from_mm: f64,
        #[serde(serialize_with = "finite")]
        to_mm: f64,
        #[serde(serialize_with = "finite")]
        inner_mm: f64,
        #[serde(serialize_with = "finite")]
        outer_mm: f64,
    },
    /// FZ4: the shell about a cone over a radial range.
    Shell {
        #[serde(serialize_with = "finite3")]
        apex_mm: [f64; 3],
        #[serde(serialize_with = "finite3")]
        direction: [f64; 3],
        #[serde(serialize_with = "finite")]
        half_angle_rad: f64,
        radii_mm: [f64; 2],
    },
}

impl WireShape {
    /// A zone's shape; `None` for one with no closed form, or of a kind
    /// this protocol cannot name.
    pub fn of(shape: &ZoneShape) -> Option<Self> {
        match shape {
            ZoneShape::Slab {
                face,
                normal,
                offset_mm,
                ..
            } => Some(Self::Slab {
                face: *face,
                normal: normal.to_array(),
                offset_mm: *offset_mm,
            }),
            ZoneShape::Annulus {
                axis,
                from_mm,
                to_mm,
                inner_mm,
                outer_mm,
                ..
            } => Some(Self::Annulus {
                point_mm: axis.point_mm.to_array(),
                direction: axis.direction.to_array(),
                from_mm: *from_mm,
                to_mm: *to_mm,
                inner_mm: *inner_mm,
                outer_mm: *outer_mm,
            }),
            ZoneShape::Shell {
                apex_mm,
                direction,
                half_angle_rad,
                radii_mm,
                ..
            } => radii_mm
                .iter()
                .all(|r| r.is_finite())
                .then_some(Self::Shell {
                    apex_mm: apex_mm.to_array(),
                    direction: direction.to_array(),
                    half_angle_rad: *half_angle_rad,
                    radii_mm: *radii_mm,
                }),
            _ => None,
        }
    }
}

/// The `hello` reply's report of one resource limit (FY3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WireLimit {
    pub name: String,
    pub took: bool,
    #[serde(deserialize_with = "nullable")]
    pub value: Option<u64>,
}

/// The `hello` reply (FH3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct HelloReply {
    pub protocol: u32,
    pub pid: u32,
    pub helper_sha256: String,
    pub lock_sha256: String,
    pub python: String,
    pub binding: String,
    pub kernel: String,
    pub site: String,
    pub platform: String,
    pub limits: Vec<WireLimit>,
    /// Why the helper cannot measure (an `ImportError` naming a missing
    /// shared library), or `null`.
    #[serde(deserialize_with = "nullable")]
    pub unavailable: Option<String>,
}

/// One body of the `scene` reply (FB2, FB3, FT6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WireBody {
    pub key: Vec<u64>,
    pub found: bool,
    pub valid: bool,
    pub volume_mm3: f64,
    pub tolerance_mm: f64,
    #[serde(rename = "box", deserialize_with = "nullable")]
    pub bounds: Option<WireBox>,
    pub landmarks: Vec<Option<WireLandmark>>,
}

impl WireBody {
    pub fn of(m: &BodyMeasure) -> Self {
        Self {
            key: m.path.clone(),
            found: m.found,
            valid: m.valid,
            volume_mm3: m.volume_mm3,
            tolerance_mm: m.tolerance_mm,
            bounds: WireBox::of(&m.bounds),
            landmarks: m.landmarks.iter().map(WireLandmark::of_measure).collect(),
        }
    }

    pub fn measure(self) -> BodyMeasure {
        let mut m = BodyMeasure::default();
        m.path = self.key;
        m.found = self.found;
        m.valid = self.valid;
        m.volume_mm3 = zero(self.volume_mm3);
        m.tolerance_mm = zero(self.tolerance_mm);
        m.bounds = WireBox::measure(self.bounds);
        m.landmarks = self
            .landmarks
            .into_iter()
            .map(WireLandmark::measure)
            .collect();
        m
    }
}

/// One solid the reader transferred (FB6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WireSolid {
    pub faces: Vec<u64>,
    pub location: Rows,
    #[serde(rename = "box", deserialize_with = "nullable")]
    pub bounds: Option<WireBox>,
}

impl WireSolid {
    pub fn of(m: &SolidMeasure) -> Self {
        Self {
            faces: m.faces.clone(),
            location: m.location,
            bounds: WireBox::of(&m.bounds),
        }
    }

    pub fn measure(self) -> SolidMeasure {
        let mut m = SolidMeasure::default();
        m.faces = self.faces;
        m.location = self.location.map(|row| row.map(zero));
        m.bounds = WireBox::measure(self.bounds);
        m
    }
}

/// The `scene` reply (FH4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SceneReply {
    pub bodies: Vec<WireBody>,
    pub solids: Vec<WireSolid>,
}

/// The candidate at one placement (FB3, FT8).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WirePlacement {
    pub occurrence: Vec<u64>,
    #[serde(rename = "box", deserialize_with = "nullable")]
    pub bounds: Option<WireBox>,
    pub landmarks: Vec<Option<WireLandmark>>,
}

impl WirePlacement {
    #[cfg(any(test, feature = "test-support"))]
    pub fn of(m: &PlacementMeasure) -> Self {
        Self {
            occurrence: m.occurrence.clone(),
            bounds: WireBox::of(&m.bounds),
            landmarks: m.landmarks.iter().map(WireLandmark::of_measure).collect(),
        }
    }

    pub fn measure(self) -> PlacementMeasure {
        let mut m = PlacementMeasure::default();
        m.occurrence = self.occurrence;
        m.bounds = WireBox::measure(self.bounds);
        m.landmarks = self
            .landmarks
            .into_iter()
            .map(WireLandmark::measure)
            .collect();
        m
    }
}

/// The `place` reply (FH4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PlaceReply {
    pub valid: bool,
    pub volume_mm3: f64,
    pub tolerance_mm: f64,
    #[serde(deserialize_with = "nullable")]
    pub sha256: Option<String>,
    pub placements: Vec<WirePlacement>,
}

/// A zone as the kernel built it (FZ7).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WireZoneMeasure {
    pub volume_mm3: f64,
    pub centroid_mm: [f64; 3],
    #[serde(rename = "box", deserialize_with = "nullable")]
    pub bounds: Option<WireBox>,
}

impl WireZoneMeasure {
    #[cfg(any(test, feature = "test-support"))]
    pub fn of(m: &ZoneMeasure) -> Self {
        Self {
            volume_mm3: m.volume_mm3,
            centroid_mm: m.centroid_mm,
            bounds: WireBox::of(&m.bounds),
        }
    }

    pub fn measure(self) -> ZoneMeasure {
        ZoneMeasure::new(
            zero(self.volume_mm3),
            self.centroid_mm.map(zero),
            WireBox::measure(self.bounds),
        )
    }
}

/// One pair of the `check` reply (FT4, FT5, FY4), named by its keys.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WirePair {
    pub occurrence: Vec<u64>,
    pub body: Vec<u64>,
    pub copy: bool,
    pub common_mm3: f64,
    pub residual_mm3: f64,
    pub inside_mm3: f64,
    #[serde(deserialize_with = "nullable")]
    pub residual_box: Option<WireBox>,
    #[serde(deserialize_with = "nullable")]
    pub distance_mm: Option<f64>,
    #[serde(deserialize_with = "nullable")]
    pub zoned_distance_mm: Option<f64>,
    #[serde(deserialize_with = "nullable")]
    pub body_kept_mm3: Option<f64>,
    #[serde(deserialize_with = "nullable")]
    pub body_inside_mm3: Option<f64>,
    pub non_analytic: bool,
    pub alerts: Vec<String>,
    pub zones: Vec<WireZoneMeasure>,
    pub proximity_zones: Vec<WireZoneMeasure>,
}

impl WirePair {
    #[cfg(any(test, feature = "test-support"))]
    /// `m` as the reply to the pair named by `key`.
    pub fn of(key: (&[u64], &[u64], bool), m: &PairMeasure) -> Self {
        Self {
            occurrence: key.0.to_vec(),
            body: key.1.to_vec(),
            copy: key.2,
            common_mm3: m.common_mm3,
            residual_mm3: m.residual_mm3,
            inside_mm3: m.inside_mm3,
            residual_box: m.residual_box.as_ref().and_then(WireBox::of),
            distance_mm: m.distance_mm,
            zoned_distance_mm: m.zoned_distance_mm,
            body_kept_mm3: m.body_kept_mm3,
            body_inside_mm3: m.body_inside_mm3,
            non_analytic: m.non_analytic,
            alerts: m.alerts.clone(),
            zones: m.zones.iter().map(WireZoneMeasure::of).collect(),
            proximity_zones: m.proximity_zones.iter().map(WireZoneMeasure::of).collect(),
        }
    }

    pub fn measure(self) -> PairMeasure {
        let mut m = PairMeasure::default();
        m.common_mm3 = zero(self.common_mm3);
        m.residual_mm3 = zero(self.residual_mm3);
        m.inside_mm3 = zero(self.inside_mm3);
        m.residual_box = self.residual_box.map(|b| WireBox::measure(Some(b)));
        m.distance_mm = self.distance_mm.map(zero);
        m.zoned_distance_mm = self.zoned_distance_mm.map(zero);
        m.body_kept_mm3 = self.body_kept_mm3.map(zero);
        m.body_inside_mm3 = self.body_inside_mm3.map(zero);
        m.non_analytic = self.non_analytic;
        m.alerts = self.alerts;
        m.zones = self
            .zones
            .into_iter()
            .map(WireZoneMeasure::measure)
            .collect();
        m.proximity_zones = self
            .proximity_zones
            .into_iter()
            .map(WireZoneMeasure::measure)
            .collect();
        m
    }
}

/// The helper's claim of reach for one body at one placement (FT8).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WireReach {
    pub body: Vec<u64>,
    pub occurrence: Vec<u64>,
    pub within: bool,
}

impl WireReach {
    #[cfg(any(test, feature = "test-support"))]
    pub fn of(m: &ReachClaim) -> Self {
        Self {
            body: m.body.clone(),
            occurrence: m.occurrence.clone(),
            within: m.within,
        }
    }

    pub fn measure(self) -> ReachClaim {
        ReachClaim::new(self.body, self.occurrence, self.within)
    }
}

/// The `check` reply (FH4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CheckReply {
    pub pairs: Vec<WirePair>,
    pub reach: Vec<WireReach>,
}

/// An `error` frame's body (FH5): the helper could not parse a request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ErrorBody {
    pub code: String,
    pub sentence: String,
}
