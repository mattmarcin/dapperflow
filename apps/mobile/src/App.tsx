import { useStore } from "./state/app-store";
import { NeedsYouView } from "./views/NeedsYouView";
import { FleetView } from "./views/FleetView";
import { SettingsView } from "./views/SettingsView";
import { TerminalPeekView } from "./views/TerminalPeekView";
import { PlanReviewView } from "./views/PlanReviewView";
import { ResolveSheet } from "./views/ResolveSheet";

export function App() {
  const store = useStore();

  return (
    <div className="app">
      <TopBar />
      {store.isDemo ? <DemoBanner /> : null}
      <main className="stage">
        {store.tab === "needs" ? <NeedsYouView /> : null}
        {store.tab === "fleet" ? <FleetView /> : null}
        {store.tab === "settings" ? <SettingsView /> : null}
      </main>
      <BottomNav />
      <OverlayHost />
      <Toast />
    </div>
  );
}

function TopBar() {
  const { conn, tab } = useStore();
  const title = tab === "needs" ? "Needs You" : tab === "fleet" ? "Fleet" : "Settings";
  return (
    <header className="topbar">
      <div className="brand">
        <BrandMark />
        <span className="brand-word">DapperFlow</span>
      </div>
      <h1 className="topbar-title">{title}</h1>
      <ConnectionDot conn={conn} />
    </header>
  );
}

function BrandMark() {
  return (
    <svg width="22" height="22" viewBox="0 0 26 26" aria-hidden className="brandmark">
      <rect x="4" y="6.5" width="18" height="3.4" rx="1.7" fill="#F5BC5E" />
      <rect x="4" y="11.3" width="14" height="3.4" rx="1.7" fill="#E6A23C" />
      <rect x="4" y="16.1" width="10" height="3.4" rx="1.7" fill="#B07C34" />
      <circle cx="23.2" cy="8.2" r="1.9" fill="#7BD0A8" />
    </svg>
  );
}

function ConnectionDot({ conn }: { conn: string }) {
  const label =
    conn === "connected"
      ? "Connected"
      : conn === "connecting"
        ? "Connecting"
        : conn === "demo"
          ? "Demo"
          : "Offline";
  return (
    <span className={`conn conn-${conn}`} title={label}>
      <span className="conn-dot" aria-hidden />
      <span className="conn-label">{label}</span>
    </span>
  );
}

function DemoBanner() {
  const { setTab } = useStore();
  return (
    <div className="demo-banner" role="status">
      <span className="demo-dot" aria-hidden />
      <span className="demo-text">
        <strong>Demo mode</strong> - showing fixtures. Pair with a desktop to go live.
      </span>
      <button className="demo-link" onClick={() => setTab("settings")}>
        Pair
      </button>
    </div>
  );
}

function BottomNav() {
  const { tab, setTab, snapshot } = useStore();
  const needsCount = snapshot?.needsYou.length ?? 0;
  return (
    <nav className="bottomnav" aria-label="Primary">
      <NavButton active={tab === "needs"} onClick={() => setTab("needs")} label="Needs You" badge={needsCount}>
        <path d="M5 8V4.5a1.4 1.4 0 012.8 0V8m0-.5V3.6a1.4 1.4 0 012.8 0V8m0-.3V5a1.4 1.4 0 012.8 0v5.2c0 2.6-2 4.3-4.8 4.3S6 12.6 5.4 10.8L4 8.4a1.4 1.4 0 012.3-1.5L7 7.8" />
      </NavButton>
      <NavButton active={tab === "fleet"} onClick={() => setTab("fleet")} label="Fleet">
        <path d="M3 4.2h10M3 8h10M3 11.8h6" />
      </NavButton>
      <NavButton active={tab === "settings"} onClick={() => setTab("settings")} label="Settings">
        <path d="M8 5.5a2.5 2.5 0 100 5 2.5 2.5 0 000-5M8 2v1.6M8 12.4V14M14 8h-1.6M3.6 8H2M12.2 3.8l-1.1 1.1M4.9 11.1l-1.1 1.1M12.2 12.2l-1.1-1.1M4.9 4.9L3.8 3.8" />
      </NavButton>
    </nav>
  );
}

function NavButton({
  active,
  onClick,
  label,
  badge,
  children,
}: {
  active: boolean;
  onClick: () => void;
  label: string;
  badge?: number;
  children: React.ReactNode;
}) {
  return (
    <button className={`nav-btn${active ? " is-active" : ""}`} onClick={onClick} aria-current={active ? "page" : undefined}>
      <span className="nav-icon" aria-hidden>
        <svg width="22" height="22" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
          {children}
        </svg>
        {badge ? <span className="nav-badge">{badge}</span> : null}
      </span>
      <span className="nav-label">{label}</span>
    </button>
  );
}

function OverlayHost() {
  const { overlay } = useStore();
  if (!overlay) return null;
  if (overlay.kind === "peek") return <TerminalPeekView sessionId={overlay.sessionId} />;
  if (overlay.kind === "plan") return <PlanReviewView cardId={overlay.cardId} />;
  return <ResolveSheet />;
}

function Toast() {
  const { toast } = useStore();
  if (!toast) return null;
  return <div className="toast" role="status">{toast}</div>;
}
