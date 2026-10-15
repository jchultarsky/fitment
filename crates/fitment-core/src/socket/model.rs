//! The `fitment.socket` document's types (`docs/research/m3-socket.md`,
//! sections 8.4 and 9.3). Built only by [`super::SocketDocument::extract`]
//! or read back by [`super::SocketDocument::from_json`].

use serde::{Deserialize, Serialize};

use crate::assembly::{AssemblyNote, MatchedBy};
use crate::features::{Axis, DefinitionRef, Extent, FeatureId, Frame};
use crate::geom::Rigid;

/// How a span of a bore or shaft fits what occupies it (B1 to B4, I1, I2,
/// J1, B8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
// The fits the rules can state; a new one is a new rule version.
#[allow(clippy::exhaustive_enums)]
pub enum Fit {
    /// A neighbour passes with room (B1, I1).
    Clearance,
    /// No fastener was read where one may be (J1, J2, B8): pinned.
    NotRead,
    /// Diameters equal within the tolerance (B2): pinned.
    LineToLine,
    /// The neighbour is larger (B3) or runs through the wall (B4): pinned.
    Interference,
}

/// A flag a requirement or relation carries (section 8.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Flag {
    /// The neighbour is another occurrence of the target (N2).
    #[serde(rename = "self")]
    SelfPaired,
    /// A thread or press fit is inferred, never confirmed (B2, B3).
    ThreadNotConfirmed,
    /// The neighbour is off the target's axis by more than the tolerance.
    Offset,
    /// The neighbour runs through the target's wall (B4).
    Conflict,
    /// The relation exists only through a side whose extent is unknown (U3).
    ExtentUnknown,
    /// The target's cylinder is undecided (U2): unverifiable.
    TargetUndecided,
    /// Material that is no read shaft may lie in the hole (B8).
    OccupantUnread,
    /// The target shaft abuts a coaxial neighbour shaft end to end (I6).
    Abutting,
    /// An opening is required where the target's end is unknown (U8):
    /// unverifiable.
    OpenAtUnknownEnd,
    /// A neighbour's partial hole holds the target shaft (I7).
    PartialNeighbour,
    /// A plane contact within the near band, the faces apart (C5).
    NearGap,
    /// A plane contact within the near band, the faces into each other
    /// (C5).
    Interference,
    /// An outline in the contact is unknown (C3, C4).
    UnknownOutline,
    /// A grip one of whose seats is unverifiable (U10): unverifiable.
    SeatUnverifiable,
    /// The original's own material lies in a span's core (U11):
    /// unverifiable.
    CoreNotClear,
}

/// What a span piece lies on (X8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum SpanBasis {
    /// A segment's own cylinder.
    Cylinder,
    /// A transition or end zone a segment stands for (X7): the original
    /// has no single diameter there.
    Zone,
    /// Past a known end of the stack, where an unknown side runs on (U3):
    /// nothing of the original is known there.
    PastKnownEnd,
}

/// One span of a bore: an interval of the hole's stations and what
/// occupies it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct BoreSpan {
    /// Where it starts, along the axis from its point, in mm.
    pub from_mm: f64,
    /// Where it ends.
    pub to_mm: f64,
    /// The original hole's diameter there.
    pub hole_diameter_mm: f64,
    /// A diameter about the axis that what occupies the span is sure to
    /// pass: the least a candidate's hole may have; `None` if no shaft was
    /// read.
    #[serde(deserialize_with = "Option::deserialize")]
    pub clear_diameter_mm: Option<f64>,
    /// The strictest fit of the relations over the span.
    pub fit: Fit,
    /// True if the diameter is pinned to the original's.
    pub pinned: bool,
    /// What the piece lies on (X8).
    pub basis: SpanBasis,
    /// A zone piece's original diameters at its two ends, joined by one
    /// cone (X8); `None` elsewhere, and where no single cone joins them.
    #[serde(deserialize_with = "Option::deserialize")]
    pub profile_mm: Option<[f64; 2]>,
}

/// One span of a shaft requirement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ShaftSpan {
    /// Where it starts, along the axis from its point, in mm.
    pub from_mm: f64,
    /// Where it ends.
    pub to_mm: f64,
    /// The original shaft's diameter there.
    pub shaft_diameter_mm: f64,
    /// The largest diameter the neighbour's hole passes; `None` if pinned.
    #[serde(deserialize_with = "Option::deserialize")]
    pub clear_diameter_mm: Option<f64>,
    /// The strictest fit.
    pub fit: Fit,
    /// True if the diameter is pinned to the original's.
    pub pinned: bool,
    /// What the piece lies on (X8).
    pub basis: SpanBasis,
    /// A zone piece's original diameters at its two ends, joined by one
    /// cone (X8); `None` elsewhere, and where no single cone joins them.
    #[serde(deserialize_with = "Option::deserialize")]
    pub profile_mm: Option<[f64; 2]>,
}

/// True if a piece states nothing of the original a candidate can be
/// held to (U9): past a known end, or in a zone with neither a pin, a
/// clear diameter nor a profile.
fn unknown_piece(
    basis: SpanBasis,
    pinned: bool,
    clear: Option<f64>,
    profile: Option<[f64; 2]>,
) -> bool {
    match basis {
        SpanBasis::PastKnownEnd => true,
        SpanBasis::Zone => !pinned && clear.is_none() && profile.is_none(),
        SpanBasis::Cylinder => false,
    }
}

/// Which ends of a hole must be open (B6, J1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct Open {
    /// The end at the lowest station.
    pub start: bool,
    /// The end at the highest station.
    pub finish: bool,
}

/// Whether a bore's depth is known (U8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Depth {
    /// Both ends of the original hole are known.
    Known,
    /// An end is unknown or irregular: no depth is required.
    UnknownEnd,
}

/// Why an `unresolved` requirement exists (section 8.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Cause {
    /// A face of the target no rule reads (U5).
    TargetUnreadFace,
    /// A neighbour definition with faces no rule reads (U4).
    NeighbourUnread,
    /// Neighbour material crosses the target's axis at an angle (U7).
    Skew,
    /// A curved neighbour face touches a target plane where no rule reads
    /// the contact (C6).
    ContactNoRule,
}

/// Where a seat's contact requires flat material (C4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Basis {
    /// The target face's own region: the true contact lies inside it.
    TargetFace,
    /// Not known: the target face's outline is unknown; unverifiable.
    Unknown,
}

/// One neighbour face in a contact.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct Toucher {
    /// The neighbour occurrence, by usage `#id`s.
    pub neighbour: Vec<u64>,
    /// Its face.
    pub face: u64,
    /// The gap along the target's normal, in mm: negative into the target.
    pub gap_mm: f64,
    /// Its flags, sorted.
    pub flags: Vec<Flag>,
}

/// One target face a seat requires (C4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct SeatContact {
    /// The target face.
    pub face: u64,
    /// Where flat material is required.
    pub basis: Basis,
    /// The target face's outline, when the basis is its region.
    pub outline: crate::features::Outline,
    /// The neighbour faces on it.
    pub by: Vec<Toucher>,
}

/// A wider neighbour cylinder ending at a target hole's rim (H1), for
/// review.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct Ring {
    /// The target hole.
    pub hole: FeatureId,
    /// The neighbour occurrence.
    pub neighbour: Vec<u64>,
    /// The ring's inner diameter: the hole's.
    pub inner_diameter_mm: f64,
    /// Its outer diameter: the neighbour cylinder's.
    pub outer_diameter_mm: f64,
}

/// What a requirement asks of a candidate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum Requires {
    /// A hole on this axis with these spans (B, J, B8).
    #[non_exhaustive]
    Bore {
        /// The target hole, or undecided cylinder.
        hole: FeatureId,
        /// Its axis, in the target's frame.
        axis: Axis,
        /// The spans that something occupies or that must be pinned.
        spans: Vec<BoreSpan>,
        /// The ends that must be open.
        open: Open,
        /// Whether its depth is known.
        depth: Depth,
    },
    /// A shaft on this axis (I).
    #[non_exhaustive]
    Shaft {
        /// The target shaft.
        shaft: FeatureId,
        /// Its axis.
        axis: Axis,
        /// The spans a neighbour's hole holds.
        spans: Vec<ShaftSpan>,
        /// How much longer the shaft may be before it reaches a blind
        /// hole's floor (I4), for information.
        #[serde(deserialize_with = "Option::deserialize")]
        max_extra_mm: Option<f64>,
    },
    /// Flat material at a target plane, covering each contact's region
    /// (C1 to C5, H1).
    #[non_exhaustive]
    Seat {
        /// The target plane.
        plane: FeatureId,
        /// Its outward normal.
        normal: crate::geom::Dir3,
        /// Its offset, in mm.
        offset_mm: f64,
        /// The target faces in contact.
        contacts: Vec<SeatContact>,
        /// Rings of wider neighbour cylinders on its holes' rims (H1).
        rings: Vec<Ring>,
    },
    /// Two opposed seats at this separation (L).
    #[non_exhaustive]
    Grip {
        /// The two seats' planes, or a conical seat's cone (L2).
        seats: Vec<FeatureId>,
        /// Their separation, in mm.
        length_mm: f64,
        /// The target holes that open onto both.
        holes: Vec<FeatureId>,
    },
    /// A cone on this axis with this apex and angle (Q1).
    #[non_exhaustive]
    ConicalSeat {
        /// The target cone.
        cone: FeatureId,
        /// Its axis.
        axis: Axis,
        /// The shared apex.
        apex_mm: crate::geom::Point3,
        /// The included angle.
        included_angle_rad: f64,
        /// The radial range the neighbour cones seated on it cover, from
        /// the least to the greatest of their ranges (Q1); `None` if one
        /// is unknown.
        #[serde(deserialize_with = "Option::deserialize")]
        radii_mm: Option<[f64; 2]>,
        /// The target cone's sense: a countersink (`hole`) or a cone on a
        /// shaft (`shaft`) (Q3).
        sense: crate::features::Sense,
    },
    /// A partial concave cylinder holding a shaft (U1): M4 v1 cannot
    /// verify it.
    #[non_exhaustive]
    PartialBore {
        /// The target cylinder.
        cylinder: FeatureId,
        /// Its axis.
        axis: Axis,
        /// Its diameter.
        diameter_mm: f64,
        /// What occupies it passes this.
        #[serde(deserialize_with = "Option::deserialize")]
        clear_diameter_mm: Option<f64>,
        /// The largest arc its stations cover.
        sweep_rad: f64,
        /// Where the occupant starts, along the axis.
        from_mm: f64,
        /// Where it ends.
        to_mm: f64,
    },
    /// A partial convex cylinder inside a neighbour hole (U1): M4 v1
    /// cannot verify it.
    #[non_exhaustive]
    PartialShaft {
        /// The target cylinder.
        cylinder: FeatureId,
        /// Its axis.
        axis: Axis,
        /// Its diameter.
        diameter_mm: f64,
        /// The largest it may be.
        #[serde(deserialize_with = "Option::deserialize")]
        clear_diameter_mm: Option<f64>,
        /// The largest arc its stations cover.
        sweep_rad: f64,
        /// Where the hole starts holding it.
        from_mm: f64,
        /// Where it stops.
        to_mm: f64,
    },
    /// Something no rule reads: never verified, fails until removed in
    /// review.
    #[non_exhaustive]
    Unresolved {
        /// The target feature it concerns, if one.
        #[serde(deserialize_with = "Option::deserialize")]
        feature: Option<FeatureId>,
        /// The neighbour definition it concerns, if one.
        #[serde(deserialize_with = "Option::deserialize")]
        definition: Option<u64>,
        /// Why, with the faces at issue.
        causes: Vec<CauseFaces>,
    },
}

/// One cause of an `unresolved` requirement.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct CauseFaces {
    /// Why.
    pub cause: Cause,
    /// The faces at issue, sorted.
    pub faces: Vec<u64>,
}

/// One relation a requirement was derived from (Y2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct SourceRef {
    /// The rule, such as `B1`.
    pub rule: String,
    /// The target occurrence, by usage `#id`s.
    pub occurrence: Vec<u64>,
    /// The neighbour occurrence.
    pub neighbour: Vec<u64>,
    /// The neighbour feature, if one.
    #[serde(deserialize_with = "Option::deserialize")]
    pub feature: Option<FeatureId>,
    /// The neighbour faces it reads, sorted.
    pub faces: Vec<u64>,
    /// The largest offset of the neighbour's axis from the target's over
    /// the relation, in mm.
    #[serde(deserialize_with = "Option::deserialize")]
    pub e_max_mm: Option<f64>,
    /// The sine of the angle between the axes.
    #[serde(deserialize_with = "Option::deserialize")]
    pub sine: Option<f64>,
    /// The radial margin (B, I), in mm.
    #[serde(deserialize_with = "Option::deserialize")]
    pub margin_mm: Option<f64>,
    /// The axial gap (J), in mm.
    #[serde(deserialize_with = "Option::deserialize")]
    pub gap_mm: Option<f64>,
    /// The stations it covers on the target's axis.
    #[serde(deserialize_with = "Option::deserialize")]
    pub span_mm: Option<[f64; 2]>,
    /// The floor of the two occurrences (Z2).
    pub floor_mm: f64,
    /// Its flags, sorted.
    pub flags: Vec<Flag>,
}

/// One requirement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct Requirement {
    /// `<kind>:<anchor>[+<anchor>]` (section 9.2).
    pub id: String,
    /// What it asks.
    pub requires: Requires,
    /// The largest floor of its relations, in mm.
    pub floor_mm: f64,
    /// Its flags, sorted.
    pub flags: Vec<Flag>,
    /// Its relations.
    pub sources: Vec<SourceRef>,
}

impl Requirement {
    /// False for the kinds every candidate fails, the original included,
    /// until review removes them (Y4): `unresolved`, `partial_bore`,
    /// `partial_shaft`, a bore or shaft flagged `target_undecided` or
    /// `core_not_clear` (U11), a bore flagged `open_at_unknown_end`, a bore
    /// or shaft with a piece past a known end or in a zone it states
    /// nothing of (U9), a seat with a contact whose basis is unknown, a
    /// grip flagged `seat_unverifiable` (U10), and a conical seat whose
    /// radial range is unknown. M3's witness test and M4's verification
    /// both ask this (SR5).
    pub fn verifiable(&self) -> bool {
        match &self.requires {
            Requires::Unresolved { .. }
            | Requires::PartialBore { .. }
            | Requires::PartialShaft { .. } => false,
            Requires::Seat { contacts, .. } => contacts.iter().all(|c| c.basis != Basis::Unknown),
            Requires::Grip { .. } => !self.flags.contains(&Flag::SeatUnverifiable),
            Requires::ConicalSeat { radii_mm, .. } => radii_mm.is_some(),
            Requires::Bore { spans, .. } => {
                self.axial_verifiable()
                    && !spans.iter().any(|s| {
                        unknown_piece(s.basis, s.pinned, s.clear_diameter_mm, s.profile_mm)
                    })
            }
            Requires::Shaft { spans, .. } => {
                self.axial_verifiable()
                    && !spans.iter().any(|s| {
                        unknown_piece(s.basis, s.pinned, s.clear_diameter_mm, s.profile_mm)
                    })
            }
        }
    }
}

impl Requirement {
    /// A bore's or shaft's flags allow it (U2, U8, U11).
    fn axial_verifiable(&self) -> bool {
        !self.flags.iter().any(|f| {
            matches!(
                f,
                Flag::TargetUndecided | Flag::OpenAtUnknownEnd | Flag::CoreNotClear
            )
        })
    }
}

/// Why a target feature is left out (A1, A2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum LeftOutReason {
    /// Nothing is on its axis.
    NothingOnAxis,
    /// A coaxial hole beyond a gap, with material between (J3).
    BlockedGap,
}

/// A target hole, shaft or undecided cylinder no requirement anchors.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct LeftOut {
    /// The feature.
    pub feature: FeatureId,
    /// Why.
    pub reason: LeftOutReason,
    /// Its axis.
    pub axis: Axis,
    /// Its largest diameter.
    pub diameter_mm: f64,
    /// Where its wall starts and ends, along the axis from its point.
    pub from_mm: f64,
    /// Where it ends.
    pub to_mm: f64,
}

/// The requirements of one or more occurrences of the target.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct Socket {
    /// `socket-<n>`, by smallest occurrence path.
    pub id: String,
    /// The occurrences it covers, by usage `#id`s.
    pub occurrences: Vec<Vec<u64>>,
    /// Its requirements, in id order (section 9.2).
    pub requirements: Vec<Requirement>,
    /// What it leaves out.
    pub left_out: Vec<LeftOut>,
}

/// The part chosen.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct Target {
    /// Its definition.
    pub definition: DefinitionRef,
    /// The frame every value is stated in.
    pub frame: Frame,
    /// Its solid.
    pub solid: FeatureId,
    /// Its largest stated uncertainty, in mm.
    pub part_uncertainty_mm: f64,
    /// Its solid's largest vertex offset, in mm.
    pub vertex_offset_mm: f64,
    /// Its features' `complete`.
    pub complete: bool,
    /// Its sound box, if it has one (V2).
    #[serde(deserialize_with = "Option::deserialize")]
    pub extent: Option<Extent>,
    /// Every hole, shaft and undecided cylinder: each anchors requirements
    /// or is left out, in every socket (A3).
    pub anchors: Vec<FeatureId>,
}

/// An occurrence: the target's or a neighbour's.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct PlacedRef {
    /// Its path, by usage `#id`s.
    pub path: Vec<u64>,
    /// The usages' ids, for people.
    pub label: Vec<Option<String>>,
    /// Its definition.
    pub definition: DefinitionRef,
    /// Its frame to the root's.
    pub transform: Rigid,
    /// Its part's uncertainty, in mm.
    pub part_uncertainty_mm: f64,
    /// Its part's vertex offset, in mm.
    pub vertex_offset_mm: f64,
    /// Its placement path's uncertainty, in mm.
    pub placement_uncertainty_mm: f64,
    /// The socket that covers it, for a target occurrence.
    #[serde(deserialize_with = "Option::deserialize")]
    pub socket: Option<String>,
}

/// What M7 checks the candidate's body against (V1, V2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct Envelope {
    /// Always `others_and_copies`.
    pub check_set: String,
    /// The target's extent, if it has one.
    #[serde(deserialize_with = "Option::deserialize")]
    pub extent: Option<Extent>,
}

/// What extraction derived; the digest covers it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct Extracted {
    /// The part chosen.
    pub target: Target,
    /// Its occurrences chosen, by path.
    pub occurrences: Vec<PlacedRef>,
    /// The neighbours a source cites, by path.
    pub neighbours: Vec<PlacedRef>,
    /// The sockets, by smallest occurrence path.
    pub sockets: Vec<Socket>,
    /// The envelope.
    pub envelope: Envelope,
}

/// How the part was chosen.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct SelectionRecord {
    /// `--part` as given.
    #[serde(deserialize_with = "Option::deserialize")]
    pub part: Option<String>,
    /// `--occurrence` as given.
    #[serde(deserialize_with = "Option::deserialize")]
    pub occurrence: Option<String>,
    /// How the part was matched.
    pub matched_by: MatchedBy,
}

/// The socket rules' constants, written into every document (Z3 to Z6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct SocketConstants {
    /// The least contact tolerance, in mm (Z3).
    pub contact_mm: f64,
    /// The near band and the reach of bounded rules, in mm (Z6).
    pub near_mm: f64,
    /// The largest sine compared as parallel (Z5).
    pub parallel_sine_rel: f64,
    /// Lengths within this merge occurrences (9.1), in mm.
    pub merge_mm: f64,
    /// Directions and angles within this merge occurrences (9.1).
    pub merge_sine: f64,
    /// The most occurrences an assembly may have (check 34).
    pub max_occurrences: u64,
    /// The most candidate pairs examined (check 39).
    pub max_relations: u64,
    /// The least guard beyond τ a clearance margin needs, in mm (B1).
    pub guard_mm: f64,
}

/// One review decision (section 9.4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ReviewEntry {
    /// The socket.
    pub socket: String,
    /// The requirement.
    pub requirement: String,
    /// The decision.
    pub status: ReviewStatus,
    /// Why, required for a removal.
    #[serde(deserialize_with = "Option::deserialize")]
    pub note: Option<String>,
}

/// A review decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
// Review keeps or removes; there is no third decision (R18).
#[allow(clippy::exhaustive_enums)]
pub enum ReviewStatus {
    /// Not reviewed: matched like `kept`.
    Unreviewed,
    /// Reviewed and kept.
    Kept,
    /// Removed from matching, with a note.
    Removed,
}

/// The notes a document carries: the assembly's.
pub type Notes = Vec<AssemblyNote>;
