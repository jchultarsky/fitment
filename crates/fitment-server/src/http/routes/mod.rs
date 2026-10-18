//! The routes of section 7, one module per resource.

mod auth;
mod server;

use std::sync::Arc;

use axum::routing::{MethodRouter, on};

use super::method_filter;
use super::operations::Operation;
use crate::Shared;

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
        other => panic!("OPERATIONS names {other}, which has no handler"),
    }
}
