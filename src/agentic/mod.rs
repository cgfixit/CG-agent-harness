//! Out-of-band agentic pipeline, reachable only through the hidden `agentic`
//! subcommand in a separate process. Never references `crate::server` or
//! `crate::shim`.

pub mod cli;
pub mod cloud_proposer;
pub mod commands;
pub mod config;
pub mod context;
pub mod ctx;
pub mod edits;
pub mod executor;
pub mod gh_client;
pub mod governance;
pub mod proposer;
pub mod real_repo_loop;
pub mod registry;
pub mod run_store;
pub mod unslop;
pub mod workspace;
pub mod writer;
