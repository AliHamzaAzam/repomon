# Auto-open the just-created agent

**Date:** 2026-07-19
**Status:** Approved

## Problem

Creating an agent must open it without hiding the fleet context. The earlier implementation
misread "open" as the full-screen Focus view, which removes the sidebar.

The user's intent: after creating an agent, land in the Split view with the fleet sidebar
visible and `INSERT` active — typing goes straight to the new agent, zero extra keypresses.

## Design

### Shared landing helper

Extract `do_spawn`'s post-spawn landing into one helper on `App`:

```rust
/// Land on a just-spawned agent, typing-ready: select its lane in the fleet,
/// arm the pending-focus intent (which also routes keys to the window before
/// lane.list shows it), and enter Split with insert mode on.
fn land_on_spawned(&mut self, lane: LaneId, window: Option<String>)
```

Behavior:

1. `select_lane_session(lane, None)` — point the fleet cursor at the lane
   (matters for the New Lane path; a same-lane `e` spawn is a no-op).
2. If `window` is `Some`, set `pending_focus_window = (lane, window)` and reset
   `pending_focus_ticks` — the cursor snaps to the new agent's session when it
   appears in `lane.list`, and `selected_window()` routes keystrokes to it
   immediately (PR #65).
3. `reset_scroll()` — show the live tail, mirroring what `i` does today.
4. `view = View::Split`, `focus_insert = true`.

`focus_managed` needs no handling: `check_focus_alive` derives it per tick.

### Call sites

- **`do_spawn`** (all `e`-spawn paths): use the shared helper so the new
  agent opens in Split with insert mode on.
- **`submit_new_lane`**: capture the `agent.spawn` response instead of
  discarding it (`let _ =`). On spawn success: keep the status line, `refresh()`
  first (so the new lane exists in `self.lanes` for `select_lane_session`),
  then call the helper with the returned window. On spawn **failure**: keep
  today's behavior (Fleet + refresh) — there is no agent to type to; surface
  the spawn error in `status` instead of swallowing it.

### Trade-off (accepted)

With insert on by default, command keys (`e`, `s`, `m`, …) right after a spawn
require `^O` first. That is what "open and typing-ready" means; Esc continues to
forward to the agent (existing design, guarded by `esc_is_forwarded_not_captured`).

## Testing

Unit tests in `app.rs::tests` using the existing dummy-client harness
(`app_on_lane_7`), driving the helper directly (the RPC halves of `do_spawn` /
`submit_new_lane` stay thin):

1. **`e`-spawn landing:** `land_on_spawned(7, Some("lane-7-2"))` → view is
   Split, `focus_insert` on, `pending_focus_window` armed,
   `selected_window() == "lane-7-2"` before the session appears in `lane.list`.
2. **New-lane landing:** app with two lanes, selection on the first;
   `land_on_spawned(8, Some("lane-8"))` → fleet selection moves to lane 8,
   Split + insert on, keys route to `lane-8`.
3. **No window (spawn response malformed):** lands in Split + insert with no
   pending intent, routing falls back to the selected session.

Existing PR #65 tests already cover intent resolution, expiry, and Tab cancel.

## Out of scope

- Full tmux attach on spawn (rejected: heavier than the mediated Split view;
  attaching mid-boot is janky and hides the sidebar).
- Auto-insert when merely *navigating* to an agent (only creation opens it).
