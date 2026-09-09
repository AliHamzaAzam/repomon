-- Claim before terminal I/O. An uncertain write is never automatically replayed.
CREATE TABLE message_push_attempts (
    message_id TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    window TEXT NOT NULL,
    attempted_at TEXT NOT NULL,
    PRIMARY KEY (message_id, window)
);
