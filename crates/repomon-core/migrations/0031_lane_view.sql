ALTER TABLE lanes ADD COLUMN view_mode TEXT CHECK (view_mode IN ('terminal', 'conversation'));
