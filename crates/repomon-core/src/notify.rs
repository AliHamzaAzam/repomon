//! Agent state-change notifications: the pure, client-agnostic heart.
//!
//! Both the TUI (local popups) and the daemon (remote `event.notification` broadcasts + push)
//! watch per-session agent statuses across refreshes and alert on meaningful transitions.
//! Everything shared lives here: session keying, the status diff, transition classification,
//! and the `(title, body)` text composition, plus the local desktop delivery
//! ([`send_native`]) shared by the TUI and the daemon — the daemon fires it as a fallback when
//! the local TUI is parked (attached to a pane) or closed. Remote delivery (APNs) and the TUI's
//! in-app banner stay with their clients.

use std::collections::{HashMap, HashSet};

use chrono::Local;
use serde::{Deserialize, Serialize};

use crate::model::{AgentSession, AgentStatus, Lane, LaneId};

/// The kind of agent state-change that fired a notification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
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
    pub fn glyph(self) -> &'static str {
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

/// Identifies one real agent session within a lane across refreshes.
///
/// Transcript-backed sessions key on the Claude session id (the transcript filename stem),
/// which is stable across polls. `claude --resume` may continue the same logical work in a new
/// transcript; that reads as one session vanishing and another appearing — acceptable noise. A
/// lane has at most one real session *without* a transcript id per snapshot (the managed
/// no-transcript placeholder or the generic process monitor — mutually exclusive branches in
/// the daemon's `overlay_agents`), so a single `Fallback` sentinel covers it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SessKey {
    Transcript(String),
    Fallback,
}

/// Key/status pairs for one lane's *real* agent sessions. Inferred "file activity" placeholders
/// are dropped so they never drive named alerts. On a (theoretically impossible) duplicate key,
/// the higher-priority status wins — the same order the old per-lane rollup used.
pub fn session_statuses(
    lane_id: LaneId,
    sessions: &[AgentSession],
) -> Vec<((LaneId, SessKey), AgentStatus)> {
    let mut out: Vec<((LaneId, SessKey), AgentStatus)> = Vec::new();
    for s in sessions.iter().filter(|s| !s.inferred) {
        let key = (
            lane_id,
            s.session_id
                .clone()
                .map(SessKey::Transcript)
                .unwrap_or(SessKey::Fallback),
        );
        match out.iter_mut().find(|(k, _)| *k == key) {
            Some((_, st)) if status_priority(s.status) < status_priority(*st) => *st = s.status,
            Some(_) => {}
            None => out.push((key, s.status)),
        }
    }
    out
}

/// Notification priority of a status (lower = more urgent).
pub fn status_priority(s: AgentStatus) -> usize {
    use AgentStatus::*;
    [RateLimited, Waiting, Running, Idle, Ended]
        .iter()
        .position(|&x| x == s)
        .unwrap_or(usize::MAX)
}

/// Diff the previous and current per-session status maps into the notifications to fire.
///
/// Sessions present in `now` are edge-detected against their previous status. Sessions that
/// vanished fire as a transition to `None` (→ Idle if they were active), except when their
/// whole lane is gone (deleting a lane isn't an agent going quiet) or when a lane's `Fallback`
/// key was handed off to a transcript-backed key (`lanes_with_managed`): the managed
/// no-transcript placeholder disappears the moment the agent's transcript becomes parseable,
/// and firing Idle there would alert on every spawn.
pub fn diff_session_transitions(
    prev: &HashMap<(LaneId, SessKey), AgentStatus>,
    now: &HashMap<(LaneId, SessKey), AgentStatus>,
    live_lanes: &HashSet<LaneId>,
    lanes_with_managed: &HashSet<LaneId>,
) -> Vec<((LaneId, SessKey), NotifKind)> {
    let mut out = Vec::new();
    for (key, &status) in now {
        let was = prev.get(key).copied();
        if was == Some(status) {
            continue;
        }
        if let Some(kind) = transition_kind(was, Some(status)) {
            out.push((key.clone(), kind));
        }
    }
    for (key, &was) in prev {
        if now.contains_key(key) || !live_lanes.contains(&key.0) {
            continue;
        }
        if key.1 == SessKey::Fallback && lanes_with_managed.contains(&key.0) {
            continue;
        }
        if let Some(kind) = transition_kind(Some(was), None) {
            out.push((key.clone(), kind));
        }
    }
    out
}

/// Resolve a session key back to the lane's session, for composing the notification text.
/// `None` when the session vanished (i.e. its disappearance was the trigger).
pub fn session_by_key<'a>(lane: &'a Lane, key: &SessKey) -> Option<&'a AgentSession> {
    lane.agent_sessions
        .iter()
        .filter(|s| !s.inferred)
        .find(|s| match key {
            SessKey::Transcript(id) => s.session_id.as_deref() == Some(id.as_str()),
            SessKey::Fallback => s.session_id.is_none(),
        })
}

/// Which of the lane's real sessions `key` resolves to: `(index, count)`, for the
/// "agent 2/3" tag in multi-agent lanes. `None` when the session vanished.
pub fn slot_by_key(lane: &Lane, key: &SessKey) -> Option<(usize, usize)> {
    let real: Vec<&AgentSession> = lane.agent_sessions.iter().filter(|s| !s.inferred).collect();
    let i = real.iter().position(|s| match key {
        SessKey::Transcript(id) => s.session_id.as_deref() == Some(id.as_str()),
        SessKey::Fallback => s.session_id.is_none(),
    })?;
    Some((i, real.len()))
}

/// Map a session's status transition to the notification it should fire, if any. `None` means
/// the session was absent from that snapshot. Priority resolves cases like
/// `Running → RateLimited` to the limit.
pub fn transition_kind(prev: Option<AgentStatus>, now: Option<AgentStatus>) -> Option<NotifKind> {
    use AgentStatus::*;
    match (prev, now) {
        // Hit a usage limit.
        (p, Some(RateLimited)) if p != Some(RateLimited) => Some(NotifKind::RateLimited),
        // Auto-resumed after a limit.
        (Some(RateLimited), Some(Running)) => Some(NotifKind::Resumed),
        // Finished its turn / needs you.
        (p, Some(Waiting)) if p != Some(Waiting) => Some(NotifKind::NeedsYou),
        // Was active, now quiet (idle / ended / the session went away).
        (Some(Running) | Some(Waiting), Some(Idle) | Some(Ended) | None) => Some(NotifKind::Idle),
        _ => None,
    }
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

// ---- local desktop delivery (shared by the TUI and the daemon) ----

/// Play the notification chime once, off-thread — a preview used when enabling sound in Settings.
pub fn play_chime() {
    #[cfg(target_os = "macos")]
    std::thread::spawn(|| {
        let _ = std::process::Command::new("afplay")
            .arg(NOTIFY_SOUND_FILE)
            .output();
    });
}

/// Fire a native desktop notification, best-effort and without blocking the caller (the actual
/// `osascript`/`notify-send` invocation runs on a detached thread). `click_focus` makes the popup
/// click-to-focus the terminal when `terminal-notifier` is installed (plain popup otherwise).
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
    // Prefer the clickable popup; fall back to osascript when terminal-notifier isn't installed
    // (or click-to-focus is off). We deliberately do NOT use osascript's own `sound name`: on
    // recent macOS the notification is attributed to "Script Editor", whose notification sound is
    // usually off, so the chime is silently dropped. Play it with `afplay` instead.
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
    if sound {
        let _ = std::process::Command::new("afplay")
            .arg(NOTIFY_SOUND_FILE)
            .output();
    }
}

/// Post a click-to-focus popup via `terminal-notifier`. Returns false when it isn't installed, so
/// the caller can fall back to a plain popup.
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
    use crate::model::{AgentKind, Repo, Worktree, WorktreeState};
    use chrono::Utc;
    use std::path::PathBuf;

    /// A minimal real-or-inferred session, mirroring the daemon's `overlay_agents` literals.
    fn sess(session_id: Option<&str>, status: AgentStatus, inferred: bool) -> AgentSession {
        AgentSession {
            id: 0,
            agent: AgentKind::ClaudeCode,
            repo_id: 1,
            worktree_id: None,
            started_at: Utc::now(),
            last_activity_at: Utc::now(),
            ended_at: None,
            manifest_path: PathBuf::new(),
            tool_call_count: 0,
            title: None,
            status,
            external: false,
            session_id: session_id.map(str::to_string),
            resume_at: None,
            inferred,
            tmux_window: None,
            last_message: None,
            pending_prompt: None,
        }
    }

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

    #[test]
    fn notification_transitions() {
        use AgentStatus::*;
        // The headline alerts.
        assert_eq!(
            transition_kind(Some(Running), Some(Waiting)),
            Some(NotifKind::NeedsYou)
        );
        assert_eq!(
            transition_kind(None, Some(Waiting)),
            Some(NotifKind::NeedsYou)
        );
        assert_eq!(
            transition_kind(Some(Running), Some(RateLimited)),
            Some(NotifKind::RateLimited)
        );
        assert_eq!(
            transition_kind(Some(RateLimited), Some(Running)),
            Some(NotifKind::Resumed)
        );
        // Gave up on the limit and now needs you.
        assert_eq!(
            transition_kind(Some(RateLimited), Some(Waiting)),
            Some(NotifKind::NeedsYou)
        );
        // Went quiet (idle / ended / the agent went away).
        assert_eq!(
            transition_kind(Some(Running), Some(Idle)),
            Some(NotifKind::Idle)
        );
        assert_eq!(transition_kind(Some(Waiting), None), Some(NotifKind::Idle));
        // Non-events: you replied, work simply started, or nothing changed.
        assert_eq!(transition_kind(Some(Waiting), Some(Running)), None);
        assert_eq!(transition_kind(None, Some(Running)), None);
        assert_eq!(transition_kind(Some(Idle), Some(Running)), None);
        assert_eq!(transition_kind(Some(Waiting), Some(Waiting)), None);
    }

    #[test]
    fn session_statuses_keys_and_filters() {
        use AgentStatus::*;
        let sessions = vec![
            sess(Some("a"), Waiting, false),
            sess(Some("b"), RateLimited, false),
            sess(None, Running, true), // inferred file-activity placeholder — excluded
            sess(None, Running, false),
        ];
        let got = session_statuses(7, &sessions);
        assert_eq!(got.len(), 3);
        assert!(got.contains(&((7, SessKey::Transcript("a".into())), Waiting)));
        assert!(got.contains(&((7, SessKey::Transcript("b".into())), RateLimited)));
        assert!(got.contains(&((7, SessKey::Fallback), Running)));

        // Defensive: a duplicate key keeps the higher-priority status.
        let dup = vec![sess(None, Idle, false), sess(None, Waiting, false)];
        assert_eq!(
            session_statuses(7, &dup),
            vec![((7, SessKey::Fallback), Waiting)]
        );
    }

    #[test]
    fn two_sessions_fire_independent_streams() {
        use AgentStatus::*;
        let k = |id: &str| (1, SessKey::Transcript(id.into()));
        let live: HashSet<LaneId> = [1].into();
        let managed = HashSet::new();

        // One agent finishes its turn while its lane-mate is still rate-limited. The old
        // per-lane rollup saw "RateLimited" before and after and fired nothing — the masking
        // this change exists to fix.
        let prev: HashMap<_, _> = [(k("a"), Running), (k("b"), RateLimited)].into();
        let now: HashMap<_, _> = [(k("a"), Waiting), (k("b"), RateLimited)].into();
        assert_eq!(
            diff_session_transitions(&prev, &now, &live, &managed),
            vec![(k("a"), NotifKind::NeedsYou)]
        );

        // And the rate-limited lane-mate resumes independently.
        let now2: HashMap<_, _> = [(k("a"), Waiting), (k("b"), Running)].into();
        assert_eq!(
            diff_session_transitions(&now, &now2, &live, &managed),
            vec![(k("b"), NotifKind::Resumed)]
        );
    }

    #[test]
    fn disappearance_fires_idle_only_when_lane_lives() {
        use AgentStatus::*;
        let k = (1, SessKey::Transcript("a".into()));
        let prev: HashMap<_, _> = [(k.clone(), Waiting)].into();
        let now = HashMap::new();
        let managed = HashSet::new();

        let live: HashSet<LaneId> = [1].into();
        assert_eq!(
            diff_session_transitions(&prev, &now, &live, &managed),
            vec![(k, NotifKind::Idle)]
        );
        // The whole lane went away (deleted): not an agent going quiet.
        assert!(diff_session_transitions(&prev, &now, &HashSet::new(), &managed).is_empty());
    }

    #[test]
    fn fallback_handoff_does_not_fire_idle() {
        use AgentStatus::*;
        let live: HashSet<LaneId> = [1].into();
        let prev: HashMap<_, _> = [((1, SessKey::Fallback), Running)].into();
        let now: HashMap<_, _> = [((1, SessKey::Transcript("a".into())), Running)].into();

        // The managed spawn's transcript became parseable: Fallback hands off to Transcript
        // within one refresh. The agent didn't stop, so nothing fires.
        let managed: HashSet<LaneId> = [1].into();
        assert!(diff_session_transitions(&prev, &now, &live, &managed).is_empty());

        // But with no managed session left in the lane, a vanished fallback is a real stop.
        let gone = HashMap::new();
        assert_eq!(
            diff_session_transitions(&prev, &gone, &live, &HashSet::new()),
            vec![((1, SessKey::Fallback), NotifKind::Idle)]
        );
    }

    #[test]
    fn new_session_already_waiting_fires_needs_you() {
        use AgentStatus::*;
        let live: HashSet<LaneId> = [1].into();
        let managed = HashSet::new();
        let prev = HashMap::new();
        let k = (1, SessKey::Transcript("a".into()));

        // A second agent appearing mid-run already waiting (e.g. a parallel session that
        // finished between refreshes) is exactly the alert the user wants.
        let waiting: HashMap<_, _> = [(k.clone(), Waiting)].into();
        assert_eq!(
            diff_session_transitions(&prev, &waiting, &live, &managed),
            vec![(k.clone(), NotifKind::NeedsYou)]
        );
        // Appearing already-running is just work starting; stay quiet.
        let running: HashMap<_, _> = [(k, Running)].into();
        assert!(diff_session_transitions(&prev, &running, &live, &managed).is_empty());
    }

    #[test]
    fn compose_shows_the_why_and_the_slot() {
        let mut s = sess(Some("abc"), AgentStatus::Waiting, false);
        s.title = Some("build the parser".into());
        s.last_message = Some("Should I also update the integration tests?".into());
        s.tool_call_count = 3;
        let (title, body) = compose(NotifKind::NeedsYou, &lane(), Some(&s), Some((1, 3)), true);
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
        let mut s = sess(Some("abc"), AgentStatus::Waiting, false);
        s.title = Some("build the parser".into());
        s.last_message = Some("Should I also update the integration tests?".into());
        let (_, body) = compose(NotifKind::NeedsYou, &lane(), Some(&s), Some((0, 1)), false);
        assert!(body.contains("build the parser"), "{body}");
        assert!(!body.contains("agent 1/1"), "{body}");
    }

    #[test]
    fn slot_by_key_indexes_real_sessions() {
        let mut l = lane();
        l.agent_sessions = vec![
            sess(Some("a"), AgentStatus::Running, false),
            sess(None, AgentStatus::Running, true), // inferred — not a slot
            sess(Some("b"), AgentStatus::Waiting, false),
        ];
        assert_eq!(
            slot_by_key(&l, &SessKey::Transcript("b".into())),
            Some((1, 2))
        );
        assert_eq!(slot_by_key(&l, &SessKey::Transcript("zz".into())), None);
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

    #[test]
    fn truncate_adds_ellipsis_only_when_over() {
        assert_eq!(truncate("short", 60), "short");
        let t = truncate("0123456789", 5);
        assert_eq!(t.chars().count(), 5);
        assert!(t.ends_with('…'));
        assert_eq!(t, "0123…");
    }

    #[test]
    fn kind_serializes_snake_case() {
        assert_eq!(
            serde_json::to_string(&NotifKind::NeedsYou).unwrap(),
            "\"needs_you\""
        );
        assert_eq!(
            serde_json::to_string(&NotifKind::RateLimited).unwrap(),
            "\"rate_limited\""
        );
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

    #[cfg(target_os = "macos")]
    #[test]
    fn escape_neutralizes_applescript_quotes() {
        assert_eq!(escape(r#"say "hi" \ now"#), r#"say \"hi\" \\ now"#);
    }
}
