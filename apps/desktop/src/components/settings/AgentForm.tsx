import { useMemo, useState } from "react";
import { Agent, ADAPTER_FAMILIES, AgentAddInput, AgentUpdateInput } from "../../model";
import { adapterLabel, ADAPTER_HINT, cautionArgs } from "../../lib/agents";
import { HarnessGlyph } from "../../lib/glyphs";

interface Props {
  // Null = add a new custom launcher; an Agent = edit it.
  editing: Agent | null;
  onSubmit: (
    input: { add: AgentAddInput } | { update: AgentUpdateInput },
  ) => Promise<{ ok: boolean; error?: string }>;
  onDone: () => void;
  onCancel: () => void;
}

interface EnvRow {
  key: string;
  value: string;
}

// The add/edit launcher form: name, adapter family, command, an extra-args list
// editor, and an extra-env key/value editor. Daemon validation surfaces inline. The
// cc-alt story (a second Claude subscription = the claude family + a config-dir env)
// is achievable in well under a minute: name, keep claude, command `claude`, one env.
export function AgentForm({ editing, onSubmit, onDone, onCancel }: Props) {
  const [name, setName] = useState(editing?.name ?? "");
  const [adapter, setAdapter] = useState(editing?.adapter ?? "claude");
  const [command, setCommand] = useState(editing?.command ?? "");
  const [args, setArgs] = useState<string[]>(editing ? [...editing.extra_args] : []);
  const [env, setEnv] = useState<EnvRow[]>(
    editing ? Object.entries(editing.extra_env).map(([key, value]) => ({ key, value })) : [],
  );
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  // A detected launcher keeps its detected identity; editing one only changes its
  // args/env/command/enabled semantics, so name + adapter stay read-only there.
  const detected = editing?.source === "detected";

  const cleanArgs = useMemo(() => args.map((a) => a.trim()).filter(Boolean), [args]);
  const danger = useMemo(() => cautionArgs(cleanArgs), [cleanArgs]);

  const submit = async () => {
    if (busy) return;
    setError(null);
    if (!name.trim()) return setError("Enter a name for the launcher.");
    if (!command.trim()) return setError("Enter the command to launch.");
    const extra_env: Record<string, string> = {};
    for (const row of env) {
      const key = row.key.trim();
      if (!key) continue;
      extra_env[key] = row.value;
    }
    setBusy(true);
    const res = editing
      ? await onSubmit({
          update: {
            id: editing.id,
            name: name.trim(),
            adapter,
            command: command.trim(),
            extra_args: cleanArgs,
            extra_env,
          },
        })
      : await onSubmit({
          add: { name: name.trim(), adapter, command: command.trim(), extra_args: cleanArgs, extra_env },
        });
    setBusy(false);
    if (res.ok) onDone();
    else setError(res.error ?? "Could not save the launcher.");
  };

  const onFormKey = (e: React.KeyboardEvent) => {
    if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
      e.preventDefault();
      submit();
    }
  };

  return (
    <div className="agent-form" onKeyDown={onFormKey}>
      <div className="agent-form-head">
        <span className="agent-form-glyph" aria-hidden>
          <HarnessGlyph harness={adapter} />
        </span>
        <h3 className="agent-form-title">{editing ? `Edit ${editing.name}` : "Add a launcher"}</h3>
      </div>

      <div className="agent-form-grid">
        <label className="field">
          <span className="field-label">Name</span>
          <input
            className="field-input"
            value={name}
            autoFocus={!editing}
            readOnly={detected}
            placeholder="cc-alt"
            onChange={(e) => {
              setName(e.target.value);
              setError(null);
            }}
          />
          {detected ? <span className="field-note">Detected launchers keep their name.</span> : null}
        </label>

        <label className="field">
          <span className="field-label">Adapter family</span>
          <select
            className="field-input"
            value={adapter}
            disabled={detected}
            onChange={(e) => setAdapter(e.target.value)}
          >
            {ADAPTER_FAMILIES.map((f) => (
              <option key={f} value={f}>
                {adapterLabel(f)}
              </option>
            ))}
          </select>
          <span className="field-note">{ADAPTER_HINT[adapter] ?? "Behavior profile this launcher uses."}</span>
        </label>
      </div>

      <label className="field">
        <span className="field-label">Command</span>
        <input
          className="field-input mono"
          value={command}
          placeholder="claude"
          onChange={(e) => {
            setCommand(e.target.value);
            setError(null);
          }}
        />
        <span className="field-note">The base executable to run. A full path or a name on PATH.</span>
      </label>

      <div className="field">
        <span className="field-label">
          Extra args <span className="field-optional">appended at every launch</span>
        </span>
        <div className="kv-editor">
          {args.length === 0 ? <p className="kv-empty">No extra arguments.</p> : null}
          {args.map((arg, i) => (
            <div className="kv-row" key={i}>
              <input
                className="field-input mono"
                value={arg}
                placeholder="--flag"
                onChange={(e) => setArgs((prev) => prev.map((a, j) => (j === i ? e.target.value : a)))}
              />
              <button
                type="button"
                className="kv-remove"
                aria-label="Remove argument"
                onClick={() => setArgs((prev) => prev.filter((_, j) => j !== i))}
              >
                <IconMinus />
              </button>
            </div>
          ))}
          <button type="button" className="kv-add" onClick={() => setArgs((prev) => [...prev, ""])}>
            <IconPlus /> Add argument
          </button>
        </div>
      </div>

      <div className="field">
        <span className="field-label">
          Extra env <span className="field-optional">merged into the launch, launcher wins</span>
        </span>
        <div className="kv-editor">
          {env.length === 0 ? <p className="kv-empty">No extra environment variables.</p> : null}
          {env.map((row, i) => (
            <div className="kv-row kv-row-pair" key={i}>
              <input
                className="field-input mono kv-key"
                value={row.key}
                placeholder="CLAUDE_CONFIG_DIR"
                onChange={(e) =>
                  setEnv((prev) => prev.map((r, j) => (j === i ? { ...r, key: e.target.value } : r)))
                }
              />
              <span className="kv-eq" aria-hidden>
                =
              </span>
              <input
                className="field-input mono kv-val"
                value={row.value}
                placeholder="C:\\Users\\you\\.claude-alt"
                onChange={(e) =>
                  setEnv((prev) => prev.map((r, j) => (j === i ? { ...r, value: e.target.value } : r)))
                }
              />
              <button
                type="button"
                className="kv-remove"
                aria-label="Remove variable"
                onClick={() => setEnv((prev) => prev.filter((_, j) => j !== i))}
              >
                <IconMinus />
              </button>
            </div>
          ))}
          <button
            type="button"
            className="kv-add"
            onClick={() => setEnv((prev) => [...prev, { key: "", value: "" }])}
          >
            <IconPlus /> Add variable
          </button>
        </div>
      </div>

      {danger.length > 0 ? (
        <div className="caution-strip" role="alert">
          <IconWarn />
          <span>
            These args weaken safety:{" "}
            {danger.map((d) => (
              <code key={d}>{d}</code>
            ))}
            . The launcher will carry a caution badge.
          </span>
        </div>
      ) : null}

      {error ? <p className="agent-form-error">{error}</p> : null}

      <div className="agent-form-foot">
        <span className="agent-form-hint">Ctrl+Enter to save</span>
        <div className="agent-form-actions">
          <button type="button" className="btn-ghost btn-sm" onClick={onCancel}>
            Cancel
          </button>
          <button type="button" className="btn-primary btn-sm" onClick={submit} disabled={busy}>
            {busy ? "Saving…" : editing ? "Save changes" : "Add launcher"}
          </button>
        </div>
      </div>
    </div>
  );
}

function IconPlus() {
  return (
    <svg width="12" height="12" viewBox="0 0 13 13" aria-hidden>
      <path d="M6.5 1.5v10M1.5 6.5h10" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
    </svg>
  );
}
function IconMinus() {
  return (
    <svg width="12" height="12" viewBox="0 0 13 13" aria-hidden>
      <path d="M1.5 6.5h10" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
    </svg>
  );
}
function IconWarn() {
  return (
    <svg width="14" height="14" viewBox="0 0 16 16" fill="none" aria-hidden>
      <path
        d="M8 2.4l6 10.4H2L8 2.4z"
        stroke="currentColor"
        strokeWidth="1.3"
        strokeLinejoin="round"
      />
      <path d="M8 6.6v3M8 11.2v.01" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" />
    </svg>
  );
}
