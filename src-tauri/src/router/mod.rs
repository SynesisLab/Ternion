//! The Triad router (design §3): Herald classification, the deterministic
//! policy engine that wraps it, and the heuristics that answer when Herald
//! can't. Everything here is plain Rust over settings + DB state, so the
//! policy engine and parser are unit-testable without a running model.

pub mod config;
pub mod herald;
pub mod policy;
pub mod sidecars;