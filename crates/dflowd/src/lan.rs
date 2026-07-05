//! The opt-in LAN listener (`security.md` / Remote access trust model; M6).
//!
//! A SECOND listener, separate from the loopback WS server and **off by default**. When
//! enabled it binds `0.0.0.0:<port>` and serves exactly three surfaces:
//!
//! - the built mobile PWA (`apps/mobile/dist`) at `/m` (the phone attention surface,
//!   `mobile.md`; `phase7-pwa.md` merge seam: "mount the built dist on the LAN listener
//!   at /m"),
//! - the same authenticated WS protocol at `/ws`, but marked LAN-origin so the auth
//!   handshake accepts **only** phone-scoped capability tokens (the root/task/round
//!   tokens never work over the LAN, a hard boundary beyond the capability gate), and
//! - the signed artifact routes (`/artifact/doc`, `/artifact/asset`) so plan review
//!   renders on the phone with the same short-lived signed URLs (cross-origin safe:
//!   the signature is over `doc_id.exp`, origin-independent).
//!
//! It deliberately does NOT expose `/shutdown` or `/hooks` (loopback-only, sensitive).
//!
//! LAN v1 ships without TLS, stated honestly (`security.md`): the capability token is
//! the gate, the listener is opt-in, and the threat model assumes a trusted network.
//! `daemon.lan.status` carries the caveat text so the UI can show it on enable.

use std::net::{SocketAddr, UdpSocket};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use axum::body::Body;
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{Path as AxPath, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use tokio::net::TcpListener;
use tokio::sync::Notify;
use tokio::task::JoinHandle;

use crate::artifact::{artifact_asset_handler, artifact_doc_handler};
use crate::conn::handle_socket;
use crate::server::AppState;

/// The default LAN port when the caller names none and none is persisted. A memorable,
/// unprivileged, rarely-occupied port distinct from the loopback (which is OS-assigned).
pub const DEFAULT_LAN_PORT: u16 = 8790;

/// The honest no-TLS-on-LAN posture (`security.md` / Remote access trust model), shown
/// whenever the listener is enabled so the human sees exactly what they are opting into.
pub const LAN_CAVEAT: &str = "LAN access ships without TLS: traffic is plain HTTP/WS on \
your local network. The phone-scoped capability token is the only gate, the listener is \
opt-in, and this assumes a trusted home/office network. Do not enable it on an untrusted \
network. True off-LAN access requires the TLS listener and device keys (a later milestone).";

/// A running LAN listener: its bound port, a shutdown signal, and the serve task.
struct LanHandle {
    port: u16,
    shutdown: Arc<Notify>,
    task: JoinHandle<()>,
}

/// The daemon-owned LAN listener slot. `None` inside means not bound.
#[derive(Default)]
pub struct LanListener {
    inner: Mutex<Option<LanHandle>>,
}

impl LanListener {
    /// The currently bound port, or `None` when not listening.
    pub fn bound_port(&self) -> Option<u16> {
        self.inner.lock().expect("lan listener poisoned").as_ref().map(|h| h.port)
    }

    /// Whether a LAN listener is currently bound.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn is_bound(&self) -> bool {
        self.bound_port().is_some()
    }

    /// Bind (or rebind) the LAN listener on `port` and start serving. Replaces any
    /// existing listener (a port change stops the old one first). Returns the bound port.
    pub async fn start(&self, state: AppState, port: u16) -> Result<u16> {
        // Bind before tearing down the old one, so a bind failure leaves the current
        // listener intact rather than knocking the phone offline for a bad port.
        let addr: SocketAddr = ([0, 0, 0, 0], port).into();
        let listener = TcpListener::bind(addr)
            .await
            .with_context(|| format!("binding LAN listener on 0.0.0.0:{port}"))?;
        let bound = listener.local_addr()?.port();

        self.stop(); // stop any prior listener now that the new bind succeeded

        let shutdown = Arc::new(Notify::new());
        let router = lan_router(state);
        let shutdown_for_task = Arc::clone(&shutdown);
        let task = tokio::spawn(async move {
            let signal = async move { shutdown_for_task.notified().await };
            if let Err(err) = axum::serve(listener, router).with_graceful_shutdown(signal).await {
                tracing::warn!(%err, "LAN listener serve ended with error");
            }
        });
        tracing::info!(port = bound, "LAN listener bound on 0.0.0.0");
        *self.inner.lock().expect("lan listener poisoned") = Some(LanHandle { port: bound, shutdown, task });
        Ok(bound)
    }

    /// Stop the LAN listener if running (graceful, then abort). Idempotent.
    pub fn stop(&self) {
        if let Some(handle) = self.inner.lock().expect("lan listener poisoned").take() {
            handle.shutdown.notify_waiters();
            handle.task.abort();
            tracing::info!(port = handle.port, "LAN listener stopped");
        }
    }
}

/// Build the LAN router: the PWA at `/m`, the LAN-origin WS at `/ws`, the signed artifact
/// routes, and `/health`. No `/shutdown`, no `/hooks` (loopback-only surfaces).
fn lan_router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/ws", get(lan_ws_handler))
        .route("/m", get(serve_pwa_index))
        .route("/m/", get(serve_pwa_index))
        .route("/m/{*path}", get(serve_pwa_path))
        .route("/artifact/doc/{doc_id}", get(artifact_doc_handler))
        .route("/artifact/asset/{name}", get(artifact_asset_handler))
        .with_state(state)
}

/// The LAN `/ws` upgrade: identical to loopback, but `lan_origin = true` so the auth
/// handshake accepts only phone-scoped tokens (`conn::authenticate`).
async fn lan_ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    ws.on_upgrade(move |socket| handle_socket(socket, state, true)).into_response()
}

// ---- PWA static serving ---------------------------------------------------

/// `GET /m` (and `/m/`): serve the PWA `index.html`.
async fn serve_pwa_index(State(_state): State<AppState>) -> Response {
    serve_pwa_file("index.html")
}

/// `GET /m/<path>`: serve a PWA asset by relative path (traversal-guarded).
async fn serve_pwa_path(AxPath(path): AxPath<String>, State(_state): State<AppState>) -> Response {
    // Reject traversal and absolute components; the PWA bundle is flat + `assets/`.
    if path.split(['/', '\\']).any(|seg| seg == ".." || seg.is_empty()) {
        return (StatusCode::BAD_REQUEST, "bad path").into_response();
    }
    serve_pwa_file(&path)
}

/// Serve `rel` from the resolved PWA dist dir, or an honest placeholder when the bundle
/// is not packaged (so `/m` never dead-ends during development).
fn serve_pwa_file(rel: &str) -> Response {
    let dist = match pwa_dist_dir() {
        Some(d) => d,
        None => return pwa_placeholder(),
    };
    let file = dist.join(rel);
    // Belt-and-suspenders: the resolved path must stay within dist.
    if !file.starts_with(&dist) {
        return (StatusCode::BAD_REQUEST, "bad path").into_response();
    }
    match std::fs::read(&file) {
        Ok(bytes) => {
            let ct = content_type_for(rel);
            (StatusCode::OK, [(header::CONTENT_TYPE, ct), (header::CACHE_CONTROL, "no-cache")], Body::from(bytes))
                .into_response()
        }
        // A missing asset for a single-page app: fall back to index so client routing
        // still lands (but never for a real asset request like `.js`/`.css`).
        Err(_) if !rel.contains('.') && rel != "index.html" => serve_pwa_file("index.html"),
        Err(_) => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

/// Resolve the built PWA dist directory (`apps/mobile/dist`).
///
/// Order: `DFLOW_PWA_DIST` (tests + packaging override) wins; else a `pwa` dir beside
/// the daemon binary (the packaged layout); else `apps/mobile/dist` walked up from the
/// binary (the dev workspace). `None` means "not packaged", and `/m` serves a
/// placeholder instead of dead-ending.
pub fn pwa_dist_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("DFLOW_PWA_DIST") {
        let p = PathBuf::from(dir);
        if p.join("index.html").is_file() {
            return Some(p);
        }
    }
    let exe = std::env::current_exe().ok()?;
    let bin_dir = exe.parent()?;
    let beside = bin_dir.join("pwa");
    if beside.join("index.html").is_file() {
        return Some(beside);
    }
    // Dev: walk up from target/<profile>/dflowd.exe to the workspace root.
    let mut cur = bin_dir;
    for _ in 0..6 {
        let candidate = cur.join("apps").join("mobile").join("dist");
        if candidate.join("index.html").is_file() {
            return Some(candidate);
        }
        cur = cur.parent()?;
    }
    None
}

/// A minimal honest placeholder when the PWA bundle is not packaged with the daemon.
fn pwa_placeholder() -> Response {
    let body = "<!doctype html><meta charset=utf-8><title>DapperFlow</title>\
        <body style=\"font-family:system-ui;padding:2rem;max-width:40rem;margin:auto\">\
        <h1>DapperFlow LAN</h1><p>The mobile web client is not packaged with this daemon \
        build. Build it with <code>pnpm --dir apps/mobile build</code> and set \
        <code>DFLOW_PWA_DIST</code>, or ship the daemon with a bundled <code>pwa/</code> \
        directory beside it. The WS endpoint and pairing still work.</p></body>";
    (StatusCode::OK, [(header::CONTENT_TYPE, "text/html; charset=utf-8")], body).into_response()
}

/// A tiny content-type map for the PWA bundle (no dependency).
fn content_type_for(rel: &str) -> &'static str {
    let ext = rel.rsplit('.').next().unwrap_or("");
    match ext {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "application/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "webmanifest" => "application/json; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        _ => "application/octet-stream",
    }
}

// ---- LAN IP discovery -----------------------------------------------------

/// The primary LAN IPv4 address (the interface that would carry outbound traffic).
///
/// Uses the connectionless-UDP trick: a UDP `connect` sets a route without sending any
/// packet, so `local_addr()` reveals the interface the OS would use. Zero dependency,
/// no traffic. Returns `None` when there is no routable interface (offline).
pub fn primary_lan_ip() -> Option<String> {
    let sock = UdpSocket::bind("0.0.0.0:0").ok()?;
    // Any routable target works; no packet is sent for a UDP connect.
    sock.connect("8.8.8.8:80").ok()?;
    let ip = sock.local_addr().ok()?.ip();
    if ip.is_loopback() || ip.is_unspecified() {
        None
    } else {
        Some(ip.to_string())
    }
}

/// The reachable `http://<lan-ip>:<port>/m` URLs (currently the primary interface).
pub fn lan_urls(port: u16) -> Vec<String> {
    match primary_lan_ip() {
        Some(ip) => vec![format!("http://{ip}:{port}/m")],
        None => Vec::new(),
    }
}
