//! The Tauri shell. Its only job in Phase 0 is to make sure `dflowd` is running
//! and hand the webview `{ port, token }` for the loopback WebSocket. All PTY data
//! flows over that WebSocket, never through Tauri IPC (`protocol.md`, `security.md`).

mod daemon;
mod mcp;

use daemon::DaemonInfo;

/// Ensure the daemon is up and return its loopback port and root token.
#[tauri::command]
async fn daemon_info() -> Result<DaemonInfo, String> {
    // Offload the blocking probe/spawn so the UI thread never stalls.
    tauri::async_runtime::spawn_blocking(daemon::ensure_running)
        .await
        .map_err(|e| format!("daemon task failed: {e}"))?
}

/// Open a project folder in the OS file manager (Projects tree > Reveal in Explorer).
/// App-defined command, so it needs no extra capability ACL.
#[tauri::command]
fn reveal_in_explorer(path: String) -> Result<(), String> {
    #[cfg(windows)]
    {
        std::process::Command::new("explorer")
            .arg(&path)
            .spawn()
            .map_err(|e| format!("could not open Explorer: {e}"))?;
        return Ok(());
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(&path)
            .spawn()
            .map_err(|e| format!("could not open Finder: {e}"))?;
        return Ok(());
    }
    #[cfg(all(not(windows), not(target_os = "macos")))]
    {
        std::process::Command::new("xdg-open")
            .arg(&path)
            .spawn()
            .map_err(|e| format!("could not open file manager: {e}"))?;
        Ok(())
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // Native folder picker for the add-project flow (FIX: pick a directory).
        .plugin(tauri_plugin_dialog::init())
        // Desktop notifications for Needs You arrivals (Phase 2 UI, deliverable 3).
        .plugin(tauri_plugin_notification::init())
        .invoke_handler(tauri::generate_handler![
            daemon_info,
            reveal_in_explorer,
            mcp::mcp_install_hint,
            mcp::mcp_detect
        ])
        .run(tauri::generate_context!())
        .expect("error while running DapperFlow");
}
