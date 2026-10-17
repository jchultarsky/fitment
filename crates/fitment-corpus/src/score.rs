//! What the corpus's runs come to (`docs/research/m6-api.md`, SC1, SC5,
//! SC8; `docs/research/m4-match.md`, KH4, KH6, KH7): the false positives,
//! the false negatives as the baseline states them, and the report's rows
//! and rates, the same whichever path judged the runs.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde_json::{Value, json};

use crate::baseline;
use crate::corpus::{Corpus, Truth};
use crate::outcome::Outcome;
use crate::policy::Policy;

/// The path a run's outcome came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunPath {
    /// `fitment-core`'s typed results.
    Library,
    /// The built `fitment`'s documents.
    Binary,
    /// A catalog run's documents.
    Catalog,
    /// The server's documents.
    Api,
}

/// One case judged under one policy.
#[derive(Debug, Clone)]
pub struct Run {
    /// The case's id.
    pub case: String,
    /// The policy.
    pub policy: Policy,
    /// What it came to.
    pub outcome: Outcome,
    /// Where from.
    pub path: RunPath,
    /// KH8: whether every check passes at the construction's placement,
    /// which only the library can say (`verify_at`); `None` elsewhere, and
    /// a false negative's cause is then the baseline's (SC5).
    pub at_construction: Option<bool>,
}

struct Scored {
    run: Run,
    truth: Truth,
    class: String,
}

impl Scored {
    fn rejected(&self) -> bool {
        self.run.outcome.verdict == "reject"
    }

    /// KH7: a pass the construction says does not fit; a waived case's
    /// pass is the user's to accept, never one (SC8).
    fn false_positive(&self) -> bool {
        !self.rejected() && !self.truth.fits(self.run.policy) && self.truth != Truth::Waived
    }

    /// KH4: a fit rejected.
    fn false_negative(&self) -> bool {
        self.rejected() && self.truth.fits(self.run.policy)
    }
}

/// The runs of one path, scored against the construction.
pub struct Score {
    runs: Vec<Scored>,
}

impl Score {
    /// The runs, each with its case's truth and class from `corpus`.
    ///
    /// # Panics
    ///
    /// When a run names a case the manifest does not hold: a harness
    /// error.
    pub fn new(corpus: &Corpus, runs: Vec<Run>) -> Self {
        let runs = runs
            .into_iter()
            .map(|run| {
                let case = corpus
                    .case(&run.case)
                    .unwrap_or_else(|| panic!("harness: no match case {}", run.case));
                Scored {
                    truth: case.truth(),
                    class: case.class().to_owned(),
                    run,
                }
            })
            .collect();
        Self { runs }
    }

    /// The runs, in the order given.
    pub fn runs(&self) -> impl Iterator<Item = &Run> {
        self.runs.iter().map(|s| &s.run)
    }

    /// The cases run, in the order given, each once in a row of its own
    /// runs.
    pub fn cases(&self) -> Vec<Value> {
        let mut ids: Vec<Value> = self.runs.iter().map(|s| json!(s.run.case)).collect();
        ids.dedup();
        ids
    }

    /// KH7: every false positive, as `case policy`.
    pub fn false_positives(&self) -> Vec<String> {
        self.runs
            .iter()
            .filter(|s| s.false_positive())
            .map(|s| format!("\"{}\" {}", s.run.case, s.run.policy.name()))
            .collect()
    }

    /// KH4: one entry per (case, policy) that fits and is rejected: the
    /// requirements that failed, with their codes, and where the rejection
    /// comes from: a stated rule, alignment (the construction's placement
    /// passes, KH8) or verification; taken from the baseline where the
    /// path cannot place by the construction (SC5).
    pub fn false_negatives(&self) -> Vec<Value> {
        let committed = baseline::committed();
        let mut out: Vec<Value> = self
            .runs
            .iter()
            .filter(|s| s.false_negative())
            .map(|s| {
                let failed: Vec<Value> = s
                    .run
                    .outcome
                    .sockets
                    .iter()
                    .flat_map(|socket| {
                        socket.failed.iter().map(move |(id, codes)| {
                            let mut codes = codes.clone();
                            codes.sort();
                            codes.dedup();
                            json!({"requirement": format!("{}/{id}", socket.socket), "codes": codes})
                        })
                    })
                    .collect();
                let cause = match (s.truth, s.run.at_construction) {
                    (Truth::FnRule | Truth::FitsFunctional, _) => "rule".to_owned(),
                    (_, Some(true)) => "alignment".to_owned(),
                    (_, Some(false)) => "verification".to_owned(),
                    (_, None) => baseline::cause(committed.as_ref(), &s.run.case, s.run.policy)
                        .unwrap_or_else(|| "unknown".to_owned()),
                };
                json!({"case": s.run.case, "policy": s.run.policy.name(), "class": s.class,
                       "failed": failed, "cause": cause})
            })
            .collect();
        out.sort_by_key(|v| (v["case"].to_string(), v["policy"].to_string()));
        out
    }

    fn policies(&self) -> Vec<Policy> {
        let mut out: Vec<Policy> = self.runs.iter().map(|s| s.run.policy).collect();
        out.sort();
        out.dedup();
        out
    }

    /// Per (policy, class): cases, match, interface match, reject, false
    /// positives, false negatives against physics and against the
    /// specification.
    fn rows(&self) -> BTreeMap<(String, String), [usize; 7]> {
        let mut rows: BTreeMap<(String, String), [usize; 7]> = BTreeMap::new();
        for s in &self.runs {
            let row = rows
                .entry((s.run.policy.name().to_owned(), s.class.clone()))
                .or_default();
            row[0] += 1;
            match s.run.outcome.verdict.as_str() {
                "match" => row[1] += 1,
                "interface_match" => row[2] += 1,
                _ => row[3] += 1,
            }
            if s.false_positive() {
                row[4] += 1;
            }
            if s.false_negative() {
                row[5] += 1;
                if s.truth == Truth::Fits {
                    row[6] += 1;
                }
            }
        }
        rows
    }

    /// KH6, KH7: the rows by policy and class, and per policy the false
    /// positives and the false negatives against physics (`collides`
    /// aside, M7's).
    pub fn report(&self) -> Value {
        let table: Vec<Value> = self
            .rows()
            .iter()
            .map(|((policy, class), [n, m, i, rj, fp, fnp, fns])| {
                json!({"policy": policy, "class": class, "cases": n, "match": m, "interface_match": i,
                       "reject": rj, "false_positives": fp, "false_negatives_physics": fnp,
                       "false_negatives_specification": fns})
            })
            .collect();
        let totals: Vec<Value> = self
            .policies()
            .into_iter()
            .map(|policy| {
                let [fit, missed, fp] = self.totals(policy);
                json!({"policy": policy.name(), "false_positives": fp, "fits": fit, "false_negatives": missed})
            })
            .collect();
        json!({"rows": table, "totals": totals})
    }

    /// Under `policy`: fits, those rejected, and false positives.
    fn totals(&self, policy: Policy) -> [usize; 3] {
        let of = |f: &dyn Fn(&Scored) -> bool| {
            self.runs
                .iter()
                .filter(|s| s.run.policy == policy && f(s))
                .count()
        };
        let fit = of(&|s| s.truth.fits(policy) && s.truth != Truth::Collides);
        let missed = of(&|s| s.false_negative() && s.truth != Truth::Collides);
        [fit, missed, of(&Scored::false_positive)]
    }

    /// The report as a table, for people.
    pub fn report_text(&self) -> String {
        let mut text = String::from(
            "policy      class               cases match inter reject FP FN(phys) FN(spec)\n",
        );
        for ((policy, class), [n, m, i, rj, fp, fnp, fns]) in &self.rows() {
            let _ = writeln!(
                text,
                "{policy:11} {class:19} {n:5} {m:5} {i:5} {rj:6} {fp:2} {fnp:8} {fns:8}"
            );
        }
        for policy in self.policies() {
            let [fit, missed, fp] = self.totals(policy);
            let _ = writeln!(
                text,
                "{}: false positives {fp}; false negatives against physics {missed} of {fit}",
                policy.name()
            );
        }
        text
    }
}
