//! The loopback artifact service (`security.md` / Artifact sandbox architecture;
//! `plan-studio.md` / Sandboxing and serving; `spike5-plan-studio-chrome.md`).
//!
//! This is the daemon-side home of the dev artifact plugin the spike proved: it composes
//! the served document (injecting the review SDK and the bundled Mermaid build as
//! same-origin scripts, stamping the artifact id), stamps the exact strict CSP as a real
//! response header, and mints/verifies short-lived signed capability URLs so the iframe
//! holds a capability, never a bearer token. The signed URL, a tampered signature, and
//! an expired signature are all enforced here (`spike5`: tampered/expired -> 403).
//!
//! Signing is HMAC-SHA1 over `"{doc_id}.{exp_ms}"` with a random per-daemon key. A
//! loopback capability URL with a short TTL and silent re-signing on the parent side does
//! not need a collision-resistant MAC; HMAC-SHA1 with a 256-bit key is ample and adds no
//! new dependency (the same `sha1` crate the engine already uses).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use rand::Rng;
use serde::Deserialize;
use sha1::{Digest, Sha1};

use crate::server::AppState;

/// The exact CSP for artifact responses (`security.md` / Artifact sandbox architecture,
/// plus the `base-uri`/`form-action` additions spike 5 recommended folding in). On the
/// collapsed loopback origin the spec's `<artifact-origin>` and `<app-origin>` both
/// resolve to `'self'` (the daemon serves the document); a packaged build with a distinct
/// webview origin would set `frame-ancestors` to that app origin. Delivered as a real
/// header (not a `<meta>`), so `frame-ancestors` is honored (spike 5).
pub const ARTIFACT_CSP: &str = "default-src 'none'; img-src 'self' data:; \
     style-src 'self' 'unsafe-inline'; script-src 'self'; frame-ancestors 'self'; \
     connect-src 'none'; base-uri 'none'; form-action 'self'";

/// Default signed-URL lifetime. Short, with the desktop re-getting to re-sign near expiry
/// (`security.md`: short-lived; spike 5 recommends a shorter production TTL than its 60s).
pub const SIGNED_URL_TTL_MS: i64 = 120_000;

/// The embedded review SDK (`assets/review-sdk.js`), injected as a same-origin script.
const SDK_JS: &str = include_str!("../assets/review-sdk.js");
/// The bundled-Mermaid placeholder (`assets/mermaid.js`); see its header for the
/// packaging swap to the real ~3.5MB build.
const MERMAID_JS: &str = include_str!("../assets/mermaid.js");

/// Per-artifact long-poll waiters (`agent-cli.md` / `dflow plan poll` bounded long-poll).
///
/// A parked `artifact.feedback.poll` awaits its artifact's `Notify`; a
/// `artifact.feedback.submit` (or an approve/end) wakes every waiter for that artifact,
/// so the poll returns the moment feedback lands instead of burning the whole ~4-minute
/// budget. The store is the source of truth (feedback is never lost); the notify is only
/// a wakeup, so a missed notification just means the poll falls back to its timeout and
/// re-reads the store.
#[derive(Default)]
pub struct ArtifactWaiters {
    map: Mutex<HashMap<String, Arc<tokio::sync::Notify>>>,
}

impl ArtifactWaiters {
    /// The `Notify` for an artifact, creating it on first use.
    pub fn notify_for(&self, artifact_id: &str) -> Arc<tokio::sync::Notify> {
        let mut map = self.map.lock().expect("artifact waiters poisoned");
        Arc::clone(map.entry(artifact_id.to_string()).or_default())
    }

    /// Wake every poll parked on this artifact (feedback submitted, approved, or ended).
    pub fn wake(&self, artifact_id: &str) {
        if let Some(n) = self.map.lock().expect("artifact waiters poisoned").get(artifact_id) {
            n.notify_waiters();
        }
    }
}

/// Mints and verifies short-lived signed artifact URLs.
pub struct ArtifactSigner {
    key: Vec<u8>,
}

impl Default for ArtifactSigner {
    fn default() -> Self {
        Self::new()
    }
}

impl ArtifactSigner {
    /// A signer with a fresh random 32-byte per-daemon key. The key never leaves the
    /// daemon; a restart invalidates outstanding URLs (the desktop re-signs on reconnect).
    pub fn new() -> Self {
        let mut rng = rand::rng();
        let key: Vec<u8> = (0..32).map(|_| rng.random::<u8>()).collect();
        Self { key }
    }

    /// The signature hex for a `(doc_id, exp_ms)` capability.
    fn sign(&self, doc_id: &str, exp_ms: i64) -> String {
        let msg = format!("{doc_id}.{exp_ms}");
        hex(&hmac_sha1(&self.key, msg.as_bytes()))
    }

    /// A path+query capability for `doc_id`, valid for `SIGNED_URL_TTL_MS`. Returns
    /// `("/artifact/doc/<doc_id>?exp=<ms>&sig=<hex>", exp_ms)`.
    pub fn signed_path(&self, doc_id: &str) -> (String, i64) {
        let exp = now_ms() + SIGNED_URL_TTL_MS;
        let sig = self.sign(doc_id, exp);
        (format!("/artifact/doc/{doc_id}?exp={exp}&sig={sig}"), exp)
    }

    /// Verify a capability: the signature must match and `exp` must be in the future.
    /// Constant-time comparison so a byte-by-byte probe cannot forge one.
    pub fn verify(&self, doc_id: &str, exp_ms: i64, sig: &str) -> bool {
        if exp_ms <= now_ms() {
            return false;
        }
        let expected = self.sign(doc_id, exp_ms);
        constant_time_eq(expected.as_bytes(), sig.as_bytes())
    }
}

/// Compose the served document from the agent HTML: stamp the strict CSP via headers
/// (the caller adds it), and inject the review SDK and (when the artifact uses Mermaid)
/// the bundled Mermaid build as same-origin `<script src>` tags carrying the artifact id
/// and round. Inline scripts in the agent HTML are blocked by the CSP by design; the SDK
/// is external, so `script-src 'self'` admits it (spike 5).
pub fn compose_document(agent_html: &str, artifact_id: &str, round: u32) -> String {
    let mut injection = String::new();
    if uses_mermaid(agent_html) {
        // Lazy-inject Mermaid only when the artifact contains a mermaid block (spike 5:
        // the heavy bundle is injected only for artifacts that need it).
        injection.push_str("<script src=\"/artifact/asset/mermaid.js\"></script>\n");
    }
    injection.push_str(&format!(
        "<script src=\"/artifact/asset/sdk.js\" data-artifact-id=\"{}\" data-round=\"{}\"></script>\n",
        html_attr_escape(artifact_id),
        round
    ));

    // Prefer just before </head>; else before </body>; else prepend to the document.
    if let Some(pos) = find_ci(agent_html, "</head>") {
        let mut out = String::with_capacity(agent_html.len() + injection.len());
        out.push_str(&agent_html[..pos]);
        out.push_str(&injection);
        out.push_str(&agent_html[pos..]);
        out
    } else if let Some(pos) = find_ci(agent_html, "</body>") {
        let mut out = String::with_capacity(agent_html.len() + injection.len());
        out.push_str(&agent_html[..pos]);
        out.push_str(&injection);
        out.push_str(&agent_html[pos..]);
        out
    } else {
        format!("{injection}{agent_html}")
    }
}

/// Whether the artifact HTML uses a Mermaid diagram (so Mermaid is lazy-injected).
fn uses_mermaid(html: &str) -> bool {
    let lower = html.to_ascii_lowercase();
    lower.contains("class=\"mermaid\"") || lower.contains("class='mermaid'") || lower.contains("mermaid.")
}

// ---- axum handlers --------------------------------------------------------

/// The signed-URL query on a document request.
#[derive(Debug, Deserialize)]
pub struct DocQuery {
    exp: i64,
    sig: String,
}

/// `GET /artifact/doc/{doc_id}?exp&sig`: serve the composed, CSP-guarded document for a
/// valid signature, else 403 (tampered/expired). Loopback only; the iframe holds only the
/// capability, never a bearer token.
pub async fn artifact_doc_handler(
    Path(doc_id): Path<String>,
    Query(q): Query<DocQuery>,
    State(state): State<AppState>,
) -> Response {
    if !state.artifact_signer.verify(&doc_id, q.exp, &q.sig) {
        return (StatusCode::FORBIDDEN, "invalid or expired artifact signature").into_response();
    }
    let artifact = match state.store.get_artifact_by_doc(&doc_id) {
        Ok(Some(a)) => a,
        Ok(None) => return (StatusCode::NOT_FOUND, "no such artifact").into_response(),
        Err(err) => {
            tracing::warn!(%err, "artifact doc lookup failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, "lookup failed").into_response();
        }
    };
    let file = state.data_dir.artifact_file(&artifact.card_id, &doc_id);
    let agent_html = match std::fs::read_to_string(&file) {
        Ok(h) => h,
        Err(err) => {
            tracing::warn!(%err, path = %file.display(), "artifact file read failed");
            return (StatusCode::NOT_FOUND, "artifact content missing").into_response();
        }
    };
    let document = compose_document(&agent_html, &artifact.id, artifact.round.max(0) as u32);
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CONTENT_SECURITY_POLICY, ARTIFACT_CSP),
            (header::CACHE_CONTROL, "no-store"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        Body::from(document),
    )
        .into_response()
}

/// `GET /artifact/asset/{name}`: serve the injected same-origin scripts (`sdk.js`,
/// `mermaid.js`). Static, public JS (no card data), so unsigned; same-origin, so
/// `script-src 'self'` admits them.
pub async fn artifact_asset_handler(Path(name): Path<String>) -> Response {
    let body = match name.as_str() {
        "sdk.js" => SDK_JS,
        "mermaid.js" => MERMAID_JS,
        _ => return (StatusCode::NOT_FOUND, "no such asset").into_response(),
    };
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/javascript; charset=utf-8"),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        body,
    )
        .into_response()
}

/// Build the absolute signed URL a client points the iframe at, given the daemon's
/// loopback origin.
pub fn signed_url(signer: &ArtifactSigner, http_origin: &str, doc_id: &str) -> (String, i64) {
    let (path, exp) = signer.signed_path(doc_id);
    (format!("{http_origin}{path}"), exp)
}

// ---- HMAC-SHA1 (no new dependency) ----------------------------------------

/// HMAC-SHA1 (RFC 2104) over the daemon signing key, for capability URL signatures.
fn hmac_sha1(key: &[u8], msg: &[u8]) -> [u8; 20] {
    const BLOCK: usize = 64;
    let mut k = [0u8; BLOCK];
    if key.len() > BLOCK {
        let digest = Sha1::digest(key);
        k[..20].copy_from_slice(&digest);
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut ipad = [0x36u8; BLOCK];
    let mut opad = [0x5cu8; BLOCK];
    for i in 0..BLOCK {
        ipad[i] ^= k[i];
        opad[i] ^= k[i];
    }
    let mut inner = Sha1::new();
    inner.update(ipad);
    inner.update(msg);
    let inner_digest = inner.finalize();
    let mut outer = Sha1::new();
    outer.update(opad);
    outer.update(inner_digest);
    let out = outer.finalize();
    let mut result = [0u8; 20];
    result.copy_from_slice(&out);
    result
}

/// Lowercase hex encoding (no dependency).
fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

/// Length-checked constant-time byte comparison.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Case-insensitive substring position (for the injection point).
fn find_ci(haystack: &str, needle: &str) -> Option<usize> {
    let hl = haystack.to_ascii_lowercase();
    let nl = needle.to_ascii_lowercase();
    hl.find(&nl)
}

/// Minimal HTML attribute escaping for the injected artifact id (a ULID, but escaped
/// defensively).
fn html_attr_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('"', "&quot;").replace('<', "&lt;").replace('>', "&gt;")
}

fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// A convenience for building the daemon's loopback HTTP origin from the runtime port.
pub fn http_origin(port: u16) -> String {
    format!("http://127.0.0.1:{port}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_verify_roundtrip_and_tamper() {
        let signer = ArtifactSigner::new();
        let (path, exp) = signer.signed_path("01DOC");
        assert!(path.starts_with("/artifact/doc/01DOC?exp="));
        // Extract the sig from the path.
        let sig = path.split("sig=").nth(1).unwrap().to_string();
        assert!(signer.verify("01DOC", exp, &sig), "a fresh signature verifies");
        // Tampered doc id, tampered sig, and tampered exp all fail.
        assert!(!signer.verify("01OTHER", exp, &sig), "a different doc id must not verify");
        assert!(!signer.verify("01DOC", exp, &format!("{sig}00")), "a tampered sig must not verify");
        assert!(!signer.verify("01DOC", exp + 1, &sig), "a tampered exp must not verify");
    }

    #[test]
    fn expired_signature_rejected() {
        let signer = ArtifactSigner::new();
        let past = now_ms() - 1000;
        let sig = signer.sign("01DOC", past);
        assert!(!signer.verify("01DOC", past, &sig), "an expired signature must be rejected");
    }

    #[test]
    fn compose_injects_sdk_and_lazy_mermaid() {
        let html = "<html><head><title>Plan</title></head><body><p>hi</p></body></html>";
        let doc = compose_document(html, "ART1", 3);
        assert!(doc.contains("/artifact/asset/sdk.js"), "sdk injected");
        assert!(doc.contains("data-artifact-id=\"ART1\""), "artifact id stamped");
        assert!(doc.contains("data-round=\"3\""), "round stamped");
        assert!(!doc.contains("mermaid.js"), "mermaid not injected when unused");

        let with_mermaid = "<head></head><body><div class=\"mermaid\">graph TD;A-->B;</div></body>";
        let doc2 = compose_document(with_mermaid, "ART2", 1);
        assert!(doc2.contains("/artifact/asset/mermaid.js"), "mermaid lazy-injected when used");
        // The injection lands before </head>.
        let head_close = doc.find("</head>").unwrap();
        let sdk_at = doc.find("sdk.js").unwrap();
        assert!(sdk_at < head_close, "sdk injected before </head>");
    }

    #[test]
    fn known_hmac_sha1_vector() {
        // RFC 2202 HMAC-SHA1 test case 1: key=0x0b*20, data="Hi There".
        let key = [0x0bu8; 20];
        let mac = hmac_sha1(&key, b"Hi There");
        assert_eq!(hex(&mac), "b617318655057264e28bc0b6fb378c8ef146be00");
    }
}
