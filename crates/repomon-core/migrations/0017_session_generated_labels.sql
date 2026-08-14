-- Auto-generated session names created by the local LLM subsystem.
-- Keyed by the durable Claude/agent transcript session id.
CREATE TABLE IF NOT EXISTS session_generated_labels (
    session_id TEXT PRIMARY KEY,
    label      TEXT NOT NULL,
    created_at TEXT NOT NULL
);
