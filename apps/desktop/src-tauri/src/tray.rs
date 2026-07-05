//! The system tray (`daemon-lifecycle.md` / Quit behavior): a persistent presence that
//! makes a backgrounded daemon visible and controllable even with the window closed.
//!
//! Menu: Open DapperFlow, a live daemon status line, Stop daemon, Restart daemon, and
//! Quit. Stop and Restart route through the frontend (a `tray://` event) so they reuse the
//! app's confirm-when-live-sessions dialog and its graceful WebSocket shutdown path - the
//! app NEVER force-kills the daemon. Quit honors the persisted keep-alive setting: on,
//! the detached daemon keeps running; off, the app sends a graceful `--stop` first.

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, Wry};

/// Build the tray icon and menu, and stash the status item so the frontend can keep it
/// current. A no-op (with a note) if there is no window icon to reuse.
pub fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let icon = match app.default_window_icon().cloned() {
        Some(icon) => icon,
        None => {
            eprintln!("dapperflow: no window icon available; skipping system tray");
            return Ok(());
        }
    };

    let open = MenuItem::with_id(app, "open", "Open DapperFlow", true, None::<&str>)?;
    // A disabled status line the frontend updates via `set_tray_status`.
    let status = MenuItem::with_id(app, "status", "Daemon: checking...", false, None::<&str>)?;
    let stop = MenuItem::with_id(app, "stop", "Stop daemon", true, None::<&str>)?;
    let restart = MenuItem::with_id(app, "restart", "Restart daemon", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit DapperFlow", true, None::<&str>)?;
    let sep1 = PredefinedMenuItem::separator(app)?;
    let sep2 = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&open, &sep1, &status, &stop, &restart, &sep2, &quit])?;

    // Hand the status item to the shell state so `set_tray_status` can update its text.
    if let Some(state) = app.try_state::<crate::ShellState>() {
        if let Ok(mut guard) = state.tray_status.lock() {
            *guard = Some(status.clone());
        }
    }

    TrayIconBuilder::with_id("main-tray")
        .icon(icon)
        .tooltip("DapperFlow")
        .menu(&menu)
        // Left-click opens the window; the menu is the right-click surface.
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| handle_menu(app, event.id.as_ref()))
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                crate::show_main_window(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

/// Dispatch a tray menu selection.
fn handle_menu(app: &AppHandle<Wry>, id: &str) {
    match id {
        "open" => crate::show_main_window(app),
        // Show the window first so the frontend's confirm-when-live dialog is visible, then
        // let the frontend run its existing graceful stop/restart flow.
        "stop" => {
            crate::show_main_window(app);
            let _ = app.emit("tray://stop-daemon", ());
        }
        "restart" => {
            crate::show_main_window(app);
            let _ = app.emit("tray://restart-daemon", ());
        }
        "quit" => {
            // Keep-alive OFF -> the daemon should not linger, so stop it gracefully first.
            // ON -> the detached daemon keeps running after the app exits.
            let keep_alive = app
                .try_state::<crate::ShellState>()
                .map(|s| s.settings.lock().map(|g| g.keep_alive).unwrap_or(true))
                .unwrap_or(true);
            if !keep_alive {
                let _ = crate::daemon::graceful_stop();
            }
            app.exit(0);
        }
        _ => {}
    }
}
