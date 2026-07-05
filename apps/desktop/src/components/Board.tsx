import { useMemo, useState } from "react";
import { useStore } from "../state/store";
import { Card as CardModel, Lane as LaneId, LANES, LANE_LABEL } from "../model";
import { isLive } from "../lib/session-state";
import { isGrantPending } from "../lib/recipes";
import { Lane } from "./Lane";
import { CardModal } from "./CardModal";
import { ConfirmDialog } from "./ConfirmDialog";

const laneIndex = (lane: LaneId) => LANES.indexOf(lane);

export function Board() {
  const store = useStore();
  const { cards, projects, sessions, filterProjectId } = store;

  const [draggingCardId, setDraggingCardId] = useState<string | null>(null);
  const [dropLane, setDropLane] = useState<LaneId | null>(null);
  const [showCreate, setShowCreate] = useState(false);
  const [pendingBackward, setPendingBackward] = useState<{ card: CardModel; to: LaneId } | null>(null);

  const projectsById = useMemo(() => new Map(projects.map((p) => [p.id, p])), [projects]);

  const sessionsByCard = useMemo(() => {
    const m = new Map<string, typeof sessions>();
    for (const s of sessions) {
      // Cardless (New Session) sessions never appear on the board; they live in the
      // Projects tree (product.md: "the board simply does not show them").
      if (!s.card_id) continue;
      const list = m.get(s.card_id) ?? [];
      list.push(s);
      m.set(s.card_id, list);
    }
    return m;
  }, [sessions]);

  const visibleCards = useMemo(
    () => (filterProjectId ? cards.filter((c) => c.project_id === filterProjectId) : cards),
    [cards, filterProjectId],
  );

  const cardsByLane = useMemo(() => {
    const m = new Map<LaneId, CardModel[]>();
    for (const lane of LANES) m.set(lane, []);
    for (const c of visibleCards) m.get(c.lane)?.push(c);
    // Newest-updated first within a lane keeps active work near the top.
    for (const lane of LANES) m.get(lane)!.sort((a, b) => b.updated_at - a.updated_at);
    return m;
  }, [visibleCards]);

  const cardHasLiveSession = (cardId: string) =>
    (sessionsByCard.get(cardId) ?? []).some((s) => isLive(s.state));

  const performMove = (card: CardModel, to: LaneId) => {
    if (to === card.lane) return;
    // Drag to Ready arms dispatch (product.md drag semantics).
    if (to === "ready") {
      store.moveCard(card.id, to).catch((e) => store.flash(String(e), { tone: "danger" }));
      store.flash(`"${truncate(card.title)}" is armed to dispatch.`, {
        action: {
          label: "Dispatch now",
          run: () =>
            store
              .dispatch({ card_id: card.id })
              .then(() => store.flash("Dispatched. A session is starting."))
              .catch((e) => {
                // Parked behind the recipe consent modal: the modal continues it.
                if (!isGrantPending(e)) store.flash(String(e), { tone: "danger" });
              }),
        },
      });
      return;
    }
    store.moveCard(card.id, to).catch((e) => store.flash(String(e), { tone: "danger" }));
  };

  const onDrop = (to: LaneId) => {
    const card = cards.find((c) => c.id === draggingCardId);
    setDraggingCardId(null);
    setDropLane(null);
    if (!card || to === card.lane) return;

    const active = card.lane === "performing" || card.lane === "verifying" || cardHasLiveSession(card.id);
    const backward = laneIndex(to) < laneIndex(card.lane);
    if (active && backward) {
      // Dragging an active card backward means cancel or park; confirm first.
      setPendingBackward({ card, to });
      return;
    }
    performMove(card, to);
  };

  const hasAnything = projects.length > 0 || cards.length > 0;

  return (
    <div className="board-view">
      <div className="board-bar">
        <div className="filter-row" role="tablist" aria-label="Project filter">
          <button
            className={`filter-chip${filterProjectId === null ? " is-active" : ""}`}
            onClick={() => store.setFilterProject(null)}
          >
            All projects
          </button>
          {projects.map((p) => (
            <button
              key={p.id}
              className={`filter-chip${filterProjectId === p.id ? " is-active" : ""}`}
              onClick={() => store.setFilterProject(p.id)}
            >
              <span className="filter-dot" aria-hidden />
              {p.name}
            </button>
          ))}
        </div>
        <button className="btn-primary board-new" onClick={() => setShowCreate(true)}>
          <svg width="13" height="13" viewBox="0 0 13 13" aria-hidden>
            <path d="M6.5 1.5v10M1.5 6.5h10" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" />
          </svg>
          New card
        </button>
      </div>

      {hasAnything ? (
        <div className="board-scroll">
          <div className="board-lanes">
            {LANES.map((lane) => (
              <Lane
                key={lane}
                lane={lane}
                cards={cardsByLane.get(lane) ?? []}
                projectsById={projectsById}
                sessionsByCard={sessionsByCard}
                draggingCardId={draggingCardId}
                isDropTarget={dropLane === lane && draggingCardId !== null}
                onOpenCard={store.openCard}
                onDragStartCard={setDraggingCardId}
                onDragEndCard={() => {
                  setDraggingCardId(null);
                  setDropLane(null);
                }}
                onDragEnterLane={setDropLane}
                onDropCard={onDrop}
                onQuickAdd={(laneId, title) => {
                  store
                    .createCard({ title, type: "feature", project_id: filterProjectId })
                    .then((card) => {
                      if (laneId !== "inbox") store.moveCard(card.id, laneId, { silent: true });
                    })
                    .catch((e) => store.flash(String(e), { tone: "danger" }));
                }}
                onBulkMove={(cardIds, to) => {
                  // Inbox bulk triage: dismiss (close) or send to Shaping, as one action.
                  Promise.all(cardIds.map((id) => store.moveCard(id, to, { silent: true })))
                    .then(() =>
                      store.flash(
                        to === "done"
                          ? `Dismissed ${cardIds.length} card${cardIds.length === 1 ? "" : "s"}.`
                          : `Sent ${cardIds.length} card${cardIds.length === 1 ? "" : "s"} to Shaping.`,
                      ),
                    )
                    .catch((e) => store.flash(String(e), { tone: "danger" }));
                }}
              />
            ))}
          </div>
        </div>
      ) : (
        <BoardEmpty />
      )}

      {showCreate ? (
        <CardModal
          defaultProjectId={filterProjectId}
          onClose={() => setShowCreate(false)}
          onCreate={(input) => {
            setShowCreate(false);
            store.createCard(input).catch((e) => store.flash(String(e), { tone: "danger" }));
          }}
        />
      ) : null}

      {pendingBackward ? (
        <ConfirmDialog
          title={`Move back to ${LANE_LABEL[pendingBackward.to]}?`}
          body="This card has a live session. Moving it back parks the work and interrupts the running agent."
          confirmLabel="Park and move"
          tone="danger"
          onCancel={() => setPendingBackward(null)}
          onConfirm={() => {
            const { card, to } = pendingBackward;
            setPendingBackward(null);
            store.moveCard(card.id, to).catch((e) => store.flash(String(e), { tone: "danger" }));
            store.cancelDispatch(card.id);
          }}
        />
      ) : null}
    </div>
  );
}

function BoardEmpty() {
  return (
    <div className="board-empty">
      <div className="board-empty-inner">
        <svg width="52" height="52" viewBox="0 0 52 52" aria-hidden className="board-empty-mark">
          <rect x="8" y="14" width="36" height="6" rx="3" fill="#F5BC5E" />
          <rect x="8" y="24" width="27" height="6" rx="3" fill="#E6A23C" />
          <rect x="8" y="34" width="18" height="6" rx="3" fill="#B07C34" />
          <circle cx="47" cy="17" r="3.4" fill="#7BD0A8" />
        </svg>
        <h2 className="board-empty-title">The board is clear</h2>
        <p className="board-empty-body">
          Register a project from the sidebar, then capture a card. Cards flow left to
          right as agents pick them up, verify, and ship.
        </p>
      </div>
    </div>
  );
}

function truncate(s: string, n = 32): string {
  return s.length > n ? `${s.slice(0, n - 1)}…` : s;
}
