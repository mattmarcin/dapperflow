// Notification preferences, persisted locally (the daemon does not own client toast
// settings). Minimal by design: a master switch and a "high priority only" gate that
// maps to a Needs You score threshold.

export interface NotificationPrefs {
  enabled: boolean;
  onlyHighPriority: boolean;
}

export const DEFAULT_NOTIFICATION_PREFS: NotificationPrefs = {
  enabled: true,
  onlyHighPriority: false,
};

const KEY = "dflow.notifications";

export function loadNotificationPrefs(): NotificationPrefs {
  try {
    const raw = localStorage.getItem(KEY);
    if (!raw) return { ...DEFAULT_NOTIFICATION_PREFS };
    const parsed = JSON.parse(raw) as Partial<NotificationPrefs>;
    return {
      enabled: parsed.enabled ?? DEFAULT_NOTIFICATION_PREFS.enabled,
      onlyHighPriority: parsed.onlyHighPriority ?? DEFAULT_NOTIFICATION_PREFS.onlyHighPriority,
    };
  } catch {
    return { ...DEFAULT_NOTIFICATION_PREFS };
  }
}

export function saveNotificationPrefs(prefs: NotificationPrefs): void {
  try {
    localStorage.setItem(KEY, JSON.stringify(prefs));
  } catch {
    /* private mode / quota - preferences just do not persist */
  }
}

// The minimum Needs You score that fires a toast under the current prefs. "High
// priority only" uses the same threshold as the queue's "high" rank band (55).
export function minNotifyScore(prefs: NotificationPrefs): number {
  return prefs.onlyHighPriority ? 55 : 0;
}
