import { useCallback, useMemo, useState } from "react";
import { useStore } from "../state/store";
import { Card, LaunchedSession, Project, Session } from "../model";
import { isLive, isResumable, ATTENTION_STATES } from "../lib/session-state";
import { StateChip } from "./StateChip";
import { ConcertmasterGlyph, HarnessGlyph, harnessLabel } from "../lib/glyphs";
import { deriveSessionTitle, elapsed, timeAgo } from "../lib/format";
import { useNow } from "../lib/use-now";
import { useContextMenu, MenuItem } from "./ContextMenu";
import { useTerminalPool } from "../state/terminal-pool";
import { revealInExplorer } from "../lib/tauri";
import { AddProjectForm } from "./AddProjectForm";

const INFLIGHT_LANES = new Set(["ready", "performing", "verifying", "needs_you", "pr"]);

export function ProjectsTree() {
  const store = useStore();
  const { projects, cards, sessions, filterProjectId } = store;
  const [expanded, setExpanded] = useState<Set<string>>(() => new Set(projects.slice(0, 2).map((p) => p.id)));
  const [adding, setAdding] = useState(false);

  // A session's project home: the daemon's cwd->project match for a cardless session,
  // else the carded project. Grouping by this (not by card linkage alone) is what keeps
  // cardless New Sessions visible in the tree after a restart.
  const cardProject = useMemo(() => new Map(cards.map((c) => [c.id, c.project_id])), [cards]);
  const resolveProjectId = useCallback(
    (s: Session): string | null =>
      s.project_id ?? (s.card_id ? cardProject.get(s.card_id) ?? null : null),
    [cardProject],
  );
  // Sessions with no resolvable project (no cwd->project match, no carded project) get a
  // Loose sessions home so nothing running is ever invisible (product.md: the tree shows
  // every session).
  const looseSessions = useMemo(
    () => sessions.filter((s) => resolveProjectId(s) === null),
    [sessions, resolveProjectId],
  );

  const toggle = (id: string) =>
    setExpanded((prev) => {
      const next = new Set(prev);
      next.has(id) ? next.delete(id) : next.add(id);
      return next;
    });

  return (
    <div className="tree">
      <div className="tree-head">
        <span className="tree-title">Projects</span>
        <button className="tree-add" aria-label="Add project" onClick={() => setAdding((v) => !v)}>
          <svg width="13" height="13" viewBox="0 0 13 13" aria-hidden>
            <path d="M6.5 1.5v10M1.5 6.5h10" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
          </svg>
        </button>
      </div>

      {adding ? (
        <AddProjectForm onAdded={() => setAdding(false)} onCancel={() => setAdding(false)} />
      ) : null}

      <ConcertmasterTreeNode />

      <div className="tree-list">
        {projects.map((p) => (
          <ProjectNode
            key={p.id}
            project={p}
            cards={cards.filter((c) => c.project_id === p.id)}
            sessions={sessions.filter((s) => resolveProjectId(s) === p.id)}
            launches={store.launchesForProject(p.id).filter((l) => l.alive)}
            expanded={expanded.has(p.id)}
            selected={filterProjectId === p.id}
            onToggle={() => toggle(p.id)}
            onSelect={() => store.setFilterProject(filterProjectId === p.id ? null : p.id)}
          />
        ))}
        {looseSessions.length > 0 ? <LooseSessionsNode sessions={looseSessions} /> : null}
        {projects.length === 0 && looseSessions.length === 0 && !adding ? (
          <button className="tree-empty" onClick={() => setAdding(true)}>
            No projects yet. Add one to begin.
          </button>
        ) : null}
      </div>
    </div>
  );
}

// The Concertmaster session, shown in the tree with its own baton glyph (product.md:
// it is a session like any other, visible in the tree with a distinct glyph). Clicking
// reveals the panel rather than opening a stage terminal - the panel owns its terminal.
function ConcertmasterTreeNode() {
  const store = useStore();
  const cm = store.concertmaster;
  if (!cm) return null;
  const scope = cm.scopeProjectId ? store.projects.find((p) => p.id === cm.scopeProjectId) : null;
  return (
    <button
      className={`tree-concertmaster${store.panelOpen ? " is-open" : ""}`}
      onClick={() => store.focusConcertmaster()}
      title="Open the Concertmaster panel"
    >
      <span className="tree-concertmaster-glyph" aria-hidden>
        <ConcertmasterGlyph />
      </span>
      <span className="tree-concertmaster-body">
        <span className="tree-concertmaster-label">
          Concertmaster{cm.demo ? " · demo" : ""}
        </span>
        <span className="tree-concertmaster-sub">{scope ? `focused · ${scope.name}` : cm.agentName}</span>
      </span>
      {cm.alive ? <span className="tree-concertmaster-dot" aria-hidden title="Live" /> : null}
    </button>
  );
}

function ProjectNode({
  project,
  cards,
  sessions,
  launches,
  expanded,
  selected,
  onToggle,
  onSelect,
}: {
  project: Project;
  cards: Card[];
  sessions: Session[];
  launches: LaunchedSession[];
  expanded: boolean;
  selected: boolean;
  onToggle: () => void;
  onSelect: () => void;
}) {
  const store = useStore();
  const { openMenu } = useContextMenu();
  const pool = useTerminalPool();
  const now = useNow();
  const [renaming, setRenaming] = useState<string | null>(null);
  const [draft, setDraft] = useState("");

  const cardById = useMemo(() => new Map(cards.map((c) => [c.id, c])), [cards]);
  const liveSessions = useMemo(() => sessions.filter((s) => isLive(s.state)), [sessions]);
  const pastSessions = useMemo(() => sessions.filter((s) => !isLive(s.state)).slice(0, 4), [sessions]);
  const inflightCards = useMemo(
    () => cards.filter((c) => INFLIGHT_LANES.has(c.lane) && !liveSessions.some((s) => s.card_id === c.id)),
    [cards, liveSessions],
  );
  const needsYou = liveSessions.filter((s) => ATTENTION_STATES.includes(s.state)).length;

  const projectMenu = (e: React.MouseEvent) => {
    openMenu(e, [
      { id: "new", label: "New session here", onSelect: () => { store.setFilterProject(project.id); store.openNewSession(); } },
      { id: "board", label: "Open on board", onSelect: () => { store.setView("board"); store.setFilterProject(project.id); } },
      // Re-audit lives here: the same offer as after add-project (product.md).
      { id: "audit", label: "Audit project…", onSelect: () => store.offerAudit(project) },
      {
        id: "reveal",
        label: "Reveal in Explorer",
        separatorBefore: true,
        onSelect: async () => {
          const ok = await revealInExplorer(project.path);
          if (!ok) {
            navigator.clipboard?.writeText(project.path).catch(() => undefined);
            store.flash("Project path copied to clipboard.");
          }
        },
      },
      { id: "copy", label: "Copy path", onSelect: () => { navigator.clipboard?.writeText(project.path).catch(() => undefined); store.flash("Project path copied to clipboard."); } },
    ]);
  };

  const commitRename = (session: Session | LaunchedSession, isLaunch: boolean) => {
    const t = draft.trim();
    const id = isLaunch ? (session as LaunchedSession).sessionId : (session as Session).id;
    if (t) {
      if (isLaunch) store.renameLaunch(id, t);
      else store.renameSession(id, t);
    }
    setRenaming(null);
    setDraft("");
  };

  const killLive = (s: Session) => {
    store.client?.kill(s.id).catch(() => undefined);
    pool.evictTerminal(s.id);
    store.refresh().catch(() => undefined);
  };

  // One-click-to-terminal for a session row: a carded session opens its card workspace
  // terminal tab; a cardless one opens its session view (which attaches the PTY). Using
  // openCard with a null card_id was a no-op - part of the click-does-nothing bug.
  const openSessionRow = (s: Session) =>
    s.card_id ? store.openCard(s.card_id, "terminal") : store.openSession(s.id);

  return (
    <div className={`proj${selected ? " is-selected" : ""}`}>
      <div className="proj-row" onContextMenu={projectMenu}>
        <button className="proj-caret" aria-label={expanded ? "Collapse" : "Expand"} onClick={onToggle}>
          <svg width="10" height="10" viewBox="0 0 10 10" className={expanded ? "is-open" : ""} aria-hidden>
            <path d="M3 2l4 3-4 3" stroke="currentColor" strokeWidth="1.4" fill="none" strokeLinecap="round" strokeLinejoin="round" />
          </svg>
        </button>
        <button className="proj-name-btn" onClick={onSelect} title={project.path}>
          <span className="proj-name">{project.name}</span>
        </button>
        {!expanded ? (
          <span className="proj-badges">
            {liveSessions.length + launches.length > 0 ? (
              <span className="badge badge-live">{liveSessions.length + launches.length}</span>
            ) : null}
            {needsYou > 0 ? <span className="badge badge-attention">{needsYou}</span> : null}
          </span>
        ) : null}
      </div>

      {expanded ? (
        <div className="proj-children">
          {liveSessions.length === 0 &&
          launches.length === 0 &&
          inflightCards.length === 0 &&
          pastSessions.length === 0 ? (
            <div className="proj-quiet">No active work.</div>
          ) : null}

          {launches.map((l) => {
            const title = l.title ?? deriveSessionTitle({ firstPrompt: l.firstPrompt, fallback: l.agent });
            const isRenaming = renaming === l.sessionId;
            return (
              <SessionRow
                key={l.sessionId}
                glyph={l.harness}
                title={title}
                agent={l.agent}
                note={null}
                state="working"
                since={l.createdAt}
                now={now}
                renaming={isRenaming}
                draft={draft}
                onDraft={setDraft}
                onClick={() => store.openSession(l.sessionId)}
                onCommit={() => commitRename(l, true)}
                onCancel={() => { setRenaming(null); setDraft(""); }}
                onMenu={(e) =>
                  openMenu(e, [
                    { id: "open", label: "Open session", onSelect: () => store.openSession(l.sessionId) },
                    { id: "rename", label: "Rename", onSelect: () => { setRenaming(l.sessionId); setDraft(title); } },
                    { id: "kill", label: "Kill session", danger: true, separatorBefore: true, onSelect: () => { store.closeLaunch(l.sessionId); pool.evictTerminal(l.sessionId); } },
                  ])
                }
              />
            );
          })}

          {liveSessions.map((s) => {
            const card = s.card_id ? cardById.get(s.card_id) : undefined;
            const agent = s.agent ?? harnessLabel(s.harness);
            const title = s.title ?? deriveSessionTitle({ firstPrompt: s.first_prompt, cardTitle: card?.title, fallback: agent });
            const isRenaming = renaming === s.id;
            return (
              <SessionRow
                key={s.id}
                glyph={s.harness}
                title={title}
                agent={agent}
                note={s.status_note ?? null}
                state={s.state}
                since={s.state_since}
                now={now}
                renaming={isRenaming}
                draft={draft}
                onDraft={setDraft}
                onClick={() => openSessionRow(s)}
                onCommit={() => commitRename(s, false)}
                onCancel={() => { setRenaming(null); setDraft(""); }}
                onMenu={(e) =>
                  openMenu(e, [
                    { id: "open", label: "Open terminal", onSelect: () => openSessionRow(s) },
                    { id: "rename", label: "Rename", onSelect: () => { setRenaming(s.id); setDraft(title); } },
                    { id: "kill", label: "Kill session", danger: true, separatorBefore: true, onSelect: () => killLive(s) },
                  ])
                }
              />
            );
          })}

          {inflightCards.map((c) => (
            <button key={c.id} className="tree-card" onClick={() => store.openCard(c.id)} onContextMenu={(e) => e.preventDefault()}>
              <span className="tree-card-dot" aria-hidden />
              <span className="tree-card-title">{c.title}</span>
            </button>
          ))}

          {pastSessions.length > 0 ? <div className="tree-subhead">Recent</div> : null}
          {pastSessions.map((s) => {
            const card = s.card_id ? cardById.get(s.card_id) : undefined;
            const agent = s.agent ?? harnessLabel(s.harness);
            const title =
              s.title ?? deriveSessionTitle({ firstPrompt: s.first_prompt, cardTitle: card?.title, fallback: agent });
            const resumable = isResumable(s.state);
            return (
              <PastSessionRow
                key={s.id}
                glyph={s.harness}
                title={title}
                resumable={resumable}
                age={timeAgo(s.ended_at ?? s.state_since, now)}
                onClick={() => (resumable ? store.openSession(s.id) : openSessionRow(s))}
                onResume={resumable ? () => store.openSession(s.id) : undefined}
                onMenu={(e: React.MouseEvent) => {
                  const items: MenuItem[] = resumable
                    ? [
                        { id: "resume", label: "Resume session", onSelect: () => store.openSession(s.id) },
                        { id: "open", label: "Open", onSelect: () => store.openSession(s.id) },
                      ]
                    : [{ id: "open", label: "Open terminal", onSelect: () => openSessionRow(s) }];
                  openMenu(e, items);
                }}
              />
            );
          })}
        </div>
      ) : null}
    </div>
  );
}

// A home in the tree for sessions with no registered project: cardless New Sessions
// started outside any project path, or sessions whose project the daemon could not match
// by cwd. Without this a cardless session could be invisible (grouped under no project),
// which is exactly how a New Session "disappeared" from the sidebar on restart.
function LooseSessionsNode({ sessions }: { sessions: Session[] }) {
  const store = useStore();
  const { openMenu } = useContextMenu();
  const pool = useTerminalPool();
  const now = useNow();
  const [expanded, setExpanded] = useState(true);
  const [renaming, setRenaming] = useState<string | null>(null);
  const [draft, setDraft] = useState("");

  const liveSessions = useMemo(() => sessions.filter((s) => isLive(s.state)), [sessions]);
  const pastSessions = useMemo(() => sessions.filter((s) => !isLive(s.state)).slice(0, 6), [sessions]);
  const needsYou = liveSessions.filter((s) => ATTENTION_STATES.includes(s.state)).length;

  const openSessionRow = (s: Session) =>
    s.card_id ? store.openCard(s.card_id, "terminal") : store.openSession(s.id);
  const commitRename = (s: Session) => {
    const t = draft.trim();
    if (t) store.renameSession(s.id, t);
    setRenaming(null);
    setDraft("");
  };
  const killLive = (s: Session) => {
    store.client?.kill(s.id).catch(() => undefined);
    pool.evictTerminal(s.id);
    store.refresh().catch(() => undefined);
  };

  return (
    <div className="proj">
      <div className="proj-row">
        <button className="proj-caret" aria-label={expanded ? "Collapse" : "Expand"} onClick={() => setExpanded((v) => !v)}>
          <svg width="10" height="10" viewBox="0 0 10 10" className={expanded ? "is-open" : ""} aria-hidden>
            <path d="M3 2l4 3-4 3" stroke="currentColor" strokeWidth="1.4" fill="none" strokeLinecap="round" strokeLinejoin="round" />
          </svg>
        </button>
        <button className="proj-name-btn" onClick={() => setExpanded((v) => !v)} title="Sessions not tied to a registered project">
          <span className="proj-name">Loose sessions</span>
        </button>
        {!expanded ? (
          <span className="proj-badges">
            {liveSessions.length > 0 ? <span className="badge badge-live">{liveSessions.length}</span> : null}
            {needsYou > 0 ? <span className="badge badge-attention">{needsYou}</span> : null}
          </span>
        ) : null}
      </div>

      {expanded ? (
        <div className="proj-children">
          {liveSessions.length === 0 && pastSessions.length === 0 ? (
            <div className="proj-quiet">No loose sessions.</div>
          ) : null}

          {liveSessions.map((s) => {
            const agent = s.agent ?? harnessLabel(s.harness);
            const title = s.title ?? deriveSessionTitle({ firstPrompt: s.first_prompt, fallback: agent });
            const isRenaming = renaming === s.id;
            return (
              <SessionRow
                key={s.id}
                glyph={s.harness}
                title={title}
                agent={agent}
                note={s.status_note ?? null}
                state={s.state}
                since={s.state_since}
                now={now}
                renaming={isRenaming}
                draft={draft}
                onDraft={setDraft}
                onClick={() => openSessionRow(s)}
                onCommit={() => commitRename(s)}
                onCancel={() => { setRenaming(null); setDraft(""); }}
                onMenu={(e) =>
                  openMenu(e, [
                    { id: "open", label: "Open terminal", onSelect: () => openSessionRow(s) },
                    { id: "rename", label: "Rename", onSelect: () => { setRenaming(s.id); setDraft(title); } },
                    { id: "kill", label: "Kill session", danger: true, separatorBefore: true, onSelect: () => killLive(s) },
                  ])
                }
              />
            );
          })}

          {pastSessions.length > 0 ? <div className="tree-subhead">Recent</div> : null}
          {pastSessions.map((s) => {
            const agent = s.agent ?? harnessLabel(s.harness);
            const title = s.title ?? deriveSessionTitle({ firstPrompt: s.first_prompt, fallback: agent });
            const resumable = isResumable(s.state);
            return (
              <PastSessionRow
                key={s.id}
                glyph={s.harness}
                title={title}
                resumable={resumable}
                age={timeAgo(s.ended_at ?? s.state_since, now)}
                onClick={() => (resumable ? store.openSession(s.id) : openSessionRow(s))}
                onResume={resumable ? () => store.openSession(s.id) : undefined}
                onMenu={(e: React.MouseEvent) => {
                  const items: MenuItem[] = resumable
                    ? [
                        { id: "resume", label: "Resume session", onSelect: () => store.openSession(s.id) },
                        { id: "open", label: "Open", onSelect: () => store.openSession(s.id) },
                      ]
                    : [{ id: "open", label: "Open terminal", onSelect: () => openSessionRow(s) }];
                  openMenu(e, items);
                }}
              />
            );
          })}
        </div>
      ) : null}
    </div>
  );
}

function SessionRow({
  glyph,
  title,
  agent,
  note,
  state,
  since,
  now,
  renaming,
  draft,
  onDraft,
  onClick,
  onCommit,
  onCancel,
  onMenu,
}: {
  glyph: string;
  title: string;
  agent: string;
  note: string | null;
  state: Session["state"];
  since: number;
  now: number;
  renaming: boolean;
  draft: string;
  onDraft: (v: string) => void;
  onClick: () => void;
  onCommit: () => void;
  onCancel: () => void;
  onMenu: (e: React.MouseEvent) => void;
}) {
  return (
    <div className="tree-session-wrap">
      <button
        className={`tree-session${note ? " has-note" : ""}`}
        onClick={onClick}
        onContextMenu={onMenu}
        title={`${agent} - open terminal`}
      >
        <span className="tree-session-glyph" aria-hidden>
          <HarnessGlyph harness={glyph} />
        </span>
        <span className="tree-session-body">
          {renaming ? (
            <input
              className="tree-rename"
              value={draft}
              autoFocus
              onClick={(e) => e.stopPropagation()}
              onChange={(e) => onDraft(e.target.value)}
              onBlur={onCommit}
              onKeyDown={(e) => {
                if (e.key === "Enter") onCommit();
                if (e.key === "Escape") onCancel();
              }}
            />
          ) : (
            <span className="tree-session-label">{title}</span>
          )}
          {note && !renaming ? <span className="tree-session-note">{note}</span> : null}
        </span>
        <StateChip state={state} variant="mini" />
        <span className="tree-session-elapsed">{elapsed(since, now)}</span>
      </button>
    </div>
  );
}

function PastSessionRow({
  glyph,
  title,
  resumable,
  age,
  onClick,
  onResume,
  onMenu,
}: {
  glyph: string;
  title: string;
  resumable: boolean;
  age: string;
  onClick: () => void;
  onResume?: () => void;
  onMenu: (e: React.MouseEvent) => void;
}) {
  return (
    <button
      className={`tree-session is-past${resumable ? " is-resumable" : ""}`}
      onClick={onClick}
      onContextMenu={onMenu}
      title={resumable ? "Resume session" : "Open session"}
    >
      <span className="tree-session-glyph" aria-hidden>
        <HarnessGlyph harness={glyph} />
      </span>
      <span className="tree-session-label">{title}</span>
      {resumable ? (
        <span
          className="tree-resume"
          role="button"
          aria-label="Resume session"
          title="Resume with full context"
          onClick={(e) => {
            e.stopPropagation();
            onResume?.();
          }}
        >
          <svg width="12" height="12" viewBox="0 0 16 16" fill="none" aria-hidden>
            <path d="M5.5 3.5l6 4.5-6 4.5z" fill="currentColor" />
          </svg>
        </span>
      ) : (
        <span className="tree-session-age">{age}</span>
      )}
    </button>
  );
}
