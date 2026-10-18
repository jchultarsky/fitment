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

use crate::http::error::{ERROR_FORMAT, ERROR_FORMAT_VERSION, ErrorCode, Problem};
use crate::http::json::PROBLEM_JSON;
use crate::http::operations::{OPERATIONS, Operation};
use crate::info::{INFO_FORMAT, INFO_FORMAT_VERSION, Info, api_version};

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

/// Every format's schema, by its name (VN7, VT36).
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
    add(
        fitment_core::MATCH_FORMAT,
        schema_of::<fitment_core::MatchDocument>(
            fitment_core::MATCH_FORMAT,
            fitment_core::MATCH_FORMAT_VERSION,
            true,
        ),
    );
    add(crate::catalog::FORMAT, crate::catalog::schema());
    add(
        INFO_FORMAT,
        schema_of::<Info>(INFO_FORMAT, INFO_FORMAT_VERSION, false),
    );
    add(
        ERROR_FORMAT,
        schema_of::<Problem>(ERROR_FORMAT, ERROR_FORMAT_VERSION, false),
    );
    schemas
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
    json!({
        "operationId": op.id,
        "summary": op.summary,
        "x-fitment-role": op.role.name(),
        "x-fitment-role-fields": role_fields,
        "x-fitment-mode": op.mode.name(),
        "responses": responses,
    })
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
        "components": { "schemas": schemas() },
    })
}

/// The document as committed: pretty JSON with a final newline.
pub fn document() -> String {
    crate::http::json::pretty(&value())
}
