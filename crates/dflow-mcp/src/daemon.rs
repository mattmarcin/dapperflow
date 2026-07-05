//! Blocking WebSocket client to `dflowd`, mirroring `dflow-cli/src/client.rs`.
//!
//! One tool call = one connection: connect, authenticate as an `mcp`-kind client
//! with the root token, send a few requests, drop the socket. Loopback latency
//! makes this trivially cheap and it removes every reconnect concern from the
//! server (`protocol.md` / Transport, Authentication).
//!
//! The `mcp` client kind (`phase6-mcp.md` merge-time request 1) is an attribution
//! marker, not a scope: the server still grants the root surface, but knows to emit
//! `concertmaster_steered` when this connection calls `session.send_verified`. The
//! MCP server holds the root token but exposes only the Concertmaster capability
//! subset as tools (`security.md` / Concertmaster capability scope); see `server.rs`
//! for the enforced exclusions. The daemon-minted Concertmaster-scoped token
//! (`auth.mint_concertmaster`) exists as the defense-in-depth upgrade, but is not
//! adopted here: this client re-authenticates with the root token from `runtime.json`
//! on every tool call, so swapping to a minted token is not the trivial change this
//! milestone scopes.

use std::net::TcpStream;
use std::time::Duration;

use dflow_proto::{
    AuthHello, ClientKind, Envelope, ErrorCode, ProtocolError, CLOSE_AUTH_FAILED,
    PROTOCOL_VERSION,
};
use serde::de::DeserializeOwned;
use serde::Serialize;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{connect, Message, WebSocket};

use crate::runtime;

/// How long a single daemon response may take before the tool call fails.
const READ_TIMEOUT: Duration = Duration::from_secs(30);

/// Everything that can go wrong talking to the daemon, phrased for the caller
/// (an LLM tool result) rather than for a stack trace.
#[derive(Debug, thiserror::Error)]
pub enum DaemonError {
    /// The daemon is not running or its runtime file is unusable.
    #[error("{0}")]
    Setup(String),
    /// The socket failed mid-conversation.
    #[error("{0}")]
    Transport(String),
    /// The daemon rejected the token (rotate/re-pair, not retry).
    #[error("the daemon rejected the token: {0}")]
    Auth(String),
    /// A structured daemon error (`protocol.md` / Errors).
    #[error("daemon error [{}]: {message}", code_str(*code))]
    Daemon { code: ErrorCode, message: String },
    /// The response payload did not decode into the expected shape.
    #[error("unexpected response shape: {0}")]
    Decode(String),
}

impl DaemonError {
    /// Whether this is the daemon saying "verb not routed in this phase".
    pub fn is_unsupported(&self) -> bool {
        matches!(self, DaemonError::Daemon { code: ErrorCode::Unsupported, .. })
    }
}

/// The wire spelling (snake_case) of a protocol error code.
fn code_str(code: ErrorCode) -> String {
    serde_json::to_string(&code)
        .map(|s| s.trim_matches('"').to_string())
        .unwrap_or_else(|_| format!("{code:?}"))
}

/// A connected, authenticated root-scope client over one WebSocket.
pub struct Daemon {
    ws: WebSocket<MaybeTlsStream<TcpStream>>,
    next_id: u64,
}

impl Daemon {
    /// Discover the daemon via the runtime file and connect.
    pub fn connect() -> Result<Daemon, DaemonError> {
        let ep = runtime::discover()?;
        Daemon::connect_to(&ep.endpoint, &ep.token)
    }

    /// Connect to an explicit endpoint (tests pass a booted temp daemon).
    pub fn connect_to(endpoint: &str, token: &str) -> Result<Daemon, DaemonError> {
        let (mut ws, _resp) = connect(endpoint).map_err(|e| {
            DaemonError::Setup(format!(
                "cannot reach the DapperFlow daemon at {endpoint}: {e}; is dflowd running?"
            ))
        })?;
        if let MaybeTlsStream::Plain(stream) = ws.get_ref() {
            let _ = stream.set_read_timeout(Some(READ_TIMEOUT));
        }

        let hello = Envelope::message(
            "auth",
            "auth.hello",
            AuthHello {
                token: token.to_string(),
                // The `mcp` kind lets the daemon attribute this connection's steers as
                // concertmaster_steered (`phase6-mcp.md` merge-time request 1). Scope is
                // still the root token's; the marker only changes attribution.
                client: ClientKind::Mcp,
                proto_versions: vec![PROTOCOL_VERSION],
            },
        );
        send(&mut ws, &hello)?;

        loop {
            let msg = ws.read().map_err(classify_handshake)?;
            match msg {
                Message::Text(t) => {
                    let env: Envelope = serde_json::from_str(t.as_str())
                        .map_err(|e| DaemonError::Decode(format!("malformed welcome: {e}")))?;
                    if env.msg_type == "auth.welcome" {
                        return Ok(Daemon { ws, next_id: 1 });
                    }
                    if env.msg_type == "error" {
                        let err: ProtocolError = env
                            .decode_payload()
                            .unwrap_or_else(|_| ProtocolError::auth("authentication failed"));
                        return Err(DaemonError::Auth(err.message));
                    }
                }
                Message::Close(frame) => {
                    let code = frame.as_ref().map(|c| u16::from(c.code)).unwrap_or(0);
                    if code == CLOSE_AUTH_FAILED {
                        return Err(DaemonError::Auth(
                            "the daemon rejected the root token (close 4001); \
                             the token may have been rotated"
                                .into(),
                        ));
                    }
                    return Err(DaemonError::Transport(format!(
                        "the daemon closed the connection during the handshake (close code {code})"
                    )));
                }
                Message::Ping(p) => {
                    let _ = ws.send(Message::Pong(p));
                }
                _ => {}
            }
        }
    }

    /// Send one request and decode its response payload.
    pub fn request<P: Serialize, T: DeserializeOwned>(
        &mut self,
        msg_type: &str,
        payload: P,
    ) -> Result<T, DaemonError> {
        let raw = self.request_value(msg_type, payload)?;
        serde_json::from_value(raw)
            .map_err(|e| DaemonError::Decode(format!("{msg_type} response: {e}")))
    }

    /// Send one request and return the raw response payload.
    ///
    /// Binary frames (PTY output that starts flowing after `session.attach`)
    /// and stray events are skipped: this client correlates strictly by id.
    pub fn request_value<P: Serialize>(
        &mut self,
        msg_type: &str,
        payload: P,
    ) -> Result<serde_json::Value, DaemonError> {
        let id = format!("m{}", self.next_id);
        self.next_id += 1;
        let env = Envelope::message(id.clone(), msg_type, payload);
        send(&mut self.ws, &env)?;

        loop {
            let msg = self.ws.read().map_err(|e| {
                DaemonError::Transport(format!("reading the {msg_type} response: {e}"))
            })?;
            match msg {
                Message::Text(t) => {
                    let env: Envelope = serde_json::from_str(t.as_str())
                        .map_err(|e| DaemonError::Decode(format!("malformed response: {e}")))?;
                    if env.id.as_deref() != Some(id.as_str()) {
                        continue; // an event or a stale frame; not our response
                    }
                    if env.msg_type == "error" {
                        let err: ProtocolError = env.decode_payload().map_err(|e| {
                            DaemonError::Decode(format!("malformed error payload: {e}"))
                        })?;
                        return Err(DaemonError::Daemon { code: err.code, message: err.message });
                    }
                    return Ok(env.payload);
                }
                Message::Binary(_) => continue, // PTY output frames; not ours to render
                Message::Close(_) => {
                    return Err(DaemonError::Transport(
                        "the daemon closed the connection mid-request".into(),
                    ))
                }
                Message::Ping(p) => {
                    let _ = self.ws.send(Message::Pong(p));
                }
                _ => {}
            }
        }
    }
}

/// Serialize and send an envelope as a text frame.
fn send(
    ws: &mut WebSocket<MaybeTlsStream<TcpStream>>,
    env: &Envelope,
) -> Result<(), DaemonError> {
    let text = serde_json::to_string(env)
        .map_err(|e| DaemonError::Decode(format!("encoding request: {e}")))?;
    ws.send(Message::text(text))
        .map_err(|e| DaemonError::Transport(format!("sending request: {e}")))
}

/// Map a read error during the handshake (a close mid-handshake reads as an error).
fn classify_handshake(err: tungstenite::Error) -> DaemonError {
    match err {
        tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed => {
            DaemonError::Auth("the daemon closed the connection during authentication".into())
        }
        other => DaemonError::Transport(format!("handshake failed: {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daemon_error_renders_wire_code() {
        let err = DaemonError::Daemon {
            code: ErrorCode::Unsupported,
            message: "no such verb".into(),
        };
        assert_eq!(err.to_string(), "daemon error [unsupported]: no such verb");
        assert!(err.is_unsupported());
    }

    #[test]
    fn unreachable_daemon_is_a_setup_error() {
        // A port from the dynamic range with nothing listening.
        let err = match Daemon::connect_to("ws://127.0.0.1:1/ws", "tok") {
            Err(e) => e,
            Ok(_) => panic!("connect to a dead port should fail"),
        };
        assert!(matches!(err, DaemonError::Setup(_)), "got {err:?}");
        assert!(err.to_string().contains("is dflowd running"));
    }
}
