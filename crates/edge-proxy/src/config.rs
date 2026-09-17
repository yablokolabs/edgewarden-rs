//! Proxy configuration: portable mode by default, TPROXY opt-in.

use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyConfig {
    pub listen_addr: SocketAddr,
    pub upstream_addr: SocketAddr,
    #[serde(default = "default_max_connections")]
    pub max_connections: usize,
    #[serde(default = "default_connect_timeout_secs")]
    pub connect_timeout_secs: u64,
    #[serde(default = "default_idle_timeout_secs")]
    pub idle_timeout_secs: u64,
    /// Linux transparent-proxy mode (requires CAP_NET_ADMIN + nftables).
    /// Defaults off; normal dev mode runs without TPROXY.
    #[serde(default)]
    pub transparent: bool,
}

fn default_max_connections() -> usize {
    512
}
fn default_connect_timeout_secs() -> u64 {
    5
}
fn default_idle_timeout_secs() -> u64 {
    300
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            listen_addr: "127.0.0.1:18080".parse().unwrap(),
            upstream_addr: "127.0.0.1:18081".parse().unwrap(),
            max_connections: default_max_connections(),
            connect_timeout_secs: default_connect_timeout_secs(),
            idle_timeout_secs: default_idle_timeout_secs(),
            transparent: false,
        }
    }
}

impl ProxyConfig {
    pub fn connect_timeout(&self) -> Duration {
        Duration::from_secs(self.connect_timeout_secs)
    }
    pub fn idle_timeout(&self) -> Duration {
        Duration::from_secs(self.idle_timeout_secs)
    }
}
