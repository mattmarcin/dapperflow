//! CLI errors and the specced exit codes (`agent-cli.md` / Exit codes).
//!
//! 0 success; 1 structured operational error; 2 usage error; 3 not in a dispatched
//! context; 4 token expired/revoked. Every error prints a structured message and a
//! `next:` line to stderr, never an interactive prompt (AXI design rule 4).

use dflow_proto::ErrorCode;

/// A CLI failure carrying its exit code and a `next:` hint.
#[derive(Debug)]
pub struct CliError {
    pub code: i32,
    pub message: String,
    pub next: String,
}

impl CliError {
    /// Exit code 3: the CLI is not running inside a dispatched DapperFlow session.
    pub fn not_dispatched(message: impl Into<String>) -> Self {
        CliError {
            code: 3,
            message: message.into(),
            next: "run `dflow` only inside a DapperFlow-dispatched session".into(),
        }
    }

    /// Exit code 4: the per-task token is expired or revoked.
    pub fn revoked(message: impl Into<String>) -> Self {
        CliError {
            code: 4,
            message: message.into(),
            next: "this task's token is no longer valid; the session has ended or been torn down".into(),
        }
    }

    /// Exit code 2: a usage error (bad arguments).
    pub fn usage(message: impl Into<String>) -> Self {
        CliError {
            code: 2,
            message: message.into(),
            next: "run `dflow help <verb>` for the correct form".into(),
        }
    }

    /// Exit code 1: a structured operational error.
    pub fn operational(message: impl Into<String>, next: impl Into<String>) -> Self {
        CliError { code: 1, message: message.into(), next: next.into() }
    }

    /// Map a daemon error envelope (`code` + `message`) to a CLI error with the right
    /// exit code and a `next:` hint tuned to the code.
    pub fn from_daemon(code: ErrorCode, message: String) -> Self {
        match code {
            // A bad request from the daemon is a usage error from the agent's side.
            ErrorCode::BadRequest => CliError::usage(message),
            ErrorCode::BudgetExceeded => CliError::operational(
                message,
                "record the remaining items in your final report",
            ),
            ErrorCode::Forbidden => CliError::operational(
                message,
                "this surface is outside your task's scope; only your own card, session, and project are reachable",
            ),
            ErrorCode::NotFound => {
                CliError::operational(message, "verify the id, or run `dflow` to see your card")
            }
            ErrorCode::AuthFailed => CliError::revoked(message),
            other => CliError::operational(message, format!("operational error ({other:?}); retry or report it")),
        }
    }

    /// Print the structured error and `next:` line to stderr.
    pub fn emit(&self) {
        eprintln!("error: {}", self.message);
        eprintln!("next: {}", self.next);
    }
}
