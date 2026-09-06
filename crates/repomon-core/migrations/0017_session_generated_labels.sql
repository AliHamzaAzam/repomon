
CREATE TABLE IF NOT EXISTS session_generated_labels (
    session_id TEXT PRIMARY KEY,
    label      TEXT NOT NULL,
    created_at TEXT NOT NULL
);
