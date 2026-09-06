-- Store individually revocable device credentials alongside the shared config token, using the same
-- local plaintext-storage trust boundary.
CREATE TABLE remote_devices (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  name TEXT NOT NULL UNIQUE,
  token TEXT NOT NULL UNIQUE,
  role TEXT NOT NULL DEFAULT 'full',
  created_at TEXT NOT NULL,
  last_seen_at TEXT
);
