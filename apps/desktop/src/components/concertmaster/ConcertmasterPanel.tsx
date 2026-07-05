import { useCallback, useRef } from "react";
import { useStore } from "../../state/store";
import { useTerminalPool } from "../../state/terminal-pool";
import { useContextMenu } from "../ContextMenu";
import { ConcertmasterGlyph, HarnessGlyph } from "../../lib/glyphs";
import { PANEL_MAX_WIDTH, PANEL_MIN_WIDTH } from "../../lib/panel-prefs";
import { ConcertmasterSession as CmSession } from "../../model";
import { ConcertmasterSetup } from "./ConcertmasterSetup";
import { ConcertmasterSession } from "./ConcertmasterSession";

// The dockable Concertmaster panel (product.md view 5): right-docked, resizable, its
// open state and width persisted, available from every view. The panel is the podium -
// a brass-railed conductor's channel distinct from the graphite panes it sits beside.
export function ConcertmasterPanel() {
  const store = useStore();
  const { panelOpen, panelWidth, concertmaster } = store;
  if (!panelOpen) return null;

  return (
    <aside
      className={`cm-panel${concertmaster?.alive ? " has-session" : ""}`}
      style={{ width: panelWidth }}
      aria-label="Concertmaster"
    >
      <ResizeHandle width={panelWidth} onResize={store.setPanelWidth} />
      <PanelHeader cm={concertmaster} />
      <div className="cm-body">
        {concertmaster ? <ConcertmasterSession cm={concertmaster} /> : <ConcertmasterSetup />}
      </div>
    </aside>
  );
}

function PanelHeader({ cm }: { cm: CmSession | null }) {
  const store = useStore();
  const pool = useTerminalPool();

  const restart = async () => {
    if (!cm) return;
    const oldId = cm.sessionId;
    await store.restartConcertmaster();
    pool.evictTerminal(oldId);
  };

  const end = () => {
    if (!cm) return;
    const id = cm.sessionId;
    store.endConcertmaster();
    pool.evictTerminal(id);
  };

  return (
    <header className="cm-head">
      <div className="cm-head-title">
        <span className="cm-head-glyph" aria-hidden>
          <ConcertmasterGlyph size={16} />
        </span>
        <span className="cm-head-name">Concertmaster</span>
        {cm ? (
          <span className="cm-head-launcher" title={`${cm.agentName} · ${cm.harness}`}>
            <span className="cm-head-launcher-glyph" aria-hidden>
              <HarnessGlyph harness={cm.harness} />
            </span>
            {cm.agentName}
            {cm.demo ? <span className="cm-demo-tag">demo</span> : null}
          </span>
        ) : null}
      </div>

      <div className="cm-head-actions">
        {cm ? <ScopeChip cm={cm} /> : null}
        {cm ? <MountDot mounted={cm.mounted} /> : null}
        {cm ? (
          <button className="cm-head-btn" onClick={restart} title="Restart the Concertmaster session" aria-label="Restart session">
            <svg width="15" height="15" viewBox="0 0 16 16" fill="none" aria-hidden>
              <path d="M12.5 6A5 5 0 103 8" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" />
              <path d="M12.8 3v3h-3" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" strokeLinejoin="round" />
            </svg>
          </button>
        ) : null}
        {cm ? (
          <button className="cm-head-btn is-danger" onClick={end} title="End the Concertmaster session" aria-label="End session">
            <svg width="15" height="15" viewBox="0 0 16 16" fill="none" aria-hidden>
              <rect x="4" y="4" width="8" height="8" rx="1.4" stroke="currentColor" strokeWidth="1.4" />
            </svg>
          </button>
        ) : null}
        <button
          className="cm-head-btn"
          onClick={() => store.setPanelOpen(false)}
          title="Collapse the panel (the session keeps running)"
          aria-label="Collapse panel"
        >
          <svg width="15" height="15" viewBox="0 0 16 16" fill="none" aria-hidden>
            <path d="M4 4l8 8M12 4l-8 8" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
          </svg>
        </button>
      </div>
    </header>
  );
}

// The scoped-session affordance (deliverable 4): a chip that focuses the Concertmaster
// on one project. Setting it steers a context line into the session (store handles it).
function ScopeChip({ cm }: { cm: CmSession }) {
  const store = useStore();
  const { openMenu } = useContextMenu();
  const project = cm.scopeProjectId ? store.projects.find((p) => p.id === cm.scopeProjectId) : null;

  const open = (e: React.MouseEvent) => {
    openMenu(e, [
      {
        id: "all",
        label: "All projects",
        onSelect: () => store.setConcertmasterScope(null),
      },
      ...store.projects.map((p) => ({
        id: p.id,
        label: p.name,
        onSelect: () => store.setConcertmasterScope(p.id),
      })),
    ]);
  };

  return (
    <button
      className={`cm-scope${project ? " is-scoped" : ""}`}
      onClick={open}
      title={project ? `Focused on ${project.name} - click to change` : "Focus on one project"}
    >
      <span className="cm-scope-dot" aria-hidden />
      <span className="cm-scope-label">{project ? project.name : "All projects"}</span>
      <svg className="cm-scope-caret" width="9" height="9" viewBox="0 0 10 10" fill="none" aria-hidden>
        <path d="M2.5 3.5L5 6l2.5-2.5" stroke="currentColor" strokeWidth="1.3" strokeLinecap="round" strokeLinejoin="round" />
      </svg>
    </button>
  );
}

function MountDot({ mounted }: { mounted: boolean | null }) {
  const label = mounted === true ? "dflow-mcp mounted" : mounted === false ? "dflow-mcp not detected" : "mount state unknown";
  const cls = mounted === true ? "is-on" : mounted === false ? "is-off" : "is-unknown";
  return <span className={`cm-mount-dot ${cls}`} title={label} aria-label={label} />;
}

// Drag the left edge to resize; the docked panel grows leftward, so width tracks the
// distance from the pointer to the right edge of the window. Persisted by the store.
function ResizeHandle({ width, onResize }: { width: number; onResize: (w: number) => void }) {
  const startX = useRef(0);
  const startW = useRef(width);

  const onPointerDown = useCallback(
    (e: React.PointerEvent) => {
      startX.current = e.clientX;
      startW.current = width;
      (e.target as HTMLElement).setPointerCapture(e.pointerId);
      const onMove = (ev: PointerEvent) => {
        // Dragging left (negative delta) widens the panel.
        onResize(startW.current + (startX.current - ev.clientX));
      };
      const onUp = (ev: PointerEvent) => {
        (e.target as HTMLElement).releasePointerCapture?.(ev.pointerId);
        window.removeEventListener("pointermove", onMove);
        window.removeEventListener("pointerup", onUp);
      };
      window.addEventListener("pointermove", onMove);
      window.addEventListener("pointerup", onUp);
    },
    [width, onResize],
  );

  return (
    <div
      className="cm-resize"
      role="separator"
      aria-orientation="vertical"
      aria-label="Resize panel"
      aria-valuenow={width}
      aria-valuemin={PANEL_MIN_WIDTH}
      aria-valuemax={PANEL_MAX_WIDTH}
      tabIndex={0}
      onPointerDown={onPointerDown}
      onKeyDown={(e) => {
        if (e.key === "ArrowLeft") onResize(width + 16);
        if (e.key === "ArrowRight") onResize(width - 16);
      }}
    />
  );
}
