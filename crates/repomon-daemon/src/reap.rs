//! Orphaned agent-window reaper.
//!
//! repomon's tmux server (`tmux -L <session>`) is long-lived — it survives daemon restarts and
//! even a wiped store. When the store is reset or a worktree is re-registered, lane ids get
//! reassigned, but the `lane-<id>` windows spawned under the old ids keep running. Those windows
//! (and their idle `claude` processes, which never exit on their own) become unreachable garbage:
//! they no longer map to the worktree their name claims, yet their cwd still inflates the
//! path-keyed live-process count in `overlay_agents`, surfacing phantom "external" sessions the
//! user can't dismiss. This module finds and kills them.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use repomon_core::agent::tmux::TmuxRuntime;
use repomon_core::model::LaneId;

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

/// Names of stale `lane-<id>` agent windows to reap: the id maps to no current lane, or the
/// worktree for that id no longer lives at the window's pane cwd. Both mean the window is a
/// leftover from a re-registered / renumbered worktree (e.g. the tmux server outliving a store
/// reset) — a managed `claude` is spawned with `-c <worktree>` and never chdirs, so a cwd
/// mismatch is proof the window belongs to a defunct generation. An **active** window is never
/// reaped, even when orphaned, so a still-running agent is left alone. Non-lane windows
/// (terminals, the usage probe) are ignored.
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
///
/// Since we already know this exact window is gone — we're the one who killed it — drop it from
/// the caches proactively instead of waiting for the next probe to (eventually) notice:
/// - `last_good_windows`, so `resolve_windows` can't mistake our kill for a bounce and reuse it.
/// - `prompt_cache`, so a future window reusing this name never inherits a stale pane sniff.
///
/// `last_managed_windows` is deliberately left untouched: `overlay_agents`'s next-tick diff
/// against it is what detects "a managed window vanished" and drops the stale
/// `live_cwds`/`cwds_sticky` process counts — pre-updating it here would erase that signal.
///
/// Shared by the orphan sweep below (killing a stale `lane-<id>` window left by a renumbered
/// worktree) and `rpc::agent.stop` (killing a live one on request) — same window-death
/// bookkeeping either way, so a stopped agent's session can never be read back as still live.
pub(crate) async fn kill_and_forget(ctx: &Ctx, window: &str) {
    let tmux = ctx.tmux.clone();
    let w = window.to_string();
    let _ = tokio::task::spawn_blocking(move || tmux.kill_named(&w)).await;
    ctx.last_good_windows.lock().await.retain(|w| w != window);
    ctx.prompt_cache.lock().await.remove(window);
    ctx.invalidate_overlay().await;
}

/// Find and kill orphaned `lane-<id>` windows once, then drop the overlay cache so the phantom
/// sessions they were propping up disappear on the next `lane.list`.
pub async fn reap_orphan_windows(ctx: &Ctx) {
    let Ok(lanes) = ctx.lanes.list().await else {
        return;
    };
    let lane_paths: HashMap<LaneId, PathBuf> = lanes
        .iter()
        .map(|l| (l.id, canonical(&l.worktree.path)))
        .collect();

    let tmux = ctx.tmux.clone();
    let raw = match tokio::task::spawn_blocking(move || tmux.list_windows_with_activity()).await {
        Ok(Ok(w)) => w,
        _ => return,
    };
    let now = chrono::Utc::now().timestamp();
    let windows: Vec<Win> = raw
        .into_iter()
        .map(|(name, cwd, activity)| Win {
            name,
            cwd: canonical(&cwd),
            active: now.saturating_sub(activity) < RUNNING_GRACE.as_secs() as i64,
        })
        .collect();

    // Nothing to own or reap on a server with no managed windows.
    if windows.is_empty() {
        return;
    }

    // Single-owner guard: claim/verify ownership of this tmux server every sweep — PROACTIVELY, so
    // the live daemon stamps the server well before any stray could, not only once it has orphans —
    // and never reap on a server another daemon owns. A second repomond sharing this session (e.g. a
    // stray test instance that kept the default `tmux_session` while pointing at its own store)
    // would otherwise mark every real `lane-<id>` window an orphan and kill it (the disappearing-
    // sessions bug). The owner token is this daemon's db path: stable across restarts (so the real
    // daemon reclaims its own stamp) and distinct per instance (so a stray never matches).
    let me = owner_token(ctx);
    let tmux_g = ctx.tmux.clone();
    let me_g = me.clone();
    let owns = tokio::task::spawn_blocking(move || tmux_g.claim_or_verify_owner(&me_g))
        .await
        .unwrap_or(false);

    let orphans = orphan_lane_windows(&windows, &lane_paths);
    if orphans.is_empty() {
        return;
    }
    if !owns {
        tracing::warn!(
            ?orphans,
            owner = %me,
            session = ctx.tmux.session(),
            "another repomond owns this tmux server; skipping reap (would kill its windows)"
        );
        return;
    }

    tracing::info!(?orphans, "reaping orphaned agent windows");

    for w in &orphans {
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
}
