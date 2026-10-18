//! The judging path (docs/research/m6-api.md, SN1, SV4): the only module
//! that builds a `Matcher` or a `MatchDocument` (from PR 6), and the one
//! that builds the fit verifier the server judges with (SV8, OP9).

use std::sync::Arc;

use fitment_core::{FitVerifier, NoFitVerifier};

use crate::config::Fit;

/// The fit verifier, built once at start (SV8): core's trait is `Sync`
/// only, so the server holds it as `Send + Sync` to move it into blocking
/// tasks.
pub type Verifier = Arc<dyn FitVerifier + Send + Sync>;

/// The verifier `fit.verifier` names (OP9): `"none"`, `NoFitVerifier`, in
/// M6. `Config::validate` refuses any other value, so a typo never falls
/// back to `"none"` silently.
pub fn verifier(fit: &Fit) -> Verifier {
    debug_assert_eq!(fit.verifier, "none");
    Arc::new(NoFitVerifier)
}

/// The kernel and version the verifier names, for `fitment.info` (DC11):
/// `None` until M7.
pub fn kernel(verifier: &Verifier) -> Option<(String, String)> {
    verifier.kernel()
}
