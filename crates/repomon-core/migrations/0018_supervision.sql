CREATE TABLE IF NOT EXISTS lane_policies (
    lane_id INTEGER PRIMARY KEY,
    enabled INTEGER NOT NULL DEFAULT 0,
    classes TEXT NOT NULL DEFAULT '{}',
    mail_mode TEXT,
    nudge_text TEXT,
    stall_mins INTEGER,
    nudge_retries INTEGER,
    expect_work INTEGER NOT NULL DEFAULT 0,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS supervision_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    at TEXT NOT NULL,
    lane_id INTEGER NOT NULL,
    window TEXT NOT NULL,
    session_id TEXT,
    agent_kind TEXT,
    trigger TEXT NOT NULL,
    dialog_class TEXT,
    repo_scoped INTEGER,
    decision TEXT NOT NULL,
    policy_source TEXT,
    keys TEXT,
    outcome TEXT NOT NULL,
    reason TEXT,
    subject TEXT,
    pane_excerpt TEXT
);
CREATE INDEX IF NOT EXISTS idx_supervision_log_lane ON supervision_log(lane_id, id);
CREATE INDEX IF NOT EXISTS idx_supervision_log_at ON supervision_log(at);
