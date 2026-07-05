//! `dflow-mcp install <harness>`: print (or write) the mount config that puts
//! this server on a harness's MCP list.
//!
//! Facts per the adapter capability matrix (`adapters.md` / capabilities.mcp):
//! claude, codex, and opencode all support MCP mounts; cursor and pi do not
//! advertise verified MCP support, so they fall through to the generic
//! stdio instructions.
//!
//! `--write` only ever touches project-local files in the current directory
//! (`.mcp.json`, `opencode.json`) except for codex, whose only config surface
//! is `~/.codex/config.toml`; that append is guarded against duplicates.

use std::path::{Path, PathBuf};

/// What `--write` would touch for one harness.
pub enum WritePlan {
    /// Merge `mcpServers.dflow` into `<cwd>/.mcp.json` (claude project scope).
    ClaudeProjectMcpJson,
    /// Merge `mcp.dflow` into `<cwd>/opencode.json`.
    OpencodeJson,
    /// Append an `[mcp_servers.dflow]` block to `~/.codex/config.toml`.
    CodexGlobalToml,
    /// Nothing writable; instructions only.
    None,
}

/// The rendered install output for one harness.
pub struct InstallOutput {
    pub text: String,
    pub plan: WritePlan,
}

/// The JSON server entry shared by claude-style configs.
fn server_entry(exe: &str, data_dir: Option<&str>) -> serde_json::Value {
    let mut entry = serde_json::json!({
        "type": "stdio",
        "command": exe,
        "args": ["serve"],
    });
    if let Some(dir) = data_dir {
        entry["env"] = serde_json::json!({ "DFLOW_DATA_DIR": dir });
    }
    entry
}

/// Render the mount config for `harness`. `exe` is the absolute path to this
/// binary; `data_dir` is a `DFLOW_DATA_DIR` override to embed (when the caller
/// runs with one set, the mounted server must see the same daemon).
pub fn render(harness: &str, exe: &str, data_dir: Option<&str>) -> InstallOutput {
    match harness {
        "claude" => render_claude(exe, data_dir),
        "codex" => render_codex(exe, data_dir),
        "opencode" => render_opencode(exe, data_dir),
        other => render_generic(other, exe, data_dir),
    }
}

fn render_claude(exe: &str, data_dir: Option<&str>) -> InstallOutput {
    let config = serde_json::json!({ "mcpServers": { "dflow": server_entry(exe, data_dir) } });
    let pretty = serde_json::to_string_pretty(&config).expect("static json");
    let env_flag = data_dir
        .map(|d| format!(" --env DFLOW_DATA_DIR={d}"))
        .unwrap_or_default();
    let text = format!(
        "# claude (Claude Code)\n\
         One-liner (user scope):\n\
         \x20 claude mcp add dflow{env_flag} -- \"{exe}\" serve\n\
         Or as --mcp-config / project .mcp.json:\n{pretty}\n\
         --write merges the block above into .mcp.json in the current directory.\n"
    );
    InstallOutput { text, plan: WritePlan::ClaudeProjectMcpJson }
}

fn render_codex(exe: &str, data_dir: Option<&str>) -> InstallOutput {
    let env_block = data_dir
        .map(|d| format!("\n[mcp_servers.dflow.env]\nDFLOW_DATA_DIR = {}", toml_string(d)))
        .unwrap_or_default();
    let block = format!(
        "[mcp_servers.dflow]\ncommand = {}\nargs = [\"serve\"]{env_block}\n",
        toml_string(exe)
    );
    let text = format!(
        "# codex (Codex CLI)\n\
         One-liner:\n\
         \x20 codex mcp add dflow -- \"{exe}\" serve\n\
         Or add to ~/.codex/config.toml:\n{block}\
         --write appends the block above to ~/.codex/config.toml (skipped if a \
         [mcp_servers.dflow] entry already exists).\n"
    );
    InstallOutput { text, plan: WritePlan::CodexGlobalToml }
}

fn render_opencode(exe: &str, data_dir: Option<&str>) -> InstallOutput {
    let mut entry = serde_json::json!({
        "type": "local",
        "command": [exe, "serve"],
        "enabled": true,
    });
    if let Some(dir) = data_dir {
        entry["environment"] = serde_json::json!({ "DFLOW_DATA_DIR": dir });
    }
    let config = serde_json::json!({
        "$schema": "https://opencode.ai/config.json",
        "mcp": { "dflow": entry },
    });
    let pretty = serde_json::to_string_pretty(&config).expect("static json");
    let text = format!(
        "# opencode\n\
         Add to opencode.json (project) or ~/.config/opencode/opencode.json (global):\n{pretty}\n\
         --write merges the mcp block into opencode.json in the current directory.\n"
    );
    InstallOutput { text, plan: WritePlan::OpencodeJson }
}

fn render_generic(harness: &str, exe: &str, data_dir: Option<&str>) -> InstallOutput {
    let env_line = data_dir
        .map(|d| format!("  env:       DFLOW_DATA_DIR={d}\n"))
        .unwrap_or_default();
    let text = format!(
        "# {harness} (no specific recipe; generic stdio MCP mount)\n\
         dflow-mcp is a standard MCP server over stdio. Configure your harness with:\n\
         \x20 command:   \"{exe}\"\n\
         \x20 args:      [\"serve\"]\n\
         \x20 transport: stdio\n\
         {env_line}\
         The server needs no other environment; it finds the DapperFlow daemon via \
         the runtime file under the data dir (DFLOW_DATA_DIR honored).\n\
         --write is not supported for this harness.\n"
    );
    InstallOutput { text, plan: WritePlan::None }
}

/// A TOML basic string with escapes (Windows paths carry backslashes).
fn toml_string(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Execute `--write` for the rendered plan. Returns a human summary line.
pub fn write(plan: &WritePlan, exe: &str, data_dir: Option<&str>, cwd: &Path) -> Result<String, String> {
    match plan {
        WritePlan::ClaudeProjectMcpJson => {
            let path = cwd.join(".mcp.json");
            let mut root = read_json_or_object(&path)?;
            root["mcpServers"]["dflow"] = server_entry(exe, data_dir);
            write_pretty_json(&path, &root)?;
            Ok(format!("wrote {}", path.display()))
        }
        WritePlan::OpencodeJson => {
            let path = cwd.join("opencode.json");
            let mut root = read_json_or_object(&path)?;
            if root.get("$schema").is_none() {
                root["$schema"] = serde_json::json!("https://opencode.ai/config.json");
            }
            let mut entry = serde_json::json!({
                "type": "local",
                "command": [exe, "serve"],
                "enabled": true,
            });
            if let Some(dir) = data_dir {
                entry["environment"] = serde_json::json!({ "DFLOW_DATA_DIR": dir });
            }
            root["mcp"]["dflow"] = entry;
            write_pretty_json(&path, &root)?;
            Ok(format!("wrote {}", path.display()))
        }
        WritePlan::CodexGlobalToml => {
            let home = std::env::var_os("USERPROFILE")
                .or_else(|| std::env::var_os("HOME"))
                .map(PathBuf::from)
                .ok_or_else(|| "cannot resolve the home directory".to_string())?;
            let path = home.join(".codex").join("config.toml");
            let existing = std::fs::read_to_string(&path).unwrap_or_default();
            if existing.contains("[mcp_servers.dflow]") {
                return Ok(format!("{} already has [mcp_servers.dflow]; nothing written", path.display()));
            }
            let env_block = data_dir
                .map(|d| format!("\n[mcp_servers.dflow.env]\nDFLOW_DATA_DIR = {}", toml_string(d)))
                .unwrap_or_default();
            let block = format!(
                "\n[mcp_servers.dflow]\ncommand = {}\nargs = [\"serve\"]{env_block}\n",
                toml_string(exe)
            );
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| format!("creating {}: {e}", parent.display()))?;
            }
            std::fs::write(&path, existing + &block).map_err(|e| format!("writing {}: {e}", path.display()))?;
            Ok(format!("appended [mcp_servers.dflow] to {}", path.display()))
        }
        WritePlan::None => Err("--write is not supported for this harness; copy the instructions above".into()),
    }
}

fn read_json_or_object(path: &Path) -> Result<serde_json::Value, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text).map_err(|e| format!("existing {} is not valid JSON: {e}", path.display())),
        Err(_) => Ok(serde_json::json!({})),
    }
}

fn write_pretty_json(path: &Path, value: &serde_json::Value) -> Result<(), String> {
    let pretty = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    std::fs::write(path, pretty + "\n").map_err(|e| format!("writing {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXE: &str = "C:\\bin\\dflow-mcp.exe";

    #[test]
    fn claude_config_is_valid_json_with_stdio_entry() {
        let out = render("claude", EXE, None);
        let json_part = out.text.split("--mcp-config / project .mcp.json:\n").nth(1).unwrap();
        let json_text = json_part.split("\n--write").next().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(json_text).unwrap();
        assert_eq!(parsed["mcpServers"]["dflow"]["command"], EXE);
        assert_eq!(parsed["mcpServers"]["dflow"]["args"][0], "serve");
        assert!(out.text.contains("claude mcp add dflow"));
    }

    #[test]
    fn codex_block_escapes_windows_paths() {
        let out = render("codex", EXE, Some("D:\\data dir"));
        assert!(out.text.contains("[mcp_servers.dflow]"));
        assert!(out.text.contains("command = \"C:\\\\bin\\\\dflow-mcp.exe\""));
        assert!(out.text.contains("DFLOW_DATA_DIR = \"D:\\\\data dir\""));
        assert!(out.text.contains("codex mcp add dflow"));
    }

    #[test]
    fn opencode_config_is_valid_json_local_server() {
        let out = render("opencode", EXE, Some("D:\\dd"));
        let json_text = out.text.split("opencode.json (global):\n").nth(1).unwrap();
        let json_text = json_text.split("\n--write").next().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(json_text).unwrap();
        assert_eq!(parsed["mcp"]["dflow"]["type"], "local");
        assert_eq!(parsed["mcp"]["dflow"]["command"][0], EXE);
        assert_eq!(parsed["mcp"]["dflow"]["environment"]["DFLOW_DATA_DIR"], "D:\\dd");
    }

    #[test]
    fn unknown_harness_gets_plain_stdio_instructions() {
        let out = render("mystery", EXE, None);
        assert!(out.text.contains("stdio"));
        assert!(out.text.contains(EXE));
        assert!(out.text.contains("--write is not supported"));
        assert!(matches!(out.plan, WritePlan::None));
    }

    #[test]
    fn write_merges_into_existing_mcp_json() {
        let dir = std::env::temp_dir().join(format!(
            "dflow-mcp-install-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(".mcp.json"),
            r#"{ "mcpServers": { "other": { "command": "x" } } }"#,
        )
        .unwrap();
        let summary = write(&WritePlan::ClaudeProjectMcpJson, EXE, None, &dir).unwrap();
        assert!(summary.contains(".mcp.json"));
        let merged: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join(".mcp.json")).unwrap()).unwrap();
        assert_eq!(merged["mcpServers"]["other"]["command"], "x", "existing entries survive");
        assert_eq!(merged["mcpServers"]["dflow"]["command"], EXE);
    }
}
