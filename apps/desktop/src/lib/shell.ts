// Tauri shell bridges for the daemon-lifecycle chrome (`daemon-lifecycle.md`): the
// persisted keep-alive setting, tray status updates, and tray menu actions. In a plain
// browser (visual dev) these degrade: keep-alive falls back to localStorage, the tray
// helpers are no-ops.

import { isTauri } from "./tauri";

const KEEP_ALIVE_LS = "dflow.keepAlive";

// Read the persisted "keep agents running when I close the window" setting (default true).
export async function getKeepAlive(): Promise<boolean> {
  if (isTauri()) {
    try {
      const { invoke } = await import("@tauri-apps/api/core");
      return await invoke<boolean>("get_keep_alive");
    } catch {
      return true;
    }
  }
  const v = localStorage.getItem(KEEP_ALIVE_LS);
  return v === null ? true : v === "true";
}

// Persist the keep-alive setting.
export async function setKeepAlive(value: boolean): Promise<void> {
  if (isTauri()) {
    const { invoke } = await import("@tauri-apps/api/core");
    await invoke("set_keep_alive", { value });
    return;
  }
  localStorage.setItem(KEEP_ALIVE_LS, String(value));
}

// Update the tray's daemon status line. No-op outside Tauri or when there is no tray.
export async function setTrayStatus(text: string): Promise<void> {
  if (!isTauri()) return;
  try {
    const { invoke } = await import("@tauri-apps/api/core");
    await invoke("set_tray_status", { text });
  } catch {
    /* no tray on this platform */
  }
}

export type TrayAction = "stop-daemon" | "restart-daemon";

// Subscribe to tray menu actions (Stop/Restart). Returns an unlisten function. No-op
// outside Tauri.
export async function onTrayAction(handler: (action: TrayAction) => void): Promise<() => void> {
  if (!isTauri()) return () => {};
  const { listen } = await import("@tauri-apps/api/event");
  const unStop = await listen("tray://stop-daemon", () => handler("stop-daemon"));
  const unRestart = await listen("tray://restart-daemon", () => handler("restart-daemon"));
  return () => {
    unStop();
    unRestart();
  };
}
