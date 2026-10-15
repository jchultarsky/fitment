//! The structural rules `from_json` checks even when the digest matches,
//! for anyone can recompute a digest (`docs/research/m3-socket.md`, 9.4).

use std::collections::BTreeSet;

use super::model::{
    Extracted, Fit, Flag, Requirement, Requires, ReviewEntry, ReviewStatus, Socket,
};
use super::{CONTACT_MM, MERGE_MM};
use crate::features::FeatureId;

/// The longest review note, in characters.
pub(crate) const MAX_NOTE_CHARS: usize = 2000;

type Checked = Result<(), String>;

/// Every rule of 9.4 that `extracted` must keep.
pub(crate) fn extracted(extracted: &Extracted) -> Checked {
    for (k, socket) in extracted.sockets.iter().enumerate() {
        if socket.id != format!("socket-{}", k + 1) {
            return Err(format!("socket {} is not numbered in order", socket.id));
        }
        let firsts = socket.occurrences.first();
        if socket.occurrences.is_empty() || !socket.occurrences.windows(2).all(|w| w[0] < w[1]) {
            return Err(format!(
                "{}'s occurrences are not sorted and unique",
                socket.id
            ));
        }
        if k > 0 && extracted.sockets[k - 1].occurrences.first() >= firsts {
            return Err(format!(
                "{} is not numbered by its smallest occurrence path",
                socket.id
            ));
        }
        socket_rules(socket, &extracted.target.anchors)?;
    }
    Ok(())
}

fn socket_rules(socket: &Socket, anchors: &[FeatureId]) -> Checked {
    let id = &socket.id;
    let mut keys = Vec::new();
    for requirement in &socket.requirements {
        requirement_rules(requirement).map_err(|e| format!("{id}/{}: {e}", requirement.id))?;
        keys.push((
            super::combine::order_key(requirement),
            requirement.id.clone(),
        ));
    }
    if !keys.windows(2).all(|w| w[0] < w[1]) {
        return Err(format!("{id}'s requirements are not in order, or repeat"));
    }
    // U10: a grip is flagged exactly when a seat of it is unverifiable.
    let mut flagged = socket.requirements.clone();
    for r in &mut flagged {
        r.flags.retain(|f| *f != Flag::SeatUnverifiable);
    }
    super::combine::flag_grips(&mut flagged);
    if flagged != socket.requirements {
        return Err(format!(
            "{id}: a grip's seat_unverifiable flag disagrees with its seats"
        ));
    }
    if socket.left_out.iter().any(|l| {
        !(positive(l.diameter_mm)
            && l.from_mm.is_finite()
            && l.to_mm.is_finite()
            && l.from_mm <= l.to_mm)
    }) {
        return Err(format!(
            "{id} leaves out a feature with no diameter or wall"
        ));
    }
    if !socket
        .left_out
        .windows(2)
        .all(|w| w[0].feature.instance() < w[1].feature.instance())
    {
        return Err(format!(
            "{id}'s left-out features are not sorted and unique"
        ));
    }
    // A3: each hole, shaft and undecided cylinder anchors requirements or
    // is left out, never both and never neither.
    let anchored: BTreeSet<FeatureId> = socket
        .requirements
        .iter()
        .filter_map(super::combine::anchor)
        .collect();
    let left: BTreeSet<FeatureId> = socket.left_out.iter().map(|l| l.feature).collect();
    for anchor in anchors {
        if anchored.contains(anchor) == left.contains(anchor) {
            return Err(format!(
                "{id}: {anchor} is {} (A3)",
                if left.contains(anchor) {
                    "both required and left out"
                } else {
                    "neither required nor left out"
                }
            ));
        }
    }
    if left.iter().any(|f| !anchors.contains(f)) {
        return Err(format!("{id} leaves out a feature that is no anchor"));
    }
    Ok(())
}

fn positive(x: f64) -> bool {
    x.is_finite() && x > 0.0
}

fn requirement_rules(requirement: &Requirement) -> Checked {
    if !requirement.flags.windows(2).all(|w| w[0] < w[1]) {
        return Err("its flags are not sorted and unique".to_owned());
    }
    if !(requirement.floor_mm.is_finite() && requirement.floor_mm >= 0.0) {
        return Err("its floor is not a length".to_owned());
    }
    let largest = requirement
        .sources
        .iter()
        .map(|s| s.floor_mm)
        .fold(0.0, f64::max);
    if !requirement.sources.is_empty() && requirement.floor_mm < largest {
        return Err("its floor is below one of its sources' (Y1)".to_owned());
    }
    lists_and_sources(requirement)?;
    requires_rules(requirement)
}

/// The rules of each kind of requirement.
fn requires_rules(requirement: &Requirement) -> Checked {
    let labelled = requirement.flags.contains(&Flag::ThreadNotConfirmed)
        || requirement.flags.contains(&Flag::Conflict);
    match &requirement.requires {
        Requires::Bore { spans, .. } => {
            bore_rules(spans, labelled)?;
            margins(
                spans.iter().map(|s| {
                    (
                        s.from_mm,
                        s.to_mm,
                        s.fit,
                        s.clear_diameter_mm,
                        s.hole_diameter_mm,
                    )
                }),
                &requirement.sources,
                -1.0,
            )?;
        }
        Requires::Shaft { spans, .. } => {
            shaft_rules(spans, labelled)?;
            margins(
                spans.iter().map(|s| {
                    (
                        s.from_mm,
                        s.to_mm,
                        s.fit,
                        s.clear_diameter_mm,
                        s.shaft_diameter_mm,
                    )
                }),
                &requirement.sources,
                1.0,
            )?;
        }
        Requires::Seat {
            contacts, rings, ..
        } => seat_rules(contacts, rings)?,
        Requires::Grip {
            seats, length_mm, ..
        } => {
            if seats.len() != 2 || seats[0] == seats[1] || !length_mm.is_finite() {
                return Err("a grip does not name two distinct seats".to_owned());
            }
        }
        Requires::ConicalSeat {
            included_angle_rad,
            radii_mm,
            ..
        } => {
            if !(*included_angle_rad > 0.0 && *included_angle_rad < std::f64::consts::PI) {
                return Err("a cone's angle is not between 0 and π".to_owned());
            }
            if radii_mm.is_some_and(|[a, b]| !(a >= 0.0 && a <= b && b.is_finite())) {
                return Err("a cone's radii are not ordered".to_owned());
            }
        }
        Requires::PartialBore {
            diameter_mm,
            sweep_rad,
            from_mm,
            to_mm,
            ..
        }
        | Requires::PartialShaft {
            diameter_mm,
            sweep_rad,
            from_mm,
            to_mm,
            ..
        } => {
            ordered([(*from_mm, *to_mm)].into_iter())?;
            let sweep = *sweep_rad > 0.0 && *sweep_rad <= 2.0 * std::f64::consts::PI + 1e-9;
            if !(positive(*diameter_mm) && sweep) {
                return Err("a partial cylinder's diameter or sweep is out of range".to_owned());
            }
        }
        Requires::Unresolved { causes, .. } => {
            if causes.is_empty()
                || causes
                    .iter()
                    .any(|c| !c.faces.windows(2).all(|w| w[0] < w[1]))
            {
                return Err("an unresolved requirement has no cause, or unsorted faces".to_owned());
            }
        }
    }
    Ok(())
}

/// A seat: each contact touched, each ring's diameters ordered, each
/// outline readable and its basis agreeing with it.
fn seat_rules(contacts: &[super::model::SeatContact], rings: &[super::model::Ring]) -> Checked {
    if contacts.is_empty() || contacts.iter().any(|c| c.by.is_empty()) {
        return Err("a seat has a contact with nothing on it".to_owned());
    }
    if rings.iter().any(|r| {
        !(positive(r.inner_diameter_mm)
            && r.inner_diameter_mm < r.outer_diameter_mm
            && r.outer_diameter_mm.is_finite())
    }) {
        return Err("a ring's diameters are not ordered and positive".to_owned());
    }
    for c in contacts {
        let known = crate::features::readback::outline(&c.outline, None)?;
        if known.is_some() == (c.basis == super::model::Basis::Unknown) {
            return Err("a contact's basis disagrees with its outline".to_owned());
        }
    }
    Ok(())
}

/// A requirement's lists in the document's order, its sources present
/// (Y2), and a conflict a source's.
fn lists_and_sources(requirement: &Requirement) -> Checked {
    // Its lists in the document's order, each unique.
    let mut ordered_copy = requirement.clone();
    super::merge::normalize(&mut ordered_copy);
    if ordered_copy != *requirement {
        return Err("its lists are not in the document's order, or repeat".to_owned());
    }
    // Every kind a relation gives cites its relations (Y2); a conflict is
    // a source's.
    let derived = matches!(
        requirement.requires,
        Requires::Grip { .. } | Requires::Unresolved { .. }
    );
    if !derived && requirement.sources.is_empty() {
        return Err("it cites no source (Y2)".to_owned());
    }
    if requirement.flags.contains(&Flag::Conflict)
        && !requirement
            .sources
            .iter()
            .any(|s| s.flags.contains(&Flag::Conflict))
    {
        return Err("it is flagged conflict, but no source is".to_owned());
    }
    Ok(())
}

fn tight(fit: Fit) -> bool {
    matches!(fit, Fit::LineToLine | Fit::Interference)
}

/// A bore's spans: ordered, positive, a clearance's clear diameter at most
/// the original, a tight fit labelled.
fn bore_rules(spans: &[super::model::BoreSpan], labelled: bool) -> Checked {
    ordered(spans.iter().map(|s| (s.from_mm, s.to_mm)))?;
    for s in spans {
        if !positive(s.hole_diameter_mm) {
            return Err("a span's diameter is not positive".to_owned());
        }
        if s.fit == Fit::Clearance
            && s.clear_diameter_mm
                .is_none_or(|c| !positive(c) || c > s.hole_diameter_mm)
        {
            return Err("a clearance span's clear diameter is above the original".to_owned());
        }
        if tight(s.fit) && !labelled {
            return Err("a tight fit carries no thread_not_confirmed".to_owned());
        }
        if tight(s.fit) && !s.pinned {
            return Err("a tight fit is not pinned (Y1)".to_owned());
        }
    }
    Ok(())
}

/// A shaft's spans: ordered, positive, a clearance's limit at least the
/// original, a tight fit labelled.
fn shaft_rules(spans: &[super::model::ShaftSpan], labelled: bool) -> Checked {
    ordered(spans.iter().map(|s| (s.from_mm, s.to_mm)))?;
    for s in spans {
        if !positive(s.shaft_diameter_mm) {
            return Err("a span's diameter is not positive".to_owned());
        }
        if s.fit == Fit::Clearance
            && s.clear_diameter_mm
                .is_none_or(|c| !positive(c) || c < s.shaft_diameter_mm)
        {
            return Err("a clearance span allows less than the original".to_owned());
        }
        if tight(s.fit) && !labelled {
            return Err("a tight fit carries no thread_not_confirmed".to_owned());
        }
        if tight(s.fit) && !s.pinned {
            return Err("a tight fit is not pinned (Y1)".to_owned());
        }
    }
    Ok(())
}

/// A clearance span agrees with every source whose span overlaps it: each
/// source's margin leaves a clearance (above its τ), and the clear
/// diameter is the strictest they give, the original diameter shifted by
/// twice the margin (`sign` −1 for a bore, whose occupant reaches in; +1
/// for a shaft, whose holder leaves room). A clear diameter lowered (a
/// bore) or raised (a shaft) by hand is refused.
fn margins(
    spans: impl Iterator<Item = (f64, f64, Fit, Option<f64>, f64)>,
    sources: &[super::model::SourceRef],
    sign: f64,
) -> Checked {
    for (from, to, fit, clear, diameter) in spans {
        let (Fit::Clearance, Some(clear)) = (fit, clear) else {
            continue;
        };
        for source in sources {
            let (Some(margin), Some([a, b])) = (source.margin_mm, source.span_mm) else {
                continue;
            };
            // Pieces are cut where spans end and joined only when equal, so
            // a source over any of a span's interior holds over all of it.
            if b.min(to) - a.max(from) <= MERGE_MM {
                continue;
            }
            if margin <= CONTACT_MM.max(source.floor_mm) {
                return Err("a clearance span has a source with no clearance".to_owned());
            }
            let limit = diameter + sign * 2.0 * margin;
            if sign * (clear - limit) > 1e-9 * diameter.max(1.0) {
                return Err(
                    "a clearance span's clear diameter is looser than a source's margin".to_owned(),
                );
            }
        }
    }
    Ok(())
}

/// Spans finite, each ordered, and in order without overlapping.
fn ordered(spans: impl Iterator<Item = (f64, f64)>) -> Checked {
    let mut last = f64::NEG_INFINITY;
    for (from, to) in spans {
        if !(from.is_finite() && to.is_finite() && from <= to && from >= last - 1e-9) {
            return Err("its spans are not finite, ordered and disjoint".to_owned());
        }
        last = to;
    }
    Ok(())
}

/// The review: each entry names a socket's requirement once; a removal
/// has a note; notes are at most `MAX_NOTE_CHARS`. A missing entry reads
/// as unreviewed.
pub(crate) fn review(extracted: &Extracted, review: &[ReviewEntry]) -> Checked {
    let mut seen = BTreeSet::new();
    for entry in review {
        let exists = extracted.sockets.iter().any(|s| {
            s.id == entry.socket && s.requirements.iter().any(|r| r.id == entry.requirement)
        });
        if !exists {
            return Err(format!(
                "the review names {}/{}, which the document does not hold",
                entry.socket, entry.requirement
            ));
        }
        if !seen.insert((&entry.socket, &entry.requirement)) {
            return Err(format!(
                "the review names {}/{} twice",
                entry.socket, entry.requirement
            ));
        }
        if entry.status == ReviewStatus::Removed
            && entry.note.as_deref().is_none_or(|n| n.trim().is_empty())
        {
            return Err(format!(
                "{}/{} is removed without a note",
                entry.socket, entry.requirement
            ));
        }
        if entry
            .note
            .as_deref()
            .is_some_and(|n| n.chars().count() > MAX_NOTE_CHARS)
        {
            return Err(format!(
                "{}/{}'s note is longer than {MAX_NOTE_CHARS} characters",
                entry.socket, entry.requirement
            ));
        }
    }
    Ok(())
}
