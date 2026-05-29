-- Record which agent kind repomon spawned in a lane, so non-Claude agents (Codex,
-- Aider, …) can be identified and shown even without a parseable transcript.
ALTER TABLE lanes ADD COLUMN agent_kind TEXT;
