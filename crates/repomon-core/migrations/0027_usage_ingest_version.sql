-- Reader-version changes replace source events and reset digest counters during replay so
-- corrections converge without double counting.
ALTER TABLE usage_ingest_cursors ADD COLUMN ingest_version INTEGER NOT NULL DEFAULT 0;
ALTER TABLE usage_sessions ADD COLUMN counts_version INTEGER NOT NULL DEFAULT 0;
