-- A full reorder assigns dense positions; NULL preserves name ordering, while a NULL label
-- preserves the folder name.
ALTER TABLE repos ADD COLUMN position INTEGER;
ALTER TABLE repos ADD COLUMN label TEXT;
