-- server.db, schema 1 (docs/research/m6-api.md, section 6.2), as the design
-- states it. The header (application_id 0x666D7453, user_version 1) and the
-- per-connection pragmas are set by state/db.rs, which runs this batch once
-- inside the transaction that creates the database (OB1).

CREATE TABLE meta  (key TEXT PRIMARY KEY, value TEXT NOT NULL) STRICT;
CREATE TABLE lease (one INTEGER PRIMARY KEY CHECK (one = 1),                 -- SD6
                    holder TEXT NOT NULL, heartbeat_ms INTEGER NOT NULL) STRICT;

CREATE TABLE user (
  key INTEGER PRIMARY KEY AUTOINCREMENT, id TEXT NOT NULL UNIQUE,            -- 'u_' + 26 base32
  kind TEXT NOT NULL DEFAULT 'person' CHECK (kind IN ('person','system')),  -- M6-56
  username_fold TEXT NOT NULL UNIQUE CHECK (length(username_fold) BETWEEN 1 AND 64),
  username TEXT NOT NULL,
  password_phc TEXT,                                                         -- NULL: token-only user
  role TEXT NOT NULL CHECK (role IN ('reader','writer','admin')),
  disabled INTEGER NOT NULL DEFAULT 0 CHECK (disabled IN (0,1)),
  created_ms INTEGER NOT NULL, updated_ms INTEGER NOT NULL) STRICT;

CREATE TABLE token (
  key INTEGER PRIMARY KEY AUTOINCREMENT, id TEXT NOT NULL UNIQUE,            -- 't_' + 26 base32
  user INTEGER NOT NULL REFERENCES user(key),
  name TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 128),
  secret_sha256 TEXT NOT NULL UNIQUE CHECK (length(secret_sha256) = 64),    -- AU3
  hint TEXT NOT NULL,                                                        -- first 4 characters after the prefix
  role TEXT NOT NULL CHECK (role IN ('reader','writer','admin')),            -- at most the presenting token's (AU10)
  parent INTEGER REFERENCES token(key),                                      -- AU10: minted from; NULL for a login's
  created_ms INTEGER NOT NULL, expires_ms INTEGER NOT NULL,                  -- AU4: always expires; AU10: ≤ the parent's
  last_used_ms INTEGER, revoked_ms INTEGER) STRICT;
CREATE INDEX token_user ON token(user, revoked_ms, expires_ms);             -- AU10's live count
CREATE INDEX token_parent ON token(parent);                                -- AU4's cascade, the sweep
CREATE TRIGGER token_not_for_system BEFORE INSERT ON token
WHEN (SELECT kind FROM user WHERE key = NEW.user) = 'system'
BEGIN SELECT RAISE(ABORT, 'the system user holds no token'); END;
CREATE TRIGGER token_not_moved_to_system BEFORE UPDATE OF user ON token
WHEN (SELECT kind FROM user WHERE key = NEW.user) = 'system'
BEGIN SELECT RAISE(ABORT, 'the system user holds no token'); END;
CREATE TRIGGER user_kind_fixed BEFORE UPDATE OF kind ON user           -- a person never becomes the system user
WHEN NEW.kind IS NOT OLD.kind
BEGIN SELECT RAISE(ABORT, 'a user''s kind never changes'); END;

CREATE TABLE object (                       -- OB2: each row promises a file hashing to sha256
  sha256 TEXT PRIMARY KEY CHECK (length(sha256) = 64 AND NOT sha256 GLOB '*[^0-9a-f]*'),
  bytes INTEGER NOT NULL CHECK (bytes >= 0), added_ms INTEGER NOT NULL) STRICT;

CREATE TABLE upload (                       -- UP3: a user's access to an object
  key INTEGER PRIMARY KEY AUTOINCREMENT,
  owner INTEGER NOT NULL REFERENCES user(key),
  sha256 TEXT NOT NULL REFERENCES object(sha256),
  created_ms INTEGER NOT NULL, expires_ms INTEGER NOT NULL,
  UNIQUE (owner, sha256)) STRICT;

CREATE TABLE assembly (                     -- RA3: a user's registered assembly
  key INTEGER PRIMARY KEY AUTOINCREMENT,
  owner INTEGER NOT NULL REFERENCES user(key),
  sha256 TEXT NOT NULL REFERENCES object(sha256),
  created_ms INTEGER NOT NULL, deleted_ms INTEGER) STRICT;
CREATE UNIQUE INDEX assembly_live ON assembly(owner, sha256) WHERE deleted_ms IS NULL;
CREATE TABLE assembly_name (assembly INTEGER NOT NULL REFERENCES assembly(key),
  name TEXT NOT NULL CHECK (length(CAST(name AS BLOB)) BETWEEN 1 AND 255),      -- RA8's file_name
  PRIMARY KEY (assembly, name)) STRICT;
CREATE TABLE assembly_parts (               -- derived, insert-only, by version (RA3, DB7's pattern)
  assembly INTEGER NOT NULL REFERENCES assembly(key),
  extractor_version INTEGER NOT NULL, socket_rules_version INTEGER NOT NULL,
  status TEXT NOT NULL CHECK (status IN ('accepted','refused')),
  document_sha256 TEXT NOT NULL REFERENCES object(sha256),                 -- fitment.parts bytes
  root TEXT,                                                               -- fitment.assembly's root, as JSON (PR 5)
  job INTEGER,                                                             -- JS13; no FK: the job record expires first
  PRIMARY KEY (assembly, extractor_version, socket_rules_version)) STRICT;

CREATE TABLE socket_line (                  -- RA4
  key INTEGER PRIMARY KEY AUTOINCREMENT,
  owner INTEGER NOT NULL REFERENCES user(key),
  id TEXT NOT NULL CHECK (length(id) = 23 AND id GLOB 'sk_*'),
  assembly_sha256 TEXT NOT NULL CHECK (length(assembly_sha256) = 64),     -- no FK: an import may name
  definition TEXT NOT NULL, occurrence TEXT NOT NULL,                      -- an assembly not stored
  last_n INTEGER NOT NULL DEFAULT 0,                                       -- RA4: its highest revision, kept when
  created_ms INTEGER NOT NULL, deleted_ms INTEGER) STRICT;                 -- its revisions are swept (PR 5)
CREATE UNIQUE INDEX socket_line_live ON socket_line(owner, id) WHERE deleted_ms IS NULL;   -- RA4: one live row,
CREATE INDEX socket_line_rows ON socket_line(owner, id);                                    -- tombstones beside it
CREATE INDEX socket_line_assembly ON socket_line(owner, assembly_sha256);                   -- RA6, the lists (PR 5)
CREATE TABLE socket_revision (              -- RA5, OB5: insert-only
  key INTEGER PRIMARY KEY AUTOINCREMENT,
  line INTEGER NOT NULL REFERENCES socket_line(key),
  owner INTEGER NOT NULL, line_id TEXT NOT NULL,                            -- copied from the line row: RA4's
  n INTEGER NOT NULL CHECK (n >= 1),                                        -- numbering spans a revived id's rows
  how TEXT NOT NULL CHECK (how IN ('extract','review','import')),
  verified INTEGER NOT NULL CHECK (verified IN (0,1)),                      -- RA10
  parent INTEGER REFERENCES socket_revision(key),
  document_sha256 TEXT NOT NULL REFERENCES object(sha256),                 -- exact fitment.socket bytes
  document_digest TEXT NOT NULL,                                           -- the document's own digest
  extractor_version INTEGER NOT NULL, socket_rules_version INTEGER NOT NULL,
  unreviewed INTEGER NOT NULL, carried INTEGER NOT NULL, stale INTEGER NOT NULL,
  job INTEGER, created_ms INTEGER NOT NULL,                               -- JS13; no FK: the job record expires first
  UNIQUE (owner, line_id, n)) STRICT;
CREATE TRIGGER socket_revision_insert_only BEFORE UPDATE ON socket_revision
BEGIN SELECT RAISE(ABORT, 'socket revisions are insert-only'); END;

CREATE TABLE job (
  key INTEGER PRIMARY KEY AUTOINCREMENT,                                   -- submission order
  id TEXT NOT NULL UNIQUE,                                                 -- 'j_' + 26 base32 (M6-55)
  owner INTEGER NOT NULL REFERENCES user(key),
  kind TEXT NOT NULL CHECK (kind IN ('features','assembly_read','socket_extract','socket_import','match',
         'catalog_ingest','catalog_replace','catalog_reindex','catalog_verify','catalog_gc','catalog_prune')),
  lane TEXT NOT NULL CHECK (lane IN ('query','write','maintenance')),
  state TEXT NOT NULL CHECK (state IN ('queued','running','cancelling','succeeded',
                                       'refused','failed','cancelled','interrupted')),
  request BLOB NOT NULL, request_sha256 TEXT NOT NULL,                     -- canonical (JB9)
  idempotency_key TEXT, idempotency_sha256 TEXT,                           -- JB9: the request as sent; a released
  released_key TEXT,                                                       -- key moves here, freeing the index
  attempt INTEGER NOT NULL DEFAULT 1, request_id TEXT NOT NULL,
  created_ms INTEGER NOT NULL, started_ms INTEGER, finished_ms INTEGER, deadline_ms INTEGER,
  cancel_requested INTEGER NOT NULL DEFAULT 0,
  exit INTEGER CHECK (exit IS NULL OR exit IN (0,1,2,3,4,5)),              -- the CLI's code (JB4)
  outcome TEXT, error_code TEXT, error_details TEXT,                       -- ER3: built from codes and values
  result_sha256 TEXT REFERENCES object(sha256), result_format TEXT,        -- the document's bytes
  result TEXT,                                                             -- what it made: fitment.job's result.href
  index_sha256 TEXT REFERENCES object(sha256),                             -- DC12: a match's candidates index
  catalog_generation INTEGER, catalog_counts TEXT,                         -- M6-25; a write job's at dispatch (JB6)
  notes TEXT NOT NULL DEFAULT '[]',                                        -- JB10
  pinned INTEGER NOT NULL DEFAULT 0, expires_ms INTEGER,
  UNIQUE (owner, idempotency_key)) STRICT;
CREATE INDEX job_queue ON job(lane, state, key) WHERE state = 'queued';
CREATE INDEX job_list ON job(owner, created_ms, id);
CREATE INDEX job_expiry ON job(expires_ms) WHERE expires_ms IS NOT NULL;      -- JS9's retention (PR 5)
CREATE INDEX job_live ON job(state) WHERE state IN ('running', 'cancelling'); -- SD4's recovery (PR 5)

CREATE TABLE job_input (job INTEGER NOT NULL REFERENCES job(key) ON DELETE CASCADE,
  n INTEGER NOT NULL, sha256 TEXT NOT NULL REFERENCES object(sha256),
  PRIMARY KEY (job, n)) STRICT;                                            -- OB4: written at submission; held until
                                                                           -- terminal, a match's for its retention
CREATE TABLE job_cites (job INTEGER NOT NULL REFERENCES job(key) ON DELETE CASCADE,
  revision INTEGER NOT NULL REFERENCES socket_revision(key),
  PRIMARY KEY (job, revision)) STRICT;                                     -- RA6, OB4: a match's socket revision,
                                                                           -- written at submission
CREATE TABLE job_event (job INTEGER NOT NULL REFERENCES job(key) ON DELETE CASCADE,
  seq INTEGER NOT NULL, at_ms INTEGER NOT NULL, type TEXT NOT NULL,
  data TEXT NOT NULL, PRIMARY KEY (job, seq)) STRICT;                      -- persisted events only (JB8)

-- OB2, OB4 (PR 4's review): every column naming an object is indexed, so
-- a lookup of what holds an object is one index probe per table; and an
-- object whose naming row goes becomes a candidate, so the sweep looks
-- only at candidates, never at every object.
CREATE INDEX upload_object ON upload(sha256);
CREATE INDEX upload_expiry ON upload(expires_ms);
CREATE INDEX assembly_object ON assembly(sha256);
CREATE INDEX assembly_parts_object ON assembly_parts(document_sha256);
CREATE INDEX socket_revision_object ON socket_revision(document_sha256);
CREATE INDEX job_input_object ON job_input(sha256);
CREATE INDEX job_result ON job(result_sha256);
CREATE INDEX job_index ON job(index_sha256);
-- JS13 (PR 5): a re-run after a crash looks for a row naming its job.
CREATE INDEX assembly_parts_job ON assembly_parts(job);
CREATE INDEX socket_revision_job ON socket_revision(job);
CREATE INDEX socket_revision_parent ON socket_revision(parent);
CREATE TABLE object_candidate (sha256 TEXT PRIMARY KEY) STRICT;            -- OB4: the sweep's to look at
CREATE TRIGGER upload_gone AFTER DELETE ON upload
BEGIN INSERT OR IGNORE INTO object_candidate VALUES (OLD.sha256); END;
CREATE TRIGGER assembly_gone AFTER DELETE ON assembly
BEGIN INSERT OR IGNORE INTO object_candidate VALUES (OLD.sha256); END;
CREATE TRIGGER assembly_parts_gone AFTER DELETE ON assembly_parts
BEGIN INSERT OR IGNORE INTO object_candidate VALUES (OLD.document_sha256); END;
CREATE TRIGGER socket_revision_gone AFTER DELETE ON socket_revision
BEGIN INSERT OR IGNORE INTO object_candidate VALUES (OLD.document_sha256); END;
CREATE TRIGGER job_input_gone AFTER DELETE ON job_input
BEGIN INSERT OR IGNORE INTO object_candidate VALUES (OLD.sha256); END;
CREATE TRIGGER job_gone AFTER DELETE ON job
BEGIN
  INSERT OR IGNORE INTO object_candidate SELECT OLD.result_sha256 WHERE OLD.result_sha256 IS NOT NULL;
  INSERT OR IGNORE INTO object_candidate SELECT OLD.index_sha256 WHERE OLD.index_sha256 IS NOT NULL;
END;
