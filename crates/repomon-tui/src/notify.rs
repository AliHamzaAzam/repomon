//! Desktop + in-app notifications for agent state changes.
//!
//! The TUI watches each agent session's status across refreshes (see
//! `App::detect_notifications`) and, on a meaningful transition, composes a notification here:
//! a native macOS popup (via `osascript`, fired on a short-lived thread so it never blocks the
//! event loop) plus an in-app banner and a scrollable history feed. Edge detection and the
//! config toggles live in `app`.

use chrono::{DateTime, Local};

use repomon_core::model::{AgentSession, Lane, LaneId};

/// The kind of agent state-change that fired a notification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
    fn verb_plural(self) -> &'static str {
        match self {
            NotifKind::NeedsYou => "need you",
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
    /// The lane the alert was about — lets the feed jump straight to it.
    pub lane_id: LaneId,
    /// The session that fired (Claude transcript id) — lets the feed open/attach the exact
    /// agent in a multi-agent lane. `None` when the session couldn't be identified.
    pub session_id: Option<String>,
    /// False until the user opens the Notifications view; drives the ⚑ unread badge.
    pub read: bool,
    pub title: String,
    pub body: String,
}

/// Build the `(title, body)` for a notification about one of `lane`'s sessions. The body
/// carries the detail that makes the alert actionable: branch, which of the lane's
/// side-by-side agents fired (`slot` = (index, count), tagged only when several run), the
/// *why* — the agent's actual last message when `show_why` is on (falling back to what you
/// originally asked) — tool count, and any reset time. `sess` is `None` when the session
/// vanished from the snapshot (its disappearance was the trigger) — the text degrades to a
/// generic "agent" line rather than borrowing another session's name and title.
pub fn compose(
    kind: NotifKind,
    lane: &Lane,
    sess: Option<&AgentSession>,
    slot: Option<(usize, usize)>,
    show_why: bool,
) -> (String, String) {
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
    if let Some((i, n)) = slot {
        if n > 1 {
            parts.push(format!("agent {}/{}", i + 1, n));
        }
    }
    let why = show_why
        .then(|| sess.and_then(|s| s.last_message.as_deref()))
        .flatten();
    if let Some(t) = why.or_else(|| sess.and_then(|s| s.title.as_deref())) {
        let t = t.trim();
        if !t.is_empty() {
            parts.push(format!("“{}”", truncate(t, 100)));
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

/// One popup for a burst of simultaneous alerts: the title counts them (with the kind's glyph
/// and verb when the whole burst is one kind, a generic ⚑ otherwise), the body lists the first
/// few lanes. `fires` pairs each alert's `repo/worktree` label with its kind.
pub fn compose_burst(fires: &[(String, NotifKind)]) -> (String, String) {
    let n = fires.len();
    let first = fires.first().map(|(_, k)| *k);
    let uniform = fires.iter().all(|(_, k)| Some(*k) == first);
    let title = match (uniform, first) {
        (true, Some(k)) => format!("{} {} agents {}", k.glyph(), n, k.verb_plural()),
        _ => format!("⚑ {n} agents need attention"),
    };
    let mut body = fires
        .iter()
        .take(3)
        .map(|(l, _)| l.as_str())
        .collect::<Vec<_>>()
        .join(" · ");
    if n > 3 {
        body.push_str(&format!(" · +{} more", n - 3));
    }
    (title, body)
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Play the notification chime once, off-thread — a preview used when the user enables sound in
/// Settings so they can confirm it's audible without waiting for an agent to change state.
pub fn play_chime() {
    #[cfg(target_os = "macos")]
    std::thread::spawn(|| {
        let _ = std::process::Command::new("afplay")
            .arg(NOTIFY_SOUND_FILE)
            .output();
    });
}

/// Fire a native desktop notification, best-effort and without blocking the caller: the actual
/// `osascript`/`notify-send` invocation runs (and is reaped) on a detached thread.
/// `click_focus` makes the popup click-to-focus the terminal when `terminal-notifier` is
/// installed (plain popup otherwise).
pub fn send_native(title: &str, body: &str, sound: bool, click_focus: bool) {
    let (title, body) = (title.to_string(), body.to_string());
    std::thread::spawn(move || {
        run_native(&title, &body, sound, click_focus);
    });
}

/// A system sound file played for an audible notification (see [`run_native`]).
#[cfg(target_os = "macos")]
const NOTIFY_SOUND_FILE: &str = "/System/Library/Sounds/Glass.aiff";

#[cfg(target_os = "macos")]
fn run_native(title: &str, body: &str, sound: bool, click_focus: bool) {
    // Prefer the clickable popup; fall back to osascript when terminal-notifier isn't
    // installed (or click-to-focus is off). We deliberately do NOT use osascript's own
    // `sound name`: on recent macOS the notification is attributed to "Script Editor", whose
    // notification sound is usually off, so the chime is silently dropped even though the
    // call succeeds.
    let clickable = click_focus && notify_clickable(title, body);
    if !clickable {
        let script = format!(
            "display notification \"{}\" with title \"{}\"",
            escape(body),
            escape(title),
        );
        let _ = std::process::Command::new("osascript")
            .arg("-e")
            .arg(script)
            .output();
    }
    // Play the sound directly instead — `afplay` is audible regardless of notification settings.
    if sound {
        let _ = std::process::Command::new("afplay")
            .arg(NOTIFY_SOUND_FILE)
            .output();
    }
}

/// Post a click-to-focus popup via `terminal-notifier`: clicking it activates the terminal
/// the TUI runs in (resolved from `$TERM_PROGRAM`). Returns false when terminal-notifier
/// isn't installed, so the caller can fall back to a plain popup.
#[cfg(target_os = "macos")]
fn notify_clickable(title: &str, body: &str) -> bool {
    let Some(bin) = terminal_notifier() else {
        return false;
    };
    let mut cmd = std::process::Command::new(bin);
    cmd.args(["-title", title, "-message", body]);
    if let Some(bundle) = std::env::var("TERM_PROGRAM")
        .ok()
        .as_deref()
        .and_then(terminal_bundle_id)
    {
        cmd.args(["-activate", bundle]);
    }
    cmd.output().is_ok()
}

/// The installed `terminal-notifier` binary, located once per process. `None` = not installed.
#[cfg(target_os = "macos")]
fn terminal_notifier() -> Option<&'static str> {
    fn locate() -> Option<String> {
        let out = std::process::Command::new("which")
            .arg("terminal-notifier")
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let p = String::from_utf8_lossy(&out.stdout).trim().to_string();
        (!p.is_empty()).then_some(p)
    }
    static FOUND: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    FOUND.get_or_init(locate).as_deref()
}

/// The macOS bundle id behind a `$TERM_PROGRAM` value — what a clicked notification focuses.
/// Unknown terminals (including `tmux`, which masks the real one) get no `-activate`.
#[cfg(target_os = "macos")]
fn terminal_bundle_id(term_program: &str) -> Option<&'static str> {
    match term_program {
        "iTerm.app" => Some("com.googlecode.iterm2"),
        "Apple_Terminal" => Some("com.apple.Terminal"),
        "WezTerm" => Some("com.github.wez.wezterm"),
        "ghostty" => Some("com.mitchellh.ghostty"),
        "vscode" => Some("com.microsoft.VSCode"),
        "kitty" => Some("net.kovidgoyal.kitty"),
        _ => None,
    }
}

#[cfg(not(target_os = "macos"))]
fn run_native(title: &str, body: &str, _sound: bool, _click_focus: bool) {
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
    use chrono::Utc;
    use repomon_core::model::{
        AgentKind, AgentSession, AgentStatus, Repo, Worktree, WorktreeState,
    };

    fn lane() -> Lane {
        let head = gix::ObjectId::null(gix::hash::Kind::Sha1);
        Lane {
            id: 7,
            repo: Repo {
                id: 1,
                path: "/code/alpha".into(),
                name: "alpha".into(),
                added_at: Utc::now(),
                worktree_root_template: None,
            },
            worktree: Worktree {
                id: 1,
                repo_id: 1,
                path: "/code/alpha".into(),
                branch: Some("main".into()),
                head,
                is_main: true,
                name: "main".into(),
            },
            state: WorktreeState {
                worktree_id: 1,
                head,
                branch: Some("feat/x".into()),
                upstream: None,
                ahead: 0,
                behind: 0,
                dirty: Default::default(),
                last_commit_at: None,
                locked: false,
                prunable: false,
                last_change_at: None,
            },
            agent_sessions: vec![],
            last_activity_at: Utc::now(),
            pinned: false,
        }
    }

    fn sess() -> AgentSession {
        AgentSession {
            id: 0,
            agent: AgentKind::ClaudeCode,
            repo_id: 1,
            worktree_id: Some(1),
            started_at: Utc::now(),
            last_activity_at: Utc::now(),
            ended_at: None,
            manifest_path: std::path::PathBuf::new(),
            tool_call_count: 3,
            title: Some("build the parser".into()),
            status: AgentStatus::Waiting,
            external: false,
            session_id: Some("abc".into()),
            resume_at: None,
            inferred: false,
            tmux_window: None,
            last_message: Some("Should I also update the integration tests?".into()),
        }
    }

    #[test]
    fn compose_shows_the_why_and_the_slot() {
        let (title, body) = compose(
            NotifKind::NeedsYou,
            &lane(),
            Some(&sess()),
            Some((1, 3)),
            true,
        );
        assert!(
            title.contains("needs you") && title.contains("alpha"),
            "{title}"
        );
        assert!(body.contains("agent 2/3"), "{body}");
        assert!(body.contains("Should I also update"), "{body}");
        assert!(
            !body.contains("build the parser"),
            "why replaces the ask: {body}"
        );
    }

    #[test]
    fn compose_without_why_falls_back_to_the_ask_and_skips_solo_slot() {
        let (_, body) = compose(
            NotifKind::NeedsYou,
            &lane(),
            Some(&sess()),
            Some((0, 1)),
            false,
        );
        assert!(body.contains("build the parser"), "{body}");
        assert!(!body.contains("agent 1/1"), "{body}");
    }

    #[test]
    fn burst_counts_kinds_and_overflow() {
        let f = |l: &str, k: NotifKind| (l.to_string(), k);
        let (t, b) = compose_burst(&[
            f("alpha/main", NotifKind::NeedsYou),
            f("beta/x", NotifKind::NeedsYou),
        ]);
        assert_eq!(t, "⏸ 2 agents need you");
        assert_eq!(b, "alpha/main · beta/x");
        let (t, b) = compose_burst(&[
            f("a/1", NotifKind::NeedsYou),
            f("b/2", NotifKind::Idle),
            f("c/3", NotifKind::NeedsYou),
            f("d/4", NotifKind::NeedsYou),
        ]);
        assert_eq!(t, "⚑ 4 agents need attention");
        assert!(b.ends_with("+1 more"), "{b}");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn bundle_ids_cover_known_terminals_only() {
        assert_eq!(
            terminal_bundle_id("iTerm.app"),
            Some("com.googlecode.iterm2")
        );
        assert_eq!(
            terminal_bundle_id("Apple_Terminal"),
            Some("com.apple.Terminal")
        );
        // tmux masks the real terminal; unknowns get no -activate.
        assert_eq!(terminal_bundle_id("tmux"), None);
        assert_eq!(terminal_bundle_id(""), None);
    }

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
