-- Which revision of the headline extractor produced a session's stored headline. A row left at 0
-- predates every versioned rule; ingest re-digests it from its source, a bounded batch per tick,
-- until it reflects the current extractor.
ALTER TABLE usage_sessions ADD COLUMN headline_version INTEGER NOT NULL DEFAULT 0;
