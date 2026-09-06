-- Keep a pending revision separate from approved content until human approval.
CREATE TABLE IF NOT EXISTS playbooks (
    name          TEXT PRIMARY KEY,
    content       TEXT NOT NULL,
    status        TEXT NOT NULL DEFAULT 'draft',
    draft_content TEXT,
    created_at    TEXT NOT NULL,
    updated_at    TEXT NOT NULL,
    approved_at   TEXT
);
