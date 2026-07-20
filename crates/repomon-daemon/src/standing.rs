//! Standing orchestrations: bounded, headless repomind runs the daemon fires on a schedule
//! (and, config-gated, as needs-you triage). A run is `claude -p <prompt>` wired to the fleet
//! MCP server with `REPOMON_MCP_UNATTENDED=1` and a low action cap, wall-clock-limited; its
//! output is journaled (`action = standing_run` / `triage_run`) and delivered through the
//! existing notification paths. Unattended runs are deliberately MORE conservative than
//! attended ones: the MCP policy refuses merge/delete outright, and the wall clock kills a
//! wedged run.

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Local, Utc};
use repomon_core::model::JournalEntry;
use repomon_core::schedule;
use serde_json::json;

use crate::{Ctx, push};

/// How often the scheduler looks for due schedules.
const TICK: Duration = Duration::from_secs(30);

/// Matches notify_watch's TUI-heartbeat freshness window: the daemon pops desktop
/// notifications only when no TUI is actively covering them.
const LOCAL_TTL: Duration = Duration::from_secs(3);

/// The scheduler loop. Runs execute inline (one at a time, oldest schedule first): standing
/// runs are minutes-scale and serializing them is a feature — a pileup of concurrent
/// unattended orchestrators is exactly what the bounds exist to prevent.
pub async fn standing_watch(ctx: Arc<Ctx>) {
    let mut tick = tokio::time::interval(TICK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tick.tick().await;
        scheduler_tick(&ctx, Local::now()).await;
    }
}

/// One scheduler pass at `now` (injected for tests). A schedule is due when its spec's next
/// firing after `last_run_at` (or `created_at`) is not in the future; `last_run_at` is stamped
/// BEFORE the run so a slow run can't double-fire.
pub async fn scheduler_tick(ctx: &Arc<Ctx>, now: DateTime<Local>) {
    let Ok(scheds) = ctx.store.list_schedules().await else {
        return;
    };
    for s in scheds {
        let Ok(spec) = schedule::parse_spec(&s.spec) else {
            tracing::warn!(id = s.id, spec = %s.spec, "unparseable schedule spec; skipping");
            continue;
        };
        let anchor = s.last_run_at.unwrap_or(s.created_at).with_timezone(&Local);
        if spec.next_after(anchor) > now {
            continue;
        }
        let _ = ctx
            .store
            .mark_schedule_run(s.id, now.with_timezone(&Utc))
            .await;
        tracing::info!(id = s.id, spec = %s.spec, "standing run firing");
        run_standing(
            ctx,
            "standing_run",
            &format!("standing-{}", s.id),
            &s.prompt,
            s.max_actions,
            json!({ "schedule_id": s.id, "spec": s.spec, "prompt": clip(&s.prompt, 200) }),
            None,
        )
        .await;
    }
}

/// Execute one bounded headless orchestration and record it: journal entry + notification.
/// `kind` is the journal action (`standing_run` / `triage_run`); `tag` prefixes the journal
/// session and notification dedup id.
pub async fn run_standing(
    ctx: &Arc<Ctx>,
    kind: &str,
    tag: &str,
    prompt: &str,
    max_actions: u32,
    params: serde_json::Value,
    lane_id: Option<i64>,
) {
    let cfg = ctx.config.read().await.clone();
    let socket = repomon_core::config::socket_path(&cfg);
    let (ok, output) = match build_run_command(&cfg, &socket, prompt, max_actions) {
        Ok(command) => {
            let timeout = Duration::from_secs(cfg.standing_timeout_secs.max(30));
            run_bounded(&command, timeout).await
        }
        Err(e) => (false, e),
    };

    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let session = format!("{tag}-{nanos}");
    let entry = JournalEntry {
        id: 0,
        at: Utc::now(),
        session: session.clone(),
        action: kind.to_string(),
        lane_id,
        repo: None,
        params: Some(params.to_string()),
        outcome: if ok { "ok" } else { "error" }.to_string(),
        detail: Some(tail(&output, 4000)),
    };
    if let Err(e) = ctx.store.append_journal(entry).await {
        tracing::warn!("standing run journal append failed: {e}");
    }

    let title = format!("repomind: {}", clip(prompt, 40));
    let body = tail(&output, 300);
    let payload = json!({
        "id": session,
        "kind": kind,
        "lane_id": lane_id,
        "title": title,
        "body": body,
        "ok": ok,
    });
    if cfg.remote.enabled {
        ctx.broadcast("event.notification", payload.clone());
        push::send_all(ctx, &title, &body, push::CATEGORY_ALERT, &payload).await;
    }
    let tui_active =
        (*ctx.local_watcher_seen.lock().await).is_some_and(|t| t.elapsed() < LOCAL_TTL);
    if !tui_active {
        repomon_core::notify::send_native(&title, &body, cfg.notify_sound, false);
    }
}

/// Resolve the headless run command. A config custom agent runs verbatim with the prompt
/// appended (the `agent.spawn` semantics — this is also the test seam); claude and its account
/// variants get the full `-p` composition against a per-run MCP config with the unattended
/// guardrail env.
fn build_run_command(
    cfg: &repomon_core::Config,
    socket: &std::path::Path,
    prompt: &str,
    max_actions: u32,
) -> Result<String, String> {
    use repomon_core::agent::tmux::shell_quote;
    let backend = crate::rpc::resolve_orchestrator_backend(&cfg.orchestrator_agent, &cfg.agents)
        .map_err(|e| e.message)?;
    if matches!(backend, crate::OrchestratorBackend::Codex) {
        return Err("headless standing runs support the claude backend only".to_string());
    }
    let base = crate::rpc::orchestrator_base_command(&cfg.orchestrator_agent, &cfg.agents);
    if cfg
        .orchestrator_agent
        .as_ref()
        .is_some_and(|a| cfg.agents.contains_key(a))
    {
        return Ok(format!("{base} {}", shell_quote(prompt)));
    }
    let extra_env = [
        ("REPOMON_MCP_UNATTENDED", "1".to_string()),
        ("REPOMON_MCP_MAX_ACTIONS", max_actions.to_string()),
    ];
    let mcp_path = crate::rpc::write_orchestrator_mcp_config_named(
        socket,
        "autonomous",
        None,
        &extra_env,
        "repomind-standing-mcp.json",
    )
    .map_err(|e| format!("couldn't write the standing-run MCP config: {e}"))?;
    Ok(build_headless_command(
        &base,
        &mcp_path,
        &cfg.orchestrator_model,
        prompt,
    ))
}

/// Compose the `claude -p` invocation for a headless run: persona + unattended addendum as the
/// system prompt, fleet tools only (no external memory server in unattended mode), no session
/// pinning (nothing attaches to a headless run).
pub fn build_headless_command(
    base: &str,
    mcp_config_path: &std::path::Path,
    model: &Option<String>,
    prompt: &str,
) -> String {
    use repomon_core::agent::tmux::shell_quote;
    let mut command = base.to_string();
    command.push_str(" -p ");
    command.push_str(&shell_quote(prompt));
    command.push_str(" --mcp-config ");
    command.push_str(&shell_quote(&mcp_config_path.to_string_lossy()));
    command.push_str(" --append-system-prompt ");
    let persona = format!(
        "{}{}",
        repomon_mcp::PERSONA,
        repomon_mcp::UNATTENDED_ADDENDUM
    );
    command.push_str(&shell_quote(&persona));
    command.push_str(" --allowedTools mcp__repomon");
    if let Some(model) = model {
        command.push_str(" --model ");
        command.push_str(&shell_quote(model));
    }
    command
}

/// Run `command` via `sh -c` with a wall clock. Returns `(succeeded, combined output)`; on
/// timeout the child is killed (`kill_on_drop`) and the output explains why.
pub async fn run_bounded(command: &str, timeout: Duration) -> (bool, String) {
    use std::process::Stdio;
    let out = tokio::process::Command::new("sh")
        .arg("-c")
        .arg(command)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .output();
    match tokio::time::timeout(timeout, out).await {
        Err(_) => (
            false,
            format!(
                "run timed out after {}s (wall-clock limit)",
                timeout.as_secs()
            ),
        ),
        Ok(Err(e)) => (false, format!("failed to launch run: {e}")),
        Ok(Ok(out)) => {
            let mut s = String::from_utf8_lossy(&out.stdout).into_owned();
            s.push_str(&String::from_utf8_lossy(&out.stderr));
            (out.status.success(), s)
        }
    }
}

/// First `max` chars of `s` (single line).
fn clip(s: &str, max: usize) -> String {
    let one_line = s.replace('\n', " ");
    if one_line.chars().count() <= max {
        return one_line;
    }
    let mut out: String = one_line.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Last `max` chars of `s`.
fn tail(s: &str, max: usize) -> String {
    let trimmed = s.trim();
    let count = trimmed.chars().count();
    if count <= max {
        return trimmed.to_string();
    }
    trimmed.chars().skip(count - max).collect()
}

/// Whether a pending needs-you triage should fire: the agent has sat unattended long enough
/// and still no UI (local TUI or remote companion) is attached. Pure so the policy is testable
/// without a fleet.
pub fn triage_due(elapsed: Duration, after_mins: u64, ui_attached: bool) -> bool {
    !ui_attached && elapsed >= Duration::from_secs(after_mins * 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn triage_fires_only_after_the_window_with_no_ui() {
        let m10 = Duration::from_secs(10 * 60);
        assert!(triage_due(m10, 10, false));
        assert!(triage_due(m10 + Duration::from_secs(1), 10, false));
        assert!(!triage_due(m10 - Duration::from_secs(1), 10, false));
        // Any attached UI suppresses triage: the human is already looking.
        assert!(!triage_due(m10, 10, true));
    }

    #[test]
    fn headless_command_composes_claude_print_mode() {
        let cmd = build_headless_command(
            "claude",
            std::path::Path::new("/tmp/x.json"),
            &Some("opus".into()),
            "morning briefing",
        );
        assert!(cmd.starts_with("claude -p "));
        assert!(cmd.contains("--mcp-config"));
        assert!(cmd.contains("--append-system-prompt"));
        assert!(cmd.contains("--allowedTools mcp__repomon"));
        assert!(cmd.contains("--model"));
        assert!(
            cmd.contains("Unattended run"),
            "system prompt must carry the unattended addendum"
        );
        assert!(
            !cmd.contains("--session-id"),
            "headless runs don't pin sessions"
        );
    }
}
