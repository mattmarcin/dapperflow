// LiveDataSource: the board over the real dflowd protocol. Coded against
// protocol.md payload shapes exactly. The card.*, project.*, dispatch.*, and
// event.* families are being built by the backend agent in parallel; until they
// land the app runs on the fixture source (see index.ts). Where protocol.md
// specifies a request but not the exact response object, the interpretation is
// marked INTERPRET and listed in the design notes

import { DflowClient } from "../client";
import { Envelope } from "../protocol";
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
  NeedsYouItem,
  Project,
  ProjectAddResult,
  Recipe,
  RemoteCapabilityProfile,
  RemoteListenerState,
  Session,
  SessionResumeResult,
} from "../model";
import { ArtifactMeta, FeedbackSubmit, FeedbackSubmitResult } from "../review/protocol";
import { DataSource } from "./source";
import { RECIPE_FIXTURES } from "./recipe-fixtures";

// The fleet.status / session.list wire row (dflow-proto SessionSummary).
// Distinct from the board's Session view-model, which sessionFromWire builds.
// `agent` / `agent_id` are read tolerantly: SessionSummary gains them in the Phase 2
// core, and are absent on the Phase 1.5 daemon this worktree runs against.
interface WireSessionSummary {
  session_id: string;
  card_id?: string | null;
  project_id?: string | null;
  project_name?: string | null;
  harness: string;
  agent?: string | null;
  agent_id?: string | null;
  title?: string | null;
  state: string;
  alive: boolean;
  elapsed_ms: number;
  resume_ref?: string | null;
  resumed_from?: string | null;
  first_prompt?: string | null;
  created_at_ms: number;
}

// The fleet.status needs_you wire row (dflow-proto NeedsYouItem). Snake-case names
// match the model twin; open items only (resolved_* absent in a fleet snapshot).
interface WireNeedsYouItem {
  id: string;
  card_id: string;
  kind: string;
  dedupe_key: string;
  score: number;
  raised_at: number;
  notified_at?: number | null;
  note?: string | null;
}

function sessionFromWire(w: WireSessionSummary): Session {
  return {
    id: w.session_id,
    card_id: w.card_id ?? null,
    // Carry the daemon's cwd->project match through. Dropping it (the old behavior) is
    // why cardless sessions vanished from the Projects tree on restart: the tree groups
    // by this, and it was never read even though fleet.status sends it.
    project_id: w.project_id ?? null,
    project_name: w.project_name ?? null,
    harness: w.harness,
    agent: w.agent ?? null,
    agent_id: w.agent_id ?? null,
    title: w.title ?? null,
    state: w.state as Session["state"],
    first_prompt: w.first_prompt ?? null,
    resume_ref: w.resume_ref ?? null,
    resumed_from: w.resumed_from ?? null,
    created_at: w.created_at_ms,
    ended_at: null,
    // v0: the wire carries elapsed-since-creation only; state transition
    // timestamps arrive with tier-1/2 signals (Phase 2), so elapsed-in-state
    // approximates as elapsed-since-creation until then.
    state_since: Date.now() - w.elapsed_ms,
    stage: null,
    status_note: null,
  };
}

function needsYouFromWire(w: WireNeedsYouItem): NeedsYouItem {
  return {
    id: w.id,
    card_id: w.card_id,
    kind: w.kind,
    dedupe_key: w.dedupe_key,
    score: w.score,
    raised_at: w.raised_at,
    notified_at: w.notified_at ?? null,
    note: w.note ?? null,
  };
}

export class LiveDataSource implements DataSource {
  readonly mode = "live" as const;

  constructor(private readonly client: DflowClient) {}

  async loadSnapshot(): Promise<BoardSnapshot> {
    // Cross-project board = registered projects + all cards + live fleet state.
    // fleet.status is one snapshot carrying both the session table and the open
    // Needs You queue (dflow-proto FleetStatusResult).
    const [projects, cards, fleet] = await Promise.all([
      this.client.call<{ projects: Project[] }>("project.list", {}).then((r) => r.projects),
      this.client.call<{ cards: Card[] }>("card.query", { filter: {} }).then((r) => r.cards),
      this.client
        .call<{ sessions: WireSessionSummary[]; needs_you?: WireNeedsYouItem[] }>("fleet.status", {})
        .catch(() => ({ sessions: [] as WireSessionSummary[], needs_you: [] as WireNeedsYouItem[] })),
    ]);
    const sessions = fleet.sessions.map(sessionFromWire);
    const needsYou = (fleet.needs_you ?? []).map(needsYouFromWire).sort((a, b) => b.score - a.score);
    return { projects, cards, sessions, needsYou };
  }

  async resumeSession(sessionId: string): Promise<SessionResumeResult> {
    try {
      // session.resume relaunches the harness in the same worktree; the daemon
      // returns the new session id and records lineage via resumed_from.
      const res = await this.client.call<{ new_session_id: string }>("session.resume", {
        session_id: sessionId,
      });
      return { ok: true, new_session_id: res.new_session_id };
    } catch (e) {
      // A daemon that cannot resume this harness by id answers with an unsupported /
      // unknown-verb error; the UI then disables the action with a tooltip rather than
      // pretending it worked (architecture.md: degrade gracefully).
      const msg = errorMessage(e);
      const unsupported = /unsupported|not supported|unknown|resume/i.test(msg);
      return { ok: false, unsupported, error: msg };
    }
  }

  async addProject(path: string): Promise<ProjectAddResult> {
    try {
      // project.add -> { project_id }; re-read to hydrate the full row.
      const { project_id } = await this.client.call<{ project_id: string }>("project.add", { path });
      const { projects } = await this.client.call<{ projects: Project[] }>("project.list", {});
      const project = projects.find((p) => p.id === project_id);
      return { ok: true, project };
    } catch (e) {
      return { ok: false, error: errorMessage(e) };
    }
  }

  async createCard(input: CardCreateInput): Promise<Card> {
    // INTERPRET: card.create response shape is unspecified; we expect the created
    // card, falling back to card.get by returned id.
    const res = await this.client.call<{ card?: Card; card_id?: string }>("card.create", input);
    if (res.card) return res.card;
    if (res.card_id) {
      const got = await this.client.call<{ card: Card }>("card.get", { card_id: res.card_id });
      return got.card;
    }
    throw new Error("card.create returned no card");
  }

  async moveCard(cardId: string, lane: Lane): Promise<Card> {
    // protocol.md names the argument `column` (the DB stores it as `lane`).
    const res = await this.client.call<{ card?: Card }>("card.move", {
      card_id: cardId,
      column: lane,
    });
    if (res.card) return res.card;
    const got = await this.client.call<{ card: Card }>("card.get", { card_id: cardId });
    return got.card;
  }

  async updateCardDial(cardId: string, recipe: string | null): Promise<Card> {
    // card.update { card_id, ... } (protocol.md). INTERPRET: dial selection is the
    // card's dial_recipe field; response mirrors card.move (card or re-get).
    const res = await this.client.call<{ card?: Card }>("card.update", {
      card_id: cardId,
      dial_recipe: recipe,
    });
    if (res.card) return res.card;
    const got = await this.client.call<{ card: Card }>("card.get", { card_id: cardId });
    return got.card;
  }

  async dispatch(input: DispatchStartInput): Promise<{ session_id?: string }> {
    return this.client.call<{ session_id?: string }>("dispatch.start", input);
  }

  async cancelDispatch(cardId: string): Promise<void> {
    await this.client.call("dispatch.cancel", { card_id: cardId });
  }

  async cardEvents(cardId: string): Promise<CardEvent[]> {
    // card.get returns the card plus its latest events (protocol.md card.get).
    const res = await this.client.call<{ events?: CardEvent[] }>("card.get", { card_id: cardId });
    return res.events ?? [];
  }

  async recentActivity(limit = 60): Promise<CardEvent[]> {
    // The daemon streams new events over event.subscribe; there is no history-replay
    // verb yet, so the initial feed starts empty and fills live. When the daemon later
    // serves a bounded replay, wire it here.
    void limit;
    return [];
  }

  async renameSession(sessionId: string, title: string): Promise<void> {
    await this.client.rename(sessionId, title);
  }

  // --- Configured agents (agents.*) -----------------------------------------

  async listAgents(): Promise<Agent[]> {
    const res = await this.client.call<{ agents: Agent[] }>("agents.list", {});
    return res.agents;
  }

  async detectAgents(): Promise<AgentsDetectResult> {
    // agents.detect runs the real PATH scan and upserts detected launchers.
    return this.client.call<AgentsDetectResult>("agents.detect", {});
  }

  async addAgent(input: AgentAddInput): Promise<AgentMutationResult> {
    try {
      const res = await this.client.call<{ agent: Agent }>("agents.add", input);
      return { ok: true, agent: res.agent };
    } catch (e) {
      return { ok: false, error: errorMessage(e) };
    }
  }

  async updateAgent(input: AgentUpdateInput): Promise<AgentMutationResult> {
    try {
      const res = await this.client.call<{ agent: Agent }>("agents.update", input);
      return { ok: true, agent: res.agent };
    } catch (e) {
      return { ok: false, error: errorMessage(e) };
    }
  }

  async removeAgent(id: string): Promise<AgentRemoveResult> {
    try {
      const res = await this.client.call<{ removed: string }>("agents.remove", { id });
      return { ok: true, removed: res.removed };
    } catch (e) {
      // The store refuses while a non-ended session references the launcher, with a
      // message that suggests disabling instead (api.rs agents_remove).
      const msg = errorMessage(e);
      const inUse = /in use|disable it instead|active session/i.test(msg);
      return { ok: false, error: msg, inUse };
    }
  }

  // --- Flow recipes (recipe.list) -------------------------------------------

  async listRecipes(): Promise<Recipe[]> {
    // recipe.list {} -> bundled + user + project recipes with source (protocol.md).
    // The recipes crate is landing in a parallel lane; until this daemon serves the
    // verb, fall back to the bundled fixture catalog so the dial stays usable.
    // INTERPRET: response shape { recipes: Recipe[] } (listed in phase5-m3-ui.md).
    try {
      const res = await this.client.call<{ recipes?: Recipe[] }>("recipe.list", {});
      if (Array.isArray(res.recipes) && res.recipes.length > 0) return res.recipes;
      return RECIPE_FIXTURES.map((r) => ({ ...r }));
    } catch {
      return RECIPE_FIXTURES.map((r) => ({ ...r }));
    }
  }

  // --- Plan Studio artifact review (artifact.*) -----------------------------
  // The daemon's artifact service lands in the next crates lane. Until it serves
  // artifact.get / artifact.feedback.submit, live mode degrades honestly: no
  // artifact -> the Plan tab shows its empty state; submit/approve reject with a
  // clear message instead of pretending. Nothing here fabricates plan data.

  async getPlanArtifact(cardId: string): Promise<ArtifactMeta | null> {
    // INTERPRET: card.get { card_id } -> { artifacts?: ArtifactMeta[] } once the
    // artifact service lands; absent field means no plan artifact.
    try {
      const res = await this.client.call<{ artifacts?: ArtifactMeta[] }>("card.get", {
        card_id: cardId,
      });
      const list = res.artifacts ?? [];
      const plans = list.filter((a) => a.kind === "plan").sort((a, b) => b.updated_at - a.updated_at);
      return plans[0] ?? null;
    } catch {
      return null;
    }
  }

  async signArtifactUrl(docId: string): Promise<string> {
    // artifact.get -> short-lived signed capability URL (security.md). The daemon
    // does not serve this yet; in dev the Vite artifact middleware answers on the
    // same origin, so this works under `pnpm tauri dev` and fails cleanly otherwise.
    const res = await fetch(`/__artifact/sign?id=${encodeURIComponent(docId)}`);
    if (!res.ok) throw new Error(`artifact serving endpoint answered ${res.status}`);
    const { url } = (await res.json()) as { url: string };
    return url;
  }

  async submitFeedback(input: FeedbackSubmit): Promise<FeedbackSubmitResult> {
    // artifact.feedback.submit { artifact_id, items } (protocol.md).
    const res = await this.client.call<Partial<FeedbackSubmitResult>>(
      "artifact.feedback.submit",
      { artifact_id: input.artifact_id, round: input.round, items: input.items, layout_warnings: input.layout_warnings },
    );
    return {
      ok: res.ok ?? true,
      round: res.round ?? input.round + 1,
      revised_doc_id: res.revised_doc_id ?? null,
      next_step: res.next_step ?? "revise the artifact in place, then poll again",
    };
  }

  async approvePlan(artifactId: string, cardId: string): Promise<void> {
    // INTERPRET: approval is a first-class feedback action (plan-studio.md records
    // plan_approved); modeled as an `action` item until the daemon names a verb.
    await this.client.call("artifact.feedback.submit", {
      artifact_id: artifactId,
      card_id: cardId,
      items: [{ kind: "action", action: "approve_plan", body: null }],
    });
  }

  // --- Onboarding audit ------------------------------------------------------

  async startAudit(projectId: string, depth: AuditDepth): Promise<AuditStartResult> {
    // The audit is a normal dispatch of the bundled audit / audit-deep recipe
    // (product.md: the recipe has no ship stage, structurally). No card: the audit
    // files its own cards. INTERPRET: dispatch.start with project_id and no card_id.
    try {
      const res = await this.client.call<{ session_id?: string }>("dispatch.start", {
        project_id: projectId,
        recipe: depth === "deep" ? "audit-deep" : "audit",
      });
      return { ok: true, session_id: res.session_id };
    } catch (e) {
      return { ok: false, error: errorMessage(e) };
    }
  }

  // --- GitHub issue import (github.*) ----------------------------------------
  // The gh transport reports gh presence/auth rather than running OAuth (roadmap.md
  // M5.1). Absent/unauthed degrades cleanly: an empty preview and a setup pointer.

  async githubAuthStatus(): Promise<GithubAuthStatus> {
    try {
      const res = await this.client.call<Partial<GithubAuthStatus>>("github.auth.status", {});
      return {
        gh_present: res.gh_present ?? false,
        authenticated: res.authenticated ?? false,
        user: res.user ?? null,
        host: res.host ?? null,
        scopes: res.scopes ?? [],
        setup_hint: res.setup_hint ?? null,
      };
    } catch (e) {
      // A daemon without the gh transport (pre-M5) answers unknown-verb: report gh as
      // unavailable with the honest setup pointer, so PR mode degrades to local-only.
      return {
        gh_present: false,
        authenticated: false,
        setup_hint: `GitHub features need the gh CLI. Install it and run 'gh auth login'. (${errorMessage(e)})`,
      };
    }
  }

  async getGithubImportConfig(projectId: string): Promise<GithubImportConfig> {
    // INTERPRET: the import filters live on the project row (project.update github block).
    // Until the daemon serves that field, read a safe default (phase12-m5ui.md).
    try {
      const res = await this.client.call<{ github_import?: GithubImportConfig }>("project.get", {
        project_id: projectId,
      });
      const g = res.github_import;
      if (g) return { assignees: g.assignees ?? [], labels: g.labels ?? [], milestone: g.milestone ?? null, state: g.state ?? "open" };
    } catch {
      /* fall through to the default */
    }
    return { assignees: [], labels: [], milestone: null, state: "open" };
  }

  async setGithubImportConfig(projectId: string, config: GithubImportConfig): Promise<GithubImportConfig> {
    try {
      await this.client.call("project.update", { project_id: projectId, github_import: config });
    } catch {
      /* the daemon may not persist this yet; the UI keeps it for the session */
    }
    return config;
  }

  async previewGithubIssues(projectId: string): Promise<GithubIssue[]> {
    // github.issues.preview { project_id } -> the filtered list without importing.
    try {
      const res = await this.client.call<{ issues?: GithubIssue[] }>("github.issues.preview", {
        project_id: projectId,
      });
      return res.issues ?? [];
    } catch {
      return [];
    }
  }

  async importGithubIssues(projectId: string, numbers: number[]): Promise<GithubImportResult> {
    // github.issues.import { project_id, numbers? } -> creates/refreshes origin cards.
    try {
      const res = await this.client.call<Partial<GithubImportResult>>("github.issues.import", {
        project_id: projectId,
        numbers: numbers.length > 0 ? numbers : undefined,
      });
      return {
        ok: res.ok ?? true,
        imported: res.imported ?? 0,
        refreshed: res.refreshed ?? 0,
        card_ids: res.card_ids ?? [],
      };
    } catch (e) {
      return { ok: false, imported: 0, refreshed: 0, card_ids: [], error: errorMessage(e) };
    }
  }

  async getGithubIssueForCard(card: Card): Promise<GithubIssue | null> {
    if (card.origin_kind !== "github_issue" || !card.origin_ref) return null;
    // INTERPRET: github.issues.get { origin_ref } once the daemon caches issue bodies;
    // absent verb degrades to no rendered issue (the Issue tab shows its link-out).
    try {
      const res = await this.client.call<{ issue?: GithubIssue }>("github.issues.get", {
        origin_ref: card.origin_ref,
      });
      return res.issue ?? null;
    } catch {
      return null;
    }
  }

  // --- Verification gate (gate.md) -------------------------------------------
  // The gate engine lands in the M5 core lane; until it serves gate runs, live mode
  // returns null (the Verify tab shows its empty state) and mutations reject honestly.

  async getGateRun(cardId: string): Promise<GateRun | null> {
    // INTERPRET: card.get { card_id } -> { gate_run?: GateRun } once the gate engine lands.
    try {
      const res = await this.client.call<{ gate_run?: GateRun | null; gate_runs?: GateRun[] }>("card.get", {
        card_id: cardId,
      });
      if (res.gate_run) return res.gate_run;
      const list = res.gate_runs ?? [];
      return list.sort((a, b) => b.updated_at - a.updated_at)[0] ?? null;
    } catch {
      return null;
    }
  }

  async startGate(cardId: string): Promise<{ ok: boolean; error?: string }> {
    // gate.run { card_id } (gate.md: triggered when the user clicks Verify).
    try {
      await this.client.call("gate.run", { card_id: cardId });
      return { ok: true };
    } catch (e) {
      return { ok: false, error: errorMessage(e) };
    }
  }

  async resolveGateFinding(cardId: string, findingId: string, resolution: FindingResolution): Promise<GateRun> {
    // gate.resolve_finding { finding_id, resolution } (gate.md step 4; the findings review
    // renders in Plan Studio chrome). No card_id in the payload. Re-reads the run after.
    await this.client.call("gate.resolve_finding", {
      finding_id: findingId,
      resolution,
    });
    const run = await this.getGateRun(cardId);
    if (!run) throw new Error("gate run unavailable after resolve");
    return run;
  }

  async mergePr(cardId: string): Promise<{ ok: boolean; error?: string }> {
    // INTERPRET: gate.merge / github.pr.merge { card_id } (gate.md ship: squash default).
    try {
      await this.client.call("gate.merge", { card_id: cardId });
      return { ok: true };
    } catch (e) {
      return { ok: false, error: errorMessage(e) };
    }
  }

  // --- Remote access / device pairing (M6; security.md) ----------------------
  // The opt-in LAN listener does not exist yet. Live mode reports it disabled and
  // unavailable (the pairing screen marks the integration seam); nothing is fabricated.

  private disabledRemote(): RemoteListenerState {
    const profile: RemoteCapabilityProfile = {
      needs_you: true,
      approvals: true,
      steering: true,
      terminals_read_only: true,
      vault_access: false,
      recipe_install: false,
    };
    return {
      enabled: false,
      lan_ip: null,
      port: null,
      url: null,
      pairing_payload: null,
      token: null,
      minted_at: null,
      profile,
      devices: [],
    };
  }

  async getRemoteState(): Promise<RemoteListenerState> {
    try {
      const res = await this.client.call<RemoteListenerState>("remote.status", {});
      return res;
    } catch {
      return this.disabledRemote();
    }
  }

  async setRemoteEnabled(enabled: boolean): Promise<RemoteListenerState> {
    try {
      return await this.client.call<RemoteListenerState>("remote.listener", { enabled });
    } catch {
      return this.disabledRemote();
    }
  }

  async rotateRemoteToken(): Promise<RemoteListenerState> {
    try {
      return await this.client.call<RemoteListenerState>("remote.rotate", {});
    } catch {
      return this.disabledRemote();
    }
  }

  async revokeRemoteDevice(deviceId: string): Promise<RemoteListenerState> {
    try {
      return await this.client.call<RemoteListenerState>("remote.revoke", { device_id: deviceId });
    } catch {
      return this.disabledRemote();
    }
  }

  subscribeEvents(handler: (event: CardEvent) => void): () => void {
    const prev = this.client.onEvent;
    this.client.onEvent = (env: Envelope) => {
      prev?.(env);
      // INTERPRET: event.subscribe streams card_events; the event payload IS the
      // card_event row. Unknown kinds pass through untouched.
      const ev = env.payload as CardEvent | undefined;
      if (ev && ev.card_id && ev.kind) handler(ev);
    };
    // Ask the daemon to start streaming from the live head.
    this.client.call("event.subscribe", {}).catch(() => undefined);
    return () => {
      this.client.onEvent = prev;
    };
  }
}

function errorMessage(e: unknown): string {
  if (e && typeof e === "object" && "message" in e) return String((e as { message: unknown }).message);
  return String(e);
}
