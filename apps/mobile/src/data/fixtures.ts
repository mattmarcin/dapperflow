// ============================================================================
// DEMO ONLY - the phone's fixture data source.
// Serves a realistic attention surface through the same MobileDataSource interface
// as the live protocol source, so every view is fully demonstrable before the LAN
// listener and pairing endpoints exist (they do not yet). None of this ships in a
// real daemon build; live mode replaces it wholesale (see live.ts, data/index.ts).
// ============================================================================

import { StyledRun } from "../client/protocol";
import { Card, FleetSnapshot, NeedsYouItem, PlanArtifact, Project, Session } from "../client/model";
import { ActionResult, MobileDataSource, TerminalPeek } from "./source";

const MIN = 60_000;
const now = Date.now();
const ago = (minutes: number) => now - minutes * MIN;

let counter = 0;
const id = (prefix: string) => `${prefix}_${(counter++).toString(36).padStart(4, "0")}`;

// --- Projects ---------------------------------------------------------------

const P_SVC = id("p");
const P_WEB = id("p");
const P_CLI = id("p");

const projects: Project[] = [
  { id: P_SVC, name: "ledger-svc", path: "C:\\Users\\m\\code\\ledger-svc" },
  { id: P_WEB, name: "dappertoast-web", path: "C:\\Users\\m\\code\\dappertoast-web" },
  { id: P_CLI, name: "orchard-cli", path: "C:\\Users\\m\\code\\orchard-cli" },
];

const projName = (pid: string | null) => projects.find((p) => p.id === pid)?.name ?? null;

// --- Cards (carried for deep-link context) ----------------------------------

interface CardSeed {
  handle: string;
  project: string;
  type: Card["type"];
  title: string;
  lane: Card["lane"];
  priority?: number;
}

const cardSeeds: CardSeed[] = [
  { handle: "auth", project: P_SVC, type: "feature", title: "Refactor auth middleware to a token store", lane: "needs_you", priority: 3 },
  { handle: "css", project: P_WEB, type: "chore", title: "Migrate hard-coded CSS to design tokens", lane: "needs_you", priority: 2 },
  { handle: "n1", project: P_SVC, type: "bug", title: "Fix N+1 query in the invoice list endpoint", lane: "verifying", priority: 2 },
  { handle: "kbd", project: P_WEB, type: "feature", title: "Keyboard navigation for the command palette", lane: "verifying", priority: 1 },
  { handle: "health", project: P_SVC, type: "chore", title: "Add /health and /ready probe endpoints", lane: "pr", priority: 1 },
  { handle: "retry", project: P_SVC, type: "feature", title: "Add webhook retry with exponential backoff", lane: "performing", priority: 1 },
  { handle: "settings", project: P_WEB, type: "feature", title: "Port the settings page to server components", lane: "performing", priority: 1 },
  { handle: "bench", project: P_CLI, type: "investigation", title: "Benchmark alacritty vs vt100 parser throughput", lane: "performing" },
];

const cards: Card[] = cardSeeds.map((s) => ({
  id: id("c"),
  project_id: s.project,
  type: s.type,
  title: s.title,
  lane: s.lane,
  priority: s.priority ?? 0,
}));

const cardByHandle = (handle: string): Card => {
  const seedIdx = cardSeeds.findIndex((s) => s.handle === handle);
  const c = cards[seedIdx];
  if (!c) throw new Error(`fixture card not found: ${handle}`);
  return c;
};

// --- Sessions (fleet strip) -------------------------------------------------

interface SessionSeed {
  card: string;
  harness: Session["harness"];
  agent?: string;
  state: Session["state"];
  stage: string;
  note: string;
  sinceMin: number;
  model?: string;
  firstPrompt: string;
}

const sessionSeeds: SessionSeed[] = [
  { card: "auth", harness: "codex", state: "needs_input", stage: "implement", note: "which token store: redis or in-process LRU?", sinceMin: 14, model: "gpt-5.4-codex", firstPrompt: "Refactor auth middleware to a pluggable token store" },
  { card: "css", harness: "claude", agent: "cc-alt", state: "blocked", stage: "implement", note: "tokens.css referenced in brief does not exist", sinceMin: 41, model: "claude-sonnet-4-5", firstPrompt: "Migrate hard-coded CSS values to design tokens" },
  { card: "kbd", harness: "claude", state: "awaiting_feedback", stage: "plan", note: "plan round 2 posted; awaiting your review", sinceMin: 18, model: "claude-opus-4-8", firstPrompt: "Add keyboard navigation to the command palette" },
  { card: "n1", harness: "codex", state: "working", stage: "verify", note: "running check: go test ./... -run Invoice", sinceMin: 1, model: "gpt-5.4-codex", firstPrompt: "Fix the N+1 query in the invoice list endpoint" },
  { card: "retry", harness: "codex", state: "working", stage: "implement", note: "wiring exponential backoff with jitter", sinceMin: 6, model: "gpt-5.4-codex", firstPrompt: "Add retry with backoff to the webhook dispatcher" },
  { card: "settings", harness: "claude", agent: "cc-alt", state: "working", stage: "implement", note: "migrating useEffect data loads to server fetches", sinceMin: 22, model: "claude-opus-4-8", firstPrompt: "Convert settings/* to React server components" },
  { card: "bench", harness: "opencode", state: "working", stage: "explore", note: "collecting flamegraphs across 3 corpora", sinceMin: 3, firstPrompt: "Compare alacritty and vt100 parse throughput" },
];

const sessions: Session[] = sessionSeeds.map((s) => {
  const card = cardByHandle(s.card);
  return {
    id: id("s"),
    card_id: card.id,
    project_id: card.project_id,
    project_name: projName(card.project_id),
    harness: s.harness,
    agent: s.agent ?? s.harness,
    title: null,
    model: s.model ?? null,
    state: s.state,
    first_prompt: s.firstPrompt,
    state_since: ago(s.sinceMin),
    stage: s.stage,
    status_note: s.note,
  };
});

const sessionForCard = (cardHandle: string): Session => {
  const card = cardByHandle(cardHandle);
  const s = sessions.find((x) => x.card_id === card.id);
  if (!s) throw new Error(`fixture session not found for card: ${cardHandle}`);
  return s;
};

// --- Needs You queue --------------------------------------------------------

interface NeedsYouSeed {
  card: string;
  kind: string;
  score: number;
  raisedMin: number;
  note: string;
}

const needsYouSeeds: NeedsYouSeed[] = [
  { card: "auth", kind: "agent_needs_input", score: 88, raisedMin: 14, note: "which token store: redis or in-process LRU?" },
  { card: "css", kind: "agent_blocked", score: 71, raisedMin: 40, note: "tokens.css referenced in brief does not exist" },
  { card: "n1", kind: "gate_finding", score: 66, raisedMin: 2, note: "review flagged a missing index on invoice.card_id" },
  { card: "kbd", kind: "plan_round", score: 58, raisedMin: 18, note: "plan round 2 awaiting your annotations" },
  { card: "health", kind: "pr_ready", score: 44, raisedMin: 3, note: "PR #318 green; ready to merge" },
];

const buildNeedsYou = (): NeedsYouItem[] =>
  needsYouSeeds
    .map((s) => {
      const c = cardByHandle(s.card);
      return {
        id: id("ny"),
        card_id: c.id,
        kind: s.kind,
        dedupe_key: `${s.kind}:${c.id}`,
        score: s.score,
        raised_at: ago(s.raisedMin),
        note: s.note,
      } satisfies NeedsYouItem;
    })
    .sort((a, b) => b.score - a.score);

// --- Styled terminal captures (session.attach snapshot twins) ----------------
// Built fresh per poll so the timestamp line advances, making poll-refresh visible.

const FG = { green: "#7BD0A8", brass: "#E6A23C", brightBrass: "#F5BC5E", red: "#E5686A", blue: "#6C9CE6", dim: "#99a1ae", violet: "#C98BDB", cyan: "#5FBFC0" };

const R = (text: string, style: Partial<StyledRun> = {}): StyledRun => ({ text, ...style });
const clock = (t: number) => new Date(t).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", second: "2-digit" });

type SnapshotBuilder = (nowMs: number) => StyledRun[][];

const snapshotBuilders: Record<string, SnapshotBuilder> = {
  // auth middleware - codex, needs_input
  [sessionForCard("auth").id]: (t) => [
    [R("codex ", { fg: FG.dim }), R("· ledger-svc · implement", { fg: FG.dim })],
    [],
    [R("  Refactored ", { fg: FG.green }), R("authMiddleware"), R(" to accept a ", { fg: FG.dim }), R("TokenStore"), R(" trait.")],
    [R("  Wired the JWT path and added round-trip tests (12 passing).", { fg: FG.dim })],
    [],
    [R("  Two viable backends for the store:", {})],
    [R("    1. redis      ", { fg: FG.cyan }), R("- shared across instances, one more dependency")],
    [R("    2. in-proc LRU ", { fg: FG.cyan }), R("- zero deps, per-instance, fine for a single node")],
    [],
    [R("? ", { fg: FG.brightBrass, bold: true }), R("Which token store should I wire as the default?", { bold: true })],
    [R("  ", {}), R("waiting for your answer", { fg: FG.brass }), R(" · ", { fg: FG.dim }), R(clock(t), { fg: FG.dim })],
    [R("> ", { fg: FG.green }), R("█", { fg: FG.brass })],
  ],
  // css migration - claude, blocked
  [sessionForCard("css").id]: (t) => [
    [R("cc-alt ", { fg: FG.dim }), R("· dappertoast-web · implement", { fg: FG.dim })],
    [],
    [R("  Scanning components for hard-coded color literals...", { fg: FG.dim })],
    [R("  Found 47 literals across 18 files.", { fg: FG.dim })],
    [],
    [R("  import ", { fg: FG.violet }), R("tokens ", {}), R("from ", { fg: FG.violet }), R("'../styles/tokens.css'", { fg: FG.green })],
    [R("  ", {}), R("ENOENT", { fg: FG.red, bold: true }), R(": styles/tokens.css does not exist", { fg: FG.red })],
    [],
    [R("✗ ", { fg: FG.red, bold: true }), R("Blocked", { fg: FG.red, bold: true }), R(": the brief names a tokens file that was never created.", {})],
    [R("  I can generate it from the found literals, but that is a design call.", { fg: FG.dim })],
    [R("  ", {}), R("blocked " + relMin(t, sessionForCard("css").state_since), { fg: FG.red }), R(" · needs you", { fg: FG.dim })],
    [R("> ", { fg: FG.green }), R("█", { fg: FG.brass })],
  ],
  // webhook retry - codex, working
  [sessionForCard("retry").id]: (t) => [
    [R("codex ", { fg: FG.dim }), R("· ledger-svc · implement", { fg: FG.dim })],
    [],
    [R("  RetryPolicy", { fg: FG.cyan }), R(" { base: 200ms, factor: 2, maxRetries: 6, jitter: full }")],
    [R("  ✓ scaffolded dispatcher wrapper", { fg: FG.green })],
    [R("  ✓ exponential schedule: 200ms 400ms 800ms 1.6s 3.2s 6.4s", { fg: FG.green })],
    [R("  • adding full-jitter and a Retry-After honor path...", { fg: FG.brass })],
    [],
    [R("  running ", { fg: FG.dim }), R("go build ./...", {}), R("  " + spinner(t), { fg: FG.brass })],
    [R("  " + clock(t), { fg: FG.dim })],
    [R("> ", { fg: FG.green }), R("█", { fg: FG.brass })],
  ],
};

function relMin(nowMs: number, sinceMs: number): string {
  const m = Math.max(1, Math.round((nowMs - sinceMs) / MIN));
  return `${m}m`;
}
function spinner(t: number): string {
  return ["|", "/", "-", "\\"][Math.floor(t / 250) % 4];
}

// A generic capture for any other live session, so every fleet row peeks.
function genericSnapshot(session: Session, t: number): StyledRun[][] {
  return [
    [R(`${session.agent ?? session.harness} `, { fg: FG.dim }), R(`· ${session.project_name ?? "no project"} · ${session.stage ?? "session"}`, { fg: FG.dim })],
    [],
    [R("  " + (session.status_note ?? "working"), {})],
    [R("  " + clock(t), { fg: FG.dim }), R("  " + spinner(t), { fg: FG.brass })],
    [R("> ", { fg: FG.green }), R("█", { fg: FG.brass })],
  ];
}

// --- Plan artifacts ---------------------------------------------------------

const plans: Record<string, PlanArtifact> = {
  [cardByHandle("kbd").id]: {
    id: id("art"),
    card_id: cardByHandle("kbd").id,
    card_title: cardByHandle("kbd").title,
    project_name: projName(cardByHandle("kbd").project_id),
    round: 2,
    status: "awaiting_review",
    summary:
      "Add full keyboard navigation to the command palette: arrow keys to move, Enter to run, Escape to close, with a visible focus ring and a roving tabindex so it stays accessible.",
    layoutWarnings: [],
    sections: [
      {
        heading: "Approach",
        body: [
          "Introduce a `useRovingFocus` hook that owns the active index and maps ArrowUp / ArrowDown / Home / End onto the filtered result list.",
          "The palette input keeps DOM focus; results are aria-activedescendant targets, so screen readers announce the highlighted command without moving focus off the input.",
        ],
      },
      {
        heading: "Keys",
        body: [
          "ArrowUp / ArrowDown - move the highlight, wrapping at the ends.",
          "Enter - run the highlighted command; Ctrl+Enter - run without closing.",
          "Escape - close the palette and restore focus to the trigger.",
        ],
      },
      {
        heading: "Round 2 changes",
        body: [
          "Addressed your round-1 note: highlight now wraps instead of stopping at the last row.",
          "Added a reduced-motion path so the scroll-into-view is instant when the user prefers reduced motion.",
        ],
      },
      {
        heading: "Open question",
        body: ["Should Tab move between result groups, or stay a no-op so it never leaves the palette? Leaning no-op for v1."],
      },
    ],
  },
};

// ============================================================================

export class FixtureMobileSource implements MobileDataSource {
  readonly mode = "fixture" as const;

  private needsYou = buildNeedsYou();
  private emptyNeedsYou = false;
  private handlers = new Set<() => void>();
  private automationTimer?: ReturnType<typeof setTimeout>;
  private planState = new Map<string, PlanArtifact>(Object.entries(plans).map(([k, v]) => [k, { ...v }]));

  async loadFleet(): Promise<FleetSnapshot> {
    await delay(140);
    return {
      projects: projects.map((p) => ({ ...p })),
      cards: cards.map((c) => ({ ...c })),
      sessions: sessions.map((s) => ({ ...s })),
      needsYou: this.emptyNeedsYou ? [] : this.needsYou.map((n) => ({ ...n })),
    };
  }

  async peekSession(sessionId: string): Promise<TerminalPeek> {
    await delay(120);
    const t = Date.now();
    const session = sessions.find((s) => s.id === sessionId);
    const builder = snapshotBuilders[sessionId];
    const lines = builder ? builder(t) : session ? genericSnapshot(session, t) : [[R("(no session)", { fg: FG.dim })]];
    return { sessionId, cols: 80, rows: lines.length, lines, capturedAt: t, scrubbed: true };
  }

  async getPlan(cardId: string): Promise<PlanArtifact | null> {
    await delay(140);
    const p = this.planState.get(cardId);
    return p ? { ...p, sections: p.sections.map((s) => ({ ...s })) } : null;
  }

  async approvePlan(planId: string, feedback: string): Promise<ActionResult> {
    await delay(260);
    for (const [cardId, plan] of this.planState) {
      if (plan.id === planId) {
        this.planState.set(cardId, { ...plan, status: "approved" });
        // Approving a plan resolves its Needs You item.
        this.needsYou = this.needsYou.filter((n) => !(n.card_id === cardId && n.kind === "plan_round"));
        void feedback;
        this.emit();
        return { ok: true };
      }
    }
    return { ok: false, error: "plan not found" };
  }

  async dismissNeedsYou(itemId: string): Promise<ActionResult> {
    await delay(90);
    this.needsYou = this.needsYou.filter((n) => n.id !== itemId);
    this.emit();
    return { ok: true };
  }

  subscribeEvents(handler: () => void): () => void {
    this.handlers.add(handler);
    if (this.automationTimer === undefined) this.scheduleArrival();
    return () => {
      this.handlers.delete(handler);
    };
  }

  demo = {
    setNeedsYouEmpty: (empty: boolean) => {
      this.emptyNeedsYou = empty;
      this.emit();
    },
    isNeedsYouEmpty: () => this.emptyNeedsYou,
  };

  private emit(): void {
    this.handlers.forEach((h) => h());
  }

  // DEMO: ~15s after a subscriber attaches, a working session flips to needs_input and a
  // new Needs You item arrives, so the live-arrival badge bump is demonstrable offline.
  private scheduleArrival(): void {
    this.automationTimer = setTimeout(() => {
      if (this.emptyNeedsYou) return;
      const bench = sessionForCard("bench");
      bench.state = "needs_input";
      bench.state_since = Date.now();
      bench.status_note = "which corpus should be the canonical benchmark?";
      const card = cardByHandle("bench");
      card.lane = "needs_you";
      if (!this.needsYou.some((n) => n.card_id === card.id)) {
        this.needsYou = [
          {
            id: id("ny"),
            card_id: card.id,
            kind: "agent_needs_input",
            dedupe_key: `agent_needs_input:${card.id}`,
            score: 64,
            raised_at: Date.now(),
            note: bench.status_note,
          },
          ...this.needsYou,
        ].sort((a, b) => b.score - a.score);
      }
      this.emit();
    }, 15_000);
  }
}

function delay(ms: number): Promise<void> {
  return new Promise((r) => setTimeout(r, ms));
}
