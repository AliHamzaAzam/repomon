//! tmux-backed agent runtime.
//!
//! Each lane's agent runs in its own window (`lane-<id>`) of a managed tmux session. The
//! daemon reads output with `capture-pane` and sends input with `send-keys`. Because tmux
//! owns the processes, agents survive the daemon and the TUI — reattach and they're still
//! there with full scrollback. All methods are synchronous; the daemon calls them from
//! `spawn_blocking`.

use std::path::Path;
use std::process::Command;

use crate::error::{Error, Result};
use crate::model::LaneId;

/// A handle to a managed tmux session. Cheap to clone.
#[derive(Clone, Debug)]
pub struct TmuxRuntime {
    session: String,
}

impl TmuxRuntime {
    pub fn new(session: impl Into<String>) -> Self {
        Self {
            session: session.into(),
        }
    }

    /// Is tmux installed and runnable?
    pub fn available() -> bool {
        Command::new("tmux")
            .arg("-V")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    pub fn session(&self) -> &str {
        &self.session
    }

    /// The tmux window name for a lane.
    pub fn window_name(lane: LaneId) -> String {
        format!("lane-{lane}")
    }

    /// The `session:window` target for a lane.
    pub fn target(&self, lane: LaneId) -> String {
        format!("{}:{}", self.session, Self::window_name(lane))
    }

    fn run(&self, args: &[&str]) -> Result<String> {
        let out = Command::new("tmux")
            .args(args)
            .output()
            .map_err(Error::Io)?;
        if !out.status.success() {
            return Err(Error::Agent(format!(
                "tmux {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    fn ok(&self, args: &[&str]) -> bool {
        Command::new("tmux")
            .args(args)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    pub fn session_exists(&self) -> bool {
        self.ok(&["has-session", "-t", &self.session])
    }

    /// Window names currently in the session.
    pub fn list_windows(&self) -> Result<Vec<String>> {
        if !self.session_exists() {
            return Ok(Vec::new());
        }
        let out = self.run(&["list-windows", "-t", &self.session, "-F", "#{window_name}"])?;
        Ok(out.lines().map(str::to_string).collect())
    }

    pub fn has_window(&self, lane: LaneId) -> bool {
        self.list_windows()
            .map(|w| w.contains(&Self::window_name(lane)))
            .unwrap_or(false)
    }

    /// Launch `command` for `lane` in `cwd`, (re)creating the window. Returns the target.
    pub fn spawn(&self, lane: LaneId, cwd: &Path, command: &str) -> Result<String> {
        let window = Self::window_name(lane);
        let cwd = cwd.to_string_lossy();
        if self.has_window(lane) {
            let _ = self.kill(lane);
        }
        if self.session_exists() {
            self.run(&[
                "new-window",
                "-t",
                &self.session,
                "-n",
                &window,
                "-c",
                &cwd,
                command,
            ])?;
        } else {
            self.run(&[
                "new-session",
                "-d",
                "-s",
                &self.session,
                "-n",
                &window,
                "-c",
                &cwd,
                command,
            ])?;
        }
        Ok(self.target(lane))
    }

    /// Capture the pane's text. With `lines`, include that many rows of scrollback.
    pub fn capture(&self, lane: LaneId, lines: Option<u32>) -> Result<String> {
        if !self.has_window(lane) {
            return Ok(String::new());
        }
        let target = self.target(lane);
        let start = lines.map(|n| format!("-{n}")).unwrap_or_default();
        let mut args = vec!["capture-pane", "-p", "-t", &target];
        if lines.is_some() {
            args.push("-S");
            args.push(&start);
        }
        self.run(&args)
    }

    /// Type `text` into the agent and press Enter.
    pub fn send_text(&self, lane: LaneId, text: &str) -> Result<()> {
        let target = self.target(lane);
        self.run(&["send-keys", "-t", &target, "-l", text])?;
        self.run(&["send-keys", "-t", &target, "Enter"])?;
        Ok(())
    }

    /// Send a raw key (e.g. `C-c`) to the agent.
    pub fn send_key(&self, lane: LaneId, key: &str) -> Result<()> {
        let target = self.target(lane);
        self.run(&["send-keys", "-t", &target, key])?;
        Ok(())
    }

    /// Terminate the agent's window.
    pub fn kill(&self, lane: LaneId) -> Result<()> {
        self.run(&["kill-window", "-t", &self.target(lane)])?;
        Ok(())
    }

    /// Args for `tmux attach` to this lane (the TUI execs this for a raw session).
    pub fn attach_args(&self, lane: LaneId) -> Vec<String> {
        vec!["attach".into(), "-t".into(), self.target(lane)]
    }
}

/// Single-quote a string for safe inclusion in a shell command.
pub fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_for_shell() {
        assert_eq!(shell_quote("hello"), "'hello'");
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
    }

    #[test]
    fn target_format() {
        let rt = TmuxRuntime::new("repomon");
        assert_eq!(rt.target(7), "repomon:lane-7");
    }

    #[test]
    fn spawn_capture_send_kill_roundtrip() {
        if !TmuxRuntime::available() {
            eprintln!("tmux not available; skipping live runtime test");
            return;
        }
        let rt = TmuxRuntime::new(format!("repomon-test-{}", std::process::id()));
        let cwd = std::env::temp_dir();
        let lane: LaneId = 1;

        rt.spawn(lane, &cwd, "sh -c 'echo HELLO_REPOMON; sleep 30'")
            .unwrap();
        assert!(rt.has_window(lane));

        std::thread::sleep(std::time::Duration::from_millis(400));
        let out = rt.capture(lane, None).unwrap();
        assert!(out.contains("HELLO_REPOMON"), "capture was: {out:?}");

        rt.send_text(lane, "echo SECOND_LINE").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(400));
        let out2 = rt.capture(lane, None).unwrap();
        assert!(out2.contains("SECOND_LINE"), "after send: {out2:?}");

        rt.kill(lane).unwrap();
        assert!(!rt.has_window(lane));

        // Tear down the test session.
        let _ = Command::new("tmux")
            .args(["kill-session", "-t", rt.session()])
            .output();
    }
}
