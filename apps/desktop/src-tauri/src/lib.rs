//! The Tauri shell. It makes sure `dflowd` is reachable and hands the webview
//! `{ port, token, ... }` for the loopback WebSocket; all PTY data flows over that
//! WebSocket, never through Tauri IPC (`protocol.md`, `security.md`).
//!
//! It also owns the OS-native lifecycle chrome (`daemon-lifecycle.md` / Quit behavior): a
//! system tray that keeps a backgrounded daemon visible and controllable, close-to-tray so
//! the daemon keeps running when the window closes, and a persisted "keep agents running"
//! setting that decides whether quitting sends a graceful `--stop`. Every app-initiated
//! daemon shutdown is graceful - the app NEVER force-kills the daemon.

mod daemon;
mod mcp;
mod tray;

use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::menu::MenuItem;
use tauri::{Manager, Wry};

use daemon::DaemonInfo;

/// Persisted app-side settings (distinct from daemon/store settings). Kept small and
/// owned by the shell because they gate shell behavior (quit).
#[derive(Serialize, Deserialize, Clone)]
pub struct AppSettings {
    /// "Keep agents running when I close the window" - when false, quitting sends a
    /// graceful `--stop` so nothing lingers. Default true (the GUI is a lens).
    pub keep_alive: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self { keep_alive: true }
    }
}

/// Shared shell state: the persisted settings plus a handle to the tray's status line so
/// the frontend can keep it current.
pub struct ShellState {
    pub settings: Mutex<AppSettings>,
    pub tray_status: Mutex<Option<MenuItem<Wry>>>,
}

/// The app-settings file (`%LOCALAPPDATA%/DapperFlow/app-settings.json`), alongside the
/// daemon data dir so it travels with the rest of the user's DapperFlow state.
fn settings_path() -> Option<std::path::PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA")?;
    Some(std::path::PathBuf::from(base).join("DapperFlow").join("app-settings.json"))
}

fn load_settings() -> AppSettings {
    settings_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn save_settings(settings: &AppSettings) -> Result<(), String> {
    let path = settings_path().ok_or_else(|| "LOCALAPPDATA is not set".to_string())?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
    }
    let json = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| format!("writing {}: {e}", path.display()))
}

/// Ensure the daemon is up (or, in dev-external mode, report that it is not) and return
/// its loopback coordinates plus the ownership mode.
#[tauri::command]
async fn daemon_info() -> Result<DaemonInfo, String> {
    // Offload the blocking probe/spawn so the UI thread never stalls.
    tauri::async_runtime::spawn_blocking(daemon::ensure_running)
        .await
        .map_err(|e| format!("daemon task failed: {e}"))?
}

/// Read the persisted "keep agents running" setting.
#[tauri::command]
fn get_keep_alive(state: tauri::State<'_, ShellState>) -> bool {
    state.settings.lock().map(|s| s.keep_alive).unwrap_or(true)
}

/// Persist the "keep agents running" setting.
#[tauri::command]
fn set_keep_alive(state: tauri::State<'_, ShellState>, value: bool) -> Result<(), String> {
    let snapshot = {
        let mut s = state.settings.lock().map_err(|_| "settings lock poisoned".to_string())?;
        s.keep_alive = value;
        s.clone()
    };
    save_settings(&snapshot)
}

/// Update the tray's daemon status line (the frontend, which tracks live sessions, drives
/// this). Best-effort: a missing tray (unsupported platform) is a no-op.
#[tauri::command]
fn set_tray_status(state: tauri::State<'_, ShellState>, text: String) -> Result<(), String> {
    if let Ok(guard) = state.tray_status.lock() {
        if let Some(item) = guard.as_ref() {
            let _ = item.set_text(text);
        }
    }
    Ok(())
}

/// Gracefully stop the daemon from the app (tray Stop, or a keep-alive-off quit). Shells
/// out to the resolved daemon binary's `--stop`, which reaps the tree via the Job Object
/// and marks sessions resumable. NEVER a force-kill.
#[tauri::command]
async fn stop_daemon_graceful() -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(daemon::graceful_stop)
        .await
        .map_err(|e| format!("stop task failed: {e}"))?
}

/// Show, unminimize, and focus the main window (tray "Open", or relaunch after
/// close-to-tray). Creates the window if it was destroyed.
pub fn show_main_window(app: &tauri::AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.show();
        let _ = win.unminimize();
        let _ = win.set_focus();
    } else if let Some(cfg) =
        app.config().app.windows.iter().find(|w| w.label == "main").cloned()
    {
        // The window was fully closed; rebuild it from the config.
        let _ = tauri::WebviewWindowBuilder::from_config(app, &cfg).and_then(|b| b.build());
    }
}

/// Open a project folder in the OS file manager (Projects tree > Reveal in Explorer).
#[tauri::command]
fn reveal_in_explorer(path: String) -> Result<(), String> {
    #[cfg(windows)]
    {
        std::process::Command::new("explorer")
            .arg(&path)
            .spawn()
            .map_err(|e| format!("could not open Explorer: {e}"))?;
        Ok(())
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(&path)
            .spawn()
            .map_err(|e| format!("could not open Finder: {e}"))?;
        Ok(())
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
        .manage(ShellState {
            settings: Mutex::new(load_settings()),
            tray_status: Mutex::new(None),
        })
        // Native folder picker for the add-project flow (FIX: pick a directory).
        .plugin(tauri_plugin_dialog::init())
        // Desktop notifications for Needs You arrivals (Phase 2 UI, deliverable 3).
        .plugin(tauri_plugin_notification::init())
        .invoke_handler(tauri::generate_handler![
            daemon_info,
            reveal_in_explorer,
            get_keep_alive,
            set_keep_alive,
            set_tray_status,
            stop_daemon_graceful,
            mcp::mcp_install_hint,
            mcp::mcp_detect
        ])
        .setup(|app| {
            tray::build_tray(app.handle())?;
            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing the WINDOW never stops the daemon: hide to the tray instead of
            // destroying, so the daemon keeps running and the tray stays in control
            // (`daemon-lifecycle.md` / Quit). A real quit goes through the tray's Quit item.
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running DapperFlow");
}
