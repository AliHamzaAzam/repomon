//! Windows session backend: per-window `repomon-agent-host.exe` processes.
//!
//! The Windows counterpart of [`TmuxRuntime`](super::tmux::TmuxRuntime): each agent window is
//! owned by one detached host process (ConPTY + child + server-side vt100 screen) that serves
//! the frozen control protocol in `crates/repomon-host/PROTOCOL.md` on
//! `\\.\pipe\repomon-<session>-<window>` and registers itself under
//! `<data_dir>\hosts\<session>\<window>.json`. Hosts survive daemon restarts; on startup the
//! backend re-adopts them by scanning the registry and `hello`-verifying each pipe — the
//! Windows equivalent of the daemon finding an existing tmux server.
//!
//! Everything decision-shaped (spawn-command parsing, host argv assembly, target formats,
//! scan adopt/skip/GC rules, shell selection) is pure logic tested on every OS; only the pipe
//! client, host spawning, and the byte-stream pump are `#[cfg(windows)]`.

use crate::error::{Error, Result};

use super::backend::AttachCommand;

// ---------------------------------------------------------------------------
// Pure logic (all OSes)
// ---------------------------------------------------------------------------

/// Environment overrides parsed out of a spawn program string (`KEY=VALUE` prefixes).
pub type EnvPairs = Vec<(String, String)>;

/// Split a [`SpawnSpec`](super::backend::SpawnSpec) `program` string into environment
/// assignments and an argv. On Unix the program is a shell fragment run via `sh -c`; there is
/// no shell on Windows, so the backend parses the common shapes itself: leading `KEY=VALUE`
/// tokens become environment overrides (`CLAUDE_CONFIG_DIR='…' claude`), and the rest is
/// whitespace-split with single/double quotes respected (quotes group, backslashes are plain
/// path characters). An empty program is an error.
pub fn split_spawn_program(program: &str) -> Result<(EnvPairs, Vec<String>)> {
    let tokens = tokenize(program);
    let mut env: Vec<(String, String)> = Vec::new();
    let mut argv: Vec<String> = Vec::new();
    for tok in tokens {
        if argv.is_empty()
            && let Some((key, value)) = tok.split_once('=')
            && is_env_key(key)
        {
            env.push((key.to_string(), value.to_string()));
            continue;
        }
        argv.push(tok);
    }
    if argv.is_empty() {
        return Err(Error::Agent(format!(
            "agent command {program:?} has no program to run"
        )));
    }
    Ok((env, argv))
}

/// A shell-ish environment-assignment key: `[A-Za-z_][A-Za-z0-9_]*`.
fn is_env_key(key: &str) -> bool {
    let mut chars = key.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Whitespace-split honoring single/double quotes (quotes group and are stripped; backslash is
/// a plain character — these are Windows paths, not shell escapes).
fn tokenize(s: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut cur = String::new();
    let mut in_token = false;
    let mut quote: Option<char> = None;
    for c in s.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => cur.push(c),
            None => match c {
                '\'' | '"' => {
                    quote = Some(c);
                    in_token = true;
                }
                c if c.is_whitespace() => {
                    if in_token {
                        tokens.push(std::mem::take(&mut cur));
                        in_token = false;
                    }
                }
                c => {
                    cur.push(c);
                    in_token = true;
                }
            },
        }
    }
    if in_token {
        tokens.push(cur);
    }
    tokens
}

/// The full argument vector for `repomon-agent-host.exe`, per the PROTOCOL.md §1 spawn
/// contract: `--session S --window W --cwd DIR --owner TOK [--env K=V]... -- PROGRAM ARGS...`.
/// `--cols`/`--rows` are omitted — the host defaults to 220×50 (tmux parity).
pub fn host_spawn_args(
    session: &str,
    window: &str,
    cwd: &str,
    owner: &str,
    env: &[(String, String)],
    argv: &[String],
) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "--session".into(),
        session.into(),
        "--window".into(),
        window.into(),
        "--cwd".into(),
        cwd.into(),
        "--owner".into(),
        owner.into(),
    ];
    for (k, v) in env {
        args.push("--env".into());
        args.push(format!("{k}={v}"));
    }
    args.push("--".into());
    args.extend(argv.iter().cloned());
    args
}

/// `session:window` — same shape as the tmux target so clients treat both opaquely.
pub fn target_of(session: &str, window: &str) -> String {
    format!("{session}:{window}")
}

/// `session:=window` — the exact-match form (tmux parity; the `=` is inert here but keeps the
/// format identical across backends).
pub fn exact_target_of(session: &str, window: &str) -> String {
    format!("{session}:={window}")
}

/// Recover the window name from a target produced by [`target_of`]/[`exact_target_of`]; a
/// bare window name passes through unchanged.
pub fn window_from_target(session: &str, target: &str) -> String {
    let rest = target
        .strip_prefix(&format!("{session}:"))
        .unwrap_or(target);
    rest.strip_prefix('=').unwrap_or(rest).to_string()
}

/// How a registry-scan connect attempt ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectOutcome {
    /// Pipe connected (hello may or may not have succeeded).
    Connected,
    /// Pipe absent: not found / connection refused — the host is gone.
    Absent,
    /// Pipe exists but every instance was momentarily busy.
    Busy,
}

/// What the scanner does with one registry entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScanAction {
    /// Live host owned by us: adopt (list it, drive it).
    Adopt,
    /// Leave it alone (foreign owner, busy pipe, or hello failed on a live pipe).
    Skip,
    /// Stale registry entry: delete the JSON file (PROTOCOL.md §8).
    Gc,
}

/// The adopt/skip/GC rule (PROTOCOL.md §6 + §8): a pipe that won't connect marks the entry
/// stale (GC); a connected host is adopted only when its `hello.owner` matches `me` — on
/// mismatch the daemon MUST back off (not adopt, not reap, not kill). A busy pipe or a failed
/// hello on a live pipe is skipped, never GC'd.
pub fn scan_action(connect: ConnectOutcome, hello_owner: Option<&str>, me: &str) -> ScanAction {
    match connect {
        ConnectOutcome::Absent => ScanAction::Gc,
        ConnectOutcome::Busy => ScanAction::Skip,
        ConnectOutcome::Connected => match hello_owner {
            Some(owner) if owner == me => ScanAction::Adopt,
            _ => ScanAction::Skip,
        },
    }
}

/// Pick the user's interactive shell for plain terminals (`terminal.open`): PowerShell 7
/// (`pwsh`) when installed, else `%COMSPEC%`, else `cmd.exe`.
pub fn user_shell_from(pwsh: Option<std::path::PathBuf>, comspec: Option<String>) -> String {
    if let Some(p) = pwsh {
        return p.to_string_lossy().into_owned();
    }
    comspec
        .filter(|c| !c.is_empty())
        .unwrap_or_else(|| "cmd.exe".to_string())
}

/// The command a client runs in a real terminal to attach to `window`: the raw byte-proxy
/// attach client (`repomon attach-host <window>`, Track F).
pub fn attach_command_for(window: &str) -> AttachCommand {
    AttachCommand {
        program: "repomon".to_string(),
        args: vec!["attach-host".to_string(), window.to_string()],
    }
}

/// Whether a host's `program` is the Claude Code CLI, for the liveness probe's per-cwd claude
/// count: basename, extension stripped (`claude`, `claude.cmd`, `C:\…\claude.exe` all match),
/// case-insensitive (Windows filenames).
#[cfg(windows)]
pub use host_backend::WindowsBackend;

// ---------------------------------------------------------------------------
// The backend proper (Windows only)
// ---------------------------------------------------------------------------

#[cfg(windows)]
mod host_backend {
    use std::collections::HashMap;
    use std::fs::File;
    use std::io::{Read, Write};
    use std::os::windows::io::AsRawHandle;
    use std::os::windows::process::CommandExt;
    use std::path::{Path, PathBuf};
    use std::process::Stdio;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use base64::Engine as _;
    use repomon_host::codec::{FrameDecoder, encode_frame};
    use repomon_host::protocol::{HelloInfo, Op, Request, StreamFrame};
    use repomon_host::registry::{self, RegistryEntry};

    use super::super::backend::{
        AttachCommand, ByteStream, CaptureOpts, Cursor, OwnerState, SessionBackend, SpawnSpec,
        WindowActivity,
    };
    use super::super::tmux::TmuxRuntime;
    use super::{
        ConnectOutcome, ScanAction, attach_command_for, exact_target_of, host_spawn_args,
        is_claude_program, scan_action, split_spawn_program, target_of, user_shell_from,
        window_from_target,
    };
    use crate::error::{Error, Result};
    use crate::model::LaneId;

    /// `DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP`: the host has no console and outlives the
    /// daemon (PROTOCOL.md §1) — durability parity with the out-of-process tmux server.
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

    /// How long a spawn waits for the fresh host's pipe to answer `hello`.
    const SPAWN_HELLO_TIMEOUT: Duration = Duration::from_secs(10);
    /// Busy-retry ceiling for ordinary control connects (mirrors `transport::connect`).
    const BUSY_CEILING: Duration = Duration::from_secs(2);
    /// Shorter ceiling for registry scans, which touch every host per call.
    const SCAN_BUSY_CEILING: Duration = Duration::from_millis(250);

    static NEXT_REQUEST_ID: AtomicU64 = AtomicU64::new(1);
    static NEXT_STREAM_ID: AtomicU64 = AtomicU64::new(0);

    /// The result of trying to reach a host's pipe once (with busy retry).
    enum Connect {
        Ok(File),
        /// `ERROR_FILE_NOT_FOUND`: no live pipe — the host is gone.
        Absent,
        /// The pipe exists but couldn't be opened right now (all instances busy, or an
        /// unexpected open error). Alive as far as we know — never GC'd.
        Busy,
    }

    /// Open `\\.\pipe\…` as a synchronous byte-mode duplex file, retrying `ERROR_PIPE_BUSY`
    /// briefly (the host pre-creates a spare instance, so busy is a tiny race window).
    fn connect_pipe(name: &str, busy_ceiling: Duration) -> Connect {
        const ERROR_PIPE_BUSY: i32 = 231;
        let mut delay = Duration::from_millis(10);
        let mut waited = Duration::ZERO;
        loop {
            match std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(name)
            {
                Ok(f) => return Connect::Ok(f),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Connect::Absent,
                Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) && waited < busy_ceiling => {
                    std::thread::sleep(delay);
                    waited += delay;
                    delay = (delay * 2).min(Duration::from_millis(100));
                }
                Err(_) => return Connect::Busy,
            }
        }
    }

    fn write_request(f: &mut File, id: u64, op: Op) -> Result<()> {
        let payload = serde_json::to_vec(&Request { id, op })
            .map_err(|e| Error::Agent(format!("encode host request: {e}")))?;
        f.write_all(&encode_frame(&payload)).map_err(Error::Io)?;
        Ok(())
    }

    /// Read frames until one arrives; `Ok(None)` on EOF.
    fn read_frame(f: &mut File, dec: &mut FrameDecoder) -> Result<Option<Vec<u8>>> {
        let mut buf = [0u8; 64 * 1024];
        loop {
            match dec.next_frame() {
                Ok(Some(p)) => return Ok(Some(p)),
                Ok(None) => {}
                Err(e) => return Err(Error::Agent(format!("host frame: {e}"))),
            }
            let n = f.read(&mut buf).map_err(Error::Io)?;
            if n == 0 {
                return Ok(None);
            }
            dec.extend(&buf[..n]);
        }
    }

    /// One request/response exchange on an open control connection: returns the `ok` body.
    fn roundtrip(f: &mut File, dec: &mut FrameDecoder, op: Op) -> Result<serde_json::Value> {
        let id = NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed);
        write_request(f, id, op)?;
        let payload = read_frame(f, dec)?
            .ok_or_else(|| Error::Agent("host closed the pipe mid-request".into()))?;
        let v: serde_json::Value = serde_json::from_slice(&payload)
            .map_err(|e| Error::Agent(format!("host response: {e}")))?;
        if let Some(err) = v.get("err").and_then(|e| e.as_str()) {
            return Err(Error::Agent(format!("host: {err}")));
        }
        Ok(v.get("ok").cloned().unwrap_or(serde_json::Value::Null))
    }

    /// One live, adopted host as a registry scan sees it.
    struct LiveHost {
        hello: HelloInfo,
    }

    /// A running byte-stream reader for one window.
    struct ActiveStream {
        id: u64,
        stop: Arc<AtomicBool>,
        thread: std::thread::JoinHandle<()>,
    }

    /// Ask a possibly-blocked synchronous reader thread to stop: set its flag, then cancel its
    /// in-flight pipe read until the thread finishes (a missed cancel while it processes a
    /// frame just means the next blocking read gets cancelled on the following attempt).
    fn stop_stream(s: ActiveStream) {
        s.stop.store(true, Ordering::Relaxed);
        std::thread::spawn(move || {
            for _ in 0..40 {
                if s.thread.is_finished() {
                    return;
                }
                unsafe {
                    windows_sys::Win32::System::IO::CancelSynchronousIo(
                        s.thread.as_raw_handle() as windows_sys::Win32::Foundation::HANDLE
                    );
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        });
    }

    /// The Windows [`SessionBackend`]: drives per-window `repomon-agent-host.exe` processes
    /// over their control pipes. Cheap to construct; all state is on disk (registry) or in the
    /// hosts themselves, so a new daemon re-adopts everything by scanning.
    pub struct WindowsBackend {
        session: String,
        /// This daemon's owner identity (its db path): stamped on every spawned host
        /// (`--owner`) and compared against `hello.owner` before adopting (PROTOCOL.md §6).
        owner: String,
        /// repomon's data dir; the host registry lives at `<data_dir>\hosts\<session>\`.
        data_dir: PathBuf,
        /// Live byte-stream readers, one per window at most (trait contract).
        streams: Arc<Mutex<HashMap<String, ActiveStream>>>,
    }

    impl WindowsBackend {
        pub fn new(
            session: impl Into<String>,
            owner: impl Into<String>,
            data_dir: impl Into<PathBuf>,
        ) -> Self {
            Self {
                session: session.into(),
                owner: owner.into(),
                data_dir: data_dir.into(),
                streams: Arc::new(Mutex::new(HashMap::new())),
            }
        }

        fn hosts_dir(&self) -> PathBuf {
            self.data_dir.join("hosts").join(&self.session)
        }

        fn pipe_name(&self, window: &str) -> String {
            registry::pipe_name(&self.session, window)
        }

        /// Locate `repomon-agent-host.exe`: env override, beside the current executable, one
        /// directory up (`target\debug\deps\…` test binaries), then `PATH`.
        fn host_binary() -> Option<PathBuf> {
            if let Ok(p) = std::env::var("REPOMON_HOST_BIN")
                && !p.is_empty()
            {
                return Some(PathBuf::from(p));
            }
            const NAME: &str = "repomon-agent-host.exe";
            if let Ok(exe) = std::env::current_exe()
                && let Some(dir) = exe.parent()
            {
                let beside = dir.join(NAME);
                if beside.exists() {
                    return Some(beside);
                }
                if let Some(parent) = dir.parent() {
                    let above = parent.join(NAME);
                    if above.exists() {
                        return Some(above);
                    }
                }
            }
            crate::exec::find_in_path("repomon-agent-host")
        }

        /// One request/response against a window's host on a fresh control connection.
        fn request(&self, window: &str, op: Op) -> Result<serde_json::Value> {
            match connect_pipe(&self.pipe_name(window), BUSY_CEILING) {
                Connect::Ok(mut f) => roundtrip(&mut f, &mut FrameDecoder::new(), op),
                Connect::Absent => Err(absent(window)),
                Connect::Busy => Err(Error::Agent(format!(
                    "host pipe for window {window} is busy"
                ))),
            }
        }

        /// Like [`request`], but a vanished window is benign (`Ok(None)`) — the analogue of
        /// the tmux impl's `run_allow_absent`.
        fn request_allow_absent(&self, window: &str, op: Op) -> Result<Option<serde_json::Value>> {
            match self.request(window, op) {
                Ok(v) => Ok(Some(v)),
                Err(e) if is_absent(&e) => Ok(None),
                Err(e) => Err(e),
            }
        }

        /// Scan the registry, GC stale entries, and return every live host we own
        /// (PROTOCOL.md §6 + §8) — re-adoption *is* this scan run on a fresh daemon.
        fn scan(&self) -> Vec<LiveHost> {
            let dir = self.hosts_dir();
            let Ok(entries) = std::fs::read_dir(&dir) else {
                return Vec::new();
            };
            let mut live = Vec::new();
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("json") {
                    continue;
                }
                let Ok(bytes) = std::fs::read(&path) else {
                    continue;
                };
                let Ok(reg) = serde_json::from_slice::<RegistryEntry>(&bytes) else {
                    continue; // unreadable/foreign file: not ours to judge
                };
                if reg.session != self.session {
                    continue;
                }
                let (outcome, hello) = match connect_pipe(&reg.pipe, SCAN_BUSY_CEILING) {
                    Connect::Ok(mut f) => {
                        let hello = roundtrip(&mut f, &mut FrameDecoder::new(), Op::Hello)
                            .ok()
                            .and_then(|ok| serde_json::from_value::<HelloInfo>(ok).ok());
                        (ConnectOutcome::Connected, hello)
                    }
                    Connect::Absent => (ConnectOutcome::Absent, None),
                    Connect::Busy => (ConnectOutcome::Busy, None),
                };
                let owner = hello.as_ref().map(|h| h.owner.as_str());
                match scan_action(outcome, owner, &self.owner) {
                    ScanAction::Adopt => live.push(LiveHost {
                        hello: hello.expect("adopt implies hello"),
                    }),
                    ScanAction::Skip => {}
                    ScanAction::Gc => {
                        let _ = std::fs::remove_file(&path);
                    }
                }
            }
            live.sort_by(|a, b| a.hello.started_at.cmp(&b.hello.started_at));
            live
        }

        /// Spawn a detached host per the PROTOCOL.md §1 contract and wait for its pipe to
        /// answer `hello` (the registry entry is written once the pipe is connectable).
        fn spawn_host(
            &self,
            window: &str,
            cwd: &Path,
            env: &[(String, String)],
            argv: &[String],
        ) -> Result<()> {
            let host = Self::host_binary().ok_or_else(|| {
                Error::Agent("repomon-agent-host.exe not found (beside repomond or on PATH)".into())
            })?;
            let args = host_spawn_args(
                &self.session,
                window,
                &cwd.to_string_lossy(),
                &self.owner,
                env,
                argv,
            );
            let mut child = std::process::Command::new(&host)
                .args(&args)
                .env("REPOMON_DATA_DIR", &self.data_dir)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP)
                .spawn()
                .map_err(Error::Io)?;
            let pipe = self.pipe_name(window);
            let deadline = Instant::now() + SPAWN_HELLO_TIMEOUT;
            loop {
                if let Connect::Ok(mut f) = connect_pipe(&pipe, SCAN_BUSY_CEILING)
                    && roundtrip(&mut f, &mut FrameDecoder::new(), Op::Hello).is_ok()
                {
                    return Ok(());
                }
                if let Ok(Some(status)) = child.try_wait() {
                    return Err(Error::Agent(format!(
                        "repomon-agent-host for window {window} exited at startup ({status})"
                    )));
                }
                if Instant::now() >= deadline {
                    return Err(Error::Agent(format!(
                        "repomon-agent-host for window {window} did not come up in {}s",
                        SPAWN_HELLO_TIMEOUT.as_secs()
                    )));
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }

        /// Spawn from a [`SpawnSpec`]: parse the program string (env prefixes + quoting),
        /// merge the spec's env, append its args, and launch the host.
        fn spawn_spec(&self, window: &str, spec: &SpawnSpec) -> Result<()> {
            let (mut env, mut argv) = split_spawn_program(&spec.program)?;
            let mut all_env = spec.env.clone();
            all_env.append(&mut env);
            argv.extend(spec.args.iter().cloned());
            self.spawn_host(window, &spec.cwd, &all_env, &argv)
        }
    }

    fn absent(window: &str) -> Error {
        Error::Agent(format!("can't find window {window}"))
    }

    fn is_absent(e: &Error) -> bool {
        matches!(e, Error::Agent(msg) if msg.starts_with("can't find window "))
    }

    impl SessionBackend for WindowsBackend {
        fn available(&self) -> bool {
            Self::host_binary().is_some()
        }

        fn label(&self) -> String {
            self.session.clone()
        }

        fn session_exists(&self) -> bool {
            !self.scan().is_empty()
        }

        /// Cooperative single-owner guard: an `owner` stamp file in the session's registry
        /// directory (the durable analogue of the tmux server option `@repomon-owner`).
        /// First writer wins; later daemons with a different identity must back off.
        fn claim_or_verify_owner(&self, me: &str) -> OwnerState {
            let path = self.hosts_dir().join("owner");
            let verify = |content: String| {
                if content.trim() == me {
                    OwnerState::Owned
                } else {
                    OwnerState::OwnedByOther
                }
            };
            if let Ok(existing) = std::fs::read_to_string(&path) {
                return verify(existing);
            }
            let _ = std::fs::create_dir_all(self.hosts_dir());
            // create_new: if a concurrent claimer got there first, re-read and compare.
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                && f.write_all(me.as_bytes()).is_err()
            {
                return OwnerState::OwnedByOther;
            }
            match std::fs::read_to_string(&path) {
                Ok(content) => verify(content),
                Err(_) => OwnerState::OwnedByOther,
            }
        }

        fn list_windows(&self) -> Result<Vec<String>> {
            Ok(self.scan().into_iter().map(|h| h.hello.window).collect())
        }

        fn list_windows_with_activity(&self) -> Result<Vec<WindowActivity>> {
            Ok(self
                .scan()
                .into_iter()
                .map(|h| WindowActivity {
                    name: h.hello.window,
                    cwd: PathBuf::from(h.hello.cwd),
                    last_activity: h.hello.last_activity,
                })
                .collect())
        }

        fn live_agent_cwds(&self) -> Option<HashMap<PathBuf, usize>> {
            let mut counts: HashMap<PathBuf, usize> = HashMap::new();
            for h in self.scan() {
                if !is_claude_program(&h.hello.program) {
                    continue;
                }
                let p = PathBuf::from(&h.hello.cwd);
                let key = p.canonicalize().unwrap_or(p);
                *counts.entry(key).or_insert(0) += 1;
            }
            Some(counts)
        }

        fn spawn(&self, lane: LaneId, spec: &SpawnSpec) -> Result<String> {
            let taken = self.windows_for(lane).unwrap_or_default();
            let window = (1..)
                .map(|slot| TmuxRuntime::slot_name(lane, slot))
                .find(|name| !taken.contains(name))
                .expect("unbounded slot range");
            self.spawn_spec(&window, spec)?;
            Ok(exact_target_of(&self.session, &window))
        }

        fn spawn_named(&self, window: &str, spec: &SpawnSpec) -> Result<String> {
            self.spawn_spec(window, spec)?;
            Ok(exact_target_of(&self.session, window))
        }

        fn open_named(&self, window: &str, cwd: &Path) -> Result<String> {
            let shell = user_shell_from(
                crate::exec::find_in_path("pwsh"),
                std::env::var("COMSPEC").ok(),
            );
            self.spawn_host(window, cwd, &[], &[shell])?;
            Ok(target_of(&self.session, window))
        }

        fn capture_named(&self, window: &str, opts: CaptureOpts) -> Result<String> {
            let ok = self.request_allow_absent(
                window,
                Op::Capture {
                    lines: opts.last_lines,
                },
            )?;
            Ok(ok
                .and_then(|v| v.get("text").and_then(|t| t.as_str()).map(str::to_string))
                .unwrap_or_default())
        }

        fn cursor_named(&self, window: &str) -> Option<Cursor> {
            let v = self.request(window, Op::Cursor).ok()?;
            let visible = v.get("visible")?.as_bool()?;
            if !visible {
                return None;
            }
            Some(Cursor {
                col: v.get("col")?.as_u64()? as u16,
                row: v.get("row")?.as_u64()? as u16,
            })
        }

        fn size_named(&self, window: &str) -> Option<(u16, u16)> {
            let v = self.request(window, Op::Size).ok()?;
            Some((
                v.get("cols")?.as_u64()? as u16,
                v.get("rows")?.as_u64()? as u16,
            ))
        }

        fn resize_named(&self, window: &str, cols: u16, rows: u16) -> Result<()> {
            self.request_allow_absent(window, Op::Resize { cols, rows })?;
            Ok(())
        }

        /// No pinned-size concept: the host's ConPTY is always last-client-wins.
        fn follow_client_named(&self, _window: &str) -> Result<()> {
            Ok(())
        }

        fn alternate_on_named(&self, window: &str) -> bool {
            self.request(window, Op::AlternateOn)
                .ok()
                .and_then(|v| v.get("on").and_then(|o| o.as_bool()))
                .unwrap_or(false)
        }

        fn scroll_wheel_named(&self, window: &str, up: bool, ticks: u32) -> Result<()> {
            if ticks == 0 {
                return Ok(());
            }
            self.request_allow_absent(window, Op::ScrollWheel { up, ticks })?;
            Ok(())
        }

        fn send_literal_named(&self, window: &str, text: &str) -> Result<()> {
            self.request(
                window,
                Op::SendLiteral {
                    text: text.to_string(),
                },
            )?;
            Ok(())
        }

        fn send_text_named(&self, window: &str, text: &str) -> Result<()> {
            self.request(
                window,
                Op::SendText {
                    text: text.to_string(),
                },
            )?;
            Ok(())
        }

        fn send_key_named(&self, window: &str, key: &str) -> Result<()> {
            self.request(
                window,
                Op::SendKey {
                    key: key.to_string(),
                },
            )?;
            Ok(())
        }

        fn kill_named(&self, window: &str) -> Result<()> {
            // The host answers `ok` then exits, removing its own registry entry.
            self.request(window, Op::Kill)?;
            Ok(())
        }

        /// Nothing to configure: capture/scrollback/mouse behavior live in the hosts.
        fn configure(&self) {}

        fn target_named(&self, window: &str) -> String {
            target_of(&self.session, window)
        }

        fn exact_target_named(&self, window: &str) -> String {
            exact_target_of(&self.session, window)
        }

        fn attach_command(&self, target: &str) -> AttachCommand {
            attach_command_for(&window_from_target(&self.session, target))
        }

        /// `subscribe_bytes` on a dedicated second connection (PROTOCOL.md §5: a subscribed
        /// connection is stream-only). The first pushed frame is a full-screen replay; every
        /// frame's payload is forwarded raw to the channel. A reader thread pumps the pipe;
        /// [`close_byte_stream`](Self::close_byte_stream) stops it via a flag +
        /// `CancelSynchronousIo`.
        fn open_byte_stream(&self, window: &str) -> Result<ByteStream> {
            let mut file = match connect_pipe(&self.pipe_name(window), BUSY_CEILING) {
                Connect::Ok(f) => f,
                Connect::Absent => return Err(absent(window)),
                Connect::Busy => {
                    return Err(Error::Agent(format!(
                        "host pipe for window {window} is busy"
                    )));
                }
            };
            let mut dec = FrameDecoder::new();
            // The subscribe response arrives on the same connection; the same decoder may
            // already hold the first stream frames behind it — hand both to the pump.
            roundtrip(&mut file, &mut dec, Op::SubscribeBytes)?;

            let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
            let stop = Arc::new(AtomicBool::new(false));
            let id = NEXT_STREAM_ID.fetch_add(1, Ordering::Relaxed);
            let mut map = self.streams.lock().expect("streams lock");
            // At most one stream per window: a fresh open supersedes a lingering reader.
            if let Some(old) = map.remove(window) {
                stop_stream(old);
            }
            let thread = {
                let stop = stop.clone();
                let streams = self.streams.clone();
                let window = window.to_string();
                std::thread::spawn(move || {
                    pump(file, dec, tx, &stop);
                    let mut map = streams.lock().expect("streams lock");
                    if map.get(&window).is_some_and(|e| e.id == id) {
                        map.remove(&window);
                    }
                })
            };
            map.insert(window.to_string(), ActiveStream { id, stop, thread });
            Ok(ByteStream { rx })
        }

        fn close_byte_stream(&self, window: &str) -> Result<()> {
            let entry = self.streams.lock().expect("streams lock").remove(window);
            if let Some(entry) = entry {
                stop_stream(entry);
            }
            Ok(())
        }
    }

    /// Drain stream frames from the subscribed connection into the channel until EOF, error,
    /// stop-flag, or the consumer hanging up. Disconnecting is the protocol's only
    /// unsubscribe (PROTOCOL.md §7.11) — dropping `file` on return is the unsubscribe.
    fn pump(
        mut file: File,
        mut dec: FrameDecoder,
        tx: tokio::sync::mpsc::UnboundedSender<Vec<u8>>,
        stop: &AtomicBool,
    ) {
        let mut buf = [0u8; 64 * 1024];
        loop {
            loop {
                match dec.next_frame() {
                    Ok(Some(payload)) => {
                        let Ok(frame) = serde_json::from_slice::<StreamFrame>(&payload) else {
                            continue; // not a stream frame (unknown push): ignore
                        };
                        if frame.stream != "bytes" {
                            continue;
                        }
                        let Ok(bytes) =
                            base64::engine::general_purpose::STANDARD.decode(&frame.data)
                        else {
                            continue;
                        };
                        if tx.send(bytes).is_err() {
                            return; // consumer gone
                        }
                    }
                    Ok(None) => break,
                    Err(_) => return, // corrupt peer
                }
            }
            if stop.load(Ordering::Relaxed) {
                return;
            }
            match file.read(&mut buf) {
                Ok(0) | Err(_) => return, // EOF (host exited) or cancelled
                Ok(n) => dec.extend(&buf[..n]),
            }
        }
    }
}

pub fn is_claude_program(program: &str) -> bool {
    let base = program
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(program)
        .to_ascii_lowercase();
    let stem = base
        .rsplit_once('.')
        .map(|(stem, _ext)| stem)
        .unwrap_or(&base);
    stem == "claude"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_a_bare_program() {
        let (env, argv) = split_spawn_program("claude").unwrap();
        assert!(env.is_empty());
        assert_eq!(argv, vec!["claude"]);
    }

    #[test]
    fn splits_program_with_args_and_quotes() {
        let (env, argv) =
            split_spawn_program(r#"claude --permission-mode plan --title "my agent""#).unwrap();
        assert!(env.is_empty());
        assert_eq!(
            argv,
            vec!["claude", "--permission-mode", "plan", "--title", "my agent"]
        );
    }

    #[test]
    fn splits_leading_env_assignments_into_env() {
        // The autodetected Claude-variant shape: `CLAUDE_CONFIG_DIR='…' claude`.
        let (env, argv) =
            split_spawn_program(r"CLAUDE_CONFIG_DIR='C:\Users\me\.claude-work' claude").unwrap();
        assert_eq!(
            env,
            vec![(
                "CLAUDE_CONFIG_DIR".to_string(),
                r"C:\Users\me\.claude-work".to_string()
            )]
        );
        assert_eq!(argv, vec!["claude"]);
    }

    #[test]
    fn env_assignment_after_the_program_is_an_argument() {
        let (env, argv) = split_spawn_program("cmd.exe FOO=bar").unwrap();
        assert!(env.is_empty());
        assert_eq!(argv, vec!["cmd.exe", "FOO=bar"]);
    }

    #[test]
    fn backslashes_are_plain_characters() {
        // Windows paths must survive: no shell-style backslash escaping.
        let (_, argv) = split_spawn_program(r"C:\tools\claude.exe --fast").unwrap();
        assert_eq!(argv, vec![r"C:\tools\claude.exe", "--fast"]);
    }

    #[test]
    fn empty_program_is_an_error() {
        assert!(split_spawn_program("").is_err());
        assert!(split_spawn_program("   ").is_err());
        // Only env assignments, nothing to run.
        assert!(split_spawn_program("FOO=bar").is_err());
    }

    #[test]
    fn host_spawn_args_follow_the_protocol_contract() {
        let args = host_spawn_args(
            "repomon",
            "lane-3-1",
            r"C:\work",
            "tok",
            &[("FOO".to_string(), "bar".to_string())],
            &[
                "claude".to_string(),
                "--permission-mode".to_string(),
                "plan".to_string(),
            ],
        );
        assert_eq!(
            args,
            vec![
                "--session",
                "repomon",
                "--window",
                "lane-3-1",
                "--cwd",
                r"C:\work",
                "--owner",
                "tok",
                "--env",
                "FOO=bar",
                "--",
                "claude",
                "--permission-mode",
                "plan",
            ]
        );
    }

    #[test]
    fn targets_match_the_tmux_shapes_and_round_trip() {
        assert_eq!(target_of("repomon", "lane-7"), "repomon:lane-7");
        assert_eq!(exact_target_of("repomon", "lane-7"), "repomon:=lane-7");
        assert_eq!(window_from_target("repomon", "repomon:lane-7"), "lane-7");
        assert_eq!(window_from_target("repomon", "repomon:=lane-7"), "lane-7");
        // A bare window name passes through (defensive).
        assert_eq!(window_from_target("repomon", "lane-7"), "lane-7");
    }

    #[test]
    fn scan_adopts_own_live_hosts_only() {
        assert_eq!(
            scan_action(ConnectOutcome::Connected, Some("me"), "me"),
            ScanAction::Adopt
        );
        // Foreign owner: back off — never adopt, reap, or kill (PROTOCOL.md §6).
        assert_eq!(
            scan_action(ConnectOutcome::Connected, Some("other"), "me"),
            ScanAction::Skip
        );
        // Live pipe but hello failed: leave it alone, never GC a connectable pipe.
        assert_eq!(
            scan_action(ConnectOutcome::Connected, None, "me"),
            ScanAction::Skip
        );
    }

    #[test]
    fn scan_gcs_only_dead_pipes() {
        assert_eq!(
            scan_action(ConnectOutcome::Absent, None, "me"),
            ScanAction::Gc
        );
        // Busy = alive: never GC, never adopt this pass.
        assert_eq!(
            scan_action(ConnectOutcome::Busy, None, "me"),
            ScanAction::Skip
        );
    }

    #[test]
    fn shell_prefers_pwsh_then_comspec_then_cmd() {
        assert_eq!(
            user_shell_from(
                Some(std::path::PathBuf::from(
                    r"C:\Program Files\PowerShell\7\pwsh.exe"
                )),
                Some(r"C:\Windows\system32\cmd.exe".to_string()),
            ),
            r"C:\Program Files\PowerShell\7\pwsh.exe"
        );
        assert_eq!(
            user_shell_from(None, Some(r"C:\Windows\system32\cmd.exe".to_string())),
            r"C:\Windows\system32\cmd.exe"
        );
        assert_eq!(user_shell_from(None, None), "cmd.exe");
    }

    #[test]
    fn attach_command_runs_the_attach_host_client() {
        let cmd = attach_command_for("lane-7");
        assert_eq!(cmd.program, "repomon");
        assert_eq!(cmd.args, vec!["attach-host", "lane-7"]);
    }

    #[test]
    fn claude_program_matching_handles_paths_and_extensions() {
        assert!(is_claude_program("claude"));
        assert!(is_claude_program("claude.cmd"));
        assert!(is_claude_program("CLAUDE.EXE"));
        assert!(is_claude_program(
            r"C:\Users\me\AppData\Roaming\npm\claude.cmd"
        ));
        assert!(!is_claude_program("codex"));
        assert!(!is_claude_program("claude-helper.exe"));
        assert!(!is_claude_program(""));
    }
}
