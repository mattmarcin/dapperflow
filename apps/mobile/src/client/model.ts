// ============================================================================
// M6 DEBT - thin duplicated domain mirror.
// A phone-scoped subset of apps/desktop/src/model.ts, which in turn mirrors
// docs/spec/protocol.md payloads and docs/spec/data-model.md entities EXACTLY.
// The phone only needs the read surfaces of its capability profile (security.md):
// the fleet, the Needs You queue, session snapshots, and plan artifacts. Cards
// are carried only for the title/project context a Needs You item deep-links to.
// ============================================================================

export type Lane =
  | "inbox"
  | "shaping"
  | "ready"
  | "performing"
  | "verifying"
  | "needs_you"
  | "pr"
  | "done";

export type CardType = "feature" | "bug" | "chore" | "test" | "investigation";

export type Harness = "claude" | "codex" | "opencode" | "pi" | "cursor" | (string & {});

// sessions.state (data-model.md). Unknown states tolerated and rendered generically.
export type SessionState =
  | "starting"
  | "working"
  | "idle"
  | "needs_input"
  | "awaiting_feedback"
  | "blocked"
  | "done"
  | "error"
  | "interrupted"
  | (string & {});

export interface Project {
  id: string;
  name: string;
  path: string;
}

// Carried for deep-link context only (the phone does not render a board).
export interface Card {
  id: string;
  project_id: string | null;
  type: CardType | string;
  title: string;
  lane: Lane | string;
  priority: number;
}

// The compact fleet row (fleet.status / session.list -> dflow-proto SessionSummary),
// shaped into the phone's session strip: harness glyph, state chip, elapsed, note.
export interface Session {
  id: string;
  card_id: string | null;
  project_id: string | null;
  project_name: string | null;
  harness: Harness;
  agent: string | null;
  title: string | null;
  model: string | null;
  state: SessionState;
  first_prompt: string | null;
  // ms epoch the session entered `state`; drives elapsed-in-state. On the wire v0
  // this approximates as created-at (see live.ts), sharpened when tier-1 signals land.
  state_since: number;
  stage: string | null;
  status_note: string | null;
}

// needs_you_items.kind (data-model.md). Unknown kinds render generically and never drop.
export type NeedsYouKind =
  | "plan_round"
  | "gate_finding"
  | "agent_blocked"
  | "agent_stuck"
  | "agent_needs_input"
  | "trust_dialog"
  | "pr_ready"
  | "env_drift"
  | "service_failed"
  | (string & {});

// The open Needs You item as carried by fleet.status (dflow-proto NeedsYouItem).
export interface NeedsYouItem {
  id: string;
  card_id: string;
  kind: NeedsYouKind;
  dedupe_key: string;
  score: number; // priority + staleness + cost-of-delay; higher ranks first
  raised_at: number;
  note: string | null;
}

// The phone's board projection: everything the attention surface reads in one pull.
// Composed live from fleet.status (+ card.query for titles); served whole by fixtures.
export interface FleetSnapshot {
  projects: Project[];
  cards: Card[];
  sessions: Session[];
  needsYou: NeedsYouItem[];
}

// --- Plan review (read-only-plus-approve) ----------------------------------
// A plan artifact for the phone. Live artifacts are HTML served over the daemon's
// HTTP endpoint via short-lived signed URLs (security.md); the phone renders that
// document read-only in a sandboxed frame. When no artifact endpoint is reachable,
// a structured fixture stands in so the review view always demos. v1 supports
// exactly two actions: approve, and one overall chat-feedback note.

export interface PlanSection {
  heading: string;
  body: string[]; // paragraphs / bullet lines, rendered read-only
}

export interface PlanArtifact {
  id: string;
  card_id: string;
  card_title: string;
  project_name: string | null;
  round: number;
  // A structured read-only rendering (fixtures, and a graceful fallback for live).
  summary: string;
  sections: PlanSection[];
  // Signed-URL HTML document when the daemon serves one (rendered in a sandboxed
  // iframe). Absent in fixtures and until the artifact endpoint lands.
  html?: string | null;
  // The phone-viewport layout audit (mobile.md 2.4): warnings the daemon reports at
  // 390px width. Empty when clean or unavailable.
  layoutWarnings: string[];
  status: "awaiting_review" | "approved";
}
