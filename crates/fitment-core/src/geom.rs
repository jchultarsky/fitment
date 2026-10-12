//! Points, vectors, directions and rigid transforms in millimetres.
//!
//! Every value is finite and never `-0.0`, so that it serializes the same
//! way on every platform: `serde_json` would write a non-finite value as
//! `null` without an error, and Open CASCADE writes `-0.0` whenever it
//! negates a direction. Constructors return `None` for a non-finite value
//! rather than panic.

use serde::{Deserialize, Serialize};

/// `x` unless it is not finite, with `-0.0` turned into `0.0`.
///
/// Adding `0.0` changes no other value: `-0.0 + 0.0` is `+0.0` under the
/// default rounding, and `x + 0.0` is `x` for every other finite `x`.
pub(crate) fn finite(x: f64) -> Option<f64> {
    x.is_finite().then_some(x + 0.0)
}

fn finite3(xyz: [f64; 3]) -> Option<[f64; 3]> {
    Some([finite(xyz[0])?, finite(xyz[1])?, finite(xyz[2])?])
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// A point, in millimetres. Serializes as `[x, y, z]`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "[f64; 3]", into = "[f64; 3]")]
// The field is private; `new` is the only way in.
#[allow(clippy::exhaustive_structs)]
pub struct Point3([f64; 3]);

impl Point3 {
    /// The origin.
    pub const ORIGIN: Self = Self([0.0; 3]);

    /// A point, or `None` if a coordinate is not finite.
    pub fn new(xyz: [f64; 3]) -> Option<Self> {
        finite3(xyz).map(Self)
    }

    /// The coordinates.
    pub fn to_array(self) -> [f64; 3] {
        self.0
    }

    /// The vector from `origin` to this point, or `None` if it overflows.
    pub fn minus(self, origin: Self) -> Option<Vec3> {
        Vec3::new([
            self.0[0] - origin.0[0],
            self.0[1] - origin.0[1],
            self.0[2] - origin.0[2],
        ])
    }
}

/// A vector, in millimetres. Serializes as `[x, y, z]`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "[f64; 3]", into = "[f64; 3]")]
#[allow(clippy::exhaustive_structs)]
pub struct Vec3([f64; 3]);

impl Vec3 {
    /// A vector, or `None` if a component is not finite.
    pub fn new(xyz: [f64; 3]) -> Option<Self> {
        finite3(xyz).map(Self)
    }

    /// The components.
    pub fn to_array(self) -> [f64; 3] {
        self.0
    }

    /// The dot product, or `None` if it overflows.
    pub fn dot(self, other: Self) -> Option<f64> {
        finite(dot(self.0, other.0))
    }

    /// The length, or `None` if it overflows.
    pub fn norm(self) -> Option<f64> {
        finite(dot(self.0, self.0).sqrt())
    }
}

/// A unit direction. Serializes as `[x, y, z]`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "[f64; 3]", into = "[f64; 3]")]
#[allow(clippy::exhaustive_structs)]
pub struct Dir3([f64; 3]);

impl Dir3 {
    /// How far from 1 a stated direction's length may be for its bits to
    /// be kept as they are.
    pub const UNIT_SLACK: f64 = 1e-12;

    /// A direction that is already unit length, kept bit for bit (apart
    /// from `-0.0`), or `None` if it is not finite or its length is more
    /// than [`UNIT_SLACK`](Self::UNIT_SLACK) from 1.
    pub fn from_unit(xyz: [f64; 3]) -> Option<Self> {
        let xyz = finite3(xyz)?;
        let length = dot(xyz, xyz).sqrt();
        ((length - 1.0).abs() <= Self::UNIT_SLACK).then_some(Self(xyz))
    }

    /// `v` divided by its length, or `None` if `v` is zero or not finite.
    pub fn normalize(v: [f64; 3]) -> Option<Self> {
        let v = finite3(v)?;
        let length = finite(dot(v, v).sqrt())?;
        if length == 0.0 {
            return None;
        }
        finite3([v[0] / length, v[1] / length, v[2] / length]).map(Self)
    }

    /// The components.
    pub fn to_array(self) -> [f64; 3] {
        self.0
    }

    /// The opposite direction.
    #[must_use]
    pub fn reversed(self) -> Self {
        Self([-self.0[0] + 0.0, -self.0[1] + 0.0, -self.0[2] + 0.0])
    }
}

macro_rules! array_conversions {
    ($($ty:ident),*) => {$(
        impl TryFrom<[f64; 3]> for $ty {
            type Error = String;
            fn try_from(xyz: [f64; 3]) -> Result<Self, String> {
                $ty::new_checked(xyz)
            }
        }
        impl From<$ty> for [f64; 3] {
            fn from(value: $ty) -> Self {
                value.0
            }
        }
    )*};
}

array_conversions!(Point3, Vec3, Dir3);

impl Point3 {
    fn new_checked(xyz: [f64; 3]) -> Result<Self, String> {
        Self::new(xyz).ok_or_else(|| format!("point {xyz:?} is not finite"))
    }
}

impl Vec3 {
    fn new_checked(xyz: [f64; 3]) -> Result<Self, String> {
        Self::new(xyz).ok_or_else(|| format!("vector {xyz:?} is not finite"))
    }
}

impl Dir3 {
    fn new_checked(xyz: [f64; 3]) -> Result<Self, String> {
        Self::from_unit(xyz).ok_or_else(|| format!("direction {xyz:?} is not a finite unit vector"))
    }
}

/// A rigid transform from a child frame to its parent, in millimetres:
/// `p_parent = R · p_child + t`, with the rotation row-major. This is
/// stepq's convention, so a transform read from stepq is copied, not
/// recomputed.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "RawRigid", into = "RawRigid")]
// The fields are private; `new` checks them.
#[allow(clippy::exhaustive_structs)]
pub struct Rigid {
    rotation: [[f64; 3]; 3],
    translation: [f64; 3],
}

impl Rigid {
    /// How far `R · Rᵀ` may be from the identity, entry by entry, for a
    /// rotation to be accepted.
    pub const ORTHONORMAL_SLACK: f64 = 1e-9;

    /// The transform that moves nothing.
    pub const IDENTITY: Self = Self {
        rotation: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        translation: [0.0; 3],
    };

    /// A transform, or `None` if a value is not finite, or the rotation is
    /// not orthonormal with determinant +1 within
    /// [`ORTHONORMAL_SLACK`](Self::ORTHONORMAL_SLACK). Both `R·Rᵀ` and
    /// `Rᵀ·R` are checked, so a skewed matrix that passes one fails the
    /// other.
    pub fn new(rotation: [[f64; 3]; 3], translation: [f64; 3]) -> Option<Self> {
        let rotation = [
            finite3(rotation[0])?,
            finite3(rotation[1])?,
            finite3(rotation[2])?,
        ];
        let translation = finite3(translation)?;
        let columns = [
            [rotation[0][0], rotation[1][0], rotation[2][0]],
            [rotation[0][1], rotation[1][1], rotation[2][1]],
            [rotation[0][2], rotation[1][2], rotation[2][2]],
        ];
        for vectors in [&rotation, &columns] {
            for (i, a) in vectors.iter().enumerate() {
                for (j, b) in vectors.iter().enumerate() {
                    let expected = if i == j { 1.0 } else { 0.0 };
                    if (dot(*a, *b) - expected).abs() > Self::ORTHONORMAL_SLACK {
                        return None;
                    }
                }
            }
        }
        let [a, b, c] = rotation;
        let determinant = dot(
            a,
            [
                b[1] * c[2] - b[2] * c[1],
                b[2] * c[0] - b[0] * c[2],
                b[0] * c[1] - b[1] * c[0],
            ],
        );
        if (determinant - 1.0).abs() > Self::ORTHONORMAL_SLACK {
            return None;
        }
        Some(Self {
            rotation,
            translation,
        })
    }

    /// The rotation, `rotation[row][col]`.
    pub fn rotation(&self) -> [[f64; 3]; 3] {
        self.rotation
    }

    /// The translation, in millimetres.
    pub fn translation(&self) -> [f64; 3] {
        self.translation
    }

    /// True for the identity, bit for bit.
    pub fn is_identity(&self) -> bool {
        *self == Self::IDENTITY
    }

    /// A point of the child frame in the parent frame, or `None` if it
    /// overflows.
    pub fn apply_point(&self, p: Point3) -> Option<Point3> {
        let rotated = self.rotation.map(|row| dot(row, p.0));
        Point3::new([
            rotated[0] + self.translation[0],
            rotated[1] + self.translation[1],
            rotated[2] + self.translation[2],
        ])
    }

    /// A direction of the child frame in the parent frame, renormalized,
    /// or `None` if it overflows.
    pub fn apply_dir(&self, d: Dir3) -> Option<Dir3> {
        Dir3::normalize(self.rotation.map(|row| dot(row, d.0)))
    }

    /// This transform, then `outer`: a child of this frame placed in
    /// `outer`'s parent. `None` if a value overflows.
    pub fn then(&self, outer: &Self) -> Option<Self> {
        let r = &outer.rotation;
        let mut rotation = [[0.0; 3]; 3];
        for (i, row) in rotation.iter_mut().enumerate() {
            for (j, value) in row.iter_mut().enumerate() {
                *value = r[i][0] * self.rotation[0][j]
                    + r[i][1] * self.rotation[1][j]
                    + r[i][2] * self.rotation[2][j];
            }
        }
        let moved = outer.rotation.map(|row| dot(row, self.translation));
        let translation = [
            moved[0] + outer.translation[0],
            moved[1] + outer.translation[1],
            moved[2] + outer.translation[2],
        ];
        Self::finite(rotation, translation)
    }

    /// The transform that undoes this one, `(Rᵀ, −Rᵀ·t)`, or `None` if it
    /// overflows.
    pub fn inverse(&self) -> Option<Self> {
        let r = &self.rotation;
        let transposed = [
            [r[0][0], r[1][0], r[2][0]],
            [r[0][1], r[1][1], r[2][1]],
            [r[0][2], r[1][2], r[2][2]],
        ];
        let moved = transposed.map(|row| dot(row, self.translation));
        Self::finite(transposed, [-moved[0], -moved[1], -moved[2]])
    }

    /// The result of composing or inverting transforms [`new`](Self::new)
    /// accepted: only finiteness is checked again, so that rounding can
    /// never make a valid composition fail the orthonormality slack.
    fn finite(rotation: [[f64; 3]; 3], translation: [f64; 3]) -> Option<Self> {
        Some(Self {
            rotation: [
                finite3(rotation[0])?,
                finite3(rotation[1])?,
                finite3(rotation[2])?,
            ],
            translation: finite3(translation)?,
        })
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRigid {
    rotation: [[f64; 3]; 3],
    translation_mm: [f64; 3],
}

impl TryFrom<RawRigid> for Rigid {
    type Error = String;
    fn try_from(raw: RawRigid) -> Result<Self, String> {
        Self::new(raw.rotation, raw.translation_mm)
            .ok_or_else(|| "a rigid transform must be finite with a proper rotation".to_owned())
    }
}

impl From<Rigid> for RawRigid {
    fn from(rigid: Rigid) -> Self {
        Self {
            rotation: rigid.rotation,
            translation_mm: rigid.translation,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finite_removes_negative_zero_and_nothing_else() {
        assert!(finite(-0.0).unwrap().is_sign_positive());
        assert_eq!(finite(-1.5), Some(-1.5));
        assert_eq!(finite(f64::MIN_POSITIVE), Some(f64::MIN_POSITIVE));
        assert_eq!(finite(f64::NAN), None);
        assert_eq!(finite(f64::INFINITY), None);
    }

    #[test]
    fn values_are_finite_and_never_negative_zero() {
        let p = Point3::new([-0.0, 1.0, 2.0]).unwrap();
        assert!(p.to_array()[0].is_sign_positive());
        assert_eq!(Point3::new([f64::NAN, 0.0, 0.0]), None);
        let d = Dir3::from_unit([0.0, 0.0, 1.0]).unwrap().reversed();
        assert_eq!(d.to_array(), [0.0, 0.0, -1.0]);
        assert!(d.to_array()[0].is_sign_positive());
    }

    #[test]
    fn a_stated_unit_direction_keeps_its_bits() {
        let stated = [0.6, 0.8, 0.0];
        assert_eq!(Dir3::from_unit(stated).unwrap().to_array(), stated);
        assert_eq!(Dir3::from_unit([0.6, 0.8, 0.1]), None);
        assert_eq!(Dir3::normalize([0.0, 0.0, 0.0]), None);
        assert_eq!(
            Dir3::normalize([0.0, 3.0, 4.0]).unwrap().to_array(),
            [0.0, 0.6, 0.8]
        );
    }

    #[test]
    fn rigid_transforms_compose_and_invert() {
        // A quarter turn about z, then a shift.
        let turn = Rigid::new(
            [[0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]],
            [10.0, 0.0, 0.0],
        )
        .unwrap();
        let p = Point3::new([1.0, 0.0, 0.0]).unwrap();
        assert_eq!(turn.apply_point(p).unwrap().to_array(), [10.0, 1.0, 0.0]);
        let back = turn.inverse().unwrap();
        assert_eq!(turn.then(&back).unwrap(), Rigid::IDENTITY);
        let twice = turn.then(&turn).unwrap();
        assert_eq!(twice.apply_point(p).unwrap().to_array(), [9.0, 10.0, 0.0]);
    }

    #[test]
    fn composing_accepted_transforms_never_fails_on_rounding() {
        // Inside the slack, but composing it with itself 40 times would
        // drift past it if each step were checked again.
        let s = 1.0 + 3.2e-10;
        let near = Rigid::new([[s, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]], [1.0; 3]).unwrap();
        let mut composed = near;
        for _ in 0..40 {
            composed = composed.then(&near).unwrap();
        }
        assert!(composed.inverse().is_some());
    }

    #[test]
    fn a_reflection_or_shear_is_not_a_rigid_transform() {
        let mirror = [[-1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        assert_eq!(Rigid::new(mirror, [0.0; 3]), None);
        let shear = [[1.0, 0.1, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        assert_eq!(Rigid::new(shear, [0.0; 3]), None);
    }

    #[test]
    fn values_round_trip_through_json() {
        let rigid = Rigid::new(Rigid::IDENTITY.rotation(), [1.0, -0.0, 3.5]).unwrap();
        let json = serde_json::to_string(&rigid).unwrap();
        assert_eq!(
            json,
            r#"{"rotation":[[1.0,0.0,0.0],[0.0,1.0,0.0],[0.0,0.0,1.0]],"translation_mm":[1.0,0.0,3.5]}"#
        );
        assert_eq!(serde_json::from_str::<Rigid>(&json).unwrap(), rigid);
        assert!(serde_json::from_str::<Dir3>("[0.0, 0.0, 2.0]").is_err());
        assert!(serde_json::from_str::<Point3>("[0.0, null, 2.0]").is_err());
    }
}
