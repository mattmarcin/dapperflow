// The session-lifecycle vocabulary, the phone-scoped twin of the desktop's
// lib/session-state.ts. A state reads at a glance from three signals - color (tone),
// shape (glyph), word (label) - rendered identically by <StateChip> everywhere.

import { SessionState } from "../client/model";

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
  demand: boolean; // needs_input + blocked: the eye MUST land here
  description: string;
}

const KNOWN: Record<string, StateMeta> = {
  starting: { label: "Starting", tone: "quiet", pulse: true, demand: false, description: "Booting the harness." },
  working: { label: "Working", tone: "working", pulse: true, demand: false, description: "Actively making progress." },
  idle: { label: "Idle", tone: "quiet", pulse: false, demand: false, description: "Waiting, nothing in flight." },
  needs_input: { label: "Needs input", tone: "attention", pulse: true, demand: true, description: "Waiting on your answer." },
  awaiting_feedback: { label: "Awaiting feedback", tone: "review", pulse: true, demand: false, description: "Parked on a plan review." },
  blocked: { label: "Blocked", tone: "trouble", pulse: true, demand: true, description: "Stuck; needs you to clear the way." },
  done: { label: "Done", tone: "done", pulse: false, demand: false, description: "Finished its work." },
  error: { label: "Error", tone: "trouble", pulse: false, demand: false, description: "Stopped on an error." },
  interrupted: { label: "Interrupted", tone: "interrupted", pulse: false, demand: false, description: "Daemon restarted; resumable." },
};

// Unknown states render generically (protocol.md: never drop unknown kinds).
export function stateMeta(state: SessionState): StateMeta {
  return (
    KNOWN[state] ?? {
      label: humanize(String(state)),
      tone: "quiet",
      pulse: false,
      demand: false,
      description: "Unknown state.",
    }
  );
}

function humanize(s: string): string {
  const t = s.replace(/_/g, " ").trim();
  return t.charAt(0).toUpperCase() + t.slice(1);
}

const ATTENTION: SessionState[] = ["needs_input", "awaiting_feedback", "blocked"];
export function isAttentionState(state: SessionState): boolean {
  return ATTENTION.includes(state);
}
