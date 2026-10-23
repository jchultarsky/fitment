//! Stored bytes out (docs/research/m6-api.md, DC1, DC4 to DC6, OB3, VN5):
//! a document or a file sent exactly as stored, read checked against its
//! name first (OB3), with the contract's headers; strong ETags of the
//! body, `If-None-Match` answered `304`; `Cache-Control` `immutable` when
//! the URL names exact bytes, `no-cache` otherwise; and `Content-Encoding:
//! gzip` when the client asks, transport only, its ETag suffixed `-gz`.

use std::io::Write as _;
use std::sync::Arc;

use axum::body::{Body, Bytes};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};

use super::error::{ApiError, ErrorCode};
use super::json;
use crate::Shared;
use crate::jobs::events::ChannelBody;
use crate::state::ObjectError;

/// VN5's cache policies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Cache {
    /// The URL names exact bytes: `private, max-age=31536000, immutable`.
    Immutable,
    /// It may serve other bytes later (a bare reference, a derived
    /// document): `no-cache`.
    NoCache,
}

impl Cache {
    fn value(self) -> &'static str {
        match self {
            Self::Immutable => "private, max-age=31536000, immutable",
            Self::NoCache => "no-cache",
        }
    }
}

/// An object to send and its headers (DC4).
#[derive(Debug, Clone)]
pub(crate) struct Served {
    /// The object's name and length.
    pub sha256: String,
    pub bytes: u64,
    /// `application/json`, or `application/octet-stream` for STEP.
    pub content_type: &'static str,
    /// `Fitment-Format`'s format and version.
    pub format: Option<(String, u32)>,
    /// `Fitment-Exit-Code` (DC3).
    pub exit_code: Option<u8>,
    /// `Fitment-Digest`: a socket's or a match's own `digest` (DC4).
    pub digest: Option<String>,
    /// VN5.
    pub cache: Cache,
    /// `Content-Disposition`'s file name (HX6).
    pub disposition: Option<String>,
}

/// VN5: true if `If-None-Match` names `etag` (weakly, RFC 9110 13.1.2),
/// or is `*`.
pub(crate) fn not_modified(headers: &HeaderMap, etag: &str) -> bool {
    let bare = |t: &str| t.trim().trim_start_matches("W/").to_owned();
    let wanted = bare(etag);
    headers
        .get_all(header::IF_NONE_MATCH)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .any(|t| t.trim() == "*" || bare(t) == wanted)
}

/// `304 Not Modified` with its ETag and cache policy (VN5).
pub(crate) fn not_modified_response(etag: &str, cache: &'static str) -> Response {
    let mut response = StatusCode::NOT_MODIFIED.into_response();
    if let Ok(value) = HeaderValue::from_str(etag) {
        response.headers_mut().insert(header::ETAG, value);
    }
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    response
}

/// DC6: the client accepts `gzip` (`Accept-Encoding`, at a quality above
/// zero).
pub(crate) fn wants_gzip(headers: &HeaderMap) -> bool {
    headers
        .get_all(header::ACCEPT_ENCODING)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .any(|entry| {
            let mut parts = entry.split(';').map(str::trim);
            let coding = parts.next().unwrap_or("");
            let q = parts
                .filter_map(|p| p.strip_prefix("q=").or_else(|| p.strip_prefix("Q=")))
                .find_map(|q| q.parse::<f32>().ok())
                .unwrap_or(1.0);
            coding.eq_ignore_ascii_case("gzip") && q > 0.0
        })
}

/// `bytes` gzipped (DC6).
fn gzip(bytes: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut encoder = flate2::write::GzEncoder::new(
        Vec::with_capacity(bytes.len() / 4),
        flate2::Compression::default(),
    );
    encoder.write_all(bytes)?;
    encoder.finish()
}

/// OB3: why an object could not be sent.
pub(crate) fn object_error(sha256: &str, error: &ObjectError) -> ApiError {
    if let ObjectError::Io(e) = error {
        tracing::error!(object = sha256, kind = ?e.kind(), "an object could not be read");
        ApiError::new(ErrorCode::IoError, "the stored bytes could not be read")
    } else {
        tracing::error!(object = sha256, error = %error, "an object is damaged");
        ApiError::new(
            ErrorCode::BlobDamaged,
            "the stored bytes are not their name's: nothing was sent",
        )
    }
}

/// The contract's headers on `response` (DC4, VN5, HX6).
fn headers(response: &mut Response, served: &Served, etag: &str, digest: &str, gz: bool) {
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(served.content_type),
    );
    if let Some(value) = served
        .format
        .as_ref()
        .and_then(|(f, v)| HeaderValue::from_str(&format!("{f}/{v}")).ok())
    {
        headers.insert("fitment-format", value);
    }
    if let Some(code) = served.exit_code {
        headers.insert("fitment-exit-code", HeaderValue::from(u16::from(code)));
    }
    if let Some(value) = served
        .digest
        .as_deref()
        .and_then(|d| HeaderValue::from_str(d).ok())
    {
        headers.insert("fitment-digest", value);
    }
    if let Ok(value) = HeaderValue::from_str(etag) {
        headers.insert(header::ETAG, value);
    }
    if let Ok(value) = HeaderValue::from_str(digest) {
        headers.insert("repr-digest", value);
    }
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(served.cache.value()),
    );
    headers.insert(header::VARY, HeaderValue::from_static("accept-encoding"));
    if gz {
        headers.insert(header::CONTENT_ENCODING, HeaderValue::from_static("gzip"));
    }
    if let Some(value) = served
        .disposition
        .as_deref()
        .and_then(|n| HeaderValue::from_str(&format!("attachment; filename=\"{n}\"")).ok())
    {
        headers.insert(header::CONTENT_DISPOSITION, value);
    }
}

fn unhex(hex: &str) -> Vec<u8> {
    (0..hex.len() / 2)
        .filter_map(|i| u8::from_str_radix(hex.get(2 * i..2 * i + 2)?, 16).ok())
        .collect()
}

/// The largest object sent from one buffer (review S1 of PR 5): above it
/// a download is streamed, so an unread response holds at most this many
/// bytes of the object, and only these are gzipped (DC6).
pub const SEND_BUFFER_MAX: u64 = 1024 * 1024;

/// The bytes each streamed read takes (OB3's second pass).
const CHUNK: usize = 256 * 1024;

/// Sends `served` (OB3): within [`SEND_BUFFER_MAX`] (and the store's
/// one-buffer cap), read whole and checked into one buffer, sent without a
/// copy and gzipped if the client asks; above it, hashed in one pass and
/// streamed in a second that fails on a change, never gzipped.
pub(crate) async fn object(
    shared: &Arc<Shared>,
    request: &HeaderMap,
    served: Served,
) -> Result<Response, ApiError> {
    let small = served.bytes <= SEND_BUFFER_MAX.min(shared.objects.one_buffer_max());
    let gz = small && wants_gzip(request);
    let etag = if gz {
        format!("\"sha256:{}-gz\"", served.sha256)
    } else {
        format!("\"sha256:{}\"", served.sha256)
    };
    if not_modified(request, &etag) {
        return Ok(not_modified_response(&etag, served.cache.value()));
    }
    if !small {
        return streamed(shared, served, &etag).await;
    }
    let (reading, sha256, bytes) = (Arc::clone(shared), served.sha256.clone(), served.bytes);
    let body = super::stored(shared, move || {
        let read = reading
            .objects
            .read(&sha256, bytes)
            .map_err(|e| object_error(&sha256, &e))?;
        if gz {
            gzip(&read).map_err(|_| ApiError::internal())
        } else {
            Ok(read)
        }
    })
    .await?;
    let digest = json::repr_digest(&body);
    let mut response = (StatusCode::OK, Body::from(Bytes::from(body))).into_response();
    headers(&mut response, &served, &etag, &digest, gz);
    Ok(response)
}

/// Bytes ready to send from memory (a catalog file re-hashed, ST11; a
/// derived document): gzipped if asked, with their `Repr-Digest`, both
/// made where they were read, on the blocking pool, never on the
/// runtime's workers (the review of PR 7b).
#[derive(Debug)]
pub(crate) struct Ready {
    body: Vec<u8>,
    gz: bool,
    digest: String,
}

/// `body`, whose SHA-256 is `sha256`, made [`Ready`]: gzipped when `gz`
/// and within [`SEND_BUFFER_MAX`] (DC6), its digest the gzipped bytes'
/// then, else its own name's. Blocking: run it where the bytes were read.
pub(crate) fn ready(body: Vec<u8>, sha256: &str, gz: bool) -> Ready {
    let small = u64::try_from(body.len()).unwrap_or(u64::MAX) <= SEND_BUFFER_MAX;
    match (small && gz).then(|| gzip(&body).ok()).flatten() {
        Some(zipped) => {
            let digest = json::repr_digest(&zipped);
            Ready {
                body: zipped,
                gz: true,
                digest,
            }
        }
        None => Ready {
            body,
            gz: false,
            digest: format!("sha-256=:{}:", json::base64(&unhex(sha256))),
        },
    }
}

/// [`Ready`] bytes sent as [`object`] sends a small object, `304` for
/// their ETag.
pub(crate) fn memory(request: &HeaderMap, served: &Served, ready: Ready) -> Response {
    let etag = if ready.gz {
        format!("\"sha256:{}-gz\"", served.sha256)
    } else {
        format!("\"sha256:{}\"", served.sha256)
    };
    if not_modified(request, &etag) {
        return not_modified_response(&etag, served.cache.value());
    }
    let mut response = (StatusCode::OK, Body::from(Bytes::from(ready.body))).into_response();
    headers(&mut response, served, &etag, &ready.digest, ready.gz);
    response
}

/// A file checked in one pass, sent from `reader`'s second (OB3): its
/// ETag `304`'d first, never gzipped; each chunk read in its own short
/// blocking call.
pub(crate) fn stream(
    request: &HeaderMap,
    served: &Served,
    reader: Box<dyn crate::catalog::FsRead>,
) -> Response {
    let etag = format!("\"sha256:{}\"", served.sha256);
    if not_modified(request, &etag) {
        return not_modified_response(&etag, served.cache.value());
    }
    sent(reader, served, &etag)
}

/// OB3's two passes, the second fed to the body by an async task: each
/// chunk is read in its own short blocking call and sent outside it, so no
/// blocking thread ever waits for a client (review R of PR 5); a client
/// gone stops the reader (VT13).
async fn streamed(shared: &Arc<Shared>, served: Served, etag: &str) -> Result<Response, ApiError> {
    let (opening, sha256, bytes) = (Arc::clone(shared), served.sha256.clone(), served.bytes);
    let reader = super::stored(shared, move || {
        opening
            .objects
            .open_checked(&sha256, bytes)
            .map_err(|e| object_error(&sha256, &e))
    })
    .await?;
    Ok(sent(reader, &served, etag))
}

/// The response whose body `reader` feeds, a chunk per blocking read.
fn sent(reader: Box<dyn crate::catalog::FsRead>, served: &Served, etag: &str) -> Response {
    let (body, out) = ChannelBody::new(4);
    tokio::spawn(async move {
        let mut reader = reader;
        loop {
            let read = tokio::task::spawn_blocking(move || {
                let mut buf = vec![0_u8; CHUNK];
                let read = reader.read_chunk(&mut buf).map(|n| {
                    buf.truncate(n);
                    buf
                });
                (reader, read)
            })
            .await;
            let Ok((back, read)) = read else {
                return;
            };
            reader = back;
            match read {
                Ok(chunk) if chunk.is_empty() => return,
                Ok(chunk) => {
                    if out.send(Ok(Bytes::from(chunk))).await.is_err() {
                        return;
                    }
                }
                Err(error) => {
                    let _ = out.send(Err(error)).await;
                    return;
                }
            }
        }
    });
    let mut response = (StatusCode::OK, Body::new(body)).into_response();
    let digest = format!("sha-256=:{}:", json::base64(&unhex(&served.sha256)));
    headers(&mut response, served, etag, &digest, false);
    response
}
