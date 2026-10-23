//! JSON in and out (docs/research/m6-api.md, M6-34, VN6, DC9): the
//! server's own extractor and responder over `serde_json::from_slice` and
//! `to_string_pretty`, so axum's `json` feature, and with it `serde_json`'s
//! `raw_value`, stays out of the graph (SV9).

use axum::body::{Body, Bytes};
use axum::extract::{FromRequest, Request};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use http_body_util::BodyExt as _;
use serde::Serialize;
use serde::de::DeserializeOwned;

use super::error::{ApiError, ErrorCode};

/// `application/json`.
pub const JSON: &str = "application/json";

/// `application/problem+json`.
pub const PROBLEM_JSON: &str = "application/problem+json";

/// The cap on a JSON request body, in bytes (HX2's `max_json_kib`), as an
/// extension the router sets.
#[derive(Debug, Clone, Copy)]
pub struct JsonCap(pub usize);

/// A JSON request body, read under VN6's conventions into `T`, whose
/// types refuse unknown fields (`deny_unknown_fields`) and, being structs,
/// duplicate keys (serde's derive).
#[derive(Debug, Clone)]
pub struct Json<T>(pub T);

impl<S: Send + Sync, T: DeserializeOwned> FromRequest<S> for Json<T> {
    type Rejection = ApiError;

    async fn from_request(request: Request, state: &S) -> Result<Self, ApiError> {
        Sent::<T>::from_request(request, state)
            .await
            .map(|sent| Self(sent.0))
    }
}

/// A JSON request body read as [`Json`] reads it, with the bytes as sent:
/// what an `Idempotency-Key` is bound to (JB9).
#[derive(Debug, Clone)]
pub struct Sent<T>(pub T, pub Bytes);

impl<S: Send + Sync, T: DeserializeOwned> FromRequest<S> for Sent<T> {
    type Rejection = ApiError;

    async fn from_request(request: Request, _: &S) -> Result<Self, ApiError> {
        check_content_type(request.headers())?;
        let cap = request
            .extensions()
            .get::<JsonCap>()
            .map_or(1024 * 1024, |c| c.0);
        let limited = http_body_util::Limited::new(request.into_body(), cap);
        let bytes = limited.collect().await.map_err(|e| {
            if e.is::<http_body_util::LengthLimitError>() {
                ApiError::new(
                    ErrorCode::BodyTooLarge,
                    format!("a JSON body is at most {} KiB", cap / 1024),
                )
            } else {
                ApiError::new(ErrorCode::BadRequest, "the body could not be read")
            }
        })?;
        let bytes = bytes.to_bytes();
        decode(&bytes).map(|value| Self(value, bytes))
    }
}

/// `Content-Type` must be `application/json` (an optional `charset=utf-8`
/// allowed), and no `Content-Encoding` is taken (DC6): `415`.
///
/// # Errors
///
/// `unsupported_media_type`.
pub fn check_content_type(headers: &HeaderMap) -> Result<(), ApiError> {
    if headers.contains_key(header::CONTENT_ENCODING) {
        return Err(ApiError::new(
            ErrorCode::UnsupportedMediaType,
            "request bodies are never decompressed: send it without Content-Encoding",
        ));
    }
    let ok = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| {
            let mut parts = v.split(';').map(str::trim);
            let media = parts.next().unwrap_or("");
            media.eq_ignore_ascii_case(JSON)
                && parts.all(|p| p.eq_ignore_ascii_case("charset=utf-8"))
        });
    if ok {
        Ok(())
    } else {
        Err(ApiError::new(
            ErrorCode::UnsupportedMediaType,
            "the body must be application/json",
        ))
    }
}

/// Reads `bytes` as `T` under VN6: UTF-8 without a byte-order mark, one
/// JSON value and nothing after it, unknown fields and duplicate keys
/// refused. The detail names the line and column, never serde's text,
/// which can echo the client's input (ER3).
///
/// # Errors
///
/// `bad_request`.
pub fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, ApiError> {
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return Err(ApiError::new(
            ErrorCode::BadRequest,
            "the body starts with a byte-order mark",
        ));
    }
    if std::str::from_utf8(bytes).is_err() {
        return Err(ApiError::new(
            ErrorCode::BadRequest,
            "the body is not UTF-8",
        ));
    }
    serde_json::from_slice(bytes).map_err(|e| {
        let what = match e.classify() {
            serde_json::error::Category::Data => "does not have the request's fields and types",
            serde_json::error::Category::Eof => "ends early",
            _ => "is not one JSON value",
        };
        ApiError::new(
            ErrorCode::BadRequest,
            format!("the body {what} (line {}, column {})", e.line(), e.column()),
        )
        .with("line", e.line())
        .with("column", e.column())
    })
}

/// `value` as pretty JSON with a final newline, as the frozen documents
/// are written (DC9).
pub fn pretty<T: Serialize>(value: &T) -> String {
    // Every key is a string and every number finite: this cannot fail.
    serde_json::to_string_pretty(value).unwrap_or_default() + "\n"
}

/// A document response: `200`, `Content-Type`, `Fitment-Format`
/// (`<format>/<version>`) and `Repr-Digest` (DC4).
pub fn document<T: Serialize>(format: &str, version: u32, value: &T) -> Response {
    bytes(
        StatusCode::OK,
        JSON,
        Some((format, version)),
        Bytes::from(pretty(value)),
    )
}

/// A response of exact bytes, with `Repr-Digest` of them (DC4).
pub fn bytes(
    status: StatusCode,
    content_type: &'static str,
    format: Option<(&str, u32)>,
    body: Bytes,
) -> Response {
    let digest = repr_digest(&body);
    bytes_digested(status, content_type, format, body, &digest)
}

/// [`bytes`] with `Repr-Digest` given, computed where the body was built:
/// a large body is never hashed on the runtime's workers (the review of
/// PR 7b).
pub fn bytes_digested(
    status: StatusCode,
    content_type: &'static str,
    format: Option<(&str, u32)>,
    body: Bytes,
    digest: &str,
) -> Response {
    let mut response = (status, Body::from(body)).into_response();
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    if let Some(value) = format.and_then(|(f, v)| HeaderValue::from_str(&format!("{f}/{v}")).ok()) {
        headers.insert("fitment-format", value);
    }
    if let Ok(value) = HeaderValue::from_str(digest) {
        headers.insert("repr-digest", value);
    }
    response
}

/// `sha-256=:<base64>:` (RFC 9530) of `body`.
pub fn repr_digest(body: &[u8]) -> String {
    format!("sha-256=:{}:", base64(&fitment_core::sha256(body)))
}

/// Standard base64 with padding (RFC 4648, section 4).
pub fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0_u32, |n, (i, &b)| n | (u32::from(b) << (16 - 8 * i)));
        for i in 0..4 {
            if i <= chunk.len() {
                let index = (n >> (18 - 6 * i)) & 0x3f;
                out.push(char::from(TABLE[usize::try_from(index).unwrap_or(0)]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// True if `accept` admits `media` (VN8): no header, `*/*`, `type/*` or
/// the type itself, at a quality above zero. Unparsable entries are
/// ignored, and a header of nothing but them admits everything.
pub fn acceptable(accept: Option<&HeaderValue>, media: &str) -> bool {
    let Some(text) = accept.and_then(|v| v.to_str().ok()) else {
        return true;
    };
    let (kind, _) = media.split_once('/').unwrap_or((media, ""));
    let mut parsed = 0;
    for entry in text.split(',') {
        let mut parts = entry.split(';').map(str::trim);
        let range = parts.next().unwrap_or("");
        let Some((t, s)) = range.split_once('/') else {
            continue;
        };
        parsed += 1;
        let q = parts
            .filter_map(|p| p.strip_prefix("q=").or_else(|| p.strip_prefix("Q=")))
            .find_map(|q| q.parse::<f32>().ok())
            .unwrap_or(1.0);
        let matches = (s == "*" && (t == "*" || t.eq_ignore_ascii_case(kind)))
            || range.eq_ignore_ascii_case(media);
        if matches && q > 0.0 {
            return true;
        }
    }
    parsed == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, Deserialize, PartialEq)]
    #[serde(deny_unknown_fields)]
    struct Probe {
        name: String,
        tolerance_mm: Option<f64>,
    }

    fn code(bytes: &[u8]) -> Option<ErrorCode> {
        decode::<Probe>(bytes).err().map(|e| e.code())
    }

    /// VN6: unknown fields, duplicate keys, a byte-order mark, invalid
    /// UTF-8, trailing garbage, a truncated document, deep nesting and
    /// numbers beyond f64 are each `400 bad_request`; a good body reads.
    #[test]
    fn request_bodies_follow_vn6() {
        let good = br#"{"name": "a", "tolerance_mm": 0.01}"#;
        assert_eq!(
            decode::<Probe>(good).unwrap(),
            Probe {
                name: "a".into(),
                tolerance_mm: Some(0.01)
            }
        );
        let deep = format!("{}{}", "[".repeat(100_000), "]".repeat(100_000));
        let bad: [&[u8]; 9] = [
            br#"{"name": "a", "extra": 1}"#,
            br#"{"name": "a", "name": "b"}"#,
            b"\xEF\xBB\xBF{\"name\": \"a\"}",
            b"{\"name\": \"\xff\"}",
            br#"{"name": "a"} trailing"#,
            br#"{"name": "a""#,
            deep.as_bytes(),
            br#"{"name": "a", "tolerance_mm": 1e400}"#,
            br#"{"name": "a", "tolerance_mm": NaN}"#,
        ];
        for body in bad {
            assert_eq!(
                code(body),
                Some(ErrorCode::BadRequest),
                "{:?}",
                String::from_utf8_lossy(body)
            );
        }
    }

    /// VN6, ER3: the detail of a refused body never echoes it.
    #[test]
    fn a_refused_body_is_not_echoed() {
        let error = decode::<Probe>(br#"{"name": "a", "/etc/passwd": 1}"#).unwrap_err();
        let problem = error.document(&axum::http::Method::POST, "r_x");
        let text = serde_json::to_string(&problem).unwrap();
        assert!(!text.contains("passwd"), "{text}");
    }

    /// DC6, VN6: a JSON route takes `application/json` only, and no
    /// `Content-Encoding`.
    #[test]
    fn content_types_are_checked() {
        let mut headers = HeaderMap::new();
        assert!(check_content_type(&headers).is_err());
        headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain"));
        assert!(check_content_type(&headers).is_err());
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json; charset=utf-8"),
        );
        assert!(check_content_type(&headers).is_ok());
        headers.insert(header::CONTENT_ENCODING, HeaderValue::from_static("gzip"));
        assert!(check_content_type(&headers).is_err());
    }

    /// VN8's matching of `Accept`.
    #[test]
    fn accept_admits_its_ranges() {
        let h = |s: &'static str| HeaderValue::from_static(s);
        assert!(acceptable(None, JSON));
        assert!(acceptable(Some(&h("*/*")), JSON));
        assert!(acceptable(Some(&h("application/*")), JSON));
        assert!(acceptable(
            Some(&h("text/html, application/json;q=0.5")),
            JSON
        ));
        assert!(!acceptable(Some(&h("text/html")), JSON));
        assert!(!acceptable(Some(&h("application/json;q=0")), JSON));
        assert!(!acceptable(Some(&h("application/xml")), JSON));
    }

    #[test]
    fn base64_is_rfc_4648s() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }
}
