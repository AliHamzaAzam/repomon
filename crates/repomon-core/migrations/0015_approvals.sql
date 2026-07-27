-- Approval-policy memory: every escalated permission verdict (events), and the human-confirmed
-- per-repo allowlist (rules) the daemon consults before escalating a routine Bash permission.
CREATE TABLE IF NOT EXISTS approval_events (
    id      INTEGER PRIMARY KEY AUTOINCREMENT,
    repo    TEXT NOT NULL,
    pattern TEXT NOT NULL,
    verdict TEXT NOT NULL,
    at      TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_approval_events_repo_pattern ON approval_events(repo, pattern);

CREATE TABLE IF NOT EXISTS approval_rules (
    repo       TEXT NOT NULL,
    pattern    TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY (repo, pattern)
);
