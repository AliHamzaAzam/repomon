-- Playbooks: procedural memory drafted by the orchestrator after completed goals, inert until a
-- human approves it. `content` is the live text (draft text before approval, approved text
-- after); `draft_content` holds a pending revision saved over an approved playbook.
CREATE TABLE IF NOT EXISTS playbooks (
    name          TEXT PRIMARY KEY,
    content       TEXT NOT NULL,
    status        TEXT NOT NULL DEFAULT 'draft',
    draft_content TEXT,
    created_at    TEXT NOT NULL,
    updated_at    TEXT NOT NULL,
    approved_at   TEXT
);
