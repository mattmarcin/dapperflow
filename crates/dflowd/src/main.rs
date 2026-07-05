//! dflowd: the DapperFlow daemon.
//!
//! Owns PTY sessions and their VT screen models, serves a loopback WebSocket to
//! the desktop app (and future clients), and keeps sessions alive across GUI
//! restarts (`architecture.md`, `protocol.md`, `security.md`).

mod api;
mod artifact;
mod conn;
mod control;
mod gate;
mod github;
mod hooks;
mod lan;
mod recipes;
mod runtime;
mod server;
mod tokens;

use anyhow::Result;
use runtime::Runtime;

const VERSION: &str = concat!("dflowd ", env!("CARGO_PKG_VERSION"));

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();

    // `--data-dir <dir>` targets a specific daemon; map it onto DFLOW_DATA_DIR so both
    // the control paths and daemon startup resolve the same location.
    if let Some(dir) = arg_value(&args, "--data-dir") {
        std::env::set_var("DFLOW_DATA_DIR", dir);
    }

    // `--version`/`-V`: print the build string and exit. The desktop app runs this on the
    // bundled and the installed daemon to decide whether to refresh its managed copy
    // (`daemon-lifecycle.md` / Production: copy when the bundled version is newer).
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("{VERSION}");
        return Ok(());
    }

    // Lifecycle control verbs exit without starting a daemon.
    if args.iter().any(|a| a == "--status") {
        return control::status();
    }
    if args.iter().any(|a| a == "--stop") {
        let code = control::stop()?;
        std::process::exit(code);
    }
    if args.iter().any(|a| a == "--pair") {
        let code = control::pair()?;
        std::process::exit(code);
    }

    run_daemon()
}

/// Parse `--flag value` or `--flag=value` from argv.
fn arg_value(args: &[String], flag: &str) -> Option<String> {
    let prefix = format!("{flag}=");
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == flag {
            return iter.next().cloned();
        }
        if let Some(value) = arg.strip_prefix(&prefix) {
            return Some(value.to_string());
        }
    }
    None
}

#[tokio::main]
async fn run_daemon() -> Result<()> {
    init_tracing();

    let runtime = match Runtime::acquire(VERSION) {
        Ok(rt) => rt,
        Err(err) => {
            // Another instance already owns the lock: this is a normal outcome when
            // the app tries to start a daemon that is already running.
            tracing::info!(%err, "not starting: {err}");
            println!("dflowd not started: {err}");
            return Ok(());
        }
    };

    // Total reaping foundation (`daemon-lifecycle.md`): assign this daemon process to a
    // kill-on-close Job Object BEFORE any session (and its ConPTY console host) can be
    // spawned, so every descendant dies with the daemon however it dies. Non-fatal: on
    // failure the per-session kill guards still bound each tree.
    if let Err(err) = dflow_core::install_process_reaping_job() {
        tracing::error!(%err, "could not install the process reaping job; a hard daemon kill may orphan ConPTY hosts");
    } else {
        tracing::info!("process reaping job installed: children die with the daemon");
    }

    tracing::info!(dir = %runtime.dir().display(), "dflowd starting");
    server::run(runtime, VERSION.to_string()).await
}

fn init_tracing() {
    use tracing_subscriber::{fmt, EnvFilter};
    let filter = EnvFilter::try_from_env("DFLOW_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    fmt().with_env_filter(filter).with_target(false).init();
}
