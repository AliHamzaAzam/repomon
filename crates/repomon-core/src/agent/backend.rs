//! The session-backend abstraction the daemon drives agents through.
//!
//! [`SessionBackend`] is the single choke point between repomon and whatever owns the agent
//! processes. On macOS/Linux the implementation is [`TmuxRuntime`](super::tmux::TmuxRuntime)
//! (a long-lived tmux server on a dedicated socket); a future Windows backend talks to
//! per-agent host processes instead. Everything above this trait — the RPC handlers, the
//! reaper, auto-continue, the usage probe, byte streaming — is backend-agnostic.
//!
//! The trait is deliberately **synchronous**: every daemon call site already runs backend IO
//! inside `tokio::task::spawn_blocking`, and tmux itself is driven via blocking subprocess
//! calls. Implementations must be `Send + Sync` so an `Arc<dyn SessionBackend>` can be shared
//! across tasks.

use std::path::{Path, PathBuf};

use crate::error::Result;
use crate::model::LaneId;

use super::tmux::TmuxRuntime;

/// How to launch an agent process, structurally — program, extra arguments, working directory,
/// and environment overrides — instead of a pre-quoted shell string.
///
/// `program` is the base command line as configured by the user (on Unix it may be a shell
/// fragment such as `CLAUDE_CONFIG_DIR='…' claude`, because tmux runs commands through `sh -c`
/// and agent commands are user-configured shell strings). `args` are appended by the backend
/// with backend-appropriate quoting (the tmux impl single-quotes them via
/// [`shell_quote`](super::tmux::shell_quote)); `env` entries are prepended as `KEY=value`
/// assignments by backends that launch through a shell, or set on the child process directly
/// by backends that don't.
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
    /// Another daemon's stamp is on the server — back off from destructive sweeps.
    OwnedByOther,
}

/// The exact command a client should run in a real terminal to attach to a target — e.g.
/// `tmux -L <session> attach -t <target>` on Unix. Carried as the optional `attach` field of
/// the `agent.target` / `terminal.target` / `orchestrator.target` RPC responses so clients
/// don't have to know which backend is running.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttachCommand {
    pub program: String,
    pub args: Vec<String>,
}

/// A live raw-PTY byte stream for one window, as handed out by
/// [`SessionBackend::open_byte_stream`]. The backend owns the plumbing (tmux: `mkfifo` +
/// `pipe-pane`, with a reader thread pumping the fifo); the consumer just drains `rx`. The
/// channel closes when the stream ends — the pipe was turned off via
/// [`SessionBackend::close_byte_stream`] or the window died.
pub struct ByteStream {
    /// Chunks of raw PTY output, bounded to a backend-chosen chunk size per message.
    pub rx: tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>,
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

    /// Every window's name, pane cwd, and last-activity time — the orphan reaper's view.
    fn list_windows_with_activity(&self) -> Result<Vec<WindowActivity>>;

    /// Launch an agent in `lane`'s first *free* slot window; returns the new window's exact
    /// attach target. A running agent is never killed — spawning again runs a second agent
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
    fn scroll_wheel_named(&self, window: &str, up: bool, ticks: u32) -> Result<()>;

    /// Send a literal string (no trailing Enter) — one keystroke's worth of input.
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

    /// Start streaming the window's raw PTY bytes. The backend owns the transport (tmux:
    /// fifo + `pipe-pane` + reader thread); the returned [`ByteStream`]'s channel closes when
    /// the stream ends. At most one stream per window (tmux allows a single `pipe-pane`) —
    /// callers refcount watchers and share it.
    fn open_byte_stream(&self, window: &str) -> Result<ByteStream>;

    /// Stop streaming the window's bytes (EOFs the reader). Benign when the window — or the
    /// whole server — is already gone.
    fn close_byte_stream(&self, window: &str) -> Result<()>;

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
}
