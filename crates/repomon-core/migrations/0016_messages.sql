CREATE TABLE IF NOT EXISTS mcp_identities (
    token_hash TEXT PRIMARY KEY,
    address TEXT NOT NULL,
    lane_id INTEGER,
    slot INTEGER,
    window TEXT,
    session_id TEXT,
    agent_kind TEXT,
    created_at TEXT NOT NULL,
    revoked_at TEXT
);
CREATE INDEX IF NOT EXISTS idx_mcp_identities_window
    ON mcp_identities(window) WHERE revoked_at IS NULL;

CREATE TABLE IF NOT EXISTS messages (
    id TEXT PRIMARY KEY,
    requested_to TEXT NOT NULL,
    sender_address TEXT NOT NULL,
    sender_lane_id INTEGER,
    sender_slot INTEGER,
    sender_window TEXT,
    sender_session_id TEXT,
    sender_agent_kind TEXT,
    recipient_address TEXT NOT NULL,
    recipient_lane_id INTEGER,
    recipient_slot INTEGER,
    recipient_window TEXT,
    recipient_session_id TEXT,
    recipient_agent_kind TEXT,
    body TEXT NOT NULL,
    thread_id TEXT NOT NULL,
    reply_to TEXT REFERENCES messages(id),
    remaining_hops INTEGER NOT NULL,
    created_at TEXT NOT NULL,
    delivered_at TEXT,
    read_at TEXT,
    delivery_error TEXT
);
CREATE INDEX IF NOT EXISTS idx_messages_recipient_created
    ON messages(recipient_address, created_at DESC, id DESC);
CREATE INDEX IF NOT EXISTS idx_messages_recipient_unread
    ON messages(recipient_address, read_at, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_messages_recipient_lane
    ON messages(recipient_lane_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_messages_sender_rate
    ON messages(sender_address, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_messages_thread
    ON messages(thread_id, created_at DESC);
