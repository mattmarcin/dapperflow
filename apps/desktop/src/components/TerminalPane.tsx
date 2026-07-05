import { useCallback, useEffect, useRef } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { DflowClient } from "../client";
import { base64ToBytes } from "../protocol";
import { terminalTheme, TERMINAL_FONT } from "../terminal-theme";
import { useContextMenu } from "./ContextMenu";

interface Props {
  client: DflowClient;
  sessionId: string;
  active: boolean;
  onDims?: (sessionId: string, cols: number, rows: number) => void;
  /** Kill this session (from the terminal's right-click menu). Wired by the caller. */
  onKill?: () => void;
  /**
   * Optional first-prompt prefill (New Session front door). Typed into the PTY once,
   * shortly after replay settles, WITHOUT a trailing newline: it lands in the agent's
   * composer for the user to review and send. Best-effort - reliable auto-submit is
   * verified submit (adapters.md), a later phase.
   */
  initialInput?: string;
  /**
   * Hand the caller a function that reads the current xterm buffer as plain text
   * (null on unmount). The Concertmaster panel uses this to screen-scrape deep-link
   * tokens from the visible terminal, never intercepting the PTY stream
   * (the design notes). Reading the rendered buffer is deliberate and
   * documented; the pool wires it per session.
   */
  onBufferReader?: (reader: (() => string) | null) => void;
}

/** Read the last `maxLines` of an xterm buffer as trimmed plain text. */
export function readTerminalBuffer(term: Terminal, maxLines = 600): string {
  const buf = term.buffer.active;
  const total = buf.length;
  const start = Math.max(0, total - maxLines);
  const lines: string[] = [];
  for (let i = start; i < total; i++) {
    const line = buf.getLine(i);
    lines.push(line ? line.translateToString(true) : "");
  }
  return lines.join("\n");
}

/**
 * One xterm instance bound to one daemon session. On mount it attaches, replays
 * scrollback to reconstruct the prior screen, then streams live output. Input and
 * resize flow back as binary frames.
 *
 * First-keypress discipline (fixes the eaten-first-key bug, two halves):
 * 1. Focus: the xterm textarea is focused at three moments - right after open,
 *    the instant scrollback replay finishes, and whenever this pane becomes the
 *    active tab - so the first key never lands on the surrounding chrome.
 * 2. Replay input buffering: keystrokes typed while the replay is being written
 *    used to be dropped wholesale (the guard exists to swallow xterm's phantom
 *    ESC responses to queries embedded in the replayed bytes). Real keystrokes
 *    are now buffered and flushed to the PTY the moment replay completes;
 *    only ESC-initiated data is still discarded during replay.
 * Verified red-green in the design notes
 */
export function TerminalPane({ client, sessionId, active, onDims, onKill, initialInput, onBufferReader }: Props) {
  const { openMenu } = useContextMenu();
  const onKillRef = useRef(onKill);
  onKillRef.current = onKill;
  const onBufferReaderRef = useRef(onBufferReader);
  onBufferReaderRef.current = onBufferReader;
  const mountRef = useRef<HTMLDivElement>(null);
  const termRef = useRef<Terminal | null>(null);
  const fitRef = useRef<FitAddon | null>(null);
  const replayingRef = useRef(false);
  // First-prompt prefill, typed once after replay settles (see Props.initialInput).
  const initialInputRef = useRef(initialInput);
  initialInputRef.current = initialInput;
  const prefilledRef = useRef(false);
  // Keystrokes typed while the scrollback replay is being written. Dropping them
  // (the old behavior) is what ate the first keypress; they flush after replay.
  const pendingInputRef = useRef("");
  const activeRef = useRef(active);
  activeRef.current = active;

  // Centralized first-keypress focus. Every path that surfaces a terminal funnels through
  // this pane (New Session, click-to-open from Mission Control or the Projects tree, a
  // card's Terminal tab, a tab or view switch, and post-attach replay), so focusing here
  // makes the guarantee universal instead of per-call-site. The burst - a rAF plus a few
  // short retries - pulls focus into the xterm textarea even against a closing modal, a
  // just-clicked row/tab button that still holds focus, or a post-attach reflow, so the
  // user's first keystroke lands in the terminal and is never eaten by the chrome.
  const focusTimersRef = useRef<number[]>([]);
  const clearFocusTimers = useCallback(() => {
    focusTimersRef.current.forEach((t) => window.clearTimeout(t));
    focusTimersRef.current = [];
  }, []);
  const scheduleFocus = useCallback(() => {
    clearFocusTimers();
    const doFocus = () => {
      if (!activeRef.current) return;
      try {
        termRef.current?.focus();
      } catch {
        /* no-op */
      }
    };
    requestAnimationFrame(doFocus);
    for (const delay of [0, 60, 160]) {
      focusTimersRef.current.push(window.setTimeout(doFocus, delay));
    }
  }, [clearFocusTimers]);

  useEffect(() => {
    const term = new Terminal({
      fontFamily: TERMINAL_FONT,
      fontSize: 13,
      lineHeight: 1.25,
      letterSpacing: 0,
      cursorBlink: true,
      cursorStyle: "block",
      cursorInactiveStyle: "outline",
      scrollback: 5000,
      fontWeight: 400,
      fontWeightBold: 600,
      drawBoldTextInBrightColors: true,
      theme: terminalTheme,
      allowProposedApi: true,
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(mountRef.current as HTMLDivElement);
    termRef.current = term;
    fitRef.current = fit;
    // Focus as soon as the terminal exists, then keep retrying through the burst, so the
    // first keystroke is ready even before the async attach/replay completes.
    scheduleFocus();

    client.registerOutput(sessionId, (data) => term.write(data));
    // Expose a buffer reader for the deep-link scraper (screen-scrape, not PTY tap).
    onBufferReaderRef.current?.(() => readTerminalBuffer(term));

    const onData = term.onData((data) => {
      if (replayingRef.current) {
        // While replaying, xterm emits phantom responses to control queries
        // embedded in the replayed bytes (they start with ESC); those must never
        // reach the PTY. Real user keystrokes (printables, Enter, Ctrl chars)
        // are buffered instead of dropped, and flush the moment replay ends.
        if (!data.startsWith("\x1b")) pendingInputRef.current += data;
        return;
      }
      client.sendInput(sessionId, data);
    });
    const onResize = term.onResize(({ cols, rows }) => {
      client.sendResize(sessionId, cols, rows);
      onDims?.(sessionId, cols, rows);
    });

    let disposed = false;
    const raf = requestAnimationFrame(async () => {
      if (disposed) return;
      try {
        if (activeRef.current) fit.fit();
      } catch {
        /* hidden panes fit on activation instead */
      }
      try {
        const attached = await client.attach(sessionId, term.cols, term.rows);
        replayingRef.current = true;
        term.reset();
        term.write(base64ToBytes(attached.replay_base64), () => {
          replayingRef.current = false;
          // Flush keystrokes the user typed while the replay was in flight.
          const pending = pendingInputRef.current;
          pendingInputRef.current = "";
          if (pending) client.sendInput(sessionId, pending);
          // Re-focus once the screen is reconstructed: the reset/replay above can
          // drop focus, and this is exactly when the user starts typing.
          scheduleFocus();
          // Prefill the first prompt once, after a short settle so a booting agent
          // TUI has painted its composer. No trailing newline: the user reviews and
          // sends. Skipped if the user already started typing (pending buffer).
          const prompt = initialInputRef.current;
          if (prompt && !prefilledRef.current && !pending) {
            prefilledRef.current = true;
            window.setTimeout(() => {
              if (!disposed) client.sendInput(sessionId, prompt);
            }, 650);
          }
        });
      } catch (err) {
        term.writeln(`\r\n\x1b[38;2;229;104;106m[dapperflow] could not attach: ${err}\x1b[0m`);
      }
    });

    const observer = new ResizeObserver(() => {
      if (!activeRef.current) return;
      try {
        fit.fit();
      } catch {
        /* no-op */
      }
    });
    if (mountRef.current) observer.observe(mountRef.current);

    return () => {
      disposed = true;
      cancelAnimationFrame(raf);
      clearFocusTimers();
      observer.disconnect();
      onData.dispose();
      onResize.dispose();
      client.unregisterOutput(sessionId);
      onBufferReaderRef.current?.(null);
      client.detach(sessionId).catch(() => undefined);
      term.dispose();
      termRef.current = null;
      fitRef.current = null;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [client, sessionId]);

  useEffect(() => {
    if (!active) return;
    // Becoming the visible pane (first show, a tab or view switch, click-to-open of an
    // already-pooled session): refit and repaint so a full-screen TUI comes back
    // pixel-perfect, then hand off to the centralized focus burst so the first keystroke
    // lands in the terminal regardless of which path surfaced it.
    const resync = () => {
      try {
        fitRef.current?.fit();
      } catch {
        /* no-op */
      }
      // Re-showing a pooled terminal that was hidden (display:none) leaves xterm's
      // canvas measured against a zero-size box; a refresh after the fit repaints every
      // row so the chrome of a full-screen TUI comes back pixel-perfect on swap-back.
      try {
        const t = termRef.current;
        if (t) t.refresh(0, t.rows - 1);
      } catch {
        /* no-op */
      }
    };
    const raf = requestAnimationFrame(resync);
    const late = window.setTimeout(resync, 140);
    scheduleFocus();
    return () => {
      cancelAnimationFrame(raf);
      window.clearTimeout(late);
    };
  }, [active, scheduleFocus]);

  const onContextMenu = (e: React.MouseEvent) => {
    e.preventDefault();
    e.stopPropagation();
    const term = termRef.current;
    const selection = term?.getSelection() ?? "";
    openMenu(e, [
      {
        id: "copy",
        label: "Copy",
        disabled: !selection,
        onSelect: () => {
          if (selection) navigator.clipboard?.writeText(selection).catch(() => undefined);
        },
      },
      {
        id: "paste",
        label: "Paste",
        onSelect: () => {
          navigator.clipboard
            ?.readText()
            .then((text) => text && client.sendInput(sessionId, text))
            .catch(() => undefined);
        },
      },
      { id: "selectall", label: "Select all", onSelect: () => term?.selectAll() },
      {
        id: "clear",
        label: "Clear scrollback",
        separatorBefore: true,
        onSelect: () => term?.clear(),
      },
      ...(onKillRef.current
        ? [
            {
              id: "kill",
              label: "Kill session",
              danger: true,
              separatorBefore: true,
              onSelect: () => onKillRef.current?.(),
            },
          ]
        : []),
    ]);
  };

  return (
    <div
      className={`terminal-host${active ? "" : " is-hidden"}`}
      aria-hidden={!active}
      onContextMenu={onContextMenu}
    >
      <div ref={mountRef} className="terminal-mount" />
    </div>
  );
}
