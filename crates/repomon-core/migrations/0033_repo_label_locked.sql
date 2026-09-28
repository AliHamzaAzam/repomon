-- A label the person here chose, including a deliberate clear back to the folder name, must not be
-- reseeded from the repository's own repo.json on the next add. `label IS NULL` cannot express that
-- difference, so record the choice itself. Existing rows default to unlocked, which is correct:
-- no released build ever seeded a label, so there is no earlier reset to preserve.
ALTER TABLE repos ADD COLUMN label_locked INTEGER NOT NULL DEFAULT 0;
