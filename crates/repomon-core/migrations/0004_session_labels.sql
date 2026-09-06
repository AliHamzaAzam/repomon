-- Key labels by durable session identity so they survive refreshes without leaking to unrelated
-- agents in recycled slots.
CREATE TABLE IF NOT EXISTS session_labels (
    session_id TEXT PRIMARY KEY,
    label      TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
