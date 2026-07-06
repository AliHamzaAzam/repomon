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

use chrono::{DateTime, Local, Utc};
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
    /// Agent looks stuck: process alive, no dialog up, turn not ended — and neither the pane
    /// nor the transcript has moved for the stall window. Gated by the needs-you toggle (a
    /// stall is a needs-you-class event, not a new setting).
    Stalled,
}

impl NotifKind {
    pub fn glyph(self) -> &'static str {
        match self {
            NotifKind::NeedsYou => "⏸",
            NotifKind::RateLimited => "⏳",
            NotifKind::Resumed => "▶",
            NotifKind::Idle => "○",
            NotifKind::Stalled => "⚠",
        }
    }
    /// The stable snake_case token for this kind (matches the serde wire name). Used to build a
    /// notification's dedup id so a flapped re-send carries the same id and clients can drop it.
    pub fn slug(self) -> &'static str {
        match self {
            NotifKind::NeedsYou => "needs_you",
            NotifKind::RateLimited => "rate_limited",
            NotifKind::Resumed => "resumed",
            NotifKind::Idle => "idle",
            NotifKind::Stalled => "stalled",
        }
    }
    fn verb(self) -> &'static str {
        match self {
            NotifKind::NeedsYou => "needs you",
            NotifKind::RateLimited => "hit a usage limit",
            NotifKind::Resumed => "resumed",
            NotifKind::Idle => "went idle",
            NotifKind::Stalled => "looks stuck",
        }
    }
    fn verb_plural(self) -> &'static str {
        match self {
            NotifKind::NeedsYou => "need you",
            NotifKind::RateLimited => "hit a usage limit",
            NotifKind::Resumed => "resumed",
            NotifKind::Idle => "went idle",
            NotifKind::Stalled => "look stuck",
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

/// A session's notification-relevant state in one snapshot: its status plus the stall flag.
/// The pair is what the edge detectors diff across polls — a stall flip alerts even though
/// the status underneath (Running/Idle) never changes.
pub type SessState = (AgentStatus, bool);

/// Key/state pairs for one lane's *real* agent sessions, used to drive notifications.
///
/// `inferred` "file-activity" sessions are worktree-isolated subagents (a Claude Code subagent
/// runs inside its parent's process and leaves no transcript or process of its own). They are
/// dropped unless `include_subagents` is set — the `notify_subagents` toggle, off by default, so
/// the user is alerted only when the *main* agent finishes, not each subagent it spawns. On a
/// (theoretically impossible) duplicate key, the higher-priority status wins — the same order the
/// old per-lane rollup used — and the stall flag is OR-merged.
pub fn session_statuses(
    lane_id: LaneId,
    sessions: &[AgentSession],
    include_subagents: bool,
) -> Vec<((LaneId, SessKey), SessState)> {
    let mut out: Vec<((LaneId, SessKey), SessState)> = Vec::new();
    for s in sessions.iter().filter(|s| include_subagents || !s.inferred) {
        let key = (
            lane_id,
            s.session_id
                .clone()
                .map(SessKey::Transcript)
                .unwrap_or(SessKey::Fallback),
        );
        match out.iter_mut().find(|(k, _)| *k == key) {
            Some((_, st)) => {
                if status_priority(s.status) < status_priority(st.0) {
                    st.0 = s.status;
                }
                st.1 |= s.stale;
            }
            None => out.push((key, (s.status, s.stale))),
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
    prev: &HashMap<(LaneId, SessKey), SessState>,
    now: &HashMap<(LaneId, SessKey), SessState>,
    live_lanes: &HashSet<LaneId>,
    lanes_with_managed: &HashSet<LaneId>,
) -> Vec<((LaneId, SessKey), NotifKind)> {
    let mut out = Vec::new();
    for (key, &(status, stale)) in now {
        let was = prev.get(key).copied();
        if was == Some((status, stale)) {
            continue;
        }
        // The stall flag flipping on is its own alert, independent of the status underneath
        // (which typically never changes — that's what makes a stall invisible otherwise).
        // Un-stalling stays quiet: output resuming is the good case.
        if stale && !was.is_some_and(|(_, st)| st) {
            out.push((key.clone(), NotifKind::Stalled));
            continue;
        }
        if let Some(kind) = transition_kind(was.map(|(s, _)| s), Some(status)) {
            out.push((key.clone(), kind));
        }
    }
    for (key, &(was, _)) in prev {
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
/// `None` when the session vanished (i.e. its disappearance was the trigger). `include_subagents`
/// must match the value passed to [`session_statuses`] so an inferred-subagent key resolves.
pub fn session_by_key<'a>(
    lane: &'a Lane,
    key: &SessKey,
    include_subagents: bool,
) -> Option<&'a AgentSession> {
    lane.agent_sessions
        .iter()
        .filter(|s| include_subagents || !s.inferred)
        .find(|s| match key {
            SessKey::Transcript(id) => s.session_id.as_deref() == Some(id.as_str()),
            SessKey::Fallback => s.session_id.is_none(),
        })
}

/// Which of the lane's real sessions `key` resolves to: `(index, count)`, for the
/// "agent 2/3" tag in multi-agent lanes. `None` when the session vanished. `include_subagents`
/// must match the value passed to [`session_statuses`].
pub fn slot_by_key(lane: &Lane, key: &SessKey, include_subagents: bool) -> Option<(usize, usize)> {
    let real: Vec<&AgentSession> = lane
        .agent_sessions
        .iter()
        .filter(|s| include_subagents || !s.inferred)
        .collect();
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
        // The session actually ended — its tmux window/process is gone (`None`) or the transcript
        // closed (`Ended`). Fires regardless of the status it last held (it may have decayed to
        // Idle first), so a real stop is reported promptly. A still-present session merely *decaying*
        // to `Idle` after IDLE_AFTER is intentionally NOT alerted: that popup is ~10 minutes stale by
        // construction (the decay is a 10-min-old event), which produced bursts of stale "went idle"
        // alerts. The status still decays for the UI; only the notification is suppressed.
        (Some(_), None) => Some(NotifKind::Idle),
        (Some(p), Some(Ended)) if p != Ended => Some(NotifKind::Idle),
        _ => None,
    }
}

/// Whether an alert for a session may fire again, anchored on the session's transcript activity
/// rather than on elapsed time.
///
/// The status signal a notification is derived from flaps: a frozen-but-waiting transcript decays
/// `Waiting → Idle` at the 10-minute mark and flips back on the next byte; the `lsof` live-process
/// probe undercounts and drops then re-includes a session; the pane sniff (and usage-limit sniff)
/// are screen-scrapes that read `Some → None → Some`. Every such round-trip re-detects a transition
/// and, since the only other guard is a 30s time-debounce, re-fires the *same* alert minutes or
/// hours later. [`AgentSession::last_activity_at`](crate::model::AgentSession::last_activity_at) —
/// the latest transcript *message* timestamp (not the raw file mtime — Claude bumps that by
/// rewriting trailer metadata) — advances **only on real agent output**, never on those flaps, so
/// it is the right thing to gate a repeat on: re-fire only when the agent has actually done new
/// work since it last alerted (the user replied and it ran, then waited again), not when detection
/// merely wobbled. Caller keeps a per-`(lane, session, kind)` record of the activity timestamp at
/// the last fire and passes it as `prev_fired_at`.
///
/// Used for `NeedsYou` / `RateLimited` / `Resumed`, whose session is present in the snapshot when
/// they fire (so `current_activity` is `Some`). `Idle` fires on disappearance — no activity anchor
/// — and stays on the time-debounce.
pub fn activity_allows_refire(
    prev_fired_at: Option<DateTime<Utc>>,
    current_activity: Option<DateTime<Utc>>,
) -> bool {
    match (prev_fired_at, current_activity) {
        (None, _) => true, // never fired this (lane, session, kind) — let it through
        (Some(_), None) => false, // fired before and no fresh anchor to justify a repeat
        (Some(p), Some(c)) => c > p, // only when the transcript advanced since the last fire
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
    // A NeedsYou names what the agent actually wants — permission, an answer, the review of
    // finished work, or just the next instruction — so the reader can triage from the
    // notification alone. Falls back to the generic verb when the session vanished (or isn't
    // in a waiting state after all).
    let verb = match (kind, sess) {
        (NotifKind::NeedsYou, Some(s)) => {
            use crate::agent::attention::{Attention, agent_attention_in};
            match agent_attention_in(lane, s) {
                Attention::Permission => "is asking permission",
                Attention::Decision => "has a question",
                Attention::DoneCandidate => "is ready for review",
                Attention::EndOfTurn => "finished its turn",
                Attention::None => kind.verb(),
            }
        }
        _ => kind.verb(),
    };
    let title = format!("{} {} {} — {}", kind.glyph(), agent, verb, lane.repo.name);

    let mut parts = vec![
        lane.state
            .branch
            .clone()
            .unwrap_or_else(|| lane.worktree.name.clone()),
    ];
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
    if kind == NotifKind::Stalled {
        if let Some(since) = sess.and_then(|s| s.stalled_since) {
            let mins = (Utc::now() - since).num_minutes().max(0);
            parts.push(format!("stalled {mins}m"));
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
    #[cfg(not(target_os = "macos"))]
    std::thread::spawn(play_sound_blocking);
}

/// Fire a native desktop notification, best-effort and without blocking the caller (the actual
/// `osascript`/`notify-send` invocation runs on a detached thread). `click_focus` makes the popup
/// click-to-focus the terminal when `terminal-notifier` is installed (macOS only — on Linux
/// there is no portable way to focus a terminal from a background process, so it's ignored).
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

/// Deliver via `notify-send` (libnotify/DBus). The `sound-name` hint covers desktops that honor
/// it; the local player in [`play_sound_blocking`] covers the rest. Both are best-effort no-ops
/// on headless boxes.
#[cfg(not(target_os = "macos"))]
fn run_native(title: &str, body: &str, sound: bool, _click_focus: bool) {
    let _ = std::process::Command::new("notify-send")
        .args(notify_send_args(title, body, sound))
        .output();
    if sound {
        play_sound_blocking();
    }
}

/// Arguments for `notify-send`: app name, normal urgency, the freedesktop sound hint when
/// `sound` is on, and title/body last behind `--` so they can never parse as flags. Pure and
/// compiled everywhere so the shape is tested on every platform.
pub fn notify_send_args(title: &str, body: &str, sound: bool) -> Vec<String> {
    let mut args: Vec<String> = ["-a", "repomon", "-u", "normal"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    if sound {
        args.push("-h".into());
        args.push("string:sound-name:message-new-instant".into());
    }
    args.push("--".into());
    args.push(title.to_string());
    args.push(body.to_string());
    args
}

/// The local chime player, probed once per process: canberra (plays the themed event sound),
/// else `paplay` with a freedesktop sound file that exists. `None` = no way to play a sound.
#[cfg(not(target_os = "macos"))]
fn sound_argv() -> Option<&'static [String]> {
    fn locate() -> Option<Vec<String>> {
        let owned = |args: &[&str]| args.iter().map(|s| s.to_string()).collect();
        if crate::exec::find_in_path("canberra-gtk-play").is_some() {
            return Some(owned(&["canberra-gtk-play", "-i", "message-new-instant"]));
        }
        if crate::exec::find_in_path("paplay").is_some() {
            for file in [
                "/usr/share/sounds/freedesktop/stereo/message-new-instant.oga",
                "/usr/share/sounds/freedesktop/stereo/message.oga",
            ] {
                if std::path::Path::new(file).exists() {
                    return Some(owned(&["paplay", file]));
                }
            }
        }
        None
    }
    static ARGV: std::sync::OnceLock<Option<Vec<String>>> = std::sync::OnceLock::new();
    ARGV.get_or_init(locate).as_deref()
}

/// Play the notification chime through whatever player this box has, if any.
#[cfg(not(target_os = "macos"))]
fn play_sound_blocking() {
    if let Some(argv) = sound_argv() {
        let _ = std::process::Command::new(&argv[0])
            .args(&argv[1..])
            .output();
    }
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

    #[test]
    fn notify_send_args_carry_app_urgency_and_sound_hint() {
        let args = notify_send_args("Title", "Body", true);
        assert_eq!(&args[..4], ["-a", "repomon", "-u", "normal"]);
        assert!(args.contains(&"string:sound-name:message-new-instant".to_string()));
        assert_eq!(&args[args.len() - 3..], ["--", "Title", "Body"]);
    }

    #[test]
    fn notify_send_args_skip_the_hint_when_silent() {
        let args = notify_send_args("T", "B", false);
        assert!(!args.iter().any(|a| a.contains("sound-name")));
        assert_eq!(&args[args.len() - 3..], ["--", "T", "B"]);
    }

    /// A snapshot state with the stall flag off — the common case in transition tests.
    fn st(s: AgentStatus) -> SessState {
        (s, false)
    }

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
            pending_dialog: None,
            stale: false,
            stalled_since: None,
            ended_turn: false,
            gate: None,
            config_dir: None,
            custom_label: None,
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
        // Ended: the session went away (window/process gone) — a real stop, alerted promptly,
        // whatever it was doing just before (including after it had decayed to Idle).
        assert_eq!(transition_kind(Some(Waiting), None), Some(NotifKind::Idle));
        assert_eq!(transition_kind(Some(Running), None), Some(NotifKind::Idle));
        assert_eq!(transition_kind(Some(Idle), None), Some(NotifKind::Idle));
        // The bare 10-minute inactivity decay (still present, just `Idle` now) is NOT an alert —
        // it would be ~10 min stale. This is the fix for the bursts of old "went idle" popups.
        assert_eq!(transition_kind(Some(Running), Some(Idle)), None);
        assert_eq!(transition_kind(Some(Waiting), Some(Idle)), None);
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
        let got = session_statuses(7, &sessions, false);
        assert_eq!(got.len(), 3);
        assert!(got.contains(&((7, SessKey::Transcript("a".into())), st(Waiting))));
        assert!(got.contains(&((7, SessKey::Transcript("b".into())), st(RateLimited))));
        assert!(got.contains(&((7, SessKey::Fallback), st(Running))));

        // Defensive: a duplicate key keeps the higher-priority status (and ORs the stall flag).
        let mut stalled = sess(None, Idle, false);
        stalled.stale = true;
        let dup = vec![stalled, sess(None, Waiting, false)];
        assert_eq!(
            session_statuses(7, &dup, false),
            vec![((7, SessKey::Fallback), (Waiting, true))]
        );
    }

    #[test]
    fn subagents_excluded_by_default_included_when_opted_in() {
        use AgentStatus::*;
        // A lane with only an inferred worktree-isolated subagent (no transcript/process), the
        // shape `overlay_agents` produces for a Claude Code subagent.
        let sessions = vec![sess(None, Running, true)];

        // Default: subagents never drive notifications — the inferred session is dropped, so it
        // can't fire an Idle when it finishes.
        assert!(session_statuses(7, &sessions, false).is_empty());

        // Opted in (`notify_subagents = true`): the subagent surfaces as a Fallback session so
        // its finish (→ disappearance) can alert.
        assert_eq!(
            session_statuses(7, &sessions, true),
            vec![((7, SessKey::Fallback), st(Running))]
        );

        // A main (transcript-backed) agent alongside a subagent: the main always counts; the
        // subagent only when opted in.
        let mixed = vec![
            sess(Some("main"), Waiting, false),
            sess(None, Running, true),
        ];
        assert_eq!(session_statuses(7, &mixed, false).len(), 1);
        assert_eq!(session_statuses(7, &mixed, true).len(), 2);
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
        let prev: HashMap<_, _> = [(k("a"), st(Running)), (k("b"), st(RateLimited))].into();
        let now: HashMap<_, _> = [(k("a"), st(Waiting)), (k("b"), st(RateLimited))].into();
        assert_eq!(
            diff_session_transitions(&prev, &now, &live, &managed),
            vec![(k("a"), NotifKind::NeedsYou)]
        );

        // And the rate-limited lane-mate resumes independently.
        let now2: HashMap<_, _> = [(k("a"), st(Waiting)), (k("b"), st(Running))].into();
        assert_eq!(
            diff_session_transitions(&now, &now2, &live, &managed),
            vec![(k("b"), NotifKind::Resumed)]
        );
    }

    #[test]
    fn disappearance_fires_idle_only_when_lane_lives() {
        use AgentStatus::*;
        let k = (1, SessKey::Transcript("a".into()));
        let prev: HashMap<_, _> = [(k.clone(), st(Waiting))].into();
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
        let prev: HashMap<_, _> = [((1, SessKey::Fallback), st(Running))].into();
        let now: HashMap<_, _> = [((1, SessKey::Transcript("a".into())), st(Running))].into();

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
        let waiting: HashMap<_, _> = [(k.clone(), st(Waiting))].into();
        assert_eq!(
            diff_session_transitions(&prev, &waiting, &live, &managed),
            vec![(k.clone(), NotifKind::NeedsYou)]
        );
        // Appearing already-running is just work starting; stay quiet.
        let running: HashMap<_, _> = [(k, st(Running))].into();
        assert!(diff_session_transitions(&prev, &running, &live, &managed).is_empty());
    }

    #[test]
    fn subagent_finishing_fires_idle_only_when_included() {
        use AgentStatus::*;
        let live: HashSet<LaneId> = [7].into();
        let managed = HashSet::new();

        // A lane whose only agent is an inferred worktree-isolated subagent, running then gone.
        let running = vec![sess(None, Running, true)];
        let gone: Vec<AgentSession> = vec![];

        // Default (subagents excluded): the subagent never enters the tracked set, so its finish
        // is invisible — no Idle.
        let prev: HashMap<_, _> = session_statuses(7, &running, false).into_iter().collect();
        let now: HashMap<_, _> = session_statuses(7, &gone, false).into_iter().collect();
        assert!(prev.is_empty());
        assert!(diff_session_transitions(&prev, &now, &live, &managed).is_empty());

        // Opted in: the subagent is tracked, so its disappearance fires Idle (it was active).
        let prev: HashMap<_, _> = session_statuses(7, &running, true).into_iter().collect();
        let now: HashMap<_, _> = session_statuses(7, &gone, true).into_iter().collect();
        assert_eq!(
            diff_session_transitions(&prev, &now, &live, &managed),
            vec![((7, SessKey::Fallback), NotifKind::Idle)]
        );
    }

    #[test]
    fn stall_flip_fires_stalled_once() {
        use AgentStatus::*;
        let live: HashSet<LaneId> = [1].into();
        let managed = HashSet::new();
        let k = (1, SessKey::Transcript("a".into()));

        // Running → running-but-stale: the flip alerts even though the status never changed.
        let prev: HashMap<_, _> = [(k.clone(), (Running, false))].into();
        let now: HashMap<_, _> = [(k.clone(), (Running, true))].into();
        assert_eq!(
            diff_session_transitions(&prev, &now, &live, &managed),
            vec![(k.clone(), NotifKind::Stalled)]
        );
        // Still stale on the next poll: no re-fire.
        assert!(diff_session_transitions(&now, &now, &live, &managed).is_empty());
        // The Running→Idle decay while the stall persists must not alert either.
        let idle: HashMap<_, _> = [(k.clone(), (Idle, true))].into();
        assert!(diff_session_transitions(&now, &idle, &live, &managed).is_empty());
        // Un-stalling quietly (output resumed): no alert.
        let back: HashMap<_, _> = [(k.clone(), (Running, false))].into();
        assert!(diff_session_transitions(&idle, &back, &live, &managed).is_empty());
        // A session first seen already-stale alerts (it stalled between polls).
        assert_eq!(
            diff_session_transitions(&HashMap::new(), &now, &live, &managed),
            vec![(k, NotifKind::Stalled)]
        );
    }

    #[test]
    fn compose_stalled_and_done_candidate_wording() {
        // Stalled: names the stall and how long the pane has been frozen.
        let mut s = sess(Some("a"), AgentStatus::Running, false);
        s.stale = true;
        s.stalled_since = Some(Utc::now() - chrono::Duration::minutes(7));
        let (title, body) = compose(NotifKind::Stalled, &lane(), Some(&s), None, true);
        assert!(title.contains("looks stuck"), "{title}");
        assert!(body.contains("stalled 7m"), "{body}");

        // Done candidate: end-of-turn on a clean lane with a fresh commit reads as
        // ready-for-review instead of the bare finished-turn wording.
        let s = sess(Some("a"), AgentStatus::Waiting, false);
        let mut l = lane();
        l.state.last_commit_at = Some(Utc::now());
        let (title, _) = compose(NotifKind::NeedsYou, &l, Some(&s), None, true);
        assert!(title.contains("is ready for review"), "{title}");
    }

    #[test]
    fn reseed_snapshot_fires_nothing() {
        use AgentStatus::*;
        // Re-seeding (what the TUI does on attach-return / toggle flip, and the daemon on
        // re-enable) sets prev = now; diffing an unchanged snapshot must produce no alerts, which
        // is what stops the daemon-covered backlog from double-firing.
        let live: HashSet<LaneId> = [1].into();
        let managed = HashSet::new();
        let k = |id: &str| (1, SessKey::Transcript(id.into()));
        let snap: HashMap<_, _> = [(k("a"), st(Waiting)), (k("b"), st(RateLimited))].into();
        assert!(diff_session_transitions(&snap, &snap, &live, &managed).is_empty());
    }

    #[test]
    fn activity_latch_gates_repeats_on_real_work() {
        let t0 = Utc::now();
        let t1 = t0 + chrono::Duration::minutes(5);

        // First time this (lane, session, kind) is seen: always fires.
        assert!(activity_allows_refire(None, Some(t0)));

        // Already fired and the transcript hasn't advanced — the flap cases the latch exists to
        // kill: an idle-decayed Waiting returning to Waiting, an lsof undercount dropping then
        // re-adding the session, a sniff reading Some→None→Some. All share the same last_activity.
        assert!(!activity_allows_refire(Some(t0), Some(t0)));

        // Fired before, but the session vanished (no current anchor): suppress the repeat rather
        // than re-firing off a presence flap.
        assert!(!activity_allows_refire(Some(t0), None));

        // Genuine new work since the last alert (user replied, the agent ran and waited again):
        // the transcript advanced, so re-fire.
        assert!(activity_allows_refire(Some(t0), Some(t1)));
    }

    #[test]
    fn compose_shows_the_why_and_the_slot() {
        let mut s = sess(Some("abc"), AgentStatus::Waiting, false);
        s.title = Some("build the parser".into());
        s.last_message = Some("Should I also update the integration tests?".into());
        s.tool_call_count = 3;
        let (title, body) = compose(NotifKind::NeedsYou, &lane(), Some(&s), Some((1, 3)), true);
        // A Waiting session with no open dialog reads as a finished turn, not a generic ask.
        assert!(
            title.contains("finished its turn") && title.contains("alpha"),
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
    fn compose_names_the_attention_for_needs_you() {
        let mut s = sess(Some("abc"), AgentStatus::Waiting, false);

        s.pending_prompt = Some("Bash command — Do you want to proceed?".into());
        let (title, _) = compose(NotifKind::NeedsYou, &lane(), Some(&s), None, true);
        assert!(title.contains("is asking permission"), "{title}");

        s.pending_prompt = Some("Which auth method should we use?".into());
        let (title, _) = compose(NotifKind::NeedsYou, &lane(), Some(&s), None, true);
        assert!(title.contains("has a question"), "{title}");

        s.pending_prompt = None;
        let (title, _) = compose(NotifKind::NeedsYou, &lane(), Some(&s), None, true);
        assert!(title.contains("finished its turn"), "{title}");

        // The session vanished from the snapshot — fall back to the generic wording.
        let (title, _) = compose(NotifKind::NeedsYou, &lane(), None, None, true);
        assert!(title.contains("needs you"), "{title}");

        // Other kinds keep their own verbs regardless of the session.
        let (title, _) = compose(NotifKind::RateLimited, &lane(), Some(&s), None, true);
        assert!(title.contains("hit a usage limit"), "{title}");
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
            slot_by_key(&l, &SessKey::Transcript("b".into()), false),
            Some((1, 2))
        );
        assert_eq!(
            slot_by_key(&l, &SessKey::Transcript("zz".into()), false),
            None
        );
        // With subagents included, the inferred session occupies a slot too (now 3 real).
        assert_eq!(
            slot_by_key(&l, &SessKey::Transcript("b".into()), true),
            Some((2, 3))
        );
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
        // slug() must stay in lock-step with the serde wire name (it keys the dedup id).
        for k in [
            NotifKind::NeedsYou,
            NotifKind::RateLimited,
            NotifKind::Resumed,
            NotifKind::Idle,
        ] {
            assert_eq!(
                serde_json::to_string(&k).unwrap(),
                format!("\"{}\"", k.slug())
            );
        }
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
