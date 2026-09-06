-- Never reuse a deleted lane ID while a surviving backend window may still carry it. Rebuild with
-- AUTOINCREMENT while preserving IDs; no foreign key references lanes.id at this migration version.
CREATE TABLE lanes_new (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    repo_id       INTEGER NOT NULL REFERENCES repos(id) ON DELETE CASCADE,
    worktree_path TEXT NOT NULL,
    pinned        INTEGER NOT NULL DEFAULT 0,
    tmux_window   TEXT,
    created_at    TEXT NOT NULL,
    agent_kind    TEXT,
    UNIQUE(repo_id, worktree_path)
);

-- Explicit IDs advance sqlite_sequence so later inserts cannot reuse them.
INSERT INTO lanes_new (id, repo_id, worktree_path, pinned, tmux_window, created_at, agent_kind)
SELECT id, repo_id, worktree_path, pinned, tmux_window, created_at, agent_kind FROM lanes;

DROP TABLE lanes;
ALTER TABLE lanes_new RENAME TO lanes;
