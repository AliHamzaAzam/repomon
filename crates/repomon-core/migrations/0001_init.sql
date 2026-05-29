-- repomon schema v1.
-- All timestamps are RFC3339 UTC text; object ids are lowercase hex text.
-- IF NOT EXISTS keeps the migration safe to (re)apply against a pre-existing database.

CREATE TABLE IF NOT EXISTS repos (
    id                     INTEGER PRIMARY KEY,
    path                   TEXT NOT NULL UNIQUE,
    name                   TEXT NOT NULL,
    added_at               TEXT NOT NULL,
    worktree_root_template TEXT
);

CREATE TABLE IF NOT EXISTS worktrees (
    id      INTEGER PRIMARY KEY,
    repo_id INTEGER NOT NULL REFERENCES repos(id) ON DELETE CASCADE,
    path    TEXT NOT NULL UNIQUE,
    branch  TEXT,
    head    TEXT NOT NULL,
    is_main INTEGER NOT NULL DEFAULT 0,
    name    TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_worktrees_repo ON worktrees(repo_id);

-- A lane is identified by (repo, worktree path); its id is assigned once and stays
-- stable across daemon restarts. Pin state and the tmux window live here too.
CREATE TABLE IF NOT EXISTS lanes (
    id            INTEGER PRIMARY KEY,
    repo_id       INTEGER NOT NULL REFERENCES repos(id) ON DELETE CASCADE,
    worktree_path TEXT NOT NULL,
    pinned        INTEGER NOT NULL DEFAULT 0,
    tmux_window   TEXT,
    created_at    TEXT NOT NULL,
    UNIQUE(repo_id, worktree_path)
);

CREATE TABLE IF NOT EXISTS commits (
    repo_id      INTEGER NOT NULL REFERENCES repos(id) ON DELETE CASCADE,
    oid          TEXT NOT NULL,
    author_name  TEXT NOT NULL,
    author_email TEXT NOT NULL,
    summary      TEXT NOT NULL,
    time         TEXT NOT NULL,
    parent_count INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (repo_id, oid)
);
CREATE INDEX IF NOT EXISTS idx_commits_time ON commits(time);

CREATE TABLE IF NOT EXISTS agent_sessions (
    id               INTEGER PRIMARY KEY,
    agent            TEXT NOT NULL,
    repo_id          INTEGER NOT NULL REFERENCES repos(id) ON DELETE CASCADE,
    worktree_id      INTEGER REFERENCES worktrees(id) ON DELETE SET NULL,
    started_at       TEXT NOT NULL,
    last_activity_at TEXT NOT NULL,
    ended_at         TEXT,
    manifest_path    TEXT NOT NULL UNIQUE,
    tool_call_count  INTEGER NOT NULL DEFAULT 0,
    title            TEXT
);
CREATE INDEX IF NOT EXISTS idx_sessions_repo ON agent_sessions(repo_id);
CREATE INDEX IF NOT EXISTS idx_sessions_active ON agent_sessions(ended_at);
