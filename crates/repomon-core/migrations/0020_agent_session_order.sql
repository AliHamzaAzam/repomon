-- Manual per-lane agent tab ordering. One row per (lane, session): `position` is a dense
-- integer assigned by a full reorder (`agent.set_tab_order`). Sessions absent from the table
-- keep the historical activity order and newly seen agents append.
CREATE TABLE agent_session_order (
    lane_id    INTEGER NOT NULL,
    session_id TEXT NOT NULL,
    position   INTEGER NOT NULL,
    PRIMARY KEY (lane_id, session_id)
);
