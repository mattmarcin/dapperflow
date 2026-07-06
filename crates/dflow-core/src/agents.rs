//! Configured-agent support: the extra-args safety surface and PATH autodetection
//! (`product.md` / Settings > Agents, `adapters.md`).
//!
//! Launchers themselves are user data in the `agents` table (see `store::agents`);
//! this module holds the two pieces of behavior that sit next to that data: deciding
//! when a launcher's default arguments weaken safety (so the UI can warn), and
//! finding installed CLIs on PATH so detection can create launchers for them.

use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Arguments that weaken an agent's safety posture (`product.md` / Settings >
/// Agents: extra args "shown with a caution styling when they weaken safety").
///
/// This is the single source of truth for the caution surface; keep it here and
/// point new entries at `product.md`. Entries are matched as whole tokens, and the
/// two-token forms (a flag plus its value) are matched as an adjacent pair or as the
/// `flag=value` spelling. Drawn from the autonomy flags in the `adapters.md`
/// capability matrix (claude `--dangerously-skip-permissions` /
/// `--permission-mode bypassPermissions`, codex `-a never`, opencode `--auto`).
const DANGER_SINGLE: &[&str] = &["--dangerously-skip-permissions", "--auto"];

/// Two-token danger forms: `(flag, value)`. Matched adjacent or as `flag=value`.
const DANGER_PAIRS: &[(&str, &str)] = &[
    ("--permission-mode", "bypassPermissions"),
    ("-a", "never"),
    ("--ask-for-approval", "never"),
];

/// Whether a launcher's `extra_args` should carry a caution badge.
pub fn caution(extra_args: &[String]) -> bool {
    let args: Vec<&str> = extra_args.iter().map(String::as_str).collect();
    if args.iter().any(|a| DANGER_SINGLE.contains(a)) {
        return true;
    }
    for (flag, value) in DANGER_PAIRS {
        let joined = format!("{flag}={value}");
        if args.iter().any(|a| *a == joined) {
            return true;
        }
        // Adjacent pair: the flag immediately followed by its dangerous value.
        if args.windows(2).any(|w| w[0] == *flag && w[1] == *value) {
            return true;
        }
    }
    false
}

/// The CLIs detection scans PATH for: `(launcher_name, adapter, binary)`. The launcher
/// name doubles as the family key for a freshly detected CLI; `binary` is what is
/// resolved on PATH, which differs for cursor (`product.md` / Autodetection).
///
/// Cursor correction (Phase 2): the `cursor` binary is the DESKTOP EDITOR shim that
/// opens the GUI - Phase 1.5 wrongly created a launcher from it. The actual agent CLI
/// is `cursor-agent` (an unambiguous alias of `agent`), verified locally at
/// `2026.07.01-41b2de7`; detection probes `cursor-agent`, never bare `cursor` (GUI) or
/// bare `agent` (too collision-prone). See `the design notes`.
pub const DETECTABLE_CLIS: &[(&str, &str, &str)] = &[
    ("claude", "claude", "claude"),
    ("codex", "codex", "codex"),
    ("opencode", "opencode", "opencode"),
    ("cursor", "cursor", "cursor-agent"),
    ("pi", "pi", "pi"),
];

/// How long a single `<cli> --version` probe may run before it is killed.
const VERSION_PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// A CLI found on PATH by [`detect_installed`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectedCli {
    /// The CLI/adapter name (`claude`, `codex`, ...).
    pub name: String,
    /// The adapter behavior family for the CLI.
    pub adapter: String,
    /// The executable resolved on PATH (full path including its shim extension).
    pub command: String,
    /// Version parsed from `<command> --version`, when the probe succeeded.
    pub version: Option<String>,
}

/// Scan the process `PATH` for the known CLIs and probe each one's version. Runs
/// only when the caller (the `agents.detect` verb) asks; never in the background.
pub fn detect_installed() -> Vec<DetectedCli> {
    detect_installed_in(&path_dirs())
}

/// Like [`detect_installed`] but over an explicit set of search directories, so
/// tests can scan a temp dir of stub executables without touching the real PATH.
pub fn detect_installed_in(dirs: &[PathBuf]) -> Vec<DetectedCli> {
    let mut found = Vec::new();
    for (name, adapter, binary) in DETECTABLE_CLIS {
        if let Some(command) = resolve_on_path(binary, dirs) {
            let version = probe_version(&command, VERSION_PROBE_TIMEOUT);
            found.push(DetectedCli {
                name: (*name).to_string(),
                adapter: (*adapter).to_string(),
                command: command.to_string_lossy().into_owned(),
                version,
            });
        }
    }
    found
}

/// Whether a stored command points at the Cursor desktop editor shim (opens the GUI)
/// rather than the `cursor-agent` CLI: the file stem is exactly `cursor`
/// (case-insensitive), not `cursor-agent`. Used to correct a stale detected launcher
/// (`store::agents::apply_detection`).
pub fn is_cursor_editor_shim(command: &str) -> bool {
    Path::new(command)
        .file_stem()
        .and_then(|s| s.to_str())
        .is_some_and(|stem| stem.eq_ignore_ascii_case("cursor"))
}

/// The directories on the process `PATH`, in order.
fn path_dirs() -> Vec<PathBuf> {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default()
}

/// The executable extensions a shell would try, in order: `PATHEXT` (Windows) plus
/// the shim kinds the product cares about, then the bare name last.
///
/// A shell prefers a `PATHEXT` match (e.g. `codex.cmd`) over an extension-less file
/// of the same name; the latter is often a launcher script that Windows cannot spawn
/// directly and so cannot be probed or launched, so it is only a last resort.
fn executable_extensions() -> Vec<OsString> {
    let mut exts: Vec<OsString> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    let mut push = |ext: &str| {
        let norm = ext.to_ascii_lowercase();
        if !norm.is_empty() && !seen.contains(&norm) {
            seen.push(norm);
            exts.push(OsString::from(ext));
        }
    };
    // PATHEXT first, mirroring the shell's own resolution order.
    if let Some(pathext) = std::env::var_os("PATHEXT") {
        for ext in std::env::split_paths(&pathext) {
            if let Some(s) = ext.to_str() {
                push(s);
            }
        }
    }
    // Ensure the shim kinds product.md names are always considered.
    for ext in [".exe", ".cmd", ".bat", ".ps1", ".com"] {
        push(ext);
    }
    // Bare name as a fallback only.
    exts.push(OsString::new());
    exts
}

/// Resolve `name` to an executable file the way a shell would: for each PATH
/// directory, try the bare name and each executable extension, first match wins.
fn resolve_on_path(name: &str, dirs: &[PathBuf]) -> Option<PathBuf> {
    let exts = executable_extensions();
    for dir in dirs {
        for ext in &exts {
            let mut file = OsString::from(name);
            file.push(ext);
            let candidate = dir.join(&file);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Rewrite a session launch argv so Windows `CreateProcessW` (portable-pty's ConPTY
/// backend) can actually spawn it (`adapters.md` / launch resolution; audit finding #2).
///
/// The DETECTION path already resolves a CLI to its real launchable form via `PATHEXT`
/// (`resolve_on_path`); the SPAWN path must do the same, because portable-pty's own
/// search prefers an *exact* (extension-less) name match over a `PATHEXT` variant. On
/// Windows, `codex`/`opencode`/`pi` install as an extension-less `#!/bin/sh` shim next to
/// a `codex.cmd`; portable-pty hands the bare shim to `CreateProcessW`, which fails with
/// os error 193 ("%1 is not a valid Win32 application"). Only `claude` ships a native
/// `.exe`, which is why it alone launched before this fix.
///
/// This resolves `command[0]` to its concrete launchable file, PREFERRING a launchable
/// extension (`.exe`/`.cmd`/`.bat`/...) over the extension-less shim, and:
/// - runs a batch shim (`.cmd`/`.bat`) through `cmd.exe /c <script> <args...>`, because
///   `CreateProcessW` cannot execute a batch script directly (also os error 193);
/// - uses a native executable (`.exe`/`.com`) directly, resolved to its absolute path.
///
/// A program that cannot be resolved on `PATH` (a shell built-in path, an unknown
/// command, a non-Windows target) is returned unchanged so the spawn surfaces the real
/// error rather than this masking it. Non-Windows is always a pass-through.
pub fn launchable_command(command: &[String]) -> Vec<String> {
    #[cfg(windows)]
    {
        launchable_command_in(command, &path_dirs())
    }
    #[cfg(not(windows))]
    {
        command.to_vec()
    }
}

/// [`launchable_command`] over an explicit set of `PATH` directories, so a test can
/// resolve against a temp dir of stub shims without touching the real `PATH`.
#[cfg(windows)]
pub fn launchable_command_in(command: &[String], dirs: &[PathBuf]) -> Vec<String> {
    let (program, args) = match command.split_first() {
        Some(split) => split,
        None => return command.to_vec(),
    };
    let resolved = match resolve_spawn_program(program, dirs) {
        Some(p) => p,
        // Unresolvable on PATH: leave it to the spawner to report the real error.
        None => return command.to_vec(),
    };
    let program_str = resolved.to_string_lossy().into_owned();
    if is_batch_script(&resolved) {
        // A .cmd/.bat is not a PE image; it must be interpreted by the command shell.
        let mut out = Vec::with_capacity(command.len() + 2);
        out.push("cmd.exe".to_string());
        out.push("/c".to_string());
        out.push(program_str);
        out.extend(args.iter().cloned());
        out
    } else {
        let mut out = Vec::with_capacity(command.len());
        out.push(program_str);
        out.extend(args.iter().cloned());
        out
    }
}

/// Whether a resolved program is a batch shim that must run under `cmd.exe /c`.
#[cfg(windows)]
fn is_batch_script(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("cmd") || e.eq_ignore_ascii_case("bat"))
}

/// Resolve a launch program (a bare name, or a path) to its concrete launchable file.
#[cfg(windows)]
fn resolve_spawn_program(program: &str, dirs: &[PathBuf]) -> Option<PathBuf> {
    let path = Path::new(program);
    // A program given with a directory component (absolute, or relative with a
    // separator) is resolved against that path, not searched on PATH.
    if path.is_absolute() || path.components().count() > 1 {
        return resolve_explicit_path(path);
    }
    resolve_launchable_on_path(program, dirs)
}

/// Resolve an explicit program path: the file itself, or, when it carries no extension,
/// the first launchable-extension sibling (`foo` -> `foo.cmd`).
#[cfg(windows)]
fn resolve_explicit_path(path: &Path) -> Option<PathBuf> {
    if path.is_file() {
        return Some(path.to_path_buf());
    }
    if path.extension().is_none() {
        for ext in launchable_extensions() {
            let candidate = PathBuf::from(format!("{}{ext}", path.display()));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Search `PATH` for `name`, preferring a launchable extension (`.exe`/`.cmd`/...) over
/// the extension-less shim, which is the whole point of the fix: the bare form is never
/// returned, so `CreateProcessW` never receives a script it cannot execute. A name that
/// already carries an extension (e.g. `cmd.exe`) is tried verbatim first.
#[cfg(windows)]
fn resolve_launchable_on_path(name: &str, dirs: &[PathBuf]) -> Option<PathBuf> {
    let has_ext = Path::new(name).extension().is_some();
    let exts = launchable_extensions();
    for dir in dirs {
        if has_ext {
            let verbatim = dir.join(name);
            if verbatim.is_file() {
                return Some(verbatim);
            }
        }
        for ext in &exts {
            let candidate = dir.join(format!("{name}{ext}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// The launchable executable extensions to try, in `PATHEXT` order, each with its leading
/// dot. Unlike [`executable_extensions`] this NEVER includes the bare (extension-less)
/// form: an extension-less file is exactly the shim `CreateProcessW` rejects, so the spawn
/// resolver must never fall back to it.
#[cfg(windows)]
fn launchable_extensions() -> Vec<String> {
    let mut exts: Vec<String> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    let mut push = |ext: &str| {
        let norm = ext.to_ascii_lowercase();
        if !norm.is_empty() && norm != "." && !seen.contains(&norm) {
            seen.push(norm);
            exts.push(ext.to_string());
        }
    };
    if let Some(pathext) = std::env::var_os("PATHEXT") {
        for ext in std::env::split_paths(&pathext) {
            if let Some(s) = ext.to_str() {
                push(s);
            }
        }
    }
    // Ensure the shim kinds the product cares about are always considered.
    for ext in [".exe", ".cmd", ".bat", ".com", ".ps1"] {
        push(ext);
    }
    exts
}

/// Run `<command> --version` with a hard timeout, returning the first non-empty
/// output line. stdout is preferred; some CLIs print the version to stderr, so that
/// is the fallback. A non-zero exit or a timeout yields `None` (best effort).
fn probe_version(command: &Path, timeout: Duration) -> Option<String> {
    let mut builder = Command::new(command);
    builder
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Windows: detection runs at app startup, so probing several CLIs must not flash a
    // console window for each. CREATE_NO_WINDOW keeps the version probe invisible.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        builder.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = builder.spawn().ok()?;

    // Drain both pipes on threads so a chatty child never deadlocks on a full pipe.
    let out_reader = child.stdout.take().map(spawn_reader);
    let err_reader = child.stderr.take().map(spawn_reader);

    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_status)) => break,
            Ok(None) => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(_) => break,
        }
    }

    let stdout = out_reader.and_then(|h| h.join().ok()).unwrap_or_default();
    let stderr = err_reader.and_then(|h| h.join().ok()).unwrap_or_default();
    first_nonempty_line(&stdout).or_else(|| first_nonempty_line(&stderr))
}

fn spawn_reader<R: Read + Send + 'static>(mut r: R) -> std::thread::JoinHandle<String> {
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = r.read_to_end(&mut buf);
        String::from_utf8_lossy(&buf).into_owned()
    })
}

fn first_nonempty_line(text: &str) -> Option<String> {
    text.lines().map(str::trim).find(|l| !l.is_empty()).map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caution_flags_the_danger_list() {
        assert!(caution(&["--dangerously-skip-permissions".into()]));
        assert!(caution(&["--auto".into()]));
        assert!(caution(&["--permission-mode".into(), "bypassPermissions".into()]));
        assert!(caution(&["--permission-mode=bypassPermissions".into()]));
        assert!(caution(&["-a".into(), "never".into()]));
        assert!(caution(&["--ask-for-approval".into(), "never".into()]));
    }

    #[test]
    fn caution_is_quiet_for_safe_args() {
        assert!(!caution(&[]));
        assert!(!caution(&["--permission-mode".into(), "acceptEdits".into()]));
        assert!(!caution(&["--model".into(), "opus".into()]));
        // The dangerous value alone, with no flag in front, is not the danger form.
        assert!(!caution(&["bypassPermissions".into()]));
        assert!(!caution(&["never".into()]));
    }

    #[test]
    fn detects_stub_clis_on_a_temp_dir_and_probes_version() {
        let dir = std::env::temp_dir().join(format!(
            "dflow-detect-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        // Two stub .cmd shims that echo a version; the rest of the catalog is absent.
        std::fs::write(dir.join("claude.cmd"), "@echo 9.9.9\r\n").unwrap();
        std::fs::write(dir.join("codex.cmd"), "@echo codex-cli 1.2.3\r\n").unwrap();

        let found = detect_installed_in(std::slice::from_ref(&dir));
        let claude = found.iter().find(|d| d.name == "claude").expect("claude detected");
        assert_eq!(claude.adapter, "claude");
        assert_eq!(claude.version.as_deref(), Some("9.9.9"));
        let codex = found.iter().find(|d| d.name == "codex").expect("codex detected");
        assert_eq!(codex.version.as_deref(), Some("codex-cli 1.2.3"));
        assert!(found.iter().all(|d| d.name != "opencode"), "opencode is not installed");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(windows)]
    #[test]
    fn spawn_resolution_skips_the_shim_for_a_launchable_cmd() {
        // A PATH dir holding BOTH an extension-less `#!/bin/sh` shim (the os error 193
        // trap) and a `.cmd`: the spawn resolver must pick the launchable `.cmd` and run
        // it under `cmd.exe /c`, never the bare shim (audit finding #2).
        let dir = std::env::temp_dir().join(format!(
            "dflow-spawn-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("codex"), "#!/bin/sh\necho hi\n").unwrap();
        std::fs::write(dir.join("codex.cmd"), "@echo hi\r\n").unwrap();

        let argv = launchable_command_in(
            &["codex".to_string(), "-m".to_string(), "haiku".to_string()],
            std::slice::from_ref(&dir),
        );
        assert_eq!(argv[0], "cmd.exe", "a batch shim must launch under cmd.exe");
        assert_eq!(argv[1], "/c");
        assert!(
            argv[2].to_ascii_lowercase().ends_with("codex.cmd"),
            "resolved to the launchable .cmd, not the shim: {:?}",
            argv[2]
        );
        // The original arguments are preserved after the resolved script.
        assert_eq!(&argv[3..], &["-m".to_string(), "haiku".to_string()]);
        // The resolved program is a real file CreateProcessW-launchable via cmd.exe.
        assert!(Path::new(&argv[2]).is_file());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(windows)]
    #[test]
    fn spawn_resolution_uses_a_native_exe_directly() {
        // A native `.exe` (the claude case) is launched directly, not wrapped in cmd.exe.
        let dir = std::env::temp_dir().join(format!(
            "dflow-spawn-exe-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("claude.exe"), b"MZ").unwrap();

        let argv = launchable_command_in(
            &["claude".to_string(), "--model".to_string(), "haiku".to_string()],
            std::slice::from_ref(&dir),
        );
        assert!(argv[0].to_ascii_lowercase().ends_with("claude.exe"), "native exe used directly: {argv:?}");
        assert_eq!(&argv[1..], &["--model".to_string(), "haiku".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(windows)]
    #[test]
    fn spawn_resolution_passes_through_when_unresolvable() {
        // Nothing launchable on PATH: return the command unchanged so the spawner
        // surfaces the real "not found" error instead of this masking it.
        let dir = std::env::temp_dir().join(format!(
            "dflow-spawn-none-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        // Only a bare shim, no launchable extension: not launchable, so passthrough.
        std::fs::write(dir.join("ghosttool"), "#!/bin/sh\n").unwrap();
        let argv = launchable_command_in(
            &["ghosttool".to_string(), "x".to_string()],
            std::slice::from_ref(&dir),
        );
        assert_eq!(argv, vec!["ghosttool".to_string(), "x".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_prefers_an_executable_extension() {
        let dir = std::env::temp_dir().join(format!(
            "dflow-resolve-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("pi.cmd"), "@echo pi 0.1\r\n").unwrap();
        let resolved = resolve_on_path("pi", std::slice::from_ref(&dir)).expect("pi resolved");
        assert!(resolved.file_name().unwrap().to_string_lossy().eq_ignore_ascii_case("pi.cmd"));
        assert!(resolve_on_path("absent-cli", std::slice::from_ref(&dir)).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
