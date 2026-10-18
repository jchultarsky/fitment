//! The server's own routes (docs/research/m6-api.md, section 7.2):
//! `/v1/health`, `/v1/ready`, `/v1/info` and `/v1/openapi.json`.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::Response;

use crate::Shared;
use crate::http::blocking;
use crate::http::error::{ApiError, ErrorCode};
use crate::http::extract::Reader;
use crate::http::json;
use crate::info::{INFO_FORMAT, INFO_FORMAT_VERSION};

/// OP5: `204` while the accept loop runs; no authentication, no database.
pub(super) async fn health() -> StatusCode {
    StatusCode::NO_CONTENT
}

/// OP7: `204`, or `503` with `shutting_down` while the server drains or
/// after it lost its lease (SD6), or holds `jobs.abandoned_max` jobs given
/// up in calls that never returned (JS8, `stuck`: restart it), or
/// `catalog_unavailable` with its reason while the catalog cannot be
/// opened. A stale catalog is ready.
pub(super) async fn ready(State(shared): State<Arc<Shared>>) -> Result<StatusCode, ApiError> {
    if shared.draining.load(Ordering::SeqCst) {
        return Err(shutting_down("draining"));
    }
    if !shared.lease_ok.load(Ordering::SeqCst) {
        return Err(shutting_down("lease"));
    }
    if shared.jobs.stuck() {
        return Err(shutting_down("stuck"));
    }
    let checked = Arc::clone(&shared);
    blocking(move || {
        let holds = checked.db.holds_lease(&checked.holder).unwrap_or(false);
        if !holds {
            return Err(shutting_down("lease"));
        }
        checked.catalog.open().map(drop).map_err(|unavailable| {
            tracing::warn!(error = %unavailable.log, "the catalog is unavailable");
            ApiError::from(unavailable)
        })
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

fn shutting_down(reason: &'static str) -> ApiError {
    let detail = match reason {
        "draining" => "the server is draining: send the request to another server or later",
        "stuck" => "jobs are stuck in calls that never returned: the server needs a restart",
        _ => "the server no longer holds its state directory's lease",
    };
    ApiError::new(ErrorCode::ShuttingDown, detail)
        .with("reason", reason)
        .retry_after(5)
}

/// DC11: `fitment.info`, for a reader (AU5), the catalog read in one
/// snapshot on the blocking pool (SV3).
pub(super) async fn info(
    State(shared): State<Arc<Shared>>,
    _: Reader,
) -> Result<Response, ApiError> {
    #[cfg(feature = "test-support")]
    if let Some(planted) = shared.hooks.reached("info").await {
        return Ok(axum::response::IntoResponse::into_response((
            StatusCode::OK,
            planted,
        )));
    }
    let catalog = catalog_info(&shared).await?;
    let kernel = crate::matching::kernel(&shared.verifier());
    let document = crate::info::info(&shared.config, kernel, catalog);
    Ok(json::document(INFO_FORMAT, INFO_FORMAT_VERSION, &document))
}

/// DC11: the catalog's generation, counts and flags, read on a pooled
/// reader behind a read slot (CX1), with whether a reindex is queued or
/// running (M6-28) and whether a lane's queue is full (JS5).
///
/// # Errors
///
/// The catalog's, mapped (CX7); `server.db`'s.
pub(super) async fn catalog_info(
    shared: &Arc<Shared>,
) -> Result<crate::catalog::CatalogInfo, ApiError> {
    let read = Arc::clone(shared);
    let mut catalog = crate::http::catalog_read(shared, move || {
        #[cfg(feature = "test-support")]
        read.hooks.blocking("info.blocking");
        let mut info = read.catalog.info()?;
        info.state.reindex = read
            .db
            .active_reindex()
            .map_err(|e| crate::http::db_error(&e))?;
        Ok(info)
    })
    .await?;
    catalog.state.queues_full = shared.jobs.queues_full();
    Ok(catalog)
}

/// VN7: the committed OpenAPI document, the bytes compiled in.
pub(super) async fn openapi() -> Response {
    json::bytes(
        StatusCode::OK,
        json::JSON,
        None,
        Bytes::from_static(crate::OPENAPI),
    )
}
