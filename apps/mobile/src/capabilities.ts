// The phone capability profile, made explicit so the UI can decline to render an
// affordance the token cannot exercise (security.md 5.2: "the app UI merely declines
// to render affordances the token cannot exercise"). Enforcement is daemon-side scope
// checking; this table is the client mirror that keeps the UI honest.
//
// Default phone profile (security.md, Remote access trust model):
//   allowed  - Needs You, approvals, steering, read-only terminals
//   denied   - vault access, recipe install, full interactive terminals,
//              secret-bearing streams (env endpoints, raw scrollback)
//
// Two M6-specific gates on top of the profile:
//   - `merge` is a real capability but its resolving action (PR merge) depends on
//     the M5 delivery pipeline, which does not exist yet: it renders as a disabled
//     preview with an honest "arrives with M5" note.
//   - `steering` is in-profile but M6 surfaces no steering path (the terminal peek is
//     strictly read-only per this milestone's scope); it is listed as allowed-but-
//     not-yet-surfaced so Settings can state it truthfully.

export interface Capability {
  key: string;
  label: string;
  state: "allowed" | "surfaced-later" | "gated-until-m5" | "denied";
  note: string;
}

export const CAPABILITIES: Capability[] = [
  {
    key: "needs_you",
    label: "Needs You queue",
    state: "allowed",
    note: "The ranked, cross-project attention queue. This is the home screen.",
  },
  {
    key: "approve_plan",
    label: "Plan approval",
    state: "allowed",
    note: "Approve a plan round with one overall feedback note (v1).",
  },
  {
    key: "terminal_peek",
    label: "Read-only terminal peek",
    state: "allowed",
    note: "A scrubbed styled screen snapshot, poll-refreshed. Look, do not type.",
  },
  {
    key: "steering",
    label: "Steering",
    state: "surfaced-later",
    note: "In the phone profile, but M6 surfaces no input path. Arrives with the attention-surface slice.",
  },
  {
    key: "merge",
    label: "Merge a green PR",
    state: "gated-until-m5",
    note: "The merge decision is a phone capability, but the delivery pipeline arrives with M5. Shown as a disabled preview.",
  },
  {
    key: "vault",
    label: "Env vault",
    state: "denied",
    note: "No vault access of any kind from the phone. Secrets never leave the desktop.",
  },
  {
    key: "recipe_install",
    label: "Recipe install / edit",
    state: "denied",
    note: "Recipes are installed and edited on the desktop, the trusted device.",
  },
  {
    key: "full_terminal",
    label: "Full interactive terminal",
    state: "denied",
    note: "No keyboard path into a PTY. The phone is an attention surface, not a second cockpit.",
  },
];

export function capability(key: string): Capability | undefined {
  return CAPABILITIES.find((c) => c.key === key);
}

export function isAllowed(key: string): boolean {
  return capability(key)?.state === "allowed";
}
