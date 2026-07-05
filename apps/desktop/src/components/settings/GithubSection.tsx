// Settings > GitHub: the read-only issue-import surface (product.md / Card sources:
// GitHub issue import; roadmap.md M5.1-2). Three parts:
//   1. gh auth status (github.auth.status) - gh presence + login, or a setup pointer.
//   2. Per-project import config (assignee/label/milestone filters) - never a firehose.
//   3. Import issues: preview (github.issues.preview) -> multi-select -> import
//      (github.issues.import), deduped on origin_ref.
// Fixture-tolerant: when gh is unauthenticated the preview is empty and PR mode degrades.

import { useCallback, useEffect, useMemo, useState } from "react";
import { useStore } from "../../state/store";
import { GithubAuthStatus, GithubImportConfig, GithubIssue, Project } from "../../model";
import { CARD_TYPE_META } from "../../lib/glyphs";

export function GithubSection() {
  const store = useStore();
  const { projects } = store;
  const [auth, setAuth] = useState<GithubAuthStatus | "loading">("loading");
  const [projectId, setProjectId] = useState<string | null>(projects[0]?.id ?? null);

  useEffect(() => {
    let cancelled = false;
    store
      .githubAuthStatus()
      .then((a) => !cancelled && setAuth(a))
      .catch(() => !cancelled && setAuth({ gh_present: false, authenticated: false }));
    return () => {
      cancelled = true;
    };
  }, [store]);

  // Keep a valid selected project as the registry loads.
  useEffect(() => {
    if (!projectId && projects.length > 0) setProjectId(projects[0].id);
  }, [projects, projectId]);

  const project = projects.find((p) => p.id === projectId) ?? null;
  const canImport = auth !== "loading" && auth.authenticated;

  return (
    <div className="agents-section">
      <div className="agents-bar">
        <div className="agents-bar-text">
          <h2 className="agents-title">GitHub</h2>
          <p className="agents-sub">
            Import GitHub issues as cards through your local <code>gh</code> CLI. Read-only: DapperFlow never
            writes to GitHub except the pull request the gate opens when a card ships.
          </p>
        </div>
      </div>

      <AuthCard auth={auth} />

      {projects.length === 0 ? (
        <div className="settings-card gh-empty">
          <p>Register a project first. Issue import is configured per project.</p>
        </div>
      ) : (
        <>
          <div className="gh-project-row" role="tablist" aria-label="Project to import for">
            {projects.map((p) => (
              <button
                key={p.id}
                role="tab"
                aria-selected={p.id === projectId}
                className={`gh-project-chip${p.id === projectId ? " is-active" : ""}`}
                onClick={() => setProjectId(p.id)}
              >
                <span className="project-dot" aria-hidden />
                {p.name}
              </button>
            ))}
          </div>

          {project ? <ImportPanel key={project.id} project={project} canImport={canImport} /> : null}
        </>
      )}
    </div>
  );
}

function AuthCard({ auth }: { auth: GithubAuthStatus | "loading" }) {
  if (auth === "loading") {
    return (
      <div className="settings-card gh-auth">
        <div className="gh-auth-row">
          <GithubMark />
          <span className="gh-auth-status is-loading">Checking gh auth status…</span>
        </div>
      </div>
    );
  }

  const ok = auth.authenticated;
  const present = auth.gh_present;
  return (
    <div className={`settings-card gh-auth${ok ? " is-ok" : " is-off"}`}>
      <div className="gh-auth-row">
        <GithubMark />
        <div className="gh-auth-main">
          <div className="gh-auth-line">
            <span className={`gh-auth-status${ok ? " is-ok" : " is-off"}`}>
              <span className="gh-auth-dot" aria-hidden />
              {ok ? "Authenticated" : present ? "Not logged in" : "gh CLI not found"}
            </span>
            {ok ? (
              <span className="gh-auth-user">
                {auth.user}
                {auth.host && auth.host !== "github.com" ? ` · ${auth.host}` : ""}
              </span>
            ) : null}
          </div>
          {ok && auth.scopes && auth.scopes.length > 0 ? (
            <div className="gh-auth-scopes">
              {auth.scopes.map((s) => (
                <code key={s} className="chip-env">
                  {s}
                </code>
              ))}
            </div>
          ) : null}
          {!ok ? (
            <p className="gh-auth-hint">
              {auth.setup_hint ??
                "Install the GitHub CLI and run gh auth login. Without it, PR mode degrades cleanly to local-only."}
            </p>
          ) : null}
        </div>
      </div>
    </div>
  );
}

function ImportPanel({ project, canImport }: { project: Project; canImport: boolean }) {
  const store = useStore();
  const [config, setConfig] = useState<GithubImportConfig | null>(null);
  const [dirty, setDirty] = useState(false);
  const [saving, setSaving] = useState(false);

  const [issues, setIssues] = useState<GithubIssue[] | null>(null);
  const [previewing, setPreviewing] = useState(false);
  const [selected, setSelected] = useState<Set<number>>(new Set());
  const [importing, setImporting] = useState(false);

  useEffect(() => {
    let cancelled = false;
    setConfig(null);
    setIssues(null);
    setSelected(new Set());
    store.getGithubImportConfig(project.id).then((c) => !cancelled && setConfig(c));
    return () => {
      cancelled = true;
    };
  }, [store, project.id]);

  const patch = useCallback((p: Partial<GithubImportConfig>) => {
    setConfig((c) => (c ? { ...c, ...p } : c));
    setDirty(true);
  }, []);

  const save = useCallback(async () => {
    if (!config) return;
    setSaving(true);
    try {
      await store.setGithubImportConfig(project.id, config);
      setDirty(false);
      store.flash("Import filters saved.");
    } catch (e) {
      store.flash(String(e), { tone: "danger" });
    } finally {
      setSaving(false);
    }
  }, [config, store, project.id]);

  const preview = useCallback(async () => {
    setPreviewing(true);
    try {
      const list = await store.previewGithubIssues(project.id);
      setIssues(list);
      // Preselect everything not already imported (the common "import all new" intent).
      setSelected(new Set(list.filter((i) => !i.imported_card_id).map((i) => i.number)));
    } catch (e) {
      store.flash(String(e), { tone: "danger" });
    } finally {
      setPreviewing(false);
    }
  }, [store, project.id]);

  const toggle = useCallback((n: number) => {
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(n)) next.delete(n);
      else next.add(n);
      return next;
    });
  }, []);

  const runImport = useCallback(async () => {
    if (selected.size === 0) return;
    setImporting(true);
    try {
      const res = await store.importGithubIssues(project.id, [...selected]);
      if (res.ok) {
        const parts = [
          res.imported ? `${res.imported} imported` : "",
          res.refreshed ? `${res.refreshed} refreshed` : "",
        ].filter(Boolean);
        store.flash(`${parts.join(", ") || "Nothing to import"} into ${project.name}'s Inbox.`);
        // Refresh the preview so the just-imported issues flip to "imported".
        await preview();
      } else {
        store.flash(res.error ?? "Import failed.", { tone: "danger" });
      }
    } catch (e) {
      store.flash(String(e), { tone: "danger" });
    } finally {
      setImporting(false);
    }
  }, [selected, store, project.id, project.name, preview]);

  const selectableCount = useMemo(
    () => (issues ?? []).filter((i) => !i.imported_card_id).length,
    [issues],
  );

  if (!config) return <div className="settings-card gh-loading">Loading import config…</div>;

  return (
    <div className="settings-card gh-import">
      <div className="gh-import-head">
        <h3 className="gh-import-title">Import config</h3>
        <span className="gh-import-sub">Filters for {project.name}. Empty filters mean the curated picker: every open issue.</span>
      </div>

      <div className="gh-filters">
        <FilterField
          label="Assignee"
          hint="Comma-separated logins, or @me"
          value={config.assignees.join(", ")}
          placeholder="@me, priya-ops"
          onChange={(v) => patch({ assignees: splitList(v) })}
        />
        <FilterField
          label="Labels"
          hint="Match any of these labels"
          value={config.labels.join(", ")}
          placeholder="bug, priority:high"
          onChange={(v) => patch({ labels: splitList(v) })}
        />
        <FilterField
          label="Milestone"
          hint="Exact milestone title"
          value={config.milestone ?? ""}
          placeholder="v1.4 hardening"
          onChange={(v) => patch({ milestone: v.trim() || null })}
        />
        <div className="gh-field">
          <label className="gh-field-label">State</label>
          <select
            className="gh-select"
            value={config.state}
            onChange={(e) => patch({ state: e.target.value === "all" ? "all" : "open" })}
          >
            <option value="open">Open only</option>
            <option value="all">Open + closed</option>
          </select>
          <span className="gh-field-hint">Never an unfiltered firehose</span>
        </div>
      </div>

      <div className="gh-import-actions">
        <button className="btn-ghost btn-sm" onClick={save} disabled={!dirty || saving}>
          {saving ? "Saving…" : dirty ? "Save filters" : "Filters saved"}
        </button>
        <button className="btn-primary btn-sm" onClick={preview} disabled={!canImport || previewing}>
          {previewing ? <Spinner /> : <IconDownload />}
          {previewing ? "Fetching…" : "Preview issues"}
        </button>
      </div>

      {!canImport ? (
        <p className="gh-import-note">
          Log in with gh to preview and import issues. The filters above are saved either way.
        </p>
      ) : null}

      {issues !== null ? (
        <div className="gh-preview">
          <div className="gh-preview-head">
            <span className="gh-preview-count">
              {issues.length} issue{issues.length === 1 ? "" : "s"} match · {selected.size} selected
            </span>
            <div className="gh-preview-tools">
              <button
                className="gh-linkbtn"
                onClick={() => setSelected(new Set(issues.filter((i) => !i.imported_card_id).map((i) => i.number)))}
                disabled={selectableCount === 0}
              >
                Select new
              </button>
              <button className="gh-linkbtn" onClick={() => setSelected(new Set())} disabled={selected.size === 0}>
                Clear
              </button>
            </div>
          </div>

          {issues.length === 0 ? (
            <p className="gh-preview-empty">No open issues match these filters.</p>
          ) : (
            <ul className="gh-issue-list">
              {issues.map((iss) => (
                <IssueRow
                  key={iss.number}
                  issue={iss}
                  selected={selected.has(iss.number)}
                  onToggle={() => toggle(iss.number)}
                />
              ))}
            </ul>
          )}

          <div className="gh-preview-foot">
            <button className="btn-primary btn-sm" onClick={runImport} disabled={selected.size === 0 || importing}>
              {importing ? "Importing…" : `Import ${selected.size} issue${selected.size === 1 ? "" : "s"}`}
            </button>
            <span className="gh-preview-foot-hint">
              Imported issues land in {project.name}'s Inbox, deduped so re-import refreshes instead of duplicating.
            </span>
          </div>
        </div>
      ) : null}
    </div>
  );
}

function IssueRow({
  issue,
  selected,
  onToggle,
}: {
  issue: GithubIssue;
  selected: boolean;
  onToggle: () => void;
}) {
  const imported = !!issue.imported_card_id;
  const type = CARD_TYPE_META[issue.suggested_type];
  return (
    <li className={`gh-issue${selected ? " is-selected" : ""}${imported ? " is-imported" : ""}`}>
      <button
        className="gh-issue-btn"
        role="checkbox"
        aria-checked={selected}
        disabled={imported}
        onClick={onToggle}
      >
        <span className={`card-check gh-issue-check${selected ? " is-on" : ""}`} aria-hidden>
          <svg width="10" height="10" viewBox="0 0 12 12" fill="none">
            <path d="M2 6.5l2.8 2.8L10 3.5" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" />
          </svg>
        </span>
        <div className="gh-issue-main">
          <div className="gh-issue-line">
            <span className="gh-issue-num">#{issue.number}</span>
            <span className="gh-issue-title">{issue.title}</span>
            <span className={`type-badge type-${type.tone} gh-issue-type`}>{type.label}</span>
            {imported ? <span className="gh-issue-imported">imported</span> : null}
          </div>
          <div className="gh-issue-meta">
            {issue.labels.map((l) => (
              <LabelChip key={l.name} name={l.name} color={l.color} />
            ))}
            {issue.assignees.length > 0 ? (
              <span className="gh-issue-assignee">@{issue.assignees[0]}</span>
            ) : null}
            {issue.milestone ? <span className="gh-issue-milestone">◈ {issue.milestone}</span> : null}
          </div>
        </div>
      </button>
    </li>
  );
}

export function LabelChip({ name, color }: { name: string; color?: string | null }) {
  // Tint from the real GitHub label color, kept subtle over the dark chrome.
  const hex = color && /^[0-9a-f]{6}$/i.test(color) ? color : "6c9ce6";
  const style = {
    color: `#${hex}`,
    borderColor: `#${hex}59`,
    background: `#${hex}1f`,
  } as React.CSSProperties;
  return (
    <span className="gh-label" style={style}>
      {name}
    </span>
  );
}

function FilterField({
  label,
  hint,
  value,
  placeholder,
  onChange,
}: {
  label: string;
  hint: string;
  value: string;
  placeholder: string;
  onChange: (v: string) => void;
}) {
  return (
    <div className="gh-field">
      <label className="gh-field-label">{label}</label>
      <input
        className="gh-input"
        type="text"
        value={value}
        placeholder={placeholder}
        onChange={(e) => onChange(e.target.value)}
      />
      <span className="gh-field-hint">{hint}</span>
    </div>
  );
}

function splitList(v: string): string[] {
  return v
    .split(",")
    .map((s) => s.trim())
    .filter(Boolean);
}

function GithubMark() {
  return (
    <svg className="gh-mark" width="22" height="22" viewBox="0 0 16 16" fill="currentColor" aria-hidden>
      <path d="M8 0C3.58 0 0 3.58 0 8c0 3.54 2.29 6.53 5.47 7.59.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49-2.01.37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13-.28-.15-.68-.52-.01-.53.63-.01 1.08.58 1.23.82.72 1.21 1.87.87 2.33.66.07-.52.28-.87.51-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82.64-.18 1.32-.27 2-.27.68 0 1.36.09 2 .27 1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01 1.93-.01 2.2 0 .21.15.46.55.38A8.01 8.01 0 0016 8c0-4.42-3.58-8-8-8z" />
    </svg>
  );
}

function IconDownload() {
  return (
    <svg width="13" height="13" viewBox="0 0 16 16" fill="none" aria-hidden>
      <path d="M8 2v8m0 0L5 7m3 3l3-3M3 13h10" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" strokeLinejoin="round" />
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
