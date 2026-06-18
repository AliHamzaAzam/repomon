//! Probing Claude's `/usage` screen for account-level usage, for the TUI's corner indicator.
//!
//! Subscription usage (the 5-hour and weekly windows) has no CLI flag, local file, or supported
//! endpoint — the only source is the interactive `/usage` command. So, per Claude account, this
//! watcher spawns a hidden throwaway `claude` window, drives it to the prompt (accepting the
//! one-time folder-trust prompt), sends `/usage`, captures the pane, parses it
//! ([`repomon_core::agent::parse_usage`]), dismisses, and kills the window. The result lands in
//! [`Ctx::usage`] for the `usage.get` RPC.
//!
//! It is deliberately frugal and opt-in: it does nothing unless `[usage_probe]` is enabled AND a
//! local TUI is attached, re-probes only every few minutes, and never sends a model prompt (just
//! `/usage` + Esc). The probe window is named `usage-probe-…` (not `lane-…`) and runs in a neutral
//! cwd, so it never pollutes repomon's own lane/agent detection. The parsing it relies on is the
//! pure, fixture-tested part; this module is the IO around it. See `docs/agents.md`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use repomon_core::agent::{self, parse_usage, UsageReport};
use repomon_core::TmuxRuntime;

use crate::Ctx;

/// How often the watcher wakes to consider a probe. Cheap (just flag checks) unless a probe is
/// actually due, so this is short enough to start probing soon after a TUI attaches.
const TICK: Duration = Duration::from_secs(20);
/// How long a usage reading stays fresh before the next probe round. Usage moves slowly and each
/// round spawns a hidden `claude` per account, so this is generous.
const REFRESH: Duration = Duration::from_secs(300);
/// How long since the local TUI's last request before we treat it as gone and stop probing (we
/// keep the last reading so reopening shows it instantly). The TUI polls ~1s; minutes of silence
/// means nobody's looking, so there's no point spawning hidden sessions.
const LOCAL_TTL: Duration = Duration::from_secs(60);

/// One account's last usage reading, with its display label and when it was captured (for an
/// "age" the client can show).
#[derive(Debug, Clone)]
pub struct UsageEntry {
    pub report: UsageReport,
    pub label: String,
    pub fetched_at: Instant,
}

pub async fn usage_watcher(ctx: Arc<Ctx>) {
    let mut tick = tokio::time::interval(TICK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut last_round: Option<Instant> = None;

    loop {
        tick.tick().await;

        if !ctx.config.read().await.usage_probe {
            // Disabled: forget any readings and re-arm so re-enabling probes promptly.
            ctx.usage.lock().await.clear();
            last_round = None;
            continue;
        }
        // Only probe while a TUI is watching — keep the last reading otherwise (don't wipe it),
        // so reopening the TUI shows data at once without spawning sessions in the meantime.
        let tui_active =
            (*ctx.local_watcher_seen.lock().await).is_some_and(|t| t.elapsed() < LOCAL_TTL);
        if !tui_active {
            continue;
        }
        if last_round.is_some_and(|t| t.elapsed() < REFRESH) {
            continue;
        }

        let accounts = accounts();
        let live: HashSet<String> = accounts.iter().map(|a| a.key.clone()).collect();
        for acct in accounts {
            let tmux = ctx.tmux.clone();
            let window = probe_window(&acct.label);
            let cwd = probe_cwd();
            let command = acct.command.clone();
            let report =
                tokio::task::spawn_blocking(move || probe_once(&tmux, &window, &cwd, &command))
                    .await
                    .ok()
                    .flatten();
            if let Some(report) = report {
                ctx.usage.lock().await.insert(
                    acct.key,
                    UsageEntry {
                        report,
                        label: acct.label,
                        fetched_at: Instant::now(),
                    },
                );
            }
        }
        // Drop readings for accounts that no longer exist.
        ctx.usage.lock().await.retain(|k, _| live.contains(k));
        last_round = Some(Instant::now());
    }
}

/// A Claude account to probe: its stable key (matches `AgentSession::config_dir`), a short label
/// (for the probe window name + display), and the launch command (carrying `CLAUDE_CONFIG_DIR`).
struct Account {
    key: String,
    label: String,
    command: String,
}

/// Enumerate the Claude accounts worth probing — those whose config dir has actually been used
/// (`projects/` exists), so a never-run account doesn't trap the probe on first-run onboarding.
fn accounts() -> Vec<Account> {
    let default = agent::claude::default_config_base();
    agent::claude::config_bases()
        .into_iter()
        .filter(|base| base.join("projects").is_dir())
        .map(|base| {
            let cfg_dir = (base != default).then(|| base.clone());
            let command = match &cfg_dir {
                None => "claude".to_string(),
                Some(p) => format!("CLAUDE_CONFIG_DIR={} claude", p.display()),
            };
            Account {
                key: agent::claude::account_key(cfg_dir.as_deref()),
                label: agent::claude::account_label(cfg_dir.as_deref()),
                command,
            }
        })
        .collect()
}

/// The probe's working dir: the home directory (the parent of `~/.claude`). Neutral and outside
/// any registered repo, so the probe never inflates a lane's agent count; typically already
/// trusted, and the trust prompt is accepted once on first run anyway.
fn probe_cwd() -> PathBuf {
    agent::claude::default_config_base()
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// The hidden probe window name for an account. Non-`lane-`/`term-` so the lane scans skip it.
fn probe_window(label: &str) -> String {
    let safe: String = label
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    format!("usage-probe-{safe}")
}

/// Spawn a hidden `claude`, drive it to a ready prompt, run `/usage`, parse the pane, then dismiss
/// and kill the window. Blocking (tmux IO + waits) — call from `spawn_blocking`. Returns `None` on
/// any failure (the caller keeps the previous reading).
fn probe_once(tmux: &TmuxRuntime, window: &str, cwd: &Path, command: &str) -> Option<UsageReport> {
    use std::thread::sleep;

    // Clear a window left over from a crashed previous round, then launch our own (detached, so an
    // attached client never gets yanked to it).
    let _ = tmux.kill_named(window);
    tmux.spawn_named(window, cwd, command).ok()?;

    // Drive to a ready REPL, accepting the one-time folder-trust prompt if it appears.
    let mut ready = false;
    for _ in 0..40 {
        sleep(Duration::from_millis(500));
        let pane = tmux.capture_named(window, None).unwrap_or_default();
        match probe_state(&pane) {
            ProbeState::Ready => {
                ready = true;
                break;
            }
            ProbeState::Trust => {
                let _ = tmux.send_key_named(window, "Enter"); // "Yes, I trust this folder"
            }
            ProbeState::NotYet => {}
        }
    }

    let mut report = None;
    if ready {
        // Type `/usage`, give the slash palette a beat to register, then Enter to run it.
        let _ = tmux.send_literal_named(window, "/usage");
        sleep(Duration::from_millis(600));
        let _ = tmux.send_key_named(window, "Enter");
        for _ in 0..12 {
            sleep(Duration::from_millis(400));
            let pane = tmux.capture_named(window, None).unwrap_or_default();
            if let Some(r) = parse_usage(&pane) {
                report = Some(r);
                break;
            }
        }
        let _ = tmux.send_key_named(window, "Escape"); // close the usage view
    }

    let _ = tmux.kill_named(window);
    report
}

/// What the probe pane is showing, so the driver knows whether to wait, accept trust, or proceed.
#[derive(Debug, PartialEq, Eq)]
enum ProbeState {
    /// The REPL is up and ready for `/usage`.
    Ready,
    /// The one-time "trust this folder?" prompt — accept it and keep waiting.
    Trust,
    /// Still starting up (or a screen we don't recognize) — keep waiting.
    NotYet,
}

fn probe_state(pane: &str) -> ProbeState {
    let low = pane.to_lowercase();
    if low.contains("trust this folder")
        || low.contains("do you trust")
        || low.contains("project you created or one you trust")
    {
        return ProbeState::Trust;
    }
    if low.contains("? for shortcuts")
        || low.contains("welcome back")
        || low.contains("welcome to claude")
        || low.contains("/help for help")
    {
        return ProbeState::Ready;
    }
    ProbeState::NotYet
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_window_is_sanitized_and_non_lane() {
        let w = probe_window("work");
        assert_eq!(w, "usage-probe-work");
        assert!(!w.starts_with("lane-"));
        // Odd labels can't break the tmux target.
        assert_eq!(probe_window("a.b/c"), "usage-probe-a-b-c");
    }

    #[test]
    fn probe_state_classifies_screens() {
        assert_eq!(
            probe_state("Quick safety check: Is this a project you created or one you trust?"),
            ProbeState::Trust
        );
        assert_eq!(
            probe_state("Welcome back Star Solutions!\n ? for shortcuts"),
            ProbeState::Ready
        );
        assert_eq!(probe_state("\n\n   loading…"), ProbeState::NotYet);
    }

    /// Full end-to-end probe against a real `claude` on the default account, in an isolated tmux
    /// server (never the user's `repomon` session). Ignored by default — it spawns a real session
    /// (a little quota + a tiny transcript) and takes ~10–20s. Run manually:
    ///   cargo test -p repomon-daemon probe_once_reads_real_usage -- --ignored --nocapture
    #[test]
    #[ignore = "spawns a real `claude` and runs /usage; run manually with --ignored"]
    fn probe_once_reads_real_usage() {
        let tmux = TmuxRuntime::new("repomon-usagetest-probe");
        let report = probe_once(&tmux, "usage-probe-test", &probe_cwd(), "claude");
        // Tear down the isolated tmux server regardless of outcome.
        let _ = std::process::Command::new("tmux")
            .args(["-L", "repomon-usagetest-probe", "kill-server"])
            .output();
        let r = report.expect("probe should scrape and parse /usage");
        eprintln!("scraped usage: {r:?}");
        assert!(
            r.session_pct.is_some() || r.week_pct.is_some(),
            "expected at least one usage percentage"
        );
    }
}
