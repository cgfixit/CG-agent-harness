//! `cgagentharness` binary: `serve` (public) and `agentic` (hidden, spawned by
//! the server through the shim, never meant for interactive use).

use std::ffi::OsString;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "cgagentharness",
    version,
    about = "Loopback-only agentic coding harness console"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Serve the harness console on 127.0.0.1 (default port 8790).
    Serve {
        /// Bind host; anything but a loopback address is refused.
        #[arg(long)]
        host: Option<String>,
        /// Bind port (1024-65535).
        #[arg(long)]
        port: Option<u16>,
    },
    /// Out-of-band agentic CLI. Spawned by the server as a child process.
    #[command(hide = true, disable_help_flag = true, allow_hyphen_values = true)]
    Agentic {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<OsString>,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Serve { host, port } => match cgagentharness::server::serve_blocking(host, port) {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                eprintln!("cgagentharness: {err}");
                ExitCode::from(1)
            }
        },
        Command::Agentic { args } => {
            let code = cgagentharness::agentic::cli::main(args);
            ExitCode::from(code)
        }
    }
}
