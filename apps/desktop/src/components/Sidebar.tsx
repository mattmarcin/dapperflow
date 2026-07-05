import { useStore } from "../state/store";
import { ProjectsTree } from "./ProjectsTree";

// The left channel strip: brand at the podium, the New Session hero (the front
// door), primary nav, and the live Projects tree. Daemon status and controls live in
// the status bar and Settings > Daemon, so the sidebar carries no daemon readout.
export function Sidebar() {
  const store = useStore();
  const { view } = store;
  const needsYouCount = store.needsYou.length;

  return (
    <aside className="sidebar">
      <div className="brand">
        <BrandMark />
        <div className="brand-word">
          <span className="brand-name">DAPPERFLOW</span>
          <span className="brand-tag">cockpit</span>
        </div>
      </div>

      <button className="new-session-cta" onClick={store.openNewSession}>
        <span className="new-session-cta-glyph" aria-hidden>
          <IconSpark />
        </span>
        <span className="new-session-cta-text">New session</span>
        <span className="new-session-cta-kbd" aria-hidden>
          Ctrl N
        </span>
      </button>

      <nav className="nav" aria-label="Primary">
        <NavItem
          label="Mission Control"
          icon={<IconRadar />}
          active={view === "mission"}
          badge={needsYouCount > 0 ? needsYouCount : undefined}
          onClick={() => store.setView("mission")}
        />
        <NavItem
          label="Board"
          icon={<IconColumns />}
          active={view === "board"}
          onClick={() => store.setView("board")}
        />
        <NavItem
          label="Settings"
          icon={<IconSliders />}
          active={view === "settings"}
          onClick={() => store.setView("settings")}
        />
      </nav>

      <ProjectsTree />
    </aside>
  );
}

function NavItem({
  label,
  icon,
  active,
  soon,
  badge,
  onClick,
}: {
  label: string;
  icon: React.ReactNode;
  active?: boolean;
  soon?: boolean;
  badge?: number;
  onClick?: () => void;
}) {
  return (
    <button
      className={`nav-item${active ? " is-active" : ""}${soon ? " is-soon" : ""}`}
      disabled={soon}
      aria-current={active ? "page" : undefined}
      onClick={onClick}
    >
      <span className="nav-icon" aria-hidden>
        {icon}
      </span>
      <span className="nav-label">{label}</span>
      {badge !== undefined ? (
        <span className="nav-badge" title={`${badge} need you`}>
          {badge}
        </span>
      ) : null}
      {soon ? <span className="nav-soon">soon</span> : null}
    </button>
  );
}

function IconSpark() {
  return (
    <svg width="15" height="15" viewBox="0 0 16 16" fill="none" aria-hidden>
      <path
        d="M8 1.5v13M1.5 8h13M3.4 3.4l9.2 9.2M12.6 3.4l-9.2 9.2"
        stroke="currentColor"
        strokeWidth="1.3"
        strokeLinecap="round"
      />
    </svg>
  );
}

function BrandMark() {
  return (
    <svg className="brand-mark" width="26" height="26" viewBox="0 0 26 26" aria-hidden>
      <rect x="4" y="6.5" width="18" height="3.4" rx="1.7" fill="#F5BC5E" />
      <rect x="4" y="11.3" width="14" height="3.4" rx="1.7" fill="#E6A23C" />
      <rect x="4" y="16.1" width="10" height="3.4" rx="1.7" fill="#B07C34" />
      <circle cx="23.2" cy="8.2" r="1.9" fill="#7BD0A8" />
    </svg>
  );
}

function IconRadar() {
  return (
    <svg width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.4">
      <circle cx="8" cy="8" r="6" />
      <circle cx="8" cy="8" r="2.4" />
      <path d="M8 8 L12.5 4" strokeLinecap="round" />
    </svg>
  );
}

function IconColumns() {
  return (
    <svg width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.4">
      <rect x="2" y="2.5" width="3.4" height="11" rx="1" />
      <rect x="6.3" y="2.5" width="3.4" height="11" rx="1" />
      <rect x="10.6" y="2.5" width="3.4" height="11" rx="1" />
    </svg>
  );
}

function IconSliders() {
  return (
    <svg width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.4">
      <path d="M3 4.5h10M3 11.5h10" strokeLinecap="round" />
      <circle cx="6" cy="4.5" r="1.9" fill="currentColor" stroke="none" />
      <circle cx="10.5" cy="11.5" r="1.9" fill="currentColor" stroke="none" />
    </svg>
  );
}
