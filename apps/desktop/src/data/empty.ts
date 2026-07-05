// EmptyDataSource: the honest "daemon unavailable" board. Used when the app is a real
// (non-dev-fixture) build and the daemon could not be reached at boot. It NEVER fabricates
// data - the board is genuinely empty and every mutation rejects with a clear "daemon
// unavailable" message - so a down daemon can never masquerade as a live fleet (which the
// old FixtureDataSource fallback did silently). Dev fixtures remain a separate, explicitly
// flagged mode (VITE_DFLOW_FIXTURES=1 -> FixtureDataSource).
//
// Its `mode` is "live": this is the real client path, only disconnected, so the UI's
// fixture/demo indicator (which keys off the source mode) correctly does NOT light up. The
// daemon-offline banner communicates the degraded state instead.

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
import { DataSource } from "./source";

const UNAVAILABLE = "Daemon unavailable - start the daemon to connect.";

function unavailable(): Promise<never> {
  return Promise.reject(new Error(UNAVAILABLE));
}

function disabledRemote(): RemoteListenerState {
  return {
    enabled: false,
    lan_ip: null,
    port: null,
    url: null,
    pairing_payload: null,
    token: null,
    minted_at: null,
    profile: {
      needs_you: true,
      approvals: true,
      steering: true,
      terminals_read_only: true,
      vault_access: false,
      recipe_install: false,
    },
    devices: [],
  };
}

export class EmptyDataSource implements DataSource {
  // Not fixtures: this is the real path with no daemon, so the fixture indicator stays off
  // and the daemon-offline banner tells the honest story.
  readonly mode = "live" as const;

  async loadSnapshot(): Promise<BoardSnapshot> {
    return { projects: [], cards: [], sessions: [], needsYou: [] };
  }

  async resumeSession(_sessionId: string): Promise<SessionResumeResult> {
    void _sessionId;
    return { ok: false, error: UNAVAILABLE };
  }

  async addProject(_path: string): Promise<ProjectAddResult> {
    void _path;
    return { ok: false, error: UNAVAILABLE };
  }

  createCard(_input: CardCreateInput): Promise<Card> {
    void _input;
    return unavailable();
  }

  moveCard(_cardId: string, _lane: Lane): Promise<Card> {
    void _cardId;
    void _lane;
    return unavailable();
  }

  updateCardDial(_cardId: string, _recipe: string | null): Promise<Card> {
    void _cardId;
    void _recipe;
    return unavailable();
  }

  dispatch(_input: DispatchStartInput): Promise<{ session_id?: string }> {
    void _input;
    return unavailable();
  }

  cancelDispatch(_cardId: string): Promise<void> {
    void _cardId;
    return unavailable();
  }

  async cardEvents(_cardId: string): Promise<CardEvent[]> {
    void _cardId;
    return [];
  }

  async recentActivity(_limit?: number): Promise<CardEvent[]> {
    void _limit;
    return [];
  }

  renameSession(_sessionId: string, _title: string): Promise<void> {
    void _sessionId;
    void _title;
    return unavailable();
  }

  async listAgents(): Promise<Agent[]> {
    return [];
  }

  async detectAgents(): Promise<AgentsDetectResult> {
    return { found: [], agents: [] };
  }

  async addAgent(_input: AgentAddInput): Promise<AgentMutationResult> {
    void _input;
    return { ok: false, error: UNAVAILABLE };
  }

  async updateAgent(_input: AgentUpdateInput): Promise<AgentMutationResult> {
    void _input;
    return { ok: false, error: UNAVAILABLE };
  }

  async removeAgent(_id: string): Promise<AgentRemoveResult> {
    void _id;
    return { ok: false, error: UNAVAILABLE };
  }

  subscribeEvents(_handler: (event: CardEvent) => void): () => void {
    void _handler;
    return () => undefined;
  }

  async listRecipes(): Promise<Recipe[]> {
    return [];
  }

  async getPlanArtifact(_cardId: string): Promise<ArtifactMeta | null> {
    void _cardId;
    return null;
  }

  signArtifactUrl(_docId: string): Promise<string> {
    void _docId;
    return unavailable();
  }

  submitFeedback(_input: FeedbackSubmit): Promise<FeedbackSubmitResult> {
    void _input;
    return unavailable();
  }

  approvePlan(_artifactId: string, _cardId: string): Promise<void> {
    void _artifactId;
    void _cardId;
    return unavailable();
  }

  async startAudit(_projectId: string, _depth: AuditDepth): Promise<AuditStartResult> {
    void _projectId;
    void _depth;
    return { ok: false, error: UNAVAILABLE };
  }

  async githubAuthStatus(): Promise<GithubAuthStatus> {
    return {
      gh_present: false,
      authenticated: false,
      user: null,
      host: null,
      scopes: [],
      setup_hint: UNAVAILABLE,
    };
  }

  async getGithubImportConfig(_projectId: string): Promise<GithubImportConfig> {
    void _projectId;
    return { assignees: [], labels: [], milestone: null, state: "open" };
  }

  async setGithubImportConfig(_projectId: string, config: GithubImportConfig): Promise<GithubImportConfig> {
    void _projectId;
    return config;
  }

  async previewGithubIssues(_projectId: string): Promise<GithubIssue[]> {
    void _projectId;
    return [];
  }

  async importGithubIssues(_projectId: string, _numbers: number[]): Promise<GithubImportResult> {
    void _projectId;
    void _numbers;
    return { ok: false, imported: 0, refreshed: 0, card_ids: [], error: UNAVAILABLE };
  }

  async getGithubIssueForCard(_card: Card): Promise<GithubIssue | null> {
    void _card;
    return null;
  }

  async getGateRun(_cardId: string): Promise<GateRun | null> {
    void _cardId;
    return null;
  }

  async startGate(_cardId: string): Promise<{ ok: boolean; error?: string }> {
    void _cardId;
    return { ok: false, error: UNAVAILABLE };
  }

  resolveGateFinding(_cardId: string, _findingId: string, _resolution: FindingResolution): Promise<GateRun> {
    void _cardId;
    void _findingId;
    void _resolution;
    return unavailable();
  }

  async mergePr(_cardId: string): Promise<{ ok: boolean; error?: string }> {
    void _cardId;
    return { ok: false, error: UNAVAILABLE };
  }

  async getRemoteState(): Promise<RemoteListenerState> {
    return disabledRemote();
  }

  async setRemoteEnabled(_enabled: boolean): Promise<RemoteListenerState> {
    void _enabled;
    return disabledRemote();
  }

  async rotateRemoteToken(): Promise<RemoteListenerState> {
    return disabledRemote();
  }

  async revokeRemoteDevice(_deviceId: string): Promise<RemoteListenerState> {
    void _deviceId;
    return disabledRemote();
  }
}
