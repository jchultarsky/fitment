//! The routes of section 7, one module per resource.

mod auth;
mod server;
mod uploads;

use std::sync::Arc;

use axum::http::{HeaderValue, header};
use axum::response::Response;
use axum::routing::{MethodRouter, on};

use super::method_filter;
use super::operations::Operation;
use crate::Shared;
use crate::auth::documents::{FORMAT_VERSION, PAGE_FORMAT, Page};
use crate::http::{json, query};

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
