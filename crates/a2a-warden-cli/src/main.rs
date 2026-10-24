//! A2A Warden: the single executable.
//!
//! See every message. Verify every agent. Approve every task.
//!
//! The binary loads the configuration, connects the configured database,
//! applies migrations, registers the configured agents, and serves the A2A
//! endpoints, the administration API, and the embedded console on a single
//! port.

use clap::Parser;

use a2a_warden_cli::{init_tracing, run, Cli};

#[tokio::main]
async fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    init_tracing(cli.verbose);

    match run(cli).await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("a2a-warden: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}
