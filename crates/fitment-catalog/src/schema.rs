//! The database (`docs/research/m5-catalog.md`, section 6.2, DB1, DB5):
//! its pragmas, its tables and its version: schema 1, as M5 shipped it in
//! 0.2.0.

use rusqlite::{Connection, OptionalExtension as _};

/// `application_id`: 'fmtC'.
pub const APPLICATION_ID: i64 = 0x666D_7443;

/// `user_version`: the schema this build writes (DB5).
pub const SCHEMA_VERSION: i64 = 1;

/// What a database is, by its header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A catalog of this build's schema.
    Current,
    /// A catalog of an older schema: migrated forward on open (DB5).
    Older(i64),
    /// A catalog of a newer schema: refused (exit 3).
    Newer(i64),
    /// Not a catalog.
    Foreign,
}

/// The per-connection settings (DB1, ST9, DB2): WAL, checked; full syncs;
/// foreign keys; no trusted schema; cell checks; the busy timeout.
///
/// # Errors
///
/// SQLite's; `journal_mode` not coming back `wal` is reported as an error
/// too (CC5).
pub fn configure(conn: &Connection, busy_timeout_ms: u32) -> rusqlite::Result<()> {
    conn.busy_timeout(std::time::Duration::from_millis(u64::from(busy_timeout_ms)))?;
    let mode: String = conn.query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))?;
    if !mode.eq_ignore_ascii_case("wal") {
        return Err(rusqlite::Error::InvalidParameterName(format!(
            "journal_mode is {mode}, not wal: the catalog must be on a local file system"
        )));
    }
    conn.execute_batch(
        "PRAGMA synchronous = FULL;
         PRAGMA foreign_keys = ON;
         PRAGMA trusted_schema = OFF;
         PRAGMA cell_size_check = ON;",
    )?;
    if cfg!(target_vendor = "apple") {
        conn.execute_batch("PRAGMA fullfsync = ON; PRAGMA checkpoint_fullfsync = ON;")?;
    }
    Ok(())
}

/// What the database at `conn` is.
///
/// # Errors
///
/// SQLite's, if its header cannot be read.
pub fn kind(conn: &Connection) -> rusqlite::Result<Kind> {
    let id: i64 = conn.query_row("PRAGMA application_id", [], |r| r.get(0))?;
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    Ok(if id != APPLICATION_ID || version < 1 {
        Kind::Foreign
    } else if version > SCHEMA_VERSION {
        Kind::Newer(version)
    } else if version < SCHEMA_VERSION {
        Kind::Older(version)
    } else {
        Kind::Current
    })
}

/// One step of DB5's forward migration: from its version to the next.
pub type Migration = fn(&Connection) -> rusqlite::Result<()>;

/// The migrations from each version to the next; none in M5.
pub const MIGRATIONS: &[(i64, Migration)] = &[];

/// DB5: migrates a catalog at `from` forward to `to` by `steps`, in one
/// `BEGIN EXCLUSIVE` transaction, recording an `op` row per step.
///
/// # Errors
///
/// SQLite's, or a missing step; nothing is changed then.
pub fn migrate(
    conn: &mut Connection,
    from: i64,
    to: i64,
    steps: &[(i64, Migration)],
) -> rusqlite::Result<()> {
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Exclusive)?;
    // DB2: read again under the lock; another process may have migrated.
    let mut at: i64 = tx.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if at < from {
        at = from;
    }
    while at < to {
        let Some((_, step)) = steps.iter().find(|(v, _)| *v == at) else {
            return Err(rusqlite::Error::InvalidParameterName(format!(
                "no migration from schema {at}"
            )));
        };
        step(&tx)?;
        at += 1;
        let generation = next_generation(&tx)?;
        tx.execute(
            "INSERT INTO op (generation, n, kind, fitment_version) VALUES (?1, 0, 'migrate', ?2)",
            rusqlite::params![generation, env!("CARGO_PKG_VERSION")],
        )?;
        tx.pragma_update(None, "user_version", at)?;
    }
    tx.commit()
}

/// DB8: raises the generation by one and returns it.
///
/// # Errors
///
/// SQLite's.
pub fn next_generation(conn: &Connection) -> rusqlite::Result<i64> {
    conn.query_row(
        "UPDATE clock SET generation = generation + 1 WHERE one = 1 RETURNING generation",
        [],
        |r| r.get(0),
    )
}

/// The generation as it stands.
///
/// # Errors
///
/// SQLite's.
pub fn generation(conn: &Connection) -> rusqlite::Result<i64> {
    conn.query_row("SELECT generation FROM clock WHERE one = 1", [], |r| {
        r.get(0)
    })
    .optional()
    .map(Option::unwrap_or_default)
}

/// The tables of schema 1 (section 6.2).
const DDL: &str = "
CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL) STRICT;
CREATE TABLE clock (
  one        INTEGER PRIMARY KEY CHECK (one = 1),
  generation INTEGER NOT NULL CHECK (generation >= 0)
) STRICT;
CREATE TABLE op (
  generation      INTEGER NOT NULL,
  n               INTEGER NOT NULL,
  kind            TEXT NOT NULL CHECK (kind IN ('init','ingest','replace','delete','reindex','gc','prune','migrate')),
  entry           INTEGER,
  sha256          TEXT,
  fitment_version TEXT NOT NULL,
  PRIMARY KEY (generation, n)
) STRICT;

CREATE TABLE blob (
  key              INTEGER PRIMARY KEY AUTOINCREMENT,
  sha256           TEXT NOT NULL UNIQUE
                   CHECK (length(sha256) = 64 AND NOT sha256 GLOB '*[^0-9a-f]*'),
  bytes            INTEGER NOT NULL CHECK (bytes >= 0),
  added_generation INTEGER NOT NULL
) STRICT;

CREATE TABLE name (
  id_fold       TEXT PRIMARY KEY,
  last_revision INTEGER NOT NULL CHECK (last_revision >= 1)
) STRICT;

CREATE TABLE entry (
  key                INTEGER PRIMARY KEY AUTOINCREMENT,
  id                 TEXT NOT NULL CHECK (length(id) BETWEEN 1 AND 64),
  id_fold            TEXT NOT NULL REFERENCES name(id_fold),
  created_generation INTEGER NOT NULL,
  deleted_generation INTEGER
                     CHECK (deleted_generation IS NULL OR deleted_generation >= created_generation)
) STRICT;
CREATE UNIQUE INDEX entry_live_id ON entry(id_fold) WHERE deleted_generation IS NULL;

CREATE TABLE revision (
  key                   INTEGER PRIMARY KEY AUTOINCREMENT,
  entry                 INTEGER NOT NULL REFERENCES entry(key),
  id_fold               TEXT NOT NULL,
  revision              INTEGER NOT NULL CHECK (revision >= 1),
  sha256                TEXT NOT NULL
                        CHECK (length(sha256) = 64 AND NOT sha256 GLOB '*[^0-9a-f]*'),
  file_name             TEXT CHECK (file_name IS NULL OR length(CAST(file_name AS BLOB)) <= 4096),
  added_generation      INTEGER NOT NULL,
  superseded_generation INTEGER
                        CHECK (superseded_generation IS NULL OR superseded_generation >= added_generation),
  pruned_generation     INTEGER CHECK (pruned_generation IS NULL OR
                          (superseded_generation IS NOT NULL AND pruned_generation >= superseded_generation)),
  UNIQUE (id_fold, revision)
) STRICT;
CREATE UNIQUE INDEX revision_current ON revision(entry) WHERE superseded_generation IS NULL;
CREATE UNIQUE INDEX revision_live_content ON revision(sha256) WHERE superseded_generation IS NULL;

CREATE TABLE features (
  sha256            TEXT NOT NULL REFERENCES blob(sha256) ON DELETE CASCADE,
  extractor_version INTEGER NOT NULL CHECK (extractor_version > 0),
  status            TEXT NOT NULL CHECK (status IN ('accepted', 'refused')),
  encoding          TEXT NOT NULL DEFAULT 'json' CHECK (encoding = 'json'),
  doc               BLOB NOT NULL,
  doc_sha256        TEXT NOT NULL
                    CHECK (length(doc_sha256) = 64 AND NOT doc_sha256 GLOB '*[^0-9a-f]*'),
  part_digest       TEXT CHECK (part_digest IS NULL OR
                      (length(part_digest) = 64 AND NOT part_digest GLOB '*[^0-9a-f]*')),
  product_id        TEXT,
  product_name      TEXT,
  holes             INTEGER,
  shafts            INTEGER,
  planes            INTEGER,
  cones             INTEGER,
  other_faces       INTEGER,
  complete          INTEGER,
  first_refusal     TEXT,
  CHECK ((status = 'accepted') = (first_refusal IS NULL)),
  CHECK ((status = 'accepted') = (part_digest IS NOT NULL)),
  PRIMARY KEY (sha256, extractor_version)
) STRICT;
CREATE TRIGGER features_immutable BEFORE UPDATE ON features
BEGIN SELECT RAISE(ABORT, 'features rows are insert-only'); END;

CREATE TABLE tokens (
  sha256            TEXT NOT NULL,
  extractor_version INTEGER NOT NULL,
  signature_version INTEGER NOT NULL CHECK (signature_version > 0),
  tokens            BLOB NOT NULL,
  tokens_sha256     TEXT NOT NULL
                    CHECK (length(tokens_sha256) = 64 AND NOT tokens_sha256 GLOB '*[^0-9a-f]*'),
  lever_mm          REAL NOT NULL CHECK (lever_mm >= 0),
  planes            INTEGER NOT NULL CHECK (planes >= 0),
  pairs_incomplete  INTEGER NOT NULL CHECK (pairs_incomplete IN (0, 1)),
  PRIMARY KEY (sha256, extractor_version, signature_version),
  FOREIGN KEY (sha256, extractor_version)
    REFERENCES features(sha256, extractor_version) ON DELETE CASCADE
) STRICT;
CREATE TRIGGER tokens_accepted_only BEFORE INSERT ON tokens
WHEN (SELECT status FROM features
      WHERE sha256 = NEW.sha256 AND extractor_version = NEW.extractor_version) IS NOT 'accepted'
BEGIN SELECT RAISE(ABORT, 'tokens for a refused or missing document'); END;
CREATE TRIGGER tokens_immutable BEFORE UPDATE ON tokens
BEGIN SELECT RAISE(ABORT, 'tokens rows are insert-only'); END;

CREATE TABLE posting (
  extractor_version INTEGER NOT NULL,
  signature_version INTEGER NOT NULL,
  sense             INTEGER NOT NULL CHECK (sense IN (0, 1)),
  bin               INTEGER NOT NULL,
  content           INTEGER NOT NULL REFERENCES blob(key) ON DELETE CASCADE,
  stack             INTEGER NOT NULL CHECK (stack >= 0),
  diameter_mm       REAL NOT NULL CHECK (diameter_mm > 0),
  PRIMARY KEY (extractor_version, signature_version, sense, bin, content, stack, diameter_mm)
) STRICT, WITHOUT ROWID;
CREATE INDEX posting_owner ON posting(content, extractor_version, signature_version);

CREATE TABLE properties (
  sha256             TEXT NOT NULL REFERENCES blob(sha256) ON DELETE CASCADE,
  properties_version INTEGER NOT NULL CHECK (properties_version > 0),
  truncated          INTEGER NOT NULL CHECK (truncated IN (0, 1)),
  stated             BLOB NOT NULL,
  stated_sha256      TEXT NOT NULL
                     CHECK (length(stated_sha256) = 64 AND NOT stated_sha256 GLOB '*[^0-9a-f]*'),
  PRIMARY KEY (sha256, properties_version)
) STRICT;
CREATE TRIGGER properties_immutable BEFORE UPDATE ON properties
BEGIN SELECT RAISE(ABORT, 'properties rows are insert-only'); END;

CREATE VIEW live AS
SELECT e.key AS entry, e.id, r.revision, r.sha256, r.file_name, r.added_generation
FROM entry e JOIN revision r ON r.entry = e.key AND r.superseded_generation IS NULL
WHERE e.deleted_generation IS NULL;
";

/// Creates schema 1 in an empty database, in one transaction, at
/// generation 1 with its `init` row.
///
/// # Errors
///
/// SQLite's; nothing is created then.
pub fn create(conn: &mut Connection) -> rusqlite::Result<()> {
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Exclusive)?;
    tx.execute_batch(DDL)?;
    tx.execute(
        "INSERT INTO meta (key, value) VALUES ('format', 'fitment.catalog')",
        [],
    )?;
    tx.execute("INSERT INTO clock (one, generation) VALUES (1, 1)", [])?;
    tx.execute(
        "INSERT INTO op (generation, n, kind, fitment_version) VALUES (1, 0, 'init', ?1)",
        [env!("CARGO_PKG_VERSION")],
    )?;
    tx.pragma_update(None, "application_id", APPLICATION_ID)?;
    tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    tx.commit()
}

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    use super::{Kind, Migration, create, generation, kind, migrate};

    fn fresh() -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON").unwrap();
        create(&mut conn).unwrap();
        conn
    }

    #[test]
    fn a_new_schema_is_current_at_generation_one() {
        let conn = fresh();
        assert_eq!(kind(&conn).unwrap(), Kind::Current);
        assert_eq!(generation(&conn).unwrap(), 1);
        let empty = Connection::open_in_memory().unwrap();
        assert_eq!(kind(&empty).unwrap(), Kind::Foreign);
        conn.pragma_update(None, "user_version", 2).unwrap();
        assert_eq!(kind(&conn).unwrap(), Kind::Newer(2));
    }

    /// DB6: every hex column refuses what is not 64 lower-case digits.
    #[test]
    fn hex_columns_refuse_what_is_not_a_sha256() {
        let conn = fresh();
        let good = "ab".repeat(32);
        for bad in ["AB".repeat(32), "ab".repeat(31) + "a", "../../x".to_owned()] {
            assert!(
                conn.execute(
                    "INSERT INTO blob (sha256, bytes, added_generation) VALUES (?1, 1, 1)",
                    [&bad],
                )
                .is_err(),
                "{bad}"
            );
        }
        conn.execute(
            "INSERT INTO blob (sha256, bytes, added_generation) VALUES (?1, 1, 1)",
            [&good],
        )
        .unwrap();
    }

    /// GT4 and DB7: tokens only for an accepted document; derived rows
    /// insert-only.
    #[test]
    fn tokens_need_an_accepted_document_and_rows_never_change() {
        let conn = fresh();
        let sha = "ab".repeat(32);
        let hash = "cd".repeat(32);
        conn.execute(
            "INSERT INTO blob (sha256, bytes, added_generation) VALUES (?1, 1, 1)",
            [&sha],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO features (sha256, extractor_version, status, doc, doc_sha256, first_refusal)
             VALUES (?1, 6, 'refused', x'7b7d', ?2, 'assembly')",
            [&sha, &hash],
        )
        .unwrap();
        let tokens = "INSERT INTO tokens (sha256, extractor_version, signature_version, tokens,
                      tokens_sha256, lever_mm, planes, pairs_incomplete)
                      VALUES (?1, 6, 2, x'7b7d', ?2, 1.0, 0, 0)";
        assert!(conn.execute(tokens, [&sha, &hash]).is_err());
        assert!(
            conn.execute("UPDATE features SET status = 'accepted'", [])
                .is_err()
        );
        // An accepted document must state its part digest, and no refusal.
        assert!(
            conn.execute(
                "INSERT INTO features (sha256, extractor_version, status, doc, doc_sha256)
                 VALUES (?1, 7, 'accepted', x'7b7d', ?2)",
                [&sha, &hash],
            )
            .is_err()
        );
        conn.execute(
            "INSERT INTO features (sha256, extractor_version, status, doc, doc_sha256, part_digest)
             VALUES (?1, 7, 'accepted', x'7b7d', ?2, ?2)",
            [&sha, &hash],
        )
        .unwrap();
        let tokens = tokens.replace("VALUES (?1, 6,", "VALUES (?1, 7,");
        conn.execute(&tokens, [&sha, &hash]).unwrap();
        assert!(conn.execute("UPDATE tokens SET planes = 1", []).is_err());
    }

    /// ID2, ID5, CR3: one live id under folding, one live content, one
    /// current revision per entry.
    #[test]
    fn the_partial_indexes_hold_identity() {
        let conn = fresh();
        let (x, y) = ("ab".repeat(32), "cd".repeat(32));
        conn.execute_batch(
            "INSERT INTO name VALUES ('a', 1), ('b', 1);
             INSERT INTO entry (id, id_fold, created_generation) VALUES ('A', 'a', 1);",
        )
        .unwrap();
        assert!(
            conn.execute(
                "INSERT INTO entry (id, id_fold, created_generation) VALUES ('a', 'a', 1)",
                [],
            )
            .is_err()
        );
        conn.execute(
            "INSERT INTO entry (id, id_fold, created_generation) VALUES ('b', 'b', 1)",
            [],
        )
        .unwrap();
        let revision = "INSERT INTO revision (entry, id_fold, revision, sha256, added_generation)
                        VALUES (?1, ?2, ?3, ?4, 1)";
        conn.execute(revision, rusqlite::params![1, "a", 1, &x])
            .unwrap();
        // The same content live twice, a second current revision.
        assert!(
            conn.execute(revision, rusqlite::params![2, "b", 1, &x])
                .is_err()
        );
        assert!(
            conn.execute(revision, rusqlite::params![1, "a", 2, &y])
                .is_err()
        );
        // A pruned generation needs a superseded one.
        assert!(
            conn.execute(
                "UPDATE revision SET pruned_generation = 2 WHERE entry = 1",
                [],
            )
            .is_err()
        );
    }

    /// DB5's mechanism: a catalog is migrated one step at a time, each step
    /// recorded, and a missing step changes nothing.
    #[test]
    fn migration_runs_each_step_once_and_records_it() {
        let mut conn = fresh();
        let step: Migration = |c| c.execute_batch("CREATE TABLE added (x INTEGER) STRICT");
        migrate(&mut conn, 1, 2, &[(1, step)]).unwrap();
        assert_eq!(kind(&conn).unwrap(), Kind::Newer(2));
        let kinds: Vec<String> = conn
            .prepare("SELECT kind FROM op ORDER BY generation, n")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(kinds, ["init", "migrate"]);
        assert!(migrate(&mut conn, 2, 3, &[]).is_err());
        assert_eq!(generation(&conn).unwrap(), 2);
    }
}
