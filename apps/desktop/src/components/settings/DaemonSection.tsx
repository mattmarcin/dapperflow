import { useState } from "react";
import { useStore } from "../../state/store";
import { isLive } from "../../lib/session-state";
import { ConfirmDialog } from "../ConfirmDialog";

// Settings > Daemon: the daemon outlives the app by design (architecture.md), so this
// is where it is inspected and, when wanted, stopped or restarted. Detachment never
// means unkillable.
export function DaemonSection() {
  const store = useStore();
  const { daemon, daemonPort, daemonVersion, daemonStarted, daemonMode, keepAlive, setKeepAlive, sessions, launches } =
    store;
  const [confirmStop, setConfirmStop] = useState(false);

  const liveCount = sessions.filter((s) => isLive(s.state)).length + launches.filter((l) => l.alive).length;
  const connected = daemon === "connected";
  const statusText =
    daemon === "connected" ? "Running" : daemon === "absent" ? "Offline" : daemon === "disconnected" ? "Reconnecting" : "Connecting";
  const modeText =
    daemonMode === "dev-external"
      ? "Dev (external)"
      : daemonMode === "prod-managed"
        ? "App-managed"
        : "--";
  const devExternal = daemonMode === "dev-external";

  return (
    <div className="agents-section">
      <div className="agents-bar">
        <div className="agents-bar-text">
          <h2 className="agents-title">Daemon</h2>
          <p className="agents-sub">
            dflowd owns every session and keeps running when you close the app, so agents never die with the
            window. Stop it here when you truly want everything to end.
          </p>
        </div>
      </div>

      <div className="settings-card">
        <div className="daemon-grid">
          <Stat label="Status" value={statusText} tone={connected ? "good" : daemon === "absent" ? "bad" : undefined} />
          <Stat label="Mode" value={modeText} />
          <Stat label="Port" value={daemonPort ? String(daemonPort) : "--"} />
          <Stat label="Live sessions" value={String(liveCount)} />
          <Stat label="This run" value={daemonStarted === undefined ? "--" : daemonStarted ? "started it" : "attached"} />
          <Stat label="Version" value={daemonVersion ?? "--"} />
        </div>
        <p className="settings-note">
          {devExternal ? (
            <>
              Development mode: the app connects to a daemon you run yourself (<code>just daemon-dev</code>) and never
              spawns one, so rebuilds never fight an exe lock. Stop/Restart here act on that external daemon.
            </>
          ) : (
            <>
              The app owns a bundled daemon copied to a stable location and spawns it detached, so it outlives the
              window and updates cleanly. PID and uptime appear once the <code>daemon.info</code> verb lands.
            </>
          )}
        </p>

        <div className="remote-toggle-row daemon-keepalive">
          <div className="remote-toggle-text">
            <span className="remote-toggle-title">Keep agents running when I close the window</span>
            <span className="remote-toggle-sub">
              {keepAlive
                ? "On. Closing the window leaves the daemon and its agents running; reopen to reconnect."
                : "Off. Quitting the app gracefully stops the daemon, so nothing lingers (sessions become resumable)."}
            </span>
          </div>
          <button
            role="switch"
            aria-checked={keepAlive}
            className={`switch${keepAlive ? " is-on" : ""}`}
            onClick={() => setKeepAlive(!keepAlive)}
            title={keepAlive ? "Stop the daemon when I quit" : "Keep the daemon running when I quit"}
          >
            <span className="switch-knob" aria-hidden />
          </button>
        </div>

        <div className="settings-actions">
          <button className="btn-ghost btn-sm" onClick={() => store.restartDaemon()} disabled={!connected}>
            Restart daemon
          </button>
          <button className="btn-danger btn-sm" onClick={() => setConfirmStop(true)} disabled={!connected}>
            Stop daemon
          </button>
        </div>
      </div>

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
    </div>
  );
}

function Stat({ label, value, tone }: { label: string; value: string; tone?: "good" | "bad" }) {
  return (
    <div className="daemon-stat">
      <span className="daemon-stat-label">{label}</span>
      <span className={`daemon-stat-value${tone ? ` is-${tone}` : ""}`}>{value}</span>
    </div>
  );
}
