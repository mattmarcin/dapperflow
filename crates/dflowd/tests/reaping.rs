//! Total-reaping acceptance test (`daemon-lifecycle.md` / correctness foundation).
//!
//! This is the whole point of the daemon-lifecycle overhaul: force-killing the daemon
//! must never orphan a ConPTY console host (`OpenConsole.exe`/`conhost.exe`) that then
//! busy-loops and burns CPU. The historical bug was that the shell was in a Job Object
//! but the console *host* was not, so a hard kill leaked it.
//!
//! The test spawns a real `dflowd`, opens a real PTY session (so a console host really
//! exists), records the daemon's console-host descendants, then kills the daemon process
//! HARD - `TerminateProcess` on just the daemon pid, which is exactly a `taskkill /F`
//! without `/T`: it does not walk the tree. The daemon-wide reaping job is the only thing
//! that can take the console host down, and the test asserts it does, within a couple
//! seconds. Everything runs under an isolated `DFLOW_DATA_DIR` so a live user daemon is
//! never touched.

mod common;

#[cfg(windows)]
mod reaping_win {
    use std::time::{Duration, Instant};

    use dflow_proto::Envelope;

    use super::common::*;

    /// One row of the process table.
    struct Proc {
        pid: u32,
        ppid: u32,
        name: String,
    }

    /// Snapshot the whole process table via the ToolHelp API.
    fn snapshot() -> Vec<Proc> {
        use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
        use windows_sys::Win32::System::Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
            TH32CS_SNAPPROCESS,
        };

        let mut out = Vec::new();
        // SAFETY: a process snapshot handle is created and always closed below; the entry
        // is zero-initialized with its `dwSize` set as the API requires.
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snap == INVALID_HANDLE_VALUE {
                return out;
            }
            let mut entry: PROCESSENTRY32W = std::mem::zeroed();
            entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
            if Process32FirstW(snap, &mut entry) != 0 {
                loop {
                    out.push(Proc {
                        pid: entry.th32ProcessID,
                        ppid: entry.th32ParentProcessID,
                        name: wide_to_string(&entry.szExeFile),
                    });
                    if Process32NextW(snap, &mut entry) == 0 {
                        break;
                    }
                }
            }
            CloseHandle(snap);
        }
        out
    }

    /// A null-terminated UTF-16 exe name to a `String`.
    fn wide_to_string(wide: &[u16]) -> String {
        let end = wide.iter().position(|&c| c == 0).unwrap_or(wide.len());
        String::from_utf16_lossy(&wide[..end])
    }

    fn is_console_host(name: &str) -> bool {
        let n = name.to_ascii_lowercase();
        n == "openconsole.exe" || n == "conhost.exe"
    }

    /// The processes whose survival the test tracks: the ConPTY console host(s) (the
    /// historically-leaking process) and the detached `ping` leaf (the process only a Job
    /// Object can reap). Everything in this set must die with the daemon.
    fn is_tracked(name: &str) -> bool {
        is_console_host(name) || name.eq_ignore_ascii_case("ping.exe")
    }

    /// Every descendant pid of `root` in this snapshot (BFS over parent links, cycle- and
    /// depth-guarded so a reused/looping ppid can never hang the walk).
    fn descendants(procs: &[Proc], root: u32) -> Vec<u32> {
        let mut result = Vec::new();
        let mut frontier = vec![root];
        let mut seen = vec![root];
        let mut depth = 0;
        while !frontier.is_empty() && depth < 64 {
            let mut next = Vec::new();
            for p in procs {
                if frontier.contains(&p.ppid) && !seen.contains(&p.pid) {
                    seen.push(p.pid);
                    result.push(p.pid);
                    next.push(p.pid);
                }
            }
            frontier = next;
            depth += 1;
        }
        result
    }

    /// The tracked descendants of `daemon_pid` (console hosts + the `ping` leaf) as
    /// `(pid, lowercased name)` pairs.
    fn tracked_descendants(daemon_pid: u32) -> Vec<(u32, String)> {
        let procs = snapshot();
        let desc = descendants(&procs, daemon_pid);
        procs
            .iter()
            .filter(|p| desc.contains(&p.pid) && is_tracked(&p.name))
            .map(|p| (p.pid, p.name.to_ascii_lowercase()))
            .collect()
    }

    /// Which of the recorded `(pid, name)` console hosts are still present (matched on both
    /// pid AND name, so a reused pid with a different image never masquerades as a survivor).
    fn survivors(hosts: &[(u32, String)]) -> Vec<u32> {
        let procs = snapshot();
        hosts
            .iter()
            .filter(|(pid, name)| {
                procs.iter().any(|p| p.pid == *pid && p.name.to_ascii_lowercase() == *name)
            })
            .map(|(pid, _)| *pid)
            .collect()
    }

    #[tokio::test]
    async fn hard_kill_reaps_the_conpty_console_host() {
        let data_dir = unique_data_dir("reaping");
        let (mut guard, port, token) = start_daemon(&data_dir, &[]);
        let daemon_pid = guard.0.id();

        let mut ws = connect_and_auth(port, &token).await;
        let mut sink = Vec::new();

        // A session running a persistent leaf that never reads stdin: `cmd /c ping -n 600`
        // stays busy for ~10 minutes, keeping the ConPTY console host attached and alive.
        // An explicit command spawns the tree directly under a real pseudoconsole.
        //
        // This is the behavioral acceptance from the spec: after an abrupt daemon death,
        // ZERO console-host/tree orphans survive. The deterministic proof that the reaping
        // job is the mechanism (the host is a job MEMBER) lives in dflow-core's
        // `reaping_membership` test; here we assert the end-to-end outcome through the real
        // daemon: nothing the daemon spawned is left running once it is gone.
        let create = Envelope::message(
            "reap-1",
            "session.create",
            serde_json::json!({
                "harness": "cmd",
                "command": ["cmd.exe", "/D", "/c", "ping -n 600 127.0.0.1"],
                "cols": 80,
                "rows": 24
            }),
        );
        let resp = request(&mut ws, &create, &mut sink).await;
        assert_eq!(resp.msg_type, "session.create", "session.create did not succeed: {resp:?}");

        // Wait until BOTH the console host and the persistent ping leaf are descendants of
        // the daemon (the precondition: a real, alive session tree to reap).
        let mut tracked = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(15);
        while Instant::now() < deadline {
            tracked = tracked_descendants(daemon_pid);
            let has_host = tracked.iter().any(|(_, n)| is_console_host(n));
            let has_leaf = tracked.iter().any(|(_, n)| n == "ping.exe");
            if has_host && has_leaf {
                break;
            }
            std::thread::sleep(Duration::from_millis(150));
        }
        assert!(
            tracked.iter().any(|(_, n)| is_console_host(n)),
            "precondition failed: no OpenConsole/conhost host under daemon pid {daemon_pid}"
        );
        assert!(
            tracked.iter().any(|(_, n)| n == "ping.exe"),
            "precondition failed: the persistent ping leaf never appeared under the daemon"
        );
        eprintln!("captured session tree under daemon {daemon_pid}: {tracked:?}");

        // HARD kill: TerminateProcess on the daemon pid ONLY (no tree walk). This is the
        // abrupt death - crash or `taskkill /F` without `/T` - that used to leak the tree.
        guard.kill_now();

        // The reaping job must take the WHOLE session tree (console host + ping leaf) down
        // with the daemon, within a couple seconds.
        let reap_deadline = Instant::now() + Duration::from_secs(6);
        loop {
            let alive = survivors(&tracked);
            if alive.is_empty() {
                break;
            }
            assert!(
                Instant::now() < reap_deadline,
                "orphaned process(es) survived a hard daemon kill: {alive:?} \
                 (the reaping job did not take the session tree)"
            );
            std::thread::sleep(Duration::from_millis(150));
        }
        eprintln!(
            "all {} tracked process(es) reaped after hard daemon kill (console host + leaf)",
            tracked.len()
        );
    }
}
