//! A small JSON Schema validator for the keywords schemars emits
//! (docs/research/m6-api.md, VT36, R14, question 9): `type`, `properties`,
//! `required`, `additionalProperties`, `items`, `prefixItems`, `minItems`,
//! `maxItems`, `enum`, `const`, `oneOf`, `anyOf`, `allOf`, `$ref` (into
//! the root's `$defs`), `minimum`, `maximum`, `exclusiveMinimum`,
//! `exclusiveMaximum`, `minLength`, `maxLength` and
//! `unevaluatedProperties` (properties evaluated by `properties`,
//! `additionalProperties` and the passing branches of `allOf`, `anyOf`
//! and `oneOf`, through `$ref`s). Annotations are
//! ignored; `pattern` and `format` are not checked (no regular
//! expressions here). Any other keyword is reported, so a schema this
//! validator cannot judge never passes unread.

use serde_json::{Map, Value};

/// Keywords that state nothing a document must satisfy.
const ANNOTATIONS: [&str; 12] = [
    "$schema",
    "$id",
    "$defs",
    "$comment",
    "title",
    "description",
    "default",
    "examples",
    "deprecated",
    "readOnly",
    "writeOnly",
    "format",
];

/// Every way `document` fails `schema` (the root, whose `$defs` the
/// `$ref`s name), each with its JSON pointer; empty when it passes.
pub fn validate(schema: &Value, document: &Value) -> Vec<String> {
    let mut problems = Vec::new();
    check(schema, schema, document, "", &mut problems);
    problems
}

fn check(root: &Value, schema: &Value, value: &Value, at: &str, out: &mut Vec<String>) {
    let object = match schema {
        Value::Bool(true) => return,
        Value::Bool(false) => {
            out.push(format!("{at}: no value is allowed here"));
            return;
        }
        Value::Object(object) => object,
        _ => {
            out.push(format!("{at}: the schema is not an object"));
            return;
        }
    };
    for (keyword, argument) in object {
        match keyword.as_str() {
            "$ref" => reference(root, argument, value, at, out),
            "type" => types(argument, value, at, out),
            "enum" => {
                if !argument.as_array().is_some_and(|a| a.contains(value)) {
                    out.push(format!("{at}: {value} is not one of {argument}"));
                }
            }
            "const" => {
                if argument != value {
                    out.push(format!("{at}: {value} is not {argument}"));
                }
            }
            "properties" | "required" | "additionalProperties" => {
                if keyword == "properties" {
                    properties(root, object, value, at, out);
                }
            }
            "items" | "prefixItems" | "minItems" | "maxItems" => {
                if keyword == "items" || (keyword == "prefixItems" && !object.contains_key("items"))
                {
                    items(root, object, value, at, out);
                }
                bounds_of_array(keyword, argument, value, at, out);
            }
            "oneOf" | "anyOf" | "allOf" => combined(root, keyword, argument, value, at, out),
            "minimum" | "maximum" | "exclusiveMinimum" | "exclusiveMaximum" => {
                number(keyword, argument, value, at, out);
            }
            "minLength" | "maxLength" => length(keyword, argument, value, at, out),
            "unevaluatedProperties" => unevaluated(root, schema, argument, value, at, out),
            "pattern" => {}
            other if ANNOTATIONS.contains(&other) || other.starts_with("x-") => {}
            other => out.push(format!("{at}: the validator does not know `{other}`")),
        }
    }
    // `required` and `additionalProperties` without `properties`.
    if !object.contains_key("properties")
        && (object.contains_key("required") || object.contains_key("additionalProperties"))
    {
        properties(root, object, value, at, out);
    }
}

fn unevaluated(
    root: &Value,
    schema: &Value,
    argument: &Value,
    value: &Value,
    at: &str,
    out: &mut Vec<String>,
) {
    let Some(object) = value.as_object() else {
        return;
    };
    let mut seen = std::collections::BTreeSet::new();
    evaluated(root, schema, value, &mut seen);
    for (name, field) in object {
        if seen.contains(name.as_str()) {
            continue;
        }
        let here = format!("{at}/{name}");
        match argument {
            Value::Bool(false) => out.push(format!("{here}: not allowed")),
            Value::Object(_) => check(root, argument, field, &here, out),
            _ => {}
        }
    }
}

/// The names of `value`'s properties `schema` evaluates (2020-12's
/// annotation collection, for `unevaluatedProperties`).
fn evaluated<'v>(
    root: &Value,
    schema: &Value,
    value: &'v Value,
    seen: &mut std::collections::BTreeSet<&'v str>,
) {
    let (Some(object), Some(rules)) = (value.as_object(), schema.as_object()) else {
        return;
    };
    let declared = rules.get("properties").and_then(Value::as_object);
    let open = rules
        .get("additionalProperties")
        .is_some_and(|a| *a != Value::Bool(false));
    for name in object.keys() {
        if open || declared.is_some_and(|d| d.contains_key(name)) {
            seen.insert(name.as_str());
        }
    }
    if let Some(target) = rules
        .get("$ref")
        .and_then(Value::as_str)
        .and_then(|r| r.strip_prefix('#'))
        .and_then(|p| root.pointer(p))
    {
        evaluated(root, target, value, seen);
    }
    for keyword in ["allOf", "anyOf", "oneOf"] {
        for branch in rules
            .get(keyword)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let mut problems = Vec::new();
            check(root, branch, value, "", &mut problems);
            if problems.is_empty() {
                evaluated(root, branch, value, seen);
            }
        }
    }
}

fn reference(root: &Value, argument: &Value, value: &Value, at: &str, out: &mut Vec<String>) {
    let target = argument
        .as_str()
        .and_then(|r| r.strip_prefix('#'))
        .and_then(|pointer| root.pointer(pointer));
    match target {
        Some(schema) => check(root, schema, value, at, out),
        None => out.push(format!("{at}: unresolved $ref {argument}")),
    }
}

fn type_matches(name: &str, value: &Value) -> bool {
    match name {
        "null" => value.is_null(),
        "boolean" => value.is_boolean(),
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "number" => value.is_number(),
        "integer" => {
            value.is_i64() || value.is_u64() || value.as_f64().is_some_and(|f| f.fract() == 0.0)
        }
        _ => false,
    }
}

fn types(argument: &Value, value: &Value, at: &str, out: &mut Vec<String>) {
    let ok = match argument {
        Value::String(name) => type_matches(name, value),
        Value::Array(names) => names
            .iter()
            .filter_map(Value::as_str)
            .any(|n| type_matches(n, value)),
        _ => false,
    };
    if !ok {
        out.push(format!("{at}: {value} is not of type {argument}"));
    }
}

fn properties(
    root: &Value,
    schema: &Map<String, Value>,
    value: &Value,
    at: &str,
    out: &mut Vec<String>,
) {
    let Some(object) = value.as_object() else {
        return;
    };
    let declared = schema.get("properties").and_then(Value::as_object);
    for name in schema
        .get("required")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        if !object.contains_key(name) {
            out.push(format!("{at}: `{name}` is required"));
        }
    }
    for (name, field) in object {
        let here = format!("{at}/{name}");
        match declared.and_then(|d| d.get(name)) {
            Some(sub) => check(root, sub, field, &here, out),
            None => match schema.get("additionalProperties") {
                Some(Value::Bool(false)) => out.push(format!("{here}: not allowed")),
                Some(extra @ Value::Object(_)) => check(root, extra, field, &here, out),
                _ => {}
            },
        }
    }
}

fn items(
    root: &Value,
    schema: &Map<String, Value>,
    value: &Value,
    at: &str,
    out: &mut Vec<String>,
) {
    let Some(array) = value.as_array() else {
        return;
    };
    let prefix = schema
        .get("prefixItems")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    for (i, item) in array.iter().enumerate() {
        let here = format!("{at}/{i}");
        if let Some(sub) = prefix.get(i).or_else(|| schema.get("items")) {
            check(root, sub, item, &here, out);
        }
    }
}

fn bounds_of_array(
    keyword: &str,
    argument: &Value,
    value: &Value,
    at: &str,
    out: &mut Vec<String>,
) {
    let (Some(array), Some(n)) = (value.as_array(), argument.as_u64()) else {
        return;
    };
    let len = u64::try_from(array.len()).unwrap_or(u64::MAX);
    let ok = match keyword {
        "minItems" => len >= n,
        "maxItems" => len <= n,
        _ => true,
    };
    if !ok {
        out.push(format!("{at}: {len} items breaks {keyword} {n}"));
    }
}

fn combined(
    root: &Value,
    keyword: &str,
    argument: &Value,
    value: &Value,
    at: &str,
    out: &mut Vec<String>,
) {
    let Some(branches) = argument.as_array() else {
        out.push(format!("{at}: {keyword} is not a list"));
        return;
    };
    let failing: Vec<Vec<String>> = branches
        .iter()
        .map(|b| {
            let mut problems = Vec::new();
            check(root, b, value, at, &mut problems);
            problems
        })
        .collect();
    let passing = failing.iter().filter(|p| p.is_empty()).count();
    let ok = match keyword {
        "oneOf" => passing == 1,
        "anyOf" => passing >= 1,
        _ => passing == branches.len(),
    };
    if !ok {
        let first = failing
            .iter()
            .flatten()
            .take(3)
            .cloned()
            .collect::<Vec<_>>()
            .join("; ");
        out.push(format!(
            "{at}: {passing} of {} branches of {keyword} pass ({first})",
            branches.len()
        ));
    }
}

fn number(keyword: &str, argument: &Value, value: &Value, at: &str, out: &mut Vec<String>) {
    let (Some(x), Some(limit)) = (value.as_f64(), argument.as_f64()) else {
        return;
    };
    let ok = match keyword {
        "minimum" => x >= limit,
        "maximum" => x <= limit,
        "exclusiveMinimum" => x > limit,
        _ => x < limit,
    };
    if !ok {
        out.push(format!("{at}: {x} breaks {keyword} {limit}"));
    }
}

fn length(keyword: &str, argument: &Value, value: &Value, at: &str, out: &mut Vec<String>) {
    let (Some(text), Some(n)) = (value.as_str(), argument.as_u64()) else {
        return;
    };
    let len = u64::try_from(text.chars().count()).unwrap_or(u64::MAX);
    let ok = if keyword == "minLength" {
        len >= n
    } else {
        len <= n
    };
    if !ok {
        out.push(format!("{at}: {len} characters breaks {keyword} {n}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// R14's controls: each keyword refuses what it should, and an unknown
    /// keyword is reported rather than passed.
    #[test]
    fn each_keyword_refuses_what_it_should() {
        let schema = json!({
            "$defs": { "N": { "type": "integer", "minimum": 0 } },
            "type": "object",
            "required": ["a", "b"],
            "additionalProperties": false,
            "properties": {
                "a": { "$ref": "#/$defs/N" },
                "b": { "anyOf": [{ "type": "null" }, { "enum": ["x", "y"] }] },
                "c": { "type": "array", "prefixItems": [{ "type": "number" }, { "type": "number" }],
                       "minItems": 2, "maxItems": 2 },
                "d": { "oneOf": [{ "const": "p" }, { "type": "object", "required": ["k"] }] },
            },
        });
        let good = json!({ "a": 1, "b": null, "c": [1.5, 2], "d": "p" });
        assert_eq!(validate(&schema, &good), Vec::<String>::new());
        for bad in [
            json!({ "a": -1, "b": null }),
            json!({ "a": 1 }),
            json!({ "a": 1, "b": "z" }),
            json!({ "a": 1, "b": null, "e": 0 }),
            json!({ "a": 1, "b": null, "c": [1] }),
            json!({ "a": 1, "b": null, "c": [1, "x"] }),
            json!({ "a": 1, "b": null, "d": "q" }),
            json!({ "a": "1", "b": null }),
        ] {
            assert_ne!(validate(&schema, &bad), Vec::<String>::new(), "{bad}");
        }
        assert_ne!(
            validate(&json!({ "if": {} }), &json!(1)),
            Vec::<String>::new()
        );
        let flattened = json!({
            "type": "object",
            "properties": { "a": { "type": "integer" } },
            "oneOf": [
                { "properties": { "b": { "type": "integer" } }, "required": ["b"] },
                { "properties": { "c": { "type": "integer" } }, "required": ["c"] },
            ],
            "unevaluatedProperties": false,
        });
        assert_eq!(
            validate(&flattened, &json!({ "a": 1, "b": 2 })),
            Vec::<String>::new()
        );
        assert_ne!(
            validate(&flattened, &json!({ "a": 1, "b": 2, "d": 3 })),
            Vec::<String>::new()
        );
        assert_ne!(
            validate(&flattened, &json!({ "a": 1, "b": 2, "c": 3 })),
            Vec::<String>::new()
        );
        assert_ne!(
            validate(&json!({ "$ref": "#/$defs/missing" }), &json!(1)),
            Vec::<String>::new()
        );
    }
}
