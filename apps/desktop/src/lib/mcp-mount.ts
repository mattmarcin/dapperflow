// The dflow-mcp mount bridge. Confirming the mount is deliberately not magic: the panel
// asks the Tauri shell to run `dflow-mcp install <harness>` (the real install helper,
// the design notes) and to look for an existing mount where that is detectable.
// When the shell or the binary is unavailable (browser dev, an un-built dflow-mcp), we
// degrade to honest manual instructions rather than pretending (product.md principle 1:
// degrade cleanly; deliverable 5: explain exactly what to run).

import { isTauri } from "./tauri";

export type MountHarness = "claude" | "codex" | "opencode" | string;

export interface InstallHint {
  ok: boolean;
  harness: string;
  /** Resolved absolute path to the dflow-mcp binary, when the shell found it. */
  exePath?: string | null;
  /** The install helper's output: the one-liner plus the config block. */
  text: string;
  /** True when called with write and the helper merged the mount config. */
  wrote?: boolean;
  /** Present when the binary could not be located; text then holds manual guidance. */
  error?: string | null;
}

export interface McpDetect {
  /** True when a dflow mount was found in a known location for this harness. */
  mounted: boolean;
  /** Where it was found (a file path), when mounted. */
  location?: string | null;
  /** The locations inspected, for an honest "checked here" note. */
  checked: string[];
  /** Detection is best-effort; some harnesses expose no inspectable mount config. */
  detectable: boolean;
  error?: string | null;
}

// Manual instructions per harness, used when the shell cannot run the real helper. The
// command spelling mirrors `dflow-mcp install` (phase6-mcp.md): a `<harness> mcp add`
// one-liner pointing at `dflow-mcp serve`. `exe` falls back to the bare name on PATH.
export function manualMountText(harness: MountHarness, exe = "dflow-mcp"): string {
  switch (harness) {
    case "claude":
      return [
        "# Mount dflow-mcp into Claude Code (once):",
        `claude mcp add dflow -- ${exe} serve`,
        "",
        "# Then launch this Concertmaster - the mount is picked up automatically.",
      ].join("\n");
    case "codex":
      return [
        "# Mount dflow-mcp into Codex (once):",
        `codex mcp add dflow -- ${exe} serve`,
      ].join("\n");
    case "opencode":
      return [
        "# Add to opencode.json under \"mcp\":",
        `"dflow": { "type": "local", "command": ["${exe}", "serve"] }`,
      ].join("\n");
    default:
      return [
        "# Mount dflow-mcp as a stdio MCP server in your harness:",
        `command: ${exe}`,
        "args: [serve]",
        "transport: stdio",
      ].join("\n");
  }
}

/**
 * Ask the shell to produce (or apply) the mount config for `harness`. `write: true`
 * merges the config into the harness's own file via the helper; otherwise it only
 * prints. Falls back to manual text off-Tauri or when the binary is missing.
 */
export async function mcpInstallHint(
  harness: MountHarness,
  opts?: { cwd?: string | null; write?: boolean },
): Promise<InstallHint> {
  if (!isTauri()) {
    return {
      ok: false,
      harness,
      text: manualMountText(harness),
      error: "Run under the desktop app to auto-mount; here is the manual command.",
    };
  }
  try {
    const { invoke } = await import("@tauri-apps/api/core");
    const res = await invoke<InstallHint>("mcp_install_hint", {
      harness,
      cwd: opts?.cwd ?? null,
      write: opts?.write ?? false,
    });
    return res;
  } catch (e) {
    // The command fails when dflow-mcp cannot be located; surface the manual path.
    return {
      ok: false,
      harness,
      text: manualMountText(harness),
      error: messageOf(e),
    };
  }
}

/** Look for an existing dflow mount in the harness's known config locations. */
export async function mcpDetect(
  harness: MountHarness,
  opts?: { cwd?: string | null },
): Promise<McpDetect> {
  if (!isTauri()) {
    return { mounted: false, checked: [], detectable: false, error: "detection needs the desktop app" };
  }
  try {
    const { invoke } = await import("@tauri-apps/api/core");
    return await invoke<McpDetect>("mcp_detect", { harness, cwd: opts?.cwd ?? null });
  } catch (e) {
    return { mounted: false, checked: [], detectable: false, error: messageOf(e) };
  }
}

function messageOf(e: unknown): string {
  if (e instanceof Error) return e.message;
  if (typeof e === "string") return e;
  if (e && typeof e === "object" && "message" in e) return String((e as { message: unknown }).message);
  return String(e);
}
