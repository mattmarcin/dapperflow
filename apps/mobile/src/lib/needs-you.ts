// The Attention Router's vocabulary for the phone: how each Needs You kind reads to a
// human, and which resolving SURFACE one tap opens (mobile.md 1: every item deep-links
// to its resolving surface). The phone has four resolving surfaces:
//   peek     - the read-only terminal snapshot (a blocked/needs-input/stuck agent);
//   plan     - the read-only-plus-approve plan review;
//   approval - the approval sheet (a green PR merge decision - gated until M5);
//   detail   - a generic detail sheet (gate findings and unknown kinds).
// Unknown kinds fall back to a generic label and the detail surface, never dropped.

export type ResolveSurface = "peek" | "plan" | "approval" | "detail";

export interface NeedsYouMeta {
  label: string; // what the item is, in the user's terms
  verb: string; // the primary action button label
  surface: ResolveSurface;
  glyph: NeedsYouGlyph;
  tone: "attention" | "trouble" | "review" | "delivery";
  /** True when the phone can act on this item directly (approve/peek), vs. detail-only. */
  actionable: boolean;
}

export type NeedsYouGlyph = "hand" | "barrier" | "clock" | "shield" | "branch" | "plug" | "doc" | "dot";

const META: Record<string, NeedsYouMeta> = {
  plan_round: { label: "Plan awaiting review", verb: "Review plan", surface: "plan", glyph: "doc", tone: "review", actionable: true },
  gate_finding: { label: "Gate finding needs judgment", verb: "View finding", surface: "detail", glyph: "shield", tone: "review", actionable: false },
  agent_blocked: { label: "Agent blocked", verb: "Peek", surface: "peek", glyph: "barrier", tone: "trouble", actionable: true },
  agent_stuck: { label: "Agent stuck", verb: "Peek", surface: "peek", glyph: "clock", tone: "attention", actionable: true },
  agent_needs_input: { label: "Agent needs input", verb: "Peek", surface: "peek", glyph: "hand", tone: "attention", actionable: true },
  trust_dialog: { label: "Trust prompt hit", verb: "Peek", surface: "peek", glyph: "shield", tone: "attention", actionable: true },
  pr_ready: { label: "PR ready to merge", verb: "Merge", surface: "approval", glyph: "branch", tone: "delivery", actionable: true },
  env_drift: { label: "Environment drift", verb: "View", surface: "detail", glyph: "plug", tone: "trouble", actionable: false },
  service_failed: { label: "Service failed to start", verb: "View", surface: "detail", glyph: "plug", tone: "trouble", actionable: false },
};

export function needsYouMeta(kind: string): NeedsYouMeta {
  return (
    META[kind] ?? {
      label: humanizeKind(kind),
      verb: "View",
      surface: "detail",
      glyph: "dot",
      tone: "attention",
      actionable: false,
    }
  );
}

export function humanizeKind(kind: string): string {
  const s = kind.replace(/_/g, " ").trim();
  return s.charAt(0).toUpperCase() + s.slice(1);
}

/** A coarse priority band from the computed score, for the queue's rank pip. */
export function scoreBand(score: number): "critical" | "high" | "normal" {
  if (score >= 80) return "critical";
  if (score >= 55) return "high";
  return "normal";
}
