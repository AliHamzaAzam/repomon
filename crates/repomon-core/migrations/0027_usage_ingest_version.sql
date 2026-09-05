-- Which revision of the readers produced a source's events, and which revision counted a
-- session's turns. A cursor left behind by an older reader is re-read from the start and its
-- events replaced; a session digest left behind by one has its counters reset rather than added
-- to, so a correction to a reader converges instead of doubling what it already recorded.
ALTER TABLE usage_ingest_cursors ADD COLUMN ingest_version INTEGER NOT NULL DEFAULT 0;
ALTER TABLE usage_sessions ADD COLUMN counts_version INTEGER NOT NULL DEFAULT 0;
