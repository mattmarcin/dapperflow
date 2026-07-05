// LiveMobileSource: the attention surface over the real dflowd protocol, spoken by a
// phone-scoped connection (client: mobile). Coded against docs/spec/protocol.md payload
// shapes. Only the read/approve verbs of the phone capability profile are used; the
// daemon enforces scope, this source never even forms a vault or recipe request.
//
// Reality note (2026-07-04): the daemon LAN listener and pairing endpoints do not exist
// yet, and card.*/artifact.* are being built in parallel. Where a verb is specified but
// its exact response object is not, the interpretation is marked INTERPRET and the call
// degrades gracefully (empty/null) rather than throwing, so a partially-built daemon
// still lights up fleet.status and session.attach (the VERIFY targets).

import { DflowMobileClient } from "../client/client";
import { Envelope, SessionPeeked, StyledRun } from "../client/protocol";
import { Card, FleetSnapshot, NeedsYouItem, Project, Session } from "../client/model";
import { ActionResult, MobileDataSource, TerminalPeek } from "./source";

interface WireSessionSummary {
  session_id: string;
  card_id?: string | null;
  project_id?: string | null;
  project_name?: string | null;
  harness: string;
  agent?: string | null;
  title?: string | null;
  state: string;
  alive: boolean;
  elapsed_ms: number;
  first_prompt?: string | null;
  created_at_ms: number;
}

interface WireNeedsYouItem {
  id: string;
  card_id: string;
  kind: string;
  dedupe_key: string;
  score: number;
  raised_at: number;
}

interface WireCard {
  id: string;
  project_id?: string | null;
  type?: string;
  title: string;
  lane?: string;
  priority?: number;
}

function sessionFromWire(w: WireSessionSummary): Session {
  return {
    id: w.session_id,
    card_id: w.card_id ?? null,
    project_id: w.project_id ?? null,
    project_name: w.project_name ?? null,
    harness: w.harness,
    agent: w.agent ?? null,
    title: w.title ?? null,
    // SessionSummary carries no `model` field; the phone strip leaves it null rather than
    // reading a key the daemon never sends.
    model: null,
    state: w.state,
    first_prompt: w.first_prompt ?? null,
    // v0 wire carries elapsed-since-creation only; state-transition timestamps arrive
    // with tier-1 signals, so elapsed-in-state approximates until then (matches desktop).
    state_since: Date.now() - w.elapsed_ms,
    stage: null,
    status_note: null,
  };
}

export class LiveMobileSource implements MobileDataSource {
  readonly mode = "live" as const;

  // needs_you.resolve needs { card_id, dedupe_key }, but the views dismiss by item id, so
  // index the open items from the last fleet snapshot to recover the resolve key on dismiss.
  private readonly needsYouIndex = new Map<string, { card_id: string; dedupe_key: string }>();

  constructor(private readonly client: DflowMobileClient) {}

  async loadFleet(): Promise<FleetSnapshot> {
    const fleet = await this.client
      .call<{ sessions: WireSessionSummary[]; needs_you?: WireNeedsYouItem[] }>("fleet.status", {})
      .catch(() => ({ sessions: [] as WireSessionSummary[], needs_you: [] as WireNeedsYouItem[] }));

    // Card titles give Needs You items and session strips their deep-link context.
    // card.query may not exist on a partial daemon; degrade to empty.
    const cards = await this.client
      .call<{ cards: WireCard[] }>("card.query", { filter: {} })
      .then((r) => r.cards.map(cardFromWire))
      .catch(() => [] as Card[]);

    const sessions = (fleet.sessions ?? []).map(sessionFromWire);

    // project.list is NOT in the phone scope (dispatch_phone routes no project verbs), so
    // calling it would only ever be forbidden. Source project names from the session rows
    // (fleet.status joins project_name), which is all the phone deep links need.
    const projects = projectsFromSessions(sessions);

    this.needsYouIndex.clear();
    const needsYou = (fleet.needs_you ?? [])
      .map((w): NeedsYouItem => {
        this.needsYouIndex.set(w.id, { card_id: w.card_id, dedupe_key: w.dedupe_key });
        return {
          id: w.id,
          card_id: w.card_id,
          kind: w.kind,
          dedupe_key: w.dedupe_key,
          score: w.score,
          raised_at: w.raised_at,
          // NeedsYouItem has no `note` field on the wire; leave it null.
          note: null,
        };
      })
      .sort((a, b) => b.score - a.score);

    return { projects, cards, sessions, needsYou };
  }

  async peekSession(sessionId: string): Promise<TerminalPeek> {
    // session.peek returns SessionPeeked { session_id, lines, text }: a scrubbed, bounded
    // PLAIN-TEXT screen tail (no styled snapshot, no attach). Split the text into unstyled
    // runs for the peek renderer. `lines` is the returned line count; cols is the widest
    // line. Read-only: no live stream, no input (session.attach is forbidden on the phone).
    const peeked: SessionPeeked = await this.client.peek(sessionId, 40);
    const textLines = peeked.text.split("\n");
    const lines: StyledRun[][] = textLines.map((line) => [{ text: line }]);
    return {
      sessionId,
      cols: textLines.reduce((max, line) => Math.max(max, line.length), 0) || 80,
      rows: peeked.lines || textLines.length,
      lines,
      capturedAt: Date.now(),
      // The daemon scrubs known secret values from any capture that leaves a session
      // (security.md); the phone trusts that scrub and never requests raw scrollback.
      scrubbed: true,
    };
  }

  async getPlan(cardId: string): Promise<PlanArtifactOrNull> {
    // card.get returns the card plus its ArtifactMeta[] (protocol.md). Find the plan
    // artifact, then artifact.get -> ArtifactGetResult { artifact, signed_url, expires_at,
    // layout_warnings: LayoutWarning[] }. There is NO inline html: fetch the document over
    // the short-lived signed URL. The round lives on `artifact.round`, and layout_warnings
    // are structured objects (not strings), so summarize each into a one-line label.
    try {
      const got = await this.client.call<{
        card?: WireCard;
        artifacts?: { id: string; kind: string; round?: number }[];
      }>("card.get", { card_id: cardId });
      const planArtifact = (got.artifacts ?? []).find((a) => a.kind === "plan");
      if (!planArtifact) return null;
      const doc = await this.client.call<{
        artifact: { id: string; round: number };
        signed_url: string;
        expires_at: number;
        layout_warnings?: WireLayoutWarning[];
      }>("artifact.get", { artifact_id: planArtifact.id });
      let html: string | null = null;
      try {
        const res = await fetch(doc.signed_url);
        if (res.ok) html = await res.text();
      } catch {
        // The signed artifact endpoint may be unreachable; fall back to the structured view.
        html = null;
      }
      return {
        id: planArtifact.id,
        card_id: cardId,
        card_title: got.card?.title ?? "Plan",
        project_name: null,
        round: doc.artifact?.round ?? 1,
        status: "awaiting_review",
        summary: "",
        sections: [],
        html,
        layoutWarnings: (doc.layout_warnings ?? []).map(layoutWarningLabel),
      };
    } catch {
      // Artifact endpoints not present yet: the view falls back to its fixture plan.
      return null;
    }
  }

  async approvePlan(planId: string, feedback: string): Promise<ActionResult> {
    // artifact.feedback.submit carries the review batch (plan-studio.md). The valid
    // FeedbackItem kinds are text_range|control|diagram_node|action|chat|element - NOT
    // "approve"/"comment". Approve is an `action` item (approve_plan); the overall note is
    // a `chat` item whose body is the feedback. The old kinds were silently dropped.
    try {
      const items: Record<string, unknown>[] = [{ kind: "action", action: "approve_plan" }];
      if (feedback.trim()) items.push({ kind: "chat", body: feedback.trim() });
      await this.client.call("artifact.feedback.submit", { artifact_id: planId, items });
      return { ok: true };
    } catch (e) {
      return { ok: false, error: errorMessage(e) };
    }
  }

  async dismissNeedsYou(itemId: string): Promise<ActionResult> {
    // dispatch_phone DOES route needs_you.resolve { card_id, dedupe_key }, so actually
    // resolve the item instead of only optimistically dropping it (which let dismissed
    // items reappear on the next fleet.status). The views dismiss by item id; recover the
    // resolve key from the last fleet snapshot's index.
    const key = this.needsYouIndex.get(itemId);
    if (!key) {
      // Not in the current snapshot (already resolved elsewhere): treat as done so the UI
      // does not get stuck; the next fleet.status reconciles.
      return { ok: true };
    }
    try {
      await this.client.call("needs_you.resolve", {
        card_id: key.card_id,
        dedupe_key: key.dedupe_key,
      });
      this.needsYouIndex.delete(itemId);
      return { ok: true };
    } catch (e) {
      return { ok: false, error: errorMessage(e) };
    }
  }

  subscribeEvents(handler: () => void): () => void {
    const prev = this.client.onEvent;
    this.client.onEvent = (env: Envelope) => {
      prev?.(env);
      // event.subscribe delivers Envelope::event("event.card_event", EventCardEvent
      // { event }), so the card_event row is NESTED under `payload.event`. Reading it flat
      // left `kind` undefined and the store never re-pulled on live updates.
      const ev = (env.payload as { event?: { kind?: string } } | undefined)?.event;
      // Any card_event may change the fleet or the queue; the store re-pulls on notify.
      if (ev && ev.kind) handler();
    };
    this.client.call("event.subscribe", {}).catch(() => undefined);
    return () => {
      this.client.onEvent = prev;
    };
  }
}

function cardFromWire(w: WireCard): Card {
  return {
    id: w.id,
    project_id: w.project_id ?? null,
    type: w.type ?? "feature",
    title: w.title,
    lane: w.lane ?? "inbox",
    priority: w.priority ?? 0,
  };
}

// A layout-audit finding (dflow-proto LayoutWarning): structured, not a string.
interface WireLayoutWarning {
  selector: string;
  kind: string;
  overflow_px?: number;
  viewport_width?: number;
  severity: string;
}

// Summarize a layout warning into the one-line label the phone review view joins.
function layoutWarningLabel(w: WireLayoutWarning): string {
  const kind = w.kind.replace(/_/g, " ");
  return w.selector ? `${kind} at ${w.selector}` : kind;
}

// Derive the projects the phone needs (id + name for deep-link context) from the session
// rows, since project.list is not in the phone scope. Path is unknown from a session row.
function projectsFromSessions(sessions: Session[]): Project[] {
  const byId = new Map<string, Project>();
  for (const s of sessions) {
    if (s.project_id && !byId.has(s.project_id)) {
      byId.set(s.project_id, {
        id: s.project_id,
        name: s.project_name ?? s.project_id,
        path: "",
      });
    }
  }
  return [...byId.values()];
}

function errorMessage(e: unknown): string {
  if (e && typeof e === "object" && "message" in e) return String((e as { message: unknown }).message);
  return String(e);
}

// Local alias to keep the import list tight (PlanArtifact is re-used from the model).
type PlanArtifactOrNull = import("../client/model").PlanArtifact | null;
