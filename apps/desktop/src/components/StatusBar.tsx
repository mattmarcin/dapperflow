import { useState } from "react";
import { useStore } from "../state/store";
import { isLive } from "../lib/session-state";
import { useContextMenu } from "./ContextMenu";
import { ConfirmDialog } from "./ConfirmDialog";

const STATUS_LABEL: Record<string, string> = {
  connecting: "connecting",
  connected: "connected",
  disconnected: "reconnecting",
  absent: "offline",
};

// The mission-control readout along the base of the stage. The connection segment is a
// control: it shows whether this run started the daemon or attached to a running one,
// and opens the daemon kill switch (Stop / Restart) so a detached daemon is never
// unkillable.
export function StatusBar() {
  const store = useStore();
  const { daemon, daemonPort, daemonVersion, daemonStarted, daemonMode, cards, sessions, launches, dataMode } =
    store;
  const { openMenu } = useContextMenu();
  const [confirmStop, setConfirmStop] = useState(false);

  const liveCount = sessions.filter((s) => isLive(s.state)).length + launches.filter((l) => l.alive).length;
  const connected = daemon === "connected";
  // "started" = this run spawned the daemon; "attached" = connected to a running one.
  const originLabel = daemonStarted ? "started" : "attached";

  const openDaemonMenu = (e: { clientX: number; clientY: number; preventDefault?: () => void }) => {
    openMenu(e, [
      { id: "restart", label: "Restart daemon", onSelect: () => store.restartDaemon() },
      {
        id: "stop",
        label: "Stop daemon",
        danger: true,
        separatorBefore: true,
        disabled: !connected,
        onSelect: () => setConfirmStop(true),
      },
    ]);
  };

  return (
    <footer className="statusbar">
      <button
        className={`status-conn is-${daemon}`}
        onClick={openDaemonMenu}
        onContextMenu={openDaemonMenu}
        title="Daemon controls"
      >
        <span className="dot" aria-hidden />
        <span className="status-conn-label">{STATUS_LABEL[daemon] ?? daemon}</span>
        {connected ? <span className="status-origin">{originLabel}</span> : null}
        <svg className="status-conn-caret" width="9" height="9" viewBox="0 0 10 10" fill="none" aria-hidden>
          <path d="M2.5 3.5L5 6l2.5-2.5" stroke="currentColor" strokeWidth="1.3" strokeLinecap="round" strokeLinejoin="round" />
        </svg>
      </button>
      <Segment label="port" value={daemonPort ? String(daemonPort) : "--"} />
      <Segment label="cards" value={String(cards.length)} />
      <Segment label="live" value={String(liveCount)} muted={liveCount === 0} />
      <span className="status-spacer" />
      {dataMode === "fixture" ? (
        <span className="status-fixture" title="Dev fixtures (VITE_DFLOW_FIXTURES): this board is demo data, not a live daemon.">
          demo data (fixtures)
        </span>
      ) : null}
      {daemonMode === "dev-external" ? (
        <span className="status-mode" title="Development: the app connects to an externally-run daemon (just daemon-dev)">
          dev daemon
        </span>
      ) : null}
      <span className="status-build">{daemonVersion ?? "dflowd"}</span>

      {confirmStop ? (
        <ConfirmDialog
          title="Stop the daemon?"
          body={
            liveCount > 0
              ? `${liveCount} live session${liveCount === 1 ? "" : "s"} will be interrupted. They keep their transcript and worktree and become resumable when the daemon restarts.`
              : "The daemon will stop. No live sessions are running, so nothing is interrupted."
          }
          confirmLabel="Stop daemon"
          tone="danger"
          onConfirm={() => {
            setConfirmStop(false);
            store.stopDaemon();
          }}
          onCancel={() => setConfirmStop(false)}
        />
      ) : null}
    </footer>
  );
}

function Segment({ label, value, muted }: { label: string; value: string; muted?: boolean }) {
  return (
    <div className="status-seg">
      <span className="status-seg-label">{label}</span>
      <span className={`status-seg-value${muted ? " is-muted" : ""}`}>{value}</span>
    </div>
  );
}
