-- Make lane ids monotonic. The original `lanes` table (0001) used `id INTEGER PRIMARY KEY`
-- WITHOUT AUTOINCREMENT, so SQLite reuses a freed rowid: when a worktree's lane row is removed
-- and a re-registered worktree later gets a new lane, it could inherit a still-running tmux
-- window's id (stale "lane-<id>" windows). Rebuilding with AUTOINCREMENT makes ids strictly
-- increasing, so a freed id is never handed back out.
--
-- Standard SQLite table-rebuild recipe: create the replacement with the AUTOINCREMENT id and
-- otherwise-identical columns/constraints, copy every row preserving its explicit id, drop the
-- old table, and rename. Nothing has a foreign key referencing lanes.id, so this is safe; the
-- columns mirror 0001_init.sql plus the `agent_kind` column added in 0002_agent_kind.sql.
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

-- Preserve existing ids. AUTOINCREMENT records the largest inserted id in sqlite_sequence, so
-- subsequent inserts continue strictly above it rather than reusing a freed value.
INSERT INTO lanes_new (id, repo_id, worktree_path, pinned, tmux_window, created_at, agent_kind)
SELECT id, repo_id, worktree_path, pinned, tmux_window, created_at, agent_kind FROM lanes;

DROP TABLE lanes;
ALTER TABLE lanes_new RENAME TO lanes;
