//! Tokio TCP proxy dataplane.
//!
//! Portable mode forwards `listen -> upstream` with bounded concurrency,
//! backpressure via TCP windows + [`tokio::io::copy_bidirectional`], idle
//! and connect timeouts, and graceful shutdown. Transparent (TPROXY) mode
//! is isolated in [`transparent`] and never required for tests.

pub mod config;
pub mod server;
pub mod transparent;

pub use config::ProxyConfig;
pub use server::{ConnectionCounters, ProxyServer, ProxyStats};
