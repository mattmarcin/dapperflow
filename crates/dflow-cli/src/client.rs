//! The blocking `dflow-proto` WebSocket client (`protocol.md` / Transport).
//!
//! One invocation = one connection: authenticate with the per-task token as an
//! `agent` client, send one request, read its response, done. No event subscription,
//! no PTY frames.

use std::net::TcpStream;

use dflow_proto::{
    AuthHello, ClientKind, Envelope, ProtocolError, CLOSE_AUTH_FAILED, PROTOCOL_VERSION,
};
use serde::Serialize;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{connect, Message, WebSocket};

use crate::error::CliError;

/// A connected, authenticated agent-CLI session over one WebSocket.
pub struct Client {
    ws: WebSocket<MaybeTlsStream<TcpStream>>,
    next_id: u64,
}

impl Client {
    /// Connect to `endpoint` and complete the auth handshake with the task token.
    pub fn connect(endpoint: &str, token: &str) -> Result<Client, CliError> {
        let (mut ws, _resp) = connect(endpoint).map_err(|e| {
            CliError::operational(
                format!("cannot reach the DapperFlow daemon at {endpoint}: {e}"),
                "the daemon may have stopped; the session is no longer live",
            )
        })?;

        let hello = Envelope::message(
            "auth",
            "auth.hello",
            AuthHello {
                token: token.to_string(),
                client: ClientKind::Agent,
                proto_versions: vec![PROTOCOL_VERSION],
            },
        );
        send(&mut ws, &hello)?;

        loop {
            let msg = ws.read().map_err(classify_close)?;
            match msg {
                Message::Text(t) => {
                    let env: Envelope = serde_json::from_str(t.as_str()).map_err(|e| {
                        CliError::operational(format!("malformed welcome: {e}"), "retry")
                    })?;
                    if env.msg_type == "auth.welcome" {
                        return Ok(Client { ws, next_id: 1 });
                    }
                    if env.msg_type == "error" {
                        let err: ProtocolError = env
                            .decode_payload()
                            .unwrap_or_else(|_| ProtocolError::auth("authentication failed"));
                        return Err(CliError::revoked(err.message));
                    }
                }
                Message::Close(frame) => {
                    let code = frame.as_ref().map(|c| u16::from(c.code)).unwrap_or(0);
                    if code == CLOSE_AUTH_FAILED {
                        return Err(CliError::revoked("the daemon rejected this task token"));
                    }
                    return Err(CliError::operational(
                        format!("the daemon closed the connection (close code {code})"),
                        "the session may have ended",
                    ));
                }
                Message::Ping(p) => {
                    let _ = ws.send(Message::Pong(p));
                }
                _ => {}
            }
        }
    }

    /// Send one request and return its response payload, mapping a daemon error
    /// envelope to the right CLI exit code.
    pub fn request<P: Serialize>(
        &mut self,
        msg_type: &str,
        payload: P,
    ) -> Result<serde_json::Value, CliError> {
        let id = format!("r{}", self.next_id);
        self.next_id += 1;
        let env = Envelope::message(id.clone(), msg_type, payload);
        send(&mut self.ws, &env)?;

        loop {
            let msg = self.ws.read().map_err(|e| {
                CliError::operational(format!("reading response: {e}"), "the session may have ended")
            })?;
            match msg {
                Message::Text(t) => {
                    let env: Envelope = serde_json::from_str(t.as_str()).map_err(|e| {
                        CliError::operational(format!("malformed response: {e}"), "retry")
                    })?;
                    if env.id.as_deref() != Some(id.as_str()) {
                        continue; // not our response (defensive; agents get no events)
                    }
                    if env.msg_type == "error" {
                        let err: ProtocolError = env.decode_payload().map_err(|e| {
                            CliError::operational(format!("malformed error payload: {e}"), "retry")
                        })?;
                        return Err(CliError::from_daemon(err.code, err.message));
                    }
                    return Ok(env.payload);
                }
                Message::Close(_) => {
                    return Err(CliError::operational(
                        "the daemon closed the connection",
                        "the session may have ended",
                    ))
                }
                Message::Ping(p) => {
                    let _ = self.ws.send(Message::Pong(p));
                }
                _ => {}
            }
        }
    }

    /// Decode a response payload into a typed message.
    pub fn decode<T: for<'de> serde::Deserialize<'de>>(value: serde_json::Value) -> Result<T, CliError> {
        serde_json::from_value(value)
            .map_err(|e| CliError::operational(format!("unexpected response shape: {e}"), "retry"))
    }
}

/// Serialize and send an envelope as a text frame.
fn send(ws: &mut WebSocket<MaybeTlsStream<TcpStream>>, env: &Envelope) -> Result<(), CliError> {
    let text = serde_json::to_string(env)
        .map_err(|e| CliError::operational(format!("encoding request: {e}"), "retry"))?;
    ws.send(Message::text(text))
        .map_err(|e| CliError::operational(format!("sending request: {e}"), "the session may have ended"))
}

/// Map a read error during the handshake (a close mid-handshake reads as an error).
fn classify_close(err: tungstenite::Error) -> CliError {
    match err {
        tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed => {
            CliError::revoked("the daemon closed the connection during authentication")
        }
        other => CliError::operational(format!("handshake failed: {other}"), "retry"),
    }
}
