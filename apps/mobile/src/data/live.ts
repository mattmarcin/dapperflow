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
import { Envelope, SessionAttached } from "../client/protocol";
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
  model?: string | null;
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
  note?: string | null;
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
    model: w.model ?? null,
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

    const projects = await this.client
      .call<{ projects: Project[] }>("project.list", {})
      .then((r) => r.projects)
      .catch(() => [] as Project[]);

    const sessions = (fleet.sessions ?? []).map(sessionFromWire);
    const needsYou = (fleet.needs_you ?? [])
      .map(
        (w): NeedsYouItem => ({
          id: w.id,
          card_id: w.card_id,
          kind: w.kind,
          dedupe_key: w.dedupe_key,
          score: w.score,
          raised_at: w.raised_at,
          note: w.note ?? null,
        }),
      )
      .sort((a, b) => b.score - a.score);

    return { projects, cards, sessions, needsYou };
  }

  async peekSession(sessionId: string): Promise<TerminalPeek> {
    // session.attach replays a styled snapshot, then the client immediately detaches:
    // read-only, no live stream, no input (the phone never holds a PTY).
    const attached: SessionAttached = await this.client.peek(sessionId, 80, 24);
    return {
      sessionId,
      cols: attached.snapshot.cols,
      rows: attached.snapshot.rows,
      lines: attached.snapshot.lines,
      capturedAt: Date.now(),
      // The daemon scrubs known secret values from any capture that leaves a session
      // (security.md); the phone trusts that scrub and never requests raw scrollback.
      scrubbed: true,
    };
  }

  async getPlan(cardId: string): Promise<PlanArtifactOrNull> {
    // INTERPRET: card.get returns the card plus its artifacts (protocol.md). Find the
    // plan artifact, then artifact.get for the served HTML + layout audit. The phone
    // renders that document read-only; approve + one comment are the only writes.
    try {
      const got = await this.client.call<{
        card?: WireCard;
        artifacts?: { id: string; kind: string }[];
      }>("card.get", { card_id: cardId });
      const planArtifact = (got.artifacts ?? []).find((a) => a.kind === "plan");
      if (!planArtifact) return null;
      const doc = await this.client.call<{
        html?: string;
        layout_warnings?: string[];
        round?: number;
      }>("artifact.get", { artifact_id: planArtifact.id });
      return {
        id: planArtifact.id,
        card_id: cardId,
        card_title: got.card?.title ?? "Plan",
        project_name: null,
        round: doc.round ?? 1,
        status: "awaiting_review",
        summary: "",
        sections: [],
        html: doc.html ?? null,
        layoutWarnings: doc.layout_warnings ?? [],
      };
    } catch {
      // Artifact endpoints not present yet: the view falls back to its fixture plan.
      return null;
    }
  }

  async approvePlan(planId: string, feedback: string): Promise<ActionResult> {
    // INTERPRET: artifact.feedback.submit carries the review batch (plan-studio.md).
    // v1 phone review = one approve item plus one overall comment when provided.
    try {
      const items: Record<string, unknown>[] = [{ kind: "approve" }];
      if (feedback.trim()) items.push({ kind: "comment", scope: "overall", text: feedback.trim() });
      await this.client.call("artifact.feedback.submit", { artifact_id: planId, items });
      return { ok: true };
    } catch (e) {
      return { ok: false, error: errorMessage(e) };
    }
  }

  async dismissNeedsYou(_itemId: string): Promise<ActionResult> {
    // No dedicated resolve verb exists yet: resolution happens on the resolving surface
    // (approve, merge, answer). Dismiss is therefore an optimistic client-side removal;
    // the daemon reconciles the true state on the next fleet.status. Documented debt.
    void _itemId;
    return { ok: true };
  }

  subscribeEvents(handler: () => void): () => void {
    const prev = this.client.onEvent;
    this.client.onEvent = (env: Envelope) => {
      prev?.(env);
      const ev = env.payload as { kind?: string } | undefined;
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

function errorMessage(e: unknown): string {
  if (e && typeof e === "object" && "message" in e) return String((e as { message: unknown }).message);
  return String(e);
}

// Local alias to keep the import list tight (PlanArtifact is re-used from the model).
type PlanArtifactOrNull = import("../client/model").PlanArtifact | null;
