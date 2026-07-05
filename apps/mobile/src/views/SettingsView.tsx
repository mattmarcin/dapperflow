import { useStore } from "../state/app-store";
import { CAPABILITIES, Capability } from "../capabilities";
import { serviceWorkerStatus } from "../sw-register";

// Settings: connection state, pairing info, the capability profile made explicit, the
// honest LAN-plaintext posture, and disconnect. No secrets are shown; the token is
// masked because a browser PWA has nowhere safer than storage to keep it (security.md).
export function SettingsView() {
  const store = useStore();
  const { pairing, conn, daemonVersion, grantedScope, isDemo } = store;
  const sw = serviceWorkerStatus();

  return (
    <div className="view settings">
      <section className="card">
        <h3 className="card-h">Connection</h3>
        <Row label="Mode" value={isDemo ? "Demo (fixtures)" : "Live (LAN)"} />
        <Row label="Status" value={connLabel(conn)} tone={conn} />
        {!isDemo ? (
          <>
            <Row label="Daemon" value={daemonVersion || "-"} />
            <Row label="Granted scope" value={grantedScope || "phone"} mono />
            {pairing ? <Row label="Endpoint" value={pairing.url} mono /> : null}
            {pairing ? <Row label="Host" value={pairing.name ?? hostOf(pairing.url)} /> : null}
            {pairing ? <Row label="Token" value={maskToken(pairing.token)} mono /> : null}
            {pairing?.pairedAt ? <Row label="Paired" value={new Date(pairing.pairedAt).toLocaleString()} /> : null}
          </>
        ) : null}

        {isDemo ? (
          <div className="pair-help">
            <p>
              To go live, open DapperFlow on the desktop, enable the LAN listener, and scan the QR it
              shows. The QR encodes <code>http://&lt;lan-ip&gt;:&lt;port&gt;/m#pair=…</code>; your phone
              camera opens it here and pairing completes automatically.
            </p>
          </div>
        ) : (
          <button className="btn-danger btn-block" onClick={store.disconnect}>
            Disconnect &amp; forget this daemon
          </button>
        )}
      </section>

      {isDemo && store.source.demo ? (
        <section className="card">
          <h3 className="card-h">Demo controls</h3>
          <div className="demo-row">
            <span>Needs You queue</span>
            <button className="btn-ghost btn-sm" onClick={store.toggleDemoEmpty}>
              {store.demoEmpty ? "Show populated" : "Show empty (all clear)"}
            </button>
          </div>
        </section>
      ) : null}

      <section className="card">
        <h3 className="card-h">What this phone can do</h3>
        <p className="card-sub">
          The pairing token carries a fixed capability profile. The daemon enforces it; this app only
          shows what the token can exercise.
        </p>
        <ul className="caps">
          {CAPABILITIES.map((c) => (
            <CapabilityRow key={c.key} cap={c} />
          ))}
        </ul>
      </section>

      <section className="card note-card">
        <h3 className="card-h">LAN security, stated honestly</h3>
        <p>
          The v1 LAN connection ships without TLS. Self-signed certs on a home network are a dead end, so
          the capability token is the gate: the listener is opt-in, and the model assumes a trusted
          home or office network. Secret-bearing streams (env vars, raw scrollback) are excluded from the
          phone entirely - the terminal peek is a scrubbed screen capture, never the raw stream.
        </p>
        <p className="note-dim">
          True off-LAN access needs the TLS listener and device keys, and is a later milestone.
        </p>
      </section>

      <section className="card note-card">
        <h3 className="card-h">Installability &amp; offline</h3>
        <p>
          Service worker: <strong>{sw.label}</strong>.
        </p>
        <p className="note-dim">{sw.detail}</p>
      </section>

      <footer className="settings-foot">
        DapperFlow M6 PWA · the phone attention surface · standalone build (client-core extraction is M7).
      </footer>
    </div>
  );
}

function CapabilityRow({ cap }: { cap: Capability }) {
  const badge =
    cap.state === "allowed"
      ? { text: "Allowed", cls: "cap-allow" }
      : cap.state === "denied"
        ? { text: "Denied", cls: "cap-deny" }
        : cap.state === "gated-until-m5"
          ? { text: "Preview · M5", cls: "cap-gate" }
          : { text: "Soon", cls: "cap-soon" };
  return (
    <li className="cap-row">
      <div className="cap-top">
        <span className="cap-label">{cap.label}</span>
        <span className={`cap-badge ${badge.cls}`}>{badge.text}</span>
      </div>
      <span className="cap-note">{cap.note}</span>
    </li>
  );
}

function Row({ label, value, mono, tone }: { label: string; value: string; mono?: boolean; tone?: string }) {
  return (
    <div className="kv">
      <span className="kv-k">{label}</span>
      <span className={`kv-v${mono ? " mono" : ""}${tone ? ` tone-conn-${tone}` : ""}`}>{value}</span>
    </div>
  );
}

function connLabel(conn: string): string {
  switch (conn) {
    case "connected":
      return "Connected";
    case "connecting":
      return "Connecting…";
    case "disconnected":
      return "Offline - retrying";
    case "demo":
      return "Demo mode";
    default:
      return conn;
  }
}

function maskToken(token: string): string {
  if (token.length <= 8) return "••••";
  return `${token.slice(0, 4)}…${token.slice(-4)}`;
}

function hostOf(url: string): string {
  try {
    return new URL(url).host;
  } catch {
    return url;
  }
}
