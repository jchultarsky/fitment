//! `OPERATIONS` (docs/research/m6-api.md, VN7, AU5, XH14): every route of
//! the router, with its method, path, role, mode, body, responses and
//! error codes. The router is built from this table alone, and the OpenAPI
//! document is generated from it, so neither can hold a route the other
//! lacks.

use super::error::ErrorCode;

/// The least role an operation needs (AU5); `None` for the four routes
/// that need no authentication.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Role {
    /// No authentication.
    None,
    /// Reads the catalog and works on the caller's own resources.
    Reader,
    /// Also changes the catalog.
    Writer,
    /// Also maintains it and manages users.
    Admin,
}

impl Role {
    /// `x-fitment-role`'s value.
    pub fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Reader => "reader",
            Self::Writer => "writer",
            Self::Admin => "admin",
        }
    }
}

/// Whether an operation answers in the request or creates a job (M6-1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Answers in the request.
    Sync,
    /// Creates a job.
    Job,
    /// A stream (NDJSON).
    Stream,
}

impl Mode {
    /// `x-fitment-mode`'s value.
    pub fn name(self) -> &'static str {
        match self {
            Self::Sync => "sync",
            Self::Job => "job",
            Self::Stream => "stream",
        }
    }
}

/// What a request carries (HX2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestBody {
    /// Nothing: a body is `413 body_too_large`.
    None,
    /// A JSON body of at most `limits.max_json_kib` (VN6), of the request
    /// schema named (a component of the OpenAPI document).
    Json {
        /// The component's name.
        schema: &'static str,
    },
}

/// One successful response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Answer {
    /// The status.
    pub status: u16,
    /// The body's media type, if any.
    pub media: Option<&'static str>,
    /// The body's format, a schema of the OpenAPI document's components,
    /// if it is one of fitment's documents.
    pub format: Option<&'static str>,
    /// For the OpenAPI document.
    pub description: &'static str,
}

/// One operation of the API.
#[derive(Debug, Clone, Copy)]
pub struct Operation {
    /// `operationId`, also the goldens' directory (VT6).
    pub id: &'static str,
    /// The method, upper case.
    pub method: &'static str,
    /// The path, `/v1/…`.
    pub path: &'static str,
    /// For the OpenAPI document.
    pub summary: &'static str,
    /// The least role (AU5), `x-fitment-role`.
    pub role: Role,
    /// Fields that raise the role (AU5), `x-fitment-role-fields`.
    pub role_fields: &'static [(&'static str, Role)],
    /// `x-fitment-mode`.
    pub mode: Mode,
    /// What the request carries.
    pub body: RequestBody,
    /// The query parameters it takes (VN3, AU8); any other is refused.
    pub query: &'static [&'static str],
    /// The successful responses.
    pub answers: &'static [Answer],
    /// The error codes it can answer.
    pub codes: &'static [ErrorCode],
}

use ErrorCode as E;

/// Every operation, in the order of section 7 (VN7, XH14).
pub const OPERATIONS: [Operation; 13] = [
    Operation {
        id: "info",
        method: "GET",
        path: "/v1/info",
        summary: "The server's versions, limits, fit verifier and catalog state (DC11)",
        role: Role::Reader,
        role_fields: &[],
        mode: Mode::Sync,
        body: RequestBody::None,
        query: &[],
        answers: &[Answer {
            status: 200,
            media: Some(super::json::JSON),
            format: Some(crate::info::INFO_FORMAT),
            description: "fitment.info",
        }],
        codes: &[
            E::BadRequest,
            E::Unauthenticated,
            E::RateLimited,
            E::BodyTooLarge,
            E::NotAcceptable,
            E::CatalogUnavailable,
            E::CatalogBusy,
            E::CatalogDamaged,
            E::IoError,
            E::RequestTimeout,
            E::Internal,
        ],
    },
    Operation {
        id: "health",
        method: "GET",
        path: "/v1/health",
        summary: "Liveness: 204 while the process runs (OP5)",
        role: Role::None,
        role_fields: &[],
        mode: Mode::Sync,
        body: RequestBody::None,
        query: &[],
        answers: &[Answer {
            status: 204,
            media: None,
            format: None,
            description: "the process runs",
        }],
        codes: &[
            E::BadRequest,
            E::BodyTooLarge,
            E::RequestTimeout,
            E::Internal,
        ],
    },
    Operation {
        id: "ready",
        method: "GET",
        path: "/v1/ready",
        summary: "Readiness: 204, or 503 while the catalog cannot be opened or the server drains (OP7)",
        role: Role::None,
        role_fields: &[],
        mode: Mode::Sync,
        body: RequestBody::None,
        query: &[],
        answers: &[Answer {
            status: 204,
            media: None,
            format: None,
            description: "ready",
        }],
        codes: &[
            E::BadRequest,
            E::BodyTooLarge,
            E::CatalogUnavailable,
            E::ShuttingDown,
            E::RequestTimeout,
            E::Internal,
        ],
    },
    Operation {
        id: "openapi",
        method: "GET",
        path: "/v1/openapi.json",
        summary: "This OpenAPI document, the committed bytes (VN7)",
        role: Role::None,
        role_fields: &[],
        mode: Mode::Sync,
        body: RequestBody::None,
        query: &[],
        answers: &[Answer {
            status: 200,
            media: Some(super::json::JSON),
            format: None,
            description: "the OpenAPI 3.1 document",
        }],
        codes: &[
            E::BadRequest,
            E::BodyTooLarge,
            E::NotAcceptable,
            E::RequestTimeout,
            E::Internal,
        ],
    },
    AUTH[0],
    AUTH[1],
    AUTH[2],
    AUTH[3],
    AUTH[4],
    AUTH[5],
    AUTH[6],
    AUTH[7],
    AUTH[8],
];

const fn answer(status: u16, format: Option<&'static str>, description: &'static str) -> Answer {
    Answer {
        status,
        media: match format {
            Some(_) => Some(super::json::JSON),
            None => None,
        },
        format,
        description,
    }
}

use crate::auth::documents::{PAGE_FORMAT, TOKEN_FORMAT, USER_FORMAT};

/// Section 7.10's routes (AU1 to AU10).
const AUTH: [Operation; 9] = [
    Operation {
        id: "login",
        method: "POST",
        path: "/v1/auth/login",
        summary: "A token from a username and password, its secret shown once (AU2, AU7, AU10)",
        role: Role::None,
        role_fields: &[],
        mode: Mode::Sync,
        body: RequestBody::Json {
            schema: "LoginRequest",
        },
        query: &[],
        answers: &[answer(
            201,
            Some(TOKEN_FORMAT),
            "fitment.token, with its secret",
        )],
        codes: &[
            E::BadRequest,
            E::InvalidParameter,
            E::NotAcceptable,
            E::TooManyTokens,
            E::BodyTooLarge,
            E::UnsupportedMediaType,
            E::RateLimited,
            E::RequestTimeout,
            E::IoError,
            E::Internal,
            E::InvalidCredentials,
        ],
    },
    Operation {
        id: "me",
        method: "GET",
        path: "/v1/auth/me",
        summary: "The caller, with the presenting token's id and effective role (AU5)",
        role: Role::Reader,
        role_fields: &[],
        mode: Mode::Sync,
        body: RequestBody::None,
        query: &[],
        answers: &[answer(200, Some(USER_FORMAT), "fitment.user")],
        codes: &[
            E::BadRequest,
            E::NotAcceptable,
            E::BodyTooLarge,
            E::RateLimited,
            E::RequestTimeout,
            E::IoError,
            E::Internal,
            E::Unauthenticated,
        ],
    },
    Operation {
        id: "password",
        method: "POST",
        path: "/v1/auth/password",
        summary: "Changes the caller's password; every other token of theirs is revoked (AU1, AU4)",
        role: Role::Reader,
        role_fields: &[],
        mode: Mode::Sync,
        body: RequestBody::Json {
            schema: "PasswordRequest",
        },
        query: &[],
        answers: &[answer(204, None, "changed")],
        codes: &[
            E::BadRequest,
            E::InvalidParameter,
            E::BodyTooLarge,
            E::UnsupportedMediaType,
            E::RateLimited,
            E::RequestTimeout,
            E::IoError,
            E::Internal,
            E::Unauthenticated,
            E::InvalidCredentials,
        ],
    },
    Operation {
        id: "tokens_list",
        method: "GET",
        path: "/v1/auth/tokens",
        summary: "A page of the caller's tokens, never a secret (AU3); an admin's ?owner= reads another's (AU8)",
        role: Role::Reader,
        role_fields: &[("owner", Role::Admin)],
        mode: Mode::Sync,
        body: RequestBody::None,
        query: &["after", "limit", "owner"],
        answers: &[answer(
            200,
            Some(PAGE_FORMAT),
            "fitment.page of fitment.token",
        )],
        codes: &[
            E::BadRequest,
            E::InvalidParameter,
            E::NotFound,
            E::NotAcceptable,
            E::BodyTooLarge,
            E::RateLimited,
            E::RequestTimeout,
            E::IoError,
            E::Internal,
            E::Unauthenticated,
            E::Forbidden,
        ],
    },
    Operation {
        id: "tokens_mint",
        method: "POST",
        path: "/v1/auth/tokens",
        summary: "Mints a token within the presenting one: role and expiry at most its own (AU10)",
        role: Role::Reader,
        role_fields: &[],
        mode: Mode::Sync,
        body: RequestBody::Json {
            schema: "MintRequest",
        },
        query: &[],
        answers: &[answer(
            201,
            Some(TOKEN_FORMAT),
            "fitment.token, with its secret",
        )],
        codes: &[
            E::BadRequest,
            E::InvalidParameter,
            E::NotAcceptable,
            E::TooManyTokens,
            E::BodyTooLarge,
            E::UnsupportedMediaType,
            E::RateLimited,
            E::RequestTimeout,
            E::IoError,
            E::Internal,
            E::Unauthenticated,
        ],
    },
    Operation {
        id: "tokens_revoke",
        method: "DELETE",
        path: "/v1/auth/tokens/{id}",
        summary: "Revokes a token and every token minted from it; `current` is the presenting one (AU4)",
        role: Role::Reader,
        role_fields: &[],
        mode: Mode::Sync,
        body: RequestBody::None,
        query: &[],
        answers: &[answer(204, None, "revoked")],
        codes: &[
            E::BadRequest,
            E::NotFound,
            E::BodyTooLarge,
            E::RateLimited,
            E::RequestTimeout,
            E::IoError,
            E::Internal,
            E::Unauthenticated,
        ],
    },
    Operation {
        id: "users_list",
        method: "GET",
        path: "/v1/auth/users",
        summary: "A page of the users, by folded username (AU5)",
        role: Role::Admin,
        role_fields: &[],
        mode: Mode::Sync,
        body: RequestBody::None,
        query: &["after", "limit"],
        answers: &[answer(
            200,
            Some(PAGE_FORMAT),
            "fitment.page of fitment.user",
        )],
        codes: &[
            E::BadRequest,
            E::InvalidParameter,
            E::NotAcceptable,
            E::BodyTooLarge,
            E::RateLimited,
            E::RequestTimeout,
            E::IoError,
            E::Internal,
            E::Unauthenticated,
            E::Forbidden,
        ],
    },
    Operation {
        id: "users_create",
        method: "POST",
        path: "/v1/auth/users",
        summary: "Makes a user, with a password or token-only (AU1, AU5)",
        role: Role::Admin,
        role_fields: &[],
        mode: Mode::Sync,
        body: RequestBody::Json {
            schema: "CreateUserRequest",
        },
        query: &[],
        answers: &[answer(201, Some(USER_FORMAT), "fitment.user")],
        codes: &[
            E::BadRequest,
            E::InvalidParameter,
            E::NotAcceptable,
            E::UsernameTaken,
            E::BodyTooLarge,
            E::UnsupportedMediaType,
            E::RateLimited,
            E::RequestTimeout,
            E::IoError,
            E::Internal,
            E::Unauthenticated,
            E::Forbidden,
        ],
    },
    Operation {
        id: "users_update",
        method: "PATCH",
        path: "/v1/auth/users/{id}",
        summary: "Sets a user's role or disabled flag, in force at the next request (AU4)",
        role: Role::Admin,
        role_fields: &[],
        mode: Mode::Sync,
        body: RequestBody::Json {
            schema: "UpdateUserRequest",
        },
        query: &[],
        answers: &[answer(200, Some(USER_FORMAT), "fitment.user")],
        codes: &[
            E::BadRequest,
            E::NotFound,
            E::NotAcceptable,
            E::BodyTooLarge,
            E::UnsupportedMediaType,
            E::RateLimited,
            E::RequestTimeout,
            E::IoError,
            E::Internal,
            E::Unauthenticated,
            E::Forbidden,
        ],
    },
];
