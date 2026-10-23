//! A fit's evidence (`docs/research/m7-fit.md`, FM3): the outcome
//! [`super::decide`] made, with the plan's pairs and the kernel's measures
//! of each, for `fitment.match` v2. Built only by core, from a plan and the
//! measures it was decided on, or from an outcome no measures were taken
//! for (FV6, FV8).

use super::decide::{FindingAt, FitOutcome, applies, touches};
use super::freedom::FreeMotion;
use super::measures::{FitMeasures, PairMeasure};
use super::plan::{FitPlan, Pair, PairState, Placed};
use super::scene::FitScene;

/// One pair within reach, as the evidence states it (FM3): the plan's
/// pair, its other body's product, and the kernel's measures of it when it
/// was asked.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct PairEvidence {
    /// The pair, with its tolerances and zones (FT7, FZ).
    pub pair: Pair,
    /// A neighbour's product id (else its name), as the file states it;
    /// `None` for a copy, or a product the file does not name.
    pub product: Option<String>,
    /// The kernel's measures, if the pair was asked.
    pub measure: Option<PairMeasure>,
}

/// A fit's evidence (FM3): its outcome (FV7), the placements it was asked
/// at (FT2), the bodies judged, every pair within reach with its measures,
/// and each excused free turn (FT10). Built only by core.
#[derive(Debug, Clone, PartialEq)]
pub struct FitEvidence {
    outcome: FitOutcome,
    placements: Vec<Placed>,
    bodies: usize,
    pairs: Vec<PairEvidence>,
    free_turns: Vec<(String, FreeMotion)>,
}

impl FitEvidence {
    /// The evidence of `outcome`, decided on `measures` of `plan`, whose
    /// scene is `scene`.
    pub(crate) fn decided(
        plan: &FitPlan,
        measures: &FitMeasures,
        outcome: FitOutcome,
        scene: &FitScene,
    ) -> Self {
        let mut asked = measures.check.pairs.iter();
        let counted = plan.asked().count() == measures.check.pairs.len();
        let pairs = plan
            .pairs()
            .iter()
            .map(|pair| {
                let measure = (pair.state == PairState::Asked)
                    .then(|| asked.next().cloned())
                    .flatten()
                    .filter(|_| counted);
                let product = (!pair.copy)
                    .then(|| scene.body(&pair.body))
                    .flatten()
                    .and_then(|b| b.definition.product.as_ref())
                    .and_then(|p| p.id.clone().or_else(|| p.name.clone()));
                PairEvidence {
                    pair: pair.clone(),
                    product,
                    measure,
                }
            })
            .collect();
        Self {
            outcome,
            placements: plan.placements().to_vec(),
            bodies: plan.bodies().iter().filter(|b| !b.selected).count(),
            pairs,
            free_turns: Vec::new(),
        }
    }

    /// The evidence of an outcome for which no measures were taken: a
    /// placement not unique (FV6) or a helper's failure (FV8), at
    /// `placements`.
    pub(crate) fn unmeasured(outcome: FitOutcome, placements: Vec<Placed>) -> Self {
        Self {
            outcome,
            placements,
            bodies: 0,
            pairs: Vec::new(),
            free_turns: Vec::new(),
        }
    }

    /// FV5: this evidence under the control's outcome.
    #[must_use]
    pub(crate) fn controlled_by(mut self, control: &FitOutcome) -> Self {
        self.outcome = self.outcome.controlled_by(control);
        self
    }

    /// This evidence with the free turns FT10 excused, by socket.
    #[must_use]
    pub(crate) fn with_free_turns(mut self, free_turns: Vec<(String, FreeMotion)>) -> Self {
        self.free_turns = free_turns;
        self
    }

    /// The outcome (FV7).
    pub fn outcome(&self) -> &FitOutcome {
        &self.outcome
    }

    /// The placements the fit was asked at, in socket and occurrence
    /// order (FT2).
    pub fn placements(&self) -> &[Placed] {
        &self.placements
    }

    /// How many bodies of the scene the candidate was judged against at
    /// each placement: every leaf fitment reads but the selected
    /// occurrences, within reach or clear (FT3).
    pub fn bodies(&self) -> usize {
        self.bodies
    }

    /// Every pair within reach, asked or not, in placement order.
    pub fn pairs(&self) -> &[PairEvidence] {
        &self.pairs
    }

    /// Each excused free turn, with its socket (FT10).
    pub fn free_turns(&self) -> &[(String, FreeMotion)] {
        &self.free_turns
    }

    /// True if the pair at `occurrence` and `body` bears on `socket`
    /// (FV7); always for the candidate as a whole (`None`).
    pub fn touches(
        &self,
        socket: Option<&str>,
        occurrence: &[u64],
        body: &[u64],
        copy: bool,
    ) -> bool {
        socket.is_none_or(|s| touches(&self.placements, s, occurrence, body, copy))
    }

    /// True if a finding `at` applies to `socket` (FV7); always for the
    /// candidate as a whole (`None`).
    pub fn applies(&self, socket: Option<&str>, at: &FindingAt) -> bool {
        socket.is_none_or(|s| applies(&self.placements, s, at))
    }
}
