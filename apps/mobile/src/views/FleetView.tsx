import { useStore } from "../state/app-store";
import { Session } from "../client/model";
import { StateChip } from "../components/StateChip";
import { HarnessGlyph } from "../lib/glyphs";
import { elapsed } from "../lib/format";
import { isAttentionState, stateMeta } from "../lib/session-state";
import { useNow } from "../lib/use-now";

// A compact Mission Control strip: every live session with its lifecycle chip, elapsed
// time, and last status note. Tap any row for a read-only terminal peek. Attention
// states (needs_input, blocked, awaiting_feedback) float to the top.
export function FleetView() {
  const store = useStore();
  useNow(1000); // re-render so elapsed ticks
  const sessions = (store.snapshot?.sessions ?? []).slice().sort(sortForFleet);

  if (store.loading && !store.snapshot) return <div className="view"><p className="view-hint">Loading fleet...</p></div>;

  if (sessions.length === 0) {
    return (
      <div className="view">
        <div className="empty">
          <h2 className="empty-title">No live sessions</h2>
          <p className="empty-sub">When an agent starts working, it shows up here.</p>
        </div>
      </div>
    );
  }

  const attention = sessions.filter((s) => isAttentionState(s.state)).length;
  return (
    <div className="view">
      <p className="view-hint">
        {sessions.length} {sessions.length === 1 ? "session" : "sessions"}
        {attention > 0 ? ` · ${attention} need${attention === 1 ? "s" : ""} you` : " · all healthy"}
      </p>
      <ul className="fleet">
        {sessions.map((s) => (
          <FleetRow key={s.id} session={s} />
        ))}
      </ul>
    </div>
  );
}

function FleetRow({ session }: { session: Session }) {
  const store = useStore();
  const card = session.card_id ? store.cardById(session.card_id) : undefined;
  const title = card?.title ?? session.title ?? session.first_prompt ?? "Session";
  const demand = stateMeta(session.state).demand;
  return (
    <li className={`fleet-row${demand ? " is-demand" : ""}`}>
      <button className="fleet-main" onClick={() => store.openPeek(session.id)}>
        <span className="fleet-harness" aria-hidden>
          <HarnessGlyph harness={session.harness} />
        </span>
        <span className="fleet-body">
          <span className="fleet-top">
            <span className="fleet-title">{title}</span>
            <span className="fleet-elapsed">{elapsed(session.state_since)}</span>
          </span>
          <span className="fleet-mid">
            <StateChip state={session.state} />
            {session.stage ? <span className="fleet-stage">{session.stage}</span> : null}
            {session.project_name ? <span className="fleet-proj">{session.project_name}</span> : null}
          </span>
          {session.status_note ? <span className="fleet-note">{session.status_note}</span> : null}
        </span>
        <span className="fleet-chevron" aria-hidden>
          <svg width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
            <path d="M6 3.5L10.5 8L6 12.5" />
          </svg>
        </span>
      </button>
    </li>
  );
}

// Attention first, then most-recently-changed.
function sortForFleet(a: Session, b: Session): number {
  const aa = isAttentionState(a.state) ? 1 : 0;
  const bb = isAttentionState(b.state) ? 1 : 0;
  if (aa !== bb) return bb - aa;
  return b.state_since - a.state_since;
}
