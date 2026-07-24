//! Request dispatch: protocol op → screen/PTY action → response payload.
//!
//! OS-neutral on purpose: the PTY side is behind [`PtyIo`], so every op's semantics are
//! tested on all OSes with a fake; the Windows server plugs in the real ConPTY.

use serde::Serialize;

use crate::protocol::{self, AlternateOk, CaptureOk, CursorOk, HelloInfo, Op, Response, SizeOk};
use crate::screen::Screen;

/// The PTY side of an op, as the dispatcher sees it. The Windows server implements this
/// over the real ConPTY; tests use a recording fake.
pub trait PtyIo: Send {
    fn write(&mut self, bytes: &[u8]) -> anyhow::Result<()>;
    fn resize(&mut self, cols: u16, rows: u16) -> anyhow::Result<()>;
    fn kill(&mut self) -> anyhow::Result<()>;
}

/// Immutable facts about this host, reported by `hello` and mirrored in the registry file.
pub struct HostMeta {
    pub session: String,
    pub window: String,
    pub cwd: String,
    pub program: String,
    pub args: Vec<String>,
    pub agent_pid: u32,
    pub owner: String,
    /// Unix epoch seconds.
    pub started_at: i64,
}

/// What the connection loop must do after sending the response frame.
#[derive(Debug, Clone, Copy)]
pub enum Effect {
    None,
    /// `subscribe_bytes` succeeded: switch this connection to stream mode. The first pushed
    /// frame must wrap [`Dispatcher::replay`].
    StartStream,
    /// `kill` was served: remove the registry entry and exit the host.
    Shutdown,
}

/// One host's shared brain: the vt100 screen, the PTY handle, and activity bookkeeping.
/// The server wraps it in a mutex; every op is a short synchronous critical section.
pub struct Dispatcher {
    meta: HostMeta,
    screen: Screen,
    pty: Box<dyn PtyIo>,
    last_activity: i64,
    /// Up to 3 trailing output bytes carried between chunks so a DSR query split across a read
    /// boundary is still detected (see [`Dispatcher::answer_dsr`]).
    dsr_tail: Vec<u8>,
}

impl Dispatcher {
    pub fn new(meta: HostMeta, screen: Screen, pty: Box<dyn PtyIo>) -> Self {
        let last_activity = meta.started_at;
        Self {
            meta,
            screen,
            pty,
            last_activity,
            dsr_tail: Vec::new(),
        }
    }

    /// Feed a chunk of PTY output (`now` = epoch seconds), tmux `#{window_activity}` parity.
    pub fn process_output(&mut self, bytes: &[u8], now: i64) {
        self.screen.process(bytes);
        self.last_activity = now;
        self.answer_dsr(bytes);
    }

    /// Answer DSR cursor-position reports (`ESC [ 6 n`) in the output stream.
    ///
    /// ConPTY emits one at startup as part of its `PSUEDOCONSOLE_INHERIT_CURSOR` handshake (and
    /// interactive apps query the cursor mid-run) and withholds further output until the
    /// terminal replies with a CPR (`ESC [ row ; col R`, 1-based) on the PTY input. This host IS
    /// the terminal, so without this reply ConPTY stalls and no application output ever appears.
    fn answer_dsr(&mut self, bytes: &[u8]) {
        const DSR_CPR: &[u8] = b"\x1b[6n";
        let mut scan = std::mem::take(&mut self.dsr_tail);
        scan.extend_from_slice(bytes);
        let hits = scan
            .windows(DSR_CPR.len())
            .filter(|w| *w == DSR_CPR)
            .count();
        for _ in 0..hits {
            let (col, row, _) = self.screen.cursor();
            let reply = format!("\x1b[{};{}R", row + 1, col + 1);
            let _ = self.pty.write(reply.as_bytes());
        }
        // Carry the last few bytes so a sequence straddling the next read is still matched; a
        // partial (< 4 bytes) tail can never itself complete a match, so nothing double-counts.
        let keep = scan.len().min(DSR_CPR.len() - 1);
        self.dsr_tail = scan.split_off(scan.len() - keep);
    }

    /// The full current-screen replay for a new byte subscriber's first frame.
    pub fn replay(&self) -> Vec<u8> {
        self.screen.replay()
    }

    /// Handle one request frame; returns the response payload (JSON, unframed) and the
    /// connection effect. Never panics on bad input — errors become `err` responses.
    pub fn handle(&mut self, payload: &[u8], _now: i64) -> (Vec<u8>, Effect) {
        let req = match protocol::parse_request(payload) {
            Ok(req) => req,
            Err(e) => {
                return (
                    to_vec(&Response::err(e.id.unwrap_or(0), e.message)),
                    Effect::None,
                );
            }
        };
        let id = req.id;
        let (payload, effect) = match req.op {
            Op::Hello => (
                to_vec(&Response::ok(
                    id,
                    &HelloInfo {
                        proto: protocol::PROTO_VERSION,
                        session: self.meta.session.clone(),
                        window: self.meta.window.clone(),
                        cwd: self.meta.cwd.clone(),
                        program: self.meta.program.clone(),
                        args: self.meta.args.clone(),
                        agent_pid: self.meta.agent_pid,
                        host_pid: std::process::id(),
                        started_at: self.meta.started_at,
                        last_activity: self.last_activity,
                        owner: self.meta.owner.clone(),
                    },
                )),
                Effect::None,
            ),
            Op::Capture { lines } => (
                to_vec(&Response::ok(
                    id,
                    &CaptureOk {
                        text: self.screen.capture(lines),
                    },
                )),
                Effect::None,
            ),
            Op::Cursor => {
                let (col, row, visible) = self.screen.cursor();
                (
                    to_vec(&Response::ok(id, &CursorOk { col, row, visible })),
                    Effect::None,
                )
            }
            Op::Size => {
                let (cols, rows) = self.screen.size();
                (
                    to_vec(&Response::ok(id, &SizeOk { cols, rows })),
                    Effect::None,
                )
            }
            Op::AlternateOn => (
                to_vec(&Response::ok(
                    id,
                    &AlternateOk {
                        on: self.screen.alternate_on(),
                    },
                )),
                Effect::None,
            ),
            Op::Resize { cols, rows } => match self.pty.resize(cols, rows) {
                Ok(()) => {
                    // Last client wins: whatever arrived most recently is the size.
                    self.screen.resize(cols, rows);
                    (to_vec(&Response::empty_ok(id)), Effect::None)
                }
                Err(e) => (
                    to_vec(&Response::err(id, format!("resize: {e:#}"))),
                    Effect::None,
                ),
            },
            Op::SendLiteral { text } => (self.write_ok(id, text.as_bytes()), Effect::None),
            Op::SendText { text } => {
                let mut bytes = text.into_bytes();
                bytes.push(b'\r');
                (self.write_ok(id, &bytes), Effect::None)
            }
            Op::SendKey { key } => match crate::keys::key_to_bytes(&key) {
                Some(bytes) => (self.write_ok(id, &bytes), Effect::None),
                None => (
                    to_vec(&Response::err(id, format!("unknown key {key:?}"))),
                    Effect::None,
                ),
            },
            Op::ScrollWheel {
                up,
                ticks,
                col,
                row,
            } => {
                if ticks == 0 {
                    (to_vec(&Response::empty_ok(id)), Effect::None)
                } else {
                    let button = if up { 64 } else { 65 };
                    let seq = format!("\x1b[<{button};{};{}M", col.max(1), row.max(1))
                        .repeat(ticks as usize);
                    (self.write_ok(id, seq.as_bytes()), Effect::None)
                }
            }
            Op::SubscribeBytes => (to_vec(&Response::empty_ok(id)), Effect::StartStream),
            Op::Kill => match self.pty.kill() {
                Ok(()) => (to_vec(&Response::empty_ok(id)), Effect::Shutdown),
                Err(e) => (
                    to_vec(&Response::err(id, format!("kill: {e:#}"))),
                    Effect::Shutdown,
                ),
            },
        };
        (cap_frame(id, payload), effect)
    }

    fn write_ok(&mut self, id: u64, bytes: &[u8]) -> Vec<u8> {
        match self.pty.write(bytes) {
            Ok(()) => to_vec(&Response::empty_ok(id)),
            Err(e) => to_vec(&Response::err(id, format!("write: {e:#}"))),
        }
    }
}

/// PROTOCOL.md §4 binds both directions to [`crate::codec::MAX_FRAME`]: a response that
/// would overflow it (a pathological deep-history capture) becomes an `err` instead of a
/// frame the client must treat as corrupt.
fn cap_frame(id: u64, payload: Vec<u8>) -> Vec<u8> {
    if payload.len() > crate::codec::MAX_FRAME {
        to_vec(&Response::err(
            id,
            "response exceeds the 16 MiB frame limit",
        ))
    } else {
        payload
    }
}

fn to_vec<T: Serialize>(v: &T) -> Vec<u8> {
    serde_json::to_vec(v).expect("response serializes")
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum PtyCall {
        Write(Vec<u8>),
        Resize(u16, u16),
        Kill,
    }

    #[derive(Clone, Default)]
    struct FakePty {
        calls: Arc<Mutex<Vec<PtyCall>>>,
    }

    impl PtyIo for FakePty {
        fn write(&mut self, bytes: &[u8]) -> anyhow::Result<()> {
            self.calls
                .lock()
                .unwrap()
                .push(PtyCall::Write(bytes.to_vec()));
            Ok(())
        }
        fn resize(&mut self, cols: u16, rows: u16) -> anyhow::Result<()> {
            self.calls.lock().unwrap().push(PtyCall::Resize(cols, rows));
            Ok(())
        }
        fn kill(&mut self) -> anyhow::Result<()> {
            self.calls.lock().unwrap().push(PtyCall::Kill);
            Ok(())
        }
    }

    fn dispatcher() -> (Dispatcher, Arc<Mutex<Vec<PtyCall>>>) {
        let pty = FakePty::default();
        let calls = pty.calls.clone();
        let meta = HostMeta {
            session: "repomon".into(),
            window: "lane-3-1".into(),
            cwd: "C:\\work".into(),
            program: "claude".into(),
            args: vec!["--plan".into()],
            agent_pid: 5678,
            owner: "tok123".into(),
            started_at: 40,
        };
        (
            Dispatcher::new(meta, crate::screen::Screen::new(80, 24), Box::new(pty)),
            calls,
        )
    }

    fn handle(d: &mut Dispatcher, req: &str) -> (serde_json::Value, Effect) {
        let (payload, effect) = d.handle(req.as_bytes(), 99);
        (serde_json::from_slice(&payload).unwrap(), effect)
    }

    #[test]
    fn hello_reports_meta_and_activity() {
        let (mut d, _) = dispatcher();
        d.process_output(b"boot noise", 42);
        let (v, effect) = handle(&mut d, r#"{"id":1,"op":"hello"}"#);
        assert!(matches!(effect, Effect::None));
        assert_eq!(v["id"], 1);
        let ok = &v["ok"];
        assert_eq!(ok["proto"], 1);
        assert_eq!(ok["session"], "repomon");
        assert_eq!(ok["window"], "lane-3-1");
        assert_eq!(ok["cwd"], "C:\\work");
        assert_eq!(ok["program"], "claude");
        assert_eq!(ok["args"][0], "--plan");
        assert_eq!(ok["agent_pid"], 5678);
        assert_eq!(ok["host_pid"], std::process::id());
        assert_eq!(ok["started_at"], 40);
        assert_eq!(ok["last_activity"], 42, "bumped by PTY output");
        assert_eq!(ok["owner"], "tok123");
    }

    #[test]
    fn last_activity_starts_at_started_at() {
        let (mut d, _) = dispatcher();
        let (v, _) = handle(&mut d, r#"{"id":1,"op":"hello"}"#);
        assert_eq!(v["ok"]["last_activity"], 40);
    }

    #[test]
    fn screen_queries_answer_from_the_vt100_state() {
        let (mut d, _) = dispatcher();
        d.process_output(b"hi there", 41);
        let (v, _) = handle(&mut d, r#"{"id":2,"op":"capture"}"#);
        assert!(v["ok"]["text"].as_str().unwrap().starts_with("hi there"));
        let (v, _) = handle(&mut d, r#"{"id":3,"op":"cursor"}"#);
        assert_eq!(
            (v["ok"]["col"].as_u64(), v["ok"]["row"].as_u64()),
            (Some(8), Some(0))
        );
        assert_eq!(v["ok"]["visible"], true);
        let (v, _) = handle(&mut d, r#"{"id":4,"op":"size"}"#);
        assert_eq!(
            (v["ok"]["cols"].as_u64(), v["ok"]["rows"].as_u64()),
            (Some(80), Some(24))
        );
        let (v, _) = handle(&mut d, r#"{"id":5,"op":"alternate_on"}"#);
        assert_eq!(v["ok"]["on"], false);
    }

    #[test]
    fn resize_hits_pty_and_screen() {
        let (mut d, calls) = dispatcher();
        let (v, _) = handle(&mut d, r#"{"id":6,"op":"resize","cols":100,"rows":30}"#);
        assert_eq!(v["ok"], serde_json::json!({}));
        assert_eq!(
            calls.lock().unwrap().as_slice(),
            &[PtyCall::Resize(100, 30)]
        );
        let (v, _) = handle(&mut d, r#"{"id":7,"op":"size"}"#);
        assert_eq!(
            (v["ok"]["cols"].as_u64(), v["ok"]["rows"].as_u64()),
            (Some(100), Some(30))
        );
    }

    #[test]
    fn send_literal_writes_exact_bytes() {
        let (mut d, calls) = dispatcher();
        handle(&mut d, r#"{"id":7,"op":"send_literal","text":"y"}"#);
        assert_eq!(
            calls.lock().unwrap().as_slice(),
            &[PtyCall::Write(b"y".to_vec())]
        );
    }

    #[test]
    fn send_text_appends_carriage_return() {
        let (mut d, calls) = dispatcher();
        handle(&mut d, r#"{"id":8,"op":"send_text","text":"continue"}"#);
        assert_eq!(
            calls.lock().unwrap().as_slice(),
            &[PtyCall::Write(b"continue\r".to_vec())]
        );
    }

    #[test]
    fn send_key_translates_and_rejects_unknown() {
        let (mut d, calls) = dispatcher();
        let (v, _) = handle(&mut d, r#"{"id":9,"op":"send_key","key":"C-c"}"#);
        assert_eq!(v["ok"], serde_json::json!({}));
        assert_eq!(
            calls.lock().unwrap().as_slice(),
            &[PtyCall::Write(vec![0x03])]
        );

        let (v, _) = handle(&mut d, r#"{"id":10,"op":"send_key","key":"Fnord"}"#);
        assert!(v["err"].as_str().unwrap().contains("Fnord"));
        assert_eq!(
            calls.lock().unwrap().len(),
            1,
            "no write for an unknown key"
        );
    }

    #[test]
    fn scroll_wheel_writes_sgr_sequences() {
        let (mut d, calls) = dispatcher();
        handle(
            &mut d,
            r#"{"id":11,"op":"scroll_wheel","up":true,"ticks":2,"col":12,"row":8}"#,
        );
        handle(
            &mut d,
            r#"{"id":12,"op":"scroll_wheel","up":false,"ticks":1,"col":4,"row":3}"#,
        );
        handle(
            &mut d,
            r#"{"id":13,"op":"scroll_wheel","up":true,"ticks":0}"#,
        );
        assert_eq!(
            calls.lock().unwrap().as_slice(),
            &[
                PtyCall::Write(b"\x1b[<64;12;8M\x1b[<64;12;8M".to_vec()),
                PtyCall::Write(b"\x1b[<65;4;3M".to_vec()),
            ],
            "64=up, 65=down, pointer coordinates preserved, ticks=0 writes nothing"
        );
    }

    #[test]
    fn subscribe_bytes_starts_streaming() {
        let (mut d, _) = dispatcher();
        let (v, effect) = handle(&mut d, r#"{"id":11,"op":"subscribe_bytes"}"#);
        assert_eq!(v["ok"], serde_json::json!({}));
        assert!(matches!(effect, Effect::StartStream));
    }

    #[test]
    fn kill_terminates_child_and_shuts_down() {
        let (mut d, calls) = dispatcher();
        let (v, effect) = handle(&mut d, r#"{"id":12,"op":"kill"}"#);
        assert_eq!(v["ok"], serde_json::json!({}));
        assert!(matches!(effect, Effect::Shutdown));
        assert_eq!(calls.lock().unwrap().as_slice(), &[PtyCall::Kill]);
    }

    #[test]
    fn unknown_op_answers_err_and_keeps_the_connection() {
        let (mut d, _) = dispatcher();
        let (v, effect) = handle(&mut d, r#"{"id":33,"op":"warp_core"}"#);
        assert_eq!(v["id"], 33);
        assert!(v["err"].is_string());
        assert!(matches!(effect, Effect::None));
    }

    #[test]
    fn garbage_answers_err_with_id_zero() {
        let (mut d, _) = dispatcher();
        let (v, effect) = handle(&mut d, "not json");
        assert_eq!(v["id"], 0);
        assert!(v["err"].is_string());
        assert!(matches!(effect, Effect::None));
    }

    #[test]
    fn oversized_response_payloads_become_errors() {
        // PROTOCOL.md §4 binds BOTH directions to the 16 MiB frame limit; a pathological
        // capture (50k history lines of escapes) must not make the host emit a frame the
        // client is required to treat as corrupt.
        let small = cap_frame(7, br#"{"id":7,"ok":{}}"#.to_vec());
        assert_eq!(
            small,
            br#"{"id":7,"ok":{}}"#.to_vec(),
            "small payloads pass through"
        );

        let huge = cap_frame(7, vec![b'x'; crate::codec::MAX_FRAME + 1]);
        let v: serde_json::Value = serde_json::from_slice(&huge).unwrap();
        assert_eq!(v["id"], 7);
        assert!(v["err"].as_str().unwrap().contains("frame limit"));
    }

    #[test]
    fn dsr_cursor_query_is_answered_with_a_cpr() {
        // ConPTY's inherit-cursor handshake sends `ESC [ 6 n` and blocks output until the
        // terminal replies with the cursor position; the host must answer on the PTY input.
        let (mut d, calls) = dispatcher();
        d.process_output(b"\x1b[6n", 41);
        assert_eq!(
            calls.lock().unwrap().as_slice(),
            &[PtyCall::Write(b"\x1b[1;1R".to_vec())],
            "fresh screen cursor at 0,0 replies 1;1 (1-based CPR)"
        );

        // A query after some output reports the live cursor position.
        calls.lock().unwrap().clear();
        d.process_output(b"abc\x1b[6n", 42);
        assert_eq!(
            calls.lock().unwrap().as_slice(),
            &[PtyCall::Write(b"\x1b[1;4R".to_vec())],
            "cursor after 'abc' is col 3 (0-based) -> 1;4"
        );
    }

    #[test]
    fn dsr_query_split_across_reads_is_still_answered() {
        let (mut d, calls) = dispatcher();
        d.process_output(b"hi\x1b[6", 41); // sequence straddles the read boundary
        assert!(
            calls.lock().unwrap().is_empty(),
            "no reply until the full sequence arrives"
        );
        d.process_output(b"n", 42);
        assert_eq!(
            calls.lock().unwrap().as_slice(),
            &[PtyCall::Write(b"\x1b[1;3R".to_vec())],
            "boundary-split CPR answered once; cursor after 'hi' is col 2 -> 1;3"
        );
    }

    #[test]
    fn replay_snapshot_matches_screen() {
        let (mut d, _) = dispatcher();
        d.process_output(b"snapshot me", 41);
        let replay = d.replay();
        let mut fresh = vt100::Parser::new(24, 80, 0);
        fresh.process(&replay);
        assert!(fresh.screen().contents().starts_with("snapshot me"));
    }
}
