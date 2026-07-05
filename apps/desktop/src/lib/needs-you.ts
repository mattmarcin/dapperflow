// The Attention Router's vocabulary: how each Needs You kind (data-model.md
// needs_you_items.kind) reads to a human and which card-workspace tab resolves it.
// product.md: "Each item deep-links into the exact card workspace tab that resolves
// it." Unknown kinds fall back to a generic label and the timeline.

import { WorkspaceTab } from "../state/store";

export interface NeedsYouMeta {
  label: string; // what the item is, in the user's terms
  verb: string; // the action that clears it (button label)
  tab: WorkspaceTab; // the resolving workspace tab (deep-link target)
  glyph: NeedsYouGlyph; // icon family
  tone: "attention" | "trouble" | "review" | "delivery"; // urgency color
}

export type NeedsYouGlyph = "hand" | "barrier" | "clock" | "shield" | "branch" | "plug" | "doc" | "dot";

export const NEEDS_YOU_META: Record<string, NeedsYouMeta> = {
  plan_round: { label: "Plan awaiting review", verb: "Review plan", tab: "plan", glyph: "doc", tone: "review" },
  gate_finding: { label: "Gate finding needs judgment", verb: "Review finding", tab: "verify", glyph: "shield", tone: "review" },
  agent_blocked: { label: "Agent blocked", verb: "Unblock", tab: "terminal", glyph: "barrier", tone: "trouble" },
  agent_stuck: { label: "Agent stuck", verb: "Check in", tab: "terminal", glyph: "clock", tone: "attention" },
  agent_needs_input: { label: "Agent needs input", verb: "Answer", tab: "terminal", glyph: "hand", tone: "attention" },
  trust_dialog: { label: "Trust prompt hit", verb: "Decide", tab: "terminal", glyph: "shield", tone: "attention" },
  pr_ready: { label: "PR ready to merge", verb: "Review PR", tab: "verify", glyph: "branch", tone: "delivery" },
  env_drift: { label: "Environment drift", verb: "Reconcile", tab: "timeline", glyph: "plug", tone: "trouble" },
  service_failed: { label: "Service failed to start", verb: "Fix service", tab: "timeline", glyph: "plug", tone: "trouble" },
  // Resolved on the board, not in a card tab: the store deep-links an audit digest
  // to the Inbox filtered to the audited project (product.md bulk triage).
  audit_digest: { label: "Audit findings to triage", verb: "Triage Inbox", tab: "timeline", glyph: "doc", tone: "review" },
};

export function needsYouMeta(kind: string): NeedsYouMeta {
  return (
    NEEDS_YOU_META[kind] ?? {
      label: humanizeKind(kind),
      verb: "Resolve",
      tab: "timeline",
      glyph: "dot",
      tone: "attention",
    }
  );
}

// Fallback for an unknown kind: "some_new_kind" -> "Some new kind".
export function humanizeKind(kind: string): string {
  const s = kind.replace(/_/g, " ").trim();
  return s.charAt(0).toUpperCase() + s.slice(1);
}

// A coarse priority band from the computed score, for the queue's rank pip.
export function scoreBand(score: number): "critical" | "high" | "normal" {
  if (score >= 80) return "critical";
  if (score >= 55) return "high";
  return "normal";
}
