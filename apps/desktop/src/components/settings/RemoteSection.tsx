// Settings > Remote: the M6 device-pairing screen (security.md / Remote access trust
// model, LAN-first). An opt-in LAN listener (off by default) that, when on, shows a QR
// encoding the pairing payload apps/mobile settled on:
//   http://<lan-ip>:<port>/m#pair=<base64url{url,token}>
// plus the plaintext URL, a rotate/revoke control, and the paired-devices list.
//
// INTEGRATION SEAM: the daemon LAN endpoints do not exist yet. The URL and phone token
// are fixtured here (see data/m5-fixtures.ts); enabling shows the honest no-TLS caveat.
// When the daemon's listener lands, the DataSource swaps to the real remote.* verbs and
// nothing in this component changes.

import { useCallback, useEffect, useState } from "react";
import { useStore } from "../../state/store";
import { PairedDevice, RemoteListenerState } from "../../model";
import { QRCode } from "../../lib/qrcode";
import { ConfirmDialog } from "../ConfirmDialog";
import { elapsed } from "../../lib/format";
import { useNow } from "../../lib/use-now";

export function RemoteSection() {
  const store = useStore();
  const [state, setState] = useState<RemoteListenerState | "loading">("loading");
  const [busy, setBusy] = useState(false);
  const [confirmRotate, setConfirmRotate] = useState(false);
  const [revoking, setRevoking] = useState<PairedDevice | null>(null);
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    let cancelled = false;
    store.getRemoteState().then((s) => !cancelled && setState(s));
    return () => {
      cancelled = true;
    };
  }, [store]);

  const toggle = useCallback(
    async (enabled: boolean) => {
      setBusy(true);
      try {
        setState(await store.setRemoteEnabled(enabled));
      } catch (e) {
        store.flash(String(e), { tone: "danger" });
      } finally {
        setBusy(false);
      }
    },
    [store],
  );

  const rotate = useCallback(async () => {
    setConfirmRotate(false);
    setBusy(true);
    try {
      setState(await store.rotateRemoteToken());
      store.flash("Pairing token rotated. Scan again to re-pair; old devices are revoked.");
    } catch (e) {
      store.flash(String(e), { tone: "danger" });
    } finally {
      setBusy(false);
    }
  }, [store]);

  const revoke = useCallback(async () => {
    const dev = revoking;
    if (!dev) return;
    setRevoking(null);
    try {
      setState(await store.revokeRemoteDevice(dev.id));
      store.flash(`Revoked ${dev.name}.`);
    } catch (e) {
      store.flash(String(e), { tone: "danger" });
    }
  }, [revoking, store]);

  const copyUrl = useCallback(
    (url: string) => {
      navigator.clipboard?.writeText(url).then(
        () => {
          setCopied(true);
          window.setTimeout(() => setCopied(false), 1600);
        },
        () => store.flash("Could not copy to clipboard.", { tone: "danger" }),
      );
    },
    [store],
  );

  if (state === "loading") {
    return <div className="agents-section"><div className="settings-card gh-loading">Loading remote access…</div></div>;
  }

  const on = state.enabled;

  return (
    <div className="agents-section">
      <div className="agents-bar">
        <div className="agents-bar-text">
          <h2 className="agents-title">Remote</h2>
          <p className="agents-sub">
            Pair a phone on your network to answer Needs You items, approve plans, steer stuck agents, and
            merge green PRs from the couch. The desktop stays the only thing that holds your code.
          </p>
        </div>
      </div>

      <div className={`settings-card remote-listener${on ? " is-on" : ""}`}>
        <div className="remote-toggle-row">
          <div className="remote-toggle-text">
            <span className="remote-toggle-title">LAN listener</span>
            <span className="remote-toggle-sub">
              {on ? "On. Reachable by paired devices on this network." : "Off. No remote device can reach this daemon."}
            </span>
          </div>
          <button
            role="switch"
            aria-checked={on}
            className={`switch${on ? " is-on" : ""}`}
            onClick={() => toggle(!on)}
            disabled={busy}
            title={on ? "Turn the LAN listener off" : "Turn the LAN listener on"}
          >
            <span className="switch-knob" aria-hidden />
          </button>
        </div>

        <div className="remote-caveat" role="note">
          <IconShieldAlert />
          <span>
            LAN v1 ships <strong>without TLS</strong> - self-signed certs on a home network are a UX dead end.
            The capability token is the gate, the listener is opt-in, and this assumes a trusted home or office
            network. True off-network remote will require the TLS listener and device keys.
          </span>
        </div>
      </div>

      {on && state.pairing_payload && state.url ? (
        <div className="settings-card remote-pair">
          <div className="remote-pair-grid">
            <div className="remote-qr-wrap">
              <QRCode value={state.pairing_payload} size={214} title="Scan to pair a phone" />
              <span className="remote-qr-cap">Scan with the phone's camera</span>
            </div>

            <div className="remote-pair-detail">
              <h3 className="remote-pair-title">Pair a device</h3>
              <p className="remote-pair-lede">
                Open the phone camera and scan, or type the URL into a phone browser on this network.
              </p>

              <div className="remote-url-block">
                <span className="remote-url-label">Pairing URL</span>
                <div className="remote-url-row">
                  <code className="remote-url">{state.url}</code>
                  <button className="btn-ghost btn-sm" onClick={() => copyUrl(state.pairing_payload!)}>
                    {copied ? "Copied" : "Copy link"}
                  </button>
                </div>
                <span className="remote-url-hint">
                  The full link carries the phone token in its <code>#pair</code> fragment; the QR encodes the same.
                </span>
              </div>

              <div className="remote-profile">
                <span className="remote-profile-label">Phone capability profile</span>
                <div className="remote-caps">
                  <Cap on={state.profile.needs_you} label="Needs You" />
                  <Cap on={state.profile.approvals} label="Approvals" />
                  <Cap on={state.profile.steering} label="Steering" />
                  <Cap on={state.profile.terminals_read_only} label="Terminals (read-only)" />
                  <Cap on={state.profile.vault_access} label="Vault access" />
                  <Cap on={state.profile.recipe_install} label="Recipe install" />
                </div>
              </div>

              <div className="remote-pair-actions">
                <button className="btn-ghost btn-sm" onClick={() => setConfirmRotate(true)} disabled={busy}>
                  <IconRotate />
                  Rotate token
                </button>
              </div>
            </div>
          </div>
          <p className="settings-note">
            Integration seam: the LAN listener endpoints are not served by the daemon yet, so the URL and token
            here are fixtured. When <code>remote.status</code> / <code>remote.listener</code> land, this screen
            renders the daemon's real pairing payload unchanged.
          </p>
        </div>
      ) : null}

      <DevicesCard devices={state.devices} onRevoke={setRevoking} disabled={busy} />

      {confirmRotate ? (
        <ConfirmDialog
          title="Rotate the pairing token?"
          body="Every paired device is revoked and must scan the new QR to reconnect. Use this if a phone was lost or a link leaked."
          confirmLabel="Rotate token"
          tone="danger"
          onCancel={() => setConfirmRotate(false)}
          onConfirm={rotate}
        />
      ) : null}

      {revoking ? (
        <ConfirmDialog
          title={`Revoke ${revoking.name}?`}
          body="This device loses access immediately. It can pair again by scanning the QR while the listener is on."
          confirmLabel="Revoke device"
          tone="danger"
          onCancel={() => setRevoking(null)}
          onConfirm={revoke}
        />
      ) : null}
    </div>
  );
}

function DevicesCard({
  devices,
  onRevoke,
  disabled,
}: {
  devices: PairedDevice[];
  onRevoke: (d: PairedDevice) => void;
  disabled: boolean;
}) {
  const now = useNow(30_000);
  return (
    <div className="settings-card remote-devices">
      <div className="remote-devices-head">
        <h3 className="remote-devices-title">Paired devices</h3>
        <span className="remote-devices-count">{devices.length}</span>
      </div>
      {devices.length === 0 ? (
        <p className="remote-devices-empty">No devices paired. Scan the QR above to add one.</p>
      ) : (
        <ul className="remote-device-list">
          {devices.map((d) => (
            <li key={d.id} className="remote-device">
              <span className="remote-device-glyph" aria-hidden>
                <IconPhone />
              </span>
              <div className="remote-device-main">
                <span className="remote-device-name">{d.name}</span>
                <span className="remote-device-meta">
                  {d.profile} · paired {elapsed(d.paired_at, now)} ago
                  {d.last_seen ? ` · seen ${elapsed(d.last_seen, now)} ago` : ""}
                </span>
              </div>
              <button className="btn-danger btn-sm" onClick={() => onRevoke(d)} disabled={disabled}>
                Revoke
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

function Cap({ on, label }: { on: boolean; label: string }) {
  return (
    <span className={`remote-cap${on ? " is-on" : " is-off"}`}>
      <span className="remote-cap-mark" aria-hidden>
        {on ? (
          <svg width="11" height="11" viewBox="0 0 12 12" fill="none">
            <path d="M2 6.5l2.8 2.8L10 3.5" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round" />
          </svg>
        ) : (
          <svg width="11" height="11" viewBox="0 0 12 12" fill="none">
            <path d="M3 3l6 6M9 3l-6 6" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" />
          </svg>
        )}
      </span>
      {label}
    </span>
  );
}

function IconShieldAlert() {
  return (
    <svg width="15" height="15" viewBox="0 0 16 16" fill="none" aria-hidden>
      <path d="M8 1.8l5 1.7v3.6c0 3.2-2.1 5.4-5 6.4-2.9-1-5-3.2-5-6.4V3.5z" stroke="currentColor" strokeWidth="1.3" strokeLinejoin="round" />
      <path d="M8 5.4v3M8 10.4v.01" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" />
    </svg>
  );
}

function IconRotate() {
  return (
    <svg width="13" height="13" viewBox="0 0 16 16" fill="none" aria-hidden>
      <path d="M13 3.5v3h-3M3 12.5v-3h3" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" strokeLinejoin="round" />
      <path d="M12.5 6.5A5 5 0 003.6 5.2M3.5 9.5a5 5 0 008.9 1.3" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" />
    </svg>
  );
}

function IconPhone() {
  return (
    <svg width="15" height="15" viewBox="0 0 16 16" fill="none" aria-hidden>
      <rect x="4.5" y="1.8" width="7" height="12.4" rx="1.6" stroke="currentColor" strokeWidth="1.3" />
      <path d="M7 12.4h2" stroke="currentColor" strokeWidth="1.3" strokeLinecap="round" />
    </svg>
  );
}
