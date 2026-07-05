//! Locate, own, and describe the `dflowd` daemon (`daemon-lifecycle.md`).
//!
//! The daemon owns all sessions and MUST outlive the GUI (`architecture.md`: "closing
//! the GUI never kills agents"). Startup resolution is the same in both modes: read the
//! runtime file and connect to a live daemon (pid alive + socket answers) if there is
//! one. Only when there is none do the two modes diverge:
//!
//! - **Production** (`prod-managed`, the default for a release build): copy the bundled
//!   daemon into a stable, writable location (`%LOCALAPPDATA%/DapperFlow/bin/dflowd.exe`)
//!   on first run and whenever the bundled version differs, then spawn THAT copy fully
//!   detached. The app never runs the compiler's `target/` output and never locks the
//!   file it might need to replace on update.
//! - **Development** (`dev-external`, the default for a debug build, or forced with
//!   `DFLOW_DEV_EXTERNAL_DAEMON`): the app does NOT spawn anything. It connects as a pure
//!   client and, if no daemon is live, returns a hint so the UI can say "start the dev
//!   daemon (`just daemon-dev`)" instead of spawning `target/debug` and locking it.

use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use serde::Serialize;

/// How the app relates to the daemon for this run (surfaced in the status bar + Settings).
#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "kebab-case")]
pub enum DaemonMode {
    /// The app connects to an externally-run daemon and never spawns one.
    DevExternal,
    /// The app owns a bundled daemon copied to a stable location and spawns it detached.
    ProdManaged,
}

/// The loopback coordinates the webview needs to open the daemon WebSocket, plus how this
/// run relates to the daemon (surfaced in the status bar and Settings > Daemon).
#[derive(Serialize, Clone)]
pub struct DaemonInfo {
    pub port: u16,
    pub token: String,
    /// This run spawned the daemon (`true`, prod only) vs attached to a live one (`false`).
    pub started: bool,
    /// Whether `port`/`token` point at a live daemon. `false` in dev-external mode when no
    /// daemon is running yet - the app shows `hint` instead of connecting.
    pub connected: bool,
    /// Dev-external vs prod-managed ownership for this run.
    pub mode: DaemonMode,
    /// Guidance shown when not connected (dev-external, no daemon running).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

const DAEMON_EXE: &str = "dflowd.exe";
const READY_TIMEOUT: Duration = Duration::from_secs(15);

/// Resolve the daemon for this run: attach to a live one, else (prod) spawn the managed
/// copy, or (dev) return a hint to start the dev daemon.
pub fn ensure_running() -> Result<DaemonInfo, String> {
    let mode = daemon_mode();

    // 1. Attach to a live daemon if the runtime file points at one that answers. Same in
    //    both modes: the GUI is a reconnectable client (`architecture.md`).
    if let Some((port, token)) = read_and_probe() {
        return Ok(DaemonInfo { port, token, started: false, connected: true, mode, hint: None });
    }

    // 2. No live daemon. Diverge by ownership mode.
    match mode {
        DaemonMode::DevExternal => Ok(DaemonInfo {
            port: 0,
            token: String::new(),
            started: false,
            connected: false,
            mode,
            hint: Some(
                "No dev daemon is running. Start it with `just daemon-dev` \
                 (or scripts/daemon-dev.ps1), then reconnect."
                    .to_string(),
            ),
        }),
        DaemonMode::ProdManaged => {
            let path = ensure_managed_daemon()?;
            spawn_daemon(&path)?;
            let deadline = Instant::now() + READY_TIMEOUT;
            while Instant::now() < deadline {
                if let Some((port, token)) = read_and_probe() {
                    return Ok(DaemonInfo {
                        port,
                        token,
                        started: true,
                        connected: true,
                        mode,
                        hint: None,
                    });
                }
                std::thread::sleep(Duration::from_millis(150));
            }
            Err("dflowd did not become ready in time".to_string())
        }
    }
}

/// Gracefully stop the running daemon by invoking `<daemon> --stop` (reaps the whole tree
/// via the Job Object, marks sessions interrupted/resumable). Used by the tray Stop and by
/// a keep-alive-off quit. Best-effort binary resolution: the managed copy, else the bundled
/// source. This is NEVER a force-kill - the app only ever asks the daemon to stop itself.
pub fn graceful_stop() -> Result<(), String> {
    let bin = managed_daemon_path()
        .ok()
        .filter(|p| p.exists())
        .or_else(|| bundled_daemon_source().ok())
        .ok_or_else(|| "could not locate a dflowd binary to stop".to_string())?;
    let status = std::process::Command::new(&bin)
        .arg("--stop")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| format!("running {} --stop: {e}", bin.display()))?;
    // `--stop` exits 0 (stopped) or 3 (nothing was running); both mean "no daemon left".
    match status.code() {
        Some(0) | Some(3) => Ok(()),
        Some(c) => Err(format!("dflowd --stop exited with code {c}")),
        None => Err("dflowd --stop was terminated by a signal".to_string()),
    }
}

/// Which ownership mode applies. `DFLOW_DEV_EXTERNAL_DAEMON` is the explicit override
/// (`1`/`true` -> dev-external, `0`/`false` -> prod-managed); unset defaults to
/// dev-external for a debug build and prod-managed for a release build.
fn daemon_mode() -> DaemonMode {
    match std::env::var("DFLOW_DEV_EXTERNAL_DAEMON").ok().as_deref().map(|s| s.trim().to_ascii_lowercase()) {
        Some(ref v) if v == "1" || v == "true" || v == "yes" || v == "on" => DaemonMode::DevExternal,
        Some(ref v) if v == "0" || v == "false" || v == "no" || v == "off" => DaemonMode::ProdManaged,
        _ => {
            if cfg!(debug_assertions) {
                DaemonMode::DevExternal
            } else {
                DaemonMode::ProdManaged
            }
        }
    }
}

/// The published runtime descriptor path (`%LOCALAPPDATA%/DapperFlow/runtime.json`),
/// matching `dflow_core::DataDir`'s default root.
fn runtime_path() -> Option<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA")?;
    Some(PathBuf::from(base).join("DapperFlow").join("runtime.json"))
}

/// Read the runtime file and confirm the daemon is genuinely live: its pid is alive and
/// its loopback port answers. A stale file (dead pid or dead port) returns `None`, so a
/// crashed daemon never blocks the next start and is never mistaken for a live one.
fn read_and_probe() -> Option<(u16, String)> {
    let text = std::fs::read_to_string(runtime_path()?).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    let port = u16::try_from(value.get("port")?.as_u64()?).ok()?;
    let token = value.get("token")?.as_str()?.to_string();
    if port == 0 {
        return None;
    }
    // pid liveness first (cheap, and rejects a stale file whose daemon has exited).
    if let Some(pid) = value.get("pid").and_then(|p| p.as_u64()) {
        if pid != 0 && !pid_alive(pid as u32) {
            return None;
        }
    }
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    match TcpStream::connect_timeout(&addr, Duration::from_millis(400)) {
        Ok(_) => Some((port, token)),
        Err(_) => None,
    }
}

/// Whether a process with this pid is currently alive (best-effort).
#[cfg(windows)]
fn pid_alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    const STILL_ACTIVE: u32 = 259;
    // SAFETY: OpenProcess returns a handle we own (or null); we query its exit code and
    // close the handle exactly once.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return false;
        }
        let mut code: u32 = 0;
        let ok = GetExitCodeProcess(handle, &mut code);
        CloseHandle(handle);
        ok != 0 && code == STILL_ACTIVE
    }
}

#[cfg(not(windows))]
fn pid_alive(_pid: u32) -> bool {
    // Unix liveness lands with M1; until then trust the socket probe alone.
    true
}

/// The stable, writable location the app runs the daemon from in production.
fn managed_daemon_path() -> Result<PathBuf, String> {
    let base = std::env::var_os("LOCALAPPDATA")
        .ok_or_else(|| "LOCALAPPDATA is not set; cannot resolve the managed daemon path".to_string())?;
    Ok(PathBuf::from(base).join("DapperFlow").join("bin").join(DAEMON_EXE))
}

/// Ensure the managed daemon copy exists and is current, and return its path. Copies the
/// bundled daemon into the writable location on first run and whenever the bundled version
/// differs (`daemon-lifecycle.md` / Production). A locked existing copy is tolerated: we
/// keep using it rather than fail the launch.
fn ensure_managed_daemon() -> Result<PathBuf, String> {
    let managed = managed_daemon_path()?;
    let source = bundled_daemon_source()?;

    // If the resolved source already IS the managed copy (e.g. DFLOWD_PATH points at it),
    // there is nothing to copy.
    if same_file(&source, &managed) {
        return Ok(managed);
    }

    let need_copy = if !managed.exists() {
        true
    } else {
        match (daemon_version(&source), daemon_version(&managed)) {
            (Some(sv), Some(mv)) => sv != mv, // bundled version changed -> refresh
            (Some(_), None) => true,          // installed copy unreadable -> refresh
            (None, _) => false,               // can't read the source version -> keep what we have
        }
    };

    if need_copy {
        if let Some(dir) = managed.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| format!("creating {}: {e}", dir.display()))?;
        }
        if let Err(e) = std::fs::copy(&source, &managed) {
            if managed.exists() {
                // Could not refresh (e.g. the file is locked by a running instance), but a
                // usable copy is already in place - proceed with it rather than fail.
                eprintln!(
                    "dapperflow: could not refresh managed daemon ({e}); using existing {}",
                    managed.display()
                );
            } else {
                return Err(format!(
                    "copying bundled daemon {} -> {}: {e}",
                    source.display(),
                    managed.display()
                ));
            }
        }
    }
    Ok(managed)
}

/// The bundled daemon shipped with the app: an explicit `DFLOWD_PATH`, the sidecar next to
/// the app executable (packaged builds), or - only when prod-managed mode is forced in a
/// dev checkout - the workspace `target/` build.
fn bundled_daemon_source() -> Result<PathBuf, String> {
    if let Some(p) = std::env::var_os("DFLOWD_PATH").map(PathBuf::from) {
        if p.exists() {
            return Ok(p);
        }
    }
    // Packaged builds ship the daemon as a sidecar next to the app executable.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join(DAEMON_EXE);
            if candidate.exists() {
                return Ok(candidate);
            }
        }
    }
    // Dev fallback (prod-managed forced in a checkout): walk up to the workspace target.
    let starts = [
        std::env::current_dir().ok(),
        std::env::current_exe().ok().and_then(|e| e.parent().map(PathBuf::from)),
    ];
    for start in starts.into_iter().flatten() {
        let mut dir: Option<&Path> = Some(start.as_path());
        while let Some(d) = dir {
            for profile in ["release", "debug"] {
                let candidate = d.join("target").join(profile).join(DAEMON_EXE);
                if candidate.exists() {
                    return Ok(candidate);
                }
            }
            dir = d.parent();
        }
    }
    Err(format!(
        "could not locate a bundled {DAEMON_EXE}; set DFLOWD_PATH or build it with `cargo build -p dflowd`"
    ))
}

/// The daemon build string via `<path> --version`, for the copy-if-newer comparison.
fn daemon_version(path: &Path) -> Option<String> {
    let out = std::process::Command::new(path).arg("--version").output().ok()?;
    if !out.status.success() {
        return None;
    }
    let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if v.is_empty() {
        None
    } else {
        Some(v)
    }
}

/// Whether two paths refer to the same existing file (by canonicalized path).
fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// Spawn the daemon fully detached on Windows: its lifetime is decoupled from the app so
/// closing (or crashing) the GUI leaves sessions running.
#[cfg(windows)]
fn spawn_daemon(path: &Path) -> Result<(), String> {
    use std::os::windows::process::CommandExt;

    // winbase.h creation flags.
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;

    // No inherited console, its own process group, no window; the daemon logs to its data
    // dir, not our stdio.
    let base = CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS | CREATE_NO_WINDOW;

    let spawn = |flags: u32| {
        std::process::Command::new(path)
            .creation_flags(flags)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
    };

    // Prefer breaking away from any Job Object the app itself runs inside (a dev harness or
    // launcher that would otherwise kill the daemon with the app's tree). If that job
    // forbids breakaway, CreateProcess fails with ACCESS_DENIED; fall back to a plain
    // detached spawn, which still escapes the console but shares the job.
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
fn spawn_daemon(path: &Path) -> Result<(), String> {
    std::process::Command::new(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("failed to start daemon at {}: {e}", path.display()))
}
