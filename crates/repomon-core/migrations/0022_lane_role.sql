-- The daemon assigns lane roles independently of worktree discovery; NULL means ordinary work, and
-- controller identifies the fleet-memory home.
ALTER TABLE lanes ADD COLUMN role TEXT;
