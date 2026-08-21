-- Manual repo ordering and custom display labels. `position` is a dense integer assigned by a
-- full reorder (`repo.reorder`); NULL keeps the repo in the legacy name order. `label` is an
-- optional display-name override shown instead of the folder name; NULL means no override.
ALTER TABLE repos ADD COLUMN position INTEGER;
ALTER TABLE repos ADD COLUMN label TEXT;
