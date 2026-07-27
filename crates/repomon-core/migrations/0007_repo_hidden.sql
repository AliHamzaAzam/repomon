-- Hide a repo from client sidebars without unregistering it. Registration is destructive enough
-- that "stop showing me this project" needed its own, reversible switch.
ALTER TABLE repos ADD COLUMN hidden INTEGER NOT NULL DEFAULT 0;
