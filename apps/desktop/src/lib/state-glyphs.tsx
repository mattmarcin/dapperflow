// Per-state lifecycle glyphs. Each one is a distinct, semantic shape (not a shared
// dot) so a session's state is legible even before the color registers: a waveform
// for working, a question mark for needs_input, a barrier for blocked, a play-arrow
// for the resumable interrupted state. Glyphs inherit currentColor from the chip's
// tone; size scales with the surface (mini in tree rows, full on strips).

import { SessionState } from "../model";

export function StateGlyph({ state, size = 12 }: { state: SessionState; size?: number }) {
  const common = {
    width: size,
    height: size,
    viewBox: "0 0 16 16",
    fill: "none" as const,
    "aria-hidden": true,
  };
  const stroke = {
    stroke: "currentColor",
    strokeWidth: 1.5,
    strokeLinecap: "round" as const,
    strokeLinejoin: "round" as const,
  };
  switch (state) {
    case "starting":
      // A spinner arc - booting. Spins via .state-glyph.is-spin.
      return (
        <svg {...common} className="state-glyph is-spin">
          <path d="M8 2.2a5.8 5.8 0 015.8 5.8" {...stroke} />
          <circle cx="8" cy="8" r="1.5" fill="currentColor" />
        </svg>
      );
    case "working":
      // An activity waveform - progress in motion.
      return (
        <svg {...common} className="state-glyph">
          <path d="M1.6 8h2.2l1.4-4 2.6 8 1.7-5 1.1 3h2.2" {...stroke} />
        </svg>
      );
    case "idle":
      // Pause bars - waiting, nothing in flight.
      return (
        <svg {...common} className="state-glyph">
          <path d="M6 4.5v7M10 4.5v7" {...stroke} />
        </svg>
      );
    case "needs_input":
      // A question mark - it needs YOU to answer.
      return (
        <svg {...common} className="state-glyph">
          <path d="M5.6 5.9a2.4 2.4 0 114 1.9c-.7.5-1.2 1-1.2 1.9" {...stroke} />
          <circle cx="8" cy="12.4" r="0.95" fill="currentColor" />
        </svg>
      );
    case "awaiting_feedback":
      // An eye - a plan under review.
      return (
        <svg {...common} className="state-glyph">
          <path d="M1.6 8s2.6-3.8 6.4-3.8S14.4 8 14.4 8s-2.6 3.8-6.4 3.8S1.6 8 1.6 8z" {...stroke} />
          <circle cx="8" cy="8" r="1.7" {...stroke} />
        </svg>
      );
    case "blocked":
      // A no-entry barrier - stuck, cannot proceed.
      return (
        <svg {...common} className="state-glyph">
          <circle cx="8" cy="8" r="5.6" {...stroke} />
          <path d="M4.2 4.2l7.6 7.6" {...stroke} />
        </svg>
      );
    case "done":
      // A check - finished.
      return (
        <svg {...common} className="state-glyph">
          <path d="M3.2 8.4l3 3L12.8 5" {...stroke} />
        </svg>
      );
    case "error":
      // An alert bang - stopped on an error.
      return (
        <svg {...common} className="state-glyph">
          <path d="M8 2.4l6 10.6H2z" {...stroke} />
          <path d="M8 6.6v3" {...stroke} />
          <circle cx="8" cy="11.4" r="0.85" fill="currentColor" />
        </svg>
      );
    case "interrupted":
      // A play triangle in a ring - paused, press to resume (not dead).
      return (
        <svg {...common} className="state-glyph">
          <circle cx="8" cy="8" r="5.8" {...stroke} strokeDasharray="2.2 2.2" />
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
