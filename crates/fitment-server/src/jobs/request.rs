//! What a job-creating request says (docs/research/m6-api.md, JB1, JB9,
//! JB11, RA8, RA9, section 7.5 to 7.7): its bodies, read under VN6; the
//! checks made before any job exists; the normalized request a job
//! records; the fingerprint of the request as sent, which an
//! `Idempotency-Key` is bound to; and `Prefer: wait`.

use axum::http::HeaderMap;
use serde::de::{self, Deserializer, MapAccess, Visitor};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::http::error::{ApiError, ErrorCode};

/// The longest `Idempotency-Key` (JB9).
pub const MAX_KEY: usize = 128;

/// The longest display text (RA8): a part or occurrence selection.
pub const MAX_TEXT: usize = 1024;

/// The longest `file_name` (RA8).
pub const MAX_FILE_NAME: usize = 255;

/// `POST /v1/features` (section 7.7).
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct FeaturesRequest {
    /// The part's upload, `sha256:HEX` (UP1).
    pub upload: String,
}

/// `POST /v1/assemblies` (section 7.5).
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct AssemblyRequest {
    /// The assembly's upload, `sha256:HEX` (UP1).
    pub upload: String,
    /// The name it is registered under (RA8): display text, never a path.
    #[serde(default)]
    pub file_name: Option<String>,
}

/// `POST /v1/sockets` (section 7.6).
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SocketRequest {
    /// The caller's registered assembly, `sha256:HEX` (RA3).
    pub assembly: String,
    /// As `--part`: `PRODUCT.id`, a name or `#<n>`.
    #[serde(default)]
    pub part: Option<String>,
    /// As `--occurrence`: usage ids or `#<n>` joined by `/`.
    #[serde(default)]
    pub occurrence: Option<String>,
    /// Carry the decisions of the line's current revision (default true),
    /// as `fitment socket -o OUT` carries OUT's.
    #[serde(default)]
    pub carry: Option<bool>,
    /// Or carry from this: another revision of the caller's lines, or an
    /// uploaded document of any version (MF11).
    #[serde(default)]
    pub carry_from: Option<CarryFrom>,
}

/// `POST /v1/jobs/{id}/pin` (JB12).
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct PinRequest {
    /// Keep the record past retention, or not.
    pub pinned: bool,
}

/// `carry_from`: `"sk_…@rN"`, or `{"upload": "sha256:…"}`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "schema", schemars(untagged))]
pub enum CarryFrom {
    /// A revision of the caller's lines.
    Revision(String),
    /// An uploaded socket document.
    Upload {
        /// `sha256:HEX`.
        upload: String,
    },
}

/// A string, or an object with one key, `upload`, given once (VN6:
/// duplicate and unknown keys refused): `(revision, upload)`.
fn revision_or_upload<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Result<String, String>, D::Error> {
    struct Of;
    impl<'de> Visitor<'de> for Of {
        type Value = Result<String, String>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(r#""sk_…@rN" or {"upload": "sha256:…"}"#)
        }
        fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
            Ok(Ok(value.to_owned()))
        }
        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
            let mut upload: Option<String> = None;
            while let Some(key) = map.next_key::<String>()? {
                if key != "upload" {
                    return Err(de::Error::unknown_field(&key, &["upload"]));
                }
                if upload.is_some() {
                    return Err(de::Error::duplicate_field("upload"));
                }
                upload = Some(map.next_value()?);
            }
            upload
                .map(Err)
                .ok_or_else(|| de::Error::missing_field("upload"))
        }
    }
    deserializer.deserialize_any(Of)
}

impl<'de> Deserialize<'de> for CarryFrom {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(match revision_or_upload(deserializer)? {
            Ok(revision) => Self::Revision(revision),
            Err(upload) => Self::Upload { upload },
        })
    }
}

/// A match's socket (section 7.8): `"sk_…"` or `"sk_…@rN"` of the
/// caller's lines, or `{"upload": "sha256:…"}`, a socket file as the CLI
/// reads it (UP6).
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "schema", schemars(untagged))]
pub enum SocketFrom {
    /// A revision of the caller's lines; a bare id its current one (RA9).
    Revision(String),
    /// An uploaded socket document.
    Upload {
        /// `sha256:HEX`.
        upload: String,
    },
}

impl<'de> Deserialize<'de> for SocketFrom {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(match revision_or_upload(deserializer)? {
            Ok(revision) => Self::Revision(revision),
            Err(upload) => Self::Upload { upload },
        })
    }
}

/// `POST /v1/sockets/{line}@r{n}/review` (section 7.6, OB5, RA7).
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ReviewRequest {
    /// Applied in order, all or none, as `fitment review` once per
    /// decision.
    pub decisions: Vec<Decision>,
}

/// One review decision: `fitment review --keep/--remove/--unreview ID
/// [--note T] [--socket S]`.
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct Decision {
    /// The requirement's id.
    pub requirement: String,
    /// Only in this socket; every socket holding it when null.
    #[serde(default)]
    pub socket: Option<String>,
    /// `kept`, `removed` or `unreviewed`.
    pub status: DecisionStatus,
    /// Why: required to remove (M3 9.4).
    #[serde(default)]
    pub note: Option<String>,
}

/// A decision's status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum DecisionStatus {
    /// Reviewed and kept.
    Kept,
    /// Taken out of matching, with a note.
    Removed,
    /// Back to unreviewed.
    Unreviewed,
}

/// `POST /v1/sockets/import` (section 7.6, RA10, UP6).
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ImportRequest {
    /// The uploaded `fitment.socket`, `sha256:HEX`.
    pub upload: String,
}

/// `POST /v1/matches` (section 7.8): a file run's candidates are uploads
/// and components of the caller's assemblies; the catalog's runs come with
/// PR 7.
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MatchRequest {
    /// The socket judged against.
    pub socket: SocketFrom,
    /// What is judged.
    pub candidates: CandidatesRequest,
    /// DC7: the CLI's flags as fields; D9's and D6's defaults.
    #[serde(default)]
    pub policy: Option<PolicyRequest>,
    /// `require_fit` and `require_reviewed`, default false (SN3).
    #[serde(default)]
    pub options: Option<OptionsRequest>,
    /// Also judge the original (VD15, SN9): the socket's own target, from
    /// its assembly, which the caller registered.
    #[serde(default)]
    pub original: Option<bool>,
}

/// A match's candidates: files and components, at least one (CM1).
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct CandidatesRequest {
    /// Uploaded STEP files, each with its label (RA8).
    #[serde(default)]
    pub files: Option<Vec<FileCandidate>>,
    /// Leaves of the caller's registered assemblies (CO5).
    #[serde(default)]
    pub components: Option<Vec<ComponentCandidate>>,
}

/// A file candidate.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct FileCandidate {
    /// `sha256:HEX`, an upload of the caller's (UP3).
    pub upload: String,
    /// The CLI's label, the argument as given (RA8): required.
    pub label: String,
}

/// A component candidate: a part of an assembly, as `--part` and
/// `--occurrence` name one.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ComponentCandidate {
    /// `sha256:HEX`, an assembly the caller registered.
    pub assembly: String,
    /// As `--part`.
    #[serde(default)]
    pub part: Option<String>,
    /// As `--occurrence`.
    #[serde(default)]
    pub occurrence: Option<String>,
    /// Its label; default `component:<assembly sha256, first 12>/#<n>`.
    #[serde(default)]
    pub label: Option<String>,
}

/// DC7's policy fields.
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct PolicyRequest {
    /// D9's linear tolerance, in mm (0.01).
    #[serde(default)]
    pub tolerance_mm: Option<f64>,
    /// D9's angular tolerance, in degrees (0.01).
    #[serde(default)]
    pub tolerance_deg: Option<f64>,
    /// D6: `strict` (the default) or `functional`.
    #[serde(default)]
    pub clearance: Option<crate::matching::ClearanceName>,
}

/// A match's options.
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct OptionsRequest {
    /// `--require-fit`.
    #[serde(default)]
    pub require_fit: Option<bool>,
    /// `--require-reviewed`; the operator's floor may force it (SN3).
    #[serde(default)]
    pub require_reviewed: Option<bool>,
}

/// HX5, UP1: `sha256:` and 64 hex digits, either case, written lower-case;
/// anything else is `400 invalid_reference`, its value never echoed.
///
/// # Errors
///
/// `invalid_reference`, naming `field`.
pub fn sha256_ref(text: &str, field: &str) -> Result<String, ApiError> {
    text.strip_prefix("sha256:")
        .filter(|hex| hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit()))
        .map(str::to_ascii_lowercase)
        .ok_or_else(|| {
            ApiError::new(
                ErrorCode::InvalidReference,
                "bytes are named sha256: and their 64 hex digits",
            )
            .with("field", field)
        })
}

/// RA8: display text: at most [`MAX_TEXT`] bytes of UTF-8 without control
/// characters.
///
/// # Errors
///
/// `name_invalid`, naming `field`.
pub fn text(value: &str, field: &str) -> Result<(), ApiError> {
    if value.is_empty() || value.len() > MAX_TEXT || value.chars().any(char::is_control) {
        return Err(ApiError::new(
            ErrorCode::NameInvalid,
            format!("{field} is 1 to {MAX_TEXT} bytes of text without control characters"),
        )
        .with("field", field));
    }
    Ok(())
}

/// RA8: a `file_name`: display text that is also a final path component:
/// no `/`, `\` or NUL, not `.` or `..`, at most [`MAX_FILE_NAME`] bytes.
/// Refused, never repaired.
///
/// # Errors
///
/// `name_invalid`.
pub fn file_name(value: &str) -> Result<(), ApiError> {
    text(value, "file_name")?;
    if value.len() > MAX_FILE_NAME
        || value.contains(['/', '\\', '\0'])
        || value == "."
        || value == ".."
    {
        return Err(ApiError::new(
            ErrorCode::NameInvalid,
            format!(
                "file_name is a final name of at most {MAX_FILE_NAME} bytes: no /, \\ or NUL, not . or .."
            ),
        )
        .with("field", "file_name"));
    }
    Ok(())
}

/// A socket reference (HX5, RA5): `sk_` and 20 lower-case hex digits, and
/// an optional `@rN` in ID9's grammar (lower-case `@r`, no leading zero).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SocketRef {
    /// `sk_…`.
    pub id: String,
    /// The revision, if named.
    pub n: Option<i64>,
}

impl SocketRef {
    /// Parses `text`.
    ///
    /// # Errors
    ///
    /// `invalid_reference`.
    pub fn parse(text: &str) -> Result<Self, ApiError> {
        let bad = || {
            ApiError::new(
                ErrorCode::InvalidReference,
                "a socket is named sk_ and 20 hex digits, and @rN for one revision",
            )
        };
        let (id, n) = match text.split_once('@') {
            Some((id, rev)) => {
                let digits = rev.strip_prefix('r').ok_or_else(bad)?;
                let ok = !digits.is_empty()
                    && digits.len() <= 18
                    && digits.bytes().all(|b| b.is_ascii_digit())
                    && !digits.starts_with('0');
                if !ok {
                    return Err(bad());
                }
                (id, Some(digits.parse::<i64>().map_err(|_| bad())?))
            }
            None => (text, None),
        };
        let hex = id.strip_prefix("sk_").ok_or_else(bad)?;
        if hex.len() != 20 || !hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
            return Err(bad());
        }
        Ok(Self {
            id: id.to_owned(),
            n,
        })
    }

    /// `sk_…@rN`, or `sk_…`.
    pub fn text(&self) -> String {
        match self.n {
            Some(n) => format!("{}@r{n}", self.id),
            None => self.id.clone(),
        }
    }
}

/// RA4: a line's id: `sk_` and the first 20 hex digits of the SHA-256 of
/// `"fitment.socket-line\n" + assembly + "\n" + definition + "\n" +
/// occurrence`, so it depends on the data alone.
pub fn line_id(assembly: &str, definition: &str, occurrence: &str) -> String {
    let digest = fitment_core::sha256_hex(
        format!("fitment.socket-line\n{assembly}\n{definition}\n{occurrence}").as_bytes(),
    );
    format!("sk_{}", &digest[..20])
}

/// JB9: `Idempotency-Key`, if sent: at most [`MAX_KEY`] printable ASCII
/// characters.
///
/// # Errors
///
/// `invalid_parameter`.
pub fn idempotency_key(headers: &HeaderMap) -> Result<Option<String>, ApiError> {
    let mut values = headers.get_all("idempotency-key").iter();
    let Some(value) = values.next() else {
        return Ok(None);
    };
    let bad = || {
        ApiError::new(
            ErrorCode::InvalidParameter,
            format!("Idempotency-Key is 1 to {MAX_KEY} printable ASCII characters, sent once"),
        )
        .with("parameter", "Idempotency-Key")
    };
    if values.next().is_some() {
        return Err(bad());
    }
    let text = value.to_str().map_err(|_| bad())?;
    if text.is_empty() || text.len() > MAX_KEY || !text.bytes().all(|b| (0x20..=0x7e).contains(&b))
    {
        return Err(bad());
    }
    Ok(Some(text.to_owned()))
}

/// JB9: the SHA-256 of the method, the path and the body as sent,
/// normalized for syntax only: parsed, then written with its keys sorted,
/// no whitespace and each number as parsed. No reference is resolved, so a
/// bare `sk_X` stays `sk_X`.
pub fn fingerprint(method: &str, path: &str, body: &[u8]) -> String {
    let normalized = serde_json::from_slice::<Value>(body)
        .map(|v| v.to_string())
        .unwrap_or_default();
    fitment_core::sha256_hex(format!("{method}\n{path}\n{normalized}").as_bytes())
}

/// JB11: `Prefer: wait=N`, if sent (RFC 7240), in seconds.
pub fn prefer_wait(headers: &HeaderMap) -> Option<u64> {
    headers
        .get_all("prefer")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split([',', ';']))
        .find_map(|p| {
            let (name, value) = p.trim().split_once('=')?;
            name.trim()
                .eq_ignore_ascii_case("wait")
                .then(|| value.trim().trim_matches('"').parse::<u64>().ok())
                .flatten()
        })
}

/// A request's normalized form (RA9): defaults filled, references
/// resolved, written compact in its fields' fixed order; its SHA-256 is
/// `request_sha256` (JB9).
pub fn normalized<T: Serialize>(value: &T) -> (String, String) {
    let text = serde_json::to_string(value).unwrap_or_default();
    let sha = fitment_core::sha256_hex(text.as_bytes());
    (text, sha)
}

/// `features`' normalized request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeaturesJob {
    /// `sha256:HEX`.
    pub upload: String,
}

/// `assembly_read`'s normalized request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssemblyJob {
    /// `sha256:HEX`.
    pub upload: String,
    /// RA8's name, or null.
    pub file_name: Option<String>,
}

/// `socket_extract`'s normalized request: `carry_from` resolved to an
/// exact revision or an upload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SocketJob {
    /// `sha256:HEX`.
    pub assembly: String,
    #[allow(missing_docs)]
    pub part: Option<String>,
    #[allow(missing_docs)]
    pub occurrence: Option<String>,
    #[allow(missing_docs)]
    pub carry: bool,
    /// `"sk_…@rN"`, `{"upload": "sha256:…"}` or null.
    pub carry_from: Option<Value>,
}

/// `socket_import`'s normalized request (RA9, RA10): the document's
/// upload, the line it names, and its assembly when the caller registered
/// it, which the job extracts from.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportJob {
    /// `sha256:HEX`, the document.
    pub upload: String,
    /// `sk_…`, the line its triple names (RA4).
    pub line: String,
    /// `sha256:HEX` of the document's source, if the caller registered it.
    pub assembly: Option<String>,
}

/// A file candidate as a match job records it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileJob {
    /// `sha256:HEX`.
    pub upload: String,
    #[allow(missing_docs)]
    pub label: String,
}

/// A component as a match job records it: its label null for the default.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentJob {
    /// `sha256:HEX`.
    pub assembly: String,
    #[allow(missing_docs)]
    pub part: Option<String>,
    #[allow(missing_docs)]
    pub occurrence: Option<String>,
    #[allow(missing_docs)]
    pub label: Option<String>,
}

/// DC7's policy, defaults filled.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyJob {
    #[allow(missing_docs)]
    pub tolerance_mm: f64,
    #[allow(missing_docs)]
    pub tolerance_deg: f64,
    #[allow(missing_docs)]
    pub clearance: crate::matching::ClearanceName,
}

/// A match's options, `require_reviewed` with the floor applied (SN3) and
/// who asked for it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OptionsJob {
    #[allow(missing_docs)]
    pub require_fit: bool,
    #[allow(missing_docs)]
    pub require_reviewed: bool,
    /// `"request"`, `"server"` or null (M6-12).
    pub require_reviewed_by: Option<String>,
}

/// `match`'s normalized request (RA9): the socket an exact revision or an
/// upload, with its document's object; every candidate by its SHA-256.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchJob {
    /// `"sk_…@rN"` or `{"upload": "sha256:…"}`.
    pub socket: Value,
    /// The socket document's object, `sha256:HEX`.
    pub socket_document: String,
    #[allow(missing_docs)]
    pub files: Vec<FileJob>,
    #[allow(missing_docs)]
    pub components: Vec<ComponentJob>,
    #[allow(missing_docs)]
    pub policy: PolicyJob,
    #[allow(missing_docs)]
    pub options: OptionsJob,
    /// The socket's own assembly, `sha256:HEX`, when the original is
    /// judged (SN9).
    pub original: Option<String>,
}

impl MatchJob {
    /// How many candidates it names, files and components (JS2's window).
    pub fn candidates(&self) -> usize {
        self.files.len() + self.components.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    /// HX5, RA5: socket references in ID9's grammar.
    #[test]
    fn socket_references_are_strict() {
        let id = "sk_0123456789abcdef0123";
        assert_eq!(SocketRef::parse(id).unwrap().n, None);
        assert_eq!(SocketRef::parse(&format!("{id}@r12")).unwrap().n, Some(12));
        for bad in [
            "sk_0123456789ABCDEF0123",
            "sk_0123456789abcdef012",
            "sk_0123456789abcdef0123@r0",
            "sk_0123456789abcdef0123@r01",
            "sk_0123456789abcdef0123@R1",
            "sk_0123456789abcdef0123@1",
            "sk_0123456789abcdef0123@r",
            "sk_0123456789abcdef0123@r1x",
            "x_0123456789abcdef0123",
        ] {
            assert!(SocketRef::parse(bad).is_err(), "{bad}");
        }
    }

    /// RA4: the id depends on the data alone.
    #[test]
    fn a_line_id_is_its_triples() {
        let a = line_id(&"a".repeat(64), "#30", "*");
        assert_eq!(a, line_id(&"a".repeat(64), "#30", "*"));
        assert_ne!(a, line_id(&"a".repeat(64), "#31", "*"));
        assert_eq!(a.len(), 23);
    }

    /// JB9: the fingerprint is over syntax: keys reordered and whitespace
    /// changed give the same; another value another.
    #[test]
    fn the_fingerprint_is_syntax_normalized() {
        let a = fingerprint("POST", "/v1/sockets", br#"{"part": "X", "assembly": "a"}"#);
        let b = fingerprint("POST", "/v1/sockets", br#"{"assembly":"a","part":"X"}"#);
        let c = fingerprint("POST", "/v1/sockets", br#"{"assembly":"a","part":"Y"}"#);
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    /// JB11: `Prefer: wait=N` among other preferences.
    #[test]
    fn prefer_wait_is_read() {
        let mut headers = HeaderMap::new();
        assert_eq!(prefer_wait(&headers), None);
        headers.insert("prefer", HeaderValue::from_static("respond-async, wait=10"));
        assert_eq!(prefer_wait(&headers), Some(10));
        headers.insert("prefer", HeaderValue::from_static("wait=x"));
        assert_eq!(prefer_wait(&headers), None);
    }

    /// RA8: names are refused, never repaired.
    #[test]
    fn names_follow_ra8() {
        assert!(file_name("s01_bolted.stp").is_ok());
        for bad in [
            "",
            ".",
            "..",
            "a/b",
            "a\\b",
            "a\0b",
            "a\u{1b}b",
            &"x".repeat(256),
        ] {
            assert!(file_name(bad).is_err(), "{bad:?}");
        }
        assert!(text("cand/c00_original.stp", "label").is_ok());
    }

    /// VN6: `carry_from` is a string or `{upload}`, nothing else.
    #[test]
    fn carry_from_is_one_of_two_forms() {
        let read = |t: &str| serde_json::from_str::<CarryFrom>(t);
        assert_eq!(
            read(r#""sk_x""#).unwrap(),
            CarryFrom::Revision("sk_x".into())
        );
        assert!(matches!(
            read(r#"{"upload": "sha256:a"}"#).unwrap(),
            CarryFrom::Upload { .. }
        ));
        assert!(read(r#"{"upload": "a", "upload": "b"}"#).is_err());
        assert!(read(r#"{"other": "a"}"#).is_err());
        assert!(read("{}").is_err());
        assert!(read("1").is_err());
    }
}
