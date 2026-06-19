//! Probing agent usage screens (Claude `/usage`, Codex `/status`) for the TUI's corner indicator.
//!
//! Subscription usage has no CLI flag, file, or supported endpoint, so per account this watcher
//! spawns a hidden throwaway session, drives it to the prompt (accepting the one-time folder-trust
//! prompt), sends the usage command, captures the pane, parses it
//! ([`repomon_core::agent::parse_usage`] / [`parse_codex_status`]), then kills the window. Results
//! land in [`Ctx::usage`] for the `usage.get` RPC, keyed so the TUI can attribute usage to the
//! focused agent's account (Claude config dir, or `"codex"`).
//!
//! It is opt-in and frugal: nothing runs unless `[usage_probe]` is enabled AND a local TUI is
//! attached; it re-probes only every few minutes and never sends a model prompt (just the usage
//! command + Esc). Probe windows are named `usage-probe-…` (not `lane-…`) and run in a neutral cwd,
//! so they never pollute repomon's own lane/agent detection. The parsing is the pure, fixture-
//! tested part; this module is the IO around it. See `docs/agents.md`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use repomon_core::agent::{self, parse_codex_status, parse_usage, UsageReport};
use repomon_core::TmuxRuntime;

use crate::Ctx;

/// How often the watcher wakes to consider a probe. Cheap (just flag checks) unless a probe is
/// actually due, so this is short enough to start probing soon after a TUI attaches.
const TICK: Duration = Duration::from_secs(20);
/// How long a usage reading stays fresh before the next probe round. Usage moves slowly and each
/// round spawns a hidden session per account, so this is generous.
const REFRESH: Duration = Duration::from_secs(300);
/// How long since the local TUI's last request before we treat it as gone and stop probing (we
/// keep the last reading so reopening shows it instantly).
const LOCAL_TTL: Duration = Duration::from_secs(60);
/// Hard ceiling on a single probe. One normally finishes in well under 35s; if a `tmux` call ever
/// hangs (a wedged tmux server, an agent that never reaches its prompt), abandon the probe rather
/// than let it `await` forever — otherwise one stuck probe freezes the whole watcher and every
/// account's usage goes stale (the bug this guards against).
const PROBE_TIMEOUT: Duration = Duration::from_secs(75);

/// One account's last usage reading, with its display label and when it was captured.
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
            ctx.usage.lock().await.clear();
            last_round = None;
            continue;
        }
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
            let spec = acct.spec;
            let probe = tokio::task::spawn_blocking(move || probe_once(&tmux, &window, &cwd, &spec));
            let report = match tokio::time::timeout(PROBE_TIMEOUT, probe).await {
                Ok(join) => join.ok().flatten(),
                Err(_) => {
                    // Probe hung (a tmux call that never returned). Abandon it — keep this
                    // account's last reading and carry on, so one stuck probe can't wedge the
                    // watcher and freeze every account. (The orphaned blocking thread is left to
                    // finish on its own; the next round's probe_once kills any leftover window.)
                    tracing::warn!("usage probe for {} timed out; skipping this round", acct.key);
                    None
                }
            };
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
        ctx.usage.lock().await.retain(|k, _| live.contains(k));
        last_round = Some(Instant::now());
    }
}

/// A usage-bearing account to probe: its stable key (matches the focused agent's attribution), a
/// short display label, and how to probe it.
struct Account {
    key: String,
    label: String,
    spec: ProbeSpec,
}

/// How to probe one agent: the launch command, the usage slash-command, the parser, and the pane
/// markers that say the REPL is ready or sitting on a folder-trust prompt.
struct ProbeSpec {
    command: String,
    slash: &'static str,
    parse: fn(&str) -> Option<UsageReport>,
    ready: &'static [&'static str],
    trust: &'static [&'static str],
}

fn claude_spec(command: String) -> ProbeSpec {
    ProbeSpec {
        command,
        slash: "/usage",
        parse: parse_usage,
        ready: &[
            "? for shortcuts",
            "welcome back",
            "welcome to claude",
            "/help for help",
        ],
        trust: &[
            "trust this folder",
            "do you trust",
            "project you created or one you trust",
        ],
    }
}

fn codex_spec() -> ProbeSpec {
    ProbeSpec {
        command: "codex".to_string(),
        slash: "/status",
        parse: parse_codex_status,
        ready: &["openai codex"],
        trust: &[
            "do you trust the contents",
            "trust the contents of this directory",
        ],
    }
}

/// Enumerate accounts worth probing: each used Claude config dir, plus Codex if it's installed
/// (its config dir exists). A never-run Claude account is skipped so first-run onboarding can't
/// trap the probe.
fn accounts() -> Vec<Account> {
    let default = agent::claude::default_config_base();
    let mut out: Vec<Account> = agent::claude::config_bases()
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
                spec: claude_spec(command),
            }
        })
        .collect();
    if probe_cwd().join(".codex").is_dir() {
        out.push(Account {
            key: "codex".to_string(),
            label: "codex".to_string(),
            spec: codex_spec(),
        });
    }
    out
}

/// The probe's working dir: the home directory. Neutral and outside any registered repo, so the
/// probe never inflates a lane's agent count; typically already trusted, and the trust prompt is
/// accepted once on first run anyway.
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

/// Spawn a hidden session, drive it to a ready prompt, run the usage command, parse the pane, then
/// dismiss and kill the window. Blocking (tmux IO + waits) — call from `spawn_blocking`. Returns
/// `None` on any failure (the caller keeps the previous reading).
fn probe_once(tmux: &TmuxRuntime, window: &str, cwd: &Path, spec: &ProbeSpec) -> Option<UsageReport> {
    use std::thread::sleep;

    let _ = tmux.kill_named(window);
    tmux.spawn_named(window, cwd, &spec.command).ok()?;

    let mut ready = false;
    for _ in 0..40 {
        sleep(Duration::from_millis(500));
        let pane = tmux.capture_named(window, None).unwrap_or_default();
        match probe_state(&pane, spec) {
            ProbeState::Ready => {
                ready = true;
                break;
            }
            ProbeState::Trust => {
                let _ = tmux.send_key_named(window, "Enter"); // accept "trust this folder/contents"
            }
            ProbeState::NotYet => {}
        }
    }

    let mut report = None;
    if ready {
        // The banner can show before the composer accepts input (notably Codex), so settle first.
        sleep(Duration::from_millis(1200));
        // Re-send the slash-command up to a few times: a typed-too-early send (composer not ready)
        // or a slow render shouldn't lose the round. Re-sending is idempotent — the parse succeeds
        // as soon as the screen is up. Claude renders on the first try, so it never retries.
        'attempts: for _ in 0..3 {
            let _ = tmux.send_literal_named(window, spec.slash);
            sleep(Duration::from_millis(700));
            let _ = tmux.send_key_named(window, "Enter");
            for _ in 0..8 {
                sleep(Duration::from_millis(450));
                let pane = tmux.capture_named(window, None).unwrap_or_default();
                if let Some(r) = (spec.parse)(&pane) {
                    report = Some(r);
                    break 'attempts;
                }
            }
        }
        let _ = tmux.send_key_named(window, "Escape");
    }

    let _ = tmux.kill_named(window);
    report
}

/// What the probe pane is showing, so the driver knows whether to wait, accept trust, or proceed.
#[derive(Debug, PartialEq, Eq)]
enum ProbeState {
    Ready,
    Trust,
    NotYet,
}

fn probe_state(pane: &str, spec: &ProbeSpec) -> ProbeState {
    let low = pane.to_lowercase();
    if spec.trust.iter().any(|m| low.contains(m)) {
        return ProbeState::Trust;
    }
    if spec.ready.iter().any(|m| low.contains(m)) {
        return ProbeState::Ready;
    }
    ProbeState::NotYet
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_window_is_sanitized_and_non_lane() {
        assert_eq!(probe_window("work"), "usage-probe-work");
        assert_eq!(probe_window("codex"), "usage-probe-codex");
        assert!(!probe_window("work").starts_with("lane-"));
        assert_eq!(probe_window("a.b/c"), "usage-probe-a-b-c");
    }

    #[test]
    fn probe_state_classifies_screens() {
        let claude = claude_spec("claude".to_string());
        assert_eq!(
            probe_state("Is this a project you created or one you trust?", &claude),
            ProbeState::Trust
        );
        assert_eq!(
            probe_state("Welcome back!\n ? for shortcuts", &claude),
            ProbeState::Ready
        );
        assert_eq!(probe_state("\n\n   loading…", &claude), ProbeState::NotYet);

        let codex = codex_spec();
        assert_eq!(
            probe_state("Do you trust the contents of this directory?", &codex),
            ProbeState::Trust
        );
        assert_eq!(
            probe_state(">_ OpenAI Codex (v0.141.0)", &codex),
            ProbeState::Ready
        );
    }

    /// Full end-to-end probe against a real `claude` on the default account, in an isolated tmux
    /// server. Ignored by default — spawns a real session (a little quota + a tiny transcript).
    ///   cargo test -p repomon-daemon probe_once_reads_real_claude -- --ignored --nocapture
    #[test]
    #[ignore = "spawns a real `claude` and runs /usage; run manually with --ignored"]
    fn probe_once_reads_real_claude() {
        let tmux = TmuxRuntime::new("repomon-usagetest-claude");
        let report = probe_once(
            &tmux,
            "usage-probe-test",
            &probe_cwd(),
            &claude_spec("claude".to_string()),
        );
        let _ = std::process::Command::new("tmux")
            .args(["-L", "repomon-usagetest-claude", "kill-server"])
            .output();
        let r = report.expect("probe should scrape and parse /usage");
        eprintln!("claude windows: {:?}", r.windows);
        assert!(!r.windows.is_empty());
    }

    /// Same, against a real `codex` /status. Ignored by default.
    ///   cargo test -p repomon-daemon probe_once_reads_real_codex -- --ignored --nocapture
    #[test]
    #[ignore = "spawns a real `codex` and runs /status; run manually with --ignored"]
    fn probe_once_reads_real_codex() {
        let tmux = TmuxRuntime::new("repomon-usagetest-codex");
        let report = probe_once(&tmux, "usage-probe-codex-test", &probe_cwd(), &codex_spec());
        let _ = std::process::Command::new("tmux")
            .args(["-L", "repomon-usagetest-codex", "kill-server"])
            .output();
        let r = report.expect("probe should scrape and parse /status");
        eprintln!("codex windows: {:?}", r.windows);
        assert!(!r.windows.is_empty());
    }
}
