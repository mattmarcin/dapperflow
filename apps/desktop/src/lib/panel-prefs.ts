// Concertmaster panel preferences (open state + docked width), persisted to
// localStorage so the panel opens where the user left it on every view and across
// restarts (deliverable 1: persists open/closed + width).

export interface PanelPrefs {
  open: boolean;
  width: number;
}

export const PANEL_MIN_WIDTH = 340;
export const PANEL_MAX_WIDTH = 760;
export const PANEL_DEFAULT_WIDTH = 432;

const KEY = "dflow.concertmaster.panel";

export function clampPanelWidth(width: number): number {
  if (Number.isNaN(width)) return PANEL_DEFAULT_WIDTH;
  return Math.min(PANEL_MAX_WIDTH, Math.max(PANEL_MIN_WIDTH, Math.round(width)));
}

export function loadPanelPrefs(): PanelPrefs {
  try {
    const raw = localStorage.getItem(KEY);
    if (!raw) return { open: false, width: PANEL_DEFAULT_WIDTH };
    const parsed = JSON.parse(raw) as Partial<PanelPrefs>;
    return {
      open: !!parsed.open,
      width: clampPanelWidth(typeof parsed.width === "number" ? parsed.width : PANEL_DEFAULT_WIDTH),
    };
  } catch {
    return { open: false, width: PANEL_DEFAULT_WIDTH };
  }
}

export function savePanelPrefs(prefs: PanelPrefs): void {
  try {
    localStorage.setItem(KEY, JSON.stringify(prefs));
  } catch {
    /* storage disabled; the panel still works for the session */
  }
}
