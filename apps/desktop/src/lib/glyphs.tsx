// Harness glyphs and timeline icons. Harness glyphs are intentionally NEUTRAL
// (they inherit currentColor): the shape carries identity, the state chip carries
// the only color in the strip. This keeps the mixing-desk channel disciplined.

import { CardType } from "../model";

export function HarnessGlyph({ harness }: { harness: string }) {
  switch (harness) {
    case "claude":
      // A radiant spark - the conductor's downbeat.
      return (
        <svg width="14" height="14" viewBox="0 0 16 16" fill="none" aria-hidden>
          <path
            d="M8 1.5v13M1.5 8h13M3.4 3.4l9.2 9.2M12.6 3.4l-9.2 9.2"
            stroke="currentColor"
            strokeWidth="1.3"
            strokeLinecap="round"
          />
        </svg>
      );
    case "codex":
      // A prism/hexagon.
      return (
        <svg width="14" height="14" viewBox="0 0 16 16" fill="none" aria-hidden>
          <path
            d="M8 1.6l5.5 3.2v6.4L8 14.4 2.5 11.2V4.8L8 1.6z"
            stroke="currentColor"
            strokeWidth="1.3"
            strokeLinejoin="round"
          />
        </svg>
      );
    case "opencode":
      // Angle brackets - open source.
      return (
        <svg width="14" height="14" viewBox="0 0 16 16" fill="none" aria-hidden>
          <path
            d="M5.6 4L2 8l3.6 4M10.4 4L14 8l-3.6 4M9.2 3l-2.4 10"
            stroke="currentColor"
            strokeWidth="1.3"
            strokeLinecap="round"
            strokeLinejoin="round"
          />
        </svg>
      );
    case "pi":
      // The pi letter.
      return (
        <svg width="14" height="14" viewBox="0 0 16 16" fill="none" aria-hidden>
          <path
            d="M2.6 4.6h10.8M5.2 4.6v7.2M10.4 4.6v6.2c0 .9.5 1.2 1.4 1"
            stroke="currentColor"
            strokeWidth="1.3"
            strokeLinecap="round"
          />
        </svg>
      );
    case "cursor":
      // A text-insertion I-beam - the editor's cursor.
      return (
        <svg width="14" height="14" viewBox="0 0 16 16" fill="none" aria-hidden>
          <path
            d="M8 3v10M5.4 3h5.2M5.4 13h5.2"
            stroke="currentColor"
            strokeWidth="1.3"
            strokeLinecap="round"
          />
        </svg>
      );
    case "custom":
      // A patched-in knob on a line - a user-configured channel.
      return (
        <svg width="14" height="14" viewBox="0 0 16 16" fill="none" aria-hidden>
          <path d="M2.5 8h11" stroke="currentColor" strokeWidth="1.3" strokeLinecap="round" />
          <circle cx="10" cy="8" r="2.3" stroke="currentColor" strokeWidth="1.3" />
        </svg>
      );
    default:
      // Shell prompt (the Phase 0 powershell default and any unknown harness).
      return (
        <svg width="14" height="14" viewBox="0 0 16 16" fill="none" aria-hidden>
          <path
            d="M3 4l4 4-4 4M8.5 12h4.5"
            stroke="currentColor"
            strokeWidth="1.3"
            strokeLinecap="round"
            strokeLinejoin="round"
          />
        </svg>
      );
  }
}

// The Concertmaster's own glyph: a conductor's baton on the downbeat. Distinct from
// every harness glyph so the podium session reads as itself in the tree and header
// (product.md: the Concertmaster session is visible in the tree with a distinct glyph).
// Neutral stroke like the harness glyphs; color comes from context.
export function ConcertmasterGlyph({ size = 14 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 16 16" fill="none" aria-hidden>
      {/* the baton, lower-left handle to upper-right tip */}
      <path d="M3.4 12.6l8.1-8.1" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
      {/* the grip */}
      <circle cx="3.1" cy="12.9" r="1.5" fill="currentColor" />
      {/* the downbeat arc it traces */}
      <path
        d="M9.3 3.1a4.4 4.4 0 013.6 3.6"
        stroke="currentColor"
        strokeWidth="1.2"
        strokeLinecap="round"
        opacity="0.55"
      />
    </svg>
  );
}

export function harnessLabel(harness: string): string {
  switch (harness) {
    case "claude":
      return "Claude Code";
    case "codex":
      return "Codex";
    case "opencode":
      return "OpenCode";
    case "pi":
      return "Pi";
    case "cursor":
      return "Cursor";
    case "custom":
      return "Custom";
    case "powershell":
      return "PowerShell";
    default:
      return harness;
  }
}

// Card type badge metadata. Types are informational, so tones are muted and drawn
// from the palette's cool/functional set - never brass (reserved for attention).
export const CARD_TYPE_META: Record<CardType, { label: string; tone: string }> = {
  feature: { label: "feature", tone: "feature" },
  bug: { label: "bug", tone: "bug" },
  chore: { label: "chore", tone: "chore" },
  test: { label: "test", tone: "test" },
  investigation: { label: "investigation", tone: "investigation" },
};

// Timeline event glyphs, grouped by lifecycle family. Unknown kinds get a neutral
// dot (protocol.md: unknown event kinds are preserved and rendered generically).
export function eventGlyph(kind: string): string {
  if (kind.startsWith("gate") || kind.startsWith("finding")) return "check";
  if (kind.startsWith("needs_you")) return "bell";
  if (kind.startsWith("plan") || kind.startsWith("artifact") || kind === "feedback_sent") return "doc";
  if (kind.startsWith("session") || kind === "state_changed" || kind === "turn_ended") return "terminal";
  if (kind === "dispatched" || kind === "worktree_leased" || kind === "env_materialized") return "bolt";
  if (kind === "pushed" || kind === "pr_opened" || kind === "ci_status" || kind === "merged") return "branch";
  if (kind === "blocked" || kind === "needs_input") return "hand";
  if (kind === "created" || kind === "shaped" || kind === "moved" || kind === "dial_changed") return "flag";
  return "dot";
}

// Needs You item icons (lib/needs-you.ts glyph families). Same neutral stroke style
// as the timeline; color comes from the item's tone at the call site.
export function NeedsYouIcon({ glyph }: { glyph: string }) {
  const p = (d: string) => (
    <svg width="15" height="15" viewBox="0 0 16 16" fill="none" aria-hidden>
      <path d={d} stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" strokeLinejoin="round" />
    </svg>
  );
  switch (glyph) {
    case "hand":
      return p("M5 8V4.6a1 1 0 012 0V8m0-.4V3.9a1 1 0 012 0V8m0-.2V5.1a1 1 0 012 0v5c0 2-1.5 3.2-3.5 3.2S5.5 12 5 10.6L4 8.7a1 1 0 011.6-1.1L6.4 8.6");
    case "barrier":
      return p("M2.5 4.5h11v3.5h-11zM4 8v4M12 8v4M3 12h10M3.2 4.7l2 3.3M6.2 4.7l2 3.3M9.2 4.7l2 3.3");
    case "clock":
      return p("M8 2.5a5.5 5.5 0 100 11 5.5 5.5 0 000-11M8 5v3.2l2 1.3");
    case "shield":
      return p("M8 2.2l4.6 1.6v3.4c0 3-2 5-4.6 6-2.6-1-4.6-3-4.6-6V3.8zM6 8l1.5 1.5L10.5 6.5");
    case "branch":
      return p("M5 3v10M5 3a1.6 1.6 0 100 3.2M5 6.2v0M11 5a1.6 1.6 0 100 3.2M11 8.2c0 2-2 2.5-4 2.8M5 13a1.6 1.6 0 100-3.2");
    case "plug":
      return p("M6 2v3M10 2v3M4.5 5h7v2.5A3.5 3.5 0 018 11a3.5 3.5 0 01-3.5-3.5zM8 11v3");
    case "doc":
      return p("M4.5 2.5h5l2.5 2.5v8.5h-7.5zM9 2.5V5h2.5M6 8h4M6 10.5h4");
    default:
      return p("M8 4.5v4.5M8 11.5h.01");
  }
}

export function EventIcon({ glyph }: { glyph: string }) {
  const p = (d: string) => (
    <svg width="13" height="13" viewBox="0 0 16 16" fill="none" aria-hidden>
      <path d={d} stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" strokeLinejoin="round" />
    </svg>
  );
  switch (glyph) {
    case "check":
      return p("M3 8.5l3.2 3.2L13 4.5");
    case "bell":
      return p("M8 2v1M5 6.5a3 3 0 016 0c0 3 1 4 1 4H4s1-1 1-4zM6.5 13a1.5 1.5 0 003 0");
    case "doc":
      return p("M4.5 2.5h5l2.5 2.5v8.5h-7.5zM9 2.5V5h2.5M6 8h4M6 10.5h4");
    case "terminal":
      return p("M3.5 4l3 3-3 3M8 10.5h4.5");
    case "bolt":
      return p("M8.5 2L4 9h3.2l-.7 5L12 7H8.8z");
    case "branch":
      return p("M5 3v10M5 3a1.6 1.6 0 100 3.2M5 6.2v0M11 5a1.6 1.6 0 100 3.2M11 8.2c0 2-2 2.5-4 2.8M5 13a1.6 1.6 0 100-3.2");
    case "hand":
      return p("M5 8V4.5a1 1 0 012 0V8m0-.5V3.8a1 1 0 012 0V8m0-.3V5a1 1 0 012 0v5c0 2-1.5 3.2-3.5 3.2S5.5 12 5 10.5L4 8.6a1 1 0 011.6-1.1L6.4 8.5");
    case "flag":
      return p("M4 2.5v11M4 3.2h7l-1.5 2.4L11 8H4");
    default:
      return p("M8 8h.01");
  }
}
