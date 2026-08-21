//! Orphaned agent-window reaper.
//!
//! repomon's tmux server (`tmux -L <session>`) is meant to be long-lived — it survives daemon
//! restarts and even a wiped store. When the store is reset or a worktree is re-registered, lane
//! ids get reassigned, but the `lane-<id>` windows spawned under the old ids keep running. Those
//! windows (and their idle `claude` processes, which never exit on their own) become unreachable
//! garbage: they no longer map to the worktree their name claims, yet their cwd still inflates
//! the path-keyed live-process count in `overlay_agents`, surfacing phantom "external" sessions
//! the user can't dismiss. This module finds and kills them.
//!
//! It's also the only place positioned to notice when that long-lived assumption fails outright
//! — the tmux server itself dies (killed out from under the daemon; nothing here reaps it one
//! window at a time), taking every agent it hosted with it. See
//! [`note_possible_session_loss`].

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use repomon_core::agent::backend::OwnerState;
use repomon_core::agent::tmux::TmuxRuntime;
use repomon_core::model::LaneId;
use repomon_core::notify::send_native;

use crate::Ctx;

/// How often the reaper sweeps. The first sweep runs immediately (tokio interval fires at once),
/// so a daemon that just started against a tmux server full of stale windows self-heals on boot;
/// the slow cadence after that catches runtime churn without racing freshly-spawned windows.
const TICK: Duration = Duration::from_secs(60);

/// A managed tmux window as the reaper sees it: name, canonical pane cwd, and whether its agent
/// is currently active (pane produced output within [`RUNNING_GRACE`]).
struct Win {
    name: String,
    cwd: PathBuf,
    active: bool,
}

/// Output silence after which an orphan's agent is treated as idle (not running) and so reapable.
/// An actively-working `claude` streams tokens/tool output continuously, so its window's activity
/// time stays fresh; an idle one sitting at its prompt goes quiet. Generous, so a brief lull in a
/// long task doesn't get an active agent reaped — the user's "don't reap a running agent" rule.
const RUNNING_GRACE: Duration = Duration::from_secs(300);

/// Names of stale `lane-<id>` agent windows *this sweep* considers orphaned: the id maps to no
/// current lane, or the worktree for that id no longer lives at the window's pane cwd. Both mean
/// the window is a leftover from a re-registered / renumbered worktree (e.g. the tmux server
/// outliving a store reset) — a managed `claude` is spawned with `-c <worktree>` and never
/// chdirs, so a cwd mismatch is proof the window belongs to a defunct generation. An **active**
/// window is never reaped, even when orphaned, so a still-running agent is left alone. Non-lane
/// windows (terminals, the usage probe) are ignored.
///
/// This is a single-snapshot judgment, not a kill decision: a transient bad read — `ctx.lanes
/// .list()` racing a store write, or `Path::canonicalize()` faulting/mismatching under disk or fd
/// pressure — can flag a real, live window as orphaned for one sweep. That used to be low-stakes,
/// because the old `kill_named` only ran tmux `kill-window`, which a stubborn or reparented CLI
/// child could simply outlive. Since d46a340 ("fix(core): terminate pane descendants on window
/// stop"), `kill_named` walks the pane's full process tree and `SIGTERM`s every descendant
/// directly, so a false positive here now genuinely and unrecoverably kills a live agent session.
/// The caller (`reap_orphan_windows`) is responsible for requiring a window to appear in this
/// list across [`ORPHAN_CONFIRM`] consecutive sweeps — see `confirm_orphans` — before it's ever
/// passed to `kill_and_forget`. Kept pure (no `Ctx`) so it stays independently unit-testable.
fn orphan_lane_windows(windows: &[Win], lane_paths: &HashMap<LaneId, PathBuf>) -> Vec<String> {
    windows
        .iter()
        .filter_map(|w| {
            if w.active {
                return None; // a running agent is spared regardless of orphan status
            }
            let id = TmuxRuntime::lane_id_of(&w.name)?;
            match lane_paths.get(&id) {
                None => Some(w.name.clone()),
                Some(path) if path != &w.cwd => Some(w.name.clone()),
                _ => None,
            }
        })
        .collect()
}

fn canonical(p: &Path) -> PathBuf {
    p.canonicalize().unwrap_or_else(|_| p.to_path_buf())
}

/// Consecutive sweeps a window must be classified orphaned (by [`orphan_lane_windows`]) before
/// [`reap_orphan_windows`] actually kills it. Mirrors `rpc::resolve_windows`'s
/// `EMPTY_WINDOWS_CONFIRM`: one bad snapshot must not be trusted as ground truth. See the doc
/// comment on `orphan_lane_windows` for why that matters much more since d46a340.
const ORPHAN_CONFIRM: u32 = 2;

/// Consecutive sweeps a zero-window server must be seen with live lanes still in the store
/// before [`note_possible_session_loss`] treats it as a real total-session loss rather than a
/// transient tmux hiccup (same "don't trust a single bad snapshot" rule as [`ORPHAN_CONFIRM`]).
/// Fires exactly once per incident: the counter keeps climbing past this past the fire point, so
/// it isn't re-alerted every sweep, and resets to 0 the moment windows reappear.
const SESSION_LOSS_CONFIRM: u32 = 2;

/// Pure state transition for the session-loss debounce: given the previous consecutive-sweep
/// count and whether *this* sweep found live lanes with zero windows, returns the count to
/// persist and whether this sweep should fire the alert. A scalar-counter analog of
/// `confirm_orphans`'s per-window map: any non-matching sweep (windows reappeared, or no lanes to
/// lose) resets to 0 immediately rather than expiring on its own, and the alert fires on the
/// exact sweep the count reaches [`SESSION_LOSS_CONFIRM`] — not on every sweep after, so one
/// incident produces one alert. Kept pure so it stays independently unit-testable, same rationale
/// as `orphan_lane_windows`.
fn advance_session_loss(count: u32, lanes_but_no_windows: bool) -> (u32, bool) {
    if !lanes_but_no_windows {
        return (0, false);
    }
    let count = count + 1;
    let fire = count == SESSION_LOSS_CONFIRM;
    (count, fire)
}

/// Called when a sweep finds the tmux server reporting zero windows while the store still lists
/// `lane_count` live lanes. A healthy reaper never sees this: a real orphan sweep kills windows
/// one at a time and always leaves the rest, so an all-at-once wipe means the tmux server itself
/// died out from under the daemon (see `Ctx::session_loss_confirm`'s doc comment) — every agent
/// hosted in it is gone, unrecoverably, and nothing else in the daemon would otherwise notice or
/// tell the user. Debounced across [`SESSION_LOSS_CONFIRM`] sweeps and fired only once per
/// incident via [`advance_session_loss`].
async fn note_possible_session_loss(ctx: &Ctx, lane_count: usize) {
    let mut count = ctx.session_loss_confirm.lock().await;
    let (new_count, fire) = advance_session_loss(*count, lane_count > 0);
    *count = new_count;
    drop(count);
    if !fire {
        return;
    }

    tracing::error!(
        lane_count,
        session = %ctx.backend.label(),
        "tmux server has zero windows but the store lists live lanes — the backing tmux \
         server was lost and every agent it hosted is gone; a fresh empty session will be \
         created on next connect"
    );

    let title = "Repomon lost its agent session";
    let body = format!(
        "The tmux server backing your agents disappeared — {lane_count} lane{} lost their \
         agent windows. Reconnecting will start a fresh empty session; nothing here can bring \
         the old windows back.",
        if lane_count == 1 { "" } else { "s" }
    );
    ctx.broadcast(
        "event.notification",
        serde_json::json!({
            "kind": "SessionLost",
            "title": title,
            "body": body,
            "lane_count": lane_count,
        }),
    );

    let cfg = ctx.config.read().await.clone();
    if cfg.notify_enabled {
        send_native(title, &body, cfg.notify_sound, cfg.notify_click_focus);
    }
}

/// Advance the reaper's cross-sweep confirmation state given this sweep's orphan set, returning
/// the updated state to persist (in [`Ctx::orphan_confirm`]) and the window names that have now
/// reached [`ORPHAN_CONFIRM`] and should be killed this sweep.
///
/// Any window not in `orphans` this sweep — its lane reappeared in `lane_paths` with a matching
/// cwd, or it became active — has its counter cleared immediately, not merely left to expire.
/// That's the key correctness property (mirroring `resolve_windows`'s reset-on-good-read
/// semantics): a window orphaned once, seen fine on the very next sweep, then orphaned again must
/// restart confirmation from zero, so two *non-consecutive* orphan sightings can never combine
/// into a kill.
fn confirm_orphans(
    mut state: HashMap<String, u32>,
    orphans: &[String],
) -> (HashMap<String, u32>, Vec<String>) {
    state.retain(|name, _| orphans.iter().any(|o| o == name));

    let mut to_kill = Vec::new();
    for name in orphans {
        let count = state.entry(name.clone()).or_insert(0);
        *count += 1;
        if *count >= ORPHAN_CONFIRM {
            to_kill.push(name.clone());
        }
    }
    // A window about to be killed won't reappear next sweep to have its counter cleared by the
    // `retain` above; drop it now so a future window that reuses the same name starts at zero.
    state.retain(|name, _| !to_kill.iter().any(|k| k == name));

    (state, to_kill)
}

/// Kill a single managed tmux window and synchronously reconcile the daemon-side caches that
/// remember it, so the *very next* overlay read reports it gone.
///
/// Left alone, a killed window's disappearance is normally caught within one overlay tick by
/// `rpc::resolve_windows`'s `EMPTY_WINDOWS_CONFIRM` debounce — deliberately slow, because that
/// debounce exists to ride out a *transient* tmux-server bounce (a fork/connection fault, or the
/// user running `tmux kill-server`) rather than trust a sudden total-empty probe as every agent
/// exiting at once. But the debounce can't tell a real bounce apart from a kill *we* just
/// performed on purpose, so it holds the stale (now-dead) window in `last_good_windows` for one
/// extra tick — long enough for an immediately-following `lane.get` (e.g. `delete_lane`'s impact
/// summary) to read the just-stopped agent back as still live.
/// Since we already know this exact window is gone — we're the one who killed it — drop it from
/// the caches proactively instead of waiting for the next probe to (eventually) notice:
/// - `last_good_windows`, so `resolve_windows` can't mistake our kill for a bounce and reuse it.
/// - `prompt_cache`, so a future window reusing this name never inherits a stale pane sniff.
/// - `live_cwds` and `cwds_sticky`, so process-accounting caches drop immediately on stop.
///
/// Shared by the orphan sweep below (killing a stale `lane-<id>` window left by a renumbered
/// worktree) and `rpc::agent.stop` (killing a live one on request) — same window-death
/// bookkeeping either way, so a stopped agent's session can never be read back as still live or
/// reappear as an external ghost session.
pub(crate) async fn kill_and_forget(ctx: &Ctx, window: &str) {
    let tmux = ctx.backend.clone();
    let w = window.to_string();
    let _ = tokio::task::spawn_blocking(move || tmux.kill_named(&w)).await;
    ctx.last_good_windows
        .lock()
        .await
        .retain(|w| w.name != window);
    ctx.last_managed_windows.lock().await.remove(window);
    ctx.prompt_cache.lock().await.remove(window);
    *ctx.live_cwds.lock().await = None;
    ctx.cwds_sticky.lock().await.clear();
    ctx.invalidate_overlay().await;
}

/// Find this sweep's orphaned `lane-<id>` windows, fold them into the cross-sweep confirmation
/// state in [`Ctx::orphan_confirm`], and kill only the ones that have now been seen as orphaned
/// on [`ORPHAN_CONFIRM`] consecutive sweeps — then drop the overlay cache so the phantom sessions
/// they were propping up disappear on the next `lane.list`.
pub async fn reap_orphan_windows(ctx: &Ctx) {
    let Ok(lanes) = ctx.lanes.list().await else {
        return;
    };
    let lane_paths: HashMap<LaneId, PathBuf> = lanes
        .iter()
        .map(|l| (l.id, canonical(&l.worktree.path)))
        .collect();

    let tmux = ctx.backend.clone();
    let raw = match tokio::task::spawn_blocking(move || tmux.list_windows_with_activity()).await {
        Ok(Ok(w)) => w,
        _ => return,
    };
    let now = chrono::Utc::now().timestamp();
    let windows: Vec<Win> = raw
        .into_iter()
        .map(|w| Win {
            name: w.name,
            cwd: canonical(&w.cwd),
            active: now.saturating_sub(w.last_activity) < RUNNING_GRACE.as_secs() as i64,
        })
        .collect();

    // Nothing to own or reap on a server with no managed windows — UNLESS this daemon run has
    // already seen managed windows before, in which case a zero-window server with live lanes
    // still in the store isn't "nothing running", it's every one of those lanes' agents having
    // vanished at once. That's never a normal reap outcome (a real orphan sweep kills windows one
    // at a time and always leaves the rest); it means the tmux server backing them died outright
    // — see the doc comment on `Ctx::session_loss_confirm`. A daemon that just started and hasn't
    // spawned anything yet also has zero windows; `saw_managed_windows` is what tells the two
    // apart, since a fresh boot is the healthy case, not a loss.
    if windows.is_empty() {
        if *ctx.saw_managed_windows.lock().await {
            note_possible_session_loss(ctx, lane_paths.len()).await;
        }
        return;
    }
    *ctx.saw_managed_windows.lock().await = true;
    *ctx.session_loss_confirm.lock().await = 0;

    // Single-owner guard: claim/verify ownership of this tmux server every sweep — PROACTIVELY, so
    // the live daemon stamps the server well before any stray could, not only once it has orphans —
    // and never reap on a server another daemon owns. A second repomond sharing this session (e.g. a
    // stray test instance that kept the default `tmux_session` while pointing at its own store)
    // would otherwise mark every real `lane-<id>` window an orphan and kill it (the disappearing-
    // sessions bug). The owner token is this daemon's db path: stable across restarts (so the real
    // daemon reclaims its own stamp) and distinct per instance (so a stray never matches).
    let me = owner_token(ctx);
    let tmux_g = ctx.backend.clone();
    let me_g = me.clone();
    let owns = tokio::task::spawn_blocking(move || tmux_g.claim_or_verify_owner(&me_g))
        .await
        .map(|state| state == OwnerState::Owned)
        .unwrap_or(false);

    let orphans = orphan_lane_windows(&windows, &lane_paths);

    // Advance the reaper's cross-sweep confirmation state every sweep — including when `orphans`
    // is empty right now, which is exactly what clears a stale count immediately for a window
    // that no longer looks orphaned, rather than leaving it to expire on its own. See
    // `confirm_orphans` for why a window must be seen as orphaned on two consecutive sweeps
    // before it's ever handed to `kill_and_forget`.
    let mut confirm = ctx.orphan_confirm.lock().await;
    let (new_state, to_kill) = confirm_orphans(std::mem::take(&mut *confirm), &orphans);
    *confirm = new_state;
    drop(confirm);

    if to_kill.is_empty() {
        return;
    }
    if !owns {
        tracing::warn!(
            ?to_kill,
            owner = %me,
            session = %ctx.backend.label(),
            "another repomond owns this tmux server; skipping reap (would kill its windows)"
        );
        return;
    }

    tracing::info!(?to_kill, "reaping orphaned agent windows");

    for w in &to_kill {
        kill_and_forget(ctx, w).await;
    }
}

/// This daemon's identity for the tmux-server single-owner guard: its db path — stable across
/// restarts (so the real daemon reclaims its own stamp) and distinct per instance (so a stray
/// test daemon's path never matches). Falls back to the pid when storeless (embedded / tests).
fn owner_token(ctx: &Ctx) -> String {
    ctx.db_path
        .as_ref()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| format!("pid:{}", std::process::id()))
}

/// Periodic reaper task; the first sweep runs immediately (covers daemon startup).
pub async fn reap_watcher(ctx: Arc<Ctx>) {
    let mut tick = tokio::time::interval(TICK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tick.tick().await;
        reap_orphan_windows(&ctx).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    /// An idle managed window (the common reaper input).
    fn idle(name: &str, cwd: &str) -> Win {
        Win {
            name: name.to_string(),
            cwd: p(cwd),
            active: false,
        }
    }

    #[test]
    fn flags_renumbered_and_unknown_lane_windows() {
        // The real bug: the tmux server outlives store generations, so a repo (here at /aaa, now
        // lane 35) ends up with leftover windows named for ids it used to have — lane-81 (now no
        // lane at all) and lane-42 (that id has since been reused for a different worktree, /sxx).
        // Both are orphans; only windows whose id+cwd match a current lane are kept.
        let lane_paths: HashMap<LaneId, PathBuf> =
            [(1, p("/repo")), (35, p("/aaa")), (42, p("/sxx"))]
                .into_iter()
                .collect();

        let windows = vec![
            idle("lane-81", "/aaa"),               // id 81: no such lane -> orphan
            idle("lane-81-2", "/aaa"),             // orphan
            idle("lane-42", "/aaa"), // id 42 is now /sxx, not /aaa -> cwd mismatch -> orphan
            idle("lane-1", "/repo"), // matches lane 1 -> keep
            idle("lane-35", "/aaa"), // matches lane 35 -> keep
            idle("term-1", "/anywhere"), // not a lane window -> ignored
            idle("usage-probe-work", "/anywhere"), // not a lane window -> ignored
        ];

        assert_eq!(
            orphan_lane_windows(&windows, &lane_paths),
            vec!["lane-81", "lane-81-2", "lane-42"]
        );
    }

    #[test]
    fn keeps_everything_when_all_windows_match() {
        let lane_paths: HashMap<LaneId, PathBuf> =
            [(1, p("/repo")), (2, p("/other"))].into_iter().collect();
        let windows = vec![
            idle("lane-1", "/repo"),
            idle("lane-1-2", "/repo"),
            idle("lane-2", "/other"),
        ];
        assert!(orphan_lane_windows(&windows, &lane_paths).is_empty());
    }

    #[test]
    fn spares_orphans_with_a_running_agent() {
        // An orphan whose agent is actively producing output is left alone (the user's
        // "don't reap a running agent" rule); the idle orphan beside it is still reaped.
        let lane_paths: HashMap<LaneId, PathBuf> = HashMap::new(); // no current lanes -> all orphan
        let windows = vec![
            Win {
                name: "lane-2".to_string(),
                cwd: p("/Users/x/Developer/Work/SAAS"),
                active: true,
            },
            idle("lane-13", "/Users/x/Developer/Aven/flick"),
        ];
        assert_eq!(orphan_lane_windows(&windows, &lane_paths), vec!["lane-13"]);
    }

    #[test]
    fn orphaned_once_is_not_killed_yet() {
        // A single sweep's orphan reading must not be trusted outright — see `confirm_orphans`.
        let (state, to_kill) = confirm_orphans(HashMap::new(), &["lane-81".to_string()]);
        assert!(to_kill.is_empty());
        assert_eq!(state.get("lane-81"), Some(&1));
    }

    #[test]
    fn orphaned_on_two_consecutive_sweeps_is_killed() {
        let (state, to_kill) = confirm_orphans(HashMap::new(), &["lane-81".to_string()]);
        assert!(to_kill.is_empty());
        let (state, to_kill) = confirm_orphans(state, &["lane-81".to_string()]);
        assert_eq!(to_kill, vec!["lane-81".to_string()]);
        // Killed windows drop out of the tracked state so a reused name starts at zero.
        assert!(!state.contains_key("lane-81"));
    }

    #[test]
    fn a_clean_sweep_between_two_orphan_sightings_resets_confirmation() {
        // Orphaned once, then NOT orphaned on the next sweep (lane reappeared with a matching
        // cwd, or the window went active), then orphaned again: this must restart confirmation
        // from zero rather than treat the two non-consecutive sightings as back-to-back — the
        // same reset-on-good-read property `resolve_windows` has for `EMPTY_WINDOWS_CONFIRM`.
        let (state, to_kill) = confirm_orphans(HashMap::new(), &["lane-81".to_string()]);
        assert!(to_kill.is_empty());
        assert_eq!(state.get("lane-81"), Some(&1));

        // Looked fine this sweep -> counter cleared immediately, not merely left to expire.
        let (state, to_kill) = confirm_orphans(state, &[]);
        assert!(to_kill.is_empty());
        assert!(!state.contains_key("lane-81"));

        // Orphaned again: this is sighting #1 of a fresh streak, so still not killed.
        let (state, to_kill) = confirm_orphans(state, &["lane-81".to_string()]);
        assert!(to_kill.is_empty());
        assert_eq!(state.get("lane-81"), Some(&1));
    }

    #[test]
    fn unrelated_windows_confirm_independently() {
        // Two windows orphaned on the same sweep confirm on their own schedules; one going clean
        // doesn't disturb the other's count.
        let (state, to_kill) = confirm_orphans(
            HashMap::new(),
            &["lane-1".to_string(), "lane-2".to_string()],
        );
        assert!(to_kill.is_empty());

        let (state, to_kill) = confirm_orphans(state, &["lane-2".to_string()]);
        assert_eq!(to_kill, vec!["lane-2".to_string()]);
        assert!(!state.contains_key("lane-1")); // lane-1 looked fine -> cleared
        assert!(!state.contains_key("lane-2")); // lane-2 was killed -> cleared
    }

    #[test]
    fn session_loss_does_not_fire_on_a_single_bad_sweep() {
        let (count, fire) = advance_session_loss(0, true);
        assert!(!fire);
        assert_eq!(count, 1);
    }

    #[test]
    fn session_loss_fires_on_the_second_consecutive_sweep() {
        let (count, fire) = advance_session_loss(0, true);
        assert!(!fire);
        let (count, fire) = advance_session_loss(count, true);
        assert!(fire);
        assert_eq!(count, SESSION_LOSS_CONFIRM);
    }

    #[test]
    fn session_loss_fires_only_once_per_incident() {
        // A sweep that already crossed the threshold keeps counting, but must not re-fire —
        // otherwise a still-empty server would alert the user again every 60s forever.
        let (count, fire) = advance_session_loss(SESSION_LOSS_CONFIRM, true);
        assert!(!fire);
        assert_eq!(count, SESSION_LOSS_CONFIRM + 1);
    }

    #[test]
    fn session_loss_resets_the_moment_windows_reappear() {
        let (count, _) = advance_session_loss(0, true);
        assert_eq!(count, 1);
        let (count, fire) = advance_session_loss(count, false);
        assert!(!fire);
        assert_eq!(count, 0);
    }

    #[test]
    fn session_loss_never_fires_with_no_lanes_to_lose() {
        // An empty server with an empty store is the healthy steady state (first boot, or every
        // lane deliberately deleted) — not an incident.
        let (count, fire) = advance_session_loss(0, false);
        assert!(!fire);
        assert_eq!(count, 0);
    }
}
