//! `dflowd --stop` and `dflowd --status`: reliable daemon lifecycle control, the dev
//! sanity check for a daemon that outlives the app (`architecture.md` / daemon
//! restarts). Both honor `DFLOW_DATA_DIR` (and `--data-dir`, mapped to it in `main`).
//!
//! Running-detection is the single-instance lock itself: a live daemon holds it for
//! its whole lifetime, and the OS releases it the instant the process dies (even on an
//! unclean death), so a stale lock or runtime file never fools this and never needs
//! manual deletion (stale-state hygiene).

use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use dflow_core::{DataDir, Store};
use fs2::FileExt;

use crate::runtime::RuntimeInfo;

/// Exit code for `--stop` when there was nothing running to stop.
pub const EXIT_NOT_RUNNING: i32 = 3;

/// `dflowd --status`: one line describing whether a daemon is running, and if so its
/// pid, port, live session count, and data dir.
pub fn status() -> Result<()> {
    let data_dir = DataDir::resolve().context("resolving data dir")?;
    let root = data_dir.root().display().to_string();
    if !daemon_running(&data_dir) {
        println!("dflowd: not running  data_dir={root}");
        return Ok(());
    }
    let info = read_runtime(&data_dir);
    let pid = info.as_ref().map(|i| i.pid).unwrap_or(0);
    let port = info.as_ref().map(|i| i.port).unwrap_or(0);
    let live = Store::open(&data_dir.db_file())
        .ok()
        .and_then(|s| s.count_live_sessions().ok())
        .unwrap_or(0);
    println!("dflowd: running  pid={pid}  port={port}  live_sessions={live}  data_dir={root}");
    Ok(())
}

/// `dflowd --stop`: request a graceful shutdown (POST /shutdown with the root token),
/// wait for the daemon to exit, and fall back to killing its process tree. Returns the
/// process exit code (0 stopped, `EXIT_NOT_RUNNING` if nothing was running).
pub fn stop() -> Result<i32> {
    let data_dir = DataDir::resolve().context("resolving data dir")?;
    if !daemon_running(&data_dir) {
        println!("dflowd: nothing to stop (not running)");
        return Ok(EXIT_NOT_RUNNING);
    }
    let info = read_runtime(&data_dir);
    let pid = info.as_ref().map(|i| i.pid).unwrap_or(0);
    let port = info.as_ref().map(|i| i.port).unwrap_or(0);
    let token = info.as_ref().map(|i| i.token.clone()).unwrap_or_default();

    // 1. Graceful: ask the daemon to shut itself down (marks sessions interrupted and
    //    takes every process tree with it).
    let mut how = "kill";
    if port != 0 && !token.is_empty() && post_shutdown(port, &token) {
        how = "graceful";
    }

    // 2. Wait for the daemon to release its lock (i.e. to exit).
    if wait_until_stopped(&data_dir, Duration::from_secs(6)) {
        println!("dflowd: stopped ({how})  pid={pid}");
        return Ok(0);
    }

    // 3. Fallback: kill the process tree by pid (the Job Object also reaps children).
    if pid != 0 {
        kill_pid_tree(pid);
        if wait_until_stopped(&data_dir, Duration::from_secs(3)) {
            println!("dflowd: killed  pid={pid} (graceful shutdown timed out)");
            return Ok(0);
        }
    }
    println!("dflowd: FAILED to stop  pid={pid}");
    Ok(1)
}

/// `dflowd --pair`: ask the running daemon (over loopback, with the root token) to
/// enable its LAN listener and mint a phone pairing, then print the pairing URL and an
/// ASCII QR so a phone can pair from the terminal tonight (`security.md` / QR pairing).
/// Returns the process exit code (0 paired, `EXIT_NOT_RUNNING` if nothing was running).
pub fn pair() -> Result<i32> {
    let data_dir = DataDir::resolve().context("resolving data dir")?;
    if !daemon_running(&data_dir) {
        println!("dflowd: not running; start the daemon first, then re-run `dflowd --pair`");
        return Ok(EXIT_NOT_RUNNING);
    }
    let info = read_runtime(&data_dir);
    let port = info.as_ref().map(|i| i.port).unwrap_or(0);
    let token = info.as_ref().map(|i| i.token.clone()).unwrap_or_default();
    if port == 0 || token.is_empty() {
        println!("dflowd: cannot pair (no runtime port/token); is the daemon fully started?");
        return Ok(1);
    }
    let body = match post_pair(port, &token) {
        Some(b) => b,
        None => {
            println!("dflowd: pairing request failed (could not reach the daemon on loopback)");
            return Ok(1);
        }
    };
    let pairing: serde_json::Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(_) => {
            println!("dflowd: pairing response was not valid JSON:\n{body}");
            return Ok(1);
        }
    };
    let pair_url = pairing["pair_url"].as_str().unwrap_or("");
    let ws_url = pairing["payload"]["url"].as_str().unwrap_or("");
    let token_id = pairing["token_id"].as_str().unwrap_or("");
    if pair_url.is_empty() {
        println!("dflowd: pairing response missing a URL:\n{body}");
        return Ok(1);
    }

    println!("\nScan this QR with your phone camera to pair DapperFlow (LAN, no TLS):\n");
    match render_qr(pair_url) {
        Some(qr) => println!("{qr}"),
        None => println!("(QR rendering unavailable; use the URL below)\n"),
    }
    println!("Pairing URL (open on the phone if the QR will not scan):\n  {pair_url}\n");
    println!("Phone WS endpoint: {ws_url}");
    println!("Revoke this device later with: daemon.lan.revoke {{ token_id: \"{token_id}\" }}");
    println!(
        "\nHeads up: LAN access is plain HTTP/WS (no TLS). The capability token is the gate; \
         only enable this on a trusted network."
    );
    Ok(0)
}

/// POST `/pair` with the root bearer token over loopback; return the JSON body on 200.
fn post_pair(port: u16, token: &str) -> Option<String> {
    let req = format!(
        "POST /pair HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {token}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    );
    let mut stream = TcpStream::connect(("127.0.0.1", port)).ok()?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    stream.write_all(req.as_bytes()).ok()?;
    let mut resp = String::new();
    stream.read_to_string(&mut resp).ok()?;
    // Split headers from body at the blank line; require a 200 status.
    let status_ok = resp.lines().next().map(|l| l.contains("200")).unwrap_or(false);
    if !status_ok {
        return None;
    }
    let body = resp.split("\r\n\r\n").nth(1).unwrap_or("").trim().to_string();
    if body.is_empty() {
        None
    } else {
        Some(body)
    }
}

/// Render `data` as a Unicode QR block for the terminal (with a quiet zone so scanners
/// lock on). Returns `None` if the payload is too large to encode.
fn render_qr(data: &str) -> Option<String> {
    use qrcode::render::unicode;
    use qrcode::QrCode;
    let code = QrCode::new(data.as_bytes()).ok()?;
    let rendered = code
        .render::<unicode::Dense1x2>()
        .quiet_zone(true)
        .dark_color(unicode::Dense1x2::Light)
        .light_color(unicode::Dense1x2::Dark)
        .build();
    Some(rendered)
}

/// Whether a daemon holds the single-instance lock for this data dir.
fn daemon_running(data_dir: &DataDir) -> bool {
    let path = data_dir.lock_file();
    let file = match OpenOptions::new().read(true).write(true).create(true).truncate(false).open(&path)
    {
        Ok(f) => f,
        Err(_) => return false,
    };
    match file.try_lock_exclusive() {
        // We acquired it, so nobody else holds it: not running. Release immediately.
        Ok(()) => {
            let _ = FileExt::unlock(&file);
            false
        }
        // Held by the live daemon.
        Err(_) => true,
    }
}

/// Poll until the daemon has released its lock, or the timeout elapses.
fn wait_until_stopped(data_dir: &DataDir, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if !daemon_running(data_dir) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(150));
    }
    !daemon_running(data_dir)
}

/// Read and parse the published runtime descriptor, if present.
fn read_runtime(data_dir: &DataDir) -> Option<RuntimeInfo> {
    let text = std::fs::read_to_string(data_dir.runtime_file()).ok()?;
    serde_json::from_str(&text).ok()
}

/// POST `/shutdown` with the root bearer token over loopback. Returns whether the
/// daemon accepted it (HTTP 200).
fn post_shutdown(port: u16, token: &str) -> bool {
    let req = format!(
        "POST /shutdown HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {token}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    );
    let mut stream = match TcpStream::connect(("127.0.0.1", port)) {
        Ok(s) => s,
        Err(_) => return false,
    };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(3)));
    if stream.write_all(req.as_bytes()).is_err() {
        return false;
    }
    let mut resp = String::new();
    let _ = stream.read_to_string(&mut resp);
    resp.lines().next().map(|l| l.contains("200")).unwrap_or(false)
}

/// Kill a process tree by pid (Windows `taskkill /T /F`).
fn kill_pid_tree(pid: u32) {
    let _ = Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .output();
}
