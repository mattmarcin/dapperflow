import { useEffect, useState } from "react";
import { useStore } from "../../state/store";
import { CardEvent } from "../../model";
import { EventIcon, eventGlyph } from "../../lib/glyphs";
import { clockTime, timeAgo } from "../../lib/format";
import { useNow } from "../../lib/use-now";

interface Props {
  cardId: string;
}

// Humanized event titles. Unknown kinds fall back to the raw kind (never dropped).
const KIND_TITLE: Record<string, string> = {
  created: "Card created",
  shaped: "Brief shaped",
  moved: "Moved lane",
  dial_changed: "Recipe changed",
  closed: "Closed",
  dispatched: "Dispatched",
  worktree_leased: "Worktree leased",
  env_materialized: "Environment materialized",
  brief_composed: "Brief composed",
  session_started: "Session started",
  state_changed: "State changed",
  turn_ended: "Turn ended",
  needs_input: "Needs input",
  blocked: "Blocked",
  steered: "Steered",
  session_ended: "Session ended",
  gate_started: "Gate started",
  gate_step: "Gate step",
  finding_raised: "Finding raised",
  finding_resolved: "Finding resolved",
  gate_passed: "Gate passed",
  gate_failed: "Gate failed",
  pushed: "Pushed",
  pr_opened: "PR opened",
  ci_status: "CI status",
  merged: "Merged",
  worktree_returned: "Worktree returned",
  needs_you_raised: "Attention raised",
  needs_you_resolved: "Attention resolved",
  artifact_opened: "Plan artifact opened",
  plan_round: "Plan round posted",
  feedback_sent: "Feedback sent",
  plan_approved: "Plan approved",
  artifact_ended: "Plan session ended",
};

function title(kind: string): string {
  return KIND_TITLE[kind] ?? kind.replace(/_/g, " ");
}

function summarize(e: CardEvent): string | null {
  const p = e.payload ?? {};
  switch (e.kind) {
    case "moved":
      return p.from && p.to ? `${p.from} → ${p.to}` : null;
    case "dispatched":
      return [p.harness, p.recipe].filter(Boolean).join(" · ") || null;
    case "dial_changed":
      return p.to ? `dial -> ${p.to}` : "dial -> project default";
    case "plan_round":
      return p.round ? `round ${p.round}` : null;
    case "feedback_sent":
      return p.round ? `round ${p.round} · ${p.items ?? 0} item${p.items === 1 ? "" : "s"}` : null;
    case "state_changed":
      return p.to ? String(p.to) : null;
    case "turn_ended":
      return (p.note as string) ?? null;
    case "needs_input":
      return (p.question as string) ?? null;
    case "blocked":
      return (p.reason as string) ?? null;
    case "worktree_leased":
      return p.path ? `slot ${p.slot ?? "?"} · ${p.path}` : null;
    case "env_materialized":
      return `${p.files ?? 0} files, ${p.vars ?? 0} vars`;
    case "brief_composed":
      return p.tokens ? `${p.tokens} tokens` : null;
    case "gate_step":
      return [p.step, p.status].filter(Boolean).join(" · ") || null;
    case "pr_opened":
      return p.number ? `PR #${p.number}` : null;
    case "pushed":
      return (p.branch as string) ?? null;
    case "ci_status":
      return (p.state as string) ?? null;
    case "needs_you_raised":
      return p.kind ? `${p.kind}${p.score ? ` · score ${p.score}` : ""}` : null;
    case "created":
      return (p.title as string) ?? null;
    default: {
      const keys = Object.keys(p);
      if (keys.length === 0) return null;
      return keys.map((k) => `${k}: ${String((p as Record<string, unknown>)[k])}`).join(", ");
    }
  }
}

const ATTENTION = new Set(["needs_input", "blocked", "needs_you_raised", "gate_failed", "finding_raised"]);
const GOOD = new Set(["gate_passed", "merged", "pr_opened", "needs_you_resolved", "finding_resolved", "plan_approved"]);

export function TimelineTab({ cardId }: Props) {
  const store = useStore();
  const now = useNow(30_000);
  const [events, setEvents] = useState<CardEvent[] | null>(null);

  useEffect(() => {
    let cancelled = false;
    store
      .cardEvents(cardId)
      .then((list) => {
        if (!cancelled) setEvents(list.slice().sort((a, b) => a.ts - b.ts));
      })
      .catch(() => {
        if (!cancelled) setEvents([]);
      });
    return () => {
      cancelled = true;
    };
    // Re-pull when the card's live state advances (new events emitted).
  }, [cardId, store, store.sessions, store.cards]);

  if (events === null) {
    return <div className="timeline-loading">Loading timeline…</div>;
  }
  if (events.length === 0) {
    return (
      <div className="timeline-empty">
        <p>No events yet. When this card is dispatched, its history appears here as evidence: every state change, gate step, and PR link.</p>
      </div>
    );
  }

  return (
    <div className="timeline">
      <ol className="timeline-list">
        {events.map((e) => {
          const tone = ATTENTION.has(e.kind) ? "attention" : GOOD.has(e.kind) ? "good" : "neutral";
          const summary = summarize(e);
          return (
            <li key={e.id} className={`tl-item tl-${tone}`}>
              <span className="tl-node" aria-hidden>
                <EventIcon glyph={eventGlyph(e.kind)} />
              </span>
              <div className="tl-body">
                <div className="tl-row">
                  <span className="tl-title">{title(e.kind)}</span>
                  <span className="tl-time" title={new Date(e.ts).toLocaleString()}>
                    {clockTime(e.ts)} · {timeAgo(e.ts, now)}
                  </span>
                </div>
                {summary ? <div className="tl-summary">{summary}</div> : null}
              </div>
            </li>
          );
        })}
      </ol>
    </div>
  );
}
