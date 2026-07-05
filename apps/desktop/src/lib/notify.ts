// Desktop notifications for Needs You arrivals. Two backends behind one API:
//
//  - Packaged app (Tauri): the tauri-plugin-notification, requested and sent through
//    @tauri-apps/plugin-notification. This is the shipping path (Cargo dep + capability
//    + pnpm package). On Windows, clicking a toast activates the app window; delivering
//    the specific card deep-link through the OS action channel is best-effort (see
//    onNotificationClick note), so the in-app Needs You queue is always the reliable
//    click-through.
//  - Browser dev (isTauri() === false): the Web Notification API, which on Windows
//    raises a real Action Center toast and delivers a reliable onclick. This is how the
//    live-daemon screenshots are captured.
//
// The two backends share one throttle and one deep-link handler so behavior is
// identical to the user.

import { isTauri } from "./tauri";

export interface NotifyDeepLink {
  cardId?: string;
  tab?: string;
}

export interface NotifyInput {
  title: string;
  body?: string;
  // Stable per-item key so the OS coalesces re-raises of the same item.
  tag?: string;
  deepLink?: NotifyDeepLink;
}

type ClickHandler = (link: NotifyDeepLink) => void;

let clickHandler: ClickHandler | null = null;

// The app registers one handler; both backends route a toast click through it. The
// store wires this to focus the window and open the resolving card workspace tab.
export function onNotificationClick(handler: ClickHandler): void {
  clickHandler = handler;
}

let permissionState: "default" | "granted" | "denied" = "default";
let tauriActionsWired = false;

// Ask the OS for permission once (idempotent). Returns whether notifications may fire.
export async function ensureNotificationPermission(): Promise<boolean> {
  if (isTauri()) {
    try {
      const plugin = await import("@tauri-apps/plugin-notification");
      let granted = await plugin.isPermissionGranted();
      if (!granted) granted = (await plugin.requestPermission()) === "granted";
      permissionState = granted ? "granted" : "denied";
      return granted;
    } catch {
      return false;
    }
  }
  if (typeof Notification === "undefined") return false;
  if (Notification.permission === "granted") {
    permissionState = "granted";
    return true;
  }
  if (Notification.permission === "denied") {
    permissionState = "denied";
    return false;
  }
  const res = await Notification.requestPermission();
  permissionState = res === "granted" ? "granted" : "denied";
  return res === "granted";
}

export function notificationPermission(): "default" | "granted" | "denied" {
  if (!isTauri() && typeof Notification !== "undefined") return Notification.permission;
  return permissionState;
}

// Fire one notification. No-ops silently if permission is not granted.
export async function notify(input: NotifyInput): Promise<void> {
  const granted = await ensureNotificationPermission();
  if (!granted) return;

  if (isTauri()) {
    try {
      const plugin = await import("@tauri-apps/plugin-notification");
      await wireTauriActions(plugin, input.deepLink);
      plugin.sendNotification({ title: input.title, body: input.body });
    } catch {
      /* notification is a courtesy, never fatal */
    }
    return;
  }

  if (typeof Notification === "undefined") return;
  try {
    const n = new Notification(input.title, { body: input.body, tag: input.tag });
    n.onclick = () => {
      window.focus();
      if (input.deepLink) clickHandler?.(input.deepLink);
      n.close();
    };
  } catch {
    /* ignore */
  }
}

// Best-effort Tauri action wiring. The plugin's onAction fires when the toast is
// activated; on Windows this is not guaranteed to carry our payload, so we keep the
// most recent deep-link and route the app's window focus through the shared handler.
let lastTauriDeepLink: NotifyDeepLink | undefined;
async function wireTauriActions(
  plugin: typeof import("@tauri-apps/plugin-notification"),
  deepLink?: NotifyDeepLink,
): Promise<void> {
  lastTauriDeepLink = deepLink;
  if (tauriActionsWired) return;
  tauriActionsWired = true;
  try {
    // onAction is available in recent plugin versions; guard defensively.
    const onAction = (plugin as unknown as { onAction?: (cb: () => void) => Promise<unknown> }).onAction;
    if (typeof onAction === "function") {
      await onAction(() => {
        if (lastTauriDeepLink) clickHandler?.(lastTauriDeepLink);
      });
    }
  } catch {
    /* the toast still shows; click-to-focus just falls back to OS default */
  }
}
