//! Writing to the system clipboard, shared by the TUI and the tmux copy binding.
//!
//! On Unix the tool is probed rather than `cfg`-gated: one testable selection path covers
//! pbcopy on macOS, wl-copy/xclip on Linux, and shims wherever they appear. On Windows the
//! clipboard is reached through Windows PowerShell (`Set-Clipboard`/`Get-Clipboard`); the
//! argv/script builders are pure and unit-tested on every OS.

use std::process::{Command, Stdio};
use std::sync::OnceLock;

/// The clipboard-write command for this platform, probed once per process. `None` when no
/// clipboard tool is installed (callers fall back to tmux's OSC52 path or skip the copy).
pub fn copy_argv() -> Option<&'static [String]> {
    static ARGV: OnceLock<Option<Vec<String>>> = OnceLock::new();
    ARGV.get_or_init(platform_copy_argv).as_deref()
}

/// Unix: probe for the display server's native tool.
#[cfg(not(windows))]
fn platform_copy_argv() -> Option<Vec<String>> {
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty());
    select_copy_argv(wayland, |bin| crate::exec::find_in_path(bin).is_some())
}

/// Windows: PowerShell ships with the OS, so there is nothing to probe.
#[cfg(windows)]
fn platform_copy_argv() -> Option<Vec<String>> {
    Some(windows_copy_argv())
}

/// Preference order: pbcopy (macOS), then the display server's native tool — wl-copy under
/// Wayland, xclip under X11 — then wl-copy as a last resort (covers Wayland sessions where
/// `$WAYLAND_DISPLAY` isn't exported to the daemon).
#[cfg(any(not(windows), test))]
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
/// tool names and flags, so space-joining is quoting-safe). `None` on Windows: tmux is
/// Unix-only, and the PowerShell argv contains scripts that are not space-join-safe.
pub fn copy_pipe_command() -> Option<String> {
    #[cfg(windows)]
    return None;
    #[cfg(not(windows))]
    copy_argv().map(|argv| argv.join(" "))
}

/// Pipe `text` into the platform clipboard tool, best-effort.
pub fn copy_text(text: &str) {
    use std::io::Write;
    let Some(argv) = copy_argv() else { return };
    let mut cmd = Command::new(&argv[0]);
    cmd.args(&argv[1..])
        .stdin(Stdio::piped())
        .stdout(Stdio::null());
    // PowerShell reports failures on stderr; keep them off the TUI's screen.
    #[cfg(windows)]
    cmd.stderr(Stdio::null());
    if let Ok(mut child) = cmd.spawn() {
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(text.as_bytes());
        }
        let _ = child.wait();
    }
}

/// The PowerShell invocation wrapper for Windows clipboard access. Always Windows
/// PowerShell 5.1 (`powershell`, present on every supported Windows), never `pwsh`:
/// PowerShell 7 dropped `Get-Clipboard -Format Image`, which the image-paste path needs.
pub fn windows_powershell_argv(script: &str) -> Vec<String> {
    [
        "powershell",
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        script,
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// Copy script: read the raw stdin bytes and decode them as UTF-8 in the script itself, so
/// the console input codepage never touches the text. This is why clip.exe (and a plain
/// `$input | Set-Clipboard`) are avoided — both run redirected stdin through the OEM
/// codepage and mangle anything outside it.
const WINDOWS_COPY_SCRIPT: &str = "$s=[Console]::OpenStandardInput();\
$m=New-Object System.IO.MemoryStream;$s.CopyTo($m);\
Set-Clipboard -Value ([System.Text.Encoding]::UTF8.GetString($m.ToArray()))";

/// The Windows clipboard-write command: UTF-8 text on stdin, `Set-Clipboard` at the end.
pub fn windows_copy_argv() -> Vec<String> {
    windows_powershell_argv(WINDOWS_COPY_SCRIPT)
}

/// Paste script: redirected stdout defaults to the OEM codepage on Windows PowerShell 5.1,
/// so force UTF-8 before `Get-Clipboard -Raw` writes anything.
const WINDOWS_PASTE_SCRIPT: &str =
    "[Console]::OutputEncoding=[System.Text.Encoding]::UTF8;Get-Clipboard -Raw";

/// The Windows clipboard-read command: UTF-8 clipboard text on stdout.
pub fn windows_paste_argv() -> Vec<String> {
    windows_powershell_argv(WINDOWS_PASTE_SCRIPT)
}

/// Normalize `Get-Clipboard -Raw` output for a Unix-lineage consumer: drop a stray UTF-8
/// BOM, convert CRLF to LF, and trim the single trailing newline PowerShell's pipeline
/// appends (interior blank lines are preserved).
pub fn normalize_pasted_text(raw: &str) -> String {
    let raw = raw.strip_prefix('\u{feff}').unwrap_or(raw);
    let mut text = raw.replace("\r\n", "\n");
    if text.ends_with('\n') {
        text.pop();
    }
    text
}

/// Read the system clipboard as text (Windows: `Get-Clipboard -Raw`). `None` when the
/// clipboard holds no text or the read fails.
#[cfg(windows)]
pub fn paste_text() -> Option<String> {
    let argv = windows_paste_argv();
    let out = Command::new(&argv[0])
        .args(&argv[1..])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = normalize_pasted_text(&String::from_utf8_lossy(&out.stdout));
    if text.is_empty() { None } else { Some(text) }
}

/// Read the system clipboard as text. Always `None` on Unix: the TUI receives paste through
/// the terminal's bracketed paste there, so no clipboard-read tool is probed.
#[cfg(not(windows))]
pub fn paste_text() -> Option<String> {
    None
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

    #[test]
    fn windows_powershell_argv_shape() {
        let argv = windows_powershell_argv("Get-Date");
        assert_eq!(
            argv,
            [
                "powershell",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "Get-Date"
            ]
        );
    }

    #[test]
    fn windows_copy_argv_is_powershell_set_clipboard_over_stdin() {
        let argv = windows_copy_argv();
        // powershell.exe, never clip.exe (clip mangles non-ASCII via the OEM codepage).
        assert_eq!(argv[0], "powershell");
        assert!(!argv.iter().any(|a| a.contains("clip.exe")));
        let script = argv.last().unwrap();
        // The script must read raw stdin bytes and decode them as UTF-8 explicitly,
        // so the console codepage never touches the text.
        assert!(script.contains("OpenStandardInput"));
        assert!(script.contains("UTF8"));
        assert!(script.contains("Set-Clipboard"));
    }

    #[test]
    fn windows_paste_argv_is_get_clipboard_raw_with_utf8_stdout() {
        let argv = windows_paste_argv();
        assert_eq!(argv[0], "powershell");
        let script = argv.last().unwrap();
        assert!(script.contains("Get-Clipboard -Raw"));
        // Redirected stdout defaults to the OEM codepage on Windows PowerShell 5.1;
        // the script must force UTF-8 before writing.
        assert!(script.contains("OutputEncoding"));
        assert!(script.contains("UTF8"));
    }

    #[test]
    fn normalize_pasted_text_converts_crlf_and_trims_one_trailing_newline() {
        assert_eq!(normalize_pasted_text("a\r\nb\r\n"), "a\nb");
        assert_eq!(normalize_pasted_text("plain\n"), "plain");
        // Only the single newline PowerShell appends is trimmed; interior blanks stay.
        assert_eq!(normalize_pasted_text("a\n\n"), "a\n");
        assert_eq!(normalize_pasted_text("no-trailing"), "no-trailing");
        assert_eq!(normalize_pasted_text(""), "");
        // A UTF-8 BOM sneaking through redirected PowerShell output is dropped.
        assert_eq!(normalize_pasted_text("\u{feff}x\r\n"), "x");
    }

    #[cfg(not(windows))]
    #[test]
    fn paste_text_is_none_on_unix() {
        // Unix TUIs receive paste through the terminal's bracketed paste, not a tool probe.
        assert_eq!(paste_text(), None);
    }
}
