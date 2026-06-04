//! Desktop + in-app notifications for agent state changes.
//!
//! The TUI watches each lane's agent status across refreshes (see `App::detect_notifications`)
//! and, on a meaningful transition, composes a notification here: a native macOS popup (via
//! `osascript`, fired on a short-lived thread so it never blocks the event loop) plus an in-app
//! banner and a scrollable history feed. Edge detection and the config toggles live in `app`.

use chrono::{DateTime, Local};

use repomon_core::model::{AgentSession, Lane};

/// The kind of agent state-change that fired a notification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotifKind {
    /// Agent finished its turn / is waiting on you.
    NeedsYou,
    /// Agent paused on a usage/rate limit.
    RateLimited,
    /// A rate-limited agent was auto-continued and resumed work.
    Resumed,
    /// Agent went idle or its session ended.
    Idle,
}

impl NotifKind {
    fn glyph(self) -> &'static str {
        match self {
            NotifKind::NeedsYou => "⏸",
            NotifKind::RateLimited => "⏳",
            NotifKind::Resumed => "▶",
            NotifKind::Idle => "○",
        }
    }
    fn verb(self) -> &'static str {
        match self {
            NotifKind::NeedsYou => "needs you",
            NotifKind::RateLimited => "hit a usage limit",
            NotifKind::Resumed => "resumed",
            NotifKind::Idle => "went idle",
        }
    }
}

/// A fired notification, kept in the in-app history feed.
#[derive(Debug, Clone)]
pub struct NotifEvent {
    pub when: DateTime<Local>,
    pub kind: NotifKind,
    pub title: String,
    pub body: String,
}

/// The session a notification is about: the managed/primary one if present (skipping the
/// inferred "file activity" placeholders, which never drive named alerts).
fn primary_session(lane: &Lane) -> Option<&AgentSession> {
    lane.agent_sessions
        .iter()
        .find(|s| !s.external && !s.inferred)
        .or_else(|| lane.agent_sessions.iter().find(|s| !s.inferred))
        .or_else(|| lane.agent_sessions.first())
}

/// Build the `(title, body)` for a notification about `lane`. The body carries the detail the
/// user asked for: branch, what they asked the agent, tool count, and any reset time.
pub fn compose(kind: NotifKind, lane: &Lane) -> (String, String) {
    let sess = primary_session(lane);
    let agent = sess
        .map(|s| s.agent.short().to_string())
        .unwrap_or_default();
    let agent = if agent.is_empty() {
        "agent".into()
    } else {
        agent
    };
    let title = format!(
        "{} {} {} — {}",
        kind.glyph(),
        agent,
        kind.verb(),
        lane.repo.name
    );

    let mut parts = vec![lane
        .state
        .branch
        .clone()
        .unwrap_or_else(|| lane.worktree.name.clone())];
    if let Some(t) = sess.and_then(|s| s.title.as_deref()) {
        let t = t.trim();
        if !t.is_empty() {
            parts.push(format!("“{}”", truncate(t, 60)));
        }
    }
    if let Some(s) = sess {
        if s.tool_call_count > 0 {
            parts.push(format!("{} tools", s.tool_call_count));
        }
    }
    if kind == NotifKind::RateLimited {
        if let Some(r) = sess.and_then(|s| s.resume_at) {
            parts.push(format!(
                "resets {}",
                r.with_timezone(&Local).format("%H:%M")
            ));
        }
    }
    (title, parts.join(" · "))
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Fire a native desktop notification, best-effort and without blocking the caller: the actual
/// `osascript`/`notify-send` invocation runs (and is reaped) on a detached thread.
pub fn send_native(title: &str, body: &str, sound: bool) {
    let (title, body) = (title.to_string(), body.to_string());
    std::thread::spawn(move || {
        run_native(&title, &body, sound);
    });
}

#[cfg(target_os = "macos")]
fn run_native(title: &str, body: &str, sound: bool) {
    let script = format!(
        "display notification \"{}\" with title \"{}\"{}",
        escape(body),
        escape(title),
        if sound { " sound name \"Ping\"" } else { "" },
    );
    let _ = std::process::Command::new("osascript")
        .arg("-e")
        .arg(script)
        .output();
}

#[cfg(not(target_os = "macos"))]
fn run_native(title: &str, body: &str, _sound: bool) {
    let _ = std::process::Command::new("notify-send")
        .arg(title)
        .arg(body)
        .output();
}

/// Escape a string for embedding in an AppleScript double-quoted literal.
#[cfg(target_os = "macos")]
fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_adds_ellipsis_only_when_over() {
        assert_eq!(truncate("short", 60), "short");
        let t = truncate("0123456789", 5);
        assert_eq!(t.chars().count(), 5);
        assert!(t.ends_with('…'));
        assert_eq!(t, "0123…");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn escape_neutralizes_applescript_quotes() {
        assert_eq!(escape(r#"say "hi" \ now"#), r#"say \"hi\" \\ now"#);
    }
}
