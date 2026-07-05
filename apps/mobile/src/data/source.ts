// The phone's data-access seam. Every view reads through this one interface, so the
// fixture source (demo mode) and the live protocol source are drop-in interchangeable
// - the same seam the desktop uses (apps/desktop/src/data/source.ts). Methods map to
// the read/approve verbs the phone capability profile allows (security.md 5.2); nothing
// here can touch the vault, install a recipe, or type into a terminal.

import { StyledRun } from "../client/protocol";
import { FleetSnapshot, PlanArtifact } from "../client/model";

// The read-only terminal peek payload: a scrubbed styled screen capture (session.attach
// snapshot), never a live stream and never a raw secret-bearing scrollback (security.md).
export interface TerminalPeek {
  sessionId: string;
  cols: number;
  rows: number;
  lines: StyledRun[][];
  capturedAt: number;
  /** Whether the daemon's secret scrubber ran over this capture before it left the host. */
  scrubbed: boolean;
}

export interface ActionResult {
  ok: boolean;
  error?: string;
}

export interface MobileDataSource {
  /** "fixture" is demo mode; "live" talks to dflowd over the phone-scoped WS connection. */
  readonly mode: "fixture" | "live";

  /** fleet.status projection: sessions + the open Needs You queue (+ card titles for context). */
  loadFleet(): Promise<FleetSnapshot>;

  /** session.attach -> styled snapshot, then immediate detach. Read-only; poll to refresh. */
  peekSession(sessionId: string): Promise<TerminalPeek>;

  /** The plan artifact for a card (a plan_round item). Fixture, or artifact.get when present. */
  getPlan(cardId: string): Promise<PlanArtifact | null>;

  /** Approve a plan round with one overall feedback note (v1). artifact.feedback.submit live. */
  approvePlan(planId: string, feedback: string): Promise<ActionResult>;

  /**
   * Dismiss / acknowledge a Needs You item from the queue. Fixtures resolve it locally;
   * live has no dedicated resolve verb yet, so this is optimistic + noted (see live.ts).
   */
  dismissNeedsYou(itemId: string): Promise<ActionResult>;

  /** event.subscribe stream (live) or the fixture's scripted automation. Returns unsubscribe. */
  subscribeEvents(handler: () => void): () => void;

  /** DEV ONLY: demo controls, present on the fixture source, absent on live. */
  demo?: {
    /** Toggle the Needs You queue between populated and all-clear for screenshots. */
    setNeedsYouEmpty(empty: boolean): void;
    isNeedsYouEmpty(): boolean;
  };
}
