// The one-click invariant on the Concertmaster's mouth (product.md). dflow-mcp
// instructs the harness to repeat every entity id as a bracketed `[kind:ULID]`
// token (the design notes), so the panel can turn each mention into a
// one-click deep link. This module is the pure parsing half: it finds tokens in a
// block of scraped terminal text. Resolution (token -> label -> navigation) lives
// at the call site, where the store's entities are in reach.
//
// Fidelity is honest and documented (the design notes): this reads the
// visible xterm buffer, never the PTY stream, so a token wrapped across a hard line
// break by the terminal renderer is not reassembled, and a token scrolled out of the
// captured window ages out. Both are acceptable: the tokens the user just saw are the
// ones worth linking.

export type DeepLinkKind = "card" | "session" | "project" | "needs_you" | "note";

export const DEEP_LINK_KINDS: DeepLinkKind[] = [
  "card",
  "session",
  "project",
  "needs_you",
  "note",
];

export interface DeepLinkToken {
  kind: DeepLinkKind;
  /** The 26-char ULID between the colon and the closing bracket. */
  id: string;
  /** The token exactly as it appeared, e.g. "[card:01KWRF...]". */
  raw: string;
}

// A ULID is 26 Crockford base32 chars. We match tolerantly (any 26 alphanumerics)
// so an almost-ULID still surfaces; the resolver decides whether it names a real
// entity. Case-insensitive: the daemon emits upper, but harnesses sometimes echo
// lower. The kinds are pinned to the five dflow-mcp emits.
const TOKEN_RE = /\[(card|session|project|needs_you|note):([0-9A-Za-z]{26})\]/gi;

function normalizeKind(kind: string): DeepLinkKind {
  return kind.toLowerCase() as DeepLinkKind;
}

/** Every token in `text`, in reading order, duplicates included. */
export function parseDeepLinkTokens(text: string): DeepLinkToken[] {
  const out: DeepLinkToken[] = [];
  if (!text) return out;
  TOKEN_RE.lastIndex = 0;
  let m: RegExpExecArray | null;
  while ((m = TOKEN_RE.exec(text)) !== null) {
    out.push({ kind: normalizeKind(m[1]), id: m[2].toUpperCase(), raw: m[0] });
  }
  return out;
}

/**
 * The most recently mentioned distinct tokens, newest first, capped. "Recent" is
 * reading order in the captured buffer: later mentions win, and a re-mention floats a
 * token back to the front. This backs the compact link bar - the user cares about
 * what the Concertmaster just referred to, not the whole history.
 */
export function recentDeepLinkTokens(text: string, cap = 6): DeepLinkToken[] {
  const all = parseDeepLinkTokens(text);
  const seen = new Map<string, DeepLinkToken>();
  // Walk oldest -> newest so the last occurrence of a duplicate sets final order.
  for (const t of all) {
    const key = `${t.kind}:${t.id}`;
    seen.delete(key); // re-insert to move to the end (most recent)
    seen.set(key, t);
  }
  // Map preserves insertion order; newest is last. Reverse for newest-first, then cap.
  return Array.from(seen.values()).reverse().slice(0, cap);
}
