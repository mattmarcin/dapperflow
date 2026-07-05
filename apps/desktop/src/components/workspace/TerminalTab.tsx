import { useEffect, useRef, useState } from "react";
import { useStore } from "../../state/store";
import { Card, Harness, Session } from "../../model";
import { HarnessGlyph, harnessLabel } from "../../lib/glyphs";
import { isLive } from "../../lib/session-state";
import { TerminalSlot } from "../../state/terminal-pool";
import { useContextMenu } from "../ContextMenu";
import { useTerminalPool } from "../../state/terminal-pool";
import { DispatchAffordance } from "./DispatchAffordance";
import { isGrantPending } from "../../lib/recipes";

interface Props {
  card: Card;
  sessions: Session[]; // the card's fixture/board sessions
}

function tabLabel(binding: { harness: string; title?: string; sessionId: string }): string {
  return binding.title ?? `${harnessLabel(binding.harness)} ${binding.sessionId.slice(-4).toLowerCase()}`;
}

export function TerminalTab({ card, sessions }: Props) {
  const store = useStore();
  const { openMenu } = useContextMenu();
  const pool = useTerminalPool();
  const bindings = store.terminalsFor(card.id);
  const daemonReady = store.daemon === "connected";
  const fixtureHarness = (sessions.find((s) => isLive(s.state))?.harness as Harness) ?? "claude";
  const hasFixtureLive = sessions.some((s) => isLive(s.state));

  const [active, setActive] = useState<string | null>(bindings[0]?.sessionId ?? null);
  const [busy, setBusy] = useState(false);
  const [renaming, setRenaming] = useState<string | null>(null);
  const [renameDraft, setRenameDraft] = useState("");
  const autoStarted = useRef(false);

  // Closing a terminal kills the daemon session and evicts it from the pool.
  const closeTerminal = (sessionId: string) => {
    store.closeTerminal(card.id, sessionId);
    pool.evictTerminal(sessionId);
  };

  const start = async (harness: Harness, opts?: { dispatch?: boolean }) => {
    if (busy) return;
    setBusy(true);
    try {
      if (opts?.dispatch) {
        // A true dispatch: advance the card through the protocol (lane move to
        // Performing, dispatched/moved events) before opening its terminal.
        await store.dispatch({ card_id: card.id, harness });
      }
      const sid = await store.startTerminal(card.id, harness);
      setActive(sid);
    } catch (e) {
      // A dispatch parked behind the recipe consent modal is not a failure: the
      // modal is the continuation, so no toast.
      if (!isGrantPending(e)) {
        store.flash(String(e instanceof Error ? e.message : e), { tone: "danger" });
      }
    } finally {
      setBusy(false);
    }
  };

  // Honor the one-click-to-terminal invariant: a card that already wears a live
  // session strip should show a live terminal the moment its workspace opens.
  useEffect(() => {
    if (autoStarted.current) return;
    if (bindings.length === 0 && hasFixtureLive && daemonReady) {
      autoStarted.current = true;
      void start(fixtureHarness);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [bindings.length, hasFixtureLive, daemonReady]);

  // Keep an active tab selected as bindings change.
  useEffect(() => {
    if (bindings.length === 0) {
      if (active !== null) setActive(null);
    } else if (!bindings.some((b) => b.sessionId === active)) {
      setActive(bindings[bindings.length - 1].sessionId);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [bindings]);

  const startRename = (sessionId: string) => {
    const b = bindings.find((x) => x.sessionId === sessionId);
    if (!b) return;
    setRenaming(sessionId);
    setRenameDraft(tabLabel(b));
  };

  const openTabMenu = (e: React.MouseEvent, sessionId: string) => {
    openMenu(e, [
      { id: "rename", label: "Rename", onSelect: () => startRename(sessionId) },
      { id: "close", label: "Close", danger: true, separatorBefore: true, onSelect: () => closeTerminal(sessionId) },
    ]);
  };

  const commitRename = (sessionId: string) => {
    const t = renameDraft.trim();
    if (t) store.renameTerminal(card.id, sessionId, t);
    setRenaming(null);
    setRenameDraft("");
  };

  // --- No live terminal yet: dispatch affordance / offline state -----------
  if (bindings.length === 0) {
    if (!daemonReady) {
      return (
        <div className="term-panel is-empty">
          <DispatchAffordance
            title="Live terminals need the daemon"
            subtitle="Start dflowd, then choose a harness to open a real terminal for this card."
            cta="Daemon offline"
            disabled
            onGo={() => undefined}
          />
        </div>
      );
    }
    return (
      <div className="term-panel is-empty">
        <DispatchAffordance
          title={hasFixtureLive ? "Attach a live terminal" : "Dispatch this card"}
          subtitle={
            hasFixtureLive
              ? "This card has an agent working. Open its live terminal to watch and steer it."
              : "Pick a harness and go. A worktree is leased and the session opens right here."
          }
          cta={hasFixtureLive ? "Open terminal" : "Dispatch"}
          busy={busy}
          onGo={(harness) => start(harness, { dispatch: !hasFixtureLive })}
        />
      </div>
    );
  }

  const client = store.client;

  return (
    <div className="term-panel">
      <div className="term-tabbar" role="tablist" aria-label="Terminals">
        {bindings.map((b) => {
          const isActive = b.sessionId === active;
          return (
            <div
              key={b.sessionId}
              role="tab"
              aria-selected={isActive}
              className={`term-tab${isActive ? " is-active" : ""}`}
              onClick={() => setActive(b.sessionId)}
              onContextMenu={(e) => openTabMenu(e, b.sessionId)}
              onDoubleClick={() => startRename(b.sessionId)}
            >
              <span className="term-tab-glyph" aria-hidden>
                <HarnessGlyph harness={b.harness} />
              </span>
              {renaming === b.sessionId ? (
                <input
                  className="term-tab-rename"
                  value={renameDraft}
                  autoFocus
                  onClick={(e) => e.stopPropagation()}
                  onChange={(e) => setRenameDraft(e.target.value)}
                  onBlur={() => commitRename(b.sessionId)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") commitRename(b.sessionId);
                    if (e.key === "Escape") {
                      setRenaming(null);
                      setRenameDraft("");
                    }
                  }}
                />
              ) : (
                <span className="term-tab-label">{tabLabel(b)}</span>
              )}
              <button
                className="term-tab-close"
                aria-label="Close terminal"
                onClick={(e) => {
                  e.stopPropagation();
                  closeTerminal(b.sessionId);
                }}
              >
                <svg width="9" height="9" viewBox="0 0 9 9" aria-hidden>
                  <path d="M1 1l7 7M8 1L1 8" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" />
                </svg>
              </button>
            </div>
          );
        })}
        <button
          className="term-tab-new"
          aria-label="New terminal"
          disabled={busy}
          onClick={() => start(fixtureHarness)}
        >
          <svg width="12" height="12" viewBox="0 0 12 12" aria-hidden>
            <path d="M6 1.5v9M1.5 6h9" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
          </svg>
        </button>
      </div>

      <div className="term-stage">
        {/* The pool keeps every opened terminal alive; only the active tab renders a
            slot, so switching tabs (or leaving and returning) never re-attaches. */}
        {client && active ? (
          <TerminalSlot
            key={active}
            sessionId={active}
            client={client}
            onKill={() => closeTerminal(active)}
          />
        ) : null}
      </div>
    </div>
  );
}
