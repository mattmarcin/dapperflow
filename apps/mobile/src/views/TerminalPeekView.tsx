import { useEffect, useRef, useState } from "react";
import { useStore } from "../state/app-store";
import { Snapshot } from "../components/Snapshot";
import { StateChip } from "../components/StateChip";
import { HarnessGlyph } from "../lib/glyphs";
import { TerminalPeek } from "../data/source";
import { clockTime } from "../lib/format";
import { Sheet } from "../components/Sheet";

const POLL_MS = 2500;

// Read-only terminal peek: a styled screen snapshot, poll-refreshed. There is NO input
// path - no xterm, no keyboard, no steering. The phone capability profile grants
// read-only terminals only (security.md), and this view renders exactly that: a scrubbed
// screen capture the user can look at, never type into.
export function TerminalPeekView({ sessionId }: { sessionId: string }) {
  const store = useStore();
  const session = store.snapshot?.sessions.find((s) => s.id === sessionId);
  const card = session?.card_id ? store.cardById(session.card_id) : undefined;
  const title = card?.title ?? session?.title ?? session?.first_prompt ?? "Session";

  const [peek, setPeek] = useState<TerminalPeek | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  const alive = useRef(true);

  const load = async () => {
    setRefreshing(true);
    try {
      const p = await store.source.peekSession(sessionId);
      if (alive.current) {
        setPeek(p);
        setErr(null);
      }
    } catch (e) {
      if (alive.current) setErr(e instanceof Error ? e.message : String(e));
    } finally {
      if (alive.current) setRefreshing(false);
    }
  };

  useEffect(() => {
    alive.current = true;
    void load();
    const t = setInterval(() => void load(), POLL_MS);
    return () => {
      alive.current = false;
      clearInterval(t);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sessionId]);

  return (
    <Sheet
      title="Terminal peek"
      onClose={store.closeOverlay}
      accessory={
        <button className="btn-ghost btn-sm" onClick={() => void load()} disabled={refreshing}>
          {refreshing ? "..." : "Refresh"}
        </button>
      }
    >
      <div className="peek-head">
        <span className="peek-harness" aria-hidden>
          <HarnessGlyph harness={session?.harness ?? "shell"} size={16} />
        </span>
        <div className="peek-headtext">
          <div className="peek-title">{title}</div>
          <div className="peek-sub">
            {session ? <StateChip state={session.state} mini /> : null}
            {session?.agent ? <span className="peek-agent">{session.agent}</span> : null}
            {session?.project_name ? <span className="peek-proj">{session.project_name}</span> : null}
          </div>
        </div>
      </div>

      <div className="readonly-banner" role="note">
        <LockGlyph />
        <span>Read-only. Screen capture is scrubbed of secrets. Steering happens on the desktop.</span>
      </div>

      {err ? (
        <div className="peek-error">Could not read the snapshot: {err}</div>
      ) : peek ? (
        <>
          <div className="peek-screen">
            <Snapshot lines={peek.lines} />
          </div>
          <div className="peek-foot">
            <span>{peek.cols}×{peek.rows}</span>
            <span>captured {clockTime(peek.capturedAt)}</span>
            <span className="peek-live">{refreshing ? "refreshing" : `auto every ${POLL_MS / 1000}s`}</span>
          </div>
        </>
      ) : (
        <div className="peek-screen is-loading">Reading screen…</div>
      )}
    </Sheet>
  );
}

function LockGlyph() {
  return (
    <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
      <rect x="3.5" y="7" width="9" height="6.5" rx="1.4" />
      <path d="M5.5 7V5.2a2.5 2.5 0 015 0V7" />
    </svg>
  );
}
