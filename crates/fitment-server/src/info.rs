//! `fitment.info` (docs/research/m6-api.md, DC11): the server's version,
//! every version constant, the API's version, the fit verifier, the
//! catalog's state and the limits. A client checks it before comparing
//! documents across servers. It holds no time and no path (DC9).

use std::collections::BTreeMap;

use serde::Serialize;

use crate::catalog::{self, CatalogInfo};
use crate::config::Config;
use crate::http::error::{ERROR_FORMAT, ERROR_FORMAT_VERSION};

/// The document's format name.
pub const INFO_FORMAT: &str = "fitment.info";

/// Its version (DC9: may grow within v1).
pub const INFO_FORMAT_VERSION: u32 = 1;

/// The API's major version: the `/v1` in every path (VN1).
pub const API_MAJOR: u32 = 1;

/// The API's minor version, raised with each addition within v1 (VN1);
/// the OpenAPI document's `info.version` is `1.<minor>` (VN7).
pub const API_MINOR: u32 = 1;

/// The OpenAPI document's `info.version`.
pub fn api_version() -> String {
    format!("{API_MAJOR}.{API_MINOR}")
}

/// `fitment.info` v1.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Info {
    /// `fitment.info`.
    pub format: &'static str,
    /// 1.
    pub format_version: u32,
    /// The server's name and version (SV5).
    pub server: ServerVersion,
    /// The API's version (VN1).
    pub api: Api,
    /// The build's version constants (VN2).
    pub versions: Versions,
    /// Each document format's version, by its name's order (VN2).
    pub formats: Vec<FormatVersion>,
    /// The fit verifier's kernel and version; null until M7 (SN11, OP9).
    pub fit_verifier: Option<Kernel>,
    /// The catalog's generation, counts and flags.
    pub catalog: CatalogInfo,
    /// The limits a client needs (section 13.1).
    pub limits: Limits,
}

/// A format and its version (VN2).
#[derive(Debug, Clone, Copy, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct FormatVersion {
    /// The format's name, as its documents' `format`.
    pub format: &'static str,
    /// Its version, as their `format_version`.
    pub format_version: u32,
}

/// The server's name and version.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ServerVersion {
    /// `fitment-server`.
    pub name: &'static str,
    /// The workspace's version, the CLI's too (SV5, DC8).
    pub version: &'static str,
}

/// `{major, minor}` (VN1).
#[derive(Debug, Clone, Copy, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Api {
    /// 1.
    pub major: u32,
    /// Raised with each addition.
    pub minor: u32,
}

/// The build's version constants (VN2): documents record theirs.
#[derive(Debug, Clone, Copy, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[allow(missing_docs)] // each is fitment-core's or fitment-catalog's constant of the same name
pub struct Versions {
    pub extractor_version: u32,
    pub socket_rules_version: u32,
    pub signature_version: u32,
    pub match_rules_version: u32,
    pub properties_version: u32,
    /// The catalog's schema this build reads.
    pub catalog_schema: i64,
    /// `server.db`'s schema.
    pub server_db_schema: i64,
}

/// A fit verifier's kernel and version (MF6).
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Kernel {
    /// The kernel.
    pub kernel: String,
    /// Its version.
    pub version: String,
}

/// The limits a client needs (DC11).
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[allow(missing_docs)] // each is the configuration key of the same name (OP3)
pub struct Limits {
    pub max_upload_mib: u64,
    pub upload_min_kib_s: u64,
    pub max_socket_mib: u64,
    pub max_json_kib: u64,
    pub max_candidates: u64,
    pub max_catalog_candidates: u64,
    /// A page's size when `limit` is not given (VN3).
    pub page_default: u32,
    pub page_max: u32,
    pub wait_max_seconds: u32,
    pub require_reviewed_floor: bool,
    pub upload_ttl_hours: u64,
    pub max_live_tokens: u32,
}

/// The build's constants.
pub fn versions() -> Versions {
    Versions {
        extractor_version: fitment_core::EXTRACTOR_VERSION,
        socket_rules_version: fitment_core::SOCKET_RULES_VERSION,
        signature_version: fitment_core::SIGNATURE_VERSION,
        match_rules_version: fitment_core::MATCH_RULES_VERSION,
        properties_version: fitment_core::PROPERTIES_VERSION,
        catalog_schema: catalog::SCHEMA_VERSION,
        server_db_schema: crate::state::db::SCHEMA_VERSION,
    }
}

/// Every format the server serves, with its version, by name (VN2, DC9).
pub fn formats() -> Vec<FormatVersion> {
    BTreeMap::from([
        (
            fitment_core::FEATURES_FORMAT,
            fitment_core::FEATURES_FORMAT_VERSION,
        ),
        (
            fitment_core::PARTS_FORMAT,
            fitment_core::PARTS_FORMAT_VERSION,
        ),
        (
            fitment_core::SOCKET_FORMAT,
            fitment_core::SOCKET_FORMAT_VERSION,
        ),
        (
            fitment_core::MATCH_FORMAT,
            fitment_core::MATCH_FORMAT_VERSION,
        ),
        (catalog::FORMAT, catalog::FORMAT_VERSION),
        (INFO_FORMAT, INFO_FORMAT_VERSION),
        (ERROR_FORMAT, ERROR_FORMAT_VERSION),
        (
            crate::auth::documents::PAGE_FORMAT,
            crate::auth::documents::FORMAT_VERSION,
        ),
        (
            crate::auth::documents::TOKEN_FORMAT,
            crate::auth::documents::FORMAT_VERSION,
        ),
        (
            crate::auth::documents::USER_FORMAT,
            crate::auth::documents::FORMAT_VERSION,
        ),
    ])
    .into_iter()
    .map(|(format, format_version)| FormatVersion {
        format,
        format_version,
    })
    .collect()
}

/// The document for `config`, the verifier's kernel and the catalog.
pub fn info(config: &Config, kernel: Option<(String, String)>, catalog: CatalogInfo) -> Info {
    let limits = &config.limits;
    Info {
        format: INFO_FORMAT,
        format_version: INFO_FORMAT_VERSION,
        server: ServerVersion {
            name: "fitment-server",
            version: env!("CARGO_PKG_VERSION"),
        },
        api: Api {
            major: API_MAJOR,
            minor: API_MINOR,
        },
        versions: versions(),
        formats: formats(),
        fit_verifier: kernel.map(|(kernel, version)| Kernel { kernel, version }),
        catalog,
        limits: Limits {
            max_upload_mib: limits.max_upload_mib,
            upload_min_kib_s: limits.upload_min_kib_s,
            max_socket_mib: limits.max_socket_mib,
            max_json_kib: limits.max_json_kib,
            max_candidates: limits.max_candidates,
            max_catalog_candidates: limits.max_catalog_candidates,
            page_default: 100.min(limits.page_max),
            page_max: limits.page_max,
            wait_max_seconds: limits.wait_max_seconds,
            require_reviewed_floor: config.matching.require_reviewed_floor,
            upload_ttl_hours: limits.upload_ttl_hours,
            max_live_tokens: config.auth.max_live_tokens,
        },
    }
}
