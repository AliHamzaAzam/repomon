-- Nested Claude subagent transcripts contribute to the parent session while retaining a separate
-- subagent token total.
ALTER TABLE usage_events ADD COLUMN subagent INTEGER NOT NULL DEFAULT 0;

CREATE INDEX idx_usage_events_source ON usage_events(source_path);
