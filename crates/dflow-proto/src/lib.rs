//! DapperFlow wire protocol types (see `docs/spec/protocol.md`).
//!
//! Two channels share one WebSocket:
//!
//! - Control messages: JSON text frames with the envelope `{ v, id, type, payload }`.
//! - PTY I/O: binary frames `[u8 kind][16-byte session id][bytes...]`.
//!
//! Phase 0 implements the `auth.*` handshake, the `session.*` subset, and a
//! `daemon.shutdown` admin verb for test teardown. Everything else in
//! `protocol.md` (project/card/dispatch/artifact/env/recipe/fleet/event families)
//! is a later phase; this crate intentionally ships only the Phase 0 subset.

mod entities;
mod frame;
mod messages;
mod snapshot;

pub use entities::{
    Agent, ArtifactMeta, Card, CardEvent, CheckCmd, FeedbackItem, FindingInfo, GateRunInfo,
    GithubIssueInfo, LayoutWarning, NeedsYouItem, Project, RecipeInvalid, RecipeSummary,
    RecipeValidationError, ServiceInfo, SessionSummary, TextAnchor,
};
pub use frame::{decode_frame, encode_frame, Frame, FrameError, FrameKind, SESSION_ID_LEN};
pub use messages::{
    AgentAdd, AgentContext, AgentContextResult, AgentRemove, AgentRemoved, AgentResult, AgentUpdate,
    AgentsDetect, AgentsDetected, AgentsList, AgentsListResult, ArtifactGet, ArtifactGetResult,
    ArtifactRegister, ArtifactRegistered, AuthHello, AuthWelcome, CardCreate, CardCreated,
    CardFilter, CardGet, CardGetResult, CardMove, CardQuery, CardQueryResult, CardResult, CardUpdate,
    ClientKind, ConcertmasterMinted, ConsentRequired, DaemonShutdown, DetectedCli, DispatchCancel,
    DispatchCancelled, DispatchStart, DispatchStarted, EnvCleaned, EnvCleanup, EnvDelete, EnvDeleted,
    EnvEntryInfo, EnvImport, EnvImportResult, EnvList, EnvListResult, EnvMaterialize, EnvMaterialized,
    EnvSet, EnvSetResult, EventAck, EventCardEvent, EventSubscribe, EventSubscribed, FeedbackPoll,
    FeedbackPollResult, FeedbackSubmit, FeedbackSubmitResult, FindingAdd, FindingAddResult,
    FindingResult, FleetStatus, FleetStatusResult, GateMerge, GateMergeResult, GateResolveFinding,
    GateRun, GateRunStarted, GateShip, GateShipResult, GateStatus, GateStatusResult,
    GithubAuthResult, GithubAuthStatus, GithubImportResult, GithubIssueFilter, GithubIssueGet,
    GithubIssueGetResult, GithubIssuePreview, GithubIssuesImport, GithubIssuesImportResult,
    GithubIssuesPreview, GithubIssuesPreviewResult, KnowAdd, KnowAddResult, KnowCatalogGroup,
    KnowFind, KnowFindResult, KnowGet, KnowGetResult, KnowIndex, KnowIndexResult, KnowNote,
    KnowNoteHit, LanDisable, LanEnable, LanPair, LanPairing, LanRevoke, LanRevoked, LanState,
    LanStatus, MintConcertmaster, NeedsYouList, NeedsYouListResult, NeedsYouResolve,
    NeedsYouResolved, NotifyForward, PairingPayload, PhonePairing, ProjectAdd, ProjectAdded,
    ProjectList, ProjectListResult, ProjectUpdate, ProjectUpdated, RecipeGet, RecipeGetResult,
    RecipeGrant, RecipeGranted, RecipeGrantRevoked, RecipeInstall, RecipeInstalled, RecipeList,
    RecipeListResult, RecipeRevokeGrant, RecipeValidate, RecipeValidateResult, RoundDigest,
    RoundDigestResult, RoundStart, RoundStarted, SelfReport, SelfReportResult, SendVerified,
    SendVerifiedResult, ServiceAdd, ServiceList, ServiceListResult, ServiceRemove, ServiceRemoved,
    ServiceResult, SessionAttach, SessionAttached, SessionCreate, SessionCreated, SessionDetach,
    SessionInfo, SessionKill, SessionList, SessionListResult, SessionPeek, SessionPeeked,
    SessionRename, SessionResume, SessionResumed, SetNote, Simple,
};
pub use snapshot::{CursorPos, StyledRun, StyledSnapshot};

use serde::{Deserialize, Serialize};

/// Current protocol envelope version. Bumps only on breaking envelope changes;
/// family payloads evolve additively with serde defaults (`protocol.md` / Versioning).
pub const PROTOCOL_VERSION: u8 = 1;

/// WebSocket close code sent when the auth handshake fails, so clients can tell
/// "reconnect" from "re-pair" (`protocol.md` / Errors).
pub const CLOSE_AUTH_FAILED: u16 = 4001;
/// WebSocket close code sent when the client is older than the daemon minimum.
pub const CLOSE_UPGRADE_REQUIRED: u16 = 4002;

/// The control-channel envelope. `id` is present on requests and their matching
/// responses (responses echo the id); server-initiated `event.*` messages omit it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope {
    pub v: u8,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub id: Option<String>,
    #[serde(rename = "type")]
    pub msg_type: String,
    #[serde(default)]
    pub payload: serde_json::Value,
}

impl Envelope {
    /// Build an envelope carrying a serializable payload under the given type.
    pub fn new(id: Option<String>, msg_type: impl Into<String>, payload: impl Serialize) -> Self {
        Self {
            v: PROTOCOL_VERSION,
            id,
            msg_type: msg_type.into(),
            payload: serde_json::to_value(payload).unwrap_or(serde_json::Value::Null),
        }
    }

    /// A request/response envelope with a correlation id.
    pub fn message(id: impl Into<String>, msg_type: impl Into<String>, payload: impl Serialize) -> Self {
        Self::new(Some(id.into()), msg_type, payload)
    }

    /// A server-initiated event envelope (no id, `event.*` type).
    pub fn event(msg_type: impl Into<String>, payload: impl Serialize) -> Self {
        Self::new(None, msg_type, payload)
    }

    /// A structured error response echoing the originating request id.
    pub fn error(id: Option<String>, err: ProtocolError) -> Self {
        Self {
            v: PROTOCOL_VERSION,
            id,
            msg_type: "error".to_string(),
            payload: serde_json::to_value(err).unwrap_or(serde_json::Value::Null),
        }
    }

    /// Deserialize the payload into a concrete message type.
    pub fn decode_payload<T: for<'de> Deserialize<'de>>(&self) -> Result<T, serde_json::Error> {
        T::deserialize(&self.payload)
    }
}

/// Stable, machine-readable error codes (`protocol.md` / Errors). Additive only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// Handshake failed or a request arrived before a successful `auth.hello`.
    AuthFailed,
    /// Malformed envelope or payload.
    BadRequest,
    /// The requested session/entity does not exist.
    NotFound,
    /// A verb the daemon does not implement in this phase.
    Unsupported,
    /// The client's protocol version is not supported.
    UpgradeRequired,
    /// An in-flight request was cancelled.
    Cancelled,
    /// The caller's token scope does not permit this action on this surface
    /// (`security.md` / Per-task tokens; e.g. an agent token reaching another card,
    /// or an audit token trying to move a lane on a card it filed).
    Forbidden,
    /// A recipe/dispatch creation budget (`cards`/`notes`) was exceeded
    /// (`recipes.md` / budgets, `knowledge.md` / audit note-budget caps).
    BudgetExceeded,
    /// A privileged recipe was dispatched without a valid per-project grant; the error
    /// `detail` carries a `ConsentRequired` JSON payload the UI turns into a consent
    /// flow (`recipes.md` / Validation and safety, `security.md` / Recipe trust tiers).
    ConsentRequired,
    /// An unexpected internal failure.
    Internal,
}

/// The structured error payload: `{ code, message, retryable, detail? }`.
#[derive(Debug, Clone, Serialize, Deserialize, thiserror::Error)]
#[error("{code:?}: {message}")]
pub struct ProtocolError {
    pub code: ErrorCode,
    pub message: String,
    pub retryable: bool,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub detail: Option<String>,
}

impl ProtocolError {
    pub fn new(code: ErrorCode, message: impl Into<String>, retryable: bool) -> Self {
        Self { code, message: message.into(), retryable, detail: None }
    }

    pub fn auth(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::AuthFailed, message, false)
    }

    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::BadRequest, message, false)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::NotFound, message, false)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Internal, message, true)
    }

    pub fn unsupported(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Unsupported, message, false)
    }

    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Forbidden, message, false)
    }

    pub fn budget_exceeded(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::BudgetExceeded, message, false)
    }

    pub fn consent_required(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::ConsentRequired, message, false)
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_round_trips() {
        let env = Envelope::message("01ABC", "session.detach", SessionDetach { session_id: "S".into() });
        let json = serde_json::to_string(&env).unwrap();
        let back: Envelope = serde_json::from_str(&json).unwrap();
        assert_eq!(back.v, PROTOCOL_VERSION);
        assert_eq!(back.msg_type, "session.detach");
        assert_eq!(back.id.as_deref(), Some("01ABC"));
        let payload: SessionDetach = back.decode_payload().unwrap();
        assert_eq!(payload.session_id, "S");
    }

    #[test]
    fn event_has_no_id() {
        let env = Envelope::event("event.ping", serde_json::json!({}));
        let json = serde_json::to_string(&env).unwrap();
        assert!(!json.contains("\"id\""));
    }

    #[test]
    fn error_serializes_stable_code() {
        let env = Envelope::error(Some("1".into()), ProtocolError::not_found("no such session"));
        let json = serde_json::to_string(&env).unwrap();
        assert!(json.contains("\"code\":\"not_found\""));
        assert!(json.contains("\"retryable\":false"));
    }
}
