import { useStore } from "../state/store";
import { adapterLabel, cautionArgs } from "../lib/agents";
import { HarnessGlyph } from "../lib/glyphs";

interface Props {
  /** Selected launcher id, or null. */
  value: string | null;
  onChange: (agentId: string) => void;
  /** Open Settings > Agents (shown in the empty state). */
  onManage?: () => void;
}

// Pick a configured launcher. Offers only enabled launchers (product.md: pickers
// list configured launchers, not hardcoded harness names), each with its adapter
// glyph, family, detected version, and a caution badge when its args weaken safety.
// Reused by the New Session front door and future dispatch surfaces.
export function LauncherPicker({ value, onChange, onManage }: Props) {
  const { agents } = useStore();
  const enabled = agents.filter((a) => a.enabled);

  if (enabled.length === 0) {
    return (
      <div className="launcher-picker">
        <p className="launcher-picker-empty">
          No enabled launchers. Detect installed agents or add one in Settings, then start a
          session.
        </p>
        {onManage ? (
          <button type="button" className="project-add" onClick={onManage}>
            Open Settings › Agents
          </button>
        ) : null}
      </div>
    );
  }

  return (
    <div className="launcher-picker" role="radiogroup" aria-label="Launcher">
      {enabled.map((agent) => {
        const danger = cautionArgs(agent.extra_args);
        const active = value === agent.id;
        return (
          <button
            key={agent.id}
            type="button"
            role="radio"
            aria-checked={active}
            className={`launcher-option${active ? " is-active" : ""}`}
            onClick={() => onChange(agent.id)}
          >
            <span className="launcher-option-glyph" aria-hidden>
              <HarnessGlyph harness={agent.adapter} />
            </span>
            <span className="launcher-option-text">
              <span className="launcher-option-name">
                {agent.name}
                {agent.caution ? (
                  <span className="caution-badge" title={`Weakens safety: ${danger.join(" ")}`}>
                    caution
                  </span>
                ) : null}
              </span>
              <span className="launcher-option-meta">
                {adapterLabel(agent.adapter)}
                {agent.detected_version ? ` · ${agent.detected_version}` : ""}
              </span>
            </span>
          </button>
        );
      })}
    </div>
  );
}
