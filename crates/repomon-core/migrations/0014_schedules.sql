-- Each unattended run carries its own action cap.
CREATE TABLE IF NOT EXISTS schedules (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    spec        TEXT NOT NULL,
    prompt      TEXT NOT NULL,
    max_actions INTEGER NOT NULL,
    created_at  TEXT NOT NULL,
    last_run_at TEXT
);
