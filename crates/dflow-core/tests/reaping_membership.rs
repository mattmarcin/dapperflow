//! Deterministic proof of the total-reaping foundation (`daemon-lifecycle.md`): the
//! ConPTY console host (`OpenConsole.exe`/`conhost.exe`) that portable-pty spawns is a
//! MEMBER of the process reaping job.
//!
//! Unlike a hard-kill/observe test, this does not depend on any particular Windows build
//! reaping console-attached trees on its own. Membership in a `KILL_ON_JOB_CLOSE` job is
//! the invariant that makes an orphan impossible: if the host is in the job, it cannot
//! outlive the process holding the job's last handle. This is exactly the gap the prior
//! design missed - the shell was in a job but the console host was not.
//!
//! The test installs the reaping job on ITS OWN process (standing in for the daemon),
//! spawns a real PTY session, and asserts a console host shows up in the job's member
//! list. It is the regression guard: remove the reaping install, or let the host escape
//! the job, and this fails deterministically.

#![cfg(windows)]

use std::time::{Duration, Instant};

use dflow_core::{
    install_process_reaping_job, reaping_job_console_host_pids, SessionManager, SessionSpec,
};

fn wait_until<F: FnMut() -> bool>(mut pred: F, timeout: Duration) -> bool {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if pred() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    pred()
}

#[test]
fn conpty_console_host_is_a_member_of_the_reaping_job() {
    // Assign THIS process to the kill-on-close reaping job, exactly as the daemon does at
    // startup. Every process spawned afterwards inherits it.
    install_process_reaping_job().expect("install the process reaping job");

    // A real PTY session: portable-pty's ConPTY backend launches a console host as a
    // child of this process, which - having been spawned after the assign, with no
    // breakaway - lands in the reaping job.
    let mgr = SessionManager::new();
    let session = mgr
        .create(SessionSpec {
            harness: "cmd".into(),
            command: vec!["cmd.exe".into(), "/D".into()],
            cols: 80,
            rows: 24,
            ..Default::default()
        })
        .expect("spawn PTY session");

    // The console host must appear among the reaping job's members. This is the whole
    // point of the foundation: the host, not just the shell, is in the job.
    let host_present =
        wait_until(|| !reaping_job_console_host_pids().is_empty(), Duration::from_secs(15));
    let hosts = reaping_job_console_host_pids();
    assert!(
        host_present && !hosts.is_empty(),
        "the ConPTY console host is NOT a member of the reaping job (members carrying a \
         console-host image: {hosts:?}); an abrupt kill could orphan it"
    );

    session.kill();
}
