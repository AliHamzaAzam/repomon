-- Device tokens are refreshed on registration and evicted when APNs reports them dead.
CREATE TABLE IF NOT EXISTS devices (
    device_token  TEXT PRIMARY KEY,
    registered_at TEXT NOT NULL
);
