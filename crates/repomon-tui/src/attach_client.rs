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
    ensure!(len <= MAX_FRAME, "frame length {len} exceeds 16 MiB — corrupt connection");
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
    ensure!(got == id, "response id {got} does not match request id {id}");
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
        assert_eq!(req_subscribe(11), json!({"id": 11, "op": "subscribe_bytes"}));
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
        assert!(
            st.on_frame(&json!({"id": 11, "err": "nope"}))
                .is_err()
        );
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

    #[test]
    fn scanner_forwards_non_detach_escape_sequences_verbatim() {
        // Arrow keys etc. arrive as VT sequences under ENABLE_VIRTUAL_TERMINAL_INPUT
        // and must reach the agent unmodified.
        let mut sc = InputScanner::default();
        let actions = sc.push(b"\x1b[A\x1b[B");
        assert_eq!(literals(&actions), vec!["\x1b[A\x1b[B"]);
    }
}
