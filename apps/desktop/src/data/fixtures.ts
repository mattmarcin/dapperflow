// ============================================================================
// DEV ONLY - fixture data source.
// Serves realistic board data through the same DataSource interface as the live
// protocol client, so every Phase 1 view is fully demonstrable before the
// card.*/project.*/dispatch.*/event.* backend lands. Toggled in data/index.ts.
// None of this ships in a real daemon build.
// ============================================================================

import {
  Agent,
  AgentAddInput,
  AgentMutationResult,
  AgentRemoveResult,
  AgentsDetectResult,
  AgentUpdateInput,
  AuditDepth,
  AuditStartResult,
  BoardSnapshot,
  Card,
  CardCreateInput,
  CardEvent,
  CardEventKind,
  DispatchStartInput,
  FindingResolution,
  GateCheck,
  GateRun,
  GithubAuthStatus,
  GithubImportConfig,
  GithubImportResult,
  GithubIssue,
  Harness,
  Lane,
  NeedsYouItem,
  PairedDevice,
  Project,
  ProjectAddResult,
  Recipe,
  RemoteListenerState,
  Session,
  SessionResumeResult,
  SessionState,
} from "../model";
import { ArtifactMeta, FeedbackSubmit, FeedbackSubmitResult } from "../review/protocol";
import { cautionArgs } from "../lib/agents";
import { generateUlid } from "../ulid";
import { DataSource } from "./source";
import { RECIPE_FIXTURES } from "./recipe-fixtures";
import {
  DEFAULT_IMPORT_CONFIG,
  GITHUB_AUTH,
  GITHUB_ISSUES,
  HEALTH_CHECKS,
  HEALTH_PR,
  KEYNAV_CHECKS,
  NPLUS1_CHECKS,
  NPLUS1_FINDINGS,
  CACHE_PR,
  initialRemoteState,
  mintRemoteToken,
  pairingPayload,
  PHONE_PROFILE,
  REMOTE_LAN_IP,
  REMOTE_PORT,
} from "./m5-fixtures";

const MIN = 60_000;
const now = Date.now();
const ago = (minutes: number) => now - minutes * MIN;

let idCounter = 0;
function id(_prefix: string): string {
  // Deterministic-ish, sortable, ULID-shaped ids for stable fixtures. The prefix
  // is a readability hint at the call site only.
  idCounter += 1;
  return generateUlid(now - (400 - idCounter) * 1000);
}

// --- Projects ---------------------------------------------------------------

const P_WEB = id("p");
const P_CLI = id("p");
const P_SVC = id("p");
const P_LAB = id("p");

const projects: Project[] = [
  {
    id: P_WEB,
    path: "C:\\Users\\m\\code\\dappertoast-web",
    name: "dappertoast-web",
    default_branch: "main",
    mode: "pr",
    default_recipe: "standard",
    created_at: ago(60 * 24 * 40),
    updated_at: ago(12),
  },
  {
    id: P_CLI,
    path: "C:\\Users\\m\\code\\orchard-cli",
    name: "orchard-cli",
    default_branch: "main",
    mode: "pr",
    default_recipe: "deep",
    created_at: ago(60 * 24 * 30),
    updated_at: ago(3),
  },
  {
    id: P_SVC,
    path: "C:\\Users\\m\\code\\ledger-svc",
    name: "ledger-svc",
    default_branch: "main",
    mode: "pr",
    default_recipe: "standard",
    created_at: ago(60 * 24 * 22),
    updated_at: ago(1),
  },
  {
    id: P_LAB,
    path: "C:\\Users\\m\\code\\atlas-notebooks",
    name: "atlas-notebooks",
    default_branch: "main",
    mode: "local_only",
    default_recipe: "presto",
    created_at: ago(60 * 24 * 9),
    updated_at: ago(60 * 5),
  },
];

// --- Cards ------------------------------------------------------------------

interface Seed {
  project: string | null;
  type: Card["type"];
  title: string;
  lane: Lane;
  priority?: number;
  brief?: string;
  recipe?: string;
  origin?: Card["origin_kind"];
  originRef?: string;
  evidence?: string;
}

const seeds: Seed[] = [
  // inbox
  { project: P_WEB, type: "bug", title: "Dark mode flickers on first paint", lane: "inbox", priority: 1 },
  { project: P_CLI, type: "chore", title: "Evaluate Biome to replace ESLint + Prettier", lane: "inbox" },
  { project: null, type: "feature", title: "Draft the Q3 product roadmap", lane: "inbox" },
  {
    project: P_SVC,
    type: "bug",
    title: "Webhook signature check rejects valid Stripe events",
    lane: "inbox",
    priority: 2,
    origin: "github_issue",
    originRef: "dappertoast/ledger-svc#241",
  },
  // inbox - audit findings (origin: audit, each carries file:line evidence)
  {
    project: P_SVC,
    type: "bug",
    title: "Unbounded retry loop has no dead-letter path",
    lane: "inbox",
    priority: 2,
    origin: "audit",
    originRef: "audit/ledger-svc/q1",
    evidence: "src/webhooks/dispatch.ts:88 - while(true) retry with no ceiling",
  },
  {
    project: P_SVC,
    type: "test",
    title: "No test covers the signature-verification failure path",
    lane: "inbox",
    origin: "audit",
    originRef: "audit/ledger-svc/q1",
    evidence: "src/webhooks/verify.ts:34 - only the happy path is exercised",
  },
  {
    project: P_SVC,
    type: "chore",
    title: "Secrets read from process.env with no validation at boot",
    lane: "inbox",
    priority: 1,
    origin: "audit",
    originRef: "audit/ledger-svc/q1",
    evidence: "src/config.ts:12 - STRIPE_KEY used unchecked; crashes late if unset",
  },
  // shaping
  {
    project: P_SVC,
    type: "feature",
    title: "Multi-currency support for invoices",
    lane: "shaping",
    priority: 2,
    brief: "Store amounts in minor units + currency code. Convert at read time using a rates table refreshed daily.",
  },
  { project: P_CLI, type: "test", title: "Flaky test: worktree lease race under load", lane: "shaping", priority: 1 },
  // ready
  {
    project: P_SVC,
    type: "feature",
    title: "Rate-limit the public API by API key",
    lane: "ready",
    priority: 2,
    recipe: "standard",
    brief: "Token bucket per key, 100 req/min default, 429 with Retry-After.",
  },
  {
    project: P_WEB,
    type: "chore",
    title: "Upgrade to React 19 and the new compiler",
    lane: "ready",
    priority: 1,
    recipe: "deep",
    brief: "Adopt the React 19 compiler; audit memo usage; verify no hydration regressions.",
  },
  // performing (live)
  { project: P_SVC, type: "feature", title: "Add webhook retry with exponential backoff", lane: "performing", priority: 1 },
  { project: P_WEB, type: "feature", title: "Port the settings page to server components", lane: "performing", priority: 1 },
  { project: P_CLI, type: "investigation", title: "Benchmark alacritty vs vt100 parser throughput", lane: "performing" },
  // verifying (gate running)
  { project: P_SVC, type: "bug", title: "Fix N+1 query in the invoice list endpoint", lane: "verifying", priority: 2 },
  { project: P_WEB, type: "feature", title: "Keyboard navigation for the command palette", lane: "verifying", priority: 1 },
  // needs you
  {
    project: P_SVC,
    type: "feature",
    title: "Harden webhook ingestion for Stripe events",
    lane: "needs_you",
    priority: 3,
    recipe: "deep",
    brief: "Plan the retry, dead-letter, and idempotency path for the webhook ingester. Review the plan artifact and approve or annotate.",
  },
  {
    project: P_WEB,
    type: "feature",
    title: "Redesign the retry metrics dashboard",
    lane: "needs_you",
    priority: 2,
    recipe: "standard",
    brief: "One-screen plan for the retry-metrics dashboard. The first draft has rendering bugs the layout audit should catch.",
  },
  { project: P_SVC, type: "feature", title: "Refactor auth middleware to a token store", lane: "needs_you", priority: 3 },
  { project: P_WEB, type: "chore", title: "Migrate hard-coded CSS to design tokens", lane: "needs_you", priority: 2 },
  // pr
  { project: P_SVC, type: "chore", title: "Add /health and /ready probe endpoints", lane: "pr", priority: 1 },
  { project: P_CLI, type: "feature", title: "Warm the parse cache on cold start", lane: "pr" },
  // done
  { project: P_WEB, type: "chore", title: "Fix typo in the onboarding README", lane: "done" },
  { project: P_CLI, type: "chore", title: "Add rustfmt and clippy to CI", lane: "done" },
  { project: P_SVC, type: "investigation", title: "Investigate steady memory growth in prod", lane: "done" },
];

const cards: Card[] = seeds.map((s, i) => ({
  id: id("c"),
  project_id: s.project,
  type: s.type,
  title: s.title,
  lane: s.lane,
  dial_recipe: s.recipe ?? null,
  priority: s.priority ?? 0,
  brief: s.brief ?? null,
  origin_kind: s.origin ?? "manual",
  origin_ref: s.originRef ?? null,
  evidence: s.evidence ?? null,
  created_at: ago(60 * 24 - i * 30),
  updated_at: ago(i),
}));

const byTitle = (t: string): Card => {
  const c = cards.find((x) => x.title.startsWith(t));
  if (!c) throw new Error(`fixture card not found: ${t}`);
  return c;
};

// --- Sessions (session strips) ---------------------------------------------

interface SessionSeed {
  cardTitle: string;
  harness: Harness;
  agent?: string; // launcher name; defaults to the harness (adapter family)
  state: SessionState;
  stage: string;
  note: string;
  sinceMin: number;
  model?: string;
  firstPrompt?: string;
  ended?: boolean;
  title?: string;
}

const sessionSeeds: SessionSeed[] = [
  {
    cardTitle: "Add webhook retry",
    harness: "codex",
    state: "working",
    stage: "implement",
    note: "wiring exponential backoff with jitter",
    sinceMin: 6,
    model: "gpt-5.4-codex",
    firstPrompt: "Add retry with backoff to the webhook dispatcher",
  },
  {
    cardTitle: "Port the settings page",
    harness: "claude",
    agent: "cc-alt",
    state: "working",
    stage: "implement",
    note: "migrating useEffect data loads to server fetches",
    sinceMin: 22,
    model: "claude-opus-4-8",
    firstPrompt: "Convert settings/* to React server components",
  },
  {
    cardTitle: "Benchmark alacritty",
    harness: "opencode",
    state: "working",
    stage: "explore",
    note: "collecting flamegraphs across 3 corpora",
    sinceMin: 3,
    firstPrompt: "Compare alacritty_terminal and vt100 parse throughput",
  },
  {
    cardTitle: "Fix N+1 query",
    harness: "codex",
    state: "working",
    stage: "verify",
    note: "running check: go test ./... -run Invoice",
    sinceMin: 1,
    model: "gpt-5.4-codex",
    firstPrompt: "Fix the N+1 query in the invoice list endpoint",
  },
  {
    cardTitle: "Keyboard navigation",
    harness: "claude",
    state: "awaiting_feedback",
    stage: "plan",
    note: "plan round 2 posted; awaiting your review",
    sinceMin: 18,
    model: "claude-opus-4-8",
    firstPrompt: "Add keyboard navigation to the command palette",
  },
  {
    cardTitle: "Harden webhook ingestion",
    harness: "claude",
    state: "awaiting_feedback",
    stage: "plan",
    note: "plan round 3 posted; awaiting your review",
    sinceMin: 9,
    model: "claude-opus-4-8",
    firstPrompt: "Plan the retry and dead-letter path for the webhook ingester",
  },
  {
    cardTitle: "Redesign the retry metrics dashboard",
    harness: "codex",
    state: "awaiting_feedback",
    stage: "plan",
    note: "plan round 1 posted; the render has layout issues",
    sinceMin: 4,
    model: "gpt-5.4-codex",
    firstPrompt: "Draft the retry-metrics dashboard plan",
  },
  {
    cardTitle: "Refactor auth middleware",
    harness: "codex",
    state: "needs_input",
    stage: "implement",
    note: "which token store: redis or in-process LRU?",
    sinceMin: 14,
    model: "gpt-5.4-codex",
    firstPrompt: "Refactor auth middleware to a pluggable token store",
  },
  {
    cardTitle: "Migrate hard-coded CSS",
    harness: "claude",
    state: "blocked",
    stage: "implement",
    note: "blocked: tokens.css referenced in brief does not exist",
    sinceMin: 41,
    model: "claude-sonnet-4-5",
    firstPrompt: "Migrate hard-coded CSS values to design tokens",
  },
  {
    cardTitle: "Add /health and /ready",
    harness: "codex",
    state: "done",
    stage: "pr",
    note: "PR #318 opened, CI green",
    sinceMin: 52,
    ended: true,
  },
  {
    cardTitle: "Warm the parse cache",
    harness: "opencode",
    state: "done",
    stage: "pr",
    note: "PR #77 opened, CI running",
    sinceMin: 90,
    ended: true,
  },
];

const sessions: Session[] = sessionSeeds.map((s) => {
  const card = byTitle(s.cardTitle);
  return {
    id: id("s"),
    card_id: card.id,
    harness: s.harness,
    agent: s.agent ?? s.harness,
    agent_id: null,
    title: s.title ?? null,
    model: s.model ?? null,
    effort: null,
    state: s.state,
    worktree_id: id("w"),
    first_prompt: s.firstPrompt ?? null,
    resume_ref: s.ended ? null : `${s.harness}-${Math.random().toString(36).slice(2, 8)}`,
    resumed_from: null,
    created_at: ago(s.sinceMin + 4),
    ended_at: s.ended ? ago(s.sinceMin - 1) : null,
    state_since: ago(s.sinceMin),
    stage: s.stage,
    status_note: s.note,
  };
});

// A couple of resumable past sessions for the Projects tree (ended / interrupted).
// The interrupted one carries a preserved-scrollback preview so the resume banner and
// lineage-divider demo reads like a real restarted session (fixture-only; live
// sessions replay the real ring through session.attach).
sessions.push(
  {
    id: id("s"),
    card_id: byTitle("Upgrade to React 19").id,
    harness: "claude",
    agent: "cc-alt",
    agent_id: null,
    title: null,
    model: "claude-opus-4-8",
    effort: null,
    state: "interrupted",
    worktree_id: id("w"),
    first_prompt: "Adopt the React 19 compiler and audit memo usage",
    resume_ref: "claude-a91f2c",
    resumed_from: null,
    created_at: ago(95),
    ended_at: ago(38),
    state_since: ago(38),
    stage: "implement",
    status_note: "migrating memo call sites to the compiler",
    scrollback_preview: [
      "$ claude --resume",
      "· Adopting the React 19 compiler across apps/web",
      "· Audited 42 memo call sites; 31 are now compiler-managed",
      "· Updated eslint-plugin-react-compiler config",
      "> Next: verify no hydration regressions on the settings route",
      "",
      "[daemon connection lost - PTY did not survive the restart]",
    ],
  },
  {
    id: id("s"),
    card_id: byTitle("Fix typo").id,
    harness: "claude",
    agent: "claude",
    agent_id: null,
    title: null,
    model: "claude-sonnet-4-5",
    effort: null,
    state: "interrupted",
    worktree_id: id("w"),
    first_prompt: "Fix the typo in onboarding README",
    resume_ref: "claude-33e0b1",
    resumed_from: null,
    created_at: ago(60 * 26),
    ended_at: ago(60 * 25),
    state_since: ago(60 * 25),
    stage: null,
    status_note: null,
    scrollback_preview: [
      "$ claude",
      "· Found the typo: 'reccomended' -> 'recommended' in README.md:14",
      "· Staged the one-line fix",
      "",
      "[daemon restarted - session interrupted before commit]",
    ],
  },
  {
    id: id("s"),
    card_id: byTitle("Investigate steady memory").id,
    harness: "codex",
    agent: "codex",
    agent_id: null,
    title: null,
    model: "gpt-5.4-codex",
    effort: null,
    state: "done",
    worktree_id: null,
    first_prompt: "Profile the service under sustained load",
    resume_ref: null,
    resumed_from: null,
    created_at: ago(60 * 30),
    ended_at: ago(60 * 28),
    state_since: ago(60 * 28),
    stage: null,
    status_note: null,
  },
);

// --- Event timelines --------------------------------------------------------

const events: CardEvent[] = [];
let evClock = 0;
function ev(cardId: string, kind: CardEventKind, payload: Record<string, unknown>, minsAgo: number): void {
  evClock += 1;
  events.push({ id: generateUlid(ago(minsAgo)), card_id: cardId, kind, payload, ts: ago(minsAgo) });
}

function dispatchTimeline(cardTitle: string, harness: string, startMin: number): void {
  const c = byTitle(cardTitle);
  ev(c.id, "created", { title: c.title }, startMin + 40);
  ev(c.id, "shaped", { brief_words: 82 }, startMin + 33);
  ev(c.id, "moved", { from: "shaping", to: "ready" }, startMin + 30);
  ev(c.id, "dispatched", { harness, recipe: "standard" }, startMin + 28);
  ev(c.id, "worktree_leased", { slot: 2, path: `.dapperflow/wt/${harness}-2` }, startMin + 27);
  ev(c.id, "env_materialized", { files: 3, vars: 11 }, startMin + 27);
  ev(c.id, "brief_composed", { tokens: 1840 }, startMin + 26);
  ev(c.id, "session_started", { harness }, startMin + 26);
  ev(c.id, "state_changed", { to: "working" }, startMin + 25);
}

dispatchTimeline("Add webhook retry", "codex", 6);
ev(byTitle("Add webhook retry").id, "turn_ended", { note: "scaffolded RetryPolicy + tests", exit: 0 }, 4);
ev(byTitle("Add webhook retry").id, "turn_ended", { note: "wiring exponential backoff with jitter" }, 1);

dispatchTimeline("Port the settings page", "claude", 22);
ev(byTitle("Port the settings page").id, "turn_ended", { note: "moved data loads to server components" }, 12);
ev(byTitle("Port the settings page").id, "turn_ended", { note: "migrating useEffect data loads to server fetches" }, 2);

dispatchTimeline("Refactor auth middleware", "codex", 14);
ev(byTitle("Refactor auth middleware").id, "needs_input", { question: "which token store: redis or in-process LRU?" }, 14);
ev(byTitle("Refactor auth middleware").id, "needs_you_raised", { kind: "agent_needs_input", score: 88 }, 14);

dispatchTimeline("Migrate hard-coded CSS", "claude", 41);
ev(byTitle("Migrate hard-coded CSS").id, "blocked", { reason: "tokens.css referenced in brief does not exist" }, 41);
ev(byTitle("Migrate hard-coded CSS").id, "needs_you_raised", { kind: "agent_blocked", score: 71 }, 40);

dispatchTimeline("Add /health and /ready", "codex", 52);
ev(byTitle("Add /health and /ready").id, "gate_started", {}, 12);
ev(byTitle("Add /health and /ready").id, "gate_step", { step: "checks", status: "passed" }, 11);
ev(byTitle("Add /health and /ready").id, "gate_step", { step: "review", status: "passed" }, 9);
ev(byTitle("Add /health and /ready").id, "gate_passed", {}, 8);
ev(byTitle("Add /health and /ready").id, "pushed", { branch: "feat/health-probes" }, 7);
ev(byTitle("Add /health and /ready").id, "pr_opened", { number: 318, url: "https://github.com/dappertoast/ledger-svc/pull/318" }, 6);
ev(byTitle("Add /health and /ready").id, "ci_status", { state: "success" }, 3);

dispatchTimeline("Fix N+1 query", "codex", 20);
ev(byTitle("Fix N+1 query").id, "turn_ended", { note: "batched the invoice line-item load" }, 6);
ev(byTitle("Fix N+1 query").id, "gate_started", {}, 2);
ev(byTitle("Fix N+1 query").id, "gate_step", { step: "checks", status: "running" }, 1);

// Sort every card's events oldest-first for the timeline.
events.sort((a, b) => a.ts - b.ts);

// --- Needs You queue (Attention Router) ------------------------------------
// The daemon's needs_you_items projection (fleet.status.needs_you), synthesized from
// the attention-state sessions plus a gate finding and a green PR, so Mission
// Control's most important surface is fully demonstrable offline. Ranked by score.

interface NeedsYouSeed {
  cardTitle: string;
  kind: string;
  score: number;
  raisedMin: number;
  note?: string;
}

const needsYouSeeds: NeedsYouSeed[] = [
  { cardTitle: "Harden webhook ingestion", kind: "plan_round", score: 90, raisedMin: 9, note: "plan round 3 awaiting your review and approval" },
  { cardTitle: "Refactor auth middleware", kind: "agent_needs_input", score: 88, raisedMin: 14, note: "which token store: redis or in-process LRU?" },
  { cardTitle: "Migrate hard-coded CSS", kind: "agent_blocked", score: 71, raisedMin: 40, note: "tokens.css referenced in brief does not exist" },
  { cardTitle: "Fix N+1 query", kind: "gate_finding", score: 66, raisedMin: 2, note: "review flagged a missing index on invoice.card_id" },
  { cardTitle: "Redesign the retry metrics dashboard", kind: "plan_round", score: 62, raisedMin: 4, note: "plan round 1 posted; layout issues to resolve" },
  { cardTitle: "Keyboard navigation", kind: "plan_round", score: 58, raisedMin: 18, note: "plan round 2 awaiting your annotations" },
  { cardTitle: "Add /health and /ready", kind: "pr_ready", score: 44, raisedMin: 3, note: "PR #318 green; ready to merge" },
];

const needsYou: NeedsYouItem[] = needsYouSeeds
  .map((s) => {
    const c = byTitle(s.cardTitle);
    return {
      id: generateUlid(ago(s.raisedMin)),
      card_id: c.id,
      kind: s.kind,
      dedupe_key: `${s.kind}:${c.id}`,
      score: s.score,
      raised_at: ago(s.raisedMin),
      notified_at: null,
      note: s.note ?? null,
    };
  })
  .sort((a, b) => b.score - a.score);

// --- Plan artifacts (artifact.*) -------------------------------------------
// Two demo plan artifacts wired to needs_you cards: the primary review loop
// (plan-good, which the agent revises to plan-good-revised on the first round so
// the four re-anchor outcomes are demonstrable) and the layout-audit gate demo
// (plan-broken). doc_id is the serving identity the dev artifact endpoint knows.

interface ArtifactSeed {
  cardTitle: string;
  docId: string;
  revisedDocId?: string;
  round: number;
}

const artifactSeeds: ArtifactSeed[] = [
  { cardTitle: "Harden webhook ingestion", docId: "plan-good", revisedDocId: "plan-good-revised", round: 3 },
  { cardTitle: "Redesign the retry metrics dashboard", docId: "plan-broken", round: 1 },
];

const artifacts: ArtifactMeta[] = artifactSeeds.map((s) => {
  const c = byTitle(s.cardTitle);
  return {
    // STABLE id (not a fresh ulid): the Plan tab keys its persisted queue draft by
    // artifact id, and a fixture id that changes per reload would break the
    // "feedback is never lost" demo. Real daemon artifact ids are stable rows.
    id: `fx-artifact-${s.docId}`,
    card_id: c.id,
    kind: "plan",
    title: c.title,
    doc_id: s.docId,
    revised_doc_id: s.revisedDocId ?? null,
    round: s.round,
    status: "awaiting_feedback",
    created_at: ago(30),
    updated_at: ago(4),
  };
});

// --- Gate runs (gate.md) ----------------------------------------------------
// Seeded onto the cards that a gate touches: the Verifying-lane cards (one escalating
// intent findings into the review chrome, one still under adversarial review) and the
// PR-lane cards (one green + mergeable, one still waiting on CI). doc_id "gate-findings"
// is the review artifact the dev artifact endpoint serves (registered in the plugin).

interface GateSeed {
  cardTitle: string;
  status: GateRun["status"];
  reviewer: string;
  checks: GateCheck[];
  findings: GateRun["findings"];
  findingsDocId?: string;
  pr: GateRun["pr"];
  startedMin: number;
}

const noPr = (branch: string): GateRun["pr"] => ({
  status: "none",
  branch,
  ci: [],
  mergeable: false,
  merge_method: "squash",
  fixes_issue: null,
});

const gateSeeds: GateSeed[] = [
  {
    cardTitle: "Fix N+1 query",
    status: "awaiting_human",
    reviewer: "claude",
    checks: NPLUS1_CHECKS,
    findings: NPLUS1_FINDINGS,
    findingsDocId: "gate-findings",
    pr: noPr("fix/invoice-nplus1"),
    startedMin: 3,
  },
  {
    cardTitle: "Keyboard navigation",
    status: "review",
    reviewer: "codex",
    checks: KEYNAV_CHECKS,
    findings: [],
    pr: noPr("feat/palette-keynav"),
    startedMin: 2,
  },
  {
    cardTitle: "Add /health and /ready",
    status: "passed",
    reviewer: "claude",
    checks: HEALTH_CHECKS,
    findings: [],
    pr: HEALTH_PR,
    startedMin: 12,
  },
  {
    cardTitle: "Warm the parse cache",
    status: "passed",
    reviewer: "opencode",
    checks: [
      { name: "build", cmd: "cargo build", status: "passed", exit_code: 0, duration_ms: 15200, output: "Compiling orchard-cli\n Finished dev [unoptimized] target(s)" },
      { name: "test", cmd: "cargo test", status: "passed", exit_code: 0, duration_ms: 22400, output: "test result: ok. 84 passed; 0 failed" },
    ],
    findings: [],
    pr: CACHE_PR,
    startedMin: 90,
  },
];

const gateRuns: GateRun[] = gateSeeds.map((s) => {
  const c = byTitle(s.cardTitle);
  return {
    id: `fx-gate-${s.findingsDocId ?? c.id}`,
    card_id: c.id,
    status: s.status,
    mode: "full",
    worktree_id: id("w"),
    reviewer_harness: s.reviewer,
    started_at: ago(s.startedMin),
    updated_at: ago(Math.max(0, s.startedMin - 2)),
    checks: s.checks.map((k) => ({ ...k })),
    findings: s.findings.map((f) => ({ ...f })),
    findings_doc_id: s.findingsDocId ?? null,
    pr: { ...s.pr, ci: s.pr.ci.map((x) => ({ ...x })) },
  };
});

// Audit finding catalogs per depth, for the onboarding-audit demo (startAudit).
// Budgets are capped for the demo; the real recipe budgets are larger (product.md).
const AUDIT_FINDINGS: Record<AuditDepth, { type: Card["type"]; title: string; evidence: string }[]> = {
  quick: [
    { type: "bug", title: "Outbound HTTP calls have no timeout", evidence: "src/client.ts:41 - fetch() with no AbortController" },
    { type: "test", title: "The parser error branch is never tested", evidence: "src/parse.ts:120 - catch block has no coverage" },
    { type: "chore", title: "Release path has an unresolved TODO", evidence: "src/release.ts:8 - // TODO: verify signature before publish" },
  ],
  deep: [
    { type: "bug", title: "Outbound HTTP calls have no timeout", evidence: "src/client.ts:41 - fetch() with no AbortController" },
    { type: "test", title: "The parser error branch is never tested", evidence: "src/parse.ts:120 - catch block has no coverage" },
    { type: "chore", title: "Release path has an unresolved TODO", evidence: "src/release.ts:8 - // TODO: verify signature before publish" },
    { type: "bug", title: "Config read from env with no validation", evidence: "src/config.ts:12 - required vars used unchecked" },
    { type: "investigation", title: "Two modules duplicate the retry logic", evidence: "src/a.ts:60 vs src/b.ts:44 - drift risk" },
  ],
};

// --- Configured launchers (Settings > Agents) -------------------------------
// DEV fixtures mirror this machine's real detected CLIs (phase15-core evidence:
// claude 2.1.200, codex 0.142.5, opencode 1.17.13, cursor 3.9.16) plus the
// canonical cc-alt custom launcher and a caution example (yolo).

const KNOWN_ADAPTERS = ["claude", "codex", "opencode", "cursor", "pi", "custom"];

const agents: Agent[] = [
  {
    id: id("a"),
    name: "claude",
    adapter: "claude",
    command: "C:\\Users\\m\\.local\\bin\\claude.EXE",
    extra_args: [],
    extra_env: {},
    source: "detected",
    detected_version: "2.1.200 (Claude Code)",
    enabled: true,
    caution: false,
  },
  {
    id: id("a"),
    name: "codex",
    adapter: "codex",
    command: "codex.CMD",
    extra_args: [],
    extra_env: {},
    source: "detected",
    detected_version: "codex-cli 0.142.5",
    enabled: true,
    caution: false,
  },
  {
    id: id("a"),
    name: "opencode",
    adapter: "opencode",
    command: "opencode.CMD",
    extra_args: [],
    extra_env: {},
    source: "detected",
    detected_version: "1.17.13",
    enabled: true,
    caution: false,
  },
  {
    id: id("a"),
    name: "cursor",
    adapter: "cursor",
    command: "cursor.CMD",
    extra_args: [],
    extra_env: {},
    source: "detected",
    detected_version: "3.9.16",
    enabled: true,
    caution: false,
  },
  {
    id: id("a"),
    name: "cc-alt",
    adapter: "claude",
    command: "claude",
    extra_args: [],
    extra_env: { CLAUDE_CONFIG_DIR: "C:\\Users\\m\\.claude-alt" },
    source: "custom",
    detected_version: null,
    enabled: true,
    caution: false,
  },
  {
    id: id("a"),
    name: "yolo",
    adapter: "claude",
    command: "claude",
    extra_args: ["--dangerously-skip-permissions"],
    extra_env: {},
    source: "custom",
    detected_version: null,
    enabled: false,
    caution: true,
  },
];

// ============================================================================

export class FixtureDataSource implements DataSource {
  readonly mode = "fixture" as const;

  private projects = projects.map((p) => ({ ...p }));
  private cards = cards.map((c) => ({ ...c }));
  private sessions = sessions.map((s) => ({ ...s }));
  private events = events.map((e) => ({ ...e }));
  private agents = agents.map((a) => ({ ...a }));
  private needsYou = needsYou.map((n) => ({ ...n }));
  private artifacts = artifacts.map((a) => ({ ...a }));
  private recipes = RECIPE_FIXTURES.map((r) => ({ ...r }));
  private githubIssues = GITHUB_ISSUES.map((i) => ({ ...i }));
  private importConfigs = new Map<string, GithubImportConfig>();
  private gateRuns = gateRuns.map((g) => ({ ...g }));
  private remote: RemoteListenerState = initialRemoteState();
  private auditCounter = 0;
  private handlers = new Set<(e: CardEvent) => void>();
  private automationTimer?: number;

  async loadSnapshot(): Promise<BoardSnapshot> {
    await delay(160); // feel of a real fetch
    return {
      projects: this.projects.map((p) => ({ ...p })),
      cards: this.cards.map((c) => ({ ...c })),
      sessions: this.sessions.map((s) => ({ ...s })),
      needsYou: this.needsYou.map((n) => ({ ...n })),
    };
  }

  async resumeSession(sessionId: string): Promise<SessionResumeResult> {
    await delay(420);
    const predecessor = this.sessions.find((s) => s.id === sessionId);
    if (!predecessor) return { ok: false, error: "session not found" };
    // Simulate harness-native resume: a fresh session row in the same worktree, linked
    // via resumed_from, with a re-captured resume_ref (architecture.md, session resume).
    const newId = id("s");
    const resumed: Session = {
      ...predecessor,
      id: newId,
      state: "working",
      resumed_from: predecessor.id,
      resume_ref: `${predecessor.harness}-${Math.random().toString(36).slice(2, 8)}`,
      created_at: Date.now(),
      ended_at: null,
      state_since: Date.now(),
      status_note: "resumed with full context",
      scrollback_preview: null,
    };
    predecessor.state = "done"; // the predecessor row retires; lineage is the chain
    this.sessions.push(resumed);
    return { ok: true, new_session_id: newId };
  }

  async addProject(path: string): Promise<ProjectAddResult> {
    await delay(320);
    const trimmed = path.trim();
    if (!trimmed) return { ok: false, error: "Enter a folder path." };
    if (this.projects.some((p) => p.path.toLowerCase() === trimmed.toLowerCase())) {
      return { ok: false, error: "That project is already registered." };
    }
    // Fixture validation: reject anything that is obviously not a repo path.
    if (!/[\\/]/.test(trimmed)) {
      return { ok: false, error: "That does not look like a folder path." };
    }
    const name = trimmed.replace(/[\\/]+$/, "").split(/[\\/]/).pop() || trimmed;
    const project: Project = {
      id: id("p"),
      path: trimmed,
      name,
      default_branch: "main",
      mode: "pr",
      default_recipe: "standard",
      created_at: Date.now(),
      updated_at: Date.now(),
    };
    this.projects.push(project);
    return { ok: true, project: { ...project } };
  }

  async createCard(input: CardCreateInput): Promise<Card> {
    await delay(180);
    const card: Card = {
      id: id("c"),
      project_id: input.project_id ?? null,
      type: input.type,
      title: input.title,
      lane: "inbox",
      dial_recipe: input.dial_recipe ?? null,
      priority: 0,
      brief: input.brief ?? null,
      origin_kind: "manual",
      origin_ref: null,
      created_at: Date.now(),
      updated_at: Date.now(),
    };
    this.cards.push(card);
    this.emit({
      id: generateUlid(),
      card_id: card.id,
      kind: "created",
      payload: { title: card.title },
      ts: Date.now(),
    });
    return { ...card };
  }

  async moveCard(cardId: string, lane: Lane): Promise<Card> {
    await delay(90);
    const card = this.cards.find((c) => c.id === cardId);
    if (!card) throw new Error("card not found");
    const from = card.lane;
    card.lane = lane;
    card.updated_at = Date.now();
    this.emit({
      id: generateUlid(),
      card_id: cardId,
      kind: "moved",
      payload: { from, to: lane },
      ts: Date.now(),
    });
    return { ...card };
  }

  async updateCardDial(cardId: string, recipe: string | null): Promise<Card> {
    await delay(90);
    const card = this.cards.find((c) => c.id === cardId);
    if (!card) throw new Error("card not found");
    const from = card.dial_recipe;
    card.dial_recipe = recipe;
    card.updated_at = Date.now();
    this.emit({
      id: generateUlid(),
      card_id: cardId,
      kind: "dial_changed",
      payload: { from, to: recipe },
      ts: Date.now(),
    });
    return { ...card };
  }

  async dispatch(input: DispatchStartInput): Promise<{ session_id?: string }> {
    await delay(240);
    const card = this.cards.find((c) => c.id === input.card_id);
    if (!card) throw new Error("card not found");
    card.lane = "performing";
    card.updated_at = Date.now();
    const session: Session = {
      id: id("s"),
      card_id: card.id,
      harness: input.harness ?? "claude",
      title: null,
      model: input.model ?? null,
      effort: input.effort ?? null,
      state: "starting",
      worktree_id: id("w"),
      first_prompt: card.brief ?? card.title,
      resume_ref: null,
      created_at: Date.now(),
      ended_at: null,
      state_since: Date.now(),
      stage: "dispatch",
      status_note: "leasing worktree and composing brief",
    };
    this.sessions.push(session);
    this.emit({ id: generateUlid(), card_id: card.id, kind: "dispatched", payload: { harness: session.harness }, ts: Date.now() });
    this.emit({ id: generateUlid(), card_id: card.id, kind: "moved", payload: { from: "ready", to: "performing" }, ts: Date.now() });
    return { session_id: session.id };
  }

  async cancelDispatch(cardId: string): Promise<void> {
    await delay(120);
    const card = this.cards.find((c) => c.id === cardId);
    if (card) card.lane = "shaping";
  }

  async cardEvents(cardId: string): Promise<CardEvent[]> {
    await delay(120);
    return this.events.filter((e) => e.card_id === cardId).map((e) => ({ ...e }));
  }

  async recentActivity(limit = 60): Promise<CardEvent[]> {
    await delay(120);
    // Newest first, across every card - the cross-project feed for Mission Control.
    return this.events
      .slice()
      .sort((a, b) => b.ts - a.ts)
      .slice(0, limit)
      .map((e) => ({ ...e }));
  }

  async renameSession(sessionId: string, title: string): Promise<void> {
    await delay(60);
    const s = this.sessions.find((x) => x.id === sessionId);
    if (s) s.title = title;
  }

  // --- Flow recipes (recipe.list) -------------------------------------------

  async listRecipes(): Promise<Recipe[]> {
    await delay(90);
    return this.recipes.map((r) => ({ ...r }));
  }

  // --- Plan Studio artifact review (artifact.*) -----------------------------

  async getPlanArtifact(cardId: string): Promise<ArtifactMeta | null> {
    await delay(110);
    const a = this.artifacts.filter((x) => x.card_id === cardId).sort((x, y) => y.updated_at - x.updated_at)[0];
    return a ? { ...a } : null;
  }

  async signArtifactUrl(docId: string): Promise<string> {
    // Mirror artifact.get returning a capability URL: ask the dev artifact endpoint
    // (the daemon's stand-in) to mint a short-lived signed URL for this doc.
    const res = await fetch(`/__artifact/sign?id=${encodeURIComponent(docId)}`);
    if (!res.ok) throw new Error(`artifact sign endpoint ${res.status}`);
    const { url } = (await res.json()) as { url: string };
    return url;
  }

  async submitFeedback(input: FeedbackSubmit): Promise<FeedbackSubmitResult> {
    await delay(220);
    const a = this.artifacts.find((x) => x.id === input.artifact_id);
    const nextRound = input.round + 1;
    let revised: string | null = null;
    if (a) {
      a.round = nextRound;
      a.updated_at = Date.now();
      // The agent revises in place. A doc with a revised twin swaps to it on the
      // first round, so the review chrome reloads and re-anchors (the four outcomes).
      if (a.revised_doc_id && a.doc_id !== a.revised_doc_id) {
        revised = a.revised_doc_id;
        a.doc_id = a.revised_doc_id;
      }
      this.emit({
        id: generateUlid(),
        card_id: a.card_id,
        kind: "feedback_sent",
        payload: { round: input.round, items: input.items.length },
        ts: Date.now(),
      });
      // Sending the batch resolves this round's plan_round attention item (the ball
      // is back in the agent's court)...
      const dedupe = `plan_round:${a.card_id}`;
      this.needsYou = this.needsYou.filter((n) => n.dedupe_key !== dedupe);
      this.emit({
        id: generateUlid(),
        card_id: a.card_id,
        kind: "needs_you_resolved",
        payload: { dedupe_key: dedupe },
        ts: Date.now(),
      });
      // ...and the fixture agent "revises instantly", so the next round's item is
      // raised a beat later (rounds repeat until approval, plan-studio.md).
      const cardId = a.card_id;
      window.setTimeout(() => {
        const stillOpen = this.artifacts.find((x) => x.id === input.artifact_id);
        if (!stillOpen || stillOpen.status === "approved") return;
        const item: NeedsYouItem = {
          id: generateUlid(),
          card_id: cardId,
          kind: "plan_round",
          dedupe_key: dedupe,
          score: 90,
          raised_at: Date.now(),
          notified_at: null,
          note: `plan round ${nextRound} awaiting your review`,
        };
        this.needsYou = [...this.needsYou.filter((n) => n.dedupe_key !== dedupe), item];
        this.emit({
          id: generateUlid(),
          card_id: cardId,
          kind: "plan_round",
          payload: { round: nextRound },
          ts: Date.now(),
        });
        this.emit({
          id: generateUlid(),
          card_id: cardId,
          kind: "needs_you_raised",
          payload: { kind: "plan_round", dedupe_key: dedupe, score: 90, note: item.note },
          ts: Date.now(),
        });
      }, 1600);
    }
    return {
      ok: true,
      round: nextRound,
      revised_doc_id: revised,
      next_step: "revise the artifact in place, then poll again",
    };
  }

  async approvePlan(artifactId: string, cardId: string): Promise<void> {
    await delay(160);
    const a = this.artifacts.find((x) => x.id === artifactId);
    if (a) a.status = "approved";
    const card = this.cards.find((c) => c.id === cardId);
    // The plan is approved: record it, clear the plan-round attention, and let the
    // agent proceed to implement (the card advances to Performing).
    this.emit({ id: generateUlid(), card_id: cardId, kind: "plan_approved", payload: { artifact_id: artifactId }, ts: Date.now() });
    this.needsYou = this.needsYou.filter((n) => n.dedupe_key !== `plan_round:${cardId}`);
    this.emit({
      id: generateUlid(),
      card_id: cardId,
      kind: "needs_you_resolved",
      payload: { dedupe_key: `plan_round:${cardId}` },
      ts: Date.now(),
    });
    const session = this.sessions.find((s) => s.card_id === cardId && s.state === "awaiting_feedback");
    if (session) {
      session.state = "working";
      session.stage = "implement";
      session.status_note = "plan approved; starting implementation";
      session.state_since = Date.now();
      this.emit({ id: generateUlid(), card_id: cardId, kind: "state_changed", payload: { to: "working" }, ts: Date.now() });
    }
    if (card && card.lane === "needs_you") {
      card.lane = "performing";
      card.updated_at = Date.now();
      this.emit({ id: generateUlid(), card_id: cardId, kind: "moved", payload: { from: "needs_you", to: "performing" }, ts: Date.now() });
    }
  }

  // --- Onboarding audit (product.md / Card sources: onboarding audit) --------

  async startAudit(projectId: string, depth: AuditDepth): Promise<AuditStartResult> {
    await delay(260);
    const project = this.projects.find((p) => p.id === projectId);
    if (!project) return { ok: false, error: "project not found" };
    const sessionId = id("s");
    const findings = AUDIT_FINDINGS[depth];
    this.auditCounter += 1;
    const fingerprint = `audit/${project.name}/${depth}-${this.auditCounter}`;
    const filed: Card[] = [];
    // Every finding lands in Inbox only, carries file:line evidence, and dedupes on a
    // content fingerprint (product.md). The audit can file but never advance its cards.
    for (const f of findings) {
      const card: Card = {
        id: id("c"),
        project_id: projectId,
        type: f.type,
        title: f.title,
        lane: "inbox",
        dial_recipe: null,
        priority: 0,
        brief: null,
        origin_kind: "audit",
        origin_ref: fingerprint,
        evidence: f.evidence,
        created_at: Date.now(),
        updated_at: Date.now(),
      };
      this.cards.push(card);
      filed.push(card);
      this.emit({ id: generateUlid(), card_id: card.id, kind: "created", payload: { title: card.title, origin: "audit" }, ts: Date.now() });
    }
    // Completion raises exactly ONE audit_digest Needs You item with deep links.
    const digest: NeedsYouItem = {
      id: generateUlid(),
      card_id: filed[0]?.id ?? "",
      kind: "audit_digest",
      dedupe_key: `audit_digest:${fingerprint}`,
      score: 52,
      raised_at: Date.now(),
      notified_at: null,
      note: `${filed.length} findings filed to Inbox for ${project.name}. Triage them.`,
    };
    this.needsYou = [...this.needsYou.filter((n) => n.dedupe_key !== digest.dedupe_key), digest];
    this.emit({
      id: generateUlid(),
      card_id: digest.card_id,
      kind: "needs_you_raised",
      payload: {
        kind: "audit_digest",
        dedupe_key: digest.dedupe_key,
        score: digest.score,
        note: digest.note,
      },
      ts: Date.now(),
    });
    return { ok: true, session_id: sessionId };
  }

  // --- GitHub issue import (github.*) ----------------------------------------

  async githubAuthStatus(): Promise<GithubAuthStatus> {
    await delay(200);
    return { ...GITHUB_AUTH, scopes: [...(GITHUB_AUTH.scopes ?? [])] };
  }

  async getGithubImportConfig(projectId: string): Promise<GithubImportConfig> {
    await delay(80);
    const cfg = this.importConfigs.get(projectId) ?? DEFAULT_IMPORT_CONFIG;
    return { ...cfg, assignees: [...cfg.assignees], labels: [...cfg.labels] };
  }

  async setGithubImportConfig(projectId: string, config: GithubImportConfig): Promise<GithubImportConfig> {
    await delay(140);
    const clean: GithubImportConfig = {
      assignees: [...config.assignees],
      labels: [...config.labels],
      milestone: config.milestone,
      state: config.state,
    };
    this.importConfigs.set(projectId, clean);
    return { ...clean, assignees: [...clean.assignees], labels: [...clean.labels] };
  }

  // Apply the project's import filters to the issue pool (assignee/label/milestone/state),
  // and stamp imported_card_id for dedupe (product.md: one issue, one card).
  private filteredIssues(projectId: string): GithubIssue[] {
    const cfg = this.importConfigs.get(projectId) ?? DEFAULT_IMPORT_CONFIG;
    return this.githubIssues
      .filter((iss) => {
        if (cfg.state === "open" && iss.state !== "open") return false;
        if (cfg.labels.length > 0 && !cfg.labels.some((l) => iss.labels.some((il) => il.name === l))) return false;
        if (cfg.milestone && iss.milestone !== cfg.milestone) return false;
        if (cfg.assignees.length > 0) {
          const wantsMe = cfg.assignees.includes("@me");
          const ok =
            (wantsMe && iss.assignees.includes(GITHUB_AUTH.user ?? "")) ||
            cfg.assignees.some((a) => a !== "@me" && iss.assignees.includes(a));
          if (!ok) return false;
        }
        return true;
      })
      .map((iss) => ({
        ...iss,
        labels: iss.labels.map((l) => ({ ...l })),
        comments: iss.comments.map((c) => ({ ...c })),
        imported_card_id: this.cardForIssue(iss)?.id ?? null,
      }));
  }

  private cardForIssue(iss: GithubIssue): Card | undefined {
    const ref = `${iss.repo}#${iss.number}`;
    return this.cards.find((c) => c.origin_kind === "github_issue" && c.origin_ref === ref);
  }

  async previewGithubIssues(projectId: string): Promise<GithubIssue[]> {
    await delay(420); // feel of a real `gh issue list --json ...`
    if (!GITHUB_AUTH.authenticated) return [];
    return this.filteredIssues(projectId);
  }

  async importGithubIssues(projectId: string, numbers: number[]): Promise<GithubImportResult> {
    await delay(360);
    const wanted = numbers.length > 0
      ? this.githubIssues.filter((i) => numbers.includes(i.number))
      : this.filteredIssues(projectId);
    let imported = 0;
    let refreshed = 0;
    const cardIds: string[] = [];
    for (const iss of wanted) {
      const existing = this.cardForIssue(iss);
      if (existing) {
        // Re-import refreshes fields but respects local lane moves (product.md).
        existing.title = iss.title;
        existing.type = iss.suggested_type;
        existing.updated_at = Date.now();
        refreshed += 1;
        cardIds.push(existing.id);
        continue;
      }
      const card: Card = {
        id: id("c"),
        project_id: projectId,
        type: iss.suggested_type,
        title: iss.title,
        lane: "inbox",
        dial_recipe: null,
        priority: iss.labels.some((l) => /priority:high|priority-high/.test(l.name)) ? 2 : 0,
        brief: null,
        origin_kind: "github_issue",
        origin_ref: `${iss.repo}#${iss.number}`,
        created_at: Date.now(),
        updated_at: Date.now(),
      };
      this.cards.push(card);
      cardIds.push(card.id);
      imported += 1;
      this.emit({
        id: generateUlid(),
        card_id: card.id,
        kind: "created",
        payload: { title: card.title, origin: "github_issue", issue: `${iss.repo}#${iss.number}` },
        ts: Date.now(),
      });
    }
    return { ok: true, imported, refreshed, card_ids: cardIds };
  }

  async getGithubIssueForCard(card: Card): Promise<GithubIssue | null> {
    await delay(220);
    if (card.origin_kind !== "github_issue" || !card.origin_ref) return null;
    const num = Number(card.origin_ref.split("#")[1]);
    const iss = this.githubIssues.find((i) => i.number === num);
    if (!iss) return null;
    return {
      ...iss,
      labels: iss.labels.map((l) => ({ ...l })),
      comments: iss.comments.map((c) => ({ ...c })),
      imported_card_id: card.id,
    };
  }

  // --- Verification gate (gate.md) -------------------------------------------

  private cloneGate(g: GateRun): GateRun {
    return {
      ...g,
      checks: g.checks.map((c) => ({ ...c })),
      findings: g.findings.map((f) => ({ ...f })),
      pr: { ...g.pr, ci: g.pr.ci.map((c) => ({ ...c })) },
    };
  }

  async getGateRun(cardId: string): Promise<GateRun | null> {
    await delay(140);
    const g = this.gateRuns.find((x) => x.card_id === cardId);
    return g ? this.cloneGate(g) : null;
  }

  async startGate(cardId: string): Promise<{ ok: boolean; error?: string }> {
    await delay(320);
    const card = this.cards.find((c) => c.id === cardId);
    if (!card) return { ok: false, error: "card not found" };
    // A fresh gate run: checks pass, review clean, no findings (the happy path). Moves
    // the card to Verifying (product.md: a gate run moves a card to Verifying).
    const existing = this.gateRuns.find((g) => g.card_id === cardId);
    const run: GateRun = existing ?? {
      id: `fx-gate-${cardId}`,
      card_id: cardId,
      status: "passed",
      mode: "full",
      worktree_id: id("w"),
      reviewer_harness: "claude",
      started_at: Date.now(),
      updated_at: Date.now(),
      checks: KEYNAV_CHECKS.map((c) => ({ ...c, status: "passed", output: c.output ?? "(ok)" })),
      findings: [],
      findings_doc_id: null,
      pr: noPr(`verify/${card.title.toLowerCase().replace(/[^a-z0-9]+/g, "-").slice(0, 24)}`),
    };
    if (!existing) this.gateRuns.push(run);
    if (card.lane !== "verifying") {
      const from = card.lane;
      card.lane = "verifying";
      card.updated_at = Date.now();
      this.emit({ id: generateUlid(), card_id: cardId, kind: "moved", payload: { from, to: "verifying" }, ts: Date.now() });
    }
    this.emit({ id: generateUlid(), card_id: cardId, kind: "gate_started", payload: {}, ts: Date.now() });
    return { ok: true };
  }

  async resolveGateFinding(cardId: string, findingId: string, resolution: FindingResolution): Promise<GateRun> {
    await delay(180);
    const run = this.gateRuns.find((g) => g.card_id === cardId);
    if (!run) throw new Error("no gate run for card");
    const finding = run.findings.find((f) => f.id === findingId);
    if (finding) {
      finding.resolution = resolution;
      run.updated_at = Date.now();
      this.emit({
        id: generateUlid(),
        card_id: cardId,
        kind: "finding_resolved",
        payload: { finding: findingId, resolution },
        ts: Date.now(),
      });
    }
    // Once every intent finding is resolved, the gate passes and opens a PR (gate.md
    // ship). Fixes #<n> when the card origin is a GitHub issue (product.md close-loop).
    const unresolved = run.findings.filter((f) => f.klass === "intent" && !f.resolution);
    if (unresolved.length === 0 && run.status === "awaiting_human") {
      run.status = "passed";
      const card = this.cards.find((c) => c.id === cardId);
      const fixes = card?.origin_kind === "github_issue" ? card.origin_ref : null;
      run.pr = {
        status: "open",
        number: 322,
        url: "https://github.com/dappertoast/ledger-svc/pull/322",
        branch: run.pr.branch,
        ci: [
          { name: "build", status: "running" },
          { name: "test", status: "queued" },
        ],
        mergeable: false,
        merge_method: "squash",
        fixes_issue: fixes,
      };
      this.emit({ id: generateUlid(), card_id: cardId, kind: "gate_passed", payload: {}, ts: Date.now() });
      this.emit({ id: generateUlid(), card_id: cardId, kind: "pushed", payload: { branch: run.pr.branch }, ts: Date.now() });
      this.emit({ id: generateUlid(), card_id: cardId, kind: "pr_opened", payload: { number: 322, url: run.pr.url }, ts: Date.now() });
      this.needsYou = this.needsYou.filter((n) => n.dedupe_key !== `gate_finding:${cardId}`);
      this.emit({ id: generateUlid(), card_id: cardId, kind: "needs_you_resolved", payload: { dedupe_key: `gate_finding:${cardId}` }, ts: Date.now() });
      if (card && card.lane === "verifying") {
        card.lane = "pr";
        card.updated_at = Date.now();
        this.emit({ id: generateUlid(), card_id: cardId, kind: "moved", payload: { from: "verifying", to: "pr" }, ts: Date.now() });
      }
      // CI turns green a beat later so the disabled-until-green Merge action unlocks.
      window.setTimeout(() => {
        const g = this.gateRuns.find((x) => x.card_id === cardId);
        if (!g) return;
        g.pr.status = "ci_passed";
        g.pr.ci = [
          { name: "build", status: "success" },
          { name: "test", status: "success" },
        ];
        g.pr.mergeable = true;
        g.updated_at = Date.now();
        this.emit({ id: generateUlid(), card_id: cardId, kind: "ci_status", payload: { state: "success" }, ts: Date.now() });
      }, 2600);
    }
    return this.cloneGate(run);
  }

  async mergePr(cardId: string): Promise<{ ok: boolean; error?: string }> {
    await delay(280);
    const run = this.gateRuns.find((g) => g.card_id === cardId);
    if (!run) return { ok: false, error: "no gate run for card" };
    if (!run.pr.mergeable) return { ok: false, error: "PR is not mergeable yet - CI must be green and findings resolved." };
    run.pr.status = "merged";
    run.updated_at = Date.now();
    const card = this.cards.find((c) => c.id === cardId);
    this.emit({ id: generateUlid(), card_id: cardId, kind: "merged", payload: { number: run.pr.number, squash: true } , ts: Date.now() });
    this.emit({ id: generateUlid(), card_id: cardId, kind: "worktree_returned", payload: { proof: "pr_merged_head_contained" }, ts: Date.now() });
    this.needsYou = this.needsYou.filter((n) => n.dedupe_key !== `pr_ready:${cardId}`);
    this.emit({ id: generateUlid(), card_id: cardId, kind: "needs_you_resolved", payload: { dedupe_key: `pr_ready:${cardId}` }, ts: Date.now() });
    if (card && card.lane !== "done") {
      const from = card.lane;
      card.lane = "done";
      card.updated_at = Date.now();
      this.emit({ id: generateUlid(), card_id: cardId, kind: "moved", payload: { from, to: "done" }, ts: Date.now() });
    }
    return { ok: true };
  }

  // --- Remote access / device pairing (M6; security.md) ----------------------

  async getRemoteState(): Promise<RemoteListenerState> {
    await delay(120);
    return this.cloneRemote();
  }

  private cloneRemote(): RemoteListenerState {
    return {
      ...this.remote,
      profile: { ...this.remote.profile },
      devices: this.remote.devices.map((d) => ({ ...d, capabilities: { ...d.capabilities } })),
    };
  }

  async setRemoteEnabled(enabled: boolean): Promise<RemoteListenerState> {
    await delay(300);
    if (enabled) {
      const token = mintRemoteToken("on");
      const { url, payload } = pairingPayload(REMOTE_LAN_IP, REMOTE_PORT, token);
      this.remote = {
        ...this.remote,
        enabled: true,
        lan_ip: REMOTE_LAN_IP,
        port: REMOTE_PORT,
        url,
        pairing_payload: payload,
        token,
        minted_at: Date.now(),
        profile: { ...PHONE_PROFILE },
      };
    } else {
      this.remote = { ...this.remote, enabled: false, url: null, pairing_payload: null, token: null, minted_at: null };
    }
    return this.cloneRemote();
  }

  async rotateRemoteToken(): Promise<RemoteListenerState> {
    await delay(260);
    // Rotation invalidates the current QR and every paired device (security.md).
    const token = mintRemoteToken("rot");
    const { url, payload } = pairingPayload(REMOTE_LAN_IP, REMOTE_PORT, token);
    this.remote = {
      ...this.remote,
      enabled: true,
      url,
      pairing_payload: payload,
      token,
      minted_at: Date.now(),
      devices: [],
    };
    return this.cloneRemote();
  }

  async revokeRemoteDevice(deviceId: string): Promise<RemoteListenerState> {
    await delay(180);
    this.remote = { ...this.remote, devices: this.remote.devices.filter((d: PairedDevice) => d.id !== deviceId) };
    return this.cloneRemote();
  }

  // --- Configured agents (agents.*) -----------------------------------------

  private withCaution(a: Agent): Agent {
    // Keep the caution flag honest as args are edited (the daemon recomputes it too).
    return { ...a, caution: cautionArgs(a.extra_args).length > 0 };
  }

  async listAgents(): Promise<Agent[]> {
    await delay(120);
    return this.agents.map((a) => this.withCaution(a));
  }

  async detectAgents(): Promise<AgentsDetectResult> {
    await delay(420);
    // The four detected CLIs already exist in the fixture; report them found, none new.
    const found = this.agents
      .filter((a) => a.source === "detected")
      .map((a) => ({ name: a.name, command: a.command, version: a.detected_version ?? null, created: false }));
    return { found, agents: this.agents.map((a) => this.withCaution(a)) };
  }

  async addAgent(input: AgentAddInput): Promise<AgentMutationResult> {
    await delay(220);
    const name = input.name.trim();
    if (!name) return { ok: false, error: "Enter a name for the launcher." };
    if (!input.command.trim()) return { ok: false, error: "Enter the command to launch." };
    if (!KNOWN_ADAPTERS.includes(input.adapter)) return { ok: false, error: `Adapter '${input.adapter}' is not known.` };
    if (this.agents.some((a) => a.name.toLowerCase() === name.toLowerCase())) {
      return { ok: false, error: `An agent named '${name}' already exists.` };
    }
    const agent: Agent = {
      id: id("a"),
      name,
      adapter: input.adapter,
      command: input.command.trim(),
      extra_args: input.extra_args,
      extra_env: input.extra_env,
      source: "custom",
      detected_version: null,
      enabled: true,
      caution: cautionArgs(input.extra_args).length > 0,
    };
    this.agents.push(agent);
    return { ok: true, agent: { ...agent } };
  }

  async updateAgent(input: AgentUpdateInput): Promise<AgentMutationResult> {
    await delay(160);
    const a = this.agents.find((x) => x.id === input.id || x.name === input.id);
    if (!a) return { ok: false, error: "Launcher not found." };
    if (input.name !== undefined) {
      const n = input.name.trim();
      if (!n) return { ok: false, error: "Enter a name for the launcher." };
      if (this.agents.some((x) => x.id !== a.id && x.name.toLowerCase() === n.toLowerCase())) {
        return { ok: false, error: `An agent named '${n}' already exists.` };
      }
      a.name = n;
    }
    if (input.adapter !== undefined) {
      if (!KNOWN_ADAPTERS.includes(input.adapter)) return { ok: false, error: `Adapter '${input.adapter}' is not known.` };
      a.adapter = input.adapter;
    }
    if (input.command !== undefined) {
      if (!input.command.trim()) return { ok: false, error: "Enter the command to launch." };
      a.command = input.command.trim();
    }
    if (input.extra_args !== undefined) a.extra_args = input.extra_args;
    if (input.extra_env !== undefined) a.extra_env = input.extra_env;
    if (input.enabled !== undefined) a.enabled = input.enabled;
    a.caution = cautionArgs(a.extra_args).length > 0;
    return { ok: true, agent: { ...a } };
  }

  async removeAgent(agentId: string): Promise<AgentRemoveResult> {
    await delay(160);
    const idx = this.agents.findIndex((x) => x.id === agentId || x.name === agentId);
    if (idx < 0) return { ok: false, error: "Launcher not found." };
    const [removed] = this.agents.splice(idx, 1);
    return { ok: true, removed: removed.name };
  }

  subscribeEvents(handler: (event: CardEvent) => void): () => void {
    this.handlers.add(handler);
    if (this.automationTimer === undefined) this.scheduleAutomationDemo();
    return () => {
      this.handlers.delete(handler);
    };
  }

  private emit(e: CardEvent): void {
    this.events.push(e);
    this.handlers.forEach((h) => h(e));
  }

  // DEV ONLY: one scripted automation move ~11s after the board is live, so the
  // smooth automation-driven lane transition (product.md drag semantics) is
  // demonstrable without a backend. The benchmark session finishes exploring and
  // asks a question, which raises Needs You and animates the card across lanes.
  private scheduleAutomationDemo(): void {
    this.automationTimer = window.setTimeout(() => {
      const card = this.cards.find((c) => c.title.startsWith("Benchmark alacritty"));
      const session = this.sessions.find((s) => card && s.card_id === card.id);
      if (!card || !session) return;
      session.state = "needs_input";
      session.state_since = Date.now();
      session.stage = "explore";
      session.status_note = "which corpus should be the canonical benchmark?";
      card.lane = "needs_you";
      this.emit({ id: generateUlid(), card_id: card.id, kind: "state_changed", payload: { to: "needs_input" }, ts: Date.now() });
      this.emit({ id: generateUlid(), card_id: card.id, kind: "needs_input", payload: { question: session.status_note }, ts: Date.now() });
      this.emit({ id: generateUlid(), card_id: card.id, kind: "moved", payload: { from: "performing", to: "needs_you" }, ts: Date.now() });
      this.emit({
        id: generateUlid(),
        card_id: card.id,
        kind: "needs_you_raised",
        payload: {
          kind: "agent_needs_input",
          dedupe_key: `agent_needs_input:${card.id}`,
          score: 64,
          note: session.status_note,
        },
        ts: Date.now(),
      });
    }, 11_000);
  }
}

function delay(ms: number): Promise<void> {
  return new Promise((r) => setTimeout(r, ms));
}
