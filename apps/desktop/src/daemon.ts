// Obtain the daemon's loopback coordinates. In the packaged app this comes from
// the Tauri IPC command; in a plain browser (used for visual development) it falls
// back to a developer-supplied /daemon-dev.json so the same UI can be exercised
// against a running dflowd.

import { isTauri } from "./lib/tauri";

// How the app relates to the daemon for this run (`daemon-lifecycle.md` / ownership).
export type DaemonMode = "dev-external" | "prod-managed";

export interface DaemonInfo {
  port: number;
  token: string;
  // Whether this app run had to spawn the daemon (true) or attached to one that was
  // already alive (false/undefined). Undefined in browser dev, where an externally-run
  // daemon is always pre-existing. Surfaced in the status bar.
  started?: boolean;
  // Whether `port`/`token` point at a live daemon. In dev-external mode with no daemon
  // running this is false and the app shows `hint` instead of connecting. Undefined in
  // browser dev (treated as connected).
  connected?: boolean;
  // Dev-external (the app connects to an externally-run daemon) vs prod-managed (the app
  // owns and spawns a bundled daemon). Undefined in browser dev.
  mode?: DaemonMode;
  // Guidance shown when not connected in dev-external mode (e.g. "start the dev daemon").
  hint?: string;
}

export async function getDaemonInfo(): Promise<DaemonInfo> {
  if (isTauri()) {
    const { invoke } = await import("@tauri-apps/api/core");
    return invoke<DaemonInfo>("daemon_info");
  }
  const res = await fetch("/daemon-dev.json", { cache: "no-store" });
  if (!res.ok) {
    throw new Error(
      "Not running under Tauri and no /daemon-dev.json found. For browser dev, run dflowd and write its {port, token} to public/daemon-dev.json.",
    );
  }
  return (await res.json()) as DaemonInfo;
}
