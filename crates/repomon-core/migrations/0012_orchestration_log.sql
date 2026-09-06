-- Persist orchestrator actions and session boundaries beyond terminal scrollback.
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
