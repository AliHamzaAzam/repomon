//! The JSON-RPC 2.0 wire protocol shared by the daemon and its clients.
//!
//! Messages are length-prefixed: a 4-byte little-endian `u32` length, then that many bytes
//! of JSON. Requests carry an `id`; server-pushed events are notifications (no `id`) with a
//! method of the form `event.<topic>`.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const JSONRPC_VERSION: &str = "2.0";
/// Reject absurd frames rather than allocating gigabytes.
pub const MAX_FRAME_BYTES: usize = 64 * 1024 * 1024;

/// A JSON-RPC request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub jsonrpc: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<u64>,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

impl Request {
    pub fn new(id: u64, method: impl Into<String>, params: Option<Value>) -> Self {
        Request {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(id),
            method: method.into(),
            params,
        }
    }
}

/// A JSON-RPC response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub jsonrpc: String,
    pub id: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

impl Response {
    pub fn ok(id: Option<u64>, result: Value) -> Self {
        Response {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id,
            result: Some(result),
            error: None,
        }
    }
    pub fn err(id: Option<u64>, error: RpcError) -> Self {
        Response {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id,
            result: None,
            error: Some(error),
        }
    }
}

/// A JSON-RPC error object.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl RpcError {
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        RpcError {
            code,
            message: message.into(),
            data: None,
        }
    }
    pub fn method_not_found(method: &str) -> Self {
        RpcError::new(-32601, format!("method not found: {method}"))
    }
    pub fn invalid_params(msg: impl Into<String>) -> Self {
        RpcError::new(-32602, msg)
    }
    pub fn internal(msg: impl Into<String>) -> Self {
        RpcError::new(-32000, msg)
    }
}

/// A server-pushed notification (no `id`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Notification {
    pub jsonrpc: String,
    pub method: String,
    pub params: Value,
}

impl Notification {
    pub fn new(method: impl Into<String>, params: Value) -> Self {
        Notification {
            jsonrpc: JSONRPC_VERSION.to_string(),
            method: method.into(),
            params,
        }
    }
}

/// Write one length-prefixed frame.
pub async fn write_frame<W: AsyncWrite + Unpin>(w: &mut W, payload: &[u8]) -> std::io::Result<()> {
    let len = (payload.len() as u32).to_le_bytes();
    w.write_all(&len).await?;
    w.write_all(payload).await?;
    w.flush().await?;
    Ok(())
}

/// Serialize `msg` and write it as one frame.
pub async fn write_message<W, T>(w: &mut W, msg: &T) -> std::io::Result<()>
where
    W: AsyncWrite + Unpin,
    T: Serialize,
{
    let bytes = serde_json::to_vec(msg)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    write_frame(w, &bytes).await
}

/// Read one length-prefixed frame. Returns `None` on clean EOF.
pub async fn read_frame<R: AsyncRead + Unpin>(r: &mut R) -> std::io::Result<Option<Vec<u8>>> {
    let mut len_buf = [0u8; 4];
    match r.read_exact(&mut len_buf).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let n = u32::from_le_bytes(len_buf) as usize;
    if n > MAX_FRAME_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("frame too large: {n} bytes"),
        ));
    }
    let mut buf = vec![0u8; n];
    r.read_exact(&mut buf).await?;
    Ok(Some(buf))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn frame_roundtrip() {
        let req = Request::new(1, "repo.list", None);
        let mut buf: Vec<u8> = Vec::new();
        write_message(&mut buf, &req).await.unwrap();

        let mut cursor = std::io::Cursor::new(buf);
        let frame = read_frame(&mut cursor).await.unwrap().unwrap();
        let back: Request = serde_json::from_slice(&frame).unwrap();
        assert_eq!(back.method, "repo.list");
        assert_eq!(back.id, Some(1));
        assert_eq!(back.jsonrpc, "2.0");
    }

    #[tokio::test]
    async fn clean_eof_is_none() {
        let mut cursor = std::io::Cursor::new(Vec::<u8>::new());
        assert!(read_frame(&mut cursor).await.unwrap().is_none());
    }
}
