//! Edge fleet agent: outbound-only control-plane sync + local dataplane.
//!
//! Invariant: loss of cloud connectivity must not stop local forwarding.
//! The agent therefore keeps last-known-good policy on disk ([`edge_state`])
//! and runs the proxy unconditionally; cloud sync only *updates* that
//! policy. Reconnection uses jittered exponential backoff.

#![allow(clippy::result_large_err)]

pub mod agent;

pub use agent::{AgentConfig, EdgeAgent};
