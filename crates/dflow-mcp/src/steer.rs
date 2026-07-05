//! The guarded-steering contract, enforced in the server before any wire call.
//!
//! Guardrails (the design notes section 6.3; all mandatory):
//!
//! 1. One-shot: `steer_session` sends exactly one verified-submit message and
//!    never reads the worker's reply.
//! 2. Playbook-bounded: the tool description binds usage to the stuck-recovery
//!    ladder (`adapters.md` / Steering and recovery).
//! 3. Attributed: the tool result reports exactly what was injected so the
//!    caller can relay attribution; the daemon-side `concertmaster_steered`
//!    event is a merge-time request (no event lands today).
//! 4. Rate-limited: a per-session steer budget per rolling hour, enforced here.
//! 5. `no_auto_steer` absolute: refused before any wire traffic, from the
//!    bundled adapter manifests; unknown adapter families refuse by default
//!    (`adapters.md`: unknown adapters get `no_auto_steer` by default).

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Per-session steer budget per rolling hour (pm-layer.md 6.3 point 4).
pub const STEERS_PER_HOUR: usize = 3;

const WINDOW: Duration = Duration::from_secs(3600);

/// Why a steer was refused before reaching the daemon.
#[derive(Debug, PartialEq, Eq)]
pub enum SteerRefusal {
    /// The adapter family refuses automated steering, absolutely.
    NoAutoSteer { harness: String, known: bool },
    /// The per-session budget for the rolling hour is spent.
    RateLimited { retry_in_secs: u64 },
}

impl std::fmt::Display for SteerRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SteerRefusal::NoAutoSteer { harness, known: true } => write!(
                f,
                "steer refused: the '{harness}' adapter declares no_auto_steer \
                 (its composer cannot be classified reliably), so automated steering \
                 is never attempted on it. This is absolute. Escalate to the human \
                 instead: the session accepts normal typing in its terminal."
            ),
            SteerRefusal::NoAutoSteer { harness, known: false } => write!(
                f,
                "steer refused: '{harness}' is not a known adapter family, and unknown \
                 adapters get no_auto_steer by default (adapters.md). Escalate to the \
                 human instead: the session accepts normal typing in its terminal."
            ),
            SteerRefusal::RateLimited { retry_in_secs } => write!(
                f,
                "steer refused: this session's steer budget ({STEERS_PER_HOUR} per hour) \
                 is spent; next slot frees in about {retry_in_secs}s. Repeated steering \
                 of the same session means it needs a human: file that instead."
            ),
        }
    }
}

/// Check the bundled adapter manifest for the session's harness family.
/// `None` means steerable; `Some` is the refusal to relay.
pub fn no_steer_refusal(harness: &str) -> Option<SteerRefusal> {
    match dflow_core::bundled_manifests().get(harness) {
        Some(m) if m.adapter.no_auto_steer => {
            Some(SteerRefusal::NoAutoSteer { harness: harness.to_string(), known: true })
        }
        Some(_) => None,
        None => Some(SteerRefusal::NoAutoSteer { harness: harness.to_string(), known: false }),
    }
}

/// The in-server, per-session rolling-hour steer budget.
#[derive(Default)]
pub struct SteerGuard {
    by_session: Mutex<HashMap<String, Vec<Instant>>>,
}

impl SteerGuard {
    pub fn new() -> SteerGuard {
        SteerGuard::default()
    }

    /// Consume one budget slot for `session_id`, or refuse with the wait time.
    ///
    /// The slot is consumed at attempt time, not at success time: a failed
    /// verified submit escalates to the human and must never retry into the
    /// void (pm-layer.md 6.3 point 4).
    pub fn try_acquire(&self, session_id: &str) -> Result<usize, SteerRefusal> {
        self.try_acquire_at(session_id, Instant::now())
    }

    /// Refund the most recent slot (used when the daemon does not route the
    /// steer verb at all: nothing reached the session, so nothing was steered).
    pub fn refund(&self, session_id: &str) {
        let mut map = self.by_session.lock().expect("steer guard poisoned");
        if let Some(times) = map.get_mut(session_id) {
            times.pop();
        }
    }

    fn try_acquire_at(&self, session_id: &str, now: Instant) -> Result<usize, SteerRefusal> {
        let mut map = self.by_session.lock().expect("steer guard poisoned");
        let times = map.entry(session_id.to_string()).or_default();
        times.retain(|t| now.duration_since(*t) < WINDOW);
        if times.len() >= STEERS_PER_HOUR {
            let oldest = times.iter().min().expect("non-empty");
            let retry_in = WINDOW.saturating_sub(now.duration_since(*oldest));
            return Err(SteerRefusal::RateLimited { retry_in_secs: retry_in.as_secs().max(1) });
        }
        times.push(now);
        Ok(STEERS_PER_HOUR - times.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_manifest_refuses_steering() {
        let refusal = no_steer_refusal("cursor").expect("cursor declares no_auto_steer");
        assert!(matches!(refusal, SteerRefusal::NoAutoSteer { known: true, .. }));
        assert!(refusal.to_string().contains("no_auto_steer"));
    }

    #[test]
    fn known_steerable_families_pass() {
        for family in ["claude", "codex", "opencode", "pi"] {
            assert!(no_steer_refusal(family).is_none(), "{family} should be steerable");
        }
    }

    #[test]
    fn unknown_family_refuses_by_default() {
        let refusal = no_steer_refusal("powershell").expect("unknown family must refuse");
        assert!(matches!(refusal, SteerRefusal::NoAutoSteer { known: false, .. }));
        assert!(refusal.to_string().contains("not a known adapter family"));
    }

    #[test]
    fn budget_is_per_session_per_hour() {
        let guard = SteerGuard::new();
        let t0 = Instant::now();
        assert_eq!(guard.try_acquire_at("s1", t0), Ok(2));
        assert_eq!(guard.try_acquire_at("s1", t0), Ok(1));
        assert_eq!(guard.try_acquire_at("s1", t0), Ok(0));
        let refusal = guard.try_acquire_at("s1", t0).unwrap_err();
        assert!(matches!(refusal, SteerRefusal::RateLimited { .. }));
        // Another session is unaffected.
        assert_eq!(guard.try_acquire_at("s2", t0), Ok(2));
        // The window rolls: an hour later the budget is back.
        let later = t0 + Duration::from_secs(3601);
        assert_eq!(guard.try_acquire_at("s1", later), Ok(2));
    }

    #[test]
    fn refund_returns_the_slot() {
        let guard = SteerGuard::new();
        let t0 = Instant::now();
        for _ in 0..STEERS_PER_HOUR {
            guard.try_acquire_at("s1", t0).unwrap();
        }
        assert!(guard.try_acquire_at("s1", t0).is_err());
        guard.refund("s1");
        assert_eq!(guard.try_acquire_at("s1", t0), Ok(0));
    }
}
