//! One-shot dev/evidence helper: send a single protocol request to the daemon
//! discovered via `DFLOW_DATA_DIR` and print the raw response payload.
//!
//! Used by the phase 6 live proof to set up and verify state through verbs the
//! MCP surface deliberately does not expose (project.add, daemon.shutdown).
//!
//!   cargo run -p dflow-mcp --example wirecall -- project.add '{"path":"..."}'

fn main() {
    let mut args = std::env::args().skip(1);
    let msg_type = args.next().unwrap_or_else(|| usage());
    let payload_text = args.next().unwrap_or_else(|| "{}".to_string());
    let payload: serde_json::Value = serde_json::from_str(&payload_text)
        .unwrap_or_else(|e| { eprintln!("payload is not JSON: {e}"); std::process::exit(2) });

    let mut d = dflow_mcp::daemon::Daemon::connect().unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1);
    });
    match d.request_value(&msg_type, payload) {
        Ok(value) => println!("{}", serde_json::to_string_pretty(&value).unwrap()),
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
}

fn usage() -> ! {
    eprintln!("usage: wirecall <family.verb> [json-payload]");
    std::process::exit(2);
}
