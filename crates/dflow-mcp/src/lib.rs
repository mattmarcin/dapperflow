//! MCP server exposing DapperFlow orchestration tools to the Concertmaster
//! (`product.md` / The Concertmaster; `the design notes` sections 5-7).
//!
//! Any MCP-capable harness mounts the `dflow-mcp` binary over stdio; the server
//! talks to the local `dflowd` daemon as a WebSocket client (`protocol.md`),
//! discovered via `<data-dir>/runtime.json` with `DFLOW_DATA_DIR` honored.
//!
//! Capability scope (`security.md` / Concertmaster capability scope): merge,
//! push, discard, permission changes, vault access, and recipe install are
//! structurally absent from the tool surface; see `server.rs`.

pub mod daemon;
pub mod install;
pub mod render;
pub mod runtime;
pub mod server;
pub mod steer;
