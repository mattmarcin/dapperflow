import { useStore } from "../state/store";
import { ConcertmasterGlyph } from "../lib/glyphs";

// A slim utility bar across the top of the stage. Its standing job is the Concertmaster
// toggle (product.md view 5: the panel is available everywhere) - a persistent podium
// button that reflects the panel's open state and shows a live pulse when the
// Concertmaster session is running. The left carries the active project scope, so the
// bar earns its space instead of floating one lonely control.
export function TopBar() {
  const store = useStore();
  const { panelOpen, concertmaster } = store;
  const filter = store.filterProjectId
    ? store.projects.find((p) => p.id === store.filterProjectId)
    : null;

  return (
    <div className="topbar">
      <div className="topbar-context">
        {filter ? (
          <button className="topbar-filter" onClick={() => store.setFilterProject(null)} title="Clear project filter">
            <span className="topbar-filter-dot" aria-hidden />
            {filter.name}
            <span className="topbar-filter-x" aria-hidden>
              ×
            </span>
          </button>
        ) : null}
      </div>

      <button
        className={`cm-toggle${panelOpen ? " is-open" : ""}${concertmaster?.alive ? " is-live" : ""}`}
        onClick={store.togglePanel}
        title={panelOpen ? "Hide the Concertmaster (Ctrl/Cmd+J)" : "Show the Concertmaster (Ctrl/Cmd+J)"}
        aria-pressed={panelOpen}
      >
        <span className="cm-toggle-glyph" aria-hidden>
          <ConcertmasterGlyph size={15} />
        </span>
        <span className="cm-toggle-text">Concertmaster</span>
        {concertmaster?.alive ? <span className="cm-toggle-pulse" aria-hidden /> : null}
        <span className="cm-toggle-kbd" aria-hidden>
          Ctrl J
        </span>
      </button>
    </div>
  );
}
