//! Sandboxed verification: checks run as argv lists against a jailed worktree
//! inside a hard sandbox (Darwin Seatbelt, Linux netns, Windows Job Object).

pub mod apply;
pub mod manifest;
pub mod runner;
pub mod sandbox;

pub use runner::{run_verification, Check, CheckResult, VerificationReport, DEFAULT_CHECK_TIMEOUT_SEC};
pub use sandbox::{production_sandbox, ArgvListSandbox, HardSandbox, SandboxOutcome, MAX_OUTPUT_CHARS};
