//! tmux-backed agent runtime.
//!
//! Each lane's agent runs in its own window (`lane-<id>`) of a managed tmux session. The
//! daemon reads output with `capture-pane` and sends input with `send-keys`. Because tmux
//! owns the processes, agents survive the daemon and the TUI — reattach and they're still
//! there with full scrollback. All methods are synchronous; the daemon calls them from
//! `spawn_blocking`.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::error::{Error, Result};
use crate::model::LaneId;

use super::backend::{
    AttachCommand, ByteStream, CaptureOpts, Cursor, OwnerState, SessionBackend, SpawnSpec,
    WindowActivity,
};

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

    /// The tmux window name for a lane's first agent slot.
    pub fn window_name(lane: LaneId) -> String {
        format!("lane-{lane}")
    }

    /// The window name for a lane's `slot`-th agent (1-based): `lane-7`, `lane-7-2`, `lane-7-3`…
    /// Several agents can run side by side in one lane, one window each.
    pub fn slot_name(lane: LaneId, slot: usize) -> String {
        if slot <= 1 {
            Self::window_name(lane)
        } else {
            format!("lane-{lane}-{slot}")
        }
    }

    /// Parse a managed agent window name back into `(lane, slot)` — the inverse of
    /// [`window_name`]/[`slot_name`]. `lane-7` → `(7, 1)`, `lane-7-3` → `(7, 3)`. Returns `None`
    /// for any name that isn't a well-formed lane window (a terminal, the usage probe, or a
    /// malformed `lane-…`), so callers can safely ignore non-agent windows. Matches the exact
    /// shapes [`lane_windows_in`] counts, so the reaper and the overlay agree on what's a slot.
    pub fn parse_lane_window(name: &str) -> Option<(LaneId, usize)> {
        let rest = name.strip_prefix("lane-")?;
        match rest.split_once('-') {
            None => rest.parse::<LaneId>().ok().map(|id| (id, 1)),
            Some((id, slot)) => {
                let id = id.parse::<LaneId>().ok()?;
                let slot = slot.parse::<usize>().ok().filter(|&s| s >= 2)?;
                Some((id, slot))
            }
        }
    }

    /// The lane a managed window belongs to, or `None` if it isn't a lane window.
    pub fn lane_id_of(name: &str) -> Option<LaneId> {
        Self::parse_lane_window(name).map(|(id, _)| id)
    }

    /// Parse a plain-terminal window name (`term-{lane}-{n}`, as `terminal.open` mints them)
    /// into its lane. `None` for anything else — agent windows, the usage probe, malformed
    /// names — so terminal scans and agent scans stay mutually blind.
    pub fn parse_term_window(name: &str) -> Option<LaneId> {
        let rest = name.strip_prefix("term-")?;
        let (id, seq) = rest.split_once('-')?;
        seq.parse::<u32>().ok()?;
        id.parse::<LaneId>().ok()
    }

    /// The 1-based agent slot a managed window occupies, or `None` if it isn't a lane window.
    pub fn slot_of_window(name: &str) -> Option<usize> {
        Self::parse_lane_window(name).map(|(_, slot)| slot)
    }

    /// Filter `names` down to `lane`'s agent windows, in slot order (= spawn order). Exact
    /// matching, so `lane-1` never claims `lane-12`'s windows.
    pub fn lane_windows_in(names: &[String], lane: LaneId) -> Vec<String> {
        let base = Self::window_name(lane);
        let prefix = format!("{base}-");
        let mut slots: Vec<(usize, String)> = names
            .iter()
            .filter_map(|n| {
                if *n == base {
                    Some((1, n.clone()))
                } else {
                    let rest = n.strip_prefix(&prefix)?;
                    let slot: usize = rest.parse().ok().filter(|&s| s >= 2)?;
                    Some((slot, n.clone()))
                }
            })
            .collect();
        slots.sort_by_key(|(s, _)| *s);
        slots.into_iter().map(|(_, n)| n).collect()
    }

    /// `lane`'s live agent windows, in slot order.
    pub fn windows_for(&self, lane: LaneId) -> Result<Vec<String>> {
        Ok(Self::lane_windows_in(&self.list_windows()?, lane))
    }

    /// The `session:window` target for a lane's first agent slot.
    pub fn target(&self, lane: LaneId) -> String {
        format!("{}:{}", self.session, Self::window_name(lane))
    }

    /// An *exact* `session:=window` target — tmux otherwise prefix-matches window names, which
    /// would let `lane-1` resolve to `lane-1-2` once the first slot is gone.
    fn exact_target(&self, name: &str) -> String {
        format!("{}:={}", self.session, name)
    }

    /// repomon runs its tmux on a dedicated socket (named after the session) so its windows
    /// never collide with — or share a server with — the user's own tmux.
    fn full_args<'a>(&'a self, args: &'a [&'a str]) -> Vec<&'a str> {
        let mut full = vec!["-L", self.session.as_str()];
        full.extend_from_slice(args);
        full
    }

    fn run(&self, args: &[&str]) -> Result<String> {
        let out = Command::new("tmux")
            .args(self.full_args(args))
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

    /// Like [`run`], but a *benign absence* — the window/session/pane is gone, or no tmux server
    /// is running — is reported as empty output instead of an error. Lets `capture`/`list-windows`
    /// skip a `has-session`/`has_named` preflight fork (the single biggest CPU win): a vanished
    /// target means "nothing to show", while a *real* tmux fault still propagates as `Err`.
    fn run_allow_absent(&self, args: &[&str]) -> Result<String> {
        let out = Command::new("tmux")
            .args(self.full_args(args))
            .output()
            .map_err(Error::Io)?;
        if out.status.success() {
            return Ok(String::from_utf8_lossy(&out.stdout).into_owned());
        }
        let stderr = String::from_utf8_lossy(&out.stderr);
        // tmux: "can't find window/session/pane: …", "no server running on …",
        // "error connecting to …" — the target simply isn't there.
        let absent = stderr.contains("can't find ")
            || stderr.contains("no server running")
            || stderr.contains("error connecting");
        if absent {
            Ok(String::new())
        } else {
            Err(Error::Agent(format!(
                "tmux {} failed: {}",
                args.join(" "),
                stderr.trim()
            )))
        }
    }

    fn ok(&self, args: &[&str]) -> bool {
        Command::new("tmux")
            .args(self.full_args(args))
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    /// The tmux socket label repomon uses (pass as `tmux -L <label>`). Equals the session name.
    pub fn socket(&self) -> &str {
        &self.session
    }

    pub fn session_exists(&self) -> bool {
        self.ok(&["has-session", "-t", &self.session])
    }

    /// Cooperative single-owner guard for this tmux server (`tmux -L <session>`). Two `repomond`s
    /// aimed at the same session — e.g. a stray test daemon that kept the default `tmux_session`
    /// while using its own socket+store — must never run destructive sweeps against each other's
    /// windows: the second daemon's store doesn't know the first's lanes, so its reaper would mark
    /// every real `lane-<id>` window an orphan and kill it (the disappearing-sessions bug). The
    /// first daemon to call this stamps `@repomon-owner` with its identity (`me`, its db path);
    /// later daemons read a different value and back off. Returns true iff `me` owns the server.
    pub fn claim_or_verify_owner(&self, me: &str) -> bool {
        match self.server_owner() {
            Some(owner) => owner == me,
            None => {
                // Claim it, then re-read: if another daemon set it concurrently we lose and back off.
                let _ = self.ok(&["set-option", "-s", "@repomon-owner", me]);
                self.server_owner().as_deref() == Some(me)
            }
        }
    }

    /// The identity the owning daemon stamped on this server, if any (unset/empty → `None`).
    fn server_owner(&self) -> Option<String> {
        let out = self.run(&["show-options", "-sv", "@repomon-owner"]).ok()?;
        let s = out.trim();
        (!s.is_empty()).then(|| s.to_string())
    }

    /// Window names currently in the session.
    pub fn list_windows(&self) -> Result<Vec<String>> {
        // No `has-session` preflight — `run_allow_absent` turns "no server / can't find session"
        // into an empty list, saving a fork on every call (overlay_agents, auto_continue, …).
        let out =
            self.run_allow_absent(&["list-windows", "-t", &self.session, "-F", "#{window_name}"])?;
        Ok(out.lines().map(str::to_string).collect())
    }

    /// Each window's name, current pane working directory, and last pane-activity time (Unix
    /// epoch seconds). Used by the orphan reaper: the cwd spots `lane-<id>` windows whose cwd no
    /// longer matches the worktree that id maps to (a stale window left by a re-registered /
    /// renumbered worktree), and the activity time lets it spare a window whose agent is still
    /// actively producing output.
    pub fn list_windows_with_activity(&self) -> Result<Vec<(String, PathBuf, i64)>> {
        let out = self.run_allow_absent(&[
            "list-windows",
            "-t",
            &self.session,
            "-F",
            "#{window_name}\t#{pane_current_path}\t#{window_activity}",
        ])?;
        Ok(out
            .lines()
            .filter_map(|l| {
                let mut it = l.splitn(3, '\t');
                let name = it.next()?.to_string();
                let path = PathBuf::from(it.next()?);
                let activity = it
                    .next()
                    .and_then(|s| s.trim().parse::<i64>().ok())
                    .unwrap_or(0);
                Some((name, path, activity))
            })
            .collect())
    }

    pub fn has_window(&self, lane: LaneId) -> bool {
        self.list_windows()
            .map(|w| w.contains(&Self::window_name(lane)))
            .unwrap_or(false)
    }

    /// Launch `command` for `lane` in `cwd` in the lane's first *free* agent slot — a running
    /// agent is never killed, so spawning again runs a second agent side by side. Returns the
    /// new window's exact target.
    pub fn spawn(&self, lane: LaneId, cwd: &Path, command: &str) -> Result<String> {
        let taken = self.windows_for(lane).unwrap_or_default();
        let window = (1..)
            .map(|slot| Self::slot_name(lane, slot))
            .find(|name| !taken.contains(name))
            .expect("unbounded slot range");
        let cwd = cwd.to_string_lossy();
        if self.session_exists() {
            // `-d`: create the window WITHOUT making it the session's active window. tmux's default
            // `new-window` selects the new window, which yanks any attached `tmux attach` client
            // (a human "all the way in" on another agent) over to it, then yanks back when it's
            // killed. Spawning detached keeps the human's focused window put. See the usage-probe
            // flap (`spawn_named`) for the worst case.
            self.run(&[
                "new-window",
                "-d",
                "-t",
                &self.session,
                "-n",
                &window,
                "-c",
                &cwd,
                command,
            ])?;
        } else {
            // A roomy detached size so the agent renders wide (vs the 80×24 default).
            self.run(&[
                "new-session",
                "-d",
                "-x",
                "220",
                "-y",
                "50",
                "-s",
                &self.session,
                "-n",
                &window,
                "-c",
                &cwd,
                command,
            ])?;
        }
        self.configure();
        Ok(self.exact_target(&window))
    }

    /// Capture the pane's text, including ANSI color escapes (`-e`).
    pub fn capture(&self, lane: LaneId, lines: Option<u32>) -> Result<String> {
        self.capture_named(&Self::window_name(lane), lines)
    }

    /// Capture a specific agent window's pane text.
    pub fn capture_named(&self, window: &str, lines: Option<u32>) -> Result<String> {
        // No `has_named` preflight (which itself forked `has-session` + `list-windows`): capture
        // directly and let `run_allow_absent` map a vanished window to empty output. Each capture
        // is now ONE fork instead of three — the dominant streamer hot path.
        let target = self.exact_target(window);
        let start = lines.map(|n| format!("-{n}")).unwrap_or_default();
        let mut args = vec!["capture-pane", "-e", "-p", "-t", &target];
        if lines.is_some() {
            args.push("-S");
            args.push(&start);
        }
        self.run_allow_absent(&args)
    }

    /// The agent pane's cursor position `(col, row)`, 0-based from the top-left of the visible
    /// pane, when the app is actually showing a cursor (`cursor_flag`). `None` if the window is
    /// gone or the cursor is hidden. Used to draw the cursor in the mediated focus/insert view.
    pub fn cursor_named(&self, window: &str) -> Option<(u16, u16)> {
        let target = self.exact_target(window);
        let out = self
            .run_allow_absent(&[
                "display-message",
                "-p",
                "-t",
                &target,
                "-F",
                "#{cursor_x} #{cursor_y} #{cursor_flag}",
            ])
            .ok()?;
        let mut it = out.split_whitespace();
        let x: u16 = it.next()?.parse().ok()?;
        let y: u16 = it.next()?.parse().ok()?;
        let visible = it.next() == Some("1");
        visible.then_some((x, y))
    }

    /// The pane's current grid `(cols, rows)`, or `None` when the window is gone. Remote
    /// clients render their emulator at exactly this grid instead of resizing the real pane
    /// (which would squeeze a simultaneously attached TUI's view).
    pub fn size_named(&self, window: &str) -> Option<(u16, u16)> {
        let target = self.exact_target(window);
        let out = self
            .run_allow_absent(&[
                "display-message",
                "-p",
                "-t",
                &target,
                "-F",
                "#{pane_width} #{pane_height}",
            ])
            .ok()?;
        let mut it = out.split_whitespace();
        let cols: u16 = it.next()?.parse().ok()?;
        let rows: u16 = it.next()?.parse().ok()?;
        Some((cols, rows))
    }

    /// Resize a window to `cols × rows` so the mediated view's pane reflows to exactly the visible
    /// width (no right-edge clipping). `resize-window` sets the window's `window-size` to `manual`;
    /// [`follow_client_named`](Self::follow_client_named) restores client-follow before an attach.
    pub fn resize_named(&self, window: &str, cols: u16, rows: u16) -> Result<()> {
        let target = self.exact_target(window);
        let (cols, rows) = (cols.to_string(), rows.to_string());
        self.run_allow_absent(&["resize-window", "-t", &target, "-x", &cols, "-y", &rows])?;
        Ok(())
    }

    /// Let `window` follow the attaching client's size again (undoing `resize_named`'s manual
    /// size), so `tmux attach` renders the agent at the real terminal's full size.
    pub fn follow_client_named(&self, window: &str) -> Result<()> {
        let target = self.exact_target(window);
        self.run_allow_absent(&["set-window-option", "-t", &target, "window-size", "latest"])?;
        Ok(())
    }

    /// Whether `window`'s app is on the *alternate screen* — i.e. a full-screen TUI (Claude, vim, …)
    /// that owns its own scrollback. `false` for a plain shell (whose scrollback lives in tmux).
    pub fn alternate_on_named(&self, window: &str) -> bool {
        let target = self.exact_target(window);
        self.run_allow_absent(&[
            "display-message",
            "-p",
            "-t",
            &target,
            "-F",
            "#{alternate_on}",
        ])
        .map(|s| s.trim() == "1")
        .unwrap_or(false)
    }

    /// Forward `ticks` mouse-wheel scroll events to `window`'s app, so a full-screen agent scrolls
    /// its own history (the mediated pane can't otherwise — alternate-screen apps keep no tmux
    /// scrollback). Sends SGR wheel sequences (button 64 = up, 65 = down) at the pane's top-left.
    pub fn scroll_wheel_named(&self, window: &str, up: bool, ticks: u32) -> Result<()> {
        if ticks == 0 {
            return Ok(());
        }
        let button = if up { 64 } else { 65 };
        let seq = format!("\x1b[<{button};1;1M").repeat(ticks as usize);
        let target = self.exact_target(window);
        self.run_allow_absent(&["send-keys", "-t", &target, "-l", &seq])?;
        Ok(())
    }

    /// Start streaming `window`'s raw PTY output into `fifo` (an existing named pipe): every
    /// byte the pane emits from now on is appended by a `cat` tmux runs for the pane. Replaces
    /// any pipe already on the window. ORDERING MATTERS: the reader must open the fifo BEFORE
    /// this is called — `cat`'s open blocks until a reader appears, and tmux buffers pane
    /// output behind a stalled pipe.
    pub fn pipe_pane_named(&self, window: &str, fifo: &Path) -> Result<()> {
        let target = self.exact_target(window);
        let cmd = format!("cat > {}", shell_quote(&fifo.to_string_lossy()));
        self.run(&["pipe-pane", "-t", &target, &cmd])?;
        Ok(())
    }

    /// Stop streaming `window`'s output (tmux's no-command `pipe-pane` form). Benign when the
    /// window — or the whole server — is already gone.
    pub fn pipe_pane_off_named(&self, window: &str) -> Result<()> {
        let target = self.exact_target(window);
        self.run_allow_absent(&["pipe-pane", "-t", &target])?;
        Ok(())
    }

    /// Send a literal string (no trailing Enter) — one keystroke's worth of input.
    pub fn send_literal(&self, lane: LaneId, text: &str) -> Result<()> {
        self.send_literal_named(&Self::window_name(lane), text)
    }

    pub fn send_literal_named(&self, window: &str, text: &str) -> Result<()> {
        tracing::debug!(target: "repomon::tmuxwrite", window = %window, op = "send-literal", text = %text.chars().take(60).collect::<String>(), "tmux write");
        self.run(&["send-keys", "-t", &self.exact_target(window), "-l", text])?;
        Ok(())
    }

    /// Type `text` into the agent and press Enter.
    pub fn send_text(&self, lane: LaneId, text: &str) -> Result<()> {
        self.send_text_named(&Self::window_name(lane), text)
    }

    pub fn send_text_named(&self, window: &str, text: &str) -> Result<()> {
        tracing::debug!(target: "repomon::tmuxwrite", window = %window, op = "send-text", text = %text.chars().take(60).collect::<String>(), "tmux write");
        let target = self.exact_target(window);
        self.run(&["send-keys", "-t", &target, "-l", text])?;
        self.run(&["send-keys", "-t", &target, "Enter"])?;
        Ok(())
    }

    /// Send a raw key (e.g. `C-c`) to the agent.
    pub fn send_key(&self, lane: LaneId, key: &str) -> Result<()> {
        self.send_key_named(&Self::window_name(lane), key)
    }

    pub fn send_key_named(&self, window: &str, key: &str) -> Result<()> {
        tracing::debug!(target: "repomon::tmuxwrite", window = %window, op = "send-key", key = %key, "tmux write");
        self.run(&["send-keys", "-t", &self.exact_target(window), key])?;
        Ok(())
    }

    /// Terminate the agent's first-slot window.
    pub fn kill(&self, lane: LaneId) -> Result<()> {
        self.kill_named(&Self::window_name(lane))
    }

    /// Make the attached experience feel like a native terminal: mouse on (wheel scroll +
    /// drag-select), system-clipboard passthrough, and drag-select copies to the clipboard.
    /// Server-global, so calling it once per session creation is enough (idempotent).
    pub fn configure(&self) {
        let _ = self.run(&["set", "-g", "mouse", "on"]);
        let _ = self.run(&["set", "-g", "set-clipboard", "on"]);
        // History deep enough to scroll back through a long plan.
        let _ = self.run(&["set", "-g", "history-limit", "50000"]);
        // Drag-select pipes into the platform clipboard tool when one exists; otherwise fall
        // back to tmux's own buffer, which `set-clipboard on` above still forwards to the
        // terminal's clipboard via OSC52 on modern emulators.
        let pipe = crate::clipboard::copy_pipe_command();
        for table in ["copy-mode", "copy-mode-vi"] {
            let bind = ["bind", "-T", table, "MouseDragEnd1Pane", "send", "-X"];
            let _ = match &pipe {
                Some(cmd) => {
                    let mut args = bind.to_vec();
                    args.extend(["copy-pipe-and-cancel", cmd.as_str()]);
                    self.run(&args)
                }
                None => {
                    let mut args = bind.to_vec();
                    args.push("copy-selection-and-cancel");
                    self.run(&args)
                }
            };
        }

        // A thin status bar that always shows the way back, so detaching is discoverable.
        let _ = self.run(&["set", "-g", "status", "on"]);
        let _ = self.run(&["set", "-g", "status-interval", "0"]); // static → no idle redraw
        let _ = self.run(&["set", "-g", "status-style", "bg=colour236,fg=colour250"]);
        let _ = self.run(&["set", "-g", "status-left", "#[bold] repomon #[nobold]"]);
        let _ = self.run(&["set", "-g", "status-left-length", "20"]);
        let _ = self.run(&[
            "set",
            "-g",
            "status-right",
            "#[reverse] F12 #[noreverse] or #[reverse] ^B d #[noreverse] back to repomon ",
        ]);
        let _ = self.run(&["set", "-g", "status-right-length", "60"]);

        // Detach keys: F12 leaves with one press (root table); prefix-d is the tmux default;
        // prefix-q is an easy mnemonic. Detach leaves the agent running in the background.
        let _ = self.run(&["bind", "-n", "F12", "detach-client"]);
        let _ = self.run(&["bind", "q", "detach-client"]);
    }

    /// The `session:window` target for an arbitrary named window (e.g. a terminal).
    pub fn target_named(&self, name: &str) -> String {
        format!("{}:{}", self.session, name)
    }

    /// Is there a window with this exact name?
    pub fn has_named(&self, name: &str) -> bool {
        self.list_windows()
            .map(|w| w.iter().any(|x| x == name))
            .unwrap_or(false)
    }

    /// Open a plain interactive shell in `cwd` as a named window (no agent); returns its
    /// target. tmux runs the user's default shell when no command is given.
    pub fn open_named(&self, name: &str, cwd: &Path) -> Result<String> {
        let cwd = cwd.to_string_lossy();
        if self.session_exists() {
            // `-d`: spawn out of the way so opening a terminal never steals an attached client's
            // active window (see `spawn`).
            self.run(&[
                "new-window",
                "-d",
                "-t",
                &self.session,
                "-n",
                name,
                "-c",
                &cwd,
            ])?;
        } else {
            self.run(&[
                "new-session",
                "-d",
                "-x",
                "220",
                "-y",
                "50",
                "-s",
                &self.session,
                "-n",
                name,
                "-c",
                &cwd,
            ])?;
        }
        self.configure();
        Ok(self.target_named(name))
    }

    /// Launch `command` in `cwd` as an arbitrary named window — like [`spawn`](Self::spawn) but
    /// with a caller-chosen window name instead of a lane slot. Used for the hidden `/usage`
    /// probe (`usage-probe-…`), whose non-`lane-` name keeps it out of the lane-window scans.
    /// Returns the window's exact target.
    pub fn spawn_named(&self, name: &str, cwd: &Path, command: &str) -> Result<String> {
        let cwd = cwd.to_string_lossy();
        if self.session_exists() {
            // `-d`: spawn detached. This is the usage probe's path; it spawns then kills a
            // throwaway `usage-probe-…` window every few minutes. Without `-d`, each spawn yanks an
            // attached client to the probe and each kill yanks it back, replaying every window's
            // pane as a flip-book in macOS fullscreen focus (the flap this fixes). See `spawn`.
            self.run(&[
                "new-window",
                "-d",
                "-t",
                &self.session,
                "-n",
                name,
                "-c",
                &cwd,
                command,
            ])?;
        } else {
            self.run(&[
                "new-session",
                "-d",
                "-x",
                "220",
                "-y",
                "50",
                "-s",
                &self.session,
                "-n",
                name,
                "-c",
                &cwd,
                command,
            ])?;
        }
        self.configure();
        Ok(self.exact_target(name))
    }

    /// Terminate a named window (an agent slot or a terminal). Exact-match target, so killing
    /// `lane-1` can't take out `lane-1-2`.
    pub fn kill_named(&self, name: &str) -> Result<()> {
        tracing::debug!(target: "repomon::tmuxwrite", window = %name, op = "kill-window", "tmux write");
        self.run(&["kill-window", "-t", &self.exact_target(name)])?;
        Ok(())
    }

    /// Args for `tmux attach` to this lane (the TUI execs this for a raw session), including
    /// the dedicated socket.
    pub fn attach_args(&self, lane: LaneId) -> Vec<String> {
        vec![
            "-L".into(),
            self.session.clone(),
            "attach".into(),
            "-t".into(),
            self.target(lane),
        ]
    }
}

/// Single-quote a string for safe inclusion in a shell command.
pub fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Render a [`SpawnSpec`] to the single shell command string tmux runs via `sh -c`:
/// `K='v' … program 'arg1' 'arg2'`. The program is the user-configured command line and is
/// passed through verbatim (it may legitimately be a shell fragment); env assignments and
/// extra args are single-quoted via [`shell_quote`].
fn render_spawn_command(spec: &SpawnSpec) -> String {
    let mut cmd = String::new();
    for (k, v) in &spec.env {
        cmd.push_str(k);
        cmd.push('=');
        cmd.push_str(&shell_quote(v));
        cmd.push(' ');
    }
    cmd.push_str(&spec.program);
    for a in &spec.args {
        cmd.push(' ');
        cmd.push_str(&shell_quote(a));
    }
    cmd
}

/// Largest chunk a byte stream delivers per channel message — keeps single `event.agent.bytes`
/// events bounded while a busy pane floods.
const BYTE_STREAM_CHUNK: usize = 16 * 1024;

/// Monotonic tag baked into every byte-stream fifo NAME, so two pipe instances of the same
/// window never share a path — a superseded reader's EOF unlink can then only ever remove its
/// own fifo, never a successor's (see `repomon-daemon`'s `bytes_stream` module doc for the
/// rapid unwatch→rewatch race this kills).
static NEXT_STREAM_TAG: AtomicU64 = AtomicU64::new(0);

impl SessionBackend for TmuxRuntime {
    fn available(&self) -> bool {
        TmuxRuntime::available()
    }

    fn label(&self) -> String {
        self.session.clone()
    }

    fn session_exists(&self) -> bool {
        TmuxRuntime::session_exists(self)
    }

    fn claim_or_verify_owner(&self, me: &str) -> OwnerState {
        if TmuxRuntime::claim_or_verify_owner(self, me) {
            OwnerState::Owned
        } else {
            OwnerState::OwnedByOther
        }
    }

    fn list_windows(&self) -> Result<Vec<String>> {
        TmuxRuntime::list_windows(self)
    }

    fn list_windows_with_activity(&self) -> Result<Vec<WindowActivity>> {
        Ok(TmuxRuntime::list_windows_with_activity(self)?
            .into_iter()
            .map(|(name, cwd, last_activity)| WindowActivity {
                name,
                cwd,
                last_activity,
            })
            .collect())
    }

    fn spawn(&self, lane: LaneId, spec: &SpawnSpec) -> Result<String> {
        TmuxRuntime::spawn(self, lane, &spec.cwd, &render_spawn_command(spec))
    }

    fn spawn_named(&self, window: &str, spec: &SpawnSpec) -> Result<String> {
        TmuxRuntime::spawn_named(self, window, &spec.cwd, &render_spawn_command(spec))
    }

    fn open_named(&self, window: &str, cwd: &Path) -> Result<String> {
        TmuxRuntime::open_named(self, window, cwd)
    }

    fn capture_named(&self, window: &str, opts: CaptureOpts) -> Result<String> {
        TmuxRuntime::capture_named(self, window, opts.last_lines)
    }

    fn cursor_named(&self, window: &str) -> Option<Cursor> {
        TmuxRuntime::cursor_named(self, window).map(|(col, row)| Cursor { col, row })
    }

    fn size_named(&self, window: &str) -> Option<(u16, u16)> {
        TmuxRuntime::size_named(self, window)
    }

    fn resize_named(&self, window: &str, cols: u16, rows: u16) -> Result<()> {
        TmuxRuntime::resize_named(self, window, cols, rows)
    }

    fn follow_client_named(&self, window: &str) -> Result<()> {
        TmuxRuntime::follow_client_named(self, window)
    }

    fn alternate_on_named(&self, window: &str) -> bool {
        TmuxRuntime::alternate_on_named(self, window)
    }

    fn scroll_wheel_named(&self, window: &str, up: bool, ticks: u32) -> Result<()> {
        TmuxRuntime::scroll_wheel_named(self, window, up, ticks)
    }

    fn send_literal_named(&self, window: &str, text: &str) -> Result<()> {
        TmuxRuntime::send_literal_named(self, window, text)
    }

    fn send_text_named(&self, window: &str, text: &str) -> Result<()> {
        TmuxRuntime::send_text_named(self, window, text)
    }

    fn send_key_named(&self, window: &str, key: &str) -> Result<()> {
        TmuxRuntime::send_key_named(self, window, key)
    }

    fn kill_named(&self, window: &str) -> Result<()> {
        TmuxRuntime::kill_named(self, window)
    }

    fn configure(&self) {
        TmuxRuntime::configure(self)
    }

    fn target_named(&self, window: &str) -> String {
        TmuxRuntime::target_named(self, window)
    }

    fn exact_target_named(&self, window: &str) -> String {
        self.exact_target(window)
    }

    fn attach_command(&self, target: &str) -> AttachCommand {
        AttachCommand {
            program: "tmux".to_string(),
            args: vec![
                "-L".to_string(),
                self.session.clone(),
                "attach".to_string(),
                "-t".to_string(),
                target.to_string(),
            ],
        }
    }

    /// The fifo + `pipe-pane` rendezvous, absorbed from the daemon's old `bytes_stream::watch`:
    /// create a uniquely-named fifo, start the reader thread FIRST (its `open()` is the
    /// rendezvous with `cat`'s write-side open — tmux buffers pane output behind a stalled
    /// pipe), then point `pipe-pane` at it. On EOF (pipe turned off or window died) the reader
    /// removes its fifo — inherently safe: the tag-unique name means it can only ever be its
    /// own, never a successor's — and drops the sender, closing the stream.
    fn open_byte_stream(&self, window: &str) -> Result<ByteStream> {
        let tag = NEXT_STREAM_TAG.fetch_add(1, Ordering::Relaxed);
        let fifo = std::env::temp_dir().join(format!(
            "repomon-bytes-{}-{window}-{tag}.fifo",
            self.session
        ));
        // The pre-mkfifo unlink guards the one same-path case left — a leftover fifo from a
        // previous daemon run (the tag counter restarts at 0).
        let _ = std::fs::remove_file(&fifo);
        let ok = Command::new("mkfifo")
            .arg(&fifo)
            .output()
            .map_err(Error::Io)?
            .status
            .success();
        if !ok {
            return Err(Error::Agent(format!("mkfifo {} failed", fifo.display())));
        }
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        {
            let fifo = fifo.clone();
            std::thread::spawn(move || {
                use std::io::Read;
                let Ok(mut f) = std::fs::File::open(&fifo) else {
                    return;
                };
                let mut buf = vec![0u8; BYTE_STREAM_CHUNK];
                loop {
                    match f.read(&mut buf) {
                        Ok(0) | Err(_) => break, // EOF: pipe turned off or window died
                        Ok(n) => {
                            if tx.send(buf[..n].to_vec()).is_err() {
                                break; // consumer gone — stop pumping
                            }
                        }
                    }
                }
                let _ = std::fs::remove_file(&fifo);
            });
        }
        if let Err(e) = self.pipe_pane_named(window, &fifo) {
            // The pipe never started: remove the fifo so it can't linger. (The reader, still
            // blocked on open(), is a pre-existing leak parity — no `cat` ever opens the write
            // side, same as before this moved out of the daemon.)
            let _ = std::fs::remove_file(&fifo);
            return Err(e);
        }
        Ok(ByteStream { rx })
    }

    fn close_byte_stream(&self, window: &str) -> Result<()> {
        self.pipe_pane_off_named(window)
    }
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
    fn renders_spawn_specs_to_sh_command_strings() {
        // Program alone passes through verbatim (it may be a user-configured shell fragment).
        let bare = SpawnSpec::new("env -u CLAUDE_CONFIG_DIR claude", "/tmp");
        assert_eq!(render_spawn_command(&bare), "env -u CLAUDE_CONFIG_DIR claude");
        // Args are single-quoted and appended — byte-identical to the old
        // `format!("{command} {}", shell_quote(task))` assembly.
        let with_task = SpawnSpec::new("claude", "/tmp").arg("fix the bug");
        assert_eq!(render_spawn_command(&with_task), "claude 'fix the bug'");
        let quoted = SpawnSpec::new("claude", "/tmp").arg("it's tricky");
        assert_eq!(render_spawn_command(&quoted), r"claude 'it'\''s tricky'");
        // Env pairs become leading KEY='value' assignments.
        let mut with_env = SpawnSpec::new("claude", "/tmp");
        with_env.env.push(("FOO".into(), "a b".into()));
        assert_eq!(render_spawn_command(&with_env), "FOO='a b' claude");
    }

    #[test]
    fn backend_attach_command_matches_the_tmux_invocation() {
        let rt = TmuxRuntime::new("repomon");
        let cmd = SessionBackend::attach_command(&rt, "repomon:=lane-7");
        assert_eq!(cmd.program, "tmux");
        assert_eq!(
            cmd.args,
            vec!["-L", "repomon", "attach", "-t", "repomon:=lane-7"]
        );
    }

    #[test]
    fn backend_targets_match_the_inherent_formats() {
        let rt = TmuxRuntime::new("repomon");
        assert_eq!(SessionBackend::target_named(&rt, "term-1-1"), "repomon:term-1-1");
        assert_eq!(rt.exact_target_named("lane-7"), "repomon:=lane-7");
        assert_eq!(SessionBackend::label(&rt), "repomon");
    }

    #[test]
    fn target_format() {
        let rt = TmuxRuntime::new("repomon");
        assert_eq!(rt.target(7), "repomon:lane-7");
    }

    #[test]
    fn slot_names_and_lane_window_filtering() {
        assert_eq!(TmuxRuntime::slot_name(7, 1), "lane-7");
        assert_eq!(TmuxRuntime::slot_name(7, 2), "lane-7-2");

        // Exact matching: lane 1 must not claim lane 12's (or a terminal's) windows, and the
        // result comes back in slot order regardless of input order.
        let names: Vec<String> = [
            "lane-12", "lane-1-3", "term-1", "lane-1", "lane-1-2", "lane-1-x",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(
            TmuxRuntime::lane_windows_in(&names, 1),
            vec!["lane-1", "lane-1-2", "lane-1-3"]
        );
        assert_eq!(TmuxRuntime::lane_windows_in(&names, 12), vec!["lane-12"]);
        assert!(TmuxRuntime::lane_windows_in(&names, 3).is_empty());
    }

    #[test]
    fn parses_lane_windows_back_to_id_and_slot() {
        // Base window is slot 1; `-N` suffix is slot N (N >= 2).
        assert_eq!(TmuxRuntime::parse_lane_window("lane-7"), Some((7, 1)));
        assert_eq!(TmuxRuntime::parse_lane_window("lane-7-2"), Some((7, 2)));
        assert_eq!(TmuxRuntime::parse_lane_window("lane-81-3"), Some((81, 3)));
        // Non-lane windows and malformed names are not agent windows.
        assert_eq!(TmuxRuntime::parse_lane_window("term-1"), None);
        assert_eq!(TmuxRuntime::parse_lane_window("usage-probe-work"), None);
        assert_eq!(TmuxRuntime::parse_lane_window("lane-"), None);
        assert_eq!(TmuxRuntime::parse_lane_window("lane-1-x"), None);
        // Slot 1 is only ever spelled `lane-7`, never `lane-7-1`.
        assert_eq!(TmuxRuntime::parse_lane_window("lane-7-1"), None);
    }

    #[test]
    fn lane_id_and_slot_accessors() {
        assert_eq!(TmuxRuntime::lane_id_of("lane-42-2"), Some(42));
        assert_eq!(TmuxRuntime::lane_id_of("lane-42"), Some(42));
        assert_eq!(TmuxRuntime::lane_id_of("term-1"), None);
        assert_eq!(TmuxRuntime::slot_of_window("lane-42"), Some(1));
        assert_eq!(TmuxRuntime::slot_of_window("lane-42-3"), Some(3));
        assert_eq!(TmuxRuntime::slot_of_window("term-1"), None);
    }

    #[test]
    fn parses_terminal_windows_back_to_lane() {
        // `terminal.open` mints `term-{lane}-{n}`; the parse is its inverse.
        assert_eq!(TmuxRuntime::parse_term_window("term-7-1"), Some(7));
        assert_eq!(TmuxRuntime::parse_term_window("term-81-12"), Some(81));
        // Agent windows, sequence-less/malformed names, and strangers are not terminals.
        assert_eq!(TmuxRuntime::parse_term_window("lane-7"), None);
        assert_eq!(TmuxRuntime::parse_term_window("term-7"), None);
        assert_eq!(TmuxRuntime::parse_term_window("term-x-1"), None);
        assert_eq!(TmuxRuntime::parse_term_window("term-7-x"), None);
        assert_eq!(TmuxRuntime::parse_term_window("orchestrator"), None);
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

        // A second spawn runs side by side in the next slot (the first agent survives), and
        // per-window ops hit the right pane even after the first slot goes away.
        rt.spawn(lane, &cwd, "sh -c 'echo SLOT_TWO; sleep 30'")
            .unwrap();
        assert_eq!(rt.windows_for(lane).unwrap(), vec!["lane-1", "lane-1-2"]);
        std::thread::sleep(std::time::Duration::from_millis(400));
        let one = rt.capture(lane, None).unwrap();
        assert!(one.contains("HELLO_REPOMON"), "slot 1 was: {one:?}");
        let two = rt.capture_named("lane-1-2", None).unwrap();
        assert!(two.contains("SLOT_TWO"), "slot 2 was: {two:?}");

        rt.kill(lane).unwrap();
        assert_eq!(rt.windows_for(lane).unwrap(), vec!["lane-1-2"]);
        // Exact targeting: the primary name must not resolve onto the surviving slot.
        assert_eq!(rt.capture(lane, None).unwrap(), "");
        rt.kill_named("lane-1-2").unwrap();
        assert!(!rt.has_window(lane));

        // Tear down the test session.
        let _ = Command::new("tmux")
            .args(["kill-session", "-t", rt.session()])
            .output();
    }

    #[test]
    fn pipe_pane_streams_raw_bytes_to_a_fifo() {
        if !TmuxRuntime::available() {
            eprintln!("tmux not available; skipping live runtime test");
            return;
        }
        let rt = TmuxRuntime::new(format!("repomon-pipetest-{}", std::process::id()));
        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("bytes.fifo");
        assert!(
            Command::new("mkfifo")
                .arg(&fifo)
                .output()
                .unwrap()
                .status
                .success(),
            "mkfifo"
        );

        rt.spawn(1, dir.path(), "sh -c 'sleep 30'").unwrap();

        // Reader FIRST (cat's open blocks until one appears), then the pipe, then output.
        let reader = {
            let fifo = fifo.clone();
            std::thread::spawn(move || {
                use std::io::Read;
                let mut f = std::fs::File::open(fifo).unwrap();
                let mut buf = [0u8; 4096];
                let mut got = String::new();
                // Read until the marker shows up (bounded by the test timeout).
                while !got.contains("PIPE_BYTES_MARKER") {
                    let n = f.read(&mut buf).unwrap();
                    if n == 0 {
                        break;
                    }
                    got.push_str(&String::from_utf8_lossy(&buf[..n]));
                }
                got
            })
        };
        rt.pipe_pane_named("lane-1", &fifo).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));
        rt.send_text_named("lane-1", "echo PIPE_BYTES_MARKER")
            .unwrap();

        let got = reader.join().unwrap();
        // The stream is the raw PTY byte flow: the echoed command AND its output pass through.
        assert!(got.contains("PIPE_BYTES_MARKER"), "stream was: {got:?}");

        rt.pipe_pane_off_named("lane-1").unwrap();
        rt.kill_named("lane-1").unwrap();
        let _ = Command::new("tmux")
            .args(["-L", rt.session(), "kill-server"])
            .output();
    }

    #[test]
    fn single_owner_guard_claims_then_blocks_others() {
        if !TmuxRuntime::available() {
            eprintln!("tmux not available; skipping live runtime test");
            return;
        }
        let rt = TmuxRuntime::new(format!("repomon-ownertest-{}", std::process::id()));
        // A server must exist before server options can be set — spawn a throwaway window.
        rt.spawn(1, &std::env::temp_dir(), "sh -c 'sleep 30'")
            .unwrap();

        // First caller claims the server and keeps verifying true on re-check (restart-safe).
        assert!(
            rt.claim_or_verify_owner("daemon-A"),
            "first claim should win"
        );
        assert!(
            rt.claim_or_verify_owner("daemon-A"),
            "owner re-verifies true"
        );
        // A different daemon sharing the server (a stray test instance) is locked out of reaping.
        assert!(
            !rt.claim_or_verify_owner("daemon-B"),
            "non-owner must back off"
        );
        // The original owner is unaffected by the other's attempt.
        assert!(
            rt.claim_or_verify_owner("daemon-A"),
            "owner still owns after B's attempt"
        );

        let _ = Command::new("tmux")
            .args(["-L", rt.session(), "kill-server"])
            .output();
    }

    /// The session's currently-active window name (the one an attached `tmux attach` client
    /// displays). `None` if the server is gone.
    fn active_window(rt: &TmuxRuntime) -> Option<String> {
        // Use the runtime's own helper (same dedicated `-L` socket + benign-absence handling as
        // production) rather than shelling out to tmux directly.
        rt.run_allow_absent(&[
            "list-windows",
            "-t",
            rt.session(),
            "-F",
            "#{window_active} #{window_name}",
        ])
        .ok()?
        .lines()
        .find_map(|l| l.strip_prefix("1 ").map(str::to_string))
    }

    #[test]
    fn spawning_a_window_does_not_steal_the_active_window() {
        if !TmuxRuntime::available() {
            eprintln!("tmux not available; skipping live runtime test");
            return;
        }
        let rt = TmuxRuntime::new(format!("repomon-activetest-{}", std::process::id()));
        let cwd = std::env::temp_dir();

        // The window a human is "attached" to (their focused agent).
        rt.spawn(1, &cwd, "sh -c 'sleep 30'").unwrap();
        assert_eq!(active_window(&rt).as_deref(), Some("lane-1"));

        // Spawning a side-by-side lane agent must leave lane-1 active, so an attached client is
        // not yanked to the new window.
        rt.spawn(2, &cwd, "sh -c 'sleep 30'").unwrap();
        assert_eq!(
            active_window(&rt).as_deref(),
            Some("lane-1"),
            "a freshly spawned lane window stole the session's active window"
        );

        // The usage-probe path (`spawn_named`) is the real flap trigger: it spawns then kills a
        // throwaway window every few minutes. Neither the spawn nor the kill may move the active
        // window, or the attached client replays the probe's pane (the fullscreen flip-book).
        rt.spawn_named("usage-probe-work", &cwd, "sh -c 'sleep 30'")
            .unwrap();
        assert_eq!(
            active_window(&rt).as_deref(),
            Some("lane-1"),
            "a usage-probe window stole the session's active window"
        );
        rt.kill_named("usage-probe-work").unwrap();
        assert_eq!(
            active_window(&rt).as_deref(),
            Some("lane-1"),
            "killing the usage-probe window moved the active window"
        );

        // A plain terminal window (`open_named`) must also spawn out of the way.
        rt.open_named("term-1", &cwd).unwrap();
        assert_eq!(
            active_window(&rt).as_deref(),
            Some("lane-1"),
            "a terminal window stole the session's active window"
        );

        let _ = Command::new("tmux")
            .args(["-L", rt.session(), "kill-server"])
            .output();
    }
}
