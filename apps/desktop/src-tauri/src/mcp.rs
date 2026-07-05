//! dflow-mcp mount helpers for the Concertmaster panel.
//!
//! The panel needs two things the webview cannot do itself: run the real
//! `dflow-mcp install <harness>` helper (`the design notes`) to produce or
//! apply the mount config, and look for an existing mount in the harness's own config
//! files so it can say "already mounted" honestly. Both live here, in the shell, next
//! to the daemon locator they mirror. Neither touches `dflowd`: install is a local
//! `dflow-mcp` invocation, detection is a read of known files.

use std::path::PathBuf;
use std::process::Command;

use serde::Serialize;

const MCP_EXE: &str = "dflow-mcp.exe";

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct InstallHint {
    pub ok: bool,
    pub harness: String,
    pub exe_path: Option<String>,
    /// The helper's stdout: the one-liner plus the config block (or an error note).
    pub text: String,
    pub wrote: bool,
    pub error: Option<String>,
}

#[derive(Serialize, Clone)]
pub struct McpDetect {
    pub mounted: bool,
    pub location: Option<String>,
    pub checked: Vec<String>,
    /// False for harnesses whose mount config we cannot inspect.
    pub detectable: bool,
    pub error: Option<String>,
}

/// Run `dflow-mcp install <harness> [--write]` and return its output. `cwd` scopes a
/// project-file write (`--write` merges into the harness file under that directory).
#[tauri::command]
pub fn mcp_install_hint(harness: String, cwd: Option<String>, write: bool) -> Result<InstallHint, String> {
    let exe = match find_mcp() {
        Some(p) => p,
        None => {
            return Ok(InstallHint {
                ok: false,
                harness,
                exe_path: None,
                text: String::new(),
                wrote: false,
                error: Some(
                    "could not locate dflow-mcp; build it with `cargo build -p dflow-mcp` at the repo root, or set DFLOW_MCP_PATH".to_string(),
                ),
            });
        }
    };

    let mut cmd = Command::new(&exe);
    cmd.arg("install").arg(&harness);
    if write {
        cmd.arg("--write");
    }
    if let Some(dir) = cwd.as_ref().filter(|d| !d.is_empty()) {
        cmd.current_dir(dir);
    }
    no_window(&mut cmd);

    let out = cmd
        .output()
        .map_err(|e| format!("failed to run dflow-mcp install: {e}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    let text = if stdout.trim().is_empty() { stderr.clone() } else { stdout };
    let ok = out.status.success();

    Ok(InstallHint {
        ok,
        harness,
        exe_path: Some(exe.display().to_string()),
        text,
        wrote: write && ok,
        error: if ok { None } else { Some(stderr.trim().to_string()).filter(|s| !s.is_empty()) },
    })
}

/// Look for an existing dflow mount in the harness's known config locations.
/// Best-effort and honest: some harnesses expose no inspectable mount config, in which
/// case `detectable` is false and the panel offers to (re)mount without claiming a state.
#[tauri::command]
pub fn mcp_detect(harness: String, cwd: Option<String>) -> Result<McpDetect, String> {
    let mut checked: Vec<String> = Vec::new();
    let cwd_dir = cwd.as_ref().filter(|d| !d.is_empty()).map(PathBuf::from);

    match harness.as_str() {
        "claude" => {
            // Project-scoped .mcp.json, then the user-global ~/.claude.json.
            if let Some(dir) = &cwd_dir {
                let f = dir.join(".mcp.json");
                if let Some(hit) = check_json_for_dflow(&f, &mut checked) {
                    return Ok(mounted(hit, checked));
                }
            }
            if let Some(home) = home_dir() {
                let f = home.join(".claude.json");
                if let Some(hit) = check_json_for_dflow(&f, &mut checked) {
                    return Ok(mounted(hit, checked));
                }
            }
            Ok(not_mounted(checked, true))
        }
        "codex" => {
            if let Some(home) = home_dir() {
                let f = home.join(".codex").join("config.toml");
                if let Some(hit) = check_text_for_dflow(&f, "[mcp_servers.dflow]", &mut checked) {
                    return Ok(mounted(hit, checked));
                }
            }
            Ok(not_mounted(checked, true))
        }
        "opencode" => {
            if let Some(dir) = &cwd_dir {
                let f = dir.join("opencode.json");
                if let Some(hit) = check_json_for_dflow(&f, &mut checked) {
                    return Ok(mounted(hit, checked));
                }
            }
            Ok(not_mounted(checked, true))
        }
        // Unknown families expose no config we can read; say so rather than guess.
        _ => Ok(not_mounted(checked, false)),
    }
}

fn mounted(location: String, checked: Vec<String>) -> McpDetect {
    McpDetect { mounted: true, location: Some(location), checked, detectable: true, error: None }
}
fn not_mounted(checked: Vec<String>, detectable: bool) -> McpDetect {
    McpDetect { mounted: false, location: None, checked, detectable, error: None }
}

/// Parse a JSON config and look for a `dflow` key under an mcp servers object. Records
/// the path in `checked`. Returns the file path when a mount is found.
fn check_json_for_dflow(path: &std::path::Path, checked: &mut Vec<String>) -> Option<String> {
    checked.push(path.display().to_string());
    let text = std::fs::read_to_string(path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    // Claude / opencode both nest under a servers map ("mcpServers" or "mcp"); accept a
    // top-level "dflow" too, to stay robust to shape drift.
    let has = json_has_dflow(&value, "mcpServers")
        || json_has_dflow(&value, "mcp")
        || value.get("dflow").is_some();
    if has {
        Some(path.display().to_string())
    } else {
        None
    }
}

fn json_has_dflow(value: &serde_json::Value, key: &str) -> bool {
    value
        .get(key)
        .and_then(|v| v.as_object())
        .map(|o| o.contains_key("dflow"))
        .unwrap_or(false)
}

/// Substring check for a marker line (TOML has no cheap serde read here). Honest and
/// documented: it confirms the section exists, not that it points at this exe.
fn check_text_for_dflow(path: &std::path::Path, marker: &str, checked: &mut Vec<String>) -> Option<String> {
    checked.push(path.display().to_string());
    let text = std::fs::read_to_string(path).ok()?;
    if text.contains(marker) {
        Some(path.display().to_string())
    } else {
        None
    }
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
}

/// Resolve the dflow-mcp binary the same way the shell resolves dflowd: an override,
/// alongside the app exe, or the workspace target tree in dev.
fn find_mcp() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("DFLOW_MCP_PATH").map(PathBuf::from) {
        if p.exists() {
            return Some(p);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join(MCP_EXE);
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }
    let starts = [
        std::env::current_dir().ok(),
        std::env::current_exe().ok().and_then(|e| e.parent().map(PathBuf::from)),
    ];
    for start in starts.into_iter().flatten() {
        let mut dir: Option<&std::path::Path> = Some(start.as_path());
        while let Some(d) = dir {
            for profile in ["debug", "release"] {
                let candidate = d.join("target").join(profile).join(MCP_EXE);
                if candidate.exists() {
                    return Some(candidate);
                }
            }
            dir = d.parent();
        }
    }
    None
}

/// Suppress the console flash when spawning a child on Windows.
#[cfg(windows)]
fn no_window(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(CREATE_NO_WINDOW);
}
#[cfg(not(windows))]
fn no_window(_cmd: &mut Command) {}
