import { useEffect, useMemo } from "react";
import { useStore, WorkspaceTab } from "../state/store";
import { LANE_LABEL } from "../model";
import { CARD_TYPE_META } from "../lib/glyphs";
import { TerminalTab } from "./workspace/TerminalTab";
import { TimelineTab } from "./workspace/TimelineTab";
import { PlanTab } from "./workspace/PlanTab";
import { IssueTab } from "./workspace/IssueTab";
import { VerifyTab } from "./workspace/VerifyTab";
import { RecipeDial } from "./RecipeDial";

const PRIORITY: Record<number, string | undefined> = { 1: "P3", 2: "P2", 3: "P1" };

export function CardWorkspace() {
  const store = useStore();
  const card = useMemo(
    () => store.cards.find((c) => c.id === store.openCardId),
    [store.cards, store.openCardId],
  );
  const project = card?.project_id ? store.projects.find((p) => p.id === card.project_id) : undefined;
  const sessions = useMemo(
    () => (card ? store.sessions.filter((s) => s.card_id === card.id) : []),
    [store.sessions, card],
  );

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") store.closeCard();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [store]);

  if (!card) return null;

  const type = CARD_TYPE_META[card.type];
  const prio = PRIORITY[card.priority];
  const isIssueCard = card.origin_kind === "github_issue";
  const tabs: { id: WorkspaceTab; label: string; soon?: boolean }[] = [
    { id: "terminal", label: "Terminal" },
    // Issue tab only for GitHub-issue-origin cards (product.md).
    ...(isIssueCard ? [{ id: "issue" as WorkspaceTab, label: "Issue" }] : []),
    { id: "timeline", label: "Timeline" },
    { id: "plan", label: "Plan" },
    { id: "verify", label: "Verify" },
    { id: "diff", label: "Diff", soon: true },
  ];

  return (
    <div className="workspace">
      <header className="ws-head">
        <button className="ws-back" onClick={store.closeCard} aria-label="Back to board">
          <svg width="16" height="16" viewBox="0 0 16 16" fill="none" aria-hidden>
            <path d="M9.5 3.5L5 8l4.5 4.5" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" />
          </svg>
          <span>Board</span>
        </button>
        <div className="ws-titleblock">
          <div className="ws-meta">
            <span className={`type-badge type-${type.tone}`}>{type.label}</span>
            {prio ? <span className={`prio prio-${card.priority === 3 ? "high" : card.priority === 2 ? "med" : "low"}`}>{prio}</span> : null}
            <span className={`project-chip${project ? "" : " is-none"}`}>
              <span className="project-dot" aria-hidden />
              {project ? project.name : "No project"}
            </span>
            <span className="ws-lane">{LANE_LABEL[card.lane]}</span>
          </div>
          <h1 className="ws-title">{card.title}</h1>
          {card.brief ? <p className="ws-brief">{card.brief}</p> : null}
        </div>
        <div className="ws-side">
          {/* The process dial: which flow recipe governs this card (product.md). */}
          <RecipeDial
            value={card.dial_recipe}
            projectId={card.project_id}
            projectDefault={project?.default_recipe ?? null}
            onChange={(name) =>
              store
                .updateCardDial(card.id, name)
                .catch((e) => store.flash(String(e), { tone: "danger" }))
            }
          />
        </div>
      </header>

      <nav className="ws-tabs" role="tablist" aria-label="Workspace">
        {tabs.map((t) => (
          <button
            key={t.id}
            role="tab"
            aria-selected={store.workspaceTab === t.id}
            className={`ws-tab${store.workspaceTab === t.id ? " is-active" : ""}${t.soon ? " is-soon" : ""}`}
            disabled={t.soon}
            onClick={() => !t.soon && store.setWorkspaceTab(t.id)}
          >
            {t.label}
            {t.soon ? <span className="ws-tab-soon">soon</span> : null}
          </button>
        ))}
      </nav>

      <div className="ws-content">
        {store.workspaceTab === "terminal" ? <TerminalTab card={card} sessions={sessions} /> : null}
        {store.workspaceTab === "issue" ? <IssueTab card={card} sessions={sessions} /> : null}
        {store.workspaceTab === "timeline" ? <TimelineTab cardId={card.id} /> : null}
        {store.workspaceTab === "plan" ? <PlanTab card={card} /> : null}
        {store.workspaceTab === "verify" ? <VerifyTab card={card} sessions={sessions} /> : null}
      </div>
    </div>
  );
}
