-- Persist the spawned kind so agents without parseable transcripts remain identifiable.
ALTER TABLE lanes ADD COLUMN agent_kind TEXT;
