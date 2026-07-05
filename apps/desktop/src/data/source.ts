// The board's data-access seam. Every view reads and mutates through this one
// interface, so the live protocol client and the dev fixtures are drop-in
// interchangeable. Methods map 1:1 to protocol.md verbs (noted per method).

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
  DispatchStartInput,
  FindingResolution,
  GateRun,
  GithubAuthStatus,
  GithubImportConfig,
  GithubImportResult,
  GithubIssue,
  Lane,
  ProjectAddResult,
  Recipe,
  RemoteListenerState,
  SessionResumeResult,
} from "../model";
import { ArtifactMeta, FeedbackSubmit, FeedbackSubmitResult } from "../review/protocol";

export interface DataSource {
  /** "fixture" is DEV ONLY; "live" talks to dflowd over the protocol. */
  readonly mode: "fixture" | "live";

  /**
   * project.list + card.query + fleet.status, composed into one board projection.
   * fleet.status carries both the live sessions and the open Needs You queue.
   */
  loadSnapshot(): Promise<BoardSnapshot>;

  /**
   * session.resume { session_id } -> { new_session_id }. Relaunches the harness in
   * the same worktree with its resume flag; lineage recorded via resumed_from
   * (architecture.md). Rejection as unsupported degrades to a disabled action.
   */
  resumeSession(sessionId: string): Promise<SessionResumeResult>;

  /** project.add { path } with git-repo validation. */
  addProject(path: string): Promise<ProjectAddResult>;

  /** card.create { title, type, project_id?, dial_recipe?, brief? }. */
  createCard(input: CardCreateInput): Promise<Card>;

  /** card.move { card_id, column }. */
  moveCard(cardId: string, lane: Lane): Promise<Card>;

  /** card.update { card_id, dial_recipe } - the process dial (product.md). */
  updateCardDial(cardId: string, recipe: string | null): Promise<Card>;

  /** dispatch.start { card_id, recipe?, harness?, model?, effort? }. */
  dispatch(input: DispatchStartInput): Promise<{ session_id?: string }>;

  /** dispatch.cancel { card_id }. */
  cancelDispatch(cardId: string): Promise<void>;

  /** card.get { card_id } -> the card's event timeline (newest last). */
  cardEvents(cardId: string): Promise<CardEvent[]>;

  /**
   * Recent cross-project activity for Mission Control's feed, newest first. In live
   * mode the event.subscribe stream fills this going forward; the initial pull is a
   * best-effort snapshot (empty when the daemon serves no history replay).
   */
  recentActivity(limit?: number): Promise<CardEvent[]>;

  /** session.rename { session_id, title }. */
  renameSession(sessionId: string, title: string): Promise<void>;

  // --- Configured agents (agents.*) -----------------------------------------

  /** agents.list {} -> the configured launchers with source/version/caution. */
  listAgents(): Promise<Agent[]>;

  /** agents.detect {} -> PATH scan: CLIs found this run + the refreshed launchers. */
  detectAgents(): Promise<AgentsDetectResult>;

  /** agents.add { name, adapter, command, extra_args, extra_env }; errors inline. */
  addAgent(input: AgentAddInput): Promise<AgentMutationResult>;

  /** agents.update { id, ... }; validation errors surfaced inline. */
  updateAgent(input: AgentUpdateInput): Promise<AgentMutationResult>;

  /** agents.remove { id }; refusal (live session references it) surfaced inline. */
  removeAgent(id: string): Promise<AgentRemoveResult>;

  /**
   * event.subscribe { cursor? }: live card_events (lane moves, state changes,
   * status notes). Returns an unsubscribe. Handlers must tolerate unknown kinds.
   */
  subscribeEvents(handler: (event: CardEvent) => void): () => void;

  // --- Flow recipes (recipe.list) -------------------------------------------

  /** recipe.list {} -> bundled + user + project recipes with source + trust tier. */
  listRecipes(): Promise<Recipe[]>;

  // --- Plan Studio artifact review (artifact.*) -----------------------------

  /**
   * The latest plan artifact for a card (from card.get artifacts), or null if the
   * card has none. The daemon's artifact service is landing next; live mode returns
   * null until then (the Plan tab shows its empty state), fixtures serve the demo.
   */
  getPlanArtifact(cardId: string): Promise<ArtifactMeta | null>;

  /**
   * Mint a short-lived signed serving URL for an artifact doc (artifact.get
   * capability URL). The iframe holds the capability, never a bearer token.
   */
  signArtifactUrl(docId: string): Promise<string>;

  /** artifact.feedback.submit { artifact_id, round, items } (from the review chrome). */
  submitFeedback(input: FeedbackSubmit): Promise<FeedbackSubmitResult>;

  /**
   * Record a first-class plan approval (plan_approved) and resolve the card's
   * plan_round Needs You item (plan-studio.md: Approve is a first-class action).
   */
  approvePlan(artifactId: string, cardId: string): Promise<void>;

  // --- Onboarding audit (product.md / Card sources: onboarding audit) -------

  /**
   * Dispatch the audit / audit-deep recipe against a project (no card). Files
   * budgeted findings into Inbox as origin:audit cards and raises one audit_digest
   * Needs You item on completion. Never auto-runs; always offered.
   */
  startAudit(projectId: string, depth: AuditDepth): Promise<AuditStartResult>;

  // --- GitHub issue import (github.*; product.md) ----------------------------

  /**
   * github.auth.status {} -> gh CLI presence + auth (roadmap.md M5.1: reports gh
   * presence/auth, not an OAuth flow). Live mode degrades to gh-absent honestly.
   */
  githubAuthStatus(): Promise<GithubAuthStatus>;

  /** The project's saved import filters (assignee/label/milestone). */
  getGithubImportConfig(projectId: string): Promise<GithubImportConfig>;

  /** Persist the project's import filters (INTERPRET: project.update github block). */
  setGithubImportConfig(projectId: string, config: GithubImportConfig): Promise<GithubImportConfig>;

  /**
   * github.issues.preview { project_id } -> the filtered issue list per the project's
   * import config, WITHOUT importing (product.md). Empty result when gh is unauthed.
   */
  previewGithubIssues(projectId: string): Promise<GithubIssue[]>;

  /**
   * github.issues.import { project_id, numbers? } -> creates/refreshes origin cards
   * (dedupe on origin_ref). Omitting numbers imports the configured filter set.
   */
  importGithubIssues(projectId: string, numbers: number[]): Promise<GithubImportResult>;

  /**
   * The GitHub issue behind an origin:github_issue card (origin_ref -> repo#number),
   * for the Issue tab. Null when the card is not issue-sourced or gh cannot fetch it.
   */
  getGithubIssueForCard(card: Card): Promise<GithubIssue | null>;

  // --- Verification gate (gate.md; protocol.md gate verbs) -------------------

  /**
   * The card's active/last gate run (card.get -> gate_runs once the M5 core serves
   * it). Null when the card has never been verified; live mode returns null until the
   * daemon's gate engine lands (the Verify tab shows its empty state).
   */
  getGateRun(cardId: string): Promise<GateRun | null>;

  /**
   * Trigger the gate for a card (gate.md: "or when the user clicks Verify"). Leases a
   * gate-class worktree, runs checks, then adversarial review. Moves the card to Verifying.
   */
  startGate(cardId: string): Promise<{ ok: boolean; error?: string }>;

  /**
   * Record the human's judgment on one escalated finding (gate.md step 4: approve /
   * fix / skip). The findings review renders in the Plan Studio chrome; this persists
   * the resolution and advances the gate once every finding is resolved.
   */
  resolveGateFinding(cardId: string, findingId: string, resolution: FindingResolution): Promise<GateRun>;

  /**
   * Merge the card's PR (gate.md ship: squash default). Rejected unless mergeable
   * (CI green and every finding resolved); the UI disables the action until then.
   */
  mergePr(cardId: string): Promise<{ ok: boolean; error?: string }>;

  // --- Remote access / device pairing (M6; security.md) ----------------------
  // The daemon LAN listener does not exist yet: these are fixtured with a clear
  // integration seam. Live mode reports the listener disabled + unavailable honestly.

  /** The LAN listener state: enabled flag, pairing URL/token, paired devices. */
  getRemoteState(): Promise<RemoteListenerState>;

  /** Toggle the opt-in LAN listener; enabling mints the pairing URL + phone token. */
  setRemoteEnabled(enabled: boolean): Promise<RemoteListenerState>;

  /** Rotate the phone capability token; invalidates the current QR and paired devices. */
  rotateRemoteToken(): Promise<RemoteListenerState>;

  /** Revoke one paired device (security.md: per-device revocation from Settings). */
  revokeRemoteDevice(deviceId: string): Promise<RemoteListenerState>;
}
