import { Session } from "../model";
import { StateChip } from "./StateChip";
import { HarnessGlyph, harnessLabel } from "../lib/glyphs";
import { elapsed } from "../lib/format";
import { useNow } from "../lib/use-now";

interface Props {
  session: Session;
  onOpenTerminal: () => void;
}

/**
 * The session strip - the board's signature instrument. A live card wears one: the
 * launcher identity (adapter glyph + launcher name), a color-coded state chip with its
 * glyph (the strip's only color), the current recipe stage, elapsed-in-state ticking
 * live, and the agent's last status note. The whole strip is one click to the terminal.
 */
export function SessionStrip({ session, onOpenTerminal }: Props) {
  const now = useNow();
  const agentName = session.agent ?? harnessLabel(session.harness);

  return (
    <button
      type="button"
      className="strip"
      onClick={(e) => {
        e.stopPropagation();
        onOpenTerminal();
      }}
      title={`Open ${agentName} terminal`}
    >
      <span className="strip-top">
        <span className="strip-harness" aria-hidden>
          <HarnessGlyph harness={session.harness} />
        </span>
        <span className="strip-agent">{agentName}</span>
        <StateChip state={session.state} variant="tab" />
        <span className="strip-meter">
          {session.stage ? <span className="strip-stage">{session.stage}</span> : null}
          <span className="strip-elapsed">{elapsed(session.state_since, now)}</span>
        </span>
      </span>
      {session.status_note ? <span className="strip-note">{session.status_note}</span> : null}
    </button>
  );
}
