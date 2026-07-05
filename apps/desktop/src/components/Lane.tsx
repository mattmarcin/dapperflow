import { useState } from "react";
import { Card as CardModel, Lane as LaneId, LANE_HINT, LANE_LABEL, Project, Session } from "../model";
import { Card } from "./Card";

interface Props {
  lane: LaneId;
  cards: CardModel[];
  projectsById: Map<string, Project>;
  sessionsByCard: Map<string, Session[]>;
  draggingCardId: string | null;
  isDropTarget: boolean;
  onOpenCard: (cardId: string, tab?: "terminal" | "timeline") => void;
  onDragStartCard: (cardId: string) => void;
  onDragEndCard: () => void;
  onDragEnterLane: (lane: LaneId) => void;
  onDropCard: (lane: LaneId) => void;
  onQuickAdd: (lane: LaneId, title: string) => void;
  /** Bulk triage (Inbox): move the selected cards together (product.md). */
  onBulkMove?: (cardIds: string[], to: LaneId) => void;
}

export function Lane({
  lane,
  cards,
  projectsById,
  sessionsByCard,
  draggingCardId,
  isDropTarget,
  onOpenCard,
  onDragStartCard,
  onDragEndCard,
  onDragEnterLane,
  onDropCard,
  onQuickAdd,
  onBulkMove,
}: Props) {
  const [adding, setAdding] = useState(false);
  const [draft, setDraft] = useState("");
  // Inbox bulk triage: a selection mode over the lane's cards (product.md:
  // "Inbox gains bulk triage (multi-select -> dismiss / send to Shaping)").
  const [triage, setTriage] = useState(false);
  const [selected, setSelected] = useState<Set<string>>(() => new Set());

  const commitAdd = () => {
    const t = draft.trim();
    if (t) onQuickAdd(lane, t);
    setDraft("");
    setAdding(false);
  };

  const canTriage = lane === "inbox" && !!onBulkMove && cards.length > 0;
  const endTriage = () => {
    setTriage(false);
    setSelected(new Set());
  };
  const toggleSelected = (id: string) =>
    setSelected((prev) => {
      const next = new Set(prev);
      next.has(id) ? next.delete(id) : next.add(id);
      return next;
    });
  const bulk = (to: LaneId) => {
    if (selected.size === 0) return;
    onBulkMove?.([...selected], to);
    endTriage();
  };

  return (
    <section
      className={`lane lane-${lane}${isDropTarget ? " is-drop" : ""}${triage ? " is-triage" : ""}`}
      onDragOver={(e) => {
        if (draggingCardId) {
          e.preventDefault();
          onDragEnterLane(lane);
        }
      }}
      onDrop={(e) => {
        if (draggingCardId) {
          e.preventDefault();
          onDropCard(lane);
        }
      }}
    >
      <header className="lane-head">
        <span className="lane-name">{LANE_LABEL[lane]}</span>
        <span className="lane-count">{cards.length}</span>
        {canTriage ? (
          <button
            className={`lane-triage${triage ? " is-active" : ""}`}
            onClick={() => (triage ? endTriage() : setTriage(true))}
            title={triage ? "Leave triage" : "Triage the Inbox: select cards, then dismiss or send to Shaping"}
          >
            {triage ? "Done" : "Triage"}
          </button>
        ) : null}
        <button
          className="lane-add"
          aria-label={`Quick-add card to ${LANE_LABEL[lane]}`}
          onClick={() => setAdding((v) => !v)}
        >
          <svg width="12" height="12" viewBox="0 0 12 12" aria-hidden>
            <path d="M6 1.5v9M1.5 6h9" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
          </svg>
        </button>
      </header>

      <div className="lane-body">
        {adding ? (
          <div className="quick-add">
            <input
              className="quick-add-input"
              value={draft}
              autoFocus
              placeholder="Card title, then Enter"
              onChange={(e) => setDraft(e.target.value)}
              onBlur={commitAdd}
              onKeyDown={(e) => {
                if (e.key === "Enter") commitAdd();
                if (e.key === "Escape") {
                  setDraft("");
                  setAdding(false);
                }
              }}
            />
          </div>
        ) : null}

        {cards.map((card) => (
          <Card
            key={card.id}
            card={card}
            project={card.project_id ? projectsById.get(card.project_id) : undefined}
            sessions={sessionsByCard.get(card.id) ?? []}
            onOpen={(tab) => onOpenCard(card.id, tab)}
            onDragStart={() => onDragStartCard(card.id)}
            onDragEnd={onDragEndCard}
            dragging={draggingCardId === card.id}
            selectable={triage}
            selected={selected.has(card.id)}
            onToggleSelect={() => toggleSelected(card.id)}
          />
        ))}

        {cards.length === 0 && !adding ? (
          <div className={`lane-empty${isDropTarget ? " is-drop" : ""}`}>
            <span className="lane-empty-hint">{LANE_HINT[lane]}</span>
          </div>
        ) : null}
      </div>

      {triage ? (
        <div className="triage-bar" role="toolbar" aria-label="Bulk triage">
          <span className="triage-count">
            {selected.size} selected
          </span>
          <button
            className="triage-btn"
            disabled={selected.size === 0}
            onClick={() => bulk("shaping")}
            title="Send the selected cards to Shaping"
          >
            Send to Shaping
          </button>
          <button
            className="triage-btn is-danger"
            disabled={selected.size === 0}
            onClick={() => bulk("done")}
            title="Dismiss the selected cards (closed; a re-audit will not refile them)"
          >
            Dismiss
          </button>
        </div>
      ) : null}
    </section>
  );
}
