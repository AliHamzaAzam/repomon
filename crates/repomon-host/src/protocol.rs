//! Serde types for the control protocol (PROTOCOL.md §5–§7). The golden-JSON tests below
//! protect the FROZEN wire shapes: if one of these breaks, the contract broke.

use base64::Engine as _;
use serde::{Deserialize, Serialize};

/// Protocol version answered in `hello`.
pub const PROTO_VERSION: u32 = 1;

/// A client request: `{"id": N, "op": "<name>", ...params}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    pub id: u64,
    #[serde(flatten)]
    pub op: Op,
}

/// The op name plus its parameters (PROTOCOL.md §7), tagged by `"op"`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    Hello,
    Capture {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lines: Option<u32>,
    },
    Cursor,
    Size,
    AlternateOn,
    Resize {
        cols: u16,
        rows: u16,
    },
    SendLiteral {
        text: String,
    },
    SendText {
        text: String,
    },
    SendKey {
        key: String,
    },
    ScrollWheel {
        up: bool,
        ticks: u32,
    },
    SubscribeBytes,
    Kill,
}

/// `hello`'s ok body — field order is part of nothing (JSON), but kept matching PROTOCOL.md
/// §7.1 for readable goldens.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelloInfo {
    pub proto: u32,
    pub session: String,
    pub window: String,
    pub cwd: String,
    pub program: String,
    pub args: Vec<String>,
    pub agent_pid: u32,
    pub host_pid: u32,
    pub started_at: i64,
    pub last_activity: i64,
    pub owner: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureOk {
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CursorOk {
    pub col: u16,
    pub row: u16,
    pub visible: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SizeOk {
    pub cols: u16,
    pub rows: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlternateOk {
    pub on: bool,
}

/// The `{}` ok body of side-effect ops (resize, send_*, …).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmptyOk {}

/// `{"id": N, "ok": {...}}` — generic so typed bodies serialize with their declared field
/// order (a `serde_json::Value` would re-sort keys alphabetically).
#[derive(Debug, Clone, Serialize)]
pub struct OkResponse<T> {
    pub id: u64,
    pub ok: T,
}

/// `{"id": N, "err": "message"}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrResponse {
    pub id: u64,
    pub err: String,
}

/// Constructors for the two response shapes.
pub struct Response;

impl Response {
    pub fn ok<T: Serialize>(id: u64, body: &T) -> OkResponse<&T> {
        OkResponse { id, ok: body }
    }

    pub fn empty_ok(id: u64) -> OkResponse<EmptyOk> {
        OkResponse { id, ok: EmptyOk {} }
    }

    pub fn err(id: u64, message: impl Into<String>) -> ErrResponse {
        ErrResponse {
            id,
            err: message.into(),
        }
    }
}

/// A pushed frame on a `subscribe_bytes` connection: `{"stream":"bytes","data":"<base64>"}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamFrame {
    pub stream: String,
    pub data: String,
}

impl StreamFrame {
    /// Wrap raw PTY bytes as a stream frame (standard base64, with padding).
    pub fn bytes(data: &[u8]) -> Self {
        Self {
            stream: "bytes".to_string(),
            data: base64::engine::general_purpose::STANDARD.encode(data),
        }
    }
}

/// A request that didn't parse. `id` is recovered when the payload was at least JSON with a
/// numeric `id`, so the host can answer `err` instead of disconnecting (PROTOCOL.md freeze
/// rule: unknown ops get an error response, never a hangup).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub id: Option<u64>,
    pub message: String,
}

/// Parse one frame payload into a [`Request`].
pub fn parse_request(payload: &[u8]) -> Result<Request, ParseError> {
    match serde_json::from_slice::<Request>(payload) {
        Ok(req) => Ok(req),
        Err(e) => {
            let id = serde_json::from_slice::<serde_json::Value>(payload)
                .ok()
                .and_then(|v| v.get("id").and_then(serde_json::Value::as_u64));
            Err(ParseError {
                id,
                message: e.to_string(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn to_json<T: serde::Serialize>(v: &T) -> String {
        serde_json::to_string(v).unwrap()
    }

    #[test]
    fn request_wire_shapes_are_frozen() {
        let cases: Vec<(Request, &str)> = vec![
            (
                Request {
                    id: 1,
                    op: Op::Hello,
                },
                r#"{"id":1,"op":"hello"}"#,
            ),
            (
                Request {
                    id: 2,
                    op: Op::Capture { lines: Some(500) },
                },
                r#"{"id":2,"op":"capture","lines":500}"#,
            ),
            (
                Request {
                    id: 2,
                    op: Op::Capture { lines: None },
                },
                r#"{"id":2,"op":"capture"}"#,
            ),
            (
                Request {
                    id: 3,
                    op: Op::Cursor,
                },
                r#"{"id":3,"op":"cursor"}"#,
            ),
            (
                Request {
                    id: 4,
                    op: Op::Size,
                },
                r#"{"id":4,"op":"size"}"#,
            ),
            (
                Request {
                    id: 5,
                    op: Op::AlternateOn,
                },
                r#"{"id":5,"op":"alternate_on"}"#,
            ),
            (
                Request {
                    id: 6,
                    op: Op::Resize {
                        cols: 190,
                        rows: 45,
                    },
                },
                r#"{"id":6,"op":"resize","cols":190,"rows":45}"#,
            ),
            (
                Request {
                    id: 7,
                    op: Op::SendLiteral { text: "y".into() },
                },
                r#"{"id":7,"op":"send_literal","text":"y"}"#,
            ),
            (
                Request {
                    id: 8,
                    op: Op::SendText {
                        text: "continue".into(),
                    },
                },
                r#"{"id":8,"op":"send_text","text":"continue"}"#,
            ),
            (
                Request {
                    id: 9,
                    op: Op::SendKey { key: "C-c".into() },
                },
                r#"{"id":9,"op":"send_key","key":"C-c"}"#,
            ),
            (
                Request {
                    id: 10,
                    op: Op::ScrollWheel { up: true, ticks: 3 },
                },
                r#"{"id":10,"op":"scroll_wheel","up":true,"ticks":3}"#,
            ),
            (
                Request {
                    id: 11,
                    op: Op::SubscribeBytes,
                },
                r#"{"id":11,"op":"subscribe_bytes"}"#,
            ),
            (
                Request {
                    id: 12,
                    op: Op::Kill,
                },
                r#"{"id":12,"op":"kill"}"#,
            ),
        ];
        for (req, golden) in cases {
            assert_eq!(to_json(&req), golden);
            assert_eq!(parse_request(golden.as_bytes()).unwrap(), req);
        }
    }

    #[test]
    fn hello_response_wire_shape_is_frozen() {
        let hello = HelloInfo {
            proto: 1,
            session: "repomon".into(),
            window: "lane-3-1".into(),
            cwd: "C:\\Users\\me\\code\\proj".into(),
            program: "claude".into(),
            args: vec!["--permission-mode".into(), "plan".into()],
            agent_pid: 5678,
            host_pid: 4321,
            started_at: 1789000000,
            last_activity: 1789000123,
            owner: "daemon-DESKTOP-ME-1a2b3c".into(),
        };
        assert_eq!(
            to_json(&Response::ok(1, &hello)),
            r#"{"id":1,"ok":{"proto":1,"session":"repomon","window":"lane-3-1","cwd":"C:\\Users\\me\\code\\proj","program":"claude","args":["--permission-mode","plan"],"agent_pid":5678,"host_pid":4321,"started_at":1789000000,"last_activity":1789000123,"owner":"daemon-DESKTOP-ME-1a2b3c"}}"#
        );
    }

    #[test]
    fn simple_ok_and_err_shapes_are_frozen() {
        assert_eq!(
            to_json(&Response::ok(2, &CaptureOk { text: "hi".into() })),
            r#"{"id":2,"ok":{"text":"hi"}}"#
        );
        assert_eq!(
            to_json(&Response::ok(
                3,
                &CursorOk {
                    col: 12,
                    row: 4,
                    visible: true
                }
            )),
            r#"{"id":3,"ok":{"col":12,"row":4,"visible":true}}"#
        );
        assert_eq!(
            to_json(&Response::ok(
                4,
                &SizeOk {
                    cols: 220,
                    rows: 50
                }
            )),
            r#"{"id":4,"ok":{"cols":220,"rows":50}}"#
        );
        assert_eq!(
            to_json(&Response::ok(5, &AlternateOk { on: true })),
            r#"{"id":5,"ok":{"on":true}}"#
        );
        assert_eq!(to_json(&Response::empty_ok(6)), r#"{"id":6,"ok":{}}"#);
        assert_eq!(
            to_json(&Response::err(9, "unknown key \"C-Fnord\"")),
            r#"{"id":9,"err":"unknown key \"C-Fnord\""}"#
        );
    }

    #[test]
    fn stream_frame_shape_is_frozen() {
        assert_eq!(
            to_json(&StreamFrame::bytes(b"\x1b[2J")),
            r#"{"stream":"bytes","data":"G1sySg=="}"#
        );
    }

    #[test]
    fn unknown_op_yields_error_with_id() {
        let err = parse_request(br#"{"id":33,"op":"warp_core"}"#).unwrap_err();
        assert_eq!(err.id, Some(33));
        assert!(err.message.contains("warp_core") || err.message.contains("unknown"));
    }

    #[test]
    fn unknown_fields_are_ignored() {
        // Additive-extension rule: peers must ignore fields they don't know.
        let req = parse_request(br#"{"id":4,"op":"size","future_hint":true}"#).unwrap();
        assert_eq!(
            req,
            Request {
                id: 4,
                op: Op::Size
            }
        );
    }

    #[test]
    fn garbage_yields_error_without_id() {
        assert_eq!(parse_request(b"not json").unwrap_err().id, None);
    }
}
