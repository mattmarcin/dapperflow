// The persistent terminal pool. xterm.js instances are expensive to reconstruct and
// re-attaching replays scrollback bytes that can land mid-frame on a live TUI, so a
// terminal must NOT be destroyed just because the user navigated away (switched a
// workspace tab, closed a card, opened Settings). Instead every open session's
// terminal lives here, mounted once at the app root, and views render a lightweight
// <TerminalSlot> that the pool positions its terminal over. Leaving a view hides the
// terminal (display:none, still attached); returning is a pure re-show (refit +
// refresh, no re-attach, no replay). Only an explicit end/kill evicts and disposes it.
//
// True re-attach (app restart, daemon reconnect) still goes through TerminalPane's
// replay path; that is a different lifecycle the daemon owns.

import {
  createContext,
  ReactNode,
  useCallback,
  useContext,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { DflowClient } from "../client";
import { TerminalPane } from "../components/TerminalPane";

interface PoolEntry {
  sessionId: string;
  client: DflowClient;
  initialInput?: string;
  onKill?: () => void;
  // The element the terminal is currently shown over, or null when the session is
  // pooled but off-screen (kept alive, hidden). Never removed on navigation.
  slot: HTMLElement | null;
}

interface TerminalPoolValue {
  // Register a session's terminal (idempotent) and bind it to a visible slot.
  showTerminal: (
    sessionId: string,
    client: DflowClient,
    slot: HTMLElement,
    opts?: { initialInput?: string; onKill?: () => void },
  ) => void;
  // Detach the slot but keep the terminal alive and attached (navigation away).
  hideTerminal: (sessionId: string, slot: HTMLElement) => void;
  // Dispose the terminal for good (end/kill).
  evictTerminal: (sessionId: string) => void;
  isPooled: (sessionId: string) => boolean;
  // Read a pooled terminal's visible buffer as plain text, or null if not pooled.
  // The Concertmaster deep-link scraper polls this (screen-scrape, never the PTY).
  readTerminal: (sessionId: string, maxLines?: number) => string | null;
}

const TerminalPoolContext = createContext<TerminalPoolValue | null>(null);

export function TerminalPoolProvider({ children }: { children: ReactNode }) {
  const [entries, setEntries] = useState<PoolEntry[]>([]);
  const entriesRef = useRef<PoolEntry[]>([]);
  entriesRef.current = entries;
  // Per-session buffer readers, registered by each pooled TerminalPane while mounted.
  const readersRef = useRef<Map<string, () => string>>(new Map());
  const registerReader = useCallback((sessionId: string, reader: (() => string) | null) => {
    if (reader) readersRef.current.set(sessionId, reader);
    else readersRef.current.delete(sessionId);
  }, []);

  const showTerminal = useCallback<TerminalPoolValue["showTerminal"]>((sessionId, client, slot, opts) => {
    setEntries((prev) => {
      const existing = prev.find((e) => e.sessionId === sessionId);
      if (existing) {
        return prev.map((e) =>
          e.sessionId === sessionId
            ? {
                ...e,
                client,
                slot,
                initialInput: opts?.initialInput ?? e.initialInput,
                onKill: opts?.onKill ?? e.onKill,
              }
            : e,
        );
      }
      return [...prev, { sessionId, client, slot, initialInput: opts?.initialInput, onKill: opts?.onKill }];
    });
  }, []);

  const hideTerminal = useCallback<TerminalPoolValue["hideTerminal"]>((sessionId, slot) => {
    setEntries((prev) =>
      prev.map((e) =>
        // Only detach if this exact slot is still the one shown; a newer view may have
        // already claimed the terminal (guards a mount/unmount race across views).
        e.sessionId === sessionId && e.slot === slot ? { ...e, slot: null } : e,
      ),
    );
  }, []);

  const evictTerminal = useCallback<TerminalPoolValue["evictTerminal"]>((sessionId) => {
    setEntries((prev) => prev.filter((e) => e.sessionId !== sessionId));
  }, []);

  const isPooled = useCallback((sessionId: string) => entriesRef.current.some((e) => e.sessionId === sessionId), []);

  const readTerminal = useCallback((sessionId: string, maxLines?: number): string | null => {
    const reader = readersRef.current.get(sessionId);
    if (!reader) return null;
    try {
      const text = reader();
      if (maxLines === undefined) return text;
      const lines = text.split("\n");
      return lines.slice(Math.max(0, lines.length - maxLines)).join("\n");
    } catch {
      return null;
    }
  }, []);

  const value = useMemo<TerminalPoolValue>(
    () => ({ showTerminal, hideTerminal, evictTerminal, isPooled, readTerminal }),
    [showTerminal, hideTerminal, evictTerminal, isPooled, readTerminal],
  );

  return (
    <TerminalPoolContext.Provider value={value}>
      {children}
      <TerminalHost entries={entries} registerReader={registerReader} />
    </TerminalPoolContext.Provider>
  );
}

export function useTerminalPool(): TerminalPoolValue {
  const ctx = useContext(TerminalPoolContext);
  if (!ctx) throw new Error("useTerminalPool must be used within TerminalPoolProvider");
  return ctx;
}

// The stable host: one persistent TerminalPane per pooled session, positioned over its
// current slot (or hidden). Mounted once at the app root, so navigation never unmounts
// a terminal.
function TerminalHost({
  entries,
  registerReader,
}: {
  entries: PoolEntry[];
  registerReader: (sessionId: string, reader: (() => string) | null) => void;
}) {
  return (
    <>
      {entries.map((e) => (
        <PooledPane key={e.sessionId} entry={e} registerReader={registerReader} />
      ))}
    </>
  );
}

function PooledPane({
  entry,
  registerReader,
}: {
  entry: PoolEntry;
  registerReader: (sessionId: string, reader: (() => string) | null) => void;
}) {
  const hostRef = useRef<HTMLDivElement>(null);
  const slot = entry.slot;

  // Keep the fixed host rectangle glued to the current slot. The terminal panels are
  // static overlays, so a ResizeObserver on the slot plus a window-resize listener is
  // enough for pixel-accurate placement; a rAF re-sync catches post-mount layout.
  useLayoutEffect(() => {
    const host = hostRef.current;
    if (!host) return;
    if (!slot) {
      host.style.display = "none";
      return;
    }
    host.style.display = "block";
    const sync = () => {
      const r = slot.getBoundingClientRect();
      host.style.top = `${r.top}px`;
      host.style.left = `${r.left}px`;
      host.style.width = `${r.width}px`;
      host.style.height = `${r.height}px`;
    };
    sync();
    const ro = new ResizeObserver(sync);
    ro.observe(slot);
    ro.observe(document.documentElement);
    window.addEventListener("resize", sync);
    const raf = requestAnimationFrame(sync);
    return () => {
      ro.disconnect();
      window.removeEventListener("resize", sync);
      cancelAnimationFrame(raf);
    };
  }, [slot]);

  return (
    <div ref={hostRef} className="pooled-term">
      <TerminalPane
        client={entry.client}
        sessionId={entry.sessionId}
        active={!!slot}
        initialInput={entry.initialInput}
        onKill={entry.onKill}
        onBufferReader={(reader) => registerReader(entry.sessionId, reader)}
      />
    </div>
  );
}

// Placeholder a view renders where it wants a terminal shown. It measures itself and
// hands its element to the pool; unmounting (navigation) hides the terminal but keeps
// it alive. The pool creates the terminal on first show and reuses it forever after.
export function TerminalSlot({
  sessionId,
  client,
  initialInput,
  onKill,
  className,
}: {
  sessionId: string;
  client: DflowClient;
  initialInput?: string;
  onKill?: () => void;
  className?: string;
}) {
  const pool = useTerminalPool();
  const ref = useRef<HTMLDivElement>(null);
  // Keep the latest onKill without re-registering the slot every render.
  const onKillRef = useRef(onKill);
  onKillRef.current = onKill;

  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    pool.showTerminal(sessionId, client, el, { initialInput, onKill: () => onKillRef.current?.() });
    return () => pool.hideTerminal(sessionId, el);
    // initialInput is only meaningful at first show; intentionally not a dep.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sessionId, client, pool]);

  return <div ref={ref} className={`terminal-slot${className ? ` ${className}` : ""}`} />;
}
