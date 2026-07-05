//! Binary PTY frames (`protocol.md` / Transport).
//!
//! Layout: `[u8 kind][16-byte session id][bytes...]`. The session id is the
//! 16-byte big-endian form of the session ULID. Keeping PTY I/O in binary frames
//! keeps the hot path off the JSON serializer.

use serde::{Deserialize, Serialize};

/// Length of the session-id header segment (a ULID is 128 bits = 16 bytes).
pub const SESSION_ID_LEN: usize = 16;

/// Frame kind discriminant (first byte on the wire).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum FrameKind {
    /// Daemon -> client: PTY output bytes.
    Output = 0,
    /// Client -> daemon: keystrokes / bytes to write to the PTY.
    Input = 1,
    /// Client -> daemon: a resize request; the body is `[cols: u16 BE][rows: u16 BE]`.
    Resize = 2,
}

impl FrameKind {
    fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(FrameKind::Output),
            1 => Some(FrameKind::Input),
            2 => Some(FrameKind::Resize),
            _ => None,
        }
    }
}

/// A decoded binary frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub kind: FrameKind,
    pub session_id: [u8; SESSION_ID_LEN],
    pub data: Vec<u8>,
}

impl Frame {
    /// Interpret a resize frame body as `(cols, rows)`.
    pub fn as_resize(&self) -> Option<(u16, u16)> {
        if self.kind != FrameKind::Resize || self.data.len() < 4 {
            return None;
        }
        let cols = u16::from_be_bytes([self.data[0], self.data[1]]);
        let rows = u16::from_be_bytes([self.data[2], self.data[3]]);
        Some((cols, rows))
    }

    /// Build the 4-byte body for a resize frame.
    pub fn resize_body(cols: u16, rows: u16) -> [u8; 4] {
        let mut body = [0u8; 4];
        body[0..2].copy_from_slice(&cols.to_be_bytes());
        body[2..4].copy_from_slice(&rows.to_be_bytes());
        body
    }
}

/// Errors decoding a binary frame.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FrameError {
    #[error("frame too short: {0} bytes, need at least {min}", min = 1 + SESSION_ID_LEN)]
    TooShort(usize),
    #[error("unknown frame kind byte: {0}")]
    UnknownKind(u8),
}

/// Encode a frame to bytes: `[kind][16-byte session id][data]`.
pub fn encode_frame(kind: FrameKind, session_id: &[u8; SESSION_ID_LEN], data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(1 + SESSION_ID_LEN + data.len());
    out.push(kind as u8);
    out.extend_from_slice(session_id);
    out.extend_from_slice(data);
    out
}

/// Decode a binary frame from bytes.
pub fn decode_frame(bytes: &[u8]) -> Result<Frame, FrameError> {
    if bytes.len() < 1 + SESSION_ID_LEN {
        return Err(FrameError::TooShort(bytes.len()));
    }
    let kind = FrameKind::from_u8(bytes[0]).ok_or(FrameError::UnknownKind(bytes[0]))?;
    let mut session_id = [0u8; SESSION_ID_LEN];
    session_id.copy_from_slice(&bytes[1..1 + SESSION_ID_LEN]);
    let data = bytes[1 + SESSION_ID_LEN..].to_vec();
    Ok(Frame { kind, session_id, data })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_frame_round_trips() {
        let sid = [7u8; SESSION_ID_LEN];
        let encoded = encode_frame(FrameKind::Output, &sid, b"hello");
        assert_eq!(encoded[0], 0);
        let frame = decode_frame(&encoded).unwrap();
        assert_eq!(frame.kind, FrameKind::Output);
        assert_eq!(frame.session_id, sid);
        assert_eq!(frame.data, b"hello");
    }

    #[test]
    fn resize_frame_carries_dimensions() {
        let sid = [1u8; SESSION_ID_LEN];
        let body = Frame::resize_body(120, 40);
        let encoded = encode_frame(FrameKind::Resize, &sid, &body);
        let frame = decode_frame(&encoded).unwrap();
        assert_eq!(frame.as_resize(), Some((120, 40)));
    }

    #[test]
    fn rejects_short_and_unknown() {
        assert!(matches!(decode_frame(&[0, 1, 2]), Err(FrameError::TooShort(3))));
        let mut buf = vec![9u8];
        buf.extend_from_slice(&[0u8; SESSION_ID_LEN]);
        assert!(matches!(decode_frame(&buf), Err(FrameError::UnknownKind(9))));
    }
}
