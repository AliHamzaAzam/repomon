//! Provides synchronous agent-window operations shared by tmux and the Windows host. Call blocking
//! backend I/O through spawn_blocking when serving asynchronous requests.

use std::path::{Path, PathBuf};

use crate::error::Result;
use crate::model::LaneId;

use super::tmux::{TmuxRuntime, WindowMeta};

/// Describes a launch using a configured base command, arguments, working directory, and
/// environment overrides; shell backends quote appended arguments and assignments, while direct
/// backends set the child environment.
#[derive(Clone, Debug, Default)]
pub struct SpawnSpec {
    /// Base command line (a shell fragment on Unix; never empty for a spawn).
    pub program: String,
    /// Extra arguments, quoted/passed by the backend.
    pub args: Vec<String>,
    /// Working directory for the agent process.
    pub cwd: PathBuf,
    /// Environment overrides applied to the agent process.
    pub env: Vec<(String, String)>,
}

impl SpawnSpec {
    /// A spec with just a program and a working directory (the common case).
    pub fn new(program: impl Into<String>, cwd: impl Into<PathBuf>) -> Self {
        SpawnSpec {
            program: program.into(),
            cwd: cwd.into(),
            args: Vec::new(),
            env: Vec::new(),
        }
    }

    /// Append an argument (backend-quoted at render time).
    pub fn arg(mut self, arg: impl Into<String>) -> Self {
        self.args.push(arg.into());
        self
    }
}

/// Options for capturing a window's pane text. `Default` captures the visible pane.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CaptureOpts {
    /// Capture starting `n` lines back into scrollback (tmux `-S -n`); `None` = visible pane.
    pub last_lines: Option<u32>,
}

impl CaptureOpts {
    /// Capture the visible pane only.
    pub fn visible() -> Self {
        CaptureOpts::default()
    }

    /// Capture the last `n` lines of scrollback plus the visible pane.
    pub fn last(n: u32) -> Self {
        CaptureOpts {
            last_lines: Some(n),
        }
    }
}

/// A pane's visible cursor position, 0-based from the top-left of the pane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cursor {
    pub col: u16,
    pub row: u16,
}

/// One mouse-wheel gesture at a 1-based terminal cell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScrollEvent {
    pub up: bool,
    pub ticks: u32,
    pub col: u16,
    pub row: u16,
}

/// One window as the orphan reaper sees it: its name, the pane's current working directory,
/// and the last pane-activity time (Unix epoch seconds).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowActivity {
    pub name: String,
    pub cwd: PathBuf,
    pub last_activity: i64,
}

/// Result of the cooperative single-owner guard (see
/// [`SessionBackend::claim_or_verify_owner`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OwnerState {
    /// This daemon owns the backend server (it claimed it, or re-verified its own stamp).
    Owned,
    /// Another daemon's stamp is on the server - back off from destructive sweeps.
    OwnedByOther,
}

/// Carries the backend-specific attach program and arguments so clients need no transport-specific
/// knowledge.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttachCommand {
    pub program: String,
    pub args: Vec<String>,
}

/// One ordered event from a live terminal stream. Grid changes and bytes share the same channel
/// so a renderer can resize before it interprets output produced at the new dimensions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ByteStreamEvent {
    Bytes(Vec<u8>),
    Grid { cols: u16, rows: u16 },
}

/// A live raw-PTY stream for one window, as handed out by
/// [`SessionBackend::open_byte_stream`]. The backend owns the plumbing; the consumer just drains
/// `rx`. The channel closes when the stream ends or the window dies.
pub struct ByteStream {
    /// Ordered raw-output and grid-change events.
    pub rx: tokio::sync::mpsc::UnboundedReceiver<ByteStreamEvent>,
}

/// A durable, out-of-process agent-session runtime: spawn/capture/input/resize/kill windows
/// that survive the daemon. See the module docs for the sync + `Send + Sync` contract.
pub trait SessionBackend: Send + Sync {
    /// Is the backend usable on this machine (e.g. tmux installed and runnable)?
    fn available(&self) -> bool;

    /// Human-readable identity of the backing server, for diagnostics/logging only
    /// (tmux: the session name).
    fn label(&self) -> String;

    /// Does the backing session/server currently exist?
    fn session_exists(&self) -> bool;

    /// Cooperative single-owner guard: stamp the server with `me` if unowned, else verify the
    /// existing stamp. Two daemons aimed at the same server must never run destructive sweeps
    /// against each other's windows.
    fn claim_or_verify_owner(&self, me: &str) -> OwnerState;

    /// Window names currently live in the session. A vanished server reads as empty.
    fn list_windows(&self) -> Result<Vec<String>>;

    /// Return a stable-enough identity for the live process in `window`, when the backend can
    /// observe one. It must change when the process is replaced, but remain stable across daemon
    /// restarts while that process survives.
    fn window_process_fingerprint(&self, _window: &str) -> Result<Option<String>> {
        Ok(None)
    }

    /// Provides durable window identity for stable transcript routing, falling back to names when
    /// metadata is unavailable.
    fn list_windows_meta(&self) -> Result<Vec<WindowMeta>> {
        Ok(self
            .list_windows()?
            .into_iter()
            .enumerate()
            .map(|(index, name)| WindowMeta {
                name,
                wid: index as u64 + 1,
                session: None,
                agent_kind: None,
            })
            .collect())
    }

    /// Persist a transcript identity on a named window when the backend supports it.
    fn set_window_session(&self, _window: &str, _session_id: &str) -> Result<()> {
        Ok(())
    }

    /// Persist a transcript identity on a backend-native window id when supported.
    fn set_window_session_by_id(&self, _wid: u64, _session_id: &str) -> Result<()> {
        Ok(())
    }

    /// Persist an agent kind on a named window when the backend supports it.
    fn set_window_agent_kind(&self, _window: &str, _kind: &str) -> Result<()> {
        Ok(())
    }

    /// Persist an agent kind on a backend-native window id when supported.
    fn set_window_agent_kind_by_id(&self, _wid: u64, _kind: &str) -> Result<()> {
        Ok(())
    }

    /// Every window's name, pane cwd, and last-activity time - the orphan reaper's view.
    fn list_windows_with_activity(&self) -> Result<Vec<WindowActivity>>;

    /// Launch an agent in `lane`'s first *free* slot window; returns the new window's exact
    /// attach target. A running agent is never killed - spawning again runs a second agent
    /// side by side.
    fn spawn(&self, lane: LaneId, spec: &SpawnSpec) -> Result<String>;

    /// Launch a command as an arbitrary named window (usage probe, orchestrator); returns the
    /// window's exact attach target.
    fn spawn_named(&self, window: &str, spec: &SpawnSpec) -> Result<String>;

    /// Open a plain interactive shell (the user's default) in `cwd` as a named window; returns
    /// its attach target.
    fn open_named(&self, window: &str, cwd: &Path) -> Result<String>;

    /// Capture the window's pane text, including ANSI color escapes. A vanished window reads
    /// as empty output.
    fn capture_named(&self, window: &str, opts: CaptureOpts) -> Result<String>;

    /// The pane's visible cursor, or `None` when hidden or the window is gone.
    fn cursor_named(&self, window: &str) -> Option<Cursor>;

    /// The pane's current grid `(cols, rows)`, or `None` when the window is gone.
    fn size_named(&self, window: &str) -> Option<(u16, u16)>;

    /// Resize the window to `cols × rows` (mediated-view reflow; pins the size).
    fn resize_named(&self, window: &str, cols: u16, rows: u16) -> Result<()>;

    /// Undo a pinned size: let the window follow the attaching client's size again.
    fn follow_client_named(&self, window: &str) -> Result<()>;

    /// Whether the window's app is on the alternate screen (a full-screen TUI).
    fn alternate_on_named(&self, window: &str) -> bool;

    /// Forward `ticks` mouse-wheel events (up or down) to the window's app.
    fn scroll_wheel_named(&self, window: &str, event: ScrollEvent) -> Result<()>;

    /// Send a literal string (no trailing Enter) - one keystroke's worth of input.
    fn send_literal_named(&self, window: &str, text: &str) -> Result<()>;

    /// Type `text` into the window and press Enter.
    fn send_text_named(&self, window: &str, text: &str) -> Result<()>;

    /// Send a named key (e.g. `C-c`, `Enter`, `Escape`).
    fn send_key_named(&self, window: &str, key: &str) -> Result<()>;

    /// Terminate a named window (an agent slot, a terminal, the orchestrator).
    fn kill_named(&self, window: &str) -> Result<()>;

    /// Make the attached experience feel native (mouse, clipboard, scrollback, status bar).
    /// Idempotent; a no-op for backends with nothing to configure.
    fn configure(&self);

    /// The attach target for a named window (tmux: `session:window`, prefix-matched).
    fn target_named(&self, window: &str) -> String;

    /// The *exact* attach target for a named window (tmux: `session:=window`), immune to
    /// prefix-matching surprises.
    fn exact_target_named(&self, window: &str) -> String;

    /// The command a client runs in a real terminal to attach to `target`.
    fn attach_command(&self, target: &str) -> AttachCommand;

    /// Start streaming the window's raw PTY bytes and authoritative grid changes in order.
    /// The returned [`ByteStream`]'s channel closes when the stream ends. Callers refcount
    /// watchers and share one stream per window.
    fn open_byte_stream(&self, window: &str) -> Result<ByteStream>;

    /// Stop streaming the window's bytes (EOFs the reader). Benign when the window - or the
    /// whole server - is already gone.
    fn close_byte_stream(&self, window: &str) -> Result<()>;

    /// Returns authoritative live-agent counts by working directory, or `None` to request the
    /// platform process probe.
    fn live_agent_cwds(&self) -> Option<std::collections::HashMap<PathBuf, usize>> {
        None
    }

    // ---- provided helpers (backend-agnostic, built on `list_windows` + the shared lane/window
    // naming convention, which is identical across backends) ----

    /// Is there a window with this exact name?
    fn has_named(&self, name: &str) -> bool {
        self.list_windows()
            .map(|w| w.iter().any(|x| x == name))
            .unwrap_or(false)
    }

    /// Does `lane`'s first agent slot window exist?
    fn has_window(&self, lane: LaneId) -> bool {
        self.has_named(&TmuxRuntime::window_name(lane))
    }

    /// `lane`'s live agent windows, in slot order.
    fn windows_for(&self, lane: LaneId) -> Result<Vec<String>> {
        Ok(TmuxRuntime::lane_windows_in(&self.list_windows()?, lane))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawn_spec_builder_collects_args() {
        let spec = SpawnSpec::new("claude", "/tmp").arg("do the thing");
        assert_eq!(spec.program, "claude");
        assert_eq!(spec.cwd, PathBuf::from("/tmp"));
        assert_eq!(spec.args, vec!["do the thing"]);
        assert!(spec.env.is_empty());
    }

    #[test]
    fn capture_opts_constructors() {
        assert_eq!(CaptureOpts::visible().last_lines, None);
        assert_eq!(CaptureOpts::last(45).last_lines, Some(45));
        assert_eq!(CaptureOpts::default(), CaptureOpts::visible());
    }

    #[test]
    fn live_agent_cwds_defaults_to_probe_unavailable() {
        // Backends without an authoritative process view (tmux) answer `None`, which the
        // daemon's liveness probe already treats as "don't filter" - the safe degradation.
        let rt = TmuxRuntime::new("repomon-live-cwds-test");
        assert!(rt.live_agent_cwds().is_none());
    }
}
