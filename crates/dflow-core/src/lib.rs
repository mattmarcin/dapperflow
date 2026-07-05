//! DapperFlow engine.
//!
//! Phase 0 built the PTY session manager and the swappable VT `ScreenModel`
//! (`architecture.md` / internalized multiplexer). Phase 1 adds the SQLite store
//! and event log (`store`), the shared data directory (`paths`), and the worktree
//! pool (`worktree`). The adapter runtime and artifact service arrive later.

pub mod agents;
pub mod env;
pub mod github;
pub mod harness;
pub mod heuristics;
pub mod knowledge;
pub mod manifest;
pub mod recipe;
pub mod secret;
pub mod service;
mod job;
mod paths;
mod ring;
mod screen;
mod session;
pub mod steer;
mod store;
mod worktree;

pub use env::{DriftEntry, EnvVault, EnvError, ImportReport, MaterializedEnv, VaultCrypto};
pub use job::{install_process_reaping_job, reaping_job_console_host_pids};
pub use github::{
    Gh, GhAuth, GhError, GhRunner, Issue, IssueFilters, MergeMethod, PrCreate, PrInfo, ProcessRunner,
    RepoInfo, RepoRef,
};
pub use service::{ServiceManager, ServiceStart, StartedWorktreeServices};
pub use manifest::{bundled_manifests, Manifest, ManifestError, ManifestSet};
pub use paths::{project_slug, DataDir, PathError};
pub use secret::{SecretRegistry, REDACTED};
pub use recipe::{
    project_recipe_dir, Recipe, RecipeCatalog, RecipeError, RecipeScope, ResolveError,
    ResolvedRecipe, Stage, TrustTier, WorktreeStrategy,
};
pub use ring::ScrollbackRing;
pub use screen::{AlacrittyScreen, ScreenModel};
pub use session::{default_command, Session, SessionError, SessionManager, SessionSpec};
pub use steer::{send_verified, SubmitConfig, VerifiedSubmit};
pub use store::{
    agent_source, artifact_status, category, env_kind, event_kind, gate_status, gate_step,
    lease_state, resolution, service_scope, session_state, setting_key, severity, AgentPatch,
    ArtifactRow, CardPatch, CardQueryFilter, DetectionOutcome, EnvEntryMeta, FindingRow, GateRunRow,
    KnowledgeRow, NeedsYouItem, NewAgent, NewCard, NewGateRun, NewSession, OriginUpsert,
    PhoneTokenRow, RecipeGrant, RecipeIndexRow, ServiceRow, SessionRow, Store, StoreError,
    WorktreeRow, SCHEMA_VERSION,
};
pub use worktree::{ReleaseOutcome, WorktreeError, WorktreePool};

// Re-export the wire types the engine produces so downstream crates have one path.
pub use dflow_proto;
