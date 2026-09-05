-- The usage ledger: one row per billable agent turn, its daily rollup, the per-session digest
-- the sessions table reads, and the ingest cursors that make a re-read idempotent.
--
-- Costs are deliberately absent: the ledger stores tokens and every query re-prices them, so a
-- corrected rate corrects history. `repo_id` and `lane_id` are plain integers rather than foreign
-- keys because a turn can predate the repo being added, or outlive the lane being deleted, and
-- losing the row would lose the spend it records.

CREATE TABLE usage_events (
  id                 INTEGER PRIMARY KEY AUTOINCREMENT,
  at                 TEXT    NOT NULL,
  agent_kind         TEXT    NOT NULL,
  model              TEXT    NOT NULL,
  account            TEXT    NOT NULL,
  lane_id            INTEGER,
  repo_id            INTEGER,
  session_id         TEXT,
  cwd                TEXT,
  window             TEXT,
  input_tokens       INTEGER NOT NULL DEFAULT 0,
  output_tokens      INTEGER NOT NULL DEFAULT 0,
  cache_read_tokens  INTEGER NOT NULL DEFAULT 0,
  cache_write_tokens INTEGER NOT NULL DEFAULT 0,
  thinking_tokens    INTEGER NOT NULL DEFAULT 0,
  estimated          INTEGER NOT NULL DEFAULT 0,
  external           INTEGER NOT NULL DEFAULT 0,
  source_path        TEXT    NOT NULL,
  source_offset      INTEGER NOT NULL,
  UNIQUE(source_path, source_offset)
);

CREATE INDEX idx_usage_events_at ON usage_events(at);
CREATE INDEX idx_usage_events_session ON usage_events(agent_kind, session_id);
CREATE INDEX idx_usage_events_lane ON usage_events(lane_id, at);

-- Where each source was read up to. `offset` is a byte offset for line sources and a millisecond
-- epoch watermark for the OpenCode database.
CREATE TABLE usage_ingest_cursors (
  source_path TEXT    PRIMARY KEY,
  offset      INTEGER NOT NULL DEFAULT 0,
  mtime       INTEGER NOT NULL DEFAULT 0,
  scanned_at  TEXT    NOT NULL,
  error       TEXT
);

-- Daily rollups, maintained as events are inserted. `repo_id` and `lane_id` use 0 for "none" so
-- the unique key works: SQLite treats NULLs in a unique index as distinct from each other.
CREATE TABLE usage_daily (
  day                TEXT    NOT NULL,
  agent_kind         TEXT    NOT NULL,
  model              TEXT    NOT NULL,
  account            TEXT    NOT NULL,
  repo_id            INTEGER NOT NULL DEFAULT 0,
  lane_id            INTEGER NOT NULL DEFAULT 0,
  input_tokens       INTEGER NOT NULL DEFAULT 0,
  output_tokens      INTEGER NOT NULL DEFAULT 0,
  cache_read_tokens  INTEGER NOT NULL DEFAULT 0,
  cache_write_tokens INTEGER NOT NULL DEFAULT 0,
  thinking_tokens    INTEGER NOT NULL DEFAULT 0,
  estimated_tokens   INTEGER NOT NULL DEFAULT 0,
  events             INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (day, agent_kind, model, account, repo_id, lane_id)
);

CREATE INDEX idx_usage_daily_day ON usage_daily(day);

-- One row per agent session, for the sessions table's headline and counters.
CREATE TABLE usage_sessions (
  agent_kind  TEXT NOT NULL,
  session_id  TEXT NOT NULL,
  headline    TEXT,
  cwd         TEXT,
  repo_id     INTEGER,
  lane_id     INTEGER,
  started_at  TEXT,
  ended_at    TEXT,
  turns       INTEGER NOT NULL DEFAULT 0,
  tool_calls  INTEGER NOT NULL DEFAULT 0,
  retries     INTEGER NOT NULL DEFAULT 0,
  external    INTEGER NOT NULL DEFAULT 0,
  source_path TEXT,
  PRIMARY KEY (agent_kind, session_id)
);

CREATE INDEX idx_usage_sessions_started ON usage_sessions(started_at);
