import { useState } from "react";
import { Harness, HARNESSES } from "../../model";
import { HarnessGlyph, harnessLabel } from "../../lib/glyphs";

interface Props {
  title: string;
  subtitle: string;
  cta: string;
  disabled?: boolean;
  busy?: boolean;
  onGo: (harness: Harness) => void;
}

// The invitation shown on a card's Terminal tab when no live session exists yet:
// pick a harness and go. One choice, one button - dispatch is a decision, not a form.
export function DispatchAffordance({ title, subtitle, cta, disabled, busy, onGo }: Props) {
  const [harness, setHarness] = useState<Harness>("claude");
  return (
    <div className="dispatch">
      <div className="dispatch-inner">
        <h3 className="dispatch-title">{title}</h3>
        <p className="dispatch-sub">{subtitle}</p>
        <div className="harness-picker" role="radiogroup" aria-label="Harness">
          {HARNESSES.map((h) => (
            <button
              key={h}
              type="button"
              role="radio"
              aria-checked={harness === h}
              className={`harness-option${harness === h ? " is-active" : ""}`}
              onClick={() => setHarness(h)}
              disabled={disabled}
            >
              <span className="harness-glyph" aria-hidden>
                <HarnessGlyph harness={h} />
              </span>
              <span className="harness-name">{harnessLabel(h)}</span>
            </button>
          ))}
        </div>
        <button className="btn-primary dispatch-go" onClick={() => onGo(harness)} disabled={disabled || busy}>
          {busy ? "Starting…" : cta}
        </button>
      </div>
    </div>
  );
}
