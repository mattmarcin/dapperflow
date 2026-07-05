// Small formatting helpers for the board's live readouts.

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

/** Relative "time ago" for timeline entries and session previews. */
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

/** Clock time for timeline stamps, e.g. "14:07". */
export function clockTime(tsMs: number): string {
  const d = new Date(tsMs);
  return `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
}

/**
 * A concise session title from a first prompt: the first few meaningful words, e.g.
 * "audit the fb-manager repo". Collapses whitespace, keeps natural case, trims a
 * dangling trailing punctuation, and ellipsizes past the char cap.
 */
export function titleFromPrompt(prompt: string, maxWords = 6, maxChars = 52): string {
  const clean = prompt.replace(/\s+/g, " ").trim();
  if (!clean) return "";
  let t = clean.split(" ").slice(0, maxWords).join(" ");
  if (t.length > maxChars) t = `${t.slice(0, maxChars).trimEnd()}…`;
  return t.replace(/[.,;:]+$/, "");
}

/**
 * The default (generated) session title: a card's title when card-linked, else a
 * concise line from the first prompt, else the caller's fallback (harness/launcher
 * label). A user rename always wins and is applied before this is ever consulted.
 */
export function deriveSessionTitle(opts: {
  firstPrompt?: string | null;
  cardTitle?: string | null;
  fallback: string;
}): string {
  const card = opts.cardTitle?.trim();
  if (card) return card;
  const prompt = opts.firstPrompt?.trim();
  if (prompt) {
    const t = titleFromPrompt(prompt);
    if (t) return t;
  }
  return opts.fallback;
}
