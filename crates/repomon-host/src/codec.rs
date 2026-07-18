//! Length-prefixed JSON framing (PROTOCOL.md §4): `[u32 LE length][length bytes of JSON]`.
//!
//! Pure logic, tested on every OS. The decoder is incremental: feed it arbitrary byte
//! chunks (pipe reads split frames wherever they like) and pull complete frames out.

/// Maximum JSON payload size (PROTOCOL.md §4): a peer seeing a larger length treats the
/// connection as corrupt.
pub const MAX_FRAME: usize = 16 * 1024 * 1024;

/// Wrap a JSON payload in the wire framing: 4-byte little-endian length + payload.
pub fn encode_frame(json: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + json.len());
    out.extend_from_slice(
        &u32::try_from(json.len())
            .expect("frame length fits u32")
            .to_le_bytes(),
    );
    out.extend_from_slice(json);
    out
}

/// Incremental frame decoder: `extend` with whatever the pipe read returned, then drain
/// complete frames with `next_frame` (`Ok(None)` = need more bytes).
#[derive(Default)]
pub struct FrameDecoder {
    buf: Vec<u8>,
}

impl FrameDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn extend(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// Pop the next complete frame's payload, if the buffer holds one.
    pub fn next_frame(&mut self) -> Result<Option<Vec<u8>>, FrameTooLarge> {
        if self.buf.len() < 4 {
            return Ok(None);
        }
        let len = u32::from_le_bytes(self.buf[..4].try_into().expect("4 bytes")) as usize;
        if len > MAX_FRAME {
            return Err(FrameTooLarge(len));
        }
        if self.buf.len() < 4 + len {
            return Ok(None);
        }
        let payload = self.buf[4..4 + len].to_vec();
        self.buf.drain(..4 + len);
        Ok(Some(payload))
    }
}

/// A peer announced a frame larger than [`MAX_FRAME`] — the connection is corrupt.
#[derive(Debug, PartialEq, Eq)]
pub struct FrameTooLarge(pub usize);

impl std::fmt::Display for FrameTooLarge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "frame of {} bytes exceeds the {MAX_FRAME}-byte limit",
            self.0
        )
    }
}

impl std::error::Error for FrameTooLarge {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_prefixes_little_endian_length() {
        let frame = encode_frame(br#"{"id":1}"#);
        assert_eq!(&frame[..4], &8u32.to_le_bytes());
        assert_eq!(&frame[4..], br#"{"id":1}"#);
    }

    #[test]
    fn decoder_round_trips_a_frame() {
        let mut dec = FrameDecoder::new();
        dec.extend(&encode_frame(br#"{"id":7,"op":"hello"}"#));
        assert_eq!(
            dec.next_frame().unwrap().as_deref(),
            Some(br#"{"id":7,"op":"hello"}"#.as_slice())
        );
        assert_eq!(dec.next_frame().unwrap(), None);
    }

    #[test]
    fn decoder_handles_split_reads() {
        let frame = encode_frame(b"{\"id\":2}");
        let mut dec = FrameDecoder::new();
        // One byte at a time: no frame until the last byte arrives.
        for (i, b) in frame.iter().enumerate() {
            dec.extend(&[*b]);
            if i < frame.len() - 1 {
                assert_eq!(
                    dec.next_frame().unwrap(),
                    None,
                    "premature frame at byte {i}"
                );
            }
        }
        assert_eq!(
            dec.next_frame().unwrap().as_deref(),
            Some(b"{\"id\":2}".as_slice())
        );
    }

    #[test]
    fn decoder_yields_back_to_back_frames() {
        let mut bytes = encode_frame(b"{\"id\":1}");
        bytes.extend_from_slice(&encode_frame(b"{\"id\":2}"));
        let mut dec = FrameDecoder::new();
        dec.extend(&bytes);
        assert_eq!(
            dec.next_frame().unwrap().as_deref(),
            Some(b"{\"id\":1}".as_slice())
        );
        assert_eq!(
            dec.next_frame().unwrap().as_deref(),
            Some(b"{\"id\":2}".as_slice())
        );
        assert_eq!(dec.next_frame().unwrap(), None);
    }

    #[test]
    fn decoder_rejects_oversized_frames() {
        let mut dec = FrameDecoder::new();
        dec.extend(&((MAX_FRAME as u32) + 1).to_le_bytes());
        assert!(dec.next_frame().is_err());
    }

    #[test]
    fn max_frame_is_16_mib() {
        assert_eq!(MAX_FRAME, 16 * 1024 * 1024);
    }
}
