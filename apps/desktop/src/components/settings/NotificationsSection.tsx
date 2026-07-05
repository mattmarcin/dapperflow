import { useState } from "react";
import { useStore } from "../../state/store";
import { notify, notificationPermission } from "../../lib/notify";

// Settings > Notifications: a minimal, honest control surface. A master switch, a
// high-priority gate, the OS permission state, and a test toast so the wiring is
// verifiable at a glance. Preferences persist locally (the daemon does not own client
// toast settings).
export function NotificationsSection() {
  const store = useStore();
  const prefs = store.notificationPrefs;
  const [perm, setPerm] = useState(notificationPermission());

  const enableAndGrant = async () => {
    store.setNotificationPrefs({ ...prefs, enabled: true });
    const granted = await store.requestNotificationPermission();
    setPerm(granted ? "granted" : notificationPermission());
  };

  const sendTest = async () => {
    await notify({
      title: "DapperFlow notifications are on",
      body: "You will hear about Needs You arrivals here.",
      tag: "dflow-test",
      deepLink: {},
    });
    setPerm(notificationPermission());
  };

  return (
    <div className="agents-section">
      <div className="agents-bar">
        <div className="agents-bar-text">
          <h2 className="agents-title">Notifications</h2>
          <p className="agents-sub">
            DapperFlow notifies you when an agent needs you - a blocked session, a plan to review, a PR ready to
            merge. Notification fatigue is a bug, so each item notifies once and high-noise items can be muted.
          </p>
        </div>
      </div>

      <div className="settings-card">
        <ToggleRow
          label="Desktop notifications"
          hint="Show a system notification when something needs you."
          on={prefs.enabled}
          onToggle={() => store.setNotificationPrefs({ ...prefs, enabled: !prefs.enabled })}
        />
        <ToggleRow
          label="High priority only"
          hint="Only notify for blocked agents and other high-cost items; skip routine ones."
          on={prefs.onlyHighPriority}
          disabled={!prefs.enabled}
          onToggle={() => store.setNotificationPrefs({ ...prefs, onlyHighPriority: !prefs.onlyHighPriority })}
        />

        <div className="settings-row">
          <div className="settings-row-text">
            <span className="settings-row-label">System permission</span>
            <span className="settings-row-hint">
              {perm === "granted"
                ? "Granted - notifications can appear."
                : perm === "denied"
                  ? "Blocked in your OS settings. Allow DapperFlow to send notifications there."
                  : "Not requested yet."}
            </span>
          </div>
          <span className={`perm-badge is-${perm}`}>{perm}</span>
        </div>

        <div className="settings-actions">
          {perm !== "granted" ? (
            <button className="btn-primary btn-sm" onClick={enableAndGrant}>
              Enable notifications
            </button>
          ) : null}
          <button className="btn-ghost btn-sm" onClick={sendTest} disabled={!prefs.enabled}>
            Send a test notification
          </button>
        </div>
      </div>
    </div>
  );
}

function ToggleRow({
  label,
  hint,
  on,
  disabled,
  onToggle,
}: {
  label: string;
  hint: string;
  on: boolean;
  disabled?: boolean;
  onToggle: () => void;
}) {
  return (
    <div className={`settings-row${disabled ? " is-disabled" : ""}`}>
      <div className="settings-row-text">
        <span className="settings-row-label">{label}</span>
        <span className="settings-row-hint">{hint}</span>
      </div>
      <button
        className={`switch${on ? " is-on" : ""}`}
        role="switch"
        aria-checked={on}
        aria-label={label}
        disabled={disabled}
        onClick={onToggle}
      >
        <span className="switch-knob" />
      </button>
    </div>
  );
}
