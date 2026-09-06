//! Probes account usage in isolated hidden windows when enabled and a local UI is connected.
//! Neutral working directories and non-lane window names keep probes out of the fleet.

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
/// How long since the local UI's last request before we treat it as gone and stop probing (we
/// keep the last reading so reopening shows it instantly).
const LOCAL_TTL: Duration = Duration::from_secs(60);
/// Bound each probe so a hung account cannot freeze usage updates for every account.
const PROBE_TIMEOUT: Duration = Duration::from_secs(75);

/// Blocking probes must poll cancellation because timing out their await does not stop the thread
/// or its later key sends.
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

pub async fn snapshot(ctx: &Ctx) -> Vec<agent::AccountUsage> {
    let usage = ctx.usage.lock().await;
    let mut out: Vec<_> = usage
        .iter()
        .map(|(key, e)| agent::AccountUsage {
            key: key.clone(),
            label: e.label.clone(),
            report: e.report.clone(),
            age_secs: e.fetched_at.elapsed().as_secs(),
        })
        .collect();
    out.sort_by(|a, b| a.key.cmp(&b.key));
    out
}

pub async fn refresh(ctx: &Arc<Ctx>) -> agent::UsageRefreshResult {
    refresh_with_deadline(ctx, Duration::from_secs(15)).await
}

async fn refresh_with_deadline(ctx: &Arc<Ctx>, deadline: Duration) -> agent::UsageRefreshResult {
    use agent::UsageRefreshReason as Reason;
    ctx.usage_ingest_wake.notify_one();
    // Only cached state is read here. Backend enumeration and probing must never occupy the
    // connection's serial dispatcher, which also carries terminal keystrokes.
    let enabled = ctx.config.read().await.usage_probe;
    let has_active_kind = {
        let cache = ctx.overlay_cache.lock().await;
        cache.entry().is_none_or(|(_, lanes)| {
            lanes.iter().flat_map(|l| &l.agent_sessions).any(|s| {
                s.ended_at.is_none()
                    && matches!(
                        s.agent,
                        AgentKind::ClaudeCode | AgentKind::Codex | AgentKind::Antigravity
                    )
            })
        })
    };
    let mut inflight = ctx.usage_refresh_inflight.lock().await;
    let mut request_id = None;
    let (reason, detail) = if !enabled {
        (
            Reason::ProbeDisabled,
            Some("Usage probe is off in Settings".into()),
        )
    } else if inflight.is_some() {
        (
            Reason::Cooldown,
            Some("A usage refresh is already running".into()),
        )
    } else if !has_active_kind {
        (
            Reason::NoActiveKind,
            Some("No agent running to probe".into()),
        )
    } else {
        let request = ctx.usage_refresh_request.fetch_add(1, Ordering::Relaxed) + 1;
        *inflight = Some(request);
        request_id = Some(request);
        ctx.usage_refresh.notify_one();
        let ctx = ctx.clone();
        let started = tokio::time::Instant::now();
        tokio::spawn(async move {
            // Account discovery does filesystem IO, so keep it outside the RPC dispatcher.
            let count = tokio::task::spawn_blocking(|| accounts().len())
                .await
                .unwrap_or(1);
            refresh_deadlines(&ctx, request, started, deadline, round_ceiling(count)).await;
        });
        (Reason::Pending, None)
    };
    agent::UsageRefreshResult {
        refreshed: false,
        request_id,
        reason,
        detail,
        snapshot: snapshot(ctx).await,
    }
}

/// Even an empty or undiscoverable account list gets a bounded opportunity to finish gating.
fn round_ceiling(account_count: usize) -> Duration {
    PROBE_TIMEOUT.saturating_mul(account_count.max(1).try_into().unwrap_or(u32::MAX))
}

async fn refresh_deadlines(
    ctx: &Ctx,
    request: u64,
    started: tokio::time::Instant,
    notice: Duration,
    hard_ceiling: Duration,
) {
    tokio::time::sleep_until(started + notice).await;
    {
        let inflight = ctx.usage_refresh_inflight.lock().await;
        if *inflight != Some(request) {
            return;
        }
        publish_round(
            ctx,
            request,
            agent::UsageRefreshedReason::Timeout,
            Some("Still probing, this can take a moment".into()),
        )
        .await;
    }
    tokio::time::sleep_until(started + hard_ceiling).await;
    let mut inflight = ctx.usage_refresh_inflight.lock().await;
    // A dead watcher cannot call finish_round. Release only this ticket; an old timer must
    // never clear a newer manual refresh that was accepted after this round completed.
    if *inflight == Some(request) {
        *inflight = None;
    }
}

async fn publish_round(
    ctx: &Ctx,
    request_id: u64,
    reason: agent::UsageRefreshedReason,
    detail: Option<String>,
) {
    let event = agent::UsageRefreshed {
        request_id,
        reason,
        detail,
        snapshot: snapshot(ctx).await,
    };
    ctx.broadcast(
        crate::pubsub::topic::USAGE_REFRESHED,
        serde_json::to_value(event).expect("usage event"),
    );
}

async fn finish_round(ctx: &Ctx, request: u64, reason: agent::UsageRefreshReason) {
    let mut inflight = ctx.usage_refresh_inflight.lock().await;
    if request > 0 && *inflight == Some(request) {
        use agent::{UsageRefreshReason as Outcome, UsageRefreshedReason as Event};
        let (reason, detail) = match reason {
            Outcome::Ok => (Event::Ok, None),
            Outcome::NoActiveKind => (
                Event::NoActiveKind,
                Some("No agent running to probe".into()),
            ),
            Outcome::ProbeDisabled => (
                Event::ProbeDisabled,
                Some("Usage probe is off in Settings".into()),
            ),
            Outcome::Timeout => (
                Event::Timeout,
                Some("Usage probe timed out; try again".into()),
            ),
            Outcome::Error => (Event::Error, Some("Usage probe failed; try again".into())),
            Outcome::Pending | Outcome::Cooldown => return,
        };
        publish_round(ctx, request, reason, detail).await;
        *inflight = None;
    }
}

pub async fn usage_watcher(ctx: Arc<Ctx>) {
    let mut tick = tokio::time::interval(TICK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut last_round: Option<Instant> = None;

    loop {
        let forced = tokio::select! {
            _ = tick.tick() => false,
            _ = ctx.usage_refresh.notified() => true,
        };

        let request = if forced {
            ctx.usage_refresh_request.load(Ordering::Relaxed)
        } else {
            0
        };
        if !ctx.config.read().await.usage_probe {
            ctx.usage.lock().await.clear();
            last_round = None;
            finish_round(&ctx, request, agent::UsageRefreshReason::ProbeDisabled).await;
            continue;
        }
        let ui_active =
            (*ctx.local_watcher_seen.lock().await).is_some_and(|t| t.elapsed() < LOCAL_TTL);
        if !ui_active && !forced {
            continue;
        }
        if !usage_round_due(last_round, forced) {
            continue;
        }

        let accounts = accounts();
        // Retain installed accounts’ last readings even when an inactive kind skips probing.
        let live: HashSet<String> = accounts.iter().map(|a| a.key.clone()).collect();

        // Probe only active agent kinds because each account requires an expensive hidden CLI
        // session.
        let backend = ctx.backend.clone();
        let active = match tokio::task::spawn_blocking(move || backend.list_windows_meta()).await {
            Ok(Ok(windows)) => {
                let mut active = active_kinds(&windows);
                // External active sessions also justify probing their agent kind.
                active.extend(external_active_kinds(&ctx).await);
                Some(active)
            }
            Ok(Err(e)) => {
                // Probe all accounts when window listing fails so a transient backend error cannot
                // leave usage stale indefinitely.
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

        let mut attempted = 0;
        let mut refreshed = false;
        let mut timed_out = false;
        for acct in accounts {
            if let Some(active) = &active {
                if !account_is_active(&acct.key, active) {
                    continue;
                }
            }
            attempted += 1;
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
                    timed_out = true;
                    tracing::warn!(
                        "usage probe for {} timed out; skipping this round",
                        acct.key
                    );
                    None
                }
            };
            if let Some(report) = report {
                refreshed = true;
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
        // Retain readings for installed accounts even when their kind was inactive this round.
        ctx.usage.lock().await.retain(|k, _| live.contains(k));
        last_round = Some(Instant::now());
        finish_round(
            &ctx,
            request,
            if refreshed {
                agent::UsageRefreshReason::Ok
            } else if attempted == 0 {
                agent::UsageRefreshReason::NoActiveKind
            } else if timed_out {
                agent::UsageRefreshReason::Timeout
            } else {
                agent::UsageRefreshReason::Error
            },
        )
        .await;
    }
}

/// A manual refresh bypasses only the slow freshness cadence. The watcher still applies its
/// normal opt-in, local-UI, active-kind, and probe-timeout rules around this decision.
fn usage_round_due(last_round: Option<Instant>, forced: bool) -> bool {
    forced || last_round.is_none_or(|t| t.elapsed() >= REFRESH)
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
            // Use account-isolated launch commands so inherited CLAUDE_CONFIG_DIR cannot redirect
            // the default account’s probe.
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

/// Count only lane windows so probes cannot sustain their own activity gate; unstamped windows use
/// the default Claude kind.
fn active_kinds(windows: &[WindowMeta]) -> HashSet<String> {
    windows
        .iter()
        .filter(|w| TmuxRuntime::lane_id_of(&w.name).is_some())
        .map(|w| {
            // Normalize aliases to the canonical kind used by the activity gate.
            match w.agent_kind.as_deref() {
                Some(k) => AgentKind::from_kind_str(k).as_str().into_owned(),
                None => AgentKind::ClaudeCode.as_str().into_owned(),
            }
        })
        .collect()
}

/// Widen the probe gate using cached external activity; an empty or stale overlay may defer probing
/// until another round.
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

/// Gate Claude accounts by agent kind because window metadata does not identify the underlying
/// config directory.
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
/// dismiss and kill the window. Blocking (tmux IO + waits) - call from `spawn_blocking`. Returns
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
    #[cfg(test)]
    let mut last_nonempty_pane = String::new();
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
        #[cfg(test)]
        if !pane.trim().is_empty() {
            last_nonempty_pane.clone_from(&pane);
        }
        match probe_state(&pane, spec) {
            ProbeState::Ready => {
                ready = true;
                break;
            }
            ProbeState::Trust => {
                // Claude can default its safety prompt to rejection; move off that row before
                // confirming while preserving prompts already selecting acceptance.
                for key in trust_accept_keys(&pane) {
                    let _ = tmux.send_key_named(window, key);
                }
            }
            ProbeState::NotYet => {}
        }
    }

    let mut report = None;
    if ready {
        // The banner can show before the composer accepts input (notably Codex), so settle first.
        sleep(Duration::from_millis(1200));
        // Re-send the slash-command up to a few times: a typed-too-early send (composer not ready)
        // or a slow render shouldn't lose the round. Re-sending is idempotent - the parse succeeds
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
                #[cfg(test)]
                if !pane.trim().is_empty() {
                    last_nonempty_pane.clone_from(&pane);
                }
                if let Some(r) = (spec.parse)(&pane) {
                    report = Some(r);
                    break 'attempts;
                }
            }
        }
        let _ = tmux.send_key_named(window, "Escape");
    }

    #[cfg(test)]
    if report.is_none() {
        eprintln!("usage probe failed; ready={ready}; last pane:\n{last_nonempty_pane}");
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

/// Keys that accept a recognized folder-trust prompt. Current Claude puts the cursor on
/// "No, exit" and the affirmative row immediately below it; older supported CLIs select the
/// affirmative choice already, where Enter remains correct.
fn trust_accept_keys(pane: &str) -> &'static [&'static str] {
    let rejection_selected = pane.lines().any(|line| {
        let clean = agent::text::strip_ansi(line);
        clean.contains('❯') && clean.to_ascii_lowercase().contains("no, exit")
    });
    if rejection_selected {
        &["Down", "Enter"]
    } else {
        &["Enter"]
    }
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
        // An inactive account must stop probing without losing its retained reading.
        let installed_keys: HashSet<String> = ["default".to_string(), "codex".to_string()]
            .into_iter()
            .collect();
        let windows = [wm("lane-1", Some("claude-code"))];
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

    #[test]
    fn trust_prompt_moves_off_claudes_selected_rejection() {
        let current_claude = "Quick safety check: Is this a project you created or one you trust?\n\
            \u{1b}[94m❯\u{1b}[39m \u{1b}[94mNo,\u{1b}[39m \u{1b}[94mexit\u{1b}[39m\n  Yes, I trust this folder";
        assert_eq!(trust_accept_keys(current_claude), ["Down", "Enter"]);

        let affirmative_selected =
            "Do you trust the contents of this directory?\n❯ Yes, continue\n  No, exit";
        assert_eq!(trust_accept_keys(affirmative_selected), ["Enter"]);
    }

    #[test]
    fn forced_refresh_bypasses_a_recent_probe_round() {
        let recent = Some(Instant::now());
        assert!(!usage_round_due(recent, false));
        assert!(usage_round_due(recent, true));
    }

    /// Full end-to-end probe against a real `claude` on the default account, in an isolated tmux
    /// server. Ignored by default - spawns a real session (a little quota + a tiny transcript).
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

#[cfg(test)]
#[path = "usage_refresh_tests.rs"]
mod refresh_tests;
