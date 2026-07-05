//! Locate, start, and describe the `dflowd` daemon.
//!
//! The daemon owns all sessions and MUST outlive the GUI (`architecture.md`: "closing
//! the GUI never kills agents"). So on startup we first try the runtime file and
//! connect to an already-running daemon (a previous GUI run, or a standalone one), and
//! only spawn a new one when that connection genuinely fails. A spawned daemon is
//! launched fully detached so closing the app - even a dev `pnpm tauri dev` run whose
//! process tree would otherwise take the child down - never kills it.

use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Duration, Instant};

use serde::Serialize;

/// The loopback coordinates the webview needs to open the daemon WebSocket, plus
/// whether this run had to spawn the daemon (`started = true`) or attached to one that
/// was already alive (`started = false`) - surfaced in the status bar.
#[derive(Serialize, Clone)]
pub struct DaemonInfo {
    pub port: u16,
    pub token: String,
    pub started: bool,
}

const DAEMON_EXE: &str = "dflowd.exe";
const READY_TIMEOUT: Duration = Duration::from_secs(15);

/// Ensure a daemon is running and return its `{ port, token, started }`.
pub fn ensure_running() -> Result<DaemonInfo, String> {
    // 1. Attach to a live daemon if the runtime file points at one that answers.
    if let Some((port, token)) = read_and_probe() {
        return Ok(DaemonInfo { port, token, started: false });
    }
    // 2. Otherwise spawn one (a stale runtime file falls through to here; the daemon's
    //    own single-instance lock is the backstop against a double spawn).
    spawn_daemon()?;
    let deadline = Instant::now() + READY_TIMEOUT;
    while Instant::now() < deadline {
        if let Some((port, token)) = read_and_probe() {
            return Ok(DaemonInfo { port, token, started: true });
        }
        std::thread::sleep(Duration::from_millis(150));
    }
    Err("dflowd did not become ready in time".to_string())
}

fn runtime_path() -> Option<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA")?;
    Some(PathBuf::from(base).join("DapperFlow").join("runtime.json"))
}

/// Read the runtime file and confirm the daemon is actually listening on its port.
/// A stale file (dead port) returns `None`, forcing a respawn.
fn read_and_probe() -> Option<(u16, String)> {
    let text = std::fs::read_to_string(runtime_path()?).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    let port = u16::try_from(value.get("port")?.as_u64()?).ok()?;
    let token = value.get("token")?.as_str()?.to_string();
    if port == 0 {
        return None;
    }
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    match TcpStream::connect_timeout(&addr, Duration::from_millis(400)) {
        Ok(_) => Some((port, token)),
        Err(_) => None,
    }
}

fn daemon_path() -> Result<PathBuf, String> {
    find_daemon().ok_or_else(|| {
        format!("could not locate {DAEMON_EXE}; build it with `cargo build` at the repo root or set DFLOWD_PATH")
    })
}

/// Spawn the daemon fully detached on Windows: its lifetime is decoupled from the app
/// so closing the GUI leaves sessions running.
#[cfg(windows)]
fn spawn_daemon() -> Result<(), String> {
    use std::os::windows::process::CommandExt;

    // winbase.h creation flags.
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;

    let path = daemon_path()?;
    // No inherited console, its own process group, no window; the daemon logs to its
    // data dir, not our stdio.
    let base = CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS | CREATE_NO_WINDOW;

    let spawn = |flags: u32| {
        std::process::Command::new(&path)
            .creation_flags(flags)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
    };

    // Prefer breaking away from any Job Object the app itself runs inside (a dev harness
    // or a launcher that would kill the daemon with the app's tree). If that job forbids
    // breakaway, CreateProcess fails with ACCESS_DENIED; fall back to a plain detached
    // spawn, which still escapes the console but shares the job. Documented in the
    // evidence file: dev-mode `pnpm tauri dev` is the case that most needs breakaway.
    match spawn(base | CREATE_BREAKAWAY_FROM_JOB) {
        Ok(_) => Ok(()),
        Err(_) => spawn(base)
            .map(|_| ())
            .map_err(|e| format!("failed to start daemon at {}: {e}", path.display())),
    }
}

/// Non-Windows spawn: a plain detached child with nulled stdio (the app is not a
/// controlling shell, so this survives the GUI closing). macOS/Linux enter CI at M1.
#[cfg(not(windows))]
fn spawn_daemon() -> Result<(), String> {
    let path = daemon_path()?;
    std::process::Command::new(&path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("failed to start daemon at {}: {e}", path.display()))
}

/// Resolve the daemon binary robustly across dev and packaged layouts.
fn find_daemon() -> Option<PathBuf> {
    // 1. Explicit override.
    if let Some(p) = std::env::var_os("DFLOWD_PATH").map(PathBuf::from) {
        if p.exists() {
            return Some(p);
        }
    }
    // 2. Alongside this executable (packaged builds ship the daemon next to the app).
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join(DAEMON_EXE);
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }
    // 3. Dev: walk up from the cwd and the exe dir looking for the workspace target.
    let starts = [
        std::env::current_dir().ok(),
        std::env::current_exe().ok().and_then(|e| e.parent().map(PathBuf::from)),
    ];
    for start in starts.into_iter().flatten() {
        let mut dir: Option<&std::path::Path> = Some(start.as_path());
        while let Some(d) = dir {
            for profile in ["debug", "release"] {
                let candidate = d.join("target").join(profile).join(DAEMON_EXE);
                if candidate.exists() {
                    return Some(candidate);
                }
            }
            dir = d.parent();
        }
    }
    // 4. Last resort: rely on PATH.
    Some(PathBuf::from(DAEMON_EXE))
}
