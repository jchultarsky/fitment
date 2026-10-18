//! Section 7.7's route (docs/research/m6-api.md, JB1, UP1, UP5, RA9):
//! `POST /v1/features` creates the job `features`, `fitment features FILE`
//! on an upload of the caller's.

use std::sync::Arc;

use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Response;

use super::jobs::{self, Id, Submission};
use crate::Shared;
use crate::http::error::ApiError;
use crate::http::extract::Reader;
use crate::http::json::Sent;
use crate::jobs::model::Kind;
use crate::jobs::request::{self, FeaturesJob, FeaturesRequest};

/// `POST /v1/features` (section 7.7).
pub(super) async fn create(
    State(shared): State<Arc<Shared>>,
    Reader(caller): Reader,
    axum::Extension(request_id): Id,
    headers: HeaderMap,
    Sent(body, sent): Sent<FeaturesRequest>,
) -> Result<Response, ApiError> {
    // JB9 (review S4 of PR 5): a key's replay is answered before the
    // references are checked, so it stands once its inputs are gone.
    let fingerprint = request::fingerprint("POST", "/v1/features", &sent);
    if let Some(replayed) = jobs::replay(&shared, &caller, &headers, &fingerprint).await? {
        return Ok(replayed);
    }
    let sha256 = request::sha256_ref(&body.upload, "upload")?;
    let bytes = super::nameable(&shared, caller.user.key, &sha256).await?;
    let (request, request_sha256) = request::normalized(&FeaturesJob {
        upload: format!("sha256:{sha256}"),
    });
    let submission = Submission {
        kind: Kind::Features,
        request,
        request_sha256,
        inputs: vec![(sha256, bytes)],
        fingerprint,
        cites: Vec::new(),
    };
    jobs::submit(&shared, &caller, &headers, &request_id, submission).await
}
