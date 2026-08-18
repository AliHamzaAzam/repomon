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
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use repomon_core::agent::backend::{CaptureOpts, SpawnSpec};
use repomon_core::agent::{
    self, UsageReport, WindowMeta, parse_antigravity_usage, parse_codex_status, parse_usage,
};
use repomon_core::model::AgentKind;
use repomon_core::{SessionBackend, TmuxRuntime};

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

/// Cooperative cancellation for a blocking probe. A probe runs on a `spawn_blocking` thread that
/// the watcher stops `await`ing once [`PROBE_TIMEOUT`] elapses, but that thread keeps executing —
/// sleeping and sending keys to tmux. Without a way to tell it to stop, those abandoned threads
/// pile up (one per stuck round). [`probe_once`] polls this between its sleep/retry steps and
/// self-aborts once the deadline passes or the watcher flips the flag, so it dies promptly instead
/// of running to completion.
#[derive(Clone)]
struct Cancel {
    deadline: Instant,
    aborted: Arc<AtomicBool>,
}

impl Cancel {
    fn new(timeout: Duration) -> Self {
        Cancel {
            deadline: Instant::now() + timeout,
            aborted: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Tell the probe to stop at its next checkpoint (called when the watcher abandons the await).
    fn abort(&self) {
        self.aborted.store(true, Ordering::Relaxed);
    }

    /// True once the probe should give up: past its deadline or explicitly aborted.
    fn is_cancelled(&self) -> bool {
        self.aborted.load(Ordering::Relaxed) || Instant::now() >= self.deadline
    }
}

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
        // Cache-retention key set: every *installed* account (has a Claude config dir with
        // history, or `~/.codex`/`~/.gemini` present), independent of whether its kind has a
        // live session this round. Gating below only skips the expensive probe IO for inactive
        // kinds - it must never also evict that account's last reading, or the TUI's usage
        // corner would blank the instant an agent of that kind exits instead of just going
        // stale. Locked in by `gating_does_not_evict_cache_for_inactive_kind` below.
        let live: HashSet<String> = accounts.iter().map(|a| a.key.clone()).collect();

        // Only probe an agent kind when the fleet currently has a lane session of that kind -
        // probing spawns a full hidden CLI session per account, so this is the expensive part
        // this gate exists to skip. One `list_windows_meta` call for the whole round, not one
        // per account.
        let backend = ctx.backend.clone();
        let active = match tokio::task::spawn_blocking(move || backend.list_windows_meta()).await {
            Ok(Ok(windows)) => {
                let mut active = active_kinds(&windows);
                // Widen with agent kinds detected running OUTSIDE tmux (a `claude`/`codex`
                // started in the user's own terminal, not one repomon spawned) - those show in
                // the sidebar with usage attribution too, so an active external session should
                // keep its kind's probe alive the same as a managed lane window does. See
                // `external_active_kinds` for why this is a cheap, best-effort cache read
                // rather than a fresh scan.
                active.extend(external_active_kinds(&ctx).await);
                Some(active)
            }
            Ok(Err(e)) => {
                // Fail OPEN: probe every account this round rather than gate on a listing we
                // couldn't get. A transient backend error then costs one round of the old
                // always-probe behavior instead of leaving usage stuck stale indefinitely
                // because we can no longer tell what's active.
                tracing::warn!(
                    "usage watcher: list_windows_meta failed ({e}); probing all accounts this round"
                );
                None
            }
            Err(e) => {
                // The spawn_blocking task itself panicked or was cancelled; same fail-open
                // rationale as the listing error above.
                tracing::warn!(
                    "usage watcher: list_windows_meta task failed ({e}); probing all accounts this round"
                );
                None
            }
        };

        for acct in accounts {
            if let Some(active) = &active {
                if !account_is_active(&acct.key, active) {
                    continue;
                }
            }
            let tmux = ctx.backend.clone();
            let window = probe_window(&acct.label);
            let cwd = probe_cwd();
            let spec = acct.spec;
            let cancel = Cancel::new(PROBE_TIMEOUT);
            let probe_cancel = cancel.clone();
            let probe = tokio::task::spawn_blocking(move || {
                probe_once(&*tmux, &window, &cwd, &spec, &probe_cancel)
            });
            let report = match tokio::time::timeout(PROBE_TIMEOUT, probe).await {
                Ok(join) => join.ok().flatten(),
                Err(_) => {
                    // Probe hung (a tmux call that never returned). Abandon the await and tell the
                    // blocking thread to stop at its next checkpoint, so abandoned probes don't pile
                    // up driving tmux. Keep this account's last reading and carry on.
                    cancel.abort();
                    tracing::warn!(
                        "usage probe for {} timed out; skipping this round",
                        acct.key
                    );
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
        // Retain against `live` (installed accounts), not `active` (this round's gate) - an
        // account whose kind was skipped this round keeps its last reading untouched until it
        // drops out of `accounts()` entirely (e.g. its Claude config dir goes unused, or
        // `~/.codex` is removed).
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
            "claude code",
            "/model",
            "auto mode on",
            "shift+tab to cycle",
            "? for shortcuts",
            "welcome back",
            "welcome to claude",
            "/help for help",
            "try \"",
            "❯",
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
        ready: &["openai codex", "codex", "model:", "for shortcuts"],
        trust: &[
            "do you trust the contents",
            "trust the contents of this directory",
            "trust this folder",
            "do you trust",
        ],
    }
}

fn antigravity_spec() -> ProbeSpec {
    ProbeSpec {
        command: "agy".to_string(),
        slash: "/usage",
        parse: parse_antigravity_usage,
        ready: &[
            "antigravity cli",
            "models & quota",
            "gemini 3.7",
            "gemini 2.5",
            "gemini 3.5",
            "for shortcuts",
            "antigravity",
        ],
        trust: &[
            "trust this folder",
            "do you trust",
            "project you created or one you trust",
        ],
    }
}

/// Enumerate accounts worth probing: each used Claude config dir, plus Codex and Antigravity if
/// installed. A never-run Claude account is skipped so first-run onboarding can't trap the probe.
fn accounts() -> Vec<Account> {
    let default = agent::claude::default_config_base();
    let mut out: Vec<Account> = agent::claude::config_bases()
        .into_iter()
        .filter(|base| base.join("projects").is_dir())
        .map(|base| {
            let cfg_dir = (base != default).then(|| base.clone());
            // `launch_command` is immune to the daemon's own env: the default account unsets
            // CLAUDE_CONFIG_DIR (`env -u …`) while variants pin their dir. A bare `claude` here
            // would instead inherit the daemon's CLAUDE_CONFIG_DIR (it's started from a claude-work
            // shell) and probe the wrong account, making two accounts read as one.
            // `account_key`/`account_label` stay keyed on `cfg_dir`, so identity is intact.
            let command = agent::claude::launch_command(&base);
            Account {
                key: agent::claude::account_key(cfg_dir.as_deref()),
                label: agent::claude::account_label(cfg_dir.as_deref()),
                spec: claude_spec(command),
            }
        })
        .collect();
    let home = probe_cwd();
    if home.join(".codex").is_dir() {
        out.push(Account {
            key: "codex".to_string(),
            label: "codex".to_string(),
            spec: codex_spec(),
        });
    }
    if home.join(".gemini").is_dir() || home.join(".config/gemini").is_dir() {
        out.push(Account {
            key: "antigravity".to_string(),
            label: "antigravity".to_string(),
            spec: antigravity_spec(),
        });
    }
    out
}

/// The agent kinds currently running in the fleet, derived from lane windows only. This is the
/// gate [`account_is_active`] checks against: usage probing is opt-in *and* frugal, so an agent
/// kind with no live session is never worth spawning a hidden CLI session to check.
///
/// Filters to lane windows (`lane-<id>`/`lane-<id>-<slot>`) via [`TmuxRuntime::lane_id_of`], so
/// this module's own `usage-probe-*` windows (and plain `term-*` windows) never count as active
/// sessions. Counting them would make the gate self-sustaining: a probe window's mere existence
/// would justify the next probe, and it would never stop.
///
/// A lane window with no `@repomon_agent_kind` stamp is treated as `claude-code`, the default
/// agent kind - windows created before kind-stamping shipped (or before the overlay's binder
/// reaches them) simply predate the option, and defaulting them to the fallback kind is correct,
/// not merely convenient.
fn active_kinds(windows: &[WindowMeta]) -> HashSet<String> {
    windows
        .iter()
        .filter(|w| TmuxRuntime::lane_id_of(&w.name).is_some())
        .map(|w| {
            // Normalize through `AgentKind` rather than comparing the raw option string, so an
            // alias the window option might carry (e.g. `"agy"`, which `AgentKind::short()` uses
            // for Antigravity) still lands on the same canonical key `account_is_active` checks
            // against. Every stamp this daemon writes today already uses `AgentKind::as_str()`'s
            // canonical form (`agent.spawn`/`agent.adopt` stamp via `kind.as_str()`), so this
            // guards future/legacy stamps rather than something today's data needs.
            match w.agent_kind.as_deref() {
                Some(k) => AgentKind::from_kind_str(k).as_str().into_owned(),
                None => AgentKind::ClaudeCode.as_str().into_owned(),
            }
        })
        .collect()
}

/// Agent kinds with an active EXTERNAL session - one the daemon detected from a transcript but
/// did not spawn (the user ran `claude`/`codex`/`agy` in their own terminal). These show in the
/// sidebar with usage attribution exactly like a managed lane session, so they widen the gate
/// [`active_kinds`] computes from managed tmux windows alone.
///
/// Reads the daemon's existing lane-overlay cache ([`Ctx::overlay_cache`]) instead of
/// recomputing it: re-running the overlay (tmux + transcript IO) just to decide whether to gate
/// a probe would be exactly the expensive mechanism this change exists to avoid. This is
/// opportunistic - the cache can be empty (nothing has called `lane.list` yet since the daemon
/// started) or a little stale - in which case external sessions simply don't widen the gate this
/// round; [`active_kinds`] alone (always freshly probed) still decides. Not separately unit
/// tested: it is a thin cache read with no branching logic of its own, and constructing
/// `AgentSession`/`Lane` fixtures here would pull lane-modeling detail into a module whose job is
/// usage probing, not lane state.
async fn external_active_kinds(ctx: &Ctx) -> HashSet<String> {
    let cache = ctx.overlay_cache.lock().await;
    let Some((_, lanes)) = cache.entry() else {
        return HashSet::new();
    };
    lanes
        .iter()
        .flat_map(|lane| &lane.agent_sessions)
        .filter(|s| s.external && s.ended_at.is_none())
        .map(|s| s.agent.as_str().into_owned())
        .collect()
}

/// Should `key` (an [`Account::key`]: a Claude config-dir path, `"default"`, or the sentinel
/// `"codex"` / `"antigravity"`) be probed this round, given `active` from [`active_kinds`]?
///
/// Claude accounts are gated on *any* `claude-code` lane window existing, not per-account: tmux
/// window metadata says which agent *kind* a window runs, not which Claude config dir backs it,
/// so per-account precision isn't derivable here. That's an accepted approximation - a fleet with
/// only a "work" Claude session open still probes every Claude account, the same cost this kind
/// always paid; the gate only removes that cost when *no* Claude session is open at all.
fn account_is_active(key: &str, active: &HashSet<String>) -> bool {
    match key {
        "codex" => active.contains("codex"),
        "antigravity" => active.contains("antigravity"),
        _ => active.contains("claude-code"),
    }
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
fn probe_once(
    tmux: &dyn SessionBackend,
    window: &str,
    cwd: &Path,
    spec: &ProbeSpec,
    cancel: &Cancel,
) -> Option<UsageReport> {
    use std::thread::sleep;

    let _ = tmux.kill_named(window);
    tmux.spawn_named(window, &SpawnSpec::new(spec.command.clone(), cwd))
        .ok()?;

    let mut ready = false;
    for _ in 0..40 {
        // Bail to the cleanup below if the watcher abandoned us (deadline passed / aborted), so
        // this blocking thread doesn't keep driving tmux after its round was given up on.
        if cancel.is_cancelled() {
            break;
        }
        sleep(Duration::from_millis(500));
        let pane = tmux
            .capture_named(window, CaptureOpts::visible())
            .unwrap_or_default();
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
            if cancel.is_cancelled() {
                break 'attempts;
            }
            let _ = tmux.send_literal_named(window, spec.slash);
            sleep(Duration::from_millis(700));
            let _ = tmux.send_key_named(window, "Enter");
            for _ in 0..8 {
                if cancel.is_cancelled() {
                    break 'attempts;
                }
                sleep(Duration::from_millis(450));
                let pane = tmux
                    .capture_named(window, CaptureOpts::visible())
                    .unwrap_or_default();
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

    /// Build a [`WindowMeta`] for the gating tests below - only `name` and `agent_kind` matter
    /// to [`active_kinds`].
    fn wm(name: &str, agent_kind: Option<&str>) -> WindowMeta {
        WindowMeta {
            name: name.to_string(),
            wid: 0,
            session: None,
            agent_kind: agent_kind.map(str::to_string),
        }
    }

    #[test]
    fn antigravity_probed_when_a_lane_window_runs_it() {
        let windows = [wm("lane-1", Some("antigravity"))];
        let active = active_kinds(&windows);
        assert!(active.contains("antigravity"));
        assert!(account_is_active("antigravity", &active));
    }

    #[test]
    fn antigravity_not_probed_without_a_lane_window() {
        let windows = [wm("lane-1", Some("claude-code"))];
        let active = active_kinds(&windows);
        assert!(!active.contains("antigravity"));
        assert!(!account_is_active("antigravity", &active));
    }

    #[test]
    fn claude_gated_on_claude_code_lane_windows_unstamped_counts_as_claude_code() {
        // No `@repomon_agent_kind` at all - an older window that predates the stamp. It must
        // still count as claude-code (the default kind), not as "no kind".
        let windows = [wm("lane-2", None)];
        let active = active_kinds(&windows);
        assert!(active.contains("claude-code"));
        // Any Claude account key - the default account and a named config-dir variant - is
        // gated the same way: on claude-code presence, not per-account.
        assert!(account_is_active("default", &active));
        assert!(account_is_active("/Users/x/.claude-work", &active));
        // Non-claude sentinels are unaffected by a claude-code window.
        assert!(!account_is_active("codex", &active));
        assert!(!account_is_active("antigravity", &active));
    }

    #[test]
    fn usage_probe_and_term_windows_never_count_as_active_sessions() {
        // If these counted, the usage probe would sustain its own gate forever: probing codex
        // spawns `usage-probe-codex`, which (if counted) would make codex look "active" for the
        // next round, on and on.
        let windows = [
            wm("usage-probe-work", Some("claude-code")),
            wm("usage-probe-codex", Some("codex")),
            wm("term-1-1", Some("antigravity")),
        ];
        let active = active_kinds(&windows);
        assert!(
            active.is_empty(),
            "probe/term windows must not be counted: {active:?}"
        );
    }

    #[test]
    fn non_lane_window_names_are_ignored() {
        let windows = [wm("random-window", Some("codex")), wm("bash", None)];
        let active = active_kinds(&windows);
        assert!(active.is_empty());
    }

    #[test]
    fn gating_does_not_evict_cache_for_inactive_kind() {
        // Design decision: an inactive kind's last usage reading stays visible in the UI (it
        // just stops refreshing) rather than being blanked the instant its lane session exits.
        // In `usage_watcher`, the cache-retention key set (`live`) comes from the full
        // `accounts()` list, unfiltered by `active_kinds`/`account_is_active` - so an account
        // gated out of *probing* this round is never gated out of the *cache*. This test locks
        // in that the two checks are genuinely independent: an account can fail
        // `account_is_active` while still belonging to the retention set.
        let installed_keys: HashSet<String> = ["default".to_string(), "codex".to_string()]
            .into_iter()
            .collect();
        let windows = [wm("lane-1", Some("claude-code"))]; // no codex lane window this round
        let active = active_kinds(&windows);

        assert!(
            !account_is_active("codex", &active),
            "codex has no active lane window, so probing it this round should be skipped"
        );
        assert!(
            installed_keys.contains("codex"),
            "codex must still be in the cache-retention key set, so ctx.usage.retain(..) \
             (which checks `live`, not `active`) keeps its last reading"
        );
    }

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
            probe_state("Claude Code v2.1.233\n auto mode on", &claude),
            ProbeState::Ready
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

        let agy = antigravity_spec();
        assert_eq!(
            probe_state("Antigravity CLI\n Models & Quota", &agy),
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
            &Cancel::new(PROBE_TIMEOUT),
        );
        let _ = std::process::Command::new(repomon_core::agent::tmux_program())
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
        let report = probe_once(
            &tmux,
            "usage-probe-codex-test",
            &probe_cwd(),
            &codex_spec(),
            &Cancel::new(PROBE_TIMEOUT),
        );
        let _ = std::process::Command::new(repomon_core::agent::tmux_program())
            .args(["-L", "repomon-usagetest-codex", "kill-server"])
            .output();
        let r = report.expect("probe should scrape and parse /status");
        eprintln!("codex windows: {:?}", r.windows);
        assert!(!r.windows.is_empty());
    }
}
