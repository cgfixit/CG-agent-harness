//! `cgagentharness` binary: `serve` (public) and `agentic` (hidden, spawned by
//! the server through the shim, never meant for interactive use).

use std::ffi::OsString;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "cgagentharness",
    version,
    about = "Private local chat, permitted web research, and governed coding",
    after_help = "QUICK START\n  cgagentharness serve                 Start the local console\n  cgagentharness account login NAME    Sign in to the running portal\n  cgagentharness web --help            URL permissions and research\n\nIn the console, /help opens the searchable command guide; /help web shows research examples.\nData lives in ~/.CGagentHarness (override: CGAGENTHARNESS_HOME). No web content is permitted until an administrator adds URL rules."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Manage dedicated machine keys using trusted local filesystem authority.
    McpKey {
        #[command(subcommand)]
        action: cgagentharness::server::mcp_keys::KeyCommand,
    },
    /// Private supervised MCP stdio worker; no network listener.
    #[command(hide = true)]
    McpStdioWorker,
    /// Account-authenticated web operations through the running portal.
    #[command(
        after_help = "EXAMPLES (quote wildcard patterns in your shell)\n  cgagentharness web allow 'https://www.veeam.com/*' 'https://example.org/*' --group vendors\n  cgagentharness web check https://www.veeam.com/\n  cgagentharness web research --group vendors 'Compare the backup approaches'\n  cgagentharness web research --url https://www.veeam.com/ 'Summarize products and permitted internal links'\n\nRules and web on/off are shared by this home, not per session. Selected web context is account-private. *.example.org excludes example.org; exact www and apex hosts are distinct. Every destination needs a matching rule; redirects and private addresses are refused."
    )]
    Web {
        /// Portal origin (not a research target), for example https://127.0.0.1:8790.
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
        Command::McpKey { action } => report(cgagentharness::server::mcp_keys::run(action)),
        Command::McpStdioWorker => ExitCode::from(cgagentharness::common::mcp_worker::main()),
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
