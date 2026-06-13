-- Push-notification device registrations (the iOS companion app). One row per device token;
-- re-registering refreshes the timestamp, APNs-reported dead tokens are evicted by the daemon.
CREATE TABLE IF NOT EXISTS devices (
    device_token  TEXT PRIMARY KEY,
    registered_at TEXT NOT NULL
);
