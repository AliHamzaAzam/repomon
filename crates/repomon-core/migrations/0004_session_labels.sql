-- User-set short labels for agent sessions, keyed by the durable Claude transcript session id
-- (stable across refreshes and daemon restarts; a new agent in a reused slot gets a new id, so a
-- label never bleeds onto an unrelated successor). Overlaid onto AgentSession at lane.list time.
CREATE TABLE IF NOT EXISTS session_labels (
    session_id TEXT PRIMARY KEY,
    label      TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
