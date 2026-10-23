//! The OpenAPI 3.1 document (docs/research/m6-api.md, VN7, M6-6), under
//! the `schema` feature only: generated from `OPERATIONS` (method, path,
//! role as `x-fitment-role`, fields that raise it as
//! `x-fitment-role-fields`, mode as `x-fitment-mode`, responses and error
//! codes) and from schemars derives: the result documents' types behind
//! fitment-core's and fitment-catalog's `schema` features (CO4, CA6) and
//! the server's own. `tests/openapi.rs` compares it with the committed
//! `openapi.json`, which the server serves as bytes.
//!
//! Each format's schema is self-contained (its own `$defs`) with an `$id`
//! naming its format and version, so a validator resolves it alone. The
//! result documents are frozen and their schemas closed
//! (`additionalProperties: false` on every object); the API's own
//! envelopes may grow within v1 (VN1) and stay open.

use schemars::JsonSchema;
use schemars::generate::SchemaSettings;
use serde_json::{Map, Value, json};

use crate::assemblies::{ASSEMBLY_FORMAT, ASSEMBLY_FORMAT_VERSION, AssemblyDocument};
use crate::auth::documents::{
    FORMAT_VERSION as AUTH_FORMAT_VERSION, PAGE_FORMAT, Page, TOKEN_FORMAT, TokenDocument,
    USER_FORMAT, UserDocument,
};
use crate::auth::requests::{
    CreateUserRequest, LoginRequest, MintRequest, PasswordRequest, UpdateUserRequest,
};
use crate::http::error::{ERROR_FORMAT, ERROR_FORMAT_VERSION, ErrorCode, Problem};
use crate::http::json::{JSON, PROBLEM_JSON};
use crate::http::operations::{OPERATIONS, Operation, RequestBody, Role};
use crate::info::{INFO_FORMAT, INFO_FORMAT_VERSION, Info, api_version};
use crate::jobs::documents::{
    EVENT_FORMAT, EventDocument, FORMAT_VERSION as JOB_FORMAT_VERSION, JOB_FORMAT, JobDocument,
};
use crate::jobs::request::{
    AssemblyRequest, DeleteRequest, FeaturesRequest, GcRequest, ImportRequest, IngestRequest,
    MatchRequest, PinRequest, PruneRequest, ReindexRequest, ReplaceRequest, ReviewRequest,
    SocketRequest, VerifyRequest,
};
use crate::uploads::{UPLOAD_FORMAT, UPLOAD_FORMAT_VERSION, UPLOAD_TYPES, UploadDocument};

/// The base of every schema's `$id`: `<base><format>/<version>`.
pub const SCHEMA_BASE: &str = "https://fitment.dev/schemas/";

/// The schema of `T` as written (`for_serialize`), self-contained, its
/// `$id` naming `format` and `version`.
fn schema_of<T: JsonSchema>(format: &str, version: u32, closed: bool) -> Value {
    let generator = SchemaSettings::draft2020_12()
        .for_serialize()
        .into_generator();
    let schema = generator.into_root_schema_for::<T>();
    let mut value = schema.to_value();
    if closed {
        close(&mut value);
    }
    if let Some(object) = value.as_object_mut() {
        object.insert(
            "$id".into(),
            format!("{SCHEMA_BASE}{format}/{version}").into(),
        );
    }
    value
}

/// Closes a frozen document's schema: an object schema with
/// `properties` gains `additionalProperties: false`, or, when serde's
/// `flatten` composed it with `anyOf`, `oneOf` or `allOf`,
/// `unevaluatedProperties: false`, its branches then left open as parts of
/// it. A frozen document holds exactly the fields its writer writes.
fn close(value: &mut Value) {
    close_part(value, false);
}

fn close_part(schema: &mut Value, part: bool) {
    let Value::Object(object) = schema else {
        return;
    };
    let composed = ["anyOf", "oneOf", "allOf"]
        .iter()
        .any(|k| object.contains_key(*k));
    let owns = object.contains_key("properties");
    if owns && !part && !object.contains_key("additionalProperties") {
        let key = if composed {
            "unevaluatedProperties"
        } else {
            "additionalProperties"
        };
        object.insert(key.into(), Value::Bool(false));
    }
    // Only subschemas: a `properties` or `$defs` map is a map of schemas,
    // never a schema itself, and `const`, `enum` or `default` are values.
    for (key, sub) in object.iter_mut() {
        match key.as_str() {
            "properties" | "$defs" | "definitions" | "patternProperties" => {
                if let Value::Object(map) = sub {
                    map.values_mut().for_each(|s| close_part(s, false));
                }
            }
            "anyOf" | "oneOf" | "allOf" => {
                if let Value::Array(branches) = sub {
                    for branch in branches {
                        close_part(branch, owns || part);
                    }
                }
            }
            "prefixItems" => {
                if let Value::Array(items) = sub {
                    for item in items {
                        close_part(item, false);
                    }
                }
            }
            "items" | "additionalProperties" | "unevaluatedProperties" | "not" => {
                close_part(sub, false);
            }
            _ => {}
        }
    }
}

/// `schema` with its `format` and `format_version` pinned by `const`, so
/// a document of one version never validates against another's (FM7,
/// the review of M7 PR 1b): an additive tightening.
fn pinned(mut schema: Value, format: &str, version: u32) -> Value {
    if let Some(properties) = schema.get_mut("properties").and_then(Value::as_object_mut) {
        for (key, value) in [
            ("format", json!(format)),
            ("format_version", json!(version)),
        ] {
            if let Some(Value::Object(property)) = properties.get_mut(key) {
                property.insert("const".into(), value);
            }
        }
    }
    schema
}

/// The schema's name for `version` of `format` beside its first:
/// `fitment.match.v2` (FM7). A component's name may not hold the `/` its
/// `$id` has.
pub fn versioned(format: &str, version: u32) -> String {
    format!("{format}.v{version}")
}

/// `fitment.match`'s schemas: v1, and v2 beside it (docs/research/m7-fit.md,
/// FM7), a run's with a fit verifier, closed, from its own types (FC7);
/// each pinned to its format and version.
fn match_schemas() -> [(String, Value); 2] {
    use fitment_core::{MATCH_FORMAT, MATCH_FORMAT_VERSION, MATCH_FORMAT_VERSION_FIT};
    [
        (
            MATCH_FORMAT.to_owned(),
            pinned(
                schema_of::<fitment_core::MatchDocument>(MATCH_FORMAT, MATCH_FORMAT_VERSION, true),
                MATCH_FORMAT,
                MATCH_FORMAT_VERSION,
            ),
        ),
        (
            versioned(MATCH_FORMAT, MATCH_FORMAT_VERSION_FIT),
            pinned(
                schema_of::<fitment_core::MatchDocumentV2>(
                    MATCH_FORMAT,
                    MATCH_FORMAT_VERSION_FIT,
                    true,
                ),
                MATCH_FORMAT,
                MATCH_FORMAT_VERSION_FIT,
            ),
        ),
    ]
}

/// Every format's schema, by its name (VN7, VT36): a format's first
/// version by the format's own name, a later one by [`versioned`].
pub fn schemas() -> Map<String, Value> {
    let mut schemas = Map::new();
    let mut add = |format: &str, schema: Value| {
        schemas.insert(format.to_owned(), schema);
    };
    add(
        fitment_core::FEATURES_FORMAT,
        schema_of::<fitment_core::FeaturesDocument>(
            fitment_core::FEATURES_FORMAT,
            fitment_core::FEATURES_FORMAT_VERSION,
            true,
        ),
    );
    add(
        fitment_core::PARTS_FORMAT,
        schema_of::<fitment_core::PartsDocument>(
            fitment_core::PARTS_FORMAT,
            fitment_core::PARTS_FORMAT_VERSION,
            true,
        ),
    );
    add(
        fitment_core::SOCKET_FORMAT,
        schema_of::<fitment_core::SocketDocument>(
            fitment_core::SOCKET_FORMAT,
            fitment_core::SOCKET_FORMAT_VERSION,
            true,
        ),
    );
    for (format, schema) in match_schemas() {
        add(&format, schema);
    }
    add(crate::catalog::FORMAT, crate::catalog::schema());
    add(
        crate::catalog::VERIFY_FORMAT,
        crate::catalog::verify_schema(),
    );
    add(crate::catalog::GC_FORMAT, crate::catalog::gc_schema());
    add(
        INFO_FORMAT,
        schema_of::<Info>(INFO_FORMAT, INFO_FORMAT_VERSION, false),
    );
    add(
        ERROR_FORMAT,
        schema_of::<Problem>(ERROR_FORMAT, ERROR_FORMAT_VERSION, false),
    );
    add(
        TOKEN_FORMAT,
        schema_of::<TokenDocument>(TOKEN_FORMAT, AUTH_FORMAT_VERSION, false),
    );
    add(
        USER_FORMAT,
        schema_of::<UserDocument>(USER_FORMAT, AUTH_FORMAT_VERSION, false),
    );
    add(
        PAGE_FORMAT,
        schema_of::<Page<Value>>(PAGE_FORMAT, AUTH_FORMAT_VERSION, false),
    );
    add(
        UPLOAD_FORMAT,
        schema_of::<UploadDocument>(UPLOAD_FORMAT, UPLOAD_FORMAT_VERSION, false),
    );
    add(
        JOB_FORMAT,
        schema_of::<JobDocument>(JOB_FORMAT, JOB_FORMAT_VERSION, false),
    );
    add(
        EVENT_FORMAT,
        schema_of::<EventDocument>(EVENT_FORMAT, JOB_FORMAT_VERSION, false),
    );
    add(
        ASSEMBLY_FORMAT,
        schema_of::<AssemblyDocument>(ASSEMBLY_FORMAT, ASSEMBLY_FORMAT_VERSION, false),
    );
    add(
        crate::runs::RUN_FORMAT,
        schema_of::<crate::runs::RunDocument>(
            crate::runs::RUN_FORMAT,
            crate::runs::RUN_FORMAT_VERSION,
            false,
        ),
    );
    schemas
}

/// The request bodies' schemas, by the name `OPERATIONS` gives them: closed,
/// since a request with an unknown field is refused (VN6).
pub fn request_schemas() -> Map<String, Value> {
    let mut schemas = Map::new();
    let mut add = |name: &str, mut schema: Value| {
        close(&mut schema);
        if let Some(object) = schema.as_object_mut() {
            object.insert("$id".into(), format!("{SCHEMA_BASE}requests/{name}").into());
        }
        schemas.insert(name.to_owned(), schema);
    };
    let requests: [(&str, Value); 19] = [
        ("LoginRequest", root::<LoginRequest>()),
        ("PasswordRequest", root::<PasswordRequest>()),
        ("MintRequest", root::<MintRequest>()),
        ("CreateUserRequest", root::<CreateUserRequest>()),
        ("UpdateUserRequest", root::<UpdateUserRequest>()),
        ("FeaturesRequest", root::<FeaturesRequest>()),
        ("AssemblyRequest", root::<AssemblyRequest>()),
        ("SocketRequest", root::<SocketRequest>()),
        ("PinRequest", root::<PinRequest>()),
        ("ImportRequest", root::<ImportRequest>()),
        ("ReviewRequest", root::<ReviewRequest>()),
        ("MatchRequest", root::<MatchRequest>()),
        ("IngestRequest", root::<IngestRequest>()),
        ("ReplaceRequest", root::<ReplaceRequest>()),
        ("ReindexRequest", root::<ReindexRequest>()),
        ("VerifyRequest", root::<VerifyRequest>()),
        ("GcRequest", root::<GcRequest>()),
        ("PruneRequest", root::<PruneRequest>()),
        ("DeleteRequest", root::<DeleteRequest>()),
    ];
    for (name, schema) in requests {
        add(name, schema);
    }
    schemas
}

/// A request body's schema as read (`for_deserialize`), whole.
fn root<T: JsonSchema>() -> Value {
    SchemaSettings::draft2020_12()
        .for_deserialize()
        .into_generator()
        .into_root_schema_for::<T>()
        .to_value()
}

/// What a query parameter means (VN3, AU8).
fn parameter(op: &str, name: &str) -> Value {
    let description = match name {
        "after" if op == "jobs_events" => {
            "the seq of the last persisted event read: replay after it (JB8)"
        }
        "after" if op == "catalog_list" => {
            "the cursor of the page to start after: the previous page's Link holds it (VN3)"
        }
        "after" => "the cursor of the page to start after: the previous page's `next` holds it",
        "limit" if op == "catalog_list" => {
            "entries per page: 100 by default, at most limits.page_max; `all`, the whole list, the CLI's bytes (VN3, HX8)"
        }
        "limit" => "items per page: 100 by default, at most limits.page_max",
        "owner" => "an admin's: the user whose items to list (AU8)",
        "kind" => "only jobs of this kind (JB1)",
        "state" => "only jobs in this state (JB3)",
        "wait" => "seconds to wait for the job to end, at most limits.wait_max_seconds (JB11)",
        "assembly" => "only the lines of this assembly, sha256:HEX",
        "socket" => "only the runs of this socket line, sk_…, or of one revision, sk_…@rN",
        "offset" => "the index of the first candidate of the page, in the document's order (DC12)",
        "status" => "only entries of this status: indexed, quarantined, stale or all (CL3)",
        _ => "",
    };
    json!({ "name": name, "in": "query", "required": false, "description": description,
            "schema": { "type": "string" } })
}

/// The path's `{name}` parameters, then the operation's query parameters.
fn parameters(op: &Operation) -> Vec<Value> {
    let mut out: Vec<Value> = op
        .path
        .split('/')
        .filter_map(|segment| segment.strip_prefix('{')?.strip_suffix('}'))
        .map(|name| {
            let mut parameter = json!({ "name": name, "in": "path", "required": true,
                                        "schema": { "type": "string" } });
            let description = match name {
                "upload" | "assembly" => {
                    Some("sha256: and the bytes' 64 hex digits, either case (HX5)")
                }
                "socket" => Some("sk_ and 20 hex digits, and @rN for one revision (RA4, RA5)"),
                "entry" => Some(
                    "a catalog reference: ID, ID@rN or sha256: and 12 to 64 hex digits (ID7, RA2)",
                ),
                "id" if op.path.starts_with("/v1/jobs") => {
                    Some("j_ and 26 base32 characters (M6-55)")
                }
                _ => None,
            };
            if let (Some(map), Some(description)) = (parameter.as_object_mut(), description) {
                map.insert("description".into(), description.into());
            }
            parameter
        })
        .collect();
    out.extend(op.query.iter().map(|name| parameter(op.id, name)));
    out
}

/// The catalog document's schema, closed (CA6).
pub(crate) fn closed_schema_of<T: JsonSchema>(format: &str, version: u32) -> Value {
    schema_of::<T>(format, version, true)
}

fn reference(format: &str) -> Value {
    json!({ "$ref": format!("#/components/schemas/{format}") })
}

/// One operation's object: its extensions, its answers and, per status,
/// the error codes it can answer.
fn operation(op: &Operation) -> Value {
    let mut responses = Map::new();
    for answer in op.answers {
        let mut response = Map::new();
        response.insert("description".into(), answer.description.into());
        if let Some(media) = answer.media {
            let schema = answer.format.map_or_else(|| json!({}), reference);
            response.insert("content".into(), json!({ media: { "schema": schema } }));
        }
        responses.insert(answer.status.to_string(), Value::Object(response));
    }
    let mut by_status: std::collections::BTreeMap<u16, Vec<&str>> =
        std::collections::BTreeMap::new();
    for code in op.codes {
        if let Some(status) = code.spec().status {
            by_status.entry(status).or_default().push(code.name());
        }
    }
    for (status, codes) in by_status {
        responses.insert(
            status.to_string(),
            json!({
                "description": codes.join(", "),
                "x-fitment-codes": codes,
                "content": { PROBLEM_JSON: { "schema": reference(ERROR_FORMAT) } },
            }),
        );
    }
    let role_fields: Vec<Value> = op
        .role_fields
        .iter()
        .map(|(field, role)| json!({ "field": field, "role": role.name() }))
        .collect();
    let mut object = json!({
        "operationId": op.id,
        "summary": op.summary,
        "x-fitment-role": op.role.name(),
        "x-fitment-role-fields": role_fields,
        "x-fitment-mode": op.mode.name(),
        "parameters": parameters(op),
        "security": if op.role == Role::None { json!([]) } else { json!([{ "bearer": [] }]) },
        "responses": responses,
    });
    let body = match op.body {
        RequestBody::None => None,
        RequestBody::Json { schema } => Some(json!({
            "required": true, "content": { JSON: { "schema": reference(schema) } } })),
        // UP2: raw bytes of any content, streamed; OpenAPI 3.1 states a
        // binary body by its media types alone.
        RequestBody::Upload => Some(json!({
            "required": true,
            "description": "the bytes, at most limits.max_upload_mib, streamed (UP2)",
            "content": UPLOAD_TYPES.iter().map(|t| ((*t).to_owned(), json!({})))
                .collect::<Map<String, Value>>(),
        })),
    };
    if let (Some(body), Some(map)) = (body, object.as_object_mut()) {
        map.insert("requestBody".into(), body);
    }
    object
}

/// The document's value.
pub fn value() -> Value {
    let mut paths = Map::new();
    for op in &OPERATIONS {
        let entry = paths
            .entry(op.path.to_owned())
            .or_insert_with(|| Value::Object(Map::new()));
        if let Some(item) = entry.as_object_mut() {
            item.insert(op.method.to_ascii_lowercase(), operation(op));
        }
    }
    let codes: Vec<&str> = ErrorCode::ALL.iter().map(|c| c.name()).collect();
    json!({
        "openapi": "3.1.0",
        "info": {
            "title": "fitment-server",
            "version": api_version(),
            "summary": "fitment's REST API: jobs over the catalog and the matcher",
            "license": { "name": "MIT", "identifier": "MIT" },
            "x-fitment-error-codes": codes,
        },
        "paths": paths,
        "components": {
            "schemas": schemas().into_iter().chain(request_schemas()).collect::<Map<String, Value>>(),
            "securitySchemes": {
                "bearer": {
                    "type": "http",
                    "scheme": "bearer",
                    "description": "a token: its prefix and 43 base64url characters (AU3)",
                },
            },
        },
    })
}

/// The document as committed: pretty JSON with a final newline.
pub fn document() -> String {
    crate::http::json::pretty(&value())
}
