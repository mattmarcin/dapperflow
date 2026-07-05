import { useStore } from "../state/app-store";
import { NeedsYouItem } from "../client/model";
import { needsYouMeta, scoreBand } from "../lib/needs-you";
import { NeedsYouIcon } from "../lib/glyphs";
import { timeAgo } from "../lib/format";
import { capability } from "../capabilities";

// The home screen: the ranked, cross-project attention queue. One question - "what needs
// me right now?" - answered top to bottom, each item one tap from its resolving surface.
export function NeedsYouView() {
  const store = useStore();
  const items = store.snapshot?.needsYou ?? [];

  if (store.loading && !store.snapshot) return <QueueSkeleton />;

  if (items.length === 0) {
    return (
      <div className="view">
        <EmptyState />
      </div>
    );
  }

  return (
    <div className="view">
      <p className="view-hint">
        {items.length} {items.length === 1 ? "item needs" : "items need"} you, ranked by urgency.
      </p>
      <ul className="queue">
        {items.map((item) => (
          <QueueRow key={item.id} item={item} />
        ))}
      </ul>
    </div>
  );
}

function QueueRow({ item }: { item: NeedsYouItem }) {
  const store = useStore();
  const meta = needsYouMeta(item.kind);
  const band = scoreBand(item.score);
  const card = store.cardById(item.card_id);
  const project = card ? store.snapshot?.projects.find((p) => p.id === card.project_id) : undefined;
  const gatedMerge = meta.surface === "approval" && capability("merge")?.state === "gated-until-m5";

  return (
    <li className={`queue-row tone-${meta.tone}`}>
      <button className="queue-main" onClick={() => store.openResolve(item)}>
        <span className={`rank rank-${band}`} aria-label={`priority ${band}`} />
        <span className="queue-glyph" aria-hidden>
          <NeedsYouIcon glyph={meta.glyph} />
        </span>
        <span className="queue-body">
          <span className="queue-kind">{meta.label}</span>
          <span className="queue-title">{card?.title ?? "Untitled card"}</span>
          {item.note ? <span className="queue-note">{item.note}</span> : null}
          <span className="queue-foot">
            {project ? <span className="queue-proj">{project.name}</span> : null}
            <span className="queue-age">{timeAgo(item.raised_at)}</span>
          </span>
        </span>
      </button>
      <div className="queue-actions">
        <button className="btn-primary btn-sm" onClick={() => store.openResolve(item)}>
          {meta.verb}
          {gatedMerge ? <span className="lock" aria-hidden> ·</span> : null}
        </button>
        <button className="btn-ghost btn-sm" onClick={() => store.dismissNeedsYou(item.id)} aria-label="Dismiss">
          Dismiss
        </button>
      </div>
    </li>
  );
}

function EmptyState() {
  return (
    <div className="empty">
      <div className="empty-mark" aria-hidden>
        <svg width="48" height="48" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" strokeLinejoin="round">
          <path d="M4 12.5l5 5L20 6" />
        </svg>
      </div>
      <h2 className="empty-title">All clear</h2>
      <p className="empty-sub">Nothing needs you right now. Your agents are working. You will see arrivals here the moment they land.</p>
    </div>
  );
}

function QueueSkeleton() {
  return (
    <div className="view">
      <ul className="queue">
        {[0, 1, 2].map((i) => (
          <li key={i} className="queue-row skeleton">
            <div className="queue-main">
              <span className="sk sk-glyph" />
              <span className="queue-body">
                <span className="sk sk-line sk-short" />
                <span className="sk sk-line" />
              </span>
            </div>
          </li>
        ))}
      </ul>
    </div>
  );
}
