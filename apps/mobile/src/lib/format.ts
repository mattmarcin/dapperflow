// Compact time formatting for the live readouts. Copied from the desktop's lib/format.ts
// (M6 debt; unifies into client-core at M7).

/** Compact elapsed duration, e.g. "3s", "4m", "1h 20m", "2d 3h". */
export function elapsed(sinceMs: number, nowMs: number = Date.now()): string {
  const s = Math.max(0, Math.floor((nowMs - sinceMs) / 1000));
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  if (m < 60) {
    const rem = s % 60;
    return rem && m < 5 ? `${m}m ${rem}s` : `${m}m`;
  }
  const h = Math.floor(m / 60);
  if (h < 24) {
    const rem = m % 60;
    return rem ? `${h}h ${rem}m` : `${h}h`;
  }
  const d = Math.floor(h / 24);
  const rem = h % 24;
  return rem ? `${d}d ${rem}h` : `${d}d`;
}

/** Relative "time ago" for queue items and previews. */
export function timeAgo(tsMs: number, nowMs: number = Date.now()): string {
  const s = Math.max(0, Math.floor((nowMs - tsMs) / 1000));
  if (s < 45) return "just now";
  const m = Math.round(s / 60);
  if (m < 60) return `${m}m ago`;
  const h = Math.round(m / 60);
  if (h < 24) return `${h}h ago`;
  const d = Math.round(h / 24);
  if (d < 30) return `${d}d ago`;
  const mo = Math.round(d / 30);
  return `${mo}mo ago`;
}

/** Clock time, e.g. "14:07". */
export function clockTime(tsMs: number): string {
  const d = new Date(tsMs);
  return `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
}

/** A concise line from a first prompt for a session title fallback. */
export function titleFromPrompt(prompt: string, maxWords = 6, maxChars = 48): string {
  const clean = prompt.replace(/\s+/g, " ").trim();
  if (!clean) return "";
  let t = clean.split(" ").slice(0, maxWords).join(" ");
  if (t.length > maxChars) t = `${t.slice(0, maxChars).trimEnd()}…`;
  return t.replace(/[.,;:]+$/, "");
}
