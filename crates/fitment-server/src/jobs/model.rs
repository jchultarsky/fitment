//! Jobs as a client sees them (docs/research/m6-api.md, JB1, JB3, JS1):
//! the kinds, their lanes, the states and the transitions between them.

use serde::Serialize;

/// A job's kind (JB1). PR 5 runs `features`, `assembly_read` and
/// `socket_extract`, PR 6 `socket_import` and `match`; the catalog's are
/// named so `server.db`'s rows and the lanes are complete, and come with
/// PR 7.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// `POST /v1/features`: `fitment features FILE`.
    Features,
    /// `POST /v1/assemblies`: `fitment parts FILE`.
    AssemblyRead,
    /// `POST /v1/sockets`: `fitment socket FILE --part X`.
    SocketExtract,
    /// `POST /v1/sockets/import`: a socket file, verified or marked.
    SocketImport,
    /// `POST /v1/matches`: `fitment match S FILE…`.
    Match,
    /// The catalog's writes and maintenance (PR 7).
    CatalogIngest,
    #[allow(missing_docs)]
    CatalogReplace,
    #[allow(missing_docs)]
    CatalogReindex,
    #[allow(missing_docs)]
    CatalogVerify,
    #[allow(missing_docs)]
    CatalogGc,
    #[allow(missing_docs)]
    CatalogPrune,
}

impl Kind {
    /// Every kind, as `server.db`'s check lists them.
    pub const ALL: [Self; 11] = [
        Self::Features,
        Self::AssemblyRead,
        Self::SocketExtract,
        Self::SocketImport,
        Self::Match,
        Self::CatalogIngest,
        Self::CatalogReplace,
        Self::CatalogReindex,
        Self::CatalogVerify,
        Self::CatalogGc,
        Self::CatalogPrune,
    ];

    /// Its name, as `server.db` and the documents write it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Features => "features",
            Self::AssemblyRead => "assembly_read",
            Self::SocketExtract => "socket_extract",
            Self::SocketImport => "socket_import",
            Self::Match => "match",
            Self::CatalogIngest => "catalog_ingest",
            Self::CatalogReplace => "catalog_replace",
            Self::CatalogReindex => "catalog_reindex",
            Self::CatalogVerify => "catalog_verify",
            Self::CatalogGc => "catalog_gc",
            Self::CatalogPrune => "catalog_prune",
        }
    }

    /// The kind of `name`.
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.name() == name)
    }

    /// Its lane (JS1): queries, the catalog's writes, `verify`.
    pub fn lane(self) -> Lane {
        match self {
            Self::Features
            | Self::AssemblyRead
            | Self::SocketExtract
            | Self::SocketImport
            | Self::Match => Lane::Query,
            Self::CatalogVerify => Lane::Maintenance,
            _ => Lane::Write,
        }
    }

    /// A pure job reads and writes nothing of the catalog: re-run at a
    /// restart, up to two attempts (JB6, SD5); a write job is interrupted.
    pub fn pure(self) -> bool {
        self.lane() != Lane::Write
    }

    /// The format of the document it produces (DC1).
    pub fn document_format(self) -> &'static str {
        match self {
            Self::Features => fitment_core::FEATURES_FORMAT,
            Self::AssemblyRead => fitment_core::PARTS_FORMAT,
            Self::SocketExtract | Self::SocketImport => fitment_core::SOCKET_FORMAT,
            Self::Match => fitment_core::MATCH_FORMAT,
            Self::CatalogVerify => "fitment.verify",
            Self::CatalogGc => "fitment.gc",
            _ => crate::catalog::FORMAT,
        }
    }

    /// The format of what a job of this kind makes, its `result` (section
    /// 7.11): an assembly, a socket revision, a run record; `None` for a
    /// kind whose result is its document alone.
    pub fn result_format(self) -> Option<&'static str> {
        match self {
            Self::AssemblyRead => Some(crate::assemblies::ASSEMBLY_FORMAT),
            Self::SocketExtract | Self::SocketImport => Some(fitment_core::SOCKET_FORMAT),
            Self::Match => Some("fitment.run"),
            _ => None,
        }
    }
}

/// A lane (JS1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Lane {
    /// `features`, `assembly_read`, `socket_extract`, `socket_import`,
    /// `match`.
    Query,
    /// The catalog's writes.
    Write,
    /// `catalog_verify`.
    Maintenance,
}

impl Lane {
    /// Every lane.
    pub const ALL: [Self; 3] = [Self::Query, Self::Write, Self::Maintenance];

    /// Its name.
    pub fn name(self) -> &'static str {
        match self {
            Self::Query => "query",
            Self::Write => "write",
            Self::Maintenance => "maintenance",
        }
    }

    /// Its index in [`Lane::ALL`].
    pub fn index(self) -> usize {
        match self {
            Self::Query => 0,
            Self::Write => 1,
            Self::Maintenance => 2,
        }
    }
}

/// A job's state (JB3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum State {
    /// Waiting for its lane and permits.
    Queued,
    /// Running.
    Running,
    /// Asked to stop (a cancel or its deadline); running until its call
    /// returns (JS8).
    Cancelling,
    /// Ran to its end: its document is the complete answer (exit 0, 4, 5).
    Succeeded,
    /// The CLI would exit 2 or 3, or a stated tightening refused it.
    Refused,
    /// The CLI would exit 1, or the job could not answer.
    Failed,
    /// Stopped at its owner's or an admin's request.
    Cancelled,
    /// A write job running when the process stopped.
    Interrupted,
}

impl State {
    /// Every state.
    pub const ALL: [Self; 8] = [
        Self::Queued,
        Self::Running,
        Self::Cancelling,
        Self::Succeeded,
        Self::Refused,
        Self::Failed,
        Self::Cancelled,
        Self::Interrupted,
    ];

    /// Its name.
    pub fn name(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Cancelling => "cancelling",
            Self::Succeeded => "succeeded",
            Self::Refused => "refused",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Interrupted => "interrupted",
        }
    }

    /// The state of `name`.
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.name() == name)
    }

    /// True once the job has ended: a terminal state is kept for good
    /// (XH12's first invariant).
    pub fn terminal(self) -> bool {
        !matches!(self, Self::Queued | Self::Running | Self::Cancelling)
    }

    /// JB3: the state an exit code gives a job that returned: 0, 4 and 5
    /// succeeded; 2 and 3 refused; 1 failed.
    pub fn of_exit(exit: u8) -> Self {
        match exit {
            0 | 4 | 5 => Self::Succeeded,
            2 | 3 => Self::Refused,
            _ => Self::Failed,
        }
    }

    /// True if the transition from `self` to `to` is one of JB3's.
    pub fn may_become(self, to: Self) -> bool {
        use State::{Cancelled, Cancelling, Failed, Interrupted, Queued, Running};
        matches!(
            (self, to),
            (Queued, Running | Cancelled)
                | (Running, Cancelling | Queued | Interrupted)
                | (Cancelling, Queued | Failed | Cancelled)
        ) || (matches!(self, Running | Cancelling) && to.terminal())
    }
}

/// Why a running job's flag was set (JS8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stop {
    /// Its owner or an admin asked: it ends `cancelled`.
    Cancel,
    /// Its deadline passed (JB7): it ends `failed`, `deadline_exceeded`.
    Deadline,
    /// The server drains (SD1): a pure job goes back to `queued`, its
    /// attempt unchanged.
    Shutdown,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// JB3: the transitions of the diagram, and no other; a terminal state
    /// becomes nothing.
    #[test]
    fn the_transitions_are_jb3s() {
        for from in State::ALL {
            for to in State::ALL {
                if from.terminal() {
                    assert!(!from.may_become(to), "{from:?} -> {to:?}");
                }
            }
        }
        assert!(State::Queued.may_become(State::Running));
        assert!(State::Queued.may_become(State::Cancelled));
        assert!(!State::Queued.may_become(State::Succeeded));
        assert!(State::Running.may_become(State::Cancelling));
        assert!(State::Cancelling.may_become(State::Cancelled));
        assert!(State::Running.may_become(State::Queued));
        assert_eq!(State::of_exit(0), State::Succeeded);
        assert_eq!(State::of_exit(4), State::Succeeded);
        assert_eq!(State::of_exit(5), State::Succeeded);
        assert_eq!(State::of_exit(2), State::Refused);
        assert_eq!(State::of_exit(3), State::Refused);
        assert_eq!(State::of_exit(1), State::Failed);
        for kind in Kind::ALL {
            assert_eq!(Kind::parse(kind.name()), Some(kind));
        }
        assert_eq!(Kind::CatalogVerify.lane(), Lane::Maintenance);
        assert_eq!(Kind::CatalogGc.lane(), Lane::Write);
        assert!(Kind::SocketExtract.pure() && !Kind::CatalogIngest.pure());
    }
}
