//! Owns durable agent windows through tmux so agents outlive the daemon and clients; synchronous
//! operations belong on blocking threads.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::error::{Error, Result};
use crate::model::LaneId;

use super::backend::{
    AttachCommand, ByteStream, ByteStreamEvent, CaptureOpts, Cursor, OwnerState, ScrollEvent,
    SessionBackend, SpawnSpec, WindowActivity,
};

/// A handle to a managed tmux session. Cheap to clone.
#[derive(Clone, Debug)]
pub struct TmuxRuntime {
    session: String,
    streams: Arc<Mutex<std::collections::HashMap<String, ActiveControlStream>>>,
}

#[derive(Debug)]
struct ActiveControlStream {
    tag: u64,
    input: ChildStdin,
}

/// One window as the overlay probes it ([`TmuxRuntime::list_windows_meta`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowMeta {
    pub name: String,
    /// Parsed from `#{window_id}` (`@12` → 12). tmux never reuses window ids within a server,
    /// so ordering by id is true creation order even when slot NAMES are recycled after an
    /// exit (`spawn` fills the first free slot). `u64::MAX` when unparsable (sorts last).
    pub wid: u64,
    /// The transcript session id bound to this window via the `@repomon_session` window
    /// option ([`TmuxRuntime::set_window_session`]), if any. tmux destroys window options
    /// with the window, so a binding can never outlive its agent.
    pub session: Option<String>,
    /// The agent kind bound to this window via the `@repomon_agent_kind` window option
    /// ([`TmuxRuntime::set_window_agent_kind`]), if any.
    pub agent_kind: Option<String>,
}

/// A printable sentinel survives tmux vis-sanitization in the C/POSIX locale, unlike tabs; probe
/// fields do not contain this sequence.
const PROBE_FIELD_SEP: &str = "%#%";

/// The roomy grid every agent pane gets at spawn (and a brand-new session via `-x/-y`). Agents
/// render their TUIs for wide terminals; a fresh pane must never inherit whatever small size the
/// session happens to be in.
pub const DEFAULT_PANE_COLS: u16 = 220;
pub const DEFAULT_PANE_ROWS: u16 = 50;

/// The smallest usable mediated-view grid. Below this an agent's TUI is unreadable and some
/// (observed with opencode) misbehave outright, so `agent.resize` / `agent.fit` clamps to here
/// rather than letting a momentary tiny client layout shrink a real agent to nothing.
pub const MIN_PANE_COLS: u16 = 80;
pub const MIN_PANE_ROWS: u16 = 24;

/// Clamp a requested pane grid to the sane floor ([`MIN_PANE_COLS`] × [`MIN_PANE_ROWS`]). Pure
/// so the floor is unit-testable without a tmux server.
pub fn clamp_pane_size(cols: u16, rows: u16) -> (u16, u16) {
    (cols.max(MIN_PANE_COLS), rows.max(MIN_PANE_ROWS))
}

/// Where a resolved tmux binary originates and what path was resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTmux {
    pub path: PathBuf,
    pub source: crate::model::TmuxDoctorSource,
}

/// Resolve the tmux binary path and its source against a custom environment, custom PATH, and candidate directories.
pub fn resolve_tmux_from(
    env_override: Option<&str>,
    path_var: Option<&std::ffi::OsStr>,
    sibling_dirs: &[PathBuf],
) -> Option<ResolvedTmux> {
    if let Some(val) = env_override {
        let trimmed = val.trim();
        if !trimmed.is_empty() {
            return Some(ResolvedTmux {
                path: PathBuf::from(trimmed),
                source: crate::model::TmuxDoctorSource::System,
            });
        }
    }

    let path_candidate = match path_var {
        Some(p) => crate::exec::find_in(p, "tmux"),
        None => crate::exec::find_in_path("tmux"),
    };
    if let Some(p) = path_candidate {
        return Some(ResolvedTmux {
            path: p,
            source: crate::model::TmuxDoctorSource::System,
        });
    }

    // 3. Bundled sidecar tmux binary next to running executable or repomond
    for dir in sibling_dirs {
        let cand = dir.join(format!("tmux{}", std::env::consts::EXE_SUFFIX));
        if cand.is_file() {
            return Some(ResolvedTmux {
                path: cand,
                source: crate::model::TmuxDoctorSource::Bundled,
            });
        }
    }

    None
}

/// Resolves tmux without caching, preferring REPOMON_TMUX, then PATH, then bundled candidates.
pub fn resolve_tmux_uncached() -> Option<ResolvedTmux> {
    let env_override = std::env::var("REPOMON_TMUX").ok();
    let mut sibling_dirs = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            sibling_dirs.push(dir.to_path_buf());
        }
    }
    let repomond_cand = crate::service::repomond_path();
    if let Some(dir) = repomond_cand.parent() {
        sibling_dirs.push(dir.to_path_buf());
    }
    resolve_tmux_from(env_override.as_deref(), None, &sibling_dirs)
}

/// The cached or freshly resolved tmux binary and source.
pub fn resolved_tmux() -> Option<ResolvedTmux> {
    // If REPOMON_TMUX is set, don't lock on static cache so test environment overrides work dynamically
    if let Ok(val) = std::env::var("REPOMON_TMUX") {
        let trimmed = val.trim();
        if !trimmed.is_empty() {
            return Some(ResolvedTmux {
                path: PathBuf::from(trimmed),
                source: crate::model::TmuxDoctorSource::System,
            });
        }
    }
    static CACHE: std::sync::OnceLock<Option<ResolvedTmux>> = std::sync::OnceLock::new();
    CACHE.get_or_init(resolve_tmux_uncached).clone()
}

/// The program path to invoke for tmux commands.
pub fn tmux_program() -> PathBuf {
    resolved_tmux()
        .map(|r| r.path)
        .unwrap_or_else(|| PathBuf::from("tmux"))
}

/// Supply a UTF-8 locale only when no locale variable is set, preventing tmux from sanitizing
/// captured output while respecting explicit user choices.
fn locale_override(
    lc_all: Option<&str>,
    lc_ctype: Option<&str>,
    lang: Option<&str>,
) -> Option<(&'static str, &'static str)> {
    if lc_all.is_none() && lc_ctype.is_none() && lang.is_none() {
        Some(("LC_ALL", "en_US.UTF-8"))
    } else {
        None
    }
}

/// [`locale_override`] applied to the daemon's actual environment - the value `run`/
/// `run_allow_absent` add to the tmux client's `Command` when set.
fn locale_env() -> Option<(&'static str, &'static str)> {
    locale_override(
        std::env::var("LC_ALL").ok().as_deref(),
        std::env::var("LC_CTYPE").ok().as_deref(),
        std::env::var("LANG").ok().as_deref(),
    )
}

impl TmuxRuntime {
    pub fn new(session: impl Into<String>) -> Self {
        Self {
            session: session.into(),
            streams: Arc::new(Mutex::new(std::collections::HashMap::new())),
        }
    }

    /// Return the resolved tmux program path.
    pub fn tmux_program() -> PathBuf {
        tmux_program()
    }

    /// Probe tmux availability, version, source, and path.
    pub fn probe() -> crate::model::TmuxDoctorInfo {
        match resolve_tmux_uncached() {
            Some(resolved) => match Command::new(&resolved.path).arg("-V").output() {
                Ok(out) if out.status.success() => {
                    let version_str = String::from_utf8_lossy(&out.stdout).trim().to_string();
                    crate::model::TmuxDoctorInfo {
                        available: true,
                        version: if version_str.is_empty() {
                            None
                        } else {
                            Some(version_str)
                        },
                        source: Some(resolved.source),
                        path: Some(resolved.path.to_string_lossy().into_owned()),
                        not_applicable: false,
                    }
                }
                _ => crate::model::TmuxDoctorInfo {
                    available: false,
                    version: None,
                    source: None,
                    path: Some(resolved.path.to_string_lossy().into_owned()),
                    not_applicable: false,
                },
            },
            None => crate::model::TmuxDoctorInfo {
                available: false,
                version: None,
                source: None,
                path: None,
                not_applicable: false,
            },
        }
    }

    /// Is tmux installed and runnable?
    pub fn available() -> bool {
        Self::probe().available
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

    /// Parses an exact managed lane-window name into its lane and slot, returning `None` for
    /// terminal, probe, and malformed names.
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
    /// into its lane. `None` for anything else - agent windows, the usage probe, malformed
    /// names - so terminal scans and agent scans stay mutually blind.
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

    /// An *exact* `session:=window` target - tmux otherwise prefix-matches window names, which
    /// would let `lane-1` resolve to `lane-1-2` once the first slot is gone.
    fn exact_target(&self, name: &str) -> String {
        format!("{}:={}", self.session, name)
    }

    /// repomon runs its tmux on a dedicated socket (named after the session) so its windows
    /// never collide with - or share a server with - the user's own tmux.
    fn full_args<'a>(&'a self, args: &'a [&'a str]) -> Vec<&'a str> {
        let mut full = vec!["-L", self.session.as_str()];
        full.extend_from_slice(args);
        full
    }

    fn run(&self, args: &[&str]) -> Result<String> {
        let mut cmd = Command::new(tmux_program());
        cmd.args(self.full_args(args));
        if let Some((key, value)) = locale_env() {
            cmd.env(key, value);
        }
        let out = cmd.output().map_err(Error::Io)?;
        if !out.status.success() {
            return Err(Error::Agent(format!(
                "tmux {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    /// Treat missing targets as empty output without a separate preflight process, while
    /// propagating other backend failures.
    fn run_allow_absent(&self, args: &[&str]) -> Result<String> {
        let mut cmd = Command::new(tmux_program());
        cmd.args(self.full_args(args));
        if let Some((key, value)) = locale_env() {
            cmd.env(key, value);
        }
        let out = cmd.output().map_err(Error::Io)?;
        if out.status.success() {
            return Ok(String::from_utf8_lossy(&out.stdout).into_owned());
        }
        let stderr = String::from_utf8_lossy(&out.stderr);
        // Target disappearance is benign, including "no current target" when the last pane exits
        // before explicit teardown.
        let absent = stderr.contains("can't find ")
            || stderr.contains("no server running")
            || stderr.contains("error connecting")
            || stderr.contains("no such window")
            || stderr.contains("no such session")
            || stderr.contains("no current target");
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
        Command::new(tmux_program())
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

    /// Claims this tmux server for the database identity or verifies existing ownership, preventing
    /// another daemon’s orphan sweep from deleting its windows.
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
        // No `has-session` preflight - `run_allow_absent` turns "no server / can't find session"
        // into an empty list, saving a fork on every call (overlay_agents, auto_continue, …).
        let out =
            self.run_allow_absent(&["list-windows", "-t", &self.session, "-F", "#{window_name}"])?;
        Ok(out.lines().map(str::to_string).collect())
    }

    /// Fingerprint the pane's root process. The tmux pane PID survives a repomond restart, while
    /// the start time prevents a recycled PID from looking like the same agent.
    pub fn window_process_fingerprint(&self, window: &str) -> Result<Option<String>> {
        let target = self.exact_target(window);
        let out =
            self.run_allow_absent(&["display-message", "-p", "-t", &target, "#{pane_pid}"])?;
        let Some(pid) = out.trim().parse::<u32>().ok() else {
            return Ok(None);
        };
        Ok(process_fingerprint(pid).map(|start| format!("{pid}:{start}")))
    }

    /// Returns window names, working directories, and activity timestamps for orphan detection and
    /// active-window protection.
    pub fn list_windows_with_activity(&self) -> Result<Vec<(String, PathBuf, i64)>> {
        let fmt = format!(
            "#{{window_name}}{sep}#{{pane_current_path}}{sep}#{{window_activity}}",
            sep = PROBE_FIELD_SEP
        );
        let out = self.run_allow_absent(&["list-windows", "-t", &self.session, "-F", &fmt])?;
        Ok(Self::parse_windows_activity(&out))
    }

    /// Parse `list_windows_with_activity` probe lines (`name%#%cwd%#%activity`).
    fn parse_windows_activity(out: &str) -> Vec<(String, PathBuf, i64)> {
        out.lines()
            .filter_map(|l| {
                let mut it = l.splitn(3, PROBE_FIELD_SEP);
                let name = it.next()?.to_string();
                let path = PathBuf::from(it.next()?);
                let activity = it
                    .next()
                    .and_then(|s| s.trim().parse::<i64>().ok())
                    .unwrap_or(0);
                Some((name, path, activity))
            })
            .collect()
    }

    /// One window as the overlay probes it: name, tmux's window id, the transcript
    /// session id stuck to it via the `@repomon_session` window option, and the agent kind
    /// option `@repomon_agent_kind`, if bound.
    pub fn list_windows_meta(&self) -> Result<Vec<WindowMeta>> {
        // Same single fork the overlay already pays for `list_windows`, richer format string.
        let fmt = format!(
            "#{{window_name}}{sep}#{{window_id}}{sep}#{{@repomon_session}}{sep}#{{@repomon_agent_kind}}",
            sep = PROBE_FIELD_SEP
        );
        let out = self.run_allow_absent(&["list-windows", "-t", &self.session, "-F", &fmt])?;
        Ok(Self::parse_windows_meta(&out))
    }

    /// Parse `list_windows_meta` probe lines (`name%#%@id%#%session?%#%agent_kind?`).
    fn parse_windows_meta(out: &str) -> Vec<WindowMeta> {
        out.lines()
            .filter_map(|l| {
                let mut it = l.splitn(4, PROBE_FIELD_SEP);
                let name = it.next()?.to_string();
                let wid = it
                    .next()
                    .and_then(|w| w.strip_prefix('@'))
                    .and_then(|n| n.parse().ok())
                    .unwrap_or(u64::MAX);
                let session = it
                    .next()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(String::from);
                let agent_kind = it
                    .next()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(String::from);
                Some(WindowMeta {
                    name,
                    wid,
                    session,
                    agent_kind,
                })
            })
            .collect()
    }

    /// Stamps a known new window’s transcript identity by name, treating disappearance as a benign
    /// no-op.
    pub fn set_window_session(&self, window: &str, session_id: &str) -> Result<()> {
        let target = self.exact_target(window);
        self.run_allow_absent(&[
            "set-option",
            "-w",
            "-t",
            &target,
            "@repomon_session",
            session_id,
        ])?;
        Ok(())
    }

    /// Stamps transcript identity by non-recycled window ID so a reused slot name cannot inherit
    /// another session’s binding.
    pub fn set_window_session_by_id(&self, wid: u64, session_id: &str) -> Result<()> {
        let target = format!("@{wid}");
        self.run_allow_absent(&[
            "set-option",
            "-w",
            "-t",
            &target,
            "@repomon_session",
            session_id,
        ])?;
        Ok(())
    }

    /// Stamp `@repomon_agent_kind` on `window` by NAME - for callers that just created the
    /// window and know which agent kind runs in it (`agent.spawn`, `agent.adopt`).
    pub fn set_window_agent_kind(&self, window: &str, kind: &str) -> Result<()> {
        let target = self.exact_target(window);
        self.run_allow_absent(&[
            "set-option",
            "-w",
            "-t",
            &target,
            "@repomon_agent_kind",
            kind,
        ])?;
        Ok(())
    }

    /// Stamp `@repomon_agent_kind` by window ID (`@N`) - the overlay binder's write-back.
    pub fn set_window_agent_kind_by_id(&self, wid: u64, kind: &str) -> Result<()> {
        let target = format!("@{wid}");
        self.run_allow_absent(&[
            "set-option",
            "-w",
            "-t",
            &target,
            "@repomon_agent_kind",
            kind,
        ])?;
        Ok(())
    }

    /// [`lane_windows_in`] for metas: `lane`'s agent windows, in slot order.
    pub fn lane_windows_meta(metas: &[WindowMeta], lane: LaneId) -> Vec<WindowMeta> {
        let mut slots: Vec<(usize, WindowMeta)> = metas
            .iter()
            .filter_map(|m| {
                let (id, slot) = Self::parse_lane_window(&m.name)?;
                (id == lane).then(|| (slot, m.clone()))
            })
            .collect();
        slots.sort_by_key(|(s, _)| *s);
        slots.into_iter().map(|(_, m)| m).collect()
    }

    pub fn has_window(&self, lane: LaneId) -> bool {
        self.list_windows()
            .map(|w| w.contains(&Self::window_name(lane)))
            .unwrap_or(false)
    }

    /// Launch `command` for `lane` in `cwd` in the lane's first *free* agent slot - a running
    /// agent is never killed, so spawning again runs a second agent side by side. Returns the
    /// bare window name accepted by the named-window operations.
    pub fn spawn(&self, lane: LaneId, cwd: &Path, command: &str) -> Result<String> {
        let taken = self.windows_for(lane).unwrap_or_default();
        // Allocate above the highest live slot to preserve spawn order across gaps, matching the
        // Windows allocator.
        let next = taken
            .last()
            .and_then(|name| Self::slot_of_window(name))
            .unwrap_or(0)
            + 1;
        let window = Self::slot_name(lane, next);
        let cwd = cwd.to_string_lossy();
        if self.session_exists() {
            // Detached creation preserves an attached human’s focused window.
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
            let cols = DEFAULT_PANE_COLS.to_string();
            let rows = DEFAULT_PANE_ROWS.to_string();
            self.run(&[
                "new-session",
                "-d",
                "-x",
                &cols,
                "-y",
                &rows,
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
        // A fresh window can inherit a tiny session grid; set a usable initial size before the
        // agent’s first paint.
        let _ = self.resize_named(&window, DEFAULT_PANE_COLS, DEFAULT_PANE_ROWS);
        Ok(window)
    }

    /// Capture the pane's text, including ANSI color escapes (`-e`).
    pub fn capture(&self, lane: LaneId, lines: Option<u32>) -> Result<String> {
        self.capture_named(&Self::window_name(lane), lines)
    }

    /// Capture a specific agent window's pane text.
    pub fn capture_named(&self, window: &str, lines: Option<u32>) -> Result<String> {
        // No `has_named` preflight (which itself forked `has-session` + `list-windows`): capture
        // directly and let `run_allow_absent` map a vanished window to empty output. Each capture
        // is now ONE fork instead of three - the dominant streamer hot path.
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

    /// Verify the returned window name before trusting its PID: tmux can resolve a missing exact
    /// target to the current window, which must never be terminated instead.
    #[cfg(unix)]
    fn pane_pid(&self, window: &str) -> Option<u32> {
        let out = self
            .run_allow_absent(&[
                "display-message",
                "-p",
                "-t",
                &self.exact_target(window),
                "-F",
                "#{window_name} #{pane_pid}",
            ])
            .ok()?;
        let (name, pid) = out.trim().split_once(' ')?;
        if name != window {
            return None;
        }
        pid.parse().ok()
    }

    /// Collect a process and all descendants using the platform's process table.
    #[cfg(unix)]
    fn process_tree(root: u32) -> Vec<u32> {
        fn children_of(pid: u32, out: &mut Vec<u32>) {
            let pid_arg = pid.to_string();
            let Ok(output) = Command::new("pgrep").args(["-P", &pid_arg]).output() else {
                return;
            };
            for child in String::from_utf8_lossy(&output.stdout)
                .lines()
                .filter_map(|line| line.trim().parse::<u32>().ok())
            {
                out.push(child);
                children_of(child, out);
            }
        }

        let mut pids = vec![root];
        children_of(root, &mut pids);
        pids
    }

    /// Signal descendants while the pane tree is still discoverable because killing its shell alone
    /// can leave surviving CLI children.
    fn terminate_pane_processes(&self, window: &str) {
        #[cfg(unix)]
        {
            if let Some(root) = self.pane_pid(window) {
                // Children first: the pane shell should not disappear before its descendants have
                // received the explicit termination signal.
                for pid in Self::process_tree(root).into_iter().rev() {
                    let pid_arg = pid.to_string();
                    let _ = Command::new("kill")
                        .args(["-TERM", &pid_arg])
                        .stderr(Stdio::null())
                        .output();
                }
            }
        }

        #[cfg(not(unix))]
        let _ = window;
    }

    /// Whether `window`'s app is on the *alternate screen* - i.e. a full-screen TUI (Claude, vim, …)
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
    /// its own history (the mediated pane can't otherwise - alternate-screen apps keep no tmux
    /// scrollback). Sends SGR wheel sequences (button 64 = up, 65 = down) at the pointer cell.
    pub fn scroll_wheel_named(&self, window: &str, event: ScrollEvent) -> Result<()> {
        if event.ticks == 0 {
            return Ok(());
        }
        let button = if event.up { 64 } else { 65 };
        let col = event.col.max(1);
        let row = event.row.max(1);
        let seq = format!("\x1b[<{button};{col};{row}M").repeat(event.ticks as usize);
        let target = self.exact_target(window);
        self.run_allow_absent(&["send-keys", "-t", &target, "-l", &seq])?;
        Ok(())
    }

    /// Streams raw pane output to an existing FIFO, replacing any previous pipe; open the reader
    /// first because a blocked FIFO open stalls pane output.
    pub fn pipe_pane_named(&self, window: &str, fifo: &Path) -> Result<()> {
        let target = self.exact_target(window);
        let cmd = format!("cat > {}", shell_quote(&fifo.to_string_lossy()));
        self.run(&["pipe-pane", "-t", &target, &cmd])?;
        Ok(())
    }

    /// Stop streaming `window`'s output (tmux's no-command `pipe-pane` form). Benign when the
    /// window - or the whole server - is already gone.
    pub fn pipe_pane_off_named(&self, window: &str) -> Result<()> {
        let target = self.exact_target(window);
        self.run_allow_absent(&["pipe-pane", "-t", &target])?;
        Ok(())
    }

    /// Send a literal string (no trailing Enter) - one keystroke's worth of input.
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
        // Allow the paste-burst detector to settle before Enter so the agent treats it as
        // submission rather than pasted text.
        std::thread::sleep(std::time::Duration::from_millis(80));
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
        // Keep the server alive through transient periods with no windows, including probe
        // teardown.
        let _ = self.run(&["set", "-g", "exit-empty", "off"]);
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

    /// Launches a caller-named window and returns its exact target, allowing probes outside the
    /// managed lane namespace.
    pub fn spawn_named(&self, name: &str, cwd: &Path, command: &str) -> Result<String> {
        let cwd = cwd.to_string_lossy();
        if self.session_exists() {
            // Spawn probes detached so creation and teardown cannot change an attached client’s
            // focus.
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
    /// `lane-1` can't take out `lane-1-2`. Signal the pane process tree first so a CLI child
    /// cannot survive the window and become an orphan reparented to PID 1.
    pub fn kill_named(&self, name: &str) -> Result<()> {
        self.terminate_pane_processes(name);
        tracing::debug!(target: "repomon::tmuxwrite", window = %name, op = "kill-window", "tmux write");
        // Signalling the pane root can make the shell exit before tmux processes this command;
        // that is already a successful teardown, so treat a vanished window as benign here.
        self.run_allow_absent(&["kill-window", "-t", &self.exact_target(name)])?;
        Ok(())
    }
}

/// Single-quote a string for safe inclusion in a shell command.
pub fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Marks tmux inapplicable on Windows, where the bundled ConPTY host provides the agent runtime.
pub fn tmux_doctor_for_platform(
    platform: crate::model::DoctorPlatform,
    mut raw: crate::model::TmuxDoctorInfo,
) -> crate::model::TmuxDoctorInfo {
    raw.not_applicable = platform == crate::model::DoctorPlatform::Windows;
    raw
}

/// Render the configured shell fragment with quoted arguments and environment assignments,
/// unsetting ambient NO_COLOR so spawned agents retain color unless explicitly overridden.
fn render_spawn_command(spec: &SpawnSpec) -> String {
    let mut cmd = String::from("env -u NO_COLOR ");
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

/// Monotonic identity for control clients, guarding an old reader's EOF cleanup from removing a
/// replacement stream for the same window.
static NEXT_STREAM_TAG: AtomicU64 = AtomicU64::new(0);

fn decode_control_output(value: &[u8]) -> Option<Vec<u8>> {
    let mut decoded = Vec::with_capacity(value.len());
    let mut index = 0;
    while index < value.len() {
        if value[index] != b'\\' {
            decoded.push(value[index]);
            index += 1;
            continue;
        }
        let digits = value.get(index + 1..index + 4)?;
        if !digits.iter().all(|byte| (b'0'..=b'7').contains(byte)) {
            return None;
        }
        let value = u16::from(digits[0] - b'0') * 64
            + u16::from(digits[1] - b'0') * 8
            + u16::from(digits[2] - b'0');
        decoded.push(u8::try_from(value).ok()?);
        index += 4;
    }
    Some(decoded)
}

fn control_layout_grid(layout: &str) -> Option<(u16, u16)> {
    let dimensions = layout.split(',').nth(1)?;
    let (cols, rows) = dimensions.split_once('x')?;
    Some((cols.parse().ok()?, rows.parse().ok()?))
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ControlEvent {
    Stream(ByteStreamEvent),
    Closed,
}

fn parse_control_event(line: &[u8], window_id: &str, pane_id: &str) -> Option<ControlEvent> {
    if let Some(rest) = line.strip_prefix(b"%output ") {
        let split = rest.iter().position(|byte| *byte == b' ')?;
        if &rest[..split] != pane_id.as_bytes() {
            return None;
        }
        return decode_control_output(&rest[split + 1..])
            .map(ByteStreamEvent::Bytes)
            .map(ControlEvent::Stream);
    }
    let text = std::str::from_utf8(line).ok()?;
    if ["%unlinked-window-close ", "%window-close "]
        .iter()
        .any(|prefix| {
            text.strip_prefix(prefix)
                .and_then(|rest| rest.split_whitespace().next())
                == Some(window_id)
        })
    {
        return Some(ControlEvent::Closed);
    }
    let rest = text.strip_prefix("%layout-change ")?;
    let mut fields = rest.split_whitespace();
    if fields.next()? != window_id {
        return None;
    }
    let (cols, rows) = control_layout_grid(fields.next()?)?;
    Some(ControlEvent::Stream(ByteStreamEvent::Grid { cols, rows }))
}

fn process_fingerprint(pid: u32) -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let after_comm = stat.rsplit_once(") ")?.1;
        // `/proc/<pid>/stat` field 22 is starttime; after the comm field, field 3 is index 0.
        after_comm.split_whitespace().nth(19).map(str::to_string)
    }

    #[cfg(all(unix, not(target_os = "linux")))]
    {
        let output = Command::new("ps")
            .args(["-p", &pid.to_string(), "-o", "lstart="])
            .output()
            .ok()?;
        String::from_utf8(output.stdout)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    }

    #[cfg(not(unix))]
    {
        let _ = pid;
        None
    }
}

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

    fn window_process_fingerprint(&self, window: &str) -> Result<Option<String>> {
        TmuxRuntime::window_process_fingerprint(self, window)
    }

    fn list_windows_meta(&self) -> Result<Vec<WindowMeta>> {
        TmuxRuntime::list_windows_meta(self)
    }

    fn set_window_session(&self, window: &str, session_id: &str) -> Result<()> {
        TmuxRuntime::set_window_session(self, window, session_id)
    }

    fn set_window_session_by_id(&self, wid: u64, session_id: &str) -> Result<()> {
        TmuxRuntime::set_window_session_by_id(self, wid, session_id)
    }

    fn set_window_agent_kind(&self, window: &str, kind: &str) -> Result<()> {
        TmuxRuntime::set_window_agent_kind(self, window, kind)
    }

    fn set_window_agent_kind_by_id(&self, wid: u64, kind: &str) -> Result<()> {
        TmuxRuntime::set_window_agent_kind_by_id(self, wid, kind)
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

    fn scroll_wheel_named(&self, window: &str, event: ScrollEvent) -> Result<()> {
        TmuxRuntime::scroll_wheel_named(self, window, event)
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
            program: tmux_program().to_string_lossy().into_owned(),
            args: vec![
                "-L".to_string(),
                self.session.clone(),
                "attach".to_string(),
                "-t".to_string(),
                target.to_string(),
            ],
        }
    }

    /// Observe grid and output changes on one ordered control stream without participating in
    /// pane-size arbitration.
    fn open_byte_stream(&self, window: &str) -> Result<ByteStream> {
        let tag = NEXT_STREAM_TAG.fetch_add(1, Ordering::Relaxed);
        let target = self.exact_target(window);
        let ids = self.run(&[
            "display-message",
            "-p",
            "-t",
            &target,
            "-F",
            "#{window_id} #{pane_id}",
        ])?;
        let mut ids = ids.split_whitespace();
        let window_id = ids
            .next()
            .ok_or_else(|| Error::Agent(format!("window id unavailable for {window}")))?
            .to_string();
        let pane_id = ids
            .next()
            .ok_or_else(|| Error::Agent(format!("pane id unavailable for {window}")))?
            .to_string();

        // Clear an existing pipe-pane before control mode so an unread pipe cannot accumulate
        // output.
        let _ = self.pipe_pane_off_named(window);
        let mut command = Command::new(tmux_program());
        command
            .args([
                "-L",
                self.session.as_str(),
                "-C",
                "attach-session",
                "-f",
                "ignore-size",
                "-t",
                target.as_str(),
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        if let Some((key, value)) = locale_env() {
            command.env(key, value);
        }
        let mut child = command.spawn().map_err(Error::Io)?;
        let input = child
            .stdin
            .take()
            .ok_or_else(|| Error::Agent("tmux control stdin unavailable".into()))?;
        let output = child
            .stdout
            .take()
            .ok_or_else(|| Error::Agent("tmux control stdout unavailable".into()))?;
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();

        let mut streams = self.streams.lock().expect("control streams lock");
        if let Some(mut old) = streams.remove(window) {
            let _ = old.input.write_all(b"detach-client\n");
            let _ = old.input.flush();
        }
        streams.insert(window.to_string(), ActiveControlStream { tag, input });
        drop(streams);

        let streams = self.streams.clone();
        let stream_window = window.to_string();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(output);
            let mut line = Vec::new();
            loop {
                line.clear();
                let Ok(read) = reader.read_until(b'\n', &mut line) else {
                    break;
                };
                if read == 0 {
                    break;
                }
                while matches!(line.last(), Some(b'\n' | b'\r')) {
                    line.pop();
                }
                match parse_control_event(&line, &window_id, &pane_id) {
                    Some(ControlEvent::Stream(event)) => match tx.send(event) {
                        Ok(()) => {}
                        Err(_) => break,
                    },
                    Some(ControlEvent::Closed) => break,
                    _ => {}
                }
            }
            let _ = child.kill();
            let _ = child.wait();
            let mut streams = streams.lock().expect("control streams lock");
            if streams
                .get(&stream_window)
                .is_some_and(|stream| stream.tag == tag)
            {
                streams.remove(&stream_window);
            }
        });
        Ok(ByteStream { rx })
    }

    fn close_byte_stream(&self, window: &str) -> Result<()> {
        if let Some(mut stream) = self
            .streams
            .lock()
            .expect("control streams lock")
            .remove(window)
        {
            let _ = stream.input.write_all(b"detach-client\n");
            let _ = stream.input.flush();
        }
        // Also clean up a pipe from a daemon version that predates control-mode streaming.
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
    fn control_output_decodes_octal_without_touching_utf8() {
        assert_eq!(
            decode_control_output(b"ready\\015\\012\\033[32m \xe2\x98\x83 \\134").unwrap(),
            b"ready\r\n\x1b[32m \xe2\x98\x83 \\"
        );
        assert!(decode_control_output(br"bad\12").is_none());
        assert!(decode_control_output(br"bad\400").is_none());
    }

    #[test]
    fn control_events_filter_identity_and_preserve_layout_order() {
        assert_eq!(
            parse_control_event(br"%layout-change @7 a87d,100x30,0,0,4 *", "@7", "%4"),
            Some(ControlEvent::Stream(ByteStreamEvent::Grid {
                cols: 100,
                rows: 30
            }))
        );
        assert_eq!(
            parse_control_event(br"%output %4 \033[2;1Hready", "@7", "%4"),
            Some(ControlEvent::Stream(ByteStreamEvent::Bytes(
                b"\x1b[2;1Hready".to_vec()
            )))
        );
        assert!(parse_control_event(br"%output %9 nope", "@7", "%4").is_none());
        assert!(
            parse_control_event(br"%layout-change @8 a87d,100x30,0,0,4 *", "@7", "%4").is_none()
        );
        assert_eq!(
            parse_control_event(br"%unlinked-window-close @7", "@7", "%4"),
            Some(ControlEvent::Closed)
        );
        assert_eq!(
            parse_control_event(br"%window-close @7", "@7", "%4"),
            Some(ControlEvent::Closed)
        );
        assert!(parse_control_event(br"%unlinked-window-close @8", "@7", "%4").is_none());
    }

    #[test]
    fn pane_size_floor_clamps_tiny_grids_and_keeps_healthy_ones() {
        // A mediated viewer reporting a momentary tiny layout must not shrink a real agent;
        // each dimension is floored independently (103x18 keeps its width, gains rows).
        assert_eq!(clamp_pane_size(20, 4), (MIN_PANE_COLS, MIN_PANE_ROWS));
        assert_eq!(clamp_pane_size(103, 18), (103, MIN_PANE_ROWS));

        assert_eq!(clamp_pane_size(211, 60), (211, 60));
        assert_eq!(clamp_pane_size(120, 40), (120, 40));
    }

    #[test]
    fn renders_spawn_specs_to_sh_command_strings() {
        // Program alone passes through verbatim (it may be a user-configured shell fragment),
        // behind the unconditional `env -u NO_COLOR` prefix.
        let bare = SpawnSpec::new("env -u CLAUDE_CONFIG_DIR claude", "/tmp");
        assert_eq!(
            render_spawn_command(&bare),
            "env -u NO_COLOR env -u CLAUDE_CONFIG_DIR claude"
        );
        // Args are single-quoted and appended - byte-identical to the old
        // `format!("{command} {}", shell_quote(task))` assembly.
        let with_task = SpawnSpec::new("claude", "/tmp").arg("fix the bug");
        assert_eq!(
            render_spawn_command(&with_task),
            "env -u NO_COLOR claude 'fix the bug'"
        );
        let quoted = SpawnSpec::new("claude", "/tmp").arg("it's tricky");
        assert_eq!(
            render_spawn_command(&quoted),
            r"env -u NO_COLOR claude 'it'\''s tricky'"
        );
        // Env pairs become leading KEY='value' assignments, after the NO_COLOR unset.
        let mut with_env = SpawnSpec::new("claude", "/tmp");
        with_env.env.push(("FOO".into(), "a b".into()));
        assert_eq!(
            render_spawn_command(&with_env),
            "env -u NO_COLOR FOO='a b' claude"
        );
    }

    #[test]
    fn render_spawn_command_always_unsets_no_color() {
        // Ambient NO_COLOR must not silently disable spawned agents’ color.
        let spec = SpawnSpec::new("agy", "/tmp");
        assert!(render_spawn_command(&spec).starts_with("env -u NO_COLOR "));
    }

    #[test]
    fn backend_attach_command_matches_the_tmux_invocation() {
        let rt = TmuxRuntime::new("repomon");
        let cmd = SessionBackend::attach_command(&rt, "repomon:=lane-7");
        assert_eq!(cmd.program, tmux_program().to_string_lossy().as_ref());
        assert_eq!(
            cmd.args,
            vec!["-L", "repomon", "attach", "-t", "repomon:=lane-7"]
        );
    }

    #[test]
    fn backend_targets_match_the_inherent_formats() {
        let rt = TmuxRuntime::new("repomon");
        assert_eq!(
            SessionBackend::target_named(&rt, "term-1-1"),
            "repomon:term-1-1"
        );
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
    fn parses_windows_meta_lines() {
        // `name%#%@id%#%session?%#%agent_kind?` - fields 3 and 4 are empty when options are
        // unset. `%#%` (not tab) is the separator so the probe survives tmux's C/POSIX-locale
        // vis-sanitization of control characters - see `PROBE_FIELD_SEP`.
        let sep = PROBE_FIELD_SEP;
        let out = format!(
            "lane-1{sep}@3{sep}abc-123{sep}claude-code\nlane-1-2{sep}@7{sep}{sep}antigravity\norchestrator{sep}@1{sep}{sep}\n"
        );
        assert_eq!(
            TmuxRuntime::parse_windows_meta(&out),
            vec![
                WindowMeta {
                    name: "lane-1".into(),
                    wid: 3,
                    session: Some("abc-123".into()),
                    agent_kind: Some("claude-code".into()),
                },
                WindowMeta {
                    name: "lane-1-2".into(),
                    wid: 7,
                    session: None,
                    agent_kind: Some("antigravity".into()),
                },
                WindowMeta {
                    name: "orchestrator".into(),
                    wid: 1,
                    session: None,
                    agent_kind: None,
                },
            ]
        );

        assert_eq!(
            TmuxRuntime::parse_windows_meta(&format!("w{sep}bogus{sep}{sep}\n"))[0].wid,
            u64::MAX
        );

        assert!(TmuxRuntime::parse_windows_meta("").is_empty());
    }

    #[test]
    fn parse_windows_meta_survives_locale_sanitized_output() {
        // The new sentinel-separated probe line - exactly what tmux now emits regardless of the
        // tmux CLIENT's locale, because `%#%` (unlike a bare tab) is never vis-sanitized away.
        let out = format!(
            "lane-81{sep}@5{sep}sid-123{sep}claude-code\n",
            sep = PROBE_FIELD_SEP
        );
        let metas = TmuxRuntime::parse_windows_meta(&out);
        assert_eq!(
            metas,
            vec![WindowMeta {
                name: "lane-81".into(),
                wid: 5,
                session: Some("sid-123".into()),
                agent_kind: Some("claude-code".into()),
            }]
        );
        assert_eq!(
            TmuxRuntime::lane_windows_meta(&metas, 81).len(),
            1,
            "sentinel-separated probe line must parse as a lane-81 window"
        );

        // A sanitized tab-delimited row must not be mistaken for a valid lane window.
        let garbled_old_style = "lane-81_@0_sid_claude-code\n";
        let garbled = TmuxRuntime::parse_windows_meta(garbled_old_style);
        assert_eq!(garbled.len(), 1);
        assert_eq!(garbled[0].name, "lane-81_@0_sid_claude-code");
        assert!(
            TmuxRuntime::parse_lane_window(&garbled[0].name).is_none(),
            "a fully garbled old-style line must NOT parse as a lane window"
        );
    }

    #[test]
    fn parses_windows_activity_lines() {
        let sep = PROBE_FIELD_SEP;
        let out = format!(
            "lane-1{sep}/repo/worktree{sep}1700000000\nlane-1-2{sep}/repo/wt2{sep}1700000005\n"
        );
        assert_eq!(
            TmuxRuntime::parse_windows_activity(&out),
            vec![
                (
                    "lane-1".to_string(),
                    PathBuf::from("/repo/worktree"),
                    1700000000
                ),
                (
                    "lane-1-2".to_string(),
                    PathBuf::from("/repo/wt2"),
                    1700000005
                ),
            ]
        );

        assert!(TmuxRuntime::parse_windows_activity("").is_empty());
    }

    #[test]
    fn locale_override_decision() {
        // No locale vars set at all - the GUI/launchd context that triggers the bug - hands the
        // tmux client a real UTF-8 locale.
        assert_eq!(
            locale_override(None, None, None),
            Some(("LC_ALL", "en_US.UTF-8"))
        );
        // Any one of the three already set means a real locale choice exists (the user's shell,
        // or ours from an earlier call) - never override it.
        assert_eq!(locale_override(Some("en_US.UTF-8"), None, None), None);
        assert_eq!(locale_override(None, Some("C"), None), None);
        assert_eq!(locale_override(None, None, Some("en_GB.UTF-8")), None);
        assert_eq!(
            locale_override(Some("en_US.UTF-8"), Some("C"), Some("en_GB.UTF-8")),
            None
        );
    }

    #[test]
    fn lane_windows_meta_filters_and_slot_orders() {
        let wm = |name: &str, wid: u64| WindowMeta {
            name: name.into(),
            wid,
            session: None,
            agent_kind: None,
        };
        // Exact lane matching (`lane-1` never claims `lane-12`), slot order regardless of
        // probe order or window id.
        let metas = vec![
            wm("lane-1-2", 9),
            wm("lane-12", 4),
            wm("lane-1", 2),
            wm("term-1-1", 5),
            wm("orchestrator", 1),
        ];
        let lane1 = TmuxRuntime::lane_windows_meta(&metas, 1);
        let names: Vec<&str> = lane1.iter().map(|w| w.name.as_str()).collect();
        assert_eq!(names, vec!["lane-1", "lane-1-2"]);
        assert_eq!(
            TmuxRuntime::lane_windows_meta(&metas, 12)
                .iter()
                .map(|w| w.name.as_str())
                .collect::<Vec<_>>(),
            vec!["lane-12"]
        );
        assert!(TmuxRuntime::lane_windows_meta(&metas, 3).is_empty());
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

    #[cfg(unix)]
    #[test]
    fn kill_named_terminates_pane_process_tree() {
        if !TmuxRuntime::available() {
            eprintln!("tmux not available; skipping live runtime test");
            return;
        }
        let rt = TmuxRuntime::new(format!("repomon-killtree-{}", std::process::id()));
        let cwd = std::env::temp_dir();
        rt.spawn_named("orchestrator", &cwd, "sh -c 'sleep 30'")
            .unwrap();

        std::thread::sleep(std::time::Duration::from_millis(200));
        let root = rt.pane_pid("orchestrator").expect("pane root pid");
        let tree = TmuxRuntime::process_tree(root);
        assert!(
            !tree.is_empty(),
            "test command should have a pane process: {tree:?}"
        );

        rt.kill_named("orchestrator").unwrap();
        for _ in 0..20 {
            if tree.iter().all(|pid| {
                Command::new("kill")
                    .args(["-0", &pid.to_string()])
                    .stderr(Stdio::null())
                    .status()
                    .map(|status| !status.success())
                    .unwrap_or(true)
            }) {
                let _ = Command::new(tmux_program())
                    .args(["-L", rt.session(), "kill-server"])
                    .output();
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        let _ = Command::new(tmux_program())
            .args(["-L", rt.session(), "kill-server"])
            .output();
        panic!("pane process tree survived kill_named: {tree:?}");
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
    fn control_stream_orders_grid_before_new_size_output_and_ignores_client_size() {
        if !TmuxRuntime::available() {
            eprintln!("tmux not available; skipping live runtime test");
            return;
        }
        let backend = TmuxRuntime::new(format!("repomon-controltest-{}", std::process::id()));
        let dir = tempfile::tempdir().unwrap();
        backend.spawn(1, dir.path(), "sh").unwrap();
        backend.resize_named("lane-1", 100, 30).unwrap();
        let before = backend.size_named("lane-1");

        let mut stream = backend.open_byte_stream("lane-1").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert_eq!(backend.size_named("lane-1"), before);

        backend.resize_named("lane-1", 120, 40).unwrap();
        backend
            .send_text_named("lane-1", "printf CONTROL_AFTER_RESIZE")
            .unwrap();

        let runtime = tokio::runtime::Runtime::new().unwrap();
        let mut saw_grid = false;
        let ordered = runtime.block_on(async {
            tokio::time::timeout(std::time::Duration::from_secs(3), async {
                while let Some(event) = stream.rx.recv().await {
                    match event {
                        ByteStreamEvent::Grid { cols, rows } => {
                            if (cols, rows) == (120, 40) {
                                saw_grid = true;
                            }
                        }
                        ByteStreamEvent::Bytes(bytes)
                            if bytes
                                .windows(b"CONTROL_AFTER_RESIZE".len())
                                .any(|window| window == b"CONTROL_AFTER_RESIZE") =>
                        {
                            return saw_grid;
                        }
                        ByteStreamEvent::Bytes(_) => {}
                    }
                }
                false
            })
            .await
            .unwrap_or(false)
        });
        assert!(ordered, "new-grid output arrived before its layout change");

        backend.close_byte_stream("lane-1").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(100));
        let clients = backend
            .run_allow_absent(&["list-clients", "-F", "#{client_control_mode}"])
            .unwrap();
        assert!(
            clients.trim().is_empty(),
            "control client leaked: {clients:?}"
        );
        backend.kill_named("lane-1").unwrap();
        let _ = Command::new(tmux_program())
            .args(["-L", backend.session(), "kill-server"])
            .output();
    }

    #[test]
    fn control_stream_closes_when_its_window_dies_but_session_survives() {
        if !TmuxRuntime::available() {
            eprintln!("tmux not available; skipping live runtime test");
            return;
        }
        let backend = TmuxRuntime::new(format!("repomon-control-close-{}", std::process::id()));
        let dir = tempfile::tempdir().unwrap();
        backend.spawn(1, dir.path(), "sh").unwrap();
        backend.spawn(2, dir.path(), "sh").unwrap();

        let mut stream = backend.open_byte_stream("lane-2").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(100));
        backend.kill_named("lane-2").unwrap();

        let runtime = tokio::runtime::Runtime::new().unwrap();
        let closed = runtime.block_on(async {
            tokio::time::timeout(std::time::Duration::from_secs(3), async {
                while stream.rx.recv().await.is_some() {}
            })
            .await
            .is_ok()
        });
        assert!(closed, "target window death did not close its byte stream");
        assert!(
            backend
                .list_windows()
                .unwrap()
                .iter()
                .any(|w| w == "lane-1"),
            "the sibling window should keep the tmux session alive"
        );
        let clients = backend
            .run_allow_absent(&["list-clients", "-F", "#{client_control_mode}"])
            .unwrap();
        assert!(
            clients.trim().is_empty(),
            "target window death leaked a control client: {clients:?}"
        );

        backend.kill_named("lane-1").unwrap();
        let _ = Command::new(tmux_program())
            .args(["-L", backend.session(), "kill-server"])
            .output();
    }

    #[test]
    fn single_owner_guard_claims_then_blocks_others() {
        if !TmuxRuntime::available() {
            eprintln!("tmux not available; skipping live runtime test");
            return;
        }
        let rt = TmuxRuntime::new(format!("repomon-ownertest-{}", std::process::id()));
        // A server must exist before server options can be set - spawn a throwaway window.
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

        let _ = Command::new(tmux_program())
            .args(["-L", rt.session(), "kill-server"])
            .output();
    }

    #[test]
    fn tmux_probe_reports_expected_shape() {
        let probe = TmuxRuntime::probe();
        if probe.available {
            assert!(probe.version.is_some());
            assert!(probe.source.is_some());
            assert!(probe.path.is_some());
            assert!(probe.version.unwrap().starts_with("tmux"));
        } else {
            assert!(probe.version.is_none());
            assert!(probe.source.is_none());
        }
    }

    #[test]
    fn resolution_order_prefers_env_override() {
        let dir = tempfile::tempdir().unwrap();
        let fake_env_tmux = dir.path().join("fake-env-tmux");
        std::fs::write(&fake_env_tmux, b"fake").unwrap();

        let sibling_dir = dir.path().join("bundle");
        std::fs::create_dir_all(&sibling_dir).unwrap();
        let sibling_tmux = sibling_dir.join(format!("tmux{}", std::env::consts::EXE_SUFFIX));
        std::fs::write(&sibling_tmux, b"sibling").unwrap();

        let resolved =
            resolve_tmux_from(Some(fake_env_tmux.to_str().unwrap()), None, &[sibling_dir]);
        assert_eq!(
            resolved,
            Some(ResolvedTmux {
                path: fake_env_tmux,
                source: crate::model::TmuxDoctorSource::System,
            })
        );
    }

    #[test]
    fn resolution_order_falls_back_to_sibling_when_path_missing() {
        let dir = tempfile::tempdir().unwrap();
        let sibling_dir = dir.path().join("bundle");
        std::fs::create_dir_all(&sibling_dir).unwrap();
        let sibling_tmux = sibling_dir.join(format!("tmux{}", std::env::consts::EXE_SUFFIX));
        std::fs::write(&sibling_tmux, b"sibling-tmux").unwrap();

        let empty_path = std::ffi::OsStr::new("");
        let resolved = resolve_tmux_from(None, Some(empty_path), &[sibling_dir]);
        assert_eq!(
            resolved,
            Some(ResolvedTmux {
                path: sibling_tmux,
                source: crate::model::TmuxDoctorSource::Bundled,
            })
        );
    }

    #[test]
    fn resolution_returns_none_when_no_source_available() {
        let dir = tempfile::tempdir().unwrap();
        let empty_path = std::ffi::OsStr::new("");
        let empty_sibling_dir = dir.path().join("empty");
        std::fs::create_dir_all(&empty_sibling_dir).unwrap();

        let resolved = resolve_tmux_from(None, Some(empty_path), &[empty_sibling_dir]);
        assert_eq!(resolved, None);
    }

    fn sample_tmux_doctor(available: bool) -> crate::model::TmuxDoctorInfo {
        crate::model::TmuxDoctorInfo {
            available,
            version: available.then(|| "tmux 3.4".to_string()),
            source: available.then_some(crate::model::TmuxDoctorSource::System),
            path: available.then(|| "/usr/bin/tmux".to_string()),
            not_applicable: false,
        }
    }

    #[test]
    fn tmux_not_applicable_on_windows_regardless_of_probe() {
        for available in [true, false] {
            let doc = tmux_doctor_for_platform(
                crate::model::DoctorPlatform::Windows,
                sample_tmux_doctor(available),
            );
            assert!(
                doc.not_applicable,
                "windows tmux must be marked not_applicable"
            );
        }
    }

    #[test]
    fn tmux_stays_applicable_off_windows() {
        for platform in [
            crate::model::DoctorPlatform::Macos,
            crate::model::DoctorPlatform::Linux,
        ] {
            let raw = sample_tmux_doctor(true);
            let doc = tmux_doctor_for_platform(platform, raw.clone());
            assert_eq!(
                doc, raw,
                "non-windows platforms must pass the probe through unchanged"
            );
        }
    }
}
