//! Name of the optional harness API key.
//!
//! The key is compatibility metadata whose presence is reported; it never
//! grants account, provider or coding authority. Legacy key-required mode is
//! deprecated and ignored (`INVARIANTS.md`, account and request boundaries).

pub const API_KEY_ENV: &str = "CGAGENTHARNESS_API_KEY";
