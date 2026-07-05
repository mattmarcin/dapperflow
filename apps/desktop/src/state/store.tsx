// The board's single source of UI truth. Loads the snapshot through the DataSource
// (fixture or live), applies incoming card_events so lanes and session strips stay
// reactive, owns board mutations with optimistic updates, tracks the daemon
// connection for live terminals, and holds card->live-terminal bindings so the
// one-click-to-terminal invariant reuses a card's real session.

import {
  createContext,
  ReactNode,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { ConnectionStatus, DflowClient } from "../client";
import { getDaemonInfo } from "../daemon";
import { createDataSource, DataSource, usingFixtures } from "../data";
import {
  Agent,
  AgentAddInput,
  AgentMutationResult,
  AgentRemoveResult,
  AgentsDetectResult,
  AgentUpdateInput,
  AuditDepth,
  AuditStartResult,
  Card,
  CardCreateInput,
  CardEvent,
  ConcertmasterSession,
  ConcertmasterStartInput,
  DispatchStartInput,
  Lane,
  LaunchedSession,
  NeedsYouItem,
  Project,
  ProjectAddResult,
  FindingResolution,
  GateRun,
  GithubAuthStatus,
  GithubImportConfig,
  GithubImportResult,
  GithubIssue,
  Recipe,
  RecipeGrantError,
  RemoteListenerState,
  Session,
  SessionResumeResult,
  SessionStartInput,
} from "../model";
import { ArtifactMeta, FeedbackSubmit, FeedbackSubmitResult } from "../review/protocol";
import { deriveSessionTitle } from "../lib/format";
import { needsYouMeta } from "../lib/needs-you";
import {
  effectiveRecipeName,
  findRecipe,
  GRANT_PENDING_MESSAGE,
  grantKey,
  loadGrants,
  needsGrant,
  recipeGrantError,
  saveGrants,
} from "../lib/recipes";
import { ensureNotificationPermission, notify, onNotificationClick } from "../lib/notify";
import {
  loadNotificationPrefs,
  minNotifyScore,
  NotificationPrefs,
  saveNotificationPrefs,
} from "../lib/notification-prefs";
import {
  clampPanelWidth,
  loadPanelPrefs,
  PanelPrefs,
  savePanelPrefs,
} from "../lib/panel-prefs";
import { scopeSteerLine } from "../lib/concertmaster";
import { generateUlid } from "../ulid";

export type WorkspaceTab = "terminal" | "issue" | "timeline" | "plan" | "verify" | "diff";
export type DaemonStatus = ConnectionStatus | "absent";
export type AppView = "board" | "mission" | "settings";

const RECENT_EVENTS_CAP = 80;
const NOTIFY_QUIET_MS = 5 * 60_000; // one toast per item per quiet period (throttle)

export interface Toast {
  id: number;
  message: string;
  tone?: "default" | "danger";
  action?: { label: string; run: () => void };
}

export interface TerminalBinding {
  sessionId: string; // real daemon session id
  harness: string; // display harness for the card
  title?: string; // user label (session.rename); falls back to a generated name
}

export interface StoreValue {
  loading: boolean;
  error: string | null;
  fixtureMode: boolean;

  projects: Project[];
  cards: Card[];
  sessions: Session[];
  agents: Agent[];

  // Attention Router: the open Needs You queue (Mission Control), highest score first.
  needsYou: NeedsYouItem[];
  openNeedsYou: (item: NeedsYouItem) => void;

  // Recent cross-project activity, newest first (Mission Control feed).
  recentEvents: CardEvent[];

  // Desktop notification preferences (Settings > Notifications) + permission request.
  notificationPrefs: NotificationPrefs;
  setNotificationPrefs: (prefs: NotificationPrefs) => void;
  requestNotificationPermission: () => Promise<boolean>;

  // Session resume for interrupted sessions (architecture.md, session resume).
  resumeSession: (sessionId: string) => Promise<SessionResumeResult>;

  view: AppView;
  setView: (view: AppView) => void;

  filterProjectId: string | null;
  setFilterProject: (id: string | null) => void;

  openCardId: string | null;
  workspaceTab: WorkspaceTab;
  openCard: (id: string, tab?: WorkspaceTab) => void;
  closeCard: () => void;
  setWorkspaceTab: (tab: WorkspaceTab) => void;

  // Configured agents (Settings > Agents).
  refreshAgents: () => Promise<void>;
  detectAgents: () => Promise<AgentsDetectResult>;
  addAgent: (input: AgentAddInput) => Promise<AgentMutationResult>;
  updateAgent: (input: AgentUpdateInput) => Promise<AgentMutationResult>;
  removeAgent: (id: string) => Promise<AgentRemoveResult>;

  // Session-first front door (New Session): cardless live sessions tracked
  // client-side, since the daemon does not persist bare sessions with project or
  // launcher linkage (see the design notes).
  launches: LaunchedSession[];
  launchesForProject: (projectId: string | null) => LaunchedSession[];
  newSessionOpen: boolean;
  openNewSession: () => void;
  closeNewSession: () => void;
  startSession: (input: SessionStartInput) => Promise<string>;
  openSessionId: string | null;
  openSession: (sessionId: string) => void;
  closeSession: () => void;
  renameLaunch: (sessionId: string, title: string) => void;
  closeLaunch: (sessionId: string) => void;

  // The Concertmaster panel (product.md view 5): a dockable chat surface backed by a
  // real harness session with dflow-mcp mounted. Panel state persists open + width.
  panelOpen: boolean;
  panelWidth: number;
  togglePanel: () => void;
  setPanelOpen: (open: boolean) => void;
  setPanelWidth: (width: number) => void;
  // The single active Concertmaster session (cardless, flagged client-side), or null.
  concertmaster: ConcertmasterSession | null;
  startConcertmaster: (input: ConcertmasterStartInput) => Promise<string>;
  startConcertmasterDemo: () => void; // DEV: a fixture transcript, no live PTY
  endConcertmaster: () => void;
  restartConcertmaster: () => Promise<string | null>;
  setConcertmasterScope: (projectId: string | null) => void;
  focusConcertmaster: () => void; // open + reveal the panel

  daemon: DaemonStatus;
  daemonPort?: number;
  daemonVersion?: string;
  daemonStarted?: boolean; // this run spawned the daemon vs attached to a running one
  client: DflowClient | null;
  // Daemon kill switch (companion to the detach fix): stop is explicit, never a nag.
  stopDaemon: () => Promise<void>;
  restartDaemon: () => Promise<void>;

  // mutations
  createCard: (input: CardCreateInput) => Promise<Card>;
  moveCard: (cardId: string, lane: Lane, opts?: { silent?: boolean }) => Promise<void>;
  addProject: (path: string) => Promise<ProjectAddResult>;
  dispatch: (input: DispatchStartInput) => Promise<void>;
  cancelDispatch: (cardId: string) => void;
  renameSession: (sessionId: string, title: string) => void;

  // live terminal bindings (one-click-to-terminal)
  terminalsFor: (cardId: string) => TerminalBinding[];
  startTerminal: (cardId: string, harness: string) => Promise<string>;
  renameTerminal: (cardId: string, sessionId: string, title: string) => void;
  closeTerminal: (cardId: string, sessionId: string) => void;
  cardEvents: (cardId: string) => Promise<CardEvent[]>;

  // Flow recipes: the dial catalog (recipe.list) + the trust-tier grant flow.
  recipes: Recipe[];
  updateCardDial: (cardId: string, recipe: string | null) => Promise<void>;
  // A privileged recipe awaiting per-project consent (the specced structured error).
  // Confirming grants and re-runs the parked dispatch; declining just closes.
  pendingGrant: RecipeGrantError | null;
  grantPendingRecipe: () => void;
  declinePendingRecipe: () => void;
  isRecipeGranted: (recipeName: string, projectId: string | null) => boolean;
  // Record a grant directly (the dial's consent panel); persists project::recipe::hash.
  recordRecipeGrant: (projectId: string, recipeName: string, contentHash: string) => void;

  // Plan Studio artifact review (the Plan tab reads through these).
  getPlanArtifact: (cardId: string) => Promise<ArtifactMeta | null>;
  signArtifactUrl: (docId: string) => Promise<string>;
  submitFeedback: (input: FeedbackSubmit) => Promise<FeedbackSubmitResult>;
  approvePlan: (artifactId: string, cardId: string) => Promise<void>;

  // Onboarding audit: the offer (after add-project and from the Projects tree).
  auditOffer: Project | null;
  offerAudit: (project: Project) => void;
  dismissAuditOffer: () => void;
  startAudit: (projectId: string, depth: AuditDepth) => Promise<AuditStartResult>;

  // GitHub issue import (Settings > GitHub, Issue tab).
  githubAuthStatus: () => Promise<GithubAuthStatus>;
  getGithubImportConfig: (projectId: string) => Promise<GithubImportConfig>;
  setGithubImportConfig: (projectId: string, config: GithubImportConfig) => Promise<GithubImportConfig>;
  previewGithubIssues: (projectId: string) => Promise<GithubIssue[]>;
  importGithubIssues: (projectId: string, numbers: number[]) => Promise<GithubImportResult>;
  getGithubIssueForCard: (card: Card) => Promise<GithubIssue | null>;

  // Verification gate (Verify tab, board gate progress).
  getGateRun: (cardId: string) => Promise<GateRun | null>;
  startGate: (cardId: string) => Promise<{ ok: boolean; error?: string }>;
  resolveGateFinding: (cardId: string, findingId: string, resolution: FindingResolution) => Promise<GateRun>;
  mergePr: (cardId: string) => Promise<{ ok: boolean; error?: string }>;

  // Remote access / device pairing (Settings > Remote).
  getRemoteState: () => Promise<RemoteListenerState>;
  setRemoteEnabled: (enabled: boolean) => Promise<RemoteListenerState>;
  rotateRemoteToken: () => Promise<RemoteListenerState>;
  revokeRemoteDevice: (deviceId: string) => Promise<RemoteListenerState>;

  toast: Toast | null;
  flash: (message: string, opts?: { tone?: "default" | "danger"; action?: Toast["action"] }) => void;
  dismissToast: () => void;

  refresh: () => Promise<void>;
}

const StoreContext = createContext<StoreValue | null>(null);

const DEFAULT_COLS = 110;
const DEFAULT_ROWS = 30;

export function StoreProvider({ children }: { children: ReactNode }) {
  const fixtureMode = usingFixtures();
  const sourceRef = useRef<DataSource | null>(null);
  const clientRef = useRef<DflowClient | null>(null);
  const bindingsRef = useRef<Map<string, TerminalBinding[]>>(new Map());

  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [projects, setProjects] = useState<Project[]>([]);
  const [cards, setCards] = useState<Card[]>([]);
  const [sessions, setSessions] = useState<Session[]>([]);
  const [agents, setAgents] = useState<Agent[]>([]);
  const [needsYou, setNeedsYou] = useState<NeedsYouItem[]>([]);
  const [recentEvents, setRecentEvents] = useState<CardEvent[]>([]);
  const [recipes, setRecipes] = useState<Recipe[]>([]);
  const [pendingGrant, setPendingGrant] = useState<RecipeGrantError | null>(null);
  const [grantedRecipes, setGrantedRecipes] = useState<Set<string>>(() => loadGrants());
  const [auditOffer, setAuditOffer] = useState<Project | null>(null);
  const [notificationPrefs, setNotificationPrefsState] = useState<NotificationPrefs>(() =>
    loadNotificationPrefs(),
  );
  // Refs so cross-callback reads (startSession looking up a launcher/project, the event
  // handler building a notification) never close over stale state.
  const agentsRef = useRef<Agent[]>([]);
  agentsRef.current = agents;
  const projectsRef = useRef<Project[]>([]);
  projectsRef.current = projects;
  const cardsRef = useRef<Card[]>([]);
  cardsRef.current = cards;
  const prefsRef = useRef<NotificationPrefs>(notificationPrefs);
  prefsRef.current = notificationPrefs;
  const recipesRef = useRef<Recipe[]>([]);
  recipesRef.current = recipes;
  const grantsRef = useRef<Set<string>>(grantedRecipes);
  grantsRef.current = grantedRecipes;
  // The dispatch parked behind the consent modal, re-run after a grant.
  const pendingDispatchRef = useRef<DispatchStartInput | null>(null);
  // Throttle bookkeeping: dedupe_key -> last-notified epoch ms.
  const notifiedRef = useRef<Map<string, number>>(new Map());
  // Auto-detect launchers once per launch, after the daemon connects.
  const didAutoDetectRef = useRef(false);

  const [view, setView] = useState<AppView>("board");
  const [launches, setLaunches] = useState<LaunchedSession[]>([]);
  const [newSessionOpen, setNewSessionOpen] = useState(false);
  const [openSessionId, setOpenSessionId] = useState<string | null>(null);

  // Concertmaster panel: docked state (persisted) + the active session.
  const initialPanelPrefs = useRef<PanelPrefs>(loadPanelPrefs());
  const [panelOpen, setPanelOpenState] = useState(initialPanelPrefs.current.open);
  const [panelWidth, setPanelWidthState] = useState(initialPanelPrefs.current.width);
  const panelOpenRef = useRef(panelOpen);
  panelOpenRef.current = panelOpen;
  const [concertmaster, setConcertmaster] = useState<ConcertmasterSession | null>(null);
  // Ref so scope-steer and restart callbacks read the current session without stale closes.
  const concertmasterRef = useRef<ConcertmasterSession | null>(null);
  concertmasterRef.current = concertmaster;

  const [filterProjectId, setFilterProjectId] = useState<string | null>(null);
  const [openCardId, setOpenCardId] = useState<string | null>(null);
  const [workspaceTab, setWorkspaceTab] = useState<WorkspaceTab>("terminal");

  const [daemon, setDaemon] = useState<DaemonStatus>("connecting");
  const [daemonPort, setDaemonPort] = useState<number>();
  const [daemonVersion, setDaemonVersion] = useState<string>();
  const [daemonStarted, setDaemonStarted] = useState<boolean>();
  const [toast, setToast] = useState<Toast | null>(null);
  const toastSeq = useRef(0);
  const toastTimer = useRef<number>();
  const [, forceTick] = useState(0);

  const dismissToast = useCallback(() => setToast(null), []);
  const flash = useCallback<StoreValue["flash"]>((message, opts) => {
    const id = (toastSeq.current += 1);
    setToast({ id, message, tone: opts?.tone, action: opts?.action });
    if (toastTimer.current) window.clearTimeout(toastTimer.current);
    toastTimer.current = window.setTimeout(() => {
      setToast((t) => (t && t.id === id ? null : t));
    }, opts?.action ? 8000 : 4500);
  }, []);

  // Fire a throttled desktop notification for a Needs You arrival. Respects the master
  // switch, the high-priority gate, and a per-item quiet period; the deep-link routes a
  // click to the resolving card workspace tab.
  const maybeNotify = useCallback((item: NeedsYouItem) => {
    const prefs = prefsRef.current;
    if (!prefs.enabled) return;
    if (item.score < minNotifyScore(prefs)) return;
    const now = Date.now();
    // Client throttle honors the item's daemon-set notified_at until the daemon serves
    // that field itself (data-model.md: one notification per item per quiet period).
    const last = notifiedRef.current.get(item.dedupe_key) ?? item.notified_at ?? 0;
    if (now - last < NOTIFY_QUIET_MS) return;
    notifiedRef.current.set(item.dedupe_key, now);
    const card = cardsRef.current.find((c) => c.id === item.card_id);
    const meta = needsYouMeta(item.kind);
    const title = card?.title ?? meta.label;
    const body = item.note ? `${meta.label} - ${item.note}` : meta.label;
    notify({
      title,
      body,
      tag: item.dedupe_key,
      deepLink: { cardId: item.card_id, tab: meta.tab },
    }).catch(() => undefined);
  }, []);

  // Apply a card_event to local state so the board, the Needs You queue, and the
  // activity feed all react to automation and to the real daemon stream. Unknown kinds
  // still flow into the activity ring untouched (protocol.md: never drop them).
  const applyEvent = useCallback(
    (e: CardEvent) => {
      setRecentEvents((prev) => {
        if (prev.some((x) => x.id === e.id)) return prev; // event.subscribe can re-deliver
        return [e, ...prev].slice(0, RECENT_EVENTS_CAP);
      });

      if (e.kind === "moved" && e.payload && typeof e.payload.to === "string") {
        const to = e.payload.to as Lane;
        setCards((prev) => prev.map((c) => (c.id === e.card_id ? { ...c, lane: to, updated_at: e.ts } : c)));
      }
      if (e.kind === "state_changed" && e.payload && typeof e.payload.to === "string") {
        const to = e.payload.to as Session["state"];
        setSessions((prev) =>
          prev.map((s) => (s.card_id === e.card_id ? { ...s, state: to, state_since: e.ts } : s)),
        );
      }
      if ((e.kind === "needs_input" || e.kind === "blocked") && e.payload) {
        const note = (e.payload.question as string) || (e.payload.reason as string) || undefined;
        if (note) {
          setSessions((prev) => prev.map((s) => (s.card_id === e.card_id ? { ...s, status_note: note } : s)));
        }
      }
      if (e.kind === "turn_ended" && e.payload && typeof e.payload.note === "string") {
        const note = e.payload.note;
        setSessions((prev) => prev.map((s) => (s.card_id === e.card_id ? { ...s, status_note: note } : s)));
      }
      if (e.kind === "needs_you_raised") {
        const p = e.payload ?? {};
        const kind = (p.kind as string) ?? "agent_stuck";
        const dedupe = (p.dedupe_key as string) ?? `${kind}:${e.card_id}`;
        const item: NeedsYouItem = {
          id: e.id,
          card_id: e.card_id,
          kind,
          dedupe_key: dedupe,
          score: typeof p.score === "number" ? p.score : 50,
          raised_at: e.ts,
          notified_at: (p.notified_at as number) ?? null,
          note: (p.note as string) ?? null,
        };
        setNeedsYou((prev) =>
          [...prev.filter((n) => n.dedupe_key !== dedupe), item].sort((a, b) => b.score - a.score),
        );
        maybeNotify(item);
      }
      if (e.kind === "needs_you_resolved") {
        const dedupe = (e.payload?.dedupe_key as string) ?? undefined;
        setNeedsYou((prev) =>
          prev.filter((n) => (dedupe ? n.dedupe_key !== dedupe : n.card_id !== e.card_id)),
        );
      }
    },
    [maybeNotify],
  );

  const refresh = useCallback(async () => {
    const source = sourceRef.current;
    if (!source) return;
    const snap = await source.loadSnapshot();
    setProjects(snap.projects);
    setCards(snap.cards);
    setSessions(snap.sessions);
    setNeedsYou(snap.needsYou);
    // Reconcile client-side cardless launches against the live fleet: mark a launch
    // ended once its PTY is gone (the daemon carries no project/launcher linkage for
    // bare sessions, so aliveness is all it can confirm).
    setLaunches((prev) =>
      prev.map((l) => {
        const wire = snap.sessions.find((s) => s.id === l.sessionId);
        return wire ? { ...l, alive: wire.state !== "done" && wire.state !== "error" } : l;
      }),
    );
    // Same aliveness reconciliation for the Concertmaster session (a bare session the
    // daemon carries with no concertmaster linkage, so aliveness is all it confirms).
    setConcertmaster((prev) => {
      if (!prev || prev.demo) return prev;
      const wire = snap.sessions.find((s) => s.id === prev.sessionId);
      return wire ? { ...prev, alive: wire.state !== "done" && wire.state !== "error" } : prev;
    });
  }, []);

  const refreshAgents = useCallback(async () => {
    const source = sourceRef.current;
    if (!source) return;
    setAgents(await source.listAgents());
  }, []);

  // Bootstrap: connect to the daemon (for live terminals) and load the board.
  useEffect(() => {
    let cancelled = false;
    let unsubscribe: (() => void) | undefined;

    (async () => {
      let client: DflowClient | null = null;
      try {
        const info = await getDaemonInfo();
        if (cancelled) return;
        setDaemonPort(info.port);
        setDaemonStarted(info.started);
        client = new DflowClient(info.port, info.token);
        client.onStatus = (s) => setDaemon(s);
        client.onReconnect = () => {
          // In live mode a reconnect should re-pull the board.
          if (!fixtureMode) refresh().catch(() => undefined);
        };
        await client.connect();
        if (cancelled) return;
        setDaemonVersion(client.daemonVersion);
        clientRef.current = client;
      } catch {
        // No daemon reachable. Fixture mode still runs fully; live terminals are
        // unavailable until the daemon is up (handled by the daemon banner).
        if (!cancelled) setDaemon("absent");
      }

      if (cancelled) return;
      try {
        sourceRef.current = createDataSource(clientRef.current);
        await refresh();
        if (cancelled) return;
        // Configured launchers power the New Session picker and Settings > Agents;
        // load them once at boot (a failure here must not block the board).
        await refreshAgents().catch(() => undefined);
        if (cancelled) return;
        // Seed the Mission Control activity feed; the live stream fills it thereafter.
        sourceRef.current
          .recentActivity()
          .then((evs) => {
            if (!cancelled) setRecentEvents(evs);
          })
          .catch(() => undefined);
        // The dial catalog (recipe.list; the bundled catalog until the crate lands).
        sourceRef.current
          .listRecipes()
          .then((list) => {
            if (!cancelled) setRecipes(list);
          })
          .catch(() => undefined);
        // Auto-detect installed launchers once per launch, after the daemon connects
        // (orchestrator request). Silent; the manual "Detect" button stays as a re-scan.
        // Skipped without a live daemon so fixture mode and reconnects never spam it.
        if (!didAutoDetectRef.current && clientRef.current?.connected) {
          didAutoDetectRef.current = true;
          sourceRef.current
            .detectAgents()
            .then((res) => {
              if (!cancelled) setAgents(res.agents);
            })
            .catch(() => undefined);
        }
        unsubscribe = sourceRef.current.subscribeEvents(applyEvent);
        setLoading(false);
      } catch (e) {
        if (!cancelled) {
          setError(messageOf(e));
          setLoading(false);
        }
      }
    })();

    return () => {
      cancelled = true;
      unsubscribe?.();
      clientRef.current?.close();
    };
  }, [fixtureMode, refresh, refreshAgents, applyEvent]);

  const openCard = useCallback((id: string, tab: WorkspaceTab = "terminal") => {
    setOpenCardId(id);
    setWorkspaceTab(tab);
    setOpenSessionId(null); // card and cardless-session overlays are mutually exclusive
  }, []);
  const closeCard = useCallback(() => setOpenCardId(null), []);

  // Attention Router deep link: open the exact card workspace tab that resolves the item
  // (product.md: each Needs You item deep-links to its resolving surface).
  const openNeedsYou = useCallback((item: NeedsYouItem) => {
    // An audit digest resolves in the Inbox, not a card tab: jump to the board
    // filtered to the audited project so bulk triage is one click away.
    if (item.kind === "audit_digest") {
      const card = cardsRef.current.find((c) => c.id === item.card_id);
      setView("board");
      setFilterProjectId(card?.project_id ?? null);
      setOpenCardId(null);
      setOpenSessionId(null);
      return;
    }
    const meta = needsYouMeta(item.kind);
    setView("board");
    setOpenCardId(item.card_id);
    setWorkspaceTab(meta.tab);
    setOpenSessionId(null);
  }, []);

  // Route a notification click to the resolving surface and focus the app window.
  useEffect(() => {
    onNotificationClick((link) => {
      try {
        window.focus();
      } catch {
        /* no-op */
      }
      if (link.cardId) {
        setView("board");
        setOpenCardId(link.cardId);
        setWorkspaceTab((link.tab as WorkspaceTab) ?? "terminal");
        setOpenSessionId(null);
      }
    });
  }, []);

  // --- Configured agents (Settings > Agents) --------------------------------

  const detectAgents = useCallback(async (): Promise<AgentsDetectResult> => {
    const res = await sourceRef.current!.detectAgents();
    setAgents(res.agents);
    return res;
  }, []);

  const addAgent = useCallback(async (input: AgentAddInput): Promise<AgentMutationResult> => {
    const res = await sourceRef.current!.addAgent(input);
    if (res.ok && res.agent) setAgents((prev) => [...prev, res.agent!]);
    return res;
  }, []);

  const updateAgent = useCallback(async (input: AgentUpdateInput): Promise<AgentMutationResult> => {
    const res = await sourceRef.current!.updateAgent(input);
    if (res.ok && res.agent) {
      const updated = res.agent;
      setAgents((prev) => prev.map((a) => (a.id === updated.id ? updated : a)));
    }
    return res;
  }, []);

  const removeAgent = useCallback(async (id: string): Promise<AgentRemoveResult> => {
    const res = await sourceRef.current!.removeAgent(id);
    if (res.ok) setAgents((prev) => prev.filter((a) => a.id !== id && a.name !== id));
    return res;
  }, []);

  // --- Session-first front door (New Session) -------------------------------

  const launchesForProject = useCallback(
    (projectId: string | null) => launches.filter((l) => l.projectId === projectId),
    [launches],
  );

  const openNewSession = useCallback(() => setNewSessionOpen(true), []);
  const closeNewSession = useCallback(() => setNewSessionOpen(false), []);

  const openSession = useCallback((sessionId: string) => {
    setOpenSessionId(sessionId);
    setOpenCardId(null);
  }, []);
  const closeSession = useCallback(() => setOpenSessionId(null), []);

  const startSession = useCallback(async (input: SessionStartInput): Promise<string> => {
    const client = clientRef.current;
    if (!client || !client.connected)
      throw new Error("The daemon is offline. Start dflowd to open a live session.");
    const agent = agentsRef.current.find((a) => a.id === input.agent || a.name === input.agent);
    const project = input.projectId ? projectsRef.current.find((p) => p.id === input.projectId) : undefined;
    // session.create resolves the launcher from `agent`, sets harness to the adapter
    // family, and uses cwd = the project path for now (worktree-leased New Session
    // arrives with the dflow-cli phase). See protocol.md session.create.
    const { session_id } = await client.createSession({
      harness: agent?.adapter ?? "",
      agent: input.agent,
      cols: DEFAULT_COLS,
      rows: DEFAULT_ROWS,
      cwd: input.cwd ?? project?.path ?? undefined,
    });
    // A generated title that reflects the work, not just the CLI name: derive it from
    // the first prompt (a cardless session has no card title) and persist it via
    // session.rename so the tree and tabs read meaningfully. A user rename still wins.
    const hasPrompt = !!input.firstPrompt?.trim();
    const derivedTitle = hasPrompt
      ? deriveSessionTitle({ firstPrompt: input.firstPrompt, fallback: agent?.name ?? input.agent })
      : null;
    if (derivedTitle) client.rename(session_id, derivedTitle).catch(() => undefined);
    const launch: LaunchedSession = {
      sessionId: session_id,
      agent: agent?.name ?? input.agent,
      harness: agent?.adapter ?? "custom",
      projectId: input.projectId,
      firstPrompt: input.firstPrompt ?? null,
      title: derivedTitle,
      createdAt: Date.now(),
      alive: true,
    };
    setLaunches((prev) => [launch, ...prev]);
    setOpenSessionId(session_id);
    setOpenCardId(null);
    setNewSessionOpen(false);
    return session_id;
  }, []);

  const renameLaunch = useCallback((sessionId: string, title: string) => {
    setLaunches((prev) => prev.map((l) => (l.sessionId === sessionId ? { ...l, title } : l)));
    clientRef.current?.rename(sessionId, title).catch(() => undefined);
  }, []);

  const closeLaunch = useCallback((sessionId: string) => {
    setLaunches((prev) => prev.filter((l) => l.sessionId !== sessionId));
    setOpenSessionId((cur) => (cur === sessionId ? null : cur));
    clientRef.current?.kill(sessionId).catch(() => undefined);
  }, []);

  // --- Concertmaster panel ---------------------------------------------------

  const persistPanel = useCallback((open: boolean, width: number) => {
    savePanelPrefs({ open, width: clampPanelWidth(width) });
  }, []);

  const setPanelOpen = useCallback(
    (open: boolean) => {
      setPanelOpenState(open);
      setPanelWidthState((w) => {
        persistPanel(open, w);
        return w;
      });
    },
    [persistPanel],
  );

  const togglePanel = useCallback(() => setPanelOpen(!panelOpenRef.current), [setPanelOpen]);

  const setPanelWidth = useCallback(
    (width: number) => {
      const clamped = clampPanelWidth(width);
      setPanelWidthState(clamped);
      persistPanel(panelOpenRef.current, clamped);
    },
    [persistPanel],
  );

  const focusConcertmaster = useCallback(() => setPanelOpen(true), [setPanelOpen]);

  const startConcertmaster = useCallback(
    async (input: ConcertmasterStartInput): Promise<string> => {
      const client = clientRef.current;
      if (!client || !client.connected)
        throw new Error("The daemon is offline. Start dflowd to summon the Concertmaster.");
      const agent = agentsRef.current.find((a) => a.id === input.agent || a.name === input.agent);
      // session.create resolves the launcher command + env and records the adapter
      // family as the session harness; cwd is where dflow-mcp mounts and scope lives.
      const { session_id } = await client.createSession({
        harness: agent?.adapter ?? "",
        agent: input.agent,
        cols: DEFAULT_COLS,
        rows: DEFAULT_ROWS,
        cwd: input.cwd ?? undefined,
      });
      // A stable, legible name in the tree and fleet (a user rename still wins).
      client.rename(session_id, "Concertmaster").catch(() => undefined);
      const cm: ConcertmasterSession = {
        sessionId: session_id,
        agentId: agent?.id ?? input.agent,
        agentName: agent?.name ?? input.agent,
        harness: agent?.adapter ?? "custom",
        scopeProjectId: input.scopeProjectId ?? null,
        mounted: input.mounted ?? null,
        createdAt: Date.now(),
        alive: true,
        demo: false,
      };
      setConcertmaster(cm);
      setPanelOpen(true);
      return session_id;
    },
    [setPanelOpen],
  );

  const startConcertmasterDemo = useCallback(() => {
    // DEV/offline: a fixture transcript with no live PTY, for screenshots. The panel
    // renders the transcript read-only and scrapes deep links straight from its text.
    const cm: ConcertmasterSession = {
      sessionId: generateUlid(),
      agentId: "demo",
      agentName: "claude · haiku",
      harness: "claude",
      scopeProjectId: null,
      mounted: true,
      createdAt: Date.now(),
      alive: true,
      demo: true,
    };
    setConcertmaster(cm);
    setPanelOpen(true);
  }, [setPanelOpen]);

  const endConcertmaster = useCallback(() => {
    const cm = concertmasterRef.current;
    setConcertmaster(null);
    if (cm && !cm.demo) clientRef.current?.kill(cm.sessionId).catch(() => undefined);
  }, []);

  const restartConcertmaster = useCallback(async (): Promise<string | null> => {
    const cm = concertmasterRef.current;
    if (!cm) return null;
    if (cm.demo) {
      startConcertmasterDemo();
      return null;
    }
    // Kill the old PTY, then relaunch the same launcher with the same scope. The panel
    // swaps its TerminalSlot to the new id and evicts the old terminal from the pool.
    clientRef.current?.kill(cm.sessionId).catch(() => undefined);
    return startConcertmaster({
      agent: cm.agentId,
      cwd: projectsRef.current.find((p) => p.id === cm.scopeProjectId)?.path ?? null,
      scopeProjectId: cm.scopeProjectId,
      mounted: cm.mounted,
    });
  }, [startConcertmaster, startConcertmasterDemo]);

  const setConcertmasterScope = useCallback((projectId: string | null) => {
    setConcertmaster((prev) => (prev ? { ...prev, scopeProjectId: projectId } : prev));
    const cm = concertmasterRef.current;
    if (!cm) return;
    // Steer the session to the new focus (product.md scoped sessions). We preload the
    // context line into the PTY WITHOUT auto-submit: verified submit is a daemon verb
    // the panel cannot invoke yet (phase6-mcp.md merge-time request 1), so the honest
    // behavior is to type it and let the user send. Demo sessions have no PTY.
    if (projectId && !cm.demo) {
      const project = projectsRef.current.find((p) => p.id === projectId);
      if (project) clientRef.current?.sendInput(cm.sessionId, scopeSteerLine(project));
    }
  }, []);

  const createCard = useCallback(async (input: CardCreateInput): Promise<Card> => {
    const source = sourceRef.current!;
    const card = await source.createCard(input);
    setCards((prev) => [...prev, card]);
    return card;
  }, []);

  const moveCard = useCallback(
    async (cardId: string, lane: Lane, opts?: { silent?: boolean }) => {
      // Optimistic move; reconcile from the response.
      let prevLane: Lane | undefined;
      setCards((prev) =>
        prev.map((c) => {
          if (c.id === cardId) {
            prevLane = c.lane;
            return { ...c, lane };
          }
          return c;
        }),
      );
      try {
        const updated = await sourceRef.current!.moveCard(cardId, lane);
        setCards((prev) => prev.map((c) => (c.id === cardId ? updated : c)));
      } catch (e) {
        // Roll back on failure.
        if (prevLane) setCards((prev) => prev.map((c) => (c.id === cardId ? { ...c, lane: prevLane! } : c)));
        if (!opts?.silent) throw e;
      }
    },
    [],
  );

  const addProject = useCallback(async (path: string): Promise<ProjectAddResult> => {
    const res = await sourceRef.current!.addProject(path);
    if (res.ok && res.project) {
      const project = res.project;
      setProjects((prev) => [...prev, project]);
      // Offer, never force (product.md): registering a project offers an audit.
      setAuditOffer(project);
    }
    return res;
  }, []);

  const dispatch = useCallback(
    async (input: DispatchStartInput) => {
      // Trust-tier gate (security.md): resolve the card's effective recipe; an
      // ungranted privileged recipe parks the dispatch behind the consent modal and
      // throws the marker so callers skip their failure toast. The daemon enforces
      // this server-side too once the recipes crate lands; the UI never bypasses it.
      const card = cardsRef.current.find((c) => c.id === input.card_id);
      const project = card?.project_id
        ? projectsRef.current.find((p) => p.id === card.project_id)
        : undefined;
      const recipeName =
        input.recipe ?? effectiveRecipeName(card?.dial_recipe, project?.default_recipe);
      const recipe = findRecipe(recipesRef.current, recipeName);
      if (recipe && needsGrant(recipe, card?.project_id ?? null, grantsRef.current)) {
        pendingDispatchRef.current = input;
        setPendingGrant(recipeGrantError(recipe, card?.project_id ?? ""));
        throw new Error(GRANT_PENDING_MESSAGE);
      }
      await sourceRef.current!.dispatch({ ...input, recipe: input.recipe ?? recipeName });
      await refresh();
    },
    [refresh],
  );

  const cancelDispatch = useCallback((cardId: string) => {
    sourceRef.current?.cancelDispatch(cardId).catch(() => undefined);
  }, []);

  const renameSession = useCallback((sessionId: string, title: string) => {
    setSessions((prev) => prev.map((s) => (s.id === sessionId ? { ...s, title } : s)));
    sourceRef.current?.renameSession(sessionId, title).catch(() => undefined);
  }, []);

  const terminalsFor = useCallback((cardId: string) => bindingsRef.current.get(cardId) ?? [], []);

  const startTerminal = useCallback(async (cardId: string, harness: string): Promise<string> => {
    const client = clientRef.current;
    if (!client || !client.connected)
      throw new Error("The daemon is offline. Start dflowd to open a live terminal.");
    // Phase 0 daemon spawns a PowerShell PTY; the chosen harness is shown as the
    // channel identity and can be launched inside the shell (as in Phase 0).
    const { session_id } = await client.createSession({
      harness: "powershell",
      cols: DEFAULT_COLS,
      rows: DEFAULT_ROWS,
    });
    const list = bindingsRef.current.get(cardId) ?? [];
    bindingsRef.current.set(cardId, [...list, { sessionId: session_id, harness }]);
    forceTick((n) => n + 1);
    return session_id;
  }, []);

  const renameTerminal = useCallback((cardId: string, sessionId: string, title: string) => {
    const list = bindingsRef.current.get(cardId);
    if (!list) return;
    bindingsRef.current.set(
      cardId,
      list.map((b) => (b.sessionId === sessionId ? { ...b, title } : b)),
    );
    forceTick((n) => n + 1);
    clientRef.current?.rename(sessionId, title).catch(() => undefined);
  }, []);

  const closeTerminal = useCallback((cardId: string, sessionId: string) => {
    const list = bindingsRef.current.get(cardId) ?? [];
    bindingsRef.current.set(
      cardId,
      list.filter((b) => b.sessionId !== sessionId),
    );
    forceTick((n) => n + 1);
    clientRef.current?.kill(sessionId).catch(() => undefined);
  }, []);

  const cardEvents = useCallback((cardId: string) => sourceRef.current!.cardEvents(cardId), []);

  // --- Flow recipes: dial + trust-tier grants --------------------------------

  const updateCardDial = useCallback(async (cardId: string, recipe: string | null) => {
    // Optimistic dial turn; reconcile from the response (dial_changed also streams).
    let prevDial: string | null = null;
    setCards((prev) =>
      prev.map((c) => {
        if (c.id === cardId) {
          prevDial = c.dial_recipe;
          return { ...c, dial_recipe: recipe };
        }
        return c;
      }),
    );
    try {
      const updated = await sourceRef.current!.updateCardDial(cardId, recipe);
      setCards((prev) => prev.map((c) => (c.id === cardId ? updated : c)));
    } catch (e) {
      setCards((prev) => prev.map((c) => (c.id === cardId ? { ...c, dial_recipe: prevDial } : c)));
      throw e;
    }
  }, []);

  const isRecipeGranted = useCallback((recipeName: string, projectId: string | null) => {
    const recipe = findRecipe(recipesRef.current, recipeName);
    return !needsGrant(recipe, projectId, grantsRef.current);
  }, []);

  const recordRecipeGrant = useCallback(
    (projectId: string, recipeName: string, contentHash: string) => {
      setGrantedRecipes((prev) => {
        const next = new Set(prev);
        next.add(grantKey(projectId, recipeName, contentHash));
        saveGrants(next);
        grantsRef.current = next;
        return next;
      });
    },
    [],
  );

  const grantPendingRecipe = useCallback(() => {
    const err = pendingGrant;
    if (!err) return;
    // Record the grant (scoped to project + recipe + hash) and resume the parked
    // dispatch, if any.
    setGrantedRecipes((prev) => {
      const next = new Set(prev);
      next.add(grantKey(err.project_id, err.recipe, err.contentHash));
      saveGrants(next);
      grantsRef.current = next;
      return next;
    });
    setPendingGrant(null);
    const parked = pendingDispatchRef.current;
    pendingDispatchRef.current = null;
    if (parked) {
      sourceRef.current!
        .dispatch({ ...parked, recipe: parked.recipe ?? err.recipe })
        .then(() => refresh())
        .then(() => flash("Granted. Dispatched under the elevated recipe."))
        .catch((e) => flash(messageOf(e), { tone: "danger" }));
    } else {
      flash(`Granted "${err.recipe}" for this project.`);
    }
  }, [pendingGrant, refresh, flash]);

  const declinePendingRecipe = useCallback(() => {
    pendingDispatchRef.current = null;
    setPendingGrant(null);
  }, []);

  // --- Plan Studio artifact review -------------------------------------------

  const getPlanArtifact = useCallback(
    (cardId: string) => sourceRef.current!.getPlanArtifact(cardId),
    [],
  );
  const signArtifactUrl = useCallback(
    (docId: string) => sourceRef.current!.signArtifactUrl(docId),
    [],
  );
  const submitFeedback = useCallback(
    (input: FeedbackSubmit) => sourceRef.current!.submitFeedback(input),
    [],
  );
  const approvePlan = useCallback(async (artifactId: string, cardId: string) => {
    await sourceRef.current!.approvePlan(artifactId, cardId);
    // Fixture resolves through emitted events; a live daemon streams the same. Either
    // way the local queue drops the plan_round item immediately for responsiveness.
    setNeedsYou((prev) => prev.filter((n) => !(n.kind === "plan_round" && n.card_id === cardId)));
  }, []);

  // --- Onboarding audit --------------------------------------------------------

  const offerAudit = useCallback((project: Project) => setAuditOffer(project), []);
  const dismissAuditOffer = useCallback(() => setAuditOffer(null), []);

  const startAudit = useCallback(
    async (projectId: string, depth: AuditDepth): Promise<AuditStartResult> => {
      const res = await sourceRef.current!.startAudit(projectId, depth);
      // The audit files cards into Inbox and raises its digest; re-pull the board.
      if (res.ok) await refresh();
      return res;
    },
    [refresh],
  );

  // --- GitHub issue import ---------------------------------------------------

  const githubAuthStatus = useCallback(() => sourceRef.current!.githubAuthStatus(), []);
  const getGithubImportConfig = useCallback(
    (projectId: string) => sourceRef.current!.getGithubImportConfig(projectId),
    [],
  );
  const setGithubImportConfig = useCallback(
    (projectId: string, config: GithubImportConfig) =>
      sourceRef.current!.setGithubImportConfig(projectId, config),
    [],
  );
  const previewGithubIssues = useCallback(
    (projectId: string) => sourceRef.current!.previewGithubIssues(projectId),
    [],
  );
  const importGithubIssues = useCallback(
    async (projectId: string, numbers: number[]): Promise<GithubImportResult> => {
      const res = await sourceRef.current!.importGithubIssues(projectId, numbers);
      // Imported issues land as origin:github_issue cards in Inbox; re-pull the board.
      if (res.ok && (res.imported > 0 || res.refreshed > 0)) await refresh();
      return res;
    },
    [refresh],
  );
  const getGithubIssueForCard = useCallback(
    (card: Card) => sourceRef.current!.getGithubIssueForCard(card),
    [],
  );

  // --- Verification gate -----------------------------------------------------

  const getGateRun = useCallback((cardId: string) => sourceRef.current!.getGateRun(cardId), []);
  const startGate = useCallback(
    async (cardId: string): Promise<{ ok: boolean; error?: string }> => {
      const res = await sourceRef.current!.startGate(cardId);
      // A gate run moves the card to Verifying; the fixture emits the moves, but re-pull
      // so a live daemon's projection reconciles immediately too.
      if (res.ok) await refresh();
      return res;
    },
    [refresh],
  );
  const resolveGateFinding = useCallback(
    (cardId: string, findingId: string, resolution: FindingResolution) =>
      sourceRef.current!.resolveGateFinding(cardId, findingId, resolution),
    [],
  );
  const mergePr = useCallback(
    async (cardId: string): Promise<{ ok: boolean; error?: string }> => {
      const res = await sourceRef.current!.mergePr(cardId);
      if (res.ok) await refresh();
      return res;
    },
    [refresh],
  );

  // --- Remote access / device pairing ----------------------------------------

  const getRemoteState = useCallback(() => sourceRef.current!.getRemoteState(), []);
  const setRemoteEnabled = useCallback((enabled: boolean) => sourceRef.current!.setRemoteEnabled(enabled), []);
  const rotateRemoteToken = useCallback(() => sourceRef.current!.rotateRemoteToken(), []);
  const revokeRemoteDevice = useCallback((deviceId: string) => sourceRef.current!.revokeRemoteDevice(deviceId), []);

  // --- Session resume (interrupted sessions) --------------------------------

  const resumeSession = useCallback(
    async (sessionId: string): Promise<SessionResumeResult> => {
      const res = await sourceRef.current!.resumeSession(sessionId);
      // On success the fleet gains the resumed session (new row, resumed_from lineage);
      // re-pull so the tree and Mission Control reflect it.
      if (res.ok) await refresh();
      return res;
    },
    [refresh],
  );

  // --- Notifications --------------------------------------------------------

  const setNotificationPrefs = useCallback((prefs: NotificationPrefs) => {
    setNotificationPrefsState(prefs);
    saveNotificationPrefs(prefs);
    // Turning notifications on is the natural moment to request OS permission.
    if (prefs.enabled) ensureNotificationPermission().catch(() => undefined);
  }, []);

  const requestNotificationPermission = useCallback(() => ensureNotificationPermission(), []);

  // --- Daemon kill switch ---------------------------------------------------

  const stopDaemon = useCallback(async () => {
    const client = clientRef.current;
    if (!client) return;
    try {
      // The daemon exits before replying, so a thrown error here is the success path.
      await client.shutdownDaemon();
    } catch {
      /* expected: socket closes as the daemon exits */
    }
    setDaemon("absent");
  }, []);

  const restartDaemon = useCallback(async () => {
    const client = clientRef.current;
    try {
      await client?.shutdownDaemon();
    } catch {
      /* expected on shutdown */
    }
    setDaemon("connecting");
    // Give the daemon a moment to exit, then reload: bootstrap re-runs the Tauri
    // ensure_running path, which respawns a detached daemon (the old port no longer
    // answers) and reconnects. Live sessions reconcile as interrupted on startup
    // (architecture.md). In browser dev the daemon is external and is not respawned;
    // documented in the evidence file.
    window.setTimeout(() => window.location.reload(), 900);
  }, []);

  const value = useMemo<StoreValue>(
    () => ({
      loading,
      error,
      fixtureMode,
      projects,
      cards,
      sessions,
      agents,
      needsYou,
      openNeedsYou,
      recentEvents,
      notificationPrefs,
      setNotificationPrefs,
      requestNotificationPermission,
      resumeSession,
      view,
      setView,
      filterProjectId,
      setFilterProject: setFilterProjectId,
      openCardId,
      workspaceTab,
      openCard,
      closeCard,
      setWorkspaceTab,
      refreshAgents,
      detectAgents,
      addAgent,
      updateAgent,
      removeAgent,
      launches,
      launchesForProject,
      newSessionOpen,
      openNewSession,
      closeNewSession,
      startSession,
      openSessionId,
      openSession,
      closeSession,
      renameLaunch,
      closeLaunch,
      panelOpen,
      panelWidth,
      togglePanel,
      setPanelOpen,
      setPanelWidth,
      concertmaster,
      startConcertmaster,
      startConcertmasterDemo,
      endConcertmaster,
      restartConcertmaster,
      setConcertmasterScope,
      focusConcertmaster,
      daemon,
      daemonPort,
      daemonVersion,
      daemonStarted,
      client: clientRef.current,
      stopDaemon,
      restartDaemon,
      createCard,
      moveCard,
      addProject,
      dispatch,
      cancelDispatch,
      renameSession,
      terminalsFor,
      startTerminal,
      renameTerminal,
      closeTerminal,
      cardEvents,
      recipes,
      updateCardDial,
      pendingGrant,
      grantPendingRecipe,
      declinePendingRecipe,
      isRecipeGranted,
      recordRecipeGrant,
      getPlanArtifact,
      signArtifactUrl,
      submitFeedback,
      approvePlan,
      auditOffer,
      offerAudit,
      dismissAuditOffer,
      startAudit,
      githubAuthStatus,
      getGithubImportConfig,
      setGithubImportConfig,
      previewGithubIssues,
      importGithubIssues,
      getGithubIssueForCard,
      getGateRun,
      startGate,
      resolveGateFinding,
      mergePr,
      getRemoteState,
      setRemoteEnabled,
      rotateRemoteToken,
      revokeRemoteDevice,
      toast,
      flash,
      dismissToast,
      refresh,
    }),
    [
      loading,
      error,
      fixtureMode,
      projects,
      cards,
      sessions,
      agents,
      needsYou,
      openNeedsYou,
      recentEvents,
      notificationPrefs,
      setNotificationPrefs,
      requestNotificationPermission,
      resumeSession,
      view,
      filterProjectId,
      openCardId,
      workspaceTab,
      openCard,
      closeCard,
      refreshAgents,
      detectAgents,
      addAgent,
      updateAgent,
      removeAgent,
      launches,
      launchesForProject,
      newSessionOpen,
      openNewSession,
      closeNewSession,
      startSession,
      openSessionId,
      openSession,
      closeSession,
      renameLaunch,
      closeLaunch,
      panelOpen,
      panelWidth,
      togglePanel,
      setPanelOpen,
      setPanelWidth,
      concertmaster,
      startConcertmaster,
      startConcertmasterDemo,
      endConcertmaster,
      restartConcertmaster,
      setConcertmasterScope,
      focusConcertmaster,
      daemon,
      daemonPort,
      daemonVersion,
      daemonStarted,
      stopDaemon,
      restartDaemon,
      createCard,
      moveCard,
      addProject,
      dispatch,
      cancelDispatch,
      renameSession,
      terminalsFor,
      startTerminal,
      renameTerminal,
      closeTerminal,
      cardEvents,
      recipes,
      updateCardDial,
      pendingGrant,
      grantPendingRecipe,
      declinePendingRecipe,
      isRecipeGranted,
      recordRecipeGrant,
      getPlanArtifact,
      signArtifactUrl,
      submitFeedback,
      approvePlan,
      auditOffer,
      offerAudit,
      dismissAuditOffer,
      startAudit,
      githubAuthStatus,
      getGithubImportConfig,
      setGithubImportConfig,
      previewGithubIssues,
      importGithubIssues,
      getGithubIssueForCard,
      getGateRun,
      startGate,
      resolveGateFinding,
      mergePr,
      getRemoteState,
      setRemoteEnabled,
      rotateRemoteToken,
      revokeRemoteDevice,
      toast,
      flash,
      dismissToast,
      refresh,
    ],
  );

  return <StoreContext.Provider value={value}>{children}</StoreContext.Provider>;
}

export function useStore(): StoreValue {
  const ctx = useContext(StoreContext);
  if (!ctx) throw new Error("useStore must be used within StoreProvider");
  return ctx;
}

function messageOf(e: unknown): string {
  if (e instanceof Error) return e.message;
  if (typeof e === "string") return e;
  if (e && typeof e === "object" && "message" in e) return String((e as { message: unknown }).message);
  return String(e);
}
