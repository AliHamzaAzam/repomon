-- A full reorder assigns dense per-lane positions; unrecorded sessions retain activity order and
-- newly seen agents append.
CREATE TABLE agent_session_order (
    lane_id    INTEGER NOT NULL,
    session_id TEXT NOT NULL,
    position   INTEGER NOT NULL,
    PRIMARY KEY (lane_id, session_id)
);
