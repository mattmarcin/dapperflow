//! Daemon endpoint discovery via the published runtime file.
//!
//! The daemon writes `<data-dir>/runtime.json` (`{ port, token, pid, version }`,
//! see `dflowd/src/runtime.rs`) on startup; the MCP server reads it to find the
//! loopback WebSocket endpoint and the root bearer token. `DFLOW_DATA_DIR`
//! overrides the data dir everywhere (`dflow-core/src/paths.rs`), which is also
//! how tests and the live proof isolate themselves from a user daemon.
//!
//! Discovery runs per tool call, not at server boot: the MCP server can be
//! mounted before the daemon starts, and it survives daemon restarts (which
//! re-mint the port and can rotate the token) without any reconnect logic.

use dflow_core::DataDir;
use serde::Deserialize;

use crate::daemon::DaemonError;

/// The resolved daemon endpoint: where to connect and how to authenticate.
#[derive(Debug, Clone)]
pub struct DaemonEndpoint {
    /// `ws://127.0.0.1:<port>/ws`.
    pub endpoint: String,
    /// The root bearer token (`security.md` / Token architecture).
    pub token: String,
}

/// The subset of `runtime.json` the client needs.
#[derive(Debug, Deserialize)]
struct RuntimeFile {
    port: u16,
    token: String,
}

/// Discover the daemon endpoint from the environment (`DFLOW_DATA_DIR` honored).
pub fn discover() -> Result<DaemonEndpoint, DaemonError> {
    let dir = DataDir::resolve().map_err(|e| DaemonError::Setup(e.to_string()))?;
    discover_at(&dir)
}

/// Discover the daemon endpoint from an explicit data dir (testable core).
pub fn discover_at(dir: &DataDir) -> Result<DaemonEndpoint, DaemonError> {
    let path = dir.runtime_file();
    let text = std::fs::read_to_string(&path).map_err(|e| {
        DaemonError::Setup(format!(
            "cannot read the daemon runtime file at {}: {e}; is dflowd running? \
             (set DFLOW_DATA_DIR if the daemon uses a non-default data dir)",
            path.display()
        ))
    })?;
    let parsed: RuntimeFile = serde_json::from_str(&text).map_err(|e| {
        DaemonError::Setup(format!("malformed runtime file {}: {e}", path.display()))
    })?;
    if parsed.port == 0 {
        return Err(DaemonError::Setup(format!(
            "runtime file {} has port 0; the daemon is still starting",
            path.display()
        )));
    }
    if parsed.token.trim().is_empty() {
        return Err(DaemonError::Setup(format!(
            "runtime file {} has an empty token",
            path.display()
        )));
    }
    Ok(DaemonEndpoint {
        endpoint: format!("ws://127.0.0.1:{}/ws", parsed.port),
        token: parsed.token,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("dflow-mcp-{tag}-{nanos}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn missing_runtime_file_names_the_path_and_the_daemon() {
        let dir = temp_dir("no-runtime");
        let err = discover_at(&DataDir::at(&dir)).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("runtime.json"), "should name the file: {msg}");
        assert!(msg.contains("is dflowd running"), "should hint at the daemon: {msg}");
    }

    #[test]
    fn valid_runtime_file_yields_loopback_endpoint() {
        let dir = temp_dir("runtime-ok");
        std::fs::write(
            dir.join("runtime.json"),
            r#"{ "port": 4567, "token": "tok123", "pid": 1, "version": "x" }"#,
        )
        .unwrap();
        let ep = discover_at(&DataDir::at(&dir)).unwrap();
        assert_eq!(ep.endpoint, "ws://127.0.0.1:4567/ws");
        assert_eq!(ep.token, "tok123");
    }

    #[test]
    fn port_zero_reads_as_still_starting() {
        let dir = temp_dir("runtime-port0");
        std::fs::write(dir.join("runtime.json"), r#"{ "port": 0, "token": "t" }"#).unwrap();
        let err = discover_at(&DataDir::at(&dir)).unwrap_err();
        assert!(err.to_string().contains("still starting"));
    }
}
