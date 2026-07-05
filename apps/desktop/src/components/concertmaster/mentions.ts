// The Concertmaster's mouth, resolved. Two halves:
//  1. useScrapedTokens - polls the visible terminal buffer (or the demo transcript) and
//     returns the recently mentioned [kind:ULID] tokens, newest first.
//  2. resolveMention - turns one token into a label + a one-click navigation, using the
//     live store so a card renamed after it was mentioned still reads correctly.
//
// Fidelity is honest (the design notes): scraping the rendered buffer means a
// token the terminal wrapped across a line is not reassembled, and tokens that scrolled
// out of the captured window age out of the bar. Both are acceptable - the bar is for
// what the user just saw.

import { useEffect, useState } from "react";
import { useTerminalPool } from "../../state/terminal-pool";
import { ConcertmasterSession } from "../../model";
import { DeepLinkToken, recentDeepLinkTokens } from "../../lib/deep-links";
import { StoreValue } from "../../state/store";
import { harnessLabel } from "../../lib/glyphs";

const POLL_MS = 1200;
const BAR_CAP = 6;

function sameTokens(a: DeepLinkToken[], b: DeepLinkToken[]): boolean {
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) if (a[i].raw !== b[i].raw) return false;
  return true;
}

/**
 * Poll the Concertmaster terminal's visible buffer for deep-link tokens. Live sessions
 * re-scan on an interval; a demo transcript is static and scans once. Returns the newest
 * distinct tokens for the link bar.
 */
export function useScrapedTokens(
  cm: ConcertmasterSession | null,
  demoText: string,
): DeepLinkToken[] {
  const pool = useTerminalPool();
  const [tokens, setTokens] = useState<DeepLinkToken[]>([]);

  useEffect(() => {
    if (!cm) {
      setTokens([]);
      return;
    }
    const scan = () => {
      const text = cm.demo ? demoText : pool.readTerminal(cm.sessionId) ?? "";
      const next = recentDeepLinkTokens(text, BAR_CAP);
      setTokens((prev) => (sameTokens(prev, next) ? prev : next));
    };
    scan();
    if (cm.demo) return;
    const iv = window.setInterval(scan, POLL_MS);
    return () => window.clearInterval(iv);
  }, [cm, demoText, pool]);

  return tokens;
}

export interface ResolvedMention {
  token: DeepLinkToken;
  kind: DeepLinkToken["kind"];
  label: string;
  /** Resolves to a real entity in the current store snapshot. */
  known: boolean;
  /** Has a one-click navigation target. */
  navigable: boolean;
  onClick?: () => void;
}

function truncate(s: string, n = 34): string {
  return s.length > n ? `${s.slice(0, n - 1)}…` : s;
}

function shortId(id: string): string {
  return id.slice(-6);
}

/**
 * Resolve one token to a label and a navigation (the one-click invariant on the
 * Concertmaster's mouth). Unknown ids still render, but muted and non-navigable - honest
 * about what the panel can and cannot reach.
 */
export function resolveMention(token: DeepLinkToken, store: StoreValue): ResolvedMention {
  const base = { token, kind: token.kind, known: false, navigable: false } as const;

  switch (token.kind) {
    case "card": {
      const card = store.cards.find((c) => c.id === token.id);
      if (!card) return { ...base, label: `card ${shortId(token.id)}` };
      return {
        ...base,
        known: true,
        navigable: true,
        label: truncate(card.title),
        onClick: () => {
          store.setPanelOpen(true);
          store.openCard(card.id, "terminal");
        },
      };
    }
    case "session": {
      // A fleet session (open its card), a cardless launch (open the session), or the
      // Concertmaster itself (already here - just reveal the panel).
      const session = store.sessions.find((s) => s.id === token.id);
      if (session) {
        const label = session.title ?? session.agent ?? harnessLabel(session.harness);
        // The one-click invariant extends to the Concertmaster's mouth (product.md): a
        // carded session opens its card terminal, a cardless one opens its session view.
        const cardId = session.card_id;
        return {
          ...base,
          known: true,
          navigable: true,
          label: truncate(label),
          onClick: cardId
            ? () => store.openCard(cardId, "terminal")
            : () => store.openSession(session.id),
        };
      }
      const launch = store.launches.find((l) => l.sessionId === token.id);
      if (launch) {
        return {
          ...base,
          known: true,
          navigable: true,
          label: truncate(launch.title ?? launch.agent),
          onClick: () => store.openSession(launch.sessionId),
        };
      }
      if (store.concertmaster?.sessionId === token.id) {
        return {
          ...base,
          known: true,
          navigable: true,
          label: "Concertmaster",
          onClick: () => store.focusConcertmaster(),
        };
      }
      return { ...base, label: `session ${shortId(token.id)}` };
    }
    case "project": {
      const project = store.projects.find((p) => p.id === token.id);
      if (!project) return { ...base, label: `project ${shortId(token.id)}` };
      return {
        ...base,
        known: true,
        navigable: true,
        label: truncate(project.name),
        onClick: () => {
          store.setView("board");
          store.setFilterProject(project.id);
        },
      };
    }
    case "needs_you": {
      const item = store.needsYou.find((n) => n.id === token.id || n.dedupe_key === token.id);
      if (!item) return { ...base, label: `needs you ${shortId(token.id)}` };
      return {
        ...base,
        known: true,
        navigable: true,
        label: "needs you",
        onClick: () => store.openNeedsYou(item),
      };
    }
    case "note":
      // Knowledge notes have no dedicated UI surface yet; recognized but not navigable.
      return { ...base, label: `note ${shortId(token.id)}` };
    default:
      return { ...base, label: `${token.kind} ${shortId(token.id)}` };
  }
}
