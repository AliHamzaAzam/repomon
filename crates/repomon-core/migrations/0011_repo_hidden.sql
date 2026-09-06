-- Hiding a repository must preserve registration and its associated state.
ALTER TABLE repos ADD COLUMN hidden INTEGER NOT NULL DEFAULT 0;
