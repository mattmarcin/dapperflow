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
  CardType,
  DispatchStartInput,
  FindingResolution,
  GateFinding,
  GateRun,
  GateStatus,
  GithubAuthStatus,
  GithubImportConfig,
  GithubImportResult,
  GithubIssue,
  GithubLabel,
  GateMode,
  Lane,
  NeedsYouItem,
  PairedDevice,
  PlanMode,
  PrivilegeKind,
  Project,
  ProjectAddResult,
  Recipe,
  RecipePrivilege,
  RecipeScope,
  RemoteCapabilityProfile,
  RemoteListenerState,
  Session,
  SessionResumeResult,
  ShipTarget,
  StageLine,
  TrustTier,
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
    // SessionSummary carries no `resumed_from` (that field lives on SessionResumed, the
    // session.resume response). The fleet row never sends it, so it is always null here.
    resumed_from: null,
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
    // NeedsYouItem has no `note` field on the wire; leave it null rather than reading a
    // key the daemon never sends.
    note: null,
  };
}

export class LiveDataSource implements DataSource {
  readonly mode = "live" as const;

  // Per-project GitHub import filters. The daemon has no field or verb to persist these
  // (project.get is not routed, and `github_import` is not a ProjectUpdate field), so
  // they are held client-side for the app session. Deferred until a daemon field exists.
  private readonly githubImportConfigs = new Map<string, GithubImportConfig>();

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
      // session.resume relaunches the harness in the same worktree; the daemon returns
      // SessionResumed { session_id, resumed_from, resume_ref? } - the NEW session id is
      // `session_id` (not `new_session_id`, which the spec wrongly named and the UI
      // followed). resumed_from/resume_ref are lineage the daemon records.
      const res = await this.client.call<{
        session_id: string;
        resumed_from?: string;
        resume_ref?: string;
      }>("session.resume", {
        session_id: sessionId,
      });
      return { ok: true, new_session_id: res.session_id };
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
    // recipe.list {} -> bundled + user + project recipes with source + trust tier.
    // The daemon answers with dflow-proto RecipeSummary rows (name, scope, version,
    // description, trust_tier, source_path, elevations), NOT the desktop's richer dial
    // view-model. Passing those raw rows straight to the dial was the crash: RecipeRow
    // reads recipe.trust and recipe.stageLines, which are undefined on a summary, so the
    // render threw and (with no error boundary) blanked the app. Normalize every row into
    // the Recipe shape here. If the verb is unanswered or empty, fall back to the bundled
    // fixture catalog so the dial stays usable.
    try {
      const res = await this.client.call<{ recipes?: unknown[] }>("recipe.list", {});
      if (Array.isArray(res.recipes) && res.recipes.length > 0) {
        return res.recipes
          .filter((r): r is Record<string, unknown> => !!r && typeof r === "object")
          .map(normalizeRecipe);
      }
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
    // Approval is a first-class feedback action (plan-studio.md records plan_approved),
    // modeled as an `{ kind:"action", action:"approve_plan" }` FeedbackItem. FeedbackSubmit
    // has no `card_id` field (the artifact_id resolves the card), so it is not sent.
    void cardId;
    await this.client.call("artifact.feedback.submit", {
      artifact_id: artifactId,
      items: [{ kind: "action", action: "approve_plan" }],
    });
  }

  // --- Onboarding audit ------------------------------------------------------

  async startAudit(projectId: string, depth: AuditDepth): Promise<AuditStartResult> {
    // The audit is a dispatch of the bundled audit / audit-deep recipe (product.md: the
    // recipe has no ship stage, structurally). DispatchStart requires a `card_id` and has
    // NO project-level form - there is no card-less dispatch verb. So the audit runs as a
    // two-step, entirely frontend-side with existing verbs: create an anchor investigation
    // card for the run, then dispatch.start against it with `audit: true` (which mints an
    // audit-scoped token so the cards it files land in Inbox and it may not move lanes).
    const recipe = depth === "deep" ? "audit-deep" : "audit";
    try {
      const created = await this.client.call<{ card_id?: string; card?: Card }>("card.create", {
        title: depth === "deep" ? "Onboarding audit (deep)" : "Onboarding audit",
        type: "investigation",
        project_id: projectId,
        dial_recipe: recipe,
      });
      const cardId = created.card_id ?? created.card?.id;
      if (!cardId) return { ok: false, error: "card.create returned no card id for the audit" };
      const res = await this.client.call<{ session_id?: string }>("dispatch.start", {
        card_id: cardId,
        recipe,
        audit: true,
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
      // GithubAuthResult { present, authenticated, account?, host?, repo? }. The UI's
      // view-model names differ (gh_present/user), and `scopes`/`setup_hint` are not on
      // the wire at all, so map present->gh_present, account->user, and compute the setup
      // hint locally from presence/auth rather than reading absent keys.
      const res = await this.client.call<{
        present?: boolean;
        authenticated?: boolean;
        account?: string | null;
        host?: string | null;
        repo?: string | null;
      }>("github.auth.status", {});
      const present = res.present ?? false;
      const authenticated = res.authenticated ?? false;
      return {
        gh_present: present,
        authenticated,
        user: res.account ?? null,
        host: res.host ?? null,
        scopes: [],
        setup_hint: authenticated
          ? null
          : present
            ? "Run 'gh auth login' to enable GitHub features. Without it, PR mode degrades cleanly to local-only."
            : "GitHub features need the gh CLI. Install it and run 'gh auth login'.",
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
    // Deferred: `project.get` is not routed by the daemon, and the Project entity has no
    // import-config field, so there is nothing to read server-side. The filters are held
    // client-side (see `githubImportConfigs`) until a daemon field exists.
    return (
      this.githubImportConfigs.get(projectId) ?? {
        assignees: [],
        labels: [],
        milestone: null,
        state: "open",
      }
    );
  }

  async setGithubImportConfig(projectId: string, config: GithubImportConfig): Promise<GithubImportConfig> {
    // Deferred: `github_import` is not a `ProjectUpdate` field and there is no dedicated
    // verb, so persisting it server-side is impossible today. Keep it client-side for the
    // session rather than firing a project.update the daemon would silently drop.
    this.githubImportConfigs.set(projectId, config);
    return config;
  }

  async previewGithubIssues(projectId: string): Promise<GithubIssue[]> {
    // github.issues.preview { project_id, filter } -> GithubIssuesPreviewResult
    // { repo, issues: GithubIssuePreview[] }. Each preview row is { number, title,
    // labels: string[], state, url, dedupe, existing_card_id? } - not a full GithubIssue -
    // so map it into the view-model and surface dedupe/existing_card_id as the import badge.
    try {
      const res = await this.client.call<{ repo?: string; issues?: WireGithubIssuePreview[] }>(
        "github.issues.preview",
        { project_id: projectId, filter: filterFromConfig(this.githubImportConfigs.get(projectId)) },
      );
      const repo = res.repo ?? "";
      return (res.issues ?? []).map((row) => githubIssueFromPreview(row, repo));
    } catch {
      return [];
    }
  }

  async importGithubIssues(projectId: string, numbers: number[]): Promise<GithubImportResult> {
    // github.issues.import { project_id, filter: { numbers? }, dial_recipe? } -> creates/
    // refreshes origin cards. The selection nests under `filter.numbers` (a top-level
    // `numbers` is dropped, which would import the whole default filter). The response is
    // GithubIssuesImportResult { repo, results: [{ number, title, card_id, outcome }] };
    // fold each outcome (created/refreshed/suppressed) into the view-model's counts.
    try {
      const filter =
        numbers.length > 0 ? { numbers } : filterFromConfig(this.githubImportConfigs.get(projectId));
      const res = await this.client.call<{ repo?: string; results?: WireGithubImportRow[] }>(
        "github.issues.import",
        { project_id: projectId, filter },
      );
      const results = res.results ?? [];
      return {
        ok: true,
        imported: results.filter((r) => r.outcome === "created").length,
        refreshed: results.filter((r) => r.outcome === "refreshed").length,
        card_ids: results.map((r) => r.card_id),
      };
    } catch (e) {
      return { ok: false, imported: 0, refreshed: 0, card_ids: [], error: errorMessage(e) };
    }
  }

  async getGithubIssueForCard(card: Card): Promise<GithubIssue | null> {
    if (card.origin_kind !== "github_issue" || !card.origin_ref) return null;
    // github.issue.get { card_id } (singular verb) -> { issue: GithubIssueInfo }. The old
    // code called the non-routed plural `github.issues.get` with `origin_ref`; the daemon
    // resolves the issue from the card id instead.
    try {
      const res = await this.client.call<{ issue?: WireGithubIssueInfo }>("github.issue.get", {
        card_id: card.id,
      });
      return res.issue ? githubIssueFromInfo(res.issue, card) : null;
    } catch {
      return null;
    }
  }

  // --- Verification gate (gate.md) -------------------------------------------
  // The gate engine lands in the M5 core lane; until it serves gate runs, live mode
  // returns null (the Verify tab shows its empty state) and mutations reject honestly.

  async getGateRun(cardId: string): Promise<GateRun | null> {
    // gate.status { card_id } -> { run?: GateRunInfo, findings: FindingInfo[] }. `card.get`
    // carries no gate fields (card/sessions/events/artifacts only), which is why the old
    // read always fell to null. GateRunInfo uses `step`/`gate_strictness` (not the UI's
    // mode/checks/pr), so map it into the view-model; no run means "not verified yet".
    try {
      const res = await this.client.call<{ run?: WireGateRunInfo | null; findings?: WireFindingInfo[] }>(
        "gate.status",
        { card_id: cardId },
      );
      if (!res.run) return null;
      return gateRunFromWire(res.run, res.findings ?? []);
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
    // gate.resolve_finding { finding_id, resolution } (gate.md step 4). The daemon's
    // resolution vocabulary is accepted|fixed|skipped, not the UI's approve|fix|skip, so
    // translate before sending. Then re-read via gate.status (getGateRun), NOT card.get,
    // which has no gate fields - reading it there is why this used to throw after a
    // successful resolve.
    await this.client.call("gate.resolve_finding", {
      finding_id: findingId,
      resolution: resolutionToDaemon(resolution),
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
    return {
      enabled: false,
      lan_ip: null,
      port: null,
      url: null,
      pairing_payload: null,
      token: null,
      minted_at: null,
      profile: phoneProfile(),
      devices: [],
    };
  }

  async getRemoteState(): Promise<RemoteListenerState> {
    // daemon.lan.status {} -> LanState { enabled, bound, port, lan_urls, caveat, phones }.
    // The old `remote.status` verb is not routed. A plain status carries no fresh pairing
    // payload (that is minted only by daemon.lan.pair), so the QR appears after enabling
    // or rotating, not on a bare read.
    try {
      const res = await this.client.call<WireLanState>("daemon.lan.status", {});
      return lanStateToRemote(res);
    } catch {
      return this.disabledRemote();
    }
  }

  async setRemoteEnabled(enabled: boolean): Promise<RemoteListenerState> {
    // daemon.lan.enable { port? } / daemon.lan.disable {} (the old `remote.listener` verb
    // is not routed). Enabling also mints one pairing (daemon.lan.pair) so the Settings
    // screen has a QR to show immediately; disabling just stops the listener.
    try {
      if (!enabled) {
        await this.client.call("daemon.lan.disable", {});
        return this.disabledRemote();
      }
      const state = await this.client.call<WireLanState>("daemon.lan.enable", {});
      const pairing = await this.client
        .call<WireLanPairing>("daemon.lan.pair", {})
        .catch(() => undefined);
      return lanStateToRemote(state, pairing);
    } catch {
      return this.disabledRemote();
    }
  }

  async rotateRemoteToken(): Promise<RemoteListenerState> {
    // No `remote.rotate` verb exists; pairing is daemon.lan.pair { name? }, which mints a
    // fresh phone token + QR. Re-read the listener status for the current device list.
    try {
      const pairing = await this.client.call<WireLanPairing>("daemon.lan.pair", {});
      const state = await this.client.call<WireLanState>("daemon.lan.status", {});
      return lanStateToRemote(state, pairing);
    } catch {
      return this.disabledRemote();
    }
  }

  async revokeRemoteDevice(deviceId: string): Promise<RemoteListenerState> {
    // daemon.lan.revoke { token_id } (not `remote.revoke`/`device_id`). The device id IS
    // the pairing token_id (PhonePairing.id). Re-read the listener status afterward.
    try {
      await this.client.call("daemon.lan.revoke", { token_id: deviceId });
      const state = await this.client.call<WireLanState>("daemon.lan.status", {});
      return lanStateToRemote(state);
    } catch {
      return this.disabledRemote();
    }
  }

  subscribeEvents(handler: (event: CardEvent) => void): () => void {
    const prev = this.client.onEvent;
    this.client.onEvent = (env: Envelope) => {
      prev?.(env);
      // event.subscribe delivers Envelope::event("event.card_event", EventCardEvent
      // { event }), so the card_event row is NESTED under `payload.event`, not the flat
      // payload. Reading it flat left card_id/kind undefined and the handler never fired
      // (live board/queue updates were silently dead). Unknown kinds pass through.
      const ev = (env.payload as { event?: CardEvent } | undefined)?.event;
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

// ---------------------------------------------------------------------------
// Wire shapes + mappers. These mirror the daemon's dflow-proto structs exactly and map
// them into the desktop's view-models, so the UI components and fixtures keep their
// existing contracts while the live source reads what the daemon actually sends.
// ---------------------------------------------------------------------------

// github.issues.preview row (dflow-proto GithubIssuePreview).
interface WireGithubIssuePreview {
  number: number;
  title: string;
  labels?: string[];
  state?: string;
  url?: string;
  dedupe?: string;
  existing_card_id?: string | null;
}

// github.issues.import outcome row (dflow-proto GithubImportResult).
interface WireGithubImportRow {
  number: number;
  title: string;
  card_id: string;
  outcome: string; // created | refreshed | suppressed
}

// github.issue.get issue snapshot (dflow-proto GithubIssueInfo).
interface WireGithubIssueInfo {
  number: number;
  repo?: string;
  title: string;
  body?: string;
  labels?: string[];
  assignees?: string[];
  milestone?: string | null;
  state?: string;
  url?: string;
}

// gate.status run (dflow-proto GateRunInfo).
interface WireGateRunInfo {
  id: string;
  card_id: string;
  worktree_id?: string | null;
  step: string; // checks | review | autofix | escalate | push | pr | ci | done
  status: string; // running | passed | failed | escalated
  gate_strictness?: string | null;
  author_harness?: string | null;
  reviewer_harness?: string | null;
  head_sha?: string | null;
  branch?: string | null;
  pr_number?: number | null;
  pr_url?: string | null;
  started_at?: number | null;
  ended_at?: number | null;
}

// gate.status finding (dflow-proto FindingInfo).
interface WireFindingInfo {
  id: string;
  gate_run_id: string;
  card_id: string;
  severity: string; // blocker | major | minor
  category: string; // mechanical | intent
  source: string;
  body: string;
  evidence?: string | null;
  resolution?: string | null; // autofixed | accepted | fixed | skipped
  created_at?: number | null;
  resolved_at?: number | null;
}

// daemon.lan.* shapes (dflow-proto LanState / LanPairing / PhonePairing).
interface WirePairingPayload {
  url: string;
  token: string;
  name?: string | null;
}
interface WireLanPairing {
  token_id: string;
  pair_url: string;
  payload: WirePairingPayload;
}
interface WirePhonePairing {
  id: string;
  name?: string | null;
  created_at: number;
  last_seen_at?: number | null;
}
interface WireLanState {
  enabled: boolean;
  bound: boolean;
  port: number;
  lan_urls?: string[];
  caveat: string;
  phones?: WirePhonePairing[];
}

// Build the daemon's GithubIssueFilter from the client-side import config. An empty config
// yields an empty filter, which the daemon reads as "every open issue" (the curated picker).
function filterFromConfig(config: GithubImportConfig | undefined): {
  assignee?: string;
  labels?: string[];
  milestone?: string;
  state?: string;
} {
  if (!config) return {};
  const filter: { assignee?: string; labels?: string[]; milestone?: string; state?: string } = {};
  if (config.assignees.length > 0) filter.assignee = config.assignees[0];
  if (config.labels.length > 0) filter.labels = config.labels;
  if (config.milestone) filter.milestone = config.milestone;
  if (config.state) filter.state = config.state;
  return filter;
}

// Label-heuristic card type (the daemon assigns the real one on import; this is only for
// the preview badge). Falls back to feature.
function suggestedTypeFromLabels(labels: string[]): CardType {
  const set = labels.map((l) => l.toLowerCase());
  if (set.some((l) => l.includes("bug") || l.includes("defect"))) return "bug";
  if (set.some((l) => l.includes("chore") || l.includes("maintenance"))) return "chore";
  if (set.some((l) => l.includes("test"))) return "test";
  if (set.some((l) => l.includes("investigat") || l.includes("spike") || l.includes("question")))
    return "investigation";
  return "feature";
}

// Map a preview row into the view-model. The preview carries only number/title/labels/
// state/url plus dedupe; the richer fields (body/author/comments) are not on the wire, so
// they degrade to empty and the imported badge comes from existing_card_id.
function githubIssueFromPreview(row: WireGithubIssuePreview, repo: string): GithubIssue {
  const labels: GithubLabel[] = (row.labels ?? []).map((name) => ({ name }));
  return {
    number: row.number,
    title: row.title,
    body: "",
    state: row.state === "closed" ? "closed" : "open",
    author: "",
    assignees: [],
    labels,
    milestone: null,
    comments: [],
    url: row.url ?? "",
    repo,
    updated_at: 0,
    imported_card_id: row.existing_card_id ?? null,
    suggested_type: suggestedTypeFromLabels(row.labels ?? []),
  };
}

// Map github.issue.get's GithubIssueInfo into the view-model. The snapshot has no author
// or comments and no timestamp, so those degrade (updated_at borrows the card's).
function githubIssueFromInfo(info: WireGithubIssueInfo, card: Card): GithubIssue {
  const labels: GithubLabel[] = (info.labels ?? []).map((name) => ({ name }));
  return {
    number: info.number,
    title: info.title,
    body: info.body ?? "",
    state: info.state === "closed" ? "closed" : "open",
    author: "",
    assignees: info.assignees ?? [],
    labels,
    milestone: info.milestone ?? null,
    comments: [],
    url: info.url ?? "",
    repo: info.repo ?? "",
    updated_at: card.updated_at,
    imported_card_id: card.id,
    suggested_type: suggestedTypeFromLabels(info.labels ?? []),
  };
}

// The UI's FindingResolution vocabulary (approve|fix|skip) vs the daemon's
// (accepted|fixed|skipped, plus autofixed for mechanical auto-applies).
function resolutionToDaemon(r: FindingResolution): string {
  switch (r) {
    case "approve":
      return "accepted";
    case "fix":
      return "fixed";
    case "skip":
      return "skipped";
    default:
      return "accepted";
  }
}
function resolutionFromDaemon(s: string | null | undefined): FindingResolution | null {
  switch (s) {
    case "accepted":
      return "approve";
    case "fixed":
    case "autofixed":
      return "fix";
    case "skipped":
      return "skip";
    default:
      return null;
  }
}

// Map GateRunInfo.step/status onto the UI's flat GateStatus. The UI also computes
// awaiting-human from open intent findings, so the exact running-step mapping is advisory.
function gateStatusFromWire(run: WireGateRunInfo): GateStatus {
  if (run.status === "passed") return "passed";
  if (run.status === "failed") return "failed";
  if (run.status === "escalated") return "awaiting_human";
  switch (run.step) {
    case "checks":
      return "checks_running";
    case "review":
      return "review";
    case "autofix":
      return "autofixing";
    case "escalate":
      return "awaiting_human";
    case "push":
    case "pr":
    case "ci":
    case "done":
      return "passed";
    default:
      return "pending";
  }
}

function gateFindingFromWire(f: WireFindingInfo): GateFinding {
  return {
    id: f.id,
    severity: (f.severity as GateFinding["severity"]) ?? "minor",
    title: f.body,
    scenario: f.evidence ?? "",
    rule: null,
    file: null,
    line: null,
    klass: f.category === "mechanical" ? "mechanical" : "intent",
    resolution: resolutionFromDaemon(f.resolution),
    auto_applied: f.resolution === "autofixed",
  };
}

// Map GateRunInfo + findings into the view-model. GateRunInfo has no per-check rows (checks
// stream as card events) and no rich PR sub-state, so `checks` is empty and `pr` carries
// only what the run row exposes (number/url/branch); `mergeable` stays false (honest: the
// UI's disabled-until-green rule needs real CI, which the run row does not carry).
function gateRunFromWire(run: WireGateRunInfo, findings: WireFindingInfo[]): GateRun {
  const mode: GateMode =
    run.gate_strictness === "checks_only" ? "checks_only" : run.gate_strictness === "none" ? "none" : "full";
  return {
    id: run.id,
    card_id: run.card_id,
    status: gateStatusFromWire(run),
    mode,
    worktree_id: run.worktree_id ?? "",
    reviewer_harness: run.reviewer_harness ?? null,
    started_at: run.started_at ?? 0,
    updated_at: run.ended_at ?? run.started_at ?? 0,
    checks: [],
    findings: findings.map(gateFindingFromWire),
    findings_doc_id: null,
    pr: {
      status: run.pr_number != null ? "open" : "none",
      number: run.pr_number ?? null,
      url: run.pr_url ?? null,
      branch: run.branch ?? "",
      ci: [],
      mergeable: false,
      merge_method: "squash",
      fixes_issue: null,
    },
  };
}

// The fixed phone capability profile (security.md): Needs You, approvals, steering, and
// read-only terminals; no vault, no recipe install.
function phoneProfile(): RemoteCapabilityProfile {
  return {
    needs_you: true,
    approvals: true,
    steering: true,
    terminals_read_only: true,
    vault_access: false,
    recipe_install: false,
  };
}

function phoneToDevice(p: WirePhonePairing): PairedDevice {
  return {
    id: p.id, // the pairing token_id, which is also the daemon.lan.revoke target
    name: p.name ?? "Phone",
    profile: "phone",
    paired_at: p.created_at,
    last_seen: p.last_seen_at ?? null,
    capabilities: phoneProfile(),
  };
}

// Map LanState (+ an optional fresh pairing) into the desktop's RemoteListenerState. A
// pairing is present only right after enable/pair, which is exactly when the QR is shown.
function lanStateToRemote(s: WireLanState, pairing?: WireLanPairing): RemoteListenerState {
  return {
    enabled: !!s.enabled,
    lan_ip: null,
    port: s.port || null,
    url: (s.lan_urls ?? [])[0] ?? null,
    pairing_payload: pairing?.pair_url ?? null,
    token: pairing?.payload?.token ?? null,
    minted_at: pairing ? Date.now() : null,
    profile: phoneProfile(),
    devices: (s.phones ?? []).map(phoneToDevice),
  };
}

// Best-effort human capability kind from a daemon elevation one-liner. The verbatim
// string is carried through as the privilege detail regardless; the kind only picks the
// consent-summary label, and the dial renders defensively if it lands off the map.
function guessPrivilegeKind(detail: string): PrivilegeKind {
  const d = detail.toLowerCase();
  if (d.includes("mcp")) return "mcp";
  if (d.includes("gate")) return "gate_disabled";
  if (d.includes("in place") || d.includes("in-place") || d.includes("checkout") || d.includes("working tree"))
    return "worktree_in_place";
  if (d.includes("merge") || d.includes("local branch") || d.includes("no pr")) return "local_merge";
  return "mcp";
}

// Map one recipe.list row (a dflow-proto RecipeSummary, or a richer future shape) into
// the desktop's Recipe dial view-model, filling every field the dial reads with a safe
// value. Tolerant of both the summary shape (trust_tier / elevations / source_path) and
// an already-normalized shape (trust / privileges / stageLines), so a daemon upgrade that
// serves the full object still works.
function normalizeRecipe(raw: Record<string, unknown>): Recipe {
  const name = typeof raw.name === "string" && raw.name.trim() ? raw.name : "recipe";
  const trustRaw = raw.trust ?? raw.trust_tier;
  const trust: TrustTier = trustRaw === "privileged" ? "privileged" : "standard";
  const scopeRaw = raw.scope;
  const scope: RecipeScope = scopeRaw === "user" || scopeRaw === "project" ? scopeRaw : "bundled";
  const elevations = Array.isArray(raw.elevations)
    ? (raw.elevations as unknown[]).filter((e): e is string => typeof e === "string")
    : [];
  const privileges: RecipePrivilege[] = Array.isArray(raw.privileges)
    ? (raw.privileges as unknown[])
        .filter((p): p is Record<string, unknown> => !!p && typeof p === "object")
        .map((p) => ({
          kind: (p.kind as PrivilegeKind) ?? guessPrivilegeKind(String(p.detail ?? "")),
          detail: typeof p.detail === "string" ? p.detail : String(p.detail ?? ""),
        }))
    : elevations.map((detail) => ({ kind: guessPrivilegeKind(detail), detail }));
  const stageLines: StageLine[] = Array.isArray(raw.stageLines)
    ? (raw.stageLines as unknown[])
        .filter((s): s is Record<string, unknown> => !!s && typeof s === "object")
        .map((s) => ({ stage: String(s.stage ?? ""), note: typeof s.note === "string" ? s.note : "" }))
    : [];
  const stages = Array.isArray(raw.stages)
    ? (raw.stages as unknown[]).map((s) => String(s))
    : stageLines.map((s) => s.stage);
  const source =
    typeof raw.source === "string"
      ? raw.source
      : typeof raw.source_path === "string"
        ? raw.source_path
        : scope === "bundled"
          ? "bundled"
          : scope;
  return {
    name,
    version: typeof raw.version === "number" ? raw.version : 1,
    description: typeof raw.description === "string" ? raw.description : "",
    scope,
    source,
    stages,
    stageLines,
    planMode: (raw.planMode as PlanMode) ?? "none",
    approval: raw.approval === "required" ? "required" : "auto",
    gate: (raw.gate as GateMode) ?? "full",
    shipTarget: (raw.shipTarget as ShipTarget) ?? "pr",
    trust,
    privileges,
    contentHash: typeof raw.contentHash === "string" ? raw.contentHash : "",
    investigation:
      typeof raw.investigation === "boolean" ? raw.investigation : /^audit(-deep)?$/.test(name),
  };
}
