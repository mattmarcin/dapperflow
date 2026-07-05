import { useState } from "react";
import { AgentsSection } from "./AgentsSection";
import { NotificationsSection } from "./NotificationsSection";
import { DaemonSection } from "./DaemonSection";
import { GithubSection } from "./GithubSection";
import { RemoteSection } from "./RemoteSection";

type Section =
  | "agents"
  | "github"
  | "remote"
  | "notifications"
  | "daemon"
  | "projects"
  | "adapters"
  | "tokens";

const SECTIONS: { id: Section; label: string; soon?: boolean }[] = [
  { id: "agents", label: "Agents" },
  { id: "github", label: "GitHub" },
  { id: "remote", label: "Remote" },
  { id: "notifications", label: "Notifications" },
  { id: "daemon", label: "Daemon" },
  { id: "projects", label: "Projects", soon: true },
  { id: "adapters", label: "Adapters", soon: true },
  { id: "tokens", label: "Tokens", soon: true },
];

// The Settings surface. Agents is the first and only live section this phase; the
// rest are named placeholders so the shape of Settings is honest (product.md view 6).
export function SettingsView() {
  const [section, setSection] = useState<Section>("agents");

  return (
    <div className="settings">
      <header className="settings-head">
        <h1 className="settings-title">Settings</h1>
        <p className="settings-lede">Configure the agents, projects, and adapters DapperFlow conducts.</p>
      </header>

      <nav className="settings-tabs" role="tablist" aria-label="Settings sections">
        {SECTIONS.map((s) => (
          <button
            key={s.id}
            role="tab"
            aria-selected={section === s.id}
            className={`settings-tab${section === s.id ? " is-active" : ""}${s.soon ? " is-soon" : ""}`}
            disabled={s.soon}
            onClick={() => !s.soon && setSection(s.id)}
          >
            {s.label}
            {s.soon ? <span className="ws-tab-soon">soon</span> : null}
          </button>
        ))}
      </nav>

      <div className="settings-body">
        {section === "agents" ? <AgentsSection /> : null}
        {section === "github" ? <GithubSection /> : null}
        {section === "remote" ? <RemoteSection /> : null}
        {section === "notifications" ? <NotificationsSection /> : null}
        {section === "daemon" ? <DaemonSection /> : null}
      </div>
    </div>
  );
}
