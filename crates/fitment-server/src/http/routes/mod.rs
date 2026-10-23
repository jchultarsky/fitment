//! The routes of section 7, one module per resource.

mod assemblies;
mod auth;
mod catalog;
mod features;
pub(crate) mod jobs;
mod matches;
mod server;
mod sockets;
mod uploads;

use std::sync::Arc;

use axum::http::{HeaderValue, header};
use axum::response::Response;
use axum::routing::{MethodRouter, on};

use super::method_filter;
use super::operations::Operation;
use crate::Shared;
use crate::auth::documents::{FORMAT_VERSION, PAGE_FORMAT, Page};
use crate::http::error::{ApiError, ErrorCode};
use crate::http::extract::{self, Principal};
use crate::http::operations::Role as Needed;
use crate::http::{blocking, db_error, json, query};

/// The handler of `op`, by its id.
///
/// # Panics
///
/// For an operation without a handler: a table entry with no route is a
/// bug the router's tests find at once (XH14).
#[allow(clippy::panic)]
pub(super) fn handler(op: &Operation) -> MethodRouter<Arc<Shared>> {
    let filter = method_filter(op.method);
    match op.id {
        "health" => on(filter, server::health),
        "ready" => on(filter, server::ready),
        "info" => on(filter, server::info),
        "openapi" => on(filter, server::openapi),
        "login" => on(filter, auth::login),
        "me" => on(filter, auth::me),
        "password" => on(filter, auth::password),
        "tokens_list" => on(filter, auth::tokens_list),
        "tokens_mint" => on(filter, auth::tokens_mint),
        "tokens_revoke" => on(filter, auth::tokens_revoke),
        "users_list" => on(filter, auth::users_list),
        "users_create" => on(filter, auth::users_create),
        "users_update" => on(filter, auth::users_update),
        "uploads_put" => on(filter, uploads::put),
        "uploads_post" => on(filter, uploads::post),
        "uploads_head" => on(filter, uploads::head),
        "uploads_get" => on(filter, uploads::get),
        "uploads_list" => on(filter, uploads::list),
        "uploads_delete" => on(filter, uploads::delete),
        "catalog_info" => on(filter, catalog::info),
        "catalog_list" => on(filter, catalog::list),
        "catalog_ingest" => on(filter, catalog::ingest),
        "catalog_get" => on(filter, catalog::get),
        "catalog_features" => on(filter, catalog::features),
        "catalog_step" => on(filter, catalog::step),
        "catalog_replace" => on(filter, catalog::replace),
        "catalog_delete" => on(filter, catalog::delete),
        "catalog_delete_many" => on(filter, catalog::delete_many),
        "catalog_reindex" => on(filter, catalog::reindex),
        "catalog_verify" => on(filter, catalog::verify),
        "catalog_gc" => on(filter, catalog::gc),
        "catalog_prune" => on(filter, catalog::prune),
        "assemblies_create" => on(filter, assemblies::create),
        "assemblies_list" => on(filter, assemblies::list),
        "assemblies_get" => on(filter, assemblies::get),
        "assemblies_parts" => on(filter, assemblies::parts),
        "assemblies_step" => on(filter, assemblies::step),
        "assemblies_sockets" => on(filter, assemblies::sockets),
        "assemblies_delete" => on(filter, assemblies::delete),
        "sockets_create" => on(filter, sockets::create),
        "sockets_list" => on(filter, sockets::list),
        "sockets_get" => on(filter, sockets::get),
        "sockets_revisions" => on(filter, sockets::revisions),
        "sockets_delete" => on(filter, sockets::delete),
        "sockets_import" => on(filter, sockets::import),
        "sockets_review" => on(filter, sockets::review),
        "matches_create" => on(filter, matches::create),
        "matches_list" => on(filter, matches::list),
        "matches_get" => on(filter, matches::get),
        "matches_document" => on(filter, matches::document),
        "matches_candidates" => on(filter, matches::candidates),
        "features_create" => on(filter, features::create),
        "jobs_list" => on(filter, jobs::list),
        "jobs_get" => on(filter, jobs::get),
        "jobs_events" => on(filter, jobs::events),
        "jobs_document" => on(filter, jobs::document),
        "jobs_cancel" => on(filter, jobs::cancel),
        "jobs_pin" => on(filter, jobs::pin),
        "jobs_delete" => on(filter, jobs::delete),
        other => panic!("OPERATIONS names {other}, which has no handler"),
    }
}

/// A page's response: `fitment.page`, and `Link: <next>; rel="next"`
/// (VN3).
pub(super) fn page<T: serde::Serialize>(page: &Page<T>) -> Response {
    let mut response = json::document(PAGE_FORMAT, FORMAT_VERSION, page);
    if let Some(value) = page
        .next
        .as_deref()
        .and_then(|next| HeaderValue::from_str(&format!("<{next}>; rel=\"next\"")).ok())
    {
        response.headers_mut().insert(header::LINK, value);
    }
    response
}

/// The next page's URL, if `rows` holds one more than `limit`.
pub(super) fn next_url<R>(
    base: &str,
    rows: &mut Vec<R>,
    limit: u32,
    key: impl Fn(&R) -> &str,
    extra: &str,
) -> Option<String> {
    let limit = usize::try_from(limit).unwrap_or(usize::MAX);
    if rows.len() <= limit {
        return None;
    }
    rows.truncate(limit);
    let last = rows.last().map(key)?;
    Some(format!(
        "{base}?after={}&limit={limit}{extra}",
        query::cursor(last)
    ))
}

/// AU8: whose resources a read names: the caller's, or with `?owner=` an
/// admin's choice (another's for an admin only, `403` below; an unknown
/// user `404`). Returns the user's key and the query's `&owner=` for a
/// next page.
pub(super) async fn owner(
    shared: &Arc<Shared>,
    caller: &Principal,
    params: &std::collections::BTreeMap<String, String>,
) -> Result<(i64, String), ApiError> {
    owner_of(shared, caller, params, false).await
}

/// [`owner`], where `system` lets an admin name the `system` user, whose
/// jobs nobody submitted (M6-56: `reindex_on_start`, the daily gc).
pub(super) async fn owner_of(
    shared: &Arc<Shared>,
    caller: &Principal,
    params: &std::collections::BTreeMap<String, String>,
    system: bool,
) -> Result<(i64, String), ApiError> {
    match params.get("owner") {
        Some(id) if *id != caller.user.id => {
            if !caller.effective.permits(Needed::Admin) {
                return Err(extract::forbidden(Needed::Admin, caller.effective));
            }
            let (reading, id) = (Arc::clone(shared), id.clone());
            let extra = format!("&owner={id}");
            let found = blocking(move || {
                if let Some(user) = reading.db.person_by_id(&id).map_err(|e| db_error(&e))? {
                    return Ok(Some(user.key));
                }
                let is_system = system && reading.db.system_user().map_err(|e| db_error(&e))? == id;
                if is_system {
                    return reading.db.system_key().map(Some).map_err(|e| db_error(&e));
                }
                Ok(None)
            })
            .await?;
            let key = found.ok_or_else(|| {
                ApiError::new(ErrorCode::NotFound, "no user of that id is yours to see")
            })?;
            Ok((key, extra))
        }
        Some(id) => Ok((caller.user.key, format!("&owner={id}"))),
        None => Ok((caller.user.key, String::new())),
    }
}

/// UP3, UP5: the length of bytes `owner` may name in a job: an unexpired
/// upload of theirs, or an input of a retained match record; else `404
/// upload_not_found`, never mid-job.
pub(super) async fn nameable(
    shared: &Arc<Shared>,
    owner: i64,
    sha256: &str,
) -> Result<u64, ApiError> {
    let (reading, sha) = (Arc::clone(shared), sha256.to_owned());
    blocking(move || {
        let now = reading.ids.now_ms();
        reading
            .db
            .upload(owner, &sha, now)
            .map_err(|e| db_error(&e))
    })
    .await?
    .map(|holding| holding.bytes)
    .ok_or_else(upload_not_found)
}

/// `404 upload_not_found` (UP3).
pub(super) fn upload_not_found() -> ApiError {
    ApiError::new(
        ErrorCode::UploadNotFound,
        "no upload of these bytes is yours: upload them (PUT /v1/uploads/sha256:HEX)",
    )
}
