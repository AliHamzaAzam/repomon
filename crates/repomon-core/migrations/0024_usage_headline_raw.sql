-- The text a session headline was read from, before the extractor stripped the blocks the CLI
-- injected into the turn. Kept so the desktop can show the operator what was really written.
ALTER TABLE usage_sessions ADD COLUMN headline_raw TEXT;
