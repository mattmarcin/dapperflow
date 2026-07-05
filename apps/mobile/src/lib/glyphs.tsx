// Harness, lifecycle-state, and Needs You glyphs - the phone-scoped copy of the
// desktop's icon set (M6 debt). Harness and state glyphs are NEUTRAL (currentColor);
// the state chip carries the only color, keeping the strip disciplined.

import { SessionState } from "../client/model";

export function HarnessGlyph({ harness, size = 15 }: { harness: string; size?: number }) {
  const common = { width: size, height: size, viewBox: "0 0 16 16", fill: "none" as const, "aria-hidden": true };
  const s = { stroke: "currentColor", strokeWidth: 1.3, strokeLinecap: "round" as const, strokeLinejoin: "round" as const };
  switch (harness) {
    case "claude":
      return (
        <svg {...common}>
          <path d="M8 1.5v13M1.5 8h13M3.4 3.4l9.2 9.2M12.6 3.4l-9.2 9.2" {...s} />
        </svg>
      );
    case "codex":
      return (
        <svg {...common}>
          <path d="M8 1.6l5.5 3.2v6.4L8 14.4 2.5 11.2V4.8L8 1.6z" {...s} />
        </svg>
      );
    case "opencode":
      return (
        <svg {...common}>
          <path d="M5.6 4L2 8l3.6 4M10.4 4L14 8l-3.6 4M9.2 3l-2.4 10" {...s} />
        </svg>
      );
    case "pi":
      return (
        <svg {...common}>
          <path d="M2.6 4.6h10.8M5.2 4.6v7.2M10.4 4.6v6.2c0 .9.5 1.2 1.4 1" {...s} />
        </svg>
      );
    case "cursor":
      return (
        <svg {...common}>
          <path d="M8 3v10M5.4 3h5.2M5.4 13h5.2" {...s} />
        </svg>
      );
    default:
      return (
        <svg {...common}>
          <path d="M3 4l4 4-4 4M8.5 12h4.5" {...s} />
        </svg>
      );
  }
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
    default:
      return harness;
  }
}

export function StateGlyph({ state, size = 12 }: { state: SessionState; size?: number }) {
  const common = { width: size, height: size, viewBox: "0 0 16 16", fill: "none" as const, "aria-hidden": true };
  const s = { stroke: "currentColor", strokeWidth: 1.5, strokeLinecap: "round" as const, strokeLinejoin: "round" as const };
  switch (state) {
    case "starting":
      return (
        <svg {...common} className="state-glyph is-spin">
          <path d="M8 2.2a5.8 5.8 0 015.8 5.8" {...s} />
          <circle cx="8" cy="8" r="1.5" fill="currentColor" />
        </svg>
      );
    case "working":
      return (
        <svg {...common} className="state-glyph">
          <path d="M1.6 8h2.2l1.4-4 2.6 8 1.7-5 1.1 3h2.2" {...s} />
        </svg>
      );
    case "idle":
      return (
        <svg {...common} className="state-glyph">
          <path d="M6 4.5v7M10 4.5v7" {...s} />
        </svg>
      );
    case "needs_input":
      return (
        <svg {...common} className="state-glyph">
          <path d="M5.6 5.9a2.4 2.4 0 114 1.9c-.7.5-1.2 1-1.2 1.9" {...s} />
          <circle cx="8" cy="12.4" r="0.95" fill="currentColor" />
        </svg>
      );
    case "awaiting_feedback":
      return (
        <svg {...common} className="state-glyph">
          <path d="M1.6 8s2.6-3.8 6.4-3.8S14.4 8 14.4 8s-2.6 3.8-6.4 3.8S1.6 8 1.6 8z" {...s} />
          <circle cx="8" cy="8" r="1.7" {...s} />
        </svg>
      );
    case "blocked":
      return (
        <svg {...common} className="state-glyph">
          <circle cx="8" cy="8" r="5.6" {...s} />
          <path d="M4.2 4.2l7.6 7.6" {...s} />
        </svg>
      );
    case "done":
      return (
        <svg {...common} className="state-glyph">
          <path d="M3.2 8.4l3 3L12.8 5" {...s} />
        </svg>
      );
    case "error":
      return (
        <svg {...common} className="state-glyph">
          <path d="M8 2.4l6 10.6H2z" {...s} />
          <path d="M8 6.6v3" {...s} />
          <circle cx="8" cy="11.4" r="0.85" fill="currentColor" />
        </svg>
      );
    case "interrupted":
      return (
        <svg {...common} className="state-glyph">
          <circle cx="8" cy="8" r="5.8" {...s} strokeDasharray="2.2 2.2" />
          <path d="M6.6 5.7l3.4 2.3-3.4 2.3z" fill="currentColor" stroke="none" />
        </svg>
      );
    default:
      return (
        <svg {...common} className="state-glyph">
          <circle cx="8" cy="8" r="1.5" fill="currentColor" />
        </svg>
      );
  }
}

export function NeedsYouIcon({ glyph, size = 16 }: { glyph: string; size?: number }) {
  const p = (d: string) => (
    <svg width={size} height={size} viewBox="0 0 16 16" fill="none" aria-hidden>
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
