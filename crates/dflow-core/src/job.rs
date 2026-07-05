//! Process-tree termination.
//!
//! Two layered Job Object mechanisms cooperate (`daemon-lifecycle.md` / total reaping):
//!
//! 1. **The daemon-wide reaping job** ([`install_process_reaping_job`]). At startup the
//!    daemon assigns *its own* process to a Job Object with
//!    `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`. Every process the daemon later spawns joins
//!    that job by inheritance (Windows adds a new process to its parent's job unless the
//!    child explicitly breaks away). Crucially this captures the ConPTY *console host*
//!    (`OpenConsole.exe`/`conhost.exe`): portable-pty spawns it via `CreatePseudoConsole`
//!    and the shell via `CreateProcessW`, and neither call passes `CREATE_BREAKAWAY_FROM_JOB`,
//!    so both land in the reaping job. When the daemon dies by ANY route (graceful stop,
//!    panic, or a hard `taskkill /F` of just the daemon pid) the OS closes the daemon's
//!    last handle to the job and kill-on-close terminates every surviving descendant.
//!    This is why no orphan `OpenConsole.exe` can outlive the daemon and busy-loop.
//!
//! 2. **Per-session kill guards** ([`KillGuard`]). Each session's shell is *also* assigned
//!    to its own nested job so killing one session (`session.kill`, teardown) takes that
//!    session's subtree without touching the others. On Windows 8+ a process already in
//!    the reaping job nests cleanly into a per-session job; if the OS refuses the nesting
//!    the guard degrades to a direct child kill, and the reaping job remains the backstop.
//!
//! On other platforms both are no-ops for Phase 0; Unix process-group handling arrives
//! when macOS and Linux enter CI at M1 (`architecture.md` / Platform notes).
//!
//! TODO(architecture.md): Unix process-group kill via setsid + killpg, and a
//! prctl(PR_SET_PDEATHSIG)/process-group analog of the reaping job.

#[cfg(windows)]
use std::sync::OnceLock;

/// The installed reaping job's handle, kept for the whole process lifetime so the job
/// (and its kill-on-close semantics) stays alive and can be queried. `None` until
/// [`install_process_reaping_job`] succeeds.
#[cfg(windows)]
static REAPING_JOB_HANDLE: OnceLock<isize> = OnceLock::new();

/// Assign the current process to a Windows Job Object with
/// `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`, so every child the process spawns afterwards
/// (shells, ConPTY console hosts, agent CLIs, and their descendants) is terminated when
/// this process dies, however it dies. Idempotent; the first call wins and the result is
/// cached. Call this once, as early as possible in the daemon, before any session spawns.
///
/// The job handle is deliberately leaked for the process lifetime: closing it while the
/// daemon is still alive would trip kill-on-close and take the daemon (and its tree) down
/// immediately. The OS closes the handle for us at process exit, which is exactly the
/// moment we want the reaping to fire.
///
/// On non-Windows this is a no-op (Unix reaping lands with M1). Failure is returned to
/// the caller to log, never fatal: the per-session [`KillGuard`]s still bound each tree.
pub fn install_process_reaping_job() -> Result<(), String> {
    #[cfg(windows)]
    {
        // Compute the install exactly once and cache its result.
        static RESULT: OnceLock<Result<(), String>> = OnceLock::new();
        RESULT
            .get_or_init(|| {
                let handle = build_reaping_job()?;
                let _ = REAPING_JOB_HANDLE.set(handle);
                Ok(())
            })
            .clone()
    }
    #[cfg(not(windows))]
    {
        Ok(())
    }
}

/// The pids of the ConPTY console hosts (`OpenConsole.exe`/`conhost.exe`) that are
/// members of the daemon-wide reaping job. Empty when the job is not installed, on
/// non-Windows, or when no session has spawned a host yet.
///
/// This is the deterministic proof that the reaping foundation holds: a console host in
/// this list is, by construction, a member of a `KILL_ON_JOB_CLOSE` job and therefore
/// cannot outlive the daemon - no dependence on any particular Windows build's console
/// teardown behavior. It doubles as a live diagnostic (how many hosts the job will reap).
pub fn reaping_job_console_host_pids() -> Vec<u32> {
    #[cfg(windows)]
    {
        let handle = match REAPING_JOB_HANDLE.get() {
            Some(&h) => h,
            None => return Vec::new(),
        };
        let members: std::collections::HashSet<u32> =
            query_job_member_pids(handle).into_iter().collect();
        if members.is_empty() {
            return Vec::new();
        }
        snapshot_processes()
            .into_iter()
            .filter(|(pid, name)| members.contains(pid) && is_console_host(name))
            .map(|(pid, _)| pid)
            .collect()
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}

#[cfg(windows)]
fn build_reaping_job() -> Result<isize, String> {
    use win32job::{ExtendedLimitInfo, Job};

    let mut info = ExtendedLimitInfo::new();
    info.limit_kill_on_job_close();
    let job = Job::create_with_limit_info(&info).map_err(|e| format!("create reaping job: {e}"))?;
    job.assign_current_process()
        .map_err(|e| format!("assign current process to reaping job: {e}"))?;
    // Keep the handle open for the whole process lifetime (see the doc comment): dropping
    // it now would close the last handle and kill-on-close would reap the daemon itself.
    // `into_handle` leaks the handle without closing it and hands us the raw value so the
    // job stays queryable via `reaping_job_console_host_pids`.
    Ok(job.into_handle())
}

/// The pids currently assigned to the job identified by `handle`, via
/// `QueryInformationJobObject(JobObjectBasicProcessIdList)`.
#[cfg(windows)]
fn query_job_member_pids(handle: isize) -> Vec<u32> {
    use windows_sys::Win32::System::JobObjects::{
        JobObjectBasicProcessIdList, QueryInformationJobObject, JOBOBJECT_BASIC_PROCESS_ID_LIST,
    };

    // Room for a generous number of member pids in one shot (the header carries a single
    // inline pid; the rest follow contiguously).
    const CAPACITY: usize = 2048;
    let header = std::mem::size_of::<JOBOBJECT_BASIC_PROCESS_ID_LIST>();
    let extra = CAPACITY.saturating_sub(1) * std::mem::size_of::<usize>();
    let mut buf = vec![0u8; header + extra];

    // SAFETY: `buf` is a correctly sized, aligned (Vec<u8> is at least usize-aligned in
    // practice; the struct's alignment is usize and the header field forces it) byte
    // buffer for the query. On success we read only the number of entries the OS reports.
    let ok = unsafe {
        QueryInformationJobObject(
            handle as _,
            JobObjectBasicProcessIdList,
            buf.as_mut_ptr() as *mut _,
            buf.len() as u32,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Vec::new();
    }
    // SAFETY: the query succeeded, so the buffer holds a valid header; read the count and
    // then that many contiguous pointer-sized pids starting at `ProcessIdList`.
    unsafe {
        let list = buf.as_ptr() as *const JOBOBJECT_BASIC_PROCESS_ID_LIST;
        let count = ((*list).NumberOfProcessIdsInList as usize).min(CAPACITY);
        let first = std::ptr::addr_of!((*list).ProcessIdList) as *const usize;
        (0..count).map(|i| *first.add(i) as u32).collect()
    }
}

/// Whether an image file name is a ConPTY console host.
#[cfg(windows)]
fn is_console_host(name: &str) -> bool {
    name.eq_ignore_ascii_case("openconsole.exe") || name.eq_ignore_ascii_case("conhost.exe")
}

/// A `(pid, image name)` snapshot of the whole process table via ToolHelp.
#[cfg(windows)]
fn snapshot_processes() -> Vec<(u32, String)> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };

    let mut out = Vec::new();
    // SAFETY: the snapshot handle is created and unconditionally closed below; the entry
    // is zero-initialized with `dwSize` set as the API requires.
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return out;
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        if Process32FirstW(snap, &mut entry) != 0 {
            loop {
                let end =
                    entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(entry.szExeFile.len());
                let name = String::from_utf16_lossy(&entry.szExeFile[..end]);
                out.push((entry.th32ProcessID, name));
                if Process32NextW(snap, &mut entry) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snap);
    }
    out
}

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
