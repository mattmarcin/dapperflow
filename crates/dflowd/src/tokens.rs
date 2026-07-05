//! Per-task token scoping for the `dflow` agent CLI (`security.md` / Per-task tokens).
//!
//! A per-task token is minted at dispatch (or cardless session-create), injected into
//! the session environment as `DFLOW_TOKEN`, and revoked at teardown. It grants the
//! agent CLI access only to its own card, session, and project surfaces: a leaked task
//! token cannot touch other cards, projects, or the vault. Audit-scoped tokens may
//! file cards into Inbox but can never move their lanes (`security.md` / Audit
//! sessions). The registry is in-memory: a daemon restart interrupts every dispatched
//! session (its PTY dies with the daemon), so a token never needs to outlive the run.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::sync::{Arc, OnceLock};

use dflow_core::Recipe;
use rand::Rng;

/// The immutable scope plus mutable bookkeeping for one per-task token.
pub struct AgentToken {
    /// The card the task owns (its dispatch card). `None` for a cardless session.
    pub card_id: Option<String>,
    /// The project the task may read/write knowledge for.
    pub project_id: Option<String>,
    /// Audit-scoped: created cards land in Inbox and their lanes may never be moved.
    pub audit: bool,
    /// Per-dispatch card-creation cap (`None` = unbudgeted).
    pub budget_cards: Option<u32>,
    /// Per-dispatch note-creation cap (`None` = unbudgeted).
    pub budget_notes: Option<u32>,
    /// The resolved flow recipe this task was dispatched under (`recipes.md`). Stage
    /// arbitration for `dflow status done` reads it; `None` for a cardless session or a
    /// pre-recipe dispatch path.
    pub recipe: Option<Arc<Recipe>>,
    /// The gate run this task belongs to, when it is a gate reviewer/fixer session
    /// (`gate.md` / Adversarial review). `dflow finding add` resolves the target run from
    /// this; `None` for a normal dispatch/session token.
    pub gate_run_id: Option<String>,
    state: Mutex<TokenState>,
}

#[derive(Default)]
struct TokenState {
    /// The daemon session this token is bound to (set just after the session spawns).
    session_id: Option<String>,
    /// Cards this token created, so it may update/move them (subject to `audit`).
    created_cards: HashSet<String>,
    cards_used: u32,
    notes_used: u32,
}

impl AgentToken {
    /// The session this token is bound to, once the daemon has bound it.
    pub fn session_id(&self) -> Option<String> {
        self.state.lock().expect("token state poisoned").session_id.clone()
    }

    /// Bind the token to its session id (called right after the session spawns).
    pub fn bind_session(&self, session_id: &str) {
        self.state.lock().expect("token state poisoned").session_id = Some(session_id.to_string());
    }

    /// Whether this token may read/mutate `card_id`: its own dispatch card or any card
    /// it created.
    pub fn owns_card(&self, card_id: &str) -> bool {
        if self.card_id.as_deref() == Some(card_id) {
            return true;
        }
        self.state.lock().expect("token state poisoned").created_cards.contains(card_id)
    }

    /// Whether creating another card is within the token's card budget.
    pub fn card_budget_ok(&self) -> bool {
        match self.budget_cards {
            Some(cap) => self.state.lock().expect("token state poisoned").cards_used < cap,
            None => true,
        }
    }

    /// Whether recording another note is within the token's note budget.
    pub fn note_budget_ok(&self) -> bool {
        match self.budget_notes {
            Some(cap) => self.state.lock().expect("token state poisoned").notes_used < cap,
            None => true,
        }
    }

    /// Record a successful card creation: count it and remember it as owned.
    pub fn record_created_card(&self, card_id: &str) {
        let mut state = self.state.lock().expect("token state poisoned");
        state.created_cards.insert(card_id.to_string());
        state.cards_used += 1;
    }

    /// Record a successful note write against the note budget.
    pub fn record_note(&self) {
        self.state.lock().expect("token state poisoned").notes_used += 1;
    }
}

/// Parameters for minting a per-task token.
pub struct TokenScope {
    pub card_id: Option<String>,
    pub project_id: Option<String>,
    pub audit: bool,
    pub budget_cards: Option<u32>,
    pub budget_notes: Option<u32>,
    /// The resolved dispatch recipe, when this token backs a recipe-driven dispatch.
    pub recipe: Option<Arc<Recipe>>,
    /// The gate run this token backs, when it is a gate reviewer/fixer session token.
    pub gate_run_id: Option<String>,
}

/// The in-memory per-task token registry (`token -> scope`).
#[derive(Default)]
pub struct TokenRegistry {
    map: Mutex<HashMap<String, Arc<AgentToken>>>,
}

impl TokenRegistry {
    /// Mint a per-task token for the given scope, returning `(token_string, handle)`.
    /// The session id is bound separately once the session spawns.
    pub fn mint(&self, scope: TokenScope) -> (String, Arc<AgentToken>) {
        let token = mint_token_string();
        let handle = Arc::new(AgentToken {
            card_id: scope.card_id,
            project_id: scope.project_id,
            audit: scope.audit,
            budget_cards: scope.budget_cards,
            budget_notes: scope.budget_notes,
            recipe: scope.recipe,
            gate_run_id: scope.gate_run_id,
            state: Mutex::new(TokenState::default()),
        });
        self.map.lock().expect("token registry poisoned").insert(token.clone(), Arc::clone(&handle));
        (token, handle)
    }

    /// Resolve a token string to its scope handle, or `None` when unknown/revoked.
    pub fn resolve(&self, token: &str) -> Option<Arc<AgentToken>> {
        self.map.lock().expect("token registry poisoned").get(token).cloned()
    }

    /// Revoke every token bound to a session (teardown). Tokens not yet bound to a
    /// session are left (they will bind momentarily or be pruned with their session).
    pub fn revoke_session(&self, session_id: &str) {
        self.map
            .lock()
            .expect("token registry poisoned")
            .retain(|_, tok| tok.session_id().as_deref() != Some(session_id));
    }

    /// Number of live tokens (diagnostics/tests).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn len(&self) -> usize {
        self.map.lock().expect("token registry poisoned").len()
    }

    /// Whether the registry holds no tokens.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// A 48-character alphanumeric task token (matches the root token's entropy).
fn mint_token_string() -> String {
    let mut rng = rand::rng();
    (0..48).map(|_| rng.sample(rand::distr::Alphanumeric) as char).collect()
}

/// Mint a phone-scoped capability bearer token (`security.md` / Remote access). The
/// daemon owns all bearer-token entropy; the store only persists the string it is given.
pub fn mint_phone_token() -> String {
    mint_token_string()
}

/// The Concertmaster-scoped token registry (`security.md` / Concertmaster capability
/// scope; `phase6-mcp.md` merge-time request 4).
///
/// A Concertmaster token is minted by an owner-scope client via
/// `auth.mint_concertmaster` and grants only the read + board + dispatch + steer +
/// knowledge surface. The excluded set - vault (`env.*`), `session.kill`,
/// `dispatch.cancel`, `agents.*`, `recipe.*`, merge/push/discard, and
/// `daemon.shutdown` - is enforced daemon-side by the connection's dispatch table
/// (defense in depth beyond the MCP server's own surface omission). The registry is
/// in-memory: a daemon restart invalidates every token and clients re-mint.
#[derive(Default)]
pub struct ConcertmasterRegistry {
    tokens: Mutex<HashSet<String>>,
}

impl ConcertmasterRegistry {
    /// Mint and record a new Concertmaster-scoped token.
    pub fn mint(&self) -> String {
        let token = mint_token_string();
        self.tokens.lock().expect("concertmaster registry poisoned").insert(token.clone());
        token
    }

    /// Whether `token` is a live Concertmaster-scoped token.
    pub fn contains(&self, token: &str) -> bool {
        self.tokens.lock().expect("concertmaster registry poisoned").contains(token)
    }

    /// Number of live Concertmaster tokens (diagnostics/tests).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn len(&self) -> usize {
        self.tokens.lock().expect("concertmaster registry poisoned").len()
    }

    /// The human-readable capability classes this scope withholds (`security.md`).
    pub fn excludes() -> Vec<String> {
        [
            "vault (env.*)",
            "session.kill",
            "dispatch.cancel",
            "agents.*",
            "recipe.install",
            "merge",
            "push",
            "discard",
            "daemon.shutdown",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }
}

/// A round-scoped token (`product.md` / Concertmaster rounds; M4).
///
/// Minted at `round.start` and injected as `DFLOW_TOKEN` into the headless round
/// session, it grants the Concertmaster *read* surface (fleet/board/knowledge) so the
/// round agent can synthesize state, plus exactly one write verb: `round.digest`, which
/// files the round one escalation digest against **its own** round card (carried here,
/// never client-supplied). Everything else is Forbidden, enforced by `dispatch_round`.
/// The registry is in-memory: a round session dies with a daemon restart, so the token
/// never needs to outlive the run.
pub struct RoundToken {
    /// The round card this token files its digest against and reads context for.
    pub round_card_id: String,
    /// The round type (`floor_check` | `garden`), stamped into `round_completed`.
    pub round_type: String,
    /// `project_id` for a scoped round, or `"all"` for a global round.
    pub scope: String,
    state: Mutex<TokenState>,
}

impl RoundToken {
    /// The session this token is bound to, once bound.
    pub fn session_id(&self) -> Option<String> {
        self.state.lock().expect("round token state poisoned").session_id.clone()
    }

    /// Bind the token to its session id (called right after the round session spawns).
    pub fn bind_session(&self, session_id: &str) {
        self.state.lock().expect("round token state poisoned").session_id = Some(session_id.to_string());
    }
}

/// The in-memory round-token registry (`token -> round scope`).
#[derive(Default)]
pub struct RoundRegistry {
    map: Mutex<HashMap<String, Arc<RoundToken>>>,
}

impl RoundRegistry {
    /// Mint a round token for a round card + scope, returning `(token_string, handle)`.
    pub fn mint(&self, round_card_id: &str, round_type: &str, scope: &str) -> (String, Arc<RoundToken>) {
        let token = mint_token_string();
        let handle = Arc::new(RoundToken {
            round_card_id: round_card_id.to_string(),
            round_type: round_type.to_string(),
            scope: scope.to_string(),
            state: Mutex::new(TokenState::default()),
        });
        self.map.lock().expect("round registry poisoned").insert(token.clone(), Arc::clone(&handle));
        (token, handle)
    }

    /// Resolve a token string to its round scope, or `None` when unknown/revoked.
    pub fn resolve(&self, token: &str) -> Option<Arc<RoundToken>> {
        self.map.lock().expect("round registry poisoned").get(token).cloned()
    }

    /// Revoke every token bound to a session (teardown), like the per-task registry.
    pub fn revoke_session(&self, session_id: &str) {
        self.map
            .lock()
            .expect("round registry poisoned")
            .retain(|_, tok| tok.session_id().as_deref() != Some(session_id));
    }
}

/// The absolute path to the `dflow` CLI binary shipped alongside `dflowd`, so it can be
/// placed on a session's PATH and referenced by the codex `notify` bridge.
///
/// Packaging TODO: in a packaged build `dflow` sits next to `dflowd` in the app's
/// binary dir, which is what `current_exe().parent()` resolves to here. If a future
/// installer separates them, this resolution (and the PATH prepend) must be revisited.
pub fn dflow_binary_path() -> Option<std::path::PathBuf> {
    static PATH: OnceLock<Option<std::path::PathBuf>> = OnceLock::new();
    PATH.get_or_init(|| {
        let dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
        let exe = if cfg!(windows) { "dflow.exe" } else { "dflow" };
        let candidate = dir.join(exe);
        if candidate.exists() {
            Some(candidate)
        } else {
            None
        }
    })
    .clone()
}

/// The directory containing the `dflow` CLI binary, for a session PATH prepend.
pub fn dflow_binary_dir() -> Option<std::path::PathBuf> {
    dflow_binary_path().and_then(|p| p.parent().map(|d| d.to_path_buf()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope(card: Option<&str>, audit: bool, budget_cards: Option<u32>) -> TokenScope {
        TokenScope {
            card_id: card.map(str::to_string),
            project_id: Some("PROJ".into()),
            audit,
            budget_cards,
            budget_notes: None,
            recipe: None,
            gate_run_id: None,
        }
    }

    #[test]
    fn mint_resolve_and_revoke() {
        let reg = TokenRegistry::default();
        assert!(reg.is_empty());
        let (token, handle) = reg.mint(scope(Some("CARD1"), false, None));
        assert_eq!(reg.len(), 1);
        assert!(reg.resolve(&token).is_some());
        handle.bind_session("SESS1");
        // Revoking a different session leaves it; revoking its own removes it.
        reg.revoke_session("OTHER");
        assert_eq!(reg.len(), 1);
        reg.revoke_session("SESS1");
        assert!(reg.resolve(&token).is_none());
        assert!(reg.is_empty());
    }

    #[test]
    fn owns_own_card_and_created_cards_only() {
        let reg = TokenRegistry::default();
        let (_t, tok) = reg.mint(scope(Some("CARD1"), false, None));
        assert!(tok.owns_card("CARD1"));
        assert!(!tok.owns_card("OTHER"));
        tok.record_created_card("CARD2");
        assert!(tok.owns_card("CARD2"));
        assert!(!tok.owns_card("CARD3"));
    }

    #[test]
    fn card_budget_gates_after_cap() {
        let reg = TokenRegistry::default();
        let (_t, tok) = reg.mint(scope(None, true, Some(2)));
        assert!(tok.card_budget_ok());
        tok.record_created_card("C1");
        assert!(tok.card_budget_ok());
        tok.record_created_card("C2");
        // Cap reached: the next create is refused.
        assert!(!tok.card_budget_ok());
    }

    #[test]
    fn unbudgeted_token_never_gates() {
        let reg = TokenRegistry::default();
        let (_t, tok) = reg.mint(scope(Some("C"), false, None));
        for i in 0..100 {
            assert!(tok.card_budget_ok());
            tok.record_created_card(&format!("card{i}"));
        }
        assert!(tok.note_budget_ok());
    }
}
