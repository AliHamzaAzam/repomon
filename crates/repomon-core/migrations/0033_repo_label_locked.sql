-- A label the person here chose, including a deliberate clear back to the folder name, must not be
-- reseeded from the repository's own repo.json on the next add. `label IS NULL` cannot express that
-- difference, so record the choice itself. Rows that predate this migration default to unlocked:
-- a reset made before it is not recoverable and will be reseeded once more.
ALTER TABLE repos ADD COLUMN label_locked INTEGER NOT NULL DEFAULT 0;
