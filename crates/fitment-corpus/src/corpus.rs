//! The generated corpus and its manifest's match cases, as the suites read
//! them (`docs/research/m6-api.md`, SC4, SC7, SC8): one way to find the
//! corpus, and each case's socket, candidate, truth and expectations from
//! the construction alone (KC1).

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::gate;
use crate::outcome::Outcome;
use crate::policy::Policy;

/// The generated corpus: its directory and its manifest.
#[derive(Debug, Clone)]
pub struct Corpus {
    dir: PathBuf,
    manifest: Value,
}

impl Corpus {
    /// SC7: the corpus in `FITMENT_CORPUS`, or `corpus/out`; `None`, with a
    /// note on stderr, when it is not there, unless
    /// `FITMENT_REQUIRE_CORPUS=1`.
    ///
    /// # Panics
    ///
    /// When the corpus is required and missing, or its manifest is not
    /// JSON.
    pub fn locate() -> Option<Self> {
        let required = std::env::var_os("FITMENT_REQUIRE_CORPUS").is_some_and(|v| v == "1");
        Self::at(&Self::default_dir(), required)
    }

    /// Where the corpus is looked for: `FITMENT_CORPUS`, or `corpus/out`
    /// in the repository.
    pub fn default_dir() -> PathBuf {
        std::env::var_os("FITMENT_CORPUS").map_or_else(
            || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/out"),
            PathBuf::from,
        )
    }

    /// As [`Self::locate`], and `None` with a note in a debug build, where
    /// the suite would take too long.
    ///
    /// # Panics
    ///
    /// As [`Self::locate`].
    pub fn locate_release_only() -> Option<Self> {
        if cfg!(debug_assertions) {
            eprintln!("skipped: a debug build; run with --release");
            return None;
        }
        Self::locate()
    }

    /// The corpus in `dir`, or `None` with a note; a required corpus that
    /// is missing is a failure, never a pass (SC7).
    ///
    /// # Panics
    ///
    /// When `required` and `dir` holds no manifest, or the manifest is not
    /// JSON.
    pub fn at(dir: &Path, required: bool) -> Option<Self> {
        let path = dir.join("manifest.json");
        if !path.is_file() {
            assert!(
                !required,
                "FITMENT_REQUIRE_CORPUS=1 but there is no corpus in {}",
                dir.display()
            );
            eprintln!(
                "skipped: no corpus in {}; run tools/gen-corpus.py corpus/out",
                dir.display()
            );
            return None;
        }
        let text = fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
        let manifest = serde_json::from_str(&text)
            .unwrap_or_else(|e| panic!("{} is not JSON: {e}", path.display()));
        Some(Self::of(dir, manifest))
    }

    /// A corpus of this manifest in `dir`, as the scorer's own tests build
    /// one.
    pub fn of(dir: &Path, manifest: Value) -> Self {
        Self {
            dir: dir.to_path_buf(),
            manifest,
        }
    }

    /// Its directory.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Its manifest.
    pub fn manifest(&self) -> &Value {
        &self.manifest
    }

    /// Every case of the manifest, in its order.
    pub fn cases(&self) -> &[Value] {
        self.manifest["cases"].as_array().map_or(&[], Vec::as_slice)
    }

    /// The match case `id`.
    pub fn case(&self, id: &str) -> Option<Case<'_>> {
        self.match_cases().into_iter().find(|c| c.id() == id)
    }

    /// The match cases, in the manifest's order.
    pub fn match_cases(&self) -> Vec<Case<'_>> {
        self.cases()
            .iter()
            .filter(|c| c["kind"] == "match")
            .map(Case::new)
            .collect()
    }

    /// The match cases by the socket they are judged against: source,
    /// target, review and edit.
    pub fn socket_groups(&self) -> BTreeMap<SocketKey, Vec<Case<'_>>> {
        let mut out: BTreeMap<SocketKey, Vec<Case<'_>>> = BTreeMap::new();
        for case in self.match_cases() {
            out.entry(case.socket()).or_default().push(case);
        }
        out
    }

    /// XH8's ingest set: every `.stp` file of the corpus but the hostile
    /// `h_*` ones (CT10), sorted.
    ///
    /// # Panics
    ///
    /// When the directory cannot be listed.
    pub fn part_files(&self) -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = fs::read_dir(&self.dir)
            .unwrap_or_else(|e| panic!("cannot list {}: {e}", self.dir.display()))
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "stp"))
            .filter(|p| {
                p.file_name()
                    .is_some_and(|n| !n.to_string_lossy().starts_with("h_"))
            })
            .collect();
        out.sort();
        out
    }
}

/// What the construction says of a candidate against its socket (KC3,
/// SC8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Truth {
    /// It fits.
    Fits,
    /// It fits, and a stated rule rejects it.
    FnRule,
    /// It fits under D6 functional only.
    FitsFunctional,
    /// It does not fit.
    DoesNotFit,
    /// It sits on a stated limit, which neither side may claim.
    Edge,
    /// It fits every interface, and its body collides: M7's to reject.
    Collides,
    /// The gate refuses it, or its socket.
    Refused,
    /// Its only defect breaks a requirement its review removed: the user
    /// accepted it (KC13, M6-38).
    Waived,
}

impl Truth {
    /// The truth the manifest names.
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "fits" => Self::Fits,
            "fn_rule" => Self::FnRule,
            "fits_functional" => Self::FitsFunctional,
            "does_not_fit" => Self::DoesNotFit,
            "edge" => Self::Edge,
            "collides" => Self::Collides,
            "refused" => Self::Refused,
            "waived" => Self::Waived,
            _ => return None,
        })
    }

    /// SC8, KC3: whether it fits under `policy`. `fits`, `fn_rule` and
    /// `collides` fit, `fits_functional` under functional only; `waived` is
    /// neither, and outside both rates.
    pub fn fits(self, policy: Policy) -> bool {
        match self {
            Self::Fits | Self::FnRule | Self::Collides => true,
            Self::FitsFunctional => policy == Policy::Functional,
            Self::DoesNotFit | Self::Edge | Self::Refused | Self::Waived => false,
        }
    }
}

/// The socket a case is judged against: its source and target, the
/// review applied to it (as JSON) and any edit of the socket gate's.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct SocketKey {
    /// The source assembly's case.
    pub source: String,
    /// The target, as `--part` takes it.
    pub part: String,
    /// The occurrence, if one is named.
    pub occurrence: Option<String>,
    /// The review's entries, as compact JSON (`[]` for none).
    pub review: String,
    /// The socket gate's edit (`stale`, `digest`, `unreviewed`), if any.
    pub edit: Option<String>,
}

/// A case's candidate (KC2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CandidateRef {
    /// A file of the corpus.
    File(String),
    /// A leaf of a corpus assembly, read in place.
    Component {
        /// The assembly's case.
        source: String,
        /// The target, as `--part` takes it.
        part: String,
    },
}

/// One match case of the manifest.
#[derive(Debug, Clone, Copy)]
pub struct Case<'m> {
    value: &'m Value,
}

fn string(v: &Value) -> Option<String> {
    v.as_str().map(str::to_owned)
}

impl<'m> Case<'m> {
    /// The case the manifest entry `value` states.
    pub fn new(value: &'m Value) -> Self {
        Self { value }
    }

    /// Its manifest entry.
    pub fn value(&self) -> &'m Value {
        self.value
    }

    /// Its id.
    pub fn id(&self) -> &'m str {
        self.value["id"].as_str().unwrap_or("?")
    }

    /// Its truth.
    ///
    /// # Panics
    ///
    /// When the manifest names an unknown truth: a harness error.
    pub fn truth(&self) -> Truth {
        let name = self.value["truth"].as_str().unwrap_or("");
        Truth::parse(name)
            .unwrap_or_else(|| panic!("{}: harness: an unknown truth {name:?}", self.id()))
    }

    /// Its class, the report's row (KH6).
    pub fn class(&self) -> &'m str {
        self.value["class"].as_str().unwrap_or("")
    }

    /// The socket it is judged against.
    pub fn socket(&self) -> SocketKey {
        let s = &self.value["socket"];
        SocketKey {
            source: string(&s["case"]).unwrap_or_default(),
            part: string(&s["part"]).unwrap_or_default(),
            occurrence: (!s["occurrence"].is_null()).then(|| s["occurrence"].to_string()),
            review: s["review"]
                .as_array()
                .map_or_else(|| "[]".to_owned(), |r| Value::Array(r.clone()).to_string()),
            edit: string(&s["edit"]),
        }
    }

    /// The review its socket is given, in order (M6-38).
    pub fn review(&self) -> &'m [Value] {
        self.value["socket"]["review"]
            .as_array()
            .map_or(&[], Vec::as_slice)
    }

    /// Its candidate.
    pub fn candidate(&self) -> CandidateRef {
        let c = &self.value["candidate"];
        match string(&c["file"]) {
            Some(file) => CandidateRef::File(file),
            None => CandidateRef::Component {
                source: string(&c["component"]["case"]).unwrap_or_default(),
                part: string(&c["component"]["part"]).unwrap_or_default(),
            },
        }
    }

    /// What it expects under `policy` (KH2); null for a policy it states
    /// nothing for.
    pub fn expectation(&self, policy: Policy) -> &'m Value {
        gate::expectation(self.value, policy.name())
    }

    /// SC8: whether its truth fits under `policy`.
    pub fn fits(&self, policy: Policy) -> bool {
        self.truth().fits(policy)
    }

    /// VD9, MF2: a case of the socket gate, refused before any candidate.
    pub fn socket_gate(&self) -> bool {
        self.expectation(Policy::Default)["verdict"] == "socket_refused"
    }

    /// KH2, KH3 on `outcome` under `policy`, against the socket's
    /// requirements as extracted.
    pub fn gate(
        &self,
        policy: Policy,
        sockets: &[fitment_core::Socket],
        outcome: &Outcome,
    ) -> Vec<String> {
        gate::gate(self.value, policy.name(), sockets, outcome)
    }
}
