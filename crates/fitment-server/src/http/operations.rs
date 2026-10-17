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
    /// The successful responses.
    pub answers: &'static [Answer],
    /// The error codes it can answer.
    pub codes: &'static [ErrorCode],
}

use ErrorCode as E;

/// Every operation, in the order of section 7 (VN7, XH14).
pub const OPERATIONS: [Operation; 4] = [
    Operation {
        id: "info",
        method: "GET",
        path: "/v1/info",
        summary: "The server's versions, limits, fit verifier and catalog state (DC11)",
        role: Role::Reader,
        role_fields: &[],
        mode: Mode::Sync,
        body: RequestBody::None,
        answers: &[Answer {
            status: 200,
            media: Some(super::json::JSON),
            format: Some(crate::info::INFO_FORMAT),
            description: "fitment.info",
        }],
        codes: &[
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
        answers: &[Answer {
            status: 204,
            media: None,
            format: None,
            description: "the process runs",
        }],
        codes: &[E::BodyTooLarge, E::RequestTimeout, E::Internal],
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
        answers: &[Answer {
            status: 204,
            media: None,
            format: None,
            description: "ready",
        }],
        codes: &[
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
        answers: &[Answer {
            status: 200,
            media: Some(super::json::JSON),
            format: None,
            description: "the OpenAPI 3.1 document",
        }],
        codes: &[
            E::BodyTooLarge,
            E::NotAcceptable,
            E::RequestTimeout,
            E::Internal,
        ],
    },
];
