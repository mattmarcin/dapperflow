import { useEffect, useRef, useState } from "react";
import { useStore } from "../state/store";
import { Session } from "../model";
import { adapterLabel, cautionArgs } from "../lib/agents";
import { HarnessGlyph, harnessLabel } from "../lib/glyphs";
import { isResumable } from "../lib/session-state";
import { deriveSessionTitle, clockTime } from "../lib/format";
import { TerminalSlot, useTerminalPool } from "../state/terminal-pool";

// The cardless session view: where a New Session lands, and where an interrupted
// session is resumed. A live session is a single pooled terminal with the launcher
// identity in the header. An interrupted session (daemon restarted) shows a resume
// banner over its preserved scrollback, and on resume the terminal opens beneath a
// lineage divider so the conversation reads as continuous.
export function SessionView() {
  const store = useStore();
  const sid = store.openSessionId;
  const launch = store.launches.find((l) => l.sessionId === sid);
  const sessionRow = !launch ? store.sessions.find((s) => s.id === sid) : undefined;

  // Sticky routing: once a session opens as interrupted, keep the resume view mounted
  // through a successful resume even though the predecessor row's state then changes
  // (so the lineage divider stays visible). Reset when a different session opens.
  const sticky = useRef<{ sid: string | null; interrupted: boolean }>({ sid: null, interrupted: false });
  if (sticky.current.sid !== (sid ?? null)) {
    sticky.current = { sid: sid ?? null, interrupted: !!sessionRow && isResumable(sessionRow.state) };
  } else if (sessionRow && isResumable(sessionRow.state)) {
    sticky.current.interrupted = true;
  }

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") store.closeSession();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [store]);

  if (sticky.current.interrupted && sessionRow) return <InterruptedSessionView session={sessionRow} />;
  if (launch) return <LiveSessionView />;
  // A live session this app run did not launch: restored from fleet.status after an app
  // restart, or a cardless session opened from Mission Control / the Projects tree. The
  // daemon still owns its PTY, so render its terminal and attach on demand. Without this
  // branch the overlay rendered nothing and clicking such a session did nothing at all -
  // the one-click-to-terminal invariant (product.md) broke for every non-launch session.
  if (sessionRow) return <WireSessionView session={sessionRow} />;

  return null;
}

function LiveSessionView() {
  const store = useStore();
  const pool = useTerminalPool();
  const launch = store.launches.find((l) => l.sessionId === store.openSessionId)!;
  const project = launch.projectId ? store.projects.find((p) => p.id === launch.projectId) : undefined;
  const agent = store.agents.find((a) => a.name === launch.agent || a.id === launch.agent);
  const client = store.client;
  const daemonReady = store.daemon === "connected";
  const danger = agent ? cautionArgs(agent.extra_args) : [];

  const end = () => {
    store.closeLaunch(launch.sessionId);
    pool.evictTerminal(launch.sessionId);
  };

  return (
    <div className="workspace">
      <header className="ws-head session-head">
        <BackButton onClick={store.closeSession} />
        <div className="ws-titleblock">
          <div className="ws-meta">
            <span className="session-launcher">
              <span className="session-launcher-glyph" aria-hidden>
                <HarnessGlyph harness={launch.harness} />
              </span>
              {launch.agent}
            </span>
            <span className="session-family">{adapterLabel(launch.harness)}</span>
            {agent?.caution ? (
              <span className="caution-badge" title={`Weakens safety: ${danger.join(" ")}`}>
                caution
              </span>
            ) : null}
            <span className={`project-chip${project ? "" : " is-none"}`}>
              <span className="project-dot" aria-hidden />
              {project ? project.name : "No project"}
            </span>
          </div>
          <h1 className="ws-title">{launch.title ?? "Live session"}</h1>
          {launch.firstPrompt ? <p className="ws-brief">{launch.firstPrompt}</p> : null}
        </div>
        <div className="session-head-actions">
          <button className="btn-danger btn-sm" onClick={end} title="Kill this session">
            End session
          </button>
        </div>
      </header>

      <div className="ws-content">
        {client && daemonReady ? (
          <div className="term-panel">
            <div className="term-stage">
              <TerminalSlot
                sessionId={launch.sessionId}
                client={client}
                initialInput={launch.firstPrompt ?? undefined}
                onKill={end}
              />
            </div>
          </div>
        ) : (
          <div className="term-panel is-empty">
            <div className="dispatch-inner">
              <h3 className="dispatch-title">The daemon went away</h3>
              <p className="dispatch-sub">
                This session lives in dflowd. Reconnect the daemon to attach its terminal again.
              </p>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}

// A live session the daemon owns that this app run did not launch (post-restart, or
// opened by id from another view). Mirrors LiveSessionView, but its identity comes from
// the persisted fleet row rather than a client-side launch. The pooled TerminalPane
// attaches to the existing PTY on mount (replaying scrollback), so the terminal comes
// back exactly where the user left it.
function WireSessionView({ session }: { session: Session }) {
  const store = useStore();
  const pool = useTerminalPool();
  const agent = store.agents.find((a) => a.name === session.agent || a.id === session.agent_id);
  const agentName = session.agent ?? harnessLabel(session.harness);
  const project = session.project_id
    ? store.projects.find((p) => p.id === session.project_id)
    : session.card_id
      ? store.projects.find((p) => p.id === store.cards.find((c) => c.id === session.card_id)?.project_id)
      : undefined;
  const projectName = project?.name ?? session.project_name ?? null;
  const client = store.client;
  const daemonReady = store.daemon === "connected";
  const danger = agent ? cautionArgs(agent.extra_args) : [];
  const title =
    session.title ?? deriveSessionTitle({ firstPrompt: session.first_prompt, fallback: agentName });

  const end = () => {
    store.client?.kill(session.id).catch(() => undefined);
    pool.evictTerminal(session.id);
    store.closeSession();
    store.refresh().catch(() => undefined);
  };

  return (
    <div className="workspace">
      <header className="ws-head session-head">
        <BackButton onClick={store.closeSession} />
        <div className="ws-titleblock">
          <div className="ws-meta">
            <span className="session-launcher">
              <span className="session-launcher-glyph" aria-hidden>
                <HarnessGlyph harness={session.harness} />
              </span>
              {agentName}
            </span>
            <span className="session-family">{adapterLabel(session.harness)}</span>
            {agent?.caution ? (
              <span className="caution-badge" title={`Weakens safety: ${danger.join(" ")}`}>
                caution
              </span>
            ) : null}
            <span className={`project-chip${projectName ? "" : " is-none"}`}>
              <span className="project-dot" aria-hidden />
              {projectName ?? "No project"}
            </span>
          </div>
          <h1 className="ws-title">{title}</h1>
          {session.first_prompt ? <p className="ws-brief">{session.first_prompt}</p> : null}
        </div>
        <div className="session-head-actions">
          <button className="btn-danger btn-sm" onClick={end} title="Kill this session">
            End session
          </button>
        </div>
      </header>

      <div className="ws-content">
        {client && daemonReady ? (
          <div className="term-panel">
            <div className="term-stage">
              <TerminalSlot sessionId={session.id} client={client} onKill={end} />
            </div>
          </div>
        ) : (
          <div className="term-panel is-empty">
            <div className="dispatch-inner">
              <h3 className="dispatch-title">The daemon went away</h3>
              <p className="dispatch-sub">
                This session lives in dflowd. Reconnect the daemon to attach its terminal again.
              </p>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}

function InterruptedSessionView({ session }: { session: Session }) {
  const store = useStore();
  const card = session.card_id ? store.cards.find((c) => c.id === session.card_id) : undefined;
  // Resolve the project from the session's own cwd->project link first (a cardless
  // session has no card to borrow one from), falling back to the carded project. Without
  // this an interrupted cardless session read "No project" even though the tree grouped
  // it under its real project.
  const project =
    (session.project_id ? store.projects.find((p) => p.id === session.project_id) : undefined) ??
    (card?.project_id ? store.projects.find((p) => p.id === card.project_id) : undefined);
  const projectName = project?.name ?? session.project_name ?? null;
  const agentName = session.agent ?? harnessLabel(session.harness);
  const title =
    session.title ?? deriveSessionTitle({ firstPrompt: session.first_prompt, cardTitle: card?.title, fallback: agentName });

  // resume lifecycle: idle -> resuming -> resumed (new session id) | unsupported
  const [phase, setPhase] = useState<"idle" | "resuming" | "resumed" | "unsupported">("idle");
  const [resumedAt, setResumedAt] = useState<number>();
  const [newSessionId, setNewSessionId] = useState<string>();

  const resume = async () => {
    setPhase("resuming");
    try {
      const res = await store.resumeSession(session.id);
      if (res.ok) {
        setNewSessionId(res.new_session_id);
        setResumedAt(Date.now());
        setPhase("resumed");
      } else if (res.unsupported) {
        setPhase("unsupported");
        store.flash("This harness cannot resume by id. Start a fresh session instead.", { tone: "danger" });
      } else {
        setPhase("idle");
        store.flash(res.error ?? "Resume failed.", { tone: "danger" });
      }
    } catch (e) {
      setPhase("idle");
      store.flash(String(e instanceof Error ? e.message : e), { tone: "danger" });
    }
  };

  const client = store.client;
  const canLiveResume = phase === "resumed" && newSessionId && client && store.daemon === "connected" && !store.fixtureMode;

  return (
    <div className="workspace">
      <header className="ws-head session-head">
        <BackButton onClick={store.closeSession} />
        <div className="ws-titleblock">
          <div className="ws-meta">
            <span className="session-launcher">
              <span className="session-launcher-glyph" aria-hidden>
                <HarnessGlyph harness={session.harness} />
              </span>
              {agentName}
            </span>
            <span className="session-family">{adapterLabel(session.harness)}</span>
            <span className="chip chip-sm chip-interrupted" title="Daemon restarted; resumable with full context">
              Interrupted
            </span>
            <span className={`project-chip${projectName ? "" : " is-none"}`}>
              <span className="project-dot" aria-hidden />
              {projectName ?? "No project"}
            </span>
          </div>
          <h1 className="ws-title">{title}</h1>
        </div>
      </header>

      <div className="ws-content">
        <div className="term-panel resume-panel">
          {phase !== "resumed" ? (
            <div className={`resume-banner${phase === "unsupported" ? " is-unsupported" : ""}`}>
              <span className="resume-banner-glyph" aria-hidden>
                <HarnessGlyph harness={session.harness} />
              </span>
              <div className="resume-banner-text">
                <strong>{phase === "unsupported" ? "Resume unavailable" : "Daemon restarted"}</strong>
                <span>
                  {phase === "unsupported"
                    ? "This harness cannot resume by id yet. The scrollback below is preserved; start a fresh session to continue."
                    : "This session can resume with full context - its transcript and worktree were preserved."}
                </span>
              </div>
              <button
                className="btn-primary btn-sm resume-btn"
                onClick={resume}
                disabled={phase !== "idle"}
                title={phase === "unsupported" ? "The daemon rejected resume for this harness" : "Relaunch with the harness resume flag"}
              >
                {phase === "resuming" ? "Resuming…" : phase === "unsupported" ? "Unavailable" : "Resume session"}
              </button>
            </div>
          ) : null}

          <div className={`term-stage resume-stage${canLiveResume ? " has-live" : ""}`}>
            <div className="scrollback-preview" aria-label="Preserved scrollback">
              {(session.scrollback_preview ?? ["(scrollback preserved on disk)"]).map((line, i) => (
                <div key={i} className="scrollback-line">
                  {line || " "}
                </div>
              ))}

              {phase === "resumed" ? (
                <>
                  <div className="lineage-divider" role="separator">
                    <span className="lineage-divider-label">
                      session resumed · full context restored{resumedAt ? ` · ${clockTime(resumedAt)}` : ""}
                    </span>
                  </div>
                  {canLiveResume ? null : (
                    <div className="scrollback-line is-resumed">
                      {`> ${agentName} is live again in the same worktree. Continue where you left off.`}
                    </div>
                  )}
                </>
              ) : null}
            </div>

            {canLiveResume ? (
              <div className="resume-live">
                <TerminalSlot sessionId={newSessionId!} client={client!} />
              </div>
            ) : null}
          </div>
        </div>
      </div>
    </div>
  );
}

function BackButton({ onClick }: { onClick: () => void }) {
  return (
    <button className="ws-back" onClick={onClick} aria-label="Back">
      <svg width="16" height="16" viewBox="0 0 16 16" fill="none" aria-hidden>
        <path d="M9.5 3.5L5 8l4.5 4.5" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" />
      </svg>
      <span>Back</span>
    </button>
  );
}
