//! `dflow-mcp`: the DapperFlow MCP server binary.
//!
//! - `dflow-mcp` / `dflow-mcp serve`: run the MCP server over stdio (what a
//!   harness mounts). All logging goes to stderr: stdout is the transport.
//! - `dflow-mcp install <harness> [--write]`: print (or write) the mount
//!   config for claude / codex / opencode, or generic stdio instructions.

use rmcp::{transport::stdio, ServiceExt};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None | Some("serve") => serve(),
        Some("install") => install(&args[1..]),
        Some("help") | Some("--help") | Some("-h") => {
            print!("{}", help());
            Ok(())
        }
        Some(other) => {
            eprintln!("unknown command `{other}`\n\n{}", help());
            std::process::exit(2);
        }
    }
}

fn help() -> &'static str {
    "dflow-mcp - the DapperFlow MCP server (stdio)\n\n\
     \x20 dflow-mcp [serve]              run the MCP server over stdio\n\
     \x20 dflow-mcp install <harness>    print mount config (claude|codex|opencode|<other>)\n\
     \x20 dflow-mcp install <harness> --write   write it (project .mcp.json / opencode.json;\n\
     \x20                                        codex: append to ~/.codex/config.toml)\n\n\
     The server finds the DapperFlow daemon via <data-dir>/runtime.json;\n\
     DFLOW_DATA_DIR overrides the data dir.\n"
}

/// Run the MCP server over stdio until the client disconnects.
fn serve() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("DFLOW_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();

    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    rt.block_on(async {
        tracing::info!("dflow-mcp serving over stdio");
        let service = dflow_mcp::server::DflowMcp::new().serve(stdio()).await?;
        service.waiting().await?;
        Ok(())
    })
}

/// `install <harness> [--write]`.
fn install(args: &[String]) -> anyhow::Result<()> {
    let harness = match args.first().filter(|a| !a.starts_with("--")) {
        Some(h) => h.clone(),
        None => {
            eprintln!("usage: dflow-mcp install <claude|codex|opencode|...> [--write]");
            std::process::exit(2);
        }
    };
    let write = args.iter().any(|a| a == "--write");
    let exe = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "dflow-mcp".into());
    let data_dir = std::env::var("DFLOW_DATA_DIR").ok().filter(|v| !v.trim().is_empty());

    let out = dflow_mcp::install::render(&harness, &exe, data_dir.as_deref());
    print!("{}", out.text);
    if write {
        let cwd = std::env::current_dir()?;
        match dflow_mcp::install::write(&out.plan, &exe, data_dir.as_deref(), &cwd) {
            Ok(summary) => println!("{summary}"),
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        }
    }
    Ok(())
}
