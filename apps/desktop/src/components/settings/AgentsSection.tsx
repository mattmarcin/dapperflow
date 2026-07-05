import { useState } from "react";
import { useStore } from "../../state/store";
import { Agent } from "../../model";
import { adapterLabel, cautionArgs } from "../../lib/agents";
import { HarnessGlyph } from "../../lib/glyphs";
import { AgentForm } from "./AgentForm";
import { ConfirmDialog } from "../ConfirmDialog";

// Settings > Agents: the launcher rack. Lists configured launchers (detected on
// PATH or custom), runs on-demand detection, and adds/edits/removes custom ones.
// Each row is a channel strip: adapter glyph, name, command, version, source, an
// enabled switch, and a real caution warning when default args weaken safety.
export function AgentsSection() {
  const store = useStore();
  const { agents } = store;
  const [mode, setMode] = useState<{ kind: "add" } | { kind: "edit"; agent: Agent } | null>(null);
  const [detecting, setDetecting] = useState(false);
  const [removing, setRemoving] = useState<Agent | null>(null);

  const detect = async () => {
    if (detecting) return;
    setDetecting(true);
    try {
      const res = await store.detectAgents();
      const created = res.found.filter((f) => f.created).length;
      const found = res.found.length;
      const names = res.found.map((f) => f.name).join(", ") || "none";
      store.flash(
        found === 0
          ? "No known agent CLIs found on PATH."
          : `Found ${found} on PATH (${created} new, ${found - created} already configured): ${names}.`,
      );
    } catch (e) {
      store.flash(messageOf(e), { tone: "danger" });
    } finally {
      setDetecting(false);
    }
  };

  const toggle = async (agent: Agent) => {
    const res = await store.updateAgent({ id: agent.id, enabled: !agent.enabled });
    if (!res.ok) store.flash(res.error ?? "Could not update the launcher.", { tone: "danger" });
  };

  const confirmRemove = async () => {
    const agent = removing;
    if (!agent) return;
    setRemoving(null);
    const res = await store.removeAgent(agent.id);
    if (res.ok) {
      store.flash(`Removed ${res.removed ?? agent.name}.`);
    } else if (res.inUse) {
      // The daemon refuses while a live session references the launcher; the honest
      // move is to disable it (data-model.md / Honesty note; api.rs agents_remove).
      store.flash(res.error ?? `${agent.name} is in use.`, {
        tone: "danger",
        action: {
          label: "Disable instead",
          run: () => {
            store
              .updateAgent({ id: agent.id, enabled: false })
              .then((r) =>
                r.ok
                  ? store.flash(`Disabled ${agent.name}.`)
                  : store.flash(r.error ?? "Could not disable.", { tone: "danger" }),
              );
          },
        },
      });
    } else {
      store.flash(res.error ?? "Could not remove the launcher.", { tone: "danger" });
    }
  };

  return (
    <div className="agents-section">
      <div className="agents-bar">
        <div className="agents-bar-text">
          <h2 className="agents-title">Agents</h2>
          <p className="agents-sub">
            Launchers pair an adapter's behavior with your own command, arguments, and
            environment. Every picker offers these, not hardcoded harness names.
          </p>
        </div>
        <div className="agents-bar-actions">
          <button className="btn-ghost btn-sm" onClick={detect} disabled={detecting}>
            {detecting ? <Spinner /> : <IconRadar />}
            {detecting ? "Detecting…" : "Detect installed agents"}
          </button>
          <button
            className="btn-primary btn-sm"
            onClick={() => setMode({ kind: "add" })}
            disabled={mode?.kind === "add"}
          >
            <IconPlus />
            Add launcher
          </button>
        </div>
      </div>

      {mode?.kind === "add" ? (
        <AgentForm
          editing={null}
          onSubmit={(input) => ("add" in input ? store.addAgent(input.add) : store.updateAgent(input.update))}
          onDone={() => {
            setMode(null);
            store.flash("Launcher added.");
          }}
          onCancel={() => setMode(null)}
        />
      ) : null}

      {agents.length === 0 && mode?.kind !== "add" ? (
        <div className="agents-empty">
          <p>
            No launchers yet. Run <strong>Detect installed agents</strong> to scan PATH for
            Claude, Codex, OpenCode, Cursor, and Pi, or add a custom one.
          </p>
        </div>
      ) : null}

      <div className="agent-rack">
        {agents.map((agent) =>
          mode?.kind === "edit" && mode.agent.id === agent.id ? (
            <AgentForm
              key={agent.id}
              editing={agent}
              onSubmit={(input) =>
                "add" in input ? store.addAgent(input.add) : store.updateAgent(input.update)
              }
              onDone={() => {
                setMode(null);
                store.flash(`Saved ${agent.name}.`);
              }}
              onCancel={() => setMode(null)}
            />
          ) : (
            <LauncherRow
              key={agent.id}
              agent={agent}
              onToggle={() => toggle(agent)}
              onEdit={() => setMode({ kind: "edit", agent })}
              onRemove={() => setRemoving(agent)}
            />
          ),
        )}
      </div>

      {removing ? (
        <ConfirmDialog
          title={`Remove ${removing.name}?`}
          body="This launcher is removed from every picker. Sessions that already ran keep their adapter family. You can add it again later."
          confirmLabel="Remove launcher"
          tone="danger"
          onCancel={() => setRemoving(null)}
          onConfirm={confirmRemove}
        />
      ) : null}
    </div>
  );
}

function LauncherRow({
  agent,
  onToggle,
  onEdit,
  onRemove,
}: {
  agent: Agent;
  onToggle: () => void;
  onEdit: () => void;
  onRemove: () => void;
}) {
  const danger = cautionArgs(agent.extra_args);
  const envKeys = Object.keys(agent.extra_env);

  return (
    <div className={`launcher${agent.enabled ? "" : " is-disabled"}${agent.caution ? " has-caution" : ""}`}>
      <div className="launcher-glyph" aria-hidden>
        <HarnessGlyph harness={agent.adapter} />
      </div>

      <div className="launcher-main">
        <div className="launcher-line">
          <span className="launcher-name">{agent.name}</span>
          <span className="launcher-family">{adapterLabel(agent.adapter)}</span>
          <span className={`launcher-source src-${agent.source}`}>{agent.source}</span>
          {agent.caution ? <span className="caution-badge" title={`Weakens safety: ${danger.join(" ")}`}>caution</span> : null}
        </div>
        <div className="launcher-line launcher-line-2">
          <code className="launcher-cmd" title={agent.command}>
            {agent.command}
          </code>
          {agent.detected_version ? (
            <span className="launcher-version">{agent.detected_version}</span>
          ) : null}
        </div>
        {agent.extra_args.length > 0 || envKeys.length > 0 ? (
          <div className="launcher-chips">
            {agent.extra_args.map((a, i) => (
              <code key={`a${i}`} className={`chip-arg${danger.includes(a) ? " is-danger" : ""}`}>
                {a}
              </code>
            ))}
            {envKeys.map((k) => (
              <code key={`e${k}`} className="chip-env">
                {k}
              </code>
            ))}
          </div>
        ) : null}
        {agent.caution ? (
          <div className="caution-strip" role="alert">
            <IconWarn />
            <span>
              Runs with reduced safety prompts:{" "}
              {danger.map((d) => (
                <code key={d}>{d}</code>
              ))}
              .
            </span>
          </div>
        ) : null}
      </div>

      <div className="launcher-actions">
        <button
          role="switch"
          aria-checked={agent.enabled}
          className={`switch${agent.enabled ? " is-on" : ""}`}
          onClick={onToggle}
          title={agent.enabled ? "Enabled - offered in pickers" : "Disabled - hidden from pickers"}
        >
          <span className="switch-knob" aria-hidden />
        </button>
        <button className="icon-btn" onClick={onEdit} aria-label={`Edit ${agent.name}`} title="Edit">
          <IconEdit />
        </button>
        <button className="icon-btn is-danger" onClick={onRemove} aria-label={`Remove ${agent.name}`} title="Remove">
          <IconTrash />
        </button>
      </div>
    </div>
  );
}

function messageOf(e: unknown): string {
  if (e instanceof Error) return e.message;
  if (e && typeof e === "object" && "message" in e) return String((e as { message: unknown }).message);
  return String(e);
}

function IconPlus() {
  return (
    <svg width="12" height="12" viewBox="0 0 13 13" aria-hidden>
      <path d="M6.5 1.5v10M1.5 6.5h10" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" />
    </svg>
  );
}
function IconRadar() {
  return (
    <svg width="13" height="13" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.4" aria-hidden>
      <circle cx="8" cy="8" r="6" />
      <circle cx="8" cy="8" r="2.4" />
      <path d="M8 8 L12.5 4" strokeLinecap="round" />
    </svg>
  );
}
function IconEdit() {
  return (
    <svg width="14" height="14" viewBox="0 0 16 16" fill="none" aria-hidden>
      <path
        d="M10.5 2.8l2.7 2.7M3 11.4l7.5-7.5 2.6 2.6L5.6 14H3v-2.6z"
        stroke="currentColor"
        strokeWidth="1.3"
        strokeLinejoin="round"
      />
    </svg>
  );
}
function IconTrash() {
  return (
    <svg width="14" height="14" viewBox="0 0 16 16" fill="none" aria-hidden>
      <path
        d="M3 4.5h10M6.2 4.5V3.2c0-.4.3-.7.7-.7h2.2c.4 0 .7.3.7.7v1.3M4.4 4.5l.5 8.1c0 .5.4.9.9.9h4.4c.5 0 .9-.4.9-.9l.5-8.1"
        stroke="currentColor"
        strokeWidth="1.3"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}
function IconWarn() {
  return (
    <svg width="14" height="14" viewBox="0 0 16 16" fill="none" aria-hidden>
      <path d="M8 2.4l6 10.4H2L8 2.4z" stroke="currentColor" strokeWidth="1.3" strokeLinejoin="round" />
      <path d="M8 6.6v3M8 11.2v.01" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" />
    </svg>
  );
}
function Spinner() {
  return (
    <svg className="spin" width="13" height="13" viewBox="0 0 16 16" fill="none" aria-hidden>
      <circle cx="8" cy="8" r="6" stroke="currentColor" strokeWidth="2" strokeOpacity="0.25" />
      <path d="M8 2a6 6 0 016 6" stroke="currentColor" strokeWidth="2" strokeLinecap="round" />
    </svg>
  );
}
