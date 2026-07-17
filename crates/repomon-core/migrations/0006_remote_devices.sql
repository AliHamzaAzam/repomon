-- Per-device remote-access credentials. Pairing mints one named, individually-revocable bearer
-- token per companion device (superseding the single shared `[remote] token` in config.toml,
-- which keeps working alongside these). Tokens are stored in plaintext deliberately: same threat
-- model as the config-file token they replace. Hashing them at rest is a future-hardening
-- candidate once the config token is retired.
CREATE TABLE remote_devices (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  name TEXT NOT NULL UNIQUE,
  token TEXT NOT NULL UNIQUE,
  role TEXT NOT NULL DEFAULT 'full',
  created_at TEXT NOT NULL,
  last_seen_at TEXT
);
