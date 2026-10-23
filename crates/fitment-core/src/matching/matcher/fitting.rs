//! The fit, asked once per candidate (`docs/research/m7-fit.md`, FC2,
//! FT11): with a verifier that has an identity, after every socket's
//! interface passed, the placement's freedom worked out at each socket
//! (FC4), the original's control run once (FT9), then one request with
//! the candidate at every occurrence of every socket (FT2), unless a
//! placement is free in a way FT10 does not excuse (FV6). The verifier
//! returns measures; the status is [`fit::decide`]'s (FV1), split into
//! each socket's fit (FV7).

use std::borrow::Cow;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock, PoisonError};

use super::{Matcher, Ready, SocketResult};
use crate::assembly::SelectedComponent;
use crate::features::Part;
use crate::fit::{
    self, Fetch, FitBody, FitCandidate, FitEvidence, FitFailure, FitIdentity, FitOutcome,
    FitRequest, FitScene, FitStatus, MotionKind, OCCURRENCE_UNPLACED,
    OCCURRENCE_UNPLACED_ELSEWHERE, Placed, PlanInput, Zones,
};
use crate::geom::Rigid;
use crate::matching::CandidateView;
use crate::socket::SocketDocument;
use crate::verdict::{RequirementId, VerdictKind, decide};

/// The reason a socket judged alone (AL14's `verify_at`) gets no fit:
/// the fit is the candidate's, at every socket at once (FT11).
pub(super) const ALONE: &str = "a socket judged alone gets no fit";

/// How the candidate's body reaches the fit (FB4): a file's bytes, a
/// stored file's bytes on demand, a component of an assembly, or nothing
/// the helper could read.
pub(super) enum Body<'b> {
    /// The bytes M2's gate accepted, and their SHA-256.
    Step { bytes: &'b [u8], sha256: String },
    /// A stored document's file, fetched only when its fit is asked
    /// (FC6), and its row's SHA-256, which the bytes must hash to.
    Stored { sha256: String, fetch: Fetch<'b> },
    /// A component, read and chosen by core; with its assembly's bytes
    /// when that is not the socket's (FB5).
    Component(&'b SelectedComponent, Option<&'b [u8]>),
    /// A given part: no bytes.
    None,
}

/// Whether a judgment counts against the cap on fits (M7-49): a
/// candidate's does; the original's (VD15) is no candidate's.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Counted {
    Yes,
    No,
}

/// What a matcher with a verifier that has an identity holds for the fit:
/// the identity, the scene and zones core read from the verifier's
/// assembly (FB1, FZ1), and the control once it has run complete (FT9).
/// A control whose run was incomplete (FV8) is not kept: the next
/// candidate runs it again; the last such run is, for the document and
/// for [`Matcher::fit_incomplete`].
pub(super) struct Fitting {
    identity: FitIdentity,
    read: Result<(FitScene, Zones), String>,
    control: OnceLock<FitEvidence>,
    /// Held while the control runs, so it runs once at a time; the last
    /// incomplete run.
    incomplete: Mutex<Option<FitEvidence>>,
    /// M7-49: the most candidates whose interface passes that a run asks
    /// fits for; `None` for no cap.
    max_fits: Option<u64>,
    /// The candidates whose every socket's interface passed so far.
    passes: AtomicU64,
}

impl Fitting {
    /// FB1: the scene of `assembly` for `document`, read once; an
    /// assembly that is missing, or not the socket's source, is no scene,
    /// and every fit is then `not_verified`.
    pub(super) fn new(
        identity: FitIdentity,
        assembly: Option<&[u8]>,
        document: &SocketDocument,
    ) -> Self {
        let named = !(identity.kernel.is_empty() || identity.version.is_empty());
        let read = match assembly {
            _ if !named => Err("the fit verifier names no kernel or version".to_owned()),
            None => Err("the fit verifier holds no assembly".to_owned()),
            Some(bytes) => FitScene::of(bytes, document)
                .map(|scene| {
                    let zones = fit::zones(document, &scene);
                    (scene, zones)
                })
                .map_err(|e| e.to_string()),
        };
        Self {
            identity,
            read,
            control: OnceLock::new(),
            incomplete: Mutex::new(None),
            max_fits: None,
            passes: AtomicU64::new(0),
        }
    }

    pub(super) fn identity(&self) -> &FitIdentity {
        &self.identity
    }

    /// Why no fit can be measured, if none can: the scene of the
    /// verifier's assembly could not be read (FU2).
    pub(super) fn unavailable(&self) -> Option<&str> {
        self.read.as_ref().err().map(String::as_str)
    }

    /// M7-49: the cap on fits.
    pub(super) fn set_max_fits(&mut self, max_fits: Option<u64>) {
        self.max_fits = max_fits;
    }

    pub(super) fn max_fits(&self) -> Option<u64> {
        self.max_fits
    }

    /// The candidates whose every socket's interface passed so far.
    pub(super) fn passes(&self) -> u64 {
        self.passes.load(Ordering::Relaxed)
    }

    /// M7-49: one more candidate passed its interface; false once the
    /// passes exceed the cap, when no fit is asked for it. The count is
    /// the interface's, so whether a run exceeds the cap never depends on
    /// the order candidates are judged in.
    fn within_max_fits(&self, counted: Counted) -> bool {
        if counted == Counted::No {
            return true;
        }
        let passes = self
            .passes
            .fetch_add(1, Ordering::Relaxed)
            .saturating_add(1);
        self.max_fits.is_none_or(|cap| passes <= cap)
    }

    /// The control: its complete run, else its last incomplete one.
    pub(super) fn control(&self) -> Option<FitEvidence> {
        self.control
            .get()
            .cloned()
            .or_else(|| self.last_incomplete())
    }

    fn last_incomplete(&self) -> Option<FitEvidence> {
        self.incomplete
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// True if a run of the control was incomplete (FV8).
    pub(super) fn control_incomplete(&self) -> bool {
        self.last_incomplete().is_some()
    }
}

/// True for a socket whose interface passed with an occurrence that could
/// not be placed (FT2).
fn unplaced(result: &SocketResult) -> bool {
    matches!(result.verdict.fit(), FitStatus::NotChecked { reason } if reason == OCCURRENCE_UNPLACED)
}

/// `ready`'s active requirements, as `decide` takes them.
fn requirement_ids(ready: &Ready<'_>) -> Vec<RequirementId> {
    ready
        .view
        .active()
        .iter()
        .map(|r| RequirementId::new(r.id.clone()))
        .collect()
}

/// The socket's verdict again, with `fit` (VD4).
fn refit(ready: &Ready<'_>, result: &mut SocketResult, fit: FitStatus) {
    result.verdict = decide(&requirement_ids(ready), result.checks.clone(), fit);
}

impl Matcher<'_> {
    /// FC2, FT11: the candidate's one fit, once every socket's interface
    /// passed, each socket's verdict decided again with its share of it
    /// (FV7); `None`, and the sockets as they are, when one rejected. A
    /// socket with an occurrence it could not place gets no fit, and
    /// neither do the others (FT2). The control runs first, once (FT9); a
    /// placement free in a way FT10 does not excuse is asked no fit (FV6).
    pub(super) fn fit_candidate(
        &self,
        fitting: &Fitting,
        part: &Part,
        cand: &CandidateView<'_>,
        (body, counted): (&Body<'_>, Counted),
        sockets: &mut [SocketResult],
    ) -> Option<FitEvidence> {
        if sockets.is_empty()
            || sockets
                .iter()
                .any(|s| s.verdict.kind() == VerdictKind::Reject)
        {
            return None;
        }
        // M7-49: past the cap the run is refused, its document unwritten;
        // the candidate's sockets keep the fit they were judged with.
        if !fitting.within_max_fits(counted) {
            return None;
        }
        if sockets.iter().any(unplaced) {
            for (ready, result) in self.sockets.iter().zip(sockets.iter_mut()) {
                if !unplaced(result) {
                    let reason = OCCURRENCE_UNPLACED_ELSEWHERE.to_owned();
                    refit(ready, result, FitStatus::NotChecked { reason });
                }
            }
            return None;
        }
        let mut free: Vec<(String, String)> = Vec::new();
        let mut turns = Vec::new();
        for (ready, result) in self.sockets.iter().zip(sockets.iter_mut()) {
            let Some(at) = result.placement.as_ref() else {
                continue;
            };
            let freedom = fit::freedom(&ready.view, cand, at, &self.policy);
            if !freedom.excused() {
                free.push((result.socket.clone(), freedom.describe()));
            }
            let excused = freedom
                .motions
                .iter()
                .filter(|m| m.kind == MotionKind::Turn && m.axisymmetric);
            turns.extend(excused.map(|m| (result.socket.clone(), m.clone())));
            result.freedom = Some(freedom);
        }
        let control = self.control_of(fitting);
        let ids: Vec<String> = sockets.iter().map(|s| s.socket.clone()).collect();
        let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
        let placements = placed(self.document, sockets);
        let evidence = if free.is_empty() {
            self.measured(fitting, part, body, placements, &ids)
        } else {
            let free: Vec<(&str, String)> =
                free.iter().map(|(s, m)| (s.as_str(), m.clone())).collect();
            let outcome = FitOutcome::not_unique(&ids, &free);
            FitEvidence::unmeasured(outcome, placements.unwrap_or_default())
        };
        let evidence = evidence
            .controlled_by(control.outcome())
            .with_free_turns(turns);
        for (ready, result) in self.sockets.iter().zip(sockets.iter_mut()) {
            let fit = evidence.outcome().socket(&result.socket).cloned();
            let fit = fit.unwrap_or_else(|| FitStatus::NotVerified {
                reason: fit::FitCode::HelperProtocol.sentence().to_owned(),
            });
            refit(ready, result, fit);
        }
        Some(evidence)
    }

    /// FT9: the control, run once, at the first candidate whose every
    /// socket's interface passed, one run at a time. A run that is
    /// incomplete (FV8) is not kept, so the next such candidate runs it
    /// again, as after a panic; it is remembered as the last incomplete one.
    fn control_of<'f>(&self, fitting: &'f Fitting) -> Cow<'f, FitEvidence> {
        let mut last = fitting
            .incomplete
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(kept) = fitting.control.get() {
            return Cow::Borrowed(kept);
        }
        let evidence = self.control_evidence(fitting);
        if evidence.outcome().incomplete() {
            *last = Some(evidence.clone());
            return Cow::Owned(evidence);
        }
        Cow::Borrowed(fitting.control.get_or_init(|| evidence))
    }

    /// FT9: the original's own solid at each selected occurrence, at the
    /// identity placement, against the same check set and zones, its
    /// copies being the originals; its placements are the assembly's own,
    /// so FT10 does not apply.
    fn control_evidence(&self, fitting: &Fitting) -> FitEvidence {
        let ids: Vec<&str> = self
            .sockets
            .iter()
            .map(|r| r.view.socket().id.as_str())
            .collect();
        let fail = |why: String| {
            let outcome = FitOutcome::failed(&ids, &FitFailure::Unavailable(why));
            FitEvidence::unmeasured(outcome, Vec::new())
        };
        let (scene, zones) = match &fitting.read {
            Ok((scene, zones)) => (scene, zones),
            Err(why) => return fail(format!("the fit's scene cannot be read: {why}")),
        };
        let original = scene.bodies().iter().find(|b| b.selected);
        let Some((original, part)) = original.and_then(|b| Some((b, scene.part(b)?))) else {
            return fail("the scene holds no selected occurrence".to_owned());
        };
        let placements: Option<Vec<Placed>> = ids
            .iter()
            .map(|id| Placed::every(self.document, id, &Rigid::IDENTITY))
            .collect::<Option<Vec<Vec<Placed>>>>()
            .map(|all| all.into_iter().flatten().collect());
        let Some(placements) = placements else {
            return fail("a placement of the original overflows".to_owned());
        };
        let body = FitBody::component(
            scene.sha256(),
            original.path.clone(),
            original.faces.clone(),
        );
        self.ask(fitting, (scene, zones), part, body, placements, &ids)
    }

    /// The candidate's fit at `placements`: no measures, and a helper's
    /// failure (FV8), when the placements, the scene or the body cannot be
    /// had.
    fn measured(
        &self,
        fitting: &Fitting,
        part: &Part,
        body: &Body<'_>,
        placements: Option<Vec<Placed>>,
        ids: &[&str],
    ) -> FitEvidence {
        let fail = |placements: Vec<Placed>, why: &str| {
            let outcome = FitOutcome::failed(ids, &FitFailure::Unavailable(why.to_owned()));
            FitEvidence::unmeasured(outcome, placements)
        };
        let Some(placements) = placements else {
            return fail(Vec::new(), "a placement of the candidate overflows");
        };
        let (scene, zones) = match &fitting.read {
            Ok((scene, zones)) => (scene, zones),
            Err(why) => {
                return fail(
                    placements,
                    &format!("the fit's scene cannot be read: {why}"),
                );
            }
        };
        // FC6: a stored file's bytes, read now that its fit is asked, and
        // only if they hash to its row.
        let fetched = match body {
            Body::Stored { sha256, fetch } => match fetch() {
                Ok(bytes) if crate::sha256::sha256_hex(&bytes) == *sha256 => Some(bytes),
                _ => {
                    let outcome = FitOutcome::failed(ids, &FitFailure::StoredBytesUnreadable);
                    return FitEvidence::unmeasured(outcome, placements);
                }
            },
            _ => None,
        };
        let body = match (body, &fetched) {
            (Body::Stored { sha256, .. }, Some(bytes)) => Some(FitBody::Step {
                bytes,
                sha256: sha256.clone(),
            }),
            _ => fit_body(body, scene),
        };
        let Some(body) = body else {
            return fail(
                placements,
                "the candidate's bytes are not available to the fit",
            );
        };
        self.ask(fitting, (scene, zones), part, body, placements, ids)
    }

    /// FC1: one request to the verifier, and the status from its measures
    /// by core's own plan and [`fit::decide`] (FV1); a helper's failure is
    /// never `verified` (FV8).
    fn ask(
        &self,
        fitting: &Fitting,
        (scene, zones): (&FitScene, &Zones),
        part: &Part,
        body: FitBody<'_>,
        placements: Vec<Placed>,
        ids: &[&str],
    ) -> FitEvidence {
        let candidate = FitCandidate::of(part, &body);
        let identity = fitting.identity.clone();
        let measured = {
            let input = PlanInput::new(
                scene,
                zones,
                &candidate,
                &placements,
                &self.policy,
                identity,
            );
            let request = FitRequest::new(body, self.document, input);
            self.fit.measure(&request).map(|measures| {
                let plan = fit::plan(request.input(), &measures.scene, &measures.place);
                let outcome = fit::decide(&plan, &measures);
                FitEvidence::decided(&plan, &measures, outcome, scene)
            })
        };
        measured.unwrap_or_else(|failure| {
            FitEvidence::unmeasured(FitOutcome::failed(ids, &failure), placements)
        })
    }
}

/// FT2: `T_k · A` at every occurrence of every socket, in socket and
/// occurrence order; `None` if one overflows.
fn placed(document: &SocketDocument, sockets: &[SocketResult]) -> Option<Vec<Placed>> {
    let mut out = Vec::new();
    for result in sockets {
        let at = result.placement.as_ref()?.transform;
        out.extend(Placed::every(document, &result.socket, &at)?);
    }
    Some(out)
}

/// FB4: the body the helper reads: a file's bytes and their SHA-256, or a
/// component named by its assembly, its occurrence (the scene's first of
/// its definition, when the assembly is the socket's) and its faces'
/// `#id`s; a component of another assembly with that assembly's bytes,
/// its second scene (FB5). A stored file's bytes are fetched by
/// [`Matcher::measured`].
fn fit_body<'b>(body: &Body<'b>, scene: &FitScene) -> Option<FitBody<'b>> {
    match body {
        Body::Step { bytes, sha256 } => Some(FitBody::Step {
            bytes,
            sha256: sha256.clone(),
        }),
        Body::Component(component, Some(assembly))
            if component.assembly_sha256() != scene.sha256() =>
        {
            Some(FitBody::component_of(
                assembly,
                fit::faces_of(component.part()),
            ))
        }
        Body::Component(component, _) => {
            let sha256 = component.assembly_sha256();
            let own = (sha256 == scene.sha256())
                .then(|| {
                    scene
                        .bodies()
                        .iter()
                        .find(|b| b.definition.instance == component.definition())
                })
                .flatten();
            Some(match own {
                Some(b) => FitBody::component(sha256, b.path.clone(), b.faces.clone()),
                None => FitBody::component(sha256, Vec::new(), fit::faces_of(component.part())),
            })
        }
        Body::Stored { .. } | Body::None => None,
    }
}
