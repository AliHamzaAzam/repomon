-- Orchestration journal: every orchestrator-initiated action (spawn/send/approve/merge/... plus
-- session_start markers), appended by the MCP layer via journal.append. Durable memory of "what
-- did repomind do and why" that outlives tmux scrollback.
CREATE TABLE IF NOT EXISTS orchestration_log (
    id      INTEGER PRIMARY KEY AUTOINCREMENT,
    at      TEXT NOT NULL,
    session TEXT NOT NULL,
    action  TEXT NOT NULL,
    lane_id INTEGER,
    repo    TEXT,
    params  TEXT,
    outcome TEXT NOT NULL,
    detail  TEXT
);
CREATE INDEX IF NOT EXISTS idx_orchestration_log_at ON orchestration_log(at);
CREATE INDEX IF NOT EXISTS idx_orchestration_log_action ON orchestration_log(action);
