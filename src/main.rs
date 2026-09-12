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
    /// Account-authenticated web operations through the running portal.
    Web {
        #[arg(long)]
        url: Option<String>,
        #[command(subcommand)]
        action: cgagentharness::server::client::WebCommand,
    },
    /// Login, change your password, or logout through the running portal.
    Account {
        #[arg(long)]
        url: Option<String>,
        #[command(subcommand)]
        action: cgagentharness::server::client::AccountCommand,
    },
    /// Inspect or explicitly renew the local TLS certificate.
    Tls {
        #[command(subcommand)]
        action: cgagentharness::server::client::TlsCommand,
    },
    /// Private desktop sidecar protocol over inherited pipes.
    #[cfg(unix)]
    #[command(hide = true)]
    Desktop,
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
        Command::Web { url, action } => report(cgagentharness::server::client::web(action, url)),
        Command::Account { url, action } => report(cgagentharness::server::client::account(action, url)),
        Command::Tls { action } => report(cgagentharness::server::client::tls(action)),
        #[cfg(unix)]
        Command::Desktop => match cgagentharness::server::desktop::run() {
            Ok(()) => ExitCode::SUCCESS,
            Err(_) => {
                // Startup diagnostics are structured and secret-free on the
                // private pipe; never print an arbitrary config/error chain.
                eprintln!("desktop backend stopped; inspect the setup window");
                ExitCode::from(3)
            }
        },
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

fn report(result: anyhow::Result<()>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("cgagentharness: {error}");
            ExitCode::from(1)
        }
    }
}
