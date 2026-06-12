//! Desktop + in-app notification *delivery* for agent state changes.
//!
//! The TUI watches each agent session's status across refreshes (see
//! `App::detect_notifications`) and, on a meaningful transition, delivers a notification here:
//! a native macOS popup (terminal-notifier/`osascript`, fired on a short-lived thread so it
//! never blocks the event loop) plus an in-app banner and a scrollable history feed. The pure
//! parts — kinds, edge detection, text composition — live in `repomon_core::notify`, shared
//! with the daemon's remote engine; the config toggles live in `app`.

use chrono::{DateTime, Local};

use repomon_core::model::LaneId;

// The pure heart (kinds, edge detection, text composition) lives in core, shared with the
// daemon's remote notification engine; this module keeps the local delivery.
pub use repomon_core::notify::{compose, compose_burst, NotifKind};

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
