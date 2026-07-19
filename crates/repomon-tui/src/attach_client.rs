//! `repomon attach-host <window>` — raw byte-proxy attach client for Windows agent hosts.
//!
//! Connects to a `repomon-agent-host.exe` control pipe (`\\.\pipe\repomon-<session>-<window>`)
//! per the frozen contract in `crates/repomon-host/PROTOCOL.md` and mirrors the agent in the
//! current console: raw stdin bytes become `send_literal` frames, a `subscribe_bytes` stream
//! (whose first frame is a full-screen replay) is written to stdout, console resizes become
//! `resize` frames (last client wins), and F12 detaches — leaving the agent running (tmux
//! parity). The heavy runtime is `#[cfg(windows)]`; the protocol layer below is
//! OS-independent and unit-tested everywhere.

use anyhow::{Context, Result, anyhow, bail, ensure};
use base64::Engine as _;
use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

// ---------------------------------------------------------------------------
// Protocol layer (OS-independent; PROTOCOL.md v1, frozen)
// ---------------------------------------------------------------------------

/// Maximum control-frame payload size (§4): 16 MiB. A larger advertised length means the
/// connection is corrupt and must be dropped.
pub const MAX_FRAME: u32 = 16 * 1024 * 1024;

/// The control pipe a host serves for `session`/`window` (§2).
pub fn pipe_name(session: &str, window: &str) -> String {
    format!(r"\\.\pipe\repomon-{session}-{window}")
}

/// Encode one frame: `[u32 length, little-endian][UTF-8 JSON]` (§4).
pub fn encode_frame(v: &Value) -> Result<Vec<u8>> {
    let payload = serde_json::to_vec(v)?;
    ensure!(
        payload.len() <= MAX_FRAME as usize,
        "frame too large ({} bytes)",
        payload.len()
    );
    let mut out = Vec::with_capacity(4 + payload.len());
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(&payload);
    Ok(out)
}

/// Write one frame to `w`.
pub async fn write_frame<W: AsyncWrite + Unpin>(w: &mut W, v: &Value) -> Result<()> {
    w.write_all(&encode_frame(v)?).await?;
    w.flush().await?;
    Ok(())
}

/// Read one frame from `r`. `Ok(None)` on clean EOF at a frame boundary; an error on a
/// truncated frame or an oversize length (corrupt connection, §4).
pub async fn read_frame<R: AsyncRead + Unpin>(r: &mut R) -> Result<Option<Value>> {
    let mut len = [0u8; 4];
    match r.read_exact(&mut len).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e.into()),
    }
    let len = u32::from_le_bytes(len);
    ensure!(
        len <= MAX_FRAME,
        "frame length {len} exceeds 16 MiB — corrupt connection"
    );
    let mut payload = vec![0u8; len as usize];
    r.read_exact(&mut payload)
        .await
        .context("connection closed mid-frame")?;
    Ok(Some(serde_json::from_slice(&payload)?))
}

// ---- request builders (§7) ----

pub fn req_hello(id: u64) -> Value {
    json!({"id": id, "op": "hello"})
}

pub fn req_resize(id: u64, cols: u16, rows: u16) -> Value {
    json!({"id": id, "op": "resize", "cols": cols, "rows": rows})
}

pub fn req_send_literal(id: u64, text: &str) -> Value {
    json!({"id": id, "op": "send_literal", "text": text})
}

pub fn req_subscribe(id: u64) -> Value {
    json!({"id": id, "op": "subscribe_bytes"})
}

/// Interpret a response frame for request `id` (§5): `{"id", "ok": {...}}` yields the payload,
/// `{"id", "err": "..."}` surfaces the host's message, anything else is a protocol error.
pub fn parse_response(v: &Value, id: u64) -> Result<Value> {
    let got = v
        .get("id")
        .and_then(Value::as_u64)
        .ok_or_else(|| anyhow!("response without an id: {v}"))?;
    ensure!(
        got == id,
        "response id {got} does not match request id {id}"
    );
    if let Some(err) = v.get("err").and_then(Value::as_str) {
        bail!("host error: {err}");
    }
    v.get("ok")
        .cloned()
        .ok_or_else(|| anyhow!("response is neither ok nor err: {v}"))
}

/// Decode a stream-mode frame `{"stream": "bytes", "data": "<base64>"}` (§7.11) into raw bytes.
pub fn parse_stream_frame(v: &Value) -> Result<Vec<u8>> {
    ensure!(
        v.get("stream").and_then(Value::as_str) == Some("bytes"),
        "expected a bytes stream frame, got: {v}"
    );
    let data = v
        .get("data")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("stream frame without data: {v}"))?;
    base64::engine::general_purpose::STANDARD
        .decode(data)
        .context("stream frame data is not valid base64")
}

/// What a decoded frame on the subscription connection means.
pub enum StreamEvent {
    /// The `subscribe_bytes` acknowledgement; stream mode starts after this.
    Ack,
    /// Raw PTY bytes to write to the terminal. The first such event is the host's
    /// full-current-screen replay (§7.11); later ones are ordered raw output chunks.
    Bytes(Vec<u8>),
}

/// Orders the subscription conversation: the `subscribe_bytes` `ok` must arrive first, then
/// every frame is a bytes frame, verbatim, in order.
pub struct StreamState {
    sub_id: u64,
    subscribed: bool,
}

impl StreamState {
    pub fn new(sub_id: u64) -> Self {
        Self {
            sub_id,
            subscribed: false,
        }
    }

    pub fn on_frame(&mut self, v: &Value) -> Result<StreamEvent> {
        if self.subscribed {
            return parse_stream_frame(v).map(StreamEvent::Bytes);
        }
        parse_response(v, self.sub_id).context("subscribe_bytes was not acknowledged")?;
        self.subscribed = true;
        Ok(StreamEvent::Ack)
    }
}

// ---------------------------------------------------------------------------
// Input scanner: raw VT stdin bytes -> protocol input actions
// ---------------------------------------------------------------------------

/// The VT sequence F12 produces under `ENABLE_VIRTUAL_TERMINAL_INPUT`. F12 is the local
/// detach key (tmux parity) and is never forwarded to the agent.
pub const DETACH_SEQ: &[u8] = b"\x1b[24~";

/// What a chunk of raw stdin bytes turns into.
#[derive(Debug)]
pub enum InputAction {
    /// Forward as a `send_literal` frame (§7.7) — the console's VT input translation already
    /// produced canonical byte sequences, so literal forwarding is byte-exact attach parity.
    Literal(String),
    /// F12: detach locally, leaving the agent running.
    Detach,
}

/// Splits a raw stdin byte stream into `send_literal` text and F12 detach events.
///
/// Holds back (a) any buffer tail that is a strict prefix of [`DETACH_SEQ`] and (b) any
/// incomplete trailing UTF-8 character, so sequences split across reads reassemble. The
/// runtime calls [`InputScanner::flush`] after a short idle timeout so a bare Esc keypress
/// (a strict prefix of the detach sequence) still reaches the agent promptly.
#[derive(Default)]
pub struct InputScanner {
    pending: Vec<u8>,
}

impl InputScanner {
    /// Feed a chunk of raw bytes; returns the completed actions, in input order.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<InputAction> {
        self.pending.extend_from_slice(bytes);
        let buf = std::mem::take(&mut self.pending);
        let mut actions = Vec::new();
        let mut lit: Vec<u8> = Vec::new();
        let mut i = 0;
        while i < buf.len() {
            let rest = &buf[i..];
            if rest.starts_with(DETACH_SEQ) {
                emit_literal(&mut actions, &mut lit);
                actions.push(InputAction::Detach);
                i += DETACH_SEQ.len();
            } else if DETACH_SEQ.starts_with(rest) {
                // The whole remainder is a strict prefix of the detach sequence: hold it
                // back until more bytes (or a flush) decide what it is.
                break;
            } else {
                lit.push(buf[i]);
                i += 1;
            }
        }
        if i < buf.len() {
            self.pending = buf[i..].to_vec();
        } else {
            // Hold back an incomplete trailing UTF-8 character so a scalar split across
            // reads is forwarded whole.
            let cut = utf8_complete_len(&lit);
            self.pending = lit.split_off(cut);
        }
        emit_literal(&mut actions, &mut lit);
        actions
    }

    /// Forward whatever is held back (idle-timeout path, e.g. a bare Esc keypress).
    pub fn flush(&mut self) -> Option<InputAction> {
        if self.pending.is_empty() {
            return None;
        }
        let held = std::mem::take(&mut self.pending);
        Some(InputAction::Literal(
            String::from_utf8_lossy(&held).into_owned(),
        ))
    }

    /// Whether bytes are held back (the runtime arms the flush timer off this).
    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }
}

fn emit_literal(actions: &mut Vec<InputAction>, lit: &mut Vec<u8>) {
    if !lit.is_empty() {
        actions.push(InputAction::Literal(
            String::from_utf8_lossy(lit).into_owned(),
        ));
        lit.clear();
    }
}

/// Length of the longest prefix of `bytes` that ends on a UTF-8 character boundary; the
/// remainder (at most 3 bytes) is an incomplete trailing character.
fn utf8_complete_len(bytes: &[u8]) -> usize {
    match std::str::from_utf8(bytes) {
        Ok(_) => bytes.len(),
        Err(e) if e.error_len().is_none() => e.valid_up_to(),
        // Invalid (not merely incomplete) UTF-8: forward everything and let the lossy
        // conversion replace the bad bytes.
        Err(_) => bytes.len(),
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// `repomon attach-host <window>` on non-Windows: agents run under tmux there.
#[cfg(not(windows))]
pub async fn run(_session: &str, _window: &str) -> Result<()> {
    bail!(
        "`repomon attach-host` is Windows-only: it attaches to a repomon-agent-host named \
         pipe. On macOS/Linux agents run under tmux — attach from the TUI instead."
    )
}

/// Attach the current console to the host serving `session`/`window` (raw byte proxy).
#[cfg(windows)]
pub async fn run(session: &str, window: &str) -> Result<()> {
    windows_impl::run(session, window).await
}

// ---------------------------------------------------------------------------
// Windows runtime
// ---------------------------------------------------------------------------

#[cfg(windows)]
mod windows_impl {
    use std::io::{Read as _, Write as _};
    use std::time::{Duration, Instant};

    use anyhow::{Context, Result, bail};
    use serde_json::Value;
    use tokio::io::{ReadHalf, WriteHalf};
    use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeClient};
    use tokio::sync::mpsc;

    use super::{InputAction, InputScanner, StreamEvent, StreamState};

    /// How long to keep retrying `ERROR_PIPE_BUSY` before giving up on a connect.
    const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
    /// Idle time before held-back stdin bytes (e.g. a bare Esc keypress) flush through.
    const ESC_FLUSH: Duration = Duration::from_millis(50);
    /// Console size poll cadence; `resize` is last-client-wins (§7.6).
    const RESIZE_POLL: Duration = Duration::from_millis(200);
    /// All pipe-server instances are busy; retry shortly (winerror `ERROR_PIPE_BUSY`).
    const ERROR_PIPE_BUSY: i32 = 231;

    enum Outcome {
        /// F12: leave the agent running.
        Detached,
        /// The host exited (agent gone) — pipe EOF on the stream connection.
        Closed,
    }

    pub async fn run(session: &str, window: &str) -> Result<()> {
        let pipe = super::pipe_name(session, window);

        // Control connection: hello / resize / send_literal, request-response forever.
        // Input cannot share the stream connection — after `subscribe_bytes` the host
        // ignores client frames on it (§5), so we hold two connections (§2 allows this).
        let (r, w) = tokio::io::split(connect(&pipe).await?);
        let mut ctrl = Ctrl { r, w, next_id: 0 };
        let hello = ctrl.request(super::req_hello).await?;
        let proto = hello.get("proto").and_then(Value::as_u64).unwrap_or(0);
        if proto != 1 {
            bail!("host speaks protocol version {proto}; this client speaks 1");
        }

        // From here we own the console: VT modes + UTF-8 codepages, restored on exit.
        let guard = console::VtGuard::activate()
            .context("`repomon attach-host` needs an interactive console")?;

        // Impose our size first (last-client-wins), *then* subscribe, so the replay
        // frame is rendered at the size it is about to be displayed at.
        let mut last_size = console_size();
        if let Some((cols, rows)) = last_size {
            report(ctrl.request(|id| super::req_resize(id, cols, rows)).await);
        }

        // Stream connection: `subscribe_bytes`; the first frame is a full-screen replay.
        let (mut sr, mut sw) = tokio::io::split(connect(&pipe).await?);
        let mut state = StreamState::new(1);
        super::write_frame(&mut sw, &super::req_subscribe(1)).await?;

        {
            let mut out = std::io::stdout().lock();
            let _ = write!(out, "\x1b]0;repomon: {window}\x07");
            let _ = out.flush();
        }

        // Dedicated mirror task: a slow control round-trip must never stall output.
        let mut mirror = tokio::spawn(async move {
            loop {
                match super::read_frame(&mut sr).await? {
                    None => return Ok::<(), anyhow::Error>(()), // host exited
                    Some(v) => match state.on_frame(&v)? {
                        StreamEvent::Ack => {}
                        StreamEvent::Bytes(b) => {
                            let mut out = std::io::stdout().lock();
                            out.write_all(&b)?;
                            out.flush()?;
                        }
                    },
                }
            }
        });

        // Raw stdin pump on a plain thread (console reads are blocking). With VT input
        // enabled the console delivers canonical VT byte sequences here.
        let (tx, mut stdin_rx) = mpsc::channel::<Vec<u8>>(64);
        std::thread::spawn(move || {
            let mut stdin = std::io::stdin();
            let mut buf = [0u8; 4096];
            loop {
                match stdin.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if tx.blocking_send(buf[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                }
            }
        });

        let mut scanner = InputScanner::default();
        let mut resize_tick = tokio::time::interval(RESIZE_POLL);
        let mut stdin_open = true;
        let outcome = loop {
            let flush_timer = tokio::time::sleep(ESC_FLUSH);
            tokio::pin!(flush_timer);
            tokio::select! {
                res = &mut mirror => {
                    match res {
                        Ok(Ok(())) => break Outcome::Closed,
                        Ok(Err(e)) => return Err(e.context("byte stream failed")),
                        Err(e) => return Err(anyhow::Error::new(e).context("mirror task failed")),
                    }
                }
                chunk = stdin_rx.recv(), if stdin_open => match chunk {
                    // stdin is gone; keep mirroring output (view-only), like tmux.
                    None => stdin_open = false,
                    Some(bytes) => {
                        if forward(&mut ctrl, scanner.push(&bytes)).await? {
                            break Outcome::Detached;
                        }
                    }
                },
                _ = &mut flush_timer, if scanner.has_pending() => {
                    forward(&mut ctrl, scanner.flush().into_iter().collect()).await?;
                }
                _ = resize_tick.tick() => {
                    let size = console_size();
                    if let Some((cols, rows)) = size
                        && size != last_size
                    {
                        last_size = size;
                        report(ctrl.request(|id| super::req_resize(id, cols, rows)).await);
                    }
                }
            }
        };

        mirror.abort();
        // The agent may have left the console on the alternate screen; come back to a
        // sane primary screen before the guard restores the original modes.
        {
            let mut out = std::io::stdout().lock();
            let _ = out.write_all(b"\x1b[?1049l\x1b[?25h\x1b[0m\r\n");
            let _ = out.flush();
        }
        drop(guard);
        match outcome {
            Outcome::Detached => println!("[repomon] detached — agent keeps running"),
            Outcome::Closed => println!("[repomon] window {window:?} closed (agent exited)"),
        }
        Ok(())
    }

    /// The control connection: one in-flight request at a time (the host answers in
    /// order, §5), with client-chosen incrementing ids.
    struct Ctrl {
        r: ReadHalf<NamedPipeClient>,
        w: WriteHalf<NamedPipeClient>,
        next_id: u64,
    }

    impl Ctrl {
        async fn request(&mut self, build: impl FnOnce(u64) -> Value) -> Result<Value> {
            self.next_id += 1;
            let id = self.next_id;
            super::write_frame(&mut self.w, &build(id)).await?;
            match super::read_frame(&mut self.r).await? {
                None => bail!("host closed the control connection"),
                Some(v) => super::parse_response(&v, id),
            }
        }
    }

    /// Send scanner actions to the host. Returns `true` on detach. Connection failures
    /// are fatal; a host-side `err` on one input frame is reported and skipped.
    async fn forward(ctrl: &mut Ctrl, actions: Vec<InputAction>) -> Result<bool> {
        for action in actions {
            match action {
                InputAction::Detach => return Ok(true),
                InputAction::Literal(text) => {
                    match ctrl.request(|id| super::req_send_literal(id, &text)).await {
                        Ok(_) => {}
                        Err(e) if e.downcast_ref::<std::io::Error>().is_some() => {
                            return Err(e.context("control connection failed"));
                        }
                        Err(e) => report::<Value>(Err(e)),
                    }
                }
            }
        }
        Ok(false)
    }

    /// Non-fatal host/request errors go to stderr; the attach keeps running.
    fn report<T>(res: Result<T>) {
        if let Err(e) = res {
            eprintln!("repomon attach-host: {e}");
        }
    }

    /// Connect to a host pipe, retrying `ERROR_PIPE_BUSY` (all server instances taken).
    async fn connect(pipe: &str) -> Result<NamedPipeClient> {
        let start = Instant::now();
        loop {
            match ClientOptions::new().open(pipe) {
                Ok(c) => return Ok(c),
                Err(e)
                    if e.raw_os_error() == Some(ERROR_PIPE_BUSY)
                        && start.elapsed() < CONNECT_TIMEOUT =>
                {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                Err(e) => {
                    return Err(anyhow::Error::new(e).context(format!(
                        "can't connect to {pipe} — is the agent's host process running?"
                    )));
                }
            }
        }
    }

    /// Current console size in (cols, rows), if there is a console.
    fn console_size() -> Option<(u16, u16)> {
        ratatui::crossterm::terminal::size().ok()
    }

    /// Console-mode plumbing via a tiny hand-rolled kernel32 binding (keeps the crate
    /// free of new Windows dependencies; `cargo check` needs no import libraries).
    mod console {
        use anyhow::{Result, bail};
        use core::ffi::c_void;

        type Handle = *mut c_void;

        const STD_INPUT_HANDLE: u32 = -10i32 as u32;
        const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
        const ENABLE_VIRTUAL_TERMINAL_INPUT: u32 = 0x0200;
        const ENABLE_PROCESSED_OUTPUT: u32 = 0x0001;
        const ENABLE_WRAP_AT_EOL_OUTPUT: u32 = 0x0002;
        const ENABLE_VIRTUAL_TERMINAL_PROCESSING: u32 = 0x0004;
        const DISABLE_NEWLINE_AUTO_RETURN: u32 = 0x0008;
        const CP_UTF8: u32 = 65001;

        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetStdHandle(std_handle: u32) -> Handle;
            fn GetConsoleMode(handle: Handle, mode: *mut u32) -> i32;
            fn SetConsoleMode(handle: Handle, mode: u32) -> i32;
            fn GetConsoleCP() -> u32;
            fn GetConsoleOutputCP() -> u32;
            fn SetConsoleCP(codepage: u32) -> i32;
            fn SetConsoleOutputCP(codepage: u32) -> i32;
        }

        /// RAII console state: VT-raw stdin (`ENABLE_VIRTUAL_TERMINAL_INPUT` only — no
        /// line buffering, echo, or Ctrl-C processing), VT-processing stdout, UTF-8
        /// codepages both ways. Everything is restored on drop.
        pub struct VtGuard {
            stdin: Handle,
            stdout: Handle,
            in_mode: u32,
            out_mode: u32,
            in_cp: u32,
            out_cp: u32,
        }

        // SAFETY: the wrapped values are process-global console pseudo-handles that are
        // not thread-affine; the guard only reads/sets console modes with them.
        unsafe impl Send for VtGuard {}

        impl VtGuard {
            pub fn activate() -> Result<Self> {
                unsafe {
                    let stdin = GetStdHandle(STD_INPUT_HANDLE);
                    let stdout = GetStdHandle(STD_OUTPUT_HANDLE);
                    let mut in_mode = 0u32;
                    let mut out_mode = 0u32;
                    if GetConsoleMode(stdin, &mut in_mode) == 0
                        || GetConsoleMode(stdout, &mut out_mode) == 0
                    {
                        bail!("stdin/stdout is not attached to a console");
                    }
                    let guard = Self {
                        stdin,
                        stdout,
                        in_mode,
                        out_mode,
                        in_cp: GetConsoleCP(),
                        out_cp: GetConsoleOutputCP(),
                    };
                    if SetConsoleMode(stdin, ENABLE_VIRTUAL_TERMINAL_INPUT) == 0 {
                        bail!("couldn't enable VT input on stdin");
                    }
                    let out_want = out_mode
                        | ENABLE_PROCESSED_OUTPUT
                        | ENABLE_WRAP_AT_EOL_OUTPUT
                        | ENABLE_VIRTUAL_TERMINAL_PROCESSING
                        | DISABLE_NEWLINE_AUTO_RETURN;
                    if SetConsoleMode(stdout, out_want) == 0 {
                        SetConsoleMode(stdin, in_mode);
                        bail!("couldn't enable VT processing on stdout");
                    }
                    // UTF-8 codepages so raw stdin reads hand us UTF-8 for send_literal.
                    SetConsoleCP(CP_UTF8);
                    SetConsoleOutputCP(CP_UTF8);
                    Ok(guard)
                }
            }
        }

        impl Drop for VtGuard {
            fn drop(&mut self) {
                unsafe {
                    SetConsoleMode(self.stdin, self.in_mode);
                    SetConsoleMode(self.stdout, self.out_mode);
                    SetConsoleCP(self.in_cp);
                    SetConsoleOutputCP(self.out_cp);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio::io::AsyncWriteExt;

    // ---- framing (§4) ----

    #[tokio::test]
    async fn frame_roundtrip_over_duplex() {
        let (mut a, mut b) = tokio::io::duplex(64 * 1024);
        let req = json!({"id": 1, "op": "hello"});
        let res = json!({"id": 1, "ok": {"proto": 1}});
        write_frame(&mut a, &req).await.unwrap();
        write_frame(&mut a, &res).await.unwrap();
        drop(a);
        assert_eq!(read_frame(&mut b).await.unwrap(), Some(req));
        assert_eq!(read_frame(&mut b).await.unwrap(), Some(res));
        // Clean EOF at a frame boundary -> None.
        assert_eq!(read_frame(&mut b).await.unwrap(), None);
    }

    #[tokio::test]
    async fn frame_encoding_is_u32_le_length_prefixed_json() {
        let (mut a, mut b) = tokio::io::duplex(1024);
        write_frame(&mut a, &json!({"id": 4, "op": "size"}))
            .await
            .unwrap();
        drop(a);
        let mut buf = Vec::new();
        tokio::io::AsyncReadExt::read_to_end(&mut b, &mut buf)
            .await
            .unwrap();
        let len = u32::from_le_bytes(buf[..4].try_into().unwrap()) as usize;
        assert_eq!(len, buf.len() - 4);
        let v: serde_json::Value = serde_json::from_slice(&buf[4..]).unwrap();
        assert_eq!(v, json!({"id": 4, "op": "size"}));
    }

    #[tokio::test]
    async fn read_frame_split_across_writes() {
        let (mut a, mut b) = tokio::io::duplex(1024);
        let frame = encode_frame(&json!({"id": 2, "op": "capture"})).unwrap();
        let (head, tail) = frame.split_at(3);
        let head = head.to_vec();
        let tail = tail.to_vec();
        let writer = tokio::spawn(async move {
            a.write_all(&head).await.unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            a.write_all(&tail).await.unwrap();
        });
        let got = read_frame(&mut b).await.unwrap();
        writer.await.unwrap();
        assert_eq!(got, Some(json!({"id": 2, "op": "capture"})));
    }

    #[tokio::test]
    async fn read_frame_rejects_oversize_length() {
        let (mut a, mut b) = tokio::io::duplex(1024);
        // 16 MiB + 1: peer must treat the connection as corrupt (§4).
        a.write_all(&(MAX_FRAME + 1).to_le_bytes()).await.unwrap();
        let err = read_frame(&mut b).await.unwrap_err();
        assert!(err.to_string().contains("frame"), "err: {err}");
    }

    #[tokio::test]
    async fn read_frame_errors_on_truncated_frame() {
        let (mut a, mut b) = tokio::io::duplex(1024);
        a.write_all(&10u32.to_le_bytes()).await.unwrap();
        a.write_all(b"{\"id\"").await.unwrap();
        drop(a); // EOF mid-payload
        assert!(read_frame(&mut b).await.is_err());
    }

    // ---- pipe naming (§2) ----

    #[test]
    fn pipe_name_matches_protocol_example() {
        assert_eq!(
            pipe_name("repomon", "lane-3-1"),
            r"\\.\pipe\repomon-repomon-lane-3-1"
        );
    }

    // ---- request builders (§7) ----

    #[test]
    fn request_builders_match_protocol_shapes() {
        assert_eq!(req_hello(1), json!({"id": 1, "op": "hello"}));
        assert_eq!(
            req_resize(6, 190, 45),
            json!({"id": 6, "op": "resize", "cols": 190, "rows": 45})
        );
        assert_eq!(
            req_send_literal(7, "y"),
            json!({"id": 7, "op": "send_literal", "text": "y"})
        );
        assert_eq!(
            req_subscribe(11),
            json!({"id": 11, "op": "subscribe_bytes"})
        );
    }

    // ---- response parsing (§5) ----

    #[test]
    fn parse_response_returns_ok_payload() {
        let v = json!({"id": 3, "ok": {"col": 12, "row": 4, "visible": true}});
        let ok = parse_response(&v, 3).unwrap();
        assert_eq!(ok["row"], 4);
    }

    #[test]
    fn parse_response_surfaces_host_err() {
        let v = json!({"id": 9, "err": "unknown key"});
        let err = parse_response(&v, 9).unwrap_err();
        assert!(err.to_string().contains("unknown key"));
    }

    #[test]
    fn parse_response_rejects_id_mismatch() {
        let v = json!({"id": 2, "ok": {}});
        assert!(parse_response(&v, 1).is_err());
    }

    // ---- stream frames (§7.11) ----

    #[test]
    fn parse_stream_frame_decodes_standard_base64() {
        let v = json!({"stream": "bytes", "data": "aGVsbG8="});
        assert_eq!(parse_stream_frame(&v).unwrap(), b"hello");
    }

    #[test]
    fn parse_stream_frame_rejects_other_shapes() {
        assert!(parse_stream_frame(&json!({"id": 1, "ok": {}})).is_err());
        assert!(parse_stream_frame(&json!({"stream": "bytes", "data": "!!"})).is_err());
    }

    // ---- subscription ordering: ack, then replay-first byte frames ----

    #[test]
    fn stream_state_requires_ack_then_yields_bytes_in_order() {
        let mut st = StreamState::new(11);
        let ack = st.on_frame(&json!({"id": 11, "ok": {}})).unwrap();
        assert!(matches!(ack, StreamEvent::Ack));
        // First frame after the ack is the full-screen replay; it and every later
        // frame surface as Bytes, in order, verbatim.
        let replay = st
            .on_frame(&json!({"stream": "bytes", "data": "G1sySg=="})) // ESC[2J
            .unwrap();
        assert!(matches!(replay, StreamEvent::Bytes(ref b) if b == b"\x1b[2J"));
        let chunk = st
            .on_frame(&json!({"stream": "bytes", "data": "aGk="}))
            .unwrap();
        assert!(matches!(chunk, StreamEvent::Bytes(ref b) if b == b"hi"));
    }

    #[test]
    fn stream_state_rejects_bytes_before_ack_and_err_ack() {
        let mut st = StreamState::new(11);
        assert!(
            st.on_frame(&json!({"stream": "bytes", "data": "aGk="}))
                .is_err()
        );
        let mut st = StreamState::new(11);
        assert!(st.on_frame(&json!({"id": 11, "err": "nope"})).is_err());
    }

    // ---- input scanner: raw VT stdin bytes -> send_literal actions + F12 detach ----

    fn literals(actions: &[InputAction]) -> Vec<String> {
        actions
            .iter()
            .filter_map(|a| match a {
                InputAction::Literal(s) => Some(s.clone()),
                InputAction::Detach => None,
            })
            .collect()
    }

    #[test]
    fn scanner_passes_plain_text_through() {
        let mut sc = InputScanner::default();
        let actions = sc.push(b"hello\r");
        assert_eq!(literals(&actions), vec!["hello\r"]);
        assert!(!sc.has_pending());
    }

    #[test]
    fn scanner_detects_f12_alone_as_detach() {
        let mut sc = InputScanner::default();
        let actions = sc.push(b"\x1b[24~");
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], InputAction::Detach));
    }

    #[test]
    fn scanner_keeps_text_around_detach_in_order() {
        let mut sc = InputScanner::default();
        let actions = sc.push(b"abc\x1b[24~def");
        assert!(matches!(&actions[0], InputAction::Literal(s) if s == "abc"));
        assert!(matches!(actions[1], InputAction::Detach));
        assert!(matches!(&actions[2], InputAction::Literal(s) if s == "def"));
    }

    #[test]
    fn scanner_detects_f12_split_at_every_boundary() {
        let seq = b"\x1b[24~";
        for cut in 1..seq.len() {
            let mut sc = InputScanner::default();
            let first = sc.push(&seq[..cut]);
            assert!(first.is_empty(), "cut {cut}: prefix must be held back");
            assert!(sc.has_pending());
            let second = sc.push(&seq[cut..]);
            assert_eq!(second.len(), 1, "cut {cut}");
            assert!(matches!(second[0], InputAction::Detach), "cut {cut}");
        }
    }

    #[test]
    fn scanner_flush_forwards_a_lone_escape() {
        // A bare Esc keypress is a strict prefix of the detach sequence; after the
        // idle timeout the runtime flushes it through as input.
        let mut sc = InputScanner::default();
        assert!(sc.push(b"\x1b").is_empty());
        assert!(sc.has_pending());
        let flushed = sc.flush();
        assert!(matches!(&flushed, Some(InputAction::Literal(s)) if s == "\x1b"));
        assert!(!sc.has_pending());
        assert!(sc.flush().is_none());
    }

    #[test]
    fn scanner_reassembles_utf8_split_across_reads() {
        let bytes = "héllo".as_bytes(); // é = 0xC3 0xA9
        let mut sc = InputScanner::default();
        let first = sc.push(&bytes[..2]); // "h" + first byte of é
        assert_eq!(literals(&first), vec!["h"]);
        assert!(sc.has_pending());
        let second = sc.push(&bytes[2..]);
        assert_eq!(literals(&second), vec!["éllo"]);
        assert!(!sc.has_pending());
    }

    // ---- entry point ----

    #[cfg(not(windows))]
    #[tokio::test]
    async fn run_bails_with_a_windows_only_error_on_unix() {
        let err = run("repomon", "lane-1-1").await.unwrap_err();
        assert!(err.to_string().contains("Windows"), "err: {err}");
    }

    #[test]
    fn scanner_forwards_non_detach_escape_sequences_verbatim() {
        // Arrow keys etc. arrive as VT sequences under ENABLE_VIRTUAL_TERMINAL_INPUT
        // and must reach the agent unmodified.
        let mut sc = InputScanner::default();
        let actions = sc.push(b"\x1b[A\x1b[B");
        assert_eq!(literals(&actions), vec!["\x1b[A\x1b[B"]);
    }
}
