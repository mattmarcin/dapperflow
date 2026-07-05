import { useEffect } from "react";
import { StoreProvider, useStore } from "./state/store";
import { TerminalPoolProvider } from "./state/terminal-pool";
import { ContextMenuProvider } from "./components/ContextMenu";
import { Sidebar } from "./components/Sidebar";
import { Board } from "./components/Board";
import { MissionControl } from "./components/MissionControl";
import { CardWorkspace } from "./components/CardWorkspace";
import { SessionView } from "./components/SessionView";
import { SettingsView } from "./components/settings/SettingsView";
import { NewSessionModal } from "./components/NewSessionModal";
import { DaemonBanner } from "./components/DaemonBanner";
import { StatusBar } from "./components/StatusBar";
import { GrantModal } from "./components/GrantModal";
import { AuditOfferModal } from "./components/AuditOfferModal";
import { TopBar } from "./components/TopBar";
import { ConcertmasterPanel } from "./components/concertmaster/ConcertmasterPanel";
import { ErrorBoundary } from "./components/ErrorBoundary";
import { ConfirmDialog } from "./components/ConfirmDialog";

export function App() {
  return (
    <StoreProvider>
      {/* ContextMenu is outermost so the pooled terminals (rendered by the pool) can
          open the one designed menu; the pool keeps xterm instances alive across
          navigation. */}
      <ContextMenuProvider>
        <TerminalPoolProvider>
          <Shell />
        </TerminalPoolProvider>
      </ContextMenuProvider>
    </StoreProvider>
  );
}

function Shell() {
  const store = useStore();

  // Global shortcut: Ctrl/Cmd+N opens the New Session front door from anywhere. It
  // never fires while typing in a field, so terminals and forms keep the key.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && !e.shiftKey && !e.altKey && (e.key === "n" || e.key === "N")) {
        const tag = (e.target as HTMLElement | null)?.tagName;
        if (tag === "INPUT" || tag === "TEXTAREA") return;
        e.preventDefault();
        store.openNewSession();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [store]);

  // Global shortcut: Ctrl/Cmd+J toggles the Concertmaster panel from anywhere. This is a
  // deliberate chord (Ctrl+J is line-feed in a raw shell, but never pressed on purpose),
  // so it fires even with a terminal or field focused - the panel is always one key away.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && !e.shiftKey && !e.altKey && (e.key === "j" || e.key === "J")) {
        e.preventDefault();
        store.togglePanel();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [store]);

  // Suppress the browser's default context menu app-wide, so a right-click reaches our
  // designed menus (or does nothing) rather than showing Chrome's Back/Reload/Save-as.
  // Text fields keep the native menu, where cut/copy/paste actually matters.
  useEffect(() => {
    const onContextMenu = (e: MouseEvent) => {
      const el = e.target as HTMLElement | null;
      const editable =
        el?.closest("input, textarea, [contenteditable='true'], [contenteditable='']");
      if (editable) return; // native cut/copy/paste
      e.preventDefault();
    };
    window.addEventListener("contextmenu", onContextMenu);
    return () => window.removeEventListener("contextmenu", onContextMenu);
  }, []);

  if (store.loading) {
    return <Boot title="Warming up the podium" detail="Loading the board…" />;
  }
  if (store.error) {
    return (
      <Boot
        title="The board could not load"
        detail={store.error}
        tone="error"
        action={{ label: "Retry", onClick: () => window.location.reload() }}
      />
    );
  }

  return (
    <div className="app">
      <Sidebar />
      <main className="stage">
        <TopBar />
        <DaemonBanner />
        <div className="stage-body">
          {/* Guard the whole content pane: a render crash in any single view or overlay
              (e.g. an unexpected data shape) shows an inline, recoverable message here
              instead of blanking the app. Keyed by the route so navigating recovers. */}
          <ErrorBoundary resetKey={`${store.view}:${store.openCardId ?? ""}:${store.openSessionId ?? ""}`}>
            {store.view === "settings" ? (
              <SettingsView />
            ) : store.view === "mission" ? (
              <MissionControl />
            ) : (
              <Board />
            )}
            {store.openCardId ? <CardWorkspace /> : null}
            {store.openSessionId ? <SessionView /> : null}
          </ErrorBoundary>
        </div>
        <StatusBar />
      </main>
      <ConcertmasterPanel />
      {store.newSessionOpen ? <NewSessionModal /> : null}
      <GrantModal />
      <AuditOfferModal />
      <TrayStopConfirm />
      <ToastHost />
    </div>
  );
}

// The confirm for a tray-initiated "Stop daemon" when live sessions exist (the tray menu
// routes through the store so it reuses the same graceful stop + confirm as the buttons).
function TrayStopConfirm() {
  const { pendingStopConfirm, liveSessionCount, confirmStopDaemon, cancelStopDaemon } = useStore();
  if (!pendingStopConfirm) return null;
  return (
    <ConfirmDialog
      title="Stop the daemon?"
      body={
        liveSessionCount > 0
          ? `${liveSessionCount} live session${liveSessionCount === 1 ? "" : "s"} will be interrupted. They keep their transcript and worktree and become resumable when the daemon restarts.`
          : "The daemon will stop. No live sessions are running, so nothing is interrupted."
      }
      confirmLabel="Stop daemon"
      tone="danger"
      onConfirm={confirmStopDaemon}
      onCancel={cancelStopDaemon}
    />
  );
}

function ToastHost() {
  const { toast, dismissToast } = useStore();
  if (!toast) return null;
  return (
    <div className={`toast toast-${toast.tone ?? "default"}`} role="status">
      <span className="toast-msg">{toast.message}</span>
      {toast.action ? (
        <button
          className="toast-action"
          onClick={() => {
            toast.action?.run();
            dismissToast();
          }}
        >
          {toast.action.label}
        </button>
      ) : null}
      <button className="toast-x" aria-label="Dismiss" onClick={dismissToast}>
        <svg width="12" height="12" viewBox="0 0 12 12" aria-hidden>
          <path d="M2 2l8 8M10 2l-8 8" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" />
        </svg>
      </button>
    </div>
  );
}

function Boot({
  title,
  detail,
  action,
  tone,
}: {
  title: string;
  detail: string;
  action?: { label: string; onClick: () => void };
  tone?: "error";
}) {
  return (
    <div className={`boot${tone === "error" ? " is-error" : ""}`}>
      <div className="boot-inner">
        <svg className="boot-mark" width="40" height="40" viewBox="0 0 40 40" aria-hidden>
          <rect x="6" y="10" width="28" height="4.6" rx="2.3" fill="#F5BC5E" />
          <rect x="6" y="18" width="21" height="4.6" rx="2.3" fill="#E6A23C" />
          <rect x="6" y="26" width="14" height="4.6" rx="2.3" fill="#B07C34" />
        </svg>
        <h1 className="boot-title">{title}</h1>
        <p className="boot-detail">{detail}</p>
        {action ? (
          <button className="btn-primary" onClick={action.onClick}>
            {action.label}
          </button>
        ) : null}
      </div>
    </div>
  );
}
