import { SessionState } from "../model";
import { STATE_META } from "../lib/session-state";
import { StateGlyph } from "../lib/state-glyphs";

type Variant = "full" | "tab" | "mini";

interface Props {
  state: SessionState;
  // full: glyph + label (board strips, Mission Control). tab: compact glyph + label
  // (session tabs). mini: glyph only in a colored disc (dense tree rows), label in the
  // tooltip. Default "full".
  variant?: Variant;
}

// The single lifecycle chip used across every surface, so a state reads identically on
// a board card, a Projects-tree row, a session tab, and Mission Control. Color (tone),
// shape (glyph), and word (label) are the three signals; demand states carry an extra
// glow ring, live states pulse.
export function StateChip({ state, variant = "full" }: Props) {
  const meta = STATE_META[state];
  const mods = `${meta.pulse ? " is-pulse" : ""}${meta.demand ? " is-demand" : ""}`;

  if (variant === "mini") {
    return (
      <span className={`mini-chip chip-${meta.tone}${mods}`} title={`${meta.label} - ${meta.description}`}>
        <StateGlyph state={state} size={9} />
      </span>
    );
  }

  const sizeClass = variant === "tab" ? " chip-sm" : "";
  const glyphSize = variant === "tab" ? 10 : 12;
  return (
    <span className={`chip${sizeClass} chip-${meta.tone}${mods}`} title={meta.description}>
      <span className="chip-glyph" aria-hidden>
        <StateGlyph state={state} size={glyphSize} />
      </span>
      {meta.label}
    </span>
  );
}
