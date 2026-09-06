//! Reaps managed windows whose lane or working directory no longer matches registration, and
//! detects loss of the backend session.

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

/// Allow output silence long enough to protect brief pauses in a running agent.
const RUNNING_GRACE: Duration = Duration::from_secs(300);

/// Identify inactive managed windows missing a lane or matching worktree; this is only a candidate
/// set, and the caller must require consecutive sightings before destructive teardown.
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

/// Require consecutive orphan observations so a single bad probe cannot kill a live window.
const ORPHAN_CONFIRM: u32 = 2;

/// Alert once when consecutive sweeps confirm live lanes with no backend windows; reset when
/// windows reappear.
const SESSION_LOSS_CONFIRM: u32 = 2;

/// Advance the consecutive-loss counter, resetting on a healthy sweep and firing only when the
/// threshold is first reached.
fn advance_session_loss(count: u32, lanes_but_no_windows: bool) -> (u32, bool) {
    if !lanes_but_no_windows {
        return (0, false);
    }
    let count = count + 1;
    let fire = count == SESSION_LOSS_CONFIRM;
    (count, fire)
}

/// Debounce and report possible total-session loss when lanes remain but the backend has no
/// windows.
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

/// Confirm only consecutive orphan sightings; any healthy or active observation immediately clears
/// that window’s count.
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

/// Stops one managed window and clears its liveness, prompt, and process caches immediately so
/// deliberate teardown cannot be mistaken for a transient probe failure.
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

/// Confirms orphan candidates, kills their windows, and invalidates liveness caches.
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

    // Only treat an empty backend as loss after this daemon has observed managed windows; a fresh
    // unpopulated boot is healthy.
    if windows.is_empty() {
        if *ctx.saw_managed_windows.lock().await {
            note_possible_session_loss(ctx, lane_paths.len()).await;
        }
        return;
    }
    *ctx.saw_managed_windows.lock().await = true;
    *ctx.session_loss_confirm.lock().await = 0;

    // Verify database-based ownership before every destructive sweep so another daemon cannot reap
    // this server’s windows.
    let me = owner_token(ctx);
    let tmux_g = ctx.backend.clone();
    let me_g = me.clone();
    let owns = tokio::task::spawn_blocking(move || tmux_g.claim_or_verify_owner(&me_g))
        .await
        .map(|state| state == OwnerState::Owned)
        .unwrap_or(false);

    let orphans = orphan_lane_windows(&windows, &lane_paths);

    // Update even an empty orphan set so a healthy observation immediately clears prior suspicion.
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

/// This daemon's identity for the tmux-server single-owner guard: its db path - stable across
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
        // Only current lane IDs with matching working directories survive orphan classification.
        let lane_paths: HashMap<LaneId, PathBuf> =
            [(1, p("/repo")), (35, p("/aaa")), (42, p("/sxx"))]
                .into_iter()
                .collect();

        let windows = vec![
            idle("lane-81", "/aaa"),
            idle("lane-81-2", "/aaa"),
            idle("lane-42", "/aaa"),
            idle("lane-1", "/repo"),
            idle("lane-35", "/aaa"),
            idle("term-1", "/anywhere"),
            idle("usage-probe-work", "/anywhere"),
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
        // A single sweep's orphan reading must not be trusted outright - see `confirm_orphans`.
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
        // A healthy observation must reset the consecutive-orphan count.
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
        assert!(!state.contains_key("lane-1"));
        assert!(!state.contains_key("lane-2"));
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
        // A sweep that already crossed the threshold keeps counting, but must not re-fire -
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
        // lane deliberately deleted) - not an incident.
        let (count, fire) = advance_session_loss(0, false);
        assert!(!fire);
        assert_eq!(count, 0);
    }
}
