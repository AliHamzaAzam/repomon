-- Versioned extraction allows bounded replay of stale headlines under current rules.
ALTER TABLE usage_sessions ADD COLUMN headline_version INTEGER NOT NULL DEFAULT 0;
