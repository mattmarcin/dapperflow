import { useMemo } from "react";
import { useStore } from "../state/store";
import { Card, NeedsYouItem, SessionState } from "../model";
import { STATE_META, STATE_ORDER, isLive, isResumable, ATTENTION_STATES } from "../lib/session-state";
import { StateChip } from "./StateChip";
import { StateGlyph } from "../lib/state-glyphs";
import { HarnessGlyph, harnessLabel, eventGlyph, EventIcon, NeedsYouIcon } from "../lib/glyphs";
import { needsYouMeta, scoreBand } from "../lib/needs-you";
import { deriveSessionTitle, elapsed, timeAgo } from "../lib/format";
import { useNow } from "../lib/use-now";

// Mission Control - the home view (product.md view 1). Opened in the morning, it must
// answer "what needs me, what is moving" in five seconds: a full-width fleet pulse
// (the signature - the fleet's state distribution as one glanceable meter), the Needs
// You queue as the hero, and the live fleet plus a cross-project activity feed beside it.

interface FleetRow {
  key: string;
  sessionId: string;
  agent: string;
  harness: string;
  title: string;
  projectName: string | null;
  state: SessionState;
  since: number;
  note?: string | null;
  onOpen: () => void;
}

export function MissionControl() {
  const store = useStore();
  const now = useNow();
  const { sessions, launches, cards, projects, needsYou } = store;

  const cardById = useMemo(() => new Map(cards.map((c) => [c.id, c])), [cards]);
  const projectName = (projectId: string | null | undefined) =>
    projectId ? projects.find((p) => p.id === projectId)?.name ?? null : null;

  // The live fleet = card-linked live sessions + cardless live launches, unified.
  const fleet = useMemo<FleetRow[]>(() => {
    const rows: FleetRow[] = [];
    for (const s of sessions) {
      // Live sessions and resumable (interrupted) ones both belong in the fleet: an
      // interrupted session must offer resume from Mission Control too (product.md
      // session resume), not only from the tree. Terminal done/error sessions drop off.
      if (!isLive(s.state) && !isResumable(s.state)) continue;
      const card = s.card_id ? cardById.get(s.card_id) : undefined;
      const agent = s.agent ?? harnessLabel(s.harness);
      rows.push({
        key: s.id,
        sessionId: s.id,
        agent,
        harness: s.harness,
        title:
          s.title ?? deriveSessionTitle({ firstPrompt: s.first_prompt, cardTitle: card?.title, fallback: agent }),
        // Prefer the daemon's resolved project name / cwd->project link so a cardless
        // session still shows its project (it has no card to borrow one from).
        projectName: s.project_name ?? projectName(s.project_id ?? card?.project_id),
        state: s.state,
        since: s.state_since,
        note: s.status_note,
        // The one-click-to-terminal invariant with attach-if-needed: a resumable session
        // opens its resume view; a live cardless session opens its terminal (attaching on
        // demand); a carded one opens the card workspace terminal tab.
        onOpen: () =>
          isResumable(s.state)
            ? store.openSession(s.id)
            : s.card_id
              ? store.openCard(s.card_id, "terminal")
              : store.openSession(s.id),
      });
    }
    for (const l of launches) {
      if (!l.alive) continue;
      const agent = l.agent;
      rows.push({
        key: l.sessionId,
        sessionId: l.sessionId,
        agent,
        harness: l.harness,
        title: l.title ?? deriveSessionTitle({ firstPrompt: l.firstPrompt, fallback: agent }),
        projectName: projectName(l.projectId),
        state: "working",
        since: l.createdAt,
        note: null,
        onOpen: () => store.openSession(l.sessionId),
      });
    }
    // Attention first, then longest-in-state (a session stuck the longest ranks up).
    return rows.sort((a, b) => {
      const aw = ATTENTION_STATES.includes(a.state) ? 0 : 1;
      const bw = ATTENTION_STATES.includes(b.state) ? 0 : 1;
      if (aw !== bw) return aw - bw;
      return a.since - b.since;
    });
  }, [sessions, launches, cardById, projects, store]);

  // Fleet pulse: count live sessions by state for the signature meter.
  const counts = useMemo(() => {
    const map = new Map<SessionState, number>();
    for (const r of fleet) map.set(r.state, (map.get(r.state) ?? 0) + 1);
    return map;
  }, [fleet]);

  const rankedNeedsYou = useMemo(() => [...needsYou].sort((a, b) => b.score - a.score), [needsYou]);

  const greeting = greetingForHour(new Date(now).getHours());
  const projectCount = new Set(fleet.map((r) => r.projectName).filter(Boolean)).size;

  return (
    <div className="mission">
      <header className="mc-head">
        <div className="mc-head-line">
          <h1 className="mc-title">Mission Control</h1>
          <p className="mc-greeting">
            {greeting}. <Digest needsYou={rankedNeedsYou.length} fleet={fleet.length} projects={projectCount} />
          </p>
        </div>
        <FleetPulse counts={counts} total={fleet.length} />
      </header>

      <div className="mc-grid">
        <section className="mc-panel mc-needs" aria-label="Needs You">
          <div className="mc-panel-head">
            <h2 className="mc-panel-title">Needs you</h2>
            {rankedNeedsYou.length > 0 ? (
              <span className="mc-panel-count is-attention">{rankedNeedsYou.length}</span>
            ) : null}
          </div>
          {rankedNeedsYou.length === 0 ? (
            <AllClear />
          ) : (
            <ol className="ny-list">
              {rankedNeedsYou.map((item, i) => (
                <NeedsYouRow
                  key={item.id}
                  rank={i + 1}
                  item={item}
                  card={cardById.get(item.card_id)}
                  projectName={projectName(cardById.get(item.card_id)?.project_id)}
                  now={now}
                  onOpen={() => store.openNeedsYou(item)}
                />
              ))}
            </ol>
          )}
        </section>

        <div className="mc-side">
          <section className="mc-panel mc-fleet" aria-label="Fleet">
            <div className="mc-panel-head">
              <h2 className="mc-panel-title">Fleet</h2>
              {fleet.length > 0 ? <span className="mc-panel-count">{fleet.length}</span> : null}
            </div>
            {fleet.length === 0 ? (
              <p className="mc-empty-line">No live sessions. Start one from the New session button.</p>
            ) : (
              <ul className="fleet-list">
                {fleet.map((r) => (
                  <FleetRowView key={r.key} row={r} now={now} />
                ))}
              </ul>
            )}
          </section>

          <section className="mc-panel mc-activity" aria-label="Recent activity">
            <div className="mc-panel-head">
              <h2 className="mc-panel-title">Recent activity</h2>
            </div>
            <ActivityFeed cards={cardById} now={now} />
          </section>
        </div>
      </div>

      <StateLegend />
    </div>
  );
}

// The lifecycle-state reference: every state's designed chip and what it means. A
// disclosure so it never crowds the morning view, and a single gallery of the chip
// system.
function StateLegend() {
  return (
    <details className="mc-legend">
      <summary className="mc-legend-summary">
        <span>Lifecycle states</span>
        <span className="mc-legend-hint">what each chip means</span>
      </summary>
      <div className="state-legend">
        {STATE_ORDER.map((state) => (
          <div key={state} className="state-legend-item">
            <span className="state-legend-chip">
              <StateChip state={state} />
            </span>
            <span className="state-legend-text">
              <span className="state-legend-name">{STATE_META[state].label}</span>
              <span className="state-legend-desc">{STATE_META[state].description}</span>
            </span>
          </div>
        ))}
      </div>
    </details>
  );
}

function Digest({ needsYou, fleet, projects }: { needsYou: number; fleet: number; projects: number }) {
  if (fleet === 0 && needsYou === 0) return <span>The fleet is quiet.</span>;
  const parts: string[] = [];
  if (needsYou > 0) parts.push(`${needsYou} ${needsYou === 1 ? "item needs" : "items need"} you`);
  parts.push(`${fleet} ${fleet === 1 ? "session" : "sessions"} live`);
  if (projects > 0) parts.push(`across ${projects} ${projects === 1 ? "project" : "projects"}`);
  return <span>{parts.join(", ")}.</span>;
}

// The signature: the fleet's state distribution as one segmented meter.
function FleetPulse({ counts, total }: { counts: Map<SessionState, number>; total: number }) {
  const present = STATE_ORDER.filter((s) => (counts.get(s) ?? 0) > 0);
  return (
    <div className="fleet-pulse" role="img" aria-label={`Fleet: ${total} live sessions`}>
      <div className="fleet-pulse-bar">
        {total === 0 ? (
          <div className="fleet-pulse-empty" />
        ) : (
          present.map((s) => {
            const n = counts.get(s) ?? 0;
            const meta = STATE_META[s];
            return (
              <div
                key={s}
                className={`fleet-pulse-seg tone-${meta.tone}${meta.demand ? " is-demand" : ""}`}
                style={{ flexGrow: n }}
                title={`${n} ${meta.label.toLowerCase()}`}
              />
            );
          })
        )}
      </div>
      <div className="fleet-pulse-legend">
        {present.map((s) => (
          <span key={s} className={`fp-key tone-${STATE_META[s].tone}`}>
            <span className="fp-key-dot" aria-hidden />
            {counts.get(s)} {STATE_META[s].label.toLowerCase()}
          </span>
        ))}
      </div>
    </div>
  );
}

function AllClear() {
  return (
    <div className="all-clear">
      <div className="all-clear-mark" aria-hidden>
        <StateGlyph state="done" size={26} />
      </div>
      <p className="all-clear-title">All clear</p>
      <p className="all-clear-body">Nothing needs you right now. The fleet runs itself until something does.</p>
    </div>
  );
}

function NeedsYouRow({
  rank,
  item,
  card,
  projectName,
  now,
  onOpen,
}: {
  rank: number;
  item: NeedsYouItem;
  card?: Card;
  projectName: string | null;
  now: number;
  onOpen: () => void;
}) {
  const meta = needsYouMeta(item.kind);
  const band = scoreBand(item.score);
  return (
    <li className={`ny-item tone-${meta.tone} band-${band}`}>
      <button className="ny-open" onClick={onOpen} title={`${meta.verb} - opens the ${meta.tab} tab`}>
        <span className="ny-rank" aria-hidden>
          {rank}
        </span>
        <span className={`ny-glyph tone-${meta.tone}`} aria-hidden>
          <NeedsYouIcon glyph={meta.glyph} />
        </span>
        <span className="ny-text">
          <span className="ny-title">{card?.title ?? meta.label}</span>
          <span className="ny-meta">
            <span className="ny-kind">{meta.label}</span>
            {projectName ? <span className="ny-proj">· {projectName}</span> : null}
            <span className="ny-age">· {timeAgo(item.raised_at, now)}</span>
          </span>
          {item.note ? <span className="ny-note">{item.note}</span> : null}
        </span>
        <span className="ny-action">
          <span className="ny-verb">{meta.verb}</span>
          <svg width="14" height="14" viewBox="0 0 16 16" fill="none" aria-hidden>
            <path d="M6 3.5L10.5 8 6 12.5" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" />
          </svg>
        </span>
      </button>
    </li>
  );
}

function FleetRowView({ row, now }: { row: FleetRow; now: number }) {
  return (
    <li className="fleet-row">
      <button className="fleet-row-btn" onClick={row.onOpen} title={`Open ${row.agent} terminal`}>
        <span className="fleet-glyph" aria-hidden>
          <HarnessGlyph harness={row.harness} />
        </span>
        <span className="fleet-main">
          <span className="fleet-line1">
            <span className="fleet-title">{row.title}</span>
            <span className="fleet-agent">{row.agent}</span>
          </span>
          <span className="fleet-line2">
            {row.projectName ? <span className="fleet-proj">{row.projectName}</span> : <span className="fleet-proj is-none">no project</span>}
            {row.note ? <span className="fleet-note">· {row.note}</span> : null}
          </span>
        </span>
        <span className="fleet-right">
          <StateChip state={row.state} variant="tab" />
          <span className="fleet-elapsed">{elapsed(row.since, now)}</span>
        </span>
      </button>
    </li>
  );
}

function ActivityFeed({ cards, now }: { cards: Map<string, Card>; now: number }) {
  const { recentEvents } = useStore();
  if (recentEvents.length === 0) {
    return <p className="mc-empty-line">Quiet across the fleet. Activity appears here as agents work.</p>;
  }
  return (
    <ul className="activity-list">
      {recentEvents.slice(0, 40).map((e) => {
        const card = cards.get(e.card_id);
        return (
          <li key={e.id} className="activity-row">
            <span className="activity-glyph" aria-hidden>
              <EventIcon glyph={eventGlyph(e.kind)} />
            </span>
            <span className="activity-text">
              <span className="activity-kind">{humanizeEvent(e.kind)}</span>
              {card ? <span className="activity-card"> · {card.title}</span> : null}
            </span>
            <span className="activity-time">{timeAgo(e.ts, now)}</span>
          </li>
        );
      })}
    </ul>
  );
}

function greetingForHour(h: number): string {
  if (h < 5) return "Late shift";
  if (h < 12) return "Good morning";
  if (h < 18) return "Good afternoon";
  return "Good evening";
}

function humanizeEvent(kind: string): string {
  const map: Record<string, string> = {
    created: "Card created",
    shaped: "Brief shaped",
    moved: "Moved lane",
    dispatched: "Dispatched",
    worktree_leased: "Worktree leased",
    env_materialized: "Environment ready",
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
    ci_status: "CI update",
    merged: "Merged",
    needs_you_raised: "Needs you",
    needs_you_resolved: "Resolved",
    plan_round: "Plan round",
    plan_approved: "Plan approved",
  };
  return map[kind] ?? kind.replace(/_/g, " ");
}
