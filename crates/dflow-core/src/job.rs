//! Process-tree termination.
//!
//! On Windows, killing the PTY child is not enough: the agent CLI it launches (and
//! that agent's own children) survive. DapperFlow assigns the child to a Windows
//! Job Object with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`, so dropping the job (or the
//! session) terminates the entire tree (`architecture.md` / supervision, deliverable 2).
//!
//! On other platforms this is a no-op for Phase 0; Unix process-group handling
//! arrives when macOS and Linux enter CI at M1 (`architecture.md` / Platform notes).
//!
//! TODO(architecture.md): Unix process-group kill via setsid + killpg.

/// A guard that terminates a process tree when killed or dropped.
pub struct KillGuard {
    #[cfg(windows)]
    job: Option<win32job::Job>,
    #[cfg(not(windows))]
    _private: (),
}

impl KillGuard {
    /// Attach the process with the given OS pid to a kill-on-close job (Windows) so
    /// the whole tree dies with the session. Failure degrades to a plain child kill.
    #[cfg(windows)]
    pub fn attach_pid(pid: u32) -> Self {
        match build_job(pid) {
            Ok(job) => KillGuard { job: Some(job) },
            Err(err) => {
                tracing::warn!(pid, %err, "could not assign process to a job object; falling back to direct kill");
                KillGuard { job: None }
            }
        }
    }

    /// Non-Windows placeholder for Phase 0.
    #[cfg(not(windows))]
    pub fn attach_pid(_pid: u32) -> Self {
        KillGuard { _private: () }
    }

    /// A guard that manages no job (used when the child pid is unavailable).
    pub fn inert() -> Self {
        #[cfg(windows)]
        {
            KillGuard { job: None }
        }
        #[cfg(not(windows))]
        {
            KillGuard { _private: () }
        }
    }

    /// Terminate the tree now by closing the last job handle.
    pub fn kill(&mut self) {
        #[cfg(windows)]
        {
            // Dropping the job closes its last handle, triggering kill-on-job-close.
            self.job.take();
        }
    }
}

#[cfg(windows)]
fn build_job(pid: u32) -> Result<win32job::Job, String> {
    use win32job::{ExtendedLimitInfo, Job};
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE,
    };

    // SAFETY: OpenProcess with a valid pid and access mask returns a handle we own,
    // or null on failure. We close it after the job takes ownership of the process.
    let handle = unsafe { OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid) };
    if handle.is_null() {
        return Err("OpenProcess returned null".to_string());
    }

    let result = (|| {
        let mut info = ExtendedLimitInfo::new();
        info.limit_kill_on_job_close();
        let job = Job::create().map_err(|e| format!("Job::create: {e}"))?;
        job.set_extended_limit_info(&info).map_err(|e| format!("set_extended_limit_info: {e}"))?;
        job.assign_process(handle as isize).map_err(|e| format!("assign_process: {e}"))?;
        Ok(job)
    })();

    // The job now references the process; our handle is no longer needed.
    // SAFETY: `handle` is the valid handle returned above and is closed exactly once.
    unsafe {
        CloseHandle(handle);
    }
    result
}
