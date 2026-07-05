// Phase 1 domain + wire types. These mirror docs/spec/protocol.md payloads and
// docs/spec/data-model.md entity shapes EXACTLY (field names as specced), so the
// live client and the dev fixtures both speak the same contract. The Phase 0
// transport (envelope, binary frames, session.*) stays in protocol.ts.

// --- Board taxonomy ---------------------------------------------------------

// cards.lane values (data-model.md). "column" is an SQLite keyword, so the DB
// column is `lane`; protocol card.move uses `column` as the argument name.
export type Lane =
  | "inbox"
  | "shaping"
  | "ready"
  | "performing"
  | "verifying"
  | "needs_you"
  | "pr"
  | "done";

export const LANES: Lane[] = [
  "inbox",
  "shaping",
  "ready",
  "performing",
  "verifying",
  "needs_you",
  "pr",
  "done",
];

export const LANE_LABEL: Record<Lane, string> = {
  inbox: "Inbox",
  shaping: "Shaping",
  ready: "Ready",
  performing: "Performing",
  verifying: "Verifying",
  needs_you: "Needs You",
  pr: "PR",
  done: "Done",
};

// One-line, end-user-facing description of what a lane means, used in empty states.
export const LANE_HINT: Record<Lane, string> = {
  inbox: "Captured, not yet shaped.",
  shaping: "Turning ideas into dispatchable briefs.",
  ready: "Briefed and armed. Drop a card here to dispatch it.",
  performing: "Agents are working.",
  verifying: "Checks and review are running.",
  needs_you: "Blocked on a human decision.",
  pr: "Pushed, PR open.",
  done: "Merged or closed.",
};

// cards.type (data-model.md).
export type CardType = "feature" | "bug" | "chore" | "test" | "investigation";

export const CARD_TYPES: CardType[] = ["feature", "bug", "chore", "test", "investigation"];

// Launch harness set (adapters.md / product.md).
export type Harness = "claude" | "codex" | "opencode" | "pi";

export const HARNESSES: Harness[] = ["claude", "codex", "opencode", "pi"];

// sessions.state (data-model.md).
export type SessionState =
  | "starting"
  | "working"
  | "idle"
  | "needs_input"
  | "awaiting_feedback"
  | "blocked"
  | "done"
  | "error"
  | "interrupted";

// --- Entities (data-model.md tables) ---------------------------------------

export interface Project {
  id: string;
  path: string;
  name: string;
  default_branch: string;
  mode: "pr" | "local_only";
  check_cmds?: { name: string; cmd: string }[];
  default_recipe?: string | null;
  created_at: number;
  updated_at: number;
}

export interface Card {
  id: string;
  project_id: string | null; // nullable: cross-project / none
  type: CardType;
  title: string;
  lane: Lane;
  dial_recipe: string | null; // null -> project default
  priority: number; // 0 = none; higher = more urgent
  brief: string | null;
  origin_kind: "manual" | "github_issue" | "concertmaster" | "audit" | string;
  origin_ref: string | null;
  // Audit-sourced cards carry file:line evidence or a repro (product.md: every
  // audit card must). Surfaced on the card as an evidence line beside the origin badge.
  evidence?: string | null;
  created_at: number;
  updated_at: number;
}

// A session as seen by the board. `state`, `state_since`, `stage`, and
// `status_note` power the session strip. protocol.md returns the compact fleet
// row via session.list / fleet.status and lifecycle via event.subscribe; the
// stage and status_note surface (product.md session strip) are composed from the
// recipe stage and the latest tier-1 status event. See the design notes
// "Protocol interpretations" for the exact reconciliation note.
export interface Session {
  id: string;
  // Nullable since session-first (product.md: `sessions.card_id` nullable): a cardless
  // New Session has no card. The board hides cardless sessions; the Projects tree shows
  // them, so this must be honestly null rather than an empty-string sentinel.
  card_id: string | null;
  // cwd->project link the daemon captures at create for a cardless session (it matches
  // the New Session cwd to a registered project; data-model.md / api.rs summarize). This
  // is what gives a cardless session its Projects-tree home so it never goes invisible on
  // restart. For a carded session it mirrors the card's project. `project_name` is the
  // resolved display name (fleet.status joins it).
  project_id?: string | null;
  project_name?: string | null;
  harness: Harness | string;
  // Launcher identity (product.md Settings > Agents). `agent` is the launcher display
  // name (e.g. "cc-alt"), `agent_id` its row id. SessionSummary gains these in the
  // Phase 2 core; until then they are absent on the wire and the UI falls back to the
  // adapter family (harness). See the design notes interpretation 5.
  agent?: string | null;
  agent_id?: string | null;
  title?: string | null; // user-set label (session.rename); falls back to a generated name
  model?: string | null;
  effort?: string | null;
  state: SessionState;
  worktree_id?: string | null;
  first_prompt?: string | null;
  resume_ref?: string | null;
  resumed_from?: string | null; // lineage: this session resumed a predecessor (data-model.md)
  created_at: number;
  ended_at?: number | null;
  // Session-strip view fields (derived; see note above).
  state_since: number; // ms epoch the session entered `state` (for elapsed-in-state)
  stage?: string | null; // current recipe stage, e.g. "implement"
  status_note?: string | null; // last tier-1 status, e.g. "wiring reducer tests"
  // DEV-ONLY: a few preserved-scrollback lines for the interrupted-session resume demo
  // (fixture-only; live sessions replay the real ring through session.attach).
  scrollback_preview?: string[] | null;
}

// --- Attention Router: Needs You queue (data-model.md needs_you_items) --------

// needs_you_items.kind (data-model.md). Unknown kinds are tolerated and rendered
// generically (protocol.md: never drop unknown kinds).
export type NeedsYouKind =
  | "plan_round"
  | "gate_finding"
  | "agent_blocked"
  | "agent_stuck"
  | "trust_dialog"
  | "pr_ready"
  | "env_drift"
  | "service_failed"
  | "audit_digest" // onboarding audit completed; Inbox has cards to triage (product.md)
  | (string & {});

// A Needs You item as carried by fleet.status (dflow-proto NeedsYouItem). The wire
// twin of the persisted store row; fleet snapshots carry OPEN items only, so
// resolved_* are absent. `notified_at` drives the notification throttle (data-model.md).
export interface NeedsYouItem {
  id: string;
  card_id: string;
  kind: NeedsYouKind;
  dedupe_key: string;
  score: number; // priority + staleness + cost_of_delay (blocked agents outrank ideas)
  raised_at: number;
  notified_at?: number | null;
  resolved_at?: number | null;
  resolved_by?: "ui" | "concertmaster" | "auto" | string | null;
  // Optional enrichment the daemon may attach (note/title); tolerated when absent.
  note?: string | null;
}

// --- Session resume (architecture.md, session resume) -----------------------

// session.resume { session_id } -> { new_session_id }, lineage via resumed_from.
// The daemon may reject resume as unsupported for a harness/adapter that cannot
// resume by id; the UI then disables the action with a tooltip rather than failing.
export interface SessionResumeResult {
  ok: boolean;
  new_session_id?: string;
  unsupported?: boolean;
  error?: string;
}

// card_events.kind taxonomy (data-model.md). Unknown kinds must be preserved and
// rendered generically (protocol.md), never dropped.
export type CardEventKind =
  | "created"
  | "shaped"
  | "moved"
  | "dial_changed"
  | "closed"
  | "dispatched"
  | "worktree_leased"
  | "env_materialized"
  | "brief_composed"
  | "session_started"
  | "state_changed"
  | "turn_ended"
  | "needs_input"
  | "blocked"
  | "steered"
  | "session_ended"
  | "artifact_opened"
  | "plan_round"
  | "feedback_sent"
  | "plan_approved"
  | "artifact_ended"
  | "gate_started"
  | "gate_step"
  | "finding_raised"
  | "finding_resolved"
  | "gate_passed"
  | "gate_failed"
  | "pushed"
  | "pr_opened"
  | "ci_status"
  | "merged"
  | "worktree_returned"
  | "needs_you_raised"
  | "needs_you_resolved"
  | (string & {}); // preserve unknown kinds

export interface CardEvent {
  id: string; // ulid; doubles as the stream cursor
  card_id: string;
  kind: CardEventKind;
  payload?: Record<string, unknown> | null;
  ts: number;
}

// --- Composed board snapshot ------------------------------------------------

// The board is a linked projection of projects + cards + live sessions
// (product.md view 2/3). In live mode this is composed from project.list +
// card.query + session.list; the fixture serves it directly.
export interface BoardSnapshot {
  projects: Project[];
  cards: Card[];
  sessions: Session[];
  // The open Needs You queue (fleet.status.needs_you), highest score first. Powers
  // Mission Control's attention queue and the notification throttle. Empty when the
  // daemon serves none; the fixture synthesizes it from attention-state sessions.
  needsYou: NeedsYouItem[];
}

// --- Request payloads (protocol.md, exact field names) ----------------------

export interface CardCreateInput {
  title: string;
  type: CardType;
  project_id?: string | null;
  dial_recipe?: string | null;
  brief?: string | null;
}

export interface DispatchStartInput {
  card_id: string;
  recipe?: string | null;
  agent?: string | null; // configured launcher (name or id); precedence over harness
  harness?: Harness | null;
  model?: string | null;
  effort?: string | null;
}

export interface ProjectAddResult {
  ok: boolean;
  project?: Project;
  error?: string; // validation message (not a git repo, path missing, etc.)
}

// --- Configured agents (Settings > Agents; data-model.md / agents) -----------

// Adapter behavior families a launcher references (adapters.md / product.md). The
// daemon's known set is these five families plus `custom` (dflow-core / harness).
export type AdapterFamily = "claude" | "codex" | "opencode" | "cursor" | "pi" | "custom";

export const ADAPTER_FAMILIES: AdapterFamily[] = [
  "claude",
  "codex",
  "opencode",
  "cursor",
  "pi",
  "custom",
];

// A configured launcher: an adapter family paired with the user's own command,
// default args, and env. Mirrors dflow-proto Agent EXACTLY (entities.rs / Agent):
// `caution` is computed by the daemon (extra_args weaken safety), never stored.
export interface Agent {
  id: string;
  name: string;
  adapter: AdapterFamily | string;
  command: string;
  extra_args: string[];
  extra_env: Record<string, string>;
  source: "detected" | "custom" | string;
  detected_version?: string | null;
  enabled: boolean;
  caution: boolean;
}

// agents.add payload (messages.rs / AgentAdd). Creates a source:custom launcher.
export interface AgentAddInput {
  name: string;
  adapter: string;
  command: string;
  extra_args: string[];
  extra_env: Record<string, string>;
}

// agents.update payload (messages.rs / AgentUpdate). `id` is a launcher id or name;
// absent fields are unchanged.
export interface AgentUpdateInput {
  id: string;
  name?: string;
  adapter?: string;
  command?: string;
  extra_args?: string[];
  extra_env?: Record<string, string>;
  enabled?: boolean;
}

// One CLI the PATH scan turned up this run (messages.rs / DetectedCli).
export interface DetectedCli {
  name: string;
  command: string;
  version?: string | null;
  created: boolean;
}

// agents.detect response (messages.rs / AgentsDetected).
export interface AgentsDetectResult {
  found: DetectedCli[];
  agents: Agent[];
}

// Mutation results surface daemon validation inline (mirrors ProjectAddResult),
// so forms show a precise message instead of throwing.
export interface AgentMutationResult {
  ok: boolean;
  agent?: Agent;
  error?: string;
}

export interface AgentRemoveResult {
  ok: boolean;
  removed?: string;
  error?: string;
  // The daemon refused because a non-ended session references the launcher; the UI
  // offers "Disable instead" (data-model.md / Honesty note; api.rs agents_remove).
  inUse?: boolean;
}

// --- Session-first front door (product.md / Session-first workflow) ----------

// A New Session launch: session.create { agent, cwd } with no card. cwd is the
// project path for now (worktree-leased New Session arrives with the dflow-cli phase).
export interface SessionStartInput {
  agent: string; // launcher name or id
  projectId: string | null;
  cwd?: string | null; // project path
  firstPrompt?: string | null;
}

// A cardless live session started from the front door. Tracked client-side because
// the daemon does not persist bare sessions with project or launcher linkage:
// session.list/fleet.status return them with null project_id/first_prompt and only
// the adapter family in `harness` (see the design notes). `agent` carries
// the launcher name for identity; `harness` is the adapter family for the glyph.
export interface LaunchedSession {
  sessionId: string;
  agent: string;
  harness: string;
  projectId: string | null;
  firstPrompt?: string | null;
  title?: string | null;
  createdAt: number;
  alive: boolean;
}

// --- The Concertmaster (product.md / The Concertmaster) ----------------------

// The Concertmaster is a persistent, cardless harness session with dflow-mcp mounted,
// hosted in the dockable panel. Tracked client-side (like a New Session launch) with a
// `concertmaster` flag until core lands the scoped-profile session (docs/spec/security.md
// / Concertmaster capability scope). One active session at a time.
export interface ConcertmasterSession {
  sessionId: string; // real daemon session id (or a synthetic id in demo mode)
  agentId: string; // configured launcher id
  agentName: string; // launcher display name (e.g. "claude", "cc-alt")
  harness: string; // adapter family, for the glyph
  // The scoped-session focus (product.md scoped sessions): when set, the panel steers
  // the Concertmaster to keep to this project. A parameter, not a separate architecture.
  scopeProjectId: string | null;
  // Whether dflow-mcp was confirmed mounted at setup (null = could not detect).
  mounted: boolean | null;
  createdAt: number;
  alive: boolean;
  // DEV/demo: a fixture transcript with no live PTY, for offline screenshots.
  demo: boolean;
}

// Setup-flow result: launch a Concertmaster with a chosen launcher and working dir.
export interface ConcertmasterStartInput {
  agent: string; // launcher id or name
  cwd?: string | null; // where the harness launches (mount + scope context live here)
  scopeProjectId?: string | null;
  mounted?: boolean | null;
}

// --- Flow recipes (recipes.md / product.md process dial) --------------------

// The fixed stage vocabulary (recipes.md: "The stage vocabulary is fixed").
export type RecipeStage = "shape" | "plan" | "implement" | "verify" | "ship";

export type RecipeScope = "bundled" | "user" | "project";
export type PlanMode = "artifact" | "markdown" | "none";
export type GateMode = "full" | "checks_only" | "none";
export type ShipTarget = "pr" | "local_merge" | "none";

// Trust tier (security.md / Recipe trust tiers). `standard` runs with no extra
// consent; `privileged` requires an explicit per-project grant that lists exactly
// what is elevated and is re-confirmed when the recipe file's hash changes.
export type TrustTier = "standard" | "privileged";

// One elevated capability of a privileged recipe, for the consent summary UI.
export type PrivilegeKind = "mcp" | "worktree_in_place" | "gate_disabled" | "local_merge";

export interface RecipePrivilege {
  kind: PrivilegeKind;
  // Exactly what is elevated, verbatim (recipes.md: "the full MCP command lines,
  // the in-place target, the disabled gate").
  detail: string;
}

// A per-stage one-line summary for the dial's stage list.
export interface StageLine {
  stage: RecipeStage | string;
  note: string; // e.g. "artifact · approval required", "full gate", "PR"
}

// A recipe as read by the dial (recipe.list). The engine enforces front matter;
// the dial only needs the display + trust surface.
export interface Recipe {
  name: string;
  version: number;
  description: string;
  scope: RecipeScope;
  source: string; // "bundled" or the winning file path (recipes.md: UI shows which won)
  stages: (RecipeStage | string)[];
  stageLines: StageLine[];
  planMode: PlanMode;
  approval: "required" | "auto";
  gate: GateMode;
  shipTarget: ShipTarget;
  trust: TrustTier;
  privileges: RecipePrivilege[]; // empty for standard recipes
  contentHash: string; // re-confirm a grant when this changes (security.md)
  investigation: boolean; // shipless (audit / audit-deep): true
}

// The structured error the daemon returns when a privileged recipe is dispatched
// onto a project that has not granted it (recipes.md / security.md). Mirrors the
// protocol.md error envelope's `code` + `detail`.
export interface RecipeGrantError {
  code: "recipe_grant_required";
  message: string;
  recipe: string;
  project_id: string;
  privileges: RecipePrivilege[];
  contentHash: string;
}

// --- Onboarding audit (product.md / Card sources: onboarding audit) ---------

export type AuditDepth = "quick" | "deep";

// dispatch of the audit / audit-deep recipe against a project (no card). Budgeted:
// quick caps at 10 cards / 6 notes, deep at 25 / 12; overflow goes into the audit's
// own report, never the board (product.md).
export interface AuditStartResult {
  ok: boolean;
  session_id?: string;
  error?: string;
}

// ============================================================================
// M5 - GitHub issue import (product.md / Card sources: GitHub issue import;
// protocol.md github.*; gate.md GitHub integration). gh-first: the daemon talks to
// GitHub through the local `gh` CLI, so auth is "is gh present and logged in", not an
// OAuth flow (roadmap.md M5.1: `github.auth.*` report gh presence/auth).
// ============================================================================

// github.auth.status -> gh CLI presence + auth. When gh is absent or unauthenticated,
// PR mode degrades cleanly to local-only with a one-line setup pointer (gate.md).
export interface GithubAuthStatus {
  gh_present: boolean; // the gh CLI is on PATH
  authenticated: boolean; // `gh auth status` reports a logged-in account
  user?: string | null; // the authenticated login
  host?: string | null; // github.com or an enterprise host
  scopes?: string[]; // token scopes gh reports
  setup_hint?: string | null; // the one-line pointer shown when absent/unauthenticated
}

// Per-project import config (product.md: assignee, label, and milestone filters, or a
// curated picker; never an unfiltered firehose). The daemon has no dedicated verb yet,
// so INTERPRET: project.update carries a github import block (listed in phase12-m5ui.md).
// Empty filters mean the curated picker: preview every open issue and pick by hand.
export interface GithubImportConfig {
  assignees: string[]; // login filters (e.g. ["@me"])
  labels: string[]; // label filters
  milestone: string | null;
  state: "open" | "all"; // default open
}

export interface GithubLabel {
  name: string;
  color?: string | null; // 6-hex from gh, tinted in the UI
}

export interface GithubComment {
  author: string;
  body: string;
  created_at: number;
}

// A github.issues.preview row (product.md: the filtered list, without importing).
export interface GithubIssue {
  number: number;
  title: string;
  body: string; // markdown source, rendered read-only in the Issue tab
  state: "open" | "closed";
  author: string;
  assignees: string[];
  labels: GithubLabel[];
  milestone: string | null;
  comments: GithubComment[];
  url: string;
  repo: string; // "owner/name"
  updated_at: number;
  // Dedupe surface (product.md: one issue, one card; re-import refreshes). Set when a
  // card already exists for this issue, so the preview can show "imported".
  imported_card_id?: string | null;
  // Label-heuristic card type the import assigns (bug/feature/...) (product.md).
  suggested_type: CardType;
}

// github.issues.import result. Dedupe on origin_ref: new issues create cards, known
// ones refresh fields but respect local lane moves (product.md).
export interface GithubImportResult {
  ok: boolean;
  imported: number; // new origin cards created
  refreshed: number; // existing origin cards refreshed in place
  card_ids: string[];
  error?: string;
}

// ============================================================================
// M5 - Verification gate (gate.md; protocol.md gate verbs). A gate run moves a card
// to Verifying (product.md); nothing becomes a PR because an agent says it is done.
// ============================================================================

export type GateCheckStatus = "pending" | "running" | "passed" | "failed" | "skipped";

// One registered check command run in a gate-class worktree (gate.md step 1). Output
// is captured as evidence, scrubbed of known secret values (security.md) before storage.
export interface GateCheck {
  name: string; // build | test | lint | typecheck | ...
  cmd: string;
  status: GateCheckStatus;
  exit_code?: number | null;
  duration_ms?: number | null;
  output?: string | null; // captured evidence tail
}

export type FindingSeverity = "blocker" | "major" | "minor";
// The human's judgment on an escalated finding (gate.md step 4): approve (accept as
// designed / not a real problem), fix (send back for a change), skip (defer, annotated).
export type FindingResolution = "approve" | "fix" | "skip";
// Autofix classification (gate.md steps 3/4): mechanical findings (lint, formatting,
// dead imports, trivial test fixes) are applied by a fixer and re-checked; intent
// findings (behavior, API shape, scope) escalate to the human finding review.
export type FindingClass = "mechanical" | "intent";

export interface GateFinding {
  id: string;
  severity: FindingSeverity;
  title: string;
  // Every finding needs a concrete failure scenario or rule citation, not vibes (gate.md).
  scenario: string;
  rule?: string | null;
  file?: string | null;
  line?: number | null;
  klass: FindingClass;
  // The resolution once decided; null while it still awaits the human. A mechanical
  // finding the fixer already applied carries auto_applied and resolution "fix".
  resolution?: FindingResolution | null;
  auto_applied?: boolean;
}

export type GateStepId = "checks" | "review" | "autofix" | "ship";

export type GateStatus =
  | "pending"
  | "checks_running"
  | "review"
  | "awaiting_human" // intent findings escalated; the human resolves them in chrome
  | "autofixing"
  | "passed"
  | "failed"
  | (string & {});

// PR lifecycle (gate.md step 5 / GitHub integration). Push goes through the git CLI's
// credential helper; PR create / CI watch / merge go through `gh`. Merge stays disabled
// until CI is green AND every finding is resolved (disabled-until-green).
export type PrStatus =
  | "none"
  | "pushing"
  | "open"
  | "ci_running"
  | "ci_failed"
  | "ci_passed"
  | "merged"
  | (string & {});

export type CiCheckStatus = "queued" | "running" | "success" | "failure" | "neutral";

export interface CiCheck {
  name: string;
  status: CiCheckStatus;
}

export interface PrState {
  status: PrStatus;
  number?: number | null;
  url?: string | null;
  branch: string;
  ci: CiCheck[];
  // True only when CI is green and every finding resolved (gate.md ship). Drives the
  // Merge action's disabled-until-green rule.
  mergeable: boolean;
  merge_method?: "squash" | "merge" | "rebase"; // squash default (gate.md)
  // "owner/name#n" when the card origin is a GitHub issue, so the PR body's Fixes line
  // closes it on merge (gate.md / product.md close-the-loop).
  fixes_issue?: string | null;
}

// A gate run for a card (card.get -> gate_runs, once the M5 core serves it). All checks,
// findings, and resolutions are card_events with evidence pointers (gate.md Findings).
export interface GateRun {
  id: string;
  card_id: string;
  status: GateStatus;
  mode: GateMode; // full | checks_only (recipe knob)
  worktree_id: string;
  reviewer_harness?: string | null; // the adversarial reviewer, a different harness (gate.md)
  started_at: number;
  updated_at: number;
  checks: GateCheck[];
  findings: GateFinding[];
  // The findings-review artifact id: the Plan Studio chrome renders it as approve/fix/
  // skip per finding (gate.md step 4, reuse plan-studio.md). Mirrors ArtifactMeta.doc_id.
  findings_doc_id?: string | null;
  pr: PrState;
}

// ============================================================================
// M6 - Remote access (security.md / Remote access trust model; LAN-first). The daemon
// LAN listener does not exist yet, so the desktop fixtures the token/URL and marks the
// integration seam; enabling it in the UI shows the honest no-TLS caveat.
// ============================================================================

// The default phone capability profile (security.md): Needs You, approvals, steering;
// terminals read-only; no vault access, no recipe install.
export interface RemoteCapabilityProfile {
  needs_you: boolean;
  approvals: boolean;
  steering: boolean;
  terminals_read_only: boolean;
  vault_access: boolean; // false in the first remote release
  recipe_install: boolean; // false
}

export interface PairedDevice {
  id: string;
  name: string; // user-facing label ("Matt's iPhone")
  profile: string; // "phone" (the default scoped profile) or a custom label
  paired_at: number;
  last_seen?: number | null;
  capabilities: RemoteCapabilityProfile;
}

// The opt-in LAN listener state (security.md: separate from loopback, off by default,
// no TLS on LAN v1 - the capability token is the gate). The pairing payload is the URL
// apps/mobile settled on: http://<lan-ip>:<port>/m#pair=<base64url{url,token}>.
export interface RemoteListenerState {
  enabled: boolean;
  lan_ip?: string | null; // the machine's LAN address
  port?: number | null;
  url?: string | null; // the plaintext base URL: http://<lan-ip>:<port>/m
  pairing_payload?: string | null; // the full URL with #pair fragment (QR-encoded)
  token?: string | null; // the phone-scoped capability token (shown for the plaintext path)
  minted_at?: number | null;
  profile: RemoteCapabilityProfile; // the default phone profile the listener advertises
  devices: PairedDevice[];
}
