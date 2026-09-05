-- Claude Code writes each subagent's transcript one directory below the session file, at
-- `<project>/<session>/subagents/agent-<id>.jsonl`. Those turns belong to the session that spawned
-- them, so they fold into its row; this column is what lets a session say how much of its spend
-- was its subagents rather than its own turns.
ALTER TABLE usage_events ADD COLUMN subagent INTEGER NOT NULL DEFAULT 0;

CREATE INDEX idx_usage_events_source ON usage_events(source_path);
