// The session-lifecycle system: the one new semantic axis the live surfaces add to
// the brass-on-ink design. A state is read at a glance from a chip that carries three
// signals - color (tone), shape (glyph), and word (label) - rendered identically
// everywhere by <StateChip> so a session reads the same on a board card, a tree row,
// a session tab, and Mission Control.
//
// Tones are drawn from the existing palette (mint = healthy, brass = attention,
// violet = review, red = trouble, blue = resumable) rather than a new rainbow.
// `pulse` marks live activity; `demand` marks the two states that must pull the eye
// (needs_input, blocked) with a stronger glow than an ordinary live pulse.

import { SessionState } from "../model";

export type StateTone =
  | "working"
  | "attention"
  | "review"
  | "trouble"
  | "done"
  | "quiet"
  | "interrupted";

export interface StateMeta {
  label: string;
  tone: StateTone;
  pulse: boolean;
  // needs_input + blocked: the eye MUST land here. Rendered with an outer glow ring
  // on top of the pulse, so they out-shout an ordinary working session.
  demand: boolean;
  // End-user-facing one-liner, used in the state legend and chip tooltips.
  description: string;
}

export const STATE_META: Record<SessionState, StateMeta> = {
  starting: {
    label: "Starting",
    tone: "quiet",
    pulse: true,
    demand: false,
    description: "Booting the harness.",
  },
  working: {
    label: "Working",
    tone: "working",
    pulse: true,
    demand: false,
    description: "Actively making progress.",
  },
  idle: {
    label: "Idle",
    tone: "quiet",
    pulse: false,
    demand: false,
    description: "Waiting, nothing in flight.",
  },
  needs_input: {
    label: "Needs input",
    tone: "attention",
    pulse: true,
    demand: true,
    description: "Waiting on your answer.",
  },
  awaiting_feedback: {
    label: "Awaiting feedback",
    tone: "review",
    pulse: true,
    demand: false,
    description: "Parked on a plan review.",
  },
  blocked: {
    label: "Blocked",
    tone: "trouble",
    pulse: true,
    demand: true,
    description: "Stuck; needs you to clear the way.",
  },
  done: {
    label: "Done",
    tone: "done",
    pulse: false,
    demand: false,
    description: "Finished its work.",
  },
  error: {
    label: "Error",
    tone: "trouble",
    pulse: false,
    demand: false,
    description: "Stopped on an error.",
  },
  interrupted: {
    label: "Interrupted",
    tone: "interrupted",
    pulse: false,
    demand: false,
    description: "Daemon restarted; resumable with full context.",
  },
};

// The state order used by the legend and the fleet pulse meter: live and
// attention-demanding first, terminal states last.
export const STATE_ORDER: SessionState[] = [
  "working",
  "needs_input",
  "blocked",
  "awaiting_feedback",
  "starting",
  "idle",
  "interrupted",
  "done",
  "error",
];

// States that mean "a human should look" - these bubble a card toward Needs You
// weighting and get the demand treatment on every surface.
export const ATTENTION_STATES: SessionState[] = ["needs_input", "awaiting_feedback", "blocked"];

// A live session holds a worktree lease and may still be steered.
export function isLive(state: SessionState): boolean {
  return state !== "done" && state !== "error" && state !== "interrupted";
}

// An interrupted session is resumable: its PTY is gone but the harness transcript
// and worktree survive (architecture.md, session resume).
export function isResumable(state: SessionState): boolean {
  return state === "interrupted";
}
