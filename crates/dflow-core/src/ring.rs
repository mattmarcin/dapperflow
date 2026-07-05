//! An in-memory scrollback ring: a byte buffer capped at a fixed capacity that
//! evicts the oldest bytes when full.
//!
//! The ring holds raw PTY output in memory. Any capture that *persists* or *leaves the
//! session* must be scrubbed of known vault-secret values first (`security.md` / Secret
//! handling policy): [`ScrollbackRing::snapshot_scrubbed`] is the scrubbed twin of
//! [`ScrollbackRing::snapshot`], so a durable ring write or an off-session capture uses
//! the redacted bytes while the live in-memory ring (what an attached human sees) is
//! left intact. Disk persistence of the ring to
//! `<app-data>/scrollback/<session-ulid>.ring` itself is a later phase; when it lands it
//! writes `snapshot_scrubbed`, not `snapshot`.

use std::collections::VecDeque;

/// A fixed-capacity byte ring buffer holding the most recent PTY output.
#[derive(Debug)]
pub struct ScrollbackRing {
    buf: VecDeque<u8>,
    capacity: usize,
}

impl ScrollbackRing {
    /// Create a ring holding at most `capacity` bytes.
    pub fn new(capacity: usize) -> Self {
        Self { buf: VecDeque::with_capacity(capacity.min(64 * 1024)), capacity: capacity.max(1) }
    }

    /// Append bytes, evicting the oldest bytes to stay within capacity.
    pub fn push(&mut self, bytes: &[u8]) {
        if bytes.len() >= self.capacity {
            // The new chunk alone exceeds capacity: keep only its tail.
            self.buf.clear();
            self.buf.extend(&bytes[bytes.len() - self.capacity..]);
            return;
        }
        let overflow = (self.buf.len() + bytes.len()).saturating_sub(self.capacity);
        if overflow > 0 {
            self.buf.drain(0..overflow);
        }
        self.buf.extend(bytes);
    }

    /// A copy of the current contents, oldest byte first.
    pub fn snapshot(&self) -> Vec<u8> {
        self.buf.iter().copied().collect()
    }

    /// A copy of the current contents with every known secret value redacted
    /// (`security.md`: scrub known secret values before scrollback persists to disk or
    /// leaves the session). With no secrets this equals [`ScrollbackRing::snapshot`].
    pub fn snapshot_scrubbed(&self, secrets: &[String]) -> Vec<u8> {
        let raw = self.snapshot();
        if secrets.is_empty() {
            return raw;
        }
        crate::secret::scrub_bytes(&raw, secrets)
    }

    /// Current number of buffered bytes.
    pub fn len(&self) -> usize {
        self.buf.len()
    }

    /// Whether the ring is empty.
    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    /// The configured maximum capacity in bytes.
    pub fn capacity(&self) -> usize {
        self.capacity
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appends_below_capacity() {
        let mut ring = ScrollbackRing::new(16);
        ring.push(b"hello");
        ring.push(b" world");
        assert_eq!(ring.snapshot(), b"hello world");
        assert_eq!(ring.len(), 11);
    }

    #[test]
    fn evicts_oldest_when_full() {
        let mut ring = ScrollbackRing::new(8);
        ring.push(b"abcdef");
        ring.push(b"ghij"); // total 10, cap 8 -> drop "ab"
        assert_eq!(ring.snapshot(), b"cdefghij");
        assert_eq!(ring.len(), 8);
    }

    #[test]
    fn oversized_chunk_keeps_tail() {
        let mut ring = ScrollbackRing::new(4);
        ring.push(b"0123456789");
        assert_eq!(ring.snapshot(), b"6789");
        assert_eq!(ring.len(), 4);
    }

    #[test]
    fn preserves_byte_order_across_many_pushes() {
        let mut ring = ScrollbackRing::new(5);
        for b in b"abcdefgh" {
            ring.push(&[*b]);
        }
        assert_eq!(ring.snapshot(), b"defgh");
    }

    #[test]
    fn scrubbed_snapshot_redacts_known_secrets() {
        let mut ring = ScrollbackRing::new(256);
        // A realistic mixed line: ANSI-ish bytes around a materialized secret value.
        ring.push(b"\x1b[32mexport TOKEN=sk-live-supersecret\x1b[0m\r\n");
        let secrets = vec!["sk-live-supersecret".to_string()];
        let scrubbed = ring.snapshot_scrubbed(&secrets);
        let text = String::from_utf8_lossy(&scrubbed);
        assert!(!text.contains("sk-live-supersecret"), "the ring scrub must redact the value: {text}");
        assert!(text.contains("[dflow:redacted]"));
        // The live (un-scrubbed) ring is untouched for the attached human.
        assert!(String::from_utf8_lossy(&ring.snapshot()).contains("sk-live-supersecret"));
    }
}
