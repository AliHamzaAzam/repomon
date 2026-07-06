//! Writing to the system clipboard, shared by the TUI and the tmux copy binding.
//!
//! The tool is probed rather than `cfg`-gated: one testable selection path covers pbcopy on
//! macOS, wl-copy/xclip on Linux, and shims wherever they appear.

use std::process::{Command, Stdio};
use std::sync::OnceLock;

/// The clipboard-write command for this platform, probed once per process. `None` when no
/// clipboard tool is installed (callers fall back to tmux's OSC52 path or skip the copy).
pub fn copy_argv() -> Option<&'static [String]> {
    static ARGV: OnceLock<Option<Vec<String>>> = OnceLock::new();
    ARGV.get_or_init(|| {
        let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty());
        select_copy_argv(wayland, |bin| crate::exec::find_in_path(bin).is_some())
    })
    .as_deref()
}

/// Preference order: pbcopy (macOS), then the display server's native tool — wl-copy under
/// Wayland, xclip under X11 — then wl-copy as a last resort (covers Wayland sessions where
/// `$WAYLAND_DISPLAY` isn't exported to the daemon).
fn select_copy_argv(wayland: bool, has: impl Fn(&str) -> bool) -> Option<Vec<String>> {
    let owned = |args: &[&str]| args.iter().map(|s| s.to_string()).collect();
    if has("pbcopy") {
        return Some(owned(&["pbcopy"]));
    }
    if wayland && has("wl-copy") {
        return Some(owned(&["wl-copy"]));
    }
    if has("xclip") {
        return Some(owned(&["xclip", "-selection", "clipboard", "-in"]));
    }
    if has("wl-copy") {
        return Some(owned(&["wl-copy"]));
    }
    None
}

/// The copy command as one string, for tmux's `copy-pipe-and-cancel` (argv parts are bare
/// tool names and flags, so space-joining is quoting-safe).
pub fn copy_pipe_command() -> Option<String> {
    copy_argv().map(|argv| argv.join(" "))
}

/// Pipe `text` into the platform clipboard tool, best-effort.
pub fn copy_text(text: &str) {
    use std::io::Write;
    let Some(argv) = copy_argv() else { return };
    if let Ok(mut child) = Command::new(&argv[0])
        .args(&argv[1..])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
    {
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(text.as_bytes());
        }
        let _ = child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefers_pbcopy_when_present() {
        let argv = select_copy_argv(true, |b| b == "pbcopy" || b == "wl-copy").unwrap();
        assert_eq!(argv, ["pbcopy"]);
    }

    #[test]
    fn wayland_picks_wl_copy_over_xclip() {
        let argv = select_copy_argv(true, |b| b == "wl-copy" || b == "xclip").unwrap();
        assert_eq!(argv, ["wl-copy"]);
    }

    #[test]
    fn x11_picks_xclip_with_clipboard_selection() {
        let argv = select_copy_argv(false, |b| b == "xclip").unwrap();
        assert_eq!(argv, ["xclip", "-selection", "clipboard", "-in"]);
    }

    #[test]
    fn wl_copy_is_the_last_resort_without_wayland() {
        let argv = select_copy_argv(false, |b| b == "wl-copy").unwrap();
        assert_eq!(argv, ["wl-copy"]);
    }

    #[test]
    fn none_when_no_tool_exists() {
        assert_eq!(select_copy_argv(false, |_| false), None);
    }
}
