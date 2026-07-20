-- Standing-orchestration schedules: bounded headless repomind runs the daemon fires on a spec
-- ("daily 09:00", "every 30m", ...). max_actions is the run's (deliberately lower) action cap.
CREATE TABLE IF NOT EXISTS schedules (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    spec        TEXT NOT NULL,
    prompt      TEXT NOT NULL,
    max_actions INTEGER NOT NULL,
    created_at  TEXT NOT NULL,
    last_run_at TEXT
);
