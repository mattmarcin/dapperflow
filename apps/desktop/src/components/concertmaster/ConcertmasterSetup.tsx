import { useEffect, useMemo, useRef, useState } from "react";
import { useStore } from "../../state/store";
import { ProjectPicker } from "../ProjectPicker";
import { LauncherPicker } from "../LauncherPicker";
import { InstallHint, McpDetect, mcpDetect, mcpInstallHint } from "../../lib/mcp-mount";
import { adapterLabel } from "../../lib/agents";

// The first-open setup flow (deliverable 2): pick a launcher, optionally focus a
// project, confirm the dflow-mcp mount, then summon. Mounting is strongly guided but
// never a hard gate - if it is missing the panel says exactly what to run and lets the
// user proceed (deliverable 5), because the harness itself will show the tools missing.
export function ConcertmasterSetup() {
  const store = useStore();
  const enabled = useMemo(() => store.agents.filter((a) => a.enabled), [store.agents]);

  const [agentId, setAgentId] = useState<string | null>(enabled[0]?.id ?? null);
  const [focusId, setFocusId] = useState<string | null>(store.filterProjectId ?? null);
  const [addingProject, setAddingProject] = useState(false);
  const [summoning, setSummoning] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const agent = enabled.find((a) => a.id === agentId);
  const harness = agent?.adapter ?? "";
  const focusProject = focusId ? store.projects.find((p) => p.id === focusId) : undefined;
  // cwd is where dflow-mcp mounts and where a project-scoped .mcp.json is read/written.
  const cwd = focusProject?.path ?? store.projects[0]?.path ?? null;
  const daemonReady = store.daemon === "connected";

  const canSummon = !!agentId && daemonReady && !summoning;

  const summon = async () => {
    if (!agentId || !daemonReady || summoning) return;
    setSummoning(true);
    setError(null);
    try {
      await store.startConcertmaster({
        agent: agentId,
        cwd,
        scopeProjectId: focusId,
        mounted: mount.detect?.mounted ?? null,
      });
      // startConcertmaster opens the panel session view.
    } catch (e) {
      setError(messageOf(e));
      setSummoning(false);
    }
  };

  const mount = useMountState(harness, cwd);

  return (
    <div className="cm-setup">
      <div className="cm-setup-intro">
        <p className="cm-setup-lede">
          A chat surface backed by a real harness session with your whole fleet mounted as
          tools. It can shape cards, dispatch work, and answer "what's going on across my
          projects" from live data.
        </p>
      </div>

      {!daemonReady ? (
        <div className="cm-setup-offline" role="status">
          The Concertmaster is a live session. Start <code>dflowd</code> to summon one - or
          preview it with a{" "}
          <button className="cm-inline-link" onClick={store.startConcertmasterDemo}>
            demo transcript
          </button>
          .
        </div>
      ) : null}

      <section className="cm-setup-step">
        <StepHead n={1} title="Launcher" hint="The harness the Concertmaster speaks through." />
        <LauncherPicker value={agentId} onChange={setAgentId} onManage={openAgents(store)} />
      </section>

      <section className="cm-setup-step">
        <StepHead
          n={2}
          title="Focus"
          hint="Optional. Scope the Concertmaster to one project from the start."
        />
        <ProjectPicker
          value={focusId}
          onChange={setFocusId}
          allowNone
          adding={addingProject}
          onAddingChange={setAddingProject}
        />
      </section>

      <section className="cm-setup-step">
        <StepHead n={3} title="Mount dflow-mcp" hint="Give the harness your fleet as MCP tools." />
        <MountSection harness={harness} harnessLabel={agent ? adapterLabel(harness) : ""} mount={mount} cwd={cwd} />
      </section>

      {error ? <p className="agent-form-error">{error}</p> : null}

      <div className="cm-setup-actions">
        <button className="btn-primary" onClick={summon} disabled={!canSummon}>
          {summoning ? "Summoning…" : "Summon the Concertmaster"}
        </button>
        <button className="btn-ghost btn-sm" onClick={store.startConcertmasterDemo} title="Preview with a fixture transcript">
          Demo
        </button>
      </div>
    </div>
  );
}

// --- Mount detection + install hint ----------------------------------------

interface MountState {
  detect: McpDetect | null;
  hint: InstallHint | null;
  checking: boolean;
  mounting: boolean;
  recheck: () => void;
  mountWrite: () => void;
}

function useMountState(harness: string, cwd: string | null): MountState {
  const [detect, setDetect] = useState<McpDetect | null>(null);
  const [hint, setHint] = useState<InstallHint | null>(null);
  const [checking, setChecking] = useState(false);
  const [mounting, setMounting] = useState(false);
  const reqRef = useRef(0);

  const run = useMemo(
    () => async () => {
      if (!harness) {
        setDetect(null);
        setHint(null);
        return;
      }
      const req = ++reqRef.current;
      setChecking(true);
      const [d, h] = await Promise.all([mcpDetect(harness, { cwd }), mcpInstallHint(harness, { cwd })]);
      if (reqRef.current !== req) return; // a newer request superseded this one
      setDetect(d);
      setHint(h);
      setChecking(false);
    },
    [harness, cwd],
  );

  useEffect(() => {
    run();
  }, [run]);

  const mountWrite = useMemo(
    () => async () => {
      if (!harness) return;
      setMounting(true);
      await mcpInstallHint(harness, { cwd, write: true });
      setMounting(false);
      run();
    },
    [harness, cwd, run],
  );

  return { detect, hint, checking, mounting, recheck: run, mountWrite };
}

function MountSection({
  harness,
  harnessLabel,
  mount,
  cwd,
}: {
  harness: string;
  harnessLabel: string;
  mount: MountState;
  cwd: string | null;
}) {
  const store = useStore();
  if (!harness) {
    return <p className="cm-mount-none">Pick a launcher first.</p>;
  }

  const { detect, hint, checking, mounting } = mount;
  const canWrite = !!cwd && !!hint?.exePath; // --write needs a real cwd and a located binary

  return (
    <div className="cm-mount">
      <div className="cm-mount-status">
        {checking ? (
          <span className="cm-mount-chip is-checking">
            <span className="dot" aria-hidden /> checking {harnessLabel}…
          </span>
        ) : detect?.mounted ? (
          <span className="cm-mount-chip is-mounted" title={detect.location ?? undefined}>
            <CheckMark /> mounted
          </span>
        ) : detect && !detect.detectable ? (
          <span className="cm-mount-chip is-unknown">mount manually - can't auto-detect for {harnessLabel}</span>
        ) : (
          <span className="cm-mount-chip is-missing">not detected</span>
        )}
        <button className="cm-inline-link" onClick={mount.recheck} disabled={checking}>
          re-check
        </button>
      </div>

      {!detect?.mounted ? (
        <>
          <pre className="cm-mount-cmd">
            <code>{hint?.text?.trim() || "…"}</code>
          </pre>
          <div className="cm-mount-actions">
            <button
              className="btn-ghost btn-sm"
              onClick={() => {
                navigator.clipboard?.writeText(hint?.text ?? "").catch(() => undefined);
                store.flash("Mount command copied.");
              }}
              disabled={!hint?.text}
            >
              Copy
            </button>
            {canWrite ? (
              <button className="btn-ghost btn-sm" onClick={mount.mountWrite} disabled={mounting}>
                {mounting ? "Mounting…" : "Mount into this project"}
              </button>
            ) : null}
          </div>
          {hint?.error ? <p className="cm-mount-note">{hint.error}</p> : null}
        </>
      ) : null}
    </div>
  );
}

// --- bits ------------------------------------------------------------------

function StepHead({ n, title, hint }: { n: number; title: string; hint: string }) {
  return (
    <div className="cm-step-head">
      <span className="cm-step-n" aria-hidden>
        {n}
      </span>
      <span className="cm-step-title">{title}</span>
      <span className="cm-step-hint">{hint}</span>
    </div>
  );
}

function CheckMark() {
  return (
    <svg width="11" height="11" viewBox="0 0 16 16" fill="none" aria-hidden>
      <path d="M3 8.5l3.2 3.2L13 4.5" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" />
    </svg>
  );
}

function openAgents(store: ReturnType<typeof useStore>) {
  return () => store.setView("settings");
}

function messageOf(e: unknown): string {
  if (e instanceof Error) return e.message;
  if (e && typeof e === "object" && "message" in e) return String((e as { message: unknown }).message);
  return String(e);
}
