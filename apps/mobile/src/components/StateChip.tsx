import { SessionState } from "../client/model";
import { stateMeta } from "../lib/session-state";
import { StateGlyph } from "../lib/glyphs";

// The single lifecycle chip, phone twin of the desktop's StateChip: color (tone),
// shape (glyph), word (label). Demand states (needs_input, blocked) carry a glow ring;
// live states pulse. `mini` is glyph-only for dense rows.
export function StateChip({ state, mini = false }: { state: SessionState; mini?: boolean }) {
  const meta = stateMeta(state);
  const mods = `${meta.pulse ? " is-pulse" : ""}${meta.demand ? " is-demand" : ""}`;
  if (mini) {
    return (
      <span className={`mini-chip chip-${meta.tone}${mods}`} title={`${meta.label} - ${meta.description}`}>
        <StateGlyph state={state} size={10} />
      </span>
    );
  }
  return (
    <span className={`chip chip-${meta.tone}${mods}`} title={meta.description}>
      <span className="chip-glyph" aria-hidden>
        <StateGlyph state={state} size={12} />
      </span>
      {meta.label}
    </span>
  );
}
