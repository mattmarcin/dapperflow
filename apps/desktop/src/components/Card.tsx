import { Card as CardModel, LANES, LANE_LABEL, Project, Session } from "../model";
import { CARD_TYPE_META } from "../lib/glyphs";
import { ATTENTION_STATES, isLive } from "../lib/session-state";
import { SessionStrip } from "./SessionStrip";
import { GateProgressStrip } from "./GateProgressStrip";
import { useContextMenu, MenuItem } from "./ContextMenu";
import { useStore } from "../state/store";

interface Props {
  card: CardModel;
  project?: Project;
  sessions: Session[];
  onOpen: (tab?: "terminal" | "timeline") => void;
  onDragStart: () => void;
  onDragEnd: () => void;
  dragging: boolean;
  // Bulk-triage selection mode (Inbox): clicking toggles membership instead of
  // opening, drag is off, and a check pip renders (product.md bulk triage).
  selectable?: boolean;
  selected?: boolean;
  onToggleSelect?: () => void;
}

const PRIORITY: Record<number, { label: string; tone: string } | undefined> = {
  1: { label: "P3", tone: "low" },
  2: { label: "P2", tone: "med" },
  3: { label: "P1", tone: "high" },
};

// Pick the session that best represents the card: one needing attention first,
// then any live one, then the most recent.
function primarySession(sessions: Session[]): Session | undefined {
  const attention = sessions.find((s) => ATTENTION_STATES.includes(s.state));
  if (attention) return attention;
  const live = sessions.find((s) => isLive(s.state));
  if (live) return live;
  return sessions[sessions.length - 1];
}

export function Card({
  card,
  project,
  sessions,
  onOpen,
  onDragStart,
  onDragEnd,
  dragging,
  selectable,
  selected,
  onToggleSelect,
}: Props) {
  const store = useStore();
  const { openMenu } = useContextMenu();
  const type = CARD_TYPE_META[card.type];
  const priority = PRIORITY[card.priority];
  const primary = primarySession(sessions.filter((s) => isLive(s.state)));
  const extraLive = sessions.filter((s) => isLive(s.state)).length - (primary ? 1 : 0);
  const hasLive = sessions.some((s) => isLive(s.state));

  const onContextMenu = (e: React.MouseEvent) => {
    const moveItems: MenuItem[] = LANES.filter((l) => l !== card.lane).map((l) => ({
      id: `move-${l}`,
      label: LANE_LABEL[l],
      onSelect: () => store.moveCard(card.id, l).catch(() => undefined),
    }));
    openMenu(e, [
      { id: "open", label: "Open", onSelect: () => onOpen() },
      ...(hasLive ? [{ id: "term", label: "Open terminal", onSelect: () => onOpen("terminal") } as MenuItem] : []),
      { id: "move", label: "Move to", submenu: moveItems, separatorBefore: true, hint: LANE_LABEL[card.lane] },
      { id: "dispatch", label: "Dispatch", onSelect: () => onOpen("terminal") },
      {
        id: "done",
        label: "Mark done",
        danger: true,
        separatorBefore: true,
        onSelect: () => store.moveCard(card.id, "done").catch(() => undefined),
      },
    ]);
  };

  return (
    <article
      className={`card${dragging ? " is-dragging" : ""}${primary ? " has-strip" : ""}${selectable ? " is-selectable" : ""}${selected ? " is-selected" : ""}`}
      draggable={!selectable}
      onDragStart={selectable ? undefined : onDragStart}
      onDragEnd={selectable ? undefined : onDragEnd}
      onClick={() => (selectable ? onToggleSelect?.() : onOpen())}
      onContextMenu={selectable ? (e) => e.preventDefault() : onContextMenu}
      onKeyDown={(e) => {
        if (e.key === "Enter") (selectable ? onToggleSelect?.() : onOpen());
        if (selectable && e.key === " ") {
          e.preventDefault();
          onToggleSelect?.();
        }
      }}
      tabIndex={0}
      role={selectable ? "checkbox" : "button"}
      aria-checked={selectable ? !!selected : undefined}
      aria-label={`${card.title} (${type.label})`}
    >
      {selectable ? (
        <span className={`card-check${selected ? " is-on" : ""}`} aria-hidden>
          <svg width="10" height="10" viewBox="0 0 12 12" fill="none">
            <path d="M2 6.5l2.8 2.8L10 3.5" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" />
          </svg>
        </span>
      ) : null}
      <div className="card-head">
        <span className={`type-badge type-${type.tone}`}>{type.label}</span>
        {priority ? <span className={`prio prio-${priority.tone}`}>{priority.label}</span> : null}
        {card.origin_kind === "github_issue" ? (
          <span className="card-origin is-github" title={card.origin_ref ?? "GitHub issue"}>
            <svg width="11" height="11" viewBox="0 0 16 16" fill="currentColor" aria-hidden className="card-origin-mark">
              <path d="M8 0C3.58 0 0 3.58 0 8c0 3.54 2.29 6.53 5.47 7.59.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49-2.01.37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13-.28-.15-.68-.52-.01-.53.63-.01 1.08.58 1.23.82.72 1.21 1.87.87 2.33.66.07-.52.28-.87.51-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82.64-.18 1.32-.27 2-.27.68 0 1.36.09 2 .27 1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01 1.93-.01 2.2 0 .21.15.46.55.38A8.01 8.01 0 0016 8c0-4.42-3.58-8-8-8z" />
            </svg>
            #{card.origin_ref?.split("#")[1] ?? "issue"}
          </span>
        ) : null}
        {card.origin_kind === "audit" ? (
          <span className="card-origin is-audit" title={card.origin_ref ?? "Filed by an onboarding audit"}>
            audit
          </span>
        ) : null}
      </div>

      <h3 className="card-title">{card.title}</h3>

      {card.evidence ? (
        <code className="card-evidence" title={card.evidence}>
          {card.evidence}
        </code>
      ) : null}

      <div className="card-foot">
        <span className={`project-chip${project ? "" : " is-none"}`}>
          <span className="project-dot" aria-hidden />
          {project ? project.name : "No project"}
        </span>
      </div>

      {/* Verifying lane: the gate's progress inline on the card (product.md). */}
      {card.lane === "verifying" ? <GateProgressStrip cardId={card.id} /> : null}

      {primary ? (
        <SessionStrip session={primary} onOpenTerminal={() => onOpen("terminal")} />
      ) : null}
      {extraLive > 0 ? <div className="card-more">+{extraLive} more session{extraLive > 1 ? "s" : ""}</div> : null}
    </article>
  );
}
