//! CGagentHarness library crate.
//!
//! Module layout mirrors CyClaw's isolation contract (Invariant 6):
//!
//! * `common/`  - shared primitives (errors, config, atomic writes, audit, auth, ...).
//! * `llm/`     - local OpenAI-compatible backend resolution and chat client.
//! * `server/`  - the loopback HTTP console (`cgagentharness serve`).
//! * `shim/`    - the ONLY server -> agentic edge: builds an argv and spawns
//!   `current_exe() agentic <action>` as a child process.
//! * `agentic/` - the out-of-band real-repo coding pipeline, reachable only
//!   through the hidden `agentic` subcommand in a separate process.
//!
//! `server/`, `shim/`, `llm/` and `common/` must never reference `agentic::`;
//! `tests/invariant_guard.rs` asserts that at the source level.

pub mod agentic;
pub mod common;
pub mod llm;
pub mod server;
pub mod shim;

/// Crate version as compiled in.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
