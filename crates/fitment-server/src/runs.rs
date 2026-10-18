//! The API's documents of a match (docs/research/m6-api.md, DC10, DC12,
//! M6-25, M6-59, section 7.11): `fitment.run`, the stored record of a
//! match, built from its job's row and the facts its end recorded, never
//! from its document; and the candidates view, pages sliced from the
//! document's stored bytes through a byte-offset index built once, in one
//! pass, when the job stores the document, so no page parses the
//! document, whatever its size.

use serde::{Deserialize, Serialize};

use crate::jobs::model::State;
use crate::matching::PolicyFacts;
use crate::state::JobRow;

/// `fitment.run`.
pub const RUN_FORMAT: &str = "fitment.run";

/// Its version (DC9: may grow within v1).
pub const RUN_FORMAT_VERSION: u32 = 1;

/// A candidates index's format, an object of the store (DC12).
pub const INDEX_FORMAT: &str = "fitment.candidates-index";

/// The kind of a page of a match's candidates.
pub const CANDIDATE_KIND: &str = "match_candidate";

/// How many candidates got each verdict, as `fitment.match`'s `summary`
/// states them (SN4: copied, never recomputed).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SummaryFacts {
    /// Distinct candidates.
    pub candidates: usize,
    /// `Match`.
    #[serde(rename = "match")]
    pub matched: usize,
    /// `InterfaceMatch`.
    pub interface_match: usize,
    /// `Reject`, refused ones included.
    pub reject: usize,
    /// Refused by M2's gate.
    pub refused: usize,
}

impl SummaryFacts {
    /// The facts of core's summary.
    pub fn of(summary: &fitment_core::Summary) -> Self {
        Self {
            candidates: summary.candidates,
            matched: summary.matched,
            interface_match: summary.interface_match,
            reject: summary.reject,
            refused: summary.refused,
        }
    }
}

/// What a match's end records of its run (DC10), as `server.db`'s
/// `job.run`: what `fitment.run` states and the row does not.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunFacts {
    /// The socket document's SHA-256, digest and source.
    pub socket_sha256: String,
    #[allow(missing_docs)]
    pub socket_digest: Option<String>,
    #[allow(missing_docs)]
    pub socket_assembly: String,
    /// Its unreviewed requirements (SN8).
    pub unreviewed: usize,
    /// The files and components asked for, and the distinct candidates.
    pub files: usize,
    #[allow(missing_docs)]
    pub components: usize,
    #[allow(missing_docs)]
    pub distinct: usize,
    /// The document's summary and digest.
    pub summary: SummaryFacts,
    #[allow(missing_docs)]
    pub digest: String,
    /// The policy, as the document states it.
    pub policy: PolicyFacts,
    /// The fit verifier, as the document names it.
    pub fit_verifier: Option<String>,
    /// The candidates that are fitment bugs, by label.
    pub bugs: Vec<String>,
    /// Why the original fails the run, if it does.
    pub original: Option<String>,
    /// The original was judged.
    pub original_judged: bool,
    /// The options as the run applied them: the floor at dispatch (SN3).
    pub options: RunOptions,
    /// The build's versions that ran it (SV5).
    pub versions: RunVersions,
}

/// One candidate's place in its document (DC12): its bytes' range and
/// their SHA-256.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexEntry {
    /// The first byte, and one past the last.
    pub start: u64,
    #[allow(missing_docs)]
    pub end: u64,
    /// `sha256` hex of those bytes.
    pub sha256: String,
}

/// A document's candidates index (DC12), stored as an object beside it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidatesIndex {
    /// [`INDEX_FORMAT`].
    pub format: String,
    /// 1.
    pub format_version: u32,
    /// The document's object.
    pub document: String,
    /// Each element of its `candidates`, in order.
    pub entries: Vec<IndexEntry>,
}

/// A byte scanner over a JSON text, enough to find the top-level key
/// `candidates` and its array's elements without building any value.
struct Scan<'a> {
    text: &'a [u8],
    at: usize,
}

impl Scan<'_> {
    fn skip_ws(&mut self) {
        while self.text.get(self.at).is_some_and(u8::is_ascii_whitespace) {
            self.at += 1;
        }
    }

    fn eat(&mut self, byte: u8) -> Result<(), &'static str> {
        self.skip_ws();
        if self.text.get(self.at) == Some(&byte) {
            self.at += 1;
            Ok(())
        } else {
            Err("not the document's structure")
        }
    }

    /// A string, from its opening quote: its raw bytes between the quotes.
    fn string(&mut self) -> Result<&[u8], &'static str> {
        self.eat(b'"')?;
        let start = self.at;
        while let Some(&b) = self.text.get(self.at) {
            self.at += 1;
            match b {
                b'\\' => self.at += 1,
                b'"' => return Ok(&self.text[start..self.at - 1]),
                _ => {}
            }
        }
        Err("an unterminated string")
    }

    /// One value, any kind, skipped whole: its range.
    fn value(&mut self) -> Result<(usize, usize), &'static str> {
        self.skip_ws();
        let start = self.at;
        match self.text.get(self.at) {
            Some(b'"') => {
                self.string()?;
            }
            Some(b'{' | b'[') => {
                let mut depth = 0_usize;
                while let Some(&b) = self.text.get(self.at) {
                    match b {
                        b'"' => {
                            self.string()?;
                            continue;
                        }
                        b'{' | b'[' => depth += 1,
                        b'}' | b']' => {
                            depth -= 1;
                            if depth == 0 {
                                self.at += 1;
                                return Ok((start, self.at));
                            }
                        }
                        _ => {}
                    }
                    self.at += 1;
                }
                return Err("an unclosed value");
            }
            Some(_) => {
                while self
                    .text
                    .get(self.at)
                    .is_some_and(|b| !matches!(b, b',' | b'}' | b']') && !b.is_ascii_whitespace())
                {
                    self.at += 1;
                }
            }
            None => return Err("a missing value"),
        }
        Ok((start, self.at))
    }

    /// The elements of the array at the cursor, each its range; between
    /// them nothing but whitespace and one comma (the writer's separators).
    fn elements(&mut self) -> Result<Vec<(usize, usize)>, &'static str> {
        self.eat(b'[')?;
        let mut out = Vec::new();
        self.skip_ws();
        if self.text.get(self.at) == Some(&b']') {
            self.at += 1;
            return Ok(out);
        }
        loop {
            out.push(self.value()?);
            self.skip_ws();
            match self.text.get(self.at) {
                Some(b',') => self.at += 1,
                Some(b']') => {
                    self.at += 1;
                    return Ok(out);
                }
                _ => return Err("an array element not followed by a separator"),
            }
        }
    }
}

/// DC12: the ranges of the top-level `candidates` array's elements of a
/// `fitment.match` document, in one pass over its bytes, checked to cover
/// the array exactly, in order; each element with the SHA-256 of its bytes.
///
/// # Errors
///
/// A sentence when the bytes are not a document of that shape.
pub fn index(document: &[u8]) -> Result<Vec<IndexEntry>, &'static str> {
    let mut scan = Scan {
        text: document,
        at: 0,
    };
    scan.eat(b'{')?;
    loop {
        scan.skip_ws();
        if scan.text.get(scan.at) == Some(&b'}') {
            return Err("the document holds no candidates");
        }
        let key = scan.string()?.to_vec();
        scan.eat(b':')?;
        if key == b"candidates" {
            scan.skip_ws();
            let ranges = scan.elements()?;
            return Ok(ranges
                .into_iter()
                .map(|(start, end)| IndexEntry {
                    start: u64::try_from(start).unwrap_or(u64::MAX),
                    end: u64::try_from(end).unwrap_or(u64::MAX),
                    sha256: fitment_core::sha256_hex(&document[start..end]),
                })
                .collect());
        }
        scan.value()?;
        scan.eat(b',')?;
    }
}

/// `fitment.run`'s socket (DC10, RA10).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RunSocket {
    /// The revision judged, `sk_…@rN`, or null for an uploaded document.
    #[serde(rename = "ref")]
    pub reference: Option<String>,
    /// The uploaded document, `sha256:HEX`, or null.
    pub upload: Option<String>,
    /// The socket document's bytes' SHA-256.
    pub sha256: Option<String>,
    /// Its own `digest` (DC5).
    pub digest: Option<String>,
    /// Its source assembly, `sha256:HEX`.
    pub assembly: Option<String>,
    /// How the revision was made: `extract`, `review`, `import`; null for
    /// an upload.
    pub how: Option<String>,
    /// RA10: extracted by this server from the caller's assembly.
    pub verified: bool,
    /// Its unreviewed requirements (SN8).
    pub unreviewed: Option<usize>,
}

/// `fitment.run`'s candidates of a file run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RunCandidates {
    /// `files`.
    pub kind: &'static str,
    /// The files and components asked for.
    pub files: usize,
    #[allow(missing_docs)]
    pub components: usize,
    /// The document's distinct candidates (MF3, CO3), once judged.
    pub distinct: Option<usize>,
}

/// `fitment.run`'s options (SN3, M6-12): as the run applied them, the
/// floor as it stood at dispatch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RunOptions {
    #[allow(missing_docs)]
    pub require_fit: bool,
    #[allow(missing_docs)]
    pub require_reviewed: bool,
    /// `"request"`, `"server"` or null.
    pub require_reviewed_by: Option<String>,
}

/// `fitment.run`'s original (VD15, SN9).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RunOriginal {
    /// The socket's own assembly, `sha256:HEX`.
    pub assembly: String,
    /// It was judged; its entry is the document's `original`.
    pub judged: bool,
}

/// `fitment.run`'s versions (SV5): the build's that ran the match,
/// recorded at the run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RunVersions {
    #[allow(missing_docs)]
    pub extractor_version: u32,
    #[allow(missing_docs)]
    pub socket_rules_version: u32,
    #[allow(missing_docs)]
    pub signature_version: u32,
    #[allow(missing_docs)]
    pub match_rules_version: u32,
    /// The server's version.
    pub server: String,
}

impl RunVersions {
    /// The versions of the build that runs a match now, its socket
    /// versions `build`'s (MF11).
    pub fn of(build: (u32, u32)) -> Self {
        Self {
            extractor_version: build.0,
            socket_rules_version: build.1,
            signature_version: fitment_core::SIGNATURE_VERSION,
            match_rules_version: fitment_core::MATCH_RULES_VERSION,
            server: env!("CARGO_PKG_VERSION").to_owned(),
        }
    }
}

/// `fitment.run`'s problems (M6-59): what makes the run exit 1, and for a
/// run that did not succeed the counts of what its kept document judged.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RunProblems {
    /// Damaged catalog rows (QR4): none in a file run.
    pub corrupt: Vec<String>,
    /// The candidates that are fitment bugs, by label.
    pub bugs: Vec<String>,
    /// Why the original fails the run.
    pub original: Option<String>,
    /// A run that did not succeed: its kept document's counts, never in
    /// `summary`.
    pub incomplete_summary: Option<SummaryFacts>,
}

/// `fitment.run`'s link to its document (DC1, DC5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RunDocumentLink {
    /// `/v1/matches/{id}/document`.
    pub href: String,
    /// The bytes' SHA-256 and length.
    pub sha256: String,
    #[allow(missing_docs)]
    pub bytes: u64,
    /// The document's own `digest`.
    pub digest: String,
}

/// `fitment.run` v1 (DC10, section 7.11). It holds no time, so two
/// servers' records of one run differ only in ids.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RunDocument {
    /// [`RUN_FORMAT`].
    pub format: &'static str,
    /// 1.
    pub format_version: u32,
    /// The match's id, its job's.
    pub id: String,
    #[allow(missing_docs)]
    pub socket: RunSocket,
    #[allow(missing_docs)]
    pub candidates: RunCandidates,
    /// As the document states it.
    pub policy: PolicyFacts,
    /// As the run applied them, the floor at dispatch; as asked for a run
    /// that recorded none.
    pub options: RunOptions,
    #[allow(missing_docs)]
    pub original: Option<RunOriginal>,
    /// The versions of the build that ran it, recorded at the run; null
    /// for a run that recorded none (it stored no document).
    pub versions: Option<RunVersions>,
    /// As the document names it; null without one (SN4, SN11).
    pub fit_verifier: Option<String>,
    /// The document's, for a run that succeeded only (M6-59).
    pub summary: Option<SummaryFacts>,
    /// Likewise.
    pub outcome: Option<String>,
    /// The CLI's exit code (JB4).
    pub exit_code: Option<u8>,
    /// True only for a run that succeeded.
    pub complete: bool,
    #[allow(missing_docs)]
    pub problems: RunProblems,
    /// The document, if one was kept.
    pub document: Option<RunDocumentLink>,
}

/// What `fitment.run` takes from the job's request and its revision: the
/// socket as asked, the counts asked for, the policy and options.
#[derive(Debug, Clone)]
pub struct Asked {
    /// The socket's block, before the run's facts complete it.
    pub socket: RunSocket,
    #[allow(missing_docs)]
    pub files: usize,
    #[allow(missing_docs)]
    pub components: usize,
    #[allow(missing_docs)]
    pub policy: PolicyFacts,
    #[allow(missing_docs)]
    pub options: RunOptions,
    /// The original's assembly, when asked.
    pub original: Option<String>,
}

/// `fitment.run` of `row` (DC10): `summary` and `outcome` only for a run
/// that succeeded, else null and the kept document's counts under
/// `problems.incomplete_summary` (M6-59, SN4).
pub fn run(row: &JobRow, asked: Asked) -> RunDocument {
    let facts: Option<RunFacts> = row
        .run
        .as_deref()
        .and_then(|r| serde_json::from_str(r).ok());
    let succeeded = row.state == State::Succeeded;
    let mut socket = asked.socket;
    if let Some(f) = &facts {
        socket.sha256 = Some(f.socket_sha256.clone());
        socket.digest.clone_from(&f.socket_digest);
        socket.assembly = Some(format!("sha256:{}", f.socket_assembly));
        socket.unreviewed = Some(f.unreviewed);
    }
    let document = match (&row.result_sha256, &facts) {
        (Some(sha256), Some(f)) => Some(RunDocumentLink {
            href: format!("/v1/matches/{}/document", row.id),
            sha256: sha256.clone(),
            bytes: row.result_bytes.unwrap_or(0),
            digest: f.digest.clone(),
        }),
        _ => None,
    };
    RunDocument {
        format: RUN_FORMAT,
        format_version: RUN_FORMAT_VERSION,
        id: row.id.clone(),
        socket,
        candidates: RunCandidates {
            kind: "files",
            files: asked.files,
            components: asked.components,
            distinct: facts.as_ref().map(|f| f.distinct),
        },
        policy: facts.as_ref().map_or(asked.policy, |f| f.policy.clone()),
        options: facts.as_ref().map_or(asked.options, |f| f.options.clone()),
        original: asked.original.map(|assembly| RunOriginal {
            assembly,
            judged: facts.as_ref().is_some_and(|f| f.original_judged),
        }),
        versions: facts.as_ref().map(|f| f.versions.clone()),
        fit_verifier: facts.as_ref().and_then(|f| f.fit_verifier.clone()),
        summary: facts.as_ref().filter(|_| succeeded).map(|f| f.summary),
        outcome: row.outcome.clone().filter(|_| succeeded),
        exit_code: row.exit,
        complete: succeeded,
        problems: RunProblems {
            corrupt: Vec::new(),
            bugs: facts.as_ref().map(|f| f.bugs.clone()).unwrap_or_default(),
            original: facts.as_ref().and_then(|f| f.original.clone()),
            incomplete_summary: facts
                .as_ref()
                .filter(|_| !succeeded && document.is_some())
                .map(|f| f.summary),
        },
        document,
    }
}

/// DC12's page, written by hand around the document's own bytes of each
/// entry, which are never parsed or re-serialized: `fitment.page` with
/// `{index, entry}` items, and the document's digest, `view: true` and the
/// run's `complete` and `exit_code` on every page (M6-59).
pub fn page(
    first: usize,
    entries: &[Vec<u8>],
    next: Option<&str>,
    digest: &str,
    complete: bool,
    exit_code: Option<u8>,
) -> Vec<u8> {
    let quoted = |s: &str| serde_json::to_string(s).unwrap_or_default();
    let mut out = format!(
        "{{\n  \"format\": \"{}\",\n  \"format_version\": {},\n  \"kind\": \"{CANDIDATE_KIND}\",\n  \"items\": [",
        crate::auth::documents::PAGE_FORMAT,
        crate::auth::documents::FORMAT_VERSION
    )
    .into_bytes();
    for (i, entry) in entries.iter().enumerate() {
        if i > 0 {
            out.push(b',');
        }
        out.extend_from_slice(
            format!(
                "\n    {{\n      \"index\": {},\n      \"entry\": ",
                first + i
            )
            .as_bytes(),
        );
        out.extend_from_slice(entry);
        out.extend_from_slice(b"\n    }");
    }
    if !entries.is_empty() {
        out.extend_from_slice(b"\n  ");
    }
    let tail = format!(
        "],\n  \"next\": {},\n  \"generation\": null,\n  \"digest\": {},\n  \"view\": true,\n  \"complete\": {complete},\n  \"exit_code\": {}\n}}\n",
        next.map_or_else(|| "null".to_owned(), quoted),
        quoted(digest),
        exit_code.map_or_else(|| "null".to_owned(), |c| c.to_string()),
    );
    out.extend_from_slice(tail.as_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// DC12: the index covers the array exactly, element by element, and
    /// refuses bytes of another shape.
    #[test]
    fn the_index_slices_the_candidates() {
        let doc = br#"{
  "format": "fitment.match",
  "socket": {"x": [1, {"y": "]"}]},
  "candidates": [
    {
      "labels": ["a\"]"],
      "n": 1
    },
    {
      "n": 2
    }
  ],
  "digest": "sha256:0"
}
"#;
        let entries = index(doc).unwrap();
        assert_eq!(entries.len(), 2);
        let at = |n: u64| usize::try_from(n).unwrap();
        let first = &doc[at(entries[0].start)..at(entries[0].end)];
        let value: serde_json::Value = serde_json::from_slice(first).unwrap();
        assert_eq!(value["n"], 1);
        assert_eq!(
            entries[1].sha256,
            fitment_core::sha256_hex(
                br#"{
      "n": 2
    }"#
            )
        );
        assert!(index(br#"{"candidates": [1 2]}"#).is_err());
        assert!(index(br#"{"other": []}"#).is_err());
        assert_eq!(index(br#"{"candidates": []}"#).unwrap(), Vec::new());
    }

    /// DC12: a page is JSON whose entries are the given bytes.
    #[test]
    fn a_page_wraps_the_bytes() {
        let page = page(
            3,
            &[b"{\"n\": 1}".to_vec()],
            Some("/next"),
            "sha256:d",
            false,
            Some(1),
        );
        let value: serde_json::Value = serde_json::from_slice(&page).unwrap();
        assert_eq!(value["items"][0]["index"], 3);
        assert_eq!(value["items"][0]["entry"]["n"], 1);
        assert_eq!(value["view"], true);
        assert_eq!(value["complete"], false);
        assert_eq!(value["exit_code"], 1);
        let empty: serde_json::Value =
            serde_json::from_slice(&super::page(0, &[], None, "d", true, Some(4))).unwrap();
        assert_eq!(empty["items"], serde_json::json!([]));
    }
}
