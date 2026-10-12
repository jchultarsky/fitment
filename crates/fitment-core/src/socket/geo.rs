//! Small vector arithmetic for the socket rules, on `[f64; 3]`.

use crate::geom::Rigid;

pub(crate) type V = [f64; 3];

pub(crate) fn add(a: V, b: V) -> V {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

pub(crate) fn sub(a: V, b: V) -> V {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub(crate) fn scale(a: V, k: f64) -> V {
    [a[0] * k, a[1] * k, a[2] * k]
}

pub(crate) fn dot(a: V, b: V) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub(crate) fn cross(a: V, b: V) -> V {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

pub(crate) fn norm(a: V) -> f64 {
    dot(a, a).sqrt()
}

/// `a` without its component along unit `axis`.
pub(crate) fn across(a: V, axis: V) -> V {
    sub(a, scale(axis, dot(a, axis)))
}

/// A rigid motion applied to bare arrays: the frame change from a
/// neighbour's part frame into the target's.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Motion {
    rotation: [[f64; 3]; 3],
    translation: V,
}

impl Motion {
    /// No motion.
    pub(crate) const IDENTITY: Self = Self {
        rotation: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        translation: [0.0; 3],
    };

    /// `inner` then the inverse of `outer`: from a frame `inner` places in
    /// the root, into the frame `outer` places there.
    pub(crate) fn between(inner: &Rigid, outer: &Rigid) -> Option<Self> {
        let moved = inner.then(&outer.inverse()?)?;
        Some(Self {
            rotation: moved.rotation(),
            translation: moved.translation(),
        })
    }

    pub(crate) fn point(&self, p: V) -> V {
        add(self.rotation.map(|row| dot(row, p)), self.translation)
    }

    pub(crate) fn dir(&self, d: V) -> V {
        self.rotation.map(|row| dot(row, d))
    }

    /// The inverse motion, for a rigid one.
    pub(crate) fn inverse(&self) -> Self {
        let r = &self.rotation;
        let transposed = [
            [r[0][0], r[1][0], r[2][0]],
            [r[0][1], r[1][1], r[2][1]],
            [r[0][2], r[1][2], r[2][2]],
        ];
        let moved = transposed.map(|row| dot(row, self.translation));
        Self {
            rotation: transposed,
            translation: [-moved[0], -moved[1], -moved[2]],
        }
    }
}

/// A unit vector perpendicular to unit `a`, and a second completing the
/// right-handed frame.
pub(crate) fn perpendiculars(a: V) -> (V, V) {
    let seed = if a[0].abs() < 0.9 {
        [1.0, 0.0, 0.0]
    } else {
        [0.0, 1.0, 0.0]
    };
    let u = across(seed, a);
    let u = scale(u, 1.0 / norm(u));
    (u, cross(a, u))
}
