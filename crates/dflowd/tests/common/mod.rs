//! Shared helpers for dflowd integration tests.
//!
//! Every test runs a real `dflowd.exe` against an isolated `DFLOW_DATA_DIR` temp
//! dir, so tests never collide with each other or with a live user daemon
//! (deliverable 2).

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use dflow_proto::{decode_frame, AuthHello, ClientKind, Envelope, FrameKind, PROTOCOL_VERSION};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message as WsMessage;

/// Kills the daemon subprocess when the test ends.
pub struct DaemonGuard(pub Child);

impl Drop for DaemonGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

impl DaemonGuard {
    /// Kill the daemon now (simulating a crash) and wait for it to exit.
    pub fn kill_now(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// A unique temp directory for one test's `DFLOW_DATA_DIR`.
pub fn unique_data_dir(tag: &str) -> PathBuf {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let dir = std::env::temp_dir().join(format!("dflowd-{tag}-{nanos}"));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Start dflowd with `DFLOW_DATA_DIR` isolation plus any extra env; return the
/// guard and `(port, token)` from the published runtime file.
pub fn start_daemon(data_dir: &Path, extra_env: &[(&str, &str)]) -> (DaemonGuard, u16, String) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_dflowd"));
    cmd.env("DFLOW_DATA_DIR", data_dir).env("DFLOW_LOG", "warn");
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    // Capture the daemon's stdout+stderr to <data-dir>/daemon.log so live-probe tests
    // can read real hook payloads; readiness is still detected via runtime.json.
    if let Ok(log) = std::fs::File::create(data_dir.join("daemon.log")) {
        if let Ok(log2) = log.try_clone() {
            cmd.stdout(std::process::Stdio::from(log));
            cmd.stderr(std::process::Stdio::from(log2));
        }
    }
    let child = cmd.spawn().expect("spawn dflowd");
    let guard = DaemonGuard(child);

    let runtime_path = data_dir.join("runtime.json");
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Ok(text) = std::fs::read_to_string(&runtime_path) {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
                if let (Some(port), Some(token)) = (json["port"].as_u64(), json["token"].as_str())
                {
                    if port > 0 {
                        return (guard, port as u16, token.to_string());
                    }
                }
            }
        }
        assert!(Instant::now() < deadline, "daemon never published a runtime file");
        std::thread::sleep(Duration::from_millis(100));
    }
}

pub type Ws = tokio_tungstenite::WebSocketStream<
    tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
>;

/// Connect to the daemon WS and complete the auth handshake.
pub async fn connect_and_auth(port: u16, token: &str) -> Ws {
    let url = format!("ws://127.0.0.1:{port}/ws");
    let (mut ws, _) = tokio_tungstenite::connect_async(url).await.expect("connect");
    let hello = Envelope::message(
        "auth-1",
        "auth.hello",
        AuthHello {
            token: token.to_string(),
            client: ClientKind::Desktop,
            proto_versions: vec![PROTOCOL_VERSION],
        },
    );
    ws.send(WsMessage::Text(serde_json::to_string(&hello).unwrap().into())).await.unwrap();
    let welcome = next_envelope(&mut ws).await;
    assert_eq!(welcome.msg_type, "auth.welcome", "expected welcome, got {welcome:?}");
    ws
}

/// Connect and authenticate with an explicit token and client kind (Concertmaster
/// token handshakes, the `mcp` attribution marker).
pub async fn connect_with(port: u16, token: &str, client: ClientKind) -> Ws {
    let url = format!("ws://127.0.0.1:{port}/ws");
    let (mut ws, _) = tokio_tungstenite::connect_async(url).await.expect("connect");
    let hello = Envelope::message(
        "auth",
        "auth.hello",
        AuthHello { token: token.to_string(), client, proto_versions: vec![PROTOCOL_VERSION] },
    );
    ws.send(WsMessage::Text(serde_json::to_string(&hello).unwrap().into())).await.unwrap();
    let welcome = next_envelope(&mut ws).await;
    assert_eq!(welcome.msg_type, "auth.welcome", "expected welcome, got {welcome:?}");
    ws
}

/// Read messages until the next text (control) envelope arrives.
pub async fn next_envelope(ws: &mut Ws) -> Envelope {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let msg = tokio::time::timeout(remaining, ws.next())
            .await
            .expect("timed out waiting for a control message")
            .expect("socket closed")
            .expect("ws error");
        if let WsMessage::Text(t) = msg {
            return serde_json::from_str(t.as_str()).expect("parse envelope");
        }
    }
}

/// Send a request envelope and return the envelope answering *this* request id,
/// buffering binary output frames into `sink` and skipping unrelated envelopes
/// (server-initiated `event.*` messages carry no id and are passed over).
pub async fn request(ws: &mut Ws, env: &Envelope, sink: &mut Vec<u8>) -> Envelope {
    let want = env.id.clone();
    ws.send(WsMessage::Text(serde_json::to_string(env).unwrap().into())).await.unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let msg = tokio::time::timeout(remaining, ws.next())
            .await
            .expect("timed out waiting for response")
            .expect("socket closed")
            .expect("ws error");
        match msg {
            WsMessage::Text(t) => {
                let parsed: Envelope = serde_json::from_str(t.as_str()).unwrap();
                if parsed.id == want {
                    return parsed;
                }
            }
            WsMessage::Binary(b) => {
                if let Ok(frame) = decode_frame(&b) {
                    if frame.kind == FrameKind::Output {
                        sink.extend_from_slice(&frame.data);
                    }
                }
            }
            _ => {}
        }
    }
}

/// Collect live binary output frames for a while, looking for `needle`.
pub async fn collect_output_until(ws: &mut Ws, needle: &str, timeout: Duration) -> String {
    let mut buf: Vec<u8> = Vec::new();
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match tokio::time::timeout(remaining, ws.next()).await {
            Ok(Some(Ok(WsMessage::Binary(b)))) => {
                if let Ok(frame) = decode_frame(&b) {
                    if frame.kind == FrameKind::Output {
                        buf.extend_from_slice(&frame.data);
                        if String::from_utf8_lossy(&buf).contains(needle) {
                            break;
                        }
                    }
                }
            }
            Ok(Some(Ok(_))) => {}
            _ => break,
        }
    }
    String::from_utf8_lossy(&buf).to_string()
}

/// Collect text envelopes until one satisfies `pred` (used for event streams).
pub async fn collect_envelope_until(
    ws: &mut Ws,
    timeout: Duration,
    mut pred: impl FnMut(&Envelope) -> bool,
) -> Option<Envelope> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match tokio::time::timeout(remaining, ws.next()).await {
            Ok(Some(Ok(WsMessage::Text(t)))) => {
                if let Ok(parsed) = serde_json::from_str::<Envelope>(t.as_str()) {
                    if pred(&parsed) {
                        return Some(parsed);
                    }
                }
            }
            Ok(Some(Ok(_))) => {}
            _ => break,
        }
    }
    None
}

/// Create a scratch git repo with one commit on `main`; returns its path.
pub fn scratch_repo(base: &Path) -> PathBuf {
    let repo = base.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    run_git(&repo, &["init", "-b", "main"]);
    run_git(&repo, &["config", "user.name", "DapperFlow Test"]);
    run_git(&repo, &["config", "user.email", "test@dapperflow.local"]);
    std::fs::write(repo.join("README.md"), "scratch\n").unwrap();
    run_git(&repo, &["add", "-A"]);
    run_git(&repo, &["commit", "-m", "init"]);
    repo
}

fn run_git(cwd: &Path, args: &[&str]) {
    let out = Command::new("git").arg("-C").arg(cwd).args(args).output().expect("run git");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}
