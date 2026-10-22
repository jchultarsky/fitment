//! JB10 (docs/research/m6-api.md): what the CLI writes on stderr for the
//! same run, as the job's `notes`, in the CLI's order: each `{code,
//! message, …fields}`, the message the CLI's line without its `fitment: `,
//! its file's name and its `warning: `, and with every "run `fitment …`"
//! replaced by the endpoint that does it (ER3). A note holds no path: the
//! names it shows are the client's own labels.

use fitment_core::{Refusal, ReviewStatus, SocketDocument};
use serde_json::{Map, Value, json};

use crate::matching::MatchNote;

/// A job's notes, in the order they were made.
#[derive(Debug, Clone, Default)]
pub(crate) struct Notes(Vec<Value>);

/// `text` with every control character escaped, and the separators and
/// bidirectional controls that split or reorder a line, as the CLI's
/// `printable` writes a line (JB10's messages are its lines).
pub(crate) fn printable(text: &str) -> String {
    let unsafe_format = |c: char| {
        matches!(
            c,
            '\u{2028}' | '\u{2029}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'
        )
    };
    text.chars()
        .flat_map(|c| {
            let escaped: Vec<char> = if c.is_control() || unsafe_format(c) {
                c.escape_default().collect()
            } else {
                vec![c]
            };
            escaped
        })
        .collect()
}

/// A length for people, as the CLI writes one: up to 4 decimals, trailing
/// zeros dropped.
fn mm(x: f64) -> String {
    let text = format!("{x:.4}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text == "-0" {
        "0".to_owned()
    } else {
        text.to_owned()
    }
}

impl Notes {
    /// Adds a note of `code` and `message`, with `fields` after them.
    pub(crate) fn push(&mut self, code: &str, message: String, fields: Map<String, Value>) {
        let mut note = Map::new();
        note.insert("code".into(), Value::String(code.to_owned()));
        note.insert("message".into(), Value::String(message));
        note.extend(fields);
        self.0.push(Value::Object(note));
    }

    /// The CLI's line per refusal of a document that refused its input
    /// (`fitment features`, `parts`, `socket`).
    pub(crate) fn refusals(&mut self, refusals: &[Refusal]) {
        for refusal in refusals {
            let mut fields = Map::new();
            fields.insert("refusal".into(), json!(refusal.code.name()));
            self.push(
                "refused",
                format!(
                    "refused: {}: {}",
                    refusal.code.name(),
                    printable(&refusal.message)
                ),
                fields,
            );
        }
    }

    /// `fitment socket … -o OUT`'s line when decisions were carried
    /// (MF11): how many, and how many went stale.
    pub(crate) fn carried(&mut self, carried: usize, stale: usize) {
        if carried + stale > 0 {
            let mut fields = Map::new();
            fields.insert("carried".into(), json!(carried));
            fields.insert("stale".into(), json!(stale));
            self.push(
                "carried_over",
                format!("{carried} decisions carried over, {stale} stale"),
                fields,
            );
        }
    }

    /// `fitment socket`'s summary of an accepted document, and its
    /// warnings: a socket with no requirement, one read only coarsely, a
    /// requirement no candidate can be checked against.
    pub(crate) fn socket(&mut self, document: &SocketDocument) {
        let Some(extracted) = document.extracted() else {
            return;
        };
        let requirements: usize = extracted.sockets.iter().map(|s| s.requirements.len()).sum();
        let left: usize = extracted.sockets.iter().map(|s| s.left_out.len()).sum();
        let name = extracted
            .target
            .definition
            .product
            .as_ref()
            .and_then(|p| p.id.as_deref())
            .unwrap_or("");
        let decided = document
            .review()
            .iter()
            .filter(|e| e.status != ReviewStatus::Unreviewed)
            .count();
        self.push(
            "summary",
            format!(
                "{}: {} occurrences, {} sockets, {requirements} requirements ({} unreviewed), {left} left out",
                printable(name),
                extracted.occurrences.len(),
                extracted.sockets.len(),
                requirements.saturating_sub(decided)
            ),
            Map::new(),
        );
        for socket in &extracted.sockets {
            let at = |extra: &[(&str, Value)]| {
                let mut fields = Map::new();
                fields.insert("socket".into(), json!(socket.id));
                for (key, value) in extra {
                    fields.insert((*key).into(), value.clone());
                }
                fields
            };
            if socket.requirements.is_empty() {
                self.push(
                    "empty_socket",
                    format!(
                        "{}: no requirement; every candidate is rejected for it",
                        socket.id
                    ),
                    at(&[]),
                );
            }
            let floor = socket
                .requirements
                .iter()
                .map(|r| r.floor_mm)
                .fold(0.0, f64::max);
            if floor > 0.01 {
                self.push(
                    "coarse_floor",
                    format!("{}: read only to within {} mm", socket.id, mm(floor)),
                    at(&[("floor_mm", json!(floor))]),
                );
            }
            for requirement in socket.requirements.iter().filter(|r| !r.verifiable()) {
                self.push(
                    "unverifiable_requirement",
                    format!(
                        "{}: {}: no candidate can be checked against it; every candidate fails it until it is removed in review",
                        socket.id, requirement.id
                    ),
                    at(&[("requirement", json!(requirement.id))]),
                );
            }
        }
    }

    /// A match's lines (`fitment match`'s stderr), from what the judging
    /// noted, in its order.
    pub(crate) fn matched(&mut self, notes: &[MatchNote]) {
        for note in notes {
            let (code, message, fields) = match_note(note);
            self.push(code, message, fields);
        }
    }

    /// RA10's note, which no CLI run writes: the socket judged was imported
    /// without its assembly.
    pub(crate) fn unverified_socket(&mut self) {
        self.push(
            "unverified_socket",
            "the socket was imported without its assembly; the server did not extract it".into(),
            Map::new(),
        );
    }

    /// The notes as a JSON array, for `server.db`; `None` when there are
    /// none, which leaves the row's `[]`.
    pub(crate) fn to_json(&self) -> Option<String> {
        (!self.0.is_empty()).then(|| Value::Array(self.0.clone()).to_string())
    }
}

fn label_field(label: &str) -> Map<String, Value> {
    let mut fields = Map::new();
    fields.insert("label".into(), json!(label));
    fields
}

/// One of `fitment match`'s lines: its code, message and fields.
fn match_note(note: &MatchNote) -> (&'static str, String, Map<String, Value>) {
    let socket = |id: &str| {
        let mut fields = Map::new();
        fields.insert("socket".into(), json!(id));
        fields
    };
    match note {
        MatchNote::Summary(text) => ("summary", text.clone(), Map::new()),
        MatchNote::EmptySocket(id) => (
            "empty_socket",
            format!(
                "{}: no requirement is active, so every candidate is rejected",
                printable(id)
            ),
            socket(id),
        ),
        MatchNote::RejectsEvery(id, requirements) => {
            let mut fields = socket(id);
            fields.insert("requirements".into(), json!(requirements));
            (
                "rejects_every_candidate",
                format!(
                    "{}: {} fail every candidate until removed in review (POST /v1/sockets/{{socket}}/review)",
                    printable(id),
                    printable(&requirements.join(", "))
                ),
                fields,
            )
        }
        MatchNote::Unreviewed(count) => {
            let mut fields = Map::new();
            fields.insert("count".into(), json!(count));
            (
                "unreviewed_requirements",
                format!("{count} requirements are unreviewed"),
                fields,
            )
        }
        MatchNote::RequireFit => (
            "require_fit_impossible",
            "require_fit: no candidate can pass without a fit verifier in this build".into(),
            Map::new(),
        ),
        MatchNote::Refused(label, code, message) => {
            let mut fields = label_field(label);
            fields.insert("refusal".into(), json!(code));
            (
                "candidate_refused",
                format!("refused: {code}: {}", printable(message)),
                fields,
            )
        }
        MatchNote::ShortlistUnsound(label) => (
            "shortlist_unsound",
            "the shortlist dropped a candidate verification passes: this is a fitment bug".into(),
            label_field(label),
        ),
        MatchNote::JudgingFailed(label) => (
            "judging_failed",
            "judging it failed: this is a fitment bug".into(),
            label_field(label),
        ),
        MatchNote::OriginalUnreadable => (
            "original_unreadable",
            "the original cannot be read".into(),
            Map::new(),
        ),
        MatchNote::OriginalExhausted(at) => (
            "original_exhausted",
            format!(
                "the original exhausts the search at {}: not checked",
                printable(at)
            ),
            Map::new(),
        ),
        MatchNote::OriginalFails(at) => (
            "original_fails_own_socket",
            format!(
                "the original fails its own socket at {}: this is a fitment bug",
                printable(at)
            ),
            Map::new(),
        ),
    }
}
