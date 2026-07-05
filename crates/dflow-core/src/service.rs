//! Per-worktree local services and the port broker (`environments.md` / Local services
//! and the port broker; M3 deliverable).
//!
//! At dispatch, after env materialization, the daemon starts each declared
//! `per_worktree` service once per leased worktree. The port broker allocates real free
//! ports per instance and injects them as `DFLOW_PORT_<NAME>` env vars (and substitutes
//! `{DFLOW_PORT_<NAME>}` into the service command), so parallel dev servers stop fighting
//! over a fixed port. Health is process-alive (v1): a required service whose process
//! dies immediately parks the card (`service_failed`) rather than launching an agent
//! against a dead backend. Teardown kills each service's whole process tree via the
//! Windows Job Object (`job.rs`), the same mechanism that reaps agent CLIs.
//!
//! Services run as plain child processes (no PTY) under the platform shell, so
//! `npm run dev` / `wrangler dev` style commands work unchanged.

use std::collections::{BTreeMap, HashMap};
use std::net::TcpListener;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::Duration;

use crate::job::KillGuard;
use crate::store::ServiceRow;

/// How long to let a freshly spawned service settle before the process-alive health
/// check, so a service that crashes on startup is caught as a failure (v1: health is a
/// point-in-time process-alive check; a service that dies AFTER the settle is observed by
/// the agent, not the health gate). A wider window catches more slow-crashing services at
/// the cost of dispatch latency; 1.2s is a pragmatic default (dev servers take longer than
/// this just to bind their ports anyway).
pub const HEALTH_SETTLE: Duration = Duration::from_millis(1200);

/// The outcome of starting one service.
#[derive(Debug, Clone)]
pub enum ServiceStart {
    /// The service is running; its allocated `DFLOW_PORT_<NAME>` map and pid.
    Started {
        name: String,
        ports: BTreeMap<String, u16>,
        pid: Option<u32>,
        required: bool,
    },
    /// The service failed its process-alive health check.
    Failed { name: String, required: bool, reason: String },
}

impl ServiceStart {
    /// The service name, whatever the outcome.
    pub fn name(&self) -> &str {
        match self {
            ServiceStart::Started { name, .. } | ServiceStart::Failed { name, .. } => name,
        }
    }

    /// Whether this is a required service that failed (the dispatch-blocking case).
    pub fn is_required_failure(&self) -> bool {
        matches!(self, ServiceStart::Failed { required: true, .. })
    }
}

/// A running service instance the daemon owns, killed (whole tree) on teardown.
struct RunningService {
    name: String,
    child: Child,
    kill_guard: KillGuard,
}

impl RunningService {
    /// Terminate the service's whole process tree.
    fn stop(&mut self) {
        self.kill_guard.kill();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The result of starting a worktree's services: the per-service outcomes plus the
/// `DFLOW_PORT_<NAME>` env additions to merge into the agent's spawn env.
pub struct StartedWorktreeServices {
    pub outcomes: Vec<ServiceStart>,
    /// `DFLOW_PORT_<NAME> -> port` for every service that started, to inject into the
    /// agent's spawn env (`environments.md`: injected as env vars).
    pub port_env: BTreeMap<String, String>,
}

impl StartedWorktreeServices {
    /// Whether any required service failed (dispatch must park the card, not launch).
    pub fn has_required_failure(&self) -> bool {
        self.outcomes.iter().any(ServiceStart::is_required_failure)
    }

    /// The first required failure's `(name, reason)`, for the parking Needs You item.
    pub fn required_failure(&self) -> Option<(&str, &str)> {
        self.outcomes.iter().find_map(|o| match o {
            ServiceStart::Failed { name, required: true, reason } => Some((name.as_str(), reason.as_str())),
            _ => None,
        })
    }
}

/// Owns every running service, keyed by leased worktree id, so dispatch teardown can
/// stop exactly the services it started.
#[derive(Default)]
pub struct ServiceManager {
    running: Mutex<HashMap<String, Vec<RunningService>>>,
}

impl ServiceManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Start every `per_worktree` service for a worktree, allocating ports and injecting
    /// them. `key` is the leased worktree id (the teardown handle). A `shared` service is
    /// skipped in v1 (M4+). Started instances are retained under `key`; on any required
    /// failure the caller stops them via [`stop_worktree`].
    pub fn start_worktree(
        &self,
        key: &str,
        services: &[ServiceRow],
        cwd: &Path,
        base_env: &BTreeMap<String, String>,
    ) -> StartedWorktreeServices {
        // `DFLOW_SERVICE_HEALTH_SETTLE_MS` tunes the process-alive settle window (a test
        // seam, and a knob for slow machines / heavy services); default `HEALTH_SETTLE`.
        let settle = std::env::var("DFLOW_SERVICE_HEALTH_SETTLE_MS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .map(Duration::from_millis)
            .unwrap_or(HEALTH_SETTLE);
        self.start_worktree_with_settle(key, services, cwd, base_env, settle)
    }

    /// Like [`start_worktree`] with an explicit health-settle window (tests use a wider
    /// one so a fast-failing command is observed even on a machine with slow shell
    /// startup).
    pub fn start_worktree_with_settle(
        &self,
        key: &str,
        services: &[ServiceRow],
        cwd: &Path,
        base_env: &BTreeMap<String, String>,
        settle: Duration,
    ) -> StartedWorktreeServices {
        let mut outcomes = Vec::new();
        let mut port_env: BTreeMap<String, String> = BTreeMap::new();
        let mut instances: Vec<RunningService> = Vec::new();

        for svc in services {
            if svc.scope != crate::store::service_scope::PER_WORKTREE {
                // shared services are a daemon-managed singleton (M4+); skip in v1.
                continue;
            }
            // Allocate a free port per declared name. Bind all first, then drop, so two
            // names in one service never collide on the same ephemeral port.
            let ports = match allocate_ports(&svc.ports) {
                Ok(p) => p,
                Err(reason) => {
                    outcomes.push(ServiceStart::Failed {
                        name: svc.name.clone(),
                        required: svc.required,
                        reason,
                    });
                    continue;
                }
            };
            // Compose the per-service env: the materialized base, plus every port env so
            // far (later services and the agent see earlier services' ports too).
            let mut env = base_env.clone();
            env.extend(port_env.clone());
            for (name, port) in &ports {
                let key = format!("DFLOW_PORT_{name}");
                env.insert(key.clone(), port.to_string());
                port_env.insert(key, port.to_string());
            }
            let cmd = substitute_ports(&svc.cmd, &ports);
            match spawn_service(&cmd, cwd, &env) {
                Ok((mut child, kill_guard)) => {
                    // Process-alive health check (v1): give it a moment, then confirm it
                    // did not exit immediately.
                    std::thread::sleep(settle);
                    match child.try_wait() {
                        Ok(Some(status)) => {
                            outcomes.push(ServiceStart::Failed {
                                name: svc.name.clone(),
                                required: svc.required,
                                reason: format!("service exited immediately ({status})"),
                            });
                        }
                        Ok(None) => {
                            let pid = child.id();
                            outcomes.push(ServiceStart::Started {
                                name: svc.name.clone(),
                                ports: ports.clone(),
                                pid: Some(pid),
                                required: svc.required,
                            });
                            instances.push(RunningService {
                                name: svc.name.clone(),
                                child,
                                kill_guard,
                            });
                        }
                        Err(e) => {
                            outcomes.push(ServiceStart::Failed {
                                name: svc.name.clone(),
                                required: svc.required,
                                reason: format!("could not poll service health: {e}"),
                            });
                        }
                    }
                }
                Err(e) => outcomes.push(ServiceStart::Failed {
                    name: svc.name.clone(),
                    required: svc.required,
                    reason: format!("could not start service: {e}"),
                }),
            }
        }

        if !instances.is_empty() {
            self.running.lock().expect("service manager poisoned").entry(key.to_string()).or_default().extend(instances);
        }
        StartedWorktreeServices { outcomes, port_env }
    }

    /// Stop and reap every service started for a worktree. Returns how many were stopped.
    pub fn stop_worktree(&self, key: &str) -> usize {
        let taken = self.running.lock().expect("service manager poisoned").remove(key);
        match taken {
            Some(mut services) => {
                let n = services.len();
                for svc in &mut services {
                    tracing::info!(service = %svc.name, worktree = key, "stopping service");
                    svc.stop();
                }
                n
            }
            None => 0,
        }
    }

    /// Stop every running service across all worktrees (daemon shutdown).
    pub fn stop_all(&self) {
        let all: Vec<(String, Vec<RunningService>)> =
            self.running.lock().expect("service manager poisoned").drain().collect();
        for (_key, mut services) in all {
            for svc in &mut services {
                svc.stop();
            }
        }
    }

    /// Number of running services for a worktree (diagnostics/tests).
    pub fn count(&self, key: &str) -> usize {
        self.running
            .lock()
            .expect("service manager poisoned")
            .get(key)
            .map(Vec::len)
            .unwrap_or(0)
    }
}

/// Allocate one free loopback port per declared name. All listeners are bound before any
/// is dropped so two names never share the same ephemeral port. A tiny TOCTOU window
/// remains between the drop and the service binding it, acceptable for v1.
fn allocate_ports(names: &[String]) -> Result<BTreeMap<String, u16>, String> {
    let mut listeners = Vec::new();
    let mut ports = BTreeMap::new();
    for name in names {
        let listener = TcpListener::bind("127.0.0.1:0")
            .map_err(|e| format!("no free port for {name}: {e}"))?;
        let port = listener
            .local_addr()
            .map_err(|e| format!("could not read allocated port for {name}: {e}"))?
            .port();
        ports.insert(name.clone(), port);
        listeners.push(listener);
    }
    drop(listeners);
    Ok(ports)
}

/// Substitute `{DFLOW_PORT_<NAME>}` placeholders in a service command with the allocated
/// ports (`environments.md`: template substitution into service commands).
pub fn substitute_ports(cmd: &str, ports: &BTreeMap<String, u16>) -> String {
    let mut out = cmd.to_string();
    for (name, port) in ports {
        out = out.replace(&format!("{{DFLOW_PORT_{name}}}"), &port.to_string());
    }
    out
}

/// Spawn a service command through the platform shell with `env`, in `cwd`, its stdio
/// discarded, attached to a kill-on-close Job Object so its whole tree dies on teardown.
fn spawn_service(
    cmd: &str,
    cwd: &Path,
    env: &BTreeMap<String, String>,
) -> std::io::Result<(Child, KillGuard)> {
    let mut command = if cfg!(windows) {
        let mut c = Command::new("cmd.exe");
        c.arg("/C").arg(cmd);
        c
    } else {
        let mut c = Command::new("sh");
        c.arg("-c").arg(cmd);
        c
    };
    command
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    for (k, v) in env {
        command.env(k, v);
    }
    let child = command.spawn()?;
    let kill_guard = match child.id() {
        pid if pid != 0 => KillGuard::attach_pid(pid),
        _ => KillGuard::inert(),
    };
    Ok((child, kill_guard))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn substitutes_named_ports() {
        let mut ports = BTreeMap::new();
        ports.insert("HTTP".to_string(), 8080u16);
        ports.insert("INSPECTOR".to_string(), 9229u16);
        let out = substitute_ports("wrangler dev --port {DFLOW_PORT_HTTP} --inspector-port {DFLOW_PORT_INSPECTOR}", &ports);
        assert_eq!(out, "wrangler dev --port 8080 --inspector-port 9229");
    }

    #[test]
    fn allocates_distinct_free_ports() {
        let names = vec!["A".to_string(), "B".to_string(), "C".to_string()];
        let ports = allocate_ports(&names).unwrap();
        assert_eq!(ports.len(), 3);
        let distinct: std::collections::BTreeSet<u16> = ports.values().copied().collect();
        assert_eq!(distinct.len(), 3, "allocated ports must be distinct: {ports:?}");
        assert!(ports.values().all(|&p| p > 0));
    }

    /// A per-worktree service starts, injects its port env, is process-alive, and stops
    /// on teardown; an immediately-exiting required service is a failure.
    #[test]
    fn starts_injects_ports_and_tears_down() {
        let mgr = ServiceManager::new();
        let cwd = std::env::temp_dir();
        // A long-lived command keeps the process alive for the health check, and embeds
        // the port placeholder so this exercises substitution end to end (the pure
        // substitution rules are unit-tested in `substitutes_named_ports`). The port must
        // land in a slot each platform accepts: `ping -w <ms>` on Windows (any positive
        // int is a valid timeout; localhost replies instantly so the 10 pings still span
        // ~9s), and inside a comment on POSIX so `sleep` gets only `10`. A bare trailing
        // arg would make `ping` treat the port as a second target (exit 1) and `sleep`
        // sum it into a multi-hour delay.
        let cmd = if cfg!(windows) {
            "ping -n 10 -w {DFLOW_PORT_HTTP} 127.0.0.1".to_string()
        } else {
            "sh -c \"sleep 10 # {DFLOW_PORT_HTTP}\"".to_string()
        };
        let svc = ServiceRow {
            id: "s1".into(),
            project_id: "p1".into(),
            name: "web".into(),
            cmd,
            scope: crate::store::service_scope::PER_WORKTREE.to_string(),
            ports: vec!["HTTP".to_string()],
            required: true,
        };
        let started = mgr.start_worktree("wt1", std::slice::from_ref(&svc), &cwd, &BTreeMap::new());
        assert!(!started.has_required_failure(), "service should be alive: {:?}", started.outcomes);
        assert!(started.port_env.contains_key("DFLOW_PORT_HTTP"), "port env injected");
        assert_eq!(mgr.count("wt1"), 1);
        assert_eq!(mgr.stop_worktree("wt1"), 1);
        assert_eq!(mgr.count("wt1"), 0);

        // A required service whose command exits with an error is a required failure. A
        // generous settle window observes the exit even where shell cold-start is slow
        // (this machine's `cmd.exe` cold-start alone is ~3s, so the default 1.2s settle
        // would still see it "starting"; a real dev server stays alive well past this).
        let bad = ServiceRow {
            id: "s2".into(),
            project_id: "p1".into(),
            name: "broken".into(),
            cmd: "exit 1".into(),
            scope: crate::store::service_scope::PER_WORKTREE.to_string(),
            ports: vec![],
            required: true,
        };
        let started = mgr.start_worktree_with_settle(
            "wt2",
            std::slice::from_ref(&bad),
            &cwd,
            &BTreeMap::new(),
            Duration::from_secs(8),
        );
        assert!(started.has_required_failure(), "an error-exit required service must fail: {:?}", started.outcomes);
        assert_eq!(started.required_failure().map(|(n, _)| n), Some("broken"));
        mgr.stop_worktree("wt2");
    }
}
