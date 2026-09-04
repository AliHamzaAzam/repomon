-- A lane's role in the fleet. NULL (the overwhelming majority) is an ordinary work lane; the one
-- lane whose role is 'controller' is the repomind home lane, where controller agents run with the
-- full fleet catalog. Nullable and unconstrained on purpose: a role is metadata the daemon sets,
-- never something a worktree scan can derive, and future roles should not need another migration.
ALTER TABLE lanes ADD COLUMN role TEXT;
